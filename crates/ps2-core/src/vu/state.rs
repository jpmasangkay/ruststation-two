//! Vector Unit (`VU0` / `VU1`) architectural register and flag state.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#vu`, `FETCHED`):
//!   * 32 128-bit vector float registers `vf00..vf31` (`vf00` hardwired to `(0.0, 0.0, 0.0, 1.0)`).
//!   * 128-bit accumulator `ACC`.
//!   * 16 16-bit integer registers `vi00..vi15` (`vi00` hardwired to `0`).
//!   * Special registers: `Status` (`vi16`), `MAC` (`vi17`), `Clipping` (`vi18`, 24-bit),
//!     `R` (`vi20`, 23-bit LFSR mantissa with implicit `0x3F80_0000` exponent), `I` (`vi21`),
//!     `Q` (`vi22`), `P` (VU1 EFU result), `TPC` (`vi26`), `CMSAR0` (`vi27`), `FBRST` (`vi28`),
//!     `VPU-STAT` (`vi29`), `CMSAR1` (`vi31`).
//! - `ps2autotests` (`tests/cpu/vu0_macro/transfer.cpp` & `transfer.expected`, `FETCHED`):
//!   * Verifies `VF00` hardwired value `0x3F800000_00000000_00000000_00000000`, `CFC2`/`CTC2`
//!     write masks for `vi00..vi31`, and `vi19` revision readback `0x0000_2E30`.

use ps2_common::Reg128;

use super::pipeline::VuPipeline;

/// Hardwired value of `vf00`: `x = 0.0, y = 0.0, z = 0.0, w = 1.0 (0x3F80_0000)`.
pub const VF00_VALUE: Reg128 = Reg128::from_u32x4(0, 0, 0, 0x3F80_0000);

/// Read-only revision ID returned by `CFC2` on control register `vi19`
/// (`ps2autotests` `tests/cpu/vu0_macro/transfer.expected` line 21, `FETCHED`).
pub const VU_REVISION_ID: u32 = 0x0000_2E30;

/// Destination mask bit for the `X` lane (word 0, bits `0..31`).
pub const DEST_X: u8 = 0b1000;
/// Destination mask bit for the `Y` lane (word 1, bits `32..63`).
pub const DEST_Y: u8 = 0b0100;
/// Destination mask bit for the `Z` lane (word 2, bits `64..95`).
pub const DEST_Z: u8 = 0b0010;
/// Destination mask bit for the `W` lane (word 3, bits `96..127`).
pub const DEST_W: u8 = 0b0001;

/// Cache-line-aligned architectural state of a single Vector Unit (`VU0` or `VU1`).
#[repr(C, align(64))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VuState {
    /// 32 128-bit vector floating-point registers (`vf[0]` is hardwired to [`VF00_VALUE`]).
    pub vf: [Reg128; 32],
    /// 128-bit floating-point accumulator (`ACC`).
    pub acc: Reg128,
    /// 16 16-bit integer registers (`vi[0]` is hardwired to `0`).
    pub vi: [u16; 16],
    /// 12-bit Status flag register (`vi16`): bits `0..=5` non-sticky (`Z, S, U, O, I, D`),
    /// bits `6..=11` sticky (`ZS, SS, US, OS, IS, DS`).
    pub status_flag: u16,
    /// 16-bit MAC flag register (`vi17`): bits `0..=3` `Zw..Zx`, `4..=7` `Sw..Sx`,
    /// `8..=11` `Uw..Ux`, `12..=15` `Ow..Ox`.
    pub mac_flag: u16,
    /// 24-bit Clipping flag shift register (`vi18`, masked with `0x00FF_FFFF`).
    pub clip_flag: u32,
    /// 23-bit LFSR mantissa (`vi20`, masked with `0x007F_FFFF`).
    pub r: u32,
    /// 32-bit Immediate float register (`vi21`).
    pub i: u32,
    /// 32-bit FDIV quotient/sqrt result register (`vi22`).
    pub q: u32,
    /// 32-bit EFU result register (`P`, `VU1` only).
    pub p: u32,
    /// Microprogram Counter in 8-byte LIW instruction units (`vi26` for `VU0`).
    pub tpc: u16,
    /// Micro subroutine call address register `CMSAR0` (`vi27`, 8-byte units).
    pub cmsar0: u16,
    /// Micro subroutine call address register `CMSAR1` (`vi31`, 8-byte units).
    pub cmsar1: u16,
    /// Control register `vi24` (mask `0x0C0C`).
    pub fbrst_ctrl: u16,
    /// Whether this VU is currently executing a microprogram.
    pub running: bool,
    /// Set when an instruction with the `E` (end) bit executes; halts after the next instruction.
    pub finish_after_slot: bool,
    /// Whether a delayed branch target is pending after the current delay slot instruction.
    pub branch_delay_active: bool,
    /// Pending branch target in 8-byte LIW instruction units.
    pub branch_delay_target: u16,
    /// Micro-mode pipeline timing and hazard tracker.
    pub pipeline: VuPipeline,
}

