//! Platform-independent PlayStation 2 Graphics Synthesizer (`ps2-gs`) reference state & renderer.
//!
//! Zero `unsafe` code, zero dependencies on `ps2-core`, and deterministic fixed-point/integer rules.

#![forbid(unsafe_code)]

pub mod coverage;
pub mod packet;
pub mod raster;
pub mod regs;
pub mod texture;
pub mod transfer;
pub mod vram;

pub use coverage::{
    is_pixel_covered, is_pixel_covered_active, CoverageConvention, CoveragePrimitive,
    ACTIVE_COVERAGE,
};
pub use packet::{
    decode_packed_qwc, execute_gsp_script, parse_gsp_script, GifDataFormat, GifStreamDecoder,
    GifTag, GspCommand, GspParseError, GspSurfaceHint, PackedDecodeOutcome,
};
pub use raster::{GsVertex, RasterizerState};
pub use regs::{gs_reg, GsRegs};
pub use texture::ClutCache;
pub use transfer::{TransferMode, TransferState};
pub use vram::{GsVram, PixelFormat, PixelLocation};

use ps2_common::Reg128;

/// Top-level GS device state (registers, 4 MB VRAM, transfers, rasterizer, display).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GsState {
    pub regs: GsRegs,
    pub vram: GsVram,
    pub transfer: TransferState,
    pub rasterizer: RasterizerState,
}

impl Default for GsState {
    fn default() -> Self {
        Self::new()
    }
}

impl GsState {
    pub fn new() -> Self {
        Self {
            regs: GsRegs::new(),
            vram: GsVram::new(),
            transfer: TransferState::new(),
            rasterizer: RasterizerState::new(),
        }
    }

    /// Clear registers, 4 MB VRAM, active transfers, and rasterizer state in place.
    pub fn reset(&mut self) {
        self.regs = GsRegs::new();
        self.vram.clear();
        self.transfer.reset();
        self.rasterizer.reset();
    }

    #[inline]
    pub fn internal_reg(&self, reg: u8) -> u64 {
        self.regs.raw[usize::from(reg & 0x7F)]
    }

    /// Apply a 64-bit internal GS register write (`0x00..=0x62`).
    pub fn write_internal_reg(&mut self, reg: u8, data: u64) {
        self.regs.write_internal(reg, data);
        match reg {
            gs_reg::PRIM => {
                self.rasterizer.on_prim_write(self.regs.prim);
            }
            gs_reg::XYZF2 => {
                self.rasterizer
                    .vertex_kick(&self.regs, &mut self.vram, data, true, true);
            }
            gs_reg::XYZ2 => {
                self.rasterizer
                    .vertex_kick(&self.regs, &mut self.vram, data, false, true);
            }
            gs_reg::XYZF3 => {
                self.rasterizer
                    .vertex_kick(&self.regs, &mut self.vram, data, true, false);
            }
            gs_reg::XYZ3 => {
                self.rasterizer
                    .vertex_kick(&self.regs, &mut self.vram, data, false, false);
            }
            gs_reg::TEX0_1 | gs_reg::TEX2_1 => {
                self.rasterizer.on_tex_reg_write(&self.regs, &self.vram, 0);
            }
            gs_reg::TEX0_2 | gs_reg::TEX2_2 => {
                self.rasterizer.on_tex_reg_write(&self.regs, &self.vram, 1);
            }
            gs_reg::TRXDIR => {
                self.transfer
                    .start_trxdir(&self.regs, &mut self.vram, data as u8);
            }
            gs_reg::HWREG => {
                self.transfer.write_hwreg(&mut self.vram, data);
            }
            _ => {}
        }
    }

    /// Update PCRTC privileged display registers (`0x1200_0000..=0x1200_00E0`).
    pub fn write_privileged_display(&mut self, offset: u32, data: u64) {
        self.regs.write_privileged_display(offset, data);
    }

    /// Read one 64-bit word from an active `Local -> Host` (`TRXDIR=1`) transfer.
    pub fn read_local_to_host_u64(&mut self) -> Option<u64> {
        self.transfer.read_local_to_host_u64(&self.vram)
    }

    /// Read one 128-bit quadword from an active `Local -> Host` (`TRXDIR=1`) transfer.
    pub fn read_local_to_host_qwc(&mut self) -> Option<Reg128> {
        self.transfer.read_local_to_host_qwc(&self.vram)
    }
}
