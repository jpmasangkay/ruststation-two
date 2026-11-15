//! Vector Units (`VU0` and `VU1`) state and micro/macro execution support.

pub mod efu;
pub mod micro;
pub mod pipeline;
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

    #[test]
    fn vu_loi_reads_previous_i_and_upper_wins_same_cycle_vf_conflict() {
        let mut vu = VuState::new();
        let mut code_mem = vec![0u8; 0x1000];
        let mut data_mem = vec![0u8; 0x1000];

        // Preload I = 2.0f (0x4000_0000), VF01.x = 3.0f (0x4040_0000).
        vu.i = 0x4000_0000;
        vu.write_vf_lane(1, 0, 0x4040_0000);

        // LIW 0: Upper = ADDi.x vf02, vf01, I (dest=X=0b1000, fs=1, fd=2, funct=0x22) | UPPER_BIT_I;
        //        Lower = 5.0f (0x40A0_0000).
        // Upper must read previous I (2.0f -> 3.0 + 2.0 = 5.0f = 0x40A0_0000), then I becomes 5.0f.
        let upper0 = UPPER_BIT_I | (0b1000 << 21) | (1 << 11) | (2 << 6) | 0x22;
        let lower0 = 0x40A0_0000u32;
        code_mem[0..4].copy_from_slice(&lower0.to_le_bytes());
        code_mem[4..8].copy_from_slice(&upper0.to_le_bytes());

        // LIW 1: Upper = ADDi.x vf03, vf02, I (5.0 + 5.0 = 10.0f = 0x4120_0000);
        //        Lower = MOVE.xy vf03, vf01 (bit 31=1, dest=XY=0b1100, ft=3, fs=1, op11=0x33C).
        // On lane X both write vf03.x -> Upper wins (10.0f); on lane Y only Lower writes vf03.y (0.0f).
        vu.write_vf_lane(1, 1, 0x3F80_0000); // VF01.y = 1.0f
        let upper1 = UPPER_BIT_E | (0b1000 << 21) | (2 << 11) | (3 << 6) | 0x22;
        let lower1: u32 = 0x8000_0000 | (0b1100 << 21) | (3 << 16) | (1 << 11) | 0x33C;
        code_mem[8..12].copy_from_slice(&lower1.to_le_bytes());
        code_mem[12..16].copy_from_slice(&upper1.to_le_bytes());

        vu.start_micro(0, VU0_TPC_MASK);
        while vu.running {
            step_micro_liw(&mut vu, &code_mem, &mut data_mem, VU0_TPC_MASK);
        }
        assert_eq!(vu.i, 0x40A0_0000);
        assert_eq!(vu.read_vf_lane(2, 0), 0x40A0_0000);
        assert_eq!(vu.read_vf_lane(3, 0), 0x4120_0000); // Upper won on X
        assert_eq!(vu.read_vf_lane(3, 1), 0x3F80_0000); // Lower wrote Y
    }

    #[test]
    fn vu_jalr_same_reg_and_r_lfsr_operations() {
        let mut vu = VuState::new();
        let mut data_mem = vec![0u8; 0x1000];

        // JALR vi01, vi01 at TPC=2 (delay_slot_tpc=3): target is old vi01 (6), vi01 becomes 4.
        vu.write_vi(1, 6);
        let jalr = (0x25 << 25) | (1 << 16) | (1 << 11);
        exec_micro_lower_type789(&mut vu, &mut data_mem, jalr, 3, VU0_TPC_MASK);
        assert!(vu.branch_delay_active);
        assert_eq!(vu.branch_delay_target, 6);
        assert_eq!(vu.read_vi(1), 4);

        // RINIT, RXOR, RGET, RNEXT via shared lower ops
        vu.write_vf_lane(1, 0, 0x3FA3_4567);
        let rinit = 0x8000_0000 | (1 << 11) | 0x43E; // RINIT R, vf01.x
        exec_shared_lower_op(&mut vu, &mut data_mem, rinit);
        assert_eq!(vu.r, 0x0023_4567);

        let rget = 0x8000_0000 | (0b1111 << 21) | (2 << 16) | 0x43D; // RGET.xyzw vf02
        exec_shared_lower_op(&mut vu, &mut data_mem, rget);
        assert_eq!(vu.read_vf_lane(2, 0), 0x3FA3_4567);

        let rnext = 0x8000_0000 | (0b1111 << 21) | (3 << 16) | 0x43C; // RNEXT.xyzw vf03
        exec_shared_lower_op(&mut vu, &mut data_mem, rnext);
        assert_eq!(vu.read_vf_lane(3, 0), 0x3FC6_8ACE);
    }

    #[test]
    fn vu_pipeline_clip_4_cycle_latency_and_fdiv_raw_stall() {
        let mut vu = VuState::new();
        let mut code_mem = vec![0u8; 0x1000];
        let mut data_mem = vec![0u8; 0x1000];

        // VF01 = (2.0, 2.0, 2.0, 2.0) -> CLIP(VF01, VF00) produces +x, +y, +z = 0x15.
        vu.write_vf(
            1,
            Reg128::from_u32x4(0x4000_0000, 0x4000_0000, 0x4000_0000, 0x4000_0000),
        );
        // VF16.w = 4.0 -> DIV(VF00.w, VF16.w) = 1.0 / 4.0 = 0.25 (0x3E80_0000), latency 7.
        vu.write_vf_lane(16, 3, 0x4080_0000);
        vu.q = 0x3F80_0000; // Initial Q = 1.0f

        let clip_vf01_vf00: u32 = (0b1110 << 21) | (1 << 11) | 0x1FF;
        let nop_upper: u32 = 0x0000_02FF;
        let fcget = |it: u32| -> u32 { (0x1C << 25) | (it << 16) };
        let nop_lower: u32 = 0x8000_033C; // MOVE with dest=0

        // LIW 0..4: CLIP at LIW 0 with FCGET(vi01..vi05) across LIW 0..4.
        // vi01..vi04 must read 0x0000; vi05 (4 cycles after LIW 0) must read 0x0015.
        let liws = [
            (clip_vf01_vf00, fcget(1)),
            (nop_upper, fcget(2)),
            (nop_upper, fcget(3)),
            (nop_upper, fcget(4)),
            (UPPER_BIT_E | nop_upper, fcget(5)),
            (nop_upper, nop_lower),
        ];
        for (i, &(u, l)) in liws.iter().enumerate() {
            code_mem[i * 8..i * 8 + 4].copy_from_slice(&l.to_le_bytes());
            code_mem[i * 8 + 4..i * 8 + 8].copy_from_slice(&u.to_le_bytes());
        }

        vu.start_micro(0, VU0_TPC_MASK);
        while vu.running {
            step_micro_liw(&mut vu, &code_mem, &mut data_mem, VU0_TPC_MASK);
        }
        assert_eq!(vu.read_vi(1), 0x0000);
        assert_eq!(vu.read_vi(2), 0x0000);
        assert_eq!(vu.read_vi(3), 0x0000);
        assert_eq!(vu.read_vi(4), 0x0000);
        assert_eq!(vu.read_vi(5), 0x0015);
    }
}
