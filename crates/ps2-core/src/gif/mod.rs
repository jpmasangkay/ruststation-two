//! Graphics Synthesizer Interface (`GIF`, `0x1000_3000..=0x1000_30A0` & `0x1000_6000..=0x1000_6FFF`).
//!
//! Primary source references (`local/ps2tek/index.html`):
//! - `#gif`, `#gifio`, `#giftags`, `#gifdataformats`, `#gifpath3masking`:
//!   * `PATH1` (`VU1 XGKICK`, deferred to Phase 2b; queue slot + arbitration modeled now) >
//!     `PATH2` (`VIF1 DIRECT` / `DIRECTHL`) > `PATH3` (`DMAC D2` / `0x1000_6000` 16-QWC FIFO).
//!   * `GIF_CTRL` (`0x1000_3000`, W): bit 0 `RST` (reset GIF), bit 3 `PSE` (pause).
//!   * `GIF_MODE` (`0x1000_3010`, W): bit 0 `M3R` (mask PATH3; deferred until ongoing DMA ends), bit 2 `IMT` (intermittent mode).
//!   * `GIF_STAT` (`0x1000_3020`, R): `0=M3R`, `1=M3P`, `2=IMT`, `3=PSE`, `5=IP3`, `6=P3Q`, `7=P2Q`,
//!     `8=P1Q`, `9=OPH`, `10..=11=APATH`, `12=DIR`, `24..=28=FQC` (`0..=16`).
//!   * `GIF_TAG0..3` (`0x1000_3040..=0x1000_3070`, R), `GIF_CNT` (`0x1000_3080`, R),
//!     `GIF_P3CNT` (`0x1000_3090`, R), `GIF_P3TAG` (`0x1000_30A0`, R): readable only when `GIF_CTRL.PSE == 1`.

use ps2_common::Reg128;

use crate::bus::gs_stub::{gs_reg, GsPrivilegedStub};
use crate::bus::intc::EeIntc;

/// Capacity of the `PATH3` hardware GIF FIFO (`16` quadwords per `ps2tek` `#gifpath3masking`).
pub const PATH3_FIFO_CAPACITY: usize = 16;

/// Pre-allocated `PATH2` input/staging FIFO capacity (quadwords).
const PATH2_QUEUE_CAPACITY: usize = 4096;

/// Pre-allocated `PATH1` queue capacity (quadwords; `XGKICK` arrives in Phase 2b).
const PATH1_QUEUE_CAPACITY: usize = 256;

/// Fixed number of captured `PATH2` packet metadata records for Step 3 diagnostics.
const CAPTURE_META_SLOTS: usize = 256;

/// Active GIF arbitration path index (`GIF_STAT.APATH`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GifPath {
    Path1 = 1,
    Path2 = 2,
    Path3 = 3,
}

impl GifPath {
    #[inline(always)]
    const fn cursor_index(self) -> usize {
        (self as usize) - 1
    }
}

/// GIFtag payload format (`GIFtag` bits `58..=59`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum GifDataFormat {
    #[default]
    Packed = 0,
    Reglist = 1,
    Image = 2,
}

/// Parsed 128-bit GIFtag (`ps2tek` `#giftags`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GifTag {
    pub raw_words: [u32; 4],
    pub nloop: u16,
    pub eop: bool,
    pub pre: bool,
    pub prim: u16,
    pub format: GifDataFormat,
    pub nregs: u8,
    pub regs: u64,
}

impl GifTag {
    /// Decode a 128-bit `Reg128` quadword into a [`GifTag`].
    #[inline]
    pub fn from_qwc(qwc: Reg128) -> Self {
        let lo = qwc.lo_u64();
        let hi = qwc.hi_u64();
        let raw_words = [lo as u32, (lo >> 32) as u32, hi as u32, (hi >> 32) as u32];
        let nloop = (lo & 0x7FFF) as u16;
        let eop = (lo & (1 << 15)) != 0;
        let pre = (lo & (1 << 46)) != 0;
        let prim = ((lo >> 47) & 0x7FF) as u16;
        let format = match (lo >> 58) & 0x3 {
            0 => GifDataFormat::Packed,
            1 => GifDataFormat::Reglist,
            _ => GifDataFormat::Image,
        };
        let raw_nregs = ((lo >> 60) & 0xF) as u8;
        let nregs = if raw_nregs == 0 { 16 } else { raw_nregs };
        Self {
            raw_words,
            nloop,
            eop,
            pre,
            prim,
            format,
            nregs,
            regs: hi,
        }
    }
}

/// Metadata for a captured `VIF1 DIRECT`/`DIRECTHL` (`PATH2`) transfer (Step 3 diagnostics).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GifPacketMeta {
    pub declared_qwc: u32,
    pub actual_qwc: u32,
    pub is_hl: bool,
    pub completed: bool,
    start_idx: usize,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct GifPathCursor {
    tag_loaded: bool,
    tag: GifTag,
    remaining_loops: u16,
    cur_reg_idx: u8,
    sub_qwc_half: u8,
}

impl GifPathCursor {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Pre-allocated fixed-capacity circular buffer of 128-bit quadwords (zero hot-path allocations).
#[derive(Clone, Debug, PartialEq, Eq)]
struct QwcFifo {
    buf: Box<[Reg128]>,
    head: usize,
    len: usize,
}

impl QwcFifo {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: vec![Reg128::ZERO; capacity].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    fn is_full(&self) -> bool {
        self.len >= self.buf.len()
    }

    #[inline]
    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    #[inline]
    fn push(&mut self, val: Reg128) -> bool {
        if self.is_full() {
            return false;
        }
        let tail = (self.head + self.len) % self.buf.len();
        self.buf[tail] = val;
        self.len += 1;
        true
    }

    #[inline]
    fn peek(&self) -> Option<Reg128> {
        if self.len == 0 {
            None
        } else {
            Some(self.buf[self.head])
        }
    }

