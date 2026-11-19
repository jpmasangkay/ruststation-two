//! GS Render Gate suites (Phase 2b): replays checked-in `.gsp` packet scripts against
//! `ps2_gs::GsState` and verifies VRAM/register outputs against independent expectations.

use std::fs;
use std::path::{Path, PathBuf};

use ps2_gs::{execute_gsp_script, GsState, PixelFormat};

pub struct GsGateSuite {
    pub name: &'static str,
    pub rel_path: &'static str,
    verify: fn(&GsState) -> Result<(), String>,
}

pub static GS_RENDER_GATE_SUITES: [GsGateSuite; 16] = [
    GsGateSuite {
        name: "gs_render/01_host_to_local_psmct32",
        rel_path: "01_host_to_local_psmct32.gsp",
        verify: verify_01_host_to_local_psmct32,
    },
    GsGateSuite {
        name: "gs_render/02_swizzle_t8_t4_24",
        rel_path: "02_swizzle_t8_t4_24.gsp",
        verify: verify_02_swizzle_t8_t4_24,
    },
    GsGateSuite {
        name: "gs_render/03_local_to_local_dir",
        rel_path: "03_local_to_local_dir.gsp",
        verify: verify_03_local_to_local_dir,
    },
    GsGateSuite {
        name: "gs_render/04_sprite_flat",
        rel_path: "04_sprite_flat.gsp",
        verify: verify_04_sprite_flat,
    },
    GsGateSuite {
        name: "gs_render/05_triangle_flat",
        rel_path: "05_triangle_flat.gsp",
        verify: verify_05_triangle_flat,
    },
    GsGateSuite {
        name: "gs_render/06_triangle_gouraud",
        rel_path: "06_triangle_gouraud.gsp",
        verify: verify_06_triangle_gouraud,
    },
    GsGateSuite {
        name: "gs_render/07_ztest_overlap",
        rel_path: "07_ztest_overlap.gsp",
        verify: verify_07_ztest_overlap,
    },
    GsGateSuite {
        name: "gs_render/08_scissor_clip",
        rel_path: "08_scissor_clip.gsp",
        verify: verify_08_scissor_clip,
    },
    GsGateSuite {
        name: "gs_render/09_fbmsk_psm16_24_adc",
        rel_path: "09_fbmsk_psm16_24_adc.gsp",
        verify: verify_09_fbmsk_psm16_24_adc,
    },
    GsGateSuite {
        name: "gs_render/10_tex_psmct32_uv_modulate",
        rel_path: "10_tex_psmct32_uv_modulate.gsp",
        verify: verify_10_tex_psmct32_uv_modulate,
    },
    GsGateSuite {
        name: "gs_render/11_tex_psmt8_csm1_swizzle",
        rel_path: "11_tex_psmt8_csm1_swizzle.gsp",
        verify: verify_11_tex_psmt8_csm1_swizzle,
    },
    GsGateSuite {
        name: "gs_render/12_tex_psmt4hl_hh",
        rel_path: "12_tex_psmt4hl_hh.gsp",
        verify: verify_12_tex_psmt4hl_hh,
    },
    GsGateSuite {
        name: "gs_render/13_tex_clamp_modes",
        rel_path: "13_tex_clamp_modes.gsp",
        verify: verify_13_tex_clamp_modes,
    },
    GsGateSuite {
        name: "gs_render/14_tex_texa_aem_csm2_cld",
        rel_path: "14_tex_texa_aem_csm2_cld.gsp",
        verify: verify_14_tex_texa_aem_csm2_cld,
    },
    GsGateSuite {
        name: "gs_render/15_tex_stq_perspective",
        rel_path: "15_tex_stq_perspective.gsp",
        verify: verify_15_tex_stq_perspective,
    },
    GsGateSuite {
        name: "gs_render/16_tex_tfx_tcc",
        rel_path: "16_tex_tfx_tcc.gsp",
        verify: verify_16_tex_tfx_tcc,
    },
];

/// One pixel where our exact-floor expectation intentionally stays as the gate check while
/// PCSX2 v2.8.2 Software is documented as selecting a different texel (`KNOWN-DIFFERENCE`).
pub struct KnownDiffPixel {
    pub x: u32,
    pub y: u32,
    pub our_texel: u8,
    pub pcsx2_texel: u8,
    pub our_color: u32,
    pub pcsx2_color: u32,
}

pub struct Pcsx2ObservedSuite {
    pub suite: GsGateSuite,
    pub known_difference_reason: Option<&'static str>,
    pub known_diff_pixels: &'static [KnownDiffPixel],
}

/// All 9 interior positive exact-integer `u` pixels in `gs_probes/16_tex_tri_narrow` where
/// PCSX2 v2.8.2 Software selects `u - 1` while `ps2-gs` retains exact rational floor (`G-022`).
pub static PROBE_16_KNOWN_DIFF_PIXELS: [KnownDiffPixel; 9] = [
    KnownDiffPixel {
        x: 55,
        y: 30,
        our_texel: 12,
        pcsx2_texel: 11,
        our_color: 0x80C0_3050,
        pcsx2_color: 0x8040_E0F0,
    },
    KnownDiffPixel {
        x: 51,
        y: 40,
        our_texel: 9,
        pcsx2_texel: 8,
        our_color: 0x8090_20C0,
        pcsx2_color: 0x8020_B040,
    },
    KnownDiffPixel {
        x: 47,
        y: 50,
        our_texel: 6,
        pcsx2_texel: 5,
        our_color: 0x8050_E010,
        pcsx2_color: 0x80B0_3070,
    },
    KnownDiffPixel {
        x: 43,
        y: 60,
        our_texel: 3,
        pcsx2_texel: 2,
        our_color: 0x80E0_1030,
        pcsx2_color: 0x8010_70F0,
    },
    KnownDiffPixel {
        x: 50,
        y: 90,
        our_texel: 9,
        pcsx2_texel: 8,
        our_color: 0x8090_20C0,
        pcsx2_color: 0x8020_B040,
    },
    KnownDiffPixel {
        x: 46,
        y: 100,
        our_texel: 6,
        pcsx2_texel: 5,
        our_color: 0x8050_E010,
        pcsx2_color: 0x80B0_3070,
    },
    KnownDiffPixel {
        x: 42,
        y: 110,
        our_texel: 3,
        pcsx2_texel: 2,
        our_color: 0x80E0_1030,
        pcsx2_color: 0x8010_70F0,
    },
    KnownDiffPixel {
        x: 45,
        y: 150,
        our_texel: 6,
        pcsx2_texel: 5,
        our_color: 0x8050_E010,
        pcsx2_color: 0x80B0_3070,
    },
    KnownDiffPixel {
        x: 41,
        y: 160,
        our_texel: 3,
        pcsx2_texel: 2,
        our_color: 0x80E0_1030,
        pcsx2_color: 0x8010_70F0,
    },
];

