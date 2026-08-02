//! The navigator core: a retained page stack with imperative
//! push/pop/replace, per-page result callbacks, opaque-page paint culling, and
//! test-pinned capture/focus/IME page-switch semantics.
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
//! pod ([`crate::authoring::build_child`]) and tearing a popped one down
//! ([`crate::authoring::teardown_child`]) both need a `BuildCtx`, and a rebuild always runs
//! every frame so a *programmatic* push/pop (from a background task, with no
//! triggering event) still lands. On every stack mutation the widget then applies
//! the explicit page-switch contract the structural-rebuild machinery does not
//! cover for a hand-managed stack: (a) it cancels an in-flight capture on the
//! outgoing top ([`crate::authoring::cancel_pod`]'s synthetic-`Cancel`), (b) clears its focus
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
//!
//! # Root overlay host
//!
//! [`overlay_host`] is the same widget wearing a different hat: a navigator
//! whose root page is the *whole app* and whose pushed pages are app-level
//! modals, so an overlay dims and blocks chrome an inner navigator's overlay
//! cannot reach. It is a constructor, not a second widget — everything below
//! (input routing, R23, `BackPolicy`, dismiss signals, `on_result`) applies to
//! it unchanged.
//!
//! # Accessibility reach (R23)
//!
//! The accessibility tree follows **input routing**, not painting:
//! [`NavigatorWidget::semantics`](Widget::semantics) forwards exactly the pages
//! [`NavigatorWidget::input_routed_pages`] says an event could reach — today
//! the top page alone — and omits every other page outright. That is a
//! deliberate, documented exception to the forward-to-every-child container
//! rule in `docs/CODE_STANDARDS.md`; the `semantics` doc comment carries the
//! derivation.
//!
//! # Observation seams (reactive-free, by construction)
//!
//! Nothing outside a page's own subtree can reach into the navigator, so every
//! observation is *published* or *pushed* — never polled through the widget, and
//! never through a signal (`frust-widgets` carries no reactive dependency; any
//! signal mirroring is the facade's job, as `back_glue`/`router_glue` do it):
//!
//! * **Transition** — [`NavigatorController::transition`] reads a published
//!   [`TransitionState`] snapshot (an `Rc<Cell<_>>` of `Copy`, `Send + Sync`
//!   data, beside `depth`/`back_interest`). Published on the **controller**
//!   because the chrome that wants to match page motion is a *sibling* of the
//!   navigator, not a descendant. Its doc carries the read-timing contract.
//! * **Page visibility** — [`PushOptions::on_visibility`] (and
//!   [`NavigatorView::on_root_visibility`] for the root page) pushes
//!   [`PageVisibility`] changes to the page itself. A callback rather than a
//!   published cell because a covered page has no pass in which to poll one.
//!   The single derivation is [`NavigatorWidget::visibility_of`], computed from
//!   the same `base_visible_index` layout and paint cull against.
//!
//! Neither seam changes disposal: a covering push still does **not** run a
//! page's `on_cleanup`, so its widget state survives the cover.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EditingState, EventCtx, EventResult,
    FrameTime, HeroDirective, HeroFrames, ImeState, InputEvent, LayoutCtx, PaintCtx, PaintScene,
    PointerPhase, SemanticsCtx, SpringDesc, TOUCH_SLOP, VelocityTracker, View, Widget,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size, Vec2};

use super::transition::{
    Layer, PageTransition, TransitionDriver, TransitionSpec, TransitionState, lerp_rect,
    make_driver, resolve_layers, resolve_spec, settle_driver,
};

// --- Edge-swipe tuning constants (see per-constant approximation notes) ------

/// Left-edge activation zone width for the interactive pop-swipe, in logical px.
///
/// **Community-approximate**: UIKit's
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

/// Where a retained page sits in the stack right now — the vocabulary
/// [`PushOptions::on_visibility`]/[`NavigatorView::on_root_visibility`] report.
///
/// Derived from exactly the state layout and paint already cull against (see
/// [`NavigatorWidget::visibility_of`]); there is deliberately no second notion of
/// "visible" anywhere in the navigator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageVisibility {
    /// Topmost: laid out, painted, and the ONLY page routed input.
    #[default]
    Current,
    /// Painted (a transparent page — a dialog/sheet/palette — sits above it) but
    /// routed no input.
    Visible,
    /// Fully covered by an opaque page: retained (its widget state survives), but
    /// neither laid out nor painted.
    Covered,
}

/// A page-visibility observer, registered per page via
/// [`PushOptions::on_visibility`] (or [`NavigatorView::on_root_visibility`] for
/// the root page). Fired by the navigator from a rebuild whenever the page's
/// [`PageVisibility`] changes — never twice with the same value.
pub type VisibilityCallback = Rc<dyn Fn(PageVisibility)>;

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

/// How a page participates in a back press routed through
/// [`NavigatorController::request_back`] (Android hardware/gesture back, via
/// the facade's back-press wiring). A page declares its policy when pushed via
/// [`PushOptions::back`]; every existing push defaults to [`Pop`](Self::Pop).
///
/// This is *internal* routing vocabulary — the user-facing overlay builder is
/// `dismissable(bool)`, which maps `true → DismissAnimated` and
/// `false → Veto` when the overlay pushes its transparent page.
///
/// # The DismissAnimated observation seam
///
/// A [`DismissAnimated`](Self::DismissAnimated) page does *not* pop on the back
/// press itself; instead the navigator increments the page's
/// [`dismiss_signal`](PushOptions::dismiss_signal) generation counter, which the
/// page's own widget subtree observes (comparing the shared `Rc<Cell<u64>>`
/// against a last-seen value on its next paint/event) and turns into its own
/// `begin_exit` staging — the overlay then pops *itself* on exit completion via
/// its existing on-close path. This keeps `frust-widgets` reactive-free (a plain
/// shared cell, mirroring [`NavigatorController::depth`]'s
/// `Rc<Cell<usize>>`) — no `frust-reactive` dependency crosses into this crate.
/// An overlay helper (`show_command_palette`/`show_dialog`/`show_bottom_sheet`)
/// wires the seam by creating one cell, handing a clone to the overlay widget it
/// builds *and* into [`PushOptions::dismiss_signal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackPolicy {
    /// The default: a back press pops this page (the normal
    /// [`pop`](NavigatorController::pop) path, transitions preserved). A back
    /// press at the root (this being the only page) is a safe no-op.
    Pop,
    /// A dismissable overlay: a back press fires the page's
    /// [`dismiss_signal`](PushOptions::dismiss_signal) (see the seam above)
    /// rather than popping — the stack is unchanged immediately, and the overlay
    /// animates its own exit before popping itself.
    DismissAnimated,
    /// A non-dismissable overlay (a modal barrier): a back press is *consumed*
    /// (the page claims it, so it never bubbles to the platform) but does
    /// nothing — the stack is unchanged and no signal fires.
    Veto,
}

/// Options for [`NavigatorController::push_with_options`], carrying a pushed
/// page's opacity, back-press [`BackPolicy`], optional per-op transition
/// override, optional result callback, and (for a
/// [`DismissAnimated`](BackPolicy::DismissAnimated) overlay) the shared
/// dismiss-signal cell the navigator bumps on a back request.
///
/// Construct with [`opaque`](Self::opaque)/[`transparent`](Self::transparent),
/// then chain the builder setters. The existing `push*` methods are unchanged —
/// they push with [`BackPolicy::Pop`] and no dismiss signal.
pub struct PushOptions<State: 'static> {
    opaque: bool,
    back: BackPolicy,
    transition: Option<TransitionSpec>,
    on_result: Option<ResultCallback<State>>,
    dismiss_signal: Option<Rc<Cell<u64>>>,
    on_visibility: Option<VisibilityCallback>,
}

