//! Emotion Engine System Control Coprocessor (`COP0`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eecop0` & `https://psi-rockin.github.io/ps2tek/#eecop0memorymanagement`):
//!   * 32 system control registers (`Index`, `Random`, `EntryLo0`, `EntryLo1`, `Context`,
//!     `PageMask`, `Wired`, `BadVAddr`, `Count`, `EntryHi`, `Compare`, `Status`, `Cause`,
//!     `EPC`, `PRId`, `Config`, `BadPAddr`, `Debug`, `Perf`, `TagLo`, `TagHi`, `ErrorEPC`).
//!   * Level 1 exception handling (`handle_exception_level1`):
//!     - Saves `EPC = pc - 4` and sets `Cause.BD` (bit 31) if `in_delay_slot` (only when `Status.EXL == 0`).
//!     - Sets `Status.EXL = 1` and updates `Cause.ExcCode` (bits `6..2`).
//!     - Selects vector base `0xBFC0_0200` if `Status.BEV == 1`, else `0x8000_0000`, plus offset
//!       `0x000` (TLB Refill when `EXL == 0`), `0x180` (Common), or `0x200` (Interrupt).
//!   * Level 2 exception handling (`handle_exception_level2`):
//!     - Saves `ErrorEPC` and `Cause.BD2` (bit 30), sets `Status.ERL = 1`, updates `Cause.EXC2` (bits `18..16`).
//!   * `ERET`: returns to `ErrorEPC` and clears `Status.ERL` if `Status.ERL == 1`, otherwise
//!     returns to `EPC` and clears `Status.EXL`.
//!   * `EI` / `DI`: sets or clears `Status.EIE` (bit 16) when `Status.EDI == 1` or in Kernel mode
//!     (`KSU == 0` or `EXL == 1` or `ERL == 1`).

use crate::bus::Bus;
use crate::mmu::{TlbEntry, TLB_ENTRY_COUNT};

use super::state::EeCpuState;

/// Architectural COP0 register indices (`ps2tek` `#eecop0`).
pub mod cop0_reg {
    pub const INDEX: usize = 0;
    pub const RANDOM: usize = 1;
    pub const ENTRY_LO0: usize = 2;
    pub const ENTRY_LO1: usize = 3;
    pub const CONTEXT: usize = 4;
    pub const PAGE_MASK: usize = 5;
    pub const WIRED: usize = 6;
    pub const BAD_VADDR: usize = 8;
    pub const COUNT: usize = 9;
    pub const ENTRY_HI: usize = 10;
    pub const COMPARE: usize = 11;
    pub const STATUS: usize = 12;
    pub const CAUSE: usize = 13;
    pub const EPC: usize = 14;
    pub const PRID: usize = 15;
    pub const CONFIG: usize = 16;
    pub const BAD_PADDR: usize = 23;
    pub const DEBUG: usize = 24;
    pub const PERF: usize = 25;
    pub const TAG_LO: usize = 28;
    pub const TAG_HI: usize = 29;
    pub const ERROR_EPC: usize = 30;
}

/// `COP0.Status` (`$12`) bit masks (`ps2tek` `#eecop0`).
pub mod status_bits {
    /// Bit 0: Master Interrupt Enable (`IE`).
    pub const IE: u32 = 1 << 0;
    /// Bit 1: Exception Level (`EXL`).
    pub const EXL: u32 = 1 << 1;
    /// Bit 2: Error Level (`ERL`).
    pub const ERL: u32 = 1 << 2;
    /// Bits `4..=3`: Kernel/Supervisor/User mode (`0` = Kernel, `1` = Supervisor, `2` = User).
    pub const KSU_MASK: u32 = 0b11 << 3;
    /// Bit 10: `INT0` (INTC) interrupt mask (`IM2`).
    pub const IM_INT0: u32 = 1 << 10;
    /// Bit 11: `INT1` (DMAC) interrupt mask (`IM3`).
    pub const IM_INT1: u32 = 1 << 11;
    /// Bit 12: Bus Error Mask (`BEM`).
    pub const BEM: u32 = 1 << 12;
    /// Bit 15: `INT5` (COP0 `Count == Compare` timer) interrupt mask (`IM7`).
    pub const IM_TIMER: u32 = 1 << 15;
    /// Mask of all 3 EE interrupt enable bits (`INT0`, `INT1`, `INT5`).
    pub const INT_MASK_ALL: u32 = IM_INT0 | IM_INT1 | IM_TIMER;
    /// Bit 16: Enable `IE` (`EIE`, controlled by `EI`/`DI`).
    pub const EIE: u32 = 1 << 16;
    /// Bit 17: Allow `EI`/`DI` in User/Supervisor mode (`EDI`).
    pub const EDI: u32 = 1 << 17;
    /// Bit 18: Cache Hit flag (`CH`).
    pub const CH: u32 = 1 << 18;
    /// Bit 22: Bootstrap Exception Vectors (`BEV`: `0` = RAM `0x8000_0000`, `1` = ROM `0xBFC0_0200`).
    pub const BEV: u32 = 1 << 22;
    /// Bit 23: Bootstrap Performance/Debug Exception Vectors (`DEV`).
    pub const DEV: u32 = 1 << 23;
    /// Bit 28: Coprocessor 0 Usable in User mode (`CU0`).
    pub const CU0: u32 = 1 << 28;
    /// Bit 29: Coprocessor 1 (FPU) Usable (`CU1`).
    pub const CU1: u32 = 1 << 29;
    /// Bit 30: Coprocessor 2 (VU0 Macro) Usable (`CU2`).
    pub const CU2: u32 = 1 << 30;
}

