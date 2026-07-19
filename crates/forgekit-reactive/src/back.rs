//! Process-wide Android **back-press** source + a "framework handles back"
//! flag (device-parity task 05, RESEARCH.md "Android back"): a shell delivers
//! a hardware/gesture back press via [`push_back_press`]; app/facade glue
//! reads the resulting event through [`back_presses`]/[`BackPresses`] and
//! publishes, via [`set_handles_back`], whether the framework wants to consume
//! the *next* press so a shell polling [`handles_back`] knows whether a
//! root-level back should fall through to the platform (activity finish).
//!
//! This mirrors the deep-link source next door (`forgekit-reactive::deep_link`)
//! in both shape and layering: `forgekit-reactive` stays router/navigator-free,
//! and the `forgekit` facade is the only crate that wires this source to a
//! `NavigatorController` (see `forgekit::back_glue`).
//!
//! # A counter, not a boolean
//!
//! Each back press is an *event*, so the source is a monotonically increasing
//! [`RwSignal<u64>`] counter (via [`BackPresses::count`]), not a `bool` "back
//! pressed" flag. App glue dedupes by the last value it consumed — exactly the
//! `RouterDeepLinks` consumed-marker pattern — so a rebuild that re-reads the
//! same count does not re-pop. A counter (rather than a value payload) is
//! enough because a back press carries no data: only "another one happened".
//!
//! # `handles_back`: default false
//!
//! [`handles_back`] backs a shell's `setFrameworkHandlesBack`-style decision
//! (RESEARCH.md): the framework pre-registers whether it will consume the next
//! back. It is a process-global [`AtomicBool`] (the `theme_override` polling
//! precedent — a plain flag a shell reads, no reactive tracking), and it
//! **defaults to `false`**: an app with no navigator (nothing to pop) must let
//! a back press exit, matching Flutter's "no routes to pop → bubble →
//! `SystemNavigator.pop`". The facade's `BackHandler` refreshes it to
//! `controller.can_pop()` each rebuild.
//!
//! # Timing note
//!
//! [`handles_back`] is refreshed at **rebuild time**. A press that races a
//! same-frame stack change reads the *previous* answer — this matches
//! Flutter's pre-registered `setFrameworkHandlesBack` semantics (RESEARCH.md)
//! and is acceptable: the window is one frame, and a mis-predicted
//! root-level back is a no-op pop (the navigator's own `len > 1` guard stays
//! authoritative) rather than an incorrect navigation.
//!
//! # Thread contract
//!
//! [`push_back_press`] must be called on the UI thread — the one
//! [`ReactiveRuntime::init`] ran on — mirroring [`push_deep_link`](crate::push_deep_link)'s
//! contract: a mobile shell's native back callback always runs on the UI
//! thread, so an off-thread call is a wiring bug and panics. A press that
//! races ahead of [`ReactiveRuntime::init`] is dropped with a logged warning
//! rather than panicking or buffering (the same rationale as the deep-link
//! source). [`set_handles_back`]/[`handles_back`], like `theme_override`, carry
//! no thread constraint — a shell polls `handles_back` from its own frame loop.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use reactive_graph::signal::RwSignal;
use reactive_graph::traits::Update;

use crate::ReactiveRuntime;
use crate::executor::is_ui_thread;

/// The app-facing back-press read surface (see the module docs). Obtained via
/// [`back_presses`] (`forgekit::back_presses()` at the facade).
#[derive(Clone)]
pub struct BackPresses {
    /// A monotonically increasing count of back presses delivered this
    /// process. Read/track it with the `Get`/`Track` traits the same way any
    /// other `RwSignal` is read; glue dedupes by comparing it against the last
    /// value it consumed (see the module docs).
    pub count: RwSignal<u64>,
}

/// The process-wide back-press counter signal. Lazily created (mirroring
/// [`deep_link`](crate::deep_link)'s slot) under the reactive root
/// [`Owner`](reactive_graph::owner::Owner) on first access once
/// [`ReactiveRuntime`] exists.
static COUNTER: OnceLock<RwSignal<u64>> = OnceLock::new();

/// The process-global "framework handles the next back press" flag. A plain
/// [`AtomicBool`] (no reactive tracking — a shell polls it), defaulting to
/// `false` (see the module docs' `handles_back` section).
static HANDLES_BACK: AtomicBool = AtomicBool::new(false);

/// Returns the process-wide back-press counter signal, creating it (under the
/// reactive root [`Owner`](reactive_graph::owner::Owner)) on first access.
///
/// # Panics
///
/// Panics if [`ReactiveRuntime::init`] has not run yet — reading or tracking
/// the counter before the reactive runtime exists is a wiring bug
/// ([`push_back_press`]'s pre-init case is handled separately, before this is
/// ever reached): the caller must initialize the runtime first.
fn counter() -> RwSignal<u64> {
    *COUNTER.get_or_init(|| {
        let rt = ReactiveRuntime::get().expect(
            "forgekit-reactive: back_presses() was called before ReactiveRuntime::init — an app \
             must run under the ForgeKit facade's entry point (which initializes the reactive \
             runtime) before reading back presses",
        );
        rt.with_owner(|| RwSignal::new(0))
    })
}

