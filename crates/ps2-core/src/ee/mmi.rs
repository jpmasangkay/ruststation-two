//! Emotion Engine (MIPS R5900) Multimedia Instructions (`MMI`, `opcode == 0x1C`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eeinstructions`):
//!   * `MMI` (`opcode = 0x1C`) top-level table and `MMI0`, `MMI1`, `MMI2`, `MMI3`, `PMFHL`, `PMTHL` sub-tables.
//! - `ps2autotests` (`local/ps2autotests/tests/cpu/ee/{alu,muldiv}.expected` &
//!   `local/ps2autotests/tests/cpu/ee_simd/{arithmetic,compare,funnel,logic,muldiv,shuffle}.expected`):
//!   * `HI0`/`LO0` (`bits 0..=63`) and `HI1`/`LO1` (`bits 64..=127`) 128-bit accumulator layout.
//!   * `PHMADH` and `PHMSBH` upper-32-bit word placement in `LO` and `HI` (`PHMSBH` stores `~p[2*i+1]`
//!     in `LO.w1, HI.w1, LO.w3, HI.w3`, `muldiv.expected` lines 145..171).
//!   * `PEXCH`/`PEXCW` ("Exchange Center") vs `PEXEH`/`PEXEW` ("Exchange Even", `shuffle.expected` lines 88..190).

use ps2_common::Reg128;

use super::cop0::Level1ExceptionCode;
use super::interpreter::StepOutcome;
use super::state::EeCpuState;

