//! The navigator core (Phase 6b, task 02): a retained page stack with imperative
//! push/pop/replace, per-page result callbacks, opaque-page paint culling, and
//! test-pinned capture/focus/IME page-switch semantics. Instant switches only —
//! transitions are task 03.
//!
//! # Shape
//!
//! [`navigator`] is the app-facing view fn (the snake_case spelling, like
//! [`scroll_view`](crate::scroll_view)/[`text_input`](crate::text_input)):
//! `navigator(controller, initial_page_builder)` produces a [`NavigatorView`]
//! whose retained [`NavigatorWidget`] owns a `Vec` of page entries. The
//! **[`NavigatorController`]** is the app-state handle — a cloneable
//! `Rc<RefCell<…>>` the app keeps in its `Component::State`; it *records requested
//! ops* (`push`/`pop`/`replace`), which the widget *applies at rebuild* (never
//! self-mutating mid-event). This mirrors the controlled-component philosophy the
//! interactive widgets follow (see `docs/CODE_STANDARDS.md`, "Controlled
//! components never self-mutate").
//!
//! # Op application is view-driven (at rebuild), not event-driven
//!
//! Structural ops are drained and applied in [`NavigatorView::rebuild`] (a
//! `BuildCtx` pass), *not* inside `NavigatorWidget::event`: building a new page
//! pod ([`crate::build_child`]) and tearing a popped one down
//! ([`crate::teardown_child`]) both need a `BuildCtx`, and a rebuild always runs
//! every frame so a *programmatic* push/pop (from a background task, with no
//! triggering event) still lands. On every stack mutation the widget then applies
//! the explicit page-switch contract the structural-rebuild machinery does not
//! cover for a hand-managed stack: (a) it cancels an in-flight capture on the
//! outgoing top ([`crate::cancel_pod`]'s synthetic-`Cancel`), (b) clears its focus
//! flag, and (c) publishes a *cleared* IME surface on the next paint so the
//! platform keyboard hides deterministically rather than waiting for the lazy
//! event-pass convergence `RenderRoot` otherwise relies on.
//!
//! A [`pop`](NavigatorController::pop_with_result) result destined for a
//! pusher-registered `on_result` callback needs `&mut State` — which a rebuild
//! (`BuildCtx`) does not carry — so the callback is queued at rebuild and flushed
//! at the start of the next [`NavigatorWidget::event`] pass, where the erased
//! app state is in scope. See [`NavigatorController::push_for_result`].
//!
//! # Paint culling (Flutter opaque-route parity)
//!
//! Only the topmost **settled opaque** page (and any transparent pages stacked
//! above it) is laid out and painted; pages fully covered by an opaque page keep
//! their retained widgets (so their state survives) but are neither laid out nor
//! painted while covered. Layout runs unconditionally every frame, so a page
//! revealed by a pop is re-laid-out and correct on the very next frame — the same
//! relayout-every-frame invariant the theme path leans on.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EditingState, EventCtx, EventResult,
    FrameTime, HeroDirective, HeroFrames, ImeState, InputEvent, LayoutCtx, PaintCtx, PaintScene,
    PointerPhase, SpringDesc, TOUCH_SLOP, VelocityTracker, View, Widget,
};
use kurbo::{Point, Rect, Size, Vec2};

use super::transition::{
    Layer, PageTransition, TransitionDriver, TransitionSpec, lerp_rect, make_driver,
    resolve_layers, settle_driver,
};

// --- Edge-swipe tuning constants (see per-constant approximation notes) ------

/// Left-edge activation zone width for the interactive pop-swipe, in logical px.
///
/// **Community-approximate** (phase-6b refuted-claims ledger #2): UIKit's
/// `interactivePopGestureRecognizer` edge zone is not a published constant;
/// ~20dp is the value the community-reverse-engineered reimplementations
/// converge on. A `Down` at `x <= EDGE_SWIPE_ZONE_DP` (with a poppable stack)
/// arms the gesture.
const EDGE_SWIPE_ZONE_DP: f64 = 20.0;

/// Progress past which a *released* edge-swipe completes the pop; at or below
/// it, the pop cancels and the page springs back.
///
/// The standard halfway commit point — Flutter's `CupertinoPageRoute` uses the
/// same 0.5 threshold for its interactive back gesture.
const EDGE_SWIPE_COMMIT_PROGRESS: f64 = 0.5;

/// Release x-velocity (logical px/s) past which an edge-swipe completes the pop
/// regardless of how far it dragged (a fast flick from near the edge still
/// pops).
///
/// **Community-approximate**: matches Flutter's Cupertino commit velocity
/// (~1300 pt/s); UIKit's exact interactive-pop fling threshold is private.
const EDGE_SWIPE_FLING_VELOCITY: f64 = 1300.0;

/// A page builder: a cheap closure that produces the page's view, re-run every
/// rebuild so a retained page's content still reconciles against live app state
/// (the pod, and thus the page's internal widget state, is preserved across the
/// rebuild — only the view descriptor is rebuilt).
pub type PageBuilder<State> = Rc<dyn Fn() -> AnyView<State>>;

/// A pusher-registered result callback: invoked with `&mut State` when the page it
/// was registered against is popped, carrying the [`PopResult`] the pop supplied.
pub type ResultCallback<State> = Rc<dyn Fn(&mut State, PopResult)>;

/// The value a [`pop`](NavigatorController::pop_with_result) hands back to the
/// pusher's [`ResultCallback`], type-erased so a page can return any `'static`
/// payload (mirroring Flutter's `Navigator.pop(result)` → `push(...).then(...)`).
///
/// Empty by default ([`PopResult::empty`]); recover a typed payload with
/// [`PopResult::take`].
pub struct PopResult(Option<Box<dyn Any>>);

impl PopResult {
    /// A result carrying no payload (a plain back-navigation).
    pub fn empty() -> Self {
        PopResult(None)
    }

    /// A result carrying `value`, recoverable by the pusher with
    /// [`PopResult::take`].
    pub fn of<T: Any>(value: T) -> Self {
        PopResult(Some(Box::new(value)))
    }

    /// Whether this result carries no payload.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Recover the payload as `T`, consuming the result. `None` if the result was
    /// empty or carries a different concrete type.
    pub fn take<T: Any>(self) -> Option<T> {
        self.0
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }
}

impl std::fmt::Debug for PopResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopResult")
            .field("has_payload", &self.0.is_some())
            .finish()
    }
}

/// One queued navigation op, recorded by the [`NavigatorController`] and drained
/// (in order) by [`NavigatorView::rebuild`].
enum NavOp<State: 'static> {
    /// Push a new page on top of the stack.
    Push {
        builder: PageBuilder<State>,
        opaque: bool,
        on_result: Option<ResultCallback<State>>,
        /// Per-op transition override (`None` → the navigator's default).
        transition: Option<TransitionSpec>,
    },
    /// Pop the top page (never the last/root page), delivering `result` to the
    /// popped page's pusher-registered callback. A pop *reverses* the popped
    /// page's own stored transition (no override slot).
    Pop { result: PopResult },
    /// Replace the top page in place.
    Replace {
        builder: PageBuilder<State>,
        opaque: bool,
        /// Per-op transition override (`None` → the navigator's default).
        transition: Option<TransitionSpec>,
    },
}

/// The app-state handle to a [`navigator`]: a cloneable op queue an app keeps in
/// its `Component::State` and drives with [`push`](Self::push)/[`pop`](Self::pop)/
/// [`replace`](Self::replace). Every clone shares one queue (`Rc`), so the handle
/// the view carries and the handle event handlers call are the same.
///
/// Ops are *recorded*, not applied — the [`NavigatorWidget`] drains and applies
/// them at its next rebuild (see the [module docs](self)).
pub struct NavigatorController<State: 'static> {
    ops: Rc<RefCell<Vec<NavOp<State>>>>,
}

impl<State: 'static> Clone for NavigatorController<State> {
    fn clone(&self) -> Self {
        Self {
            ops: Rc::clone(&self.ops),
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
        }
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
    /// [`PageTransition::SlideUp`] for a bottom sheet — see
    /// [`crate::material::dialog`]/[`crate::material::sheet`]'s usage sketch),
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

    /// Shared push-op construction every `push*` method above funnels through —
    /// the five public variants differ only in which of `opaque`/`on_result`/
    /// `transition` they fix vs. expose.
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

    /// Replace the top page in place with an opaque page built by `builder`,
    /// using the navigator's default transition.
    pub fn replace(&self, builder: impl Fn() -> AnyView<State> + 'static) {
        self.enqueue(NavOp::Replace {
            builder: Rc::new(builder),
            opaque: true,
            transition: None,
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
        });
    }

    fn enqueue(&self, op: NavOp<State>) {
        self.ops.borrow_mut().push(op);
    }

    /// Take the queued ops (leaving the queue empty). Called by
    /// [`NavigatorView::rebuild`]/`build`.
    fn drain(&self) -> Vec<NavOp<State>> {
        std::mem::take(&mut *self.ops.borrow_mut())
    }
}

/// A declarative navigator. See the [module docs](self).
pub struct NavigatorView<State: 'static> {
    controller: NavigatorController<State>,
    initial: PageBuilder<State>,
    /// The transition applied to a push/replace that supplies no per-op override.
    /// Defaults to [`TransitionSpec::NONE`] (instant switches — the task-02
    /// behavior).
    default_transition: TransitionSpec,
    /// Explicit override for the interactive edge-swipe back gesture (task 05).
    /// `None` derives it from the default transition preset — on for
    /// [`PageTransition::IosPush`], off otherwise.
    pop_swipe: Option<bool>,
}

impl<State: 'static> NavigatorView<State> {
    /// Set the default page transition applied to every push/replace that does
    /// not carry its own [`push_with`](NavigatorController::push_with)/
    /// [`replace_with`](NavigatorController::replace_with) override.
    pub fn transition(mut self, spec: TransitionSpec) -> Self {
        self.default_transition = spec;
        self
    }

    /// Explicitly enable or disable the interactive edge-swipe back gesture
    /// (task 05), overriding the preset-derived default (on for
    /// [`PageTransition::IosPush`], off otherwise). The gesture pops the top page
    /// with a left-edge drag: drag progress reverses the popped page's transition,
    /// and release completes or cancels the pop by progress/velocity.
    pub fn pop_swipe(mut self, enabled: bool) -> Self {
        self.pop_swipe = Some(enabled);
        self
    }

    /// Resolve whether the edge-swipe gesture is enabled: the explicit override if
    /// set, else on for the iOS-push preset (the transition the swipe is designed
    /// around) and off for every other default.
    fn resolve_pop_swipe(&self) -> bool {
        self.pop_swipe
            .unwrap_or(self.default_transition.preset == PageTransition::IosPush)
    }
}

/// Build a [`NavigatorView`] driven by `controller`, whose initial (root) page is
/// produced by `initial`. The app-facing entry point (see [module docs](self)).
pub fn navigator<State: 'static>(
    controller: &NavigatorController<State>,
    initial: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    NavigatorView {
        controller: controller.clone(),
        initial: Rc::new(initial),
        default_transition: TransitionSpec::NONE,
        pop_swipe: None,
    }
}

