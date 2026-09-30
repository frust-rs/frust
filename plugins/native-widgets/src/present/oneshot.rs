//! A minimal, `std`-only single-producer/single-consumer oneshot future —
//! the channel every [`super::Presentation`] resolves through. No
//! `futures`/`tokio` dependency (the platform-plugin charter,
//! `docs/PLUGINS_CODE_STANDARDS.md`): a platform arm resolves exactly one
//! outcome per presentation, from whatever thread its platform callback lands
//! on (the Android main looper, an Apple block dispatched onto the main
//! queue, …), and the caller polls the [`Receiver`] from whatever executor it
//! likes (`frust::spawn_local` or any other).
//!
//! Ported from `frust-auth-session`'s own `oneshot` module and made generic
//! over the outcome type, so one channel serves every presentation kind.
//!
//! # Shape
//!
//! [`channel`] returns a [`Sender`]/[`Receiver`] pair sharing one [`Inner`]
//! behind an `Arc`. [`Sender::send`] stores the value and wakes whichever
//! [`Waker`] the [`Receiver`] last registered, if any (a `send` before the
//! first `poll` simply leaves the value waiting — [`Receiver::poll`] sees it
//! immediately on that first poll and never registers a waker at all).
//! Dropping the [`Sender`] without calling [`Sender::send`] first — an arm
//! bailing out before it ever reaches a resolution point — resolves the
//! channel to [`PresentError::Platform`] rather than leaving the [`Receiver`]
//! pending forever.
//!
//! # The generation both halves carry
//!
//! A pair is created for exactly one presentation and both halves carry that
//! presentation's generation, so either end can tell [`super`] which
//! presentation just ended — [`super::release_if_live`], the one seam between
//! this module and the process-wide Busy slot:
//!
//! - the [`Sender`] releases the slot as it resolves (by value or by the
//!   drop-without-send fallback), **before** waking the caller, so a caller
//!   whose continuation immediately shows the next presentation is not
//!   refused as `Busy` by the one that just finished;
//! - the [`Receiver`] releases it when dropped, so a caller abandoning the
//!   future frees the slot instead of wedging it. A value sent after that is
//!   discarded on arrival ([`Sender::send`] answers `false`).
//!
//! Release is generation-checked, so a channel built with a generation
//! nothing is live under drops and sends with no effect on the slot — which
//! is what keeps this module independently testable.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use super::PresentError;

/// The value a [`Sender`]/[`Receiver`] pair carries — a presentation's
/// eventual outcome, or the error that ended it.
pub(crate) type Value<T> = Result<T, PresentError>;

/// The [`PresentError::Platform`] message the drop-without-send fallback
/// resolves with.
pub(crate) const DROPPED_WITHOUT_OUTCOME: &str = "presentation ended without an outcome";

/// The state shared between a [`Sender`] and its [`Receiver`], behind one
/// [`Mutex`].
struct State<T> {
    /// `Some` once [`Sender::send`] (or the drop-without-send fallback) has
    /// run; [`Receiver::poll`] takes it on the first poll that observes it.
    value: Option<Value<T>>,
    /// The waker from the most recent [`Receiver::poll`] that found no value
    /// yet, so [`Sender::send`] has something to wake.
    waker: Option<Waker>,
    /// Set by [`Receiver`]'s `Drop`: nobody will ever read a value again, so
    /// a later send discards its value instead of storing it.
    closed: bool,
}

/// The state a [`Sender`]/[`Receiver`] pair shares, held behind an `Arc` so
/// either half can outlive the other.
struct Inner<T> {
    state: Mutex<State<T>>,
}

