//! Glyph skeleton placeholder: a fixed-size
//! box painted with a repeating 3-stop gradient sweep — the "content is
//! loading" shimmer placeholder pattern.
//!
//! # Tokens
//!
//! Base = `colors.surface_container` (Glyph's `bg-surface` — "cards, inputs");
//! highlight = `colors.surface_container_high` (Glyph's
//! `bg-raised` — "raised surfaces"), giving a subtle raised sheen as it
//! sweeps. Radius defaults to the themed `shape.small` (Glyph's canonical
//! `--radius-sm` = 6px), overridable via [`SkeletonView::radius`].
//! Unthemed fallbacks below match the Glyph **dark** hex values exactly, per
//! this catalog's charter (`glyph::mod`'s module docs).
//!
//! # Sweep
//!
//! `AnimationController::repeat`, [`SWEEP_PERIOD`] = 1.6s, `Curve::EaseInOut` —
//! original authored values (no Glyph design-reference source dimensions a
//! skeleton loader). The sweep is a linear gradient (base → highlight → base,
//! both real `ColorScheme` roles — no synthetic white-blend) whose `(start,
//! end)` band translates across the box every frame, the same band-translate
//! technique [`super::progress`]'s shimmer uses.
//!
//! `reduce_motion` disables the sweep (paints the flat base, no
//! `request_frame`) — the Details' "shimmer/dots become static" rule.

use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust::{AnimationController, Curve};
use kurbo::{Point, Size};
use peniko::{Brush, Color, Gradient};

/// The sweep's full period (`AnimationController::repeat`, ease-in-out). An
/// original, hand-picked value — see the [module docs](self).
const SWEEP_PERIOD: Duration = Duration::from_millis(1600);

/// The sweep band's width, as a fraction of the box width. An original,
/// hand-picked value.
const SWEEP_BAND_FRACTION: f64 = 0.6;

/// Unthemed corner-radius fallback — Glyph's canonical `--radius-sm` (6px).
const FALLBACK_RADIUS: f64 = 6.0;

/// Unthemed base fallback — Glyph dark's `bg-surface` (`#161a23`).
const FALLBACK_BASE: Color = Color::from_rgb8(0x16, 0x1a, 0x23);
/// Unthemed highlight fallback — Glyph dark's `bg-raised` (`#1e2330`).
const FALLBACK_HIGHLIGHT: Color = Color::from_rgb8(0x1e, 0x23, 0x30);

/// A Glyph skeleton placeholder box. See the [module docs](self).
pub struct SkeletonView {
    width: f64,
    height: f64,
    radius: Option<f64>,
}

/// Create a skeleton placeholder box of the given size.
pub fn skeleton(width: f64, height: f64) -> SkeletonView {
    SkeletonView {
        width: width.max(0.0),
        height: height.max(0.0),
        radius: None,
    }
}

/// PascalCase alias for [`skeleton`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn Skeleton(width: f64, height: f64) -> SkeletonView {
    skeleton(width, height)
}

impl SkeletonView {
    /// Override the corner radius (default: themed `shape.small`, or
    /// [`FALLBACK_RADIUS`] unthemed).
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = Some(radius);
        self
    }
}

impl<State: 'static> View<State> for SkeletonView {
    type Element = SkeletonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SkeletonWidget {
        let mut sweep = AnimationController::new(SWEEP_PERIOD).with_curve(Curve::EaseInOut);
        sweep.repeat();
        SkeletonWidget {
            width: self.width,
            height: self.height,
            radius: self.radius,
            sweep,
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

/// The retained widget for a [`SkeletonView`]. See the [module docs](self).
pub struct SkeletonWidget {
    width: f64,
    height: f64,
    radius: Option<f64>,
    /// Drives the sweep's phase (`0.0..=1.0`, repeating, ease-in-out). Idle
    /// (advanced but unused) while `reduce_motion` is active.
    sweep: AnimationController,
}

impl SkeletonWidget {
    /// The `(base, highlight)` colors: themed `colors.surface_container`/
    /// `colors.surface_container_high`, or the unthemed [`FALLBACK_BASE`]/
    /// [`FALLBACK_HIGHLIGHT`] (see the [module docs](self)).
    fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                (scheme.surface_container, scheme.surface_container_high)
            }
            None => (FALLBACK_BASE, FALLBACK_HIGHLIGHT),
        }
    }

    /// The corner radius: an explicit builder override, else themed
    /// `shape.small`, else [`FALLBACK_RADIUS`].
    fn resolve_radius(&self, theme: Option<&Theme>) -> f64 {
        self.radius
            .unwrap_or_else(|| theme.map(|t| t.shape.small).unwrap_or(FALLBACK_RADIUS))
    }

    /// Build the sweep's 3-stop `(base, highlight, base)` linear gradient —
    /// see [`super::progress::ProgressWidget::shimmer_gradient`] for the
    /// shared band-translation technique.
    fn sweep_gradient(
        origin: Point,
        width: f64,
        height: f64,
        phase: f64,
        base: Color,
        highlight: Color,
    ) -> Gradient {
        let band_w = (width * SWEEP_BAND_FRACTION).max(1.0);
        let travel = width + band_w * 2.0;
        let x = origin.x - band_w + phase * travel;
        let mid_y = origin.y + height / 2.0;
        Gradient::new_linear(Point::new(x, mid_y), Point::new(x + band_w, mid_y)).with_stops([
            (0.0f32, base),
            (0.5f32, highlight),
            (1.0f32, base),
        ])
    }
}

