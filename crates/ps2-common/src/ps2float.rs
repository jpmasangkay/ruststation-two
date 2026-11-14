//! Non-IEEE 754 PlayStation 2 Floating-Point Engine (`ps2float`).
//!
//! Primary source references:
//! - `ps2tek` ("Emotion Engine -> EE FPU (COP1)"):
//!   * Exponent `0xFF` (255) is a valid finite normalized exponent (no `Infinity` or `NaN`).
//!     Values range up to `±(2 - 2^-23) * 2^128` (`0x7FFF_FFFF` / `0xFFFF_FFFF`).
//!   * Overflow beyond exponent `0xFF` saturates to `0x7FFF_FFFF` (positive) or `0xFFFF_FFFF`
//!     (negative) and sets `O` (Overflow, bit 15) and `SO` (Sticky Overflow, bit 4) in `FCR31`.
//!   * Exponent `0x00` (denormal) is flushed to signed zero (`±0.0`) on arithmetic inputs and
//!     outputs. Underflow sets `U` (Underflow, bit 14) and `SU` (Sticky Underflow, bit 3).
//!   * Rounding is unconditionally toward zero (truncation).
//! - `Play!` (`Source/ee/FpAddTruncate.cpp`, Copyright (c) Jean-Philip Desjardins, BSD-2-Clause)
//!   & `ps2autotests` (`tests/cpu/ee/fpu`, ISC):
//!   * Mantissa alignment during addition/subtraction uses a single guard bit with no sticky bit,
//!     producing the hardware 1-ULP truncation behavior when subtracting a tiny operand (`diff >= 25`).
//!   * `MIN.S` and `MAX.S` compare operands as signed-magnitude 32-bit integers (`+0.0 > -0.0`)
//!     without trapping on exponent `0xFF` and without modifying `FCR31` cause flags.

use bitflags::bitflags;

bitflags! {
    /// COP1 Control/Status Register (`FCR31`) condition, cause, and sticky flags.
    ///
    /// Reference: `ps2tek` ("EE FPU (COP1) -> Control Registers").
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    pub struct Fcr31Flags: u32 {
        /// Sticky Underflow flag (bit 3).
        const SU = 1 << 3;
        /// Sticky Overflow flag (bit 4).
        const SO = 1 << 4;
        /// Sticky Divide-by-Zero flag (bit 5).
        const SD = 1 << 5;
        /// Sticky Invalid Operation flag (bit 6).
        const SI = 1 << 6;

        /// Cause: Underflow flag (bit 14).
        const U  = 1 << 14;
        /// Cause: Overflow flag (bit 15).
        const O  = 1 << 15;
        /// Cause: Divide-by-Zero flag (bit 16).
        const D  = 1 << 16;
        /// Cause: Invalid Operation flag (bit 17).
        const I  = 1 << 17;

        /// Condition bit set by `C.F.S` / `C.EQ.S` / `C.LT.S` / `C.LE.S` (bit 23).
        const C  = 1 << 23;

        /// Mask of all per-instruction Cause flags (`U | O | D | I`).
        const CAUSE_MASK  = Self::U.bits() | Self::O.bits() | Self::D.bits() | Self::I.bits();
        /// Mask of all Sticky flags (`SU | SO | SD | SI`).
        const STICKY_MASK = Self::SU.bits() | Self::SO.bits() | Self::SD.bits() | Self::SI.bits();
    }
}

impl Fcr31Flags {
    /// Merge an arithmetic instruction's flags into an existing `FCR31` register value:
    /// clears previous cause bits (`I`, `D`, `O`, `U`), sets the new cause bits, and ORs
    /// the new sticky bits (`SI`, `SD`, `SO`, `SU`).
    #[inline(always)]
    pub const fn apply_to_fcr31(self, fcr31: u32) -> u32 {
        (fcr31 & !Self::CAUSE_MASK.bits()) | self.bits()
    }
}

/// Result of a PS2 floating-point arithmetic operation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Ps2FloatResult {
    /// Resulting 32-bit PS2 floating-point bit pattern.
    pub bits: u32,
    /// Cause and sticky flags produced by the operation.
    pub flags: Fcr31Flags,
}

impl Ps2FloatResult {
    #[inline(always)]
    pub const fn new(bits: u32, flags: Fcr31Flags) -> Self {
        Self { bits, flags }
    }
}

