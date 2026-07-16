//! Stack layout container (spec §6.2): a z-ordered overlay of children.
//!
//! [`StackView`]/[`StackWidget`] lay every child under the same loose
//! constraints, positioned at the stack origin so they overlap. The stack sizes
//! itself to the largest child (clamped to its own constraints). Children paint
//! in order (first = bottom-most) and hit-test in reverse order (last = topmost),
//! the same z-order convention [`crate::FlexWidget`] establishes.

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Point, Size};

/// A declarative z-ordered stack. See the [module docs](self).
pub struct StackView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Overlay `children` in a z-order stack (first child at the bottom).
#[allow(non_snake_case)]
pub fn Stack<State: 'static>(children: Vec<AnyView<State>>) -> StackView<State> {
    StackView { children }
}

/// The retained widget for a [`StackView`].
pub struct StackWidget {
    children: Vec<ChildPod>,
}

impl<State: 'static> View<State> for StackView<State> {
    type Element = StackWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> StackWidget {
        StackWidget {
            children: self
                .children
                .iter()
                .map(|view| crate::build_child(view, ctx))
                .collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut StackWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        crate::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |view| view,
        )
    }

    fn teardown(&self, element: &mut StackWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            crate::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for StackWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = bc.loosen();
        let mut max = Size::ZERO;
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &loose);
            pod.set_origin(Point::ZERO);
            max = Size::new(max.width.max(size.width), max.height.max(size.height));
        }
        bc.constrain(max)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::route_event(&mut self.children, ctx, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{leaf_any, probe};
    use forgekit_core::{BuildCtx, LayoutCtx, PointerButton, PointerEvent, PointerPhase};

    fn build<S: 'static>(view: &StackView<S>) -> StackWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn sizes_to_largest_child() {
        let view: StackView<()> = Stack(vec![leaf_any(30.0, 60.0), leaf_any(80.0, 20.0)]);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        // Widest child is 80 (child 1); tallest is 60 (child 0).
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(80.0, 60.0));
        // Children overlap at the origin.
        assert_eq!(w.children[0].origin(), Point::ZERO);
        assert_eq!(w.children[1].origin(), Point::ZERO);
    }

    #[test]
    fn hit_test_prefers_topmost_child() {
        // Two fully-overlapping probes; the last (topmost) must consume the event.
        let view: StackView<Vec<u32>> = Stack(vec![probe(0).into_any(), probe(1).into_any()]);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));

        let mut log: Vec<u32> = Vec::new();
        let mut ectx = forgekit_core::EventCtx::new(&mut log, Point::ZERO, Size::new(100.0, 100.0));
        let result = w.event(&mut ectx, &down(50.0, 50.0));

        assert_eq!(result, EventResult::Handled);
        // Only the topmost probe (id 1) saw the event — reverse-order hit test.
        assert_eq!(log, vec![1]);
    }
}
