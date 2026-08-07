//! The v1 serde type subset mirroring the OpenIAP 3.0.1 JSON wire shapes
//! (`tmp/openiap/packages/gql/src/*.graphql`'s schema, cross-checked against
//! the *generated* Kotlin/Swift `toJson()`/`fromJson()` output — the actual
//! wire truth, not the GraphQL schema alone; see each type's own doc for its
//! source).
//!
//! # Field casing
//!
//! Every type is `#[serde(rename_all = "camelCase")]` (a bare `#[derive]`
//! away from the wire's own camelCase field names), with per-field
//! `#[serde(rename = "...")]` only where a Rust keyword (`type`) or an
//! enum-variant-name mismatch needs it — see each type for specifics.
//!
//! # Enum values are kebab-case on the wire, not the GraphQL schema's SCREAMING_CASE
//!
//! `error.graphql`/`type.graphql` spell enum members `UserCancelled`/
//! `InApp`/... (GraphQL convention), but the *generated* Kotlin/Swift
//! `toJson()` emits their `rawValue` — kebab-case (`"user-cancelled"`,
//! `"in-app"`) — which is what actually crosses the wire. Every enum below
//! uses `#[serde(rename_all = "kebab-case")]` to match that, verified against
//! `Types.kt`'s per-enum `rawValue` table for every variant (see this
//! module's tests).
//!
//! # Union types: common fields + lossless `extra`
//!
//! The wire's `Product`/`ProductSubscription`/`Purchase` unions
//! (`type.graphql`) resolve to a platform-tagged concrete type
//! (`ProductAndroid`/`ProductIOS`/…, `__typename`-tagged — see
//! `OpenIapSerialization.swift`, which stamps `__typename` in from the
//! concrete Swift type name at serialize time, and each Kotlin data class's
//! own `toJson()`, which does the same). Rather than a Rust enum per union
//! (which would need `#[serde(tag = "__typename")]` — a combination with
//! per-variant `#[serde(flatten)]` that has a known serde pitfall: the
//! internally-tagged enum's own tag-writing step and a flattened field that
//! *also* captured the wire's `__typename` on deserialize would each write
//! the key independently on serialize, producing a duplicate-key object),
//! each of these three is a **plain struct**: the fields both platform
//! variants share (`ProductCommon`/`PurchaseCommon` in `type.graphql`) are
//! strongly typed, and every remaining field — every `*Android`/`*IOS`-suffixed
//! platform-specific field, *and* the `__typename` discriminator itself,
//! since it is not one of the named fields below — lands in `extra` via
//! `#[serde(flatten)] extra: serde_json::Map<String, serde_json::Value>`.
//! This sidesteps the pitfall above entirely (nothing here writes the tag a
//! second time) while still losslessly round-tripping every wire field.
//! Access a platform-specific value via `extra.get("nameAndroid")` etc.; the
//! typed `platform`/`kind` fields already common to both variants are enough
//! to branch on Android vs iOS or product-vs-subscription without reading
//! `extra["__typename"]`, though it is there too.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Mirrors `type.graphql`'s `enum IapPlatform` (`IOS`/`Android` on the
/// GraphQL schema; wire values `"ios"`/`"android"` per `Types.kt`'s
/// `IapPlatform.rawValue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IapPlatform {
    Ios,
    Android,
}

/// Mirrors `type.graphql`'s `enum ProductType` (`InApp`/`Subs`; wire values
/// `"in-app"`/`"subs"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProductType {
    InApp,
    Subs,
}

/// Mirrors `type.graphql`'s `enum PurchaseState` (`Pending`/`Purchased`/
/// `Unknown`; wire values `"pending"`/`"purchased"`/`"unknown"`).
///
/// `#[serde(other)]` on `Unknown` additionally catches any wire value this
/// crate doesn't recognize (a future spec addition), matching this crate's
/// forward-compatibility stance for closed store-vocabulary enums (see
/// [`IapErrorCode`]'s own `Unknown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PurchaseState {
    Pending,
    Purchased,
    #[serde(other)]
    Unknown,
}

