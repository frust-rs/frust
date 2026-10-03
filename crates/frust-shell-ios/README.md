# frust-shell-ios

The iOS platform shell for Frust: the Rust half of the C-ABI bridge. It is the iOS counterpart to `frust-shell-desktop` and `frust-shell-android`. The generated Swift app drives it by calling a fixed set of `frust_*` C functions, and each app supplies its own state and logic through the `ios_app!` macro, which generates those exports bound to the app's types.

On iOS the crate composes the retained tree, scene, render and text crates. On every other target only the macro definition and the pure host-testable helpers compile, so the crate is inert in a host build.

Applications do not depend on this crate directly. The Swift embedding under `platform/ios/` in this crate hosts it, and `frust create` wires that up.

Optional features: `perf-trace`, `devtools` and `gpu`, each forwarding to the matching feature of the crates below it. docs.rs builds this crate for `aarch64-apple-ios`.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## Native embedding

The Swift embedding package lives in `platform/ios/FrustEmbedding/` inside this crate and ships with it, so a published `frust-shell-ios` carries the host package it pairs with.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option (SPDX: `MIT OR Apache-2.0`).
