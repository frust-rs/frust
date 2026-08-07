//! The Android [`Backend`] — Play Billing through `openiap-google`, driven
//! from a `dev.frust.iap.FrustIapHost` Kotlin host
//! (`plugins/iap/platform/android/`) over this crate's own JNI surface. Every
//! request and response crosses as an OpenIAP JSON string inside
//! `frust_plugin::android`'s scoped attach.
//!
//! # The frozen contract
//!
//! **`FrustIapHost.kt` and this module build to this table — changing it means
//! updating both files together.** `dev.frust.iap` is a subpackage of the
//! embedding module's `dev.frust` (`dev.frust` is `frust-embedding`'s exclusive
//! package — `docs/CODE_STANDARDS.md`'s Plugin Conventions);
//! `FrustIapHost`'s package is baked into every JNI symbol name below, so it
//! may never move once shipped.
//!
//! ## Rust → Kotlin: one static, resolved via
//! `context.getClassLoader().loadClass(...)` (the `FrustCameraHost`/
//! `FrustBiometric` mechanism)
//!
//! | Method | Signature (Java) | Notes |
//! |---|---|---|
//! | `call` | `(long requestId, String method, String argsJson) -> void` | Never blocks. Answers with **exactly one** `nativeOnIapResult` carrying the same `requestId` — including for a failure raised before the host even dispatches |
//!
//! One static rather than one per operation: unlike `frust-camera`'s typed
//! `int`/`float` statics, every operation here has the identical shape (a JSON
//! request in, a JSON document or an OpenIAP error JSON out). A new operation
//! is a new `method` string, never a new static — the additive rule the
//! sibling plugins state for their own tables.
//!
//! | `method` | `argsJson` | `okJson` |
//! |---|---|---|
//! | [`METHOD_INIT_CONNECTION`] | `{"config": obj?}` | `true`/`false` |
//! | [`METHOD_END_CONNECTION`] | `{}` | `true`/`false` |
//! | [`METHOD_FETCH_PRODUCTS`] | an OpenIAP `ProductRequest` | a [`FetchProductsResult`] envelope |
//! | [`METHOD_GET_AVAILABLE_PURCHASES`] | `{"options": obj?}` | `Purchase[]` |
//! | [`METHOD_GET_ACTIVE_SUBSCRIPTIONS`] | `{"subscriptionIds": string[]?}` | `ActiveSubscription[]` |
//! | [`METHOD_GET_STOREFRONT`] | `{}` | a JSON string |
//! | [`METHOD_REQUEST_PURCHASE`] | a `RequestPurchaseProps` | `null` (see *A purchase resolves at dispatch* below) |
//! | [`METHOD_FINISH_TRANSACTION`] | `{"purchase": obj, "isConsumable": bool?}` | `null` |
//! | [`METHOD_RESTORE_PURCHASES`] | `{}` | `null` |
//! | [`METHOD_DEEP_LINK_TO_SUBSCRIPTIONS`] | `{"options": obj?}` | `null` |
//! | [`METHOD_ACKNOWLEDGE_PURCHASE`] | `{"purchaseToken": string}` | `true`/`false` |
//! | [`METHOD_CONSUME_PURCHASE`] | `{"purchaseToken": string}` | `true`/`false` |
//!
//! ## Kotlin → Rust (this crate's own `#[unsafe(no_mangle)]` JNI exports —
//! the package is baked into the symbol names)
//!
//! - [`Java_dev_frust_iap_FrustIapHost_nativeOnIapResult`]`(env, class, requestId: jlong, okJson: JString, errJson: JString)`
//!   — exactly one of `okJson`/`errJson` is non-null (*Correlating an answer*).
//! - [`Java_dev_frust_iap_FrustIapHost_nativeOnIapEvent`]`(env, class, kind: JString, payloadJson: JString)`
//!   — [`EVENT_PURCHASE_UPDATED`] carries an OpenIAP `Purchase`,
//!   [`EVENT_PURCHASE_ERROR`] a `PurchaseError`. Called on whatever thread
//!   Play Billing reported on (the main thread, for a `BillingClient` built
//!   with no custom executor) and queued for the plugin's own delivery
//!   thread rather than fanned out there.
//!
//! Each export upgrades its [`jni::EnvUnowned`] via
//! [`jni::EnvUnowned::with_env`], which wraps the body in `catch_unwind` — the
//! crate's own no-unwind-across-FFI guarantee (`docs/CODE_STANDARDS.md`'s
//! Language Idioms). This module holds **no `unsafe` block at all**; its only
//! `unsafe` tokens are the two exports' `#[unsafe(no_mangle)]` attributes.
//!
//! # Guard order
//!
//! Every method below runs [`Backend`]'s documented guard order before any host
//! work: platform readiness ([`IapError::PlatformNotInitialized`], a plain flag
//! inside `frust_plugin`, checked before any JNI call) → the UI-thread
//! fail-fast ([`IapError::UiThread`], a live `Looper.myLooper() ==
//! Looper.getMainLooper()` test) → the connection state
//! ([`IapError::NotConnected`]).
//!
//! [`Backend::request_purchase`] is the one store-touching call with **no**
//! UI-thread guard: `launchBillingFlow` is itself a main-thread Play API, and
//! the host reaches that thread on its own (`OpenIapModule` re-posts through
//! `Activity.runOnUiThread`), so a UI-thread caller is served rather than
//! refused — the crate doc's *Blocking API* table states the same exemption.
//!
//! # Correlating an answer
//!
//! `call` is fire-and-forget on the Java side, so every answer has to be matched
//! back to the caller parked on it. [`arm_request`] mints a monotonic id, files
//! a one-slot channel under it in [`PENDING`], and only then issues the JNI
//! call; `nativeOnIapResult` **removes** the entry and sends into it. The
//! removal is what makes resolution exactly-once: a second answer for the same
//! id finds nothing to remove and is logged and dropped, so a late answer to a
//! timed-out call can never resolve the next one. The wait itself happens
//! **outside** the scoped JNI attachment ([`with_host`] has already returned),
//! so a blocked caller never pins a JVM attachment away from the thread that
//! has to deliver its answer.
//!
//! # A purchase resolves at dispatch, not at outcome
//!
//! `OpenIapStore.requestPurchase` suspends for the *whole* purchase flow —
//! including however long the user spends in Play's sheet. The public contract
//! is the opposite ([`crate::Iap::request_purchase`] returns once the store has
//! accepted the request), so the host resolves this one method as soon as it has
//! validated the request and started the flow. The outcome then arrives the only
//! way it ever does: as an [`IapEvent`] on the listener registry, which is also
//! where Play's own in-flow rejections (an unknown SKU, an already-owned
//! product, a cancel) are published — **once** per failure: OpenIAP publishes
//! every one of them through its own publish-once gate before the detached
//! flow rethrows, and the host's `catch` around that flow logs rather than
//! emitting a second copy.
//!
//! # Connection state lives here
//!
//! [`CONNECTED`] is this backend's copy of the store connection's open/closed
//! state, so the [`IapError::NotConnected`] guard is a plain atomic read rather
//! than a JNI round trip — and so [`Backend::end_connection`] can answer
//! "nothing was open" without touching the host at all. It is set by a
//! successful [`Backend::init_connection`] and cleared by
//! [`Backend::end_connection`].
//!
//! # Fail-soft, never a panic
//!
//! Any JNI failure — a missing `FrustIapHost` class (the plugin's Gradle module
//! isn't wired into the app), a thrown exception, an answer that never arrives —
//! maps to a typed [`IapError`] naming the failing operation, never a panic
//! across the FFI boundary.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::jlong;
use jni::{Env, EnvUnowned, jni_sig, jni_str};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::types::{
    ActiveSubscription, FetchProductsResult, IapErrorCode, IapEvent, IapPurchaseError,
    ProductRequest, Purchase, PurchaseInput, PurchaseOptions, RequestPurchaseProps,
};
use crate::{Backend, IapError};

