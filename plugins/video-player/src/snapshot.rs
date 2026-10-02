//! The state every backend publishes into and every caller reads out of: an
//! atomic snapshot plus this session's single listener slot.
//!
//! # Why atomics rather than a lock around one struct
//!
//! [`crate::PlayerSession::snapshot`] is called from any thread — typically
//! a frame build, once per frame, while a platform callback is writing a new
//! position on the main thread. A mutex there would put the UI thread behind
//! the player's own callback thread for no benefit: every field is
//! independently meaningful, and a reader that sees a new position with a
//! not-yet-updated duration is reading a state the player genuinely passed
//! through. Only the error slot needs a lock (a `VideoError` is not an
//! integer), and [`Shared::snapshot`] takes it **only** when the state says
//! there is an error to read, so the ordinary path is lock-free.
//!
//! # Publishing, and the one rule a listener must follow
//!
//! [`Shared::publish`] updates the atomics **first** and invokes the
//! listener afterwards, so a listener that immediately calls
//! [`crate::PlayerSession::snapshot`] sees the event it was just handed
//! rather than the state before it. Every host callback publishes on the
//! platform main thread, so a listener runs there too: it must never block,
//! and it must never call back into its own session — a control call made
//! from inside a delivery is refused with [`VideoError::Reentrant`] by
//! [`reject_if_delivering`] rather than being allowed to re-enter the
//! backend mid-callback.
//!
//! `close` is the one call exempt from that refusal (its own doc says why),
//! and every backend's close publishes its own state change, so a publish
//! can happen while this thread is already delivering another one. That
//! nested publish is never run on the spot — it would put the listener back
//! on this thread's own stack and would clear the delivering flag out from
//! under the outer call the moment it finished. Instead it is queued and
//! delivered after the outer listener call returns, on the same thread, in
//! the order it was published: [`reject_if_delivering`] keeps refusing for
//! the whole outer delivery, including whatever the nested publish's own
//! drain runs, and the listener is never on the stack twice.

//! # Why the publishing half carries a dead-code allowance
//!
//! Only a platform backend publishes, so a non-test library build has no
//! caller for that half at all — the no-backend target reaches no player,
//! and the Android/Apple backends are placeholders that publish nothing
//! yet. `frust-iap`'s event-delivery path carries the identical
//! `cfg_attr(not(test), allow(dead_code))` for the same reason. Scoped to
//! `not(test)` rather than blanketed, so the path stays lint-covered in the
//! configuration that does exercise it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{PlaybackState, PlayerEvent, PlayerSnapshot, VideoError};

/// A registered listener callback, without the generation tag — what a
/// deferred delivery captures at publish time (module doc).
type Listener = Arc<dyn Fn(PlayerEvent) + Send + Sync>;

/// The listener currently registered on a session, tagged with the
/// generation that registered it.
type ListenerSlot = Mutex<Option<(u64, Listener)>>;

/// One session's published state, shared between the backend (writer) and
/// the app (reader) — see the module doc.
#[derive(Default)]
pub(crate) struct Shared {
    /// [`PlaybackState`] as its frozen wire code (see
    /// [`PlaybackState::from_code`]); `0` (`Idle`) until the backend
    /// publishes.
    state: AtomicU8,
    /// Playback position, milliseconds.
    position_ms: AtomicU64,
    /// Item duration in milliseconds, or `-1` for "not known yet" — a live
    /// stream never resolves one, and a file's is unknown until the item is
    /// ready.
    duration_ms: AtomicI64,
    /// Decoded video width/height in pixels, `0` until the platform reports
    /// the first frame geometry. Both are written from one
    /// [`PlayerEvent::VideoSize`], so a reader that sees a non-zero width
    /// with a zero height is reading between the two stores — the
    /// [`Shared::snapshot`] below reports `None` for that case rather than
    /// half a size.
    video_width: AtomicU32,
    video_height: AtomicU32,
    /// The last error published, readable through [`PlayerSnapshot::error`].
    /// Behind a lock because a [`VideoError`] carries owned strings; read
    /// only when [`Self::state`] says `Error` (module doc).
    error: Mutex<Option<VideoError>>,
    /// This session's single listener — see [`Self::set_listener`].
    listener: ListenerSlot,
    /// Bumped by every [`Self::set_listener`]; the tag a
    /// [`crate::ListenerHandle`]'s drop must match to unregister, so a stale
    /// handle can never remove the listener that replaced it.
    generation: AtomicU64,
    /// Set once by [`crate::PlayerSession::close`], which is what makes
    /// closing idempotent and every later control call report
    /// [`VideoError::Closed`].
    closed: AtomicBool,
}

