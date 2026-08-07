//! `frust-iap`: an in-app-purchase plugin over Android Play Billing
//! (`openiap-google`) and iOS StoreKit (`openiap-apple`), speaking the
//! OpenIAP 3.0.1 wire protocol as JSON strings across the Rust<->Kotlin/Swift
//! FFI boundary.
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
//! # The store connection is an explicit, app-owned lifecycle
//!
//! Nothing else works until [`Iap::init_connection`] has opened the store
//! connection, and every store-touching call reports [`IapError::NotConnected`]
//! before it or after [`Iap::end_connection`]. Two contract decisions both
//! backends implement identically, so an app sees one story rather than
//! Play Billing's and StoreKit's:
//!
//! - **`init_connection` is idempotent.** A second call on an already-open
//!   connection is `Ok(true)`, never an error — a backend receiving the
//!   platform's own "already prepared" refusal maps it onto that success.
//! - **`end_connection` reports what it did.** `Ok(true)` when it closed an
//!   open connection, `Ok(false)` when none was open. Closing twice is not an
//!   error.
//!
//! # Blocking API — pair with `spawn_blocking`, never the UI thread
//!
//! Every call below waits on the store (a Play Billing round trip, a StoreKit
//! `async` query) and therefore blocks its caller. Like `frust-camera`'s
//! blocking calls, each pairs with `frust_reactive::spawn_blocking` — named
//! here as documentation only; the platform-plugin charter above forbids this
//! crate from actually depending on `frust-reactive`.
//!
//! | Call | Blocks until |
//! |---|---|
//! | [`Iap::init_connection`] / [`Iap::end_connection`] | the billing client is connected / torn down |
//! | [`Iap::fetch_products`] | the store answers the product query |
//! | [`Iap::get_available_purchases`] | the store answers the purchase query |
//! | [`Iap::get_active_subscriptions`] / [`Iap::has_active_subscriptions`] | the store answers the subscription query |
//! | [`Iap::get_storefront`] | the store reports the account's storefront |
//! | [`Iap::finish_transaction`] | the acknowledge/consume/finish call is accepted |
//! | [`Iap::acknowledge_purchase`] / [`Iap::consume_purchase`] | Play Billing accepts the token |
//! | [`Iap::get_pending_transactions`] | StoreKit answers the unfinished-transaction query |
//! | [`Iap::restore_purchases`] | the platform's restore/sync pass completes |
//! | [`Iap::deep_link_to_subscriptions`] | the platform accepts the deep link |
//!
//! **Never call one on the UI thread.** Every backend **fails fast** with
//! [`IapError::UiThread`] instead of parking there (Android:
//! `Looper.myLooper() == Looper.getMainLooper()`; iOS: the main thread), the
//! same guard `frust-camera` documents — a UI-thread caller would otherwise
//! park on the very queue that has to deliver its answer.
//!
//! Exactly two calls are **not** rows in the table and stay callable from
//! anywhere, including the UI thread: [`Iap::set_purchase_listener`] (pure
//! in-process bookkeeping, it never reaches the store) and
//! [`Iap::request_purchase`] (see below — Play Billing's `launchBillingFlow`
//! is itself a main-thread API, so its backend hops onto the Activity's
//! thread rather than refusing the caller).
//!
//! # A purchase is two-phase: an ack now, the outcome on the event stream
//!
//! [`Iap::request_purchase`] returns as soon as the store has **accepted** the
//! request. It never carries a purchase back. The result — success or failure
//! — arrives later as an [`IapEvent`] on the listener registered with
//! [`Iap::set_purchase_listener`], which is the **single source of truth** for
//! whether anything was bought. An app with no listener registered when the
//! flow completes has silently lost the purchase, so register one before the
//! first [`Iap::request_purchase`] and keep the returned [`ListenerHandle`]
//! alive.
//!
//! ## The revenue-safe sequence: request → event → verify → grant → finish
//!
//! The two phases above are five steps in practice, and skipping the middle
//! one is the actual purchase-fraud hole this API leaves open if you let it:
//!
//! 1. [`Iap::request_purchase`] asks the store to open the flow — ack-only,
//!    never a purchase.
//! 2. The outcome lands on the listener as an [`IapEvent`] —
//!    [`IapEvent::PurchaseUpdated`] carries a [`Purchase`] that is still
//!    **unverified client input** at this point, not a fact.
//! 3. **Verify the purchase server-side** — your server checks the token
//!    (Android: [`Purchase::purchase_token`] against the Play Developer API;
//!    iOS: the JWS carried in that same field) before anyone trusts it. See
//!    [`Purchase`]'s doc for exactly which fields a verifying server needs.
//! 4. Grant entitlement only once the server verdict is positive.
//! 5. Only then call [`Iap::finish_transaction`] — see its doc for why
//!    settling before verification forfeits the fraud check entirely.
//!
//! # Event delivery: one plugin-owned thread, and callbacks must not block
//!
//! Every [`IapEvent`] is delivered on **the** plugin-owned event thread — one
//! thread per process, shared by both platforms — **never** the UI thread, and
//! never the thread that called [`Iap::request_purchase`]. The hop is this
//! crate's own: each platform hands its event over on whatever thread it
//! reports on (Android: Play Billing's listener, which is the main thread;
//! iOS: OpenIAP's main-actor listener delivery), and the crate queues it
//! rather than running an app callback there. Because there is one consumer,
//! **events arrive in the order they were reported**, one at a time.
//!
//! The same heavy-work routing rule `frust-camera`'s image-stream callback
//! documents applies (`docs/CODE_STANDARDS.md`): a listener must not block,
//! must not do a signal write, and must not call back into [`Iap`]'s blocking
//! API from inside the callback — hand the event off
//! (`frust_reactive::use_task`, a channel, or a signal write scheduled back
//! onto the UI thread) and return immediately. Blocking that one thread stalls
//! every later event as well as every other listener. A listener that panics
//! is caught and logged rather than poisoning the registry or ending the
//! delivery thread, but a panic inside one is still a bug.
//!
//! # Backends
//!
//! [`Iap`] routes by `#[cfg(target_os = ...)]` to one backend: `android`
//! (Play Billing through `dev.frust.iap.FrustIapHost`), `apple` (iOS
//! StoreKit through the `FrustIapBridge` Swift glue), or [`desktop`]
//! (macOS/Linux/Windows — every call reports
//! [`IapError::NotAvailable`], a deliberate v1 deferral rather than a
//! capability gap; see that module's doc), plus a total-cover arm reporting
//! the same unavailability on any other target. The backend seam itself is a
//! crate-private trait exercised host-side by a `#[cfg(test)]` in-memory fake
//! store, so the connection state machine, the two-phase purchase contract,
//! and the listener semantics above are checked by `cargo test` on every
//! host rather than only on a device.

