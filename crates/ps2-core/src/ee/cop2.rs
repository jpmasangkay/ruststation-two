//! Emotion Engine (MIPS R5900) `COP2` (`VU0` Macro Mode & Control/Data Transfers).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeinstructions` & `#vumacro`, `FETCHED`):
//!   * `LQC2` (`0x36`) and `SQC2` (`0x3E`) load/store 128-bit quadwords between EE virtual
//!     memory (with low 4 address bits masked to `0`) and `VU0` `vf00..vf31`.
//!   * `QMFC2` (`fmt = 0x01`), `CFC2` (`fmt = 0x02`), `QMTC2` (`fmt = 0x05`), `CTC2` (`fmt = 0x06`),
//!     `BC2F/T/FL/TL` (`fmt = 0x08`), and `COP2 Special1/Special2` (`bit 25 = 1`).
//!   * Bit 0 (`.I`) on `QMFC2`/`CFC2`/`QMTC2`/`CTC2` interlocks with `VU0`, waiting for any
//!     running `VU0` microprogram to finish before transferring.
//! - `ps2autotests` (`tests/cpu/vu0_macro/integer.cpp` & `transfer.cpp`, `FETCHED`):
//!   * Verifies `LQC2`/`SQC2` offset/alignment behavior, `VF00` hardwired constant, `CFC2`
//!     32-to-64-bit sign extension preserving upper 64 bits of `GPR[rt]`, `CTC2` register masks
//!     for `vi00..vi31`, `VCALLMS` + `CFC2.I` interlock + `TPC` (`vi26`), and macro integer ops.

use crate::bus::Bus;

use super::cop0::{cause_bits, cop0_reg, status_bits, Level1ExceptionCode};
use super::interpreter::{branch_target, StepOutcome};
use super::state::EeCpuState;

impl EeCpuState {
    /// Check whether `COP2` is usable (`COP0.Status.CU2` bit 30 set); if not, raise a
    /// Level 1 `CoprocessorUnusable` exception with `Cause.CE = 2`.
    #[inline(always)]
    fn check_cop2_usable(&mut self) -> bool {
        if (self.cop0_regs[cop0_reg::STATUS] & status_bits::CU2) == 0 {
            self.cop0_regs[cop0_reg::CAUSE] =
                (self.cop0_regs[cop0_reg::CAUSE] & !cause_bits::CE_MASK) | (2 << 28);
            self.raise_level1_exception(Level1ExceptionCode::CoprocessorUnusable, false);
            return false;
        }
        true
    }

