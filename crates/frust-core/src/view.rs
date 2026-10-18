//! Layer 1: the declarative [`View`] trait.
//!
//! Views are cheap, short-lived descriptors produced by a root component's
//! `Component::build`, a pure function of application state. They are not the
//! retained tree: re-running the build on every state mutation must stay cheap.
//!
//! The lifecycle mirrors `xilem_core`'s `View` (`build`/`rebuild`/`teardown`/
//! `message`) but owns its implementation. There is no `Action` generic:
//! messages route directly against `State`.

use std::any::Any;

use crate::widget::Widget;

/// Bitflags describing what work a [`View::rebuild`] pass invalidated; combine with `|`.
///
/// `LAYOUT` implies a paint but the flags are stored orthogonally;
/// [`ChangeFlags::needs_paint`] answers "does anything need repainting?".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangeFlags(u8);

impl ChangeFlags {
    /// Nothing changed.
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

/// A stable identity for a node in the widget tree: the `tree_arena` node id, allocated by [`BuildCtx`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WidgetId(pub u64);

impl From<WidgetId> for u64 {
    fn from(id: WidgetId) -> Self {
        id.0
    }
}

/// Context threaded through [`View::build`] / [`View::rebuild`]: allocates unique
/// [`WidgetId`]s from the render root's counter and carries the effective focus
/// chain ([`BuildCtx::has_focus`]) and element-replaced signal
/// ([`BuildCtx::mark_element_replaced`]).
pub struct BuildCtx<'a> {
    next_id: &'a mut u64,
    has_focus: bool,
    element_replaced: bool,
    /// The owning root's last paint snapshot; `None` outside a root.
    hints: Option<std::rc::Rc<[crate::shortcuts::ShortcutHint]>>,
}

impl<'a> BuildCtx<'a> {
    /// Create a context borrowing the render root's id counter.
    ///
    /// The focus chain seeds `true` ("unknown, assume live"): over-releasing costs
    /// one tap, while under-releasing strands the shell's IME surface over a dead
    /// widget. Seams that know the real value set it with [`BuildCtx::set_has_focus`].
    pub fn new(next_id: &'a mut u64) -> Self {
        Self {
            next_id,
            has_focus: true,
            element_replaced: false,
            hints: None,
        }
    }

    /// Hand the context its root's last paint snapshot ([`BuildCtx::shortcut_hints`]).
    pub fn with_shortcut_hints(
        mut self,
        hints: std::rc::Rc<[crate::shortcuts::ShortcutHint]>,
    ) -> Self {
        self.hints = Some(hints);
        self
    }

    /// Allocate a fresh, unique widget id.
    pub fn alloc_id(&mut self) -> WidgetId {
        *self.next_id += 1;
        WidgetId(*self.next_id)
    }

    /// Whether the recorded focus path is live from the root to the node being
    /// built/rebuilt/torn down: the rebuild-pass mirror of
    /// [`PaintCtx::has_focus`](crate::widget::PaintCtx::has_focus).
    ///
    /// A blur clears the link at the nearest common ancestor only, so a `focused`
    /// [`ChildPod`](crate::widget::ChildPod) flag below it goes stale harmlessly. A
    /// reconciler raises [`mark_focus_orphaned`](crate::event::mark_focus_orphaned)
    /// only when `ctx.has_focus() && pod.is_focused()`: the session is actually live.
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// The previous paint's shortcut declarations in paint order, unreachable ones
    /// included; label each chord with `Chord::display`. They lag this build by a frame, and are
    /// empty on a context its root did not seed.
    pub fn shortcut_hints(&self) -> std::rc::Rc<[crate::shortcuts::ShortcutHint]> {
        self.hints
            .clone()
            .unwrap_or_else(|| std::rc::Rc::from(Vec::new()))
    }

    /// Seed the focus chain for a seam that starts one (`RenderRoot`'s view diff, or a
    /// component carrying its outer chain across an inner `BuildCtx`). A container
    /// descending into a pod uses [`BuildCtx::with_focus_link`], which can only narrow.
    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// Run `f` with the focus chain extended by one link (`link_focused` is the pod's
    /// `is_focused` flag), restoring it afterwards: the closure sees
    /// `self.has_focus() && link_focused`, so no descent can widen the chain.
    pub fn with_focus_link<R>(&mut self, link_focused: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.has_focus;
        self.has_focus = outer && link_focused;
        let result = f(self);
        self.has_focus = outer;
        result
    }

