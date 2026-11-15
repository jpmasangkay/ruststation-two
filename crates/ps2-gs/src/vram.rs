//! 4 MB GS local memory (VRAM) and swizzled pixel addressing across all 13 PS2 formats.
//!
//! Swizzle block/column tables and page dimensions are derived from `Play!`
//! (`Source/gs/GsPixelFormats.h` and `Source/gs/GsPixelFormats.cpp`, BSD-2, FETCHED)
//! and cross-checked with `local/ps2tek/index.html` (`FETCHED`) and `ps2sdk`
//! (`common/include/gs_psm.h`, AFL-2.0, FETCHED) — see `THIRD_PARTY_NOTICES.md`
//! and `docs/HARDWARE_NOTES.md`.

/// Total size of GS local memory in bytes (`4 MiB`).
pub const VRAM_SIZE_BYTES: usize = 4 * 1024 * 1024;

/// Total size of GS local memory in 32-bit words (`1,048,576` words).
pub const VRAM_WORDS: usize = VRAM_SIZE_BYTES / 4;

/// Bit mask wrapping a 32-bit word index into 4 MB VRAM (`0x000F_FFFF`).
pub const VRAM_WORD_MASK: u32 = (VRAM_WORDS as u32) - 1;

/// GS pixel storage format (`PSM` field in `FRAME`, `BITBLTBUF`, `TEX0`, `DISPFB`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFormat {
    Psmct32 = 0x00,
    Psmct24 = 0x01,
    Psmct16 = 0x02,
    Psmct16s = 0x0A,
    Psmt8 = 0x13,
    Psmt4 = 0x14,
    Psmt8h = 0x1B,
    Psmt4hl = 0x24,
    Psmt4hh = 0x2C,
    Psmz32 = 0x30,
    Psmz24 = 0x31,
    Psmz16 = 0x32,
    Psmz16s = 0x3A,
}

impl PixelFormat {
    /// Decode a 6-bit GS `PSM` value into [`PixelFormat`].
    #[inline]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw & 0x3F {
            0x00 => Some(Self::Psmct32),
            0x01 => Some(Self::Psmct24),
            0x02 => Some(Self::Psmct16),
            0x0A => Some(Self::Psmct16s),
            0x13 => Some(Self::Psmt8),
            0x14 => Some(Self::Psmt4),
            0x1B => Some(Self::Psmt8h),
            0x24 => Some(Self::Psmt4hl),
            0x2C => Some(Self::Psmt4hh),
            0x30 => Some(Self::Psmz32),
            0x31 => Some(Self::Psmz24),
            0x32 => Some(Self::Psmz16),
            0x3A => Some(Self::Psmz16s),
            _ => None,
        }
    }

    /// Decode a 4-bit `ZBUF.PSM` value (`0=PSMZ32`, `1=PSMZ24`, `2=PSMZ16`, `0xA=PSMZ16S`).
    #[inline]
    pub const fn from_zbuf_raw(raw: u8) -> Option<Self> {
        Self::from_raw((raw & 0x0F) | 0x30)
    }

    /// Bits per pixel carried by transfers and rasterizer outputs for this format.
    #[inline]
    pub const fn bpp(self) -> u8 {
        match self {
            Self::Psmct32 | Self::Psmz32 => 32,
            Self::Psmct24 | Self::Psmz24 => 24,
            Self::Psmct16 | Self::Psmct16s | Self::Psmz16 | Self::Psmz16s => 16,
            Self::Psmt8 | Self::Psmt8h => 8,
            Self::Psmt4 | Self::Psmt4hl | Self::Psmt4hh => 4,
        }
    }

    /// Page dimensions `(width, height)` in pixels (`8192` bytes per page).
    #[inline]
    pub const fn page_size(self) -> (u32, u32) {
        match self {
            Self::Psmct32
            | Self::Psmct24
            | Self::Psmt8h
            | Self::Psmt4hl
            | Self::Psmt4hh
            | Self::Psmz32
            | Self::Psmz24 => (64, 32),
            Self::Psmct16 | Self::Psmct16s | Self::Psmz16 | Self::Psmz16s => (64, 64),
            Self::Psmt8 => (128, 64),
            Self::Psmt4 => (128, 128),
        }
    }
}

