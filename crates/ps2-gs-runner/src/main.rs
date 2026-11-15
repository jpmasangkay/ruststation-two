//! Offline `.gsp` GS packet-stream runner and PNG framebuffer dumper (`Phase 2b Step 2`).

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use clap::Parser;
use ps2_gs::{execute_gsp_script, GsState, GspParseError, PixelFormat};
use thiserror::Error;

#[derive(Parser, Debug)]
#[command(
    name = "ps2-gs-runner",
    about = "Replay GS .gsp packet scripts and dump VRAM framebuffer regions to PNG"
)]
struct Cli {
    /// One or more `.gsp` packet-stream script paths.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Framebuffer region `FBP,FBW,PSM,W,H` (`FBP` in 2048-word pages, `FBW` in 64-px units).
    /// Falls back to `.gsp` `surface` directive when omitted.
    #[arg(long, value_name = "FBP,FBW,PSM,W,H")]
    rect: Option<DumpRect>,

    /// Explicit output PNG path (valid only when a single `.gsp` input is given).
    #[arg(short, long)]
    out: Option<PathBuf>,

    /// Output directory for `<input-stem>.png` files when `--out` is omitted.
    #[arg(long, default_value = "local/gs_out")]
    out_dir: PathBuf,

    /// Export 4-channel RGBA PNG instead of opaque RGB (preserves GS destination alpha).
    #[arg(long)]
    rgba: bool,

    /// Integer nearest-neighbor upscale factor (`N >= 1`) applied to exported PNG dimensions.
    #[arg(long, default_value_t = 1, value_name = "N", value_parser = parse_nonzero_scale)]
    scale: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct DumpRect {
    bp_blocks: u32,
    bw: u32,
    format: PixelFormat,
    width: u32,
    height: u32,
}

impl FromStr for DumpRect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 5 {
            return Err(format!(
                "expected 'FBP,FBW,PSM,W,H' (5 comma-separated fields), got '{s}'"
            ));
        }
        let fbp = parse_u32_lit(parts[0]).map_err(|e| format!("invalid FBP: {e}"))?;
        let bw = parse_u32_lit(parts[1]).map_err(|e| format!("invalid FBW: {e}"))?;
        let format = parse_psm(parts[2])?;
        let width = parse_u32_lit(parts[3]).map_err(|e| format!("invalid width: {e}"))?;
        let height = parse_u32_lit(parts[4]).map_err(|e| format!("invalid height: {e}"))?;
        if width == 0 || height == 0 {
            return Err("dump width and height must both be > 0".to_string());
        }
        let bp_blocks = fbp
            .checked_mul(32)
            .ok_or_else(|| "FBP overflows block address".to_string())?;
        Ok(Self {
            bp_blocks,
            bw,
            format,
            width,
            height,
        })
    }
}

#[derive(Debug, Error)]
enum RunnerError {
    #[error("`--out` can only be used with a single input file")]
    OutWithMultipleInputs,
    #[error("failed to read '{path}': {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse/execute '{path}': {source}")]
    ParseGsp {
        path: String,
        #[source]
        source: GspParseError,
    },
    #[error("no `--rect` supplied and '{0}' contains neither a valid `surface` directive nor an active FRAME_1/SCISSOR_1 rect")]
    MissingSurface(String),
    #[error("unsupported PNG dump pixel format {0:?} (expected 32/24/16-bit color/Z format)")]
    UnsupportedFormat(PixelFormat),
    #[error("invalid PNG scale factor {0} (must be >= 1 and fit u32 dimensions)")]
    InvalidScale(u32),
    #[error("I/O error writing '{path}': {source}")]
    WriteIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("PNG encoding error for '{path}': {source}")]
    PngEncode {
        path: String,
        #[source]
        source: png::EncodingError,
    },
}

fn parse_nonzero_scale(token: &str) -> Result<u32, String> {
    let scale = parse_u32_lit(token)?;
    if scale == 0 {
        return Err("scale factor must be >= 1".to_string());
    }
    Ok(scale)
}

fn parse_u32_lit(token: &str) -> Result<u32, String> {
    let clean = token.replace('_', "");
    let res = if let Some(hex) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16)
    } else {
        clean.parse::<u32>()
    };
    res.map_err(|e| format!("'{token}': {e}"))
}

