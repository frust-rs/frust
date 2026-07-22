//! Glyph shimmer progress bar (task 21, glyph-design-system): a determinate
//! fill in the Glyph accent (bright amber) over the Glyph "progress track"
//! role, with a continuously sweeping shimmer highlight over the filled
//! region.
//!
//! # Tokens
//!
//! Fill = `colors.primary_container` (Glyph's bright-fill amber role, uniform
//! across both brightnesses — see `frust_theme::glyph::color`'s module docs'
//! accent-role convention); track = `colors.surface_container_highest`, the
//! Glyph "overlay" surface role (RESEARCH §1.1's `bg-overlay` row: "overlays,
//! toggles, **progress track**" — a source-cited role assignment, not a
//! guess). Unthemed fallbacks below match the Glyph **dark** hex values
//! exactly, per this catalog's charter (`glyph::mod`'s module docs).
//!
//! # Shimmer
//!
//! The shimmer is a repeating linear-gradient sweep (`AnimationController::repeat`,
//! [`SHIMMER_PERIOD`] = 1.6s, linear) painted as the fill's own brush: a
//! 3-stop gradient (fill → a lighter highlight tint → fill) whose `(start,
//! end)` points translate across the filled width every frame, so the
//! highlight band enters from the left, sweeps across, and exits right,
//! looping continuously. [`SHIMMER_PERIOD`]/[`SHIMMER_HIGHLIGHT_MIX`]/
//! [`SHIMMER_BAND_FRACTION`] are this task's own authored values — the Glyph
//! HTML references don't dimension a progress-bar shimmer, so there is no
//! source to cite (unlike the durations/easings on `MotionScheme::glyph`,
//! which *are* exact source values).
//!
//! Not a controlled component in the `ProgressValue::Indeterminate` sense
//! `material::progress` uses: this is always a known `0.0..=1.0` fraction,
//! reconciled set-if-different on `rebuild` like every other controlled
//! widget (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! `reduce_motion` disables the shimmer sweep entirely (paints the flat fill,
//! no `request_frame`) — the Details' "shimmer/dots become static" rule.

use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, LayoutCtx, PaintCtx,
    PaintScene, SemanticsCtx, Tween, View, Widget,
};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::{Brush, Color, Gradient};

/// Progress-bar track thickness, in logical px.
///
/// This task's own choice — the Glyph HTML references don't dimension a
/// progress bar — picked to read as a slim, minimalist bar consistent with
/// Glyph's borders-first aesthetic (RESEARCH §1.3).
const TRACK_HEIGHT: f64 = 6.0;

/// The shimmer sweep's full period (`AnimationController::repeat`, linear).
///
/// This task's own choice (no Glyph source value — see the [module docs](self)).
const SHIMMER_PERIOD: Duration = Duration::from_millis(1600);

/// How far toward white the shimmer's highlight stop leans, `0.0..=1.0`
/// (`0.0` = no visible highlight, `1.0` = pure white). This task's own choice.
const SHIMMER_HIGHLIGHT_MIX: f64 = 0.4;

/// The shimmer highlight band's width, as a fraction of the filled width.
/// This task's own choice.
const SHIMMER_BAND_FRACTION: f64 = 0.5;

/// Unthemed fill fallback — the Glyph dark accent (`#ffb627`, same both
/// brightnesses; see `frust_theme::glyph::color`'s accent-role convention).
const FALLBACK_FILL: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Unthemed track fallback — Glyph dark's `bg-overlay` (`#272d3d`, RESEARCH
/// §1.1: "overlays, toggles, progress track").
const FALLBACK_TRACK: Color = Color::from_rgb8(0x27, 0x2d, 0x3d);

const WHITE: Color = Color::from_rgb8(0xff, 0xff, 0xff);

/// A Glyph determinate shimmer progress bar. See the [module docs](self).
pub struct ProgressView {
    value: f64,
}

/// Create a progress bar at `value` (clamped to `0.0..=1.0`).
pub fn progress(value: f64) -> ProgressView {
    ProgressView {
        value: value.clamp(0.0, 1.0),
    }
}

/// PascalCase alias for [`progress`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn Progress(value: f64) -> ProgressView {
    progress(value)
}

