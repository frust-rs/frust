# frust-shell-android

The Android platform shell for Frust: the Rust half of the JNI bridge. It is the Android counterpart to `frust-shell-desktop`. Instead of owning an event loop, it is driven by the Kotlin `FrustSurfaceView`, which calls a fixed set of `Java_dev_frust_FrustSurfaceView_native*` symbols. Each app supplies its own state and logic through the `android_app!` macro, which generates those symbols bound to the app's types.

On Android the crate composes the retained tree, scene, render and text crates with the `jni` and `ndk` FFI crates. On every other target only the macro definition and a small host-testable helper module compile, so the crate is inert in a host build.

Applications do not depend on this crate directly. The Kotlin embedding under `platform/android/` in the repository hosts it, and `frust create` wires that up.

Optional features: `perf-trace`, `devtools` and `gpu`, each forwarding to the matching feature of the crates below it. docs.rs builds this crate for `aarch64-linux-android`.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option (SPDX: `MIT OR Apache-2.0`).