    #[inline]
    fn pop(&mut self) -> Option<Reg128> {
        if self.len == 0 {
            return None;
        }
        let val = self.buf[self.head];
        self.head = (self.head + 1) % self.buf.len();
        self.len -= 1;
        Some(val)
    }
}

/// Full Phase 2a Graphics Synthesizer Interface (`GIF`) unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GifUnit {
    /// `GIF_CTRL.PSE` (bit 3): pause GIF transfers.
    paused: bool,
    /// `GIF_MODE.M3R` (bit 0): active `PATH3` mask from `GIF_MODE`.
    m3r: bool,
    /// Pending `M3R = 1` written while `D2` DMA or `PATH3` packet was ongoing (`ps2tek` `#gifio`).
    // TODO(verify): `ps2tek` notes "When PATH3 is masked by this register and GIF DMA is ongoing,
    // the mask applies once GIF DMA ends its transfer"; verify exact boundary (`D2` end vs packet `EOP`).
    pending_m3r: bool,
    /// Tracks whether `DMAC D2` (`PATH3`) currently has an ongoing DMA transfer.
    path3_dma_ongoing: bool,
    /// `GIF_STAT.M3P` (bit 1): `PATH3` masked by `VIF1 MSKPATH3`.
    pub m3p_vif1: bool,
    /// `GIF_MODE.IMT` (bit 2): intermittent mode (`PATH3` yields to `PATH1`/`PATH2`).
    imt: bool,
    /// `GIF_STAT.IP3` (bit 5): `PATH3` was interrupted and awaits resumption.
    ip3: bool,
    /// Currently arbitrated active transfer path (`APATH`).
    active_path: Option<GifPath>,
    /// Per-path GIFtag & loop cursors (`[Path1, Path2, Path3]`).
    cursors: [GifPathCursor; 3],
    /// Most recently parsed GIFtag raw words (`GIF_TAG0..3`).
    last_tag_words: [u32; 4],
    /// Internal `Q` register initialized to `1.0f` (`0x3F80_0000`) on `NLOOP > 0` GIFtag (`#giftags`).
    internal_q: u32,
    /// Pre-allocated input queues for `PATH1`, `PATH2`, and `PATH3` (`16` QWCs for `PATH3`).
    path1_queue: QwcFifo,
    path2_queue: QwcFifo,
    path3_queue: QwcFifo,
    /// Total `PATH2` (`VIF1 DIRECT`/`DIRECTHL`) packets received (for Step 3 regression tests).
    pub total_packets: u64,
    /// Pre-allocated capture buffer for `PATH2` packet inspection (Step 3 diagnostics).
    capture_buf: Box<[Reg128]>,
    capture_write_pos: usize,
    capture_meta: [GifPacketMeta; CAPTURE_META_SLOTS],
    capture_count: usize,
    capture_active: bool,
}

impl Default for GifUnit {
    fn default() -> Self {
        Self::new()
    }
}

impl GifUnit {
    /// Create a newly reset [`GifUnit`] with pre-allocated fixed buffers.
    pub fn new() -> Self {
        Self {
            paused: false,
            m3r: false,
            pending_m3r: false,
            path3_dma_ongoing: false,
            m3p_vif1: false,
            imt: false,
            ip3: false,
            active_path: None,
            cursors: [GifPathCursor::default(); 3],
            last_tag_words: [0; 4],
            internal_q: 0x3F80_0000,
            path1_queue: QwcFifo::with_capacity(PATH1_QUEUE_CAPACITY),
            path2_queue: QwcFifo::with_capacity(PATH2_QUEUE_CAPACITY),
            path3_queue: QwcFifo::with_capacity(PATH3_FIFO_CAPACITY),
            total_packets: 0,
            capture_buf: vec![Reg128::ZERO; PATH2_QUEUE_CAPACITY].into_boxed_slice(),
            capture_write_pos: 0,
            capture_meta: [GifPacketMeta::default(); CAPTURE_META_SLOTS],
            capture_count: 0,
            capture_active: false,
        }
    }

    /// Reset all GIF queues, cursors, mode/control bits, and active path state (`GIF_CTRL.RST`).
    // TODO(verify): verify on real hardware whether `GIF_CTRL.RST` clears `M3P` (`VIF1 MSKPATH3`).
    pub fn reset(&mut self) {
        self.paused = false;
        self.m3r = false;
        self.pending_m3r = false;
        self.path3_dma_ongoing = false;
        self.m3p_vif1 = false;
        self.imt = false;
        self.ip3 = false;
        self.active_path = None;
        for cursor in &mut self.cursors {
            cursor.reset();
        }
        self.last_tag_words = [0; 4];
        self.internal_q = 0x3F80_0000;
        self.path1_queue.clear();
        self.path2_queue.clear();
        self.path3_queue.clear();
        self.capture_write_pos = 0;
        self.capture_count = 0;
        self.capture_active = false;
    }

    /// Return `true` when `GIF_CTRL.PSE` (pause) is set.
    #[inline(always)]
    pub const fn is_paused(&self) -> bool {
        self.paused
    }

    /// Return `true` if `PATH3` is currently executing an `IMAGE` (`FLG=2|3`) primitive.
    #[inline]
    pub fn path3_image_active(&self) -> bool {
        self.active_path == Some(GifPath::Path3)
            && self.cursors[GifPath::Path3.cursor_index()].tag_loaded
            && self.cursors[GifPath::Path3.cursor_index()].tag.format == GifDataFormat::Image
    }

    /// Return `true` if `PATH1` or `PATH2` has active or queued work (`VIF1 FLUSH` / `MSCALF`).
    #[inline]
    pub fn is_path1_or_path2_busy(&self) -> bool {
        !self.path1_queue.is_empty()
            || !self.path2_queue.is_empty()
            || matches!(self.active_path, Some(GifPath::Path1 | GifPath::Path2))
    }

    /// Return `true` if any GIF path has active, interrupted, or queued work (`VIF1 FLUSHA`).
    #[inline]
    pub fn is_any_path_busy(&self) -> bool {
        self.is_path1_or_path2_busy()
            || !self.path3_queue.is_empty()
            || self.active_path.is_some()
            || self.ip3
    }

    /// Return the current number of quadwords buffered in the `PATH3` 16-QWC FIFO.
    #[inline(always)]
    pub fn path3_fifo_len(&self) -> usize {
        self.path3_queue.len()
    }

    /// Return `true` if the `PATH3` 16-QWC FIFO can accept another quadword (`DMAC D2` backpressure).
    #[inline(always)]
    pub fn can_accept_path3_qwc(&self) -> bool {
        !self.path3_queue.is_full()
    }

    /// Push one 128-bit quadword into the `PATH3` 16-QWC FIFO (`DMAC D2` or `0x1000_6000`).
    #[inline]
    pub fn push_path3_qwc(&mut self, qwc: Reg128) -> bool {
        self.path3_queue.push(qwc)
    }

    /// Push one 128-bit quadword into the `PATH1` queue (Phase 2a unit tests; `XGKICK` in Phase 2b).
    #[inline]
    pub fn push_path1_qwc(&mut self, qwc: Reg128) -> bool {
        self.path1_queue.push(qwc)
    }

