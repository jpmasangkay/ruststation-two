//! Emotion Engine 10-Channel DMA Controller (`DMAC`, `0x1000_8000..=0x1000_E060`, `0x1000_F520`, `0x1000_F590`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#dmac`, `#dmacchannels`, `#dmactags`, `#dmacio`):
//!   * 10 channels (`0: VIF0`, `1: VIF1`, `2: GIF`, `3: fromIPU`, `4: toIPU`, `5: SIF0`,
//!     `6: SIF1`, `7: SIF2`, `8: fromSPR`, `9: toSPR`).
//!   * Global control/status registers: `D_CTRL` (`0x1000_E000`), `D_STAT` (`0x1000_E010`),
//!     `D_PCR` (`0x1000_E020`), `D_SQWC` (`0x1000_E030`), `D_RBSR` (`0x1000_E040`),
//!     `D_RBOR` (`0x1000_E050`), `D_STADR` (`0x1000_E060`), `D_ENABLER` (`0x1000_F520`),
//!     and `D_ENABLEW` (`0x1000_F590`).
//!   * DMAC runs on `BUSCLK` (`147.456 MHz` = `1/2` EE clock), transferring one 128-bit quadword
//!     per `BUSCLK` cycle (`2` EE cycles per QWC).
//! - `ps2autotests` (`local/ps2autotests/tests/dma/`):
//!   * `dma/dmac/cpcond.{cpp,expected}`: `CPCOND0` (`BC0F`/`BC0T`) is `((!D_PCR | D_STAT) & 0x3FF) == 0x3FF`.
//!   * `dma/dmac/tagintr.{cpp,expected}`: Source-chain `REF`/`END` tag handling, `CHCR.TAG` (`bits 16..=31`),
//!     `TADR` advancement (`END` leaves `TADR` pointing at the `END` tag), and `CHCR.TIE` + `tag.IRQ` termination.
//!   * `dma/spr/normal.{cpp,expected}`: `fromSPR` (`D8`) and `toSPR` (`D9`) `SADR` masking (`& 0x3FF0`)
//!     and wraparound modulo 16 KB, and `MADR` clearing bit 31 (`& 0x7FFF_FFF0`).
//!   * `dma/spr/chain.{cpp,expected}`: `toSPR` Source Chain (`REFE` with `QWC == 0` transfers `0` quadwords)
//!     and `SADR` alignment masking (`& 0x3FF0`).
//!   * `dma/spr/interleave.{cpp,expected}`: `D_SQWC` 8-bit `SQWC`/`TQWC` masking (`& 0x00FF_00FF`),
//!     and interleaved slice transfers (`TQWC` quadwords transferred followed by `SQWC * 16` byte skip
//!     on `MADR` after every slice, including the final slice).

use crate::gif::GifUnit;
use crate::scheduler::{HardwareEventKind, Scheduler};
use crate::vif::VifUnit;

use super::{EE_RAM_SIZE, SCRATCHPAD_SIZE};

/// Number of architectural EE DMAC channels (`D0..=D9`).
pub const DMAC_CHANNEL_COUNT: usize = 10;

/// Maximum number of source/destination chain tags processed in a single transfer before stopping
/// to guard against malformed infinite self-referential tag loops.
const MAX_CHAIN_TAGS_PER_TRANSFER: usize = 65_536;

/// Channel indices (`ps2tek` `#dmacchannels`).
pub mod dmac_ch {
    pub const VIF0: usize = 0;
    pub const VIF1: usize = 1;
    pub const GIF: usize = 2;
    pub const FROM_IPU: usize = 3;
    pub const TO_IPU: usize = 4;
    pub const SIF0: usize = 5;
    pub const SIF1: usize = 6;
    pub const SIF2: usize = 7;
    pub const FROM_SPR: usize = 8;
    pub const TO_SPR: usize = 9;
}

/// `CHCR` bit masks (`ps2tek` `#dmacchannels`).
pub mod chcr_bits {
    /// Bit 0: Direction (`0` = to memory, `1` = from memory; fixed in hardware for most channels).
    pub const DIR: u32 = 1 << 0;
    /// Bits `2..=3`: Transfer mode (`0` = Normal, `1` = Chain, `2` = Interleave).
    pub const MOD_MASK: u32 = 0b11 << 2;
    ///Bits `4..=5`: Address stack pointer (`0..=2` for `ASR0`/`ASR1`).
    pub const ASP_MASK: u32 = 0b11 << 4;
    /// Bit 6: Tag Transfer Enable (`TTE`).
    pub const TTE: u32 = 1 << 6;
    /// Bit 7: Tag Interrupt Enable (`TIE`).
    pub const TIE: u32 = 1 << 7;
    /// Bit 8: Start / Busy (`STR`).
    pub const STR: u32 = 1 << 8;
    /// Bits `16..=31`: Upper 16 bits of most recently read DMATag (`TAG`).
    pub const TAG_MASK: u32 = 0xFFFF_0000;
    /// Mask of architectural writable/readable `CHCR` bits (`0xFFFF_01FD`: bit 1 and bits `9..=15` are `0`).
    pub const WRITABLE_MASK: u32 = 0xFFFF_01FD;
}

/// `D_CTRL` (`0x1000_E000`) bit masks (`ps2tek` `#dmacio`).
pub mod d_ctrl_bits {
    /// Bit 0: Master DMA Enable (`DMAE`).
    pub const DMAE: u32 = 1 << 0;
    /// Bit 1: Cycle Stealing / Release Enable (`RELE`).
    pub const RELE: u32 = 1 << 1;
    /// Bits `2..=3`: Memory FIFO Drain Channel (`0..1` = None, `2` = VIF1, `3` = GIF).
    pub const MFD_MASK: u32 = 0b11 << 2;
    /// Bits `4..=5`: Stall Control Source Channel (`0` = None, `1` = SIF0, `2` = fromSPR, `3` = fromIPU).
    pub const STS_MASK: u32 = 0b11 << 4;
    /// Bits `6..=7`: Stall Control Drain Channel (`0` = None, `1` = VIF1, `2` = GIF, `3` = SIF1).
    pub const STD_MASK: u32 = 0b11 << 6;
}

/// `D_STAT` (`0x1000_E010`) bit masks (`ps2tek` `#dmacio`).
pub mod d_stat_bits {
    /// Bits `0..=9`: Channel Interrupt Status (`CIS0..CIS9`, Write-1-to-Clear).
    pub const CIS_MASK: u32 = 0x03FF;
    /// Bit 13: DMA Stall Interrupt Status (`SIS`, Write-1-to-Clear).
    pub const SIS: u32 = 1 << 13;
    /// Bit 14: MFIFO Empty Interrupt Status (`MEIS`, Write-1-to-Clear).
    pub const MEIS: u32 = 1 << 14;
    /// Bit 15: Bus Error Interrupt Status (`BEIS`, Write-1-to-Clear).
    pub const BEIS: u32 = 1 << 15;
    /// Mask of all Write-1-to-Clear status bits in the lower 16 bits (`CIS0..9 | SIS | MEIS | BEIS`).
    pub const STATUS_W1C_MASK: u32 = 0xE3FF;
    /// Bits `16..=25`: Channel Interrupt Mask (`CIM0..CIM9`, Write-1-to-Toggle).
    pub const CIM_MASK: u32 = 0x03FF << 16;
    /// Bit 29: DMA Stall Interrupt Mask (`SIM`, Write-1-to-Toggle).
    pub const SIM: u32 = 1 << 29;
    /// Bit 30: MFIFO Empty Interrupt Mask (`MEIM`, Write-1-to-Toggle).
    pub const MEIM: u32 = 1 << 30;
    /// Mask of all Write-1-to-Toggle mask bits in the upper 16 bits (`CIM0..9 | SIM | MEIM`).
    pub const MASK_W1T_MASK: u32 = 0x63FF_0000;
}

/// Details of an attempted DMA transfer on a peripheral channel (`0..=7`) that is not yet wired in Task 5.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UnimplementedDmaTransfer {
    /// DMAC channel index (`0..=7`).
    pub channel: u8,
    /// Human-readable channel name (`"VIF0"`, `"VIF1"`, `"GIF"`, `"fromIPU"`, `"toIPU"`, `"SIF0"`, `"SIF1"`, `"SIF2"`).
    pub channel_name: &'static str,
    /// `CHCR` value written to start the transfer.
    pub chcr: u32,
    /// `MADR` value at transfer start.
    pub madr: u32,
    /// `QWC` value at transfer start.
    pub qwc: u32,
    /// `TADR` value at transfer start.
    pub tadr: u32,
}

