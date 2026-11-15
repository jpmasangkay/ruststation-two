//! Stateless GIF packet decoding helpers (`ps2tek` `#giftags`, `#gifdataformats`).

use ps2_common::Reg128;
use thiserror::Error;

use crate::regs::gs_reg;
use crate::vram::PixelFormat;
use crate::GsState;

/// GIFtag payload format (`GIFtag` bits `58..=59`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum GifDataFormat {
    #[default]
    Packed = 0,
    Reglist = 1,
    Image = 2,
}

/// Parsed 128-bit GIFtag (`ps2tek` `#giftags`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GifTag {
    pub raw_words: [u32; 4],
    pub nloop: u16,
    pub eop: bool,
    pub pre: bool,
    pub prim: u16,
    pub format: GifDataFormat,
    pub nregs: u8,
    pub regs: u64,
}

impl GifTag {
    /// Decode a 128-bit `Reg128` quadword into a [`GifTag`].
    #[inline]
    pub fn from_qwc(qwc: Reg128) -> Self {
        let lo = qwc.lo_u64();
        let hi = qwc.hi_u64();
        let raw_words = [lo as u32, (lo >> 32) as u32, hi as u32, (hi >> 32) as u32];
        let nloop = (lo & 0x7FFF) as u16;
        let eop = (lo & (1 << 15)) != 0;
        let pre = (lo & (1 << 46)) != 0;
        let prim = ((lo >> 47) & 0x7FF) as u16;
        let format = match (lo >> 58) & 0x3 {
            0 => GifDataFormat::Packed,
            1 => GifDataFormat::Reglist,
            _ => GifDataFormat::Image,
        };
        let raw_nregs = ((lo >> 60) & 0xF) as u8;
        let nregs = if raw_nregs == 0 { 16 } else { raw_nregs };
        Self {
            raw_words,
            nloop,
            eop,
            pre,
            prim,
            format,
            nregs,
            regs: hi,
        }
    }
}

/// Outcome of unpacking a single `PACKED`-mode quadword against `reg_desc` (`0x0..=0xF`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PackedDecodeOutcome {
    pub reg_write: Option<(u8, u64)>,
    pub next_internal_q: u32,
}

/// Unpack one 128-bit `PACKED`-mode GIF quadword (`ps2tek` `#gifdataformats`).
#[inline]
pub fn decode_packed_qwc(reg_desc: u8, qwc: Reg128, internal_q: u32) -> PackedDecodeOutcome {
    let lo = qwc.lo_u64();
    let hi = qwc.hi_u64();
    let mut next_internal_q = internal_q;

    let reg_write = match reg_desc & 0xF {
        0x0 => Some((gs_reg::PRIM, lo & 0x7FF)),
        0x1 => {
            let r = lo & 0xFF;
            let g = (lo >> 32) & 0xFF;
            let b = hi & 0xFF;
            let a = (hi >> 32) & 0xFF;
            let rgbaq = r | (g << 8) | (b << 16) | (a << 24) | (u64::from(internal_q) << 32);
            Some((gs_reg::RGBAQ, rgbaq))
        }
        0x2 => {
            next_internal_q = hi as u32;
            Some((gs_reg::ST, lo))
        }
        0x3 => {
            let u = lo & 0x3FFF;
            let v = (lo >> 32) & 0x3FFF;
            Some((gs_reg::UV, u | (v << 16)))
        }
        0x4 => {
            let disable_draw = ((hi >> 47) & 1) != 0;
            let x = lo & 0xFFFF;
            let y = (lo >> 32) & 0xFFFF;
            let z = (hi >> 4) & 0x00FF_FFFF;
            let f = (hi >> 36) & 0xFF;
            let reg = if disable_draw {
                gs_reg::XYZF3
            } else {
                gs_reg::XYZF2
            };
            Some((reg, x | (y << 16) | (z << 32) | (f << 56)))
        }
        0x5 => {
            let disable_draw = ((hi >> 47) & 1) != 0;
            let x = lo & 0xFFFF;
            let y = (lo >> 32) & 0xFFFF;
            let z = hi & 0xFFFF_FFFF;
            let reg = if disable_draw {
                gs_reg::XYZ3
            } else {
                gs_reg::XYZ2
            };
            Some((reg, x | (y << 16) | (z << 32)))
        }
        0x6..=0x9 | 0xC | 0xD => Some((reg_desc & 0xF, lo)),
        0xA => Some((gs_reg::FOG, ((hi >> 36) & 0xFF) << 56)),
        0xB | 0xF => None,
        0xE => Some(((hi & 0xFF) as u8, lo)),
        _ => None,
    };

    PackedDecodeOutcome {
        reg_write,
        next_internal_q,
    }
}

