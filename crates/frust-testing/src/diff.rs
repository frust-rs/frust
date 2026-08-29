//! Per-pixel, per-channel image comparator.
//!
//! Ported in shape from upstream's `sparse_strips/vello_sparse_tests/tests/
//! util.rs` (`get_diff`/`is_pix_diff`): per-pixel absolute difference against
//! a threshold, a count of differing pixels checked against a budget, and a
//! `width*3 x height` triptych (expected | diff-marked | actual). Three
//! deliberate departures from that shape:
//!
//! 1. **All four channels are compared, alpha included.** Upstream's
//!    `is_pix_diff` skips the whole pixel when both alpha values are `0`.
//!    Frust cannot make that assumption: `Command::ClearRect` is an alpha
//!    contract (a transparent clear must read back as `(0, 0, 0, 0)`,
//!    not merely "some fully-transparent color"), so an alpha regression at
//!    alpha `0` must still be visible to the comparator. [`Tolerance`]
//!    already carries a separate `alpha` threshold from `channel` for
//!    exactly this reason.
//! 2. **Mean absolute error per channel and a bounding box of changed
//!    pixels** are always computed (`docs/TESTING.md`'s Comparison
//!    section requires both alongside the mismatch count/percentage).
//! 3. **[`DiffReport::pixels`] is capped** at [`MAX_RECORDED_PIXELS`]
//!    entries; a report over the cap sets [`DiffReport::truncated`] instead
//!    of growing an unbounded per-pixel list for a large mismatch.
//!
//! [`diff_images`]'s `eroded_interior` option 1-px-erodes the raw
//! above-threshold mask before it is used for [`DiffReport::pixel_count`]/
//! the pass-fail budget (though never for [`DiffReport::max_difference`]/
//! [`DiffReport::mean_abs_error`], which stay whole-image diagnostics
//! regardless) — a thin, single-pixel-wide ring of above-threshold pixels
//! (the common shape of an antialiased-edge difference) eroded away leaves
//! only an at-least-2px-thick interior difference, the perceptual pair's
//! way of ignoring edge jitter while still catching a real regression.

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::case::Tolerance;

/// [`DiffReport::pixels`] never grows past this many entries; a mismatch
/// wider than this sets [`DiffReport::truncated`] instead.
pub const MAX_RECORDED_PIXELS: usize = 2_000;

/// The inclusive pixel-coordinate bounds of every changed pixel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

/// One differing pixel: its coordinate, both source values, and their
/// signed per-channel difference (`actual - expected`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PixelDiff {
    pub x: u32,
    pub y: u32,
    /// RGBA from the expected (golden/reference) image.
    pub expected: [u8; 4],
    /// RGBA from the actual (rendered) image.
    pub actual: [u8; 4],
    /// Per-channel `actual - expected`, signed.
    pub difference: [i16; 4],
}

/// The full result of comparing two images: how many pixels differ (and
/// where), whole-image error magnitude, and a capped list of the individual
/// differences for review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiffReport {
    /// Count of pixels the comparator flagged as differing — after erosion,
    /// when [`diff_images`] was called with `eroded_interior: true`.
    pub pixel_count: usize,
    /// `pixel_count` as a percentage of the compared image's total pixels.
    pub mismatched_percent: f64,
    /// Maximum absolute per-channel difference `[R, G, B, A]`, over every
    /// pixel in the image regardless of threshold or erosion.
    pub max_difference: [i16; 4],
    /// Mean absolute per-channel difference `[R, G, B, A]`, over every pixel
    /// in the image regardless of threshold or erosion.
    pub mean_abs_error: [f64; 4],
    /// Bounding box of the pixels counted into `pixel_count`; `None` when
    /// `pixel_count` is `0`.
    pub bounding_box: Option<BoundingBox>,
    /// Up to [`MAX_RECORDED_PIXELS`] of the differing pixels, in row-major
    /// order.
    pub pixels: Vec<PixelDiff>,
    /// `true` when more than [`MAX_RECORDED_PIXELS`] pixels differed and
    /// `pixels` was capped rather than growing further.
    pub truncated: bool,
}