/// Per-channel EE DMAC register set (`CHCR`, `MADR`, `QWC`, `TADR`, `ASR0`, `ASR1`, `SADR`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DmaChannel {
    /// `+0x00`: Channel Control Register (`CHCR`).
    pub chcr: u32,
    /// `+0x10`: Memory Address Register (`MADR`, 16-byte aligned).
    pub madr: u32,
    /// `+0x20`: Quadword Count Register (`QWC`, 16-bit).
    pub qwc: u32,
    /// `+0x30`: Tag Address Register (`TADR`, 16-byte aligned).
    pub tadr: u32,
    /// `+0x40`: Address Stack Register 0 (`ASR0`, 16-byte aligned).
    pub asr0: u32,
    /// `+0x50`: Address Stack Register 1 (`ASR1`, 16-byte aligned).
    pub asr1: u32,
    /// `+0x80`: Scratchpad Address Register (`SADR`, 16-byte aligned within 16 KB: `& 0x3FF0`).
    pub sadr: u32,
}

impl DmaChannel {
    /// Human-readable hardware name for channel `index` (`0..=9`).
    #[inline]
    pub const fn name(index: usize) -> &'static str {
        match index {
            0 => "VIF0",
            1 => "VIF1",
            2 => "GIF",
            3 => "fromIPU",
            4 => "toIPU",
            5 => "SIF0",
            6 => "SIF1",
            7 => "SIF2",
            8 => "fromSPR",
            9 => "toSPR",
            _ => "UNKNOWN",
        }
    }

    /// Transfer mode field `CHCR.MOD` (`bits 2..=3`: `0` = Normal, `1` = Chain, `2` = Interleave).
    #[inline(always)]
    pub const fn mode(&self) -> u32 {
        (self.chcr & chcr_bits::MOD_MASK) >> 2
    }

    /// Call stack pointer `CHCR.ASP` (`bits 4..=5`: `0..=2`).
    #[inline(always)]
    pub const fn asp(&self) -> u32 {
        (self.chcr & chcr_bits::ASP_MASK) >> 4
    }

    /// Set `CHCR.ASP` (`bits 4..=5`).
    #[inline(always)]
    pub fn set_asp(&mut self, asp: u32) {
        self.chcr = (self.chcr & !chcr_bits::ASP_MASK) | ((asp & 0b11) << 4);
    }

    /// Tag Interrupt Enable flag `CHCR.TIE` (bit 7).
    #[inline(always)]
    pub const fn tie(&self) -> bool {
        (self.chcr & chcr_bits::TIE) != 0
    }

    /// Tag Transfer Enable flag `CHCR.TTE` (bit 6).
    #[inline(always)]
    pub const fn tte(&self) -> bool {
        (self.chcr & chcr_bits::TTE) != 0
    }
}

/// Per-channel state for scheduler-driven incremental FIFO DMA transfers (`VIF0`, `VIF1`, `GIF`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum FifoDmaPhase {
    #[default]
    Idle,
    /// Source Chain (`MOD == 1`): ready to fetch next DMAtag from `TADR`.
    ReadTag,
    /// Source Chain (`MOD == 1`, `CHCR.TTE == 1`): waiting to send tag bits `64..=127` to FIFO.
    SendTte { lo: u32, hi: u32 },
    /// Streaming `ch.qwc` 128-bit quadwords from `ch.madr` (RDRAM or Scratchpad via bit 31).
    TransferData,
}

/// Resumable DMAC transfer cursor for `D0` (`VIF0`), `D1` (`VIF1`), and `D2` (`GIF`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct FifoDmaState {
    phase: FifoDmaPhase,
    chain_end: bool,
}

/// Static-dispatch adapter for scheduler-driven 128-bit peripheral FIFO targets (`VIF0`, `VIF1`, `GIF`).
trait DmaFifoSink {
    fn supports_tte(&self) -> bool;
    fn can_accept_qwc(&self) -> bool;
    fn push_dma_qwc(&mut self, qwc: ps2_common::Reg128);
    fn can_accept_tte(&self) -> bool;
    fn push_dma_tte(&mut self, lo: u32, hi: u32);
}

impl DmaFifoSink for VifUnit {
    #[inline(always)]
    fn supports_tte(&self) -> bool {
        true
    }

    #[inline(always)]
    fn can_accept_qwc(&self) -> bool {
        VifUnit::can_accept_qwc(self)
    }

    #[inline(always)]
    fn push_dma_qwc(&mut self, qwc: ps2_common::Reg128) {
        VifUnit::push_dma_qwc(self, qwc);
    }

    #[inline(always)]
    fn can_accept_tte(&self) -> bool {
        VifUnit::can_accept_tte(self)
    }

    #[inline(always)]
    fn push_dma_tte(&mut self, lo: u32, hi: u32) {
        VifUnit::push_dma_tte(self, lo, hi);
    }
}

impl DmaFifoSink for GifUnit {
    #[inline(always)]
    fn supports_tte(&self) -> bool {
        false
    }

    #[inline(always)]
    fn can_accept_qwc(&self) -> bool {
        GifUnit::can_accept_path3_qwc(self)
    }

    #[inline(always)]
    fn push_dma_qwc(&mut self, qwc: ps2_common::Reg128) {
        let _ = GifUnit::push_path3_qwc(self, qwc);
    }

    #[inline(always)]
    fn can_accept_tte(&self) -> bool {
        false
    }

    #[inline(always)]
    fn push_dma_tte(&mut self, _lo: u32, _hi: u32) {}
}

/// Outcome of advancing one Source Chain tag via [`EeDmac::step_source_chain_tag`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SourceChainTagStep {
    /// `REFS` stall (`D_CTRL.STD` check failed against `D_STADR`; `D_STAT.SIS` asserted).
    Stalled,
    /// DMAtag parsed and channel `CHCR.TAG` / `QWC` / `MADR` / `TADR` / `ASR` updated.
    Advanced {
        tte_lo: u32,
        tte_hi: u32,
        qwc: u32,
        end_chain: bool,
    },
}

/// Emotion Engine 10-Channel DMA Controller (`DMAC`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EeDmac {
    /// Per-channel register banks (`D0..=D9`).
    pub channels: [DmaChannel; DMAC_CHANNEL_COUNT],
    /// `0x1000_E000` (`D_CTRL`): DMAC Control Register (initialized to `D_CTRL_DMAE = 1`).
    pub d_ctrl: u32,
    /// `0x1000_E010` (`D_STAT`): DMAC Interrupt Status & Mask Register.
    pub d_stat: u32,
    /// `0x1000_E020` (`D_PCR`): DMAC Priority & `CPCOND0` Control Register.
    pub d_pcr: u32,
    /// `0x1000_E030` (`D_SQWC`): Interleave Skip (`bits 0..=7`) & Transfer (`bits 16..=23`) Quadword Counts.
    pub d_sqwc: u32,
    /// `0x1000_E040` (`D_RBSR`): MFIFO Ringbuffer Size Register (`16`-byte aligned mask).
    // TODO(verify): MFIFO ringbuffer address wrap logic when VIF1/GIF drain channels are implemented in Phase 2.
    pub d_rbsr: u32,
    /// `0x1000_E050` (`D_RBOR`): MFIFO Ringbuffer Offset Register (`16`-byte aligned base).
    pub d_rbor: u32,
    /// `0x1000_E060` (`D_STADR`): Stall Address Register (`16`-byte aligned).
    pub d_stadr: u32,
    /// `0x1000_F500`: Undocumented EE DMAC/bus management configuration latch written with `0x1` during early BIOS reset.
    // TODO(verify): `ps2tek` (`#dmacio`) documents `0x1000_F520` (`D_ENABLER`) and `0x1000_F590` (`D_ENABLEW`)
    // in the `0x1000_F5xx` block, but omits `0x1000_F500`; verify exact bit semantics on silicon.
    pub d_f500: u32,
    /// `0x1000_F520` (`D_ENABLER`) / `0x1000_F590` (`D_ENABLEW`): Master DMA hold/suspend register
    /// (initialized to `0x1201` per `ps2tek` `https://psi-rockin.github.io/ps2tek/#dmacio`).
    pub d_enable: u32,
    /// Pending unimplemented peripheral DMA transfer error (channels `0..=7`), if triggered by guest code.
    pub unimplemented_transfer: Option<UnimplementedDmaTransfer>,
    /// Incremental BUSCLK state for `D0` (`VIF0`), `D1` (`VIF1`), and `D2` (`GIF`).
    fifo_dma: [FifoDmaState; 3],
}

impl Default for EeDmac {
    fn default() -> Self {
        Self::new()
    }
}

impl EeDmac {
    /// Create a newly initialized [`EeDmac`].
    pub const fn new() -> Self {
        Self {
            channels: [DmaChannel {
                chcr: 0,
                madr: 0,
                qwc: 0,
                tadr: 0,
                asr0: 0,
                asr1: 0,
                sadr: 0,
            }; DMAC_CHANNEL_COUNT],
            d_ctrl: d_ctrl_bits::DMAE,
            d_stat: 0,
            d_pcr: 0,
            d_sqwc: 0,
            d_rbsr: 0,
            d_rbor: 0,
            d_stadr: 0,
            d_f500: 0,
            d_enable: 0x1201,
            unimplemented_transfer: None,
            fifo_dma: [FifoDmaState {
                phase: FifoDmaPhase::Idle,
                chain_end: false,
            }; 3],
        }
    }

