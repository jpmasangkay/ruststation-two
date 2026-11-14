//! Architectural 48-entry R5900 COP0 Translation Lookaside Buffer (TLB).
//!
//! Primary source reference:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eecop0memorymanagement`):
//!   * 48 dual-page entries (`0..48`), each mapping an Even and an Odd page of size
//!     `4 KB`, `16 KB`, `64 KB`, `256 KB`, `1 MB`, `4 MB`, or `16 MB` according to the
//!     12-bit `MASK` field (`bits 24..13`) of `COP0.PageMask` (`$5`).
//!   * `EntryLo0` (`$2`) bit 31 (`S` bit) enables the 16 KB Scratchpad SRAM mapping (`0x7000_0000`).
//!   * `PFN` (Physical Frame Number) occupies bits `25..6` of `EntryLo0` (`$2`) and `EntryLo1` (`$3`).

/// Number of hardware TLB entries in the R5900 COP0 MMU (`ps2tek`).
pub const TLB_ENTRY_COUNT: usize = 48;

/// Single R5900 COP0 TLB entry mapping an even/odd virtual page pair.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TlbEntry {
    /// COP0 `PageMask` register (`COP0[5]`): bits `24..13` hold the 12-bit page size mask.
    pub page_mask: u32,
    /// COP0 `EntryHi` register (`COP0[10]`): bits `31..13` = `VPN2`, bits `7..0` = `ASID`.
    pub entry_hi: u32,
    /// COP0 `EntryLo0` register (`COP0[2]`): bit 31 = `S` (Scratchpad), bits `25..6` = `PFN0`,
    /// bits `5..3` = `C`, bit 2 = `D`, bit 1 = `V`, bit 0 = `G`.
    pub entry_lo0: u32,
    /// COP0 `EntryLo1` register (`COP0[3]`): bits `25..6` = `PFN1`, bits `5..3` = `C`,
    /// bit 2 = `D`, bit 1 = `V`, bit 0 = `G`.
    pub entry_lo1: u32,
}

impl TlbEntry {
    /// Return the 12-bit `PageMask` field (`bits 24..13`).
    #[inline(always)]
    pub const fn mask12(&self) -> u32 {
        (self.page_mask >> 13) & 0xFFF
    }

    /// Return the size in bytes of a single page (even or odd half) mapped by this entry.
    ///
    /// Reference: `ps2tek` ("EE COP0 -> PageMask"):
    /// `0x000` -> 4 KB, `0x003` -> 16 KB, `0x00F` -> 64 KB, `0x03F` -> 256 KB,
    /// `0x0FF` -> 1 MB, `0x3FF` -> 4 MB, `0xFFF` -> 16 MB.
    #[inline(always)]
    pub const fn page_size_bytes(&self) -> u32 {
        (self.mask12() + 1) << 12
    }

    /// Return the base virtual address (`VPN2 << 13`) masked by the entry's dual-page size.
    #[inline(always)]
    pub const fn even_vbase(&self) -> u32 {
        let vpn2_mask = (!self.mask12()) & 0x7FFFF;
        ((self.entry_hi >> 13) & vpn2_mask) << 13
    }

    /// Return the odd-page virtual base address (`even_vbase + page_size_bytes`).
    #[inline(always)]
    pub const fn odd_vbase(&self) -> u32 {
        self.even_vbase().wrapping_add(self.page_size_bytes())
    }

    /// Return `true` if the Scratchpad `S` bit (bit 31 of `EntryLo0`) is set (`ps2tek`).
    #[inline(always)]
    pub const fn is_scratchpad(&self) -> bool {
        (self.entry_lo0 & 0x8000_0000) != 0
    }

    /// Return `true` if the even page is marked Valid (`V`, bit 1 of `EntryLo0`).
    #[inline(always)]
    pub const fn even_valid(&self) -> bool {
        (self.entry_lo0 & 0x2) != 0
    }

    /// Return `true` if the odd page is marked Valid (`V`, bit 1 of `EntryLo1`).
    #[inline(always)]
    pub const fn odd_valid(&self) -> bool {
        (self.entry_lo1 & 0x2) != 0
    }

    /// Physical base address (`PFN0 << 12`) of the even page.
    #[inline(always)]
    pub const fn even_paddr(&self) -> u32 {
        ((self.entry_lo0 >> 6) & 0xFFFFF) << 12
    }

    /// Physical base address (`PFN1 << 12`) of the odd page.
    #[inline(always)]
    pub const fn odd_paddr(&self) -> u32 {
        ((self.entry_lo1 >> 6) & 0xFFFFF) << 12
    }

    /// Check if `vaddr` and `asid` match this TLB entry (`TLBP` probe logic, `ps2tek`).
    #[inline]
    pub const fn matches(&self, vaddr: u32, asid: u8) -> bool {
        let vpn2_mask = (!self.mask12()) & 0x7FFFF;
        let entry_vpn2 = (self.entry_hi >> 13) & vpn2_mask;
        let addr_vpn2 = (vaddr >> 13) & vpn2_mask;
        let global = ((self.entry_lo0 & 1) != 0) && ((self.entry_lo1 & 1) != 0);
        let entry_asid = (self.entry_hi & 0xFF) as u8;
        (entry_vpn2 == addr_vpn2) && (global || entry_asid == asid)
    }
}

/// 48-entry architectural COP0 TLB register file.
pub struct Tlb {
    pub entries: [TlbEntry; TLB_ENTRY_COUNT],
}

impl Default for Tlb {
    fn default() -> Self {
        Self::new()
    }
}

impl Tlb {
    pub const fn new() -> Self {
        Self {
            entries: [TlbEntry {
                page_mask: 0,
                entry_hi: 0,
                entry_lo0: 0,
                entry_lo1: 0,
            }; TLB_ENTRY_COUNT],
        }
    }

    /// Probe the 48 entries for a match against `(vaddr, asid)`, returning the matching index (`0..48`).
    pub fn probe(&self, vaddr: u32, asid: u8) -> Option<usize> {
        self.entries.iter().position(|e| e.matches(vaddr, asid))
    }
}
