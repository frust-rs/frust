//! The listener registry and the plugin-owned thread every
//! [`ConnectivityEvent`] is delivered on — the same shape as `frust-iap`'s
//! purchase-event path.
//!
//! Process-global, like the one platform monitor it reports on: there is one
//! default network per process, so a per-instance registry would have nothing
//! to hang off.
//!
//! # One plugin-owned delivery thread, in enqueue order
//!
//! [`emit`] runs no listener. It copies the event onto a bounded channel and
//! returns, releasing the platform thread that called in (a
//! `ConnectivityManager` callback thread, a Network.framework dispatch
//! queue). A single consumer thread, `frust-connectivity-events`, spawned
//! lazily on the first emit, dequeues and fans each event out:
//!
//! - **Never the UI thread and never the caller's.** Whatever thread a
//!   platform reports on, listeners run on this one.
//! - **A listener sees only events emitted after it registered.** Every
//!   registration takes the next value of a monotonically increasing
//!   sequence, and [`emit`] stamps each queued event with the sequence that
//!   is current at that moment. Delivery skips any listener whose sequence
//!   is not below the stamp, so a listener registered while an earlier event
//!   still waits in the queue never receives it.
//! - **Delivery order is enqueue order**, and every listener sees an event in
//!   registration order. A `Lost` and the `Available` that follows it must
//!   never race, so there is one FIFO consumer, never a thread per event.
//!
//! # A full queue drops the newest event
//!
//! The channel holds [`QUEUE_CAPACITY`] undelivered events. A listener that
//! blocks the consumer long enough to fill it costs events, not memory: the
//! event that does not fit is dropped and logged by kind only. The state
//! [`crate::Connectivity::current`] reads is updated before the enqueue, so it
//! stays correct even when an event is lost.
//!
//! # Lock strategy
//!
//! The registry lock is held only to register, unregister, stamp an emit, or
//! snapshot the listener list — never while a listener runs. A listener may therefore
//! register another listener or drop a handle (its own included) from inside
//! its callback. A listener panic is caught and logged so the one consumer
//! keeps running; the registry lock recovers from poisoning the same way.
//!
//! # Shutdown
//!
//! The consumer is never joined. The sender is a static, so the receiver it
//! owns lives for the whole process and the thread simply ends with it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::ConnectivityEvent;

type Listener = Arc<dyn Fn(ConnectivityEvent) + Send + Sync>;

/// One registered listener: its registration sequence and the callback.
type Entry = (u64, Listener);

struct Registry {
    listeners: Vec<Entry>,
    /// The sequence the next registration takes; also the stamp an emitted
    /// event carries, since every listener registered so far holds a lower one.
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

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keeps a listener registered; **dropping it unregisters the listener**.
///
/// `let _ = Connectivity::set_listener(cb);` therefore unregisters on the very
/// next statement. Store the handle for as long as the listener should live.
/// Dropping it from inside a callback is supported.
///
/// A drop takes effect for events not yet fanned out. A fan-out already in
/// flight holds its own snapshot of the listeners, so the dropped listener may
/// still be invoked once more for the event being delivered at that moment.
#[must_use = "dropping the handle unregisters the listener immediately"]
#[derive(Debug)]
pub struct ListenerHandle {
    id: u64,
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        registry().listeners.retain(|(id, _)| *id != self.id);
    }
}

pub(crate) fn register(listener: Box<dyn Fn(ConnectivityEvent) + Send + Sync>) -> ListenerHandle {
    let mut registry = registry();
    let id = registry.next_id;
    registry.next_id += 1;
    registry.listeners.push((id, Arc::from(listener)));
    ListenerHandle { id }
}

// --- The delivery thread ----------------------------------------------------
//
// Items below carry `cfg_attr(not(test), allow(dead_code))` because a no-op
// build (Linux, Windows, web) never reports, so nothing reaches `emit` outside
// tests. A dead-code allowance is not transitive, hence one per item.

/// Undelivered events the queue holds before the newest is dropped.
#[cfg_attr(not(test), allow(dead_code))]
const QUEUE_CAPACITY: usize = 64;

/// The thread every listener runs on.
#[cfg_attr(not(test), allow(dead_code))]
const THREAD_NAME: &str = "frust-connectivity-events";

#[cfg_attr(not(test), allow(dead_code))]
enum Delivery {
    /// An event with the registration sequence current when it was emitted:
    /// only listeners registered below it receive the event.
    Event(ConnectivityEvent, u64),
    #[cfg(test)]
    Barrier(mpsc::Sender<()>),
}

