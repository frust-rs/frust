//! The iOS [`Backend`] — StoreKit, reached through the `FrustIapBridge` Swift
//! glue (`plugins/iap/platform/ios`) as JSON strings over the ObjC runtime.
//!
//! `#[cfg(target_os = "ios")]` — iOS only, not `target_vendor = "apple"`: this
//! crate has no macOS arm to share with (see [`crate::desktop`]'s deferral),
//! and the bridge it drives is iOS-side Swift.
//!
//! # Shape: one selector, one completion, no correlation table
//!
//! Every operation below is the same three steps — encode the arguments as
//! JSON, message `-[FrustIapBridge call:argsJson:completion:]`, and park on a
//! rendezvous channel the completion block fulfills. **The completion *is* the
//! correlation**: the glue fires it exactly once for the call it belongs to, so
//! neither side keeps a request-id map (unlike `frust-camera`'s Android arm,
//! whose host callbacks carry no such per-call block).
//!
//! The wire shapes are the Swift class's own doc, and this module is written
//! against it verbatim:
//!
//! - arguments are **always a JSON object** (`{}` when there are none);
//! - a success answers `{"value": …}` — [`Reply`] — with `null` for the calls
//!   that return nothing;
//! - a failure answers either the spec's `{code, message, productId?}` (a
//!   store-reported `PurchaseError` → [`IapError::Store`]) or a code-less
//!   `{message}` (a glue-level failure: an unknown method, undecodable
//!   arguments, a result the glue itself could not encode →
//!   [`IapError::Platform`]). A payload that is neither parses as
//!   [`IapError::Serialization`], as does a reply that does not fit the type
//!   the caller expected.
//!
//! Which shape a failure takes is decided on the Swift side by the **type**
//! of the throw (`PurchaseError` vs the glue's own `BridgeError`), never by
//! re-labelling: a local encode failure is the glue's, so it never borrows a
//! spec error code and cannot reach an app as a store refusal the store never
//! made.
//!
//! [`crate::types`]'s two designed envelopes cross here unchanged:
//! `FetchProductsResult`'s `{"type": …, "items": […]}` and `IapEvent`'s
//! `{"event": …, "data": …}`.
//!
//! # Threading
//!
//! Every call parks the calling thread until the glue answers or the wait
//! bounds out ([`STORE_TIMEOUT`]/[`PROMPT_TIMEOUT`]/[`ACK_TIMEOUT`]), so a lost
//! answer degrades to a typed error instead of a permanently parked thread —
//! `frust-camera`'s bounded-wait precedent. The completion block runs on a
//! Swift cooperative-pool thread, never the caller's.
//!
//! The main-thread refusal ([`reject_on_main_thread`], guard-order step 2) is
//! what makes that parking safe: several of the StoreKit calls behind the glue
//! hop to the main actor internally (`showManageSubscriptions`, the purchase
//! sheet's scene lookup), so a main-thread caller would park on the very queue
//! that has to deliver its answer. [`Backend::request_purchase`] is the one
//! exception and deliberately has **no** step 2 — see below.
//!
//! The delivery-thread refusal ([`reject_on_delivery_thread`], guard-order step
//! 0) is the same argument one thread over, and has **no** exception: parking
//! the plugin's own event thread stalls every later purchase event, so every
//! method here — `request_purchase` included — refuses it first.
//!
//! Events arrive on the **main thread**: OpenIAP snapshots its listeners in a
//! `Task` and then invokes them inside `await MainActor.run { … }`, so the
//! glue's sink block — and this module's [`deliver`] — run there. They only
//! queue: [`crate::event::emit`] hands the event to the plugin-owned delivery
//! thread and returns, which is what makes the crate doc's "never the UI
//! thread" promise true on iOS. The listener contract (don't block, hand off)
//! still binds, now against that thread rather than this one.
//!
//! # Two calls acknowledge rather than complete
//!
//! `requestPurchase` and `deepLinkToSubscriptions` resolve on the Swift side
//! only when the *user* is done with a StoreKit sheet. The glue therefore acks
//! them as soon as the request is dispatched and lets the real `await` run on
//! detached, which is exactly what this crate's own contracts already say:
//! [`crate::Iap::request_purchase`] returns "the store accepted the request"
//! and reports the outcome on the event stream, and
//! [`crate::Iap::deep_link_to_subscriptions`] returns when "the platform
//! accepts the deep link". Two consequences worth naming:
//!
//! - a purchase the store rejects outright reports as an
//!   [`crate::IapEvent::PurchaseError`] rather than as this call's `Err` —
//!   the event stream is the documented single source of truth either way,
//!   and carries the failure exactly once (OpenIAP publishes it on its own
//!   error listener before rethrowing, so the glue's detached `catch` logs
//!   instead of re-emitting; its one synthesized event covers a throw that is
//!   not a `PurchaseError`, which upstream never produces today);
//! - a deep link that fails *after* dispatch is logged by the glue and
//!   reported nowhere else. That is this backend's one blind spot.
//!
//! Because the ack resolves without the main thread, `request_purchase` can
//! park on it briefly from any thread — which is why it keeps its non-blocking,
//! UI-thread-callable contract while still reporting a malformed request
//! synchronously.
//!
//! # Connection state, idempotency, and the event sink
//!
//! [`CONNECTED`] is this crate's own view of the store connection: guard-order
//! step 3 reads it, [`Backend::init_connection`] sets it, and
//! [`Backend::end_connection`] clears it. **`init_connection`'s idempotency is
//! mapped here rather than in the glue**: a second call short-circuits on that
//! flag exactly as `OpenIapModule.initConnection` short-circuits on its own,
//! and a platform "already prepared" refusal that reaches us anyway is mapped
//! onto the same `Ok(true)`.
//!
//! The event sink is installed once, on the first successful connection
//! ([`install_event_sink`]) — nothing can emit before that, and OpenIAP's
//! listeners survive an `endConnection`, so re-installing on a reconnect would
//! double every event.
//!
//! # Replay and dedup
//!
//! The glue registers the purchase-updated listener with
//! `dedupeTransactionIOS: true`, so one transaction id is delivered at most
//! once **per connection session**. StoreKit still replays an unfinished
//! transaction on the next app launch, so an app may see the same purchase
//! again after a restart — [`crate::Iap::finish_transaction`] is what stops
//! that, and there is deliberately no cross-launch dedupe (it would need
//! persistence neither this crate nor the glue owns).
//!
//! # `unsafe`
//!
//! Confined to this module and each `# Safety`/`SAFETY`-noted, the
//! `frust-camera`/`frust-clipboard` apple-backend precedent
//! (`docs/CODE_STANDARDS.md`'s sanctioned zones): `objc2` marks an untyped
//! message send `unsafe` (there is no generated binding for a class this crate
//! resolves by runtime name), and the two completion blocks receive raw
//! `NSString` pointers whose validity only the glue's own contract establishes.
//! Both block bodies wrap their work in [`catch_unwind`] so nothing unwinds
//! into the Swift frame that called them (`docs/CODE_STANDARDS.md`'s
//! no-unwind-across-FFI rule; this crate cannot use `frust-shell-common`'s
//! `guard`, which lives above the plugin charter line).
//!
//! # What is checked where
//!
//! The wire codec and the park/answer machinery ([`interpret`],
//! [`await_answer`], [`decode`]) are exercised by this module's own tests
//! against a **fake boundary** — a thread standing in for the completion block
//! — so the timeout, the two error shapes and the reply envelope are pinned
//! without a device or a store account.
//!
//! Those tests are iOS-target-only, so a plain `cargo test` never sees them.
//! The iOS compile gate builds them (`cargo check --all-targets --target
//! aarch64-apple-ios-sim -p frust-iap`), and a booted Simulator *runs* them
//! without any Xcode project, since a simulator-target test binary is an
//! ordinary executable there:
//!
//! ```text
//! CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn booted" \
//!   cargo test --target aarch64-apple-ios-sim -p frust-iap --lib
//! ```
//!
//! What they deliberately do **not** cover is the ObjC hop itself
//! ([`invoke`]'s message send and block) and StoreKit's own behavior — a
//! device/Simulator app is the only thing that can. That is the same split the
//! crate's [`crate::mock`] fake store exists for: the platform-independent
//! contract (connection state machine, two-phase purchase, guard order) is
//! checked there, on every host.

