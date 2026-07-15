//! Layer 1: the declarative [`View`] trait (spec §5).
//!
//! Views are cheap, short-lived descriptors produced by a pure `app_logic`
//! function of application state (`fn app_logic(&mut State) -> impl View<State>`).
//! They are *not* the retained tree — re-running `app_logic` on every state
//! mutation must stay cheap by construction.
//!
//! The lifecycle mirrors `xilem_core`'s proven `View` design
//! (`build`/`rebuild`/`teardown`/`message`) but owns its implementation: no
//! xilem/masonry dependency. The `Action` generic is intentionally omitted for
//! v0 — messages route directly against `State`.

use std::any::Any;

use crate::widget::Widget;

/// Bitflags describing what work a [`View::rebuild`] pass invalidated.
///
/// Combine with `|`. `LAYOUT` implies a subsequent paint, but the flags are
/// stored orthogonally so a caller can distinguish "geometry changed" from
/// "only pixels changed"; use [`ChangeFlags::needs_paint`] for the common
/// "does anything need repainting?" query.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangeFlags(u8);

impl ChangeFlags {
    /// Nothing changed; no downstream work required.
    pub const NONE: Self = Self(0);
    /// The widget must be re-painted.
    pub const PAINT: Self = Self(0b0000_0001);
    /// The widget must be re-laid-out (and therefore re-painted).
    pub const LAYOUT: Self = Self(0b0000_0010);

    /// Whether `self` contains every bit set in `other`.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// The union of two flag sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether no flags are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether this change requires a repaint (either `PAINT` or `LAYOUT`).
    pub const fn needs_paint(self) -> bool {
        !self.is_empty()
    }

    /// Whether this change requires a relayout.
    pub const fn needs_layout(self) -> bool {
        self.contains(Self::LAYOUT)
    }
}

impl core::ops::BitOr for ChangeFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::BitOrAssign for ChangeFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl Default for ChangeFlags {
    fn default() -> Self {
        Self::NONE
    }
}

/// A stable identity for a node in the widget tree.
///
/// Wraps the `u64` node id used by `tree_arena`. Allocated by [`BuildCtx`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WidgetId(pub u64);

impl From<WidgetId> for u64 {
    fn from(id: WidgetId) -> Self {
        id.0
    }
}

/// Context threaded through [`View::build`] / [`View::rebuild`].
///
/// For v0 its sole responsibility is allocating unique [`WidgetId`]s. It borrows
/// the id counter owned by the render root so ids stay monotonic across passes.
pub struct BuildCtx<'a> {
    next_id: &'a mut u64,
}

impl<'a> BuildCtx<'a> {
    /// Create a context borrowing the render root's id counter.
    pub fn new(next_id: &'a mut u64) -> Self {
        Self { next_id }
    }

    /// Allocate a fresh, unique widget id.
    pub fn alloc_id(&mut self) -> WidgetId {
        *self.next_id += 1;
        WidgetId(*self.next_id)
    }
}

/// A declarative description of a piece of UI.
///
/// Each `View` knows how to materialise itself into a retained [`Widget`]
/// ([`View::build`]) and how to reconcile a previous version of itself against
/// the live widget ([`View::rebuild`]). `State` is `'static` so views never
/// capture borrowed data — they are values, re-created every frame.
pub trait View<State: 'static>: 'static {
    /// The retained widget this view produces.
    type Element: Widget;

    /// Materialise a fresh widget for this view.
    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element;

    /// Reconcile `prev` (the previous view of the same type) against the live
    /// `element`, mutating it in place and reporting what changed.
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags;

