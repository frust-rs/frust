//! [`NavigatorController`]: the cloneable app-state handle that *records*
//! navigation ops for a [`NavigatorWidget`]
//! to drain at its next rebuild, plus the published depth / back-interest /
//! transition / route-stack read seams the facade and app code observe it
//! through.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::AnyView;

use super::ambient::host_reachable;
use super::options::{
    BackPolicy, NavOp, NavigatorId, PopResult, PushOptions, ReplaceOptions, ResultCallback,
};
use super::route_state::RouteStack;
use super::transition::{TransitionSpec, TransitionState};

// Named only by the doc comments moved here with this module's items, so their
// intra-doc links keep resolving to the same targets they did in `navigator`.
#[allow(unused_imports)]
use super::navigator::NavigatorWidget;
#[allow(unused_imports)]
use super::transition::PageTransition;
#[allow(unused_imports)]
use super::view::navigator;
#[allow(unused_imports)]
use frust_core::Widget;

/// The app-state handle to a [`navigator`]: a cloneable op queue an app keeps in
/// its `Component::State` and drives with [`push`](Self::push)/[`pop`](Self::pop)/
/// [`replace`](Self::replace). Every clone shares one queue (`Rc`), so the handle
/// the view carries and the handle event handlers call are the same.
///
/// Ops are *recorded*, not applied — the [`NavigatorWidget`] drains and applies
/// them at its next rebuild (see the [module docs](self)).
pub struct NavigatorController<State: 'static> {
    pub(super) ops: Rc<RefCell<Vec<NavOp<State>>>>,
    /// The current page-stack **depth**, published by the attached
    /// [`NavigatorWidget`] on every `build`/`rebuild`/`apply_ops`. The widget
    /// owns the authoritative stack; this shared cell is
    /// the read seam [`depth`](Self::depth)/[`can_pop`](Self::can_pop) expose so
    /// the facade's back handler (and app code) can ask "would a pop do
    /// anything?" without reaching into the widget. `0` until a widget attaches.
    ///
    /// `frust-widgets` stays reactive-free: this is a plain `Rc<Cell<_>>`,
    /// not a signal — a shell/facade polls it at rebuild time (see the timing
    /// note in `frust-reactive::back`).
    pub(super) depth: Rc<Cell<usize>>,
    /// Whether a back press should be *claimed* by the navigator ahead-of-time
    /// (predictive-back parity) — published by the attached
    /// [`NavigatorWidget`] alongside [`depth`](Self::depth). `true` iff the stack
    /// is poppable (`depth > 1`) **or** the top page's [`BackPolicy`] is not
    /// [`Pop`](BackPolicy::Pop) (a dismissable/veto overlay claims back even at
    /// the root) — **and** the page hosting this navigator is itself
    /// input-reachable (the R23 gate; see
    /// [`NavigatorWidget::reachable`], always satisfied for a top-level
    /// navigator). This is the signal the facade's back handler computes
    /// `handles_back` from — it differs from [`can_pop`](Self::can_pop) exactly
    /// in the depth-1-with-overlay case, where a raw pop would do nothing but the
    /// overlay still owns the press. Same plain `Rc<Cell<_>>` (reactive-free)
    /// polled-at-rebuild contract as `depth`.
    pub(super) back_interest: Rc<Cell<bool>>,
    /// The published snapshot of the navigator's single in-flight page
    /// transition, written by the attached [`NavigatorWidget`] at every
    /// transition edge *and* on every paint frame that advances the driver. The
    /// read seam is [`transition`](Self::transition), whose doc carries the
    /// timing contract.
    ///
    /// Same reactive-free idiom as [`depth`](Self::depth)/
    /// [`back_interest`](Self::back_interest) — a plain `Rc<Cell<_>>` of `Copy`
    /// data, never a signal. Unlike those two it is published from *paint* as
    /// well as build, which is what makes a frame-exact read possible.
    pub(super) transition: Rc<Cell<TransitionState>>,
    /// How many live [`NavigatorWidget`]s currently render this controller's
    /// stack — incremented by [`NavigatorView::build`] and (if a live widget's
    /// controller is swapped) by [`NavigatorView::rebuild`]; decremented by
    /// `teardown` and by that same swap handling for the controller being
    /// swapped *away from*. Read through [`is_mounted`](Self::is_mounted).
    ///
    /// A **liveness** seam, not a published-state one: the three cells above
    /// answer "what does the stack look like?", this one answers "is this
    /// navigator in the retained tree at all?". The facade's back arbitration
    /// needs the latter to tell a navigator that merely did not wire this pass
    /// from one whose screen was torn down (see `frust::back_glue`'s R44-back
    /// prune). A count, not a bool, so a reconcile that builds the replacement
    /// widget before tearing down the old one never reads as unmounted.
    ///
    /// **The widget never decrements this cell by looking `self.controller` up
    /// again** — [`NavigatorWidget`] holds its own clone (`mounted`, alongside
    /// `depth`/`back_interest`/`transition`), rebound only at `build` and at a
    /// controller-swap `rebuild`, and every decrement goes through that field.
    /// A `NavigatorView`'s `self.controller` is whatever the app currently
    /// hands it — after a swap that is already the *new* controller — so
    /// `teardown` reading it instead would double-unmount the new one and never
    /// correct the old one, exactly the structural gap this field closes (see
    /// [`NavigatorView::rebuild`]'s controller-swap comment).
    ///
    /// Same reactive-free `Rc<Cell<_>>` idiom as the rest of the controller.
    pub(super) mounted: Rc<Cell<usize>>,
    /// The [`PageEntry::reach`] cell of the page **hosting** this navigator,
    /// bound by the attached [`NavigatorWidget`] at `build`/`rebuild`; `None`
    /// for a top-level navigator (and until a widget attaches).
    ///
    /// The R23 back-reach gate [`back_interest`](Self::back_interest) reads
    /// *live*, rather than a value baked into the published cell: the page
    /// hosting a nested navigator can stop being input-routed on a pass in which
    /// that navigator does not rebuild at all (a page frozen by
    /// [`cull_covered_builds`](NavigatorView::cull_covered_builds), or one
    /// stashed out of the stack by a pop transition), and a baked-in value would
    /// go stale exactly there — the worst case for this gate.
    ///
    /// A `RefCell` slot around a plain `Rc<Cell<bool>>`, like `ops` — a
    /// *binding* that moves when the widget re-captures its host, wrapping the
    /// same reactive-free cell idiom as the published state.
    pub(super) host_reach: Rc<RefCell<Option<Rc<Cell<bool>>>>>,
    /// The published route-state snapshot — a fourth published slot
    /// beside `depth`/`back_interest`/`transition`, written by
    /// [`NavigatorWidget::publish_state`] after every committed stack
    /// mutation (see `route_state`'s module docs for the staleness contract
    /// and why an in-flight interactive edge swipe does not publish here).
    ///
    /// `RefCell`, not `Cell`: the payload (`RouteStack`) is not `Copy` —
    /// precedented on this same type by [`ops`](Self::ops) and
    /// [`host_reach`](Self::host_reach).
    pub(super) route_stack: Rc<RefCell<RouteStack>>,
}

