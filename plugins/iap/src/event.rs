//! The purchase-event listener registry and the plugin-owned thread every
//! event is delivered on: the one place a backend's platform callback (a Play
//! Billing `PurchasesUpdatedListener`, a StoreKit transaction-update task)
//! turns into the app-facing [`crate::Iap::set_purchase_listener`] callbacks.
//!
//! Process-global, like the store connection it reports on — there is one
//! billing client per process, so a per-instance registry would have nothing
//! to hang off. `frust-camera`'s per-session image-stream callback is the
//! contrasting shape: a camera session is a handle an app can hold several of.
//!
//! # One plugin-owned delivery thread, in enqueue order
//!
//! [`emit`] runs no listener at all. It clones the event onto a channel and
//! returns, releasing the platform thread that called in; a single consumer
//! thread (`frust-iap-events`, spawned lazily on the first emit) dequeues and
//! fans each event out. The hop lives here because neither host has one to
//! offer: Play Billing calls its `PurchasesUpdatedListener` on the main thread
//! unless the `BillingClient` was built with a custom executor (openiap-google
//! builds it without one), and OpenIAP's iOS listeners are invoked inside
//! `await MainActor.run { … }`. Both would otherwise land an app's listener on
//! the UI thread — the exact thread the contract promises it is never on.
//!
//! Two properties follow from "one consumer", and both are load-bearing:
//!
//! - **Never the UI thread and never the caller's.** Whatever thread a
//!   platform reports an event on, listeners run on this one.
//! - **Delivery order is enqueue order.** A single FIFO consumer, never a
//!   thread per event — a purchase and the follow-up event that supersedes it
//!   must not race.
//!
//! A listener still owes the non-blocking contract
//! [`crate::Iap::set_purchase_listener`] states: it is holding the one thread
//! every later event queues behind, so a slow listener delays every listener
//! behind it *and* every event behind that.
//!
//! # The queue is bounded, and a full queue drops the newest event
//!
//! [`QUEUE_CAPACITY`] items, not unbounded. Two concrete reasons: the queue
//! holds cloned [`crate::Purchase`]es, whose `purchase_token` is a bearer
//! credential, so an unbounded backlog is an unbounded number of live
//! credentials sitting in memory; and an unbounded queue turns a wedged
//! listener into unbounded growth instead of a bounded, reported loss.
//!
//! **Drop-newest.** An overflowing [`send`] drops the event it was handed and
//! logs its *kind* at error level — never any part of the payload. Drop-newest
//! rather than drop-oldest because it is the only policy `std`'s channel
//! implements without a second lock in front of it, and because it preserves
//! the order already queued: whatever the app has not seen yet still arrives in
//! the order the store reported it. Blocking the caller instead is disqualified
//! outright — that caller is the platform's own callback thread (Android's main
//! thread), and parking it is the ANR this whole hop exists to avoid.
//!
//! A dropped event is a lost purchase outcome, recoverable exactly the way an
//! event delivered with no listener registered is:
//! [`crate::Iap::get_available_purchases`]. Reaching the bound at all takes a
//! listener that blocks for as long as a store needs to report hundreds of
//! events — the misuse [`crate::Iap::set_purchase_listener`] documents, and
//! which the guard below refuses outright for its worst case.
//!
//! # A listener may not re-enter the blocking API — enforced, not just documented
//!
//! Every [`crate::Iap`] call that reaches a store parks its caller. Called from
//! a listener, that caller *is* this one consumer, so a single re-entrant call
//! stalls every later event for as long as the host takes to answer (up to a
//! minute on Android). Both backends therefore run one extra guard ahead of
//! every other: a call made on this thread fails fast with
//! [`crate::IapError::EventThread`] instead of parking — see
//! [`reject_from_delivery_thread`].
//!
//! The refusal covers **every** host round trip, including
//! [`crate::Iap::request_purchase`]'s dispatch-only ack, which still parks the
//! consumer for seconds. Registering or removing a listener is not a round trip
//! and stays allowed from inside a callback — that is the re-entrancy the
//! snapshot-then-call strategy below exists for.
//!
//! The thread is identified by the [`ThreadId`] it publishes when it starts,
//! not by its name: a name check would be fooled by any thread an app happened
//! to name the same, and an id comparison costs nothing.
//!
//! # A recognized event that will not decode becomes a reported failure
//!
//! [`decode_failure`] builds the [`crate::IapEvent::PurchaseError`] a backend
//! emits when its host reports an event kind this crate models but a payload it
//! cannot parse. Dropping that with only a log would silently lose a *completed
//! purchase*: the app learns neither that it happened nor that anything went
//! wrong. An event kind this crate does not model at all is still dropped — it
//! is not this crate's event to report on.
//!
//! The consumer is a plain `std` thread with no JVM attachment of its own,
//! unlike the Play Billing thread it replaces. Nothing regresses from that —
//! every plugin's JNI reach is a scoped per-call attach
//! (`docs/PLUGINS_ARCHITECTURE.md`'s convention), which attaches this thread
//! like any other — but a listener that reaches Android APIs must go through
//! one rather than assume it is already attached.
//!
//! # Lock strategy: snapshot, unlock, then call — and a panicking listener is contained
//!
//! The consumer never holds the registry lock while running a listener. It
//! clones the current listener list (a `Vec` of `Arc`s — cheap) under the
//! lock, drops the guard, and only then calls each one. Two things follow,
//! both load-bearing:
//!
//! - **No re-entrancy deadlock.** A listener that drops a [`ListenerHandle`]
//!   (or registers another listener) re-enters this module and takes the same
//!   lock; with the guard already dropped, that is an ordinary lock
//!   acquisition rather than a self-deadlock on the delivery thread.
//! - **A panicking listener cannot poison the registry.** Each call is wrapped
//!   in [`std::panic::catch_unwind`], so the unwind stops here, is logged, and
//!   the remaining listeners still receive the event. Every lock acquisition
//!   additionally recovers from poisoning
//!   (`unwrap_or_else(PoisonError::into_inner)`), so even a panic raised while
//!   the lock *is* held (there is no such path today) degrades to a warning
//!   rather than making every later call fail.
//! - **A panic anywhere else in a dispatch cannot end the consumer.** The
//!   per-listener guard above only covers the listener call itself, so the
//!   delivery loop wraps the *whole* per-event dispatch in a second
//!   `catch_unwind`: a future panic in the snapshot path — the one thread every
//!   event in this process is delivered on — degrades to a logged warning and
//!   the next event, rather than permanently killing the only consumer. The
//!   loop's test-barrier arm sits outside that guard, so a flush is
//!   acknowledged rather than swallowed.
//!
//! The queue hand-off is what keeps an unwind away from the JNI/ObjC frame
//! that called into Rust — undefined behavior rather than a bug
//! (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule) — since a listener
//! no longer runs anywhere near one.
//!
//! Under a release build's `panic = "abort"` (`docs/DEVELOPMENT.md`'s
//! release-profile hardening) `catch_unwind` catches nothing — the process
//! aborts at the panic site. The containment above is a debug/profile
//! safeguard and a correctness statement about this module's own state, not a
//! promise that a panicking listener is survivable in release.
//!
//! # Who receives a queued event
//!
//! Listeners are snapshotted when an event is **dequeued**, not when it is
//! emitted, and run in registration order. So a listener registered while an
//! event is still in flight may see it, and a [`ListenerHandle`] dropped
//! before that dequeue-snapshot stops it. The snapshot is also the boundary
//! of [`ListenerHandle::remove`]'s promise: a removal landing *after* the
//! snapshot but before this listener's turn does not recall it — the
//! callback runs one last time, so callbacks must tolerate a single
//! invocation racing removal. An event dequeued with nothing registered is
//! **dropped**, never buffered for a later listener: that is exactly the loss
//! [`crate::Iap::request_purchase`] warns about, and recovering it is
//! [`crate::Iap::get_available_purchases`]'s job rather than this queue's.
//!
//! # Shutdown: there is none, deliberately
//!
//! The channel's sender is a process-lifetime static, so the consumer parks in
//! `recv` forever rather than the loop ever ending, and the thread is never
//! joined. One parked thread per process that ever emitted an event is the
//! same deliberate process-lifetime leak the iOS event sink carries (`apple.rs`'s
//! `install_event_sink`): there is no teardown path on either side of this
//! plugin — the store connection itself outlives every `end_connection` — so a
//! join point would be a fiction, and draining at exit would mean running app
//! callbacks during process teardown.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::ThreadId;

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::types::{IapErrorCode, IapPurchaseError};
use crate::{IapError, IapEvent};

