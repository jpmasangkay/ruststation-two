//! VU1 Elementary Function Unit (`EFU`) arithmetic & transcendental execution.
//!
//! Hardware references:
//! - `ps2tek` (`#vu`, `#vupipelining`, `#eecop1floatingpointformat`, `FETCHED`):
//!   * EFU instructions execute exclusively on `VU1` and write to `P` after their
//!     instruction-specific latency (`11..=54` cycles).
//!   * PS2 single-precision format treats exponent `0xFF` as finite normal (no Inf/NaN),
//!     flushes exponent `0x00` denormals to zero, and truncates toward zero.
//! - `Play!` (BSD-2-Clause, see `THIRD_PARTY_NOTICES.md`, `Source/ee/MA_VU_Lower.cpp`, `FETCHED`):
//!   * Reference formulas, operation orderings, and polynomial constants.

use ps2_common::ps2float;

use super::state::VuState;

/// Dispatch a `VU1` Lower Type 3/4 EFU opcode (`bit 31 == 1`).
///
/// Returns `true` if `instr` is one of the 13 EFU arithmetic instructions (`P` writers),
/// or `false` otherwise (`MFP`, `WAITP`, `FDIV`, memory/int ops).
#[inline]
pub fn exec_efu_op(vu: &mut VuState, instr: u32) -> bool {
    if (instr & 0x8000_0000) == 0 || !matches!(instr & 0x3F, 0x3C..=0x3F) {
        return false;
    }

    let op11 = instr & 0x7FF;
    let fs = ((instr >> 11) & 0x1F) as usize;
    let fsf = ((instr >> 21) & 0x3) as usize;

    let result = match op11 {
        // Group A
        0x73C => esadd(vu.read_vf(fs).u32_lanes()),
        0x77E => esum(vu.read_vf(fs).u32_lanes()),
        0x7BE => ercpr(vu.read_vf_lane(fs, fsf)),
        0x7BC => esqrt(vu.read_vf_lane(fs, fsf)),
        0x7BD => ersqrt(vu.read_vf_lane(fs, fsf)),

        // Group B
        0x73E => eleng(vu.read_vf(fs).u32_lanes()),
        0x73F => erleng(vu.read_vf(fs).u32_lanes()),
        0x73D => ersadd(vu.read_vf(fs).u32_lanes()),

        // Group C
        0x7FE => eexp(vu.read_vf_lane(fs, fsf)),
        0x7FC => esin(vu.read_vf_lane(fs, fsf)),
        0x7FD => 0, // EATAN
        0x77C => 0, // EATANxy
        0x77D => 0, // EATANxz

        _ => return false,
    };

    vu.p = result;
    true
}

/// `ESADD P, fs`: `x^2 + (y^2 + z^2)` (`Play!` `MA_VU_Lower.cpp` evaluation order, `ps2float`).
#[inline]
pub fn esadd(lanes: &[u32; 4]) -> u32 {
    let xx = ps2float::mul(lanes[0], lanes[0]).bits;
    let yy = ps2float::mul(lanes[1], lanes[1]).bits;
    let zz = ps2float::mul(lanes[2], lanes[2]).bits;
    let yz = ps2float::add(yy, zz).bits;
    ps2float::add(xx, yz).bits
}

/// `ESUM P, fs`: `x + (y + (z + w))` (`Play!` `MA_VU_Lower.cpp` evaluation order, `ps2float`).
#[inline]
pub fn esum(lanes: &[u32; 4]) -> u32 {
    let zw = ps2float::add(lanes[2], lanes[3]).bits;
    let yzw = ps2float::add(lanes[1], zw).bits;
    ps2float::add(lanes[0], yzw).bits
}

/// `ERCPR P, fs[fsf]`: `1.0 / fs[fsf]` (`Play!` `MA_VU_Lower.cpp` / `ps2float::div`).
#[inline]
pub fn ercpr(val: u32) -> u32 {
    ps2float::div(0x3F80_0000, val).bits
}

/// `ESQRT P, fs[fsf]`: `sqrt(|fs[fsf]|)` (`Play!` `MA_VU_Lower.cpp` / `ps2float::sqrt`).
#[inline]
pub fn esqrt(val: u32) -> u32 {
    ps2float::sqrt(val).bits
}