/// The Kotlin host's fully-qualified class name in **binary/dotted** form, as
/// `ClassLoader.loadClass` expects (**not** the slash form `FindClass` wants) —
/// the module doc's frozen contract, and the package baked into both JNI export
/// symbols below. Looked up through the application classloader (see
/// [`load_host_class`]).
const HOST_CLASS_BINARY: &str = "dev.frust.iap.FrustIapHost";

/// `call`'s `method`: open the store connection (module doc's method table).
const METHOD_INIT_CONNECTION: &str = "initConnection";
/// `call`'s `method`: close the store connection.
const METHOD_END_CONNECTION: &str = "endConnection";
/// `call`'s `method`: the product query.
const METHOD_FETCH_PRODUCTS: &str = "fetchProducts";
/// `call`'s `method`: the owned-purchase query.
const METHOD_GET_AVAILABLE_PURCHASES: &str = "getAvailablePurchases";
/// `call`'s `method`: the subscription query.
const METHOD_GET_ACTIVE_SUBSCRIPTIONS: &str = "getActiveSubscriptions";
/// `call`'s `method`: the account's billing country.
const METHOD_GET_STOREFRONT: &str = "getStorefront";
/// `call`'s `method`: start a purchase flow.
const METHOD_REQUEST_PURCHASE: &str = "requestPurchase";
/// `call`'s `method`: acknowledge or consume a purchase.
const METHOD_FINISH_TRANSACTION: &str = "finishTransaction";
/// `call`'s `method`: re-query Play for the account's purchases.
const METHOD_RESTORE_PURCHASES: &str = "restorePurchases";
/// `call`'s `method`: open Play's subscription-management page.
const METHOD_DEEP_LINK_TO_SUBSCRIPTIONS: &str = "deepLinkToSubscriptions";
/// `call`'s `method`: acknowledge a purchase token.
const METHOD_ACKNOWLEDGE_PURCHASE: &str = "acknowledgePurchase";
/// `call`'s `method`: consume a purchase token.
const METHOD_CONSUME_PURCHASE: &str = "consumePurchase";

/// `nativeOnIapEvent`'s `kind` for an OpenIAP `Purchase` payload — maps onto
/// [`IapEvent::PurchaseUpdated`] (module doc's export list).
const EVENT_PURCHASE_UPDATED: &str = "purchase-updated";
/// `nativeOnIapEvent`'s `kind` for an OpenIAP `PurchaseError` payload — maps
/// onto [`IapEvent::PurchaseError`].
const EVENT_PURCHASE_ERROR: &str = "purchase-error";

/// [`IapEvent`]'s own adjacently-tagged envelope tag for a purchase update
/// (`types.rs`'s designed wire shape, `#[serde(tag = "event", content =
/// "data")]` with camelCase variant names).
const EVENT_TAG_PURCHASE_UPDATED: &str = "purchaseUpdated";
/// [`IapEvent`]'s envelope tag for a purchase failure.
const EVENT_TAG_PURCHASE_ERROR: &str = "purchaseError";

/// How long a genuinely store-touching, UI-thread-refused call parks on its
/// `nativeOnIapResult` answer before reporting [`IapError::Platform`].
///
/// Every method except [`Backend::request_purchase`] (which uses
/// [`ACK_TIMEOUT`] instead — its dispatch-ack never reaches the store) waits
/// this long. **Not a platform value** — neither Play Billing nor OpenIAP
/// publishes a bound for a query's completion (OpenIAP's own longest internal
/// wait, on a concurrent connection attempt, is 15 s). A minute comfortably
/// covers a slow product query on a bad network while still failing a caller
/// whose answer never arrives at all — which, since every call here is paired
/// with `frust_reactive::spawn_blocking`, would otherwise pin a blocking-pool
/// thread for the life of the process.
const HOST_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the dispatch-only ack for [`Backend::request_purchase`] waits
/// before reporting [`IapError::Platform`].
///
/// Nothing on this path touches the store: `FrustIapHost.call`'s
/// `requestPurchase` branch validates the request, resolves the current
/// Activity, detaches the actual `OpenIapStore.requestPurchase` flow into its
/// own coroutine, and answers immediately — so the wait covers only a JNI hop
/// plus a coroutine launch, nothing store-touching. Mirrors `apple.rs`'s
/// `ACK_TIMEOUT` and its reasoning: a wait this long already means the host
/// never ran.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// The cached `dev.frust.iap.FrustIapHost` class reference.
///
/// **One** global reference for the whole process (plus the method id ART
/// caches behind `call_static_method`), never a per-call one: ART's global-ref
/// table is a hard-capped budget, and a local `JClass` cannot be cached instead
/// — locals die with their JNI frame.
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Whether [`Backend::init_connection`] has opened the store connection and
/// [`Backend::end_connection`] has not closed it (module doc's *Connection
/// state lives here*).
static CONNECTED: AtomicBool = AtomicBool::new(false);

