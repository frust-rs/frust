//! Integration test for double-erasure swap detection and wrapper-view residual pin.
//!
//! Verifies that the public `rebuild_child` funnel correctly detects type swaps.
//! The idempotent `AnyView::new` closes the double-erasure case, but a wrapper
//! view with `type Element = Box<dyn Widget>` that re-boxes an inner `AnyView`
//! remains blind to inner type swaps — this test pins that residual
//! (`focus-wrapper-erasure-swap-blind` in docs/LIMITATIONS.md).

use frust_core::{AnyView, BuildCtx, ChangeFlags, View, any};
use frust_widgets::{authoring::build_child, authoring::rebuild_child, text};

/// A local wrapper view that re-boxes an inner AnyView.
/// This replicates the shape of the in-tree ReorderableListView and pins the
/// focus-wrapper-erasure-swap-blind residual.
struct WrapperView<State: 'static> {
    inner: AnyView<State>,
}

impl<State: 'static> View<State> for WrapperView<State> {
    type Element = Box<dyn frust_core::widget::Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        self.inner.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.inner.rebuild(&prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.inner.teardown(element, ctx);
    }
}

#[test]
fn double_erased_same_type_no_swap() {
    // Verify that double-erased views of the same concrete type do not report
    // a swap through rebuild_child. After idempotent AnyView::new, both views
    // preserve their concrete element type (Text), and the focused flag persists.
    let mut counter = 0u64;

    let prev: AnyView<()> = any(any(text("hello")));
    let next: AnyView<()> = any(any(text("world")));

    let mut ctx = BuildCtx::new(&mut counter);
    let mut pod = build_child(&prev, &mut ctx);

    // Mark the pod as focused to test that it persists on a same-type rebuild.
    pod.set_focused(true);
    assert!(pod.is_focused(), "pod should be focused before rebuild");

    // Call rebuild_child through the production funnel.
    let flags = rebuild_child(&prev, &next, &mut pod, &mut ctx);

    // Same concrete type: no swap detected, so focused flag should persist.
    assert!(
        pod.is_focused(),
        "same-type rebuild should not clear the focused flag"
    );

    // Text content change typically results in PAINT flags only.
    // (Exact flags depend on the Text rebuild implementation.)
    assert!(
        flags.needs_paint(),
        "text content change should require repaint"
    );
}

#[test]
fn double_erased_different_type_detected_as_swap() {
    // Verify that double-erased views of different concrete types are detected
    // as a swap through rebuild_child. The focused flag should be cleared,
    // and flags should indicate a full rebuild.
    use frust_widgets::EdgeInsets;
    use frust_widgets::Padding;

    let mut counter = 0u64;

    let prev: AnyView<()> = any(any(text("hello")));
    let next: AnyView<()> = any(any(Padding(EdgeInsets::all(10.0), text("world"))));

    let mut ctx = BuildCtx::new(&mut counter);
    let mut pod = build_child(&prev, &mut ctx);

    // Mark the pod as focused to test that it's cleared on a swap.
    pod.set_focused(true);
    assert!(pod.is_focused(), "pod should be focused before rebuild");

    // Call rebuild_child.
    let flags = rebuild_child(&prev, &next, &mut pod, &mut ctx);

    // Different concrete types: swap detected, so focused flag should be cleared.
    assert!(!pod.is_focused(), "swap should clear the focused flag");

    // A swap triggers a full rebuild (LAYOUT | PAINT).
    assert_eq!(
        flags,
        ChangeFlags::LAYOUT | ChangeFlags::PAINT,
        "swap should report full rebuild flags"
    );
}

