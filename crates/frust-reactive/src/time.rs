//! Main-thread timers with Dart's names: [`sleep`] is `Future.delayed`, [`Timer`] is `Timer`,
//! except that dropping a [`Timer`] cancels it (Dart's keeps running; [`Timer::detach`]
//! restores that). [`spawn_local_scoped`] is `spawn_local` bound to the calling owner.
//!
//! Durations are raw wall time, not the animation clock. A deadline only makes the local
//! task ready; it runs when the shell next pumps the UI-thread queue:
//!
//! | Shell | A reached deadline runs |
//! |---|---|
//! | Desktop | on the next event-loop turn (the deadline wakes a parked loop) |
//! | Android, iOS | on the next display tick while resumed; nothing runs while backgrounded |
//! | Web | from the browser's task queue |
//!
//! So a timer does not fire in the background: an overdue one fires on the first tick after
//! the app resumes, unless it was flagged [`Timer::flush_on_background`], which runs it
//! synchronously inside the platform's foreground-exit callback.
//!
//! Arming and calling are safe anywhere on the UI thread, `Component::build` and event
//! handlers included: the underlying wait is only constructed on first poll, inside a pump.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::{Rc, Weak};
use std::time::Duration;

use any_spawner::Executor;
use futures::future::{AbortHandle, Abortable};
use reactive_graph::owner::{Owner, on_cleanup};

use crate::AppLifecycleListener;
use crate::executor::is_ui_thread;

/// Completes once `duration` of wall time has passed. Await it in `spawn_local`,
/// [`spawn_local_scoped`] or `spawn`; on wasm the future is `!Send`, so `spawn_local` only.
/// Calling this never panics: nothing happens until the first poll.
pub async fn sleep(duration: Duration) {
    imp::sleep(duration).await;
}

mod imp {
    use std::time::Duration;

