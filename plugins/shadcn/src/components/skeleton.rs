//! Ports shadcn/ui's **Skeleton** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/skeleton.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a muted rounded placeholder
//! block that pulses (`animate-pulse`) while content loads.
//!
//! # Pulse translation
//!
//! Tailwind's `animate-pulse` keyframes are `0%, 100% { opacity: 1 } 50% {
//! opacity: .5 }` over a 2s `cubic-bezier(0.4, 0, 0.6, 1)` period (source:
//! `tailwindcss@4.3.0`'s default `--animate-pulse` theme value). This port
//! drives an [`AnimationController::repeat`] at that period under
//! [`Curve::EaseInOut`] and folds its `0.0..1.0` phase into a **triangle**
//! wave (0 at the endpoints, 1 at the midpoint) before mapping it onto the
//! `[0.5, 1.0]` opacity band the keyframes describe — reproducing the
//! same "fade down to half, back up to full" shape a two-keyframe CSS
//! animation with an implicit reverse produces.
//!
//! `reduce_motion` paints the flat, fully-opaque block and stops requesting
//! frames — the same skip-animation shape `glyph::skeleton`'s shimmer and
//! `Button`'s loading spinner use.

use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Size, View,
    Widget,
};
use frust::{AnimationController, Curve, Theme};

use crate::style::with_alpha;
use crate::tokens::ShadcnTokens;

/// The pulse's full period — Tailwind's default `animate-pulse` duration (see
/// the [module docs](self)).
const PULSE_PERIOD: Duration = Duration::from_millis(2000);
/// The opacity floor at the pulse's midpoint (`opacity: .5`).
const PULSE_MIN_ALPHA: f32 = 0.5;

/// Unthemed fallback fill (a neutral grey; a theme resolves this from
/// `colors.primary_container`, shadcn's `--accent`).
const FALLBACK_FILL: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback corner radius, in logical px (`rounded-md`).
const FALLBACK_RADIUS: f64 = 8.0;

/// A declarative shadcn skeleton placeholder box.
pub struct SkeletonView {
    width: f64,
    height: f64,
    radius: Option<f64>,
}

/// Create a skeleton placeholder of the given size.
pub fn skeleton(width: f64, height: f64) -> SkeletonView {
    SkeletonView {
        width: width.max(0.0),
        height: height.max(0.0),
        radius: None,
    }
}

impl SkeletonView {
    /// Override the corner radius (default: themed `rounded-md`, else
    /// [`FALLBACK_RADIUS`]).
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = Some(radius);
        self
    }
}

/// The retained widget for a [`SkeletonView`].
pub struct SkeletonWidget {
    width: f64,
    height: f64,
    radius: Option<f64>,
    /// Drives the pulse's phase (`0.0..=1.0`, repeating, ease-in-out). Idle
    /// (advanced but visually unused) while `reduce_motion` is active.
    pulse: AnimationController,
}