/// Physical location of a single pixel inside [`GsVram::words`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PixelLocation {
    pub word_index: usize,
    pub bit_shift: u8,
    pub mask: u32,
}

// Swizzle tables derived from `Play!` (`Source/gs/GsPixelFormats.h`, BSD-2, FETCHED).

#[rustfmt::skip]
const BLOCK_32: [[u16; 8]; 4] = [
    [ 0,  1,  4,  5, 16, 17, 20, 21],
    [ 2,  3,  6,  7, 18, 19, 22, 23],
    [ 8,  9, 12, 13, 24, 25, 28, 29],
    [10, 11, 14, 15, 26, 27, 30, 31],
];

#[rustfmt::skip]
const BLOCK_Z32: [[u16; 8]; 4] = [
    [24, 25, 28, 29,  8,  9, 12, 13],
    [26, 27, 30, 31, 10, 11, 14, 15],
    [16, 17, 20, 21,  0,  1,  4,  5],
    [18, 19, 22, 23,  2,  3,  6,  7],
];

#[rustfmt::skip]
const COL_32: [[u16; 8]; 2] = [
    [0, 1, 4, 5,  8,  9, 12, 13],
    [2, 3, 6, 7, 10, 11, 14, 15],
];

#[rustfmt::skip]
const BLOCK_16: [[u16; 4]; 8] = [
    [ 0,  2,  8, 10],
    [ 1,  3,  9, 11],
    [ 4,  6, 12, 14],
    [ 5,  7, 13, 15],
    [16, 18, 24, 26],
    [17, 19, 25, 27],
    [20, 22, 28, 30],
    [21, 23, 29, 31],
];

#[rustfmt::skip]
const BLOCK_16S: [[u16; 4]; 8] = [
    [ 0,  2, 16, 18],
    [ 1,  3, 17, 19],
    [ 8, 10, 24, 26],
    [ 9, 11, 25, 27],
    [ 4,  6, 20, 22],
    [ 5,  7, 21, 23],
    [12, 14, 28, 30],
    [13, 15, 29, 31],
];

#[rustfmt::skip]
const BLOCK_Z16: [[u16; 4]; 8] = [
    [24, 26, 16, 18],
    [25, 27, 17, 19],
    [28, 30, 20, 22],
    [29, 31, 21, 23],
    [ 8, 10,  0,  2],
    [ 9, 11,  1,  3],
    [12, 14,  4,  6],
    [13, 15,  5,  7],
];

#[rustfmt::skip]
const BLOCK_Z16S: [[u16; 4]; 8] = [
    [24, 26,  8, 10],
    [25, 27,  9, 11],
    [16, 18,  0,  2],
    [17, 19,  1,  3],
    [28, 30, 12, 14],
    [29, 31, 13, 15],
    [20, 22,  4,  6],
    [21, 23,  5,  7],
];

#[rustfmt::skip]
const COL_16: [[u16; 16]; 2] = [
    [0, 2, 8, 10, 16, 18, 24, 26, 1, 3,  9, 11, 17, 19, 25, 27],
    [4, 6, 12, 14, 20, 22, 28, 30, 5, 7, 13, 15, 21, 23, 29, 31],
];

#[rustfmt::skip]
const T8_WORD_TABLE: [[[u16; 8]; 2]; 2] = [
    [
        [0, 1, 4, 5,  8,  9, 12, 13],
        [2, 3, 6, 7, 10, 11, 14, 15],
    ],
    [
        [ 8,  9, 12, 13, 0, 1, 4, 5],
        [10, 11, 14, 15, 2, 3, 6, 7],
    ],
];

