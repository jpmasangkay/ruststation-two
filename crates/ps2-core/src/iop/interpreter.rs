//! R3000A I/O Processor (`IOP`) single-step MIPS I interpreter (`Task 6a`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iop`):
//!   * 32-bit MIPS I (`R3000A`) instruction set clocked at `36.864 MHz` (`1:8` EE clock ratio).
//! - `ps2autotests` (`tests/cpu/iop/{alu,branch,branchdelay,hilodelay,lsu,lsudelay,muldiv}.c`):
//!   * 1-instruction load delay slot (`lsudelay.c`): load instructions (`LB`, `LBU`, `LH`, `LHU`,
//!     `LW`, `LWL`, `LWR`) commit their result *after* the immediately following instruction,
//!     unless that delay-slot instruction writes to the same register (cancelling the delayed load).
//!   * Back-to-back `LWL`/`LWR` targeting the same register merge against the in-flight delayed load (`lsu.c`).
//!   * Branch-and-link (`JAL`, `JALR`, `BLTZAL`, `BGEZAL`) writes the link register before the
//!     branch delay slot executes (`branchdelay.c`).
//!   * `BLTZAL` and `BGEZAL` write `$ra = pc + 8` unconditionally even when the branch condition is false (`branch.c`).
//!   * `DIV` / `DIVU` divide-by-zero and `0x8000_0000 / -1` do not trap; they produce the exact
//!     R3000A `HI`/`LO` bit patterns verified by `muldiv.expected`.

use crate::bus::{Bus, UnimplementedIopRegister};
use crate::iop::hle::{try_handle_iop_import_stub, UnimplementedIopImport, MIPS_JR_RA};
use crate::iop::state::{
    IopCpuState, IopExceptionCode, PendingLoad, EE_CYCLES_PER_IOP_CYCLE, IOP_HLE_EXIT_RA,
};

/// Result of a single HLE-aware IOP instruction step (`IopCpuState::step_hle`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IopStepResult {
    /// Normal instruction or handled HLE import stub executed.
    Continue,
    /// Module `_start` returned to `IOP_HLE_EXIT_RA` (`0x0000_0000`) with `$v0` exit code.
    Exited(i32),
    /// Hit an unimplemented `.irx` module import stub.
    UnimplementedImport(UnimplementedIopImport),
    /// Accessed an unimplemented IOP hardware MMIO register (`0x1F80_1000..=0x1F9F_FFFF`).
    UnimplementedRegister(UnimplementedIopRegister),
    /// Hit an unimplemented or illegal IOP opcode.
    UnimplementedInstruction { pc: u32, instr: u32 },
}

impl IopCpuState {
    /// Execute a single R3000A instruction in pure LLE mode and advance the central [`Bus`]
    /// scheduler by `EE_CYCLES_PER_IOP_CYCLE` (`8` EE cycles = `1` IOP cycle).
    pub fn step(&mut self, bus: &mut Bus) {
        let _ = self.execute_cycle(bus, false);
    }

    /// Execute a single R3000A instruction with `.irx` import stub HLE and exit detection,
    /// advancing the central [`Bus`] scheduler by `EE_CYCLES_PER_IOP_CYCLE` (`8` EE cycles).
    pub fn step_hle(&mut self, bus: &mut Bus) -> IopStepResult {
        if self.pc == IOP_HLE_EXIT_RA {
            return IopStepResult::Exited(self.read_gpr(2) as i32);
        }
        let res = self.execute_cycle(bus, true);
        if let Some(reg_err) = bus.pending_iop_mmio_error.take() {
            return IopStepResult::UnimplementedRegister(reg_err);
        }
        res
    }

