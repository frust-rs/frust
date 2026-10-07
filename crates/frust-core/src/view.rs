//! Layer 1: the declarative [`View`] trait.
//!
//! Views are cheap, short-lived descriptors produced by a root component's `Component::build`
//! closure, a pure function of application state (`fn build(&mut State) -> impl View<State>`).
//! They are *not* the retained tree — re-running the build closure on every state
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
/// Two responsibilities: allocating unique [`WidgetId`]s (it borrows the id
/// counter owned by the render root so ids stay monotonic across passes), and
/// carrying the **effective focus chain** — whether every link from the root down
/// to the node being built/rebuilt/torn down is focused (see
/// [`BuildCtx::has_focus`]).
pub struct BuildCtx<'a> {
    next_id: &'a mut u64,
    /// The effective focus chain: `true` only while every recorded focus link
    /// from the root down to the current node is set. See
    /// [`BuildCtx::has_focus`].
    has_focus: bool,
}

impl<'a> BuildCtx<'a> {
    /// Create a context borrowing the render root's id counter.
    ///
    /// The focus chain seeds **`true`** — "unknown, assume live". A caller that
    /// does not thread the chain therefore keeps the pre-chain behavior (every
    /// reconciler under it treats a `focused` pod as the live one) instead of
    /// silently suppressing a release it owed: over-releasing costs the user one
    /// tap, while under-releasing strands the shell's IME surface over a widget
    /// that no longer exists and nothing on an idle screen ever corrects it (the
    /// same default-to-must-run rule the frame gate follows). The seams that
    /// *know* the real value — [`RenderRoot::rebuild`](crate::app::RenderRoot)'s
    /// view diff and [`ComponentWidget`](crate::component::ComponentWidget)'s
    /// inner context — set it explicitly with [`BuildCtx::set_has_focus`].
    pub fn new(next_id: &'a mut u64) -> Self {
        Self {
            next_id,
            has_focus: true,
        }
    }

    /// Allocate a fresh, unique widget id.
    pub fn alloc_id(&mut self) -> WidgetId {
        *self.next_id += 1;
        WidgetId(*self.next_id)
    }

