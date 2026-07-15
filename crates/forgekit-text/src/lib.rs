//! Parley 0.11 text pipeline for ForgeKit (spec §4, §9).
//!
//! Wraps parley's font matching and layout into a small, renderer-agnostic
//! surface: [`TextContext`] owns the heavyweight font/layout state, [`TextStyle`]
//! carries the v1 styling knobs, and [`TextLayout`] is a finished, measurable
//! block of text that converts into [`forgekit_scene::GlyphRun`]s. Only
//! `kurbo`/`peniko`/`forgekit-scene` types appear in the public API — no vello
//! or wgpu types leak through (spec §7). The parley -> vello glyph coordinate
//! contract lives in [`convert`].

mod context;
mod convert;
mod layout;
mod style;

pub use context::TextContext;
pub use layout::TextLayout;
pub use style::TextStyle;

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_scene::GlyphRun;
    use kurbo::Point;
    use peniko::Color;

    fn style(size: f32) -> TextStyle {
        TextStyle::new(size, Color::BLACK)
    }

    /// Assert that within each run the glyph x positions are non-decreasing.
    fn assert_monotonic_x(run: &GlyphRun) {
        let mut prev = f32::NEG_INFINITY;
        for glyph in &run.glyphs {
            assert!(
                glyph.x >= prev,
                "glyph x positions must be non-decreasing within a run: {} < {prev}",
                glyph.x
            );
            prev = glyph.x;
        }
    }

    #[test]
    fn single_line_has_glyphs_with_monotonic_x() {
        let mut cx = TextContext::new();
        let layout = cx.layout("Hello from ForgeKit", &style(32.0), None);

        let size = layout.size();
        assert!(
            size.width > 0.0 && size.height > 0.0,
            "expected non-zero size, got {size:?} — is a system font available? \
             (parley GenericFamily::SystemUi failed to resolve)"
        );

        let runs = layout.to_scene_runs(Point::ORIGIN);
        assert!(
            !runs.is_empty(),
            "expected at least one glyph run — no system font resolved?"
        );

        let total_glyphs: usize = runs.iter().map(|r| r.glyphs.len()).sum();
        assert!(
            total_glyphs > 0,
            "expected a positive glyph count for non-empty text"
        );

        for run in &runs {
            assert_eq!(run.font_size, 32.0);
            assert_monotonic_x(run);
        }
    }

    #[test]
    fn constrained_width_produces_multiple_lines_with_distinct_baselines() {
        let mut cx = TextContext::new();
        // A width narrow enough to force wrapping of this multi-word string.
        let layout = cx.layout(
            "Hello from ForgeKit, the pure Rust mobile UI toolkit",
            &style(24.0),
            Some(80.0),
        );

        let runs = layout.to_scene_runs(Point::ORIGIN);
        assert!(
            !runs.is_empty(),
            "expected glyph runs — no system font resolved?"
        );

        // Distinct baselines show up as distinct glyph y values across runs.
        let mut baselines: Vec<f32> = runs
            .iter()
            .filter_map(|r| r.glyphs.first().map(|g| g.y))
            .collect();
        baselines.sort_by(|a, b| a.partial_cmp(b).unwrap());
        baselines.dedup();
        assert!(
            baselines.len() > 1,
            "constrained width should yield >1 distinct baseline, got {baselines:?}"
        );
    }

    #[test]
    fn empty_string_yields_no_runs_and_zero_ish_size() {
        let mut cx = TextContext::new();
        let layout = cx.layout("", &style(20.0), None);

        let runs = layout.to_scene_runs(Point::ORIGIN);
        assert!(runs.is_empty(), "empty text must produce no glyph runs");

        let size = layout.size();
        assert_eq!(size.width, 0.0, "empty text should have zero width");
        // Height may be a single empty line's height; it must be finite and small.
        assert!(size.height.is_finite());
    }

    #[test]
    fn to_scene_runs_translates_positions_by_origin() {
        let mut cx = TextContext::new();
        let text = "Ag";
        let sty = style(28.0);

        let at_origin = cx.layout(text, &sty, None).to_scene_runs(Point::ORIGIN);
        let offset = Point::new(100.0, 50.0);
        let translated = cx.layout(text, &sty, None).to_scene_runs(offset);

        assert_eq!(at_origin.len(), translated.len());
        assert!(!at_origin.is_empty(), "expected glyph runs for \"Ag\"");

        for (base, moved) in at_origin.iter().zip(&translated) {
            // Glyph-local coordinates are identical; only the run transform
            // carries the origin translation.
            assert_eq!(base.glyphs, moved.glyphs);
            assert_eq!(base.transform, kurbo::Affine::translate((0.0, 0.0)));
            assert_eq!(
                moved.transform,
                kurbo::Affine::translate((offset.x, offset.y))
            );
            // Run metadata is carried through.
            assert_eq!(moved.font_size, 28.0);
            assert!(matches!(moved.brush, peniko::Brush::Solid(_)));
        }
    }

    #[test]
    fn relative_glyph_positions_are_preserved() {
        let mut cx = TextContext::new();
        let runs = cx
            .layout("WWW", &style(30.0), None)
            .to_scene_runs(Point::ORIGIN);

        let run = runs.first().expect("expected a glyph run for \"WWW\"");
        assert!(
            run.glyphs.len() >= 2,
            "expected multiple glyphs to compare relative positions"
        );
        // Repeated glyphs should advance by a strictly positive step.
        let x0 = run.glyphs[0].x;
        let x1 = run.glyphs[1].x;
        assert!(x1 > x0, "second glyph must advance past the first");
    }
}
