//! M3 progress indicators (Phase 6c, PLAN.md D5, task 10): linear (4dp) and
//! circular (40dp) variants, both determinate and indeterminate. The circular
//! variant needs the new `Command::Path`/`fill_path`/`stroke_path` primitive
//! (PLAN.md D2b, task 05). Wavy/Expressive variants are experimental upstream
//! and deferred (PLAN.md D5).
//!
//! [`LinearProgress`]/[`CircularProgress`] are both **controlled components**
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics): the app passes a
//! [`ProgressValue`] every frame — `Determinate(f)` for a known `0.0..=1.0`
//! fraction, or `Indeterminate` for an unknown-duration loop — and the widget
//! never has anything to report back (there is no callback; a progress
//! indicator has no user interaction). `rebuild` simply reconciles the
//! `element`'s stored value to whatever the view says, the same
//! set-if-different reconciliation `TextInput`'s `value` uses.
//!
//! **Track thickness/diameter** (research-cited, see
//! `research/RESEARCH.md`'s `m3-component-specs-idioms` section): linear
//! track thickness 4dp, indicator = `colors.primary`, track =
//! `colors.primary_container`; circular outer diameter 40dp, stroke 4dp,
//! indicator = `colors.primary`, track = transparent by default (so only the
//! indicator arc is painted, no ring behind it) — both cited to
//! `material-components-android`'s `ProgressIndicator.md`.
//!
//! **Indeterminate motion is a documented approximation, not the real M3
//! two-segment choreography.** Upstream's indeterminate linear/circular
//! motion runs two independently-eased segments with a multi-keyframe timing
//! spec (`LinearProgressIndicator`'s `firstLineHead`/`firstLineTail`/
//! `secondLineHead`/`secondLineTail` fractions, `CircularProgressIndicator`'s
//! rotating-plus-growing/shrinking arc) that is Compose-implementation
//! internal, not part of the publicly cited component spec this crate's
//! research ledger covers. This module ships a single-segment approximation
//! instead (see [`LINEAR_INDETERMINATE_SEGMENT_FRACTION`]/
//! [`CIRCULAR_INDETERMINATE_SWEEP`] below) — visually "a segment/arc loops
//! continuously", not upstream's exact choreography. Revisit if a future
//! phase needs pixel-accurate parity.

use std::f64::consts::{PI, TAU};
use std::time::Duration;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, LayoutCtx, PaintCtx,
    PaintScene, SemanticsCtx, View, Widget,
};
use forgekit_scene::arc_path;
use forgekit_theme::Theme;
use kurbo::{Point, Size};
use peniko::{Brush, Color};

/// A progress indicator's controlled value (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProgressValue {
    /// A known progress fraction. Clamped to `0.0..=1.0` wherever it is read
    /// (layout/paint/semantics), so an out-of-range app value never panics or
    /// paints outside the track.
    Determinate(f64),
    /// An unknown-duration, looping progress animation.
    Indeterminate,
}

impl ProgressValue {
    /// The clamped `0.0..=1.0` fraction, or `None` for [`ProgressValue::Indeterminate`].
    fn determinate_fraction(self) -> Option<f64> {
        match self {
            ProgressValue::Determinate(v) => Some(v.clamp(0.0, 1.0)),
            ProgressValue::Indeterminate => None,
        }
    }
}

// -- Linear ------------------------------------------------------------

/// Linear track thickness, in logical px (source: androidx
/// `ProgressIndicator.md`, `app:trackThickness` default — see the
/// [module docs](self)).
const LINEAR_TRACK_HEIGHT: f64 = 4.0;

