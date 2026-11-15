//! Emotion Engine Vector Unit Interfaces (`VIF0` and `VIF1`).
//!
//! Primary source references (`FETCHED`):
//! - `local/ps2tek/index.html` (`#vif`, `#vifioregisters`, `#vifcommands`, `#vifunpack`)
//! - `local/ps2autotests/tests/dma/vif/{vifregs.h,basic.expected,intstall.expected,mskpath3.expected,stcycl.expected,stmod.expected,unpack.expected}`
//!
//! Hardware notes:
//! - `VIF0` has an `8`-QWC (`128`-byte) DMAC-facing FIFO (`4 KB` VU0 Micro / `4 KB` VU0 Data).
//! - `VIF1` has a `16`-QWC (`256`-byte) DMAC-facing FIFO (`16 KB` VU1 Micro / `16 KB` VU1 Data),
//!   double buffering (`BASE`/`OFST`/`TOPS`/`TOP`/`DBF`), `MSKPATH3`, and Path 2 `DIRECT`/`DIRECTHL` to `GIF`.
//! - Modelled with an inferred `4`-QWC (`16`-word) pre-decode staging buffer (`work_words`) fed from
//!   `dma_fifo`, explaining `STAT.FQC == 0` in `intstall.expected` when `VIS` stalls on word 1 of a 3-QWC DMA.
//! - Register reads (`&self`) are pure state observations and never advance emulation time.

pub mod unpack;

use ps2_common::Reg128;

use crate::gif::GifUnit;
use crate::vu::{VuState, VU0_TPC_MASK, VU1_TPC_MASK};
use unpack::{UnpackCmd, UnpackState, VIF0_DATA_QWC_MASK, VIF1_DATA_QWC_MASK};

/// DMAC-facing FIFO size for `VIF0` in 128-bit quadwords (`8` QWC = `128` bytes = `32` words).
pub const VIF0_FIFO_QWC: usize = 8;
/// DMAC-facing FIFO size for `VIF1` in 128-bit quadwords (`16` QWC = `256` bytes = `64` words).
pub const VIF1_FIFO_QWC: usize = 16;
/// Maximum word capacity of the DMAC-facing FIFO (`VIF1` = `64` words).
const MAX_FIFO_WORDS: usize = VIF1_FIFO_QWC * 4;
/// Inferred 4-QWC (`16`-word) pre-decode staging buffer between `dma_fifo` and the VIF decoder.
const WORK_WORD_CAP: usize = 16;

/// Identifies whether a [`VifUnit`] instance is `VIF0` (VU0) or `VIF1` (VU1 + GIF).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VifKind {
    Vif0,
    Vif1,
}

impl VifKind {
    #[inline]
    pub const fn fifo_qwc_cap(self) -> usize {
        match self {
            Self::Vif0 => VIF0_FIFO_QWC,
            Self::Vif1 => VIF1_FIFO_QWC,
        }
    }

    #[inline]
    pub const fn fifo_word_cap(self) -> usize {
        self.fifo_qwc_cap() * 4
    }

    #[inline]
    pub const fn data_qwc_mask(self) -> usize {
        match self {
            Self::Vif0 => VIF0_DATA_QWC_MASK,
            Self::Vif1 => VIF1_DATA_QWC_MASK,
        }
    }

    #[inline]
    pub const fn tpc_mask(self) -> u16 {
        match self {
            Self::Vif0 => VU0_TPC_MASK,
            Self::Vif1 => VU1_TPC_MASK,
        }
    }

    #[inline]
    pub const fn is_vif1(self) -> bool {
        matches!(self, Self::Vif1)
    }
}

/// `VIFn_STAT.VPS` (`bits 0..=1`) pipeline status (`FETCHED` `#vifioregisters`).
#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum VifVps {
    #[default]
    Idle = 0,
    DataWait = 1,
    Decoding = 2,
    Transferring = 3,
}

/// Active multi-cycle VIF command state (`VifCmdState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum VifCmdState {
    #[default]
    Idle,
    /// `FLUSHE` / `FLUSH` / `FLUSHA`: waits for `!vu.running` (`VEW`) and optionally GIF (`VGW`).
    WaitFlush {
        wait_gif_path12: bool,
        check_gif_path3: bool,
        pending_irq: bool,
    },
    /// `MSCAL` / `MSCALF` / `MSCNT`: waits for `!vu.running` (`VEW`), latches regs, starts VU.
    WaitMscal {
        target_tpc: Option<u16>,
        wait_gif: bool,
        pending_irq: bool,
    },
    /// `MPG` (`0x4A`) waiting for `!vu.running` (`VEW`) before writing VU micro memory.
    WaitMpgVu {
        remaining_words: u16,
        byte_offset: u16,
        pending_irq: bool,
    },
    /// `STMASK` (`0x20`): waiting for 1 `u32` word.
    Stmask { pending_irq: bool },
    /// `STROW` (`0x30`): waiting for 4 `u32` words.
    Strow { idx: u8, pending_irq: bool },
    /// `STCOL` (`0x31`): waiting for 4 `u32` words.
    Stcol { idx: u8, pending_irq: bool },
    /// `MPG` (`0x4A`): uploading `remaining_words` x 32-bit words into VU micro memory.
    Mpg {
        remaining_words: u16,
        byte_offset: u16,
        pending_irq: bool,
    },
    /// `VIF1` `DIRECT` (`0x50`) / `DIRECTHL` (`0x51`): streaming `remaining_qwc` to `GIF`.
    Direct {
        remaining_qwc: u32,
        is_hl: bool,
        started: bool,
        words: [u32; 4],
        word_idx: u8,
        pending_irq: bool,
    },
    /// `UNPACK` (`0x60..=0x7F`): active vector decompression into VU data memory.
    Unpack { pending_irq: bool },
}

