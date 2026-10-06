//! Integration test for double-erasure swap detection.
//!
//! Verifies that `rebuild_child` correctly detects type swaps when working with
//! double-erased views like `any(any(view_a))` and `any(any(view_b))`. The
//! idempotent `AnyView::new` ensures swap detection compares the inner view's
//! concrete type, not an extra Box layer.

use std::any::Any;

use frust_core::{AnyView, BuildCtx, ChangeFlags, View, any};
use frust_widgets::{authoring::build_child, text};

/// Helper to build a simple AnyView child and check if a rebuild detects a swap.
/// Returns `(flags, swapped)` just like the internal `rebuild_child_tracked`.
fn check_swap(prev: &AnyView<()>, next: &AnyView<()>, counter: &mut u64) -> (ChangeFlags, bool) {
    let mut ctx = BuildCtx::new(counter);
    let mut pod = build_child(prev, &mut ctx);

    // Capture the TypeId before the rebuild.
    let before = {
        let element = pod
            .widget_mut()
            .downcast_mut::<Box<dyn frust_core::widget::Widget>>()
            .expect("child element is a boxed widget");
        let any: &dyn Any = &**element;
        any.type_id()
    };

    // Rebuild the pod with the next view.
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn frust_core::widget::Widget>>()
        .expect("child element is a boxed widget");
    let flags = ctx.with_focus_link(false, |ctx| next.rebuild(prev, element, ctx));

    // Capture the TypeId after the rebuild.
    let after = {
        let element = pod
            .widget_mut()
            .downcast_mut::<Box<dyn frust_core::widget::Widget>>()
            .expect("child element is a boxed widget");
        let any: &dyn Any = &**element;
        any.type_id()
    };

    (flags, before != after)
}

#[test]
fn double_erased_same_type_no_swap() {
    // When rebuilding double-erased views of the same concrete type,
    // swap should not be detected (TypeId is stable).
    let mut counter = 0u64;

    let prev: AnyView<()> = any(any(text("hello")));
    let next: AnyView<()> = any(any(text("world")));

    let (flags, swapped) = check_swap(&prev, &next, &mut counter);

    // Same concrete type (Text) → no swap, typed rebuild.
    // Different text content may trigger PAINT or LAYOUT (or both), but no swap.
    assert!(!swapped, "same type should not be detected as a swap");
    assert!(flags.needs_paint(), "text change should require repaint");
}

#[test]
fn double_erased_different_type_detected_as_swap() {
    // When rebuilding double-erased views of different concrete types,
    // swap should be detected (TypeId differs after the rebuild).
    use frust_widgets::EdgeInsets;
    use frust_widgets::Padding;

    let mut counter = 0u64;

    // First view: Text (simple view)
    let prev: AnyView<()> = any(any(text("hello")));

    // Second view: Padding(Text) (container view with different concrete type)
    let next: AnyView<()> = any(any(Padding(EdgeInsets::all(10.0), text("world"))));

    let (flags, swapped) = check_swap(&prev, &next, &mut counter);

    // Different concrete types → swap detected, full rebuild
    assert!(swapped, "different types should be detected as a swap");
    assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
}

#[test]
fn double_erased_then_single_erased_different_type_detected_as_swap() {
    // Verify the swap is detected when mixing double-erased and single-erased:
    // `any(any(text))` → `any(Padding(text))` should still detect the swap.
    use frust_widgets::EdgeInsets;
    use frust_widgets::Padding;

    let mut counter = 0u64;

    // First view: double-erased Text
    let prev: AnyView<()> = any(any(text("hello")));

    // Second view: single-erased Padding (different type)
    let next: AnyView<()> = any(Padding(EdgeInsets::all(10.0), text("world")));

    let (flags, swapped) = check_swap(&prev, &next, &mut counter);

    // Different concrete types → swap detected
    assert!(
        swapped,
        "double→single with different types should be detected as a swap"
    );
    assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
}
