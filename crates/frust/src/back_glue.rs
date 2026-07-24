//! Back-press ⇄ navigator glue (device-parity task 05; predictive-back + facade
//! auto-wiring, glyph-refinements tasks 02/08): the facade is the only crate
//! that sees both `frust-widgets`' [`NavigatorController`] and `frust-reactive`'s
//! back-press source together — mirroring [`router_glue`](crate::router_glue)
//! exactly (`frust-widgets` stays reactive-free, `frust-reactive` stays
//! navigator-free).
//!
//! # The Android back contract (RESEARCH.md "Android back")
//!
//! A shell delivers a hardware/gesture back press → the framework attempts an
//! async back → the framework maintains a "handles back" boolean the shell reads
//! so a *root-level* back falls through to the platform (activity finish). This
//! module is the framework half. A back press is consumed exactly once and
//! routed through [`NavigatorController::request_back`] (task 02), which applies
//! the top page's `BackPolicy` — `Pop` pops, `DismissAnimated` fires the
//! overlay's dismiss signal, `Veto` swallows it — rather than a bare
//! [`pop`](NavigatorController::pop). The "handles back" answer is computed from
//! [`back_interest`](NavigatorController::back_interest) (not
//! [`can_pop`](NavigatorController::can_pop)), so a dismissable/veto overlay at
//! the *root* still claims the press ahead-of-time (predictive-back parity)
//! instead of letting it exit the app.
//!
//! # Two entry points, one consumption source
//!
//! Back handling reaches a controller two ways, and both funnel through the same
//! process-wide consumption marker so a press is consumed *exactly once*:
//!
//! - **Automatic** (task 08): the facade's [`navigator`](crate::navigator)
//!   wrapper calls [`auto_wire`] on every rebuild, so any app using
//!   `frust::navigator` gets back handling with ZERO back-specific app code.
//! - **Explicit**: [`BackHandler`]/[`attach_back_handler`] — the pre-task-08
//!   surface an app constructs in `Component::init` and drives with
//!   [`track`](BackHandler::track) from every `Component::build`. Still fully
//!   supported (Huddle uses it); it now shares the same consumption marker and
//!   `request_back`/`back_interest` routing as the automatic path.
//!
//! Because the whole back system is single-navigator today (frust-reactive's
//! live-provider slot is single-registrant — see `frust_reactive::back`), a
//! single process-wide (UI-thread-affine) [`SHARED`] marker is the "single
//! consumption source": when Huddle constructs a [`BackHandler`] *and* calls
//! `frust::navigator` on the same controller, whichever runs first in a given
//! rebuild consumes the press; the other observes the same already-consumed
//! count and no-ops, so one press = one `request_back` (no double-pop).
//!
//! # Timing: the live provider closes the stale-window
//!
//! The wiring runs during a rebuild's *build* pass, **before** the navigator's
//! `apply_ops` (a later reconciliation step) publishes the new stack depth — so
//! the polled [`set_handles_back`] flag it writes lags a stack change by one
//! frame, and if the frame gate skips the settle frame that stale value can
//! persist. To close that window, the wiring also registers a live provider
//! ([`set_can_pop_provider`]) reading `controller.back_interest()`, which
//! `frust_reactive::handles_back` queries in preference to the flag: it reads
//! the navigator's CURRENT interest at press time (after `apply_ops` published
//! it), so a back press is always decided against the real state, with no
//! one-frame lag. The provider is UI-thread-affine (it reads an `Rc`-backed
//! controller through [`SHARED`]). See `frust_reactive::back`'s module docs for
//! the source-side contract.

use std::cell::RefCell;

use frust_reactive::{CanPopRegistration, back_presses, set_can_pop_provider, set_handles_back};
use frust_widgets::NavigatorController;
use reactive_graph::traits::{Get, GetUntracked};

thread_local! {
    /// The process-wide (UI-thread-affine) back-press wiring shared by BOTH the
    /// automatic [`navigator`](crate::navigator) path and the explicit
    /// [`BackHandler`]. The whole back system is single-navigator today
    /// (frust-reactive's provider slot is single-registrant), so a single shared
    /// marker is the "single consumption source" that makes an auto-wired
    /// `navigator()` and a manual `BackHandler` on the same controller consume a
    /// press EXACTLY once — no double-pop (see the module docs).
    ///
    /// UI-thread-affine because [`SharedBack::interest`] captures an `Rc`-backed
    /// `NavigatorController` (`!Send`), exactly like frust-reactive's live
    /// provider slot it feeds.
    static SHARED: RefCell<SharedBack> = const { RefCell::new(SharedBack::new()) };
}

