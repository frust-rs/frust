//! The navigator core: a retained page stack with imperative
//! push/pop/replace, per-page result callbacks, opaque-page paint culling, and
//! test-pinned capture/focus/IME page-switch semantics.
//!
//! # Shape
//!
//! [`navigator`] is the app-facing view fn: `navigator(controller,
//! initial_page_builder)` produces a [`NavigatorView`] whose retained
//! [`NavigatorWidget`] owns a `Vec` of page entries. The
//! **[`NavigatorController`]** is the app-state handle the app keeps in its
//! `Component::State` (a cloneable `Rc<RefCell<…>>`): it *records requested ops*
//! (`push`/`pop`/`replace`), which the widget *applies at rebuild*, never
//! self-mutating mid-event (`docs/CODE_STANDARDS.md`, "Controlled components
//! never self-mutate").
//!
//! # Op application is view-driven (at rebuild), which enqueue guarantees runs
//!
//! Structural ops are drained and applied in [`NavigatorView::rebuild`] (a
//! `BuildCtx` pass), *not* inside `NavigatorWidget::event`: building a new page
//! pod ([`crate::authoring::build_child`]) and tearing a popped one down
//! ([`crate::authoring::teardown_child`]) both need a `BuildCtx`. A rebuild runs
//! every frame only on desktop; the mobile shells gate a frame behind a run/skip
//! decision (`frust-shell-common`'s `FrameGate`) that a bare queued op does not
//! by itself satisfy, so a *programmatic* push/pop (from a background task, with
//! no triggering event) is otherwise invisible to it. [`NavigatorController::enqueue`]
//! closes that gap unconditionally: every recorded op also raises
//! [`frust_core::mark_pending_result_flush`], which the mobile frame gate peeks
//! (`FrameInputs::deferred_callbacks_pending`) as a run-forcing input independent
//! of anything else dirty — the same flag [`finalize_transition`](NavigatorWidget::finalize_transition)
//! already raises for a pop-result callback (below), reused here for a queued op
//! rather than a callback needing `&mut State`. Left ungated, a programmatic push
//! measured on device as a page mounted but painted nothing for 15+ seconds,
//! until whatever input arrived next forced a frame. On every stack mutation the
//! widget then applies the page-switch contract the structural-rebuild machinery
//! does not cover for a hand-managed stack, in that order: (a) cancel an
//! in-flight capture on the outgoing page ([`crate::authoring::cancel_pod`]'s
//! synthetic `Cancel`), (b) clear its focus flag, (c) publish a *cleared* IME
//! surface on the next paint so the platform keyboard hides deterministically
//! rather than waiting for the lazy event-pass convergence `RenderRoot`
//! otherwise relies on. On a push the outgoing page is the one being
//! **covered**.
//!
//! A [`pop`](NavigatorController::pop_with_result) result destined for a
//! pusher-registered `on_result` callback needs `&mut State` — which a rebuild
//! (`BuildCtx`) does not carry — so the callback is queued at rebuild and flushed
//! at the start of the next [`NavigatorWidget::event`] pass, where the erased
//! app state is in scope. The mark already raised at `enqueue` time is what makes
//! the *same* rebuild dispatch a non-input [`InputEvent::Housekeeping`] broadcast
//! and flush the callback before the frame ends, so a result lands on the frame
//! that produced it, not just on some later frame the gate happens to run — the
//! [`apply_pop`](NavigatorWidget::apply_pop) call site that queues the callback
//! marks it again regardless, a defensive second raise (idempotent, so free) in
//! case a future caller ever reaches it outside the op queue. Waiting on the
//! next touch instead measured on device as a sheet opening seconds after its
//! menu row — or never, when that touch went to chrome outside the navigator. An
//! eager `NavOp::Pop` delivers on the pop's own frame; an interactive edge-swipe
//! pop delivers on its settle frame, since that is where it queues (via
//! `finalize_transition`, not `enqueue` — an interactive pop is driven from
//! `NavigatorWidget::event` directly, never through the op queue, so it still
//! needs its own explicit mark). See [`NavigatorController::push_for_result`].
//!
//! # Paint culling (Flutter opaque-route parity)
//!
//! Only the topmost **settled opaque** page (and any transparent pages stacked
//! above it) is laid out and painted; pages fully covered by an opaque page keep
//! their retained widgets (so their state survives) but are neither laid out nor
//! painted while covered. Layout runs unconditionally every frame, so a page
//! revealed by a pop is re-laid-out and correct on the very next frame.
//!
//! # Root overlay host
//!
//! [`overlay_host`] is the same widget wearing a different hat: a navigator whose
//! root page is the *whole app* and whose pushed pages are app-level modals, so an
//! overlay dims and blocks chrome an inner navigator's overlay cannot reach. A
//! constructor, not a second widget — everything below (input routing, R23,
//! `BackPolicy`, dismiss signals, `on_result`) applies to it unchanged.
//!
//! # Accessibility reach (R23)
//!
//! The accessibility tree follows **input routing**, not painting:
//! [`NavigatorWidget::semantics`](Widget::semantics) forwards exactly the pages
//! [`NavigatorWidget::input_routed_pages`] says an event could reach — today the
//! top page alone — and omits every other page outright: a deliberate, documented
//! exception to the forward-to-every-child container rule in
//! `docs/CODE_STANDARDS.md`, whose derivation the `semantics` doc comment carries.
//!
//! # Observation seams (reactive-free, by construction)
//!
//! Nothing outside a page's own subtree can reach into the navigator, so every
//! observation is *published* or *pushed* — never polled through the widget, and
//! never through a signal (`frust-widgets` carries no reactive dependency; signal
//! mirroring is the facade's job, as `back_glue`/`router_glue` do it). The seams
//! themselves are catalogued in `docs/WIDGETS_ARCHITECTURE.md`; what is fixed here
//! is *where* each lives and why:
//!
//! * **Transition** — a published [`TransitionState`] snapshot, on the
//!   **controller** ([`NavigatorController::transition`]) because the chrome
//!   matching page motion is a *sibling* of the navigator, not a descendant. Its
//!   own doc carries the read-timing contract.
//! * **Page visibility** — a **callback** ([`PushOptions::on_visibility`],
//!   [`NavigatorView::on_root_visibility`]) rather than a published cell, because
//!   a covered page has no pass in which to poll one. Single derivation:
//!   [`NavigatorWidget::visibility_of`], from the same `base_visible_index`
//!   layout and paint cull against.
//!
//! Neither seam changes disposal: a covering push still does **not** run a
//! page's `on_cleanup`, so its widget state survives the cover.
//!
//! # Back reach follows input routing too (R23)
//!
//! Back arbitration obeys the same reach as input and semantics: a navigator
//! whose **hosting page** is not one of its host navigator's
//! [`input_routed_pages`](NavigatorWidget::input_routed_pages) reports **no**
//! [`back_interest`](NavigatorController::back_interest), whatever its own stack
//! looks like. Otherwise a nested navigator sitting on a covered page — a
//! section stack with a detail page pushed over it — would claim the press and
//! pop a stack nobody can see, leaving the visible page put.
//!
//! The mechanism is a third published seam, and the only one that flows
//! *downward*: each [`PageEntry::reach`] is an `Rc<Cell<bool>>` its navigator
//! sets to `own_reachability && page is input-routed`, installed as an ambient
//! scope ([`with_page_reach`]) around that page's builder and subtree reconcile.
//! A navigator built anywhere under it captures the cell
//! ([`ambient_page_reach`]) as its own [`NavigatorWidget::host_reach`] and ANDs
//! it into what it publishes — so the invariant composes to any depth with no
//! tree walk, and holds even for a page frozen by
//! [`cull_covered_builds`](NavigatorView::cull_covered_builds) (the cell is shared
//! and live, not a per-wire snapshot).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, DiscardScene, EditingState, EventCtx,
    EventResult, FrameTime, HeroDirective, HeroFrames, ImeState, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, SpringDesc, TOUCH_SLOP, View, Widget,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size, Vec2};

use super::ambient::{
    ambient_page_reach, ambient_swipe_claim, host_reachable, with_page_reach, with_swipe_claim,
};
use super::edge_swipe::{EDGE_SWIPE_ZONE_DP, EdgeSwipe};
use super::options::NavOp;
use super::path::Location;
use super::route_state::RouteStack;
use super::transition::{
    Layer, PageTransition, TransitionDriver, TransitionSpec, TransitionState, lerp_rect,
    make_driver, resolve_layers, resolve_spec, settle_driver,
};

pub use super::controller::NavigatorController;
pub use super::options::{
    BackPolicy, NavigatorId, PageBuilder, PageVisibility, PopResult, PushOptions, ReplaceOptions,
    ResultCallback, RouteChangeCallback, VisibilityCallback,
};
pub use super::view::{NavigatorView, navigator, overlay_host};

/// One retained page in the [`NavigatorWidget`]'s stack: its builder (re-run each
/// rebuild), the last view it produced (for reconciliation), the retained child
/// pod, its opacity, and the pusher's result callback (fired when this page pops).
pub(super) struct PageEntry<State: 'static> {
    builder: PageBuilder<State>,
    view: AnyView<State>,
    pod: ChildPod,
    opaque: bool,
    on_result: Option<ResultCallback<State>>,
    /// The transition this page was pushed/replaced with — *reversed* when the
    /// page is later popped (a pop animates the popped page's own transition
    /// backwards, Flutter-parity: a route carries its transition).
    pub(super) transition: TransitionSpec,
    /// How a back press routed through
    /// [`request_back`](NavigatorController::request_back) treats this page.
    /// Pushed pages set it via [`PushOptions::back`]; the root and
    /// replaced pages default to [`BackPolicy::Pop`].
    pub(super) back: BackPolicy,
    /// The shared generation cell a
    /// [`DismissAnimated`](BackPolicy::DismissAnimated) back press increments so
    /// the page's own widget subtree observes it (see [`BackPolicy`]'s seam).
    /// `None` for any page that did not supply one.
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// This page's [`PageVisibility`] as of the last
    /// [`publish_visibility`](NavigatorWidget::publish_visibility) pass.
    /// `None` until the *first* publish, so a freshly pushed page's opening
    /// [`Current`](PageVisibility::Current) always fires (there is no
    /// "unknown" enum variant to model that with).
    visibility: Option<PageVisibility>,
    /// The page-visibility observer from [`PushOptions::on_visibility`] (or, for
    /// the root page, [`NavigatorView::on_root_visibility`]). `None` for a page
    /// that registered none.
    on_visibility: Option<VisibilityCallback>,
    /// Whether the *previous* per-page reconcile pass saw this page as
    /// [`Covered`](PageVisibility::Covered). Only read when
    /// [`NavigatorView::cull_covered_builds`] is on, and it is what implements
    /// that switch's "the frame a page becomes covered still rebuilds it" rule:
    /// a page is skipped only once it has *already* been reconciled while
    /// covered.
    reconciled_covered: bool,
    /// This page's route identity, stamped at push time from
    /// [`PushOptions::route`]/[`NavigatorView::root_route`]/
    /// [`ReplaceOptions::route`]. `None` for a bare-builder overlay/dialog
    /// push — see `route_state`'s module docs.
    route: Option<Location>,
    /// This page's edge-swipe override from [`PushOptions::pop_swipe`] —
    /// the highest-ranked slot in [`NavigatorWidget::swipe_armable`]'s
    /// resolution. `None` for the root page and for any page pushed/replaced
    /// without one, deferring to the navigator's own resolved default
    /// (`pop_swipe_enabled`).
    pub(super) pop_swipe: Option<bool>,
    /// Whether an input event can reach **this page**, all the way up: this
    /// navigator is itself reachable AND this page is in
    /// [`input_routed_pages`](NavigatorWidget::input_routed_pages). Republished
    /// by [`publish_reach`](NavigatorWidget::publish_reach) on every stack
    /// mutation and rebuild.
    ///
    /// Installed as the ambient [`PAGE_REACH`] scope while this page's builder
    /// and subtree reconcile run, so a *nested* navigator on this page can read
    /// it — the seam that makes back arbitration follow input routing (R23) even
    /// though a nested navigator has no idea what the navigator hosting it is
    /// doing. Shared with every such descendant, so it stays a live read rather
    /// than a snapshot: a page frozen by
    /// [`cull_covered_builds`](NavigatorView::cull_covered_builds) still reports
    /// truthfully.
    pub(super) reach: Rc<Cell<bool>>,
}