pub static PCSX2_OBSERVED_SUITES: [Pcsx2ObservedSuite; 7] = [
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_probes/10_subpixel_probe",
            rel_path: "10_subpixel_probe.gsp",
            verify: verify_10_subpixel_probe,
        },
        known_difference_reason: None,
        known_diff_pixels: &[],
    },
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_render/10_tex_psmct32_uv_modulate",
            rel_path: "gs_render/10_tex_psmct32_uv_modulate.gsp",
            verify: verify_10_tex_psmct32_uv_modulate,
        },
        known_difference_reason: None,
        known_diff_pixels: &[],
    },
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_render/11_tex_psmt8_csm1_swizzle",
            rel_path: "gs_render/11_tex_psmt8_csm1_swizzle.gsp",
            verify: verify_11_tex_psmt8_csm1_swizzle,
        },
        known_difference_reason: None,
        known_diff_pixels: &[],
    },
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_render/12_tex_psmt4hl_hh",
            rel_path: "gs_render/12_tex_psmt4hl_hh.gsp",
            verify: verify_12_tex_psmt4hl_hh,
        },
        known_difference_reason: None,
        known_diff_pixels: &[],
    },
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_probes/14_tex_tri_wide_uv",
            rel_path: "14_tex_tri_wide_uv.gsp",
            verify: verify_14_tex_tri_wide_uv,
        },
        known_difference_reason: None,
        known_diff_pixels: &[],
    },
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_probes/15_tex_tri_stq_perspective",
            rel_path: "15_tex_tri_stq_perspective.gsp",
            verify: verify_15_tex_tri_stq_perspective,
        },
        known_difference_reason: None,
        known_diff_pixels: &[],
    },
    Pcsx2ObservedSuite {
        suite: GsGateSuite {
            name: "gs_probes/16_tex_tri_narrow",
            rel_path: "16_tex_tri_narrow.gsp",
            verify: verify_16_tex_tri_narrow,
        },
        known_difference_reason: Some(
            "9/1900 exact-integer u>0 ties differ from PCSX2 v2.8.2 Software (PCSX2=u-1; see G-022 & HARDWARE_NOTES.md §15)",
        ),
        known_diff_pixels: &PROBE_16_KNOWN_DIFF_PIXELS,
    },
];

fn expect_pixel(
    gs: &GsState,
    format: PixelFormat,
    fbp: u32,
    fbw: u32,
    (x, y): (u32, u32),
    expected: u32,
    label: &str,
) -> Result<(), String> {
    let got = gs.vram.read_page_pixel_raw(format, fbp, fbw, x, y);
    if got != expected {
        return Err(format!(
            "{label} ({x},{y}) expected {expected:#010X}, got {got:#010X}"
        ));
    }
    Ok(())
}

fn resolve_fixture_path(subdir: &str, rel_path: &str) -> PathBuf {
    for candidate in [
        Path::new("tests").join(subdir).join(rel_path),
        Path::new("tests").join(rel_path),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests")
            .join(subdir)
            .join(rel_path),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests")
            .join(rel_path),
    ] {
        if candidate.exists() {
            return candidate;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .join(subdir)
        .join(rel_path)
}

fn verify_01_host_to_local_psmct32(gs: &GsState) -> Result<(), String> {
    // Independently computed raw VRAM word offset for (x=2, y=1) in block 0, col 0: COL_32[1][2] = 6.
    if gs.vram.words[6] != 0xAA00_0001 {
        return Err(format!(
            "raw word[6] mismatch for (2,1): expected 0xAA000001, got {:#010X}",
            gs.vram.words[6]
        ));
    }

    for y in 0..4u32 {
        for x in 0..8u32 {
            let expected = if (2..6).contains(&x) && (1..3).contains(&y) {
                let idx = (y - 1) * 4 + (x - 2) + 1;
                0xAA00_0000 | idx
            } else {
                0
            };
            let got = gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, x, y);
            if got != expected {
                return Err(format!(
                    "pixel ({x},{y}) expected {expected:#010X}, got {got:#010X}"
                ));
            }
        }
    }
    Ok(())
}

fn verify_02_swizzle_t8_t4_24(gs: &GsState) -> Result<(), String> {
    // 1) PSMT8 4x2 at dbp=32, dbw=2: values 0x10..=0x17.
    for y in 0..2u32 {
        for x in 0..4u32 {
            let expected = 0x10 + y * 4 + x;
            let got = gs.vram.read_pixel_raw(PixelFormat::Psmt8, 32, 2, x, y);
            if got != expected {
                return Err(format!(
                    "PSMT8 ({x},{y}) expected {expected:#04X}, got {got:#04X}"
                ));
            }
        }
    }

    // 2) PSMT4 8x1 at dbp=64, dbw=2: nibbles 1..=8.
    for x in 0..8u32 {
        let expected = x + 1;
        let got = gs.vram.read_pixel_raw(PixelFormat::Psmt4, 64, 2, x, 0);
        if got != expected {
            return Err(format!(
                "PSMT4 ({x},0) expected {expected:#X}, got {got:#X}"
            ));
        }
    }

    // 3) PSMT8H + PSMCT24 overlay at dbp=96, dbw=1 -> read via PSMCT32.
    let expected32 = [0x11AA_BBCC, 0x2212_3456, 0x3378_9ABC, 0x4400_0000];
    for (x, &exp) in expected32.iter().enumerate() {
        let got = gs
            .vram
            .read_pixel_raw(PixelFormat::Psmct32, 96, 1, x as u32, 0);
        if got != exp {
            return Err(format!(
                "PSMCT24/8H overlay x={x} expected {exp:#010X}, got {got:#010X}"
            ));
        }
    }
    Ok(())
}

fn verify_03_local_to_local_dir(gs: &GsState) -> Result<(), String> {
    if !gs.regs.priv_display.pmode.en1() {
        return Err("PMODE.EN1 was not latched".to_owned());
    }
    if gs.regs.priv_display.bgcolor.0 != 0x0011_2233 {
        return Err(format!(
            "BGCOLOR expected 0x00112233, got {:#X}",
            gs.regs.priv_display.bgcolor.0
        ));
    }

    for row in 0..4u32 {
        for col in 0..4u32 {
            let expected = 0x1001 + row * 4 + col;
            let dst_dbp0 =
                gs.vram
                    .read_pixel_raw(PixelFormat::Psmct32, 0, 1, (2047 + col) & 0x7FF, row);
            if dst_dbp0 != expected {
                return Err(format!(
                    "DIR=3 wrapped blit ({col},{row}) expected {expected:#X}, got {dst_dbp0:#X}"
                ));
            }
            let dst_dbp32 = gs
                .vram
                .read_pixel_raw(PixelFormat::Psmct32, 32, 1, col, row);
            if dst_dbp32 != expected {
                return Err(format!(
                    "DIR=0 copy ({col},{row}) expected {expected:#X}, got {dst_dbp32:#X}"
                ));
            }
        }
    }
    Ok(())
}

fn verify_04_sprite_flat(gs: &GsState) -> Result<(), String> {
    // Hand-derived coverage under half-open [min_x, max_x) x [min_y, max_y) with last-vertex color:
    //   Sprite 1: [0, 320) x [0, 240) -> BG    (0x8024_1814)
    //   Sprite 2: [32, 144) x [40, 160) -> AMBER (0x8020_90E8; v0 dummy red ignored)
    //   Sprite 3: [112, 288) x [88, 208) -> TEAL  (0x809C_A828; overwrites amber on [112,144)x[88,160))
    //   Sprite 4: [16, 24) x [16, 24) -> WHITE (0x80FF_FFFF)
    const BG: u32 = 0x8024_1814;
    const AMBER: u32 = 0x8020_90E8;
    const TEAL: u32 = 0x809C_A828;
    const WHITE: u32 = 0x80FF_FFFF;

    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (31, 40),
        BG,
        "bg left of amber",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (32, 40),
        AMBER,
        "amber top-left",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (111, 88),
        AMBER,
        "amber before teal",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (112, 88),
        TEAL,
        "teal top-left overlap",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (143, 159),
        TEAL,
        "overlap interior",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (144, 40),
        BG,
        "bg right of amber",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (287, 207),
        TEAL,
        "teal inner right-bottom",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (288, 208),
        BG,
        "teal exclusive end",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (16, 16),
        WHITE,
        "marker start",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (23, 23),
        WHITE,
        "marker end-1",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (24, 24),
        BG,
        "marker exclusive end",
    )?;
    Ok(())
}