impl<State: 'static> PushOptions<State> {
    /// Options for an **opaque** page (the page below is culled while covered).
    /// Defaults: [`BackPolicy::Pop`], navigator-default transition, no result
    /// callback, no dismiss signal.
    pub fn opaque() -> Self {
        Self {
            opaque: true,
            back: BackPolicy::Pop,
            transition: None,
            on_result: None,
            dismiss_signal: None,
            on_visibility: None,
        }
    }

    /// Options for a **transparent** page (e.g. a dialog/sheet/palette overlay —
    /// the page below stays visible). Same defaults as [`opaque`](Self::opaque)
    /// otherwise.
    pub fn transparent() -> Self {
        Self {
            opaque: false,
            ..Self::opaque()
        }
    }

    /// Set the page's back-press [`BackPolicy`] (default [`BackPolicy::Pop`]).
    pub fn back(mut self, policy: BackPolicy) -> Self {
        self.back = policy;
        self
    }

    /// Override the navigator's default transition for this push only.
    pub fn transition(mut self, spec: TransitionSpec) -> Self {
        self.transition = Some(spec);
        self
    }

    /// Register a result callback invoked with `&mut State` when this page is
    /// later popped (carrying the pop's [`PopResult`]) — the same delivery the
    /// [`push_for_result`](NavigatorController::push_for_result) path uses.
    pub fn on_result(mut self, callback: impl Fn(&mut State, PopResult) + 'static) -> Self {
        self.on_result = Some(Rc::new(callback));
        self
    }

    /// Supply the shared generation cell the navigator increments when a
    /// [`DismissAnimated`](BackPolicy::DismissAnimated) back press routes to this
    /// page (see [`BackPolicy`]'s observation seam). Ignored for the other
    /// policies.
    pub fn dismiss_signal(mut self, signal: Rc<Cell<u64>>) -> Self {
        self.dismiss_signal = Some(signal);
        self
    }

    /// Observe this page's [`PageVisibility`]: fired once at push (with
    /// [`Current`](PageVisibility::Current)) and on every subsequent change,
    /// **never twice with the same value**. This is the seam a screen
    /// pauses/resumes polling, a timer, or a camera session from.
    ///
    /// # Why it is a callback, not a published cell
    ///
    /// A covered page is neither painted nor (under
    /// [`NavigatorView::cull_covered_builds`]) rebuilt, so it has **no pass in
    /// which to poll** anything. The seam therefore has to push. It is shaped
    /// exactly like [`on_result`](Self::on_result)/
    /// [`dismiss_signal`](Self::dismiss_signal): a plain `Rc<dyn Fn>` slot, no
    /// signal — `frust-widgets` is reactive-free.
    ///
    /// # No `&mut State`
    ///
    /// The callback receives **no** `&mut State`, unlike
    /// [`on_result`](Self::on_result). It fires from a rebuild (a `BuildCtx`),
    /// which carries no erased app state; and deferring it to the next event pass
    /// the way `on_result` does would mean a covered page learns it is covered
    /// only if some unrelated input happens to arrive. Capture what you need
    /// (a signal, an `Rc<Cell<_>>`, a controller handle) in the closure instead.
    ///
    /// # "No cleanup on cover" is the contract, not a bug
    ///
    /// A covering push does **not** fire the page's `on_cleanup` and must never
    /// start to: the page stays mounted so its widget state survives the cover
    /// (the retained-page-state guarantee the whole navigator rests on — see the
    /// [module docs](self)' paint-culling section). This seam exists precisely so
    /// a page can release its *own* resources on
    /// [`Covered`](PageVisibility::Covered) and re-acquire them on
    /// [`Current`](PageVisibility::Current), without the navigator disposing
    /// anything.
    ///
    /// Calling back into the [`NavigatorController`] from here is safe: a
    /// `push`/`pop` only *records* an op, drained at the next rebuild.
    pub fn on_visibility(mut self, f: impl Fn(PageVisibility) + 'static) -> Self {
        self.on_visibility = Some(Rc::new(f));
        self
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
        /// How a back press treats this page (default [`BackPolicy::Pop`]).
        back: BackPolicy,
        /// The shared generation cell bumped on a
        /// [`DismissAnimated`](BackPolicy::DismissAnimated) back press.
        dismiss_signal: Option<Rc<Cell<u64>>>,
        /// The page-visibility observer registered by
        /// [`PushOptions::on_visibility`], if any.
        on_visibility: Option<VisibilityCallback>,
    },
    /// Pop the top page (never the last/root page), delivering `result` to the
    /// popped page's pusher-registered callback. A pop *reverses* the popped
    /// page's own stored transition (no override slot).
    Pop { result: PopResult },
    /// Route a back press through the top page's [`BackPolicy`]:
    /// [`Pop`](BackPolicy::Pop) pops, [`DismissAnimated`](BackPolicy::DismissAnimated)
    /// fires the page's dismiss signal, [`Veto`](BackPolicy::Veto) consumes it.
    RequestBack,
    /// Replace the top page in place.
    Replace {
        builder: PageBuilder<State>,
        opaque: bool,
        /// Per-op transition override (`None` → the navigator's default).
        transition: Option<TransitionSpec>,
    },
}

/// An opaque identity for the navigator a [`NavigatorController`] drives: every
/// clone of one controller reports the same value, and two independently
/// constructed controllers never do.
///
/// The seam a *multi-navigator* registry keys on — the facade's back-press
/// arbitration (rule **R44-back**) holds one entry per registered controller and
/// needs to tell "this controller again" from "a second controller", which it
/// cannot do through the op queue or the published cells.
///
/// **Uniqueness holds among *live* controllers only.** The value is derived from
/// the address of the shared op queue, so a holder that wants the identity to
/// stay meaningful must keep a controller clone alive alongside it (as the
/// facade's registry does) — otherwise a freed allocation could be reused and
/// two ids collide.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NavigatorId(usize);

/// The app-state handle to a [`navigator`]: a cloneable op queue an app keeps in
/// its `Component::State` and drives with [`push`](Self::push)/[`pop`](Self::pop)/
/// [`replace`](Self::replace). Every clone shares one queue (`Rc`), so the handle
/// the view carries and the handle event handlers call are the same.
///
/// Ops are *recorded*, not applied — the [`NavigatorWidget`] drains and applies
/// them at its next rebuild (see the [module docs](self)).
pub struct NavigatorController<State: 'static> {
    ops: Rc<RefCell<Vec<NavOp<State>>>>,
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
    depth: Rc<Cell<usize>>,
    /// Whether a back press should be *claimed* by the navigator ahead-of-time
    /// (predictive-back parity) — published by the attached
    /// [`NavigatorWidget`] alongside [`depth`](Self::depth). `true` iff the stack
    /// is poppable (`depth > 1`) **or** the top page's [`BackPolicy`] is not
    /// [`Pop`](BackPolicy::Pop) (a dismissable/veto overlay claims back even at
    /// the root). This is the signal the facade's back handler computes
    /// `handles_back` from — it differs from [`can_pop`](Self::can_pop) exactly
    /// in the depth-1-with-overlay case, where a raw pop would do nothing but the
    /// overlay still owns the press. Same plain `Rc<Cell<_>>` (reactive-free)
    /// polled-at-rebuild contract as `depth`.
    back_interest: Rc<Cell<bool>>,
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
    transition: Rc<Cell<TransitionState>>,
}

impl<State: 'static> Clone for NavigatorController<State> {
    fn clone(&self) -> Self {
        Self {
            ops: Rc::clone(&self.ops),
            depth: Rc::clone(&self.depth),
            back_interest: Rc::clone(&self.back_interest),
            transition: Rc::clone(&self.transition),
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
        }
    }

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
    /// **Advisory**, published at the last rebuild like [`depth`](Self::depth) —
    /// a query racing a same-frame stack change sees the previous value; the
    /// navigator's own [`request_back`](Self::request_back) routing stays
    /// authoritative regardless (see `frust-reactive::back`'s timing note).
    pub fn back_interest(&self) -> bool {
        self.back_interest.get()
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
    /// Defaults to [`TransitionSpec::NONE`] (instant switches).
    default_transition: TransitionSpec,
    /// Explicit override for the interactive edge-swipe back gesture.
    /// `None` derives it from the default transition preset — on for
    /// [`PageTransition::IosPush`], off otherwise.
    pop_swipe: Option<bool>,
    /// The ROOT page's [`PageVisibility`] observer (the root has no
    /// [`PushOptions`] to carry one). Installed on the root page entry at
    /// `build`, like the root's [`BackPolicy`] — not live-refreshed.
    root_visibility: Option<VisibilityCallback>,
    /// Whether a [`Covered`](PageVisibility::Covered) page stops re-running its
    /// builder. Default `false` — the shipped behaviour.
    cull_covered_builds: bool,
}