/// Lock `inner`, recovering from poisoning instead of panicking — a panic on
/// one side of the channel (arm or caller) must not turn the other side's
/// next call into a panic too (the no-panic rule near an FFI boundary). The
/// guarded data is a plain `Option`/`Waker`/flag triple, so a poisoned view
/// is still coherent enough to use.
fn lock<T>(inner: &Inner<T>) -> MutexGuard<'_, State<T>> {
    inner
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Release `generation`'s slot, then store `value` and wake the receiver —
/// unless the receiver is already gone, in which case the value is dropped
/// here. Answers whether the value was stored for a live receiver.
fn complete<T>(inner: &Inner<T>, generation: u64, value: Value<T>) -> bool {
    // Slot first, wake second: the woken caller may run arbitrary code —
    // including its next `show` — and must find the slot free.
    super::release_if_live(generation);
    let waker = {
        let mut state = lock(inner);
        if state.closed {
            return false;
        }
        state.value = Some(value);
        state.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
    true
}

/// The sending half of a oneshot channel — handed to the cfg-selected
/// platform arm, which calls [`Self::send`] exactly once.
pub(crate) struct Sender<T> {
    inner: Arc<Inner<T>>,
    /// The generation of the presentation this sender resolves (module
    /// doc's *The generation both halves carry*).
    generation: u64,
    /// Set by [`Self::send`] just before it consumes `self`, so the [`Drop`]
    /// impl below knows a value was already stored and does not overwrite it
    /// with the drop-without-send fallback.
    sent: bool,
}

impl<T> Sender<T> {
    /// The generation of the presentation this sender resolves.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Resolve the presentation with `value`: release its Busy slot, then
    /// store the value and wake whichever [`Waker`] the [`Receiver`] last
    /// registered. Consumes `self` — a [`Sender`] resolves at most once.
    ///
    /// Answers `false` when the [`Receiver`] was already dropped: the value
    /// is discarded (the module doc's late-send rule). An arm needs no
    /// special handling for that case.
    pub(crate) fn send(mut self, value: Value<T>) -> bool {
        self.sent = true;
        complete(&self.inner, self.generation, value)
    }
}

impl<T> Drop for Sender<T> {
    /// A [`Sender`] dropped without [`Self::send`] having run — an arm
    /// bailing out before it reaches a resolution point — resolves the
    /// channel to [`PresentError::Platform`] rather than leaving the
    /// [`Receiver`] pending forever (this module's doc's *Shape* section).
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        complete(
            &self.inner,
            self.generation,
            Err(PresentError::Platform(DROPPED_WITHOUT_OUTCOME.to_string())),
        );
    }
}

/// The receiving half of a oneshot channel — the future a pending
/// [`super::Presentation`] polls.
pub(crate) struct Receiver<T> {
    inner: Arc<Inner<T>>,
    /// The generation of the presentation this receiver belongs to, so
    /// [`Self::drop`] can release that presentation's slot.
    generation: u64,
}

impl<T> Drop for Receiver<T> {
    /// The caller dropped the future — mark the channel closed (a later send
    /// is discarded) and release the Busy slot if it is still this
    /// presentation's, so the next `show` is accepted rather than refused as
    /// [`PresentError::Busy`]. A no-op on the slot when the presentation
    /// already resolved (the usual case) or a newer one now holds it.
    fn drop(&mut self) {
        lock(&self.inner).closed = true;
        super::release_if_live(self.generation);
    }
}

