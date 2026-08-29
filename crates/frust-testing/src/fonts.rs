//! Bundled, permissively-licensed test fonts for deterministic text goldens.
//!
//! `docs/TESTING.md`'s Deterministic Inputs section: Frust resolves
//! [`frust_text::FontFamily::SystemUi`] through the platform font collection,
//! so a golden/host test that shapes text against it is runner-local (a
//! different installed font package changes the pixels) and not a portable
//! reference image. This module is the bundled-font registration path that
//! closes that gap: four subsetted faces (`testing/fonts/`, full provenance
//! and license text in `testing/fonts/LICENSES.md`) covering Latin +
//! combining marks, Arabic (RTL + joining), CJK, and COLRv1 colour emoji —
//! together enough to shape every script exercised by
//! [`tests::shapes_every_bundled_script_with_no_system_font`] below with
//! zero dependency on what the host happens to have installed.
//!
//! [`test_fonts`] exposes the raw bytes (e.g. for a caller that wants its own
//! registration order or subset), and [`register_test_fonts`] is the usual
//! entry point: register every bundled face into a fresh or existing
//! [`frust_text::TextContext`] in one call.

use frust_text::{FontError, RegisteredFamily, TextContext};

/// Latin sans + combining marks (`Hello`, `é` U+00E9, combining circumflex
/// U+0302) — Noto Sans, SIL OFL 1.1.
const LATIN: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");
/// Arabic RTL + joining (`مرحبا`) — Noto Sans Arabic, SIL OFL 1.1.
const ARABIC: &[u8] = include_bytes!("../../../testing/fonts/NotoSansArabic-Subset.ttf");
/// CJK (`日本語`) — Noto Sans JP, SIL OFL 1.1.
const CJK: &[u8] = include_bytes!("../../../testing/fonts/NotoSansJP-Subset.otf");
/// COLRv1 colour emoji (`😀`, U+1F600) — Apache License 2.0.
const EMOJI: &[u8] = include_bytes!("../../../testing/fonts/NotoEmoji-COLRv1-Subset.ttf");

/// Every bundled test font's raw bytes, paired with a short label
/// identifying it (for a panic/assertion message, not parsed by any caller).
///
/// Order matters to a caller that registers these into a
/// [`frust_text::FontFamily::stack`] fallback list: earlier entries win font
/// selection for a codepoint both cover, and this order (Latin, Arabic, CJK,
/// emoji) is a no-op for the disjoint scripts these four subsets actually
/// carry — see `testing/fonts/LICENSES.md` for the exact codepoint sets.
pub fn test_fonts() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("Noto Sans (Latin + combining marks)", LATIN),
        ("Noto Sans Arabic (RTL + joining)", ARABIC),
        ("Noto Sans JP (CJK)", CJK),
        ("Frust Test Emoji COLR (COLRv1 colour emoji)", EMOJI),
    ]
}

/// Registers every [`test_fonts`] face into `cx` via
/// [`TextContext::register_fonts`], in [`test_fonts`]'s order, returning the
/// resolved [`RegisteredFamily`] for each.
///
/// # Panics
///
/// Panics if any bundled face fails to register (a bundled test fixture
/// failing to parse is a bug in this crate's own font files, not a
/// caller-recoverable error) — this is a test-only helper, not part of a
/// production font-loading path.
pub fn register_test_fonts(cx: &mut TextContext) -> Vec<RegisteredFamily> {
    test_fonts()
        .into_iter()
        .flat_map(|(label, bytes)| {
            cx.register_fonts(bytes.to_vec())
                .unwrap_or_else(|err: FontError| {
                    panic!("bundled test font {label:?} failed to register: {err}")
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_text::{FontFamily, TextStyle};
    use kurbo::Point;
    use peniko::Color;

    /// The exact mixed-script string this module's fonts exist to shape:
    /// Latin, Arabic, CJK, a stacked combining-mark case, and a COLRv1 emoji.
    const SAMPLE: &str = "Hello, مرحبا, 日本語, é\u{0302} , 😀";

    /// A run's font resolved to bundled bytes identical to one of
    /// [`test_fonts`]'s four entries — the "no system font consulted" check:
    /// every run's `FontHandle` must round-trip to exactly one of these
    /// fixed byte slices, never some other (system) font's bytes.
    fn resolved_label(run_font_bytes: &[u8]) -> Option<&'static str> {
        test_fonts()
            .into_iter()
            .find(|(_, bytes)| *bytes == run_font_bytes)
            .map(|(label, _)| label)
    }

    #[test]
    fn register_test_fonts_reports_every_bundled_family() {
        let mut cx = TextContext::new();
        let registered = register_test_fonts(&mut cx);

        let names: Vec<&str> = registered.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Noto Sans",
                "Noto Sans Arabic",
                "Noto Sans JP",
                "Frust Test Emoji COLR"
            ],
            "register_test_fonts must report the resolved family name for \
             every bundled face, in test_fonts() order"
        );
        for family in &registered {
            assert!(
                family.face_count >= 1,
                "{} registered with zero faces",
                family.name
            );
        }
    }

    #[test]
    fn shapes_every_bundled_script_with_no_system_font() {
        let mut cx = TextContext::new();
        let registered = register_test_fonts(&mut cx);
        let family_stack = FontFamily::stack(registered.iter().map(|f| f.name.clone()));

        let style = TextStyle {
            family: family_stack,
            ..TextStyle::new(32.0, Color::BLACK)
        };
        let layout = cx.layout(SAMPLE, &style, None);
        let runs = layout.to_scene_runs(Point::ORIGIN);

        assert!(
            !runs.is_empty(),
            "expected at least one glyph run shaping {SAMPLE:?}"
        );

        let mut resolved_labels = std::collections::BTreeSet::new();
        for run in &runs {
            assert!(
                !run.glyphs.is_empty(),
                "every glyph run must carry at least one glyph"
            );
            let font_bytes = run.font.font().data.as_ref();
            let label = resolved_label(font_bytes).unwrap_or_else(|| {
                panic!(
                    "a glyph run resolved to font bytes matching none of the four \
                     bundled test fonts — a system font was consulted instead of \
                     one of {:?}",
                    test_fonts().into_iter().map(|(l, _)| l).collect::<Vec<_>>()
                )
            });
            resolved_labels.insert(label);
        }

        for expected in [
            "Noto Sans (Latin + combining marks)",
            "Noto Sans Arabic (RTL + joining)",
            "Noto Sans JP (CJK)",
            "Frust Test Emoji COLR (COLRv1 colour emoji)",
        ] {
            assert!(
                resolved_labels.contains(expected),
                "expected a glyph run resolved against {expected:?}; got runs \
                 resolved against {resolved_labels:?}"
            );
        }
    }
}