    /// Report that this view's `rebuild` replaced its element's widget while keeping
    /// the element type (an enum view switching arms, e.g. `Either`).
    ///
    /// A reconciler detects an [`AnyView`] type swap by type id and drops the child
    /// pod's capture and focus links; an enum keeps one static type, so it raises this
    /// signal instead. The nearest enclosing
    /// [`track_element_replaced`](BuildCtx::track_element_replaced) scope consumes it
    /// (a pod inside the swapped subtree consumes its own); with no scope it is inert
    /// or reaches the next one up: an over-release, never an under-release.
    pub fn mark_element_replaced(&mut self) {
        self.element_replaced = true;
    }

    /// Run `f` as a tracking scope for [`mark_element_replaced`](BuildCtx::mark_element_replaced),
    /// returning `f`'s result and whether anything inside raised the signal. The
    /// caller's pending value is saved and restored.
    pub fn track_element_replaced<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> (R, bool) {
        let outer = std::mem::replace(&mut self.element_replaced, false);
        let result = f(self);
        let replaced = std::mem::replace(&mut self.element_replaced, outer);
        (result, replaced)
    }
}

/// A declarative description of a piece of UI.
///
/// A `View` builds itself into a retained [`Widget`] ([`View::build`]) and
/// reconciles a previous version against the live widget ([`View::rebuild`]).
/// `State` is `'static`, so views are values re-created every frame.
pub trait View<State: 'static>: 'static {
    /// The retained widget this view produces.
    type Element: Widget;

    /// Materialise a fresh widget for this view.
    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element;

    /// Reconcile `prev` (the previous view of the same type) against the live `element`, in place.
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags;

    /// Tear down `element` when this view is being removed; a no-op for leaf views.
    fn teardown(&self, _element: &mut Self::Element, _ctx: &mut BuildCtx<'_>) {}

    /// Deliver an event message to this view, mutating application state (stubbed:
    /// no event routing goes through it yet).
    fn message(&self, _element: &mut Self::Element, _state: &mut State) {}
}

/// Object-safe mirror of [`View`], the erased backing of [`AnyView`]; `as_any` lets a rebuild downcast the previous view.
trait ErasedView<State: 'static>: 'static {
    fn dyn_build(&self, ctx: &mut BuildCtx<'_>) -> Box<dyn Widget>;

    /// Reconcile against `prev`: a typed in-place rebuild if it is the same concrete
    /// type, otherwise tear the old widget down and build a fresh one.
    fn dyn_rebuild(
        &self,
        prev: &dyn ErasedView<State>,
        element: &mut Box<dyn Widget>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags;

    fn dyn_teardown(&self, element: &mut Box<dyn Widget>, ctx: &mut BuildCtx<'_>);

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
            let element = (**element)
                .downcast_mut::<V::Element>()
                .expect("erased element type matches its originating view");
            self.rebuild(prev, element, ctx)
        } else {
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
/// between frames while still fitting the statically typed rebuild machinery.
///
/// The previous view is downcast to detect a type change: same type is a typed
/// in-place rebuild; otherwise the old widget is torn down and a fresh one swapped
/// in (`LAYOUT | PAINT`). Its `Element` is a `Box<dyn Widget>`.
///
/// Generic builders erase internally, so app code needs `AnyView` only for stored
/// views, branches of more than eight arms, and heterogeneous dynamic lists (see
/// [`any`]); `either(..)` and `EitherOf3`..`EitherOf8` stay statically typed.
pub struct AnyView<State: 'static> {
    inner: Box<dyn ErasedView<State>>,
}

impl<State: 'static> AnyView<State> {
    /// Erase `view` into an `AnyView`; idempotent, so `any(any(v))` and
    /// `AnyView::new(any_view)` do not box twice.
    pub fn new<V: View<State>>(view: V) -> Self {
        let mut slot = Some(view);
        if let Some(erased) =
            (&mut slot as &mut dyn core::any::Any).downcast_mut::<Option<AnyView<State>>>()
        {
            return erased.take().expect("slot is Some");
        }
        Self {
            inner: Box::new(slot.expect("slot is Some")),
        }
    }
}

/// Erase `view` into an [`AnyView`], the free-function spelling of [`AnyView::new`].
///
/// Idempotent, so double erasure is safe for swap detection in containers like
/// `inflexible(any(x))`. Wrapping an argument in `any(..)` is redundant since the
/// generic builders erase; reach for it to *store* a view, for a branch of more
/// than eight arms, or for a heterogeneous dynamic list.
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

/// A child sequence: what a container's child-list parameter accepts, so app code
/// passes heterogeneous children without writing [`any`].
///
/// A container takes `impl ViewSeq<State, M>` and calls [`ViewSeq::extend_views`]
/// once, the one place erasure happens ([`AnyView::new`], idempotent). Implemented
/// for any single [`View`], tuples of 1 to 12 sequences (nest beyond that; nested
/// tuples flatten in order), `Vec<S>`, `[S; N]`, `Option<S>` and [`views(iter)`](views).
///
/// Identity is positional: a `None` contributes nothing, so later siblings move up
/// one slot when it flips. `M` lets the single-view impl coexist with the container
/// impls (else a downstream `View for Vec` could overlap); inference resolves it.
#[diagnostic::on_unimplemented(
    message = "this is not a child sequence",
    note = "a child list is a View, a tuple of up to 12 views (nest tuples beyond 12), an array, a Vec, an Option, or views(iter) for an iterator"
)]
pub trait ViewSeq<State: 'static, M> {
    /// Push every element of this sequence, in order, onto `out`, erasing each with [`AnyView::new`].
    fn extend_views(self, out: &mut Vec<AnyView<State>>);
}

