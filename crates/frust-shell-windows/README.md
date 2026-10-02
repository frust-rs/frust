# frust-shell-windows

This crate provides `WindowsExtensions`, the Windows implementation of the
`DesktopExtensions` seam in `frust-shell-desktop`. From the app's
`DesktopConfig` it sets the process AppUserModelID, attaches the window icon, builds a
native Win32 menu bar with accelerators, and follows the resolved light or dark
theme in the titlebar. The native bindings are declared only for Windows
targets; on other targets the crate compiles but does nothing.

Applications normally reach this crate through the `frust` facade rather than
depending on it directly.

See <https://frust.dev> for the framework documentation and
<https://github.com/frust-rs/frust> for the source repository.

## License

Licensed under either of the MIT license or the Apache License, Version 2.0
(SPDX expression `MIT OR Apache-2.0`), at your option. The full texts are in
`LICENSE-MIT` and `LICENSE-APACHE` beside this file.
