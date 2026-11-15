//! Vector Unit (`VU0` / `VU1`) micro-mode pipeline timing and hazard model.
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#vu`, `FETCHED`):
//!   * FMAC Upper instructions have a 4-cycle pipeline (`E1..E4`); a subsequent instruction reading
//!     a written `VF` lane stalls until stage `E4` completes (`1..=3` stall cycles).
//!   * `CLIP` updates the 24-bit `CLIP` flag register with a 4-cycle pipeline latency.
//!   * `FDIV` instructions update `Q` after `7` cycles (`DIV`, `SQRT`) or `13` cycles (`RSQRT`).
//!     Issuing another `FDIV` instruction or `WAITQ` while `FDIV` is busy stalls the VU until the
//!     in-flight `FDIV` completes and commits `Q`.
//!   * `EFU` instructions (`VU1`) update `P` after their unit latency (`WAITP` or a subsequent `EFU`
//!     instruction stalls until `P` commits).
//! - `ps2autotests` (`tests/vu/lower/fdivdelay.cpp` & `fdivdelay.expected`,
//!   `tests/vu/upper/clip.cpp` & `clip.expected`, `FETCHED`):
//!   * Verifies exact cycle counts for `DIV`/`SQRT` (`7` cycles), `RSQRT` (`13` cycles), `WAITQ`,
//!     back-to-back `FDIV` stalls, FMAC-to-`CLIP` RAW stalls (`0`, `1`, `2` intervening ops and branch
//!     delay slots), and 4-cycle `CLIP`-to-`FCGET` flag pipeline latency.

use super::state::VuState;

/// Latency in VU cycles for `DIV` and `SQRT` (`ps2tek` `#vu`, `tests/vu/lower/fdivdelay.expected`, `FETCHED`).
pub const FDIV_LATENCY_DIV_SQRT: u8 = 7;
/// Latency in VU cycles for `RSQRT` (`ps2tek` `#vu`, `tests/vu/lower/fdivdelay.expected`, `FETCHED`).
pub const FDIV_LATENCY_RSQRT: u8 = 13;

/// Per-unit micro-mode pipeline timing state encapsulated outside the opcode handlers.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VuPipeline {
    /// 3-stage FMAC `VF` writeback hazard window:
    /// - `fmac_pipe[0]`: `(reg, dest)` written 1 cycle ago (requires 3 stall cycles on RAW read).
    /// - `fmac_pipe[1]`: `(reg, dest)` written 2 cycles ago (requires 2 stall cycles on RAW read).
    /// - `fmac_pipe[2]`: `(reg, dest)` written 3 cycles ago (requires 1 stall cycle on RAW read).
    pub fmac_pipe: [(u8, u8); 3],
    /// 3-stage shift queue feeding architectural `vu.clip_flag` (`4`-cycle total `CLIP` latency).
    pub clip_queue: [u32; 3],
    /// Most recently computed 24-bit `CLIP` shift register value at the head of the 4-cycle pipeline.
    pub clip_latest: u32,
    /// Pending `Q` result computed by an in-flight `DIV`, `SQRT`, or `RSQRT` instruction.
    pub fdiv_pending_q: u32,
    /// Remaining cycles until `fdiv_pending_q` commits to `vu.q` (`0` = idle).
    pub fdiv_cycles: u8,
    /// Pending `P` result computed by an in-flight `VU1` `EFU` instruction.
    pub efu_pending_p: u32,
    /// Remaining cycles until `efu_pending_p` commits to `vu.p` (`0` = idle).
    pub efu_cycles: u8,
}

impl Default for VuPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl VuPipeline {
    /// Create an idle `VuPipeline`.
    pub const fn new() -> Self {
        Self {
            fmac_pipe: [(0, 0); 3],
            clip_queue: [0; 3],
            clip_latest: 0,
            fdiv_pending_q: 0,
            fdiv_cycles: 0,
            efu_pending_p: 0,
            efu_cycles: 0,
        }
    }

    /// Reset all in-flight pipeline stages.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Synchronize the `CLIP` flag pipeline when `CTC2 $18` or `FCSET` writes `clip_flag` directly.
    #[inline(always)]
    pub fn set_clip_flag(&mut self, val: u32) {
        let masked = val & 0x00FF_FFFF;
        self.clip_queue = [masked; 3];
        self.clip_latest = masked;
    }

