//! Nearest texture sampling, CLUT cache (`CSM1`/`CSM2`), coordinate clamping,
//! `TEXA` expansion, and `TFX` color combination (`Phase 2b Step 3a`).
//!
//! Register semantics follow `local/ps2tek/index.html` (`#gstextures`, `FETCHED`) and
//! `Play!` (`Source/gs/GSHandler.cpp`, `GSH_OpenGL.cpp`, `GSH_OpenGL_Shader.cpp`,
//! BSD-2-Clause, commit `1f27232772e484bfb79364b751b84f41f4c87481`, `FETCHED`).
//! Unspecified hardware edge cases are catalogued under `G-017`..`G-021` in `docs/GS_GUESSES.md`.

use crate::regs::{ContextRegs, Tex0Reg, TexAReg, TexClutReg};
use crate::vram::{GsVram, PixelFormat};

/// 1 KiB hardware GS Color Look-Up Table cache (`512` x 16-bit slots) and `CBP0`/`CBP1` tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClutCache {
    clut: [u16; 512],
    cbp0: u32,
    cbp1: u32,
}

impl Default for ClutCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ClutCache {
    pub const fn new() -> Self {
        Self {
            clut: [0; 512],
            cbp0: 0,
            cbp1: 0,
        }
    }

    pub fn reset(&mut self) {
        self.clut.fill(0);
        self.cbp0 = 0;
        self.cbp1 = 0;
    }

    /// Synchronize the internal 1 KiB CLUT cache on a `TEX0` or `TEX2` write (`CLD 0..=5`, `G-021`).
    pub fn sync_clut(&mut self, tex0: Tex0Reg, texclut: TexClutReg, vram: &GsVram) {
        if !self.process_cld(tex0) {
            return;
        }
        let Some(tex_fmt) = PixelFormat::from_raw(tex0.psm_raw()) else {
            return;
        };
        let is_idtex4 = match tex_fmt {
            PixelFormat::Psmt4 | PixelFormat::Psmt4hl | PixelFormat::Psmt4hh => true,
            PixelFormat::Psmt8 | PixelFormat::Psmt8h => false,
            _ => return,
        };
        let Some(cpsm) = decode_clut_psm(tex0.cpsm_raw()) else {
            return;
        };
        let cbp = tex0.cbp();

        if tex0.csm2() {
            let fmt16 = match cpsm {
                PixelFormat::Psmct16s => PixelFormat::Psmct16s,
                _ => PixelFormat::Psmct16,
            };
            let cbw = texclut.cbw().max(1);
            let cou_px = texclut.cou_pixels();
            let cov = texclut.cov();
            let count = if is_idtex4 { 16u32 } else { 256u32 };
            for i in 0..count {
                let raw16 = vram.read_pixel_raw(fmt16, cbp, cbw, cou_px + i, cov) as u16;
                self.clut[i as usize] = raw16;
            }
            return;
        }

        // CSM1 layout in VRAM (`bw = 1`, `8x2` for IDTEX4 and `16x16` with `0x08 <-> 0x10` swap for IDTEX8).
        let is_32bit_clut = matches!(cpsm, PixelFormat::Psmct32 | PixelFormat::Psmct24);
        if is_idtex4 {
            let csa_offset = if is_32bit_clut {
                usize::from(tex0.csa() & 0x0F) * 16
            } else {
                usize::from(tex0.csa()) * 16
            };
            for j in 0..2u32 {
                for i in 0..8u32 {
                    let slot = csa_offset + (j * 8 + i) as usize;
                    if is_32bit_clut {
                        let word = vram.read_pixel_raw(PixelFormat::Psmct32, cbp, 1, i, j);
                        self.clut[slot] = word as u16;
                        self.clut[slot + 0x100] = (word >> 16) as u16;
                    } else {
                        let half = vram.read_pixel_raw(cpsm, cbp, 1, i, j) as u16;
                        self.clut[slot] = half;
                    }
                }
            }
        } else {
            for j in 0..16u32 {
                for i in 0..16u32 {
                    let raw_idx = (j * 16 + i) as usize;
                    // Hardware 8-bit CSM1 swizzle: swap bits 3 (0x08) and 4 (0x10).
                    let swizzled =
                        (raw_idx & !0x18) | ((raw_idx & 0x08) << 1) | ((raw_idx & 0x10) >> 1);
                    if is_32bit_clut {
                        let word = vram.read_pixel_raw(PixelFormat::Psmct32, cbp, 1, i, j);
                        self.clut[swizzled] = word as u16;
                        self.clut[swizzled + 0x100] = (word >> 16) as u16;
                    } else {
                        let half = vram.read_pixel_raw(cpsm, cbp, 1, i, j) as u16;
                        self.clut[swizzled] = half;
                    }
                }
            }
        }
    }

