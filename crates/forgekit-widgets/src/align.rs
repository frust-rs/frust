//! Align layout container (spec §6.2): positions a single child within the
//! space the align itself is given.
//!
//! [`AlignView`]/[`AlignWidget`] loosen the incoming constraints for the child
//! (so it takes its natural size), grow to fill the available (max) space when it
//! is bounded, and position the child within that box per an [`Alignment`].

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, View, Widget, any,
};
use kurbo::{Point, Size};

/// A relative alignment within a box: each axis runs `-1.0` (start) through
/// `0.0` (center) to `1.0` (end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Horizontal position: `-1.0` = left, `0.0` = center, `1.0` = right.
    pub x: f64,
    /// Vertical position: `-1.0` = top, `0.0` = center, `1.0` = bottom.
    pub y: f64,
}

impl Alignment {
    /// Center of the box.
    pub const CENTER: Self = Self { x: 0.0, y: 0.0 };
    /// Top-left corner.
    pub const TOP_LEFT: Self = Self { x: -1.0, y: -1.0 };
    /// Top-right corner.
    pub const TOP_RIGHT: Self = Self { x: 1.0, y: -1.0 };
    /// Bottom-left corner.
    pub const BOTTOM_LEFT: Self = Self { x: -1.0, y: 1.0 };
    /// Bottom-right corner.
    pub const BOTTOM_RIGHT: Self = Self { x: 1.0, y: 1.0 };

    /// A custom alignment with `x`/`y` each in `-1.0..=1.0`.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Map an alignment component in `-1.0..=1.0` to a `0.0..=1.0` fraction of
    /// the free space (`-1 → 0`, `0 → 0.5`, `1 → 1`).
    fn fraction(component: f64) -> f64 {
        (component + 1.0) / 2.0
    }
}

/// A declarative alignment container. See the [module docs](self).
pub struct AlignView<State: 'static> {
    alignment: Alignment,
    child: AnyView<State>,
}

/// Position `child` within the align's box per `alignment`.
#[allow(non_snake_case)]
pub fn Align<State: 'static, V: View<State>>(alignment: Alignment, child: V) -> AlignView<State> {
    AlignView {
        alignment,
        child: any(child),
    }
}

/// The retained widget for an [`AlignView`].
pub struct AlignWidget {
    alignment: Alignment,
    child: ChildPod,
}

impl<State: 'static> View<State> for AlignView<State> {
    type Element = AlignWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AlignWidget {
        AlignWidget {
            alignment: self.alignment,
            child: crate::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AlignWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.alignment != self.alignment {
            element.alignment = self.alignment;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut AlignWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AlignWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The child takes its natural size under loosened constraints.
        let child_size = self.child.layout_child(ctx, &bc.loosen());
        // Fill the available space on each bounded axis; shrink-wrap an unbounded
        // one to the child.
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            child_size.width
        };
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            child_size.height
        };
        let size = bc.constrain(Size::new(width, height));
        // Position the child within the free space per the alignment fractions.
        let x = (size.width - child_size.width) * Alignment::fraction(self.alignment.x);
        let y = (size.height - child_size.height) * Alignment::fraction(self.alignment.y);
        self.child.set_origin(Point::new(x, y));
        size
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

    fn build<S: 'static>(view: &AlignView<S>) -> AlignWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn placed(alignment: Alignment) -> (Size, Point) {
        // 20x20 child inside a 100x100 align box.
        let view: AlignView<()> = Align(alignment, leaf(20.0, 20.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));
        (size, w.child.origin())
    }

    #[test]
    fn fills_box_and_centers_child() {
        let (size, origin) = placed(Alignment::CENTER);
        assert_eq!(size, Size::new(100.0, 100.0));
        // (100 - 20) * 0.5 = 40 on each axis.
        assert_eq!(origin, Point::new(40.0, 40.0));
    }

    #[test]
    fn positions_child_at_corners() {
        assert_eq!(placed(Alignment::TOP_LEFT).1, Point::new(0.0, 0.0));
        assert_eq!(placed(Alignment::TOP_RIGHT).1, Point::new(80.0, 0.0));
        assert_eq!(placed(Alignment::BOTTOM_LEFT).1, Point::new(0.0, 80.0));
        assert_eq!(placed(Alignment::BOTTOM_RIGHT).1, Point::new(80.0, 80.0));
    }
}