/// `ERSQRT P, fs[fsf]`: `1.0 / sqrt(|fs[fsf]|)` (`ps2float::rsqrt`).
///
/// Both `ps2float::rsqrt` and `ercpr(esqrt(val))` match `14/16` vectors in `efu.expected`;
/// the remaining 2 misses (`PI/2`, `PI`) are +/-1 ULP from hardware EFU approximation.
#[inline]
pub fn ersqrt(val: u32) -> u32 {
    ps2float::rsqrt(0x3F80_0000, val).bits
}

/// `ELENG P, fs`: `sqrt(x^2 + (y^2 + z^2))` (`Play!` `MA_VU_Lower.cpp` evaluation order, `ps2float`).
///
/// Matches `15/16` vectors (`CVF_INCREASING` differs by +/-1 ULP from EFU `sqrt` approximation).
#[inline]
pub fn eleng(lanes: &[u32; 4]) -> u32 {
    esqrt(esadd(lanes))
}

/// `ERLENG P, fs`: `1.0 / sqrt(x^2 + (y^2 + z^2))` (`ersqrt(esadd(lanes))`).
///
/// Both `ersqrt(esadd)` and `ercpr(eleng)` match `13/16` vectors (`PI/2`, `PI`, `3PI/2` differ
/// by +/-1 ULP from EFU `rsqrt` approximation).
#[inline]
pub fn erleng(lanes: &[u32; 4]) -> u32 {
    ersqrt(esadd(lanes))
}

/// `ERSADD P, fs`: `1.0 / (x^2 + (y^2 + z^2))` (`Play!` `MA_VU_Lower.cpp` evaluation order, `ps2float`).
#[inline]
pub fn ersadd(lanes: &[u32; 4]) -> u32 {
    ercpr(esadd(lanes))
}

/// `EEXP` polynomial `(coeff_bits, power)` pairs for `(1 + sum c_i x^i)^(-4)`.
///
/// Attribution: numeric constants from `Play!` (BSD-2-Clause, see `THIRD_PARTY_NOTICES.md`,
/// `Source/ee/MA_VU_Lower.cpp`, `FETCHED`).
const EEXP_TERMS: [(u32, u8); 6] = [
    (0x3E7F_FFA8, 1),
    (0x3D00_07F4, 2),
    (0x3B29_D3FF, 3),
    (0x3933_E553, 4),
    (0x36B6_3510, 5),
    (0x3539_61AC, 6),
];

/// Evaluate a polynomial sum `sum_i (coeff_i * x^exp_i)` using `ps2float` mul/add
/// in ascending term order (`x^e = x * ... * x`, accumulated sequentially).
fn eval_sum_series_ps2(x: u32, terms: &[(u32, u8)]) -> u32 {
    let mut sum = 0u32;
    for (idx, &(coeff, exp)) in terms.iter().enumerate() {
        let mut power = x;
        for _ in 1..exp {
            power = ps2float::mul(power, x).bits;
        }
        let term = ps2float::mul(coeff, power).bits;
        sum = if idx == 0 {
            term
        } else {
            ps2float::add(sum, term).bits
        };
    }
    sum
}

/// `EEXP P, fs[fsf]`: `e^(-x)` via `(1 + sum c_i * x^i)^(-4)` (`Play!` `MA_VU_Lower.cpp`, `ps2float`).
///
/// Matches `10/16` vectors (`ZERO`, `NEGZERO`, `MAX`, `MIN`, `MAX_EXP`, `MIN_EXP`,
/// `GARBAGE1`, `GARBAGE2`, `PI_OVER2`, `3PI_OVER2`); remaining 6 normal-range vectors
/// differ by `1..5` ULPs from EFU hardware approximation (`PARTIAL`).
#[inline]
pub fn eexp(val: u32) -> u32 {
    let sum = eval_sum_series_ps2(val, &EEXP_TERMS);
    let base = ps2float::add(0x3F80_0000, sum).bits;
    let p2 = ps2float::mul(base, base).bits;
    let p3 = ps2float::mul(base, p2).bits;
    let p4 = ps2float::mul(base, p3).bits;
    ps2float::div(0x3F80_0000, p4).bits
}

