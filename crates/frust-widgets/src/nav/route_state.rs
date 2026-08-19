//! Route-state observable: a signal-free snapshot of the navigator's page
//! stack as route identities, published by
//! [`NavigatorWidget::publish_state`](super::navigator::NavigatorWidget) after
//! every *committed* stack mutation.
//!
//! # R-B1
//!
//! A page's route identity is *given at push time*
//! ([`PushOptions::route`](super::navigator::PushOptions::route) /
//! [`NavigatorView::root_route`](super::navigator::NavigatorView::root_route))
//! and *published by the navigator* — nothing derives it from depth, and no
//! consumer keeps a second copy of the stack.
//!
//! # `None` entries
//!
//! A page pushed as a bare builder (an overlay/dialog — no
//! [`PushOptions::route`](super::navigator::PushOptions::route) call)
//! publishes `None`: [`RouteStack::current`] reports it (the raw top), but
//! [`RouteStack::current_route`] skips it — the seam chrome reads so a
//! transparent overlay never retitles the bar.
//!
//! # Direction is derived, never recorded
//!
//! [`NavChange`] is computed by diffing the freshly computed entries against
//! the last-published set, not stamped at each mutation site: a stack is
//! *state*, and `publish_state` sees it after every mutation (including one a
//! per-site recorder would miss, like a `request_back` pop or an
//! interactive-swipe pop finalized well after the steal), so a missed call
//! site is inexpressible. The derivation:
//!
//! ```text
//! prev empty                             -> Initial
//! len+1 & prev is a prefix of next       -> Push
//! len-1 & next is a prefix of prev       -> Pop
//! same len, only the top entry differs   -> Replace
//! anything else                          -> Reset
//! ```
//!
//! # Staleness contract
//!
//! - **Authoritative-at-publish.** After `apply_ops` the published
//!   [`RouteStack`] *is* the stack — the guarantee
//!   `NavigatorController::depth` explicitly disclaims as merely advisory.
//! - **A queued-but-undrained op is not in it.** The op queue is *intent*
//!   (`RouteNavigator::location`, the router's own request queue); this
//!   observable is *fact*. Both stay — the split is deliberate.
//! - **One bounded staleness window: an in-flight interactive edge swipe —
//!   and the two halves diverge, not agree.** `begin_interactive_pop` pops
//!   the page out of `self.pages` immediately, at steal, before the drag
//!   paints a single frame. `NavigatorView::rebuild`'s trailing
//!   `publish_state()` call is UNCONDITIONAL, so every drag frame
//!   republishes `depth()`/`can_pop()`/`back_interest()` against that
//!   already-popped count — EAGER, ahead of the commit. This observable's
//!   own publish (`publish_route_stack`, called from inside `publish_state`)
//!   is gated on `self.transition` being `Some` and `interactive` and
//!   returns early while that holds, so it keeps reporting the *pre-swipe*
//!   stack — CONSERVATIVE, behind the commit — until the settle-frame
//!   publish (after `finalize_transition` clears the flag). Depth leads, the
//!   stack lags, for the whole drag — unbounded in wall time because a
//!   finger can hold. A cancelled swipe makes the lagging observable read
//!   retroactively correct rather than something to retract.
//!   **Consequence**: at depth 2, a held swipe already publishes
//!   `compute_back_interest(1, Pop) == false` — a back press read mid-drag
//!   claims no interest and escapes to the platform (activity finish on
//!   Android), even though releasing below the commit point restores the
//!   page. Frame-accurate drag chrome already has
//!   `NavigatorController::transition()` (`is_pop`/`progress`/`interactive`)
//!   for exactly this window — this observable is not it.

use super::path::Location;

/// How the page stack changed between the last two published [`RouteStack`]s.
/// DERIVED from the diff of consecutive published stacks — never recorded at
/// a mutation site (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavChange {
    /// The first publish this navigator ever made (no previous stack to diff
    /// against).
    #[default]
    Initial,
    /// The new stack is the previous one plus exactly one more entry on top.
    Push,
    /// The new stack is a strict prefix of the previous one (exactly one
    /// entry removed from the top).
    Pop,
    /// Same depth as the previous stack; only the top entry differs.
    Replace,
    /// Anything else — e.g. a router `go()` that swaps the whole chain in one
    /// batch, or a multi-page pop that isn't a strict-prefix shrink.
    Reset,
}

/// The page stack as route identities, bottom→top. `None` at an index means
/// that page was pushed as a bare builder (an overlay/dialog): it carries no
/// location, and it must never retitle route-derived chrome — see the [module
/// docs](self).
///
/// Cloned out of [`NavigatorController::route_stack`](super::navigator::NavigatorController::route_stack)
/// — a snapshot, not a live handle. See the [module docs](self)' staleness
/// contract for what "authoritative-at-publish" does and does not promise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteStack {
    entries: Vec<Option<Location>>,
    generation: u64,
    change: NavChange,
}

impl RouteStack {
    /// Every page's route identity, bottom→top. `None` at an index is a
    /// route-less (overlay/dialog) page — see the [module docs](self).
    pub fn entries(&self) -> &[Option<Location>] {
        &self.entries
    }

    /// The stack depth (page count) as of this publish.
    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    /// The topmost page's route identity, `None` if it has none (an
    /// overlay/dialog on top). Chrome that needs to skip a route-less top
    /// wants [`current_route`](Self::current_route) instead.
    pub fn current(&self) -> Option<&Location> {
        self.entries.last().and_then(Option::as_ref)
    }