fn verify_05_triangle_flat(gs: &GsState) -> Result<(), String> {
    const BG: u32 = 0x801C_1410;
    const CYAN: u32 = 0x80FF_B638;
    const GREEN: u32 = 0x8060_D040;
    const MAGENTA: u32 = 0x8090_40D0;
    const YELLOW: u32 = 0x8040_E0FF;
    const ORANGE: u32 = 0x8020_70FF;

    // Large center flat triangle uses last-vertex cyan.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (160, 120),
        CYAN,
        "center tri",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (159, 25),
        BG,
        "above tri apex",
    )?;
    // Shared diagonal [24..64) x [24..64): upper-right CW gets x >= y, lower-left CCW gets y > x.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (40, 40),
        GREEN,
        "shared diagonal",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (63, 24),
        GREEN,
        "top-right quad corner",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (64, 24),
        BG,
        "right edge excluded",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (39, 40),
        MAGENTA,
        "lower-left interior",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (24, 63),
        MAGENTA,
        "bottom-left corner",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (24, 64),
        BG,
        "bottom edge excluded",
    )?;
    // TriangleFan with ADC: wedge 1 drawn, wedge 2 culled, wedge 3 drawn.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (290, 34),
        YELLOW,
        "fan wedge 1",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (276, 56),
        BG,
        "fan wedge 2 ADC culled",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (250, 44),
        ORANGE,
        "fan wedge 3 after ADC",
    )?;
    Ok(())
}

fn verify_06_triangle_gouraud(gs: &GsState) -> Result<(), String> {
    const BG: u32 = 0x8014_0E0E;
    // Triangle (160,20) R=240, (280,200) G=240, (40,200) B=240.
    // Centroid (160,140): exact w0 = w1 = w2 = 1/3 -> RGB=(80, 80, 80) = 0x8050_5050.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (160, 140),
        0x8050_5050,
        "gouraud centroid",
    )?;
    // Mid-height center (160,110): w0 = 1/2, w1 = 1/4, w2 = 1/4 -> RGB=(120, 60, 60).
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (160, 110),
        0x803C_3C78,
        "gouraud midpoint",
    )?;
    // Left edge midpoint (100,110): top-left included, w0 = 1/2, w1 = 0, w2 = 1/2 -> RGB=(120, 0, 120).
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (100, 110),
        0x8078_0078,
        "gouraud left edge",
    )?;
    // Right edge midpoint (220,110): non-top-left excluded -> background.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (220, 110),
        BG,
        "gouraud right edge excluded",
    )?;
    Ok(())
}