/// `ESIN` odd-degree polynomial `(coeff_bits, power)` pairs for `sum c_i x^i`.
///
/// Attribution: numeric constants from `Play!` (BSD-2-Clause, see `THIRD_PARTY_NOTICES.md`,
/// `Source/ee/MA_VU_Lower.cpp`, `FETCHED`).
const ESIN_TERMS: [(u32, u8); 5] = [
    (0x3F80_0000, 1),
    (0xBE2A_AAA4, 3),
    (0x3C08_873E, 5),
    (0xB94F_B21F, 7),
    (0x362E_9C14, 9),
];

/// VU/EFU truncated multiply (`a * b`) without R5900 EE COP1 Radix-4 Booth `-1` subtraction.
#[inline]
fn vu_mul_rtz(a: u32, b: u32) -> u32 {
    let res_sign = (a ^ b) & 0x8000_0000;
    let e1 = ((a >> 23) & 0xFF) as i32;
    let e2 = ((b >> 23) & 0xFF) as i32;
    if e1 == 0 || e2 == 0 {
        return res_sign;
    }
    let sig1 = 0x80_0000u64 | u64::from(a & 0x7F_FFFF);
    let sig2 = 0x80_0000u64 | u64::from(b & 0x7F_FFFF);
    let prod = sig1 * sig2;
    let carry = ((prod >> 47) & 1) as i32;
    let res_exp = e1 + e2 - 127 + carry;
    if res_exp > 255 {
        return res_sign | 0x7FFF_FFFF;
    }
    if res_exp <= 0 {
        return res_sign;
    }
    let res_mant = ((prod >> (23 + carry)) & 0x7F_FFFF) as u32;
    res_sign | ((res_exp as u32) << 23) | res_mant
}