    /// The topmost page that **has** a route, skipping any route-less
    /// (overlay/dialog) pages above it — the read chrome (an app bar's title,
    /// e.g.) wants, so a transparent overlay never retitles it. `None` only
    /// if no page in the stack carries a route at all.
    pub fn current_route(&self) -> Option<&Location> {
        self.entries.iter().rev().find_map(Option::as_ref)
    }

    /// A monotonically increasing counter, bumped once per actual publish (an
    /// unchanged stack across N rebuilds bumps it zero times) — an O(1) change
    /// gate for a consumer that only wants to know "did anything happen since
    /// I last looked?" without comparing entries itself.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A diff label for **this** generation — not a running direction; see
    /// [`NavChange`].
    pub fn change(&self) -> NavChange {
        self.change
    }

    /// Overwrite the stack with `entries`, bump the generation, and derive
    /// [`change`](Self::change) from the diff against the previous entries.
    ///
    /// Always applies — the caller
    /// ([`NavigatorWidget::publish_route_stack`](super::navigator::NavigatorWidget))
    /// is responsible for skipping this call entirely when `entries` would be
    /// unchanged, which is what keeps an untouched stack's publish
    /// allocation-free (see the [module docs](self)' derivation note): the
    /// comparison against the live stack happens directly against
    /// `entries()`, before any `Vec` is built.
    pub(super) fn set(&mut self, entries: Vec<Option<Location>>) {
        self.change = derive_nav_change(&self.entries, &entries);
        self.entries = entries;
        self.generation += 1;
    }
}

/// The pure diff derivation the [module docs](self) table describes. Free
/// (not a method) and `pub(super)` so it is directly unit-testable, mirroring
/// `compute_back_interest`'s (`super::navigator`) shape.
pub(super) fn derive_nav_change(prev: &[Option<Location>], next: &[Option<Location>]) -> NavChange {
    if prev.is_empty() {
        return NavChange::Initial;
    }
    if next.len() == prev.len() + 1 && next[..prev.len()] == *prev {
        return NavChange::Push;
    }
    if prev.len() == next.len() + 1 && prev[..next.len()] == *next {
        return NavChange::Pop;
    }
    if prev.len() == next.len() && prev[..prev.len() - 1] == next[..next.len() - 1] {
        return NavChange::Replace;
    }
    NavChange::Reset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(path: &str) -> Option<Location> {
        Some(Location::parse(path))
    }

    #[test]
    fn empty_prev_is_always_initial() {
        assert_eq!(derive_nav_change(&[], &[loc("/a")]), NavChange::Initial);
        assert_eq!(derive_nav_change(&[], &[]), NavChange::Initial);
    }

    #[test]
    fn an_exact_one_entry_extension_is_push() {
        let prev = [loc("/a")];
        let next = [loc("/a"), loc("/b")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Push);
    }

    #[test]
    fn an_exact_one_entry_prefix_shrink_is_pop() {
        let prev = [loc("/a"), loc("/b")];
        let next = [loc("/a")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Pop);
    }

    #[test]
    fn same_len_top_only_differs_is_replace() {
        let prev = [loc("/a"), loc("/b")];
        let next = [loc("/a"), loc("/c")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Replace);
    }

    #[test]
    fn same_len_single_entry_differs_is_replace() {
        // depth 1 -> depth 1, the empty-prefix edge case.
        let prev = [loc("/a")];
        let next = [loc("/b")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Replace);
    }

    #[test]
    fn a_whole_chain_swap_is_reset() {
        // A router `go()` that replaces the root AND pushes more in one
        // batch: neither a strict extension, shrink, nor top-only swap.
        let prev = [loc("/a")];
        let next = [loc("/b"), loc("/c")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Reset);
    }

    #[test]
    fn a_multi_page_pop_is_reset_not_pop() {
        let prev = [loc("/a"), loc("/b"), loc("/c")];
        let next = [loc("/a")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Reset);
    }

    #[test]
    fn none_entries_participate_in_the_diff_like_any_other() {
        let prev = [loc("/a"), None];
        let next = [loc("/a"), None, loc("/c")];
        assert_eq!(derive_nav_change(&prev, &next), NavChange::Push);
    }

    #[test]
    fn current_reports_the_raw_top_including_none() {
        let mut stack = RouteStack::default();
        stack.set(vec![loc("/a"), None]);
        assert_eq!(stack.current(), None);
        assert_eq!(stack.current_route().map(|l| l.path.as_str()), Some("/a"));
    }

    #[test]
    fn current_route_skips_every_routeless_page_on_top() {
        let mut stack = RouteStack::default();
        stack.set(vec![None, None]);
        assert_eq!(
            stack.current_route(),
            None,
            "no page in the stack carries a route"
        );
    }

    #[test]
    fn set_bumps_generation_and_replaces_entries() {
        let mut stack = RouteStack::default();
        assert_eq!(stack.generation(), 0);
        assert_eq!(stack.change(), NavChange::Initial);

        stack.set(vec![loc("/a")]);
        assert_eq!(stack.generation(), 1);
        assert_eq!(stack.change(), NavChange::Initial);
        assert_eq!(stack.depth(), 1);

        stack.set(vec![loc("/a"), loc("/b")]);
        assert_eq!(stack.generation(), 2);
        assert_eq!(stack.change(), NavChange::Push);
        assert_eq!(stack.depth(), 2);
    }
}