impl<State: 'static> View<State> for ProgressView {
    type Element = ProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProgressWidget {
        let mut shimmer = AnimationController::new(SHIMMER_PERIOD).with_curve(Curve::Linear);
        shimmer.repeat();
        ProgressWidget {
            value: self.value,
            shimmer,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.value != self.value {
            element.value = self.value;
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// The retained widget for a [`ProgressView`]. See the [module docs](self).
pub struct ProgressWidget {
    value: f64,
    /// Drives the shimmer sweep's phase (`0.0..=1.0`, repeating, linear). Idle
    /// (advanced but unused) while `reduce_motion` is active.
    shimmer: AnimationController,
}

impl ProgressWidget {
    /// The `(fill, track)` colors: themed `colors.primary_container`/
    /// `colors.surface_container_highest`, or the unthemed [`FALLBACK_FILL`]/
    /// [`FALLBACK_TRACK`] (see the [module docs](self)).
    fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                (scheme.primary_container, scheme.surface_container_highest)
            }
            None => (FALLBACK_FILL, FALLBACK_TRACK),
        }
    }

    /// Build the shimmer's 3-stop `(fill, highlight, fill)` linear gradient,
    /// its `(start, end)` band shifted across `[−band_w, fill_width+band_w]`
    /// by `phase` (see the [module docs](self)'s Shimmer section).
    fn shimmer_gradient(
        origin: Point,
        fill_width: f64,
        height: f64,
        phase: f64,
        fill: Color,
    ) -> Gradient {
        let highlight = Tween::new(fill, WHITE).lerp(SHIMMER_HIGHLIGHT_MIX);
        let band_w = (fill_width * SHIMMER_BAND_FRACTION).max(1.0);
        let travel = fill_width + band_w;
        let x = origin.x - band_w + phase * travel;
        let mid_y = origin.y + height / 2.0;
        Gradient::new_linear(Point::new(x, mid_y), Point::new(x + band_w, mid_y)).with_stops([
            (0.0f32, fill),
            (0.5f32, highlight),
            (1.0f32, fill),
        ])
    }
}

