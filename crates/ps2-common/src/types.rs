//! 128-bit register representation and fixed-size hardware types.
//!
//! Primary source reference:
//! - `ps2tek` ("Emotion Engine -> Registers & Data Types"):
//!   All 32 EE General Purpose Registers (GPRs) are 128 bits wide (`GPR[0]` is hardwired to 0).
//!   Standard 32-bit and 64-bit scalar MIPS instructions modify only the lower 64 bits
//!   (`bits 0..63`), leaving the upper 64 bits (`bits 64..127`) untouched. Only 128-bit
//!   Quadword loads (`LQ`) and Multimedia Instructions (`MMI`) modify the upper 64 bits.

use bytemuck::{Pod, Zeroable};

/// 128-bit Emotion Engine General Purpose / Vector Register.
///
/// Aligned to 16 bytes (`#[repr(C, align(16))]`) for zero-cost host SIMD (`SSE`/`AVX2`/`NEON`)
/// loads and stores in the JIT and safe zero-copy lane reinterpretation via `bytemuck`.
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Reg128(pub u128);

impl Reg128 {
    /// Constant zeroed 128-bit register (`0x00000000_00000000_00000000_00000000`).
    pub const ZERO: Self = Self(0);

    /// Construct a `Reg128` from a raw `u128` value.
    #[inline(always)]
    pub const fn from_u128(val: u128) -> Self {
        Self(val)
    }

    /// Construct a `Reg128` from lower (`bits 0..63`) and upper (`bits 64..127`) 64-bit lanes.
    #[inline(always)]
    pub const fn from_u64_pair(lo: u64, hi: u64) -> Self {
        Self((lo as u128) | ((hi as u128) << 64))
    }

    /// Construct a `Reg128` from four 32-bit unsigned lanes (`w0` = bits `0..31` .. `w3` = bits `96..127`).
    #[inline(always)]
    pub const fn from_u32x4(w0: u32, w1: u32, w2: u32, w3: u32) -> Self {
        let lo = (w0 as u64) | ((w1 as u64) << 32);
        let hi = (w2 as u64) | ((w3 as u64) << 32);
        Self::from_u64_pair(lo, hi)
    }

    /// Return the full 128-bit unsigned integer value.
    #[inline(always)]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// Return the lower 64 bits (`bits 0..63`) as `u64`.
    #[inline(always)]
    pub const fn lo_u64(self) -> u64 {
        self.0 as u64
    }

    /// Return the upper 64 bits (`bits 64..127`) as `u64`.
    #[inline(always)]
    pub const fn hi_u64(self) -> u64 {
        (self.0 >> 64) as u64
    }

    /// Return the lower 64 bits (`bits 0..63`) as signed `i64`.
    #[inline(always)]
    pub const fn lo_i64(self) -> i64 {
        self.lo_u64() as i64
    }

    /// Return the upper 64 bits (`bits 64..127`) as signed `i64`.
    #[inline(always)]
    pub const fn hi_i64(self) -> i64 {
        self.hi_u64() as i64
    }

    /// Return the lowest 32 bits (`bits 0..31`) as `u32`.
    #[inline(always)]
    pub const fn lo_u32(self) -> u32 {
        self.0 as u32
    }

    /// Return the lowest 32 bits (`bits 0..31`) as signed `i32`.
    #[inline(always)]
    pub const fn lo_i32(self) -> i32 {
        self.lo_u32() as i32
    }

    /// Write the lower 64 bits (`bits 0..63`) while preserving the upper 64 bits (`bits 64..127`).
    ///
    /// Reference: `ps2tek` ("EE Core Instruction Set") — 64-bit scalar operations (`DADDU`, `LD`, etc.)
    /// do not touch bits `64..127`.
    #[inline(always)]
    pub fn set_lo_u64(&mut self, val: u64) {
        self.u64_lanes_mut()[0] = val;
    }

    /// Write the upper 64 bits (`bits 64..127`) while preserving the lower 64 bits (`bits 0..63`).
    #[inline(always)]
    pub fn set_hi_u64(&mut self, val: u64) {
        self.u64_lanes_mut()[1] = val;
    }

    /// Write the lower 64 bits (`bits 0..63`) as signed `i64` while preserving the upper 64 bits.
    #[inline(always)]
    pub fn set_lo_i64(&mut self, val: i64) {
        self.i64_lanes_mut()[0] = val;
    }

    /// Sign-extend a 32-bit value into the lower 64 bits (`bits 0..63`) while preserving
    /// the upper 64 bits (`bits 64..127`).
    ///
    /// Reference: `ps2tek` ("EE Core Instruction Set") — 32-bit MIPS instructions (`ADDU`, `LW`, `LUI`,
    /// `SLL`, `MULT`, etc.) sign-extend bit 31 into bits `32..63` and preserve bits `64..127`.
    #[inline(always)]
    pub fn set_sext32(&mut self, val: i32) {
        self.i64_lanes_mut()[0] = i64::from(val);
    }

    /// View the register as two 64-bit unsigned integer lanes `[lo, hi]`.
    #[inline(always)]
    pub fn u64_lanes(&self) -> &[u64; 2] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as two 64-bit unsigned integer lanes `[lo, hi]`.
    #[inline(always)]
    pub fn u64_lanes_mut(&mut self) -> &mut [u64; 2] {
        bytemuck::cast_mut(self)
    }

