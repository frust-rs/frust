//! [`ReactiveRuntime`]: the process-wide reactive substrate.
//!
//! It owns a background tokio runtime, installs the custom [`any_spawner`]
//! executor, holds the root reactive [`Owner`], and carries a swappable
//! [`FrameWaker`] the executor fires to nudge the shell into pumping the
//! UI-thread local task queue.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use any_spawner::Executor;
use reactive_graph::owner::Owner;
use tokio::runtime::{Builder, Handle, Runtime};

use crate::executor::{self, ForgeExecutor};

/// A thread-safe, cheaply-cloneable "wake up and pump soon" callback. On
/// desktop this is a winit `EventLoopProxy` send; on mobile (continuous frame
/// loop) it is a no-op. It must be callable from any thread, since the executor
/// fires it and background tasks may drive signal writes.
pub type FrameWaker = Arc<dyn Fn() + Send + Sync>;

/// The one process-wide runtime. Installed on first [`ReactiveRuntime::init`]
/// and never torn down (process lifetime — the matrix-rust-sdk precedent).
static RUNTIME: OnceLock<ReactiveRuntime> = OnceLock::new();

/// The process-wide reactive runtime. Construct via [`ReactiveRuntime::init`]
/// (once, on the UI thread) and reach later calls via [`ReactiveRuntime::get`].
pub struct ReactiveRuntime {
    // Kept alive for the process lifetime; the static is never dropped, so the
    // runtime never shuts down. Read only through `handle`.
    _runtime: Runtime,
    handle: Handle,
    root: Owner,
    waker: Mutex<FrameWaker>,
    /// Process-wide "did any tracked signal change since I last asked" flag —
    /// the reactive input to the mobile frame gate. Set by
    /// [`crate::tracked::TrackedScope`]'s dirty path (any tracked scope's
    /// invalidation trips it, coalesced by nature since it's a bool, not a
    /// counter); drained by [`take_signals_dirty`](Self::take_signals_dirty).
    signals_dirty: AtomicBool,
}

impl ReactiveRuntime {
    /// Process-wide initialization (idempotent).
    ///
    /// **Must be called on the UI thread** — the calling thread claims itself as
    /// the owner of the local task queue that [`pump_local`](Self::pump_local)
    /// drains. On the first call it builds the background tokio runtime, the
    /// custom executor, and the root [`Owner`]. A second call (e.g. a relaunched
    /// shell in the same process) does **not** rebuild anything: it re-marks the
    /// current thread as the UI thread, **replaces the waker** so the new shell
    /// re-owns wake-up, and returns the existing runtime. `any_spawner`'s
    /// `AlreadySet` on a repeated executor install is expected and benign.
    pub fn init(waker: FrameWaker) -> &'static ReactiveRuntime {
        // Claim the UI thread on every call: the fast path below must still
        // (re-)mark a relaunched shell's thread.
        executor::init_ui_thread();

        if let Some(existing) = RUNTIME.get() {
            existing.set_waker(waker);
            return existing;
        }

        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .thread_name("frust-reactive")
            .build()
            .expect("frust-reactive: failed to build the background tokio runtime");
        let handle = runtime.handle().clone();
        let root = Owner::new();

        let candidate = ReactiveRuntime {
            _runtime: runtime,
            handle: handle.clone(),
            root,
            waker: Mutex::new(waker),
            signals_dirty: AtomicBool::new(false),
        };

