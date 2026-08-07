//! Shared conformance suite for every [`Backend`] implementation.
//!
//! `#[cfg(test)]`-only: [`run_conformance_suite`] exercises the contract
//! [`crate::Iap`] promises independently of any platform — the guard order,
//! the connection state machine (pre-init, double-init, end-then-call), the
//! product/entitlement queries, the two-phase purchase (an ack now, the
//! outcome on the event stream), the error-event path, and listener add/remove
//! semantics.
//!
//! # Signature: a factory over the fake store, not `Box<dyn Backend>`
//!
//! `frust-secure-storage`'s suite takes a `&dyn Fn(&str) -> Box<dyn Backend>`
//! factory so an on-target Keychain/Keystore test module can call the same
//! assertions; `frust-shared-preferences`' takes a single `&dyn Backend`.
//! This one takes a factory (each sub-test needs its own store — the
//! connection state machine is exactly what is under test, so a shared
//! fixture would leak state between assertions), but types it on
//! [`MockStore`] rather than `Box<dyn Backend>`, for two reasons:
//!
//! - Half of what needs asserting only exists behind **rigging** no real
//!   backend can offer: holding a purchase event until the test releases it
//!   (the ack-before-event ordering), failing a chosen SKU, and taking the
//!   platform handles away. A `Box<dyn Backend>` factory would have to leave
//!   all three untested to stay honest.
//! - Unlike a secure store, neither IAP backend can run host-side **or**
//!   unattended on-target: a real assertion needs a signed build, a store
//!   account, and a human tapping a purchase sheet. The generality would
//!   never acquire a second caller.
//!
//! The seam is not weakened by that choice: every sub-test drives the store
//! through `&dyn Backend`, so what it exercises is the trait the mobile
//! backends implement, not the fake's inherent API.
//!
//! # The whole suite runs under the event registry's test guard
//!
//! The listener registry is process-global, so [`run_conformance_suite`]
//! takes `crate::event::test_guard()` for its whole run — that is what makes
//! "nothing has been delivered yet" and "no delivery after removal" exact
//! assertions rather than races against another test's listeners.

use std::fmt::Debug;
use std::sync::mpsc::{self, Receiver};
use std::thread::ThreadId;

use crate::mock::{MockStore, SKU_CONSUMABLE, SKU_PENDING, SKU_SUBSCRIPTION, SKU_UNKNOWN};
use crate::types::{
    FetchProductsResult, IapErrorCode, IapEvent, ProductQueryType, ProductRequest, Purchase,
    PurchaseInput, PurchaseState,
};
use crate::{Backend, IapError, ListenerHandle};

/// Run the full suite. `fresh` must produce a store that is platform-ready,
/// **not** connected, and seeded with the standard fake account (see
/// [`MockStore::new`]).
pub(crate) fn run_conformance_suite(fresh: &dyn Fn() -> MockStore) {
    let _guard = crate::event::test_guard();

    platform_readiness_precedes_every_other_guard(fresh);
    every_store_op_reports_not_connected_before_init(fresh);
    init_is_idempotent_and_end_reports_what_it_closed(fresh);
    fetch_products_answers_the_requested_envelope(fresh);
    entitlement_queries_answer_the_seeded_account(fresh);
    a_purchase_acks_first_and_its_event_carries_the_purchase(fresh);
    a_purchase_can_be_finished_from_its_own_event(fresh);
    a_rigged_sku_reports_on_the_error_event_path(fresh);
    a_removed_listener_receives_nothing_further(fresh);
    restore_re_emits_owned_purchases_without_duplicating_them(fresh);
    token_settlement_and_deep_link_reach_the_store(fresh);
}

/// Guard order step 1 (see [`Backend`]'s contract): with no platform handles
/// installed, **every** call reports [`IapError::PlatformNotInitialized`] —
/// including `init_connection` itself, and including the calls that would
/// otherwise report [`IapError::NotConnected`]. Getting this order wrong
/// tells a user with an un-migrated scaffold to "call init_connection first",
/// which cannot help them.
fn platform_readiness_precedes_every_other_guard(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.disable_platform_handles();

    assert_platform_not_initialized("init_connection", store.init_connection(None));
    assert_platform_not_initialized("end_connection", store.end_connection());
    for (label, result) in every_store_op(&store) {
        assert_platform_not_initialized(label, result);
    }
}