/// Single-stream stateless/FIFO-free GIF packet feeder (`GIFtag` + `PACKED`/`REGLIST`/`IMAGE`),
/// used by `.gsp` packet-stream tests and standalone `ps2-gs` packet replay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GifStreamDecoder {
    in_packet: bool,
    tag: GifTag,
    remaining_loops: u16,
    cur_reg_idx: u8,
    internal_q: u32,
}

impl GifStreamDecoder {
    pub const fn new() -> Self {
        Self {
            in_packet: false,
            tag: GifTag {
                raw_words: [0; 4],
                nloop: 0,
                eop: false,
                pre: false,
                prim: 0,
                format: GifDataFormat::Packed,
                nregs: 0,
                regs: 0,
            },
            remaining_loops: 0,
            cur_reg_idx: 0,
            internal_q: 0x3F80_0000,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Feed one 128-bit quadword into the GS. Returns `true` if a `GIFtag` with `EOP=1` just completed.
    pub fn feed_qwc(&mut self, gs: &mut GsState, qwc: Reg128) -> bool {
        if !self.in_packet {
            let tag = GifTag::from_qwc(qwc);
            if tag.nloop == 0 {
                return tag.eop;
            }
            if tag.pre && tag.format == GifDataFormat::Packed {
                gs.write_internal_reg(gs_reg::PRIM, u64::from(tag.prim));
            }
            self.tag = tag;
            self.remaining_loops = tag.nloop;
            self.cur_reg_idx = 0;
            self.in_packet = true;
            return false;
        }

        match self.tag.format {
            GifDataFormat::Packed => {
                let reg_desc = ((self.tag.regs >> (u32::from(self.cur_reg_idx) * 4)) & 0xF) as u8;
                let outcome = decode_packed_qwc(reg_desc, qwc, self.internal_q);
                if let Some((addr, data)) = outcome.reg_write {
                    gs.write_internal_reg(addr, data);
                }
                self.internal_q = outcome.next_internal_q;
                self.advance_reg_loop()
            }
            GifDataFormat::Reglist => {
                for half in [qwc.lo_u64(), qwc.hi_u64()] {
                    if !self.in_packet {
                        break;
                    }
                    let reg_desc =
                        ((self.tag.regs >> (u32::from(self.cur_reg_idx) * 4)) & 0xF) as u8;
                    if reg_desc != 0xF {
                        gs.write_internal_reg(reg_desc, half);
                    }
                    if self.advance_reg_loop() {
                        return true;
                    }
                }
                false
            }
            GifDataFormat::Image => {
                gs.write_internal_reg(gs_reg::HWREG, qwc.lo_u64());
                gs.write_internal_reg(gs_reg::HWREG, qwc.hi_u64());
                self.remaining_loops = self.remaining_loops.saturating_sub(1);
                if self.remaining_loops == 0 {
                    self.in_packet = false;
                    return self.tag.eop;
                }
                false
            }
        }
    }

    fn advance_reg_loop(&mut self) -> bool {
        self.cur_reg_idx += 1;
        if self.cur_reg_idx >= self.tag.nregs {
            self.cur_reg_idx = 0;
            self.remaining_loops = self.remaining_loops.saturating_sub(1);
            if self.remaining_loops == 0 {
                self.in_packet = false;
                return self.tag.eop;
            }
        }
        false
    }
}

/// Optional target surface rectangle metadata inside a `.gsp` script (`surface ...`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GspSurfaceHint {
    pub bp_blocks: u32,
    pub bw: u32,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
}

/// Parsed command from an ASCII `.gsp` GS packet-stream script (`docs/GS_PACKET_FORMAT.md`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GspCommand {
    InternalReg { addr: u8, data: u64 },
    PrivilegedReg { offset: u32, data: u64 },
    Qwc(Reg128),
    Surface(GspSurfaceHint),
}