/// Clamps an arbitrary 32-bit float bit pattern into a safe **host IEEE-754** finite float.
///
/// Converts exponent `0xFF` (`NaN` / `±Infinity` on IEEE-754 hosts) into the largest finite
/// IEEE-754 float (`0x7F7F_FFFF` / `0xFF7F_FFFF`) and flushes denormals (exponent `0x00`) to
/// signed zero (`±0.0`).
///
/// Note: This helper is used when feeding values to host IEEE-754 hardware instructions.
/// The software `ps2float` functions below support full PS2 exponent `0xFF` natively up to
/// `0x7FFF_FFFF` / `0xFFFF_FFFF` (`ps2tek`, `ps2autotests/tests/cpu/ee/fpu`).
#[inline(always)]
pub const fn ps2_clamp_f32_bits(bits: u32) -> u32 {
    let exp = (bits >> 23) & 0xFF;
    if exp == 0xFF {
        (bits & 0x8000_0000) | 0x7F7F_FFFF
    } else if exp == 0x00 {
        bits & 0x8000_0000
    } else {
        bits
    }
}

/// Flushes a PS2 denormal (exponent `0x00`) to signed zero while preserving exponent `0xFF`.
#[inline(always)]
pub const fn ps2_flush_denormal(bits: u32) -> u32 {
    if ((bits >> 23) & 0xFF) == 0 {
        bits & 0x8000_0000
    } else {
        bits
    }
}

/// Returns `true` if the bit pattern is `±0.0` or a denormal (exponent `0x00`).
#[inline(always)]
pub const fn is_zero_or_denormal(bits: u32) -> bool {
    ((bits >> 23) & 0xFF) == 0
}

/// PS2 `ADD.S`: bit-exact non-IEEE 754 single-precision addition with 1-guard-bit truncation.
///
/// Reference: `ps2tek` ("EE FPU -> ADD.S"), `Play!` (`FpAddTruncate.cpp`), `ps2autotests`.
pub fn add(a: u32, b: u32) -> Ps2FloatResult {
    let a = ps2_flush_denormal(a);
    let b = ps2_flush_denormal(b);

    let s1 = a >> 31;
    let e1 = ((a >> 23) & 0xFF) as i32;
    let m1 = a & 0x7F_FFFF;

    let s2 = b >> 31;
    let e2 = ((b >> 23) & 0xFF) as i32;
    let m2 = b & 0x7F_FFFF;

    // Handle zero operands.
    if e1 == 0 && e2 == 0 {
        // -0.0 + -0.0 = -0.0; all other zero combinations yield +0.0.
        let sign = (s1 & s2) << 31;
        return Ps2FloatResult::new(sign, Fcr31Flags::empty());
    }
    if e1 == 0 {
        return Ps2FloatResult::new(b, Fcr31Flags::empty());
    }
    if e2 == 0 {
        return Ps2FloatResult::new(a, Fcr31Flags::empty());
    }

    // Order operands so (e_hi, m_hi, s_hi) has the larger or equal magnitude.
    let (s_hi, e_hi, m_hi, s_lo, e_lo, m_lo) = if (e1 > e2) || (e1 == e2 && m1 >= m2) {
        (s1, e1, m1, s2, e2, m2)
    } else {
        (s2, e2, m2, s1, e1, m1)
    };

    // 25-bit significands (hidden bit at bit 24, 1 guard bit at bit 0, no sticky bit).
    let sig_hi = (0x80_0000 | m_hi) << 1;
    let sig_lo = (0x80_0000 | m_lo) << 1;
    let exp_diff = (e_hi - e_lo) as u32;
    let sig_lo_aligned = if exp_diff >= 25 {
        0
    } else {
        sig_lo >> exp_diff
    };

    if s_hi == s_lo {
        // Same-sign addition.
        let sum = sig_hi + sig_lo_aligned;
        let carry = (sum >> 25) & 1;
        let res_exp = e_hi + (carry as i32);
        if res_exp > 255 {
            return Ps2FloatResult::new((s_hi << 31) | 0x7FFF_FFFF, Fcr31Flags::O | Fcr31Flags::SO);
        }
        let res_mant = (sum >> (1 + carry)) & 0x7F_FFFF;
        Ps2FloatResult::new(
            (s_hi << 31) | ((res_exp as u32) << 23) | res_mant,
            Fcr31Flags::empty(),
        )
    } else {
        // Opposite-sign subtraction.
        let diff = sig_hi - sig_lo_aligned;
        if diff == 0 {
            // Exact cancellation always yields +0.0 on R5900 COP1.
            return Ps2FloatResult::new(0x0000_0000, Fcr31Flags::empty());
        }
        // Normalize so the leading 1 sits at bit 24 (which has 7 leading zeros in a u32).
        let lz = diff.leading_zeros().saturating_sub(7);
        let res_exp = e_hi - (lz as i32);
        if res_exp <= 0 {
            return Ps2FloatResult::new(s_hi << 31, Fcr31Flags::U | Fcr31Flags::SU);
        }
        let norm = diff << lz;
        let res_mant = (norm >> 1) & 0x7F_FFFF;
        Ps2FloatResult::new(
            (s_hi << 31) | ((res_exp as u32) << 23) | res_mant,
            Fcr31Flags::empty(),
        )
    }
}

