//! The purchase-event listener registry: the one place a backend's platform
//! callback (a Play Billing `PurchasesUpdatedListener`, a StoreKit
//! transaction-update task) turns into the app-facing
//! [`crate::Iap::set_purchase_listener`] callbacks.
//!
//! Process-global, like the store connection it reports on — there is one
//! billing client per process, so a per-instance registry would have nothing
//! to hang off. `frust-camera`'s per-session image-stream callback is the
//! contrasting shape: a camera session is a handle an app can hold several of.
//!
//! # Lock strategy: snapshot, unlock, then call — and a panicking listener is contained
//!
//! [`emit`] never holds the registry lock while running a listener. It clones
//! the current listener list (a `Vec` of `Arc`s — cheap) under the lock, drops
//! the guard, and only then calls each one. Two things follow, both
//! load-bearing:
//!
//! - **No re-entrancy deadlock.** A listener that drops a [`ListenerHandle`]
//!   (or registers another listener) re-enters this module and takes the same
//!   lock; with the guard already dropped, that is an ordinary lock acquisition
//!   rather than a self-deadlock on a platform callback thread.
//! - **A panicking listener cannot poison the registry.** Each call is wrapped
//!   in [`std::panic::catch_unwind`], so the unwind stops here, is logged, and
//!   the remaining listeners still receive the event — and the unwind never
//!   reaches the JNI/ObjC frame that called into Rust, which would be
//!   undefined behavior rather than a bug (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI
//!   rule). Every lock acquisition additionally recovers from poisoning
//!   (`unwrap_or_else(PoisonError::into_inner)`), so even a panic raised while
//!   the lock *is* held (there is no such path today) degrades to a warning
//!   rather than making every later call fail.
//!
//! Under a release build's `panic = "abort"` (`docs/DEVELOPMENT.md`'s
//! release-profile hardening) `catch_unwind` catches nothing — the process
//! aborts at the panic site. The containment above is a debug/profile
//! safeguard and a correctness statement about this module's own state, not a
//! promise that a panicking listener is survivable in release.
//!
//! # Delivery order
//!
//! Listeners run in registration order, on the emitting thread, one after the
//! other — so a slow listener delays the ones behind it (and, on a real
//! backend, the platform callback thread itself). That is the reason
//! [`crate::Iap::set_purchase_listener`] documents its callbacks as
//! non-blocking.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard};

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
    /// reaches it.
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

/// Deliver `event` to every registered listener, in registration order.
///
/// Called by a backend from whatever thread the platform delivers its
/// purchase updates on; see the module doc for the lock strategy and the
/// panic containment this provides, and
/// [`crate::Iap::set_purchase_listener`] for the contract the listeners
/// themselves owe.
///
/// The `dead_code` allowance covers every non-test build: the only callers
/// outside the fake store are the two mobile backends, whose placeholder
/// implementations don't emit yet, so a plain `cargo build` on any target
/// currently sees this as unreachable (the same shape `frust-camera` uses for
/// its Android-only completion-correlation module).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn emit(event: &IapEvent) {
    // Snapshot under the lock, call outside it — see the module doc.
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

/// Serializes every test that touches the process-global registry, and hands
/// back a cleared one.
///
/// The registry is process-global by design, so two `#[test]`s registering
/// listeners in parallel would see each other's events. Every test in this
/// crate that registers, removes, or emits takes this guard first; holding it
/// for the whole test body is what makes "no delivery after removal" and
/// "nothing was delivered yet" deterministic assertions rather than races.
///
/// Poisoning is recovered rather than propagated: one failing test must not
/// cascade into every later one.
#[cfg(test)]
pub(crate) fn test_guard() -> TestGuard {
    let guard = TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    #[test]
    fn a_registered_listener_receives_an_emitted_event() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handle = register(Box::new(move |event| tx.send(label_of(&event)).unwrap()));
        emit(&event("alpha"));

        assert_eq!(rx.try_recv().unwrap(), "alpha");
        handle.remove();
    }

    #[test]
    fn nothing_is_delivered_after_the_handle_is_removed() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();

        let handle = register(Box::new(move |event| tx.send(label_of(&event)).unwrap()));
        handle.remove();
        emit(&event("alpha"));

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
        emit(&event("alpha"));

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

        emit(&event("alpha"));
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
        emit(&event("alpha"));

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

        emit(&event("alpha"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the listener behind the panicking one still ran"
        );

        emit(&event("beta"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the registry still works after a panic"
        );

        panicking.remove();
        counting.remove();
    }

    /// Emitting with nothing registered is a no-op, not a failure — the state
    /// a backend is in before the app registers anything.
    #[test]
    fn emitting_with_no_listeners_is_a_no_op() {
        let _guard = test_guard();
        emit(&event("alpha"));
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

        emit(&event("alpha"));
        assert!(rx.try_recv().is_ok(), "the callback completed, no deadlock");
        dropper.remove();
    }
}
