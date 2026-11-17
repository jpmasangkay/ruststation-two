//! Headless CLI runner for `ps2autotests` `.elf` test suites.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use ps2_core::bus::{
    Bus, UnimplementedDmaTransfer, UnimplementedEeRegister, UnimplementedIopRegister,
};

mod gs_elf_gate;
mod gs_gate;
use ps2_core::ee::{EeCpuState, EeHleState, HleStepResult};
use ps2_core::iop::{
    is_irx_binary, load_irx, IopCpuState, IopStepResult, IrxError, LoadedIrx,
    UnimplementedIopImport,
};
use ps2_core::{load_elf32, ElfError, LoadedElf};
use similar::TextDiff;

#[derive(Debug, Parser)]
#[command(name = "ps2-autotest", about = "Headless PS2 autotest runner")]
struct Args {
    /// Positional path to a `.elf` test binary.
    #[arg(value_name = "ELF")]
    elf: Option<PathBuf>,

    /// Path to a single `.elf` or `.irx` test binary (alias for positional `ELF`).
    #[arg(long)]
    test: Option<PathBuf>,

    /// Run all 52 hardware gate suites (35 Phase 1 + 17 Phase 2a), print the summary table, and exit nonzero on any non-deferred failure.
    #[arg(long)]
    gate: bool,

    /// Run the 52 hardware autotest suites in headless benchmark mode (no GS repo fixtures required) and print throughput summary.
    #[arg(long)]
    bench: bool,

    /// Number of timed iterations for `--bench` (minimum 1).
    #[arg(long, default_value_t = 3)]
    bench_runs: usize,

    /// Boot the PS2 BIOS image from reset (`0xBFC0_0000`) in coupled `(EE + IOP)` LLE mode until the first blocker.
    #[arg(long)]
    boot_bios: bool,

    /// Path to the `ps2autotests` repository root or its `tests/` directory.
    #[arg(
        long = "tests-dir",
        visible_alias = "suite-dir",
        default_value = "local/ps2autotests/tests"
    )]
    tests_dir: PathBuf,

    /// Optional path to a 4 MB PS2 BIOS dump directory or file.
    #[arg(long, default_value = "local/bios/scph10000.bin")]
    bios: PathBuf,

    /// Maximum EE cycles to execute per test before timing out.
    #[arg(long, default_value_t = 50_000_000)]
    max_cycles: u64,
}

/// Termination reason for an ELF test run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStopReason {
    /// Program exited cleanly via HLE syscall `Exit` / `ExitThread` / `ExitDeleteThread`.
    Exited(i32),
    /// Hit `--max-cycles` limit before exiting.
    MaxCyclesReached(u64),
    /// Hit an unimplemented EE kernel syscall (`$v1`).
    UnimplementedSyscall { pc: u32, syscall: i16 },
    /// Hit an instruction from a deferred ISA extension (`MMI`, `COP1`, `COP2`).
    UnimplementedInstruction {
        pc: u32,
        instr: u32,
        category: &'static str,
    },
    /// Attempted a DMA transfer on an unimplemented peripheral channel (`0..=7`).
    UnimplementedDmaTransfer(UnimplementedDmaTransfer),
    /// Hit an unimplemented IOP `.irx` module import stub.
    UnimplementedIopImport(UnimplementedIopImport),
    /// Accessed an unimplemented EE hardware register.
    UnimplementedEeRegister(UnimplementedEeRegister),
    /// Accessed an unimplemented IOP hardware register.
    UnimplementedIopRegister(UnimplementedIopRegister),
}

impl std::fmt::Display for RunStopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "Exited({code})"),
            Self::MaxCyclesReached(max) => write!(f, "MaxCyclesReached({max})"),
            Self::UnimplementedSyscall { pc, syscall } => write!(
                f,
                "UnimplementedSyscall(pc=0x{pc:08X}, syscall={syscall} / 0x{:02X})",
                syscall.unsigned_abs()
            ),
            Self::UnimplementedInstruction {
                pc,
                instr,
                category,
            } => write!(
                f,
                "UnimplementedInstruction(pc=0x{pc:08X}, instr=0x{instr:08X}, category={category})"
            ),
            Self::UnimplementedDmaTransfer(info) => write!(
                f,
                "UnimplementedDmaTransfer(ch={} [{}], chcr=0x{:08X}, madr=0x{:08X}, qwc=0x{:04X}, tadr=0x{:08X})",
                info.channel, info.channel_name, info.chcr, info.madr, info.qwc, info.tadr
            ),
            Self::UnimplementedIopImport(imp) => write!(f, "{imp}"),
            Self::UnimplementedEeRegister(reg) => write!(
                f,
                "UnimplementedEeRegister(paddr=0x{:08X}, width={}, is_write={}, value=0x{:08X})",
                reg.paddr, reg.width, reg.is_write, reg.value
            ),
            Self::UnimplementedIopRegister(reg) => write!(
                f,
                "UnimplementedIopRegister(paddr=0x{:08X}, width={}, is_write={}, value=0x{:08X})",
                reg.paddr, reg.width, reg.is_write, reg.value
            ),
        }
    }
}

/// Complete result of executing an ELF32 test binary in `ps2-autotest`.
#[derive(Clone, Debug)]
pub struct ElfRunReport {
    /// Loaded ELF metadata (`entry_pc`, segment count, total bytes).
    pub loaded: LoadedElf,
    /// Total EE cycles (instructions) executed.
    pub cycles: u64,
    /// Why execution stopped.
    pub stop_reason: RunStopReason,
    /// Captured EE stdout (normalized to `\n` line endings).
    pub stdout: String,
}

/// Complete result of executing an IOP `.irx` test binary in `ps2-autotest`.
#[derive(Clone, Debug)]
pub struct IrxRunReport {
    /// Loaded IRX metadata (`base_addr`, `entry_pc`, `gp`, `relocations_applied`).
    pub loaded: LoadedIrx,
    /// Total IOP cycles executed (`1` IOP cycle = `8` EE cycles).
    pub iop_cycles: u64,
    /// Total EE cycles elapsed on the central `Scheduler` (`iop_cycles * 8`).
    pub ee_cycles: u64,
    /// Why execution stopped.
    pub stop_reason: RunStopReason,
    /// Captured IOP stdout (normalized to `\n` line endings).
    pub stdout: String,
}

/// Load and execute an IOP `.irx` binary image headlessly for up to `max_ee_cycles` EE cycles
/// (`1` IOP cycle = `8` EE cycles on the central `Scheduler`).
pub fn run_irx_bytes(irx_bytes: &[u8], max_ee_cycles: u64) -> Result<IrxRunReport, IrxError> {
    let mut iop = IopCpuState::new();
    let mut bus = Bus::new();

    let loaded = load_irx(irx_bytes, &mut iop, &mut bus)?;
    let max_iop_cycles = (max_ee_cycles / 8).max(1);
    let mut iop_cycles = 0u64;
    let stop_reason = loop {
        if iop_cycles >= max_iop_cycles {
            break RunStopReason::MaxCyclesReached(bus.scheduler.current_ee_cycle);
        }
        let step_res = iop.step_hle(&mut bus);
        if matches!(step_res, IopStepResult::Exited(_)) {
            if let IopStepResult::Exited(code) = step_res {
                break RunStopReason::Exited(code);
            }
        }
        iop_cycles += 1;
        match step_res {
            IopStepResult::Continue => {}
            IopStepResult::Exited(code) => break RunStopReason::Exited(code),
            IopStepResult::UnimplementedImport(imp) => {
                break RunStopReason::UnimplementedIopImport(imp);
            }
            IopStepResult::UnimplementedRegister(reg) => {
                break RunStopReason::UnimplementedIopRegister(reg);
            }
            IopStepResult::UnimplementedInstruction { pc, instr } => {
                break RunStopReason::UnimplementedInstruction {
                    pc,
                    instr,
                    category: "IOP_R3000A",
                };
            }
        }
    };

    let raw_stdout = String::from_utf8_lossy(&bus.stdout_buffer);
    let stdout = normalize_newlines(&raw_stdout);

    Ok(IrxRunReport {
        loaded,
        iop_cycles,
        ee_cycles: bus.scheduler.current_ee_cycle,
        stop_reason,
        stdout,
    })
}

