//! The TEXT corpus: `Command::GlyphRun` goldens across script, size, color
//! and one transform-adjacent command (a rounded clip cutting a run off),
//! shaped against the BUNDLED test fonts only
//! (`testing/fonts/`, `crate::fonts`) — no host font ever participates
//! (`crate::frame::foreign_font_runs` is the check that proves it).
//!
//! # Why every case registers all four bundled faces
//!
//! [`bundled_context`] registers every face in [`crate::fonts::test_fonts`]
//! and builds one [`FontFamily::stack`] naming all four, the same
//! "register everything, let parley's own fallback pick the right face per
//! script" shape [`crate::fonts`]'s own acceptance test and
//! [`crate::corpus::unit::unit_cases`]'s `unit-glyph-run` case use. A case
//! that only needs Latin could name a narrower stack, but a shared helper
//! that always resolves the same way is one less place a case-specific typo
//! could accidentally reach past the bundled faces.
//!
//! # Codepoint budget
//!
//! Every bundled subset carries only the codepoints
//! `testing/fonts/LICENSES.md` lists — `"Hello, مرحبا, 日本語, é\u{0302} , 😀"`
//! and nothing else. A case here is written only from those codepoints; see
//! `docs/TESTING.md`'s Deterministic Inputs section for why a codepoint
//! outside a bundled subset sends parley to a host font instead.
//!
//! # Backdrop
//!
//! Every case fills an opaque backdrop before drawing its glyph run(s) — the
//! same reasoning [`crate::corpus::unit::unit_cases`]'s `unit-glyph-run` case
//! states: black-on-transparent text would store its whole antialiased ramp
//! in the alpha channel alone, where a colour regression could hide.

use frust_scene::Scene;
use frust_scene::SceneBuilder;
use frust_text::{FontFamily, TextContext, TextStyle};
use kurbo::{Point, Rect};
use peniko::color::palette::css::{BLACK, BLUE, RED, WHITE};
use peniko::color::{ColorSpaceTag, DynamicColor, HueDirection};
use peniko::{Brush, Color, ColorStop, ColorStops, Gradient, GradientKind, LinearGradientPosition};

use super::{CorpusCase, Expect, Probe};
use crate::case::CaseSpec;
use crate::fonts::register_test_fonts;

/// Every case in the text corpus, in the order a reviewer reads them: script
/// coverage (Latin at several sizes, RTL Arabic, CJK, stacked combining
/// marks, COLR emoji), then paint/geometry interactions with a run (a
/// gradient brush, a rounded clip), then the perf-shaped stress case.
#[must_use]
pub fn text_cases() -> Vec<CorpusCase> {
    vec![
        latin_mixed_sizes(),
        rtl_arabic(),
        cjk(),
        combining_marks(),
        colr_emoji(),
        gradient_brush(),
        clipped(),
        ten_thousand_shaped_glyphs(),
    ]
}

/// A fresh [`TextContext`] with every bundled test face registered
/// (`crate::fonts::register_test_fonts`), plus the [`FontFamily`] stack
/// naming all four in [`crate::fonts::test_fonts`]'s order — see the module
/// docs for why every case shares this rather than naming a narrower stack.
fn bundled_context() -> (TextContext, FontFamily) {
    let mut cx = TextContext::new();
    let registered = register_test_fonts(&mut cx);
    let family = FontFamily::stack(registered.iter().map(|f| f.name.clone()));
    (cx, family)
}

/// An opaque backdrop filling the whole `width`x`height` frame — see the
/// module docs for why every case needs one.
fn backdrop(builder: &mut SceneBuilder<'_>, width: u32, height: u32, color: Color) {
    builder.fill_rect(
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        Brush::Solid(color),
    );
}

/// `Command::GlyphRun` at five sizes an app actually ships — caption through
/// display (8, 12, 16, 24, 48px) — stacked in one frame so a regression in
/// one size range is visible next to its neighbours rather than only in
/// isolation.
fn latin_mixed_sizes() -> CorpusCase {
    /// `(font size, top y)` pairs, top to bottom: [`frust_text::TextLayout::
    /// to_scene_runs`]'s `origin` is the layout's own TOP-LEFT (parley's
    /// coordinate convention, y increasing downward from there), not a
    /// baseline, so each row's `y` is measured headroom above that size's own
    /// ink plus a fixed gap before the next row.
    const SIZES: [(f32, f64); 5] = [
        (8.0, 8.0),
        (12.0, 24.0),
        (16.0, 46.0),
        (24.0, 73.0),
        (48.0, 110.0),
    ];
    const WIDTH: u32 = 200;
    const HEIGHT: u32 = 190;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        for (size, top_y) in SIZES {
            let style = TextStyle {
                family: family.clone(),
                ..TextStyle::new(size, BLACK)
            };
            let layout = cx.layout("Hello", &style, None);
            for run in layout.to_scene_runs(Point::new(8.0, top_y)) {
                builder.draw_glyph_run(run);
            }
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-latin-mixed-sizes").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: WIDTH - 5,
            y: HEIGHT - 5,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the frame's bottom-right corner sits outside every line and stays the plain \
                  white backdrop",
        }],
        eroded_interior: false,
        about: "one Latin word shaped at five sizes (8-48px), stacked in one frame",
    }
}