/// PS2 `SUB.S`: bit-exact non-IEEE 754 single-precision subtraction (`a - b`).
#[inline]
pub fn sub(a: u32, b: u32) -> Ps2FloatResult {
    add(a, b ^ 0x8000_0000)
}

/// PS2 `MUL.S`: bit-exact non-IEEE 754 single-precision multiplication with truncation.
///
/// Reference: `ps2tek` ("EE FPU -> MUL.S"), `ps2autotests` (`tests/cpu/ee/fpu`).
pub fn mul(a: u32, b: u32) -> Ps2FloatResult {
    let res_sign = (a ^ b) & 0x8000_0000;
    let e1 = ((a >> 23) & 0xFF) as i32;
    let e2 = ((b >> 23) & 0xFF) as i32;

    if e1 == 0 || e2 == 0 {
        return Ps2FloatResult::new(res_sign, Fcr31Flags::empty());
    }

    let sig1 = 0x80_0000u64 | u64::from(a & 0x7F_FFFF);
    let sig2 = 0x80_0000u64 | u64::from(b & 0x7F_FFFF);

    // 24-bit * 24-bit = 48-bit product. Leading 1 is at bit 47 (carry = 1) or bit 46 (carry = 0).
    // On R5900 hardware (`ps2autotests` `tests/cpu/ee_fpu/muldiv.expected` lines 87, 95, 96, 311,
    // 316, 317), the Radix-4 Booth multiplier omits the +1 two's-complement hot-one of the lowest
    // Booth group when `sig2[1:0]` encodes a negative Booth digit (`(sig2 & 3) >= 2`).
    let prod = (sig1 * sig2).wrapping_sub(u64::from((sig2 & 3) >= 2));
    let carry = ((prod >> 47) & 1) as i32;
    let res_exp = e1 + e2 - 127 + carry;

    if res_exp > 255 {
        return Ps2FloatResult::new(res_sign | 0x7FFF_FFFF, Fcr31Flags::O | Fcr31Flags::SO);
    }
    if res_exp <= 0 {
        return Ps2FloatResult::new(res_sign, Fcr31Flags::U | Fcr31Flags::SU);
    }

    let res_mant = ((prod >> (23 + carry)) & 0x7F_FFFF) as u32;
    Ps2FloatResult::new(
        res_sign | ((res_exp as u32) << 23) | res_mant,
        Fcr31Flags::empty(),
    )
}

/// PS2 `MADD.S` / `MADDA.S`: multiply-accumulate (`acc + (s * t)`).
///
/// Reference: `ps2tek` ("EE FPU -> MADD.S") & `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected`
/// lines 57, 60, 63, 118, 121, 124):
/// - If `s * t` overflows exponent `0xFF`, the MAC operation immediately saturates to `prod.bits`
///   (`±0x7FFF_FFFF` with the sign of `s * t`) and sets `O | SO`.
/// - Otherwise, adds the truncated product `prod.bits` to `acc`.
pub fn madd(acc: u32, s: u32, t: u32) -> Ps2FloatResult {
    let prod = mul(s, t);
    if prod.flags.contains(Fcr31Flags::O) {
        return prod;
    }
    let sum = add(acc, prod.bits);
    let sticky = (prod.flags | sum.flags) & Fcr31Flags::STICKY_MASK;
    let cause = sum.flags & Fcr31Flags::CAUSE_MASK;
    Ps2FloatResult::new(sum.bits, cause | sticky)
}

