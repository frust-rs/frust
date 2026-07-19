//! The custom [`any_spawner`] executor that routes `spawn` to the background
//! tokio runtime and `spawn_local` to a UI-thread-owned local task queue.
//!
//! `any_spawner`'s built-in `init_tokio()` is unusable here: its `spawn_local`
//! calls `tokio::task::spawn_local`, which panics unless the current thread is
//! inside a `LocalSet` — and the UI thread has none (see the phase-5.5 research
//! doc §2). This module is the fix: `spawn` hands `Send` futures to the
//! background runtime, while `spawn_local` pushes `!Send` futures onto a
//! `thread_local!` [`LocalPool`] that the shell drains via
//! [`ReactiveRuntime::pump_local`](crate::ReactiveRuntime::pump_local).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use any_spawner::{CustomExecutor, PinnedFuture, PinnedLocalFuture};
use futures::executor::{LocalPool, LocalSpawner};
use futures::task::LocalSpawnExt;
use tokio::runtime::Handle;

// The thread-locals live in an allow-scoped module: on the mobile targets
// (no `#[thread_local]` fast path) std's macro expands through the `os_local`
// path, where clippy's `missing_const_for_thread_local` misfires — it flags
// even the already-const `IS_UI_THREAD`, and a per-static `#[allow]` does not
// survive that expansion. `LOCAL_POOL`/`LOCAL_SPAWNER` genuinely cannot be
// `const` (`LocalPool::new()`/`spawner()` are not const fns).
#[allow(clippy::missing_const_for_thread_local)]
mod tls {
    use super::*;

    thread_local! {
        /// The UI thread's local task queue for `!Send` futures. Empty on every
        /// other thread (each thread has its own, so pumping off the UI thread
        /// is a harmless no-op rather than a panic).
        pub(super) static LOCAL_POOL: RefCell<LocalPool> = RefCell::new(LocalPool::new());
        /// Spawner into `LOCAL_POOL`, cached to avoid re-deriving it per spawn.
        pub(super) static LOCAL_SPAWNER: LocalSpawner =
            LOCAL_POOL.with(|pool| pool.borrow().spawner());
        /// Set on the thread that called [`init_ui_thread`]; gates `spawn_local`.
        pub(super) static IS_UI_THREAD: Cell<bool> = const { Cell::new(false) };
    }
}
use tls::{IS_UI_THREAD, LOCAL_POOL, LOCAL_SPAWNER};

/// Marks the calling thread as the UI thread — the one that owns the local task
/// queue and drains it via `pump_local`. Called from `ReactiveRuntime::init`,
/// which is documented to run on the UI thread.
pub(crate) fn init_ui_thread() {
    IS_UI_THREAD.with(|flag| flag.set(true));
}

/// Whether the calling thread is the UI thread.
pub(crate) fn is_ui_thread() -> bool {
    IS_UI_THREAD.with(Cell::get)
}

/// Runs the calling thread's local task queue until it stalls. The caller is
/// responsible for establishing a tokio runtime context first (so that
/// `tokio::time::sleep` inside a local task can register with the background
/// runtime's timer driver) — see [`ReactiveRuntime::pump_local`].
///
/// [`ReactiveRuntime::pump_local`]: crate::ReactiveRuntime::pump_local
pub(crate) fn run_until_stalled() {
    LOCAL_POOL.with(|pool| {
        // `try_borrow_mut` guards against a re-entrant pump (a task that itself
        // triggers a pump): if the pool is already borrowed we are nested, so
        // do nothing rather than double-borrow-panic.
        if let Ok(mut pool) = pool.try_borrow_mut() {
            pool.run_until_stalled();
        }
    });
}