const fn build_lut_32(blocks: &[[u16; 8]; 4]) -> [u16; 2048] {
    let mut lut = [0u16; 2048];
    let mut y = 0usize;
    while y < 32 {
        let mut x = 0usize;
        while x < 64 {
            let block = blocks[y / 8][x / 8];
            let bx = x % 8;
            let by = y % 8;
            let col = (by / 2) as u16;
            let word = COL_32[by % 2][bx];
            lut[y * 64 + x] = block * 64 + col * 16 + word;
            x += 1;
        }
        y += 1;
    }
    lut
}

const fn build_lut_16(blocks: &[[u16; 4]; 8]) -> [u16; 4096] {
    let mut lut = [0u16; 4096];
    let mut y = 0usize;
    while y < 64 {
        let mut x = 0usize;
        while x < 64 {
            let block = blocks[y / 8][x / 16];
            let bx = x % 16;
            let by = y % 8;
            let col = (by / 2) as u16;
            let hw = COL_16[by % 2][bx];
            lut[y * 64 + x] = block * 128 + col * 32 + hw;
            x += 1;
        }
        y += 1;
    }
    lut
}

const fn build_lut_t8() -> [u16; 8192] {
    let mut lut = [0u16; 8192];
    let mut y = 0usize;
    while y < 64 {
        let mut x = 0usize;
        while x < 128 {
            let block = BLOCK_32[y / 16][x / 16];
            let bx = x % 16;
            let by = y % 16;
            let col = (by / 4) as u16;
            let cy = by % 4;
            let tbl = ((cy & 2) >> 1) ^ ((col as usize) & 1);
            let byte = (((bx & 8) >> 2) + ((cy & 2) >> 1)) as u16;
            let word = T8_WORD_TABLE[tbl][cy & 1][bx & 7];
            lut[y * 128 + x] = block * 256 + col * 64 + word * 4 + byte;
            x += 1;
        }
        y += 1;
    }
    lut
}

const fn build_lut_t4() -> [u16; 16384] {
    let mut lut = [0u16; 16384];
    let mut y = 0usize;
    while y < 128 {
        let mut x = 0usize;
        while x < 128 {
            let block = BLOCK_16[y / 16][x / 32];
            let bx = x % 32;
            let by = y % 16;
            let col = (by / 4) as u16;
            let cy = by % 4;
            let tbl = ((cy & 2) >> 1) ^ ((col as usize) & 1);
            let nibble = (((bx & 0x18) + ((cy & 2) << 1)) >> 2) as u16;
            let word = T8_WORD_TABLE[tbl][cy & 1][bx & 7];
            lut[y * 128 + x] = block * 512 + col * 128 + word * 8 + nibble;
            x += 1;
        }
        y += 1;
    }
    lut
}

static LUT_CT32: [u16; 2048] = build_lut_32(&BLOCK_32);
static LUT_Z32: [u16; 2048] = build_lut_32(&BLOCK_Z32);
static LUT_CT16: [u16; 4096] = build_lut_16(&BLOCK_16);
static LUT_CT16S: [u16; 4096] = build_lut_16(&BLOCK_16S);
static LUT_Z16: [u16; 4096] = build_lut_16(&BLOCK_Z16);
static LUT_Z16S: [u16; 4096] = build_lut_16(&BLOCK_Z16S);
static LUT_T8: [u16; 8192] = build_lut_t8();
static LUT_T4: [u16; 16384] = build_lut_t4();

/// 4 MB GS local memory backed by 32-bit little-endian words (`[u32; 1_048_576]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GsVram {
    pub words: Box<[u32]>,
}

impl Default for GsVram {
    fn default() -> Self {
        Self::new()
    }
}

impl GsVram {
    /// Allocate zero-initialized 4 MB GS local memory once.
    pub fn new() -> Self {
        Self {
            words: vec![0u32; VRAM_WORDS].into_boxed_slice(),
        }
    }

    /// Zero all 4 MB of VRAM in place without reallocating.
    pub fn clear(&mut self) {
        self.words.fill(0);
    }

