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
//! | Android, iOS | on the next display tick while resumed; no regular ticks while backgrounded |
//! | Web | from the browser's task queue |
//!
//! So there are no regular ticks in the background: a due timer normally fires on the first
//! tick after the app resumes, but may run earlier in one of the few background pumps a shell
//! makes (iOS `didEnterBackground` and memory warnings, Android trim-memory callbacks); do not
//! rely on either. A timer flagged [`Timer::flush_on_background`] instead runs synchronously
//! at the first of the app going Resumed to Inactive or Inactive to Hidden. One armed while
//! already Inactive runs at the Hidden edge (Android `onStop`, iOS `didEnterBackground`),
//! still synchronously on the UI thread.
//!
//! Flagged timers also fire on focus loss with no foreground exit: losing window focus on
//! desktop and web; the notification shade, a system dialog or the unfocused half of
//! split-screen on Android; Control Center or a call on iOS. The timer is one-shot and is
//! consumed, so do not flag a timer whose delay must be honoured while the app stays visible.
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
///
/// With a virtual clock installed on the polling thread (`test-support` feature or this
/// crate's own tests, see [`test_support`]) the wait is for virtual time instead.
pub async fn sleep(duration: Duration) {
    #[cfg(any(test, feature = "test-support"))]
    if test_support::is_virtual() {
        return virtual_clock::VirtualSleep::new(duration).await;
    }
    imp::sleep(duration).await;
}

#[cfg(any(test, feature = "test-support"))]
mod virtual_clock {
    use std::cell::RefCell;
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    pub(super) struct Entry {
        waker: Option<Waker>,
        alive: Arc<AtomicBool>,
    }

    #[derive(Default)]
    pub(super) struct VirtualClock {
        pub(super) now: Duration,
        next_seq: u64,
        heap: BinaryHeap<Reverse<(Duration, u64)>>,
        pub(super) entries: HashMap<u64, Entry>,
    }

    pub(super) enum Source {
        Real,
        Virtual(VirtualClock),
    }

    // Clippy's `missing_const_for_thread_local` flags even an already-const initializer on
    // the mobile targets, so this `tls` module is allow-scoped (as the flush-registry
    // `tls` module further down is).
    #[allow(clippy::missing_const_for_thread_local)]
    mod tls {
        use super::*;
        thread_local! {
            pub(in crate::time) static SOURCE: RefCell<Source> = const { RefCell::new(Source::Real) };
        }
    }
    pub(super) use tls::SOURCE;

    impl VirtualClock {
        /// Takes the next group of live sleeps due at or before `target`: sets `now` to their
        /// shared deadline and removes them in registration order. Stale heap tuples (sleeps
        /// dropped or already removed) are discarded without moving `now`. Returns the removed
        /// entries so the caller can wake and drop them outside the borrow; `None` when
        /// nothing live is due.
        pub(super) fn take_next_due(&mut self, target: Duration) -> Option<Vec<Entry>> {
            let mut taken = Vec::new();
            let mut group: Option<Duration> = None;
            while let Some(Reverse((deadline, seq))) = self.heap.peek().copied() {
                if deadline > target || group.is_some_and(|g| deadline != g) {
                    break;
                }
                self.heap.pop();
                match self.entries.remove(&seq) {
                    Some(entry) if entry.alive.load(Ordering::Relaxed) => {
                        group = Some(deadline);
                        self.now = deadline;
                        taken.push(entry);
                    }
                    // Stale: handed back so it drops outside the borrow, never woken.
                    Some(entry) => taken.push(entry),
                    None => {}
                }
            }
            (!taken.is_empty()).then_some(taken)
        }

        /// The sleeps registered and not dropped.
        pub(super) fn live_count(&self) -> usize {
            self.entries
                .values()
                .filter(|entry| entry.alive.load(Ordering::Relaxed))
                .count()
        }
    }

    impl Entry {
        pub(super) fn wake_if_live(self) {
            if self.alive.load(Ordering::Relaxed)
                && let Some(waker) = self.waker
            {
                waker.wake();
            }
        }
    }

