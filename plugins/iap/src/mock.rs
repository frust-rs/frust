//! A `#[cfg(test)]` in-memory fake store implementing [`Backend`] — the
//! fixture the shared conformance suite ([`crate::conformance`]) runs against
//! on every host.
//!
//! Neither real backend can run host-side (`android`/`apple` compile only for
//! their own targets, and even there they need a live Play/StoreKit account),
//! so without this the crate's whole platform-independent contract — the
//! connection state machine, the two-phase purchase, the event stream, the
//! guard order — would be checked by nothing but a device gate. This store is
//! deliberately behind the *same* [`Backend`] seam the mobile backends
//! implement, so the suite exercises the real dispatch contract rather than a
//! parallel one.
//!
//! # What it fakes, and what it deliberately does not
//!
//! It models a small store account: a two-product catalog, one already-active
//! subscription, one already-pending transaction, and whatever the test buys.
//! It does **not** model money, receipts, signature verification, or
//! Play-vs-StoreKit behavioral differences — every one of those lives in the
//! real backends, and faking them here would test this file rather than the
//! contract.
//!
//! # It implements every operation — including the three no real backend fully owns
//!
//! `acknowledge_purchase`/`consume_purchase` (Android-only) and
//! `get_pending_transactions` (iOS-only) are each overridden below, unlike
//! either real backend — `android.rs` owns the first two and falls through to
//! the [`Backend`] trait's `NotSupportedOnPlatform` default for the third;
//! `apple.rs` is the mirror image. This store therefore stands in for "a
//! backend that supports every operation": its guard order for those three
//! documents what a *supporting* backend must do (what Android already does
//! for ack/consume, what iOS already does for get_pending), not a universal
//! claim about every backend's unsupported arm, which short-circuits with
//! `NotSupportedOnPlatform` before any guard runs at all. See
//! `crate::conformance`'s `universal_store_ops` for the split this drives in
//! the guard-order sweeps.
//!
//! # Rigging: events are held until the test releases them
//!
//! A real purchase acknowledges immediately and reports its outcome later, on
//! another thread ([`crate::Iap::request_purchase`]). Emitting straight from
//! `request_purchase` would make that ordering untestable. So a queued event
//! stays queued until [`MockStore::release_events`], which emits the queue and
//! then waits on the registry's own delivery barrier: the assertion "nothing
//! was delivered yet" is exact rather than a timing guess.
//!
//! The rig needs no thread of its own for the "not the caller's thread" half:
//! the registry hands every event to the plugin-owned delivery thread
//! ([`crate::event`]), so the mock emits from the test thread exactly as a
//! platform callback would and the hop under test is the real one rather than
//! a spawn staged here.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::types::{
    ActiveSubscription, FetchProductsResult, IapErrorCode, IapEvent, IapPlatform, IapPurchaseError,
    IapStore, Product, ProductQueryType, ProductRequest, ProductSubscription, ProductType,
    Purchase, PurchaseInput, PurchaseOptions, PurchaseState, RequestPurchaseProps,
    RequestPurchasePropsByPlatforms,
};
use crate::{Backend, IapError};

/// A consumable in the seeded catalog — the SKU the suite buys.
pub(crate) const SKU_CONSUMABLE: &str = "premium_upgrade";
/// A subscription in the seeded catalog, already active on the fake account.
pub(crate) const SKU_SUBSCRIPTION: &str = "monthly_premium";
/// A SKU no catalog entry matches.
pub(crate) const SKU_UNKNOWN: &str = "no_such_sku";
/// The product id of the seeded already-pending transaction.
pub(crate) const SKU_PENDING: &str = "pending_upgrade";
/// The storefront the fake account is billed through.
pub(crate) const STOREFRONT: &str = "USA";

/// A fixed transaction timestamp (epoch ms) — the mock never reads a clock, so
/// every payload it produces is byte-identical across runs.
const TRANSACTION_DATE: f64 = 1_738_000_000_000.0;

/// A queued purchase-flow outcome, plus whether releasing it also grants the
/// purchase.
///
/// The flag is what keeps [`MockStore::restore_purchases`] from duplicating
/// entitlements: a restore re-emits events for purchases the account
/// *already* owns, so its events carry `grants = false`, while a fresh
/// purchase's event carries `true`.
struct QueuedEvent {
    event: IapEvent,
    grants: bool,
}