/// Self-contained state of a `VIF0` or `VIF1` unit (zero heap allocations).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VifUnit {
    pub kind: VifKind,

    // `VIFn_STAT` flags
    vps: VifVps,
    pub stat_vew: bool,
    pub stat_vgw: bool,
    pub stat_mrk: bool,
    pub dbf: bool,
    pub stat_vss: bool,
    pub stat_vfs: bool,
    pub stat_vis: bool,
    pub stat_int: bool,
    pub stat_er0: bool,
    pub stat_er1: bool,
    /// `VIF1_STAT.FDR` (`bit 23`): `0 = Mem->VIF`, `1 = VIF->Mem` (`VIF1` only).
    pub fdr: bool,

    // `VIFn_ERR`
    pub err_mii: bool,
    pub err_me0: bool,
    pub err_me1: bool,

    // Architectural VIF registers
    pub mark: u16,
    pub cycle: u16,
    pub mode: u8,
    pub mask: u32,
    pub code: u32,
    pub itops: u16,
    pub itop: u16,
    pub base: u16,
    pub ofst: u16,
    pub tops: u16,
    pub top: u16,
    pub row: [u32; 4],
    pub col: [u32; 4],

    /// Set when a non-`MARK` command raises bit 31 `I`; causes `VIS` stall before the next command.
    vis_armed: bool,

    // Inline word FIFOs (zero heap allocation)
    dma_fifo: [u32; MAX_FIFO_WORDS],
    dma_head: u8,
    dma_len: u8,
    work_words: [u32; WORK_WORD_CAP],
    work_head: u8,
    work_len: u8,

    // Active command & unpack decoder
    cmd_state: VifCmdState,
    unpack: UnpackState,
}

impl VifUnit {
    /// Construct a reset `VIF0` or `VIF1` instance.
    pub const fn new(kind: VifKind) -> Self {
        Self {
            kind,
            vps: VifVps::Idle,
            stat_vew: false,
            stat_vgw: false,
            stat_mrk: false,
            dbf: false,
            stat_vss: false,
            stat_vfs: false,
            stat_vis: false,
            stat_int: false,
            stat_er0: false,
            stat_er1: false,
            fdr: false,
            err_mii: false,
            err_me0: false,
            err_me1: false,
            mark: 0,
            cycle: 0,
            mode: 0,
            mask: 0,
            code: 0,
            itops: 0,
            itop: 0,
            base: 0,
            ofst: 0,
            tops: 0,
            top: 0,
            row: [0; 4],
            col: [0; 4],
            vis_armed: false,
            dma_fifo: [0; MAX_FIFO_WORDS],
            dma_head: 0,
            dma_len: 0,
            work_words: [0; WORK_WORD_CAP],
            work_head: 0,
            work_len: 0,
            cmd_state: VifCmdState::Idle,
            unpack: UnpackState::new(),
        }
    }

    /// Reset all VIF state (`FBRST.RST`). Does not alter attached VU execution state.
    pub fn reset(&mut self) {
        *self = Self::new(self.kind);
    }

    /// Return current `FQC` (quadword occupancy of `dma_fifo`: `0..=8` on `VIF0`, `0..=16` on `VIF1`).
    #[inline]
    pub const fn fqc(&self) -> u32 {
        (self.dma_len as u32).div_ceil(4)
    }

    /// Return `true` when `dma_fifo` has room for at least 1 QWC (`4` x 32-bit words).
    #[inline]
    pub const fn can_accept_qwc(&self) -> bool {
        (self.dma_len as usize) + 4 <= self.kind.fifo_word_cap()
    }

    /// Return `true` when `dma_fifo` has room for `2` x 32-bit `DMAC TTE` tag words.
    #[inline]
    pub const fn can_accept_tte(&self) -> bool {
        (self.dma_len as usize) + 2 <= self.kind.fifo_word_cap()
    }

    /// Return `true` when both FIFOs are empty and no command is currently active or stalled.
    #[inline]
    pub const fn is_idle(&self) -> bool {
        self.dma_len == 0
            && self.work_len == 0
            && matches!(self.cmd_state, VifCmdState::Idle)
            && !self.is_stalled()
    }

    /// Return `true` if any software/error/interrupt/unit stall flag currently blocks execution.
    #[inline]
    pub const fn is_stalled(&self) -> bool {
        self.stat_vss || self.stat_vfs || self.stat_vis || self.stat_er0 || self.stat_er1
    }

    /// Push one 128-bit quadword from DMAC or CPU `*VIFn_FIFO` into `dma_fifo`.
    ///
    /// Returns `false` if the DMAC-facing FIFO is full (`!can_accept_qwc()`).
    pub fn push_dma_qwc(&mut self, qwc: Reg128) -> bool {
        if !self.can_accept_qwc() {
            return false;
        }
        let words = qwc.u32_lanes();
        for &w in words {
            self.push_fifo_word_unchecked(w);
        }
        true
    }

    /// Push the upper 64 bits (`bits 64..=127`) of a DMAC Source Chain tag (`CHCR.TTE = 1`).
    pub fn push_dma_tte(&mut self, lo: u32, hi: u32) -> bool {
        if !self.can_accept_tte() {
            return false;
        }
        self.push_fifo_word_unchecked(lo);
        self.push_fifo_word_unchecked(hi);
        true
    }

    /// Push a single 32-bit word written by CPU directly into `*VIFn_FIFO`.
    pub fn push_cpu_fifo_word(&mut self, word: u32) -> bool {
        if (self.dma_len as usize) >= self.kind.fifo_word_cap() {
            return false;
        }
        self.push_fifo_word_unchecked(word);
        true
    }

    /// Push a 128-bit quadword written by CPU directly into `*VIFn_FIFO`.
    #[inline]
    pub fn push_cpu_fifo_qwc(&mut self, qwc: Reg128) -> bool {
        self.push_dma_qwc(qwc)
    }

    #[inline]
    fn push_fifo_word_unchecked(&mut self, word: u32) {
        let cap = self.kind.fifo_word_cap();
        let tail = ((self.dma_head as usize) + (self.dma_len as usize)) % cap;
        self.dma_fifo[tail] = word;
        self.dma_len += 1;
    }

    #[inline]
    fn pop_fifo_word(&mut self) -> Option<u32> {
        if self.dma_len == 0 {
            return None;
        }
        let cap = self.kind.fifo_word_cap();
        let word = self.dma_fifo[self.dma_head as usize];
        self.dma_head = (((self.dma_head as usize) + 1) % cap) as u8;
        self.dma_len -= 1;
        Some(word)
    }

    #[inline]
    fn refill_work_words(&mut self) {
        while (self.work_len as usize) < WORK_WORD_CAP {
            let Some(word) = self.pop_fifo_word() else {
                break;
            };
            let tail = ((self.work_head as usize) + (self.work_len as usize)) % WORK_WORD_CAP;
            self.work_words[tail] = word;
            self.work_len += 1;
        }
    }

    #[inline]
    const fn peek_work_word(&self) -> Option<u32> {
        if self.work_len == 0 {
            None
        } else {
            Some(self.work_words[self.work_head as usize])
        }
    }

    #[inline]
    fn pop_work_word(&mut self) -> Option<u32> {
        if self.work_len == 0 {
            return None;
        }
        let word = self.work_words[self.work_head as usize];
        self.work_head = (((self.work_head as usize) + 1) % WORK_WORD_CAP) as u8;
        self.work_len -= 1;
        Some(word)
    }