/// The outcome of [`diff_images`]: the pass/fail verdict, the [`DiffReport`],
/// and the rendered triptych artifact.
pub struct DiffOutcome {
    /// `true` when `report.pixel_count` is within `tolerance.diff_pixels`.
    pub passed: bool,
    pub report: DiffReport,
    /// A `width*3 x height` image: expected (left third) | diff marker (red
    /// on differing pixels, black elsewhere; middle third) | actual (right
    /// third).
    pub triptych: RgbaImage,
}

/// Reads pixel `(x, y)` from `image`, or fully-transparent black when the
/// coordinate falls outside it — lets [`diff_images`] compare
/// mismatched-size images the same way upstream's `get_diff` does, by
/// treating a missing pixel as maximally different from any opaque one.
fn pixel_or_transparent(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    if x < image.width() && y < image.height() {
        image.get_pixel(x, y).0
    } else {
        [0, 0, 0, 0]
    }
}

/// Whether `expected`/`actual` differ by more than `tolerance` allows —
/// `tolerance.channel` for R/G/B, `tolerance.alpha` for A, no special case
/// for a shared alpha of `0` (see this module's doc comment).
fn is_pixel_diff(expected: [u8; 4], actual: [u8; 4], tolerance: &Tolerance) -> bool {
    let mut different = false;
    for i in 0..3 {
        different |= expected[i].abs_diff(actual[i]) > tolerance.channel;
    }
    different |= expected[3].abs_diff(actual[3]) > tolerance.alpha;
    different
}

/// 1-px erodes a row-major `width x height` boolean mask (4-connected
/// structuring element): a pixel stays `true` only if it and all four of its
/// up/down/left/right neighbors were `true`; an out-of-bounds neighbor
/// counts as `false`, so a `true` region touching the image border erodes
/// there too.
fn erode(mask: &[bool], width: u32, height: u32) -> Vec<bool> {
    let get = |x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
            false
        } else {
            mask[(y as u32 * width + x as u32) as usize]
        }
    };

    let mut eroded = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if !mask[idx] {
                continue;
            }
            let (xi, yi) = (i64::from(x), i64::from(y));
            eroded[idx] = get(xi - 1, yi) && get(xi + 1, yi) && get(xi, yi - 1) && get(xi, yi + 1);
        }
    }
    eroded
}

