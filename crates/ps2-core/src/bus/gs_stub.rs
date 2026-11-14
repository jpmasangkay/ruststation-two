//! Graphics Synthesizer (GS) Privileged Registers Stub (`0x1200_0000..=0x1200_1FFF`).
//!
//! Primary source reference:
//! - `ps2tek` ("GS Privileged Registers"):
//!   * Mapped into the EE physical address space at `0x1200_0000` in 64-bit register slots.
//!   * `GS_CSR` (`0x1200_1000`):
//!     - Bits `0..=4` (`SIGNAL`, `FINISH`, `HSINT`, `VSINT`, `EDWINT`): writing `1` clears
//!       the corresponding interrupt status bit.
//!     - Bit `13` (`FIELD`): toggles every VBlank (`0` = Even, `1` = Odd).
//!     - Bits `14..=15` (`FIFO`): `0b01` = FIFO empty (so BIOS/game wait loops do not stall).
//!     - Bits `16..=23` (`REV` = `0x1B`), Bits `24..=31` (`ID` = `0x55`).

/// Default read-only `REV` (`0x1B`) and `ID` (`0x55`) plus `FIFO = Empty` (`1 << 14`) in `GS_CSR` (`ps2tek`).
const GS_CSR_FIXED_BITS: u64 = (0x55u64 << 24) | (0x1Bu64 << 16) | (1u64 << 14);

/// Stub state for the GS Privileged Registers in Phase 1 (before full `ps2-gs` arrives in Phase 2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GsPrivilegedStub {
    pub pmode: u64,
    pub smode1: u64,
    pub smode2: u64,
    pub srfsh: u64,
    pub synch1: u64,
    pub synch2: u64,
    pub syncv: u64,
    pub dispfb1: u64,
    pub display1: u64,
    pub dispfb2: u64,
    pub display2: u64,
    pub extbuf: u64,
    pub extdata: u64,
    pub extwrite: u64,
    pub bgcolor: u64,
    pub csr: u64,
    pub imr: u64,
    pub busdir: u64,
    pub siglblid: u64,
}

impl Default for GsPrivilegedStub {
    fn default() -> Self {
        Self::new()
    }
}

impl GsPrivilegedStub {
    pub const fn new() -> Self {
        Self {
            pmode: 0,
            smode1: 0,
            smode2: 0,
            srfsh: 0,
            synch1: 0,
            synch2: 0,
            syncv: 0,
            dispfb1: 0,
            display1: 0,
            dispfb2: 0,
            display2: 0,
            extbuf: 0,
            extdata: 0,
            extwrite: 0,
            bgcolor: 0,
            csr: GS_CSR_FIXED_BITS,
            imr: 0x7F00,
            busdir: 0,
            siglblid: 0,
        }
    }

    /// Toggle `GS_CSR.FIELD` (bit 13) and set `GS_CSR.VSINT` (bit 3) on VBlank start (`ps2tek`).
    #[inline(always)]
    pub fn on_vblank_start(&mut self) {
        self.csr ^= 1 << 13;
        self.csr |= 1 << 3;
    }

    /// Set `GS_CSR.HSINT` (bit 2) on HBlank (`ps2tek`).
    #[inline(always)]
    pub fn on_hblank(&mut self) {
        self.csr |= 1 << 2;
    }

    /// Read a 64-bit GS privileged register at `paddr` (`0x1200_0000..=0x1200_1FFF`).
    pub const fn read64(&self, paddr: u32) -> u64 {
        match paddr & 0x1FFF {
            0x0000 => self.pmode,
            0x0010 => self.smode1,
            0x0020 => self.smode2,
            0x0030 => self.srfsh,
            0x0040 => self.synch1,
            0x0050 => self.synch2,
            0x0060 => self.syncv,
            0x0070 => self.dispfb1,
            0x0080 => self.display1,
            0x0090 => self.dispfb2,
            0x00A0 => self.display2,
            0x00B0 => self.extbuf,
            0x00C0 => self.extdata,
            0x00D0 => self.extwrite,
            0x00E0 => self.bgcolor,
            0x1000 => self.csr | GS_CSR_FIXED_BITS,
            0x1010 => self.imr,
            0x1040 => self.busdir,
            0x1080 => self.siglblid,
            _ => 0,
        }
    }

    /// Write a 64-bit GS privileged register at `paddr` (`0x1200_0000..=0x1200_1FFF`).
    pub fn write64(&mut self, paddr: u32, val: u64) {
        match paddr & 0x1FFF {
            0x0000 => self.pmode = val,
            0x0010 => self.smode1 = val,
            0x0020 => self.smode2 = val,
            0x0030 => self.srfsh = val,
            0x0040 => self.synch1 = val,
            0x0050 => self.synch2 = val,
            0x0060 => self.syncv = val,
            0x0070 => self.dispfb1 = val,
            0x0080 => self.display1 = val,
            0x0090 => self.dispfb2 = val,
            0x00A0 => self.display2 = val,
            0x00B0 => self.extbuf = val,
            0x00C0 => self.extdata = val,
            0x00D0 => self.extwrite = val,
            0x00E0 => self.bgcolor = val,
            0x1000 => {
                // Writing 1 to bits 0..=4 clears those interrupt flags (`ps2tek`).
                let clear_mask = val & 0x1F;
                self.csr = (self.csr & !clear_mask) | GS_CSR_FIXED_BITS;
            }
            0x1010 => self.imr = val,
            0x1040 => self.busdir = val,
            0x1080 => self.siglblid = val,
            _ => {}
        }
    }

    /// Read a 32-bit half of a 64-bit GS privileged register.
    #[inline]
    pub const fn read32(&self, paddr: u32) -> u32 {
        let val64 = self.read64(paddr & !0x7);
        if (paddr & 0x4) != 0 {
            (val64 >> 32) as u32
        } else {
            val64 as u32
        }
    }

    /// Write a 32-bit half of a 64-bit GS privileged register.
    #[inline]
    pub fn write32(&mut self, paddr: u32, val: u32) {
        let aligned = paddr & !0x7;
        let current = self.read64(aligned);
        let updated = if (paddr & 0x4) != 0 {
            if (aligned & 0x1FFF) == 0x1000 {
                // Upper 32-bit write to CSR does not clear low 5 interrupt bits.
                return;
            }
            (current & 0x0000_0000_FFFF_FFFF) | (u64::from(val) << 32)
        } else if (aligned & 0x1FFF) == 0x1000 {
            u64::from(val)
        } else {
            (current & 0xFFFF_FFFF_0000_0000) | u64::from(val)
        };
        self.write64(aligned, updated);
    }
}
