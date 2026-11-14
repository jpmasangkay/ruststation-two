//! Sub-System Interface (`SIF`) memory-mapped registers (`0x1000_F200..=0x1000_F260`).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#sif`):
//!   * `0x1000_F200` (`SIF_MSCOM`): EE -> IOP communication address (`R/W` on EE).
//!   * `0x1000_F210` (`SIF_SMCOM`): IOP -> EE communication address (`R` on EE, `R/W` on IOP).
//!   * `0x1000_F220` (`SIF_MSFLG`): EE -> IOP status flags (`EE` writes set bits; `IOP` writes clear bits).
//!   * `0x1000_F230` (`SIF_SMFLG`): IOP -> EE status flags (`EE` writes clear bits; `IOP` writes set bits).
//!   * `0x1000_F240` (`SIF_CTRL`): Control register.
//!   * `0x1000_F260` (`SIF_BD6`): Boot/unknown register (`0x1000_F260`).

/// Sub-System Interface (`SIF`) register file (`0x1000_F200..=0x1000_F260`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SifRegs {
    /// `0x1000_F200` (`SIF_MSCOM`): EE -> IOP communication register.
    pub mscom: u32,
    /// `0x1000_F210` (`SIF_SMCOM`): IOP -> EE communication register.
    pub smcom: u32,
    /// `0x1000_F220` (`SIF_MSFLG`): EE -> IOP flags (`EE` write sets bits).
    pub msflg: u32,
    /// `0x1000_F230` (`SIF_SMFLG`): IOP -> EE flags (`EE` write clears bits).
    pub smflg: u32,
    /// `0x1000_F240` (`SIF_CTRL`): Control register.
    // TODO(verify): Exact hardware reset value and bit masks of SIF_CTRL (0x1000_F240) and SIF_BD6 (0x1000_F260).
    pub ctrl: u32,
    /// `0x1000_F260` (`SIF_BD6`): Boot/status register.
    pub bd6: u32,
}

impl Default for SifRegs {
    fn default() -> Self {
        Self::new()
    }
}

impl SifRegs {
    /// Create a newly reset [`SifRegs`] register block.
    pub const fn new() -> Self {
        Self {
            mscom: 0,
            smcom: 0,
            msflg: 0,
            smflg: 0,
            ctrl: 0,
            bd6: 0,
        }
    }

    /// Read a 32-bit SIF register from the Emotion Engine side (`0x1000_F200..=0x1000_F260`).
    #[inline]
    pub fn read32_ee(&self, paddr: u32) -> u32 {
        match paddr {
            0x1000_F200 => self.mscom,
            0x1000_F210 => self.smcom,
            0x1000_F220 => self.msflg,
            0x1000_F230 => self.smflg,
            0x1000_F240 => self.ctrl,
            0x1000_F260 => self.bd6,
            _ => {
                tracing::trace!(
                    paddr = format_args!("0x{paddr:08X}"),
                    "Unhandled SIF read32_ee"
                );
                0
            }
        }
    }

    /// Write a 32-bit SIF register from the Emotion Engine side (`0x1000_F200..=0x1000_F260`).
    ///
    /// Reference: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#sif`):
    /// - `0x1000_F200` (`MSCOM`): sets `mscom = val`.
    /// - `0x1000_F210` (`SMCOM`): read-only from the EE.
    /// - `0x1000_F220` (`MSFLG`): EE writes set bits (`msflg |= val`).
    /// - `0x1000_F230` (`SMFLG`): EE writes clear bits (`smflg &= !val`).
    #[inline]
    pub fn write32_ee(&mut self, paddr: u32, val: u32) {
        match paddr {
            0x1000_F200 => self.mscom = val,
            0x1000_F210 => {
                // Read-only from EE (`ps2tek` `#sif`).
            }
            0x1000_F220 => self.msflg |= val,
            0x1000_F230 => self.smflg &= !val,
            0x1000_F240 => self.ctrl = val,
            0x1000_F260 => self.bd6 = val,
            _ => {
                tracing::trace!(
                    paddr = format_args!("0x{paddr:08X}"),
                    val = format_args!("0x{val:08X}"),
                    "Unhandled SIF write32_ee"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sif_ee_register_read_write_semantics() {
        let mut sif = SifRegs::new();

        // MSCOM is read/write on EE; SMCOM is read-only on EE.
        sif.write32_ee(0x1000_F200, 0x0012_3450);
        assert_eq!(sif.read32_ee(0x1000_F200), 0x0012_3450);
        sif.smcom = 0x0008_0000;
        sif.write32_ee(0x1000_F210, 0xDEAD_BEEF);
        assert_eq!(sif.read32_ee(0x1000_F210), 0x0008_0000);

        // MSFLG sets bits on EE write; SMFLG clears bits on EE write.
        sif.write32_ee(0x1000_F220, 0x0001_0000);
        sif.write32_ee(0x1000_F220, 0x0002_0000);
        assert_eq!(sif.read32_ee(0x1000_F220), 0x0003_0000);

        sif.smflg = 0x0003_0000;
        sif.write32_ee(0x1000_F230, 0x0001_0000);
        assert_eq!(sif.read32_ee(0x1000_F230), 0x0002_0000);
    }
}
