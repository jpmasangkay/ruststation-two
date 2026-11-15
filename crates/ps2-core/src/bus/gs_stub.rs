//! Graphics Synthesizer (GS) Privileged Registers Stub (`0x1200_0000..=0x1200_1FFF`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iomaps` & `https://psi-rockin.github.io/ps2tek/#gs`):
//!   * Mapped into the EE physical address space at `0x1200_0000` in 64-bit register slots:
//!     `0x1200_0000` `PMODE`, `0x1200_0010` `SMODE1`, `0x1200_0020` `SMODE2`, `0x1200_0030` `SRFSH`,
//!     `0x1200_0040` `SYNCH1`, `0x1200_0050` `SYNCH2`, `0x1200_0060` `SYNCV`, `0x1200_0070` `DISPFB1`,
//!     `0x1200_0080` `DISPLAY1`, `0x1200_0090` `DISPFB2`, `0x1200_00A0` `DISPLAY2`, `0x1200_00B0` `EXTBUF`,
//!     `0x1200_00C0` `EXTDATA`, `0x1200_00D0` `EXTWRITE`, `0x1200_00E0` `BGCOLOR`, `0x1200_1000` `GS_CSR`,
//!     `0x1200_1010` `GS_IMR`, `0x1200_1040` `BUSDIR`, `0x1200_1080` `SIGLBLID`.
//! - `ps2autotests` (`tests/gs/gsregs.h` & `tests/gs/signal.cpp`, `FETCHED`):
//!   * `GS_CSR` (`0x1200_1000`) bit 0 is `SIGNAL` (acknowledged/cleared by writing `1` to bit 0).
//!   * Single `SIGNAL` rule: writing `1` to bit 0 of `GS_CSR` (`0x1200_1000`) clears `GS_CSR.SIGNAL`
//!     (`0`), while writing `0` leaves it unchanged; writing to internal GS register `SIGNAL` (`0x60`)
//!     with `(ID = data[31:0], IDMSK = data[63:32])` updates
//!     `SIGLBLID[31:0] = (SIGLBLID[31:0] & !IDMSK) | (ID & IDMSK)` and sets `GS_CSR.SIGNAL = 1`.
//! - `ps2sdk` (`ee/libgs/include/libgs.h` lines 504–565, `struct GS_CSR` and `struct GS_IMR`, `FETCHED`):
//!   * `GS_CSR` (`0x1200_1000`) bit layout (`SEARCH-ONLY` beyond `SIGNAL`/`FINISH`):
//!     - bit 0: `signal_evnt` (`SIGNAL`)
//!     - bit 1: `finish_evnt` (`FINISH`)
//!     - bit 2: `hsint_intrupt` (`HSINT`)
//!     - bit 3: `vsint_intrupt` (`VSINT`)
//!     - bit 4: `write_terminate` (`EDWINT`)
//!     - bit 5..6: `exhsint`, `exvsint`
//!     - bit 8: `flush`, bit 9: `reset`, bit 10: `exverr`, bit 11: `exfield`, bit 12: `nfield`
//!     - bit 13: `current_field` (`FIELD`)
//!     - bits 14..15: `fifo_status` (`FIFO`, `0b01` = empty)
//!     - bits 16..23: `gs_rev_number` (`REV`, `0x1B`, `SEARCH-ONLY`/`GUESS`)
//!     - bits 24..31: `gs_id` (`ID`, `0x55`, `SEARCH-ONLY`/`GUESS`)

use crate::bus::intc::{EeIntc, IntcFlags};
pub use ps2_gs::gs_reg;

/// Default read-only `REV` (`0x1B`) and `ID` (`0x55`) plus `FIFO = Empty` (`1 << 14`) in `GS_CSR`
/// (`ps2sdk` `ee/libgs/include/libgs.h`, `SEARCH-ONLY`/`GUESS`).
const GS_CSR_FIXED_BITS: u64 = (0x55u64 << 24) | (0x1Bu64 << 16) | (1u64 << 14);