    fn process_cld(&mut self, tex0: Tex0Reg) -> bool {
        match tex0.cld() {
            0 => false,
            1 => true,
            2 => {
                self.cbp0 = tex0.cbp();
                true
            }
            3 => {
                self.cbp1 = tex0.cbp();
                true
            }
            4 => {
                if self.cbp0 == tex0.cbp() {
                    false
                } else {
                    self.cbp0 = tex0.cbp();
                    true
                }
            }
            5 => {
                if self.cbp1 == tex0.cbp() {
                    false
                } else {
                    self.cbp1 = tex0.cbp();
                    true
                }
            }
            _ => false,
        }
    }

    /// Resolve a palette index (`0..15` or `0..255`) from the cached 1 KiB CLUT (`RGBA`).
    pub fn lookup(&self, tex0: Tex0Reg, texa: TexAReg, is_idtex4: bool, index: u8) -> [u8; 4] {
        let Some(cpsm) = decode_clut_psm(tex0.cpsm_raw()) else {
            return [0, 0, 0, 0x80];
        };
        if !tex0.csm2() && matches!(cpsm, PixelFormat::Psmct32 | PixelFormat::Psmct24) {
            let slot = if is_idtex4 {
                usize::from(tex0.csa() & 0x0F) * 16 + usize::from(index & 0x0F)
            } else {
                (usize::from(tex0.csa() & 0x0F) * 16 + usize::from(index)) & 0xFF
            };
            let word = u32::from(self.clut[slot]) | (u32::from(self.clut[slot + 0x100]) << 16);
            if matches!(cpsm, PixelFormat::Psmct24) {
                expand_rgb24_with_texa(word, texa)
            } else {
                unpack_rgba32(word)
            }
        } else {
            let slot = if tex0.csm2() {
                if is_idtex4 {
                    usize::from(index & 0x0F)
                } else {
                    usize::from(index)
                }
            } else if is_idtex4 {
                (usize::from(tex0.csa()) * 16 + usize::from(index & 0x0F)) & 0x1FF
            } else {
                (usize::from(tex0.csa()) * 16 + usize::from(index)) & 0xFF
            };
            expand_rgb5551_with_texa(self.clut[slot], texa)
        }
    }
}

#[inline]
const fn decode_clut_psm(raw: u8) -> Option<PixelFormat> {
    match raw & 0xF {
        0x0 => Some(PixelFormat::Psmct32),
        0x1 => Some(PixelFormat::Psmct24),
        0x2 => Some(PixelFormat::Psmct16),
        0xA => Some(PixelFormat::Psmct16s),
        _ => None,
    }
}

#[inline]
const fn unpack_rgba32(word: u32) -> [u8; 4] {
    [
        word as u8,
        (word >> 8) as u8,
        (word >> 16) as u8,
        (word >> 24) as u8,
    ]
}

/// Expand 24-bit RGB texel with `TEXA.TA0` and `TEXA.AEM` black-transparency check.
#[inline]
pub const fn expand_rgb24_with_texa(word: u32, texa: TexAReg) -> [u8; 4] {
    let r = word as u8;
    let g = (word >> 8) as u8;
    let b = (word >> 16) as u8;
    let a = if texa.aem() && r == 0 && g == 0 && b == 0 {
        0
    } else {
        texa.ta0()
    };
    [r, g, b, a]
}

/// Expand 16-bit `RGB5551` texel into 8-bit channels with `TEXA.TA0`/`TA1` and `TEXA.AEM`.
#[inline]
pub const fn expand_rgb5551_with_texa(pix16: u16, texa: TexAReg) -> [u8; 4] {
    let r = ((pix16 & 0x1F) << 3) as u8;
    let g = (((pix16 >> 5) & 0x1F) << 3) as u8;
    let b = (((pix16 >> 10) & 0x1F) << 3) as u8;
    let a = if texa.aem() && r == 0 && g == 0 && b == 0 {
        0
    } else if (pix16 >> 15) != 0 {
        texa.ta1()
    } else {
        texa.ta0()
    };
    [r, g, b, a]
}

