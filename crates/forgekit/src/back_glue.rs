//! Back-press ⇄ navigator glue (device-parity task 05): the facade is the only
//! crate that sees both `forgekit-widgets`' [`NavigatorController`] and
//! `forgekit-reactive`'s back-press source together — mirroring
//! [`router_glue`](crate::router_glue) exactly (`forgekit-widgets` stays
//! reactive-free, `forgekit-reactive` stays navigator-free).
//!
//! # The Android back contract (RESEARCH.md "Android back")
//!
//! A shell delivers a hardware/gesture back press → the framework attempts an
//! async pop → the framework maintains a "handles back" boolean the shell reads
//! so a *root-level* back falls through to the platform (activity finish). This
//! module is the framework half: [`BackHandler::track`], run from every
//! `Component::build`,
//!
//! 1. **consumes new back presses** (dedup'd by the counter, exactly like
//!    [`RouterDeepLinks`](crate::RouterDeepLinks)'s consumed marker) and calls
//!    [`NavigatorController::pop`] when [`can_pop`](NavigatorController::can_pop)
//!    — a press at the root leaves the stack untouched (the navigator's own
//!    `len > 1` guard is authoritative, and the shell already routed a
//!    root-level back to the platform because `handles_back` was false); and
//! 2. **refreshes** [`set_handles_back`]`(controller.can_pop())` so the shell's
//!    polled-flag fallback matches the current stack depth, and
//! 3. registers, once at construction, a **live can-pop provider**
//!    ([`set_can_pop_provider`]) reading `controller.can_pop()` — the primary
//!    source the shell's `handles_back()` consults (see the timing note).
//!
//! # Timing: the live provider closes the stale-window
//!
//! `track` runs during a rebuild's *build* pass, **before** the navigator's
//! `apply_ops` (a later reconciliation step) publishes the new stack depth — so
//! the polled [`set_handles_back`] flag it writes lags a stack change by one
//! frame, and if the frame gate skips the settle frame that stale `false` can
//! persist and let a root-level back fall through to activity-finish while the
//! stack is still poppable. To close that window, [`new`](BackHandler::new)
//! registers a live provider (`move || controller.can_pop()`) that
//! `forgekit_reactive::handles_back` queries in preference to the flag: it reads
//! the navigator's CURRENT depth at press time (after `apply_ops` published it),
//! so a back press is always decided against the real depth, with no one-frame
//! lag. The provider is UI-thread-affine (it captures the `Rc`-backed
//! controller) and is unregistered via `on_cleanup` when the host component
//! tears down. See `forgekit_reactive::back`'s module docs for the source-side
//! contract.

use forgekit_reactive::{
    back_presses, clear_can_pop_provider, on_cleanup, set_can_pop_provider, set_handles_back,
};
use forgekit_widgets::NavigatorController;
use reactive_graph::traits::{Get, GetUntracked};
use std::cell::Cell;
use std::rc::Rc;

/// A [`NavigatorController`] wired to the process-wide back-press source (see
/// the module docs). Construct once with [`attach_back_handler`] (or
/// [`BackHandler::new`] directly) — typically from `Component::init`, storing
/// the result in `Component::State` — then call [`track`](Self::track) from
/// every `Component::build` so a back press pops the navigator and the
/// framework's `handles_back` flag stays in sync with the stack depth.
///
/// This mirrors [`RouterDeepLinks`](crate::RouterDeepLinks)'s shape: a
/// consumed-marker dedupe over a `forgekit-reactive` signal, driven from
/// `build`.
pub struct BackHandler<State: 'static> {
    controller: NavigatorController<State>,
    /// The last back-press count this handler consumed — the dedupe marker
    /// (mirroring `RouterDeepLinks`'s consumed `DeepLink`). A rebuild that
    /// re-reads the same count does not re-pop.
    consumed: Rc<Cell<u64>>,
}

