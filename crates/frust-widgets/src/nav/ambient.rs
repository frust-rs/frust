//! The navigator's two thread-local ambient scopes: the per-page
//! **back-reach** cell a nested navigator learns its hosting page's input
//! reachability through (rule **R23**), and the per-navigator **swipe-claim**
//! cell an inner navigator reports an edge-swipe arm into (**R-B3-inner**).
//!
//! Both seams and their composition rules are documented in
//! [`navigator`](super::navigator)'s module docs; this file holds only the
//! thread-locals, their drop guards, and the scope/read helpers.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

// --- Back reach: the ambient "is the hosting page input-routed?" seam --------

thread_local! {
    /// The stack of per-page **back-reach** cells for the page builders currently
    /// on the call stack — the ambient seam a *nested* navigator learns its
    /// hosting page's input reachability through (rule **R23**: navigator reach
    /// follows input routing, exactly).
    ///
    /// A navigator pushes the reach cell of the page whose builder / reconcile it
    /// is about to run ([`with_page_reach`]) and pops it again afterwards, so any
    /// navigator built anywhere inside that page's subtree — at any depth, through
    /// any container — reads it with [`ambient_page_reach`]. A stack rather than a
    /// single slot because navigators nest: each level restores its parent's cell.
    ///
    /// Reactive-free by construction (`frust-widgets` carries no `reactive_graph`
    /// dependency): a plain `Rc<Cell<bool>>`, the same idiom as `depth`/
    /// `back_interest`/`transition`, never a signal. UI-thread-affine for the same
    /// reason those are — the cells are `Rc`-backed and only ever touched from a
    /// build pass.
    static PAGE_REACH: RefCell<Vec<Rc<Cell<bool>>>> = const { RefCell::new(Vec::new()) };
}

/// Pops [`PAGE_REACH`] on drop so an unwinding page builder cannot leave a stale
/// scope behind for the rest of the thread's life.
struct PageReachGuard;

impl Drop for PageReachGuard {
    fn drop(&mut self) {
        PAGE_REACH.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Run `f` with `reach` installed as the ambient page-reach cell — i.e. declare
/// "everything built in here lives on the page this cell describes".
///
/// The [`PAGE_REACH`] borrow is released *before* `f` runs, so `f` may nest
/// another `with_page_reach` (a navigator inside a page inside a navigator) or
/// call [`ambient_page_reach`] freely.
pub(super) fn with_page_reach<R>(reach: &Rc<Cell<bool>>, f: impl FnOnce() -> R) -> R {
    PAGE_REACH.with(|stack| stack.borrow_mut().push(Rc::clone(reach)));
    let _guard = PageReachGuard;
    f()
}

/// The reach cell of the page currently being built/reconciled, or `None` at the
/// top level (a root navigator, whose reach is unconditional).
pub(super) fn ambient_page_reach() -> Option<Rc<Cell<bool>>> {
    PAGE_REACH.with(|stack| stack.borrow().last().cloned())
}

// --- R-B3-inner: the ambient "did anything below me arm an edge-swipe on this
// Down?" seam ------------------------------------------------------------

thread_local! {
    /// The stack of per-navigator **swipe-claim** cells for the navigators
    /// currently routing a left-edge `Down` (`R-B3-inner`, mirrors
    /// [`PAGE_REACH`]/[`with_page_reach`]).
    ///
    /// A left-edge `Down` does not capture: the outer navigator only records
    /// `edge.armed` and forwards it through [`route_top`](NavigatorWidget::route_top)
    /// (children see `Down` first), so a nested navigator's own `event_at` runs
    /// underneath — and may *also* arm. Preemption is not decided at `Down`; it
    /// is decided later, at the decisive `Move` steal site, where the OUTER
    /// navigator — upstream in parent→child routing — reaches its steal branch
    /// before the inner ever sees the event. So a navigator pushes its own
    /// claim cell before forwarding `Down` ([`with_swipe_claim`]), reads it back
    /// once forwarding returns (`edge.inner_claimed`), and propagates the
    /// combined result into whatever cell is now ambient
    /// ([`ambient_swipe_claim`]) — its own host navigator's, if any — so a
    /// third nesting level defers too.
    ///
    /// A stack (not a single slot) for the same reason `PAGE_REACH` is one:
    /// navigators nest, and each level must see its own immediate host's cell,
    /// not some ancestor's.
    static SWIPE_CLAIM: RefCell<Vec<Rc<Cell<bool>>>> = const { RefCell::new(Vec::new()) };
}

/// Pops [`SWIPE_CLAIM`] on drop, mirroring [`PageReachGuard`].
struct SwipeClaimGuard;

impl Drop for SwipeClaimGuard {
    fn drop(&mut self) {
        SWIPE_CLAIM.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Run `f` (a `Down` forward through [`route_top`](NavigatorWidget::route_top))
/// with `claim` installed as the ambient swipe-claim cell, so any navigator
/// reached underneath can report "I armed" into it via [`ambient_swipe_claim`].
///
/// The borrow is released before `f` runs, so `f` may itself nest another
/// `with_swipe_claim` call (a third level of navigator nesting).
pub(super) fn with_swipe_claim<R>(claim: &Rc<Cell<bool>>, f: impl FnOnce() -> R) -> R {
    SWIPE_CLAIM.with(|stack| stack.borrow_mut().push(Rc::clone(claim)));
    let _guard = SwipeClaimGuard;
    f()
}

/// The swipe-claim cell of the navigator currently forwarding a `Down` through
/// [`route_top`](NavigatorWidget::route_top), or `None` if no navigator is (a
/// top-level navigator's `Down` handling, or any non-`Down` event).
pub(super) fn ambient_swipe_claim() -> Option<Rc<Cell<bool>>> {
    SWIPE_CLAIM.with(|stack| stack.borrow().last().cloned())
}

/// Whether a navigator hosted under `host_reach` is itself reachable: `true` at
/// the top level (no hosting page), otherwise whatever the hosting page's cell
/// currently says.
pub(super) fn host_reachable(host_reach: Option<&Rc<Cell<bool>>>) -> bool {
    host_reach.is_none_or(|cell| cell.get())
}