    /// Prepare pipeline state when starting a microprogram (`VCALLMS`, `VCALLMSR`, `CTC2 $31`).
    #[inline]
    pub fn on_start_micro(&mut self, initial_clip: u32) {
        self.fmac_pipe = [(0, 0); 3];
        self.set_clip_flag(initial_clip);
        self.fdiv_cycles = 0;
        self.efu_cycles = 0;
    }
}

/// Snapshot of architectural registers captured immediately before an LIW executes.
#[derive(Copy, Clone, Debug)]
pub struct PreLiwState {
    pub pre_q: u32,
    pub pre_p: u32,
    pub pre_clip: u32,
}

/// Return the `(reg, dest)` `VF` write produced by an Upper FMAC instruction, or `(0, 0)` if none.
#[inline]
pub const fn upper_fmac_vf_write(upper: u32) -> (u8, u8) {
    let dest = ((upper >> 21) & 0xF) as u8;
    let ft = ((upper >> 16) & 0x1F) as u8;
    let fd = ((upper >> 6) & 0x1F) as u8;
    let funct6 = upper & 0x3F;

    match funct6 {
        0x00..=0x2D | 0x2F => {
            if fd != 0 && dest != 0 {
                (fd, dest)
            } else {
                (0, 0)
            }
        }
        0x2E => {
            let xyz_dest = dest & 0b1110;
            if fd != 0 && xyz_dest != 0 {
                (fd, xyz_dest)
            } else {
                (0, 0)
            }
        }
        0x3C..=0x3F => {
            let op11 = upper & 0x7FF;
            if matches!(op11, 0x13C..=0x17F | 0x1FD) && ft != 0 && dest != 0 {
                (ft, dest)
            } else {
                (0, 0)
            }
        }
        _ => (0, 0),
    }
}

/// Return the `(reg, lane_mask)` pairs read from `VF` registers by an Upper instruction.
#[inline]
pub const fn upper_vf_reads(upper: u32) -> [(u8, u8); 2] {
    let dest = ((upper >> 21) & 0xF) as u8;
    let ft = ((upper >> 16) & 0x1F) as u8;
    let fs = ((upper >> 11) & 0x1F) as u8;
    let funct6 = upper & 0x3F;
    let bc_mask = 1u8 << (3 - (upper & 3));

    match funct6 {
        0x00..=0x1B => [(fs, dest), (ft, bc_mask)],
        0x1C..=0x27 => [(fs, dest), (0, 0)],
        0x28..=0x2D | 0x2F => [(fs, dest), (ft, dest)],
        0x2E => [(fs, 0b1110), (ft, 0b1110)],
        0x3C..=0x3F => {
            let op11 = upper & 0x7FF;
            match op11 {
                0x03C..=0x0FF | 0x1BC..=0x1BF => [(fs, dest), (ft, bc_mask)],
                0x13C..=0x17F | 0x1FC..=0x1FE | 0x23C..=0x27F => [(fs, dest), (0, 0)],
                0x1FF => [(fs, 0b1110), (ft, 0b0001)],
                0x2BC..=0x2FD => [(fs, dest), (ft, dest)],
                0x2FE => [(fs, 0b1110), (ft, 0b1110)],
                _ => [(0, 0), (0, 0)],
            }
        }
        _ => [(0, 0), (0, 0)],
    }
}