    /// Returns `true` if the DMAC `INT1` interrupt line (`COP0.Cause.IP3`, bit 11) is currently asserted.
    ///
    /// Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#dmacio`):
    /// `INT1` is asserted when any status bit in `D_STAT[14:0]` (`CIS0..9`, `SIS`, `MEIS`) and its
    /// corresponding mask bit in `D_STAT[30:16]` (`CIM0..9`, `SIM`, `MEIM`) are both `1`, or when
    /// `BEIS` (bit 15) is `1`.
    #[inline(always)]
    pub const fn int1_pending(&self) -> bool {
        let status = self.d_stat & 0x7FFF;
        let mask = (self.d_stat >> 16) & 0x7FFF;
        (status & mask) != 0 || (self.d_stat & d_stat_bits::BEIS) != 0
    }

    /// Evaluate the `CPCOND0` condition signal tested by `BC0F` / `BC0T` / `BC0FL` / `BC0TL`.
    ///
    /// Primary source references:
    /// - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#dmacio`):
    ///   `D_PCR` bits `0..=9` (`CPC0..CPC9`) enable channel completion status checks for `CPCOND0`.
    /// - `ps2autotests` (`local/ps2autotests/tests/dma/dmac/cpcond.cpp` & `cpcond.expected` lines 2..7):
    ///   `CPCOND0` is `1` (`true`) when every channel enabled in `D_PCR.CPC0..9` has its corresponding
    ///   `D_STAT.CIS0..9` bit set (`((!d_pcr | d_stat) & 0x3FF) == 0x3FF`).
    #[inline(always)]
    pub const fn cpcond0(&self) -> bool {
        ((!self.d_pcr | self.d_stat) & d_stat_bits::CIS_MASK) == d_stat_bits::CIS_MASK
    }

    /// Complete a scheduled DMA transfer on `channel` (`0..=9`), clearing `CHCR.STR` and setting `D_STAT.CIS[channel]`.
    #[inline]
    pub fn complete_channel(&mut self, channel: usize) {
        if let Some(ch) = self.channels.get_mut(channel) {
            ch.chcr &= !chcr_bits::STR;
            self.d_stat |= 1u32 << (channel as u32);
        }
    }

    /// Map a physical MMIO address in `0x1000_8000..=0x1000_DFFF` to `(channel_index, reg_offset)`.
    #[inline]
    pub const fn decode_channel_addr(paddr: u32) -> Option<(usize, u32)> {
        let base = paddr & 0xFFFF_FC00;
        let offset = paddr & 0x3FF;
        if offset > 0xFF {
            return None;
        }
        let ch = match base {
            0x1000_8000 => Some(dmac_ch::VIF0),
            0x1000_9000 => Some(dmac_ch::VIF1),
            0x1000_A000 => Some(dmac_ch::GIF),
            0x1000_B000 => Some(dmac_ch::FROM_IPU),
            0x1000_B400 => Some(dmac_ch::TO_IPU),
            0x1000_C000 => Some(dmac_ch::SIF0),
            0x1000_C400 => Some(dmac_ch::SIF1),
            0x1000_C800 => Some(dmac_ch::SIF2),
            0x1000_D000 => Some(dmac_ch::FROM_SPR),
            0x1000_D400 => Some(dmac_ch::TO_SPR),
            _ => None,
        };
        match ch {
            Some(idx) => Some((idx, offset)),
            None => None,
        }
    }

    /// Read a 32-bit DMAC register at `paddr` (`0x1000_8000..=0x1000_E060` or `0x1000_F520` / `0x1000_F590`).
    pub fn read32(&self, paddr: u32) -> u32 {
        match paddr {
            0x1000_8000..=0x1000_DFFF => {
                if let Some((ch_idx, offset)) = Self::decode_channel_addr(paddr) {
                    let ch = &self.channels[ch_idx];
                    match offset {
                        0x00 => ch.chcr,
                        0x10 => ch.madr,
                        0x20 => ch.qwc & 0xFFFF,
                        0x30 => ch.tadr,
                        0x40 => ch.asr0,
                        0x50 => ch.asr1,
                        0x80 => ch.sadr,
                        _ => 0,
                    }
                } else {
                    0
                }
            }
            0x1000_E000 => self.d_ctrl,
            0x1000_E010 => self.d_stat,
            0x1000_E020 => self.d_pcr,
            0x1000_E030 => self.d_sqwc,
            0x1000_E040 => self.d_rbsr,
            0x1000_E050 => self.d_rbor,
            0x1000_E060 => self.d_stadr,
            0x1000_F500 => self.d_f500,
            0x1000_F520 | 0x1000_F590 => self.d_enable,
            _ => 0,
        }
    }

    /// Write a 32-bit DMAC register at `paddr` (`0x1000_8000..=0x1000_E060` or `0x1000_F500` / `0x1000_F590`),
    /// starting and scheduling any triggered DMA transfer.
    pub fn write32(
        &mut self,
        paddr: u32,
        val: u32,
        ram: &mut [u8],
        scratchpad: &mut [u8],
        scheduler: &mut Scheduler,
    ) {
        match paddr {
            0x1000_8000..=0x1000_DFFF => {
                if let Some((ch_idx, offset)) = Self::decode_channel_addr(paddr) {
                    self.write_channel_reg(ch_idx, offset, val, ram, scratchpad, scheduler);
                }
            }
            0x1000_E000 => {
                self.d_ctrl = val;
            }
            0x1000_E010 => {
                // Low 16 bits (`0xE3FF`) are Write-1-to-Clear; high 16 bits (`0x63FF_0000`) are Write-1-to-Toggle
                // (`ps2tek` `https://psi-rockin.github.io/ps2tek/#dmacio` & `dma/dmac/cpcond.cpp` line 62).
                let clear_bits = val & d_stat_bits::STATUS_W1C_MASK;
                let toggle_bits = val & d_stat_bits::MASK_W1T_MASK;
                self.d_stat = (self.d_stat & !clear_bits) ^ toggle_bits;
            }
            0x1000_E020 => {
                self.d_pcr = val;
            }
            0x1000_E030 => {
                // `SQWC` (`bits 0..=7`) and `TQWC` (`bits 16..=23`) are 8-bit fields
                // (`ps2autotests` `dma/spr/interleave.expected` lines 240, 259, 299).
                self.d_sqwc = val & 0x00FF_00FF;
            }
            0x1000_E040 => {
                self.d_rbsr = val & !0xF;
            }
            0x1000_E050 => {
                self.d_rbor = val & !0xF;
            }
            0x1000_E060 => {
                self.d_stadr = val & !0xF;
            }
            0x1000_F500 => {
                self.d_f500 = val;
            }
            0x1000_F520 => {
                // `D_ENABLER` is read-only (`ps2tek` `#dmacio`).
            }
            0x1000_F590 => {
                self.d_enable = val;
            }
            _ => {}
        }
    }

    fn write_channel_reg(
        &mut self,
        ch_idx: usize,
        offset: u32,
        val: u32,
        ram: &mut [u8],
        scratchpad: &mut [u8],
        scheduler: &mut Scheduler,
    ) {
        match offset {
            0x00 => {
                let ch = &mut self.channels[ch_idx];
                ch.chcr = val & chcr_bits::WRITABLE_MASK;
                if (ch.chcr & chcr_bits::STR) != 0 {
                    self.start_channel_transfer(ch_idx, ram, scratchpad, scheduler);
                }
            }
            0x10 => {
                // Channels 8 (`fromSPR`) and 9 (`toSPR`) ignore bit 31 (`0x8000_0000`, the SPR selection bit)
                // as well as low bits `0..=3` (`dma/spr/normal.expected` lines 21..27).
                let mask = if ch_idx == dmac_ch::FROM_SPR || ch_idx == dmac_ch::TO_SPR {
                    0x7FFF_FFF0
                } else {
                    !0xF
                };
                self.channels[ch_idx].madr = val & mask;
            }
            0x20 => {
                self.channels[ch_idx].qwc = val & 0xFFFF;
            }
            0x30 => {
                self.channels[ch_idx].tadr = val & !0xF;
            }
            0x40 => {
                self.channels[ch_idx].asr0 = val & !0xF;
            }
            0x50 => {
                self.channels[ch_idx].asr1 = val & !0xF;
            }
            0x80 => {
                // `SADR` is a 14-bit quadword-aligned offset within the 16 KB Scratchpad (`& 0x3FF0`)
                // (`dma/spr/normal.expected` lines 3..19 & `dma/spr/chain.expected` lines 3..19).
                self.channels[ch_idx].sadr = val & 0x3FF0;
            }
            _ => {}
        }
    }

    /// Return `true` when EE DMAC master enable is active (`D_CTRL.DMAE == 1` and `D_ENABLE.CPND == 0`).
    #[inline(always)]
    pub const fn dma_enabled(&self) -> bool {
        (self.d_ctrl & d_ctrl_bits::DMAE) != 0 && (self.d_enable & (1 << 16)) == 0
    }