/// One retained page in the [`NavigatorWidget`]'s stack: its builder (re-run each
/// rebuild), the last view it produced (for reconciliation), the retained child
/// pod, its opacity, and the pusher's result callback (fired when this page pops).
struct PageEntry<State: 'static> {
    builder: PageBuilder<State>,
    view: AnyView<State>,
    pod: ChildPod,
    opaque: bool,
    on_result: Option<ResultCallback<State>>,
    /// The transition this page was pushed/replaced with — *reversed* when the
    /// page is later popped (a pop animates the popped page's own transition
    /// backwards, Flutter-parity: a route carries its transition).
    transition: TransitionSpec,
}

/// The single in-flight page transition a [`NavigatorWidget`] owns (Flutter
/// parity: created per push/pop, disposed on settle). Pairs the progress
/// [`TransitionDriver`] with the retained *leaving* page, when the op removed it
/// from the stack (pop/replace); a push's leaving page stays in the stack below
/// the new top, so `stashed` is `None` there.
struct ActiveTransition<State: 'static> {
    /// Drives `0.0..=1.0`; advanced from `PaintCtx::frame_time` during paint.
    driver: TransitionDriver,
    /// The visual preset (slide/fade/parallax geometry).
    preset: PageTransition,
    /// Direction: `true` reverses the horizontal motion + paint order (a pop).
    is_pop: bool,
    /// The removed page retained until settle (pop/replace). `None` for a push,
    /// whose leaving page is still in the stack at `len - 2`.
    stashed: Option<PageEntry<State>>,
    /// The spring a manual [`settle`](NavigatorWidget::settle_transition) uses
    /// when the timing mode is duration-based (a duration has no spring).
    settle_spring: SpringDesc,
    /// Set by paint when the driver reaches rest; the next rebuild finalizes the
    /// transition (tears down `stashed`, resumes culling).
    settled: bool,
    /// This transition is being driven by an interactive edge-swipe (task 05):
    /// its progress is `Held` by the drag, then settled on release. An
    /// interactive pop stashed the top page *without* queuing its result
    /// callback (a swipe may still cancel), so finalize does the completion
    /// bookkeeping the [`NavOp::Pop`] path did eagerly.
    interactive: bool,
    /// Set when an interactive pop was *cancelled* (settled toward `0.0`): finalize
    /// pushes the stashed page back onto the stack instead of tearing it down (the
    /// page was never really popped). See [`NavigatorWidget::finalize_transition`].
    restore_on_finalize: bool,
    /// Shared-element ("hero") state (task 07). Page-local rects of the tagged
    /// heroes discovered on the **leaving** page during the previous transition
    /// paint, keyed by tag. `layout`/`paint` capture these each frame; the next
    /// frame reads them to place the morph overlay. Empty until the first paint
    /// discovers any (so the morph starts a frame into the flight — the rects
    /// are static page layout, so the delay is invisible).
    hero_leaving: HashMap<String, Rect>,
    /// Page-local hero rects discovered on the **entering** page — the morph
    /// target endpoint. See [`hero_leaving`](ActiveTransition::hero_leaving).
    hero_entering: HashMap<String, Rect>,
}

/// The interactive edge-swipe gesture state (task 05). Mirrors
/// [`ScrollWidget`](crate::ScrollWidget)'s arm/steal model: `armed` on a
/// left-edge `Down`, promoted to `active` (an interactive pop in flight) once a
/// decisive horizontal drag steals the gesture from the page.
struct EdgeSwipe {
    /// A left-edge `Down` on a poppable stack armed the gesture, but the slop has
    /// not yet been crossed. Disarmed by a vertical/leftward drag or `Up`.
    armed: bool,
    /// The gesture stole from the page and is driving a held pop transition; the
    /// navigator owns the pointer stream until release.
    active: bool,
    /// The `Down` position the drag delta is measured from.
    down_start: Point,
    /// Trailing-window x-velocity tracker for the release fling decision.
    tracker: VelocityTracker,
}

impl EdgeSwipe {
    fn new() -> Self {
        Self {
            armed: false,
            active: false,
            down_start: Point::ZERO,
            tracker: VelocityTracker::new(),
        }
    }
}

/// The retained widget for a [`NavigatorView`]: owns the page stack and applies
/// the [`NavigatorController`]'s queued ops at rebuild. See the [module docs](self).
pub struct NavigatorWidget<State: 'static> {
    pages: Vec<PageEntry<State>>,
    /// Pop-result callbacks awaiting `&mut State` — flushed at the start of the
    /// next [`event`](NavigatorWidget::event) pass.
    pending_results: Vec<(ResultCallback<State>, PopResult)>,
    /// Set on every stack mutation; the next paint publishes a cleared IME surface
    /// and clears this, so the platform keyboard hides deterministically.
    needs_ime_clear: bool,
    /// The navigator's default transition (per-op overrides win). Refreshed from
    /// the view on rebuild so an app can change it live.
    default_transition: TransitionSpec,
    /// The single in-flight transition, if any (task 03). `None` between
    /// transitions — the common case, where paint/layout cull normally.
    transition: Option<ActiveTransition<State>>,
    /// Whether the interactive edge-swipe back gesture is enabled (task 05).
    /// Resolved from the view each rebuild — default-on for the iOS-push preset,
    /// or explicitly via [`NavigatorView::pop_swipe`].
    pop_swipe_enabled: bool,
    /// The in-progress edge-swipe gesture state (task 05).
    edge: EdgeSwipe,
    /// The most recent frame time seen during [`paint`](NavigatorWidget::paint),
    /// reused as the event-pass timestamp for velocity tracking — the event pass
    /// carries no clock of its own (spec §8 provides time only at paint). The
    /// same seam [`ScrollWidget`](crate::ScrollWidget) uses.
    last_frame_time: FrameTime,
}

