//! Emotion Engine Interrupt Controller (`INTC`, `0x1000_F000` / `0x1000_F010`).
//!
//! Primary source reference:
//! - `ps2tek` ("EE Interrupt Controller (INTC)"):
//!   * `INTC_STAT` (`0x1000_F000`): Bits `0..=14` indicate pending interrupt causes.
//!     Writing `1` to any bit **clears** that bit (`stat &= !val`).
//!   * `INTC_MASK` (`0x1000_F010`): Bits `0..=14` enable/disable interrupt causes.
//!     Writing `1` to any bit **toggles (flips)** that bit (`mask ^= val`).
//!   * The `INT0` interrupt line to EE `COP0` (`Cause.IP2`, bit 10) is asserted whenever
//!     `(INTC_STAT & INTC_MASK) != 0`.

use bitflags::bitflags;

bitflags! {
    /// Emotion Engine `INTC` interrupt cause/mask bit positions (`ps2tek` "EE INTC").
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub struct IntcFlags: u32 {
        /// Bit 0: Graphics Synthesizer (GS) interrupt.
        const GS           = 1 << 0;
        /// Bit 1: SBUS (IOP-to-EE) interrupt.
        const SBUS         = 1 << 1;
        /// Bit 2: Vertical Blank Start (`VBON`).
        const VBLANK_START = 1 << 2;
        /// Bit 3: Vertical Blank End (`VBOF`).
        const VBLANK_END   = 1 << 3;
        /// Bit 4: VIF0 interrupt.
        const VIF0         = 1 << 4;
        /// Bit 5: VIF1 interrupt.
        const VIF1         = 1 << 5;
        /// Bit 6: VU0 interrupt.
        const VU0          = 1 << 6;
        /// Bit 7: VU1 interrupt.
        const VU1          = 1 << 7;
        /// Bit 8: Image Processing Unit (IPU) interrupt.
        const IPU          = 1 << 8;
        /// Bit 9: EE Timer 0 interrupt.
        const TIM0         = 1 << 9;
        /// Bit 10: EE Timer 1 interrupt.
        const TIM1         = 1 << 10;
        /// Bit 11: EE Timer 2 interrupt.
        const TIM2         = 1 << 11;
        /// Bit 12: EE Timer 3 interrupt.
        const TIM3         = 1 << 12;
        /// Bit 13: SIF FIFO error interrupt (`SFIFO`).
        const SFIFO        = 1 << 13;
        /// Bit 14: VU0 Watchdog interrupt (`VU0WD`).
        const VU0WD        = 1 << 14;
    }
}

/// Mask of valid 15-bit `INTC` registers (`bits 0..=14`).
const INTC_VALID_MASK: u32 = 0x7FFF;

/// Emotion Engine Interrupt Controller state.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EeIntc {
    /// `INTC_STAT` (`0x1000_F000`).
    pub stat: u32,
    /// `INTC_MASK` (`0x1000_F010`).
    pub mask: u32,
}

impl EeIntc {
    pub const fn new() -> Self {
        Self { stat: 0, mask: 0 }
    }

    /// Read `INTC_STAT` (`0x1000_F000`).
    #[inline(always)]
    pub const fn read_stat(&self) -> u32 {
        self.stat & INTC_VALID_MASK
    }

    /// Write `INTC_STAT` (`0x1000_F000`): write-1-to-clear (`ps2tek`).
    #[inline(always)]
    pub fn write_stat(&mut self, val: u32) {
        self.stat &= !(val & INTC_VALID_MASK);
    }

    /// Read `INTC_MASK` (`0x1000_F010`).
    #[inline(always)]
    pub const fn read_mask(&self) -> u32 {
        self.mask & INTC_VALID_MASK
    }

    /// Write `INTC_MASK` (`0x1000_F010`): write-1-to-toggle (`ps2tek`).
    #[inline(always)]
    pub fn write_mask(&mut self, val: u32) {
        self.mask ^= val & INTC_VALID_MASK;
    }

    /// Raise one or more hardware interrupt lines in `INTC_STAT`.
    #[inline(always)]
    pub fn raise_irq(&mut self, flags: IntcFlags) {
        self.stat |= flags.bits() & INTC_VALID_MASK;
    }

    /// Returns `true` if `INT0` should be asserted on EE `COP0` (`(stat & mask) != 0`).
    #[inline(always)]
    pub const fn int0_pending(&self) -> bool {
        (self.stat & self.mask & INTC_VALID_MASK) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intc_stat_write_one_to_clear_and_mask_write_one_to_toggle() {
        let mut intc = EeIntc::new();
        intc.raise_irq(IntcFlags::VBLANK_START | IntcFlags::TIM0);
        assert_eq!(intc.read_stat(), (1 << 2) | (1 << 9));
        assert!(!intc.int0_pending());

        // Writing 1 toggles INTC_MASK bits on.
        intc.write_mask(IntcFlags::VBLANK_START.bits());
        assert_eq!(intc.read_mask(), 1 << 2);
        assert!(intc.int0_pending());

        // Writing 1 to INTC_STAT clears VBLANK_START while leaving TIM0 set.
        intc.write_stat(IntcFlags::VBLANK_START.bits());
        assert_eq!(intc.read_stat(), 1 << 9);
        assert!(!intc.int0_pending());

        // Writing 1 to INTC_MASK again toggles VBLANK_START back off.
        intc.write_mask(IntcFlags::VBLANK_START.bits());
        assert_eq!(intc.read_mask(), 0);
    }
}