/// PS2 `MSUB.S` / `MSUBA.S`: multiply-subtract (`acc - (s * t)`).
///
/// Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 178, 181,
/// 191, 193, 204, 239, 242, 252, 254, 265):
/// - If `s * t` overflows exponent `0xFF`, the MAC operation immediately saturates to
///   `prod.bits ^ 0x8000_0000` (`±0x7FFF_FFFF` with the negated sign of `s * t`) and sets `O | SO`.
/// - Otherwise, subtracts the truncated product `prod.bits` from `acc`.
pub fn msub(acc: u32, s: u32, t: u32) -> Ps2FloatResult {
    let prod = mul(s, t);
    if prod.flags.contains(Fcr31Flags::O) {
        return Ps2FloatResult::new(prod.bits ^ 0x8000_0000, prod.flags);
    }
    let diff = sub(acc, prod.bits);
    let sticky = (prod.flags | diff.flags) & Fcr31Flags::STICKY_MASK;
    let cause = diff.flags & Fcr31Flags::CAUSE_MASK;
    Ps2FloatResult::new(diff.bits, cause | sticky)
}

/// PS2 `DIV.S`: single-precision division (`a / b`) with PS2 divide-by-zero / invalid saturation.
///
/// Reference: `ps2tek` ("EE FPU -> DIV.S & Exceptions") & `ps2autotests` (`tests/cpu/ee_fpu/muldiv.expected` line 29):
/// - `0 / 0` sets `I | SI` and saturates to `(sign << 31) | 0x7FFF_FFFF`.
/// - `nonzero / 0` sets `D | SD` and saturates to `(sign << 31) | 0x7FFF_FFFF`.
/// - Quotient significand is rounded with 1 guard bit (`((sig1 << (24 + borrow)) / sig2 + 1) >> 1`).
pub fn div(a: u32, b: u32) -> Ps2FloatResult {
    let res_sign = (a ^ b) & 0x8000_0000;
    let e1 = ((a >> 23) & 0xFF) as i32;
    let e2 = ((b >> 23) & 0xFF) as i32;

    if e2 == 0 {
        let flags = if e1 == 0 {
            Fcr31Flags::I | Fcr31Flags::SI
        } else {
            Fcr31Flags::D | Fcr31Flags::SD
        };
        return Ps2FloatResult::new(res_sign | 0x7FFF_FFFF, flags);
    }

    if e1 == 0 {
        return Ps2FloatResult::new(res_sign, Fcr31Flags::empty());
    }

    let sig1 = 0x80_0000u64 | u64::from(a & 0x7F_FFFF);
    let sig2 = 0x80_0000u64 | u64::from(b & 0x7F_FFFF);

    let borrow = i32::from(sig1 < sig2);
    let mut res_exp = e1 - e2 + 127 - borrow;

    let quot_x2 = (sig1 << (24 + borrow)) / sig2;
    let mut quot = ((quot_x2 + 1) >> 1) as u32;
    if quot >= 0x100_0000 {
        quot >>= 1;
        res_exp += 1;
    }

    if res_exp > 255 {
        // Reference: `ps2tek` ("EE FPU -> DIV.S"): FDIV instructions only update `I`/`D` flags,
        // not `O`/`U` (`ps2autotests` `tests/cpu/ee_fpu/fcr.expected` line 48).
        return Ps2FloatResult::new(res_sign | 0x7FFF_FFFF, Fcr31Flags::empty());
    }
    if res_exp <= 0 {
        return Ps2FloatResult::new(res_sign, Fcr31Flags::empty());
    }

    let res_mant = quot & 0x7F_FFFF;
    Ps2FloatResult::new(
        res_sign | ((res_exp as u32) << 23) | res_mant,
        Fcr31Flags::empty(),
    )
}

/// Helper: integer square root (`floor(sqrt(n))`) for `u128`.
#[inline]
const fn isqrt_u128(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let shift = (127 - n.leading_zeros()) & !1;
    let mut bit = 1u128 << shift;
    let mut res = 0u128;
    let mut rem = n;
    while bit != 0 {
        if rem >= res + bit {
            rem -= res + bit;
            res = (res >> 1) + bit;
        } else {
            res >>= 1;
        }
        bit >>= 2;
    }
    res
}

