//! Regression test: color emoji SHAPING works end-to-end through the pinned
//! parley/fontique stack with **no changes to the render path**
//! (`frust-render`/`frust-scene`).
//!
//! What this proves is shaping-side fallback: parley auto-tags emoji clusters
//! `GenericFamily::Emoji`, and fontique's CoreText backend on macOS resolves
//! that to Apple Color Emoji with zero manual font registration, yielding real
//! (non-`.notdef`) glyphs in a per-run-font `GlyphRun` — the correct
//! granularity for a mixed text+emoji string. No `frust-text` fix was needed
//! (`TextContext`'s default fontique setup already resolves emoji fallback
//! correctly on this host).
//!
//! Rendering those glyphs is the engine's business and is NOT covered here:
//! the engine's glyph path is glifo-based, and its color/bitmap-glyph coverage
//! is tracked in `docs/LIMITATIONS.md`'s `engine-bitmap-glyphs-gap` (this test
//! predates the engine swap, when the classic renderer drew COLR/sbix color
//! glyphs natively).
//!
//! **CBDT/Android caveat**: this only proves fallback resolution on this
//! (macOS/Apple Color Emoji, an sbix bitmap-strike font) host. CBDT (the bitmap
//! emoji table format Android system fonts typically use) has **not** been
//! verified on-device — that remains a device-truth item.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use frust_text::{FontFamily, TextContext, TextStyle};
use kurbo::Point;
use peniko::Color;

/// Skips announced so far; a test that cannot run bumps it instead of passing silently.
static SKIPS: AtomicUsize = AtomicUsize::new(0);

/// How many tests skipped for a missing host precondition.
fn skips() -> usize {
    SKIPS.load(Ordering::SeqCst)
}

/// The missing precondition of the system-fallback test, else `None`: a host emoji
/// font resolves U+1F600 to a face other than the plain-text one. Probed once.
fn missing_precondition() -> Option<String> {
    static CHECK: OnceLock<Option<String>> = OnceLock::new();
    CHECK
        .get_or_init(|| {
            let mut cx = TextContext::new();
            let text_runs = cx
                .layout("hi", &style(24.0), None)
                .to_scene_runs(Point::ORIGIN);
            let text_font = text_runs.first()?.font.font().clone();
            let runs = cx
                .layout("hi \u{1F600}", &style(24.0), None)
                .to_scene_runs(Point::ORIGIN);
            let found = runs.iter().any(|run| *run.font.font() != text_font);
            (!found).then(|| "no host emoji font resolves U+1F600".to_string())
        })
        .clone()
}

/// True when the host has what `test` needs; otherwise prints one counted
/// `SKIPPED (n)` line naming the missing precondition and returns false.
/// macOS always ships Apple Color Emoji, so a missing precondition there fails.
fn host_ready(test: &str) -> bool {
    let Some(missing) = missing_precondition() else {
        return true;
    };
    if cfg!(target_os = "macos") {
        panic!("{test}: macOS must resolve a host emoji font, but: {missing}");
    }
    let n = SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
    println!("SKIPPED ({n}): {test}: {missing}");
    false
}

fn style(size: f32) -> TextStyle {
    TextStyle::new(size, Color::BLACK)
}

/// The bundled COLRv1 emoji subset (U+1F600 only), renamed so it cannot
/// collide with a host font.
const SUBSET: &[u8] = include_bytes!("../../../testing/fonts/NotoEmoji-COLRv1-Subset.ttf");
const SUBSET_FAMILY: &str = "Frust Test Emoji COLR";

/// `.notdef` is glyph id 0 by TrueType/OpenType convention — the tofu-box
/// glyph a font falls back to when it has no outline/bitmap for a codepoint.
const NOTDEF_GLYPH_ID: u32 = 0;

