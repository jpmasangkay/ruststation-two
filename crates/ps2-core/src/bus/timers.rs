//! Emotion Engine 16-bit Hardware Timers (`TIM0..TIM3`, `0x1000_0000..=0x1000_1FFF`).
//!
//! Primary source reference:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eetimers` & `https://psi-rockin.github.io/ps2tek/#iomaps`):
//!   * Four 16-bit timers clocked from `BUSCLK` (`~147.456 MHz`, i.e., `1` BUSCLK = `2` EE cycles):
//!     - Timer 0: `0x1000_0000..=0x1000_0030` (`INTC` bit 9 = `TIM0`)
//!     - Timer 1: `0x1000_0800..=0x1000_0830` (`INTC` bit 10 = `TIM1`)
//!     - Timer 2: `0x1000_1000..=0x1000_1020` (`INTC` bit 11 = `TIM2`, no `HOLD` register)
//!     - Timer 3: `0x1000_1800..=0x1000_1820` (`INTC` bit 12 = `TIM3`, no `HOLD` register)
//!   * Register offsets within each `0x800` timer block:
//!     - `+0x00`: `TN_COUNT` (16-bit counter)
//!     - `+0x10`: `TN_MODE` (bits `0..=9` control, bits `10..=11` `EQUF`/`OVFF` write-1-to-clear)
//!     - `+0x20`: `TN_COMP` (16-bit compare target)
//!     - `+0x30`: `TN_HOLD` (16-bit hold register, `TIM0` and `TIM1` only)
//!   * Edge-triggered interrupt behavior (`ps2tek` `#eetimers`):
//!     - When `COUNT` reaches `COMP`, if `CMPE` (bit 8) is set and `EQUF` (bit 10) transitions
//!       from `0 -> 1`, sets `EQUF = 1` and raises `INTC` `TIMn`. If `ZRET` (bit 6) is set,
//!       resets `COUNT` to `0`.
//!     - When `COUNT` wraps `0xFFFF -> 0x0000`, if `OVFE` (bit 9) is set and `OVFF` (bit 11)
//!       transitions from `0 -> 1`, sets `OVFF = 1` and raises `INTC` `TIMn`.

use super::intc::{EeIntc, IntcFlags};

/// Number of hardware timers on the Emotion Engine (`ps2tek` `#eetimers`).
pub const EE_TIMER_COUNT: usize = 4;

/// `TN_MODE` bit masks (`ps2tek` `#eetimers`).
pub mod mode_bits {
    /// Bits `0..=1`: Clock selection (`0` = BUSCLK, `1` = BUSCLK/16, `2` = BUSCLK/256, `3` = HBLANK).
    pub const CLKS_MASK: u32 = 0b11;
    /// Bit `2`: Gate function enable.
    pub const GATE: u32 = 1 << 2;
    /// Bit `3`: Gate input (`0` = HBLANK, `1` = VBLANK).
    pub const GATS: u32 = 1 << 3;
    /// Bits `4..=5`: Gate mode (`0..=3`).
    pub const GATM_MASK: u32 = 0b11 << 4;
    /// Bit `6`: Zero-return when `COUNT == COMP`.
    pub const ZRET: u32 = 1 << 6;
    /// Bit `7`: Count enable (`1` = running).
    pub const CUE: u32 = 1 << 7;
    /// Bit `8`: Compare interrupt enable.
    pub const CMPE: u32 = 1 << 8;
    /// Bit `9`: Overflow interrupt enable.
    pub const OVFE: u32 = 1 << 9;
    /// Bit `10`: Equal flag (`COUNT == COMP` reached; write `1` to clear).
    pub const EQUF: u32 = 1 << 10;
    /// Bit `11`: Overflow flag (`0xFFFF -> 0x0000` wrapped; write `1` to clear).
    pub const OVFF: u32 = 1 << 11;
}

/// State of a single 16-bit EE hardware timer (`ps2tek` `#eetimers`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EeTimerChannel {
    /// `TN_COUNT` (`+0x00`): 16-bit counter (`0x0000..=0xFFFF`).
    pub count: u16,
    /// `TN_MODE` (`+0x10`): 12-bit mode and status register.
    pub mode: u32,
    /// `TN_COMP` (`+0x20`): 16-bit compare value (`0x0000..=0xFFFF`).
    pub comp: u16,
    /// `TN_HOLD` (`+0x30`): 16-bit hold value (`TIM0` and `TIM1` only).
    pub hold: u16,
    /// Sub-tick EE cycle remainder for prescaled BUSCLK modes (`2`, `32`, or `512` EE cycles per tick).
    pub ee_cycle_remainder: u32,
}

impl EeTimerChannel {
    pub const fn new() -> Self {
        Self {
            count: 0,
            mode: 0,
            comp: 0,
            hold: 0,
            ee_cycle_remainder: 0,
        }
    }

    /// Returns `true` if `CUE` (bit 7 of `TN_MODE`) is set (`ps2tek` `#eetimers`).
    #[inline(always)]
    pub const fn is_enabled(&self) -> bool {
        (self.mode & mode_bits::CUE) != 0
    }

