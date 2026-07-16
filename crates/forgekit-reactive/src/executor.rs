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

use any_spawner::{CustomExecutor, PinnedFuture, PinnedLocalFuture};
use futures::executor::{LocalPool, LocalSpawner};
use futures::task::LocalSpawnExt;
use tokio::runtime::Handle;

thread_local! {
    /// The UI thread's local task queue for `!Send` futures. Empty on every
    /// other thread (each thread has its own, so pumping off the UI thread is a
    /// harmless no-op rather than a panic).
    static LOCAL_POOL: RefCell<LocalPool> = RefCell::new(LocalPool::new());
    /// Spawner into `LOCAL_POOL`, cached to avoid re-deriving it per spawn.
    static LOCAL_SPAWNER: LocalSpawner =
        LOCAL_POOL.with(|pool| pool.borrow().spawner());
    /// Set on the thread that called [`init_ui_thread`]; gates `spawn_local`.
    static IS_UI_THREAD: Cell<bool> = const { Cell::new(false) };
}

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

/// The ForgeKit `any_spawner` executor. Stored inside `any_spawner`'s global
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
                "forgekit-reactive: Executor::spawn_local was called off the UI \
                 thread. `!Send` local futures can only be spawned on the UI \
                 thread (the one `ReactiveRuntime::init` ran on). This is a \
                 wiring bug: route the work through `Executor::spawn` instead, or \
                 hand it back to the UI thread before spawning it locally."
            );
        }

        LOCAL_SPAWNER.with(|spawner| {
            spawner
                .spawn_local(fut)
                .expect("forgekit-reactive: UI-thread local task queue rejected a future");
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