/// Compares `expected` against `actual` under `tolerance`, returning the
/// pass/fail verdict, a [`DiffReport`], and a triptych artifact.
///
/// `eroded_interior` 1-px-erodes the above-threshold mask before it is used
/// for `pixel_count`/the pass-fail budget and the triptych's diff-marker
/// column — see this module's doc comment. `max_difference`/`mean_abs_error`
/// are always whole-image statistics, erosion or not.
pub fn diff_images(
    expected: &RgbaImage,
    actual: &RgbaImage,
    tolerance: Tolerance,
    eroded_interior: bool,
) -> DiffOutcome {
    let width = expected.width().max(actual.width());
    let height = expected.height().max(actual.height());

    let mut triptych = RgbaImage::new(width.saturating_mul(3), height);
    let total_pixels = (width as usize) * (height as usize);

    let mut expected_px = vec![[0u8; 4]; total_pixels];
    let mut actual_px = vec![[0u8; 4]; total_pixels];
    let mut differs = vec![false; total_pixels];

    let mut sum_abs = [0f64; 4];
    let mut max_diff = [0i16; 4];

    for y in 0..height {
        for x in 0..width {
            let exp = pixel_or_transparent(expected, x, y);
            let act = pixel_or_transparent(actual, x, y);

            triptych.put_pixel(x, y, Rgba(exp));
            triptych.put_pixel(x + 2 * width, y, Rgba(act));

            for i in 0..4 {
                let abs = i16::from(exp[i]).abs_diff(i16::from(act[i]));
                #[allow(clippy::cast_possible_wrap)]
                let abs = abs as i16;
                sum_abs[i] += f64::from(abs);
                max_diff[i] = max_diff[i].max(abs);
            }

            let idx = (y * width + x) as usize;
            expected_px[idx] = exp;
            actual_px[idx] = act;
            differs[idx] = is_pixel_diff(exp, act, &tolerance);
        }
    }

    let mean_abs_error: [f64; 4] = if total_pixels == 0 {
        [0.0; 4]
    } else {
        std::array::from_fn(|i| sum_abs[i] / total_pixels as f64)
    };

    let effective_mask = if eroded_interior {
        erode(&differs, width, height)
    } else {
        differs
    };

    let mut pixel_count = 0usize;
    let mut pixels = Vec::new();
    let mut truncated = false;
    let mut bounding_box: Option<BoundingBox> = None;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if !effective_mask[idx] {
                triptych.put_pixel(x + width, y, Rgba([0, 0, 0, 255]));
                continue;
            }

            pixel_count += 1;
            triptych.put_pixel(x + width, y, Rgba([255, 0, 0, 255]));

            bounding_box = Some(match bounding_box {
                None => BoundingBox {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                },
                Some(b) => BoundingBox {
                    min_x: b.min_x.min(x),
                    min_y: b.min_y.min(y),
                    max_x: b.max_x.max(x),
                    max_y: b.max_y.max(y),
                },
            });

            if pixels.len() < MAX_RECORDED_PIXELS {
                let expected = expected_px[idx];
                let actual = actual_px[idx];
                let difference =
                    std::array::from_fn(|i| i16::from(actual[i]) - i16::from(expected[i]));
                pixels.push(PixelDiff {
                    x,
                    y,
                    expected,
                    actual,
                    difference,
                });
            } else {
                truncated = true;
            }
        }
    }

    let mismatched_percent = if total_pixels == 0 {
        0.0
    } else {
        (pixel_count as f64 / total_pixels as f64) * 100.0
    };

    let passed = (pixel_count as u64) <= u64::from(tolerance.diff_pixels);

    DiffOutcome {
        passed,
        report: DiffReport {
            pixel_count,
            mismatched_percent,
            max_difference: max_diff,
            mean_abs_error,
            bounding_box,
            pixels,
            truncated,
        },
        triptych,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    #[test]
    fn identical_images_report_zero_diff_and_pass() {
        let image = solid(4, 4, [10, 20, 30, 255]);
        let outcome = diff_images(&image, &image, Tolerance::new(), false);
        assert!(outcome.passed);
        assert_eq!(outcome.report.pixel_count, 0);
        assert_eq!(outcome.report.max_difference, [0, 0, 0, 0]);
        assert_eq!(outcome.report.mean_abs_error, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(outcome.report.bounding_box, None);
        assert!(outcome.report.pixels.is_empty());
        assert!(!outcome.report.truncated);
        assert_eq!(outcome.triptych.width(), 12);
        assert_eq!(outcome.triptych.height(), 4);
    }

    #[test]
    fn a_single_shifted_pixel_fails_under_exact_tolerance() {
        let mut expected = solid(4, 4, [0, 0, 0, 255]);
        let actual = expected.clone();
        expected.put_pixel(1, 1, Rgba([50, 0, 0, 255]));

        let outcome = diff_images(&expected, &actual, Tolerance::exact(), false);
        assert!(!outcome.passed);
        assert_eq!(outcome.report.pixel_count, 1);
        assert_eq!(
            outcome.report.bounding_box,
            Some(BoundingBox {
                min_x: 1,
                min_y: 1,
                max_x: 1,
                max_y: 1
            })
        );
        assert_eq!(outcome.report.pixels.len(), 1);
        assert_eq!(outcome.report.pixels[0].difference, [-50, 0, 0, 0]);
    }

    #[test]
    fn alpha_only_difference_at_zero_alpha_still_fails() {
        // Upstream's is_pix_diff would treat two (0,0,0,0)-vs-(0,0,0,alpha)
        // pixels as equal once alpha is `0` on either side; frust's
        // Command::ClearRect alpha contract requires this to be caught.
        let expected = solid(2, 2, [0, 0, 0, 0]);
        let mut actual = solid(2, 2, [0, 0, 0, 0]);
        actual.put_pixel(0, 0, Rgba([0, 0, 0, 10]));

        let outcome = diff_images(&expected, &actual, Tolerance::new(), false);
        assert!(!outcome.passed);
        assert_eq!(outcome.report.pixel_count, 1);
        assert_eq!(outcome.report.pixels[0].difference, [0, 0, 0, 10]);
    }

    #[test]
    fn eroded_interior_drops_a_one_pixel_wide_ring_but_keeps_a_solid_block() {
        let tolerance = Tolerance::exact();

        // A single isolated differing pixel has no fully-differing
        // neighborhood, so erosion drops it entirely.
        let mut expected = solid(5, 5, [0, 0, 0, 255]);
        let mut actual = expected.clone();
        actual.put_pixel(2, 2, Rgba([255, 0, 0, 255]));
        let isolated = diff_images(&expected, &actual, tolerance, true);
        assert_eq!(isolated.report.pixel_count, 0);
        assert!(isolated.passed);

        // A solid 3x3 block of differing pixels keeps its 1x1 interior after
        // one round of erosion.
        expected = solid(5, 5, [0, 0, 0, 255]);
        actual = expected.clone();
        for y in 1..4 {
            for x in 1..4 {
                actual.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let block = diff_images(&expected, &actual, tolerance, true);
        assert_eq!(block.report.pixel_count, 1);
        assert_eq!(
            block.report.bounding_box,
            Some(BoundingBox {
                min_x: 2,
                min_y: 2,
                max_x: 2,
                max_y: 2
            })
        );

        // Without erosion the same block reports all 9 pixels.
        let unfiltered = diff_images(&expected, &actual, tolerance, false);
        assert_eq!(unfiltered.report.pixel_count, 9);
    }

    #[test]
    fn max_and_mean_error_are_whole_image_regardless_of_erosion() {
        let expected = solid(3, 1, [0, 0, 0, 255]);
        let mut actual = expected.clone();
        actual.put_pixel(0, 0, Rgba([30, 0, 0, 255]));

        let with_erosion = diff_images(&expected, &actual, Tolerance::exact(), true);
        let without_erosion = diff_images(&expected, &actual, Tolerance::exact(), false);
        assert_eq!(with_erosion.report.max_difference[0], 30);
        assert_eq!(without_erosion.report.max_difference[0], 30);
        assert_eq!(
            with_erosion.report.mean_abs_error,
            without_erosion.report.mean_abs_error
        );
        // Erosion still removes the isolated pixel from the counted budget.
        assert_eq!(with_erosion.report.pixel_count, 0);
        assert_eq!(without_erosion.report.pixel_count, 1);
    }

    #[test]
    fn a_wide_mismatch_is_capped_and_marked_truncated() {
        let expected = solid(100, 100, [0, 0, 0, 255]);
        let actual = solid(100, 100, [255, 255, 255, 255]);

        let outcome = diff_images(&expected, &actual, Tolerance::exact(), false);
        assert!(!outcome.passed);
        assert_eq!(outcome.report.pixel_count, 10_000);
        assert_eq!(outcome.report.pixels.len(), MAX_RECORDED_PIXELS);
        assert!(outcome.report.truncated);
    }

    #[test]
    fn diff_pixels_budget_allows_a_bounded_mismatch_to_pass() {
        let mut expected = solid(4, 4, [0, 0, 0, 255]);
        let actual = expected.clone();
        expected.put_pixel(0, 0, Rgba([255, 0, 0, 255]));

        let tolerance = Tolerance::exact().with_diff_pixels(1);
        let outcome = diff_images(&expected, &actual, tolerance, false);
        assert!(outcome.passed);
        assert_eq!(outcome.report.pixel_count, 1);
    }

    #[test]
    fn mismatched_dimensions_treat_missing_pixels_as_transparent() {
        let expected = solid(2, 1, [10, 10, 10, 255]);
        let actual = solid(1, 1, [10, 10, 10, 255]);

        let outcome = diff_images(&expected, &actual, Tolerance::exact(), false);
        assert!(!outcome.passed);
        assert_eq!(outcome.report.pixel_count, 1);
        assert_eq!(outcome.triptych.width(), 6);
        let missing = &outcome.report.pixels[0];
        assert_eq!((missing.x, missing.y), (1, 0));
        assert_eq!(missing.actual, [0, 0, 0, 0]);
    }

    #[test]
    fn report_serializes_to_json() {
        let expected = solid(2, 2, [0, 0, 0, 255]);
        let mut actual = expected.clone();
        actual.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let outcome = diff_images(&expected, &actual, Tolerance::exact(), false);

        let json = serde_json::to_string(&outcome.report).expect("serialize DiffReport");
        assert!(json.contains("\"pixel_count\":1"));
        let parsed: DiffReport = serde_json::from_str(&json).expect("deserialize DiffReport");
        assert_eq!(parsed, outcome.report);
    }
}