/// PS2 `SQRT.S`: single-precision square root (`sqrt(|t|)`).
///
/// Reference: `ps2tek` ("EE FPU -> SQRT.S") & `ps2autotests` (`tests/cpu/ee_fpu/sqrt.expected` lines 44..65):
/// - If `t` has exponent 0, returns `+0.0` (`0x0000_0000`) and sets `I | SI` if `sign == 1`.
/// - If `t < 0` (`sign == 1` and `exp > 0`), sets `I | SI` and returns `+sqrt(|t|)`.
/// - Significand is computed with 1 guard bit (`(isqrt_u128(sig << 2) + 1) >> 1`).
pub fn sqrt(t: u32) -> Ps2FloatResult {
    let sign = t >> 31;
    let exp = ((t >> 23) & 0xFF) as i32;
    let mant = t & 0x7F_FFFF;

    if exp == 0 {
        let flags = if sign != 0 {
            Fcr31Flags::I | Fcr31Flags::SI
        } else {
            Fcr31Flags::empty()
        };
        return Ps2FloatResult::new(0, flags);
    }

    let flags = if sign != 0 {
        Fcr31Flags::I | Fcr31Flags::SI
    } else {
        Fcr31Flags::empty()
    };

    // Unbiased exponent: e_unbiased = exp - 127.
    let e_unbiased = exp - 127;
    // Scale significand so we take integer sqrt with 24 fractional bits (23 mantissa + 1 guard bit):
    let odd = (e_unbiased & 1) != 0;
    let mut res_exp = ((e_unbiased - i32::from(odd)) / 2) + 127;

    let sig = u128::from(0x80_0000u32 | mant) << (25 + u32::from(odd));
    let root25 = isqrt_u128(sig);
    let mut root24 = ((root25 + 1) >> 1) as u32;
    if root24 >= 0x100_0000 {
        root24 >>= 1;
        res_exp += 1;
    }
    let res_mant = root24 & 0x7F_FFFF;

    Ps2FloatResult::new(((res_exp as u32) << 23) | res_mant, flags)
}

/// PS2 `RSQRT.S`: single-precision reciprocal square root (`s / sqrt(|t|)`).
///
/// Reference: `ps2tek` ("EE FPU -> RSQRT.S") & `ps2autotests` (`tests/cpu/ee_fpu/sqrt.expected` lines 2..41):
/// - Result sign is always `s & 0x8000_0000` (sign of `s`, ignoring sign of `t`).
/// - Hardware computes the 24-bit `sqrt(|t|)` significand, takes its 1-guard-bit rounded reciprocal,
///   and multiplies by `s`.
pub fn rsqrt(s: u32, t: u32) -> Ps2FloatResult {
    let res_sign = s & 0x8000_0000;
    let exp_s = ((s >> 23) & 0xFF) as i32;
    let exp_t = ((t >> 23) & 0xFF) as i32;
    let flags = if (t >> 31) != 0 {
        Fcr31Flags::I | Fcr31Flags::SI
    } else {
        Fcr31Flags::empty()
    };

    if exp_t == 0 {
        return Ps2FloatResult::new(
            res_sign | 0x7FFF_FFFF,
            flags | Fcr31Flags::D | Fcr31Flags::SD,
        );
    }

    if exp_s == 0 {
        return Ps2FloatResult::new(res_sign, flags);
    }

    let mant_t = t & 0x7F_FFFF;
    let e_t = exp_t - 127;
    let odd = (e_t & 1) != 0;

    let base_inv_exp = -((e_t - i32::from(odd)) / 2) - 1;
    let sig_t = u128::from(0x80_0000u32 | mant_t) << (25 + u32::from(odd));
    let root25 = isqrt_u128(sig_t);
    let root24 = (root25 + 1) >> 1;
    let inv_sig_x2 = (1u128 << 48) / root24;
    let rounded = ((inv_sig_x2 + 1) >> 1) as u64;
    let (inv_exp, inv_sig) = if rounded >= 0x100_0000 {
        (base_inv_exp + 1, rounded >> 1)
    } else {
        (base_inv_exp, rounded)
    };

    let sig_s = 0x80_0000u64 | u64::from(s & 0x7F_FFFF);
    let prod = sig_s * inv_sig;
    let carry = ((prod >> 47) & 1) as i32;
    let res_exp = exp_s + inv_exp + carry;

    if res_exp > 255 {
        return Ps2FloatResult::new(res_sign | 0x7FFF_FFFF, flags);
    }
    if res_exp <= 0 {
        return Ps2FloatResult::new(res_sign, flags);
    }

    let res_mant = ((prod >> (23 + carry)) & 0x7F_FFFF) as u32;
    Ps2FloatResult::new(res_sign | ((res_exp as u32) << 23) | res_mant, flags)
}

/// PS2 `ABS.S`: clears sign bit (Note: on R5900 `ABS.S` does not flush denormals and clears flags).
#[inline(always)]
pub const fn abs(s: u32) -> u32 {
    s & 0x7FFF_FFFF
}

/// PS2 `NEG.S`: flips sign bit.
#[inline(always)]
pub const fn neg(s: u32) -> u32 {
    s ^ 0x8000_0000
}

