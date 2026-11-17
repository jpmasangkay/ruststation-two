//! EE -> GIF -> GS ELF round-trip verification for GS Render Gate fixtures (`Phase 2b Step 2.5`).
//!
//! Emits a standalone R5900 ELF32 via `ps2_gs_runner::elf_emit` for each `.gsp` script, executes
//! it through `EeCpuState` + `Bus` + `GifUnit` (`PATH3 GIF_FIFO`) + `GsState` without syscalls,
//! and asserts 100% bit-for-bit agreement with direct `ps2_gs::execute_gsp_script` replay.

use std::fs;
use std::path::{Path, PathBuf};

use ps2_core::bus::Bus;
use ps2_core::ee::{EeCpuState, EeHleState, HleStepResult};
use ps2_core::load_elf32;
use ps2_gs::{execute_gsp_script, parse_gsp_script, GsState, GspCommand};
use ps2_gs_runner::elf_emit::{
    build_elf_from_gsp, DisplaySetupRegs, ELF_ENTRY_VADDR, R5900_SPIN_BEQ,
};
use ps2_gs_runner::{encode_framebuffer_png, infer_rect_from_state, DumpRect};

/// Frozen 9 Step-2 GS Render Gate fixture list (probe scripts stay outside until Step 4).
pub const FROZEN_GS_ELF_SUITES: [(&str, &str); 9] = [
    (
        "gs_render/01_host_to_local_psmct32",
        "01_host_to_local_psmct32.gsp",
    ),
    ("gs_render/02_swizzle_t8_t4_24", "02_swizzle_t8_t4_24.gsp"),
    (
        "gs_render/03_local_to_local_dir",
        "03_local_to_local_dir.gsp",
    ),
    ("gs_render/04_sprite_flat", "04_sprite_flat.gsp"),
    ("gs_render/05_triangle_flat", "05_triangle_flat.gsp"),
    ("gs_render/06_triangle_gouraud", "06_triangle_gouraud.gsp"),
    ("gs_render/07_ztest_overlap", "07_ztest_overlap.gsp"),
    ("gs_render/08_scissor_clip", "08_scissor_clip.gsp"),
    (
        "gs_render/09_fbmsk_psm16_24_adc",
        "09_fbmsk_psm16_24_adc.gsp",
    ),
];

const MAX_ELF_IDLE_CYCLES: u64 = 5_000_000;

fn gs_render_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/gs_render")
}

/// Boot `elf_bytes` on a fresh EE + Bus + GIF + GS machine until it reaches the stub spin loop
/// and the GIF `PATH3` FIFO & all active GS paths are completely drained.
pub fn run_gsp_elf_until_idle(elf_bytes: &[u8], max_cycles: u64) -> Result<(GsState, u64), String> {
    let mut cpu = EeCpuState::new();
    let mut bus = Bus::new();
    let mut hle = EeHleState::new();

    load_elf32(elf_bytes, &mut cpu, &mut bus).map_err(|e| format!("ELF load failed: {e}"))?;

    for cycles in 1..=max_cycles {
        match cpu.step_hle(&mut bus, &mut hle) {
            HleStepResult::Continue => {}
            HleStepResult::Exited(code) => {
                return Err(format!("unexpected syscall exit({code}) at cycle {cycles}"));
            }
            HleStepResult::UnimplementedSyscall { pc, syscall } => {
                return Err(format!(
                    "unexpected syscall {syscall} at pc={pc:#010X}, cycle={cycles}"
                ));
            }
            HleStepResult::UnimplementedInstruction {
                pc,
                instr,
                category,
            } => {
                return Err(format!(
                    "unimplemented {category:?} instr {instr:#010X} at pc={pc:#010X}, cycle={cycles}"
                ));
            }
            HleStepResult::UnimplementedDmaTransfer(info) => {
                return Err(format!(
                    "unexpected DMAC transfer {info:?} at cycle {cycles}"
                ));
            }
        }

        if let Some(mmio_err) = bus.pending_ee_mmio_error.take() {
            return Err(format!("EE MMIO error at cycle {cycles}: {mmio_err:?}"));
        }

        let at_spin = cpu.pc >= ELF_ENTRY_VADDR && bus.read32(cpu.pc) == R5900_SPIN_BEQ;
        let gif_drained = bus.gif.path3_fifo_len() == 0
            && !bus.gif.can_step(&bus.gs)
            && !bus.gif.is_any_path_busy();
        if at_spin && gif_drained {
            return Ok((bus.gs.core.clone(), cycles));
        }
    }

    Err(format!(
        "timed out after {max_cycles} EE cycles waiting for ELF spin + GIF drain (pc={:#010X}, fqc={}, active={})",
        cpu.pc,
        bus.gif.path3_fifo_len(),
        bus.gif.is_any_path_busy()
    ))
}