mod event;
mod types;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod desktop;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod mock;

pub use event::ListenerHandle;
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
/// precedent (`PlatformNotInitialized`/`UiThread`/`NotAvailable`/`Platform`),
/// plus two variants specific to a purchase session's lifecycle
/// (`NotConnected`, `NotSupportedOnPlatform`) and one for a genuine
/// store-reported failure (`Store`).
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum IapError {
    /// The platform's backend has no store handle to work with yet — two
    /// unrelated causes, one per platform: **Android** — the host shell
    /// never installed the `(JavaVM, Context)` platform handles this crate's
    /// backend needs (`frust-plugin`'s pre-init state), an old scaffold
    /// predating `nativeInitPlatform`; **iOS** — `bridge()` (`apple.rs`)
    /// couldn't resolve the `FrustIapBridge` Swift class or its `shared`
    /// singleton at runtime, because the Swift package isn't linked into the
    /// app or the linker dead-stripped it (nothing else in the app
    /// references it directly). Never a panic; the caller degrades
    /// gracefully.
    ///
    /// Checked **first**, ahead of every other guard — see [`Backend`]'s
    /// guard-order contract.
    #[error("iap platform not initialized")]
    PlatformNotInitialized,

    /// A **blocking** call was made on the platform's UI thread, where
    /// parking on a platform/store answer deadlocks the thread that has to
    /// deliver it — the same fail-fast guard `frust-camera`'s
    /// `CameraError::UiThread` documents. Re-issue the call from
    /// `frust_reactive::spawn_blocking`.
    ///
    /// A fail-fast guard, not a capability report: nothing about the store is
    /// wrong, only the calling thread.
    #[error(
        "iap call refused: this is a blocking call and was made on the UI thread — re-issue it \
         from `spawn_blocking`"
    )]
    UiThread,

    /// An operation that needs an open store connection (fetching products,
    /// requesting a purchase, restoring purchases) was called before
    /// [`Iap::init_connection`] or after [`Iap::end_connection`].
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
    /// the Kotlin/Swift host didn't parse into the expected wire shape, or a
    /// Rust-side request failed to encode.
    #[error("iap (de)serialization error: {0}")]
    Serialization(String),

    /// A backend-specific failure that is none of the above — a JNI error, a
    /// missing ObjC class, a host method that answered with nothing at all.
    /// The sibling plugins' `Platform(String)` variant
    /// (`CameraError::Platform`, `ClipboardError::Platform`,
    /// `HapticsError::Platform`), for the failures a typed variant would
    /// over-promise about.
    #[error("iap platform error: {0}")]
    Platform(String),

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

