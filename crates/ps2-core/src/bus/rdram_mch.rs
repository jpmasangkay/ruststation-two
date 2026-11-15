//! RDRAM Memory Controller Handshake Registers (`0x1000_F430` / `0x1000_F440`).
//!
//! Primary source reference:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#eerdram` & `https://psi-rockin.github.io/ps2tek/#iomaps`):
//!   * During early BIOS initialization, the Emotion Engine initializes RDRAM via `0x1000_F430`
//!     and `0x1000_F440`.
//!   * Reading `0x1000_F430` always returns `0`.
//!   * Writing `data` to `0x1000_F430`:
//!     - `SA = (data >> 16) & 0xFFF`, `SBC = (data >> 6) & 0xF`.
//!     - If `SA == 0x21 && SBC == 1 && ((MCH_DRD >> 7) & 1) == 0`, resets `rdram_sdevid = 0`.
//!     - Stores `MCH_RICM = data & !0x8000_0000`.
//!   * Writing `data` to `0x1000_F440` stores `MCH_DRD = data`.
//!   * Reading `0x1000_F440`:
//!     - `SOP = (MCH_RICM >> 6) & 0xF`, `SA = (MCH_RICM >> 16) & 0xFFF`.
//!     - If `SOP == 0`:
//!       * `SA == 0x21`: if `rdram_sdevid < 2`, increments `rdram_sdevid` and returns `0x1F`; else `0`.
//!       * `SA == 0x23`: returns `0x0D0D`.
//!       * `SA == 0x24`: returns `0x0090`.
//!       * `SA == 0x40`: returns `MCH_RICM & 0x1F`.
//!       * Otherwise returns `0`.

/// Emotion Engine RDRAM controller (`MCH`, `0x1000_F400..=0x1000_F4F0`) state machine (`ps2tek` `#eerdram`).
// TODO(verify): `ps2tek` `#eerdram` only documents `0x1000_F430` (`MCH_RICM`) and `0x1000_F440` (`MCH_DRD`).
// Remaining 16-byte-aligned slots in `0x1000_F400..=0x1000_F4F0` (such as `0x1000_F410`, accessed by the EE BIOS
// memory initialization routine at `PC = 0x9FC4_11B4`) are backed by 32-bit R/W latches in `regs`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RdramMch {
    pub ricm: u32,
    pub drd: u32,
    pub rdram_sdevid: u32,
    pub regs: [u32; 16],
}

impl RdramMch {
    pub const fn new() -> Self {
        Self {
            ricm: 0,
            drd: 0,
            rdram_sdevid: 0,
            regs: [0; 16],
        }
    }

    /// Read a 32-bit register in `0x1000_F400..=0x1000_F4F0`.
    #[inline]
    pub fn read32(&mut self, paddr: u32) -> u32 {
        match paddr {
            0x1000_F430 => self.read_f430(),
            0x1000_F440 => self.read_f440(),
            _ => {
                let idx = ((paddr - 0x1000_F400) >> 4) as usize & 0xF;
                self.regs[idx]
            }
        }
    }

    /// Write a 32-bit register in `0x1000_F400..=0x1000_F4F0`.
    #[inline]
    pub fn write32(&mut self, paddr: u32, data: u32) {
        match paddr {
            0x1000_F430 => self.write_f430(data),
            0x1000_F440 => self.write_f440(data),
            _ => {
                let idx = ((paddr - 0x1000_F400) >> 4) as usize & 0xF;
                self.regs[idx] = data;
            }
        }
    }

    /// Read `0x1000_F430`: always returns `0` (`ps2tek` `#eerdram`).
    #[inline(always)]
    pub const fn read_f430(&self) -> u32 {
        0
    }

    /// Write `0x1000_F430` (`ps2tek` `#eerdram`).
    #[inline]
    pub fn write_f430(&mut self, data: u32) {
        let sa = (data >> 16) & 0xFFF;
        let sbc = (data >> 6) & 0xF;
        if sa == 0x21 && sbc == 1 && ((self.drd >> 7) & 1) == 0 {
            self.rdram_sdevid = 0;
        }
        self.ricm = data & !0x8000_0000;
    }

    /// Write `0x1000_F440` (`ps2tek` `#eerdram`).
    #[inline(always)]
    pub fn write_f440(&mut self, data: u32) {
        self.drd = data;
    }

    /// Read `0x1000_F440`, mutating `rdram_sdevid` when probing `SA == 0x21` with `SOP == 0` (`ps2tek` `#eerdram`).
    #[inline]
    pub fn read_f440(&mut self) -> u32 {
        let sop = (self.ricm >> 6) & 0xF;
        let sa = (self.ricm >> 16) & 0xFFF;
        if sop == 0 {
            match sa {
                0x21 => {
                    if self.rdram_sdevid < 2 {
                        self.rdram_sdevid += 1;
                        0x1F
                    } else {
                        0
                    }
                }
                0x23 => 0x0D0D,
                0x24 => 0x0090,
                0x40 => self.ricm & 0x1F,
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

        // Reset rdram_sdevid via SA=0x21, SBC=1, drd bit 7 == 0.
        mch.write_f440(0);
        mch.write_f430((0x21 << 16) | (1 << 6) | 0x8000_0000);
        assert_eq!(mch.read_f430(), 0);
        // SOP != 0 returns 0.
        assert_eq!(mch.read_f440(), 0);

        // Probe SA=0x21 with SOP=0 three times: returns 0x1F twice (2 modules), then 0.
        mch.write_f430(0x21 << 16);
        assert_eq!(mch.read_f440(), 0x1F);
        assert_eq!(mch.read_f440(), 0x1F);
        assert_eq!(mch.read_f440(), 0);

        // SA=0x23 -> 0x0D0D, SA=0x24 -> 0x0090, SA=0x40 -> ricm & 0x1F.
        mch.write_f430(0x23 << 16);
        assert_eq!(mch.read_f440(), 0x0D0D);
        mch.write_f430(0x24 << 16);
        assert_eq!(mch.read_f440(), 0x0090);
        mch.write_f430((0x40 << 16) | 0x15);
        assert_eq!(mch.read_f440(), 0x15);
    }
}