/// Mirrors `type.graphql`'s `enum IapStore` (wire values `"unknown"`/
/// `"apple"`/`"google"`/`"horizon"`/`"amazon"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IapStore {
    Unknown,
    Apple,
    Google,
    Horizon,
    Amazon,
}

/// Mirrors `type.graphql`'s `enum ProductQueryType` (wire values
/// `"in-app"`/`"subs"`/`"all"`) — [`ProductRequest::kind`]'s/
/// [`RequestPurchaseProps::kind`]'s type. Defaults to [`Self::InApp`],
/// matching the GraphQL schema's `type: ProductQueryType = InApp` default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProductQueryType {
    #[default]
    InApp,
    Subs,
    All,
}

/// Mirrors `error.graphql`'s `enum ErrorCode` — all 36 spec codes, wire
/// values kebab-case per `Types.kt`'s `ErrorCode.rawValue` table (every
/// variant verified against it; see this module's tests).
///
/// `#[serde(other)] Unknown` additionally catches any code string this crate
/// doesn't recognize (a spec addition ahead of this crate's own update),
/// rather than a hard deserialization error — the same forward-compatibility
/// stance `Types.kt`'s own `fromJson` takes by accepting kebab-case,
/// `SCREAMING_CASE`, *and* `PascalCase` spellings (this crate only accepts
/// the canonical kebab-case wire form; the other two spellings are
/// react-native-iap/legacy-library compatibility shims Kotlin/Swift carry
/// that this crate's own host glue, not this type, is responsible for if it
/// ever needs them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IapErrorCode {
    UserCancelled,
    UserError,
    ItemUnavailable,
    RemoteError,
    NetworkError,
    ServiceError,
    PurchaseVerificationFailed,
    PurchaseVerificationFinished,
    PurchaseVerificationFinishFailed,
    NotPrepared,
    NotEnded,
    AlreadyOwned,
    DeveloperError,
    BillingResponseJsonParseError,
    DeferredPayment,
    Interrupted,
    IapNotAvailable,
    PurchaseError,
    SyncError,
    TransactionValidationFailed,
    ActivityUnavailable,
    AlreadyPrepared,
    Pending,
    ConnectionClosed,
    InitConnection,
    ServiceDisconnected,
    ServiceTimeout,
    QueryProduct,
    SkuNotFound,
    SkuOfferMismatch,
    ItemNotOwned,
    BillingUnavailable,
    FeatureNotSupported,
    EmptySkuList,
    DuplicatePurchase,
    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Product / Purchase unions (common fields + lossless `extra`)
// ---------------------------------------------------------------------------

/// Mirrors `type.graphql`'s `union Product = ProductAndroid | ProductIOS`
/// (the `ProductCommon` interface's fields, strongly typed) — see this
/// module's doc for the common-fields-plus-`extra` strategy every field not
/// listed here (`nameAndroid`, `displayNameIOS`, `typeIOS`,
/// `jsonRepresentationIOS`, `discountOffers`, `subscriptionOffers`,
/// `debugDescription`, `displayName`, `__typename`, …) falls into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Product {
    pub id: String,
    pub title: String,
    pub description: String,
    pub price: Option<f64>,
    pub display_price: String,
    pub currency: String,
    #[serde(rename = "type")]
    pub kind: ProductType,
    pub platform: IapPlatform,
    /// Every wire field not named above, losslessly — including
    /// `__typename` (`"ProductAndroid"`/`"ProductIOS"`). Re-serializing a
    /// `Product` reproduces every extra field verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Mirrors `type.graphql`'s `union ProductSubscription =