    /// Whether the recorded focus path is live all the way from the root to the
    /// node currently being built/rebuilt/torn down — the rebuild-pass mirror of
    /// [`PaintCtx::has_focus`](crate::widget::PaintCtx::has_focus), composed the
    /// same way (`self.focused && ctx.has_focus()`).
    ///
    /// This is what makes a `focused` [`ChildPod`](crate::widget::ChildPod) flag
    /// *deep inside a blurred branch* harmless. A container-routed blur clears the
    /// focus link at the nearest common ancestor only, so flags below it
    /// legitimately go stale until focus next enters that subtree; a pod under a
    /// cleared link sees `has_focus() == false` here, exactly as it sees `false`
    /// in paint and exactly as focus-routed events never reach it. A reconciler
    /// therefore raises [`mark_focus_orphaned`](crate::event::mark_focus_orphaned)
    /// only when `ctx.has_focus() && pod.is_focused()` — only when the pod losing
    /// its identity is the one whose session is actually live.
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Seed the effective focus chain (see [`BuildCtx::has_focus`]).
    ///
    /// For the two kinds of seam that *start* a chain rather than descend one:
    /// [`RenderRoot`](crate::app::RenderRoot)'s view diff, which seeds it from the
    /// root's own session mirror, and a component-style widget that builds an
    /// inner `BuildCtx` over its own id counter
    /// ([`ComponentWidget`](crate::component::ComponentWidget) is the in-crate
    /// one) and must carry its outer chain across that boundary. A container
    /// descending into a child pod uses [`BuildCtx::with_focus_link`] instead — it
    /// can only narrow, which is what keeps this an AND-chain.
    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    /// Run `f` with the focus chain extended by one link, restoring the caller's
    /// chain when it returns.
    ///
    /// `link_focused` is the descended-into pod's own
    /// [`ChildPod::is_focused`](crate::widget::ChildPod::is_focused) flag, so the
    /// closure sees `self.has_focus() && link_focused`: a cleared link anywhere
    /// above forces `false` for the whole subtree below it, and no descent can
    /// ever widen the chain. The rebuild-pass counterpart of
    /// [`ChildPod::paint_child`](crate::widget::ChildPod::paint_child)'s
    /// `set_has_focus(self.focused && ctx.has_focus())`, in the scoped-closure
    /// shape [`SemanticsCtx::descend_into_pod`](crate::semantics::SemanticsCtx)
    /// already uses for the semantics pass.
    pub fn with_focus_link<R>(&mut self, link_focused: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.has_focus;
        self.has_focus = outer && link_focused;
        let result = f(self);
        self.has_focus = outer;
        result
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
    /// stable as event routing is added later.
    fn message(&self, _element: &mut Self::Element, _state: &mut State) {}
}

/// Object-safe mirror of [`View`], used only as the erased backing of
/// [`AnyView`].
///
/// The methods mirror `build`/`rebuild`/`teardown` but drop the associated
/// `Element` type in favour of a `Box<dyn Widget>`, and add [`ErasedView::as_any`]
/// so a rebuild can downcast the *previous* erased view to detect a
/// concrete-type change (the xilem `AnyView` trick).
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
    ///
    /// Erasure is idempotent: if `view` is already an `AnyView<State>`, it is
    /// returned unchanged — `any(any(v))` and `AnyView::new(any_view)` do not box
    /// twice.
    pub fn new<V: View<State>>(view: V) -> Self {
        // Slot holds the value so it can be moved out through `&mut dyn Any`'s borrow.
        // `take()` extracts it after the downcast_mut succeeds, or the original is boxed.
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

/// Erase `view` into an [`AnyView`] — the free-function spelling of
/// [`AnyView::new`], mirroring the `text(..)`/`button(..)` view-fn vocabulary.
///
/// Erasure is idempotent: `any(any(v))` returns the inner `AnyView` unchanged,
/// not a doubly-boxed wrapper. This makes double erasure safe for swap detection
/// in containers like `inflexible(any(x))`.
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

/// A child sequence: what a container's child-list parameter accepts, so app
/// code passes heterogeneous children without writing [`any`] at the call site.
///
/// A container takes `children: impl ViewSeq<State, M>` (generic over the
/// inference marker `M`, which callers never name) and calls
/// [`ViewSeq::extend_views`] once to push every element into the erased child
/// vector it already stores (`Vec<AnyView<State>>`). That push is the **one place
/// erasure happens**: each element goes through [`AnyView::new`], which is
/// idempotent, so an element that is already an [`AnyView`] is moved in as-is —
/// never boxed twice — and no intermediate collection is built.
///
/// Implemented for:
///
/// - any single [`View`] (one element; [`AnyView`] included),
/// - tuples `(S1, .., Sn)` for `n = 1..=12`, where every element is itself a
///   `ViewSeq`, so elements of *different* concrete types mix freely and tuples
///   nest — a nested tuple flattens in order,
/// - `Vec<S>` and `[S; N]` (homogeneous; the element is any `ViewSeq`),
/// - `Option<S>` (`None` contributes no element — see below),
/// - [`views(iter)`](views) for any iterator of views (there is deliberately no
///   blanket impl over `IntoIterator`: it would overlap the tuple impls).
///
/// **Identity is positional.** The reconciler matches children by position (plus
/// the concrete element type), so a tuple's elements keep their identity only
/// while their positions do. An `Option` that is `None` contributes nothing, so
/// every *later* sibling moves up one slot when it flips — exactly like a
/// conditional child added through a builder's `.when_some(..)`/`.when(..)`.
/// Put optional children last, or use the container's keyed-child API (still a
/// `Vec`-based affair, not a tuple one) when sibling identity must survive.
///
/// **Arity.** A single tuple holds at most 12 elements; beyond that, nest tuples
/// (`((a, b, .., l), (m, n))`), which flattens to the same order.
///
/// **The marker parameter.** `M` exists only so the single-view impl
/// (`impl<V: View<State>> ViewSeq<State, _> for V`) can coexist with the
/// container impls — without it the compiler must assume some downstream crate
/// could implement `View` for a `Vec` or a tuple and rejects the overlap. Each
/// impl picks a distinct marker from [`seq_marker`], so inference always resolves
/// it; a function accepting a sequence just declares a generic `M`.
///
/// ```
/// use frust_core::{AnyView, ViewSeq};
///
/// // A container-style API: generic over the marker, erasing into one Vec.
/// fn children<M>(seq: impl ViewSeq<(), M>) -> Vec<AnyView<()>> {
///     let mut out = Vec::new();
///     seq.extend_views(&mut out);
///     out
/// }
///
/// let none = || None::<AnyView<()>>;
/// let empty = Vec::<AnyView<()>>::new;
/// // Twelve elements is the widest flat tuple.
/// let twelve = (
///     none(), none(), none(), none(), none(), none(),
///     none(), none(), none(), none(), none(), empty(),
/// );
/// assert!(children(twelve).is_empty());
/// ```
///
/// A thirteenth flat element is not a sequence — nest instead:
///
/// ```compile_fail,E0277
/// use frust_core::{AnyView, ViewSeq};
///
/// fn children<M>(seq: impl ViewSeq<(), M>) -> Vec<AnyView<()>> {
///     let mut out = Vec::new();
///     seq.extend_views(&mut out);
///     out
/// }
///
/// let none = || None::<AnyView<()>>;
/// let thirteen = (
///     none(), none(), none(), none(), none(), none(), none(),
///     none(), none(), none(), none(), none(), none(),
/// );
/// let _ = children(thirteen);
/// ```
#[diagnostic::on_unimplemented(
    message = "this is not a child sequence",
    note = "a child list is a View, a tuple of up to 12 views (nest tuples beyond 12), an array, a Vec, an Option, or views(iter) for an iterator"
)]
pub trait ViewSeq<State: 'static, M> {
    /// Push every element of this sequence, in order, onto `out` — erasing each
    /// with [`AnyView::new`] (a no-op for an element already an [`AnyView`]).
    fn extend_views(self, out: &mut Vec<AnyView<State>>);
}