impl<T> Future for Receiver<T> {
    type Output = Value<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = lock(&self.inner);
        if let Some(value) = state.value.take() {
            return Poll::Ready(value);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Create a fresh [`Sender`]/[`Receiver`] pair sharing one [`Inner`], for
/// the presentation identified by `generation`.
pub(crate) fn channel<T>(generation: u64) -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            value: None,
            waker: None,
            closed: false,
        }),
    });
    (
        Sender {
            inner: Arc::clone(&inner),
            generation,
            sent: false,
        },
        Receiver { inner, generation },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

    /// The generation these tests build their channels with. Deliberately at
    /// the top of the range `super::super::next_generation` counts up from,
    /// so a pair built here can never match a live presentation another test
    /// in the same binary holds — the release both halves perform then has
    /// nothing to take, which is the point: this module's own tests exercise
    /// the channel, not the slot (`super::super`'s tests cover that).
    const UNCLAIMED_GENERATION: u64 = u64::MAX;

    /// A `Waker` that only counts how many times it was woken — enough to
    /// assert `Sender::send` actually wakes a previously-registered waker,
    /// without needing a real executor.
    pub(crate) fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
        fn clone(data: *const ()) -> RawWaker {
            // SAFETY: `data` is an `Arc<AtomicUsize>` pointer produced by
            // `Arc::into_raw` below; the clone takes one more strong count.
            unsafe { Arc::increment_strong_count(data as *const AtomicUsize) };
            RawWaker::new(data, &VTABLE)
        }
        fn wake(data: *const ()) {
            // SAFETY: consumes the strong count this waker owns.
            let counter = unsafe { Arc::from_raw(data as *const AtomicUsize) };
            counter.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(data: *const ()) {
            // SAFETY: borrows the count this waker owns without consuming it.
            let counter = unsafe { &*(data as *const AtomicUsize) };
            counter.fetch_add(1, Ordering::SeqCst);
        }
        fn drop_fn(data: *const ()) {
            // SAFETY: releases the strong count this waker owns.
            unsafe { drop(Arc::from_raw(data as *const AtomicUsize)) };
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_fn);

        let counter = Arc::new(AtomicUsize::new(0));
        let raw = RawWaker::new(Arc::into_raw(Arc::clone(&counter)) as *const (), &VTABLE);
        // SAFETY: the vtable above upholds the `RawWaker` contract for an
        // `Arc<AtomicUsize>` data pointer.
        (unsafe { Waker::from_raw(raw) }, counter)
    }

    fn poll_once<T>(receiver: &mut Receiver<T>, waker: &Waker) -> Poll<Value<T>> {
        let mut cx = Context::from_waker(waker);
        Pin::new(receiver).poll(&mut cx)
    }

    #[test]
    fn send_before_poll_is_observed_on_first_poll() {
        let (sender, mut receiver) = channel::<&str>(UNCLAIMED_GENERATION);
        assert!(sender.send(Ok("done")));

        let (waker, counter) = counting_waker();
        assert_eq!(poll_once(&mut receiver, &waker), Poll::Ready(Ok("done")));
        // No poll ever found the value missing, so no waker was ever stored
        // or fired.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn poll_then_send_wakes_the_registered_waker_once() {
        let (sender, mut receiver) = channel::<u32>(UNCLAIMED_GENERATION);

        let (waker, counter) = counting_waker();
        assert_eq!(poll_once(&mut receiver, &waker), Poll::Pending);
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        assert!(sender.send(Ok(7)));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(poll_once(&mut receiver, &waker), Poll::Ready(Ok(7)));
    }

    #[test]
    fn the_value_is_delivered_once_then_the_receiver_stays_pending() {
        let (sender, mut receiver) = channel::<u32>(UNCLAIMED_GENERATION);
        assert!(sender.send(Ok(1)));

        let (waker, _counter) = counting_waker();
        assert_eq!(poll_once(&mut receiver, &waker), Poll::Ready(Ok(1)));
        // A spurious re-poll after completion is answered `Pending`, never a
        // second value and never a panic.
        assert_eq!(poll_once(&mut receiver, &waker), Poll::Pending);
    }

    #[test]
    fn drop_without_send_resolves_platform_error() {
        let (sender, mut receiver) = channel::<u32>(UNCLAIMED_GENERATION);
        drop(sender);

        let (waker, _counter) = counting_waker();
        assert_eq!(
            poll_once(&mut receiver, &waker),
            Poll::Ready(Err(PresentError::Platform(
                DROPPED_WITHOUT_OUTCOME.to_string()
            )))
        );
    }

    #[test]
    fn a_late_send_after_the_receiver_dropped_is_discarded() {
        let (sender, receiver) = channel::<u32>(UNCLAIMED_GENERATION);
        drop(receiver);
        assert!(
            !sender.send(Ok(3)),
            "a send into a closed channel must report the value discarded"
        );
    }

    #[test]
    fn both_halves_carry_the_generation() {
        let (sender, receiver) = channel::<u32>(UNCLAIMED_GENERATION);
        assert_eq!(sender.generation(), UNCLAIMED_GENERATION);
        assert_eq!(receiver.generation, UNCLAIMED_GENERATION);
    }
}