    /// Compute the word address, bit shift, and field mask for pixel `(x, y)` in a buffer at
    /// `bp_blocks` (`256`-byte / `64`-word blocks) with buffer width `bw` (`64`-pixel units).
    ///
    /// Coordinates wrap modulo `2048` on caller/transfer paths, and VRAM word indices wrap
    /// modulo `1,048,576` (`VRAM_WORD_MASK`).
    #[inline]
    pub fn pixel_location(
        format: PixelFormat,
        bp_blocks: u32,
        bw: u32,
        x: u32,
        y: u32,
    ) -> PixelLocation {
        let (page_w, page_h) = format.page_size();
        // `G-001` (`docs/GS_GUESSES.md`): clamp `bw` so `bw == 0` (and `bw == 1` under 128-px pages)
        // never collapses rows to `pages_per_row == 0`.
        let bw_pixels = bw.max(1) * 64;
        let pages_per_row = (bw_pixels / page_w).max(1);
        let page_num = (x / page_w) + (y / page_h) * pages_per_row;
        let page_base_word = bp_blocks
            .wrapping_mul(64)
            .wrapping_add(page_num.wrapping_mul(2048));
        let px = (x % page_w) as usize;
        let py = (y % page_h) as usize;

        match format {
            PixelFormat::Psmct32 | PixelFormat::Psmz32 => {
                let lut = if matches!(format, PixelFormat::Psmct32) {
                    &LUT_CT32
                } else {
                    &LUT_Z32
                };
                let word_off = u32::from(lut[py * 64 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(word_off) & VRAM_WORD_MASK) as usize,
                    bit_shift: 0,
                    mask: 0xFFFF_FFFF,
                }
            }
            PixelFormat::Psmct24 | PixelFormat::Psmz24 => {
                let lut = if matches!(format, PixelFormat::Psmct24) {
                    &LUT_CT32
                } else {
                    &LUT_Z32
                };
                let word_off = u32::from(lut[py * 64 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(word_off) & VRAM_WORD_MASK) as usize,
                    bit_shift: 0,
                    mask: 0x00FF_FFFF,
                }
            }
            PixelFormat::Psmt8h => {
                let word_off = u32::from(LUT_CT32[py * 64 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(word_off) & VRAM_WORD_MASK) as usize,
                    bit_shift: 24,
                    mask: 0xFF,
                }
            }
            PixelFormat::Psmt4hl => {
                let word_off = u32::from(LUT_CT32[py * 64 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(word_off) & VRAM_WORD_MASK) as usize,
                    bit_shift: 24,
                    mask: 0xF,
                }
            }
            PixelFormat::Psmt4hh => {
                let word_off = u32::from(LUT_CT32[py * 64 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(word_off) & VRAM_WORD_MASK) as usize,
                    bit_shift: 28,
                    mask: 0xF,
                }
            }
            PixelFormat::Psmct16
            | PixelFormat::Psmct16s
            | PixelFormat::Psmz16
            | PixelFormat::Psmz16s => {
                let lut = match format {
                    PixelFormat::Psmct16 => &LUT_CT16,
                    PixelFormat::Psmct16s => &LUT_CT16S,
                    PixelFormat::Psmz16 => &LUT_Z16,
                    _ => &LUT_Z16S,
                };
                let hw_off = u32::from(lut[py * 64 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(hw_off >> 1) & VRAM_WORD_MASK)
                        as usize,
                    bit_shift: ((hw_off & 1) * 16) as u8,
                    mask: 0xFFFF,
                }
            }
            PixelFormat::Psmt8 => {
                let byte_off = u32::from(LUT_T8[py * 128 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(byte_off >> 2) & VRAM_WORD_MASK)
                        as usize,
                    bit_shift: ((byte_off & 3) * 8) as u8,
                    mask: 0xFF,
                }
            }
            PixelFormat::Psmt4 => {
                let nibble_off = u32::from(LUT_T4[py * 128 + px]);
                PixelLocation {
                    word_index: (page_base_word.wrapping_add(nibble_off >> 3) & VRAM_WORD_MASK)
                        as usize,
                    bit_shift: ((nibble_off & 7) * 4) as u8,
                    mask: 0xF,
                }
            }
        }
    }

