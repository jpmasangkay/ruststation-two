//! Emotion Engine (MIPS R5900) COP1 Floating-Point Unit (FPU) Interpreter.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eefpu` & `https://psi-rockin.github.io/ps2tek/#eeinstructions`):
//!   * Single-precision (`fmt = 0x10`) and word (`fmt = 0x14`) COP1 instructions, 32 x 32-bit `FPR`s,
//!     32-bit `ACC` accumulator, `FCR0` implementation register, and `FCR31` control/status register.
//! - `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/{arithmetic,branch,branchdelay,compare,convert,fcr,muldiv,sqrt,transfer}.cpp` & `.expected`):
//!   * `FCR0` reads `0x0000_2E30` for control register indices `0..=15`; `FCR31` is mirrored across
//!     indices `16..=31` (`fcr.expected` lines 3 & 11).
//!   * `CTC1` only writes `FCR31` when `fs == 31`, with writable mask `0x0083_C078` and hardwired bits
//!     `0x0100_0001` (`fcr.expected` lines 4..29, `transfer.expected` line 20).
//!   * `ABS.S`, `NEG.S`, `MAX.S`, and `MIN.S` clear the `I, D, O, U` cause bits in `FCR31` without
//!     flushing denormals (`arithmetic.expected`).
//!   * `MOV.S`, `CVT.W.S`, `CVT.S.W`, and `C.<cond>.S` do not modify the `I, D, O, U` cause bits in `FCR31`.

use ps2_common::ps2float::{self, Fcr31Flags};

use crate::bus::Bus;

use super::cop0::{cause_bits, cop0_reg, status_bits, Level1ExceptionCode};
use super::interpreter::{branch_target, StepOutcome};
use super::state::{EeCpuState, EE_COP1_FCR31_DEFAULT};

/// Writable bits in `FCR31` via `CTC1 $rt, $31` (`C` bit 23, `I/D/O/U` bits `17..=14`, `SI/SD/SO/SU` bits `6..=3`).
///
/// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` lines 4..29
/// & `local/ps2autotests/tests/cpu/ee_fpu/transfer.expected` line 20).
pub const FCR31_WRITABLE_MASK: u32 = 0x0083_C078;

impl EeCpuState {
    /// Check whether `COP1` is usable (`COP0.Status.CU1` bit 29 set); if not, raise a
    /// Level 1 `CoprocessorUnusable` exception with `Cause.CE = 1`.
    #[inline(always)]
    fn check_cop1_usable(&mut self) -> bool {
        if (self.cop0_regs[cop0_reg::STATUS] & status_bits::CU1) == 0 {
            self.cop0_regs[cop0_reg::CAUSE] =
                (self.cop0_regs[cop0_reg::CAUSE] & !cause_bits::CE_MASK) | (1 << 28);
            self.raise_level1_exception(Level1ExceptionCode::CoprocessorUnusable, false);
            return false;
        }
        true
    }

    /// Update `FCR31` cause (`I, D, O, U`) and sticky (`SI, SD, SO, SU`) flags after an arithmetic instruction.
    #[inline(always)]
    fn apply_fpu_flags(&mut self, flags: Fcr31Flags) {
        let cleared = self.fcr31 & !Fcr31Flags::CAUSE_MASK.bits();
        self.fcr31 = cleared | flags.bits();
    }

    /// Set or clear the condition bit `FCR31.C` (bit 23).
    #[inline(always)]
    fn set_fcr31_cond(&mut self, cond: bool) {
        if cond {
            self.fcr31 |= Fcr31Flags::C.bits();
        } else {
            self.fcr31 &= !Fcr31Flags::C.bits();
        }
    }

    /// Execute `LWC1 ft, offset(base)` (`opcode == 0x31`).
    ///
    /// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/transfer.expected` lines 2..8).
    pub(super) fn execute_lwc1(
        &mut self,
        rs: usize,
        ft: usize,
        simm16: i32,
        bus: &mut Bus,
    ) -> StepOutcome {
        if !self.check_cop1_usable() {
            return StepOutcome::ExceptionOrEret;
        }
        let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
        if (vaddr & 3) != 0 {
            self.raise_address_error(vaddr, false);
            return StepOutcome::ExceptionOrEret;
        }
        self.fpr[ft & 31] = bus.read32(vaddr);
        StepOutcome::Normal
    }

