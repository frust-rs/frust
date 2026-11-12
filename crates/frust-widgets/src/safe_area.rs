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
//! # Consumption
//!
//! A safe area both **pads** its child and **removes the padding it consumed
//! from its subtree** (Flutter's `SafeArea` does the same through
//! `MediaQuery.removePadding`). The child is laid out and painted inside
//! `LayoutCtx::with_window_insets` / `PaintCtx::with_window_insets` with
//! [`WindowInsets::consuming`](frust_core::WindowInsets::consuming) applied to
//! the enabled edges, so a descendant reading `ctx.window_insets().padding()`
//! sees `0.0` on every edge this safe area already covered. That keeps a
//! self-insetting descendant (a chrome bar that grows by its own edge's inset)
//! from insetting a second time, and makes nested safe areas consume once: the
//! inner one sees zero on the outer's consumed edges and pads there only by
//! its own `.minimum`. `.minimum` is extra padding, not window inset — it never
//! affects what is consumed. `view_insets` (the IME) always flows through
//! unchanged, and a safe area with every edge disabled passes the insets
//! through untouched.
//!
//! # Keyboard edge
//!
//! By default the bottom edge uses the same [`WindowInsets::padding`] formula
//! as every other edge, so an IME overlapping the bottom system inset
//! collapses that edge's resolved padding to zero and the keyboard itself adds
//! nothing. [`SafeAreaView::keyboard`] opts into holding the bottom edge above
//! the keyboard: with it (and the bottom edge enabled) the bottom padding is
//! `max(view_padding.bottom, view_insets.bottom)`, so the subtree's bottom edge
//! sits at the top of the keyboard when it is up and at the system inset
//! otherwise. Both the bottom `view_padding` and the bottom `view_insets` are
//! then consumed from the subtree, so a self-insetting bar inside does not add
//! the system inset under the keyboard and a nested keyboard safe area does not
//! pad twice.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, View, Widget, WindowInsets, any,
};
use kurbo::{Point, Size};

/// A declarative safe-area container. See the [module docs](self).
pub struct SafeAreaView<State: 'static> {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
    keyboard: bool,
    minimum: crate::EdgeInsets,
    child: frust_core::AnyView<State>,
}