    /// Return `true` when channel `ch_idx` currently has `CHCR.STR == 1`.
    #[inline(always)]
    pub const fn is_channel_active(&self, ch_idx: usize) -> bool {
        ch_idx < DMAC_CHANNEL_COUNT && (self.channels[ch_idx].chcr & chcr_bits::STR) != 0
    }

    /// Return `true` when any FIFO channel (`VIF0`, `VIF1`, or `GIF`) currently has `CHCR.STR == 1`.
    #[inline(always)]
    pub const fn has_active_fifo_transfer(&self) -> bool {
        ((self.channels[dmac_ch::VIF0].chcr
            | self.channels[dmac_ch::VIF1].chcr
            | self.channels[dmac_ch::GIF].chcr)
            & chcr_bits::STR)
            != 0
    }

    #[inline]
    fn fifo_channel_can_step_inner<S: DmaFifoSink>(&self, ch_idx: usize, sink: &S) -> bool {
        let ch = &self.channels[ch_idx];
        match self.fifo_dma[ch_idx].phase {
            FifoDmaPhase::Idle => false,
            FifoDmaPhase::ReadTag => true,
            FifoDmaPhase::SendTte { .. } => sink.supports_tte() && sink.can_accept_tte(),
            FifoDmaPhase::TransferData => ch.qwc == 0 || sink.can_accept_qwc(),
        }
    }

    /// Return `true` if `ch_idx` (`VIF0` or `VIF1`) is active and not currently blocked on VIF FIFO / `REFS`.
    pub fn vif_channel_can_step(&self, ch_idx: usize, vif: &VifUnit) -> bool {
        if !self.dma_enabled() || ch_idx > dmac_ch::VIF1 {
            return false;
        }
        let ch = &self.channels[ch_idx];
        if (ch.chcr & chcr_bits::STR) == 0 {
            return false;
        }
        if ch_idx == dmac_ch::VIF1 && vif.fdr && (ch.chcr & chcr_bits::DIR) == 0 {
            return false;
        }
        self.fifo_channel_can_step_inner(ch_idx, vif)
    }

    /// Return `true` if `D2` (`GIF` `PATH3`) is active and can transfer a beat to the `PATH3` FIFO.
    pub fn gif_channel_can_step(&self, gif: &GifUnit) -> bool {
        if !self.dma_enabled() {
            return false;
        }
        let ch = &self.channels[dmac_ch::GIF];
        if (ch.chcr & chcr_bits::STR) == 0 {
            return false;
        }
        if (ch.chcr & chcr_bits::DIR) == 0 || ch.mode() > 1 || ch.tte() {
            return false;
        }
        self.fifo_channel_can_step_inner(dmac_ch::GIF, gif)
    }

    /// Advance `ch_idx` (`dmac_ch::VIF0` or `dmac_ch::VIF1`) by at most one BUSCLK beat (`1` tag or `1` QWC).
    ///
    /// Returns `true` if the channel advanced or completed (so another BUSCLK tick may be needed),
    /// or `false` if blocked (DMAC disabled/suspended, VIF FIFO full, or `REFS` stall).
    pub fn step_vif_channel(
        &mut self,
        ch_idx: usize,
        vif: &mut VifUnit,
        ram: &[u8],
        scratchpad: &[u8],
    ) -> bool {
        if !self.dma_enabled() || ch_idx > dmac_ch::VIF1 {
            return false;
        }
        if (self.channels[ch_idx].chcr & chcr_bits::STR) == 0 {
            self.fifo_dma[ch_idx] = FifoDmaState::default();
            return false;
        }
        if ch_idx == dmac_ch::VIF1 && vif.fdr && (self.channels[ch_idx].chcr & chcr_bits::DIR) == 0
        {
            return false;
        }
        self.step_fifo_channel_inner(ch_idx, vif, ram, scratchpad)
    }

    /// Advance `D2` (`GIF` `PATH3`) by at most one BUSCLK beat (`1` source-chain tag or `1` QWC).
    pub fn step_gif_channel(&mut self, gif: &mut GifUnit, ram: &[u8], scratchpad: &[u8]) -> bool {
        if !self.dma_enabled() {
            return false;
        }
        let ch = &self.channels[dmac_ch::GIF];
        if (ch.chcr & chcr_bits::STR) == 0 {
            self.fifo_dma[dmac_ch::GIF] = FifoDmaState::default();
            return false;
        }
        if (ch.chcr & chcr_bits::DIR) == 0 || ch.mode() > 1 || ch.tte() {
            return false;
        }
        gif.on_path3_dma_start();
        let progressed = self.step_fifo_channel_inner(dmac_ch::GIF, gif, ram, scratchpad);
        if (self.channels[dmac_ch::GIF].chcr & chcr_bits::STR) == 0 {
            gif.on_path3_dma_end();
        }
        progressed
    }

    fn step_fifo_channel_inner<S: DmaFifoSink>(
        &mut self,
        ch_idx: usize,
        sink: &mut S,
        ram: &[u8],
        scratchpad: &[u8],
    ) -> bool {
        // Recover phase if channel was already busy (`STR == 1`) before `start_channel_transfer` ran.
        if self.fifo_dma[ch_idx].phase == FifoDmaPhase::Idle {
            let is_chain = self.channels[ch_idx].mode() == 1;
            self.fifo_dma[ch_idx] = FifoDmaState {
                phase: if is_chain {
                    FifoDmaPhase::ReadTag
                } else {
                    FifoDmaPhase::TransferData
                },
                chain_end: !is_chain,
            };
        }

        match self.fifo_dma[ch_idx].phase {
            FifoDmaPhase::Idle => false,
            FifoDmaPhase::ReadTag => {
                match self.step_source_chain_tag(ch_idx, true, ram, scratchpad) {
                    SourceChainTagStep::Stalled => false,
                    SourceChainTagStep::Advanced {
                        tte_lo,
                        tte_hi,
                        qwc,
                        end_chain,
                    } => {
                        self.fifo_dma[ch_idx].chain_end = end_chain;
                        if sink.supports_tte() && self.channels[ch_idx].tte() {
                            if sink.can_accept_tte() {
                                sink.push_dma_tte(tte_lo, tte_hi);
                                self.finish_or_advance_after_tag(ch_idx, qwc, end_chain);
                            } else {
                                self.fifo_dma[ch_idx].phase = FifoDmaPhase::SendTte {
                                    lo: tte_lo,
                                    hi: tte_hi,
                                };
                            }
                        } else {
                            self.finish_or_advance_after_tag(ch_idx, qwc, end_chain);
                        }
                        true
                    }
                }
            }
            FifoDmaPhase::SendTte { lo, hi } => {
                if !sink.supports_tte() || !sink.can_accept_tte() {
                    return false;
                }
                sink.push_dma_tte(lo, hi);
                let qwc = self.channels[ch_idx].qwc;
                let end_chain = self.fifo_dma[ch_idx].chain_end;
                self.finish_or_advance_after_tag(ch_idx, qwc, end_chain);
                true
            }
            FifoDmaPhase::TransferData => {
                let qwc_left = self.channels[ch_idx].qwc;
                if qwc_left == 0 {
                    if self.fifo_dma[ch_idx].chain_end {
                        self.complete_channel(ch_idx);
                        self.fifo_dma[ch_idx] = FifoDmaState::default();
                    } else {
                        self.fifo_dma[ch_idx].phase = FifoDmaPhase::ReadTag;
                    }
                    return true;
                }
                if !sink.can_accept_qwc() {
                    return false;
                }
                let src_addr = self.channels[ch_idx].madr;
                let qwc_bytes = read_source_qwc(ram, scratchpad, src_addr, true);
                let qwc = ps2_common::Reg128::from_u128(u128::from_le_bytes(qwc_bytes));
                sink.push_dma_qwc(qwc);

                let ch = &mut self.channels[ch_idx];
                // Preserve bit 31 Scratchpad selector across sequential QWC increments.
                let spr_bit = src_addr & 0x8000_0000;
                ch.madr = spr_bit | (src_addr.wrapping_add(16) & 0x7FFF_FFF0);
                ch.qwc = qwc_left - 1;
                if ch.qwc == 0 {
                    if self.fifo_dma[ch_idx].chain_end {
                        self.complete_channel(ch_idx);
                        self.fifo_dma[ch_idx] = FifoDmaState::default();
                    } else {
                        self.fifo_dma[ch_idx].phase = FifoDmaPhase::ReadTag;
                    }
                }
                true
            }
        }
    }

    #[inline]
    fn finish_or_advance_after_tag(&mut self, ch_idx: usize, qwc: u32, end_chain: bool) {
        if qwc > 0 {
            self.fifo_dma[ch_idx].phase = FifoDmaPhase::TransferData;
        } else if end_chain {
            self.complete_channel(ch_idx);
            self.fifo_dma[ch_idx] = FifoDmaState::default();
        } else {
            self.fifo_dma[ch_idx].phase = FifoDmaPhase::ReadTag;
        }
    }

