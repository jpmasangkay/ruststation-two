//! Internal and privileged GS register definitions and typed field decoders.
//!
//! Bit offsets follow `local/ps2tek/index.html` (`#gs`), corroborated and completed where `ps2tek`
//! is silent using `Play!` (`Source/gs/GSHandler.h`, BSD-2) and `ps2sdk` (`common/include/gs_gp.h`,
//! `common/include/gs_privileged.h`, `ee/libgs/include/libgs.h`, AFL-2.0) — see `THIRD_PARTY_NOTICES.md`
//! and `docs/HARDWARE_NOTES.md`.

/// Internal GS register addresses (`0x00..=0x62`, accessible via GIF).
pub mod gs_reg {
    pub const PRIM: u8 = 0x00;
    pub const RGBAQ: u8 = 0x01;
    pub const ST: u8 = 0x02;
    pub const UV: u8 = 0x03;
    pub const XYZF2: u8 = 0x04;
    pub const XYZ2: u8 = 0x05;
    pub const TEX0_1: u8 = 0x06;
    pub const TEX0_2: u8 = 0x07;
    pub const CLAMP_1: u8 = 0x08;
    pub const CLAMP_2: u8 = 0x09;
    pub const FOG: u8 = 0x0A;
    pub const XYZF3: u8 = 0x0C;
    pub const XYZ3: u8 = 0x0D;
    pub const TEX1_1: u8 = 0x14;
    pub const TEX1_2: u8 = 0x15;
    pub const TEX2_1: u8 = 0x16;
    pub const TEX2_2: u8 = 0x17;
    pub const XYOFFSET_1: u8 = 0x18;
    pub const XYOFFSET_2: u8 = 0x19;
    pub const PRMODECONT: u8 = 0x1A;
    pub const PRMODE: u8 = 0x1B;
    pub const TEXCLUT: u8 = 0x1C;
    pub const SCANMSK: u8 = 0x22;
    pub const MIPTBP1_1: u8 = 0x34;
    pub const MIPTBP1_2: u8 = 0x35;
    pub const MIPTBP2_1: u8 = 0x36;
    pub const MIPTBP2_2: u8 = 0x37;
    pub const TEXA: u8 = 0x3B;
    pub const FOGCOL: u8 = 0x3D;
    pub const TEXFLUSH: u8 = 0x3F;
    pub const SCISSOR_1: u8 = 0x40;
    pub const SCISSOR_2: u8 = 0x41;
    pub const ALPHA_1: u8 = 0x42;
    pub const ALPHA_2: u8 = 0x43;
    pub const DIMX: u8 = 0x44;
    pub const DTHE: u8 = 0x45;
    pub const COLCLAMP: u8 = 0x46;
    pub const TEST_1: u8 = 0x47;
    pub const TEST_2: u8 = 0x48;
    pub const PABE: u8 = 0x49;
    pub const FBA_1: u8 = 0x4A;
    pub const FBA_2: u8 = 0x4B;
    pub const FRAME_1: u8 = 0x4C;
    pub const FRAME_2: u8 = 0x4D;
    pub const ZBUF_1: u8 = 0x4E;
    pub const ZBUF_2: u8 = 0x4F;
    pub const BITBLTBUF: u8 = 0x50;
    pub const TRXPOS: u8 = 0x51;
    pub const TRXREG: u8 = 0x52;
    pub const TRXDIR: u8 = 0x53;
    pub const HWREG: u8 = 0x54;
    pub const SIGNAL: u8 = 0x60;
    pub const FINISH: u8 = 0x61;
    pub const LABEL: u8 = 0x62;
    pub const NOP: u8 = 0x7F;
}

/// Mask of `TEX0` bits overwritten when writing `TEX2_1`/`TEX2_2` (`PSM`, `CBP`, `CPSM`, `CSM`, `CSA`, `CLD`).
pub const TEX2_TO_TEX0_MASK: u64 = 0xFFFF_FFE0_03F0_0000;

/// GS primitive type (`PRIM[0..=2]`, `ps2tek` `#gsprimitives`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PrimitiveType {
    Point = 0,
    Line = 1,
    LineStrip = 2,
    Triangle = 3,
    TriangleStrip = 4,
    TriangleFan = 5,
    Sprite = 6,
    Reserved = 7,
}

impl PrimitiveType {
    #[inline]
    pub const fn from_raw(raw: u8) -> Self {
        match raw & 0x7 {
            0 => Self::Point,
            1 => Self::Line,
            2 => Self::LineStrip,
            3 => Self::Triangle,
            4 => Self::TriangleStrip,
            5 => Self::TriangleFan,
            6 => Self::Sprite,
            _ => Self::Reserved,
        }
    }
}