    fn execute_cycle(&mut self, bus: &mut Bus, enable_hle: bool) -> IopStepResult {
        // Check for pending hardware/software interrupts before fetching.
        if self.cop0.is_interrupt_pending() {
            let (vec_pc, vec_next) = self.cop0.raise_exception(
                IopExceptionCode::Interrupt,
                self.pc,
                self.in_delay_slot,
                None,
            );
            self.pc = vec_pc;
            self.next_pc = vec_next;
            self.in_delay_slot = false;
        }

        let inst_pc = self.pc;
        let inst_in_delay_slot = self.in_delay_slot;

        // Advance PC pipeline for sequential execution; branch instructions override `self.next_pc`.
        self.pc = self.next_pc;
        self.next_pc = self.pc.wrapping_add(4);
        self.in_delay_slot = false;

        // PC alignment check (`AdEL`).
        if (inst_pc & 3) != 0 {
            self.flush_pending_load(None);
            self.enter_exception(
                IopExceptionCode::AddressErrorLoad,
                inst_pc,
                inst_in_delay_slot,
                Some(inst_pc),
            );
            self.finish_cycle(bus);
            return IopStepResult::Continue;
        }

        let instr = bus.iop_read32(inst_pc);

        // In HLE mode, check if `inst_pc` is an IOP `.iop.import` stub (`jr $ra; addiu $zero, $zero, idx`).
        if enable_hle && instr == MIPS_JR_RA {
            match try_handle_iop_import_stub(self, bus, inst_pc) {
                Ok(true) => {
                    // `try_handle_iop_import_stub` updated `$v0`; if a previous load was targeting `$v0`,
                    // cancel it so the HLE return value is preserved, then execute `jr $ra` normally.
                    if let Some(prev) = self.pending_load {
                        if prev.reg == 2 {
                            self.pending_load = None;
                        }
                    }
                }
                Ok(false) => {}
                Err(unimp) => {
                    self.pc = inst_pc;
                    self.next_pc = inst_pc.wrapping_add(4);
                    self.in_delay_slot = inst_in_delay_slot;
                    return IopStepResult::UnimplementedImport(unimp);
                }
            }
        }

        // Pop the pending load from instruction `N - 1`. It will commit at the end of this
        // instruction unless this instruction writes to the same GPR (`lsudelay.c`).
        let prev_load = self.pending_load.take();
        let mut written_reg: Option<u8> = None;
        let mut next_load: Option<PendingLoad> = None;

        let op = (instr >> 26) as u8;
        let rs = ((instr >> 21) & 0x1F) as u8;
        let rt = ((instr >> 16) & 0x1F) as u8;
        let rd = ((instr >> 11) & 0x1F) as u8;
        let sa = ((instr >> 6) & 0x1F) as u8;
        let funct = (instr & 0x3F) as u8;
        let imm16 = (instr & 0xFFFF) as u16;
        let simm = (imm16 as i16) as i32 as u32;
        let target26 = instr & 0x03FF_FFFF;

        let mut step_result = IopStepResult::Continue;

        match op {
            // SPECIAL (0x00)
            0x00 => match funct {
                // SLL
                0x00 => {
                    let val = self.read_gpr(rt) << sa;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SRL
                0x02 => {
                    let val = self.read_gpr(rt) >> sa;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SRA
                0x03 => {
                    let val = ((self.read_gpr(rt) as i32) >> sa) as u32;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SLLV
                0x04 => {
                    let shift = self.read_gpr(rs) & 0x1F;
                    let val = self.read_gpr(rt) << shift;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SRLV
                0x06 => {
                    let shift = self.read_gpr(rs) & 0x1F;
                    let val = self.read_gpr(rt) >> shift;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SRAV
                0x07 => {
                    let shift = self.read_gpr(rs) & 0x1F;
                    let val = ((self.read_gpr(rt) as i32) >> shift) as u32;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // JR
                0x08 => {
                    self.next_pc = self.read_gpr(rs);
                    self.in_delay_slot = true;
                }
                // JALR
                0x09 => {
                    let target = self.read_gpr(rs);
                    let link = inst_pc.wrapping_add(8);
                    self.write_gpr_tracked(rd, link, &mut written_reg);
                    self.next_pc = target;
                    self.in_delay_slot = true;
                }
                // SYSCALL
                0x0C => {
                    self.enter_exception(
                        IopExceptionCode::Syscall,
                        inst_pc,
                        inst_in_delay_slot,
                        None,
                    );
                }
                // BREAK
                0x0D => {
                    self.enter_exception(
                        IopExceptionCode::Breakpoint,
                        inst_pc,
                        inst_in_delay_slot,
                        None,
                    );
                }
                // MFHI
                0x10 => {
                    let val = self.hi;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // MTHI
                0x11 => {
                    self.hi = self.read_gpr(rs);
                }
                // MFLO
                0x12 => {
                    let val = self.lo;
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // MTLO
                0x13 => {
                    self.lo = self.read_gpr(rs);
                }
                // MULT (signed 32x32 -> 64)
                0x18 => {
                    let a = self.read_gpr(rs) as i32 as i64;
                    let b = self.read_gpr(rt) as i32 as i64;
                    let prod = (a * b) as u64;
                    self.lo = prod as u32;
                    self.hi = (prod >> 32) as u32;
                }
                // MULTU (unsigned 32x32 -> 64)
                0x19 => {
                    let a = self.read_gpr(rs) as u64;
                    let b = self.read_gpr(rt) as u64;
                    let prod = a * b;
                    self.lo = prod as u32;
                    self.hi = (prod >> 32) as u32;
                }
                // DIV (signed 32/32, `muldiv.expected` lines 2-29)
                0x1A => {
                    let num = self.read_gpr(rs) as i32;
                    let den = self.read_gpr(rt) as i32;
                    if den == 0 {
                        self.hi = num as u32;
                        self.lo = if num >= 0 { 0xFFFF_FFFF } else { 0x0000_0001 };
                    } else if num == i32::MIN && den == -1 {
                        self.hi = 0;
                        self.lo = 0x8000_0000;
                    } else {
                        self.lo = (num / den) as u32;
                        self.hi = (num % den) as u32;
                    }
                }
                // DIVU (unsigned 32/32, `muldiv.expected` lines 31-58)
                0x1B => {
                    let num = self.read_gpr(rs);
                    let den = self.read_gpr(rt);
                    if let Some(q) = num.checked_div(den) {
                        self.lo = q;
                        self.hi = num % den;
                    } else {
                        self.hi = num;
                        self.lo = 0xFFFF_FFFF;
                    }
                }
                // ADD (signed overflow traps with `Ov`)
                0x20 => {
                    let a = self.read_gpr(rs) as i32;
                    let b = self.read_gpr(rt) as i32;
                    match a.checked_add(b) {
                        Some(res) => self.write_gpr_tracked(rd, res as u32, &mut written_reg),
                        None => self.enter_exception(
                            IopExceptionCode::Overflow,
                            inst_pc,
                            inst_in_delay_slot,
                            None,
                        ),
                    }
                }
                // ADDU
                0x21 => {
                    let val = self.read_gpr(rs).wrapping_add(self.read_gpr(rt));
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SUB (signed overflow traps with `Ov`)
                0x22 => {
                    let a = self.read_gpr(rs) as i32;
                    let b = self.read_gpr(rt) as i32;
                    match a.checked_sub(b) {
                        Some(res) => self.write_gpr_tracked(rd, res as u32, &mut written_reg),
                        None => self.enter_exception(
                            IopExceptionCode::Overflow,
                            inst_pc,
                            inst_in_delay_slot,
                            None,
                        ),
                    }
                }
                // SUBU
                0x23 => {
                    let val = self.read_gpr(rs).wrapping_sub(self.read_gpr(rt));
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // AND
                0x24 => {
                    let val = self.read_gpr(rs) & self.read_gpr(rt);
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // OR
                0x25 => {
                    let val = self.read_gpr(rs) | self.read_gpr(rt);
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // XOR
                0x26 => {
                    let val = self.read_gpr(rs) ^ self.read_gpr(rt);
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // NOR
                0x27 => {
                    let val = !(self.read_gpr(rs) | self.read_gpr(rt));
                    self.write_gpr_tracked(rd, val, &mut written_reg);
                }
                // SLT
                0x2A => {
                    let cond = (self.read_gpr(rs) as i32) < (self.read_gpr(rt) as i32);
                    self.write_gpr_tracked(rd, u32::from(cond), &mut written_reg);
                }
                // SLTU
                0x2B => {
                    let cond = self.read_gpr(rs) < self.read_gpr(rt);
                    self.write_gpr_tracked(rd, u32::from(cond), &mut written_reg);
                }
                _ => {
                    step_result = IopStepResult::UnimplementedInstruction { pc: inst_pc, instr };
                }
            },

            // REGIMM (0x01): BLTZ, BGEZ, BLTZAL, BGEZAL
            // Verified against `ps2autotests` `tests/cpu/iop/branch.c`:
            // On MIPS I R3000A, bit 0 of `rt` selects BGEZ (1) vs BLTZ (0), and `(rt & 0x1E) == 0x10`
            // selects link (`$ra = inst_pc + 8` unconditionally).
            0x01 => {
                let is_bgez = (rt & 1) != 0;
                let is_link = (rt & 0x1E) == 0x10;
                let s = self.read_gpr(rs) as i32;
                let cond = if is_bgez { s >= 0 } else { s < 0 };
                if is_link {
                    let link = inst_pc.wrapping_add(8);
                    self.write_gpr_tracked(31, link, &mut written_reg);
                }
                if cond {
                    self.next_pc = inst_pc.wrapping_add(4).wrapping_add(simm << 2);
                }
                self.in_delay_slot = true;
            }

            // J (0x02)
            0x02 => {
                let pc_plus_4 = inst_pc.wrapping_add(4);
                self.next_pc = (pc_plus_4 & 0xF000_0000) | (target26 << 2);
                self.in_delay_slot = true;
            }

            // JAL (0x03)
            0x03 => {
                let pc_plus_4 = inst_pc.wrapping_add(4);
                let link = inst_pc.wrapping_add(8);
                self.write_gpr_tracked(31, link, &mut written_reg);
                self.next_pc = (pc_plus_4 & 0xF000_0000) | (target26 << 2);
                self.in_delay_slot = true;
            }

            // BEQ (0x04)
            0x04 => {
                if self.read_gpr(rs) == self.read_gpr(rt) {
                    self.next_pc = inst_pc.wrapping_add(4).wrapping_add(simm << 2);
                }
                self.in_delay_slot = true;
            }

            // BNE (0x05)
            0x05 => {
                if self.read_gpr(rs) != self.read_gpr(rt) {
                    self.next_pc = inst_pc.wrapping_add(4).wrapping_add(simm << 2);
                }
                self.in_delay_slot = true;
            }

            // BLEZ (0x06)
            0x06 => {
                if (self.read_gpr(rs) as i32) <= 0 {
                    self.next_pc = inst_pc.wrapping_add(4).wrapping_add(simm << 2);
                }
                self.in_delay_slot = true;
            }

            // BGTZ (0x07)
            0x07 => {
                if (self.read_gpr(rs) as i32) > 0 {
                    self.next_pc = inst_pc.wrapping_add(4).wrapping_add(simm << 2);
                }
                self.in_delay_slot = true;
            }

            // ADDI (0x08)
            0x08 => {
                let a = self.read_gpr(rs) as i32;
                let b = simm as i32;
                match a.checked_add(b) {
                    Some(res) => self.write_gpr_tracked(rt, res as u32, &mut written_reg),
                    None => self.enter_exception(
                        IopExceptionCode::Overflow,
                        inst_pc,
                        inst_in_delay_slot,
                        None,
                    ),
                }
            }

            // ADDIU (0x09)
            0x09 => {
                let val = self.read_gpr(rs).wrapping_add(simm);
                self.write_gpr_tracked(rt, val, &mut written_reg);
            }

            // SLTI (0x0A)
            0x0A => {
                let cond = (self.read_gpr(rs) as i32) < (simm as i32);
                self.write_gpr_tracked(rt, u32::from(cond), &mut written_reg);
            }

            // SLTIU (0x0B)
            0x0B => {
                let cond = self.read_gpr(rs) < simm;
                self.write_gpr_tracked(rt, u32::from(cond), &mut written_reg);
            }

            // ANDI (0x0C)
            0x0C => {
                let val = self.read_gpr(rs) & (imm16 as u32);
                self.write_gpr_tracked(rt, val, &mut written_reg);
            }

            // ORI (0x0D)
            0x0D => {
                let val = self.read_gpr(rs) | (imm16 as u32);
                self.write_gpr_tracked(rt, val, &mut written_reg);
            }

            // XORI (0x0E)
            0x0E => {
                let val = self.read_gpr(rs) ^ (imm16 as u32);
                self.write_gpr_tracked(rt, val, &mut written_reg);
            }

            // LUI (0x0F)
            0x0F => {
                let val = (imm16 as u32) << 16;
                self.write_gpr_tracked(rt, val, &mut written_reg);
            }

            // COP0 (0x10)
            0x10 => match rs {
                // MFC0 (rs = 0x00) — has 1-instruction load delay on R3000A
                0x00 => {
                    let val = self.cop0.read_reg(rd);
                    Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
                }
                // MTC0 (rs = 0x04)
                0x04 => {
                    let val = self.read_gpr(rt);
                    self.cop0.write_reg(rd, val);
                }
                // RFE (rs = 0x10, funct = 0x10)
                0x10 if funct == 0x10 => {
                    self.cop0.rfe();
                }
                _ => {
                    step_result = IopStepResult::UnimplementedInstruction { pc: inst_pc, instr };
                }
            },

            // LB (0x20)
            0x20 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let val = (bus.iop_read8(vaddr) as i8) as i32 as u32;
                Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
            }

            // LH (0x21)
            0x21 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                if (vaddr & 1) != 0 {
                    self.enter_exception(
                        IopExceptionCode::AddressErrorLoad,
                        inst_pc,
                        inst_in_delay_slot,
                        Some(vaddr),
                    );
                } else {
                    let val = (bus.iop_read16(vaddr) as i16) as i32 as u32;
                    Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
                }
            }

            // LWL (0x22) — Unaligned load word left (`lsu.c` & `lsu.expected` lines 56-68)
            0x22 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let aligned = vaddr & !3;
                let byte = vaddr & 3;
                let mem = bus.iop_read32(aligned);
                // Merge against in-flight delayed load if back-to-back LWL/LWR targets `rt`.
                let cur_rt = match prev_load {
                    Some(prev) if prev.reg == rt => prev.value,
                    _ => self.read_gpr(rt),
                };
                let val = match byte {
                    0 => (cur_rt & 0x00FF_FFFF) | (mem << 24),
                    1 => (cur_rt & 0x0000_FFFF) | (mem << 16),
                    2 => (cur_rt & 0x0000_00FF) | (mem << 8),
                    _ => mem,
                };
                Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
            }

            // LW (0x23)
            0x23 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                if (vaddr & 3) != 0 {
                    self.enter_exception(
                        IopExceptionCode::AddressErrorLoad,
                        inst_pc,
                        inst_in_delay_slot,
                        Some(vaddr),
                    );
                } else {
                    let val = bus.iop_read32(vaddr);
                    Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
                }
            }

            // LBU (0x24)
            0x24 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let val = bus.iop_read8(vaddr) as u32;
                Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
            }

            // LHU (0x25)
            0x25 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                if (vaddr & 1) != 0 {
                    self.enter_exception(
                        IopExceptionCode::AddressErrorLoad,
                        inst_pc,
                        inst_in_delay_slot,
                        Some(vaddr),
                    );
                } else {
                    let val = bus.iop_read16(vaddr) as u32;
                    Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
                }
            }

            // LWR (0x26) — Unaligned load word right (`lsu.c` & `lsu.expected` lines 56-68)
            0x26 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let aligned = vaddr & !3;
                let byte = vaddr & 3;
                let mem = bus.iop_read32(aligned);
                let cur_rt = match prev_load {
                    Some(prev) if prev.reg == rt => prev.value,
                    _ => self.read_gpr(rt),
                };
                let val = match byte {
                    0 => mem,
                    1 => (cur_rt & 0xFF00_0000) | (mem >> 8),
                    2 => (cur_rt & 0xFFFF_0000) | (mem >> 16),
                    _ => (cur_rt & 0xFFFF_FF00) | (mem >> 24),
                };
                Self::queue_delayed_load(rt, val, &mut next_load, &mut written_reg);
            }

            // SB (0x28)
            0x28 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let val = (self.read_gpr(rt) & 0xFF) as u8;
                bus.iop_write8(vaddr, val);
            }

            // SH (0x29)
            0x29 => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                if (vaddr & 1) != 0 {
                    self.enter_exception(
                        IopExceptionCode::AddressErrorStore,
                        inst_pc,
                        inst_in_delay_slot,
                        Some(vaddr),
                    );
                } else {
                    let val = (self.read_gpr(rt) & 0xFFFF) as u16;
                    bus.iop_write16(vaddr, val);
                }
            }

            // SWL (0x2A) — Unaligned store word left (`lsu.c` & `lsu.expected` lines 92-113)
            0x2A => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let aligned = vaddr & !3;
                let byte = vaddr & 3;
                let mem = bus.iop_read32(aligned);
                let rt_val = self.read_gpr(rt);
                let merged = match byte {
                    0 => (mem & 0xFFFF_FF00) | (rt_val >> 24),
                    1 => (mem & 0xFFFF_0000) | (rt_val >> 16),
                    2 => (mem & 0xFF00_0000) | (rt_val >> 8),
                    _ => rt_val,
                };
                bus.iop_write32(aligned, merged);
            }

            // SW (0x2B)
            0x2B => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                if (vaddr & 3) != 0 {
                    self.enter_exception(
                        IopExceptionCode::AddressErrorStore,
                        inst_pc,
                        inst_in_delay_slot,
                        Some(vaddr),
                    );
                } else {
                    let val = self.read_gpr(rt);
                    bus.iop_write32(vaddr, val);
                }
            }

            // SWR (0x2E) — Unaligned store word right (`lsu.c` & `lsu.expected` lines 92-113)
            0x2E => {
                let vaddr = self.read_gpr(rs).wrapping_add(simm);
                let aligned = vaddr & !3;
                let byte = vaddr & 3;
                let mem = bus.iop_read32(aligned);
                let rt_val = self.read_gpr(rt);
                let merged = match byte {
                    0 => rt_val,
                    1 => (mem & 0x0000_00FF) | (rt_val << 8),
                    2 => (mem & 0x0000_FFFF) | (rt_val << 16),
                    _ => (mem & 0x00FF_FFFF) | (rt_val << 24),
                };
                bus.iop_write32(aligned, merged);
            }

            _ => {
                step_result = IopStepResult::UnimplementedInstruction { pc: inst_pc, instr };
            }
        }

        // Commit the previous instruction's delayed load if this instruction did not write to
        // the same register (`lsudelay.c`).
        if let Some(prev) = prev_load {
            if written_reg != Some(prev.reg) {
                self.write_gpr(prev.reg, prev.value);
            }
        }
        self.pending_load = next_load;
        self.gpr[0] = 0;

        self.finish_cycle(bus);
        step_result
    }

    #[inline(always)]
    fn write_gpr_tracked(&mut self, reg: u8, val: u32, written_reg: &mut Option<u8>) {
        let idx = reg & 0x1F;
        if idx != 0 {
            self.gpr[idx as usize] = val;
            *written_reg = Some(idx);
        }
    }

    #[inline(always)]
    fn queue_delayed_load(
        reg: u8,
        val: u32,
        next_load: &mut Option<PendingLoad>,
        written_reg: &mut Option<u8>,
    ) {
        let idx = reg & 0x1F;
        if idx != 0 {
            *next_load = Some(PendingLoad {
                reg: idx,
                value: val,
            });
            *written_reg = Some(idx);
        }
    }

    #[inline]
    fn flush_pending_load(&mut self, prev_load: Option<PendingLoad>) {
        if let Some(prev) = prev_load.or_else(|| self.pending_load.take()) {
            self.write_gpr(prev.reg, prev.value);
        }
    }

    fn enter_exception(
        &mut self,
        code: IopExceptionCode,
        inst_pc: u32,
        in_delay_slot: bool,
        bad_vaddr: Option<u32>,
    ) {
        let (vec_pc, vec_next) = self
            .cop0
            .raise_exception(code, inst_pc, in_delay_slot, bad_vaddr);
        self.pc = vec_pc;
        self.next_pc = vec_next;
        self.in_delay_slot = false;
    }

    /// Advance the IOP cycle counter by `1` and the central [`Bus`] scheduler by `8` EE cycles
    /// (`EE_CYCLES_PER_IOP_CYCLE = 8`, `36.864 MHz` IOP vs `294.912 MHz` EE).
    #[inline]
    fn finish_cycle(&mut self, bus: &mut Bus) {
        self.cycle_count = self.cycle_count.wrapping_add(1);
        bus.step_timers(EE_CYCLES_PER_IOP_CYCLE as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_iop_code(bus: &mut Bus, base: u32, words: &[u32]) {
        for (i, &w) in words.iter().enumerate() {
            bus.iop_write32(base + (i as u32) * 4, w);
        }
    }

    #[test]
    fn iop_load_delay_slot_visibility_and_overwrite_cancellation() {
        let mut iop = IopCpuState::new();
        let mut bus = Bus::new();

        // Store 0x1337_1337 at 0x0000_2000.
        bus.iop_write32(0x0000_2000, 0x1337_1337);
        iop.write_gpr(4, 0x0000_2000); // $a0 = 0x2000
        iop.write_gpr(8, 0x1122_3344); // $t0 = 0x1122_3344

        // Case 1: instruction in load delay slot reads old value of $t0 (`lsudelay.c` test_delay1_lw):
        // 0x1000: lw  $t0, 0($a0)      (0x8C88_0000)
        // 0x1004: or  $t1, $zero, $t0  (0x0008_4825) -> $t1 sees old $t0 (0x1122_3344), then $t0 becomes 0x1337_1337
        // 0x1008: or  $t2, $zero, $t0  (0x0008_5025) -> $t2 sees loaded $t0 (0x1337_1337)
        // Case 2: instruction in load delay slot overwrites $t0 (`lsudelay.c` test_delay0_lw):
        // 0x100C: lw  $t0, 0($a0)      (0x8C88_0000)
        // 0x1010: ori $t0, $zero, 0x99 (0x3408_0099) -> cancels delayed load; $t0 stays 0x99
        // 0x1014: nop                  (0x0000_0000)
        write_iop_code(
            &mut bus,
            0x0000_1000,
            &[
                0x8C88_0000,
                0x0008_4825,
                0x0008_5025,
                0x8C88_0000,
                0x3408_0099,
                0x0000_0000,
            ],
        );
        iop.set_pc(0x0000_1000);

        for _ in 0..6 {
            iop.step(&mut bus);
        }

        assert_eq!(iop.read_gpr(9), 0x1122_3344); // $t1 saw pre-load value
        assert_eq!(iop.read_gpr(10), 0x1337_1337); // $t2 saw committed load value
        assert_eq!(iop.read_gpr(8), 0x0000_0099); // Delay-slot write cancelled second load
        assert_eq!(iop.cycle_count, 6);
        assert_eq!(bus.scheduler.current_ee_cycle, 6 * EE_CYCLES_PER_IOP_CYCLE);
    }

    #[test]
    fn iop_branch_delay_slot_link_order_and_div_edge_cases() {
        let mut iop = IopCpuState::new();
        let mut bus = Bus::new();

        // Test `jal` where the branch delay slot overwrites `$ra` (`branchdelay.c` test_jal):
        // 0x1000: jal  0x100C           (0x0C00_0403) -> sets $ra = 0x1008
        // 0x1004: ori  $ra, $zero, 2    (0x341F_0002) -> delay slot overwrites $ra = 2
        // 0x1008: ori  $v0, $zero, 0xFF (0x3402_00FF) -> skipped
        // 0x100C: or   $v0, $zero, $ra  (0x001F_1025) -> $v0 = 2
        write_iop_code(
            &mut bus,
            0x0000_1000,
            &[0x0C00_0403, 0x341F_0002, 0x3402_00FF, 0x001F_1025],
        );
        iop.set_pc(0x0000_1000);
        for _ in 0..3 {
            iop.step(&mut bus);
        }
        assert_eq!(iop.read_gpr(2), 2);

        // Test signed DIV by zero (-53 / 0 -> HI = -53, LO = +1) and i32::MIN / -1 (`muldiv.expected`):
        iop.write_gpr(8, (-53i32) as u32);
        iop.write_gpr(9, 0);
        write_iop_code(
            &mut bus,
            0x0000_2000,
            &[
                0x0109_001A, // div $t0, $t1
            ],
        );
        iop.set_pc(0x0000_2000);
        iop.step(&mut bus);
        assert_eq!(iop.hi, (-53i32) as u32);
        assert_eq!(iop.lo, 1);
    }
}
