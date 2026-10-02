// swift-tools-version: 5.9
import PackageDescription

// `frust-iap`'s iOS Swift package — the ObjC-visible bridge Rust reaches
// through the ObjC runtime, over OpenIAP's Apple SDK.
//
// The package lives in its own `FrustIap/` directory (like
// `platform/ios/FrustEmbedding`), not directly in `platform/ios`: SwiftPM
// names a local package by its directory's last path component, so a second
// plugin package at `…/platform/ios` would share camera's identity `ios`
// and be silently dropped from any app that references both.
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
//    first SwiftPM resolution needs GitHub reachable. Pinned by commit
//    `revision:`, not the mutable `3.0.1` tag it currently names — a
//    payments-path dependency is pinned to an immutable commit
//    (`docs/DEVELOPMENT.md`'s Version-Pin Policy): the tag alone can be
//    force-moved upstream, the revision cannot. Bump
//    the revision and this comment's tag name together, in LOCKSTEP with
//    Android's `openiap-google` pin (same Version-Pin Policy row).
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
        // OpenIAP 3.0.1's tag commit — `git rev-parse '3.0.1^{commit}'`
        // against the upstream repo — verified to resolve cleanly as a
        // local package's external dependency added directly to an app
        // target (SwiftPM's root-only restriction on unversioned
        // requirements does not bite here: `FrustIap` is that root).
        .package(url: "https://github.com/hyodotdev/openiap.git", revision: "43ecc85bfda0fcd8f56d381e43d9d661afbd4729")
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