/// Errors returned while parsing `.gsp` scripts.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum GspParseError {
    #[error("line {line}: unknown directive '{op}'")]
    UnknownDirective { line: usize, op: String },
    #[error("line {line}: expected {expected} arguments after '{op}', got {got}")]
    ArgCount {
        line: usize,
        op: String,
        expected: usize,
        got: usize,
    },
    #[error("line {line}: invalid register specifier '{token}'")]
    InvalidRegister { line: usize, token: String },
    #[error("line {line}: invalid integer literal '{token}'")]
    InvalidNumber { line: usize, token: String },
    #[error("line {line}: unknown or unsupported pixel format '{token}'")]
    InvalidPixelFormat { line: usize, token: String },
}

fn parse_u64_token(line: usize, token: &str) -> Result<u64, GspParseError> {
    let clean = token.replace('_', "");
    let res = if let Some(hex) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        clean.parse::<u64>()
    };
    res.map_err(|_| GspParseError::InvalidNumber {
        line,
        token: token.to_string(),
    })
}

fn parse_u32_token(line: usize, token: &str) -> Result<u32, GspParseError> {
    let v = parse_u64_token(line, token)?;
    u32::try_from(v).map_err(|_| GspParseError::InvalidNumber {
        line,
        token: token.to_string(),
    })
}

fn parse_internal_reg_name(line: usize, token: &str) -> Result<u8, GspParseError> {
    match token.to_ascii_uppercase().as_str() {
        "PRIM" => Ok(gs_reg::PRIM),
        "RGBAQ" => Ok(gs_reg::RGBAQ),
        "ST" => Ok(gs_reg::ST),
        "UV" => Ok(gs_reg::UV),
        "XYZF2" => Ok(gs_reg::XYZF2),
        "XYZ2" => Ok(gs_reg::XYZ2),
        "TEX0_1" => Ok(gs_reg::TEX0_1),
        "TEX0_2" => Ok(gs_reg::TEX0_2),
        "CLAMP_1" => Ok(gs_reg::CLAMP_1),
        "CLAMP_2" => Ok(gs_reg::CLAMP_2),
        "FOG" => Ok(gs_reg::FOG),
        "XYZF3" => Ok(gs_reg::XYZF3),
        "XYZ3" => Ok(gs_reg::XYZ3),
        "TEX1_1" => Ok(gs_reg::TEX1_1),
        "TEX1_2" => Ok(gs_reg::TEX1_2),
        "TEX2_1" => Ok(gs_reg::TEX2_1),
        "TEX2_2" => Ok(gs_reg::TEX2_2),
        "XYOFFSET_1" => Ok(gs_reg::XYOFFSET_1),
        "XYOFFSET_2" => Ok(gs_reg::XYOFFSET_2),
        "PRMODECONT" => Ok(gs_reg::PRMODECONT),
        "PRMODE" => Ok(gs_reg::PRMODE),
        "TEXCLUT" => Ok(gs_reg::TEXCLUT),
        "SCANMSK" => Ok(gs_reg::SCANMSK),
        "MIPTBP1_1" => Ok(gs_reg::MIPTBP1_1),
        "MIPTBP1_2" => Ok(gs_reg::MIPTBP1_2),
        "MIPTBP2_1" => Ok(gs_reg::MIPTBP2_1),
        "MIPTBP2_2" => Ok(gs_reg::MIPTBP2_2),
        "TEXA" => Ok(gs_reg::TEXA),
        "FOGCOL" => Ok(gs_reg::FOGCOL),
        "TEXFLUSH" => Ok(gs_reg::TEXFLUSH),
        "SCISSOR_1" => Ok(gs_reg::SCISSOR_1),
        "SCISSOR_2" => Ok(gs_reg::SCISSOR_2),
        "ALPHA_1" => Ok(gs_reg::ALPHA_1),
        "ALPHA_2" => Ok(gs_reg::ALPHA_2),
        "DIMX" => Ok(gs_reg::DIMX),
        "DTHE" => Ok(gs_reg::DTHE),
        "COLCLAMP" => Ok(gs_reg::COLCLAMP),
        "TEST_1" => Ok(gs_reg::TEST_1),
        "TEST_2" => Ok(gs_reg::TEST_2),
        "PABE" => Ok(gs_reg::PABE),
        "FBA_1" => Ok(gs_reg::FBA_1),
        "FBA_2" => Ok(gs_reg::FBA_2),
        "FRAME_1" => Ok(gs_reg::FRAME_1),
        "FRAME_2" => Ok(gs_reg::FRAME_2),
        "ZBUF_1" => Ok(gs_reg::ZBUF_1),
        "ZBUF_2" => Ok(gs_reg::ZBUF_2),
        "BITBLTBUF" => Ok(gs_reg::BITBLTBUF),
        "TRXPOS" => Ok(gs_reg::TRXPOS),
        "TRXREG" => Ok(gs_reg::TRXREG),
        "TRXDIR" => Ok(gs_reg::TRXDIR),
        "HWREG" => Ok(gs_reg::HWREG),
        "SIGNAL" => Ok(gs_reg::SIGNAL),
        "FINISH" => Ok(gs_reg::FINISH),
        "LABEL" => Ok(gs_reg::LABEL),
        "NOP" => Ok(gs_reg::NOP),
        _ => {
            let v = parse_u64_token(line, token).map_err(|_| GspParseError::InvalidRegister {
                line,
                token: token.to_string(),
            })?;
            if v <= 0x7F {
                Ok(v as u8)
            } else {
                Err(GspParseError::InvalidRegister {
                    line,
                    token: token.to_string(),
                })
            }
        }
    }
}

