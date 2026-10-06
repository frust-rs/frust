//! Stack layout container: a z-ordered overlay of children.
//!
//! [`StackView`]/[`StackWidget`] lay every child under the same loose
//! constraints, positioned at the stack origin so they overlap. The stack sizes
//! itself to the largest child (clamped to its own constraints). Children paint
//! in order (first = bottom-most) and hit-test in reverse order (last = topmost),
//! the same z-order convention [`crate::FlexWidget`] establishes.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

/// A declarative z-ordered stack. See the [module docs](self).
pub struct StackView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Overlay `children` in a z-order stack (first child at the bottom).
///
/// `children` holds one view type; each item is erased here, so a homogeneous
/// list needs no `any()`. A mixed-type list still erases per item, and a bare
/// empty list needs its item type spelled out. The parameter stays a `Vec` so an
/// un-annotated `.collect()` argument keeps inferring; for any other iterable,
/// use [`stack`] with `.children(..)`.
///
/// ```
/// use frust_core::any;
/// use frust_widgets::{Stack, StackView, text};
/// # fn demo() -> (StackView<()>, StackView<()>) {
/// let layers = Stack(vec![text("under"), text("over")]);
/// let mixed = Stack(vec![any(text("under")), any(Stack(vec![text("over")]))]);
/// # (layers, mixed)
/// # }
/// # let _ = demo();
/// ```
#[allow(non_snake_case)]
pub fn Stack<State: 'static, V: View<State>>(children: Vec<V>) -> StackView<State> {
    StackView {
        children: children.into_iter().map(any).collect(),
    }
}

/// An empty z-order stack, ready for fluent children: no `any()` needed.
pub fn stack<State: 'static>() -> StackView<State> {
    StackView {
        children: Vec::new(),
    }
}

impl<State: 'static> StackView<State> {
    /// Append a child on top of the existing ones. Accepts any [`View`].
    pub fn child<V: View<State>>(mut self, view: V) -> Self {
        self.children.push(any(view));
        self
    }

    /// Append every item of `iter` as a child, in order.
    pub fn children<I, V>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: View<State>,
    {
        self.children.extend(iter.into_iter().map(any));
        self
    }

    /// Apply `f` to the builder only when `cond` is true.
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    /// Apply `f` with the contained value when `opt` is `Some`.
    pub fn when_some<T>(self, opt: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match opt {
            Some(value) => f(self, value),
            None => self,
        }
    }
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
                .map(|view| crate::authoring::build_child(view, ctx))
                .collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut StackWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Stack children are plain `AnyView`s with no key storage, so every child
        // reports `None` and reconciliation stays positional. Keyed stacks are a
        // future extension (they would need a keyed child descriptor like Flex's).
        crate::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |view| view,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut StackWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(view, pod, ctx);
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
        crate::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent overlay container: forward each child (bottom-to-top).
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    crate::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{leaf_any, probe};
    use frust_core::{BuildCtx, LayoutCtx, PointerButton, PointerEvent, PointerPhase};

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
        let view: StackView<()> = stack()
            .child(leaf_any(30.0, 60.0))
            .child(leaf_any(80.0, 20.0));
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
        let view: StackView<Vec<u32>> = stack()
            .child(probe(0).into_any())
            .child(probe(1).into_any());
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));

        let mut log: Vec<u32> = Vec::new();
        let mut ectx = frust_core::EventCtx::new(&mut log, Point::ZERO, Size::new(100.0, 100.0));
        let result = w.event(&mut ectx, &down(50.0, 50.0));

        assert_eq!(result, EventResult::Handled);
        // Only the topmost probe (id 1) saw the event — reverse-order hit test.
        assert_eq!(log, vec![1]);
    }
}