    /// Compute current `VIFn_NUM` value (`8`-bit on `VIF0`, `9`-bit up to `256` on `VIF1`).
    pub const fn read_num(&self) -> u32 {
        let raw = match &self.cmd_state {
            VifCmdState::Mpg {
                remaining_words, ..
            }
            | VifCmdState::WaitMpgVu {
                remaining_words, ..
            } => remaining_words.div_ceil(2) as u32,
            VifCmdState::Unpack { .. } => self.unpack.remaining as u32,
            _ => 0,
        };
        if self.kind.is_vif1() {
            raw & 0x1FF
        } else {
            raw & 0xFF
        }
    }

    /// Compute current `VIFn_STAT` bitmask (`FETCHED` `#vifioregisters`).
    pub const fn read_stat(&self) -> u32 {
        let mut val = (self.vps as u32) & 0x3;
        if self.stat_vew {
            val |= 1 << 2;
        }
        if self.kind.is_vif1() && self.stat_vgw {
            val |= 1 << 3;
        }
        if self.stat_mrk {
            val |= 1 << 6;
        }
        if self.kind.is_vif1() && self.dbf {
            val |= 1 << 7;
        }
        if self.stat_vss {
            val |= 1 << 8;
        }
        if self.stat_vfs {
            val |= 1 << 9;
        }
        if self.stat_vis {
            val |= 1 << 10;
        }
        if self.stat_int {
            val |= 1 << 11;
        }
        if self.stat_er0 {
            val |= 1 << 12;
        }
        if self.stat_er1 {
            val |= 1 << 13;
        }
        if self.kind.is_vif1() && self.fdr {
            val |= 1 << 23;
        }
        val |= (self.fqc() & 0x1F) << 24;
        val
    }

    /// Pure 32-bit register read (`&self`; never mutates VIF or advances time).
    pub fn read32(&self, paddr: u32) -> u32 {
        let reg = paddr & 0x03F0;
        match reg {
            0x000 => self.read_stat(),
            0x010 => 0, // `FBRST` is write-only
            0x020 => {
                (self.err_mii as u32) | ((self.err_me0 as u32) << 1) | ((self.err_me1 as u32) << 2)
            }
            0x030 => self.mark as u32,
            0x040 => self.cycle as u32,
            0x050 => self.mode as u32,
            0x060 => self.read_num(),
            0x070 => self.mask,
            0x080 => self.code,
            0x090 => (self.itops as u32) & 0x3FF,
            0x0A0 => {
                if self.kind.is_vif1() {
                    (self.base as u32) & 0x3FF
                } else {
                    0
                }
            }
            0x0B0 => {
                if self.kind.is_vif1() {
                    (self.ofst as u32) & 0x3FF
                } else {
                    0
                }
            }
            0x0C0 => {
                if self.kind.is_vif1() {
                    (self.tops as u32) & 0x3FF
                } else {
                    0
                }
            }
            0x0D0 => (self.itop as u32) & 0x3FF,
            0x0E0 => {
                if self.kind.is_vif1() {
                    (self.top as u32) & 0x3FF
                } else {
                    0
                }
            }
            0x100..=0x130 => self.row[((reg >> 4) & 3) as usize],
            0x140..=0x170 => self.col[((reg >> 4) & 3) as usize],
            _ => 0,
        }
    }

    /// 32-bit MMIO register write (`VIFn_STAT.FDR`, `VIFn_FBRST`, `VIFn_ERR`, `VIFn_MARK`).
    pub fn write32(&mut self, paddr: u32, val: u32) {
        let reg = paddr & 0x03F0;
        match reg {
            0x000 => {
                // Only `FDR` (bit 23 on `VIF1`) is writable via `VIF1_STAT`.
                if self.kind.is_vif1() {
                    self.fdr = (val & (1 << 23)) != 0;
                }
            }
            0x010 => {
                // `VIFn_FBRST` (`FETCHED` `#vifioregisters`)
                if (val & 0x01) != 0 {
                    self.reset();
                    return;
                }
                if (val & 0x02) != 0 {
                    // `FBK`: force break current command, set `VFS`.
                    self.stat_vfs = true;
                    self.cmd_state = VifCmdState::Idle;
                    self.unpack.reset();
                    self.stat_vew = false;
                    self.stat_vgw = false;
                    self.vps = VifVps::Idle;
                }
                if (val & 0x04) != 0 {
                    // `STP`: stop at current/next VIF command (`VSS`).
                    self.stat_vss = true;
                }
                if (val & 0x08) != 0 {
                    // `STC`: clear `VSS`, `VFS`, `VIS`, `INT`, `ER0`, `ER1`.
                    self.stat_vss = false;
                    self.stat_vfs = false;
                    self.stat_vis = false;
                    self.stat_int = false;
                    self.stat_er0 = false;
                    self.stat_er1 = false;
                    self.vis_armed = false;
                }
            }
            0x020 => {
                self.err_mii = (val & 0x01) != 0;
                self.err_me0 = (val & 0x02) != 0;
                self.err_me1 = (val & 0x04) != 0;
            }
            0x030 => {
                self.mark = (val & 0xFFFF) as u16;
                self.stat_mrk = false;
            }
            _ => {}
        }
    }

    /// Record a DMAtag mismatch error (`ER0`) if unmasked (`!ERR.ME0`).
    pub fn flag_dma_tag_error(&mut self) {
        if !self.err_me0 {
            self.stat_er0 = true;
        }
    }