/// The fake account's whole state.
struct State {
    /// Step 1 of the guard order — see [`Backend`]'s contract. `false`
    /// models an Android scaffold with no platform handles installed.
    platform_ready: bool,
    /// Step 3 of the guard order: whether `init_connection` has run.
    connected: bool,
    /// One-shot: the next `init_connection` reports the store's own decline
    /// (`Ok(false)`) instead of connecting, then clears itself.
    decline_connect: bool,
    products: Vec<Product>,
    subscriptions: Vec<ProductSubscription>,
    active: Vec<ActiveSubscription>,
    /// Purchases the account holds — seeded with one `Pending` transaction,
    /// grown by a released purchase event.
    owned: Vec<Purchase>,
    /// Transaction ids `finish_transaction`/`acknowledge_purchase` settled
    /// without removing (the non-consumable path).
    settled: Vec<String>,
    /// SKUs rigged to fail their purchase flow.
    failing: HashSet<String>,
    queued: Vec<QueuedEvent>,
    deep_links: usize,
    next_transaction: u64,
}

/// The in-memory fake store — see the module doc.
pub(crate) struct MockStore {
    state: Mutex<State>,
}

impl MockStore {
    /// A fresh store: platform ready, **not** connected, seeded catalog and
    /// account.
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                platform_ready: true,
                connected: false,
                decline_connect: false,
                products: vec![product(
                    SKU_CONSUMABLE,
                    "Premium Upgrade",
                    ProductType::InApp,
                )],
                subscriptions: vec![subscription(SKU_SUBSCRIPTION, "Monthly Premium")],
                active: vec![active_subscription(SKU_SUBSCRIPTION)],
                owned: vec![pending_purchase()],
                settled: Vec::new(),
                failing: HashSet::new(),
                queued: Vec::new(),
                deep_links: 0,
                next_transaction: 1,
            }),
        }
    }

    /// Take the platform handles away: every call must now report
    /// [`IapError::PlatformNotInitialized`], ahead of every other guard.
    ///
    /// A knob rather than a second constructor so the conformance suite needs
    /// only the one fresh-store factory it is handed.
    pub(crate) fn disable_platform_handles(&self) {
        self.lock().platform_ready = false;
    }

    /// Rig `sku`'s purchase flow to fail: a request for it queues an
    /// [`IapEvent::PurchaseError`] instead of a purchase.
    pub(crate) fn fail_sku(&self, sku: &str) {
        self.lock().failing.insert(sku.to_owned());
    }

    /// Rig the next `init_connection` to answer the store's own decline
    /// (`Ok(false)`, never an error) instead of connecting — the one-shot
    /// counterpart to a real backend's own store-refused-to-connect path.
    pub(crate) fn decline_next_connect(&self) {
        self.lock().decline_connect = true;
    }

    /// Deliver every queued event and wait for delivery to finish — see the
    /// module doc's *Rigging* section.
    pub(crate) fn release_events(&self) {
        let queued = std::mem::take(&mut self.lock().queued);

        {
            let mut state = self.lock();
            for entry in &queued {
                if let (true, IapEvent::PurchaseUpdated(purchase)) = (entry.grants, &entry.event) {
                    state.owned.push(purchase.clone());
                }
            }
        }

        // Emitted from here exactly as a platform callback would: the registry
        // owns the hop onto its delivery thread, and the barrier below is what
        // keeps the test deterministic.
        for entry in queued {
            crate::event::emit(&entry.event);
        }
        crate::event::flush();
    }

    /// Transaction ids settled without being removed (the non-consumable
    /// `finish_transaction`/`acknowledge_purchase` path).
    pub(crate) fn settled(&self) -> Vec<String> {
        self.lock().settled.clone()
    }

    /// How many times [`Backend::deep_link_to_subscriptions`] ran.
    pub(crate) fn deep_link_count(&self) -> usize {
        self.lock().deep_links
    }

    /// Whether any event is still waiting for [`Self::release_events`].
    pub(crate) fn has_queued_events(&self) -> bool {
        !self.lock().queued.is_empty()
    }

    /// Poison-recovering lock, matching the registry's own strategy: a test
    /// that panicked mid-assertion must not cascade into every later one.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The guard order [`Backend`] documents, as far as this store can honour
    /// it: the delivery-thread refusal, then readiness, then (a real backend's
    /// UI-thread check, which has no counterpart in-process), then the
    /// connection state.
    ///
    /// Step 0 is not platform-specific — the delivery thread is this crate's
    /// own, on every target — so the fake store runs the real check rather than
    /// skipping it like step 2. That is what lets the conformance suite assert
    /// a listener's re-entrant call is refused, on every host, instead of only
    /// on a device.
    fn guard(&self, needs_connection: bool) -> Result<MutexGuard<'_, State>, IapError> {
        crate::event::reject_from_delivery_thread()?;
        let state = self.lock();
        if !state.platform_ready {
            return Err(IapError::PlatformNotInitialized);
        }
        if needs_connection && !state.connected {
            return Err(IapError::NotConnected);
        }
        Ok(state)
    }
}