use std::ffi::CStr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Once, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_foundation::NSString;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::types::{
    ActiveSubscription, FetchProductsResult, IapErrorCode, IapEvent, IapPurchaseError,
    ProductRequest, Purchase, PurchaseInput, PurchaseOptions, RequestPurchaseProps,
};
use crate::{Backend, IapError};

/// The bare `@objc(...)` runtime name of the Swift glue class, resolved by
/// name rather than linked by symbol — the iOS half of
/// `docs/CODE_STANDARDS.md`'s runtime-name LAW, the same way
/// `frust-camera`'s `CameraPreviewFactory` is reached. Renaming either side
/// alone is a breaking FFI change that fails as a `nil` lookup, not a link
/// error, which is also why the class must survive the app's `-dead_strip`.
const BRIDGE_CLASS_NAME: &CStr = c"FrustIapBridge";

/// The event names the glue forwards, the OpenIAP spec's own event strings.
const EVENT_PURCHASE_UPDATED: &str = "purchase-updated";
/// See [`EVENT_PURCHASE_UPDATED`].
const EVENT_PURCHASE_ERROR: &str = "purchase-error";

/// How long a store round trip may take before it reports a timeout.
///
/// A product/purchase query is a network call against the App Store; 30s is a
/// "the answer is never coming" bound, not a performance budget.
const STORE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long [`Backend::restore_purchases`] waits.
///
/// Generous on purpose, matching `frust-camera`'s permission-dialog bound:
/// StoreKit's `sync` may present an App Store password prompt, and the call
/// resolves only once the user has dealt with it.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a dispatch-only ack waits (the module doc's *Two calls
/// acknowledge* section). Nothing on that path talks to the store or the user
/// — it decodes arguments and starts a task — so a wait this long already
/// means the glue never ran.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether [`Backend::init_connection`] has opened the store connection and
/// [`Backend::end_connection`] has not closed it — guard-order step 3.
///
/// Process-global like the connection it tracks (there is one StoreKit client
/// per process), and a plain atomic so the check holds under `panic = "abort"`
/// (`docs/PLUGINS_ARCHITECTURE.md`'s pre-init convention).
static CONNECTED: AtomicBool = AtomicBool::new(false);

/// Installs the event sink exactly once per process — see
/// [`install_event_sink`].
static EVENT_SINK: Once = Once::new();

/// The Apple backend. Stateless: the connection flag above and the sink latch
/// are process-global, exactly like the StoreKit state they mirror.
pub(crate) struct AppleIap;

/// One answer from the glue: `(resultJson, errorJson)`, exactly one of which
/// is populated.
type Answer = (Option<String>, Option<String>);