/// One registered callback. `Arc`, not `Box`, so [`emit`] can snapshot the
/// list and release the lock before calling anything.
type Listener = Arc<dyn Fn(IapEvent) + Send + Sync>;

/// The listener list plus the id counter [`ListenerHandle`]s are minted from.
struct Registry {
    listeners: Vec<(u64, Listener)>,
    /// Ids start at 1 and are never reused, so a handle for a removed
    /// listener can never unregister a later one.
    next_id: u64,
}

impl Registry {
    const fn new() -> Self {
        Self {
            listeners: Vec::new(),
            next_id: 1,
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// The registry, with poisoning recovered rather than propagated (see the
/// module doc's lock strategy).
fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A registered purchase listener. **Dropping this unregisters the
/// listener** — see [`crate::Iap::set_purchase_listener`], which is the only
/// way to obtain one.
///
/// Deliberately opaque and not `Clone`: exactly one handle owns each
/// registration, so there is never a question of which copy's drop ends it.
#[derive(Debug)]
pub struct ListenerHandle {
    id: u64,
}

impl ListenerHandle {
    /// Unregister the listener now. No event emitted after this returns
    /// reaches it. An event already queued is also stopped **if** removal
    /// lands before the delivery thread snapshots the registry for it; a
    /// fan-out whose snapshot was already taken still invokes this listener
    /// one last time (module doc, *Who receives a queued event*). Callbacks
    /// must therefore tolerate a single invocation racing `remove`/drop —
    /// e.g. never `unwrap` a send to a receiver torn down alongside the
    /// handle.
    ///
    /// The explicit form of dropping the handle — identical in effect, and
    /// worth spelling out at a call site where the drop would otherwise be
    /// invisible (the end of a scope, a struct field being overwritten).
    pub fn remove(self) {
        // `Drop` does the unregistration; consuming `self` here is what makes
        // the intent readable, and guarantees the handle can't be used after.
        drop(self);
    }
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        registry().listeners.retain(|(id, _)| *id != self.id);
    }
}

/// Register `callback` and return the handle that keeps it registered — see
/// [`crate::Iap::set_purchase_listener`] for the public contract.
pub(crate) fn register(callback: Box<dyn Fn(IapEvent) + Send + Sync>) -> ListenerHandle {
    let mut registry = registry();
    let id = registry.next_id;
    registry.next_id += 1;
    registry.listeners.push((id, Arc::from(callback)));
    ListenerHandle { id }
}

// --- The delivery thread ----------------------------------------------------
//
// Every item below carries `cfg_attr(not(test), allow(dead_code))` for one
// reason: on a **desktop** build nothing emits. The only callers are the two
// mobile backends (compiled only for their own targets) and the `cfg(test)`
// fake store, and `desktop`'s always-unavailable arm reaches no store at all,
// so a plain `cargo build` on a desktop host sees this whole path as
// unreachable — the same shape `frust-camera` uses for its Android-only
// completion-correlation module. The allowance on `emit` alone would not
// cover the rest: a dead-code allowance is not transitive to what an item
// calls.

/// How many undelivered items the queue holds before it starts dropping — see
/// the module doc's bound-and-drop-policy section.
///
/// **Not a platform value.** Generous rather than tuned: a store reports
/// purchase events at human pace (a purchase sheet, a restore pass), so a
/// backlog this deep already means a listener has been blocking for a very long
/// time. Small enough that the cloned bearer credentials it can hold stay a
/// bounded set, large enough that no honest listener ever reaches it.
const QUEUE_CAPACITY: usize = 256;

/// One item on the delivery thread's queue.
#[cfg_attr(not(test), allow(dead_code))]
enum Delivery {
    /// Fan this event out to whatever is registered when it is dequeued.
    Event(IapEvent),
    /// A test-only barrier: acknowledged once everything queued ahead of it
    /// has been fanned out — see [`flush`].
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
}

impl Delivery {
    /// The item's kind, as [`IapEvent`]'s own envelope tag — the **only** thing
    /// about a dropped item that may reach a log.
    ///
    /// A `Purchase`'s `purchase_token` is a bearer credential (see
    /// [`crate::Purchase`]'s doc), and it rides inside the payload of exactly
    /// the event a full queue drops, so the log line names the kind and nothing
    /// else.
    #[cfg_attr(not(test), allow(dead_code))]
    fn kind(&self) -> &'static str {
        match self {
            Self::Event(IapEvent::PurchaseUpdated(_)) => "purchaseUpdated",
            Self::Event(IapEvent::PurchaseError(_)) => "purchaseError",
            #[cfg(test)]
            Self::Barrier(_) => "barrier",
        }
    }
}

/// The delivery thread's queue, created together with the thread on first use.
#[cfg_attr(not(test), allow(dead_code))]
static QUEUE: OnceLock<SyncSender<Delivery>> = OnceLock::new();

/// The delivery thread's own id, published by that thread before it dequeues
/// anything — what [`reject_from_delivery_thread`] compares against.
///
/// An id rather than the thread's name: a [`ThreadId`] is unique for the life
/// of the process, so the comparison cannot be fooled by an app thread that
/// happens to be named `frust-iap-events`, and it costs no string compare.
#[cfg_attr(not(test), allow(dead_code))]
static DELIVERY_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Refuse a call that would park the delivery thread on a host round trip —
/// guard-order step 0 in both backends (see [`crate::Backend`]).
///
/// A plain id comparison: no lock, no platform call, and `None` until the
/// thread exists at all, so it is cheap enough to sit in front of every entry
/// point and holds under `panic = "abort"` like the readiness flag beside it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn reject_from_delivery_thread() -> Result<(), IapError> {
    if DELIVERY_THREAD.get() == Some(&std::thread::current().id()) {
        return Err(IapError::EventThread);
    }
    Ok(())
}