    /// Return the clock divider in EE cycles per timer increment for `CLKS` modes `0..=2`,
    /// or `None` when `CLKS == 3` (`HBLANK` clock source).
    ///
    /// Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eetimers`):
    /// - `CLKS == 0`: `BUSCLK` = `2` EE cycles
    /// - `CLKS == 1`: `BUSCLK / 16` = `32` EE cycles
    /// - `CLKS == 2`: `BUSCLK / 256` = `512` EE cycles
    /// - `CLKS == 3`: `HBLANK`
    #[inline(always)]
    pub const fn ee_cycles_per_tick(&self) -> Option<u32> {
        match self.mode & mode_bits::CLKS_MASK {
            0 => Some(2),
            1 => Some(32),
            2 => Some(512),
            _ => None,
        }
    }

    /// Write `TN_MODE` (`+0x10`): bits `0..=9` replace the mode configuration; bits `10..=11`
    /// (`EQUF` and `OVFF`) are write-1-to-clear (`ps2tek` `#eetimers`).
    #[inline]
    pub fn write_mode(&mut self, val: u32) {
        let preserved_flags = self.mode & (mode_bits::EQUF | mode_bits::OVFF);
        let cleared_flags = preserved_flags & !(val & (mode_bits::EQUF | mode_bits::OVFF));
        self.mode = (val & 0x03FF) | cleared_flags;
    }

    /// Advance this timer by `ticks` increments, raising `irq_flag` on `intc` on `0 -> 1`
    /// transitions of `EQUF` or `OVFF` (`ps2tek` `#eetimers`).
    pub fn advance_ticks(&mut self, mut ticks: u32, irq_flag: IntcFlags, intc: &mut EeIntc) {
        while ticks > 0 {
            let current = u32::from(self.count);
            let comp = u32::from(self.comp);

            // Distance in ticks to the next compare match (`1..=0x10000`) and overflow (`1..=0x10000`).
            let ticks_to_comp = if current < comp {
                comp - current
            } else {
                0x1_0000 - current + comp
            };
            let ticks_to_ovf = 0x1_0000 - current;
            let next_event_ticks = ticks_to_comp.min(ticks_to_ovf);

            if ticks < next_event_ticks {
                self.count = self.count.wrapping_add(ticks as u16);
                break;
            }

            ticks -= next_event_ticks;
            self.count = self.count.wrapping_add(next_event_ticks as u16);

            // Check overflow first if we stepped `ticks_to_ovf` (wrapping 0xFFFF -> 0x0000).
            if next_event_ticks == ticks_to_ovf
                && (self.mode & mode_bits::OVFE) != 0
                && (self.mode & mode_bits::OVFF) == 0
            {
                self.mode |= mode_bits::OVFF;
                intc.raise_irq(irq_flag);
            }

            // Check compare match (`COUNT == COMP`, which can also coincide with `COMP == 0` on overflow).
            if self.count == self.comp {
                if (self.mode & mode_bits::CMPE) != 0 && (self.mode & mode_bits::EQUF) == 0 {
                    self.mode |= mode_bits::EQUF;
                    intc.raise_irq(irq_flag);
                }
                if (self.mode & mode_bits::ZRET) != 0 {
                    self.count = 0;
                }
            }
        }
    }
}

/// Bank of 4 Emotion Engine 16-bit hardware timers (`TIM0..TIM3`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EeTimers {
    pub channels: [EeTimerChannel; EE_TIMER_COUNT],
}

impl EeTimers {
    pub const fn new() -> Self {
        Self {
            channels: [EeTimerChannel::new(); EE_TIMER_COUNT],
        }
    }

    /// Map timer index `0..=3` to its `INTC` interrupt bit (`TIM0..TIM3`, bits `9..=12`, `ps2tek` `#eeintc`).
    #[inline(always)]
    pub const fn timer_irq_flag(index: usize) -> IntcFlags {
        match index {
            0 => IntcFlags::TIM0,
            1 => IntcFlags::TIM1,
            2 => IntcFlags::TIM2,
            _ => IntcFlags::TIM3,
        }
    }

    /// Advance all BUSCLK-driven timers (`CLKS` `0..=2`) by `ee_cycles` (`ps2tek` `#eetimers`).
    pub fn step_ee_cycles(&mut self, ee_cycles: u32, intc: &mut EeIntc) {
        for (idx, ch) in self.channels.iter_mut().enumerate() {
            if !ch.is_enabled() {
                continue;
            }
            if let Some(divider) = ch.ee_cycles_per_tick() {
                let total_cycles = ch.ee_cycle_remainder.wrapping_add(ee_cycles);
                let ticks = total_cycles / divider;
                ch.ee_cycle_remainder = total_cycles % divider;
                if ticks > 0 {
                    ch.advance_ticks(ticks, Self::timer_irq_flag(idx), intc);
                }
            }
        }
    }

    /// Advance any HBLANK-clocked timers (`CLKS == 3`) by 1 tick on HBlank (`ps2tek` `#eetimers`).
    pub fn on_hblank(&mut self, intc: &mut EeIntc) {
        for (idx, ch) in self.channels.iter_mut().enumerate() {
            if ch.is_enabled() && ch.ee_cycles_per_tick().is_none() {
                ch.advance_ticks(1, Self::timer_irq_flag(idx), intc);
            }
        }
    }