/// The single in-flight page transition a [`NavigatorWidget`] owns (Flutter
/// parity: created per push/pop, disposed on settle). Pairs the progress
/// [`TransitionDriver`] with the retained *leaving* page, when the op removed it
/// from the stack (pop/replace); a push's leaving page stays in the stack below
/// the new top, so `stashed` is `None` there.
pub(super) struct ActiveTransition<State: 'static> {
    /// Drives `0.0..=1.0`; advanced from `PaintCtx::frame_time` during paint.
    pub(super) driver: TransitionDriver,
    /// The visual preset (slide/fade/parallax geometry).
    pub(super) preset: PageTransition,
    /// Direction: `true` reverses the horizontal motion + paint order (a pop).
    pub(super) is_pop: bool,
    /// The removed page retained until settle (pop/replace). `None` for a push,
    /// whose leaving page is still in the stack at `len - 2`.
    pub(super) stashed: Option<PageEntry<State>>,
    /// The spring a manual [`settle`](NavigatorWidget::settle_transition) uses
    /// when the timing mode is duration-based (a duration has no spring).
    pub(super) settle_spring: SpringDesc,
    /// Set by paint when the driver reaches rest; the next rebuild finalizes the
    /// transition (tears down `stashed`, resumes culling).
    pub(super) settled: bool,
    /// This transition is being driven by an interactive edge-swipe: its
    /// progress is `Held` by the drag, then settled on release. An
    /// interactive pop stashed the top page *without* queuing its result
    /// callback (a swipe may still cancel), so finalize does the completion
    /// bookkeeping the [`NavOp::Pop`] path did eagerly.
    pub(super) interactive: bool,
    /// Set when an interactive pop was *cancelled* (settled toward `0.0`): finalize
    /// pushes the stashed page back onto the stack instead of tearing it down (the
    /// page was never really popped). See [`NavigatorWidget::finalize_transition`].
    pub(super) restore_on_finalize: bool,
    /// Shared-element ("hero") state. Page-local rects of the tagged
    /// heroes discovered on the **leaving** page during the previous transition
    /// paint, keyed by tag. `layout`/`paint` capture these each frame; the next
    /// frame reads them to place the morph overlay. Empty until the first paint
    /// discovers any (so the morph starts a frame into the flight — the rects
    /// are static page layout, so the delay is invisible).
    pub(super) hero_leaving: HashMap<String, Rect>,
    /// Page-local hero rects discovered on the **entering** page — the morph
    /// target endpoint. See [`hero_leaving`](ActiveTransition::hero_leaving).
    pub(super) hero_entering: HashMap<String, Rect>,
    /// The unresolved [`TransitionSpec`] awaiting theme resolution on the first
    /// paint — the LAZY driver seam mirroring [`motion::switcher`](crate::motion).
    /// A programmatic transition is staged in a `BuildCtx`
    /// ([`start_transition`](NavigatorWidget::start_transition)), which carries
    /// no theme, so [`Timing::ThemeDefault`](super::transition::Timing) and
    /// `reduce_motion` cannot resolve there. `Some` until the first
    /// [`paint_transition`](NavigatorWidget::paint_transition) resolves it
    /// against the active [`MotionScheme`](frust_theme::MotionScheme) — via
    /// [`resolve_spec`] — and rebuilds `driver`/`preset`/`settle_spring` before
    /// any frame is staged; `None` thereafter (and always `None` for the
    /// interactive edge-swipe path, whose progress is drag-held, not
    /// theme-timed). With no theme threaded the `make_driver` fallback built at
    /// `start_transition` (M3 defaults) stands — the unthemed behavior
    /// `docs/CODE_STANDARDS.md` mandates.
    pub(super) pending_spec: Option<TransitionSpec>,
}

impl<State: 'static> crate::authoring::VisitPods for PageEntry<State> {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.pod);
    }
}

impl<State: 'static> crate::authoring::VisitPods for ActiveTransition<State> {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        // Only the stashed (removed-but-still-animating) page: a transition's
        // other participants are still in `pages`.
        crate::authoring::VisitPods::visit_pods(&self.stashed, visitor);
    }
}

/// The retained widget for a [`NavigatorView`]: owns the page stack and applies
/// the [`NavigatorController`]'s queued ops at rebuild. See the [module docs](self).
pub struct NavigatorWidget<State: 'static> {
    pub(super) pages: Vec<PageEntry<State>>,
    /// Pop-result callbacks awaiting `&mut State` — flushed at the start of the
    /// next [`event`](NavigatorWidget::event) pass, which the queuing rebuild
    /// guarantees itself by raising
    /// [`frust_core::mark_pending_result_flush`] (see the [module docs](self)).
    pending_results: Vec<(ResultCallback<State>, PopResult)>,
    /// Set on every stack mutation; the next paint publishes a cleared IME surface
    /// and clears this, so the platform keyboard hides deterministically.
    pub(super) needs_ime_clear: bool,
    /// The navigator's default transition (per-op overrides win). Refreshed from
    /// the view on rebuild so an app can change it live.
    default_transition: TransitionSpec,
    /// The single in-flight transition, if any. `None` between
    /// transitions — the common case, where paint/layout cull normally.
    pub(super) transition: Option<ActiveTransition<State>>,
    /// Whether the interactive edge-swipe back gesture is enabled.
    /// Resolved from the view each rebuild — default-on for the iOS-push preset,
    /// or explicitly via [`NavigatorView::pop_swipe`].
    pub(super) pop_swipe_enabled: bool,
    /// The in-progress edge-swipe gesture state.
    pub(super) edge: EdgeSwipe,
    /// The most recent frame time seen during [`paint`](NavigatorWidget::paint),
    /// reused as the event-pass timestamp for velocity tracking — the event pass
    /// carries no clock of its own (time is provided only at paint). The
    /// same seam [`ScrollWidget`](crate::ScrollWidget) uses.
    last_frame_time: FrameTime,
    /// The shared depth slot published to the [`NavigatorController`] every
    /// `build`/`rebuild`. A clone of the controller's
    /// `Rc<Cell<usize>>`, updated by [`publish_state`](Self::publish_state)
    /// after every stack mutation so `NavigatorController::can_pop` reads the
    /// authoritative page count.
    depth: Rc<Cell<usize>>,
    /// The shared back-interest slot published to the [`NavigatorController`]
    /// alongside `depth`. A clone of the controller's
    /// `Rc<Cell<bool>>`, recomputed by [`publish_state`](Self::publish_state)
    /// from the current depth + top-page [`BackPolicy`] after every stack
    /// mutation so `NavigatorController::back_interest` is authoritative.
    back_interest: Rc<Cell<bool>>,
    /// The shared [`TransitionState`] slot published to the
    /// [`NavigatorController`]. A clone of the controller's
    /// `Rc<Cell<TransitionState>>`, written at every transition edge
    /// (start/interactive-start/hold/settle/finalize) *and* on every paint frame
    /// that advances the driver — see
    /// [`NavigatorController::transition`]'s timing contract.
    transition_state: Rc<Cell<TransitionState>>,
    /// The shared liveness slot [`mount`](NavigatorController::mount)
    /// increments and [`unmount_cell`] decrements, cloned from whichever
    /// controller's cells this widget is currently bound to (`build`, or the
    /// last controller-swap `rebuild`). Stored on the *widget* — not read back
    /// off `self.controller` — so `teardown`'s decrement always pairs with
    /// whichever cell the widget last incremented, even if a later rebuild
    /// swapped `NavigatorView::controller` again in between: pairing is
    /// structural (same field written and read), never a lookup by identity
    /// that could drift. See [`NavigatorView::rebuild`]'s controller-swap
    /// handling for how this field gets re-bound.
    mounted: Rc<Cell<usize>>,
    /// Whether a [`Covered`](PageVisibility::Covered) page skips its per-frame
    /// reconcile. Refreshed from the view each rebuild (live-configurable, like
    /// `pop_swipe_enabled`); default `false`.
    cull_covered_builds: bool,
    /// The [`PageEntry::reach`] cell of the page **hosting this navigator**, or
    /// `None` for a navigator at the top level (a root navigator / a root
    /// [`overlay_host`], whose reach is unconditional).
    ///
    /// Captured from the ambient [`PAGE_REACH`] scope at `build` and re-captured
    /// at every `rebuild`, so it always names whichever page this navigator is
    /// currently reconciled under. Read through
    /// [`reachable`](Self::reachable) — the one input this navigator has into
    /// "can a back press legitimately reach me?".
    host_reach: Option<Rc<Cell<bool>>>,
    /// The shared route-state slot published to the [`NavigatorController`] —
    /// a clone of the controller's `Rc<RefCell<RouteStack>>`, refreshed
    /// by [`publish_route_stack`](Self::publish_route_stack) — called from
    /// [`publish_state`](Self::publish_state) — after every committed stack
    /// mutation.
    route_stack: Rc<RefCell<RouteStack>>,
    /// The navigator-wide route-change observer from
    /// [`NavigatorView::on_route_change`], refreshed every `build`/`rebuild`
    /// (unlike per-page `on_visibility`, since this observes the whole
    /// navigator).
    route_change: Option<RouteChangeCallback>,
}

/// Whether the navigator's **own stack** wants a back press ahead-of-time: it is
/// poppable (`depth > 1`) **or** the top page's [`BackPolicy`] is not
/// [`Pop`](BackPolicy::Pop). Pure so it is unit-testable directly, including the
/// depth-1-with-overlay case a raw `can_pop` cannot express.
///
/// This is only half the answer [`NavigatorController::back_interest`] gives:
/// the R23 reach gate (is the page hosting this navigator input-routed at all?)
/// is ANDed on at *read* time, deliberately not baked in here — see
/// [`NavigatorController::host_reach`].
fn compute_back_interest(depth: usize, top_policy: BackPolicy) -> bool {
    depth > 1 || top_policy != BackPolicy::Pop
}