thread_local! {
    /// Whether *this* thread is currently inside [`Shared::publish`]'s
    /// listener invocation — see [`reject_if_delivering`].
    static DELIVERING: Cell<bool> = const { Cell::new(false) };

    /// Events published while [`DELIVERING`] was already set on this thread
    /// — see the module doc's deferred-delivery rule. Drained in FIFO order
    /// by the outer [`Shared::publish`] once its own listener call returns.
    static DEFERRED: RefCell<VecDeque<(Listener, PlayerEvent)>> =
        RefCell::new(VecDeque::new());
}

/// Refuse a control call made from inside a listener invocation — the guard
/// every [`crate::PlayerSession`] control method runs first.
///
/// A thread-local flag rather than a lock: delivery is single-threaded by
/// construction (the platform main thread), and the check has to be cheap
/// enough to sit in front of every entry point.
pub(crate) fn reject_if_delivering() -> Result<(), VideoError> {
    if DELIVERING.with(Cell::get) {
        return Err(VideoError::Reentrant);
    }
    Ok(())
}

/// Restores the delivering flag to whatever it was before this delivery
/// began, however [`Shared::publish`]'s listener call ends — including an
/// unwind, so one panicking listener cannot leave the main thread
/// permanently unable to issue control calls. Restoring rather than
/// unconditionally clearing matters once a nested delivery is possible: this
/// guard is only ever constructed when the flag was clear (a publish that
/// finds it already set queues instead — module doc), so the saved value is
/// always `false` in practice, but clearing unconditionally would be
/// correct only by accident.
///
/// An unwinding listener also drops whatever queued up behind it: a
/// listener that panicked gets no guarantee its queued follow-up events are
/// ever delivered, and delivering them to some unrelated later call would
/// be worse than dropping them.
#[cfg_attr(not(test), allow(dead_code))]
struct DeliveryGuard {
    previous: bool,
}

impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            DEFERRED.with(|queue| queue.borrow_mut().clear());
        }
        DELIVERING.with(|flag| flag.set(self.previous));
    }
}

impl Shared {
    /// A session's state before the backend has published anything: `Idle`,
    /// position zero, nothing else known.
    pub(crate) fn new() -> Self {
        Self {
            duration_ms: AtomicI64::new(UNKNOWN_DURATION),
            ..Self::default()
        }
    }

    /// The current state, consistent enough to render from — see the module
    /// doc on why the fields are read independently.
    pub(crate) fn snapshot(&self) -> PlayerSnapshot {
        let state = PlaybackState::from_code(i32::from(self.state.load(Ordering::Acquire)))
            .unwrap_or(PlaybackState::Idle);
        let duration_ms = self.duration_ms.load(Ordering::Relaxed);
        let width = self.video_width.load(Ordering::Relaxed);
        let height = self.video_height.load(Ordering::Relaxed);

        PlayerSnapshot {
            state,
            position: Duration::from_millis(self.position_ms.load(Ordering::Relaxed)),
            duration: u64::try_from(duration_ms).ok().map(Duration::from_millis),
            video_size: (width > 0 && height > 0).then_some((width, height)),
            // Locked only in the error state (module doc), so the per-frame
            // read stays lock-free.
            error: (state == PlaybackState::Error)
                .then(|| lock(&self.error).clone())
                .flatten(),
        }
    }

