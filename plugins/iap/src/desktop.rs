//! The desktop (macOS/Linux/Windows) [`Backend`] — unavailable in v1, by
//! **deferral** rather than by absence.
//!
//! `#[cfg(any(target_os = "macos", target_os = "linux", target_os =
//! "windows"))]`. The wording matters, and is the opposite of
//! `frust-haptics`'s desktop arm: haptics has no first-class desktop API to
//! route to at all, a genuine capability gap. In-app purchase is the reverse —
//! macOS ships the Mac App Store's own StoreKit and Windows ships the
//! Microsoft Store's API, so a real desktop backend here is a buildable
//! future addition that only mobile-first v1 scope keeps out. Linux alone has
//! nothing to route to. See `plugins/iap/Cargo.toml`'s desktop-stanza comment
//! for the same framing at the dependency level.
//!
//! Consequences of that framing, both deliberate:
//!
//! - Every call reports [`IapError::NotAvailable`]
//!   ([`Unavailability::UnsupportedPlatform`]) rather than panicking or
//!   silently succeeding — so an app's desktop preview build runs its real
//!   store code path and simply gets nothing, the fail-soft shape every
//!   sibling plugin uses.
//! - The three platform-specific operations
//!   ([`Backend::acknowledge_purchase`], [`Backend::consume_purchase`],
//!   [`Backend::get_pending_transactions`]) are **not** overridden here, so
//!   they answer [`IapError::NotSupportedOnPlatform`] from the trait's own
//!   default arm. That is the more precise of the two answers: those calls
//!   have no counterpart on this platform even once a desktop backend exists.
//!
//! Adding a real backend later is additive: nothing in this module is part of
//! the public API, and a caller already handles both errors.

use crate::types::{
    ActiveSubscription, FetchProductsResult, ProductRequest, Purchase, PurchaseInput,
    PurchaseOptions, RequestPurchaseProps,
};
use crate::{Backend, IapError, Unavailability};

/// The desktop backend. Stateless — there is nothing to open or hold; every
/// call reports [`IapError::NotAvailable`] synchronously, on any thread (a
/// call that never reaches a store cannot deadlock one, so the UI-thread
/// guard has nothing to guard here).
pub(crate) struct DesktopIap;

/// The one answer every method below gives.
fn unavailable<T>() -> Result<T, IapError> {
    Err(IapError::NotAvailable(Unavailability::UnsupportedPlatform))
}

impl Backend for DesktopIap {
    fn init_connection(&self, _config: Option<&serde_json::Value>) -> Result<bool, IapError> {
        unavailable()
    }

    fn end_connection(&self) -> Result<bool, IapError> {
        unavailable()
    }

    fn fetch_products(&self, _request: &ProductRequest) -> Result<FetchProductsResult, IapError> {
        unavailable()
    }

    fn get_available_purchases(
        &self,
        _options: Option<&PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError> {
        unavailable()
    }

    fn get_active_subscriptions(
        &self,
        _ids: Option<&[String]>,
    ) -> Result<Vec<ActiveSubscription>, IapError> {
        unavailable()
    }

    fn get_storefront(&self) -> Result<String, IapError> {
        unavailable()
    }

    fn request_purchase(&self, _props: &RequestPurchaseProps) -> Result<(), IapError> {
        unavailable()
    }

    fn finish_transaction(
        &self,
        _purchase: &PurchaseInput,
        _is_consumable: Option<bool>,
    ) -> Result<(), IapError> {
        unavailable()
    }

    fn restore_purchases(&self) -> Result<(), IapError> {
        unavailable()
    }

    fn deep_link_to_subscriptions(
        &self,
        _options: Option<&serde_json::Value>,
    ) -> Result<(), IapError> {
        unavailable()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::types::{ProductQueryType, ProductRequest, PurchaseInput, PurchaseState};
    use crate::{Iap, IapError, IapEvent, Unavailability};

    fn is_unavailable<T: std::fmt::Debug>(result: &Result<T, IapError>) -> bool {
        matches!(
            result,
            Err(IapError::NotAvailable(Unavailability::UnsupportedPlatform))
        )
    }

    fn purchase_input() -> PurchaseInput {
        PurchaseInput {
            id: "txn".to_owned(),
            product_id: "sku".to_owned(),
            ids: None,
            transaction_date: 0.0,
            purchase_token: None,
            store: None,
            quantity: 1,
            purchase_state: PurchaseState::Purchased,
            is_auto_renewing: false,
        }
    }

    /// Every store-touching call on the public entry point reports the typed
    /// unavailability on desktop — never a panic, never a misleading `Ok`.
    #[test]
    fn every_store_call_reports_not_available() {
        assert!(is_unavailable(&Iap::init_connection(None)));
        assert!(is_unavailable(&Iap::end_connection()));
        assert!(is_unavailable(&Iap::fetch_products(ProductRequest {
            skus: vec!["sku".to_owned()],
            kind: ProductQueryType::InApp,
        })));
        assert!(is_unavailable(&Iap::get_available_purchases(None)));
        assert!(is_unavailable(&Iap::get_active_subscriptions(None)));
        assert!(is_unavailable(&Iap::has_active_subscriptions(None)));
        assert!(is_unavailable(&Iap::get_storefront()));
        assert!(is_unavailable(&Iap::request_purchase(
            crate::mock::purchase_props("sku")
        )));
        assert!(is_unavailable(&Iap::finish_transaction(
            purchase_input(),
            None
        )));
        assert!(is_unavailable(&Iap::restore_purchases()));
        assert!(is_unavailable(&Iap::deep_link_to_subscriptions(None)));
    }

    /// The three platform-specific operations answer from the trait's default
    /// arm instead (see this module's doc) — a different, more precise error
    /// than the one above.
    #[test]
    fn the_platform_specific_operations_report_not_supported() {
        assert!(matches!(
            Iap::acknowledge_purchase("token"),
            Err(IapError::NotSupportedOnPlatform)
        ));
        assert!(matches!(
            Iap::consume_purchase("token"),
            Err(IapError::NotSupportedOnPlatform)
        ));
        assert!(matches!(
            Iap::get_pending_transactions(),
            Err(IapError::NotSupportedOnPlatform)
        ));
    }

    /// Registering a listener is in-process bookkeeping that reaches no store,
    /// so it works on desktop like anywhere else — it simply never fires,
    /// there being no backend to emit.
    #[test]
    fn a_listener_can_still_be_registered_and_removed() {
        let _guard = crate::event::test_guard();
        let calls = Arc::new(AtomicUsize::new(0));

        let handle = {
            let calls = Arc::clone(&calls);
            Iap::set_purchase_listener(Box::new(move |_: IapEvent| {
                calls.fetch_add(1, Ordering::SeqCst);
            }))
        };
        assert!(is_unavailable(&Iap::request_purchase(
            crate::mock::purchase_props("sku")
        )));
        handle.remove();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no desktop backend ever emits"
        );
    }
}
