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
use crate::vu::micro::{exec_macro_op, step_micro_liw};
use crate::vu::{VuState, VU0_TPC_MASK, VU1_TPC_MASK, VU_REVISION_ID};

pub use dmac::{DmaChannel, EeDmac, UnimplementedDmaTransfer};
pub use gs_stub::GsPrivilegedStub;
pub use intc::{EeIntc, IntcFlags, IopIntc, IopIrqFlags};
pub use rdram_mch::RdramMch;
pub use sif::SifRegs;
pub use timers::EeTimers;

/// 32 MB Main RDRAM size in bytes (`ps2tek` `#memorymap`, `FETCHED`).
pub const EE_RAM_SIZE: usize = 32 * 1024 * 1024;
/// 16 KB Scratchpad SRAM size in bytes (`ps2tek` `#memorymap`, `FETCHED`).
pub const SCRATCHPAD_SIZE: usize = 16 * 1024;
/// 4 MB System BIOS ROM size in bytes (`ps2tek` `#memorymap`, `FETCHED`).
pub const BIOS_SIZE: usize = 4 * 1024 * 1024;
/// 2 MB IOP Main RAM size in bytes (`ps2tek` `#memorymap`, `FETCHED`).
pub const IOP_RAM_SIZE: usize = 2 * 1024 * 1024;
/// 1 KB IOP Scratchpad SRAM size in bytes (`0x1F80_0000..=0x1F80_03FF`, `ps2tek` `#memorymap`, `FETCHED`).
pub const IOP_SCRATCHPAD_SIZE: usize = 1024;
/// 64 KB VU0/1 memory window (`ps2tek` `#memorymap`, `FETCHED`).
pub const VU_MEM_WINDOW_SIZE: usize = 64 * 1024;

/// Emotion Engine on-chip Serial I/O (`EE_SIO` / UART, `0x1000_F100..=0x1000_F1C0`).
///
/// Primary source references (`FETCHED`):
/// - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iomaps`, `local/ps2tek/index.html` line 212):
///   * `0x1000_F180` (`KPUTCHAR` / `SIO_TXFIFO`, 8-bit): kernel serial console character output.
/// - `ps2sdk` (`ee/kernel/include/sio.h` & `ee/kernel/src/sio.c`, `FETCHED`):
///   * `0x1000_F100` (`SIO_LCR`): Line Control Register.
///   * `0x1000_F110` (`SIO_LSR`): Line Status Register.
///   * `0x1000_F120` (`SIO_IER`): Interrupt Enable Register.
///   * `0x1000_F130` (`SIO_ISR`): Interrupt Status Register (`sio_putc` polls `(_lw(SIO_ISR) & 0xf000) == 0x8000`
///     for TX FIFO full; `0` = ready to transmit; bits `0..=2` write-1-to-clear).
///   * `0x1000_F140` (`SIO_FCR`): FIFO Control Register.
///   * `0x1000_F150` (`SIO_BGR`): Baud Rate Generator Register.
///   * `0x1000_F180` (`SIO_TXFIFO`): Transmit FIFO.
///   * `0x1000_F1C0` (`SIO_RXFIFO`): Receive FIFO.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct EeSio {
    /// `0x1000_F100` (`SIO_LCR`): Line Control Register.
    pub lcr: u32,
    /// `0x1000_F110` (`SIO_LSR`): Line Status Register.
    pub lsr: u32,
    /// `0x1000_F120` (`SIO_IER`): Interrupt Enable Register.
    pub ier: u32,
    /// `0x1000_F130` (`SIO_ISR`): Interrupt Status Register.
    pub isr: u32,
    /// `0x1000_F140` (`SIO_FCR`): FIFO Control Register.
    pub fcr: u32,
    /// `0x1000_F150` (`SIO_BGR`): Baud Rate Generator Register.
    pub bgr: u32,
    /// `0x1000_F1C0` (`SIO_RXFIFO`): Receive FIFO.
    pub rxfifo: u32,
}

/// R3000A IOP Sub-System Bus Controller (`SSBUSC`) and Memory Control registers.
///
/// Primary source references (`FETCHED`):
/// - `ps2sdk` (`iop/system/ssbusc/include/ssbusc.h`, `iop/system/ssbusc/src/ssbusc.c`,
///   `common/include/iop_regs.h`, and `common/include/iop_mmio_hwport.h`, `FETCHED`):
///   * `0x1F80_1000..=0x1F80_1020`: `ssbus1_mmio_hwport_t` device base and delay/configuration
///     registers (`0x1F80_1010` = `BIOS_ROM` delay/size; bit 12 `0x1000` is `ATYP` [`0 = 8-bit`, `1 = 16-bit`],
///     preserved via read-modify-write during IOP BIOS boot at `PC = 0xBFC0_20C4`).
///   * `0x1F80_1060`: `A_PS1_RAM_SIZE` (`RAM_SIZE` register).
///   * `0x1F80_1400..=0x1F80_1420`: `ssbus2_mmio_hwport_t` extended PS2 `SSBUSC` base/delay registers.
// TODO(verify): `ps2tek` (`FETCHED`) omits the IOP `SSBUSC` timing/delay register bitfields (`0x1F80_1000..=0x1F80_1020`,
// `0x1F80_1060`, `0x1F80_1400..=0x1F80_1420`); verified against `ps2sdk` `iop/system/ssbusc/include/ssbusc.h` (`FETCHED`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IopSsbusc {
    /// `0x1F80_1000..=0x1F80_1023` (`9` words: `EXP1_BASE` .. `COM_DELAY`).
    pub memctrl1: [u32; 9],
    /// `0x1F80_1060` (`RAM_SIZE`).
    pub ram_size: u32,
    /// `0x1F80_1400..=0x1F80_1423` (`9` words: PS2 extended `SSBUSC` base/delay registers).
    pub ssbus_ext: [u32; 9],
}

/// Per-channel R3000A IOP DMA Controller registers (`Dn_MADR`, `Dn_BCR`, `Dn_CHCR`, `Dn_TADR`).
///
/// Primary source references (`FETCHED`):
/// - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iopdma`, `local/ps2tek/index.html` lines 3970-4050).
/// - `ps2sdk` (`iop/system/dmacman/src/dmacman.c` lines 20-73, 108-340).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IopDmacChannel {
    /// `Dn_MADR` (`+0x0`): Start address in IOP RAM (24-bit, `0x00FF_FFFF` mask).
    pub madr: u32,
    /// `Dn_BCR` (`+0x4`): Block Control Register (`[15:0]` block size in words, `[31:16]` block count).
    pub bcr: u32,
    /// `Dn_CHCR` (`+0x8`): Channel Control Register (bit `24` = start/busy).
    pub chcr: u32,
    /// `Dn_TADR` (`+0xC`): Tag Address Register in IOP RAM (24-bit, used in chain mode on `SPU`, `SIF0`, `SIF1`).
    pub tadr: u32,
}