/// Inference markers for [`ViewSeq`]'s second parameter: one per impl, so the
/// single-[`View`] impl does not overlap the container impls. Never named by callers.
pub mod seq_marker {
    use core::marker::PhantomData;

    pub enum One {}

    pub struct VecOf<M>(PhantomData<fn() -> M>);

    pub struct ArrayOf<M>(PhantomData<fn() -> M>);

    pub struct OptionOf<M>(PhantomData<fn() -> M>);

    pub enum Iter {}
}

impl<State: 'static, V: View<State>> ViewSeq<State, seq_marker::One> for V {
    fn extend_views(self, out: &mut Vec<AnyView<State>>) {
        out.push(AnyView::new(self));
    }
}

impl<State: 'static, M, S: ViewSeq<State, M>> ViewSeq<State, seq_marker::VecOf<M>> for Vec<S> {
    fn extend_views(self, out: &mut Vec<AnyView<State>>) {
        out.reserve(self.len());
        for seq in self {
            seq.extend_views(out);
        }
    }
}

impl<State: 'static, M, S: ViewSeq<State, M>, const N: usize> ViewSeq<State, seq_marker::ArrayOf<M>>
    for [S; N]
{
    fn extend_views(self, out: &mut Vec<AnyView<State>>) {
        out.reserve(N);
        for seq in self {
            seq.extend_views(out);
        }
    }
}

impl<State: 'static, M, S: ViewSeq<State, M>> ViewSeq<State, seq_marker::OptionOf<M>>
    for Option<S>
{
    fn extend_views(self, out: &mut Vec<AnyView<State>>) {
        if let Some(seq) = self {
            seq.extend_views(out);
        }
    }
}

/// Implements [`ViewSeq`] per tuple arity; the tuple's marker is the tuple of its elements' markers.
macro_rules! impl_view_seq_for_tuples {
    ($( ($($S:ident $M:ident $idx:tt),+) )+) => {$(
        impl<State: 'static, $($M, $S: ViewSeq<State, $M>),+> ViewSeq<State, ($($M,)+)>
            for ($($S,)+)
        {
            fn extend_views(self, out: &mut Vec<AnyView<State>>) {
                $(self.$idx.extend_views(out);)+
            }
        }
    )+};
}

impl_view_seq_for_tuples! {
    (S0 M0 0)
    (S0 M0 0, S1 M1 1)
    (S0 M0 0, S1 M1 1, S2 M2 2)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5, S6 M6 6)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5, S6 M6 6, S7 M7 7)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5, S6 M6 6, S7 M7 7, S8 M8 8)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5, S6 M6 6, S7 M7 7, S8 M8 8,
        S9 M9 9)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5, S6 M6 6, S7 M7 7, S8 M8 8,
        S9 M9 9, S10 M10 10)
    (S0 M0 0, S1 M1 1, S2 M2 2, S3 M3 3, S4 M4 4, S5 M5 5, S6 M6 6, S7 M7 7, S8 M8 8,
        S9 M9 9, S10 M10 10, S11 M11 11)
}

/// The [`ViewSeq`] adapter for an iterator of views, built by [`views`].
pub struct Views<I>(I);

impl<State: 'static, I> ViewSeq<State, seq_marker::Iter> for Views<I>
where
    I: IntoIterator,
    I::Item: View<State>,
{
    fn extend_views(self, out: &mut Vec<AnyView<State>>) {
        out.extend(self.0.into_iter().map(AnyView::new));
    }
}

