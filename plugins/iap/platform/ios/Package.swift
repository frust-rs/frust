// swift-tools-version: 5.9
import PackageDescription

// `frust-iap`'s iOS Swift package — the ObjC-visible bridge Rust reaches
// through the ObjC runtime, over OpenIAP's Apple SDK.
//
// Two deliberate differences from `plugins/camera/platform/ios`, the existing
// plugin-package precedent:
//
// 1. NO C target. The camera package carries a header-only `CFrustCamera`
//    target so Swift can call a Rust `extern "C"` export; this package's
//    traffic runs the other way (Rust -> Swift, `objc2` message sends against
//    the `@objc(FrustIapBridge)` runtime name), so there is no C shim and no
//    symbol resolved at the consuming app's final link.
// 2. An EXTERNAL package dependency. `FrustCamera` depends only on the
//    sibling LOCAL `FrustEmbedding` package; this one resolves
//    `github.com/hyodotdev/openiap` over the network, so a consuming app's
//    first SwiftPM resolution needs GitHub reachable. Pinned `exact:`: the
//    Apple SDK version and the store contract it encodes move together, so a
//    range would let a store-behavior change in without a code change here.
//
// `platforms:` mirrors `FrustEmbedding`'s `.iOS(.v15)` — the project-wide iOS
// floor (`docs/DEVELOPMENT.md`'s Platform-Support Policy). OpenIAP's own
// manifest declares the same floor, so nothing here rises above a consuming
// app's `IPHONEOS_DEPLOYMENT_TARGET`, which would be a compile error rather
// than a warning.
let package = Package(
    name: "FrustIap",
    platforms: [.iOS(.v15)],
    products: [
        .library(name: "FrustIap", targets: ["FrustIap"])
    ],
    dependencies: [
        .package(url: "https://github.com/hyodotdev/openiap.git", exact: "3.0.1")
    ],
    targets: [
        .target(
            name: "FrustIap",
            dependencies: [
                // Package identity for a URL dependency is the lowercased
                // last path component (`openiap`), not the manifest's
                // declared `name` (`OpenIAP`) — the product name is the one
                // that keeps its case.
                .product(name: "OpenIAP", package: "openiap")
            ]
        )
    ]
)