    /// View the register as two 64-bit signed integer lanes `[lo, hi]`.
    #[inline(always)]
    pub fn i64_lanes(&self) -> &[i64; 2] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as two 64-bit signed integer lanes `[lo, hi]`.
    #[inline(always)]
    pub fn i64_lanes_mut(&mut self) -> &mut [i64; 2] {
        bytemuck::cast_mut(self)
    }

    /// View the register as four 32-bit unsigned integer lanes `[w0, w1, w2, w3]`.
    #[inline(always)]
    pub fn u32_lanes(&self) -> &[u32; 4] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as four 32-bit unsigned integer lanes `[w0, w1, w2, w3]`.
    #[inline(always)]
    pub fn u32_lanes_mut(&mut self) -> &mut [u32; 4] {
        bytemuck::cast_mut(self)
    }

    /// View the register as four 32-bit signed integer lanes `[w0, w1, w2, w3]`.
    #[inline(always)]
    pub fn i32_lanes(&self) -> &[i32; 4] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as four 32-bit signed integer lanes `[w0, w1, w2, w3]`.
    #[inline(always)]
    pub fn i32_lanes_mut(&mut self) -> &mut [i32; 4] {
        bytemuck::cast_mut(self)
    }

    /// View the register as eight 16-bit unsigned integer lanes `[h0..h7]`.
    #[inline(always)]
    pub fn u16_lanes(&self) -> &[u16; 8] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as eight 16-bit unsigned integer lanes `[h0..h7]`.
    #[inline(always)]
    pub fn u16_lanes_mut(&mut self) -> &mut [u16; 8] {
        bytemuck::cast_mut(self)
    }

    /// View the register as eight 16-bit signed integer lanes `[h0..h7]`.
    #[inline(always)]
    pub fn i16_lanes(&self) -> &[i16; 8] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as eight 16-bit signed integer lanes `[h0..h7]`.
    #[inline(always)]
    pub fn i16_lanes_mut(&mut self) -> &mut [i16; 8] {
        bytemuck::cast_mut(self)
    }

    /// View the register as sixteen 8-bit unsigned byte lanes `[b0..b15]`.
    #[inline(always)]
    pub fn u8_lanes(&self) -> &[u8; 16] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as sixteen 8-bit unsigned byte lanes `[b0..b15]`.
    #[inline(always)]
    pub fn u8_lanes_mut(&mut self) -> &mut [u8; 16] {
        bytemuck::cast_mut(self)
    }

    /// View the register as sixteen 8-bit signed byte lanes `[b0..b15]`.
    #[inline(always)]
    pub fn i8_lanes(&self) -> &[i8; 16] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as sixteen 8-bit signed byte lanes `[b0..b15]`.
    #[inline(always)]
    pub fn i8_lanes_mut(&mut self) -> &mut [i8; 16] {
        bytemuck::cast_mut(self)
    }

    /// View the register as four 32-bit float lanes `[x, y, z, w]`.
    #[inline(always)]
    pub fn f32_lanes(&self) -> &[f32; 4] {
        bytemuck::cast_ref(self)
    }

    /// Mutably view the register as four 32-bit float lanes `[x, y, z, w]`.
    #[inline(always)]
    pub fn f32_lanes_mut(&mut self) -> &mut [f32; 4] {
        bytemuck::cast_mut(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg128_size_and_alignment() {
        assert_eq!(core::mem::size_of::<Reg128>(), 16);
        assert_eq!(core::mem::align_of::<Reg128>(), 16);
    }

    #[test]
    fn reg128_scalar_writes_preserve_upper_64_bits() {
        let mut r = Reg128::from_u64_pair(0x1111_2222_3333_4444, 0xAAAA_BBBB_CCCC_DDDD);
        r.set_sext32(-1);
        assert_eq!(r.lo_u64(), 0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(r.hi_u64(), 0xAAAA_BBBB_CCCC_DDDD);

        r.set_sext32(0x7FFF_FFFF);
        assert_eq!(r.lo_u64(), 0x0000_0000_7FFF_FFFF);
        assert_eq!(r.hi_u64(), 0xAAAA_BBBB_CCCC_DDDD);

        r.set_lo_u64(0x0123_4567_89AB_CDEF);
        assert_eq!(r.lo_u64(), 0x0123_4567_89AB_CDEF);
        assert_eq!(r.hi_u64(), 0xAAAA_BBBB_CCCC_DDDD);
    }

    #[test]
    fn reg128_lane_views_match_little_endian_layout() {
        let r = Reg128::from_u32x4(0x0302_0100, 0x0706_0504, 0x0B0A_0908, 0x0F0E_0D0C);
        for (i, &b) in r.u8_lanes().iter().enumerate() {
            assert_eq!(usize::from(b), i);
        }
        assert_eq!(r.u16_lanes()[0], 0x0100);
        assert_eq!(r.u16_lanes()[7], 0x0F0E);
        assert_eq!(r.u32_lanes()[0], 0x0302_0100);
        assert_eq!(r.u32_lanes()[3], 0x0F0E_0D0C);
        assert_eq!(r.u64_lanes()[0], 0x0706_0504_0302_0100);
        assert_eq!(r.u64_lanes()[1], 0x0F0E_0D0C_0B0A_0908);
    }
}