    /// Tear down `element` when this view is being removed.
    ///
    /// A no-op for v0 leaf views; kept in the trait so the lifecycle is
    /// complete and container/removal logic (later phases) has a hook.
    fn teardown(&self, _element: &mut Self::Element, _ctx: &mut BuildCtx<'_>) {}

    /// Deliver an event message to this view, mutating application state.
    ///
    /// Stubbed for v0 (no event routing yet); present so the trait shape is
    /// stable for task 08 and beyond.
    fn message(&self, _element: &mut Self::Element, _state: &mut State) {}
}

/// Object-safe mirror of [`View`], used only as the erased backing of
/// [`AnyView`].
///
/// The methods mirror `build`/`rebuild`/`teardown` but drop the associated
/// `Element` type in favour of a `Box<dyn Widget>`, and add [`ErasedView::as_any`]
/// so a rebuild can downcast the *previous* erased view to detect a
/// concrete-type change (the xilem `AnyView` trick — see `research/RESEARCH.md`).
trait ErasedView<State: 'static>: 'static {
    /// Materialise a fresh boxed widget for this view.
    fn dyn_build(&self, ctx: &mut BuildCtx<'_>) -> Box<dyn Widget>;

    /// Reconcile against `prev` (the previous erased view). If `prev` is the same
    /// concrete type, do a typed in-place rebuild; otherwise tear the old widget
    /// down and build a fresh one, replacing `element`.
    fn dyn_rebuild(
        &self,
        prev: &dyn ErasedView<State>,
        element: &mut Box<dyn Widget>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags;

    /// Tear down `element` (dispatched to the concrete view's `teardown`).
    fn dyn_teardown(&self, element: &mut Box<dyn Widget>, ctx: &mut BuildCtx<'_>);

    /// Upcast to `&dyn Any` so a rebuild can downcast the previous view.
    fn as_any(&self) -> &dyn Any;
}

impl<State: 'static, V: View<State>> ErasedView<State> for V {
    fn dyn_build(&self, ctx: &mut BuildCtx<'_>) -> Box<dyn Widget> {
        Box::new(self.build(ctx))
    }

    fn dyn_rebuild(
        &self,
        prev: &dyn ErasedView<State>,
        element: &mut Box<dyn Widget>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if let Some(prev) = prev.as_any().downcast_ref::<V>() {
            // Same concrete view type: recover the typed element and rebuild in
            // place. The element's erased type is `V::Element` because it was
            // produced by this view's `build` (via `dyn_build`).
            let element = (**element)
                .downcast_mut::<V::Element>()
                .expect("erased element type matches its originating view");
            self.rebuild(prev, element, ctx)
        } else {
            // Concrete type changed: tear the old widget down through the *old*
            // view, then build a fresh one and swap it in.
            prev.dyn_teardown(element, ctx);
            *element = self.dyn_build(ctx);
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
    }

    fn dyn_teardown(&self, element: &mut Box<dyn Widget>, ctx: &mut BuildCtx<'_>) {
        if let Some(element) = (**element).downcast_mut::<V::Element>() {
            self.teardown(element, ctx);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A type-erased [`View`]: lets a piece of UI change its concrete view type
/// between frames (e.g. a conditional `if cond { text(..) } else { button(..) }`)
/// while still fitting the statically-typed rebuild machinery.
///
/// Rebuild follows the xilem `AnyView` pattern: the previous view is downcast to
/// detect whether the concrete type is unchanged. Same type → a typed in-place
/// rebuild; different type → the old widget is torn down and a fresh one built
/// and swapped in (signalling `LAYOUT | PAINT`). Its `Element` is a
/// `Box<dyn Widget>`, which implements [`Widget`] through the blanket impl so it
/// satisfies `View::Element: Widget`.
pub struct AnyView<State: 'static> {
    inner: Box<dyn ErasedView<State>>,
}

impl<State: 'static> AnyView<State> {
    /// Erase `view` into an `AnyView`.
    pub fn new<V: View<State>>(view: V) -> Self {
        Self {
            inner: Box::new(view),
        }
    }
}

/// Erase `view` into an [`AnyView`] — the free-function spelling of
/// [`AnyView::new`], mirroring the `text(..)`/`button(..)` view-fn vocabulary.
pub fn any<State: 'static, V: View<State>>(view: V) -> AnyView<State> {
    AnyView::new(view)
}

impl<State: 'static> View<State> for AnyView<State> {
    type Element = Box<dyn Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        self.inner.dyn_build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.inner.dyn_rebuild(prev.inner.as_ref(), element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.inner.dyn_teardown(element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::BoxConstraints;
    use crate::widget::{LayoutCtx, PaintCtx, PaintScene, Widget};
    use kurbo::Size;
    use std::cell::Cell;
    use std::rc::Rc;

    // --- AnyView fixtures: two distinct view/widget type pairs. ---

    struct WidgetA {
        n: u32,
    }
    impl Widget for WidgetA {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::new(self.n as f64, 1.0)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    struct WidgetB;
    impl Widget for WidgetB {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::new(99.0, 99.0)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    struct ViewA {
        n: u32,
        torn: Rc<Cell<u32>>,
    }
    impl View<()> for ViewA {
        type Element = WidgetA;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> WidgetA {
            WidgetA { n: self.n }
        }
        fn rebuild(
            &self,
            prev: &Self,
            element: &mut WidgetA,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.n != self.n {
                element.n = self.n;
                ChangeFlags::PAINT
            } else {
                ChangeFlags::NONE
            }
        }
        fn teardown(&self, _element: &mut WidgetA, _ctx: &mut BuildCtx<'_>) {
            self.torn.set(self.torn.get() + 1);
        }
    }

    struct ViewB;
    impl View<()> for ViewB {
        type Element = WidgetB;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> WidgetB {
            WidgetB
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut WidgetB,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn any_view_same_type_rebuilds_in_place() {
        let torn = Rc::new(Cell::new(0));
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);

        let prev = any(ViewA {
            n: 1,
            torn: torn.clone(),
        });
        let mut element = prev.build(&mut ctx);

        let next = any(ViewA {
            n: 2,
            torn: torn.clone(),
        });
        let flags = next.rebuild(&prev, &mut element, &mut ctx);

        // Same concrete type → typed in-place rebuild, no teardown.
        assert_eq!(flags, ChangeFlags::PAINT);
        assert_eq!(torn.get(), 0);
        let a = (*element)
            .downcast_mut::<WidgetA>()
            .expect("still a WidgetA");
        assert_eq!(a.n, 2);
    }

    #[test]
    fn any_view_type_swap_tears_down_and_replaces() {
        let torn = Rc::new(Cell::new(0));
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);

        let prev = any(ViewA {
            n: 7,
            torn: torn.clone(),
        });
        let mut element = prev.build(&mut ctx);

        let next = any(ViewB);
        let flags = next.rebuild(&prev, &mut element, &mut ctx);

        // Concrete type changed → the old view's teardown ran and the widget was
        // replaced with the new type.
        assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
        assert_eq!(torn.get(), 1, "old view should be torn down exactly once");
        assert!((*element).downcast_mut::<WidgetB>().is_some());
        assert!((*element).downcast_mut::<WidgetA>().is_none());
    }

    #[test]
    fn any_view_element_works_inside_a_child_pod() {
        // Criterion 3: `Box<dyn Widget>` implements `Widget` — an AnyView's boxed
        // element drives layout when nested in a container's ChildPod.
        use crate::widget::ChildPod;

        let torn = Rc::new(Cell::new(0));
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);

        let view = any(ViewA { n: 5, torn });
        let element: Box<dyn Widget> = view.build(&mut ctx); // Box<dyn Widget>

        // Nest the boxed widget inside a ChildPod (double-boxed): the blanket
        // `Widget for Box<dyn Widget>` impl forwards layout through both layers.
        let mut pod = ChildPod::new(Box::new(element));
        let mut lctx = LayoutCtx::new();
        let size = pod.layout_child(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
        assert_eq!(size, Size::new(5.0, 1.0));
    }

    #[test]
    fn change_flags_union_and_contains() {
        let both = ChangeFlags::PAINT | ChangeFlags::LAYOUT;
        assert!(both.contains(ChangeFlags::PAINT));
        assert!(both.contains(ChangeFlags::LAYOUT));
        assert!(both.needs_layout());
        assert!(both.needs_paint());

        assert!(ChangeFlags::NONE.is_empty());
        assert!(!ChangeFlags::NONE.needs_paint());

        let paint_only = ChangeFlags::PAINT;
        assert!(paint_only.needs_paint());
        assert!(!paint_only.needs_layout());
    }

    #[test]
    fn change_flags_bitor_assign() {
        let mut f = ChangeFlags::NONE;
        f |= ChangeFlags::PAINT;
        assert!(f.contains(ChangeFlags::PAINT));
        assert!(!f.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn build_ctx_allocates_unique_ids() {
        let mut counter = 0;
        let mut ctx = BuildCtx::new(&mut counter);
        let a = ctx.alloc_id();
        let b = ctx.alloc_id();
        assert_ne!(a, b);
        assert_eq!(u64::from(a), 1);
        assert_eq!(u64::from(b), 2);
    }
}