/// `Command::GlyphRun` shaping an Arabic word — RTL, with joining forms
/// between its letters — against the bundled Arabic face
/// (`testing/fonts/NotoSansArabic-Subset.ttf`). Bidi/direction resolution is
/// parley's own; this case draws nothing that overrides it, so a regression
/// in the shaper's bidi handling shows here rather than being papered over by
/// an explicit direction the widget layer would never set either.
fn rtl_arabic() -> CorpusCase {
    const WIDTH: u32 = 200;
    const HEIGHT: u32 = 100;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        let style = TextStyle {
            family,
            ..TextStyle::new(32.0, BLACK)
        };
        // "مرحبا" ("Hello") — every codepoint is in the bundled Arabic
        // subset (`testing/fonts/LICENSES.md`), including the joining forms
        // its letters take next to one another.
        let layout = cx.layout("مرحبا", &style, None);
        for run in layout.to_scene_runs(Point::new(16.0, 30.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-rtl-arabic").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: 4,
            y: 4,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the frame's top-left corner sits outside the shaped word and stays the plain \
                  white backdrop",
        }],
        eroded_interior: false,
        about: "an Arabic word (RTL, joining forms) shaped against the bundled test font",
    }
}

/// `Command::GlyphRun` shaping a CJK word against the bundled Japanese face
/// (`testing/fonts/NotoSansJP-Subset.otf`).
fn cjk() -> CorpusCase {
    const WIDTH: u32 = 200;
    const HEIGHT: u32 = 100;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        let style = TextStyle {
            family,
            ..TextStyle::new(32.0, BLACK)
        };
        // "日本語" ("Japanese language") — every codepoint is in the bundled
        // CJK subset.
        let layout = cx.layout("日本語", &style, None);
        for run in layout.to_scene_runs(Point::new(12.0, 30.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-cjk").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: 4,
            y: 4,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the frame's top-left corner sits outside the shaped word and stays the plain \
                  white backdrop",
        }],
        eroded_interior: false,
        about: "a CJK word shaped against the bundled Japanese test font",
    }
}

/// `Command::GlyphRun` shaping a stacked-diacritic grapheme: `é` (U+00E9,
/// itself pre-composed with an acute accent) plus a combining circumflex
/// (U+0302) layered on top of it — the exact `é\u{0302}` snippet
/// [`crate::fonts`]'s own acceptance test shapes, at a size large enough that
/// the stacking is legible in a diff rather than a handful of ambiguous
/// pixels.
fn combining_marks() -> CorpusCase {
    const WIDTH: u32 = 100;
    const HEIGHT: u32 = 100;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        let style = TextStyle {
            family,
            ..TextStyle::new(48.0, BLACK)
        };
        let layout = cx.layout("é\u{0302}", &style, None);
        for run in layout.to_scene_runs(Point::new(20.0, 20.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-combining").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: 4,
            y: 4,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the frame's top-left corner sits outside the glyph and stays the plain white \
                  backdrop",
        }],
        eroded_interior: false,
        about: "a pre-composed accented letter with a second combining mark stacked on top",
    }
}

/// `Command::GlyphRun` shaping a COLRv1 colour emoji against the bundled
/// emoji face (`testing/fonts/NotoEmoji-COLRv1-Subset.ttf`). PNG-strike/BGRA/
/// Mask bitmap emoji are a named gap (`docs/LIMITATIONS.md`); this case pins
/// the vector COLR path only, the one the p5 phase actually supports.
fn colr_emoji() -> CorpusCase {
    const WIDTH: u32 = 100;
    const HEIGHT: u32 = 100;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        let style = TextStyle {
            family,
            ..TextStyle::new(48.0, BLACK)
        };
        // U+1F600, the one codepoint the bundled emoji subset carries. This
        // face's glyph sits almost entirely BELOW its own baseline (measured
        // ~2..51px down from it, unlike a Latin glyph's mostly-above
        // placement), so the baseline is placed near the frame's top rather
        // than at its usual two-thirds mark.
        let layout = cx.layout("\u{1F600}", &style, None);
        for run in layout.to_scene_runs(Point::new(20.0, 14.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-colr-emoji").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: 4,
            y: 4,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the frame's top-left corner sits outside the glyph and stays the plain white \
                  backdrop",
        }],
        eroded_interior: false,
        about: "a COLRv1 colour emoji glyph shaped against the bundled test font",
    }
}

