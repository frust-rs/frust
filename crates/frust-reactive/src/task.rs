//! The blessed heavy-work idiom (phase 9.A): [`AsyncValue<T>`] + [`use_task`].
//!
//! This is Frust's direct counterpart to Flutter's
//! `compute()`/`FutureBuilder` — a one-call way to run heavy work off the UI
//! thread and get exhaustive load/error/ready states back, with cancellation
//! semantics Flutter's model doesn't offer.
//!
//! # The idiom
//!
//! ```ignore
//! // inside `Component::init`/`build`, under the component's `Owner`:
//! let data = use_task(|| async { spawn_blocking(parse).await });
//! // `data.signal()` is an `RwSignal<AsyncValue<T>>` read in `build`;
//! // `data.restart()` re-runs the fetch.
//! ```
//!
//! # Threading and cancellation contract
//!
//! `use_task` splits the work into two halves, wired explicitly (never
//! assuming any implicit cancellation — the leptos precedent shows
//! "implicit cancellation" claims are usually wrong; see the phase-9
//! research ledger §9):
//!
//! 1. **A background half** — the fetcher future is handed to the process-wide
//!    tokio runtime via [`ReactiveRuntime`]'s handle. Heavy work inside it
//!    hops threads through [`crate::spawn_blocking`] (one-off CPU work) or
//!    `frust::spawn` (async IO). The tokio [`JoinHandle`](tokio::task::JoinHandle)'s
//!    [`AbortHandle`](tokio::task::AbortHandle) is registered in `on_cleanup`,
//!    so owner teardown aborts the background task (best-effort: a
//!    `spawn_blocking` closure *already running* cannot be interrupted — a
//!    documented limitation shared by every runtime).
//! 2. **A UI-side coordinator** — a `!Send` future spawned via
//!    reactive_graph's [`spawn_local_scoped_with_cancellation`] so it aborts
//!    on owner cleanup. It `await`s the background [`JoinHandle`](tokio::task::JoinHandle)
//!    and only then writes the result signal.
//!
//! Because **every signal write happens on the UI thread** (the coordinator
//! awaits the background result, then sets), the same-frame cross-thread
//! write/read race the huddle review deferred as A10 is *structurally
//! impossible* for idiom users: there is no background-thread `signal.set`,
//! and a coordinator aborted by owner cleanup never writes to a disposed
//! signal (closing A9). The `use_task` stress tests below are the audit A10
//! asked for, executed against the idiom rather than by inspection.

use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use reactive_graph::owner::{Owner, on_cleanup};
use reactive_graph::signal::RwSignal;
use reactive_graph::spawn_local_scoped_with_cancellation;
use reactive_graph::traits::{Set, Update};
use tokio::task::AbortHandle;

use crate::runtime::ReactiveRuntime;

/// The boxed error an [`AsyncValue::Error`] carries. `Arc`-wrapped so a
/// clone of the state is cheap and the error is shareable across the tree.
pub type TaskError = Arc<dyn std::error::Error + Send + Sync>;

/// The exhaustive state of an asynchronously-loaded value (phase 9.A).
///
/// Deliberately named for parity with Riverpod's `AsyncValue<T>` (Flutter) —
/// the genuine prior art for a load/data/error sum type with exhaustive
/// matching (research §9). The four states are:
///
/// - [`Idle`](Self::Idle) — nothing requested yet.
/// - [`Loading`](Self::Loading) — a fetch is in flight. It carries the
///   *previous* value (`Some` on a refresh, `None` on a first load), so a UI
///   can keep showing stale data instead of flickering to a spinner —
///   mirroring Riverpod's `copyWithPrevious`.
/// - [`Ready`](Self::Ready) — the fetch resolved to a value.
/// - [`Error`](Self::Error) — the fetch failed.
#[derive(Default)]
pub enum AsyncValue<T> {
    /// No fetch requested yet.
    #[default]
    Idle,
    /// A fetch is in flight; carries the previous value for
    /// refresh-without-flicker (`None` on a first load).
    Loading(Option<T>),
    /// The fetch resolved.
    Ready(T),
    /// The fetch failed.
    Error(TaskError),
}

