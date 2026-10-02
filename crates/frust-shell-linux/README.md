# frust-shell-linux

This crate provides `LinuxExtensions`, the Linux implementation of the
`DesktopExtensions` seam in `frust-shell-desktop`. Linux has no native menu bar
to build, because the menu is drawn by the framework's own widgets, so the
crate is small: it sets the Wayland `app_id` and X11 `WM_CLASS` and the window
icon through winit's cross-platform API. On other targets the crate compiles but
does nothing.

Applications normally reach this crate through the `frust` facade rather than
depending on it directly.

See <https://frust.dev> for the framework documentation and
<https://github.com/frust-rs/frust> for the source repository.

## License

Licensed under either of the MIT license or the Apache License, Version 2.0
(SPDX expression `MIT OR Apache-2.0`), at your option. The full texts are in
`LICENSE-MIT` and `LICENSE-APACHE` beside this file.
