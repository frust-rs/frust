# frust-beui

The beUI design-system catalog for [Frust](https://frust.dev). It is a port of
beUI v2 (`github.com/starc007/ui-components`) onto Frust's `View`/`Widget`
tree: the vendored token tables folded onto the baseline `ColorScheme`, a
`BeuiTokens` extension, three glass recipes, catalog motion tokens, an overlay
hosting seam, and the component catalog, together with the bundled Geist and
Geist Mono fonts. An optional `gpu-effects` feature adds a GPU perspective
substrate for a few 3D component variants.

It is built on the `frust` facade's public `authoring` surface only. Applications
depend on `frust-beui` directly, next to `frust`; the facade does not re-export it.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

The crate's SPDX expression is `(MIT OR Apache-2.0) AND OFL-1.1`: the Frust
sources are dual-licensed under [LICENSE-MIT](LICENSE-MIT) or
[LICENSE-APACHE](LICENSE-APACHE), and the bundled fonts are under the SIL Open
Font License 1.1.

Third-party material shipped in this package:

- beUI v2, MIT, Copyright (c) 2026 Saurabh Chauhan: [LICENSE-beui.md](LICENSE-beui.md).
- Geist and Geist Mono, OFL-1.1, Copyright 2024 The Geist Project Authors
  (<https://github.com/vercel/geist-font>): [fonts/geist/OFL.txt](fonts/geist/OFL.txt),
  [fonts/geist-mono/OFL.txt](fonts/geist-mono/OFL.txt).
