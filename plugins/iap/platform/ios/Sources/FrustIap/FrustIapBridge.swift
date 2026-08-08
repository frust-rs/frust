import Foundation
import OpenIAP

/// The ObjC-runtime surface `frust-iap`'s Rust side reaches through `objc2`.
///
/// The bare `@objc` runtime name (`FrustIapBridge`, no package prefix) is the
/// same LAW `plugins/camera`'s `@objc(CameraPreviewFactory)` follows
/// (`docs/CODE_STANDARDS.md`'s Naming Conventions): the Rust side hard-codes
/// this exact string and resolves the class by name at runtime, so renaming
/// it is a breaking FFI change rather than a refactor. Nothing in a consuming
/// app references this class symbolically, which is also why it must survive
/// the app's final link on its own — a dead-stripped class is a `nil` class
/// lookup on the Rust side, not a link error.
///
/// # The wire: one generic entry point, JSON strings both ways
///
/// [`call(_:argsJson:completion:)`](x-source-tag://call) is the whole
/// Rust->Swift surface — one selector rather than a method per operation, so
/// adding a store call is a new `case` here and a new caller there, never a
/// new selector both sides must agree on. It speaks OpenIAP's own JSON
/// vocabulary (fields camelCase, enum values kebab-case) because
/// `OpenIapSerialization.encode` already emits exactly that; nothing here
/// re-maps a field name.
///
/// - `argsJson` is **always a JSON object** — `{}` for a call that takes no
///   arguments. (A bare `null` would need `JSONSerialization`'s
///   `.fragmentsAllowed`; requiring an object removes that edge entirely.)
/// - A success completes `(resultJson, nil)`, where `resultJson` is
///   `{"value": <result>}` — again an object, for the same reason, with
///   `null` as the value for the calls that answer nothing.
/// - A failure completes `(nil, errorJson)` in one of exactly two shapes:
///   - the spec's `{"code": <kebab>, "message": …, "productId": …?}` when the
///     store/OpenIAP reported a `PurchaseError` — the Rust side maps a payload
///     carrying `code` onto `IapError::Store`;
///   - a bare `{"message": …}` with **no** `code` for a glue-level failure
///     (unknown method, undecodable arguments, a result this glue could not
///     encode) — the Rust side maps that onto `IapError::Platform`.
///
/// **Which shape a failure takes is decided by the throw's type**
/// (`PurchaseError` vs this file's own `BridgeError`, discriminated in
/// `errorJson(from:method:)`), never by re-labelling a local failure with a
/// spec code. A store refusal an app can act on and a bug in this glue are
/// different things, and an app matching `IapError::Store` must never be
/// handed the second as the first.
///
/// The completion is the correlation: it fires exactly once per call, so
/// neither side carries a request-id map. It fires on a cooperative-pool
/// thread, not the caller's.
///
/// | `method` | `argsJson` | `value` |
/// |---|---|---|
/// | `initConnection` | `{"config": …?}` (ignored — see below) | `true` |
/// | `endConnection` | `{}` | `true` |
/// | `fetchProducts` | an OpenIAP `ProductRequest` (`{"skus": […], "type": …}`) | `{"type": "products"\|"subscriptions"\|"all", "items": […]}` |
/// | `getAvailablePurchases` | `{"options": …?}` | `[Purchase]` |
/// | `getActiveSubscriptions` | `{"subscriptionIds": […]?}` | `[ActiveSubscription]` |
/// | `getStorefront` | `{}` | `"USA"` |
/// | `requestPurchase` | an OpenIAP `RequestPurchaseProps` | `null` (an ack — see below) |
/// | `finishTransaction` | `{"purchase": {…}, "isConsumable": …?}` | `null` |
/// | `restorePurchases` | `{}` | `null` |
/// | `deepLinkToSubscriptions` | `{"options": …?}` | `null` (an ack — see below) |
/// | `getPendingTransactionsIOS` | `{}` | `[PurchaseIOS]` |
///
/// `initConnection`'s `config` has no StoreKit counterpart in OpenIAP 3.0.1
/// (`initConnection()` takes no parameters; the config the cross-platform API
/// models is Google Play alternative-billing setup) — it is accepted and
/// ignored rather than refused, so the Rust API can stay one shape on both
/// platforms. **Idempotency is mapped on the Rust side**, which short-circuits
/// a second `initConnection` against its own connection flag exactly as
/// `OpenIapModule.initConnection` short-circuits against its own.
///
/// # Two calls acknowledge instead of completing
///
/// `requestPurchase` and `deepLinkToSubscriptions` complete their Rust
/// completion as soon as the request is **dispatched**, then let the real
/// `await` run on in a detached `Task`. Both of the awaited calls resolve only
/// when the *user* is done with a StoreKit sheet, so awaiting them here would
/// park the calling Rust thread for the length of a human interaction and time
/// it out; `requestPurchase` in particular is documented non-blocking and
/// UI-thread-callable on the Rust side, and its outcome is the event stream's
/// job (see the crate doc's two-phase purchase contract). A purchase that then
/// fails reaches the app **exactly once**: `OpenIapModule.requestPurchase`
/// publishes the canonical failure on the purchase-error listener before it
/// rethrows, so the detached `catch` here logs a `PurchaseError` and emits
/// nothing. It synthesizes an event only for a throw that is *not* a
/// `PurchaseError` — a shape upstream never produces today, and the one that
/// would otherwise be lost with no report at all. A `deepLinkToSubscriptions`
/// that fails after dispatch has no event of its own and is logged instead —
/// the one accepted blind spot in this file.
///
/// # A Rust-side timeout does not stop the `Task`
///
/// The Rust side's own wait on `call:argsJson:completion:` is bounded (a
/// short ack for `requestPurchase`/`deepLinkToSubscriptions`, a longer bound
/// for every other operation — `plugins/iap/src/apple.rs`'s
/// `ACK_TIMEOUT`/`STORE_TIMEOUT`/`PROMPT_TIMEOUT`), and there is no
/// requestId table on that side to cancel in the first place — the
/// completion block *is* the correlation. So a Rust-side timeout only stops
/// that side from waiting on its rendezvous channel; it never reaches back
/// into this class to cancel the `Task` `call` spawned. That `Task` keeps
/// running to whatever `OpenIapModule` member it awaits — including a
/// purchase sheet already presented to the user, or a `finishTransaction`
/// call already sent to StoreKit — and still invokes `completion` when it
/// finishes; the answer simply lands on a channel the Rust side has already
/// stopped listening on and is silently discarded there.
///
/// This is deliberate: a requestId-keyed `Task` cancellation table here
/// would let the Rust side *ask* this `Task` to stop, but there would be
/// nothing for it to dismiss once StoreKit's own sheet or settlement call is
/// in flight — StoreKit does not offer a way to take either back. So a
/// Rust-side timeout leaves a store-touching operation's own outcome
/// **at-least-once** from this glue's point of view: it still runs to
/// completion and still tries to answer, even when nothing is listening
/// anymore.
///
/// # Events
///
/// [`setEventSink(_:)`](x-source-tag://setEventSink) registers OpenIAP's
/// purchase listeners **once** and forwards each event as
/// `(name, envelopeJson)`: the name is the spec's own event string
/// (`"purchase-updated"`/`"purchase-error"`, `OpenIAP.IapEvent`'s `rawValue`),
/// and the envelope is `frust-iap`'s adjacently-tagged `IapEvent` shape
/// (`{"event": "purchaseUpdated"|"purchaseError", "data": {…}}` — the Rust
/// crate's `types.rs` designs it; the tag there is camelCase because that is
/// its variant spelling, not the spec's kebab event name).
///
/// **Replay/dedup:** the purchase-updated listener is registered with
/// `dedupeTransactionIOS: true` — pinned explicitly rather than left to
/// upstream's default — so one transaction id is delivered at most once *per
/// connection session*. StoreKit still replays an **unfinished** transaction
/// on the next launch (a new session), so an app may legitimately see the same
/// purchase again after a restart; finishing it is what stops that, and there
/// is deliberately no cross-launch dedupe here (it would need persistence this
/// glue has no business owning).
///
/// # Debug probes
///
/// `ping` and `callMeBack` are on no production path. They stay because they
/// bisect a bring-up failure that `call` alone cannot: `ping` proves the class
/// resolved and OpenIAP is genuinely linked, `callMeBack` proves a Rust-owned
/// completion block survives delivery off the caller's thread — both without
/// touching StoreKit, so a failure in either is a bridge problem while a
/// failure only in `call` is a store problem.
@objc(FrustIapBridge)
public final class FrustIapBridge: NSObject {
    /// The singleton Rust messages. StoreKit state (transaction listeners,
    /// the update stream) is process-global, so a per-call instance would own
    /// nothing; the ObjC-exposed class property is what makes it reachable
    /// without a Rust-side allocation.
    @objc public static let shared = FrustIapBridge()