/// The `{"value": …}` envelope every successful reply carries — a JSON object
/// rather than a bare value so neither side needs `JSONSerialization`'s
/// fragment mode for a `true`/`"USA"`/`null` result.
#[derive(Deserialize)]
struct Reply {
    value: serde_json::Value,
}

/// The glue's code-less error shape: a failure that never reached the store
/// (an unknown method, an argument the glue could not decode).
#[derive(Deserialize)]
struct GlueError {
    message: String,
}

// --- The boundary ------------------------------------------------------------

/// The glue class, or `None` when the app was built without the plugin's Swift
/// package (or stripped it) — guard-order step 1.
///
/// Cached: a runtime lookup by name is a hash lookup, but the answer cannot
/// change within a process, and step 1 must stay a plain check rather than a
/// platform round trip.
fn bridge_class() -> Option<&'static AnyClass> {
    static CLASS: OnceLock<Option<&'static AnyClass>> = OnceLock::new();
    *CLASS.get_or_init(|| AnyClass::get(BRIDGE_CLASS_NAME))
}

/// Guard-order step 1: the glue's singleton, or
/// [`IapError::PlatformNotInitialized`] when this app has no bridge at all.
fn bridge() -> Result<Retained<AnyObject>, IapError> {
    let class = bridge_class().ok_or(IapError::PlatformNotInitialized)?;
    // SAFETY: `+[FrustIapBridge shared]` is the class property the Swift glue
    // declares (`@objc public static let shared`), so the selector exists on
    // the class resolved above and returns an object at +0. `Option` rather
    // than a bare `Retained` because a `nil` answer must degrade to the typed
    // error below, never to `msg_send!`'s own panic across an FFI frame.
    let shared: Option<Retained<AnyObject>> = unsafe { msg_send![class, shared] };
    shared.ok_or(IapError::PlatformNotInitialized)
}

/// Guard-order step 2: refuse a blocking call made on the platform's UI
/// thread, the Apple half of the crate-wide rule whose Android half reads
/// `Looper.myLooper() == Looper.getMainLooper()`.
///
/// `MainThreadMarker::new()` is `pthread_main_np()` underneath — a thread
/// identity check, no message send and no allocation — so it is cheap enough
/// to sit in front of every entry point.
fn reject_on_main_thread() -> Result<(), IapError> {
    if MainThreadMarker::new().is_some() {
        return Err(IapError::UiThread);
    }
    Ok(())
}

/// Guard-order step 0: refuse a call made from inside a purchase listener, on
/// the plugin's own event-delivery thread.
///
/// Unlike step 2 this one binds **every** method, including
/// [`Backend::request_purchase`]: its ack is short, but parking the one thread
/// every purchase event is delivered on for even a few seconds stalls every
/// later event. A thread-id comparison, so it runs ahead of the class lookup in
/// [`bridge`] — see [`crate::Backend`]'s guard order for why step 0 can
/// precede step 1 without masking it.
fn reject_on_delivery_thread() -> Result<(), IapError> {
    crate::event::reject_from_delivery_thread()
}

/// Guard-order step 3.
fn require_connected() -> Result<(), IapError> {
    if CONNECTED.load(Ordering::Acquire) {
        return Ok(());
    }
    Err(IapError::NotConnected)
}

/// The full guard order (delivery thread → readiness → UI thread → connection
/// state) in one place, for the store-touching calls that need all four.
///
/// The three calls that differ — `init_connection`/`end_connection` (no step
/// 3; opening or closing *is* the state change) and `request_purchase` (no
/// step 2; see the module doc) — compose the steps themselves so the deviation
/// is visible at the call site rather than hidden in a flag. **Step 0 is not
/// one of the deviations**: all four compose it first.
fn connected_bridge() -> Result<Retained<AnyObject>, IapError> {
    reject_on_delivery_thread()?;
    let bridge = bridge()?;
    reject_on_main_thread()?;
    require_connected()?;
    Ok(bridge)
}

/// Run `method` across the boundary and deserialize its result.
fn call<T: DeserializeOwned>(
    bridge: &AnyObject,
    method: &str,
    args: &serde_json::Value,
    timeout: Duration,
) -> Result<T, IapError> {
    decode(invoke(bridge, method, args, timeout)?, method)
}

/// Message the glue and park until its completion answers, or `timeout`.
fn invoke(
    bridge: &AnyObject,
    method: &str,
    args: &serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, IapError> {
    let args_json = serde_json::to_string(args).map_err(|error| {
        IapError::Serialization(format!("{method}: unencodable arguments: {error}"))
    })?;
    let method_string = NSString::from_str(method);
    let args_string = NSString::from_str(&args_json);

    let (tx, rx) = sync_channel::<Answer>(1);
    let completion = RcBlock::new(move |result: *mut NSString, error: *mut NSString| {
        // Nothing may unwind back into the Swift frame that called this block.
        let answer = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the glue passes either null or a live `NSString` it owns
            // for the duration of this call, and this borrow does not outlive
            // the block body.
            unsafe { (read_string(result), read_string(error)) }
        }))
        .unwrap_or_else(|_| {
            log::error!("frust-iap: panicked while reading the iOS bridge's answer");
            (None, None)
        });
        // A full or disconnected channel means the caller already timed out
        // and moved on — dropping the late answer is correct, and `try_send`
        // keeps this arbitrary-thread callback from ever blocking.
        let _ = tx.try_send(answer);
    });

    // SAFETY: `-[FrustIapBridge call:argsJson:completion:]` takes two
    // `NSString`s and a `void (^)(NSString *, NSString *)` block, which is
    // exactly what is passed here; `bridge` is that class's own singleton. The
    // block outliving this stack frame is the glue's business, not ours: it is
    // declared `@escaping` on the Swift side, so the bridged closure retains
    // the block, and a late completion therefore fires into a live block whose
    // send lands nowhere.
    unsafe {
        let _: () = msg_send![
            bridge,
            call: &*method_string,
            argsJson: &*args_string,
            completion: &*completion,
        ];
    }

    await_answer(&rx, timeout, method)
}