    enum Slot {
        Unpolled,
        /// The deadline is not representable: never due, no heap entry.
        Never,
        /// Due at once (zero duration): no heap entry.
        Immediate,
        Registered(Duration, u64),
    }

    /// A sleep on the thread's virtual clock; registered on first poll.
    pub(super) struct VirtualSleep {
        duration: Duration,
        slot: Slot,
        alive: Arc<AtomicBool>,
    }

    impl VirtualSleep {
        pub(super) fn new(duration: Duration) -> Self {
            Self {
                duration,
                slot: Slot::Unpolled,
                alive: Arc::new(AtomicBool::new(true)),
            }
        }
    }

    impl Future for VirtualSleep {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            SOURCE.with_borrow_mut(|source| {
                let Source::Virtual(clock) = source else {
                    // The clock was uninstalled under a pending sleep: nothing can wake it.
                    return Poll::Pending;
                };
                if matches!(self.slot, Slot::Unpolled) {
                    self.slot = match clock.now.checked_add(self.duration) {
                        None => Slot::Never,
                        Some(deadline) if deadline <= clock.now => Slot::Immediate,
                        Some(deadline) => {
                            let seq = clock.next_seq;
                            clock.next_seq += 1;
                            clock.heap.push(Reverse((deadline, seq)));
                            clock.entries.insert(
                                seq,
                                Entry {
                                    waker: None,
                                    alive: Arc::clone(&self.alive),
                                },
                            );
                            Slot::Registered(deadline, seq)
                        }
                    };
                }
                match self.slot {
                    Slot::Unpolled | Slot::Never => Poll::Pending,
                    Slot::Immediate => Poll::Ready(()),
                    Slot::Registered(deadline, seq) => {
                        if clock.now >= deadline {
                            return Poll::Ready(());
                        }
                        if let Some(entry) = clock.entries.get_mut(&seq) {
                            entry.waker = Some(cx.waker().clone());
                        }
                        Poll::Pending
                    }
                }
            })
        }
    }

    impl Drop for VirtualSleep {
        fn drop(&mut self) {
            self.alive.store(false, Ordering::Relaxed);
            let Slot::Registered(_, seq) = self.slot else {
                return;
            };
            // A sleep moved to another thread sees that thread's source, where `seq` may name
            // a foreign entry: only remove ours. The removed entry (and its waker) drops
            // outside the borrow. If the borrow is unavailable the alive flag is the fallback.
            let removed = SOURCE
                .try_with(|source| {
                    let mut source = source.try_borrow_mut().ok()?;
                    let Source::Virtual(clock) = &mut *source else {
                        return None;
                    };
                    if clock
                        .entries
                        .get(&seq)
                        .is_some_and(|entry| Arc::ptr_eq(&entry.alive, &self.alive))
                    {
                        clock.entries.remove(&seq)
                    } else {
                        None
                    }
                })
                .ok()
                .flatten();
            drop(removed);
        }
    }
}