/// The shared back state behind [`SHARED`] (see its docs).
struct SharedBack {
    /// The last back-press count consumed by *either* entry point; `None` until
    /// the first observation. A press up to this count has already been routed
    /// through `request_back`, so a second entry point (or a re-run rebuild)
    /// observing the same count does not re-fire (the `RouterDeepLinks`
    /// consumed-marker pattern, shared across both back entry points).
    consumed: Option<u64>,
    /// The active controller's `back_interest` probe, refreshed on every wire.
    /// Overwritten (not appended) so at most one controller is retained — the
    /// single-navigator model. The live provider (below) reads through this
    /// slot, so a press is decided against the CURRENT navigator's interest.
    interest: Option<Box<dyn Fn() -> bool>>,
    /// The live can-pop provider registration, made once on the first wire and
    /// kept for the process lifetime (single-navigator model). Its closure reads
    /// [`interest`](Self::interest) live, so `handles_back()` answers against the
    /// current navigator with no one-frame lag (see the module docs' timing
    /// note). `None` until the first wire registers it.
    provider: Option<CanPopRegistration>,
}

impl SharedBack {
    const fn new() -> Self {
        Self {
            consumed: None,
            interest: None,
            provider: None,
        }
    }
}

/// Refresh the active `back_interest` probe and the polled `handles_back`
/// fallback flag, registering the live provider once. Called on every wire from
/// both entry points ([`auto_wire`] and [`BackHandler::new`]/[`track`](BackHandler::track)).
///
/// # Panics
///
/// The first call registers the live provider via [`set_can_pop_provider`],
/// which panics off the UI thread — always the case here (a rebuild / a
/// `Component::init` runs on the UI thread).
fn refresh_interest<State: 'static>(controller: &NavigatorController<State>) {
    let probe = controller.clone();
    SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        shared.interest = Some(Box::new(move || probe.back_interest()));
        if shared.provider.is_none() {
            // Register the stable live provider ONCE. It reads whatever interest
            // probe is currently active from `SHARED`, so re-wiring a new
            // controller (overwriting `interest`) needs no re-registration.
            shared.provider = Some(set_can_pop_provider(Box::new(|| {
                SHARED.with(|shared| {
                    shared
                        .borrow()
                        .interest
                        .as_ref()
                        .map(|probe| probe())
                        .unwrap_or(false)
                })
            })));
        }
    });
    // Refresh the polled fallback flag (the live provider is authoritative; this
    // only keeps the no-provider fallback roughly in sync — see the module docs).
    set_handles_back(controller.back_interest());
}

/// Consume any new back press against the shared marker, routing it through
/// [`NavigatorController::request_back`] (task 02 applies the top page's
/// `BackPolicy`). The tracked read of the back-press counter subscribes THIS
/// rebuild, so a later `push_back_press` wakes it (State & Reactivity: the
/// track-per-rebuild contract).
///
/// The shared marker is the single consumption source (see the module docs): if
/// both entry points run in the same rebuild on the same controller, the first
/// fires `request_back` and the second no-ops on the already-consumed count.
fn consume_back<State: 'static>(controller: &NavigatorController<State>) {
    // Tracked read — subscribes the rebuild to the back-press counter.
    let count = back_presses().count.get();
    let fire = SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        match shared.consumed {
            // A fresh process/marker: seed to the current count WITHOUT firing,
            // so a press delivered before any handler existed does not trigger a
            // spurious back on the first wire.
            None => {
                shared.consumed = Some(count);
                false
            }
            // Already caught up — a re-run rebuild or the second entry point.
            Some(prev) if prev == count => false,
            // A new press (a burst collapses to one `request_back`: catch up to
            // the current depth, don't replay each event — the back semantics).
            Some(_) => {
                shared.consumed = Some(count);
                true
            }
        }
    });
    if fire {
        controller.request_back();
    }
}

/// Seed the shared consumption marker to the current back-press count if it is
/// not seeded yet, so a press delivered before a [`BackHandler`] existed does
/// not trigger a spurious `request_back` on the first [`track`](BackHandler::track).
/// Idempotent (only-if-unseeded), so it composes with a `navigator()` that
/// already seeded the marker on an earlier rebuild.
fn seed_consumed() {
    let count = back_presses().count.get_untracked();
    SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        if shared.consumed.is_none() {
            shared.consumed = Some(count);
        }
    });
}