/// `ptr` as an owned [`String`], or `None` for a null pointer.
///
/// # Safety
/// `ptr` must be null or a valid, live `NSString` for the duration of the
/// call.
unsafe fn read_string(ptr: *mut NSString) -> Option<String> {
    // SAFETY: the caller contract above is exactly `as_ref`'s.
    unsafe { ptr.as_ref() }.map(ToString::to_string)
}

/// Park on the rendezvous channel until the completion answers.
///
/// Split out from [`invoke`] so the whole park/answer path is testable against
/// a thread standing in for the block (this module's tests).
fn await_answer(
    rx: &Receiver<Answer>,
    timeout: Duration,
    method: &str,
) -> Result<serde_json::Value, IapError> {
    match rx.recv_timeout(timeout) {
        Ok(answer) => interpret(answer, method),
        Err(RecvTimeoutError::Timeout) => Err(IapError::Platform(format!(
            "{method}: the iOS bridge did not answer within {}s",
            timeout.as_secs()
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(IapError::Platform(format!(
            "{method}: the iOS bridge dropped the call without answering"
        ))),
    }
}

/// One `(result, error)` pair as a typed outcome — the module doc's wire
/// contract, in code.
///
/// The unreadable-reply arm bounds what it quotes ([`crate::types::payload_excerpt`]):
/// serde's diagnosis quotes the value it rejected, and the reply it is
/// rejecting can be a purchase carrying a bearer token.
fn interpret(answer: Answer, method: &str) -> Result<serde_json::Value, IapError> {
    match answer {
        // Checked first: a glue that somehow populated both is reporting a
        // failure, and reading the result would be reading a stale one.
        (_, Some(error)) => Err(bridge_error(&error, method)),
        (Some(result), None) => serde_json::from_str::<Reply>(&result)
            .map(|reply| reply.value)
            .map_err(|error| {
                IapError::Serialization(format!(
                    "{method}: unreadable reply: {}",
                    crate::types::payload_excerpt(&error.to_string())
                ))
            }),
        (None, None) => Err(IapError::Platform(format!(
            "{method}: the iOS bridge answered with neither a result nor an error"
        ))),
    }
}

/// An error payload as a typed error: the spec shape (it carries `code`) is a
/// store failure, the code-less shape is a glue failure, anything else is a
/// contract break between the two sides.
///
/// The last arm is the only one that quotes the payload, and quotes a bounded
/// excerpt of it ([`crate::types::payload_excerpt`]) — an unrecognized payload
/// is exactly the case where nothing is known about what it holds.
fn bridge_error(payload: &str, method: &str) -> IapError {
    if let Ok(store) = serde_json::from_str::<IapPurchaseError>(payload) {
        return IapError::Store(store);
    }
    if let Ok(glue) = serde_json::from_str::<GlueError>(payload) {
        return IapError::Platform(glue.message);
    }
    IapError::Serialization(format!(
        "{method}: unreadable error payload: {}",
        crate::types::payload_excerpt(payload)
    ))
}

/// A reply's `value` as the type the caller expected.
///
/// Bounded for the same reason as [`interpret`]'s reply arm: this is the step
/// that decodes a purchase list, so serde's rejected-value quote is the one
/// most likely to hold a token.
fn decode<T: DeserializeOwned>(value: serde_json::Value, method: &str) -> Result<T, IapError> {
    serde_json::from_value(value).map_err(|error| {
        IapError::Serialization(format!(
            "{method}: unexpected result: {}",
            crate::types::payload_excerpt(&error.to_string())
        ))
    })
}

/// `value` as this bridge's argument JSON.
fn to_args<T: serde::Serialize>(value: &T, method: &str) -> Result<serde_json::Value, IapError> {
    serde_json::to_value(value)
        .map_err(|error| IapError::Serialization(format!("{method}: unencodable request: {error}")))
}

// --- Events ------------------------------------------------------------------

/// Hand the glue a block to forward every purchase event through, once per
/// process.
///
/// The block is deliberately leaked: it outlives every call by design (the
/// glue holds it for the process lifetime, and there is no unregister path on
/// either side), so there is exactly one leak here rather than a `'static`
/// dance around a value nothing can ever drop.
fn install_event_sink(bridge: &AnyObject) {
    EVENT_SINK.call_once(|| {
        let sink = RcBlock::new(move |name: *mut NSString, payload: *mut NSString| {
            // As in `invoke`: no unwind may reach the Swift caller. The
            // registry catches a panicking *listener* itself, so this catches
            // only what happens on the way in.
            let _ = catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: the glue passes two live `NSString`s (never null) and
                // this borrow does not outlive the block body.
                let (name, payload) = unsafe { (read_string(name), read_string(payload)) };
                deliver(name.as_deref(), payload.as_deref());
            }));
        });

        // SAFETY: `-[FrustIapBridge setEventSink:]` takes exactly one
        // `void (^)(NSString *, NSString *)` block, which is what is passed.
        unsafe {
            let _: () = msg_send![bridge, setEventSink: &*sink];
        }
        std::mem::forget(sink);
    });
}