fn verify_07_ztest_overlap(gs: &GsState) -> Result<(), String> {
    // Hand-derived crossing-depth barycentric intersection (`TEST_1` = GEQUAL, bg Z = 0, ZMSK = 0):
    //   Tri A: v0=(40,48) Z=1000 #2060FF, v1=(264,144) Z=100 #20E0FF, v2=(40,224) Z=1000 #1040D0
    //     area2_px = 224 * 176 = 39424.
    //     At (96, 136): (e0, e1, e2) = (15232, 9856, 14336) = 896 * (17, 11, 16), sum = 44*896.
    //       Z_A = (17*1000 + 11*100 + 16*1000)/44 = 34100/44 = 775.
    //       R_A = (17*32 + 11*32 + 16*16)/44 = 1152/44 = 26 (0x1A).
    //       G_A = (17*96 + 11*224 + 16*64)/44 = 5120/44 = 116 (0x74).
    //       B_A = (17*255 + 11*255 + 16*208)/44 = 10468/44 = 237 (0xED) -> 0x80ED_741A.
    //   Tri B: v0=(280,48) Z=1000 #FF8010, v1=(280,224) Z=1000 #E04010, v2=(56,144) Z=100 #FFC030
    //     At (96, 136): (e0, e1, e2) = (4992, 2048, 32384) => Z_B = 10278400/39424 = 260 < 775 (FAILS!).
    //     At (224, 136) (symmetric mirror): Tri A gives Z_A = 260; Tri B gives weights (17, 16, 11)/44,
    //       Z_B = 775 >= 260 (PASSES!):
    //       R_B = (17*255 + 16*224 + 11*255)/44 = 10724/44 = 243 (0xF3).
    //       G_B = (17*128 + 16*64 + 11*192)/44 = 5312/44 = 120 (0x78).
    //       B_B = (17*16 + 16*16 + 11*48)/44 = 1056/44 = 24 (0x18) -> 0x8018_78F3.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (96, 136),
        0x80ED_741A,
        "left crossing pixel (Tri A Z=775 > Tri B Z=260)",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (224, 136),
        0x8018_78F3,
        "right crossing pixel (Tri B Z=775 >= Tri A Z=260)",
    )?;

    // Control band: Box 0 NEVER (stays green 0x8050_C030), Box 1 ZMSK=1 allows Z=600 yellow (0x8030_F0F0),
    // Box 2 GEQUAL eq passes yellow (0x8030_F0F0), Box 3 GREATER eq fails green / Z=501 passes blue (0x80FF_6030).
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (40, 22),
        0x8050_C030,
        "ZTST NEVER",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (100, 22),
        0x8030_F0F0,
        "ZMSK=1 preserves base Z",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (160, 22),
        0x8030_F0F0,
        "ZTST GEQUAL equal",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (205, 15),
        0x8050_C030,
        "ZTST GREATER equal fails",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (220, 22),
        0x80FF_6030,
        "ZTST GREATER gt passes",
    )?;
    Ok(())
}

fn verify_08_scissor_clip(gs: &GsState) -> Result<(), String> {
    const BG: u32 = 0x8022_161A;
    const FRAME_BORDER: u32 = 0x80C0_C0C0;
    const CORAL: u32 = 0x8058_68F0;

    // Guide border LineStrip (39,29)->(280,29)->(280,210)->(39,210)->(39,29) is a closed loop
    // (`v4 == v0`), so every dropped half-open segment end equals the next/first segment's start:
    // 0 pixels change under `IntegerCenterHalfOpenLine`, and all 4 corners remain FRAME_BORDER.
    for corner in [(39, 29), (280, 29), (280, 210), (39, 210)] {
        expect_pixel(
            gs,
            PixelFormat::Psmct32,
            0,
            5,
            corner,
            FRAME_BORDER,
            "closed-loop guide border corner",
        )?;
    }
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (39, 30),
        FRAME_BORDER,
        "left border",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (280, 209),
        FRAME_BORDER,
        "right border",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (160, 25),
        BG,
        "outside scissor top",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (30, 50),
        BG,
        "outside scissor left",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (290, 50),
        BG,
        "outside scissor right",
    )?;
    // Inside scissor [40..=279] x [30..=209]: coral banner at (50, 40), Gouraud triangle at (160, 30).
    // Hand derivation at (160, 30) after subtracting XYOFFSET=(2048,2048):
    //   v0=(160,0) RGB=(255,224,48), v1=(318,238) RGB=(64,224,255), v2=(2,238) RGB=(192,64,255).
    //   area2_px = 2 * 158 * 238 = 75208. At (160,30): e0=316*208, e1=158*30, e2=158*30 -> (208,15,15)/238.
    //   R = (208*255 + 15*64 + 15*192)/238 = 56880/238 = 238 (0xEE)
    //   G = (208*224 + 15*224 + 15*64)/238 = 50912/238 = 213 (0xD5)
    //   B = (208*48 + 15*255 + 15*255)/238 = 17634/238 = 74  (0x4A), A = 0x80 -> 0x804A_D5EE.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (50, 40),
        CORAL,
        "clipped banner",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        5,
        (160, 30),
        0x804A_D5EE,
        "clipped triangle apex interpolation",
    )?;
    Ok(())
}

fn verify_09_fbmsk_psm16_24_adc(gs: &GsState) -> Result<(), String> {
    // 09_fbmsk_psm16_24_adc contains only Sprite, Point (12.125,4.25)->(12,4), and TriangleStrip
    // primitives (no Line/LineStrip), so 0 pixels change under `IntegerCenterHalfOpenLine`.
    // 1. PSMCT24 preserving upper 8H byte 0xAB and masked Green channel 0x22.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        1,
        (1, 1),
        0xAB99_2277,
        "PSMCT24 + FBMSK green mask",
    )?;
    // Outside [0..4)x[0..4) retains original seeded 0xAB11_2233.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        1,
        (5, 5),
        0xAB11_2233,
        "PSMCT24 outside draw",
    )?;
    // 2. PSMCT16 at page 2 with FBMSK = 0x8000_00FF preserving R5=31 and A1=1.
    expect_pixel(
        gs,
        PixelFormat::Psmct16,
        2,
        1,
        (1, 1),
        0x83FF,
        "PSMCT16 bit-63 alpha preserve",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct16,
        2,
        1,
        (5, 5),
        0x801F,
        "PSMCT16 unmasked region",
    )?;
    // 3. Snapped point at (12, 4).
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        1,
        (12, 4),
        0x8012_3456,
        "Point snap",
    )?;
    // 4. TriangleStrip with ADC: tri1 drawn, tri2 culled, tri3 drawn.
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        1,
        (22, 6),
        0x8000_FF00,
        "TriStrip tri1",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        1,
        (26, 10),
        0,
        "TriStrip ADC tri2 culled",
    )?;
    expect_pixel(
        gs,
        PixelFormat::Psmct32,
        0,
        1,
        (26, 14),
        0x80FF_4020,
        "TriStrip tri3 after ADC",
    )?;
    Ok(())
}

