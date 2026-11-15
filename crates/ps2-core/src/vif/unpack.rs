//! `VIF0` / `VIF1` `UNPACK` (`0x60..=0x7F`), `STCYCL`, `STMOD`, `STMASK`, `STROW`, `STCOL` decoder.
//!
//! Primary source references (`FETCHED`):
//! - `local/ps2tek/index.html` (`#vifcommands`, `#vifunpack`):
//!   * Opcode bits: `M = bit 4`, `vn = bits 2..=3` (`0=S, 1=V2, 2=V3, 3=V4`), `vl = bits 0..=1`
//!     (`0=32-bit, 1=16-bit, 2=8-bit, 3=V4-5` only when `vn == 3`).
//!   * `IMMEDIATE`: `ADDR = bits 0..=9` (QWC offset), `USN = bit 14`, `FLG = bit 15` (`VIF1`: `+ TOPS`).
//!   * `NUM`: `0 => 256` vectors. Input stream is byte-packed per vector, then padded to the next
//!     32-bit word boundary at the end of the `UNPACK` command.
//! - `local/ps2autotests/tests/dma/vif/unpack.expected`:
//!   * `V2-*` duplicates `X, Y` into `Z, W` (`[x, y, x, y]`).
//!   * `V3-*` fills `W = 0` (`[x, y, z, 0]`).
//!   * `V4-5` (`0x6F`/`0x7F`) unpacks `0xFFFF -> (0xF8, 0xF8, 0xF8, 0x80)` (`r<<3, g<<3, b<<3, a<<7`).
//! - `local/ps2autotests/tests/dma/vif/stcycl.expected`:
//!   * Destination quadword address wraps modulo `256` QWC (`4 KB`, `& 0xFF`) on `VIF0` and
//!     modulo `1024` QWC (`16 KB`, `& 0x3FF`) on `VIF1`.
//!   * `WL == 0` consumes `0` input vectors regardless of `CL`, never resets `write_cycle`, and
//!     writes `NUM` fill vectors sequentially with `col = min(3, write_cycle)`.
//! - `local/ps2autotests/tests/dma/vif/stmod.expected`:
//!   * Mask pairs (`shift = min(3, write_cycle) * 8 + lane * 2`):
//!     `00 = Data` (or `ROW` in fill slot), `01 = ROW[lane]` (never updates `ROW`),
//!     `10 = COL[col]`, `11 = WriteProtect`.
//!   * `MODE` (`00 = Data` lanes only):
//!     `0 = Normal` (`val = elem`), `1 = Offset` (`val = elem + row[l]`, `row` unchanged),
//!     `2 = Difference` (`val = elem + row[l]`, `row[l] = val`),
//!     `3 = Write-through to ROW` (`val = elem`, `row[l] = val`; writes both VU data memory & `ROW`).

/// Quadword index wrap mask for `VIF0` (`VU0` Data Memory = `4 KB` = `256` QWCs).
pub const VIF0_DATA_QWC_MASK: usize = 0xFF;

/// Quadword index wrap mask for `VIF1` (`VU1` Data Memory = `16 KB` = `1024` QWCs).
pub const VIF1_DATA_QWC_MASK: usize = 0x3FF;

/// Decoded parameters of a `VIF` `UNPACK` (`0x60..=0x7F`) instruction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UnpackCmd {
    /// Vector dimension (`0 = S`, `1 = V2`, `2 = V3`, `3 = V4`).
    pub vn: u8,
    /// Element bit width (`0 = 32`, `1 = 16`, `2 = 8`, `3 = 5+5+5+1`).
    pub vl: u8,
    /// Mask enable bit (`CMD` bit 4 / VIFcode bit 28).
    pub masked: bool,
    /// Unsigned zero-extend bit (`IMMEDIATE` bit 14).
    pub usn: bool,
    /// `FLG` bit (`IMMEDIATE` bit 15): add `TOPS` on `VIF1`.
    pub flg: bool,
    /// Base destination quadword address (`IMMEDIATE` bits `0..=9`).
    pub addr_qwc: u16,
    /// Output vector count (`1..=256`, where raw `NUM == 0` encodes `256`).
    pub num_vectors: u16,
}

