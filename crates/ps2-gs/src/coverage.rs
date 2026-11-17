//! Single source of truth for GS pixel coverage rules.
//!
//! `pixel_center_offset_fp` and top-left edge semantics follow Hans-Kristian Arntzen's
//! (`themaister`) technical documentation of `paraLLEl-GS` (`FETCHED`, opened directly
//! in this session; see `docs/HARDWARE_NOTES.md` Section 14):
//! - *"Triangles are top-left raster, just like modern GPUs."*
//! - *"Pixel center is on integer coordinate, just like D3D9."*
//! - *"Lines use Bresenham's algorithm"*
//! - *"Points snap to nearest pixel. Unsure which rounding is used though ..."*
//!
//! Switch [`ACTIVE_COVERAGE`] to test alternative conventions (half-pixel centers, half-open
//! line endpoints) in Step 4 PCSX2 comparisons without modifying the rasterizer.

/// Pixel-center, point-rounding, top-left fill, sprite, and line-end convention selector.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoverageConvention {
    /// Integer pixel centers at `(px, py)` (D3D9-style, `themaister` `paraLLEl-GS` `FETCHED`),
    /// top-left edge rule on triangles and sprites (`min <= c < max`), nearest-integer point snap,
    /// and Bresenham lines with inclusive endpoints (`G-009`..`G-012`).
    IntegerCenter,
    /// Integer pixel centers at `(px, py)` with half-open Bresenham lines (drops final endpoint).
    IntegerCenterHalfOpenLine,
    /// Half-pixel centers at `(px + 0.5, py + 0.5)` (D3D10+/GL/Vulkan style), floor point snap,
    /// top-left edge rule on triangles and sprites, and half-open Bresenham lines.
    HalfPixelCenterHalfOpenLine,
}

/// Active GS coverage convention used by [`crate::raster`].
///
/// Adopted: matches PCSX2 `v2.8.2` Software on Scene 10 (`7/7` Points, `6/6` Lines, `3/3` Sprites);
/// real PS2 hardware remains unconfirmed (`G-009`, `G-011`). Change this single constant to switch
/// pixel-center, sprite-edge, and line-end rules globally.
pub const ACTIVE_COVERAGE: CoverageConvention = CoverageConvention::IntegerCenterHalfOpenLine;

/// Primitive geometry passed in 12.4 fixed-point screen coordinates (after `XYOFFSET` subtraction).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoveragePrimitive {
    Point {
        x_fp: i32,
        y_fp: i32,
    },
    Line {
        x0_fp: i32,
        y0_fp: i32,
        x1_fp: i32,
        y1_fp: i32,
    },
    Triangle {
        x0_fp: i32,
        y0_fp: i32,
        x1_fp: i32,
        y1_fp: i32,
        x2_fp: i32,
        y2_fp: i32,
    },
    Sprite {
        min_x_fp: i32,
        min_y_fp: i32,
        max_x_fp: i32,
        max_y_fp: i32,
    },
}

/// Sample center offset `(cx_sub, cy_sub)` in 12.4 subpixel units (`0` = integer center, `8` = `+0.5`).
#[inline]
pub const fn sample_center_offset_fp(conv: CoverageConvention) -> i32 {
    match conv {
        CoverageConvention::IntegerCenter | CoverageConvention::IntegerCenterHalfOpenLine => 0,
        CoverageConvention::HalfPixelCenterHalfOpenLine => 8,
    }
}

/// Return sample coordinates `(cx_fp, cy_fp)` in 12.4 subpixel units for integer pixel `(px, py)`.
#[inline]
pub const fn pixel_sample_fp(px: i32, py: i32, conv: CoverageConvention) -> (i64, i64) {
    let off = sample_center_offset_fp(conv) as i64;
    ((px as i64) * 16 + off, (py as i64) * 16 + off)
}

/// Snap a 12.4 fixed-point coordinate to its owning integer pixel index under `conv`.
#[inline]
pub const fn snap_coord_to_pixel(coord_fp: i32, conv: CoverageConvention) -> i32 {
    let center_off = sample_center_offset_fp(conv);
    // Voronoi cell centered at `(p * 16 + center_off)` spans `[p*16 + center_off - 8, ... + 8)`.
    (coord_fp - center_off + 8).div_euclid(16)
}

