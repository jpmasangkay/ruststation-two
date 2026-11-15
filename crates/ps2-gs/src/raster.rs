//! Non-textured GS software rasterizer (Phase 2b Step 2).
//!
//! Implements vertex assembly (`XYZ2`/`XYZF2` draw kick, `XYZ3`/`XYZF3` vertex-only / ADC),
//! flat & Gouraud shading (`PRIM.IIP` / `PRMODECONT`), `XYOFFSET`, `SCISSOR`, `ZBUF` &
//! `TEST.ZTE`/`ZTST`/`ZMSK`, and `FRAME` (`FBMSK` + 32/24/16-bit formats). All pixel
//! coverage tests delegate exclusively to [`crate::coverage::is_pixel_covered_active`].

use tracing::warn;

use crate::coverage::{
    edge_fn, is_pixel_covered_active, pixel_sample_fp, snap_coord_to_pixel, CoveragePrimitive,
    ACTIVE_COVERAGE,
};
use crate::regs::{ContextRegs, GsRegs, PrimReg, PrimitiveType, XyzReg};
use crate::vram::{GsVram, PixelFormat};

/// Latched per-vertex attributes captured on a GS vertex kick (`XYZ2/3` or `XYZF2/3`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GsVertex {
    pub x_raw: u16,
    pub y_raw: u16,
    pub z: u32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
    pub fog: u8,
    pub s_bits: u32,
    pub t_bits: u32,
    pub q_bits: u32,
    pub u_raw: u16,
    pub v_raw: u16,
}

impl GsVertex {
    fn from_regs(regs: &GsRegs, xyz_raw: u64, has_fog: bool) -> Self {
        let xyz = if has_fog {
            XyzReg::from_xyzf(xyz_raw)
        } else {
            XyzReg::from_xyz(xyz_raw)
        };
        Self {
            x_raw: xyz.x_raw,
            y_raw: xyz.y_raw,
            z: xyz.z,
            r: regs.rgbaq.r(),
            g: regs.rgbaq.g(),
            b: regs.rgbaq.b(),
            a: regs.rgbaq.a(),
            fog: xyz.fog.unwrap_or_else(|| regs.fog.f()),
            s_bits: regs.st.s_bits(),
            t_bits: regs.st.t_bits(),
            q_bits: regs.rgbaq.q_bits(),
            u_raw: regs.uv.u_raw(),
            v_raw: regs.uv.v_raw(),
        }
    }
}

/// Vertex assembly queue and non-textured primitive rasterizer state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterizerState {
    prim_type: PrimitiveType,
    queue: [GsVertex; 3],
    queue_len: u8,
    warned_unsupported_features: bool,
}

impl Default for RasterizerState {
    fn default() -> Self {
        Self::new()
    }
}

