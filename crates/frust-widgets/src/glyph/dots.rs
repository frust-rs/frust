//! Glyph three-dot loader (task 21, glyph-design-system): three dots
//! opacity-pulsing on a 1.2s cycle, each offset 0.15s behind the last — the
//! classic "typing indicator" loop.
//!
//! # One controller, not three
//!
//! A single repeating [`AnimationController`] (`AnimationController::repeat`,
//! [`CYCLE_PERIOD`] = 1.2s, linear) drives a shared clock phase
//! `0.0..=1.0`; each dot `i`'s own pulse phase is `(clock + i *
//! PHASE_OFFSET_FRACTION).rem_euclid(1.0)` — task 07's per-index phase-offset
//! idiom (`frust_core::anim::StaggerSpec`'s "item i starts `i *
//! per_item_delay` later" shape), generalized to a **wrapping** loop via
//! `rem_euclid` (`StaggerSpec::item_progress` itself is a one-shot reveal
//! that saturates at `1.0` past its window — it has no wrap concept, so it
//! doesn't fit a continuously-repeating pulse; the per-index phase-offset
//! math is what's reused, via [`Curve::interval`] below, task 07's other
//! primitive). Each dot's local phase then maps to an opacity pulse via two
//! [`Curve::interval`] halves (`EaseInOut` rising over `[0.0, 0.5]`, falling
//! over `[0.5, 1.0]`) — see [`dot_pulse`].
//!
//! [`CYCLE_PERIOD`]/[`PHASE_OFFSET_SECONDS`]/[`MIN_OPACITY`] are this task's
//! own authored values (no Glyph HTML source dimensions a dot loader).
//!
//! `reduce_motion` freezes every dot at full opacity (no pulse, no
//! `request_frame`) — the Details' "shimmer/dots become static" rule.

use std::time::Duration;

use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, LayoutCtx, PaintCtx,
    PaintScene, View, Widget,
};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

/// Number of dots.
const DOT_COUNT: usize = 3;
/// Each dot's diameter, in logical px. This task's own choice.
const DOT_DIAMETER: f64 = 8.0;
/// Gap between adjacent dots, in logical px. This task's own choice.
const DOT_GAP: f64 = 6.0;

/// The shared clock's full period (`AnimationController::repeat`, linear).
/// This task's own choice — see the [module docs](self).
const CYCLE_PERIOD: Duration = Duration::from_millis(1200);
/// Per-dot phase offset, in seconds (dot `i` lags dot `0` by `i *
/// PHASE_OFFSET_SECONDS`). This task's own choice.
const PHASE_OFFSET_SECONDS: f64 = 0.15;
/// [`PHASE_OFFSET_SECONDS`] expressed as a fraction of [`CYCLE_PERIOD`].
const PHASE_OFFSET_FRACTION: f64 = PHASE_OFFSET_SECONDS / 1.2;

/// The pulse's opacity floor, `0.0..=1.0` (a dot never fully disappears).
/// This task's own choice.
const MIN_OPACITY: f64 = 0.25;

/// Unthemed dot fallback — the Glyph dark accent (`#ffb627`), mirroring
/// `material::loading_indicator`'s unthemed-primary convention.
const FALLBACK_COLOR: Color = Color::from_rgb8(0xff, 0xb6, 0x27);

/// A single dot's smoothed opacity pulse at local phase `t` (`0.0..=1.0`,
/// already wrapped): rises `0.0 → 1.0` over `[0.0, 0.5]`, falls `1.0 → 0.0`
/// over `[0.5, 1.0]`, both eased via [`Curve::EaseInOut`] — task 07's
/// [`Curve::interval`] (see the [module docs](self)). Never negative or
/// above `1.0` (both halves are themselves `[0, 1]`-bounded).
fn dot_pulse(t: f64) -> f64 {
    let rising = Curve::EaseInOut.interval(0.0, 0.5);
    let falling = Curve::EaseInOut.interval(0.5, 1.0);
    rising.transform(t) - falling.transform(t)
}

