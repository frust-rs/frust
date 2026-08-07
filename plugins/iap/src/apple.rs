//! The iOS [`Backend`] — **a placeholder implementation**, replaced wholesale
//! by the real StoreKit arm.
//!
//! `#[cfg(target_os = "ios")]` — iOS only, not `target_vendor = "apple"`: this
//! crate has no macOS arm to share with (see [`crate::desktop`]'s deferral),
//! and the bridge it drives is iOS-side Swift.
//!
//! The real backend calls `FrustIapBridge` (`plugins/iap/platform/ios`,
//! already proven to resolve by `@objc` runtime name, to link OpenIAP, and to
//! deliver a Rust-owned completion block off the caller's thread) with an
//! OpenIAP JSON string per request, and reads one back.
//!
//! Until then every method answers [`IapError::NotAvailable`] — the same
//! fail-soft shape the [`crate::desktop`] arm uses, never a panic, so an iOS
//! build of an app using this crate links and runs rather than failing at the
//! FFI boundary.
//!
//! # What replacing this file owes the rest of the crate
//!
//! Nothing above it changes: [`crate::Iap`] and the [`Backend`] trait are
//! fixed, and this is the only iOS-compiled file. The contract the real
//! implementation must satisfy is written down in three places, all of them
//! testable without a device — [`Backend`]'s guard-order doc (readiness →
//! UI thread → connection state, the second step being the main-thread check
//! this placeholder has nothing to check), the crate doc's *Blocking API*
//! table, and the conformance suite's assertions about the connection state
//! machine and the two-phase purchase. A purchase outcome reaches the app by
//! calling `crate::event::emit` from whatever thread StoreKit's
//! transaction-update task delivers on; the registry handles the rest.

use crate::types::{
    ActiveSubscription, FetchProductsResult, ProductRequest, Purchase, PurchaseInput,
    PurchaseOptions, RequestPurchaseProps,
};
use crate::{Backend, IapError, Unavailability};

/// The Apple backend. Stateless today; the real one holds the connection
/// state and the bridge class handle.
pub(crate) struct AppleIap;

/// The one answer every method below gives while this arm is a placeholder.
fn not_implemented<T>() -> Result<T, IapError> {
    Err(IapError::NotAvailable(Unavailability::UnsupportedPlatform))
}

impl Backend for AppleIap {
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

    /// iOS **is** the platform that has this operation, so the real backend
    /// overrides the trait default — this placeholder keeps the override in
    /// place (answering like every other method above) rather than falling
    /// through to a `NotSupportedOnPlatform` that would be a lie about the
    /// platform.
    fn get_pending_transactions(&self) -> Result<Vec<Purchase>, IapError> {
        not_implemented()
    }

    // `acknowledge_purchase`/`consume_purchase` are deliberately NOT
    // overridden: they are Android-only, so the trait's
    // `NotSupportedOnPlatform` default is the correct and permanent answer
    // here — StoreKit has no acknowledgement step.
}