/// Decoded `PRIM` (`0x00`) / `PRMODE` (`0x1B`) register attributes (`ps2tek` `#gsprimitives`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PrimReg(pub u64);

impl PrimReg {
    #[inline]
    pub const fn prim_type(self) -> PrimitiveType {
        PrimitiveType::from_raw((self.0 & 0x7) as u8)
    }
    #[inline]
    pub const fn iip(self) -> bool {
        (self.0 & (1 << 3)) != 0
    }
    #[inline]
    pub const fn tme(self) -> bool {
        (self.0 & (1 << 4)) != 0
    }
    #[inline]
    pub const fn fge(self) -> bool {
        (self.0 & (1 << 5)) != 0
    }
    #[inline]
    pub const fn abe(self) -> bool {
        (self.0 & (1 << 6)) != 0
    }
    #[inline]
    pub const fn aa1(self) -> bool {
        (self.0 & (1 << 7)) != 0
    }
    #[inline]
    pub const fn fst(self) -> bool {
        (self.0 & (1 << 8)) != 0
    }
    #[inline]
    pub const fn ctxt(self) -> usize {
        ((self.0 >> 9) & 1) as usize
    }
    #[inline]
    pub const fn fix(self) -> bool {
        (self.0 & (1 << 10)) != 0
    }
    #[inline]
    pub const fn with_attributes_from(self, prmode: u64) -> Self {
        const ATTR_MASK: u64 = 0x7F8; // bits 3..=10
        Self((self.0 & !ATTR_MASK) | (prmode & ATTR_MASK))
    }
}

/// Decoded `RGBAQ` (`0x01`, `ps2tek` `#gsvertexattributes`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RgbaqReg(pub u64);

impl RgbaqReg {
    #[inline]
    pub const fn r(self) -> u8 {
        self.0 as u8
    }
    #[inline]
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    #[inline]
    pub const fn b(self) -> u8 {
        (self.0 >> 16) as u8
    }
    #[inline]
    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
    #[inline]
    pub const fn q_bits(self) -> u32 {
        (self.0 >> 32) as u32
    }
    #[inline]
    pub fn q(self) -> f32 {
        f32::from_bits(self.q_bits())
    }
}

/// Decoded `ST` (`0x02`, `ps2tek` `#gstextures`: lower 8 mantissa bits ignored).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StReg(pub u64);

impl StReg {
    #[inline]
    pub const fn s_bits(self) -> u32 {
        (self.0 as u32) & !0xFF
    }
    #[inline]
    pub const fn t_bits(self) -> u32 {
        ((self.0 >> 32) as u32) & !0xFF
    }
    #[inline]
    pub fn s(self) -> f32 {
        f32::from_bits(self.s_bits())
    }
    #[inline]
    pub fn t(self) -> f32 {
        f32::from_bits(self.t_bits())
    }
}

/// Decoded `UV` (`0x03`, `ps2tek` `#gstextures` + `Play! GSHandler.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct UvReg(pub u64);

impl UvReg {
    #[inline]
    pub const fn u_raw(self) -> u16 {
        (self.0 & 0x3FFF) as u16
    }
    #[inline]
    pub const fn v_raw(self) -> u16 {
        ((self.0 >> 16) & 0x3FFF) as u16
    }
}

/// Decoded `XYZF2/3` (`0x04`/`0x0C`) or `XYZ2/3` (`0x05`/`0x0D`) vertex coordinates (`ps2tek`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct XyzReg {
    pub x_raw: u16,
    pub y_raw: u16,
    pub z: u32,
    pub fog: Option<u8>,
}

impl XyzReg {
    #[inline]
    pub const fn from_xyzf(data: u64) -> Self {
        Self {
            x_raw: data as u16,
            y_raw: (data >> 16) as u16,
            z: ((data >> 32) & 0x00FF_FFFF) as u32,
            fog: Some((data >> 56) as u8),
        }
    }

    #[inline]
    pub const fn from_xyz(data: u64) -> Self {
        Self {
            x_raw: data as u16,
            y_raw: (data >> 16) as u16,
            z: (data >> 32) as u32,
            fog: None,
        }
    }
}

/// Decoded `TEX0_1/2` (`0x06`/`0x07`, `ps2tek` `#gstextures`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Tex0Reg(pub u64);

