//! R3000A I/O Processor (`IOP`) architectural state and COP0 system control coprocessor.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iop` & `#overview`):
//!   * 32-bit MIPS I (`R3000A`) CPU clocked at `36.864 MHz` in PS2 mode (`1/8` of the `294.912 MHz` EE clock).
//!   * `COP0.PRId` (`$15`) is `0x0000_001F` in PS2 mode.
//! - `ps2autotests` (`tests/cpu/iop/lsudelay.c`, `branchdelay.c`, `hilodelay.c`, `muldiv.c`):
//!   * Architectural 1-instruction load delay slot (`pending_load`).
//!   * Architectural 1-instruction branch delay slot (`pc`, `next_pc`, `in_delay_slot`).

/// Number of Emotion Engine (`294.912 MHz`) cycles per single R3000A IOP (`36.864 MHz`) cycle (`ps2tek` `#overview`).
pub const EE_CYCLES_PER_IOP_CYCLE: u64 = 8;

/// Reset vector (`0xBFC0_0000` in uncached `KSEG1`, `ps2tek` `#memorymap`).
pub const IOP_RESET_PC: u32 = 0xBFC0_0000;

/// Sentinel return address placed in `$ra` when invoking an `.irx` module's `_start` entry point.
pub const IOP_HLE_EXIT_RA: u32 = 0x0000_0000;

/// In-flight 1-instruction delayed load write (`ps2autotests` `tests/cpu/iop/lsudelay.c`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PendingLoad {
    /// Target GPR index (`1..=31`).
    pub reg: u8,
    /// 32-bit value to commit after the load delay slot instruction finishes.
    pub value: u32,
}

/// MIPS I (`R3000A`) exception codes (`Cause.ExcCode`, bits `6:2`).
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IopExceptionCode {
    /// Hardware interrupt (`Int = 0`).
    Interrupt = 0,
    /// Address error on load or instruction fetch (`AdEL = 4`).
    AddressErrorLoad = 4,
    /// Address error on store (`AdES = 5`).
    AddressErrorStore = 5,
    /// System call (`Syscall = 8`).
    Syscall = 8,
    /// Breakpoint (`Bp = 9`).
    Breakpoint = 9,
    /// Reserved instruction (`RI = 10`).
    ReservedInstruction = 10,
    /// Coprocessor unusable (`CpU = 11`).
    CoprocessorUnusable = 11,
    /// Arithmetic overflow (`Ov = 12`).
    Overflow = 12,
}

/// R3000A System Control Coprocessor (`COP0`) register file.
#[repr(C)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IopCop0 {
    /// `$3` `BPC`: Breakpoint on execute address.
    pub bpc: u32,
    /// `$5` `BDA`: Breakpoint on data access address.
    pub bda: u32,
    /// `$6` `JUMPDEST`: Memorized jump target address.
    pub jumpdest: u32,
    /// `$7` `DCIC`: Breakpoint control and status.
    pub dcic: u32,
    /// `$8` `BadVAddr`: Failing virtual address on `AdEL` / `AdES`.
    pub bad_vaddr: u32,
    /// `$9` `BDAM`: Data address breakpoint mask.
    pub bdam: u32,
    /// `$11` `BPCM`: Execute address breakpoint mask.
    pub bpcm: u32,
    /// `$12` `SR` (`Status`): Interrupt mask, `BEV` (`bit 22`), `IsC` (`bit 16`), and 3-level `KU`/`IE` stack (`bits 5:0`).
    pub status: u32,
    /// `$13` `Cause`: `BD` (`bit 31`), `CE` (`bits 29:28`), `IP` (`bits 15:8`), `ExcCode` (`bits 6:2`).
    pub cause: u32,
    /// `$14` `EPC`: Exception Program Counter.
    pub epc: u32,
    /// `$15` `PRId`: Processor Revision Identifier (`0x0000_001F` on PS2 IOP, `ps2tek` `#iop`).
    pub prid: u32,
}

impl Default for IopCop0 {
    fn default() -> Self {
        Self::new()
    }
}

impl IopCop0 {
    /// Initial `COP0.SR` value on reset (`BEV = 1` at bit 22).
    pub const RESET_STATUS: u32 = 1 << 22;
    /// PS2 IOP `COP0.PRId` value (`ps2tek` `#iop`).
    pub const PS2_IOP_PRID: u32 = 0x0000_001F;