impl UnpackCmd {
    /// Decode an `UNPACK` command from raw 32-bit `vifcode` (`cmd_id = 0x60..=0x7F`).
    #[inline]
    pub const fn decode(vifcode: u32) -> Self {
        let cmd = ((vifcode >> 24) & 0x7F) as u8;
        let raw_num = ((vifcode >> 16) & 0xFF) as u16;
        let imm = (vifcode & 0xFFFF) as u16;
        Self {
            vn: (cmd >> 2) & 0x03,
            vl: cmd & 0x03,
            masked: (cmd & 0x10) != 0,
            usn: (imm & 0x4000) != 0,
            flg: (imm & 0x8000) != 0,
            addr_qwc: imm & 0x03FF,
            num_vectors: if raw_num == 0 { 256 } else { raw_num },
        }
    }

    /// Return `true` when `(vn, vl)` selects one of the 13 valid `UNPACK` formats.
    ///
    /// Invalid encodings (`vl == 3 && vn != 3`: `0x03`, `0x07`, `0x0B`) trigger `ER1` if unmasked.
    #[inline]
    pub const fn is_valid_format(vn: u8, vl: u8) -> bool {
        !(vl == 3 && vn != 3)
    }

    /// Return exact input byte size per vector (`1..=16`) for valid `(vn, vl)` combinations.
    #[inline]
    pub const fn vector_bytes(vn: u8, vl: u8) -> usize {
        match vl {
            0 => (vn as usize + 1) * 4,
            1 => (vn as usize + 1) * 2,
            2 => vn as usize + 1,
            _ => 2, // `V4-5` (`vn == 3, vl == 3`)
        }
    }
}

/// Resumable state for an in-progress `UNPACK` transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnpackState {
    /// Whether an `UNPACK` command is currently executing.
    pub active: bool,
    /// Vector dimension (`0..=3`).
    pub vn: u8,
    /// Element bit width (`0..=3`).
    pub vl: u8,
    /// Whether `VIFn_MASK` applies during writes.
    pub masked: bool,
    /// Unsigned zero-extend flag.
    pub usn: bool,
    /// Current target quadword offset (wrapped modulo `data_qwc_mask` on write).
    pub qwc: u16,
    /// Remaining output vectors (`1..=256`).
    pub remaining: u16,
    /// Latched `STCYCL.CL` value for this `UNPACK`.
    pub cl: u16,
    /// Latched `STCYCL.WL` value for this `UNPACK`.
    pub wl: u16,
    /// Current `STCYCL` sub-cycle index.
    pub write_cycle: u16,
    /// Input byte size of one unpacked vector (`1..=16`).
    pub vec_bytes: u8,
    /// Partial input byte staging buffer (max 15 carried bytes + 4 newly pushed word bytes = 19).
    pub byte_buf: [u8; 20],
    /// Number of valid input bytes stored at the front of `byte_buf`.
    pub byte_len: u8,
}

impl Default for UnpackState {
    fn default() -> Self {
        Self::new()
    }
}

impl UnpackState {
    /// Construct an idle `UnpackState`.
    pub const fn new() -> Self {
        Self {
            active: false,
            vn: 0,
            vl: 0,
            masked: false,
            usn: false,
            qwc: 0,
            remaining: 0,
            cl: 1,
            wl: 1,
            write_cycle: 0,
            vec_bytes: 0,
            byte_buf: [0; 20],
            byte_len: 0,
        }
    }

    /// Reset all active `UNPACK` state back to idle.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Initialize `UnpackState` from a decoded `UNPACK` command and current `CYCLE`/`TOPS`.
    pub fn begin(&mut self, cmd: UnpackCmd, cycle: u16, tops: u16, is_vif1: bool) {
        let mut start_qwc = cmd.addr_qwc;
        if is_vif1 && cmd.flg {
            start_qwc = start_qwc.wrapping_add(tops) & 0x03FF;
        }
        self.active = true;
        self.vn = cmd.vn;
        self.vl = cmd.vl;
        self.masked = cmd.masked;
        self.usn = cmd.usn;
        self.qwc = start_qwc;
        self.remaining = cmd.num_vectors;
        self.cl = cycle & 0x00FF;
        self.wl = (cycle >> 8) & 0x00FF;
        self.write_cycle = 0;
        self.vec_bytes = UnpackCmd::vector_bytes(cmd.vn, cmd.vl) as u8;
        self.byte_len = 0;
    }

    /// Return `true` if the next output slot is a fill slot (no input data consumed).
    #[inline]
    pub const fn next_slot_is_fill(&self) -> bool {
        self.wl == 0 || self.write_cycle >= self.cl
    }