impl RasterizerState {
    pub const fn new() -> Self {
        Self {
            prim_type: PrimitiveType::Point,
            queue: [GsVertex {
                x_raw: 0,
                y_raw: 0,
                z: 0,
                r: 0,
                g: 0,
                b: 0,
                a: 0,
                fog: 0,
                s_bits: 0,
                t_bits: 0,
                q_bits: 0,
                u_raw: 0,
                v_raw: 0,
            }; 3],
            queue_len: 0,
            warned_unsupported_features: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Update active primitive topology on a `PRIM` (`0x00`) write and clear pending vertices.
    ///
    /// Note (`Play! GSH_OpenGL.cpp` BSD-2): `PRMODE`/`PRMODECONT` alter attributes without resetting
    /// the vertex queue, whereas every `PRIM` write restarts primitive assembly.
    #[inline]
    pub fn on_prim_write(&mut self, prim: PrimReg) {
        self.prim_type = prim.prim_type();
        self.queue_len = 0;
    }

    /// Process a vertex kick (`XYZ2`/`XYZF2` with `drawing_kick = true`, `XYZ3`/`XYZF3` with `false`).
    pub fn vertex_kick(
        &mut self,
        regs: &GsRegs,
        vram: &mut GsVram,
        xyz_raw: u64,
        has_fog: bool,
        drawing_kick: bool,
    ) {
        let required: u8 = match self.prim_type {
            PrimitiveType::Point => 1,
            PrimitiveType::Line | PrimitiveType::LineStrip | PrimitiveType::Sprite => 2,
            PrimitiveType::Triangle | PrimitiveType::TriangleStrip | PrimitiveType::TriangleFan => {
                3
            }
            PrimitiveType::Reserved => return,
        };

        let vertex = GsVertex::from_regs(regs, xyz_raw, has_fog);
        let idx = usize::from(self.queue_len);
        if idx < self.queue.len() {
            self.queue[idx] = vertex;
            self.queue_len += 1;
        }

        if self.queue_len < required {
            return;
        }

        if drawing_kick {
            let eff_prim = regs.effective_prim();
            if (eff_prim.tme() || eff_prim.fge() || eff_prim.abe() || eff_prim.aa1())
                && !self.warned_unsupported_features
            {
                self.warned_unsupported_features = true;
                warn!(
                    prim = eff_prim.0,
                    "Step 2 non-textured rasterizer ignoring TME/FGE/ABE/AA1 until Step 3"
                );
            }
            self.draw_assembled(regs, vram, eff_prim);
        }

        match self.prim_type {
            PrimitiveType::Point
            | PrimitiveType::Line
            | PrimitiveType::Triangle
            | PrimitiveType::Sprite => {
                self.queue_len = 0;
            }
            PrimitiveType::LineStrip => {
                self.queue[0] = self.queue[1];
                self.queue_len = 1;
            }
            PrimitiveType::TriangleStrip => {
                self.queue[0] = self.queue[1];
                self.queue[1] = self.queue[2];
                self.queue_len = 2;
            }
            PrimitiveType::TriangleFan => {
                self.queue[1] = self.queue[2];
                self.queue_len = 2;
            }
            PrimitiveType::Reserved => {
                self.queue_len = 0;
            }
        }
    }

    fn draw_assembled(&self, regs: &GsRegs, vram: &mut GsVram, eff_prim: PrimReg) {
        let ctx = &regs.ctx[eff_prim.ctxt()];
        let Some(fb_fmt) = PixelFormat::from_raw(ctx.frame.psm_raw()) else {
            warn!(
                psm = ctx.frame.psm_raw(),
                "ignoring draw to unknown FRAME.PSM"
            );
            return;
        };
        if !is_renderable_frame_format(fb_fmt) {
            warn!(?fb_fmt, "ignoring draw to non-renderable FRAME.PSM");
            return;
        }

        let scax0 = i32::from(ctx.scissor.scax0());
        let scax1 = i32::from(ctx.scissor.scax1());
        let scay0 = i32::from(ctx.scissor.scay0());
        let scay1 = i32::from(ctx.scissor.scay1());
        if scax0 > scax1 || scay0 > scay1 {
            return;
        }

        let target = RenderTarget {
            ctx,
            scissor: [scax0, scax1, scay0, scay1],
            fb_fmt,
            z_fmt: PixelFormat::from_zbuf_raw(ctx.zbuf.zsm_raw()),
        };
        let gouraud = eff_prim.iip();

        match self.prim_type {
            PrimitiveType::Point => {
                raster_point(target, vram, self.queue[0]);
            }
            PrimitiveType::Line | PrimitiveType::LineStrip => {
                raster_line(target, vram, self.queue[0], self.queue[1], gouraud);
            }
            PrimitiveType::Sprite => {
                raster_sprite(target, vram, self.queue[0], self.queue[1]);
            }
            PrimitiveType::Triangle | PrimitiveType::TriangleStrip | PrimitiveType::TriangleFan => {
                raster_triangle(
                    target,
                    vram,
                    [self.queue[0], self.queue[1], self.queue[2]],
                    gouraud,
                );
            }
            PrimitiveType::Reserved => {}
        }
    }
}

#[inline]
const fn is_renderable_frame_format(fmt: PixelFormat) -> bool {
    matches!(
        fmt,
        PixelFormat::Psmct32
            | PixelFormat::Psmct24
            | PixelFormat::Psmct16
            | PixelFormat::Psmct16s
            | PixelFormat::Psmz32
            | PixelFormat::Psmz24
            | PixelFormat::Psmz16
            | PixelFormat::Psmz16s
    )
}

#[inline]
fn to_screen_fp(ctx: &ContextRegs, v: GsVertex) -> (i32, i32) {
    (
        i32::from(v.x_raw) - i32::from(ctx.xyoffset.ofx_raw()),
        i32::from(v.y_raw) - i32::from(ctx.xyoffset.ofy_raw()),
    )
}

#[inline]
fn clipped_bbox(
    min_x_fp: i32,
    max_x_fp: i32,
    min_y_fp: i32,
    max_y_fp: i32,
    scissor: [i32; 4],
) -> (i32, i32, i32, i32) {
    // Expand by +/- 1 pixel before applying exact coverage so bbox pruning never assumes a sample offset.
    let min_x = (min_x_fp.div_euclid(16) - 1).max(scissor[0]);
    let max_x = (max_x_fp.div_euclid(16) + 1).min(scissor[1]);
    let min_y = (min_y_fp.div_euclid(16) - 1).max(scissor[2]);
    let max_y = (max_y_fp.div_euclid(16) + 1).min(scissor[3]);
    (min_x, max_x, min_y, max_y)
}

#[inline]
const fn z_format_mask(fmt: PixelFormat) -> u32 {
    match fmt {
        PixelFormat::Psmz24 => 0x00FF_FFFF,
        PixelFormat::Psmz16 | PixelFormat::Psmz16s => 0x0000_FFFF,
        _ => 0xFFFF_FFFF,
    }
}

/// Convert 32-bit RGBA (`R | G<<8 | B<<16 | A<<24`) to GS 16-bit `RGBA5551` (`A1 B5 G5 R5`).
#[inline]
pub const fn pack_rgba32_to_rgb5551(rgba32: u32) -> u16 {
    let r5 = ((rgba32 >> 3) & 0x1F) as u16;
    let g5 = ((rgba32 >> 11) & 0x1F) as u16;
    let b5 = ((rgba32 >> 19) & 0x1F) as u16;
    let a1 = ((rgba32 >> 31) & 0x01) as u16;
    r5 | (g5 << 5) | (b5 << 10) | (a1 << 15)
}

/// Expand GS 16-bit `RGBA5551` back into 32-bit RGBA positions for `FBMSK` masking before format conversion.
#[inline]
pub const fn expand_rgb5551_to_rgba32(pixel16: u16) -> u32 {
    let raw = pixel16 as u32;
    let r8 = (raw & 0x1F) << 3;
    let g8 = ((raw >> 5) & 0x1F) << 3;
    let b8 = ((raw >> 10) & 0x1F) << 3;
    let a8 = ((raw >> 15) & 0x01) << 7;
    r8 | (g8 << 8) | (b8 << 16) | (a8 << 24)
}

#[inline]
const fn pack_rgba32(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: u128, steps: u128) -> u8 {
    (((steps - t) * u128::from(a) + t * u128::from(b)) / steps) as u8
}

#[inline]
fn lerp_u32(a: u32, b: u32, t: u128, steps: u128) -> u32 {
    (((steps - t) * u128::from(a) + t * u128::from(b)) / steps) as u32
}

#[inline]
fn bilerp_u8(a0: u8, a1: u8, a2: u8, w0: u128, w1: u128, w2: u128, denom: u128) -> u8 {
    ((w0 * u128::from(a0) + w1 * u128::from(a1) + w2 * u128::from(a2)) / denom) as u8
}

#[derive(Copy, Clone)]
struct RenderTarget<'a> {
    ctx: &'a ContextRegs,
    scissor: [i32; 4],
    fb_fmt: PixelFormat,
    z_fmt: Option<PixelFormat>,
}

fn shade_pixel(target: RenderTarget<'_>, vram: &mut GsVram, px: u32, py: u32, rgba32: u32, z: u32) {
    let ctx = target.ctx;
    let fbp = ctx.frame.fbp();
    let fbw = ctx.frame.fbw();
    let zbp = ctx.zbuf.zbp();

    // 1. Z-buffer test (`TEST.ZTE` and `TEST.ZTST`, `ps2tek` `#gstestsandpixelcontrol`).
    if ctx.test.zte() {
        match ctx.test.ztst() {
            0 => return, // NEVER
            1 => {}      // ALWAYS
            2 | 3 => {
                if let Some(zf) = target.z_fmt {
                    let mask = z_format_mask(zf);
                    let test_z = z & mask;
                    let buf_z = vram.read_page_pixel_raw(zf, zbp, fbw, px, py) & mask;
                    let passed = if ctx.test.ztst() == 2 {
                        test_z >= buf_z
                    } else {
                        test_z > buf_z
                    };
                    if !passed {
                        return;
                    }
                }
            }
            _ => {}
        }

        // 2. Z-buffer update unless masked by `ZBUF.ZMSK`.
        if !ctx.zbuf.zmsk() {
            if let Some(zf) = target.z_fmt {
                let mask = z_format_mask(zf);
                vram.write_page_pixel_raw(zf, zbp, fbw, px, py, z & mask);
            }
        }
    }

    // 3. Framebuffer write (`FRAME.FBMSK` applied before format conversion per `ps2tek`).
    let fbmsk = ctx.frame.fbmsk();
    if fbmsk == 0xFFFF_FFFF {
        return;
    }

    let fb_fmt = target.fb_fmt;
    match fb_fmt {
        PixelFormat::Psmct32 | PixelFormat::Psmz32 => {
            let final_color = if fbmsk == 0 {
                rgba32
            } else {
                let old = vram.read_page_pixel_raw(fb_fmt, fbp, fbw, px, py);
                (rgba32 & !fbmsk) | (old & fbmsk)
            };
            vram.write_page_pixel_raw(fb_fmt, fbp, fbw, px, py, final_color);
        }
        PixelFormat::Psmct24 | PixelFormat::Psmz24 => {
            let final_24 = if fbmsk == 0 {
                rgba32 & 0x00FF_FFFF
            } else {
                let old = vram.read_page_pixel_raw(fb_fmt, fbp, fbw, px, py);
                ((rgba32 & !fbmsk) | (old & fbmsk)) & 0x00FF_FFFF
            };
            vram.write_page_pixel_raw(fb_fmt, fbp, fbw, px, py, final_24);
        }
        PixelFormat::Psmct16
        | PixelFormat::Psmct16s
        | PixelFormat::Psmz16
        | PixelFormat::Psmz16s => {
            let masked_32 = if fbmsk == 0 {
                rgba32
            } else {
                let old_16 = vram.read_page_pixel_raw(fb_fmt, fbp, fbw, px, py) as u16;
                let old_32 = expand_rgb5551_to_rgba32(old_16);
                (rgba32 & !fbmsk) | (old_32 & fbmsk)
            };
            let packed_16 = u32::from(pack_rgba32_to_rgb5551(masked_32));
            vram.write_page_pixel_raw(fb_fmt, fbp, fbw, px, py, packed_16);
        }
        _ => {}
    }
}

fn raster_point(target: RenderTarget<'_>, vram: &mut GsVram, v: GsVertex) {
    let (x_fp, y_fp) = to_screen_fp(target.ctx, v);
    let prim = CoveragePrimitive::Point { x_fp, y_fp };
    let (min_x, max_x, min_y, max_y) = clipped_bbox(x_fp, x_fp, y_fp, y_fp, target.scissor);
    let rgba32 = pack_rgba32(v.r, v.g, v.b, v.a);

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if is_pixel_covered_active(&prim, px, py) {
                shade_pixel(target, vram, px as u32, py as u32, rgba32, v.z);
            }
        }
    }
}

