//! SizedBox layout container (spec §6.2): forces a specific size on the given
//! axes, passing the others through.
//!
//! [`SizedBoxView`]/[`SizedBoxWidget`] tighten each axis for which a `width`/
//! `height` is set (clamped into the incoming constraints) and leave the other
//! axis untouched. A `SizedBox` may be childless — a fixed-size spacer.

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, View, Widget, any,
};
use kurbo::{Point, Size};

/// A declarative fixed-size box, optionally wrapping a child. See the
/// [module docs](self).
pub struct SizedBoxView<State: 'static> {
    width: Option<f64>,
    height: Option<f64>,
    child: Option<AnyView<State>>,
}

/// A box that forces `width`/`height` where `Some`, passing through where `None`.
///
/// Childless by default (a spacer); attach content with [`SizedBoxView::child`].
#[allow(non_snake_case)]
pub fn SizedBox<State: 'static>(width: Option<f64>, height: Option<f64>) -> SizedBoxView<State> {
    SizedBoxView {
        width,
        height,
        child: None,
    }
}

impl<State: 'static> SizedBoxView<State> {
    /// Attach a child sized by this box.
    pub fn child<V: View<State>>(mut self, child: V) -> Self {
        self.child = Some(any(child));
        self
    }
}

/// The retained widget for a [`SizedBoxView`].
pub struct SizedBoxWidget {
    width: Option<f64>,
    height: Option<f64>,
    child: Option<ChildPod>,
}

impl<State: 'static> View<State> for SizedBoxView<State> {
    type Element = SizedBoxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SizedBoxWidget {
        SizedBoxWidget {
            width: self.width,
            height: self.height,
            child: self
                .child
                .as_ref()
                .map(|view| crate::build_child(view, ctx)),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SizedBoxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.width != self.width || prev.height != self.height {
            element.width = self.width;
            element.height = self.height;
            flags |= ChangeFlags::LAYOUT;
        }
        match (&prev.child, &self.child, &mut element.child) {
            (Some(prev_view), Some(next_view), Some(pod)) => {
                flags |= crate::rebuild_child(prev_view, next_view, pod, ctx);
            }
            (None, Some(next_view), _) => {
                element.child = Some(crate::build_child(next_view, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_view), None, Some(pod)) => {
                crate::teardown_child(prev_view, pod, ctx);
                element.child = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }
        flags
    }

    fn teardown(&self, element: &mut SizedBoxWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(view), Some(pod)) = (&self.child, &mut element.child) {
            crate::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for SizedBoxWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve each requested axis, clamped into the incoming constraints.
        let target_w = self.width.map(|w| w.clamp(bc.min().width, bc.max().width));
        let target_h = self
            .height
            .map(|h| h.clamp(bc.min().height, bc.max().height));

        // Tighten the requested axes; pass the rest of the constraint through.
        let child_bc = BoxConstraints::new(
            Size::new(
                target_w.unwrap_or(bc.min().width),
                target_h.unwrap_or(bc.min().height),
            ),
            Size::new(
                target_w.unwrap_or(bc.max().width),
                target_h.unwrap_or(bc.max().height),
            ),
        );

        let size = if let Some(pod) = &mut self.child {
            let child_size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::ZERO);
            // A tightened axis wins; an untightened axis takes the child's choice.
            Size::new(
                target_w.unwrap_or(child_size.width),
                target_h.unwrap_or(child_size.height),
            )
        } else {
            // Childless spacer: tightened axes, else collapse to the minimum.
            Size::new(
                target_w.unwrap_or(bc.min().width),
                target_h.unwrap_or(bc.min().height),
            )
        };
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(pod) = &mut self.child {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(pod) = &mut self.child
            && pod.contains(event.position())
        {
            return pod.event_child(ctx, event);
        }
        EventResult::Ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use forgekit_core::BuildCtx;

    fn build<S: 'static>(view: &SizedBoxView<S>) -> SizedBoxWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn tightens_both_axes_over_child_intrinsic() {
        // Both axes forced to 40x40, overriding the child's 200x10 intrinsic.
        let view: SizedBoxView<()> = SizedBox(Some(40.0), Some(40.0)).child(leaf(200.0, 10.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0, 40.0));
        assert_eq!(w.child.as_ref().unwrap().size(), Size::new(40.0, 40.0));
    }

    #[test]
    fn passes_through_the_unspecified_axis() {
        // Only width is forced (to 40); height passes through to the child's 30.
        let view: SizedBoxView<()> = SizedBox(Some(40.0), None).child(leaf(200.0, 30.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0, 30.0));
    }

    #[test]
    fn childless_box_is_a_fixed_size_spacer() {
        let view: SizedBoxView<()> = SizedBox(Some(16.0), Some(24.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(16.0, 24.0));
    }
}