        match RUNTIME.set(candidate) {
            Ok(()) => {
                let rt = RUNTIME.get().expect("runtime was just installed");
                // Install the executor AFTER the runtime is reachable, so the
                // executor's `spawn_local` can fire the waker via `get()`.
                // `AlreadySet` (a prior shell already installed it) is benign.
                let _ = Executor::init_custom_executor(ForgeExecutor::new(handle));
                rt
            }
            Err(candidate) => {
                // Lost a concurrent init race (init is documented single-thread,
                // so this is defensive): adopt the winner, swap our waker in.
                let winner = RUNTIME.get().expect("runtime is set on the Err path");
                let waker = candidate
                    .waker
                    .into_inner()
                    .expect("candidate waker mutex is uncontended");
                winner.set_waker(waker);
                winner
            }
        }
    }

    /// The installed runtime, if [`init`](Self::init) has run.
    pub fn get() -> Option<&'static ReactiveRuntime> {
        RUNTIME.get()
    }

    /// Runs `f` under the root reactive [`Owner`]. Shells wrap their per-frame
    /// rebuild in this so signals created during a rebuild are owned by the root
    /// (and disposed only at process end).
    pub fn with_owner<R>(&self, f: impl FnOnce() -> R) -> R {
        self.root.with(f)
    }

    /// Drains the UI-thread local task queue until it stalls, inside a tokio
    /// runtime context so `tokio::time::sleep` in a local task registers with
    /// the background runtime's timer driver. Shells call this each loop turn.
    ///
    /// Must be called on the UI thread (the one [`init`](Self::init) ran on);
    /// off-thread it pumps a distinct, empty queue.
    pub fn pump_local(&self) {
        debug_assert!(
            executor::is_ui_thread(),
            "frust-reactive: pump_local called off the UI thread — this pumps \
             an unrelated empty queue and is a wiring bug"
        );
        let _guard = self.handle.enter();
        executor::run_until_stalled();
    }

    /// Replaces the frame waker (a relaunched shell re-owns wake-up).
    pub fn set_waker(&self, waker: FrameWaker) {
        *self
            .waker
            .lock()
            .expect("frust-reactive: waker mutex poisoned") = waker;
    }

    /// Fires the current frame waker. Used by the executor's `spawn_local` and
    /// by the signals dirty-bridge (task 03).
    pub fn wake(&self) {
        // Clone the Arc out before invoking so the lock is not held across the
        // callback (which may re-enter the runtime).
        let waker = self
            .waker
            .lock()
            .expect("frust-reactive: waker mutex poisoned")
            .clone();
        waker();
    }

    /// The background runtime handle. `pub(crate)` for the test that asserts a
    /// repeated executor install is benign.
    #[cfg(test)]
    pub(crate) fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Trips the process-wide signals-dirty flag. Called from
    /// [`crate::tracked::TrackedScope`]'s dirty-notification path — any
    /// tracked scope's invalidation trips this, not just the coalesced
    /// frame-waker edge, so a shell can ask "did *anything* change" cheaply
    /// once per frame.
    pub(crate) fn mark_signals_dirty(&self) {
        self.signals_dirty.store(true, Ordering::SeqCst);
    }

    /// Drains the signals-dirty flag: returns whether any tracked signal
    /// changed since the last `take_signals_dirty` call, and clears it.
    ///
    /// **Ordering contract for shells:** pump local tasks
    /// ([`pump_local`](Self::pump_local)) FIRST, then call this — a
    /// `spawn_local` continuation that writes a signal during the pump must
    /// be observed by the *same* frame's dirty check. Calling this before the
    /// pump can miss a write a just-drained local task makes.
    pub fn take_signals_dirty(&self) -> bool {
        self.signals_dirty.swap(false, Ordering::SeqCst)
    }

    /// Non-draining peek at the signals-dirty flag (does not clear it).
    /// Prefer [`take_signals_dirty`](Self::take_signals_dirty) for the actual
    /// once-per-frame gate check; this is for tests/diagnostics that want to
    /// observe the flag without consuming it.
    pub fn signals_dirty(&self) -> bool {
        self.signals_dirty.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracked::TrackedScope;
    use any_spawner::Executor;
    use reactive_graph::signal::RwSignal;
    use reactive_graph::traits::{Get, Set};

    fn noop_waker() -> FrameWaker {
        Arc::new(|| {})
    }

    /// Criterion 1: a write inside a tracked rebuild trips the process-wide
    /// signals-dirty flag; `take_signals_dirty` drains it (true once, then
    /// false with no intervening write).
    #[test]
    fn signals_dirty_set_by_tracked_write_and_drained_by_take() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let rt = ReactiveRuntime::init(noop_waker());
        // Drain any flag left dirty by a previous test sharing this
        // process-wide runtime.
        rt.take_signals_dirty();

        let scope = TrackedScope::new();
        let sig = rt.with_owner(|| RwSignal::new(0));
        scope.track(|| sig.get());
        assert!(!rt.signals_dirty(), "no write yet — flag must be clean");

        sig.set(1);
        assert!(
            rt.signals_dirty(),
            "a write to a tracked signal must trip the process-wide flag"
        );
        assert!(
            rt.take_signals_dirty(),
            "take must observe the dirty flag and drain it"
        );
        assert!(
            !rt.take_signals_dirty(),
            "a second take with no intervening write must return false"
        );
    }

    /// Criterion: writes from multiple distinct tracked scopes still coalesce
    /// onto the one process-wide flag (a bool, not a counter) — one drain
    /// clears every pending scope's contribution at once.
    #[test]
    fn signals_dirty_coalesces_across_scopes() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let rt = ReactiveRuntime::init(noop_waker());
        rt.take_signals_dirty();

        let scope_a = TrackedScope::new();
        let scope_b = TrackedScope::new();
        let sig_a = rt.with_owner(|| RwSignal::new(0));
        let sig_b = rt.with_owner(|| RwSignal::new(0));
        scope_a.track(|| sig_a.get());
        scope_b.track(|| sig_b.get());

        sig_a.set(1);
        sig_b.set(1);
        sig_a.set(2);

        assert!(
            rt.take_signals_dirty(),
            "multiple writes across multiple scopes must still trip the flag"
        );
        assert!(
            !rt.take_signals_dirty(),
            "drain must clear every pending contribution at once"
        );
    }

    /// The ordering contract: a spawned local task that writes a tracked
    /// signal *during* `pump_local` must be observed by a `take_signals_dirty`
    /// call that runs after the pump — the documented pump-first contract
    /// shells rely on.
    #[test]
    fn signals_dirty_pump_then_take_ordering() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let rt = ReactiveRuntime::init(noop_waker());
        rt.take_signals_dirty();

        let scope = TrackedScope::new();
        let sig = rt.with_owner(|| RwSignal::new(0));
        scope.track(|| sig.get());

        Executor::spawn_local(async move {
            sig.set(1);
        });
        assert!(
            !rt.signals_dirty(),
            "the local task has not run yet — spawning it must not itself \
             dirty the flag"
        );

        rt.pump_local();
        assert!(
            rt.take_signals_dirty(),
            "a signal write from a pumped local task must be observed by \
             take_signals_dirty called after the pump — the pump-first \
             ordering contract"
        );
    }
}