impl<State: 'static> NavigatorWidget<State> {
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

    /// Cancel any in-flight capture and clear the focus flag on the *current* top
    /// page — the page being covered/replaced/popped by a stack mutation.
    ///
    /// Capture unwinds via [`crate::cancel_pod`]'s synthetic `Cancel` (the outgoing
    /// widget's state machine must not fire on a later `Up`); focus is a reflected
    /// pod flag, so clearing it is enough (no widget-internal blur to drive) — the
    /// same asymmetry the container reconcilers document.
    fn cancel_top(&mut self) {
        if let Some(top) = self.pages.last_mut() {
            if top.pod.is_active() {
                crate::cancel_pod(&mut top.pod);
                top.pod.set_active(false);
            }
            if top.pod.is_focused() {
                top.pod.set_focused(false);
            }
        }
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
        });
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
                    }
                    crate::teardown_child(&stashed.view, &mut stashed.pod, ctx);
                }
            }
        }
    }

    /// Task 05 seam: pin the active transition's progress to `p` (an edge-swipe
    /// drag holds it here between frames). No-op if no transition is active.
    ///
    /// The gesture that drives this lives in task 05; the navigator supplies the
    /// held-progress driver state a swipe manipulates.
    pub fn set_transition_progress(&mut self, p: f64) {
        if let Some(t) = self.transition.as_mut() {
            t.driver = TransitionDriver::Held { value: p };
            t.settled = false;
        }
    }

    /// Task 05 seam: release the active transition into a spring settle toward
    /// `1.0` (non-negative `velocity`) or `0.0` (negative). No-op if no transition.
    ///
    /// Note: a settle toward `0.0` runs the *visual* reversal, but restoring the
    /// stack (un-popping the retained page on a cancelled pop) is task 05's
    /// responsibility — this seam only drives the progress driver.
    pub fn settle_transition(&mut self, velocity: f64) {
        if let Some(t) = self.transition.as_mut() {
            let from = t.driver.value();
            let target = if velocity >= 0.0 { 1.0 } else { 0.0 };
            t.driver = settle_driver(t.settle_spring, from, velocity, target);
            t.settled = false;
        }
    }

    /// The event-pass timestamp (ms) for velocity tracking — the last frame time
    /// seen at paint, since the event pass carries no clock (see
    /// [`last_frame_time`](NavigatorWidget::last_frame_time)).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Steal the gesture from the top page into an interactive pop (task 05).
    ///
    /// Mirrors the [`NavOp::Pop`] animated-pop structure but built entirely in the
    /// event pass (no `BuildCtx`): the top page's in-flight capture is
    /// synthetically cancelled ([`cancel_top`](Self::cancel_top) — the covered
    /// widget's press machine must not fire on a later `Up`), then the page is
    /// popped into a **held** pop transition the drag drives via
    /// [`set_transition_progress`](Self::set_transition_progress). Building/tearing
    /// pods is deferred: the stashed page's teardown (or restore) happens at
    /// finalize, which runs at rebuild with a `BuildCtx` in scope.
    ///
    /// The pusher's result callback is intentionally *not* queued here — a swipe
    /// may still cancel; it fires only if the pop later completes (see
    /// [`finalize_transition`](Self::finalize_transition)).
    fn begin_interactive_pop(&mut self, initial_progress: f64) {
        debug_assert!(self.pages.len() > 1, "steal requires a poppable stack");
        debug_assert!(
            self.transition.is_none(),
            "steal requires no active transition"
        );
        self.cancel_top();
        let popped = self.pages.pop().expect("depth > 1 checked before steal");
        let spec = popped.transition;
        // The swipe animates the popped page's own preset; a page pushed without an
        // animated transition still swipes with the iOS-push geometry (a swipe is
        // inherently an iOS-style interaction). The timing only supplies the
        // fallback settle spring — the drag itself holds progress.
        let preset = if spec.is_animated() {
            spec.preset
        } else {
            PageTransition::IosPush
        };
        let (_driver, settle_spring) = make_driver(spec.timing);
        self.transition = Some(ActiveTransition {
            driver: TransitionDriver::Held {
                value: initial_progress.clamp(0.0, 1.0),
            },
            preset,
            is_pop: true,
            stashed: Some(popped),
            settle_spring,
            settled: false,
            interactive: true,
            restore_on_finalize: false,
            hero_leaving: HashMap::new(),
            hero_entering: HashMap::new(),
        });
        self.needs_ime_clear = true;
    }

    /// Settle a released interactive pop toward completion (`1.0`) or cancellation
    /// (`0.0`), springing from the held progress with initial `progress_velocity`
    /// (progress units/s). A cancel flags the transition for
    /// [restore](Self::finalize_transition) rather than teardown.
    fn settle_interactive(&mut self, complete: bool, progress_velocity: f64) {
        if let Some(t) = self.transition.as_mut() {
            let from = t.driver.value();
            let target = if complete { 1.0 } else { 0.0 };
            t.driver = settle_driver(t.settle_spring, from, progress_velocity, target);
            t.settled = false;
            t.restore_on_finalize = !complete;
        }
    }

    /// Drive an in-progress interactive edge-swipe from a pointer event (task 05):
    /// `Move` maps drag-x to held progress; `Up` settles by progress/velocity;
    /// `Cancel` (system gesture steal) cancels the pop with no state mutation.
    ///
    /// This runs *before* the mid-transition input block in
    /// [`event_at`](Self::event_at) — the swipe owns the pointer stream and must
    /// keep receiving moves/releases even though a (held) transition is present.
    fn drive_edge_swipe(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &InputEvent,
        t_ms: f64,
    ) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            // Only pointer events drive a swipe; ignore anything else while active.
            return EventResult::Ignored;
        };
        let width = ctx.size().width.max(1.0);
        match p.phase {
            PointerPhase::Move => {
                self.edge.tracker.record(t_ms, p.position.x);
                let dx = p.position.x - self.edge.down_start.x;
                let progress = (dx / width).clamp(0.0, 1.0);
                self.set_transition_progress(progress);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let finger_v = self.edge.tracker.velocity();
                let progress = self
                    .transition
                    .as_ref()
                    .map(|t| t.driver.value())
                    .unwrap_or(0.0);
                // Complete if dragged past the commit point, or flicked rightward
                // fast enough — the low-progress high-velocity case (criterion 1e).
                let complete =
                    progress > EDGE_SWIPE_COMMIT_PROGRESS || finger_v > EDGE_SWIPE_FLING_VELOCITY;
                // The spring drives *progress*, so convert the px/s finger velocity
                // into progress/s by the drag axis length.
                self.settle_interactive(complete, finger_v / width);
                self.edge.active = false;
                self.edge.armed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // System cancel mid-drag = cancel path: spring back and restore the
                // page. No state mutation here (the `()` Cancel tripwire contract).
                self.settle_interactive(false, 0.0);
                self.edge.active = false;
                self.edge.armed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            // A stray extra Down during a drive: swallow it (the navigator owns the
            // stream), don't re-enter arming.
            PointerPhase::Down => EventResult::Handled,
        }
    }

    /// Route an event to the top page via the shared single-child router.
    fn route_top(&mut self, ctx: &mut EventCtx<'_>, event: &InputEvent) -> EventResult {
        if let Some(top) = self.pages.last_mut() {
            crate::route_event_single(&mut top.pod, ctx, event)
        } else {
            EventResult::Ignored
        }
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
        // Input-blocking contract (task 03): while a non-interactive transition is
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
                // Arm an edge-swipe on a left-edge Down over a poppable stack. The
                // navigator does NOT capture here (ScrollView precedent): the page
                // still sees the Down and may capture; a later steal sends the page
                // a synthetic Cancel. No buffering/re-dispatch — children see Down
                // first.
                self.edge.armed = self.pop_swipe_enabled
                    && self.pages.len() > 1
                    && p.position.x <= EDGE_SWIPE_ZONE_DP;
                if self.edge.armed {
                    self.edge.down_start = p.position;
                    self.edge.tracker.clear();
                    self.edge.tracker.record(t_ms, p.position.x);
                }
                self.route_top(ctx, event)
            }
            PointerPhase::Move => {
                if !self.edge.armed {
                    return self.route_top(ctx, event);
                }
                self.edge.tracker.record(t_ms, p.position.x);
                let dx = p.position.x - self.edge.down_start.x;
                let dy = p.position.y - self.edge.down_start.y;
                if dx > TOUCH_SLOP && dx.abs() > dy.abs() {
                    // Decisive rightward horizontal drag → STEAL from the page.
                    // Re-validate stack depth at the steal site: a structural op
                    // (a programmatic pop/replace) applied at a rebuild between
                    // this arm's `Down` and now may have emptied the poppable
                    // stack. `begin_interactive_pop`'s only depth check is a
                    // debug-only `debug_assert!` (compiled out in release), so
                    // without this guard a stale arm could pop the root page in a
                    // release build — draining the stack to zero pages. If the
                    // stack is no longer poppable, drop the stale arm and fall
                    // through to normal routing rather than stealing.
                    if self.pages.len() <= 1 {
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
                    self.route_top(ctx, event)
                } else {
                    // Still within slop: keep observing, forward to the page.
                    self.route_top(ctx, event)
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                // An armed-but-never-stolen gesture just releases its arm; the page
                // owned the Down/Move/Up stream throughout.
                self.edge.armed = false;
                self.route_top(ctx, event)
            }
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
                } => {
                    let spec = self.effective_spec(transition);
                    self.cancel_top();
                    // Disarm any pending edge-swipe: a structural stack mutation
                    // invalidates an arm captured against the pre-mutation stack
                    // (mirrors the capture/focus-clearing `cancel_top` contract).
                    self.edge.armed = false;
                    let view = builder();
                    let pod = crate::build_child(&view, ctx);
                    self.pages.push(PageEntry {
                        builder,
                        view,
                        pod,
                        opaque,
                        on_result,
                        transition: spec,
                    });
                    self.needs_ime_clear = true;
                    // A push's leaving page (now at `len - 2`) stays in the stack;
                    // the transition keeps it painted (culling deferred to settle).
                    if spec.is_animated() && self.pages.len() >= 2 {
                        self.start_transition(spec, false, None, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                NavOp::Pop { result } => {
                    // A navigator always keeps its root page; a pop of the last
                    // page is a no-op (its result payload is dropped).
                    if self.pages.len() > 1 {
                        self.cancel_top();
                        // Disarm any pending edge-swipe: this pop shrinks the
                        // stack, so an arm captured before it must not later steal
                        // an interactive pop against the now-shallower stack
                        // (mirrors the `cancel_top` capture/focus-clearing contract).
                        self.edge.armed = false;
                        let mut popped = self.pages.pop().expect("len checked > 1");
                        let spec = popped.transition;
                        if let Some(callback) = popped.on_result.take() {
                            self.pending_results.push((callback, result));
                        }
                        self.needs_ime_clear = true;
                        if spec.is_animated() {
                            // Keep the popped page alive & painted, animating out;
                            // torn down on settle (a pop reverses its transition).
                            self.start_transition(spec, true, Some(popped), ctx);
                        } else {
                            crate::teardown_child(&popped.view, &mut popped.pod, ctx);
                        }
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
                NavOp::Replace {
                    builder,
                    opaque,
                    transition,
                } => {
                    let spec = self.effective_spec(transition);
                    self.cancel_top();
                    // Disarm any pending edge-swipe: replacing the top page
                    // invalidates an arm captured against the outgoing page
                    // (mirrors the `cancel_top` capture/focus-clearing contract).
                    self.edge.armed = false;
                    let view = builder();
                    let pod = crate::build_child(&view, ctx);
                    if let Some(top) = self.pages.last_mut() {
                        let entry = PageEntry {
                            builder,
                            view,
                            pod,
                            opaque,
                            on_result: None,
                            transition: spec,
                        };
                        if spec.is_animated() {
                            // Stash the old top and animate the new one in over it
                            // (push-like direction); torn down on settle.
                            let old = std::mem::replace(top, entry);
                            self.start_transition(spec, false, Some(old), ctx);
                        } else {
                            crate::teardown_child(&top.view, &mut top.pod, ctx);
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
                        });
                    }
                    self.needs_ime_clear = true;
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
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
        let (adv, is_pop, preset, has_stashed) = {
            let t = self.transition.as_mut().expect("transition present");
            let adv = t.driver.advance(ctx.frame_time());
            (adv, t.is_pop, t.preset, t.stashed.is_some())
        };
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

/// Paint one transition page: offset its pod origin by the layer's `dx`/`dy` (so
/// paint and hit-testing move together), and composite at the layer's opacity
/// via a `push_layer`/`pop_layer` pair when it is below full opacity.
fn paint_page_layer(
    pod: &mut ChildPod,
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    layer: Layer,
    area: Size,
) {
    pod.set_origin(Point::new(layer.dx, layer.dy));
    let alpha = layer.alpha.clamp(0.0, 1.0);
    if alpha < 1.0 {
        scene.push_layer(ctx.origin(), area, alpha);
        pod.paint_child(ctx, scene);
        scene.pop_layer();
    } else {
        pod.paint_child(ctx, scene);
    }
}

impl<State: 'static> View<State> for NavigatorView<State> {
    type Element = NavigatorWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NavigatorWidget<State> {
        let view = (self.initial)();
        let pod = crate::build_child(&view, ctx);
        let mut widget = NavigatorWidget {
            pages: vec![PageEntry {
                builder: self.initial.clone(),
                view,
                pod,
                opaque: true,
                on_result: None,
                transition: self.default_transition,
            }],
            pending_results: Vec::new(),
            needs_ime_clear: false,
            default_transition: self.default_transition,
            transition: None,
            pop_swipe_enabled: self.resolve_pop_swipe(),
            edge: EdgeSwipe::new(),
            last_frame_time: FrameTime::ZERO,
        };
        // Apply any ops the app queued before the first frame.
        let ops = self.controller.drain();
        if !ops.is_empty() {
            widget.apply_ops(ops, ctx);
        }
        widget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut NavigatorWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // Keep the widget's default transition in sync with the view so an app can
        // change it live (per-op overrides always win over it).
        element.default_transition = self.default_transition;
        // Refresh the edge-swipe enable flag from the view too (live-configurable).
        element.pop_swipe_enabled = self.resolve_pop_swipe();
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
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // 2. Reconcile every retained page (including culled ones) by re-running
        //    its builder against live state — the pod, and thus the page's own
        //    widget state, is preserved; only the view descriptor is rebuilt.
        for entry in &mut element.pages {
            let next_view = (entry.builder)();
            flags |= crate::rebuild_child(&entry.view, &next_view, &mut entry.pod, ctx);
            entry.view = next_view;
        }
        flags
    }

    fn teardown(&self, element: &mut NavigatorWidget<State>, ctx: &mut BuildCtx<'_>) {
        // Tear down a transition's retained (leaving) page first, then the stack.
        element.finalize_transition(ctx);
        for entry in &mut element.pages {
            crate::teardown_child(&entry.view, &mut entry.pod, ctx);
        }
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
        // Flush pop-result callbacks queued at the previous rebuild — this is the
        // first point after a pop where the erased app state is in scope.
        if !self.pending_results.is_empty() {
            let pending = std::mem::take(&mut self.pending_results);
            let state = ctx.state_mut::<State>();
            for (callback, result) in pending {
                callback(state, result);
            }
        }
        // The rest of the event body (edge-swipe arm/steal/drive + the
        // mid-transition input block + top-page routing) runs against the
        // paint-derived event-pass clock. See [`event_at`](Self::event_at).
        //
        // Input-blocking contract (refuter-verified, STRICT): while a
        // non-interactive transition is in flight, `event_at` suppresses ALL
        // routing to pages — a mid-transition `Down` reaches no page and records
        // no `active`/focus path (the involved pages' captures were already
        // synthetically cancelled at transition start via `cancel_top`). An
        // interactive edge-swipe is the deliberate exception: it drives a held
        // transition and keeps receiving its own pointer stream.
        let t_ms = self.event_time_ms();
        self.event_at(ctx, event, t_ms)
    }
}

/// The cleared/inactive IME surface the navigator publishes after a page switch so
/// the platform keyboard hides deterministically (see [`NavigatorWidget::paint`]).
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingScene;
    use forgekit_core::{FrameTime, PointerButton, PointerEvent, PointerPhase, RenderRoot, any};
    use kurbo::Rect;
    use std::cell::Cell;

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn move_to(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn up(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Up,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn cancel_ev(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // --- A leaf with retained internal state (a counter) that reports its value
    //     during paint, so a test can prove the page's widget state survives a
    //     push→pop round-trip (pod retention, not a rebuild-from-scratch). ---

    struct CounterView {
        observed: Rc<Cell<u32>>,
    }
    struct CounterWidget {
        count: u32,
        observed: Rc<Cell<u32>>,
    }
    impl View<()> for CounterView {
        type Element = CounterWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterWidget {
            CounterWidget {
                count: 0,
                observed: self.observed.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CounterWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // Refresh the observation handle but preserve the retained count.
            element.observed = self.observed.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for CounterWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.observed.set(self.count);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                self.count += 1;
                ctx.request_redraw();
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    fn counter_page(observed: &Rc<Cell<u32>>) -> AnyView<()> {
        any(CounterView {
            observed: observed.clone(),
        })
    }

    /// A leaf that fills a rect of a fixed size — RecordingScene captures its
    /// (origin, size) so a paint-culling test can tell pages apart by size.
    struct SizedLeaf {
        size: Size,
    }
    impl<S: 'static> View<S> for SizedLeaf {
        type Element = SizedLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SizedLeafWidget {
            SizedLeafWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SizedLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    struct SizedLeafWidget {
        size: Size,
    }
    impl Widget for SizedLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        }
    }
    fn sized_page<S: 'static>(w: f64, h: f64) -> AnyView<S> {
        any(SizedLeaf {
            size: Size::new(w, h),
        })
    }

    // --- Criterion 1: retained per-page widget state across push → pop. ---

    #[test]
    fn push_pop_preserves_page_widget_state() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed = Rc::new(Cell::new(0u32));

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let obs = observed.clone();
            move |_: &mut ()| {
                navigator(&ctrl, {
                    let obs = obs.clone();
                    move || counter_page(&obs)
                })
            }
        };
        let mut state = ();

        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 0, "fresh counter starts at zero");

        // Tap page A → its retained counter increments to 1.
        root.event(&mut state, &down(5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1);

        // Push B (opaque): A is culled — not painted, count untouched.
        controller.push(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1, "covered page A is not repainted");

        // Pop B → A is revealed and repainted; its retained count is still 1
        // (proving the pod survived rather than being rebuilt from scratch).
        controller.pop();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1, "page A's widget state survived push→pop");
    }

    // --- Criterion 2: a pop result reaches the on_result callback with state. ---

    #[derive(Default)]
    struct ResultState {
        received: Option<i32>,
    }

    #[test]
    fn pop_result_reaches_callback_with_state() {
        let controller: NavigatorController<ResultState> = NavigatorController::new();

        let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ResultState| {
                navigator(&ctrl, || {
                    any(SizedLeaf {
                        size: Size::new(10.0, 10.0),
                    })
                })
            }
        };
        let mut state = ResultState::default();

        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push B, registering a result callback that records into app state.
        controller.push_for_result(
            || {
                any(SizedLeaf {
                    size: Size::new(10.0, 10.0),
                })
            },
            |state: &mut ResultState, result: PopResult| {
                state.received = result.take::<i32>();
            },
        );
        root.rebuild(&mut app, &mut state);

        // Pop B with a payload; the structural pop applies at rebuild and queues
        // the callback.
        controller.pop_with_result(PopResult::of(42i32));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            state.received, None,
            "callback not yet flushed (no event pass)"
        );

        // The next event pass flushes the queued callback with `&mut State`.
        root.event(&mut state, &move_to(5.0, 5.0));
        assert_eq!(state.received, Some(42));
    }

    // --- push_transparent_for_result: the modal+result combination carries its
    //     callback through the pop, same as push_for_result's opaque case, while
    //     also keeping the page below visible (transparent) the whole time. ---

    #[test]
    fn push_transparent_for_result_carries_callback_through_pop() {
        let controller: NavigatorController<ResultState> = NavigatorController::new();
        let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ResultState| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ResultState::default();

        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push a transparent "dialog" page registering a result callback.
        controller.push_transparent_for_result(
            || sized_page(20.0, 20.0),
            TransitionSpec::NONE,
            |state: &mut ResultState, result: PopResult| {
                state.received = result.take::<i32>();
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        assert_eq!(
            scene.rects,
            vec![
                (Point::ZERO, Size::new(100.0, 100.0)),
                (Point::ZERO, Size::new(20.0, 20.0)),
            ],
            "the page below the transparent dialog stays visible"
        );

        // Pop the dialog with a payload; the callback is queued at rebuild and
        // flushed at the next event pass, exactly like the opaque
        // push_for_result path.
        controller.pop_with_result(PopResult::of(7i32));
        root.rebuild(&mut app, &mut state);
        assert_eq!(state.received, None, "not yet flushed (no event pass)");
        root.event(&mut state, &move_to(5.0, 5.0));
        assert_eq!(state.received, Some(7));
    }

    // --- Criterion 3: opaque-page paint culling. ---

    fn drive_paint(root: &mut RenderRoot<(), NavigatorView<()>>) -> Vec<(Point, Size)> {
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        scene.rects
    }

    #[test]
    fn opaque_top_culls_pages_below() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // Only the root page paints.
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(10.0, 10.0))]
        );

        // Push an opaque page B (20x20): only B paints, A is culled.
        controller.push(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(20.0, 20.0))]
        );
    }

    #[test]
    fn transparent_top_paints_page_below() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // A transparent top page B (20x20) over opaque A (10x10): both paint,
        // bottom-to-top (A then B).
        controller.push_transparent(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![
                (Point::ZERO, Size::new(10.0, 10.0)),
                (Point::ZERO, Size::new(20.0, 20.0)),
            ]
        );
    }

    // --- Criterion 4a: a captured drag on the top page is cancelled on push. ---

    struct CaptureLeaf {
        cancelled: Rc<Cell<bool>>,
    }
    struct CaptureLeafWidget {
        cancelled: Rc<Cell<bool>>,
    }
    impl View<()> for CaptureLeaf {
        type Element = CaptureLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CaptureLeafWidget {
            CaptureLeafWidget {
                cancelled: self.cancelled.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CaptureLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.cancelled = self.cancelled.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for CaptureLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    // A `Cancel` arm must never touch application state (the `()`
                    // tripwire): only record that it fired.
                    PointerPhase::Cancel => {
                        self.cancelled.set(true);
                        return EventResult::Handled;
                    }
                    _ => return EventResult::Handled,
                }
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn push_cancels_captured_drag_on_outgoing_top() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let cancelled = Rc::new(Cell::new(false));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let flag = cancelled.clone();
            move |_: &mut ()| {
                let flag = flag.clone();
                navigator(&ctrl, move || {
                    any(CaptureLeaf {
                        cancelled: flag.clone(),
                    })
                })
            }
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Capture a drag on page A.
        root.event(&mut state, &down(5.0, 5.0));
        assert!(root.is_pointer_captured());
        assert!(!cancelled.get());

        // Push B → A's in-flight capture is cancelled (synthetic Cancel, no state
        // access) as it becomes the covered page.
        controller.push(|| {
            any(SizedLeaf {
                size: Size::new(10.0, 10.0),
            })
        });
        root.rebuild(&mut app, &mut state);
        assert!(
            cancelled.get(),
            "the covered page received a synthetic Cancel"
        );
    }

    // --- Criterion 4b: a focused field's IME surface is cleared on push. ---

    struct EditableLeaf;
    struct EditableLeafWidget;
    impl View<()> for EditableLeaf {
        type Element = EditableLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> EditableLeafWidget {
            EditableLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut EditableLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for EditableLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.request_focus();
                ctx.publish_ime_state(ImeState {
                    active: true,
                    editing: EditingState {
                        text: "abc".to_string(),
                        selection_base: 3,
                        selection_extent: 3,
                        composing_base: -1,
                        composing_extent: -1,
                    },
                    caret: Some(Rect::new(0.0, 0.0, 1.0, 12.0)),
                });
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn push_clears_focused_field_ime_surface() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || any(EditableLeaf))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Focus the editable on page A: it publishes an active IME surface.
        root.event(&mut state, &down(5.0, 5.0));
        assert!(root.is_focus_active());
        let ime = root
            .ime_state()
            .expect("focused field published an IME surface");
        assert!(ime.active);
        assert_eq!(ime.editing.text, "abc");

        // Push B programmatically (no blurring tap): the navigator's own switch
        // handling must clear the stale IME surface deterministically at paint.
        controller.push(|| {
            any(SizedLeaf {
                size: Size::new(10.0, 10.0),
            })
        });
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        let ime = root
            .ime_state()
            .expect("a cleared IME surface is still published");
        assert!(
            !ime.active,
            "the stale active IME surface was cleared, not left stale"
        );
        assert!(ime.editing.text.is_empty());
    }

    // --- Criterion 5 / replace: an example-style stack driven through the facade
    //     API (counter pattern), proving replace swaps the top in place. ---

    #[test]
    fn replace_swaps_top_in_place() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(10.0, 10.0))]
        );

        // Replace the root page with a 30x30 page: still a single page, new size.
        controller.replace(|| sized_page(30.0, 30.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(30.0, 30.0))]
        );

        // Pop is a no-op on a single-page stack (root is never popped).
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(30.0, 30.0))]
        );
    }

    #[test]
    fn pop_result_take_recovers_typed_payload() {
        assert_eq!(PopResult::of(7u8).take::<u8>(), Some(7));
        assert_eq!(PopResult::of(7u8).take::<i64>(), None);
        assert!(PopResult::empty().is_empty());
        assert_eq!(PopResult::empty().take::<u8>(), None);
    }

    // ---------------------------------------------------------------------
    // Task 03: page-transition machinery.
    // ---------------------------------------------------------------------

    use super::super::transition::{PageTransition, Timing, TransitionSpec};
    use forgekit_core::Curve;
    use forgekit_theme::MotionSpring;
    use std::time::Duration;

    /// A recording scene that captures each fill's (origin, size) *and* the alpha
    /// of the enclosing `push_layer` — so a transition test can assert both a
    /// page's animated offset (origin) and its opacity.
    #[derive(Default)]
    struct TransitionScene {
        fills: Vec<(Point, Size, f32)>,
        layer_alpha: Vec<f32>,
    }
    impl PaintScene for TransitionScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
            let alpha = self.layer_alpha.last().copied().unwrap_or(1.0);
            self.fills.push((origin, size, alpha));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layer_alpha.push(alpha);
        }
        fn pop_layer(&mut self) {
            self.layer_alpha.pop();
        }
    }

    fn ft(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    /// Run one full frame (rebuild → layout → paint) at `time`, returning the
    /// recorded fills and whether another frame was requested. Generic over
    /// `State` so a result-carrying transition test (task 06) can share it too.
    fn full_frame<State: 'static>(
        root: &mut RenderRoot<State, NavigatorView<State>>,
        app: &mut impl FnMut(&mut State) -> NavigatorView<State>,
        state: &mut State,
        time: FrameTime,
    ) -> (Vec<(Point, Size, f32)>, bool) {
        root.rebuild(app, state);
        root.layout(Size::new(100.0, 100.0));
        let mut scene = TransitionScene::default();
        let out = root.paint(&mut scene, time);
        (scene.fills, out.needs_frame)
    }

    /// Find the fill for the page of the given height (tests tag pages A/B by a
    /// distinct height).
    fn fill_h(fills: &[(Point, Size, f32)], h: f64) -> (Point, Size, f32) {
        *fills
            .iter()
            .find(|(_, s, _)| (s.height - h).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no fill with height {h} in {fills:?}"))
    }

    // --- Criterion 1: push animates both pages with moving origins, settling at
    //     final geometry; controller disposed after settle. ---

    #[test]
    fn push_animates_moving_origins_and_disposes_after_settle() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            // Root page A is 100 tall; pushed page B is 80 tall (so a test can
            // tell them apart in the recorded fills).
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let spec = TransitionSpec::new(
            PageTransition::M3SharedAxisX,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_with(|| sized_page(100.0, 80.0), spec);

        // Seed frame: driver at 0. A (leaving) at rest, B (entering) slid in 30dp.
        let (f0, nf0) = full_frame(&mut root, &mut app, &mut state, ft(0));
        assert!(nf0, "a running transition requests frames");
        assert_eq!(fill_h(&f0, 100.0).0.x, 0.0, "A at rest at start");
        assert_eq!(
            fill_h(&f0, 80.0).0.x,
            30.0,
            "B slid in by the 30dp shared axis"
        );

        // Mid frame (50ms → progress 0.5): both origins have moved inward.
        let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(50));
        let a1x = fill_h(&f1, 100.0).0.x;
        let b1x = fill_h(&f1, 80.0).0.x;
        assert!(a1x < 0.0, "A slides out to the left (was {a1x})");
        assert!(b1x > 0.0 && b1x < 30.0, "B slides toward rest (was {b1x})");

        // End frame (150ms → past the 100ms duration): settles at final geometry.
        let (f2, nf2) = full_frame(&mut root, &mut app, &mut state, ft(150));
        assert_eq!(fill_h(&f2, 80.0).0.x, 0.0, "B rests exactly at 0");
        assert!(nf2, "the settling frame still requests one finalize frame");

        // Finalize frame: the controller is disposed, culling resumes, and no
        // further frame is requested.
        let (f3, nf3) = full_frame(&mut root, &mut app, &mut state, ft(300));
        assert!(!nf3, "no frame requested once the transition is disposed");
        assert_eq!(f3.len(), 1, "culling resumed: only the top page paints");
        assert_eq!(f3[0].1, Size::new(100.0, 80.0), "the surviving page is B");
    }

    // --- Criterion 2: input is blocked mid-transition; no page receives the Down
    //     and no stale capture is left; routing resumes after settle. ---

    #[test]
    fn input_blocked_mid_transition_then_resumes() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed_a = Rc::new(Cell::new(0u32));
        let observed_b = Rc::new(Cell::new(0u32));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let a = observed_a.clone();
            move |_: &mut ()| {
                let a = a.clone();
                navigator(&ctrl, move || counter_page(&a))
            }
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push B (a counter) with a long, still-running transition.
        let spec = TransitionSpec::new(
            PageTransition::M3FadeThrough,
            Timing::Duration(Duration::from_millis(1000), Curve::Linear),
        );
        {
            let b = observed_b.clone();
            controller.push_with(move || counter_page(&b), spec);
        }
        full_frame(&mut root, &mut app, &mut state, ft(0)); // seed; running

        // A pointer Down mid-transition reaches NO page and records no capture.
        root.event(&mut state, &down(5.0, 5.0));
        assert!(
            !root.is_pointer_captured(),
            "no capture is recorded mid-transition"
        );
        full_frame(&mut root, &mut app, &mut state, ft(16));
        assert_eq!(observed_a.get(), 0, "the Down did not reach page A");
        assert_eq!(observed_b.get(), 0, "the Down did not reach page B");

        // Advance past the end to settle, then finalize on the next rebuild.
        full_frame(&mut root, &mut app, &mut state, ft(1100));
        full_frame(&mut root, &mut app, &mut state, ft(1116));

        // Routing has resumed with no stale block: a Down now reaches top page B.
        root.event(&mut state, &down(5.0, 5.0));
        full_frame(&mut root, &mut app, &mut state, ft(1132));
        assert_eq!(observed_b.get(), 1, "routing resumed after the transition");
        assert!(!root.is_pointer_captured());
    }

    // --- Criterion 3: the below page's secondary animation (iOS push parallax +
    //     dim). ---

    #[test]
    fn ios_push_parallaxes_and_dims_below_page() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let spec = TransitionSpec::new(
            PageTransition::IosPush,
            Timing::Duration(Duration::from_millis(350), Curve::Linear),
        );
        controller.push_with(|| sized_page(100.0, 80.0), spec);

        // Seed: below page A at rest, full opacity; incoming B a full width off.
        let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(0));
        assert_eq!(fill_h(&f0, 100.0).0.x, 0.0);
        assert_eq!(fill_h(&f0, 100.0).2, 1.0);
        assert_eq!(
            fill_h(&f0, 80.0).0.x,
            100.0,
            "B enters a full width to the right"
        );

        // Halfway (175ms → 0.5): A (below) has parallaxed left and dimmed.
        let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(175));
        let a = fill_h(&f1, 100.0);
        assert!(a.0.x < 0.0, "below page parallaxes left (was {})", a.0.x);
        assert!(a.2 < 1.0, "below page is dimmed (alpha {})", a.2);
    }

    // --- Criterion 4: a spatial spring overshoots position, never opacity. ---

    #[test]
    fn spring_spatial_overshoots_position_but_not_opacity() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // M3 default spatial preset (damping 0.9) — overshoots.
        let spring = MotionSpring {
            damping_ratio: 0.9,
            stiffness: 700.0,
        };
        let spec = TransitionSpec::spring(PageTransition::M3SharedAxisX, spring);
        controller.push_with(|| sized_page(100.0, 80.0), spec);

        let mut min_b_x = f64::MAX;
        let mut max_alpha = f32::MIN;
        let mut running = true;
        let mut t = 0u64;
        for _ in 0..2_000 {
            let (fills, needs_frame) = full_frame(&mut root, &mut app, &mut state, ft(t));
            // B (entering, height 80) is only present while the transition runs.
            if let Some((p, _, a)) = fills
                .iter()
                .find(|(_, s, _)| (s.height - 80.0).abs() < 1e-9)
            {
                min_b_x = min_b_x.min(p.x);
                max_alpha = max_alpha.max(*a);
            }
            running = needs_frame;
            if !running {
                break;
            }
            t += 8; // ~120fps
        }
        assert!(!running, "spring transition failed to settle");
        // The entering page rests at x = 0; an under-damped spring carries it past
        // that (x < 0) before settling — the overshoot the spatial preset exists
        // to produce.
        assert!(
            min_b_x < -1e-3,
            "expected a position overshoot past the resting x=0, min x was {min_b_x}"
        );
        // Opacity is fed the clamped value, so it never exceeds 1.0 even as the
        // spatial spring overshoots.
        assert!(
            max_alpha <= 1.0 + 1e-6,
            "opacity must never overshoot 1.0 (saw {max_alpha})"
        );
    }

    // --- Replace with a transition retains + tears down the outgoing page. ---

    #[test]
    fn animated_replace_settles_to_new_page() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let spec = TransitionSpec::new(
            PageTransition::M3FadeThrough,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.replace_with(|| sized_page(100.0, 40.0), spec);

        // Drive to completion.
        let mut last = Vec::new();
        for t in [0u64, 50, 150, 300] {
            let (fills, _) = full_frame(&mut root, &mut app, &mut state, ft(t));
            last = fills;
        }
        // After settle only the new page (height 40) remains — the replaced page
        // was retained during the animation and torn down at settle.
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].1, Size::new(100.0, 40.0));
    }

    // --- Pop reverses the popped page's transition and reveals the page below. ---

    #[test]
    fn animated_pop_reverses_and_reveals_below() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push B instantly (no transition), then animate the pop.
        controller.push_with(
            || sized_page(100.0, 60.0),
            TransitionSpec::new(
                PageTransition::IosPush,
                Timing::Duration(Duration::from_millis(100), Curve::Linear),
            ),
        );
        // Settle the push first.
        for t in [0u64, 50, 150, 300] {
            full_frame(&mut root, &mut app, &mut state, ft(t));
        }

        // Now pop: the popped page (B) reverses its iOS transition (slides right),
        // revealing A below it.
        controller.pop();
        let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1000));
        // Both A (100) and B (60) paint during the pop.
        assert!(f0.iter().any(|(_, s, _)| (s.height - 100.0).abs() < 1e-9));
        assert!(f0.iter().any(|(_, s, _)| (s.height - 60.0).abs() < 1e-9));

        // Drive to completion: B is torn down, A revealed and culled to top.
        let mut last = Vec::new();
        for t in [1050u64, 1150, 1300] {
            let (fills, _) = full_frame(&mut root, &mut app, &mut state, ft(t));
            last = fills;
        }
        assert_eq!(last.len(), 1, "only the revealed page remains");
        assert_eq!(last[0].1, Size::new(100.0, 100.0), "the revealed page is A");
    }

    // ---------------------------------------------------------------------
    // Task 06: SlideUp preset + push_transparent_for_result.
    // ---------------------------------------------------------------------

    // --- SlideUp paint sequence: the entering sheet's origin moves bottom→top
    //     as progress advances, settles exactly at rest, and is disposed (culling
    //     resumes) once the transition finalizes — mirroring
    //     `push_animates_moving_origins_and_disposes_after_settle` above, but
    //     checking the vertical origin a horizontal preset never moves. ---

    #[test]
    fn slide_up_moves_origin_bottom_to_top_and_disposes_after_settle() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            // Root page A is 100x100; the pushed "sheet" B is 100 wide, 40 tall
            // (a distinct height so a test can tell it apart in the fills).
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let spec = TransitionSpec::new(
            PageTransition::SlideUp,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_with(|| sized_page(100.0, 40.0), spec);

        // Seed frame: driver at 0. The sheet (B) sits a full 100px (the
        // navigator's height) below rest; the page below (A) is untouched.
        let (f0, nf0) = full_frame(&mut root, &mut app, &mut state, ft(0));
        assert!(nf0, "a running transition requests frames");
        assert_eq!(fill_h(&f0, 100.0).0, Point::ZERO, "A never moves");
        let b0 = fill_h(&f0, 40.0).0;
        assert_eq!(b0.x, 0.0, "SlideUp never offsets horizontally");
        assert_eq!(b0.y, 100.0, "B starts a full height below rest");

        // Mid frame (50ms → progress 0.5): B has moved halfway up; A is still
        // static.
        let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(50));
        assert_eq!(
            fill_h(&f1, 100.0).0,
            Point::ZERO,
            "A stays static mid-transition"
        );
        let b1 = fill_h(&f1, 40.0).0;
        assert_eq!(b1.x, 0.0);
        assert!((b1.y - 50.0).abs() < 1e-6, "B is halfway up (was {})", b1.y);
        assert!(b1.y < b0.y, "B's origin moves toward the top (up)");

        // End frame (150ms → past the 100ms duration): settles at exact rest.
        let (f2, nf2) = full_frame(&mut root, &mut app, &mut state, ft(150));
        assert_eq!(
            fill_h(&f2, 40.0).0,
            Point::ZERO,
            "B rests exactly at origin"
        );
        assert!(nf2, "the settling frame still requests one finalize frame");

        // Finalize frame: culling resumes, only the sheet (now opaque top of the
        // *animated* stack — still the same page) is considered; here it was
        // pushed with `push_with` (opaque, the default), so only B survives.
        let (f3, nf3) = full_frame(&mut root, &mut app, &mut state, ft(300));
        assert!(!nf3, "no frame requested once the transition is disposed");
        assert_eq!(f3.len(), 1, "culling resumed: only the top page paints");
        assert_eq!(f3[0].1, Size::new(100.0, 40.0), "the surviving page is B");
    }

    // --- SlideUp pop: the sheet reverses (slides back down and out) while the
    //     revealed page below never moves — the modal-sheet paint contract. ---

    #[test]
    fn slide_up_pop_slides_sheet_down_without_moving_revealed_page() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push the sheet instantly, then animate the pop.
        controller.push_with(
            || sized_page(100.0, 40.0),
            TransitionSpec::new(
                PageTransition::SlideUp,
                Timing::Duration(Duration::from_millis(100), Curve::Linear),
            ),
        );
        for t in [0u64, 50, 150, 300] {
            full_frame(&mut root, &mut app, &mut state, ft(t));
        }

        // Pop: the sheet (B) reverses, sliding back down; A (revealed) never
        // moves throughout. The pop starts a *fresh* driver (a new
        // `AnimationController` per `start_transition`), so this first frame
        // after `pop()` is its seed (progress 0, matching the push seed's
        // convention above) — the sheet still sits at rest here.
        controller.pop();
        let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1000));
        assert_eq!(fill_h(&f0, 100.0).0, Point::ZERO, "A never moves on pop");
        assert_eq!(
            fill_h(&f0, 40.0).0,
            Point::ZERO,
            "the pop's seed frame: sheet still at rest"
        );

        // 50ms in (half the 100ms duration): the sheet has started sliding down;
        // A still hasn't moved.
        let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(1050));
        assert_eq!(fill_h(&f1, 100.0).0, Point::ZERO, "A still never moves");
        let b1 = fill_h(&f1, 40.0).0;
        assert_eq!(b1.x, 0.0);
        assert!(b1.y > 0.0, "the sheet has begun sliding down (y={})", b1.y);

        let mut last = Vec::new();
        for t in [1150u64, 1300] {
            let (fills, _) = full_frame(&mut root, &mut app, &mut state, ft(t));
            last = fills;
        }
        assert_eq!(last.len(), 1, "only the revealed page remains");
        assert_eq!(last[0].1, Size::new(100.0, 100.0), "the revealed page is A");
        assert_eq!(
            last[0].0,
            Point::ZERO,
            "A settles back at its resting origin"
        );
    }

    // --- Composed: transparent + result + SlideUp — the dialog/sheet-shaped
    //     usage `push_transparent_for_result` exists for. The page below stays
    //     visible throughout (transparent), the sheet animates in/out via
    //     SlideUp, and the pop result still reaches the pusher's callback. ---

    #[test]
    fn transparent_result_slide_up_composed_dialog_usage() {
        let controller: NavigatorController<ResultState> = NavigatorController::new();
        let mut root: RenderRoot<ResultState, NavigatorView<ResultState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ResultState| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ResultState::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        let spec = TransitionSpec::new(
            PageTransition::SlideUp,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_transparent_for_result(
            || sized_page(100.0, 40.0),
            spec,
            |state: &mut ResultState, result: PopResult| {
                state.received = result.take::<i32>();
            },
        );

        // Seed frame (progress 0): the page below (A, 100 tall) already paints —
        // the sheet is transparent, so culling never hides it — while the sheet
        // (B, 40 tall) starts a full height below rest.
        let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(0));
        assert_eq!(fill_h(&f0, 100.0).0, Point::ZERO, "A stays visible below");
        assert_eq!(
            fill_h(&f0, 40.0).0,
            Point::new(0.0, 100.0),
            "B starts a full height below rest"
        );

        // Mid-animation (50ms of the 100ms duration): B is partway up; A is
        // still visible and unmoved.
        let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(50));
        assert_eq!(fill_h(&f1, 100.0).0, Point::ZERO, "A stays visible below");
        let b1 = fill_h(&f1, 40.0).0;
        assert!(b1.y > 0.0 && b1.y < 100.0, "B is mid-slide (y={})", b1.y);

        // Settle the push.
        for t in [150u64, 300] {
            full_frame(&mut root, &mut app, &mut state, ft(t));
        }
        // Still transparent: A remains visible under the settled sheet.
        let (settled, _) = full_frame(&mut root, &mut app, &mut state, ft(316));
        assert_eq!(settled.len(), 2, "both A and the settled sheet paint");

        // Pop the sheet with a result payload; drive the reverse transition to
        // settle, then flush the queued callback at the next event pass.
        controller.pop_with_result(PopResult::of(99i32));
        for t in [1000u64, 1050, 1150, 1300] {
            full_frame(&mut root, &mut app, &mut state, ft(t));
        }
        assert_eq!(state.received, None, "not yet flushed (no event pass)");
        root.event(&mut state, &move_to(5.0, 5.0));
        assert_eq!(
            state.received,
            Some(99),
            "the transparent+SlideUp dialog still delivers its pop result"
        );
    }

    // ---------------------------------------------------------------------
    // Task 05: interactive edge-swipe back gesture.
    // ---------------------------------------------------------------------

    /// Downcast the root widget to a `&NavigatorWidget` so a gesture test can
    /// inspect the private edge/transition state.
    fn nav_widget(root: &RenderRoot<(), NavigatorView<()>>) -> &NavigatorWidget<()> {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<NavigatorWidget<()>>()
            .expect("root is a NavigatorWidget")
    }

    /// A page leaf that captures the pointer on `Down`, counts the `Move`s it
    /// receives, and records whether it got a synthetic `Cancel` — so a test can
    /// tell a steal (child gets Cancel, no more moves) from a yield (child keeps
    /// receiving moves). Its `Cancel` arm touches no application state (the `()`
    /// tripwire contract).
    struct DragProbe {
        moves: Rc<Cell<u32>>,
        cancelled: Rc<Cell<bool>>,
    }
    struct DragProbeWidget {
        moves: Rc<Cell<u32>>,
        cancelled: Rc<Cell<bool>>,
    }
    impl View<()> for DragProbe {
        type Element = DragProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> DragProbeWidget {
            DragProbeWidget {
                moves: self.moves.clone(),
                cancelled: self.cancelled.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut DragProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.moves = self.moves.clone();
            element.cancelled = self.cancelled.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for DragProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    PointerPhase::Move => {
                        self.moves.set(self.moves.get() + 1);
                        return EventResult::Handled;
                    }
                    PointerPhase::Cancel => {
                        self.cancelled.set(true);
                        return EventResult::Handled;
                    }
                    PointerPhase::Up => return EventResult::Handled,
                }
            }
            EventResult::Ignored
        }
    }

    fn drag_probe_page(moves: &Rc<Cell<u32>>, cancelled: &Rc<Cell<bool>>) -> AnyView<()> {
        any(DragProbe {
            moves: moves.clone(),
            cancelled: cancelled.clone(),
        })
    }

    /// Drive one shell-style frame (rebuild → layout → paint) at `time`, returning
    /// the alpha-tagged fills and whether another frame was requested. Sharing the
    /// task-03 `TransitionScene`/`full_frame`/`fill_h`/`ft` helpers above.
    /// Run frames until the tree stops requesting them (a settle finishes and the
    /// transition finalizes), returning the last frame's fills.
    fn run_until_settled(
        root: &mut RenderRoot<(), NavigatorView<()>>,
        app: &mut impl FnMut(&mut ()) -> NavigatorView<()>,
        state: &mut (),
        mut time_ms: u64,
    ) -> Vec<(Point, Size, f32)> {
        for _ in 0..10_000 {
            let (fills, needs_frame) = full_frame(root, app, state, ft(time_ms));
            if !needs_frame {
                return fills;
            }
            time_ms += 16;
        }
        panic!("transition failed to settle");
    }

    // --- Criterion 1a: an edge drag past slop steals from a capturing child. ---

    #[test]
    fn edge_drag_steals_from_capturing_child() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let moves = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(false));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();

        // Push a capturing page B on top (instant — default transition is NONE).
        {
            let m = moves.clone();
            let c = cancelled.clone();
            controller.push(move || drag_probe_page(&m, &c));
        }
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Down at the left edge: B captures (mid-press); nothing cancelled yet.
        root.event(&mut state, &down(5.0, 50.0));
        assert!(!cancelled.get(), "no cancel before the steal");

        // A rightward drag past the slop steals: B receives a synthetic Cancel and
        // no further moves; the navigator now drives an interactive pop.
        root.paint(&mut scene, ft(16));
        root.event(&mut state, &move_to(40.0, 50.0));
        assert!(
            cancelled.get(),
            "the mid-press child got a synthetic Cancel"
        );
        let moves_at_steal = moves.get();
        assert!(
            nav_widget(&root).transition.is_some(),
            "an interactive pop began"
        );
        assert!(
            nav_widget(&root).edge.active,
            "the navigator drives the swipe"
        );

        // Subsequent drag moves drive the pop and never reach B.
        root.paint(&mut scene, ft(32));
        root.event(&mut state, &move_to(60.0, 50.0));
        assert_eq!(
            moves.get(),
            moves_at_steal,
            "B receives no moves after the steal"
        );
    }

    // --- Criterion 1b: drag moves pages, origins tracking progress. ---

    #[test]
    fn edge_drag_moves_pages_tracking_progress() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0)); // B, instant
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Down at the edge, then drag to x=40 (progress 0.35): B (leaving, iOS-pop)
        // sits at dx = progress * width.
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut scene, ft(16));
        root.event(&mut state, &move_to(40.0, 50.0)); // steal, progress (40-5)/100
        let (f1, _) = full_frame(&mut root, &mut app, &mut state, ft(24));
        let b1 = fill_h(&f1, 60.0).0.x;
        assert!(
            (b1 - 35.0).abs() < 1e-6,
            "B tracks drag progress (x was {b1})"
        );

        // Drag further right → B's origin advances with progress.
        root.event(&mut state, &move_to(70.0, 50.0)); // progress (70-5)/100 = 0.65
        let (f2, _) = full_frame(&mut root, &mut app, &mut state, ft(40));
        let b2 = fill_h(&f2, 60.0).0.x;
        assert!(
            (b2 - 65.0).abs() < 1e-6,
            "B follows the finger (x was {b2})"
        );
        assert!(
            b2 > b1,
            "the popped page moves right as the drag progresses"
        );
    }

    // --- Criterion 1c: release past the halfway point completes the pop. ---

    #[test]
    fn release_past_half_completes_pop() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0)); // B, instant
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Slow drag (100 ms apart → low velocity) past the halfway commit point.
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut scene, ft(100));
        root.event(&mut state, &move_to(70.0, 50.0)); // progress 0.65, velocity ~650 px/s
        root.paint(&mut scene, ft(200));
        root.event(&mut state, &up(70.0, 50.0)); // >0.5 → complete

        // After settle the pop completed: only page A (height 100) remains.
        let fills = run_until_settled(&mut root, &mut app, &mut state, 300);
        assert_eq!(
            fills.len(),
            1,
            "the pop completed — stack shrank to one page"
        );
        assert!(
            (fills[0].1.height - 100.0).abs() < 1e-9,
            "the surviving page is A"
        );
        assert_eq!(
            nav_widget(&root).pages.len(),
            1,
            "the retained stack shrank"
        );
        assert!(
            nav_widget(&root).transition.is_none(),
            "the transition finalized"
        );
    }

    // --- Criterion 1c (result path): a completed swipe delivers the pop result. ---

    #[derive(Default)]
    struct SwipeResultState {
        popped: bool,
    }

    #[test]
    fn swipe_complete_delivers_result_to_callback() {
        let controller: NavigatorController<SwipeResultState> = NavigatorController::new();
        let mut root: RenderRoot<SwipeResultState, NavigatorView<SwipeResultState>> =
            RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut SwipeResultState| {
                navigator(&ctrl, || {
                    any(SizedLeaf {
                        size: Size::new(100.0, 100.0),
                    })
                })
                .pop_swipe(true)
            }
        };
        let mut state = SwipeResultState::default();

        // Push B (instant) registering a result callback fired on its pop.
        controller.push_for_result(
            || {
                any(SizedLeaf {
                    size: Size::new(100.0, 60.0),
                })
            },
            |state: &mut SwipeResultState, _result: PopResult| {
                state.popped = true;
            },
        );
        let mut sink = RecordingScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut sink, FrameTime::ZERO);

        // Swipe across and release past the commit point → complete.
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut sink, ft(100));
        root.event(&mut state, &move_to(80.0, 50.0));
        root.paint(&mut sink, ft(200));
        root.event(&mut state, &up(80.0, 50.0));

        // Drive to settle/finalize (the callback is queued at finalize).
        for t in [300u64, 316, 332, 348, 400, 500, 800, 1200, 2000] {
            root.rebuild(&mut app, &mut state);
            root.layout(Size::new(100.0, 100.0));
            root.paint(&mut sink, ft(t));
        }
        assert!(
            !state.popped,
            "callback not fired until the next event pass"
        );

        // The next event pass flushes the queued result callback.
        root.event(&mut state, &move_to(5.0, 5.0));
        assert!(state.popped, "the completed swipe delivered its pop result");
    }

    // --- Criterion 1d: release below threshold cancels; page restored exactly. ---

    #[test]
    fn release_below_threshold_cancels_and_restores() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0)); // B, instant
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Small, slow drag (well under half, low velocity) then release → cancel.
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut scene, ft(200));
        root.event(&mut state, &move_to(30.0, 50.0)); // progress 0.25 (< 0.5)
        root.paint(&mut scene, ft(400));
        root.event(&mut state, &up(30.0, 50.0)); // cancel

        // After settle the pop was cancelled: page B (height 60) is restored on top
        // at exact resting geometry (origin ZERO), the stack is unchanged (depth 2).
        let fills = run_until_settled(&mut root, &mut app, &mut state, 500);
        assert_eq!(
            fills.len(),
            1,
            "cancelled pop: only the opaque top page paints"
        );
        assert!(
            (fills[0].1.height - 60.0).abs() < 1e-9,
            "page B was restored on top"
        );
        assert_eq!(
            fills[0].0,
            Point::ZERO,
            "restored page sits at exact resting origin"
        );
        assert_eq!(nav_widget(&root).pages.len(), 2, "the stack is unchanged");
        assert!(
            nav_widget(&root).transition.is_none(),
            "the transition finalized"
        );
    }

    // --- Criterion 1e: a low-progress high-velocity release completes the pop. ---

    #[test]
    fn low_progress_high_velocity_release_completes() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0)); // B, instant
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Fast flick: x 5→25 in 10 ms ≈ 2000 px/s, but progress only 0.20 (< 0.5).
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut scene, ft(10));
        root.event(&mut state, &move_to(25.0, 50.0)); // steal, progress 0.20
        assert!(
            nav_widget(&root)
                .transition
                .as_ref()
                .map(|t| t.driver.value() < EDGE_SWIPE_COMMIT_PROGRESS)
                .unwrap_or(false),
            "progress is below the commit threshold at release"
        );
        root.paint(&mut scene, ft(20));
        root.event(&mut state, &up(25.0, 50.0)); // low progress, high velocity → complete

        let fills = run_until_settled(&mut root, &mut app, &mut state, 100);
        assert_eq!(fills.len(), 1, "the fast flick completed the pop");
        assert!(
            (fills[0].1.height - 100.0).abs() < 1e-9,
            "the surviving page is A"
        );
        assert_eq!(
            nav_widget(&root).pages.len(),
            1,
            "the stack shrank to one page"
        );
    }

    // --- A system Cancel mid-drag takes the cancel path (page restored). ---

    #[test]
    fn system_cancel_mid_drag_cancels_pop() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0)); // B, instant
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Steal an interactive pop, then a system Cancel arrives mid-drag: the pop
        // cancels and the page is restored (no application-state access — a Cancel
        // arm must never touch state).
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut scene, ft(100));
        root.event(&mut state, &move_to(70.0, 50.0)); // steal, progress 0.65
        assert!(nav_widget(&root).edge.active, "the swipe is driving");
        root.paint(&mut scene, ft(200));
        root.event(&mut state, &cancel_ev(70.0, 50.0)); // system gesture steal
        assert!(
            !nav_widget(&root).edge.active,
            "the cancel released the drive"
        );

        // After settle the pop was cancelled: page B is restored on top (depth 2).
        let fills = run_until_settled(&mut root, &mut app, &mut state, 300);
        assert_eq!(
            fills.len(),
            1,
            "cancelled pop leaves the opaque top painting"
        );
        assert!(
            (fills[0].1.height - 60.0).abs() < 1e-9,
            "page B was restored"
        );
        assert_eq!(nav_widget(&root).pages.len(), 2, "the stack is unchanged");
    }

    // --- Criterion 2 (a): a non-edge drag never arms the gesture. ---

    #[test]
    fn non_edge_down_never_arms() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0));
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // A Down well inside the page (x=50, past the ~20px edge zone) never arms.
        root.event(&mut state, &down(50.0, 50.0));
        assert!(
            !nav_widget(&root).edge.armed,
            "a non-edge Down does not arm"
        );
        root.paint(&mut scene, ft(16));
        root.event(&mut state, &move_to(90.0, 50.0)); // large rightward drag
        assert!(
            !nav_widget(&root).edge.active,
            "no steal from a non-edge drag"
        );
        assert!(
            nav_widget(&root).transition.is_none(),
            "no interactive pop began"
        );
    }

    // --- Criterion 2 (b): a vertical drag starting in the edge zone stays with the
    //     page (a ScrollView child scrolls normally). ---

    #[test]
    fn vertical_drag_in_edge_zone_stays_with_page() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let moves = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(false));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        {
            let m = moves.clone();
            let c = cancelled.clone();
            controller.push(move || drag_probe_page(&m, &c));
        }
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Down in the edge zone (arms), then a vertical drag: the arm releases and
        // the page keeps the gesture — the child keeps receiving moves, no Cancel.
        root.event(&mut state, &down(5.0, 30.0));
        assert!(nav_widget(&root).edge.armed, "edge-zone Down arms");
        root.paint(&mut scene, ft(16));
        root.event(&mut state, &move_to(5.0, 70.0)); // vertical → disarm, yield
        root.paint(&mut scene, ft(32));
        root.event(&mut state, &move_to(5.0, 100.0)); // stays with the page

        assert!(
            !cancelled.get(),
            "a vertical drag never steals from the page"
        );
        assert!(moves.get() >= 1, "the page keeps receiving the drag moves");
        assert!(
            !nav_widget(&root).edge.active,
            "no interactive pop for a vertical drag"
        );
        assert!(nav_widget(&root).transition.is_none());
    }

    // --- Criterion 3: a depth-1 stack disables the gesture. ---

    #[test]
    fn depth_one_stack_disables_gesture() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // A single-page stack: an edge Down never arms (nothing to pop back to).
        root.event(&mut state, &down(5.0, 50.0));
        assert!(!nav_widget(&root).edge.armed, "depth-1 stack: no arm");
        root.paint(&mut scene, ft(16));
        root.event(&mut state, &move_to(60.0, 50.0));
        assert!(
            nav_widget(&root).transition.is_none(),
            "no interactive pop at depth 1"
        );
    }

    // --- Regression (F1): a programmatic instant pop between an edge-swipe arm
    //     and its steal must not empty the page stack. The arm is captured at
    //     depth 2; a default (non-animated) pop applied at the next rebuild
    //     shrinks the stack to the root page and never runs a transition (so the
    //     `finalize_transition` arm-clearing never fires); a decisive rightward
    //     Move must then NOT steal an interactive pop against the now-depth-1
    //     stack — which, in a release build (where `begin_interactive_pop`'s only
    //     depth guard is a compiled-out `debug_assert!`), would pop the root page
    //     and leave zero pages. ---

    #[test]
    fn programmatic_pop_between_arm_and_steal_does_not_empty_stack() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        controller.push(|| sized_page(100.0, 60.0)); // B, instant → depth 2
        let mut scene = TransitionScene::default();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(0));

        // Arm the gesture on an edge Down at depth 2.
        root.event(&mut state, &down(5.0, 50.0));
        assert!(
            nav_widget(&root).edge.armed,
            "an edge-zone Down arms at depth 2"
        );

        // A programmatic instant (default non-animated) pop lands at the next
        // rebuild, shrinking the stack to the root page — and disarming the stale
        // edge gesture as a structural mutation, with no transition to clear it.
        controller.pop();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut scene, ft(16));
        assert_eq!(
            nav_widget(&root).pages.len(),
            1,
            "the pop shrank the stack to the root page"
        );
        assert!(
            !nav_widget(&root).edge.armed,
            "the structural pop disarmed the stale edge gesture"
        );

        // A decisive rightward Move past the slop must NOT steal an interactive
        // pop on the now-depth-1 stack (which would empty it).
        root.event(&mut state, &move_to(60.0, 50.0));
        assert!(
            nav_widget(&root).transition.is_none(),
            "no interactive pop was stolen on the depth-1 stack"
        );
        assert_eq!(
            nav_widget(&root).pages.len(),
            1,
            "the root page is intact — the stack was never emptied"
        );
        assert!(
            !nav_widget(&root).edge.armed,
            "the stale arm did not survive"
        );
    }

    // --- Criterion 4 / config: pop-swipe defaults on for the iOS-push preset. ---

    #[test]
    fn pop_swipe_defaults_on_for_ios_preset() {
        // Default transition IosPush → gesture enabled without an explicit flag.
        let ios = NavigatorController::<()>::new();
        let mut root_ios: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app_ios = {
            let ctrl = ios.clone();
            move |_: &mut ()| {
                navigator(&ctrl, || sized_page(10.0, 10.0))
                    .transition(TransitionSpec::duration(PageTransition::IosPush))
            }
        };
        let mut s = ();
        root_ios.rebuild(&mut app_ios, &mut s);
        assert!(
            nav_widget(&root_ios).pop_swipe_enabled,
            "iOS-push default enables the pop-swipe"
        );

        // Default transition NONE → gesture off unless explicitly enabled.
        let plain = NavigatorController::<()>::new();
        let mut root_plain: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app_plain = {
            let ctrl = plain.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut s2 = ();
        root_plain.rebuild(&mut app_plain, &mut s2);
        assert!(
            !nav_widget(&root_plain).pop_swipe_enabled,
            "the default (instant) preset leaves the pop-swipe off"
        );
    }

    // ---------------------------------------------------------------------
    // Task 07: shared-element ("hero") transitions.
    // ---------------------------------------------------------------------

    use kurbo::Affine;

    /// A recording scene that separates page fills painted at the identity
    /// transform (`plain`) from fills painted under a pushed transform
    /// (`morphs`, recorded as their transformed bounding boxes) — so a hero test
    /// can assert the morph overlay's interpolated rect and that a suppressed
    /// endpoint painted nothing at its rest position.
    #[derive(Default)]
    struct HeroScene {
        transforms: Vec<Affine>,
        plain: Vec<(Point, Size)>,
        morphs: Vec<Rect>,
    }
    impl PaintScene for HeroScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
            let rect = Rect::from_origin_size(origin, size);
            match self.transforms.last() {
                Some(t) => self.morphs.push(t.transform_rect_bbox(rect)),
                None => self.plain.push((origin, size)),
            }
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            let composed = self.transforms.last().copied().unwrap_or(Affine::IDENTITY) * transform;
            self.transforms.push(composed);
        }
        fn pop_transform(&mut self) {
            self.transforms.pop();
        }
    }

    /// A page that is a single tagged hero wrapping a fixed-size leaf (hero at
    /// page-local origin).
    fn hero_leaf_page(tag: &'static str, w: f64, h: f64) -> AnyView<()> {
        any(crate::hero(
            tag,
            SizedLeaf {
                size: Size::new(w, h),
            },
        ))
    }

    /// A page whose tagged hero is inset by `(left, top)` — so its page-local
    /// rect differs from a [`hero_leaf_page`]'s, giving the morph a real
    /// translation *and* (with a different size) scale to interpolate.
    fn hero_offset_page(tag: &'static str, w: f64, h: f64, left: f64, top: f64) -> AnyView<()> {
        any(crate::Padding(
            crate::EdgeInsets {
                left,
                top,
                right: 0.0,
                bottom: 0.0,
            },
            crate::hero(
                tag,
                SizedLeaf {
                    size: Size::new(w, h),
                },
            ),
        ))
    }

    fn hero_frame(
        root: &mut RenderRoot<(), NavigatorView<()>>,
        app: &mut impl FnMut(&mut ()) -> NavigatorView<()>,
        state: &mut (),
        time: FrameTime,
    ) -> (HeroScene, bool) {
        root.rebuild(app, state);
        root.layout(Size::new(200.0, 200.0));
        let mut scene = HeroScene::default();
        let out = root.paint(&mut scene, time);
        (scene, out.needs_frame)
    }

    /// Whether `rects` holds a rect approximately equal to `(origin, size)`.
    fn has_rect(rects: &[Rect], origin: Point, size: Size) -> bool {
        rects.iter().any(|r| {
            (r.x0 - origin.x).abs() < 1e-6
                && (r.y0 - origin.y).abs() < 1e-6
                && (r.width() - size.width).abs() < 1e-6
                && (r.height() - size.height).abs() < 1e-6
        })
    }

    #[test]
    fn hero_push_morphs_between_pages_and_suppresses_endpoints() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            // Root A: a 40x40 hero at (0,0). It is the leaving page's endpoint.
            move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(200.0, 200.0));

        // Push B: an 80x80 hero inset to (20,30) — the entering endpoint.
        let spec = TransitionSpec::new(
            PageTransition::M3SharedAxisX,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);

        // Seed frame (p=0): no rects captured yet, so both endpoints paint
        // normally and no morph overlay is painted (discovery frame).
        let (f0, _) = hero_frame(&mut root, &mut app, &mut state, ft(0));
        assert!(f0.morphs.is_empty(), "no morph on the discovery frame");
        assert!(
            has_rect(
                &f0.plain
                    .iter()
                    .map(|(o, s)| Rect::from_origin_size(*o, *s))
                    .collect::<Vec<_>>(),
                Point::new(0.0, 0.0),
                Size::new(40.0, 40.0),
            ),
            "A's hero paints normally on the seed frame: {:?}",
            f0.plain
        );

        // Mid frame (p=0.5): the matched endpoints morph. The overlay rect is the
        // interpolation of A's (0,0,40,40) and B's (20,30,80,80):
        //   origin = (10, 15), size = (60, 60). Both endpoints are suppressed at
        //   their rest positions (only the morph paints the shared element).
        let (f1, _) = hero_frame(&mut root, &mut app, &mut state, ft(50));
        assert!(
            has_rect(&f1.morphs, Point::new(10.0, 15.0), Size::new(60.0, 60.0)),
            "morph overlay interpolates source→target: {:?}",
            f1.morphs
        );
        let plain_rects: Vec<Rect> = f1
            .plain
            .iter()
            .map(|(o, s)| Rect::from_origin_size(*o, *s))
            .collect();
        assert!(
            !has_rect(&plain_rects, Point::new(0.0, 0.0), Size::new(40.0, 40.0)),
            "A's endpoint is suppressed mid-flight: {:?}",
            f1.plain
        );

        // Settle + finalize: culling resumes, only B's hero paints — normally.
        let mut t = 150u64;
        loop {
            let (fr, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t));
            if !needs {
                assert!(
                    fr.morphs.is_empty(),
                    "no morph once settled: {:?}",
                    fr.morphs
                );
                assert!(
                    has_rect(
                        &fr.plain
                            .iter()
                            .map(|(o, s)| Rect::from_origin_size(*o, *s))
                            .collect::<Vec<_>>(),
                        Point::new(20.0, 30.0),
                        Size::new(80.0, 80.0),
                    ),
                    "B's hero rests at its own position after settle: {:?}",
                    fr.plain
                );
                break;
            }
            t += 16;
            assert!(t < 5000, "transition failed to settle");
        }
    }

    #[test]
    fn hero_tag_on_one_side_only_never_morphs() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(200.0, 200.0));

        // Push B carrying a DIFFERENT tag: no tag is present on both pages, so no
        // morph ever paints and both heroes paint normally throughout.
        let spec = TransitionSpec::new(
            PageTransition::M3SharedAxisX,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_with(|| hero_offset_page("other", 80.0, 80.0, 20.0, 30.0), spec);

        for t in [0u64, 50, 100] {
            let (fr, _) = hero_frame(&mut root, &mut app, &mut state, ft(t));
            assert!(
                fr.morphs.is_empty(),
                "an unmatched tag must not morph (frame {t}): {:?}",
                fr.morphs
            );
        }
    }

    #[test]
    fn hero_pop_morphs_backward_and_finalizes_normal() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(200.0, 200.0));

        // Push B (animated) and let it settle so we start the pop from rest.
        let spec = TransitionSpec::new(
            PageTransition::M3SharedAxisX,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);
        let mut t = 0u64;
        loop {
            let (_, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t));
            if !needs {
                break;
            }
            t += 16;
            assert!(t < 5000, "push failed to settle");
        }

        // Pop B: the pop reverses B's stored transition and morphs the shared
        // element from B's rest rect back toward A's.
        controller.pop();
        // Seed frame (discovery).
        hero_frame(&mut root, &mut app, &mut state, ft(t));
        // Mid frame (~half the 100ms reversed transition): the morph is present.
        let (fmid, _) = hero_frame(&mut root, &mut app, &mut state, ft(t + 50));
        assert!(
            !fmid.morphs.is_empty(),
            "the pop paints a hero morph overlay: {:?}",
            fmid.morphs
        );

        // Settle + finalize: only the revealed root A remains, painting normally.
        let mut t2 = t + 100;
        loop {
            let (fr, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t2));
            if !needs {
                assert!(
                    fr.morphs.is_empty(),
                    "no morph once popped: {:?}",
                    fr.morphs
                );
                assert!(
                    has_rect(
                        &fr.plain
                            .iter()
                            .map(|(o, s)| Rect::from_origin_size(*o, *s))
                            .collect::<Vec<_>>(),
                        Point::new(0.0, 0.0),
                        Size::new(40.0, 40.0),
                    ),
                    "the revealed root hero paints normally after the pop: {:?}",
                    fr.plain
                );
                break;
            }
            t2 += 16;
            assert!(t2 < 10000, "pop failed to settle");
        }
    }

    #[test]
    fn hero_edge_swipe_cancel_restores_endpoints() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| {
                navigator(&ctrl, || hero_leaf_page("avatar", 40.0, 40.0)).pop_swipe(true)
            }
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(200.0, 200.0));

        // Push B with the iOS-push preset so it is swipe-poppable; settle it.
        let spec = TransitionSpec::new(
            PageTransition::IosPush,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.push_with(|| hero_offset_page("avatar", 80.0, 80.0, 20.0, 30.0), spec);
        let mut t = 0u64;
        loop {
            let (_, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t));
            if !needs {
                break;
            }
            t += 16;
            assert!(t < 5000, "push failed to settle");
        }

        // Begin an edge swipe: a left-edge Down then a rightward drag steals into
        // an interactive (Held) pop.
        root.event(&mut state, &down(5.0, 100.0));
        hero_frame(&mut root, &mut app, &mut state, ft(t + 16));
        root.event(&mut state, &move_to(40.0, 100.0));
        assert!(
            nav_widget(&root).transition.is_some(),
            "the edge drag started an interactive pop"
        );
        // A held frame past discovery paints the morph following the drag.
        hero_frame(&mut root, &mut app, &mut state, ft(t + 32));
        let (held, _) = hero_frame(&mut root, &mut app, &mut state, ft(t + 48));
        assert!(
            !held.morphs.is_empty(),
            "the interactive pop paints a hero morph while held: {:?}",
            held.morphs
        );

        // Drag back toward the edge (low progress, leftward velocity) and release
        // there → the pop cancels and springs back rather than completing.
        root.event(&mut state, &move_to(8.0, 100.0));
        hero_frame(&mut root, &mut app, &mut state, ft(t + 64));
        root.event(&mut state, &up(7.0, 100.0));
        let mut t2 = t + 80;
        loop {
            let (fr, needs) = hero_frame(&mut root, &mut app, &mut state, ft(t2));
            if !needs {
                assert_eq!(
                    nav_widget(&root).pages.len(),
                    2,
                    "the cancelled pop restored B onto the stack"
                );
                assert!(
                    fr.morphs.is_empty(),
                    "no morph lingers after the cancel settles: {:?}",
                    fr.morphs
                );
                assert!(
                    has_rect(
                        &fr.plain
                            .iter()
                            .map(|(o, s)| Rect::from_origin_size(*o, *s))
                            .collect::<Vec<_>>(),
                        Point::new(20.0, 30.0),
                        Size::new(80.0, 80.0),
                    ),
                    "B's hero paints normally after the cancel: {:?}",
                    fr.plain
                );
                break;
            }
            t2 += 16;
            assert!(t2 < 10000, "cancel failed to settle");
        }
    }
}
