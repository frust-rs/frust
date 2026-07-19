//! Reactive-programming substrate for ForgeKit (phase 5.5).
//!
//! This crate owns the process-wide [`ReactiveRuntime`]: a background tokio
//! runtime, a custom [`any_spawner`] executor that routes `spawn` to that
//! runtime and `spawn_local` to a UI-thread local task queue, a swappable
//! [`FrameWaker`], and the root reactive [`Owner`]. It also re-exports the
//! `reactive_graph` types the later programming-model tasks build on, and
//! owns the process-wide deep-link source shells write platform deep links
//! into (see [`push_deep_link`]/[`deep_links`]) and the process-wide Android
//! back-press source next to it (see [`push_back_press`]/[`back_presses`] and
//! the `handles_back` flag). A process-wide
//! signals-dirty flag (`ReactiveRuntime::take_signals_dirty`), tripped by
//! [`TrackedScope`]'s dirty path, lets a shell ask synchronously and cheaply
//! once per frame "did any tracked signal change since I last asked" — the
//! reactive input to the mobile frame gate.
//!
//! It is a leaf substrate: no `winit`, no `vello`/`wgpu`, no `forgekit-core`
//! dependency. Shells own the wake-up wiring and call [`ReactiveRuntime::init`]
//! (once, on the UI thread) and [`ReactiveRuntime::pump_local`] each frame.

mod back;
mod deep_link;
mod executor;
mod runtime;
mod tracked;

pub use back::{
    BackPresses, CanPopRegistration, back_presses, clear_can_pop_provider, handles_back,
    push_back_press, set_can_pop_provider, set_handles_back,
};
pub use deep_link::{DeepLink, DeepLinks, deep_links, push_deep_link};
pub use runtime::{FrameWaker, ReactiveRuntime};
pub use tracked::TrackedScope;

pub use reactive_graph::owner::{Owner, on_cleanup, provide_context, use_context};
pub use reactive_graph::signal::RwSignal;

/// Serializes every test that installs a recording [`FrameWaker`] and asserts on
/// wake counts. The frame waker is process-global and swappable, so two such
/// tests running on the parallel test-runner's separate threads would clobber
/// each other's waker mid-assertion. Both the runtime end-to-end test and the
/// tracked-scope test acquire this before touching the waker.
#[cfg(test)]
pub(crate) static WAKER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use reactive_graph::traits::{Get, Set};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use any_spawner::{Executor, ExecutorError};

    #[test]
    fn owner_and_signal_round_trip() {
        let owner = Owner::new();
        owner.set();

        let signal = RwSignal::new(1);
        assert_eq!(signal.get(), 1);

        signal.set(2);
        assert_eq!(signal.get(), 2);
    }

    /// A recording waker: an `Arc<AtomicUsize>` bumped once per `wake()`.
    fn recording_waker() -> (FrameWaker, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let seen = counter.clone();
        let waker: FrameWaker = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (waker, seen)
    }

    /// The runtime, executor, and UI-thread markers are all process-global, and
    /// the UI-thread markers are thread-local — so the whole scenario runs in one
    /// `#[test]` on one thread to stay independent of the parallel test runner.
    /// Each block maps to an acceptance criterion.
    #[test]
    fn reactive_runtime_end_to_end() {
        // Serialize with the tracked-scope test: both swap the global waker.
        let _guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (waker1, waker1_count) = recording_waker();
        let rt = ReactiveRuntime::init(waker1);

        // `with_owner` runs its closure under the root owner, so a signal
        // created there is valid and readable afterward.
        let scoped = rt.with_owner(|| {
            let s = RwSignal::new(41);
            s.set(42);
            s
        });
        assert_eq!(scoped.get(), 42);

        // Criterion 2: `spawn` runs on the background runtime.
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let ui_thread = std::thread::current().id();
            Executor::spawn(async move {
                let _ = tx.send(std::thread::current().id());
            });
            let ran_on = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("spawned background task did not run");
            assert_ne!(
                ran_on, ui_thread,
                "Executor::spawn should run on a background worker thread"
            );
        }

        // Criterion 1 (immediate): a `spawn_local` future runs to completion via
        // `pump_local`. Criterion 3: `spawn_local` fires the waker.
        {
            let ran = Arc::new(AtomicUsize::new(0));
            let flag = ran.clone();
            let before = waker1_count.load(Ordering::SeqCst);
            Executor::spawn_local(async move {
                flag.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(
                waker1_count.load(Ordering::SeqCst),
                before + 1,
                "spawn_local should fire the frame waker exactly once"
            );
            assert_eq!(
                ran.load(Ordering::SeqCst),
                0,
                "task should not run before pump"
            );
            rt.pump_local();
            assert_eq!(
                ran.load(Ordering::SeqCst),
                1,
                "spawn_local future should complete after pump_local"
            );
        }

        // Criterion 1 (timer): a local future awaiting tokio time completes after
        // pumping past the deadline.
        {
            let done = Arc::new(AtomicUsize::new(0));
            let flag = done.clone();
            Executor::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                flag.fetch_add(1, Ordering::SeqCst);
            });
            let start = Instant::now();
            while done.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(2) {
                rt.pump_local();
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(
                done.load(Ordering::SeqCst),
                1,
                "local task awaiting tokio::time::sleep should complete after pumping"
            );
        }

        // Criterion 4: a second `init` is benign, returns the same runtime, and
        // swaps the waker (the new one now fires on spawn_local).
        {
            let (waker2, waker2_count) = recording_waker();
            let rt2 = ReactiveRuntime::init(waker2);
            assert!(
                std::ptr::eq(rt, rt2),
                "second init must return the existing runtime"
            );
            Executor::spawn_local(async {});
            assert_eq!(
                waker2_count.load(Ordering::SeqCst),
                1,
                "second init must swap in the new waker"
            );
            rt.pump_local();

            // Criterion 4 (executor double-init): a repeated executor install
            // returns AlreadySet rather than panicking.
            let repeated =
                Executor::init_custom_executor(crate::executor::ForgeExecutor::new(rt.handle()));
            assert!(
                matches!(repeated, Err(ExecutorError::AlreadySet)),
                "repeated executor install should be a benign AlreadySet"
            );
        }

        // Criterion 5: `spawn_local` from a non-UI OS thread panics with the
        // wiring-bug message (the executor is already installed above, so this is
        // our panic, not any_spawner's uninitialized-executor panic).
        {
            let joined = std::thread::spawn(|| {
                Executor::spawn_local(async {});
            })
            .join();
            let payload = joined.expect_err("spawn_local off the UI thread must panic");
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            assert!(
                msg.contains("UI thread") && msg.contains("wiring bug"),
                "panic should name the UI-thread wiring bug, got: {msg}"
            );
        }
    }
}
