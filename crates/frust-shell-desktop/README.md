# frust-shell-desktop

This crate wraps winit's event loop around the Frust render stack so that an
application opens in a native window. It is the cross-platform half of the
desktop shells: `run_desktop` starts an app with default settings,
`run_desktop_with` takes a `DesktopConfig` for the app's identity and a
`DesktopExtensions` implementation for per-OS behaviour. The per-OS crates
`frust-shell-macos`, `frust-shell-windows` and `frust-shell-linux` implement
that extension seam. Optional features are `perf-trace`, `devtools` and `gpu`.

Applications normally reach this crate through the `frust` facade rather than
depending on it directly.

See <https://frust.dev> for the framework documentation and
<https://github.com/frust-rs/frust> for the source repository.

## License

Licensed under either of the MIT license or the Apache License, Version 2.0
(SPDX expression `MIT OR Apache-2.0`), at your option. The full texts are in
`LICENSE-MIT` and `LICENSE-APACHE` beside this file.