/// Shapes a plain-text-only string and a string with an embedded emoji
/// cluster (including a skin-tone modifier sequence, the ZWJ-adjacent case
/// the task calls out) through the same [`TextContext`], and asserts:
///
/// - fallback actually engages: at least one run in the mixed string
///   resolves to a font different from the plain-text run's font, and
/// - that fallback run's glyphs are real: non-empty, no `.notdef` ids, and a
///   positive advance between glyphs (not degenerate zero-width tofu boxes).
///
/// The `.notdef`/advance checks are host-portable and always run. An
/// additional macOS-only assertion (behind `cfg(target_os = "macos")`)
/// double-checks the resolved fallback font's own `name` table family name
/// contains "Emoji", confirming CoreText picked Apple Color Emoji
/// specifically rather than merely a font with *some* glyph for the
/// codepoint.
#[test]
fn emoji_cluster_engages_fallback_font_with_real_glyphs() {
    if !host_ready("emoji_cluster_engages_fallback_font_with_real_glyphs") {
        return;
    }
    let mut cx = TextContext::new();

    // Baseline: the font resolved for plain ASCII text, no emoji involved.
    let text_only = cx.layout("hi", &style(24.0), None);
    let text_runs = text_only.to_scene_runs(Point::ORIGIN);
    let text_font = text_runs
        .first()
        .expect("expected a glyph run for plain text \"hi\"")
        .font
        .font()
        .clone();

    // "hi " + thumbs-up-medium-skin-tone (a modifier-sequence emoji cluster)
    // + a plain emoji — exercises both a modified and unmodified color-emoji
    // cluster in the same shaped string.
    let mixed = cx.layout("hi \u{1F44D}\u{1F3FD} \u{1F389}", &style(24.0), None);
    let runs = mixed.to_scene_runs(Point::ORIGIN);
    assert!(!runs.is_empty(), "expected glyph runs for mixed text+emoji");

    let emoji_run = runs
        .iter()
        .find(|run| *run.font.font() != text_font)
        .expect(
            "expected at least one run in the mixed string to resolve to a \
             font different from the plain-text font -- fallback did not \
             engage for the emoji cluster",
        );

    assert!(
        !emoji_run.glyphs.is_empty(),
        "the emoji fallback run has no glyphs"
    );
    for glyph in &emoji_run.glyphs {
        assert_ne!(
            glyph.id, NOTDEF_GLYPH_ID,
            "emoji glyph resolved to .notdef (glyph id 0) -- a tofu box, not \
             a real color glyph"
        );
    }
    if emoji_run.glyphs.len() >= 2 {
        let x0 = emoji_run.glyphs[0].x;
        let x1 = emoji_run.glyphs[1].x;
        assert!(
            x1 > x0,
            "expected a positive advance between emoji glyphs, got {x0} -> {x1}"
        );
    }

    #[cfg(target_os = "macos")]
    {
        let family = sfnt_family_name(emoji_run.font.font()).expect(
            "failed to parse a family name out of the emoji run's font \
             `name` table",
        );
        assert!(
            family.contains("Emoji"),
            "expected the macOS emoji-fallback font's family name to \
             contain \"Emoji\" (i.e. CoreText resolved Apple Color Emoji \
             specifically), got {family:?}"
        );
    }
}

/// Host-independent: the bundled subset, registered with the context, serves
/// an emoji cluster that the plain-text font cannot, with real glyphs. The
/// subset names its own family in the style so no host emoji font is needed.
#[test]
fn registered_subset_serves_emoji_cluster_with_real_glyphs() {
    let mut cx = TextContext::new();
    let families = cx
        .register_fonts(SUBSET.to_vec())
        .expect("the bundled emoji subset registers");
    assert!(
        families.iter().any(|f| f.name == SUBSET_FAMILY),
        "expected family {SUBSET_FAMILY:?}, got {:?}",
        families.iter().map(|f| &f.name).collect::<Vec<_>>()
    );

    let text_runs = cx
        .layout("hi", &style(24.0), None)
        .to_scene_runs(Point::ORIGIN);
    let text_font = text_runs
        .first()
        .expect("expected a glyph run for plain text \"hi\"")
        .font
        .font()
        .clone();

    let mut emoji_style = style(24.0);
    emoji_style.family = FontFamily::named(SUBSET_FAMILY);
    let runs = cx
        .layout("hi \u{1F600}", &emoji_style, None)
        .to_scene_runs(Point::ORIGIN);
    let emoji_run = runs
        .iter()
        .find(|run| *run.font.font() != text_font)
        .expect("the registered emoji subset did not serve the emoji cluster");
    // The served face is the registered subset itself, not a host emoji font.
    assert_eq!(
        emoji_run.font.font().data.data(),
        SUBSET,
        "the emoji cluster was served by a face other than the registered subset"
    );
    assert!(!emoji_run.glyphs.is_empty(), "the emoji run has no glyphs");
    for glyph in &emoji_run.glyphs {
        assert_ne!(glyph.id, NOTDEF_GLYPH_ID, "emoji glyph resolved to .notdef");
    }
}

