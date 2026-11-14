//! Software 4 KB Virtual-to-Physical Page Table (`vtlb`).
//!
//! Primary source references:
//! - `ps2tek` ("EE Memory Map" & "EE COP0 -> TLB"):
//!   * `KSEG0` (`0x8000_0000..=0x9FFF_FFFF`) and `KSEG1` (`0xA000_0000..=0xBFFF_FFFF`) are
//!     architecturally unmapped segments where `paddr = vaddr & 0x1FFF_FFFF`.
//!   * `KUSEG` (`0x0000_0000..=0x7FFF_FFFF`) is TLB-mapped. The PS2 kernel default TLB layout
//!     maps Main RDRAM at `0x0000_0000..=0x01FF_FFFF` (Cached), `0x2000_0000..=0x21FF_FFFF`
//!     (Uncached), and `0x3010_0000..=0x31FF_FFFF` (Uncached Accelerated / UCAB), identity-maps
//!     I/O, VU memory, GS Privileged registers, IOP RAM, and BIOS ROM in `0x1000_0000..=0x1FFF_FFFF`,
//!     and maps the 16 KB Scratchpad SRAM at `0x7000_0000..=0x7000_3FFF` via `EntryLo0.S = 1`.

use ps2_common::vmem::{GUEST_PAGE_MASK, GUEST_PAGE_SHIFT, GUEST_PAGE_SIZE, VTLB_PAGE_COUNT};

use super::tlb::TlbEntry;

/// Backing target region for a 4 KB virtual page in [`Vtlb`].
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VtlbTarget {
    /// Unmapped virtual page (access triggers TLB Invalid / Refill or returns 0 on open bus).
    Unmapped = 0,
    /// 32 MB Main RDRAM (`0x0000_0000..=0x01FF_FFFF`).
    Ram = 1,
    /// 16 KB Scratchpad SRAM (`0x7000_0000..=0x7000_3FFF`).
    Scratchpad = 2,
    /// 4 MB System BIOS ROM (`0x1FC0_0000..=0x1FFF_FFFF`).
    Bios = 3,
    /// 2 MB IOP Main RAM mirror (`0x1C00_0000..=0x1C1F_FFFF`).
    IopRam = 4,
    /// 64 KB VU0/VU1 Micro and Data memory window (`0x1100_0000..=0x1100_FFFF`).
    VuMem = 5,
    /// Memory-Mapped I/O registers (`0x1000_0000..=0x1000_FFFF`, `0x1200_0000..=0x1200_1FFF`, etc.).
    Mmio = 6,
}

/// Compact 32-bit `vtlb` page descriptor.
///
/// Layout:
/// - Bits `31..28`: [`VtlbTarget`] discriminant (`0..=6`).
/// - Bits `27..0`: Page-aligned physical or backing buffer offset (`offset & 0x0FFF_F000`, or full
///   29-bit physical page base for `Mmio` since EE physical addresses fit in 29 bits: `0x0000_0000..=0x1FFF_FFFF`
///   where `bits 11..0` are zero, so we store `paddr >> 4` in `bits 27..0`!).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct VtlbEntry(u32);

impl VtlbEntry {
    pub const UNMAPPED: Self = Self(0);

    /// Construct a `VtlbEntry` from a [`VtlbTarget`] and a 4 KB-aligned offset (`bits 11..0` must be 0).
    #[inline(always)]
    pub const fn new(target: VtlbTarget, page_aligned_offset: u32) -> Self {
        let tag = (target as u32) << 28;
        let encoded_page = (page_aligned_offset >> 4) & 0x0FFF_FFFF;
        Self(tag | encoded_page)
    }