impl<T> AsyncValue<T> {
    /// Whether this is [`Idle`](Self::Idle).
    pub fn is_idle(&self) -> bool {
        matches!(self, AsyncValue::Idle)
    }

    /// Whether a fetch is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self, AsyncValue::Loading(_))
    }

    /// Whether the fetch resolved to a value.
    pub fn is_ready(&self) -> bool {
        matches!(self, AsyncValue::Ready(_))
    }

    /// Whether the fetch failed.
    pub fn is_error(&self) -> bool {
        matches!(self, AsyncValue::Error(_))
    }

    /// The resolved value, if [`Ready`](Self::Ready).
    pub fn ready(&self) -> Option<&T> {
        match self {
            AsyncValue::Ready(t) => Some(t),
            _ => None,
        }
    }

    /// The best-available value: the resolved one when [`Ready`](Self::Ready),
    /// or the carried-over previous one while [`Loading`](Self::Loading) a
    /// refresh. This is what a flicker-free UI reads.
    pub fn value(&self) -> Option<&T> {
        match self {
            AsyncValue::Ready(t) => Some(t),
            AsyncValue::Loading(prev) => prev.as_ref(),
            _ => None,
        }
    }

    /// The error, if [`Error`](Self::Error).
    pub fn error(&self) -> Option<&TaskError> {
        match self {
            AsyncValue::Error(e) => Some(e),
            _ => None,
        }
    }
}

impl<T: Clone> Clone for AsyncValue<T> {
    fn clone(&self) -> Self {
        match self {
            AsyncValue::Idle => AsyncValue::Idle,
            AsyncValue::Loading(prev) => AsyncValue::Loading(prev.clone()),
            AsyncValue::Ready(t) => AsyncValue::Ready(t.clone()),
            AsyncValue::Error(e) => AsyncValue::Error(e.clone()),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for AsyncValue<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsyncValue::Idle => f.write_str("Idle"),
            AsyncValue::Loading(prev) => f.debug_tuple("Loading").field(prev).finish(),
            AsyncValue::Ready(t) => f.debug_tuple("Ready").field(t).finish(),
            AsyncValue::Error(e) => f.debug_tuple("Error").field(e).finish(),
        }
    }
}

/// The handle [`use_task`] returns: a read handle to the task's
/// [`AsyncValue<T>`] state plus a `restart`/refresh trigger.
///
/// Read the state reactively via [`signal`](Self::signal) (or the [`Get`]
/// trait on it) from `Component::build`; call [`restart`](Self::restart) to
/// re-run the fetch (e.g. a pull-to-refresh).
///
/// [`Get`]: reactive_graph::traits::Get
pub struct UseTask<T: Send + Sync + 'static> {
    signal: RwSignal<AsyncValue<T>>,
    // `Rc<dyn Fn()>` — UI-thread-only; re-runs the fetch under the captured
    // owner so cancellation stays wired even when `restart` fires from an
    // arbitrary (owner-less) event-handler context.
    run: Rc<dyn Fn()>,
}

impl<T: Send + Sync + 'static> UseTask<T> {
    /// The reactive state signal. Read it with the [`Get`] trait
    /// (`task.signal().get()`) inside a tracked `Component::build` so a state
    /// transition wakes the shell.
    ///
    /// [`Get`]: reactive_graph::traits::Get
    pub fn signal(&self) -> RwSignal<AsyncValue<T>> {
        self.signal
    }

    /// Re-runs the fetch: transitions the state to
    /// [`Loading`](AsyncValue::Loading) (carrying the current value for
    /// refresh-without-flicker), aborts any in-flight background task, and
    /// starts a fresh one. Last-write-wins by generation, so a restart storm
    /// settles on the newest fetch's result regardless of completion order.
    pub fn restart(&self) {
        (self.run)();
    }
}