/// Oriented 2D cross product `(bx - ax)*(py - ay) - (by - ay)*(px - ax)`.
///
/// Positive when `p` lies to the clockwise interior side of directed edge `a -> b` in GS
/// screen coordinates (`+X` right, `+Y` down).
#[inline]
pub const fn edge_fn(ax: i64, ay: i64, bx: i64, by: i64, px: i64, py: i64) -> i64 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

#[inline]
const fn is_top_left_cw(dx: i64, dy: i64) -> bool {
    dy < 0 || (dy == 0 && dx > 0)
}

#[inline]
const fn line_includes_end(conv: CoverageConvention) -> bool {
    matches!(conv, CoverageConvention::IntegerCenter)
}

/// Single-source-of-truth coverage test defining pixel centers, triangle top-left rules,
/// sprite rectangle boundaries, point snapping, and Bresenham line endpoints.
pub fn is_pixel_covered(
    prim: &CoveragePrimitive,
    px: i32,
    py: i32,
    conv: CoverageConvention,
) -> bool {
    match *prim {
        CoveragePrimitive::Point { x_fp, y_fp } => {
            px == snap_coord_to_pixel(x_fp, conv) && py == snap_coord_to_pixel(y_fp, conv)
        }
        CoveragePrimitive::Line {
            x0_fp,
            y0_fp,
            x1_fp,
            y1_fp,
        } => {
            let ix0 = snap_coord_to_pixel(x0_fp, conv);
            let iy0 = snap_coord_to_pixel(y0_fp, conv);
            let ix1 = snap_coord_to_pixel(x1_fp, conv);
            let iy1 = snap_coord_to_pixel(y1_fp, conv);
            let include_end = line_includes_end(conv);

            let dx = i64::from(ix1).abs_diff(i64::from(ix0)) as i64;
            let dy = i64::from(iy1).abs_diff(i64::from(iy0)) as i64;

            if dx == 0 && dy == 0 {
                return include_end && px == ix0 && py == iy0;
            }
            if !include_end && px == ix1 && py == iy1 {
                return false;
            }

            if dx >= dy {
                let min_x = ix0.min(ix1);
                let max_x = ix0.max(ix1);
                if px < min_x || px > max_x {
                    return false;
                }
                let t = i64::from(px - ix0).abs();
                let sy = i64::from(iy1 - iy0).signum();
                let expected_y = i64::from(iy0) + sy * ((2 * t * dy + dx) / (2 * dx));
                i64::from(py) == expected_y
            } else {
                let min_y = iy0.min(iy1);
                let max_y = iy0.max(iy1);
                if py < min_y || py > max_y {
                    return false;
                }
                let t = i64::from(py - iy0).abs();
                let sx = i64::from(ix1 - ix0).signum();
                let expected_x = i64::from(ix0) + sx * ((2 * t * dx + dy) / (2 * dy));
                i64::from(px) == expected_x
            }
        }
        CoveragePrimitive::Triangle {
            x0_fp,
            y0_fp,
            x1_fp,
            y1_fp,
            x2_fp,
            y2_fp,
        } => {
            let (x0, y0) = (i64::from(x0_fp), i64::from(y0_fp));
            let (mut x1, mut y1) = (i64::from(x1_fp), i64::from(y1_fp));
            let (mut x2, mut y2) = (i64::from(x2_fp), i64::from(y2_fp));

            let area2 = edge_fn(x0, y0, x1, y1, x2, y2);
            if area2 == 0 {
                return false;
            }
            if area2 < 0 {
                core::mem::swap(&mut x1, &mut x2);
                core::mem::swap(&mut y1, &mut y2);
            }

            let (cx, cy) = pixel_sample_fp(px, py, conv);
            let e0 = edge_fn(x1, y1, x2, y2, cx, cy);
            let e1 = edge_fn(x2, y2, x0, y0, cx, cy);
            let e2 = edge_fn(x0, y0, x1, y1, cx, cy);

            let c0 = e0 > 0 || (e0 == 0 && is_top_left_cw(x2 - x1, y2 - y1));
            let c1 = e1 > 0 || (e1 == 0 && is_top_left_cw(x0 - x2, y0 - y2));
            let c2 = e2 > 0 || (e2 == 0 && is_top_left_cw(x1 - x0, y1 - y0));
            c0 && c1 && c2
        }
        CoveragePrimitive::Sprite {
            min_x_fp,
            min_y_fp,
            max_x_fp,
            max_y_fp,
        } => {
            let (cx, cy) = pixel_sample_fp(px, py, conv);
            cx >= i64::from(min_x_fp)
                && cx < i64::from(max_x_fp)
                && cy >= i64::from(min_y_fp)
                && cy < i64::from(max_y_fp)
        }
    }
}