impl EeCpuState {
    /// Execute an `MMI` (`opcode == 0x1C`) instruction (`ps2tek` `#eeinstructions`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_mmi(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        shamt: u32,
        funct: u32,
    ) -> StepOutcome {
        match funct {
            // MADD rd, rs, rt (0x00)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 173..232).
            0x00 => {
                let acc =
                    ((self.hi.lo_u64() & 0xFFFF_FFFF) << 32) | (self.lo.lo_u64() & 0xFFFF_FFFF);
                let prod = i64::from(self.gpr_i32(rs)) * i64::from(self.gpr_i32(rt));
                let res = acc.wrapping_add(prod as u64);
                let lo32 = res as i32;
                let hi32 = (res >> 32) as i32;
                self.lo.set_sext32(lo32);
                self.hi.set_sext32(hi32);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // MADDU rd, rs, rt (0x01)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 233..292).
            0x01 => {
                let acc =
                    ((self.hi.lo_u64() & 0xFFFF_FFFF) << 32) | (self.lo.lo_u64() & 0xFFFF_FFFF);
                let prod = u64::from(self.gpr_u32(rs)) * u64::from(self.gpr_u32(rt));
                let res = acc.wrapping_add(prod);
                let lo32 = res as i32;
                let hi32 = (res >> 32) as i32;
                self.lo.set_sext32(lo32);
                self.hi.set_sext32(hi32);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // PLZCW rd, rs (0x04): Parallel Leading Zero or One Count Word.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/alu.expected` lines 536..549).
            0x04 => {
                let src = self.gpr_u64(rs);
                let w0 = src as u32;
                let w1 = (src >> 32) as u32;
                let c0 = plzcw_word(w0);
                let c1 = plzcw_word(w1);
                let res = u64::from(c0) | (u64::from(c1) << 32);
                self.set_gpr_u64(rd, res);
                StepOutcome::Normal
            }

            // MMI0 sub-table (0x08)
            0x08 => self.execute_mmi0(instr, instr_pc, rs, rt, rd, shamt),

            // MMI2 sub-table (0x09)
            0x09 => self.execute_mmi2(instr, instr_pc, rs, rt, rd, shamt),

            // MFHI1 rd (0x10)
            0x10 => {
                let val = self.hi.hi_u64();
                self.set_gpr_u64(rd, val);
                StepOutcome::Normal
            }

            // MTHI1 rs (0x11)
            0x11 => {
                let val = self.gpr_u64(rs);
                self.hi.set_hi_u64(val);
                StepOutcome::Normal
            }

            // MFLO1 rd (0x12)
            0x12 => {
                let val = self.lo.hi_u64();
                self.set_gpr_u64(rd, val);
                StepOutcome::Normal
            }

            // MTLO1 rs (0x13)
            0x13 => {
                let val = self.gpr_u64(rs);
                self.lo.set_hi_u64(val);
                StepOutcome::Normal
            }

            // MULT1 rd, rs, rt (0x18)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 409..466).
            0x18 => {
                let prod = i64::from(self.gpr_i32(rs)) * i64::from(self.gpr_i32(rt));
                let lo32 = prod as i32;
                let hi32 = (prod >> 32) as i32;
                self.lo.set_hi_u64(i64::from(lo32) as u64);
                self.hi.set_hi_u64(i64::from(hi32) as u64);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // MULTU1 rd, rs, rt (0x19)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 467..524).
            0x19 => {
                let prod = u64::from(self.gpr_u32(rs)) * u64::from(self.gpr_u32(rt));
                let lo32 = prod as i32;
                let hi32 = (prod >> 32) as i32;
                self.lo.set_hi_u64(i64::from(lo32) as u64);
                self.hi.set_hi_u64(i64::from(hi32) as u64);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // DIV1 rs, rt (0x1A)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 353..380).
            0x1A => {
                let n = self.gpr_i32(rs);
                let d = self.gpr_i32(rt);
                let (q, r) = div_signed_32(n, d);
                self.lo.set_hi_u64(i64::from(q) as u64);
                self.hi.set_hi_u64(i64::from(r) as u64);
                StepOutcome::Normal
            }

            // DIVU1 rs, rt (0x1B)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 381..408).
            0x1B => {
                let n = self.gpr_u32(rs);
                let d = self.gpr_u32(rt);
                let (q, r) = div_unsigned_32(n, d);
                self.lo.set_hi_u64(i64::from(q) as u64);
                self.hi.set_hi_u64(i64::from(r) as u64);
                StepOutcome::Normal
            }

            // MADD1 rd, rs, rt (0x20)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 525..584).
            0x20 => {
                let acc =
                    ((self.hi.hi_u64() & 0xFFFF_FFFF) << 32) | (self.lo.hi_u64() & 0xFFFF_FFFF);
                let prod = i64::from(self.gpr_i32(rs)) * i64::from(self.gpr_i32(rt));
                let res = acc.wrapping_add(prod as u64);
                let lo32 = res as i32;
                let hi32 = (res >> 32) as i32;
                self.lo.set_hi_u64(i64::from(lo32) as u64);
                self.hi.set_hi_u64(i64::from(hi32) as u64);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // MADDU1 rd, rs, rt (0x21)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 585..644).
            0x21 => {
                let acc =
                    ((self.hi.hi_u64() & 0xFFFF_FFFF) << 32) | (self.lo.hi_u64() & 0xFFFF_FFFF);
                let prod = u64::from(self.gpr_u32(rs)) * u64::from(self.gpr_u32(rt));
                let res = acc.wrapping_add(prod);
                let lo32 = res as i32;
                let hi32 = (res >> 32) as i32;
                self.lo.set_hi_u64(i64::from(lo32) as u64);
                self.hi.set_hi_u64(i64::from(hi32) as u64);
                self.set_gpr_sext32(rd, lo32);
                StepOutcome::Normal
            }

            // MMI1 sub-table (0x28)
            0x28 => self.execute_mmi1(instr, instr_pc, rs, rt, rd, shamt),

            // MMI3 sub-table (0x29)
            0x29 => self.execute_mmi3(instr, instr_pc, rs, rt, rd, shamt),

            // PMFHL rd (0x30): Parallel Move From HI/LO Register.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 491..645).
            0x30 => self.execute_pmfhl(instr, instr_pc, rd, shamt),

            // PMTHL.LW rs (0x31): Parallel Move To HI/LO Register (Lower Word).
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 708..738).
            0x31 => {
                if shamt == 0 {
                    let s = self.gpr_128(rs);
                    let sw = s.u32_lanes();
                    self.lo.u32_lanes_mut()[0] = sw[0];
                    self.hi.u32_lanes_mut()[0] = sw[1];
                    self.lo.u32_lanes_mut()[2] = sw[2];
                    self.hi.u32_lanes_mut()[2] = sw[3];
                    StepOutcome::Normal
                } else {
                    self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                    StepOutcome::ExceptionOrEret
                }
            }

            // PSLLH rd, rt, sa (0x34)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 25..50).
            0x34 => {
                let s = shamt & 0xF;
                let t = self.gpr_128(rt);
                let mut out = Reg128::ZERO;
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = t.u16_lanes()[i] << s;
                }
                self.set_gpr_128(rd, out);
                StepOutcome::Normal
            }

            // PSRLH rd, rt, sa (0x36)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 79..104).
            0x36 => {
                let s = shamt & 0xF;
                let t = self.gpr_128(rt);
                let mut out = Reg128::ZERO;
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = t.u16_lanes()[i] >> s;
                }
                self.set_gpr_128(rd, out);
                StepOutcome::Normal
            }

            // PSRAH rd, rt, sa (0x37)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 52..77).
            0x37 => {
                let s = shamt & 0xF;
                let t = self.gpr_128(rt);
                let mut out = Reg128::ZERO;
                for i in 0..8 {
                    out.i16_lanes_mut()[i] = t.i16_lanes()[i] >> s;
                }
                self.set_gpr_128(rd, out);
                StepOutcome::Normal
            }

            // PSLLW rd, rt, sa (0x3C)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 106..133).
            0x3C => {
                let t = self.gpr_128(rt);
                let mut out = Reg128::ZERO;
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = t.u32_lanes()[i] << shamt;
                }
                self.set_gpr_128(rd, out);
                StepOutcome::Normal
            }

            // PSRLW rd, rt, sa (0x3E)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 216..243).
            0x3E => {
                let t = self.gpr_128(rt);
                let mut out = Reg128::ZERO;
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = t.u32_lanes()[i] >> shamt;
                }
                self.set_gpr_128(rd, out);
                StepOutcome::Normal
            }

            // PSRAW rd, rt, sa (0x3F)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 245..272).
            0x3F => {
                let t = self.gpr_128(rt);
                let mut out = Reg128::ZERO;
                for i in 0..4 {
                    out.i32_lanes_mut()[i] = t.i32_lanes()[i] >> shamt;
                }
                self.set_gpr_128(rd, out);
                StepOutcome::Normal
            }

            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    funct,
                    "Reserved MMI funct"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                StepOutcome::ExceptionOrEret
            }
        }
    }

    /// Execute an `MMI0` (`opcode == 0x1C, funct == 0x08`) instruction decoded by `shamt`.
    fn execute_mmi0(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        shamt: u32,
    ) -> StepOutcome {
        let a = self.gpr_128(rs);
        let b = self.gpr_128(rt);
        let mut out = Reg128::ZERO;

        match shamt {
            // PADDW (0x00)
            0x00 => {
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = a.u32_lanes()[i].wrapping_add(b.u32_lanes()[i]);
                }
            }
            // PSUBW (0x01)
            0x01 => {
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = a.u32_lanes()[i].wrapping_sub(b.u32_lanes()[i]);
                }
            }
            // PCGTW (0x02)
            0x02 => {
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = if a.i32_lanes()[i] > b.i32_lanes()[i] {
                        0xFFFF_FFFF
                    } else {
                        0
                    };
                }
            }
            // PMAXW (0x03)
            0x03 => {
                for i in 0..4 {
                    out.i32_lanes_mut()[i] = a.i32_lanes()[i].max(b.i32_lanes()[i]);
                }
            }
            // PADDH (0x04)
            0x04 => {
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = a.u16_lanes()[i].wrapping_add(b.u16_lanes()[i]);
                }
            }
            // PSUBH (0x05)
            0x05 => {
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = a.u16_lanes()[i].wrapping_sub(b.u16_lanes()[i]);
                }
            }
            // PCGTH (0x06)
            0x06 => {
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = if a.i16_lanes()[i] > b.i16_lanes()[i] {
                        0xFFFF
                    } else {
                        0
                    };
                }
            }
            // PMAXH (0x07)
            0x07 => {
                for i in 0..8 {
                    out.i16_lanes_mut()[i] = a.i16_lanes()[i].max(b.i16_lanes()[i]);
                }
            }
            // PADDB (0x08)
            0x08 => {
                for i in 0..16 {
                    out.u8_lanes_mut()[i] = a.u8_lanes()[i].wrapping_add(b.u8_lanes()[i]);
                }
            }
            // PSUBB (0x09)
            0x09 => {
                for i in 0..16 {
                    out.u8_lanes_mut()[i] = a.u8_lanes()[i].wrapping_sub(b.u8_lanes()[i]);
                }
            }
            // PCGTB (0x0A)
            0x0A => {
                for i in 0..16 {
                    out.u8_lanes_mut()[i] = if a.i8_lanes()[i] > b.i8_lanes()[i] {
                        0xFF
                    } else {
                        0
                    };
                }
            }
            // PADDSW (0x10)
            0x10 => {
                for i in 0..4 {
                    out.i32_lanes_mut()[i] = a.i32_lanes()[i].saturating_add(b.i32_lanes()[i]);
                }
            }
            // PSUBSW (0x11)
            0x11 => {
                for i in 0..4 {
                    out.i32_lanes_mut()[i] = a.i32_lanes()[i].saturating_sub(b.i32_lanes()[i]);
                }
            }
            // PEXTLW (0x12)
            0x12 => {
                let aw = a.u32_lanes();
                let bw = b.u32_lanes();
                out = Reg128::from_u32x4(bw[0], aw[0], bw[1], aw[1]);
            }
            // PPACW (0x13)
            0x13 => {
                let aw = a.u32_lanes();
                let bw = b.u32_lanes();
                out = Reg128::from_u32x4(bw[0], bw[2], aw[0], aw[2]);
            }
            // PADDSH (0x14)
            0x14 => {
                for i in 0..8 {
                    out.i16_lanes_mut()[i] = a.i16_lanes()[i].saturating_add(b.i16_lanes()[i]);
                }
            }
            // PSUBSH (0x15)
            0x15 => {
                for i in 0..8 {
                    out.i16_lanes_mut()[i] = a.i16_lanes()[i].saturating_sub(b.i16_lanes()[i]);
                }
            }
            // PEXTLH (0x16)
            0x16 => {
                for i in 0..4 {
                    out.u16_lanes_mut()[2 * i] = b.u16_lanes()[i];
                    out.u16_lanes_mut()[2 * i + 1] = a.u16_lanes()[i];
                }
            }
            // PPACH (0x17)
            0x17 => {
                for i in 0..4 {
                    out.u16_lanes_mut()[i] = b.u16_lanes()[2 * i];
                    out.u16_lanes_mut()[i + 4] = a.u16_lanes()[2 * i];
                }
            }
            // PADDSB (0x18)
            0x18 => {
                for i in 0..16 {
                    out.i8_lanes_mut()[i] = a.i8_lanes()[i].saturating_add(b.i8_lanes()[i]);
                }
            }
            // PSUBSB (0x19)
            0x19 => {
                for i in 0..16 {
                    out.i8_lanes_mut()[i] = a.i8_lanes()[i].saturating_sub(b.i8_lanes()[i]);
                }
            }
            // PEXTLB (0x1A)
            0x1A => {
                for i in 0..8 {
                    out.u8_lanes_mut()[2 * i] = b.u8_lanes()[i];
                    out.u8_lanes_mut()[2 * i + 1] = a.u8_lanes()[i];
                }
            }
            // PPACB (0x1B)
            0x1B => {
                for i in 0..8 {
                    out.u8_lanes_mut()[i] = b.u8_lanes()[2 * i];
                    out.u8_lanes_mut()[i + 8] = a.u8_lanes()[2 * i];
                }
            }
            // PEXT5 (0x1E)
            0x1E => {
                for i in 0..4 {
                    let w = b.u32_lanes()[i];
                    let r5 = w & 0x1F;
                    let g5 = (w >> 5) & 0x1F;
                    let b5 = (w >> 10) & 0x1F;
                    let a1 = (w >> 15) & 1;
                    out.u32_lanes_mut()[i] = (r5 << 3) | (g5 << 11) | (b5 << 19) | (a1 << 31);
                }
            }
            // PPAC5 (0x1F)
            0x1F => {
                for i in 0..4 {
                    let w = b.u32_lanes()[i];
                    let r5 = (w >> 3) & 0x1F;
                    let g5 = (w >> 11) & 0x1F;
                    let b5 = (w >> 19) & 0x1F;
                    let a1 = (w >> 31) & 1;
                    out.u32_lanes_mut()[i] = r5 | (g5 << 5) | (b5 << 10) | (a1 << 15);
                }
            }
            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    shamt,
                    "Reserved MMI0 shamt"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                return StepOutcome::ExceptionOrEret;
            }
        }

        self.set_gpr_128(rd, out);
        StepOutcome::Normal
    }

    /// Execute an `MMI1` (`opcode == 0x1C, funct == 0x28`) instruction decoded by `shamt`.
    fn execute_mmi1(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        shamt: u32,
    ) -> StepOutcome {
        let a = self.gpr_128(rs);
        let b = self.gpr_128(rt);
        let mut out = Reg128::ZERO;

        match shamt {
            // PABSW (0x01)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/arithmetic.expected` lines 28..52):
            // `i32::MIN` saturates to `i32::MAX` (`0x7FFF_FFFF`).
            0x01 => {
                for i in 0..4 {
                    out.i32_lanes_mut()[i] = b.i32_lanes()[i].saturating_abs();
                }
            }
            // PCEQW (0x02)
            0x02 => {
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = if a.u32_lanes()[i] == b.u32_lanes()[i] {
                        0xFFFF_FFFF
                    } else {
                        0
                    };
                }
            }
            // PMINW (0x03)
            0x03 => {
                for i in 0..4 {
                    out.i32_lanes_mut()[i] = a.i32_lanes()[i].min(b.i32_lanes()[i]);
                }
            }
            // PADSBH (0x04)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/arithmetic.expected` lines 324..352).
            0x04 => {
                for i in 0..4 {
                    out.u16_lanes_mut()[i] = a.u16_lanes()[i].wrapping_sub(b.u16_lanes()[i]);
                }
                for i in 4..8 {
                    out.u16_lanes_mut()[i] = a.u16_lanes()[i].wrapping_add(b.u16_lanes()[i]);
                }
            }
            // PABSH (0x05)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/arithmetic.expected` lines 2..26):
            // `i16::MIN` saturates to `i16::MAX` (`0x7FFF`).
            0x05 => {
                for i in 0..8 {
                    out.i16_lanes_mut()[i] = b.i16_lanes()[i].saturating_abs();
                }
            }
            // PCEQH (0x06)
            0x06 => {
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = if a.u16_lanes()[i] == b.u16_lanes()[i] {
                        0xFFFF
                    } else {
                        0
                    };
                }
            }
            // PMINH (0x07)
            0x07 => {
                for i in 0..8 {
                    out.i16_lanes_mut()[i] = a.i16_lanes()[i].min(b.i16_lanes()[i]);
                }
            }
            // PCEQB (0x0A)
            0x0A => {
                for i in 0..16 {
                    out.u8_lanes_mut()[i] = if a.u8_lanes()[i] == b.u8_lanes()[i] {
                        0xFF
                    } else {
                        0
                    };
                }
            }
            // PADDUW (0x10)
            0x10 => {
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = a.u32_lanes()[i].saturating_add(b.u32_lanes()[i]);
                }
            }
            // PSUBUW (0x11)
            0x11 => {
                for i in 0..4 {
                    out.u32_lanes_mut()[i] = a.u32_lanes()[i].saturating_sub(b.u32_lanes()[i]);
                }
            }
            // PEXTUW (0x12)
            0x12 => {
                let aw = a.u32_lanes();
                let bw = b.u32_lanes();
                out = Reg128::from_u32x4(bw[2], aw[2], bw[3], aw[3]);
            }
            // PADDUH (0x14)
            0x14 => {
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = a.u16_lanes()[i].saturating_add(b.u16_lanes()[i]);
                }
            }
            // PSUBUH (0x15)
            0x15 => {
                for i in 0..8 {
                    out.u16_lanes_mut()[i] = a.u16_lanes()[i].saturating_sub(b.u16_lanes()[i]);
                }
            }
            // PEXTUH (0x16)
            0x16 => {
                for i in 0..4 {
                    out.u16_lanes_mut()[2 * i] = b.u16_lanes()[i + 4];
                    out.u16_lanes_mut()[2 * i + 1] = a.u16_lanes()[i + 4];
                }
            }
            // PADDUB (0x18)
            0x18 => {
                for i in 0..16 {
                    out.u8_lanes_mut()[i] = a.u8_lanes()[i].saturating_add(b.u8_lanes()[i]);
                }
            }
            // PSUBUB (0x19)
            0x19 => {
                for i in 0..16 {
                    out.u8_lanes_mut()[i] = a.u8_lanes()[i].saturating_sub(b.u8_lanes()[i]);
                }
            }
            // PEXTUB (0x1A)
            0x1A => {
                for i in 0..8 {
                    out.u8_lanes_mut()[2 * i] = b.u8_lanes()[i + 8];
                    out.u8_lanes_mut()[2 * i + 1] = a.u8_lanes()[i + 8];
                }
            }
            // QFSRV rd, rs, rt (0x1B)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 274..309).
            0x1B => {
                let byte_shift = (self.sa & 0xF) as usize;
                if byte_shift == 0 {
                    out = b;
                } else {
                    let bit_shift = byte_shift * 8;
                    out = Reg128((b.0 >> bit_shift) | (a.0 << (128 - bit_shift)));
                }
            }
            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    shamt,
                    "Reserved MMI1 shamt"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                return StepOutcome::ExceptionOrEret;
            }
        }

        self.set_gpr_128(rd, out);
        StepOutcome::Normal
    }

    /// Execute an `MMI2` (`opcode == 0x1C, funct == 0x09`) instruction decoded by `shamt`.
    fn execute_mmi2(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        shamt: u32,
    ) -> StepOutcome {
        let a = self.gpr_128(rs);
        let b = self.gpr_128(rt);
        let mut out = Reg128::ZERO;

        match shamt {
            // PMADDW rd, rs, rt (0x00)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 211..280).
            0x00 => {
                for half in 0..2 {
                    let w = half * 2;
                    let acc = ((u64::from(self.hi.u32_lanes()[w])) << 32)
                        | u64::from(self.lo.u32_lanes()[w]);
                    let prod = i64::from(a.i32_lanes()[w]) * i64::from(b.i32_lanes()[w]);
                    let res = acc.wrapping_add(prod as u64);
                    let lo32 = res as i32;
                    let hi32 = (res >> 32) as i32;
                    self.lo.i64_lanes_mut()[half] = i64::from(lo32);
                    self.hi.i64_lanes_mut()[half] = i64::from(hi32);
                    out.i32_lanes_mut()[w] = lo32;
                    out.i32_lanes_mut()[w + 1] = hi32;
                }
            }
            // PSLLVW rd, rt, rs (0x02)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 135..160).
            0x02 => {
                for half in 0..2 {
                    let w = half * 2;
                    let s = a.u32_lanes()[w] & 0x1F;
                    let res = (b.u32_lanes()[w] << s) as i32;
                    out.i64_lanes_mut()[half] = i64::from(res);
                }
            }
            // PSRLVW rd, rt, rs (0x03)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 189..214).
            0x03 => {
                for half in 0..2 {
                    let w = half * 2;
                    let s = a.u32_lanes()[w] & 0x1F;
                    let res = (b.u32_lanes()[w] >> s) as i32;
                    out.i64_lanes_mut()[half] = i64::from(res);
                }
            }
            // PMSUBW rd, rs, rt (0x04)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 316..350).
            0x04 => {
                for half in 0..2 {
                    let w = half * 2;
                    let acc = ((u64::from(self.hi.u32_lanes()[w])) << 32)
                        | u64::from(self.lo.u32_lanes()[w]);
                    let prod = i64::from(a.i32_lanes()[w]) * i64::from(b.i32_lanes()[w]);
                    let res = acc.wrapping_sub(prod as u64);
                    let lo32 = res as i32;
                    let hi32 = (res >> 32) as i32;
                    self.lo.i64_lanes_mut()[half] = i64::from(lo32);
                    self.hi.i64_lanes_mut()[half] = i64::from(hi32);
                    out.i32_lanes_mut()[w] = lo32;
                    out.i32_lanes_mut()[w + 1] = hi32;
                }
            }
            // PMFHI rd (0x08)
            0x08 => {
                out = self.hi;
            }
            // PMFLO rd (0x09)
            0x09 => {
                out = self.lo;
            }
            // PINTH rd, rs, rt (0x0A)
            0x0A => {
                for i in 0..4 {
                    out.u16_lanes_mut()[2 * i] = b.u16_lanes()[i];
                    out.u16_lanes_mut()[2 * i + 1] = a.u16_lanes()[i + 4];
                }
            }
            // PMULTW rd, rs, rt (0x0C)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 421..455).
            0x0C => {
                for half in 0..2 {
                    let w = half * 2;
                    let prod = i64::from(a.i32_lanes()[w]) * i64::from(b.i32_lanes()[w]);
                    let lo32 = prod as i32;
                    let hi32 = (prod >> 32) as i32;
                    self.lo.i64_lanes_mut()[half] = i64::from(lo32);
                    self.hi.i64_lanes_mut()[half] = i64::from(hi32);
                    out.i32_lanes_mut()[w] = lo32;
                    out.i32_lanes_mut()[w + 1] = hi32;
                }
            }
            // PDIVW rs, rt (0x0D)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 73..107).
            0x0D => {
                for half in 0..2 {
                    let w = half * 2;
                    let (q, r) = div_signed_32(a.i32_lanes()[w], b.i32_lanes()[w]);
                    self.lo.i64_lanes_mut()[half] = i64::from(q);
                    self.hi.i64_lanes_mut()[half] = i64::from(r);
                }
                return StepOutcome::Normal;
            }
            // PCPYLD rd, rs, rt (0x0E)
            0x0E => {
                out = Reg128::from_u64_pair(b.lo_u64(), a.lo_u64());
            }
            // PMADDH rd, rs, rt (0x10)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 173..209).
            0x10 => {
                let mut p = [0u32; 8];
                for (i, item) in p.iter_mut().enumerate() {
                    *item = (i32::from(a.i16_lanes()[i]) * i32::from(b.i16_lanes()[i])) as u32;
                }
                let lw = self.lo.u32_lanes_mut();
                lw[0] = lw[0].wrapping_add(p[0]);
                lw[1] = lw[1].wrapping_add(p[1]);
                lw[2] = lw[2].wrapping_add(p[4]);
                lw[3] = lw[3].wrapping_add(p[5]);
                let hw = self.hi.u32_lanes_mut();
                hw[0] = hw[0].wrapping_add(p[2]);
                hw[1] = hw[1].wrapping_add(p[3]);
                hw[2] = hw[2].wrapping_add(p[6]);
                hw[3] = hw[3].wrapping_add(p[7]);
                out = Reg128::from_u32x4(
                    self.lo.u32_lanes()[0],
                    self.hi.u32_lanes()[0],
                    self.lo.u32_lanes()[2],
                    self.hi.u32_lanes()[2],
                );
            }
            // PHMADH rd, rs, rt (0x11)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 118..144).
            0x11 => {
                let mut p = [0i32; 8];
                for (i, item) in p.iter_mut().enumerate() {
                    *item = i32::from(a.i16_lanes()[i]) * i32::from(b.i16_lanes()[i]);
                }
                let r0 = p[1].wrapping_add(p[0]) as u32;
                let r1 = p[3].wrapping_add(p[2]) as u32;
                let r2 = p[5].wrapping_add(p[4]) as u32;
                let r3 = p[7].wrapping_add(p[6]) as u32;
                self.lo = Reg128::from_u32x4(r0, p[1] as u32, r2, p[5] as u32);
                self.hi = Reg128::from_u32x4(r1, p[3] as u32, r3, p[7] as u32);
                out = Reg128::from_u32x4(r0, r1, r2, r3);
            }
            // PAND rd, rs, rt (0x12)
            0x12 => {
                out = Reg128(a.0 & b.0);
            }
            // PXOR rd, rs, rt (0x13)
            0x13 => {
                out = Reg128(a.0 ^ b.0);
            }
            // PMSUBH rd, rs, rt (0x14)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 281..315).
            0x14 => {
                let mut p = [0u32; 8];
                for (i, item) in p.iter_mut().enumerate() {
                    *item = (i32::from(a.i16_lanes()[i]) * i32::from(b.i16_lanes()[i])) as u32;
                }
                let lw = self.lo.u32_lanes_mut();
                lw[0] = lw[0].wrapping_sub(p[0]);
                lw[1] = lw[1].wrapping_sub(p[1]);
                lw[2] = lw[2].wrapping_sub(p[4]);
                lw[3] = lw[3].wrapping_sub(p[5]);
                let hw = self.hi.u32_lanes_mut();
                hw[0] = hw[0].wrapping_sub(p[2]);
                hw[1] = hw[1].wrapping_sub(p[3]);
                hw[2] = hw[2].wrapping_sub(p[6]);
                hw[3] = hw[3].wrapping_sub(p[7]);
                out = Reg128::from_u32x4(
                    self.lo.u32_lanes()[0],
                    self.hi.u32_lanes()[0],
                    self.lo.u32_lanes()[2],
                    self.hi.u32_lanes()[2],
                );
            }
            // PHMSBH rd, rs, rt (0x15)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 145..171):
            // Upper 32-bit words of each 64-bit half of LO/HI hold `~p[2*i + 1]`.
            0x15 => {
                let mut p = [0i32; 8];
                for (i, item) in p.iter_mut().enumerate() {
                    *item = i32::from(a.i16_lanes()[i]) * i32::from(b.i16_lanes()[i]);
                }
                let r0 = p[1].wrapping_sub(p[0]) as u32;
                let r1 = p[3].wrapping_sub(p[2]) as u32;
                let r2 = p[5].wrapping_sub(p[4]) as u32;
                let r3 = p[7].wrapping_sub(p[6]) as u32;
                self.lo = Reg128::from_u32x4(r0, !(p[1] as u32), r2, !(p[5] as u32));
                self.hi = Reg128::from_u32x4(r1, !(p[3] as u32), r3, !(p[7] as u32));
                out = Reg128::from_u32x4(r0, r1, r2, r3);
            }
            // PEXEH rd, rt (0x1A): Parallel Exchange Even Halfword.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 140..164).
            0x1A => {
                let bh = b.u16_lanes();
                let out_h = out.u16_lanes_mut();
                out_h[0] = bh[2];
                out_h[1] = bh[1];
                out_h[2] = bh[0];
                out_h[3] = bh[3];
                out_h[4] = bh[6];
                out_h[5] = bh[5];
                out_h[6] = bh[4];
                out_h[7] = bh[7];
            }
            // PREVH rd, rt (0x1B): Parallel Reverse Halfword.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 574..598).
            0x1B => {
                let bh = b.u16_lanes();
                let out_h = out.u16_lanes_mut();
                out_h[0] = bh[3];
                out_h[1] = bh[2];
                out_h[2] = bh[1];
                out_h[3] = bh[0];
                out_h[4] = bh[7];
                out_h[5] = bh[6];
                out_h[6] = bh[5];
                out_h[7] = bh[4];
            }
            // PMULTH rd, rs, rt (0x1C)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 351..384).
            0x1C => {
                let mut p = [0u32; 8];
                for (i, item) in p.iter_mut().enumerate() {
                    *item = (i32::from(a.i16_lanes()[i]) * i32::from(b.i16_lanes()[i])) as u32;
                }
                self.lo = Reg128::from_u32x4(p[0], p[1], p[4], p[5]);
                self.hi = Reg128::from_u32x4(p[2], p[3], p[6], p[7]);
                out = Reg128::from_u32x4(p[0], p[2], p[4], p[6]);
            }
            // PDIVBW rs, rt (0x1D)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 2..36).
            0x1D => {
                let d = i32::from(b.i16_lanes()[0]);
                for i in 0..4 {
                    let (q, r) = div_signed_32(a.i32_lanes()[i], d);
                    self.lo.i32_lanes_mut()[i] = q;
                    self.hi.i32_lanes_mut()[i] = r;
                }
                return StepOutcome::Normal;
            }
            // PEXEW rd, rt (0x1E): Parallel Exchange Even Word.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 166..190).
            0x1E => {
                let bw = b.u32_lanes();
                out = Reg128::from_u32x4(bw[2], bw[1], bw[0], bw[3]);
            }
            // PROT3W rd, rt (0x1F): Parallel Rotate 3 Words.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 600..624).
            0x1F => {
                let bw = b.u32_lanes();
                out = Reg128::from_u32x4(bw[1], bw[2], bw[0], bw[3]);
            }
            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    shamt,
                    "Reserved MMI2 shamt"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                return StepOutcome::ExceptionOrEret;
            }
        }

        self.set_gpr_128(rd, out);
        StepOutcome::Normal
    }

    /// Execute an `MMI3` (`opcode == 0x1C, funct == 0x29`) instruction decoded by `shamt`.
    fn execute_mmi3(
        &mut self,
        instr: u32,
        instr_pc: u32,
        rs: usize,
        rt: usize,
        rd: usize,
        shamt: u32,
    ) -> StepOutcome {
        let a = self.gpr_128(rs);
        let b = self.gpr_128(rt);
        let mut out = Reg128::ZERO;

        match shamt {
            // PMADDUW rd, rs, rt (0x00)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 246..280).
            0x00 => {
                for half in 0..2 {
                    let w = half * 2;
                    let acc = ((u64::from(self.hi.u32_lanes()[w])) << 32)
                        | u64::from(self.lo.u32_lanes()[w]);
                    let prod = u64::from(a.u32_lanes()[w]) * u64::from(b.u32_lanes()[w]);
                    let res = acc.wrapping_add(prod);
                    let lo32 = res as i32;
                    let hi32 = (res >> 32) as i32;
                    self.lo.i64_lanes_mut()[half] = i64::from(lo32);
                    self.hi.i64_lanes_mut()[half] = i64::from(hi32);
                    out.i32_lanes_mut()[w] = lo32;
                    out.i32_lanes_mut()[w + 1] = hi32;
                }
            }
            // PSRAVW rd, rt, rs (0x03)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/funnel.expected` lines 162..187).
            0x03 => {
                for half in 0..2 {
                    let w = half * 2;
                    let s = a.u32_lanes()[w] & 0x1F;
                    let res = b.i32_lanes()[w] >> s;
                    out.i64_lanes_mut()[half] = i64::from(res);
                }
            }
            // PMTHI rs (0x08)
            0x08 => {
                self.hi = a;
                return StepOutcome::Normal;
            }
            // PMTLO rs (0x09)
            0x09 => {
                self.lo = a;
                return StepOutcome::Normal;
            }
            // PINTEH rd, rs, rt (0x0A)
            0x0A => {
                for i in 0..4 {
                    out.u16_lanes_mut()[2 * i] = b.u16_lanes()[2 * i];
                    out.u16_lanes_mut()[2 * i + 1] = a.u16_lanes()[2 * i];
                }
            }
            // PMULTUW rd, rs, rt (0x0C)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 456..490).
            0x0C => {
                for half in 0..2 {
                    let w = half * 2;
                    let prod = u64::from(a.u32_lanes()[w]) * u64::from(b.u32_lanes()[w]);
                    let lo32 = prod as i32;
                    let hi32 = (prod >> 32) as i32;
                    self.lo.i64_lanes_mut()[half] = i64::from(lo32);
                    self.hi.i64_lanes_mut()[half] = i64::from(hi32);
                    out.i32_lanes_mut()[w] = lo32;
                    out.i32_lanes_mut()[w + 1] = hi32;
                }
            }
            // PDIVUW rs, rt (0x0D)
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 37..72).
            0x0D => {
                for half in 0..2 {
                    let w = half * 2;
                    let (q, r) = div_unsigned_32(a.u32_lanes()[w], b.u32_lanes()[w]);
                    self.lo.i64_lanes_mut()[half] = i64::from(q);
                    self.hi.i64_lanes_mut()[half] = i64::from(r);
                }
                return StepOutcome::Normal;
            }
            // PCPYUD rd, rs, rt (0x0E)
            0x0E => {
                out = Reg128::from_u64_pair(a.hi_u64(), b.hi_u64());
            }
            // POR rd, rs, rt (0x12)
            0x12 => {
                out = Reg128(a.0 | b.0);
            }
            // PNOR rd, rs, rt (0x13)
            0x13 => {
                out = Reg128(!(a.0 | b.0));
            }
            // PEXCH rd, rt (0x1A): Parallel Exchange Center Halfword.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 88..112).
            0x1A => {
                let bh = b.u16_lanes();
                let out_h = out.u16_lanes_mut();
                out_h[0] = bh[0];
                out_h[1] = bh[2];
                out_h[2] = bh[1];
                out_h[3] = bh[3];
                out_h[4] = bh[4];
                out_h[5] = bh[6];
                out_h[6] = bh[5];
                out_h[7] = bh[7];
            }
            // PCPYH rd, rt (0x1B): Parallel Copy Halfword.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 1..30).
            0x1B => {
                let bh = b.u16_lanes();
                let out_h = out.u16_lanes_mut();
                for item in out_h.iter_mut().take(4) {
                    *item = bh[0];
                }
                for item in out_h.iter_mut().skip(4) {
                    *item = bh[4];
                }
            }
            // PEXCW rd, rt (0x1E): Parallel Exchange Center Word.
            // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/shuffle.expected` lines 114..138).
            0x1E => {
                let bw = b.u32_lanes();
                out = Reg128::from_u32x4(bw[0], bw[2], bw[1], bw[3]);
            }
            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    shamt,
                    "Reserved MMI3 shamt"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                return StepOutcome::ExceptionOrEret;
            }
        }

        self.set_gpr_128(rd, out);
        StepOutcome::Normal
    }

    /// Execute `PMFHL` (`opcode == 0x1C, funct == 0x30`, sub-op `shamt = 0..=4`).
    ///
    /// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 491..645).
    fn execute_pmfhl(&mut self, instr: u32, instr_pc: u32, rd: usize, shamt: u32) -> StepOutcome {
        let lw = self.lo.u32_lanes();
        let hw = self.hi.u32_lanes();
        let mut out = Reg128::ZERO;

        match shamt {
            // PMFHL.LW (0x00)
            0x00 => {
                out = Reg128::from_u32x4(lw[0], hw[0], lw[2], hw[2]);
            }
            // PMFHL.UW (0x01)
            0x01 => {
                out = Reg128::from_u32x4(lw[1], hw[1], lw[3], hw[3]);
            }
            // PMFHL.SLW (0x02): Saturate 64-bit `(HI.w0 << 32) | LO.w0` and `(HI.w2 << 32) | LO.w2` to signed 32-bit, sign-extended to 64 bits.
            0x02 => {
                let v0 = (((u64::from(hw[0])) << 32) | u64::from(lw[0])) as i64;
                let v1 = (((u64::from(hw[2])) << 32) | u64::from(lw[2])) as i64;
                let s0 = v0.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
                let s1 = v1.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
                out = Reg128::from_u64_pair(s0 as u64, s1 as u64);
            }
            // PMFHL.LH (0x03)
            0x03 => {
                let words = [lw[0], lw[1], hw[0], hw[1], lw[2], lw[3], hw[2], hw[3]];
                for (i, &w) in words.iter().enumerate() {
                    out.u16_lanes_mut()[i] = w as u16;
                }
            }
            // PMFHL.SH (0x04)
            0x04 => {
                let words = [lw[0], lw[1], hw[0], hw[1], lw[2], lw[3], hw[2], hw[3]];
                for (i, &w) in words.iter().enumerate() {
                    let clamped = (w as i32).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                    out.i16_lanes_mut()[i] = clamped as i16;
                }
            }
            _ => {
                tracing::debug!(
                    pc = format_args!("0x{instr_pc:08X}"),
                    instr = format_args!("0x{instr:08X}"),
                    shamt,
                    "Reserved PMFHL shamt"
                );
                self.raise_level1_exception(Level1ExceptionCode::ReservedInstruction, false);
                return StepOutcome::ExceptionOrEret;
            }
        }

        self.set_gpr_128(rd, out);
        StepOutcome::Normal
    }
}

