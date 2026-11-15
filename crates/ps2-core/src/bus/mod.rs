//! Emotion Engine System Bus, Physical/Virtual Memory Dispatch, and Core I/O Map.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#memorymap` & `https://psi-rockin.github.io/ps2tek/#iomaps`):
//!   * `0x0000_0000..=0x01FF_FFFF`: 32 MB Main RDRAM (`8`/`16`/`32`/`64`/`128`-bit).
//!   * `0x1000_0000..=0x1000_FFFF`: 64 KB EE I/O Registers (`Timers`, `INTC`, `DMAC`, `SIF`, `MCH`, `KPUTCHAR`).
//!   * `0x1100_0000..=0x1100_FFFF`: 64 KB VU0/VU1 Micro & Data Memory window:
//!     - `0x1100_0000..=0x1100_0FFF`: 4 KB VU0 Micro Memory (`ps2tek` `#memorymap`).
//!     - `0x1100_4000..=0x1100_4FFF`: 4 KB VU0 Data Memory (`ps2tek` `#memorymap`).
//!     - `0x1100_8000..=0x1100_BFFF`: 16 KB VU1 Micro Memory (`ps2tek` `#memorymap`).
//!     - `0x1100_C000..=0x1100_FFFF`: 16 KB VU1 Data Memory (`ps2tek` `#memorymap`).
//!   * `0x1200_0000..=0x1200_1FFF`: 8 KB GS Privileged Registers (`64`-bit slots).
//!   * `0x1C00_0000..=0x1C1F_FFFF`: 2 MB IOP Main RAM mirror.
//!   * `0x1FC0_0000..=0x1FFF_FFFF`: 4 MB System BIOS ROM (Read-Only).
//!   * `0x7000_0000..=0x7000_3FFF`: 16 KB Scratchpad SRAM (`8`/`16`/`32`/`64`/`128`-bit).

pub mod dmac;
pub mod gs_stub;
pub mod intc;
pub mod rdram_mch;
pub mod sif;
pub mod timers;

use ps2_common::Reg128;

use crate::mmu::{Tlb, TlbEntry, TranslatedAddr, Vtlb, VtlbTarget};
use crate::scheduler::{HardwareEventKind, Scheduler};

pub use dmac::{DmaChannel, EeDmac, UnimplementedDmaTransfer};
pub use gs_stub::GsPrivilegedStub;
pub use intc::{EeIntc, IntcFlags};
pub use rdram_mch::RdramMch;
pub use sif::SifRegs;
pub use timers::EeTimers;

/// 32 MB Main RDRAM size in bytes (`ps2tek` `#memorymap`).
pub const EE_RAM_SIZE: usize = 32 * 1024 * 1024;
/// 16 KB Scratchpad SRAM size in bytes (`ps2tek` `#memorymap`).
pub const SCRATCHPAD_SIZE: usize = 16 * 1024;
/// 4 MB System BIOS ROM size in bytes (`ps2tek` `#memorymap`).
pub const BIOS_SIZE: usize = 4 * 1024 * 1024;
/// 2 MB IOP Main RAM size in bytes (`ps2tek` `#memorymap`).
pub const IOP_RAM_SIZE: usize = 2 * 1024 * 1024;
/// 1 KB IOP Scratchpad SRAM size in bytes (`0x1F80_0000..=0x1F80_03FF`, `ps2tek` `#memorymap`).
pub const IOP_SCRATCHPAD_SIZE: usize = 1024;
/// 64 KB VU0/1 memory window (`ps2tek` `#memorymap`).
pub const VU_MEM_WINDOW_SIZE: usize = 64 * 1024;

/// Diagnostic error recorded when the IOP accesses an unimplemented hardware register.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UnimplementedIopRegister {
    /// Physical IOP address (`0x1F80_1000..=0x1F9F_FFFF`).
    pub paddr: u32,
    /// Access width in bytes (`1`, `2`, or `4`).
    pub width: u8,
    /// `true` if the access was a write, `false` if a read.
    pub is_write: bool,
    /// Value written (or `0` for reads).
    pub value: u32,
}

// TODO(verify): 4x mirroring of 4 KB VU0 Micro (0x1100_0000..=0x1100_3FFF) and 4 KB VU0 Data
// (0x1100_4000..=0x1100_7FFF) across the 16 KB windows is not explicitly stated in ps2tek #memorymap.
/// Normalize a 16-bit offset within `0x1100_0000..=0x1100_FFFF` to account for 4 KB VU0 Micro/Data mirroring.
#[inline(always)]
const fn normalize_vu_mem_offset(offset: u32) -> usize {
    let off = (offset & 0xFFFF) as usize;
    if off < 0x4000 {
        off & 0x0FFF // VU0 Micro: 4 KB mirrored across 0x0000..=0x3FFF
    } else if off < 0x8000 {
        0x4000 | (off & 0x0FFF) // VU0 Data: 4 KB mirrored across 0x4000..=0x7FFF
    } else {
        off // VU1 Micro (0x8000..=0xBFFF) & VU1 Data (0xC000..=0xFFFF): 16 KB each
    }
}