/// GS Privileged Registers (`0x1200_0000..=0x1200_1FFF`) + forwarding into [`ps2_gs::GsState`].
///
/// `SIGNAL`/`FINISH`/`LABEL`/`CSR`/`IMR`/`SIGLBLID` interrupt & stall behavior stay here so EE
/// timing and Phase 2a hardware gates remain exact (`gs/label`, `gs/signal`), while internal GS
/// registers (`0x00..=0x62`) and PCRTC display writes forward to `core`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GsPrivilegedStub {
    pub pmode: u64,
    pub smode1: u64,
    pub smode2: u64,
    pub srfsh: u64,
    pub synch1: u64,
    pub synch2: u64,
    pub syncv: u64,
    pub dispfb1: u64,
    pub display1: u64,
    pub dispfb2: u64,
    pub display2: u64,
    pub extbuf: u64,
    pub extdata: u64,
    pub extwrite: u64,
    pub bgcolor: u64,
    pub csr: u64,
    pub imr: u64,
    pub busdir: u64,
    pub siglblid: u64,
    /// Shared GS core state (decoded internal/display registers + VRAM + transfers).
    pub core: ps2_gs::GsState,
    /// Total internal GS register writes accepted from GIF paths.
    pub internal_write_count: u64,
    /// Total `HWREG` (`0x54`) writes accepted (`IMAGE` mode).
    pub hwreg_write_count: u64,
}

impl Default for GsPrivilegedStub {
    fn default() -> Self {
        Self::new()
    }
}

impl GsPrivilegedStub {
    pub fn new() -> Self {
        Self {
            pmode: 0,
            smode1: 0,
            smode2: 0,
            srfsh: 0,
            synch1: 0,
            synch2: 0,
            syncv: 0,
            dispfb1: 0,
            display1: 0,
            dispfb2: 0,
            display2: 0,
            extbuf: 0,
            extdata: 0,
            extwrite: 0,
            bgcolor: 0,
            csr: GS_CSR_FIXED_BITS,
            imr: 0x7F00,
            busdir: 0,
            siglblid: 0,
            core: ps2_gs::GsState::new(),
            internal_write_count: 0,
            hwreg_write_count: 0,
        }
    }

    /// Return `true` when `GS_BUSDIR` bit 0 selects `Local->Host` (`GS->EE`, `SEARCH-ONLY`).
    #[inline(always)]
    pub const fn busdir_gs_to_ee(&self) -> bool {
        (self.busdir & 1) != 0
    }

    /// Return `true` when `GS_CSR.SIGNAL` (bit 0) is set (`SIGNAL` stall active until cleared).
    #[inline(always)]
    pub const fn signal_stalled(&self) -> bool {
        (self.csr & 1) != 0
    }

    /// Return `true` if internal GS register `reg` can currently be written without stalling.
    ///
    /// A second `SIGNAL` (`0x60`) stalls GS/GIF while `GS_CSR.SIGNAL == 1` (`SEARCH-ONLY`).
    #[inline(always)]
    pub const fn can_accept_internal_reg(&self, reg: u8) -> bool {
        reg != gs_reg::SIGNAL || (self.csr & 1) == 0
    }

    /// Read the most recent 64-bit value written to internal GS register `reg` (`0x00..=0x7F`).
    #[inline(always)]
    pub fn internal_reg(&self, reg: u8) -> u64 {
        self.core.internal_reg(reg)
    }

    /// Write a 64-bit internal GS register (`0x00..=0x7F`) delivered via GIF.
    ///
    /// Returns `false` if `reg == 0x60` (`SIGNAL`) while `GS_CSR.SIGNAL` (bit 0) is already `1`
    /// (**stall**: `SIGLBLID` and counters are left untouched until EE clears `GS_CSR.SIGNAL`,
    /// `SEARCH-ONLY`). Otherwise applies GS side effects (`SIGNAL`, `FINISH`, `LABEL`, `HWREG`)
    /// and raises `IntcFlags::GS` if unmasked by `GS_IMR`.
    pub fn write_internal_reg(&mut self, reg: u8, data: u64, intc: &mut EeIntc) -> bool {
        if reg == gs_reg::SIGNAL && (self.csr & 1) != 0 {
            return false;
        }
        self.internal_write_count += 1;
        self.core.write_internal_reg(reg, data);
        match reg {
            gs_reg::HWREG => {
                self.hwreg_write_count += 1;
            }
            gs_reg::SIGNAL => {
                let id = data & 0xFFFF_FFFF;
                let idmsk = data >> 32;
                let old_sig = self.siglblid & 0xFFFF_FFFF;
                let new_sig = (old_sig & !idmsk) | (id & idmsk);
                self.siglblid = (self.siglblid & 0xFFFF_FFFF_0000_0000) | new_sig;
                self.csr |= 1;
                if (self.imr & (1 << 8)) == 0 {
                    intc.raise_irq(IntcFlags::GS);
                }
            }
            gs_reg::FINISH => {
                // `SEARCH-ONLY`: setting GS_CSR.FINISH (bit 1) + GS IRQ gated by GS_IMR[9].
                self.csr |= 1 << 1;
                if (self.imr & (1 << 9)) == 0 {
                    intc.raise_irq(IntcFlags::GS);
                }
            }
            gs_reg::LABEL => {
                let id = data & 0xFFFF_FFFF;
                let idmsk = data >> 32;
                let old_lbl = self.siglblid >> 32;
                let new_lbl = (old_lbl & !idmsk) | (id & idmsk);
                self.siglblid = (self.siglblid & 0xFFFF_FFFF) | (new_lbl << 32);
            }
            _ => {}
        }
        true
    }

