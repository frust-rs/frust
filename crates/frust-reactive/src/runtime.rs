//! [`ReactiveRuntime`]: the process-wide reactive substrate.
//!
//! It owns a background tokio runtime, installs the custom [`any_spawner`]
//! executor, holds the root reactive [`Owner`], and carries a swappable
//! [`FrameWaker`] the executor fires to nudge the shell into pumping the
//! UI-thread local task queue.
//!
//! **wasm32 arm.** `wasm32-unknown-unknown` has no OS threads and no
//! mio-backed reactor, so the native background runtime's `rt-multi-thread`
//! and `net` tokio features fail to compile there (see `Cargo.toml`'s
//! target-gated `tokio` rows). [`ReactiveRuntime::init`] therefore builds a
//! bare current-thread runtime instead of the multi-thread pool, and
//! installs a wasm [`CustomExecutor`](any_spawner::CustomExecutor) (built
//! inline here, not the native [`ForgeExecutor`](crate::executor::ForgeExecutor))
//! whose `spawn` **and** `spawn_local` both route straight to
//! `wasm_bindgen_futures::spawn_local` — a browser has one JS thread, so
//! "spawn on a thread pool" and "spawn on the local microtask queue" are the
//! same operation there, and `Send` futures need no different handling than
//! `!Send` ones. This is the one call site that makes `wasm-bindgen-futures`
//! (`Cargo.toml`'s wasm-only rows) a real direct dependency rather than a
//! transitively-resolved one. Because both paths are driven by the browser's
//! own microtask queue, [`pump_local`](ReactiveRuntime::pump_local) has
//! nothing of this crate's own to drain on this arm, and `poll_local` is a
//! no-op. The current-thread runtime is never driven (nothing calls
//! `block_on`), so it exists only to give
//! [`handle`](ReactiveRuntime::handle)/[`spawn_blocking`] a real
//! `tokio::runtime::Handle` to type-check against.
//!
//! **What still doesn't work on wasm32.** `frust::spawn`/`frust::spawn_local`
//! now run for real (above). [`spawn_blocking`] and `use_task`'s background
//! half do not: `wasm32-unknown-unknown` has no OS threads, so there is no
//! blocking pool to hand CPU-bound work to. `spawn_blocking` fails loudly
//! (panics, naming the API and target) on this target rather than silently
//! swallowing the closure — see its own docs. Treat that as an open gap, not
//! a proven path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use any_spawner::Executor;
#[cfg(target_family = "wasm")]
use any_spawner::{CustomExecutor, PinnedFuture, PinnedLocalFuture};
use reactive_graph::owner::Owner;
use tokio::runtime::{Builder, Handle, Runtime};

use crate::executor;
#[cfg(not(target_family = "wasm"))]
use crate::executor::ForgeExecutor;

/// The wasm [`any_spawner`] executor (see the module docs for why it exists
/// instead of `any_spawner::Executor::init_wasm_bindgen()` and instead of the
/// native [`ForgeExecutor`](crate::executor::ForgeExecutor)). A unit struct —
/// nothing to hold, since `wasm_bindgen_futures::spawn_local` is a free
/// function reachable from anywhere.
#[cfg(target_family = "wasm")]
struct WasmExecutor;

#[cfg(target_family = "wasm")]
impl CustomExecutor for WasmExecutor {
    /// `Send` futures also go to `wasm_bindgen_futures::spawn_local`: a
    /// browser has one JS thread, so there is no separate thread-pool
    /// destination to route `Send` futures to the way the native
    /// [`ForgeExecutor`](crate::executor::ForgeExecutor) does. This is the
    /// call site that makes `wasm-bindgen-futures` a real direct dependency
    /// (see `Cargo.toml`'s wasm-only rows).
    fn spawn(&self, fut: PinnedFuture<()>) {
        wasm_bindgen_futures::spawn_local(fut);
    }

    /// `!Send` futures route the same way — `wasm_bindgen_futures::spawn_local`
    /// requires only `Future<Output = ()> + 'static`, which `PinnedFuture`
    /// (this crate's `spawn` path) already satisfies too.
    fn spawn_local(&self, fut: PinnedLocalFuture<()>) {
        wasm_bindgen_futures::spawn_local(fut);
    }

    /// Nothing to drain: every task spawned above is driven by the browser's
    /// own microtask queue, not a queue this crate owns (see the module
    /// docs).
    fn poll_local(&self) {}
}

/// A thread-safe, cheaply-cloneable "wake up and pump soon" callback. On
/// desktop this is a winit `EventLoopProxy` send; on mobile (continuous frame
/// loop) it is a no-op. It must be callable from any thread, since the executor
/// fires it and background tasks may drive signal writes.
pub type FrameWaker = Arc<dyn Fn() + Send + Sync>;

/// Builds the one background runtime [`ReactiveRuntime::init`] installs.
///
/// Native: the real multi-thread worker pool with the time + IO drivers
/// enabled — this arm is byte-identical to the pre-wasm code, `cfg`'d only by
/// which arm compiles for a given target.
#[cfg(not(target_family = "wasm"))]
fn build_background_runtime() -> Runtime {
    Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        // IO driver: `frust::spawn`'s background tasks (tonic/hyper socket
        // work) need a live reactor, not just the timer driver above.
        .enable_io()
        .thread_name("frust-reactive")
        .build()
        .expect("frust-reactive: failed to build the background tokio runtime")
}

