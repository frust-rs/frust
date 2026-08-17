//! Ports shadcn/ui's **Spinner** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/spinner.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a continuously-rotating
//! `Loader2Icon` (`size-4 animate-spin`).
//!
//! # Icon → stroked arc
//!
//! There is no bundled Lucide icon set to draw the real `Loader2` glyph from,
//! so this port draws the same shape every "activity spinner" resolves to: a
//! partial ring with a gap, rotating at a constant rate — the identical
//! `KurboArc` technique `Button`'s loading state and
//! `material::loading_indicator` use. `animate-spin` is Tailwind's `1s linear
//! infinite`; this drives that exact period/curve via
//! [`AnimationController::repeat`].
//!
//! # Color
//!
//! Lucide icons paint `currentColor` — this component has no surrounding text
//! context to inherit from, so [`SpinnerView::color`] is the explicit override
//! and the theme rung resolves `colors.on_surface_variant` (a muted ink, the
//! common "loading" tone), falling back to [`FALLBACK_INK`] unthemed.
//!
//! `reduce_motion` freezes the arc wherever it currently sits and stops
//! requesting frames, at a dimmed alpha — the same skip-animation shape
//! `Button`'s loading spinner and `material::loading_indicator` use.

use std::f64::consts::{PI, TAU};
use std::time::Duration;

use frust::authoring::{
    BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Point,
    Role, SemanticsCtx, Shape, Size, Vec2, View, Widget,
};
use frust::{AnimationController, Curve, Theme};
use kurbo::Arc as KurboArc;

use crate::style::{ICON_SIZE, scale_alpha};

/// Full-rotation period — Tailwind's `animate-spin` (`1s linear infinite`).
const SPIN_PERIOD: Duration = Duration::from_millis(1000);
/// Sweep angle of the drawn arc, in radians — a partial ring (270°), matching
/// `Button`'s loading-spinner precedent.
const SWEEP: f64 = PI * 1.5;
/// Stroke width, in logical px.
const STROKE_WIDTH: f64 = 2.0;
/// Flattening tolerance for the arc's stroke path.
const PATH_TOLERANCE: f64 = 0.1;
/// Alpha the arc paints at while `reduce_motion` freezes it — dims the frozen
/// glyph rather than leaving it looking like a static, finished ring.
const REDUCED_MOTION_ALPHA: f32 = 0.5;

/// Unthemed fallback ink (a muted grey; a theme resolves this from
/// `colors.on_surface_variant`).
const FALLBACK_INK: Color = Color::from_rgb8(0x73, 0x73, 0x73);

/// A declarative shadcn spinner. See the [module docs](self).
pub struct SpinnerView {
    size: f64,
    color: Option<Color>,
}

/// Create a spinner at the default `size-4` (16px) edge.
pub fn spinner() -> SpinnerView {
    SpinnerView {
        size: ICON_SIZE,
        color: None,
    }
}

impl SpinnerView {
    /// Override the square edge length (default [`ICON_SIZE`], 16px).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0);
        self
    }

    /// Override the ink color — the explicit (top) rung of the resolution
    /// ladder, winning over the theme's `on_surface_variant`.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

/// The retained widget for a [`SpinnerView`].
pub struct SpinnerWidget {
    size: f64,
    color: Option<Color>,
    /// Drives the rotation phase (`0.0..=1.0`, repeating, linear). Advanced
    /// but visually frozen while `reduce_motion` is active.
    spin: AnimationController,
}

impl<State: 'static> View<State> for SpinnerView {
    type Element = SpinnerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SpinnerWidget {
        let mut spin = AnimationController::new(SPIN_PERIOD).with_curve(Curve::Linear);
        spin.repeat();
        SpinnerWidget {
            size: self.size,
            color: self.color,
            spin,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SpinnerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl SpinnerWidget {
    /// The resolved ink: explicit override > themed `on_surface_variant` >
    /// [`FALLBACK_INK`].
    fn resolve_ink(&self, theme: Option<&Theme>) -> Color {
        if let Some(color) = self.color {
            return color;
        }
        theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface_variant)
    }
}

impl Widget for SpinnerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.size, self.size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink = self.resolve_ink(theme);
        let size = ctx.size();
        let radius = (size.width.min(size.height) / 2.0 - STROKE_WIDTH / 2.0).max(0.0);
        let center_local = Point::new(size.width / 2.0, size.height / 2.0);

        let angle = if reduce_motion {
            self.spin.value_clamped() * TAU
        } else {
            self.spin.advance(ctx.frame_time());
            ctx.request_frame_paced();
            self.spin.value_clamped() * TAU
        };
        let color = if reduce_motion {
            scale_alpha(ink, REDUCED_MOTION_ALPHA)
        } else {
            ink
        };

        let arc = KurboArc::new(center_local, Vec2::new(radius, radius), angle, SWEEP, 0.0);
        scene.stroke_path(
            ctx.origin(),
            &arc.to_path(PATH_TOLERANCE),
            STROKE_WIDTH,
            &Brush::Solid(color),
        );
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="status" aria-label="Loading"` upstream.
        ctx.push_node(Role::Status, |node| {
            node.set_label("Loading");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::BezPath;

    fn build(view: &SpinnerView) -> SpinnerWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    #[derive(Default)]
    struct Recorder {
        strokes: Vec<(f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, width: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push((width, *c));
            }
        }
    }

    fn paint(w: &mut SpinnerWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn default_size_is_icon_size() {
        let mut w = build(&spinner());
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(size, Size::new(ICON_SIZE, ICON_SIZE));
    }

    #[test]
    fn size_override_resizes_the_square() {
        let mut w = build(&spinner().size(24.0));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(size, Size::new(24.0, 24.0));
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_ink() {
        let mut w = build(&spinner());
        let rec = paint(&mut w, Size::new(ICON_SIZE, ICON_SIZE), None);
        assert_eq!(rec.strokes[0], (STROKE_WIDTH, FALLBACK_INK));
    }

    #[test]
    fn themed_paint_resolves_on_surface_variant() {
        let theme = crate::tokens::theme();
        let mut w = build(&spinner());
        let rec = paint(&mut w, Size::new(ICON_SIZE, ICON_SIZE), Some(&theme));
        assert_eq!(rec.strokes[0].1, theme.scheme().on_surface_variant);
    }

    #[test]
    fn explicit_color_wins_over_the_theme() {
        let theme = crate::tokens::theme();
        let explicit = Color::from_rgb8(0x12, 0x34, 0x56);
        let mut w = build(&spinner().color(explicit));
        let rec = paint(&mut w, Size::new(ICON_SIZE, ICON_SIZE), Some(&theme));
        assert_eq!(rec.strokes[0].1, explicit);
    }

    #[test]
    fn paints_requests_frames_unless_reduce_motion() {
        let mut w = build(&spinner());
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(ICON_SIZE, ICON_SIZE));
        let mut scene = Recorder::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame());
    }

    #[test]
    fn reduce_motion_freezes_the_arc_dimmed_and_stops_requesting_frames() {
        let mut theme = crate::tokens::theme();
        theme.motion.reduce_motion = true;
        let mut w = build(&spinner());
        let mut ctx =
            PaintCtx::new(Point::ZERO, Size::new(ICON_SIZE, ICON_SIZE)).with_theme(&theme);
        let mut scene = Recorder::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
        assert_eq!(
            scene.strokes[0].1,
            scale_alpha(theme.scheme().on_surface_variant, REDUCED_MOTION_ALPHA)
        );
    }
}