fn verify_10_tex_psmct32_uv_modulate(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // Hand-derived from 10_tex_psmct32_uv_modulate.gsp (320x240, FBP=0, FBW=5):
    // bg = 0x801C_1410; texel(tx, ty) = (24 + 14*tx, 24 + 14*ty, 0xE0 or 0x20, 0x80).
    expect_pixel(gs, f, 0, 5, (0, 0), 0x801C_1410, "bg")?;
    // Sprite A (1x) [24..40, 32..48] with RGBA=(0x80,0x80,0x80,0x80) -> identity
    expect_pixel(gs, f, 0, 5, (23, 32), 0x801C_1410, "spriteA outside left")?;
    expect_pixel(gs, f, 0, 5, (24, 32), 0x80E0_1818, "spriteA texel(0,0)")?;
    expect_pixel(gs, f, 0, 5, (27, 34), 0x8020_3442, "spriteA texel(3,2)")?;
    expect_pixel(gs, f, 0, 5, (39, 47), 0x80E0_EAEA, "spriteA texel(15,15)")?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (40, 47),
        0x801C_1410,
        "spriteA half-open right",
    )?;
    // Sprite B (2x) [64..96, 32..64]
    expect_pixel(gs, f, 0, 5, (64, 32), 0x80E0_1818, "spriteB 2x (0,0) a")?;
    expect_pixel(gs, f, 0, 5, (65, 33), 0x80E0_1818, "spriteB 2x (0,0) b")?;
    expect_pixel(gs, f, 0, 5, (66, 32), 0x8020_1826, "spriteB 2x (1,0)")?;
    expect_pixel(gs, f, 0, 5, (95, 63), 0x80E0_EAEA, "spriteB 2x (15,15)")?;
    // Sprite C (2x + Modulate RGBA=(0x40, 0xFF, 0x80, 0x80)) [128..160, 32..64]
    // (0,0): R=(0x40*24)>>7=12(0x0C), G=(0xFF*24)>>7=47(0x2F), B=224(0xE0), A=0x80
    expect_pixel(gs, f, 0, 5, (128, 32), 0x80E0_2F0C, "spriteC mod (0,0)")?;
    // (15,15): R=(64*234)>>7=117(0x75), G=min(255,(255*234)>>7)=255(0xFF), B=224(0xE0), A=0x80
    expect_pixel(gs, f, 0, 5, (158, 62), 0x80E0_FF75, "spriteC mod (15,15)")?;
    Ok(())
}

fn verify_11_tex_psmt8_csm1_swizzle(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // Hand-derived from 11_tex_psmt8_csm1_swizzle.gsp (320x240, FBP=0, FBW=5):
    // In CSM1 8-bit CLUT, bit3 <-> bit4 swap means indices 8..15 live at VRAM row y=1 x=0..7 (green/cyan)
    // and indices 16..23 live at VRAM row y=0 x=8..15 (orange/red).
    expect_pixel(gs, f, 0, 5, (0, 0), 0x8000_0000, "bg")?;
    // 1x sprite [24..40, 32..36]:
    expect_pixel(gs, f, 0, 5, (24, 32), 0x80A0_3030, "1x idx 0 (blue)")?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (32, 32),
        0x8020_F010,
        "1x idx 8 (green, swapped)",
    )?;
    expect_pixel(gs, f, 0, 5, (39, 32), 0x80C8_F010, "1x idx 15 (green-cyan)")?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (24, 33),
        0x8010_20F0,
        "1x idx 16 (orange, swapped)",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (31, 33),
        0x8010_C8F0,
        "1x idx 23 (yellow-orange)",
    )?;
    expect_pixel(gs, f, 0, 5, (24, 34), 0x8020_F010, "1x row 2 idx 8")?;
    // 2x sprite [64..96, 48..56]:
    expect_pixel(gs, f, 0, 5, (80, 48), 0x8020_F010, "2x idx 8 a")?;
    expect_pixel(gs, f, 0, 5, (81, 49), 0x8020_F010, "2x idx 8 b")?;
    expect_pixel(gs, f, 0, 5, (64, 50), 0x8010_20F0, "2x idx 16")?;
    expect_pixel(gs, f, 0, 5, (94, 54), 0x8020_F010, "2x row 3 col 15 idx 8")?;
    Ok(())
}

fn verify_12_tex_psmt4hl_hh(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // Hand-derived from 12_tex_psmt4hl_hh.gsp (320x240, FBP=0, FBW=5):
    // PSMT4HL [24..32, 32..36] & [48..64, 32..40]: nibble=1 GREEN (0x8000_FF00) / nibble=3 BLUE (0x80FF_8000)
    expect_pixel(gs, f, 0, 5, (24, 32), 0x8000_FF00, "4HL 1x left green")?;
    expect_pixel(gs, f, 0, 5, (28, 35), 0x80FF_8000, "4HL 1x right blue")?;
    expect_pixel(gs, f, 0, 5, (48, 32), 0x8000_FF00, "4HL 2x left green")?;
    expect_pixel(gs, f, 0, 5, (63, 39), 0x80FF_8000, "4HL 2x right blue")?;
    // PSMT4HH [88..96, 32..36] & [112..128, 32..40]: nibble=2 MAGENTA (0x80FF_00FF) / nibble=4 YELLOW (0x8000_FFFF)
    expect_pixel(gs, f, 0, 5, (88, 32), 0x80FF_00FF, "4HH 1x left magenta")?;
    expect_pixel(gs, f, 0, 5, (92, 35), 0x8000_FFFF, "4HH 1x right yellow")?;
    expect_pixel(gs, f, 0, 5, (112, 32), 0x80FF_00FF, "4HH 2x left magenta")?;
    expect_pixel(gs, f, 0, 5, (127, 39), 0x8000_FFFF, "4HH 2x right yellow")?;
    Ok(())
}

fn verify_13_tex_clamp_modes(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // Texture row y=2 (T=0.25 -> v=2, G=16+2*30=76=0x4C): pixel(u) = 0x8040_4C00 | (16 + u*30)
    let texel = |u: u32| -> u32 { 0x8040_4C00 | (16 + u * 30) };
    // S runs from -0.5 (-4) to +1.5 (+12) over x=0..16 -> raw iu = x - 4.
    // Band 0 (y=0..4): WMS=0 REPEAT -> u = (x - 4) & 7
    expect_pixel(gs, f, 0, 1, (0, 1), texel(4), "repeat x=0 -> u=4")?;
    expect_pixel(gs, f, 0, 1, (4, 1), texel(0), "repeat x=4 -> u=0")?;
    expect_pixel(gs, f, 0, 1, (15, 1), texel(3), "repeat x=15 -> u=3")?;
    // Band 1 (y=8..12): WMS=1 CLAMP -> clamp(iu, 0, 7)
    expect_pixel(gs, f, 0, 1, (0, 9), texel(0), "clamp x=0 -> u=0")?;
    expect_pixel(gs, f, 0, 1, (9, 9), texel(5), "clamp x=9 -> u=5")?;
    expect_pixel(gs, f, 0, 1, (15, 9), texel(7), "clamp x=15 -> u=7")?;
    // Band 2 (y=16..20): WMS=2 REGION_CLAMP [2, 5]
    expect_pixel(gs, f, 0, 1, (0, 17), texel(2), "region_clamp x=0 -> u=2")?;
    expect_pixel(gs, f, 0, 1, (7, 17), texel(3), "region_clamp x=7 -> u=3")?;
    expect_pixel(gs, f, 0, 1, (15, 17), texel(5), "region_clamp x=15 -> u=5")?;
    // Band 3 (y=24..28): WMS=3 REGION_REPEAT (umsk=3, ufix=4) -> (iu & 3) | 4
    expect_pixel(gs, f, 0, 1, (0, 25), texel(4), "region_repeat x=0 -> u=4")?;
    expect_pixel(gs, f, 0, 1, (3, 25), texel(7), "region_repeat x=3 -> u=7")?;
    expect_pixel(gs, f, 0, 1, (4, 25), texel(4), "region_repeat x=4 -> u=4")?;
    Ok(())
}