    /// Create a reset-initialized [`IopCop0`].
    pub const fn new() -> Self {
        Self {
            bpc: 0,
            bda: 0,
            jumpdest: 0,
            dcic: 0,
            bad_vaddr: 0,
            bdam: 0,
            bpcm: 0,
            status: Self::RESET_STATUS,
            cause: 0,
            epc: 0,
            prid: Self::PS2_IOP_PRID,
        }
    }

    /// Read a 32-bit `COP0` register (`MFC0`).
    #[inline]
    pub const fn read_reg(&self, reg: u8) -> u32 {
        match reg {
            3 => self.bpc,
            5 => self.bda,
            6 => self.jumpdest,
            7 => self.dcic,
            8 => self.bad_vaddr,
            9 => self.bdam,
            11 => self.bpcm,
            12 => self.status,
            13 => self.cause,
            14 => self.epc,
            15 => self.prid,
            _ => 0,
        }
    }

    /// Write a 32-bit `COP0` register (`MTC0`).
    #[inline]
    pub fn write_reg(&mut self, reg: u8, val: u32) {
        match reg {
            3 => self.bpc = val,
            5 => self.bda = val,
            6 => {} // JUMPDEST is read-only
            7 => self.dcic = val,
            8 => {} // BadVAddr is read-only
            9 => self.bdam = val,
            11 => self.bpcm = val,
            12 => self.status = val,
            // Only software interrupt bits IP0..IP1 (bits 9:8) are writable in Cause.
            13 => self.cause = (self.cause & !0x0300) | (val & 0x0300),
            14 => self.epc = val,
            15 => {} // PRId is read-only
            _ => {}
        }
    }

    /// Enter an R3000A exception and return `(new_pc, new_next_pc)`.
    ///
    /// Pushes the 3-level `(KUo, IEo, KUp, IEp, KUc, IEc)` stack in `SR[5:0]` left by 2 bits
    /// (`KUc = 0` kernel mode, `IEc = 0` interrupts disabled).
    pub fn raise_exception(
        &mut self,
        code: IopExceptionCode,
        inst_pc: u32,
        in_delay_slot: bool,
        bad_vaddr: Option<u32>,
    ) -> (u32, u32) {
        if let Some(vaddr) = bad_vaddr {
            self.bad_vaddr = vaddr;
        }

        // Push 3-level KU/IE stack in SR[5:0]: [KUo, IEo, KUp, IEp, KUc, IEc] <= [KUp, IEp, KUc, IEc, 0, 0].
        let stack = (self.status & 0x0F) << 2;
        self.status = (self.status & !0x3F) | stack;

        // Set Cause.ExcCode (bits 6:2) and Cause.BD (bit 31).
        let bd_bit = if in_delay_slot { 1u32 << 31 } else { 0 };
        self.cause = (self.cause & !0x8000_007C) | bd_bit | ((code as u32) << 2);

        self.epc = if in_delay_slot {
            inst_pc.wrapping_sub(4)
        } else {
            inst_pc
        };

        let vector = if (self.status & (1 << 22)) != 0 {
            0xBFC0_0180
        } else {
            0x8000_0080
        };
        (vector, vector.wrapping_add(4))
    }

    /// Execute `RFE` (Return From Exception, `COP0` funct `0x10`).
    ///
    /// Pops the 3-level `(KU, IE)` stack in `SR[5:0]` right by 2 bits (`SR[3:0] = SR[5:2]`),
    /// leaving `SR[5:4]` (`KUo`, `IEo`) unchanged per MIPS I architecture.
    #[inline]
    pub fn rfe(&mut self) {
        let popped = (self.status >> 2) & 0x0F;
        self.status = (self.status & !0x0F) | popped;
    }

    /// Returns `true` if a hardware or software interrupt is currently enabled and pending.
    #[inline(always)]
    pub const fn is_interrupt_pending(&self) -> bool {
        let iec = (self.status & 1) != 0;
        let pending = (self.status & self.cause) & 0xFF00;
        iec && pending != 0
    }
}