/// Emotion Engine System Bus and backing memory arrays.
pub struct Bus {
    /// 32 MB Main RDRAM (`0x0000_0000..=0x01FF_FFFF`).
    pub ram: Box<[u8]>,
    /// 16 KB Scratchpad SRAM (`0x7000_0000..=0x7000_3FFF`).
    pub scratchpad: Box<[u8]>,
    /// 4 MB System BIOS ROM (`0x1FC0_0000..=0x1FFF_FFFF`).
    pub bios: Box<[u8]>,
    /// 2 MB IOP Main RAM (`0x1C00_0000..=0x1C1F_FFFF` on EE, `0x0000_0000..=0x001F_FFFF` on IOP).
    pub iop_ram: Box<[u8]>,
    /// 1 KB IOP Scratchpad SRAM (`0x1F80_0000..=0x1F80_03FF` on IOP, `ps2tek` `#memorymap`).
    pub iop_scratchpad: Box<[u8]>,
    /// 64 KB VU0/VU1 Micro & Data memory window (`0x1100_0000..=0x1100_FFFF`).
    pub vu_mem: Box<[u8]>,
    /// Architectural 48-entry COP0 TLB.
    pub tlb: Tlb,
    /// Software 4 KB virtual-to-physical page table (`1,048,576` entries).
    pub vtlb: Vtlb,
    /// EE 16-bit Hardware Timers (`TIM0..TIM3`, `0x1000_0000..=0x1000_1FFF`).
    pub timers: EeTimers,
    /// EE Interrupt Controller (`0x1000_F000` / `0x1000_F010`).
    pub intc: EeIntc,
    /// RDRAM initialization handshake registers (`0x1000_F430` / `0x1000_F440`).
    pub rdram_mch: RdramMch,
    /// GS Privileged Registers (`0x1200_0000..=0x1200_1FFF`).
    pub gs: GsPrivilegedStub,
    /// EE 10-Channel DMA Controller (`0x1000_8000..=0x1000_E060`, `0x1000_F520`, `0x1000_F590`).
    pub dmac: EeDmac,
    /// Sub-System Interface (`SIF`) registers (`0x1000_F200..=0x1000_F260`).
    pub sif: SifRegs,
    /// Central event-driven hardware scheduler (`294.912 MHz` EE cycle timebase).
    pub scheduler: Scheduler,
    /// Captured EE/IOP serial & console stdout bytes (`KPUTCHAR` `0x1000_F180` + HLE stdout, `ps2tek` `#iomaps`).
    pub stdout_buffer: Vec<u8>,
    /// Diagnostic error set if the IOP reads or writes an unimplemented IOP MMIO register (`0x1F80_1000..=0x1F9F_FFFF`).
    pub pending_iop_mmio_error: Option<UnimplementedIopRegister>,
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus {
    /// Allocate and initialize the PS2 system bus and default `vtlb` mappings.
    pub fn new() -> Self {
        Self {
            ram: vec![0u8; EE_RAM_SIZE].into_boxed_slice(),
            scratchpad: vec![0u8; SCRATCHPAD_SIZE].into_boxed_slice(),
            bios: vec![0u8; BIOS_SIZE].into_boxed_slice(),
            iop_ram: vec![0u8; IOP_RAM_SIZE].into_boxed_slice(),
            iop_scratchpad: vec![0u8; IOP_SCRATCHPAD_SIZE].into_boxed_slice(),
            vu_mem: vec![0u8; VU_MEM_WINDOW_SIZE].into_boxed_slice(),
            tlb: Tlb::new(),
            vtlb: Vtlb::new(),
            timers: EeTimers::new(),
            intc: EeIntc::new(),
            rdram_mch: RdramMch::new(),
            gs: GsPrivilegedStub::new(),
            dmac: EeDmac::new(),
            sif: SifRegs::new(),
            scheduler: Scheduler::new(),
            stdout_buffer: Vec::new(),
            pending_iop_mmio_error: None,
        }
    }

    /// Load a 4 MB BIOS image into `self.bios`.
    pub fn load_bios(&mut self, bios_bytes: &[u8]) -> Result<(), &'static str> {
        if bios_bytes.len() != BIOS_SIZE {
            return Err("BIOS image must be exactly 4 MB (4,194,304 bytes)");
        }
        self.bios.copy_from_slice(bios_bytes);
        Ok(())
    }