fn priv_displays_equal(lhs: &GsState, rhs: &GsState) -> Result<(), String> {
    let a = &lhs.regs.priv_display;
    let b = &rhs.regs.priv_display;
    if a.pmode != b.pmode {
        return Err(format!(
            "PMODE mismatch: direct={:#018X} elf={:#018X}",
            a.pmode.0, b.pmode.0
        ));
    }
    if a.smode1 != b.smode1 {
        return Err(format!(
            "SMODE1 mismatch: direct={:#018X} elf={:#018X}",
            a.smode1, b.smode1
        ));
    }
    if a.smode2 != b.smode2 {
        return Err(format!(
            "SMODE2 mismatch: direct={:#018X} elf={:#018X}",
            a.smode2.0, b.smode2.0
        ));
    }
    if a.dispfb1 != b.dispfb1 {
        return Err(format!(
            "DISPFB1 mismatch: direct={:#018X} elf={:#018X}",
            a.dispfb1.0, b.dispfb1.0
        ));
    }
    if a.display1 != b.display1 {
        return Err(format!(
            "DISPLAY1 mismatch: direct={:#018X} elf={:#018X}",
            a.display1.0, b.display1.0
        ));
    }
    if a.dispfb2 != b.dispfb2 {
        return Err(format!(
            "DISPFB2 mismatch: direct={:#018X} elf={:#018X}",
            a.dispfb2.0, b.dispfb2.0
        ));
    }
    if a.display2 != b.display2 {
        return Err(format!(
            "DISPLAY2 mismatch: direct={:#018X} elf={:#018X}",
            a.display2.0, b.display2.0
        ));
    }
    if a.bgcolor != b.bgcolor {
        return Err(format!(
            "BGCOLOR mismatch: direct={:#018X} elf={:#018X}",
            a.bgcolor.0, b.bgcolor.0
        ));
    }
    Ok(())
}

/// Emit an ELF for `gsp_path`, run it on `EE -> GIF -> GS`, and verify exact parity with direct `.gsp` replay.
pub fn verify_gsp_elf_roundtrip(name: &str, gsp_path: &Path) -> Result<u64, String> {
    let src = fs::read_to_string(gsp_path)
        .map_err(|e| format!("failed to read {}: {e}", gsp_path.display()))?;
    let cmds = parse_gsp_script(&src)
        .map_err(|e| format!("failed to parse {}: {e}", gsp_path.display()))?;

    let mut direct_gs = GsState::new();
    let hint = execute_gsp_script(&mut direct_gs, &src)
        .map_err(|e| format!("direct replay error in {name}: {e}"))?;
    let rect: DumpRect = hint
        .map(Into::into)
        .or_else(|| infer_rect_from_state(&direct_gs))
        .ok_or_else(|| format!("missing surface hint or FRAME_1/SCISSOR_1 in {name}"))?;

    // Compute exact expected privileged display registers: CRT setup defaults + any script `priv` overrides.
    let mut expected_display_gs = GsState::new();
    let surface_hint = rect.into();
    DisplaySetupRegs::from_surface(&surface_hint).apply_to_gs(&mut expected_display_gs);
    for &cmd in &cmds {
        if let GspCommand::PrivilegedReg { offset, data } = cmd {
            expected_display_gs.write_privileged_display(offset, data);
        }
    }

    let elf = build_elf_from_gsp(&cmds, &surface_hint)
        .map_err(|e| format!("ELF build failed for {name}: {e}"))?;
    let (elf_gs, cycles) = run_gsp_elf_until_idle(&elf.bytes, MAX_ELF_IDLE_CYCLES)?;

    priv_displays_equal(&expected_display_gs, &elf_gs)?;

    if direct_gs.vram != elf_gs.vram {
        for y in 0..rect.height {
            for x in 0..rect.width {
                let d = direct_gs
                    .vram
                    .read_pixel_raw(rect.format, rect.bp_blocks, rect.bw, x, y);
                let e = elf_gs
                    .vram
                    .read_pixel_raw(rect.format, rect.bp_blocks, rect.bw, x, y);
                if d != e {
                    return Err(format!(
                        "{name}: first VRAM diff at ({x},{y}): direct={d:#010X} elf={e:#010X}"
                    ));
                }
            }
        }
        return Err(format!(
            "{name}: 4 MB VRAM differs outside primary surface rectangle"
        ));
    }

    let mut direct_png = Vec::new();
    encode_framebuffer_png(&direct_gs, rect, true, 1, &mut direct_png, "direct")
        .map_err(|e| format!("PNG encode direct {name}: {e}"))?;
    let mut elf_png = Vec::new();
    encode_framebuffer_png(&elf_gs, rect, true, 1, &mut elf_png, "elf")
        .map_err(|e| format!("PNG encode elf {name}: {e}"))?;
    if direct_png != elf_png {
        return Err(format!(
            "{name}: encoded RGBA PNG bytes differ between direct and ELF replay"
        ));
    }

    Ok(cycles)
}