/// One backend implementation — the Android Play Billing backend, the Apple
/// StoreKit backend, [`desktop`]'s always-unavailable arm, or (under
/// `cfg(test)`) the in-memory fake store the conformance suite runs against.
///
/// Crate-private and deliberately mirrors [`Iap`]'s public surface one-to-one:
/// [`Iap`] is a thin cfg-dispatch wrapper over exactly one implementation per
/// target, the shape `frust-clipboard`/`frust-haptics` use. The one member
/// with no [`Iap`] counterpart is none — and the one [`Iap`] member with no
/// trait counterpart is [`Iap::set_purchase_listener`], which never reaches a
/// store.
///
/// # Guard order every implementation owes its caller
///
/// A store-touching method checks, **in this order, before doing any platform
/// work**:
///
/// 1. **Backend readiness** — the platform handles/host object this backend
///    needs exist at all. A plain atomic/`OnceLock` check, never a platform
///    round trip, so it holds even under `panic = "abort"`
///    (`docs/PLUGINS_ARCHITECTURE.md`'s pre-init convention) →
///    [`IapError::PlatformNotInitialized`].
/// 2. **The UI-thread guard** — the crate doc's *Blocking API* table →
///    [`IapError::UiThread`]. Live per-backend (Android's `Looper`, Apple's
///    main thread); an implementation with no UI thread to check (the fake
///    store) simply has no step 2.
/// 3. **The connection state** — [`Self::init_connection`] has run and
///    [`Self::end_connection`] has not → [`IapError::NotConnected`].
///
/// Reversing 1 and 2 would report a thread problem on a device that never had
/// the plugin installed; reversing 2 and 3 would park a UI-thread caller
/// inside a state check it was never allowed to make.
pub(crate) trait Backend: Send + Sync {
    /// See [`Iap::init_connection`].
    fn init_connection(&self, config: Option<&serde_json::Value>) -> Result<bool, IapError>;
    /// See [`Iap::end_connection`].
    fn end_connection(&self) -> Result<bool, IapError>;
    /// See [`Iap::fetch_products`].
    fn fetch_products(&self, request: &ProductRequest) -> Result<FetchProductsResult, IapError>;
    /// See [`Iap::get_available_purchases`].
    fn get_available_purchases(
        &self,
        options: Option<&PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError>;
    /// See [`Iap::get_active_subscriptions`].
    fn get_active_subscriptions(
        &self,
        ids: Option<&[String]>,
    ) -> Result<Vec<ActiveSubscription>, IapError>;
    /// See [`Iap::get_storefront`].
    fn get_storefront(&self) -> Result<String, IapError>;
    /// See [`Iap::request_purchase`].
    fn request_purchase(&self, props: &RequestPurchaseProps) -> Result<(), IapError>;
    /// See [`Iap::finish_transaction`].
    fn finish_transaction(
        &self,
        purchase: &PurchaseInput,
        is_consumable: Option<bool>,
    ) -> Result<(), IapError>;
    /// See [`Iap::restore_purchases`].
    fn restore_purchases(&self) -> Result<(), IapError>;
    /// See [`Iap::deep_link_to_subscriptions`].
    fn deep_link_to_subscriptions(
        &self,
        options: Option<&serde_json::Value>,
    ) -> Result<(), IapError>;

    /// See [`Iap::has_active_subscriptions`].
    ///
    /// Defaults to deriving the answer from [`Self::get_active_subscriptions`]
    /// so the two can never disagree — a backend whose host exposes a cheaper
    /// native query may override it, but must keep that equivalence.
    fn has_active_subscriptions(&self, ids: Option<&[String]>) -> Result<bool, IapError> {
        Ok(self
            .get_active_subscriptions(ids)?
            .iter()
            .any(|subscription| subscription.is_active))
    }

    /// See [`Iap::acknowledge_purchase`] — **Android-only**; the default arm
    /// is the answer every other platform owes ([`Iap`]'s platform-specific
    /// operations are trait defaults precisely so a backend states only what
    /// it *does* have).
    fn acknowledge_purchase(&self, purchase_token: &str) -> Result<(), IapError> {
        let _ = purchase_token;
        Err(IapError::NotSupportedOnPlatform)
    }
    /// See [`Iap::consume_purchase`] — **Android-only**, same default arm as
    /// [`Self::acknowledge_purchase`].
    fn consume_purchase(&self, purchase_token: &str) -> Result<(), IapError> {
        let _ = purchase_token;
        Err(IapError::NotSupportedOnPlatform)
    }
    /// See [`Iap::get_pending_transactions`] — **iOS-only**, same default arm
    /// as [`Self::acknowledge_purchase`].
    fn get_pending_transactions(&self) -> Result<Vec<Purchase>, IapError> {
        Err(IapError::NotSupportedOnPlatform)
    }
}

/// Run `call` against this target's backend.
///
/// The crate's single cfg-dispatch point — every [`Iap`] method routes
/// through it, rather than repeating a four-arm `#[cfg]` chain fifteen times
/// the way `frust-clipboard`'s three associated functions each do.
#[cfg(target_os = "android")]
fn with_backend<T>(call: impl FnOnce(&dyn Backend) -> Result<T, IapError>) -> Result<T, IapError> {
    call(&android::AndroidIap)
}

/// See the Android arm above.
#[cfg(target_os = "ios")]
fn with_backend<T>(call: impl FnOnce(&dyn Backend) -> Result<T, IapError>) -> Result<T, IapError> {
    call(&apple::AppleIap)
}

/// See the Android arm above.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn with_backend<T>(call: impl FnOnce(&dyn Backend) -> Result<T, IapError>) -> Result<T, IapError> {
    call(&desktop::DesktopIap)
}