/// Return the `(reg, lane_mask)` pairs read from `VF` registers by a Lower instruction.
#[inline]
pub const fn lower_vf_reads(lower: u32, has_i_bit: bool) -> [(u8, u8); 2] {
    if has_i_bit {
        return [(0, 0), (0, 0)];
    }
    let dest = ((lower >> 21) & 0xF) as u8;
    let ft = ((lower >> 16) & 0x1F) as u8;
    let fs = ((lower >> 11) & 0x1F) as u8;
    let fsf_mask = 1u8 << (3 - ((lower >> 21) & 3));
    let ftf_mask = 1u8 << (3 - ((lower >> 23) & 3));

    if (lower & 0x8000_0000) != 0 {
        let op11 = lower & 0x7FF;
        match op11 {
            0x33C | 0x33D | 0x37D | 0x37F => [(fs, dest), (0, 0)],
            0x3BC | 0x3BE => [(fs, fsf_mask), (ft, ftf_mask)],
            0x3BD => [(ft, ftf_mask), (0, 0)],
            0x3FC | 0x43E | 0x43F => [(fs, fsf_mask), (0, 0)],
            0x73C..=0x73F => [(fs, 0b1110), (0, 0)],
            0x77C => [(fs, 0b1100), (0, 0)],
            0x77D => [(fs, 0b1010), (0, 0)],
            0x77E => [(fs, 0b1111), (0, 0)],
            0x7BC..=0x7BE | 0x7FC..=0x7FE => [(fs, fsf_mask), (0, 0)],
            _ => [(0, 0), (0, 0)],
        }
    } else {
        let op7 = (lower >> 25) & 0x7F;
        if op7 == 0x01 {
            [(fs, dest), (0, 0)]
        } else {
            [(0, 0), (0, 0)]
        }
    }
}

/// Return the hardware latency in VU cycles for a `VU1` `EFU` instruction
/// (`ps2tek` `#vulower`, `ps2autotests` `tests/vu/assemble/ops.cpp` lines 107-130, `FETCHED`).
#[inline]
pub const fn efu_latency(op11: u32) -> Option<u8> {
    match op11 {
        0x73C => Some(11), // ESADD
        0x73D => Some(18), // ERSADD
        0x73E => Some(18), // ELENG
        0x73F => Some(24), // ERLENG
        0x77C => Some(54), // EATANxy
        0x77D => Some(54), // EATANxz
        0x77E => Some(12), // ESUM
        0x7BC => Some(12), // ESQRT
        0x7BD => Some(18), // ERSQRT
        0x7BE => Some(12), // ERCPR
        0x7FC => Some(29), // ESIN
        0x7FD => Some(54), // EATAN
        0x7FE => Some(44), // EEXP
        _ => None,
    }
}

/// Compute the number of stall cycles required before `(upper, lower)` can issue.
#[inline]
pub fn compute_stall_cycles(pipe: &VuPipeline, upper: u32, lower: u32, has_i_bit: bool) -> u8 {
    let mut stall = 0u8;

    // 1. Check FMAC VF RAW hazards (stages 0, 1, 2 correspond to 3, 2, 1 stall cycles).
    let u_reads = upper_vf_reads(upper);
    let l_reads = lower_vf_reads(lower, has_i_bit);
    for (stage_idx, &(w_reg, w_mask)) in pipe.fmac_pipe.iter().enumerate() {
        if w_reg != 0 && w_mask != 0 {
            let stage_stall = (3 - stage_idx) as u8;
            for &(r_reg, r_mask) in u_reads.iter().chain(l_reads.iter()) {
                if r_reg == w_reg && (r_mask & w_mask) != 0 && stage_stall > stall {
                    stall = stage_stall;
                }
            }
        }
    }

    // 2. Check FDIV busy stall (`DIV`, `SQRT`, `RSQRT`, `WAITQ`) and EFU busy stall (`EFU`, `WAITP`).
    if !has_i_bit && (lower & 0x8000_0000) != 0 {
        let op11 = lower & 0x7FF;
        if matches!(op11, 0x3BC..=0x3BF) && pipe.fdiv_cycles > stall {
            stall = pipe.fdiv_cycles;
        }
        if (op11 == 0x7BF || efu_latency(op11).is_some()) && pipe.efu_cycles > stall {
            stall = pipe.efu_cycles;
        }
    }

    stall
}