/// A Glyph three-dot loader. See the [module docs](self).
pub struct DotsLoaderView;

/// Create a three-dot loader.
pub fn dots_loader() -> DotsLoaderView {
    DotsLoaderView
}

/// PascalCase alias for [`dots_loader`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn DotsLoader() -> DotsLoaderView {
    dots_loader()
}

impl<State: 'static> View<State> for DotsLoaderView {
    type Element = DotsLoaderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> DotsLoaderWidget {
        let mut clock = AnimationController::new(CYCLE_PERIOD).with_curve(Curve::Linear);
        clock.repeat();
        DotsLoaderWidget { clock }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut DotsLoaderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Stateless view: the widget owns all animation state.
        ChangeFlags::NONE
    }
}

/// The retained widget for a [`DotsLoaderView`]. See the [module docs](self).
pub struct DotsLoaderWidget {
    /// Shared clock (`0.0..=1.0`, repeating, linear) every dot's phase
    /// offsets from. Idle (advanced but unused) while `reduce_motion` is
    /// active.
    clock: AnimationController,
}

impl DotsLoaderWidget {
    /// The dot fill color: themed `colors.primary`, or [`FALLBACK_COLOR`].
    fn resolve_color(theme: Option<&Theme>) -> Color {
        match theme {
            Some(theme) => theme.scheme().primary,
            None => FALLBACK_COLOR,
        }
    }

    /// Dot `i`'s local (wrapped) pulse phase given the shared `clock` phase.
    fn local_phase(clock_phase: f64, i: usize) -> f64 {
        (clock_phase + i as f64 * PHASE_OFFSET_FRACTION).rem_euclid(1.0)
    }
}

/// Total intrinsic size of the three-dot row.
fn intrinsic_size() -> Size {
    let width = DOT_COUNT as f64 * DOT_DIAMETER + (DOT_COUNT as f64 - 1.0) * DOT_GAP;
    Size::new(width, DOT_DIAMETER)
}

