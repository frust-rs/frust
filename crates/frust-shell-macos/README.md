# frust-shell-macos

This crate provides `MacosExtensions`, the macOS implementation of the
`DesktopExtensions` seam in `frust-shell-desktop`. It builds a native menu bar
from the app's `DesktopConfig` (the standard application menu plus the app's own
menu), implements quit and reopen semantics for apps that keep running with no
window open, and confines the AppKit calls that winit and muda do not expose to
one module. The native bindings are declared only for macOS targets; on other
targets the crate compiles but does nothing.

Applications normally reach this crate through the `frust` facade rather than
depending on it directly.

See <https://frust.dev> for the framework documentation and
<https://github.com/frust-rs/frust> for the source repository.

## License

Licensed under either of the MIT license or the Apache License, Version 2.0
(SPDX expression `MIT OR Apache-2.0`), at your option. The full texts are in
`LICENSE-MIT` and `LICENSE-APACHE` beside this file.
