//! `CupertinoActivityIndicator`: the iOS
//! spinner — 20pt, eight radial "spoke" segments whose opacity falls off around
//! the ring, rotating continuously.
//!
//! Each spoke is a short, round-capped stroked line drawn with the
//! [`PaintScene::stroke_path`] primitive. The indeterminate loop is
//! advanced during [`Widget::paint`] and re-requested via
//! [`PaintCtx::request_frame`] every frame (the crate's shared
//! advance-during-paint contract — see `docs/CODE_STANDARDS.md`'s Theming &
//! Animation Conventions). The active spoke index steps once per
//! `ROTATION_PERIOD`-long loop, so the bright spoke rotates around the ring
//! like the stock `UIActivityIndicatorView`.
//!
//! Color is the theme's `on_surface_variant` (iOS secondaryLabel), or the
//! `SPOKE_FALLBACK` gray when unthemed.

use std::f64::consts::TAU;
use std::time::Duration;

use frust::authoring::Role;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust::{AnimationController, Curve, Theme};
use kurbo::{BezPath, Point, Size};
use peniko::{Brush, Color};

/// The spinner's diameter, in logical px (source: Apple's
/// `UIActivityIndicatorView` `.medium` style is a 20×20pt frame).
const DIAMETER: f64 = 20.0;
/// The number of radial spokes (Apple's classic spinner has eight).
const SPOKE_COUNT: usize = 8;
/// Each spoke's stroke width, in logical px.
///
/// **Community-approximate**: iOS does not publish the spoke geometry; ~2pt
/// wide, round-capped is the value community reimplementations converge on.
const SPOKE_WIDTH: f64 = 2.0;
/// Fraction of the radius the spoke's inner end sits at (the spokes are drawn
/// as a ring of short lines, not full spokes to the center).
///
/// **Community-approximate**: the inner/outer radii below are the
/// community-converged proportions for the classic iOS spinner spoke ring.
const SPOKE_INNER_FRACTION: f64 = 0.42;
/// Fraction of the radius the spoke's outer end sits at.
const SPOKE_OUTER_FRACTION: f64 = 0.90;
/// The minimum opacity a spoke fades to (the dimmest spoke, opposite the bright
/// one). The brightest spoke is fully opaque; the rest ramp linearly between.
const SPOKE_MIN_ALPHA: f32 = 0.15;

/// One full rotation period of the spinner.
///
/// **Community-approximate**: Apple does not publish the spinner's exact
/// period; ~0.8s for a full turn matches the stock spinner's cadence closely.
const ROTATION_PERIOD: Duration = Duration::from_millis(800);

/// Unthemed spoke color fallback — a mid gray (iOS systemGray), used when no
/// theme is threaded (a theme resolves the spoke color from
/// `colors.on_surface_variant`, i.e. iOS secondaryLabel).
const SPOKE_FALLBACK: Color = Color::from_rgb8(0x8E, 0x8E, 0x93);

/// The spoke base color: themed `colors.on_surface_variant`, or the
/// [`SPOKE_FALLBACK`] gray when unthemed.
fn resolve_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface_variant,
        None => SPOKE_FALLBACK,
    }
}

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * alpha])
}

/// A declarative iOS activity indicator (spinner). See the [module docs](self).
pub struct CupertinoActivityIndicatorView {
    /// Whether the spinner animates (a stopped spinner still paints its spokes
    /// at rest — matching `UIActivityIndicatorView`'s `hidesWhenStopped =
    /// false` static appearance).
    animating: bool,
}

/// Create a spinning activity indicator.
pub fn cupertino_activity_indicator() -> CupertinoActivityIndicatorView {
    CupertinoActivityIndicatorView { animating: true }
}

/// PascalCase alias for [`cupertino_activity_indicator`].
#[allow(non_snake_case)]
pub fn CupertinoActivityIndicator() -> CupertinoActivityIndicatorView {
    cupertino_activity_indicator()
}