    /// Write a COP0 TLB entry at `index` (`0..48`) and update the software `vtlb`.
    pub fn set_tlb_entry(&mut self, index: usize, entry: TlbEntry) {
        if let Some(slot) = self.tlb.entries.get_mut(index) {
            *slot = entry;
            self.vtlb.map_tlb_entry(&entry);
        }
    }

    /// Read an 8-bit byte from guest virtual address `vaddr`.
    #[inline]
    pub fn read8(&mut self, vaddr: u32) -> u8 {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => self.ram[(offset as usize) & (EE_RAM_SIZE - 1)],
            VtlbTarget::Scratchpad => self.scratchpad[(offset as usize) & (SCRATCHPAD_SIZE - 1)],
            VtlbTarget::Bios => self.bios[(offset as usize) & (BIOS_SIZE - 1)],
            VtlbTarget::IopRam => self.iop_ram[(offset as usize) & (IOP_RAM_SIZE - 1)],
            VtlbTarget::VuMem => self.vu_mem[normalize_vu_mem_offset(offset)],
            VtlbTarget::Mmio => {
                let shift = (offset & 3) * 8;
                ((self.read_mmio32(offset & !3) >> shift) & 0xFF) as u8
            }
            VtlbTarget::Unmapped => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "Unmapped read8");
                0
            }
        }
    }

    /// Read a 16-bit little-endian halfword from guest virtual address `vaddr`.
    #[inline]
    pub fn read16(&mut self, vaddr: u32) -> u16 {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => read_le_u16(&self.ram, (offset as usize) & (EE_RAM_SIZE - 2)),
            VtlbTarget::Scratchpad => {
                read_le_u16(&self.scratchpad, (offset as usize) & (SCRATCHPAD_SIZE - 2))
            }
            VtlbTarget::Bios => read_le_u16(&self.bios, (offset as usize) & (BIOS_SIZE - 2)),
            VtlbTarget::IopRam => {
                read_le_u16(&self.iop_ram, (offset as usize) & (IOP_RAM_SIZE - 2))
            }
            VtlbTarget::VuMem => read_le_u16(&self.vu_mem, normalize_vu_mem_offset(offset & !1)),
            VtlbTarget::Mmio => {
                let shift = (offset & 2) * 8;
                ((self.read_mmio32(offset & !3) >> shift) & 0xFFFF) as u16
            }
            VtlbTarget::Unmapped => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "Unmapped read16");
                0
            }
        }
    }

    /// Read a 32-bit little-endian word from guest virtual address `vaddr`.
    #[inline]
    pub fn read32(&mut self, vaddr: u32) -> u32 {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => read_le_u32(&self.ram, (offset as usize) & (EE_RAM_SIZE - 4)),
            VtlbTarget::Scratchpad => {
                read_le_u32(&self.scratchpad, (offset as usize) & (SCRATCHPAD_SIZE - 4))
            }
            VtlbTarget::Bios => read_le_u32(&self.bios, (offset as usize) & (BIOS_SIZE - 4)),
            VtlbTarget::IopRam => {
                read_le_u32(&self.iop_ram, (offset as usize) & (IOP_RAM_SIZE - 4))
            }
            VtlbTarget::VuMem => read_le_u32(&self.vu_mem, normalize_vu_mem_offset(offset & !3)),
            VtlbTarget::Mmio => self.read_mmio32(offset & !3),
            VtlbTarget::Unmapped => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "Unmapped read32");
                0
            }
        }
    }

    /// Read a 64-bit little-endian doubleword from guest virtual address `vaddr`.
    #[inline]
    pub fn read64(&mut self, vaddr: u32) -> u64 {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => read_le_u64(&self.ram, (offset as usize) & (EE_RAM_SIZE - 8)),
            VtlbTarget::Scratchpad => {
                read_le_u64(&self.scratchpad, (offset as usize) & (SCRATCHPAD_SIZE - 8))
            }
            VtlbTarget::Bios => read_le_u64(&self.bios, (offset as usize) & (BIOS_SIZE - 8)),
            VtlbTarget::IopRam => {
                read_le_u64(&self.iop_ram, (offset as usize) & (IOP_RAM_SIZE - 8))
            }
            VtlbTarget::VuMem => read_le_u64(&self.vu_mem, normalize_vu_mem_offset(offset & !7)),
            VtlbTarget::Mmio => self.read_mmio64(offset & !7),
            VtlbTarget::Unmapped => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "Unmapped read64");
                0
            }
        }
    }

    /// Read a 128-bit quadword (`LQ`) from guest virtual address `vaddr`.
    ///
    /// Reference: `ps2tek` ("EE Core Instruction Set -> LQ"):
    /// `LQ` silently masks the lowest 4 bits of the effective address (`vaddr & !0xF`).
    #[inline]
    pub fn read128(&mut self, vaddr: u32) -> Reg128 {
        let aligned = vaddr & !0xF;
        let TranslatedAddr { target, offset } = self.vtlb.translate(aligned);
        match target {
            VtlbTarget::Ram => read_le_reg128(&self.ram, (offset as usize) & (EE_RAM_SIZE - 16)),
            VtlbTarget::Scratchpad => {
                read_le_reg128(&self.scratchpad, (offset as usize) & (SCRATCHPAD_SIZE - 16))
            }
            VtlbTarget::Bios => read_le_reg128(&self.bios, (offset as usize) & (BIOS_SIZE - 16)),
            VtlbTarget::IopRam => {
                read_le_reg128(&self.iop_ram, (offset as usize) & (IOP_RAM_SIZE - 16))
            }
            VtlbTarget::VuMem => {
                read_le_reg128(&self.vu_mem, normalize_vu_mem_offset(offset & !0xF))
            }
            VtlbTarget::Mmio => {
                let lo = self.read_mmio64(offset & !0xF);
                let hi = self.read_mmio64((offset & !0xF).wrapping_add(8));
                Reg128::from_u64_pair(lo, hi)
            }
            VtlbTarget::Unmapped => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "Unmapped read128");
                Reg128::ZERO
            }
        }
    }

    /// Write an 8-bit byte to guest virtual address `vaddr`.
    #[inline]
    pub fn write8(&mut self, vaddr: u32, val: u8) {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => self.ram[(offset as usize) & (EE_RAM_SIZE - 1)] = val,
            VtlbTarget::Scratchpad => {
                self.scratchpad[(offset as usize) & (SCRATCHPAD_SIZE - 1)] = val;
            }
            VtlbTarget::IopRam => self.iop_ram[(offset as usize) & (IOP_RAM_SIZE - 1)] = val,
            VtlbTarget::VuMem => self.vu_mem[normalize_vu_mem_offset(offset)] = val,
            VtlbTarget::Bios => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "Ignored write8 to BIOS ROM"
                );
            }
            VtlbTarget::Mmio => self.write_mmio8(offset, val),
            VtlbTarget::Unmapped => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "Unmapped write8"
                );
            }
        }
    }

    /// Write a 16-bit little-endian halfword to guest virtual address `vaddr`.
    #[inline]
    pub fn write16(&mut self, vaddr: u32, val: u16) {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => {
                write_le_u16(&mut self.ram, (offset as usize) & (EE_RAM_SIZE - 2), val)
            }
            VtlbTarget::Scratchpad => write_le_u16(
                &mut self.scratchpad,
                (offset as usize) & (SCRATCHPAD_SIZE - 2),
                val,
            ),
            VtlbTarget::IopRam => {
                write_le_u16(
                    &mut self.iop_ram,
                    (offset as usize) & (IOP_RAM_SIZE - 2),
                    val,
                );
            }
            VtlbTarget::VuMem => {
                write_le_u16(&mut self.vu_mem, normalize_vu_mem_offset(offset & !1), val);
            }
            VtlbTarget::Bios => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "Ignored write16 to BIOS ROM"
                );
            }
            VtlbTarget::Mmio => self.write_mmio32(offset & !3, u32::from(val)),
            VtlbTarget::Unmapped => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "Unmapped write16"
                );
            }
        }
    }

    /// Write a 32-bit little-endian word to guest virtual address `vaddr`.
    #[inline]
    pub fn write32(&mut self, vaddr: u32, val: u32) {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => {
                write_le_u32(&mut self.ram, (offset as usize) & (EE_RAM_SIZE - 4), val)
            }
            VtlbTarget::Scratchpad => write_le_u32(
                &mut self.scratchpad,
                (offset as usize) & (SCRATCHPAD_SIZE - 4),
                val,
            ),
            VtlbTarget::IopRam => {
                write_le_u32(
                    &mut self.iop_ram,
                    (offset as usize) & (IOP_RAM_SIZE - 4),
                    val,
                );
            }
            VtlbTarget::VuMem => {
                write_le_u32(&mut self.vu_mem, normalize_vu_mem_offset(offset & !3), val);
            }
            VtlbTarget::Bios => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "Ignored write32 to BIOS ROM"
                );
            }
            VtlbTarget::Mmio => self.write_mmio32(offset & !3, val),
            VtlbTarget::Unmapped => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "Unmapped write32"
                );
            }
        }
    }

    /// Write a 64-bit little-endian doubleword to guest virtual address `vaddr`.
    #[inline]
    pub fn write64(&mut self, vaddr: u32, val: u64) {
        let TranslatedAddr { target, offset } = self.vtlb.translate(vaddr);
        match target {
            VtlbTarget::Ram => {
                write_le_u64(&mut self.ram, (offset as usize) & (EE_RAM_SIZE - 8), val)
            }
            VtlbTarget::Scratchpad => write_le_u64(
                &mut self.scratchpad,
                (offset as usize) & (SCRATCHPAD_SIZE - 8),
                val,
            ),
            VtlbTarget::IopRam => {
                write_le_u64(
                    &mut self.iop_ram,
                    (offset as usize) & (IOP_RAM_SIZE - 8),
                    val,
                );
            }
            VtlbTarget::VuMem => {
                write_le_u64(&mut self.vu_mem, normalize_vu_mem_offset(offset & !7), val);
            }
            VtlbTarget::Bios => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "Ignored write64 to BIOS ROM"
                );
            }
            VtlbTarget::Mmio => self.write_mmio64(offset & !7, val),
            VtlbTarget::Unmapped => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "Unmapped write64"
                );
            }
        }
    }

    /// Write a 128-bit quadword (`SQ`) to guest virtual address `vaddr`.
    ///
    /// Reference: `ps2tek` ("EE Core Instruction Set -> SQ"):
    /// `SQ` silently masks the lowest 4 bits of the effective address (`vaddr & !0xF`).
    #[inline]
    pub fn write128(&mut self, vaddr: u32, val: Reg128) {
        let aligned = vaddr & !0xF;
        let TranslatedAddr { target, offset } = self.vtlb.translate(aligned);
        match target {
            VtlbTarget::Ram => {
                write_le_reg128(&mut self.ram, (offset as usize) & (EE_RAM_SIZE - 16), val);
            }
            VtlbTarget::Scratchpad => write_le_reg128(
                &mut self.scratchpad,
                (offset as usize) & (SCRATCHPAD_SIZE - 16),
                val,
            ),
            VtlbTarget::IopRam => {
                write_le_reg128(
                    &mut self.iop_ram,
                    (offset as usize) & (IOP_RAM_SIZE - 16),
                    val,
                );
            }
            VtlbTarget::VuMem => {
                write_le_reg128(
                    &mut self.vu_mem,
                    normalize_vu_mem_offset(offset & !0xF),
                    val,
                );
            }
            VtlbTarget::Bios => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "Ignored write128 to BIOS ROM"
                );
            }
            VtlbTarget::Mmio => {
                self.write_mmio64(offset & !0xF, val.lo_u64());
                self.write_mmio64((offset & !0xF).wrapping_add(8), val.hi_u64());
            }
            VtlbTarget::Unmapped => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "Unmapped write128");
            }
        }
    }

    /// Advance active EE hardware timers (`TIM0..TIM3`) and the central hardware [`Scheduler`]
    /// by `ee_cycles`, updating `self.intc` and completing due DMAC channel events.
    #[inline]
    pub fn step_timers(&mut self, ee_cycles: u32) {
        self.timers.step_ee_cycles(ee_cycles, &mut self.intc);
        self.scheduler.advance(u64::from(ee_cycles));
        while let Some(event) = self.scheduler.pop_due_event() {
            if let HardwareEventKind::DmacChannelComplete(ch) = event.kind {
                self.dmac.complete_channel(usize::from(ch));
            }
        }
    }

    #[cold]
    fn read_mmio32(&mut self, paddr: u32) -> u32 {
        match paddr {
            0x1000_0000..=0x1000_1FFF => self.timers.read32(paddr),
            0x1000_8000..=0x1000_E060 | 0x1000_F520 | 0x1000_F590 => self.dmac.read32(paddr),
            0x1000_F000 => self.intc.read_stat(),
            0x1000_F010 => self.intc.read_mask(),
            0x1000_F200..=0x1000_F260 => self.sif.read32_ee(paddr),
            0x1000_F430 => self.rdram_mch.read_f430(),
            0x1000_F440 => self.rdram_mch.read_f440(),
            0x1200_0000..=0x1200_1FFF => self.gs.read32(paddr),
            _ => {
                tracing::trace!(
                    paddr = format_args!("0x{paddr:08X}"),
                    "Unhandled MMIO read32"
                );
                0
            }
        }
    }

    #[cold]
    fn read_mmio64(&mut self, paddr: u32) -> u64 {
        match paddr {
            0x1200_0000..=0x1200_1FFF => self.gs.read64(paddr),
            _ => {
                let lo = u64::from(self.read_mmio32(paddr));
                let hi = u64::from(self.read_mmio32(paddr.wrapping_add(4)));
                lo | (hi << 32)
            }
        }
    }

    #[cold]
    fn write_mmio8(&mut self, paddr: u32, val: u8) {
        if paddr == 0x1000_F180 {
            // KPUTCHAR register (`ps2tek` `https://psi-rockin.github.io/ps2tek/#iomaps`):
            // captures kernel serial console character output.
            self.stdout_buffer.push(val);
            return;
        }
        self.write_mmio32(paddr & !3, u32::from(val));
    }

    #[cold]
    fn write_mmio32(&mut self, paddr: u32, val: u32) {
        match paddr {
            0x1000_0000..=0x1000_1FFF => self.timers.write32(paddr, val),
            0x1000_8000..=0x1000_E060 | 0x1000_F520 | 0x1000_F590 => {
                self.dmac.write32(
                    paddr,
                    val,
                    &mut self.ram,
                    &mut self.scratchpad,
                    &mut self.scheduler,
                );
            }
            0x1000_F000 => self.intc.write_stat(val),
            0x1000_F010 => self.intc.write_mask(val),
            0x1000_F180 => self.stdout_buffer.push((val & 0xFF) as u8),
            0x1000_F200..=0x1000_F260 => self.sif.write32_ee(paddr, val),
            0x1000_F430 => self.rdram_mch.write_f430(val),
            0x1000_F440 => self.rdram_mch.write_f440(val),
            0x1200_0000..=0x1200_1FFF => self.gs.write32(paddr, val),
            _ => {
                tracing::trace!(
                    paddr = format_args!("0x{paddr:08X}"),
                    val = format_args!("0x{val:08X}"),
                    "Unhandled MMIO write32"
                );
            }
        }
    }

    #[cold]
    fn write_mmio64(&mut self, paddr: u32, val: u64) {
        match paddr {
            0x1200_0000..=0x1200_1FFF => self.gs.write64(paddr, val),
            _ => {
                self.write_mmio32(paddr, val as u32);
                self.write_mmio32(paddr.wrapping_add(4), (val >> 32) as u32);
            }
        }
    }

    // -------------------------------------------------------------------------
    // R3000A IOP Memory Map (`ps2tek` `#memorymap`)
    // - `0x0000_0000..=0x001F_FFFF` (mirrored across `0x0000_0000..=0x007F_FFFF` in KUSEG/KSEG0/KSEG1):
    //   2 MB IOP Main RAM.
    // - `0x1F80_0000..=0x1F80_03FF`: 1 KB IOP Scratchpad SRAM.
    // - `0x1F80_1000..=0x1F9F_FFFF`: IOP Hardware Registers (unimplemented in Task 6a; raises
    //   `UnimplementedIopRegister`).
    // - `0x1FC0_0000..=0x1FFF_FFFF`: 4 MB System BIOS ROM (Read-Only).
    // -------------------------------------------------------------------------

    /// Read an 8-bit byte from the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_read8(&mut self, vaddr: u32) -> u8 {
        let paddr = vaddr & 0x1FFF_FFFF;
        match paddr {
            0x0000_0000..=0x007F_FFFF => self.iop_ram[(paddr as usize) & (IOP_RAM_SIZE - 1)],
            0x1F80_0000..=0x1F80_03FF => {
                self.iop_scratchpad[(paddr as usize) & (IOP_SCRATCHPAD_SIZE - 1)]
            }
            0x1FC0_0000..=0x1FFF_FFFF => self.bios[(paddr as usize) & (BIOS_SIZE - 1)],
            0x1F80_1000..=0x1F9F_FFFF => {
                self.record_unimplemented_iop_reg(paddr, 1, false, 0);
                0
            }
            _ => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "IOP unmapped read8");
                0
            }
        }
    }

    /// Read a 16-bit little-endian halfword from the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_read16(&mut self, vaddr: u32) -> u16 {
        let paddr = vaddr & 0x1FFF_FFFF;
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                read_le_u16(&self.iop_ram, (paddr as usize) & (IOP_RAM_SIZE - 2))
            }
            0x1F80_0000..=0x1F80_03FF => read_le_u16(
                &self.iop_scratchpad,
                (paddr as usize) & (IOP_SCRATCHPAD_SIZE - 2),
            ),
            0x1FC0_0000..=0x1FFF_FFFF => {
                read_le_u16(&self.bios, (paddr as usize) & (BIOS_SIZE - 2))
            }
            0x1F80_1000..=0x1F9F_FFFF => {
                self.record_unimplemented_iop_reg(paddr, 2, false, 0);
                0
            }
            _ => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "IOP unmapped read16");
                0
            }
        }
    }

    /// Read a 32-bit little-endian word from the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_read32(&mut self, vaddr: u32) -> u32 {
        let paddr = vaddr & 0x1FFF_FFFF;
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                read_le_u32(&self.iop_ram, (paddr as usize) & (IOP_RAM_SIZE - 4))
            }
            0x1F80_0000..=0x1F80_03FF => read_le_u32(
                &self.iop_scratchpad,
                (paddr as usize) & (IOP_SCRATCHPAD_SIZE - 4),
            ),
            0x1FC0_0000..=0x1FFF_FFFF => {
                read_le_u32(&self.bios, (paddr as usize) & (BIOS_SIZE - 4))
            }
            0x1F80_1000..=0x1F9F_FFFF => {
                self.record_unimplemented_iop_reg(paddr, 4, false, 0);
                0
            }
            _ => {
                tracing::debug!(vaddr = format_args!("0x{vaddr:08X}"), "IOP unmapped read32");
                0
            }
        }
    }

    /// Write an 8-bit byte to the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_write8(&mut self, vaddr: u32, val: u8) {
        let paddr = vaddr & 0x1FFF_FFFF;
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                self.iop_ram[(paddr as usize) & (IOP_RAM_SIZE - 1)] = val;
            }
            0x1F80_0000..=0x1F80_03FF => {
                self.iop_scratchpad[(paddr as usize) & (IOP_SCRATCHPAD_SIZE - 1)] = val;
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "IOP ignored write8 to BIOS ROM"
                );
            }
            0x1F80_1000..=0x1F9F_FFFF => {
                self.record_unimplemented_iop_reg(paddr, 1, true, val as u32);
            }
            _ => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "IOP unmapped write8"
                );
            }
        }
    }

    /// Write a 16-bit little-endian halfword to the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_write16(&mut self, vaddr: u32, val: u16) {
        let paddr = vaddr & 0x1FFF_FFFF;
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                write_le_u16(
                    &mut self.iop_ram,
                    (paddr as usize) & (IOP_RAM_SIZE - 2),
                    val,
                );
            }
            0x1F80_0000..=0x1F80_03FF => {
                write_le_u16(
                    &mut self.iop_scratchpad,
                    (paddr as usize) & (IOP_SCRATCHPAD_SIZE - 2),
                    val,
                );
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "IOP ignored write16 to BIOS ROM"
                );
            }
            0x1F80_1000..=0x1F9F_FFFF => {
                self.record_unimplemented_iop_reg(paddr, 2, true, val as u32);
            }
            _ => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "IOP unmapped write16"
                );
            }
        }
    }

    /// Write a 32-bit little-endian word to the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_write32(&mut self, vaddr: u32, val: u32) {
        let paddr = vaddr & 0x1FFF_FFFF;
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                write_le_u32(
                    &mut self.iop_ram,
                    (paddr as usize) & (IOP_RAM_SIZE - 4),
                    val,
                );
            }
            0x1F80_0000..=0x1F80_03FF => {
                write_le_u32(
                    &mut self.iop_scratchpad,
                    (paddr as usize) & (IOP_SCRATCHPAD_SIZE - 4),
                    val,
                );
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "IOP ignored write32 to BIOS ROM"
                );
            }
            0x1F80_1000..=0x1F9F_FFFF => {
                self.record_unimplemented_iop_reg(paddr, 4, true, val);
            }
            _ => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    val,
                    "IOP unmapped write32"
                );
            }
        }
    }

    #[cold]
    fn record_unimplemented_iop_reg(&mut self, paddr: u32, width: u8, is_write: bool, value: u32) {
        tracing::warn!(
            paddr = format_args!("0x{paddr:08X}"),
            width,
            is_write,
            value = format_args!("0x{value:08X}"),
            "Unimplemented IOP hardware register access"
        );
        if self.pending_iop_mmio_error.is_none() {
            self.pending_iop_mmio_error = Some(UnimplementedIopRegister {
                paddr,
                width,
                is_write,
                value,
            });
        }
    }
}