impl Tex0Reg {
    #[inline]
    pub const fn tbp0(self) -> u32 {
        (self.0 & 0x3FFF) as u32
    }
    #[inline]
    pub const fn tbp0_bytes(self) -> u32 {
        self.tbp0() * 256
    }
    #[inline]
    pub const fn tbw(self) -> u32 {
        ((self.0 >> 14) & 0x3F) as u32
    }
    #[inline]
    pub const fn tbw_pixels(self) -> u32 {
        self.tbw() * 64
    }
    #[inline]
    pub const fn psm_raw(self) -> u8 {
        ((self.0 >> 20) & 0x3F) as u8
    }
    #[inline]
    pub const fn tw_log2(self) -> u32 {
        ((self.0 >> 26) & 0xF) as u32
    }
    #[inline]
    pub const fn th_log2(self) -> u32 {
        ((self.0 >> 30) & 0xF) as u32
    }
    #[inline]
    pub const fn width(self) -> u32 {
        let shift = self.tw_log2();
        if shift >= 10 {
            1024
        } else {
            1u32 << shift
        }
    }
    #[inline]
    pub const fn height(self) -> u32 {
        let shift = self.th_log2();
        if shift >= 10 {
            1024
        } else {
            1u32 << shift
        }
    }
    #[inline]
    pub const fn tcc(self) -> bool {
        ((self.0 >> 34) & 1) != 0
    }
    #[inline]
    pub const fn tfx(self) -> u8 {
        ((self.0 >> 35) & 0x3) as u8
    }
    #[inline]
    pub const fn cbp(self) -> u32 {
        ((self.0 >> 37) & 0x3FFF) as u32
    }
    #[inline]
    pub const fn cbp_bytes(self) -> u32 {
        self.cbp() * 256
    }
    #[inline]
    pub const fn cpsm_raw(self) -> u8 {
        ((self.0 >> 51) & 0xF) as u8
    }
    #[inline]
    pub const fn csm2(self) -> bool {
        ((self.0 >> 55) & 1) != 0
    }
    #[inline]
    pub const fn csa(self) -> u8 {
        ((self.0 >> 56) & 0x1F) as u8
    }
    #[inline]
    pub const fn cld(self) -> u8 {
        ((self.0 >> 61) & 0x7) as u8
    }
}

/// Decoded `CLAMP_1/2` (`0x08`/`0x09`, `Play! GSHandler.h` BSD-2 / `ps2sdk gs_gp.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ClampReg(pub u64);

impl ClampReg {
    #[inline]
    pub const fn wms(self) -> u8 {
        (self.0 & 0x3) as u8
    }
    #[inline]
    pub const fn wmt(self) -> u8 {
        ((self.0 >> 2) & 0x3) as u8
    }
    #[inline]
    pub const fn minu(self) -> u16 {
        ((self.0 >> 4) & 0x3FF) as u16
    }
    #[inline]
    pub const fn maxu(self) -> u16 {
        ((self.0 >> 14) & 0x3FF) as u16
    }
    #[inline]
    pub const fn minv(self) -> u16 {
        ((self.0 >> 24) & 0x3FF) as u16
    }
    #[inline]
    pub const fn maxv(self) -> u16 {
        ((self.0 >> 34) & 0x3FF) as u16
    }
}

/// Decoded `FOG` (`0x0A`, bits `56..=63` per `Play! GSHandler.cpp` & `ps2sdk gs_gp.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FogReg(pub u64);

impl FogReg {
    #[inline]
    pub const fn f(self) -> u8 {
        (self.0 >> 56) as u8
    }
}

/// Decoded `TEX1_1/2` (`0x14`/`0x15`, `ps2tek` `#gstextures`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Tex1Reg(pub u64);

impl Tex1Reg {
    #[inline]
    pub const fn lcm(self) -> u8 {
        (self.0 & 1) as u8
    }
    #[inline]
    pub const fn mxl(self) -> u8 {
        ((self.0 >> 2) & 0x7) as u8
    }
    #[inline]
    pub const fn mmag(self) -> u8 {
        ((self.0 >> 5) & 1) as u8
    }
    #[inline]
    pub const fn mmin(self) -> u8 {
        ((self.0 >> 6) & 0x7) as u8
    }
    #[inline]
    pub const fn mtba(self) -> bool {
        ((self.0 >> 9) & 1) != 0
    }
    #[inline]
    pub const fn l(self) -> u8 {
        ((self.0 >> 19) & 0x3) as u8
    }
    #[inline]
    pub const fn k_raw(self) -> i16 {
        let raw = ((self.0 >> 32) & 0xFFF) as u16;
        ((raw << 4) as i16) >> 4
    }
}

/// Decoded `XYOFFSET_1/2` (`0x18`/`0x19`, `ps2tek` `#gsvertexattributes`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct XyOffsetReg(pub u64);

impl XyOffsetReg {
    #[inline]
    pub const fn ofx_raw(self) -> u16 {
        self.0 as u16
    }
    #[inline]
    pub const fn ofy_raw(self) -> u16 {
        (self.0 >> 32) as u16
    }
}

/// Decoded `TEXCLUT` (`0x1C`, `Play! GSHandler.h` BSD-2 / `ps2sdk libgs.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TexClutReg(pub u64);