/// R3000A I/O Processor (`IOP`) architectural state.
///
/// Cache-line aligned (`#[repr(C, align(64))]`) per `AGENTS.md`.
#[repr(C, align(64))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IopCpuState {
    /// Thirty-two 32-bit General Purpose Registers (`gpr[0]` is hardwired to `0`).
    pub gpr: [u32; 32],
    /// Current 32-bit Program Counter.
    pub pc: u32,
    /// Next 32-bit Program Counter (supports architectural 1-instruction branch delay slot).
    pub next_pc: u32,
    /// Multiply/Divide `HI` register (remainder for `DIV`/`DIVU`, high 32 bits for `MULT`/`MULTU`).
    pub hi: u32,
    /// Multiply/Divide `LO` register (quotient for `DIV`/`DIVU`, low 32 bits for `MULT`/`MULTU`).
    pub lo: u32,
    /// `true` if the instruction at `pc` is executing inside a branch delay slot.
    pub in_delay_slot: bool,
    /// Pending 1-instruction load delay write scheduled by the previous instruction.
    pub pending_load: Option<PendingLoad>,
    /// System Control Coprocessor (`COP0`).
    pub cop0: IopCop0,
    /// Total R3000A IOP instruction cycles executed (`1` IOP cycle = `8` EE cycles).
    pub cycle_count: u64,
}

impl Default for IopCpuState {
    fn default() -> Self {
        Self::new()
    }
}

impl IopCpuState {
    /// Create a reset-initialized [`IopCpuState`] at `pc = 0xBFC0_0000`.
    pub const fn new() -> Self {
        Self {
            gpr: [0; 32],
            pc: IOP_RESET_PC,
            next_pc: IOP_RESET_PC.wrapping_add(4),
            hi: 0,
            lo: 0,
            in_delay_slot: false,
            pending_load: None,
            cop0: IopCop0::new(),
            cycle_count: 0,
        }
    }

    /// Reset the IOP CPU to power-on state (`pc = 0xBFC0_0000`).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Set the IOP Program Counter (`pc`) and `next_pc = pc + 4`, clearing delay slot state.
    #[inline]
    pub fn set_pc(&mut self, entry_pc: u32) {
        self.pc = entry_pc;
        self.next_pc = entry_pc.wrapping_add(4);
        self.in_delay_slot = false;
        self.pending_load = None;
    }

    /// Read a 32-bit GPR (`0..32`). `gpr[0]` always returns `0`.
    #[inline(always)]
    pub const fn read_gpr(&self, reg: u8) -> u32 {
        if (reg & 0x1F) == 0 {
            0
        } else {
            self.gpr[(reg & 0x1F) as usize]
        }
    }

    /// Write a 32-bit GPR (`0..32`). Writes to `gpr[0]` are ignored.
    #[inline(always)]
    pub fn write_gpr(&mut self, reg: u8, val: u32) {
        let idx = (reg & 0x1F) as usize;
        if idx != 0 {
            self.gpr[idx] = val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iop_cpu_state_is_64_byte_aligned_and_r0_is_hardwired_zero() {
        assert_eq!(std::mem::align_of::<IopCpuState>(), 64);
        let mut iop = IopCpuState::new();
        assert_eq!(iop.pc, IOP_RESET_PC);
        assert_eq!(iop.cop0.read_reg(15), IopCop0::PS2_IOP_PRID);

        iop.write_gpr(0, 0xDEAD_BEEF);
        assert_eq!(iop.read_gpr(0), 0);
        iop.write_gpr(4, 0x1234_5678);
        assert_eq!(iop.read_gpr(4), 0x1234_5678);
    }

    #[test]
    fn iop_cop0_exception_and_rfe_push_pop_3_level_status_stack() {
        let mut cop0 = IopCop0::new();
        // Set KUc=1, IEc=1 (bits 1:0 = 0b11), BEV=0.
        cop0.write_reg(12, 0x0000_0003);
        let (vec_pc, vec_next) =
            cop0.raise_exception(IopExceptionCode::Syscall, 0x0001_0020, true, None);
        assert_eq!(vec_pc, 0x8000_0080);
        assert_eq!(vec_next, 0x8000_0084);
        assert_eq!(cop0.epc, 0x0001_001C);
        assert_ne!(cop0.cause & (1 << 31), 0);
        assert_eq!((cop0.cause >> 2) & 0x1F, IopExceptionCode::Syscall as u32);
        // KUp=1, IEp=1, KUc=0, IEc=0 => bits 5:0 = 0b001100 = 0x0C.
        assert_eq!(cop0.status & 0x3F, 0x0C);

        cop0.rfe();
        // Restored KUc=1, IEc=1 => bits 3:0 = 0b0011.
        assert_eq!(cop0.status & 0x0F, 0x03);
    }
}