fn parse_psm(token: &str) -> Result<PixelFormat, String> {
    match token.to_ascii_uppercase().as_str() {
        "PSMCT32" => Ok(PixelFormat::Psmct32),
        "PSMCT24" => Ok(PixelFormat::Psmct24),
        "PSMCT16" => Ok(PixelFormat::Psmct16),
        "PSMCT16S" => Ok(PixelFormat::Psmct16s),
        "PSMZ32" => Ok(PixelFormat::Psmz32),
        "PSMZ24" => Ok(PixelFormat::Psmz24),
        "PSMZ16" => Ok(PixelFormat::Psmz16),
        "PSMZ16S" => Ok(PixelFormat::Psmz16s),
        _ => {
            let v = parse_u32_lit(token).map_err(|e| format!("invalid PSM: {e}"))?;
            u8::try_from(v)
                .ok()
                .and_then(PixelFormat::from_raw)
                .ok_or_else(|| format!("unknown pixel format '{token}'"))
        }
    }
}

fn decode_pixel_rgba8(raw: u32, format: PixelFormat) -> Result<[u8; 4], RunnerError> {
    match format {
        PixelFormat::Psmct32 | PixelFormat::Psmz32 => Ok([
            raw as u8,
            (raw >> 8) as u8,
            (raw >> 16) as u8,
            (raw >> 24) as u8,
        ]),
        PixelFormat::Psmct24 | PixelFormat::Psmz24 => {
            Ok([raw as u8, (raw >> 8) as u8, (raw >> 16) as u8, 0xFF])
        }
        PixelFormat::Psmct16
        | PixelFormat::Psmct16s
        | PixelFormat::Psmz16
        | PixelFormat::Psmz16s => {
            let r5 = (raw & 0x1F) as u8;
            let g5 = ((raw >> 5) & 0x1F) as u8;
            let b5 = ((raw >> 10) & 0x1F) as u8;
            let a1 = ((raw >> 15) & 1) as u8;
            Ok([
                (r5 << 3) | (r5 >> 2),
                (g5 << 3) | (g5 >> 2),
                (b5 << 3) | (b5 >> 2),
                if a1 != 0 { 0xFF } else { 0x00 },
            ])
        }
        other => Err(RunnerError::UnsupportedFormat(other)),
    }
}

