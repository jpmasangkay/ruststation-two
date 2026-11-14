//! Virtual memory constants and configuration stub for Phase 1.
//!
//! In Phase 1, `ps2-core` uses a portable software-backed `vtlb` table (4 KB page granularity).
//! The 4 GB OS virtual memory arena (`VirtualAlloc2` / `mmap`) and `SIGSEGV` / `VEH`
//! instruction backpatching will be added here in Phase 3 alongside `ps2-jit`.

/// Guest page size used by the Emotion Engine COP0 TLB and software `vtlb` (4 KB).
///
/// Reference: `ps2tek` ("EE COP0 -> TLB"): smallest page mask `0x000` maps 4 KB even/odd pages.
pub const GUEST_PAGE_SHIFT: usize = 12;
pub const GUEST_PAGE_SIZE: usize = 1 << GUEST_PAGE_SHIFT; // 4096 bytes
pub const GUEST_PAGE_MASK: u32 = (GUEST_PAGE_SIZE as u32) - 1;
pub const VTLB_PAGE_COUNT: usize = 1 << (32 - GUEST_PAGE_SHIFT); // 1,048,576 pages (4 GB)

/// Virtual memory configuration for the emulation core.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct VmemConfig {
    /// Whether OS-backed 4 GB fastmem arena is enabled (always `false` in Phase 1).
    pub enable_os_fastmem: bool,
}