/// Unthemed indicator fill fallback (mirrors [`super::switch`]/`button.rs`'s
/// unthemed-primary convention; a theme resolves this from `colors.primary`).
const LINEAR_FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Unthemed track fallback (a theme resolves this from
/// `colors.primary_container`).
const LINEAR_TRACK: Color = Color::from_rgb8(0xD6, 0xE4, 0xFF);

/// Default period of the indeterminate linear sweep loop.
///
/// **Community-approximate**: this is a single-segment stand-in for
/// upstream's real multi-keyframe indeterminate timing (see the
/// [module docs](self)'s Indeterminate motion note) — not a cited spec value.
const LINEAR_INDETERMINATE_PERIOD: Duration = Duration::from_millis(1800);

/// The indeterminate sweeping segment's width, as a fraction of the track's
/// full width.
///
/// **Community-approximate**: chosen to read clearly as "a segment sweeping
/// across the track" at typical progress-bar widths — not a cited spec value
/// (see the [module docs](self)'s Indeterminate motion note).
const LINEAR_INDETERMINATE_SEGMENT_FRACTION: f64 = 0.35;

/// A determinate/indeterminate linear progress indicator. See the
/// [module docs](self).
pub struct LinearProgressView {
    value: ProgressValue,
}

/// Create a linear progress indicator driven by `value`.
pub fn linear_progress(value: ProgressValue) -> LinearProgressView {
    LinearProgressView { value }
}

/// PascalCase alias for [`linear_progress`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn LinearProgress(value: ProgressValue) -> LinearProgressView {
    linear_progress(value)
}