impl<T: Send + Sync + 'static> Clone for UseTask<T> {
    fn clone(&self) -> Self {
        UseTask {
            signal: self.signal,
            run: self.run.clone(),
        }
    }
}

/// Shared, UI-thread-owned coordination state for one [`use_task`] instance.
struct Coordinator<T: Send + Sync + 'static> {
    signal: RwSignal<AsyncValue<T>>,
    /// Bumped on every run; the coordinator only writes its result if its
    /// captured generation still matches (last-write-wins under a restart
    /// storm). UI-thread-only, so a plain [`Cell`] suffices.
    generation: Cell<u64>,
    /// The current background task's abort handle, shared with the single
    /// `on_cleanup` registration so owner teardown aborts it. `Arc<Mutex<_>>`
    /// because `on_cleanup` requires a `Send + Sync` closure — the tokio
    /// [`AbortHandle`] is itself `Send + Sync`.
    bg_abort: Arc<Mutex<Option<AbortHandle>>>,
}

/// Runs a fetch, wiring a heavy-work idiom around it (phase 9.A).
///
/// Called from `Component::init`/`build` under the component's [`Owner`]. It
/// immediately starts a first fetch and returns a [`UseTask<T>`] to read the
/// [`AsyncValue<T>`] state and to `restart` it.
///
/// - `T` is the loaded value type (`Send + Sync` — it moves from a background
///   thread to the UI thread, and lives in a thread-safe signal).
/// - `E` is any [`std::error::Error`] the fetch may fail with (tokio's
///   `JoinError` qualifies, so `|| async { spawn_blocking(f).await }` works
///   directly).
///
/// See the [module docs](self) for the full threading/cancellation contract.
///
/// # Decision: hand-rolled vs `AsyncDerived`
///
/// reactive_graph 0.2 ships `AsyncDerived` (research §9), which this could
/// wrap for a *signal-driven* restart. It is deliberately **not** used here:
/// `AsyncDerived` re-runs when a tracked signal it reads changes, whereas
/// `use_task`'s contract is an *imperative* first-load + explicit `restart`
/// (the pull-to-refresh / retry shape), and its cancellation story is the
/// leptos one the research refuted as non-explicit. Hand-rolling keeps all
/// three guarantees visible in one place — background `AbortHandle` in
/// `on_cleanup`, coordinator abort via `spawn_local_scoped_with_cancellation`,
/// and last-write-wins by generation. A future `AsyncDerived`-backed
/// signal-driven variant can live alongside this without changing it (a
/// documented Future Enhancement in the plan).
pub fn use_task<T, E, Fut, F>(fetch: F) -> UseTask<T>
where
    T: Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
    F: Fn() -> Fut + 'static,
{
    let coord = Rc::new(Coordinator {
        signal: RwSignal::new(AsyncValue::Idle),
        generation: Cell::new(0),
        bg_abort: Arc::new(Mutex::new(None)),
    });

    // Register a single owner-cleanup that aborts whatever background task is
    // current at teardown time. Capturing only the `Send + Sync` abort slot
    // (not the `!Send` `Rc<Coordinator>`) keeps the closure within
    // `on_cleanup`'s bound.
    {
        let bg_abort = coord.bg_abort.clone();
        on_cleanup(move || {
            if let Some(handle) = bg_abort.lock().expect("bg_abort poisoned").take() {
                handle.abort();
            }
        });
    }

    let signal = coord.signal;
    let fetch = Rc::new(fetch);

    // Capture the owner so every run (initial + restart) re-enters it: the
    // coordinator's `spawn_local_scoped_with_cancellation` and the background
    // `on_cleanup` both bind to *this* owner, even if `restart` is called from
    // an owner-less context (an event handler).
    let owner = Owner::current();
    let run: Rc<dyn Fn()> = {
        let coord = coord.clone();
        let fetch = fetch.clone();
        Rc::new(move || {
            let go = || run_once(&coord, &fetch);
            match &owner {
                Some(owner) => owner.with(go),
                None => go(),
            }
        })
    };

    // Kick off the first load.
    run();

    UseTask { signal, run }
}

