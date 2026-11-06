//! The CoreText colour-bitmap rasterizer behind Apple emoji: the module is
//! compiled in directly (no public seam is added to the crate) and driven
//! against the installed `AppleColorEmoji` face.
//!
//! What is pinned:
//!
//! - **A colour glyph comes back as saturated, opaque, premultiplied RGBA.**
//! - **The size limit refuses.** A `max_dim` below the glyph's bounds is `None`.
//! - **Channel order is RGBA.** A red square is mostly red, a blue one blue.
//! - **A glyph with no ink is refused**, and an outline face yields coverage
//!   in black rather than colour.
//!
//! The tests print why and pass when the face is not installed. The three
//! tests that draw an emoji glyph are ignored on macOS, where CoreText's
//! `sbix` bitmap draw faults inside ImageIO; they run on iOS, e.g. in the
//! simulator with `CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER` set to a
//! `xcrun simctl spawn <device>` wrapper and `--target aarch64-apple-ios-sim`.

#![cfg(target_vendor = "apple")]
#![allow(dead_code)]

#[path = "../src/text/apple_raster.rs"]
mod apple_raster;

use apple_raster::{glyph_count, glyph_for_char, rasterize_glyph};

const EMOJI: &str = "AppleColorEmoji";
const HIDDEN_EMOJI: &str = ".AppleColorEmojiUI";

fn emoji(character: char, ppem: u16, max_dim: u32) -> Option<apple_raster::RasterGlyph> {
    let Some(glyph) = glyph_for_char(EMOJI, character) else {
        eprintln!("AppleColorEmoji does not map {character:?} here; skipping");
        return None;
    };
    let count = glyph_count(EMOJI)?;
    rasterize_glyph(EMOJI, glyph, count, ppem, max_dim)
}

/// Mean (r, g, b) over the texels that are at least half opaque, as 0..=255.
fn mean_colour(glyph: &apple_raster::RasterGlyph) -> (f64, f64, f64) {
    let (mut r, mut g, mut b, mut n) = (0.0, 0.0, 0.0, 0.0);
    for texel in glyph.pixmap.data().iter().filter(|t| t.a >= 128) {
        let a = f64::from(texel.a) / 255.0;
        r += f64::from(texel.r) / a;
        g += f64::from(texel.g) / a;
        b += f64::from(texel.b) / a;
        n += 1.0;
    }
    assert!(n > 0.0, "no opaque texels");
    (r / n, g / n, b / n)
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "CoreText sbix bitmap draws fault on macOS; run on iOS"
)]
fn colour_emoji_rasterizes_to_coloured_opaque_texels() {
    let Some(glyph) = emoji('\u{1F600}', 64, 256) else {
        return;
    };
    let (w, h) = (glyph.pixmap.width(), glyph.pixmap.height());
    assert!(w > 0 && h > 0 && u32::from(w) <= 256 && u32::from(h) <= 256);
    assert_eq!(glyph.pixmap.data().len(), usize::from(w) * usize::from(h));
    let coloured = glyph
        .pixmap
        .data()
        .iter()
        .filter(|t| {
            let hi = t.r.max(t.g).max(t.b);
            let lo = t.r.min(t.g).min(t.b);
            t.a == 255 && hi - lo > 60
        })
        .count();
    assert!(coloured > 50, "only {coloured} saturated opaque texels");
    for t in glyph.pixmap.data() {
        assert!(t.r <= t.a && t.g <= t.a && t.b <= t.a, "not premultiplied");
    }
    // The image sits on the baseline's upper side and spans the pen.
    assert!(glyph.origin_y > 0, "origin_y {}", glyph.origin_y);
    assert!(glyph.origin_x <= 0 || i32::from(w) > glyph.origin_x);
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "CoreText sbix bitmap draws fault on macOS; run on iOS"
)]
fn a_max_dim_below_the_glyph_refuses() {
    let Some(glyph) = emoji('\u{1F600}', 64, 256) else {
        return;
    };
    let smaller = u32::from(glyph.pixmap.width().max(glyph.pixmap.height())) - 1;
    assert!(emoji('\u{1F600}', 64, smaller).is_none());
    assert!(emoji('\u{1F600}', 64, 0).is_none());
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "CoreText sbix bitmap draws fault on macOS; run on iOS"
)]
fn channel_order_decodes_known_hues() {
    if let Some(red) = emoji('\u{1F7E5}', 64, 256) {
        let (r, g, b) = mean_colour(&red);
        assert!(
            r > 150.0 && r > g * 1.8 && r > b * 1.8,
            "red square {r} {g} {b}"
        );
    }
    if let Some(blue) = emoji('\u{1F7E6}', 64, 256) {
        let (r, g, b) = mean_colour(&blue);
        assert!(
            b > 150.0 && b > r * 1.5 && b > g * 1.2,
            "blue square {r} {g} {b}"
        );
    }
}