/// Wrap an iterator of views as a [`ViewSeq`] (one child per item), e.g.
/// `views(items.iter().map(|i| text(&i.name)))`. A `Vec` or array needs no wrapping.
pub fn views<State: 'static, I>(iter: I) -> Views<I>
where
    I: IntoIterator,
    I::Item: View<State>,
{
    Views(iter)
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
    fn double_erasure_is_idempotent() {
        // Criterion: `any(any(view))` returns the inner `AnyView` unchanged,
        // not a doubly-boxed wrapper. The element's TypeId matches the inner
        // view's concrete type, not `Box<dyn Widget>`.
        let torn = Rc::new(Cell::new(0));
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);

        let double_erased = any(any(ViewA {
            n: 5,
            torn: torn.clone(),
        }));
        let mut element = double_erased.build(&mut ctx);

        // The element should downcast directly to WidgetA, not to Box<dyn Widget>
        // as it would if double-erased.
        let a = (*element)
            .downcast_mut::<WidgetA>()
            .expect("double-erased any(any(ViewA)) should have WidgetA element directly");
        assert_eq!(a.n, 5);

        // Single-erasure behavior unchanged: rebuild with same type reports no swap
        let double_erased_2 = any(any(ViewA {
            n: 10,
            torn: torn.clone(),
        }));
        let flags = double_erased_2.rebuild(&double_erased, &mut element, &mut ctx);

        // Same concrete type → typed in-place rebuild, no teardown.
        assert_eq!(flags, ChangeFlags::PAINT);
        assert_eq!(torn.get(), 0, "no teardown on same-type rebuild");
        let a = (*element)
            .downcast_mut::<WidgetA>()
            .expect("still a WidgetA after rebuild");
        assert_eq!(a.n, 10);
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

    #[test]
    fn build_ctx_focus_chain_only_narrows_and_restores() {
        let mut counter = 0;
        let mut ctx = BuildCtx::new(&mut counter);
        assert!(
            ctx.has_focus(),
            "an unseeded context assumes a live chain (see BuildCtx::new)"
        );

        // A focused link keeps a live chain live...
        ctx.with_focus_link(true, |ctx| assert!(ctx.has_focus()));
        // ...and the descent is scoped: the caller's chain comes back.
        assert!(ctx.has_focus());

        // A cleared link closes the chain for the whole subtree below it —
        // including a *focused* link nested under the cleared one, which is the
        // stale-flag-below-a-blurred-ancestor case in one line.
        ctx.with_focus_link(false, |ctx| {
            assert!(!ctx.has_focus());
            ctx.with_focus_link(true, |ctx| {
                assert!(
                    !ctx.has_focus(),
                    "no descent may widen the chain a cleared ancestor closed"
                );
            });
            assert!(!ctx.has_focus());
        });
        assert!(
            ctx.has_focus(),
            "the outer chain is restored, not clobbered"
        );

        // Seeding is the one absolute write (the root / a component boundary).
        ctx.set_has_focus(false);
        assert!(!ctx.has_focus());
        ctx.with_focus_link(true, |ctx| assert!(!ctx.has_focus()));
    }

    #[test]
    fn build_ctx_element_replaced_is_consumed_by_the_nearest_scope() {
        let mut counter = 0;
        let mut ctx = BuildCtx::new(&mut counter);

        // A scope with nothing raised inside reports nothing.
        let ((), replaced) = ctx.track_element_replaced(|_| {});
        assert!(!replaced);

        // A signal raised inside is reported, and the result passes through.
        let (n, replaced) = ctx.track_element_replaced(|ctx| {
            ctx.mark_element_replaced();
            7
        });
        assert_eq!(n, 7);
        assert!(replaced);

        // The nearest scope consumes it: an inner scope (a pod inside the
        // subtree) takes its own signal, and the outer scope sees none.
        let (inner, outer) = ctx.track_element_replaced(|ctx| {
            let ((), inner) = ctx.track_element_replaced(|ctx| ctx.mark_element_replaced());
            inner
        });
        assert!(inner, "the inner scope consumes its own signal");
        assert!(!outer, "a consumed signal does not reach the outer scope");

        // A pending signal raised outside any scope is neither consumed nor lost
        // by a sibling's scope: it is restored when that scope returns.
        ctx.mark_element_replaced();
        let ((), sibling) = ctx.track_element_replaced(|_| {});
        assert!(!sibling, "a scope sees only signals raised inside it");
        let ((), outer) = ctx.track_element_replaced(|ctx| {
            ctx.mark_element_replaced();
        });
        assert!(outer);
        // The originally pending value came back after both scopes.
        let ((), _) = ctx.track_element_replaced(|_| {});
        assert!(
            std::mem::take(&mut ctx.element_replaced),
            "the caller's pending signal is restored, not clobbered"
        );
    }

    // --- ViewSeq: child sequences erase once, in order. ---

    fn seq<M>(children: impl ViewSeq<(), M>) -> Vec<AnyView<()>> {
        let mut out = Vec::new();
        children.extend_views(&mut out);
        out
    }

    fn a(n: u32) -> ViewA {
        ViewA {
            n,
            torn: Rc::new(Cell::new(0)),
        }
    }

    /// Build each erased child and report it: `WidgetA`'s `n`, or 99 for a
    /// `WidgetB`. The element is downcast *directly*, so a doubly-boxed child
    /// (whose element would be a `Box<dyn Widget>`) fails the expect.
    fn tags(children: &[AnyView<()>]) -> Vec<u32> {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        children
            .iter()
            .map(|child| {
                let mut element = child.build(&mut ctx);
                if let Some(a) = (*element).downcast_mut::<WidgetA>() {
                    a.n
                } else {
                    (*element)
                        .downcast_mut::<WidgetB>()
                        .expect("element is the concrete widget, not a re-boxed one");
                    99
                }
            })
            .collect()
    }

    #[test]
    fn view_seq_single_view_is_one_element() {
        assert_eq!(tags(&seq(a(4))), vec![4]);
    }

    #[test]
    fn view_seq_tuple_of_mixed_views_keeps_order() {
        let children = seq((a(1), ViewB, a(3)));
        assert_eq!(children.len(), 3);
        assert_eq!(tags(&children), vec![1, 99, 3]);
    }

    #[test]
    fn view_seq_nested_tuples_flatten_in_order() {
        let children = seq(((a(1), (ViewB, a(2))), a(3), (a(4),)));
        assert_eq!(tags(&children), vec![1, 99, 2, 3, 4]);
    }

    #[test]
    fn view_seq_vec_array_and_option_counts() {
        assert_eq!(tags(&seq(vec![a(1), a(2)])), vec![1, 2]);
        assert!(seq(Vec::<ViewB>::new()).is_empty());
        assert_eq!(tags(&seq([a(5), a(6), a(7)])), vec![5, 6, 7]);
        assert_eq!(tags(&seq(Some(a(8)))), vec![8]);
        assert!(seq(None::<ViewA>).is_empty());
        // `None` contributes nothing, so a later sibling moves up a slot.
        assert_eq!(tags(&seq((None::<ViewA>, ViewB))), vec![99]);
        // Container elements may themselves be sequences.
        assert_eq!(
            tags(&seq(vec![(a(1), ViewB), (a(2), ViewB)])),
            vec![1, 99, 2, 99]
        );
    }

    #[test]
    fn view_seq_does_not_rebox_an_any_view_element() {
        // Erasure is idempotent: an element already an `AnyView` is moved into
        // the child vector as-is, so its element is still the concrete widget.
        let children = seq((any(a(1)), any(any(ViewB)), vec![any(a(2))]));
        assert_eq!(tags(&children), vec![1, 99, 2]);
    }

    #[test]
    fn view_seq_views_adapter_maps_an_iterator() {
        let children = seq(views((1..=3).map(a)));
        assert_eq!(tags(&children), vec![1, 2, 3]);
        // The adapter is itself a sequence element.
        let children = seq((ViewB, views([4, 5].into_iter().map(a))));
        assert_eq!(tags(&children), vec![99, 4, 5]);
    }

    #[test]
    fn view_seq_twelve_tuple_compiles_and_nesting_goes_beyond() {
        let twelve = (
            a(1),
            a(2),
            a(3),
            a(4),
            a(5),
            a(6),
            a(7),
            a(8),
            a(9),
            a(10),
            a(11),
            a(12),
        );
        assert_eq!(tags(&seq(twelve)), (1..=12).collect::<Vec<_>>());
        // Past 12, nest: the order is the flattened order.
        let thirteen = (
            (
                a(1),
                a(2),
                a(3),
                a(4),
                a(5),
                a(6),
                a(7),
                a(8),
                a(9),
                a(10),
                a(11),
                a(12),
            ),
            a(13),
        );
        assert_eq!(tags(&seq(thirteen)), (1..=13).collect::<Vec<_>>());
    }
}
