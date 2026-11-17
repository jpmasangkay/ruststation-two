//! GS software rasterizer (`Phase 2b Step 2` + `Step 3a` nearest texturing).
//!
//! Implements vertex assembly (`XYZ2`/`XYZF2` draw kick, `XYZ3`/`XYZF3` vertex-only / ADC),
//! flat & Gouraud shading (`PRIM.IIP` / `PRMODECONT`), nearest-sampled textures (`PRIM.TME`,
//! `PRIM.FST` `UV`/`STQ`, `TEX0`/`TEX2`/`TEXFLUSH`, `CLUT` `CSM1`/`CSM2`/`TEXCLUT`/`CLD`/`CSA`,
//! `CLAMP`, `TEXA`, `TCC`/`TFX`), `XYOFFSET`, `SCISSOR`, `ZBUF` & `TEST.ZTE`/`ZTST`/`ZMSK`, and
//! `FRAME` (`FBMSK` + 32/24/16-bit formats). All pixel coverage tests delegate exclusively to
//! [`crate::coverage::is_pixel_covered_active`].

use tracing::warn;

use crate::coverage::{
    edge_fn, is_pixel_covered_active, pixel_sample_fp, snap_coord_to_pixel, CoveragePrimitive,
    ACTIVE_COVERAGE,
};
use crate::regs::{ContextRegs, GsRegs, PrimReg, PrimitiveType, TexAReg, XyzReg};
use crate::texture::{apply_tfx, sample_nearest_texel, sanitize_q, ClutCache};
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