    #[cfg(not(target_family = "wasm"))]
    pub(super) async fn sleep(duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    /// The wasm runtime has no time driver, so the browser's `setTimeout` keeps the time.
    #[cfg(target_family = "wasm")]
    pub(super) async fn sleep(duration: Duration) {
        let millis = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
        let promise = js_sys::Promise::new(&mut |resolve, _reject| match web_sys::window() {
            Some(window) => {
                let _ =
                    window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, millis);
            }
            None => log::warn!("frust-reactive: sleep has no `window`, so it never completes"),
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }
}

/// Spawns `fut` on the UI thread under the calling owner: every poll re-enters that owner,
/// and cleaning the owner up aborts the future.
///
/// Use this, not `spawn_local`, when the future reads context or must die with its
/// component; a plain `spawn_local` continuation runs with no owner because the shell
/// pumps are bare. Panics off the UI thread, like `spawn_local`.
pub fn spawn_local_scoped(fut: impl Future<Output = ()> + 'static) {
    reactive_graph::spawn_local_scoped_with_cancellation(fut);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Armed,
    Fired,
    Cancelled,
}

struct TimerInner {
    callback: RefCell<Option<Box<dyn FnOnce()>>>,
    abort: AbortHandle,
    owner: Option<Owner>,
    state: Cell<State>,
    detached: Cell<bool>,
    flush: Cell<bool>,
}

impl TimerInner {
    /// An owner cleanup can only reach the (`Send + Sync`) abort handle, so `state` alone
    /// cannot say whether this timer is still armed.
    fn is_active(&self) -> bool {
        self.state.get() == State::Armed && !self.abort.is_aborted()
    }

    fn cancel(&self) {
        if self.state.get() == State::Armed {
            self.state.set(State::Cancelled);
            self.abort.abort();
            drop(self.callback.take());
        }
    }

    fn fire_now(&self) {
        if self.is_active() {
            self.abort.abort();
            self.run_callback();
        }
    }

    /// The local task's end: the sleep completed.
    fn expire(&self) {
        if self.is_active() {
            self.run_callback();
        }
    }

    /// The local task's end: aborted, by [`cancel`](Self::cancel), `fire_now` or an owner
    /// cleanup. Only the last leaves `Armed` and the callback behind.
    fn abandon(&self) {
        if self.state.get() == State::Armed {
            self.state.set(State::Cancelled);
        }
        drop(self.callback.take());
    }

    /// Catches a panic so it cannot strand [`Owner::with`]'s thread-local (that function has
    /// no unwind guard) or stop the other timers of a background flush.
    fn run_callback(&self) {
        self.state.set(State::Fired);
        let Some(callback) = self.callback.take() else {
            return;
        };
        let callback = AssertUnwindSafe(callback);
        let outcome = match &self.owner {
            Some(owner) => owner.with(|| catch_unwind(callback)),
            None => catch_unwind(callback),
        };
        if let Err(payload) = outcome {
            log::error!(
                "frust-reactive: a Timer callback panicked ({}); the panic was caught",
                panic_message(&*payload)
            );
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string payload")
}

/// A one-shot UI-thread timer, cancelled when this handle is dropped or the owner it was
/// armed under is cleaned up. The callback runs at most once, under that owner, and a panic
/// in it is logged rather than propagated. See the [module docs](self) for when it fires.
#[must_use = "dropping a Timer cancels it; keep the handle, or call detach()"]
pub struct Timer {
    inner: Rc<TimerInner>,
}

impl Timer {
    /// Arms a timer that runs `callback` once `duration` of wall time has passed.
    ///
    /// # Panics
    ///
    /// Panics off the UI thread, or before [`ReactiveRuntime::init`](crate::ReactiveRuntime::init).
    pub fn new(duration: Duration, callback: impl FnOnce() + 'static) -> Self {
        assert!(
            is_ui_thread(),
            "frust-reactive: Timer::new was called off the UI thread (the one \
             `ReactiveRuntime::init` ran on). This is a wiring bug: a Timer's callback is \
             `!Send`, so hand the work back to the UI thread before arming one."
        );
        let (abort, registration) = AbortHandle::new_pair();
        let on_owner_cleanup = abort.clone();
        on_cleanup(move || on_owner_cleanup.abort());
        let inner = Rc::new(TimerInner {
            callback: RefCell::new(Some(Box::new(callback))),
            abort,
            owner: Owner::current(),
            state: Cell::new(State::Armed),
            detached: Cell::new(false),
            flush: Cell::new(false),
        });
        let task = Rc::clone(&inner);
        Executor::spawn_local(async move {
            match Abortable::new(sleep(duration), registration).await {
                Ok(()) => task.expire(),
                Err(_aborted) => task.abandon(),
            }
        });
        Self { inner }
    }

    /// Cancels the timer; the callback is dropped unrun. A no-op once it has fired.
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Whether the callback can still run: not yet fired, cancelled or owner-cleaned.
    pub fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    /// Runs the callback now, synchronously in the caller, and retires the deadline. A
    /// no-op unless the timer is active.
    pub fn fire_now(&self) {
        self.inner.fire_now();
    }

    /// Gives up the handle and leaves the timer running, Dart's behaviour. An owner cleanup
    /// still cancels it.
    pub fn detach(self) {
        self.inner.detached.set(true);
    }

    /// Also fires this timer when the app leaves the foreground ([`on_inactive`]; on desktop
    /// and web that includes losing window focus).
    ///
    /// The callback runs synchronously on the UI thread inside iOS's
    /// `applicationWillResignActive` or Android's `Activity.onPause`, before control returns
    /// to the OS, so work done **synchronously in the callback** completes before the
    /// platform can suspend the process. Work it hands to `spawn_blocking`, `spawn_local` or
    /// `spawn` is not covered: the process may be suspended first, and nothing pumps the
    /// local queue after `onPause`. Keep it short, because the OS waits on these main-thread
    /// callbacks: a small SQLite transaction or an atomic file write is in budget, a network
    /// call is not.
    ///
    /// [`on_inactive`]: AppLifecycleListener::on_inactive
    pub fn flush_on_background(self) -> Self {
        if self.inner.is_active() && !self.inner.flush.replace(true) {
            register_flush(&self.inner);
        }
        self
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if !self.inner.detached.get() {
            self.inner.cancel();
        }
    }
}

impl fmt::Debug for Timer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timer")
            .field("active", &self.is_active())
            .finish_non_exhaustive()
    }
}

// Allow-scoped for the reason `executor.rs` gives: on the mobile targets clippy's
// `missing_const_for_thread_local` flags even an already-const initializer.
#[allow(clippy::missing_const_for_thread_local)]
mod tls {
    use super::*;

    thread_local! {
        pub(super) static FLUSH_REGISTRY: RefCell<Vec<Weak<TimerInner>>> =
            const { RefCell::new(Vec::new()) };
        /// Created by the first flagged timer and never dropped: it lives as long as the
        /// UI thread.
        pub(super) static FLUSH_LISTENER: RefCell<Option<AppLifecycleListener>> =
            const { RefCell::new(None) };
    }
}
use tls::{FLUSH_LISTENER, FLUSH_REGISTRY};

fn register_flush(inner: &Rc<TimerInner>) {
    FLUSH_REGISTRY.with_borrow_mut(|registry| {
        registry.retain(|timer| timer.upgrade().is_some_and(|timer| timer.is_active()));
        registry.push(Rc::downgrade(inner));
    });
    FLUSH_LISTENER.with_borrow_mut(|listener| {
        listener
            .get_or_insert_with(|| AppLifecycleListener::new().on_inactive(drain_flush_registry));
    });
}

/// Empties the registry before firing, so a callback that arms a flagged timer leaves it
/// for the next foreground exit. [`TimerInner::run_callback`] isolates each panic.
fn drain_flush_registry() {
    let timers = FLUSH_REGISTRY.with_borrow_mut(std::mem::take);
    for timer in timers.iter().filter_map(Weak::upgrade) {
        timer.fire_now();
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use crate::{
        AppLifecycleState, ReactiveRuntime, provide_context, push_app_lifecycle, use_context,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, MutexGuard};
    use std::time::Instant;

    const SHORT: Duration = Duration::from_millis(10);
    const LONG: Duration = Duration::from_secs(10);
    /// Comfortably past [`SHORT`], for the assertions that something did not happen.
    const PAST_SHORT: Duration = Duration::from_millis(60);
    const DEADLINE: Duration = Duration::from_secs(5);

    #[derive(Clone)]
    struct Marker(u32);

    /// Serializes on the process-global waker and lifecycle state, and makes this test's
    /// thread the UI thread.
    fn setup() -> (MutexGuard<'static, ()>, &'static ReactiveRuntime) {
        let guard = crate::WAKER_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (guard, ReactiveRuntime::init(Arc::new(|| {})))
    }

    fn pump_until(rt: &ReactiveRuntime, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() && start.elapsed() < DEADLINE {
            rt.pump_local();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn pump_for(rt: &ReactiveRuntime, duration: Duration) {
        let start = Instant::now();
        pump_until(rt, || start.elapsed() >= duration);
    }

    fn counter() -> (Rc<Cell<u32>>, impl FnOnce() + 'static) {
        let count = Rc::new(Cell::new(0));
        let bump = Rc::clone(&count);
        (count, move || bump.set(bump.get() + 1))
    }

    #[test]
    fn a_timer_fires_once_after_its_deadline() {
        let (_guard, rt) = setup();
        let (count, bump) = counter();
        let timer = Timer::new(SHORT, bump);
        assert!(timer.is_active());
        assert_eq!(count.get(), 0, "arming must not run the callback");

        pump_until(rt, || count.get() > 0);
        assert_eq!(count.get(), 1);
        assert!(!timer.is_active());

        pump_for(rt, PAST_SHORT);
        assert_eq!(count.get(), 1, "the callback runs exactly once");
    }

    #[test]
    fn cancel_stops_the_timer_at_once() {
        let (_guard, rt) = setup();
        let (count, bump) = counter();
        let timer = Timer::new(SHORT, bump);
        timer.cancel();
        assert!(!timer.is_active());
        pump_for(rt, PAST_SHORT);
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn dropping_the_handle_cancels_and_detach_does_not() {
        let (_guard, rt) = setup();
        let (dropped, bump_dropped) = counter();
        drop(Timer::new(SHORT, bump_dropped));
        let (detached, bump_detached) = counter();
        Timer::new(SHORT, bump_detached).detach();

        pump_until(rt, || detached.get() > 0);
        pump_for(rt, PAST_SHORT);
        assert_eq!(dropped.get(), 0, "a dropped handle cancels");
        assert_eq!(detached.get(), 1, "a detached handle keeps running");
    }

    #[test]
    fn owner_cleanup_cancels_held_and_detached_timers() {
        let (_guard, rt) = setup();
        let (held, bump_held) = counter();
        let (detached, bump_detached) = counter();
        let (owner, timer) = rt.with_owner(|| {
            let owner = Owner::new();
            let timer = owner.with(|| {
                Timer::new(SHORT, bump_detached).detach();
                Timer::new(SHORT, bump_held)
            });
            (owner, timer)
        });
        assert!(timer.is_active());

        owner.cleanup();
        assert!(!timer.is_active(), "inactive before the next poll");
        pump_for(rt, PAST_SHORT);
        assert_eq!(held.get(), 0);
        assert_eq!(
            detached.get(),
            0,
            "detach escapes the handle, not the owner"
        );
        timer.fire_now();
        assert_eq!(held.get(), 0, "fire_now after an owner cleanup is a no-op");
    }

    #[test]
    fn the_callback_runs_under_the_arming_owner() {
        let (_guard, rt) = setup();
        let seen = Rc::new(Cell::new(None));
        let write = Rc::clone(&seen);
        let owner = rt.with_owner(Owner::new);
        let _timer = owner.with(|| {
            provide_context(Marker(7));
            Timer::new(SHORT, move || {
                write.set(use_context::<Marker>().map(|marker| marker.0));
            })
        });
        pump_until(rt, || seen.get().is_some());
        assert_eq!(seen.get(), Some(7));
    }

    #[test]
    fn spawn_local_scoped_keeps_the_owner_across_an_await_and_spawn_local_loses_it() {
        let (_guard, rt) = setup();
        let scoped = Rc::new(Cell::new(None));
        let plain = Rc::new(Cell::new(None));
        let (scoped_write, plain_write) = (Rc::clone(&scoped), Rc::clone(&plain));
        let owner = rt.with_owner(Owner::new);
        owner.with(|| {
            provide_context(Marker(7));
            spawn_local_scoped(async move {
                sleep(Duration::from_millis(5)).await;
                scoped_write.set(Some(use_context::<Marker>().is_some()));
            });
            Executor::spawn_local(async move {
                sleep(Duration::from_millis(5)).await;
                plain_write.set(Some(use_context::<Marker>().is_some()));
            });
        });

        pump_until(rt, || scoped.get().is_some() && plain.get().is_some());
        assert_eq!(scoped.get(), Some(true));
        assert_eq!(plain.get(), Some(false));
    }

    #[test]
    fn owner_cleanup_aborts_spawn_local_scoped() {
        let (_guard, rt) = setup();
        let (count, bump) = counter();
        let owner = rt.with_owner(Owner::new);
        owner.with(|| {
            spawn_local_scoped(async move {
                sleep(SHORT).await;
                bump();
            });
        });
        owner.cleanup();
        pump_for(rt, PAST_SHORT);
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn fire_now_runs_in_the_caller_and_retires_the_deadline() {
        let (_guard, rt) = setup();
        let (count, bump) = counter();
        let timer = Timer::new(SHORT, bump);
        timer.fire_now();
        assert_eq!(count.get(), 1, "fire_now is synchronous");
        assert!(!timer.is_active());
        pump_for(rt, PAST_SHORT);
        timer.fire_now();
        assert_eq!(
            count.get(),
            1,
            "the deadline and a second fire_now are no-ops"
        );
    }

    #[test]
    fn fire_now_on_a_cancelled_timer_is_a_no_op() {
        let (_guard, _rt) = setup();
        let (count, bump) = counter();
        let timer = Timer::new(LONG, bump);
        timer.cancel();
        timer.fire_now();
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn a_panicking_callback_restores_the_owner() {
        let (_guard, rt) = setup();
        let owner = rt.with_owner(Owner::new);
        let timer = owner.with(|| Timer::new(LONG, || panic!("timer callback failed")));
        assert!(Owner::current().is_none());
        timer.fire_now();
        assert!(
            Owner::current().is_none(),
            "the panic must not strand the owner"
        );
        assert!(!timer.is_active());
    }

    #[test]
    fn flush_on_background_fires_flagged_timers_inside_the_push() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);

        let flushed = Arc::new(AtomicUsize::new(0));
        let write = Arc::clone(&flushed);
        let flagged = Timer::new(LONG, move || {
            write.fetch_add(1, Ordering::SeqCst);
        })
        .flush_on_background();
        let (plain_count, bump) = counter();
        let plain = Timer::new(LONG, bump);
        let (dropped_count, bump_dropped) = counter();
        drop(Timer::new(LONG, bump_dropped).flush_on_background());
        let (detached_count, bump_detached) = counter();
        Timer::new(LONG, bump_detached)
            .flush_on_background()
            .detach();

        // Registered after the flush listener, so it runs later in the same push: what it
        // reads is what a platform callback has already got when the push is still running.
        let seen_during_push = Arc::new(AtomicUsize::new(usize::MAX));
        let (seen, source) = (Arc::clone(&seen_during_push), Arc::clone(&flushed));
        let _observer = AppLifecycleListener::new().on_inactive(move || {
            seen.store(source.load(Ordering::SeqCst), Ordering::SeqCst);
        });

        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(
            seen_during_push.load(Ordering::SeqCst),
            1,
            "the callback completes before the foreground-exit push returns"
        );
        assert_eq!(flushed.load(Ordering::SeqCst), 1);
        assert!(!flagged.is_active());
        assert_eq!(detached_count.get(), 1, "a detached flagged timer flushes");
        assert_eq!(dropped_count.get(), 0, "a dropped flagged timer does not");
        assert_eq!(plain_count.get(), 0);
        assert!(plain.is_active(), "an unflagged timer is left alone");

        push_app_lifecycle(AppLifecycleState::Inactive);
        push_app_lifecycle(AppLifecycleState::Resumed);
        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(flushed.load(Ordering::SeqCst), 1, "a flush fires only once");
    }

    #[test]
    fn a_panicking_flush_callback_does_not_stop_the_others() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);
        let _panics = Timer::new(LONG, || panic!("flush failed")).flush_on_background();
        let (count, bump) = counter();
        let _next = Timer::new(LONG, bump).flush_on_background();
        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn registering_a_flagged_timer_prunes_retired_ones() {
        let (_guard, _rt) = setup();
        for _ in 0..8 {
            drop(Timer::new(LONG, || {}).flush_on_background());
        }
        let _live = Timer::new(LONG, || {}).flush_on_background();
        assert_eq!(FLUSH_REGISTRY.with_borrow(Vec::len), 1);
    }

    #[test]
    fn sleep_completes_after_pumping() {
        let (_guard, rt) = setup();
        let (count, bump) = counter();
        Executor::spawn_local(async move {
            sleep(SHORT).await;
            bump();
        });
        pump_until(rt, || count.get() > 0);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn calling_sleep_outside_a_runtime_context_does_not_panic() {
        let (_guard, rt) = setup();
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "this test must run with no entered runtime handle"
        );
        let fut = sleep(SHORT);
        let (count, bump) = counter();
        Executor::spawn_local(async move {
            fut.await;
            bump();
        });
        pump_until(rt, || count.get() > 0);
        assert_eq!(count.get(), 1);
    }

    #[test]
    #[should_panic(expected = "off the UI thread")]
    fn arming_off_the_ui_thread_panics() {
        let (_guard, _rt) = setup();
        std::thread::spawn(|| drop(Timer::new(SHORT, || {})))
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