    /// Classify a 29-bit physical address (`0x0000_0000..=0x1FFF_FFFF`) into its [`VtlbEntry`].
    ///
    /// Reference: `ps2tek` ("EE Memory Map").
    #[inline]
    pub const fn from_physical_page(paddr: u32) -> Self {
        let ppage = paddr & 0x1FFF_F000;
        match ppage {
            0x0000_0000..=0x01FF_F000 => Self::new(VtlbTarget::Ram, ppage & 0x01FF_F000),
            0x1000_0000..=0x1000_F000 => Self::new(VtlbTarget::Mmio, ppage),
            0x1100_0000..=0x1100_F000 => Self::new(VtlbTarget::VuMem, ppage & 0x0000_F000),
            0x1200_0000..=0x1200_1000 => Self::new(VtlbTarget::Mmio, ppage),
            0x1C00_0000..=0x1C1F_F000 => Self::new(VtlbTarget::IopRam, ppage & 0x001F_F000),
            0x1FC0_0000..=0x1FFF_F000 => Self::new(VtlbTarget::Bios, ppage & 0x003F_F000),
            // Other physical I/O pages (e.g. CDVD/DEV9/BIOS ROM1/ROM2 probes at 0x1E00_0000+)
            // route to the MMIO dispatcher so open-bus / stub reads are logged cleanly.
            _ => Self::new(VtlbTarget::Mmio, ppage),
        }
    }

    /// Extract the [`VtlbTarget`] of this page.
    #[inline(always)]
    pub const fn target(self) -> VtlbTarget {
        match self.0 >> 28 {
            1 => VtlbTarget::Ram,
            2 => VtlbTarget::Scratchpad,
            3 => VtlbTarget::Bios,
            4 => VtlbTarget::IopRam,
            5 => VtlbTarget::VuMem,
            6 => VtlbTarget::Mmio,
            _ => VtlbTarget::Unmapped,
        }
    }

    /// Extract the 4 KB-aligned base offset (`0x0000_0000..=0xFFFF_F000`).
    #[inline(always)]
    pub const fn page_base_offset(self) -> u32 {
        (self.0 & 0x0FFF_FFFF) << 4
    }
}

/// Result of translating a 32-bit guest virtual address through [`Vtlb`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TranslatedAddr {
    pub target: VtlbTarget,
    pub offset: u32,
}

/// 1,048,576-entry software page table covering the 4 GB EE virtual address space in 4 KB pages.
pub struct Vtlb {
    entries: Box<[VtlbEntry]>,
}

impl Default for Vtlb {
    fn default() -> Self {
        Self::new()
    }
}

impl Vtlb {
    /// Allocate and initialize the 4 GB software `vtlb` with the default PS2 segment mappings.
    pub fn new() -> Self {
        let mut vtlb = Self {
            entries: vec![VtlbEntry::UNMAPPED; VTLB_PAGE_COUNT].into_boxed_slice(),
        };
        vtlb.reset_default_mappings();
        vtlb
    }