impl TexClutReg {
    #[inline]
    pub const fn cbw(self) -> u32 {
        (self.0 & 0x3F) as u32
    }
    #[inline]
    pub const fn cbw_pixels(self) -> u32 {
        self.cbw() * 64
    }
    #[inline]
    pub const fn cou(self) -> u32 {
        ((self.0 >> 6) & 0x3F) as u32
    }
    #[inline]
    pub const fn cou_pixels(self) -> u32 {
        self.cou() * 16
    }
    #[inline]
    pub const fn cov(self) -> u32 {
        ((self.0 >> 12) & 0x3FF) as u32
    }
}

/// Decoded `SCANMSK` (`0x22`, `ps2sdk gs_gp.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanMskReg(pub u64);

impl ScanMskReg {
    #[inline]
    pub const fn msk(self) -> u8 {
        (self.0 & 0x3) as u8
    }
}

/// Decoded `MIPTBP1_1/2` (`0x34`/`0x35`) and `MIPTBP2_1/2` (`0x36`/`0x37`, `Play! GSHandler.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MipTbpReg(pub u64);

impl MipTbpReg {
    #[inline]
    pub const fn tbp(self, slot: usize) -> u32 {
        let shift = match slot {
            0 => 0,
            1 => 20,
            _ => 40,
        };
        ((self.0 >> shift) & 0x3FFF) as u32
    }
    #[inline]
    pub const fn tbw(self, slot: usize) -> u32 {
        let shift = match slot {
            0 => 14,
            1 => 34,
            _ => 54,
        };
        ((self.0 >> shift) & 0x3F) as u32
    }
}

/// Decoded `TEXA` (`0x3B`, `Play! GSHandler.h` BSD-2 / `ps2sdk gs_gp.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TexAReg(pub u64);

impl TexAReg {
    #[inline]
    pub const fn ta0(self) -> u8 {
        self.0 as u8
    }
    #[inline]
    pub const fn aem(self) -> bool {
        ((self.0 >> 15) & 1) != 0
    }
    #[inline]
    pub const fn ta1(self) -> u8 {
        (self.0 >> 32) as u8
    }
}

/// Decoded `FOGCOL` (`0x3D`, `ps2tek` `#gsfog`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FogColReg(pub u64);

impl FogColReg {
    #[inline]
    pub const fn fcr(self) -> u8 {
        self.0 as u8
    }
    #[inline]
    pub const fn fcg(self) -> u8 {
        (self.0 >> 8) as u8
    }
    #[inline]
    pub const fn fcb(self) -> u8 {
        (self.0 >> 16) as u8
    }
}

/// Decoded `SCISSOR_1/2` (`0x40`/`0x41`, `ps2tek` `#gstestsandpixelcontrol`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScissorReg(pub u64);

impl ScissorReg {
    #[inline]
    pub const fn scax0(self) -> u16 {
        (self.0 & 0x7FF) as u16
    }
    #[inline]
    pub const fn scax1(self) -> u16 {
        ((self.0 >> 16) & 0x7FF) as u16
    }
    #[inline]
    pub const fn scay0(self) -> u16 {
        ((self.0 >> 32) & 0x7FF) as u16
    }
    #[inline]
    pub const fn scay1(self) -> u16 {
        ((self.0 >> 48) & 0x7FF) as u16
    }
}

/// Decoded `ALPHA_1/2` (`0x42`/`0x43`).
///
/// Uses `FIX` at bits `32..=39` (`Play! GSHandler.h` BSD-2, `ps2sdk gs_gp.h` & `libgs.h`)
/// rather than the `8-15` typo in `ps2tek` `#gsalphablending`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AlphaReg(pub u64);

impl AlphaReg {
    #[inline]
    pub const fn a(self) -> u8 {
        (self.0 & 0x3) as u8
    }
    #[inline]
    pub const fn b(self) -> u8 {
        ((self.0 >> 2) & 0x3) as u8
    }
    #[inline]
    pub const fn c(self) -> u8 {
        ((self.0 >> 4) & 0x3) as u8
    }
    #[inline]
    pub const fn d(self) -> u8 {
        ((self.0 >> 6) & 0x3) as u8
    }
    #[inline]
    pub const fn fix(self) -> u8 {
        (self.0 >> 32) as u8
    }
}

/// Decoded `DIMX` (`0x44`, `ps2sdk ee/libgs/include/libgs.h`: sixteen 3-bit signed entries at `y*4 + x` nibbles).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DimxReg(pub u64);

impl DimxReg {
    #[inline]
    pub const fn offset(self, x: usize, y: usize) -> i8 {
        let idx = ((y & 3) * 4) + (x & 3);
        let raw3 = ((self.0 >> (idx * 4)) & 0x7) as i8;
        (raw3 << 5) >> 5
    }
}

