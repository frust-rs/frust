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
//!   the remaining listeners still receive the event — and it never reaches
//!   the delivery loop, which would otherwise end the only thread events are
//!   delivered on. Every lock acquisition additionally recovers from poisoning
//!   (`unwrap_or_else(PoisonError::into_inner)`), so even a panic raised while
//!   the lock *is* held (there is no such path today) degrades to a warning
//!   rather than making every later call fail.
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
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::IapEvent;

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

/// One item on the delivery thread's queue.
#[cfg_attr(not(test), allow(dead_code))]
enum Delivery {
    /// Fan this event out to whatever is registered when it is dequeued.
    Event(IapEvent),
    /// A test-only barrier: acknowledged once everything queued ahead of it
    /// has been fanned out — see [`flush`].
    #[cfg(test)]
    Barrier(Sender<()>),
}

/// The delivery thread's queue, created together with the thread on first use.
#[cfg_attr(not(test), allow(dead_code))]
static QUEUE: OnceLock<Sender<Delivery>> = OnceLock::new();

/// The queue, spawning the delivery thread the first time anything emits.
///
/// Lazy rather than eager so a process that never touches IAP never pays for
/// a thread; `OnceLock` so two platform callback threads racing here still
/// produce exactly one consumer.
#[cfg_attr(not(test), allow(dead_code))]
fn queue() -> &'static Sender<Delivery> {
    QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Delivery>();
        let spawned = std::thread::Builder::new()
            .name("frust-iap-events".to_owned())
            // Never joined, and `receiver` outlives every send because
            // `sender` is a static — see the module doc's shutdown note.
            .spawn(move || {
                for delivery in receiver {
                    match delivery {
                        Delivery::Event(event) => fan_out(&event),
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
/// Returns as soon as the event is queued: no listener runs on the caller's
/// thread. See the module doc for the ordering, snapshot and panic-containment
/// contract this buys, and [`crate::Iap::set_purchase_listener`] for what the
/// listeners themselves owe.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn emit(event: &IapEvent) {
    send(Delivery::Event(event.clone()));
}

/// Queue one item, logging rather than panicking if the delivery thread is
/// gone (it never starts if the OS refused the spawn above).
#[cfg_attr(not(test), allow(dead_code))]
fn send(delivery: Delivery) {
    if queue().send(delivery).is_err() {
        log::warn!(
            "frust-iap: the purchase-event delivery thread is not running; an event was dropped"
        );
    }
}

/// Run every registered listener against `event`, on the delivery thread.
///
/// Snapshot under the lock, call outside it — see the module doc.
#[cfg_attr(not(test), allow(dead_code))]
fn fan_out(event: &IapEvent) {
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
/// Never hangs when there is no consumer: the barrier's own sender is dropped
/// with the failed [`send`], which ends the wait immediately. Calling it from
/// inside a listener would deadlock (the consumer would be waiting on itself);
/// nothing does.
#[cfg(test)]
pub(crate) fn flush() {
    let (ack, acked) = mpsc::channel();
    send(Delivery::Barrier(ack));
    let _ = acked.recv();
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
}
