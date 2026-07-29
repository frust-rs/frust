//! parley layout -> `frust_scene` glyph-run conversion.
//!
//! This module owns the parley -> vello-0.9 glyph-mapping contract
//! (`frust-render` consumes the resulting [`frust_scene::GlyphRun`]s on the vello side).
//!
//! # Coordinate convention
//!
//! Each emitted [`frust_scene::Glyph`] carries **run-local** coordinates that
//! are exactly what `vello::Glyph` expects:
//!
//! - `x` is the glyph's absolute horizontal position within its run, measured
//!   from the run origin — parley's per-glyph advance offset plus the run's
//!   `offset()` (the leading whitespace/indent before the first glyph). This is
//!   the value produced by [`parley`]'s `positioned_glyphs()`.
//! - `y` is the glyph's baseline-relative vertical position with the line
//!   baseline already applied (again via `positioned_glyphs()`), so glyphs on
//!   the same line share a `y` and successive lines get distinct, increasing
//!   `y` values.
//!
//! The block-level placement (`origin`, passed by the widget) is **not** baked
//! into the glyph coordinates; it lives on [`frust_scene::GlyphRun::transform`]
//! as a translation. The render backend applies that transform to the whole run,
//! matching vello's `draw_glyphs(...).transform(..)` model. Keeping origin in the
//! transform (rather than added into every glyph) lets the scene builder compose
//! ancestor transforms cheaply (see `frust_scene::SceneBuilder::draw_glyph_run`).

use kurbo::{Affine, Point};
use peniko::Brush;

use frust_scene::{FontHandle, Glyph, GlyphRun};

/// Walks every line's glyph runs, mapping each into a scene glyph run whose
/// transform translates it to `origin`.
///
/// Inline boxes (not used in v1) are skipped. An empty or run-less layout
/// yields an empty vector.
pub(crate) fn layout_to_scene_runs(layout: &parley::Layout<Brush>, origin: Point) -> Vec<GlyphRun> {
    let transform = Affine::translate((origin.x, origin.y));
    let mut runs = Vec::new();

    for line in layout.lines() {
        for item in line.items() {
            let parley::layout::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                // InlineBox — not emitted in v1.
                continue;
            };

            let run = glyph_run.run();
            let font = FontHandle::new(run.font().clone());
            let font_size = run.font_size();
            // Per-run brush resolved from the pushed `StyleProperty::Brush`.
            let brush = glyph_run.style().brush.clone();

            let glyphs: Vec<Glyph> = glyph_run
                .positioned_glyphs()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                })
                .collect();

            runs.push(GlyphRun {
                font,
                font_size,
                brush,
                transform,
                glyphs,
            });
        }
    }

    runs
}
