//! Component-scoped polling: run a callback every `period` until unmount.
//!
//! [`use_interval`] is the Frust transliteration of
//! `clean-signals-leptos`'s `use_interval` hook: a `spawn_local` loop that
//! sleeps via the framework-portable [`clean_signals::time::sleep`] and
//! checks an alive-flag flipped in [`on_cleanup`](frust::on_cleanup), so
//! it stops the moment the owning component unmounts (no leaked timers, no
//! callbacks firing into a torn-down scope).
//!
//! # Why this differs from `clean-signals-leptos`'s `use_interval`
//!
//! The leptos crate's `use_interval` is `cfg`-split: a real implementation
//! under `#[cfg(target_arch = "wasm32")]` (the only place leptos's
//! `spawn_local` — a single-threaded, browser-owned executor — runs) and a
//! no-op under every other target (SSR has no client-side scope to poll).
//! Frust supports both native and `wasm32` targets — `frust::spawn_local`
//! routes to the native UI-thread task queue (`ReactiveRuntime::pump_local`)
//! on desktop/Android/iOS, and to the browser's microtask executor
//! (via `wasm_bindgen_futures::spawn_local`) on `wasm32`. This module is
//! **unconditional** (no `cfg` split), so the same code works for both platforms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use frust::{on_cleanup, spawn_local};

/// Runs `f` every `period` for as long as the current component is mounted.
///
/// Spawns a local task ([`frust::spawn_local`]) that sleeps `period` via
/// [`clean_signals::time::sleep`], checks an alive-flag (flipped `false` by
/// an [`on_cleanup`](frust::on_cleanup) handler registered on the current
/// owner), then invokes `f` — looping until the flag clears. The first call
/// fires after the first `period`, not immediately; trigger an initial call
/// separately if you need one.
///
/// `f` must be `Send + Sync` for the same reason [`on_cleanup`]'s callback
/// and [`use_controller`](crate::use_controller)'s disposal closure are:
/// Frust's reactive owner may run cleanups (and, here, the interval's own
/// callback) off the construction thread.
///
/// # Ambient-Owner contract
///
/// Must be called where a reactive [`Owner`](reactive_graph::owner::Owner) is
/// ambient — i.e. from [`Component::init`](frust::Component::init).
/// Outside an owner, `on_cleanup` silently no-ops: the alive-flag never
/// flips, and the interval **never stops**, ticking for the rest of the
/// process.
///
/// # Pumping contract
///
/// The interval only ticks while something pumps the UI-thread local task
/// queue: the desktop shell pumps on wake (a signal write, or this task's own
/// `sleep` completing, nudges the frame waker), the Android/iOS shells pump
/// once per frame — but a backgrounded iOS app pauses its `CADisplayLink`, so
/// nothing pumps and the interval stalls until `frust_resume` fires the
/// next pump (see `docs/CODE_STANDARDS.md` in the Frust repo).
///
/// # Example
///
/// ```rust,ignore
/// // Refetch every 8 seconds while the screen is mounted.
/// use_interval(Duration::from_secs(8), move || {
///     let handle = Arc::clone(&controller);
///     frust::spawn_local(async move { handle.reload().await });
/// });
/// ```
pub fn use_interval(period: Duration, f: impl Fn() + Send + Sync + 'static) {
    // `Arc<AtomicBool>` (not `Rc<Cell>`): `on_cleanup`'s callback must be
    // `Send + Sync`, so the shared alive-flag has to be too.
    let alive = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&alive);
    on_cleanup(move || flag.store(false, Ordering::SeqCst));

    spawn_local(run_interval(
        period,
        move || alive.load(Ordering::SeqCst),
        f,
    ));
}

/// The polling loop, factored out of [`use_interval`] so its stop-on-cleanup
/// behavior is testable without a component/owner at all: it sleeps `period`,
/// breaks when `alive()` returns `false`, otherwise calls `f`, and repeats.
async fn run_interval<A, F>(period: Duration, alive: A, f: F)
where
    A: Fn() -> bool,
    F: Fn(),
{
    loop {
        clean_signals::time::sleep(period).await;
        if !alive() {
            break;
        }
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[tokio::test]
    async fn run_interval_stops_when_alive_flag_clears() {
        let alive = Rc::new(Cell::new(true));
        let count = Rc::new(Cell::new(0u32));

        // The callback flips the alive-flag off after its third invocation;
        // the loop must observe that and stop rather than fire a fourth time.
        let alive_for_f = Rc::clone(&alive);
        let count_for_f = Rc::clone(&count);
        run_interval(
            Duration::from_millis(1),
            {
                let alive = Rc::clone(&alive);
                move || alive.get()
            },
            move || {
                count_for_f.set(count_for_f.get() + 1);
                if count_for_f.get() >= 3 {
                    alive_for_f.set(false);
                }
            },
        )
        .await;

        assert_eq!(count.get(), 3, "loop stops once the alive-flag is cleared");
    }
}
