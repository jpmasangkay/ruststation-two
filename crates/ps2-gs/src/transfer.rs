//! GS transfer state machine: `Host -> Local`, `Local -> Host`, and `Local -> Local`.
//!
//! Register layouts and rules follow `local/ps2tek/index.html` (`#gstransfers`, FETCHED).

use ps2_common::Reg128;
use tracing::warn;

use crate::regs::GsRegs;
use crate::vram::{GsVram, PixelFormat};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TransferMode {
    Idle,
    HostToLocal,
    LocalToHost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferState {
    mode: TransferMode,
    src_format: PixelFormat,
    dst_format: PixelFormat,
    sbp: u32,
    sbw: u32,
    dbp: u32,
    dbw: u32,
    ssax: u16,
    ssay: u16,
    dsax: u16,
    dsay: u16,
    rrw: u16,
    rrh: u16,
    cur_x: u16,
    cur_y: u16,
    /// Fixed byte staging buffer for non-power-of-two `PSMCT24`/`PSMZ24` across 8-byte `HWREG`s.
    pending_bytes: [u8; 16],
    pending_len: usize,
}

impl Default for TransferState {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferState {
    pub const fn new() -> Self {
        Self {
            mode: TransferMode::Idle,
            src_format: PixelFormat::Psmct32,
            dst_format: PixelFormat::Psmct32,
            sbp: 0,
            sbw: 0,
            dbp: 0,
            dbw: 0,
            ssax: 0,
            ssay: 0,
            dsax: 0,
            dsay: 0,
            rrw: 0,
            rrh: 0,
            cur_x: 0,
            cur_y: 0,
            pending_bytes: [0; 16],
            pending_len: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    #[inline]
    pub const fn mode(&self) -> TransferMode {
        self.mode
    }

    /// Start or execute a GS transfer triggered by writing `TRXDIR` (`0x53`).
    pub fn start_trxdir(&mut self, regs: &GsRegs, vram: &mut GsVram, trxdir: u8) {
        self.pending_len = 0;
        self.cur_x = 0;
        self.cur_y = 0;

        let dir = trxdir & 0x3;
        if dir == 3 {
            self.mode = TransferMode::Idle;
            return;
        }

        let rrw = regs.trxreg.rrw();
        let rrh = regs.trxreg.rrh();
        if rrw == 0 || rrh == 0 {
            self.mode = TransferMode::Idle;
            return;
        }

        self.sbp = regs.bitbltbuf.sbp();
        self.sbw = regs.bitbltbuf.sbw();
        self.dbp = regs.bitbltbuf.dbp();
        self.dbw = regs.bitbltbuf.dbw();
        self.ssax = regs.trxpos.ssax();
        self.ssay = regs.trxpos.ssay();
        self.dsax = regs.trxpos.dsax();
        self.dsay = regs.trxpos.dsay();
        self.rrw = rrw;
        self.rrh = rrh;

        match dir {
            0 => {
                let Some(dst_fmt) = PixelFormat::from_raw(regs.bitbltbuf.dpsm_raw()) else {
                    warn!(
                        dpsm = regs.bitbltbuf.dpsm_raw(),
                        "Unsupported DPSM for TRXDIR=0 (Host->Local) transfer"
                    );
                    self.mode = TransferMode::Idle;
                    return;
                };
                self.dst_format = dst_fmt;
                self.mode = TransferMode::HostToLocal;
            }
            1 => {
                let Some(src_fmt) = PixelFormat::from_raw(regs.bitbltbuf.spsm_raw()) else {
                    warn!(
                        spsm = regs.bitbltbuf.spsm_raw(),
                        "Unsupported SPSM for TRXDIR=1 (Local->Host) transfer"
                    );
                    self.mode = TransferMode::Idle;
                    return;
                };
                self.src_format = src_fmt;
                self.mode = TransferMode::LocalToHost;
            }
            2 => {
                self.mode = TransferMode::Idle;
                let Some(src_fmt) = PixelFormat::from_raw(regs.bitbltbuf.spsm_raw()) else {
                    warn!(
                        spsm = regs.bitbltbuf.spsm_raw(),
                        "Unsupported SPSM for TRXDIR=2 (Local->Local) transfer"
                    );
                    return;
                };
                let Some(dst_fmt) = PixelFormat::from_raw(regs.bitbltbuf.dpsm_raw()) else {
                    warn!(
                        dpsm = regs.bitbltbuf.dpsm_raw(),
                        "Unsupported DPSM for TRXDIR=2 (Local->Local) transfer"
                    );
                    return;
                };
                if src_fmt.bpp() != dst_fmt.bpp() {
                    warn!(
                        ?src_fmt,
                        ?dst_fmt,
                        "TRXDIR=2 (Local->Local) requires equal bits-per-pixel; ignoring"
                    );
                    return;
                }
                self.src_format = src_fmt;
                self.dst_format = dst_fmt;
                self.execute_local_to_local(vram, regs.trxpos.dir());
            }
            _ => {
                self.mode = TransferMode::Idle;
            }
        }
    }

    /// Feed one 64-bit `HWREG` (`0x54`) word during an active `Host -> Local` transfer.
    pub fn write_hwreg(&mut self, vram: &mut GsVram, data: u64) {
        if self.mode != TransferMode::HostToLocal {
            return;
        }
        let bpp = self.dst_format.bpp();
        if bpp == 24 {
            let chunk = data.to_le_bytes();
            let end = self.pending_len + 8;
            self.pending_bytes[self.pending_len..end].copy_from_slice(&chunk);
            self.pending_len = end;

            let mut consumed = 0usize;
            while consumed + 3 <= self.pending_len && self.cur_y < self.rrh {
                let pixel = u32::from_le_bytes([
                    self.pending_bytes[consumed],
                    self.pending_bytes[consumed + 1],
                    self.pending_bytes[consumed + 2],
                    0,
                ]);
                self.write_dst_pixel(vram, pixel);
                consumed += 3;
            }
            if self.cur_y >= self.rrh {
                self.mode = TransferMode::Idle;
                self.pending_len = 0;
            } else if consumed > 0 {
                let rem = self.pending_len - consumed;
                self.pending_bytes
                    .copy_within(consumed..self.pending_len, 0);
                self.pending_len = rem;
            }
            return;
        }

        let pixels_per_word = 64 / u32::from(bpp);
        let mask = if bpp == 32 {
            u64::from(u32::MAX)
        } else {
            (1u64 << bpp) - 1
        };

        for i in 0..pixels_per_word {
            if self.cur_y >= self.rrh {
                break;
            }
            let shift = i * u32::from(bpp);
            let pixel = ((data >> shift) & mask) as u32;
            self.write_dst_pixel(vram, pixel);
        }

        if self.cur_y >= self.rrh {
            self.mode = TransferMode::Idle;
        }
    }

    /// Drain one 64-bit word from an active `Local -> Host` (`TRXDIR=1`) transfer.
    pub fn read_local_to_host_u64(&mut self, vram: &GsVram) -> Option<u64> {
        if self.mode != TransferMode::LocalToHost {
            return None;
        }
        let bpp = self.src_format.bpp();
        if bpp == 24 {
            while self.pending_len < 8 && self.cur_y < self.rrh {
                let px = self.read_src_pixel(vram);
                let bytes = px.to_le_bytes();
                self.pending_bytes[self.pending_len..self.pending_len + 3]
                    .copy_from_slice(&bytes[0..3]);
                self.pending_len += 3;
            }
            if self.pending_len == 0 {
                self.mode = TransferMode::Idle;
                return None;
            }
            let take = self.pending_len.min(8);
            let mut out = [0u8; 8];
            out[..take].copy_from_slice(&self.pending_bytes[..take]);
            let rem = self.pending_len - take;
            self.pending_bytes.copy_within(take..self.pending_len, 0);
            self.pending_len = rem;
            if self.pending_len == 0 && self.cur_y >= self.rrh {
                self.mode = TransferMode::Idle;
            }
            return Some(u64::from_le_bytes(out));
        }

        if self.cur_y >= self.rrh {
            self.mode = TransferMode::Idle;
            return None;
        }

        let pixels_per_word = 64 / u32::from(bpp);
        let mut acc = 0u64;
        for i in 0..pixels_per_word {
            if self.cur_y >= self.rrh {
                break;
            }
            let px = self.read_src_pixel(vram);
            acc |= u64::from(px) << (i * u32::from(bpp));
        }
        if self.cur_y >= self.rrh {
            self.mode = TransferMode::Idle;
        }
        Some(acc)
    }

    /// Drain one 128-bit `Reg128` quadword from an active `Local -> Host` (`TRXDIR=1`) transfer.
    pub fn read_local_to_host_qwc(&mut self, vram: &GsVram) -> Option<Reg128> {
        let lo = self.read_local_to_host_u64(vram)?;
        let hi = self.read_local_to_host_u64(vram).unwrap_or(0);
        Some(Reg128::from_u64_pair(lo, hi))
    }

    #[inline]
    fn write_dst_pixel(&mut self, vram: &mut GsVram, pixel: u32) {
        let x = (u32::from(self.dsax) + u32::from(self.cur_x)) & 0x7FF;
        let y = (u32::from(self.dsay) + u32::from(self.cur_y)) & 0x7FF;
        vram.write_pixel_raw(self.dst_format, self.dbp, self.dbw, x, y, pixel);
        self.advance_cursor();
    }

    #[inline]
    fn read_src_pixel(&mut self, vram: &GsVram) -> u32 {
        let x = (u32::from(self.ssax) + u32::from(self.cur_x)) & 0x7FF;
        let y = (u32::from(self.ssay) + u32::from(self.cur_y)) & 0x7FF;
        let px = vram.read_pixel_raw(self.src_format, self.sbp, self.sbw, x, y);
        self.advance_cursor();
        px
    }

    #[inline]
    fn advance_cursor(&mut self) {
        self.cur_x += 1;
        if self.cur_x >= self.rrw {
            self.cur_x = 0;
            self.cur_y += 1;
        }
    }

    fn execute_local_to_local(&self, vram: &mut GsVram, dir: u8) {
        let rev_y = (dir & 1) != 0;
        let rev_x = (dir & 2) != 0;
        let w = u32::from(self.rrw);
        let h = u32::from(self.rrh);

        for row in 0..h {
            let dy = if rev_y { h - 1 - row } else { row };
            let sy = (u32::from(self.ssay) + dy) & 0x7FF;
            let dst_y = (u32::from(self.dsay) + dy) & 0x7FF;
            for col in 0..w {
                let dx = if rev_x { w - 1 - col } else { col };
                let sx = (u32::from(self.ssax) + dx) & 0x7FF;
                let dst_x = (u32::from(self.dsax) + dx) & 0x7FF;
                let px = vram.read_pixel_raw(self.src_format, self.sbp, self.sbw, sx, sy);
                vram.write_pixel_raw(self.dst_format, self.dbp, self.dbw, dst_x, dst_y, px);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regs::gs_reg;
    use crate::GsState;

    fn setup_transfer(
        gs: &mut GsState,
        dir: u8,
        fmt: PixelFormat,
        (bp, bw): (u32, u32),
        (x, y): (u16, u16),
        (w, h): (u16, u16),
    ) {
        let psm = u64::from((fmt as u8) & 0x3F);
        let (bitblt, trxpos) = if dir == 0 {
            (
                (u64::from(bp & 0x3FFF) << 32) | (u64::from(bw & 0x3F) << 48) | (psm << 56),
                (u64::from(x) << 32) | (u64::from(y) << 48),
            )
        } else {
            (
                u64::from(bp & 0x3FFF) | (u64::from(bw & 0x3F) << 16) | (psm << 24),
                u64::from(x) | (u64::from(y) << 16),
            )
        };
        let trxreg = u64::from(w) | (u64::from(h) << 32);
        gs.write_internal_reg(gs_reg::BITBLTBUF, bitblt);
        gs.write_internal_reg(gs_reg::TRXPOS, trxpos);
        gs.write_internal_reg(gs_reg::TRXREG, trxreg);
        gs.write_internal_reg(gs_reg::TRXDIR, u64::from(dir));
    }

    #[test]
    fn host_local_host_roundtrip_across_all_13_pixel_formats() {
        let formats = [
            PixelFormat::Psmct32,
            PixelFormat::Psmct24,
            PixelFormat::Psmct16,
            PixelFormat::Psmct16s,
            PixelFormat::Psmt8,
            PixelFormat::Psmt4,
            PixelFormat::Psmt8h,
            PixelFormat::Psmt4hl,
            PixelFormat::Psmt4hh,
            PixelFormat::Psmz32,
            PixelFormat::Psmz24,
            PixelFormat::Psmz16,
            PixelFormat::Psmz16s,
        ];

        for fmt in formats {
            let mut gs = GsState::new();
            let bpp = u32::from(fmt.bpp());
            let mask: u32 = if bpp == 32 {
                u32::MAX
            } else {
                (1u32 << bpp) - 1
            };

            // Use an unaligned odd width/height so partial HWREG/QWC tail padding is tested too!
            let (w, h) = (5u16, 3u16);
            setup_transfer(&mut gs, 0, fmt, (0, 2), (3, 5), (w, h));

            // Encode 15 pixels deterministically into a byte/nibble stream padded to 8-byte words.
            let expected: Vec<u32> = (0..15u32)
                .map(|i| (0x9A87_6543u32.wrapping_mul(i + 1) ^ (i << 19)) & mask)
                .collect();

            let mut host_words = Vec::new();
            if bpp == 24 {
                let mut stream = Vec::new();
                for &px in &expected {
                    stream.extend_from_slice(&px.to_le_bytes()[0..3]);
                }
                while stream.len() % 8 != 0 {
                    stream.push(0);
                }
                for chunk in stream.chunks_exact(8) {
                    host_words.push(u64::from_le_bytes(chunk.try_into().unwrap()));
                }
            } else {
                let per_word = 64 / bpp;
                let mut idx = 0usize;
                while idx < expected.len() {
                    let mut word = 0u64;
                    for slot in 0..per_word {
                        if idx < expected.len() {
                            word |= u64::from(expected[idx]) << (slot * bpp);
                            idx += 1;
                        }
                    }
                    host_words.push(word);
                }
            }

            for word in &host_words {
                gs.write_internal_reg(gs_reg::HWREG, *word);
            }
            assert_eq!(
                gs.transfer.mode(),
                TransferMode::Idle,
                "{fmt:?} did not complete"
            );

            // Check VRAM pixel-by-pixel.
            for dy in 0..h {
                for dx in 0..w {
                    let got =
                        gs.vram
                            .read_pixel_raw(fmt, 0, 2, 3 + u32::from(dx), 5 + u32::from(dy));
                    assert_eq!(
                        got,
                        expected[usize::from(dy * w + dx)],
                        "{fmt:?} at ({dx},{dy})"
                    );
                }
            }

            // Read back via Local -> Host (`TRXDIR = 1`).
            setup_transfer(&mut gs, 1, fmt, (0, 2), (3, 5), (w, h));
            let mut read_words = Vec::new();
            while let Some(w64) = gs.read_local_to_host_u64() {
                read_words.push(w64);
            }
            assert_eq!(read_words, host_words, "{fmt:?} Local->Host mismatch");
            assert_eq!(gs.transfer.mode(), TransferMode::Idle);
        }
    }

    #[test]
    fn host_to_local_psmct24_preserves_upper_8h_plane() {
        let mut gs = GsState::new();
        // Pre-seed bits 24..31 via PSMT8H at (0,0)..(3,0).
        setup_transfer(&mut gs, 0, PixelFormat::Psmt8h, (0, 1), (0, 0), (4, 1));
        gs.write_internal_reg(gs_reg::HWREG, 0x4433_2211);

        // Now upload 3x1 (odd 9-byte span crossing 2 HWREGs) via PSMCT24.
        setup_transfer(&mut gs, 0, PixelFormat::Psmct24, (0, 1), (0, 0), (3, 1));
        // p0 = 0xAA_BB_CC, p1 = 0x12_34_56, p2 = 0x78_9A_BC
        let b: [u8; 16] = [
            0xCC, 0xBB, 0xAA, 0x56, 0x34, 0x12, 0xBC, 0x9A, 0x78, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF,
        ];
        gs.write_internal_reg(
            gs_reg::HWREG,
            u64::from_le_bytes(b[0..8].try_into().unwrap()),
        );
        gs.write_internal_reg(
            gs_reg::HWREG,
            u64::from_le_bytes(b[8..16].try_into().unwrap()),
        );

        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0),
            0x11AA_BBCC
        );
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 0),
            0x2212_3456
        );
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 2, 0),
            0x3378_9ABC
        );
        // Pixel (3,0) untouched.
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 3, 0),
            0x4400_0000
        );
    }

    #[test]
    fn local_to_local_all_four_dir_modes_and_2048_wrap() {
        let mut gs = GsState::new();
        let bitblt = (1u64 << 16) | (1u64 << 48); // sbw=1, dbw=1, PSMCT32

        // Write a 3x3 pattern at (2046, 2047) testing 2048 X/Y wrap.
        let vals = [1u32, 2, 3, 4, 5, 6, 7, 8, 9];
        for (i, &v) in vals.iter().enumerate() {
            let x = (2046 + (i as u32 % 3)) & 0x7FF;
            let y = (2047 + (i as u32 / 3)) & 0x7FF;
            gs.vram.write_pixel_raw(PixelFormat::Psmct32, 0, 1, x, y, v);
        }

        // Overlapping self-blit shifted (+1, +1) using DIR=3 (LR -> UL) preserves data cleanly.
        gs.write_internal_reg(gs_reg::BITBLTBUF, bitblt);
        let trxpos_dir3 = 2046u64 | (2047u64 << 16) | (2047u64 << 32) | (3u64 << 59);
        gs.write_internal_reg(gs_reg::TRXPOS, trxpos_dir3);
        gs.write_internal_reg(gs_reg::TRXREG, 3 | (3 << 32));
        gs.write_internal_reg(gs_reg::TRXDIR, 2);

        for (i, &v) in vals.iter().enumerate() {
            let x = (2047 + (i as u32 % 3)) & 0x7FF;
            let y = (i as u32 / 3) & 0x7FF;
            assert_eq!(gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, x, y), v);
        }

        // Non-overlapping blits in DIR=0, 1, 2 to separate destination base pointer dbp=4.
        for dir in 0..=2u64 {
            let dst_bp = 4u64 + dir * 4;
            gs.write_internal_reg(
                gs_reg::BITBLTBUF,
                (1u64 << 16) | (dst_bp << 32) | (1u64 << 48),
            );
            let trxpos = 2047u64 | (dir << 59);
            gs.write_internal_reg(gs_reg::TRXPOS, trxpos);
            gs.write_internal_reg(gs_reg::TRXREG, 3 | (3 << 32));
            gs.write_internal_reg(gs_reg::TRXDIR, 2);

            for (i, &v) in vals.iter().enumerate() {
                let x = i as u32 % 3;
                let y = i as u32 / 3;
                assert_eq!(
                    gs.vram
                        .read_pixel_raw(PixelFormat::Psmct32, dst_bp as u32, 1, x, y),
                    v,
                    "DIR={dir} mismatch"
                );
            }
        }
    }

    #[test]
    fn local_to_local_bpp_mismatch_is_ignored_without_corrupting_vram() {
        let mut gs = GsState::new();
        gs.vram
            .write_pixel_raw(PixelFormat::Psmct32, 1, 1, 0, 0, 0xCAFE_BABE);
        // Source PSMCT32 (32bpp), Dest PSMCT16 (16bpp) -> mismatch -> no-op.
        let bitblt = (1u64 << 16)
            | (1u64 << 32)
            | (1u64 << 48)
            | (u64::from(PixelFormat::Psmct16 as u8) << 56);
        gs.write_internal_reg(gs_reg::BITBLTBUF, bitblt);
        gs.write_internal_reg(gs_reg::TRXPOS, 0);
        gs.write_internal_reg(gs_reg::TRXREG, 1 | (1 << 32));
        gs.write_internal_reg(gs_reg::TRXDIR, 2);
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 1, 1, 0, 0),
            0xCAFE_BABE
        );
    }

    #[test]
    fn read_local_to_host_qwc_handles_unaligned_24bpp_transfer() {
        let mut gs = GsState::new();
        setup_transfer(&mut gs, 0, PixelFormat::Psmct24, (0, 1), (0, 0), (3, 1));
        gs.write_internal_reg(gs_reg::HWREG, 0x9A_BC_12_34_56_AA_BB_CC);
        gs.write_internal_reg(gs_reg::HWREG, 0x78);

        setup_transfer(&mut gs, 1, PixelFormat::Psmct24, (0, 1), (0, 0), (3, 1));
        let q0 = gs.read_local_to_host_qwc().expect("expected qwc");
        assert_eq!(q0.lo_u64(), 0x9A_BC_12_34_56_AA_BB_CC);
        assert_eq!(q0.hi_u64(), 0x78);
        assert!(gs.read_local_to_host_qwc().is_none());
    }
}