/// Load and execute an ELF32 binary image headlessly for up to `max_cycles` EE cycles.
pub fn run_elf_bytes(elf_bytes: &[u8], max_cycles: u64) -> Result<ElfRunReport, ElfError> {
    let mut cpu = EeCpuState::new();
    let mut bus = Bus::new();
    let mut hle = EeHleState::new();

    let loaded = load_elf32(elf_bytes, &mut cpu, &mut bus)?;
    let mut cycles = 0u64;
    let stop_reason = loop {
        if cycles >= max_cycles {
            break RunStopReason::MaxCyclesReached(cycles);
        }
        let step_res = cpu.step_hle(&mut bus, &mut hle);
        cycles += 1;
        match step_res {
            HleStepResult::Continue => {}
            HleStepResult::Exited(code) => break RunStopReason::Exited(code),
            HleStepResult::UnimplementedSyscall { pc, syscall } => {
                break RunStopReason::UnimplementedSyscall { pc, syscall };
            }
            HleStepResult::UnimplementedInstruction {
                pc,
                instr,
                category,
            } => {
                break RunStopReason::UnimplementedInstruction {
                    pc,
                    instr,
                    category,
                };
            }
            HleStepResult::UnimplementedDmaTransfer(info) => {
                break RunStopReason::UnimplementedDmaTransfer(info);
            }
        }
    };

    let raw_stdout = String::from_utf8_lossy(&bus.stdout_buffer);
    let stdout = normalize_newlines(&raw_stdout);

    Ok(ElfRunReport {
        loaded,
        cycles,
        stop_reason,
        stdout,
    })
}

/// Normalize CRLF (`\r\n`) to LF (`\n`) for cross-platform `.expected` comparison.
pub fn normalize_newlines(input: &str) -> String {
    input.replace("\r\n", "\n")
}

/// Format a unified diff between `expected` and `actual`.
pub fn format_expected_diff(expected: &str, actual: &str) -> String {
    let diff = TextDiff::from_lines(expected, actual);
    diff.unified_diff()
        .context_radius(3)
        .header("expected (.expected)", "actual (EE stdout)")
        .to_string()
}

fn run_single_elf_file(elf_path: &Path, max_cycles: u64) -> ExitCode {
    let elf_bytes = match fs::read(elf_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("ERROR: failed to read ELF {}: {err}", elf_path.display());
            return ExitCode::FAILURE;
        }
    };

    let (stdout, stop_reason, cycles, entry_pc, label) = if is_irx_binary(&elf_bytes) {
        let report = match run_irx_bytes(&elf_bytes, max_cycles) {
            Ok(rep) => rep,
            Err(err) => {
                eprintln!("ERROR: failed to load IRX {}: {err}", elf_path.display());
                return ExitCode::FAILURE;
            }
        };
        (
            report.stdout,
            report.stop_reason,
            report.iop_cycles,
            report.loaded.entry_pc,
            "IOP",
        )
    } else {
        let report = match run_elf_bytes(&elf_bytes, max_cycles) {
            Ok(rep) => rep,
            Err(err) => {
                eprintln!("ERROR: failed to load ELF {}: {err}", elf_path.display());
                return ExitCode::FAILURE;
            }
        };
        (
            report.stdout,
            report.stop_reason,
            report.cycles,
            report.loaded.entry_pc,
            "EE",
        )
    };

    println!("=== {label} STDOUT ({}) ===", elf_path.display());
    print!("{stdout}");
    if !stdout.ends_with('\n') && !stdout.is_empty() {
        println!();
    }
    println!(
        "=== STOP: {} after {} cycles (entry=0x{:08X}) ===",
        stop_reason, cycles, entry_pc
    );

    let expected_path = elf_path.with_extension("expected");
    if expected_path.exists() {
        match fs::read_to_string(&expected_path) {
            Ok(raw_expected) => {
                let expected = normalize_newlines(&raw_expected);
                if stdout == expected {
                    println!("RESULT: PASS ({})", elf_path.display());
                    ExitCode::SUCCESS
                } else {
                    println!("RESULT: FAIL ({})", elf_path.display());
                    print!("{}", format_expected_diff(&expected, &stdout));
                    ExitCode::FAILURE
                }
            }
            Err(err) => {
                eprintln!(
                    "ERROR: failed to read expected file {}: {err}",
                    expected_path.display()
                );
                ExitCode::FAILURE
            }
        }
    } else {
        ExitCode::SUCCESS
    }
}

/// Canonical list of all 35 suites in the Phase 1 Hardware Gate (`23` EE + `5` DMAC/SPR + `7` `cpu/iop`).
pub const PHASE1_HW_GATE_SUITES: [&str; 35] = [
    // 23 EE CPU, COP0, COP1 FPU, MMI SIMD, and Memory suites
    "cpu/ee/alu.elf",
    "cpu/ee/branch.elf",
    "cpu/ee/branchdelay.elf",
    "cpu/ee/lsu.elf",
    "cpu/ee/lsudelay.elf",
    "cpu/ee/muldiv.elf",
    "cpu/ee_cop0/performance.elf",
    "cpu/ee_fpu/arithmetic.elf",
    "cpu/ee_fpu/branch.elf",
    "cpu/ee_fpu/branchdelay.elf",
    "cpu/ee_fpu/compare.elf",
    "cpu/ee_fpu/convert.elf",
    "cpu/ee_fpu/fcr.elf",
    "cpu/ee_fpu/muldiv.elf",
    "cpu/ee_fpu/sqrt.elf",
    "cpu/ee_fpu/transfer.elf",
    "cpu/ee_simd/arithmetic.elf",
    "cpu/ee_simd/compare.elf",
    "cpu/ee_simd/funnel.elf",
    "cpu/ee_simd/logic.elf",
    "cpu/ee_simd/muldiv.elf",
    "cpu/ee_simd/shuffle.elf",
    "memory/ee/default.elf",
    // 5 EE DMAC & Scratchpad DMA suites
    "dma/dmac/cpcond.elf",
    "dma/dmac/tagintr.elf",
    "dma/spr/chain.elf",
    "dma/spr/interleave.elf",
    "dma/spr/normal.elf",
    // 7 R3000A IOP CPU suites
    "cpu/iop/alu.irx",
    "cpu/iop/branch.irx",
    "cpu/iop/branchdelay.irx",
    "cpu/iop/hilodelay.irx",
    "cpu/iop/lsu.irx",
    "cpu/iop/lsudelay.irx",
    "cpu/iop/muldiv.irx",
];

