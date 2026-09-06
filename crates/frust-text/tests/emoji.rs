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

use frust_text::{TextContext, TextStyle};
use kurbo::Point;
use peniko::Color;

fn style(size: f32) -> TextStyle {
    TextStyle::new(size, Color::BLACK)
}

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
