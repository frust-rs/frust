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

## Host page

The host-page template (`index.html`, `frust_web.js` and a README) lives in `platform/web/` inside this crate and ships with it.

## License

The crate's SPDX expression is `(MIT OR Apache-2.0) AND OFL-1.1`. The Frust
sources are licensed under either of the MIT license or the Apache License,
Version 2.0, at your option; the full texts are in `LICENSE-MIT` and
`LICENSE-APACHE` beside this file.

The crate also bundles the Inter typeface as its default face
(`fonts/InterVariable.ttf`), which is under the SIL Open Font License 1.1,
Copyright (c) 2016 The Inter Project Authors (https://github.com/rsms/inter).
The licence text is in `fonts/OFL.txt`.