/// One host answer: the `okJson` payload document, or the OpenIAP error JSON.
type HostAnswer = Result<String, String>;

/// The id the next [`arm_request`] mints. Starts at 1 and only ever increases,
/// so an id is never reused and a late answer cannot land on a later call.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Every in-flight call's one-slot answer channel, keyed by request id — the
/// map `nativeOnIapResult` resolves against (module doc's *Correlating an
/// answer*).
///
/// `HashMap::new` is not `const`, so this is a [`LazyLock`] rather than a bare
/// `static Mutex<HashMap<..>>`.
static PENDING: LazyLock<Mutex<HashMap<u64, SyncSender<HostAnswer>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lock a module-global mutex, recovering from poisoning instead of panicking:
/// a panic caught by an export's `catch_unwind` must not turn every later
/// billing call into a panic near the FFI boundary
/// (`docs/CODE_STANDARDS.md`'s no-unwind rule). The guarded data is plain
/// bookkeeping, so a poisoned view is still coherent enough to use.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- JNI plumbing -----------------------------------------------------------

/// Run `f` with a live [`Env`] and the cached `FrustIapHost` class inside a
/// scoped JNI attachment, flattening the two error layers: a missing platform
/// handle → [`IapError::PlatformNotInitialized`] (the ready flag is checked
/// *first*, before any JNI work), a JVM attach failure →
/// [`IapError::Platform`].
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, IapError>,
) -> Result<T, IapError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(IapError::PlatformNotInitialized)
        }
        Err(other) => Err(IapError::Platform(format!(
            "android iap backend: platform handle error: {other}"
        ))),
    }
}

