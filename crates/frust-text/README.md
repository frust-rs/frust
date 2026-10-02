# frust-text

`frust-text` is Frust's text pipeline. It wraps parley's font matching and layout in a small, renderer-agnostic surface: `TextContext` owns the font and layout state, `TextStyle` carries the styling parameters, and `TextLayout` is a measurable block of text that converts into `frust_scene::GlyphRun`s. Only kurbo, peniko and frust-scene types appear in its public API.

Applications do not normally depend on this crate directly; they reach it through the `frust` facade crate.

- Website: https://frust.dev
- Repository: https://github.com/frust-rs/frust

## License

Licensed under either of Apache License, Version 2.0 (`LICENSE-APACHE`) or MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
