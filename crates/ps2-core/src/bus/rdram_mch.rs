//! RDRAM Memory Controller Handshake Registers (`MCH_RICM` `0x1000_F430` / `MCH_DRCM` `0x1000_F440`).
//!
//! Primary source reference:
//! - `ps2tek` ("EE I/O Registers -> MCH_RICM / MCH_DRCM"):
//!   * During early BIOS initialization, the Emotion Engine probes RDRAM modules by writing commands
//!     to `MCH_RICM` (`0x1000_F430`) and `MCH_DRCM` (`0x1000_F440`) and reading back `MCH_DRCM`.
//!   * `MCH_RICM` (`0x1000_F430`) always reads `0`.
//!   * On write to `MCH_RICM`: if `SA == 0x21` (`(val >> 16) & 0xFFF == 0x21`), `SOP == 1`
//!     (`(val >> 6) & 0xF == 1`), and `(drcm >> 7) & 1 == 0`, the RDRAM device counter `sdevid_sr`
//!     resets to `0`. Bit 31 (busy flag) is cleared immediately.
//!   * On read from `MCH_DRCM` (`0x1000_F440`):
//!     - If `SOP == 0` (`(ricm >> 6) & 0xF == 0`), returns `0x0D0D_0000`.
//!     - If `SOP == 1`, switches on `SA = (ricm >> 16) & 0xFFF`:
//!       * `0x21` -> `0x1F`
//!       * `0x23` -> `0x0D0D`
//!       * `0x24` -> `0x0090 | ((ricm & 0x100) >> 3)`
//!       * `0x40` -> returns `sdevid_sr` and increments `sdevid_sr` if `< 2` (for two 16 MB RDRAM modules = 32 MB).

/// Emotion Engine RDRAM controller (`MCH_RICM` / `MCH_DRCM`) state machine.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RdramMch {
    pub ricm: u32,
    pub drcm: u32,
    pub sdevid_sr: u32,
}

impl RdramMch {
    pub const fn new() -> Self {
        Self {
            ricm: 0,
            drcm: 0,
            sdevid_sr: 0,
        }
    }

    /// Read `MCH_RICM` (`0x1000_F430`): always returns `0` (`ps2tek`).
    #[inline(always)]
    pub const fn read_ricm(&self) -> u32 {
        0
    }

    /// Write `MCH_RICM` (`0x1000_F430`).
    #[inline]
    pub fn write_ricm(&mut self, val: u32) {
        let sa = (val >> 16) & 0xFFF;
        let sop = (val >> 6) & 0xF;
        if sa == 0x21 && sop == 1 && ((self.drcm >> 7) & 1) == 0 {
            self.sdevid_sr = 0;
        }
        // Clear bit 31 (busy bit) immediately.
        self.ricm = val & !0x8000_0000;
    }

    /// Write `MCH_DRCM` (`0x1000_F440`).
    #[inline(always)]
    pub fn write_drcm(&mut self, val: u32) {
        self.drcm = val;
    }

    /// Read `MCH_DRCM` (`0x1000_F440`), mutating `sdevid_sr` when probing `SA == 0x40` (`ps2tek`).
    #[inline]
    pub fn read_drcm(&mut self) -> u32 {
        let sop = (self.ricm >> 6) & 0xF;
        if sop == 0 {
            return 0x0D0D_0000;
        }
        if sop == 1 {
            let sa = (self.ricm >> 16) & 0xFFF;
            match sa {
                0x21 => 0x1F,
                0x23 => 0x0D0D,
                0x24 => 0x0090 | ((self.ricm & 0x100) >> 3),
                0x40 => {
                    let x = self.sdevid_sr;
                    if self.sdevid_sr < 2 {
                        self.sdevid_sr += 1;
                    }
                    x
                }
                _ => 0,
            }
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdram_mch_bios_init_sequence_enumerates_two_16mb_modules() {
        let mut mch = RdramMch::new();

        // SOP == 0 returns 0x0D0D_0000.
        mch.write_ricm(0);
        assert_eq!(mch.read_ricm(), 0);
        assert_eq!(mch.read_drcm(), 0x0D0D_0000);

        // Reset sdevid_sr via SA=0x21, SOP=1, drcm bit 7 == 0.
        mch.write_drcm(0);
        mch.write_ricm((0x21 << 16) | (1 << 6) | 0x8000_0000);
        assert_eq!(mch.read_drcm(), 0x1F);

        // Probe SA=0x40 three times: returns 0, 1, 2, and stays at 2 (two 16 MB modules = 32 MB).
        mch.write_ricm((0x40 << 16) | (1 << 6));
        assert_eq!(mch.read_drcm(), 0);
        assert_eq!(mch.read_drcm(), 1);
        assert_eq!(mch.read_drcm(), 2);
        assert_eq!(mch.read_drcm(), 2);
    }
}