    /// Read a raw unexpanded pixel value (low bits aligned to bit 0) at `(x, y)`.
    #[inline]
    pub fn read_pixel_raw(
        &self,
        format: PixelFormat,
        bp_blocks: u32,
        bw: u32,
        x: u32,
        y: u32,
    ) -> u32 {
        let loc = Self::pixel_location(format, bp_blocks, bw, x, y);
        (self.words[loc.word_index] >> loc.bit_shift) & loc.mask
    }

    /// Write a raw pixel value (low bits aligned to bit 0) at `(x, y)`, preserving bits outside
    /// `format`'s bit range (e.g., bits `24..=31` during `PSMCT24` writes and neighboring nibbles/bytes).
    #[inline]
    pub fn write_pixel_raw(
        &mut self,
        format: PixelFormat,
        bp_blocks: u32,
        bw: u32,
        x: u32,
        y: u32,
        value: u32,
    ) {
        let loc = Self::pixel_location(format, bp_blocks, bw, x, y);
        let clear_mask = !(loc.mask << loc.bit_shift);
        let word = self.words[loc.word_index];
        self.words[loc.word_index] = (word & clear_mask) | ((value & loc.mask) << loc.bit_shift);
    }

    /// Convenience helper using `FRAME.FBP` / `DISPFB.FBP` (`2048`-word / `8192`-byte page index).
    #[inline]
    pub fn read_page_pixel_raw(
        &self,
        format: PixelFormat,
        fbp: u32,
        fbw: u32,
        x: u32,
        y: u32,
    ) -> u32 {
        self.read_pixel_raw(format, fbp * 32, fbw, x, y)
    }