    /// Guards [`eventSink`] and the two listener tokens below. Held across the
    /// registration in `setEventSink` too, so two racing callers cannot both
    /// register a listener pair; OpenIAP's own registration is synchronous and
    /// never calls back into this class, so there is nothing to deadlock on.
    private let lock = NSLock()

    /// The Rust-owned block every event is forwarded to, or `nil` before the
    /// Rust side installs one.
    private var eventSink: ((NSString, NSString) -> Void)?

    /// OpenIAP's listener handles, kept so the registration can be recognized
    /// as already done (and released if this class ever grows a teardown —
    /// today the sink lives for the process, matching the Rust side).
    private var purchaseUpdatedToken: Subscription?
    private var purchaseErrorToken: Subscription?

    override public init() {
        super.init()
    }

    // MARK: - Rust -> Swift

    /// Run `method` against `OpenIapModule.shared` and answer `completion`
    /// with either a result or an error, both JSON strings.
    ///
    /// See the class doc for the argument/result/error shapes and the
    /// per-method table. Unknown methods answer the code-less error shape, so
    /// a Rust/Swift version skew reports as one diagnosable
    /// `IapError::Platform` rather than a silent no-op.
    ///
    /// - Tag: call
    @objc public func call(
        _ method: NSString,
        argsJson: NSString,
        completion: @escaping (NSString?, NSString?) -> Void
    ) {
        let name = method as String
        let rawArgs = argsJson as String
        Task {
            do {
                let args = try Self.decodeArguments(rawArgs, method: name)
                let value = try await Self.perform(name, args: args)
                guard let reply = jsonString(from: ["value": value]) else {
                    throw BridgeError(
                        "\(name): the result could not be serialized as JSON"
                    )
                }
                completion(reply as NSString, nil)
            } catch {
                completion(nil, Self.errorJson(from: error, method: name) as NSString)
            }
        }
    }