/// ProductSubscriptionAndroid | ProductSubscriptionIOS` — the same
/// `ProductCommon` common-fields-plus-`extra` shape as [`Product`] (this
/// module's doc); [`Self::kind`] is [`ProductType::Subs`] on every real wire
/// payload, unlike [`Product`]'s [`ProductType::InApp`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductSubscription {
    pub id: String,
    pub title: String,
    pub description: String,
    pub price: Option<f64>,
    pub display_price: String,
    pub currency: String,
    #[serde(rename = "type")]
    pub kind: ProductType,
    pub platform: IapPlatform,
    /// See [`Product::extra`].
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Mirrors `type.graphql`'s `union ProductOrSubscription = Product |
/// ProductSubscription` (`FetchProductsResult::All`'s element type).
///
/// A type alias, not a third struct: [`Product`] and [`ProductSubscription`]
/// already share an identical common-field shape (both mirror
/// `ProductCommon`), and [`Product::kind`] (`ProductType::InApp` vs
/// `ProductType::Subs`) is the wire's own authoritative discriminator for
/// which of the two a given payload is — an `Ok`-either-way alias reflects
/// that faithfully without inventing a redundant enum wrapper a caller would
/// have to unwrap for no informational gain.
pub type ProductOrSubscription = Product;

/// Mirrors `type.graphql`'s `union Purchase = PurchaseAndroid | PurchaseIOS`
/// (the `PurchaseCommon` interface's fields, strongly typed) — the same
/// common-fields-plus-`extra` strategy as [`Product`] (this module's doc).
/// Every platform-specific field (`transactionId`, `signatureAndroid`,
/// `offerIOS`, `revocationReasonIOS`, `__typename`, …) falls into
/// [`Self::extra`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Purchase {
    pub id: String,
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    pub transaction_date: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_token: Option<String>,
    pub store: IapStore,
    pub quantity: i32,
    pub purchase_state: PurchaseState,
    pub is_auto_renewing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_plan_id: Option<String>,
    /// See [`Product::extra`].
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Errors / events
// ---------------------------------------------------------------------------

/// Mirrors `error.graphql`'s `type PurchaseError`, narrowed to the fields
/// this crate's v1 surface needs (`code`/`message`/`productId`) — the wire
/// payload also carries `debugMessage`/`responseCode`/
/// `subResponseCodeAndroid`/`productIds`/`productType`/`isEmptyProductList`
/// (see `error.graphql`), silently dropped on deserialize rather than
/// captured in an `extra` map: unlike [`Product`]/[`Purchase`], this crate
/// does not promise lossless round-tripping for this type (approved-extras
/// scope, this task's Notes).
///
/// Also a [`std::error::Error`] (`thiserror`-derived) so [`crate::IapError::Store`]
/// can wrap it directly via `#[from]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("iap store error ({code:?}): {message}")]
pub struct IapPurchaseError {
    pub code: IapErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<String>,
}

/// The purchase-update event vocabulary a later task's listener API emits —
/// mirrors `event.graphql`'s `purchaseUpdated`/`purchaseError` subscription
/// fields (`Purchase!`/`PurchaseError!` payloads respectively). Adjacently
/// tagged (`{"event": "purchaseUpdated", "data": {...}}` /
/// `{"event": "purchaseError", "data": {...}}`) — this crate's own designed
/// envelope for a *single* combined event stream, since `event.graphql`
/// models the two as separate GraphQL subscription fields rather than one
/// tagged union; both mobile hosts (a later task) emit onto this shape, the
/// same "this task designs the envelope, both hosts implement it" contract
/// [`FetchProductsResult`] uses for its own designed envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum IapEvent {
    PurchaseUpdated(Purchase),
    PurchaseError(IapPurchaseError),
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Mirrors `type.graphql`'s `input ProductRequest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductRequest {
    pub skus: Vec<String>,
    #[serde(rename = "type", default)]
    pub kind: ProductQueryType,
}

/// Mirrors `type.graphql`'s `input RequestPurchasePropsByPlatforms` (also
/// reused for `input RequestSubscriptionPropsByPlatforms`, which has the
/// identical `{apple, google}` shape). The per-platform payloads
/// (`RequestPurchaseIosProps`/`RequestPurchaseAndroidProps`/
/// `RequestSubscriptionIosProps`/`RequestSubscriptionAndroidProps` —
/// `type-ios.graphql`/`type-android.graphql`) are **not** modeled in v1
/// (approved-extras scope, this task's Notes): a caller builds the raw JSON
/// object matching the target platform's field names directly, which the
/// Kotlin/Swift host (a later task) parses with its own generated types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestPurchasePropsByPlatforms {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apple: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google: Option<serde_json::Value>,
}