impl<State: 'static> NavigatorView<State> {
    /// Set the default page transition applied to every push/replace that does
    /// not carry its own [`push_with`](NavigatorController::push_with)/
    /// [`replace_with`](NavigatorController::replace_with) override.
    pub fn transition(mut self, spec: TransitionSpec) -> Self {
        self.default_transition = spec;
        self
    }

    /// Explicitly enable or disable the interactive edge-swipe back gesture,
    /// overriding the preset-derived default (on for
    /// [`PageTransition::IosPush`], off otherwise). The gesture pops the top page
    /// with a left-edge drag: drag progress reverses the popped page's transition,
    /// and release completes or cancels the pop by progress/velocity.
    pub fn pop_swipe(mut self, enabled: bool) -> Self {
        self.pop_swipe = Some(enabled);
        self
    }

    /// Observe the **root** page's [`PageVisibility`] — the same seam
    /// [`PushOptions::on_visibility`] gives a pushed page, for the one page that
    /// has no `PushOptions`. Fired once with
    /// [`Current`](PageVisibility::Current) on the navigator's first build, then
    /// on every change (e.g. [`Covered`](PageVisibility::Covered) when an opaque
    /// page is pushed over it).
    ///
    /// Read [`PushOptions::on_visibility`]'s doc for the full contract: no
    /// `&mut State`, no duplicate values, and — importantly — **no `on_cleanup`
    /// on cover**; the root page stays mounted with its widget state intact.
    ///
    /// The callback is captured at the navigator's first `build` (like the root
    /// page's builder itself) and is not refreshed on later rebuilds.
    pub fn on_root_visibility(mut self, f: impl Fn(PageVisibility) + 'static) -> Self {
        self.root_visibility = Some(Rc::new(f));
        self
    }

    /// Skip re-running the builder (and the child reconcile) for pages that are
    /// [`Covered`](PageVisibility::Covered).
    ///
    /// **Default `false`** — the shipped behaviour, where every retained page
    /// reconciles every frame whether covered or not. Making covered builds stop
    /// is a real behaviour change (an app may rely on a covered page's builder
    /// running against live state), so it is strictly opt-in.
    ///
    /// Two ordering rules hold when enabled, and neither needs a wake mechanism:
    ///
    /// - **A revealed page rebuilds in the same pass that revealed it.** Ops are
    ///   applied — and a settled transition finalized — *before* the per-page
    ///   reconcile loop in [`NavigatorView::rebuild`], so by the time the loop
    ///   asks [`visibility_of`](NavigatorWidget::visibility_of) the revealed page
    ///   is no longer `Covered`. There is no cross-frame gap to bridge.
    /// - **The frame on which a page *becomes* `Covered` still rebuilds it.** The
    ///   cull decision reads the page's visibility as of the *previous* reconcile,
    ///   so a page gets exactly one final reconcile after its
    ///   `on_visibility(Covered)` fires — a page staging teardown UI on cover can
    ///   still render it.
    pub fn cull_covered_builds(mut self, enabled: bool) -> Self {
        self.cull_covered_builds = enabled;
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
        root_visibility: None,
        cull_covered_builds: false,
    }
}

/// The **root overlay host**: a navigator whose root page is the whole app —
/// chrome, tab shell, inner navigator and all — and whose pushed pages are the
/// app's modals. Because the host sits *above* every piece of chrome, an overlay
/// pushed here dims and blocks chrome that an overlay on an inner navigator
/// cannot reach.
///
/// It is a [`navigator`] with two defaults changed and nothing else:
///
/// * **[`pop_swipe(false)`](NavigatorView::pop_swipe)** — an edge swipe must
///   never dismiss an overlay.
/// * **[`TransitionSpec::NONE`]** — each overlay widget stages its *own*
///   enter/exit (the contract the dialog/sheet catalogs already rely on), so the
///   host must not animate the page swap underneath them.
///
/// Everything else is the ordinary navigator, deliberately: dismiss-signal
/// routing, [`PushOptions`]/[`BackPolicy`], per-overlay
/// [`on_result`](PushOptions::on_result), the keyboard drop on a page switch,
/// and the capture-cancel + focus-clear are inherited rather than re-invented.
///
/// ```no_run
/// # use frust_widgets::{NavigatorController, overlay_host, text};
/// # use frust_core::any;
/// # let controller: NavigatorController<()> = NavigatorController::new();
/// # let app_root = || any(text("the whole app: chrome, tabs, inner navigator"));
/// // Wrap the app's existing root view; nothing inside it changes.
/// let root = overlay_host(&controller, move || app_root());
/// # let _ = root;
/// ```
///
/// # The host owns no scrim
///
/// The per-page-paints-its-own-scrim convention is preserved verbatim: the host
/// is purely structural and paints nothing of its own. An overlay page already
/// fills `ctx.origin()..ctx.size()` with its scrim, and at the root that rect
/// *is* the window — so the catalogs' dialogs and sheets need no change to dim
/// the whole app.
///
/// # Chrome inertness is not new code
///
/// [`NavigatorWidget::event`](Widget::event) routes to
/// [`input_routed_pages`](NavigatorWidget::input_routed_pages) — the top page
/// only — so with an overlay up the entire app root, chrome included, receives
/// nothing. The accessibility tree says the same thing through the same
/// function under rule R23 (see [`semantics`](Widget::semantics)), so there is
/// no second reachability path to keep in sync.
///
/// # Back arbitration
///
/// With a root host *and* an inner navigator there are two back registrants. The
/// facade's back glue (`frust::back_glue`) routes a press to the first
/// registrant claiming [`back_interest`](NavigatorController::back_interest) in
/// build order — outermost first — so a press with a root overlay open reaches
/// the host, and a press with none open falls through to the inner navigator
/// (a host at depth 1 with the default [`BackPolicy::Pop`] claims nothing).
pub fn overlay_host<State: 'static>(
    controller: &NavigatorController<State>,
    app: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    navigator(controller, app)
        // Both are stated explicitly rather than left to the `navigator`
        // defaults: they are the host's *contract*, not a coincidence of what
        // `navigator` happens to default to.
        .pop_swipe(false)
        .transition(TransitionSpec::NONE)
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
    /// How a back press routed through
    /// [`request_back`](NavigatorController::request_back) treats this page.
    /// Pushed pages set it via [`PushOptions::back`]; the root and
    /// replaced pages default to [`BackPolicy::Pop`].
    back: BackPolicy,
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
    /// This transition is being driven by an interactive edge-swipe: its
    /// progress is `Held` by the drag, then settled on release. An
    /// interactive pop stashed the top page *without* queuing its result
    /// callback (a swipe may still cancel), so finalize does the completion
    /// bookkeeping the [`NavOp::Pop`] path did eagerly.
    interactive: bool,
    /// Set when an interactive pop was *cancelled* (settled toward `0.0`): finalize
    /// pushes the stashed page back onto the stack instead of tearing it down (the
    /// page was never really popped). See [`NavigatorWidget::finalize_transition`].
    restore_on_finalize: bool,
    /// Shared-element ("hero") state. Page-local rects of the tagged
    /// heroes discovered on the **leaving** page during the previous transition
    /// paint, keyed by tag. `layout`/`paint` capture these each frame; the next
    /// frame reads them to place the morph overlay. Empty until the first paint
    /// discovers any (so the morph starts a frame into the flight — the rects
    /// are static page layout, so the delay is invisible).
    hero_leaving: HashMap<String, Rect>,
    /// Page-local hero rects discovered on the **entering** page — the morph
    /// target endpoint. See [`hero_leaving`](ActiveTransition::hero_leaving).
    hero_entering: HashMap<String, Rect>,
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
    pending_spec: Option<TransitionSpec>,
}

/// The interactive edge-swipe gesture state. Mirrors
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
    /// The single in-flight transition, if any. `None` between
    /// transitions — the common case, where paint/layout cull normally.
    transition: Option<ActiveTransition<State>>,
    /// Whether the interactive edge-swipe back gesture is enabled.
    /// Resolved from the view each rebuild — default-on for the iOS-push preset,
    /// or explicitly via [`NavigatorView::pop_swipe`].
    pop_swipe_enabled: bool,
    /// The in-progress edge-swipe gesture state.
    edge: EdgeSwipe,
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
    /// Whether a [`Covered`](PageVisibility::Covered) page skips its per-frame
    /// reconcile. Refreshed from the view each rebuild (live-configurable, like
    /// `pop_swipe_enabled`); default `false`.
    cull_covered_builds: bool,
}

/// Whether the navigator should claim a back press ahead-of-time: the
/// stack is poppable (`depth > 1`) **or** the top page's [`BackPolicy`] is not
/// [`Pop`](BackPolicy::Pop). Pure so it is unit-testable directly, including the
/// depth-1-with-overlay case a raw `can_pop` cannot express.
fn compute_back_interest(depth: usize, top_policy: BackPolicy) -> bool {
    depth > 1 || top_policy != BackPolicy::Pop
}

