//! Reusable library interface for `ps2-gs-runner`: `.gsp` PNG dumping, standalone
//! R5900 ELF32 emission (`--emit-elf`), and PCSX2 PNG diff comparison (`--compare-png`).

#![forbid(unsafe_code)]

pub mod elf_emit;
pub mod png_diff;

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use ps2_gs::{
    execute_gsp_script, parse_gsp_script, GsState, GspParseError, GspSurfaceHint, PixelFormat,
};
use thiserror::Error;

use crate::elf_emit::{build_elf_from_gsp, disassemble_stub, ElfEmitError};
use crate::png_diff::{compare_rgb_images, decode_png_rgb, write_rgb_png, PngDiffError, RgbImage};

/// CLI arguments for `ps2-gs-runner`.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "ps2-gs-runner",
    about = "Replay GS .gsp packet scripts, dump PNGs, emit test ELFs, or diff PCSX2 PNGs"
)]
pub struct Cli {
    /// Target framebuffer region `FBP,FBW,PSM,W,H` (`FBP` in pages, `FBW` in 64-px units).
    #[arg(long, value_name = "FBP,FBW,PSM,W,H")]
    pub rect: Option<DumpRect>,

    /// Explicit output file path (PNG in render mode, ELF in `--emit-elf` mode).
    #[arg(short = 'o', long, value_name = "OUT")]
    pub out: Option<PathBuf>,

    /// Output directory when `--out` is omitted.
    #[arg(long, default_value = "local/gs_out")]
    pub out_dir: PathBuf,

    /// Export 4-channel RGBA PNG instead of opaque RGB.
    #[arg(long)]
    pub rgba: bool,

    /// Integer nearest-neighbor upscale factor (`N >= 1`) applied to exported PNG dimensions.
    #[arg(long, default_value_t = 1, value_parser = parse_nonzero_scale, value_name = "N")]
    pub scale: u32,

    /// Emit a standalone PS2 ELF32 (`.elf`) instead of dumping a PNG.
    #[arg(long)]
    pub emit_elf: bool,

    /// Print R5900 disassembly of the generated ELF stub (valid with `--emit-elf`).
    #[arg(long)]
    pub disasm_stub: bool,

    /// Compare replayed `.gsp` output against an external reference PNG (e.g. PCSX2 screenshot).
    #[arg(long, value_name = "REF_PNG")]
    pub compare_png: Option<PathBuf>,

    /// Optional diff PNG output path (darkened reference + yellow ±1 / magenta >1 markers).
    #[arg(long, value_name = "DIFF_PNG")]
    pub diff_out: Option<PathBuf>,

    /// Maximum allowed absolute per-channel difference `max(|dR|,|dG|,|dB|)` in `--compare-png`.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub tolerance: u8,

    /// One or more `.gsp` packet-stream script paths.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
}

/// Target VRAM rectangle specification matching `.gsp surface` / `--rect`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DumpRect {
    pub bp_blocks: u32,
    pub bw: u32,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
}

impl From<GspSurfaceHint> for DumpRect {
    fn from(h: GspSurfaceHint) -> Self {
        Self {
            bp_blocks: h.bp_blocks,
            bw: h.bw,
            format: h.format,
            width: h.width,
            height: h.height,
        }
    }
}

impl From<DumpRect> for GspSurfaceHint {
    fn from(r: DumpRect) -> Self {
        Self {
            bp_blocks: r.bp_blocks,
            bw: r.bw,
            format: r.format,
            width: r.width,
            height: r.height,
        }
    }
}

impl FromStr for DumpRect {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        if parts.len() != 5 {
            return Err("expected FBP,FBW,PSM,W,H (5 comma-separated fields)".to_string());
        }
        let fbp_pages = parse_u32_lit(parts[0])?;
        let bw = parse_u32_lit(parts[1])?;
        let format = parse_psm(parts[2])?;
        let width = parse_u32_lit(parts[3])?;
        let height = parse_u32_lit(parts[4])?;
        if bw == 0 || width == 0 || height == 0 {
            return Err("FBW, W, and H must all be non-zero".to_string());
        }
        Ok(Self {
            bp_blocks: fbp_pages * 32,
            bw,
            format,
            width,
            height,
        })
    }
}

