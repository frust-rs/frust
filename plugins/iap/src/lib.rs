//! `frust-iap`: an in-app-purchase plugin over Android Play Billing
//! (`openiap-google`) and iOS StoreKit (`openiap-apple`), speaking the
//! OpenIAP 3.0.1 wire protocol (see [`types`]) as JSON strings across the
//! Rust<->Kotlin/Swift FFI boundary.
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-camera`](../frust_camera/index.html) and
//! `frust-clipboard`/`frust-haptics`/`frust-secure-storage`/
//! `frust-shared-preferences`, this is a **platform plugin** (see
//! `docs/ARCHITECTURE.md`'s Module Structure): it depends on `frust-plugin`
//! plus FFI crates only, and carries **no other `frust-*` framework
//! dependency**. An app adds this crate to its own `Cargo.toml` alongside
//! `frust`, the Flutter-pubspec model; the `frust` facade does not depend on
//! or re-export it.
//!
//! # Status: crate skeleton + wire-type subset (this task)
//!
//! This crate currently ships only its `Cargo.toml` (every target-gated FFI
//! dependency both mobile backends will need, stamped up front and frozen —
//! see that file's own comments) and [`types`] (the v1 serde subset mirroring
//! the OpenIAP JSON wire shapes). There is no `Iap` entry point, no
//! `#[cfg(target_os = ...)]` backend dispatch, and no Android/iOS module yet
//! — a later task builds the public API surface and routes it to
//! `platform/android`'s `FrustIapHost`/`platform/ios`'s Swift glue the same
//! way `frust-haptics::Haptics::perform` dispatches to its own three
//! backends.
//!
//! # Event-stream contract (forward pointer)
//!
//! [`types::IapEvent`] is the closed vocabulary a later task's purchase-update
//! listener emits (`PurchaseUpdated`/`PurchaseError`, mirroring
//! `event.graphql`'s `purchaseUpdated`/`purchaseError` subscriptions) —
//! adjacently tagged (`{"event": "purchaseUpdated", "data": {...}}`, see that
//! type's own doc) as the wire envelope both mobile hosts will emit onto,
//! parity with [`types::FetchProductsResult`]'s own designed envelope. The
//! actual delivery channel (how a JNI callback/iOS closure reaches Rust, and
//! how that becomes a Frust-facing listener API) is not built here.

mod types;

pub use types::{
    ActiveSubscription, FetchProductsResult, IapErrorCode, IapEvent, IapPlatform, IapPurchaseError,
    IapStore, Product, ProductOrSubscription, ProductQueryType, ProductRequest,
    ProductSubscription, ProductType, Purchase, PurchaseInput, PurchaseOptions, PurchaseState,
    RequestPurchaseProps, RequestPurchasePropsByPlatforms,
};

/// Errors from an `frust-iap` operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant rather than only displaying it. Variant naming follows
/// `frust-camera`'s [`CameraError`](../frust_camera/enum.CameraError.html)/
/// `frust-secure-storage`'s
/// [`SecureStorageError`](../frust_secure_storage/enum.SecureStorageError.html)
/// precedent (`PlatformNotInitialized`/`UiThread`/`NotAvailable`), plus two
/// variants specific to a purchase session's lifecycle (`NotConnected`,
/// `NotSupportedOnPlatform`) and one for a genuine store-reported failure
/// (`Store`).
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum IapError {
    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles this crate's Android backend needs
    /// (`frust-plugin`'s pre-init state) — an old scaffold predating
    /// `nativeInitPlatform`. Never a panic; the caller degrades gracefully.
    #[error("iap platform not initialized")]
    PlatformNotInitialized,

    /// A **blocking** call was made on the platform's UI thread, where
    /// parking on a platform/store answer deadlocks the thread that has to
    /// deliver it — the same fail-fast guard `frust-camera`'s
    /// `CameraError::UiThread` documents. Re-issue the call from
    /// `frust_reactive::spawn_blocking`.
    #[error(
        "iap call refused: this is a blocking call and was made on the UI thread — re-issue it \
         from `spawn_blocking`"
    )]
    UiThread,

    /// An operation that needs an open store connection (fetching products,
    /// requesting a purchase, restoring purchases) was called before
    /// `init_connection` (a later task's API) or after `end_connection`.
    #[error("iap store connection is not open — call `init_connection` first")]
    NotConnected,

    /// The requested operation has no counterpart on the current platform
    /// (an Android-only or iOS-only call made on the other platform, or a
    /// target this crate has no billing backend for at all — see
    /// [`Unavailability::UnsupportedPlatform`] for the whole-crate version of
    /// this gap).
    #[error("this iap operation is not supported on the current platform")]
    NotSupportedOnPlatform,

    /// The store itself reported a purchase failure (StoreKit/Play Billing),
    /// carried as the typed [`IapPurchaseError`] payload
    /// (`event.graphql`'s `PurchaseError`/`purchaseError` shape) rather than
    /// a generic string.
    #[error("{0}")]
    Store(#[from] IapPurchaseError),

    /// A JSON (de)serialization failure at the FFI boundary — a payload from
    /// the Kotlin/Swift host didn't parse into the expected [`types`] shape,
    /// or a Rust-side request failed to encode.
    #[error("iap (de)serialization error: {0}")]
    Serialization(String),

    /// IAP is unavailable for the given reason (see [`Unavailability`]) —
    /// distinct from [`Self::NotSupportedOnPlatform`], which is a single
    /// operation's platform mismatch rather than a whole-crate gap.
    #[error("iap unavailable: {0:?}")]
    NotAvailable(Unavailability),
}

/// Why IAP is unavailable (the [`IapError::NotAvailable`] payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unavailability {
    /// This platform has no IAP backend this crate drives. Desktop (macOS,
    /// Linux, Windows) is unconditionally in this state in v1 — a deliberate
    /// **deferral**, not a capability gap: macOS/Windows both ship their own
    /// store APIs (Mac App Store StoreKit, Microsoft Store), unlike
    /// `frust-haptics`'s desktop arm, which has no first-class API to route
    /// to at all (see `plugins/iap/Cargo.toml`'s desktop-stanza comment). Also
    /// reported for a build target this crate has no backend module for at
    /// all (not Android/iOS — e.g. tvOS, wasm).
    UnsupportedPlatform,
}
