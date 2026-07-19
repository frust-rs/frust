//! Process-wide deep-link source (spec §19, research ledger #7's `app_links`
//! semantics): a shell delivers a platform link (cold-start intent data, or a
//! running app's `onNewIntent`/`openURLContexts`) via [`push_deep_link`]; app
//! code reads the current state through [`deep_links`]/[`DeepLinks`] — the
//! `frust` facade re-exports both as `frust::deep_links()`/
//! `frust::DeepLinks`, so app code never names this crate directly.
//!
//! # `app_links` semantics
//!
//! Unlike the discontinued `uni_links` (an initial link fetched once, then a
//! separate stream for subsequent links), `app_links`-style delivery treats
//! the initial link as just the *first* element of one uniform stream: every
//! link (cold-start and warm) is written to the same [`RwSignal`]
//! ([`DeepLinks::latest`]), so a subscriber that only tracks `latest` sees
//! both uniformly. [`DeepLinks::initial`] additionally snapshots the very
//! first link ever pushed in this process — set at most once, readable any
//! number of times — for callers (the router glue, task 08) that need
//! cold-start precedence without setting up a subscription.
//!
//! **Documented limitation**: a push carries no "this is the cold-start link"
//! flag, so `initial` is simply "whichever link arrived first" in this
//! process. That is correct for the intended case (a real cold-start link
//! always arrives before the first app rebuild, per task 07's queue-until-
//! handle-exists contract), but a session with no real cold-start link whose
//! first-ever push happens to race ahead of the first [`deep_links`] call
//! would also see that push recorded as `initial`. Not a concern in practice
//! given the shell init order.
//!
//! # Thread contract
//!
//! [`push_deep_link`] must be called on the UI thread — the one
//! [`ReactiveRuntime::init`] ran on — mirroring `Executor::spawn_local`'s
//! contract (see `frust-reactive::executor`): the mobile shells' native
//! callbacks always run on the UI thread, so an off-thread call is a wiring
//! bug, not a runtime-data condition, and panics with the same message
//! convention `Executor::spawn_local` uses.
//!
//! A push that races ahead of [`ReactiveRuntime::init`] (an odd shell-
//! ordering edge case — the documented shell flow queues platform-side until
//! the native handle exists, so this should not happen through it) is
//! **dropped with a logged warning** rather than panicking or buffering
//! indefinitely: buffering would need to pick a bound and a flush point for a
//! path that isn't expected to be exercised, and a bare drop can never lose
//! the *cold-start* link specifically (that path is always delivered after
//! init, once the native handle exists).

use std::sync::{Mutex, OnceLock};

use reactive_graph::signal::RwSignal;
use reactive_graph::traits::Set;

use crate::ReactiveRuntime;
use crate::executor::is_ui_thread;

/// One delivered deep link: the raw platform-provided URL/location string
/// (e.g. `myapp://profile/42`), unparsed — turning it into a route is the
/// router layer's job (task 08's router/deep-link glue), not this crate's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepLink {
    pub url: String,
}

impl DeepLink {
    /// Wrap a raw URL/location string.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

/// The app-facing deep-link read surface (see the module docs' `app_links`
/// semantics). Obtained via [`deep_links`] (`frust::deep_links()` at the
/// facade).
#[derive(Clone)]
pub struct DeepLinks {
    /// The first link ever pushed in this process, if any — a plain
    /// snapshot taken when [`deep_links`] is called, not itself reactive.
    /// The underlying value is set at most once (idempotent), so repeated
    /// calls to [`deep_links`] see the same `initial` once it exists.
    pub initial: Option<String>,
    /// The most recently pushed link — cold-start or warm, uniformly (see
    /// the module docs). Read/track it with the `Get`/`Track` traits
    /// (`frust::{Get, Track}`) the same way any other `RwSignal` is read.
    pub latest: RwSignal<Option<DeepLink>>,
}

/// The process-wide deep-link slot: the first-ever-pushed snapshot plus the
/// live `latest` signal. Lazily created (mirroring [`ReactiveRuntime`]'s own
/// process-wide, lazily-installed static) on first access once
/// [`ReactiveRuntime`] exists, without needing to modify
/// `ReactiveRuntime::init` itself.
struct Slot {
    initial: Mutex<Option<String>>,
    latest: RwSignal<Option<DeepLink>>,
}

static SLOT: OnceLock<Slot> = OnceLock::new();

/// Returns the process-wide slot, creating it (under the reactive root
/// [`Owner`](reactive_graph::owner::Owner)) on first access.
///
/// # Panics
///
/// Panics if [`ReactiveRuntime::init`] has not run yet — reading or pushing a
/// deep link before the reactive runtime exists across every other path
/// (`push_deep_link`'s pre-init case is handled separately, before this is
/// ever called) is a genuine wiring bug: the caller must initialize the
/// runtime first.
fn slot() -> &'static Slot {
    SLOT.get_or_init(|| {
        let rt = ReactiveRuntime::get().expect(
            "frust-reactive: deep_links() was called before ReactiveRuntime::init — an app \
             must run under the Frust facade's entry point (which initializes the reactive \
             runtime) before reading deep links",
        );
        Slot {
            initial: Mutex::new(None),
            latest: rt.with_owner(|| RwSignal::new(None)),
        }
    })
}