impl Default for VuState {
    fn default() -> Self {
        Self::new()
    }
}

impl VuState {
    /// Create a reset `VuState` with `vf[0] = (0.0, 0.0, 0.0, 1.0)` and `vi[0] = 0`.
    pub const fn new() -> Self {
        let mut vf = [Reg128::ZERO; 32];
        vf[0] = VF00_VALUE;
        Self {
            vf,
            acc: Reg128::ZERO,
            vi: [0; 16],
            status_flag: 0,
            mac_flag: 0,
            clip_flag: 0,
            r: 0,
            i: 0,
            q: 0,
            p: 0,
            tpc: 0,
            cmsar0: 0,
            cmsar1: 0,
            fbrst_ctrl: 0,
            running: false,
            finish_after_slot: false,
            branch_delay_active: false,
            branch_delay_target: 0,
            pipeline: VuPipeline::new(),
        }
    }

    /// Reset status/flags and execution state (triggered by `FBRST` reset bit).
    pub fn reset_unit(&mut self) {
        self.vf[0] = VF00_VALUE;
        self.vi[0] = 0;
        self.status_flag = 0;
        self.mac_flag = 0;
        self.clip_flag = 0;
        self.running = false;
        self.finish_after_slot = false;
        self.branch_delay_active = false;
        self.branch_delay_target = 0;
        self.pipeline.reset();
    }

    /// Read a 128-bit vector register `vf[idx]` (`vf[0]` always returns [`VF00_VALUE`]).
    #[inline(always)]
    pub const fn read_vf(&self, idx: usize) -> Reg128 {
        if (idx & 0x1F) == 0 {
            VF00_VALUE
        } else {
            self.vf[idx & 0x1F]
        }
    }

    /// Write all 128 bits of `vf[idx]` (writes to `vf[0]` are ignored).
    #[inline(always)]
    pub fn write_vf(&mut self, idx: usize, val: Reg128) {
        let reg = idx & 0x1F;
        if reg != 0 {
            self.vf[reg] = val;
        }
    }

    /// Write a single 32-bit lane (`0 = X, 1 = Y, 2 = Z, 3 = W`) of `vf[idx]` (writes to `vf[0]` are ignored).
    #[inline(always)]
    pub fn write_vf_lane(&mut self, idx: usize, lane: usize, bits: u32) {
        let reg = idx & 0x1F;
        if reg != 0 {
            self.vf[reg].u32_lanes_mut()[lane & 3] = bits;
        }
    }

    /// Read a single 32-bit lane (`0 = X, 1 = Y, 2 = Z, 3 = W`) of `vf[idx]`.
    #[inline(always)]
    pub fn read_vf_lane(&self, idx: usize, lane: usize) -> u32 {
        self.read_vf(idx).u32_lanes()[lane & 3]
    }

    /// Read a 16-bit integer register `vi[idx]` (`vi[0]` always returns `0`).
    #[inline(always)]
    pub const fn read_vi(&self, idx: usize) -> u16 {
        if (idx & 0xF) == 0 {
            0
        } else {
            self.vi[idx & 0xF]
        }
    }

    /// Write a 16-bit integer register `vi[idx]` (writes to `vi[0]` are ignored).
    #[inline(always)]
    pub fn write_vi(&mut self, idx: usize, val: u16) {
        let reg = idx & 0xF;
        if reg != 0 {
            self.vi[reg] = val;
        }
    }

    /// Start microprogram execution at `start_tpc` (in 8-byte LIW instruction units).
    #[inline]
    pub fn start_micro(&mut self, start_tpc: u16, tpc_mask: u16) {
        self.tpc = start_tpc & tpc_mask;
        self.running = true;
        self.finish_after_slot = false;
        self.branch_delay_active = false;
        self.branch_delay_target = 0;
        self.pipeline.on_start_micro(self.clip_flag);
    }
}
