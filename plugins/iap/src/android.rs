//! The Android [`Backend`] — **a placeholder implementation**, replaced
//! wholesale by the real Play Billing arm.
//!
//! `#[cfg(target_os = "android")]`. The real backend drives
//! `dev.frust.iap.FrustIapHost` (`plugins/iap/platform/android`, already
//! wired to `openiap-google` and holding the resumed Activity a billing flow
//! needs) over this crate's own JNI surface, marshalling every request and
//! response as an OpenIAP JSON string through `frust_plugin::android`'s
//! scoped attach.
//!
//! Until then every method answers [`IapError::NotAvailable`] — the same
//! fail-soft shape the [`crate::desktop`] arm uses, never a panic, so an
//! Android build of an app using this crate links and runs rather than
//! failing at the FFI boundary.
//!
//! # What replacing this file owes the rest of the crate
//!
//! Nothing above it changes: [`crate::Iap`] and the [`Backend`] trait are
//! fixed, and this is the only Android-compiled file. The contract the real
//! implementation must satisfy is written down in three places, all of them
//! testable without a device — [`Backend`]'s guard-order doc (readiness →
//! UI thread → connection state, the second step being the `Looper`
//! check this placeholder has nothing to check), the crate doc's *Blocking
//! API* table, and the conformance suite's assertions about the connection
//! state machine and the two-phase purchase. A purchase outcome reaches the
//! app by calling `crate::event::emit` from whatever thread Play Billing's
//! `PurchasesUpdatedListener` delivers on; the registry handles the rest.

use crate::types::{
    ActiveSubscription, FetchProductsResult, ProductRequest, Purchase, PurchaseInput,
    PurchaseOptions, RequestPurchaseProps,
};
use crate::{Backend, IapError, Unavailability};

/// The Android backend. Stateless today; the real one holds the connection
/// state and the cached host-class handles.
pub(crate) struct AndroidIap;

/// The one answer every method below gives while this arm is a placeholder.
fn not_implemented<T>() -> Result<T, IapError> {
    Err(IapError::NotAvailable(Unavailability::UnsupportedPlatform))
}

impl Backend for AndroidIap {
    fn init_connection(&self, _config: Option<&serde_json::Value>) -> Result<bool, IapError> {
        not_implemented()
    }

    fn end_connection(&self) -> Result<bool, IapError> {
        not_implemented()
    }

    fn fetch_products(&self, _request: &ProductRequest) -> Result<FetchProductsResult, IapError> {
        not_implemented()
    }

    fn get_available_purchases(
        &self,
        _options: Option<&PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError> {
        not_implemented()
    }

    fn get_active_subscriptions(
        &self,
        _ids: Option<&[String]>,
    ) -> Result<Vec<ActiveSubscription>, IapError> {
        not_implemented()
    }

    fn get_storefront(&self) -> Result<String, IapError> {
        not_implemented()
    }

    fn request_purchase(&self, _props: &RequestPurchaseProps) -> Result<(), IapError> {
        not_implemented()
    }

    fn finish_transaction(
        &self,
        _purchase: &PurchaseInput,
        _is_consumable: Option<bool>,
    ) -> Result<(), IapError> {
        not_implemented()
    }

    fn restore_purchases(&self) -> Result<(), IapError> {
        not_implemented()
    }

    fn deep_link_to_subscriptions(
        &self,
        _options: Option<&serde_json::Value>,
    ) -> Result<(), IapError> {
        not_implemented()
    }

    /// Android **is** the platform that has this operation, so the real
    /// backend overrides the trait default — this placeholder keeps the
    /// override in place (answering like every other method above) rather
    /// than falling through to a `NotSupportedOnPlatform` that would be a
    /// lie about the platform.
    fn acknowledge_purchase(&self, _purchase_token: &str) -> Result<(), IapError> {
        not_implemented()
    }

    /// Android-only, same reasoning as [`Self::acknowledge_purchase`].
    fn consume_purchase(&self, _purchase_token: &str) -> Result<(), IapError> {
        not_implemented()
    }

    // `get_pending_transactions` is deliberately NOT overridden: it is
    // iOS-only, so the trait's `NotSupportedOnPlatform` default is the
    // correct and permanent answer here.
}