/// A virtual clock for tests (`test-support` feature, or this crate's own tests).
///
/// [`install_virtual_clock`] makes every [`sleep`] and [`Timer`] polled on the installing
/// thread wait for virtual time, which the test moves with [`advance`]; real shells never
/// install one and are unchanged. Time is a [`Duration`](std::time::Duration) offset from
/// the install, raw and unscaled (it ignores the animation time scale).
///
/// A sleep takes its deadline (`now + duration`) at its first poll, so pump once after
/// arming a timer and before advancing. `advance` steps through each intervening deadline in
/// order: it moves `now` to the deadline, wakes the sleeps due there, and drains the UI-thread
/// queue before moving on, so a continuation runs with `now` at that deadline and a sleep it
/// arms is measured from it. `advance(a + b)` therefore matches `advance(a); advance(b)`.
///
/// Only futures polled on the installing thread are reached. Any future polled elsewhere
/// stays on wall time, explicitly including `frust::spawn` futures (tokio workers) and
/// `use_task` fetch futures.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::virtual_clock::{SOURCE, Source, VirtualClock};
    use crate::ReactiveRuntime;
    use std::time::Duration;

    /// Uninstalls the virtual clock on drop, restoring wall time and discarding pending
    /// entries.
    #[must_use = "the virtual clock is uninstalled when the guard drops"]
    pub struct VirtualClockGuard {
        _not_send: std::marker::PhantomData<*const ()>,
    }

    /// Installs a virtual clock on this thread, starting at zero.
    ///
    /// # Panics
    ///
    /// Panics if one is already installed on this thread.
    pub fn install_virtual_clock() -> VirtualClockGuard {
        SOURCE.with_borrow_mut(|source| {
            assert!(
                matches!(source, Source::Real),
                "frust-reactive: a virtual clock is already installed on this thread; drop its \
                 guard before installing another"
            );
            *source = Source::Virtual(VirtualClock::default());
        });
        VirtualClockGuard {
            _not_send: std::marker::PhantomData,
        }
    }

    impl Drop for VirtualClockGuard {
        fn drop(&mut self) {
            // Dropped outside the borrow: dropping wakers can run arbitrary code.
            let old = SOURCE
                .try_with(|source| std::mem::replace(&mut *source.borrow_mut(), Source::Real));
            drop(old);
        }
    }

    /// Moves virtual time forward by `by`, stepping through each intervening deadline in
    /// order. For each deadline it sets `now` to it, wakes the live sleeps due there in
    /// registration order, then drains the local task queue (when this is the UI thread of
    /// an initialized runtime) so continuations run at that time and new sleeps are measured
    /// from it. A
    /// sleep armed inside the interval fires in the same call. Afterwards `now` is the
    /// target, `now + by` saturating at [`Duration::MAX`].
    ///
    /// The cost scales with the deadlines inside the interval: a self-re-arming timer whose
    /// period is much shorter than `by` runs one iteration per period. That is intended.
    ///
    /// # Panics
    ///
    /// Panics if no virtual clock is installed on this thread.
    pub fn advance(by: Duration) {
        let target = SOURCE.with_borrow(|source| match source {
            Source::Virtual(clock) => clock.now.saturating_add(by),
            Source::Real => {
                panic!("frust-reactive: advance called with no virtual clock installed")
            }
        });
        loop {
            // Woken and dropped outside the borrow: both can run arbitrary code.
            let due = SOURCE.with_borrow_mut(|source| match source {
                Source::Virtual(clock) => clock.take_next_due(target),
                Source::Real => None,
            });
            let Some(entries) = due else { break };
            for entry in entries {
                entry.wake_if_live();
            }
            // Only the UI thread owns the local queue; `pump_local` asserts that.
            if crate::executor::is_ui_thread()
                && let Some(runtime) = ReactiveRuntime::get()
            {
                runtime.pump_local();
            }
        }
        SOURCE.with_borrow_mut(|source| {
            if let Source::Virtual(clock) = source {
                clock.now = clock.now.max(target);
            }
        });
    }

    /// The virtual time elapsed since install, or zero when none is installed.
    pub fn now() -> Duration {
        SOURCE.with_borrow(|source| match source {
            Source::Virtual(clock) => clock.now,
            Source::Real => Duration::ZERO,
        })
    }

    /// Whether this thread has a virtual clock installed.
    pub fn is_virtual() -> bool {
        SOURCE.with_borrow(|source| matches!(source, Source::Virtual(_)))
    }

    /// Live sleeps registered and not yet due: a dropped sleep leaves the count at once.
    /// (A sleep whose deadline cannot be represented is never registered.)
    pub fn pending_count() -> usize {
        SOURCE.with_borrow(|source| match source {
            Source::Virtual(clock) => clock.live_count(),
            Source::Real => 0,
        })
    }
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
    /// no unwind guard) or stop the other timers of a background flush. Only in unwinding
    /// builds: under `panic = "abort"` (the release profile) `catch_unwind` catches nothing
    /// and a panicking callback aborts the process.
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
/// in it is logged rather than propagated in unwinding builds; with `panic = "abort"` (the
/// release profile) a panicking callback aborts the process, including inside a flush. See
/// the [module docs](self) for when it fires.
#[must_use = "dropping a Timer cancels it; keep the handle, or call detach()"]
pub struct Timer {
    inner: Rc<TimerInner>,
}