    /// Install `sink` as the destination for every purchase event, registering
    /// OpenIAP's listeners the first time it is called.
    ///
    /// Re-installing replaces the sink without re-registering: OpenIAP would
    /// otherwise deliver each event once per registered listener pair. The
    /// sink is invoked on whatever thread OpenIAP delivers on — today the
    /// **main** thread, since `OpenIapModule` invokes its listeners inside
    /// `await MainActor.run { … }`. Nothing app-facing runs there: the Rust
    /// sink treats the call as a hand-off and queues the event for the
    /// plugin's own delivery thread.
    ///
    /// - Tag: setEventSink
    @objc public func setEventSink(_ sink: @escaping (NSString, NSString) -> Void) {
        lock.lock()
        defer { lock.unlock() }
        eventSink = sink
        guard purchaseUpdatedToken == nil, purchaseErrorToken == nil else { return }

        purchaseUpdatedToken = OpenIapModule.shared.purchaseUpdatedListener(
            { [weak self] purchase in
                self?.forward(.purchaseUpdated, payload: OpenIapSerialization.purchase(purchase))
            },
            // Pinned rather than defaulted — see the class doc's replay note.
            options: PurchaseUpdatedListenerOptions(dedupeTransactionIOS: true)
        )
        purchaseErrorToken = OpenIapModule.shared.purchaseErrorListener { [weak self] error in
            self?.forward(.purchaseError, payload: OpenIapSerialization.encode(error))
        }
    }

    // MARK: - Debug probes

    /// Synchronous round-trip: echoes `payloadJson` back alongside proof that
    /// OpenIAP's own code is linked into the app and its resource bundle
    /// survived the build (see `openIapProof`).
    @objc public func ping(_ payloadJson: NSString) -> NSString {
        let body = "{\"echo\":\(jsonQuoted(payloadJson as String)),"
            + "\"openiap\":\(Self.openIapProof())}"
        return body as NSString
    }