    /// Populate hardware `KSEG0`/`KSEG1` mappings and default `KUSEG` mirrors (`ps2tek`).
    pub fn reset_default_mappings(&mut self) {
        self.entries.fill(VtlbEntry::UNMAPPED);

        // 1. Architecturally unmapped KSEG0 (0x8000_0000..=0x9FFF_FFFF) and KSEG1 (0xA000_0000..=0xBFFF_FFFF).
        let kseg_pages = 0x2000_0000usize >> GUEST_PAGE_SHIFT; // 131,072 pages per 512 MB segment
        for page_idx in 0..kseg_pages {
            let paddr = (page_idx << GUEST_PAGE_SHIFT) as u32;
            let entry = VtlbEntry::from_physical_page(paddr);
            self.entries[(0x8000_0000usize >> GUEST_PAGE_SHIFT) + page_idx] = entry;
            self.entries[(0xA000_0000usize >> GUEST_PAGE_SHIFT) + page_idx] = entry;
        }

        // 2. Default KUSEG mappings (ps2tek "EE Memory Map"):
        // - 0x0000_0000..=0x01FF_FFFF: 32 MB Main RDRAM (Cached)
        // - 0x2000_0000..=0x21FF_FFFF: 32 MB Main RDRAM (Uncached)
        // - 0x3000_0000..=0x31FF_FFFF: 32 MB Main RDRAM (Uncached Accelerated / UCAB, e.g. 0x3010_0000+)
        let ram_pages = (32 * 1024 * 1024) >> GUEST_PAGE_SHIFT; // 8,192 pages
        for page_idx in 0..ram_pages {
            let offset = (page_idx << GUEST_PAGE_SHIFT) as u32;
            let entry = VtlbEntry::new(VtlbTarget::Ram, offset);
            self.entries[page_idx] = entry;
            self.entries[(0x2000_0000usize >> GUEST_PAGE_SHIFT) + page_idx] = entry;
            self.entries[(0x3000_0000usize >> GUEST_PAGE_SHIFT) + page_idx] = entry;
        }

        // - 0x1000_0000..=0x1FFF_FFFF: Default kernel identity mapping of physical I/O, VU Mem, GS, IOP RAM, BIOS.
        let io_pages = 0x1000_0000usize >> GUEST_PAGE_SHIFT;
        for page_idx in 0..io_pages {
            let paddr = 0x1000_0000u32 + ((page_idx << GUEST_PAGE_SHIFT) as u32);
            self.entries[(0x1000_0000usize >> GUEST_PAGE_SHIFT) + page_idx] =
                VtlbEntry::from_physical_page(paddr);
        }

        // - 0x7000_0000..=0x7000_3FFF: 16 KB Scratchpad SRAM (4 x 4 KB pages).
        for page_idx in 0..4usize {
            let offset = (page_idx << GUEST_PAGE_SHIFT) as u32;
            self.entries[(0x7000_0000usize >> GUEST_PAGE_SHIFT) + page_idx] =
                VtlbEntry::new(VtlbTarget::Scratchpad, offset);
        }
    }

    /// Translate a 32-bit guest virtual address in $O(1)$ via a single table lookup.
    #[inline(always)]
    pub fn translate(&self, vaddr: u32) -> TranslatedAddr {
        let entry = self.entries[(vaddr >> GUEST_PAGE_SHIFT) as usize];
        TranslatedAddr {
            target: entry.target(),
            offset: entry.page_base_offset() | (vaddr & GUEST_PAGE_MASK),
        }
    }

    /// Apply a COP0 [`TlbEntry`] (on `TLBWI` or `TLBWR`) to the software `vtlb` page table.
    ///
    /// Reference: `ps2tek` ("EE COP0 -> TLB"):
    /// - If `EntryLo0` bit 31 (`S` Scratchpad bit) is set, maps 16 KB (4 pages) at `even_vbase`
    ///   to [`VtlbTarget::Scratchpad`].
    /// - Otherwise maps `page_size_bytes()` for the Even page (`EntryLo0`) and Odd page (`EntryLo1`).
    pub fn map_tlb_entry(&mut self, entry: &TlbEntry) {
        let even_vbase = entry.even_vbase();
        if entry.is_scratchpad() {
            let start_page = (even_vbase >> GUEST_PAGE_SHIFT) as usize;
            for i in 0..4usize {
                if let Some(slot) = self.entries.get_mut(start_page.wrapping_add(i)) {
                    *slot = VtlbEntry::new(VtlbTarget::Scratchpad, (i << GUEST_PAGE_SHIFT) as u32);
                }
            }
            return;
        }

        let page_size = entry.page_size_bytes();
        let subpages = (page_size as usize) / GUEST_PAGE_SIZE;

        // Even page range
        let even_start_page = (even_vbase >> GUEST_PAGE_SHIFT) as usize;
        let even_pbase = entry.even_paddr();
        for i in 0..subpages {
            let page_idx = even_start_page.wrapping_add(i);
            // Do not overwrite hardware-unmapped KSEG0/KSEG1 (0x8000_0000..=0xBFFF_FFFF).
            if (0x80000..0xC0000).contains(&page_idx) {
                continue;
            }
            if let Some(slot) = self.entries.get_mut(page_idx) {
                *slot = if entry.even_valid() {
                    let paddr = even_pbase.wrapping_add((i << GUEST_PAGE_SHIFT) as u32);
                    VtlbEntry::from_physical_page(paddr)
                } else {
                    VtlbEntry::UNMAPPED
                };
            }
        }

        // Odd page range
        let odd_vbase = entry.odd_vbase();
        let odd_start_page = (odd_vbase >> GUEST_PAGE_SHIFT) as usize;
        let odd_pbase = entry.odd_paddr();
        for i in 0..subpages {
            let page_idx = odd_start_page.wrapping_add(i);
            if (0x80000..0xC0000).contains(&page_idx) {
                continue;
            }
            if let Some(slot) = self.entries.get_mut(page_idx) {
                *slot = if entry.odd_valid() {
                    let paddr = odd_pbase.wrapping_add((i << GUEST_PAGE_SHIFT) as u32);
                    VtlbEntry::from_physical_page(paddr)
                } else {
                    VtlbEntry::UNMAPPED
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vtlb_default_mappings_cover_kuseg_ucab_kseg_and_scratchpad() {
        let vtlb = Vtlb::new();

        // KUSEG cached, KUSEG uncached (0x2000_0000), KUSEG UCAB (0x3010_0000), KSEG0, KSEG1 -> RAM
        assert_eq!(
            vtlb.translate(0x0010_1234),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0010_1234
            }
        );
        assert_eq!(
            vtlb.translate(0x2010_1234),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0010_1234
            }
        );
        assert_eq!(
            vtlb.translate(0x3010_1234),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0010_1234
            }
        );
        assert_eq!(
            vtlb.translate(0x8010_1234),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0010_1234
            }
        );
        assert_eq!(
            vtlb.translate(0xA010_1234),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0010_1234
            }
        );

