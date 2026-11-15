//! Emotion Engine (MIPS R5900) Base ISA & COP0 Single-Step Interpreter.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeinstructions` & `https://psi-rockin.github.io/ps2tek/#eecop0instructions`):
//!   * Opcode encoding tables (`Opcode`, `SPECIAL`, `REGIMM`, `COP0`, `BC0`, `TLB/Exception`).
//!   * 3-operand `MULT rd, rs, rt` and `MULTU rd, rs, rt` write the 32-bit sign-extended low product
//!     to `GPR[rd]` (if `rd != 0`) in addition to updating `LO0` and `HI0`.
//!   * `LQ` and `SQ` mask the lowest 4 bits of the effective virtual address (`addr & !0xF`).
//! - `ps2autotests` (`tests/cpu/ee/{alu,branch,branchdelay,lsu,muldiv}.cpp` & `.expected`):
//!   * Scalar ALU/shift/load/store operations write to the lower 64 bits (`bits 0..=63`) of `GPR[rd]`,
//!     preserving the upper 64 bits (`bits 64..=127`).
//!   * 32-bit ALU and shift instructions (`ADD`, `ADDU`, `SUB`, `SUBU`, `ADDI`, `ADDIU`, `LUI`,
//!     `SLL`, `SRL`, `SRA`, `SLLV`, `SRLV`, `SRAV`) sign-extend bit 31 of their result into bits `32..=63`.
//!   * `DIV` by zero sets `HI0 = sext32(rs)` and `LO0 = 1` (if `rs as i32 < 0`) or `-1` (if `rs as i32 >= 0`),
//!     and `DIV i32::MIN, -1` sets `HI0 = 0`, `LO0 = sext32(i32::MIN)` without trapping (`muldiv.expected`).
//!   * `DIVU` by zero sets `HI0 = sext32(rs)` and `LO0 = -1` (`0xFFFF_FFFF_FFFF_FFFF`, `muldiv.expected`).
//!   * `LWR` sign-extends bit 31 into bits `32..=63` only when `(addr & 3) == 0`; for `(addr & 3) != 0`
//!     it replaces only the low `(4 - (addr & 3))` bytes, preserving bits `24..=63` (`lsu.expected`).

use crate::bus::{Bus, UnimplementedDmaTransfer};

use super::cop0::{cause_bits, cop0_reg, Level1ExceptionCode};
use super::state::EeCpuState;
use super::syscall::{EeHleState, SyscallError};

/// Writable bit mask for `COP0.Perf.PCCR` (`0x800F_FBFE`: bits `0`, `10`, and `20..=30` are hardwired `0`).
///
/// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_cop0/performance.cpp` & `performance.expected`).
pub const COP0_PCCR_WRITABLE_MASK: u32 = 0x800F_FBFE;

/// Result of executing a single instruction in HLE test-runner mode (`EeCpuState::step_hle`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HleStepResult {
    /// Instruction or HLE syscall completed normally; execution continues.
    Continue,
    /// Program exited cleanly via HLE `Exit` (`0x04`), `ExitThread` (`0x23`), or `ExitDeleteThread` (`0x24`).
    Exited(i32),
    /// Program executed an unimplemented syscall (`$v1` syscall number).
    UnimplementedSyscall {
        /// Faulting `SYSCALL` instruction address.
        pc: u32,
        /// Signed syscall number from `$v1`.
        syscall: i16,
    },
    /// Program fetched an instruction from an unimplemented ISA extension (`MMI`, `COP1`, or `COP2`).
    UnimplementedInstruction {
        /// Address of the unimplemented instruction.
        pc: u32,
        /// Raw 32-bit instruction word.
        instr: u32,
        /// Subsystem category (`"MMI (Task 4)"`, `"COP1 / FPU (Task 4)"`, or `"COP2 / VU0 Macro (Task 4)"`).
        category: &'static str,
    },
    /// Program started a DMA transfer on an unimplemented peripheral channel (`0..=7`).
    UnimplementedDmaTransfer(UnimplementedDmaTransfer),
}

/// Control-flow outcome of a single R5900 instruction execution.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum StepOutcome {
    /// Normal sequential instruction (`PC = NPC`, `NPC = NPC + 4`, `in_delay_slot = false`).
    Normal,
    /// Unconditional jump or taken conditional branch (`PC = NPC`, `NPC = target`, `in_delay_slot = true`).
    BranchTaken(u32),
    /// Taken non-likely COP1 branch (`BC1F` / `BC1T`): chains through delay-slot branches
    /// (`PC = instr_pc + 4`, `NPC = target`, `in_delay_slot = true`).
    ///
    /// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/branchdelay.cpp` & `branchdelay.expected` line 2).
    /// TODO(verify): Confirm whether real hardware reaches `target4` due to non-likely `BC1F`/`BC1T`
    /// delay-slot chaining or a 2-instruction `C.<cond>.S -> BC1` condition-bit pipeline hazard.
    Cop1BranchTaken(u32),
    /// Non-taken normal conditional branch (`PC = NPC`, `NPC = NPC + 4`, `in_delay_slot = true`).
    BranchNotTaken,
    /// Non-taken Branch Likely (`PC = NPC + 4`, `NPC = NPC + 8`, `in_delay_slot = false`, nullifying the delay slot).
    BranchLikelyNullified,
    /// Exception raised or `ERET` executed (`PC`, `NPC`, and `in_delay_slot` already updated).
    ExceptionOrEret,
}

impl EeCpuState {
    /// Execute a single R5900 instruction at `self.pc`, advancing `COP0.Count` and EE hardware
    /// timers by 1 EE cycle.
    pub fn step(&mut self, bus: &mut Bus) {
        // 1. Check for pending, deliverable hardware interrupts (`INT0`, `INT1`, `INT5`).
        if self.check_and_service_interrupts(bus) {
            self.advance_cop0_count(1);
            bus.step_timers(1);
            self.cycle_countdown -= 1;
            return;
        }

        // 2. Instruction fetch alignment check (`PC` must be 4-byte aligned, `ps2tek` `#eecop0`).
        let instr_pc = self.pc;
        if (instr_pc & 3) != 0 {
            self.raise_address_error(instr_pc, false);
            self.advance_cop0_count(1);
            bus.step_timers(1);
            self.cycle_countdown -= 1;
            return;
        }

        let default_after = self.next_pc;
        let was_in_delay_slot = self.in_delay_slot;
        let instr = bus.read32(instr_pc);

        // 3. Decode and execute instruction while `self.pc == instr_pc` and `self.in_delay_slot == was_in_delay_slot`
        //    so any synchronous exception records the exact faulting `EPC` and `Cause.BD`.
        let outcome = self.execute_instruction(instr, instr_pc, was_in_delay_slot, bus);

        // 4. Commit control-flow state transition.
        match outcome {
            StepOutcome::Normal => {
                self.pc = default_after;
                self.next_pc = default_after.wrapping_add(4);
                self.in_delay_slot = false;
            }
            StepOutcome::BranchTaken(target) => {
                if was_in_delay_slot {
                    // Branch inside a branch delay slot (`ps2autotests` `tests/cpu/ee/branchdelay.cpp` `test_beq`
                    // and `tests/cpu/ee_fpu/branchdelay.cpp` `test_bc1tl`):
                    // overwrites the pending branch target and jumps to `target` immediately after this delay slot.
                    self.pc = target;
                    self.next_pc = target.wrapping_add(4);
                    self.in_delay_slot = false;
                } else {
                    self.pc = default_after;
                    self.next_pc = target;
                    self.in_delay_slot = true;
                }
            }
            StepOutcome::Cop1BranchTaken(target) => {
                self.pc = instr_pc.wrapping_add(4);
                self.next_pc = target;
                self.in_delay_slot = true;
            }
            StepOutcome::BranchNotTaken => {
                self.pc = default_after;
                self.next_pc = default_after.wrapping_add(4);
                self.in_delay_slot = true;
            }
            StepOutcome::BranchLikelyNullified => {
                // Skip the delay-slot instruction at `default_after` (`ps2autotests` `tests/cpu/ee/branch.expected`).
                let skipped = default_after.wrapping_add(4);
                self.pc = skipped;
                self.next_pc = skipped.wrapping_add(4);
                self.in_delay_slot = false;
            }
            StepOutcome::ExceptionOrEret => {
                // `self.pc`, `self.next_pc`, and `self.in_delay_slot` were set by `raise_level1_exception` or `execute_eret`.
            }
        }

        // 5. Advance COP0 `Count` and EE hardware timers by 1 EE cycle.
        self.advance_cop0_count(1);
        bus.step_timers(1);
        self.cycle_countdown -= 1;
    }