/// A [`Waker`] that, when woken, nudges BOTH the internal `LocalPool` task
/// bookkeeping AND the shell.
///
/// The `LocalPool` hands each polled task its own internal waker via the poll
/// `Context`. A future that returns `Pending` (e.g. one awaiting a
/// `tokio::time::sleep`) registers *that* waker with whatever will re-wake it —
/// here, the background runtime's timer driver, running on a tokio worker
/// thread. When the timer fires it wakes only the `LocalPool` waker, marking the
/// task ready in the pool's internal queue — but nothing tells the shell to
/// actually *run* [`ReactiveRuntime::pump_local`], so a parked desktop
/// `ControlFlow::Wait` loop never drains the ready task (see ../BUG.md B1).
///
/// [`WakeBridge`] substitutes this composite waker for the pool's own before
/// polling the inner future, so a re-wake from any source both (a) keeps the
/// pool's bookkeeping intact (`inner.wake`) and (b) fires the installed
/// [`FrameWaker`](crate::FrameWaker) so the shell pumps
/// ([`ReactiveRuntime::wake`](crate::ReactiveRuntime::wake)).
///
/// `Send + Sync` is required because re-wakes arrive from the tokio timer
/// thread; the wrapped `inner` [`Waker`] is `Send + Sync` and
/// `ReactiveRuntime::wake` reaches a process-global. On mobile the installed
/// waker is a no-op (the continuous frame loop needs no nudge — see
/// [`ReactiveRuntime::init`](crate::ReactiveRuntime::init)), so firing it there
/// is harmless.
struct CompositeWaker {
    /// The `LocalPool`'s own waker for this task — must still be woken so the
    /// pool moves the task from parked to ready.
    inner: Waker,
}

impl CompositeWaker {
    /// Fires the installed [`FrameWaker`] by resolving the runtime at wake time.
    ///
    /// Reads the CURRENT runtime/waker rather than a captured clone: a relaunched
    /// shell swaps the waker via `ReactiveRuntime::init` without rebuilding the
    /// runtime, and `ReactiveRuntime::wake` re-reads the live waker under its
    /// lock — so a re-wake always nudges the shell that currently owns wake-up
    /// (the same path spawn-time firing uses).
    ///
    /// **Shell-waker contract note:** this composite makes the [`FrameWaker`]
    /// fire from arbitrary threads (e.g. the background tokio TIMER thread
    /// completing a `sleep` a `spawn_local` future awaits) — a newly-reachable
    /// call site beyond the UI-thread spawn/signal paths. A shell's waker was
    /// always required to be `Send + Sync` and callable from any thread; it
    /// must also be panic-free off the UI thread, or the panic unwinds into
    /// the tokio timer driver.
    fn wake_shell() {
        if let Some(rt) = crate::ReactiveRuntime::get() {
            rt.wake();
        }
    }
}

impl Wake for CompositeWaker {
    fn wake(self: Arc<Self>) {
        Self::wake_shell();
        self.inner.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        Self::wake_shell();
        self.inner.wake_by_ref();
    }
}

/// Wraps a `spawn_local` future so every poll swaps the `LocalPool`'s task waker
/// for a [`CompositeWaker`]. This is the seam that closes the desktop wake gap
/// (../BUG.md B1): whatever the inner future clones out of the poll `Context`
/// and hands to its re-wake source is the composite waker, so a later re-wake
/// fires the [`FrameWaker`](crate::FrameWaker) too, not just the pool's own.
struct WakeBridge {
    inner: PinnedLocalFuture<()>,
}

impl Future for WakeBridge {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // `WakeBridge` holds only a `Pin<Box<..>>` (itself `Unpin`) and no
        // self-referential state, so it is `Unpin` and `get_mut` needs no
        // `unsafe`.
        let this = self.get_mut();
        let composite = Waker::from(Arc::new(CompositeWaker {
            inner: cx.waker().clone(),
        }));
        let mut inner_cx = Context::from_waker(&composite);
        this.inner.as_mut().poll(&mut inner_cx)
    }
}

/// The Frust `any_spawner` executor. Stored inside `any_spawner`'s global
/// `OnceLock`, so it must be `Send + Sync + 'static` — it is, holding only a
/// cloneable [`Handle`].
pub(crate) struct ForgeExecutor {
    handle: Handle,
}

impl ForgeExecutor {
    pub(crate) fn new(handle: Handle) -> Self {
        Self { handle }
    }
}

impl CustomExecutor for ForgeExecutor {
    /// `Send` futures go to the background tokio runtime.
    fn spawn(&self, fut: PinnedFuture<()>) {
        self.handle.spawn(fut);
    }