fn encode_framebuffer_png<W: Write>(
    gs: &GsState,
    rect: DumpRect,
    rgba: bool,
    scale: u32,
    writer: W,
    label: &str,
) -> Result<(), RunnerError> {
    if scale == 0 {
        return Err(RunnerError::InvalidScale(scale));
    }
    let out_w = rect
        .width
        .checked_mul(scale)
        .ok_or(RunnerError::InvalidScale(scale))?;
    let out_h = rect
        .height
        .checked_mul(scale)
        .ok_or(RunnerError::InvalidScale(scale))?;

    let channels = if rgba { 4 } else { 3 };
    let total = (out_w as usize)
        .checked_mul(out_h as usize)
        .and_then(|px| px.checked_mul(channels))
        .ok_or(RunnerError::InvalidScale(scale))?;
    let mut pixels = Vec::with_capacity(total);
    let mut scaled_row = Vec::with_capacity((out_w as usize) * channels);

    for y in 0..rect.height {
        scaled_row.clear();
        for x in 0..rect.width {
            let raw = gs
                .vram
                .read_pixel_raw(rect.format, rect.bp_blocks, rect.bw, x, y);
            let [r, g, b, a] = decode_pixel_rgba8(raw, rect.format)?;
            for _ in 0..scale {
                scaled_row.push(r);
                scaled_row.push(g);
                scaled_row.push(b);
                if rgba {
                    scaled_row.push(a);
                }
            }
        }
        for _ in 0..scale {
            pixels.extend_from_slice(&scaled_row);
        }
    }

    let mut encoder = png::Encoder::new(writer, out_w, out_h);
    encoder.set_color(if rgba {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder
        .write_header()
        .map_err(|source| RunnerError::PngEncode {
            path: label.to_string(),
            source,
        })?;
    png_writer
        .write_image_data(&pixels)
        .map_err(|source| RunnerError::PngEncode {
            path: label.to_string(),
            source,
        })?;
    Ok(())
}

fn infer_rect_from_state(gs: &GsState) -> Option<DumpRect> {
    let frame = gs.regs.ctx[0].frame;
    let scissor = gs.regs.ctx[0].scissor;
    let w = u32::from(scissor.scax1()).saturating_sub(u32::from(scissor.scax0())) + 1;
    let h = u32::from(scissor.scay1()).saturating_sub(u32::from(scissor.scay0())) + 1;
    let format = PixelFormat::from_raw(frame.psm_raw())?;
    if frame.fbw() == 0 || (scissor.scax0() == 0 && scissor.scax1() == 0) {
        return None;
    }
    Some(DumpRect {
        bp_blocks: frame.fbp() * 32,
        bw: frame.fbw(),
        format,
        width: w,
        height: h,
    })
}

fn run(cli: Cli) -> Result<(), RunnerError> {
    if cli.out.is_some() && cli.inputs.len() > 1 {
        return Err(RunnerError::OutWithMultipleInputs);
    }

    for input_path in &cli.inputs {
        let input_str = input_path.display().to_string();
        let src = fs::read_to_string(input_path).map_err(|source| RunnerError::ReadFile {
            path: input_str.clone(),
            source,
        })?;

        let mut gs = GsState::new();
        let surface_hint =
            execute_gsp_script(&mut gs, &src).map_err(|source| RunnerError::ParseGsp {
                path: input_str.clone(),
                source,
            })?;

        let rect = if let Some(explicit) = cli.rect {
            explicit
        } else if let Some(hint) = surface_hint {
            DumpRect {
                bp_blocks: hint.bp_blocks,
                bw: hint.bw,
                format: hint.format,
                width: hint.width,
                height: hint.height,
            }
        } else {
            infer_rect_from_state(&gs)
                .ok_or_else(|| RunnerError::MissingSurface(input_str.clone()))?
        };

        let out_path = if let Some(explicit_out) = &cli.out {
            explicit_out.clone()
        } else {
            let stem = input_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("frame");
            cli.out_dir.join(format!("{stem}.png"))
        };

        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| RunnerError::WriteIo {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
        }

        let out_str = out_path.display().to_string();
        let file = File::create(&out_path).map_err(|source| RunnerError::WriteIo {
            path: out_str.clone(),
            source,
        })?;
        encode_framebuffer_png(
            &gs,
            rect,
            cli.rgba,
            cli.scale,
            BufWriter::new(file),
            &out_str,
        )?;
        println!("wrote {}", out_path.display());
    }

    Ok(())
}

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ps2-gs-runner error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn decode_png_bytes(data: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let decoder = png::Decoder::new(Cursor::new(data));
        let mut reader = decoder.read_info().expect("valid png header");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("valid png frame");
        buf.truncate(info.buffer_size());
        (info, buf)
    }

    #[test]
    fn parse_dump_rect_accepts_named_and_numeric_formats() {
        let r = DumpRect::from_str("2,5,PSMCT32,320,240").expect("valid rect");
        assert_eq!(
            r,
            DumpRect {
                bp_blocks: 64,
                bw: 5,
                format: PixelFormat::Psmct32,
                width: 320,
                height: 240,
            }
        );

        let r16 = DumpRect::from_str("0x1,1,0x02,16,8").expect("hex rect");
        assert_eq!(r16.bp_blocks, 32);
        assert_eq!(r16.format, PixelFormat::Psmct16);

        assert!(DumpRect::from_str("0,1,PSMCT32,0,10").is_err());
        assert!(DumpRect::from_str("bad").is_err());
    }

    #[test]
    fn encode_framebuffer_png_roundtrips_32_24_and_16bit_pixels() {
        let script = "\
            surface 0 1 PSMCT32 4 4
            reg PRMODECONT 1
            reg FRAME_1 0x00000000_00010000
            reg SCISSOR_1 0x00030000_00030000
            reg TEST_1 0x00030000
            reg PRIM 6
            reg XYZ2 0x00000000_00000000
            reg RGBAQ 0x3F800000_801122FF
            reg XYZ2 0x00000000_00200020
        ";
        let mut gs = GsState::new();
        let hint = execute_gsp_script(&mut gs, script)
            .expect("gsp")
            .expect("surface");
        let rect = DumpRect {
            bp_blocks: hint.bp_blocks,
            bw: hint.bw,
            format: hint.format,
            width: hint.width,
            height: hint.height,
        };

        // Default opaque RGB export (R=0xFF, G=0x22, B=0x11 inside [0..2)x[0..2), black outside).
        let mut rgb_png = Vec::new();
        encode_framebuffer_png(&gs, rect, false, 1, &mut rgb_png, "test_rgb").expect("encode rgb");
        let (info, rgb_pixels) = decode_png_bytes(&rgb_png);
        assert_eq!(info.width, 4);
        assert_eq!(info.height, 4);
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(&rgb_pixels[0..3], &[0xFF, 0x22, 0x11]);
        assert_eq!(&rgb_pixels[5 * 3..5 * 3 + 3], &[0xFF, 0x22, 0x11]);
        assert_eq!(
            &rgb_pixels[(4 * 2 + 2) * 3..(4 * 2 + 2) * 3 + 3],
            &[0x00, 0x00, 0x00]
        );

        // RGBA mode preserves GS alpha byte 0x80.
        let mut rgba_png = Vec::new();
        encode_framebuffer_png(&gs, rect, true, 1, &mut rgba_png, "test_rgba")
            .expect("encode rgba");
        let (info_a, rgba_pixels) = decode_png_bytes(&rgba_png);
        assert_eq!(info_a.color_type, png::ColorType::Rgba);
        assert_eq!(&rgba_pixels[0..4], &[0xFF, 0x22, 0x11, 0x80]);

        // 16-bit PSMCT16 decode check: 0x801F -> R=255, G=0, B=0, A=255.
        let mut gs16 = GsState::new();
        gs16.vram
            .write_page_pixel_raw(PixelFormat::Psmct16, 0, 1, 0, 0, 0x801F);
        let rect16 = DumpRect {
            bp_blocks: 0,
            bw: 1,
            format: PixelFormat::Psmct16,
            width: 2,
            height: 2,
        };
        let mut png16 = Vec::new();
        encode_framebuffer_png(&gs16, rect16, true, 1, &mut png16, "test_16").expect("encode 16");
        let (_, pixels16) = decode_png_bytes(&png16);
        assert_eq!(&pixels16[0..4], &[0xFF, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn nearest_neighbor_scale_replicates_pixels_and_rejects_zero() {
        let mut gs = GsState::new();
        // Distinct 2x2 pattern in PSMCT32:
        // (0,0)=#11223344, (1,0)=#55667788, (0,1)=#99AABBCC, (1,1)=#DDEEFF00
        gs.vram
            .write_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 0, 0x4433_2211);
        gs.vram
            .write_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 0, 0x8877_6655);
        gs.vram
            .write_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 0, 1, 0xCCBB_AA99);
        gs.vram
            .write_page_pixel_raw(PixelFormat::Psmct32, 0, 1, 1, 1, 0x00FF_EEDD);

        let rect = DumpRect {
            bp_blocks: 0,
            bw: 1,
            format: PixelFormat::Psmct32,
            width: 2,
            height: 2,
        };
        let mut png_scaled = Vec::new();
        encode_framebuffer_png(&gs, rect, true, 3, &mut png_scaled, "test_scale3")
            .expect("encode 3x");
        let (info, px) = decode_png_bytes(&png_scaled);
        assert_eq!(info.width, 6);
        assert_eq!(info.height, 6);
        assert_eq!(info.color_type, png::ColorType::Rgba);

        for sy in 0..6u32 {
            for sx in 0..6u32 {
                let expected = match (sx / 3, sy / 3) {
                    (0, 0) => [0x11, 0x22, 0x33, 0x44],
                    (1, 0) => [0x55, 0x66, 0x77, 0x88],
                    (0, 1) => [0x99, 0xAA, 0xBB, 0xCC],
                    (1, 1) => [0xDD, 0xEE, 0xFF, 0x00],
                    _ => unreachable!(),
                };
                let idx = ((sy * 6 + sx) * 4) as usize;
                assert_eq!(&px[idx..idx + 4], &expected, "pixel ({sx}, {sy})");
            }
        }

        let mut png_zero = Vec::new();
        assert!(matches!(
            encode_framebuffer_png(&gs, rect, false, 0, &mut png_zero, "test_zero"),
            Err(RunnerError::InvalidScale(0))
        ));
        assert!(Cli::try_parse_from(["ps2-gs-runner", "--scale", "0", "dummy.gsp"]).is_err());
        let parsed =
            Cli::try_parse_from(["ps2-gs-runner", "--scale", "4", "dummy.gsp"]).expect("scale 4");
        assert_eq!(parsed.scale, 4);
    }
}