    /// Execute a single R5900 instruction at `self.pc` with HLE kernel syscall interception
    /// and explicit unimplemented-ISA detection (`MMI`, `COP1`, `COP2`) for `ps2-autotest`.
    pub fn step_hle(&mut self, bus: &mut Bus, hle: &mut EeHleState) -> HleStepResult {
        if hle.exited {
            return HleStepResult::Exited(hle.exit_code);
        }

        if self.check_and_service_interrupts(bus) {
            self.advance_cop0_count(1);
            bus.step_timers(1);
            self.cycle_countdown -= 1;
            return HleStepResult::Continue;
        }

        let instr_pc = self.pc;
        if (instr_pc & 3) != 0 {
            self.raise_address_error(instr_pc, false);
            self.advance_cop0_count(1);
            bus.step_timers(1);
            self.cycle_countdown -= 1;
            return HleStepResult::Continue;
        }

        let default_after = self.next_pc;
        let instr = bus.read32(instr_pc);
        let opcode = instr >> 26;
        let funct = instr & 0x3F;

        // Intercept `SYSCALL` (`opcode == 0x00, funct == 0x0C`) in HLE runner mode.
        if opcode == 0x00 && funct == 0x0C {
            match hle.handle_syscall(self, bus, default_after) {
                Ok(Some(redirect_pc)) => {
                    self.pc = redirect_pc;
                    self.next_pc = redirect_pc.wrapping_add(4);
                    self.in_delay_slot = false;
                }
                Ok(None) => {
                    self.pc = default_after;
                    self.next_pc = default_after.wrapping_add(4);
                    self.in_delay_slot = false;
                }
                Err(SyscallError::Unimplemented(syscall)) => {
                    return HleStepResult::UnimplementedSyscall {
                        pc: instr_pc,
                        syscall,
                    };
                }
            }
            self.advance_cop0_count(1);
            bus.step_timers(1);
            self.cycle_countdown -= 1;
            if hle.exited {
                return HleStepResult::Exited(hle.exit_code);
            }
            return HleStepResult::Continue;
        }

        self.step(bus);
        if let Some(info) = bus.dmac.unimplemented_transfer.take() {
            return HleStepResult::UnimplementedDmaTransfer(info);
        }
        HleStepResult::Continue
    }

