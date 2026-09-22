//! A minimal, `std`-only single-producer/single-consumer oneshot future —
//! the [`crate::AuthSession::start`] awaitable itself. No `futures`/`tokio`
//! dependency (this crate's platform-plugin charter — see
//! `docs/CODE_STANDARDS.md`'s Plugin Conventions): a platform backend
//! resolves exactly one outcome per session, from an arbitrary thread (the
//! Android main looper, an Apple completion block dispatched onto the main
//! queue, …), and the caller polls the [`Receiver`] from whatever executor
//! it likes (see [`crate`]'s crate doc's *awaitable-future contract*).
//!
//! # Shape
//!
//! [`channel`] returns a [`Sender`]/[`Receiver`] pair sharing one [`Inner`]
//! behind an `Arc`. [`Sender::send`] stores the outcome and wakes whichever
//! [`Waker`] the [`Receiver`] last registered, if any (a `send` before the
//! first `poll` simply leaves the value waiting — [`Receiver::poll`] sees it
//! immediately on that first poll and never registers a waker at all).
//! Dropping the [`Sender`] without calling [`Sender::send`] first — a
//! backend bailing out before it ever reaches a resolution point — resolves
//! the channel to [`crate::AuthSessionError::Platform`] rather than leaving
//! the [`Receiver`] pending forever.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use crate::{AuthSessionError, AuthSessionOutcome};

/// The value a [`Sender`]/[`Receiver`] pair carries — [`crate::AuthSession::
/// start`]'s eventual `Result`.
type Value = Result<AuthSessionOutcome, AuthSessionError>;

/// The state shared between a [`Sender`] and its [`Receiver`], behind one
/// [`Mutex`].
struct State {
    /// `Some` once [`Sender::send`] (or the drop-without-send fallback) has
    /// run; [`Receiver::poll`] takes it on the first poll that observes it.
    value: Option<Value>,
    /// The waker from the most recent [`Receiver::poll`] that found no
    /// value yet, so [`Sender::send`] has something to wake.
    waker: Option<Waker>,
}

/// The state a [`Sender`]/[`Receiver`] pair shares, held behind an `Arc` so
/// either half can outlive the other.
struct Inner {
    state: Mutex<State>,
}

/// Lock `mutex`, recovering from poisoning instead of panicking — a panic on
/// one side of the channel (backend or caller) must not turn the other
/// side's next call into a panic too (`docs/CODE_STANDARDS.md`'s no-panic
/// rule near an FFI boundary; matches `plugins/camera/src/android.rs`'s own
/// `lock` helper). The guarded data is a plain `Option`/`Waker` pair, so a
/// poisoned view is still coherent enough to use.
fn lock(inner: &Inner) -> MutexGuard<'_, State> {
    inner
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The sending half of a oneshot channel — held by [`crate`]'s `ACTIVE` slot
/// and handed to the cfg-selected backend, which calls [`Self::send`]
/// exactly once.
pub(crate) struct Sender {
    inner: Arc<Inner>,
    /// Set by [`Self::send`] just before it consumes `self`, so the [`Drop`]
    /// impl below knows a value was already stored and does not overwrite it
    /// with the drop-without-send fallback.
    sent: bool,
}

impl Sender {
    /// Store `value` and wake whichever [`Waker`] the [`Receiver`] last
    /// registered, if any. Consumes `self` — a [`Sender`] resolves at most
    /// once.
    pub(crate) fn send(mut self, value: Value) {
        self.sent = true;
        let waker = {
            let mut state = lock(&self.inner);
            state.value = Some(value);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl Drop for Sender {
    /// A [`Sender`] dropped without [`Self::send`] having run — a backend
    /// bailing out before it reaches a resolution point — resolves the
    /// channel to [`AuthSessionError::Platform`] rather than leaving the
    /// [`Receiver`] pending forever (this module's doc's *Shape* section).
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        let waker = {
            let mut state = lock(&self.inner);
            state.value = Some(Err(AuthSessionError::Platform(
                "session dropped without an outcome".to_string(),
            )));
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The receiving half of a oneshot channel — the [`Future`]
/// [`crate::AuthSession::start`] returns.
pub(crate) struct Receiver {
    inner: Arc<Inner>,
}

impl Future for Receiver {
    type Output = Value;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = lock(&self.inner);
        if let Some(value) = state.value.take() {
            return Poll::Ready(value);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Create a fresh [`Sender`]/[`Receiver`] pair sharing one [`Inner`].
pub(crate) fn channel() -> (Sender, Receiver) {
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            value: None,
            waker: None,
        }),
    });
    (
        Sender {
            inner: Arc::clone(&inner),
            sent: false,
        },
        Receiver { inner },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

    /// A `Waker` that only counts how many times it was woken — enough to
    /// assert `Sender::send` actually wakes a previously-registered waker,
    /// without needing a real executor.
    fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
        fn clone(data: *const ()) -> RawWaker {
            unsafe { Arc::increment_strong_count(data as *const AtomicUsize) };
            RawWaker::new(data, &VTABLE)
        }
        fn wake(data: *const ()) {
            let counter = unsafe { Arc::from_raw(data as *const AtomicUsize) };
            counter.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(data: *const ()) {
            let counter = unsafe { &*(data as *const AtomicUsize) };
            counter.fetch_add(1, Ordering::SeqCst);
        }
        fn drop_fn(data: *const ()) {
            unsafe { drop(Arc::from_raw(data as *const AtomicUsize)) };
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_fn);

        let counter = Arc::new(AtomicUsize::new(0));
        let raw = RawWaker::new(Arc::into_raw(Arc::clone(&counter)) as *const (), &VTABLE);
        (unsafe { Waker::from_raw(raw) }, counter)
    }

    fn poll_once(receiver: &mut Receiver, waker: &Waker) -> Poll<Value> {
        let mut cx = Context::from_waker(waker);
        Pin::new(receiver).poll(&mut cx)
    }

    #[test]
    fn send_before_poll_is_observed_on_first_poll() {
        let (sender, mut receiver) = channel();
        sender.send(Ok(AuthSessionOutcome::Cancelled));

        let (waker, counter) = counting_waker();
        match poll_once(&mut receiver, &waker) {
            Poll::Ready(Ok(AuthSessionOutcome::Cancelled)) => {}
            other => panic!("expected Ready(Ok(Cancelled)), got {other:?}"),
        }
        // No poll ever found the value missing, so no waker was ever stored
        // or fired.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn poll_then_send_wakes_the_registered_waker() {
        let (sender, mut receiver) = channel();

        let (waker, counter) = counting_waker();
        assert!(matches!(poll_once(&mut receiver, &waker), Poll::Pending));
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        sender.send(Ok(AuthSessionOutcome::Callback("scheme://cb".to_string())));
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        match poll_once(&mut receiver, &waker) {
            Poll::Ready(Ok(AuthSessionOutcome::Callback(cb))) => assert_eq!(cb, "scheme://cb"),
            other => panic!("expected Ready(Ok(Callback(..))), got {other:?}"),
        }
    }

    #[test]
    fn drop_without_send_resolves_platform_error() {
        let (sender, mut receiver) = channel();
        drop(sender);

        let (waker, _counter) = counting_waker();
        match poll_once(&mut receiver, &waker) {
            Poll::Ready(Err(AuthSessionError::Platform(message))) => {
                assert_eq!(message, "session dropped without an outcome");
            }
            other => panic!("expected Ready(Err(Platform(..))), got {other:?}"),
        }
    }
}