    /// Execute `LQC2 vt, offset(base)` (`opcode == 0x36`).
    pub(super) fn execute_lqc2(
        &mut self,
        rs: usize,
        vt: usize,
        simm16: i32,
        bus: &mut Bus,
    ) -> StepOutcome {
        if !self.check_cop2_usable() {
            return StepOutcome::ExceptionOrEret;
        }
        let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32) & !0xF;
        let val = bus.read128(vaddr);
        bus.vu0.write_vf(vt, val);
        StepOutcome::Normal
    }

    /// Execute `SQC2 vt, offset(base)` (`opcode == 0x3E`).
    pub(super) fn execute_sqc2(
        &mut self,
        rs: usize,
        vt: usize,
        simm16: i32,
        bus: &mut Bus,
    ) -> StepOutcome {
        if !self.check_cop2_usable() {
            return StepOutcome::ExceptionOrEret;
        }
        let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32) & !0xF;
        let val = bus.vu0.read_vf(vt);
        bus.write128(vaddr, val);
        StepOutcome::Normal
    }

    /// Execute a `COP2` (`opcode == 0x12`) instruction.
    pub(super) fn execute_cop2(
        &mut self,
        instr: u32,
        instr_pc: u32,
        simm16: i32,
        bus: &mut Bus,
    ) -> StepOutcome {
        if !self.check_cop2_usable() {
            return StepOutcome::ExceptionOrEret;
        }

        let fmt = (instr >> 21) & 0x1F;
        let rt = ((instr >> 16) & 0x1F) as usize;
        let fs = ((instr >> 11) & 0x1F) as usize;
        let interlock = (instr & 1) != 0;

        match fmt {
            // QMFC2[.I] rt, fs
            0x01 => {
                if interlock {
                    bus.finish_vu0_micro();
                }
                let val = bus.vu0.read_vf(fs);
                self.set_gpr_128(rt, val);
                StepOutcome::Normal
            }
            // CFC2[.I] rt, fs
            0x02 => {
                if interlock {
                    bus.finish_vu0_micro();
                }
                let val = bus.read_cfc2(fs);
                self.set_gpr_sext32(rt, val as i32);
                StepOutcome::Normal
            }
            // QMTC2[.I] rt, fs
            0x05 => {
                if interlock {
                    bus.finish_vu0_micro();
                }
                let val = self.gpr_128(rt);
                bus.vu0.write_vf(fs, val);
                StepOutcome::Normal
            }
            // CTC2[.I] rt, fs
            0x06 => {
                if interlock {
                    bus.finish_vu0_micro();
                }
                let val = self.gpr_u32(rt);
                bus.write_ctc2(fs, val);
                StepOutcome::Normal
            }
            // BC2F / BC2T / BC2FL / BC2TL
            0x08 => {
                let cond = bus.vu0.running;
                let target = branch_target(instr_pc, simm16);
                match rt & 0x3 {
                    // BC2F
                    0 => {
                        if !cond {
                            StepOutcome::BranchTaken(target)
                        } else {
                            StepOutcome::BranchNotTaken
                        }
                    }
                    // BC2T
                    1 => {
                        if cond {
                            StepOutcome::BranchTaken(target)
                        } else {
                            StepOutcome::BranchNotTaken
                        }
                    }
                    // BC2FL
                    2 => {
                        if !cond {
                            StepOutcome::BranchTaken(target)
                        } else {
                            StepOutcome::BranchLikelyNullified
                        }
                    }
                    // BC2TL
                    3 => {
                        if cond {
                            StepOutcome::BranchTaken(target)
                        } else {
                            StepOutcome::BranchLikelyNullified
                        }
                    }
                    _ => unreachable!(),
                }
            }
            // COP2 Special1 / Special2 (bit 25 == 1)
            0x10..=0x1F => {
                let funct = instr & 0x3F;
                match funct {
                    // VCALLMS imm15
                    0x38 => {
                        let imm15 = ((instr >> 6) & 0x7FFF) as u16;
                        bus.start_vu0_micro(imm15);
                    }
                    // VCALLMSR
                    0x39 => {
                        let start = bus.vu0.cmsar0;
                        bus.start_vu0_micro(start);
                    }
                    _ => {
                        bus.exec_vu0_macro(instr);
                    }
                }
                StepOutcome::Normal
            }
            _ => {
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vu::VF00_VALUE;
    use ps2_common::Reg128;

    #[test]
    fn cop2_cfc2_sign_extends_and_ctc2_masks_control_regs() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Set GPR[1] = 0xFFFF_FFFF
        cpu.set_gpr_sext32(1, -1);
        // Set GPR[2] = 0x1234_5678 in all 4 lanes
        cpu.set_gpr_128(
            2,
            Reg128::from_u32x4(0x1234_5678, 0x1234_5678, 0x1234_5678, 0x1234_5678),
        );

        // CTC2 $1, vi21 (fmt = 0x06, rt = 1, fs = 21)
        let ctc2_vi21 = (0x12 << 26) | (0x06 << 21) | (1 << 16) | (21 << 11);
        cpu.execute_cop2(ctc2_vi21, 0x0010_0000, 0, &mut bus);
        assert_eq!(bus.read_cfc2(21), 0xFFFF_FFFF);

        // CFC2 $2, vi21 -> sign-extends low 32 bits into bits 32..63 and preserves bits 64..127
        let cfc2_vi21 = (0x12 << 26) | (0x02 << 21) | (2 << 16) | (21 << 11);
        cpu.execute_cop2(cfc2_vi21, 0x0010_0004, 0, &mut bus);
        assert_eq!(
            cpu.gpr_128(2),
            Reg128::from_u32x4(0xFFFF_FFFF, 0xFFFF_FFFF, 0x1234_5678, 0x1234_5678)
        );

        // Check CTC2 register masks
        let expected_masks = [
            (0usize, 0x0000_0000u32),
            (1, 0x0000_FFFF),
            (16, 0x0000_0FC0),
            (17, 0x0000_0000),
            (18, 0x00FF_FFFF),
            (19, 0x0000_2E30),
            (20, 0x007F_FFFF),
            (21, 0xFFFF_FFFF),
            (22, 0xFFFF_FFFF),
            (23, 0x0000_0000),
            (24, 0x0000_0C0C),
            (25, 0x0000_0000),
            (27, 0x0000_FFFF),
            (28, 0x0000_0000),
            (30, 0x0000_0000),
        ];
        for (reg, expected) in expected_masks {
            bus.write_ctc2(reg, 0xFFFF_FFFF);
            assert_eq!(bus.read_cfc2(reg), expected, "mismatch on vi{reg}");
        }
    }

    #[test]
    fn cop2_lqc2_sqc2_mask_low_4_address_bits_and_protect_vf00() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        let pattern = Reg128::from_u32x4(0x2345_6789, 0xABCD_EF01, 0xBEEF_DEAD, 0xC0DE_C0DE);
        bus.write128(0x0010_0100, pattern);

        // Set base register $4 = 0x0010_0107 (unaligned low 4 bits)
        cpu.set_gpr_sext32(4, 0x0010_0107);
        cpu.execute_lqc2(4, 1, 0, &mut bus);
        assert_eq!(bus.vu0.read_vf(1), pattern);

        // LQC2 into $vf0 must leave $vf0 untouched
        cpu.execute_lqc2(4, 0, 0, &mut bus);
        assert_eq!(bus.vu0.read_vf(0), VF00_VALUE);

        // SQC2 from $vf1 to unaligned $4 + 0x10
        cpu.execute_sqc2(4, 1, 16, &mut bus);
        assert_eq!(bus.read128(0x0010_0110), pattern);
    }
}