    fn start_channel_transfer(
        &mut self,
        ch_idx: usize,
        ram: &mut [u8],
        scratchpad: &mut [u8],
        scheduler: &mut Scheduler,
    ) {
        if ch_idx <= dmac_ch::GIF {
            let is_chain = self.channels[ch_idx].mode() == 1;
            self.fifo_dma[ch_idx] = FifoDmaState {
                phase: if is_chain {
                    FifoDmaPhase::ReadTag
                } else {
                    FifoDmaPhase::TransferData
                },
                chain_end: !is_chain,
            };
        }

        // Check master DMA enable (`D_CTRL.DMAE`) and master suspend (`D_ENABLE.CPND` bit 16).
        if !self.dma_enabled() {
            return;
        }

        // Cancel any previously scheduled completion event for this channel.
        scheduler.cancel_by_kind(HardwareEventKind::DmacChannelComplete(ch_idx as u8));

        match ch_idx {
            dmac_ch::VIF0 | dmac_ch::VIF1 => {
                // Scheduler-driven 1 QWC per BUSCLK (2 EE cycles): transfer runs incrementally
                // in `step_vif_channel` and honors VIF FIFO backpressure mid-packet.
                scheduler.cancel_by_kind(HardwareEventKind::DmacTick);
                scheduler.schedule(2, HardwareEventKind::DmacTick);
            }
            dmac_ch::GIF => {
                let ch = &self.channels[ch_idx];
                // TODO(verify): `ps2tek` is silent on GIF->EE VRAM DMA readback (`DIR == 0`),
                // `Interleave` mode on `D2`, and `CHCR.TTE` 64-to-128 expansion on `GIF`.
                if (ch.chcr & chcr_bits::DIR) == 0 || ch.mode() > 1 || ch.tte() {
                    self.fifo_dma[ch_idx] = FifoDmaState::default();
                    self.unimplemented_transfer = Some(UnimplementedDmaTransfer {
                        channel: ch_idx as u8,
                        channel_name: DmaChannel::name(ch_idx),
                        chcr: ch.chcr,
                        madr: ch.madr,
                        qwc: ch.qwc,
                        tadr: ch.tadr,
                    });
                    return;
                }
                scheduler.cancel_by_kind(HardwareEventKind::DmacTick);
                scheduler.schedule(2, HardwareEventKind::DmacTick);
            }
            dmac_ch::FROM_SPR => {
                let (bus_cycles, stalled) = self.execute_from_spr(ram, scratchpad);
                if !stalled {
                    // Schedule completion at 2 EE cycles per BUSCLK (`ps2tek` `#dmac`).
                    let ee_cycles = u64::from(bus_cycles.max(1)) * 2;
                    scheduler.schedule(
                        ee_cycles,
                        HardwareEventKind::DmacChannelComplete(ch_idx as u8),
                    );
                }
            }
            dmac_ch::TO_SPR => {
                let (bus_cycles, stalled) = self.execute_to_spr(ram, scratchpad);
                if !stalled {
                    let ee_cycles = u64::from(bus_cycles.max(1)) * 2;
                    scheduler.schedule(
                        ee_cycles,
                        HardwareEventKind::DmacChannelComplete(ch_idx as u8),
                    );
                }
            }
            _ => {
                // Channels 3..=7 (`fromIPU`, `toIPU`, `SIF0`, `SIF1`, `SIF2`)
                // are not yet wired in Step 4; record an explicit unimplemented-transfer error.
                let ch = &self.channels[ch_idx];
                self.unimplemented_transfer = Some(UnimplementedDmaTransfer {
                    channel: ch_idx as u8,
                    channel_name: DmaChannel::name(ch_idx),
                    chcr: ch.chcr,
                    madr: ch.madr,
                    qwc: ch.qwc,
                    tadr: ch.tadr,
                });
            }
        }
    }

    /// Execute `D8` (`fromSPR`: Scratchpad -> RDRAM) in Normal (`0`), Destination Chain (`1`),
    /// or Interleave (`2`) mode. Returns `(bus_cycles, stalled)`.
    fn execute_from_spr(&mut self, ram: &mut [u8], scratchpad: &mut [u8]) -> (u32, bool) {
        let mode = self.channels[dmac_ch::FROM_SPR].mode();
        let update_stadr = ((self.d_ctrl & d_ctrl_bits::STS_MASK) >> 4) == 2;

        match mode {
            // Normal mode (`MOD == 0`)
            0 => {
                let count = self.channels[dmac_ch::FROM_SPR].qwc & 0xFFFF;
                self.transfer_slice_from_spr(count, ram, scratchpad, update_stadr);
                (count.max(1), false)
            }
            // Destination Chain mode (`MOD == 1`, `ps2tek` `#dmactags`)
            1 => {
                let mut total_bus_cycles = 0u32;
                for _ in 0..MAX_CHAIN_TAGS_PER_TRANSFER {
                    let sadr = (self.channels[dmac_ch::FROM_SPR].sadr & 0x3FF0) as usize;
                    let tag_lo = read_le_u32(scratchpad, sadr);
                    let tag_addr = read_le_u32(scratchpad, sadr + 4);
                    self.channels[dmac_ch::FROM_SPR].sadr =
                        (self.channels[dmac_ch::FROM_SPR].sadr.wrapping_add(16)) & 0x3FF0;

                    let qwc = tag_lo & 0xFFFF;
                    let tag_id = (tag_lo >> 28) & 0x7;
                    let tag_irq = (tag_lo & (1 << 31)) != 0;

                    let ch = &mut self.channels[dmac_ch::FROM_SPR];
                    ch.chcr = (ch.chcr & !chcr_bits::TAG_MASK) | (tag_lo & chcr_bits::TAG_MASK);
                    ch.qwc = qwc;
                    ch.madr = tag_addr & 0x7FFF_FFF0;

                    // Tag IDs on `fromSPR` (`ps2tek` `#dmactags`):
                    // 0 = CNTS (transfers QWC quadwords and updates D_STADR if STS == fromSPR)
                    // 1 = CNT  (transfers QWC quadwords)
                    // 7 = END  (transfers QWC quadwords and finishes chain)
                    let is_cnts = tag_id == 0;
                    self.transfer_slice_from_spr(qwc, ram, scratchpad, update_stadr && is_cnts);
                    total_bus_cycles = total_bus_cycles.saturating_add(1 + qwc);

                    let stop_on_irq = self.channels[dmac_ch::FROM_SPR].tie() && tag_irq;
                    if tag_id == 7 || stop_on_irq {
                        break;
                    }
                }
                (total_bus_cycles.max(1), false)
            }
            // Interleave mode (`MOD == 2`, `dma/spr/interleave.expected`)
            2 => {
                let sqwc = self.d_sqwc & 0xFF;
                let tqwc = (self.d_sqwc >> 16) & 0xFF;
                let mut remaining = self.channels[dmac_ch::FROM_SPR].qwc & 0xFFFF;
                let total_qwc = remaining;

                if tqwc == 0 {
                    self.transfer_slice_from_spr(remaining, ram, scratchpad, update_stadr);
                } else {
                    while remaining > 0 {
                        let slice = remaining.min(tqwc);
                        self.transfer_slice_from_spr(slice, ram, scratchpad, update_stadr);
                        remaining -= slice;
                        // Advance MADR by `SQWC * 16` at the end of every TQWC slice
                        // (`ps2autotests` `dma/spr/interleave.expected`).
                        let skip_bytes = sqwc.wrapping_mul(16);
                        let ch = &mut self.channels[dmac_ch::FROM_SPR];
                        ch.madr = ch.madr.wrapping_add(skip_bytes) & 0x7FFF_FFF0;
                    }
                }
                (total_qwc.max(1), false)
            }
            _ => (1, false),
        }
    }

    /// Execute `D9` (`toSPR`: RDRAM -> Scratchpad) in Normal (`0`), Source Chain (`1`),
    /// or Interleave (`2`) mode. Returns `(bus_cycles, stalled)`.
    fn execute_to_spr(&mut self, ram: &[u8], scratchpad: &mut [u8]) -> (u32, bool) {
        let mode = self.channels[dmac_ch::TO_SPR].mode();

        match mode {
            // Normal mode (`MOD == 0`)
            0 => {
                let count = self.channels[dmac_ch::TO_SPR].qwc & 0xFFFF;
                self.transfer_slice_to_spr(count, ram, scratchpad);
                (count.max(1), false)
            }
            // Source Chain mode (`MOD == 1`, `ps2tek` `#dmactags`, `dma/dmac/tagintr.expected`, `dma/spr/chain.expected`)
            1 => self.execute_source_chain_to_spr(ram, scratchpad),
            // Interleave mode (`MOD == 2`, `dma/spr/interleave.expected`)
            2 => {
                let sqwc = self.d_sqwc & 0xFF;
                let tqwc = (self.d_sqwc >> 16) & 0xFF;
                let mut remaining = self.channels[dmac_ch::TO_SPR].qwc & 0xFFFF;
                let total_qwc = remaining;

                if tqwc == 0 {
                    self.transfer_slice_to_spr(remaining, ram, scratchpad);
                } else {
                    while remaining > 0 {
                        let slice = remaining.min(tqwc);
                        self.transfer_slice_to_spr(slice, ram, scratchpad);
                        remaining -= slice;
                        // Advance MADR by `SQWC * 16` at the end of every TQWC slice
                        // (`ps2autotests` `dma/spr/interleave.expected`).
                        let skip_bytes = sqwc.wrapping_mul(16);
                        let ch = &mut self.channels[dmac_ch::TO_SPR];
                        ch.madr = ch.madr.wrapping_add(skip_bytes) & 0x7FFF_FFF0;
                    }
                }
                (total_qwc.max(1), false)
            }
            _ => (1, false),
        }
    }