    /// Decode and execute `instr` fetched from `instr_pc` using a plain `match` on `opcode` and `funct`.
    fn execute_instruction(
        &mut self,
        instr: u32,
        instr_pc: u32,
        was_in_delay_slot: bool,
        bus: &mut Bus,
    ) -> StepOutcome {
        let opcode = instr >> 26;
        let rs = ((instr >> 21) & 0x1F) as usize;
        let rt = ((instr >> 16) & 0x1F) as usize;
        let rd = ((instr >> 11) & 0x1F) as usize;
        let shamt = (instr >> 6) & 0x1F;
        let funct = instr & 0x3F;
        let imm16 = (instr & 0xFFFF) as u16;
        let simm16 = (imm16 as i16) as i32;
        let simm64 = i64::from(simm16);

        match opcode {
            // SPECIAL (0x00)
            0x00 => self.execute_special(instr, instr_pc, rs, rt, rd, shamt, funct),

            // REGIMM (0x01)
            0x01 => self.execute_regimm(instr_pc, rs, rt, simm16),

            // J (0x02)
            0x02 => {
                let target =
                    (instr_pc.wrapping_add(4) & 0xF000_0000) | ((instr & 0x03FF_FFFF) << 2);
                StepOutcome::BranchTaken(target)
            }

            // JAL (0x03)
            0x03 => {
                let target =
                    (instr_pc.wrapping_add(4) & 0xF000_0000) | ((instr & 0x03FF_FFFF) << 2);
                let link = instr_pc.wrapping_add(8) as i32;
                self.set_gpr_sext32(31, link);
                StepOutcome::BranchTaken(target)
            }

            // BEQ (0x04)
            0x04 => {
                if self.gpr_u64(rs) == self.gpr_u64(rt) {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BNE (0x05)
            0x05 => {
                if self.gpr_u64(rs) != self.gpr_u64(rt) {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BLEZ (0x06)
            0x06 => {
                if self.gpr_i64(rs) <= 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BGTZ (0x07)
            0x07 => {
                if self.gpr_i64(rs) > 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // ADDI (0x08)
            0x08 => match self.gpr_i32(rs).checked_add(simm16) {
                Some(res) => {
                    self.set_gpr_sext32(rt, res);
                    StepOutcome::Normal
                }
                None => {
                    self.raise_level1_exception(Level1ExceptionCode::Overflow, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            // ADDIU (0x09)
            0x09 => {
                let res = self.gpr_i32(rs).wrapping_add(simm16);
                self.set_gpr_sext32(rt, res);
                StepOutcome::Normal
            }

            // SLTI (0x0A)
            0x0A => {
                let cond = u64::from(self.gpr_i64(rs) < simm64);
                self.set_gpr_u64(rt, cond);
                StepOutcome::Normal
            }

            // SLTIU (0x0B)
            0x0B => {
                let cond = u64::from(self.gpr_u64(rs) < (simm64 as u64));
                self.set_gpr_u64(rt, cond);
                StepOutcome::Normal
            }

            // ANDI (0x0C)
            0x0C => {
                let res = self.gpr_u64(rs) & u64::from(imm16);
                self.set_gpr_u64(rt, res);
                StepOutcome::Normal
            }

            // ORI (0x0D)
            0x0D => {
                let res = self.gpr_u64(rs) | u64::from(imm16);
                self.set_gpr_u64(rt, res);
                StepOutcome::Normal
            }

            // XORI (0x0E)
            0x0E => {
                let res = self.gpr_u64(rs) ^ u64::from(imm16);
                self.set_gpr_u64(rt, res);
                StepOutcome::Normal
            }

            // LUI (0x0F)
            0x0F => {
                let res = (u32::from(imm16) << 16) as i32;
                self.set_gpr_sext32(rt, res);
                StepOutcome::Normal
            }

            // COP0 (0x10)
            0x10 => self.execute_cop0(instr, instr_pc, rs, rt, rd, funct, simm16, bus),

            // COP1 (0x11)
            0x11 => self.execute_cop1(instr, instr_pc, simm16),

            // COP2 (0x12)
            0x12 => self.execute_cop2(instr, instr_pc, simm16, bus),

            // BEQL (0x14)
            0x14 => {
                if self.gpr_u64(rs) == self.gpr_u64(rt) {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // BNEL (0x15)
            0x15 => {
                if self.gpr_u64(rs) != self.gpr_u64(rt) {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // BLEZL (0x16)
            0x16 => {
                if self.gpr_i64(rs) <= 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // BGTZL (0x17)
            0x17 => {
                if self.gpr_i64(rs) > 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // DADDI (0x18)
            0x18 => match self.gpr_i64(rs).checked_add(simm64) {
                Some(res) => {
                    self.set_gpr_u64(rt, res as u64);
                    StepOutcome::Normal
                }
                None => {
                    self.raise_level1_exception(Level1ExceptionCode::Overflow, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            // DADDIU (0x19)
            0x19 => {
                let res = self.gpr_u64(rs).wrapping_add(simm64 as u64);
                self.set_gpr_u64(rt, res);
                StepOutcome::Normal
            }

            // LDL (0x1A)
            0x1A => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 29..42).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let aligned = vaddr & !7;
                let mem_d = bus.read64(aligned);
                let shift = (7 - (vaddr & 7)) * 8;
                let mask = if shift == 0 { 0 } else { (1u64 << shift) - 1 };
                let merged = (self.gpr_u64(rt) & mask) | (mem_d << shift);
                self.set_gpr_u64(rt, merged);
                StepOutcome::Normal
            }

            // LDR (0x1B)
            0x1B => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 29..42).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let aligned = vaddr & !7;
                let mem_d = bus.read64(aligned);
                let shift = (vaddr & 7) * 8;
                let mask = if shift == 0 {
                    0
                } else {
                    !((1u64 << (64 - shift)) - 1)
                };
                let merged = (self.gpr_u64(rt) & mask) | (mem_d >> shift);
                self.set_gpr_u64(rt, merged);
                StepOutcome::Normal
            }

            // MMI (0x1C)
            0x1C => self.execute_mmi(instr, instr_pc, rs, rt, rd, shamt, funct),

            // LQ (0x1E)
            0x1E => {
                // Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeinstructions`)
                // & `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 93..102):
                // `LQ` masks the lowest 4 bits of the effective address (`vaddr & !0xF`).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let qword = bus.read128(vaddr);
                self.set_gpr_128(rt, qword);
                StepOutcome::Normal
            }

            // SQ (0x1F)
            0x1F => {
                // Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeinstructions`)
                // & `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 160..169):
                // `SQ` masks the lowest 4 bits of the effective address (`vaddr & !0xF`).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let qword = self.gpr_128(rt);
                bus.write128(vaddr, qword);
                StepOutcome::Normal
            }

            // LB (0x20)
            0x20 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let val = (bus.read8(vaddr) as i8) as i32;
                self.set_gpr_sext32(rt, val);
                StepOutcome::Normal
            }

            // LH (0x21)
            0x21 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 1) != 0 {
                    self.raise_address_error(vaddr, false);
                    return StepOutcome::ExceptionOrEret;
                }
                let val = (bus.read16(vaddr) as i16) as i32;
                self.set_gpr_sext32(rt, val);
                StepOutcome::Normal
            }

            // LWL (0x22)
            0x22 => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 70..82):
                // `LWL` merges into the upper bytes of the low 32-bit word and always sign-extends
                // bit 31 into bits 32..=63.
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let mem_w = bus.read32(vaddr & !3);
                let shift = (3 - (vaddr & 3)) * 8;
                let mask = if shift == 0 { 0 } else { (1u32 << shift) - 1 };
                let merged32 = (self.gpr_u32(rt) & mask) | (mem_w << shift);
                self.set_gpr_sext32(rt, merged32 as i32);
                StepOutcome::Normal
            }

            // LW (0x23)
            0x23 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 3) != 0 {
                    self.raise_address_error(vaddr, false);
                    return StepOutcome::ExceptionOrEret;
                }
                let val = bus.read32(vaddr) as i32;
                self.set_gpr_sext32(rt, val);
                StepOutcome::Normal
            }

            // LBU (0x24)
            0x24 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let val = u64::from(bus.read8(vaddr));
                self.set_gpr_u64(rt, val);
                StepOutcome::Normal
            }

            // LHU (0x25)
            0x25 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 1) != 0 {
                    self.raise_address_error(vaddr, false);
                    return StepOutcome::ExceptionOrEret;
                }
                let val = u64::from(bus.read16(vaddr));
                self.set_gpr_u64(rt, val);
                StepOutcome::Normal
            }

            // LWR (0x26)
            0x26 => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 70..82):
                // When `(vaddr & 3) == 0`, `LWR` loads a full 32-bit word and sign-extends bit 31
                // into bits 32..=63. When `(vaddr & 3) != 0`, `LWR` only modifies the low
                // `(4 - (vaddr & 3))` bytes, preserving bits 24..=63 of `GPR[rt]`.
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let byte_off = vaddr & 3;
                let mem_w = bus.read32(vaddr & !3);
                if byte_off == 0 {
                    self.set_gpr_sext32(rt, mem_w as i32);
                } else {
                    let shift = byte_off * 8;
                    let keep_mask = !((1u64 << (32 - shift)) - 1);
                    let loaded = u64::from(mem_w >> shift);
                    let merged = (self.gpr_u64(rt) & keep_mask) | loaded;
                    self.set_gpr_u64(rt, merged);
                }
                StepOutcome::Normal
            }

            // LWU (0x27)
            0x27 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 3) != 0 {
                    self.raise_address_error(vaddr, false);
                    return StepOutcome::ExceptionOrEret;
                }
                let val = u64::from(bus.read32(vaddr));
                self.set_gpr_u64(rt, val);
                StepOutcome::Normal
            }

            // SB (0x28)
            0x28 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                bus.write8(vaddr, (self.gpr_u32(rt) & 0xFF) as u8);
                StepOutcome::Normal
            }

            // SH (0x29)
            0x29 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 1) != 0 {
                    self.raise_address_error(vaddr, true);
                    return StepOutcome::ExceptionOrEret;
                }
                bus.write16(vaddr, (self.gpr_u32(rt) & 0xFFFF) as u16);
                StepOutcome::Normal
            }

            // SWL (0x2A)
            0x2A => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 148..159).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let aligned = vaddr & !3;
                let old_mem = bus.read32(aligned);
                let shift = (3 - (vaddr & 3)) * 8;
                let mask = if shift == 0 {
                    0
                } else {
                    !((1u32 << (32 - shift)) - 1)
                };
                let new_mem = (old_mem & mask) | (self.gpr_u32(rt) >> shift);
                bus.write32(aligned, new_mem);
                StepOutcome::Normal
            }

            // SW (0x2B)
            0x2B => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 3) != 0 {
                    self.raise_address_error(vaddr, true);
                    return StepOutcome::ExceptionOrEret;
                }
                bus.write32(vaddr, self.gpr_u32(rt));
                StepOutcome::Normal
            }

            // SDL (0x2C)
            0x2C => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 120..131).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let aligned = vaddr & !7;
                let old_mem = bus.read64(aligned);
                let shift = (7 - (vaddr & 7)) * 8;
                let mask = if shift == 0 {
                    0
                } else {
                    !((1u64 << (64 - shift)) - 1)
                };
                let new_mem = (old_mem & mask) | (self.gpr_u64(rt) >> shift);
                bus.write64(aligned, new_mem);
                StepOutcome::Normal
            }

            // SDR (0x2D)
            0x2D => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 120..131).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let aligned = vaddr & !7;
                let old_mem = bus.read64(aligned);
                let shift = (vaddr & 7) * 8;
                let mask = if shift == 0 { 0 } else { (1u64 << shift) - 1 };
                let new_mem = (old_mem & mask) | (self.gpr_u64(rt) << shift);
                bus.write64(aligned, new_mem);
                StepOutcome::Normal
            }

            // SWR (0x2E)
            0x2E => {
                // Reference: `ps2autotests` (`tests/cpu/ee/lsu.expected` lines 148..159).
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                let aligned = vaddr & !3;
                let old_mem = bus.read32(aligned);
                let shift = (vaddr & 3) * 8;
                let mask = if shift == 0 { 0 } else { (1u32 << shift) - 1 };
                let new_mem = (old_mem & mask) | (self.gpr_u32(rt) << shift);
                bus.write32(aligned, new_mem);
                StepOutcome::Normal
            }

            // CACHE (0x2F) & PREF (0x33)
            0x2F | 0x33 => {
                // Architecturally no-op in Phase 1 software vtlb (`ps2autotests` `lsu.expected` line 170).
                StepOutcome::Normal
            }

            // LWC1 (0x31)
            0x31 => self.execute_lwc1(rs, rt, simm16, bus),

            // LQC2 (0x36)
            0x36 => self.execute_lqc2(rs, rt, simm16, bus),

            // LD (0x37)
            0x37 => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 7) != 0 {
                    self.raise_address_error(vaddr, false);
                    return StepOutcome::ExceptionOrEret;
                }
                let val = bus.read64(vaddr);
                self.set_gpr_u64(rt, val);
                StepOutcome::Normal
            }

            // SWC1 (0x39)
            0x39 => self.execute_swc1(rs, rt, simm16, bus),

            // SQC2 (0x3E)
            0x3E => self.execute_sqc2(rs, rt, simm16, bus),

            // SD (0x3F)
            0x3F => {
                let vaddr = self.gpr_u32(rs).wrapping_add(simm16 as u32);
                if (vaddr & 7) != 0 {
                    self.raise_address_error(vaddr, true);
                    return StepOutcome::ExceptionOrEret;
                }
                bus.write64(vaddr, self.gpr_u64(rt));
                StepOutcome::Normal
            }

            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    opcode,
                    was_in_delay_slot,
                    "Reserved/unimplemented primary opcode in Task 3a"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }

    /// Execute a `SPECIAL` (`opcode == 0x00`) instruction decoded by `funct` (`ps2tek` `#eeinstructions`).
    #[allow(clippy::too_many_arguments)]
    fn execute_special(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        shamt: u32,
        funct: u32,
    ) -> StepOutcome {
        match funct {
            // SLL (0x00)
            0x00 => {
                let res = (self.gpr_u32(rt) << shamt) as i32;
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // SRL (0x02)
            0x02 => {
                let res = (self.gpr_u32(rt) >> shamt) as i32;
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // SRA (0x03)
            0x03 => {
                let res = self.gpr_i32(rt) >> shamt;
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // SLLV (0x04)
            0x04 => {
                let s = self.gpr_u32(rs) & 0x1F;
                let res = (self.gpr_u32(rt) << s) as i32;
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // SRLV (0x06)
            0x06 => {
                let s = self.gpr_u32(rs) & 0x1F;
                let res = (self.gpr_u32(rt) >> s) as i32;
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // SRAV (0x07)
            0x07 => {
                let s = self.gpr_u32(rs) & 0x1F;
                let res = self.gpr_i32(rt) >> s;
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // JR (0x08)
            0x08 => {
                let target = self.gpr_u32(rs);
                StepOutcome::BranchTaken(target)
            }

            // JALR (0x09)
            0x09 => {
                // Read `rs` BEFORE writing link address to `rd` (`ps2autotests` `branchdelay.cpp` `jalr: rs/rd match`).
                let target = self.gpr_u32(rs);
                let link = instr_pc.wrapping_add(8) as i32;
                self.set_gpr_sext32(rd, link);
                StepOutcome::BranchTaken(target)
            }

            // MOVZ (0x0A)
            0x0A => {
                if self.gpr_u64(rt) == 0 {
                    let val = self.gpr_u64(rs);
                    self.set_gpr_u64(rd, val);
                }
                StepOutcome::Normal
            }

            // MOVN (0x0B)
            0x0B => {
                if self.gpr_u64(rt) != 0 {
                    let val = self.gpr_u64(rs);
                    self.set_gpr_u64(rd, val);
                }
                StepOutcome::Normal
            }

            // SYSCALL (0x0C)
            0x0C => {
                self.raise_level1_exception(Level1ExceptionCode::Syscall, false);
                StepOutcome::ExceptionOrEret
            }

            // BREAK (0x0D)
            0x0D => {
                self.raise_level1_exception(Level1ExceptionCode::Breakpoint, false);
                StepOutcome::ExceptionOrEret
            }

            // SYNC (0x0F)
            0x0F => StepOutcome::Normal,

            // MFHI (0x10)
            0x10 => {
                let val = self.hi.lo_u64();
                self.set_gpr_u64(rd, val);
                StepOutcome::Normal
            }

            // MTHI (0x11)
            0x11 => {
                let val = self.gpr_u64(rs);
                self.hi.set_lo_u64(val);
                StepOutcome::Normal
            }

            // MFLO (0x12)
            0x12 => {
                let val = self.lo.lo_u64();
                self.set_gpr_u64(rd, val);
                StepOutcome::Normal
            }

            // MTLO (0x13)
            0x13 => {
                let val = self.gpr_u64(rs);
                self.lo.set_lo_u64(val);
                StepOutcome::Normal
            }

            // DSLLV (0x14)
            0x14 => {
                let s = self.gpr_u32(rs) & 0x3F;
                let res = self.gpr_u64(rt) << s;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSRLV (0x16)
            0x16 => {
                let s = self.gpr_u32(rs) & 0x3F;
                let res = self.gpr_u64(rt) >> s;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSRAV (0x17)
            0x17 => {
                let s = self.gpr_u32(rs) & 0x3F;
                let res = (self.gpr_i64(rt) >> s) as u64;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // MULT (0x18): 3-operand R5900 signed multiply (`MULT rd, rs, rt`).
            // Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeinstructions`)
            // & `ps2autotests` (`tests/cpu/ee/muldiv.expected` lines 57..114).
            0x18 => {
                let prod = i64::from(self.gpr_i32(rs)) * i64::from(self.gpr_i32(rt));
                let lo32 = prod as i32;
                let hi32 = (prod >> 32) as i32;
                self.lo.set_sext32(lo32);
                self.hi.set_sext32(hi32);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // MULTU (0x19): 3-operand R5900 unsigned multiply (`MULTU rd, rs, rt`).
            // Reference: `ps2autotests` (`tests/cpu/ee/muldiv.expected` lines 115..172).
            0x19 => {
                let prod = u64::from(self.gpr_u32(rs)) * u64::from(self.gpr_u32(rt));
                let lo32 = prod as i32;
                let hi32 = (prod >> 32) as i32;
                self.lo.set_sext32(lo32);
                self.hi.set_sext32(hi32);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // DIV (0x1A): R5900 signed 32-bit divide (`LO0 = quotient`, `HI0 = remainder`).
            // Reference: `ps2autotests` (`tests/cpu/ee/muldiv.expected` lines 1..28):
            // - Divide by zero: `HI0 = sext32(rs)`, `LO0 = 1` if `rs < 0` else `-1`.
            // - `i32::MIN / -1`: `HI0 = 0`, `LO0 = sext32(i32::MIN)` (`0xFFFF_FFFF_8000_0000`).
            0x1A => {
                let n = self.gpr_i32(rs);
                let d = self.gpr_i32(rt);
                if d == 0 {
                    self.hi.set_sext32(n);
                    self.lo.set_sext32(if n < 0 { 1 } else { -1 });
                } else if n == i32::MIN && d == -1 {
                    self.hi.set_sext32(0);
                    self.lo.set_sext32(i32::MIN);
                } else {
                    self.lo.set_sext32(n / d);
                    self.hi.set_sext32(n % d);
                }
                StepOutcome::Normal
            }

            // DIVU (0x1B): R5900 unsigned 32-bit divide (`LO0 = quotient`, `HI0 = remainder`).
            // Reference: `ps2autotests` (`tests/cpu/ee/muldiv.expected` lines 29..56):
            // - Divide by zero: `HI0 = sext32(rs)`, `LO0 = -1` (`0xFFFF_FFFF_FFFF_FFFF`).
            0x1B => {
                let n = self.gpr_u32(rs);
                let d = self.gpr_u32(rt);
                if let Some(q) = n.checked_div(d) {
                    self.lo.set_sext32(q as i32);
                    self.hi.set_sext32((n % d) as i32);
                } else {
                    self.hi.set_sext32(n as i32);
                    self.lo.set_sext32(-1);
                }
                StepOutcome::Normal
            }

            // ADD (0x20)
            0x20 => match self.gpr_i32(rs).checked_add(self.gpr_i32(rt)) {
                Some(res) => {
                    self.set_gpr_sext32(rd, res);
                    StepOutcome::Normal
                }
                None => {
                    self.raise_level1_exception(Level1ExceptionCode::Overflow, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            // ADDU (0x21)
            0x21 => {
                let res = self.gpr_i32(rs).wrapping_add(self.gpr_i32(rt));
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // SUB (0x22)
            0x22 => match self.gpr_i32(rs).checked_sub(self.gpr_i32(rt)) {
                Some(res) => {
                    self.set_gpr_sext32(rd, res);
                    StepOutcome::Normal
                }
                None => {
                    self.raise_level1_exception(Level1ExceptionCode::Overflow, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            // SUBU (0x23)
            0x23 => {
                let res = self.gpr_i32(rs).wrapping_sub(self.gpr_i32(rt));
                self.set_gpr_sext32(rd, res);
                StepOutcome::Normal
            }

            // AND (0x24)
            0x24 => {
                let res = self.gpr_u64(rs) & self.gpr_u64(rt);
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // OR (0x25)
            0x25 => {
                let res = self.gpr_u64(rs) | self.gpr_u64(rt);
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // XOR (0x26)
            0x26 => {
                let res = self.gpr_u64(rs) ^ self.gpr_u64(rt);
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // NOR (0x27)
            0x27 => {
                let res = !(self.gpr_u64(rs) | self.gpr_u64(rt));
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // MFSA (0x28)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 16..23):
            // `MFSA` and `MTSA` expose `SA` as a 4-bit byte shift count (`0..=15`).
            0x28 => {
                self.set_gpr_u64(rd, u64::from(self.sa & 0xF));
                StepOutcome::Normal
            }

            // MTSA (0x29)
            0x29 => {
                self.sa = self.gpr_u32(rs) & 0xF;
                StepOutcome::Normal
            }

            // SLT (0x2A)
            0x2A => {
                let cond = u64::from(self.gpr_i64(rs) < self.gpr_i64(rt));
                self.set_gpr_u64(rd, cond);
                StepOutcome::Normal
            }

            // SLTU (0x2B)
            0x2B => {
                let cond = u64::from(self.gpr_u64(rs) < self.gpr_u64(rt));
                self.set_gpr_u64(rd, cond);
                StepOutcome::Normal
            }

            // DADD (0x2C)
            0x2C => match self.gpr_i64(rs).checked_add(self.gpr_i64(rt)) {
                Some(res) => {
                    self.set_gpr_u64(rd, res as u64);
                    StepOutcome::Normal
                }
                None => {
                    self.raise_level1_exception(Level1ExceptionCode::Overflow, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            // DADDU (0x2D)
            0x2D => {
                let res = self.gpr_u64(rs).wrapping_add(self.gpr_u64(rt));
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSUB (0x2E)
            0x2E => match self.gpr_i64(rs).checked_sub(self.gpr_i64(rt)) {
                Some(res) => {
                    self.set_gpr_u64(rd, res as u64);
                    StepOutcome::Normal
                }
                None => {
                    self.raise_level1_exception(Level1ExceptionCode::Overflow, false);
                    StepOutcome::ExceptionOrEret
                }
            },

            // DSUBU (0x2F)
            0x2F => {
                let res = self.gpr_u64(rs).wrapping_sub(self.gpr_u64(rt));
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // TGE (0x30)
            0x30 => {
                if self.gpr_i64(rs) >= self.gpr_i64(rt) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TGEU (0x31)
            0x31 => {
                if self.gpr_u64(rs) >= self.gpr_u64(rt) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TLT (0x32)
            0x32 => {
                if self.gpr_i64(rs) < self.gpr_i64(rt) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TLTU (0x33)
            0x33 => {
                if self.gpr_u64(rs) < self.gpr_u64(rt) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TEQ (0x34)
            0x34 => {
                if self.gpr_u64(rs) == self.gpr_u64(rt) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TNE (0x36)
            0x36 => {
                if self.gpr_u64(rs) != self.gpr_u64(rt) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // DSLL (0x38)
            0x38 => {
                let res = self.gpr_u64(rt) << shamt;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSRL (0x3A)
            0x3A => {
                let res = self.gpr_u64(rt) >> shamt;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSRA (0x3B)
            0x3B => {
                let res = (self.gpr_i64(rt) >> shamt) as u64;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSLL32 (0x3C)
            0x3C => {
                let res = self.gpr_u64(rt) << (shamt + 32);
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSRL32 (0x3E)
            0x3E => {
                let res = self.gpr_u64(rt) >> (shamt + 32);
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // DSRA32 (0x3F)
            0x3F => {
                let res = (self.gpr_i64(rt) >> (shamt + 32)) as u64;
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    funct,
                    "Reserved SPECIAL funct"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }

    /// Execute a `REGIMM` (`opcode == 0x01`) instruction decoded by `rt` (`bits 20..=16`, `ps2tek` `#eeinstructions`).
    fn execute_regimm(&mut self, instr_pc: u32, rs: usize, rt: usize, simm16: i32) -> StepOutcome {
        let simm64 = i64::from(simm16);
        match rt {
            // BLTZ (0x00)
            0x00 => {
                if self.gpr_i64(rs) < 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BGEZ (0x01)
            0x01 => {
                if self.gpr_i64(rs) >= 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BLTZL (0x02)
            0x02 => {
                if self.gpr_i64(rs) < 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // BGEZL (0x03)
            0x03 => {
                if self.gpr_i64(rs) >= 0 {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // TGEI (0x08)
            0x08 => {
                if self.gpr_i64(rs) >= simm64 {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TGEIU (0x09)
            0x09 => {
                if self.gpr_u64(rs) >= (simm64 as u64) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TLTI (0x0A)
            0x0A => {
                if self.gpr_i64(rs) < simm64 {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TLTIU (0x0B)
            0x0B => {
                if self.gpr_u64(rs) < (simm64 as u64) {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TEQI (0x0C)
            0x0C => {
                if self.gpr_i64(rs) == simm64 {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // TNEI (0x0E)
            0x0E => {
                if self.gpr_i64(rs) != simm64 {
                    self.raise_level1_exception(Level1ExceptionCode::Trap, false);
                    StepOutcome::ExceptionOrEret
                } else {
                    StepOutcome::Normal
                }
            }

            // BLTZAL (0x10)
            0x10 => {
                let cond = self.gpr_i64(rs) < 0;
                let link = instr_pc.wrapping_add(8) as i32;
                self.set_gpr_sext32(31, link);
                if cond {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BGEZAL (0x11)
            0x11 => {
                let cond = self.gpr_i64(rs) >= 0;
                let link = instr_pc.wrapping_add(8) as i32;
                self.set_gpr_sext32(31, link);
                if cond {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchNotTaken
                }
            }

            // BLTZALL (0x12)
            0x12 => {
                let cond = self.gpr_i64(rs) < 0;
                let link = instr_pc.wrapping_add(8) as i32;
                self.set_gpr_sext32(31, link);
                if cond {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // BGEZALL (0x13)
            0x13 => {
                let cond = self.gpr_i64(rs) >= 0;
                let link = instr_pc.wrapping_add(8) as i32;
                self.set_gpr_sext32(31, link);
                if cond {
                    StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                } else {
                    StepOutcome::BranchLikelyNullified
                }
            }

            // MTSAB (0x18)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 2..14):
            // `MTSAB` sets `SA = (rs ^ imm) & 0xF` (bytes), `MTSAH` sets `SA = ((rs ^ imm) & 0x7) << 1` (bytes).
            0x18 => {
                self.sa = (self.gpr_u32(rs) ^ (simm16 as u32)) & 0xF;
                StepOutcome::Normal
            }

            // MTSAH (0x19)
            0x19 => {
                self.sa = ((self.gpr_u32(rs) ^ (simm16 as u32)) & 0x7) << 1;
                StepOutcome::Normal
            }

            _ => {
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }

    /// Execute a `COP0` (`opcode == 0x10`) instruction (`ps2tek` `#eecop0instructions`).
    #[allow(clippy::too_many_arguments)]
    fn execute_cop0(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        funct: u32,
        simm16: i32,
        bus: &mut Bus,
    ) -> StepOutcome {
        // Check COP0 usability (`Status.CU0` bit 28 or Kernel Mode, `ps2tek` `#eecop0`).
        let status = self.cop0_regs[cop0_reg::STATUS];
        if !self.in_kernel_mode() && (status & super::cop0::status_bits::CU0) == 0 {
            self.cop0_regs[cop0_reg::CAUSE] &= !cause_bits::CE_MASK;
            self.raise_level1_exception(Level1ExceptionCode::CoprocessorUnusable, false);
            return StepOutcome::ExceptionOrEret;
        }

        match rs {
            // MFC0 / MFPS / MFPC (rs == 0x00)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_cop0/performance.cpp`,
            // `performance.expected`, and `performance.elf` disassembly):
            // - When `rd == 25` (`COP0.Perf`):
            //   * `(instr & 1) == 0` is `MFPS rt, reg` (`reg = (instr >> 1) & 0x1F`): reads `PCCR`
            //     for any specifier `reg` (`0..=31`).
            //   * `(instr & 1) == 1` is `MFPC rt, reg` (`reg = (instr >> 1) & 0x1F`): even `reg`
            //     reads `PCR0`, odd `reg` reads `PCR1`.
            0x00 => {
                if rd == cop0_reg::PERF {
                    let is_pcr = (instr & 1) != 0;
                    let reg_spec = ((instr >> 1) & 0x1F) as usize;
                    let val = if is_pcr {
                        self.cop0_pcr[reg_spec & 1] as i32
                    } else {
                        self.cop0_pccr as i32
                    };
                    self.set_gpr_sext32(rt, val);
                } else {
                    let val = self.read_cop0(rd, bus) as i32;
                    self.set_gpr_sext32(rt, val);
                }
                StepOutcome::Normal
            }

            // MTC0 / MTPS / MTPC (rs == 0x04)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_cop0/performance.cpp`,
            // `performance.expected`, and `performance.elf` disassembly):
            // - When `rd == 25` (`COP0.Perf`):
            //   * `(instr & 1) == 0` is `MTPS rt, reg` (`reg = (instr >> 1) & 0x1F`): writes
            //     `PCCR = val & 0x800F_FBFE` only when `reg == 0`; any `reg != 0` is a no-op.
            //   * `(instr & 1) == 1` is `MTPC rt, reg` (`reg = (instr >> 1) & 0x1F`): even `reg`
            //     writes `PCR0`, odd `reg` writes `PCR1`.
            0x04 => {
                let val = self.gpr_u32(rt);
                if rd == cop0_reg::PERF {
                    let is_pcr = (instr & 1) != 0;
                    let reg_spec = ((instr >> 1) & 0x1F) as usize;
                    if is_pcr {
                        self.cop0_pcr[reg_spec & 1] = val;
                    } else if reg_spec == 0 {
                        self.cop0_pccr = val & COP0_PCCR_WRITABLE_MASK;
                    }
                } else {
                    self.write_cop0(rd, val);
                }
                StepOutcome::Normal
            }

            // BC0 (rs == 0x08): BC0F (0), BC0T (1), BC0FL (2), BC0TL (3) (`ps2tek` `#eeinstructions`)
            0x08 => {
                let cond = self.cpcond0 && bus.dmac.cpcond0();
                match rt {
                    // BC0F
                    0x00 => {
                        if !cond {
                            StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchNotTaken
                        }
                    }
                    // BC0T
                    0x01 => {
                        if cond {
                            StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchNotTaken
                        }
                    }
                    // BC0FL
                    0x02 => {
                        if !cond {
                            StepOutcome::BranchTaken(branch_target(instr_pc, simm16))
                        } else {
                            StepOutcome::BranchLikelyNullified
                        }
                    }
                    // BC0TL
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

            // CO / TLB / Exception (rs == 0x10)
            0x10 => match funct {
                // TLBR (0x01)
                0x01 => {
                    self.execute_tlbr(bus);
                    StepOutcome::Normal
                }
                // TLBWI (0x02)
                0x02 => {
                    self.execute_tlbwi(bus);
                    StepOutcome::Normal
                }
                // TLBWR (0x06)
                0x06 => {
                    self.execute_tlbwr(bus);
                    StepOutcome::Normal
                }
                // TLBP (0x08)
                0x08 => {
                    self.execute_tlbp(bus);
                    StepOutcome::Normal
                }
                // ERET (0x18)
                0x18 => {
                    self.execute_eret();
                    StepOutcome::ExceptionOrEret
                }
                // EI (0x38)
                0x38 => {
                    self.execute_ei();
                    StepOutcome::Normal
                }
                // DI (0x39)
                0x39 => {
                    self.execute_di();
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
}

#[inline(always)]
pub(super) const fn branch_target(instr_pc: u32, simm16: i32) -> u32 {
    instr_pc
        .wrapping_add(4)
        .wrapping_add((simm16 as u32).wrapping_shl(2))
}

#[cfg(test)]
mod tests {
    use ps2_common::Reg128;

    use super::*;
    use crate::bus::timers::mode_bits;
    use crate::bus::IntcFlags;
    use crate::ee::cop0::status_bits;

    /// Helper to assemble R-type instructions (`opcode = 0`).
    const fn asm_special(rs: u32, rt: u32, rd: u32, shamt: u32, funct: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | (shamt << 6) | funct
    }

    /// Helper to assemble I-type instructions.
    const fn asm_itype(opcode: u32, rs: u32, rt: u32, imm16: u16) -> u32 {
        (opcode << 26) | (rs << 21) | (rt << 16) | (imm16 as u32)
    }

    /// Write a sequence of 32-bit instructions into `bus` starting at `base_vaddr` and set `cpu.pc`.
    fn load_program(cpu: &mut EeCpuState, bus: &mut Bus, base_vaddr: u32, code: &[u32]) {
        for (i, &word) in code.iter().enumerate() {
            bus.write32(base_vaddr.wrapping_add((i as u32) * 4), word);
        }
        cpu.pc = base_vaddr;
        cpu.next_pc = base_vaddr.wrapping_add(4);
        cpu.in_delay_slot = false;
    }

    #[test]
    fn ee_scalar_alu_32bit_sign_extension_and_upper_64bit_preservation() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Preload upper 64 bits of $t0 (reg 8) with sentinel 0x0000133A_00001339 (`ps2autotests` `alu.expected`).
        let upper_sentinel = 0x0000_133A_0000_1339u64;
        cpu.set_gpr_128(8, Reg128::from_u64_pair(0xDEAD_BEEF, upper_sentinel));

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // lui $t0, 0x8000 -> $t0[0..63] = 0xFFFFFFFF_80000000, $t0[64..127] preserved
                asm_itype(0x0F, 0, 8, 0x8000),
                // addiu $t1, $zero, -1 -> $t1[0..63] = 0xFFFFFFFF_FFFFFFFF
                asm_itype(0x09, 0, 9, 0xFFFF),
                // addu $t0, $t1, $t1 -> -1 + -1 = -2 (0xFFFFFFFF_FFFFFFFE), upper 64 bits of $t0 preserved
                asm_special(9, 9, 8, 0, 0x21),
                // ori $zero, $t1, 0x1234 -> writes to $zero must be ignored
                asm_itype(0x0D, 9, 0, 0x1234),
            ],
        );

        cpu.step(&mut bus);
        assert_eq!(
            cpu.gpr_128(8),
            Reg128::from_u64_pair(0xFFFF_FFFF_8000_0000, upper_sentinel)
        );

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(9), 0xFFFF_FFFF_FFFF_FFFF);

        cpu.step(&mut bus);
        assert_eq!(
            cpu.gpr_128(8),
            Reg128::from_u64_pair(0xFFFF_FFFF_FFFF_FFFE, upper_sentinel)
        );

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_128(0), Reg128::ZERO);
    }

    #[test]
    fn ee_scalar_alu_64bit_and_overflow_exceptions() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Clear Status.EXL and Status.BEV so exceptions vector to 0x8000_0180.
        cpu.cop0_regs[cop0_reg::STATUS] = status_bits::CU0;
        cpu.set_gpr_u64(1, 0x0000_0000_7FFF_FFFF); // i32::MAX
        cpu.set_gpr_u64(2, 1);

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // daddu $t0, $at, $v0 -> 64-bit add does NOT sign-extend bit 31 (`0x00000000_80000000`)
                asm_special(1, 2, 8, 0, 0x2D),
                // add $t1, $at, $v0 -> 32-bit signed overflow (0x7FFF_FFFF + 1) triggers Level 1 Ov (ExcCode = 12)
                asm_special(1, 2, 9, 0, 0x20),
            ],
        );

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(8), 0x0000_0000_8000_0000);

        cpu.step(&mut bus);
        assert_eq!(cpu.pc, 0x8000_0180);
        assert_eq!(cpu.cop0_regs[cop0_reg::EPC], 0x0010_0004);
        assert_eq!(
            (cpu.cop0_regs[cop0_reg::CAUSE] & cause_bits::EXCCODE_MASK) >> 2,
            Level1ExceptionCode::Overflow as u32
        );
    }

    #[test]
    fn ee_shifts_32bit_and_64bit_edge_cases() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Set $t0 (reg 8) = 0x00000000_FFFF8000 (`C_S16_MIN` in `ps2autotests` `alu.expected`).
        cpu.set_gpr_u64(8, 0xFFFF_FFFF_FFFF_8000);
        cpu.set_gpr_u64(9, 0xFFFF_8000); // shamt & 0x1F == 0

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // srlv $t2, $t0, $t1 -> shamt = 0, bit 31 of 0xFFFF8000 is 1, so sign-extends to 0xFFFFFFFF_FFFF8000 (`alu.expected` line 713)
                asm_special(9, 8, 10, 0, 0x06),
                // dsll32 $t3, $t0, 3 -> (0xFFFFFFFF_FFFF8000 << 35) = 0xFFFC0000_00000000 (`alu.expected` line 174)
                asm_special(0, 8, 11, 3, 0x3C),
                // dsra32 $t4, $t0, 3 -> (-32768 >> 35) = -1 (`0xFFFFFFFF_FFFFFFFF`, `alu.expected` line 244)
                asm_special(0, 8, 12, 3, 0x3F),
            ],
        );

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(10), 0xFFFF_FFFF_FFFF_8000);

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(11), 0xFFFC_0000_0000_0000);

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(12), 0xFFFF_FFFF_FFFF_FFFF);
    }

    #[test]
    fn ee_muldiv_three_operand_and_div_edge_cases() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Seed HI1 and LO1 (upper 64 bits of HI/LO) to verify `MULT`/`DIV` preserve them (`muldiv.expected`).
        let hi1_sentinel = 0x2345_6789_ABCD_EF01u64;
        let lo1_sentinel = 0x4567_89AB_CDEF_0123u64;
        cpu.hi = Reg128::from_u64_pair(0, hi1_sentinel);
        cpu.lo = Reg128::from_u64_pair(0, lo1_sentinel);

        cpu.set_gpr_sext32(1, 100_000);
        cpu.set_gpr_sext32(2, -300_000);
        cpu.set_gpr_sext32(3, i32::MIN);
        cpu.set_gpr_sext32(4, -1);

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // mult $t0, $1, $2 -> 3-operand MULT writes low 32-bit sign-extended product to $t0 as well as LO0/HI0
                asm_special(1, 2, 8, 0, 0x18),
                // div $zero, $1, $zero -> positive / 0: HI0 = 100_000, LO0 = -1 (`muldiv.expected` line 6)
                asm_special(1, 0, 0, 0, 0x1A),
                // div $zero, $2, $zero -> negative / 0: HI0 = -300_000, LO0 = +1 (`muldiv.expected` line 18)
                asm_special(2, 0, 0, 0, 0x1A),
                // div $zero, $3, $4 -> i32::MIN / -1: HI0 = 0, LO0 = sext32(i32::MIN) (`muldiv.expected` line 16)
                asm_special(3, 4, 0, 0, 0x1A),
                // divu $zero, $2, $zero -> unsigned / 0: HI0 = sext32($2), LO0 = -1 (`muldiv.expected` line 46)
                asm_special(2, 0, 0, 0, 0x1B),
            ],
        );

        let expected_prod = 100_000i64 * -300_000i64;
        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_i64(8), i64::from(expected_prod as i32));
        assert_eq!(cpu.lo.lo_i64(), i64::from(expected_prod as i32));
        assert_eq!(cpu.hi.lo_i64(), i64::from((expected_prod >> 32) as i32));
        assert_eq!(cpu.hi.hi_u64(), hi1_sentinel);
        assert_eq!(cpu.lo.hi_u64(), lo1_sentinel);

        cpu.step(&mut bus);
        assert_eq!(cpu.hi.lo_i64(), 100_000);
        assert_eq!(cpu.lo.lo_i64(), -1);

        cpu.step(&mut bus);
        assert_eq!(cpu.hi.lo_i64(), -300_000);
        assert_eq!(cpu.lo.lo_i64(), 1);

        cpu.step(&mut bus);
        assert_eq!(cpu.hi.lo_i64(), 0);
        assert_eq!(cpu.lo.lo_u64(), 0xFFFF_FFFF_8000_0000);

        cpu.step(&mut bus);
        assert_eq!(cpu.hi.lo_i64(), -300_000);
        assert_eq!(cpu.lo.lo_u64(), 0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(cpu.hi.hi_u64(), hi1_sentinel);
        assert_eq!(cpu.lo.hi_u64(), lo1_sentinel);
    }

    #[test]
    fn ee_branches_jumps_jalr_clobber_and_branch_likely_nullification() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Set $t0 (reg 8) = 0x0010_0010 (target of jalr $t0, $t0).
        cpu.set_gpr_sext32(8, 0x0010_0010);

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // 0x0010_0000: jalr $t0, $t0 -> jumps to old $t0 (0x0010_0010) and sets $t0 = 0x0010_0008 (`branchdelay.cpp`)
                asm_special(8, 0, 8, 0, 0x09),
                // 0x0010_0004: ori $t1, $zero, 0xAA (delay slot executes!)
                asm_itype(0x0D, 0, 9, 0x00AA),
                // 0x0010_0008: ori $t1, $zero, 0xBB (skipped)
                asm_itype(0x0D, 0, 9, 0x00BB),
                // 0x0010_000C: nop
                0,
                // 0x0010_0010: beql $t1, $zero, +4 (not taken -> nullifies delay slot at 0x0010_0014!)
                asm_itype(0x14, 9, 0, 4),
                // 0x0010_0014: ori $t2, $zero, 0xFF (nullified!)
                asm_itype(0x0D, 0, 10, 0x00FF),
                // 0x0010_0018: ori $t2, $zero, 0x42 (executed!)
                asm_itype(0x0D, 0, 10, 0x0042),
            ],
        );

        // Execute jalr $t0, $t0
        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(8), 0x0010_0008);
        assert_eq!(cpu.pc, 0x0010_0004);
        assert!(cpu.in_delay_slot);

        // Execute delay slot (ori $t1, $zero, 0xAA) -> jumps to 0x0010_0010
        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(9), 0xAA);
        assert_eq!(cpu.pc, 0x0010_0010);
        assert!(!cpu.in_delay_slot);

        // Execute beql (not taken) -> nullifies 0x0010_0014 and lands on 0x0010_0018
        cpu.step(&mut bus);
        assert_eq!(cpu.pc, 0x0010_0018);
        assert!(!cpu.in_delay_slot);

        // Execute 0x0010_0018
        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u64(10), 0x42);
    }

    #[test]
    fn ee_loads_stores_lq_sq_and_unaligned_lwl_lwr_ldl_ldr_swl_swr_sdl_sdr() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Store C_PATTERN[1] from `ps2autotests/tests/cpu/ee/lsu.cpp` at 0x0010_1000:
        // {0x23456789, 0xABCDEF01, 0xBEEFDEAD, 0xC0DEC0DE}
        let pattern = Reg128::from_u32x4(0x2345_6789, 0xABCD_EF01, 0xBEEF_DEAD, 0xC0DE_C0DE);
        bus.write128(0x0010_1000, pattern);

        // Base address in $s0 (reg 16) = 0x0010_1000.
        cpu.set_gpr_sext32(16, 0x0010_1000);
        // Seed $t0 (reg 8) with C_GARBAGE1 = {0x1337, 0x1338, 0x1339, 0x133A}.
        let garbage = Reg128::from_u32x4(0x0000_1337, 0x0000_1338, 0x0000_1339, 0x0000_133A);
        cpu.set_gpr_128(8, garbage);
        cpu.set_gpr_128(9, garbage);

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // lwr $t0, 1($s0) -> `lsu.expected` line 79: 0000133a 00001339 00001338 00234567
                asm_itype(0x26, 16, 8, 1),
                // lwl $t1, 0($s0) -> `lsu.expected` line 76: 0000133a 00001339 ffffffff 89001337
                asm_itype(0x22, 16, 9, 0),
                // lwr $t1, 3($s0) -> `lsu.expected` line 80: 0000133a 00001339 ffffffff 89001323
                asm_itype(0x26, 16, 9, 3),
                // lq $t2, 11($s0) -> `lsu.expected` line 97: masks low 4 bits, loads full 128-bit pattern
                asm_itype(0x1E, 16, 10, 11),
                // sq $t2, 0x107($s0) -> masks low 4 bits, stores full 128-bit pattern at 0x0010_1100
                asm_itype(0x1F, 16, 10, 0x0107),
            ],
        );

        cpu.step(&mut bus);
        assert_eq!(
            cpu.gpr_128(8),
            Reg128::from_u32x4(0x0023_4567, 0x0000_1338, 0x0000_1339, 0x0000_133A)
        );

        cpu.step(&mut bus);
        assert_eq!(
            cpu.gpr_128(9),
            Reg128::from_u32x4(0x8900_1337, 0xFFFF_FFFF, 0x0000_1339, 0x0000_133A)
        );

        cpu.step(&mut bus);
        assert_eq!(
            cpu.gpr_128(9),
            Reg128::from_u32x4(0x8900_1323, 0xFFFF_FFFF, 0x0000_1339, 0x0000_133A)
        );

        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_128(10), pattern);

        cpu.step(&mut bus);
        assert_eq!(bus.read128(0x0010_1100), pattern);
    }

    #[test]
    fn ee_cop0_delay_slot_exception_eret_and_timer_interrupt() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Clear BEV and EXL so Level 1 exceptions jump to 0x8000_0180 (common) and 0x8000_0200 (interrupt).
        cpu.cop0_regs[cop0_reg::STATUS] = status_bits::CU0 | status_bits::IE | status_bits::IM_INT0;

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // 0x0010_0000: beq $zero, $zero, +4 (branch to 0x0010_0014)
                asm_itype(0x04, 0, 0, 4),
                // 0x0010_0004: syscall (in branch delay slot -> EPC must be 0x0010_0000 and Cause.BD = 1!)
                asm_special(0, 0, 0, 0, 0x0C),
            ],
        );

        // Place exception handler at 0x8000_0180: advance EPC by 8 (skipping branch + syscall) and ERET.
        let handler = [
            // mfc0 $k0 (26), EPC (14)
            asm_itype(0x10, 0x00, 26, (14 << 11) as u16),
            // addiu $k0, $k0, 8
            asm_itype(0x09, 26, 26, 8),
            // mtc0 $k0 (26), EPC (14)
            asm_itype(0x10, 0x04, 26, (14 << 11) as u16),
            // ei
            0x4200_0038,
            // eret
            0x4200_0018,
        ];
        for (i, &w) in handler.iter().enumerate() {
            bus.write32(0x8000_0180 + (i as u32) * 4, w);
        }

        // Step 1: beq
        cpu.step(&mut bus);
        assert_eq!(cpu.pc, 0x0010_0004);
        assert!(cpu.in_delay_slot);

        // Step 2: syscall in delay slot -> vectors to 0x8000_0180 with EPC = 0x0010_0000 and BD = 1
        cpu.step(&mut bus);
        assert_eq!(cpu.pc, 0x8000_0180);
        assert_eq!(cpu.cop0_regs[cop0_reg::EPC], 0x0010_0000);
        assert_ne!(cpu.cop0_regs[cop0_reg::CAUSE] & cause_bits::BD, 0);
        assert_ne!(cpu.cop0_regs[cop0_reg::STATUS] & status_bits::EXL, 0);

        // Step 3..7: run exception handler through ERET -> returns to 0x0010_0008 with EXL = 0 and EIE = 1
        for _ in 0..5 {
            cpu.step(&mut bus);
        }
        assert_eq!(cpu.pc, 0x0010_0008);
        assert_eq!(cpu.cop0_regs[cop0_reg::STATUS] & status_bits::EXL, 0);
        assert_ne!(cpu.cop0_regs[cop0_reg::STATUS] & status_bits::EIE, 0);

        // Now configure EE Timer 0 (COMP = 1, BUSCLK = 2 EE cycles, CUE = 1, CMPE = 1) and unmask TIM0 in INTC_MASK.
        bus.write32(0x1000_F010, IntcFlags::TIM0.bits());
        bus.write32(0x1000_0020, 1);
        bus.write32(0x1000_0010, mode_bits::CUE | mode_bits::CMPE);

        // Place nops at 0x0010_0008..0x0010_0014. After 2 steps (2 EE cycles = 1 BUSCLK), Timer 0 reaches COMP=1,
        // raising INTC.TIM0, and on the next step CPU vectors to 0x8000_0200 (Level 1 Interrupt vector).
        cpu.step(&mut bus);
        cpu.step(&mut bus);
        assert!(bus.intc.int0_pending());

        cpu.step(&mut bus);
        assert_eq!(cpu.pc, 0x8000_0200);
        assert_eq!(
            (cpu.cop0_regs[cop0_reg::CAUSE] & cause_bits::EXCCODE_MASK) >> 2,
            Level1ExceptionCode::Interrupt as u32
        );
    }

    #[test]
    fn ee_cop0_performance_counter_pccr_pcr0_pcr1_specifiers_and_counting() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Values matching `local/ps2autotests/tests/cpu/ee_cop0/performance.cpp` & `performance.expected`:
        cpu.set_gpr_sext32(4, 0x7FFF_FFFFu32 as i32); // $a0 = 0x7FFF_FFFF
        cpu.set_gpr_sext32(5, 4); // $a1 = 4
        cpu.set_gpr_sext32(6, 5); // $a2 = 5
                                  // PCCR_CTE (1 << 31) | PCCR_EXL0..U0 (0x1E) | PCCR_EVENT0_PROCESSOR_CYCLE (1 << 5) = 0x8000_003E
        cpu.set_gpr_sext32(7, 0x8000_003Eu32 as i32);

        load_program(
            &mut cpu,
            &mut bus,
            0x0010_0000,
            &[
                // mtps $a0, 0 (0x4084_C800) -> PCCR = 0x7FFF_FFFF & 0x800F_FBFE = 0x000F_FBFE (`performance.expected` line 4)
                0x4084_C800,
                // mfps $v0, 31 (0x4002_C83E) -> reads PCCR for any specifier (`performance.expected` line 7)
                0x4002_C83E,
                // mtps $zero, 15 (0x4080_C81E) -> non-zero specifier is a no-op (`performance.expected` line 8)
                0x4080_C81E,
                // mtpc $a1, 30 (0x4085_C83D) -> even specifier writes PCR0 = 4 (`performance.expected` line 6)
                0x4085_C83D,
                // mtpc $a2, 31 (0x4086_C83F) -> odd specifier writes PCR1 = 5 (`performance.expected` line 6)
                0x4086_C83F,
                // mfpc $v1, 2 (0x4003_C805) -> even specifier reads PCR0 = 4
                0x4003_C805,
                // mfpc $t0, 3 (0x4008_C807) -> odd specifier reads PCR1 = 5
                0x4008_C807,
                // mtps $a3, 0 (0x4087_C800) -> enable CTE + PCR0 processor cycles (`performance.expected` line 9)
                0x4087_C800,
                // nop (advances PCR0 by 1 while PCR1 stays unchanged)
                0x0000_0000,
            ],
        );

        // Step 1: mtps $a0, 0
        cpu.step(&mut bus);
        assert_eq!(cpu.cop0_pccr, 0x000F_FBFE);

        // Step 2: mfps $v0, 31
        cpu.step(&mut bus);
        assert_eq!(cpu.gpr_u32(2), 0x000F_FBFE);

        // Step 3: mtps $zero, 15 (no-op)
        cpu.step(&mut bus);
        assert_eq!(cpu.cop0_pccr, 0x000F_FBFE);

        // Step 4..7: mtpc / mfpc even/odd specifiers
        for _ in 0..4 {
            cpu.step(&mut bus);
        }
        assert_eq!(cpu.gpr_u32(3), 4);
        assert_eq!(cpu.gpr_u32(8), 5);

        // Step 8: mtps $a3, 0 (enables PCR0 cycle counting; this step itself increments PCR0 from 4 -> 5)
        cpu.step(&mut bus);
        assert_eq!(cpu.cop0_pcr[0], 5);
        assert_eq!(cpu.cop0_pcr[1], 5);

        // Step 9: nop (increments PCR0 from 5 -> 6, PCR1 unchanged at 5)
        cpu.step(&mut bus);
        assert_eq!(cpu.cop0_pcr[0], 6);
        assert_eq!(cpu.cop0_pcr[1], 5);
    }
}