/// Deliver a platform back press into the process-wide source. Called by a
/// shell (the Android back callback) on the UI thread; app code never calls
/// this directly.
///
/// Bumps the [`BackPresses::count`] counter by one, so a tracked reader (the
/// facade's `BackHandler`, run under `Component::build`) observes a new event
/// and dedupes it against the last count it consumed.
///
/// # Panics
///
/// Panics if called off the UI thread (see the module docs' thread contract).
/// A call before [`ReactiveRuntime::init`] does **not** panic — it is dropped
/// with a logged warning (see the module docs).
pub fn push_back_press() {
    if !is_ui_thread() {
        panic!(
            "forgekit-reactive: push_back_press was called off the UI thread. Back presses can \
             only be pushed from the UI thread (the one `ReactiveRuntime::init` ran on) — this \
             is a wiring bug: route the platform delivery through the UI thread before pushing, \
             the same contract `push_deep_link`/`Executor::spawn_local` enforce."
        );
    }

    if ReactiveRuntime::get().is_none() {
        eprintln!(
            "forgekit-reactive: push_back_press() dropped — ReactiveRuntime::init has not run \
             yet. A back press before the runtime exists indicates an odd init-ordering race, \
             not normal operation (the shell wires the back callback only after init)."
        );
        return;
    }

    counter().update(|c| *c += 1);
}

/// The current back-press read surface: the live [`BackPresses::count`]
/// signal. Call from a tracked context (e.g. inside `Component::build`, via the
/// facade's `BackHandler`) to observe subsequent presses as they arrive.
pub fn back_presses() -> BackPresses {
    BackPresses { count: counter() }
}

/// Publish whether the framework will consume the **next** back press (see the
/// module docs' `handles_back` section). The facade's `BackHandler` calls this
/// each rebuild with `controller.can_pop()`; a shell reads the answer via
/// [`handles_back`].
///
/// Unlike [`push_back_press`], this carries no thread constraint (it is a plain
/// atomic store — the `theme_override` precedent).
pub fn set_handles_back(handles: bool) {
    HANDLES_BACK.store(handles, Ordering::Relaxed);
}

/// Whether the framework wants to consume the next back press. A shell polls
/// this to decide whether a back press should be routed into the app (`true`)
/// or fall through to the platform / activity finish (`false`, the default
/// until glue publishes otherwise — see the module docs).
pub fn handles_back() -> bool {
    HANDLES_BACK.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameWaker, TrackedScope};
    use reactive_graph::traits::{Get, GetUntracked};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    /// A recording waker: an `Arc<AtomicUsize>` bumped once per `wake()`.
    fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = counter.clone();
        let waker: FrameWaker = Arc::new(move || {
            counter.fetch_add(1, AtomicOrdering::SeqCst);
        });
        (waker, seen)
    }

    /// Every non-panic acceptance criterion in one `#[test]`, serialized on the
    /// shared waker lock (`ReactiveRuntime::init` swaps the process-wide waker,
    /// which would otherwise race the other waker-asserting tests in this
    /// crate — see `WAKER_TEST_LOCK`'s doc comment in `lib.rs`).
    #[test]
    fn back_press_push_dedupe_and_handles_flag() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (waker, wakes) = recording_waker();
        let _rt = ReactiveRuntime::init(waker);

        // handles_back defaults to false (an app with no navigator lets back
        // exit), and set/get round-trips.
        assert!(!handles_back(), "handles_back defaults to false");
        set_handles_back(true);
        assert!(handles_back(), "set_handles_back(true) is observable");
        set_handles_back(false);
        assert!(!handles_back(), "set_handles_back(false) is observable");

        // A tracked scope reading `count` starts clean; a push bumps the
        // counter and fires the waker exactly once (coalesced signal-write).
        let start = back_presses().count.get_untracked();
        let scope = TrackedScope::new();
        let seen = scope.track(|| back_presses().count.get());
        assert_eq!(seen, start, "a fresh track observes the current count");
        assert!(!scope.is_dirty(), "a fresh track starts clean");

        let before = wakes.load(AtomicOrdering::SeqCst);
        push_back_press();
        assert!(
            scope.is_dirty(),
            "push_back_press must dirty a scope tracking `count`"
        );
        assert_eq!(
            wakes.load(AtomicOrdering::SeqCst) - before,
            1,
            "a push must fire the waker exactly once"
        );
        assert_eq!(
            back_presses().count.get_untracked(),
            start + 1,
            "each push increments the counter by one"
        );

        // Dedupe contract (mirrors RouterDeepLinks): a consumer that records
        // the last-seen count no-ops on a re-read with no new push, and sees
        // exactly one new event after another push.
        let mut consumed = back_presses().count.get_untracked();
        assert_eq!(
            back_presses().count.get_untracked(),
            consumed,
            "no push -> the count is unchanged, so a dedup'd consumer no-ops"
        );
        push_back_press();
        push_back_press();
        let now = back_presses().count.get_untracked();
        assert_eq!(now, consumed + 2, "two pushes advance the counter by two");
        // A consumer catches up to the latest count in one step (it dedupes by
        // value, not by replaying each intermediate press).
        assert_ne!(now, consumed, "there is unconsumed back-press progress");
        consumed = now;
        assert_eq!(back_presses().count.get_untracked(), consumed);
    }

    /// The UI-thread contract is enforced (mirrors `push_deep_link`'s
    /// off-thread panic). Serializes on the waker lock since
    /// `ReactiveRuntime::init` swaps the process-wide waker.
    #[test]
    #[should_panic(expected = "wiring bug")]
    fn push_off_ui_thread_panics() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        std::thread::spawn(push_back_press)
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