        // Scratchpad (0x7000_0000..=0x7000_3FFF)
        assert_eq!(
            vtlb.translate(0x7000_2ABC),
            TranslatedAddr {
                target: VtlbTarget::Scratchpad,
                offset: 0x2ABC
            }
        );

        // Reset Vector in KSEG1 (0xBFC0_0000 -> BIOS offset 0)
        assert_eq!(
            vtlb.translate(0xBFC0_0000),
            TranslatedAddr {
                target: VtlbTarget::Bios,
                offset: 0
            }
        );

        // GS Privileged CSR register (0x1200_1000)
        assert_eq!(
            vtlb.translate(0x1200_1000),
            TranslatedAddr {
                target: VtlbTarget::Mmio,
                offset: 0x1200_1000
            }
        );
    }

    #[test]
    fn vtlb_dynamic_tlb_entry_remapping() {
        let mut vtlb = Vtlb::new();

        // Map virtual 0x4000_0000 (even 4 KB page) to physical RAM 0x0050_0000,
        // and virtual 0x4000_1000 (odd 4 KB page) to physical RAM 0x0060_0000.
        let entry = TlbEntry {
            page_mask: 0x0000_0000,             // 4 KB pages
            entry_hi: 0x4000_0000,              // VPN2 = 0x4000_0000 >> 13
            entry_lo0: (0x00500 << 6) | 0b0111, // PFN0 = 0x500, D=1, V=1, G=1
            entry_lo1: (0x00600 << 6) | 0b0111, // PFN1 = 0x600, D=1, V=1, G=1
        };
        vtlb.map_tlb_entry(&entry);

        assert_eq!(
            vtlb.translate(0x4000_0120),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0050_0120
            }
        );
        assert_eq!(
            vtlb.translate(0x4000_1340),
            TranslatedAddr {
                target: VtlbTarget::Ram,
                offset: 0x0060_0340
            }
        );

        // Test Scratchpad remapping via EntryLo0 bit 31 (`S` bit) to 0x7800_0000.
        let spr_entry = TlbEntry {
            page_mask: 0x0000_0000,
            entry_hi: 0x7800_0000,
            entry_lo0: 0x8000_0000 | 0b0111,
            entry_lo1: 0,
        };
        vtlb.map_tlb_entry(&spr_entry);
        assert_eq!(
            vtlb.translate(0x7800_3456),
            TranslatedAddr {
                target: VtlbTarget::Scratchpad,
                offset: 0x3456
            }
        );
    }
}