/// Wasm: a bare current-thread runtime with no driver enabled (see the module
/// docs for why) — nothing ever calls `block_on` on it, so it exists purely
/// to give [`ReactiveRuntime::handle`] a real `tokio::runtime::Handle` to
/// type-check against, not a functioning executor. [`spawn_blocking`] never
/// reaches this handle on wasm32 (it panics first — see its own docs).
#[cfg(target_family = "wasm")]
fn build_background_runtime() -> Runtime {
    Builder::new_current_thread()
        .thread_name("frust-reactive")
        .build()
        .expect("frust-reactive: failed to build the wasm placeholder tokio runtime")
}

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

        let runtime = build_background_runtime();
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
                #[cfg(not(target_family = "wasm"))]
                {
                    let _ = Executor::init_custom_executor(ForgeExecutor::new(handle));
                }
                // No automatic wasm default exists (see module docs); install
                // the wasm `CustomExecutor` explicitly. `AlreadySet` on a
                // repeated install (e.g. a relaunched shell in the same
                // process) is handled the same benign way as the native arm.
                #[cfg(target_family = "wasm")]
                {
                    let _ = Executor::init_custom_executor(WasmExecutor);
                }
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
    ///
    /// **Wasm:** a harmless no-op drain of an always-empty queue. On this
    /// target `spawn_local` routes to `wasm_bindgen_futures::spawn_local`
    /// rather than this crate's own local pool (see the module docs), so
    /// there is nothing of this crate's own for a shell to pump — it is still
    /// safe (and, for cross-platform shell code, simplest) to call this every
    /// loop turn on wasm too.
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
    /// by the signals dirty-bridge.
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

    /// The background runtime handle. `pub(crate)` so the heavy-work idiom
    /// ([`crate::task`]) can `spawn`/`spawn_blocking` onto the background
    /// runtime, and so the test that asserts a repeated executor install is
    /// benign can rebuild the executor. Deliberately not part of the public
    /// API — app code routes through `spawn`/`spawn_local`/`spawn_blocking`
    /// rather than naming a raw `tokio::runtime::Handle`.
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

/// Runs a one-off, blocking CPU workload on the background runtime's blocking
/// thread pool, returning a [`JoinHandle`](tokio::task::JoinHandle) to `.await`
/// its result.
///
/// This is the CPU-bound entry point of the heavy-work routing convention:
///
/// | Call | Use for |
/// |---|---|
/// | `frust::spawn` | `Send` async IO-bound work |
/// | `frust::spawn_local` | `!Send` work that must stay on the UI thread |
/// | `frust::spawn_blocking` | one-off **CPU-bound** blocking work (JSON parse, decode, hashing) |
/// | `rayon` | data-parallel compute — an **app-level** choice, deliberately not bundled |
///
/// The pool already exists (the runtime is built `rt-multi-thread`), so this
/// is a thin facade over [`tokio::runtime::Handle::spawn_blocking`]. Compose it
/// inside a [`use_task`](crate::use_task) fetcher —
/// `use_task(|| async { spawn_blocking(parse).await })` — to get load/error
/// states and cancellation for free.
///
/// **Cancellation limitation:** dropping/aborting the returned handle stops the
/// result from being delivered, but a blocking closure *already running*
/// cannot be interrupted (there is no safe way to unwind arbitrary blocking
/// code) — the same limitation every runtime has.
///
/// **No working wasm equivalent yet — fails loudly.** `wasm32-unknown-unknown`
/// has no OS threads, so there is no blocking pool for `Handle::spawn_blocking`
/// to hand work to (see the module docs). Rather than returning an inert
/// `JoinHandle` that silently never resolves, this function panics up front
/// on that target (see Panics below), before ever reaching tokio. Treat this
/// as an open gap on wasm, not a proven path — the eventual fix is a
/// browser-thread/Web-Worker-backed `JoinHandle`, not yet built.
///
/// # Panics
///
/// - Panics if [`ReactiveRuntime::init`] has not run yet — the same
///   wiring-bug-not-runtime-condition contract as `spawn_local` off the UI
///   thread.
/// - **Always panics on `wasm32`** (see above), independent of `init` state.
pub fn spawn_blocking<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        let _ = f;
        panic!(
            "frust-reactive: spawn_blocking called on wasm32 — \
             wasm32-unknown-unknown has no OS threads, so there is no \
             blocking thread pool to hand this closure to. This is an \
             unimplemented gap on this target, not a wiring bug: route the \
             work through a different mechanism until a \
             browser-thread/Web-Worker-backed JoinHandle lands for \
             `frust::spawn_blocking`."
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let rt = ReactiveRuntime::get().expect(
            "frust-reactive: spawn_blocking called before ReactiveRuntime::init — \
             this is a wiring bug: initialize the reactive runtime (the shell does \
             this on startup) before spawning work",
        );
        rt.handle.spawn_blocking(f)
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

    /// The runtime's IO driver must actually be live on the `frust::spawn`
    /// path (`Executor::spawn` -> `ForgeExecutor::spawn` -> `Handle::spawn`,
    /// the same route real async IO takes), not just the timer driver the
    /// other tests in this module exercise: a spawned task must be able to
    /// bind a socket and complete an accept/connect round-trip.
    #[test]
    fn spawn_reaches_io_driver() {
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        ReactiveRuntime::init(noop_waker());

        let (tx, rx) = std::sync::mpsc::channel();
        Executor::spawn(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind must succeed with the IO driver enabled");
            let addr = listener.local_addr().expect("listener has a local addr");

            let accept = tokio::spawn(async move { listener.accept().await });
            tokio::net::TcpStream::connect(addr)
                .await
                .expect("connect must succeed with the IO driver enabled");
            accept
                .await
                .expect("accept task must not panic")
                .expect("accept must succeed");

            tx.send(()).expect("test receiver must still be alive");
        });

        rx.recv_timeout(std::time::Duration::from_secs(5)).expect(
            "a spawned task must complete an accept/connect round-trip \
             through the IO driver within 5s",
        );
    }
}