#[test]
fn double_erased_then_single_erased_different_type_detected_as_swap() {
    // Verify the swap is detected when mixing double-erased and single-erased:
    // `any(any(text))` → `any(Padding(text))` should still detect the swap
    // and clear the focused flag.
    use frust_widgets::EdgeInsets;
    use frust_widgets::Padding;

    let mut counter = 0u64;

    let prev: AnyView<()> = any(any(text("hello")));
    let next: AnyView<()> = any(Padding(EdgeInsets::all(10.0), text("world")));

    let mut ctx = BuildCtx::new(&mut counter);
    let mut pod = build_child(&prev, &mut ctx);

    // Mark the pod as focused to observe the swap behavior.
    pod.set_focused(true);

    // Call rebuild_child.
    let flags = rebuild_child(&prev, &next, &mut pod, &mut ctx);

    // Different concrete types → swap detected, focused flag cleared.
    assert!(
        !pod.is_focused(),
        "double→single swap should clear the focused flag"
    );

    // Full rebuild.
    assert_eq!(
        flags,
        ChangeFlags::LAYOUT | ChangeFlags::PAINT,
        "swap should report full rebuild flags"
    );
}

#[test]
fn wrapper_view_same_type_rebuild_no_panic() {
    // Verify that a same-type inner rebuild through the wrapper does not panic
    // and preserves the pod's focused flag. With the fixture now correctly
    // returning self.inner.build(ctx) (not double-boxed), AnyView's idempotent
    // erasure closes the double-erasure case, so the rebuild succeeds.
    let mut counter = 0u64;

    // Wrapper containing Text.
    let prev: AnyView<()> = any(WrapperView {
        inner: any(text("hello")),
    });

    // Wrapper containing Text with different content — same inner type, so no swap.
    let next: AnyView<()> = any(WrapperView {
        inner: any(text("goodbye")),
    });

    let mut ctx = BuildCtx::new(&mut counter);
    let mut pod = build_child(&prev, &mut ctx);

    // Mark the pod as focused to test that it persists on a same-type rebuild.
    pod.set_focused(true);
    assert!(pod.is_focused(), "pod should be focused before rebuild");

    // Call rebuild_child through the production funnel. This should not panic
    // (the fixture's element is no longer double-boxed, so AnyView can correctly
    // detect the same concrete type and update in place).
    let flags = rebuild_child(&prev, &next, &mut pod, &mut ctx);

    // Same concrete type: no swap detected, so focused flag should persist.
    assert!(
        pod.is_focused(),
        "same-type rebuild through wrapper should not clear the focused flag"
    );

    // Text content change typically results in PAINT flags only.
    assert!(
        flags.needs_paint(),
        "text content change should require repaint"
    );
}

#[test]
fn wrapper_view_erasure_swap_blind() {
    // Pin the focus-wrapper-erasure-swap-blind residual: a wrapper view with
    // `type Element = Box<dyn Widget>` that re-boxes an inner AnyView remains
    // blind to inner type swaps. This test documents that the blind spot exists
    // and will fail when focus-wrapper-erasure-swap-blind is fixed (shared TypeId
    // reporting through ErasedView).
    use frust_widgets::EdgeInsets;
    use frust_widgets::Padding;

    let mut counter = 0u64;

    // Wrapper containing Text.
    let prev: AnyView<()> = any(WrapperView {
        inner: any(text("hello")),
    });

    // Wrapper containing Padding(Text) — inner type changed, but wrapper's
    // element type is still Box<dyn Widget>, so the swap is blind to rebuild_child.
    let next: AnyView<()> = any(WrapperView {
        inner: any(Padding(EdgeInsets::all(10.0), text("world"))),
    });

    let mut ctx = BuildCtx::new(&mut counter);
    let mut pod = build_child(&prev, &mut ctx);

    // Mark the pod as focused to test the swap-clears-focus behavior.
    pod.set_focused(true);
    assert!(pod.is_focused(), "pod should be focused before rebuild");

    // Call rebuild_child through the production funnel.
    let _flags = rebuild_child(&prev, &next, &mut pod, &mut ctx);

    // The swap is blind: rebuild_child did not detect the inner type swap
    // (both wrapper elements are Box<dyn Widget>), so the pod's focused flag
    // should NOT have been cleared. This test pins the residual and documents
    // that it will flip when focus-wrapper-erasure-swap-blind is fixed.
    assert!(
        pod.is_focused(),
        "wrapper-view swap is blind: focused flag should persist \
         (this test will fail once focus-wrapper-erasure-swap-blind is fixed)"
    );
}
