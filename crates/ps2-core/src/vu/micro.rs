//! Functional (untimed) VU micro-mode interpreter core and shared integer/memory helpers.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#vu` & `#vumemory`, `FETCHED`):
//!   * Each 64-bit LIW instruction has a 32-bit lower word (`bits 0..31`) and a 32-bit upper word
//!     (`bits 32..63`).
//!   * Upper word bit 31 (`I`) is `LOI`: the lower 32-bit word is loaded directly into the `I`
//!     register instead of executing as a lower instruction.
//!   * Upper word bit 30 (`E`) ends the microprogram after a 1-instruction delay slot.
//! - `ps2autotests` (`tests/cpu/vu0_macro/integer.cpp`, `transfer.cpp`, `tests/vu/assemble/ops.cpp`, `FETCHED`):
//!   * Verifies `TPC` (`vi26`) progression across the `E`-bit delay slot and 16-bit wrapping
//!     integer arithmetic (`VIADD`, `VISUB`, `VIADDI`, `VIAND`, `VIOR`, `VILWR`, `VISWR`).

use super::state::{VuState, DEST_W, DEST_X, DEST_Y, DEST_Z};

/// Upper instruction bit 31: `LOI` (lower 32-bit word is loaded into `I` register).
pub const UPPER_BIT_I: u32 = 1 << 31;
/// Upper instruction bit 30: `E` (end microprogram after 1 delay-slot instruction).
pub const UPPER_BIT_E: u32 = 1 << 30;

/// Sign-extend a 5-bit immediate (`bits 0..4`) to a 16-bit unsigned integer (`u16`).
#[inline(always)]
pub const fn simm5_to_u16(imm5: u32) -> u16 {
    let raw = (imm5 & 0x1F) as i8;
    let sext = (raw << 3) >> 3;
    sext as i16 as u16
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

/// Execute a single 32-bit Lower/Type1/3/5 instruction shared between COP2 macro and micro mode.
pub fn exec_shared_lower_op(vu: &mut VuState, data_mem: &mut [u8], instr: u32) {
    let dest = ((instr >> 21) & 0xF) as u8;
    let it = ((instr >> 16) & 0x1F) as usize;
    let is = ((instr >> 11) & 0x1F) as usize;
    let id = ((instr >> 6) & 0x1F) as usize;
    let funct = instr & 0x3F;

    match funct {
        0x30 => exec_viadd(vu, id, is, it),
        0x31 => exec_visub(vu, id, is, it),
        0x32 => exec_viaddi(vu, it, is, id as u32),
        0x34 => exec_viand(vu, id, is, it),
        0x35 => exec_vior(vu, id, is, it),
        0x3C..=0x3F => {
            let op11 = instr & 0x7FF;
            match op11 {
                0x3FE => exec_ilwr(vu, data_mem, dest, it, is),
                0x3FF => exec_iswr(vu, data_mem, dest, it, is),
                _ => {}
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

    if vu.branch_delay_active {
        vu.tpc = vu.branch_delay_target & tpc_mask;
        vu.branch_delay_active = false;
    } else {
        vu.tpc = vu.tpc.wrapping_add(1) & tpc_mask;
    }

    if has_i_bit {
        vu.i = lower;
    } else if (lower & 0x8000_0000) != 0 {
        exec_shared_lower_op(vu, data_mem, lower);
    }

    if was_finishing {
        vu.running = false;
        vu.finish_after_slot = false;
    } else if has_e_bit {
        vu.finish_after_slot = true;
    }
}