/// Replace non-finite or zero perspective `Q` with `1.0` (`G-018`).
#[inline]
pub fn sanitize_q(q: f64) -> f64 {
    if q == 0.0 || !q.is_finite() {
        1.0
    } else {
        q
    }
}

/// Apply `CLAMP.WMS`/`WMT` (`0=Repeat`, `1=Clamp`, `2=RegionClamp`, `3=RegionRepeat`, `G-021`).
#[inline]
pub const fn apply_clamp_coord(
    coord: i32,
    tex_size: u32,
    mode: u8,
    min_val: u16,
    max_val: u16,
) -> u32 {
    let mask = (tex_size as i32) - 1;
    let wrapped = match mode & 0x3 {
        0 => coord & mask,
        1 => {
            if coord < 0 {
                0
            } else if coord > mask {
                mask
            } else {
                coord
            }
        }
        2 => {
            let lo = min_val as i32;
            let hi = max_val as i32;
            let floor_lo = if coord > lo { coord } else { lo };
            let clamped = if hi < floor_lo { hi } else { floor_lo };
            clamped & mask
        }
        _ => ((coord & (min_val as i32)) | (max_val as i32)) & mask,
    };
    wrapped as u32
}

/// Sample one nearest texel `[r, g, b, a]` at integer texel `(iu, iv)` after `CLAMP`.
pub fn sample_nearest_texel(
    vram: &GsVram,
    clut: &ClutCache,
    ctx: &ContextRegs,
    texa: TexAReg,
    iu: i32,
    iv: i32,
) -> [u8; 4] {
    let tex0 = ctx.tex0;
    let clamp = ctx.clamp;
    let u = apply_clamp_coord(iu, tex0.width(), clamp.wms(), clamp.minu(), clamp.maxu());
    let v = apply_clamp_coord(iv, tex0.height(), clamp.wmt(), clamp.minv(), clamp.maxv());
    let Some(tex_fmt) = PixelFormat::from_raw(tex0.psm_raw()) else {
        return [0, 0, 0, 0x80];
    };
    let raw = vram.read_pixel_raw(tex_fmt, tex0.tbp0(), tex0.tbw(), u, v);

    match tex_fmt {
        PixelFormat::Psmct32 | PixelFormat::Psmz32 => unpack_rgba32(raw),
        PixelFormat::Psmct24 | PixelFormat::Psmz24 => expand_rgb24_with_texa(raw, texa),
        PixelFormat::Psmct16
        | PixelFormat::Psmct16s
        | PixelFormat::Psmz16
        | PixelFormat::Psmz16s => expand_rgb5551_with_texa(raw as u16, texa),
        PixelFormat::Psmt8 | PixelFormat::Psmt8h => clut.lookup(tex0, texa, false, raw as u8),
        PixelFormat::Psmt4 | PixelFormat::Psmt4hl | PixelFormat::Psmt4hh => {
            clut.lookup(tex0, texa, true, raw as u8)
        }
    }
}