/// Inference markers for [`ViewSeq`]'s second parameter.
///
/// One marker per impl keeps the single-[`View`] impl from overlapping the
/// container impls. They are never constructed and callers never name them: an
/// API accepting a child sequence declares a generic `M` and lets inference pick.
pub mod seq_marker {
    use core::marker::PhantomData;

    /// Marks the impl for a single [`View`](super::View).
    pub enum One {}

    /// Marks the impl for `Vec<S>`, where `S`'s own marker is `M`.
    pub struct VecOf<M>(PhantomData<fn() -> M>);

    /// Marks the impl for `[S; N]`, where `S`'s own marker is `M`.
    pub struct ArrayOf<M>(PhantomData<fn() -> M>);

    /// Marks the impl for `Option<S>`, where `S`'s own marker is `M`.
    pub struct OptionOf<M>(PhantomData<fn() -> M>);

    /// Marks the impl for [`Views`](super::Views), the [`views`](super::views)
    /// iterator adapter.
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

/// Implements [`ViewSeq`] for one tuple arity per `(Element Marker index, ..)`
/// group; the tuple's marker is the tuple of its elements' markers.
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

/// Wrap an iterator of views as a [`ViewSeq`] — the child-sequence spelling of
/// "one child per item", e.g. `views(items.iter().map(|i| text(&i.name)))`.
///
/// `ViewSeq` has no blanket impl over `IntoIterator` (it would overlap the tuple
/// impls), so an iterator goes through this adapter; a `Vec` or array needs no
/// wrapping. Each item is erased once, as it is pushed.
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
