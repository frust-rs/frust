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
/// Everything below is the bridge's proving surface: a synchronous
/// round-trip that also proves OpenIAP itself is linked (not merely
/// resolved), and an asynchronous one that proves a completion block handed
/// in from Rust survives delivery off the caller's thread. The purchase API
/// is additive on top of them.
@objc(FrustIapBridge)
public final class FrustIapBridge: NSObject {
    /// The singleton Rust messages. StoreKit state (transaction listeners,
    /// the update stream) is process-global, so a per-call instance would own
    /// nothing; the ObjC-exposed class property is what makes it reachable
    /// without a Rust-side allocation.
    @objc public static let shared = FrustIapBridge()

    override public init() {
        super.init()
    }

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
            let thread = Thread.isMainThread ? "main" : "background"
            let body = "{\"echo\":\(jsonQuoted(payloadJson as String)),"
                + "\"thread\":\"\(thread)\"}"
            completion(body as NSString, nil)
        }
    }

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
