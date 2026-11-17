//! PNG pixel-comparison and diff-image diagnostics for PCSX2 software screenshots.
//!
//! Supports direct `1:1` comparison, uniform integer-upscale normalization, black CRTC border
//! cropping around rendered bounding boxes, explicit non-uniform scaling errors, diagnostic shift
//! hints, and yellow/magenta (`±1` LSB / `>1`) diff PNG visualization.

use std::fs::File;
use std::io::{BufWriter, Read};

use thiserror::Error;

/// Decoded 8-bit RGB image owned in row-major `[r, g, b]` order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
}

impl RgbImage {
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y as usize) * (self.width as usize) + (x as usize)]
    }
}

/// First-mismatch sample entry formatted for diagnostic reports.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PixelMismatch {
    pub x: u32,
    pub y: u32,
    pub expected: [u8; 3],
    pub actual: [u8; 3],
}

/// Summary report emitted by [`compare_rgb_images`].
#[derive(Clone, Debug, PartialEq)]
pub struct PngDiffReport {
    pub expected_dims: (u32, u32),
    pub reference_dims: (u32, u32),
    pub detected_scale: (u32, u32),
    pub detected_crop: (u32, u32),
    pub total_pixels: usize,
    pub exact_pixels: usize,
    pub pm1_pixels: usize,
    pub gt1_pixels: usize,
    pub max_abs_delta: [u8; 3],
    pub mae: f64,
    pub rmse: f64,
    pub first_mismatches: Vec<PixelMismatch>,
    pub best_shift_hint: Option<(i32, i32, usize)>,
    pub delta_hist: [usize; 256],
    pub diff_image: RgbImage,
}

impl PngDiffReport {
    pub fn pixels_exceeding(&self, tolerance: u8) -> usize {
        self.delta_hist[(usize::from(tolerance) + 1)..].iter().sum()
    }

    pub fn exact_pct(&self) -> f64 {
        if self.total_pixels == 0 {
            100.0
        } else {
            (self.exact_pixels as f64) * 100.0 / (self.total_pixels as f64)
        }
    }

    pub fn max_channel_delta(&self) -> u8 {
        self.max_abs_delta[0]
            .max(self.max_abs_delta[1])
            .max(self.max_abs_delta[2])
    }

    pub fn format_summary(&self) -> String {
        let mut out = format!(
            "matched={}x{} ref={}x{} scale={}x{} crop=({}, {}) exact={}/{} ({:.2}%) pm1={} gt1={} max_delta=({},{},{}) mae={:.4} rmse={:.4}",
            self.expected_dims.0,
            self.expected_dims.1,
            self.reference_dims.0,
            self.reference_dims.1,
            self.detected_scale.0,
            self.detected_scale.1,
            self.detected_crop.0,
            self.detected_crop.1,
            self.exact_pixels,
            self.total_pixels,
            self.exact_pct(),
            self.pm1_pixels,
            self.gt1_pixels,
            self.max_abs_delta[0],
            self.max_abs_delta[1],
            self.max_abs_delta[2],
            self.mae,
            self.rmse
        );
        if let Some((dx, dy, shifted_mismatches)) = self.best_shift_hint {
            out.push_str(&format!(
                "\nshift_hint: ref shifted by ({dx}, {dy}) would have {shifted_mismatches} mismatched pixels"
            ));
        }
        for m in &self.first_mismatches {
            out.push_str(&format!(
                "\n  diff ({}, {}): ours=#{:02X}{:02X}{:02X} pcsx2=#{:02X}{:02X}{:02X}",
                m.x,
                m.y,
                m.expected[0],
                m.expected[1],
                m.expected[2],
                m.actual[0],
                m.actual[1],
                m.actual[2],
            ));
        }
        out
    }
}