    /// Shared Source Chain tag walker used by `toSPR` (`D9`), `VIF0` (`D0`), and `VIF1` (`D1`).
    fn step_source_chain_tag(
        &mut self,
        ch_idx: usize,
        allow_spr_src: bool,
        ram: &[u8],
        scratchpad: &[u8],
    ) -> SourceChainTagStep {
        let addr_mask = if allow_spr_src {
            0xFFFF_FFF0
        } else {
            0x7FFF_FFF0
        };
        let tadr = self.channels[ch_idx].tadr & addr_mask;
        let tag_bytes = read_source_qwc(ram, scratchpad, tadr, allow_spr_src);
        let tag_lo = read_le_u32(&tag_bytes, 0);
        let tag_addr = read_le_u32(&tag_bytes, 4) & addr_mask;
        let tte_lo = read_le_u32(&tag_bytes, 8);
        let tte_hi = read_le_u32(&tag_bytes, 12);

        let qwc = tag_lo & 0xFFFF;
        let tag_id = (tag_lo >> 28) & 0x7;
        let tag_irq = (tag_lo & (1 << 31)) != 0;

        // Stall check on `REFS` (`tag_id == 4`) when stall control drain is active (`ps2tek` `#dmactags`):
        // if the transfer region exceeds `D_STADR`, set `D_STAT.SIS` and pause before consuming the tag.
        let std_val = (self.d_ctrl & d_ctrl_bits::STD_MASK) >> 6;
        let stall_drain_enabled = (ch_idx == dmac_ch::TO_SPR && std_val != 0)
            || (ch_idx == dmac_ch::VIF1 && std_val == 1);
        if tag_id == 4 && stall_drain_enabled && (tag_addr & 0x8000_0000) == 0 {
            let end_addr = tag_addr.wrapping_add(qwc.wrapping_mul(16));
            if end_addr > self.d_stadr {
                self.d_stat |= d_stat_bits::SIS;
                return SourceChainTagStep::Stalled;
            }
        }

        // Latch upper 16 bits of the tag into `CHCR[31:16]` and set `QWC`.
        {
            let ch = &mut self.channels[ch_idx];
            ch.chcr = (ch.chcr & !chcr_bits::TAG_MASK) | (tag_lo & chcr_bits::TAG_MASK);
            ch.qwc = qwc;
        }

        let spr_bit = tadr & 0x8000_0000;
        let next_seq = spr_bit | (tadr.wrapping_add(16) & 0x7FFF_FFF0);
        let mut end_chain = false;
        match tag_id {
            // 0: REFE — transfer `QWC` qwords from `ADDR`, `TADR += 16`, then end chain.
            0 => {
                let ch = &mut self.channels[ch_idx];
                ch.madr = tag_addr;
                ch.tadr = next_seq;
                end_chain = true;
            }
            // 1: CNT — transfer `QWC` qwords following the tag (`TADR + 16`), `TADR = MADR + QWC * 16`.
            1 => {
                let next_tadr =
                    spr_bit | (next_seq.wrapping_add(qwc.wrapping_mul(16)) & 0x7FFF_FFF0);
                let ch = &mut self.channels[ch_idx];
                ch.madr = next_seq;
                ch.tadr = next_tadr;
            }
            // 2: NEXT — transfer `QWC` qwords following the tag (`TADR + 16`), `TADR = ADDR`.
            2 => {
                let ch = &mut self.channels[ch_idx];
                ch.madr = next_seq;
                ch.tadr = tag_addr;
            }
            // 3: REF / 4: REFS — transfer `QWC` qwords from `ADDR`, `TADR += 16`.
            3 | 4 => {
                let ch = &mut self.channels[ch_idx];
                ch.madr = tag_addr;
                ch.tadr = next_seq;
            }
            // 5: CALL — transfer `QWC` qwords following the tag (`TADR + 16`), push `MADR + QWC * 16`
            //    onto `ASR0`/`ASR1` (up to depth 2), and set `TADR = ADDR` (`ps2tek` `#dmactags`).
            5 => {
                let ret_addr =
                    spr_bit | (next_seq.wrapping_add(qwc.wrapping_mul(16)) & 0x7FFF_FFF0);
                let ch = &mut self.channels[ch_idx];
                ch.madr = next_seq;
                match ch.asp() {
                    0 => {
                        ch.asr0 = ret_addr;
                        ch.set_asp(1);
                        ch.tadr = tag_addr;
                    }
                    1 => {
                        ch.asr1 = ret_addr;
                        ch.set_asp(2);
                        ch.tadr = tag_addr;
                    }
                    _ => {
                        // Stack overflow (`ASP == 2`): terminate chain after transferring `QWC`.
                        end_chain = true;
                    }
                }
            }
            // 6: RET — transfer `QWC` qwords following the tag (`TADR + 16`), pop `TADR` from `ASR1`/`ASR0`
            //    or end chain if `ASP == 0` (`ps2tek` `#dmactags`).
            6 => {
                let ch = &mut self.channels[ch_idx];
                ch.madr = next_seq;
                match ch.asp() {
                    2 => {
                        ch.tadr = ch.asr1;
                        ch.set_asp(1);
                    }
                    1 => {
                        ch.tadr = ch.asr0;
                        ch.set_asp(0);
                    }
                    _ => {
                        end_chain = true;
                    }
                }
            }
            // 7: END — transfer `QWC` qwords following the tag (`TADR + 16`), leave `TADR` unchanged,
            //    and end chain (`ps2autotests` `dma/dmac/tagintr.expected` lines 5, 7, 9: `TADR offset: 00000020`).
            7 => {
                let ch = &mut self.channels[ch_idx];
                ch.madr = next_seq;
                end_chain = true;
            }
            _ => unreachable!(),
        }

        let stop_on_irq = self.channels[ch_idx].tie() && tag_irq;
        SourceChainTagStep::Advanced {
            tte_lo,
            tte_hi,
            qwc,
            end_chain: end_chain || stop_on_irq,
        }
    }

    /// Execute Source Chain mode on `D9` (`toSPR`), supporting all 8 architectural DMATag IDs
    /// via [`Self::step_source_chain_tag`].
    fn execute_source_chain_to_spr(&mut self, ram: &[u8], scratchpad: &mut [u8]) -> (u32, bool) {
        let mut total_bus_cycles = 0u32;

        for _ in 0..MAX_CHAIN_TAGS_PER_TRANSFER {
            match self.step_source_chain_tag(dmac_ch::TO_SPR, false, ram, scratchpad) {
                SourceChainTagStep::Stalled => return (total_bus_cycles.max(1), true),
                SourceChainTagStep::Advanced { qwc, end_chain, .. } => {
                    self.transfer_slice_to_spr(qwc, ram, scratchpad);
                    total_bus_cycles = total_bus_cycles.saturating_add(1 + qwc);
                    if end_chain {
                        break;
                    }
                }
            }
        }

        (total_bus_cycles.max(1), false)
    }

    /// Copy `count` 128-bit quadwords from `Scratchpad[SADR]` to `RDRAM[MADR]`, wrapping `SADR`
    /// modulo 16 KB (`& 0x3FF0`) after each quadword (`dma/spr/normal.expected` lines 30..35).
    fn transfer_slice_from_spr(
        &mut self,
        count: u32,
        ram: &mut [u8],
        scratchpad: &[u8],
        update_stadr: bool,
    ) {
        let ch = &mut self.channels[dmac_ch::FROM_SPR];
        for _ in 0..count {
            let spr_off = (ch.sadr as usize) & (SCRATCHPAD_SIZE - 16);
            let ram_off = (ch.madr as usize) & (EE_RAM_SIZE - 16);
            ram[ram_off..ram_off + 16].copy_from_slice(&scratchpad[spr_off..spr_off + 16]);
            ch.sadr = ch.sadr.wrapping_add(16) & 0x3FF0;
            ch.madr = ch.madr.wrapping_add(16) & 0x7FFF_FFF0;
            ch.qwc = ch.qwc.wrapping_sub(1) & 0xFFFF;
        }
        if update_stadr {
            self.d_stadr = ch.madr;
        }
    }