/// Reports the skip counter so a skipped run is visible in captured output. Tests run in
/// parallel, so the count is what had been announced by now; when the host is ready no
/// test can have skipped, which is asserted.
#[test]
fn zz_skip_summary() {
    match missing_precondition() {
        None => {
            println!("SKIP SUMMARY: 0 skipped, the host has every precondition");
            assert_eq!(skips(), 0, "a ready host never skips");
        }
        Some(why) => println!(
            "SKIP SUMMARY: {} skipped so far; the system-fallback test skips here: {why}",
            skips()
        ),
    }
}

/// Minimal, hand-rolled OpenType `name`-table reader — just enough to
/// recover a font's family name (nameID 1, the canonical family name) for
/// the macOS-only assertion above.
///
/// No font-parsing crate is added for this (task constraint: no new
/// dependencies) — this reads the handful of bytes the sfnt table
/// directory + naming-table format-0 records require directly out of the
/// `peniko::FontData` blob already on hand. Handles both a bare sfnt and a
/// TrueType Collection (`ttcf`) container, since macOS ships Apple Color
/// Emoji as a `.ttc` — `FontData::index` selects which collection member to
/// read.
#[cfg(target_os = "macos")]
fn sfnt_family_name(font: &peniko::FontData) -> Option<String> {
    let data = font.data.data();
    let index = font.index as usize;

    let base = if data.get(0..4)? == b"ttcf" {
        let num_fonts = u32::from_be_bytes(data.get(8..12)?.try_into().ok()?) as usize;
        if index >= num_fonts {
            return None;
        }
        let offset_rec = 12 + index * 4;
        u32::from_be_bytes(data.get(offset_rec..offset_rec + 4)?.try_into().ok()?) as usize
    } else {
        0
    };

    let num_tables = u16::from_be_bytes(data.get(base + 4..base + 6)?.try_into().ok()?) as usize;
    let mut name_table_offset = None;
    for i in 0..num_tables {
        let rec = base + 12 + i * 16;
        let tag = data.get(rec..rec + 4)?;
        if tag == b"name" {
            let offset = u32::from_be_bytes(data.get(rec + 8..rec + 12)?.try_into().ok()?) as usize;
            name_table_offset = Some(offset);
            break;
        }
    }
    let table = data.get(name_table_offset?..)?;

    let count = u16::from_be_bytes(table.get(2..4)?.try_into().ok()?) as usize;
    let string_offset = u16::from_be_bytes(table.get(4..6)?.try_into().ok()?) as usize;

    let mut fallback: Option<String> = None;
    for i in 0..count {
        let rec = 6 + i * 12;
        let platform_id = u16::from_be_bytes(table.get(rec..rec + 2)?.try_into().ok()?);
        let name_id = u16::from_be_bytes(table.get(rec + 6..rec + 8)?.try_into().ok()?);
        let length = u16::from_be_bytes(table.get(rec + 8..rec + 10)?.try_into().ok()?) as usize;
        let str_offset =
            u16::from_be_bytes(table.get(rec + 10..rec + 12)?.try_into().ok()?) as usize;
        if name_id != 1 && name_id != 16 {
            continue;
        }
        let start = string_offset + str_offset;
        let bytes = table.get(start..start + length)?;
        let decoded = if platform_id == 1 {
            // Macintosh platform: single-byte, ASCII-compatible for the
            // font names this check cares about.
            bytes.iter().map(|&b| b as char).collect::<String>()
        } else {
            // Windows/Unicode platforms: UTF-16BE.
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16(&units).ok()?
        };
        if name_id == 1 {
            return Some(decoded);
        }
        fallback = Some(decoded);
    }
    fallback
}