/// Errors encountered while decoding or normalizing a reference PNG.
#[derive(Debug, Error)]
pub enum PngDiffError {
    #[error("I/O error reading PNG '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode PNG '{path}': {source}")]
    Decode {
        path: String,
        #[source]
        source: png::DecodingError,
    },
    #[error("unsupported PNG color type {color_type:?} in '{path}'")]
    UnsupportedColorType {
        path: String,
        color_type: png::ColorType,
    },
    #[error("I/O error writing diff PNG '{path}': {source}")]
    WriteIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to encode diff PNG '{path}': {source}")]
    Encode {
        path: String,
        #[source]
        source: png::EncodingError,
    },
    #[error(
        "reference PNG ({actual_w}x{actual_h}) is smaller than expected surface ({exp_w}x{exp_h})"
    )]
    SmallerThanExpected {
        exp_w: u32,
        exp_h: u32,
        actual_w: u32,
        actual_h: u32,
    },
    #[error(
        "non-uniform upscaling or bilinear filtering detected in {actual_w}x{actual_h} reference PNG \
         (expected integer multiple/crop of {exp_w}x{exp_h}; disable bilinear/display filtering in PCSX2)"
    )]
    NonUniformScaling {
        exp_w: u32,
        exp_h: u32,
        actual_w: u32,
        actual_h: u32,
    },
    #[error(
        "could not align {actual_w}x{actual_h} reference PNG cleanly to {exp_w}x{exp_h} surface \
         (not an exact uniform integer scale and black-border alignment failed)"
    )]
    IncompatibleDimensions {
        exp_w: u32,
        exp_h: u32,
        actual_w: u32,
        actual_h: u32,
    },
}

