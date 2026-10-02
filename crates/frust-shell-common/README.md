# frust-shell-common

This crate holds the machinery every Frust shell needs and none of them should
duplicate: the type-erased `AppTree` that lets a native handle drive any app's
state and logic, helpers for crossing an FFI boundary safely, the conversion of
platform density and insets into logical window metrics, the theme override slot,
frame-timing instrumentation, the skip-frame decision, pointer resampling and
the UI-to-render-thread handoff. Optional features are `perf-trace`, `devtools`
and `gpu`.

Applications do not depend on this crate directly; they reach it through the
`frust` facade, and the platform shells (`frust-shell-desktop`,
`frust-shell-android`, `frust-shell-ios`, `frust-shell-web`) build on it.

See <https://frust.dev> for the framework documentation and
<https://github.com/frust-rs/frust> for the source repository.

## License

Licensed under either of the MIT license or the Apache License, Version 2.0
(SPDX expression `MIT OR Apache-2.0`), at your option. The full texts are in
`LICENSE-MIT` and `LICENSE-APACHE` beside this file.