impl Backend for MockStore {
    fn init_connection(&self, _config: Option<&serde_json::Value>) -> Result<bool, IapError> {
        let mut state = self.guard(false)?;
        if std::mem::take(&mut state.decline_connect) {
            return Ok(false);
        }
        // Idempotent by contract — a second call is a success, not an
        // `AlreadyPrepared` error (see `Iap::init_connection`).
        state.connected = true;
        Ok(true)
    }

    fn end_connection(&self) -> Result<bool, IapError> {
        let mut state = self.guard(false)?;
        let was_open = state.connected;
        state.connected = false;
        Ok(was_open)
    }

    fn fetch_products(&self, request: &ProductRequest) -> Result<FetchProductsResult, IapError> {
        let state = self.guard(true)?;
        if request.skus.is_empty() {
            return Err(store_error(
                IapErrorCode::EmptySkuList,
                "no skus requested",
                None,
            ));
        }
        let wanted = |id: &str| request.skus.iter().any(|sku| sku == id);

        let products: Vec<Product> = state
            .products
            .iter()
            .filter(|product| wanted(&product.id))
            .cloned()
            .collect();
        let subscriptions: Vec<ProductSubscription> = state
            .subscriptions
            .iter()
            .filter(|subscription| wanted(&subscription.id))
            .cloned()
            .collect();

        Ok(match request.kind {
            ProductQueryType::InApp => FetchProductsResult::Products(products),
            ProductQueryType::Subs => FetchProductsResult::Subscriptions(subscriptions),
            ProductQueryType::All => FetchProductsResult::All(
                products
                    .into_iter()
                    .chain(subscriptions.iter().map(as_product))
                    .collect(),
            ),
        })
    }

