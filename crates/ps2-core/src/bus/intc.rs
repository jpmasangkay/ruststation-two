//! Emotion Engine (`EE`) and I/O Processor (`IOP`) Interrupt Controllers (`INTC`).
//!
//! Primary source references (`FETCHED`):
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeintc`, `local/ps2tek/index.html` lines 1635-1677):
//!   * `INTC_STAT` (`0x1000_F000`): Bits `0..=14` indicate pending EE interrupt causes.
//!     Writing `1` to any bit **clears** that bit (`stat &= !val`).
//!   * `INTC_MASK` (`0x1000_F010`): Bits `0..=14` enable/disable EE interrupt causes.
//!     Writing `1` to any bit **reverses (toggles)** that bit (`mask ^= val`).
//!   * The `INT0` interrupt line to EE `COP0` (`Cause` bit 10) is asserted whenever
//!     `(INTC_STAT & INTC_MASK) != 0`.
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iopint`, `local/ps2tek/index.html` lines 3910-3962):
//!   * `I_STAT` (`0x1F80_1070`): Read returns status (`1` = IRQ raised); write acknowledges
//!     interrupts (`0` = clear bit, `1` = no effect: `stat &= val`).
//!   * `I_MASK` (`0x1F80_1074`): Read/Write interrupt mask (`0` = disabled, `1` = enabled).
//!   * `I_CTRL` (`0x1F80_1078`): Bit 0 is global IOP interrupt enable. Reading `I_CTRL` returns
//!     bit 0 **and clears it**, disabling interrupts; writing sets or resets bit 0.
//! - `ps2sdk` (`iop/system/intrman/src/intrman.c` lines 838-875, 1216-1220, `FETCHED`):
//!   * Confirms `I_STAT` acknowledge via `sw ~bit, 0(0xBF801070)`, `I_CTRL` atomic read-and-disable
//!     at `0xBF80_1078`, and `COP0.Status` / `COP0.Cause` bit 10 (`IP2 = 0x0400`) as the hardware
//!     interrupt line (`ori $t0, $t0, 0x404`), whereas `COP0.Cause` bits `9:8` (`0x0300`) are software
//!     interrupts.

use bitflags::bitflags;