    /// Return `true` if `UNPACK` needs at least one more 32-bit input word before writing the next vector.
    #[inline]
    pub const fn needs_input_word(&self) -> bool {
        self.active
            && !self.next_slot_is_fill()
            && (self.byte_len as usize) < (self.vec_bytes as usize)
    }

    /// Push one 32-bit little-endian input word into the partial vector byte buffer.
    #[inline]
    pub fn push_word(&mut self, word: u32) {
        let pos = self.byte_len as usize;
        debug_assert!(pos + 4 <= self.byte_buf.len());
        let bytes = word.to_le_bytes();
        self.byte_buf[pos..pos + 4].copy_from_slice(&bytes);
        self.byte_len = (pos + 4) as u8;
    }

    /// Advance `UNPACK` as far as currently buffered bytes (and fill slots) permit.
    ///
    /// Returns `true` when the entire `UNPACK` command finishes (`remaining == 0`), discarding any
    /// remaining partial-word alignment bytes cleanly at the end of the transfer.
    pub fn step(
        &mut self,
        data_mem: &mut [u8],
        data_qwc_mask: usize,
        mode: u8,
        mask: u32,
        row: &mut [u32; 4],
        col: &[u32; 4],
    ) -> bool {
        while self.active {
            let is_fill = self.next_slot_is_fill();
            if is_fill {
                let col_idx = self.write_cycle.min(3) as usize;
                write_fill_qwc(
                    data_mem,
                    self.qwc,
                    data_qwc_mask,
                    self.masked,
                    mask,
                    col_idx,
                    row,
                    col,
                );
            } else {
                let need = self.vec_bytes as usize;
                if (self.byte_len as usize) < need {
                    return false;
                }
                let vec = decode_vector(&self.byte_buf[..need], self.vn, self.vl, self.usn);
                let leftover = (self.byte_len as usize) - need;
                if leftover > 0 {
                    self.byte_buf.copy_within(need..self.byte_len as usize, 0);
                }
                self.byte_len = leftover as u8;

                let col_idx = self.write_cycle.min(3) as usize;
                write_data_qwc(
                    data_mem,
                    self.qwc,
                    data_qwc_mask,
                    vec,
                    self.masked,
                    mode,
                    mask,
                    col_idx,
                    row,
                    col,
                );
            }

            self.remaining -= 1;
            if self.wl == 0 {
                // Infinite-fill mode (`stcycl.expected` lines 3/12/21/30/39): never resets cycle
                // and advances sequentially (`qwc += 1`) regardless of `CL`.
                self.qwc = self.qwc.wrapping_add(1);
                self.write_cycle = self.write_cycle.saturating_add(1);
            } else if self.cl < self.wl {
                // Filling write: `WL` slots written consecutively per cycle, resetting to `0`.
                self.qwc = self.qwc.wrapping_add(1);
                self.write_cycle += 1;
                if self.write_cycle >= self.wl {
                    self.write_cycle = 0;
                }
            } else {
                // Skipping write (`CL >= WL > 0`): writes `WL` consecutive slots then jumps `CL - WL`.
                self.write_cycle += 1;
                if self.write_cycle >= self.wl {
                    self.qwc = self.qwc.wrapping_add(1 + (self.cl - self.wl));
                    self.write_cycle = 0;
                } else {
                    self.qwc = self.qwc.wrapping_add(1);
                }
            }

            if self.remaining == 0 {
                self.active = false;
                self.byte_len = 0;
                return true;
            }
        }
        true
    }
}

/// Decode one compressed input vector into 4 x 32-bit `XYZW` lanes (`unpack.expected`).
#[inline]
fn decode_vector(bytes: &[u8], vn: u8, vl: u8, usn: bool) -> [u32; 4] {
    if vl == 3 {
        // `V4-5` (`RGBA5551`): verified in `unpack.expected` (`0xFFFF -> F8, F8, F8, 80`).
        let raw = u16::from_le_bytes([bytes[0], bytes[1]]);
        let r = u32::from(raw & 0x1F) << 3;
        let g = u32::from((raw >> 5) & 0x1F) << 3;
        let b = u32::from((raw >> 10) & 0x1F) << 3;
        let a = u32::from((raw >> 15) & 0x01) << 7;
        return [r, g, b, a];
    }

    let mut elems = [0u32; 4];
    let count = (vn as usize) + 1;
    match vl {
        0 => {
            for (i, out) in elems.iter_mut().take(count).enumerate() {
                let off = i * 4;
                *out = u32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
            }
        }
        1 => {
            for (i, out) in elems.iter_mut().take(count).enumerate() {
                let off = i * 2;
                let raw = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
                *out = if usn {
                    u32::from(raw)
                } else {
                    (raw as i16 as i32) as u32
                };
            }
        }
        2 => {
            for (i, out) in elems.iter_mut().take(count).enumerate() {
                let raw = bytes[i];
                *out = if usn {
                    u32::from(raw)
                } else {
                    (raw as i8 as i32) as u32
                };
            }
        }
        _ => {}
    }

    match vn {
        0 => [elems[0], elems[0], elems[0], elems[0]], // `S-*`: broadcast `X` to `XYZW`
        1 => [elems[0], elems[1], elems[0], elems[1]], // `V2-*`: duplicate `XY` into `ZW` (`unpack.expected`)
        2 => [elems[0], elems[1], elems[2], 0],        // `V3-*`: `W = 0` (`unpack.expected`)
        _ => elems,                                    // `V4-*`
    }
}

