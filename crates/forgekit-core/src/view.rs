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

#[cfg(test)]
mod tests {
    use super::*;

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