bitflags! {
    /// Emotion Engine `INTC` interrupt cause/mask bit positions (`ps2tek` `#eeintc`, `FETCHED`).
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

bitflags! {
    /// R3000A IOP `I_STAT` / `I_MASK` interrupt bit positions (`ps2tek` `#iopint` lines 3922-3948, `FETCHED`).
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub struct IopIrqFlags: u32 {
        /// Bit 0 (`IRQ0`): VBLANK start.
        const VBLANK_START = 1 << 0;
        /// Bit 1 (`IRQ1`): GPU (used in PSX mode).
        const GPU          = 1 << 1;
        /// Bit 2 (`IRQ2`): CDVD Drive.
        const CDVD         = 1 << 2;
        /// Bit 3 (`IRQ3`): IOP DMA controller.
        const DMA          = 1 << 3;
        /// Bit 4 (`IRQ4`): IOP Timer 0.
        const TMR0         = 1 << 4;
        /// Bit 5 (`IRQ5`): IOP Timer 1.
        const TMR1         = 1 << 5;
        /// Bit 6 (`IRQ6`): IOP Timer 2.
        const TMR2         = 1 << 6;
        /// Bit 7 (`IRQ7`): SIO0.
        const SIO0         = 1 << 7;
        /// Bit 8 (`IRQ8`): SIO1.
        const SIO1         = 1 << 8;
        /// Bit 9 (`IRQ9`): SPU2.
        const SPU2         = 1 << 9;
        /// Bit 10 (`IRQ10`): PIO.
        const PIO          = 1 << 10;
        /// Bit 11 (`IRQ11`): VBLANK end.
        const VBLANK_END   = 1 << 11;
        /// Bit 12 (`IRQ12`): DVD.
        const DVD          = 1 << 12;
        /// Bit 13 (`IRQ13`): PCMCIA / DEV9 expansion slot.
        const DEV9         = 1 << 13;
        /// Bit 14 (`IRQ14`): IOP Timer 3.
        const TMR3         = 1 << 14;
        /// Bit 15 (`IRQ15`): IOP Timer 4.
        const TMR4         = 1 << 15;
        /// Bit 16 (`IRQ16`): IOP Timer 5.
        const TMR5         = 1 << 16;
        /// Bit 17 (`IRQ17`): SIO2.
        const SIO2         = 1 << 17;
        /// Bit 22 (`IRQ22`): USB.
        const USB          = 1 << 22;
        /// Bit 24 (`IRQ24`): FireWire (`FWRE`).
        const FWRE         = 1 << 24;
        /// Bit 25 (`IRQ25`): FireWire DMA (`FDMA`).
        const FDMA         = 1 << 25;
    }
}

/// Mask of valid 15-bit EE `INTC` registers (`bits 0..=14`).
const INTC_VALID_MASK: u32 = 0x7FFF;

/// Mask of valid 26-bit IOP `I_STAT` / `I_MASK` lines (`bits 0..=25`, `ps2tek` `#iopint` line 3948).
const IOP_INTC_VALID_MASK: u32 = 0x03FF_FFFF;

/// Emotion Engine Interrupt Controller state (`0x1000_F000` / `0x1000_F010`).
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

    /// Write `INTC_STAT` (`0x1000_F000`): write-1-to-clear (`ps2tek` `#eeintc`, `FETCHED`).
    #[inline(always)]
    pub fn write_stat(&mut self, val: u32) {
        self.stat &= !(val & INTC_VALID_MASK);
    }

    /// Read `INTC_MASK` (`0x1000_F010`).
    #[inline(always)]
    pub const fn read_mask(&self) -> u32 {
        self.mask & INTC_VALID_MASK
    }

    /// Write `INTC_MASK` (`0x1000_F010`): write-1-to-toggle (`ps2tek` `#eeintc`, `FETCHED`).
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

/// R3000A IOP Interrupt Controller (`I_STAT` `0x1F80_1070`, `I_MASK` `0x1F80_1074`, `I_CTRL` `0x1F80_1078`).
///
/// Primary source references (`FETCHED`):
/// - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iopint`, `local/ps2tek/index.html` lines 3916-3961).
/// - `ps2sdk` (`iop/system/intrman/src/intrman.c` lines 201-258, 816-875).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IopIntc {
    /// `0x1F80_1070` (`I_STAT`): Interrupt status register (`1` = raised; write `0` to clear).
    pub stat: u32,
    /// `0x1F80_1074` (`I_MASK`): Interrupt mask register (`0` = disabled, `1` = enabled).
    pub mask: u32,
    /// `0x1F80_1078` (`I_CTRL`): Global interrupt enable (bit `0`; reading returns bit `0` and clears it).
    pub ctrl: u32,
}

impl IopIntc {
    /// Create a reset-initialized [`IopIntc`].
    #[inline]
    pub const fn new() -> Self {
        Self {
            stat: 0,
            mask: 0,
            ctrl: 0,
        }
    }

    /// Read `I_STAT` (`0x1F80_1070`, `ps2tek` `#iopint` line 3918, `FETCHED`).
    #[inline(always)]
    pub const fn read_stat(&self) -> u32 {
        self.stat & IOP_INTC_VALID_MASK
    }

    /// Write `I_STAT` (`0x1F80_1070`): write-0-to-clear (`0` = clear bit, `1` = no effect,
    /// `ps2tek` `#iopint` line 3919, `FETCHED`).
    #[inline(always)]
    pub fn write_stat(&mut self, val: u32) {
        self.stat &= val & IOP_INTC_VALID_MASK;
    }

    /// Read `I_MASK` (`0x1F80_1074`, `ps2tek` `#iopint` line 3920, `FETCHED`).
    #[inline(always)]
    pub const fn read_mask(&self) -> u32 {
        self.mask & IOP_INTC_VALID_MASK
    }

    /// Write `I_MASK` (`0x1F80_1074`, `ps2tek` `#iopint` line 3920, `FETCHED`).
    #[inline(always)]
    pub fn write_mask(&mut self, val: u32) {
        self.mask = val & IOP_INTC_VALID_MASK;
    }