    /// Called when `DMAC D2` starts so `GIF_MODE.M3R` defers until DMA end.
    #[inline]
    pub fn on_path3_dma_start(&mut self) {
        self.path3_dma_ongoing = true;
    }

    /// Called when `DMAC D2` completes so a deferred `GIF_MODE.M3R` mask takes effect.
    #[inline]
    pub fn on_path3_dma_end(&mut self) {
        self.path3_dma_ongoing = false;
        if self.pending_m3r && self.active_path != Some(GifPath::Path3) {
            self.m3r = true;
            self.pending_m3r = false;
        }
    }

    /// Return `true` when `VIF1 DIRECT` (`is_hl = false`) or `DIRECTHL` (`is_hl = true`) can push data.
    #[inline]
    pub fn can_accept_vif1_direct(&self, is_hl: bool) -> bool {
        if self.paused || self.path2_queue.is_full() || self.path3_image_active() {
            return false;
        }
        if !is_hl && self.active_path == Some(GifPath::Path3) && !self.imt {
            return false;
        }
        true
    }

    /// Begin capturing and queuing a `VIF1 DIRECT`/`DIRECTHL` (`PATH2`) transfer.
    pub fn begin_vif1_direct(&mut self, declared_qwc: u32, is_hl: bool) {
        if self.capture_active {
            self.finish_vif1_direct();
        }
        let slot = (self.total_packets as usize) % CAPTURE_META_SLOTS;
        self.capture_meta[slot] = GifPacketMeta {
            declared_qwc,
            actual_qwc: 0,
            is_hl,
            completed: false,
            start_idx: self.capture_write_pos,
        };
        self.total_packets += 1;
        if self.capture_count < CAPTURE_META_SLOTS {
            self.capture_count += 1;
        }
        self.capture_active = true;
    }

    /// Push one payload quadword of an active `VIF1 DIRECT`/`DIRECTHL` packet into `PATH2`.
    pub fn push_vif1_direct_qwc(&mut self, qwc: Reg128) {
        if !self.path2_queue.push(qwc) {
            tracing::warn!("GIF PATH2 queue full; dropping DIRECT quadword");
        }
        if self.capture_active {
            let cap = self.capture_buf.len();
            self.capture_buf[self.capture_write_pos % cap] = qwc;
            self.capture_write_pos = (self.capture_write_pos + 1) % cap;
            let slot = ((self.total_packets - 1) as usize) % CAPTURE_META_SLOTS;
            self.capture_meta[slot].actual_qwc =
                self.capture_meta[slot].actual_qwc.saturating_add(1);
        }
    }

    /// Mark the active `VIF1 DIRECT`/`DIRECTHL` transfer complete in diagnostic capture metadata.
    pub fn finish_vif1_direct(&mut self) {
        if self.capture_active && self.total_packets > 0 {
            let slot = ((self.total_packets - 1) as usize) % CAPTURE_META_SLOTS;
            self.capture_meta[slot].completed = true;
            self.capture_active = false;
        }
    }

    /// Return the most recent `VIF1` `DIRECT`/`DIRECTHL` packet metadata (Step 3 diagnostics).
    pub fn last_packet(&self) -> Option<GifPacketMeta> {
        if self.capture_count == 0 {
            None
        } else {
            let slot = ((self.total_packets - 1) as usize) % CAPTURE_META_SLOTS;
            Some(self.capture_meta[slot])
        }
    }

    /// Read quadword `qwc_idx` from a captured `PATH2` packet (Step 3 diagnostics).
    pub fn read_packet_qwc(&self, meta: &GifPacketMeta, qwc_idx: usize) -> Option<Reg128> {
        if qwc_idx >= (meta.actual_qwc as usize) {
            return None;
        }
        let cap = self.capture_buf.len();
        Some(self.capture_buf[(meta.start_idx + qwc_idx) % cap])
    }

    #[inline(always)]
    fn path3_masked(&self) -> bool {
        self.m3r || self.m3p_vif1
    }

    #[inline]
    fn queue_for_path(&self, path: GifPath) -> &QwcFifo {
        match path {
            GifPath::Path1 => &self.path1_queue,
            GifPath::Path2 => &self.path2_queue,
            GifPath::Path3 => &self.path3_queue,
        }
    }

    #[inline]
    fn queue_for_path_mut(&mut self, path: GifPath) -> &mut QwcFifo {
        match path {
            GifPath::Path1 => &mut self.path1_queue,
            GifPath::Path2 => &mut self.path2_queue,
            GifPath::Path3 => &mut self.path3_queue,
        }
    }

    /// Return `true` if `GIF` currently has unblocked work that [`Self::step`] can execute.
    pub fn can_step(&self, gs: &GsPrivilegedStub) -> bool {
        if self.paused || gs.busdir_gs_to_ee() {
            return false;
        }
        let candidate = match self.active_path {
            Some(GifPath::Path3)
                if self.imt
                    && self.cursors[GifPath::Path3.cursor_index()].sub_qwc_half == 0
                    && (!self.path1_queue.is_empty() || !self.path2_queue.is_empty()) =>
            {
                if !self.path1_queue.is_empty() {
                    Some(GifPath::Path1)
                } else {
                    Some(GifPath::Path2)
                }
            }
            Some(p) => Some(p),
            None => self.select_next_path(),
        };
        let Some(path) = candidate else {
            return false;
        };
        let Some(qwc) = self.queue_for_path(path).peek() else {
            return false;
        };
        if !gs.signal_stalled() {
            return true;
        }
        !self.next_step_would_hit_signal(path, qwc)
    }