    /// `!Send` futures go onto the UI thread's local queue, and the frame waker
    /// is fired so the shell pumps the queue soon.
    fn spawn_local(&self, fut: PinnedLocalFuture<()>) {
        if !is_ui_thread() {
            // Cross-thread `spawn_local` hand-off is unsupported: the futures
            // are `!Send`, so they can only be enqueued on the thread that owns
            // the queue. Reaching here means a background task tried to spawn a
            // local future — a wiring bug, not a runtime-data condition.
            panic!(
                "frust-reactive: Executor::spawn_local was called off the UI \
                 thread. `!Send` local futures can only be spawned on the UI \
                 thread (the one `ReactiveRuntime::init` ran on). This is a \
                 wiring bug: route the work through `Executor::spawn` instead, or \
                 hand it back to the UI thread before spawning it locally."
            );
        }

        // Wrap the future so each poll installs a composite waker: a later
        // re-wake (e.g. the tokio timer driver completing a `sleep` the future
        // awaits) then fires the FrameWaker too, not just the LocalPool's own
        // waker — closing the desktop wake gap (see [`WakeBridge`] / ../BUG.md
        // B1). Without this, only the spawn-time `wake()` below ever nudges the
        // shell, so a parked desktop loop never pumps the completion.
        let fut = WakeBridge { inner: fut };
        LOCAL_SPAWNER.with(|spawner| {
            spawner
                .spawn_local(fut)
                .expect("frust-reactive: UI-thread local task queue rejected a future");
        });

        // Ask the shell to pump the queue on its next turn. The runtime is
        // installed before the executor, so `get()` is always `Some` here.
        if let Some(rt) = crate::ReactiveRuntime::get() {
            rt.wake();
        }
    }

    /// Drains the local queue under a runtime context (so local-task timers
    /// register with the background runtime's timer driver).
    fn poll_local(&self) {
        let _guard = self.handle.enter();
        run_until_stalled();
    }
}

#[cfg(test)]
mod tests {
    use crate::{FrameWaker, ReactiveRuntime};
    use any_spawner::Executor;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A recording waker: an `Arc<AtomicUsize>` bumped once per `wake()`.
    fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = counter.clone();
        let waker: FrameWaker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (waker, seen)
    }

    /// The exact broken link from ../BUG.md B1: a spawned local future that
    /// returns `Pending` awaiting a `tokio::time::sleep` is re-woken by the
    /// background runtime's timer driver *from a tokio worker thread*. That
    /// re-wake must fire the FrameWaker so a parked shell pumps — pre-fix it
    /// reached only the `LocalPool`'s internal waker and the count never moved
    /// after the initial spawn (verified failing against the pre-fix executor
    /// via `git stash`).
    ///
    /// Serialized on `WAKER_TEST_LOCK` because the FrameWaker is process-global
    /// and swappable (see its doc in `lib.rs`).
    #[test]
    fn spawn_local_rewake_from_timer_thread_fires_frame_waker() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (waker, count) = recording_waker();
        let rt = ReactiveRuntime::init(waker);

        let done = Arc::new(AtomicUsize::new(0));
        let flag = done.clone();

        let before = count.load(Ordering::SeqCst);
        Executor::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            flag.fetch_add(1, Ordering::SeqCst);
        });
        // The spawn-time firing bumps the waker exactly once (existing contract).
        assert_eq!(
            count.load(Ordering::SeqCst),
            before + 1,
            "spawn_local should fire the frame waker once at spawn"
        );

        // Drain the initial pump: the future registers its sleep timer with the
        // background runtime's timer driver and returns Pending — i.e. stalls.
        rt.pump_local();
        assert_eq!(
            done.load(Ordering::SeqCst),
            0,
            "future must still be pending — the 50ms timer has not fired"
        );
        let after_pump = count.load(Ordering::SeqCst);

        // Wait for the timer thread's re-wake WITHOUT any further pump. This is
        // the assertion that fails pre-fix: the re-wake reaches only the
        // LocalPool waker, so the FrameWaker count never moves here. Post-fix the
        // composite waker fires it from the timer thread.
        let start = Instant::now();
        while count.load(Ordering::SeqCst) == after_pump && start.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            count.load(Ordering::SeqCst) > after_pump,
            "a timer-driven re-wake must fire the FrameWaker with no pump in \
             between (this is the desktop wake gap the composite waker closes)"
        );

        // Now that a wake arrived, a pump drains the ready task to completion.
        let start = Instant::now();
        while done.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            rt.pump_local();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            done.load(Ordering::SeqCst),
            1,
            "the local future should complete after the re-wake + pump"
        );
    }
}
