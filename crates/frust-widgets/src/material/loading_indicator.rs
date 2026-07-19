//! The Material 3 Expressive loading indicator (Phase 6f, PLAN.md Phase B
//! item 2): a continuously-morphing sequence of seven Material shapes.
//!
//! On a linear timer, the indicator steps through [`SHAPE_CYCLE`] one shape
//! every [`PER_SHAPE_PERIOD`] (650 ms — RESEARCH.md:80-81,
//! m3.material.io/components/loading-indicator). The morph *between* two
//! consecutive shapes is driven by a spring (stiffness `200`, damping ratio
//! `0.6` — [`LOADING_SPRING`], same source), so each shape springs into the
//! next with a small expressive overshoot rather than easing linearly.
//!
//! It is not a controlled component — there is nothing for an app to feed it or
//! read back (like [`super::progress`]'s indeterminate mode, it is a pure
//! looping animation). It sizes to [`LOADING_DIAMETER`] and paints in the
//! theme's `colors.primary` (mirroring [`super::progress`]'s theme reads),
//! falling back to [`FALLBACK_FILL`] when no theme is threaded.
//!
//! # The seven-shape cycle
//!
//! Research does not pin *which* shapes the sequence uses, so [`SHAPE_CYCLE`]
//! picks seven visually-distinct rounded polygons spanning the "circle →
//! squircle → n-gon" family (circle, squircle, pentagon, hexagon, heptagon,
//! octagon, rounded triangle), each rotated a little more than the last so the
//! whole figure appears to rotate as it morphs. The list is a private const so
//! a later full M3X shape library (see [`super::shape_morph`]'s scope note) can
//! replace it wholesale.
//!
//! # Timer vs. spring, and dropped frames
//!
//! The linear timer ([`AnimationController::repeat`]) only exposes its
//! `0.0..1.0` phase within the current 650 ms cycle, not a cycle count, so the
//! shape index advances on the phase *wrapping* (this frame's phase < last
//! frame's). A dropped frame longer than one full period therefore skips a
//! shape visually rather than catching every intermediate one — acceptable for
//! a decorative loading spinner, and it always self-corrects on the next frame.

use std::time::Duration;

use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, LayoutCtx, PaintCtx,
    PaintScene, Spring, SpringDesc, View, Widget,
};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::{Brush, Color};

use super::shape_morph::{RoundedPolygon, morph_path};

/// Overall diameter of the indicator, in logical px.
///
/// **Community-approximate**: the M3 loading indicator's active-indicator size
/// is documented as 48dp; no exact per-shape circumradius is published, so the
/// shapes are inscribed in this box (see [`shape_radius`]).
const LOADING_DIAMETER: f64 = 48.0;

/// The circumradius the morphing shapes are drawn at, given the box size.
///
/// A hair inside the half-diameter so the widest shape (the circle) doesn't
/// touch the box edge.
fn shape_radius(diameter: f64) -> f64 {
    diameter / 2.0 * 0.92
}

/// Period the indicator dwells on / morphs through one shape before advancing
/// to the next (source: RESEARCH.md:80-81 — see the [module docs](self)).
const PER_SHAPE_PERIOD: Duration = Duration::from_millis(650);

/// The per-morph spring: stiffness `200`, damping ratio `0.6`.
///
/// Source: RESEARCH.md:80-81 (m3.material.io/components/loading-indicator).
/// Under-damped (ζ `0.6 < 1`), so each morph overshoots slightly before
/// settling — the expressive "spring into the next shape" feel. Mass is `1.0`
/// (every M3 spring preset is mass-1; see `frust_theme::MotionSpring`).
const LOADING_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 200.0,
    damping_ratio: 0.6,
};

/// Unthemed indicator fill fallback (shares [`super::progress`]'s unthemed
/// primary; a theme resolves this from `colors.primary`).
const FALLBACK_FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// The seven-shape morph cycle (see the [module docs](self)). The sequence
/// wraps — the last shape morphs back into the first.
const SHAPE_CYCLE: [RoundedPolygon; 7] = [
    RoundedPolygon::circle(),
    RoundedPolygon::new(4, 0.55, 0.10), // squircle
    RoundedPolygon::new(5, 0.40, 0.55), // pentagon
    RoundedPolygon::new(6, 0.35, 1.00), // hexagon
    RoundedPolygon::new(7, 0.30, 1.45), // heptagon
    RoundedPolygon::new(8, 0.30, 1.90), // octagon
    RoundedPolygon::new(3, 0.50, 2.35), // rounded triangle
];

/// A Material 3 Expressive loading indicator. See the [module docs](self).
pub struct LoadingIndicatorView;

/// Create a loading indicator.
pub fn loading_indicator() -> LoadingIndicatorView {
    LoadingIndicatorView
}

/// PascalCase alias for [`loading_indicator`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn LoadingIndicator() -> LoadingIndicatorView {
    loading_indicator()
}

impl<State: 'static> View<State> for LoadingIndicatorView {
    type Element = LoadingIndicatorWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LoadingIndicatorWidget {
        let mut timer = AnimationController::new(PER_SHAPE_PERIOD).with_curve(Curve::Linear);
        timer.repeat();
        LoadingIndicatorWidget {
            timer,
            index: 0,
            last_phase: 0.0,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut LoadingIndicatorWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Stateless view: the widget owns all animation state and needs no
        // reconciliation.
        ChangeFlags::NONE
    }
}

/// The retained widget for a [`LoadingIndicatorView`]. See the [module docs](self).
pub struct LoadingIndicatorWidget {
    /// Linear 650 ms-period loop; its wrapping phase advances [`Self::index`].
    timer: AnimationController,
    /// The current shape in [`SHAPE_CYCLE`] (morphing toward the next).
    index: usize,
    /// Last frame's timer phase, to detect a wrap (see the [module docs](self)).
    last_phase: f64,
}