impl Timer {
    /// Arms a timer that runs `callback` once `duration` of wall time has passed.
    ///
    /// Each arming under an owner adds one small cleanup entry that lasts until that owner is
    /// cleaned up, so a self-re-arming timer or per-event debounce that runs indefinitely
    /// should arm under a short-lived child owner.
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

    /// Also fires this timer at the first of the app going Resumed to Inactive
    /// ([`on_inactive`]) or Inactive to Hidden ([`on_hide`]). A timer armed while already
    /// Inactive, or by a callback in the Inactive flush, runs at the Hidden edge (Android
    /// `onStop`, iOS `didEnterBackground`). It also fires on focus loss with no foreground
    /// exit (desktop and web window focus, the Android notification shade or split-screen,
    /// iOS Control Center or a call); it is one-shot and consumed, so do not flag a timer
    /// whose delay must be honoured while the app stays visible.
    ///
    /// The callback runs synchronously on the UI thread inside iOS's
    /// `applicationWillResignActive` or Android's `Activity.onPause` (or, at the Hidden edge,
    /// `onStop` / `didEnterBackground`), before control returns to the OS, so work done
    /// **synchronously in the callback** completes before the platform can suspend the
    /// process. Work it hands to `spawn_blocking`, `spawn_local` or `spawn` is not covered:
    /// the few background pumps a shell makes are not guaranteed to happen, and the process
    /// may be suspended first. Keep it short, because the OS waits on these main-thread
    /// callbacks: a small SQLite transaction or an atomic file write is in budget, a network
    /// call is not.
    ///
    /// [`on_inactive`]: AppLifecycleListener::on_inactive
    /// [`on_hide`]: AppLifecycleListener::on_hide
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
        listener.get_or_insert_with(|| {
            let owner = std::thread::current().id();
            AppLifecycleListener::new()
                .on_inactive(move || drain_on_owner(owner))
                .on_hide(move || drain_on_owner(owner))
        });
    });
}

/// The listener table is process-wide and the registry is per thread, so only the thread
/// that installed a listener may drain through it. Otherwise a listener whose thread is
/// still exiting would drain the pushing thread's registry a second time.
fn drain_on_owner(owner: std::thread::ThreadId) {
    if std::thread::current().id() == owner {
        drain_flush_registry();
    }
}