/// Mirrors `type.graphql`'s `input RequestPurchaseProps`. `Types.kt`'s own
/// `RequestPurchaseProps` collapses `requestPurchase`/`requestSubscription`
/// into one internal `Request` sum type with a same-class invariant tying
/// `type` to whichever branch is populated (`InApp` with `requestPurchase`,
/// `Subs` with `requestSubscription`); this crate instead mirrors the raw
/// GraphQL input shape directly — both fields present as `Option`s, exactly
/// one expected populated by convention — leaving that invariant to the
/// caller/host rather than encoding it in the type, since enforcing it here
/// would need the same sum-type restructuring `Types.kt` did internally,
/// out of scope for this task's skeleton.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPurchaseProps {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_purchase: Option<RequestPurchasePropsByPlatforms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_subscription: Option<RequestPurchasePropsByPlatforms>,
    #[serde(rename = "type", default)]
    pub kind: ProductQueryType,
}

/// Mirrors `type.graphql`'s `input PurchaseOptions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub also_publish_to_event_listener_ios: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_suspended_android: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_include_active_items_ios: Option<bool>,
}

/// Mirrors `type.graphql`'s `input PurchaseInput` — what a later task's
/// `finish_transaction` sends back to acknowledge/consume a purchase.
/// `Types.kt` aliases its own `PurchaseInput` directly onto the `Purchase`
/// sealed interface (`public typealias PurchaseInput = Purchase`); this
/// crate instead mirrors the GraphQL input shape's own, narrower field list
/// (no platform-specific extras — a purchase-finishing call only needs
/// enough to identify the transaction, not its full wire payload back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseInput {
    pub id: String,
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    pub transaction_date: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<IapStore>,
    pub quantity: i32,
    pub purchase_state: PurchaseState,
    pub is_auto_renewing: bool,
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// Mirrors `type.graphql`'s `type FetchProductsResult { products,
/// subscriptions, all }` — but `Types.kt` has **no `toJson()`/`fromJson()`
/// for this type** (its Kotlin/Swift representation is a sealed
/// interface/enum the native host hand-maps per call site, not a
/// (de)serializable wire object). This crate therefore **designs** the wire
/// envelope here, adjacently tagged on which of the three GraphQL fields is
/// populated: `{"type": "products", "items": [...]}` /
/// `{"type": "subscriptions", "items": [...]}` / `{"type": "all", "items":
/// [...]}`. Both mobile hosts (tasks 05/06) implement this exact shape when
/// answering a `fetchProducts` call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "items", rename_all = "camelCase")]
pub enum FetchProductsResult {
    Products(Vec<Product>),
    Subscriptions(Vec<ProductSubscription>),
    All(Vec<ProductOrSubscription>),
}