    /// Asynchronous round-trip: hands `(result, error)` to `completion` from
    /// inside a `Task`, so the block fires *after* this method has returned
    /// and (in the general case) on a cooperative-pool thread rather than the
    /// caller's. That is the shape every real purchase call has — StoreKit 2
    /// is `async`/`await` throughout — so proving a Rust-owned block survives
    /// this hop is the whole point of the method.
    ///
    /// The reported thread is part of the result payload rather than an
    /// assertion: a cooperative-pool task is free to land on the main thread,
    /// so treating "background" as required would be a flaky check, whereas
    /// "the block ran at all, after return" is the actual contract.
    @objc public func callMeBack(
        _ payloadJson: NSString,
        completion: @escaping (NSString?, NSString?) -> Void
    ) {
        Task {
            // `pthread_main_np()` rather than `Thread.isMainThread`: the latter
            // is unavailable from an asynchronous context (a hard error under
            // the Swift 6 language mode). It is also the same primitive the
            // Rust side's own main-thread guard reads.
            let thread = pthread_main_np() != 0 ? "main" : "background"
            let body = "{\"echo\":\(jsonQuoted(payloadJson as String)),"
                + "\"thread\":\"\(thread)\"}"
            completion(body as NSString, nil)
        }
    }

    // MARK: - Dispatch

    /// The one place a `method` string turns into an `OpenIapModule` call.
    ///
    /// Returns the JSON value the class doc's table promises: a `Bool`, a
    /// `String`, an array/object of already-encoded OpenIAP payloads, or
    /// `NSNull` for the calls that answer nothing.
    private static func perform(_ method: String, args: [String: Any]) async throws -> Any {
        let module = OpenIapModule.shared
        switch method {
        case "initConnection":
            // `args["config"]` is deliberately unread — see the class doc.
            return try await module.initConnection()

        case "endConnection":
            return try await module.endConnection()

        case "fetchProducts":
            let request = try OpenIapSerialization.decode(object: args, as: ProductRequest.self)
            let result = try await module.fetchProducts(request)
            return try productsEnvelope(result)

        case "getAvailablePurchases":
            var options: PurchaseOptions?
            if let raw = value(args["options"]) {
                options = try OpenIapSerialization.purchaseOptions(from: raw)
            }
            let purchases = try await module.getAvailablePurchases(options)
            return try OpenIapSerialization.purchasesRequired(purchases)

        case "getActiveSubscriptions":
            let ids = args["subscriptionIds"] as? [String]
            let subscriptions = try await module.getActiveSubscriptions(ids)
            return try encodeAll(subscriptions, "ActiveSubscription")

        case "getStorefront":
            return try await module.getStorefront()

        case "requestPurchase":
            // Decoded synchronously so a malformed request is reported to the
            // caller; the purchase itself runs on past this ack (class doc).
            let props = try OpenIapSerialization.requestPurchaseProps(from: args)
            Task {
                do {
                    _ = try await OpenIapModule.shared.requestPurchase(props)
                } catch let error as PurchaseError {
                    // Already published on the purchase-error listener by
                    // `requestPurchase` itself — the single source of truth for
                    // a purchase outcome. Re-emitting would double the event,
                    // so this arm only records that the throw happened.
                    NSLog("[FrustIap] requestPurchase failed: \(error.code.rawValue) \(error.message)")
                } catch {
                    // Upstream canonicalizes every failure into a
                    // `PurchaseError` before rethrowing, so this arm is a
                    // backstop against a future upstream that does not: the
                    // event stream is the app's only report, so synthesize it
                    // here rather than let the outcome vanish.
                    NSLog("[FrustIap] requestPurchase threw an unpublished error: \(error)")
                    FrustIapBridge.shared.forward(
                        .purchaseError,
                        payload: OpenIapSerialization.encode(
                            PurchaseError.wrap(error, fallback: .purchaseError)
                        )
                    )
                }
            }
            return NSNull()

        case "finishTransaction":
            guard let raw = args["purchase"] as? [String: Any] else {
                throw BridgeError("finishTransaction: `purchase` must be a JSON object")
            }
            try await module.finishTransaction(
                purchase: purchaseInput(from: raw),
                isConsumable: args["isConsumable"] as? Bool
            )
            return NSNull()

        case "restorePurchases":
            try await module.restorePurchases()
            return NSNull()

        case "deepLinkToSubscriptions":
            var options: DeepLinkOptions?
            if let raw = value(args["options"]) {
                options = try OpenIapSerialization.decode(object: raw, as: DeepLinkOptions.self)
            }
            Task {
                do {
                    try await OpenIapModule.shared.deepLinkToSubscriptions(options)
                } catch {
                    // No event carries this one, so the console is the only
                    // report — see the class doc's accepted blind spot.
                    NSLog("[FrustIap] deepLinkToSubscriptions failed: \(error)")
                }
            }
            return NSNull()

        case "getPendingTransactionsIOS":
            let pending = try await module.getPendingTransactionsIOS()
            return try encodeAll(pending, "PurchaseIOS")

        default:
            throw BridgeError("unknown method `\(method)`")
        }
    }