/// Auto-wire back handling for `controller` (task 08): the entry point the
/// facade's [`navigator`](crate::navigator) wrapper calls on every rebuild.
/// Consumes a new back press (routing it through `request_back`) and refreshes
/// the `handles_back` interest — so an app using `frust::navigator` gets the
/// full Android back contract (overlay dismiss → pop → app exit) with ZERO
/// back-specific app code, and with no double-pop against a manual
/// [`BackHandler`] on the same controller (see the module docs).
///
/// # Panics
///
/// Runs during a rebuild's build pass; the first call registers the live
/// provider, which panics off the UI thread (always the UI thread here).
pub(crate) fn auto_wire<State: 'static>(controller: &NavigatorController<State>) {
    consume_back(controller);
    refresh_interest(controller);
}

/// A [`NavigatorController`] wired to the process-wide back-press source (see
/// the module docs) — the **explicit** back-wiring surface predating task 08's
/// automatic [`navigator`](crate::navigator) auto-wiring.
///
/// App code using `frust::navigator` no longer needs this: back handling is
/// automatic. It stays fully supported for apps that want an explicit handle
/// (Huddle constructs one), and now shares the same single consumption source
/// and `request_back`/`back_interest` routing as the automatic path — so
/// constructing a `BackHandler` *and* calling `frust::navigator` on the same
/// controller still consumes each press exactly once.
///
/// Construct once with [`attach_back_handler`] (or [`BackHandler::new`]) —
/// typically from `Component::init`, storing the result in `Component::State` —
/// then call [`track`](Self::track) from every `Component::build`.
pub struct BackHandler<State: 'static> {
    controller: NavigatorController<State>,
}

impl<State: 'static> BackHandler<State> {
    /// Wire `controller` to the back-press source. Seeds the shared consumption
    /// marker to the current back-press count (so a press delivered before this
    /// handler existed does not trigger a spurious back on the first
    /// [`track`](Self::track)), publishes the initial `handles_back` fallback
    /// flag, and registers the live `back_interest` provider (see the module
    /// docs' timing note).
    ///
    /// Call this once per controller (e.g. from `Component::init`) — from the UI
    /// thread, since the provider slot is UI-thread-affine (it reads the
    /// `Rc`-backed controller). A `Component::init` always runs on the UI thread.
    pub fn new(controller: NavigatorController<State>) -> Self {
        seed_consumed();
        refresh_interest(&controller);
        Self { controller }
    }

    /// Consume a new back press (routing it through
    /// [`request_back`](NavigatorController::request_back)) and refresh the
    /// framework's `handles_back` interest from the current stack. Call from
    /// every `Component::build`.
    ///
    /// Shares the single consumption source with the automatic
    /// [`navigator`](crate::navigator) path (see the module docs): a rebuild
    /// re-run that observes the same already-consumed count neither re-fires nor
    /// double-counts, and a `frust::navigator` on the same controller in the
    /// same rebuild cannot double-pop.
    pub fn track(&self) {
        consume_back(&self.controller);
        refresh_interest(&self.controller);
    }

    /// The wired controller — hand it to
    /// [`navigator`](frust_widgets::navigator), or drive it directly.
    pub fn controller(&self) -> &NavigatorController<State> {
        &self.controller
    }
}

