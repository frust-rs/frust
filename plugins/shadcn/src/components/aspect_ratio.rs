//! Ports shadcn/ui's **AspectRatio** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/aspect-ratio.tsx`.
//!
//! Radix's `AspectRatio.Root` is pure layout: it sizes itself to the
//! available width and constrains its height to `width / ratio` (the classic
//! `padding-bottom` trick, done properly via layout rather than CSS here),
//! with the child absolutely filling the resulting box. There is no paint of
//! its own — [`AspectRatioWidget::paint`] does nothing but forward to the
//! child — and no interaction, no theme read, no fallback constant: a pure
//! `layout()` wrapper, the shape [`Widget::layout`]'s own docs point to.
//!
//! An unbounded incoming width (an `AspectRatio` inside something that never
//! constrains it, e.g. a horizontally-scrolling row) has nothing to derive a
//! height from; it falls back to the incoming height (or zero if that is also
//! unbounded) rather than producing an infinite or NaN box.

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, SemanticsCtx, Size, View, Widget, any,
};

/// shadcn/Radix's default ratio (a square) when none is given.
pub const DEFAULT_RATIO: f64 = 1.0;

/// A declarative aspect-ratio wrapper. See the [module docs](self).
pub struct AspectRatioView<State: 'static> {
    ratio: f64,
    child: AnyView<State>,
}

/// Constrain `child` to `ratio` (`width / height`) — `1.0` is a square, `16.0
/// / 9.0` a widescreen frame.
pub fn aspect_ratio<State: 'static, V: View<State>>(
    ratio: f64,
    child: V,
) -> AspectRatioView<State> {
    AspectRatioView {
        ratio,
        child: any(child),
    }
}

/// The retained widget for an [`AspectRatioView`].
pub struct AspectRatioWidget {
    ratio: f64,
    child: ChildPod,
}

impl<State: 'static> View<State> for AspectRatioView<State> {
    type Element = AspectRatioWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AspectRatioWidget {
        AspectRatioWidget {
            ratio: self.ratio,
            child: frust::authoring::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AspectRatioWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.ratio != self.ratio {
            element.ratio = self.ratio;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut AspectRatioWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AspectRatioWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        let width = if max.width.is_finite() {
            max.width
        } else if bc.min().width > 0.0 {
            bc.min().width
        } else {
            0.0
        };
        let ratio = if self.ratio > 0.0 { self.ratio } else { 1.0 };
        let mut height = width / ratio;
        // No finite width to derive from (unbounded on both axes): fall back
        // to whatever height the incoming constraints do supply.
        if width == 0.0 {
            height = if max.height.is_finite() {
                max.height
            } else {
                0.0
            };
        }
        let size = bc.constrain(Size::new(width, height));
        self.child.layout_child(ctx, &BoxConstraints::tight(size));
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    frust::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::BuildCtx;
    use frust_widgets::test_support::leaf;

    fn build<S: 'static>(view: &AspectRatioView<S>) -> AspectRatioWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AspectRatioWidget, bc: BoxConstraints) -> Size {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &bc)
    }

    #[test]
    fn holds_a_16_by_9_ratio_under_loose_constraints() {
        let view: AspectRatioView<()> = aspect_ratio(16.0 / 9.0, leaf(10.0, 10.0));
        let mut w = build(&view);
        let size = layout(&mut w, BoxConstraints::loose(Size::new(400.0, 1000.0)));
        assert_eq!(size.width, 400.0);
        assert!((size.height - 225.0).abs() < 1e-6);
    }

    #[test]
    fn defaults_to_a_square() {
        let view: AspectRatioView<()> = aspect_ratio(DEFAULT_RATIO, leaf(10.0, 10.0));
        let mut w = build(&view);
        let size = layout(&mut w, BoxConstraints::loose(Size::new(200.0, 1000.0)));
        assert_eq!(size, Size::new(200.0, 200.0));
    }

    #[test]
    fn a_tight_ratio_still_fills_the_incoming_width_and_scales_the_child() {
        let view: AspectRatioView<()> = aspect_ratio(2.0, leaf(10.0, 10.0));
        let mut w = build(&view);
        let size = layout(&mut w, BoxConstraints::loose(Size::new(100.0, 1000.0)));
        assert_eq!(size, Size::new(100.0, 50.0));
        // The child is stretched to fill the constrained box, at the origin.
        assert_eq!(w.child.origin(), Point::ZERO);
    }

    #[test]
    fn unbounded_width_falls_back_to_the_incoming_height() {
        let view: AspectRatioView<()> = aspect_ratio(1.0, leaf(10.0, 10.0));
        let mut w = build(&view);
        let size = layout(
            &mut w,
            BoxConstraints::loose(Size::new(f64::INFINITY, 80.0)),
        );
        assert_eq!(size, Size::new(0.0, 80.0));
    }
}