fn parse_priv_reg_offset(line: usize, token: &str) -> Result<u32, GspParseError> {
    match token.to_ascii_uppercase().as_str() {
        "PMODE" => Ok(0x0000),
        "SMODE1" => Ok(0x0010),
        "SMODE2" => Ok(0x0020),
        "SRFSH" => Ok(0x0030),
        "SYNCH1" => Ok(0x0040),
        "SYNCH2" => Ok(0x0050),
        "SYNCV" => Ok(0x0060),
        "DISPFB1" => Ok(0x0070),
        "DISPLAY1" => Ok(0x0080),
        "DISPFB2" => Ok(0x0090),
        "DISPLAY2" => Ok(0x00A0),
        "EXTBUF" => Ok(0x00B0),
        "EXTDATA" => Ok(0x00C0),
        "EXTWRITE" => Ok(0x00D0),
        "BGCOLOR" => Ok(0x00E0),
        _ => parse_u32_token(line, token).map_err(|_| GspParseError::InvalidRegister {
            line,
            token: token.to_string(),
        }),
    }
}

fn parse_pixel_format_token(line: usize, token: &str) -> Result<PixelFormat, GspParseError> {
    match token.to_ascii_uppercase().as_str() {
        "PSMCT32" => Ok(PixelFormat::Psmct32),
        "PSMCT24" => Ok(PixelFormat::Psmct24),
        "PSMCT16" => Ok(PixelFormat::Psmct16),
        "PSMCT16S" => Ok(PixelFormat::Psmct16s),
        "PSMT8" | "PSMCT8" => Ok(PixelFormat::Psmt8),
        "PSMT4" | "PSMCT4" => Ok(PixelFormat::Psmt4),
        "PSMT8H" | "PSMCT8H" => Ok(PixelFormat::Psmt8h),
        "PSMT4HL" | "PSMCT4HL" => Ok(PixelFormat::Psmt4hl),
        "PSMT4HH" | "PSMCT4HH" => Ok(PixelFormat::Psmt4hh),
        "PSMZ32" => Ok(PixelFormat::Psmz32),
        "PSMZ24" => Ok(PixelFormat::Psmz24),
        "PSMZ16" => Ok(PixelFormat::Psmz16),
        "PSMZ16S" => Ok(PixelFormat::Psmz16s),
        _ => {
            let v =
                parse_u64_token(line, token).map_err(|_| GspParseError::InvalidPixelFormat {
                    line,
                    token: token.to_string(),
                })?;
            u8::try_from(v)
                .ok()
                .and_then(PixelFormat::from_raw)
                .ok_or_else(|| GspParseError::InvalidPixelFormat {
                    line,
                    token: token.to_string(),
                })
        }
    }
}

