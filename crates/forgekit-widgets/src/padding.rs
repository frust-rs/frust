//! Padding layout container (spec §6.2): insets a single child.
//!
//! [`PaddingView`]/[`PaddingWidget`] deflate the incoming constraints by the
//! [`EdgeInsets`], lay the child out in the reduced space, offset it by the
//! top-left insets, and report a size of `child + insets` (clamped to the
//! original constraints).

use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, View, Widget, any,
};
use kurbo::{Point, Size};

/// Per-edge inset amounts, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeInsets {
    /// Inset from the left edge.
    pub left: f64,
    /// Inset from the top edge.
    pub top: f64,
    /// Inset from the right edge.
    pub right: f64,
    /// Inset from the bottom edge.
    pub bottom: f64,
}

impl EdgeInsets {
    /// Equal inset on all four edges.
    pub fn all(value: f64) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }

    /// Symmetric horizontal (`left == right`) and vertical (`top == bottom`) insets.
    pub fn symmetric(horizontal: f64, vertical: f64) -> Self {
        Self {
            left: horizontal,
            top: vertical,
            right: horizontal,
            bottom: vertical,
        }
    }

    /// Total horizontal inset (`left + right`).
    fn horizontal(&self) -> f64 {
        self.left + self.right
    }

    /// Total vertical inset (`top + bottom`).
    fn vertical(&self) -> f64 {
        self.top + self.bottom
    }
}

/// A declarative padding container. See the [module docs](self).
pub struct PaddingView<State: 'static> {
    insets: EdgeInsets,
    child: forgekit_core::AnyView<State>,
}

/// Inset `child` by `insets` on each edge.
#[allow(non_snake_case)]
pub fn Padding<State: 'static, V: View<State>>(insets: EdgeInsets, child: V) -> PaddingView<State> {
    PaddingView {
        insets,
        child: any(child),
    }
}

/// The retained widget for a [`PaddingView`].
pub struct PaddingWidget {
    insets: EdgeInsets,
    child: ChildPod,
}

impl<State: 'static> View<State> for PaddingView<State> {
    type Element = PaddingWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PaddingWidget {
        PaddingWidget {
            insets: self.insets,
            child: crate::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PaddingWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.insets != self.insets {
            element.insets = self.insets;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut PaddingWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for PaddingWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let h = self.insets.horizontal();
        let v = self.insets.vertical();
        // Deflate the constraints by the insets (never below zero — insets larger
        // than the available space collapse the child to nothing).
        let child_bc = BoxConstraints::new(
            Size::new(
                (bc.min().width - h).max(0.0),
                (bc.min().height - v).max(0.0),
            ),
            Size::new(
                (bc.max().width - h).max(0.0),
                (bc.max().height - v).max(0.0),
            ),
        );
        let child_size = self.child.layout_child(ctx, &child_bc);
        self.child
            .set_origin(Point::new(self.insets.left, self.insets.top));
        bc.constrain(Size::new(child_size.width + h, child_size.height + v))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.child.contains(event.position()) {
            self.child.event_child(ctx, event)
        } else {
            EventResult::Ignored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use forgekit_core::BuildCtx;

    fn build<S: 'static>(view: &PaddingView<S>) -> PaddingWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn adds_insets_around_child() {
        // 40x20 child, insets (l=5, t=10, r=15, b=20) → size 60x50, child at (5,10).
        let view: PaddingView<()> = Padding(
            EdgeInsets {
                left: 5.0,
                top: 10.0,
                right: 15.0,
                bottom: 20.0,
            },
            leaf(40.0, 20.0),
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(60.0, 50.0));
        assert_eq!(w.child.origin(), Point::new(5.0, 10.0));
        assert_eq!(w.child.size(), Size::new(40.0, 20.0));
    }

    #[test]
    fn clamps_when_insets_exceed_max() {
        // Uniform 100px inset but only 50x50 available: the child collapses to
        // zero and the padding's own size clamps back to the 50x50 max.
        let view: PaddingView<()> = Padding(EdgeInsets::all(100.0), leaf(30.0, 30.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));
        assert_eq!(w.child.size(), Size::ZERO);
        assert_eq!(size, Size::new(50.0, 50.0));
    }
}
