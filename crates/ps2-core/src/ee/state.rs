//! Emotion Engine (MIPS R5900) Architectural State (`#[repr(C, align(64))]`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#ee` & `https://psi-rockin.github.io/ps2tek/#eecop0`):
//!   * 32 x 128-bit General Purpose Registers (`GPR[0..32]`), with `GPR[0]` (`$zero`) hardwired to `0`.
//!   * Scalar ALU, branch, shift, load/store (`< 128`-bit), and COP0 instructions operate on the
//!     lower 64 bits (`bits 0..=63`) while preserving the upper 64 bits (`bits 64..=127`).
//!   * 128-bit `HI` and `LO` registers: `HI0`/`LO0` (`bits 0..=63`) hold the primary multiply/divide
//!     results; `HI1`/`LO1` (`bits 64..=127`) hold the MMI pipeline 1 multiply/divide results.
//!   * `SA` (Shift Amount) register for quadword funnel shifts (`QFSRV`).
//!   * Reset vector: `0xBFC0_0000` (`KSEG1` uncached BIOS ROM base).

use ps2_common::Reg128;

use super::cop0::{cop0_reg, status_bits};

/// Reset vector address on the Emotion Engine (`ps2tek` `#memorymap`).
pub const EE_RESET_PC: u32 = 0xBFC0_0000;

/// R5900 `COP0.PRId` (`$15`) processor revision identifier (`0x0000_2E20`, `ps2tek` `#eecop0`).
pub const EE_COP0_PRID_DEFAULT: u32 = 0x0000_2E20;

/// R5900 `COP1.FCR0` implementation and revision register (`0x0000_2E30`).
///
/// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` line 3).
pub const EE_COP1_FCR0_DEFAULT: u32 = 0x0000_2E30;

/// R5900 `COP1.FCR31` initial control/status register (`0x0100_0001`: bit 24 `FS` and bit 0 `RM` hardwired to `1`).
///
/// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` line 3
/// & `local/ps2autotests/tests/cpu/ee_fpu/transfer.expected` line 18).
pub const EE_COP1_FCR31_DEFAULT: u32 = 0x0100_0001;

/// Cache-line-aligned (`64`-byte) Emotion Engine CPU state (`docs/PS2_BLUEPRINT.md` §3 & §12).
///
/// Kept `#[repr(C, align(64))]` so the Phase 3 Cranelift JIT can index hot registers directly
/// by fixed struct offset (`AGENTS.md`).
#[repr(C, align(64))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EeCpuState {
    /// 32 x 128-bit General Purpose Registers (`$zero` at index 0 is hardwired to `0`).
    pub gpr: [Reg128; 32],
    /// 128-bit `HI` register (`bits 0..63` = `HI0`, `bits 64..127` = `HI1`).
    pub hi: Reg128,
    /// 128-bit `LO` register (`bits 0..63` = `LO0`, `bits 64..127` = `LO1`).
    pub lo: Reg128,
    /// Shift Amount (`SA`) register: stores byte/bit shift count (`0..=127`, `MTSAB`/`MTSAH`/`MTSA`).
    pub sa: u32,
    /// Current instruction virtual address (`PC`).
    pub pc: u32,
    /// Next instruction virtual address (`NPC`, defaults to `pc + 4` unless a branch/jump is pending).
    pub next_pc: u32,
    /// `true` when the instruction at `self.pc` is executing in a branch/jump delay slot (`Cause.BD`).
    pub in_delay_slot: bool,
    /// Downcounting EE cycle budget for the current timeslice (`docs/PS2_BLUEPRINT.md` §3).
    pub cycle_countdown: i32,
    /// 32 x 32-bit COP0 system control registers (`Index`, `Status`, `Cause`, `EPC`, `ErrorEPC`, etc.).
    pub cop0_regs: [u32; 32],
    /// COP0 Performance Counter Control Register (`PCCR`, `MFPS`/`MTPS`).
    pub cop0_pccr: u32,
    /// COP0 Performance Counter Registers 0 and 1 (`PCR0`/`PCR1`, `MFPC`/`MTPC`).
    pub cop0_pcr: [u32; 2],
    /// External `INT1` (DMAC) interrupt line state (`Cause` bit 11).
    pub int1_line: bool,
    /// External CPCOND0 condition line connected to `BC0F`/`BC0T` (`true` = DMAC stall/condition met).
    pub cpcond0: bool,
    /// 32 x 32-bit COP1 Floating-Point Registers (`FPR[0..32]`, populated in Task 4).
    pub fpr: [u32; 32],
    /// COP1 Implementation and Revision Register (`FCR0`).
    pub fcr0: u32,
    /// COP1 Control/Status Register (`FCR31`).
    pub fcr31: u32,
    /// COP1 Floating-Point Accumulator (`ACC`).
    pub facc: u32,
}