    /// Read `I_CTRL` (`0x1F80_1078`): returns bit 0 **and clears it**, disabling IOP interrupts
    /// (`ps2tek` `#iopint` line 3956; `ps2sdk` `intrman.c` lines 201, 249, `FETCHED`).
    #[inline(always)]
    pub fn read_ctrl(&mut self) -> u32 {
        let old = self.ctrl & 1;
        self.ctrl = 0;
        old
    }

    /// Peek `I_CTRL` (`0x1F80_1078`) without clearing bit 0 (used for non-destructive inspection
    /// and sub-word write merges).
    #[inline(always)]
    pub const fn peek_ctrl(&self) -> u32 {
        self.ctrl & 1
    }

    /// Write `I_CTRL` (`0x1F80_1078`): sets or resets bit 0 (`ps2tek` `#iopint` line 3957, `FETCHED`).
    #[inline(always)]
    pub fn write_ctrl(&mut self, val: u32) {
        self.ctrl = val & 1;
    }

    /// Raise one or more IOP hardware interrupt lines in `I_STAT`.
    #[inline(always)]
    pub fn raise_irq(&mut self, flags: IopIrqFlags) {
        self.stat |= flags.bits() & IOP_INTC_VALID_MASK;
    }

    /// Returns `true` if the IOP hardware interrupt line (`COP0.Cause.IP2`, bit 10) is asserted
    /// (`I_CTRL.0 != 0 && (I_STAT & I_MASK) != 0`, `ps2tek` `#iopint` line 3960, `FETCHED`).
    #[inline(always)]
    pub const fn irq_pending(&self) -> bool {
        (self.ctrl & 1) != 0 && (self.stat & self.mask & IOP_INTC_VALID_MASK) != 0
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

    #[test]
    fn iop_intc_stat_write_zero_to_clear_and_ctrl_read_disables_interrupts() {
        let mut iop_intc = IopIntc::new();
        assert_eq!(iop_intc.read_stat(), 0);
        assert_eq!(iop_intc.read_mask(), 0);
        assert_eq!(iop_intc.peek_ctrl(), 0);
        assert!(!iop_intc.irq_pending());

        // Raise VBLANK_START (bit 0) and DMA (bit 3).
        iop_intc.raise_irq(IopIrqFlags::VBLANK_START | IopIrqFlags::DMA);
        assert_eq!(iop_intc.read_stat(), (1 << 0) | (1 << 3));

        // Enable both in I_MASK, but I_CTRL is still 0 -> irq_pending() is false.
        iop_intc.write_mask((1 << 0) | (1 << 3));
        assert!(!iop_intc.irq_pending());

        // Enable global interrupts via I_CTRL bit 0 -> irq_pending() becomes true.
        iop_intc.write_ctrl(0xFFFF_FFFF);
        assert_eq!(iop_intc.peek_ctrl(), 1);
        assert!(iop_intc.irq_pending());

        // Reading I_CTRL returns old bit 0 (1) AND clears bit 0 -> irq_pending() becomes false.
        assert_eq!(iop_intc.read_ctrl(), 1);
        assert_eq!(iop_intc.peek_ctrl(), 0);
        assert!(!iop_intc.irq_pending());
        assert_eq!(iop_intc.read_ctrl(), 0);

        // Re-enable I_CTRL and acknowledge VBLANK_START by writing 0 to bit 0 and 1 to bit 3.
        iop_intc.write_ctrl(1);
        assert!(iop_intc.irq_pending());
        iop_intc.write_stat(!IopIrqFlags::VBLANK_START.bits());
        assert_eq!(iop_intc.read_stat(), IopIrqFlags::DMA.bits());
        assert!(iop_intc.irq_pending());

        // Acknowledge DMA (bit 3) by writing 0 to bit 3 -> stat is 0, irq_pending() is false.
        iop_intc.write_stat(!IopIrqFlags::DMA.bits());
        assert_eq!(iop_intc.read_stat(), 0);
        assert!(!iop_intc.irq_pending());
    }
}
