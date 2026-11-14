//! Headless CLI runner for `ps2autotests` `.elf` test suites.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use ps2_core::bus::Bus;
use ps2_core::ee::{EeCpuState, EeHleState, HleStepResult};
use ps2_core::{load_elf32, ElfError, LoadedElf};
use similar::TextDiff;

#[derive(Debug, Parser)]
#[command(name = "ps2-autotest", about = "Headless PS2 autotest runner")]
struct Args {
    /// Positional path to a `.elf` test binary.
    #[arg(value_name = "ELF")]
    elf: Option<PathBuf>,

    /// Path to a single `.elf` test binary (alias for positional `ELF`).
    #[arg(long)]
    test: Option<PathBuf>,

    /// Path to the `ps2autotests` `tests/` directory.
    #[arg(long, default_value = "local/ps2autotests/tests")]
    suite_dir: PathBuf,

    /// Optional path to a 4 MB PS2 BIOS dump directory or file.
    #[arg(long, default_value = "local/bios")]
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

    let report = match run_elf_bytes(&elf_bytes, max_cycles) {
        Ok(rep) => rep,
        Err(err) => {
            eprintln!("ERROR: failed to load ELF {}: {err}", elf_path.display());
            return ExitCode::FAILURE;
        }
    };

    println!("=== EE STDOUT ({}) ===", elf_path.display());
    print!("{}", report.stdout);
    if !report.stdout.ends_with('\n') && !report.stdout.is_empty() {
        println!();
    }
    println!(
        "=== STOP: {} after {} cycles (entry=0x{:08X}) ===",
        report.stop_reason, report.cycles, report.loaded.entry_pc
    );

    let expected_path = elf_path.with_extension("expected");
    if expected_path.exists() {
        match fs::read_to_string(&expected_path) {
            Ok(raw_expected) => {
                let expected = normalize_newlines(&raw_expected);
                if report.stdout == expected {
                    println!("RESULT: PASS ({})", elf_path.display());
                    ExitCode::SUCCESS
                } else {
                    println!("RESULT: FAIL ({})", elf_path.display());
                    print!("{}", format_expected_diff(&expected, &report.stdout));
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

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    if let Some(elf_path) = args.elf.as_ref().or(args.test.as_ref()) {
        return run_single_elf_file(elf_path, args.max_cycles);
    }

    tracing::info!(
        suite_dir = %args.suite_dir.display(),
        bios = %args.bios.display(),
        max_cycles = args.max_cycles,
        "No ELF path provided; pass <path/to/test.elf> or --test <path/to/test.elf>"
    );
    ExitCode::SUCCESS
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
    fn runner_executes_mmi_cop1_and_reports_out_of_scope_cop2() {
        // MMI POR (`0x7010_94A9`), COP1 MTC1 (`0x4480_0000`), followed by Exit(0)
        let code = [
            0x7010_94A9, // por $s2, $zero, $s0
            0x4480_0000, // mtc1 $zero, $f0
            0x2404_0000, // addiu $a0, $zero, 0
            0x2403_0004, // addiu $v1, $zero, 0x04 (Exit)
            0x0000_000C, // syscall
        ];
        let elf_ok = build_synthetic_mips_elf(0x0010_0000, &code, &[]);
        let rep_ok = run_elf_bytes(&elf_ok, 100).expect("ELF should load");
        assert_eq!(rep_ok.stop_reason, RunStopReason::Exited(0));

        // Immediate COP2 / VU0 Macro instruction (`0x4A00_0000`) at entry point
        let elf_cop2 = build_synthetic_mips_elf(0x0010_0000, &[0x4A00_0000], &[]);
        let rep_cop2 = run_elf_bytes(&elf_cop2, 100).expect("ELF should load");
        assert_eq!(
            rep_cop2.stop_reason,
            RunStopReason::UnimplementedInstruction {
                pc: 0x0010_0000,
                instr: 0x4A00_0000,
                category: "COP2 / VU0 Macro (Out of Phase 1 Scope)",
            }
        );
    }
}