/// `COP0.Cause` (`$13`) bit masks (`ps2tek` `#eecop0`).
pub mod cause_bits {
    /// Bits `6..=2`: Level 1 Exception Code (`ExcCode`).
    pub const EXCCODE_MASK: u32 = 0x1F << 2;
    /// Bit 10: `INT0` (INTC) interrupt pending (`IP2`).
    pub const IP_INT0: u32 = 1 << 10;
    /// Bit 11: `INT1` (DMAC) interrupt pending (`IP3`).
    pub const IP_INT1: u32 = 1 << 11;
    /// Bit 15: `INT5` (COP0 Timer `Count == Compare`) interrupt pending (`IP7`).
    pub const IP_TIMER: u32 = 1 << 15;
    /// Bits `18..=16`: Level 2 Exception Code (`EXC2`).
    pub const EXC2_MASK: u32 = 0b111 << 16;
    /// Bits `29..=28`: Coprocessor Error number (`CE`).
    pub const CE_MASK: u32 = 0b11 << 28;
    /// Bit 30: Level 2 Branch Delay Slot flag (`BD2`).
    pub const BD2: u32 = 1 << 30;
    /// Bit 31: Level 1 Branch Delay Slot flag (`BD`).
    pub const BD: u32 = 1 << 31;
}

/// Level 1 Exception Codes (`COP0.Cause.ExcCode`, bits `6..=2`, `ps2tek` `#eecop0`).
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Level1ExceptionCode {
    /// `0`: Interrupt (`Int`).
    Interrupt = 0,
    /// `1`: TLB Modified (`Mod`).
    TlbModified = 1,
    /// `2`: TLB Invalid / Refill on Load or Instruction Fetch (`TLBL`).
    TlbLoad = 2,
    /// `3`: TLB Invalid / Refill on Store (`TLBS`).
    TlbStore = 3,
    /// `4`: Address Error on Load or Instruction Fetch (`AdEL`).
    AddressErrorLoad = 4,
    /// `5`: Address Error on Store (`AdES`).
    AddressErrorStore = 5,
    /// `6`: Bus Error on Instruction Fetch (`IBE`).
    BusErrorInstruction = 6,
    /// `7`: Bus Error on Data Reference (`DBE`).
    BusErrorData = 7,
    /// `8`: System Call (`SYSCALL`).
    Syscall = 8,
    /// `9`: Breakpoint (`BREAK`).
    Breakpoint = 9,
    /// `10`: Reserved Instruction (`RI`).
    ReservedInstruction = 10,
    /// `11`: Coprocessor Unusable (`CpU`).
    CoprocessorUnusable = 11,
    /// `12`: Arithmetic Overflow (`Ov`).
    Overflow = 12,
    /// `13`: Trap (`Tr`).
    Trap = 13,
}

impl EeCpuState {
    /// Returns `true` if the CPU is currently executing with Kernel privileges
    /// (`KSU == 0` or `EXL == 1` or `ERL == 1`, `ps2tek` `#eecop0`).
    #[inline(always)]
    pub const fn in_kernel_mode(&self) -> bool {
        let status = self.cop0_regs[cop0_reg::STATUS];
        (status & status_bits::KSU_MASK) == 0
            || (status & (status_bits::EXL | status_bits::ERL)) != 0
    }