    /// Advance VIF execution until blocked on data, VU/GIF stall, or `VIS`/`ER1` interrupt stall.
    ///
    /// Returns `true` if `STAT.INT` transitioned `false -> true` during this step (caller must
    /// assert the matching `INTC` flag `IntcFlags::VIF0` or `IntcFlags::VIF1`).
    pub fn step(
        &mut self,
        vu: &mut VuState,
        vu_micro_mem: &mut [u8],
        vu_data_mem: &mut [u8],
        mut gif: Option<&mut GifUnit>,
    ) -> bool {
        let mut raised_irq = false;
        let data_qwc_mask = self.kind.data_qwc_mask();
        let micro_mask = vu_micro_mem.len().saturating_sub(1);

        // Staging buffer drains continuously from `dma_fifo` even if VIF is stalled (`intstall.expected`).
        self.refill_work_words();

        if self.is_stalled() {
            return false;
        }

        // Guard against infinite zero-consumption loops in malformed packets.
        for _ in 0..4096 {
            self.refill_work_words();
            if self.is_stalled() {
                break;
            }

            let state = core::mem::take(&mut self.cmd_state);
            match state {
                VifCmdState::Idle => {
                    let Some(next_word) = self.peek_work_word() else {
                        self.vps = VifVps::Idle;
                        break;
                    };

                    if self.vis_armed {
                        // Interrupt stall triggers before executing the next VIFcode (`intstall.expected`).
                        self.code = next_word;
                        self.stat_vis = true;
                        self.vps = VifVps::Decoding;
                        self.vis_armed = false;
                        break;
                    }

                    let cmd_raw = self.pop_work_word().unwrap_or(next_word);
                    self.code = cmd_raw;
                    self.refill_work_words();

                    let cmd_id = ((cmd_raw >> 24) & 0x7F) as u8;
                    let raw_num = ((cmd_raw >> 16) & 0xFF) as u8;
                    let imm = (cmd_raw & 0xFFFF) as u16;
                    let i_bit = (cmd_raw & 0x8000_0000) != 0 && !self.err_mii;

                    let finished_immediate = self.decode_command(
                        cmd_raw,
                        cmd_id,
                        raw_num,
                        imm,
                        i_bit,
                        micro_mask,
                        gif.as_deref_mut(),
                    );
                    if finished_immediate && i_bit && !self.stat_er1 && !self.stat_vfs {
                        self.stat_int = true;
                        if cmd_id != 0x07 {
                            self.vis_armed = true;
                        }
                        raised_irq = true;
                    }
                }

                VifCmdState::WaitFlush {
                    wait_gif_path12,
                    check_gif_path3,
                    pending_irq,
                } => {
                    let gif_busy = if self.kind.is_vif1() {
                        gif.as_deref().is_some_and(|g| {
                            (check_gif_path3 && (g.is_any_path_busy() || g.is_paused()))
                                || (wait_gif_path12
                                    && (g.is_path1_or_path2_busy() || g.is_paused()))
                        })
                    } else {
                        false
                    };
                    if vu.running {
                        self.stat_vew = true;
                        self.stat_vgw = gif_busy;
                        self.vps = VifVps::Transferring;
                        self.cmd_state = VifCmdState::WaitFlush {
                            wait_gif_path12,
                            check_gif_path3,
                            pending_irq,
                        };
                        break;
                    }
                    self.stat_vew = false;
                    if gif_busy {
                        self.stat_vgw = true;
                        self.vps = VifVps::Transferring;
                        self.cmd_state = VifCmdState::WaitFlush {
                            wait_gif_path12,
                            check_gif_path3,
                            pending_irq,
                        };
                        break;
                    }
                    self.stat_vgw = false;
                    self.finish_command(pending_irq, &mut raised_irq);
                }

                VifCmdState::WaitMscal {
                    target_tpc,
                    wait_gif,
                    pending_irq,
                } => {
                    let gif_busy = if wait_gif && self.kind.is_vif1() {
                        gif.as_deref()
                            .is_some_and(|g| g.is_path1_or_path2_busy() || g.is_paused())
                    } else {
                        false
                    };
                    if vu.running || gif_busy {
                        self.stat_vew = vu.running;
                        self.stat_vgw = gif_busy;
                        self.vps = VifVps::Transferring;
                        self.cmd_state = VifCmdState::WaitMscal {
                            target_tpc,
                            wait_gif,
                            pending_irq,
                        };
                        break;
                    }
                    self.stat_vew = false;
                    self.stat_vgw = false;
                    self.latch_and_start_vu(vu, target_tpc);
                    self.finish_command(pending_irq, &mut raised_irq);
                }

                VifCmdState::WaitMpgVu {
                    remaining_words,
                    byte_offset,
                    pending_irq,
                } => {
                    if vu.running {
                        self.stat_vew = true;
                        self.vps = VifVps::Transferring;
                        self.cmd_state = VifCmdState::WaitMpgVu {
                            remaining_words,
                            byte_offset,
                            pending_irq,
                        };
                        break;
                    }
                    self.stat_vew = false;
                    self.cmd_state = VifCmdState::Mpg {
                        remaining_words,
                        byte_offset,
                        pending_irq,
                    };
                }

                VifCmdState::Stmask { pending_irq } => {
                    if let Some(w) = self.pop_work_word() {
                        self.mask = w;
                        self.finish_command(pending_irq, &mut raised_irq);
                    } else {
                        self.vps = VifVps::DataWait;
                        self.cmd_state = VifCmdState::Stmask { pending_irq };
                        break;
                    }
                }

                VifCmdState::Strow {
                    mut idx,
                    pending_irq,
                } => {
                    while idx < 4 {
                        let Some(w) = self.pop_work_word() else {
                            break;
                        };
                        self.row[idx as usize] = w;
                        idx += 1;
                    }
                    if idx == 4 {
                        self.finish_command(pending_irq, &mut raised_irq);
                    } else {
                        self.vps = VifVps::DataWait;
                        self.cmd_state = VifCmdState::Strow { idx, pending_irq };
                        break;
                    }
                }

                VifCmdState::Stcol {
                    mut idx,
                    pending_irq,
                } => {
                    while idx < 4 {
                        let Some(w) = self.pop_work_word() else {
                            break;
                        };
                        self.col[idx as usize] = w;
                        idx += 1;
                    }
                    if idx == 4 {
                        self.finish_command(pending_irq, &mut raised_irq);
                    } else {
                        self.vps = VifVps::DataWait;
                        self.cmd_state = VifCmdState::Stcol { idx, pending_irq };
                        break;
                    }
                }

                VifCmdState::Mpg {
                    mut remaining_words,
                    mut byte_offset,
                    pending_irq,
                } => {
                    while remaining_words > 0 {
                        let Some(word) = self.pop_work_word() else {
                            break;
                        };
                        let off = (byte_offset as usize) & micro_mask;
                        if off + 4 <= vu_micro_mem.len() {
                            vu_micro_mem[off..off + 4].copy_from_slice(&word.to_le_bytes());
                        }
                        byte_offset = ((off + 4) & micro_mask) as u16;
                        remaining_words -= 1;
                    }
                    if remaining_words == 0 {
                        self.finish_command(pending_irq, &mut raised_irq);
                    } else {
                        self.vps = VifVps::DataWait;
                        self.cmd_state = VifCmdState::Mpg {
                            remaining_words,
                            byte_offset,
                            pending_irq,
                        };
                        break;
                    }
                }

                VifCmdState::Direct {
                    mut remaining_qwc,
                    is_hl,
                    mut started,
                    mut words,
                    mut word_idx,
                    pending_irq,
                } => {
                    let mut blocked_gif = false;
                    let mut blocked_data = false;

                    if let Some(gif_ref) = gif.as_deref_mut() {
                        if !gif_ref.can_accept_vif1_direct(is_hl) {
                            blocked_gif = true;
                        } else {
                            if !started {
                                gif_ref.begin_vif1_direct(remaining_qwc, is_hl);
                                started = true;
                            }
                            while remaining_qwc > 0 {
                                if !gif_ref.can_accept_vif1_direct(is_hl) {
                                    blocked_gif = true;
                                    break;
                                }
                                while word_idx < 4 {
                                    let Some(w) = self.pop_work_word() else {
                                        blocked_data = true;
                                        break;
                                    };
                                    words[word_idx as usize] = w;
                                    word_idx += 1;
                                }
                                if blocked_data {
                                    break;
                                }
                                gif_ref.push_vif1_direct_qwc(Reg128::from_u32x4(
                                    words[0], words[1], words[2], words[3],
                                ));
                                word_idx = 0;
                                remaining_qwc -= 1;
                            }
                            if remaining_qwc == 0 {
                                gif_ref.finish_vif1_direct();
                            }
                        }
                    }

                    if remaining_qwc == 0 {
                        self.stat_vgw = false;
                        self.finish_command(pending_irq, &mut raised_irq);
                    } else {
                        self.stat_vgw = blocked_gif;
                        self.vps = if blocked_gif {
                            VifVps::Transferring
                        } else {
                            VifVps::DataWait
                        };
                        self.cmd_state = VifCmdState::Direct {
                            remaining_qwc,
                            is_hl,
                            started,
                            words,
                            word_idx,
                            pending_irq,
                        };
                        break;
                    }
                }

                VifCmdState::Unpack { pending_irq } => {
                    loop {
                        while self.unpack.needs_input_word() {
                            let Some(w) = self.pop_work_word() else {
                                break;
                            };
                            self.unpack.push_word(w);
                        }
                        if self.unpack.step(
                            vu_data_mem,
                            data_qwc_mask,
                            self.mode,
                            self.mask,
                            &mut self.row,
                            &self.col,
                        ) {
                            self.finish_command(pending_irq, &mut raised_irq);
                            break;
                        }
                        // Needs more words; check if staging buffer just refilled or is empty.
                        self.refill_work_words();
                        if self.peek_work_word().is_none() {
                            self.vps = VifVps::DataWait;
                            self.cmd_state = VifCmdState::Unpack { pending_irq };
                            return raised_irq;
                        }
                    }
                }
            }
        }

        raised_irq
    }