/// Count leading sign bits minus 1 for a single 32-bit word (`PLZCW`).
#[inline(always)]
const fn plzcw_word(w: u32) -> u32 {
    let inverted = if (w as i32) < 0 { !w } else { w };
    inverted.leading_zeros() - 1
}

/// R5900 signed 32-bit division (`(quotient, remainder)`).
#[inline(always)]
const fn div_signed_32(n: i32, d: i32) -> (i32, i32) {
    if d == 0 {
        (if n < 0 { 1 } else { -1 }, n)
    } else if n == i32::MIN && d == -1 {
        (i32::MIN, 0)
    } else {
        (n / d, n % d)
    }
}

/// R5900 unsigned 32-bit division (`(quotient, remainder)` as `i32` for sign-extension).
#[inline(always)]
const fn div_unsigned_32(n: u32, d: u32) -> (i32, i32) {
    if let Some(q) = n.checked_div(d) {
        (q as i32, (n % d) as i32)
    } else {
        (-1, n as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    const fn encode_mmi(rs: u32, rt: u32, rd: u32, shamt: u32, funct: u32) -> u32 {
        (0x1C << 26) | (rs << 21) | (rt << 16) | (rd << 11) | (shamt << 6) | funct
    }

    fn run_instrs(cpu: &mut EeCpuState, bus: &mut Bus, instrs: &[u32]) {
        for (i, &instr) in instrs.iter().enumerate() {
            bus.write32(0x8010_0000 + (i as u32) * 4, instr);
        }
        cpu.pc = 0x8010_0000;
        cpu.next_pc = 0x8010_0004;
        for _ in 0..instrs.len() {
            cpu.step(bus);
        }
    }

    #[test]
    fn mmi_plzcw_and_qfsrv() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // Set $t0 (8) lower 64 bits to [0x0000_000F, 0xFFFF_FFF0]
        cpu.set_gpr_128(8, Reg128::from_u32x4(0x0000_000F, 0xFFFF_FFF0, 0, 0));
        // PLZCW $t1 (9), $t0 (8): funct = 0x04
        // MTSAB $zero, 4: opcode 0x01, rs = 0, rt = 0x18, imm = 4
        let plzcw = encode_mmi(8, 0, 9, 0, 0x04);
        let mtsab = (0x01 << 26) | (0x18 << 16) | 4;
        // QFSRV $t2 (10), $t3 (11), $t4 (12): MMI1 (funct = 0x28), shamt = 0x1B
        cpu.set_gpr_128(
            11,
            Reg128::from_u64_pair(0xDEAD_BEEF_CAFE_BABE, 0x0123_4567_89AB_CDEF),
        );
        cpu.set_gpr_128(
            12,
            Reg128::from_u64_pair(0x1122_3344_5566_7788, 0x99AA_BBCC_DDEE_FF00),
        );
        let qfsrv = encode_mmi(11, 12, 10, 0x1B, 0x28);

        run_instrs(&mut cpu, &mut bus, &[plzcw, mtsab, qfsrv]);

        assert_eq!(cpu.gpr_128(9).u32_lanes()[0], 27);
        assert_eq!(cpu.gpr_128(9).u32_lanes()[1], 27);
        assert_eq!(cpu.sa, 4);
        assert_eq!(
            cpu.gpr_128(10),
            Reg128::from_u32x4(0x1122_3344, 0xDDEE_FF00, 0x99AA_BBCC, 0xCAFE_BABE)
        );
    }

    #[test]
    fn mmi0_mmi1_saturating_add_sub_and_ext() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        cpu.set_gpr_128(
            1,
            Reg128::from_u32x4(0x7FFF_FFF0, 0x8000_0010, 0x0000_0001, 0xFFFF_FFFF),
        );
        cpu.set_gpr_128(
            2,
            Reg128::from_u32x4(0x0000_0020, 0x8000_0000, 0x0000_0002, 0x0000_0001),
        );

        // PADDSW $3, $1, $2 (MMI0 shamt 0x10)
        let paddsw = encode_mmi(1, 2, 3, 0x10, 0x08);
        // PEXTLW $4, $1, $2 (MMI0 shamt 0x12)
        let pextlw = encode_mmi(1, 2, 4, 0x12, 0x08);
        // PADDUW $5, $1, $2 (MMI1 shamt 0x10)
        let padduw = encode_mmi(1, 2, 5, 0x10, 0x28);

        run_instrs(&mut cpu, &mut bus, &[paddsw, pextlw, padduw]);

        assert_eq!(
            cpu.gpr_128(3),
            Reg128::from_u32x4(0x7FFF_FFFF, 0x8000_0000, 3, 0)
        );
        assert_eq!(
            cpu.gpr_128(4),
            Reg128::from_u32x4(0x0000_0020, 0x7FFF_FFF0, 0x8000_0000, 0x8000_0010)
        );
        assert_eq!(
            cpu.gpr_128(5),
            Reg128::from_u32x4(0x8000_0010, 0xFFFF_FFFF, 3, 0xFFFF_FFFF)
        );
    }

    #[test]
    fn mmi2_mmi3_phmsbh_and_pmfhl() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        cpu.set_gpr_128(1, Reg128::from_u32x4(0x0002_0003, 0x0004_0005, 0, 0));
        cpu.set_gpr_128(2, Reg128::from_u32x4(0x0001_0001, 0x0001_0001, 0, 0));

        // PHMSBH $3, $1, $2 (MMI2 funct=0x09, shamt=0x15)
        let phmsbh = encode_mmi(1, 2, 3, 0x15, 0x09);
        // PMFHL.UW $4 (funct=0x30, shamt=0x01)
        let pmfhl_uw = encode_mmi(0, 0, 4, 0x01, 0x30);

        run_instrs(&mut cpu, &mut bus, &[phmsbh, pmfhl_uw]);

        // p[0] = 3*1 = 3, p[1] = 2*1 = 2 -> r0 = 2 - 3 = -1 (0xFFFF_FFFF)
        // p[2] = 5*1 = 5, p[3] = 4*1 = 4 -> r1 = 4 - 5 = -1 (0xFFFF_FFFF)
        assert_eq!(
            cpu.gpr_128(3),
            Reg128::from_u32x4(0xFFFF_FFFF, 0xFFFF_FFFF, 0, 0)
        );
        // Upper words of LO/HI hold !p[1] = !2, !p[3] = !4, !p[5] = !0, !p[7] = !0
        assert_eq!(
            cpu.gpr_128(4),
            Reg128::from_u32x4(!2u32, !4u32, !0u32, !0u32)
        );
    }
}