    /// Read a 32-bit GIF register (`0x1000_3000..=0x1000_30FF`). Reads never advance GIF state.
    pub fn read32(&self, paddr: u32, gs_busdir_gs_to_ee: bool) -> u32 {
        match paddr & 0xFF {
            0x20 => {
                let mut stat = 0u32;
                if self.m3r {
                    stat |= 1 << 0;
                }
                if self.m3p_vif1 {
                    stat |= 1 << 1;
                }
                if self.imt {
                    stat |= 1 << 2;
                }
                if self.paused {
                    stat |= 1 << 3;
                }
                if self.ip3 {
                    stat |= 1 << 5;
                }
                if !self.path3_queue.is_empty() || self.ip3 {
                    stat |= 1 << 6;
                }
                if !self.path2_queue.is_empty() || self.active_path == Some(GifPath::Path2) {
                    stat |= 1 << 7;
                }
                if !self.path1_queue.is_empty() || self.active_path == Some(GifPath::Path1) {
                    stat |= 1 << 8;
                }
                if self.active_path.is_some() {
                    stat |= 1 << 9;
                }
                if let Some(apath) = self.active_path {
                    stat |= (apath as u32) << 10;
                }
                if gs_busdir_gs_to_ee {
                    stat |= 1 << 12;
                }
                let fqc = (self.path3_queue.len().min(PATH3_FIFO_CAPACITY)) as u32;
                stat | ((fqc & 0x1F) << 24)
            }
            // `GIF_TAG0..3`, `GIF_CNT`, `GIF_P3CNT`, `GIF_P3TAG` are strictly `PSE`-gated (`#gifio`).
            0x40 => {
                if self.paused {
                    self.last_tag_words[0]
                } else {
                    0
                }
            }
            0x50 => {
                if self.paused {
                    self.last_tag_words[1]
                } else {
                    0
                }
            }
            0x60 => {
                if self.paused {
                    self.last_tag_words[2]
                } else {
                    0
                }
            }
            0x70 => {
                if self.paused {
                    self.last_tag_words[3]
                } else {
                    0
                }
            }
            0x80 => {
                if !self.paused {
                    return 0;
                }
                let Some(path) = self.active_path else {
                    return 0;
                };
                let cursor = &self.cursors[path.cursor_index()];
                let nloop = u32::from(cursor.remaining_loops) & 0x7FFF;
                let reg_desc = if cursor.tag_loaded
                    && cursor.remaining_loops > 0
                    && cursor.tag.format != GifDataFormat::Image
                {
                    u32::from((cursor.cur_reg_idx + 1) & 0xF)
                } else {
                    0
                };
                // TODO(verify): bits `20..=29` (`VU` data address for `PATH1 XGKICK`) arrive in Phase 2b.
                nloop | (reg_desc << 16)
            }
            0x90 => {
                if self.paused && self.ip3 {
                    u32::from(self.cursors[GifPath::Path3.cursor_index()].remaining_loops) & 0x7FFF
                } else {
                    0
                }
            }
            0xA0 if self.paused && self.ip3 => {
                self.cursors[GifPath::Path3.cursor_index()].tag.raw_words[0]
            }
            _ => 0,
        }
    }

    /// Write a 32-bit GIF control/mode register (`GIF_CTRL` / `GIF_MODE`).
    ///
    /// Does not synchronously drain queued GIF transfers; execution stays bounded in [`Self::step`].
    pub fn write32(&mut self, paddr: u32, val: u32, path3_dma_ongoing: bool) {
        match paddr & 0xFF {
            0x00 => {
                if (val & 1) != 0 {
                    self.reset();
                    return;
                }
                self.paused = (val & (1 << 3)) != 0;
            }
            0x10 => {
                let want_m3r = (val & 1) != 0;
                self.imt = (val & (1 << 2)) != 0;
                self.path3_dma_ongoing = path3_dma_ongoing;
                if want_m3r {
                    if path3_dma_ongoing || self.active_path == Some(GifPath::Path3) {
                        self.pending_m3r = true;
                    } else {
                        self.m3r = true;
                        self.pending_m3r = false;
                    }
                } else {
                    self.m3r = false;
                    self.pending_m3r = false;
                }
            }
            _ => {}
        }
    }

    /// Execute at most one bounded BUSCLK beat (`1` GIFtag or payload QWC) to the GS.
    ///
    /// Returns `true` if GIF consumed input/completed a state transition; returns `false` when idle,
    /// paused (`PSE`), masked, missing input, or stalled on `GS_CSR.SIGNAL`.
    pub fn step(&mut self, gs: &mut GsPrivilegedStub, intc: &mut EeIntc) -> bool {
        if self.paused || gs.busdir_gs_to_ee() {
            return false;
        }

        // In intermittent mode (`IMT=1`), `PATH3` yields cleanly at QWC boundaries to `PATH1`/`PATH2`.
        if self.active_path == Some(GifPath::Path3)
            && self.imt
            && self.cursors[GifPath::Path3.cursor_index()].sub_qwc_half == 0
            && (!self.path1_queue.is_empty() || !self.path2_queue.is_empty())
        {
            self.ip3 = true;
            self.active_path = None;
        }

        if self.active_path.is_none() {
            self.active_path = self.select_next_path();
            if self.active_path == Some(GifPath::Path3) {
                self.ip3 = false;
            }
        }

        let Some(path) = self.active_path else {
            return false;
        };
        let Some(qwc) = self.queue_for_path(path).peek() else {
            return false;
        };

        let progressed = if !self.cursors[path.cursor_index()].tag_loaded {
            self.process_giftag(path, qwc, gs, intc)
        } else {
            match self.cursors[path.cursor_index()].tag.format {
                GifDataFormat::Packed => self.process_packed_qwc(path, qwc, gs, intc),
                GifDataFormat::Reglist => self.process_reglist_qwc(path, qwc, gs, intc),
                GifDataFormat::Image => self.process_image_qwc(path, qwc, gs, intc),
            }
        };

        if progressed
            && self.pending_m3r
            && !self.path3_dma_ongoing
            && self.active_path != Some(GifPath::Path3)
        {
            self.m3r = true;
            self.pending_m3r = false;
        }

        progressed
    }

    fn select_next_path(&self) -> Option<GifPath> {
        if !self.path1_queue.is_empty() {
            Some(GifPath::Path1)
        } else if !self.path2_queue.is_empty() {
            Some(GifPath::Path2)
        } else if !self.path3_masked() && (!self.path3_queue.is_empty() || self.ip3) {
            Some(GifPath::Path3)
        } else {
            None
        }
    }

    fn next_step_would_hit_signal(&self, path: GifPath, qwc: Reg128) -> bool {
        let cursor = &self.cursors[path.cursor_index()];
        if !cursor.tag_loaded {
            return false;
        }
        match cursor.tag.format {
            GifDataFormat::Packed => {
                let reg_desc =
                    ((cursor.tag.regs >> (u32::from(cursor.cur_reg_idx) * 4)) & 0xF) as u8;
                reg_desc == 0xE && ((qwc.hi_u64() & 0xFF) as u8) == gs_reg::SIGNAL
            }
            GifDataFormat::Reglist | GifDataFormat::Image => false,
        }
    }