impl<State: 'static> View<State> for LinearProgressView {
    type Element = LinearProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LinearProgressWidget {
        let mut widget = LinearProgressWidget {
            value: self.value,
            indeterminate: AnimationController::new(LINEAR_INDETERMINATE_PERIOD)
                .with_curve(Curve::Linear),
        };
        if matches!(widget.value, ProgressValue::Indeterminate) {
            widget.indeterminate.repeat();
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LinearProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.value != self.value {
            element.value = self.value;
            match self.value {
                ProgressValue::Indeterminate => {
                    if !element.indeterminate.is_animating() {
                        element.indeterminate.repeat();
                    }
                }
                ProgressValue::Determinate(_) => element.indeterminate.stop(),
            }
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

/// The retained widget for a [`LinearProgressView`]. See the [module docs](self).
pub struct LinearProgressWidget {
    value: ProgressValue,
    /// Drives the indeterminate sweep loop (`0.0..=1.0`, repeating). Idle
    /// (never advanced) while [`ProgressValue::Determinate`].
    indeterminate: AnimationController,
}

impl LinearProgressWidget {
    /// The `(indicator, track)` fill colors: themed `colors.primary`/
    /// `colors.primary_container`, or the unthemed [`LINEAR_FILL`]/
    /// [`LINEAR_TRACK`] constants.
    fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
        match theme {
            Some(theme) => (theme.scheme().primary, theme.scheme().primary_container),
            None => (LINEAR_FILL, LINEAR_TRACK),
        }
    }
}

impl Widget for LinearProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        bc.constrain(Size::new(width, LINEAR_TRACK_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (indicator, track) = Self::resolve_colors(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let radius = size.height / 2.0;

        scene.fill_rounded_rect(origin, size, radius, track);

        match self.value {
            ProgressValue::Determinate(_) => {
                let frac = self.value.determinate_fraction().unwrap_or(0.0);
                let fill_width = size.width * frac;
                if fill_width > 0.0 {
                    scene.fill_rounded_rect(
                        origin,
                        Size::new(fill_width, size.height),
                        radius,
                        indicator,
                    );
                }
            }
            ProgressValue::Indeterminate => {
                self.indeterminate.advance(ctx.frame_time());
                let t = self.indeterminate.value_clamped();
                let segment_w = size.width * LINEAR_INDETERMINATE_SEGMENT_FRACTION;
                // Sweep the segment's leading edge from off the left edge to
                // off the right edge, so it visibly enters and exits the
                // track (see the module docs' Indeterminate motion note).
                let travel = size.width + segment_w;
                let x = -segment_w + t * travel;
                let clipped_x = x.max(0.0);
                let clipped_w = (x + segment_w).min(size.width) - clipped_x;
                if clipped_w > 0.0 {
                    scene.fill_rounded_rect(
                        Point::new(origin.x + clipped_x, origin.y),
                        Size::new(clipped_w, size.height),
                        radius,
                        indicator,
                    );
                }
                ctx.request_frame();
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ProgressIndicator, |node| {
            // A determinate indicator reports its numeric value/range; an
            // indeterminate one leaves numeric_value unset entirely — the
            // ARIA/accesskit convention for "progress with no known
            // completion fraction" (no min/max/value = indeterminate).
            if let Some(frac) = self.value.determinate_fraction() {
                node.set_numeric_value(frac);
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(1.0);
            }
        });
    }
}

// -- Circular ------------------------------------------------------------

/// Circular indicator's outer diameter, in logical px (source: androidx
/// `ProgressIndicator.md`'s default `app:indicatorSize` — see the
/// [module docs](self)).
const CIRCULAR_DIAMETER: f64 = 40.0;
/// Circular indicator's stroke width, in logical px (same source as
/// [`CIRCULAR_DIAMETER`]).
const CIRCULAR_STROKE: f64 = 4.0;

/// The arc's starting angle: straight up (12 o'clock), matching
/// `kurbo::Arc`'s convention (0 = positive x-axis, positive = clockwise in a
/// y-down space — see `forgekit-scene::arc_path`'s docs) rotated a quarter
/// turn counter-clockwise from the positive x-axis.
const START_ANGLE: f64 = -PI / 2.0;

/// Default period of one full indeterminate rotation.
///
/// **Community-approximate**: a single-segment stand-in for upstream's real
/// growing/shrinking-arc indeterminate timing (see the [module docs](self)'s
/// Indeterminate motion note) — not a cited spec value.
const CIRCULAR_INDETERMINATE_PERIOD: Duration = Duration::from_millis(1500);

/// The indeterminate spinner's fixed sweep angle (a full turn minus a quarter
/// turn — i.e. a 270° arc), in radians.
///
/// **Community-approximate**: chosen to read clearly as a rotating spinner
/// arc, matching common Material-spinner reimplementations — not a cited spec
/// value (see the [module docs](self)'s Indeterminate motion note).
const CIRCULAR_INDETERMINATE_SWEEP: f64 = 0.75 * TAU;

/// A determinate/indeterminate circular progress indicator. See the
/// [module docs](self).
pub struct CircularProgressView {
    value: ProgressValue,
}

/// Create a circular progress indicator driven by `value`.
pub fn circular_progress(value: ProgressValue) -> CircularProgressView {
    CircularProgressView { value }
}

/// PascalCase alias for [`circular_progress`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn CircularProgress(value: ProgressValue) -> CircularProgressView {
    circular_progress(value)
}

impl<State: 'static> View<State> for CircularProgressView {
    type Element = CircularProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CircularProgressWidget {
        let mut widget = CircularProgressWidget {
            value: self.value,
            indeterminate: AnimationController::new(CIRCULAR_INDETERMINATE_PERIOD)
                .with_curve(Curve::Linear),
        };
        if matches!(widget.value, ProgressValue::Indeterminate) {
            widget.indeterminate.repeat();
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CircularProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.value != self.value {
            element.value = self.value;
            match self.value {
                ProgressValue::Indeterminate => {
                    if !element.indeterminate.is_animating() {
                        element.indeterminate.repeat();
                    }
                }
                ProgressValue::Determinate(_) => element.indeterminate.stop(),
            }
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

/// The retained widget for a [`CircularProgressView`]. See the [module docs](self).
pub struct CircularProgressWidget {
    value: ProgressValue,
    /// Drives the indeterminate rotation loop (`0.0..=1.0`, repeating). Idle
    /// (never advanced) while [`ProgressValue::Determinate`].
    indeterminate: AnimationController,
}

impl CircularProgressWidget {
    /// The indicator stroke color: themed `colors.primary`, or the unthemed
    /// [`LINEAR_FILL`] constant (circular shares linear's unthemed indicator
    /// color — the track itself is transparent by default per the
    /// [module docs](self), so there is no circular track color to resolve).
    fn resolve_color(theme: Option<&Theme>) -> Color {
        match theme {
            Some(theme) => theme.scheme().primary,
            None => LINEAR_FILL,
        }
    }
}

impl Widget for CircularProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let color = Self::resolve_color(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let radius = ((size.width.min(size.height)) - CIRCULAR_STROKE) / 2.0;
        let center = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let brush = Brush::Solid(color);

        match self.value {
            ProgressValue::Determinate(_) => {
                let frac = self.value.determinate_fraction().unwrap_or(0.0);
                if frac > 0.0 {
                    let sweep = TAU * frac;
                    let path = arc_path(center, radius, START_ANGLE, sweep);
                    scene.stroke_path(Point::ZERO, &path, CIRCULAR_STROKE, &brush);
                }
            }
            ProgressValue::Indeterminate => {
                self.indeterminate.advance(ctx.frame_time());
                let t = self.indeterminate.value_clamped();
                let start = START_ANGLE + t * TAU;
                let path = arc_path(center, radius, start, CIRCULAR_INDETERMINATE_SWEEP);
                scene.stroke_path(Point::ZERO, &path, CIRCULAR_STROKE, &brush);
                ctx.request_frame();
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ProgressIndicator, |node| {
            if let Some(frac) = self.value.determinate_fraction() {
                node.set_numeric_value(frac);
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(1.0);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::BuildCtx;
    use kurbo::PathEl;

    fn build_linear(value: ProgressValue) -> LinearProgressWidget {
        let view = linear_progress(value);
        let mut counter = 0u64;
        <LinearProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn build_circular(value: ProgressValue) -> CircularProgressWidget {
        let view = circular_progress(value);
        let mut counter = 0u64;
        <CircularProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    // -- shared recording scene -----------------------------------------

    #[derive(Default)]
    struct RecordingScene {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(kurbo::BezPath, f64)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(
            &mut self,
            _origin: Point,
            path: &kurbo::BezPath,
            width: f64,
            _brush: &Brush,
        ) {
            self.strokes.push((path.clone(), width));
        }
    }

    fn paint_at(w: &mut dyn Widget, origin: Point, size: Size) -> RecordingScene {
        let mut ctx = PaintCtx::new(origin, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    // -- linear: determinate geometry ------------------------------------

    #[test]
    fn linear_determinate_fills_proportionally_to_value() {
        let mut w = build_linear(ProgressValue::Determinate(0.25));
        let scene = paint_at(&mut w, Point::ZERO, Size::new(200.0, LINEAR_TRACK_HEIGHT));
        // rrects[0] is the track (full width), rrects[1] is the fill.
        assert_eq!(scene.rrects.len(), 2);
        let (_, track_size, _, _) = scene.rrects[0];
        assert_eq!(track_size.width, 200.0);
        let (_, fill_size, _, _) = scene.rrects[1];
        assert_eq!(fill_size.width, 50.0, "25% of 200 = 50");
    }

    #[test]
    fn linear_determinate_clamps_out_of_range_values() {
        let mut over = build_linear(ProgressValue::Determinate(1.5));
        let scene = paint_at(
            &mut over,
            Point::ZERO,
            Size::new(100.0, LINEAR_TRACK_HEIGHT),
        );
        let (_, fill_size, _, _) = scene.rrects[1];
        assert_eq!(fill_size.width, 100.0, "clamped to 1.0");

        let mut under = build_linear(ProgressValue::Determinate(-0.5));
        let scene = paint_at(
            &mut under,
            Point::ZERO,
            Size::new(100.0, LINEAR_TRACK_HEIGHT),
        );
        // No fill rect painted at all for a clamped-to-zero fraction.
        assert_eq!(scene.rrects.len(), 1, "only the track, no zero-width fill");
    }

    #[test]
    fn linear_determinate_paints_zero_progress_with_no_fill() {
        let mut w = build_linear(ProgressValue::Determinate(0.0));
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
        assert_eq!(scene.rrects.len(), 1);
    }

    // -- linear: indeterminate requests frames continuously ---------------

    #[test]
    fn linear_indeterminate_requests_a_frame_every_paint() {
        // `PaintCtx::new` in a bare-core test defaults to `FrameTime::ZERO`
        // (no shell clock threaded in); `AnimationController::repeat`'s drive
        // still reports "still animating" on every `advance` regardless of
        // elapsed time, so `request_frame` fires on every paint call — the
        // "requests frames continuously" contract this test asserts.
        let mut w = build_linear(ProgressValue::Indeterminate);
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(
                ctx.needs_frame(),
                "indeterminate must keep requesting frames"
            );
        }
    }

    // -- controlled contract: never self-mutates --------------------------

    #[test]
    fn rebuild_reconciles_the_view_supplied_value() {
        let view = linear_progress(ProgressValue::Determinate(0.1));
        let mut w = build_linear(ProgressValue::Determinate(0.1));
        let next = linear_progress(ProgressValue::Determinate(0.9));
        let mut counter = 0u64;
        let flags = <LinearProgressView as View<()>>::rebuild(
            &next,
            &view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(flags.contains(ChangeFlags::PAINT));
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
        let (_, fill_size, _, _) = scene.rrects[1];
        assert_eq!(
            fill_size.width, 90.0,
            "widget reflects the new view value, not a self-mutated one"
        );
    }

    // -- circular: determinate geometry -----------------------------------

    #[test]
    fn circular_determinate_sweep_is_proportional_to_value() {
        let mut half = build_circular(ProgressValue::Determinate(0.5));
        let scene = paint_at(
            &mut half,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert_eq!(scene.strokes.len(), 1);
        let (path, width) = &scene.strokes[0];
        assert_eq!(*width, CIRCULAR_STROKE);
        // A half sweep (PI radians) produces more curve segments than a
        // quarter sweep at the same tolerance.
        let mut quarter = build_circular(ProgressValue::Determinate(0.25));
        let quarter_scene = paint_at(
            &mut quarter,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        let (quarter_path, _) = &quarter_scene.strokes[0];
        let curve_count = |p: &kurbo::BezPath| {
            p.elements()
                .iter()
                .filter(|el| matches!(el, PathEl::CurveTo(..) | PathEl::QuadTo(..)))
                .count()
        };
        assert!(curve_count(path) >= curve_count(quarter_path));
        assert!(!path.elements().is_empty());
    }

    #[test]
    fn circular_zero_progress_paints_no_stroke() {
        let mut w = build_circular(ProgressValue::Determinate(0.0));
        let scene = paint_at(
            &mut w,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert!(scene.strokes.is_empty());
    }

    // -- circular: indeterminate requests frames continuously -------------

    #[test]
    fn circular_indeterminate_requests_a_frame_every_paint() {
        let mut w = build_circular(ProgressValue::Indeterminate);
        for _ in 0..3 {
            let mut ctx =
                PaintCtx::new(Point::ZERO, Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(ctx.needs_frame());
            assert_eq!(
                scene.strokes.len(),
                1,
                "always paints exactly one arc segment"
            );
        }
    }

    // -- semantics -----------------------------------------------------------
    //
    // `SemanticsCtx::new`/`finish` are `pub(crate)` to `forgekit-core` (only
    // `RenderRoot::semantics` — a public API — can produce a real
    // `SemanticsUpdate`), so these drive a single-widget tree through a real
    // `RenderRoot` rebuild + layout + semantics pass, mirroring
    // `tests/semantics_tree.rs`'s integration-test pattern at unit scale.

    fn semantics_root_node<V>(logic: impl FnMut(&mut ()) -> V) -> forgekit_core::accesskit::Node
    where
        V: View<()> + 'static,
    {
        let mut root: forgekit_core::RenderRoot<(), V> = forgekit_core::RenderRoot::new();
        let mut state = ();
        let mut logic = logic;
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        // The progress indicator is a single leaf node under the window root.
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id != update.root)
            .expect("the progress indicator contributes exactly one node");
        node.clone()
    }

    #[test]
    fn linear_determinate_semantics_reports_numeric_value_and_range() {
        let node = semantics_root_node(|_| linear_progress(ProgressValue::Determinate(0.42)));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(node.numeric_value(), Some(0.42));
        assert_eq!(node.min_numeric_value(), Some(0.0));
        assert_eq!(node.max_numeric_value(), Some(1.0));
    }

    #[test]
    fn indeterminate_semantics_omits_numeric_value() {
        let node = semantics_root_node(|_| linear_progress(ProgressValue::Indeterminate));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(
            node.numeric_value(),
            None,
            "indeterminate carries no numeric value"
        );
    }

    #[test]
    fn circular_semantics_role_and_value() {
        let node = semantics_root_node(|_| circular_progress(ProgressValue::Determinate(0.75)));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(node.numeric_value(), Some(0.75));
    }
}