/// Turn one forwarded `(name, envelope)` pair into a queued
/// [`crate::IapEvent`].
///
/// Split out of the block so it is plain, testable Rust: the name selects
/// whether this crate models the event at all, and the envelope — the
/// adjacently-tagged shape [`crate::types`] designs — carries the payload.
/// Runs on the main thread (the module doc's *Threading*) and runs no
/// listener there: [`crate::event::emit`] queues for the plugin-owned
/// delivery thread.
///
/// An event this crate models whose payload will not decode is **not**
/// dropped: it is emitted as the synthesized failure
/// [`crate::event::decode_failure`] builds, so a StoreKit transaction that
/// really happened is reported rather than silently lost. An unmodelled name
/// stays ignored — there is nothing for this crate to report about it.
///
/// The log line names the kind only; serde's own diagnosis rides on the
/// synthesized event, because an `invalid type` message quotes the value it
/// rejected and this payload carries the JWS bearer credential ([`Purchase`]'s
/// doc).
fn deliver(name: Option<&str>, payload: Option<&str>) {
    let (Some(name), Some(payload)) = (name, payload) else {
        log::warn!("frust-iap: the iOS bridge forwarded an event with no name or no payload");
        return;
    };
    if !matches!(name, EVENT_PURCHASE_UPDATED | EVENT_PURCHASE_ERROR) {
        log::debug!("frust-iap: ignoring an unmodelled iOS purchase event: {name}");
        return;
    }
    match serde_json::from_str::<IapEvent>(payload) {
        Ok(event) => crate::event::emit(&event),
        Err(error) => {
            log::warn!(
                "frust-iap: a {name} event payload did not decode; reporting it as a purchase \
                 failure rather than dropping it (the decoder's diagnosis rides on that event)"
            );
            crate::event::emit(&crate::event::decode_failure(name, &error.to_string()));
        }
    }
}

// --- The backend -------------------------------------------------------------

impl Backend for AppleIap {
    fn init_connection(&self, config: Option<&serde_json::Value>) -> Result<bool, IapError> {
        reject_on_delivery_thread()?;
        let bridge = bridge()?;
        reject_on_main_thread()?;
        // Idempotent by contract: a second call is a success, not an error.
        if CONNECTED.load(Ordering::Acquire) {
            return Ok(true);
        }

        let args = json!({ "config": config });
        let opened = match call::<bool>(&bridge, "initConnection", &args, STORE_TIMEOUT) {
            Ok(opened) => opened,
            // The platform's own already-prepared refusal is this contract's
            // success — the same mapping the crate doc promises.
            Err(IapError::Store(error)) if error.code == IapErrorCode::AlreadyPrepared => true,
            Err(error) => return Err(error),
        };

        if opened {
            CONNECTED.store(true, Ordering::Release);
            install_event_sink(&bridge);
        }
        Ok(opened)
    }

    fn end_connection(&self) -> Result<bool, IapError> {
        reject_on_delivery_thread()?;
        let bridge = bridge()?;
        reject_on_main_thread()?;
        if !CONNECTED.swap(false, Ordering::AcqRel) {
            // Closed nothing, which is not an error.
            return Ok(false);
        }

        // Reports what it closed rather than how the teardown went: this
        // crate's connection is shut either way, and a caller has nothing to
        // do about a StoreKit-side teardown complaint.
        if let Err(error) = call::<bool>(&bridge, "endConnection", &json!({}), STORE_TIMEOUT) {
            log::warn!("frust-iap: the iOS store connection reported {error} while closing");
        }
        Ok(true)
    }

    fn fetch_products(&self, request: &ProductRequest) -> Result<FetchProductsResult, IapError> {
        let bridge = connected_bridge()?;
        let args = to_args(request, "fetchProducts")?;
        call(&bridge, "fetchProducts", &args, STORE_TIMEOUT)
    }

    fn get_available_purchases(
        &self,
        options: Option<&PurchaseOptions>,
    ) -> Result<Vec<Purchase>, IapError> {
        let bridge = connected_bridge()?;
        call(
            &bridge,
            "getAvailablePurchases",
            &json!({ "options": options }),
            STORE_TIMEOUT,
        )
    }

    fn get_active_subscriptions(
        &self,
        ids: Option<&[String]>,
    ) -> Result<Vec<ActiveSubscription>, IapError> {
        let bridge = connected_bridge()?;
        call(
            &bridge,
            "getActiveSubscriptions",
            &json!({ "subscriptionIds": ids }),
            STORE_TIMEOUT,
        )
    }

    fn get_storefront(&self) -> Result<String, IapError> {
        let bridge = connected_bridge()?;
        call(&bridge, "getStorefront", &json!({}), STORE_TIMEOUT)
    }

    /// Guard order **without step 2**: this call is documented non-blocking
    /// and callable from any thread including the UI thread, and the ack it
    /// parks on resolves off the main actor (module doc). Step 0 still binds:
    /// "any thread" excludes the one thread that has to deliver the outcome.
    fn request_purchase(&self, props: &RequestPurchaseProps) -> Result<(), IapError> {
        reject_on_delivery_thread()?;
        let bridge = bridge()?;
        require_connected()?;
        let args = to_args(props, "requestPurchase")?;
        call(&bridge, "requestPurchase", &args, ACK_TIMEOUT)
    }