/// The cached [`HOST_CLASS`], loading it on first use. A racing loser's
/// reference is dropped immediately (`Global`'s own `Drop` releases it), so at
/// most one global ref survives.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, IapError> {
    if let Some(class) = HOST_CLASS.get() {
        return Ok(class);
    }
    let class = load_host_class(env, context)?;
    Ok(HOST_CLASS.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass("dev.frust.iap.FrustIapHost")`, promoted
/// to a process-lifetime global reference.
///
/// The application `Context`'s classloader is the only loader that can see
/// app-defined classes — a bare `FindClass` on a JNI worker thread sees the
/// bootstrap loader only, which is exactly why this explicit path exists. A
/// `ClassNotFoundException` here means the plugin's Android Gradle module isn't
/// wired into the app.
fn load_host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, IapError> {
    run_jni(
        env,
        "loading dev.frust.iap.FrustIapHost (is the plugin's Android Gradle module wired into the \
         app?)",
        |env| {
            let loader = env
                .call_method(
                    context,
                    jni_str!("getClassLoader"),
                    jni_sig!("()Ljava/lang/ClassLoader;"),
                    &[],
                )?
                .l()?;
            let name = env.new_string(HOST_CLASS_BINARY)?;
            let class = env
                .call_method(
                    &loader,
                    jni_str!("loadClass"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                    &[JValue::Object(&name)],
                )?
                .l()?;
            let class = env.cast_local::<JClass>(class)?;
            env.new_global_ref(class)
        },
    )
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// typed [`IapError::Platform`] naming `op`.
///
/// `jni` 0.22 returns `Err(Error::JavaException)` and leaves the exception
/// **pending** — undefined behaviour for the next JNI call — so we always
/// check/clear it here before returning, whatever `f` reported (the
/// `frust-camera`/`frust-secure-storage` `run_jni` shape). A *store* failure
/// never arrives this way: the host catches those and answers with OpenIAP
/// error JSON, which [`map_host_error`] turns into [`IapError::Store`].
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, IapError> {
    let result = f(env);
    if env.exception_check() {
        return Err(take_pending_exception(env, op));
    }
    result.map_err(|err| IapError::Platform(format!("android iap backend: {op}: {err}")))
}

/// Extract, **clear**, and describe the pending Java exception. Clears first
/// (mirroring `jni`'s own `exception_catch`) so the subsequent class/message
/// queries run without a pending exception; a defensive final clear covers the
/// unlikely case one of those queries itself throws.
fn take_pending_exception(env: &mut Env<'_>, op: &str) -> IapError {
    let Some(throwable) = env.exception_occurred() else {
        env.exception_clear();
        return IapError::Platform(format!(
            "android iap backend: {op}: JNI reported an exception with no throwable"
        ));
    };
    env.exception_clear();

    let class_name = match env.get_object_class(&throwable) {
        Ok(class) => match class.get_name(env) {
            Ok(name) => name.to_string(),
            Err(_) => "<unknown exception class>".to_string(),
        },
        Err(_) => "<unknown exception class>".to_string(),
    };
    let message = match throwable.get_message(env) {
        Ok(msg) => msg.to_string(),
        Err(_) => "<no message>".to_string(),
    };

    // Defensive: don't leave a second exception pending for the next JNI call.
    if env.exception_check() {
        env.exception_clear();
    }

    IapError::Platform(format!(
        "android iap backend: {op}: {class_name}: {message}"
    ))
}

/// Refuse a blocking call made on the UI thread, before it does anything at all
/// (module doc's *Guard order*).
///
/// The test is Android's own: `Looper.myLooper() == Looper.getMainLooper()`. A
/// plain Rust worker thread has no `Looper` at all, so `myLooper()` returns null
/// there and `IsSameObject(null, mainLooper)` is false — no separate null check
/// is needed (it *would* be needed if both sides could be null, since
/// `IsSameObject` calls two nulls equal, and `getMainLooper()` never is).
///
/// `android.os.Looper` is a framework class, so the bare `find_class` works on
/// any thread — unlike an app class, which needs the application classloader
/// ([`load_host_class`]).
fn ensure_off_ui_thread(env: &mut Env<'_>) -> Result<(), IapError> {
    let on_ui_thread = run_jni(env, "android.os.Looper.myLooper", |env| {
        let looper_class = env.find_class(jni_str!("android/os/Looper"))?;
        let mine = env
            .call_static_method(
                &looper_class,
                jni_str!("myLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        let main = env
            .call_static_method(
                &looper_class,
                jni_str!("getMainLooper"),
                jni_sig!("()Landroid/os/Looper;"),
                &[],
            )?
            .l()?;
        env.is_same_object(&mine, &main)
    })?;

    if on_ui_thread {
        return Err(IapError::UiThread);
    }
    Ok(())
}

// --- Guards -----------------------------------------------------------------

/// Which of [`Backend`]'s three guards an operation runs — step 1 (platform
/// readiness) is unconditional, so only the other two vary.
#[derive(Clone, Copy)]
struct Guards {
    /// Run step 2, the [`IapError::UiThread`] fail-fast. False only for
    /// [`Backend::request_purchase`] (module doc's *Guard order*).
    ui_thread: bool,
    /// Run step 3, the [`IapError::NotConnected`] check. False for
    /// [`Backend::init_connection`]/[`Backend::end_connection`], which are the
    /// calls that move that state.
    connection: bool,
}

impl Guards {
    /// The guards every store-touching call runs.
    const FULL: Self = Self {
        ui_thread: true,
        connection: true,
    };
    /// The connection lifecycle's own two calls: no connection check.
    const LIFECYCLE: Self = Self {
        ui_thread: true,
        connection: false,
    };
    /// [`Backend::request_purchase`]: connection-checked, never UI-thread
    /// refused.
    const PURCHASE: Self = Self {
        ui_thread: false,
        connection: true,
    };

    /// Run the guards in [`Backend`]'s documented order: readiness → UI thread
    /// → connection state.
    fn check(self) -> Result<(), IapError> {
        // Steps 1 and 2. `with_host` reports `PlatformNotInitialized` before it
        // performs any JNI work at all, so readiness genuinely precedes the
        // `Looper` test even though both live in this one call.
        if self.ui_thread {
            with_host(|env, _class| ensure_off_ui_thread(env))?;
        } else {
            with_host(|_env, _class| Ok(()))?;
        }
        // Step 3.
        if self.connection && !CONNECTED.load(Ordering::Acquire) {
            return Err(IapError::NotConnected);
        }
        Ok(())
    }
}

// --- Request/answer correlation ---------------------------------------------

/// Mint a request id and file its one-slot answer channel, returning both.
///
/// Called **before** the JNI call it belongs to, so an answer delivered while
/// `call` is still returning cannot be missed.
fn arm_request() -> (u64, Receiver<HostAnswer>) {
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = sync_channel(1);
    lock(&PENDING).insert(request_id, sender);
    (request_id, receiver)
}

/// Drop `request_id`'s channel — the caller has stopped waiting (the JNI call
/// failed, or the wait timed out). A later answer for it is then indistinguishable
/// from an answer to an unknown id, and is dropped the same way.
fn cancel_request(request_id: u64) {
    lock(&PENDING).remove(&request_id);
}

/// Deliver `answer` to whoever is parked on `request_id`; `true` when it landed
/// on a live request.
///
/// **This is the exactly-once point** (module doc's *Correlating an answer*):
/// the entry is *removed* under the lock, so only the first answer for an id can
/// ever find a channel to send into. A duplicate — or an answer to a request
/// that already timed out — is logged and dropped rather than resolving
/// something else.
fn deliver_answer(request_id: u64, answer: HostAnswer) -> bool {
    let Some(sender) = lock(&PENDING).remove(&request_id) else {
        log::debug!(
            "frust-iap: nativeOnIapResult for request {request_id}, which is no longer awaited — \
             dropped"
        );
        return false;
    };
    if sender.try_send(answer).is_err() {
        // The caller was cancelled between the removal above and this send.
        log::debug!(
            "frust-iap: request {request_id} was answered after its caller stopped waiting"
        );
    }
    true
}

/// Issue `method` against the host and park on its answer up to `timeout`,
/// returning the raw `okJson` document.
///
/// The wait happens **outside** the JNI attachment — [`with_host`] has already
/// dropped it — so the host's own thread is free to attach and deliver. The
/// caller picks the bound: [`HOST_CALL_TIMEOUT`] for a genuinely store-touching
/// call, [`ACK_TIMEOUT`] for [`Backend::request_purchase`]'s dispatch-only ack.
fn call_host_json(method: &str, args: &Value, timeout: Duration) -> Result<String, IapError> {
    let args_json = serde_json::to_string(args).map_err(|err| {
        IapError::Serialization(format!(
            "android iap backend: could not encode the arguments for {method}: {err}"
        ))
    })?;

    let (request_id, answer) = arm_request();
    let issued = with_host(|env, class| {
        run_jni(env, "FrustIapHost.call", |env| {
            let method_jstr = env.new_string(method)?;
            let args_jstr = env.new_string(&args_json)?;
            env.call_static_method(
                class,
                jni_str!("call"),
                jni_sig!("(JLjava/lang/String;Ljava/lang/String;)V"),
                &[
                    JValue::Long(request_id as jlong),
                    JValue::Object(&method_jstr),
                    JValue::Object(&args_jstr),
                ],
            )?
            .v()
        })
    });
    if let Err(err) = issued {
        cancel_request(request_id);
        return Err(err);
    }

    await_answer(&answer, timeout, request_id, method)
}

/// Park on `answer` up to `timeout`, cancelling `request_id`'s slot on a
/// timeout so a later answer can never resolve a different call.
///
/// Split out of [`call_host_json`] so the wait/timeout logic is testable
/// against a fake boundary (this module's tests) rather than only through a
/// real JNI call.
fn await_answer(
    answer: &Receiver<HostAnswer>,
    timeout: Duration,
    request_id: u64,
    method: &str,
) -> Result<String, IapError> {
    match answer.recv_timeout(timeout) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(error_json)) => Err(map_host_error(&error_json)),
        Err(_) => {
            cancel_request(request_id);
            Err(IapError::Platform(format!(
                "android iap backend: {method} did not answer within {}s",
                timeout.as_secs()
            )))
        }
    }
}

/// [`call_host_json`], with the answer parsed into the wire type the method
/// promises (module doc's method table).
fn call_host<T: DeserializeOwned>(
    method: &str,
    args: &Value,
    timeout: Duration,
) -> Result<T, IapError> {
    let payload = call_host_json(method, args, timeout)?;
    serde_json::from_str(&payload).map_err(|err| {
        IapError::Serialization(format!(
            "android iap backend: {method} answered with a payload this crate could not parse: \
             {err}"
        ))
    })
}

/// [`call_host_json`] for a method whose result is `null` — the payload is
/// parsed only to prove it is well-formed JSON, then discarded.
fn call_host_unit(method: &str, args: &Value, timeout: Duration) -> Result<(), IapError> {
    call_host::<Value>(method, args, timeout).map(|_| ())
}

/// Map the host's OpenIAP error JSON onto this crate's error type.
///
/// A payload carrying the spec's `code`/`message` becomes the typed
/// [`IapError::Store`] (a genuine store refusal the app can match on); anything
/// else — a truncated payload, a shape from a future spec — degrades to
/// [`IapError::Platform`] with the raw JSON preserved rather than being reported
/// as a store error it may not be.
fn map_host_error(error_json: &str) -> IapError {
    match serde_json::from_str::<IapPurchaseError>(error_json) {
        Ok(error) => IapError::Store(error),
        Err(err) => IapError::Platform(format!(
            "android iap backend: the host reported a failure this crate could not parse ({err}): \
             {error_json}"
        )),
    }
}

// --- Event delivery ---------------------------------------------------------

/// Rebuild one purchase event from the wire and hand it to the listener
/// registry.
///
/// Runs on whatever thread Play Billing called the host's listener on — which
/// is the main thread, since `openiap-google` builds its `BillingClient`
/// without a custom executor — so it only *queues*: [`crate::event::emit`]
/// hands the event to the plugin-owned delivery thread and returns, which is
/// what makes [`crate::Iap::set_purchase_listener`]'s contract (never the UI
/// thread, never the thread that called [`crate::Iap::request_purchase`])
/// true on this platform.
fn deliver_event(kind: &str, payload_json: &str) {
    match event_from_wire(kind, payload_json) {
        Some(event) => crate::event::emit(&event),
        None => log::warn!(
            "frust-iap: a `{kind}` event could not be decoded and was dropped — no listener saw it"
        ),
    }
}

/// `(kind, payload)` as an [`IapEvent`], or `None` for an event this crate
/// cannot decode (a `kind` it doesn't know, a payload that isn't the shape that
/// `kind` promises).
///
/// The pair is assembled into [`IapEvent`]'s own adjacently-tagged envelope
/// (`types.rs`'s designed `{"event": ..., "data": ...}` shape) and deserialized
/// through it, rather than the variants being constructed by hand — so the
/// envelope stays the single definition of what an event *is*, and adding a
/// third variant is a `kind` mapping and nothing else.
fn event_from_wire(kind: &str, payload_json: &str) -> Option<IapEvent> {
    let tag = match kind {
        EVENT_PURCHASE_UPDATED => EVENT_TAG_PURCHASE_UPDATED,
        EVENT_PURCHASE_ERROR => EVENT_TAG_PURCHASE_ERROR,
        unknown => {
            log::warn!("frust-iap: nativeOnIapEvent reported an unknown kind `{unknown}`");
            return None;
        }
    };
    let data: Value = match serde_json::from_str(payload_json) {
        Ok(data) => data,
        Err(err) => {
            log::warn!("frust-iap: a `{kind}` event payload was not valid JSON: {err}");
            return None;
        }
    };
    match serde_json::from_value(json!({ "event": tag, "data": data })) {
        Ok(event) => Some(event),
        Err(err) => {
            log::warn!("frust-iap: a `{kind}` event payload did not match its wire shape: {err}");
            None
        }
    }
}

// --- The backend ------------------------------------------------------------

/// The Android backend.
///
/// Stateless itself — the connection state lives in [`CONNECTED`] and the host
/// objects live on the Kotlin side — so it holds no JNI reference and is
/// trivially `Send + Sync`, the per-call-attachment shape every sibling plugin
/// uses.
pub(crate) struct AndroidIap;

impl Backend for AndroidIap {
    /// Opens the connection and records it in [`CONNECTED`].
    ///
    /// Idempotent by contract ([`crate::Iap::init_connection`]): the host already
    /// answers `true` for an already-open client, and a store that instead
    /// reports the spec's [`IapErrorCode::AlreadyPrepared`] refusal is mapped
    /// onto the same success here — belt and braces across both halves of the
    /// bridge, since the guarantee is the app-visible contract rather than
    /// either side's implementation detail.
    fn init_connection(&self, config: Option<&Value>) -> Result<bool, IapError> {
        Guards::LIFECYCLE.check()?;
        let result = call_host::<bool>(
            METHOD_INIT_CONNECTION,
            &json!({ "config": config }),
            HOST_CALL_TIMEOUT,
        );
        match result {
            Ok(true) => {
                CONNECTED.store(true, Ordering::Release);
                Ok(true)
            }
            // The host reached the store and it declined to connect (no Play
            // Store on the device, Play Services unavailable). Reported as the
            // `false` the contract has for exactly this, not as an error.
            Ok(false) => Ok(false),
            Err(IapError::Store(error)) if error.code == IapErrorCode::AlreadyPrepared => {
                CONNECTED.store(true, Ordering::Release);
                Ok(true)
            }
            Err(err) => Err(err),
        }
    }

    /// `Ok(true)` when this call closed an open connection, `Ok(false)` when
    /// none was open — and **never** an error out of the close itself
    /// ([`crate::Iap::end_connection`]'s frozen semantics), so a host failure
    /// here is logged and the connection is still considered closed. The guards
    /// above it can still refuse, exactly as the fake store's own
    /// `end_connection` does.
    fn end_connection(&self) -> Result<bool, IapError> {
        Guards::LIFECYCLE.check()?;
        // This flag, not the host's own boolean, is what answers "did this call
        // close something": it is the state the `NotConnected` guard reads, so
        // the two can never disagree about whether a connection was open.
        if !CONNECTED.swap(false, Ordering::AcqRel) {
            // Nothing was open, so there is nothing to close and no reason to
            // cross JNI for it.
            return Ok(false);
        }
        if let Err(err) = call_host_unit(METHOD_END_CONNECTION, &json!({}), HOST_CALL_TIMEOUT) {
            log::warn!("frust-iap: end_connection: the host reported {err} — treated as closed");
        }
        Ok(true)
    }

    fn fetch_products(&self, request: &ProductRequest) -> Result<FetchProductsResult, IapError> {
        Guards::FULL.check()?;
        call_host(METHOD_FETCH_PRODUCTS, &encode(request)?, HOST_CALL_TIMEOUT)
    }

    fn get_available_purchases(
        &self,
        options: Option<&PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError> {
        Guards::FULL.check()?;
        call_host(
            METHOD_GET_AVAILABLE_PURCHASES,
            &json!({ "options": options }),
            HOST_CALL_TIMEOUT,
        )
    }

    fn get_active_subscriptions(
        &self,
        ids: Option<&[String]>,
    ) -> Result<Vec<ActiveSubscription>, IapError> {
        Guards::FULL.check()?;
        call_host(
            METHOD_GET_ACTIVE_SUBSCRIPTIONS,
            &json!({ "subscriptionIds": ids }),
            HOST_CALL_TIMEOUT,
        )
    }

    fn get_storefront(&self) -> Result<String, IapError> {
        Guards::FULL.check()?;
        call_host(METHOD_GET_STOREFRONT, &json!({}), HOST_CALL_TIMEOUT)
    }

    /// Hands the request to Play and returns as soon as the host has accepted
    /// it — the outcome arrives as an [`IapEvent`] (module doc's *A purchase
    /// resolves at dispatch*). Deliberately **not** UI-thread guarded.
    ///
    /// It still uses the same request/answer correlation as every other call,
    /// so it does wait — but only for the host's *dispatch* acknowledgement (a
    /// JNI round trip plus a coroutine hop), bounded by [`ACK_TIMEOUT`] rather
    /// than [`HOST_CALL_TIMEOUT`], never for Play. That answer is delivered off
    /// a background dispatcher rather than the main `Looper`, so it lands even
    /// for a caller sitting on the UI thread, which is what makes the exemption
    /// safe rather than merely permitted. The wait is what buys a synchronous
    /// error for a malformed request or a missing Activity, instead of
    /// silence.
    fn request_purchase(&self, props: &RequestPurchaseProps) -> Result<(), IapError> {
        Guards::PURCHASE.check()?;
        call_host_unit(METHOD_REQUEST_PURCHASE, &encode(props)?, ACK_TIMEOUT)
    }

    fn finish_transaction(
        &self,
        purchase: &PurchaseInput,
        is_consumable: Option<bool>,
    ) -> Result<(), IapError> {
        Guards::FULL.check()?;
        call_host_unit(
            METHOD_FINISH_TRANSACTION,
            &json!({ "purchase": encode(purchase)?, "isConsumable": is_consumable }),
            HOST_CALL_TIMEOUT,
        )
    }

    fn restore_purchases(&self) -> Result<(), IapError> {
        Guards::FULL.check()?;
        call_host_unit(METHOD_RESTORE_PURCHASES, &json!({}), HOST_CALL_TIMEOUT)
    }

    fn deep_link_to_subscriptions(&self, options: Option<&Value>) -> Result<(), IapError> {
        Guards::FULL.check()?;
        call_host_unit(
            METHOD_DEEP_LINK_TO_SUBSCRIPTIONS,
            &json!({ "options": options }),
            HOST_CALL_TIMEOUT,
        )
    }

    /// Android **is** the platform that has this operation, so the trait
    /// default is overridden here (`docs`-wise the mirror of the Apple backend's
    /// `get_pending_transactions`).
    ///
    /// Play answers a non-OK response code with `false` rather than an
    /// exception, so a `false` is turned into a typed failure here — silently
    /// reporting success for an unacknowledged purchase would let Play's 3-day
    /// auto-refund revoke an entitlement the app already granted.
    fn acknowledge_purchase(&self, purchase_token: &str) -> Result<(), IapError> {
        Guards::FULL.check()?;
        settle_token(METHOD_ACKNOWLEDGE_PURCHASE, purchase_token)
    }

    /// Android-only, same reasoning as [`Self::acknowledge_purchase`].
    fn consume_purchase(&self, purchase_token: &str) -> Result<(), IapError> {
        Guards::FULL.check()?;
        settle_token(METHOD_CONSUME_PURCHASE, purchase_token)
    }

    // `get_pending_transactions` is deliberately NOT overridden: it is
    // iOS-only, so the trait's `NotSupportedOnPlatform` default is the correct
    // and permanent answer here. `has_active_subscriptions` is likewise left on
    // its default, which derives the answer from `get_active_subscriptions` —
    // the host's own `hasActiveSubscriptions` would cost the identical single
    // round trip, so overriding it would buy nothing and could only drift from
    // the equivalence the trait promises.
}

/// The shared body of [`Backend::acknowledge_purchase`]/
/// [`Backend::consume_purchase`]: both hand Play a token and both answer with a
/// boolean (see [`Backend::acknowledge_purchase`]'s doc). Both callers run
/// [`Guards::FULL`], so both are genuinely store-touching — [`HOST_CALL_TIMEOUT`]
/// applies.
fn settle_token(method: &str, purchase_token: &str) -> Result<(), IapError> {
    let settled: bool = call_host(
        method,
        &json!({ "purchaseToken": purchase_token }),
        HOST_CALL_TIMEOUT,
    )?;
    if settled {
        return Ok(());
    }
    Err(IapError::Platform(format!(
        "android iap backend: Play refused {method} for this purchase token (already settled, or \
         a token from another account/app?)"
    )))
}

/// Serialize a request type into the JSON the host's own `fromJson` reads.
fn encode<T: serde::Serialize>(value: &T) -> Result<Value, IapError> {
    serde_json::to_value(value).map_err(|err| {
        IapError::Serialization(format!(
            "android iap backend: could not encode a request: {err}"
        ))
    })
}

// --- Kotlin -> Rust JNI exports (contract table above) ---------------------

/// `Java_dev_frust_iap_FrustIapHost_nativeOnIapResult` — the answer to the
/// `call` issued with `request_id`, waking the caller parked in
/// [`call_host_json`].
///
/// Exactly one of `ok_json`/`err_json` is non-null. An answer for an id that is
/// no longer awaited (a duplicate, or a late answer to a timed-out call) is
/// dropped by [`deliver_answer`], never applied to another request — the
/// module doc's *Correlating an answer* invariant.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_iap_FrustIapHost_nativeOnIapResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    request_id: jlong,
    ok_json: JString<'local>,
    err_json: JString<'local>,
) {
    env.with_env(|env| {
        let answer = if !err_json.is_null() {
            Err(err_json.try_to_string(env)?)
        } else if !ok_json.is_null() {
            Ok(ok_json.try_to_string(env)?)
        } else {
            Err(
                "the host delivered neither a result nor an error payload — a FrustIapHost bug"
                    .to_string(),
            )
        };
        // The host mints no ids of its own: `request_id` is always one this
        // module minted as a `u64` and handed over as a `jlong`, so the cast
        // back round-trips exactly.
        deliver_answer(request_id as u64, answer);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_iap_FrustIapHost_nativeOnIapEvent` — one purchase update or
/// purchase failure, forwarded by the host on whatever thread Play Billing
/// called it on.
///
/// Runs **no** app listener on that thread: it decodes the payload and queues
/// it for the plugin-owned delivery thread ([`crate::event::emit`]), so the
/// JVM frame below is released immediately and a listener never holds a
/// platform callback thread. A panic inside a listener is caught by the
/// registry, on that other thread; a panic anywhere here is caught by
/// [`jni::EnvUnowned::with_env`] — neither unwinds into the JVM frame below.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_iap_FrustIapHost_nativeOnIapEvent<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    kind: JString<'local>,
    payload_json: JString<'local>,
) {
    env.with_env(|env| {
        if kind.is_null() || payload_json.is_null() {
            log::warn!("frust-iap: nativeOnIapEvent was called with a null argument — dropped");
            return Ok(());
        }
        let kind = kind.try_to_string(env)?;
        let payload_json = payload_json.try_to_string(env)?;
        deliver_event(&kind, &payload_json);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    /// A minimal OpenIAP `PurchaseAndroid` payload — the shape
    /// `nativeOnIapEvent` carries for [`EVENT_PURCHASE_UPDATED`].
    fn purchase_payload(product_id: &str) -> String {
        json!({
            "__typename": "PurchaseAndroid",
            "id": "GPA.0000-1111-2222-33333",
            "productId": product_id,
            "transactionDate": 1_738_000_000_000.0_f64,
            "purchaseToken": "opaque-token-value",
            "store": "google",
            "quantity": 1,
            "purchaseState": "purchased",
            "isAutoRenewing": false,
        })
        .to_string()
    }

    /// An OpenIAP error payload, as `OpenIapError.toJSON()` produces it.
    fn error_payload(code: &str) -> String {
        json!({
            "code": code,
            "message": "the store said no",
            "platform": "android",
            "debugMessage": null,
            "subResponseCodeAndroid": null,
            "productId": "monthly_premium",
        })
        .to_string()
    }

    /// The happy path: an answer delivered before the deadline reaches the
    /// caller parked on that id.
    #[test]
    fn an_answer_reaches_the_request_that_is_waiting_for_it() {
        let (request_id, answer) = arm_request();

        assert!(deliver_answer(request_id, Ok("true".to_string())));

        assert_eq!(
            answer.recv_timeout(Duration::from_secs(1)),
            Ok(Ok("true".to_string()))
        );
    }

    /// The exactly-once invariant: the second answer for an id finds nothing to
    /// resolve and is dropped, rather than queueing behind the first (or
    /// resolving a later request that reused the slot).
    #[test]
    fn a_second_answer_for_the_same_request_is_ignored() {
        let (request_id, answer) = arm_request();

        assert!(deliver_answer(request_id, Ok("first".to_string())));
        assert!(
            !deliver_answer(request_id, Ok("second".to_string())),
            "the id was already resolved, so the duplicate had nowhere to land"
        );

        assert_eq!(answer.try_recv(), Ok(Ok("first".to_string())));
        assert_eq!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    /// An answer for an id nobody armed (a host bug, or an id from a previous
    /// process) is dropped, not applied to whatever is in flight.
    #[test]
    fn an_answer_for_an_unknown_request_is_dropped() {
        let (armed_id, answer) = arm_request();
        let unknown_id = armed_id.wrapping_add(10_000);

        assert!(!deliver_answer(unknown_id, Ok("stray".to_string())));

        assert_eq!(answer.try_recv(), Err(mpsc::TryRecvError::Empty));
        cancel_request(armed_id);
    }

    /// A timed-out call cancels its slot, so the late answer it was waiting for
    /// can never resolve the *next* call — the failure mode the removal-based
    /// design exists to prevent.
    #[test]
    fn a_cancelled_request_no_longer_accepts_an_answer() {
        let (request_id, answer) = arm_request();
        cancel_request(request_id);
        drop(answer);

        assert!(!deliver_answer(request_id, Ok("late".to_string())));
    }

    /// Ids are never reused, so two concurrent calls cannot collide.
    #[test]
    fn every_request_gets_its_own_id() {
        let (first, first_answer) = arm_request();
        let (second, second_answer) = arm_request();

        assert_ne!(first, second);
        assert!(deliver_answer(second, Ok("second".to_string())));

        assert_eq!(first_answer.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(second_answer.try_recv(), Ok(Ok("second".to_string())));
        cancel_request(first);
    }

    /// [`await_answer`] honors whatever `timeout` its caller passes rather
    /// than a single crate-wide bound: an answer delivered after a short
    /// per-call timeout times that call out, while the identically-timed
    /// delivery to a *separately* armed request still lands under a longer
    /// timeout — a fake boundary (a thread standing in for the JNI answer),
    /// no real JNI, pinning that `request_purchase`'s [`ACK_TIMEOUT`] and
    /// every other call's [`HOST_CALL_TIMEOUT`] are genuinely per-call rather
    /// than read from a shared constant.
    #[test]
    fn a_per_call_timeout_is_honored_independently_of_a_longer_default() {
        let delivery_delay = Duration::from_millis(80);

        // A short timeout: the answer arrives after the deadline, so this
        // call times out even though an answer was in fact on its way.
        let (short_id, short_answer) = arm_request();
        let short_result = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(delivery_delay);
                deliver_answer(short_id, Ok("late for the short wait".to_string()));
            });
            await_answer(
                &short_answer,
                Duration::from_millis(5),
                short_id,
                "requestPurchase",
            )
        });
        match short_result {
            Err(IapError::Platform(message)) => {
                assert!(message.contains("requestPurchase"), "{message}");
            }
            other => panic!("expected a platform timeout error, got {other:?}"),
        }

        // The identical delivery timing against a longer, separately armed
        // timeout still succeeds — the short call above did not shrink
        // anyone else's bound.
        let (long_id, long_answer) = arm_request();
        let long_result = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(delivery_delay);
                deliver_answer(long_id, Ok("on time for the long wait".to_string()));
            });
            await_answer(
                &long_answer,
                Duration::from_secs(5),
                long_id,
                "requestPurchase",
            )
        });
        assert_eq!(long_result.unwrap(), "on time for the long wait");
    }

    /// The event boundary, exercised exactly as the JNI export exercises it:
    /// the two `kind` strings decode through [`IapEvent`]'s own envelope.
    #[test]
    fn both_event_kinds_decode_through_the_envelope() {
        let updated = event_from_wire(EVENT_PURCHASE_UPDATED, &purchase_payload("monthly_premium"));
        match updated {
            Some(IapEvent::PurchaseUpdated(purchase)) => {
                assert_eq!(purchase.product_id, "monthly_premium");
            }
            other => panic!("expected a purchase update, got {other:?}"),
        }

        let failed = event_from_wire(EVENT_PURCHASE_ERROR, &error_payload("user-cancelled"));
        match failed {
            Some(IapEvent::PurchaseError(error)) => {
                assert_eq!(error.code, IapErrorCode::UserCancelled);
                assert_eq!(error.product_id.as_deref(), Some("monthly_premium"));
            }
            other => panic!("expected a purchase error, got {other:?}"),
        }
    }

    /// An undecodable event is dropped rather than panicking across the JNI
    /// frame or being reported as some other event.
    #[test]
    fn an_undecodable_event_is_dropped() {
        assert!(event_from_wire("purchase-refunded", &purchase_payload("sku")).is_none());
        assert!(event_from_wire(EVENT_PURCHASE_UPDATED, "not json at all").is_none());
        assert!(event_from_wire(EVENT_PURCHASE_ERROR, r#"{"nope": true}"#).is_none());
    }

    /// The whole callback path: what the export does with a delivered event
    /// reaches every registered listener.
    #[test]
    fn a_delivered_event_fans_out_to_every_listener() {
        let _guard = crate::event::test_guard();
        let (tx, rx) = mpsc::channel();

        let first = {
            let tx = tx.clone();
            crate::event::register(Box::new(move |event| tx.send(event).unwrap()))
        };
        let second = crate::event::register(Box::new(move |event| tx.send(event).unwrap()));

        deliver_event(EVENT_PURCHASE_UPDATED, &purchase_payload("premium_upgrade"));
        crate::event::flush();

        let received: Vec<IapEvent> = rx.try_iter().collect();
        assert_eq!(received.len(), 2, "both listeners saw the event");
        for event in &received {
            match event {
                IapEvent::PurchaseUpdated(purchase) => {
                    assert_eq!(purchase.product_id, "premium_upgrade");
                }
                other => panic!("expected a purchase update, got {other:?}"),
            }
        }

        // An undecodable event reaches nobody rather than emitting a fabricated
        // one.
        deliver_event(EVENT_PURCHASE_UPDATED, "{}");
        crate::event::flush();
        assert!(rx.try_recv().is_err());

        first.remove();
        second.remove();
    }

    /// An OpenIAP error payload becomes the typed store error an app matches
    /// on, with the fields this crate keeps.
    #[test]
    fn an_openiap_error_payload_becomes_a_typed_store_error() {
        match map_host_error(&error_payload("item-unavailable")) {
            IapError::Store(error) => {
                assert_eq!(error.code, IapErrorCode::ItemUnavailable);
                assert_eq!(error.message, "the store said no");
                assert_eq!(error.product_id.as_deref(), Some("monthly_premium"));
            }
            other => panic!("expected a store error, got {other:?}"),
        }
    }

    /// A code this crate doesn't recognize still parses (the forward-compatible
    /// `Unknown` fallback), rather than degrading to an untyped platform error.
    #[test]
    fn an_unrecognized_error_code_still_parses_as_a_store_error() {
        match map_host_error(&error_payload("some-future-code")) {
            IapError::Store(error) => assert_eq!(error.code, IapErrorCode::Unknown),
            other => panic!("expected a store error, got {other:?}"),
        }
    }

    /// A payload that isn't an error shape at all degrades to a platform error
    /// carrying the raw JSON, rather than being reported as a store refusal it
    /// may not be.
    #[test]
    fn an_unparseable_error_payload_degrades_to_a_platform_error() {
        match map_host_error("<html>gateway timeout</html>") {
            IapError::Platform(message) => assert!(message.contains("gateway timeout")),
            other => panic!("expected a platform error, got {other:?}"),
        }
    }
}