/// Empties the registry before firing. A flagged timer armed by a callback during the
/// Inactive drain is consumed at the Hidden edge of the same exit; one armed during the
/// Hidden drain waits for the next exit. Runs at Inactive and at Hidden; a timer fires at
/// most once, because the drain empties the registry and `fire_now` is a no-op after the
/// first. [`TimerInner::run_callback`] isolates each panic.
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

    fn counter() -> (Rc<Cell<u32>>, impl FnOnce() + 'static) {
        let count = Rc::new(Cell::new(0));
        let bump = Rc::clone(&count);
        (count, move || bump.set(bump.get() + 1))
    }

    #[test]
    fn a_timer_fires_once_after_its_deadline() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let timer = Timer::new(SHORT, bump);
        rt.pump_local();
        assert!(timer.is_active());
        assert_eq!(count.get(), 0, "arming must not run the callback");

        advance(SHORT);
        rt.pump_local();
        assert_eq!(count.get(), 1);
        assert!(!timer.is_active());

        advance(LONG);
        rt.pump_local();
        assert_eq!(count.get(), 1, "the callback runs exactly once");
    }

    #[test]
    fn cancel_stops_the_timer_at_once() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let timer = Timer::new(SHORT, bump);
        rt.pump_local();
        timer.cancel();
        assert!(!timer.is_active());
        advance(LONG);
        rt.pump_local();
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn dropping_the_handle_cancels_and_detach_does_not() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (dropped, bump_dropped) = counter();
        drop(Timer::new(SHORT, bump_dropped));
        let (detached, bump_detached) = counter();
        Timer::new(SHORT, bump_detached).detach();
        rt.pump_local();

        advance(SHORT);
        rt.pump_local();
        advance(LONG);
        rt.pump_local();
        assert_eq!(dropped.get(), 0, "a dropped handle cancels");
        assert_eq!(detached.get(), 1, "a detached handle keeps running");
    }

    #[test]
    fn owner_cleanup_cancels_held_and_detached_timers() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
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
        rt.pump_local();
        assert!(timer.is_active());

        owner.cleanup();
        assert!(!timer.is_active(), "inactive before the next poll");
        advance(LONG);
        rt.pump_local();
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
        let _clock = install_virtual_clock();
        let seen = Rc::new(Cell::new(None));
        let write = Rc::clone(&seen);
        let owner = rt.with_owner(Owner::new);
        let _timer = owner.with(|| {
            provide_context(Marker(7));
            Timer::new(SHORT, move || {
                write.set(use_context::<Marker>().map(|marker| marker.0));
            })
        });
        rt.pump_local();
        advance(SHORT);
        rt.pump_local();
        assert_eq!(seen.get(), Some(7));
    }

    #[test]
    fn spawn_local_scoped_keeps_the_owner_across_an_await_and_spawn_local_loses_it() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let scoped = Rc::new(Cell::new(None));
        let plain = Rc::new(Cell::new(None));
        let (scoped_write, plain_write) = (Rc::clone(&scoped), Rc::clone(&plain));
        let owner = rt.with_owner(Owner::new);
        owner.with(|| {
            provide_context(Marker(7));
            spawn_local_scoped(async move {
                sleep(SHORT).await;
                scoped_write.set(Some(use_context::<Marker>().is_some()));
            });
            Executor::spawn_local(async move {
                sleep(SHORT).await;
                plain_write.set(Some(use_context::<Marker>().is_some()));
            });
        });

        rt.pump_local();
        advance(SHORT);
        rt.pump_local();
        assert_eq!(scoped.get(), Some(true));
        assert_eq!(plain.get(), Some(false));
    }

    #[test]
    fn owner_cleanup_aborts_spawn_local_scoped() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let owner = rt.with_owner(Owner::new);
        owner.with(|| {
            spawn_local_scoped(async move {
                sleep(SHORT).await;
                bump();
            });
        });
        rt.pump_local();
        owner.cleanup();
        advance(LONG);
        rt.pump_local();
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn fire_now_runs_in_the_caller_and_retires_the_deadline() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let timer = Timer::new(SHORT, bump);
        timer.fire_now();
        assert_eq!(count.get(), 1, "fire_now is synchronous");
        assert!(!timer.is_active());
        advance(LONG);
        rt.pump_local();
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
    fn a_timer_flagged_while_inactive_flushes_at_hidden() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);
        push_app_lifecycle(AppLifecycleState::Inactive);
        let (count, bump) = counter();
        let timer = Timer::new(LONG, bump).flush_on_background();
        assert!(timer.is_active());
        assert_eq!(count.get(), 0);

        push_app_lifecycle(AppLifecycleState::Paused);
        assert_eq!(count.get(), 1, "fires synchronously inside the push");
        assert!(!timer.is_active());

        push_app_lifecycle(AppLifecycleState::Resumed);
        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn a_direct_resumed_to_paused_push_flushes_a_flagged_timer_at_inactive() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);
        let flushed = Arc::new(AtomicUsize::new(0));
        let write = Arc::clone(&flushed);
        let timer = Timer::new(LONG, move || {
            write.fetch_add(1, Ordering::SeqCst);
        })
        .flush_on_background();

        // Registered after the flush listener, so it runs later in the same push.
        let seen_at_inactive = Arc::new(AtomicUsize::new(usize::MAX));
        let seen_at_hide = Arc::new(AtomicUsize::new(usize::MAX));
        let (seen_inactive, source_inactive) =
            (Arc::clone(&seen_at_inactive), Arc::clone(&flushed));
        let (seen_hide, source_hide) = (Arc::clone(&seen_at_hide), Arc::clone(&flushed));
        let _observer = AppLifecycleListener::new()
            .on_inactive(move || {
                seen_inactive.store(source_inactive.load(Ordering::SeqCst), Ordering::SeqCst);
            })
            .on_hide(move || {
                seen_hide.store(source_hide.load(Ordering::SeqCst), Ordering::SeqCst);
            });

        // Resumed -> Paused is one push that walks Inactive, Hidden, Paused.
        push_app_lifecycle(AppLifecycleState::Paused);
        assert_eq!(flushed.load(Ordering::SeqCst), 1, "fired exactly once");
        assert_eq!(
            seen_at_inactive.load(Ordering::SeqCst),
            1,
            "already fired when the inactive observer runs"
        );
        assert_eq!(
            seen_at_hide.load(Ordering::SeqCst),
            1,
            "already fired when the hide observer runs"
        );
        assert!(!timer.is_active());
    }

    #[test]
    fn a_timer_flagged_while_resumed_fires_once_across_inactive_and_hidden() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);
        let (count, bump) = counter();
        let _timer = Timer::new(LONG, bump).flush_on_background();
        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(count.get(), 1);
        push_app_lifecycle(AppLifecycleState::Hidden);
        push_app_lifecycle(AppLifecycleState::Paused);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn a_timer_flagged_while_inactive_then_resumed_flushes_at_the_next_inactive() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);
        push_app_lifecycle(AppLifecycleState::Inactive);
        let (count, bump) = counter();
        let timer = Timer::new(LONG, bump).flush_on_background();
        push_app_lifecycle(AppLifecycleState::Resumed);
        assert_eq!(count.get(), 0);
        assert!(timer.is_active());
        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn another_threads_listener_does_not_drain_this_threads_registry() {
        let (_guard, _rt) = setup();
        let (count, bump) = counter();
        let _timer = Timer::new(LONG, bump).flush_on_background();
        let other = std::thread::spawn(|| std::thread::current().id())
            .join()
            .expect("id thread");

        drain_on_owner(other);
        assert_eq!(
            count.get(),
            0,
            "a foreign listener must not fire this timer"
        );

        drain_on_owner(std::thread::current().id());
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn a_timer_flagged_inside_a_flush_runs_at_the_hidden_edge_of_the_same_exit() {
        let (_guard, _rt) = setup();
        push_app_lifecycle(AppLifecycleState::Resumed);
        let (a_count, bump_a) = counter();
        let (b_count, bump_b) = counter();
        let b_slot: Rc<RefCell<Option<Timer>>> = Rc::new(RefCell::new(None));
        let arm_b = Rc::clone(&b_slot);
        let _a = Timer::new(LONG, move || {
            bump_a();
            *arm_b.borrow_mut() = Some(Timer::new(LONG, bump_b).flush_on_background());
        })
        .flush_on_background();

        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(a_count.get(), 1);
        assert_eq!(
            b_count.get(),
            0,
            "B is armed by the Inactive flush, not run by it"
        );
        assert!(b_slot.borrow().as_ref().is_some_and(Timer::is_active));

        push_app_lifecycle(AppLifecycleState::Hidden);
        assert_eq!(
            b_count.get(),
            1,
            "B runs at the Hidden edge of the same exit"
        );

        push_app_lifecycle(AppLifecycleState::Paused);
        assert_eq!(a_count.get(), 1);
        assert_eq!(b_count.get(), 1);
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

    // ---- virtual clock ----

    use super::test_support::{self, advance, install_virtual_clock};

    const fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn virtual_timer_fires_exactly_at_its_deadline_once() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let timer = Timer::new(ms(300), bump);
        rt.pump_local();
        advance(ms(299));
        rt.pump_local();
        assert_eq!(count.get(), 0);
        assert!(timer.is_active());
        advance(ms(1));
        rt.pump_local();
        assert_eq!(count.get(), 1);
        assert!(!timer.is_active());
        advance(ms(1000));
        rt.pump_local();
        assert_eq!(count.get(), 1);
        assert_eq!(test_support::pending_count(), 0);
    }

    #[test]
    fn virtual_timers_fire_in_deadline_order() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let order = Rc::new(RefCell::new(Vec::new()));
        let (a, b) = (Rc::clone(&order), Rc::clone(&order));
        let _slow = Timer::new(ms(100), move || a.borrow_mut().push(100));
        let _fast = Timer::new(ms(50), move || b.borrow_mut().push(50));
        rt.pump_local();
        advance(ms(100));
        rt.pump_local();
        assert_eq!(*order.borrow(), vec![50, 100]);
    }

    #[test]
    fn virtual_cancelled_and_dropped_timers_leave_no_entry() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let cancelled = Timer::new(ms(100), bump);
        let (dropped_count, bump_dropped) = counter();
        let dropped = Timer::new(ms(100), bump_dropped);
        rt.pump_local();
        assert_eq!(test_support::pending_count(), 2);
        cancelled.cancel();
        drop(dropped);
        rt.pump_local();
        advance(ms(500));
        rt.pump_local();
        assert_eq!(count.get(), 0);
        assert_eq!(dropped_count.get(), 0);
        assert_eq!(test_support::pending_count(), 0);
    }

    #[test]
    fn virtual_sleep_resolves_only_after_advance() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        Executor::spawn_local(async move {
            sleep(ms(20)).await;
            bump();
        });
        rt.pump_local();
        std::thread::sleep(ms(50));
        rt.pump_local();
        assert_eq!(count.get(), 0, "wall time must not move a virtual sleep");
        advance(ms(20));
        rt.pump_local();
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn virtual_fire_now_and_owner_cleanup_behave_as_on_wall_time() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (count, bump) = counter();
        let timer = Timer::new(LONG, bump);
        rt.pump_local();
        timer.fire_now();
        assert_eq!(count.get(), 1);
        assert!(!timer.is_active());
        advance(LONG);
        rt.pump_local();
        timer.fire_now();
        assert_eq!(count.get(), 1);

        let (held, bump_held) = counter();
        let (detached, bump_detached) = counter();
        let (owner, timer) = rt.with_owner(|| {
            let owner = Owner::new();
            let timer = owner.with(|| {
                Timer::new(ms(10), bump_detached).detach();
                Timer::new(ms(10), bump_held)
            });
            (owner, timer)
        });
        rt.pump_local();
        owner.cleanup();
        assert!(!timer.is_active());
        rt.pump_local();
        advance(ms(10));
        rt.pump_local();
        assert_eq!((held.get(), detached.get()), (0, 0));
        assert_eq!(test_support::pending_count(), 0);
    }

    #[test]
    fn virtual_clock_guard_restores_wall_time_and_double_install_panics() {
        let (_guard, rt) = setup();
        {
            let _clock = install_virtual_clock();
            assert!(test_support::is_virtual());
            advance(ms(5));
            assert_eq!(test_support::now(), ms(5));
        }
        assert!(!test_support::is_virtual());
        let (count, bump) = counter();
        Executor::spawn_local(async move {
            sleep(SHORT).await;
            bump();
        });
        pump_until(rt, || count.get() > 0);
        assert_eq!(count.get(), 1);
    }

    #[test]
    #[should_panic(expected = "already installed")]
    fn installing_a_second_virtual_clock_panics() {
        let (_guard, _rt) = setup();
        let _first = install_virtual_clock();
        let _second = install_virtual_clock();
    }

    /// `frust::spawn` needs a `Send` future, so the virtual arm must not cost `sleep` that.
    #[test]
    fn sleep_stays_send_with_the_virtual_clock_compiled_in() {
        fn assert_send<T: Send>(_: T) {}
        assert_send(sleep(SHORT));
    }

    #[test]
    fn advance_off_the_ui_thread_wakes_without_draining() {
        let (_guard, _rt) = setup();
        std::thread::spawn(|| {
            let _clock = test_support::install_virtual_clock();
            let mut wait = Box::pin(sleep(SHORT));
            let waker = futures::task::noop_waker();
            let mut cx = std::task::Context::from_waker(&waker);
            assert!(wait.as_mut().poll(&mut cx).is_pending());
            test_support::advance(SHORT);
            assert!(wait.as_mut().poll(&mut cx).is_ready());
        })
        .join()
        .expect("advance must not assert off the UI thread");
    }

    #[test]
    fn a_flagged_virtual_timer_still_flushes_at_inactive() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        push_app_lifecycle(AppLifecycleState::Resumed);
        let (count, bump) = counter();
        let timer = Timer::new(LONG, bump).flush_on_background();
        rt.pump_local();
        push_app_lifecycle(AppLifecycleState::Inactive);
        assert_eq!(count.get(), 1);
        assert!(!timer.is_active());
        rt.pump_local();
        advance(LONG);
        rt.pump_local();
        assert_eq!(count.get(), 1);
    }

    fn spawn(fut: impl Future<Output = ()> + 'static) {
        Executor::spawn_local(fut);
    }

    #[test]
    fn chained_sleeps_fire_under_one_advance_as_under_two() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let flag = Rc::new(Cell::new(false));
        let set = Rc::clone(&flag);
        spawn(async move {
            sleep(ms(50)).await;
            sleep(ms(50)).await;
            set.set(true);
        });
        rt.pump_local();
        advance(ms(100));
        assert!(flag.get());
        assert_eq!(test_support::now(), ms(100));

        let split = Rc::new(Cell::new(false));
        let set = Rc::clone(&split);
        spawn(async move {
            sleep(ms(50)).await;
            sleep(ms(50)).await;
            set.set(true);
        });
        rt.pump_local();
        advance(ms(50));
        rt.pump_local();
        advance(ms(50));
        assert!(split.get());
    }

    fn rearm(times: Rc<RefCell<Vec<Duration>>>, left: u32) {
        let next = Rc::clone(&times);
        let timer = Timer::new(ms(100), move || {
            next.borrow_mut().push(test_support::now());
            if left > 1 {
                rearm(next, left - 1);
            }
        });
        timer.detach();
    }

    #[test]
    fn a_re_arming_timer_fires_from_each_previous_deadline() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let times = Rc::new(RefCell::new(Vec::new()));
        rearm(Rc::clone(&times), 10);
        rt.pump_local();
        advance(ms(1000));
        let expected: Vec<Duration> = (1..=10).map(|n| ms(100 * n)).collect();
        assert_eq!(*times.borrow(), expected);
    }

    #[test]
    fn interleaved_chains_log_in_virtual_time_order() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let log = Rc::new(RefCell::new(Vec::new()));
        let a = Rc::clone(&log);
        spawn(async move {
            sleep(ms(10)).await;
            sleep(ms(10)).await;
            a.borrow_mut().push("A");
        });
        let b = Rc::clone(&log);
        spawn(async move {
            sleep(ms(30)).await;
            b.borrow_mut().push("B");
        });
        rt.pump_local();
        advance(ms(30));
        assert_eq!(*log.borrow(), vec!["A", "B"]);
    }

    #[test]
    fn now_is_the_deadline_during_a_firing_and_the_target_after() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let (a, b) = (Rc::clone(&seen), Rc::clone(&seen));
        let _t1 = Timer::new(ms(30), move || a.borrow_mut().push(test_support::now()));
        let _t2 = Timer::new(ms(70), move || b.borrow_mut().push(test_support::now()));
        rt.pump_local();
        advance(ms(100));
        assert_eq!(*seen.borrow(), vec![ms(30), ms(70)]);
        assert_eq!(test_support::now(), ms(100));
    }

    #[test]
    fn an_unrepresentable_deadline_never_fires_and_does_not_panic() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        advance(ms(5));
        let (count, bump) = counter();
        let timer = Timer::new(Duration::MAX, bump);
        rt.pump_local();
        assert_eq!(test_support::pending_count(), 0);
        advance(Duration::MAX);
        rt.pump_local();
        assert_eq!(count.get(), 0);
        assert!(timer.is_active());
        assert_eq!(test_support::now(), Duration::MAX);
    }

    #[test]
    fn a_dropped_sleep_leaves_pending_count_at_once() {
        let (_guard, rt) = setup();
        let _clock = install_virtual_clock();
        let (task, handle) = futures::future::abortable(sleep(ms(100)));
        spawn(async move {
            let _ = task.await;
        });
        rt.pump_local();
        assert_eq!(test_support::pending_count(), 1);
        handle.abort();
        rt.pump_local();
        assert_eq!(test_support::pending_count(), 0);
    }
}
