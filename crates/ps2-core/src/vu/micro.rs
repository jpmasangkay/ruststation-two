//! Functional (untimed) VU micro-mode and macro-mode instruction set interpreter.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#vu`, `#vuupper`, `#vulower`, `#vumemory`, `#vuspecialregs`, `FETCHED`):
//!   * Each 64-bit LIW instruction has a 32-bit lower word (`bits 0..31`) and a 32-bit upper word
//!     (`bits 32..63`).
//!   * Upper word bit 31 (`I`) is `LOI`: the lower 32-bit word is loaded into the `I` register
//!     after the upper instruction reads the previous `I` value (`tests/vu/games/triace.cpp`, `FETCHED`).
//!   * Upper word bit 30 (`E`) ends the microprogram after a 1-instruction delay slot.
//!   * When both Upper and Lower instructions write to the same `VF` register in the same LIW,
//!     both read the pre-LIW `VF` state and the Upper instruction takes priority on overlapping lanes.
//! - `ps2autotests` (`tests/vu/assemble/ops.cpp`, `tests/vu/games/triace.cpp`, `tests/vu/lower/{branch,integer,random}.cpp`, `FETCHED`):
//!   * Exact opcode encodings and semantics for Upper Types 0–3 and Lower Types 1, 3, 4, 5, 7, 8, 9.

use ps2_common::ps2float;
use ps2_common::Reg128;

use super::pipeline;
use super::state::{VuState, DEST_W, DEST_X, DEST_Y, DEST_Z};

/// Upper instruction bit 31: `LOI` (lower 32-bit word is loaded into `I` register).
pub const UPPER_BIT_I: u32 = 1 << 31;
/// Upper instruction bit 30: `E` (end microprogram after 1 delay-slot instruction).
pub const UPPER_BIT_E: u32 = 1 << 30;

/// Check whether `lane` (`0 = X, 1 = Y, 2 = Z, 3 = W`) is enabled in the 4-bit `dest` mask.
#[inline(always)]
pub const fn lane_enabled(dest: u8, lane: usize) -> bool {
    (dest & (1 << (3 - (lane & 3)))) != 0
}

/// Sign-extend a 5-bit immediate (`bits 0..4`) to a 16-bit unsigned integer (`u16`).
#[inline(always)]
pub const fn simm5_to_u16(imm5: u32) -> u16 {
    let raw = (imm5 & 0x1F) as i8;
    let sext = (raw << 3) >> 3;
    sext as i16 as u16
}

/// Sign-extend an 11-bit immediate (`bits 0..10`) to a 16-bit signed integer (`i16`).
#[inline(always)]
pub const fn simm11_to_i16(imm11: u32) -> i16 {
    let raw = (imm11 & 0x7FF) as i16;
    (raw << 5) >> 5
}

/// Execute `VIADD id, is, it` (`vi[id] = vi[is].wrapping_add(vi[it])`).
#[inline(always)]
pub fn exec_viadd(vu: &mut VuState, id: usize, is: usize, it: usize) {
    let res = vu.read_vi(is).wrapping_add(vu.read_vi(it));
    vu.write_vi(id, res);
}

/// Execute `VISUB id, is, it` (`vi[id] = vi[is].wrapping_sub(vi[it])`).
#[inline(always)]
pub fn exec_visub(vu: &mut VuState, id: usize, is: usize, it: usize) {
    let res = vu.read_vi(is).wrapping_sub(vu.read_vi(it));
    vu.write_vi(id, res);
}

/// Execute `VIADDI it, is, imm5` (`vi[it] = vi[is].wrapping_add(sext5(imm5))`).
#[inline(always)]
pub fn exec_viaddi(vu: &mut VuState, it: usize, is: usize, imm5: u32) {
    let res = vu.read_vi(is).wrapping_add(simm5_to_u16(imm5));
    vu.write_vi(it, res);
}

/// Execute `VIAND id, is, it` (`vi[id] = vi[is] & vi[it]`).
#[inline(always)]
pub fn exec_viand(vu: &mut VuState, id: usize, is: usize, it: usize) {
    let res = vu.read_vi(is) & vu.read_vi(it);
    vu.write_vi(id, res);
}

/// Execute `VIOR id, is, it` (`vi[id] = vi[is] | vi[it]`).
#[inline(always)]
pub fn exec_vior(vu: &mut VuState, id: usize, is: usize, it: usize) {
    let res = vu.read_vi(is) | vu.read_vi(it);
    vu.write_vi(id, res);
}