fn verify_14_tex_texa_aem_csm2_cld(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // Part 1: PSMCT16 TEXA (TA0=0x30, AEM=1, TA1=0xB0) at x=0..4, y=0..2
    expect_pixel(gs, f, 0, 1, (0, 0), 0x0000_0000, "texa aem rgb=0 a1=0")?;
    expect_pixel(gs, f, 0, 1, (1, 0), 0x3000_00F8, "texa ta0 non-black a1=0")?;
    expect_pixel(gs, f, 0, 1, (2, 0), 0x0000_0000, "texa aem rgb=0 a1=1")?;
    expect_pixel(gs, f, 0, 1, (3, 0), 0xB000_F800, "texa ta1 non-black a1=1")?;
    // Part 2: CSM2 + TEXCLUT + CLD 0..=5 sprites across x=8..60, y=0..2
    let green = 0xB000_F800;
    let red = 0xB000_00F8;
    let blue = 0xB0F8_0000;
    expect_pixel(gs, f, 0, 1, (8, 0), green, "cld=2 loads green")?;
    expect_pixel(gs, f, 0, 1, (16, 0), green, "cld=0 retains green")?;
    expect_pixel(gs, f, 0, 1, (24, 0), green, "cld=4 same cbp retains green")?;
    expect_pixel(gs, f, 0, 1, (32, 0), red, "cld=1 reloads red")?;
    expect_pixel(gs, f, 0, 1, (40, 0), blue, "cld=3 cbp1+texclut loads blue")?;
    expect_pixel(gs, f, 0, 1, (48, 0), blue, "cld=5 same cbp1 retains blue")?;
    expect_pixel(gs, f, 0, 1, (56, 0), red, "cld=5 diff cbp reloads red")?;
    Ok(())
}

fn verify_15_tex_stq_perspective(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    let texel =
        |u: u32| -> u32 { 0x8000_0000 | ((0xFF - 20 * u) << 16) | (0x80 << 8) | (20 + 30 * u) };
    // Sprite [0..16, 0..8], S=0..2, Q=2..4 (S/Q = 0..0.5 -> texels 0..3)
    expect_pixel(gs, f, 0, 1, (0, 2), texel(0), "stq sprite x=0 -> texel 0")?;
    expect_pixel(gs, f, 0, 1, (8, 2), texel(2), "stq sprite x=8 -> texel 2")?;
    expect_pixel(gs, f, 0, 1, (15, 2), texel(3), "stq sprite x=15 -> texel 3")?;
    // Sprite Q==0 -> 1.0 [0..8, 10..18]
    expect_pixel(gs, f, 0, 1, (0, 12), texel(0), "stq q==0 x=0 -> texel 0")?;
    expect_pixel(gs, f, 0, 1, (6, 12), texel(3), "stq q==0 x=6 -> texel 3")?;
    // Triangle (8,24)->(56,24)->(8,56), Q=(1,2,1), S=(0,2,0)
    // Midpoint (32,24): S=1.0, Q=1.5 -> u=(1.0/1.5)*8 = 5.333 -> texel 5 (linear would be 4)
    expect_pixel(gs, f, 0, 1, (8, 24), texel(0), "stq tri v0 -> texel 0")?;
    expect_pixel(
        gs,
        f,
        0,
        1,
        (32, 24),
        texel(5),
        "stq tri midpoint -> perspective texel 5",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        1,
        (44, 24),
        texel(6),
        "stq tri 3/4 -> perspective texel 6",
    )?;
    Ok(())
}

fn verify_16_tex_tfx_tcc(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // Texel=(100,80,40,60), Vertex=(128,64,192,96)
    // Row y=0..6 (TCC=0):
    expect_pixel(gs, f, 0, 1, (1, 1), 0x603C_2864, "tfx=0 mod tcc=0")?;
    expect_pixel(gs, f, 0, 1, (9, 1), 0x6028_5064, "tfx=1 decal tcc=0")?;
    expect_pixel(gs, f, 0, 1, (17, 1), 0x609C_88C4, "tfx=2 hi tcc=0")?;
    expect_pixel(gs, f, 0, 1, (25, 1), 0x609C_88C4, "tfx=3 hi2 tcc=0")?;
    // Row y=16..22 (TCC=1):
    expect_pixel(gs, f, 0, 1, (1, 17), 0x2D3C_2864, "tfx=0 mod tcc=1")?;
    expect_pixel(gs, f, 0, 1, (9, 17), 0x3C28_5064, "tfx=1 decal tcc=1")?;
    expect_pixel(gs, f, 0, 1, (17, 17), 0x9C9C_88C4, "tfx=2 hi tcc=1")?;
    expect_pixel(gs, f, 0, 1, (25, 17), 0x3C9C_88C4, "tfx=3 hi2 tcc=1")?;
    Ok(())
}

fn expect_bbox_pixels(
    gs: &GsState,
    label: &str,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
    expected_color: u32,
    expected_coords: &[(u32, u32)],
) -> Result<(), String> {
    const BG: u32 = 0x8000_0000;
    let mut got_coords = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let raw = gs
                .vram
                .read_page_pixel_raw(PixelFormat::Psmct32, 0, 5, x, y);
            if raw != BG {
                if raw != expected_color {
                    return Err(format!(
                        "{label} at ({x},{y}) expected color {expected_color:#010X}, got {raw:#010X}"
                    ));
                }
                got_coords.push((x, y));
            }
        }
    }
    let mut expected_sorted = expected_coords.to_vec();
    expected_sorted.sort_unstable_by_key(|&(x, y)| (y, x));
    if got_coords != expected_sorted {
        return Err(format!(
            "{label} pixel set mismatch: expected {expected_sorted:?}, got {got_coords:?}"
        ));
    }
    Ok(())
}