    /// Toggle `GS_CSR.FIELD` (bit 13) and set `GS_CSR.VSINT` (bit 3) on VBlank start
    /// (`ps2sdk` `ee/libgs/include/libgs.h`, `FETCHED`).
    #[inline(always)]
    pub fn on_vblank_start(&mut self) {
        self.csr ^= 1 << 13;
        self.csr |= 1 << 3;
    }

    /// Set `GS_CSR.HSINT` (bit 2) on HBlank (`ps2sdk` `ee/libgs/include/libgs.h`, `FETCHED`).
    #[inline(always)]
    pub fn on_hblank(&mut self) {
        self.csr |= 1 << 2;
    }

    /// Read a 64-bit GS privileged register at `paddr` (`0x1200_0000..=0x1200_1FFF`).
    pub const fn read64(&self, paddr: u32) -> u64 {
        match paddr & 0x1FFF {
            0x0000 => self.pmode,
            0x0010 => self.smode1,
            0x0020 => self.smode2,
            0x0030 => self.srfsh,
            0x0040 => self.synch1,
            0x0050 => self.synch2,
            0x0060 => self.syncv,
            0x0070 => self.dispfb1,
            0x0080 => self.display1,
            0x0090 => self.dispfb2,
            0x00A0 => self.display2,
            0x00B0 => self.extbuf,
            0x00C0 => self.extdata,
            0x00D0 => self.extwrite,
            0x00E0 => self.bgcolor,
            0x1000 => self.csr | GS_CSR_FIXED_BITS,
            0x1010 => self.imr,
            0x1040 => self.busdir,
            0x1080 => self.siglblid,
            _ => 0,
        }
    }

    /// Write a 64-bit GS privileged register at `paddr` (`0x1200_0000..=0x1200_1FFF`).
    pub fn write64(&mut self, paddr: u32, val: u64) {
        let off = paddr & 0x1FFF;
        match off {
            0x0000 => self.pmode = val,
            0x0010 => self.smode1 = val,
            0x0020 => self.smode2 = val,
            0x0030 => self.srfsh = val,
            0x0040 => self.synch1 = val,
            0x0050 => self.synch2 = val,
            0x0060 => self.syncv = val,
            0x0070 => self.dispfb1 = val,
            0x0080 => self.display1 = val,
            0x0090 => self.dispfb2 = val,
            0x00A0 => self.display2 = val,
            0x00B0 => self.extbuf = val,
            0x00C0 => self.extdata = val,
            0x00D0 => self.extwrite = val,
            0x00E0 => self.bgcolor = val,
            0x1000 => {
                // Bits 0..=4 (SIGNAL, FINISH, HSINT, VSINT, EDWINT) write-1-to-clear
                // (`ps2autotests` `tests/gs/signal.cpp` & `ps2sdk` `ee/libgs/include/libgs.h`, `FETCHED`).
                let clear_mask = val & 0x1F;
                self.csr = (self.csr & !clear_mask) | GS_CSR_FIXED_BITS;
            }
            0x1010 => self.imr = val,
            0x1040 => self.busdir = val,
            0x1080 => self.siglblid = val,
            _ => {}
        }
        if off <= 0x00E0 {
            self.core.write_privileged_display(off, val);
        }
    }