/// Load enabled lanes of a 128-bit qword in `data_mem` at `qword_addr` into `vf[ft]`.
#[inline]
pub fn load_vf_from_qword(
    vu: &mut VuState,
    data_mem: &[u8],
    dest: u8,
    ft: usize,
    qword_addr: usize,
) {
    let mask = data_mem.len().saturating_sub(1) & !0xF;
    let base = (qword_addr.wrapping_mul(16)) & mask;
    for lane in 0..4 {
        if lane_enabled(dest, lane) {
            let off = base + lane * 4;
            let word = u32::from_le_bytes([
                data_mem[off],
                data_mem[off + 1],
                data_mem[off + 2],
                data_mem[off + 3],
            ]);
            vu.write_vf_lane(ft, lane, word);
        }
    }
}

/// Store enabled lanes of `vf[fs]` into a 128-bit qword in `data_mem` at `qword_addr`.
#[inline]
pub fn store_vf_to_qword(
    vu: &VuState,
    data_mem: &mut [u8],
    dest: u8,
    fs: usize,
    qword_addr: usize,
) {
    let mask = data_mem.len().saturating_sub(1) & !0xF;
    let base = (qword_addr.wrapping_mul(16)) & mask;
    let reg = vu.read_vf(fs);
    let lanes = reg.u32_lanes();
    for (lane, &word) in lanes.iter().enumerate() {
        if lane_enabled(dest, lane) {
            let off = base + lane * 4;
            data_mem[off..off + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
}

/// Load a 16-bit integer from a 128-bit qword in `data_mem` at `qword_addr` using `dest` mask.
#[inline]
pub fn load_vi_from_qword(
    vu: &mut VuState,
    data_mem: &[u8],
    dest: u8,
    it: usize,
    qword_addr: usize,
) {
    let mask = data_mem.len().saturating_sub(1) & !0xF;
    let base = (qword_addr.wrapping_mul(16)) & mask;
    let lane_offset = if (dest & DEST_X) != 0 {
        0
    } else if (dest & DEST_Y) != 0 {
        4
    } else if (dest & DEST_Z) != 0 {
        8
    } else if (dest & DEST_W) != 0 {
        12
    } else {
        return;
    };
    let addr = base + lane_offset;
    let val = u16::from_le_bytes([data_mem[addr], data_mem[addr + 1]]);
    vu.write_vi(it, val);
}

/// Store a 16-bit integer `vi[it]` (zero-extended to 32 bits per enabled lane) to `data_mem` at `qword_addr`.
#[inline]
pub fn store_vi_to_qword(
    vu: &VuState,
    data_mem: &mut [u8],
    dest: u8,
    it: usize,
    qword_addr: usize,
) {
    let mask = data_mem.len().saturating_sub(1) & !0xF;
    let base = (qword_addr.wrapping_mul(16)) & mask;
    let bytes = u32::from(vu.read_vi(it)).to_le_bytes();
    if (dest & DEST_X) != 0 {
        data_mem[base..base + 4].copy_from_slice(&bytes);
    }
    if (dest & DEST_Y) != 0 {
        data_mem[base + 4..base + 8].copy_from_slice(&bytes);
    }
    if (dest & DEST_Z) != 0 {
        data_mem[base + 8..base + 12].copy_from_slice(&bytes);
    }
    if (dest & DEST_W) != 0 {
        data_mem[base + 12..base + 16].copy_from_slice(&bytes);
    }
}

/// Execute `VILWR` / `ILWR` (`vi[it] = mem128[vi[is]].dest`).
#[inline]
pub fn exec_ilwr(vu: &mut VuState, data_mem: &[u8], dest: u8, it: usize, is: usize) {
    let qword_addr = usize::from(vu.read_vi(is));
    load_vi_from_qword(vu, data_mem, dest, it, qword_addr);
}

/// Execute `VISWR` / `ISWR` (`mem128[vi[is]].dest = vi[it]`).
#[inline]
pub fn exec_iswr(vu: &VuState, data_mem: &mut [u8], dest: u8, it: usize, is: usize) {
    let qword_addr = usize::from(vu.read_vi(is));
    store_vi_to_qword(vu, data_mem, dest, it, qword_addr);
}

/// Advance the 23-bit VU LFSR (`R`) by one step and return `0x3F80_0000 | R`
/// (`ps2tek` `#vuspecialregs` & `tests/vu/lower/random.expected`, `FETCHED`).
#[inline]
pub fn step_r_lfsr(vu: &mut VuState) -> u32 {
    let x = (vu.r >> 4) & 1;
    let y = (vu.r >> 22) & 1;
    vu.r = ((vu.r << 1) ^ x ^ y) & 0x007F_FFFF;
    0x3F80_0000 | vu.r
}

/// Convert a 32-bit signed integer to PS2 float scaled by $2^{-\text{shift}}$ (`ITOF0/4/12/15`).
#[inline]
fn vu_itof(bits: u32, shift: u8) -> u32 {
    let res = ps2float::cvt_s_w(bits as i32);
    if shift == 0 || (res & 0x7FFF_FFFF) == 0 {
        return res;
    }
    let sign = res & 0x8000_0000;
    let exp = ((res >> 23) & 0xFF) as i32 - i32::from(shift);
    let mant = res & 0x007F_FFFF;
    if exp <= 0 {
        sign
    } else {
        sign | ((exp as u32) << 23) | mant
    }
}

/// Convert a PS2 float scaled by $2^{\text{shift}}$ to a 32-bit signed integer (`FTOI0/4/12/15`).
#[inline]
fn vu_ftoi(bits: u32, shift: u8) -> u32 {
    if (bits & 0x7F80_0000) == 0 {
        return 0;
    }
    if shift == 0 {
        return ps2float::cvt_w_s(bits) as u32;
    }
    let sign = bits & 0x8000_0000;
    let exp = (((bits >> 23) & 0xFF) + u32::from(shift)).min(0xFF);
    let mant = bits & 0x007F_FFFF;
    let scaled = sign | (exp << 23) | mant;
    ps2float::cvt_w_s(scaled) as u32
}

/// Compute the 6-bit clipping judgement for `CLIPw.xyz fs, ft` (`ps2autotests` `tests/vu/upper/clip.cpp`, `FETCHED`).
#[inline]
pub fn compute_clip_bits(fs: Reg128, ft_w: u32) -> u32 {
    let w_mag = if (ft_w & 0x7F80_0000) == 0 {
        0
    } else {
        ft_w & 0x7FFF_FFFF
    };
    let lanes = fs.u32_lanes();
    let mut bits = 0u32;
    for (i, &c) in lanes[..3].iter().enumerate() {
        let c_mag = if (c & 0x7F80_0000) == 0 {
            0
        } else {
            c & 0x7FFF_FFFF
        };
        if c_mag > w_mag {
            if (c & 0x8000_0000) == 0 {
                bits |= 1 << (i * 2); // +x (0), +y (2), +z (4)
            } else {
                bits |= 1 << (i * 2 + 1); // -x (1), -y (3), -z (5)
            }
        }
    }
    bits
}

/// Execute a 32-bit Upper VU instruction on `vu`.
pub fn exec_upper_op(vu: &mut VuState, instr: u32) {
    let ft = ((instr >> 16) & 0x1F) as usize;
    let fs = ((instr >> 11) & 0x1F) as usize;
    let vs = vu.read_vf(fs).u32_lanes().to_owned();
    let vt = vu.read_vf(ft).u32_lanes().to_owned();
    let q_val = vu.q;
    let i_val = vu.i;
    exec_upper_op_with_sources(vu, instr, vs, vt, q_val, i_val);
}

/// Execute a 32-bit Upper VU instruction on `vu` using pre-LIW source inputs.
#[allow(clippy::needless_range_loop)]
pub fn exec_upper_op_with_sources(
    vu: &mut VuState,
    instr: u32,
    vs: [u32; 4],
    vt: [u32; 4],
    q_val: u32,
    i_val: u32,
) {
    let dest = ((instr >> 21) & 0xF) as u8;
    let ft = ((instr >> 16) & 0x1F) as usize;
    let fd = ((instr >> 6) & 0x1F) as usize;
    let funct6 = instr & 0x3F;
    let bc = (instr & 0x3) as usize;

    let acc = vu.acc.u32_lanes().to_owned();
    let vt_bc = vt[bc];

    match funct6 {
        // ADDbc (0x00..=0x03)
        0x00..=0x03 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::add(vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // SUBbc (0x04..=0x07)
        0x04..=0x07 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::sub(vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MADDbc (0x08..=0x0B)
        0x08..=0x0B => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::madd(acc[lane], vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MSUBbc (0x0C..=0x0F)
        0x0C..=0x0F => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::msub(acc[lane], vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MAXbc (0x10..=0x13)
        0x10..=0x13 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::max(vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r);
                }
            }
        }
        // MINIbc (0x14..=0x17)
        0x14..=0x17 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::min(vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r);
                }
            }
        }
        // MULbc (0x18..=0x1B)
        0x18..=0x1B => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::mul(vs[lane], vt_bc);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MULq (0x1C)
        0x1C => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::mul(vs[lane], q_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MAXi (0x1D)
        0x1D => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::max(vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r);
                }
            }
        }
        // MULi (0x1E)
        0x1E => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::mul(vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MINIi (0x1F)
        0x1F => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::min(vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r);
                }
            }
        }
        // ADDq (0x20)
        0x20 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::add(vs[lane], q_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MADDq (0x21)
        0x21 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::madd(acc[lane], vs[lane], q_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // ADDi (0x22)
        0x22 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::add(vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MADDi (0x23)
        0x23 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::madd(acc[lane], vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // SUBq (0x24)
        0x24 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::sub(vs[lane], q_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MSUBq (0x25)
        0x25 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::msub(acc[lane], vs[lane], q_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // SUBi (0x26)
        0x26 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::sub(vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MSUBi (0x27)
        0x27 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::msub(acc[lane], vs[lane], i_val);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // ADD (0x28)
        0x28 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::add(vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MADD (0x29)
        0x29 => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::madd(acc[lane], vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MUL (0x2A)
        0x2A => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::mul(vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MAX (0x2B)
        0x2B => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::max(vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r);
                }
            }
        }
        // SUB (0x2C)
        0x2C => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::sub(vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // MSUB (0x2D)
        0x2D => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::msub(acc[lane], vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r.bits);
                }
            }
        }
        // OPMSUB (0x2E)
        0x2E => {
            let rx = ps2float::msub(acc[0], vs[1], vt[2]).bits;
            let ry = ps2float::msub(acc[1], vs[2], vt[0]).bits;
            let rz = ps2float::msub(acc[2], vs[0], vt[1]).bits;
            if lane_enabled(dest, 0) {
                vu.write_vf_lane(fd, 0, rx);
            }
            if lane_enabled(dest, 1) {
                vu.write_vf_lane(fd, 1, ry);
            }
            if lane_enabled(dest, 2) {
                vu.write_vf_lane(fd, 2, rz);
            }
        }
        // MINI (0x2F)
        0x2F => {
            for lane in 0..4 {
                if lane_enabled(dest, lane) {
                    let r = ps2float::min(vs[lane], vt[lane]);
                    vu.write_vf_lane(fd, lane, r);
                }
            }
        }
        // UpperType2 / UpperType3 (0x3C..=0x3F)
        0x3C..=0x3F => {
            let op11 = instr & 0x7FF;
            match op11 {
                // ADDAbc (0x03C..=0x03F)
                0x03C..=0x03F => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::add(vs[lane], vt_bc).bits;
                        }
                    }
                }
                // SUBAbc (0x07C..=0x07F)
                0x07C..=0x07F => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::sub(vs[lane], vt_bc).bits;
                        }
                    }
                }
                // MADDAbc (0x0BC..=0x0BF)
                0x0BC..=0x0BF => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::madd(acc[lane], vs[lane], vt_bc).bits;
                        }
                    }
                }
                // MSUBAbc (0x0FC..=0x0FF)
                0x0FC..=0x0FF => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::msub(acc[lane], vs[lane], vt_bc).bits;
                        }
                    }
                }
                // ITOF0 / ITOF4 / ITOF12 / ITOF15
                0x13C..=0x13F => {
                    let shift = match op11 & 3 {
                        0 => 0,
                        1 => 4,
                        2 => 12,
                        _ => 15,
                    };
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, vu_itof(vs[lane], shift));
                        }
                    }
                }
                // FTOI0 / FTOI4 / FTOI12 / FTOI15
                0x17C..=0x17F => {
                    let shift = match op11 & 3 {
                        0 => 0,
                        1 => 4,
                        2 => 12,
                        _ => 15,
                    };
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, vu_ftoi(vs[lane], shift));
                        }
                    }
                }
                // MULAbc (0x1BC..=0x1BF)
                0x1BC..=0x1BF => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::mul(vs[lane], vt_bc).bits;
                        }
                    }
                }
                // MULAq (0x1FC)
                0x1FC => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::mul(vs[lane], q_val).bits;
                        }
                    }
                }
                // ABS (0x1FD)
                0x1FD => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, vs[lane] & 0x7FFF_FFFF);
                        }
                    }
                }
                // MULAi (0x1FE)
                0x1FE => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::mul(vs[lane], i_val).bits;
                        }
                    }
                }
                // CLIP (0x1FF)
                0x1FF => {
                    let bits =
                        compute_clip_bits(Reg128::from_u32x4(vs[0], vs[1], vs[2], vs[3]), vt[3]);
                    vu.clip_flag = ((vu.clip_flag << 6) | bits) & 0x00FF_FFFF;
                }
                // ADDAq (0x23C)
                0x23C => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::add(vs[lane], q_val).bits;
                        }
                    }
                }
                // MADDAq (0x23D)
                0x23D => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::madd(acc[lane], vs[lane], q_val).bits;
                        }
                    }
                }
                // ADDAi (0x23E)
                0x23E => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::add(vs[lane], i_val).bits;
                        }
                    }
                }
                // MADDAi (0x23F)
                0x23F => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::madd(acc[lane], vs[lane], i_val).bits;
                        }
                    }
                }
                // SUBAq (0x27C)
                0x27C => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::sub(vs[lane], q_val).bits;
                        }
                    }
                }
                // MSUBAq (0x27D)
                0x27D => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::msub(acc[lane], vs[lane], q_val).bits;
                        }
                    }
                }
                // SUBAi (0x27E)
                0x27E => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::sub(vs[lane], i_val).bits;
                        }
                    }
                }
                // MSUBAi (0x27F)
                0x27F => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::msub(acc[lane], vs[lane], i_val).bits;
                        }
                    }
                }
                // ADDA (0x2BC)
                0x2BC => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::add(vs[lane], vt[lane]).bits;
                        }
                    }
                }
                // MADDA (0x2BD)
                0x2BD => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::madd(acc[lane], vs[lane], vt[lane]).bits;
                        }
                    }
                }
                // MULA (0x2BE)
                0x2BE => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::mul(vs[lane], vt[lane]).bits;
                        }
                    }
                }
                // SUBA (0x2FC)
                0x2FC => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] = ps2float::sub(vs[lane], vt[lane]).bits;
                        }
                    }
                }
                // MSUBA (0x2FD)
                0x2FD => {
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.acc.u32_lanes_mut()[lane] =
                                ps2float::msub(acc[lane], vs[lane], vt[lane]).bits;
                        }
                    }
                }
                // OPMULA (0x2FE)
                0x2FE => {
                    let rx = ps2float::mul(vs[1], vt[2]).bits;
                    let ry = ps2float::mul(vs[2], vt[0]).bits;
                    let rz = ps2float::mul(vs[0], vt[1]).bits;
                    let acc_lanes = vu.acc.u32_lanes_mut();
                    acc_lanes[0] = rx;
                    acc_lanes[1] = ry;
                    acc_lanes[2] = rz;
                }
                // NOP (0x2FF)
                _ => {}
            }
        }
        _ => {}
    }
}