/// Guard order step 3: before [`Backend::init_connection`], every
/// store-touching call reports [`IapError::NotConnected`] — never a panic, an
/// empty success, or a platform error.
fn every_store_op_reports_not_connected_before_init(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    for (label, result) in every_store_op(&store) {
        assert!(
            matches!(result, Err(IapError::NotConnected)),
            "{label} before init_connection: {result:?}"
        );
    }
}

/// The connection lifecycle contract: a second `init_connection` succeeds
/// rather than reporting the platform's own "already prepared" refusal;
/// `end_connection` reports whether it closed anything; and after it, the
/// store-touching calls are refused again.
fn init_is_idempotent_and_end_reports_what_it_closed(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();

    assert!(store.init_connection(None).unwrap());
    assert!(
        store.init_connection(None).unwrap(),
        "a second init_connection is idempotent, not an error"
    );
    assert!(
        store.get_storefront().is_ok(),
        "the connection is still usable after a double init"
    );

    assert!(
        store.end_connection().unwrap(),
        "end_connection reports that it closed an open connection"
    );
    assert!(
        !store.end_connection().unwrap(),
        "a second end_connection closed nothing, and is still not an error"
    );

    for (label, result) in every_store_op(&store) {
        assert!(
            matches!(result, Err(IapError::NotConnected)),
            "{label} after end_connection: {result:?}"
        );
    }

    assert!(
        store.init_connection(None).is_ok(),
        "a connection can be reopened after being closed"
    );
    assert!(store.get_storefront().is_ok());
}

/// The result envelope follows the request's own `type`, and an unknown SKU is
/// omitted rather than raised as an error (see [`crate::Iap::fetch_products`]).
fn fetch_products_answers_the_requested_envelope(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();

    let skus = vec![
        SKU_CONSUMABLE.to_owned(),
        SKU_SUBSCRIPTION.to_owned(),
        SKU_UNKNOWN.to_owned(),
    ];

    let products = store
        .fetch_products(&ProductRequest {
            skus: skus.clone(),
            kind: ProductQueryType::InApp,
        })
        .unwrap();
    match products {
        FetchProductsResult::Products(items) => {
            let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
            assert_eq!(
                ids,
                [SKU_CONSUMABLE],
                "unknown skus are omitted, not errors"
            );
        }
        other => panic!("an in-app query must answer the products envelope: {other:?}"),
    }

    let subscriptions = store
        .fetch_products(&ProductRequest {
            skus: skus.clone(),
            kind: ProductQueryType::Subs,
        })
        .unwrap();
    match subscriptions {
        FetchProductsResult::Subscriptions(items) => {
            let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
            assert_eq!(ids, [SKU_SUBSCRIPTION]);
        }
        other => panic!("a subs query must answer the subscriptions envelope: {other:?}"),
    }

    let all = store
        .fetch_products(&ProductRequest {
            skus,
            kind: ProductQueryType::All,
        })
        .unwrap();
    match all {
        FetchProductsResult::All(items) => {
            let mut ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
            ids.sort_unstable();
            assert_eq!(ids, vec![SKU_SUBSCRIPTION, SKU_CONSUMABLE]);
        }
        other => panic!("an `all` query must answer the all envelope: {other:?}"),
    }

    // A store refusal is a typed store error, not a silent empty result.
    let empty = store.fetch_products(&ProductRequest {
        skus: Vec::new(),
        kind: ProductQueryType::InApp,
    });
    assert!(
        matches!(empty, Err(IapError::Store(ref error)) if error.code == IapErrorCode::EmptySkuList),
        "an empty sku list is refused by the store: {empty:?}"
    );
}

