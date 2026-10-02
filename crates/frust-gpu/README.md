# frust-gpu

`frust-gpu` is the layer directly above wgpu in Frust. It probes what an adapter can do (`TierCaps`) and builds the instance, device, surfaces and pooled GPU resources that the render engine consumes. It owns nothing scene-specific: no display list and no strip pipeline. `frust-render` re-exports `RenderContext` from it to the platform shells.

Applications do not normally depend on this crate directly; they reach it through the `frust` facade crate.

- Website: https://frust.dev
- Repository: https://github.com/frust-rs/frust

## License

Licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`) or MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