/// The queue, spawning the delivery thread the first time anything emits.
///
/// Lazy rather than eager so a process that never touches IAP never pays for
/// a thread; `OnceLock` so two platform callback threads racing here still
/// produce exactly one consumer.
#[cfg_attr(not(test), allow(dead_code))]
fn queue() -> &'static SyncSender<Delivery> {
    QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Delivery>(QUEUE_CAPACITY);
        let spawned = std::thread::Builder::new()
            .name("frust-iap-events".to_owned())
            // Never joined, and `receiver` outlives every send because
            // `sender` is a static — see the module doc's shutdown note.
            .spawn(move || {
                // Publish the tag before dequeuing anything, so the guard every
                // backend runs first sees it from the very first event.
                let _ = DELIVERY_THREAD.set(std::thread::current().id());
                for delivery in receiver {
                    match delivery {
                        // The whole dispatch, not just the listener call inside
                        // it: an unwind escaping here would end the only
                        // consumer this process has (module doc's lock
                        // strategy).
                        Delivery::Event(event) => {
                            if catch_unwind(AssertUnwindSafe(|| fan_out(&event))).is_err() {
                                log::warn!(
                                    "frust-iap: the purchase-event delivery thread caught a panic \
                                     while dispatching an event; that event may not have reached \
                                     every listener, and delivery continues"
                                );
                            }
                        }
                        // Deliberately outside the guard above: a swallowed
                        // barrier would leave a test flush waiting forever.
                        #[cfg(test)]
                        Delivery::Barrier(ack) => drop(ack.send(())),
                    }
                }
            });
        if let Err(error) = spawned {
            // The OS refused a thread. Report it once, here, rather than
            // panicking on a platform callback thread; every later `send` then
            // fails against the dropped receiver and logs its own dropped
            // event.
            log::error!(
                "frust-iap: could not spawn the purchase-event delivery thread ({error}); \
                 purchase events cannot be delivered"
            );
        }
        sender
    })
}