/// Execute a single 32-bit Lower Type 1/3/4/5 instruction (`bit 31 == 1` in micro mode, or COP2 macro mode).
pub fn exec_shared_lower_op(vu: &mut VuState, data_mem: &mut [u8], instr: u32) {
    let dest = ((instr >> 21) & 0xF) as u8;
    let fsf = ((instr >> 21) & 0x3) as usize;
    let ftf = ((instr >> 23) & 0x3) as usize;
    let ft = ((instr >> 16) & 0x1F) as usize;
    let fs = ((instr >> 11) & 0x1F) as usize;
    let fd = ((instr >> 6) & 0x1F) as usize;
    let it = ft & 0xF;
    let is = fs & 0xF;
    let id = fd & 0xF;
    let funct = instr & 0x3F;

    match funct {
        0x30 => exec_viadd(vu, id, is, it),
        0x31 => exec_visub(vu, id, is, it),
        0x32 => exec_viaddi(vu, it, is, fd as u32),
        0x34 => exec_viand(vu, id, is, it),
        0x35 => exec_vior(vu, id, is, it),
        0x3C..=0x3F => {
            let op11 = instr & 0x7FF;
            match op11 {
                // MOVE.dest ft, fs
                0x33C => {
                    let src = vu.read_vf(fs).u32_lanes().to_owned();
                    for (lane, &val) in src.iter().enumerate() {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, val);
                        }
                    }
                }
                // MR32.dest ft, fs
                0x33D => {
                    let src = vu.read_vf(fs).u32_lanes().to_owned();
                    let rotated = [src[1], src[2], src[3], src[0]];
                    for (lane, &val) in rotated.iter().enumerate() {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, val);
                        }
                    }
                }
                // LQI.dest ft, (is++)
                0x37C => {
                    let addr = usize::from(vu.read_vi(is));
                    load_vf_from_qword(vu, data_mem, dest, ft, addr);
                    vu.write_vi(is, (addr as u16).wrapping_add(1));
                }
                // SQI.dest fs, (it++)
                0x37D => {
                    let addr = usize::from(vu.read_vi(it));
                    store_vf_to_qword(vu, data_mem, dest, fs, addr);
                    vu.write_vi(it, (addr as u16).wrapping_add(1));
                }
                // LQD.dest ft, (--is)
                0x37E => {
                    let next = vu.read_vi(is).wrapping_sub(1);
                    vu.write_vi(is, next);
                    load_vf_from_qword(vu, data_mem, dest, ft, usize::from(next));
                }
                // SQD.dest fs, (--it)
                0x37F => {
                    let next = vu.read_vi(it).wrapping_sub(1);
                    vu.write_vi(it, next);
                    store_vf_to_qword(vu, data_mem, dest, fs, usize::from(next));
                }
                // DIV Q, fs[fsf], ft[ftf]
                0x3BC => {
                    let a = vu.read_vf_lane(fs, fsf);
                    let b = vu.read_vf_lane(ft, ftf);
                    vu.q = ps2float::div(a, b).bits;
                }
                // SQRT Q, ft[ftf]
                0x3BD => {
                    let b = vu.read_vf_lane(ft, ftf);
                    vu.q = ps2float::sqrt(b).bits;
                }
                // RSQRT Q, fs[fsf], ft[ftf]
                0x3BE => {
                    let a = vu.read_vf_lane(fs, fsf);
                    let b = vu.read_vf_lane(ft, ftf);
                    vu.q = ps2float::rsqrt(a, b).bits;
                }
                // WAITQ
                0x3BF => {}
                // MTIR it, fs[fsf]
                0x3FC => {
                    let val = vu.read_vf_lane(fs, fsf) as u16;
                    vu.write_vi(it, val);
                }
                // MFIR.dest ft, is
                0x3FD => {
                    let sext = (vu.read_vi(is) as i16 as i32) as u32;
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, sext);
                        }
                    }
                }
                // ILWR.dest it, (is)
                0x3FE => exec_ilwr(vu, data_mem, dest, it, is),
                // ISWR.dest it, (is)
                0x3FF => exec_iswr(vu, data_mem, dest, it, is),
                // RNEXT.dest ft
                0x43C => {
                    let val = step_r_lfsr(vu);
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, val);
                        }
                    }
                }
                // RGET.dest ft
                0x43D => {
                    let val = 0x3F80_0000 | (vu.r & 0x007F_FFFF);
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, val);
                        }
                    }
                }
                // RINIT R, fs[fsf]
                0x43E => {
                    vu.r = vu.read_vf_lane(fs, fsf) & 0x007F_FFFF;
                }
                // RXOR R, fs[fsf]
                0x43F => {
                    vu.r = (vu.r ^ vu.read_vf_lane(fs, fsf)) & 0x007F_FFFF;
                }
                // MFP.dest ft, P
                0x67C => {
                    let val = vu.p;
                    for lane in 0..4 {
                        if lane_enabled(dest, lane) {
                            vu.write_vf_lane(ft, lane, val);
                        }
                    }
                }
                // WAITP
                0x7BF => {}
                _ => {}
            }
        }
        _ => {}
    }
}

