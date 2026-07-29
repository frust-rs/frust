// swift-tools-version: 5.9
import PackageDescription

// `frust-camera`'s iOS Swift package — the FIRST plugin Swift package
// (`docs/ARCHITECTURE.md`'s Module Structure). Mirrors
// `platform/ios/FrustEmbedding/Package.swift`'s shape exactly: one
// header-only C target declaring an `extern "C"` surface resolved at the
// consuming app's final link, plus a Swift target consuming it.
//
// TWO differences from that one-package precedent, both new ground this
// plugin proves:
//
// 1. This is a SECOND local package beside `FrustEmbedding` in the same
//    consuming app — its own C target (`CFrustCamera`) resolving symbols
//    from the SAME app-built Rust staticlib `CFrustFFI` already resolves
//    against. Both C targets are header-only (no `frust_*`/`frust_camera_*`
//    definitions anywhere in either package), so there is no duplicate-
//    symbol risk at the app's final link — only the Rust staticlib defines
//    the exports either C target declares.
// 2. `FrustCamera` depends on `FrustEmbedding`'s `FrustPlatformViewFactory`
//    protocol (`CameraPreviewFactory.swift` conforms to it), so this
//    manifest declares a LOCAL package dependency on the sibling
//    `FrustEmbedding` package by relative path — the first inter-plugin/
//    embedding Swift package dependency in this repo. A generated app's
//    `Contribution::SwiftPackageRef` adds this package as an
//    `XCLocalSwiftPackageReference` beside `FrustEmbedding`, resolving both
//    from the same project — SwiftPM/Xcode dedupes the shared
//    `FrustEmbedding` reference rather than vendoring it twice.
//
// `platforms:` mirrors `FrustEmbedding`'s `.iOS(.v15)` — a package minimum
// ABOVE the consuming app's `IPHONEOS_DEPLOYMENT_TARGET` is a compile error,
// not a warning, so this must never rise above `FrustEmbedding`'s own pin.
let package = Package(
    name: "FrustCamera",
    platforms: [.iOS(.v15)],
    products: [
        .library(name: "FrustCamera", targets: ["FrustCamera"])
    ],
    dependencies: [
        // Relative to this manifest: plugins/camera/platform/ios/ ->
        // platform/ios/FrustEmbedding (repo-root-relative
        // platform/ios/FrustEmbedding, four levels up from this file).
        .package(path: "../../../../platform/ios/FrustEmbedding")
    ],
    targets: [
        .target(name: "CFrustCamera"),
        .target(
            name: "FrustCamera",
            dependencies: [
                "CFrustCamera",
                .product(name: "FrustEmbedding", package: "FrustEmbedding"),
            ]
        ),
    ]
)