/// Errors produced by `ps2-gs-runner`.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("failed to read '{path}': {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse .gsp script '{path}': {source}")]
    ParseGsp {
        path: String,
        #[source]
        source: GspParseError,
    },
    #[error("`--out` cannot be combined with multiple input scripts")]
    OutWithMultipleInputs,
    #[error("`--compare-png` requires exactly one input `.gsp` script")]
    CompareWithMultipleInputs,
    #[error("`--emit-elf` requires exactly one input `.gsp` script")]
    EmitWithMultipleInputs,
    #[error("`--emit-elf` and `--compare-png` cannot be used together")]
    ConflictingModes,
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
    #[error("ELF emission error: {0}")]
    ElfEmit(#[from] ElfEmitError),
    #[error("PNG comparison error: {0}")]
    PngCompare(#[from] PngDiffError),
    #[error(
        "PNG comparison exceeded tolerance {tolerance}: max per-channel delta was {max_delta} \
         ({mismatch_pixels} pixel(s) outside tolerance)"
    )]
    ToleranceExceeded {
        tolerance: u8,
        max_delta: u8,
        mismatch_pixels: usize,
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
        "PSMT8" | "PSMCT8" => Ok(PixelFormat::Psmt8),
        "PSMT4" | "PSMCT4" => Ok(PixelFormat::Psmt4),
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

/// Convert a native GS VRAM pixel into 8-bit `[R, G, B, A]`.
pub fn decode_pixel_rgba8(raw: u32, format: PixelFormat) -> Result<[u8; 4], RunnerError> {
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

/// Read a GS VRAM region into an opaque 8-bit [`RgbImage`].
pub fn extract_rgb_image(gs: &GsState, rect: DumpRect) -> Result<RgbImage, RunnerError> {
    let total = (rect.width as usize).saturating_mul(rect.height as usize);
    let mut pixels = Vec::with_capacity(total);
    for y in 0..rect.height {
        for x in 0..rect.width {
            let raw = gs
                .vram
                .read_pixel_raw(rect.format, rect.bp_blocks, rect.bw, x, y);
            let [r, g, b, _] = decode_pixel_rgba8(raw, rect.format)?;
            pixels.push([r, g, b]);
        }
    }
    Ok(RgbImage {
        width: rect.width,
        height: rect.height,
        pixels,
    })
}

/// Encode `rect` from `gs.vram` as an RGB or RGBA PNG with integer nearest-neighbor `scale`.
pub fn encode_framebuffer_png<W: Write>(
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

/// Infer framebuffer dump dimensions from `FRAME_1` and `SCISSOR_1` when `surface` is absent.
pub fn infer_rect_from_state(gs: &GsState) -> Option<DumpRect> {
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

fn resolve_dump_rect(
    explicit: Option<DumpRect>,
    surface_hint: Option<GspSurfaceHint>,
    gs: &GsState,
    input_str: &str,
) -> Result<DumpRect, RunnerError> {
    if let Some(rect) = explicit {
        Ok(rect)
    } else if let Some(hint) = surface_hint {
        Ok(hint.into())
    } else {
        infer_rect_from_state(gs).ok_or_else(|| RunnerError::MissingSurface(input_str.to_string()))
    }
}

fn ensure_parent_dir(path: &std::path::Path) -> Result<(), RunnerError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| RunnerError::WriteIo {
                path: parent.display().to_string(),
                source,
            })?;
        }
    }
    Ok(())
}

fn resolve_output_path(cli: &Cli, input_path: &std::path::Path, default_ext: &str) -> PathBuf {
    if let Some(explicit_out) = &cli.out {
        explicit_out.clone()
    } else {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("frame");
        cli.out_dir.join(format!("{stem}.{default_ext}"))
    }
}