impl Delivery {
    /// The event's kind, which is all a drop log names: the path itself is
    /// not logged.
    #[cfg_attr(not(test), allow(dead_code))]
    fn kind(&self) -> &'static str {
        match self {
            Self::Event(ConnectivityEvent::Available { .. }, _) => "available",
            Self::Event(ConnectivityEvent::Lost, _) => "lost",
            #[cfg(test)]
            Self::Barrier(_) => "barrier",
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
static QUEUE: OnceLock<SyncSender<Delivery>> = OnceLock::new();

#[cfg_attr(not(test), allow(dead_code))]
fn queue() -> &'static SyncSender<Delivery> {
    QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Delivery>(QUEUE_CAPACITY);
        let spawned = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                for delivery in receiver {
                    match delivery {
                        Delivery::Event(event, stamp) => {
                            if catch_unwind(AssertUnwindSafe(|| fan_out(event, stamp))).is_err() {
                                log::warn!(
                                    "frust-connectivity: the delivery thread caught a panic while \
                                     dispatching an event; delivery continues"
                                );
                            }
                        }
                        // Outside the guard: a swallowed barrier would leave a
                        // test flush waiting forever.
                        #[cfg(test)]
                        Delivery::Barrier(ack) => drop(ack.send(())),
                    }
                }
            });
        if let Err(error) = spawned {
            // Reported once rather than panicking on a platform callback
            // thread; every later send fails against the dropped receiver.
            log::error!(
                "frust-connectivity: could not spawn the event delivery thread ({error}); \
                 connectivity events cannot be delivered"
            );
        }
        sender
    })
}

/// Queue `event` for every registered listener, without blocking.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn emit(event: ConnectivityEvent) {
    let stamp = registry().next_id;
    send(Delivery::Event(event, stamp));
}