    fn get_available_purchases(
        &self,
        _options: Option<&PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError> {
        let state = self.guard(true)?;
        // A pending transaction is not yet an entitlement — it answers
        // `get_pending_transactions` instead.
        Ok(state
            .owned
            .iter()
            .filter(|purchase| purchase.purchase_state == PurchaseState::Purchased)
            .cloned()
            .collect())
    }

    fn get_active_subscriptions(
        &self,
        ids: Option<&[String]>,
    ) -> Result<Vec<ActiveSubscription>, IapError> {
        let state = self.guard(true)?;
        Ok(state
            .active
            .iter()
            .filter(|subscription| ids.is_none_or(|ids| ids.contains(&subscription.product_id)))
            .cloned()
            .collect())
    }

    fn get_storefront(&self) -> Result<String, IapError> {
        let _state = self.guard(true)?;
        Ok(STOREFRONT.to_owned())
    }

    fn request_purchase(&self, props: &RequestPurchaseProps) -> Result<(), IapError> {
        let mut state = self.guard(true)?;
        let Some(sku) = requested_sku(props) else {
            return Err(store_error(
                IapErrorCode::EmptySkuList,
                "no sku in the purchase request",
                None,
            ));
        };

        let queued = if state.failing.contains(&sku) {
            QueuedEvent {
                event: IapEvent::PurchaseError(IapPurchaseError {
                    code: IapErrorCode::ItemUnavailable,
                    message: "the rigged sku is unavailable".to_owned(),
                    product_id: Some(sku),
                }),
                grants: false,
            }
        } else if state.products.iter().any(|product| product.id == sku)
            || state
                .subscriptions
                .iter()
                .any(|subscription| subscription.id == sku)
        {
            let index = state.next_transaction;
            state.next_transaction += 1;
            QueuedEvent {
                event: IapEvent::PurchaseUpdated(purchase(&sku, index)),
                grants: true,
            }
        } else {
            QueuedEvent {
                event: IapEvent::PurchaseError(IapPurchaseError {
                    code: IapErrorCode::SkuNotFound,
                    message: "no such sku in this store".to_owned(),
                    product_id: Some(sku),
                }),
                grants: false,
            }
        };
        state.queued.push(queued);

        // The ack: the flow is under way, the outcome is the event stream's.
        Ok(())
    }

    fn finish_transaction(
        &self,
        purchase: &PurchaseInput,
        is_consumable: Option<bool>,
    ) -> Result<(), IapError> {
        let mut state = self.guard(true)?;
        let Some(index) = state.owned.iter().position(|owned| owned.id == purchase.id) else {
            return Err(store_error(
                IapErrorCode::ItemNotOwned,
                "no such transaction on this account",
                Some(purchase.product_id.clone()),
            ));
        };

        if is_consumable == Some(true) {
            state.owned.remove(index);
        } else {
            state.settled.push(purchase.id.clone());
        }
        Ok(())
    }

    fn restore_purchases(&self) -> Result<(), IapError> {
        let mut state = self.guard(true)?;
        let restored: Vec<QueuedEvent> = state
            .owned
            .iter()
            .filter(|purchase| purchase.purchase_state == PurchaseState::Purchased)
            .map(|purchase| QueuedEvent {
                event: IapEvent::PurchaseUpdated(purchase.clone()),
                // Already owned — re-emitting must not grant it twice.
                grants: false,
            })
            .collect();
        state.queued.extend(restored);
        Ok(())
    }

    fn deep_link_to_subscriptions(
        &self,
        _options: Option<&serde_json::Value>,
    ) -> Result<(), IapError> {
        let mut state = self.guard(true)?;
        state.deep_links += 1;
        Ok(())
    }

    fn acknowledge_purchase(&self, purchase_token: &str) -> Result<(), IapError> {
        let mut state = self.guard(true)?;
        let Some(id) = state
            .owned
            .iter()
            .find(|owned| owned.purchase_token.as_deref() == Some(purchase_token))
            .map(|owned| owned.id.clone())
        else {
            return Err(store_error(
                IapErrorCode::ItemNotOwned,
                "no purchase holds that token",
                None,
            ));
        };
        state.settled.push(id);
        Ok(())
    }

    fn consume_purchase(&self, purchase_token: &str) -> Result<(), IapError> {
        let mut state = self.guard(true)?;
        let Some(index) = state
            .owned
            .iter()
            .position(|owned| owned.purchase_token.as_deref() == Some(purchase_token))
        else {
            return Err(store_error(
                IapErrorCode::ItemNotOwned,
                "no purchase holds that token",
                None,
            ));
        };
        state.owned.remove(index);
        Ok(())
    }

    fn get_pending_transactions(&self) -> Result<Vec<Purchase>, IapError> {
        let state = self.guard(true)?;
        Ok(state
            .owned
            .iter()
            .filter(|purchase| purchase.purchase_state == PurchaseState::Pending)
            .cloned()
            .collect())
    }
}

// --- Fixtures ---------------------------------------------------------------

/// A purchase request for `sku`, shaped like the Android (`google`) payload —
/// [`RequestPurchasePropsByPlatforms`] carries raw platform JSON by design,
/// so this is what a real caller writes too.
pub(crate) fn purchase_props(sku: &str) -> RequestPurchaseProps {
    RequestPurchaseProps {
        request_purchase: Some(RequestPurchasePropsByPlatforms {
            apple: None,
            google: Some(serde_json::json!({ "skus": [sku] })),
        }),
        request_subscription: None,
        kind: ProductQueryType::InApp,
    }
}

/// The SKU a request names, from whichever platform payload it carries.
fn requested_sku(props: &RequestPurchaseProps) -> Option<String> {
    let by_platform = props
        .request_purchase
        .as_ref()
        .or(props.request_subscription.as_ref())?;
    let payload = by_platform.google.as_ref().or(by_platform.apple.as_ref())?;
    Some(
        payload
            .get("skus")?
            .as_array()?
            .first()?
            .as_str()?
            .to_owned(),
    )
}

fn product(id: &str, title: &str, kind: ProductType) -> Product {
    Product {
        id: id.to_owned(),
        title: title.to_owned(),
        description: format!("{title} (mock)"),
        price: Some(4.99),
        display_price: "$4.99".to_owned(),
        currency: "USD".to_owned(),
        kind,
        platform: IapPlatform::Android,
        extra: serde_json::Map::new(),
    }
}

fn subscription(id: &str, title: &str) -> ProductSubscription {
    ProductSubscription {
        id: id.to_owned(),
        title: title.to_owned(),
        description: format!("{title} (mock)"),
        price: Some(9.99),
        display_price: "$9.99/month".to_owned(),
        currency: "USD".to_owned(),
        kind: ProductType::Subs,
        platform: IapPlatform::Android,
        extra: serde_json::Map::new(),
    }
}

/// A subscription payload as a [`Product`] — what
/// [`FetchProductsResult::All`]'s element type
/// ([`crate::ProductOrSubscription`]) already is.
fn as_product(subscription: &ProductSubscription) -> Product {
    Product {
        id: subscription.id.clone(),
        title: subscription.title.clone(),
        description: subscription.description.clone(),
        price: subscription.price,
        display_price: subscription.display_price.clone(),
        currency: subscription.currency.clone(),
        kind: subscription.kind,
        platform: subscription.platform,
        extra: subscription.extra.clone(),
    }
}

fn active_subscription(product_id: &str) -> ActiveSubscription {
    ActiveSubscription {
        product_id: product_id.to_owned(),
        is_active: true,
        expiration_date_ios: None,
        auto_renewing_android: Some(true),
        environment_ios: None,
        days_until_expiration_ios: None,
        transaction_id: "mock-subscription-txn".to_owned(),
        purchase_token: Some("mock-subscription-token".to_owned()),
        transaction_date: TRANSACTION_DATE,
        base_plan_id_android: None,
        purchase_token_android: None,
        current_plan_id: None,
        renewal_info_ios: None,
    }
}

fn purchase(product_id: &str, index: u64) -> Purchase {
    Purchase {
        id: format!("mock-txn-{index}"),
        product_id: product_id.to_owned(),
        ids: None,
        transaction_date: TRANSACTION_DATE,
        purchase_token: Some(format!("mock-token-{index}")),
        store: IapStore::Google,
        quantity: 1,
        purchase_state: PurchaseState::Purchased,
        is_auto_renewing: false,
        current_plan_id: None,
        extra: serde_json::Map::new(),
    }
}

/// The seeded transaction the store already holds in `Pending` — what
/// [`Backend::get_pending_transactions`] answers with.
fn pending_purchase() -> Purchase {
    Purchase {
        purchase_state: PurchaseState::Pending,
        ..purchase(SKU_PENDING, 0)
    }
}

fn store_error(code: IapErrorCode, message: &str, product_id: Option<String>) -> IapError {
    IapError::Store(IapPurchaseError {
        code,
        message: message.to_owned(),
        product_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole shared contract suite, against a fresh fake store per
    /// sub-test — the host-runnable half of this crate's backend coverage
    /// (`docs/PLUGINS_ARCHITECTURE.md`'s shared-conformance convention).
    #[test]
    fn conformance() {
        crate::conformance::run_conformance_suite(&MockStore::new);
    }

    /// The fixture builder itself: a purchase request round-trips to the SKU
    /// it names, through the raw platform payload the wire type carries.
    #[test]
    fn purchase_props_names_its_sku() {
        assert_eq!(
            requested_sku(&purchase_props(SKU_CONSUMABLE)).as_deref(),
            Some(SKU_CONSUMABLE)
        );
    }

    /// A request naming no SKU at all is refused by the store rather than
    /// silently queuing an event nobody asked for.
    #[test]
    fn a_request_with_no_sku_is_refused() {
        let store = MockStore::new();
        store.init_connection(None).unwrap();

        let empty = RequestPurchaseProps {
            request_purchase: None,
            request_subscription: None,
            kind: ProductQueryType::InApp,
        };
        assert!(matches!(
            store.request_purchase(&empty),
            Err(IapError::Store(_))
        ));
        assert!(!store.has_queued_events());
    }
}
