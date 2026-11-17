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

pub static GS_RENDER_GATE_SUITES: [GsGateSuite; 9] = [
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
];

pub static PCSX2_OBSERVED_SUITES: [GsGateSuite; 1] = [GsGateSuite {
    name: "gs_probes/10_subpixel_probe",
    rel_path: "10_subpixel_probe.gsp",
    verify: verify_10_subpixel_probe,
}];

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
    let cwd_path = Path::new("tests").join(subdir).join(rel_path);
    if cwd_path.exists() {
        return cwd_path;
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
pub fn run_pcsx2_observed_gate() -> (usize, Vec<(&'static str, String)>) {
    run_gs_suite_table(
        "PCSX2-observed (v2.8.2), not hardware",
        "PCSX2-observed (v2.8.2), not hardware",
        "gs_probes",
        &PCSX2_OBSERVED_SUITES,
    )
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
        for suite in &PCSX2_OBSERVED_SUITES {
            run_one_gs_suite("gs_probes", suite)
                .unwrap_or_else(|e| panic!("{} failed: {e}", suite.name));
        }
    }
}