/// Vertex assembly queue, CLUT cache, and software primitive rasterizer state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterizerState {
    prim_type: PrimitiveType,
    queue: [GsVertex; 3],
    queue_len: u8,
    warned_unsupported_features: bool,
    clut: ClutCache,
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
            clut: ClutCache::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Update the cached CLUT when `TEX0_{1,2}` or `TEX2_{1,2}` is written.
    #[inline]
    pub fn on_tex_reg_write(&mut self, regs: &GsRegs, vram: &GsVram, ctxt: usize) {
        self.clut.sync_clut(regs.ctx[ctxt].tex0, regs.texclut, vram);
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
            if (eff_prim.fge() || eff_prim.abe() || eff_prim.aa1())
                && !self.warned_unsupported_features
            {
                self.warned_unsupported_features = true;
                warn!(
                    prim = eff_prim.0,
                    "Step 3a rasterizer ignoring FGE/ABE/AA1 until Step 3b/3c"
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
            clut: &self.clut,
            texa: regs.texa,
            scissor: [scax0, scax1, scay0, scay1],
            fb_fmt,
            z_fmt: PixelFormat::from_zbuf_raw(ctx.zbuf.zsm_raw()),
            tme: eff_prim.tme(),
            fst: eff_prim.fst(),
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

#[inline]
fn stq_to_texel(s: f64, t: f64, q: f64, tw: u32, th: u32) -> (i32, i32) {
    let q_eff = sanitize_q(q);
    let iu = ((s / q_eff) * f64::from(tw)).floor() as i32;
    let iv = ((t / q_eff) * f64::from(th)).floor() as i32;
    (iu, iv)
}

#[inline]
fn vertex_stq_f64(v: GsVertex) -> (f64, f64, f64) {
    (
        f64::from(f32::from_bits(v.s_bits)),
        f64::from(f32::from_bits(v.t_bits)),
        f64::from(f32::from_bits(v.q_bits)),
    )
}

#[derive(Copy, Clone)]
struct RenderTarget<'a> {
    ctx: &'a ContextRegs,
    clut: &'a ClutCache,
    texa: TexAReg,
    scissor: [i32; 4],
    fb_fmt: PixelFormat,
    z_fmt: Option<PixelFormat>,
    tme: bool,
    fst: bool,
}

#[inline]
fn shade_fragment(
    target: RenderTarget<'_>,
    vram: &mut GsVram,
    px: u32,
    py: u32,
    vert_rgba: [u8; 4],
    texel_uv: Option<(i32, i32)>,
    z: u32,
) {
    let rgba32 = match texel_uv {
        Some((iu, iv)) => {
            let texel = sample_nearest_texel(vram, target.clut, target.ctx, target.texa, iu, iv);
            apply_tfx(target.ctx.tex0, vert_rgba, texel)
        }
        None => pack_rgba32(vert_rgba[0], vert_rgba[1], vert_rgba[2], vert_rgba[3]),
    };
    shade_pixel(target, vram, px, py, rgba32, z);
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
    let vert_rgba = [v.r, v.g, v.b, v.a];
    let texel_uv = if target.tme {
        if target.fst {
            Some((i32::from(v.u_raw) >> 4, i32::from(v.v_raw) >> 4))
        } else {
            let (s, t, q) = vertex_stq_f64(v);
            Some(stq_to_texel(
                s,
                t,
                q,
                target.ctx.tex0.width(),
                target.ctx.tex0.height(),
            ))
        }
    } else {
        None
    };

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if is_pixel_covered_active(&prim, px, py) {
                shade_fragment(target, vram, px as u32, py as u32, vert_rgba, texel_uv, v.z);
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
    let flat_rgba = [v1.r, v1.g, v1.b, v1.a];
    let (s0, t0, q0) = vertex_stq_f64(v0);
    let (s1, t1, q1) = vertex_stq_f64(v1);
    let tw = target.ctx.tex0.width();
    let th = target.ctx.tex0.height();

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
            let (vert_rgba, z, texel_uv) = if steps == 0 {
                let uv = if target.tme {
                    if target.fst {
                        Some((i32::from(v1.u_raw) >> 4, i32::from(v1.v_raw) >> 4))
                    } else {
                        Some(stq_to_texel(s1, t1, q1, tw, th))
                    }
                } else {
                    None
                };
                (flat_rgba, v1.z, uv)
            } else {
                let t = if adx >= ady {
                    px.abs_diff(ix0) as u128
                } else {
                    py.abs_diff(iy0) as u128
                };
                let z = lerp_u32(v0.z, v1.z, t, steps);
                let vert_rgba = if gouraud {
                    [
                        lerp_u8(v0.r, v1.r, t, steps),
                        lerp_u8(v0.g, v1.g, t, steps),
                        lerp_u8(v0.b, v1.b, t, steps),
                        lerp_u8(v0.a, v1.a, t, steps),
                    ]
                } else {
                    flat_rgba
                };
                let uv = if target.tme {
                    if target.fst {
                        let s_den = steps as i128;
                        let st = t as i128;
                        let u_fp = ((s_den - st) * i128::from(v0.u_raw) + st * i128::from(v1.u_raw))
                            .div_euclid(s_den) as i32;
                        let v_fp = ((s_den - st) * i128::from(v0.v_raw) + st * i128::from(v1.v_raw))
                            .div_euclid(s_den) as i32;
                        Some((u_fp.div_euclid(16), v_fp.div_euclid(16)))
                    } else {
                        let frac = (t as f64) / (steps as f64);
                        let s = (1.0 - frac) * s0 + frac * s1;
                        let tc = (1.0 - frac) * t0 + frac * t1;
                        let q = (1.0 - frac) * q0 + frac * q1;
                        Some(stq_to_texel(s, tc, q, tw, th))
                    }
                } else {
                    None
                };
                (vert_rgba, z, uv)
            };
            shade_fragment(target, vram, px as u32, py as u32, vert_rgba, texel_uv, z);
        }
    }
}

fn raster_sprite(target: RenderTarget<'_>, vram: &mut GsVram, v0: GsVertex, v1: GsVertex) {
    let (x0_fp, y0_fp) = to_screen_fp(target.ctx, v0);
    let (x1_fp, y1_fp) = to_screen_fp(target.ctx, v1);
    let (min_x_fp, max_x_fp, left_v, right_v) = if x0_fp <= x1_fp {
        (x0_fp, x1_fp, v0, v1)
    } else {
        (x1_fp, x0_fp, v1, v0)
    };
    let (min_y_fp, max_y_fp, top_v, bot_v) = if y0_fp <= y1_fp {
        (y0_fp, y1_fp, v0, v1)
    } else {
        (y1_fp, y0_fp, v1, v0)
    };

    let prim = CoveragePrimitive::Sprite {
        min_x_fp,
        min_y_fp,
        max_x_fp,
        max_y_fp,
    };
    let (min_x, max_x, min_y, max_y) =
        clipped_bbox(min_x_fp, max_x_fp, min_y_fp, max_y_fp, target.scissor);

    // `Play! GSH_OpenGL.cpp` (`BSD-2`, `FETCHED`): sprite uses final vertex `v1`'s color & Z.
    let vert_rgba = [v1.r, v1.g, v1.b, v1.a];
    let z = v1.z;
    let dx_fp = i64::from(max_x_fp) - i64::from(min_x_fp);
    let dy_fp = i64::from(max_y_fp) - i64::from(min_y_fp);
    let tw = f64::from(target.ctx.tex0.width());
    let th = f64::from(target.ctx.tex0.height());
    let (sl, _, ql) = vertex_stq_f64(left_v);
    let (sr, _, qr) = vertex_stq_f64(right_v);
    let (_, tt, qt) = vertex_stq_f64(top_v);
    let (_, tb, qb) = vertex_stq_f64(bot_v);
    let u_left = (sl / sanitize_q(ql)) * tw;
    let u_right = (sr / sanitize_q(qr)) * tw;
    let v_top = (tt / sanitize_q(qt)) * th;
    let v_bot = (tb / sanitize_q(qb)) * th;

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            if !is_pixel_covered_active(&prim, px, py) {
                continue;
            }
            let texel_uv = if target.tme {
                let (cx_fp, cy_fp) = pixel_sample_fp(px, py, ACTIVE_COVERAGE);
                if target.fst {
                    let u_fp = i64::from(left_v.u_raw)
                        + ((cx_fp - i64::from(min_x_fp))
                            * (i64::from(right_v.u_raw) - i64::from(left_v.u_raw)))
                        .div_euclid(dx_fp);
                    let v_fp = i64::from(top_v.v_raw)
                        + ((cy_fp - i64::from(min_y_fp))
                            * (i64::from(bot_v.v_raw) - i64::from(top_v.v_raw)))
                        .div_euclid(dy_fp);
                    Some(((u_fp as i32).div_euclid(16), (v_fp as i32).div_euclid(16)))
                } else {
                    let fx = ((cx_fp - i64::from(min_x_fp)) as f64) / (dx_fp as f64);
                    let fy = ((cy_fp - i64::from(min_y_fp)) as f64) / (dy_fp as f64);
                    let iu = (u_left + fx * (u_right - u_left)).floor() as i32;
                    let iv = (v_top + fy * (v_bot - v_top)).floor() as i32;
                    Some((iu, iv))
                }
            } else {
                None
            };
            shade_fragment(target, vram, px as u32, py as u32, vert_rgba, texel_uv, z);
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
    let flat_rgba = [verts[2].r, verts[2].g, verts[2].b, verts[2].a];

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
    let (s0, t0, q0) = vertex_stq_f64(verts[0]);
    let (s1, t1, q1) = vertex_stq_f64(verts[1]);
    let (s2, t2, q2) = vertex_stq_f64(verts[2]);
    let tw = target.ctx.tex0.width();
    let th = target.ctx.tex0.height();

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

            let vert_rgba = if gouraud {
                [
                    bilerp_u8(verts[0].r, verts[1].r, verts[2].r, w0, w1, w2, denom),
                    bilerp_u8(verts[0].g, verts[1].g, verts[2].g, w0, w1, w2, denom),
                    bilerp_u8(verts[0].b, verts[1].b, verts[2].b, w0, w1, w2, denom),
                    bilerp_u8(verts[0].a, verts[1].a, verts[2].a, w0, w1, w2, denom),
                ]
            } else {
                flat_rgba
            };

            let texel_uv = if target.tme {
                if target.fst {
                    let s_den = denom as i128;
                    let u_fp = (w0 as i128 * i128::from(verts[0].u_raw)
                        + w1 as i128 * i128::from(verts[1].u_raw)
                        + w2 as i128 * i128::from(verts[2].u_raw))
                    .div_euclid(s_den) as i32;
                    let v_fp = (w0 as i128 * i128::from(verts[0].v_raw)
                        + w1 as i128 * i128::from(verts[1].v_raw)
                        + w2 as i128 * i128::from(verts[2].v_raw))
                    .div_euclid(s_den) as i32;
                    Some((u_fp.div_euclid(16), v_fp.div_euclid(16)))
                } else {
                    let inv = 1.0 / (denom as f64);
                    let l0 = (w0 as f64) * inv;
                    let l1 = (w1 as f64) * inv;
                    let l2 = (w2 as f64) * inv;
                    let s = l0 * s0 + l1 * s1 + l2 * s2;
                    let t = l0 * t0 + l1 * t1 + l2 * t2;
                    let q = l0 * q0 + l1 * q1 + l2 * q2;
                    Some(stq_to_texel(s, t, q, tw, th))
                }
            } else {
                None
            };

            shade_fragment(target, vram, px as u32, py as u32, vert_rgba, texel_uv, z);
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
                                                                // Hand derivation (`IntegerCenterHalfOpenLine`): culled segment leaves (1,0)=0;
                                                                // half-open vertical segment (4,0)->(4,3) draws (4,0),(4,1),(4,2) and omits final (4,3).
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 0),
            0
        );
        for y in 0..3u32 {
            assert_eq!(
                gs.vram
                    .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, y),
                0x8000_00EE,
                "line segment missing pixel (4,{y})"
            );
        }
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, 3),
            0,
            "half-open line must omit final endpoint (4,3)"
        );

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
        // Half-open rule leaves final endpoint (4,2) unwritten (0).
        let got_mid = gs
            .vram
            .read_page_pixel_raw(PixelFormat::Psmct16s, 20, 1, 2, 2);
        assert_eq!(got_mid, 15 | (15 << 10));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmz16, 80, 1, 2, 2),
            0x0200
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct16s, 20, 1, 4, 2),
            0
        );
    }

    #[test]
    fn textured_point_line_reversed_sprite_and_stq_zero_fallback() {
        let mut gs = GsState::new();
        setup_basic_32(&mut gs, 16, 16);
        // 4x4 PSMCT32 texture at block 64 (`TBP0=64`, `TBW=1`, `TW=2 (4px)`, `TH=2 (4px)`), Decal TCC=1.
        for ty in 0..4u32 {
            for tx in 0..4u32 {
                let c = 0x8000_0000 | (tx * 40 + 10) | ((ty * 50 + 20) << 8) | ((tx + ty) << 16);
                gs.vram
                    .write_pixel_raw(PixelFormat::Psmct32, 64, 1, tx, ty, c);
            }
        }
        let tex0 = 64u64 | (1 << 14) | (2 << 26) | (2 << 30) | (1 << 34) | (1 << 35); // Decal TCC=1
        gs.write_internal_reg(gs_reg::TEX0_1, tex0);
        gs.write_internal_reg(gs_reg::CLAMP_1, 0x05); // WMS=1 CLAMP, WMT=1 CLAMP

        // 1. Textured Point with FST=1 (`PRIM = Point | TME | FST` = 0 | 16 | 256 = 0x110) at UV=(2.0, 3.0).
        gs.write_internal_reg(gs_reg::PRIM, 0x110);
        gs.write_internal_reg(gs_reg::UV, (2 * 16) | ((3 * 16) << 16));
        gs.write_internal_reg(gs_reg::RGBAQ, pack_rgba(0xFF, 0xFF, 0xFF, 0x80));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(1, 1, 1));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 1),
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 64, 1, 2, 3)
        );

        // 2. Textured Line with FST=0 ST/Q (`Q == 0` falls back to `1.0`): (4,1)->(8,1) s=0.0..1.0.
        gs.write_internal_reg(gs_reg::PRIM, 0x011); // Line | TME
        gs.write_internal_reg(
            gs_reg::ST,
            u64::from(0.0f32.to_bits() & !0xFF) | (u64::from(0.0f32.to_bits() & !0xFF) << 32),
        );
        gs.write_internal_reg(gs_reg::RGBAQ, 0x0080_8080); // Q=0 -> fallback 1.0
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(4, 1, 1));
        gs.write_internal_reg(
            gs_reg::ST,
            u64::from(1.0f32.to_bits() & !0xFF) | (u64::from(0.0f32.to_bits() & !0xFF) << 32),
        );
        gs.write_internal_reg(gs_reg::RGBAQ, 0x0080_8080);
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(8, 1, 1));
        // Half-open line draws x=4..7 (`s = 0, 0.25, 0.5, 0.75` -> texels `0, 1, 2, 3`) and leaves x=8 clear.
        for x in 4..8u32 {
            assert_eq!(
                gs.vram
                    .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, x, 1),
                gs.vram
                    .read_pixel_raw(PixelFormat::Psmct32, 64, 1, x - 4, 0)
            );
        }
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 8, 1),
            0
        );

        // 3. Reversed-corner textured Sprite (`PRIM = 6 | 16 | 256 = 0x116`), 2x magnification `(12,12)->(4,4)` UV=(4,4)->(0,0).
        gs.write_internal_reg(gs_reg::PRIM, 0x116);
        gs.write_internal_reg(gs_reg::UV, (4 * 16) | ((4 * 16) << 16));
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(12, 12, 1));
        gs.write_internal_reg(gs_reg::UV, 0);
        gs.write_internal_reg(gs_reg::XYZ2, pack_xyz(4, 4, 1));
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 4, 4),
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 64, 1, 0, 0)
        );
        assert_eq!(
            gs.vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 11, 11),
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 64, 1, 3, 3)
        );
    }
}