    /// Copy `count` 128-bit quadwords from `RDRAM[MADR]` to `Scratchpad[SADR]`, wrapping `SADR`
    /// modulo 16 KB (`& 0x3FF0`) after each quadword (`dma/spr/normal.expected` lines 30..35).
    fn transfer_slice_to_spr(&mut self, count: u32, ram: &[u8], scratchpad: &mut [u8]) {
        let ch = &mut self.channels[dmac_ch::TO_SPR];
        for _ in 0..count {
            let ram_off = (ch.madr as usize) & (EE_RAM_SIZE - 16);
            let spr_off = (ch.sadr as usize) & (SCRATCHPAD_SIZE - 16);
            scratchpad[spr_off..spr_off + 16].copy_from_slice(&ram[ram_off..ram_off + 16]);
            ch.sadr = ch.sadr.wrapping_add(16) & 0x3FF0;
            ch.madr = ch.madr.wrapping_add(16) & 0x7FFF_FFF0;
            ch.qwc = ch.qwc.wrapping_sub(1) & 0xFFFF;
        }
    }
}

/// Read one 16-byte aligned quadword from EE RAM or Scratchpad (`bit 31` when `allow_spr`).
#[inline]
fn read_source_qwc(ram: &[u8], scratchpad: &[u8], addr: u32, allow_spr: bool) -> [u8; 16] {
    let mut out = [0u8; 16];
    if allow_spr && (addr & 0x8000_0000) != 0 {
        let off = (addr as usize) & (SCRATCHPAD_SIZE - 16);
        out.copy_from_slice(&scratchpad[off..off + 16]);
    } else {
        let off = (addr as usize) & (EE_RAM_SIZE - 16);
        out.copy_from_slice(&ram[off..off + 16]);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dmatag(ram: &mut [u8], offset: u32, qwc: u16, id: u32, irq: bool, addr: u32) {
        let lo = u32::from(qwc) | ((id & 0x7) << 28) | if irq { 1 << 31 } else { 0 };
        let off = offset as usize;
        ram[off..off + 4].copy_from_slice(&lo.to_le_bytes());
        ram[off + 4..off + 8].copy_from_slice(&addr.to_le_bytes());
        ram[off + 8..off + 16].fill(0);
    }

    #[test]
    fn dmac_call_ret_two_level_stack_and_zero_qwc_tags() {
        let mut dmac = EeDmac::new();
        let mut ram = vec![0u8; EE_RAM_SIZE];
        let mut spr = vec![0u8; SCRATCHPAD_SIZE];
        let mut sched = Scheduler::new();

        // Build a 2-level nested CALL/RET chain in RAM:
        // 0x1000: CALL (id=5, qwc=0) -> target sub1 at 0x2000 (pushes ASR0 = 0x1010, ASP = 1)
        // 0x1010: END  (id=7, qwc=1) -> payload at 0x1020 (transfers 1 qword, ends chain with TADR = 0x1010)
        // 0x2000: CALL (id=5, qwc=1) -> payload at 0x2010, target sub2 at 0x3000 (pushes ASR1 = 0x2020, ASP = 2)
        // 0x2020: RET  (id=6, qwc=0) -> pops ASR0 (0x1010), ASP = 0
        // 0x3000: RET  (id=6, qwc=1) -> payload at 0x3010, pops ASR1 (0x2020), ASP = 1
        write_dmatag(&mut ram, 0x1000, 0, 5, false, 0x2000);
        write_dmatag(&mut ram, 0x1010, 1, 7, false, 0);
        ram[0x1020..0x1024].copy_from_slice(&0x3333_3333u32.to_le_bytes());

        write_dmatag(&mut ram, 0x2000, 1, 5, false, 0x3000);
        ram[0x2010..0x2014].copy_from_slice(&0x1111_1111u32.to_le_bytes());
        write_dmatag(&mut ram, 0x2020, 0, 6, false, 0);

        write_dmatag(&mut ram, 0x3000, 1, 6, false, 0);
        ram[0x3010..0x3014].copy_from_slice(&0x2222_2222u32.to_le_bytes());

        // Start toSPR (`0x1000_D400`) in Chain mode (`MOD = 1`, `STR = 1`).
        dmac.write32(0x1000_D430, 0x1000, &mut ram, &mut spr, &mut sched); // TADR = 0x1000
        dmac.write32(0x1000_D480, 0x0000, &mut ram, &mut spr, &mut sched); // SADR = 0x0000
        dmac.write32(
            0x1000_D400,
            (1 << 2) | chcr_bits::STR,
            &mut ram,
            &mut spr,
            &mut sched,
        );

        // Verify ASP returned to 0, ASR0 = 0x1010, ASR1 = 0x2020, TADR = 0x1010 (END tag), SADR = 0x0030 (3 qwords).
        let ch9 = &dmac.channels[dmac_ch::TO_SPR];
        assert_eq!(ch9.asp(), 0);
        assert_eq!(ch9.asr0, 0x1010);
        assert_eq!(ch9.asr1, 0x2020);
        assert_eq!(ch9.tadr, 0x1010);
        assert_eq!(ch9.sadr, 0x0030);
        assert_eq!(read_le_u32(&spr, 0x00), 0x1111_1111);
        assert_eq!(read_le_u32(&spr, 0x10), 0x2222_2222);
        assert_eq!(read_le_u32(&spr, 0x20), 0x3333_3333);
    }

    #[test]
    fn dmac_stall_control_refs_and_dst_cnts() {
        let mut dmac = EeDmac::new();
        let mut ram = vec![0u8; EE_RAM_SIZE];
        let mut spr = vec![0u8; SCRATCHPAD_SIZE];
        let mut sched = Scheduler::new();

        // Enable STS = fromSPR (2 << 4) and STD = VIF1 (1 << 6) in D_CTRL, with D_STADR = 0x4000.
        dmac.write32(
            0x1000_E000,
            d_ctrl_bits::DMAE | (2 << 4) | (1 << 6),
            &mut ram,
            &mut spr,
            &mut sched,
        );
        dmac.write32(0x1000_E060, 0x4000, &mut ram, &mut spr, &mut sched);

        // Place a REFS tag (id=4, qwc=2, addr=0x4000) at TADR=0x1000: end_addr = 0x4020 > D_STADR (0x4000) -> stalls!
        write_dmatag(&mut ram, 0x1000, 2, 4, false, 0x4000);
        dmac.write32(0x1000_D430, 0x1000, &mut ram, &mut spr, &mut sched);
        dmac.write32(
            0x1000_D400,
            (1 << 2) | chcr_bits::STR,
            &mut ram,
            &mut spr,
            &mut sched,
        );

        // Verify D_STAT.SIS (bit 13) is set and channel 9 remains busy (STR == 1) without transferring.
        assert_ne!(dmac.d_stat & d_stat_bits::SIS, 0);
        assert_ne!(dmac.channels[dmac_ch::TO_SPR].chcr & chcr_bits::STR, 0);
        assert_eq!(dmac.channels[dmac_ch::TO_SPR].sadr, 0);

        // Now run a fromSPR Destination Chain with DST_CNTS (id=0, qwc=2, addr=0x4000) to advance D_STADR to 0x4020.
        write_dmatag(&mut spr, 0x0100, 2, 0, false, 0x4000);
        write_dmatag(&mut spr, 0x0130, 0, 7, false, 0x4020);
        dmac.write32(0x1000_D080, 0x0100, &mut ram, &mut spr, &mut sched);
        dmac.write32(
            0x1000_D000,
            (1 << 2) | chcr_bits::STR,
            &mut ram,
            &mut spr,
            &mut sched,
        );
        assert_eq!(dmac.d_stadr, 0x4020);
    }

    #[test]
    fn dmac_unimplemented_peripheral_channel_records_explicit_error() {
        let mut dmac = EeDmac::new();
        let mut ram = vec![0u8; EE_RAM_SIZE];
        let mut spr = vec![0u8; SCRATCHPAD_SIZE];
        let mut sched = Scheduler::new();

        // Attempt to start a fromIPU (channel 3, 0x1000_B000) transfer.
        dmac.write32(0x1000_B010, 0x0010_0000, &mut ram, &mut spr, &mut sched);
        dmac.write32(0x1000_B020, 4, &mut ram, &mut spr, &mut sched);
        dmac.write32(0x1000_B000, chcr_bits::STR, &mut ram, &mut spr, &mut sched);

        assert_eq!(
            dmac.unimplemented_transfer,
            Some(UnimplementedDmaTransfer {
                channel: 3,
                channel_name: "fromIPU",
                chcr: chcr_bits::STR,
                madr: 0x0010_0000,
                qwc: 4,
                tadr: 0,
            })
        );
    }

    #[test]
    fn dmac_gif_d2_backpressure_source_chain_and_unimplemented_dir0() {
        let mut dmac = EeDmac::new();
        let mut ram = vec![0u8; EE_RAM_SIZE];
        let mut spr = vec![0u8; SCRATCHPAD_SIZE];
        let mut sched = Scheduler::new();
        let mut gif = GifUnit::new();

        // Fill `D2` chain with 18 QWCs via two tags (`CNT` 10 + `END` 8) to exercise 16-QWC backpressure.
        let tadr = 0x1000usize;
        let payload2 = tadr + 16 + 10 * 16;
        // Tag 0: CNT QWC=10
        let tag0_lo = 10u32 | (1u32 << 28);
        ram[tadr..tadr + 4].copy_from_slice(&tag0_lo.to_le_bytes());
        ram[tadr + 4..tadr + 8].copy_from_slice(&0u32.to_le_bytes());
        for i in 0..10u32 {
            let off = tadr + 16 + (i as usize) * 16;
            ram[off..off + 4].copy_from_slice(&(0x100 + i).to_le_bytes());
        }
        // Tag 1 at `payload2`: END QWC=8
        let tag1_lo = 8u32 | (7u32 << 28);
        ram[payload2..payload2 + 4].copy_from_slice(&tag1_lo.to_le_bytes());
        ram[payload2 + 4..payload2 + 8].copy_from_slice(&0u32.to_le_bytes());
        for i in 0..8u32 {
            let off = payload2 + 16 + (i as usize) * 16;
            ram[off..off + 4].copy_from_slice(&(0x200 + i).to_le_bytes());
        }

        dmac.write32(0x1000_A030, tadr as u32, &mut ram, &mut spr, &mut sched);
        dmac.write32(
            0x1000_A000,
            chcr_bits::DIR | (1 << 2) | chcr_bits::STR,
            &mut ram,
            &mut spr,
            &mut sched,
        );

        // Read Tag 0 + push 10 QWCs + read Tag 1 + push 6 QWCs = 18 steps fill PATH3 16-QWC FIFO,
        // then step 19 stalls cleanly on backpressure!
        for _ in 0..18 {
            assert!(dmac.step_gif_channel(&mut gif, &ram, &spr));
        }
        assert_eq!(gif.path3_fifo_len(), 16);
        assert!(!dmac.gif_channel_can_step(&gif));
        assert!(!dmac.step_gif_channel(&mut gif, &ram, &spr));
        assert_ne!(dmac.channels[dmac_ch::GIF].chcr & chcr_bits::STR, 0);

        // Starting D2 with `DIR=0` (GS->EE) must record explicit unimplemented error.
        dmac.write32(0x1000_A000, chcr_bits::STR, &mut ram, &mut spr, &mut sched);
        assert_eq!(dmac.unimplemented_transfer.map(|u| u.channel), Some(2));
    }

    #[test]
    fn dmac_vif0_fifo_backpressure_stalls_and_resumes_mid_packet() {
        use crate::vif::{VifKind, VifUnit};
        use crate::vu::VuState;

        let mut dmac = EeDmac::new();
        let mut ram = vec![0u8; EE_RAM_SIZE];
        let mut spr = vec![0u8; SCRATCHPAD_SIZE];
        let mut sched = Scheduler::new();
        let mut vif0 = VifUnit::new(VifKind::Vif0);
        let mut vu0 = VuState::new();
        let mut vu0_micro = vec![0u8; 0x1000];
        let mut vu0_data = vec![0u8; 0x1000];

        // Pack 20 QWCs in RAM: `NOP(I)` followed by `STMASK` (triggers `VIS` stall at word 1),
        // followed by 19 more QWCs ending with `MARK(0xCAFE)`.
        let q0 = [0x8000_0000u32, 0x20 << 24, 0x1234_5678, 0];
        for (i, w) in q0.iter().enumerate() {
            ram[0x1000 + i * 4..0x1004 + i * 4].copy_from_slice(&w.to_le_bytes());
        }
        for q in 1..20u32 {
            let base = 0x1000 + (q as usize) * 16;
            let w0 = if q == 19 { (0x07u32 << 24) | 0xCAFE } else { 0 };
            ram[base..base + 4].copy_from_slice(&w0.to_le_bytes());
        }

        dmac.write32(0x1000_8010, 0x1000, &mut ram, &mut spr, &mut sched); // MADR
        dmac.write32(0x1000_8020, 20, &mut ram, &mut spr, &mut sched); // QWC = 20
        dmac.write32(
            0x1000_8000,
            chcr_bits::STR | chcr_bits::DIR,
            &mut ram,
            &mut spr,
            &mut sched,
        );

        // Must NOT transfer synchronously! Scheduler holds `DmacTick` for cycle 2.
        assert_eq!(dmac.channels[dmac_ch::VIF0].qwc, 20);
        assert_eq!(sched.pop_due_event(), None);
        sched.advance(2);
        assert_eq!(
            sched.pop_due_event(),
            Some(crate::scheduler::ScheduledEvent {
                trigger_cycle: 2,
                kind: HardwareEventKind::DmacTick
            })
        );

        // Step DMAC beat by beat alongside VIF0:
        // `work_words` holds 4 QWCs (QWCs 0..3) and `dma_fifo` holds 8 QWCs (QWCs 4..11).
        // Starting at QWC 12, DMAC must stall mid-packet with `STR == 1` and `QWC == 8`.
        for _ in 0..30 {
            dmac.step_vif_channel(dmac_ch::VIF0, &mut vif0, &ram, &spr);
            vif0.step(&mut vu0, &mut vu0_micro, &mut vu0_data, None);
        }
        assert!(vif0.stat_vis);
        assert_eq!(vif0.fqc(), 8);
        assert_eq!(dmac.channels[dmac_ch::VIF0].qwc, 8);
        assert_ne!(dmac.channels[dmac_ch::VIF0].chcr & chcr_bits::STR, 0);

        // Clear `VIS` via `FBRST.STC`, resume ticking, and confirm packet completes mid-stream!
        vif0.write32(0x1000_3810, 0x08);
        for _ in 0..30 {
            vif0.step(&mut vu0, &mut vu0_micro, &mut vu0_data, None);
            dmac.step_vif_channel(dmac_ch::VIF0, &mut vif0, &ram, &spr);
        }
        vif0.step(&mut vu0, &mut vu0_micro, &mut vu0_data, None);
        assert_eq!(dmac.channels[dmac_ch::VIF0].qwc, 0);
        assert_eq!(dmac.channels[dmac_ch::VIF0].chcr & chcr_bits::STR, 0);
        assert_ne!(dmac.d_stat & 1, 0);
        assert_eq!(vif0.mask, 0x1234_5678);
        assert_eq!(vif0.mark, 0xCAFE);
    }

    #[test]
    fn dmac_vif1_chain_tte_and_scratchpad_bit31_source() {
        use crate::gif::GifUnit;
        use crate::vif::{VifKind, VifUnit};
        use crate::vu::VuState;

        let mut dmac = EeDmac::new();
        let mut ram = vec![0u8; EE_RAM_SIZE];
        let mut spr = vec![0u8; SCRATCHPAD_SIZE];
        let mut sched = Scheduler::new();
        let mut vif1 = VifUnit::new(VifKind::Vif1);
        let mut vu1 = VuState::new();
        let mut vu1_micro = vec![0u8; 0x4000];
        let mut vu1_data = vec![0u8; 0x4000];
        let mut gif = GifUnit::new();

        // Place DMAtag in Scratchpad at offset `0x0100` (`TADR = 0x8000_0100`):
        // `END` (id=7, qwc=1), TTE words = `[STCYCL(0x0101), UNPACK V4-32 NUM=1 addr=4]`,
        // payload immediately after in Scratchpad at `0x0110` = `[0x11, 0x22, 0x33, 0x44]`.
        let lo = 1u32 | (7u32 << 28);
        let tte0 = (0x01u32 << 24) | 0x0101;
        let tte1 = (0x6Cu32 << 24) | (1u32 << 16) | 4;
        spr[0x0100..0x0104].copy_from_slice(&lo.to_le_bytes());
        spr[0x0104..0x0108].copy_from_slice(&0u32.to_le_bytes());
        spr[0x0108..0x010C].copy_from_slice(&tte0.to_le_bytes());
        spr[0x010C..0x0110].copy_from_slice(&tte1.to_le_bytes());
        for (i, v) in [0x11u32, 0x22, 0x33, 0x44].iter().enumerate() {
            spr[0x0110 + i * 4..0x0114 + i * 4].copy_from_slice(&v.to_le_bytes());
        }

        dmac.write32(0x1000_9030, 0x8000_0100, &mut ram, &mut spr, &mut sched); // TADR in SPR
        dmac.write32(
            0x1000_9000,
            chcr_bits::DIR | (1 << 2) | chcr_bits::TTE | chcr_bits::STR,
            &mut ram,
            &mut spr,
            &mut sched,
        );

        for _ in 0..8 {
            dmac.step_vif_channel(dmac_ch::VIF1, &mut vif1, &ram, &spr);
            vif1.step(&mut vu1, &mut vu1_micro, &mut vu1_data, Some(&mut gif));
        }

        assert_eq!(dmac.channels[dmac_ch::VIF1].chcr & chcr_bits::STR, 0);
        assert_ne!(dmac.d_stat & (1 << 1), 0);
        assert_eq!(vif1.cycle, 0x0101);
        for i in 0..4usize {
            let word = read_le_u32(&vu1_data, 4 * 16 + i * 4);
            assert_eq!(word, [0x11u32, 0x22, 0x33, 0x44][i]);
        }
    }
}