/// Execute a 32-bit Lower Type 7/8/9 micro instruction (`bit 31 == 0`).
pub fn exec_micro_lower_type789(
    vu: &mut VuState,
    data_mem: &mut [u8],
    instr: u32,
    delay_slot_tpc: u16,
    tpc_mask: u16,
) {
    let op7 = (instr >> 25) & 0x7F;
    let dest = ((instr >> 21) & 0xF) as u8;
    let ft = ((instr >> 16) & 0x1F) as usize;
    let fs = ((instr >> 11) & 0x1F) as usize;
    let it = ft & 0xF;
    let is = fs & 0xF;
    let simm11 = simm11_to_i16(instr) as u16;
    let imm15 = ((instr & 0x7FF) | (((instr >> 21) & 0xF) << 11)) as u16;
    let imm12 = ((instr & 0x7FF) | (((instr >> 21) & 0x1) << 11)) as u16;
    let imm24 = instr & 0x00FF_FFFF;

    match op7 {
        // LQ.dest ft, imm11(is)
        0x00 => {
            let addr = usize::from(vu.read_vi(is).wrapping_add(simm11));
            load_vf_from_qword(vu, data_mem, dest, ft, addr);
        }
        // SQ.dest fs, imm11(it)
        0x01 => {
            let addr = usize::from(vu.read_vi(it).wrapping_add(simm11));
            store_vf_to_qword(vu, data_mem, dest, fs, addr);
        }
        // ILW.dest it, imm11(is)
        0x04 => {
            let addr = usize::from(vu.read_vi(is).wrapping_add(simm11));
            load_vi_from_qword(vu, data_mem, dest, it, addr);
        }
        // ISW.dest it, imm11(is)
        0x05 => {
            let addr = usize::from(vu.read_vi(is).wrapping_add(simm11));
            store_vi_to_qword(vu, data_mem, dest, it, addr);
        }
        // IADDIU it, is, imm15
        0x08 => {
            let res = vu.read_vi(is).wrapping_add(imm15);
            vu.write_vi(it, res);
        }
        // ISUBIU it, is, imm15
        0x09 => {
            let res = vu.read_vi(is).wrapping_sub(imm15);
            vu.write_vi(it, res);
        }
        // FCEQ vi01, imm24
        0x10 => {
            let eq = u16::from((vu.clip_flag & 0x00FF_FFFF) == imm24);
            vu.write_vi(1, eq);
        }
        // FCSET imm24
        0x11 => {
            vu.clip_flag = imm24 & 0x00FF_FFFF;
        }
        // FCAND vi01, imm24
        0x12 => {
            let nz = u16::from((vu.clip_flag & imm24) != 0);
            vu.write_vi(1, nz);
        }
        // FCOR vi01, imm24
        0x13 => {
            let all = u16::from(((vu.clip_flag | imm24) & 0x00FF_FFFF) == 0x00FF_FFFF);
            vu.write_vi(1, all);
        }
        // FSEQ it, imm12
        0x14 => {
            let eq = u16::from((vu.status_flag & 0x0FFF) == imm12);
            vu.write_vi(it, eq);
        }
        // FSSET imm12
        0x15 => {
            vu.status_flag = (vu.status_flag & 0x003F) | (imm12 & 0x0FC0);
        }
        // FSAND it, imm12
        0x16 => {
            vu.write_vi(it, vu.status_flag & imm12);
        }
        // FSOR it, imm12
        0x17 => {
            vu.write_vi(it, (vu.status_flag | imm12) & 0x0FFF);
        }
        // FMEQ it, is
        0x18 => {
            let eq = u16::from(vu.mac_flag == vu.read_vi(is));
            vu.write_vi(it, eq);
        }
        // FMAND it, is
        0x1A => {
            let res = vu.mac_flag & vu.read_vi(is);
            vu.write_vi(it, res);
        }
        // FMOR it, is
        0x1B => {
            let res = vu.mac_flag | vu.read_vi(is);
            vu.write_vi(it, res);
        }
        // FCGET it
        0x1C => {
            vu.write_vi(it, (vu.clip_flag & 0x0FFF) as u16);
        }
        // B imm11
        0x20 => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // BAL it, imm11
        0x21 => {
            let link = delay_slot_tpc.wrapping_add(1) & tpc_mask;
            vu.write_vi(it, link);
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // JR is
        0x24 => {
            let target = vu.read_vi(is) & tpc_mask;
            vu.branch_delay_active = true;
            vu.branch_delay_target = target;
        }
        // JALR it, is
        0x25 => {
            let target = vu.read_vi(is) & tpc_mask;
            let link = delay_slot_tpc.wrapping_add(1) & tpc_mask;
            vu.write_vi(it, link);
            vu.branch_delay_active = true;
            vu.branch_delay_target = target;
        }
        // IBEQ it, is, imm11
        0x28 if vu.read_vi(it) == vu.read_vi(is) => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // IBNE it, is, imm11
        0x29 if vu.read_vi(it) != vu.read_vi(is) => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // IBLTZ is, imm11
        0x2C if (vu.read_vi(is) as i16) < 0 => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // IBGTZ is, imm11
        0x2D if (vu.read_vi(is) as i16) > 0 => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // IBLEZ is, imm11
        0x2E if (vu.read_vi(is) as i16) <= 0 => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        // IBGEZ is, imm11
        0x2F if (vu.read_vi(is) as i16) >= 0 => {
            vu.branch_delay_active = true;
            vu.branch_delay_target = delay_slot_tpc.wrapping_add(simm11) & tpc_mask;
        }
        _ => {}
    }
}

/// Execute a single `COP2` macro-mode instruction (`Special1`/`Special2`) on `vu`.
pub fn exec_macro_op(vu: &mut VuState, data_mem: &mut [u8], instr: u32) {
    let funct6 = instr & 0x3F;
    match funct6 {
        0x00..=0x2F => exec_upper_op(vu, instr),
        0x30..=0x35 => exec_shared_lower_op(vu, data_mem, instr),
        0x3C..=0x3F => {
            let op11 = instr & 0x7FF;
            if op11 < 0x300 {
                exec_upper_op(vu, instr);
            } else {
                exec_shared_lower_op(vu, data_mem, instr);
            }
        }
        _ => {}
    }
}

/// Step one 64-bit LIW instruction on `vu` in micro mode.
pub fn step_micro_liw(vu: &mut VuState, code_mem: &[u8], data_mem: &mut [u8], tpc_mask: u16) {
    if !vu.running {
        return;
    }

    let pc_index = usize::from(vu.tpc & tpc_mask);
    let byte_offset = (pc_index * 8) & (code_mem.len().saturating_sub(8));
    let lower = u32::from_le_bytes([
        code_mem[byte_offset],
        code_mem[byte_offset + 1],
        code_mem[byte_offset + 2],
        code_mem[byte_offset + 3],
    ]);
    let upper = u32::from_le_bytes([
        code_mem[byte_offset + 4],
        code_mem[byte_offset + 5],
        code_mem[byte_offset + 6],
        code_mem[byte_offset + 7],
    ]);

    let was_finishing = vu.finish_after_slot;
    let has_e_bit = (upper & UPPER_BIT_E) != 0;
    let has_i_bit = (upper & UPPER_BIT_I) != 0;

    let delay_slot_tpc = vu.tpc.wrapping_add(1) & tpc_mask;
    if vu.branch_delay_active {
        vu.tpc = vu.branch_delay_target & tpc_mask;
        vu.branch_delay_active = false;
    } else {
        vu.tpc = delay_slot_tpc;
    }

    let pre = pipeline::before_liw(vu, upper, lower, has_i_bit);

    if has_i_bit {
        // Upper instruction reads the previous `I` register before `LOI` updates `I`
        // (`ps2autotests` `tests/vu/games/triace.cpp`, `FETCHED`).
        pipeline::prepare_upper_clip(vu, upper);
        exec_upper_op(vu, upper);
        vu.i = lower;
    } else {
        // Both Upper and Lower read pre-LIW register values (`vs`, `vt`, `Q`, `I`),
        // and Upper wins on any same-cycle VF destination conflict (`ps2tek` `#vu`, `FETCHED`).
        let fs = ((upper >> 11) & 0x1F) as usize;
        let ft = ((upper >> 16) & 0x1F) as usize;
        let vs = vu.read_vf(fs).u32_lanes().to_owned();
        let vt = vu.read_vf(ft).u32_lanes().to_owned();
        let q_val = vu.q;
        let i_val = vu.i;

        if (lower & 0x8000_0000) != 0 {
            exec_shared_lower_op(vu, data_mem, lower);
        } else {
            exec_micro_lower_type789(vu, data_mem, lower, delay_slot_tpc, tpc_mask);
        }

        pipeline::prepare_upper_clip(vu, upper);
        exec_upper_op_with_sources(vu, upper, vs, vt, q_val, i_val);
    }

    pipeline::after_liw(vu, upper, lower, has_i_bit, pre);

    if was_finishing {
        vu.running = false;
        vu.finish_after_slot = false;
    } else if has_e_bit {
        vu.finish_after_slot = true;
    }
}