/// Decode an 8-bit PNG from any reader into an [`RgbImage`].
pub fn decode_png_rgb<R: Read>(reader: R, label: &str) -> Result<RgbImage, PngDiffError> {
    let mut decoder = png::Decoder::new(reader);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut png_reader = decoder.read_info().map_err(|source| PngDiffError::Decode {
        path: label.to_string(),
        source,
    })?;
    let mut raw = vec![0u8; png_reader.output_buffer_size()];
    let info = png_reader
        .next_frame(&mut raw)
        .map_err(|source| PngDiffError::Decode {
            path: label.to_string(),
            source,
        })?;
    raw.truncate(info.buffer_size());

    let pixel_count = (info.width as usize) * (info.height as usize);
    let mut pixels = Vec::with_capacity(pixel_count);
    match info.color_type {
        png::ColorType::Rgb => {
            for chunk in raw.chunks_exact(3) {
                pixels.push([chunk[0], chunk[1], chunk[2]]);
            }
        }
        png::ColorType::Rgba => {
            for chunk in raw.chunks_exact(4) {
                pixels.push([chunk[0], chunk[1], chunk[2]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in &raw {
                pixels.push([g, g, g]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in raw.chunks_exact(2) {
                pixels.push([chunk[0], chunk[0], chunk[0]]);
            }
        }
        other => {
            return Err(PngDiffError::UnsupportedColorType {
                path: label.to_string(),
                color_type: other,
            });
        }
    }

    Ok(RgbImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// Write an [`RgbImage`] out as an 8-bit RGB PNG file.
pub fn write_rgb_png(img: &RgbImage, path: &str) -> Result<(), PngDiffError> {
    let file = File::create(path).map_err(|source| PngDiffError::WriteIo {
        path: path.to_string(),
        source,
    })?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), img.width, img.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|source| PngDiffError::Encode {
            path: path.to_string(),
            source,
        })?;
    let mut flat = Vec::with_capacity(img.pixels.len() * 3);
    for px in &img.pixels {
        flat.extend_from_slice(px);
    }
    writer
        .write_image_data(&flat)
        .map_err(|source| PngDiffError::Encode {
            path: path.to_string(),
            source,
        })
}

fn try_uniform_downsample(
    img: &RgbImage,
    crop_x: u32,
    crop_y: u32,
    exp_w: u32,
    exp_h: u32,
    sx: u32,
    sy: u32,
) -> Option<RgbImage> {
    let span_w = exp_w.checked_mul(sx)?;
    let span_h = exp_h.checked_mul(sy)?;
    if crop_x.checked_add(span_w)? > img.width || crop_y.checked_add(span_h)? > img.height {
        return None;
    }

    let mut out = Vec::with_capacity((exp_w as usize) * (exp_h as usize));
    for y in 0..exp_h {
        let base_y = crop_y + y * sy;
        for x in 0..exp_w {
            let base_x = crop_x + x * sx;
            let first = img.get(base_x, base_y);
            for dy in 0..sy {
                for dx in 0..sx {
                    if img.get(base_x + dx, base_y + dy) != first {
                        return None;
                    }
                }
            }
            out.push(first);
        }
    }
    Some(RgbImage {
        width: exp_w,
        height: exp_h,
        pixels: out,
    })
}

fn non_black_bbox(img: &RgbImage) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;
    for y in 0..img.height {
        for x in 0..img.width {
            if img.get(x, y) != [0, 0, 0] {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    any.then_some((min_x, min_y, max_x, max_y))
}

fn outer_region_is_black(img: &RgbImage, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
    for y in 0..img.height {
        for x in 0..img.width {
            if (x < x0 || x >= x1 || y < y0 || y >= y1) && img.get(x, y) != [0, 0, 0] {
                return false;
            }
        }
    }
    true
}

struct NormalizedReference {
    image: RgbImage,
    scale: (u32, u32),
    crop: (u32, u32),
}

fn normalize_reference_image(
    expected: &RgbImage,
    reference: &RgbImage,
) -> Result<NormalizedReference, PngDiffError> {
    if reference.width < expected.width || reference.height < expected.height {
        return Err(PngDiffError::SmallerThanExpected {
            exp_w: expected.width,
            exp_h: expected.height,
            actual_w: reference.width,
            actual_h: reference.height,
        });
    }

    if reference.width == expected.width && reference.height == expected.height {
        return Ok(NormalizedReference {
            image: reference.clone(),
            scale: (1, 1),
            crop: (0, 0),
        });
    }

    // 1. Exact integer multiple: check uniform block replication.
    if reference.width % expected.width == 0 && reference.height % expected.height == 0 {
        let sx = reference.width / expected.width;
        let sy = reference.height / expected.height;
        if let Some(downsampled) =
            try_uniform_downsample(reference, 0, 0, expected.width, expected.height, sx, sy)
        {
            return Ok(NormalizedReference {
                image: downsampled,
                scale: (sx, sy),
                crop: (0, 0),
            });
        }
        return Err(PngDiffError::NonUniformScaling {
            exp_w: expected.width,
            exp_h: expected.height,
            actual_w: reference.width,
            actual_h: reference.height,
        });
    }

    // 2. Overscan / CRTC black-border crop search aligned to rendered bounding box.
    if let (Some((emin_x, emin_y, _, _)), Some((rmin_x, rmin_y, _, _))) =
        (non_black_bbox(expected), non_black_bbox(reference))
    {
        for sx in 1..=(reference.width / expected.width).min(8) {
            for sy in 1..=(reference.height / expected.height).min(8) {
                let Some(crop_x) = rmin_x.checked_sub(emin_x * sx) else {
                    continue;
                };
                let Some(crop_y) = rmin_y.checked_sub(emin_y * sy) else {
                    continue;
                };
                let x1 = crop_x + expected.width * sx;
                let y1 = crop_y + expected.height * sy;
                if x1 <= reference.width
                    && y1 <= reference.height
                    && outer_region_is_black(reference, crop_x, crop_y, x1, y1)
                {
                    if let Some(downsampled) = try_uniform_downsample(
                        reference,
                        crop_x,
                        crop_y,
                        expected.width,
                        expected.height,
                        sx,
                        sy,
                    ) {
                        return Ok(NormalizedReference {
                            image: downsampled,
                            scale: (sx, sy),
                            crop: (crop_x, crop_y),
                        });
                    }
                    return Err(PngDiffError::NonUniformScaling {
                        exp_w: expected.width,
                        exp_h: expected.height,
                        actual_w: reference.width,
                        actual_h: reference.height,
                    });
                }
            }
        }
    }

    Err(PngDiffError::IncompatibleDimensions {
        exp_w: expected.width,
        exp_h: expected.height,
        actual_w: reference.width,
        actual_h: reference.height,
    })
}

fn find_best_shift_hint(
    expected: &RgbImage,
    aligned: &RgbImage,
    direct_mismatches: usize,
) -> Option<(i32, i32, usize)> {
    if direct_mismatches == 0 {
        return None;
    }
    let w = expected.width as i32;
    let h = expected.height as i32;
    let mut best: Option<(i32, i32, usize)> = None;

    for dy in -4..=4i32 {
        for dx in -4..=4i32 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let mut count = 0usize;
            for y in 0..h {
                let sy = y + dy;
                for x in 0..w {
                    let sx = x + dx;
                    let r = if sx >= 0 && sx < w && sy >= 0 && sy < h {
                        aligned.get(sx as u32, sy as u32)
                    } else {
                        [0, 0, 0]
                    };
                    if expected.get(x as u32, y as u32) != r {
                        count += 1;
                    }
                }
            }
            if count < direct_mismatches && best.map_or(true, |b| count < b.2) {
                best = Some((dx, dy, count));
            }
        }
    }
    best
}

/// Default maximum number of mismatched coordinates recorded in [`PngDiffReport::first_mismatches`].
pub const DEFAULT_MAX_MISMATCHES: usize = 16;

/// Normalize `reference` against `expected` and compute pixel-level diff metrics and diff image,
/// keeping up to [`DEFAULT_MAX_MISMATCHES`] mismatch coordinates.
pub fn compare_rgb_images(
    expected: &RgbImage,
    reference: &RgbImage,
) -> Result<PngDiffReport, PngDiffError> {
    compare_rgb_images_with_limit(expected, reference, DEFAULT_MAX_MISMATCHES)
}

/// Normalize `reference` against `expected` and compute pixel-level diff metrics and diff image,
/// keeping up to `max_mismatches` mismatch coordinates in scanline order.
pub fn compare_rgb_images_with_limit(
    expected: &RgbImage,
    reference: &RgbImage,
    max_mismatches: usize,
) -> Result<PngDiffReport, PngDiffError> {
    let NormalizedReference {
        image: aligned,
        scale: detected_scale,
        crop: detected_crop,
    } = normalize_reference_image(expected, reference)?;

    let total_pixels = expected.pixels.len();
    let mut exact_pixels = 0usize;
    let mut pm1_pixels = 0usize;
    let mut gt1_pixels = 0usize;
    let mut max_abs_delta = [0u8; 3];
    let mut abs_sum = 0u64;
    let mut sq_sum = 0u64;
    let mut first_mismatches = Vec::new();
    let mut delta_hist = [0usize; 256];
    let mut diff_pixels = Vec::with_capacity(total_pixels);

    for y in 0..expected.height {
        for x in 0..expected.width {
            let e = expected.get(x, y);
            let a = aligned.get(x, y);
            let dr = e[0].abs_diff(a[0]);
            let dg = e[1].abs_diff(a[1]);
            let db = e[2].abs_diff(a[2]);
            max_abs_delta[0] = max_abs_delta[0].max(dr);
            max_abs_delta[1] = max_abs_delta[1].max(dg);
            max_abs_delta[2] = max_abs_delta[2].max(db);

            abs_sum += u64::from(dr) + u64::from(dg) + u64::from(db);
            sq_sum += u64::from(dr) * u64::from(dr)
                + u64::from(dg) * u64::from(dg)
                + u64::from(db) * u64::from(db);

            let max_c = dr.max(dg).max(db);
            delta_hist[usize::from(max_c)] += 1;
            if max_c == 0 {
                exact_pixels += 1;
                diff_pixels.push([e[0] / 4, e[1] / 4, e[2] / 4]);
            } else {
                if max_c == 1 {
                    pm1_pixels += 1;
                    diff_pixels.push([0xFF, 0xE0, 0x40]);
                } else {
                    gt1_pixels += 1;
                    diff_pixels.push([0xFF, 0x20, 0x80]);
                }
                if first_mismatches.len() < max_mismatches {
                    first_mismatches.push(PixelMismatch {
                        x,
                        y,
                        expected: e,
                        actual: a,
                    });
                }
            }
        }
    }

    let channel_samples = (total_pixels * 3).max(1) as f64;
    let mae = (abs_sum as f64) / channel_samples;
    let rmse = ((sq_sum as f64) / channel_samples).sqrt();
    let best_shift_hint = find_best_shift_hint(expected, &aligned, pm1_pixels + gt1_pixels);

    Ok(PngDiffReport {
        expected_dims: (expected.width, expected.height),
        reference_dims: (reference.width, reference.height),
        detected_scale,
        detected_crop,
        total_pixels,
        exact_pixels,
        pm1_pixels,
        gt1_pixels,
        max_abs_delta,
        mae,
        rmse,
        first_mismatches,
        best_shift_hint,
        delta_hist,
        diff_image: RgbImage {
            width: expected.width,
            height: expected.height,
            pixels: diff_pixels,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pattern(w: u32, h: u32) -> RgbImage {
        let mut pixels = vec![[0u8, 0, 0]; (w * h) as usize];
        pixels[(2 * w + 3) as usize] = [100, 150, 200];
        pixels[(5 * w + 6) as usize] = [255, 40, 80];
        RgbImage {
            width: w,
            height: h,
            pixels,
        }
    }

    fn upscale(img: &RgbImage, sx: u32, sy: u32) -> RgbImage {
        let mut pixels = Vec::with_capacity((img.width * sx * img.height * sy) as usize);
        for y in 0..img.height {
            for _ in 0..sy {
                for x in 0..img.width {
                    let px = img.get(x, y);
                    for _ in 0..sx {
                        pixels.push(px);
                    }
                }
            }
        }
        RgbImage {
            width: img.width * sx,
            height: img.height * sy,
            pixels,
        }
    }

    #[test]
    fn direct_and_uniform_integer_upscale_and_border_crop_normalize_cleanly() {
        let exp = make_pattern(8, 8);

        // 1:1 exact match.
        let r1 = compare_rgb_images(&exp, &exp).expect("1:1 diff");
        assert_eq!(r1.detected_scale, (1, 1));
        assert_eq!(r1.detected_crop, (0, 0));
        assert_eq!(r1.exact_pixels, 64);

        // 2x1 uniform scale match.
        let ref_2x1 = upscale(&exp, 2, 1);
        let r2 = compare_rgb_images(&exp, &ref_2x1).expect("2x1 diff");
        assert_eq!(r2.detected_scale, (2, 1));
        assert_eq!(r2.exact_pixels, 64);

        // Bordered 2x2 upscale inside a 20x18 canvas with black margins.
        let ref_2x2 = upscale(&exp, 2, 2);
        let mut bordered = vec![[0u8, 0, 0]; 20 * 18];
        for y in 0..ref_2x2.height {
            for x in 0..ref_2x2.width {
                bordered[((y + 1) * 20 + (x + 2)) as usize] = ref_2x2.get(x, y);
            }
        }
        let bordered_img = RgbImage {
            width: 20,
            height: 18,
            pixels: bordered,
        };
        let r3 = compare_rgb_images(&exp, &bordered_img).expect("cropped 2x2 diff");
        assert_eq!(r3.detected_scale, (2, 2));
        assert_eq!(r3.detected_crop, (2, 1));
        assert_eq!(r3.exact_pixels, 64);
    }

    #[test]
    fn rejects_non_uniform_filtered_image_and_marks_pm1_vs_gt1_diff_colors() {
        let exp = make_pattern(8, 8);
        let mut bad_2x2 = upscale(&exp, 2, 2);
        // Corrupt one sub-pixel inside a 2x2 block so bilinear/filtered scaling is rejected.
        bad_2x2.pixels[1] = [1, 1, 1];
        assert!(matches!(
            compare_rgb_images(&exp, &bad_2x2),
            Err(PngDiffError::NonUniformScaling { .. })
        ));

        let mut mod_ref = exp.clone();
        mod_ref.pixels[0] = [1, 0, 0]; // delta 1 -> yellow #FFE040
        mod_ref.pixels[2 * 8 + 3] = [100, 155, 200]; // delta 5 -> magenta #FF2080
        let r = compare_rgb_images(&exp, &mod_ref).expect("diff colors");
        assert_eq!(r.pm1_pixels, 1);
        assert_eq!(r.gt1_pixels, 1);
        assert_eq!(r.max_abs_delta, [1, 5, 0]);
        assert_eq!(r.diff_image.get(0, 0), [0xFF, 0xE0, 0x40]);
        assert_eq!(r.diff_image.get(3, 2), [0xFF, 0x20, 0x80]);
        assert_eq!(r.diff_image.get(6, 5), [255 / 4, 40 / 4, 80 / 4]);
    }

    #[test]
    fn compare_rgb_images_with_limit_caps_recorded_mismatches() {
        let exp = make_pattern(8, 8);
        let mut modified = exp.clone();
        modified.pixels[..8].fill([10, 0, 0]);
        let r_zero = compare_rgb_images_with_limit(&exp, &modified, 0).expect("limit 0");
        assert_eq!(r_zero.gt1_pixels, 8);
        assert!(r_zero.first_mismatches.is_empty());

        let r_three = compare_rgb_images_with_limit(&exp, &modified, 3).expect("limit 3");
        assert_eq!(r_three.gt1_pixels, 8);
        assert_eq!(r_three.first_mismatches.len(), 3);
        assert_eq!(
            (r_three.first_mismatches[2].x, r_three.first_mismatches[2].y),
            (2, 0)
        );

        let r_many = compare_rgb_images_with_limit(&exp, &modified, 64).expect("limit 64");
        assert_eq!(r_many.first_mismatches.len(), 8);
    }
}