    /// Read a 32-bit COP0 register (`MFC0`, `ps2tek` `#eecop0`).
    #[inline]
    pub fn read_cop0(&self, reg: usize, bus: &Bus) -> u32 {
        match reg & 31 {
            cop0_reg::RANDOM => {
                let wired = (self.cop0_regs[cop0_reg::WIRED] & 0x3F).min(47);
                let span = (TLB_ENTRY_COUNT as u32) - wired;
                let count = self.cop0_regs[cop0_reg::COUNT];
                wired + (count % span)
            }
            cop0_reg::CAUSE => {
                let mut cause =
                    self.cop0_regs[cop0_reg::CAUSE] & !(cause_bits::IP_INT0 | cause_bits::IP_INT1);
                if bus.intc.int0_pending() {
                    cause |= cause_bits::IP_INT0;
                }
                if self.int1_line {
                    cause |= cause_bits::IP_INT1;
                }
                cause
            }
            idx => self.cop0_regs[idx],
        }
    }

    /// Write a 32-bit COP0 register (`MTC0`, `ps2tek` `#eecop0`).
    #[inline]
    pub fn write_cop0(&mut self, reg: usize, val: u32) {
        match reg & 31 {
            cop0_reg::RANDOM | cop0_reg::PRID | cop0_reg::BAD_VADDR | cop0_reg::BAD_PADDR => {
                // Read-only architectural registers (`ps2tek` `#eecop0`).
            }
            cop0_reg::INDEX => {
                self.cop0_regs[cop0_reg::INDEX] = val & 0x8000_003F;
            }
            cop0_reg::PAGE_MASK => {
                self.cop0_regs[cop0_reg::PAGE_MASK] = val & (0xFFF << 13);
            }
            cop0_reg::WIRED => {
                self.cop0_regs[cop0_reg::WIRED] = val & 0x3F;
            }
            cop0_reg::COMPARE => {
                self.cop0_regs[cop0_reg::COMPARE] = val;
                // Writing to `Compare` clears the timer interrupt flag (`Cause.IP7`, bit 15, `ps2tek` `#eecop0`).
                self.cop0_regs[cop0_reg::CAUSE] &= !cause_bits::IP_TIMER;
            }
            cop0_reg::CAUSE => {
                // Only software-writable fields (none on R5900; preserve read-only status bits).
                let _ = val;
            }
            idx => {
                self.cop0_regs[idx] = val;
            }
        }
    }

    /// Advance `COP0.Count` (`$9`) and enabled `COP0.Perf` counters (`PCR0`, `PCR1`) by `ee_cycles`,
    /// asserting `Cause.IP7` (bit 15) if `Count` reaches `Compare` (`$11`).
    ///
    /// Primary source references:
    /// - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eecop0`).
    /// - `ps2autotests` (`local/ps2autotests/tests/cpu/ee_cop0/performance.cpp` & `performance.expected`):
    ///   * `PCCR.CTE` (bit 31) enables performance counter counting.
    ///   * `PCR0` increments on processor cycles when `(PCCR & 0x0000_001E) != 0` (at least one mode bit
    ///     `EXL0 | K0 | S0 | U0` enabled) and `EVENT0` (`(PCCR >> 5) & 0x1F`) is `1` (`Processor cycle`).
    ///   * `PCR1` increments on processor cycles when `(PCCR & 0x0000_7800) != 0` (at least one mode bit
    ///     `EXL1 | K1 | S1 | U1` enabled) and `EVENT1` (`(PCCR >> 15) & 0x1F`) is `1` (`Processor cycle`).
    #[inline]
    pub fn advance_cop0_count(&mut self, ee_cycles: u32) {
        let old_count = self.cop0_regs[cop0_reg::COUNT];
        let new_count = old_count.wrapping_add(ee_cycles);
        let compare = self.cop0_regs[cop0_reg::COMPARE];
        let diff_to_compare = compare.wrapping_sub(old_count);
        if diff_to_compare > 0 && diff_to_compare <= ee_cycles {
            self.cop0_regs[cop0_reg::CAUSE] |= cause_bits::IP_TIMER;
        }
        self.cop0_regs[cop0_reg::COUNT] = new_count;

        let pccr = self.cop0_pccr;
        if (pccr & (1 << 31)) != 0 {
            let event0 = (pccr >> 5) & 0x1F;
            if (pccr & 0x0000_001E) != 0 && event0 == 1 {
                self.cop0_pcr[0] = self.cop0_pcr[0].wrapping_add(ee_cycles);
            }
            let event1 = (pccr >> 15) & 0x1F;
            if (pccr & 0x0000_7800) != 0 && event1 == 1 {
                self.cop0_pcr[1] = self.cop0_pcr[1].wrapping_add(ee_cycles);
            }
        }
    }