impl Default for EeCpuState {
    fn default() -> Self {
        Self::new()
    }
}

impl EeCpuState {
    /// Create a newly reset Emotion Engine CPU state at `PC = 0xBFC0_0000`.
    ///
    /// Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eecop0`):
    /// - `COP0.Status` starts with `BEV = 1` (bit 22) and `ERL = 1` (bit 2) on hardware reset.
    /// - `COP0.PRId` (`$15`) is initialized to `0x0000_2E20`.
    pub const fn new() -> Self {
        let mut cop0_regs = [0u32; 32];
        cop0_regs[cop0_reg::STATUS] = status_bits::BEV
            | status_bits::ERL
            | status_bits::CU0
            | status_bits::CU1
            | status_bits::CU2;
        cop0_regs[cop0_reg::PRID] = EE_COP0_PRID_DEFAULT;

        Self {
            gpr: [Reg128::ZERO; 32],
            hi: Reg128::ZERO,
            lo: Reg128::ZERO,
            sa: 0,
            pc: EE_RESET_PC,
            next_pc: EE_RESET_PC.wrapping_add(4),
            in_delay_slot: false,
            cycle_countdown: 0,
            cop0_regs,
            cop0_pccr: 0,
            cop0_pcr: [0; 2],
            int1_line: false,
            cpcond0: true,
            fpr: [0; 32],
            fcr0: EE_COP1_FCR0_DEFAULT,
            fcr31: EE_COP1_FCR31_DEFAULT,
            facc: 0,
        }
    }

    /// Read the low 32-bit unsigned word of `GPR[reg]`.
    #[inline(always)]
    pub const fn gpr_u32(&self, reg: usize) -> u32 {
        if reg == 0 {
            0
        } else {
            self.gpr[reg & 31].lo_u32()
        }
    }

    /// Read the low 32-bit signed word of `GPR[reg]`.
    #[inline(always)]
    pub const fn gpr_i32(&self, reg: usize) -> i32 {
        self.gpr_u32(reg) as i32
    }

    /// Read the low 64-bit unsigned doubleword of `GPR[reg]`.
    #[inline(always)]
    pub const fn gpr_u64(&self, reg: usize) -> u64 {
        if reg == 0 {
            0
        } else {
            self.gpr[reg & 31].lo_u64()
        }
    }

    /// Read the low 64-bit signed doubleword of `GPR[reg]`.
    #[inline(always)]
    pub const fn gpr_i64(&self, reg: usize) -> i64 {
        self.gpr_u64(reg) as i64
    }

    /// Read the full 128-bit value of `GPR[reg]`.
    #[inline(always)]
    pub const fn gpr_128(&self, reg: usize) -> Reg128 {
        if reg == 0 {
            Reg128::ZERO
        } else {
            self.gpr[reg & 31]
        }
    }

    /// Sign-extend a 32-bit value to 64 bits and write it to the lower 64 bits of `GPR[reg]`,
    /// preserving bits `64..=127` (ignored if `reg == 0`, `ps2tek` `#ee`).
    #[inline(always)]
    pub fn set_gpr_sext32(&mut self, reg: usize, val: i32) {
        let idx = reg & 31;
        if idx != 0 {
            self.gpr[idx].set_sext32(val);
        }
    }

    /// Write a 64-bit value to the lower 64 bits of `GPR[reg]`, preserving bits `64..=127`
    /// (ignored if `reg == 0`, `ps2tek` `#ee`).
    #[inline(always)]
    pub fn set_gpr_u64(&mut self, reg: usize, val: u64) {
        let idx = reg & 31;
        if idx != 0 {
            self.gpr[idx].set_lo_u64(val);
        }
    }

    /// Write a full 128-bit quadword to `GPR[reg]` (ignored if `reg == 0`).
    #[inline(always)]
    pub fn set_gpr_128(&mut self, reg: usize, val: Reg128) {
        let idx = reg & 31;
        if idx != 0 {
            self.gpr[idx] = val;
        }
    }
}