/// Decoded `TEST_1/2` (`0x47`/`0x48`, `ps2tek` `#gstestsandpixelcontrol`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TestReg(pub u64);

impl TestReg {
    #[inline]
    pub const fn ate(self) -> bool {
        (self.0 & 1) != 0
    }
    #[inline]
    pub const fn atst(self) -> u8 {
        ((self.0 >> 1) & 0x7) as u8
    }
    #[inline]
    pub const fn aref(self) -> u8 {
        ((self.0 >> 4) & 0xFF) as u8
    }
    #[inline]
    pub const fn afail(self) -> u8 {
        ((self.0 >> 12) & 0x3) as u8
    }
    #[inline]
    pub const fn date(self) -> bool {
        ((self.0 >> 14) & 1) != 0
    }
    #[inline]
    pub const fn datm(self) -> u8 {
        ((self.0 >> 15) & 1) as u8
    }
    #[inline]
    pub const fn zte(self) -> bool {
        ((self.0 >> 16) & 1) != 0
    }
    #[inline]
    pub const fn ztst(self) -> u8 {
        ((self.0 >> 17) & 0x3) as u8
    }
}

/// Decoded `FRAME_1/2` (`0x4C`/`0x4D`, `ps2tek` `#gsframeandzbuffers`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameReg(pub u64);

