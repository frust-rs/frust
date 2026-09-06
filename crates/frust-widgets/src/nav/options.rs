//! The navigator's option and vocabulary types: the page/callback type
//! aliases, [`PageVisibility`], [`PopResult`], [`BackPolicy`], [`PushOptions`],
//! [`ReplaceOptions`], the queued [`NavOp`], and [`NavigatorId`].
//!
//! [`navigator`](super::navigator) re-exports every public name here, so the
//! `nav::navigator::*` paths callers already use keep resolving.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use frust_core::AnyView;

use super::path::Location;
use super::route_state::RouteStack;
use super::transition::TransitionSpec;

// Named only by the doc comments moved here with this module's items, so their
// intra-doc links keep resolving to the same targets they did in `navigator`.
#[allow(unused_imports)]
use super::controller::NavigatorController;
#[allow(unused_imports)]
use super::navigator::NavigatorWidget;
#[allow(unused_imports)]
use super::view::NavigatorView;

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

/// A route-change observer, registered navigator-wide via
/// [`NavigatorView::on_route_change`]. Fired from [`NavigatorWidget::publish_state`]
/// only when the published [`RouteStack`] actually changed — see
/// `route_state`'s module docs.
pub type RouteChangeCallback = Rc<dyn Fn(&RouteStack)>;

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
    pub(super) opaque: bool,
    pub(super) back: BackPolicy,
    pub(super) transition: Option<TransitionSpec>,
    pub(super) on_result: Option<ResultCallback<State>>,
    pub(super) dismiss_signal: Option<Rc<Cell<u64>>>,
    pub(super) on_visibility: Option<VisibilityCallback>,
    pub(super) route: Option<Location>,
    pub(super) pop_swipe: Option<bool>,
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
            route: None,
            pop_swipe: None,
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
    /// which carries no erased app state, and it fires *synchronously* there:
    /// nothing is queued, so a covered page learns it is covered on the frame it
    /// happens. Capture what you need (a signal, an `Rc<Cell<_>>`, a controller
    /// handle) in the closure instead.
    ///
    /// (`on_result` does defer — it needs `&mut State` — but it is no longer
    /// waiting on user input to be delivered: the rebuild that queues it also
    /// dispatches the `InputEvent::Housekeeping` broadcast that flushes it, so
    /// both seams now land on the same frame. See the [module docs](self).)
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

    /// Stamp this page's route identity (`R-B1`): the [`Location`] it was
    /// pushed with, published on the navigator's [`RouteStack`] and never
    /// derived from depth. Omit for a page pushed as a bare builder (an
    /// overlay/dialog) — it then publishes a `None` entry, which
    /// [`RouteStack::current_route`] skips.
    pub fn route(mut self, location: Location) -> Self {
        self.route = Some(location);
        self
    }

    /// Override this page's edge-swipe eligibility, ranked above every other
    /// gesture-policy slot: page → navigator explicit
    /// ([`NavigatorView::pop_swipe`]) → platform
    /// ([`NavigatorView::platform_pop_swipe`]) → preset-derived default. Still
    /// subject to [`BackPolicy`]: a [`DismissAnimated`](BackPolicy::DismissAnimated)
    /// or [`Veto`](BackPolicy::Veto) page never arms the gesture regardless of
    /// this override (see [`NavigatorWidget::swipe_armable`]).
    pub fn pop_swipe(mut self, enabled: bool) -> Self {
        self.pop_swipe = Some(enabled);
        self
    }
}

/// Options for [`NavigatorController::replace_with_options`] — opacity, an
/// optional per-op transition override, and the route identity attached
/// to the replacement page. A smaller sibling of [`PushOptions`]: a replaced
/// page has no pusher-side use for a result callback, a `BackPolicy`, a
/// dismiss signal, or a visibility observer, since it does not sit *under*
/// anything it could be dismissed back to.
pub struct ReplaceOptions {
    pub(super) opaque: bool,
    pub(super) transition: Option<TransitionSpec>,
    pub(super) route: Option<Location>,
}

impl ReplaceOptions {
    /// Options for an **opaque** replacement page (the shipped
    /// [`NavigatorController::replace`] default).
    pub fn opaque() -> Self {
        Self {
            opaque: true,
            transition: None,
            route: None,
        }
    }

    /// Options for a **transparent** replacement page.
    pub fn transparent() -> Self {
        Self {
            opaque: false,
            ..Self::opaque()
        }
    }

    /// Override the navigator's default transition for this replace only.
    pub fn transition(mut self, spec: TransitionSpec) -> Self {
        self.transition = Some(spec);
        self
    }

    /// Stamp the replacement page's route identity — see
    /// [`PushOptions::route`].
    pub fn route(mut self, location: Location) -> Self {
        self.route = Some(location);
        self
    }
}

/// One queued navigation op, recorded by the [`NavigatorController`] and drained
/// (in order) by [`NavigatorView::rebuild`].
pub(super) enum NavOp<State: 'static> {
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
        /// The route identity from [`PushOptions::route`], `None` for a
        /// bare-builder overlay/dialog push.
        route: Option<Location>,
        /// The per-route edge-swipe override from [`PushOptions::pop_swipe`],
        /// `None` for every `push*` method other than
        /// [`push_with_options`](NavigatorController::push_with_options).
        pop_swipe: Option<bool>,
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
        /// The route identity from [`ReplaceOptions::route`], `None` for
        /// a plain [`NavigatorController::replace`]/`replace_with`.
        route: Option<Location>,
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
pub struct NavigatorId(pub(super) usize);