/// Parse an ASCII `.gsp` GS packet-stream script into a list of [`GspCommand`]s.
pub fn parse_gsp_script(source: &str) -> Result<Vec<GspCommand>, GspParseError> {
    let mut cmds = Vec::new();
    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let without_hash = raw_line.split('#').next().unwrap_or("");
        let code = without_hash.split("//").next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = code.split_whitespace().collect();
        let op = tokens[0].to_ascii_lowercase();
        let args = &tokens[1..];
        match op.as_str() {
            "reg" => {
                if args.len() != 2 {
                    return Err(GspParseError::ArgCount {
                        line: line_no,
                        op,
                        expected: 2,
                        got: args.len(),
                    });
                }
                let addr = parse_internal_reg_name(line_no, args[0])?;
                let data = parse_u64_token(line_no, args[1])?;
                cmds.push(GspCommand::InternalReg { addr, data });
            }
            "priv" => {
                if args.len() != 2 {
                    return Err(GspParseError::ArgCount {
                        line: line_no,
                        op,
                        expected: 2,
                        got: args.len(),
                    });
                }
                let offset = parse_priv_reg_offset(line_no, args[0])?;
                let data = parse_u64_token(line_no, args[1])?;
                cmds.push(GspCommand::PrivilegedReg { offset, data });
            }
            "qwc" => {
                if args.len() != 2 {
                    return Err(GspParseError::ArgCount {
                        line: line_no,
                        op,
                        expected: 2,
                        got: args.len(),
                    });
                }
                let lo = parse_u64_token(line_no, args[0])?;
                let hi = parse_u64_token(line_no, args[1])?;
                cmds.push(GspCommand::Qwc(Reg128::from_u64_pair(lo, hi)));
            }
            "qwc32" => {
                if args.len() != 4 {
                    return Err(GspParseError::ArgCount {
                        line: line_no,
                        op,
                        expected: 4,
                        got: args.len(),
                    });
                }
                let w0 = parse_u32_token(line_no, args[0])?;
                let w1 = parse_u32_token(line_no, args[1])?;
                let w2 = parse_u32_token(line_no, args[2])?;
                let w3 = parse_u32_token(line_no, args[3])?;
                cmds.push(GspCommand::Qwc(Reg128::from_u32x4(w0, w1, w2, w3)));
            }
            "surface" => {
                if args.len() != 5 {
                    return Err(GspParseError::ArgCount {
                        line: line_no,
                        op,
                        expected: 5,
                        got: args.len(),
                    });
                }
                let bp_blocks = parse_u32_token(line_no, args[0])?;
                let bw = parse_u32_token(line_no, args[1])?;
                let format = parse_pixel_format_token(line_no, args[2])?;
                let width = parse_u32_token(line_no, args[3])?;
                let height = parse_u32_token(line_no, args[4])?;
                cmds.push(GspCommand::Surface(GspSurfaceHint {
                    bp_blocks,
                    bw,
                    format,
                    width,
                    height,
                }));
            }
            _ => {
                return Err(GspParseError::UnknownDirective { line: line_no, op });
            }
        }
    }
    Ok(cmds)
}