/// Decrement a [`NavigatorController`]'s `mounted` cell, saturating so an
/// already-zero cell can never wrap. The one place a mounted count is ever
/// decremented — [`NavigatorView::teardown`] and [`NavigatorView::rebuild`]'s
/// controller-swap handling both call this against the cell
/// [`NavigatorWidget`] itself owns (never by looking `NavigatorController`
/// back up), which is what makes the mount/unmount pairing structural rather
/// than an identity lookup that could drift after a swap.
fn unmount_cell(mounted: &Cell<usize>) {
    mounted.set(mounted.get().saturating_sub(1));
}

impl<State: 'static> NavigatorWidget<State> {
    /// Publish the current page-stack depth **and** back-interest to the shared
    /// controller slots, refresh the published [`TransitionState`]'s depths,
    /// fire any page-visibility changes, and republish the route-state
    /// snapshot via [`publish_route_stack`](Self::publish_route_stack).
    /// Called after every **committed** stack mutation — at the end of
    /// `apply_ops`, after a transition finalize, and at the end of
    /// `build`/`rebuild` — so `NavigatorController::depth`/`can_pop`/
    /// `back_interest`/`transition`/`route_stack` read authoritative values.
    /// Deliberately **not** called from
    /// [`begin_interactive_pop`](Self::begin_interactive_pop): an in-flight
    /// interactive edge-swipe pop is uncommitted (see `route_state`'s module
    /// docs' staleness contract).
    fn publish_state(&mut self) {
        self.depth.set(self.pages.len());
        let top_policy = self.pages.last().map(|p| p.back).unwrap_or(BackPolicy::Pop);
        // Reach first: every page's cell must be current before a nested
        // navigator on one of them reconciles below (R23).
        self.publish_reach();
        self.back_interest
            .set(compute_back_interest(self.pages.len(), top_policy));
        // Keep the published transition's depths coherent even for a stack
        // mutation that started no transition at all (an instant push/pop): the
        // stack is always the transition's *destination*, and with nothing in
        // flight it is both endpoints.
        let mut t = self.transition_state.get();
        t.to_depth = self.pages.len();
        if !t.active {
            t.from_depth = t.to_depth;
        }
        self.transition_state.set(t);
        self.publish_visibility();
        self.publish_route_stack();
    }

    /// Recompute the route-state snapshot from the current page stack and
    /// publish it iff it actually changed — an unchanged stack costs only the
    /// O(depth) comparison below, no allocation (see `route_state`'s module
    /// docs' derivation note). Called from
    /// [`publish_state`](Self::publish_state), so it runs at build, at the
    /// end of `apply_ops`, after a transition finalize, and at the end of
    /// `rebuild` — **never** from [`begin_interactive_pop`](Self::begin_interactive_pop)
    /// itself, which is the deliberate uncommitted-swipe gap `route_state`'s
    /// staleness contract documents.
    ///
    /// **The interactive guard.** `rebuild`'s trailing `publish_state` call is
    /// unconditional (it also refreshes `depth`/`back_interest` every pass),
    /// so it still runs on every settle-spring frame between release and
    /// finalize — not just at steal. `self.pages` has already lost the
    /// stashed page for that whole window (popped at steal, restored or torn
    /// down only at [`finalize_transition`](Self::finalize_transition), which
    /// clears `self.transition` first thing), so this checks the transition
    /// itself rather than trying to keep every OTHER call site from ever
    /// running during the drag: while `self.transition` is `Some` and
    /// `interactive`, the stack is still provisional and this returns without
    /// touching the published snapshot at all — not even the unchanged-check
    /// below runs. The settle-frame publish (`finalize_transition` already
    /// cleared `self.transition`) is what finally sees the real diff, in one
    /// step, whichever way the drag resolved.
    fn publish_route_stack(&mut self) {
        if self.transition.as_ref().is_some_and(|t| t.interactive) {
            return;
        }
        let unchanged = {
            let published = self.route_stack.borrow();
            let entries = published.entries();
            entries.len() == self.pages.len()
                && entries
                    .iter()
                    .zip(self.pages.iter())
                    .all(|(prev, page)| *prev == page.route)
        };
        if unchanged {
            return;
        }
        let entries: Vec<Option<Location>> = self.pages.iter().map(|p| p.route.clone()).collect();
        self.route_stack.borrow_mut().set(entries);
        if let Some(observer) = self.route_change.clone() {
            // Clone the `Rc` out before firing, like `publish_visibility`: the
            // callback is app code and may reach back into the controller.
            observer(&self.route_stack.borrow());
        }
    }

    /// Whether **this navigator** is reachable by input at all — `true` unless
    /// the page hosting it is not the page its own host navigator routes input
    /// to (see [`host_reach`](Self::host_reach)).
    ///
    /// Always `true` for a top-level navigator, which is why every
    /// single-navigator app is unaffected by the R23 back gate.
    fn reachable(&self) -> bool {
        host_reachable(self.host_reach.as_ref())
    }

    /// Republish every page's [`PageEntry::reach`] cell: a page is reachable iff
    /// this navigator is reachable AND the page is in
    /// [`input_routed_pages`](Self::input_routed_pages).
    ///
    /// **This is the whole propagation mechanism.** The AND folds this
    /// navigator's own reachability into what it publishes to its pages, so the
    /// invariant composes to any nesting depth without anyone walking the tree:
    /// a navigator three levels down reads one cell and gets the answer for the
    /// entire chain above it. Called from
    /// [`publish_state`](Self::publish_state), i.e. after every stack mutation
    /// and at the end of every `build`/`rebuild`, and always *before* the
    /// per-page reconcile loop that re-runs page builders — so a nested
    /// navigator reconciling this pass reads the value for the stack it is
    /// actually being reconciled into.
    ///
    /// Reach is derived from `input_routed_pages`, not from
    /// [`PageVisibility`](crate::PageVisibility): a page under a *transparent*
    /// overlay is still `Visible` but is routed no input, and R23 tracks input
    /// routing exactly (the same divergence [`semantics`](Widget::semantics)
    /// documents).
    fn publish_reach(&mut self) {
        let own = self.reachable();
        let routed = self.input_routed_pages();
        for index in 0..self.pages.len() {
            self.pages[index].reach.set(own && routed.contains(&index));
        }
    }

    /// Fire every page's [`PushOptions::on_visibility`] observer whose
    /// [`PageVisibility`] changed since the last pass (and no others — a value is
    /// never reported twice in a row).
    ///
    /// Called only from [`publish_state`](Self::publish_state), so it runs at
    /// build, at the end of `apply_ops`, after a transition finalize, and at the
    /// end of `rebuild` — in every case *before* the per-page reconcile loop
    /// gets to decide anything, which is what lets a revealed page rebuild in the
    /// same pass that revealed it.
    ///
    /// Re-entrancy is safe by construction: a callback that calls
    /// `controller.push()`/`pop()` only records a [`NavOp`], drained at the next
    /// rebuild — it cannot re-enter the widget.
    fn publish_visibility(&mut self) {
        for i in 0..self.pages.len() {
            let next = self.visibility_of(i);
            if self.pages[i].visibility == Some(next) {
                continue;
            }
            self.pages[i].visibility = Some(next);
            // Clone the `Rc` out before firing: the callback is app code and may
            // reach back into the controller.
            if let Some(observer) = self.pages[i].on_visibility.clone() {
                observer(next);
            }
        }
    }

    /// Where the page at `index` sits in the stack right now.
    ///
    /// **The single derivation of "visible" in the navigator**, computed from the
    /// same [`base_visible_index`](Self::base_visible_index) that `layout` and
    /// `paint` already cull against — the visibility seam, the covered-build cull
    /// and the semantics rule all read this one function rather than
    /// recomputing it.
    ///
    /// [`Current`](PageVisibility::Current) iff `index` is the top of the stack;
    /// [`Covered`](PageVisibility::Covered) iff it is below the topmost opaque
    /// page; [`Visible`](PageVisibility::Visible) otherwise (a page under a
    /// transparent overlay).
    ///
    /// # During a transition
    ///
    /// Layout/paint culling is *suspended* mid-transition, but visibility is
    /// computed against the settled stack regardless: a pop/replace stashes the
    /// leaving page out of `self.pages` and a push already has the new page on
    /// top, so `self.pages` **is** the destination stack from the transition's
    /// first frame. A page therefore learns it is about to be covered when the
    /// push is applied, not 340ms later — which is the point, since the
    /// observation exists to let it release resources.
    fn visibility_of(&self, index: usize) -> PageVisibility {
        if index + 1 >= self.pages.len() {
            PageVisibility::Current
        } else if index < self.base_visible_index() {
            PageVisibility::Covered
        } else {
            PageVisibility::Visible
        }
    }

    /// The index of the topmost **opaque** page — the bottom of the visible
    /// (laid-out + painted) range. Pages below it are culled. With no opaque page
    /// at all (an all-transparent stack), everything is visible.
    fn base_visible_index(&self) -> usize {
        for i in (0..self.pages.len()).rev() {
            if self.pages[i].opaque {
                return i;
            }
        }
        0
    }

    /// Publish a fresh [`TransitionState`] for a transition that is *starting*,
    /// bumping the generation. The stack has already been mutated to the
    /// destination when this runs, so `to_depth` is simply the current page
    /// count; `from_depth` is the pre-op depth the caller knows.
    ///
    /// Shared by the programmatic
    /// [`start_transition`](Self::start_transition) and the interactive
    /// edge-swipe [`begin_interactive_pop`](Self::begin_interactive_pop) — the
    /// first two of the publication points listed on
    /// [`NavigatorController::transition`].
    pub(super) fn publish_transition_start(
        &self,
        from_depth: usize,
        progress: f64,
        is_pop: bool,
        interactive: bool,
    ) {
        let to_depth = self.pages.len();
        let generation = self.transition_state.get().generation.wrapping_add(1);
        self.transition_state.set(TransitionState {
            active: true,
            progress,
            is_pop,
            interactive,
            from_depth,
            to_depth,
            generation,
        });
    }

    /// Update the published progress of the in-flight transition, leaving every
    /// other field alone. `interactive` is set alongside it (a drag holds the
    /// progress; a release hands it back to a spring).
    pub(super) fn publish_transition_progress(&self, progress: f64, interactive: Option<bool>) {
        let mut t = self.transition_state.get();
        t.progress = progress;
        if let Some(interactive) = interactive {
            t.interactive = interactive;
        }
        self.transition_state.set(t);
    }

    /// Cancel any in-flight capture and clear the focus flag on the *current* top
    /// page — the page being covered/replaced/popped by a stack mutation.
    ///
    /// Capture unwinds via [`crate::authoring::cancel_pod`]'s synthetic `Cancel` (the outgoing
    /// widget's state machine must not fire on a later `Up`); focus is a reflected
    /// pod flag, so clearing it is enough (no widget-internal blur to drive) — the
    /// same asymmetry the container reconcilers document.
    pub(super) fn cancel_top(&mut self) {
        if let Some(top) = self.pages.last_mut() {
            if top.pod.is_active() {
                crate::authoring::cancel_pod(&mut top.pod);
                top.pod.set_active(false);
            }
            if top.pod.is_focused() {
                top.pod.set_focused(false);
            }
        }
    }

    /// Whether the page currently on top holds the recorded focus path
    /// ([`ChildPod::is_focused`](frust_core::ChildPod::is_focused)) — the
    /// outgoing/covered page's own half of the gate every
    /// [`needs_ime_clear`](Self::needs_ime_clear) producer in this widget shares.
    ///
    /// # Why every producer is gated
    ///
    /// Raising `needs_ime_clear` makes the next [`paint`](Widget::paint) publish
    /// [`cleared_ime_state`], and an **inactive** publish is not a value update:
    /// `RenderRoot` reads it as a full focus/IME **session release** at the root
    /// (see `docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle). The published
    /// surface bubbles last-write-wins, so a navigator that mutates its stack
    /// while the live session belongs to an *unrelated* subtree — a search field
    /// sitting above the navigator, painted earlier in the same frame — would
    /// otherwise kill that field's session every push/pop, deterministically and
    /// with no self-heal (the next paint re-seeds `has_focus == false` for the
    /// blurred field, so its own republish never fires; only a user tap recovers).
    ///
    /// So a producer clears only when the outgoing/covered subtree is the one
    /// that actually owns the session: `ctx.has_focus()` ANDed with the outgoing
    /// pod's own `is_focused()` — the same composition `frust-widgets`'
    /// `mark_orphan_if_live` applies to the orphan mark, ANDing the rebuild-pass
    /// chain down to *this navigator* onto the page's own link. Neither half
    /// alone is evidence: a page-pod flag can be stale under an already-blurred
    /// ancestor, and a live chain running past an unfocused navigator says
    /// nothing about it.
    ///
    /// # Read it before [`cancel_top`](Self::cancel_top)
    ///
    /// `cancel_top` clears this very flag, so every caller reads it *first*;
    /// reading after would report `false` unconditionally and suppress a clear
    /// that was genuinely owed.
    pub(super) fn top_pod_focused(&self) -> bool {
        self.pages.last().is_some_and(|p| p.pod.is_focused())
    }

    /// The effective transition for an op, resolving a `None` per-op override to
    /// the navigator's default.
    fn effective_spec(&self, over: Option<TransitionSpec>) -> TransitionSpec {
        over.unwrap_or(self.default_transition)
    }

    /// Begin a new transition, finalizing any in-flight one first (a new op
    /// supersedes a running transition — snap it to its end and tear down its
    /// retained page). `stashed` is the removed leaving page (pop/replace) or
    /// `None` for a push (leaving stays in the stack).
    ///
    /// # Interactive supersede
    ///
    /// If the superseded transition is an *unreleased* interactive edge-swipe
    /// pop, the [`finalize_transition`](Self::finalize_transition) call here
    /// **completes** it (tears down the stashed page and queues its result
    /// callback) rather than cancelling it — a programmatic op wins over an
    /// in-flight drag, and the drag's page does not spring back.
    fn start_transition(
        &mut self,
        spec: TransitionSpec,
        is_pop: bool,
        stashed: Option<PageEntry<State>>,
        ctx: &mut BuildCtx<'_>,
    ) {
        self.finalize_transition(ctx);
        // A stashed (leaving) page is out of `self.pages`, so `publish_reach`
        // will never see it again — mark it unreachable HERE or a nested
        // navigator riding it out would keep claiming back presses for the whole
        // flight (input is fully suppressed mid-transition anyway). A cancelled
        // interactive pop pushes the page back onto the stack, and the finalize
        // that does so is followed by an explicit `publish_state` that restores
        // this.
        if let Some(leaving) = stashed.as_ref() {
            leaving.reach.set(false);
        }
        // The stack is already at its destination here (the op mutated it before
        // staging the animation), so the *pre-op* depth is derived from the op
        // shape: a pop removed a page (now in `stashed`), a replace swapped one
        // in place, a push added one.
        let to_depth = self.pages.len();
        let from_depth = if is_pop {
            to_depth + 1
        } else if stashed.is_some() {
            to_depth
        } else {
            to_depth.saturating_sub(1)
        };
        // Build a fallback driver eagerly (the unthemed M3 default — current
        // behavior), and stash the *unresolved* spec so the first paint can
        // re-resolve `ThemeDefault` timing + `reduce_motion` against the live
        // theme (a `BuildCtx` carries none). See `pending_spec`.
        let (driver, settle_spring) = make_driver(spec.timing);
        self.transition = Some(ActiveTransition {
            driver,
            preset: spec.preset,
            is_pop,
            stashed,
            settle_spring,
            settled: false,
            interactive: false,
            restore_on_finalize: false,
            hero_leaving: HashMap::new(),
            hero_entering: HashMap::new(),
            pending_spec: Some(spec),
        });
        // Publication point: a programmatic transition starts at progress 0, not
        // interactive, with a fresh generation. This runs in a `BuildCtx` pass,
        // so a build-time observer sees `active` flip on this very frame.
        self.publish_transition_start(from_depth, 0.0, is_pop, false);
    }

    /// Dispose the active transition: tear down its retained (leaving) page, if
    /// any, and drop it. Called on settle (from rebuild) and when a new op
    /// supersedes a running transition.
    ///
    /// # Interactive-pop finalization
    ///
    /// A *cancelled* interactive edge-swipe pop
    /// ([`restore_on_finalize`](ActiveTransition)) never really removed its page —
    /// the stashed page is pushed back onto the stack (origin reset, so it lands
    /// at exact resting geometry) instead of torn down. A *completing* interactive
    /// pop, conversely, is where its result callback is queued (the swipe path
    /// defers this since the pop may still cancel — unlike the eager
    /// [`NavOp::Pop`] path). Also clears the edge-swipe drive flags: the
    /// transition ending means no interactive drive continues.
    fn finalize_transition(&mut self, ctx: &mut BuildCtx<'_>) {
        if let Some(mut t) = self.transition.take() {
            self.edge.active = false;
            self.edge.armed = false;
            self.edge.inner_claimed = false;
            if let Some(mut stashed) = t.stashed.take() {
                if t.restore_on_finalize {
                    // Cancelled interactive pop: the page was never popped — restore
                    // it at exact resting geometry.
                    stashed.pod.set_origin(Point::ZERO);
                    self.pages.push(stashed);
                } else {
                    // A completing interactive pop is the point where its pusher's
                    // result callback fires (the non-interactive pop queued it up
                    // front; the swipe defers until it commits).
                    if t.interactive
                        && let Some(callback) = stashed.on_result.take()
                    {
                        self.pending_results.push((callback, PopResult::empty()));
                        // Ask this frame's rebuild for a housekeeping pass so the
                        // callback runs on the settle frame instead of waiting for
                        // whatever input happens to arrive next.
                        frust_core::mark_pending_result_flush();
                    }
                    crate::authoring::teardown_child(&stashed.view, &mut stashed.pod, ctx);
                }
            }
            // Publication point: the transition is over. Publish an at-rest
            // snapshot for the (possibly just-restored) stack, keeping the
            // generation so an observer can still tell which transition ended.
            // Like `start_transition` this runs in a `BuildCtx` pass, so the
            // `active` falling edge is visible to a build-time observer on the
            // frame it happens.
            let generation = self.transition_state.get().generation;
            self.transition_state
                .set(TransitionState::settled(self.pages.len(), generation));
        }
    }

    /// Interactive-edge-swipe seam: pin the active transition's progress to `p`
    /// (an edge-swipe drag holds it here between frames). No-op if no
    /// transition is active.
    ///
    /// The gesture that drives this lives in `event`; the navigator supplies the
    /// held-progress driver state a swipe manipulates.
    pub fn set_transition_progress(&mut self, p: f64) {
        let Some(t) = self.transition.as_mut() else {
            return;
        };
        t.driver = TransitionDriver::Held { value: p };
        t.settled = false;
        // Publication point: a drag is holding the progress.
        self.publish_transition_progress(p, Some(true));
    }

    /// Interactive-edge-swipe seam: release the active transition into a spring
    /// settle toward `1.0` (non-negative `velocity`) or `0.0` (negative). No-op
    /// if no transition.
    ///
    /// Note: a settle toward `0.0` runs the *visual* reversal, but restoring the
    /// stack (un-popping the retained page on a cancelled pop) is the
    /// finalize path's responsibility — this seam only drives the progress
    /// driver.
    pub fn settle_transition(&mut self, velocity: f64) {
        let Some(t) = self.transition.as_mut() else {
            return;
        };
        let from = t.driver.value();
        let target = if velocity >= 0.0 { 1.0 } else { 0.0 };
        t.driver = settle_driver(t.settle_spring, from, velocity, target);
        t.settled = false;
        // Publication point: the drag released — progress is unchanged this
        // instant, but a spring (not a finger) drives it from here.
        self.publish_transition_progress(from, Some(false));
    }

    /// The event-pass timestamp (ms) for velocity tracking — the last frame time
    /// seen at paint, since the event pass carries no clock (see
    /// [`last_frame_time`](NavigatorWidget::last_frame_time)).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// The set of pages an input event can reach, as an index range into
    /// `self.pages` (ascending = bottom-to-top).
    ///
    /// **The single derivation of "reachable"**, read by both
    /// [`route_top`](Self::route_top) and
    /// [`semantics`](Widget::semantics) so input reach and the accessibility
    /// tree can never drift apart (rule R23 — *navigator semantics forwarding
    /// follows input routing, exactly*). Today the set is exactly
    /// `{ pages.last() }`; if non-modal overlays ever start passing input
    /// through, both sides widen together by construction.
    ///
    /// This is deliberately **narrower** than the painted range
    /// [`base_visible_index`](Self::base_visible_index) yields: a page under a
    /// transparent overlay is [`PageVisibility::Visible`] — painted, but routed
    /// no input — and is therefore *not* in this set.
    fn input_routed_pages(&self) -> std::ops::Range<usize> {
        self.pages.len().saturating_sub(1)..self.pages.len()
    }

    /// Route an event to the top page via the shared single-child router.
    ///
    /// Walks [`input_routed_pages`](Self::input_routed_pages) top-first,
    /// stopping at the first page that consumes the event — one page today.
    fn route_top(&mut self, ctx: &mut EventCtx<'_>, event: &InputEvent) -> EventResult {
        for i in self.input_routed_pages().rev() {
            if crate::authoring::route_event_single(&mut self.pages[i].pod, ctx, event)
                == EventResult::Handled
            {
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }

    /// The event body with an explicit timestamp so velocity math is deterministic
    /// in tests ([`Widget::event`] supplies the real paint-derived clock).
    ///
    /// Ordering: an active interactive swipe is driven first (it bypasses the
    /// mid-transition input block); otherwise a running transition suppresses all
    /// page routing; otherwise pointer events run the edge-swipe arm/steal
    /// machinery before falling through to normal top-page routing.
    fn event_at(&mut self, ctx: &mut EventCtx<'_>, event: &InputEvent, t_ms: f64) -> EventResult {
        // An in-progress interactive swipe owns the pointer stream.
        if self.edge.active {
            return self.drive_edge_swipe(ctx, event, t_ms);
        }
        // Input-blocking contract: while a non-interactive transition is
        // in flight, suppress ALL routing to pages.
        if self.transition.is_some() {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            // Focus-routed / scroll events route straight to the top page.
            return self.route_top(ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                // Arm an edge-swipe on a left-edge Down over a poppable stack
                // whose top page currently honours the gesture. The navigator
                // does NOT capture here (ScrollView precedent): the page
                // still sees the Down and may capture; a later steal sends the page
                // a synthetic Cancel. No buffering/re-dispatch — children see Down
                // first.
                // Only a primary press arms the swipe: a secondary press is a
                // context gesture, never the start of an interactive pop.
                self.edge.armed = crate::authoring::presses(p)
                    && self.swipe_armable()
                    && self.pages.len() > 1
                    && p.position.x <= EDGE_SWIPE_ZONE_DP;
                if self.edge.armed {
                    self.edge.down_start = p.position;
                    self.edge.tracker.clear();
                    self.edge.tracker.record(t_ms, p.position.x);
                }
                // R-B3-inner. A left-edge Down does not capture (above), so it
                // is forwarded through `route_top` unconditionally — a nested
                // navigator on the routed page's own `event_at` runs
                // underneath and may ALSO arm (both legitimately arm; nothing
                // is stolen yet). Record whatever armed below this navigator
                // into a fresh claim cell, read it back once routing returns
                // (`edge.inner_claimed`, consulted at the Move steal site),
                // and propagate the combined result into whatever cell is now
                // ambient — this navigator's own host, if any — so a third
                // nesting level defers too.
                let claim = Rc::new(Cell::new(false));
                let routed = with_swipe_claim(&claim, || self.route_top(ctx, event));
                self.edge.inner_claimed = claim.get();
                if let Some(host) = ambient_swipe_claim() {
                    host.set(self.edge.inner_claimed || self.edge.armed);
                }
                routed
            }
            PointerPhase::Move => {
                if !self.edge.armed {
                    return self.route_top(ctx, event);
                }
                self.edge.tracker.record(t_ms, p.position.x);
                let dx = p.position.x - self.edge.down_start.x;
                let dy = p.position.y - self.edge.down_start.y;
                if dx > TOUCH_SLOP && dx.abs() > dy.abs() {
                    // Decisive rightward horizontal drag → STEAL from the page —
                    // after re-validating everything the `Down` arm captured
                    // against, since a rebuild may have run between then and now:
                    //
                    // R-B3-inner: a nested navigator on the same `Down` armed too
                    // (`edge.inner_claimed`) — it is upstream of nobody in the
                    // routing order, so defer to it instead of stealing here.
                    if self.edge.inner_claimed {
                        self.edge.armed = false;
                        return self.route_top(ctx, event);
                    }
                    // BackPolicy/pop_swipe re-check (§4.1): a push applied between
                    // `Down` and now may have put a page on top this arm no longer
                    // honours (a DismissAnimated/Veto top, or a page-level
                    // `pop_swipe(false)` override).
                    //
                    // Depth re-check: a programmatic pop/replace applied at a
                    // rebuild between this arm's `Down` and now may have emptied
                    // the poppable stack. `begin_interactive_pop`'s only depth
                    // check is a debug-only `debug_assert!` (compiled out in
                    // release), so without this guard a stale arm could steal
                    // wrongly or pop the root page in a release build — draining
                    // the stack to zero pages. Either way, drop the stale arm and
                    // fall through to normal routing rather than stealing.
                    if self.pages.len() <= 1 || !self.swipe_armable() {
                        self.edge.armed = false;
                        return self.route_top(ctx, event);
                    }
                    let width = ctx.size().width.max(1.0);
                    let progress = (dx / width).clamp(0.0, 1.0);
                    self.begin_interactive_pop(progress);
                    self.edge.armed = false;
                    self.edge.active = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    EventResult::Handled
                } else if dy.abs() > TOUCH_SLOP || dx < -TOUCH_SLOP {
                    // Vertical dominance or a leftward drag: not an edge pop. Disarm
                    // and let the page own the gesture (e.g. a ScrollView child that
                    // starts in the edge zone but drags vertically still scrolls).
                    self.edge.armed = false;
                    self.edge.inner_claimed = false;
                    self.route_top(ctx, event)
                } else {
                    // Still within slop: keep observing, forward to the page.
                    self.route_top(ctx, event)
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                // An armed-but-never-stolen gesture just releases its arm; the page
                // owned the Down/Move/Up stream throughout. Clears the R-B3-inner
                // claim too, so a later independent swipe is not deferred against a
                // stale record from this one.
                self.edge.armed = false;
                self.edge.inner_claimed = false;
                self.route_top(ctx, event)
            }
        }
    }

    /// Pop the top page (the shared body of [`NavOp::Pop`] and a
    /// [`BackPolicy::Pop`] back request), delivering `result` to the popped
    /// page's pusher-registered callback. A pop of the last/root page is a safe
    /// no-op (the navigator always keeps one page; the result payload is
    /// dropped). Transitions are preserved: an animated page animates out and is
    /// torn down on settle. Returns the accumulated dirtiness.
    fn apply_pop(&mut self, result: PopResult, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.pages.len() > 1 {
            // The outgoing page's own focus link, read before `cancel_top` drops
            // it (see `top_pod_focused`).
            let outgoing_focused = self.top_pod_focused();
            self.cancel_top();
            // Disarm any pending edge-swipe: this pop shrinks the stack, so an arm
            // captured before it must not later steal an interactive pop against
            // the now-shallower stack (mirrors the `cancel_top` contract).
            self.edge.armed = false;
            self.edge.inner_claimed = false;
            let mut popped = self.pages.pop().expect("len checked > 1");
            let spec = popped.transition;
            if let Some(callback) = popped.on_result.take() {
                self.pending_results.push((callback, result));
                // Ask this frame's rebuild for a housekeeping pass so the callback
                // runs on the very frame the pop applied, with no input needed.
                // `apply_pop` is only ever reached through the op queue, whose
                // `enqueue` already raised this mark before the rebuild started —
                // idempotent, so a defensive re-raise here is free insurance
                // against a future caller reaching this method any other way.
                frust_core::mark_pending_result_flush();
            }
            // Full gate (`top_pod_focused`): the popped page's own link ANDed
            // with the rebuild-pass chain down to this navigator. A pop inside a
            // navigator that never held focus leaves an unrelated subtree's live
            // session alone.
            if outgoing_focused && ctx.has_focus() {
                self.needs_ime_clear = true;
            }
            if spec.is_animated() {
                // Keep the popped page alive & painted, animating out; torn down
                // on settle (a pop reverses its transition).
                self.start_transition(spec, true, Some(popped), ctx);
            } else {
                crate::authoring::teardown_child(&popped.view, &mut popped.pod, ctx);
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    /// Route a back press through the top page's [`BackPolicy`]:
    ///
    /// - [`Pop`](BackPolicy::Pop) → a normal [`apply_pop`](Self::apply_pop) (the
    ///   existing path, transitions preserved; a no-op at the root);
    /// - [`DismissAnimated`](BackPolicy::DismissAnimated) → increment the top
    ///   page's dismiss-signal generation (the page's own subtree observes it
    ///   and begins its exit, then pops itself); the stack is unchanged now, so
    ///   only `PAINT` is flagged (a repaint is needed for the page to observe the
    ///   bump);
    /// - [`Veto`](BackPolicy::Veto) → the press is *consumed* (the page claimed
    ///   it via `back_interest`, so it never reached the platform) but nothing
    ///   happens — no signal, no stack change, no dirtiness.
    fn apply_request_back(&mut self, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let policy = self.pages.last().map(|p| p.back).unwrap_or(BackPolicy::Pop);
        match policy {
            BackPolicy::Pop => self.apply_pop(PopResult::empty(), ctx),
            BackPolicy::DismissAnimated => {
                if let Some(top) = self.pages.last()
                    && let Some(signal) = &top.dismiss_signal
                {
                    // Bump exactly once per request — the page compares the shared
                    // generation against its last-seen value and stages its exit.
                    signal.set(signal.get().wrapping_add(1));
                }
                ChangeFlags::PAINT
            }
            // Consumed, but no visible change and no stack mutation.
            BackPolicy::Veto => ChangeFlags::NONE,
        }
    }

    /// Drain and apply the controller's queued ops (structural changes only),
    /// building/tearing down pods through `ctx`. Returns the accumulated dirtiness.
    fn apply_ops(&mut self, ops: Vec<NavOp<State>>, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        for op in ops {
            match op {
                NavOp::Push {
                    builder,
                    opaque,
                    on_result,
                    transition,
                    back,
                    dismiss_signal,
                    on_visibility,
                    route,
                    pop_swipe,
                } => {
                    let spec = self.effective_spec(transition);
                    // A push severs nothing, but it does *cover* the current top
                    // — and a covered page's session must go down with the
                    // keyboard, which is why the existing
                    // `push_clears_focused_field_ime_surface` behavior is
                    // deliberate. The COVERED page is therefore the outgoing pod
                    // here; read its link before `cancel_top` drops it (see
                    // `top_pod_focused`).
                    let covered_focused = self.top_pod_focused();
                    self.cancel_top();
                    // Disarm any pending edge-swipe: a structural stack mutation
                    // invalidates an arm captured against the pre-mutation stack
                    // (mirrors the capture/focus-clearing `cancel_top` contract).
                    self.edge.armed = false;
                    self.edge.inner_claimed = false;
                    // The incoming page becomes the top, so it is reachable iff
                    // this navigator is; its cell is live from before its own
                    // builder runs, so a nested navigator built in there reads a
                    // correct value on its very first `build`.
                    let reach = Rc::new(Cell::new(self.reachable()));
                    let (view, pod) = with_page_reach(&reach, || {
                        let view = builder();
                        let pod = crate::authoring::build_child(&view, ctx);
                        (view, pod)
                    });
                    self.pages.push(PageEntry {
                        builder,
                        view,
                        pod,
                        opaque,
                        on_result,
                        transition: spec,
                        back,
                        dismiss_signal,
                        // `None`, not `Current`: the end-of-`apply_ops`
                        // `publish_state` is what fires this page's opening
                        // `Current` observation.
                        visibility: None,
                        on_visibility,
                        reconciled_covered: false,
                        route,
                        pop_swipe,
                        reach,
                    });
                    // Full gate (`top_pod_focused`): the covered page's own link
                    // ANDed with the rebuild-pass chain down to this navigator.
                    // `ctx.has_focus()` is unchanged by the `build_child` above —
                    // that descent restores the chain on the way out.
                    if covered_focused && ctx.has_focus() {
                        self.needs_ime_clear = true;
                    }
                    // A push's leaving page (now at `len - 2`) stays in the stack;
                    // the transition keeps it painted (culling deferred to settle).
                    if spec.is_animated() && self.pages.len() >= 2 {
                        self.start_transition(spec, false, None, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                NavOp::Pop { result } => {
                    flags |= self.apply_pop(result, ctx);
                }
                NavOp::RequestBack => {
                    flags |= self.apply_request_back(ctx);
                }
                NavOp::Replace {
                    builder,
                    opaque,
                    transition,
                    route,
                } => {
                    let spec = self.effective_spec(transition);
                    // The replaced top is the outgoing pod; read its link before
                    // `cancel_top` drops it (see `top_pod_focused`).
                    let outgoing_focused = self.top_pod_focused();
                    self.cancel_top();
                    // Disarm any pending edge-swipe: replacing the top page
                    // invalidates an arm captured against the outgoing page
                    // (mirrors the `cancel_top` capture/focus-clearing contract).
                    self.edge.armed = false;
                    self.edge.inner_claimed = false;
                    // As the push arm: the replacement page is the new top.
                    let reach = Rc::new(Cell::new(self.reachable()));
                    let (view, pod) = with_page_reach(&reach, || {
                        let view = builder();
                        let pod = crate::authoring::build_child(&view, ctx);
                        (view, pod)
                    });
                    if let Some(top) = self.pages.last_mut() {
                        let entry = PageEntry {
                            builder,
                            view,
                            pod,
                            opaque,
                            on_result: None,
                            transition: spec,
                            // A replaced page pops on back like the root — an
                            // overlay uses `push_with_options`, never replace.
                            back: BackPolicy::Pop,
                            dismiss_signal: None,
                            visibility: None,
                            // A replace has no `PushOptions`, so the incoming
                            // page carries no observer (the outgoing one's dies
                            // with it — it is being torn down, not covered).
                            on_visibility: None,
                            reconciled_covered: false,
                            route,
                            // A replace carries no `ReplaceOptions::pop_swipe`
                            // (unspecced) — the replacement page defers to the
                            // navigator's own resolved default, same as a
                            // page pushed with no override.
                            pop_swipe: None,
                            reach: Rc::clone(&reach),
                        };
                        if spec.is_animated() {
                            // Stash the old top and animate the new one in over it
                            // (push-like direction); torn down on settle.
                            let old = std::mem::replace(top, entry);
                            self.start_transition(spec, false, Some(old), ctx);
                        } else {
                            crate::authoring::teardown_child(&top.view, &mut top.pod, ctx);
                            *top = entry;
                        }
                    } else {
                        // Defensive: an empty stack should not occur (build seeds
                        // the root page), but replace-into-empty pushes.
                        self.pages.push(PageEntry {
                            builder,
                            view,
                            pod,
                            opaque,
                            on_result: None,
                            transition: spec,
                            back: BackPolicy::Pop,
                            dismiss_signal: None,
                            visibility: None,
                            on_visibility: None,
                            reconciled_covered: false,
                            route,
                            pop_swipe: None,
                            reach,
                        });
                    }
                    // Full gate (`top_pod_focused`): the replaced page's own link
                    // ANDed with the rebuild-pass chain down to this navigator.
                    if outgoing_focused && ctx.has_focus() {
                        self.needs_ime_clear = true;
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        // Publish the (possibly changed) stack depth + back-interest so
        // `NavigatorController::can_pop`/`back_interest` reflect this batch of
        // ops.
        self.publish_state();
        flags
    }

    /// Lay out the two pages a transition involves (entering = top of stack,
    /// leaving = the retained page or the page below), returning the union size.
    /// Origins are reset to `ZERO`; paint applies the animated offset per frame.
    fn layout_transition(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let mut size = Size::ZERO;
        if let Some(entry) = self.pages.last_mut() {
            let child = entry.pod.layout_child(ctx, bc);
            entry.pod.set_origin(Point::ZERO);
            size = Size::new(size.width.max(child.width), size.height.max(child.height));
        }
        // Leaving page: retained (pop/replace) or the page below (push).
        let has_stashed = self
            .transition
            .as_ref()
            .map(|t| t.stashed.is_some())
            .unwrap_or(false);
        if has_stashed {
            if let Some(t) = self.transition.as_mut()
                && let Some(stashed) = t.stashed.as_mut()
            {
                let child = stashed.pod.layout_child(ctx, bc);
                stashed.pod.set_origin(Point::ZERO);
                size = Size::new(size.width.max(child.width), size.height.max(child.height));
            }
        } else {
            let n = self.pages.len();
            if n >= 2 {
                let child = self.pages[n - 2].pod.layout_child(ctx, bc);
                self.pages[n - 2].pod.set_origin(Point::ZERO);
                size = Size::new(size.width.max(child.width), size.height.max(child.height));
            }
        }
        bc.constrain(size)
    }

    /// Paint a transition frame: advance the driver, resolve per-page geometry,
    /// and paint both pages (clipped to the navigator area) in the order the
    /// direction dictates. Requests the next frame while running; flags `settled`
    /// (finalized on the next rebuild) once the driver reaches rest.
    fn paint_transition(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // LAZY driver resolution (mirrors `motion::switcher`'s deferred driver):
        // the first paint of a programmatic transition resolves `ThemeDefault`
        // timing + `reduce_motion` against the active `MotionScheme` — which the
        // `BuildCtx` that staged the transition could not reach — and rebuilds
        // the driver, preset, and settle spring before this frame advances. With
        // no theme threaded the `make_driver` fallback built at
        // `start_transition` (M3 defaults) stands (docs/CODE_STANDARDS.md).
        if let Some(spec) = self.transition.as_ref().and_then(|t| t.pending_spec) {
            if let Some(theme) = Theme::from_paint_ctx(ctx) {
                let resolved = resolve_spec(spec, Some(&theme.motion));
                let (driver, settle_spring) = make_driver(resolved.timing);
                if let Some(t) = self.transition.as_mut() {
                    t.driver = driver;
                    t.preset = resolved.preset;
                    t.settle_spring = settle_spring;
                }
            }
            if let Some(t) = self.transition.as_mut() {
                t.pending_spec = None;
            }
        }
        let (adv, is_pop, preset, has_stashed) = {
            let t = self.transition.as_mut().expect("transition present");
            let adv = t.driver.advance(ctx.frame_time());
            (adv, t.is_pop, t.preset, t.stashed.is_some())
        };
        // Publication point, immediately after the driver advanced: this is the
        // ONLY place progress moves for a programmatic transition, and it is what
        // makes a read from a widget painted AFTER the navigator frame-exact.
        // See `NavigatorController::transition`'s timing contract.
        self.publish_transition_progress(adv.value, None);
        let area = ctx.size();
        let nav_origin = ctx.origin();
        let (enter_layer, leave_layer) = resolve_layers(preset, adv.value, is_pop, area);

        // Shared-element ("hero") directives for this frame, derived from the
        // rects the *previous* frame captured (page layout is static during a
        // transition, so last frame's rects are the current resting geometry).
        // The morph rect is clamped-progress interpolated so a spatial-spring
        // overshoot past 1.0 never flips a hero dimension.
        let (enter_dirs, leave_dirs) =
            self.hero_directives(adv.value.clamp(0.0, 1.0), is_pop, nav_origin);

        // Clip every offset page to the navigator's own area — content that slides
        // off-screen must not bleed past the navigator (relevant when nested).
        scene.push_clip(nav_origin, area);
        // Paint order: a push paints leaving (below) then entering (on top); a pop
        // paints entering (revealed, below) then leaving (popped, on top). Each
        // page paints with a hero reporter installed so its tagged descendants
        // report their rects and the matched endpoint paints (or suppresses) the
        // morph. The overlay is painted by the hero on the page drawn LAST, so it
        // always lands above both page layers.
        let (new_entering, new_leaving);
        if is_pop {
            new_entering =
                self.paint_entering_heroes(ctx, scene, enter_layer, area, nav_origin, enter_dirs);
            new_leaving = self.paint_leaving_heroes(
                ctx,
                scene,
                leave_layer,
                area,
                nav_origin,
                has_stashed,
                leave_dirs,
            );
        } else {
            new_leaving = self.paint_leaving_heroes(
                ctx,
                scene,
                leave_layer,
                area,
                nav_origin,
                has_stashed,
                leave_dirs,
            );
            new_entering =
                self.paint_entering_heroes(ctx, scene, enter_layer, area, nav_origin, enter_dirs);
        }
        scene.pop_clip();

        // Store this frame's captured rects for next frame's directives.
        if let Some(t) = self.transition.as_mut() {
            t.hero_entering = new_entering;
            t.hero_leaving = new_leaving;
        }

        if adv.animating {
            ctx.request_frame();
        }
        if adv.done {
            // Settle: mark for finalize and request one more frame so the next
            // rebuild tears down the retained page and resumes culling.
            if let Some(t) = self.transition.as_mut() {
                t.settled = true;
            }
            ctx.request_frame();
        }
    }

    /// Compute this frame's per-page hero [`HeroDirective`]s from the rects the
    /// previous paint captured. A tag present on **both** pages is a matched
    /// shared element: the hero on the page painted last (on top — entering for
    /// a push, leaving for a pop) paints the morph at the interpolated absolute
    /// rect; its counterpart is suppressed. Returns `(entering, leaving)`
    /// directive maps.
    fn hero_directives(
        &self,
        p: f64,
        is_pop: bool,
        nav_origin: Point,
    ) -> (
        HashMap<String, HeroDirective>,
        HashMap<String, HeroDirective>,
    ) {
        let mut enter_dirs = HashMap::new();
        let mut leave_dirs = HashMap::new();
        let Some(t) = self.transition.as_ref() else {
            return (enter_dirs, leave_dirs);
        };
        for (tag, leaving_rect) in &t.hero_leaving {
            let Some(entering_rect) = t.hero_entering.get(tag) else {
                continue;
            };
            let interp = lerp_rect(*leaving_rect, *entering_rect, p);
            let dest =
                Rect::from_origin_size(nav_origin + interp.origin().to_vec2(), interp.size());
            let (painter, suppressed) = if is_pop {
                (&mut leave_dirs, &mut enter_dirs)
            } else {
                (&mut enter_dirs, &mut leave_dirs)
            };
            painter.insert(tag.clone(), HeroDirective::Morph { dest });
            suppressed.insert(tag.clone(), HeroDirective::Suppress);
        }
        (enter_dirs, leave_dirs)
    }

    /// Paint the entering page (always the stack top) with `layer`, a hero
    /// reporter installed, returning its captured page-local hero rects.
    fn paint_entering_heroes(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        layer: Layer,
        area: Size,
        nav_origin: Point,
        directives: HashMap<String, HeroDirective>,
    ) -> HashMap<String, Rect> {
        if let Some(entry) = self.pages.last_mut() {
            let reference = nav_origin + Vec2::new(layer.dx, layer.dy);
            paint_page_heroes(
                &mut entry.pod,
                ctx,
                scene,
                layer,
                area,
                reference,
                directives,
            )
        } else {
            HashMap::new()
        }
    }

    /// Paint the leaving page — the retained page (pop/replace) or the page below
    /// the new top (push) — with `layer`, a hero reporter installed, returning
    /// its captured page-local hero rects.
    #[allow(clippy::too_many_arguments)]
    fn paint_leaving_heroes(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        layer: Layer,
        area: Size,
        nav_origin: Point,
        has_stashed: bool,
        directives: HashMap<String, HeroDirective>,
    ) -> HashMap<String, Rect> {
        let reference = nav_origin + Vec2::new(layer.dx, layer.dy);
        if has_stashed {
            if let Some(t) = self.transition.as_mut()
                && let Some(stashed) = t.stashed.as_mut()
            {
                return paint_page_heroes(
                    &mut stashed.pod,
                    ctx,
                    scene,
                    layer,
                    area,
                    reference,
                    directives,
                );
            }
            HashMap::new()
        } else {
            let n = self.pages.len();
            if n >= 2 {
                paint_page_heroes(
                    &mut self.pages[n - 2].pod,
                    ctx,
                    scene,
                    layer,
                    area,
                    reference,
                    directives,
                )
            } else {
                HashMap::new()
            }
        }
    }
}

/// Paint one transition page with a hero reporter installed over its subtree,
/// returning the page-local rects its tagged descendants captured. `reference`
/// is the page's absolute top-left this frame (nav origin + the layer's
/// animated offset), subtracted from each reported rect so captures are stable
/// across the slide.
#[allow(clippy::too_many_arguments)]
fn paint_page_heroes(
    pod: &mut ChildPod,
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    layer: Layer,
    area: Size,
    reference: Point,
    directives: HashMap<String, HeroDirective>,
) -> HashMap<String, Rect> {
    let registry = RefCell::new(HeroFrames::new(reference, directives));
    ctx.with_hero_registry(&registry, |page_ctx| {
        paint_page_layer(pod, page_ctx, scene, layer, area);
    });
    registry.into_inner().into_captured()
}

/// Build the affine that scales uniformly by `scale` about the absolute point
/// `pivot` — the standard translate/scale/translate-back "scale about a point"
/// construction (mirrors `motion::switcher`'s `scale_about`).
fn scale_about(pivot: Point, scale: f64) -> Affine {
    Affine::translate((pivot.x, pivot.y))
        * Affine::scale(scale)
        * Affine::translate((-pivot.x, -pivot.y))
}

/// Paint one transition page: offset its pod origin by the layer's `dx`/`dy` (so
/// paint and hit-testing move together), then bracket its paint with a
/// `push_transform` scale (about the page's paint-area centre) and a `push_layer`
/// opacity when either differs from the identity — strict LIFO (transform outer,
/// opacity inner). The scale realises M3 fade-through's `0.92 → 1.0` incoming
/// scale-up ([`Layer::scale`](super::transition::Layer::scale));
/// every other preset leaves `scale == 1.0`, so the transform is skipped. Mirrors
/// `motion::switcher`'s `paint_staged_child`.
///
/// For the split-crossfade presets (M3SharedAxisX, M3FadeThrough, Glyph),
/// `resolve_layers` leaves at most one page visible at any instant — the
/// other is `alpha == 0`, and at the exact split instant both are. Rather
/// than rasterize an alpha-0 page under a zero-opacity layer — real GPU work
/// multiplied away to nothing — it paints into a [`DiscardScene`] sink
/// instead. The paint pass still has to run for its side effects (hero
/// rects reported through `PaintCtx::with_hero_registry`, animating
/// descendants advancing), so this is a redirect of the *scene*, not a skip of
/// the pass; no `push_layer`/`push_transform` bracket is needed since nothing
/// the sink records is ever composited.
fn paint_page_layer(
    pod: &mut ChildPod,
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    layer: Layer,
    area: Size,
) {
    pod.set_origin(Point::new(layer.dx, layer.dy));
    let alpha = layer.alpha.clamp(0.0, 1.0);

    if alpha <= 0.0 {
        let mut sink = DiscardScene;
        pod.paint_child(ctx, &mut sink);
        return;
    }

    let has_scale = (layer.scale - 1.0).abs() > f64::EPSILON;
    let has_alpha = alpha < 1.0;

    if has_scale {
        let origin = ctx.origin();
        let pivot = Point::new(origin.x + area.width / 2.0, origin.y + area.height / 2.0);
        scene.push_transform(scale_about(pivot, layer.scale));
    }
    if has_alpha {
        scene.push_layer(ctx.origin(), area, alpha);
    }
    pod.paint_child(ctx, scene);
    if has_alpha {
        scene.pop_layer();
    }
    if has_scale {
        scene.pop_transform();
    }
}

impl<State: 'static> View<State> for NavigatorView<State> {
    type Element = NavigatorWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NavigatorWidget<State> {
        // Publish liveness FIRST: the root page builder below can wire a nested
        // navigator (an app's inner navigator inside a root `overlay_host`'s
        // page), and the facade's back arbitration must already see this
        // navigator as mounted when that happens — the widget itself does not
        // exist until the end of this function.
        self.controller.mount();
        // Capture the hosting page's reach cell BEFORE installing our own root
        // page's — `ambient_page_reach` here names the page *this navigator*
        // lives on (`None` at the top level), which is what gates whether this
        // navigator may claim a back press at all (R23).
        let host_reach = ambient_page_reach();
        // Bind it on the controller too: that is where the R23 gate is applied,
        // live, by `NavigatorController::back_interest`.
        self.controller.bind_host_reach(host_reach.clone());
        // The root page is the only page, hence the routed one: its reach is
        // this navigator's own.
        let root_reach = Rc::new(Cell::new(host_reachable(host_reach.as_ref())));
        let (view, pod) = with_page_reach(&root_reach, || {
            let view = (self.initial)();
            let pod = crate::authoring::build_child(&view, ctx);
            (view, pod)
        });
        let mut widget = NavigatorWidget {
            pages: vec![PageEntry {
                builder: self.initial.clone(),
                view,
                pod,
                opaque: true,
                on_result: None,
                transition: self.default_transition,
                // The root page always pops on back (never an overlay policy).
                back: BackPolicy::Pop,
                dismiss_signal: None,
                visibility: None,
                on_visibility: self.root_visibility.clone(),
                reconciled_covered: false,
                route: self.root_route.clone(),
                // The root has no `PushOptions` to carry an override (mirrors
                // `root_visibility`/`root_route`'s shape) — it defers to the
                // navigator's own resolved default.
                pop_swipe: None,
                reach: root_reach,
            }],
            pending_results: Vec::new(),
            needs_ime_clear: false,
            default_transition: self.default_transition,
            transition: None,
            pop_swipe_enabled: self.resolve_pop_swipe(),
            edge: EdgeSwipe::new(),
            last_frame_time: FrameTime::ZERO,
            depth: Rc::clone(&self.controller.depth),
            back_interest: Rc::clone(&self.controller.back_interest),
            transition_state: Rc::clone(&self.controller.transition),
            mounted: Rc::clone(&self.controller.mounted),
            cull_covered_builds: self.cull_covered_builds,
            host_reach,
            route_stack: Rc::clone(&self.controller.route_stack),
            route_change: self.route_change.clone(),
        };
        // Apply any ops the app queued before the first frame.
        let ops = self.controller.drain();
        if !ops.is_empty() {
            widget.apply_ops(ops, ctx);
        }
        // Publish the initial (post-any-queued-ops) depth + back-interest so
        // `can_pop`/`back_interest` are authoritative from the first frame, even
        // if no ops ran.
        widget.publish_state();
        widget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut NavigatorWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // 0. Controller identity. `AnyView::rebuild` (crates/frust-core/src/view.rs)
        //    matches only the concrete view type (`NavigatorView<State>`), never
        //    controller identity — so a slot that gets rebuilt against a
        //    *different* `NavigatorController` reaches this `rebuild`, not
        //    `build`, unlike every other structural change. Left unhandled, the
        //    widget would keep draining ops from `self.controller` (the new one,
        //    correct) while publishing depth/back_interest/transition/mounted
        //    into the cells captured at `build` (the old one) — an ops/state
        //    split, and the old controller's mounted count would never return to
        //    0 (permanently "mounted", defeating the mounted-veto prune in
        //    `frust::back_glue`).
        //
        //    Re-bind rather than rebuild the element: swapping which controller
        //    drives a navigator is app-level misuse (idiomatic usage keeps one
        //    controller per `Component::State` for the view's whole life), but
        //    tearing down and rebuilding the retained page stack on top of that
        //    misuse would additionally blow away every page's widget state
        //    (`push_pop_preserves_page_widget_state`'s guarantee) for a
        //    consequence out of proportion to the mistake. Re-binding keeps the
        //    stack — and the app's data — intact; only the five published cells
        //    (`route_stack` joined the original four) move to point at the
        //    new controller.
        //
        //    Ordering: unmount the OLD controller through `element.mounted`
        //    (the cell still bound from the last build/rebind) BEFORE rebinding
        //    that field to the new controller's cell — otherwise the decrement
        //    would land on the wrong cell and the leak would just move rather
        //    than close. Mount the NEW controller only after every cell points
        //    at it, so a re-entrant read mid-rebind never sees a half-swapped
        //    widget.
        //
        //    An in-flight transition (`element.transition`, the widget's own
        //    retained animation state — never controller-owned) is left
        //    running untouched; only where its progress gets *published*
        //    moves. The new controller's `transition` cell starts at
        //    `TransitionState::default()` (inactive), so a chrome observer
        //    reading it through the very next frame after a mid-transition swap
        //    sees a one-frame-stale "at rest" snapshot — self-healing at the
        //    next paint (which republishes progress every frame a transition is
        //    active) or at `finalize_transition`, whichever comes first. Bounded
        //    and self-correcting, not a permanent split — the cost of swapping
        //    controllers mid-transition, which is already deep into misuse
        //    territory.
        if self.controller.id() != _prev.controller.id() {
            unmount_cell(&element.mounted);
            element.depth = Rc::clone(&self.controller.depth);
            element.back_interest = Rc::clone(&self.controller.back_interest);
            element.transition_state = Rc::clone(&self.controller.transition);
            element.mounted = Rc::clone(&self.controller.mounted);
            element.route_stack = Rc::clone(&self.controller.route_stack);
            self.controller.mount();
        }
        // Keep the widget's default transition in sync with the view so an app can
        // change it live (per-op overrides always win over it).
        element.default_transition = self.default_transition;
        // Refresh the edge-swipe enable flag from the view too (live-configurable).
        element.pop_swipe_enabled = self.resolve_pop_swipe();
        // Refresh the route-change observer too — navigator-wide (unlike
        // per-page `on_visibility`), so live-configurable exactly like the two
        // fields above.
        element.route_change = self.route_change.clone();
        // Same for the covered-build cull switch (default `false`).
        element.cull_covered_builds = self.cull_covered_builds;
        // Re-capture the hosting page's reach cell: this rebuild runs inside
        // whichever page's `with_page_reach` scope currently owns this
        // navigator, which is authoritative even if the subtree moved between
        // pages (or between a page and the top level) since `build`. Before the
        // ops drain, because `apply_ops` stamps `self.reachable()` onto any page
        // it builds.
        element.host_reach = ambient_page_reach();
        // Re-bind on the controller as well (`self.controller` is the CURRENT
        // one, so this is correct across a controller swap too) — the live R23
        // gate reads it there.
        self.controller.bind_host_reach(element.host_reach.clone());
        // Republish page reach IMMEDIATELY, not just from the end-of-rebuild
        // `publish_state`: this navigator's own reachability may have changed
        // this frame (an ancestor covered the page hosting it), and the pages'
        // cells must already say so before the reconcile loop below rebuilds a
        // navigator nested inside one of them — that nested navigator reads the
        // cell during its own rebuild, which happens strictly before this
        // rebuild's closing publish. Without this, reach propagated only one
        // nesting level per frame.
        element.publish_reach();
        // 1. Structural ops (view-driven): push/pop/replace the retained stack.
        let ops = self.controller.drain();
        if !ops.is_empty() {
            flags |= element.apply_ops(ops, ctx);
        }
        // 1b. Finalize a transition that settled during the previous paint: tear
        //     down its retained (leaving) page and resume normal culling.
        if element
            .transition
            .as_ref()
            .map(|t| t.settled)
            .unwrap_or(false)
        {
            element.finalize_transition(ctx);
            // Finalizing MUTATES the page stack — a cancelled interactive pop
            // pushes its stashed page back on — so publish explicitly here
            // rather than leaning on the end-of-rebuild publish below to cover
            // it by accident. This is also what fires the restored page's
            // `on_visibility` before the reconcile loop reads it.
            element.publish_state();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // 2. Reconcile every retained page by re-running its builder against live
        //    state — the pod, and thus the page's own widget state, is preserved;
        //    only the view descriptor is rebuilt.
        //
        //    Covered pages are included by default. Under
        //    `cull_covered_builds(true)` a page is skipped once it has ALREADY
        //    been reconciled while covered, so the frame it becomes covered still
        //    gets one final reconcile (see that builder's doc). Steps 1 and 1b
        //    above both ran before this loop, so a page revealed this frame is no
        //    longer `Covered` and rebuilds in the same pass that revealed it.
        let cull = element.cull_covered_builds;
        for index in 0..element.pages.len() {
            let covered = element.pages[index].visibility == Some(PageVisibility::Covered);
            let skip = cull && covered && element.pages[index].reconciled_covered;
            element.pages[index].reconciled_covered = covered;
            if skip {
                continue;
            }
            // Install this page's reach cell for the whole builder + subtree
            // reconcile: a nested navigator anywhere under it (the app's page
            // builder is where `frust::navigator` auto-wires, and the nested
            // `NavigatorView::rebuild` runs inside `rebuild_child`) reads it and
            // gates its own back interest on it — R23, structurally.
            let reach = Rc::clone(&element.pages[index].reach);
            flags |= with_page_reach(&reach, || {
                let entry = &mut element.pages[index];
                let next_view = (entry.builder)();
                let child_flags =
                    crate::authoring::rebuild_child(&entry.view, &next_view, &mut entry.pod, ctx);
                entry.view = next_view;
                child_flags
            });
        }
        // Republish depth + back-interest at rebuild time — the rebuild-time
        // refresh contract the back handler relies on. A settled-transition
        // finalize (step 1b) above can change the stack, so publish once more
        // here after `apply_ops` already did.
        element.publish_state();
        flags
    }

    fn teardown(&self, element: &mut NavigatorWidget<State>, ctx: &mut BuildCtx<'_>) {
        // Tear down a transition's retained (leaving) page first, then the stack.
        element.finalize_transition(ctx);
        for entry in &mut element.pages {
            crate::authoring::teardown_child(&entry.view, &mut entry.pod, ctx);
        }
        // This navigator has left the tree: drop the liveness `mount()` count
        // this widget holds. Decrement through `element.mounted` — the cell
        // `build`/the last controller-swap `rebuild` bound — rather than calling
        // `self.controller.unmount()`. `self.controller` is merely whichever
        // controller *this* view instance happens to carry; after a swap it is
        // already the NEW controller, which `rebuild`'s swap handling already
        // mounted for a widget still alive. Unmounting through `self.controller`
        // here would double-unmount the new one (permanently `mounted() ==
        // false` even while the app still holds it elsewhere) and leave the OLD
        // one's earlier `rebuild`-time unmount as the only correction it ever
        // got — the mount/unmount pair is only provably balanced when both ends
        // read the same cell, which `element.mounted` guarantees regardless of
        // how many times the controller was swapped underneath this widget.
        unmount_cell(&element.mounted);
    }
}

impl<State: 'static> Widget for NavigatorWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // While a transition runs, both involved pages must be laid out (culling
        // is deferred to settle — Flutter opaque-route parity).
        if self.transition.is_some() {
            return self.layout_transition(ctx, bc);
        }
        // Lay out only the visible range (topmost opaque page + any transparent
        // pages above it); covered pages keep their retained widgets but are not
        // laid out while covered (re-laid-out on the next frame once revealed).
        // Constraints pass through unchanged — a full-screen page returns
        // `bc.max()`; the navigator sizes to the largest visible page.
        let start = self.base_visible_index();
        let mut size = Size::ZERO;
        for entry in &mut self.pages[start..] {
            let child = entry.pod.layout_child(ctx, bc);
            entry.pod.set_origin(Point::ZERO);
            size = Size::new(size.width.max(child.width), size.height.max(child.height));
        }
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Record the shared frame clock so the between-frames event pass (which
        // carries no clock) has a timestamp for edge-swipe velocity tracking.
        self.last_frame_time = ctx.frame_time();
        if self.transition.is_some() {
            // Animated page switch: paint both involved pages with per-frame
            // offsets/opacity and drive the transition off the frame clock.
            self.paint_transition(ctx, scene);
        } else {
            // Paint the visible range bottom-to-top: only the topmost opaque page
            // (and any transparent pages above it) — fully-covered pages are culled.
            let start = self.base_visible_index();
            for entry in &mut self.pages[start..] {
                entry.pod.paint_child(ctx, scene);
            }
        }
        // Deterministic IME hide after a stack mutation: publish a cleared surface
        // so the platform keyboard drops immediately rather than waiting for the
        // lazy event-pass convergence. `RenderRoot::paint` only accepts this while
        // focus is (still) active — exactly the stale-focus window a switch opens.
        if self.needs_ime_clear {
            ctx.publish_ime_state(cleared_ime_state());
            self.needs_ime_clear = false;
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Flush pop-result callbacks queued during the rebuild — this is the
        // first point after a pop where the erased app state is in scope. Runs
        // for every event kind, including the `Housekeeping` broadcast the
        // rebuild dispatches for exactly this purpose, so a result never waits on
        // user input that may never arrive.
        if !self.pending_results.is_empty() {
            let pending = std::mem::take(&mut self.pending_results);
            let state = ctx.state_mut::<State>();
            for (callback, result) in pending {
                callback(state, result);
            }
        }
        if event.is_broadcast() {
            // A broadcast is not user input, so none of `event_at`'s machinery
            // applies: it must not drive an in-flight edge swipe, must not be
            // swallowed by the mid-transition input block, and must not be
            // narrowed to `input_routed_pages` — a nested navigator on a *covered*
            // page can have queued a result too, and it is entitled to the same
            // same-frame flush. Forward to every page, consume nothing.
            //
            // This is deliberately wider than R23's input/semantics reach and does
            // not weaken it: R23 governs what a *user* can activate, and a
            // housekeeping pass activates nothing — every widget below either has
            // deferred work of its own to run or ignores it outright.
            for entry in &mut self.pages {
                crate::authoring::route_event_single(&mut entry.pod, ctx, event);
            }
            return EventResult::Ignored;
        }
        // The rest of the event body (edge-swipe arm/steal/drive + the
        // mid-transition input block + top-page routing) runs against the
        // paint-derived event-pass clock. See [`event_at`](Self::event_at).
        //
        // Input-blocking contract (STRICT): while a non-interactive transition is
        // in flight, `event_at` suppresses ALL routing to pages — a mid-transition
        // `Down` reaches no page and records no `active`/focus path (the involved
        // pages' captures were already synthetically cancelled at transition start
        // via `cancel_top`). An interactive edge-swipe is the deliberate
        // exception: it drives a held transition and keeps receiving its own
        // pointer stream.
        let t_ms = self.event_time_ms();
        self.event_at(ctx, event, t_ms)
    }

    /// **R23 — semantics forwarding follows input routing, exactly.**
    ///
    /// A transparent container (like [`Stack`](crate::Stack)): the navigator
    /// contributes **no node of its own** and forwards
    /// [`ChildPod::semantics_child`] for exactly the pages
    /// [`input_routed_pages`](Self::input_routed_pages) says an input event
    /// could reach — today `{ pages.last() }`. Every other page is **omitted**:
    /// no node, no recursion. Offering a screen-reader user a control they
    /// physically cannot activate is worse than not offering it, so the two
    /// reaches are derived from one function rather than kept in sync by hand.
    ///
    /// # This omits more than paint culling does
    ///
    /// A page under a *transparent* overlay is [`PageVisibility::Visible`] —
    /// still painted — yet it is omitted here, because R23 tracks **input
    /// routing**, not painting, and [`route_top`](Self::route_top) routes only
    /// to the top page. The divergence is deliberate: it is what makes a modal
    /// modal to assistive technology for free (the page beneath a dialog is
    /// already inert to a finger). The topmost transparent page — the dialog
    /// itself — *is* forwarded, since it is `pages.last()`.
    ///
    /// # Why omission and not an accesskit flag
    ///
    /// Not `hidden`: it would need a synthesized per-page wrapper node to carry
    /// the flag (churning node ids on every navigation for nodes that exist
    /// only to say "ignore me"), it would publish the **stale bounds** of a
    /// covered page that `layout` skipped, and whether every platform adapter
    /// honours the flag is unverified — omission needs no such trust. Not
    /// `clips_children`, which asserts `overflow: hidden` and would be simply
    /// false here. Not `modal`, which is the right flag but belongs on the
    /// dialog widget: the navigator does not know a page is a dialog and cannot
    /// infer it from [`BackPolicy`] (most shipped modals push through
    /// `push_transparent_for_result` and so take the default
    /// [`BackPolicy::Pop`]).
    ///
    /// # During a transition
    ///
    /// No special case: `pages.last()` is the *destination* page for a push,
    /// pop and replace alike, and its mid-flight bounds are the animated pod
    /// origins `paint` is using — consistent with the screen and
    /// self-correcting within one transition. Input is *fully* suppressed
    /// mid-transition ([`event_at`](Self::event_at)), so a screen reader
    /// activating a node during those ≤340ms hits exactly the same suppression
    /// a finger would.
    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The routed set is always the top of the settled stack — i.e. exactly
        // the pages `visibility_of` calls `Current`. Pinned here so a future
        // change to either derivation trips in debug rather than silently
        // widening the accessibility tree past the input reach.
        debug_assert!(
            self.input_routed_pages()
                .all(|i| self.visibility_of(i) == PageVisibility::Current),
            "R23: the input-routed page set must be exactly the Current page(s)"
        );
        for i in self.input_routed_pages() {
            self.pages[i].pod.semantics_child(ctx);
        }
    }

    // Every RETAINED page, not just the ones input or semantics reach:
    // an inspector's job is to show what the tree holds, including a
    // covered page and the stashed page of a running transition.
    crate::authoring::visit_children!(pages, transition);
}

/// The cleared/inactive IME surface the navigator publishes after a page switch so
/// the platform keyboard hides deterministically (see [`NavigatorWidget::paint`]).
///
/// `active: false` is what makes this a **session release**, not a surface
/// refresh: `RenderRoot::paint`'s take path reads the inactive flag as "the
/// focus session is over" and clears `focus_active` *and* the stored surface
/// (storing `None`, never this value), so a popped page's focus cannot outlive
/// the widget that held it. The rest of the fields are the empty/no-selection
/// form and are never read by any shell for an inactive surface — both mobile
/// bridges serialise `None` to the byte-identical inactive JSON — but they stay
/// spelled out so the value is a valid, self-describing `ImeState` on its own.
fn cleared_ime_state() -> ImeState {
    ImeState {
        active: false,
        editing: EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: None,
        content_type: Default::default(),
    }
}

#[cfg(test)]
#[path = "navigator_tests/mod.rs"]
mod tests;