/// Evaluate coverage using [`ACTIVE_COVERAGE`].
#[inline]
pub fn is_pixel_covered_active(prim: &CoveragePrimitive, px: i32, py: i32) -> bool {
    is_pixel_covered(prim, px, py, ACTIVE_COVERAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_convention_changes_line_end_and_subpixel_center_rules() {
        let hline = CoveragePrimitive::Line {
            x0_fp: 0,
            y0_fp: 0,
            x1_fp: 4 * 16,
            y1_fp: 0,
        };
        assert!(is_pixel_covered(
            &hline,
            4,
            0,
            CoverageConvention::IntegerCenter
        ));
        assert!(!is_pixel_covered(
            &hline,
            4,
            0,
            CoverageConvention::IntegerCenterHalfOpenLine
        ));
        assert!(!is_pixel_covered(
            &hline,
            4,
            0,
            CoverageConvention::HalfPixelCenterHalfOpenLine
        ));

        // Subpixel point at (0.75, 0.25): integer center rounds to x=1; half-pixel center floors to x=0.
        let pt = CoveragePrimitive::Point { x_fp: 12, y_fp: 4 };
        assert!(is_pixel_covered(
            &pt,
            1,
            0,
            CoverageConvention::IntegerCenter
        ));
        assert!(!is_pixel_covered(
            &pt,
            0,
            0,
            CoverageConvention::IntegerCenter
        ));
        assert!(is_pixel_covered(
            &pt,
            0,
            0,
            CoverageConvention::HalfPixelCenterHalfOpenLine
        ));

        // Subpixel sprite [0.5, 2.5) x [0, 2): integer covers x in {1, 2}; half-pixel covers x in {0, 1}.
        let sp = CoveragePrimitive::Sprite {
            min_x_fp: 8,
            min_y_fp: 0,
            max_x_fp: 2 * 16 + 8,
            max_y_fp: 2 * 16,
        };
        assert!(!is_pixel_covered(
            &sp,
            0,
            0,
            CoverageConvention::IntegerCenter
        ));
        assert!(is_pixel_covered(
            &sp,
            1,
            0,
            CoverageConvention::IntegerCenter
        ));
        assert!(is_pixel_covered(
            &sp,
            2,
            0,
            CoverageConvention::IntegerCenter
        ));
        assert!(is_pixel_covered(
            &sp,
            0,
            0,
            CoverageConvention::HalfPixelCenterHalfOpenLine
        ));
        assert!(!is_pixel_covered(
            &sp,
            2,
            0,
            CoverageConvention::HalfPixelCenterHalfOpenLine
        ));
    }

    #[test]
    fn bresenham_y_major_and_zero_area_triangle() {
        let steep = CoveragePrimitive::Line {
            x0_fp: 16,
            y0_fp: 16,
            x1_fp: 2 * 16,
            y1_fp: 4 * 16,
        };
        let mut pixels = Vec::new();
        for y in 0..=5 {
            for x in 0..=4 {
                if is_pixel_covered(&steep, x, y, CoverageConvention::IntegerCenter) {
                    pixels.push((x, y));
                }
            }
        }
        assert_eq!(pixels, vec![(1, 1), (1, 2), (2, 3), (2, 4)]);

        let degenerate = CoveragePrimitive::Triangle {
            x0_fp: 0,
            y0_fp: 0,
            x1_fp: 32,
            y1_fp: 32,
            x2_fp: 64,
            y2_fp: 64,
        };
        for y in 0..5 {
            for x in 0..5 {
                assert!(!is_pixel_covered(
                    &degenerate,
                    x,
                    y,
                    CoverageConvention::IntegerCenter
                ));
            }
        }
    }
}