impl Widget for DotsLoaderWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(intrinsic_size())
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let color = Self::resolve_color(theme);
        let origin = ctx.origin();
        let size = ctx.size();
        let cy = origin.y + size.height / 2.0;

        let clock_phase = if reduce_motion {
            None
        } else {
            self.clock.advance(ctx.frame_time());
            Some(self.clock.value_clamped())
        };

        for i in 0..DOT_COUNT {
            let opacity = match clock_phase {
                Some(phase) => {
                    let local = Self::local_phase(phase, i);
                    MIN_OPACITY + (1.0 - MIN_OPACITY) * dot_pulse(local)
                }
                None => 1.0,
            };
            let cx = origin.x + i as f64 * (DOT_DIAMETER + DOT_GAP) + DOT_DIAMETER / 2.0;
            let dot_origin = Point::new(cx - DOT_DIAMETER / 2.0, cy - DOT_DIAMETER / 2.0);
            let dot_size = Size::new(DOT_DIAMETER, DOT_DIAMETER);
            scene.fill_rounded_rect(
                dot_origin,
                dot_size,
                DOT_DIAMETER / 2.0,
                with_alpha(color, opacity as f32),
            );
        }

        if !reduce_motion {
            ctx.request_frame();
        }
    }
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors the
/// per-module helper of the same shape used across `frust-widgets`, e.g.
/// `slider.rs`/`cupertino::switch`).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BuildCtx, FrameTime};

    fn build() -> DotsLoaderWidget {
        let view = dots_loader();
        let mut counter = 0u64;
        <DotsLoaderView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct RecordingScene {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    fn paint_at(w: &mut dyn Widget, size: Size, theme: Option<&Theme>) -> RecordingScene {
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    // -- pure pulse math ----------------------------------------------------

    #[test]
    fn dot_pulse_peaks_at_the_half_cycle_and_troughs_at_the_edges() {
        assert!(dot_pulse(0.0).abs() < 1e-9, "trough at the cycle start");
        assert!(
            (dot_pulse(0.5) - 1.0).abs() < 1e-9,
            "peak at the half cycle"
        );
        assert!(
            dot_pulse(1.0).abs() < 1e-9,
            "trough at the cycle end (wraps to start)"
        );
        // Monotonically rising on the first half, falling on the second.
        assert!(dot_pulse(0.25) < dot_pulse(0.4));
        assert!(dot_pulse(0.6) > dot_pulse(0.9));
    }

    #[test]
    fn per_dot_phase_offsets_are_staggered_and_wrap() {
        // At clock phase 0.0, dot 0 is at local phase 0.0; dot 1/2 are
        // offset forward by PHASE_OFFSET_FRACTION each (task 07's per-index
        // phase-offset shape).
        let p0 = DotsLoaderWidget::local_phase(0.0, 0);
        let p1 = DotsLoaderWidget::local_phase(0.0, 1);
        let p2 = DotsLoaderWidget::local_phase(0.0, 2);
        assert!((p0 - 0.0).abs() < 1e-9);
        assert!((p1 - PHASE_OFFSET_FRACTION).abs() < 1e-9);
        assert!((p2 - 2.0 * PHASE_OFFSET_FRACTION).abs() < 1e-9);

        // Near the end of the cycle, a later dot's phase wraps back toward 0.
        let near_end = 1.0 - PHASE_OFFSET_FRACTION / 2.0;
        let wrapped = DotsLoaderWidget::local_phase(near_end, 1);
        assert!(wrapped < PHASE_OFFSET_FRACTION, "must wrap, not exceed 1.0");
    }

    // -- widget-level phase/paint wiring -------------------------------------

    #[test]
    fn clock_advances_linearly_and_repeats() {
        let mut w = build();
        assert!(w.clock.advance(ft_secs(0.0)));
        assert!(w.clock.advance(ft_secs(0.3)));
        assert!((w.clock.value_clamped() - 0.25).abs() < 1e-6);
        assert!(w.clock.advance(ft_secs(1.3)));
        assert!((w.clock.value_clamped() - (0.1 / 1.2)).abs() < 1e-6);
    }

    #[test]
    fn paints_three_dots_and_requests_frames_unless_reduce_motion() {
        let mut w = build();
        let scene = paint_at(&mut w, intrinsic_size(), None);
        assert_eq!(scene.rrects.len(), DOT_COUNT);

        let mut ctx = PaintCtx::new(Point::ZERO, intrinsic_size());
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame());
    }

    #[test]
    fn reduce_motion_freezes_all_dots_at_full_opacity_and_stops_requesting_frames() {
        let mut theme = Theme::glyph_baseline();
        theme.motion.reduce_motion = true;
        let mut w = build();
        let mut ctx = PaintCtx::new(Point::ZERO, intrinsic_size()).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
        assert_eq!(scene.rrects.len(), DOT_COUNT);
        for (_, _, _, color) in &scene.rrects {
            assert_eq!(
                color.components[3], 1.0,
                "static dots paint at full opacity"
            );
        }
    }

    #[test]
    fn glyph_tokens_resolve_dark_and_light() {
        let mut dark = Theme::glyph_baseline();
        dark.motion.reduce_motion = true;
        let mut light = Theme::glyph_baseline().with_brightness(frust_theme::Brightness::Light);
        light.motion.reduce_motion = true;

        let mut w = build();
        let scene = paint_at(&mut w, intrinsic_size(), Some(&dark));
        assert_eq!(
            scene.rrects[0].3.components[..3],
            dark.scheme().primary.components[..3]
        );

        let mut w2 = build();
        let scene2 = paint_at(&mut w2, intrinsic_size(), Some(&light));
        assert_eq!(
            scene2.rrects[0].3.components[..3],
            light.scheme().primary.components[..3]
        );
    }
}