/// The read-only entitlement surface against the seeded account: one active
/// subscription, one pending transaction, no purchased entitlement yet.
fn entitlement_queries_answer_the_seeded_account(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();

    assert!(
        store.get_available_purchases(None).unwrap().is_empty(),
        "a pending transaction is not an entitlement"
    );

    let pending = store.get_pending_transactions().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].product_id, SKU_PENDING);
    assert_eq!(pending[0].purchase_state, PurchaseState::Pending);

    let active = store.get_active_subscriptions(None).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].product_id, SKU_SUBSCRIPTION);

    let narrowed = store
        .get_active_subscriptions(Some(&[SKU_UNKNOWN.to_owned()]))
        .unwrap();
    assert!(narrowed.is_empty(), "the id filter actually narrows");

    // `has_active_subscriptions` must always agree with the list it derives
    // from (see `Backend::has_active_subscriptions`).
    assert!(store.has_active_subscriptions(None).unwrap());
    assert!(
        store
            .has_active_subscriptions(Some(&[SKU_SUBSCRIPTION.to_owned()]))
            .unwrap()
    );
    assert!(
        !store
            .has_active_subscriptions(Some(&[SKU_UNKNOWN.to_owned()]))
            .unwrap()
    );

    assert!(!store.get_storefront().unwrap().is_empty());
}

/// The two-phase purchase contract: `request_purchase` returns an ack with
/// **nothing delivered yet**, and the purchase itself arrives later, on a
/// thread that is not the caller's.
fn a_purchase_acks_first_and_its_event_carries_the_purchase(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();
    let (listener, events) = listen();

    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    assert!(
        events.try_recv().is_err(),
        "request_purchase acks the request; it never carries the outcome back"
    );
    assert!(store.has_queued_events(), "the outcome is still in flight");

    store.release_events();

    let (thread, event) = events.try_recv().expect("the outcome reached the listener");
    assert_ne!(
        thread,
        std::thread::current().id(),
        "an event is delivered on a plugin-owned thread, never the caller's"
    );
    match event {
        IapEvent::PurchaseUpdated(purchase) => {
            assert_eq!(purchase.product_id, SKU_CONSUMABLE);
            assert_eq!(purchase.purchase_state, PurchaseState::Purchased);
        }
        other => panic!("expected a purchase update: {other:?}"),
    }

    // And the purchase is now an entitlement the store reports.
    let owned = store.get_available_purchases(None).unwrap();
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].product_id, SKU_CONSUMABLE);

    listener.remove();
}

/// The settlement half of the flow: the purchase from the event is exactly
/// what `finish_transaction` takes, a consumable settlement clears the
/// entitlement, and settling something the account never owned is a typed
/// store error rather than a silent success.
fn a_purchase_can_be_finished_from_its_own_event(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();
    let (listener, events) = listen();

    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();
    let (_, event) = events.try_recv().unwrap();
    let IapEvent::PurchaseUpdated(purchase) = event else {
        panic!("expected a purchase update: {event:?}");
    };

    // A non-consumable settlement acknowledges without revoking.
    store
        .finish_transaction(&input_for(&purchase), Some(false))
        .unwrap();
    assert_eq!(store.settled(), vec![purchase.id.clone()]);
    assert_eq!(
        store.get_available_purchases(None).unwrap().len(),
        1,
        "an acknowledged non-consumable stays owned"
    );

    // A consumable settlement makes the sku buyable again.
    store
        .finish_transaction(&input_for(&purchase), Some(true))
        .unwrap();
    assert!(
        store.get_available_purchases(None).unwrap().is_empty(),
        "a consumed purchase leaves the entitlement list"
    );

    let unowned = store.finish_transaction(&input_for(&purchase), Some(true));
    assert!(
        matches!(unowned, Err(IapError::Store(ref error)) if error.code == IapErrorCode::ItemNotOwned),
        "settling an unowned transaction is a typed store error: {unowned:?}"
    );

    listener.remove();
}