/// Total-cover fallback: any target not one of the arms above (tvOS, wasm, …)
/// has no backend module compiled in at all — report it as a typed
/// unavailability rather than failing to compile with a confusing "no arm
/// produced a value" error (matches `frust-clipboard`'s own dispatch
/// functions). There is nothing to hand `call`, so it is dropped unrun.
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "linux",
    target_os = "windows"
)))]
fn with_backend<T>(call: impl FnOnce(&dyn Backend) -> Result<T, IapError>) -> Result<T, IapError> {
    let _ = call;
    Err(IapError::NotAvailable(Unavailability::UnsupportedPlatform))
}

/// The in-app-purchase entry point.
///
/// Carries no state — every operation is a plain associated function, matching
/// `frust-camera`'s `Camera` and `frust-clipboard`'s `Clipboard` (there is
/// exactly one store connection per process, no named-handle model).
///
/// **Every operation here blocks except [`Self::request_purchase`] and
/// [`Self::set_purchase_listener`]** — pair each with
/// `frust_reactive::spawn_blocking` and never call one on the UI thread; see
/// the crate doc's *Blocking API* table for the full matrix and the
/// [`IapError::UiThread`] fail-fast it is enforced by.
pub struct Iap;

impl Iap {
    /// Open the store connection. Everything else reports
    /// [`IapError::NotConnected`] until this succeeds.
    ///
    /// Idempotent: calling it on an already-open connection is `Ok(true)`, not
    /// an error (the crate doc's *store connection* section).
    ///
    /// `config` is an untyped passthrough, handed to the host verbatim. v1
    /// models no configuration type of its own because the only thing
    /// OpenIAP's `initConnection` config carries is platform-specific
    /// alternative-billing setup (Google Play's alternative billing / external
    /// offer modes), which is out of v1 scope — `None` is the v1 path, and a
    /// caller needing more can hand the raw JSON object the host's own
    /// generated types parse. A future typed `InitConnectionConfig` slots in
    /// behind the same `Option`, so passing `None` today is forward-compatible.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// [`IapError::PlatformNotInitialized`] on an Android scaffold predating
    /// `nativeInitPlatform`; [`IapError::UiThread`] when called on the
    /// platform's UI thread; [`IapError::NotAvailable`] on desktop and any
    /// other backend-less target; [`IapError::Store`]/[`IapError::Platform`]
    /// when the store itself refuses to connect (no Play Store on the device,
    /// a billing-unavailable account).
    pub fn init_connection(config: Option<serde_json::Value>) -> Result<bool, IapError> {
        with_backend(|backend| backend.init_connection(config.as_ref()))
    }