/// Run EE -> GIF -> GS ELF self-check across all 9 frozen Step 2 GS gate suites.
pub fn run_gs_elf_gate() -> (usize, Vec<(&'static str, String)>) {
    let base_dir = gs_render_dir();
    let total = FROZEN_GS_ELF_SUITES.len();
    let mut passed = 0usize;
    let mut failures = Vec::new();

    println!("\n|  # | GS ELF Self-Check (EE->GIF->GS)    | Status   |    Cycles |");
    println!("|---:|:-----------------------------------|:---------|----------:|");
    for (idx, &(name, rel_path)) in FROZEN_GS_ELF_SUITES.iter().enumerate() {
        let path = base_dir.join(rel_path);
        match verify_gsp_elf_roundtrip(name, &path) {
            Ok(cycles) => {
                passed += 1;
                println!("| {:>2} | {name:<34} | PASS     | {:>9} |", idx + 1, cycles);
            }
            Err(reason) => {
                println!("| {:>2} | {name:<34} | FAIL     |         - |", idx + 1);
                failures.push((name, reason));
            }
        }
    }
    println!(
        "\n=== GS ELF SELF-CHECK GATE: {passed}/{total} PASSED, {} FAILED ===",
        failures.len()
    );
    (passed, failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs_render_elf_roundtrip_matches_direct_replay() {
        let base_dir = gs_render_dir();
        for &(name, rel_path) in &FROZEN_GS_ELF_SUITES {
            let path = base_dir.join(rel_path);
            let cycles = verify_gsp_elf_roundtrip(name, &path)
                .unwrap_or_else(|e| panic!("ELF self-check failed for {name}: {e}"));
            assert!(cycles > 0, "{name} executed zero cycles");
        }
    }

    #[test]
    fn subpixel_probe_elf_roundtrip_matches_direct_replay() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/gs_probes/10_subpixel_probe.gsp");
        let cycles = verify_gsp_elf_roundtrip("gs_probes/10_subpixel_probe", &path)
            .expect("10_subpixel_probe ELF round-trip");
        assert!(cycles > 0);

        let src = fs::read_to_string(&path).expect("read probe");
        let mut gs = GsState::new();
        execute_gsp_script(&mut gs, &src).expect("execute probe");
        let px = |x, y| {
            gs.vram
                .read_pixel_raw(ps2_gs::PixelFormat::Psmct32, 0, 5, x, y)
        };
        // Point snaps (P0..P6) under IntegerCenter half-up:
        assert_eq!(px(20, 20), 0x8040_40FF, "P0 (20.0, 20.0)");
        assert_eq!(px(36, 20), 0x8040_FF40, "P1 (36.4375, 20.0)");
        assert_eq!(px(53, 20), 0x80FF_8040, "P2 (52.5, 20.0)");
        assert_eq!(px(69, 20), 0x8040_FFFF, "P3 (68.5625, 20.0)");
        assert_eq!(px(36, 40), 0x80FF_FF40, "P4 (36.0, 40.4375)");
        assert_eq!(px(52, 41), 0x80FF_40FF, "P5 (52.0, 40.5)");
        assert_eq!(px(68, 41), 0x8040_A0FF, "P6 (68.0, 40.5625)");
        // Line endpoints, half-step diagonal (L3), zero-length (L5):
        assert_eq!(px(28, 80), 0x80FF_FFFF, "L0 final endpoint (28, 80)");
        assert_eq!(
            px(50, 80),
            0x80FF_FF80,
            "L1 reversed final endpoint (50, 80)"
        );
        assert_eq!(
            px(90, 84),
            0x8080_FFFF,
            "L2 vertical final endpoint (90, 84)"
        );
        assert_eq!(px(21, 111), 0x8080_FF80, "L3 slope-0.5 tie-break (21, 111)");
        assert_eq!(px(28, 114), 0x8080_FF80, "L3 final endpoint (28, 114)");
        assert_eq!(px(54, 110), 0x80C0_80FF, "L4 final endpoint (54, 110)");
        assert_eq!(px(80, 110), 0x8040_C0FF, "L5 zero-length line (80, 110)");
        // Sprites: normal (S0), reversed corners (S1), subpixel [80.5, 84.5) (S2):
        assert_eq!(px(20, 150), 0x80D0_E040, "S0 normal top-left");
        assert_eq!(px(50, 150), 0x8050_70FF, "S1 reversed-corner top-left");
        assert_eq!(px(80, 150), 0x8000_0000, "S2 subpixel excluded x=80");
        assert_eq!(px(81, 150), 0x80FF_A0C0, "S2 subpixel included x=81");
        assert_eq!(px(84, 153), 0x80FF_A0C0, "S2 subpixel included x=84");
    }

    #[test]
    fn gouraud_and_z_probes_elf_roundtrip_matches_direct_replay() {
        for (name, rel) in [
            (
                "gs_probes/11_gouraud_wide_irrational",
                "../../tests/gs_probes/11_gouraud_wide_irrational.gsp",
            ),
            (
                "gs_probes/12_gouraud_slanted_all_channels",
                "../../tests/gs_probes/12_gouraud_slanted_all_channels.gsp",
            ),
            (
                "gs_probes/13_gouraud_z_probe",
                "../../tests/gs_probes/13_gouraud_z_probe.gsp",
            ),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
            let cycles = verify_gsp_elf_roundtrip(name, &path)
                .unwrap_or_else(|e| panic!("ELF self-check failed for {name}: {e}"));
            assert!(cycles > 0, "{name} executed zero cycles");
        }
    }
}