impl FrameReg {
    #[inline]
    pub const fn fbp(self) -> u32 {
        (self.0 & 0x1FF) as u32
    }
    #[inline]
    pub const fn fbp_bytes(self) -> u32 {
        self.fbp() * 8192
    }
    #[inline]
    pub const fn fbw(self) -> u32 {
        ((self.0 >> 16) & 0x3F) as u32
    }
    #[inline]
    pub const fn fbw_pixels(self) -> u32 {
        self.fbw() * 64
    }
    #[inline]
    pub const fn psm_raw(self) -> u8 {
        ((self.0 >> 24) & 0x3F) as u8
    }
    #[inline]
    pub const fn fbmsk(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

/// Decoded `ZBUF_1/2` (`0x4E`/`0x4F`, `ps2tek` `#gsframeandzbuffers`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ZbufReg(pub u64);

impl ZbufReg {
    #[inline]
    pub const fn zbp(self) -> u32 {
        (self.0 & 0x1FF) as u32
    }
    #[inline]
    pub const fn zbp_bytes(self) -> u32 {
        self.zbp() * 8192
    }
    #[inline]
    pub const fn zsm_raw(self) -> u8 {
        ((self.0 >> 24) & 0xF) as u8
    }
    #[inline]
    pub const fn zmsk(self) -> bool {
        ((self.0 >> 32) & 1) != 0
    }
}

/// Decoded `BITBLTBUF` (`0x50`, `ps2tek` `#gstransfers`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BitbltBufReg(pub u64);

impl BitbltBufReg {
    #[inline]
    pub const fn sbp(self) -> u32 {
        (self.0 & 0x3FFF) as u32
    }
    #[inline]
    pub const fn sbp_bytes(self) -> u32 {
        self.sbp() * 256
    }
    #[inline]
    pub const fn sbw(self) -> u32 {
        ((self.0 >> 16) & 0x3F) as u32
    }
    #[inline]
    pub const fn sbw_pixels(self) -> u32 {
        self.sbw() * 64
    }
    #[inline]
    pub const fn spsm_raw(self) -> u8 {
        ((self.0 >> 24) & 0x3F) as u8
    }
    #[inline]
    pub const fn dbp(self) -> u32 {
        ((self.0 >> 32) & 0x3FFF) as u32
    }
    #[inline]
    pub const fn dbp_bytes(self) -> u32 {
        self.dbp() * 256
    }
    #[inline]
    pub const fn dbw(self) -> u32 {
        ((self.0 >> 48) & 0x3F) as u32
    }
    #[inline]
    pub const fn dbw_pixels(self) -> u32 {
        self.dbw() * 64
    }
    #[inline]
    pub const fn dpsm_raw(self) -> u8 {
        ((self.0 >> 56) & 0x3F) as u8
    }
}

/// Decoded `TRXPOS` (`0x51`, `ps2tek` `#gstransfers`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TrxPosReg(pub u64);

impl TrxPosReg {
    #[inline]
    pub const fn ssax(self) -> u16 {
        (self.0 & 0x7FF) as u16
    }
    #[inline]
    pub const fn ssay(self) -> u16 {
        ((self.0 >> 16) & 0x7FF) as u16
    }
    #[inline]
    pub const fn dsax(self) -> u16 {
        ((self.0 >> 32) & 0x7FF) as u16
    }
    #[inline]
    pub const fn dsay(self) -> u16 {
        ((self.0 >> 48) & 0x7FF) as u16
    }
    #[inline]
    pub const fn dir(self) -> u8 {
        ((self.0 >> 59) & 0x3) as u8
    }
}

/// Decoded `TRXREG` (`0x52`, `ps2tek` `#gstransfers`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TrxRegReg(pub u64);

impl TrxRegReg {
    #[inline]
    pub const fn rrw(self) -> u16 {
        (self.0 & 0xFFF) as u16
    }
    #[inline]
    pub const fn rrh(self) -> u16 {
        ((self.0 >> 32) & 0xFFF) as u16
    }
}

/// Decoded `PMODE` (`0x1200_0000`, `Play! GSHandler.h` BSD-2 / `ps2sdk gs_privileged.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PmodeReg(pub u64);

impl PmodeReg {
    #[inline]
    pub const fn en1(self) -> bool {
        (self.0 & 1) != 0
    }
    #[inline]
    pub const fn en2(self) -> bool {
        ((self.0 >> 1) & 1) != 0
    }
    #[inline]
    pub const fn crtmd(self) -> u8 {
        ((self.0 >> 2) & 0x7) as u8
    }
    #[inline]
    pub const fn mmod(self) -> bool {
        ((self.0 >> 5) & 1) != 0
    }
    #[inline]
    pub const fn amod(self) -> bool {
        ((self.0 >> 6) & 1) != 0
    }
    #[inline]
    pub const fn slbg(self) -> bool {
        ((self.0 >> 7) & 1) != 0
    }
    #[inline]
    pub const fn alp(self) -> u8 {
        (self.0 >> 8) as u8
    }
}

/// Decoded `SMODE2` (`0x1200_0020`, `Play! GSHandler.h` BSD-2 / `ps2sdk gs_privileged.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Smode2Reg(pub u64);

impl Smode2Reg {
    #[inline]
    pub const fn interlaced(self) -> bool {
        (self.0 & 1) != 0
    }
    #[inline]
    pub const fn ffmd(self) -> bool {
        ((self.0 >> 1) & 1) != 0
    }
    #[inline]
    pub const fn dpms(self) -> u8 {
        ((self.0 >> 2) & 0x3) as u8
    }
}

/// Decoded `DISPFB1/2` (`0x1200_0070`/`0x1200_0090`, `Play! GSHandler.h` BSD-2 / `ps2sdk gs_privileged.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DispFbReg(pub u64);

impl DispFbReg {
    #[inline]
    pub const fn fbp(self) -> u32 {
        (self.0 & 0x1FF) as u32
    }
    #[inline]
    pub const fn fbp_bytes(self) -> u32 {
        self.fbp() * 8192
    }
    #[inline]
    pub const fn fbw(self) -> u32 {
        ((self.0 >> 9) & 0x3F) as u32
    }
    #[inline]
    pub const fn fbw_pixels(self) -> u32 {
        self.fbw() * 64
    }
    #[inline]
    pub const fn psm_raw(self) -> u8 {
        ((self.0 >> 15) & 0x1F) as u8
    }
    #[inline]
    pub const fn dbx(self) -> u16 {
        ((self.0 >> 32) & 0x7FF) as u16
    }
    #[inline]
    pub const fn dby(self) -> u16 {
        ((self.0 >> 43) & 0x7FF) as u16
    }
}

/// Decoded `DISPLAY1/2` (`0x1200_0080`/`0x1200_00A0`, `Play! GSHandler.h` BSD-2 / `ps2sdk gs_privileged.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayReg(pub u64);

impl DisplayReg {
    #[inline]
    pub const fn dx(self) -> u16 {
        (self.0 & 0xFFF) as u16
    }
    #[inline]
    pub const fn dy(self) -> u16 {
        ((self.0 >> 12) & 0x7FF) as u16
    }
    #[inline]
    pub const fn magh(self) -> u8 {
        ((self.0 >> 23) & 0xF) as u8
    }
    #[inline]
    pub const fn magv(self) -> u8 {
        ((self.0 >> 27) & 0x3) as u8
    }
    #[inline]
    pub const fn dw(self) -> u16 {
        ((self.0 >> 32) & 0xFFF) as u16
    }
    #[inline]
    pub const fn dh(self) -> u16 {
        ((self.0 >> 44) & 0x7FF) as u16
    }
}

/// Decoded `BGCOLOR` (`0x1200_00E0`, `ps2sdk gs_privileged.h`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BgColorReg(pub u64);

impl BgColorReg {
    #[inline]
    pub const fn r(self) -> u8 {
        self.0 as u8
    }
    #[inline]
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    #[inline]
    pub const fn b(self) -> u8 {
        (self.0 >> 16) as u8
    }
}

/// Privileged PCRTC / display register snapshot (`0x1200_0000..=0x1200_00E0`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PrivilegedDisplayRegs {
    pub pmode: PmodeReg,
    pub smode1: u64,
    pub smode2: Smode2Reg,
    pub srfsh: u64,
    pub synch1: u64,
    pub synch2: u64,
    pub syncv: u64,
    pub dispfb1: DispFbReg,
    pub display1: DisplayReg,
    pub dispfb2: DispFbReg,
    pub display2: DisplayReg,
    pub extbuf: u64,
    pub extdata: u64,
    pub extwrite: u64,
    pub bgcolor: BgColorReg,
}

/// Drawing context registers (`Context 1` or `Context 2`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextRegs {
    pub tex0: Tex0Reg,
    pub clamp: ClampReg,
    pub tex1: Tex1Reg,
    pub tex2: u64,
    pub xyoffset: XyOffsetReg,
    pub miptbp1: MipTbpReg,
    pub miptbp2: MipTbpReg,
    pub scissor: ScissorReg,
    pub alpha: AlphaReg,
    pub test: TestReg,
    pub fba: bool,
    pub frame: FrameReg,
    pub zbuf: ZbufReg,
}