fn raster_line(
    target: RenderTarget<'_>,
    vram: &mut GsVram,
    v0: GsVertex,
    v1: GsVertex,
    gouraud: bool,
) {
    let (x0_fp, y0_fp) = to_screen_fp(target.ctx, v0);
    let (x1_fp, y1_fp) = to_screen_fp(target.ctx, v1);
    let prim = CoveragePrimitive::Line {
        x0_fp,
        y0_fp,
        x1_fp,
        y1_fp,
    };

    let ix0 = snap_coord_to_pixel(x0_fp, ACTIVE_COVERAGE);
    let iy0 = snap_coord_to_pixel(y0_fp, ACTIVE_COVERAGE);
    let ix1 = snap_coord_to_pixel(x1_fp, ACTIVE_COVERAGE);
    let iy1 = snap_coord_to_pixel(y1_fp, ACTIVE_COVERAGE);
    let adx = ix1.abs_diff(ix0) as u128;
    let ady = iy1.abs_diff(iy0) as u128;
    let steps = adx.max(ady);
    let flat_rgba32 = pack_rgba32(v1.r, v1.g, v1.b, v1.a);

    let (min_x, max_x, min_y, max_y) = clipped_bbox(
        x0_fp.min(x1_fp),
        x0_fp.max(x1_fp),
        y0_fp.min(y1_fp),
        y0_fp.max(y1_fp),
        target.scissor,
    );

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if !is_pixel_covered_active(&prim, px, py) {
                continue;
            }
            let (rgba32, z) = if steps == 0 {
                (flat_rgba32, v1.z)
            } else {
                let t = if adx >= ady {
                    px.abs_diff(ix0) as u128
                } else {
                    py.abs_diff(iy0) as u128
                };
                let z = lerp_u32(v0.z, v1.z, t, steps);
                let color = if gouraud {
                    pack_rgba32(
                        lerp_u8(v0.r, v1.r, t, steps),
                        lerp_u8(v0.g, v1.g, t, steps),
                        lerp_u8(v0.b, v1.b, t, steps),
                        lerp_u8(v0.a, v1.a, t, steps),
                    )
                } else {
                    flat_rgba32
                };
                (color, z)
            };
            shade_pixel(target, vram, px as u32, py as u32, rgba32, z);
        }
    }
}