/// Mirrors `type.graphql`'s `type ActiveSubscription`. `renewalInfoIOS`
/// (StoreKit 2 renewal metadata, `RenewalInfoIOS`) is intentionally **not**
/// modeled as its own type in v1 (approved-extras scope, this task's Notes)
/// — captured as raw JSON rather than dropped, since (unlike
/// [`IapPurchaseError`]'s narrower fields) there is no reason to lose it
/// outright when a future task can start reading it without a wire-shape
/// change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSubscription {
    pub product_id: String,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration_date_ios: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_renewing_android: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_ios: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_until_expiration_ios: Option<f64>,
    pub transaction_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_token: Option<String>,
    pub transaction_date: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_plan_id_android: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_token_android: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_info_ios: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Parses a hand-written JSON literal into a `Value` — used for the
    /// larger wire-shape fixtures below instead of the `json!` macro, which
    /// hits rustc's default macro recursion limit on an object with this
    /// many keys.
    fn parse(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).expect("fixture is valid JSON")
    }

    /// Hand-derived from `Types.kt`'s `ProductAndroid.toJson()` field list
    /// (`packages/google/openiap/.../Types.kt`), spellings copied exactly.
    #[test]
    fn product_android_round_trips_losslessly() {
        let raw = parse(
            r#"{
                "__typename": "ProductAndroid",
                "currency": "USD",
                "debugDescription": null,
                "description": "Remove ads and unlock all features",
                "discountOffers": null,
                "displayName": null,
                "displayPrice": "$4.99",
                "id": "premium_upgrade",
                "nameAndroid": "Premium Upgrade",
                "platform": "android",
                "price": 4.99,
                "productStatusAndroid": "ok",
                "subscriptionOffers": null,
                "title": "Premium Upgrade",
                "type": "in-app"
            }"#,
        );
        let product: Product = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(product.id, "premium_upgrade");
        assert_eq!(product.title, "Premium Upgrade");
        assert_eq!(product.display_price, "$4.99");
        assert_eq!(product.price, Some(4.99));
        assert_eq!(product.kind, ProductType::InApp);
        assert_eq!(product.platform, IapPlatform::Android);
        // Platform-specific + __typename fields land in `extra`.
        assert_eq!(
            product.extra.get("__typename"),
            Some(&json!("ProductAndroid"))
        );
        assert_eq!(
            product.extra.get("nameAndroid"),
            Some(&json!("Premium Upgrade"))
        );
        assert_eq!(
            product.extra.get("productStatusAndroid"),
            Some(&json!("ok"))
        );

        let round_tripped = serde_json::to_value(&product).unwrap();
        assert_eq!(round_tripped, raw);
    }

    /// Hand-derived from `Types.kt`'s `ProductIOS.toJson()` field list.
    #[test]
    fn product_ios_round_trips_losslessly() {
        let raw = parse(
            r#"{
                "__typename": "ProductIOS",
                "currency": "USD",
                "debugDescription": null,
                "description": "Remove ads and unlock all features",
                "displayName": null,
                "displayNameIOS": "Premium Upgrade",
                "displayPrice": "$4.99",
                "id": "premium_upgrade",
                "isFamilyShareableIOS": false,
                "jsonRepresentationIOS": "{}",
                "platform": "ios",
                "price": 4.99,
                "pricingTermsIOS": null,
                "subscriptionOffers": null,
                "title": "Premium Upgrade",
                "type": "in-app",
                "typeIOS": "consumable"
            }"#,
        );
        let product: Product = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(product.platform, IapPlatform::Ios);
        assert_eq!(product.extra.get("typeIOS"), Some(&json!("consumable")));
        assert_eq!(
            product.extra.get("isFamilyShareableIOS"),
            Some(&json!(false))
        );

        let round_tripped = serde_json::to_value(&product).unwrap();
        assert_eq!(round_tripped, raw);
    }

    /// Hand-derived from `Types.kt`'s `ProductSubscriptionAndroid.toJson()`.
    #[test]
    fn product_subscription_android_round_trips_losslessly() {
        let raw = parse(
            r#"{
                "__typename": "ProductSubscriptionAndroid",
                "currency": "USD",
                "debugDescription": null,
                "description": "Monthly subscription",
                "displayName": null,
                "displayPrice": "$9.99/month",
                "id": "monthly_premium",
                "nameAndroid": "Monthly Premium",
                "platform": "android",
                "price": 9.99,
                "productStatusAndroid": "ok",
                "subscriptionOffers": [],
                "title": "Monthly Premium",
                "type": "subs"
            }"#,
        );
        let subscription: ProductSubscription = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(subscription.kind, ProductType::Subs);
        assert_eq!(subscription.id, "monthly_premium");

        let round_tripped = serde_json::to_value(&subscription).unwrap();
        assert_eq!(round_tripped, raw);
    }

    /// `ProductOrSubscription` is `Product` — a subscription payload
    /// deserializes through it identically to `ProductSubscription`.
    #[test]
    fn product_or_subscription_accepts_a_subscription_payload() {
        let raw = parse(
            r#"{
                "__typename": "ProductSubscriptionAndroid",
                "currency": "USD",
                "description": "Monthly subscription",
                "displayPrice": "$9.99/month",
                "id": "monthly_premium",
                "nameAndroid": "Monthly Premium",
                "platform": "android",
                "price": 9.99,
                "title": "Monthly Premium",
                "type": "subs"
            }"#,
        );
        let value: ProductOrSubscription = serde_json::from_value(raw).unwrap();
        assert_eq!(value.kind, ProductType::Subs);
        assert_eq!(
            value.extra.get("__typename"),
            Some(&json!("ProductSubscriptionAndroid"))
        );
    }

    /// Hand-derived from `Types.kt`'s `PurchaseAndroid.toJson()`.
    #[test]
    fn purchase_android_round_trips_losslessly() {
        let raw = parse(
            r#"{
                "__typename": "PurchaseAndroid",
                "autoRenewingAndroid": true,
                "currentPlanId": "premium-month",
                "dataAndroid": "purchase-data-blob",
                "developerPayloadAndroid": null,
                "id": "GPA.1234-5678-9012-34567",
                "ids": ["GPA.1234-5678-9012-34567"],
                "isAcknowledgedAndroid": true,
                "isAutoRenewing": true,
                "isSuspendedAndroid": false,
                "obfuscatedAccountIdAndroid": null,
                "obfuscatedProfileIdAndroid": null,
                "packageNameAndroid": "it.f0x.huddle",
                "pendingPurchaseUpdateAndroid": null,
                "productId": "monthly_premium",
                "purchaseState": "purchased",
                "purchaseToken": "opaque-token-value",
                "quantity": 1,
                "signatureAndroid": "signature-blob",
                "store": "google",
                "transactionDate": 1738000000000.0,
                "transactionId": "GPA.1234-5678-9012-34567",
                "userIdAmazon": null,
                "userMarketplaceAmazon": null
            }"#,
        );
        let purchase: Purchase = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(purchase.id, "GPA.1234-5678-9012-34567");
        assert_eq!(purchase.product_id, "monthly_premium");
        assert_eq!(purchase.store, IapStore::Google);
        assert_eq!(purchase.purchase_state, PurchaseState::Purchased);
        assert!(purchase.is_auto_renewing);
        assert_eq!(
            purchase.extra.get("signatureAndroid"),
            Some(&json!("signature-blob"))
        );

        let round_tripped = serde_json::to_value(&purchase).unwrap();
        assert_eq!(round_tripped, raw);
    }

    /// Hand-derived from `Types.kt`'s `PurchaseIOS.toJson()`.
    #[test]
    fn purchase_ios_round_trips_losslessly() {
        let raw = parse(
            r#"{
                "__typename": "PurchaseIOS",
                "advancedCommerceInfoIOS": null,
                "appAccountToken": null,
                "appBundleIdIOS": "it.f0x.huddle",
                "billingPlanTypeIOS": null,
                "bundleOriginalTransactionIdIOS": null,
                "bundleProductIdIOS": null,
                "bundleSubscriptionGroupIdIOS": null,
                "bundleTransactionIdIOS": null,
                "commitmentInfoIOS": null,
                "countryCodeIOS": "US",
                "currencyCodeIOS": "USD",
                "currencySymbolIOS": "$",
                "currentPlanId": "premium_monthly",
                "environmentIOS": "Production",
                "expirationDateIOS": null,
                "id": "2000000123456789",
                "ids": ["2000000123456789"],
                "isAutoRenewing": true,
                "isUpgradedIOS": false,
                "offerIOS": null,
                "originalTransactionDateIOS": 1738000000000.0,
                "originalTransactionIdentifierIOS": "2000000123456789",
                "ownershipTypeIOS": "PURCHASED",
                "previousOriginalTransactionIdIOS": null,
                "productId": "premium_monthly",
                "purchaseState": "purchased",
                "purchaseToken": "jws-token-value",
                "quantity": 1,
                "quantityIOS": 1,
                "reasonIOS": null,
                "reasonStringRepresentationIOS": null,
                "renewalInfoIOS": null,
                "revocationDateIOS": null,
                "revocationReasonIOS": null,
                "revocationTypeIOS": null,
                "store": "apple",
                "storefrontCountryCodeIOS": "USA",
                "subscriptionGroupIdIOS": "21400000",
                "transactionDate": 1738000000000.0,
                "transactionId": "2000000123456789",
                "transactionReasonIOS": "PURCHASE",
                "webOrderLineItemIdIOS": "1000000098765432"
            }"#,
        );
        let purchase: Purchase = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(purchase.store, IapStore::Apple);
        assert_eq!(purchase.current_plan_id.as_deref(), Some("premium_monthly"));
        assert_eq!(
            purchase.extra.get("transactionReasonIOS"),
            Some(&json!("PURCHASE"))
        );

        let round_tripped = serde_json::to_value(&purchase).unwrap();
        assert_eq!(round_tripped, raw);
    }

    /// Hand-derived from `Types.kt`'s `PurchaseError.toJson()`, narrowed to
    /// this crate's kept fields (`code`/`message`/`productId`) — the other
    /// six wire fields are deliberately dropped, not captured (see
    /// [`IapPurchaseError`]'s doc).
    #[test]
    fn iap_purchase_error_deserializes_the_kept_fields() {
        let raw = json!({
            "__typename": "PurchaseError",
            "code": "user-cancelled",
            "message": "User cancelled the purchase flow",
            "productId": "monthly_premium",
            "debugMessage": null,
            "responseCode": null,
            "subResponseCodeAndroid": null,
            "productIds": null,
            "productType": null,
            "isEmptyProductList": null,
        });
        let error: IapPurchaseError = serde_json::from_value(raw).unwrap();
        assert_eq!(error.code, IapErrorCode::UserCancelled);
        assert_eq!(error.message, "User cancelled the purchase flow");
        assert_eq!(error.product_id.as_deref(), Some("monthly_premium"));

        // Only the 3 kept fields survive re-serialization (by design).
        let round_tripped = serde_json::to_value(&error).unwrap();
        assert_eq!(
            round_tripped,
            json!({
                "code": "user-cancelled",
                "message": "User cancelled the purchase flow",
                "productId": "monthly_premium",
            })
        );
    }

    /// A handful of `IapErrorCode` variants, checked against `Types.kt`'s
    /// `ErrorCode.rawValue` table verbatim.
    #[test]
    fn iap_error_code_matches_wire_spelling() {
        let cases: &[(IapErrorCode, &str)] = &[
            (IapErrorCode::Unknown, "unknown"),
            (IapErrorCode::UserCancelled, "user-cancelled"),
            (
                IapErrorCode::PurchaseVerificationFinishFailed,
                "purchase-verification-finish-failed",
            ),
            (
                IapErrorCode::BillingResponseJsonParseError,
                "billing-response-json-parse-error",
            ),
            (IapErrorCode::IapNotAvailable, "iap-not-available"),
            (IapErrorCode::SkuOfferMismatch, "sku-offer-mismatch"),
            (IapErrorCode::DuplicatePurchase, "duplicate-purchase"),
        ];
        for (code, wire) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(wire));
            let round_tripped: IapErrorCode = serde_json::from_value(json!(wire)).unwrap();
            assert_eq!(round_tripped, *code);
        }
    }

    /// An unrecognized code string falls back to `Unknown` rather than
    /// failing deserialization (`#[serde(other)]`, forward-compat stance).
    #[test]
    fn iap_error_code_unrecognized_value_falls_back_to_unknown() {
        let code: IapErrorCode = serde_json::from_value(json!("some-future-code")).unwrap();
        assert_eq!(code, IapErrorCode::Unknown);
    }

    /// `PurchaseState`'s own `#[serde(other)]` fallback.
    #[test]
    fn purchase_state_unrecognized_value_falls_back_to_unknown() {
        let state: PurchaseState = serde_json::from_value(json!("some-future-state")).unwrap();
        assert_eq!(state, PurchaseState::Unknown);
    }

    /// This crate's own designed envelope (`FetchProductsResult`'s doc) —
    /// no `Types.kt` counterpart to compare against.
    #[test]
    fn fetch_products_result_envelope_round_trips() {
        let product = Product {
            id: "sku_1".into(),
            title: "Widget".into(),
            description: "A widget".into(),
            price: Some(1.99),
            display_price: "$1.99".into(),
            currency: "USD".into(),
            kind: ProductType::InApp,
            platform: IapPlatform::Android,
            extra: serde_json::Map::new(),
        };
        let result = FetchProductsResult::Products(vec![product]);
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["type"], json!("products"));
        assert!(value["items"].is_array());

        let round_tripped: FetchProductsResult = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, result);
    }

    /// This crate's own designed envelope (`IapEvent`'s doc).
    #[test]
    fn iap_event_envelope_round_trips() {
        let error = IapPurchaseError {
            code: IapErrorCode::UserCancelled,
            message: "cancelled".into(),
            product_id: None,
        };
        let event = IapEvent::PurchaseError(error);
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], json!("purchaseError"));

        let round_tripped: IapEvent = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, event);
    }

    /// Mirrors `Types.kt`'s `ActiveSubscription.toJson()` shape.
    #[test]
    fn active_subscription_round_trips() {
        let raw = json!({
            "productId": "monthly_premium",
            "isActive": true,
            "expirationDateIOS": null,
            "autoRenewingAndroid": true,
            "environmentIOS": null,
            "daysUntilExpirationIOS": null,
            "transactionId": "GPA.1234-5678-9012-34567",
            "purchaseToken": "opaque-token-value",
            "transactionDate": 1738000000000.0,
            "basePlanIdAndroid": "premium-month",
            "purchaseTokenAndroid": "opaque-token-value",
            "currentPlanId": "premium-month",
            "renewalInfoIOS": null,
        });
        let subscription: ActiveSubscription = serde_json::from_value(raw).unwrap();
        assert_eq!(subscription.product_id, "monthly_premium");
        assert!(subscription.is_active);
        assert_eq!(subscription.auto_renewing_android, Some(true));
    }

    /// `ProductRequest`'s default `type` (`ProductQueryType::InApp`) when
    /// omitted — matches `type.graphql`'s `type: ProductQueryType = InApp`.
    #[test]
    fn product_request_type_defaults_to_in_app() {
        let raw = json!({ "skus": ["sku_1", "sku_2"] });
        let request: ProductRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(request.kind, ProductQueryType::InApp);
        assert_eq!(request.skus, vec!["sku_1", "sku_2"]);
    }

    /// `RequestPurchaseProps`' nested envelope — a `google` (Android) purchase
    /// request, matching `type.graphql`'s `input RequestPurchaseProps`
    /// nesting (`requestPurchase: RequestPurchasePropsByPlatforms`).
    #[test]
    fn request_purchase_props_nested_envelope_round_trips() {
        let props = RequestPurchaseProps {
            request_purchase: Some(RequestPurchasePropsByPlatforms {
                apple: None,
                google: Some(json!({ "skus": ["sku_1"] })),
            }),
            request_subscription: None,
            kind: ProductQueryType::InApp,
        };
        let value = serde_json::to_value(&props).unwrap();
        assert_eq!(value["type"], json!("in-app"));
        assert_eq!(value["requestPurchase"]["google"]["skus"], json!(["sku_1"]));
        assert!(value.get("requestSubscription").is_none());

        let round_tripped: RequestPurchaseProps = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, props);
    }

    /// `PurchaseInput` — what `finish_transaction` sends back, matching
    /// `type.graphql`'s `input PurchaseInput`.
    #[test]
    fn purchase_input_round_trips() {
        let raw = json!({
            "id": "GPA.1234-5678-9012-34567",
            "productId": "monthly_premium",
            "ids": ["GPA.1234-5678-9012-34567"],
            "transactionDate": 1738000000000.0,
            "purchaseToken": "opaque-token-value",
            "store": "google",
            "quantity": 1,
            "purchaseState": "purchased",
            "isAutoRenewing": true,
        });
        let input: PurchaseInput = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(input.store, Some(IapStore::Google));

        let round_tripped = serde_json::to_value(&input).unwrap();
        assert_eq!(round_tripped, raw);
    }

    /// `PurchaseOptions` — every field optional and omitted when unset,
    /// matching `type.graphql`'s `input PurchaseOptions`.
    #[test]
    fn purchase_options_default_omits_every_field() {
        let options = PurchaseOptions::default();
        let value = serde_json::to_value(&options).unwrap();
        assert_eq!(value, json!({}));
    }
}