fn verify_10_subpixel_probe(gs: &GsState) -> Result<(), String> {
    // PCSX2 v2.8.2 Software observed exact pixel sets under `IntegerCenterHalfOpenLine`:
    // Points P0..P6 (`7/7` match, half-up integer centers):
    expect_bbox_pixels(
        gs,
        "P0 (20.0,20.0)",
        (18, 18, 22, 22),
        0x8040_40FF,
        &[(20, 20)],
    )?;
    expect_bbox_pixels(
        gs,
        "P1 (36.4375,20.0)",
        (34, 18, 38, 22),
        0x8040_FF40,
        &[(36, 20)],
    )?;
    expect_bbox_pixels(
        gs,
        "P2 (52.5,20.0)",
        (50, 18, 55, 22),
        0x80FF_8040,
        &[(53, 20)],
    )?;
    expect_bbox_pixels(
        gs,
        "P3 (68.5625,20.0)",
        (66, 18, 71, 22),
        0x8040_FFFF,
        &[(69, 20)],
    )?;
    expect_bbox_pixels(
        gs,
        "P4 (36.0,40.4375)",
        (34, 38, 38, 42),
        0x80FF_FF40,
        &[(36, 40)],
    )?;
    expect_bbox_pixels(
        gs,
        "P5 (52.0,40.5)",
        (50, 38, 54, 43),
        0x80FF_40FF,
        &[(52, 41)],
    )?;
    expect_bbox_pixels(
        gs,
        "P6 (68.0,40.5625)",
        (66, 38, 70, 43),
        0x8040_A0FF,
        &[(68, 41)],
    )?;

    // Half-open Bresenham lines L0..L5 (`6/6` match; final endpoints dropped):
    expect_bbox_pixels(
        gs,
        "L0 horiz L->R",
        (19, 79, 29, 81),
        0x80FF_FFFF,
        &[
            (20, 80),
            (21, 80),
            (22, 80),
            (23, 80),
            (24, 80),
            (25, 80),
            (26, 80),
            (27, 80),
        ],
    )?;
    expect_bbox_pixels(
        gs,
        "L1 horiz R->L",
        (49, 79, 59, 81),
        0x80FF_FF80,
        &[
            (51, 80),
            (52, 80),
            (53, 80),
            (54, 80),
            (55, 80),
            (56, 80),
            (57, 80),
            (58, 80),
        ],
    )?;
    expect_bbox_pixels(
        gs,
        "L2 vert T->B",
        (89, 75, 91, 85),
        0x8080_FFFF,
        &[
            (90, 76),
            (90, 77),
            (90, 78),
            (90, 79),
            (90, 80),
            (90, 81),
            (90, 82),
            (90, 83),
        ],
    )?;
    expect_bbox_pixels(
        gs,
        "L3 slope 0.5",
        (19, 109, 29, 115),
        0x8080_FF80,
        &[
            (20, 110),
            (21, 111),
            (22, 111),
            (23, 112),
            (24, 112),
            (25, 113),
            (26, 113),
            (27, 114),
        ],
    )?;
    expect_bbox_pixels(
        gs,
        "L4 steep -dY",
        (49, 109, 55, 119),
        0x80C0_80FF,
        &[
            (54, 111),
            (53, 112),
            (53, 113),
            (52, 114),
            (52, 115),
            (51, 116),
            (51, 117),
            (50, 118),
        ],
    )?;
    expect_bbox_pixels(gs, "L5 zero-length", (79, 109, 81, 111), 0x8040_C0FF, &[])?;

    // Sprites S0..S2 (`3/3` match):
    let rect_coords = |x0: u32, y0: u32, x1: u32, y1: u32| -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.push((x, y));
            }
        }
        out
    };
    expect_bbox_pixels(
        gs,
        "S0 normal",
        (19, 149, 29, 159),
        0x80D0_E040,
        &rect_coords(20, 150, 27, 157),
    )?;
    expect_bbox_pixels(
        gs,
        "S1 reversed corners",
        (49, 149, 59, 159),
        0x8050_70FF,
        &rect_coords(50, 150, 57, 157),
    )?;
    expect_bbox_pixels(
        gs,
        "S2 subpixel [80.5,84.5)",
        (79, 149, 86, 155),
        0x80FF_A0C0,
        &rect_coords(81, 150, 84, 153),
    )?;
    Ok(())
}

/// Frozen hand-derived predictions from `docs/GS_GUESSES.md` (commit `98ea99d`).
fn verify_14_tex_tri_wide_uv(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    expect_pixel(gs, f, 0, 5, (20, 30), 0x8020_20E0, "dx=0 u=0 -> texel 0")?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (84, 40),
        0x8010_70F0,
        "dx=64 u=832/281 -> texel 2",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (85, 40),
        0x80E0_1030,
        "dx=65 u=845/281 (+2/281) -> texel 3",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (128, 40),
        0x8030_F0D0,
        "dx=108 u=1404/281 (-1/281) -> texel 4",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (193, 40),
        0x8020_B040,
        "dx=173 u=2249/281 (+1/281) -> texel 8",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (236, 40),
        0x8090_20C0,
        "dx=216 u=2808/281 (-2/281) -> texel 9",
    )?;
    Ok(())
}

/// Frozen hand-derived predictions from `docs/GS_GUESSES.md` (commit `98ea99d`).
fn verify_15_tex_tri_stq_perspective(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (36, 40),
        0x80F0_D020,
        "dx=16 u=624/313 (-2/313) -> texel 1",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (68, 40),
        0x8030_F0D0,
        "dx=48 u=1872/377 (-13/377) -> texel 4",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (83, 40),
        0x8050_E010,
        "dx=63 u=2457/407 (+15/407) -> texel 6",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (160, 40),
        0x8090_20C0,
        "dx=140 midpoint u=5460/561 -> texel 9",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (168, 40),
        0x80F0_9020,
        "dx=148 u=5772/577 (+2/577) -> texel 10",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (245, 40),
        0x80C0_3050,
        "dx=225 u=8775/731 (+3/731) -> texel 12",
    )?;
    Ok(())
}