/// Hand `event` to the delivery thread — the seam every backend's platform
/// callback calls, from whatever thread the platform reports on.
///
/// Returns as soon as the event is queued — and never parks the caller: an
/// event that does not fit the bound is dropped and logged rather than made to
/// wait, since this caller is the platform's own callback thread (the module
/// doc's drop policy). See the module doc for the ordering, snapshot and
/// panic-containment contract this buys, and
/// [`crate::Iap::set_purchase_listener`] for what the listeners themselves owe.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn emit(event: &IapEvent) {
    send(Delivery::Event(event.clone()));
}

/// The event a backend emits in place of one its host reported under a kind
/// this crate models but with a payload it could not decode — the module doc's
/// decode-failure section.
///
/// `detail` is the decoder's own diagnosis; the payload itself is never
/// carried, since a `purchase-updated` payload holds a bearer credential. This
/// event is also where that diagnosis is reported at all — a backend's log line
/// names only the kind, because serde quotes the value it rejected and that
/// value comes out of the payload. `product_id` is `None` rather than guessed:
/// the field it would come from is part of what failed to parse.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn decode_failure(kind: &str, detail: &str) -> IapEvent {
    IapEvent::PurchaseError(IapPurchaseError {
        code: IapErrorCode::BillingResponseJsonParseError,
        message: format!("a `{kind}` event from the platform host could not be decoded: {detail}"),
        product_id: None,
    })
}