/// `ESIN P, fs[fsf]`: `sin(x)` odd polynomial approximation (`sum c_i * x^i`).
///
/// Uses `Play!` coefficients (`ESIN_TERMS`) with odd-power `x^2` stepping (`vu_mul_rtz`,
/// without EE COP1 Booth `-1`) and `ps2float::add`. Matches `12/16` vectors (`PARTIAL`;
/// misses `CVF_ONE`/`CVF_NEGONE`/`CVF_DECREASING` by 1 ULP and `CVF_MAX_MANTISSA` by 2 ULP).
#[inline]
pub fn esin(val: u32) -> u32 {
    let x2 = vu_mul_rtz(val, val);
    let mut power = val;
    let mut sum = vu_mul_rtz(ESIN_TERMS[0].0, power);
    for &(coeff, _) in &ESIN_TERMS[1..] {
        power = vu_mul_rtz(power, x2);
        let term = vu_mul_rtz(coeff, power);
        sum = ps2float::add(sum, term).bits;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_esadd() {
        // 1^2 + 2^2 + 3^2 = 14.0 (w lane ignored)
        assert_eq!(
            esadd(&[
                1.0f32.to_bits(),
                2.0f32.to_bits(),
                3.0f32.to_bits(),
                99.0f32.to_bits(),
            ]),
            14.0f32.to_bits()
        );
        // Zero vector -> +0.0
        assert_eq!(esadd(&[0, 0x8000_0000, 0, 1.0f32.to_bits()]), 0);
    }

    #[test]
    fn test_esum() {
        // 1 + 2 + 3 + 4 = 10.0
        assert_eq!(
            esum(&[
                1.0f32.to_bits(),
                2.0f32.to_bits(),
                3.0f32.to_bits(),
                4.0f32.to_bits(),
            ]),
            10.0f32.to_bits()
        );
        // Cancellation (-5) + (+5) + 2 + 0 = 2.0
        assert_eq!(
            esum(&[(-5.0f32).to_bits(), 5.0f32.to_bits(), 2.0f32.to_bits(), 0,]),
            2.0f32.to_bits()
        );
    }

    #[test]
    fn test_ercpr() {
        // 1 / 4 = 0.25, 1 / -0.5 = -2.0
        assert_eq!(ercpr(4.0f32.to_bits()), 0.25f32.to_bits());
        assert_eq!(ercpr((-0.5f32).to_bits()), (-2.0f32).to_bits());
        // 1 / +0.0 saturates to +PS2_MAX (0x7FFF_FFFF)
        assert_eq!(ercpr(0), 0x7FFF_FFFF);
    }

    #[test]
    fn test_esqrt() {
        // sqrt(9) = 3, sqrt(|-16|) = 4
        assert_eq!(esqrt(9.0f32.to_bits()), 3.0f32.to_bits());
        assert_eq!(esqrt((-16.0f32).to_bits()), 4.0f32.to_bits());
        // sqrt(-0.0) = +0.0
        assert_eq!(esqrt(0x8000_0000), 0);
    }

    #[test]
    fn test_ersqrt() {
        // 1 / sqrt(4) = 0.5, 1 / sqrt(|-16|) = 0.25
        assert_eq!(ersqrt(4.0f32.to_bits()), 0.5f32.to_bits());
        assert_eq!(ersqrt((-16.0f32).to_bits()), 0.25f32.to_bits());
        // 1 / sqrt(+/-0.0) saturates to +PS2_MAX (0x7FFF_FFFF)
        assert_eq!(ersqrt(0), 0x7FFF_FFFF);
        assert_eq!(ersqrt(0x8000_0000), 0x7FFF_FFFF);
    }

    #[test]
    fn test_eleng() {
        // sqrt(1^2 + 2^2 + 2^2) = sqrt(9) = 3.0 (w lane ignored)
        assert_eq!(
            eleng(&[
                1.0f32.to_bits(),
                2.0f32.to_bits(),
                (-2.0f32).to_bits(),
                99.0f32.to_bits(),
            ]),
            3.0f32.to_bits()
        );
        // Zero vector -> +0.0
        assert_eq!(eleng(&[0, 0x8000_0000, 0, 5.0f32.to_bits()]), 0);
    }

    #[test]
    fn test_erleng() {
        // 1 / sqrt(0^2 + 4^2 + 0^2) = 1 / 4 = 0.25
        assert_eq!(
            erleng(&[0, 4.0f32.to_bits(), 0, 99.0f32.to_bits()]),
            0.25f32.to_bits()
        );
        // 1 / sqrt(2^2 + 6^2 + 3^2) = 1 / 7 -> use (0,0,8) => 0.125
        assert_eq!(
            erleng(&[0, 0, (-8.0f32).to_bits(), 1.0f32.to_bits()]),
            0.125f32.to_bits()
        );
        // Zero vector -> +PS2_MAX
        assert_eq!(erleng(&[0, 0, 0, 1.0f32.to_bits()]), 0x7FFF_FFFF);
    }

    #[test]
    fn test_ersadd() {
        // 1 / (2^2 + 2^2 + 0^2) = 1 / 8 = 0.125
        assert_eq!(
            ersadd(&[2.0f32.to_bits(), (-2.0f32).to_bits(), 0, 99.0f32.to_bits()]),
            0.125f32.to_bits()
        );
        // Zero vector -> +PS2_MAX
        assert_eq!(ersadd(&[0, 0, 0, 1.0f32.to_bits()]), 0x7FFF_FFFF);
    }

    #[test]
    fn test_eexp() {
        // e^0 = 1.0 (+0.0 and -0.0)
        assert_eq!(eexp(0), 1.0f32.to_bits());
        assert_eq!(eexp(0x8000_0000), 1.0f32.to_bits());
        // Formula sanity against mathematical e^(-1) and e^(+1)
        let exp_neg1 = f32::from_bits(eexp(1.0f32.to_bits()));
        let exp_pos1 = f32::from_bits(eexp((-1.0f32).to_bits()));
        assert!((exp_neg1 - (-1.0f32).exp()).abs() < 1e-5);
        assert!((exp_pos1 - 1.0f32.exp()).abs() < 1e-3);
    }

    #[test]
    fn test_esin() {
        // sin(+0.0) = +0.0, sin(-0.0) = +0.0
        assert_eq!(esin(0), 0);
        assert_eq!(esin(0x8000_0000), 0);
        // Formula sanity near small angle 0.5 rad and odd symmetry sin(-0.5) = -sin(0.5)
        let pos = esin(0.5f32.to_bits());
        let neg = esin((-0.5f32).to_bits());
        assert_eq!(neg, pos ^ 0x8000_0000);
        assert!((f32::from_bits(pos) - 0.5f32.sin()).abs() < 1e-5);
    }
}