    /// Raise a Level 1 exception (`ps2tek` `https://psi-rockin.github.io/ps2tek/#eecop0`).
    ///
    /// - Sets `Cause.ExcCode = code`.
    /// - If `Status.EXL == 0`, records `EPC` (`pc - 4` if `in_delay_slot`, else `pc`) and `Cause.BD`.
    /// - Sets `Status.EXL = 1` and redirects `pc` / `next_pc` to the appropriate vector.
    pub fn raise_level1_exception(&mut self, code: Level1ExceptionCode, tlb_refill: bool) {
        let status = self.cop0_regs[cop0_reg::STATUS];
        let exl_was_set = (status & status_bits::EXL) != 0;

        let mut cause = self.cop0_regs[cop0_reg::CAUSE] & !cause_bits::EXCCODE_MASK;
        cause |= (code as u32) << 2;

        if !exl_was_set {
            if self.in_delay_slot {
                self.cop0_regs[cop0_reg::EPC] = self.pc.wrapping_sub(4);
                cause |= cause_bits::BD;
            } else {
                self.cop0_regs[cop0_reg::EPC] = self.pc;
                cause &= !cause_bits::BD;
            }
        }

        self.cop0_regs[cop0_reg::CAUSE] = cause;
        self.cop0_regs[cop0_reg::STATUS] = status | status_bits::EXL;

        let vector_base = if (status & status_bits::BEV) != 0 {
            0xBFC0_0200u32
        } else {
            0x8000_0000u32
        };

        let vector_offset = if tlb_refill && !exl_was_set {
            0x0000u32
        } else if code == Level1ExceptionCode::Interrupt {
            0x0200u32
        } else {
            0x0180u32
        };

        let target = vector_base.wrapping_add(vector_offset);
        self.pc = target;
        self.next_pc = target.wrapping_add(4);
        self.in_delay_slot = false;
    }

    /// Raise an Address Error (`AdEL` or `AdES`), recording the faulting address in `BadVAddr` (`$8`).
    #[inline]
    pub fn raise_address_error(&mut self, bad_vaddr: u32, is_store: bool) {
        self.cop0_regs[cop0_reg::BAD_VADDR] = bad_vaddr;
        let code = if is_store {
            Level1ExceptionCode::AddressErrorStore
        } else {
            Level1ExceptionCode::AddressErrorLoad
        };
        self.raise_level1_exception(code, false);
    }

    /// Check if any enabled hardware interrupt (`INT0`, `INT1`, `INT5`) is pending and deliverable,
    /// and if so, enter the Level 1 Interrupt vector (`0x8000_0200` / `0xBFC0_0400`, `ps2tek` `#eecop0`).
    #[inline]
    pub fn check_and_service_interrupts(&mut self, bus: &Bus) -> bool {
        let status = self.cop0_regs[cop0_reg::STATUS];
        let ie_enabled =
            (status & (status_bits::IE | status_bits::EIE)) == (status_bits::IE | status_bits::EIE);
        let not_in_exception = (status & (status_bits::EXL | status_bits::ERL)) == 0;
        if !ie_enabled || !not_in_exception {
            return false;
        }

        let cause = self.read_cop0(cop0_reg::CAUSE, bus);
        if (status & cause & status_bits::INT_MASK_ALL) != 0 {
            self.cop0_regs[cop0_reg::CAUSE] = cause;
            self.raise_level1_exception(Level1ExceptionCode::Interrupt, false);
            true
        } else {
            false
        }
    }

    /// Execute `ERET` (`0x4200_0018`, `ps2tek` `https://psi-rockin.github.io/ps2tek/#eecop0instructions`).
    #[inline]
    pub fn execute_eret(&mut self) {
        let status = self.cop0_regs[cop0_reg::STATUS];
        if (status & status_bits::ERL) != 0 {
            self.pc = self.cop0_regs[cop0_reg::ERROR_EPC];
            self.cop0_regs[cop0_reg::STATUS] = status & !status_bits::ERL;
        } else {
            self.pc = self.cop0_regs[cop0_reg::EPC];
            self.cop0_regs[cop0_reg::STATUS] = status & !status_bits::EXL;
        }
        self.next_pc = self.pc.wrapping_add(4);
        self.in_delay_slot = false;
    }