fn raster_sprite(target: RenderTarget<'_>, vram: &mut GsVram, v0: GsVertex, v1: GsVertex) {
    let (x0_fp, y0_fp) = to_screen_fp(target.ctx, v0);
    let (x1_fp, y1_fp) = to_screen_fp(target.ctx, v1);
    let min_x_fp = x0_fp.min(x1_fp);
    let max_x_fp = x0_fp.max(x1_fp);
    let min_y_fp = y0_fp.min(y1_fp);
    let max_y_fp = y0_fp.max(y1_fp);

    let prim = CoveragePrimitive::Sprite {
        min_x_fp,
        min_y_fp,
        max_x_fp,
        max_y_fp,
    };
    let (min_x, max_x, min_y, max_y) =
        clipped_bbox(min_x_fp, max_x_fp, min_y_fp, max_y_fp, target.scissor);

    // `Play! GSH_OpenGL.cpp` (`BSD-2`, `FETCHED`): sprite uses final vertex `v1`'s color & Z.
    let rgba32 = pack_rgba32(v1.r, v1.g, v1.b, v1.a);
    let z = v1.z;

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if is_pixel_covered_active(&prim, px, py) {
                shade_pixel(target, vram, px as u32, py as u32, rgba32, z);
            }
        }
    }
}

