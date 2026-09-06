//! `SafeArea` layout container: pads its child by the
//! window's safe-area insets, per enabled edge, floored at a per-edge
//! `.minimum` (Flutter parity — `safe_area.dart:118-127`).
//!
//! Unlike [`Padding`](crate::Padding), the inset amount is resolved
//! dynamically every layout pass from `LayoutCtx::window_insets()`
//! (`WindowInsets`) rather than a view-declared constant — a live inset push
//! (rotation, IME show/hide) must re-pad the child with no view rebuild, so
//! [`SafeAreaWidget`] mirrors `PaddingWidget`'s deflate/offset layout math but
//! recomputes the inset amount itself each pass instead of wrapping a
//! `PaddingWidget`.
//!
//! Flutter's `maintainBottomViewPadding` (an override that substitutes
//! `viewPadding.bottom` for the derived `padding.bottom` while the keyboard is
//! up, to avoid a layout jump) is intentionally **deferred** past v1: the
//! bottom edge always uses the same [`WindowInsets::padding`] formula as every
//! other edge, so an IME overlapping the bottom system inset collapses that
//! edge's resolved padding to zero rather than holding it at the system-bar
//! value.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

/// A declarative safe-area container. See the [module docs](self).
pub struct SafeAreaView<State: 'static> {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
    minimum: crate::EdgeInsets,
    child: frust_core::AnyView<State>,
}

/// Pad `child` by the window's resolved safe-area insets — all four edges
/// enabled by default (opt an edge out with `.left`/`.top`/`.right`/`.bottom`),
/// never less than `.minimum` (zero by default) on an enabled edge.
pub fn safe_area<State: 'static, V: View<State>>(child: V) -> SafeAreaView<State> {
    SafeAreaView {
        left: true,
        top: true,
        right: true,
        bottom: true,
        minimum: crate::EdgeInsets::all(0.0),
        child: any(child),
    }
}

impl<State: 'static> SafeAreaView<State> {
    /// Enable/disable the left-edge inset (default `true`). A disabled edge
    /// still honors `.minimum` (Flutter parity — `.minimum` applies
    /// regardless of whether the edge consumes the window inset).
    pub fn left(mut self, enabled: bool) -> Self {
        self.left = enabled;
        self
    }

    /// Enable/disable the top-edge inset (default `true`).
    pub fn top(mut self, enabled: bool) -> Self {
        self.top = enabled;
        self
    }

    /// Enable/disable the right-edge inset (default `true`).
    pub fn right(mut self, enabled: bool) -> Self {
        self.right = enabled;
        self
    }

    /// Enable/disable the bottom-edge inset (default `true`).
    pub fn bottom(mut self, enabled: bool) -> Self {
        self.bottom = enabled;
        self
    }

    /// Floor for the resolved per-edge padding — an enabled edge never pads by
    /// less than this even where the window inset is smaller, and a disabled
    /// edge still pads by at least this (default zero on all edges). Mirrors
    /// Flutter's `SafeArea.minimum`.
    pub fn minimum(mut self, minimum: crate::EdgeInsets) -> Self {
        self.minimum = minimum;
        self
    }
}

/// The retained widget for a [`SafeAreaView`].
pub struct SafeAreaWidget {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
    minimum: crate::EdgeInsets,
    child: ChildPod,
}

