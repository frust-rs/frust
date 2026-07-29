//! Integration tests for [`frust_text::TextContext::register_fonts`]
//! (the runtime font-registration seam).
//!
//! Test fonts (`tests/fonts/`) are a subsetted copy of "Tuffy" — a
//! public-domain font (see `tests/fonts/TUFFY-LICENSE.txt`) also used as a
//! test asset by the `fontdb` crate — kept under 50KB via `fonttools
//! pyftsubset` (ASCII glyphs only, hinting/features dropped).
//! `Tuffy-As-Helvetica.ttf` is the same face with its `name` table family
//! entries rewritten to "Helvetica", used only to exercise the
//! registered-shadows-system-family contract deterministically.

use frust_text::{FontError, FontFamily, TextContext, TextStyle};
use kurbo::Point;
use peniko::Color;

/// The registered test font's raw bytes, embedded so `register_fonts`'
/// caller-supplied `Vec<u8>` round-trips through the exact same bytes a
/// resulting `GlyphRun`'s `FontHandle` carries.
const TUFFY: &[u8] = include_bytes!("fonts/Tuffy-Subset.ttf");

/// The same face, renamed to "Helvetica" in its own `name` table — used to
/// prove the registered-family-shadows-system-family contract without
/// depending on whether the test host happens to ship a real "Helvetica".
const TUFFY_AS_HELVETICA: &[u8] = include_bytes!("fonts/Tuffy-As-Helvetica.ttf");

fn style(size: f32, family: FontFamily) -> TextStyle {
    TextStyle {
        family,
        ..TextStyle::new(size, Color::BLACK)
    }
}

/// Extracts the raw font-file bytes a shaped `GlyphRun` resolved against, so
/// a test can assert *which* face actually shaped the text rather than just
/// that shaping succeeded.
fn shaped_font_bytes(cx: &mut TextContext, text: &str, sty: &TextStyle) -> Vec<u8> {
    let layout = cx.layout(text, sty, None);
    let runs = layout.to_scene_runs(Point::ORIGIN);
    let run = runs.first().expect("expected at least one glyph run");
    run.font.font().data.as_ref().to_vec()
}

#[test]
fn registering_valid_ttf_returns_the_family_name_and_shapes_with_it() {
    let mut cx = TextContext::new();

    let families = cx
        .register_fonts(TUFFY.to_vec())
        .expect("valid TTF bytes must register");
    assert_eq!(families.len(), 1, "expected exactly one registered family");
    assert_eq!(families[0].name, "Tuffy");
    assert!(
        families[0].face_count >= 1,
        "expected at least one registered face"
    );

    let registered_style = style(24.0, FontFamily::named("Tuffy"));
    let registered_bytes = shaped_font_bytes(&mut cx, "Hello from Frust", &registered_style);
    assert_eq!(
        registered_bytes, TUFFY,
        "a GlyphRun shaped against the named registered family must resolve to \
         the exact bytes passed to register_fonts"
    );

    // The SystemUi shaping of the same string, in a pristine context, must
    // resolve to a different face's bytes (the platform UI font, not our
    // registered one).
    let mut system_cx = TextContext::new();
    let system_style = style(24.0, FontFamily::SystemUi);
    let system_bytes = shaped_font_bytes(&mut system_cx, "Hello from Frust", &system_style);
    assert_ne!(
        registered_bytes, system_bytes,
        "the registered family's shaping must differ from the SystemUi shaping \
         of the same string"
    );
}

#[test]
fn registered_family_shadows_a_same_named_family() {
    // fontique 0.11 semantics (documented on `register_fonts`): the
    // registered map is checked before the system map, so requesting
    // "Helvetica" after registering a face under that exact name resolves to
    // the registered face regardless of whether the host also ships a real
    // system "Helvetica" — deterministic across every test host.
    let mut cx = TextContext::new();
    cx.register_fonts(TUFFY_AS_HELVETICA.to_vec())
        .expect("valid TTF bytes must register");

    let helvetica_style = style(20.0, FontFamily::named("Helvetica"));
    let bytes = shaped_font_bytes(&mut cx, "Shadowed", &helvetica_style);
    assert_eq!(
        bytes, TUFFY_AS_HELVETICA,
        "requesting the registered family name must resolve to the registered \
         face, shadowing any same-named system family"
    );
}

#[test]
fn registration_invalidates_the_shape_cache() {
    let mut cx = TextContext::new();
    let sty = style(16.0, FontFamily::SystemUi);

    // Prime the cache with a shape.
    let _ = cx.layout("cache me", &sty, None);
    assert_eq!(cx.shape_cache_stats().shapes, 1);

    // Registering fonts must reset the cache — even though this style's key
    // (SystemUi, size 16) doesn't reference the newly registered family, a
    // same-named registered family could change what SystemUi resolves to
    // via generic-family fallback, so the contract is a full clear rather
    // than a keyed invalidation.
    cx.register_fonts(TUFFY.to_vec())
        .expect("valid TTF bytes must register");
    let stats_after_register = cx.shape_cache_stats();
    assert_eq!(
        stats_after_register.shapes, 0,
        "register_fonts must clear the shape cache (and its stats)"
    );

    // Re-requesting the exact same (text, style) that was cached before
    // registration must re-shape, not serve a stale pre-registration hit.
    let _ = cx.layout("cache me", &sty, None);
    assert_eq!(
        cx.shape_cache_stats().shapes,
        1,
        "a layout requested after register_fonts must re-shape, not hit a \
         stale cache entry"
    );
}

#[test]
fn empty_bytes_yield_a_typed_error_not_a_panic() {
    let mut cx = TextContext::new();
    let err = cx
        .register_fonts(Vec::new())
        .expect_err("empty data must not register any face");
    assert_eq!(err, FontError::NoFacesFound);
}

#[test]
fn garbage_bytes_yield_a_typed_error_not_a_panic() {
    let mut cx = TextContext::new();
    // Not a recognized sfnt/OpenType magic (0x00010000, "OTTO", "true",
    // "typ1", "ttcf") — must be rejected as data, never panic the parser.
    let garbage = vec![0xDEu8, 0xAD, 0xBE, 0xEF, 1, 2, 3, 4, 5, 6, 7, 8];
    let err = cx
        .register_fonts(garbage)
        .expect_err("unparseable bytes must not register any face");
    assert_eq!(err, FontError::NoFacesFound);
}