/// Queue one item; `true` when it was queued.
///
/// Never blocks and never panics: a full queue drops the newest item and logs
/// its kind (module doc's drop policy), and a delivery thread that never
/// started (the OS refused the spawn above) fails every send from then on.
#[cfg_attr(not(test), allow(dead_code))]
fn send(delivery: Delivery) -> bool {
    match queue().try_send(delivery) {
        Ok(()) => true,
        Err(TrySendError::Full(dropped)) => {
            // The kind only — never the payload (see `Delivery::kind`).
            log::error!(
                "frust-iap: the purchase-event queue is full ({QUEUE_CAPACITY} undelivered \
                 events); a `{}` event was dropped. A purchase listener is blocking the delivery \
                 thread; recover the lost outcome with `Iap::get_available_purchases`",
                dropped.kind()
            );
            false
        }
        Err(TrySendError::Disconnected(_)) => {
            log::warn!(
                "frust-iap: the purchase-event delivery thread is not running; an event was dropped"
            );
            false
        }
    }
}

/// Run every registered listener against `event`, on the delivery thread.
///
/// Snapshot under the lock, call outside it — see the module doc.
#[cfg_attr(not(test), allow(dead_code))]
fn fan_out(event: &IapEvent) {
    // The injected failure the delivery loop's own `catch_unwind` is proved
    // against: nothing here panics in production, and a test is how "if that
    // ever changes, the consumer survives it" stays checked rather than
    // claimed.
    #[cfg(test)]
    if PANIC_ON_NEXT_DISPATCH.swap(false, Ordering::SeqCst) {
        panic!("frust-iap test: injected dispatch panic");
    }

    let listeners: Vec<Listener> = registry()
        .listeners
        .iter()
        .map(|(_, listener)| Arc::clone(listener))
        .collect();

    for listener in listeners {
        let event = event.clone();
        if catch_unwind(AssertUnwindSafe(move || listener(event))).is_err() {
            log::warn!(
                "frust-iap: a purchase listener panicked; the event was still delivered to the \
                 remaining listeners"
            );
        }
    }
}

/// Block until every event queued so far has been delivered.
///
/// Test-only, and the reason delivery stays assertable now that [`emit`] is
/// asynchronous: a barrier behind the queued events is acknowledged only after
/// the consumer has fanned all of them out, so "the listener saw it" and
/// "nothing was delivered" both stay exact rather than becoming sleeps.
///
/// Unlike an event, a barrier is **blocked on rather than dropped** when the
/// queue is full: a barrier the bound discarded would report "everything has
/// been delivered" about a queue that had not drained, turning every assertion
/// behind it into a race. It therefore uses the blocking send rather than
/// [`send`]'s `try_send`, which parks only while the queue is full — a state
/// only a deliberately parked consumer produces, and the one test that produces
/// it releases the consumer before flushing.
///
/// Never hangs when there is no consumer: a disconnected send fails
/// immediately rather than parking. Calling it from inside a listener would
/// deadlock (the consumer would be waiting on itself); nothing does.
#[cfg(test)]
pub(crate) fn flush() {
    let (ack, acked) = mpsc::channel();
    if queue().send(Delivery::Barrier(ack)).is_err() {
        return;
    }
    let _ = acked.recv();
}

/// Make the next dispatch on the delivery thread panic once, before any
/// listener runs — see [`fan_out`].
#[cfg(test)]
static PANIC_ON_NEXT_DISPATCH: AtomicBool = AtomicBool::new(false);

/// See [`PANIC_ON_NEXT_DISPATCH`].
#[cfg(test)]
fn panic_on_next_dispatch() {
    PANIC_ON_NEXT_DISPATCH.store(true, Ordering::SeqCst);
}