/// R3000A IOP DMA Controller channel, priority, and interrupt control registers
/// (`0x1F80_1080..=0x1F80_10F4`, `0x1F80_1500..=0x1F80_157C`, `0x1F80_15F0`).
///
/// Primary source references (`FETCHED`):
/// - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#iopdma`, `local/ps2tek/index.html` lines 3965-4118):
///   * `0x1F80_1080..=0x1F80_10EC`: Channels `0..6` (`MDECin`, `MDECout`, `SIF2`, `CDVD`, `SPU1`, `PIO`, `OTC`).
///   * `0x1F80_10F0` (`DPCR`): DMA Priority/Enable for channels 0..6 (initial value on PS2 reset: `0x0777_7777`).
///   * `0x1F80_10F4` (`DICR`): DMA Interrupt Register for channels 0..6 (bits `24..=30` write-1-to-clear,
///     bit `31` = `DICR.15 | (DICR.23 && (DICR.24..30 || DICR2.24..29))` when `DMACINTEN.0 == 1`).
///   * `0x1F80_1500..=0x1F80_155C`: Channels `7..12` (`SPU2`, `DEV9`, `SIF0`, `SIF1`, `SIO2in`, `SIO2out`).
///   * `0x1F80_1570` (`DPCR2`): DMA Priority/Enable 2 for channels 7..12.
///   * `0x1F80_1574` (`DICR2`): DMA Interrupt Register 2 for channels 7..12 (bits `24..=29` write-1-to-clear).
///   * `0x1F80_1578` (`DMACEN`): Global DMA Enable (bit `0`: `0` = all transfers disabled, `1` = enabled).
///   * `0x1F80_157C` (`DMACINTEN`): Global DMA Interrupt Control (bit `0`: when `0`, master interrupt flag is `0`
///     except for bus error; bit `1`: when `1`, IRQ 3 is masked from INTC).
/// - `ps2sdk` (`iop/system/dmacman/src/dmacman.c` lines 59-64, 342-401, 453-474, `FETCHED`):
///   * `0x1F80_1560`, `0x1F80_1564`, `0x1F80_1568`: `dmac_channel_9_4_9_a` (`SIF0`), `dmac_channel_A_4_9_a` (`SIF1`),
///     `dmac_channel_4_4_9_a` (`SPU`).
///   * `0x1F80_15F0` (`DPCR3`): DMA Priority/Enable 3 for `FDMA0..2`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct IopDmacControl {
    /// Per-channel register banks for IOP DMA channels `0..12`.
    pub channels: [IopDmacChannel; 13],
    /// `0x1F80_1560..=0x1F80_1568`: Auxiliary registers for `SIF0` (`[0]`), `SIF1` (`[1]`), and `SPU` (`[2]`).
    pub ch_aux_4_9_a: [u32; 3],
    /// `0x1F80_10F0` (`DPCR`): DMA Priority/Enable (`0..6`).
    pub dpcr: u32,
    /// `0x1F80_10F4` (`DICR`): DMA Interrupt Register (`0..6`).
    pub dicr: u32,
    /// `0x1F80_1570` (`DPCR2`): DMA Priority/Enable 2 (`7..12`).
    pub dpcr2: u32,
    /// `0x1F80_1574` (`DICR2`): DMA Interrupt Register 2 (`7..12`).
    pub dicr2: u32,
    /// `0x1F80_15F0` (`DPCR3`): DMA Priority/Enable 3 (`FDMA0..2`).
    pub dpcr3: u32,
    /// `0x1F80_1578` (`DMACEN`): Global DMA Enable.
    pub dmacen: u32,
    /// `0x1F80_157C` (`DMACINTEN`): Global DMA Interrupt Control.
    pub dmacinten: u32,
}

impl Default for IopDmacControl {
    fn default() -> Self {
        Self::new()
    }
}

impl IopDmacControl {
    /// Initialize IOP DMA control registers to PS2 power-on defaults (`ps2tek` `#iopdma` line 4066, `FETCHED`).
    #[inline]
    pub const fn new() -> Self {
        Self {
            channels: [IopDmacChannel {
                madr: 0,
                bcr: 0,
                chcr: 0,
                tadr: 0,
            }; 13],
            ch_aux_4_9_a: [0; 3],
            dpcr: 0x0777_7777,
            dicr: 0,
            dpcr2: 0,
            dicr2: 0,
            dpcr3: 0,
            dmacen: 0,
            dmacinten: 0,
        }
    }

    /// Read a per-channel register (`MADR`, `BCR`, `CHCR`, `TADR`) for channel `ch` (`0..12`).
    #[inline]
    pub const fn read_channel_reg(&self, ch: usize, reg_offset: u32) -> u32 {
        let c = &self.channels[ch];
        match reg_offset & 0xC {
            0x0 => c.madr,
            0x4 => c.bcr,
            0x8 => c.chcr,
            0xC => c.tadr,
            _ => 0,
        }
    }

    /// Write a per-channel register (`MADR`, `BCR`, `CHCR`, `TADR`) for channel `ch` (`0..12`).
    /// Returns `true` if `Dn_CHCR` bit 24 (`Start/Busy`) was written as `1`.
    #[inline]
    pub fn write_channel_reg(&mut self, ch: usize, reg_offset: u32, val: u32) -> bool {
        let c = &mut self.channels[ch];
        match reg_offset & 0xC {
            0x0 => {
                c.madr = val & 0x00FF_FFFF;
                false
            }
            0x4 => {
                c.bcr = val;
                false
            }
            0x8 => {
                c.chcr = val;
                (val & (1 << 24)) != 0
            }
            0xC => {
                c.tadr = val & 0x00FF_FFFF;
                false
            }
            _ => false,
        }
    }

    /// Recompute `DICR.31` master interrupt flag (`ps2tek` `#iopdma` lines 4093-4094, 4116, `FETCHED`).
    #[inline]
    pub fn recompute_master_flag(&mut self) {
        let bus_err = (self.dicr & (1 << 15)) != 0;
        let ch_int_enabled = (self.dmacinten & 1) != 0 && (self.dicr & (1 << 23)) != 0;
        let any_ch_flag = ((self.dicr >> 24) & 0x7F) != 0 || ((self.dicr2 >> 24) & 0x3F) != 0;
        if bus_err || (ch_int_enabled && any_ch_flag) {
            self.dicr |= 1 << 31;
        } else {
            self.dicr &= !(1 << 31);
        }
    }

    /// Returns `true` if `DICR.31` is set and `DMACINTEN.1` does not mask `IRQ 3` (`ps2tek` `#iopdma` lines 4093, 4117, `FETCHED`).
    #[inline(always)]
    pub const fn irq3_asserted(&self) -> bool {
        (self.dicr & (1 << 31)) != 0 && (self.dmacinten & 2) == 0
    }

    /// Write `DICR` (`0x1F80_10F4`), clearing bits `24..=30` where `1` is written (`ps2tek` `#iopdma` line 4091, `FETCHED`).
    #[inline]
    pub fn write_dicr(&mut self, val: u32) {
        const RW_MASK: u32 = 0x00FF_807F;
        const W1C_MASK: u32 = 0x7F00_0000;
        let cleared_flags = (self.dicr & W1C_MASK) & !(val & W1C_MASK);
        self.dicr = (val & RW_MASK) | cleared_flags;
        self.recompute_master_flag();
    }

    /// Write `DICR2` (`0x1F80_1574`), clearing bits `24..=29` where `1` is written (`ps2tek` `#iopdma` line 4104, `FETCHED`).
    #[inline]
    pub fn write_dicr2(&mut self, val: u32) {
        const RW_MASK: u32 = 0x003F_1FFF;
        const W1C_MASK: u32 = 0x3F00_0000;
        let cleared_flags = (self.dicr2 & W1C_MASK) & !(val & W1C_MASK);
        self.dicr2 = (val & RW_MASK) | cleared_flags;
        self.recompute_master_flag();
    }
}

/// Diagnostic error recorded when the EE accesses an unimplemented hardware register.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UnimplementedEeRegister {
    /// Physical EE address (`0x1000_0000..=0x1FFF_FFFF`).
    pub paddr: u32,
    /// Access width in bytes (`1`, `2`, `4`, or `8`).
    pub width: u8,
    /// `true` if the access was a write, `false` if a read.
    pub is_write: bool,
    /// Value written (low 32 bits, or `0` for reads).
    pub value: u32,
}