    #[inline]
    fn finish_command(&mut self, pending_irq: bool, raised_irq: &mut bool) {
        self.cmd_state = VifCmdState::Idle;
        self.vps = VifVps::Idle;
        if pending_irq {
            self.stat_int = true;
            self.vis_armed = true;
            *raised_irq = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_command(
        &mut self,
        cmd_raw: u32,
        cmd_id: u8,
        raw_num: u8,
        imm: u16,
        i_bit: bool,
        micro_mask: usize,
        gif: Option<&mut GifUnit>,
    ) -> bool {
        match cmd_id {
            0x00 => true, // `NOP`
            0x01 => {
                self.cycle = imm;
                true
            }
            0x02 => {
                if !self.kind.is_vif1() {
                    self.flag_illegal_vifcode(cmd_raw);
                    return false;
                }
                self.ofst = imm & 0x03FF;
                self.dbf = false;
                self.tops = self.base;
                true
            }
            0x03 => {
                if !self.kind.is_vif1() {
                    self.flag_illegal_vifcode(cmd_raw);
                    return false;
                }
                self.base = imm & 0x03FF;
                true
            }
            0x04 => {
                self.itops = imm & 0x03FF;
                true
            }
            0x05 => {
                self.mode = (imm & 0x03) as u8;
                true
            }
            0x06 => {
                if !self.kind.is_vif1() {
                    self.flag_illegal_vifcode(cmd_raw);
                    return false;
                }
                if let Some(gif_ref) = gif {
                    gif_ref.m3p_vif1 = (imm & 0x8000) != 0;
                }
                true
            }
            0x07 => {
                self.mark = imm;
                self.stat_mrk = true;
                true
            }
            0x10 => {
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::WaitFlush {
                    wait_gif_path12: false,
                    check_gif_path3: false,
                    pending_irq: i_bit,
                };
                false
            }
            0x11 | 0x13 => {
                if !self.kind.is_vif1() {
                    self.flag_illegal_vifcode(cmd_raw);
                    return false;
                }
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::WaitFlush {
                    wait_gif_path12: true,
                    check_gif_path3: cmd_id == 0x13,
                    pending_irq: i_bit,
                };
                false
            }
            0x14 | 0x15 => {
                let target_tpc = imm & self.kind.tpc_mask();
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::WaitMscal {
                    target_tpc: Some(target_tpc),
                    wait_gif: cmd_id == 0x15,
                    pending_irq: i_bit,
                };
                false
            }
            0x17 => {
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::WaitMscal {
                    target_tpc: None,
                    wait_gif: false,
                    pending_irq: i_bit,
                };
                false
            }
            0x20 => {
                self.vps = VifVps::Decoding;
                self.cmd_state = VifCmdState::Stmask { pending_irq: i_bit };
                false
            }
            0x30 => {
                self.vps = VifVps::Decoding;
                self.cmd_state = VifCmdState::Strow {
                    idx: 0,
                    pending_irq: i_bit,
                };
                false
            }
            0x31 => {
                self.vps = VifVps::Decoding;
                self.cmd_state = VifCmdState::Stcol {
                    idx: 0,
                    pending_irq: i_bit,
                };
                false
            }
            0x4A => {
                let units = if raw_num == 0 {
                    256u16
                } else {
                    u16::from(raw_num)
                };
                let byte_offset = (((imm as usize) * 8) & micro_mask) as u16;
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::WaitMpgVu {
                    remaining_words: units * 2,
                    byte_offset,
                    pending_irq: i_bit,
                };
                false
            }
            0x50 | 0x51 => {
                if !self.kind.is_vif1() {
                    self.flag_illegal_vifcode(cmd_raw);
                    return false;
                }
                let qwc_count = if imm == 0 { 65_536 } else { u32::from(imm) };
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::Direct {
                    remaining_qwc: qwc_count,
                    is_hl: cmd_id == 0x51,
                    started: false,
                    words: [0; 4],
                    word_idx: 0,
                    pending_irq: i_bit,
                };
                false
            }
            0x60..=0x7F => {
                let cmd = UnpackCmd::decode(cmd_raw);
                if !UnpackCmd::is_valid_format(cmd.vn, cmd.vl) {
                    self.flag_illegal_vifcode(cmd_raw);
                    return false;
                }
                self.unpack
                    .begin(cmd, self.cycle, self.tops, self.kind.is_vif1());
                self.vps = VifVps::Transferring;
                self.cmd_state = VifCmdState::Unpack { pending_irq: i_bit };
                false
            }
            _ => {
                self.flag_illegal_vifcode(cmd_raw);
                false
            }
        }
    }

    fn flag_illegal_vifcode(&mut self, cmd_raw: u32) {
        let idx = usize::from(self.kind.is_vif1());
        tracing::warn!(
            "VIF{}: unrecognized or invalid VIFcode 0x{:08X}",
            idx,
            cmd_raw
        );
        if !self.err_me1 {
            self.stat_er1 = true;
        }
    }

    fn latch_and_start_vu(&mut self, vu: &mut VuState, target_tpc: Option<u16>) {
        self.itop = self.itops;
        if self.kind.is_vif1() {
            self.top = self.tops;
            self.dbf = !self.dbf;
            self.tops = if self.dbf {
                self.base.wrapping_add(self.ofst) & 0x03FF
            } else {
                self.base
            };
        }
        let start_tpc = target_tpc.unwrap_or(vu.tpc);
        vu.start_micro(start_tpc, self.kind.tpc_mask());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vu::micro::UPPER_BIT_E;

    fn run_vif_qwc(
        kind: VifKind,
        qwcs: &[Reg128],
    ) -> (VifUnit, VuState, Vec<u8>, Vec<u8>, GifUnit, bool) {
        let mut vif = VifUnit::new(kind);
        let mut vu = VuState::new();
        let micro_len = if kind.is_vif1() { 0x4000 } else { 0x1000 };
        let data_len = if kind.is_vif1() { 0x4000 } else { 0x1000 };
        let mut micro = vec![0u8; micro_len];
        let mut data = vec![0u8; data_len];
        let mut gif = GifUnit::new();
        let mut any_irq = false;

        for &qwc in qwcs {
            assert!(vif.push_dma_qwc(qwc));
            let irq = vif.step(
                &mut vu,
                &mut micro,
                &mut data,
                if kind.is_vif1() { Some(&mut gif) } else { None },
            );
            any_irq |= irq;
        }
        (vif, vu, micro, data, gif, any_irq)
    }

    fn read_vu_qwc(data: &[u8], qwc_idx: usize) -> [u32; 4] {
        let off = qwc_idx * 16;
        [
            u32::from_le_bytes(data[off..off + 4].try_into().unwrap()),
            u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()),
            u32::from_le_bytes(data[off + 8..off + 12].try_into().unwrap()),
            u32::from_le_bytes(data[off + 12..off + 16].try_into().unwrap()),
        ]
    }

    #[test]
    fn vif_mpg_0x4a_upload_wrap_and_odd_length() {
        // Test NUM=3 (odd: 3 LIW = 6 words = 24 bytes = 1.5 QWC), starting near end of VU0 4 KB micro mem
        // (`imm = 511` -> byte offset `511 * 8 = 4088`, wraps cleanly at `4096`).
        let mut vif = VifUnit::new(VifKind::Vif0);
        let mut vu = VuState::new();
        let mut micro = vec![0u8; 0x1000];
        let mut data = vec![0u8; 0x1000];

        // Pack: [MPG(imm=511, num=3), w0, w1, w2] then [w3, w4, w5, MARK(0xBEEF)]
        let mpg = (0x4Au32 << 24) | (3u32 << 16) | 511u32;
        let mark = (0x07u32 << 24) | 0xBEEFu32;

        // First QWC starts MPG (leaves 3 words remaining -> VIF0_NUM == 2)
        assert!(vif.push_dma_qwc(Reg128::from_u32x4(
            mpg,
            0x1111_1111,
            0x2222_2222,
            0x3333_3333
        )));
        vif.step(&mut vu, &mut micro, &mut data, None);
        assert_eq!(vif.read32(0x1000_3860), 2);
        assert_eq!(vif.read32(0x1000_3800) & 0x3, 1); // `VPS = DataWait`

        // Second QWC completes MPG (wrapping into byte 0..16) and executes trailing `MARK`
        assert!(vif.push_dma_qwc(Reg128::from_u32x4(
            0x4444_4444,
            0x5555_5555,
            0x6666_6666,
            mark
        )));
        vif.step(&mut vu, &mut micro, &mut data, None);
        assert_eq!(vif.read32(0x1000_3860), 0);
        assert_eq!(vif.read32(0x1000_3830), 0xBEEF);

        assert_eq!(
            u32::from_le_bytes(micro[4088..4092].try_into().unwrap()),
            0x1111_1111
        );
        assert_eq!(
            u32::from_le_bytes(micro[4092..4096].try_into().unwrap()),
            0x2222_2222
        );
        assert_eq!(
            u32::from_le_bytes(micro[0..4].try_into().unwrap()),
            0x3333_3333
        );
        assert_eq!(
            u32::from_le_bytes(micro[4..8].try_into().unwrap()),
            0x4444_4444
        );
        assert_eq!(
            u32::from_le_bytes(micro[8..12].try_into().unwrap()),
            0x5555_5555
        );
        assert_eq!(
            u32::from_le_bytes(micro[12..16].try_into().unwrap()),
            0x6666_6666
        );
    }

    #[test]
    fn vif_mpg_num_zero_uploads_2048_bytes_and_respects_vew_stall() {
        let mut vif = VifUnit::new(VifKind::Vif0);
        let mut vu = VuState::new();
        let mut micro = vec![0u8; 0x1000];
        let mut data = vec![0u8; 0x1000];

        vu.running = true;
        // `MPG(imm=0, num=0)` -> 256 LIWs = 512 words = 2048 bytes (`128` QWC of payload).
        let mpg = 0x4Au32 << 24;
        assert!(vif.push_dma_qwc(Reg128::from_u32x4(mpg, 0xAABB_CCDD, 0x1234_5678, 0)));
        vif.step(&mut vu, &mut micro, &mut data, None);
        // Must stall on `VEW` without overwriting micro[0..12] while VU runs.
        assert!(vif.stat_vew);
        assert_eq!(u32::from_le_bytes(micro[0..4].try_into().unwrap()), 0);
        assert_eq!(vif.read32(0x1000_3860), 0); // `VIF0_NUM` wraps `256 & 0xFF = 0`

        // When VU halts, `MPG` consumes the already-staged 3 words and expects the rest.
        vu.running = false;
        vif.step(&mut vu, &mut micro, &mut data, None);
        assert!(!vif.stat_vew);
        assert_eq!(
            u32::from_le_bytes(micro[0..4].try_into().unwrap()),
            0xAABB_CCDD
        );
        assert_eq!(
            u32::from_le_bytes(micro[4..8].try_into().unwrap()),
            0x1234_5678
        );
        assert_eq!(u32::from_le_bytes(micro[8..12].try_into().unwrap()), 0);

        for idx in 1..128 {
            let base = (idx as u32) * 4;
            assert!(vif.push_dma_qwc(Reg128::from_u32x4(base, base + 1, base + 2, base + 3)));
            vif.step(&mut vu, &mut micro, &mut data, None);
        }
        assert_eq!(vif.read32(0x1000_3860), 1);
        assert_eq!(vif.read32(0x1000_3800) & 0x3, 1); // 1 word (`NOP`/pad) still needed (`1 + 512` = 513 words)
        assert!(vif.push_cpu_fifo_word(0x9999_8888));
        vif.step(&mut vu, &mut micro, &mut data, None);
        assert!(vif.is_idle());
        assert_eq!(
            u32::from_le_bytes(micro[2044..2048].try_into().unwrap()),
            0x9999_8888
        );
    }

    #[test]
    fn vif_unresolved_0x46_flags_er1_instead_of_guessing_mpg() {
        let cmd_46 = 0x46u32 << 24;
        let (vif, _vu, _micro, _data, _gif, _irq) =
            run_vif_qwc(VifKind::Vif0, &[Reg128::from_u32x4(cmd_46, 0, 0, 0)]);
        assert!(vif.stat_er1);
        assert_eq!(vif.code, cmd_46);
    }

    #[test]
    fn vif_unpack_formats_sign_v2_v3_v4_5_and_word_padding() {
        // Test S-32, V2-16 (duplication XYXY), V3-8 (W=0), V4-5 (0xFFFF -> f8,f8,f8,80), and padding
        let stcycl = (0x01u32 << 24) | 0x0101;
        let s32 = (0x60u32 << 24) | (1 << 16); // NUM=1 at qwc 0
        let v2_16_signed = (0x65u32 << 24) | (1 << 16) | 1; // V2-16 signed at qwc 1
        let v3_8_unsigned = (0x6Au32 << 24) | (1 << 16) | 0x4000 | 2; // V3-8 unsigned at qwc 2 (reads 1 word, drops byte 3)
        let v4_5 = (0x6Fu32 << 24) | (1 << 16) | 3; // V4-5 at qwc 3

        let q0 = Reg128::from_u32x4(stcycl, s32, 0xCAFE_BABE, v2_16_signed);
        let q1 = Reg128::from_u32x4(0xFFFF_0007, v3_8_unsigned, 0xAA_03_02_01, v4_5);
        let q2 = Reg128::from_u32x4(0x0000_FFFF, 0, 0, 0);

        let (_vif, _vu, _micro, data, _gif, _irq) = run_vif_qwc(VifKind::Vif1, &[q0, q1, q2]);
        assert_eq!(read_vu_qwc(&data, 0), [0xCAFE_BABE; 4]);
        assert_eq!(read_vu_qwc(&data, 1), [7, 0xFFFF_FFFF, 7, 0xFFFF_FFFF]);
        assert_eq!(read_vu_qwc(&data, 2), [1, 2, 3, 0]);
        assert_eq!(read_vu_qwc(&data, 3), [0xF8, 0xF8, 0xF8, 0x80]);
    }

    #[test]
    fn vif_stcycl_wrap_skip_fill_and_wl_zero() {
        let mut vif = VifUnit::new(VifKind::Vif0);
        let mut vu = VuState::new();
        let mut micro = vec![0u8; 0x1000];
        let mut data = vec![0u8; 0x1000];

        // Set STROW = [10,20,30,40], STCOL = [1,2,3,4], STCYCL(CL=2, WL=0), UNPACK(addr=254, NUM=4)
        // Must wrap 254, 255, 0, 1 in VU0 256-QWC memory while consuming 0 payload words.
        let q0 = Reg128::from_u32x4(0x30 << 24, 10, 20, 30);
        let q1 = Reg128::from_u32x4(40, (0x01 << 24) | 0x0002, (0x6C << 24) | (4 << 16) | 254, 0);
        assert!(vif.push_dma_qwc(q0));
        assert!(vif.push_dma_qwc(q1));
        vif.step(&mut vu, &mut micro, &mut data, None);

        assert_eq!(read_vu_qwc(&data, 254), [10, 20, 30, 40]);
        assert_eq!(read_vu_qwc(&data, 255), [10, 20, 30, 40]);
        assert_eq!(read_vu_qwc(&data, 0), [10, 20, 30, 40]);
        assert_eq!(read_vu_qwc(&data, 1), [10, 20, 30, 40]);

        // Also test Skipping write CL=2, WL=1 starting at qwc 10, NUM=2 (writes qwc 10 and 12)
        let q2 = Reg128::from_u32x4(
            (0x01 << 24) | 0x0102,
            (0x60 << 24) | (2 << 16) | 10,
            0xAA,
            0xBB,
        );
        assert!(vif.push_dma_qwc(q2));
        vif.step(&mut vu, &mut micro, &mut data, None);
        assert_eq!(read_vu_qwc(&data, 10), [0xAA; 4]);
        assert_eq!(read_vu_qwc(&data, 12), [0xBB; 4]);
    }

    #[test]
    fn vif_stmod_and_stmask_modes_0_to_3() {
        let mut vif = VifUnit::new(VifKind::Vif0);
        let mut vu = VuState::new();
        let mut micro = vec![0u8; 0x1000];
        let mut data = vec![0u8; 0x1000];

        // Configure STROW = 0x1000 across all 4 lanes, STCYCL = 0x0101, MODE = 2 (Difference),
        // MASK = 0x0000_0004 (col0 lane1 = 01 ROW, others = 00 Data), V4-32 masked at qwc 0, NUM=1.
        let q0 = Reg128::from_u32x4(0x30 << 24, 0x1000, 0x1000, 0x1000);
        let q1 = Reg128::from_u32x4(0x1000, (0x01 << 24) | 0x0101, (0x05 << 24) | 2, 0x20 << 24);
        let q2 = Reg128::from_u32x4(0b00_00_01_00, (0x7C << 24) | (1 << 16), 1, 2);
        let q3 = Reg128::from_u32x4(3, 4, (0x05 << 24) | 3, (0x7C << 24) | (1 << 16) | 1);
        let q4 = Reg128::from_u32x4(10, 20, 30, 40);

        for q in [q0, q1, q2, q3, q4] {
            assert!(vif.push_dma_qwc(q));
            vif.step(&mut vu, &mut micro, &mut data, None);
        }

        // Mode 2 (Difference) + lane1 ROW-masked: qwc 0 = [0x1001, 0x1000, 0x1003, 0x1004], row updated on 0/2/3 only.
        assert_eq!(read_vu_qwc(&data, 0), [0x1001, 0x1000, 0x1003, 0x1004]);
        // Mode 3 writes input direct to BOTH vumem & ROW on unmasked lanes, preserves lane1 as old ROW (0x1000).
        assert_eq!(read_vu_qwc(&data, 1), [10, 0x1000, 30, 40]);
        assert_eq!(vif.row, [10, 0x1000, 30, 40]);
    }

    #[test]
    fn vif_intstall_vis_and_mark_i_behavior() {
        // Mirror `intstall.expected`: 3-QWC DMA [NOP(I), STMASK(I), 0xFFFFFFFF, NOP...]
        let mut vif = VifUnit::new(VifKind::Vif0);
        let mut vu = VuState::new();
        let mut micro = vec![0u8; 0x1000];
        let mut data = vec![0u8; 0x1000];

        let nop_i = 0x8000_0000u32;
        let stmask_i = 0x8000_0000u32 | (0x20u32 << 24);
        let mark_i = 0x8000_0000u32 | (0x07u32 << 24) | 0x1234;

        let q0 = Reg128::from_u32x4(nop_i, stmask_i, 0xFFFF_FFFF, 0);
        let q1 = Reg128::from_u32x4(0, 0, 0, 0);
        let q2 = Reg128::from_u32x4(0, 0, 0, 0);

        assert!(vif.push_dma_qwc(q0));
        assert!(vif.push_dma_qwc(q1));
        assert!(vif.push_dma_qwc(q2));
        let irq = vif.step(&mut vu, &mut micro, &mut data, None);
        assert!(irq);
        // `STAT = 0x0000_0c02` (`INT | VIS | VPS=2`) and `FQC == 0`!
        assert_eq!(vif.read32(0x1000_3800), 0x0000_0C02);
        assert_eq!(vif.code, stmask_i);
        assert_eq!(vif.mask, 0);

        // `FBRST.STC` clears stall; `STMASK(I)` resumes, updates `MASK`, and raises `INT`.
        vif.write32(0x1000_3810, 0x08);
        let irq2 = vif.step(&mut vu, &mut micro, &mut data, None);
        assert!(irq2);
        assert_eq!(vif.mask, 0xFFFF_FFFF);
        assert!(vif.stat_int);

        // `MARK(I)` must NOT arm `VIS` on subsequent commands.
        vif.write32(0x1000_3810, 0x08);
        assert!(vif.push_dma_qwc(Reg128::from_u32x4(mark_i, (0x01 << 24) | 0x0203, 0, 0)));
        let irq3 = vif.step(&mut vu, &mut micro, &mut data, None);
        assert!(irq3);
        assert_eq!(vif.read32(0x1000_3800) & 0x0C40, 0x0840); // `INT | MRK`, `VIS == 0`!
        assert_eq!(vif.cycle, 0x0203); // `STCYCL` after `MARK(I)` executed cleanly!
    }

    #[test]
    fn vif1_double_buffering_flg_direct_mskpath3_and_fbrst() {
        let mut vif = VifUnit::new(VifKind::Vif1);
        let mut vu = VuState::new();
        let mut micro = vec![0u8; 0x4000];
        let mut data = vec![0u8; 0x4000];
        let mut gif = GifUnit::new();

        // Write simple VU1 halt microprogram at TPC 0 (`E`-bit at slot 0 -> delay slot 1)
        micro[4..8].copy_from_slice(&UPPER_BIT_E.to_le_bytes());

        let base_cmd = (0x03u32 << 24) | 100;
        let ofst_cmd = (0x02u32 << 24) | 50; // `DBF = false`, `TOPS = 100`
        let itop_cmd = (0x04u32 << 24) | 77;
        let mskpath3_cmd = (0x06u32 << 24) | 0x8000;
        let stcycl_cmd = (0x01u32 << 24) | 0x0101;
        let unpack_flg = (0x60u32 << 24) | (1 << 16) | 0x8000 | 5; // `addr = 5 + TOPS(100) = 105`
        let mscal_cmd = 0x14u32 << 24;
        let direct_cmd = (0x50u32 << 24) | 1;

        for q in [
            Reg128::from_u32x4(base_cmd, ofst_cmd, itop_cmd, mskpath3_cmd),
            Reg128::from_u32x4(stcycl_cmd, unpack_flg, 0xDEADBEEF, mscal_cmd),
            Reg128::from_u32x4(direct_cmd, 1, 2, 3),
            Reg128::from_u32x4(4, 0, 0, 0),
        ] {
            assert!(vif.push_dma_qwc(q));
            vif.step(&mut vu, &mut micro, &mut data, Some(&mut gif));
        }

        // First MSCAL latched `TOP = 100`, toggled `DBF = true`, `TOPS = 150`, `ITOP = 77`, started VU1.
        assert_eq!(vif.top, 100);
        assert!(vif.dbf);
        assert_eq!(vif.tops, 150);
        assert_eq!(vif.itop, 77);
        assert!(gif.m3p_vif1);
        assert_eq!(read_vu_qwc(&data, 105), [0xDEADBEEF; 4]);
        assert!(vu.running);

        // Next DIRECT (`qwc=1`) is after MSCAL: since GIF is active right after, verify packet arrived!
        assert_eq!(gif.total_packets, 1);
        let meta = gif.last_packet().expect("DIRECT packet stored");
        assert_eq!(
            gif.read_packet_qwc(&meta, 0),
            Some(Reg128::from_u32x4(1, 2, 3, 4))
        );

        // Test FIFO capacity limits (`8` for VIF0, `16` for VIF1) and `FBRST.RST`.
        let mut vif0 = VifUnit::new(VifKind::Vif0);
        for _ in 0..8 {
            assert!(vif0.push_dma_qwc(Reg128::ZERO));
        }
        assert!(!vif0.can_accept_qwc());
        assert_eq!(vif0.fqc(), 8);
        vif0.write32(0x1000_3810, 0x01); // `RST`
        assert!(vif0.can_accept_qwc());
        assert_eq!(vif0.fqc(), 0);
    }
}