impl<State: 'static> View<State> for SkeletonView {
    type Element = SkeletonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SkeletonWidget {
        let mut pulse = AnimationController::new(PULSE_PERIOD).with_curve(Curve::EaseInOut);
        pulse.repeat();
        SkeletonWidget {
            width: self.width,
            height: self.height,
            radius: self.radius,
            pulse,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SkeletonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.width != self.width || prev.height != self.height {
            element.width = self.width;
            element.height = self.height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.radius != self.radius {
            element.radius = self.radius;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl SkeletonWidget {
    /// The block's fill: themed `colors.primary_container` (`--accent`), else
    /// [`FALLBACK_FILL`].
    fn resolve_fill(theme: Option<&Theme>) -> Color {
        theme.map_or(FALLBACK_FILL, |t| t.scheme().primary_container)
    }

    /// The corner radius: an explicit builder override, else the themed
    /// `rounded-md` (`ShadcnTokens::resolve_radius(...).md`), else
    /// [`FALLBACK_RADIUS`].
    fn resolve_radius(&self, theme: Option<&Theme>) -> f64 {
        match self.radius {
            Some(radius) => radius,
            None => match theme {
                Some(theme) => ShadcnTokens::resolve_radius(None, Some(theme)).md,
                None => FALLBACK_RADIUS,
            },
        }
    }

    /// The pulse opacity for phase `t` (`0.0..=1.0`): the triangle-wave
    /// mapping the [module docs](self) describe, landing on `1.0` at the
    /// endpoints and [`PULSE_MIN_ALPHA`] at the midpoint.
    fn pulse_alpha(t: f64) -> f32 {
        let triangle = 1.0 - (2.0 * t - 1.0).abs();
        (1.0 - (1.0 - PULSE_MIN_ALPHA as f64) * triangle) as f32
    }
}

impl Widget for SkeletonWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.width, self.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let fill = Self::resolve_fill(theme);
        let radius = self.resolve_radius(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        if reduce_motion {
            scene.fill_rounded_rect(origin, size, radius, fill);
            return;
        }
        self.pulse.advance(ctx.frame_time());
        let alpha = Self::pulse_alpha(self.pulse.value_clamped());
        scene.fill_rounded_rect(origin, size, radius, with_alpha(fill, alpha));
        // A pulse is a decorative loop — its exact cadence is imperceptible,
        // so the mobile frame gate may pace it.
        ctx.request_frame_paced();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{BuildCtx, Point};

    fn build(w: f64, h: f64) -> SkeletonWidget {
        let view = skeleton(w, h);
        let mut counter = 0u64;
        <SkeletonView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    fn paint_at(w: &mut dyn Widget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        let mut scene = Recorder::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    #[test]
    fn layout_reports_the_given_size_and_clamps_negatives() {
        let mut w = build(120.0, 16.0);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(size, Size::new(120.0, 16.0));

        let mut counter = 0u64;
        let neg: SkeletonView = skeleton(-5.0, -1.0);
        let w2 = <SkeletonView as View<()>>::build(&neg, &mut BuildCtx::new(&mut counter));
        assert_eq!((w2.width, w2.height), (0.0, 0.0));
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_constants() {
        let mut w = build(100.0, 16.0);
        let scene = paint_at(&mut w, Size::new(100.0, 16.0), None);
        assert_eq!(scene.rrects[0].2, FALLBACK_RADIUS);
        // At phase 0 the alpha is full, so the resting color equals the fill.
        assert_eq!(
            scene.rrects[0].3.components[..3],
            FALLBACK_FILL.components[..3]
        );
    }

    #[test]
    fn themed_paint_resolves_the_accent_role_and_rounded_md() {
        let theme = crate::tokens::theme();
        let mut w = build(100.0, 16.0);
        let scene = paint_at(&mut w, Size::new(100.0, 16.0), Some(&theme));
        assert_eq!(
            scene.rrects[0].3.components[..3],
            theme.scheme().primary_container.components[..3]
        );
        assert_eq!(
            scene.rrects[0].2,
            ShadcnTokens::resolve_radius(None, Some(&theme)).md
        );
    }

    #[test]
    fn pulse_alpha_bottoms_out_at_the_midpoint_and_is_full_at_the_endpoints() {
        assert_eq!(SkeletonWidget::pulse_alpha(0.0), 1.0);
        assert_eq!(SkeletonWidget::pulse_alpha(1.0), 1.0);
        assert!((SkeletonWidget::pulse_alpha(0.5) - PULSE_MIN_ALPHA).abs() < 1e-6);
    }

    #[test]
    fn paints_requests_frames_unless_reduce_motion() {
        let mut w = build(100.0, 16.0);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 16.0));
        let mut scene = Recorder::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame());
        assert!(
            ctx.needs_frame_paced_only(),
            "a pulse is a CosmeticLoop request — the frame gate must be able to pace it"
        );
    }

    #[test]
    fn reduce_motion_paints_flat_full_opacity_and_stops_requesting_frames() {
        let mut theme = crate::tokens::theme();
        theme.motion.reduce_motion = true;
        let mut w = build(100.0, 16.0);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 16.0)).with_theme(&theme);
        let mut scene = Recorder::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
        assert_eq!(scene.rrects[0].3, theme.scheme().primary_container);
    }

    #[test]
    fn advance_over_the_full_period_reaches_the_midpoint_alpha() {
        let mut w = build(100.0, 16.0);
        w.pulse.advance(ft_secs(0.0));
        w.pulse.advance(ft_secs(1.0)); // half of the 2s period
        assert!((w.pulse.value_clamped() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn radius_override_wins_over_the_theme() {
        let theme = crate::tokens::theme();
        let mut w = build(50.0, 10.0);
        w.radius = Some(3.0);
        let scene = paint_at(&mut w, Size::new(50.0, 10.0), Some(&theme));
        assert_eq!(scene.rrects[0].2, 3.0);
    }
}