    /// Execute `EI` (`0x4200_0038`, `ps2tek` `#eecop0`).
    #[inline]
    pub fn execute_ei(&mut self) {
        let status = self.cop0_regs[cop0_reg::STATUS];
        if (status & status_bits::EDI) != 0 || self.in_kernel_mode() {
            self.cop0_regs[cop0_reg::STATUS] = status | status_bits::EIE;
        }
    }

    /// Execute `DI` (`0x4200_0039`, `ps2tek` `#eecop0`).
    #[inline]
    pub fn execute_di(&mut self) {
        let status = self.cop0_regs[cop0_reg::STATUS];
        if (status & status_bits::EDI) != 0 || self.in_kernel_mode() {
            self.cop0_regs[cop0_reg::STATUS] = status & !status_bits::EIE;
        }
    }

    /// Execute `TLBR` (`0x4200_0001`): load COP0 `PageMask`, `EntryHi`, `EntryLo0`, `EntryLo1`
    /// from `bus.tlb.entries[Index]` (`ps2tek` `#eecop0memorymanagement`).
    pub fn execute_tlbr(&mut self, bus: &Bus) {
        let idx = (self.cop0_regs[cop0_reg::INDEX] & 0x3F) as usize;
        if let Some(entry) = bus.tlb.entries.get(idx) {
            self.cop0_regs[cop0_reg::PAGE_MASK] = entry.page_mask;
            self.cop0_regs[cop0_reg::ENTRY_HI] = entry.entry_hi & !(entry.page_mask);
            let g_bit = (entry.entry_lo0 & 1) & (entry.entry_lo1 & 1);
            self.cop0_regs[cop0_reg::ENTRY_LO0] = (entry.entry_lo0 & !1) | g_bit;
            self.cop0_regs[cop0_reg::ENTRY_LO1] = (entry.entry_lo1 & !1) | g_bit;
        }
    }

    /// Execute `TLBWI` (`0x4200_0002`): write COP0 TLB registers to `bus.tlb.entries[Index]`
    /// and update `bus.vtlb` (`ps2tek` `#eecop0memorymanagement`).
    pub fn execute_tlbwi(&self, bus: &mut Bus) {
        let idx = (self.cop0_regs[cop0_reg::INDEX] & 0x3F) as usize;
        let entry = self.current_tlb_entry_from_cop0();
        bus.set_tlb_entry(idx, entry);
    }

    /// Execute `TLBWR` (`0x4200_0006`): write COP0 TLB registers to `bus.tlb.entries[Random]`
    /// and update `bus.vtlb` (`ps2tek` `#eecop0memorymanagement`).
    pub fn execute_tlbwr(&self, bus: &mut Bus) {
        let idx = (self.read_cop0(cop0_reg::RANDOM, bus) & 0x3F) as usize;
        let entry = self.current_tlb_entry_from_cop0();
        bus.set_tlb_entry(idx, entry);
    }

    /// Execute `TLBP` (`0x4200_0008`): probe `bus.tlb` for a match with `EntryHi`, setting
    /// `Index` (`0..48` on hit, or `0x8000_0000` on miss, `ps2tek` `#eecop0memorymanagement`).
    pub fn execute_tlbp(&mut self, bus: &Bus) {
        let entry_hi = self.cop0_regs[cop0_reg::ENTRY_HI];
        let vaddr = entry_hi & 0xFFFF_E000;
        let asid = (entry_hi & 0xFF) as u8;
        match bus.tlb.probe(vaddr, asid) {
            Some(idx) => self.cop0_regs[cop0_reg::INDEX] = idx as u32,
            None => self.cop0_regs[cop0_reg::INDEX] = 0x8000_0000,
        }
    }

    #[inline]
    const fn current_tlb_entry_from_cop0(&self) -> TlbEntry {
        TlbEntry {
            page_mask: self.cop0_regs[cop0_reg::PAGE_MASK],
            entry_hi: self.cop0_regs[cop0_reg::ENTRY_HI],
            entry_lo0: self.cop0_regs[cop0_reg::ENTRY_LO0],
            entry_lo1: self.cop0_regs[cop0_reg::ENTRY_LO1],
        }
    }
}
