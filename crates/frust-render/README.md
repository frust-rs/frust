# frust-render

`frust-render` is the GPU backend layer of Frust. It takes the renderer-agnostic `frust_scene::Scene` display list and renders it into a window's swapchain, driving the `frust-engine` strip pipeline over the `frust-gpu` device and surface foundation. Platform shells use it through `RenderContext` and `SurfaceRenderer`.

Applications do not normally depend on this crate directly; they reach it through the `frust` facade crate.

- Website: https://frust.dev
- Repository: https://github.com/frust-rs/frust

## License

Licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`) or MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