/// Serializes every test that touches the process-global registry, and hands
/// back a cleared one.
///
/// The registry is process-global by design, so two `#[test]`s registering
/// listeners in parallel would see each other's events. Every test in this
/// crate that registers, removes, or emits takes this guard first; holding it
/// for the whole test body is what makes "no delivery after removal" and
/// "nothing was delivered yet" deterministic assertions rather than races.
///
/// The delivery thread is process-global too, so the guard also drains it:
/// an event a previous test left in flight is delivered *before* this test's
/// listeners exist, rather than landing in the middle of its assertions.
///
/// Poisoning is recovered rather than propagated: one failing test must not
/// cascade into every later one.
#[cfg(test)]
pub(crate) fn test_guard() -> TestGuard {
    let guard = TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    flush();
    // A test that panicked mid-body may have left listeners behind; start
    // every test from an empty registry rather than inheriting them.
    registry().listeners.clear();
    // …and must not leave an armed dispatch panic for the next test either.
    PANIC_ON_NEXT_DISPATCH.store(false, Ordering::SeqCst);
    TestGuard(guard)
}

#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// [`test_guard`]'s RAII token — held for a test's whole body.
#[cfg(test)]
pub(crate) struct TestGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::*;
    use crate::types::{IapErrorCode, IapPurchaseError};

    /// A distinguishable event carrying `label` as its product id.
    fn event(label: &str) -> IapEvent {
        IapEvent::PurchaseError(IapPurchaseError {
            code: IapErrorCode::UserCancelled,
            message: "test".into(),
            product_id: Some(label.into()),
        })
    }

    /// The product id of a `PurchaseError` event, for assertions.
    fn label_of(event: &IapEvent) -> String {
        match event {
            IapEvent::PurchaseError(error) => error.product_id.clone().unwrap_or_default(),
            IapEvent::PurchaseUpdated(purchase) => purchase.product_id.clone(),
        }
    }

    /// Emit and wait for the delivery thread to finish with it — the pairing
    /// every assertion below needs, since [`emit`] itself only queues.
    fn deliver(event: &IapEvent) {
        emit(event);
        flush();
    }

    #[test]
    fn a_registered_listener_receives_an_emitted_event() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handle = register(Box::new(move |event| tx.send(label_of(&event)).unwrap()));
        deliver(&event("alpha"));

        assert_eq!(rx.try_recv().unwrap(), "alpha");
        handle.remove();
    }

    #[test]
    fn nothing_is_delivered_after_the_handle_is_removed() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handle = register(Box::new(move |event| tx.send(label_of(&event)).unwrap()));
        handle.remove();
        deliver(&event("alpha"));

        assert!(
            rx.try_recv().is_err(),
            "a removed listener must never see a later event"
        );
    }

    /// Dropping the handle is the same unregistration `remove` performs — the
    /// footgun `set_purchase_listener` documents, pinned as behavior.
    #[test]
    fn dropping_the_handle_unregisters_just_like_remove() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        drop(register(Box::new(move |event| {
            tx.send(label_of(&event)).unwrap();
        })));
        deliver(&event("alpha"));

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn every_listener_sees_every_event_in_registration_order() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handles: Vec<_> = (0..8)
            .map(|index| {
                let tx = tx.clone();
                register(Box::new(move |event| {
                    tx.send((index, label_of(&event))).unwrap();
                }))
            })
            .collect();
        drop(tx);

        deliver(&event("alpha"));
        let received: Vec<_> = rx.try_iter().collect();

        assert_eq!(received.len(), 8, "all eight listeners were called");
        for (position, (index, label)) in received.iter().enumerate() {
            assert_eq!(*index, position, "registration order is delivery order");
            assert_eq!(label, "alpha");
        }
        drop(handles);
    }

    /// Removing one listener leaves the others registered — the id-keyed
    /// removal, not a blanket clear.
    #[test]
    fn removing_one_listener_leaves_the_others() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let first = {
            let tx = tx.clone();
            register(Box::new(move |_| tx.send("first").unwrap()))
        };
        let second = register(Box::new(move |_| tx.send("second").unwrap()));

        first.remove();
        deliver(&event("alpha"));

        assert_eq!(rx.try_recv().unwrap(), "second");
        assert!(
            rx.try_recv().is_err(),
            "the removed listener stayed removed"
        );
        second.remove();
    }

    /// A panicking listener is contained: the registry survives, the
    /// listeners after it still run, and a later emit still works. (Under
    /// `panic = "abort"` this test's premise cannot arise at all — see the
    /// module doc.)
    #[test]
    fn a_panicking_listener_does_not_poison_the_registry() {
        let _guard = test_guard();
        let calls = Arc::new(AtomicUsize::new(0));

        let panicking = register(Box::new(|_| panic!("listener panicked on purpose")));
        let counting = {
            let calls = Arc::clone(&calls);
            register(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };

        deliver(&event("alpha"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the listener behind the panicking one still ran"
        );

        deliver(&event("beta"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the registry still works after a panic"
        );

        panicking.remove();
        counting.remove();
    }

    /// Emitting with nothing registered is a no-op, not a failure — the state
    /// a backend is in before the app registers anything. The event is
    /// dropped at dequeue rather than buffered for a later listener.
    #[test]
    fn emitting_with_no_listeners_is_a_no_op() {
        let _guard = test_guard();
        deliver(&event("alpha"));

        let (tx, rx) = mpsc::channel();
        let handle = register(Box::new(move |event| tx.send(label_of(&event)).unwrap()));
        flush();
        assert!(
            rx.try_recv().is_err(),
            "a listener registered afterwards never receives an already-delivered event"
        );
        handle.remove();
    }

    /// A listener that unregisters itself from inside its own callback must
    /// not deadlock — the re-entrancy the snapshot-then-call strategy exists
    /// for.
    #[test]
    fn a_listener_may_drop_a_handle_from_inside_the_callback() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let victim = register(Box::new(|_| {}));
        let victim = Mutex::new(Some(victim));
        let dropper = register(Box::new(move |_| {
            // Drops the other handle — re-enters the registry lock.
            drop(victim.lock().unwrap().take());
            tx.send(()).unwrap();
        }));

        deliver(&event("alpha"));
        assert!(rx.try_recv().is_ok(), "the callback completed, no deadlock");
        dropper.remove();
    }

    /// The delivery contract itself: events emitted from two *different*
    /// caller threads are delivered in enqueue order, on one consistent
    /// thread that is neither caller's nor the test's.
    ///
    /// The spawns are joined one after the other so enqueue order is exact
    /// rather than a race — what is under test is the consumer's ordering,
    /// not the channel's tie-breaking between two live senders.
    #[test]
    fn events_are_delivered_in_enqueue_order_on_one_plugin_owned_thread() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handle = register(Box::new(move |event| {
            tx.send((std::thread::current().id(), label_of(&event)))
                .unwrap();
        }));

        let first_caller = std::thread::spawn(|| {
            emit(&event("alpha"));
            std::thread::current().id()
        })
        .join()
        .unwrap();
        let second_caller = std::thread::spawn(|| {
            emit(&event("beta"));
            std::thread::current().id()
        })
        .join()
        .unwrap();
        flush();

        let received: Vec<_> = rx.try_iter().collect();
        let labels: Vec<&str> = received.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(labels, ["alpha", "beta"], "enqueue order is delivery order");

        let delivering = received[0].0;
        assert_eq!(
            received[1].0, delivering,
            "one delivery thread for every event, never a thread per event"
        );
        assert_ne!(delivering, first_caller, "never the emitting thread");
        assert_ne!(delivering, second_caller, "never the emitting thread");
        assert_ne!(
            delivering,
            std::thread::current().id(),
            "never the thread under test"
        );

        handle.remove();
    }

    /// One emit is one delivery per listener — the property an emitting path
    /// that also re-reports its own failure (a host catch re-emitting what the
    /// store already published) would break, pinned here at the seam every
    /// such path funnels through.
    #[test]
    fn one_emit_delivers_exactly_once_to_each_listener() {
        let _guard = test_guard();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));

        let first = {
            let calls = Arc::clone(&first_calls);
            register(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };
        let second = {
            let calls = Arc::clone(&second_calls);
            register(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };

        deliver(&event("alpha"));

        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);

        // A second emit is a second delivery — one per emit, no coalescing
        // either.
        deliver(&event("alpha"));
        assert_eq!(first_calls.load(Ordering::SeqCst), 2);
        assert_eq!(second_calls.load(Ordering::SeqCst), 2);

        first.remove();
        second.remove();
    }

    /// The bound and its drop policy (module doc): with the one consumer parked
    /// inside a listener, exactly [`QUEUE_CAPACITY`] events fit behind it, every
    /// later one is dropped, and what *was* queued still arrives in order —
    /// drop-newest, not drop-oldest and not a blocked emitter.
    #[test]
    fn a_full_queue_drops_the_newest_event_and_keeps_the_queued_ones() {
        let _guard = test_guard();
        let (unpark, parked) = mpsc::channel::<()>();
        let (entered, listener_entered) = mpsc::channel::<()>();
        let (tx, rx) = mpsc::channel::<String>();

        // The first delivery parks the delivery thread inside the listener;
        // everything emitted from then on has to fit in the bound or be
        // dropped.
        let gate = Mutex::new(Some(parked));
        let handle = register(Box::new(move |event| {
            if let Some(parked) = gate.lock().unwrap().take() {
                entered.send(()).unwrap();
                parked.recv().unwrap();
            }
            tx.send(label_of(&event)).unwrap();
        }));

        emit(&event("parker"));
        listener_entered
            .recv()
            .expect("the consumer is now inside the listener");

        for index in 0..QUEUE_CAPACITY {
            assert!(
                send(Delivery::Event(event(&format!("queued-{index}")))),
                "event {index} fits inside the bound"
            );
        }
        for index in 0..4 {
            assert!(
                !send(Delivery::Event(event(&format!("overflow-{index}")))),
                "the bound is enforced rather than the queue growing"
            );
        }

        unpark.send(()).unwrap();
        flush();

        let delivered: Vec<String> = rx.try_iter().collect();
        assert_eq!(
            delivered.len(),
            QUEUE_CAPACITY + 1,
            "the parked event plus everything that fit"
        );
        assert_eq!(delivered[0], "parker");
        assert_eq!(delivered[1], "queued-0", "queued order is preserved");
        assert_eq!(
            delivered[QUEUE_CAPACITY],
            format!("queued-{}", QUEUE_CAPACITY - 1)
        );
        assert!(
            !delivered.iter().any(|label| label.starts_with("overflow")),
            "the dropped events are the newest ones, never the queued ones"
        );

        handle.remove();
    }

    /// A panic in the dispatch path itself — outside the per-listener guard —
    /// costs that one event and nothing else: the consumer survives and the
    /// next event is delivered normally. (Under `panic = "abort"` this test's
    /// premise cannot arise at all — see the module doc.)
    #[test]
    fn the_consumer_survives_a_panic_in_the_dispatch_path() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handle = register(Box::new(move |event| tx.send(label_of(&event)).unwrap()));

        panic_on_next_dispatch();
        deliver(&event("alpha"));
        assert!(
            rx.try_recv().is_err(),
            "the dispatch that panicked delivered nothing"
        );

        deliver(&event("beta"));
        assert_eq!(
            rx.try_recv().unwrap(),
            "beta",
            "the one consumer is still running"
        );

        handle.remove();
    }

    /// The re-entrancy guard both backends run first: the check refuses on the
    /// delivery thread and permits everywhere else. A listener re-entering a
    /// blocking call is what it exists for — it would park the one consumer for
    /// up to a store timeout.
    #[test]
    fn a_blocking_call_is_refused_on_the_delivery_thread_and_permitted_off_it() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        assert!(
            reject_from_delivery_thread().is_ok(),
            "an ordinary thread is not refused"
        );

        let handle = register(Box::new(move |_| {
            tx.send(reject_from_delivery_thread()).unwrap();
        }));
        deliver(&event("alpha"));

        let refused = rx.try_recv().expect("the listener ran");
        assert!(
            matches!(refused, Err(IapError::EventThread)),
            "a listener's own thread is refused: {refused:?}"
        );

        handle.remove();
    }

    /// The synthesized decode failure a backend emits instead of dropping a
    /// completed purchase: a typed, spec-coded event carrying the kind and the
    /// decoder's diagnosis, never the payload.
    #[test]
    fn a_decode_failure_is_a_typed_parse_error_event() {
        let IapEvent::PurchaseError(error) =
            decode_failure("purchase-updated", "missing field `productId`")
        else {
            panic!("a decode failure is always a purchase-error event");
        };
        assert_eq!(error.code, IapErrorCode::BillingResponseJsonParseError);
        assert_eq!(
            error.product_id, None,
            "the id would have come from the payload that failed to parse"
        );
        assert!(error.message.contains("purchase-updated"), "{error:?}");
        assert!(error.message.contains("missing field"), "{error:?}");
    }
}
