# frust-engine

`frust-engine` is the sparse-strip render pipeline Frust draws with. `EngineRenderer` compiles a `frust_scene::Scene` into sparse strips, packs them into the layouts its WGSL shaders read, and records the frame's passes into a target through a command encoder that the caller owns and submits.

Applications do not normally depend on this crate directly; they reach it through the `frust` facade crate.

- Website: https://frust.dev
- Repository: https://github.com/frust-rs/frust

## License

Licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`) or MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).

This crate vendors material derived from the Vello project. The WGSL shaders in `shaders/` carry `Copyright the Vello Authors` and `SPDX-License-Identifier: Apache-2.0 OR MIT` headers, and are derived from `vello_sparse_shaders` 0.2.0. See the headers in those files.
