//! Process-wide Android **back-press** source + a "framework handles back"
//! flag (device-parity task 05, RESEARCH.md "Android back"): a shell delivers
//! a hardware/gesture back press via [`push_back_press`]; app/facade glue
//! reads the resulting event through [`back_presses`]/[`BackPresses`] and
//! publishes, via [`set_handles_back`], whether the framework wants to consume
//! the *next* press so a shell polling [`handles_back`] knows whether a
//! root-level back should fall through to the platform (activity finish).
//!
//! This mirrors the deep-link source next door (`frust-reactive::deep_link`)
//! in both shape and layering: `frust-reactive` stays router/navigator-free,
//! and the `frust` facade is the only crate that wires this source to a
//! `NavigatorController` (see `frust::back_glue`).
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
//! # Timing: a live provider closes the stale-window
//!
//! [`handles_back`] has two sources. The polled [`set_handles_back`] flag is
//! refreshed at **rebuild time**, so it is stale by up to one frame — a press
//! racing a same-frame stack change reads the *previous* answer, and if the
//! frame gate skips the settle frame that stale answer can persist. On its own
//! that window can let a root-level back fall through to activity-finish while
//! the stack is still poppable.
//!
//! To close it, the facade's `BackHandler` also registers a **live provider**
//! (see [`set_can_pop_provider`]) that [`handles_back`] consults *first*: the
//! provider reads the navigator's CURRENT stack depth at press time — after the
//! navigator's `apply_ops` has published it — so a back press is always decided
//! against the real depth rather than a rebuild-time snapshot, with no
//! one-frame lag. The polled flag stays the fallback when no provider is
//! registered (an app with no `BackHandler`); there the navigator's own
//! `len > 1` guard still makes a mis-predicted root-level back a safe no-op pop
//! rather than an incorrect navigation.
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
//! The **live provider** slot ([`set_can_pop_provider`]/[`clear_can_pop_provider`])
//! is the exception: its closure captures an `Rc`-backed `NavigatorController`
//! (`!Send`), so it lives in UI-thread-affine `thread_local` storage and
//! `set_can_pop_provider` panics if called off the UI thread — the same
//! wiring-bug convention `push_back_press`/`Executor::spawn_local` enforce.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use reactive_graph::signal::RwSignal;
use reactive_graph::traits::Update;

use crate::ReactiveRuntime;
use crate::executor::is_ui_thread;