impl<State: 'static> NavigatorWidget<State> {
    /// Publish the current page-stack depth **and** back-interest to the shared
    /// controller slots, refresh the published [`TransitionState`]'s depths, and
    /// fire any page-visibility changes. Called after every stack mutation — at
    /// the end of `apply_ops`, after a transition finalize, and at the end of
    /// `build`/`rebuild` — so `NavigatorController::depth`/`can_pop`/
    /// `back_interest`/`transition` read authoritative values.
    fn publish_state(&mut self) {
        self.depth.set(self.pages.len());
        let top_policy = self.pages.last().map(|p| p.back).unwrap_or(BackPolicy::Pop);
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
    /// and (task 06) the semantics rule all read this one function rather than
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
    fn publish_transition_start(
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
    fn publish_transition_progress(&self, progress: f64, interactive: Option<bool>) {
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
    fn cancel_top(&mut self) {
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

    /// Steal the gesture from the top page into an interactive pop.
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
        let from_depth = self.pages.len() + 1;
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
        let held = initial_progress.clamp(0.0, 1.0);
        self.transition = Some(ActiveTransition {
            driver: TransitionDriver::Held { value: held },
            preset,
            is_pop: true,
            stashed: Some(popped),
            settle_spring,
            settled: false,
            interactive: true,
            restore_on_finalize: false,
            hero_leaving: HashMap::new(),
            hero_entering: HashMap::new(),
            // The swipe drives a held progress, not a theme-timed driver, so
            // there is no deferred timing to resolve at paint.
            pending_spec: None,
        });
        // Publication point: an interactive pop starts at whatever progress the
        // drag has already reached, and is flagged `interactive` until release.
        // Unlike `start_transition` this runs in the EVENT pass, so a build-time
        // observer sees the edge on the next frame's build (the paint-time reader
        // still sees it this frame).
        self.publish_transition_start(from_depth, held, true, true);
        self.needs_ime_clear = true;
    }

    /// Settle a released interactive pop toward completion (`1.0`) or cancellation
    /// (`0.0`), springing from the held progress with initial `progress_velocity`
    /// (progress units/s). A cancel flags the transition for
    /// [restore](Self::finalize_transition) rather than teardown.
    fn settle_interactive(&mut self, complete: bool, progress_velocity: f64) {
        let Some(t) = self.transition.as_mut() else {
            return;
        };
        let from = t.driver.value();
        let target = if complete { 1.0 } else { 0.0 };
        t.driver = settle_driver(t.settle_spring, from, progress_velocity, target);
        t.settled = false;
        t.restore_on_finalize = !complete;
        // The edge-swipe's own release path, publishing exactly what the public
        // `settle_transition` seam does: a spring, not a finger, drives it now.
        self.publish_transition_progress(from, Some(false));
    }

    /// Drive an in-progress interactive edge-swipe from a pointer event:
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
                // fast enough — the low-progress high-velocity case.
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

    /// Pop the top page (the shared body of [`NavOp::Pop`] and a
    /// [`BackPolicy::Pop`] back request), delivering `result` to the popped
    /// page's pusher-registered callback. A pop of the last/root page is a safe
    /// no-op (the navigator always keeps one page; the result payload is
    /// dropped). Transitions are preserved: an animated page animates out and is
    /// torn down on settle. Returns the accumulated dirtiness.
    fn apply_pop(&mut self, result: PopResult, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.pages.len() > 1 {
            self.cancel_top();
            // Disarm any pending edge-swipe: this pop shrinks the stack, so an arm
            // captured before it must not later steal an interactive pop against
            // the now-shallower stack (mirrors the `cancel_top` contract).
            self.edge.armed = false;
            let mut popped = self.pages.pop().expect("len checked > 1");
            let spec = popped.transition;
            if let Some(callback) = popped.on_result.take() {
                self.pending_results.push((callback, result));
            }
            self.needs_ime_clear = true;
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
                } => {
                    let spec = self.effective_spec(transition);
                    self.cancel_top();
                    // Disarm any pending edge-swipe: a structural stack mutation
                    // invalidates an arm captured against the pre-mutation stack
                    // (mirrors the capture/focus-clearing `cancel_top` contract).
                    self.edge.armed = false;
                    let view = builder();
                    let pod = crate::authoring::build_child(&view, ctx);
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
                    flags |= self.apply_pop(result, ctx);
                }
                NavOp::RequestBack => {
                    flags |= self.apply_request_back(ctx);
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
                    let pod = crate::authoring::build_child(&view, ctx);
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
                        });
                    }
                    self.needs_ime_clear = true;
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
fn paint_page_layer(
    pod: &mut ChildPod,
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    layer: Layer,
    area: Size,
) {
    pod.set_origin(Point::new(layer.dx, layer.dy));
    let alpha = layer.alpha.clamp(0.0, 1.0);
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
        let view = (self.initial)();
        let pod = crate::authoring::build_child(&view, ctx);
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
            cull_covered_builds: self.cull_covered_builds,
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
        // Keep the widget's default transition in sync with the view so an app can
        // change it live (per-op overrides always win over it).
        element.default_transition = self.default_transition;
        // Refresh the edge-swipe enable flag from the view too (live-configurable).
        element.pop_swipe_enabled = self.resolve_pop_swipe();
        // Same for the covered-build cull switch (default `false`).
        element.cull_covered_builds = self.cull_covered_builds;
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
            let entry = &mut element.pages[index];
            let next_view = (entry.builder)();
            flags |= crate::authoring::rebuild_child(&entry.view, &next_view, &mut entry.pod, ctx);
            entry.view = next_view;
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
        content_type: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Stack;
    use crate::test_support::RecordingScene;
    use frust_core::{FrameTime, PointerButton, PointerEvent, PointerPhase, RenderRoot, any};
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

    // --- Retained per-page widget state across push → pop. ---

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

    // --- The controller's depth slot tracks the stack through rebuilds, and
    //     `can_pop` mirrors it. ---

    #[test]
    fn controller_depth_and_can_pop_track_the_stack() {
        let controller: NavigatorController<()> = NavigatorController::new();

        // Before any navigator attaches, depth is 0 and can_pop is false (an
        // app with no navigator must let back exit).
        assert_eq!(controller.depth(), 0, "no navigator attached yet");
        assert!(!controller.can_pop(), "can_pop is false with no navigator");

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();

        // First rebuild seeds the root page: depth 1, still can't pop the root.
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 1, "root page published on build");
        assert!(!controller.can_pop(), "a single (root) page cannot pop");

        // Push B: depth 2, can_pop true (published at rebuild, when apply_ops runs).
        controller.push(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2, "push published through rebuild");
        assert!(controller.can_pop(), "a two-page stack can pop");

        // Push C: depth 3.
        controller.push(|| sized_page(30.0, 30.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 3);
        assert!(controller.can_pop());

        // Pop back down to the root: depth returns to 1, can_pop false again.
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2);
        assert!(controller.can_pop());

        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 1, "back at the root");
        assert!(!controller.can_pop(), "root again: pop is a no-op");

        // A pop at the root is a safe no-op — depth stays 1 (the widget's
        // len > 1 guard is authoritative, can_pop is advisory).
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.depth(),
            1,
            "pop-at-root does not remove the root"
        );
        assert!(!controller.can_pop());
    }

    // --- A pop result reaches the on_result callback with state. ---

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

    // --- Opaque-page paint culling. ---

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

    // --- A captured drag on the top page is cancelled on push. ---

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

    // --- A focused field's IME surface is cleared on push. ---

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
                    content_type: Default::default(),
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

    // --- An example-style stack driven through the facade
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
    // Back-request routing + per-page dismiss policy.
    // ---------------------------------------------------------------------

    // --- Pure: back_interest is depth>1 OR a non-Pop top policy.
    //     Tested directly so the depth-1-with-overlay case (which `can_pop`
    //     cannot express) is covered without needing a depth-1 overlay through
    //     the push API. ---
    #[test]
    fn compute_back_interest_covers_depth_and_policy() {
        // Root only: not poppable, plain Pop policy -> no interest (a root back
        // must bubble to the platform).
        assert!(!compute_back_interest(1, BackPolicy::Pop));
        // Depth 2 (plain pages): poppable -> interest.
        assert!(compute_back_interest(2, BackPolicy::Pop));
        // Root + a Veto overlay: not poppable, but the overlay claims back.
        assert!(compute_back_interest(1, BackPolicy::Veto));
        // Root + a dismissable overlay: same — it wants the press to animate out.
        assert!(compute_back_interest(1, BackPolicy::DismissAnimated));
    }

    // --- The controller publishes back_interest
    //     through a real rebuild for the reachable cases. ---
    #[test]
    fn back_interest_publishes_through_the_controller() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();

        // Root only: false.
        root.rebuild(&mut app, &mut state);
        assert!(!controller.back_interest(), "root only -> no back interest");

        // Depth 2 (a plain page): true (poppable).
        controller.push(|| sized_page(20.0, 20.0));
        root.rebuild(&mut app, &mut state);
        assert!(controller.back_interest(), "depth 2 -> back interest");

        // Root + a Veto overlay (pop back to root, then push a Veto page): still
        // depth 2 here, so this exercises the reachable overlay case.
        controller.pop();
        root.rebuild(&mut app, &mut state);
        controller.push_with_options(
            || sized_page(30.0, 30.0),
            PushOptions::transparent().back(BackPolicy::Veto),
        );
        root.rebuild(&mut app, &mut state);
        assert!(
            controller.back_interest(),
            "a Veto overlay claims back interest"
        );
    }

    // --- request_back on a plain (Pop-policy) stack pops one page,
    //     and the pop transition is preserved (routes through the same animated
    //     pop path as `pop()`). ---
    #[test]
    fn request_back_pops_plain_page_preserving_transition() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push B with an animated iOS transition, then settle the push.
        controller.push_with(
            || sized_page(100.0, 60.0),
            TransitionSpec::new(
                PageTransition::IosPush,
                Timing::Duration(Duration::from_millis(100), Curve::Linear),
            ),
        );
        for t in [0u64, 50, 150, 300] {
            full_frame(&mut root, &mut app, &mut state, ft(t));
        }
        assert_eq!(controller.depth(), 2, "B pushed");

        // A back request routes through the popped page's own transition: both
        // pages paint during the animated pop (transition preserved).
        controller.request_back();
        let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(1000));
        assert!(
            f0.iter().any(|(_, s, _)| (s.height - 100.0).abs() < 1e-9),
            "A paints during the pop"
        );
        assert!(
            f0.iter().any(|(_, s, _)| (s.height - 60.0).abs() < 1e-9),
            "B (leaving) still paints during the pop"
        );

        // Drive to completion: only A remains (depth back to 1).
        for t in [1050u64, 1150, 1300] {
            full_frame(&mut root, &mut app, &mut state, ft(t));
        }
        assert_eq!(controller.depth(), 1, "request_back popped one page");
    }

    // --- request_back at the root (Pop policy, depth 1) is a safe
    //     no-op. ---
    #[test]
    fn request_back_at_root_is_a_noop() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 1);

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 1, "root back does not pop the root");
    }

    // --- A DismissAnimated top page leaves the stack unchanged and
    //     fires its observable dismiss signal exactly once per request. ---
    #[test]
    fn request_back_dismiss_animated_fires_signal_once_no_pop() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // Push a transparent, dismissable overlay carrying a shared dismiss
        // signal (the seam the overlay widget would observe).
        let signal = Rc::new(Cell::new(0u64));
        controller.push_with_options(
            || sized_page(20.0, 20.0),
            PushOptions::transparent()
                .back(BackPolicy::DismissAnimated)
                .dismiss_signal(signal.clone()),
        );
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2, "overlay pushed");
        assert_eq!(signal.get(), 0, "no back yet");

        // First back request: signal fires once, stack unchanged.
        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2, "DismissAnimated does not pop");
        assert_eq!(signal.get(), 1, "signal fired exactly once");

        // Second back request: fires again (once more), still no pop.
        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2);
        assert_eq!(signal.get(), 2, "signal fired once per request");
    }

    // --- A Veto top page consumes the press without changing the
    //     stack and without firing any signal. ---
    #[test]
    fn request_back_veto_consumes_without_change() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // A non-dismissable overlay: Veto policy, still carrying a signal to prove
        // it is NOT fired.
        let signal = Rc::new(Cell::new(0u64));
        controller.push_with_options(
            || sized_page(20.0, 20.0),
            PushOptions::transparent()
                .back(BackPolicy::Veto)
                .dismiss_signal(signal.clone()),
        );
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2);

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2, "Veto leaves the stack unchanged");
        assert_eq!(signal.get(), 0, "Veto fires no dismiss signal");
    }

    // ---------------------------------------------------------------------
    // Page-transition machinery.
    // ---------------------------------------------------------------------

    use super::super::transition::{PageTransition, Timing, TransitionSpec};
    use frust_core::Curve;
    use frust_theme::{MotionSpring, Theme};
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
    /// `State` so a result-carrying transition test can share it too.
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

    // --- Push animates both pages with moving origins, settling at
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

    // --- Input is blocked mid-transition; no page receives the Down
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

    // --- The below page's secondary animation (iOS push parallax +
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

    // --- A spatial spring overshoots position, never opacity. ---

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
    // ThemeDefault / reduce_motion / Layer::scale wiring through the REAL
    // navigator.
    // ---------------------------------------------------------------------

    // --- A `ThemeDefault` Glyph push resolves the *enter duration* from
    //     the active MotionScheme (m3_baseline's `slow` = 500ms), not the 300ms
    //     unthemed M3 fallback. Proven by the transition still running at 340ms
    //     under the theme, where the unthemed control has already finalized. ---

    #[test]
    fn glyph_theme_default_resolves_enter_duration_from_scheme() {
        // Themed: push `TransitionSpec::glyph()` (ThemeDefault timing) under the
        // m3_baseline theme whose Glyph enter timing is the scheme's slow = 500ms.
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.set_theme(Box::new(Theme::m3_baseline()));
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        controller.push_with(|| sized_page(100.0, 80.0), TransitionSpec::glyph());
        full_frame(&mut root, &mut app, &mut state, ft(0)); // seed + resolve
        let (_f1, nf1) = full_frame(&mut root, &mut app, &mut state, ft(320));
        let (_f2, nf2) = full_frame(&mut root, &mut app, &mut state, ft(340));
        assert!(
            nf1 && nf2,
            "the theme-resolved 500ms enter duration is still running at 320/340ms"
        );

        // Control: the identical push with NO theme threaded falls back to the
        // 300ms M3 default, which settles (ft 320) and finalizes (ft 340) — so it
        // requests no frame at 340ms. The divergence proves the theme resolution
        // changed the enter duration (300ms → 500ms) at first paint.
        let ctrl2: NavigatorController<()> = NavigatorController::new();
        let mut root2: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app2 = {
            let c = ctrl2.clone();
            move |_: &mut ()| navigator(&c, || sized_page(100.0, 100.0))
        };
        let mut s2 = ();
        root2.rebuild(&mut app2, &mut s2);
        root2.layout(Size::new(100.0, 100.0));
        ctrl2.push_with(|| sized_page(100.0, 80.0), TransitionSpec::glyph());
        full_frame(&mut root2, &mut app2, &mut s2, ft(0));
        full_frame(&mut root2, &mut app2, &mut s2, ft(320)); // settle
        let (_c2, nfc) = full_frame(&mut root2, &mut app2, &mut s2, ft(340)); // finalize
        assert!(
            !nfc,
            "the unthemed 300ms M3 fallback has finalized by 340ms (no frame requested)"
        );
    }

    // --- A Glyph push under a `reduce_motion` MotionScheme collapses per
    //     `resolve_spec`'s contract — the 16px directional Glyph slide becomes the
    //     non-directional M3 fade-through crossfade (no slide). ---

    #[test]
    fn reduce_motion_collapses_glyph_push_to_crossfade() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut reduced = Theme::m3_baseline();
        reduced.motion.reduce_motion = true;
        root.set_theme(Box::new(reduced));
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // A Glyph push normally slides the incoming page in by 16px at p=0; under
        // reduce_motion it must collapse to the crossfade family and NOT slide.
        controller.push_with(|| sized_page(100.0, 80.0), TransitionSpec::glyph());
        let (f0, _) = full_frame(&mut root, &mut app, &mut state, ft(0));
        assert_eq!(
            fill_h(&f0, 80.0).0.x,
            0.0,
            "reduce_motion collapses the 16px Glyph slide to a non-directional crossfade"
        );

        // Scale-leak guard: the reduced crossfade must also paint
        // ZERO scale transforms mid-transition — a reduce_motion user never sees
        // the fade-through 0.92→1.0 zoom.
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, ft(60));
        assert_eq!(
            scene.transforms.len(),
            0,
            "reduced-motion crossfade must not scale either page"
        );
    }

    // --- A mid-transition M3 fade-through paint brackets the incoming
    //     page with a `push_transform` scale < 1.0 (the 0.92 → 1.0
    //     scale-up), balanced LIFO; the leaving page (scale 1.0) pushes none. ---

    #[test]
    fn fade_through_paint_scales_incoming_page_below_one() {
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
        controller.push_with(|| sized_page(100.0, 80.0), spec);

        // Seed frame (p=0): the incoming page holds at the 0.92 fade-through start
        // scale, so exactly one sub-unit `push_transform` brackets its paint.
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, ft(0));
        assert_eq!(
            scene.transforms.len(),
            1,
            "only the incoming (scale 0.92) page is bracketed, not the leaving one"
        );
        let sx = scene.transforms[0].as_coeffs()[0];
        assert!(
            sx < 1.0 && sx > 0.9,
            "the incoming page is scaled below 1.0 (was {sx})"
        );
        assert_eq!(
            scene.transform_pops, 1,
            "the scale bracket is balanced (strict LIFO)"
        );
    }

    // ---------------------------------------------------------------------
    // SlideUp preset + push_transparent_for_result.
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
    // Interactive edge-swipe back gesture.
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
    /// `TransitionScene`/`full_frame`/`fill_h`/`ft` helpers above.
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

    // --- An edge drag past slop steals from a capturing child. ---

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

    // --- Drag moves pages, origins tracking progress. ---

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

    // --- Release past the halfway point completes the pop. ---

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

    // --- An interactive edge-swipe pop drives the popped page's own
    //     `Custom` preset RAW, bypassing `resolve_spec`'s `reduce_motion`
    //     collapse — unlike a programmatic push/pop. This lives here (not in
    //     `transition.rs`'s test module) because driving an interactive pop
    //     needs this module's private test harness (`NavigatorController`,
    //     `down`/`move_to`, `full_frame`, `nav_widget`) — the interactive-pop
    //     path is not reachable from `transition.rs` at all. See
    //     `PageTransition::Custom`'s doc comment and
    //     `transition.rs`'s `custom_preset_collapses_under_reduce_motion_on_the_resolve_spec_path`
    //     for the contrasting programmatic-path case. ---

    #[test]
    fn interactive_pop_calls_custom_fn_under_reduce_motion() {
        // A plain `fn` pointer can't capture, so invocation is recorded
        // through a process-static flag — fine here since this exact
        // function is only ever installed/read by this one test.
        static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn recording_custom(_p: f64, _is_pop: bool, _size: Size) -> (Layer, Layer) {
            CALLED.store(true, std::sync::atomic::Ordering::SeqCst);
            (Layer::IDENTITY, Layer::IDENTITY)
        }

        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut reduced = Theme::m3_baseline();
        reduced.motion.reduce_motion = true;
        root.set_theme(Box::new(reduced));
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Push B with a `Custom` transition (explicit duration, so the
        // collapse below is attributable to `reduce_motion` alone). This
        // push's own animation still resolves through `resolve_spec` at its
        // first paint (the `pending_spec` seam `paint_transition` drains)
        // and collapses to `ReducedCrossfade` as documented — the caller's
        // fn must NOT run for the push itself.
        controller.push_with(
            || sized_page(100.0, 80.0),
            TransitionSpec::new(
                PageTransition::Custom(recording_custom),
                Timing::Duration(Duration::from_millis(80), Curve::Linear),
            ),
        );
        run_until_settled(&mut root, &mut app, &mut state, 0);
        assert!(
            !CALLED.load(std::sync::atomic::Ordering::SeqCst),
            "the programmatic push collapsed to ReducedCrossfade — Custom's fn must not run"
        );
        assert!(
            nav_widget(&root).transition.is_none(),
            "the push transition finalized before the swipe begins"
        );

        // Drive an interactive edge-swipe pop of B. `begin_interactive_pop`
        // takes `spec.preset` (`Custom`) RAW and sets `pending_spec: None`,
        // so `resolve_spec`'s `reduce_motion` collapse never runs on this
        // path — `paint_transition` calls `resolve_layers` with the
        // unresolved `Custom` preset directly.
        let mut scene = TransitionScene::default();
        root.event(&mut state, &down(5.0, 50.0));
        root.paint(&mut scene, ft(1000));
        root.event(&mut state, &move_to(40.0, 50.0)); // past slop -> steals
        assert!(
            nav_widget(&root).transition.is_some(),
            "an interactive pop began"
        );

        full_frame(&mut root, &mut app, &mut state, ft(1016));
        assert!(
            CALLED.load(std::sync::atomic::Ordering::SeqCst),
            "an interactive edge-swipe pop drives the popped page's own Custom \
             preset directly under reduce_motion — the caller's fn IS called"
        );
    }

    // --- A completed swipe delivers the pop result. ---

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

    // --- Release below threshold cancels; page restored exactly. ---

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

    // --- A low-progress high-velocity release completes the pop. ---

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

    // --- A non-edge drag never arms the gesture. ---

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

    // --- A vertical drag starting in the edge zone stays with the
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

    // --- A depth-1 stack disables the gesture. ---

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

    // --- Regression: a programmatic instant pop between an edge-swipe arm
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

    // --- Pop-swipe defaults on for the iOS-push preset. ---

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
    // Shared-element ("hero") transitions.
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

    // --- #40: the published TransitionState, observed from OUTSIDE the
    //     navigator subtree (the case the seam exists for). ---

    /// What a chrome probe stacked above the navigator saw, per pass.
    #[derive(Default)]
    struct ProbeLog {
        /// `TransitionState::active` as read from each `View::build`/`rebuild`
        /// (a `BuildCtx` pass).
        build_active: Vec<bool>,
        /// `TransitionState::progress` as read from each `Widget::paint`.
        paint_progress: Vec<f64>,
        /// `TransitionState::generation` as read from each `Widget::paint`.
        paint_generation: Vec<u32>,
    }

    /// A zero-content "chrome" widget that only *observes* the navigator through
    /// a controller clone — the sibling-not-descendant position #40 is about.
    struct ProbeView {
        controller: NavigatorController<()>,
        log: Rc<RefCell<ProbeLog>>,
    }
    struct ProbeWidget {
        controller: NavigatorController<()>,
        log: Rc<RefCell<ProbeLog>>,
    }
    impl ProbeView {
        fn record_build(&self) {
            let active = self.controller.transition().active;
            self.log.borrow_mut().build_active.push(active);
        }
    }
    impl View<()> for ProbeView {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            self.record_build();
            ProbeWidget {
                controller: self.controller.clone(),
                log: self.log.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            self.record_build();
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(1.0, 1.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            let t = self.controller.transition();
            let mut log = self.log.borrow_mut();
            log.paint_progress.push(t.progress);
            log.paint_generation.push(t.generation);
        }
    }

    /// One full frame of a `Stack(vec![navigator, probe])` root, returning the
    /// navigator's recorded page fills plus whether another frame was requested.
    fn probe_frame(
        root: &mut RenderRoot<(), AnyView<()>>,
        app: &mut impl FnMut(&mut ()) -> AnyView<()>,
        state: &mut (),
        time: FrameTime,
    ) -> (Vec<(Point, Size, f32)>, bool) {
        root.rebuild(app, state);
        root.layout(Size::new(100.0, 100.0));
        let mut scene = TransitionScene::default();
        let out = root.paint(&mut scene, time);
        (scene.fills, out.needs_frame)
    }

    #[test]
    fn transition_state_is_frame_exact_for_chrome_painted_after_the_navigator() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let log = Rc::new(RefCell::new(ProbeLog::default()));

        // Before any navigator attaches: the at-rest default.
        let idle = controller.transition();
        assert!(!idle.active, "no navigator attached yet");
        assert_eq!(idle.generation, 0);

        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let log = log.clone();
            move |_: &mut ()| {
                any(Stack(vec![
                    any(navigator(&ctrl, || sized_page(100.0, 100.0))),
                    any(ProbeView {
                        controller: ctrl.clone(),
                        log: log.clone(),
                    }),
                ]))
            }
        };
        let mut state = ();

        probe_frame(&mut root, &mut app, &mut state, ft(0));
        let at_rest = controller.transition();
        assert!(
            !at_rest.active,
            "a settled navigator publishes an idle state"
        );
        assert_eq!(
            (at_rest.from_depth, at_rest.to_depth),
            (1, 1),
            "an idle snapshot's endpoints are both the current stack"
        );

        // Push B on a 100ms linear shared-axis transition: 30dp of entering slide
        // makes the navigator's own painted progress recoverable from the fills.
        let spec = TransitionSpec::new(
            PageTransition::M3SharedAxisX,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        let builds_before = log.borrow().build_active.len();
        controller.push_with(|| sized_page(100.0, 80.0), spec);

        let mut painted = Vec::new();
        for ms in [0u64, 25, 50, 75] {
            let (fills, needs) = probe_frame(&mut root, &mut app, &mut state, ft(ms));
            assert!(needs, "a running transition keeps requesting frames");
            painted.push(fill_h(&fills, 80.0).0.x);
        }

        let log = log.borrow();
        // (a) The BUILD-time edge: the probe builds after the navigator, and
        //     `start_transition` publishes from that same BuildCtx pass, so
        //     `active` is visible on the very frame of the push.
        assert!(
            log.build_active[builds_before],
            "a build-time reader sees `active` flip on the push frame: {:?}",
            log.build_active
        );

        // (b) The PAINT-time reads: exactly the value the navigator painted with.
        //     Entering dx is `30 * (1 - progress)` for a shared-axis push, so
        //     inverting the geometry recovers the navigator's own progress.
        let reads = &log.paint_progress[log.paint_progress.len() - painted.len()..];
        for (i, (&p, &x)) in reads.iter().zip(painted.iter()).enumerate() {
            assert!(
                (30.0 * (1.0 - p) - x).abs() < 1e-9,
                "frame {i}: probe read {p} but the navigator painted at x={x}"
            );
        }
        // (c) Strictly increasing across frames.
        for pair in reads.windows(2) {
            assert!(
                pair[1] > pair[0],
                "progress advances monotonically: {reads:?}"
            );
        }
        // (d) One transition = one generation, bumped exactly once.
        let generations = &log.paint_generation[log.paint_generation.len() - painted.len()..];
        assert!(
            generations.iter().all(|g| *g == 1),
            "one started transition = generation 1 throughout: {generations:?}"
        );
        drop(log);

        // Settle, then finalize: `active` falls, the depths collapse onto the new
        // stack, and the generation is retained so an observer can still tell
        // WHICH transition ended.
        probe_frame(&mut root, &mut app, &mut state, ft(150));
        let (_, needs) = probe_frame(&mut root, &mut app, &mut state, ft(200));
        assert!(!needs, "the transition finalized");
        let done = controller.transition();
        assert!(!done.active);
        assert!(!done.interactive);
        assert_eq!((done.from_depth, done.to_depth), (2, 2));
        assert_eq!(done.generation, 1);
        assert_eq!(done.progress, 1.0, "a settled snapshot is fully arrived");
    }

    #[test]
    fn transition_state_marks_a_pop_and_survives_an_instant_stack_mutation() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // An INSTANT (non-animated) push starts no transition at all, but
        // `publish_state` still leaves a coherent snapshot on the new depth.
        controller.push(|| sized_page(100.0, 80.0));
        root.rebuild(&mut app, &mut state);
        let t = controller.transition();
        assert!(!t.active, "an instant push animates nothing");
        assert_eq!((t.from_depth, t.to_depth), (2, 2));
        assert_eq!(t.generation, 0, "no transition started, no generation bump");

        // An ANIMATED pop publishes `is_pop` with the shrinking depth pair.
        let spec = TransitionSpec::new(
            PageTransition::M3SharedAxisX,
            Timing::Duration(Duration::from_millis(100), Curve::Linear),
        );
        controller.replace_with(|| sized_page(100.0, 80.0), spec);
        full_frame(&mut root, &mut app, &mut state, ft(0));
        let t = controller.transition();
        assert!(t.active);
        assert!(!t.is_pop, "a replace animates in the push direction");
        assert_eq!(
            (t.from_depth, t.to_depth),
            (2, 2),
            "a replace swaps in place: the depth is unchanged"
        );
        // Let it finish so the pop below starts clean.
        full_frame(&mut root, &mut app, &mut state, ft(150));
        full_frame(&mut root, &mut app, &mut state, ft(200));

        controller.pop();
        full_frame(&mut root, &mut app, &mut state, ft(216));
        let t = controller.transition();
        assert!(t.active);
        assert!(t.is_pop, "a pop of an animated page runs backwards");
        assert!(!t.interactive, "a programmatic pop is not a drag");
        assert_eq!((t.from_depth, t.to_depth), (2, 1));
        assert_eq!(t.generation, 2, "second transition of this navigator");
    }

    #[test]
    fn transition_state_tracks_the_interactive_edge_swipe() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized_page(100.0, 100.0)).pop_swipe(true)
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        controller.push(|| sized_page(100.0, 80.0));
        full_frame(&mut root, &mut app, &mut state, ft(0));

        // Arm on the left edge, then steal with a decisive rightward drag.
        root.event(&mut state, &down(5.0, 50.0));
        root.event(&mut state, &move_to(45.0, 50.0));
        let held = controller.transition();
        assert!(held.active, "the steal started a transition");
        assert!(held.is_pop);
        assert!(held.interactive, "a drag is holding the progress");
        assert_eq!((held.from_depth, held.to_depth), (2, 1));
        assert!(held.progress > 0.0, "the drag carried initial progress");

        // Dragging further advances the published progress in place.
        root.event(&mut state, &move_to(70.0, 50.0));
        let further = controller.transition();
        assert!(further.progress > held.progress);
        assert!(further.interactive);
        assert_eq!(
            further.generation, held.generation,
            "the same transition, later — not a new one"
        );

        // Release: a spring drives it from here, so `interactive` clears while the
        // transition stays active.
        root.event(&mut state, &up(70.0, 50.0));
        let released = controller.transition();
        assert!(released.active, "the settle is still a live transition");
        assert!(!released.interactive, "the finger let go");
    }

    // --- #28: page visibility. ---

    type VisLog = Rc<RefCell<Vec<(&'static str, PageVisibility)>>>;

    #[test]
    fn on_visibility_reports_current_covered_current_and_never_repeats() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let seen: VisLog = Rc::new(RefCell::new(Vec::new()));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let seen = seen.clone();
            move |_: &mut ()| {
                let seen = seen.clone();
                navigator(&ctrl, || sized_page(100.0, 100.0))
                    .on_root_visibility(move |v| seen.borrow_mut().push(("root", v)))
            }
        };
        let mut state = ();

        // The root page's opening observation fires on the navigator's build.
        root.rebuild(&mut app, &mut state);
        assert_eq!(*seen.borrow(), vec![("root", PageVisibility::Current)]);

        // Idle frames repeat nothing.
        root.rebuild(&mut app, &mut state);
        root.rebuild(&mut app, &mut state);
        assert_eq!(seen.borrow().len(), 1, "a value is never reported twice");

        // A TRANSPARENT push (a dialog) leaves the root painted → `Visible`.
        {
            let seen = seen.clone();
            controller.push_with_options(
                || sized_page(50.0, 50.0),
                PushOptions::transparent()
                    .on_visibility(move |v| seen.borrow_mut().push(("dialog", v))),
            );
        }
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            *seen.borrow(),
            vec![
                ("root", PageVisibility::Current),
                ("root", PageVisibility::Visible),
                ("dialog", PageVisibility::Current),
            ],
        );

        // An OPAQUE push over both covers the root and demotes the dialog.
        {
            let seen = seen.clone();
            controller.push_with_options(
                || sized_page(100.0, 60.0),
                PushOptions::opaque().on_visibility(move |v| seen.borrow_mut().push(("b", v))),
            );
        }
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            seen.borrow()[3..],
            [
                ("root", PageVisibility::Covered),
                ("dialog", PageVisibility::Covered),
                ("b", PageVisibility::Current),
            ],
        );

        // Popping B reveals both again, in stack order.
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            seen.borrow()[6..],
            [
                ("root", PageVisibility::Visible),
                ("dialog", PageVisibility::Current),
            ],
        );

        // Popping the dialog restores the root to `Current`.
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(seen.borrow()[8..], [("root", PageVisibility::Current)]);
        assert_eq!(seen.borrow().len(), 9, "no extra observations anywhere");
    }

    /// Count how many times the ROOT page's builder runs, with the covered-build
    /// cull either on or off. Returns `(counter, root, app)` ready to drive.
    #[allow(clippy::type_complexity)]
    fn cull_harness(
        controller: &NavigatorController<()>,
        cull: bool,
    ) -> (
        Rc<Cell<u32>>,
        RenderRoot<(), NavigatorView<()>>,
        impl FnMut(&mut ()) -> NavigatorView<()>,
    ) {
        let builds = Rc::new(Cell::new(0u32));
        let root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let app = {
            let ctrl = controller.clone();
            let builds = builds.clone();
            move |_: &mut ()| {
                let builds = builds.clone();
                navigator(&ctrl, move || {
                    builds.set(builds.get() + 1);
                    sized_page(100.0, 100.0)
                })
                .cull_covered_builds(cull)
            }
        };
        (builds, root, app)
    }

    #[test]
    fn cull_covered_builds_freezes_a_covered_page_and_resumes_on_the_reveal_frame() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let (builds, mut root, mut app) = cull_harness(&controller, true);
        let mut state = ();

        root.rebuild(&mut app, &mut state); // build seeds the root page
        root.rebuild(&mut app, &mut state);
        let before_cover = builds.get();

        // The covering push: the root learns it is `Covered` BEFORE the reconcile
        // loop, but still gets exactly one final rebuild on this frame (so a page
        // staging teardown UI on cover can render it).
        controller.push(|| sized_page(100.0, 60.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            builds.get(),
            before_cover + 1,
            "the frame a page becomes covered still rebuilds it"
        );

        // From here the builder is frozen.
        let frozen = builds.get();
        root.rebuild(&mut app, &mut state);
        root.rebuild(&mut app, &mut state);
        root.rebuild(&mut app, &mut state);
        assert_eq!(builds.get(), frozen, "a covered page stops rebuilding");

        // The pop is applied before the reconcile loop, so the revealed page
        // rebuilds on the SAME frame — nothing has to wake it.
        controller.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            builds.get(),
            frozen + 1,
            "a revealed page rebuilds in the pass that revealed it"
        );
        root.rebuild(&mut app, &mut state);
        assert_eq!(builds.get(), frozen + 2, "and keeps rebuilding after that");
    }

    #[test]
    fn cull_covered_builds_defaults_off() {
        let controller: NavigatorController<()> = NavigatorController::new();
        // Same harness, `false` — which is also what plain `navigator(...)` gives:
        // the shipped every-page-every-frame behaviour is unchanged.
        let (builds, mut root, mut app) = cull_harness(&controller, false);
        let mut state = ();

        root.rebuild(&mut app, &mut state);
        controller.push(|| sized_page(100.0, 60.0));
        root.rebuild(&mut app, &mut state);
        let covered_at = builds.get();
        root.rebuild(&mut app, &mut state);
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            builds.get(),
            covered_at + 2,
            "by default a covered page still reconciles every frame"
        );

        // And the default really is `false` on a bare navigator.
        let bare: NavigatorView<()> = navigator(&controller, || sized_page(1.0, 1.0));
        assert!(!bare.cull_covered_builds);
    }

    #[test]
    fn a_covered_page_keeps_its_widget_state_under_the_build_cull() {
        // The "no cleanup on cover" contract, pinned against the opt-in cull:
        // freezing a page's builder must not dispose it.
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed = Rc::new(Cell::new(0u32));
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let obs = observed.clone();
            move |_: &mut ()| {
                let obs = obs.clone();
                navigator(&ctrl, move || counter_page(&obs)).cull_covered_builds(true)
            }
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        // Tap the root page → its retained counter is 1.
        root.event(&mut state, &down(5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1);

        // Cover it for several frames, then reveal: the retained pod (and thus the
        // count) survived — a covering push never runs `on_cleanup`.
        controller.push(|| sized_page(100.0, 60.0));
        for _ in 0..4 {
            root.rebuild(&mut app, &mut state);
            root.layout(Size::new(100.0, 100.0));
            root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        }
        controller.pop();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(
            observed.get(),
            1,
            "the covered page's widget state survived the build cull"
        );
    }

    // ---------------------------------------------------------------------
    // #44 — the root overlay host (`overlay_host`).
    // ---------------------------------------------------------------------

    /// A full-screen stand-in for one piece of the app root (its content, its
    /// chrome) or for an overlay pushed over it. It fills its whole box, counts
    /// the pointer `Down`s that actually reach it, and contributes exactly one
    /// labelled accessibility node — so a test reads the input reach and the
    /// accessibility reach of the same thing as two sets of labels (the shape
    /// `tests/semantics_tree.rs`'s R23 parity helper uses).
    struct HostProbe {
        label: &'static str,
        hits: Rc<Cell<u32>>,
    }
    struct HostProbeWidget {
        label: &'static str,
        hits: Rc<Cell<u32>>,
    }
    impl View<()> for HostProbe {
        type Element = HostProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> HostProbeWidget {
            HostProbeWidget {
                label: self.label,
                hits: self.hits.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut HostProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.label = self.label;
            element.hits = self.hits.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for HostProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        }
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                self.hits.set(self.hits.get() + 1);
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(frust_core::accesskit::Role::Button, |node| {
                node.set_label(self.label)
            });
        }
    }

    fn host_probe(label: &'static str, hits: &Rc<Cell<u32>>) -> AnyView<()> {
        any(HostProbe {
            label,
            hits: hits.clone(),
        })
    }

    /// Every label present anywhere in the collected accessibility tree.
    fn semantic_labels(root: &RenderRoot<(), NavigatorView<()>>) -> Vec<String> {
        root.semantics()
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().map(str::to_string))
            .collect()
    }

    /// The app-root shape a host wraps: `Stack[content, chrome]`, both
    /// full-screen, so the chrome paints over the content and — by
    /// `StackWidget`'s reverse-order hit-testing — takes any pointer that
    /// reaches the app root at all.
    type HostAppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;
    type HostFixture = (
        NavigatorController<()>,
        RenderRoot<(), NavigatorView<()>>,
        HostAppLogic,
        Rc<Cell<u32>>,
        Rc<Cell<u32>>,
    );

    fn overlay_host_fixture() -> HostFixture {
        let controller: NavigatorController<()> = NavigatorController::new();
        let content_hits = Rc::new(Cell::new(0u32));
        let chrome_hits = Rc::new(Cell::new(0u32));
        let app: HostAppLogic = {
            let ctrl = controller.clone();
            let content = content_hits.clone();
            let chrome = chrome_hits.clone();
            Box::new(move |_: &mut ()| {
                let content = content.clone();
                let chrome = chrome.clone();
                overlay_host(&ctrl, move || {
                    any(Stack(vec![
                        host_probe("app-content", &content),
                        host_probe("app-chrome", &chrome),
                    ]))
                })
            })
        };
        (
            controller,
            RenderRoot::new(),
            app,
            content_hits,
            chrome_hits,
        )
    }

    /// The host is the ordinary navigator with exactly two defaults changed —
    /// asserted on the view, and then behaviourally: a left-edge drag over an
    /// open overlay must not dismiss it (an edge swipe is a navigation gesture,
    /// never a modal dismissal), and the overlay must appear with no host
    /// transition of its own (each overlay stages its own enter/exit).
    #[test]
    fn overlay_host_is_a_navigator_with_two_defaults_changed() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let view = overlay_host(&controller, || sized_page(10.0, 10.0));
        assert_eq!(
            view.pop_swipe,
            Some(false),
            "the host pins pop_swipe off explicitly, not by default-derivation"
        );
        assert!(!view.resolve_pop_swipe());
        assert_eq!(view.default_transition, TransitionSpec::NONE);

        let (controller, mut root, mut app, ..) = overlay_host_fixture();
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        controller.push_with_options(|| sized_page(1e4, 1e4), PushOptions::transparent());
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        // No host transition: the overlay is settled on the very frame it is
        // pushed, so it takes input immediately rather than after ≤340ms of
        // mid-transition input suppression.
        assert!(
            !controller.transition().active,
            "TransitionSpec::NONE: the overlay push settles instantly"
        );
        assert_eq!(controller.depth(), 2);

        // A full left-edge drag: arm at x <= EDGE_SWIPE_ZONE_DP, cross the slop,
        // release well past the commit threshold.
        root.event(&mut state, &down(2.0, 50.0));
        root.event(&mut state, &move_to(80.0, 50.0));
        root.event(&mut state, &up(90.0, 50.0));
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.depth(),
            2,
            "pop_swipe(false): an edge swipe never dismisses an overlay"
        );
    }

    /// The chrome-inertness half of #44, and the reason the host must sit
    /// *above* the chrome rather than beside it: with an overlay up, a pointer
    /// at the chrome's own coordinates reaches the overlay. No new suppression
    /// code — `route_top` already routes to `input_routed_pages()` only.
    #[test]
    fn an_overlay_takes_the_pointer_from_the_chrome_beneath_it() {
        let (controller, mut root, mut app, content_hits, chrome_hits) = overlay_host_fixture();
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // Baseline: with no overlay, a press at (50, 50) lands on the chrome.
        root.event(&mut state, &down(50.0, 50.0));
        assert_eq!(chrome_hits.get(), 1, "no overlay: the chrome takes presses");
        assert_eq!(content_hits.get(), 0, "the chrome is above the content");

        let overlay_hits = Rc::new(Cell::new(0u32));
        controller.push_with_options(
            {
                let overlay_hits = overlay_hits.clone();
                move || host_probe("overlay", &overlay_hits)
            },
            PushOptions::transparent(),
        );
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        chrome_hits.set(0);
        content_hits.set(0);
        root.event(&mut state, &down(50.0, 50.0));
        assert_eq!(overlay_hits.get(), 1, "the overlay takes the press");
        assert_eq!(
            chrome_hits.get(),
            0,
            "the chrome under a root overlay is inert — the whole point of #44"
        );
        assert_eq!(content_hits.get(), 0, "so is the app content");
    }

    /// The host owns no scrim, and does not need to: an overlay page already
    /// fills `ctx.origin()..ctx.size()`, and at the ROOT that rect is the
    /// window. This is what lets `glyph::dialog`/`material::sheet` dim the whole
    /// app with no change at all.
    #[test]
    fn an_overlays_scrim_rect_equals_the_window_rect() {
        let (controller, mut root, mut app, ..) = overlay_host_fixture();
        let mut state = ();
        let window = Size::new(320.0, 640.0);
        root.rebuild(&mut app, &mut state);
        root.layout(window);

        // A page that fills whatever box it is given — the scrim shape every
        // overlay catalog widget paints first.
        controller.push_with_options(|| sized_page(1e4, 1e4), PushOptions::transparent());
        root.rebuild(&mut app, &mut state);
        root.layout(window);
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);

        assert!(
            scene.rects.contains(&(Point::ZERO, window)),
            "the overlay's scrim covers the whole window, not a sub-rect \
             (recorded rects: {:?})",
            scene.rects
        );
    }

    /// **R23 parity at the root.** With an overlay up, the app root and its
    /// chrome contribute NO accessibility nodes — asserted, not assumed, and
    /// asserted *together with* the input reach so the two can only ever agree.
    /// A root overlay that dimmed the chrome visually while a screen reader
    /// still read it out would be #44 re-created inside the accessibility tree.
    #[test]
    fn an_overlay_host_omits_the_app_root_and_its_chrome_from_semantics() {
        let (controller, mut root, mut app, content_hits, chrome_hits) = overlay_host_fixture();
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // With no overlay, the app root IS the routed page: both reaches carry
        // the content and the chrome (the #23 regression guard, at the root).
        let labels = semantic_labels(&root);
        assert!(
            labels.iter().any(|l| l == "app-content") && labels.iter().any(|l| l == "app-chrome"),
            "no overlay: the whole app root is in the accessibility tree ({labels:?})"
        );

        let overlay_hits = Rc::new(Cell::new(0u32));
        controller.push_with_options(
            {
                let overlay_hits = overlay_hits.clone();
                move || host_probe("overlay", &overlay_hits)
            },
            PushOptions::transparent(),
        );
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));

        // The accessibility reach…
        let labels = semantic_labels(&root);
        assert!(
            labels.iter().any(|l| l == "overlay"),
            "the overlay itself is present ({labels:?})"
        );
        assert!(
            !labels.iter().any(|l| l == "app-content") && !labels.iter().any(|l| l == "app-chrome"),
            "R23: the app root and its chrome contribute nothing under an \
             overlay ({labels:?})"
        );

        // …equals the input reach, measured the same way.
        content_hits.set(0);
        chrome_hits.set(0);
        root.event(&mut state, &down(50.0, 50.0));
        assert_eq!(overlay_hits.get(), 1);
        assert_eq!(
            (content_hits.get(), chrome_hits.get()),
            (0, 0),
            "R23 parity: what contributes no node is exactly what receives no input"
        );
    }
}
