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

pub static GS_RENDER_GATE_SUITES: [GsGateSuite; 3] = [
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
];

fn resolve_fixture_path(rel_path: &str) -> PathBuf {
    let cwd_path = Path::new("tests/gs_render").join(rel_path);
    if cwd_path.exists() {
        return cwd_path;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/gs_render")
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

/// Run one GS Render Gate suite and return `Ok(())` on pass or `Err(reason)` on mismatch.
pub fn run_one_gs_suite(suite: &GsGateSuite) -> Result<(), String> {
    let path = resolve_fixture_path(suite.rel_path);
    let src =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut gs = GsState::new();
    execute_gsp_script(&mut gs, &src)
        .map_err(|e| format!("parse error in {}: {e}", path.display()))?;
    (suite.verify)(&gs)
}

/// Execute all checked-in GS Render Gate suites and print a summary table. Returns `(passed, failures)`.
pub fn run_gs_render_gate() -> (usize, Vec<(&'static str, String)>) {
    println!("\n|  # | {:<36} | {:<8} |", "GS Render Suite", "Status");
    println!("|----|--------------------------------------|----------|");

    let mut passed = 0usize;
    let mut failures = Vec::new();
    for (idx, suite) in GS_RENDER_GATE_SUITES.iter().enumerate() {
        let status = match run_one_gs_suite(suite) {
            Ok(()) => {
                passed += 1;
                "PASS"
            }
            Err(err) => {
                failures.push((suite.name, err));
                "FAIL"
            }
        };
        println!("| {:>2} | {:<36} | {:<8} |", idx + 1, suite.name, status);
    }

    let total = GS_RENDER_GATE_SUITES.len();
    let failed = failures.len();
    println!("\n=== GS RENDER GATE: {passed}/{total} PASSED, {failed} FAILED ===");
    (passed, failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs_render_gate_suites_all_pass() {
        for suite in &GS_RENDER_GATE_SUITES {
            run_one_gs_suite(suite).unwrap_or_else(|e| panic!("{} failed: {e}", suite.name));
        }
    }
}