#[inline(always)]
fn read_le_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

#[inline(always)]
fn read_le_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

#[inline(always)]
fn read_le_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}

#[inline(always)]
fn read_le_reg128(buf: &[u8], offset: usize) -> Reg128 {
    let lo = read_le_u64(buf, offset);
    let hi = read_le_u64(buf, offset + 8);
    Reg128::from_u64_pair(lo, hi)
}

#[inline(always)]
fn write_le_u16(buf: &mut [u8], offset: usize, val: u16) {
    let bytes = val.to_le_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
}

#[inline(always)]
fn write_le_u32(buf: &mut [u8], offset: usize, val: u32) {
    let bytes = val.to_le_bytes();
    buf[offset..offset + 4].copy_from_slice(&bytes);
}

#[inline(always)]
fn write_le_u64(buf: &mut [u8], offset: usize, val: u64) {
    let bytes = val.to_le_bytes();
    buf[offset..offset + 8].copy_from_slice(&bytes);
}

#[inline(always)]
fn write_le_reg128(buf: &mut [u8], offset: usize, val: Reg128) {
    write_le_u64(buf, offset, val.lo_u64());
    write_le_u64(buf, offset + 8, val.hi_u64());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_ram_mirrors_and_scratchpad_access_widths() {
        let mut bus = Bus::new();

        // Write 32-bit via KUSEG cached (0x0010_0000), read back via KSEG0, KSEG1, Uncached (0x2010_0000), and UCAB (0x3010_0000).
        bus.write32(0x0010_0000, 0xDEAD_BEEF);
        assert_eq!(bus.read32(0x8010_0000), 0xDEAD_BEEF);
        assert_eq!(bus.read32(0xA010_0000), 0xDEAD_BEEF);
        assert_eq!(bus.read32(0x2010_0000), 0xDEAD_BEEF);
        assert_eq!(bus.read32(0x3010_0000), 0xDEAD_BEEF);
        assert_eq!(bus.read16(0x0010_0002), 0xDEAD);
        assert_eq!(bus.read8(0x0010_0000), 0xEF);

        // 128-bit LQ/SQ on Scratchpad (0x7000_0000) masks low 4 address bits (& !0xF).
        let qword = Reg128::from_u64_pair(0x0123_4567_89AB_CDEF, 0xFEDC_BA98_7654_3210);
        bus.write128(0x7000_0107, qword);
        assert_eq!(bus.read128(0x7000_0100), qword);
        assert_eq!(bus.read128(0x7000_010F), qword);
        assert_eq!(bus.read64(0x7000_0108), 0xFEDC_BA98_7654_3210);
    }

    #[test]
    fn bus_vu0_memory_mirroring_and_mmio_dispatch() {
        let mut bus = Bus::new();

        // VU0 Micro memory (4 KB at 0x1100_0000) mirrors across 0x1100_1000, 0x1100_2000, 0x1100_3000.
        bus.write32(0x1100_0120, 0xCAFE_BABE);
        assert_eq!(bus.read32(0x1100_1120), 0xCAFE_BABE);
        assert_eq!(bus.read32(0x1100_3120), 0xCAFE_BABE);

        // VU0 Data memory (4 KB at 0x1100_4000) mirrors across 0x1100_5000..0x1100_7FFF.
        bus.write32(0x1100_4080, 0x1234_5678);
        assert_eq!(bus.read32(0x1100_7080), 0x1234_5678);

        // GS Privileged CSR (0x1200_1000) VBlank flag set & write-1-to-clear.
        bus.gs.on_vblank_start();
        assert_ne!(bus.read64(0x1200_1000) & (1 << 3), 0);
        bus.write64(0x1200_1000, 1 << 3);
        assert_eq!(bus.read64(0x1200_1000) & (1 << 3), 0);

        // DMAC D_ENABLER (0x1000_F520) / D_ENABLEW (0x1000_F590).
        assert_eq!(bus.read32(0x1000_F520), 0x1201);
        bus.write32(0x1000_F590, 0x0001_1201);
        assert_eq!(bus.read32(0x1000_F520), 0x0001_1201);
    }

    #[test]
    fn iop_memory_map_ram_scratchpad_and_unimplemented_mmio_error() {
        let mut bus = Bus::new();

        // 2 MB IOP RAM mirrored across KUSEG (0x0001_0000), KSEG0 (0x8001_0000), and KSEG1 (0xA001_0000).
        bus.iop_write32(0x0001_0000, 0x1122_3344);
        assert_eq!(bus.iop_read32(0x8001_0000), 0x1122_3344);
        assert_eq!(bus.iop_read32(0xA001_0000), 0x1122_3344);
        assert_eq!(bus.iop_read16(0x0001_0002), 0x1122);
        assert_eq!(bus.iop_read8(0x0001_0000), 0x44);

        // 1 KB IOP Scratchpad at 0x1F80_0000..=0x1F80_03FF.
        bus.iop_write32(0x1F80_0100, 0xA5A5_5A5A);
        assert_eq!(bus.iop_read32(0x1F80_0100), 0xA5A5_5A5A);
        assert_eq!(bus.iop_read16(0x1F80_0102), 0xA5A5);

        // Unimplemented IOP hardware register (e.g., IOP I_STAT at 0x1F80_1070) records explicit error.
        assert_eq!(bus.pending_iop_mmio_error, None);
        bus.iop_write32(0xBF80_1070, 0x0000_0001);
        assert_eq!(
            bus.pending_iop_mmio_error,
            Some(UnimplementedIopRegister {
                paddr: 0x1F80_1070,
                width: 4,
                is_write: true,
                value: 1,
            })
        );
    }
}