    /// Read a 32-bit timer register at `paddr` (`0x1000_0000..=0x1000_1FFF`, `ps2tek` `#eetimers`).
    pub const fn read32(&self, paddr: u32) -> u32 {
        let timer_idx = ((paddr >> 11) & 0x3) as usize;
        let reg_offset = paddr & 0x7FF;
        let ch = &self.channels[timer_idx];
        match reg_offset {
            0x000 => ch.count as u32,
            0x010 => ch.mode & 0x0FFF,
            0x020 => ch.comp as u32,
            0x030 if timer_idx < 2 => ch.hold as u32,
            _ => 0,
        }
    }

    /// Write a 32-bit timer register at `paddr` (`0x1000_0000..=0x1000_1FFF`, `ps2tek` `#eetimers`).
    pub fn write32(&mut self, paddr: u32, val: u32) {
        let timer_idx = ((paddr >> 11) & 0x3) as usize;
        let reg_offset = paddr & 0x7FF;
        let ch = &mut self.channels[timer_idx];
        match reg_offset {
            0x000 => ch.count = (val & 0xFFFF) as u16,
            0x010 => ch.write_mode(val),
            0x020 => ch.comp = (val & 0xFFFF) as u16,
            0x030 if timer_idx < 2 => ch.hold = (val & 0xFFFF) as u16,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ee_timer_compare_zret_and_edge_triggered_interrupt() {
        let mut timers = EeTimers::new();
        let mut intc = EeIntc::new();

        // Configure Timer 0: COMP = 10, CLKS = 0 (BUSCLK = 2 EE cycles/tick), ZRET = 1, CUE = 1, CMPE = 1.
        timers.write32(0x1000_0020, 10);
        timers.write32(
            0x1000_0010,
            mode_bits::ZRET | mode_bits::CUE | mode_bits::CMPE,
        );

        // Advance 18 EE cycles (9 BUSCLK ticks) -> COUNT = 9, no interrupt yet.
        timers.step_ee_cycles(18, &mut intc);
        assert_eq!(timers.read32(0x1000_0000), 9);
        assert_eq!(intc.read_stat() & IntcFlags::TIM0.bits(), 0);

        // Advance 2 more EE cycles (1 BUSCLK tick) -> COUNT reaches 10 == COMP, ZRET resets COUNT to 0, EQUF=1, TIM0 IRQ raised.
        timers.step_ee_cycles(2, &mut intc);
        assert_eq!(timers.read32(0x1000_0000), 0);
        assert_ne!(timers.read32(0x1000_0010) & mode_bits::EQUF, 0);
        assert_ne!(intc.read_stat() & IntcFlags::TIM0.bits(), 0);

        // Acknowledge INTC TIM0 bit, but leave EQUF=1 in TN_MODE: next compare match must NOT re-trigger TIM0 IRQ (`ps2tek`).
        intc.write_stat(IntcFlags::TIM0.bits());
        timers.step_ee_cycles(20, &mut intc);
        assert_eq!(timers.read32(0x1000_0000), 0);
        assert_eq!(intc.read_stat() & IntcFlags::TIM0.bits(), 0);

        // Clear EQUF by writing 1 to bit 10 of TN_MODE; next compare match triggers TIM0 IRQ again.
        timers.write32(
            0x1000_0010,
            mode_bits::ZRET | mode_bits::CUE | mode_bits::CMPE | mode_bits::EQUF,
        );
        assert_eq!(timers.read32(0x1000_0010) & mode_bits::EQUF, 0);
        timers.step_ee_cycles(20, &mut intc);
        assert_ne!(intc.read_stat() & IntcFlags::TIM0.bits(), 0);
    }

    #[test]
    fn ee_timer_overflow_interrupt_and_prescaler_256() {
        let mut timers = EeTimers::new();
        let mut intc = EeIntc::new();

        // Configure Timer 2 (0x1000_1000): COUNT = 0xFFFE, CLKS = 2 (BUSCLK/256 = 512 EE cycles/tick), CUE = 1, OVFE = 1.
        timers.write32(0x1000_1000, 0xFFFE);
        timers.write32(0x1000_1010, 2 | mode_bits::CUE | mode_bits::OVFE);

        // 512 EE cycles -> COUNT = 0xFFFF.
        timers.step_ee_cycles(512, &mut intc);
        assert_eq!(timers.read32(0x1000_1000), 0xFFFF);
        assert_eq!(intc.read_stat() & IntcFlags::TIM2.bits(), 0);

        // Another 512 EE cycles -> COUNT wraps 0xFFFF -> 0x0000, OVFF=1, TIM2 IRQ raised.
        timers.step_ee_cycles(512, &mut intc);
        assert_eq!(timers.read32(0x1000_1000), 0x0000);
        assert_ne!(timers.read32(0x1000_1010) & mode_bits::OVFF, 0);
        assert_ne!(intc.read_stat() & IntcFlags::TIM2.bits(), 0);
    }
}