/// Write a single 32-bit lane to VU Data Memory at `qwc_idx` (wrapped modulo `data_qwc_mask`).
#[inline(always)]
fn write_vu_data_lane(
    data_mem: &mut [u8],
    qwc_idx: u16,
    data_qwc_mask: usize,
    lane: usize,
    val: u32,
) {
    let wrapped_qwc = (qwc_idx as usize) & data_qwc_mask;
    let byte_off = wrapped_qwc * 16 + lane * 4;
    if byte_off + 4 <= data_mem.len() {
        data_mem[byte_off..byte_off + 4].copy_from_slice(&val.to_le_bytes());
    }
}

/// Write an active input vector using `VIFn_MASK` and `VIFn_MODE` (`stmod.expected`).
#[allow(clippy::too_many_arguments)]
fn write_data_qwc(
    data_mem: &mut [u8],
    qwc_idx: u16,
    data_qwc_mask: usize,
    vec: [u32; 4],
    masked: bool,
    mode: u8,
    mask: u32,
    col_idx: usize,
    row: &mut [u32; 4],
    col: &[u32; 4],
) {
    let col_shift = (col_idx.min(3) as u32) * 8;
    for (lane, &elem) in vec.iter().enumerate() {
        let mask_sel = if masked {
            (mask >> (col_shift + (lane as u32) * 2)) & 0x3
        } else {
            0
        };
        match mask_sel {
            0 => {
                // `00` = Data lane: `MODE` applies and `Difference`/`Mode3` update `ROW[lane]`.
                let val = match mode & 0x3 {
                    0 => elem,
                    1 => elem.wrapping_add(row[lane]),
                    2 => {
                        let sum = elem.wrapping_add(row[lane]);
                        row[lane] = sum;
                        sum
                    }
                    _ => {
                        // Mode 3 (`stmod.expected` lines 51-65): writes `elem` to BOTH VU memory & `ROW`.
                        row[lane] = elem;
                        elem
                    }
                };
                write_vu_data_lane(data_mem, qwc_idx, data_qwc_mask, lane, val);
            }
            1 => {
                // `01` = `ROW[lane]` (`ROW` register never modified).
                write_vu_data_lane(data_mem, qwc_idx, data_qwc_mask, lane, row[lane]);
            }
            2 => {
                // `10` = `COL[col_idx]` (`ROW` register never modified).
                write_vu_data_lane(data_mem, qwc_idx, data_qwc_mask, lane, col[col_idx.min(3)]);
            }
            _ => {
                // `11` = Write-protected.
            }
        }
    }
}

/// Write a `STCYCL` fill vector (`0` input bytes consumed, `00` Data mask falls back to `ROW`).
#[allow(clippy::too_many_arguments)]
fn write_fill_qwc(
    data_mem: &mut [u8],
    qwc_idx: u16,
    data_qwc_mask: usize,
    masked: bool,
    mask: u32,
    col_idx: usize,
    row: &[u32; 4],
    col: &[u32; 4],
) {
    let col_shift = (col_idx.min(3) as u32) * 8;
    for (lane, &row_val) in row.iter().enumerate() {
        let mask_sel = if masked {
            (mask >> (col_shift + (lane as u32) * 2)) & 0x3
        } else {
            0
        };
        match mask_sel {
            0 | 1 => write_vu_data_lane(data_mem, qwc_idx, data_qwc_mask, lane, row_val),
            2 => write_vu_data_lane(data_mem, qwc_idx, data_qwc_mask, lane, col[col_idx.min(3)]),
            _ => {}
        }
    }
}