/// Parse and replay an ASCII `.gsp` script against [`GsState`], returning the optional
/// `surface` hint declared by the script.
pub fn execute_gsp_script(
    gs: &mut GsState,
    source: &str,
) -> Result<Option<GspSurfaceHint>, GspParseError> {
    let cmds = parse_gsp_script(source)?;
    let mut decoder = GifStreamDecoder::new();
    let mut surface = None;

    for cmd in cmds {
        match cmd {
            GspCommand::InternalReg { addr, data } => {
                gs.write_internal_reg(addr, data);
            }
            GspCommand::PrivilegedReg { offset, data } => {
                gs.write_privileged_display(offset, data);
            }
            GspCommand::Qwc(qwc) => {
                decoder.feed_qwc(gs, qwc);
            }
            GspCommand::Surface(hint) => {
                surface = Some(hint);
            }
        }
    }

    Ok(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_stream_decoder_runs_packed_reglist_and_image() {
        let mut gs = GsState::new();
        let mut dec = GifStreamDecoder::new();

        // A+D GIFtag setup + 4-loop A+D setting up 2x2 PSMCT32 Host->Local at (0,0), dbw=1:
        // BITBLTBUF = (1 << 48), TRXPOS = 0, TRXREG = 2 | (2<<32), TRXDIR = 0
        let ad_tag = Reg128::from_u64_pair(4 | (1 << 15) | (1u64 << 60), 0xE);
        assert!(!dec.feed_qwc(&mut gs, ad_tag));
        assert!(!dec.feed_qwc(
            &mut gs,
            Reg128::from_u64_pair(1 << 48, u64::from(gs_reg::BITBLTBUF))
        ));
        assert!(!dec.feed_qwc(&mut gs, Reg128::from_u64_pair(0, u64::from(gs_reg::TRXPOS))));
        assert!(!dec.feed_qwc(
            &mut gs,
            Reg128::from_u64_pair(2 | (2 << 32), u64::from(gs_reg::TRXREG))
        ));
        assert!(dec.feed_qwc(&mut gs, Reg128::from_u64_pair(0, u64::from(gs_reg::TRXDIR))));

        // IMAGE GIFtag: nloop=1, eop=1, flg=2 (IMAGE)
        let img_tag = Reg128::from_u64_pair(1 | (1 << 15) | (2u64 << 58), 0);
        assert!(!dec.feed_qwc(&mut gs, img_tag));
        assert!(dec.feed_qwc(&mut gs, Reg128::from_u32x4(0x11, 0x22, 0x33, 0x44)));

        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0),
            0x11
        );
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 0),
            0x22
        );
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 1),
            0x33
        );
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 1),
            0x44
        );

        // REGLIST with odd NREG=3 (last doubleword discarded) + PRE in PACKED
        let reglist_tag = Reg128::from_u64_pair(
            1 | (1 << 15) | (1u64 << 58) | (3u64 << 60),
            u64::from(gs_reg::PRIM) | (u64::from(gs_reg::FOG) << 4) | (0xF << 8),
        );
        assert!(!dec.feed_qwc(&mut gs, reglist_tag));
        assert!(!dec.feed_qwc(&mut gs, Reg128::from_u64_pair(0x6, 0x55 << 56)));
        assert!(dec.feed_qwc(&mut gs, Reg128::from_u64_pair(0xDEAD, 0xBEEF)));
        assert_eq!(gs.internal_reg(gs_reg::PRIM), 0x6);
        assert_eq!(gs.regs.fog.f(), 0x55);
    }

    #[test]
    fn gsp_script_parser_executes_and_reports_syntax_errors() {
        let script = r#"
            # Setup display + 2x1 PSMCT32 image via packets
            priv PMODE 0x01
            surface 0 1 PSMCT32 2 1
            reg BITBLTBUF 0x0001_0000_0000_0000
            reg TRXPOS 0
            reg TRXREG 0x0000_0001_0000_0002
            reg TRXDIR 0
            qwc32 0x0000_8001 0x0800_0000 0 0
            qwc32 0xFF00_00AA 0xFF00_BB00 0 0
        "#;
        let mut gs = GsState::new();
        let hint = execute_gsp_script(&mut gs, script)
            .expect("valid script")
            .expect("surface");
        assert_eq!(hint.width, 2);
        assert_eq!(hint.height, 1);
        assert!(gs.regs.priv_display.pmode.en1());
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0),
            0xFF00_00AA
        );
        assert_eq!(
            gs.vram.read_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 0),
            0xFF00_BB00
        );

        assert!(matches!(
            parse_gsp_script("bogus 1 2"),
            Err(GspParseError::UnknownDirective { line: 1, .. })
        ));
        assert!(matches!(
            parse_gsp_script("reg BITBLTBUF"),
            Err(GspParseError::ArgCount {
                line: 1,
                expected: 2,
                got: 1,
                ..
            })
        ));
        assert!(matches!(
            parse_gsp_script("surface 0 1 BADFMT 4 4"),
            Err(GspParseError::InvalidPixelFormat { line: 1, .. })
        ));
    }
}
