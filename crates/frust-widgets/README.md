# frust-widgets

`frust-widgets` is Frust's baseline widget set: a text leaf, the primitive layout containers (`Row`, `Column`, `Stack`, `Padding`, `Align`, `SizedBox`), scrolling, buttons and other controls, built as `View`/`Widget` pairs over `frust-core`. Its `authoring` module exposes the helpers that design systems and custom widgets build on. Design languages such as Material and Cupertino ship as separate plugin crates.

Applications do not normally depend on this crate directly; they reach it through the `frust` facade crate.

- Website: https://frust.dev
- Repository: https://github.com/frust-rs/frust

## License

Licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`) or MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).

The icon path data in `src/icons/mod.rs` is derived from Google's Material Symbols, which is licensed under the Apache License, Version 2.0. Its provenance is recorded in `src/icons/LICENSE-material-symbols`, which is included in the published package.
