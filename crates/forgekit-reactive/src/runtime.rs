//! [`ReactiveRuntime`]: the process-wide reactive substrate.
//!
//! It owns a background tokio runtime, installs the custom [`any_spawner`]
//! executor, holds the root reactive [`Owner`], and carries a swappable
//! [`FrameWaker`] the executor fires to nudge the shell into pumping the
//! UI-thread local task queue.

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
            .thread_name("forgekit-reactive")
            .build()
            .expect("forgekit-reactive: failed to build the background tokio runtime");
        let handle = runtime.handle().clone();
        let root = Owner::new();

        let candidate = ReactiveRuntime {
            _runtime: runtime,
            handle: handle.clone(),
            root,
            waker: Mutex::new(waker),
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
            "forgekit-reactive: pump_local called off the UI thread — this pumps \
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
            .expect("forgekit-reactive: waker mutex poisoned") = waker;
    }

    /// Fires the current frame waker. Used by the executor's `spawn_local` and
    /// by the signals dirty-bridge (task 03).
    pub fn wake(&self) {
        // Clone the Arc out before invoking so the lock is not held across the
        // callback (which may re-enter the runtime).
        let waker = self
            .waker
            .lock()
            .expect("forgekit-reactive: waker mutex poisoned")
            .clone();
        waker();
    }

    /// The background runtime handle. `pub(crate)` for the test that asserts a
    /// repeated executor install is benign.
    #[cfg(test)]
    pub(crate) fn handle(&self) -> Handle {
        self.handle.clone()
    }
}