fn run_emit_elf(cli: &Cli) -> Result<(), RunnerError> {
    if cli.inputs.len() != 1 {
        return Err(RunnerError::EmitWithMultipleInputs);
    }
    let input_path = &cli.inputs[0];
    let input_str = input_path.display().to_string();
    let src = fs::read_to_string(input_path).map_err(|source| RunnerError::ReadFile {
        path: input_str.clone(),
        source,
    })?;

    let cmds = parse_gsp_script(&src).map_err(|source| RunnerError::ParseGsp {
        path: input_str.clone(),
        source,
    })?;

    let mut gs = GsState::new();
    let surface_hint =
        execute_gsp_script(&mut gs, &src).map_err(|source| RunnerError::ParseGsp {
            path: input_str.clone(),
            source,
        })?;
    let rect = resolve_dump_rect(cli.rect, surface_hint, &gs, &input_str)?;
    let hint: GspSurfaceHint = rect.into();
    let elf = build_elf_from_gsp(&cmds, &hint)?;

    let out_path = resolve_output_path(cli, input_path, "elf");
    ensure_parent_dir(&out_path)?;
    fs::write(&out_path, &elf.bytes).map_err(|source| RunnerError::WriteIo {
        path: out_path.display().to_string(),
        source,
    })?;

    println!(
        "wrote {} ({} bytes, {} priv writes, {} QWC)",
        out_path.display(),
        elf.bytes.len(),
        elf.priv_writes.len(),
        elf.qwc_stream.len()
    );
    if cli.disasm_stub {
        for line in disassemble_stub(&elf.stub_words) {
            println!("  {line}");
        }
    }
    Ok(())
}

fn run_compare_png(cli: &Cli, ref_path: &std::path::Path) -> Result<(), RunnerError> {
    if cli.inputs.len() != 1 {
        return Err(RunnerError::CompareWithMultipleInputs);
    }
    let input_path = &cli.inputs[0];
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
    let rect = resolve_dump_rect(cli.rect, surface_hint, &gs, &input_str)?;
    let expected = extract_rgb_image(&gs, rect)?;

    let ref_str = ref_path.display().to_string();
    let ref_file = File::open(ref_path).map_err(|source| PngDiffError::Io {
        path: ref_str.clone(),
        source,
    })?;
    let reference = decode_png_rgb(BufReader::new(ref_file), &ref_str)?;
    let report = compare_rgb_images(&expected, &reference)?;

    println!("{}", report.format_summary());

    if let Some(diff_path) = &cli.diff_out {
        ensure_parent_dir(diff_path)?;
        write_rgb_png(&report.diff_image, &diff_path.display().to_string())?;
        println!("wrote diff {}", diff_path.display());
    }

    let max_delta = report.max_channel_delta();
    if max_delta > cli.tolerance {
        let mismatch_pixels = report.pixels_exceeding(cli.tolerance);
        return Err(RunnerError::ToleranceExceeded {
            tolerance: cli.tolerance,
            max_delta,
            mismatch_pixels,
        });
    }

    Ok(())
}

fn run_render_pngs(cli: &Cli) -> Result<(), RunnerError> {
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

        let rect = resolve_dump_rect(cli.rect, surface_hint, &gs, &input_str)?;
        let out_path = resolve_output_path(cli, input_path, "png");
        ensure_parent_dir(&out_path)?;

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

/// Dispatch CLI execution in render, `--emit-elf`, or `--compare-png` mode.
pub fn run_cli(cli: Cli) -> Result<(), RunnerError> {
    if cli.emit_elf && cli.compare_png.is_some() {
        return Err(RunnerError::ConflictingModes);
    }
    if cli.emit_elf {
        run_emit_elf(&cli)
    } else if let Some(ref_path) = &cli.compare_png {
        run_compare_png(&cli, ref_path)
    } else {
        run_render_pngs(&cli)
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
        let rect: DumpRect = hint.into();

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

        let mut rgba_png = Vec::new();
        encode_framebuffer_png(&gs, rect, true, 1, &mut rgba_png, "test_rgba")
            .expect("encode rgba");
        let (info_a, rgba_pixels) = decode_png_bytes(&rgba_png);
        assert_eq!(info_a.color_type, png::ColorType::Rgba);
        assert_eq!(&rgba_pixels[0..4], &[0xFF, 0x22, 0x11, 0x80]);

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
        let parsed = Cli::try_parse_from([
            "ps2-gs-runner",
            "--emit-elf",
            "-o",
            "local/pcsx2/04.elf",
            "dummy.gsp",
        ])
        .expect("emit-elf cli");
        assert!(parsed.emit_elf);
        assert_eq!(parsed.out, Some(PathBuf::from("local/pcsx2/04.elf")));
    }
}
