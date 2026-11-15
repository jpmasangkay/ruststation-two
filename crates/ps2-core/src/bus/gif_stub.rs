//! Minimal Graphics Interface (`GIF`) stub for Phase 2a Step 3 (`VIF1` `DIRECT`/`DIRECTHL`/`MSKPATH3`).
//!
//! Primary source references (`FETCHED`):
//! - `local/ps2tek/index.html` `#gifio` (`lines 1265..=1355`):
//!   * `0x1000_3000` `GIF_CTRL` (`W`: bit 0 `RST`, bit 3 `PSE` temporary stop)
//!   * `0x1000_3010` `GIF_MODE` (`W`: bit 0 `M3R`, bit 2 `IMT`)
//!   * `0x1000_3020` `GIF_STAT` (`R`: bit 0 `M3R`, bit 1 `M3P` from `VIF1 MSKPATH3`, bit 2 `IMT`, bit 3 `PSE`)
//!   * `0x1000_3040..=0x1000_30A0` `GIF_TAG0..3`, `GIF_CNT`, `GIF_P3CNT`, `GIF_P3TAG`
//!   * `0x1000_6000..=0x1000_6FFF` `GIF_FIFO`
//! - `local/ps2autotests/tests/dma/vif/mskpath3.cpp`: verifies `GIF_STAT.M3P` (`0x02`) tracking `VIF1 MSKPATH3`.
//!
//! Note: Step 3 stores received `DIRECT`/`DIRECTHL` packets in a pre-allocated ring buffer with no GS rendering;
//! full `GIF` arbiter, GIFtag parser, PATH3 transfers, and `GS` register forwarding are added in Step 4.

use ps2_common::Reg128;

/// Capacity of the pre-allocated quadword ring buffer storing received GIF packets (`64 KB`).
const GIF_STUB_QWC_CAPACITY: usize = 4096;

/// Maximum number of packet descriptors retained in the Step 3 ring metadata table.
const GIF_STUB_META_CAPACITY: usize = 128;

/// Descriptor of a single packet received by [`GifStub`] (e.g. from `VIF1` `DIRECT`/`DIRECTHL`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GifPacketMeta {
    /// Start index into the quadword ring buffer (modulo `GIF_STUB_QWC_CAPACITY`).
    pub start_qwc: usize,
    /// Number of quadwords received for this packet.
    pub qwc_len: usize,
    /// Declared quadword length from `DIRECT`/`DIRECTHL` (`1..=65536`).
    pub expected_qwc: u32,
    /// Source path (`2` = `VIF1 DIRECT`/`DIRECTHL`, `3` = direct `GIF_FIFO` write).
    pub path: u8,
    /// `true` if submitted via `DIRECTHL` (`0x51`), `false` for `DIRECT` (`0x50`) / FIFO.
    pub is_hl: bool,
    /// `true` once all expected quadwords have arrived.
    pub completed: bool,
}

/// Minimal `GIF` stub capturing `VIF1` `DIRECT`/`DIRECTHL` packets and `GIF_STAT` bits (`Step 3`).
#[derive(Clone, Debug)]
pub struct GifStub {
    /// `0x1000_3000` (`GIF_CTRL`): Control register (bit 3 = `PSE` temporary stop).
    pub ctrl: u32,
    /// `0x1000_3010` (`GIF_MODE`): Mode register (bit 0 = `M3R`, bit 2 = `IMT`).
    pub mode: u32,
    /// `GIF_STAT` bit 1 (`M3P`): `PATH3` masked by `VIF1 MSKPATH3` (`0x06`).
    pub m3p_vif1: bool,
    /// `true` while a `PATH3` `IMAGE`-mode transfer is active (blocks `VIF1` `DIRECTHL` `0x51`).
    pub path3_image_active: bool,
    /// Pre-allocated quadword ring buffer (`zero` hot-path heap allocations).
    qwc_ring: Box<[Reg128]>,
    qwc_write_cursor: usize,
    /// Circular metadata table for recently received packets.
    packets: [GifPacketMeta; GIF_STUB_META_CAPACITY],
    packet_write_idx: usize,
    active_packet_idx: Option<usize>,
    /// Cumulative number of completed or started packets pushed to this stub.
    pub total_packets: u64,
    /// Cumulative number of quadwords pushed to this stub.
    pub total_qwc: u64,
}

impl Default for GifStub {
    fn default() -> Self {
        Self::new()
    }
}