/// The error-event path: a failed purchase reports on the **same** stream as a
/// successful one, not through `request_purchase`'s return value, and grants
/// nothing.
fn a_rigged_sku_reports_on_the_error_event_path(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();
    store.fail_sku(SKU_CONSUMABLE);
    let (listener, events) = listen();

    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();

    let (_, event) = events.try_recv().expect("the failure reached the listener");
    match event {
        IapEvent::PurchaseError(error) => {
            assert_eq!(error.code, IapErrorCode::ItemUnavailable);
            assert_eq!(error.product_id.as_deref(), Some(SKU_CONSUMABLE));
        }
        other => panic!("expected a purchase error: {other:?}"),
    }
    assert!(
        store.get_available_purchases(None).unwrap().is_empty(),
        "a failed purchase grants nothing"
    );

    // A sku the store has never heard of takes the same path.
    store
        .request_purchase(&crate::mock::purchase_props(SKU_UNKNOWN))
        .unwrap();
    store.release_events();
    let (_, event) = events.try_recv().unwrap();
    assert!(
        matches!(event, IapEvent::PurchaseError(ref error) if error.code == IapErrorCode::SkuNotFound),
        "an unknown sku fails on the event stream too: {event:?}"
    );

    listener.remove();
}

/// Listener add/remove semantics across the real store path: two listeners
/// both see an event, a removed one sees nothing further, and an event with
/// no listener registered is simply lost (the reason
/// [`crate::Iap::request_purchase`] insists the listener is registered
/// first).
fn a_removed_listener_receives_nothing_further(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();

    let (first, first_events) = listen();
    let (second, second_events) = listen();

    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();
    assert!(first_events.try_recv().is_ok(), "both listeners are called");
    assert!(second_events.try_recv().is_ok());

    first.remove();
    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();
    assert!(
        first_events.try_recv().is_err(),
        "a removed listener receives nothing further"
    );
    assert!(second_events.try_recv().is_ok(), "the other one still does");

    second.remove();
    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();
    assert!(
        second_events.try_recv().is_err(),
        "with no listener registered the outcome is lost, not queued"
    );
}

/// A restore re-emits what the account already owns onto the same stream —
/// and must not grant those purchases a second time.
fn restore_re_emits_owned_purchases_without_duplicating_them(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();
    let (listener, events) = listen();

    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();
    events.try_recv().unwrap();
    let owned_before = store.get_available_purchases(None).unwrap().len();

    store.restore_purchases().unwrap();
    store.release_events();

    let (_, event) = events.try_recv().expect("the restore reached the listener");
    assert!(matches!(event, IapEvent::PurchaseUpdated(_)));
    assert_eq!(
        store.get_available_purchases(None).unwrap().len(),
        owned_before,
        "a restore re-reports entitlements, it does not duplicate them"
    );

    listener.remove();
}

/// The token-level Android settlement pair and the subscription deep link —
/// the ops a real backend answers from the same open connection.
fn token_settlement_and_deep_link_reach_the_store(fresh: &dyn Fn() -> MockStore) {
    let store = fresh();
    store.init_connection(None).unwrap();
    let (listener, events) = listen();

    store
        .request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))
        .unwrap();
    store.release_events();
    let (_, event) = events.try_recv().unwrap();
    let IapEvent::PurchaseUpdated(purchase) = event else {
        panic!("expected a purchase update: {event:?}");
    };
    let token = purchase.purchase_token.clone().expect("a purchase token");

    store.acknowledge_purchase(&token).unwrap();
    assert_eq!(store.settled(), vec![purchase.id.clone()]);
    assert_eq!(
        store.get_available_purchases(None).unwrap().len(),
        1,
        "acknowledging does not revoke"
    );

    store.consume_purchase(&token).unwrap();
    assert!(
        store.get_available_purchases(None).unwrap().is_empty(),
        "consuming does"
    );

    let unknown = store.consume_purchase("no-such-token");
    assert!(
        matches!(unknown, Err(IapError::Store(ref error)) if error.code == IapErrorCode::ItemNotOwned),
        "an unknown token is a typed store error: {unknown:?}"
    );

    assert_eq!(store.deep_link_count(), 0);
    store.deep_link_to_subscriptions(None).unwrap();
    assert_eq!(store.deep_link_count(), 1);

    listener.remove();
}

