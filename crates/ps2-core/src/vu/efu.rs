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

        // Group B (pending)
        0x73E => 0, // ELENG
        0x73F => 0, // ERLENG
        0x73D => 0, // ERSADD

        // Group C (pending)
        0x7FE => 0, // EEXP
        0x7FC => 0, // ESIN
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
}