#[cfg_attr(not(test), allow(dead_code))]
fn send(delivery: Delivery) -> bool {
    match queue().try_send(delivery) {
        Ok(()) => true,
        Err(TrySendError::Full(dropped)) => {
            log::warn!(
                "frust-connectivity: the event queue is full ({QUEUE_CAPACITY} undelivered \
                 events); a `{}` event was dropped. A listener is blocking the delivery thread; \
                 `Connectivity::current` still reports the latest state",
                dropped.kind()
            );
            false
        }
        Err(TrySendError::Disconnected(dropped)) => {
            log::warn!(
                "frust-connectivity: the delivery thread is not running; a `{}` event was dropped",
                dropped.kind()
            );
            false
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn fan_out(event: ConnectivityEvent, stamp: u64) {
    let listeners: Vec<Listener> = registry()
        .listeners
        .iter()
        .filter(|(id, _)| *id < stamp)
        .map(|(_, listener)| Arc::clone(listener))
        .collect();
    for listener in listeners {
        if catch_unwind(AssertUnwindSafe(|| listener(event))).is_err() {
            log::warn!(
                "frust-connectivity: a listener panicked; the event was still delivered to the \
                 remaining listeners"
            );
        }
    }
}

/// Block until every event queued so far has been delivered.
#[cfg(test)]
pub(crate) fn flush() {
    let (ack, acked) = mpsc::channel();
    if queue().send(Delivery::Barrier(ack)).is_err() {
        return;
    }
    let _ = acked.recv();
}

/// Drop every registered listener, so a test that panicked mid-body cannot
/// leave one behind for the next.
#[cfg(test)]
pub(crate) fn clear_listeners() {
    registry().listeners.clear();
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::test_guard;

    fn deliver(event: ConnectivityEvent) {
        emit(event);
        flush();
    }

    #[test]
    fn a_registered_listener_receives_an_emitted_event() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let _handle = register(Box::new(move |event| tx.send(event).unwrap()));
        deliver(ConnectivityEvent::Lost);
        assert_eq!(rx.try_recv().unwrap(), ConnectivityEvent::Lost);
    }

    /// Park the delivery thread inside a first listener, run `during` while
    /// events are queued behind it, then release and flush.
    fn with_parked_consumer(during: impl FnOnce()) {
        let parked = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let once = std::sync::Once::new();
        let _parker = {
            let parked = Arc::clone(&parked);
            let release = Arc::clone(&release);
            register(Box::new(move |_| {
                once.call_once(|| {
                    parked.wait();
                    release.wait();
                });
            }))
        };
        emit(ConnectivityEvent::Lost);
        parked.wait();
        during();
        release.wait();
        flush();
    }

    #[test]
    fn a_listener_registered_after_an_emit_does_not_receive_that_event() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let mut late = None;
        with_parked_consumer(|| {
            emit(ConnectivityEvent::Lost);
            late = Some(register(Box::new(move |event| tx.send(event).unwrap())));
        });
        assert!(
            rx.try_recv().is_err(),
            "an event emitted before registration is never delivered to it"
        );
        deliver(ConnectivityEvent::Lost);
        assert_eq!(rx.try_recv().unwrap(), ConnectivityEvent::Lost);
        drop(late);
    }

    #[test]
    fn a_listener_registered_before_an_emit_receives_it_while_queued() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let early = register(Box::new(move |event| tx.send(event).unwrap()));
        with_parked_consumer(|| emit(ConnectivityEvent::Lost));
        let received: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            received.len(),
            2,
            "both the parked event and the queued one reach it"
        );
        drop(early);
    }

    #[test]
    fn dropping_the_handle_unregisters_the_listener() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        drop(register(Box::new(move |event| tx.send(event).unwrap())));
        deliver(ConnectivityEvent::Lost);
        assert!(
            rx.try_recv().is_err(),
            "a dropped handle's listener never sees a later event"
        );
    }

    #[test]
    fn every_listener_sees_every_event_in_registration_order() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let _handles: Vec<_> = (0..8)
            .map(|index| {
                let tx = tx.clone();
                register(Box::new(move |_| tx.send(index).unwrap()))
            })
            .collect();
        drop(tx);
        deliver(ConnectivityEvent::Lost);
        let received: Vec<usize> = rx.try_iter().collect();
        assert_eq!(received, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn a_panicking_listener_does_not_stop_the_others_or_the_thread() {
        let _guard = test_guard();
        let calls = Arc::new(AtomicUsize::new(0));
        let _panicking = register(Box::new(|_| panic!("listener panicked on purpose")));
        let _counting = {
            let calls = Arc::clone(&calls);
            register(Box::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };
        deliver(ConnectivityEvent::Lost);
        deliver(ConnectivityEvent::Lost);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_listener_may_drop_a_handle_from_inside_its_callback() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let victim = Mutex::new(Some(register(Box::new(|_| {}))));
        let _dropper = register(Box::new(move |_| {
            drop(victim.lock().unwrap().take());
            tx.send(()).unwrap();
        }));
        deliver(ConnectivityEvent::Lost);
        assert!(rx.try_recv().is_ok(), "the callback completed, no deadlock");
    }

    #[test]
    fn events_arrive_in_enqueue_order_on_the_plugin_thread() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let _handle = register(Box::new(move |event| {
            let thread = std::thread::current();
            tx.send((thread.id(), thread.name().map(str::to_owned), event))
                .unwrap();
        }));
        let caller = std::thread::spawn(|| {
            emit(ConnectivityEvent::Lost);
            emit(ConnectivityEvent::Available {
                path: crate::state::tests::path(1, crate::Transport::Wifi),
                initial: false,
            });
            std::thread::current().id()
        })
        .join()
        .unwrap();
        flush();
        let received: Vec<_> = rx.try_iter().collect();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0].2, ConnectivityEvent::Lost, "enqueue order");
        assert!(matches!(received[1].2, ConnectivityEvent::Available { .. }));
        assert_eq!(received[0].0, received[1].0, "one delivery thread");
        assert_ne!(received[0].0, caller, "never the emitting thread");
        assert_ne!(received[0].0, std::thread::current().id());
        assert_eq!(received[0].1.as_deref(), Some(THREAD_NAME));
    }

    #[test]
    fn a_full_queue_drops_the_newest_event_and_keeps_the_queued_ones() {
        let _guard = test_guard();
        let parked = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (tx, rx) = mpsc::channel::<usize>();
        let seen = AtomicUsize::new(0);
        let _handle = {
            let parked = Arc::clone(&parked);
            let release = Arc::clone(&release);
            register(Box::new(move |_| {
                let index = seen.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    // Park the consumer inside the first delivery: everything
                    // sent from now on has to fit in the bound.
                    parked.wait();
                    release.wait();
                }
                tx.send(index).unwrap();
            }))
        };
        emit(ConnectivityEvent::Lost);
        parked.wait();
        for index in 0..QUEUE_CAPACITY {
            assert!(
                send(Delivery::Event(ConnectivityEvent::Lost, u64::MAX)),
                "event {index} fits inside the bound"
            );
        }
        for _ in 0..4 {
            assert!(
                !send(Delivery::Event(ConnectivityEvent::Lost, u64::MAX)),
                "the bound is enforced rather than the queue growing"
            );
        }
        release.wait();
        flush();
        let delivered: Vec<usize> = rx.try_iter().collect();
        assert_eq!(
            delivered,
            (0..=QUEUE_CAPACITY).collect::<Vec<_>>(),
            "the parked event plus everything that fit, in order; the overflow is gone"
        );
    }
}