impl<State: 'static> Clone for NavigatorController<State> {
    fn clone(&self) -> Self {
        Self {
            ops: Rc::clone(&self.ops),
            depth: Rc::clone(&self.depth),
            back_interest: Rc::clone(&self.back_interest),
            transition: Rc::clone(&self.transition),
            mounted: Rc::clone(&self.mounted),
            host_reach: Rc::clone(&self.host_reach),
            route_stack: Rc::clone(&self.route_stack),
        }
    }
}

impl<State: 'static> Default for NavigatorController<State> {
    fn default() -> Self {
        Self::new()
    }
}

impl<State: 'static> NavigatorController<State> {
    /// A fresh controller with an empty op queue.
    pub fn new() -> Self {
        Self {
            ops: Rc::new(RefCell::new(Vec::new())),
            depth: Rc::new(Cell::new(0)),
            back_interest: Rc::new(Cell::new(false)),
            transition: Rc::new(Cell::new(TransitionState::default())),
            mounted: Rc::new(Cell::new(0)),
            host_reach: Rc::new(RefCell::new(None)),
            route_stack: Rc::new(RefCell::new(RouteStack::default())),
        }
    }

    /// Bind (or re-bind) the hosting page's reach cell — called by the attached
    /// [`NavigatorWidget`] from `build` and every `rebuild` with whatever
    /// [`ambient_page_reach`] says at that point, so the binding follows the
    /// navigator if its subtree ever moves between pages.
    ///
    /// Deliberately not public: reach is derived by the navigator hosting this
    /// one, never declared by an app.
    pub(super) fn bind_host_reach(&self, reach: Option<Rc<Cell<bool>>>) {
        *self.host_reach.borrow_mut() = reach;
    }

    /// Whether the page hosting this navigator is input-routed right now — the
    /// **R23 back-reach gate**, read live (see
    /// [`host_reach`](Self::host_reach)). `true` for a top-level navigator and
    /// before a widget attaches.
    fn host_reachable(&self) -> bool {
        host_reachable(self.host_reach.borrow().as_ref())
    }

    /// Whether a live [`NavigatorWidget`] currently renders this controller's
    /// stack — i.e. whether this navigator is in the retained tree *right now*.
    ///
    /// `false` before the first `build` and again after the widget's `teardown`
    /// (a screen with its own nested navigator, popped). Unlike
    /// [`depth`](Self::depth)/[`back_interest`](Self::back_interest) this is not
    /// an advisory snapshot of the stack: it is exact at every point after the
    /// widget's `build` began, because `build`/`teardown` write it directly.
    ///
    /// The facade's back arbitration is the intended consumer: a registered
    /// controller that is still mounted is still part of the tree, so it must
    /// keep its place in the arbitration list even on a pass in which it did not
    /// re-wire; one that is no longer mounted can be released.
    pub fn is_mounted(&self) -> bool {
        self.mounted.get() > 0
    }

    /// Record that a [`NavigatorWidget`] attached to this controller. Called at
    /// the very top of [`NavigatorView::build`], *before* the root page builder
    /// runs: that builder may itself wire a nested navigator, and the facade's
    /// back arbitration must already see this navigator as mounted by then.
    pub(super) fn mount(&self) {
        self.mounted.set(self.mounted.get() + 1);
    }

    // No `unmount` method here deliberately: unmounting always goes through
    // the widget-owned `mounted` cell (`unmount_cell`, below), never back
    // through a `NavigatorController` reference — see the `mounted` field
    // doc's structural-pairing note and `NavigatorView::rebuild`'s
    // controller-swap handling.

    /// This controller's [`NavigatorId`] — stable across clones, distinct per
    /// independently constructed controller. See [`NavigatorId`] for the
    /// liveness caveat.
    pub fn id(&self) -> NavigatorId {
        NavigatorId(Rc::as_ptr(&self.ops) as *const u8 as usize)
    }

    /// The current page-stack depth of the navigator this controller drives, as
    /// last published by that navigator's `build`/`rebuild`, or `0` if no
    /// navigator is attached yet.
    ///
    /// **Advisory**: this reflects the depth at the last rebuild, so a query
    /// racing a same-frame stack change sees the previous value (the
    /// rebuild-time refresh contract — see [`can_pop`](Self::can_pop) and
    /// `frust-reactive::back`'s timing note).
    pub fn depth(&self) -> usize {
        self.depth.get()
    }

    /// Whether a [`pop`](Self::pop) would actually remove a page — `true` iff
    /// the navigator has more than one page ([`depth`](Self::depth)` > 1`).
    ///
    /// **Advisory**, for exactly the Android back contract: the
    /// facade's back handler reads this to decide whether a back press pops or
    /// bubbles to the platform, and publishes it as
    /// `frust-reactive::set_handles_back`. The authoritative guard stays the
    /// widget's own `len > 1` check in [`apply_ops`](NavigatorWidget) — a pop at
    /// the root remains a safe no-op even if this raced stale, so a
    /// mis-predicted root-level back never removes the last page.
    pub fn can_pop(&self) -> bool {
        self.depth.get() > 1
    }

    /// Whether the navigator claims the next back press ahead-of-time
    /// (predictive-back parity) — `true` iff the stack is poppable
    /// **or** the top page declares a non-[`Pop`](BackPolicy::Pop) policy (a
    /// dismissable/veto overlay). The facade's back handler reads this
    /// (in preference to [`can_pop`](Self::can_pop)) to compute the shell's
    /// `handles_back`, so a dismissable overlay at the root still consumes back
    /// rather than exiting the app.
    ///
    /// # Nested navigators: reach follows input routing (R23)
    ///
    /// A navigator hosted on a page its own host navigator routes **no input**
    /// to (a covered page, or the page under a transparent overlay) reports
    /// `false` here regardless of its own stack: back arbitration reaches
    /// exactly as far as input does, so a press can never pop an off-screen
    /// stack while the visible page stays put. Unconditional `true` for the
    /// reach term at the top level, so a single-navigator app is unaffected.
    ///
    /// **Advisory**, published at the last rebuild like [`depth`](Self::depth) —
    /// a query racing a same-frame stack change sees the previous value; the
    /// navigator's own [`request_back`](Self::request_back) routing stays
    /// authoritative regardless (see `frust-reactive::back`'s timing note).
    pub fn back_interest(&self) -> bool {
        // The published cell answers for this navigator's own stack; the reach
        // gate is ANDed HERE (live) rather than baked into the cell, because the
        // hosting page can stop being input-routed on a pass in which this
        // navigator never rebuilds — see `host_reach`.
        self.back_interest.get() && self.host_reachable()
    }

    /// The navigator's current [`TransitionState`] — the observation seam chrome
    /// *outside* the navigator subtree (an app bar, a tab bar, a progress
    /// indicator) drives its own motion from. [`TransitionState::default`] (an
    /// at-rest depth-0 snapshot) until a navigator attaches.
    ///
    /// Published on the **controller**, not the widget, deliberately: chrome that
    /// wants to match page motion is a *sibling* of the navigator, not a
    /// descendant, so it can never reach the widget — but it can hold a
    /// controller clone, exactly as it already does to `push`.
    ///
    /// # The timing contract
    ///
    /// The progress driver advances in exactly one place — `paint`, off
    /// `PaintCtx::frame_time` (the No-`Instant::now()` rule in
    /// `docs/REVIEW_FOCUS.md` forbids any other clock in `frust-widgets`).
    /// Therefore:
    ///
    /// - **A read during your own `Widget::paint`, from a widget painted *after*
    ///   the navigator, is exact for the current frame.** In a root
    ///   `Stack(vec![navigator_subtree, chrome])`, `StackWidget::paint` walks its
    ///   children in order, so `chrome` paints second and reads the value the
    ///   navigator wrote microseconds earlier **in the same frame**. This is the
    ///   frame-perfect path; it is the one to use for choreography.
    /// - **A read during `Component::build` (or any `View::build`/`rebuild`) is
    ///   always exactly one frame stale** for `progress`, because build precedes
    ///   paint. Fine for "is a transition running?"; wrong for choreography.
    /// - **The [`active`](TransitionState::active) edges are the exception.**
    ///   Both `start_transition` and `finalize_transition` publish from a
    ///   `BuildCtx` pass, so a build-time reader that builds *after* the navigator
    ///   sees `active` flip on the very frame it happens. Only intermediate
    ///   `progress` lags.
    ///
    /// The navigator already requests a frame for every frame a transition runs,
    /// so a paint-time observer needs no wake of its own.
    pub fn transition(&self) -> TransitionState {
        self.transition.get()
    }

    /// The navigator's currently published route-state snapshot — a
    /// clone, authoritative as of the last publish (see `route_state`'s
    /// module docs' staleness contract, in particular the interactive-swipe
    /// window).
    pub fn route_stack(&self) -> RouteStack {
        self.route_stack.borrow().clone()
    }

    /// The route stack's current generation — an O(1) change gate equivalent
    /// to `route_stack().generation()` but without cloning the whole
    /// snapshot.
    pub fn route_generation(&self) -> u64 {
        self.route_stack.borrow().generation()
    }

    /// Push an **opaque** page built by `builder` on top of the stack, using the
    /// navigator's default transition (instant unless the navigator sets one).
    pub fn push(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.push_impl(builder, true, None, None);
    }

    /// Push an **opaque** page with an explicit [`TransitionSpec`], overriding the
    /// navigator's default for this push only. The spec is stored on the pushed
    /// page and *reversed* when it is later popped.
    pub fn push_with(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        transition: TransitionSpec,
    ) {
        self.push_impl(builder, true, None, Some(transition));
    }

    /// Push a **transparent** page (e.g. a dialog/overlay) — the page below it
    /// stays visible and painted (see [module docs](self)'s paint culling).
    pub fn push_transparent(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.push_impl(builder, false, None, None);
    }

    /// Push an opaque page and register `on_result`, invoked with `&mut State`
    /// when *this* page is later popped (carrying the pop's [`PopResult`]).
    ///
    /// The callback is delivered at the start of the [`NavigatorWidget::event`]
    /// pass after the pop's rebuild — the first point after the pop where the
    /// erased app state is in scope (a rebuild carries only a `BuildCtx`).
    pub fn push_for_result(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        on_result: impl Fn(&mut State, PopResult) + 'static,
    ) {
        self.push_impl(builder, true, Some(Rc::new(on_result)), None);
    }

    /// Push a **transparent** page (e.g. a dialog/bottom sheet) with an explicit
    /// [`TransitionSpec`] (e.g. [`PageTransition::M3FadeThrough`] for a dialog,
    /// [`PageTransition::SlideUp`] for a bottom sheet — a design system's own
    /// dialog/sheet helpers are the shipped callers),
    /// and register `on_result`, invoked with `&mut State` when *this* page is
    /// later popped (carrying the pop's [`PopResult`]) — the modal-with-a-result
    /// combination [`push_transparent`](Self::push_transparent) and
    /// [`push_for_result`](Self::push_for_result) each cover only half of.
    ///
    /// ```ignore
    /// // A confirm dialog that reports whether the user confirmed:
    /// controller.push_transparent_for_result(
    ///     || dialog_view(),
    ///     TransitionSpec::duration(PageTransition::M3FadeThrough),
    ///     |state: &mut State, result: PopResult| {
    ///         state.confirmed = result.take::<bool>().unwrap_or(false);
    ///     },
    /// );
    /// ```
    ///
    /// The callback is delivered the same way [`push_for_result`](Self::push_for_result)'s
    /// is: at the start of the [`NavigatorWidget::event`] pass after the pop's
    /// rebuild.
    pub fn push_transparent_for_result(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        transition: TransitionSpec,
        on_result: impl Fn(&mut State, PopResult) + 'static,
    ) {
        self.push_impl(builder, false, Some(Rc::new(on_result)), Some(transition));
    }

    /// Push a page with an explicit [`PushOptions`] — the full-control variant
    /// carrying a back-press [`BackPolicy`] (and, for a
    /// [`DismissAnimated`](BackPolicy::DismissAnimated) overlay, its dismiss
    /// signal) alongside opacity/transition/result. The dismissable-overlay
    /// helpers push through this; every other `push*` method
    /// pushes with [`BackPolicy::Pop`].
    pub fn push_with_options(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        options: PushOptions<State>,
    ) {
        self.enqueue(NavOp::Push {
            builder: Rc::new(builder),
            opaque: options.opaque,
            on_result: options.on_result,
            transition: options.transition,
            back: options.back,
            dismiss_signal: options.dismiss_signal,
            on_visibility: options.on_visibility,
            route: options.route,
            pop_swipe: options.pop_swipe,
        });
    }

    /// Shared push-op construction every `push*` method above funnels through —
    /// the five public variants differ only in which of `opaque`/`on_result`/
    /// `transition` they fix vs. expose. All push with [`BackPolicy::Pop`] and
    /// no dismiss signal, and no visibility observer; a page wanting any of those
    /// uses [`push_with_options`](Self::push_with_options).
    fn push_impl(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        opaque: bool,
        on_result: Option<ResultCallback<State>>,
        transition: Option<TransitionSpec>,
    ) {
        self.enqueue(NavOp::Push {
            builder: Rc::new(builder),
            opaque,
            on_result,
            transition,
            back: BackPolicy::Pop,
            dismiss_signal: None,
            on_visibility: None,
            route: None,
            pop_swipe: None,
        });
    }

    /// Pop the top page with no result payload (a plain back-navigation). A pop
    /// of the last/root page is ignored (a navigator always keeps one page).
    pub fn pop(&self) {
        self.enqueue(NavOp::Pop {
            result: PopResult::empty(),
        });
    }

    /// Pop the top page, handing `result` to its pusher-registered
    /// [`push_for_result`](Self::push_for_result) callback.
    pub fn pop_with_result(&self, result: PopResult) {
        self.enqueue(NavOp::Pop { result });
    }

    /// Route a back press through the top page's [`BackPolicy`] (the
    /// entry the facade's back handler drives instead of a bare
    /// [`pop`](Self::pop)):
    ///
    /// - [`Pop`](BackPolicy::Pop) → a normal pop (transitions preserved; a safe
    ///   no-op at the root);
    /// - [`DismissAnimated`](BackPolicy::DismissAnimated) → fires the top page's
    ///   dismiss signal (stack unchanged; the overlay animates its own exit and
    ///   pops itself), see [`BackPolicy`]'s observation seam;
    /// - [`Veto`](BackPolicy::Veto) → the press is consumed but nothing happens.
    ///
    /// Recorded like every other op and applied at the next rebuild (never
    /// self-mutating mid-event). Whether this call *would* claim the press is
    /// [`back_interest`](Self::back_interest).
    pub fn request_back(&self) {
        self.enqueue(NavOp::RequestBack);
    }

    /// Replace the top page in place with an opaque page built by `builder`,
    /// using the navigator's default transition.
    pub fn replace(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.enqueue(NavOp::Replace {
            builder: Rc::new(builder),
            opaque: true,
            transition: None,
            route: None,
        });
    }

    /// Replace the top page with an explicit [`TransitionSpec`], overriding the
    /// navigator's default for this replace only.
    pub fn replace_with(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        transition: TransitionSpec,
    ) {
        self.enqueue(NavOp::Replace {
            builder: Rc::new(builder),
            opaque: true,
            transition: Some(transition),
            route: None,
        });
    }

    /// Replace the top page with an explicit [`ReplaceOptions`] — the
    /// full-control variant carrying a route identity alongside opacity
    /// and transition. [`Router::go`](super::router::Router::go)/
    /// [`Router::replace`](super::router::Router::replace) push through this.
    pub fn replace_with_options(
        &self,
        builder: impl Fn() -> AnyView<State> + 'static,
        options: ReplaceOptions,
    ) {
        self.enqueue(NavOp::Replace {
            builder: Rc::new(builder),
            opaque: options.opaque,
            transition: options.transition,
            route: options.route,
        });
    }

    /// Record `op`, then raise [`frust_core::mark_pending_result_flush`] so the
    /// next tick's mobile frame gate `Run`s and reaches the rebuild that drains
    /// it — the same guarantee [`finalize_transition`](NavigatorWidget::finalize_transition)
    /// already gives a pop-result callback, reused here for a different reason:
    /// this flag is not just "a callback needs `&mut State`", it is the shell's
    /// one thread-affine "something is owed, run the next frame regardless of
    /// what else is dirty" side channel, and a queued op recorded from outside
    /// any input/signal path (a `spawn_local` continuation, e.g.) needs exactly
    /// that with no callback involved at all. Without it a page could mount and
    /// sit unpainted until whatever input happened to arrive next forced a
    /// frame — see the [module docs](self).
    ///
    /// Unconditional, on every op, deliberately, even though the shared flag's
    /// other reader (`RenderRoot::rebuild`'s pending-flush convergence loop)
    /// cannot tell "just force a run" apart from "a callback is genuinely
    /// owed": the frame that applies a queued op also pays one extra, empty
    /// `app_logic` + view-diff pass it did not strictly need. That pass is
    /// bounded (never more than one here, since nothing re-raises the flag for
    /// a plain structural op) and lands only on the frame a nav op was actually
    /// queued, not on every frame — a cost this crate's authoring toolkit
    /// already treats as affordable (rebuilds are cheap by construction) and a
    /// small, known price next to the bug it replaces: a page mounted with zero
    /// frames painted for 15+ seconds.
    fn enqueue(&self, op: NavOp<State>) {
        self.ops.borrow_mut().push(op);
        frust_core::mark_pending_result_flush();
    }

    /// Take the queued ops (leaving the queue empty). Called by
    /// [`NavigatorView::rebuild`]/`build`.
    pub(super) fn drain(&self) -> Vec<NavOp<State>> {
        std::mem::take(&mut *self.ops.borrow_mut())
    }
}