impl<State: 'static> View<State> for SafeAreaView<State> {
    type Element = SafeAreaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SafeAreaWidget {
        SafeAreaWidget {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
            minimum: self.minimum,
            child: crate::authoring::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SafeAreaWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.left != self.left
            || prev.top != self.top
            || prev.right != self.right
            || prev.bottom != self.bottom
            || prev.minimum != self.minimum
        {
            element.left = self.left;
            element.top = self.top;
            element.right = self.right;
            element.bottom = self.bottom;
            element.minimum = self.minimum;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut SafeAreaWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for SafeAreaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve per-edge padding dynamically: the window insets arrive via
        // ctx every pass (unlike Padding's view-declared constant), so this is
        // computed fresh rather than cached on the widget.
        let padding = ctx.window_insets().padding();
        let left = (if self.left { padding.left } else { 0.0 }).max(self.minimum.left);
        let top = (if self.top { padding.top } else { 0.0 }).max(self.minimum.top);
        let right = (if self.right { padding.right } else { 0.0 }).max(self.minimum.right);
        let bottom = (if self.bottom { padding.bottom } else { 0.0 }).max(self.minimum.bottom);

        let h = left + right;
        let v = top + bottom;
        // Deflate the constraints by the resolved insets (never below zero —
        // mirrors Padding's layout math).
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
        self.child.set_origin(Point::new(left, top));
        bc.constrain(Size::new(child_size.width + h, child_size.height + v))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent inset wrapper, mirroring Padding: forward to the child.
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use frust_core::{
        PointerButton, PointerEvent, PointerPhase, RenderRoot, WindowEdgeInsets, WindowInsets,
    };
    use std::any::Any;

    fn safe_area_widget(root: &RenderRoot<(), SafeAreaView<()>>) -> &SafeAreaWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<SafeAreaWidget>()
            .expect("root is a SafeAreaWidget")
    }

    #[test]
    fn no_insets_behaves_like_minimum_padding() {
        // With zero window insets, the resolved padding on every edge is just
        // the (per-edge) minimum — same shape as a `Padding` with that inset.
        fn logic(_: &mut ()) -> SafeAreaView<()> {
            safe_area(leaf(40.0, 20.0)).minimum(crate::EdgeInsets {
                left: 5.0,
                top: 10.0,
                right: 15.0,
                bottom: 20.0,
            })
        }
        let mut root: RenderRoot<(), SafeAreaView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let size = root.layout(Size::new(500.0, 500.0));
        assert_eq!(size, Size::new(60.0, 50.0));
        let w = safe_area_widget(&root);
        assert_eq!(w.child.origin(), Point::new(5.0, 10.0));
        assert_eq!(w.child.size(), Size::new(40.0, 20.0));
    }

    #[test]
    fn pushed_insets_pad_enabled_edges_only() {
        fn logic(_: &mut ()) -> SafeAreaView<()> {
            safe_area(leaf(40.0, 20.0)).right(false)
        }
        let mut root: RenderRoot<(), SafeAreaView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(10.0, 24.0, 30.0, 34.0),
            WindowEdgeInsets::ZERO,
        ));
        let size = root.layout(Size::new(500.0, 500.0));
        let w = safe_area_widget(&root);
        // Left/top/bottom pick up the pushed inset; the disabled right edge
        // stays 0 even though the window inset on that edge is nonzero.
        assert_eq!(w.child.origin(), Point::new(10.0, 24.0));
        assert_eq!(size, Size::new(40.0 + 10.0, 20.0 + 24.0 + 34.0));
    }

    #[test]
    fn minimum_wins_when_larger() {
        fn logic(_: &mut ()) -> SafeAreaView<()> {
            safe_area(leaf(40.0, 20.0)).minimum(crate::EdgeInsets::all(50.0))
        }
        let mut root: RenderRoot<(), SafeAreaView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(10.0, 10.0, 10.0, 10.0),
            WindowEdgeInsets::ZERO,
        ));
        root.layout(Size::new(500.0, 500.0));
        let w = safe_area_widget(&root);
        assert_eq!(
            w.child.origin(),
            Point::new(50.0, 50.0),
            "the larger minimum wins over the smaller pushed inset on every edge"
        );
    }

    #[test]
    fn ime_overlap_clamps_bottom_padding_to_zero() {
        // A 24px status bar / 34px home indicator with a 340px keyboard up:
        // the IME (view_insets) overlaps the bottom system inset, so the
        // derived safe-area padding for that edge collapses to 0 while the
        // top status bar stays intact (WindowInsets::padding's formula).
        fn logic(_: &mut ()) -> SafeAreaView<()> {
            safe_area(leaf(40.0, 20.0))
        }
        let mut root: RenderRoot<(), SafeAreaView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            WindowEdgeInsets::new(0.0, 0.0, 0.0, 340.0),
        ));
        root.layout(Size::new(500.0, 500.0));
        let w = safe_area_widget(&root);
        assert_eq!(
            w.child.origin(),
            Point::new(0.0, 24.0),
            "bottom padding clamps to 0 under the IME overlap; top is unaffected"
        );
    }

    // -- Event routing through the resolved offset --

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn pointer_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// Build a `safe_area(Button)` with a `.minimum` (so the offset is
    /// deterministic under `LayoutCtx::new`'s zero window insets), laid out
    /// under generous constraints.
    fn button_safe_area(minimum: crate::EdgeInsets) -> SafeAreaWidget {
        let view: SafeAreaView<Counter> =
            safe_area(crate::button::<Counter, _>("go", |s: &mut Counter| {
                s.presses += 1
            }))
            .minimum(minimum);
        let mut counter = 0u64;
        let mut w = view.build(&mut BuildCtx::new(&mut counter));
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        w
    }

    #[test]
    fn event_coordinates_route_through_the_offset() {
        let mut w = button_safe_area(crate::EdgeInsets::all(10.0));
        let origin = w.child.origin();
        assert_eq!(origin, Point::new(10.0, 10.0));

        let mut state = Counter::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(500.0, 500.0));

        let inside = Point::new(origin.x + 2.0, origin.y + 2.0);
        assert_eq!(
            w.event(
                &mut ctx,
                &pointer_ev(PointerPhase::Down, inside.x, inside.y)
            ),
            EventResult::Handled
        );
        assert_eq!(
            w.event(&mut ctx, &pointer_ev(PointerPhase::Up, inside.x, inside.y)),
            EventResult::Handled
        );
        drop(ctx);
        assert_eq!(
            state.presses, 1,
            "a synthetic tap at the child's offset location must reach it"
        );
    }
}