/// Helper: converts a 32-bit sign-magnitude PS2 float bit pattern into a totally ordered `i64`
/// where `-max < ... < -0.0 < +0.0 < ... < +max`.
///
/// Reference: `ps2tek` ("EE FPU -> MIN.S / MAX.S") & Blueprint §11 (DobieStation Issue #51):
/// `MIN.S` and `MAX.S` compare all 32 bits as signed-magnitude integers, ordering `+0.0 > -0.0`
/// and ordering exponent `0xFF` values normally without `NaN` propagation.
#[inline(always)]
const fn signed_magnitude_order(bits: u32) -> i64 {
    let mag = (bits & 0x7FFF_FFFF) as i64;
    if (bits & 0x8000_0000) != 0 {
        -mag - 1
    } else {
        mag
    }
}

/// PS2 `MIN.S`: signed-magnitude minimum of two 32-bit PS2 floats.
#[inline(always)]
pub const fn min(a: u32, b: u32) -> u32 {
    if signed_magnitude_order(a) <= signed_magnitude_order(b) {
        a
    } else {
        b
    }
}

/// PS2 `MAX.S`: signed-magnitude maximum of two 32-bit PS2 floats.
#[inline(always)]
pub const fn max(a: u32, b: u32) -> u32 {
    if signed_magnitude_order(a) >= signed_magnitude_order(b) {
        a
    } else {
        b
    }
}

/// PS2 `CVT.W.S`: converts a 32-bit PS2 float to a 32-bit signed integer (`i32`), truncating
/// toward zero and clamping out-of-range values to `i32::MAX` (`0x7FFF_FFFF`) or `i32::MIN`
/// (`-2147483648`).
///
/// Reference: `ps2tek` ("EE FPU -> CVT.W.S").
pub const fn cvt_w_s(s: u32) -> i32 {
    let sign = s >> 31;
    let exp = ((s >> 23) & 0xFF) as i32;
    let mant = s & 0x7F_FFFF;

    if exp < 127 {
        return 0;
    }
    let shift = exp - 127;
    if shift >= 31 {
        return if sign != 0 { i32::MIN } else { i32::MAX };
    }

    let sig = 0x80_0000u64 | (mant as u64);
    let abs_val = if shift >= 23 {
        (sig << (shift - 23)) as i32
    } else {
        (sig >> (23 - shift)) as i32
    };

    if sign != 0 {
        -abs_val
    } else {
        abs_val
    }
}

/// PS2 `CVT.S.W`: converts a 32-bit signed integer (`i32`) to a 32-bit PS2 float, truncating
/// toward zero when the integer has more than 24 significant bits.
///
/// Reference: `ps2tek` ("EE FPU -> CVT.S.W").
pub const fn cvt_s_w(w: i32) -> u32 {
    if w == 0 {
        return 0;
    }
    let sign = if w < 0 { 0x8000_0000u32 } else { 0 };
    let mag = w.unsigned_abs();
    let msb = 31 - mag.leading_zeros(); // 0..=31
    let exp = (127 + msb) << 23;
    let mant = if msb <= 23 {
        (mag << (23 - msb)) & 0x7F_FFFF
    } else {
        (mag >> (msb - 23)) & 0x7F_FFFF
    };
    sign | exp | mant
}

/// PS2 `C.EQ.S`: compares two PS2 floats for equality (`+0.0 == -0.0`, denormals flushed to `0`).
#[inline(always)]
pub const fn c_eq(a: u32, b: u32) -> bool {
    let a = ps2_flush_denormal(a);
    let b = ps2_flush_denormal(b);
    if ((a | b) & 0x7FFF_FFFF) == 0 {
        return true;
    }
    a == b
}

/// PS2 `C.LT.S`: compares `a < b` (`+0.0 == -0.0`, denormals flushed to `0`).
#[inline(always)]
pub const fn c_lt(a: u32, b: u32) -> bool {
    let a = ps2_flush_denormal(a);
    let b = ps2_flush_denormal(b);
    if ((a | b) & 0x7FFF_FFFF) == 0 {
        return false;
    }
    signed_magnitude_order(a) < signed_magnitude_order(b)
}