    // MARK: - Marshaling

    /// `argsJson` as the JSON object every call promises to send.
    private static func decodeArguments(_ json: String, method: String) throws -> [String: Any] {
        guard
            let data = json.data(using: .utf8),
            let decoded = try? JSONSerialization.jsonObject(with: data),
            let args = decoded as? [String: Any]
        else {
            throw BridgeError("\(method): argsJson must be a JSON object")
        }
        return args
    }

    /// `raw` as a present JSON value — `nil` for both an absent key and an
    /// explicit `null`, which a Rust `Option::None` may serialize as.
    private static func value(_ raw: Any?) -> Any? {
        guard let raw, !(raw is NSNull) else { return nil }
        return raw
    }

    /// `frust-iap`'s own `FetchProductsResult` envelope — `{"type": …,
    /// "items": […]}`, tagged on which of the GraphQL type's three fields is
    /// populated. Designed by the Rust crate (`plugins/iap/src/types.rs`,
    /// `FetchProductsResult`'s doc) because `Types.swift`'s counterpart is a
    /// plain enum with no wire form of its own.
    private static func productsEnvelope(_ result: FetchProductsResult) throws -> [String: Any] {
        let tag: String
        switch result {
        case .products: tag = "products"
        case .subscriptions: tag = "subscriptions"
        case .all: tag = "all"
        }
        let items = OpenIapSerialization.products(result)
        guard !items.contains(where: { $0.isEmpty }) else {
            // A `BridgeError`, not a `PurchaseError`: the store answered
            // fine and *this* code could not encode the answer, so it takes
            // the code-less glue shape (`IapError::Platform`) rather than
            // borrowing a spec code the store never reported. See the class
            // doc's error shapes.
            throw BridgeError("fetchProducts: a fetched product could not be serialized")
        }
        return ["type": tag, "items": items]
    }

    /// Every element of `values` encoded, refusing the empty dictionary
    /// `OpenIapSerialization.encode` answers with on a serialization failure —
    /// an all-or-nothing result, never a silently half-populated one.
    ///
    /// The refusal is a `BridgeError` for the same reason `productsEnvelope`'s
    /// is: a local encode failure is this glue's, not the store's.
    private static func encodeAll<T: Encodable>(
        _ values: [T],
        _ what: String
    ) throws -> [[String: Any]] {
        try values.map { value in
            let encoded = OpenIapSerialization.encode(value)
            guard !encoded.isEmpty else {
                throw BridgeError("a native \(what) payload could not be serialized")
            }
            return encoded
        }
    }

    /// A `PurchaseInput` from the narrower object the Rust side sends.
    ///
    /// `PurchaseInput` is `Purchase` (a platform union) upstream, and its iOS
    /// arm additionally requires `transactionId` and `store` — neither of
    /// which is part of `type.graphql`'s `input PurchaseInput`, so a
    /// spec-shaped payload cannot decode as-is. Both are filled in from what
    /// the payload does carry rather than refused: on this platform the store
    /// is always Apple, and StoreKit's transaction id *is* the purchase id
    /// (`OpenIapModule.finishTransaction` looks the transaction up by `id`, so
    /// the filled value is never the one that settles anything).
    private static func purchaseInput(from raw: [String: Any]) throws -> PurchaseInput {
        var normalized = raw
        if normalized["transactionId"] == nil {
            normalized["transactionId"] = raw["id"]
        }
        if normalized["store"] == nil || normalized["store"] is NSNull {
            normalized["store"] = IapStore.apple.rawValue
        }
        let purchase = try OpenIapSerialization.decode(object: normalized, as: PurchaseIOS.self)
        return .purchaseIos(purchase)
    }

    /// The error JSON handed back as `call`'s second completion argument — the
    /// spec shape for a `PurchaseError`, the code-less shape for anything
    /// else (class doc).
    ///
    /// Never fails: a payload that will not serialize degrades to a
    /// hand-escaped object rather than leaving the caller with two nils.
    private static func errorJson(from error: Error, method: String) -> String {
        if let purchaseError = error as? PurchaseError {
            let encoded = OpenIapSerialization.encode(purchaseError)
            if !encoded.isEmpty, let json = jsonString(from: encoded) {
                return json
            }
            return "{\"code\":\(jsonQuoted(purchaseError.code.rawValue)),"
                + "\"message\":\(jsonQuoted(purchaseError.message))}"
        }
        let message = (error as? BridgeError)?.message ?? "\(method): \(error)"
        return "{\"message\":\(jsonQuoted(message))}"
    }

