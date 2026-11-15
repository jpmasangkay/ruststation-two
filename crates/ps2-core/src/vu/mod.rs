//! Vector Units (`VU0` and `VU1`) state and micro/macro execution support.

pub mod micro;
pub mod state;

pub use state::{VuState, DEST_W, DEST_X, DEST_Y, DEST_Z, VF00_VALUE, VU_REVISION_ID};

/// `VU0` microprogram counter mask in 8-byte LIW units (`4 KB / 8 = 512` instructions, `0x01FF`).
pub const VU0_TPC_MASK: u16 = 0x01FF;
/// `VU1` microprogram counter mask in 8-byte LIW units (`16 KB / 8 = 2048` instructions, `0x07FF`).
pub const VU1_TPC_MASK: u16 = 0x07FF;

#[cfg(test)]
mod tests {
    use super::micro::*;
    use super::*;
    use ps2_common::Reg128;

    #[test]
    fn vu_state_vf00_and_vi00_hardwired() {
        let mut vu = VuState::new();
        assert_eq!(vu.read_vf(0), VF00_VALUE);
        assert_eq!(vu.read_vf_lane(0, 3), 0x3F80_0000);
        vu.write_vf(0, Reg128::from_u128(0xDEAD_BEEF));
        vu.write_vf_lane(0, 0, 0x1234_5678);
        assert_eq!(vu.read_vf(0), VF00_VALUE);

        assert_eq!(vu.read_vi(0), 0);
        vu.write_vi(0, 0xFFFF);
        assert_eq!(vu.read_vi(0), 0);
    }

    #[test]
    fn vu_integer_ops_and_ilwr_iswr() {
        let mut vu = VuState::new();
        let mut data_mem = vec![0u8; 0x1000];
        vu.write_vi(2, 0xDEAD);
        vu.write_vi(3, 0xFFFF);
        exec_viadd(&mut vu, 1, 2, 3);
        assert_eq!(vu.read_vi(1), 0xDEAC);

        exec_visub(&mut vu, 1, 2, 3);
        assert_eq!(vu.read_vi(1), 0xDEAE);

        exec_viaddi(&mut vu, 1, 0, 0x10); // -16 in 5-bit signed
        assert_eq!(vu.read_vi(1), 0xFFF0);

        vu.write_vi(4, 2); // qword address 2 -> byte 32
        vu.write_vi(5, 0x1338);
        exec_iswr(&vu, &mut data_mem, DEST_Y, 5, 4);
        exec_ilwr(&mut vu, &data_mem, DEST_Y, 6, 4);
        assert_eq!(vu.read_vi(6), 0x1338);
    }

    #[test]
    fn vu_micro_e_bit_delay_slot_sets_tpc() {
        let mut vu = VuState::new();
        let mut code_mem = vec![0u8; 0x1000];
        let mut data_mem = vec![0u8; 0x1000];
        // Place E-bit at instruction index 0x1D (byte offset 0xE8, upper word at 0xEC)
        code_mem[0xEC..0xF0].copy_from_slice(&UPPER_BIT_E.to_le_bytes());

        vu.start_micro(0, VU0_TPC_MASK);
        while vu.running {
            step_micro_liw(&mut vu, &code_mem, &mut data_mem, VU0_TPC_MASK);
        }
        assert_eq!(vu.tpc, 0x001F);
    }
}