/// PS2 `C.LE.S`: compares `a <= b` (`+0.0 == -0.0`, denormals flushed to `0`).
#[inline(always)]
pub const fn c_le(a: u32, b: u32) -> bool {
    c_eq(a, b) || c_lt(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_f32_bits_handles_exponent_ff_and_denormals() {
        // Positive & negative exponent 0xFF clamp to max finite IEEE-754 float.
        assert_eq!(ps2_clamp_f32_bits(0x7F80_0000), 0x7F7F_FFFF);
        assert_eq!(ps2_clamp_f32_bits(0x7FFF_FFFF), 0x7F7F_FFFF);
        assert_eq!(ps2_clamp_f32_bits(0xFF80_0000), 0xFF7F_FFFF);
        assert_eq!(ps2_clamp_f32_bits(0xFFFF_FFFF), 0xFF7F_FFFF);

        // Denormals flush to signed zero.
        assert_eq!(ps2_clamp_f32_bits(0x0000_0001), 0x0000_0000);
        assert_eq!(ps2_clamp_f32_bits(0x807F_FFFF), 0x8000_0000);

        // Normal floats pass through unchanged.
        assert_eq!(ps2_clamp_f32_bits(0x3F80_0000), 0x3F80_0000);
    }

    #[test]
    fn ps2float_exponent_ff_arithmetic_and_overflow_saturation() {
        // 0x7F80_0000 is 1.0 * 2^128 on PS2 (not Infinity!).
        // Adding 1.0 * 2^128 + 0.5 * 2^128 (0x7F00_0000) yields 1.5 * 2^128 (0x7FC0_0000) with no overflow.
        let r = add(0x7F80_0000, 0x7F00_0000);
        assert_eq!(r.bits, 0x7FC0_0000);
        assert!(r.flags.is_empty());

        // Adding 1.0 * 2^128 + 1.0 * 2^128 overflows exponent 0xFF and saturates to 0x7FFF_FFFF.
        let ov = add(0x7F80_0000, 0x7F80_0000);
        assert_eq!(ov.bits, 0x7FFF_FFFF);
        assert_eq!(ov.flags, Fcr31Flags::O | Fcr31Flags::SO);

        // Negative overflow saturates to 0xFFFF_FFFF.
        let neg_ov = add(0xFF80_0000, 0xFF80_0000);
        assert_eq!(neg_ov.bits, 0xFFFF_FFFF);
        assert_eq!(neg_ov.flags, Fcr31Flags::O | Fcr31Flags::SO);
    }

    #[test]
    fn ps2float_sub_1_ulp_no_sticky_bit_behavior() {
        // Reference: Play! Source/ee/FpAddTruncate.cpp & ps2autotests.
        // In IEEE-754, 1.0f (0x3F80_0000) - 2^-26 (0x3280_0000, exp_diff = 26)
        // uses a sticky bit and produces 0x3F7F_FFFF.
        // On PS2 COP1, there is only 1 guard bit and no sticky bit, so exp_diff >= 25 shifts
        // the subtrahend out completely and returns 1.0f (0x3F80_0000) unchanged!
        let r = sub(0x3F80_0000, 0x3280_0000);
        assert_eq!(r.bits, 0x3F80_0000);

        // Whereas exp_diff == 24 (1.0f - 2^-24 = 0x3380_0000) hits the guard bit and decrements to 0x3F7F_FFFF.
        let r2 = sub(0x3F80_0000, 0x3380_0000);
        assert_eq!(r2, Ps2FloatResult::new(0x3F7F_FFFF, Fcr31Flags::empty()));
    }

    #[test]
    fn ps2float_mul_truncation_and_underflow() {
        // (1 + 2^-23) * (1 + 2^-23) = 1 + 2^-22 + 2^-46 -> truncates 2^-46 to 1 + 2^-22 (0x3F80_0002).
        let r = mul(0x3F80_0001, 0x3F80_0001);
        assert_eq!(r.bits, 0x3F80_0002);
        assert!(r.flags.is_empty());

        // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 308..317):
        // R5900 Radix-4 Booth multiplier omits the +1 hot-one of the lowest Booth group when `t`
        // significand bits `[1:0]` >= 2: `1.0 * 0x3FFF_FFFF = 0x3FFF_FFFE`, whereas
        // `0x3FFF_FFFF * 1.0 = 0x3FFF_FFFF`.
        assert_eq!(mul(0x3F80_0000, 0x3FFF_FFFF).bits, 0x3FFF_FFFE);
        assert_eq!(mul(0x3FFF_FFFF, 0x3F80_0000).bits, 0x3FFF_FFFF);

        // Minimum normal 2^-126 (0x0080_0000) * 0.5 (0x3F00_0000) underflows to +0.0 and sets U | SU.
        let uf = mul(0x0080_0000, 0x3F00_0000);
        assert_eq!(uf.bits, 0x0000_0000);
        assert_eq!(uf.flags, Fcr31Flags::U | Fcr31Flags::SU);
    }

    #[test]
    fn ps2float_madd_overflow_and_two_stage_accumulation() {
        // Reference: `ps2autotests` (`local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 57, 178, 191):
        // When `s * t` overflows exponent `0xFF`, `MADD.S` / `MSUB.S` immediately saturates the
        // entire MAC result to `±0x7FFF_FFFF` and sets `O | SO`.
        let r_ov = madd(0xFFFF_FFFF, 0x7FFF_FFFF, 0x7FFF_FFFF);
        assert_eq!(r_ov.bits, 0x7FFF_FFFF);
        assert_eq!(r_ov.flags, Fcr31Flags::O | Fcr31Flags::SO);

        let r_msub_ov = msub(0x7F80_0000, 0x7F80_0000, 0x7F80_0000);
        assert_eq!(r_msub_ov.bits, 0xFFFF_FFFF);
        assert_eq!(r_msub_ov.flags, Fcr31Flags::O | Fcr31Flags::SO);

        // When `s * t` underflows (`U | SU`), the sticky `SU` flag is preserved while `acc` is returned.
        let r_uf = madd(0x3F80_0000, 0x0080_0000, 0x3F00_0000);
        assert_eq!(r_uf.bits, 0x3F80_0000);
        assert!(r_uf.flags.contains(Fcr31Flags::SU));
        assert!(!r_uf.flags.contains(Fcr31Flags::U));
    }

    #[test]
    fn ps2float_div_and_sqrt_exceptions() {
        // 1.0 / 0.0 -> 0x7FFF_FFFF with D | SD.
        let d = div(0x3F80_0000, 0x0000_0000);
        assert_eq!(d.bits, 0x7FFF_FFFF);
        assert_eq!(d.flags, Fcr31Flags::D | Fcr31Flags::SD);

        // 0.0 / 0.0 -> 0x7FFF_FFFF with I | SI.
        let inv = div(0x0000_0000, 0x0000_0000);
        assert_eq!(inv.bits, 0x7FFF_FFFF);
        assert_eq!(inv.flags, Fcr31Flags::I | Fcr31Flags::SI);

        // sqrt(-4.0) -> +2.0 (0x4000_0000) with I | SI.
        let sq = sqrt(0xC080_0000);
        assert_eq!(sq.bits, 0x4000_0000);
        assert_eq!(sq.flags, Fcr31Flags::I | Fcr31Flags::SI);

        // sqrt(-0.0) and sqrt(-denormal) -> +0.0 (0x0000_0000) with I | SI (`sqrt.expected` lines 45, 55).
        let sq_neg_zero = sqrt(0x8000_0000);
        assert_eq!(sq_neg_zero.bits, 0x0000_0000);
        assert_eq!(sq_neg_zero.flags, Fcr31Flags::I | Fcr31Flags::SI);

        // rsqrt(1.5, 1.5) = 1.5 / sqrt(1.5) = sqrt(1.5) -> 0x3F9C_C470.
        let rsq = rsqrt(0x3FC0_0000, 0x3FC0_0000);
        assert_eq!(rsq.bits, 0x3F9C_C470);
        assert!(rsq.flags.is_empty());
    }

    #[test]
    fn ps2float_min_max_signed_magnitude_ordering() {
        // +0.0 > -0.0
        assert_eq!(max(0x0000_0000, 0x8000_0000), 0x0000_0000);
        assert_eq!(min(0x0000_0000, 0x8000_0000), 0x8000_0000);

        // Exponent 0xFF values compare as finite magnitudes, not NaN.
        assert_eq!(max(0x7F80_0000, 0x7FFF_FFFF), 0x7FFF_FFFF);
        assert_eq!(min(0xFF80_0000, 0xFFFF_FFFF), 0xFFFF_FFFF);
    }

    #[test]
    fn ps2float_cvt_w_s_and_cvt_s_w() {
        assert_eq!(cvt_w_s(0x3FC0_0000), 1); // 1.5 -> 1
        assert_eq!(cvt_w_s(0xBFC0_0000), -1); // -1.5 -> -1
        assert_eq!(cvt_w_s(0x7F80_0000), i32::MAX);
        assert_eq!(cvt_w_s(0xFF80_0000), i32::MIN);

        // 0x0100_0001 (16,777,217) truncates lowest bit when converted to 24-bit float significand (16,777,216 = 2^24).
        assert_eq!(cvt_s_w(0x0100_0001), 0x4B80_0000);
        assert_eq!(cvt_s_w(-1), 0xBF80_0000);
    }
}
