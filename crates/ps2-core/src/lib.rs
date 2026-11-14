//! Platform-independent PlayStation 2 emulation core (`ps2-core`).
//!
//! Contains the event-driven scheduler, physical/virtual memory map (`vtlb`),
//! Emotion Engine (MIPS R5900 + MMI + COP0 + COP1), IOP (MIPS R3000A),
//! 10-channel DMAC, and SIF subsystems.

pub mod bus;
pub mod ee;
pub mod mmu;
pub mod scheduler;

pub use ee::EeCpuState;
pub use ps2_common::Reg128;

/// Core clock frequencies in Hz (`ps2tek` "Overview -> Clock Speeds").
pub mod clocks {
    /// Emotion Engine (R5900 + VU0/VU1) clock rate: 294.912 MHz.
    pub const EE_CLOCK_HZ: u64 = 294_912_000;
    /// System Bus, DMAC, and GS clock rate: 147.456 MHz (`EE_CLOCK_HZ / 2`).
    pub const BUS_CLOCK_HZ: u64 = EE_CLOCK_HZ / 2;
    /// I/O Processor (R3000A IOP) clock rate in PS2 mode: 36.864 MHz (`EE_CLOCK_HZ / 8`).
    pub const IOP_CLOCK_HZ: u64 = EE_CLOCK_HZ / 8;
}

#[cfg(test)]
mod tests {
    use super::clocks::*;

    #[test]
    fn clock_domain_ratios_match_hardware() {
        assert_eq!(EE_CLOCK_HZ, BUS_CLOCK_HZ * 2);
        assert_eq!(EE_CLOCK_HZ, IOP_CLOCK_HZ * 8);
    }
}