    fn finish_transaction(
        &self,
        purchase: &PurchaseInput,
        is_consumable: Option<bool>,
    ) -> Result<(), IapError> {
        let bridge = connected_bridge()?;
        call(
            &bridge,
            "finishTransaction",
            &json!({ "purchase": purchase, "isConsumable": is_consumable }),
            STORE_TIMEOUT,
        )
    }

    fn restore_purchases(&self) -> Result<(), IapError> {
        let bridge = connected_bridge()?;
        call(&bridge, "restorePurchases", &json!({}), PROMPT_TIMEOUT)
    }

    fn deep_link_to_subscriptions(
        &self,
        options: Option<&serde_json::Value>,
    ) -> Result<(), IapError> {
        let bridge = connected_bridge()?;
        call(
            &bridge,
            "deepLinkToSubscriptions",
            &json!({ "options": options }),
            ACK_TIMEOUT,
        )
    }

    /// iOS **is** the platform that has this operation, so the trait default
    /// is overridden here (and only here).
    fn get_pending_transactions(&self) -> Result<Vec<Purchase>, IapError> {
        let bridge = connected_bridge()?;
        call(
            &bridge,
            "getPendingTransactionsIOS",
            &json!({}),
            STORE_TIMEOUT,
        )
    }

    // `acknowledge_purchase`/`consume_purchase` are deliberately NOT
    // overridden: they are Android-only, so the trait's
    // `NotSupportedOnPlatform` default is the correct and permanent answer
    // here — StoreKit has no acknowledgement step.
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::SyncSender;
    use std::thread;

    use super::*;
    use crate::types::{IapPlatform, ProductType};

    /// A rendezvous channel plus a stand-in for the completion block: the same
    /// pair [`invoke`] builds, with a test driving the sender instead of the
    /// Swift side.
    fn boundary() -> (SyncSender<Answer>, Receiver<Answer>) {
        sync_channel::<Answer>(1)
    }

    /// One successful reply, encoded the way the glue encodes it.
    fn reply(value: serde_json::Value) -> Answer {
        (Some(json!({ "value": value }).to_string()), None)
    }