    /// Close the store connection opened by [`Self::init_connection`].
    ///
    /// `Ok(true)` when this call closed an open connection, `Ok(false)` when
    /// none was open — closing twice is not an error. Registered purchase
    /// listeners are **not** removed (they are in-process bookkeeping, see
    /// [`Self::set_purchase_listener`]), but nothing will emit onto them until
    /// the connection is opened again.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::init_connection`], minus the store-refusal cases.
    pub fn end_connection() -> Result<bool, IapError> {
        with_backend(|backend| backend.end_connection())
    }

    /// Fetch product metadata for the SKUs in `request`.
    ///
    /// The result envelope follows `request`'s
    /// [`kind`](ProductRequest::kind): [`ProductQueryType::InApp`] answers
    /// [`FetchProductsResult::Products`], [`ProductQueryType::Subs`] answers
    /// [`FetchProductsResult::Subscriptions`], and
    /// [`ProductQueryType::All`] answers [`FetchProductsResult::All`]. A SKU
    /// the store doesn't know is **omitted** from the result rather than
    /// reported as an error — compare the returned ids against the requested
    /// ones to detect a typo or an unpublished product.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// [`IapError::NotConnected`] before [`Self::init_connection`];
    /// [`IapError::Store`] when the store refuses the query (an empty SKU
    /// list, a developer configuration error); otherwise as
    /// [`Self::init_connection`].
    pub fn fetch_products(request: ProductRequest) -> Result<FetchProductsResult, IapError> {
        with_backend(|backend| backend.fetch_products(&request))
    }

