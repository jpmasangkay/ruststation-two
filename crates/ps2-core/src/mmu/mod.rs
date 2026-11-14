//! Emotion Engine Memory Management Unit (`COP0` 48-entry TLB and software 4 KB `vtlb`).

pub mod tlb;
pub mod vtlb;

pub use tlb::{Tlb, TlbEntry, TLB_ENTRY_COUNT};
pub use vtlb::{TranslatedAddr, Vtlb, VtlbEntry, VtlbTarget};