/// One fetch cycle: supersede any prior run, flip to `Loading`, spawn the
/// background work, and spawn the UI-side coordinator that writes the result.
fn run_once<T, E, Fut, F>(coord: &Rc<Coordinator<T>>, fetch: &Rc<F>)
where
    T: Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
    F: Fn() -> Fut + 'static,
{
    let rt = match ReactiveRuntime::get() {
        Some(rt) => rt,
        // No runtime installed (init never ran). Nothing to do — the state
        // stays `Idle` rather than panicking.
        None => return,
    };

    // Abort the previous in-flight background task (restart supersedes it).
    if let Some(handle) = coord.bg_abort.lock().expect("bg_abort poisoned").take() {
        handle.abort();
    }

    // Claim a fresh generation; only this run may write its result.
    let generation = coord.generation.get().wrapping_add(1);
    coord.generation.set(generation);

    // Transition to Loading, carrying the current value for a flicker-free
    // refresh.
    coord.signal.update(|state| {
        let prev = match std::mem::take(state) {
            AsyncValue::Ready(t) => Some(t),
            AsyncValue::Loading(prev) => prev,
            _ => None,
        };
        *state = AsyncValue::Loading(prev);
    });

    // Background half: hand the fetcher future to the tokio runtime and
    // register its abort handle for owner teardown / the next restart.
    let join = rt.handle().spawn((fetch)());
    *coord.bg_abort.lock().expect("bg_abort poisoned") = Some(join.abort_handle());

    // UI-side coordinator: await the background result, then write the signal
    // on the UI thread. Scoped-with-cancellation so owner cleanup aborts it —
    // a disposed signal is never written.
    let coord = coord.clone();
    spawn_local_scoped_with_cancellation(async move {
        let outcome = join.await;

        // Last-write-wins: a newer run has already claimed the signal.
        if coord.generation.get() != generation {
            return;
        }

        match outcome {
            Ok(Ok(value)) => coord.signal.set(AsyncValue::Ready(value)),
            Ok(Err(err)) => {
                let err: TaskError = Arc::new(err);
                coord.signal.set(AsyncValue::Error(err));
            }
            // The background task was aborted (owner teardown or a restart) or
            // panicked. On abort the coordinator is normally torn down too, so
            // this arm is a race-safe fallback: leave the state as the caller
            // (or the newer run) set it rather than clobbering it.
            Err(_join_err) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactive_graph::owner::Owner;
    use reactive_graph::traits::GetUntracked;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use crate::ReactiveRuntime;

    fn noop_waker() -> crate::FrameWaker {
        Arc::new(|| {})
    }

    /// Ensures a shared, initialized runtime on the calling (UI) thread.
    fn init_rt() -> &'static ReactiveRuntime {
        ReactiveRuntime::init(noop_waker())
    }

    /// Pump the UI-thread local queue until `cond` holds or the deadline
    /// passes; returns whether `cond` became true. The 1ms yield between pumps
    /// is not a *synchronization* device — it only lets the background tokio
    /// task make progress; correctness is asserted on `cond`, not on elapsed
    /// time.
    fn pump_until(rt: &ReactiveRuntime, timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            rt.pump_local();
            if cond() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Pump a fixed handful of turns to drain aborted coordinators; used where
    /// the assertion is "no panic" rather than a state condition.
    fn pump_a_few(rt: &ReactiveRuntime) {
        for _ in 0..4 {
            rt.pump_local();
        }
    }

    #[derive(Debug)]
    struct TestError(&'static str);
    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for TestError {}

    /// A `use_task` fetch resolves to `Ready` on the UI thread after the
    /// background work completes. This is the milestone shape:
    /// `use_task(|| async { spawn_blocking(f).await })` (fetch error type is
    /// tokio's `JoinError`).
    #[test]
    fn use_task_resolves_to_ready() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        let owner = Owner::new();
        let task = owner.with(|| use_task(|| async { crate::spawn_blocking(|| 6 * 7).await }));

        assert!(
            task.signal().get_untracked().is_loading(),
            "state must be Loading immediately after use_task"
        );
        assert!(
            pump_until(rt, Duration::from_secs(5), || task
                .signal()
                .get_untracked()
                .is_ready()),
            "task should reach Ready after pumping"
        );
        assert_eq!(task.signal().get_untracked().ready().copied(), Some(42));

        owner.cleanup();
    }

    /// A failing fetch lands in `Error`, carrying the fetch's own error type.
    #[test]
    fn use_task_reports_error() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        let owner = Owner::new();
        let task = owner.with(|| {
            use_task(|| async {
                crate::spawn_blocking(|| ()).await.expect("join");
                Result::<i32, TestError>::Err(TestError("boom"))
            })
        });

        assert!(
            pump_until(rt, Duration::from_secs(5), || task
                .signal()
                .get_untracked()
                .is_error()),
            "task should reach Error after pumping"
        );
        assert_eq!(
            task.signal().get_untracked().error().map(|e| e.to_string()),
            Some("boom".to_string())
        );

        owner.cleanup();
    }

    /// `Loading` carries the previous `Ready` value across a `restart`, so a
    /// refresh can render stale data instead of flickering to a spinner.
    #[test]
    fn restart_carries_previous_value_in_loading() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        let seq = Arc::new(AtomicUsize::new(0));
        let owner = Owner::new();
        let task = {
            let seq = seq.clone();
            owner.with(|| {
                use_task(move || {
                    let seq = seq.clone();
                    async move {
                        crate::spawn_blocking(move || seq.fetch_add(1, Ordering::SeqCst)).await
                    }
                })
            })
        };

        assert!(pump_until(rt, Duration::from_secs(5), || task
            .signal()
            .get_untracked()
            .is_ready()));
        assert_eq!(task.signal().get_untracked().ready().copied(), Some(0));

        task.restart();
        // Immediately after restart: Loading, but carrying the previous value.
        assert_eq!(
            task.signal().get_untracked().value().copied(),
            Some(0),
            "Loading must carry the previous Ready value for flicker-free refresh"
        );
        assert!(task.signal().get_untracked().is_loading());

        owner.cleanup();
    }

    /// A9/A10 core: mount, start a load, unmount *while loading*, then let the
    /// background task complete — no panic, and no write to the disposed
    /// signal (the coordinator is aborted on cleanup). ≥1000 iterations,
    /// headless.
    #[test]
    fn unmount_while_loading_never_writes_disposed_signal() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        for i in 0..1_000u32 {
            let ran = Arc::new(AtomicUsize::new(0));
            let owner = Owner::new();
            let task = {
                let ran = ran.clone();
                owner.with(|| {
                    use_task(move || {
                        let ran = ran.clone();
                        async move {
                            crate::spawn_blocking(move || {
                                ran.fetch_add(1, Ordering::SeqCst);
                                i
                            })
                            .await
                        }
                    })
                })
            };

            // Tear the owner down while the fetch is (almost certainly) still
            // in flight: runs the on_cleanup that aborts the coordinator and
            // the background task, and disposes the signal.
            owner.cleanup();
            drop(task);

            // Pump a few turns; the background task may still complete, but the
            // aborted coordinator never writes the disposed signal. The only
            // guarantee under test is "no panic".
            pump_a_few(rt);
        }
    }

    /// A task that only completes *after* teardown must not panic when its
    /// background work finishes. ≥1000 iterations.
    #[test]
    fn task_completing_after_teardown_is_safe() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        for _ in 0..1_000u32 {
            let (tx, rx) = mpsc::channel::<()>();
            let rx = Arc::new(Mutex::new(rx));
            let owner = Owner::new();
            let task = {
                let rx = rx.clone();
                owner.with(|| {
                    use_task(move || {
                        let rx = rx.clone();
                        async move {
                            // Block the background task until *after* teardown,
                            // guaranteeing "completes after teardown".
                            crate::spawn_blocking(move || {
                                let _ = rx.lock().expect("rx").recv();
                                1u32
                            })
                            .await
                        }
                    })
                })
            };

            owner.cleanup();
            drop(task);
            // Release the background task only now: it completes post-teardown.
            let _ = tx.send(());
            pump_a_few(rt);
        }
    }

    /// Restart storm: hammer `restart` many times; the final state settles on
    /// the newest fetch's value regardless of completion order (the generation
    /// guard is last-write-wins). ≥1000 restarts.
    #[test]
    fn restart_storm_settles_on_latest() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        let counter = Arc::new(AtomicUsize::new(0));
        let owner = Owner::new();
        let task = {
            let counter = counter.clone();
            owner.with(|| {
                use_task(move || {
                    // Assign the sequence number at fetch-*call* time (run_once
                    // runs synchronously in generation order), not at poll time
                    // (background scheduling order is nondeterministic) — so the
                    // newest generation deterministically owns the highest seq.
                    let seq = counter.fetch_add(1, Ordering::SeqCst);
                    async move { crate::spawn_blocking(move || seq).await }
                })
            })
        };

        for _ in 0..1_000u32 {
            task.restart();
        }
        let last_seq = counter.load(Ordering::SeqCst) - 1;

        assert!(
            pump_until(rt, Duration::from_secs(10), || {
                matches!(
                    task.signal().get_untracked().ready().copied(),
                    Some(seq) if seq == last_seq
                )
            }),
            "restart storm must settle on the newest fetch's value (last-write-wins)"
        );

        owner.cleanup();
    }

    /// Cross-thread completion ordering: an *earlier* fetch that finishes
    /// *later* must not clobber a *newer* fetch that already resolved. The
    /// ordering is enforced explicitly with a channel, not wall-clock timing.
    /// ≥1000 iterations.
    #[test]
    fn out_of_order_completion_respects_generation() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let rt = init_rt();

        for _ in 0..1_000u32 {
            let seq = Arc::new(AtomicUsize::new(0));
            // The first fetch blocks on this receiver until we release it.
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let release_rx = Arc::new(Mutex::new(release_rx));

            let owner = Owner::new();
            let task = {
                let seq = seq.clone();
                let release_rx = release_rx.clone();
                owner.with(|| {
                    use_task(move || {
                        let n = seq.fetch_add(1, Ordering::SeqCst);
                        let release_rx = release_rx.clone();
                        async move {
                            crate::spawn_blocking(move || {
                                if n == 0 {
                                    // First fetch: block until released, so it
                                    // completes AFTER the newer one.
                                    let _ = release_rx.lock().expect("rx").recv();
                                }
                                n
                            })
                            .await
                        }
                    })
                })
            };

            // Second fetch supersedes the (blocked) first.
            task.restart();

            // The newer fetch (seq == 1) resolves first.
            assert!(
                pump_until(rt, Duration::from_secs(5), || matches!(
                    task.signal().get_untracked().ready().copied(),
                    Some(1)
                )),
                "the newer fetch must resolve to 1"
            );

            // Release the stale first fetch; it completes now but must NOT
            // overwrite 1 (its generation is stale).
            let _ = release_tx.send(());
            pump_a_few(rt);
            assert_eq!(
                task.signal().get_untracked().ready().copied(),
                Some(1),
                "a stale, later-completing fetch must not clobber the newer result"
            );

            owner.cleanup();
        }
    }
}