// --- Helpers ----------------------------------------------------------------

/// Every store-touching op, run against `store`, paired with its name — the
/// list the guard-order sub-tests sweep. `init_connection`/`end_connection`
/// are deliberately absent: they are the calls that *establish* the state the
/// others are refused for.
fn every_store_op(store: &MockStore) -> Vec<(&'static str, Result<Erased, IapError>)> {
    vec![
        (
            "fetch_products",
            erase(store.fetch_products(&ProductRequest {
                skus: vec![SKU_CONSUMABLE.to_owned()],
                kind: ProductQueryType::InApp,
            })),
        ),
        (
            "get_available_purchases",
            erase(store.get_available_purchases(None)),
        ),
        (
            "get_active_subscriptions",
            erase(store.get_active_subscriptions(None)),
        ),
        (
            "has_active_subscriptions",
            erase(store.has_active_subscriptions(None)),
        ),
        ("get_storefront", erase(store.get_storefront())),
        (
            "request_purchase",
            erase(store.request_purchase(&crate::mock::purchase_props(SKU_CONSUMABLE))),
        ),
        (
            "finish_transaction",
            erase(store.finish_transaction(
                &PurchaseInput {
                    id: "mock-txn-1".to_owned(),
                    product_id: SKU_CONSUMABLE.to_owned(),
                    ids: None,
                    transaction_date: 0.0,
                    purchase_token: None,
                    store: None,
                    quantity: 1,
                    purchase_state: PurchaseState::Purchased,
                    is_auto_renewing: false,
                },
                None,
            )),
        ),
        ("restore_purchases", erase(store.restore_purchases())),
        (
            "deep_link_to_subscriptions",
            erase(store.deep_link_to_subscriptions(None)),
        ),
        (
            "acknowledge_purchase",
            erase(store.acknowledge_purchase("mock-token-1")),
        ),
        (
            "consume_purchase",
            erase(store.consume_purchase("mock-token-1")),
        ),
        (
            "get_pending_transactions",
            erase(store.get_pending_transactions()),
        ),
    ]
}

/// A discarded success value — the guard-order sweeps care only about which
/// error came back, across ops with a dozen different `Ok` types.
#[derive(Debug)]
struct Erased;

fn erase<T: Debug>(result: Result<T, IapError>) -> Result<Erased, IapError> {
    result.map(|_| Erased)
}

fn assert_platform_not_initialized(label: &str, result: Result<impl Debug, IapError>) {
    assert!(
        matches!(result, Err(IapError::PlatformNotInitialized)),
        "{label} with no platform handles: {result:?}"
    );
}

/// Register a listener that forwards `(delivering thread, event)` — the
/// thread id is what proves an event never arrives on the caller's thread.
fn listen() -> (ListenerHandle, Receiver<(ThreadId, IapEvent)>) {
    let (sender, receiver) = mpsc::channel();
    let handle = crate::Iap::set_purchase_listener(Box::new(move |event| {
        // A closed receiver means the test already finished with this
        // listener; dropping the event is the right answer, not a panic
        // inside a callback.
        let _ = sender.send((std::thread::current().id(), event));
    }));
    (handle, receiver)
}

/// The `PurchaseInput` an app builds from a delivered [`Purchase`] to settle
/// it — the narrower input shape `type.graphql` defines.
fn input_for(purchase: &Purchase) -> PurchaseInput {
    PurchaseInput {
        id: purchase.id.clone(),
        product_id: purchase.product_id.clone(),
        ids: purchase.ids.clone(),
        transaction_date: purchase.transaction_date,
        purchase_token: purchase.purchase_token.clone(),
        store: Some(purchase.store),
        quantity: purchase.quantity,
        purchase_state: purchase.purchase_state,
        is_auto_renewing: purchase.is_auto_renewing,
    }
}