/// Advance the VU pipeline by 1 cycle, shifting `fmac_write` into `fmac_pipe[0]`.
#[inline]
pub fn advance_cycle_with_fmac(vu: &mut VuState, fmac_write: (u8, u8)) {
    // 1. Shift FMAC writeback hazard window.
    vu.pipeline.fmac_pipe[2] = vu.pipeline.fmac_pipe[1];
    vu.pipeline.fmac_pipe[1] = vu.pipeline.fmac_pipe[0];
    vu.pipeline.fmac_pipe[0] = fmac_write;

    // 2. Shift 4-cycle CLIP flag pipeline.
    vu.clip_flag = vu.pipeline.clip_queue[2];
    vu.pipeline.clip_queue[2] = vu.pipeline.clip_queue[1];
    vu.pipeline.clip_queue[1] = vu.pipeline.clip_queue[0];
    vu.pipeline.clip_queue[0] = vu.pipeline.clip_latest;

    // 3. Tick FDIV (`Q`) unit.
    if vu.pipeline.fdiv_cycles > 0 {
        vu.pipeline.fdiv_cycles -= 1;
        if vu.pipeline.fdiv_cycles == 0 {
            vu.q = vu.pipeline.fdiv_pending_q;
        }
    }

    // 4. Tick EFU (`P`) unit.
    if vu.pipeline.efu_cycles > 0 {
        vu.pipeline.efu_cycles -= 1;
        if vu.pipeline.efu_cycles == 0 {
            vu.p = vu.pipeline.efu_pending_p;
        }
    }
}

/// Advance the VU pipeline by 1 stall cycle (no new FMAC write issued).
#[inline]
pub fn advance_stall_cycle(vu: &mut VuState) {
    advance_cycle_with_fmac(vu, (0, 0));
}

/// Run any pre-issue stall cycles for `(upper, lower)` and snapshot architectural `Q`, `P`, `CLIP`.
#[inline]
pub fn before_liw(vu: &mut VuState, upper: u32, lower: u32, has_i_bit: bool) -> PreLiwState {
    let stall = compute_stall_cycles(&vu.pipeline, upper, lower, has_i_bit);
    for _ in 0..stall {
        advance_stall_cycle(vu);
    }
    PreLiwState {
        pre_q: vu.q,
        pre_p: vu.p,
        pre_clip: vu.clip_flag,
    }
}

/// If `upper` is `CLIP` (`0x1FF`), load `clip_latest` into `vu.clip_flag` immediately before
/// `exec_upper_op_with_sources` runs so the untimed `CLIP` handler shifts `clip_latest`.
#[inline]
pub fn prepare_upper_clip(vu: &mut VuState, upper: u32) {
    if (upper & 0x7FF) == 0x1FF {
        vu.clip_flag = vu.pipeline.clip_latest;
    }
}

/// Commit `CLIP`, `FDIV`, `EFU`, and `FMAC` pipeline transitions after an LIW executes and
/// advance the pipeline by 1 execution cycle.
#[inline]
pub fn after_liw(vu: &mut VuState, upper: u32, lower: u32, has_i_bit: bool, pre: PreLiwState) {
    let upper_is_clip = (upper & 0x7FF) == 0x1FF;
    let lower_is_fcset = !has_i_bit && (lower & 0x8000_0000) == 0 && ((lower >> 25) & 0x7F) == 0x11;

    if lower_is_fcset {
        let fcset_val = lower & 0x00FF_FFFF;
        if upper_is_clip {
            let new_clip = vu.clip_flag;
            vu.pipeline.set_clip_flag(fcset_val);
            vu.pipeline.clip_latest = new_clip;
            vu.clip_flag = fcset_val;
        } else {
            vu.clip_flag = fcset_val;
            vu.pipeline.set_clip_flag(fcset_val);
        }
    } else if upper_is_clip {
        vu.pipeline.clip_latest = vu.clip_flag;
        vu.clip_flag = pre.pre_clip;
    }

    if !has_i_bit && (lower & 0x8000_0000) != 0 {
        let op11 = lower & 0x7FF;
        match op11 {
            0x3BC | 0x3BD => {
                vu.pipeline.fdiv_pending_q = vu.q;
                vu.pipeline.fdiv_cycles = FDIV_LATENCY_DIV_SQRT;
                vu.q = pre.pre_q;
            }
            0x3BE => {
                vu.pipeline.fdiv_pending_q = vu.q;
                vu.pipeline.fdiv_cycles = FDIV_LATENCY_RSQRT;
                vu.q = pre.pre_q;
            }
            _ => {
                if let Some(lat) = efu_latency(op11) {
                    vu.pipeline.efu_pending_p = vu.p;
                    vu.pipeline.efu_cycles = lat;
                    vu.p = pre.pre_p;
                }
            }
        }
    }

    let fmac_write = upper_fmac_vf_write(upper);
    advance_cycle_with_fmac(vu, fmac_write);
}