    /// Publish one event: update the atomics, then hand it to the listener.
    ///
    /// Called from the platform main thread by every backend's host
    /// callback. The listener is cloned out of its slot and the slot's lock
    /// released **before** the call, so a listener that registers a
    /// replacement (or drops its handle) from inside delivery cannot
    /// deadlock against its own registration.
    ///
    /// If this thread is already inside a delivery — in practice, `close()`
    /// called from a listener, since every backend's close publishes
    /// `StateChanged(Idle)` — the event is not delivered now. It is pushed
    /// onto a thread-local queue and this call returns immediately; the
    /// outer call drains that queue, in the order events were published,
    /// once its own listener call returns. The listener used for a queued
    /// event is the one captured here, at publish time, matching the
    /// clone-before-call rule above — not whatever happens to be registered
    /// by the time the queue drains.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn publish(&self, event: PlayerEvent) {
        match &event {
            PlayerEvent::StateChanged(state) => {
                self.state.store(state.code(), Ordering::Release);
            }
            PlayerEvent::Position { position, duration } => {
                self.position_ms
                    .store(duration_to_ms(*position), Ordering::Relaxed);
                self.duration_ms.store(
                    duration.map_or(UNKNOWN_DURATION, |d| {
                        i64::try_from(duration_to_ms(d)).unwrap_or(i64::MAX)
                    }),
                    Ordering::Relaxed,
                );
            }
            PlayerEvent::VideoSize { width, height } => {
                self.video_width.store(*width, Ordering::Relaxed);
                self.video_height.store(*height, Ordering::Relaxed);
            }
            PlayerEvent::Error(error) => {
                // The error slot is filled before the state flips to
                // `Error`, because `snapshot` reads the slot only *after*
                // seeing that state — the reverse order would let a reader
                // see `Error` with no error to report.
                *lock(&self.error) = Some(error.clone());
                self.state
                    .store(PlaybackState::Error.code(), Ordering::Release);
            }
        }

        let listener = lock(&self.listener).as_ref().map(|(_, l)| Arc::clone(l));
        let Some(listener) = listener else {
            return;
        };

        if DELIVERING.with(Cell::get) {
            DEFERRED.with(|queue| queue.borrow_mut().push_back((listener, event)));
            return;
        }

        let previous = DELIVERING.with(|flag| flag.replace(true));
        let _guard = DeliveryGuard { previous };
        listener(event);

        // Drain whatever queued up while the listener above was running. An
        // event queued while draining is appended to the same queue and
        // picked up by this same loop, so delivery stays exactly publish
        // order and the listener is never on the stack twice.
        while let Some((listener, event)) = DEFERRED.with(|queue| queue.borrow_mut().pop_front()) {
            listener(event);
        }
    }

    /// Install `callback` as this session's only listener, replacing any
    /// previous one, and return the generation tagging the registration.
    pub(crate) fn set_listener(&self, callback: Box<dyn Fn(PlayerEvent) + Send + Sync>) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        *lock(&self.listener) = Some((generation, Arc::from(callback)));
        generation
    }

    /// Unregister the listener registered at `generation`, and no other — a
    /// handle whose registration was already replaced removes nothing.
    pub(crate) fn remove_listener(&self, generation: u64) {
        let mut slot = lock(&self.listener);
        if slot.as_ref().is_some_and(|(g, _)| *g == generation) {
            *slot = None;
        }
    }

    /// Mark the session closed, reporting whether this call is the one that
    /// closed it — what makes [`crate::PlayerSession::close`] idempotent.
    pub(crate) fn close(&self) -> bool {
        !self.closed.swap(true, Ordering::AcqRel)
    }

    /// Whether the session has been closed (by [`crate::PlayerSession::close`]
    /// or by its `Drop`).
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// `duration_ms`' "not known yet" sentinel — see the field's own doc.
const UNKNOWN_DURATION: i64 = -1;

#[cfg_attr(not(test), allow(dead_code))]
/// A [`Duration`] as whole milliseconds, saturating rather than wrapping: a
/// host that reports a nonsense position must not turn into a small one.
fn duration_to_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Lock `mutex`, recovering from poisoning rather than propagating it.
///
/// A panicking listener must not permanently break a session's error slot or
/// listener registration; the data behind both locks is a plain value whose
/// invariants a panic mid-update cannot violate.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