/// Combine vertex `[r, g, b, a]` and texel `[r, g, b, a]` via `TEX0.TFX` and `TEX0.TCC` (`ps2tek`, `G-020`).
#[inline]
pub const fn apply_tfx(tex0: Tex0Reg, vert: [u8; 4], texel: [u8; 4]) -> u32 {
    #[inline]
    const fn mod_ch(v: u8, t: u8) -> u16 {
        ((v as u16) * (t as u16)) >> 7
    }
    #[inline]
    const fn clamp_u8(val: u16) -> u8 {
        if val > 255 {
            255
        } else {
            val as u8
        }
    }

    let [vr, vg, vb, va] = vert;
    let [tr, tg, tb, ta] = texel;
    let tcc = tex0.tcc();

    let (r, g, b, a) = match tex0.tfx() & 0x3 {
        0 => {
            // Modulate
            let r = clamp_u8(mod_ch(vr, tr));
            let g = clamp_u8(mod_ch(vg, tg));
            let b = clamp_u8(mod_ch(vb, tb));
            let a = if tcc { clamp_u8(mod_ch(va, ta)) } else { va };
            (r, g, b, a)
        }
        1 => {
            // Decal
            let a = if tcc { ta } else { va };
            (tr, tg, tb, a)
        }
        2 => {
            // Highlight
            let r = clamp_u8(mod_ch(vr, tr) + (va as u16));
            let g = clamp_u8(mod_ch(vg, tg) + (va as u16));
            let b = clamp_u8(mod_ch(vb, tb) + (va as u16));
            let a = if tcc {
                clamp_u8((ta as u16) + (va as u16))
            } else {
                va
            };
            (r, g, b, a)
        }
        _ => {
            // Highlight2
            let r = clamp_u8(mod_ch(vr, tr) + (va as u16));
            let g = clamp_u8(mod_ch(vg, tg) + (va as u16));
            let b = clamp_u8(mod_ch(vb, tb) + (va as u16));
            let a = if tcc { ta } else { va };
            (r, g, b, a)
        }
    };

    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tex0Spec {
        tbp0: u64,
        tbw: u64,
        psm: u64,
        tw: u64,
        th: u64,
        tcc: bool,
        tfx: u64,
        cbp: u64,
        cpsm: u64,
        csm2: bool,
        csa: u64,
        cld: u64,
    }

    fn make_tex0(s: Tex0Spec) -> Tex0Reg {
        Tex0Reg(
            s.tbp0
                | (s.tbw << 14)
                | (s.psm << 20)
                | (s.tw << 26)
                | (s.th << 30)
                | (u64::from(s.tcc) << 34)
                | (s.tfx << 35)
                | (s.cbp << 37)
                | (s.cpsm << 51)
                | (u64::from(s.csm2) << 55)
                | (s.csa << 56)
                | (s.cld << 61),
        )
    }

    #[test]
    fn all_four_clamp_modes_map_coordinates_independently() {
        // REPEAT (0) on width 16
        assert_eq!(apply_clamp_coord(-1, 16, 0, 0, 0), 15);
        assert_eq!(apply_clamp_coord(18, 16, 0, 0, 0), 2);
        // CLAMP (1) on width 16
        assert_eq!(apply_clamp_coord(-5, 16, 1, 0, 0), 0);
        assert_eq!(apply_clamp_coord(25, 16, 1, 0, 0), 15);
        // REGION_CLAMP (2) [3..=9] on width 16
        assert_eq!(apply_clamp_coord(0, 16, 2, 3, 9), 3);
        assert_eq!(apply_clamp_coord(6, 16, 2, 3, 9), 6);
        assert_eq!(apply_clamp_coord(14, 16, 2, 3, 9), 9);
        // REGION_REPEAT (3): UMSK=0b0011 (3), UFIX=0b1100 (12) -> atlas range [12..=15] inside 16
        assert_eq!(apply_clamp_coord(0, 16, 3, 3, 12), 12);
        assert_eq!(apply_clamp_coord(5, 16, 3, 3, 12), 13);
        assert_eq!(apply_clamp_coord(7, 16, 3, 3, 12), 15);
    }

    #[test]
    fn texa_ta0_ta1_and_aem_expand_16_and_24bit_texels() {
        let texa = TexAReg(0x40 | (1 << 15) | (0xC0u64 << 32)); // TA0=0x40, AEM=1, TA1=0xC0
                                                                // PSMCT24: non-black gets TA0; exact RGB=(0,0,0) becomes alpha 0 due to AEM
        assert_eq!(
            expand_rgb24_with_texa(0xFF10_2030, texa),
            [0x30, 0x20, 0x10, 0x40]
        );
        assert_eq!(expand_rgb24_with_texa(0xFF00_0000, texa), [0, 0, 0, 0]);
        // PSMCT16: bit15=0 -> TA0, bit15=1 -> TA1, black -> 0 when AEM=1
        assert_eq!(expand_rgb5551_with_texa(0x001F, texa), [0xF8, 0, 0, 0x40]);
        assert_eq!(expand_rgb5551_with_texa(0x801F, texa), [0xF8, 0, 0, 0xC0]);
        assert_eq!(expand_rgb5551_with_texa(0x8000, texa), [0, 0, 0, 0]);
    }

    #[test]
    fn csm1_8bit_swaps_bits_0x08_and_0x10_and_cld_0_to_5_control_cache() {
        let mut vram = GsVram::new();
        let mut cache = ClutCache::new();
        // Store distinct palette colors in CBP=100 (PSMCT32) at (x,y) = (8,0) (raw 0x08 -> logical 0x10)
        // and (x,y) = (0,1) (raw 0x10 -> logical 0x08), plus entry (0,0).
        vram.write_pixel_raw(PixelFormat::Psmct32, 100, 1, 8, 0, 0x80AA_BBCC);
        vram.write_pixel_raw(PixelFormat::Psmct32, 100, 1, 0, 1, 0x8011_2233);

        let tex0_load_cbp0 = make_tex0(Tex0Spec {
            tbp0: 0,
            tbw: 1,
            psm: 0x13,
            tw: 4,
            th: 4,
            tcc: true,
            tfx: 0,
            cbp: 100,
            cpsm: 0,
            csm2: false,
            csa: 0,
            cld: 2,
        }); // CLD=2 -> reload & CBP0=100
        cache.sync_clut(tex0_load_cbp0, TexClutReg(0), &vram);
        assert_eq!(
            cache.lookup(tex0_load_cbp0, TexAReg(0), false, 0x10),
            [0xCC, 0xBB, 0xAA, 0x80]
        );
        assert_eq!(
            cache.lookup(tex0_load_cbp0, TexAReg(0), false, 0x08),
            [0x33, 0x22, 0x11, 0x80]
        );

        // Mutate VRAM at CBP=100, then verify CLD=0 and CLD=4 (CBP==CBP0) DO NOT reload stale VRAM!
        vram.write_pixel_raw(PixelFormat::Psmct32, 100, 1, 0, 1, 0x8099_8877);
        let tex0_cld0 = make_tex0(Tex0Spec {
            tbp0: 0,
            tbw: 1,
            psm: 0x13,
            tw: 4,
            th: 4,
            tcc: true,
            tfx: 0,
            cbp: 100,
            cpsm: 0,
            csm2: false,
            csa: 0,
            cld: 0,
        });
        cache.sync_clut(tex0_cld0, TexClutReg(0), &vram);
        assert_eq!(
            cache.lookup(tex0_cld0, TexAReg(0), false, 0x08),
            [0x33, 0x22, 0x11, 0x80]
        );
        let tex0_cld4_same = make_tex0(Tex0Spec {
            tbp0: 0,
            tbw: 1,
            psm: 0x13,
            tw: 4,
            th: 4,
            tcc: true,
            tfx: 0,
            cbp: 100,
            cpsm: 0,
            csm2: false,
            csa: 0,
            cld: 4,
        });
        cache.sync_clut(tex0_cld4_same, TexClutReg(0), &vram);
        assert_eq!(
            cache.lookup(tex0_cld4_same, TexAReg(0), false, 0x08),
            [0x33, 0x22, 0x11, 0x80]
        );

        // CLD=3 sets CBP1=108; CLD=5 skips reload on CBP1=108; CLD=5 on CBP=100 reloads!
        vram.write_pixel_raw(PixelFormat::Psmct32, 108, 1, 0, 1, 0x8044_5566);
        let tex0_cld3 = make_tex0(Tex0Spec {
            tbp0: 0,
            tbw: 1,
            psm: 0x13,
            tw: 4,
            th: 4,
            tcc: true,
            tfx: 0,
            cbp: 108,
            cpsm: 0,
            csm2: false,
            csa: 0,
            cld: 3,
        });
        cache.sync_clut(tex0_cld3, TexClutReg(0), &vram);
        assert_eq!(
            cache.lookup(tex0_cld3, TexAReg(0), false, 0x08),
            [0x66, 0x55, 0x44, 0x80]
        );

        vram.write_pixel_raw(PixelFormat::Psmct32, 108, 1, 0, 1, 0x8001_0203);
        let tex0_cld5_same = make_tex0(Tex0Spec {
            tbp0: 0,
            tbw: 1,
            psm: 0x13,
            tw: 4,
            th: 4,
            tcc: true,
            tfx: 0,
            cbp: 108,
            cpsm: 0,
            csm2: false,
            csa: 0,
            cld: 5,
        });
        cache.sync_clut(tex0_cld5_same, TexClutReg(0), &vram);
        assert_eq!(
            cache.lookup(tex0_cld5_same, TexAReg(0), false, 0x08),
            [0x66, 0x55, 0x44, 0x80]
        );

        let tex0_cld5_new = make_tex0(Tex0Spec {
            tbp0: 0,
            tbw: 1,
            psm: 0x13,
            tw: 4,
            th: 4,
            tcc: true,
            tfx: 0,
            cbp: 100,
            cpsm: 0,
            csm2: false,
            csa: 0,
            cld: 5,
        });
        cache.sync_clut(tex0_cld5_new, TexClutReg(0), &vram);
        assert_eq!(
            cache.lookup(tex0_cld5_new, TexAReg(0), false, 0x08),
            [0x77, 0x88, 0x99, 0x80]
        );
    }

    #[test]
    fn csm2_texclut_and_psmt4hl_hh_upper_nibble_lookup() {
        let mut vram = GsVram::new();
        let mut cache = ClutCache::new();
        // CSM2 linear CLUT in PSMCT16 at CBP=96, TEXCLUT CBW=1, COU=1 (16 px), COV=3.
        let texclut = TexClutReg(1 | (1 << 6) | (3 << 12));
        // Put green (R=0,G=31,B=0,A=0 -> 0x03E0) at offset +1 (`x=17, y=3`) and magenta at offset +2 (`x=18, y=3`).
        vram.write_pixel_raw(PixelFormat::Psmct16, 96, 1, 17, 3, 0x03E0);
        vram.write_pixel_raw(PixelFormat::Psmct16, 96, 1, 18, 3, 0x8000 | 31 | (31 << 10));
        // Store shared PSMCT32 texel word at TBP0=32: low24=0x112233, nibble4HL=1, nibble4HH=2.
        vram.write_pixel_raw(PixelFormat::Psmct32, 32, 1, 0, 0, 0x2111_2233);

        let tex4hl = make_tex0(Tex0Spec {
            tbp0: 32,
            tbw: 1,
            psm: 0x24,
            tw: 3,
            th: 3,
            tcc: true,
            tfx: 1,
            cbp: 96,
            cpsm: 0x02,
            csm2: true,
            csa: 0,
            cld: 1,
        });
        let tex4hh = make_tex0(Tex0Spec {
            tbp0: 32,
            tbw: 1,
            psm: 0x2C,
            tw: 3,
            th: 3,
            tcc: true,
            tfx: 1,
            cbp: 96,
            cpsm: 0x02,
            csm2: true,
            csa: 0,
            cld: 1,
        });
        cache.sync_clut(tex4hl, texclut, &vram);
        let texa = TexAReg(0x40 | (0xFFu64 << 32));
        let ctx_hl = ContextRegs {
            tex0: tex4hl,
            ..ContextRegs::default()
        };
        let ctx_hh = ContextRegs {
            tex0: tex4hh,
            ..ContextRegs::default()
        };
        assert_eq!(
            sample_nearest_texel(&vram, &cache, &ctx_hl, texa, 0, 0),
            [0x00, 0xF8, 0x00, 0x40]
        );
        assert_eq!(
            sample_nearest_texel(&vram, &cache, &ctx_hh, texa, 0, 0),
            [0xF8, 0x00, 0xF8, 0xFF]
        );
    }

    #[test]
    fn tfx_all_modes_with_tcc_0_and_1() {
        let v = [128, 64, 192, 96];
        let t = [100, 80, 40, 60];
        let t0 = |tcc, tfx| {
            make_tex0(Tex0Spec {
                tbp0: 0,
                tbw: 1,
                psm: 0,
                tw: 4,
                th: 4,
                tcc,
                tfx,
                cbp: 0,
                cpsm: 0,
                csm2: false,
                csa: 0,
                cld: 0,
            })
        };

        // Modulate (0): (128*100)>>7=100, (64*80)>>7=40, (192*40)>>7=60; A: 96 vs (96*60)>>7=45
        assert_eq!(apply_tfx(t0(false, 0), v, t), 0x603C_2864);
        assert_eq!(apply_tfx(t0(true, 0), v, t), 0x2D3C_2864);
        // Decal (1): RGB=texel, A: 96 vs 60
        assert_eq!(apply_tfx(t0(false, 1), v, t), 0x6028_5064);
        assert_eq!(apply_tfx(t0(true, 1), v, t), 0x3C28_5064);
        // Highlight (2): RGB=(100+96, 40+96, 60+96)=(196,136,156)=0xC4,0x88,0x9C; A: 96 vs 60+96=156
        assert_eq!(apply_tfx(t0(false, 2), v, t), 0x609C_88C4);
        assert_eq!(apply_tfx(t0(true, 2), v, t), 0x9C9C_88C4);
        // Highlight2 (3): same RGB as Highlight; A: 96 vs 60
        assert_eq!(apply_tfx(t0(false, 3), v, t), 0x609C_88C4);
        assert_eq!(apply_tfx(t0(true, 3), v, t), 0x3C9C_88C4);
    }
}