/// `Command::GlyphRun` with a GRADIENT brush rather than a solid one — the
/// run's own colour ramp, not the backdrop's.
///
/// [`frust_text::TextLayout::to_scene_runs`]'s docs are explicit that a
/// caller-supplied brush must be expressed in the run's LOCAL (pre-origin)
/// space, because the run's `transform` composes with its paint the same way
/// it composes with the glyph outlines — so the gradient's stops below span
/// `0..layout width`, not the frame's own coordinates, exactly the shape
/// `crates/frust-engine/tests/text_basic.rs`'s own `gradient_brush` helper
/// uses.
fn gradient_brush() -> CorpusCase {
    const WIDTH: u32 = 220;
    const HEIGHT: u32 = 100;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        let style = TextStyle {
            family,
            ..TextStyle::new(40.0, BLACK)
        };
        let layout = cx.layout("Hello", &style, None);
        let width = layout.size().width;
        let brush = Brush::Gradient(Gradient {
            kind: GradientKind::Linear(LinearGradientPosition {
                start: Point::new(0.0, 0.0),
                end: Point::new(width, 0.0),
            }),
            stops: ColorStops(
                vec![
                    ColorStop {
                        offset: 0.0,
                        color: DynamicColor::from_alpha_color(RED),
                    },
                    ColorStop {
                        offset: 1.0,
                        color: DynamicColor::from_alpha_color(BLUE),
                    },
                ]
                .into(),
            ),
            interpolation_cs: ColorSpaceTag::Srgb,
            hue_direction: HueDirection::Shorter,
            ..Default::default()
        });
        for mut run in layout.to_scene_runs(Point::new(12.0, 25.0)) {
            run.brush = brush.clone();
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-gradient-brush").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: 4,
            y: 4,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the frame's top-left corner sits outside the run and stays the plain white \
                  backdrop",
        }],
        eroded_interior: false,
        about: "a glyph run painted with a red-to-blue linear gradient rather than a solid brush",
    }
}

/// `Command::GlyphRun` drawn under a `Command::PushClipRounded` bracket
/// narrower than the run itself, so the run's tail is cut off by the clip
/// rather than reaching the frame edge — the same "must fail if the clip is
/// dropped" shape [`crate::corpus::unit::unit_cases`]'s `unit-clip-rect` case
/// uses, applied to text.
fn clipped() -> CorpusCase {
    const WIDTH: u32 = 220;
    const HEIGHT: u32 = 100;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, WHITE);
        builder.push_clip_rounded(Rect::new(8.0, 8.0, 148.0, 92.0), 16.0);
        let style = TextStyle {
            family,
            ..TextStyle::new(40.0, BLACK)
        };
        // "Hello, Hello" is wide enough at 40px that its second word runs
        // past the clip's right edge.
        let layout = cx.layout("Hello, Hello", &style, None);
        for run in layout.to_scene_runs(Point::new(12.0, 20.0)) {
            builder.draw_glyph_run(run);
        }
        builder.pop_clip();
    }
    CorpusCase {
        spec: CaseSpec::new("text-clipped").with_size(WIDTH, HEIGHT),
        record,
        probes: &[Probe {
            x: WIDTH - 10,
            y: 50,
            expect: Expect::Exact([255, 255, 255, 255]),
            why: "the run continues past the clip's right edge, which must cut it back to the \
                  plain white backdrop",
        }],
        eroded_interior: false,
        about: "a glyph run continuing past a rounded clip's edge, which must cut it off",
    }
}