/// Deliver a platform deep link (cold-start or warm) into the process-wide
/// source. Called by a shell (task 07's Android/iOS FFI glue) on the UI
/// thread; app code never calls this directly.
///
/// The first call in a process snapshots its URL into
/// [`DeepLinks::initial`] (idempotent — later calls do not overwrite it);
/// every call (including the first) also writes [`DeepLinks::latest`], so a
/// tracked reader observes both cold-start and warm links through the same
/// signal (see the module docs' `app_links` semantics).
///
/// # Panics
///
/// Panics if called off the UI thread (see the module docs' thread
/// contract). A call before [`ReactiveRuntime::init`] does **not** panic — it
/// is dropped with a logged warning (see the module docs).
pub fn push_deep_link(url: impl Into<String>) {
    let url = url.into();

    if !is_ui_thread() {
        panic!(
            "frust-reactive: push_deep_link was called off the UI thread. Deep links can \
             only be pushed from the UI thread (the one `ReactiveRuntime::init` ran on) — this \
             is a wiring bug: route the platform delivery through the UI thread before pushing, \
             the same contract `Executor::spawn_local` enforces."
        );
    }

    if ReactiveRuntime::get().is_none() {
        eprintln!(
            "frust-reactive: push_deep_link(\"{url}\") dropped — ReactiveRuntime::init has \
             not run yet. A shell should queue a link platform-side until its native handle \
             exists (see task 07); reaching this indicates an odd init-ordering race, not normal \
             operation."
        );
        return;
    }

    let slot = slot();
    {
        let mut initial = slot
            .initial
            .lock()
            .expect("frust-reactive: deep_link initial mutex poisoned");
        if initial.is_none() {
            *initial = Some(url.clone());
        }
    }
    slot.latest.set(Some(DeepLink::new(url)));
}

/// The current deep-link read surface: a snapshot of [`DeepLinks::initial`]
/// plus the live [`DeepLinks::latest`] signal. Call from a tracked context
/// (e.g. inside `Component::build`) to observe subsequent pushes as they
/// arrive.
pub fn deep_links() -> DeepLinks {
    let slot = slot();
    let initial = slot
        .initial
        .lock()
        .expect("frust-reactive: deep_link initial mutex poisoned")
        .clone();
    DeepLinks {
        initial,
        latest: slot.latest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameWaker, TrackedScope};
    use reactive_graph::traits::{Get, GetUntracked};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A recording waker: an `Arc<AtomicUsize>` bumped once per `wake()`.
    fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = counter.clone();
        let waker: FrameWaker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (waker, seen)
    }

    /// Every acceptance criterion in one `#[test]`, serialized on the shared
    /// waker lock (`ReactiveRuntime::init` here swaps the process-wide waker,
    /// which would otherwise race the other waker-asserting tests in this
    /// crate — see `WAKER_TEST_LOCK`'s doc comment in `lib.rs`).
    #[test]
    fn deep_link_push_and_wake_bridge() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (waker, wakes) = recording_waker();
        let _rt = ReactiveRuntime::init(waker);

        // Criterion: push before any tracked read — a late subscriber sees it
        // immediately via BOTH the `initial` snapshot and the live `latest`
        // signal (app_links semantics: cold-start and warm links flow through
        // the same source).
        let url1 = "frust-test://a/1".to_string();
        push_deep_link(url1.clone());

        let links = deep_links();
        assert_eq!(links.initial.as_deref(), Some(url1.as_str()));
        assert_eq!(
            links.latest.get_untracked(),
            Some(DeepLink::new(url1.clone()))
        );

        // A tracked scope reading `latest` after the push observes the
        // already-pushed link with no wake needed — an ordinary read.
        let scope = TrackedScope::new();
        let seen = scope.track(|| deep_links().latest.get());
        assert_eq!(seen, Some(DeepLink::new(url1.clone())));
        assert!(!scope.is_dirty(), "a fresh track starts clean");

        // Criterion: push after a tracked read fires the signal-write -> wake
        // contract — the tracked scope re-dirties and the waker fires exactly
        // once (coalesced).
        let before = wakes.load(Ordering::SeqCst);
        let url2 = "frust-test://b/2".to_string();
        push_deep_link(url2.clone());
        assert!(
            scope.is_dirty(),
            "push_deep_link must dirty a scope tracking `latest`"
        );
        assert_eq!(
            wakes.load(Ordering::SeqCst) - before,
            1,
            "a push must fire the waker exactly once"
        );

        // `initial` is set once and never overwritten, even though `latest`
        // has since moved on to the second link.
        let links2 = deep_links();
        assert_eq!(
            links2.initial.as_deref(),
            Some(url1.as_str()),
            "initial is set once and stays"
        );
        assert_eq!(links2.latest.get_untracked(), Some(DeepLink::new(url2)));
    }

    /// Criterion: the UI-thread contract is enforced (mirrors
    /// `Executor::spawn_local`'s off-thread panic). Also serializes on the
    /// waker lock since `ReactiveRuntime::init` swaps the process-wide waker.
    #[test]
    #[should_panic(expected = "wiring bug")]
    fn push_off_ui_thread_panics() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        std::thread::spawn(|| {
            push_deep_link("frust-test://off-thread");
        })
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