/// Convenience constructor equivalent to [`BackHandler::new`] — see its docs and
/// [`RouterDeepLinks`](crate::RouterDeepLinks)/[`router_with_deep_links`](crate::router_with_deep_links)
/// for the mirrored API shape.
///
/// Note that back handling is **automatic** for any app using
/// [`frust::navigator`](crate::navigator) (task 08); this explicit surface is
/// only needed when an app wants a handle it drives directly (e.g. Huddle).
///
/// ```no_run
/// use frust::{
///     AnyView, BackHandler, Component, NavigatorController, any, attach_back_handler,
///     navigator, text,
/// };
///
/// #[derive(Default)]
/// struct App;
///
/// struct AppState {
///     back: BackHandler<AppState>,
/// }
///
/// impl Component for App {
///     type State = AppState;
///
///     fn init(&self) -> AppState {
///         let nav: NavigatorController<AppState> = NavigatorController::new();
///         AppState {
///             back: attach_back_handler(nav),
///         }
///     }
///
///     fn build(&self, state: &mut AppState) -> AnyView<AppState> {
///         // Called every rebuild: consumes a new back press (via `request_back`)
///         // and keeps the framework's handles-back interest in sync. The
///         // `frust::navigator` call auto-wires the SAME controller — one press
///         // still pops exactly once (shared consumption source).
///         state.back.track();
///         let controller = state.back.controller();
///         any(navigator(controller, || any(text("home"))))
///     }
/// }
///
/// frust::app!(App);
/// # fn main() {}
/// ```
pub fn attach_back_handler<State: 'static>(
    controller: NavigatorController<State>,
) -> BackHandler<State> {
    BackHandler::new(controller)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{AnyView, RenderRoot, any};
    use frust_reactive::{
        ReactiveRuntime, clear_can_pop_provider, handles_back, push_back_press, set_handles_back,
    };
    use frust_widgets::{
        BackPolicy, NavigatorController, NavigatorView, PushOptions, navigator as raw_navigator,
    };
    use std::sync::{Arc, Mutex};

    /// Serializes every test in this module: they all touch `frust-reactive`'s
    /// process-wide back-press counter / `handles_back` flag / live-provider slot
    /// AND this module's `SHARED` marker, so concurrent runs would race the
    /// shared counter (one test's `push_back_press` advancing it under another's
    /// feet). `reset()` (below) additionally clears the per-thread state a reused
    /// harness thread would otherwise carry between tests.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Reset the per-thread wiring so each test starts clean: force-clear
    /// frust-reactive's live provider, drop the shared marker/interest/provider,
    /// and reset the polled flag. Call at the top of every test (after taking
    /// [`TEST_LOCK`], under an initialized runtime).
    fn reset() {
        clear_can_pop_provider();
        set_handles_back(false);
        SHARED.with(|shared| *shared.borrow_mut() = SharedBack::new());
    }

    fn page() -> AnyView<()> {
        any(frust_widgets::text("x"))
    }

    /// The `app_logic` closure type [`RenderRoot::rebuild`] drives, boxed so
    /// [`Harness`] can store it (mirroring `router_glue`'s `AppLogic`).
    type AppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

    /// A `RenderRoot`/app closure driving a controller's `navigator`. The
    /// `wire` closure runs each rebuild BEFORE reconciliation to exercise a back
    /// entry point (the automatic `auto_wire`, an explicit `BackHandler::track`,
    /// or both) exactly as a `Component::build` would.
    struct Harness {
        controller: NavigatorController<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: AppLogic,
        wire: Box<dyn FnMut()>,
    }

    impl Harness {
        /// A fresh-controller harness whose per-rebuild wiring runs `wire`
        /// (given a clone of that controller) — the seam each test uses to pick
        /// its back entry point.
        fn new(wire: impl FnMut(&NavigatorController<()>) + 'static) -> Self {
            Self::with_controller(NavigatorController::new(), wire)
        }

        /// As [`new`](Self::new) but drives an EXISTING `controller` — so a test
        /// can share one controller between a `BackHandler` and the navigator
        /// (the manual + auto criterion).
        fn with_controller(
            controller: NavigatorController<()>,
            mut wire: impl FnMut(&NavigatorController<()>) + 'static,
        ) -> Self {
            let app_controller = controller.clone();
            let app: AppLogic = Box::new(move |_: &mut ()| raw_navigator(&app_controller, page));
            let wire_controller = controller.clone();
            let wire: Box<dyn FnMut()> = Box::new(move || wire(&wire_controller));
            let mut harness = Harness {
                controller,
                root: RenderRoot::new(),
                app,
                wire,
            };
            harness.rebuild();
            harness
        }

        fn rebuild(&mut self) {
            // Wire under the build pass, exactly as a Component::build would,
            // then reconcile (which applies the navigator's queued ops).
            (self.wire)();
            self.root.rebuild(&mut self.app, &mut ());
        }

        fn depth(&self) -> usize {
            self.controller.depth()
        }
    }

    /// Criterion 1: an app using the AUTOMATIC `frust::navigator` auto-wiring
    /// (no `BackHandler` in app code) pops one page on a back press. Also
    /// covers criterion 2 (depth 1, no overlay → `handles_back()` false: the
    /// press falls through to the platform).
    #[test]
    fn auto_wire_pops_without_a_backhandler() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = Harness::new(auto_wire);

        // Criterion 2: at the root (depth 1, no overlay) the framework does NOT
        // handle back — a root-level press bubbles to the platform.
        assert_eq!(h.depth(), 1, "starts at the root page");
        assert!(!handles_back(), "depth 1 + no overlay: handles_back false");

        // Push to depth 2 (a normal opaque push → BackPolicy::Pop).
        h.controller.push(page);
        h.rebuild();
        assert_eq!(h.depth(), 2, "pushed to depth 2");
        assert!(
            handles_back(),
            "poppable stack: handles_back true, same rebuild"
        );

        // Criterion 1: one back press pops one page — with NO BackHandler.
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "auto-wired back press pops one page");
        assert!(
            !handles_back(),
            "back at the root: handles_back false again"
        );
    }

    /// Criterion 3: a `Veto` overlay claims the back press (`handles_back()`
    /// true — predictive-back parity via `back_interest`), the press is
    /// consumed, and the stack is UNCHANGED — the routing-through-`request_back`
    /// (not a bare `pop`) contract. This is the meaningful facade-reachable
    /// form of "depth 1 + Veto overlay": the overlay sits over the root, and
    /// even though a raw `pop()` *would* remove it (the stack is poppable),
    /// `request_back` honors the Veto policy and pops nothing. (A Veto policy on
    /// the depth-1 *root* itself is not expressible through the public push API
    /// — the root is always `BackPolicy::Pop` — and is covered by task 02's
    /// widget-level `compute_back_interest` test.)
    #[test]
    fn veto_overlay_claims_back_but_does_not_pop() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = Harness::new(auto_wire);

        // Push a transparent Veto overlay over the root.
        h.controller
            .push_with_options(page, PushOptions::transparent().back(BackPolicy::Veto));
        h.rebuild();

        // The overlay claims back ahead-of-time (facade reads `back_interest`).
        assert!(handles_back(), "a Veto overlay claims the back press");

        let before = h.depth();
        // A back press is consumed but Veto swallows it — the stack is unchanged.
        // A bare `pop()` here would have removed the overlay (regression guard
        // for the request_back-not-pop switch).
        push_back_press();
        h.rebuild();
        assert_eq!(
            h.depth(),
            before,
            "Veto consumes the press via request_back: stack unchanged (not popped)"
        );
        assert!(handles_back(), "still claiming back (overlay still on top)");
    }

    /// The EXPLICIT `BackHandler` path still works on its own (regression for
    /// the pre-task-08 surface, now routing through `request_back`): a press
    /// pops one page and `handles_back()` tracks the depth with no settle frame
    /// (the live provider closes the stale-window).
    #[test]
    fn explicit_backhandler_pops_and_tracks_handles_back() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let controller: NavigatorController<()> = NavigatorController::new();
        let back = BackHandler::new(controller.clone());
        // Only the explicit handler drives back here — on the SAME controller the
        // harness's navigator uses (a raw navigator view, no auto-wiring).
        let mut h = Harness::with_controller(controller, move |_c| back.track());

        assert_eq!(h.depth(), 1, "starts at the root");
        assert!(
            !handles_back(),
            "root-level back bubbles: handles_back false"
        );

        h.controller.push(page);
        h.rebuild();
        assert_eq!(h.depth(), 2, "push published to depth immediately");
        assert!(
            handles_back(),
            "poppable stack reports handles_back true on the SAME rebuild, no settle frame"
        );

        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "one back press pops one page");
        assert!(
            !handles_back(),
            "root again: handles_back false immediately"
        );

        // A back press at the root is a safe no-op (consumed, pops nothing).
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "pop-at-root is a no-op");

        // After a force-clear the polled fallback flag answers (depth-1 false).
        clear_can_pop_provider();
        assert!(
            !handles_back(),
            "after clear, handles_back reads the polled fallback"
        );
    }

    /// Criterion 4 (regression): a manual `BackHandler` AND the automatic
    /// `navigator()` auto-wiring on the SAME controller consume one press
    /// exactly once — no double-pop. Both run every rebuild (as Huddle does:
    /// `state.back.track()` then `frust::navigator(&controller, ...)`).
    #[test]
    fn manual_backhandler_plus_auto_wire_pops_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        // Construct the manual handler first (as an app would, in init).
        let controller: NavigatorController<()> = NavigatorController::new();
        let back = BackHandler::new(controller.clone());

        // The harness's navigator AND both back entry points drive the SAME
        // controller. Each rebuild runs BOTH entry points in Huddle's order: the
        // manual `track()` then the auto-wiring (`c` is the harness controller).
        let mut h = Harness::with_controller(controller, move |c| {
            back.track();
            auto_wire(c);
        });

        // Push to depth 3 so a single-vs-double pop is unambiguous.
        h.controller.push(page);
        h.rebuild();
        h.controller.push(page);
        h.rebuild();
        assert_eq!(h.depth(), 3, "pushed to depth 3");

        // ONE back press: with a shared consumption source this pops exactly one
        // page (depth 2). A double-consumption bug would pop two (depth 1).
        push_back_press();
        h.rebuild();
        assert_eq!(
            h.depth(),
            2,
            "manual + auto share one consumption source: one press pops once"
        );

        // A rebuild with no new press does not re-pop (dedupe).
        h.rebuild();
        assert_eq!(h.depth(), 2, "no new press -> no extra pop");
    }
}