/// The app-facing back-press read surface (see the module docs). Obtained via
/// [`back_presses`] (`frust::back_presses()` at the facade).
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
/// `false` (see the module docs' `handles_back` section). The **fallback**
/// answer [`handles_back`] returns when no live provider is registered.
static HANDLES_BACK: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The live "can the framework pop?" provider (see the module docs' timing
    /// section). UI-thread-affine because the closure the facade's `BackHandler`
    /// registers captures an `Rc`-backed `NavigatorController` (`!Send`); it
    /// therefore lives here in `thread_local` storage rather than a global, and
    /// [`set_can_pop_provider`] panics off the UI thread. When set,
    /// [`handles_back`] consults it in preference to the polled [`HANDLES_BACK`]
    /// flag, so a back press reads the navigator's CURRENT depth (post-
    /// `apply_ops`) rather than a stale rebuild-time snapshot.
    static CAN_POP_PROVIDER: RefCell<Option<RegisteredProvider>> =
        const { RefCell::new(None) };

    /// Monotonic id source for [`CanPopRegistration`] tokens (UI-thread-only,
    /// like the slot itself), so a stale registration's cleanup can be told
    /// apart from the live one's.
    static CAN_POP_NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// A registered live provider: its registration id + the closure itself
/// (see [`CanPopRegistration`] for the id's role).
type RegisteredProvider = (u64, Box<dyn Fn() -> bool>);

/// Proof-of-registration token returned by [`set_can_pop_provider`].
///
/// **The provider slot is single-registrant: at most one live provider exists
/// at a time, and a later [`set_can_pop_provider`] call replaces the earlier
/// registration without warning** (the facade's `BackHandler` is expected to be
/// constructed once, at the app root). This token is what makes that
/// replacement safe against out-of-order teardown: [`CanPopRegistration::unregister`]
/// clears the slot **only if this registration is still the live one**, so a
/// replaced (stale) handler's `on_cleanup` can never clear a newer handler's
/// provider out from under it. A future multi-handler/intercept design must
/// replace this slot with a stack — see the module docs.
#[must_use = "dropping the registration token without storing it makes the provider impossible to unregister scoped-safely"]
#[derive(Debug)]
pub struct CanPopRegistration(u64);

impl CanPopRegistration {
    /// Unregister this provider **iff it is still the live registration**;
    /// a stale token (already replaced by a newer [`set_can_pop_provider`]
    /// call) is a harmless no-op, leaving the newer provider intact. Safe on
    /// any thread (a non-UI thread's slot is empty, so it no-ops).
    pub fn unregister(self) {
        CAN_POP_PROVIDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(id, _)| *id == self.0) {
                *slot = None;
            }
        });
    }
}

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
            "frust-reactive: back_presses() was called before ReactiveRuntime::init — an app \
             must run under the Frust facade's entry point (which initializes the reactive \
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
            "frust-reactive: push_back_press was called off the UI thread. Back presses can \
             only be pushed from the UI thread (the one `ReactiveRuntime::init` ran on) — this \
             is a wiring bug: route the platform delivery through the UI thread before pushing, \
             the same contract `push_deep_link`/`Executor::spawn_local` enforce."
        );
    }

    if ReactiveRuntime::get().is_none() {
        eprintln!(
            "frust-reactive: push_back_press() dropped — ReactiveRuntime::init has not run \
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

/// Register a live provider [`handles_back`] consults to answer "would a back
/// press pop?" against the navigator's CURRENT stack depth, not a rebuild-time
/// snapshot. The facade's `BackHandler` registers `move || controller.can_pop()`,
/// so a press arriving between frames — after `apply_ops` published the new
/// depth but before the next rebuild refreshed the polled flag — is decided
/// correctly instead of falling through to activity finish (see the module
/// docs' timing section). A registered provider wins over
/// [`set_handles_back`]'s flag; the returned [`CanPopRegistration`] token
/// unregisters it scoped-safely.
///
/// **Single-registrant invariant:** the slot holds at most ONE provider; a
/// second call replaces the first silently (last-writer-wins). Constructing
/// more than one live `BackHandler` is therefore unsupported today — the
/// replaced handler stops influencing [`handles_back`] immediately, and its
/// later cleanup no-ops (token-guarded) rather than clearing the newer
/// registration. A future back-intercept/stacked design (ACTION_ITEMS A8)
/// must widen this slot to a stack instead of registering a second provider.
///
/// # Panics
///
/// Panics if called off the UI thread. The provider captures an `Rc`-backed
/// `NavigatorController` (`!Send`) and lives in UI-thread `thread_local`
/// storage, so registering it from another thread is a wiring bug — the same
/// convention [`push_back_press`]/`Executor::spawn_local` enforce.
pub fn set_can_pop_provider(provider: Box<dyn Fn() -> bool>) -> CanPopRegistration {
    if !is_ui_thread() {
        panic!(
            "frust-reactive: set_can_pop_provider was called off the UI thread. The can-pop \
             provider captures an Rc-backed NavigatorController (!Send) and lives in UI-thread \
             storage — this is a wiring bug: register it from the UI thread (the one \
             `ReactiveRuntime::init` ran on), the same contract `push_back_press`/\
             `Executor::spawn_local` enforce."
        );
    }
    let id = CAN_POP_NEXT_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    CAN_POP_PROVIDER.with(|slot| *slot.borrow_mut() = Some((id, provider)));
    CanPopRegistration(id)
}

/// **Force-clear** the live can-pop provider unconditionally, restoring the
/// polled [`handles_back`] fallback — a teardown/test hammer, NOT the handler
/// cleanup path. A handler's cleanup must go through its own
/// [`CanPopRegistration::unregister`] so a stale registration can never clear
/// a newer one (see the single-registrant invariant on
/// [`set_can_pop_provider`]). Idempotent — safe with no provider registered,
/// and it carries no thread constraint (clearing another thread's empty slot
/// is a harmless no-op).
pub fn clear_can_pop_provider() {
    CAN_POP_PROVIDER.with(|slot| *slot.borrow_mut() = None);
}

/// Whether the framework wants to consume the next back press. A shell polls
/// this to decide whether a back press should be routed into the app (`true`)
/// or fall through to the platform / activity finish (`false`, the default
/// until glue publishes otherwise — see the module docs).
///
/// A live provider (see [`set_can_pop_provider`]), when registered, wins: it is
/// queried against the navigator's CURRENT depth so a press is never mis-decided
/// against a stale rebuild-time snapshot. Otherwise this returns the polled
/// [`set_handles_back`] flag.
pub fn handles_back() -> bool {
    // A live provider reads the navigator's current stack depth at call time —
    // the whole point of the slot is to bypass the polled flag's one-frame lag.
    // The closure only reads the controller's depth cell (never re-enters this
    // module), so calling it under the borrow is safe.
    if let Some(answer) = CAN_POP_PROVIDER.with(|slot| slot.borrow().as_ref().map(|(_, f)| f())) {
        return answer;
    }
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

    /// The live can-pop provider wins over the polled `handles_back` flag and is
    /// queried afresh each call (so it reflects the CURRENT navigator depth, not
    /// a rebuild-time snapshot), and unregistering it restores the flag fallback.
    /// Serializes on the waker lock since `ReactiveRuntime::init` swaps the
    /// process-wide waker (and marks this thread as the UI thread, which
    /// `set_can_pop_provider` requires).
    #[test]
    fn can_pop_provider_wins_and_unregisters() {
        use std::cell::Cell;
        use std::rc::Rc;

        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        // Start from a known state: no provider, flag false.
        clear_can_pop_provider();
        set_handles_back(false);
        assert!(!handles_back(), "no provider + flag false -> false");
        set_handles_back(true);
        assert!(
            handles_back(),
            "no provider -> the polled flag is the answer"
        );

        // Register a live provider driven by a local cell; point the polled flag
        // the OPPOSITE way so the assertions can only pass if the provider wins.
        let live = Rc::new(Cell::new(true));
        let probe = live.clone();
        let reg = set_can_pop_provider(Box::new(move || probe.get()));

        set_handles_back(false);
        assert!(
            handles_back(),
            "a registered provider wins over the (opposite) polled flag"
        );

        // The provider is queried live each call — flipping the cell (as a
        // post-apply_ops depth change would) is observed immediately, with no
        // rebuild in between: the whole point of the slot.
        live.set(false);
        set_handles_back(true);
        assert!(
            !handles_back(),
            "the provider is re-queried live, not cached, and still wins"
        );

        // Unregistering (token-scoped) restores the polled-flag fallback.
        reg.unregister();
        assert!(handles_back(), "after clear, the flag (true) answers again");
        set_handles_back(false);
        assert!(!handles_back(), "fallback tracks the flag once more");

        // Idempotent: a force-clear with nothing registered is a safe no-op.
        clear_can_pop_provider();
        assert!(!handles_back());
    }

    /// The single-registrant invariant's token guard: a REPLACED registration's
    /// cleanup must never clear the newer provider out from under it — the
    /// exact out-of-order-teardown hazard the round-1 review flagged (a stale
    /// `on_cleanup` firing after a second `BackHandler` registered would
    /// silently revert `handles_back` to the polled fallback).
    #[test]
    fn stale_unregister_never_clears_a_newer_registration() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        clear_can_pop_provider();
        set_handles_back(false);

        let reg1 = set_can_pop_provider(Box::new(|| false));
        assert!(!handles_back(), "first provider answers false");

        // A second registration replaces the first (last-writer-wins).
        let reg2 = set_can_pop_provider(Box::new(|| true));
        assert!(handles_back(), "second provider replaced the first");

        // The STALE token's cleanup is a no-op — the live provider survives.
        reg1.unregister();
        assert!(
            handles_back(),
            "stale unregister must not clear the newer registration"
        );

        // The live token's cleanup clears for real, restoring the fallback.
        reg2.unregister();
        assert!(!handles_back(), "live unregister restores the polled flag");
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

    /// Registering the live provider off the UI thread is a wiring bug and
    /// panics — the closure captures an `Rc`-backed controller (`!Send`), so it
    /// can only live in the UI thread's storage. Serializes on the waker lock
    /// since `ReactiveRuntime::init` swaps the process-wide waker.
    #[test]
    #[should_panic(expected = "wiring bug")]
    fn set_provider_off_ui_thread_panics() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        std::thread::spawn(|| {
            let _ = set_can_pop_provider(Box::new(|| true));
        })
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