    /// Every purchase currently available to the signed-in account —
    /// unconsumed one-time purchases and active subscriptions.
    ///
    /// This is the restore/entitlement query an app runs at launch to decide
    /// what the user owns; it is **not** how a fresh purchase arrives (that is
    /// the event stream — see [`Self::request_purchase`]).
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::fetch_products`].
    pub fn get_available_purchases(
        options: Option<PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError> {
        with_backend(|backend| backend.get_available_purchases(options.as_ref()))
    }

    /// The account's subscriptions, optionally narrowed to `ids`.
    ///
    /// `None` asks for every subscription the store reports. An entry's
    /// [`is_active`](ActiveSubscription::is_active) is what decides
    /// entitlement — a returned entry is not automatically an active one.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::fetch_products`].
    pub fn get_active_subscriptions(
        ids: Option<Vec<String>>,
    ) -> Result<Vec<ActiveSubscription>, IapError> {
        with_backend(|backend| backend.get_active_subscriptions(ids.as_deref()))
    }

    /// Whether any of `ids` (or any subscription at all, for `None`) is
    /// currently active — the boolean shortcut over
    /// [`Self::get_active_subscriptions`], and always consistent with it.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::fetch_products`].
    pub fn has_active_subscriptions(ids: Option<Vec<String>>) -> Result<bool, IapError> {
        with_backend(|backend| backend.has_active_subscriptions(ids.as_deref()))
    }

    /// The storefront the signed-in account is billed through, as the
    /// platform's own country code (iOS: StoreKit's three-letter
    /// `Storefront.countryCode`, e.g. `"USA"`; Android: Play's two-letter
    /// billing country, e.g. `"US"`). **Not** normalized between the two —
    /// the raw platform value is more useful than a lossy common denominator.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::fetch_products`].
    pub fn get_storefront() -> Result<String, IapError> {
        with_backend(|backend| backend.get_storefront())
    }

    /// Ask the store to start a purchase flow.
    ///
    /// # This call's `Ok(())` means "the store accepted the request" — nothing more
    ///
    /// **It never reports whether anything was bought.** The outcome arrives
    /// later as an [`IapEvent`] on the listener registered with
    /// [`Self::set_purchase_listener`], which is the **single source of
    /// truth**: [`IapEvent::PurchaseUpdated`] carries the [`Purchase`],
    /// [`IapEvent::PurchaseError`] carries the failure (including the user
    /// simply cancelling, [`IapErrorCode::UserCancelled`]). A purchase
    /// completed while no listener was registered is **lost to the app** until
    /// its next [`Self::get_available_purchases`] — register the listener
    /// first, and keep its [`ListenerHandle`] alive.
    ///
    /// The event's [`Purchase`] is **unverified input** — verify it
    /// server-side before granting entitlement; see
    /// [`Self::finish_transaction`] for where that check sits in the
    /// sequence. Never grant off this call's own return either, which
    /// reports only that the store accepted the request, not that anything
    /// was bought.
    ///
    /// Non-blocking, and callable from any thread including the UI thread: the
    /// backend reaches whatever thread the platform demands itself (Play
    /// Billing's `launchBillingFlow` is a main-thread API).
    ///
    /// # Errors
    /// [`IapError::NotConnected`] before [`Self::init_connection`];
    /// [`IapError::Store`] when the store rejects the request outright (an
    /// unknown SKU, an already-owned non-consumable) rather than opening a
    /// flow; otherwise as [`Self::init_connection`], minus
    /// [`IapError::UiThread`] which this call never reports.
    pub fn request_purchase(props: RequestPurchaseProps) -> Result<(), IapError> {
        with_backend(|backend| backend.request_purchase(&props))
    }

    /// Settle a purchase received from the event stream: acknowledge it
    /// (non-consumable/subscription) or consume it (consumable, making it
    /// buyable again).
    ///
    /// # This is step 5 — after server-verify and after granting
    ///
    /// Call this only once your server has verified the purchase (the crate
    /// doc's *revenue-safe sequence*) and entitlement has been durably
    /// granted. Finishing an unverified purchase acknowledges/consumes it on
    /// the store's side, which forfeits the fraud check — the store
    /// considers the transaction settled either way, and there is no way to
    /// claw it back after the fact.
    ///
    /// # Play's 3-day acknowledgement deadline
    ///
    /// **An Android purchase that is not acknowledged or consumed within 3
    /// days is automatically refunded and revoked by Google Play.** The
    /// countdown starts when the purchase completes, not when the app next
    /// launches — so settle every purchase as soon as the app has durably
    /// recorded the entitlement, including purchases learned from
    /// [`Self::get_available_purchases`] at startup rather than from a live
    /// event. iOS has no equivalent deadline, but an unfinished StoreKit
    /// transaction is re-delivered to the transaction listener forever until
    /// it is finished.
    ///
    /// `is_consumable` picks between the two settlements. `None` lets the
    /// backend decide from the purchase itself (Android: acknowledge; iOS:
    /// `finish`, which is the only settlement StoreKit has) — pass
    /// `Some(true)` for a consumable, or a second purchase of the same SKU
    /// will be refused as already-owned on Android.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// [`IapError::Store`] when the store rejects the settlement (an unknown
    /// or already-settled transaction); otherwise as [`Self::fetch_products`].
    pub fn finish_transaction(
        purchase: PurchaseInput,
        is_consumable: Option<bool>,
    ) -> Result<(), IapError> {
        with_backend(|backend| backend.finish_transaction(&purchase, is_consumable))
    }

    /// Ask the platform to re-sync the account's purchases (iOS:
    /// StoreKit's `sync`, which may prompt for the App Store password;
    /// Android: a fresh purchase query against Play).
    ///
    /// Like [`Self::request_purchase`], anything this recovers surfaces on the
    /// event stream and through [`Self::get_available_purchases`] — this call
    /// itself returns no purchases.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::fetch_products`].
    pub fn restore_purchases() -> Result<(), IapError> {
        with_backend(|backend| backend.restore_purchases())
    }

    /// Open the platform's own subscription-management UI (iOS: the App
    /// Store's manage-subscriptions sheet; Android: the Play Store's
    /// subscription page for this package), leaving the app.
    ///
    /// `opts` is an untyped passthrough for the platform's own options (Play's
    /// `sku`/`packageName` pair, which deep-links to one subscription instead
    /// of the list). v1 models no options type of its own, for the same reason
    /// [`Self::init_connection`]'s `config` doesn't: the fields are
    /// platform-specific and `None` is the v1 path.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::fetch_products`].
    pub fn deep_link_to_subscriptions(opts: Option<serde_json::Value>) -> Result<(), IapError> {
        with_backend(|backend| backend.deep_link_to_subscriptions(opts.as_ref()))
    }

    /// Acknowledge a Play Billing purchase by its purchase token — the
    /// lower-level half of [`Self::finish_transaction`], for an app that
    /// already holds a token (e.g. one its server verified) rather than a
    /// whole [`PurchaseInput`].
    ///
    /// **Android-only.** iOS and desktop report
    /// [`IapError::NotSupportedOnPlatform`]; StoreKit has no acknowledgement
    /// step at all. See [`Self::finish_transaction`]'s 3-day deadline — it
    /// governs this call too.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// [`IapError::NotSupportedOnPlatform`] off Android; otherwise as
    /// [`Self::finish_transaction`].
    pub fn acknowledge_purchase(purchase_token: &str) -> Result<(), IapError> {
        with_backend(|backend| backend.acknowledge_purchase(purchase_token))
    }

    /// Consume a Play Billing purchase by its purchase token, making the SKU
    /// buyable again — the consumable counterpart to
    /// [`Self::acknowledge_purchase`].
    ///
    /// **Android-only.** iOS and desktop report
    /// [`IapError::NotSupportedOnPlatform`].
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// As [`Self::acknowledge_purchase`].
    pub fn consume_purchase(purchase_token: &str) -> Result<(), IapError> {
        with_backend(|backend| backend.consume_purchase(purchase_token))
    }

    /// StoreKit transactions that have not been finished yet — purchases
    /// delivered to the transaction listener whose
    /// [`Self::finish_transaction`] never ran (an app killed mid-flow, an
    /// entitlement grant that failed). StoreKit re-delivers these forever
    /// until they are finished.
    ///
    /// **iOS-only.** Android and desktop report
    /// [`IapError::NotSupportedOnPlatform`]; Play's equivalent is
    /// [`Self::get_available_purchases`], where an unacknowledged purchase
    /// simply shows up like any other.
    ///
    /// **Blocking** — pair with `frust_reactive::spawn_blocking`.
    ///
    /// # Errors
    /// [`IapError::NotSupportedOnPlatform`] off iOS; otherwise as
    /// [`Self::fetch_products`].
    pub fn get_pending_transactions() -> Result<Vec<Purchase>, IapError> {
        with_backend(|backend| backend.get_pending_transactions())
    }

    /// Register `callback` to receive every [`IapEvent`] this process
    /// observes, and return the handle that keeps it registered.
    ///
    /// # The returned handle is the registration
    ///
    /// **Dropping the [`ListenerHandle`] unregisters the callback** — so
    /// `let _ = Iap::set_purchase_listener(cb);` unregisters it immediately,
    /// on the very next statement. Store the handle for as long as the
    /// listener should live (an app-lifetime listener belongs in a
    /// process-lifetime slot); [`ListenerHandle::remove`] is the explicit form
    /// of the same unregistration.
    ///
    /// # Threading contract
    ///
    /// `callback` runs on the plugin's own event-delivery thread — one thread
    /// for the whole process, never the UI thread and never the caller's,
    /// hence `Send + Sync`. It **must not block, must not do a signal write,
    /// and must not call back into [`Iap`]'s blocking API**: hand the event
    /// off (`frust_reactive::use_task`, a channel, a signal write scheduled
    /// onto the UI thread) and return immediately, the same rule
    /// `frust-camera`'s image-stream callback documents — that one thread is
    /// what every later event queues behind. Multiple listeners are supported
    /// and each sees every event, in registration order, in the order the
    /// platform reported the events.
    ///
    /// Registration reaches no store and cannot fail — hence no `Result` —
    /// and stays valid across [`Self::end_connection`]/[`Self::init_connection`]
    /// cycles.
    pub fn set_purchase_listener(callback: Box<dyn Fn(IapEvent) + Send + Sync>) -> ListenerHandle {
        event::register(callback)
    }
}