/// `text-10k`: a long, WRAPPED block of text SHAPED through
/// [`TextContext::layout`] — roughly ten thousand glyphs spread across many
/// ordinary-sized lines/runs, the shape a genuinely long scrolling document
/// produces. This is deliberately not
/// [`crate::corpus::adversarial::adversarial_cases`]'s `adv-10k-glyphs`
/// again: that case hand-builds ONE pathological `GlyphRun` to stress the
/// glyph-BATCH path at a single draw call's scale; this one stresses the
/// LAYOUT/wrap path (parley's line-breaker, many runs, many draw calls) at
/// the same overall glyph count.
///
/// `no_ref`: only the frame's own top-left corner is asserted — the laid-out
/// block is many times taller than the deliberately small viewport, so most
/// of it renders off the bottom and there is nothing stable on any backend
/// for a stored baseline to pin (the same reasoning
/// `crate::corpus::adversarial`'s `adv-5k-layers` states for its own
/// depth-only stress case).
fn ten_thousand_shaped_glyphs() -> CorpusCase {
    const WIDTH: u32 = 300;
    const HEIGHT: u32 = 120;
    /// `"Hello, "` (7 codepoints, all in the bundled Latin subset) repeated
    /// this many times shapes to roughly ten thousand glyphs once wrapped.
    const REPEATS: usize = 1_500;

    fn record(scene: &mut Scene) {
        let (mut cx, family) = bundled_context();
        let mut builder = SceneBuilder::new(scene);
        backdrop(&mut builder, WIDTH, HEIGHT, BLACK);
        let style = TextStyle {
            family,
            ..TextStyle::new(14.0, WHITE)
        };
        let text = "Hello, ".repeat(REPEATS);
        let layout = cx.layout(&text, &style, Some(WIDTH as f32));
        for run in layout.to_scene_runs(Point::new(4.0, 16.0)) {
            builder.draw_glyph_run(run);
        }
    }
    CorpusCase {
        spec: CaseSpec::new("text-10k")
            .with_size(WIDTH, HEIGHT)
            .with_no_ref(true),
        record,
        probes: &[Probe {
            x: 6,
            y: 24,
            // The backdrop is already opaque black (alpha 255 everywhere),
            // so an alpha probe would pass whether or not any glyph painted
            // — this checks the RED channel instead, which only the white
            // text brush can raise.
            expect: Expect::AtLeast {
                channel: 0,
                min: 64,
            },
            why: "the very first glyph of a ten-thousand-glyph wrapped block must still paint, \
                  not stall or panic before reaching it",
        }],
        eroded_interior: true,
        about: "a wrapped ~10,000-glyph block shaped through TextContext (not hand-built), \
                stressing the layout/wrap and glyph-batch paths at a scale ordinary text never \
                reaches",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::frame::{foreign_font_runs, glyph_run_count, scene_fingerprint};

    #[test]
    fn every_case_name_is_unique_and_text_prefixed() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for case in text_cases() {
            assert!(
                case.spec.name.starts_with("text-"),
                "`{}` must carry the text- golden-file prefix",
                case.spec.name
            );
            assert!(
                seen.insert(case.spec.name),
                "duplicate case name `{}`",
                case.spec.name
            );
        }
    }

    #[test]
    fn every_case_records_commands_and_shapes_text() {
        for case in text_cases() {
            let scene = case.scene();
            assert!(
                !scene.commands().is_empty(),
                "`{}` recorded nothing",
                case.spec.name
            );
            assert!(
                glyph_run_count(&scene) > 0,
                "`{}` is a text case and shaped no text",
                case.spec.name
            );
        }
    }

    #[test]
    fn no_case_shapes_against_a_host_font() {
        let mut failures = Vec::new();
        for case in text_cases() {
            for message in foreign_font_runs(&case.scene()) {
                failures.push(format!("[{}] {message}", case.spec.name));
            }
        }
        assert!(
            failures.is_empty(),
            "{} non-portable frame(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn a_case_is_a_pure_function_of_its_inputs() {
        // Two independent captures of the same case must record identical
        // command lists — parley's shape cache is per-`TextContext`, and
        // every case builds a fresh one, so this also proves a case does not
        // depend on shaping state left over from a previous case.
        for case in text_cases() {
            let first = scene_fingerprint(&case.scene());
            let second = scene_fingerprint(&case.scene());
            assert_eq!(first, second, "`{}` is not deterministic", case.spec.name);
        }
    }

    /// The whole text corpus against its committed `cpu/` baselines.
    ///
    /// ```text
    /// cargo test -p frust-testing --lib corpus::text::tests::the_text_corpus_matches_its_goldens
    /// UPDATE_GOLDENS=1 cargo test -p frust-testing --lib \
    ///   corpus::text::tests::the_text_corpus_matches_its_goldens
    /// ```
    ///
    /// This is the same [`crate::frame::run_cpu_goldens`] gate
    /// `crates/frust-testing/tests/page_goldens.rs` runs for the widget/page
    /// corpus — see that file's docs for the check order. It lives here
    /// rather than in a sibling `tests/` binary because `run_cpu_goldens`
    /// itself already carries that shared pipeline; a corpus module's own
    /// `#[cfg(test)]` is a normal, GPU-free part of `cargo test -p
    /// frust-testing`'s default (non-`--ignored`) run either way.
    #[test]
    fn the_text_corpus_matches_its_goldens() {
        let report = crate::frame::run_cpu_goldens(&text_cases());
        println!(
            "corpus::text: {} compared, {} promoted",
            report.compared, report.promoted
        );
        report.assert_passed("the text corpus");
    }
}