impl<State: 'static> BackHandler<State> {
    /// Wire `controller` to the back-press source. Records the current
    /// back-press count as already-consumed (so a press delivered before this
    /// handler existed does not trigger a spurious pop on the first
    /// [`track`](Self::track)), publishes the initial
    /// [`set_handles_back`]`(controller.can_pop())` fallback flag, and registers
    /// a **live can-pop provider** so a press between frames reads the
    /// navigator's current depth rather than the rebuild-time flag (see the
    /// module docs' timing note). The provider is unregistered via `on_cleanup`
    /// when the host component's owner is disposed.
    ///
    /// Call this once per controller (e.g. from `Component::init`) — from the UI
    /// thread, since the provider slot is UI-thread-affine (it captures the
    /// `Rc`-backed controller). A `Component::init` always runs on the UI thread.
    pub fn new(controller: NavigatorController<State>) -> Self {
        let consumed = Rc::new(Cell::new(back_presses().count.get_untracked()));
        set_handles_back(controller.can_pop());
        // Register the live provider (device-parity fix F2): `handles_back()`
        // consults this in preference to the polled flag, reading the CURRENT
        // stack depth at press time. Unregister it when the owning component
        // tears down (a no-op outside an owner, e.g. in unit tests).
        let provider_controller = controller.clone();
        set_can_pop_provider(Box::new(move || provider_controller.can_pop()));
        on_cleanup(clear_can_pop_provider);
        Self {
            controller,
            consumed,
        }
    }

    /// Track the live back-press counter and pop the navigator on any press not
    /// yet consumed, then refresh the framework's `handles_back` **fallback**
    /// flag from the current stack depth. Call from every `Component::build`.
    ///
    /// Dedup'd by the consumed marker (see the module docs): a rebuild re-run
    /// that observes the same already-consumed count neither pops nor
    /// double-counts. A press at the root (`!can_pop`) is consumed but pops
    /// nothing — the shell already bubbled it to the platform because
    /// `handles_back` was false.
    ///
    /// The `set_handles_back` write here maintains only the *fallback* flag; the
    /// live provider registered in [`new`](Self::new) is what a shell's
    /// `handles_back()` actually reads, so this flag's one-frame lag no longer
    /// affects the press decision (see the module docs' timing note).
    pub fn track(&self) {
        let count = back_presses().count.get();
        if count != self.consumed.get() {
            // Consume every press up to `count` in one step (like
            // `RouterDeepLinks`, this dedupes by value — a burst of presses
            // between rebuilds collapses, which is the correct back semantics:
            // catch up to the current depth, don't replay each event).
            self.consumed.set(count);
            if self.controller.can_pop() {
                self.controller.pop();
            }
        }
        // Refresh the fallback flag at rebuild time. The live provider (see the
        // module docs' timing note) is authoritative; this only keeps the
        // no-provider fallback roughly in sync.
        set_handles_back(self.controller.can_pop());
    }

    /// The wired controller — hand it to
    /// [`navigator`](forgekit_widgets::navigator), or drive it directly.
    pub fn controller(&self) -> &NavigatorController<State> {
        &self.controller
    }
}