    /// Read a 32-bit half of a 64-bit GS privileged register.
    #[inline]
    pub const fn read32(&self, paddr: u32) -> u32 {
        let val64 = self.read64(paddr & !0x7);
        if (paddr & 0x4) != 0 {
            (val64 >> 32) as u32
        } else {
            val64 as u32
        }
    }

    /// Write a 32-bit half of a 64-bit GS privileged register.
    #[inline]
    pub fn write32(&mut self, paddr: u32, val: u32) {
        let aligned = paddr & !0x7;
        let current = self.read64(aligned);
        let updated = if (paddr & 0x4) != 0 {
            if (aligned & 0x1FFF) == 0x1000 {
                // Upper 32-bit write to CSR does not clear low 5 interrupt bits.
                return;
            }
            (current & 0x0000_0000_FFFF_FFFF) | (u64::from(val) << 32)
        } else if (aligned & 0x1FFF) == 0x1000 {
            u64::from(val)
        } else {
            (current & 0xFFFF_FFFF_0000_0000) | u64::from(val)
        };
        self.write64(aligned, updated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs_signal_updates_sigid_sets_csr_and_stalls_second_signal_until_cleared() {
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        // Unmask SIGNAL IRQ (clear bit 8 of IMR) and write initial SIGLBLID.
        gs.write64(0x1200_1010, gs.imr & !(1 << 8));
        gs.write64(0x1200_1080, 0x7654_3210_0000_0000);

        // First SIGNAL succeeds.
        assert!(gs.write_internal_reg(gs_reg::SIGNAL, (0xF0 << 32) | 0xF0, &mut intc));
        assert_eq!(gs.csr & 1, 1);
        assert_eq!(gs.siglblid, 0x7654_3210_0000_00F0);
        assert_ne!(intc.stat & IntcFlags::GS.bits(), 0);

        // Second SIGNAL stalls while GS_CSR.SIGNAL == 1; SIGLBLID must not change.
        assert!(!gs.write_internal_reg(
            gs_reg::SIGNAL,
            (0xFFFF_FFFF << 32) | 0xDEAD_BEEF,
            &mut intc
        ));
        assert_eq!(gs.siglblid, 0x7654_3210_0000_00F0);

        // Clearing GS_CSR.SIGNAL (W1C bit 0) allows retry to succeed.
        gs.write64(0x1200_1000, 1);
        assert_eq!(gs.csr & 1, 0);
        gs.write64(0x1200_1080, 0x3333_3333_3333_3333);
        assert!(gs.write_internal_reg(gs_reg::SIGNAL, 0xF0 << 32, &mut intc));
        assert_eq!(gs.csr & 1, 1);
        assert_eq!(gs.siglblid, 0x3333_3333_3333_3303);
    }

    #[test]
    fn gs_label_and_finish_respect_masks_and_imr() {
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        gs.write64(0x1200_1080, 0x0000_0000_7654_3210);
        assert!(gs.write_internal_reg(gs_reg::LABEL, (0xF0 << 32) | 0xF0, &mut intc));
        assert_eq!(gs.siglblid, 0x0000_00F0_7654_3210);
        assert_eq!(gs.csr & 1, 0);

        gs.write64(0x1200_1080, 0x3333_3333_3333_3333);
        assert!(gs.write_internal_reg(gs_reg::LABEL, 0xF0 << 32, &mut intc));
        assert_eq!(gs.siglblid, 0x3333_3303_3333_3333);

        // FINISH with IMR[9] == 1 sets CSR[1] without raising GS IRQ.
        assert!(gs.write_internal_reg(gs_reg::FINISH, 0, &mut intc));
        assert_eq!(gs.csr & (1 << 1), 1 << 1);
        assert_eq!(intc.stat & IntcFlags::GS.bits(), 0);

        // Clear FINISH and unmask IMR[9] -> FINISH raises GS IRQ.
        gs.write64(0x1200_1000, 1 << 1);
        gs.write64(0x1200_1010, gs.imr & !(1 << 9));
        assert!(gs.write_internal_reg(gs_reg::FINISH, 0, &mut intc));
        assert_eq!(gs.csr & (1 << 1), 1 << 1);
        assert_ne!(intc.stat & IntcFlags::GS.bits(), 0);
    }
}