    fn process_giftag(
        &mut self,
        path: GifPath,
        qwc: Reg128,
        gs: &mut GsPrivilegedStub,
        intc: &mut EeIntc,
    ) -> bool {
        let tag = GifTag::from_qwc(qwc);
        self.queue_for_path_mut(path).pop();
        self.last_tag_words = tag.raw_words;

        let idx = path.cursor_index();
        if tag.nloop == 0 {
            self.cursors[idx].tag = tag;
            self.cursors[idx].remaining_loops = 0;
            self.cursors[idx].cur_reg_idx = 0;
            self.cursors[idx].sub_qwc_half = 0;
            self.cursors[idx].tag_loaded = false;
            if tag.eop {
                self.finish_path_packet(path);
            }
            return true;
        }

        self.internal_q = 0x3F80_0000;
        self.cursors[idx] = GifPathCursor {
            tag_loaded: true,
            tag,
            remaining_loops: tag.nloop,
            cur_reg_idx: 0,
            sub_qwc_half: 0,
        };
        if tag.pre {
            let _ = gs.write_internal_reg(gs_reg::PRIM, u64::from(tag.prim), intc);
        }
        true
    }

    fn process_packed_qwc(
        &mut self,
        path: GifPath,
        qwc: Reg128,
        gs: &mut GsPrivilegedStub,
        intc: &mut EeIntc,
    ) -> bool {
        let idx = path.cursor_index();
        let reg_idx = self.cursors[idx].cur_reg_idx;
        let reg_desc = ((self.cursors[idx].tag.regs >> (u32::from(reg_idx) * 4)) & 0xF) as u8;

        let lo = qwc.lo_u64();
        let hi = qwc.hi_u64();
        let write_target: Option<(u8, u64)> = match reg_desc {
            0x0 => Some((gs_reg::PRIM, lo & 0x7FF)),
            0x1 => {
                let r = lo & 0xFF;
                let g = (lo >> 32) & 0xFF;
                let b = hi & 0xFF;
                let a = (hi >> 32) & 0xFF;
                let rgbaq =
                    r | (g << 8) | (b << 16) | (a << 24) | (u64::from(self.internal_q) << 32);
                Some((gs_reg::RGBAQ, rgbaq))
            }
            0x2 => Some((gs_reg::ST, lo)),
            0x3 => {
                // TODO(verify): `ps2tek` omits standalone `03h UV` GS register bit offsets;
                // follows PS2 GS `UV` layout (`U` in bits `0..=13`, `V` in bits `16..=29`).
                let u = lo & 0x3FFF;
                let v = (lo >> 32) & 0x3FFF;
                Some((gs_reg::UV, u | (v << 16)))
            }
            0x4 => {
                let disable_draw = ((hi >> 47) & 1) != 0;
                let x = lo & 0xFFFF;
                let y = (lo >> 32) & 0xFFFF;
                let z = (hi >> 4) & 0x00FF_FFFF;
                let f = (hi >> 36) & 0xFF;
                let reg = if disable_draw {
                    gs_reg::XYZF3
                } else {
                    gs_reg::XYZF2
                };
                Some((reg, x | (y << 16) | (z << 32) | (f << 56)))
            }
            0x5 => {
                let disable_draw = ((hi >> 47) & 1) != 0;
                let x = lo & 0xFFFF;
                let y = (lo >> 32) & 0xFFFF;
                let z = hi & 0xFFFF_FFFF;
                let reg = if disable_draw {
                    gs_reg::XYZ3
                } else {
                    gs_reg::XYZ2
                };
                Some((reg, x | (y << 16) | (z << 32)))
            }
            // Per `ps2tek` `#gifdataformats`, unspecified descriptors `0x6..=0x9`, `0xC..=0xD`
            // forward `lo` directly to GS register `reg_desc`.
            0x6..=0x9 | 0xC | 0xD => Some((reg_desc, lo)),
            // TODO(verify): `ps2tek` `#gsvertexattributes` omits standalone `0Ah FOG` bit position;
            // `04h XYZF2` places 8-bit `F` in GS register bits `56..=63`.
            0xA => Some((gs_reg::FOG, ((hi >> 36) & 0xFF) << 56)),
            // TODO(verify): `ps2tek` `#gifdataformats` omits descriptor `0Bh`.
            0xB | 0xF => None,
            0xE => Some(((hi & 0xFF) as u8, lo)),
            _ => unreachable!("4-bit descriptor"),
        };

        if let Some((gs_reg_addr, gs_data)) = write_target {
            if !gs.write_internal_reg(gs_reg_addr, gs_data, intc) {
                return false;
            }
        }
        if reg_desc == 0x2 {
            self.internal_q = hi as u32;
        }

        self.queue_for_path_mut(path).pop();
        self.advance_reg_loop(path);
        true
    }

    fn process_reglist_qwc(
        &mut self,
        path: GifPath,
        qwc: Reg128,
        gs: &mut GsPrivilegedStub,
        intc: &mut EeIntc,
    ) -> bool {
        let idx = path.cursor_index();
        let start_half = self.cursors[idx].sub_qwc_half;
        let words = [qwc.lo_u64(), qwc.hi_u64()];

        for (half, &data) in words.iter().enumerate().skip(usize::from(start_half)) {
            if !self.cursors[idx].tag_loaded {
                // `NREGS * NLOOP` was odd; discard trailing high 64 bits (`ps2tek` `#gifdataformats`).
                break;
            }
            let reg_idx = self.cursors[idx].cur_reg_idx;
            let reg_desc = ((self.cursors[idx].tag.regs >> (u32::from(reg_idx) * 4)) & 0xF) as u8;
            if reg_desc != 0xF && !gs.write_internal_reg(reg_desc, data, intc) {
                self.cursors[idx].sub_qwc_half = half as u8;
                return false;
            }
            self.advance_reg_loop(path);
        }

        self.cursors[idx].sub_qwc_half = 0;
        self.queue_for_path_mut(path).pop();
        true
    }

    fn process_image_qwc(
        &mut self,
        path: GifPath,
        qwc: Reg128,
        gs: &mut GsPrivilegedStub,
        intc: &mut EeIntc,
    ) -> bool {
        let idx = path.cursor_index();
        let _ = gs.write_internal_reg(gs_reg::HWREG, qwc.lo_u64(), intc);
        let _ = gs.write_internal_reg(gs_reg::HWREG, qwc.hi_u64(), intc);
        self.queue_for_path_mut(path).pop();

        self.cursors[idx].remaining_loops -= 1;
        if self.cursors[idx].remaining_loops == 0 {
            self.cursors[idx].tag_loaded = false;
            if self.cursors[idx].tag.eop {
                self.finish_path_packet(path);
            }
        }
        true
    }