    // MARK: - Events

    /// Forward one OpenIAP event to the Rust sink, if one is installed.
    private func forward(_ event: IapEvent, payload: [String: Any]) {
        lock.lock()
        let sink = eventSink
        lock.unlock()
        guard let sink else { return }

        let tag = event == .purchaseUpdated ? "purchaseUpdated" : "purchaseError"
        guard let json = jsonString(from: ["event": tag, "data": payload]) else {
            NSLog("[FrustIap] dropped a \(event.rawValue) event: payload is not JSON-serializable")
            return
        }
        sink(event.rawValue as NSString, json as NSString)
    }

    // MARK: - Link proofs

    /// A JSON object naming what could be observed of the OpenIAP dependency
    /// from inside the linked app.
    ///
    /// `symbol` is the load-bearing half: naming the type forces OpenIAP's
    /// module into the app's link graph, so a non-null value here means the
    /// external dependency is genuinely linked and not merely resolved by
    /// SwiftPM. `bundle`/`apple` additionally report whether OpenIAP's
    /// `openiap-versions.json` resource bundle was copied into the app.
    private static func openIapProof() -> String {
        let symbol = jsonQuoted(String(describing: OpenIapVersion.self))
        guard let bundle = openIapResourceBundle() else {
            return "{\"symbol\":\(symbol),\"bundle\":null,\"apple\":null}"
        }
        // `OpenIapVersion.current` reads `openiap-versions.json` out of
        // OpenIAP's own `Bundle.module` and `fatalError`s when it is absent —
        // hence the guard: the call only runs once the bundle is known to be
        // present, so a resource-copy failure reports as a null field instead
        // of killing the process.
        let name = jsonQuoted(bundle.lastPathComponent)
        let apple = jsonQuoted(OpenIapVersion.current)
        return "{\"symbol\":\(symbol),\"bundle\":\(name),\"apple\":\(apple)}"
    }

    /// OpenIAP's SwiftPM resource bundle inside the running app, if it was
    /// copied there. Located by scanning the main bundle's resource directory
    /// rather than by its conventional `<Package>_<Target>.bundle` name, so a
    /// naming change upstream degrades to a null field rather than a false
    /// negative that then trips `openIapProof`'s guard for the wrong reason.
    private static func openIapResourceBundle() -> URL? {
        guard
            let resources = Bundle.main.resourceURL,
            let entries = try? FileManager.default.contentsOfDirectory(
                at: resources,
                includingPropertiesForKeys: nil
            )
        else { return nil }
        return entries.first {
            $0.pathExtension == "bundle" && $0.lastPathComponent.contains("OpenIAP")
        }
    }
}

/// A glue-level failure — an unknown method, an undecodable argument — as
/// opposed to a `PurchaseError` the store reported. Crosses the wire as the
/// code-less `{"message": …}` shape the Rust side maps onto
/// `IapError::Platform`.
private struct BridgeError: Error {
    let message: String

    init(_ message: String) {
        self.message = message
    }
}

/// `value` as a JSON string, or `nil` when it holds something JSON cannot
/// represent (the flutter plugin's own marshaling guard — the pattern, not the
/// code, which is a different library's).
private func jsonString(from value: Any) -> String? {
    guard JSONSerialization.isValidJSONObject(value) else { return nil }
    guard let data = try? JSONSerialization.data(withJSONObject: value) else { return nil }
    return String(data: data, encoding: .utf8)
}

/// Quote and escape `value` as a JSON string literal.
///
/// Hand-rolled rather than `JSONSerialization`, matching the framework's
/// hand-roll-JSON-at-the-FFI-boundary idiom (`docs/CODE_STANDARDS.md`'s
/// Language Idioms): the payloads crossing this bridge are a handful of fixed
/// fields, and the escaper is the whole of what a builder would provide.
private func jsonQuoted(_ value: String) -> String {
    var out = "\""
    for scalar in value.unicodeScalars {
        switch scalar {
        case "\"": out += "\\\""
        case "\\": out += "\\\\"
        case "\n": out += "\\n"
        case "\r": out += "\\r"
        case "\t": out += "\\t"
        default:
            if scalar.value < 0x20 {
                out += String(format: "\\u%04x", scalar.value)
            } else {
                out.unicodeScalars.append(scalar)
            }
        }
    }
    out += "\""
    return out
}