/// Canonical list of all 17 suites in the Phase 2a Hardware Gate (`2` `cpu/vu0_macro` + `7` `vu/` + `6` `dma/vif` + `2` `gs/`).
pub const PHASE2A_HW_GATE_SUITES: [&str; 17] = [
    "cpu/vu0_macro/integer.elf",
    "cpu/vu0_macro/transfer.elf",
    "vu/games/triace.elf",
    "vu/lower/branch.elf",
    "vu/lower/efu.elf",
    "vu/lower/fdivdelay.elf",
    "vu/lower/integer.elf",
    "vu/lower/random.elf",
    "vu/upper/clip.elf",
    "dma/vif/basic.elf",
    "dma/vif/intstall.elf",
    "dma/vif/mskpath3.elf",
    "dma/vif/stcycl.elf",
    "dma/vif/stmod.elf",
    "dma/vif/unpack.elf",
    "gs/label.elf",
    "gs/signal.elf",
];

/// Checked-in list of gate suites explicitly deferred from blocking CI / gate exit code.
pub const DEFERRED_GATE_SUITES: [(&str, &str); 1] = [(
    "vu/lower/efu.elf",
    "partial: 184/208 vectors; see HARDWARE_NOTES.md EFU section",
)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateOutcome {
    Pass,
    Deferred(&'static str),
    Fail,
}

fn deferred_gate_reason(rel_path: &str) -> Option<&'static str> {
    DEFERRED_GATE_SUITES
        .iter()
        .find(|&&(suite, _)| suite == rel_path)
        .map(|&(_, reason)| reason)
}

fn evaluate_gate_outcome(rel_path: &str, is_pass: bool) -> GateOutcome {
    if is_pass {
        GateOutcome::Pass
    } else if let Some(reason) = deferred_gate_reason(rel_path) {
        GateOutcome::Deferred(reason)
    } else {
        GateOutcome::Fail
    }
}

/// Accept either `<repo>/tests` directly or a cloned `ps2autotests` repository root containing `tests/cpu`.
fn resolve_tests_dir(path: &Path) -> PathBuf {
    let nested = path.join("tests");
    if !path.join("cpu").exists() && nested.join("cpu").is_dir() {
        nested
    } else {
        path.to_path_buf()
    }
}

struct SuiteExecSummary {
    target_label: &'static str,
    cycles: u64,
    ee_cycles: u64,
    iop_cycles: u64,
    outcome: GateOutcome,
    stop_display: String,
    failure_diff: Option<String>,
}

fn execute_suite_bytes(
    rel_path: &str,
    bin_bytes: &[u8],
    expected: &str,
    max_cycles: u64,
) -> SuiteExecSummary {
    let (stdout, stop_reason, cycles, ee_cycles, iop_cycles, target_label) =
        if is_irx_binary(bin_bytes) {
            match run_irx_bytes(bin_bytes, max_cycles) {
                Ok(rep) => (
                    rep.stdout,
                    rep.stop_reason,
                    rep.iop_cycles,
                    0,
                    rep.iop_cycles,
                    "IOP",
                ),
                Err(err) => {
                    return SuiteExecSummary {
                        target_label: "IOP",
                        cycles: 0,
                        ee_cycles: 0,
                        iop_cycles: 0,
                        outcome: GateOutcome::Fail,
                        stop_display: format!("LoadError({err})"),
                        failure_diff: Some(format!("IRX load error: {err}\n")),
                    };
                }
            }
        } else {
            match run_elf_bytes(bin_bytes, max_cycles) {
                Ok(rep) => (rep.stdout, rep.stop_reason, rep.cycles, rep.cycles, 0, "EE"),
                Err(err) => {
                    return SuiteExecSummary {
                        target_label: "EE",
                        cycles: 0,
                        ee_cycles: 0,
                        iop_cycles: 0,
                        outcome: GateOutcome::Fail,
                        stop_display: format!("LoadError({err})"),
                        failure_diff: Some(format!("ELF load error: {err}\n")),
                    };
                }
            }
        };

    match evaluate_gate_outcome(rel_path, stdout == expected) {
        GateOutcome::Pass => SuiteExecSummary {
            target_label,
            cycles,
            ee_cycles,
            iop_cycles,
            outcome: GateOutcome::Pass,
            stop_display: stop_reason.to_string(),
            failure_diff: None,
        },
        GateOutcome::Deferred(reason) => SuiteExecSummary {
            target_label,
            cycles,
            ee_cycles,
            iop_cycles,
            outcome: GateOutcome::Deferred(reason),
            stop_display: format!("{stop_reason} ({reason})"),
            failure_diff: None,
        },
        GateOutcome::Fail => SuiteExecSummary {
            target_label,
            cycles,
            ee_cycles,
            iop_cycles,
            outcome: GateOutcome::Fail,
            stop_display: stop_reason.to_string(),
            failure_diff: Some(format_expected_diff(expected, &stdout)),
        },
    }
}

fn run_hardware_gate(suite_dir: &Path, max_cycles: u64) -> ExitCode {
    let start_time = Instant::now();
    let mut passed_count = 0usize;
    let mut deferred_count = 0usize;
    let total_count = PHASE1_HW_GATE_SUITES.len() + PHASE2A_HW_GATE_SUITES.len();
    let mut failure_diffs: Vec<(String, String)> = Vec::new();

    println!(
        "| {:>2} | {:<32} | {:<6} | {:<8} | {:>12} | {:<18} |",
        "#", "Suite", "Target", "Status", "Cycles", "Stop Reason"
    );
    println!(
        "|---:|:---------------------------------|:-------|:---------|-------------:|:-------------------|"
    );

    let all_suites = PHASE1_HW_GATE_SUITES
        .iter()
        .chain(PHASE2A_HW_GATE_SUITES.iter());

    for (idx, rel_path) in all_suites.enumerate() {
        let full_path = suite_dir.join(rel_path);
        let expected_path = full_path.with_extension("expected");

        let bin_bytes = match fs::read(&full_path) {
            Ok(b) => b,
            Err(err) => {
                println!(
                    "| {:>2} | {:<32} | {:<6} | {:<8} | {:>12} | {:<18} |",
                    idx + 1,
                    rel_path,
                    "N/A",
                    "FAIL",
                    0,
                    format!("ReadError({err})")
                );
                failure_diffs.push((
                    (*rel_path).to_owned(),
                    format!("Failed to read {}: {err}\n", full_path.display()),
                ));
                continue;
            }
        };

        let expected_raw = match fs::read_to_string(&expected_path) {
            Ok(s) => s,
            Err(err) => {
                println!(
                    "| {:>2} | {:<32} | {:<6} | {:<8} | {:>12} | {:<18} |",
                    idx + 1,
                    rel_path,
                    "N/A",
                    "FAIL",
                    0,
                    format!("ExpectedError({err})")
                );
                failure_diffs.push((
                    (*rel_path).to_owned(),
                    format!("Failed to read {}: {err}\n", expected_path.display()),
                ));
                continue;
            }
        };
        let expected = normalize_newlines(&expected_raw);

        let summary = execute_suite_bytes(rel_path, &bin_bytes, &expected, max_cycles);
        let status_str = match summary.outcome {
            GateOutcome::Pass => {
                passed_count += 1;
                "PASS"
            }
            GateOutcome::Deferred(_) => {
                deferred_count += 1;
                "DEFERRED"
            }
            GateOutcome::Fail => {
                if let Some(diff) = summary.failure_diff {
                    failure_diffs.push(((*rel_path).to_owned(), diff));
                }
                "FAIL"
            }
        };

        println!(
            "| {:>2} | {:<32} | {:<6} | {:<8} | {:>12} | {:<18} |",
            idx + 1,
            rel_path,
            summary.target_label,
            status_str,
            summary.cycles,
            summary.stop_display
        );
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    let failed_count = failure_diffs.len();
    println!(
        "\n=== HARDWARE GATE: {passed_count}/{total_count} PASSED, {deferred_count} DEFERRED, {failed_count} FAILED in {elapsed:.3} s ==="
    );

    let (gs_passed, gs_failures) = gs_gate::run_gs_render_gate();
    let _ = gs_passed;
    let (_pcsx2_passed, pcsx2_failures) = gs_gate::run_pcsx2_observed_gate();
    let (_elf_passed, elf_failures) = gs_elf_gate::run_gs_elf_gate();

    if !failure_diffs.is_empty()
        || !gs_failures.is_empty()
        || !pcsx2_failures.is_empty()
        || !elf_failures.is_empty()
    {
        for (suite, diff) in &failure_diffs {
            eprintln!("\n--- FAILURE DIFF: {suite} ---\n{diff}");
        }
        for (suite, reason) in &gs_failures {
            eprintln!("\n--- GS RENDER GATE FAILURE: {suite} ---\n{reason}");
        }
        for (suite, reason) in &pcsx2_failures {
            eprintln!("\n--- PCSX2-OBSERVED GATE FAILURE: {suite} ---\n{reason}");
        }
        for (suite, reason) in &elf_failures {
            eprintln!("\n--- GS ELF GATE FAILURE: {suite} ---\n{reason}");
        }
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

struct LoadedGateSuite {
    rel_path: &'static str,
    bin_bytes: Vec<u8>,
    expected: String,
}

fn load_gate_suite_bucket(
    tests_dir: &Path,
    suites: &[&'static str],
) -> Result<Vec<LoadedGateSuite>, String> {
    let mut loaded = Vec::with_capacity(suites.len());
    for &rel_path in suites {
        let full_path = tests_dir.join(rel_path);
        let expected_path = full_path.with_extension("expected");
        let bin_bytes = fs::read(&full_path)
            .map_err(|err| format!("failed to read {}: {err}", full_path.display()))?;
        let expected_raw = fs::read_to_string(&expected_path)
            .map_err(|err| format!("failed to read {}: {err}", expected_path.display()))?;
        loaded.push(LoadedGateSuite {
            rel_path,
            bin_bytes,
            expected: normalize_newlines(&expected_raw),
        });
    }
    Ok(loaded)
}

#[derive(Default)]
struct BucketRunTotals {
    cycles: u64,
    ee_cycles: u64,
    iop_cycles: u64,
    passed: usize,
    deferred: usize,
    failures: Vec<(String, String)>,
}

fn run_loaded_bucket(suites: &[LoadedGateSuite], max_cycles: u64) -> BucketRunTotals {
    let mut totals = BucketRunTotals::default();
    for suite in suites {
        let summary = execute_suite_bytes(
            suite.rel_path,
            &suite.bin_bytes,
            &suite.expected,
            max_cycles,
        );
        totals.cycles += summary.cycles;
        totals.ee_cycles += summary.ee_cycles;
        totals.iop_cycles += summary.iop_cycles;
        match summary.outcome {
            GateOutcome::Pass => totals.passed += 1,
            GateOutcome::Deferred(_) => totals.deferred += 1,
            GateOutcome::Fail => {
                let detail = summary
                    .failure_diff
                    .unwrap_or_else(|| summary.stop_display.clone());
                totals.failures.push((suite.rel_path.to_owned(), detail));
            }
        }
    }
    totals
}

/// Return `(best_min, median, worst_max)` elapsed seconds over non-empty positive duration samples.
fn summarize_secs(samples: &[f64]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
    };
    (sorted[0], median, sorted[n - 1])
}

fn mcycles_per_sec(cycles: u64, secs: f64) -> f64 {
    if secs > 0.0 {
        (cycles as f64) / 1_000_000.0 / secs
    } else {
        0.0
    }
}

fn format_commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn run_bench(tests_dir: &Path, max_cycles: u64, bench_runs: usize) -> ExitCode {
    if !tests_dir.is_dir() {
        eprintln!(
            "ERROR: tests directory {} does not exist; pass --tests-dir <path> (ps2autotests repo root or its tests/ subdirectory)",
            tests_dir.display()
        );
        return ExitCode::FAILURE;
    }

    let phase1 = match load_gate_suite_bucket(tests_dir, &PHASE1_HW_GATE_SUITES) {
        Ok(suites) => suites,
        Err(err) => {
            eprintln!("ERROR: {err}");
            return ExitCode::FAILURE;
        }
    };
    let phase2a = match load_gate_suite_bucket(tests_dir, &PHASE2A_HW_GATE_SUITES) {
        Ok(suites) => suites,
        Err(err) => {
            eprintln!("ERROR: {err}");
            return ExitCode::FAILURE;
        }
    };

    let runs = bench_runs.max(1);
    let total_suites = phase1.len() + phase2a.len();
    let mut p1_times = Vec::with_capacity(runs);
    let mut p2_times = Vec::with_capacity(runs);
    let mut total_times = Vec::with_capacity(runs);
    let mut run_records = Vec::with_capacity(runs);

    let mut total_ee_cycles = 0u64;
    let mut total_iop_cycles = 0u64;
    let mut p1_cycles = 0u64;
    let mut p2_cycles = 0u64;
    let mut passed_count = 0usize;
    let mut deferred_count = 0usize;
    let mut failed_count = 0usize;

    for run_idx in 0..runs {
        let p1_start = Instant::now();
        let p1_tot = run_loaded_bucket(&phase1, max_cycles);
        let p1_secs = p1_start.elapsed().as_secs_f64();

        let p2_start = Instant::now();
        let p2_tot = run_loaded_bucket(&phase2a, max_cycles);
        let p2_secs = p2_start.elapsed().as_secs_f64();

        let run_secs = p1_secs + p2_secs;
        let run_cycles = p1_tot.cycles + p2_tot.cycles;

        if run_idx == 0 {
            total_ee_cycles = p1_tot.ee_cycles + p2_tot.ee_cycles;
            total_iop_cycles = p1_tot.iop_cycles + p2_tot.iop_cycles;
            p1_cycles = p1_tot.cycles;
            p2_cycles = p2_tot.cycles;
            passed_count = p1_tot.passed + p2_tot.passed;
            deferred_count = p1_tot.deferred + p2_tot.deferred;
            failed_count = p1_tot.failures.len() + p2_tot.failures.len();
        }

        if !p1_tot.failures.is_empty() || !p2_tot.failures.is_empty() {
            for (suite, diff) in p1_tot.failures.iter().chain(p2_tot.failures.iter()) {
                eprintln!(
                    "\n--- BENCH FAILURE ({suite}, run {}) ---\n{diff}",
                    run_idx + 1
                );
            }
            return ExitCode::FAILURE;
        }

        p1_times.push(p1_secs);
        p2_times.push(p2_secs);
        total_times.push(run_secs);
        run_records.push((run_idx + 1, run_secs, run_cycles, p1_secs, p2_secs));
    }

    let total_cycles = total_ee_cycles + total_iop_cycles;
    let (best_s, med_s, worst_s) = summarize_secs(&total_times);
    let (p1_best_s, p1_med_s, _) = summarize_secs(&p1_times);
    let (p2_best_s, p2_med_s, _) = summarize_secs(&p2_times);

    println!("=== PS2-AUTOTEST BENCHMARK (Phase 1 + Phase 2a, headless interpreter) ===");
    println!("Tests dir   : {}", tests_dir.display());
    println!(
        "Suites      : {} ({} Phase 1 + {} Phase 2a) | Measured runs: {}",
        total_suites,
        phase1.len(),
        phase2a.len(),
        runs
    );
    println!(
        "Cycles/run  : {} EE + {} IOP = {} total",
        format_commas(total_ee_cycles),
        format_commas(total_iop_cycles),
        format_commas(total_cycles)
    );
    println!(
        "  Phase 1   : {} cycles ({} suites)",
        format_commas(p1_cycles),
        phase1.len()
    );
    println!(
        "  Phase 2a  : {} cycles ({} suites)",
        format_commas(p2_cycles),
        phase2a.len()
    );
    for (num, run_s, run_cycles, p1_s, p2_s) in run_records {
        println!(
            "  Run {:>2}    : {:.3} s ({:.2} Mcycles/s) [Phase 1: {:.3} s, Phase 2a: {:.3} s]",
            num,
            run_s,
            mcycles_per_sec(run_cycles, run_s),
            p1_s,
            p2_s
        );
    }
    println!(
        "Summary     : Median {:.3} s ({:.2} Mcycles/s) | Best {:.3} s ({:.2} Mcycles/s) | Worst {:.3} s ({:.2} Mcycles/s)",
        med_s,
        mcycles_per_sec(total_cycles, med_s),
        best_s,
        mcycles_per_sec(total_cycles, best_s),
        worst_s,
        mcycles_per_sec(total_cycles, worst_s)
    );
    println!(
        "Breakdown   : Phase 1 median {:.3} s ({:.2} Mcycles/s), best {:.3} s ({:.2} Mcycles/s) | Phase 2a median {:.3} s ({:.2} Mcycles/s), best {:.3} s ({:.2} Mcycles/s)",
        p1_med_s,
        mcycles_per_sec(p1_cycles, p1_med_s),
        p1_best_s,
        mcycles_per_sec(p1_cycles, p1_best_s),
        p2_med_s,
        mcycles_per_sec(p2_cycles, p2_med_s),
        p2_best_s,
        mcycles_per_sec(p2_cycles, p2_best_s)
    );
    println!(
        "Correctness : {passed_count}/{total_suites} PASSED, {deferred_count} DEFERRED, {failed_count} FAILED"
    );

    ExitCode::SUCCESS
}

const MIPS_GPR_NAMES: [&str; 32] = [
    "$zero", "$at", "$v0", "$v1", "$a0", "$a1", "$a2", "$a3", "$t0", "$t1", "$t2", "$t3", "$t4",
    "$t5", "$t6", "$t7", "$s0", "$s1", "$s2", "$s3", "$s4", "$s5", "$s6", "$s7", "$t8", "$t9",
    "$k0", "$k1", "$gp", "$sp", "$fp", "$ra",
];

fn peek_ee_word(bus: &Bus, vaddr: u32) -> u32 {
    let paddr = vaddr & 0x1FFF_FFFF;
    match paddr {
        0x0000_0000..=0x01FF_FFFF => {
            let off = (paddr as usize) & (bus.ram.len() - 4);
            u32::from_le_bytes([
                bus.ram[off],
                bus.ram[off + 1],
                bus.ram[off + 2],
                bus.ram[off + 3],
            ])
        }
        0x1FC0_0000..=0x1FFF_FFFF => {
            let off = (paddr as usize) & (bus.bios.len() - 4);
            u32::from_le_bytes([
                bus.bios[off],
                bus.bios[off + 1],
                bus.bios[off + 2],
                bus.bios[off + 3],
            ])
        }
        _ => 0,
    }
}

fn peek_iop_word(bus: &Bus, vaddr: u32) -> u32 {
    let paddr = vaddr & 0x1FFF_FFFF;
    match paddr {
        0x0000_0000..=0x007F_FFFF => {
            let off = (paddr as usize) & (bus.iop_ram.len() - 4);
            u32::from_le_bytes([
                bus.iop_ram[off],
                bus.iop_ram[off + 1],
                bus.iop_ram[off + 2],
                bus.iop_ram[off + 3],
            ])
        }
        0x1FC0_0000..=0x1FFF_FFFF => {
            let off = (paddr as usize) & (bus.bios.len() - 4);
            u32::from_le_bytes([
                bus.bios[off],
                bus.bios[off + 1],
                bus.bios[off + 2],
                bus.bios[off + 3],
            ])
        }
        _ => 0,
    }
}

fn disasm_mips_instr(pc: u32, instr: u32) -> String {
    if instr == 0 {
        return "nop".to_owned();
    }
    let op = instr >> 26;
    let rs = ((instr >> 21) & 0x1F) as usize;
    let rt = ((instr >> 16) & 0x1F) as usize;
    let rd = ((instr >> 11) & 0x1F) as usize;
    let sa = (instr >> 6) & 0x1F;
    let funct = instr & 0x3F;
    let imm = (instr & 0xFFFF) as u16;
    let simm = imm as i16;
    let btarget = pc
        .wrapping_add(4)
        .wrapping_add(((simm as i32) as u32).wrapping_shl(2));
    let jtarget = (pc.wrapping_add(4) & 0xF000_0000) | ((instr & 0x03FF_FFFF) << 2);

    match op {
        0x00 => match funct {
            0x00 => format!("sll {}, {}, {sa}", MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rt]),
            0x02 => format!("srl {}, {}, {sa}", MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rt]),
            0x03 => format!("sra {}, {}, {sa}", MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rt]),
            0x08 => format!("jr {}", MIPS_GPR_NAMES[rs]),
            0x09 => format!("jalr {}, {}", MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs]),
            0x0C => "syscall".to_owned(),
            0x0F => "sync".to_owned(),
            0x10 => format!("mfhi {}", MIPS_GPR_NAMES[rd]),
            0x12 => format!("mflo {}", MIPS_GPR_NAMES[rd]),
            0x18 => format!(
                "mult {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x19 => format!(
                "multu {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x1A => format!("div {}, {}", MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]),
            0x1B => format!("divu {}, {}", MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]),
            0x21 => format!(
                "addu {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x23 => format!(
                "subu {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x24 => format!(
                "and {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x25 => format!(
                "or {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x26 => format!(
                "xor {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x27 => format!(
                "nor {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x2A => format!(
                "slt {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x2B => format!(
                "sltu {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            0x2D => format!(
                "daddu {}, {}, {}",
                MIPS_GPR_NAMES[rd], MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
            ),
            _ => format!("special(funct=0x{funct:02X})"),
        },
        0x01 => match rt {
            0x00 => format!("bltz {}, 0x{btarget:08X}", MIPS_GPR_NAMES[rs]),
            0x01 => format!("bgez {}, 0x{btarget:08X}", MIPS_GPR_NAMES[rs]),
            0x10 => format!("bltzal {}, 0x{btarget:08X}", MIPS_GPR_NAMES[rs]),
            0x11 => format!("bgezal {}, 0x{btarget:08X}", MIPS_GPR_NAMES[rs]),
            _ => format!("regimm(rt=0x{rt:02X})"),
        },
        0x02 => format!("j 0x{jtarget:08X}"),
        0x03 => format!("jal 0x{jtarget:08X}"),
        0x04 => format!(
            "beq {}, {}, 0x{btarget:08X}",
            MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
        ),
        0x05 => format!(
            "bne {}, {}, 0x{btarget:08X}",
            MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
        ),
        0x06 => format!("blez {}, 0x{btarget:08X}", MIPS_GPR_NAMES[rs]),
        0x07 => format!("bgtz {}, 0x{btarget:08X}", MIPS_GPR_NAMES[rs]),
        0x08 => format!(
            "addi {}, {}, {simm}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x09 => format!(
            "addiu {}, {}, 0x{imm:04X} ({simm})",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x0A => format!(
            "slti {}, {}, {simm}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x0B => format!(
            "sltiu {}, {}, {simm}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x0C => format!(
            "andi {}, {}, 0x{imm:04X}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x0D => format!(
            "ori {}, {}, 0x{imm:04X}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x0E => format!(
            "xori {}, {}, 0x{imm:04X}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x0F => format!("lui {}, 0x{imm:04X}", MIPS_GPR_NAMES[rt]),
        0x10 => match rs {
            0x00 => format!("mfc0 {}, ${rd}", MIPS_GPR_NAMES[rt]),
            0x04 => format!("mtc0 {}, ${rd}", MIPS_GPR_NAMES[rt]),
            0x10 => format!("cop0_co(funct=0x{funct:02X})"),
            _ => format!("cop0(rs=0x{rs:02X})"),
        },
        0x14 => format!(
            "beql {}, {}, 0x{btarget:08X}",
            MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
        ),
        0x15 => format!(
            "bnel {}, {}, 0x{btarget:08X}",
            MIPS_GPR_NAMES[rs], MIPS_GPR_NAMES[rt]
        ),
        0x19 => format!(
            "daddiu {}, {}, {simm}",
            MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]
        ),
        0x1E => format!("lq {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x1F => format!("sq {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x20 => format!("lb {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x21 => format!("lh {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x23 => format!("lw {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x24 => format!("lbu {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x25 => format!("lhu {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x27 => format!("lwu {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x28 => format!("sb {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x29 => format!("sh {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x2B => format!("sw {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x2F => format!("cache 0x{rt:02X}, {simm}({})", MIPS_GPR_NAMES[rs]),
        0x37 => format!("ld {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        0x3F => format!("sd {}, {simm}({})", MIPS_GPR_NAMES[rt], MIPS_GPR_NAMES[rs]),
        _ => format!("op=0x{op:02X} (0x{instr:08X})"),
    }
}

fn print_disasm_window(bus: &Bus, is_iop: bool, center_pc: u32) {
    for delta in -2i32..=2i32 {
        let addr = center_pc.wrapping_add((delta * 4) as u32);
        let word = if is_iop {
            peek_iop_word(bus, addr)
        } else {
            peek_ee_word(bus, addr)
        };
        let marker = if delta == 0 { "=>" } else { "  " };
        println!(
            "  {marker} 0x{addr:08X}: 0x{word:08X}  {}",
            disasm_mips_instr(addr, word)
        );
    }
}

struct BiosBootReport {
    stop_cpu: &'static str,
    stop_pc: u32,
    stop_reason: RunStopReason,
    ee_cycles: u64,
    iop_cycles: u64,
    ee_pc: u32,
    iop_pc: u32,
    bus: Bus,
}

fn run_bios_boot_bytes(
    bios_bytes: &[u8],
    max_ee_cycles: u64,
) -> Result<BiosBootReport, &'static str> {
    let mut ee = EeCpuState::new();
    let mut iop = IopCpuState::new();
    let mut bus = Bus::new();
    bus.load_bios(bios_bytes)?;

    let mut ee_cycles = 0u64;
    let mut iop_cycles = 0u64;

    let (stop_cpu, stop_pc, stop_reason) = 'outer: loop {
        if ee_cycles >= max_ee_cycles {
            break ("EE/IOP", ee.pc, RunStopReason::MaxCyclesReached(ee_cycles));
        }

        for _ in 0..8 {
            let ee_pc_before = ee.pc;
            let ee_instr = peek_ee_word(&bus, ee_pc_before);
            let opcode = ee_instr >> 26;
            if matches!(opcode, 0x12 | 0x32 | 0x3A) {
                break 'outer (
                    "EE",
                    ee_pc_before,
                    RunStopReason::UnimplementedInstruction {
                        pc: ee_pc_before,
                        instr: ee_instr,
                        category: "COP2 / VU0 Macro (Phase 2)",
                    },
                );
            }

            ee.step(&mut bus);
            ee_cycles += 1;

            if let Some(reg_err) = bus.pending_ee_mmio_error.take() {
                break 'outer (
                    "EE",
                    ee_pc_before,
                    RunStopReason::UnimplementedEeRegister(reg_err),
                );
            }
            if let Some(dma_err) = bus.dmac.unimplemented_transfer.take() {
                break 'outer (
                    "EE",
                    ee_pc_before,
                    RunStopReason::UnimplementedDmaTransfer(dma_err),
                );
            }
        }

        let iop_pc_before = iop.pc;
        let iop_res = iop.step_coupled(&mut bus);
        iop_cycles += 1;
        match iop_res {
            IopStepResult::Continue | IopStepResult::Exited(_) => {}
            IopStepResult::UnimplementedImport(imp) => {
                break 'outer (
                    "IOP",
                    iop_pc_before,
                    RunStopReason::UnimplementedIopImport(imp),
                );
            }
            IopStepResult::UnimplementedRegister(reg_err) => {
                break 'outer (
                    "IOP",
                    iop_pc_before,
                    RunStopReason::UnimplementedIopRegister(reg_err),
                );
            }
            IopStepResult::UnimplementedInstruction { pc, instr } => {
                break 'outer (
                    "IOP",
                    pc,
                    RunStopReason::UnimplementedInstruction {
                        pc,
                        instr,
                        category: "IOP_R3000A",
                    },
                );
            }
        }
    };

    Ok(BiosBootReport {
        stop_cpu,
        stop_pc,
        stop_reason,
        ee_cycles,
        iop_cycles,
        ee_pc: ee.pc,
        iop_pc: iop.pc,
        bus,
    })
}

fn run_bios_boot(bios_path: &Path, max_ee_cycles: u64) -> ExitCode {
    let resolved_path = if bios_path.is_dir() {
        bios_path.join("scph10000.bin")
    } else {
        bios_path.to_path_buf()
    };

    let bios_bytes = match fs::read(&resolved_path) {
        Ok(b) => b,
        Err(err) => {
            eprintln!(
                "ERROR: failed to read BIOS image at {}: {err}",
                resolved_path.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let report = match run_bios_boot_bytes(&bios_bytes, max_ee_cycles) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("ERROR: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("=== LLE BIOS BOOT REPORT ===");
    println!(
        "Stopped on: {} at PC = 0x{:08X}",
        report.stop_cpu, report.stop_pc
    );
    println!("Stop reason: {}", report.stop_reason);
    println!(
        "Progress: ee_cycles = {}, iop_cycles = {} (current EE PC = 0x{:08X}, IOP PC = 0x{:08X})",
        report.ee_cycles, report.iop_cycles, report.ee_pc, report.iop_pc
    );
    if report.stop_cpu == "EE" || report.stop_cpu == "EE/IOP" {
        println!("EE Disassembly around 0x{:08X}:", report.stop_pc);
        print_disasm_window(&report.bus, false, report.stop_pc);
    }
    if report.stop_cpu == "IOP" || report.stop_cpu == "EE/IOP" {
        let pc_to_show = if report.stop_cpu == "IOP" {
            report.stop_pc
        } else {
            report.iop_pc
        };
        println!("IOP Disassembly around 0x{:08X}:", pc_to_show);
        print_disasm_window(&report.bus, true, pc_to_show);
    }
    if !report.bus.stdout_buffer.is_empty() {
        let out = String::from_utf8_lossy(&report.bus.stdout_buffer);
        println!(
            "Console output ({} bytes):\n{}",
            report.bus.stdout_buffer.len(),
            out
        );
    }

    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    if args.boot_bios {
        return run_bios_boot(&args.bios, args.max_cycles);
    }

    if let Some(elf_path) = args.elf.as_ref().or(args.test.as_ref()) {
        return run_single_elf_file(elf_path, args.max_cycles);
    }

    let resolved_tests_dir = resolve_tests_dir(&args.tests_dir);

    if args.bench {
        return run_bench(&resolved_tests_dir, args.max_cycles, args.bench_runs);
    }

    if args.gate || resolved_tests_dir.exists() {
        return run_hardware_gate(&resolved_tests_dir, args.max_cycles);
    }

    eprintln!(
        "ERROR: tests directory {} does not exist; pass --tests-dir <path> (or --suite-dir <path>) or a single test binary",
        args.tests_dir.display()
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid 32-bit little-endian MIPS ELF (`e_entry = 0x0010_0000`) with a single
    /// `PT_LOAD` segment containing `instructions` followed by `rodata`.
    fn build_synthetic_mips_elf(entry_vaddr: u32, instructions: &[u32], rodata: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(instructions.len() * 4 + rodata.len());
        for &word in instructions {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        payload.extend_from_slice(rodata);

        let filesz = payload.len() as u32;
        let memsz = filesz + 64;
        let phoff = 52u32;
        let data_off = 128u32;
        let mut elf = vec![0u8; (data_off + filesz) as usize];

        // ELF header
        elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        elf[4] = 1; // ELFCLASS32
        elf[5] = 1; // ELFDATA2LSB
        elf[6] = 1; // EV_CURRENT
        elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        elf[18..20].copy_from_slice(&8u16.to_le_bytes()); // EM_MIPS
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        elf[24..28].copy_from_slice(&entry_vaddr.to_le_bytes());
        elf[28..32].copy_from_slice(&phoff.to_le_bytes());
        elf[40..42].copy_from_slice(&52u16.to_le_bytes());
        elf[42..44].copy_from_slice(&32u16.to_le_bytes());
        elf[44..46].copy_from_slice(&1u16.to_le_bytes());

        // Single PT_LOAD program header
        let ph = &mut elf[52..84];
        ph[0..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        ph[4..8].copy_from_slice(&data_off.to_le_bytes());
        ph[8..12].copy_from_slice(&entry_vaddr.to_le_bytes());
        ph[12..16].copy_from_slice(&entry_vaddr.to_le_bytes());
        ph[16..20].copy_from_slice(&filesz.to_le_bytes());
        ph[20..24].copy_from_slice(&memsz.to_le_bytes());
        ph[24..28].copy_from_slice(&7u32.to_le_bytes());
        ph[28..32].copy_from_slice(&16u32.to_le_bytes());

        elf[data_off as usize..].copy_from_slice(&payload);
        elf
    }

    #[test]
    fn runner_executes_elf_captures_stdout_and_exits_cleanly() {
        // Program at 0x0010_0000:
        // 1. Calls SetupThread (0x3C) with $a1 = 0xFFFF_FFFF -> sets $sp ($29) = 0x0200_0000
        // 2. Writes "OK\n" to 0x1000_F180 (EE_SIO / KPUTCHAR)
        // 3. Calls Exit(0) (0x04)
        let code = [
            // ori $a1 ($5), $zero, 0xFFFF
            0x3405_FFFF,
            // sll $a1 ($5), $a1, 16
            0x0005_2C00,
            // ori $a1 ($5), $a1, 0xFFFF
            0x34A5_FFFF,
            // addiu $v1 ($3), $zero, 0x3C (SetupThread)
            0x2403_003C,
            // syscall
            0x0000_000C,
            // daddu $sp ($29), $v0 ($2), $zero
            0x0040_E82D,
            // lui $t0 ($8), 0x1000
            0x3C08_1000,
            // ori $t0 ($8), $t0, 0xF180
            0x3508_F180,
            // ori $t1 ($9), $zero, 'O' (0x4F)
            0x3409_004F,
            // sb $t1, 0($t0)
            0xA109_0000,
            // ori $t1 ($9), $zero, 'K' (0x4B)
            0x3409_004B,
            // sb $t1, 0($t0)
            0xA109_0000,
            // ori $t1 ($9), $zero, '\n' (0x0A)
            0x3409_000A,
            // sb $t1, 0($t0)
            0xA109_0000,
            // addiu $a0 ($4), $zero, 0
            0x2404_0000,
            // addiu $v1 ($3), $zero, 0x04 (Exit)
            0x2403_0004,
            // syscall
            0x0000_000C,
        ];

        let elf = build_synthetic_mips_elf(0x0010_0000, &code, &[]);
        let report = run_elf_bytes(&elf, 1_000).expect("synthetic ELF should load and run");
        assert_eq!(report.stop_reason, RunStopReason::Exited(0));
        assert_eq!(report.stdout, "OK\n");
    }

    #[test]
    fn runner_executes_mmi_cop1_and_cop2() {
        // MMI POR (`0x7010_94A9`), COP1 MTC1 (`0x4480_0000`), COP2 VNOP (`0x4A00_02FF`), followed by Exit(0)
        let code = [
            0x7010_94A9, // por $s2, $zero, $s0
            0x4480_0000, // mtc1 $zero, $f0
            0x4A00_02FF, // vnop
            0x2404_0000, // addiu $a0, $zero, 0
            0x2403_0004, // addiu $v1, $zero, 0x04 (Exit)
            0x0000_000C, // syscall
        ];
        let elf_ok = build_synthetic_mips_elf(0x0010_0000, &code, &[]);
        let rep_ok = run_elf_bytes(&elf_ok, 100).expect("ELF should load");
        assert_eq!(rep_ok.stop_reason, RunStopReason::Exited(0));
    }

    #[test]
    fn runner_executes_irx_and_reports_unimplemented_iop_import() {
        // Build a synthetic ET_SCE_IOPRELEXEC (0xFF80) IRX image:
        // Offset 0x00..0x14: .iop.import header for "stdio\0\0\0"
        // Offset 0x14..0x1C: jr $ra; addiu $zero, $zero, 4 (printf)
        // Offset 0x1C..0x24: 0, 0 terminator
        // Offset 0x24..0x38: _start:
        //   0x24: lui   $a0, 0x0001       (0x3C04_0001)
        //   0x28: ori   $a0, $a0, 0x0038  (0x3484_0038) -> "HI\n\0" at base + 0x38
        //   0x2C: jal   0x0001_0014       (0x0C00_4005) -> call printf stub at 0x0001_0014
        //   0x30: nop                     (0x0000_0000)
        //   0x34: jr    $ra               (0x03E0_0008) -> wait: _start saves $ra or jumps after
        // Better: since _start here is leaf-calling printf, save $ra in $s0 ($16):
        let words = [
            0x41E0_0000, // 0x00: import magic
            0x0000_0000, // 0x04: next
            0x0000_0102, // 0x08: version
            u32::from_le_bytes(*b"stdi"),
            u32::from_le_bytes(*b"o\0\0\0"),
            0x03E0_0008, // 0x14: jr $ra
            0x2400_0004, // 0x18: addiu $zero, $zero, 4 (printf)
            0x0000_0000, // 0x1C: terminator
            0x0000_0000, // 0x20: terminator
            // _start at 0x24:
            0x001F_8025, // 0x24: or $s0, $zero, $ra
            0x3C04_0001, // 0x28: lui $a0, 0x0001
            0x3484_0040, // 0x2C: ori $a0, $a0, 0x0040 ("HI\n\0")
            0x0C00_4005, // 0x30: jal 0x0001_0014
            0x0000_0000, // 0x34: nop
            0x0200_0008, // 0x38: jr $s0
            0x2402_0000, // 0x3C: addiu $v0, $zero, 0
            u32::from_le_bytes(*b"HI\n\0"),
        ];
        let mut irx = build_synthetic_mips_elf(0x0000_0024, &words, &[]);
        irx[16..18].copy_from_slice(&0xFF80u16.to_le_bytes()); // ET_SCE_IOPRELEXEC
        irx[52 + 8..52 + 12].copy_from_slice(&0u32.to_le_bytes()); // p_vaddr = 0
        irx[52 + 12..52 + 16].copy_from_slice(&0u32.to_le_bytes()); // p_paddr = 0

        let rep = run_irx_bytes(&irx, 1_000).expect("synthetic IRX should load");
        assert_eq!(rep.stop_reason, RunStopReason::Exited(0));
        assert_eq!(rep.stdout, "HI\n");
        assert_eq!(rep.ee_cycles, rep.iop_cycles * 8);

        // Change import index from 4 (printf) to 99 (unimplemented) and verify explicit error:
        let mut words_unimp = words;
        words_unimp[6] = 0x2400_0063; // index 99
        let mut irx_unimp = build_synthetic_mips_elf(0x0000_0024, &words_unimp, &[]);
        irx_unimp[16..18].copy_from_slice(&0xFF80u16.to_le_bytes());
        irx_unimp[52 + 8..52 + 12].copy_from_slice(&0u32.to_le_bytes());
        irx_unimp[52 + 12..52 + 16].copy_from_slice(&0u32.to_le_bytes());

        let rep_unimp = run_irx_bytes(&irx_unimp, 1_000).expect("synthetic IRX should load");
        assert_eq!(
            rep_unimp.stop_reason,
            RunStopReason::UnimplementedIopImport(UnimplementedIopImport {
                pc: 0x0001_0014,
                module: "stdio".to_owned(),
                index: 99,
            })
        );
    }

    #[test]
    fn bios_boot_regression_guard_reaches_eeconf_if_local_bios_present() {
        let bios_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/bios");
        let mut bios_bytes: Option<Vec<u8>> = None;
        if let Ok(entries) = fs::read_dir(&bios_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(meta) = path.metadata() {
                        if meta.len() == 4 * 1024 * 1024 {
                            if let Ok(bytes) = fs::read(&path) {
                                bios_bytes = Some(bytes);
                                break;
                            }
                        }
                    }
                }
            }
        }

        let Some(bytes) = bios_bytes else {
            println!("skipped");
            return;
        };

        let report =
            run_bios_boot_bytes(&bytes, 5_000_000).expect("4 MB BIOS image should load cleanly");
        assert!(
            report.ee_cycles >= 2_180_464,
            "expected ee_cycles >= 2,180,464, got {}",
            report.ee_cycles
        );
        assert!(
            report.iop_cycles >= 272_558,
            "expected iop_cycles >= 272,558, got {}",
            report.iop_cycles
        );
        assert_eq!(
            report.stop_reason,
            RunStopReason::UnimplementedIopRegister(UnimplementedIopRegister {
                paddr: 0x1F40_2005,
                width: 1,
                is_write: false,
                value: 0,
            })
        );
    }

    #[test]
    fn hardware_gate_lists_52_suites_and_ignores_only_deferred_efu_failure() {
        assert_eq!(
            PHASE1_HW_GATE_SUITES.len() + PHASE2A_HW_GATE_SUITES.len(),
            52
        );
        assert_eq!(
            DEFERRED_GATE_SUITES,
            [(
                "vu/lower/efu.elf",
                "partial: 184/208 vectors; see HARDWARE_NOTES.md EFU section"
            )]
        );
        assert_eq!(
            evaluate_gate_outcome("vu/lower/efu.elf", false),
            GateOutcome::Deferred("partial: 184/208 vectors; see HARDWARE_NOTES.md EFU section")
        );
        assert_eq!(
            evaluate_gate_outcome("vu/lower/efu.elf", true),
            GateOutcome::Pass
        );
        assert_eq!(
            evaluate_gate_outcome("gs/label.elf", false),
            GateOutcome::Fail
        );
        assert_eq!(
            evaluate_gate_outcome("cpu/ee/alu.elf", false),
            GateOutcome::Fail
        );
    }

    #[test]
    fn cli_accepts_bench_and_tests_dir_aliases() {
        let parsed = Args::try_parse_from([
            "ps2-autotest",
            "--bench",
            "--tests-dir",
            "custom/ps2autotests",
        ])
        .expect("--bench and --tests-dir should parse");
        assert!(parsed.bench);
        assert!(!parsed.gate);
        assert_eq!(parsed.bench_runs, 3);
        assert_eq!(parsed.tests_dir, PathBuf::from("custom/ps2autotests"));

        let alias_parsed =
            Args::try_parse_from(["ps2-autotest", "--gate", "--suite-dir", "legacy/tests"])
                .expect("--suite-dir alias should continue to parse");
        assert!(alias_parsed.gate);
        assert_eq!(alias_parsed.tests_dir, PathBuf::from("legacy/tests"));
    }

    #[test]
    fn resolve_tests_dir_handles_repo_root_and_direct_tests_dir() {
        let base = std::env::temp_dir().join(format!(
            "ps2_autotest_tests_dir_check_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);

        let repo_root = base.join("cloned_repo");
        let nested_tests = repo_root.join("tests");
        fs::create_dir_all(nested_tests.join("cpu")).expect("create nested tests/cpu dir");
        assert_eq!(resolve_tests_dir(&repo_root), nested_tests);
        assert_eq!(resolve_tests_dir(&nested_tests), nested_tests);

        let missing = base.join("missing");
        assert_eq!(resolve_tests_dir(&missing), missing);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn summarize_secs_and_formatting_work_for_odd_and_even_samples() {
        assert_eq!(summarize_secs(&[2.5, 1.5, 2.0]), (1.5, 2.0, 2.5));
        assert_eq!(summarize_secs(&[4.0, 1.0, 3.0, 2.0]), (1.0, 2.5, 4.0));
        assert_eq!(summarize_secs(&[]), (0.0, 0.0, 0.0));
        assert_eq!(format_commas(0), "0");
        assert_eq!(format_commas(114_447_792), "114,447,792");
    }
}