/// Full decoded internal + PCRTC GS register file (`ps2tek` `#gsregisterlist`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GsRegs {
    pub raw: [u64; 128],
    pub prim: PrimReg,
    pub prmodecont: bool,
    pub prmode: u64,
    pub rgbaq: RgbaqReg,
    pub st: StReg,
    pub uv: UvReg,
    pub fog: FogReg,
    pub texclut: TexClutReg,
    pub scanmsk: ScanMskReg,
    pub texa: TexAReg,
    pub fogcol: FogColReg,
    pub dimx: DimxReg,
    pub dthe: bool,
    pub colclamp: bool,
    pub pabe: bool,
    pub bitbltbuf: BitbltBufReg,
    pub trxpos: TrxPosReg,
    pub trxreg: TrxRegReg,
    pub trxdir: u8,
    pub ctx: [ContextRegs; 2],
    pub priv_display: PrivilegedDisplayRegs,
}

impl Default for GsRegs {
    fn default() -> Self {
        Self::new()
    }
}

impl GsRegs {
    pub fn new() -> Self {
        Self {
            raw: [0; 128],
            prim: PrimReg(0),
            prmodecont: false,
            prmode: 0,
            rgbaq: RgbaqReg(0),
            st: StReg(0),
            uv: UvReg(0),
            fog: FogReg(0),
            texclut: TexClutReg(0),
            scanmsk: ScanMskReg(0),
            texa: TexAReg(0),
            fogcol: FogColReg(0),
            dimx: DimxReg(0),
            dthe: false,
            colclamp: false,
            pabe: false,
            bitbltbuf: BitbltBufReg(0),
            trxpos: TrxPosReg(0),
            trxreg: TrxRegReg(0),
            trxdir: 3,
            ctx: [ContextRegs::default(); 2],
            priv_display: PrivilegedDisplayRegs::default(),
        }
    }

    /// Return the active effective `PRIM` settings honoring `PRMODECONT` (`ps2tek` `#gsprimitives`).
    #[inline]
    pub const fn effective_prim(&self) -> PrimReg {
        if self.prmodecont {
            self.prim
        } else {
            self.prim.with_attributes_from(self.prmode)
        }
    }