    /// Execute `SWC1 ft, offset(base)` (`opcode == 0x39`).
    ///
    /// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/transfer.expected` lines 10..16).
    pub(super) fn execute_swc1(
        &mut self,
        rs: usize,
        ft: usize,
        simm16: i32,
        bus: &mut Bus,
    ) -> StepOutcome {
        if !self.check_cop1_usable() {
            return StepOutcome::ExceptionOrEret;
        }
        let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
        if (vaddr & 3) != 0 {
            self.raise_address_error(vaddr, true);
            return StepOutcome::ExceptionOrEret;
        }
        bus.write32(vaddr, self.fpr[ft & 31]);
        StepOutcome::Normal
    }

    /// Execute a `COP1` (`opcode == 0x11`) instruction.
    pub(super) fn execute_cop1(&mut self, instr: u32, instr_pc: u32, simm16: i32) -> StepOutcome {
        if !self.check_cop1_usable() {
            return StepOutcome::ExceptionOrEret;
        }

        let fmt = ((instr >> 21) & 0x1F) as usize;
        let ft = ((instr >> 16) & 0x1F) as usize;
        let fs = ((instr >> 11) & 0x1F) as usize;
        let fd = ((instr >> 6) & 0x1F) as usize;
        let funct = instr & 0x3F;

        match fmt {
            // MFC1 rt, fs (fmt == 0x00)
            0x00 => {
                let val = self.fpr[fs] as i32;
                self.set_gpr_sext32(ft, val);
                StepOutcome::Normal
            }

            // CFC1 rt, fs (fmt == 0x02)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` lines 3 & 11):
            // FCR0..15 mirror FCR0 (`0x0000_2E30`); FCR16..31 mirror FCR31.
            0x02 => {
                let val = if (fs & 16) == 0 {
                    self.fcr0
                } else {
                    self.fcr31
                };
                self.set_gpr_sext32(ft, val as i32);
                StepOutcome::Normal
            }

            // MTC1 rt, fs (fmt == 0x04)
            0x04 => {
                self.fpr[fs] = self.gpr_u32(ft);
                StepOutcome::Normal
            }

            // CTC1 rt, fs (fmt == 0x06)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` lines 4..29):
            // Only fs == 31 writes FCR31; writable bits are `0x0083_C078` and hardwired bits are `0x0100_0001`.
            0x06 => {
                let val = self.gpr_u32(ft);
                if fs == 31 {
                    self.fcr31 = (val & FCR31_WRITABLE_MASK) | EE_COP1_FCR31_DEFAULT;
                }
                StepOutcome::Normal
            }

            // BC1 (fmt == 0x08): BC1F (0), BC1T (1), BC1FL (2), BC1TL (3)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/{branch,branchdelay}.expected`).
            0x08 => {
                let cond = (self.fcr31 & Fcr31Flags::C.bits()) != 0;
                match ft {
                    // BC1F
                    0x00 => {
                        if !cond {
                            StepOutcome::Cop1BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchNotTaken
                        }
                    }
                    // BC1T
                    0x01 => {
                        if cond {
                            StepOutcome::Cop1BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchNotTaken
                        }
                    }
                    // BC1FL
                    0x02 => {
                        if !cond {
                            StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchLikelyNullified
                        }
                    }
                    // BC1TL
                    0x03 => {
                        if cond {
                            StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchLikelyNullified
                        }
                    }
                    _ => {
                        self.raise_level1_exception(
                            Level1ExceptionCode::ReservedInstruction,
                            false,
                        );
                        StepOutcome::ExceptionOrEret
                    }
                }
            }

            // Single-precision format `.S` (fmt == 0x10)
            0x10 => self.execute_cop1_s(instr, instr_pc, ft, fs, fd, funct),

            // Fixed-point word format `.W` (fmt == 0x14)
            0x14 => match funct {
                // CVT.S.W fd, fs (0x20)
                0x20 => {
                    self.fpr[fd] = ps2float::cvt_s_w(self.fpr[fs] as i32);
                    StepOutcome::Normal
                }
                _ => {
                    self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            _ => {
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }

    /// Execute a single-precision (`fmt == 0x10`) COP1 instruction.
    fn execute_cop1_s(
        &mut self,
        instr: u32,
        instr_pc: u32,
        ft: usize,
        fs: usize,
        fd: usize,
        funct: u32,
    ) -> StepOutcome {
        match funct {
            // ADD.S fd, fs, ft (0x00)
            0x00 => {
                let res = ps2float::add(self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // SUB.S fd, fs, ft (0x01)
            0x01 => {
                let res = ps2float::sub(self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // MUL.S fd, fs, ft (0x02)
            0x02 => {
                let res = ps2float::mul(self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // DIV.S fd, fs, ft (0x03)
            0x03 => {
                let res = ps2float::div(self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // SQRT.S fd, ft (0x04)
            0x04 => {
                let res = ps2float::sqrt(self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // ABS.S fd, fs (0x05)
            0x05 => {
                self.fpr[fd] = ps2float::abs(self.fpr[fs]);
                self.apply_fpu_flags(Fcr31Flags::empty());
                StepOutcome::Normal
            }

            // MOV.S fd, fs (0x06)
            0x06 => {
                self.fpr[fd] = self.fpr[fs];
                StepOutcome::Normal
            }

            // NEG.S fd, fs (0x07)
            0x07 => {
                self.fpr[fd] = ps2float::neg(self.fpr[fs]);
                self.apply_fpu_flags(Fcr31Flags::empty());
                StepOutcome::Normal
            }

            // RSQRT.S fd, fs, ft (0x16)
            0x16 => {
                let res = ps2float::rsqrt(self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // ADDA.S fs, ft (0x18)
            0x18 => {
                let res = ps2float::add(self.fpr[fs], self.fpr[ft]);
                self.facc = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // SUBA.S fs, ft (0x19)
            0x19 => {
                let res = ps2float::sub(self.fpr[fs], self.fpr[ft]);
                self.facc = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // MULA.S fs, ft (0x1A)
            0x1A => {
                let res = ps2float::mul(self.fpr[fs], self.fpr[ft]);
                self.facc = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // MADD.S fd, fs, ft (0x1C)
            0x1C => {
                let res = ps2float::madd(self.facc, self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // MSUB.S fd, fs, ft (0x1D)
            0x1D => {
                let res = ps2float::msub(self.facc, self.fpr[fs], self.fpr[ft]);
                self.fpr[fd] = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // MADDA.S fs, ft (0x1E)
            0x1E => {
                let res = ps2float::madd(self.facc, self.fpr[fs], self.fpr[ft]);
                self.facc = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // MSUBA.S fs, ft (0x1F)
            0x1F => {
                let res = ps2float::msub(self.facc, self.fpr[fs], self.fpr[ft]);
                self.facc = res.bits;
                self.apply_fpu_flags(res.flags);
                StepOutcome::Normal
            }

            // CVT.W.S fd, fs (0x24)
            0x24 => {
                self.fpr[fd] = ps2float::cvt_w_s(self.fpr[fs]) as u32;
                StepOutcome::Normal
            }

            // MAX.S fd, fs, ft (0x28)
            0x28 => {
                self.fpr[fd] = ps2float::max(self.fpr[fs], self.fpr[ft]);
                self.apply_fpu_flags(Fcr31Flags::empty());
                StepOutcome::Normal
            }

            // MIN.S fd, fs, ft (0x29)
            0x29 => {
                self.fpr[fd] = ps2float::min(self.fpr[fs], self.fpr[ft]);
                self.apply_fpu_flags(Fcr31Flags::empty());
                StepOutcome::Normal
            }

            // C.F.S fs, ft (0x30)
            0x30 => {
                self.set_fcr31_cond(false);
                StepOutcome::Normal
            }

            // C.EQ.S fs, ft (0x32)
            0x32 => {
                let cond = ps2float::c_eq(self.fpr[fs], self.fpr[ft]);
                self.set_fcr31_cond(cond);
                StepOutcome::Normal
            }

            // C.LT.S fs, ft (0x34)
            0x34 => {
                let cond = ps2float::c_lt(self.fpr[fs], self.fpr[ft]);
                self.set_fcr31_cond(cond);
                StepOutcome::Normal
            }

            // C.LE.S fs, ft (0x36)
            0x36 => {
                let cond = ps2float::c_le(self.fpr[fs], self.fpr[ft]);
                self.set_fcr31_cond(cond);
                StepOutcome::Normal
            }

            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    funct,
                    "Reserved COP1.S funct"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    const fn encode_cop1_s(ft: u32, fs: u32, fd: u32, funct: u32) -> u32 {
        (0x11 << 26) | (0x10 << 21) | (ft << 16) | (fs << 11) | (fd << 6) | funct
    }

    fn run_instrs(cpu: &mut EeCpuState, bus: &mut Bus, instrs: &[u32]) {
        for (i, &instr) in instrs.iter().enumerate() {
            bus.write32(0x8010_0000 + (i as u32) * 4, instr);
        }
        cpu.pc = 0x8010_0000;
        cpu.next_pc = 0x8010_0004;
        for _ in 0..instrs.len() {
            cpu.step(bus);
        }
    }

    #[test]
    fn cop1_cfc1_ctc1_mirroring_and_mask() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        cpu.set_gpr_sext32(1, 0xFFFF_FFFFu32 as i32);
        // CTC1 $1, $fcr16 -> ignored because fs != 31
        let ctc1_16 = (0x11 << 26) | (0x06 << 21) | (1 << 16) | (16 << 11);
        // CFC1 $2, $fcr16 -> mirrors FCR31 (initially 0x0100_0001)
        let cfc1_16 = (0x11 << 26) | (0x02 << 21) | (2 << 16) | (16 << 11);
        // CFC1 $3, $fcr15 -> mirrors FCR0 (0x0000_2E30)
        let cfc1_15 = (0x11 << 26) | (0x02 << 21) | (3 << 16) | (15 << 11);
        // CTC1 $1, $fcr31 -> writes writable mask 0x0083_C078 | hardwired 0x0100_0001 = 0x0183_C079
        let ctc1_31 = (0x11 << 26) | (0x06 << 21) | (1 << 16) | (31 << 11);
        // CFC1 $4, $fcr31
        let cfc1_31 = (0x11 << 26) | (0x02 << 21) | (4 << 16) | (31 << 11);

        run_instrs(
            &mut cpu,
            &mut bus,
            &[ctc1_16, cfc1_16, cfc1_15, ctc1_31, cfc1_31],
        );

        assert_eq!(cpu.gpr_u32(2), 0x0100_0001);
        assert_eq!(cpu.gpr_u32(3), 0x0000_2E30);
        assert_eq!(cpu.gpr_u32(4), 0x0183_C079);
    }

    #[test]
    fn cop1_mula_madd_and_branch_on_condition() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        cpu.fpr[1] = 0x4000_0000; // 2.0
        cpu.fpr[2] = 0x4040_0000; // 3.0
        cpu.fpr[3] = 0x3F80_0000; // 1.0

        // MULA.S $f1, $f2 (ACC = 6.0)
        let mula = encode_cop1_s(2, 1, 0, 0x1A);
        // MADD.S $f4, $f2, $f3 ($f4 = 6.0 + 3.0 * 1.0 = 9.0 = 0x4110_0000)
        let madd = encode_cop1_s(3, 2, 4, 0x1C);
        // C.LT.S $f1, $f4 (2.0 < 9.0 -> C = 1)
        let c_lt = encode_cop1_s(4, 1, 0, 0x34);
        // BC1T +1 (branch to ADDIU $6, $0, 42)
        let bc1t = (0x11 << 26) | (0x08 << 21) | (0x01 << 16) | 1;
        // Delay slot: ADDIU $5, $0, 10
        let delay = (0x09 << 26) | (5 << 16) | 10;
        // Target: ADDIU $6, $0, 42
        let target = (0x09 << 26) | (6 << 16) | 42;

        run_instrs(&mut cpu, &mut bus, &[mula, madd, c_lt, bc1t, delay, target]);

        assert_eq!(cpu.fpr[4], 0x4110_0000);
        assert_ne!(cpu.fcr31 & Fcr31Flags::C.bits(), 0);
        assert_eq!(cpu.gpr_u32(5), 10);
        assert_eq!(cpu.gpr_u32(6), 42);
    }
}