/// Pad `child` by the window's resolved safe-area insets — all four edges
/// enabled by default (opt an edge out with `.left`/`.top`/`.right`/`.bottom`),
/// never less than `.minimum` (zero by default) on an enabled edge. The
/// enabled edges' insets are consumed: `child`'s subtree reads zero safe-area
/// padding there (see the [module docs](self)).
pub fn safe_area<State: 'static, V: View<State>>(child: V) -> SafeAreaView<State> {
    SafeAreaView {
        left: true,
        top: true,
        right: true,
        bottom: true,
        keyboard: false,
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

    /// Hold the bottom edge above the soft keyboard (default `false`). When
    /// `true` and the bottom edge is enabled, the bottom padding is
    /// `max(view_padding.bottom, view_insets.bottom)` (still combined with
    /// `minimum.bottom`): the keyboard's height while it is up, the system
    /// inset otherwise. The subtree's bottom edge therefore sits at the top of
    /// the keyboard. Both the bottom `view_padding` and `view_insets` are
    /// consumed from the subtree. Other edges are unchanged.
    pub fn keyboard(mut self, enabled: bool) -> Self {
        self.keyboard = enabled;
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
    keyboard: bool,
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
            keyboard: self.keyboard,
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
            || prev.keyboard != self.keyboard
            || prev.minimum != self.minimum
        {
            element.left = self.left;
            element.top = self.top;
            element.right = self.right;
            element.bottom = self.bottom;
            element.keyboard = self.keyboard;
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

impl SafeAreaWidget {
    /// `insets` with this safe area's enabled edges consumed — the value its
    /// subtree reads during both layout and paint.
    fn consumed_insets(&self, insets: WindowInsets) -> WindowInsets {
        let mut consumed = insets.consuming(self.left, self.top, self.right, self.bottom);
        if self.keyboard && self.bottom {
            // The keyboard edge covers both the system inset and the keyboard.
            consumed.view_padding.bottom = 0.0;
            consumed.view_insets.bottom = 0.0;
        }
        consumed
    }
}

impl Widget for SafeAreaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve per-edge padding dynamically: the window insets arrive via
        // ctx every pass (unlike Padding's view-declared constant), so this is
        // computed fresh rather than cached on the widget.
        let insets = ctx.window_insets();
        let padding = insets.padding();
        let left = (if self.left { padding.left } else { 0.0 }).max(self.minimum.left);
        let top = (if self.top { padding.top } else { 0.0 }).max(self.minimum.top);
        let right = (if self.right { padding.right } else { 0.0 }).max(self.minimum.right);
        let bottom_inset = if !self.bottom {
            0.0
        } else if self.keyboard {
            insets.view_padding.bottom.max(insets.view_insets.bottom)
        } else {
            padding.bottom
        };
        let bottom = bottom_inset.max(self.minimum.bottom);

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
        // Remove what this safe area consumed from its subtree, so a
        // self-insetting descendant (or a nested safe area) does not inset by
        // the same edge again. `minimum` is extra padding, not window inset, so
        // it plays no part in consumption.
        let consumed = self.consumed_insets(ctx.window_insets());
        let child = &mut self.child;
        let child_size = ctx.with_window_insets(consumed, |ctx| child.layout_child(ctx, &child_bc));
        self.child.set_origin(Point::new(left, top));
        bc.constrain(Size::new(child_size.width + h, child_size.height + v))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The same consumed value the child was laid out under, so a paint-time
        // inset read agrees with the layout-time one.
        let consumed = self.consumed_insets(ctx.window_insets());
        let child = &mut self.child;
        ctx.with_window_insets(consumed, |ctx| child.paint_child(ctx, scene));
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent inset wrapper, mirroring Padding: forward to the child.
        self.child.semantics_child(ctx);
    }

    // Insets its child and paints nothing of its own.
    fn hit_test_self(&self) -> bool {
        false
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

    /// Reports the bottom padding a child sees via `ctx.window_insets()`.
    struct Probe {
        seen: std::rc::Rc<std::cell::Cell<f64>>,
        seen_insets: std::rc::Rc<std::cell::Cell<f64>>,
    }

    impl Widget for Probe {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.seen.set(ctx.window_insets().padding().bottom);
            self.seen_insets.set(ctx.window_insets().view_insets.bottom);
            bc.constrain(Size::new(40.0, 20.0))
        }
        fn paint(&mut self, _: &mut PaintCtx, _: &mut dyn PaintScene) {}
        fn event(&mut self, _: &mut EventCtx, _: &InputEvent) -> EventResult {
            EventResult::Ignored
        }
    }

    fn kb_insets(system: f64, keyboard: f64) -> WindowInsets {
        WindowInsets::new(
            WindowEdgeInsets::new(0.0, 0.0, 0.0, system),
            WindowEdgeInsets::new(0.0, 0.0, 0.0, keyboard),
        )
    }

    #[test]
    fn keyboard_bottom_padding_formula() {
        fn run(kb: bool, system: f64, keyboard: f64) -> f64 {
            fn logic_on(_: &mut ()) -> SafeAreaView<()> {
                safe_area(leaf(40.0, 20.0)).keyboard(true)
            }
            fn logic_off(_: &mut ()) -> SafeAreaView<()> {
                safe_area(leaf(40.0, 20.0)).keyboard(false)
            }
            let mut root: RenderRoot<(), SafeAreaView<()>> = RenderRoot::new();
            let mut state = ();
            if kb {
                root.rebuild(&mut logic_on, &mut state);
            } else {
                root.rebuild(&mut logic_off, &mut state);
            }
            root.set_insets(kb_insets(system, keyboard));
            let size = root.layout(Size::new(500.0, 500.0));
            size.height - 20.0
        }
        assert_eq!(run(true, 34.0, 0.0), 34.0, "no keyboard: system inset");
        assert_eq!(run(true, 34.0, 340.0), 340.0, "keyboard up: its height");
        assert_eq!(run(false, 34.0, 340.0), 0.0, "off: today's collapse");
        assert_eq!(run(false, 34.0, 0.0), 34.0, "off: today's system inset");
    }

    fn probe_pair() -> (
        std::rc::Rc<std::cell::Cell<f64>>,
        std::rc::Rc<std::cell::Cell<f64>>,
    ) {
        (
            std::rc::Rc::new(std::cell::Cell::new(-1.0)),
            std::rc::Rc::new(std::cell::Cell::new(-1.0)),
        )
    }

    fn lay_out_widget(w: &mut SafeAreaWidget, insets: WindowInsets) -> Size {
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        lctx.with_window_insets(insets, |ctx| {
            w.layout(ctx, &BoxConstraints::loose(Size::new(500.0, 500.0)))
        })
    }

    fn widget_with_probe(
        kb: bool,
        nested: bool,
    ) -> (SafeAreaWidget, std::rc::Rc<std::cell::Cell<f64>>) {
        let (seen, seen_insets) = probe_pair();
        let probe = Probe {
            seen: seen.clone(),
            seen_insets,
        };
        let child = ChildPod::new(Box::new(probe));
        let inner = if nested {
            SafeAreaWidget {
                left: true,
                top: true,
                right: true,
                bottom: true,
                keyboard: true,
                minimum: crate::EdgeInsets::all(0.0),
                child,
            }
        } else {
            return (
                SafeAreaWidget {
                    left: true,
                    top: true,
                    right: true,
                    bottom: true,
                    keyboard: kb,
                    minimum: crate::EdgeInsets::all(0.0),
                    child,
                },
                seen,
            );
        };
        (
            SafeAreaWidget {
                left: true,
                top: true,
                right: true,
                bottom: true,
                keyboard: kb,
                minimum: crate::EdgeInsets::all(0.0),
                child: ChildPod::new(Box::new(inner)),
            },
            seen,
        )
    }

    #[test]
    fn keyboard_consumes_padding_for_descendants() {
        for keyboard in [0.0, 340.0] {
            let (mut w, seen) = widget_with_probe(true, false);
            lay_out_widget(&mut w, kb_insets(34.0, keyboard));
            assert_eq!(seen.get(), 0.0, "keyboard {keyboard}");
        }
    }

    #[test]
    fn nested_keyboard_safe_area_adds_nothing() {
        for keyboard in [0.0, 340.0] {
            let (mut w, seen) = widget_with_probe(true, true);
            let size = lay_out_widget(&mut w, kb_insets(34.0, keyboard));
            let expected = if keyboard > 0.0 { keyboard } else { 34.0 };
            assert_eq!(size.height, 20.0 + expected, "padded once");
            assert_eq!(seen.get(), 0.0);
        }
    }

    #[test]
    fn toggling_keyboard_relayouts() {
        let a: SafeAreaView<()> = safe_area(leaf(1.0, 1.0)).keyboard(false);
        let b: SafeAreaView<()> = safe_area(leaf(1.0, 1.0)).keyboard(true);
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let mut el = a.build(&mut ctx);
        let flags = b.rebuild(&a, &mut el, &mut ctx);
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(el.keyboard);
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