    #[inline]
    fn advance_reg_loop(&mut self, path: GifPath) {
        let idx = path.cursor_index();
        let cursor = &mut self.cursors[idx];
        cursor.cur_reg_idx += 1;
        if cursor.cur_reg_idx >= cursor.tag.nregs {
            cursor.cur_reg_idx = 0;
            cursor.remaining_loops -= 1;
            if cursor.remaining_loops == 0 {
                cursor.tag_loaded = false;
                if cursor.tag.eop {
                    self.finish_path_packet(path);
                }
            }
        }
    }

    #[inline]
    fn finish_path_packet(&mut self, path: GifPath) {
        self.active_path = None;
        if path == GifPath::Path3 {
            self.ip3 = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_giftag(
        nloop: u16,
        eop: bool,
        pre: bool,
        prim: u16,
        flg: u8,
        nregs: u8,
        regs: u64,
    ) -> Reg128 {
        let mut lo = u64::from(nloop & 0x7FFF);
        if eop {
            lo |= 1 << 15;
        }
        if pre {
            lo |= 1 << 46;
        }
        lo |= u64::from(prim & 0x7FF) << 47;
        lo |= u64::from(flg & 0x3) << 58;
        lo |= u64::from(nregs & 0xF) << 60;
        Reg128::from_u64_pair(lo, regs)
    }

    fn drain_gif(
        gif: &mut GifUnit,
        gs: &mut GsPrivilegedStub,
        intc: &mut EeIntc,
        max_steps: usize,
    ) -> usize {
        let mut steps = 0;
        while steps < max_steps && gif.step(gs, intc) {
            steps += 1;
        }
        steps
    }

    #[test]
    fn gif_nloop_zero_ignores_pre_and_q_and_honors_eop() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        // NLOOP=0, EOP=1, PRE=1, PRIM=0x123 -> must NOT write PRIM or touch internal_q beyond NLOOP=0 skip.
        gif.internal_q = 0x1234_5678;
        let tag = make_giftag(0, true, true, 0x123, 0, 1, 0xE);
        assert!(gif.push_path3_qwc(tag));
        assert_eq!(drain_gif(&mut gif, &mut gs, &mut intc, 16), 1);
        assert_eq!(gs.internal_write_count, 0);
        assert_eq!(gs.internal_reg(gs_reg::PRIM), 0);
        assert_eq!(gif.internal_q, 0x1234_5678);
        assert_eq!(gif.read32(0x1000_3020, false) & (1 << 9), 0); // OPH=0
    }

    #[test]
    fn gif_pre_prim_and_packed_all_descriptors() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        // NLOOP=1, EOP=1, PRE=1, PRIM=0x2AB, FLG=PACKED(0), NREG=16 (0 in raw field!)
        // Descriptors 0..=15 in order 0xFEDC_BA98_7654_3210:
        // 0:PRIM, 1:RGBAQ (uses Q=1.0f initially), 2:ST (updates Q), 3:UV, 4:XYZF2,
        // 5:XYZ3 (ADC=1), 6:TEX0_1, 7:TEX0_2, 8:CLAMP_1, 9:CLAMP_2, A:FOG,
        // B:(reserved/ignored), C:XYZF3, D:XYZ3, E:A+D(LABEL), F:NOP
        let tag = make_giftag(1, true, true, 0x2AB, 0, 0, 0xFEDC_BA98_7654_3210);
        assert!(gif.push_path3_qwc(tag));
        // Step GIFtag: initializes internal Q = 1.0f and writes PRE PRIM = 0x2AB.
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::PRIM), 0x2AB);

        // Push 16 QWCs covering descriptors 0..=15.
        let q0_prim = Reg128::from_u64_pair(0x006, 0);
        let q1_rgbaq = Reg128::from_u32x4(0x11, 0x22, 0x33, 0x44); // should pair with Q=1.0f (0x3F800000)
        let q2_stq = Reg128::from_u32x4(0xAAAA_0001, 0xBBBB_0002, 0x4000_0000, 0); // sets Q = 2.0f
        let q3_uv = Reg128::from_u32x4(0x1234, 0x3567, 0, 0);
        let q4_xyzf2 = Reg128::from_u64_pair(
            0x0200_u64 << 32 | 0x0100,
            (0x05_u64 << 36) | (0x12_3456_u64 << 4),
        ); // ADC=0, Z=0x123456, F=0x05
        let q5_xyz2_adc1 =
            Reg128::from_u64_pair((0x0400_u64 << 32) | 0x0300, (1u64 << 47) | 0x89AB_CDEF); // ADC=1 -> XYZ3
        let q6_tex0_1 = Reg128::from_u64_pair(0x1111_2222_3333_4444, 0xDEAD);
        let q7_tex0_2 = Reg128::from_u64_pair(0x5555_6666_7777_8888, 0);
        let q8_clamp_1 = Reg128::from_u64_pair(0x9999_0000_1111_2222, 0);
        let q9_clamp_2 = Reg128::from_u64_pair(0x3333_4444_5555_6666, 0);
        let qa_fog = Reg128::from_u64_pair(0, 0xAB_u64 << 36);
        let qb_resv = Reg128::from_u64_pair(0xFFFF_FFFF_FFFF_FFFF, 0);
        let qc_xyzf3 = Reg128::from_u64_pair(0x0C0C_0C0C_0C0C_0C0C, 0);
        let qd_xyz3 = Reg128::from_u64_pair(0x0D0D_0D0D_0D0D_0D0D, 0);
        let qe_ad_label = Reg128::from_u64_pair((0x77 << 32) | 0x77, u64::from(gs_reg::LABEL));
        let qf_nop = Reg128::from_u64_pair(0x1234, u64::from(gs_reg::SIGNAL)); // must NOT trigger SIGNAL!

        let qwcs = [
            q0_prim,
            q1_rgbaq,
            q2_stq,
            q3_uv,
            q4_xyzf2,
            q5_xyz2_adc1,
            q6_tex0_1,
            q7_tex0_2,
            q8_clamp_1,
            q9_clamp_2,
            qa_fog,
            qb_resv,
            qc_xyzf3,
            qd_xyz3,
            qe_ad_label,
            qf_nop,
        ];

        for q in qwcs {
            assert!(gif.push_path3_qwc(q));
            assert!(gif.step(&mut gs, &mut intc));
        }

        assert_eq!(gs.internal_reg(gs_reg::PRIM), 0x006);
        assert_eq!(gs.internal_reg(gs_reg::RGBAQ), 0x3F80_0000_4433_2211);
        assert_eq!(gs.internal_reg(gs_reg::ST), 0xBBBB_0002_AAAA_0001);
        assert_eq!(gif.internal_q, 0x4000_0000);
        assert_eq!(gs.internal_reg(gs_reg::UV), 0x3567_1234 & 0x3FFF_3FFF);
        assert_eq!(gs.internal_reg(gs_reg::XYZF2), 0x0512_3456_0200_0100);
        // Note `qd_xyz3` overwrote XYZ3 (0x0D) after `q5_xyz2_adc1` wrote XYZ3 (0x0D)!
        assert_eq!(gs.internal_reg(gs_reg::XYZ2), 0);
        assert_eq!(gs.internal_reg(gs_reg::XYZ3), 0x0D0D_0D0D_0D0D_0D0D);
        assert_eq!(gs.internal_reg(gs_reg::TEX0_1), 0x1111_2222_3333_4444);
        assert_eq!(gs.internal_reg(gs_reg::TEX0_2), 0x5555_6666_7777_8888);
        assert_eq!(gs.internal_reg(gs_reg::CLAMP_1), 0x9999_0000_1111_2222);
        assert_eq!(gs.internal_reg(gs_reg::CLAMP_2), 0x3333_4444_5555_6666);
        assert_eq!(gs.internal_reg(gs_reg::FOG), 0xAB << 56);
        assert_eq!(gs.internal_reg(gs_reg::XYZF3), 0x0C0C_0C0C_0C0C_0C0C);
        assert_eq!(gs.siglblid, 0x0000_0077_0000_0000);
        assert_eq!(gs.csr & 1, 0);
    }

    #[test]
    fn gif_reglist_odd_count_discards_last_doubleword() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        // NLOOP=1, NREG=3 -> 3 doublewords total (odd): [PRIM, RGBAQ, ST], 4th dword discarded!
        let tag = make_giftag(1, true, false, 0, 1, 3, 0x0000_0000_0000_0210);
        assert!(gif.push_path3_qwc(tag));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(0x0004, 0x1122_3344_5566_7788)));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(
            0xAAAA_BBBB_CCCC_DDDD,
            0xDEAD_BEEF_CAFE_BABE
        )));

        assert_eq!(drain_gif(&mut gif, &mut gs, &mut intc, 8), 3);
        assert_eq!(gs.internal_reg(gs_reg::PRIM), 0x0004);
        assert_eq!(gs.internal_reg(gs_reg::RGBAQ), 0x1122_3344_5566_7788);
        assert_eq!(gs.internal_reg(gs_reg::ST), 0xAAAA_BBBB_CCCC_DDDD);
        // Only 3 GS writes happened (`0xDEAD_BEEF_CAFE_BABE` was discarded).
        assert_eq!(gs.internal_write_count, 3);
    }

    #[test]
    fn gif_image_mode_sends_two_hwregs_per_qwc() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        // NLOOP=2, EOP=1, FLG=2 (IMAGE)
        let tag = make_giftag(2, true, false, 0, 2, 1, 0);
        assert!(gif.push_path3_qwc(tag));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(0x11, 0x22)));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(0x33, 0x44)));

        // GIFtag makes `path3_image_active` true once loaded.
        assert!(gif.step(&mut gs, &mut intc));
        assert!(gif.path3_image_active());
        assert!(!gif.can_accept_vif1_direct(true));

        assert_eq!(drain_gif(&mut gif, &mut gs, &mut intc, 8), 2);
        assert!(!gif.path3_image_active());
        assert_eq!(gs.hwreg_write_count, 4);
        assert_eq!(gs.internal_reg(gs_reg::HWREG), 0x44);
    }

    #[test]
    fn gif_pse_gates_debug_registers_and_pauses_transfers() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        let tag = make_giftag(2, true, false, 0, 0, 2, 0x21);
        assert!(gif.push_path3_qwc(tag));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(0x01, 0)));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(0x02, 0)));

        // Step GIFtag + 1st data QWC (`RGBAQ`), leaving `remaining_loops = 2`, `cur_reg_idx = 1` (`ST` next -> desc pos 2).
        assert!(gif.step(&mut gs, &mut intc));
        assert!(gif.step(&mut gs, &mut intc));

        // With PSE=0, debug registers MUST return 0!
        assert_eq!(gif.read32(0x1000_3040, false), 0);
        assert_eq!(gif.read32(0x1000_3080, false), 0);

        // Set PSE=1 (`GIF_CTRL = 8`).
        gif.write32(0x1000_3000, 1 << 3, false);
        assert!(!gif.step(&mut gs, &mut intc));
        assert_eq!(gif.read32(0x1000_3040, false), tag.lo_u64() as u32);
        assert_eq!(gif.read32(0x1000_3050, false), (tag.lo_u64() >> 32) as u32);
        assert_eq!(gif.read32(0x1000_3060, false), tag.hi_u64() as u32);
        assert_eq!(gif.read32(0x1000_3070, false), (tag.hi_u64() >> 32) as u32);
        assert_eq!(gif.read32(0x1000_3080, false), 2 | (2 << 16));
    }

    #[test]
    fn gif_imt_interrupts_path3_saves_p3cnt_p3tag_and_resumes_after_higher_priority_paths() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        // Enable IMT=1 (`GIF_MODE = 4`).
        gif.write32(0x1000_3010, 1 << 2, false);

        // Queue a 3-QWC PATH3 PACKED primitive (`NLOOP=3`, EOP=1, A+D).
        let p3_tag = make_giftag(3, true, false, 0, 0, 1, 0xE);
        assert!(gif.push_path3_qwc(p3_tag));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(1, u64::from(gs_reg::TEX0_1))));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(2, u64::from(gs_reg::TEX0_1))));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(3, u64::from(gs_reg::TEX0_1))));

        // Execute PATH3 GIFtag + 1 QWC (`TEX0_1 = 1`, `remaining_loops = 2`).
        assert!(gif.step(&mut gs, &mut intc));
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::TEX0_1), 1);

        // Now both PATH2 and PATH1 receive packets! Arbitration MUST pick PATH1 > PATH2 > PATH3.
        gif.begin_vif1_direct(2, false);
        gif.push_vif1_direct_qwc(make_giftag(1, true, false, 0, 0, 1, 0xE));
        gif.push_vif1_direct_qwc(Reg128::from_u64_pair(0x2222, u64::from(gs_reg::CLAMP_1)));
        gif.finish_vif1_direct();

        assert!(gif.push_path1_qwc(make_giftag(1, true, false, 0, 0, 1, 0xE)));
        assert!(gif.push_path1_qwc(Reg128::from_u64_pair(0x1111, u64::from(gs_reg::PRIM))));

        // Step once: PATH3 yields cleanly (`IP3=1`), PATH1 starts (`APATH=1`).
        assert!(gif.step(&mut gs, &mut intc));
        gif.write32(0x1000_3000, 1 << 3, false); // Pause to inspect PSE-gated P3CNT & P3TAG
        let stat = gif.read32(0x1000_3020, false);
        assert_ne!(stat & (1 << 5), 0); // IP3 = 1
        assert_eq!((stat >> 10) & 3, 1); // APATH = PATH1
        assert_eq!(gif.read32(0x1000_3090, false), 2); // GIF_P3CNT == remaining 2
        assert_eq!(gif.read32(0x1000_30A0, false), p3_tag.lo_u64() as u32);
        gif.write32(0x1000_3000, 0, false); // Unpause

        // Next step completes PATH1 PRIM=0x111 (masked 11-bit).
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::PRIM), 0x1111);

        // Next two steps run PATH2 (`CLAMP_1 = 0x2222`) while PATH3 remains interrupted.
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!((gif.read32(0x1000_3020, false) >> 10) & 3, 2); // APATH = PATH2
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::CLAMP_1), 0x2222);

        // Next steps resume PATH3 (`IP3` clears, `TEX0_1` finishes `2` and `3`).
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::TEX0_1), 2);
        assert_eq!((gif.read32(0x1000_3020, false) >> 10) & 3, 3); // APATH = PATH3
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::TEX0_1), 3);
        assert_eq!(gif.read32(0x1000_3020, false) & ((1 << 5) | (1 << 9)), 0); // IP3=0, OPH=0
    }

    #[test]
    fn gif_signal_stall_halts_progress_until_csr_cleared_and_reads_never_advance() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        let tag = make_giftag(2, true, false, 0, 0, 1, 0xE);
        assert!(gif.push_path3_qwc(tag));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(
            (0xF0 << 32) | 0xF0,
            u64::from(gs_reg::SIGNAL)
        )));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(
            (0xFF << 32) | 0x12,
            u64::from(gs_reg::SIGNAL)
        )));

        // First SIGNAL executes cleanly.
        assert_eq!(drain_gif(&mut gif, &mut gs, &mut intc, 2), 2);
        assert_eq!(gs.siglblid, 0xF0);
        assert_eq!(gs.csr & 1, 1);

        // Second SIGNAL stalls! Stepping or reading many times makes zero progress.
        for _ in 0..20 {
            assert!(!gif.step(&mut gs, &mut intc));
            let _ = gif.read32(0x1000_3020, false);
            let _ = gs.read64(0x1200_1000);
            let _ = gs.read64(0x1200_1080);
        }
        assert_eq!(gs.siglblid, 0xF0);
        assert_eq!(gif.path3_fifo_len(), 1);

        // Clear GS_CSR.SIGNAL -> unstalls and delivers second SIGNAL.
        gs.write64(0x1200_1000, 1);
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.siglblid, 0x12);
        assert_eq!(gs.csr & 1, 1);
        assert_eq!(gif.path3_fifo_len(), 0);
    }

    #[test]
    fn gif_mode_m3r_defers_until_dma_and_active_packet_end() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        gif.on_path3_dma_start();
        assert!(gif.push_path3_qwc(make_giftag(1, true, false, 0, 0, 1, 0xE)));
        assert!(gif.push_path3_qwc(Reg128::from_u64_pair(0x55, u64::from(gs_reg::CLAMP_1))));
        // Start PATH3 packet, then set M3R while DMA + PATH3 packet are ongoing.
        assert!(gif.step(&mut gs, &mut intc));
        gif.write32(0x1000_3010, 1, true);
        assert_eq!(gif.read32(0x1000_3020, false) & 1, 0); // Still unmasked until DMA & packet end

        // Remaining QWC of active packet still delivers cleanly before DMA ends.
        assert!(gif.step(&mut gs, &mut intc));
        assert_eq!(gs.internal_reg(gs_reg::CLAMP_1), 0x55);
        assert_eq!(gif.read32(0x1000_3020, false) & 1, 0); // DMA still marked ongoing

        // Once D2 DMA completes, deferred M3R takes effect immediately and holds new PATH3 data in FIFO.
        gif.on_path3_dma_end();
        assert_eq!(gif.read32(0x1000_3020, false) & 1, 1);
        assert!(gif.push_path3_qwc(make_giftag(1, true, false, 0, 0, 1, 0xE)));
        assert!(!gif.step(&mut gs, &mut intc));
        assert_eq!(gif.path3_fifo_len(), 1);
    }

    #[test]
    fn gif_stat_reflects_mskpath3_mode_and_direct_vs_directhl_rules() {
        let mut gif = GifUnit::new();
        let mut gs = GsPrivilegedStub::new();
        let mut intc = EeIntc::new();

        gif.m3p_vif1 = true;
        gif.write32(0x1000_3010, (1 << 2) | (1 << 0), false);
        gif.write32(0x1000_3000, 1 << 3, false); // PSE
        let stat = gif.read32(0x1000_3020, true);
        assert_eq!(stat & 0xF, 0xF); // M3R | M3P | IMT | PSE
        assert_ne!(stat & (1 << 12), 0); // DIR = 1 (GS->EE)

        // Reset via GIF_CTRL.RST clears PSE/IMT/M3R.
        gif.write32(0x1000_3000, 1, false);
        assert_eq!(gif.read32(0x1000_3020, false) & 0xF, 0);

        // Non-IMAGE active PATH3 blocks `DIRECT` (`is_hl=false`) when `IMT=0`, while `DIRECTHL` (`is_hl=true`) is allowed!
        let packed_tag = make_giftag(2, true, false, 0, 0, 1, 0xE);
        assert!(gif.push_path3_qwc(packed_tag));
        assert!(gif.step(&mut gs, &mut intc));
        assert!(!gif.path3_image_active());
        assert!(!gif.can_accept_vif1_direct(false));
        assert!(gif.can_accept_vif1_direct(true));

        // When IMT=1, even `DIRECT` (`is_hl=false`) can queue alongside active non-IMAGE PATH3.
        gif.write32(0x1000_3010, 1 << 2, false);
        assert!(gif.can_accept_vif1_direct(false));
    }
}
