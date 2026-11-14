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

use crate::scheduler::{HardwareEventKind, Scheduler};

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
    /// `0x1000_F520` (`D_ENABLER`) / `0x1000_F590` (`D_ENABLEW`): Master DMA hold/suspend register
    /// (initialized to `0x1201` per `ps2tek` `https://psi-rockin.github.io/ps2tek/#dmacio`).
    pub d_enable: u32,
    /// Pending unimplemented peripheral DMA transfer error (channels `0..=7`), if triggered by guest code.
    pub unimplemented_transfer: Option<UnimplementedDmaTransfer>,
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
            d_enable: 0x1201,
            unimplemented_transfer: None,
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
            0x1000_F520 | 0x1000_F590 => self.d_enable,
            _ => 0,
        }
    }

    /// Write a 32-bit DMAC register at `paddr` (`0x1000_8000..=0x1000_E060` or `0x1000_F590`),
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

    fn start_channel_transfer(
        &mut self,
        ch_idx: usize,
        ram: &mut [u8],
        scratchpad: &mut [u8],
        scheduler: &mut Scheduler,
    ) {
        // Check master DMA enable (`D_CTRL.DMAE`) and master suspend (`D_ENABLE.CPND` bit 16).
        if (self.d_ctrl & d_ctrl_bits::DMAE) == 0 || (self.d_enable & (1 << 16)) != 0 {
            return;
        }

        // Cancel any previously scheduled completion event for this channel.
        scheduler.cancel_by_kind(HardwareEventKind::DmacChannelComplete(ch_idx as u8));

        match ch_idx {
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
                // Channels 0..=7 (`VIF0`, `VIF1`, `GIF`, `fromIPU`, `toIPU`, `SIF0`, `SIF1`, `SIF2`)
                // are register-level only in Task 5; record an explicit unimplemented-transfer error.
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

    /// Execute Source Chain mode on `D9` (`toSPR`), supporting all 8 architectural DMATag IDs
    /// (`REFE`, `CNT`, `NEXT`, `REF`, `REFS`, `CALL`, `RET`, `END`), `ASR0`/`ASR1` 2-level call stack,
    /// `TIE` + `tag.IRQ` early termination, and `REFS` stall control against `D_STADR`.
    fn execute_source_chain_to_spr(&mut self, ram: &[u8], scratchpad: &mut [u8]) -> (u32, bool) {
        let mut total_bus_cycles = 0u32;
        let stall_drain_enabled = ((self.d_ctrl & d_ctrl_bits::STD_MASK) >> 6) != 0;

        for _ in 0..MAX_CHAIN_TAGS_PER_TRANSFER {
            let tadr = self.channels[dmac_ch::TO_SPR].tadr & 0x7FFF_FFF0;
            let ram_off = (tadr as usize) & (EE_RAM_SIZE - 16);
            let tag_lo = read_le_u32(ram, ram_off);
            let tag_addr = read_le_u32(ram, ram_off + 4) & 0x7FFF_FFF0;

            let qwc = tag_lo & 0xFFFF;
            let tag_id = (tag_lo >> 28) & 0x7;
            let tag_irq = (tag_lo & (1 << 31)) != 0;

            // Stall check on `REFS` (`tag_id == 4`) when stall control drain is active (`ps2tek` `#dmactags`):
            // if the transfer region exceeds `D_STADR`, set `D_STAT.SIS` and pause before executing the tag.
            if tag_id == 4 && stall_drain_enabled {
                let end_addr = tag_addr.wrapping_add(qwc.wrapping_mul(16));
                if end_addr > self.d_stadr {
                    self.d_stat |= d_stat_bits::SIS;
                    return (total_bus_cycles.max(1), true);
                }
            }

            // Latch upper 16 bits of the tag into `CHCR[31:16]` and set `QWC`.
            {
                let ch = &mut self.channels[dmac_ch::TO_SPR];
                ch.chcr = (ch.chcr & !chcr_bits::TAG_MASK) | (tag_lo & chcr_bits::TAG_MASK);
                ch.qwc = qwc;
            }

            let mut end_chain = false;
            match tag_id {
                // 0: REFE — transfer `QWC` qwords from `ADDR`, `TADR += 16`, then end chain.
                0 => {
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = tag_addr;
                    ch.tadr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                    end_chain = true;
                }
                // 1: CNT — transfer `QWC` qwords following the tag (`TADR + 16`), `TADR = MADR + QWC * 16`.
                1 => {
                    let payload_addr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                    let next_tadr = payload_addr.wrapping_add(qwc.wrapping_mul(16)) & 0x7FFF_FFF0;
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = payload_addr;
                    ch.tadr = next_tadr;
                }
                // 2: NEXT — transfer `QWC` qwords following the tag (`TADR + 16`), `TADR = ADDR`.
                2 => {
                    let payload_addr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = payload_addr;
                    ch.tadr = tag_addr;
                }
                // 3: REF / 4: REFS — transfer `QWC` qwords from `ADDR`, `TADR += 16`.
                3 | 4 => {
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = tag_addr;
                    ch.tadr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                }
                // 5: CALL — transfer `QWC` qwords following the tag (`TADR + 16`), push `MADR + QWC * 16`
                //    onto `ASR0`/`ASR1` (up to depth 2), and set `TADR = ADDR` (`ps2tek` `#dmactags`).
                5 => {
                    let payload_addr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                    let ret_addr = payload_addr.wrapping_add(qwc.wrapping_mul(16)) & 0x7FFF_FFF0;
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = payload_addr;
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
                    let payload_addr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = payload_addr;
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
                    let payload_addr = tadr.wrapping_add(16) & 0x7FFF_FFF0;
                    let ch = &mut self.channels[dmac_ch::TO_SPR];
                    ch.madr = payload_addr;
                    end_chain = true;
                }
                _ => unreachable!(),
            }

            self.transfer_slice_to_spr(qwc, ram, scratchpad);
            total_bus_cycles = total_bus_cycles.saturating_add(1 + qwc);

            let stop_on_irq = self.channels[dmac_ch::TO_SPR].tie() && tag_irq;
            if end_chain || stop_on_irq {
                break;
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

        // Attempt to start a GIF (channel 2, 0x1000_A000) transfer.
        dmac.write32(0x1000_A010, 0x0010_0000, &mut ram, &mut spr, &mut sched);
        dmac.write32(0x1000_A020, 4, &mut ram, &mut spr, &mut sched);
        dmac.write32(0x1000_A000, chcr_bits::STR, &mut ram, &mut spr, &mut sched);

        assert_eq!(
            dmac.unimplemented_transfer,
            Some(UnimplementedDmaTransfer {
                channel: 2,
                channel_name: "GIF",
                chcr: chcr_bits::STR,
                madr: 0x0010_0000,
                qwc: 4,
                tadr: 0,
            })
        );
    }
}