impl LoadingIndicatorWidget {
    /// The indicator fill color: themed `colors.primary`, or [`FALLBACK_FILL`].
    fn resolve_color(theme: Option<&Theme>) -> Color {
        match theme {
            Some(theme) => theme.scheme().primary,
            None => FALLBACK_FILL,
        }
    }

    /// Advance the linear timer to frame time `now` and bump [`Self::index`]
    /// when the cycle phase wraps (see the [module docs](self)). Split out of
    /// `paint` so the wrap logic is drivable in a unit test without threading a
    /// shell clock through `PaintCtx` (whose frame-time setter is crate-private
    /// to `frust-core`).
    fn step(&mut self, now: frust_core::FrameTime) {
        self.timer.advance(now);
        let phase = self.timer.value_clamped();
        if phase < self.last_phase {
            self.index = (self.index + 1) % SHAPE_CYCLE.len();
        }
        self.last_phase = phase;
    }

    /// The spring-eased morph parameter for a linear within-cycle `phase`
    /// (`0.0..1.0`). Starts at `0.0` and springs toward `1.0`, overshooting
    /// slightly (ζ `0.6`); returned **unclamped** so the overshoot survives
    /// into the morph.
    fn morph_t(phase: f64) -> f64 {
        let elapsed = phase * PER_SHAPE_PERIOD.as_secs_f64();
        // Released from displacement -1 (i.e. t = 0) toward equilibrium at the
        // target (t = 1), zero initial velocity — the same construction
        // `AnimationController::fling` uses, evaluated directly so each cycle
        // restarts cleanly from the timer phase.
        let spring = Spring::new(LOADING_SPRING, -1.0, 0.0);
        1.0 + spring.position(elapsed)
    }
}

impl Widget for LoadingIndicatorWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(LOADING_DIAMETER, LOADING_DIAMETER))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let color = Self::resolve_color(theme);
        let size = ctx.size();
        let origin = ctx.origin();

        self.step(ctx.frame_time());
        let t = Self::morph_t(self.last_phase);
        let from = &SHAPE_CYCLE[self.index];
        let to = &SHAPE_CYCLE[(self.index + 1) % SHAPE_CYCLE.len()];

        let center = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let radius = shape_radius(size.width.min(size.height));
        let path = morph_path(from, to, t, center, radius);
        scene.fill_path(Point::ZERO, &path, &Brush::Solid(color));

        // Always looping — keep requesting frames.
        ctx.request_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::FrameTime;
    use kurbo::{BezPath, PathEl};

    fn build() -> LoadingIndicatorWidget {
        let view = loading_indicator();
        let mut counter = 0u64;
        <LoadingIndicatorView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[derive(Default)]
    struct RecordingScene {
        fills: Vec<BezPath>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_path(&mut self, _origin: Point, path: &BezPath, _brush: &Brush) {
            self.fills.push(path.clone());
        }
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[test]
    fn spring_constants_match_the_research_values() {
        assert_eq!(LOADING_SPRING.stiffness, 200.0);
        assert_eq!(LOADING_SPRING.damping_ratio, 0.6);
        assert_eq!(LOADING_SPRING.mass, 1.0);
    }

    #[test]
    fn morph_t_starts_at_zero_and_springs_toward_one() {
        // Phase 0 → t exactly 0.
        assert!(LoadingIndicatorWidget::morph_t(0.0).abs() < 1e-12);
        // By the end of the cycle an under-damped ζ=0.6 spring has effectively
        // settled near the target.
        assert!((LoadingIndicatorWidget::morph_t(1.0) - 1.0).abs() < 0.05);
    }

    #[test]
    fn paints_a_filled_path_and_keeps_requesting_frames() {
        // A leaf paint test threads no shell clock (FrameTime::ZERO), so the
        // repeating timer reports "still animating" every frame without
        // advancing — enough to prove paint emits one closed shape and always
        // re-requests a frame, without panicking. Timer-driven index advance is
        // covered by `shape_index_advances_after_one_period_wraps` below.
        let mut w = build();
        for _ in 0..5 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LOADING_DIAMETER, LOADING_DIAMETER));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert_eq!(scene.fills.len(), 1, "paints exactly one morphing shape");
            assert!(matches!(
                scene.fills[0].elements().last(),
                Some(PathEl::ClosePath)
            ));
            assert!(ctx.needs_frame(), "a loop must keep requesting frames");
        }
    }

    #[test]
    fn shape_index_advances_after_one_period_wraps() {
        // Drive the timer step directly (paint delegates to it); this needs a
        // real advancing clock, which `PaintCtx` can't be given from outside
        // `frust-core`.
        let mut w = build();
        assert_eq!(w.index, 0);
        w.step(ft_secs(0.0)); // seed the clock (zero delta)
        // Just before one full 650 ms period: still on the first shape.
        w.step(ft_secs(0.6));
        assert_eq!(w.index, 0);
        // Past one full period: the phase wrapped, advancing to the next shape.
        w.step(ft_secs(0.7));
        assert_eq!(w.index, 1);
    }

    #[test]
    fn index_wraps_around_the_seven_shape_cycle() {
        let mut w = build();
        w.index = SHAPE_CYCLE.len() - 1;
        w.step(ft_secs(0.0)); // seed
        // Advance across several full periods; the modulo keeps the index in
        // range and returns it to the first shape after the last.
        w.step(ft_secs(0.6));
        w.step(ft_secs(0.7)); // wrap from the last shape -> back to 0
        assert_eq!(w.index, 0);
        assert!(w.index < SHAPE_CYCLE.len());
    }
}