/// Convenience constructor equivalent to [`BackHandler::new`] — see its docs
/// for the consume-current-count and `handles_back` contracts, and
/// [`RouterDeepLinks`](crate::RouterDeepLinks)/[`router_with_deep_links`](crate::router_with_deep_links)
/// for the mirrored API shape.
///
/// ```no_run
/// use forgekit::{
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
///         // Called every rebuild: pops on a new back press, and keeps the
///         // framework's handles-back flag in sync with the stack depth.
///         state.back.track();
///         let controller = state.back.controller();
///         any(navigator(controller, || any(text("home"))))
///     }
/// }
///
/// forgekit::app!(App);
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
    use forgekit_core::{AnyView, RenderRoot, any};
    use forgekit_reactive::{
        ReactiveRuntime, clear_can_pop_provider, handles_back, push_back_press,
    };
    use forgekit_widgets::{NavigatorView, navigator};
    use std::sync::Arc;

    fn page() -> AnyView<()> {
        any(forgekit_widgets::text("x"))
    }

    /// The `app_logic` closure type [`RenderRoot::rebuild`] drives, boxed so
    /// [`Harness`] can store it (mirroring `router_glue`'s `AppLogic`).
    type AppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

    /// A `BackHandler` plus the `RenderRoot`/app closure driving its controller's
    /// `navigator`, mirroring `router_glue`'s `Harness`.
    struct Harness {
        back: BackHandler<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: AppLogic,
        state: (),
    }

    impl Harness {
        fn new() -> Self {
            let controller: NavigatorController<()> = NavigatorController::new();
            let back = BackHandler::new(controller.clone());
            let app: AppLogic = Box::new(move |_: &mut ()| navigator(&controller, page));
            let mut harness = Harness {
                back,
                root: RenderRoot::new(),
                app,
                state: (),
            };
            harness.rebuild();
            harness
        }

        fn rebuild(&mut self) {
            // Track under the build pass, exactly as an app's Component::build
            // would, then reconcile.
            self.back.track();
            self.root.rebuild(&mut self.app, &mut self.state);
        }

        fn depth(&self) -> usize {
            self.back.controller().depth()
        }
    }

    // All criteria in ONE `#[test]`: `forgekit_reactive::back`'s process-wide
    // counter, `handles_back` flag, and live-provider slot are shared by every
    // test in this binary, and this is the only test in `forgekit`'s suite that
    // touches them — a single function gives deterministic ordering without a
    // crate-private test lock (unreachable from here), and it asserts nothing
    // about waker counts.
    //
    // F2 (device-parity fix): the stale-false window is CLOSED. `track()` runs
    // during a rebuild's build pass, before the navigator's `apply_ops`
    // publishes the new depth in the same rebuild, so the polled flag lags by a
    // frame — but `Harness::new` registered a live can-pop provider, so
    // `handles_back()` reads the CURRENT depth and needs NO settle rebuild. Every
    // `handles_back()` assertion below therefore runs on the SAME rebuild that
    // applied the stack change, which is exactly the stale-window the old code
    // failed in.
    #[test]
    fn back_handler_glue() {
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        let mut h = Harness::new();

        // At the root: depth 1 (authoritative from the widget), and the
        // framework does NOT handle back (a root-level back must bubble to the
        // platform) — no settle rebuild needed.
        assert_eq!(h.depth(), 1, "starts at the root page");
        assert!(
            !handles_back(),
            "root-level back bubbles: handles_back false"
        );

        // Push a page and rebuild ONCE. This is the stale-window: `track()` wrote
        // the flag from the pre-push depth (1 -> false), THEN `apply_ops` raised
        // the depth to 2 later in the same rebuild. Without the live provider a
        // back press here would fall through to activity-finish while the stack is
        // poppable (the A2 defect). With it, `handles_back()` is true immediately.
        h.back.controller().push(page);
        h.rebuild();
        assert_eq!(h.depth(), 2, "push published to depth immediately");
        assert!(
            handles_back(),
            "stale-window closed: a poppable stack reports handles_back true on the \
             SAME rebuild that pushed, with no settle frame"
        );

        h.back.controller().push(page);
        h.rebuild();
        assert_eq!(h.depth(), 3);
        assert!(handles_back(), "still poppable, still no settle frame");

        // A back press in that same-frame window pops instead of falling through.
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 2, "one back press pops one page");
        assert!(handles_back(), "still poppable");

        // Dedupe: a rebuild with no new press does not re-pop.
        h.rebuild();
        assert_eq!(h.depth(), 2, "no new press -> no extra pop");

        // Another press pops down to the root; handles_back flips to false on the
        // same rebuild the depth-1 stack is published (no settle).
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "back at the root");
        assert!(
            !handles_back(),
            "root again: handles_back false immediately"
        );

        // A back press at the root is a safe no-op (the press is consumed but
        // pops nothing — the shell already bubbled it since handles_back false).
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "pop-at-root is a no-op");
        assert!(!handles_back());

        // Provider unregistration is safe: after clearing, `handles_back()` falls
        // back to the polled flag (last set to the depth-1 `false`).
        clear_can_pop_provider();
        assert!(
            !handles_back(),
            "after clear, handles_back reads the polled fallback flag"
        );
    }
}
