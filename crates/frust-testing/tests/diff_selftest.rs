//! Self-test for [`frust_testing::diff`], the per-pixel per-channel
//! comparator every golden gate uses.
//!
//! Covers the black-box behavior the comparator promises: identical images
//! pass, a single shifted pixel fails, an alpha-only difference fails (the
//! deliberate departure from upstream's `is_pix_diff`, which would treat two
//! zero-alpha pixels as equal regardless of color), and both the triptych
//! and the JSON diff report are produced on a mismatch. `crates/
//! frust-testing/src/diff.rs`'s own `#[cfg(test)]` module covers the
//! erosion/truncation/bounding-box behavior in more depth; this file is the
//! public-API-level proof the four self-test requirements hold.

use frust_testing::case::Tolerance;
use frust_testing::diff::diff_images;
use image::{Rgba, RgbaImage};

fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(width, height, Rgba(color))
}

#[test]
fn identical_images_pass() {
    let image = solid(8, 8, [12, 34, 56, 255]);
    let outcome = diff_images(&image, &image, Tolerance::new(), false);

    assert!(outcome.passed);
    assert_eq!(outcome.report.pixel_count, 0);
    assert!(outcome.report.bounding_box.is_none());
    assert!(outcome.report.pixels.is_empty());
}

#[test]
fn a_one_pixel_shift_fails() {
    let expected = solid(8, 8, [0, 0, 0, 255]);
    let mut actual = expected.clone();
    // Shifts a single pixel far enough to exceed even a generous tolerance.
    actual.put_pixel(4, 4, Rgba([255, 255, 255, 255]));

    let outcome = diff_images(&expected, &actual, Tolerance::new(), false);

    assert!(!outcome.passed);
    assert_eq!(outcome.report.pixel_count, 1);
    let diff = &outcome.report.pixels[0];
    assert_eq!((diff.x, diff.y), (4, 4));
}

#[test]
fn an_alpha_only_difference_fails() {
    // Same RGB on both sides; only alpha differs, and both sides sit at
    // alpha 0 on the expected image — exactly the case upstream's
    // `is_pix_diff` special-cases away. Frust's comparator must not.
    let expected = solid(4, 4, [200, 100, 50, 0]);
    let mut actual = expected.clone();
    actual.put_pixel(0, 0, Rgba([200, 100, 50, 40]));

    let outcome = diff_images(&expected, &actual, Tolerance::new(), false);

    assert!(!outcome.passed, "an alpha-only change must not be ignored");
    assert_eq!(outcome.report.pixel_count, 1);
    assert_eq!(outcome.report.pixels[0].difference, [0, 0, 0, 40]);
}

#[test]
fn triptych_and_json_are_produced_on_a_mismatch() {
    let expected = solid(3, 2, [0, 0, 0, 255]);
    let mut actual = expected.clone();
    actual.put_pixel(1, 1, Rgba([255, 0, 0, 255]));

    let outcome = diff_images(&expected, &actual, Tolerance::exact(), false);
    assert!(!outcome.passed);

    // Triptych: width*3 x height, expected | diff-marker | actual.
    assert_eq!(outcome.triptych.width(), 9);
    assert_eq!(outcome.triptych.height(), 2);
    assert_eq!(
        outcome.triptych.get_pixel(1, 1).0,
        expected.get_pixel(1, 1).0
    );
    assert_eq!(
        outcome.triptych.get_pixel(3 + 1, 1).0,
        [255, 0, 0, 255],
        "diff-marker column must flag the differing pixel red"
    );
    assert_eq!(
        outcome.triptych.get_pixel(6 + 1, 1).0,
        actual.get_pixel(1, 1).0
    );

    // JSON: the report round-trips through serde_json with the expected
    // shape.
    let json = serde_json::to_string(&outcome.report).expect("serialize DiffReport to JSON");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse DiffReport JSON");
    assert_eq!(value["pixel_count"], 1);
    assert!(value.get("mean_abs_error").is_some());
    assert!(value.get("max_difference").is_some());
    assert!(value.get("bounding_box").is_some());
    assert!(value.get("pixels").is_some());
}