impl Widget for SkeletonWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.width, self.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (base, highlight) = Self::resolve_colors(theme);
        let radius = self.resolve_radius(theme);
        let size = ctx.size();
        let origin = ctx.origin();

        if reduce_motion {
            scene.fill_rounded_rect(origin, size, radius, base);
            return;
        }
        self.sweep.advance(ctx.frame_time());
        let phase = self.sweep.value_clamped();
        let gradient =
            Self::sweep_gradient(origin, size.width, size.height, phase, base, highlight);
        scene.fill_rounded_rect_brush(origin, size, radius, &Brush::Gradient(gradient));
        // A shimmer sweep is a decorative loop — its exact cadence is
        // imperceptible, so the mobile frame gate may pace it.
        ctx.request_frame_paced();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::BuildCtx;

    fn build(w: f64, h: f64) -> SkeletonWidget {
        let view = skeleton(w, h);
        let mut counter = 0u64;
        <SkeletonView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
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

    fn paint_at(w: &mut dyn Widget, size: Size, theme: Option<&Theme>) -> RecordingScene {
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    #[test]
    fn layout_reports_the_given_size() {
        use frust::authoring::LayoutCtx;
        let mut w = build(120.0, 16.0);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(size, Size::new(120.0, 16.0));
    }

    #[test]
    fn negative_dimensions_clamp_to_zero() {
        let mut counter = 0u64;
        let view: SkeletonView = skeleton(-5.0, -1.0);
        let w = <SkeletonView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.width, 0.0);
        assert_eq!(w.height, 0.0);
    }

    #[test]
    fn sweep_phase_advances_ease_in_out_over_the_period() {
        let mut w = build(100.0, 16.0);
        assert!(w.sweep.advance(ft_secs(0.0)));
        assert!(w.sweep.advance(ft_secs(0.8)));
        // Ease-in-out is symmetric; its own midpoint (0.5 through the cycle)
        // is exactly 0.5.
        assert!((w.sweep.value_clamped() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn paints_gradient_brush_and_requests_frames_unless_reduce_motion() {
        let mut w = build(100.0, 16.0);
        let scene = paint_at(&mut w, Size::new(100.0, 16.0), None);
        assert_eq!(scene.rrects.len(), 0);
        assert_eq!(scene.brush_rrects.len(), 1);

        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 16.0));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame());
        assert!(
            ctx.needs_frame_paced_only(),
            "a shimmer sweep is a CosmeticLoop request — the frame gate must be able to pace it"
        );
    }

    #[test]
    fn reduce_motion_paints_flat_base_and_stops_requesting_frames() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build(100.0, 16.0);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 16.0)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
        assert_eq!(scene.rrects.len(), 1);
        assert!(scene.brush_rrects.is_empty());
        assert_eq!(scene.rrects[0].3, theme.scheme().surface_container);
    }

    #[test]
    fn radius_defaults_to_themed_shape_small_or_the_unthemed_fallback() {
        let mut w = build(50.0, 10.0);
        let theme = crate::baseline();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(50.0, 10.0)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.brush_rrects[0].2, theme.shape.small);

        let mut w_override = build(50.0, 10.0);
        let overridden = skeleton(50.0, 10.0).radius(2.0);
        let mut counter = 0u64;
        let flags = <SkeletonView as View<()>>::rebuild(
            &overridden,
            &skeleton(50.0, 10.0),
            &mut w_override,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(flags.contains(ChangeFlags::PAINT));
        assert_eq!(w_override.radius, Some(2.0));
    }

    #[test]
    fn glyph_tokens_resolve_dark_and_light() {
        let dark = crate::baseline();
        let light = dark.clone().with_brightness(frust::Brightness::Light);
        let mut theme_dark = dark.clone();
        theme_dark.motion.reduce_motion = true;
        let mut theme_light = light.clone();
        theme_light.motion.reduce_motion = true;

        let mut w = build(100.0, 16.0);
        let scene = paint_at(&mut w, Size::new(100.0, 16.0), Some(&theme_dark));
        assert_eq!(scene.rrects[0].3, dark.scheme().surface_container);

        let mut w2 = build(100.0, 16.0);
        let scene2 = paint_at(&mut w2, Size::new(100.0, 16.0), Some(&theme_light));
        assert_eq!(scene2.rrects[0].3, light.scheme().surface_container);
        assert_ne!(
            dark.scheme().surface_container,
            light.scheme().surface_container
        );
    }
}
