# frust-shell-web

This crate is the browser host tier, alongside the desktop, Android and iOS
shells. It runs a Frust app on a canvas through winit and
`requestAnimationFrame`, with a single-threaded frame executor over the wgpu
surface and an input-method overlay built on a hidden `<input>` element bound to
the browser's composition events. It does not depend on `frust-shell-desktop`;
it builds directly on `frust-shell-common`. The browser-owning parts compile
only for `wasm32`, and the translation layer is also unit-tested on the build
host.

Applications normally reach this crate through the `frust` facade rather than
depending on it directly.

See <https://frust.dev> for the framework documentation and
<https://github.com/frust-rs/frust> for the source repository.

## License

Licensed under either of the MIT license or the Apache License, Version 2.0
(SPDX expression `MIT OR Apache-2.0`), at your option. The full texts are in
`LICENSE-MIT` and `LICENSE-APACHE` beside this file.