    /// Latch a 64-bit internal register write and update decoded state fields.
    pub fn write_internal(&mut self, reg: u8, data: u64) {
        let idx = usize::from(reg & 0x7F);
        self.raw[idx] = data;
        match reg {
            gs_reg::PRIM => self.prim = PrimReg(data),
            gs_reg::RGBAQ => self.rgbaq = RgbaqReg(data),
            gs_reg::ST => self.st = StReg(data),
            gs_reg::UV => self.uv = UvReg(data),
            gs_reg::XYZF2 | gs_reg::XYZF3 => {
                // Writing XYZF2/3 updates the active fog register value (`ps2tek` `#gsvertexattributes`),
                // while leaving `raw[FOG]` untouched for raw-register inspection helpers.
                self.fog = FogReg(data);
            }
            gs_reg::TEX0_1 => self.ctx[0].tex0 = Tex0Reg(data),
            gs_reg::TEX0_2 => self.ctx[1].tex0 = Tex0Reg(data),
            gs_reg::CLAMP_1 => self.ctx[0].clamp = ClampReg(data),
            gs_reg::CLAMP_2 => self.ctx[1].clamp = ClampReg(data),
            gs_reg::FOG => self.fog = FogReg(data),
            gs_reg::TEX1_1 => self.ctx[0].tex1 = Tex1Reg(data),
            gs_reg::TEX1_2 => self.ctx[1].tex1 = Tex1Reg(data),
            gs_reg::TEX2_1 => {
                self.ctx[0].tex2 = data;
                let updated =
                    (self.ctx[0].tex0.0 & !TEX2_TO_TEX0_MASK) | (data & TEX2_TO_TEX0_MASK);
                self.ctx[0].tex0 = Tex0Reg(updated);
                self.raw[usize::from(gs_reg::TEX0_1)] = updated;
            }
            gs_reg::TEX2_2 => {
                self.ctx[1].tex2 = data;
                let updated =
                    (self.ctx[1].tex0.0 & !TEX2_TO_TEX0_MASK) | (data & TEX2_TO_TEX0_MASK);
                self.ctx[1].tex0 = Tex0Reg(updated);
                self.raw[usize::from(gs_reg::TEX0_2)] = updated;
            }
            gs_reg::XYOFFSET_1 => self.ctx[0].xyoffset = XyOffsetReg(data),
            gs_reg::XYOFFSET_2 => self.ctx[1].xyoffset = XyOffsetReg(data),
            gs_reg::PRMODECONT => self.prmodecont = (data & 1) != 0,
            gs_reg::PRMODE => self.prmode = data,
            gs_reg::TEXCLUT => self.texclut = TexClutReg(data),
            gs_reg::SCANMSK => self.scanmsk = ScanMskReg(data),
            gs_reg::MIPTBP1_1 => self.ctx[0].miptbp1 = MipTbpReg(data),
            gs_reg::MIPTBP1_2 => self.ctx[1].miptbp1 = MipTbpReg(data),
            gs_reg::MIPTBP2_1 => self.ctx[0].miptbp2 = MipTbpReg(data),
            gs_reg::MIPTBP2_2 => self.ctx[1].miptbp2 = MipTbpReg(data),
            gs_reg::TEXA => self.texa = TexAReg(data),
            gs_reg::FOGCOL => self.fogcol = FogColReg(data),
            gs_reg::SCISSOR_1 => self.ctx[0].scissor = ScissorReg(data),
            gs_reg::SCISSOR_2 => self.ctx[1].scissor = ScissorReg(data),
            gs_reg::ALPHA_1 => self.ctx[0].alpha = AlphaReg(data),
            gs_reg::ALPHA_2 => self.ctx[1].alpha = AlphaReg(data),
            gs_reg::DIMX => self.dimx = DimxReg(data),
            gs_reg::DTHE => self.dthe = (data & 1) != 0,
            gs_reg::COLCLAMP => self.colclamp = (data & 1) != 0,
            gs_reg::TEST_1 => self.ctx[0].test = TestReg(data),
            gs_reg::TEST_2 => self.ctx[1].test = TestReg(data),
            gs_reg::PABE => self.pabe = (data & 1) != 0,
            gs_reg::FBA_1 => self.ctx[0].fba = (data & 1) != 0,
            gs_reg::FBA_2 => self.ctx[1].fba = (data & 1) != 0,
            gs_reg::FRAME_1 => self.ctx[0].frame = FrameReg(data),
            gs_reg::FRAME_2 => self.ctx[1].frame = FrameReg(data),
            gs_reg::ZBUF_1 => self.ctx[0].zbuf = ZbufReg(data),
            gs_reg::ZBUF_2 => self.ctx[1].zbuf = ZbufReg(data),
            gs_reg::BITBLTBUF => self.bitbltbuf = BitbltBufReg(data),
            gs_reg::TRXPOS => self.trxpos = TrxPosReg(data),
            gs_reg::TRXREG => self.trxreg = TrxRegReg(data),
            gs_reg::TRXDIR => self.trxdir = (data & 0x3) as u8,
            _ => {}
        }
    }

    /// Update PCRTC privileged display registers (`0x1200_0000..=0x1200_00E0`).
    pub fn write_privileged_display(&mut self, offset: u32, data: u64) {
        match offset & 0x1FFF {
            0x0000 => self.priv_display.pmode = PmodeReg(data),
            0x0010 => self.priv_display.smode1 = data,
            0x0020 => self.priv_display.smode2 = Smode2Reg(data),
            0x0030 => self.priv_display.srfsh = data,
            0x0040 => self.priv_display.synch1 = data,
            0x0050 => self.priv_display.synch2 = data,
            0x0060 => self.priv_display.syncv = data,
            0x0070 => self.priv_display.dispfb1 = DispFbReg(data),
            0x0080 => self.priv_display.display1 = DisplayReg(data),
            0x0090 => self.priv_display.dispfb2 = DispFbReg(data),
            0x00A0 => self.priv_display.display2 = DisplayReg(data),
            0x00B0 => self.priv_display.extbuf = data,
            0x00C0 => self.priv_display.extdata = data,
            0x00D0 => self.priv_display.extwrite = data,
            0x00E0 => self.priv_display.bgcolor = BgColorReg(data),
            _ => {}
        }
    }
}
