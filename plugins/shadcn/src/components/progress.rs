//! Ports shadcn/ui's **Progress** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/progress.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a `h-2 w-full rounded-full`
//! track (`bg-primary/20`) with a solid `bg-primary` fill sized to `value`.
//!
//! No variants: a single determinate bar, `value` clamped to `0.0..=100.0`.
//! `transition-all` (the fill's width animating toward a new `value`) is not
//! ported — the fill snaps to the new fraction immediately, matching this
//! catalog's `progress` sibling in `material`/`glyph`, neither of which
//! animates the determinate case either.

use frust::Theme;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Role,
    SemanticsCtx, Size, View, Widget,
};

use crate::style::with_alpha;

/// Track height (`h-2`), in logical px.
const HEIGHT: f64 = 8.0;
/// Track/fill alpha for the empty portion — `bg-primary/20`.
const TRACK_ALPHA: f32 = 0.20;

/// Unthemed fallback fill (a theme resolves this from `colors.primary`).
const FALLBACK_FILL: Color = Color::from_rgb8(0x17, 0x17, 0x17);

/// A declarative shadcn progress bar.
pub struct ProgressView {
    value: f64,
}

/// Create a progress bar at `value` percent (`0.0..=100.0`, clamped).
pub fn progress(value: f64) -> ProgressView {
    ProgressView {
        value: value.clamp(0.0, 100.0),
    }
}

/// The retained widget for a [`ProgressView`].
pub struct ProgressWidget {
    value: f64,
}

impl<State: 'static> View<State> for ProgressView {
    type Element = ProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProgressWidget {
        ProgressWidget { value: self.value }
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

/// The track's full-strength fill: themed `colors.primary`, else
/// [`FALLBACK_FILL`]. `TRACK_ALPHA`/opaque is applied by the caller.
fn resolve_fill(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_FILL, |t| t.scheme().primary)
}

impl Widget for ProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let fill = resolve_fill(theme);
        let origin = ctx.origin();
        let size = ctx.size();
        let radius = size.height / 2.0;

        scene.fill_rounded_rect(origin, size, radius, with_alpha(fill, TRACK_ALPHA));

        let fraction = self.value / 100.0;
        let fill_width = size.width * fraction;
        if fill_width > 0.0 {
            // A full-height rounded rect sized to the fraction, at the
            // resting `bg-primary` fill (fully opaque — no hover/press state
            // exists for a non-interactive indicator).
            scene.fill_rounded_rect(origin, Size::new(fill_width, size.height), radius, fill);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ProgressIndicator, |node| {
            node.set_numeric_value(self.value);
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(100.0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::Point;

    fn build(value: f64) -> ProgressWidget {
        let view = progress(value);
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
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

    fn paint(w: &mut ProgressWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn value_clamps_to_0_100() {
        let over = build(150.0);
        assert_eq!(over.value, 100.0);
        let under = build(-10.0);
        assert_eq!(under.value, 0.0);
    }

    #[test]
    fn layout_fills_finite_width_at_track_height() {
        let mut w = build(50.0);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(size, Size::new(200.0, HEIGHT));
    }

    #[test]
    fn unthemed_paint_draws_a_20pct_track_and_a_full_alpha_fill() {
        let mut w = build(50.0);
        let rec = paint(&mut w, Size::new(200.0, HEIGHT), None);
        assert_eq!(rec.rrects.len(), 2);
        assert_eq!(rec.rrects[0].3, with_alpha(FALLBACK_FILL, TRACK_ALPHA));
        assert_eq!(rec.rrects[1].3, FALLBACK_FILL);
        assert_eq!(rec.rrects[1].1.width, 100.0, "50% of 200px");
    }

    #[test]
    fn themed_paint_resolves_the_primary_role() {
        let theme = crate::tokens::theme();
        let mut w = build(25.0);
        let rec = paint(&mut w, Size::new(200.0, HEIGHT), Some(&theme));
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(theme.scheme().primary, TRACK_ALPHA)
        );
        assert_eq!(rec.rrects[1].3, theme.scheme().primary);
        assert_eq!(rec.rrects[1].1.width, 50.0, "25% of 200px");
    }

    #[test]
    fn zero_value_paints_only_the_track() {
        let mut w = build(0.0);
        let rec = paint(&mut w, Size::new(200.0, HEIGHT), None);
        assert_eq!(rec.rrects.len(), 1, "no fill rect below 0%");
    }

    #[test]
    fn rebuild_repaints_only_on_a_value_change() {
        let prev = progress(10.0);
        let same = progress(10.0);
        let next = progress(20.0);
        let mut element = build(10.0);
        let mut counter = 0u64;
        let flags_same =
            View::<()>::rebuild(&same, &prev, &mut element, &mut BuildCtx::new(&mut counter));
        assert_eq!(flags_same, ChangeFlags::NONE);
        let flags_next =
            View::<()>::rebuild(&next, &prev, &mut element, &mut BuildCtx::new(&mut counter));
        assert_eq!(flags_next, ChangeFlags::PAINT);
        assert_eq!(element.value, 20.0);
    }
}