fn raster_triangle(
    target: RenderTarget<'_>,
    vram: &mut GsVram,
    mut verts: [GsVertex; 3],
    gouraud: bool,
) {
    // Capture provoking (last) vertex color before any CW vertex swap (`Play! GSH_OpenGL.cpp`).
    let flat_rgba32 = pack_rgba32(verts[2].r, verts[2].g, verts[2].b, verts[2].a);

    let (x0_fp, y0_fp) = to_screen_fp(target.ctx, verts[0]);
    let (x1_fp, y1_fp) = to_screen_fp(target.ctx, verts[1]);
    let (x2_fp, y2_fp) = to_screen_fp(target.ctx, verts[2]);

    let prim = CoveragePrimitive::Triangle {
        x0_fp,
        y0_fp,
        x1_fp,
        y1_fp,
        x2_fp,
        y2_fp,
    };

    let (x0, y0) = (i64::from(x0_fp), i64::from(y0_fp));
    let (mut x1, mut y1) = (i64::from(x1_fp), i64::from(y1_fp));
    let (mut x2, mut y2) = (i64::from(x2_fp), i64::from(y2_fp));

    let mut area2 = edge_fn(x0, y0, x1, y1, x2, y2);
    if area2 == 0 {
        return;
    }
    if area2 < 0 {
        area2 = -area2;
        verts.swap(1, 2);
        core::mem::swap(&mut x1, &mut x2);
        core::mem::swap(&mut y1, &mut y2);
    }
    let denom = area2 as u128;

    let (min_x, max_x, min_y, max_y) = clipped_bbox(
        x0_fp.min(x1_fp).min(x2_fp),
        x0_fp.max(x1_fp).max(x2_fp),
        y0_fp.min(y1_fp).min(y2_fp),
        y0_fp.max(y1_fp).max(y2_fp),
        target.scissor,
    );

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if !is_pixel_covered_active(&prim, px, py) {
                continue;
            }
            let (cx, cy) = pixel_sample_fp(px, py, ACTIVE_COVERAGE);
            let w0 = edge_fn(x1, y1, x2, y2, cx, cy).max(0) as u128;
            let w1 = edge_fn(x2, y2, x0, y0, cx, cy).max(0) as u128;
            let w2 = edge_fn(x0, y0, x1, y1, cx, cy).max(0) as u128;

            let z = ((w0 * u128::from(verts[0].z)
                + w1 * u128::from(verts[1].z)
                + w2 * u128::from(verts[2].z))
                / denom) as u32;

            let rgba32 = if gouraud {
                let r = bilerp_u8(verts[0].r, verts[1].r, verts[2].r, w0, w1, w2, denom);
                let g = bilerp_u8(verts[0].g, verts[1].g, verts[2].g, w0, w1, w2, denom);
                let b = bilerp_u8(verts[0].b, verts[1].b, verts[2].b, w0, w1, w2, denom);
                let a = bilerp_u8(verts[0].a, verts[1].a, verts[2].a, w0, w1, w2, denom);
                pack_rgba32(r, g, b, a)
            } else {
                flat_rgba32
            };

            shade_pixel(target, vram, px as u32, py as u32, rgba32, z);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::regs::gs_reg;
    use crate::{GsState, PixelFormat};

    fn pack_xyz(x_px: i32, y_px: i32, z: u32) -> u64 {
        let x = ((x_px * 16) as u16) as u64;
        let y = ((y_px * 16) as u16) as u64;
        x | (y << 16) | (u64::from(z) << 32)
    }

    fn pack_xyz_fp(x_fp: i32, y_fp: i32, z: u32) -> u64 {
        let x = (x_fp as u16) as u64;
        let y = (y_fp as u16) as u64;
        x | (y << 16) | (u64::from(z) << 32)
    }

    fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u64 {
        u64::from(r)
            | (u64::from(g) << 8)
            | (u64::from(b) << 16)
            | (u64::from(a) << 24)
            | (0x3F80_0000u64 << 32)
    }

    fn pack_scissor(x0: u64, x1: u64, y0: u64, y1: u64) -> u64 {
        x0 | (x1 << 16) | (y0 << 32) | (y1 << 48)
    }

    fn pack_frame(fbp: u64, fbw: u64, psm: u64, fbmsk: u64) -> u64 {
        fbp | (fbw << 16) | (psm << 24) | (fbmsk << 32)
    }

    fn pack_zbuf(zbp: u64, zsm: u64, zmsk: u64) -> u64 {
        zbp | (zsm << 24) | (zmsk << 32)
    }

    fn pack_test_z(zte: bool, ztst: u64) -> u64 {
        ((u64::from(zte)) << 16) | (ztst << 17)
    }

    fn setup_basic_32(gs: &mut GsState, w: u64, h: u64) {
        gs.write_internal_reg(gs_reg::PRMODECONT, 1);
        gs.write_internal_reg(gs_reg::FRAME_1, pack_frame(0, 1, 0x00, 0));
        gs.write_internal_reg(gs_reg::ZBUF_1, pack_zbuf(64, 0x00, 0));
        gs.write_internal_reg(gs_reg::SCISSOR_1, pack_scissor(0, w - 1, 0, h - 1));
        gs.write_internal_reg(gs_reg::XYOFFSET_1, 0);
        gs.write_internal_reg(gs_reg::TEST_1, pack_test_z(true, 1)); // ZTE=1, ZTST=ALWAYS
    }

    #[test]
    fn sprite_covers_half_open_rect_and_uses_last_vertex() {
        let mut gs = GsState::new();
        setup_basic_32(&mut gs, 16, 16);
        gs.write_internal_reg(gs_reg::PRIM, 6); // Sprite, CTXT=0
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x11, 0x22, 0x33, 0x44));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(2, 3, 10));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xAA, 0xBB, 0xCC, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(6, 5, 250));

        for y in 0..10u32 {
            for x in 0..10u32 {
                let got = gs
                    .vram
                    .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, x, y);
                let expected = if (2..6).contains(&x) && (3..5).contains(&y) {
                    0x80CC_BBAA
                } else {
                    0
                };
                assert_eq!(got, expected, "mismatch at ({x}, {y})");
                let got_z = gs
                    .vram
                    .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, x, y);
                let expected_z = if (2..6).contains(&x) && (3..5).contains(&y) {
                    250
                } else {
                    0
                };
                assert_eq!(got_z, expected_z, "z mismatch at ({x}, {y})");
            }
        }
    }

    #[test]
    fn small_triangles_follow_explicit_top_left_coverage_with_no_gaps_or_overdraw() {
        let mut gs = GsState::new();
        setup_basic_32(&mut gs, 16, 16);

        // Two triangles sharing diagonal (2,2)-(6,6) forming exact [2,6) x [2,6) square:
        // Upper-right tri (CW): (2,2), (6,2), (6,6) -> green (using IIP=0, last vtx color)
        // Lower-left tri (CCW): (2,2), (2,6), (6,6) -> blue
        gs.write_internal_reg(gs_reg::PRIM, 3); // Triangle, flat
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xFF, 0x00, 0x00, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(2, 2, 100));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(6, 2, 100));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x00, 0xFF, 0x00, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(6, 6, 100));

        // Explicit hand-computed covered pixels for upper-right tri (2,2)-(6,2)-(6,6) at integer centers:
        // Edge (2,2)->(6,2) is top (dy=0,dx>0 -> included); edge (6,6)->(2,2) is left (dy<0 -> included!);
        // edge (6,2)->(6,6) is right (dy>0 -> excluded). Covers all (x, y) in [2,6)x[2,6) with x >= y:
        let tri1_expected: [(u32, u32); 10] = [
            (2, 2),
            (3, 2),
            (4, 2),
            (5, 2),
            (3, 3),
            (4, 3),
            (5, 3),
            (4, 4),
            (5, 4),
            (5, 5),
        ];
        for y in 0..8u32 {
            for x in 0..8u32 {
                let got = gs
                    .vram
                    .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, x, y);
                let exp = if tri1_expected.contains(&(x, y)) {
                    0x8000_FF00
                } else {
                    0
                };
                assert_eq!(got, exp, "tri1 pixel mismatch at ({x}, {y})");
            }
        }

        // Now draw lower-left triangle in CCW vertex order (2,2), (2,6), (6,6) with ALWAYS blue;
        // diagonal (2,2)->(6,6) goes down-right so it is non-top-left and excluded -> zero overdraw!
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xFF, 0x00, 0x00, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(2, 2, 100));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(2, 6, 100));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x00, 0x00, 0xFF, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(6, 6, 100));

        for y in 0..8u32 {
            for x in 0..8u32 {
                let got = gs
                    .vram
                    .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, x, y);
                let exp = if tri1_expected.contains(&(x, y)) {
                    0x8000_FF00
                } else if (2..6).contains(&x) && (2..6).contains(&y) {
                    0x80FF_0000
                } else {
                    0
                };
                assert_eq!(got, exp, "shared-edge complement mismatch at ({x}, {y})");
            }
        }
    }

    #[test]
    fn gouraud_triangle_and_line_interpolate_rgb_and_z() {
        let mut gs = GsState::new();
        setup_basic_32(&mut gs, 16, 16);

        // Triangle with PRIM.IIP = 1: (0,0) red=(200,0,0,0x80) z=0, (8,0) green=(0,200,0,0x80) z=800,
        // (0,8) blue=(0,0,200,0x80) z=800.
        gs.write_internal_reg(gs_reg::PRIM, 3 | (1 << 3)); // Triangle + IIP
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(200, 0, 0, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(0, 0, 0));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0, 200, 0, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(8, 0, 800));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0, 0, 200, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(0, 8, 800));

        // Sample (0,0): w0=1, w1=0, w2=0 -> RGB=(200,0,0), Z=0.
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0),
            0x8000_00C8
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 0, 0),
            0
        );

        // Sample (4,0) on top edge: w0=1/2, w1=1/2, w2=0 -> RGB=(100,100,0), Z=400.
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, 0),
            0x8000_6464
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 4, 0),
            400
        );

        // Sample (2,2): w0=1/2, w1=1/4, w2=1/4 -> RGB=(100,50,50), Z=400.
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 2, 2),
            0x8032_3264
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 2, 2),
            400
        );
    }

    #[test]
    fn ztest_all_four_modes_and_zmsk() {
        let mut gs = GsState::new();
        setup_basic_32(&mut gs, 8, 8);

        let draw_1x1 = |gs: &mut GsState, x: i32, y: i32, z: u32, r: u8| {
            gs.write_internal_reg(gs_reg::PRIM, 6); // Sprite
            gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(x, y, z));
            gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(r, 0, 0, 0x80));
            gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(x + 1, y + 1, z));
        };

        // Initialize pixel (0,0) with ALWAYS: color R=10, Z=100.
        draw_1x1(&mut gs, 0, 0, 100, 10);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            10
        );

        // ZTST=0 (NEVER): neither color nor Z updates even with larger Z=500.
        gs.write_internal_reg(gs_reg::TEST_1, pack_test_z(true, 0));
        draw_1x1(&mut gs, 0, 0, 500, 20);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            10
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 0, 0),
            100
        );

        // ZTST=1 (ALWAYS) with ZMSK=1: color updates to 30, Z remains 100!
        gs.write_internal_reg(gs_reg::ZBUF_1, pack_zbuf(64, 0x00, 1));
        gs.write_internal_reg(gs_reg::TEST_1, pack_test_z(true, 1));
        draw_1x1(&mut gs, 0, 0, 50, 30);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            30
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 0, 0),
            100
        );

        // Unmask ZMSK=0. ZTST=2 (GEQUAL): Z=99 fails, Z=100 passes, Z=200 passes.
        gs.write_internal_reg(gs_reg::ZBUF_1, pack_zbuf(64, 0x00, 0));
        gs.write_internal_reg(gs_reg::TEST_1, pack_test_z(true, 2));
        draw_1x1(&mut gs, 0, 0, 99, 40);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            30
        );
        draw_1x1(&mut gs, 0, 0, 100, 45);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            45
        );
        draw_1x1(&mut gs, 0, 0, 200, 50);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            50
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 0, 0),
            200
        );

        // ZTST=3 (GREATER): Z=200 fails, Z=201 passes!
        gs.write_internal_reg(gs_reg::TEST_1, pack_test_z(true, 3));
        draw_1x1(&mut gs, 0, 0, 200, 60);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            50
        );
        draw_1x1(&mut gs, 0, 0, 201, 70);
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0)
                & 0xFF,
            70
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz32, 64, 1, 0, 0),
            201
        );
    }

    #[test]
    fn scissor_xyoffset_fbmsk_and_psm_16_24() {
        let mut gs = GsState::new();
        gs.write_internal_reg(gs_reg::PRMODECONT, 1);
        gs.write_internal_reg(gs_reg::ZBUF_1, pack_zbuf(64, 0x00, 1));
        gs.write_internal_reg(gs_reg::TEST_1, pack_test_z(false, 1));

        // 1. Scissor + XYOFFSET test in PSMCT24 preserving top byte 0xAB.
        gs.vram
            .write_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 2, 2, 0xAB11_2233);
        gs.vram
            .write_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, 2, 0xAB11_2233);
        // Set XYOFFSET = (100, 200) pixels, SCISSOR = [1..=3] x [2..=3]
        gs.write_internal_reg(gs_reg::XYOFFSET_1, (100 * 16) | ((200 * 16) << 32));
        gs.write_internal_reg(gs_reg::SCISSOR_1, pack_scissor(1, 3, 2, 3));
        // PSMCT24 (0x01) with FBMSK masking Green channel (0x0000_FF00).
        gs.write_internal_reg(gs_reg::FRAME_1, pack_frame(0, 1, 0x01, 0x0000_FF00));
        gs.write_internal_reg(gs_reg::PRIM, 6);
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(100, 200, 0));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x77, 0x88, 0x99, 0xFF));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(106, 206, 0));

        // (2,2) inside scissor: upper byte 0xAB preserved, G kept at 0x22, R=0x77, B=0x99!
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 2, 2),
            0xAB99_2277
        );
        // (4,2) outside scissor: untouched!
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, 2),
            0xAB11_2233
        );

        // 2. PSMCT16 with FBMSK applied before format conversion (`bit 63` preserves alpha bit 15).
        gs.write_internal_reg(gs_reg::FRAME_1, pack_frame(10, 1, 0x02, 0));
        gs.write_internal_reg(gs_reg::XYOFFSET_1, 0);
        gs.write_internal_reg(gs_reg::SCISSOR_1, pack_scissor(0, 7, 0, 7));
        gs.write_internal_reg(gs_reg::PRIM, 6);
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(0, 0, 0));
        // R=0xFF(31), G=0x00(0), B=0x00(0), A=0x80(1) -> 0x801F
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xFF, 0x00, 0x00, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(2, 2, 0));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct16, 10, 1, 1, 1),
            0x801F
        );

        // Now mask R and alpha MSB (FBMSK = 0x8000_00FF), draw G=0xFF, A=0 -> R5=31, G5=31, B5=0, A1=1 -> 0x83FF!
        gs.write_internal_reg(gs_reg::FRAME_1, pack_frame(10, 1, 0x02, 0x8000_00FF));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(0, 0, 0));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x00, 0xFF, 0x00, 0x00));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(2, 2, 0));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct16, 10, 1, 1, 1),
            0x83FF
        );
    }

    #[test]
    fn strip_fan_line_point_and_adc_xyz3_rules() {
        let mut gs = GsState::new();
        setup_basic_32(&mut gs, 16, 16);

        // 1. Point snaps subpixel coordinates near integer center.
        gs.write_internal_reg(gs_reg::PRIM, 0); // Point
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x55, 0x66, 0x77, 0x80));
        // 2.25 -> snaps to x=2; 3.4 -> snaps to y=3
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz_fp(2 * 16 + 4, 3 * 16 + 6, 1));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 2, 3),
            0x8077_6655
        );

        // 2. LineStrip with ADC (XYZ3) skips masked segment without breaking subsequent vertex!
        gs.write_internal_reg(gs_reg::PRIM, 2); // LineStrip
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xEE, 0x00, 0x00, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(0, 0, 1));
        gs.write_internal_reg(gs_reg::XYZ3, pack_xyz(4, 0, 1)); // ADC: culled segment (0,0)-(4,0)
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(4, 3, 1)); // Drawn segment (4,0)-(4,3)
                                                                // Pixel (1,0) from culled segment must still be empty; (4,0)..=(4,3) must be red!
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 0),
            0
        );
        for y in 0..=3u32 {
            assert_eq!(
                gs.vram
                    .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, y),
                0x8000_00EE,
                "line segment missing pixel (4,{y})"
            );
        }

        // 3. TriangleFan keeps hub vertex v0=(8,8) across 4 kicks, with v3 using XYZ3 ADC.
        gs.write_internal_reg(gs_reg::PRIM, 5); // TriangleFan
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x10, 0x20, 0x30, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(8, 8, 1)); // v0 hub
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(12, 8, 1)); // v1
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(12, 12, 1)); // v2 -> draws (v0, v1, v2)
                                                                  // Check pixel inside (v0,v1,v2): (11, 9)
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 11, 9),
            0x8030_2010
        );
        // v3 with XYZ3: skips tri (8,8)-(12,12)-(8,12), shifts queue to [v0=(8,8), v3=(8,12)]!
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0x90, 0x80, 0x70, 0x80));
        gs.write_internal_reg(gs_reg::XYZ3, pack_xyz(8, 12, 1));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 9, 11),
            0
        );
        // v4 with XYZ2: draws (v0=(8,8), v3=(8,12), v4=(6,10))! Pixel (7,10) is strictly inside!
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xAA, 0x55, 0x11, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(6, 10, 1));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 7, 10),
            0x8011_55AA
        );
    }

    #[test]
    fn prmode_context2_gouraud_line_and_psmz16_truncation() {
        let mut gs = GsState::new();
        // Configure Context 2 (`FRAME_2`, `ZBUF_2`, `SCISSOR_2`, `TEST_2`), selected via `PRMODE` with `PRMODECONT=0`.
        gs.write_internal_reg(gs_reg::PRMODECONT, 0);
        gs.write_internal_reg(gs_reg::PRMODE, (1 << 3) | (1 << 9)); // IIP=1, CTXT=1
        gs.write_internal_reg(gs_reg::FRAME_2, pack_frame(20, 1, 0x0A, 0)); // PSMCT16S at page 20
        gs.write_internal_reg(gs_reg::ZBUF_2, pack_zbuf(80, 0x02, 0)); // PSMZ16 at page 80
        gs.write_internal_reg(gs_reg::SCISSOR_2, pack_scissor(0, 15, 0, 15));
        gs.write_internal_reg(gs_reg::XYOFFSET_2, 0);
        gs.write_internal_reg(gs_reg::TEST_2, pack_test_z(true, 1));

        // PRIM sets Line topology ONLY (`IIP=0, CTXT=0` ignored because `PRMODECONT=0`).
        gs.write_internal_reg(gs_reg::PRIM, 1); // Line
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0, 0, 0, 0));
        gs.write_internal_reg(gs_reg::XYZF2, pack_xyz(0, 2, 0x12_0000)); // masked to 0x0000 in PSMZ16
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(248, 0, 248, 0x80));
        gs.write_internal_reg(gs_reg::XYZF2, pack_xyz(4, 2, 0x12_0400)); // masked to 0x0400

        // Midpoint x=2: R=124(15), G=0, B=124(15), A=0x40(0); Z = 0x12_0200 & 0xFFFF = 0x0200.
        let got_mid = gs
            .vram
            .read_page_pixel_raw(PixelFormat::Psmct16s, 20, 1, 2, 2);
        assert_eq!(got_mid, 15 | (15 << 10));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz16, 80, 1, 2, 2),
            0x0200
        );
    }
}