/// Frozen exact-floor expectations for `16_tex_tri_narrow` (commit `98ea99d`) plus verification
/// that all 9 `PROBE_16_KNOWN_DIFF_PIXELS` retain our exact-floor texels (`KNOWN-DIFFERENCE` vs PCSX2).
fn verify_16_tex_tri_narrow(gs: &GsState) -> Result<(), String> {
    let f = PixelFormat::Psmct32;
    // 6 frozen hand-derived predictions from commit 98ea99d:
    expect_pixel(
        gs,
        f,
        0,
        5,
        (40, 20),
        0x8020_20E0,
        "dx=0,dy=0 u=0 -> texel 0",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (41, 33),
        0x8020_20E0,
        "dx=1,dy=13 N=189 (-1/190) -> texel 0",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (41, 34),
        0x80F0_D020,
        "dx=1,dy=14 N=192 (+2/190) -> texel 1",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (45, 23),
        0x80E0_1030,
        "dx=5,dy=3 N=759 (-1/190) -> texel 3",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (45, 24),
        0x8030_F0D0,
        "dx=5,dy=4 N=762 (+2/190) -> texel 4",
    )?;
    expect_pixel(
        gs,
        f,
        0,
        5,
        (53, 67),
        0x8040_E0F0,
        "dx=13,dy=47 N=2091 (+1/190) -> texel 11",
    )?;
    // Verify our exact-floor output remains intact at all 9 KNOWN-DIFFERENCE integer-tie pixels:
    for p in &PROBE_16_KNOWN_DIFF_PIXELS {
        let label = format!(
            "exact-floor u={} (KNOWN-DIFF: PCSX2 v2.8.2 observes u={} {:#010X})",
            p.our_texel, p.pcsx2_texel, p.pcsx2_color
        );
        expect_pixel(gs, f, 0, 5, (p.x, p.y), p.our_color, &label)?;
    }
    Ok(())
}

/// Run one GS suite against `tests/<subdir>/<rel_path>` and return `Ok(())` on pass.
pub fn run_one_gs_suite(subdir: &str, suite: &GsGateSuite) -> Result<(), String> {
    let path = resolve_fixture_path(subdir, suite.rel_path);
    let src =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut gs = GsState::new();
    execute_gsp_script(&mut gs, &src)
        .map_err(|e| format!("parse error in {}: {e}", path.display()))?;
    (suite.verify)(&gs)
}

fn run_gs_suite_table(
    col_header: &str,
    banner_label: &str,
    subdir: &str,
    suites: &[GsGateSuite],
) -> (usize, Vec<(&'static str, String)>) {
    let col_w = col_header.len().max(36);
    println!("\n|  # | {:<col_w$} | {:<8} |", col_header, "Status");
    println!("|----|-{}-|----------|", "-".repeat(col_w));

    let mut passed = 0usize;
    let mut failures = Vec::new();
    for (idx, suite) in suites.iter().enumerate() {
        let status = match run_one_gs_suite(subdir, suite) {
            Ok(()) => {
                passed += 1;
                "PASS"
            }
            Err(err) => {
                failures.push((suite.name, err));
                "FAIL"
            }
        };
        println!(
            "| {:>2} | {:<col_w$} | {:<8} |",
            idx + 1,
            suite.name,
            status
        );
    }

    let total = suites.len();
    let failed = failures.len();
    println!("\n=== {banner_label}: {passed}/{total} PASSED, {failed} FAILED ===");
    (passed, failures)
}

/// Execute all checked-in GS Render Gate suites and print a summary table. Returns `(passed, failures)`.
pub fn run_gs_render_gate() -> (usize, Vec<(&'static str, String)>) {
    run_gs_suite_table(
        "GS Render Suite",
        "GS RENDER GATE",
        "gs_render",
        &GS_RENDER_GATE_SUITES,
    )
}

/// Execute PCSX2-observed (v2.8.2, not hardware) GS probe suites. Returns `(passed, failures)`.
/// Entries with `known_difference_reason` verify our exact-floor output and report `KNOWN-DIFF`
/// without failing the gate (analogous to EFU `DEFERRED`).
pub fn run_pcsx2_observed_gate() -> (usize, Vec<(&'static str, String)>) {
    let col_header = "PCSX2-observed (v2.8.2), not hardware";
    let col_w = col_header.len().max(38);
    println!("\n|  # | {:<col_w$} | {:<10} | Notes", col_header, "Status");
    println!("|----|-{}-|------------|------", "-".repeat(col_w));

    let mut passed = 0usize;
    let mut known_diff = 0usize;
    let mut failures = Vec::new();
    for (idx, entry) in PCSX2_OBSERVED_SUITES.iter().enumerate() {
        match run_one_gs_suite("gs_probes", &entry.suite) {
            Ok(()) => {
                if let Some(reason) = entry.known_difference_reason {
                    known_diff += 1;
                    println!(
                        "| {:>2} | {:<col_w$} | {:<10} | {} ({} px)",
                        idx + 1,
                        entry.suite.name,
                        "KNOWN-DIFF",
                        reason,
                        entry.known_diff_pixels.len()
                    );
                } else {
                    passed += 1;
                    println!(
                        "| {:>2} | {:<col_w$} | {:<10} |",
                        idx + 1,
                        entry.suite.name,
                        "PASS"
                    );
                }
            }
            Err(err) => {
                failures.push((entry.suite.name, err));
                println!(
                    "| {:>2} | {:<col_w$} | {:<10} |",
                    idx + 1,
                    entry.suite.name,
                    "FAIL"
                );
            }
        }
    }

    let total = PCSX2_OBSERVED_SUITES.len();
    let failed = failures.len();
    println!(
        "\n=== {col_header}: {passed}/{total} PASSED, {known_diff} KNOWN-DIFFERENCE, {failed} FAILED ==="
    );
    (passed, failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs_render_gate_suites_all_pass() {
        for suite in &GS_RENDER_GATE_SUITES {
            run_one_gs_suite("gs_render", suite)
                .unwrap_or_else(|e| panic!("{} failed: {e}", suite.name));
        }
    }

    #[test]
    fn pcsx2_observed_suites_all_pass() {
        for entry in &PCSX2_OBSERVED_SUITES {
            run_one_gs_suite("gs_probes", &entry.suite)
                .unwrap_or_else(|e| panic!("{} failed: {e}", entry.suite.name));
            if entry.known_difference_reason.is_some() {
                assert!(
                    !entry.known_diff_pixels.is_empty(),
                    "{} marked KNOWN-DIFFERENCE without listing differing pixels",
                    entry.suite.name
                );
            }
        }
    }
}
