// swift-tools-version: 5.9
import PackageDescription

// Frust's iOS embedding package — the Swift half of the platform integration a
// generated app consumes, and the iOS mirror of Android's `frust-embedding`
// Gradle module. A scaffolded app references this package locally
// (`XCLocalSwiftPackageReference`, relative path) and owns only a four-line
// `AppDelegate` plus a three-line `SceneDelegate`.
//
// The native half does NOT live here: the Rust staticlib is still produced by
// the consuming app target's "Build Rust staticlib" run-script phase, and the
// `frust_*` C symbols `CFrustFFI` declares stay UNDEFINED in this package's
// object files — they resolve at the app's final link. That split mirrors
// Flutter's (`flutter_embedding` is Java/Kotlin-only; the engine ships
// separately) and is what the s1 spike proved on both the device
// (`arm64-apple-ios15.0`) and simulator triples, Debug and Release.
//
// `platforms:` must stay **at or below** every consumer's
// `IPHONEOS_DEPLOYMENT_TARGET` — a package minimum ABOVE the app's is a compile
// error, not a warning. `.iOS(.v15)` is exactly the generated app's 15.0; do
// not raise it without raising the template's deployment target first.
let package = Package(
    name: "FrustEmbedding",
    platforms: [.iOS(.v15)],
    products: [
        .library(name: "FrustEmbedding", targets: ["FrustEmbedding"])
    ],
    targets: [
        .target(name: "CFrustFFI"),
        .target(name: "FrustEmbedding", dependencies: ["CFrustFFI"]),
    ]
)