impl Widget for ProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        bc.constrain(Size::new(width, TRACK_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (fill, track) = Self::resolve_colors(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let radius = size.height / 2.0;

        scene.fill_rounded_rect(origin, size, radius, track);

        let fill_width = size.width * self.value;
        if fill_width <= 0.0 {
            return;
        }
        if reduce_motion {
            scene.fill_rounded_rect(origin, Size::new(fill_width, size.height), radius, fill);
            return;
        }
        self.shimmer.advance(ctx.frame_time());
        let phase = self.shimmer.value_clamped();
        let gradient = Self::shimmer_gradient(origin, fill_width, size.height, phase, fill);
        scene.fill_rounded_rect_brush(
            origin,
            Size::new(fill_width, size.height),
            radius,
            &Brush::Gradient(gradient),
        );
        ctx.request_frame();
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ProgressIndicator, |node| {
            node.set_numeric_value(self.value);
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BuildCtx, FrameTime};

    fn build(value: f64) -> ProgressWidget {
        let view = progress(value);
        let mut counter = 0u64;
        <ProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct RecordingScene {
        rrects: Vec<(Point, Size, f64, Color)>,
        brush_rrects: Vec<(Point, Size, f64, Brush)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_rounded_rect_brush(&mut self, o: Point, s: Size, radius: f64, brush: &Brush) {
            self.brush_rrects.push((o, s, radius, brush.clone()));
        }
    }

    fn paint_at(
        w: &mut dyn Widget,
        origin: Point,
        size: Size,
        theme: Option<&Theme>,
    ) -> RecordingScene {
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(origin, size).with_theme(t),
            None => PaintCtx::new(origin, size),
        };
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    #[test]
    fn fills_proportionally_and_clamps() {
        let mut w = build(0.25);
        let scene = paint_at(&mut w, Point::ZERO, Size::new(200.0, TRACK_HEIGHT), None);
        assert_eq!(scene.rrects.len(), 1, "only the track is a plain rrect");
        assert_eq!(scene.rrects[0].1.width, 200.0);
        assert_eq!(
            scene.brush_rrects.len(),
            1,
            "the fill paints via the shimmer brush"
        );
        assert_eq!(scene.brush_rrects[0].1.width, 50.0);

        let mut over = build(1.5);
        let over_scene = paint_at(&mut over, Point::ZERO, Size::new(100.0, TRACK_HEIGHT), None);
        assert_eq!(over_scene.brush_rrects[0].1.width, 100.0, "clamped to 1.0");
    }

    #[test]
    fn zero_value_paints_no_fill() {
        let mut w = build(0.0);
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, TRACK_HEIGHT), None);
        assert_eq!(scene.rrects.len(), 1);
        assert!(scene.brush_rrects.is_empty());
    }

    #[test]
    fn rebuild_reconciles_the_view_supplied_value() {
        let view = progress(0.1);
        let mut w = build(0.1);
        let next = progress(0.9);
        let mut counter = 0u64;
        let flags = <ProgressView as View<()>>::rebuild(
            &next,
            &view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(flags.contains(ChangeFlags::PAINT));
        assert_eq!(w.value, 0.9);
    }

    #[test]
    fn shimmer_phase_advances_linearly_over_the_period() {
        let mut w = build(0.5);
        // Seed the clock, then check the phase at known fractions of the 1.6s
        // period (linear curve → phase == elapsed / period).
        assert!(w.shimmer.advance(ft_secs(0.0)));
        assert!(w.shimmer.advance(ft_secs(0.4)));
        assert!((w.shimmer.value_clamped() - 0.25).abs() < 1e-6);
        assert!(w.shimmer.advance(ft_secs(0.8)));
        assert!((w.shimmer.value_clamped() - 0.5).abs() < 1e-6);
        // Repeats indefinitely.
        assert!(w.shimmer.advance(ft_secs(1.7)));
        assert!((w.shimmer.value_clamped() - (0.1 / 1.6)).abs() < 1e-6);
    }

    #[test]
    fn indeterminate_shimmer_requests_frames_unless_reduce_motion() {
        let mut w = build(0.5);
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, TRACK_HEIGHT), None);
        assert!(!scene.brush_rrects.is_empty());

        // Re-check via ctx.needs_frame() through a real paint call.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, TRACK_HEIGHT));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(
            ctx.needs_frame(),
            "an active shimmer must request another frame"
        );
    }

    #[test]
    fn reduce_motion_paints_a_static_flat_fill_and_stops_requesting_frames() {
        let mut theme = Theme::glyph_baseline();
        theme.motion.reduce_motion = true;
        let mut w = build(0.6);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, TRACK_HEIGHT)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(
            !ctx.needs_frame(),
            "reduce_motion must not request a continuation frame"
        );
        assert_eq!(
            scene.rrects.len(),
            2,
            "track + a plain (non-brush) fill rect"
        );
        assert!(
            scene.brush_rrects.is_empty(),
            "reduce_motion paints a flat fill, not the shimmer brush"
        );
    }

    #[test]
    fn glyph_tokens_resolve_dark_and_light() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);

        let mut w = build(0.5);
        let scene = paint_at(
            &mut w,
            Point::ZERO,
            Size::new(100.0, TRACK_HEIGHT),
            Some(&dark),
        );
        assert_eq!(scene.rrects[0].3, dark.scheme().surface_container_highest);

        let mut w2 = build(0.5);
        let scene2 = paint_at(
            &mut w2,
            Point::ZERO,
            Size::new(100.0, TRACK_HEIGHT),
            Some(&light),
        );
        assert_eq!(scene2.rrects[0].3, light.scheme().surface_container_highest);
        assert_ne!(
            dark.scheme().surface_container_highest,
            light.scheme().surface_container_highest,
            "dark/light overlay tones actually differ"
        );
    }

    #[test]
    fn semantics_reports_progress_indicator_role_and_value() {
        use frust_core::RenderRoot;
        fn logic(_s: &mut ()) -> ProgressView {
            progress(0.42)
        }
        let mut root: RenderRoot<(), ProgressView> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id != update.root)
            .expect("the progress bar contributes exactly one node");
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(node.numeric_value(), Some(0.42));
    }

    #[test]
    fn shimmer_gradient_endpoints_track_the_sweep_phase() {
        // Endpoint sanity: at phase 0.0 the band starts fully off the left
        // edge; at phase 1.0 it has traveled fully past the right edge.
        let g0 =
            ProgressWidget::shimmer_gradient(Point::ZERO, 100.0, TRACK_HEIGHT, 0.0, FALLBACK_FILL);
        let g1 =
            ProgressWidget::shimmer_gradient(Point::ZERO, 100.0, TRACK_HEIGHT, 1.0, FALLBACK_FILL);
        assert_ne!(
            g0.kind, g1.kind,
            "the gradient geometry must move with phase"
        );
        assert_eq!(g0.stops.len(), 3);
    }
}