impl CupertinoActivityIndicatorView {
    /// Set whether the spinner animates (defaults to `true`). A stopped spinner
    /// still paints its (static, dimmed) spoke ring.
    pub fn animating(mut self, animating: bool) -> Self {
        self.animating = animating;
        self
    }
}

/// The retained widget for a [`CupertinoActivityIndicatorView`].
pub struct CupertinoActivityIndicatorWidget {
    animating: bool,
    /// Drives the `0.0..=1.0` rotation loop (repeating while `animating`).
    rotation: AnimationController,
}

impl<State: 'static> View<State> for CupertinoActivityIndicatorView {
    type Element = CupertinoActivityIndicatorWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CupertinoActivityIndicatorWidget {
        let mut rotation = AnimationController::new(ROTATION_PERIOD).with_curve(Curve::Linear);
        if self.animating {
            rotation.repeat();
        }
        CupertinoActivityIndicatorWidget {
            animating: self.animating,
            rotation,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoActivityIndicatorWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.animating != self.animating {
            element.animating = self.animating;
            if self.animating {
                if !element.rotation.is_animating() {
                    element.rotation.repeat();
                }
            } else {
                element.rotation.stop();
            }
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for CupertinoActivityIndicatorWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(DIAMETER, DIAMETER))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let base = resolve_color(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let radius = size.width.min(size.height) / 2.0;
        let center = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);

        // Which spoke is currently the brightest (steps once per loop).
        // `reduce_motion` freezes the rotation wherever it currently sits and
        // stops requesting frames — the same skip-animation shape a shimmer
        // effect uses.
        let phase = if self.animating {
            if !reduce_motion {
                self.rotation.advance(ctx.frame_time());
            }
            self.rotation.value_clamped()
        } else {
            0.0
        };
        let lead = (phase * SPOKE_COUNT as f64).floor() as usize % SPOKE_COUNT;

        let inner = radius * SPOKE_INNER_FRACTION;
        let outer = radius * SPOKE_OUTER_FRACTION;
        for i in 0..SPOKE_COUNT {
            // Angle around the ring (0 = up, stepping clockwise).
            let angle = -std::f64::consts::FRAC_PI_2 + (i as f64) * TAU / SPOKE_COUNT as f64;
            let (sin, cos) = angle.sin_cos();
            let p0 = Point::new(center.x + cos * inner, center.y + sin * inner);
            let p1 = Point::new(center.x + cos * outer, center.y + sin * outer);

            // Opacity falls off with distance *behind* the leading spoke, so the
            // bright spoke trails a comet-like gradient around the ring.
            let dist = (i + SPOKE_COUNT - lead) % SPOKE_COUNT;
            let t = dist as f32 / (SPOKE_COUNT - 1) as f32;
            let alpha = 1.0 - t * (1.0 - SPOKE_MIN_ALPHA);

            let mut path = BezPath::new();
            path.move_to(p0);
            path.line_to(p1);
            scene.stroke_path(
                Point::ZERO,
                &path,
                SPOKE_WIDTH,
                &Brush::Solid(with_alpha(base, alpha)),
            );
        }

        if self.animating && !reduce_motion {
            // The rotating spoke ring is a decorative loop — its exact
            // cadence is imperceptible, so the mobile frame gate may pace it.
            ctx.request_frame_paced();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // An indeterminate spinner reports the progress-indicator role with no
        // numeric value (the "unknown completion" convention).
        ctx.push_node(Role::ProgressIndicator, |_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(animating: bool) -> CupertinoActivityIndicatorWidget {
        let view = cupertino_activity_indicator().animating(animating);
        let mut counter = 0u64;
        <CupertinoActivityIndicatorView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[derive(Default)]
    struct StrokeRecorder {
        strokes: Vec<(BezPath, f64, Color)>,
    }

    impl PaintScene for StrokeRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_path(&mut self, _origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::BLACK,
            };
            self.strokes.push((path.clone(), width, color));
        }
    }

    fn paint(w: &mut CupertinoActivityIndicatorWidget) -> StrokeRecorder {
        let mut rec = StrokeRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(DIAMETER, DIAMETER));
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn layout_is_a_20pt_square() {
        let mut w = build(true);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, Size::new(20.0, 20.0));
    }

    #[test]
    fn paints_eight_spokes() {
        let mut w = build(true);
        let rec = paint(&mut w);
        assert_eq!(rec.strokes.len(), SPOKE_COUNT);
        for (_, width, _) in &rec.strokes {
            assert_eq!(*width, SPOKE_WIDTH);
        }
    }

    #[test]
    fn spoke_opacity_falls_off_around_the_ring() {
        let mut w = build(true);
        let rec = paint(&mut w);
        // At rest (phase 0) spoke 0 is the leading (fully opaque) one; the
        // last-in-trail spoke is the dimmest.
        let lead_alpha = rec.strokes[0].2.components[3];
        let dim_alpha = rec.strokes[SPOKE_COUNT - 1].2.components[3];
        assert!(
            (lead_alpha - 1.0).abs() < 1e-5,
            "leading spoke is fully opaque"
        );
        assert!(dim_alpha < lead_alpha, "trailing spoke is dimmer");
        assert!((dim_alpha - SPOKE_MIN_ALPHA).abs() < 1e-5);
    }

    #[test]
    fn animating_requests_a_frame_every_paint() {
        let mut w = build(true);
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(DIAMETER, DIAMETER));
            let mut rec = StrokeRecorder::default();
            w.paint(&mut ctx, &mut rec);
            assert!(
                ctx.needs_frame(),
                "an animating spinner keeps requesting frames"
            );
            assert!(
                ctx.needs_frame_paced_only(),
                "the rotating spoke ring is a CosmeticLoop request — the frame gate must be able to pace it"
            );
        }
    }

    #[test]
    fn reduce_motion_freezes_the_ring_and_stops_requesting_frames() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build(true);
        for _ in 0..3 {
            let mut ctx =
                PaintCtx::new(Point::ZERO, Size::new(DIAMETER, DIAMETER)).with_theme(&theme);
            let mut rec = StrokeRecorder::default();
            w.paint(&mut ctx, &mut rec);
            assert!(
                !ctx.needs_frame(),
                "reduce_motion must not request a continuation frame"
            );
            assert_eq!(rec.strokes.len(), SPOKE_COUNT, "still paints a frozen ring");
        }
    }

    #[test]
    fn stopped_spinner_does_not_request_frames_but_still_paints() {
        let mut w = build(false);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(DIAMETER, DIAMETER));
        let mut rec = StrokeRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert!(!ctx.needs_frame(), "a stopped spinner is idle");
        assert_eq!(rec.strokes.len(), SPOKE_COUNT, "but still paints its ring");
    }

    #[test]
    fn themed_paint_uses_secondary_label_color() {
        let theme = crate::baseline();
        let mut w = build(false);
        let mut rec = StrokeRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(DIAMETER, DIAMETER)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        // The leading spoke carries the base color at full alpha-multiplier;
        // its RGB must match the themed secondaryLabel role.
        let base = theme.scheme().on_surface_variant;
        let lead = rec.strokes[0].2.components;
        assert!((lead[0] - base.components[0]).abs() < 1e-6);
        assert!((lead[1] - base.components[1]).abs() < 1e-6);
        assert!((lead[2] - base.components[2]).abs() < 1e-6);
    }

    #[test]
    fn semantics_reports_progress_indicator_role() {
        fn logic(_state: &mut ()) -> CupertinoActivityIndicatorView {
            cupertino_activity_indicator()
        }
        let mut root: frust_core::RenderRoot<(), CupertinoActivityIndicatorView> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id != update.root)
            .expect("the spinner contributes one node");
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(
            node.numeric_value(),
            None,
            "indeterminate: no numeric value"
        );
    }
}