/// Diagnostic error recorded when the IOP accesses an unimplemented hardware register.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UnimplementedIopRegister {
    /// Physical IOP address.
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
    /// Vector Unit 0 (`VU0`) architectural state (`COP2` macro + micro mode).
    pub vu0: VuState,
    /// Vector Unit 1 (`VU1`) architectural state (micro mode).
    pub vu1: VuState,
    /// Architectural 48-entry COP0 TLB.
    pub tlb: Tlb,
    /// Software 4 KB virtual-to-physical page table (`1,048,576` entries).
    pub vtlb: Vtlb,
    /// EE 16-bit Hardware Timers (`TIM0..TIM3`, `0x1000_0000..=0x1000_1FFF`).
    pub timers: EeTimers,
    /// EE Interrupt Controller (`0x1000_F000` / `0x1000_F010`).
    pub intc: EeIntc,
    /// EE Serial I/O (`EE_SIO`, `0x1000_F100..=0x1000_F1C0`).
    pub sio: EeSio,
    /// RDRAM initialization handshake registers (`0x1000_F430` / `0x1000_F440`).
    pub rdram_mch: RdramMch,
    /// GS Privileged Registers (`0x1200_0000..=0x1200_1FFF`).
    pub gs: GsPrivilegedStub,
    /// EE 10-Channel DMA Controller (`0x1000_8000..=0x1000_E060`, `0x1000_F520`, `0x1000_F590`).
    pub dmac: EeDmac,
    /// Sub-System Interface (`SIF`) registers (`0x1000_F200..=0x1000_F260`).
    pub sif: SifRegs,
    /// IOP Sub-System Bus Controller (`SSBUSC`, `0x1F80_1000..=0x1F80_1020`, `0x1F80_1060`, `0x1F80_1400..=0x1F80_1420`).
    pub iop_ssbusc: IopSsbusc,
    /// IOP Interrupt Controller (`I_STAT` `0x1F80_1070`, `I_MASK` `0x1F80_1074`, `I_CTRL` `0x1F80_1078`).
    pub iop_intc: IopIntc,
    /// IOP DMA Controller global priority and interrupt registers (`0x1F80_10F0..=0x1F80_10F4`, `0x1F80_1570..=0x1F80_157C`).
    pub iop_dmac: IopDmacControl,
    /// IOP `0x1F80_1450` (`GM_IF` in `ps2sdk` `common/include/iop_regs.h` & `iop/system/intrman/src/intrman.c`, `FETCHED`):
    /// IOP mode & CPU type register (`0` on reset = PS2 mode / MIPS R3000A).
    // TODO(verify): `ps2tek` `#ps1drv` line 5095 (`FETCHED`) writes `1F801570.3` (a digit-transposition typo for
    // `1F801450.3`); `ps2sdk` `iop/system/intrman/src/intrman.c` lines 51-65 (`FETCHED`) confirms `0x1F80_1450` bit 3
    // (`0x08`) selects PS1 (`1`) vs PS2 (`0`) IOP bootstrap mode.
    pub iop_gm_if: u32,
    /// IOP Expansion Region 2 POST2 diagnostic status latch (`0x1F80_2070`, `ps2sdk` `common/include/iop_mmio_hwport.h`, `FETCHED`).
    // TODO(verify): `ps2tek` (`FETCHED`) omits `0x1F80_2070` (`A_IOP_UNK_2070` / `HW_POST2`); written by the IOP BIOS
    // boot table (`PC = 0xBFC0_20E8`, `val = 0x0000_00FC`) as a power-on self-test status latch.
    pub iop_post2: u32,
    /// IOP `KSEG2` Cache Control registers (`0xFFFE_0130..=0xFFFE_014F`, `ps2tek` `#memorymap` line 106, `FETCHED`).
    // TODO(verify): `ps2tek` `#memorymap` line 106 (`FETCHED`) lists `FFFE0000h (KSEG2) Cache control` without
    // per-offset bitfields (`0xFFFE_0130` `BIU_CONFIG` and `0xFFFE_0140..=0xFFFE_014F`).
    pub iop_cache_ctrl: [u32; 8],
    /// Central event-driven hardware scheduler (`294.912 MHz` EE cycle timebase).
    pub scheduler: Scheduler,
    /// Captured EE/IOP serial & console stdout bytes (`KPUTCHAR` `0x1000_F180` + HLE stdout, `ps2tek` `#iomaps`, `FETCHED`).
    pub stdout_buffer: Vec<u8>,
    /// Diagnostic error set if the EE reads or writes an unimplemented EE MMIO register.
    pub pending_ee_mmio_error: Option<UnimplementedEeRegister>,
    /// Diagnostic error set if the IOP reads or writes an unimplemented IOP MMIO register.
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
            vu0: VuState::new(),
            vu1: VuState::new(),
            tlb: Tlb::new(),
            vtlb: Vtlb::new(),
            timers: EeTimers::new(),
            intc: EeIntc::new(),
            sio: EeSio::default(),
            rdram_mch: RdramMch::new(),
            gs: GsPrivilegedStub::new(),
            dmac: EeDmac::new(),
            sif: SifRegs::new(),
            iop_ssbusc: IopSsbusc::default(),
            iop_intc: IopIntc::new(),
            iop_dmac: IopDmacControl::new(),
            iop_gm_if: 0,
            iop_post2: 0,
            iop_cache_ctrl: [0; 8],
            scheduler: Scheduler::new(),
            stdout_buffer: Vec::new(),
            pending_ee_mmio_error: None,
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
            VtlbTarget::Mmio => self.read_mmio8(offset),
            VtlbTarget::Unmapped => {
                self.record_unimplemented_ee_reg(vaddr, 1, false, 0);
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
            VtlbTarget::Mmio => self.read_mmio16(offset & !1),
            VtlbTarget::Unmapped => {
                self.record_unimplemented_ee_reg(vaddr, 2, false, 0);
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
                self.record_unimplemented_ee_reg(vaddr, 4, false, 0);
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
                self.record_unimplemented_ee_reg(vaddr, 8, false, 0);
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
                self.record_unimplemented_ee_reg(vaddr, 1, true, u32::from(val));
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
            VtlbTarget::Mmio => self.write_mmio16(offset & !1, val),
            VtlbTarget::Unmapped => {
                self.record_unimplemented_ee_reg(vaddr, 2, true, u32::from(val));
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
                self.record_unimplemented_ee_reg(vaddr, 4, true, val);
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
                self.record_unimplemented_ee_reg(vaddr, 8, true, val as u32);
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
                self.record_unimplemented_ee_reg(vaddr, 16, true, val.lo_u64() as u32);
            }
        }
    }

    /// Advance active EE hardware timers (`TIM0..TIM3`), running `VU0`/`VU1` microprograms,
    /// and the central hardware [`Scheduler`] by `ee_cycles`, updating `self.intc` and
    /// completing due DMAC channel events.
    #[inline]
    pub fn step_timers(&mut self, ee_cycles: u32) {
        self.timers.step_ee_cycles(ee_cycles, &mut self.intc);
        if self.vu0.running || self.vu1.running {
            let vu_steps = 1.max(ee_cycles / 2);
            for _ in 0..vu_steps {
                if self.vu0.running {
                    let (vu0_code, rest) = self.vu_mem.split_at_mut(0x4000);
                    step_micro_liw(
                        &mut self.vu0,
                        &vu0_code[..0x1000],
                        &mut rest[..0x1000],
                        VU0_TPC_MASK,
                    );
                }
                if self.vu1.running {
                    let (_, vu1_half) = self.vu_mem.split_at_mut(0x8000);
                    let (vu1_code, vu1_data) = vu1_half.split_at_mut(0x4000);
                    step_micro_liw(&mut self.vu1, vu1_code, vu1_data, VU1_TPC_MASK);
                }
                if !self.vu0.running && !self.vu1.running {
                    break;
                }
            }
        }
        self.scheduler.advance(u64::from(ee_cycles));
        while let Some(event) = self.scheduler.pop_due_event() {
            if let HardwareEventKind::DmacChannelComplete(ch) = event.kind {
                self.dmac.complete_channel(usize::from(ch));
            }
        }
    }

    /// Execute a single `COP2` macro-mode instruction on `self.vu0`.
    #[inline]
    pub fn exec_vu0_macro(&mut self, instr: u32) {
        exec_macro_op(&mut self.vu0, &mut self.vu_mem[0x4000..0x5000], instr);
    }

    /// Start `VU0` microprogram execution at `start_tpc` (8-byte LIW instruction index).
    #[inline]
    pub fn start_vu0_micro(&mut self, start_tpc: u16) {
        self.vu0.start_micro(start_tpc, VU0_TPC_MASK);
    }

    /// Start `VU1` microprogram execution at `start_tpc` (8-byte LIW instruction index).
    #[inline]
    pub fn start_vu1_micro(&mut self, start_tpc: u16) {
        self.vu1.start_micro(start_tpc, VU1_TPC_MASK);
    }

    /// Interlock helper (`CFC2.I`, `CTC2.I`, `QMFC2.I`, `QMTC2.I`): step `VU0` until idle.
    #[inline]
    pub fn finish_vu0_micro(&mut self) {
        let mut guard = 0u32;
        while self.vu0.running && guard < 65_536 {
            let (vu0_code, rest) = self.vu_mem.split_at_mut(0x4000);
            step_micro_liw(
                &mut self.vu0,
                &vu0_code[..0x1000],
                &mut rest[..0x1000],
                VU0_TPC_MASK,
            );
            guard += 1;
        }
    }

    /// Read a 32-bit `COP2` control register (`vi00..vi31`) via `CFC2`.
    #[inline]
    pub const fn read_cfc2(&self, reg: usize) -> u32 {
        match reg & 0x1F {
            0 => 0,
            r @ 1..=15 => self.vu0.vi[r] as u32,
            16 => self.vu0.status_flag as u32,
            17 => self.vu0.mac_flag as u32,
            18 => self.vu0.clip_flag & 0x00FF_FFFF,
            19 => VU_REVISION_ID,
            20 => self.vu0.r & 0x007F_FFFF,
            21 => self.vu0.i,
            22 => self.vu0.q,
            23 => 0,
            24 => self.vu0.fbrst_ctrl as u32,
            25 => 0,
            26 => self.vu0.tpc as u32,
            27 => self.vu0.cmsar0 as u32,
            28 => 0,
            29 => (self.vu0.running as u32) | ((self.vu1.running as u32) << 8),
            30 => 0,
            31 => self.vu1.cmsar1 as u32,
            _ => 0,
        }
    }

    /// Write a 32-bit `COP2` control register (`vi00..vi31`) via `CTC2`.
    #[inline]
    pub fn write_ctc2(&mut self, reg: usize, val: u32) {
        match reg & 0x1F {
            0 => {}
            r @ 1..=15 => self.vu0.vi[r] = val as u16,
            16 => self.vu0.status_flag = (val as u16) & 0x0FC0,
            17 => {}
            18 => self.vu0.clip_flag = val & 0x00FF_FFFF,
            19 => {}
            20 => self.vu0.r = val & 0x007F_FFFF,
            21 => self.vu0.i = val,
            22 => self.vu0.q = val,
            23 => {}
            24 => self.vu0.fbrst_ctrl = (val as u16) & 0x0C0C,
            25 | 26 => {}
            27 => self.vu0.cmsar0 = val as u16,
            28 => {
                if (val & 0x0001) != 0 {
                    self.vu0.running = false;
                }
                if (val & 0x0002) != 0 {
                    self.vu0.reset_unit();
                }
                if (val & 0x0100) != 0 {
                    self.vu1.running = false;
                }
                if (val & 0x0200) != 0 {
                    self.vu1.reset_unit();
                }
            }
            29 | 30 => {}
            31 => {
                self.vu1.cmsar1 = val as u16;
                if !self.vu1.running {
                    self.start_vu1_micro(val as u16);
                }
            }
            _ => {}
        }
    }

    #[cold]
    fn read_mmio8(&mut self, paddr: u32) -> u8 {
        match paddr & !3 {
            0x1000_0000..=0x1000_1FFF
            | 0x1000_8000..=0x1000_E060
            | 0x1000_F000
            | 0x1000_F010
            | 0x1000_F100
            | 0x1000_F110
            | 0x1000_F120
            | 0x1000_F130
            | 0x1000_F140
            | 0x1000_F150
            | 0x1000_F180
            | 0x1000_F1C0
            | 0x1000_F200..=0x1000_F260
            | 0x1000_F400..=0x1000_F4F0
            | 0x1000_F500
            | 0x1000_F520
            | 0x1000_F590
            | 0x1200_0000..=0x1200_1FFF => {
                let shift = (paddr & 3) * 8;
                ((self.read_mmio32(paddr & !3) >> shift) & 0xFF) as u8
            }
            _ => {
                self.record_unimplemented_ee_reg(paddr, 1, false, 0);
                0
            }
        }
    }

    #[cold]
    fn read_mmio16(&mut self, paddr: u32) -> u16 {
        match paddr & !3 {
            0x1000_0000..=0x1000_1FFF
            | 0x1000_8000..=0x1000_E060
            | 0x1000_F000
            | 0x1000_F010
            | 0x1000_F100
            | 0x1000_F110
            | 0x1000_F120
            | 0x1000_F130
            | 0x1000_F140
            | 0x1000_F150
            | 0x1000_F180
            | 0x1000_F1C0
            | 0x1000_F200..=0x1000_F260
            | 0x1000_F400..=0x1000_F4F0
            | 0x1000_F500
            | 0x1000_F520
            | 0x1000_F590
            | 0x1200_0000..=0x1200_1FFF => {
                let shift = (paddr & 2) * 8;
                ((self.read_mmio32(paddr & !3) >> shift) & 0xFFFF) as u16
            }
            _ => {
                self.record_unimplemented_ee_reg(paddr, 2, false, 0);
                0
            }
        }
    }

    #[cold]
    fn read_mmio32(&mut self, paddr: u32) -> u32 {
        match paddr {
            0x1000_0000..=0x1000_1FFF => self.timers.read32(paddr),
            0x1000_8000..=0x1000_E060 | 0x1000_F500 | 0x1000_F520 | 0x1000_F590 => {
                self.dmac.read32(paddr)
            }
            0x1000_F000 => self.intc.read_stat(),
            0x1000_F010 => self.intc.read_mask(),
            0x1000_F100 => self.sio.lcr,
            0x1000_F110 => self.sio.lsr,
            0x1000_F120 => self.sio.ier,
            0x1000_F130 => self.sio.isr,
            0x1000_F140 => self.sio.fcr,
            0x1000_F150 => self.sio.bgr,
            0x1000_F180 => 0,
            0x1000_F1C0 => self.sio.rxfifo,
            0x1000_F200..=0x1000_F260 => self.sif.read32_ee(paddr),
            0x1000_F400..=0x1000_F4F0 if (paddr & 0xF) == 0 => self.rdram_mch.read32(paddr),
            0x1200_0000..=0x1200_1FFF => self.gs.read32(paddr),
            _ => {
                self.record_unimplemented_ee_reg(paddr, 4, false, 0);
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
        match paddr & !3 {
            0x1000_0000..=0x1000_1FFF
            | 0x1000_8000..=0x1000_E060
            | 0x1000_F000
            | 0x1000_F010
            | 0x1000_F100
            | 0x1000_F110
            | 0x1000_F120
            | 0x1000_F130
            | 0x1000_F140
            | 0x1000_F150
            | 0x1000_F1C0
            | 0x1000_F200..=0x1000_F260
            | 0x1000_F400..=0x1000_F4F0
            | 0x1000_F500
            | 0x1000_F520
            | 0x1000_F590
            | 0x1200_0000..=0x1200_1FFF => self.write_mmio32(paddr & !3, u32::from(val)),
            _ => self.record_unimplemented_ee_reg(paddr, 1, true, u32::from(val)),
        }
    }

    #[cold]
    fn write_mmio16(&mut self, paddr: u32, val: u16) {
        match paddr & !3 {
            0x1000_0000..=0x1000_1FFF
            | 0x1000_8000..=0x1000_E060
            | 0x1000_F000
            | 0x1000_F010
            | 0x1000_F100
            | 0x1000_F110
            | 0x1000_F120
            | 0x1000_F130
            | 0x1000_F140
            | 0x1000_F150
            | 0x1000_F180
            | 0x1000_F1C0
            | 0x1000_F200..=0x1000_F260
            | 0x1000_F400..=0x1000_F4F0
            | 0x1000_F500
            | 0x1000_F520
            | 0x1000_F590
            | 0x1200_0000..=0x1200_1FFF => self.write_mmio32(paddr & !3, u32::from(val)),
            _ => self.record_unimplemented_ee_reg(paddr, 2, true, u32::from(val)),
        }
    }

    #[cold]
    fn write_mmio32(&mut self, paddr: u32, val: u32) {
        match paddr {
            0x1000_0000..=0x1000_1FFF => self.timers.write32(paddr, val),
            0x1000_8000..=0x1000_E060 | 0x1000_F500 | 0x1000_F520 | 0x1000_F590 => {
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
            0x1000_F100 => self.sio.lcr = val,
            0x1000_F110 => self.sio.lsr = val,
            0x1000_F120 => self.sio.ier = val,
            0x1000_F130 => self.sio.isr &= !(val & 0x7),
            0x1000_F140 => self.sio.fcr = val,
            0x1000_F150 => self.sio.bgr = val,
            0x1000_F180 => self.stdout_buffer.push((val & 0xFF) as u8),
            0x1000_F1C0 => self.sio.rxfifo = val,
            0x1000_F200..=0x1000_F260 => self.sif.write32_ee(paddr, val),
            0x1000_F400..=0x1000_F4F0 if (paddr & 0xF) == 0 => self.rdram_mch.write32(paddr, val),
            0x1200_0000..=0x1200_1FFF => self.gs.write32(paddr, val),
            _ => {
                self.record_unimplemented_ee_reg(paddr, 4, true, val);
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

    #[cold]
    fn record_unimplemented_ee_reg(&mut self, paddr: u32, width: u8, is_write: bool, value: u32) {
        tracing::warn!(
            paddr = format_args!("0x{paddr:08X}"),
            width,
            is_write,
            value = format_args!("0x{value:08X}"),
            "Unimplemented EE hardware register access"
        );
        if self.pending_ee_mmio_error.is_none() {
            self.pending_ee_mmio_error = Some(UnimplementedEeRegister {
                paddr,
                width,
                is_write,
                value,
            });
        }
    }

    // -------------------------------------------------------------------------
    // R3000A IOP Memory Map (`ps2tek` `#memorymap`)
    // - `0x0000_0000..=0x001F_FFFF` (mirrored across `0x0000_0000..=0x007F_FFFF` in KUSEG/KSEG0/KSEG1):
    //   2 MB IOP Main RAM.
    // - `0x1F80_0000..=0x1F80_03FF`: 1 KB IOP Scratchpad SRAM.
    // - `0x1FC0_0000..=0x1FFF_FFFF`: 4 MB System BIOS ROM (Read-Only).
    // - Any other IOP physical address raises `UnimplementedIopRegister`.
    // -------------------------------------------------------------------------

    /// Translate an R3000A IOP virtual address to its physical/bus address (`ps2tek` `#memorymap`).
    /// `KUSEG` (`0x0000_0000..=0x7FFF_FFFF`), `KSEG0` (`0x8000_0000..=0x9FFF_FFFF`), and
    /// `KSEG1` (`0xA000_0000..=0xBFFF_FFFF`) strip the top 3 segment bits; `KSEG2` (`0xC000_0000..=0xFFFF_FFFF`)
    /// preserves the full address (`0xFFFE_0130` Cache Control).
    #[inline(always)]
    const fn iop_translate_addr(vaddr: u32) -> u32 {
        if vaddr >= 0xC000_0000 {
            vaddr
        } else {
            vaddr & 0x1FFF_FFFF
        }
    }

    /// Read an 8-bit byte from the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_read8(&mut self, vaddr: u32) -> u8 {
        let paddr = Self::iop_translate_addr(vaddr);
        match paddr {
            0x0000_0000..=0x007F_FFFF => self.iop_ram[(paddr as usize) & (IOP_RAM_SIZE - 1)],
            0x1F80_0000..=0x1F80_03FF => {
                self.iop_scratchpad[(paddr as usize) & (IOP_SCRATCHPAD_SIZE - 1)]
            }
            0x1F80_1000..=0x1F80_1023
            | 0x1F80_1060..=0x1F80_1063
            | 0x1F80_1070..=0x1F80_107B
            | 0x1F80_1080..=0x1F80_10F7
            | 0x1F80_1400..=0x1F80_1423
            | 0x1F80_1450..=0x1F80_1453
            | 0x1F80_1500..=0x1F80_157F
            | 0x1F80_15F0..=0x1F80_15F3
            | 0x1F80_2070..=0x1F80_2073
            | 0xFFFE_0130..=0xFFFE_014F => {
                let shift = (paddr & 3) * 8;
                let word = self.iop_read32(paddr & !3);
                ((word >> shift) & 0xFF) as u8
            }
            0x1FC0_0000..=0x1FFF_FFFF => self.bios[(paddr as usize) & (BIOS_SIZE - 1)],
            _ => {
                self.record_unimplemented_iop_reg(paddr, 1, false, 0);
                0
            }
        }
    }

    /// Read a 16-bit little-endian halfword from the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_read16(&mut self, vaddr: u32) -> u16 {
        let paddr = Self::iop_translate_addr(vaddr);
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                read_le_u16(&self.iop_ram, (paddr as usize) & (IOP_RAM_SIZE - 2))
            }
            0x1F80_0000..=0x1F80_03FF => read_le_u16(
                &self.iop_scratchpad,
                (paddr as usize) & (IOP_SCRATCHPAD_SIZE - 2),
            ),
            0x1F80_1000..=0x1F80_1023
            | 0x1F80_1060..=0x1F80_1063
            | 0x1F80_1070..=0x1F80_107B
            | 0x1F80_1080..=0x1F80_10F7
            | 0x1F80_1400..=0x1F80_1423
            | 0x1F80_1450..=0x1F80_1453
            | 0x1F80_1500..=0x1F80_157F
            | 0x1F80_15F0..=0x1F80_15F3
            | 0x1F80_2070..=0x1F80_2073
            | 0xFFFE_0130..=0xFFFE_014F => {
                let shift = (paddr & 2) * 8;
                let word = self.iop_read32(paddr & !3);
                ((word >> shift) & 0xFFFF) as u16
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                read_le_u16(&self.bios, (paddr as usize) & (BIOS_SIZE - 2))
            }
            _ => {
                self.record_unimplemented_iop_reg(paddr, 2, false, 0);
                0
            }
        }
    }

    /// Read a 32-bit little-endian word from the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_read32(&mut self, vaddr: u32) -> u32 {
        let paddr = Self::iop_translate_addr(vaddr);
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                read_le_u32(&self.iop_ram, (paddr as usize) & (IOP_RAM_SIZE - 4))
            }
            0x1F80_0000..=0x1F80_03FF => read_le_u32(
                &self.iop_scratchpad,
                (paddr as usize) & (IOP_SCRATCHPAD_SIZE - 4),
            ),
            0x1F80_1000..=0x1F80_1023 => {
                self.iop_ssbusc.memctrl1[((paddr - 0x1F80_1000) >> 2) as usize]
            }
            0x1F80_1060 => self.iop_ssbusc.ram_size,
            0x1F80_1070 => self.iop_intc.read_stat(),
            0x1F80_1074 => self.iop_intc.read_mask(),
            0x1F80_1078 => self.iop_intc.read_ctrl(),
            0x1F80_1080..=0x1F80_10EC => {
                let ch = ((paddr - 0x1F80_1080) >> 4) as usize;
                self.iop_dmac.read_channel_reg(ch, paddr & 0xF)
            }
            0x1F80_10F0 => self.iop_dmac.dpcr,
            0x1F80_10F4 => self.iop_dmac.dicr,
            0x1F80_1400..=0x1F80_1423 => {
                self.iop_ssbusc.ssbus_ext[((paddr - 0x1F80_1400) >> 2) as usize]
            }
            0x1F80_1450 => self.iop_gm_if,
            0x1F80_1500..=0x1F80_155C => {
                let ch = 7 + (((paddr - 0x1F80_1500) >> 4) as usize);
                self.iop_dmac.read_channel_reg(ch, paddr & 0xF)
            }
            0x1F80_1560..=0x1F80_1568 => {
                self.iop_dmac.ch_aux_4_9_a[((paddr - 0x1F80_1560) >> 2) as usize]
            }
            0x1F80_1570 => self.iop_dmac.dpcr2,
            0x1F80_1574 => self.iop_dmac.dicr2,
            0x1F80_1578 => self.iop_dmac.dmacen,
            0x1F80_157C => self.iop_dmac.dmacinten,
            0x1F80_15F0 => self.iop_dmac.dpcr3,
            0x1F80_2070 => self.iop_post2,
            0xFFFE_0130..=0xFFFE_014F => self.iop_cache_ctrl[((paddr - 0xFFFE_0130) >> 2) as usize],
            0x1FC0_0000..=0x1FFF_FFFF => {
                read_le_u32(&self.bios, (paddr as usize) & (BIOS_SIZE - 4))
            }
            _ => {
                self.record_unimplemented_iop_reg(paddr, 4, false, 0);
                0
            }
        }
    }

    /// Write an 8-bit byte to the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_write8(&mut self, vaddr: u32, val: u8) {
        let paddr = Self::iop_translate_addr(vaddr);
        match paddr {
            0x0000_0000..=0x007F_FFFF => {
                self.iop_ram[(paddr as usize) & (IOP_RAM_SIZE - 1)] = val;
            }
            0x1F80_0000..=0x1F80_03FF => {
                self.iop_scratchpad[(paddr as usize) & (IOP_SCRATCHPAD_SIZE - 1)] = val;
            }
            0x1F80_1070..=0x1F80_1073 => {
                let shift = (paddr & 3) * 8;
                let mask32 = !(0xFFu32 << shift) | (u32::from(val) << shift);
                self.iop_intc.write_stat(mask32);
            }
            0x1F80_1078..=0x1F80_107B => {
                if (paddr & 3) == 0 {
                    self.iop_intc.write_ctrl(u32::from(val));
                }
            }
            0x1F80_1000..=0x1F80_1023
            | 0x1F80_1060..=0x1F80_1063
            | 0x1F80_1074..=0x1F80_1077
            | 0x1F80_1080..=0x1F80_10F7
            | 0x1F80_1400..=0x1F80_1423
            | 0x1F80_1450..=0x1F80_1453
            | 0x1F80_1500..=0x1F80_157F
            | 0x1F80_15F0..=0x1F80_15F3
            | 0x1F80_2070..=0x1F80_2073
            | 0xFFFE_0130..=0xFFFE_014F => {
                let shift = (paddr & 3) * 8;
                let aligned = paddr & !3;
                let word = self.iop_read32(aligned);
                self.iop_write32(
                    aligned,
                    (word & !(0xFF << shift)) | (u32::from(val) << shift),
                );
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "IOP ignored write8 to BIOS ROM"
                );
            }
            _ => {
                self.record_unimplemented_iop_reg(paddr, 1, true, val as u32);
            }
        }
    }

    /// Write a 16-bit little-endian halfword to the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_write16(&mut self, vaddr: u32, val: u16) {
        let paddr = Self::iop_translate_addr(vaddr);
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
            0x1F80_1070..=0x1F80_1073 => {
                let shift = (paddr & 2) * 8;
                let mask32 = !(0xFFFFu32 << shift) | (u32::from(val) << shift);
                self.iop_intc.write_stat(mask32);
            }
            0x1F80_1078..=0x1F80_107B => {
                if (paddr & 2) == 0 {
                    self.iop_intc.write_ctrl(u32::from(val));
                }
            }
            0x1F80_1000..=0x1F80_1023
            | 0x1F80_1060..=0x1F80_1063
            | 0x1F80_1074..=0x1F80_1077
            | 0x1F80_1080..=0x1F80_10F7
            | 0x1F80_1400..=0x1F80_1423
            | 0x1F80_1450..=0x1F80_1453
            | 0x1F80_1500..=0x1F80_157F
            | 0x1F80_15F0..=0x1F80_15F3
            | 0x1F80_2070..=0x1F80_2073
            | 0xFFFE_0130..=0xFFFE_014F => {
                let shift = (paddr & 2) * 8;
                let aligned = paddr & !3;
                let word = self.iop_read32(aligned);
                self.iop_write32(
                    aligned,
                    (word & !(0xFFFF << shift)) | (u32::from(val) << shift),
                );
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "IOP ignored write16 to BIOS ROM"
                );
            }
            _ => {
                self.record_unimplemented_iop_reg(paddr, 2, true, val as u32);
            }
        }
    }

    /// Write a 32-bit little-endian word to the R3000A IOP virtual address space.
    #[inline]
    pub fn iop_write32(&mut self, vaddr: u32, val: u32) {
        let paddr = Self::iop_translate_addr(vaddr);
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
            0x1F80_1000..=0x1F80_1023 => {
                self.iop_ssbusc.memctrl1[((paddr - 0x1F80_1000) >> 2) as usize] = val;
            }
            0x1F80_1060 => {
                self.iop_ssbusc.ram_size = val;
            }
            0x1F80_1070 => {
                self.iop_intc.write_stat(val);
            }
            0x1F80_1074 => {
                self.iop_intc.write_mask(val);
            }
            0x1F80_1078 => {
                self.iop_intc.write_ctrl(val);
            }
            0x1F80_1080..=0x1F80_10EC => {
                let ch = ((paddr - 0x1F80_1080) >> 4) as usize;
                if self.iop_dmac.write_channel_reg(ch, paddr & 0xF, val) {
                    self.record_unimplemented_iop_reg(paddr, 4, true, val);
                }
            }
            0x1F80_10F0 => {
                self.iop_dmac.dpcr = val;
            }
            0x1F80_10F4 => {
                self.iop_dmac.write_dicr(val);
                if self.iop_dmac.irq3_asserted() {
                    self.iop_intc.raise_irq(IopIrqFlags::DMA);
                }
            }
            0x1F80_1400..=0x1F80_1423 => {
                self.iop_ssbusc.ssbus_ext[((paddr - 0x1F80_1400) >> 2) as usize] = val;
            }
            0x1F80_1450 => {
                self.iop_gm_if = val;
            }
            0x1F80_1500..=0x1F80_155C => {
                let ch = 7 + (((paddr - 0x1F80_1500) >> 4) as usize);
                if self.iop_dmac.write_channel_reg(ch, paddr & 0xF, val) {
                    self.record_unimplemented_iop_reg(paddr, 4, true, val);
                }
            }
            0x1F80_1560..=0x1F80_1568 => {
                self.iop_dmac.ch_aux_4_9_a[((paddr - 0x1F80_1560) >> 2) as usize] = val;
            }
            0x1F80_1570 => {
                self.iop_dmac.dpcr2 = val;
            }
            0x1F80_1574 => {
                self.iop_dmac.write_dicr2(val);
                if self.iop_dmac.irq3_asserted() {
                    self.iop_intc.raise_irq(IopIrqFlags::DMA);
                }
            }
            0x1F80_1578 => {
                self.iop_dmac.dmacen = val & 1;
            }
            0x1F80_157C => {
                self.iop_dmac.dmacinten = val & 3;
                self.iop_dmac.recompute_master_flag();
                if self.iop_dmac.irq3_asserted() {
                    self.iop_intc.raise_irq(IopIrqFlags::DMA);
                }
            }
            0x1F80_15F0 => {
                self.iop_dmac.dpcr3 = val;
            }
            0x1F80_2070 => {
                self.iop_post2 = val;
            }
            0xFFFE_0130..=0xFFFE_014F => {
                self.iop_cache_ctrl[((paddr - 0xFFFE_0130) >> 2) as usize] = val;
            }
            0x1FC0_0000..=0x1FFF_FFFF => {
                tracing::debug!(
                    vaddr = format_args!("0x{vaddr:08X}"),
                    "IOP ignored write32 to BIOS ROM"
                );
            }
            _ => {
                self.record_unimplemented_iop_reg(paddr, 4, true, val);
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

        // Unimplemented IOP hardware register (e.g., CDVD at 0x1F80_1800) records explicit error.
        assert_eq!(bus.pending_iop_mmio_error, None);
        bus.iop_write32(0xBF80_1800, 0x0000_0001);
        assert_eq!(
            bus.pending_iop_mmio_error,
            Some(UnimplementedIopRegister {
                paddr: 0x1F80_1800,
                width: 4,
                is_write: true,
                value: 1,
            })
        );
    }

    #[test]
    fn ee_f500_and_mch_f4xx_read_write_and_reset_semantics() {
        let mut bus = Bus::new();

        // EE 0x1000_F500 reset value is 0; R/W does not disturb D_ENABLER (0x1000_F520 = 0x1201).
        assert_eq!(bus.read32(0x1000_F500), 0);
        assert_eq!(bus.read32(0x1000_F520), 0x1201);
        bus.write32(0x1000_F500, 0x0000_0001);
        assert_eq!(bus.read32(0x1000_F500), 0x0000_0001);
        assert_eq!(bus.read8(0x1000_F500), 0x01);
        assert_eq!(bus.read32(0x1000_F520), 0x1201);

        // EE MCH 0x1000_F400..=0x1000_F4F0: non-F430/F440 slots (e.g. 0x1000_F410) round-trip 32-bit values.
        assert_eq!(bus.read32(0x1000_F410), 0);
        bus.write32(0x1000_F410, 0x8000_0000);
        assert_eq!(bus.read32(0x1000_F410), 0x8000_0000);
        assert_eq!(bus.pending_ee_mmio_error, None);
    }

    #[test]
    fn ee_sio_registers_isr_w1c_and_txfifo_stdout_capture() {
        let mut bus = Bus::new();

        // Reset values are 0; SIO_ISR & 0xF000 != 0x8000 indicates TX FIFO ready.
        for addr in [
            0x1000_F100,
            0x1000_F110,
            0x1000_F120,
            0x1000_F130,
            0x1000_F140,
            0x1000_F150,
            0x1000_F180,
            0x1000_F1C0,
        ] {
            assert_eq!(bus.read32(addr), 0);
        }

        // R/W on LCR, LSR, IER, FCR, BGR, RXFIFO.
        bus.write32(0x1000_F100, 0x0000_0023);
        bus.write32(0x1000_F110, 0x0000_0007);
        bus.write32(0x1000_F120, 0x0000_0005);
        bus.write32(0x1000_F140, 0x0000_000F);
        bus.write32(0x1000_F150, 0x0000_004E);
        bus.write32(0x1000_F1C0, 0x0000_0055);
        assert_eq!(bus.read32(0x1000_F100), 0x23);
        assert_eq!(bus.read32(0x1000_F110), 0x07);
        assert_eq!(bus.read32(0x1000_F120), 0x05);
        assert_eq!(bus.read32(0x1000_F140), 0x0F);
        assert_eq!(bus.read32(0x1000_F150), 0x4E);
        assert_eq!(bus.read32(0x1000_F1C0), 0x55);

        // SIO_ISR (0x1000_F130): low 3 bits are write-1-to-clear.
        bus.sio.isr = 0b111;
        bus.write32(0x1000_F130, 0b101);
        assert_eq!(bus.read32(0x1000_F130), 0b010);

        // SIO_TXFIFO / KPUTCHAR (0x1000_F180): 8-bit and 32-bit writes append to stdout_buffer.
        bus.write8(0x1000_F180, b'A');
        bus.write32(0x1000_F180, u32::from(b'B'));
        assert_eq!(&bus.stdout_buffer, b"AB");
        assert_eq!(bus.read32(0x1000_F180), 0);
        assert_eq!(bus.pending_ee_mmio_error, None);
    }

    #[test]
    fn iop_gm_if_ssbusc_post2_and_kseg2_cache_ctrl_semantics() {
        let mut bus = Bus::new();

        // IOP GM_IF (0x1F80_1450): reset value 0 (bit 3 == 0 selects PS2 mode, bit 31 == 0 selects R3000A).
        assert_eq!(bus.iop_read32(0xBF80_1450), 0);
        bus.iop_write32(0xBF80_1450, 0x0000_00A5);
        assert_eq!(bus.iop_read32(0xBF80_1450), 0x0000_00A5);
        assert_eq!(bus.iop_read8(0xBF80_1450), 0xA5);

        // IOP SSBUSC memctrl1 (0x1F80_1000..=0x1F80_1020), ram_size (0x1F80_1060), and ssbus_ext (0x1F80_1400..=0x1F80_1420).
        // Verify ATYP bit 12 (0x1000) read-modify-write on 0xBF80_1010 (BIOS_ROM delay/size).
        assert_eq!(bus.iop_read32(0xBF80_1010), 0);
        bus.iop_write32(0xBF80_1010, 0x0000_1000);
        let at = bus.iop_read32(0xBF80_1010) & 0x1000;
        bus.iop_write32(0xBF80_1010, 0x0013_243F | at);
        assert_eq!(bus.iop_read32(0xBF80_1010), 0x0013_343F);

        bus.iop_write32(0xBF80_1060, 0x0000_0B88);
        assert_eq!(bus.iop_read32(0xBF80_1060), 0x0000_0B88);

        bus.iop_write32(0xBF80_1414, 0x0005_1234);
        assert_eq!(bus.iop_read32(0xBF80_1414), 0x0005_1234);

        // IOP POST2 (0x1F80_2070): 8-bit, 16-bit, and 32-bit R/W.
        assert_eq!(bus.iop_read32(0xBF80_2070), 0);
        bus.iop_write8(0xBF80_2070, 0xFC);
        assert_eq!(bus.iop_read8(0xBF80_2070), 0xFC);
        assert_eq!(bus.iop_read32(0xBF80_2070), 0x0000_00FC);

        // IOP KSEG2 Cache Control (0xFFFE_0130..=0xFFFE_014F): does not alias BIOS ROM (0x1FFE_0130).
        bus.bios[0x003E_0130..0x003E_0134].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert_eq!(bus.iop_read32(0xFFFE_0130), 0);
        bus.iop_write32(0xFFFE_0130, 0x0001_E988);
        bus.iop_write32(0xFFFE_0144, 0x0000_0001);
        assert_eq!(bus.iop_read32(0xFFFE_0130), 0x0001_E988);
        assert_eq!(bus.iop_read32(0xFFFE_0144), 0x0000_0001);
        assert_eq!(bus.iop_read32(0xBFFE_0130), 0xDEAD_BEEF);
        assert_eq!(bus.pending_iop_mmio_error, None);
    }

    #[test]
    fn iop_dmac_control_and_intc_irq3_propagation() {
        let mut bus = Bus::new();

        // DPCR (0x1F80_10F0) resets to 0x0777_7777 on PS2 (`ps2tek` `#iopdma` line 4066).
        assert_eq!(bus.iop_read32(0xBF80_10F0), 0x0777_7777);
        assert_eq!(bus.iop_read32(0xBF80_10F4), 0);
        assert_eq!(bus.iop_read32(0xBF80_1570), 0);
        assert_eq!(bus.iop_read32(0xBF80_1574), 0);
        assert_eq!(bus.iop_read32(0xBF80_1578), 0);
        assert_eq!(bus.iop_read32(0xBF80_157C), 0);

        // Simulate channel 0 interrupt flag set in DICR (bit 24) and channel 9 flag in DICR2 (bit 26).
        bus.iop_dmac.dicr = 1 << 24;
        bus.iop_dmac.dicr2 = 1 << 26;

        // Enable master channel interrupt in DICR (bit 23) while DMACINTEN.0 == 0 -> DICR.31 stays 0.
        bus.iop_write32(0xBF80_10F4, 1 << 23);
        assert_eq!(bus.iop_read32(0xBF80_10F4) & (1 << 31), 0);
        assert_eq!(bus.iop_read32(0xBF80_1070) & IopIrqFlags::DMA.bits(), 0);

        // Enable DMACINTEN bit 0 (0xBF80_157C = 1) -> DICR.31 becomes 1 and asserts IRQ3 in I_STAT (0xBF80_1070).
        bus.iop_write32(0xBF80_157C, 1);
        assert_ne!(bus.iop_read32(0xBF80_10F4) & (1 << 31), 0);
        assert_ne!(bus.iop_read32(0xBF80_1070) & IopIrqFlags::DMA.bits(), 0);

        // Write-1-to-clear channel 0 flag in DICR (bit 24); DICR.31 stays 1 because DICR2 bit 26 is still set.
        bus.iop_write32(0xBF80_10F4, (1 << 23) | (1 << 24));
        assert_eq!(bus.iop_read32(0xBF80_10F4) & (1 << 24), 0);
        assert_ne!(bus.iop_read32(0xBF80_10F4) & (1 << 31), 0);

        // Write-1-to-clear channel 9 flag in DICR2 (bit 26) -> DICR.31 clears to 0.
        bus.iop_write32(0xBF80_1574, 1 << 26);
        assert_eq!(bus.iop_read32(0xBF80_1574) & (1 << 26), 0);
        assert_eq!(bus.iop_read32(0xBF80_10F4) & (1 << 31), 0);
        assert_eq!(bus.pending_iop_mmio_error, None);
    }

    #[test]
    fn iop_dmac_channels_aux_and_dpcr3_semantics() {
        let mut bus = Bus::new();

        // DPCR3 (0x1F80_15F0) and auxiliary 4/9/A registers (0x1F80_1560..=0x1F80_1568).
        assert_eq!(bus.iop_read32(0xBF80_15F0), 0);
        bus.iop_write32(0xBF80_15F0, 0x0000_0777);
        assert_eq!(bus.iop_read32(0xBF80_15F0), 0x0000_0777);

        for (i, addr) in [0xBF80_1560u32, 0xBF80_1564, 0xBF80_1568]
            .into_iter()
            .enumerate()
        {
            assert_eq!(bus.iop_read32(addr), 0);
            let val = 0x1111_0000 | (i as u32);
            bus.iop_write32(addr, val);
            assert_eq!(bus.iop_read32(addr), val);
        }

        // Old channels 0..6 (0x1F80_1080..=0x1F80_10EC) and new channels 7..12 (0x1F80_1500..=0x1F80_155C).
        // Verify MADR and TADR mask to 24 bits (0x00FF_FFFF), BCR and CHCR store 32 bits, and CHCR bit 24 == 0 does not error.
        bus.iop_write32(0xBF80_10C0, 0xFF12_3456); // Ch 4 (SPU) MADR
        bus.iop_write32(0xBF80_10C4, 0x0010_0020); // Ch 4 (SPU) BCR
        bus.iop_write32(0xBF80_10C8, 0x0000_0601); // Ch 4 (SPU) CHCR (stopped)
        bus.iop_write32(0xBF80_10CC, 0xAB65_4320); // Ch 4 (SPU) TADR
        assert_eq!(bus.iop_read32(0xBF80_10C0), 0x0012_3456);
        assert_eq!(bus.iop_read32(0xBF80_10C4), 0x0010_0020);
        assert_eq!(bus.iop_read32(0xBF80_10C8), 0x0000_0601);
        assert_eq!(bus.iop_read32(0xBF80_10CC), 0x0065_4320);

        bus.iop_write32(0xBF80_1520, 0x9900_8000); // Ch 9 (SIF0) MADR
        bus.iop_write32(0xBF80_1524, 0x0001_0008); // Ch 9 (SIF0) BCR
        bus.iop_write32(0xBF80_1528, 0x0000_0701); // Ch 9 (SIF0) CHCR (stopped)
        bus.iop_write32(0xBF80_152C, 0xFE01_2340); // Ch 9 (SIF0) TADR
        assert_eq!(bus.iop_read32(0xBF80_1520), 0x0000_8000);
        assert_eq!(bus.iop_read32(0xBF80_1524), 0x0001_0008);
        assert_eq!(bus.iop_read32(0xBF80_1528), 0x0000_0701);
        assert_eq!(bus.iop_read32(0xBF80_152C), 0x0001_2340);
        assert_eq!(bus.pending_iop_mmio_error, None);
    }
}