impl GifStub {
    /// Allocate and initialize a fresh [`GifStub`] (no heap allocations occur after construction).
    pub fn new() -> Self {
        Self {
            ctrl: 0,
            mode: 0,
            m3p_vif1: false,
            path3_image_active: false,
            qwc_ring: vec![Reg128::ZERO; GIF_STUB_QWC_CAPACITY].into_boxed_slice(),
            qwc_write_cursor: 0,
            packets: [GifPacketMeta::default(); GIF_STUB_META_CAPACITY],
            packet_write_idx: 0,
            active_packet_idx: None,
            total_packets: 0,
            total_qwc: 0,
        }
    }

    /// Reset `GIF` stub state while preserving the pre-allocated backing buffer.
    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.mode = 0;
        self.m3p_vif1 = false;
        self.path3_image_active = false;
        self.qwc_write_cursor = 0;
        self.packets = [GifPacketMeta::default(); GIF_STUB_META_CAPACITY];
        self.packet_write_idx = 0;
        self.active_packet_idx = None;
        self.total_packets = 0;
        self.total_qwc = 0;
    }

    /// Returns `true` when `GIF` transfers are temporarily paused via `GIF_CTRL.PSE` (bit 3).
    #[inline(always)]
    pub const fn is_paused(&self) -> bool {
        (self.ctrl & (1 << 3)) != 0
    }

    /// Returns `true` when `VIF1` `DIRECT` (`0x50`) or `DIRECTHL` (`0x51`) can transfer immediately.
    #[inline(always)]
    pub const fn can_accept_vif1_direct(&self, is_hl: bool) -> bool {
        if self.is_paused() {
            return false;
        }
        if is_hl && self.path3_image_active {
            return false;
        }
        true
    }

    /// Begin receiving a `VIF1` `DIRECT` / `DIRECTHL` (`PATH2`) packet of `expected_qwc` quadwords.
    pub fn begin_vif1_direct(&mut self, expected_qwc: u32, is_hl: bool) {
        let idx = self.packet_write_idx % GIF_STUB_META_CAPACITY;
        self.packets[idx] = GifPacketMeta {
            start_qwc: self.qwc_write_cursor,
            qwc_len: 0,
            expected_qwc,
            path: 2,
            is_hl,
            completed: false,
        };
        self.packet_write_idx = self.packet_write_idx.wrapping_add(1);
        self.active_packet_idx = Some(idx);
        self.total_packets = self.total_packets.saturating_add(1);
    }

    /// Append one 128-bit quadword to the currently active `VIF1` `DIRECT`/`DIRECTHL` packet.
    pub fn push_vif1_direct_qwc(&mut self, qwc: Reg128) {
        let ring_pos = self.qwc_write_cursor % GIF_STUB_QWC_CAPACITY;
        self.qwc_ring[ring_pos] = qwc;
        self.qwc_write_cursor = self.qwc_write_cursor.wrapping_add(1);
        self.total_qwc = self.total_qwc.saturating_add(1);
        if let Some(idx) = self.active_packet_idx {
            self.packets[idx].qwc_len = self.packets[idx].qwc_len.saturating_add(1);
        }
    }

    /// Mark the active `VIF1` `DIRECT`/`DIRECTHL` packet complete.
    pub fn finish_vif1_direct(&mut self) {
        if let Some(idx) = self.active_packet_idx.take() {
            self.packets[idx].completed = true;
        }
    }

    /// Accept a direct 128-bit `GIF_FIFO` write (`0x1000_6000..=0x1000_6FFF`) in Step 3 stub mode.
    pub fn push_fifo_qwc(&mut self, qwc: Reg128) {
        let ring_pos = self.qwc_write_cursor % GIF_STUB_QWC_CAPACITY;
        self.qwc_ring[ring_pos] = qwc;
        self.qwc_write_cursor = self.qwc_write_cursor.wrapping_add(1);
        self.total_qwc = self.total_qwc.saturating_add(1);
    }

    /// Inspect the most recently started/completed packet metadata, if any.
    pub fn last_packet(&self) -> Option<GifPacketMeta> {
        if self.packet_write_idx == 0 {
            return None;
        }
        let idx = (self.packet_write_idx - 1) % GIF_STUB_META_CAPACITY;
        Some(self.packets[idx])
    }

    /// Read the `i`-th quadword of `meta` (within ring retention window).
    pub fn read_packet_qwc(&self, meta: &GifPacketMeta, qwc_index: usize) -> Option<Reg128> {
        if qwc_index >= meta.qwc_len || qwc_index >= GIF_STUB_QWC_CAPACITY {
            return None;
        }
        let pos = meta.start_qwc.wrapping_add(qwc_index) % GIF_STUB_QWC_CAPACITY;
        Some(self.qwc_ring[pos])
    }

    /// Read a 32-bit `GIF` register in `0x1000_3000..=0x1000_30AF` (`ps2tek` `#gifio`).
    #[inline]
    pub const fn read32(&self, paddr: u32) -> u32 {
        match paddr {
            // `0x1000_3020` `GIF_STAT`:
            // bit 0 = M3R (GIF_MODE.0)
            // bit 1 = M3P (VIF1 MSKPATH3)
            // bit 2 = IMT (GIF_MODE.2)
            // bit 3 = PSE (GIF_CTRL.3)
            0x1000_3020 => {
                let m3r = self.mode & 1;
                let m3p = (self.m3p_vif1 as u32) << 1;
                let imt = ((self.mode >> 2) & 1) << 2;
                let pse = ((self.ctrl >> 3) & 1) << 3;
                m3r | m3p | imt | pse
            }
            // Write-only / paused-diagnostic registers read as `0` in Step 3 stub.
            0x1000_3000 | 0x1000_3010 | 0x1000_3040 | 0x1000_3050 | 0x1000_3060 | 0x1000_3070
            | 0x1000_3080 | 0x1000_3090 | 0x1000_30A0 => 0,
            _ => 0,
        }
    }

    /// Write a 32-bit `GIF` register in `0x1000_3000..=0x1000_30AF` (`ps2tek` `#gifio`).
    pub fn write32(&mut self, paddr: u32, val: u32) {
        match paddr {
            0x1000_3000 => {
                if (val & 1) != 0 {
                    // `GIF_CTRL.RST` resets GIF state (note: VIF1 MSKPATH3 `m3p_vif1` is VIF-owned).
                    self.ctrl = 0;
                    self.mode = 0;
                    self.path3_image_active = false;
                    self.active_packet_idx = None;
                }
                self.ctrl = (self.ctrl & !(1 << 3)) | (val & (1 << 3));
            }
            0x1000_3010 => {
                self.mode = val & 0b101;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_stub_stat_reflects_mskpath3_and_mode_bits() {
        let mut gif = GifStub::new();
        assert_eq!(gif.read32(0x1000_3020), 0);

        gif.m3p_vif1 = true;
        assert_eq!(gif.read32(0x1000_3020) & 0x02, 0x02);

        gif.write32(0x1000_3010, 0b101); // M3R + IMT
        assert_eq!(gif.read32(0x1000_3020) & 0x07, 0x07);

        gif.write32(0x1000_3000, 1 << 3); // PSE pause
        assert!(gif.is_paused());
        assert!(!gif.can_accept_vif1_direct(false));

        // Reset via GIF_CTRL.RST preserves M3P (owned by VIF1 MSKPATH3) and clears PSE/M3R/IMT.
        gif.write32(0x1000_3000, 1);
        assert!(!gif.is_paused());
        assert_eq!(gif.read32(0x1000_3020), 0x02);
    }

    #[test]
    fn gif_stub_stores_vif1_direct_packets_and_enforces_directhl_rule() {
        let mut gif = GifStub::new();
        gif.path3_image_active = true;
        assert!(gif.can_accept_vif1_direct(false));
        assert!(!gif.can_accept_vif1_direct(true));

        let q0 = Reg128::from_u32x4(0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888);
        let q1 = Reg128::from_u32x4(0x9999_AAAA, 0xBBBB_CCCC, 0xDDDD_EEEE, 0xFFFF_0000);

        gif.begin_vif1_direct(2, false);
        gif.push_vif1_direct_qwc(q0);
        gif.push_vif1_direct_qwc(q1);
        gif.finish_vif1_direct();

        let meta = gif.last_packet().expect("packet stored");
        assert_eq!(meta.expected_qwc, 2);
        assert_eq!(meta.qwc_len, 2);
        assert_eq!(meta.path, 2);
        assert!(meta.completed);
        assert_eq!(gif.read_packet_qwc(&meta, 0), Some(q0));
        assert_eq!(gif.read_packet_qwc(&meta, 1), Some(q1));
    }
}