    /// Convenience helper using `FRAME.FBP` (`2048`-word / `8192`-byte page index).
    #[inline]
    pub fn write_page_pixel_raw(
        &mut self,
        format: PixelFormat,
        fbp: u32,
        fbw: u32,
        x: u32,
        y: u32,
        value: u32,
    ) {
        self.write_pixel_raw(format, fbp * 32, fbw, x, y, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_page_bijective(format: PixelFormat) {
        let (pw, ph) = format.page_size();
        let total = (pw * ph) as usize;
        let mut seen = vec![false; total];

        for y in 0..ph {
            for x in 0..pw {
                let loc = GsVram::pixel_location(format, 0, 2, x, y);
                let unit = match format.bpp() {
                    32 | 24 => loc.word_index,
                    16 => (loc.word_index * 2) + usize::from(loc.bit_shift / 16),
                    8 => (loc.word_index * 4) + usize::from(loc.bit_shift / 8),
                    4 => (loc.word_index * 8) + usize::from(loc.bit_shift / 4),
                    _ => unreachable!(),
                };
                assert!(
                    unit < total,
                    "{format:?} unit {unit} >= {total} at ({x}, {y})"
                );
                assert!(
                    !seen[unit],
                    "{format:?} collision at unit {unit} ({x}, {y})"
                );
                seen[unit] = true;
            }
        }
        assert!(
            seen.into_iter().all(|v| v),
            "{format:?} missed slots in page"
        );
    }

    #[test]
    fn page_swizzle_is_100_percent_bijective_for_all_base_formats() {
        for fmt in [
            PixelFormat::Psmct32,
            PixelFormat::Psmz32,
            PixelFormat::Psmct16,
            PixelFormat::Psmct16s,
            PixelFormat::Psmz16,
            PixelFormat::Psmz16s,
            PixelFormat::Psmt8,
            PixelFormat::Psmt4,
        ] {
            assert_page_bijective(fmt);
        }
    }

    #[test]
    fn hand_computed_swizzle_addresses_match_block_and_column_tables() {
        // PSMCT32 at (x=9, y=3): block (bx_idx=1, by_idx=0) => BLOCK_32[0][1] = 1 -> word 64;
        // inside block: bx=1, by=3 => col=1 -> +16; COL_32[1][1] = 3 -> word 64+16+3 = 83.
        let loc32 = GsVram::pixel_location(PixelFormat::Psmct32, 0, 1, 9, 3);
        assert_eq!(loc32.word_index, 83);
        assert_eq!(loc32.bit_shift, 0);

        // PSMZ32 at (x=0, y=0): BLOCK_Z32[0][0] = 24 -> word 24*64 = 1536.
        let loc_z32 = GsVram::pixel_location(PixelFormat::Psmz32, 0, 1, 0, 0);
        assert_eq!(loc_z32.word_index, 24 * 64);

        // PSMCT16S at (x=32, y=8): block (bx_idx=2, by_idx=1) => BLOCK_16S[1][2] = 17 -> hw 17*128 = 2176 -> word 1088.
        let loc_16s = GsVram::pixel_location(PixelFormat::Psmct16s, 0, 1, 32, 8);
        assert_eq!(loc_16s.word_index, 17 * 64);
        assert_eq!(loc_16s.bit_shift, 0);

        // Second page on row 1 (bw=2 => 128 px wide, 2 pages/row for PSMCT32): (x=65, y=34) ->
        // page_num = 1 + 1*2 = 3 -> page_base_word = 3 * 2048 = 6144;
        // in page: (1, 2) -> block 0, col 1, COL_32[0][1] = 1 -> word = 6144 + 16 + 1 = 6161.
        let loc_p3 = GsVram::pixel_location(PixelFormat::Psmct32, 0, 2, 65, 34);
        assert_eq!(loc_p3.word_index, 6161);
    }

    #[test]
    fn psm24_and_high_upper_byte_formats_preserve_other_bitfields() {
        let mut vram = GsVram::new();
        // Write a full 32-bit word via PSMCT32 first.
        vram.write_pixel_raw(PixelFormat::Psmct32, 0, 1, 5, 7, 0xAB12_3456);
        assert_eq!(vram.read_pixel_raw(PixelFormat::Psmt8h, 0, 1, 5, 7), 0xAB);
        assert_eq!(vram.read_pixel_raw(PixelFormat::Psmt4hl, 0, 1, 5, 7), 0xB);
        assert_eq!(vram.read_pixel_raw(PixelFormat::Psmt4hh, 0, 1, 5, 7), 0xA);

        // Writing PSMCT24 must preserve bits 24..31 (0xAB).
        vram.write_pixel_raw(PixelFormat::Psmct24, 0, 1, 5, 7, 0xFF99_8877);
        assert_eq!(
            vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 5, 7),
            0xAB99_8877
        );
        assert_eq!(
            vram.read_pixel_raw(PixelFormat::Psmct24, 0, 1, 5, 7),
            0x0099_8877
        );

        // Writing PSMT4HL updates bits 24..27 only; PSMT4HH updates bits 28..31 only.
        vram.write_pixel_raw(PixelFormat::Psmt4hl, 0, 1, 5, 7, 0x3);
        assert_eq!(
            vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 5, 7),
            0xA399_8877
        );
        vram.write_pixel_raw(PixelFormat::Psmt4hh, 0, 1, 5, 7, 0x7);
        assert_eq!(
            vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 5, 7),
            0x7399_8877
        );

        // Writing PSMT8H updates bits 24..31 while preserving low 24 RGB bits.
        vram.write_pixel_raw(PixelFormat::Psmt8h, 0, 1, 5, 7, 0xE1);
        assert_eq!(
            vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 5, 7),
            0xE199_8877
        );
    }

    #[test]
    fn vram_wraps_at_4mb_boundary_and_sub_page_block_offsets() {
        let mut vram = GsVram::new();
        // Last block pointer inside 4 MB is 16383 (0x3FFF * 64 words = 0xFFFC0); adding page 1 (+2048) wraps!
        vram.write_pixel_raw(PixelFormat::Psmct32, 16384, 1, 0, 0, 0xDEAD_BEEF);
        assert_eq!(
            vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0),
            0xDEAD_BEEF
        );

        // Block-aligned base pointer (bp = 1 => +64 words) isolates sub-page CLUTs cleanly.
        vram.write_pixel_raw(PixelFormat::Psmct32, 1, 1, 0, 0, 0x1122_3344);
        assert_eq!(vram.words[64], 0x1122_3344);
    }
}