    #[test]
    fn a_completion_that_answers_late_still_unparks_the_caller() {
        let (tx, rx) = boundary();
        // The block fires from another thread, after the caller is already
        // parked — the whole point of the rendezvous.
        let firing = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            tx.try_send(reply(json!("USA"))).unwrap();
        });

        let value = await_answer(&rx, Duration::from_secs(5), "getStorefront").unwrap();
        assert_eq!(
            decode::<String>(value, "getStorefront").unwrap(),
            "USA".to_owned()
        );
        firing.join().unwrap();
    }

    #[test]
    fn a_completion_that_never_fires_times_out_rather_than_parking_forever() {
        let (tx, rx) = boundary();
        let error = await_answer(&rx, Duration::from_millis(20), "fetchProducts").unwrap_err();
        assert!(
            matches!(&error, IapError::Platform(message) if message.contains("fetchProducts")),
            "{error:?}"
        );
        drop(tx);
    }

    /// A block dropped without ever firing (a glue that lost the call) is
    /// diagnosably different from a slow one.
    #[test]
    fn a_dropped_completion_reports_a_dropped_call() {
        let (tx, rx) = boundary();
        drop(tx);
        let error = await_answer(&rx, Duration::from_secs(30), "getStorefront").unwrap_err();
        assert!(
            matches!(&error, IapError::Platform(message) if message.contains("without answering")),
            "{error:?}"
        );
    }

    /// The spec error shape (it carries `code`) is a store failure.
    #[test]
    fn an_error_payload_with_a_code_is_a_store_error() {
        let payload = json!({
            "code": "user-cancelled",
            "message": "User cancelled the purchase flow",
            "productId": "premium_upgrade",
        })
        .to_string();

        let error = interpret((None, Some(payload)), "requestPurchase").unwrap_err();
        let IapError::Store(store) = error else {
            panic!("expected a store error, got {error:?}");
        };
        assert_eq!(store.code, IapErrorCode::UserCancelled);
        assert_eq!(store.product_id.as_deref(), Some("premium_upgrade"));
    }

    /// The glue's code-less shape is a boundary failure, not a store one.
    #[test]
    fn an_error_payload_without_a_code_is_a_platform_error() {
        let payload = json!({ "message": "unknown method `nope`" }).to_string();

        let error = interpret((None, Some(payload)), "nope").unwrap_err();
        assert!(
            matches!(&error, IapError::Platform(message) if message == "unknown method `nope`"),
            "{error:?}"
        );
    }

    /// Neither shape: the two sides disagree about the wire, which is a
    /// serialization failure rather than a store or platform one.
    #[test]
    fn an_unreadable_error_payload_is_a_serialization_error() {
        let error =
            interpret((None, Some("not json at all".to_owned())), "getStorefront").unwrap_err();
        assert!(matches!(error, IapError::Serialization(_)), "{error:?}");
    }

    /// An unreadable payload is quoted as a bounded excerpt, never whole: a
    /// payload this side cannot recognize is exactly the one nothing is known
    /// about, and a purchase document holds a bearer token.
    #[test]
    fn an_unreadable_payload_is_truncated_before_it_reaches_the_error() {
        let secret = "opaque-bearer-token-value";
        let padding = "x".repeat(crate::types::PAYLOAD_EXCERPT_BYTES * 2);

        let error = interpret(
            (None, Some(format!("<html>{padding}{secret}</html>"))),
            "getAvailablePurchases",
        )
        .unwrap_err();
        let IapError::Serialization(message) = &error else {
            panic!("expected a serialization error, got {error:?}");
        };
        assert!(!message.contains(secret), "{message}");
        assert!(message.contains("truncated"), "{message}");

        // The reply path bounds serde's own diagnosis the same way — it quotes
        // the value it rejected, and that value comes out of the payload (here
        // a token-shaped string where a timestamp was expected).
        let error = decode::<Vec<Purchase>>(
            json!([{
                "id": "GPA.0000-1111-2222-33333",
                "productId": "monthly_premium",
                "transactionDate": format!("{padding}{secret}"),
                "store": "google",
                "quantity": 1,
                "purchaseState": "purchased",
                "isAutoRenewing": false,
            }]),
            "getAvailablePurchases",
        )
        .unwrap_err();
        let IapError::Serialization(message) = &error else {
            panic!("expected a serialization error, got {error:?}");
        };
        assert!(!message.contains(secret), "{message}");
        assert!(
            message.contains("truncated"),
            "serde quoted the rejected value, so the bound must have applied: {message}"
        );
    }

    /// An error wins over a result, so a glue bug cannot present a stale
    /// success.
    #[test]
    fn an_error_beside_a_result_still_reports_the_error() {
        let answer = (
            Some(json!({ "value": true }).to_string()),
            Some(json!({ "message": "boom" }).to_string()),
        );
        assert!(matches!(
            interpret(answer, "initConnection"),
            Err(IapError::Platform(_))
        ));
    }

    /// A completion carrying nothing at all is reported, never treated as a
    /// success.
    #[test]
    fn an_empty_answer_is_a_platform_error() {
        let error = interpret((None, None), "endConnection").unwrap_err();
        assert!(
            matches!(&error, IapError::Platform(message) if message.contains("neither")),
            "{error:?}"
        );
    }

    /// A reply that is not the `{"value": …}` envelope is a wire mismatch.
    #[test]
    fn a_reply_outside_the_envelope_is_a_serialization_error() {
        let error = interpret(
            (Some(json!({ "nope": 1 }).to_string()), None),
            "getStorefront",
        )
        .unwrap_err();
        assert!(matches!(error, IapError::Serialization(_)), "{error:?}");
    }

    /// The `fetchProducts` envelope, exactly as `types.rs` designs it
    /// (`FetchProductsResult`'s doc: `{"type": "products", "items": [...]}`),
    /// travelling inside the reply envelope.
    #[test]
    fn a_fetch_products_reply_decodes_into_the_designed_envelope() {
        let answer = reply(json!({
            "type": "products",
            "items": [{
                "__typename": "ProductIOS",
                "currency": "USD",
                "description": "Remove ads",
                "displayPrice": "$4.99",
                "id": "premium_upgrade",
                "platform": "ios",
                "price": 4.99,
                "title": "Premium Upgrade",
                "type": "in-app",
                "typeIOS": "consumable",
            }],
        }));

        let value = interpret(answer, "fetchProducts").unwrap();
        let result: FetchProductsResult = decode(value, "fetchProducts").unwrap();
        let FetchProductsResult::Products(products) = result else {
            panic!("the `products` tag must decode into the Products arm");
        };
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].id, "premium_upgrade");
        assert_eq!(products[0].kind, ProductType::InApp);
        assert_eq!(products[0].platform, IapPlatform::Ios);
        // Every iOS-specific field survives in `extra`, losslessly.
        assert_eq!(products[0].extra.get("typeIOS"), Some(&json!("consumable")));
    }

    /// The `all` arm carries both concrete iOS product shapes, so its element
    /// type ([`crate::ProductOrSubscription`]) has to accept a
    /// `ProductSubscriptionIOS` payload as well as a `ProductIOS` one.
    ///
    /// The field sets below are what a StoreKit-Testing `fetchProducts` run
    /// actually put on the wire (`jsonRepresentationIOS`'s opaque catalog blob
    /// abbreviated — it is an `extra` passthrough either way), so this pins the
    /// glue's real output against `types.rs`'s required fields rather than a
    /// hand-guessed shape.
    #[test]
    fn a_mixed_fetch_products_reply_decodes_both_product_shapes() {
        let answer = reply(json!({
            "type": "all",
            "items": [
                {
                    "__typename": "ProductIOS",
                    "currency": "USD",
                    "debugDescription": "Remove ads and unlock everything",
                    "description": "Remove ads and unlock everything",
                    "displayName": "Premium Upgrade",
                    "displayNameIOS": "Premium Upgrade",
                    "displayPrice": "$4.99",
                    "id": "dev.frusttest.iapsmoke.premium",
                    "isFamilyShareableIOS": false,
                    "jsonRepresentationIOS": "{}",
                    "platform": "ios",
                    "price": 4.99,
                    "title": "Premium Upgrade",
                    "type": "in-app",
                    "typeIOS": "non-consumable",
                },
                {
                    "__typename": "ProductSubscriptionIOS",
                    "currency": "USD",
                    "debugDescription": "Everything, monthly",
                    "description": "Everything, monthly",
                    "displayName": "Pro Monthly",
                    "displayNameIOS": "Pro Monthly",
                    "displayPrice": "$9.99",
                    "id": "dev.frusttest.iapsmoke.pro_monthly",
                    "introductoryPricePaymentModeIOS": "empty",
                    "isFamilyShareableIOS": false,
                    "jsonRepresentationIOS": "{}",
                    "platform": "ios",
                    "price": 9.99,
                    "subscriptionGroupIdIOS": "200001",
                    "subscriptionPeriodNumberIOS": "1",
                    "subscriptionPeriodUnitIOS": "month",
                    "title": "Pro Monthly",
                    "type": "subs",
                    "typeIOS": "auto-renewable-subscription",
                },
            ],
        }));

        let value = interpret(answer, "fetchProducts").unwrap();
        let result: FetchProductsResult = decode(value, "fetchProducts").unwrap();
        let FetchProductsResult::All(items) = result else {
            panic!("the `all` tag must decode into the All arm");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, ProductType::InApp);
        assert_eq!(items[1].kind, ProductType::Subs);
        assert_eq!(
            items[1].extra.get("subscriptionPeriodUnitIOS"),
            Some(&json!("month"))
        );
    }

    /// The calls that answer nothing send `null`, which is the unit type.
    #[test]
    fn a_void_reply_decodes_as_the_unit_type() {
        let value = interpret(reply(json!(null)), "restorePurchases").unwrap();
        decode::<()>(value, "restorePurchases").unwrap();
    }

    /// Arguments are always a JSON object — `{}` when the call takes none, and
    /// an explicit `null` member for an absent option (the glue reads both as
    /// "not provided").
    #[test]
    fn absent_arguments_still_serialize_as_an_object() {
        let options: Option<&PurchaseOptions> = None;
        assert_eq!(json!({ "options": options }), json!({ "options": null }));

        let request = ProductRequest {
            skus: vec!["premium_upgrade".to_owned()],
            kind: crate::types::ProductQueryType::Subs,
        };
        assert_eq!(
            to_args(&request, "fetchProducts").unwrap(),
            json!({ "skus": ["premium_upgrade"], "type": "subs" })
        );
    }

    /// The event envelope `types.rs` designs, delivered end to end through the
    /// registry — the sink block's own body, minus the pointer reads.
    #[test]
    fn a_forwarded_event_reaches_the_listeners() {
        let _guard = crate::event::test_guard();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = crate::Iap::set_purchase_listener(Box::new(move |event| {
            tx.send(event).unwrap();
        }));

        deliver(
            Some(EVENT_PURCHASE_ERROR),
            Some(
                &json!({
                    "event": "purchaseError",
                    "data": { "code": "user-cancelled", "message": "cancelled" },
                })
                .to_string(),
            ),
        );
        crate::event::flush();

        let IapEvent::PurchaseError(error) = rx.try_recv().unwrap() else {
            panic!("a purchase-error event must arrive as the PurchaseError variant");
        };
        assert_eq!(error.code, IapErrorCode::UserCancelled);
        handle.remove();
    }

    /// An event this crate does not model at all, and a forward with nothing in
    /// it, are both dropped rather than delivered as something else.
    #[test]
    fn an_unmodelled_event_is_dropped() {
        let _guard = crate::event::test_guard();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = crate::Iap::set_purchase_listener(Box::new(move |event| {
            tx.send(event).unwrap();
        }));

        deliver(Some("promoted-product-ios"), Some("{}"));
        deliver(None, None);
        crate::event::flush();

        assert!(rx.try_recv().is_err(), "nothing should have been delivered");
        handle.remove();
    }

    /// A payload under a name this crate *does* model, which will not decode,
    /// is reported as a typed parse failure rather than dropped — an
    /// undecodable `purchase-updated` is a StoreKit transaction that already
    /// happened.
    #[test]
    fn a_malformed_modelled_event_synthesizes_a_purchase_error() {
        let _guard = crate::event::test_guard();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = crate::Iap::set_purchase_listener(Box::new(move |event| {
            tx.send(event).unwrap();
        }));

        deliver(Some(EVENT_PURCHASE_UPDATED), Some("{ not json"));
        deliver(
            Some(EVENT_PURCHASE_ERROR),
            Some(&json!({ "event": "purchaseError", "data": { "nope": true } }).to_string()),
        );
        crate::event::flush();

        let delivered: Vec<IapEvent> = rx.try_iter().collect();
        assert_eq!(delivered.len(), 2);
        for event in &delivered {
            let IapEvent::PurchaseError(error) = event else {
                panic!("expected a synthesized parse failure, got {event:?}");
            };
            assert_eq!(error.code, IapErrorCode::BillingResponseJsonParseError);
            assert_eq!(error.product_id, None);
        }

        handle.remove();
    }

    /// Guard-order step 0, wired into this backend: a store-touching call made
    /// from inside a listener is refused before the bridge is even resolved —
    /// including `request_purchase`, which runs no main-thread guard but still
    /// parks the one delivery thread on its ack.
    #[test]
    fn a_store_call_from_the_delivery_thread_is_refused() {
        let _guard = crate::event::test_guard();
        let (tx, rx) = std::sync::mpsc::channel();

        let handle = crate::Iap::set_purchase_listener(Box::new(move |_| {
            tx.send((
                AppleIap.get_storefront().err(),
                AppleIap.init_connection(None).err(),
                AppleIap
                    .request_purchase(&crate::mock::purchase_props("premium_upgrade"))
                    .err(),
            ))
            .unwrap();
        }));

        deliver(
            Some(EVENT_PURCHASE_ERROR),
            Some(
                &json!({
                    "event": "purchaseError",
                    "data": { "code": "user-cancelled", "message": "cancelled" },
                })
                .to_string(),
            ),
        );
        crate::event::flush();

        let (storefront, init, purchase) = rx.try_recv().expect("the listener ran");
        for error in [storefront, init, purchase] {
            assert!(
                matches!(error, Some(IapError::EventThread)),
                "refused before any bridge lookup: {error:?}"
            );
        }

        handle.remove();
    }
}