#[test]
fn a_zero_ppem_and_an_inkless_glyph_refuse() {
    let Some(glyph) = glyph_for_char(EMOJI, '\u{1F600}') else {
        return;
    };
    assert!(rasterize_glyph(EMOJI, glyph, glyph_count(EMOJI).unwrap_or(0), 0, 256).is_none());
    if let Some(space) = glyph_for_char("Helvetica", ' ') {
        let count = glyph_count("Helvetica").unwrap_or(0);
        assert!(rasterize_glyph("Helvetica", space, count, 64, 256).is_none());
    }
}

#[test]
fn an_outline_face_yields_black_coverage() {
    let Some(a) = glyph_for_char("Helvetica", 'A') else {
        eprintln!("Helvetica not available; skipping");
        return;
    };
    let glyph = rasterize_glyph(
        "Helvetica",
        a,
        glyph_count("Helvetica").unwrap_or(0),
        48,
        256,
    )
    .expect("outline glyph rasterizes");
    assert!(glyph.pixmap.data().iter().any(|t| t.a == 255));
    assert!(
        glyph
            .pixmap
            .data()
            .iter()
            .all(|t| t.r == 0 && t.g == 0 && t.b == 0)
    );
}

#[test]
fn a_name_that_opens_a_different_font_refuses() {
    let Some(a) = glyph_for_char("Helvetica", 'A') else {
        return;
    };
    let count = glyph_count("Helvetica").unwrap_or(0);
    assert!(rasterize_glyph("NoSuchFont-Regular", a, count, 48, 256).is_none());
    assert!(glyph_for_char("NoSuchFont-Regular", 'A').is_none());
}

#[test]
fn a_wrong_expected_glyph_count_refuses() {
    let Some(glyph) = glyph_for_char(EMOJI, '\u{1F600}') else {
        return;
    };
    let count = glyph_count(EMOJI).unwrap_or(0);
    // A wrong count is refused before any drawing.
    assert!(rasterize_glyph(HIDDEN_EMOJI, glyph, count.wrapping_add(1), 64, 256).is_none());
    // An exact name match with the wrong count is refused too.
    assert!(rasterize_glyph(EMOJI, glyph, count.wrapping_add(1), 64, 256).is_none());
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "CoreText sbix bitmap draws fault on macOS; run on iOS"
)]
fn the_hidden_emoji_face_draws_through_its_public_sibling() {
    let Some(glyph) = glyph_for_char(EMOJI, '\u{1F600}') else {
        return;
    };
    let Some(count) = glyph_count(EMOJI) else {
        return;
    };
    let public = rasterize_glyph(EMOJI, glyph, count, 64, 256).expect("public face draws");
    let hidden = rasterize_glyph(HIDDEN_EMOJI, glyph, count, 64, 256)
        .expect("hidden face draws via its public sibling");
    assert_eq!(
        (public.pixmap.width(), public.pixmap.height()),
        (hidden.pixmap.width(), hidden.pixmap.height())
    );
    assert_eq!(
        (public.origin_x, public.origin_y),
        (hidden.origin_x, hidden.origin_y)
    );
    assert!(
        hidden
            .pixmap
            .data()
            .iter()
            .any(|t| t.a == 255 && t.r.max(t.g).max(t.b) - t.r.min(t.g).min(t.b) > 60),
        "not coloured"
    );
}
