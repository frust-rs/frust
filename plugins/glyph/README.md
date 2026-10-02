# frust-glyph

The Glyph design-system catalog for [Frust](https://frust.dev): a terminal-native,
dark-first, monospace-led design language. The crate holds the Glyph tokens
(color schemes, type, shape, elevation, motion and glass scales), the assembled
theme, two motion patterns, and the widget catalog (badges, alerts, loaders,
toast, navigation chrome, cards, terminal block, command palette, and its own
`toggle` switch). The bundled monospace fonts are behind the default-on
`bundled-fonts` feature.

It is built on the `frust` facade's public `authoring` surface only. Applications
depend on `frust-glyph` directly, next to `frust`; the facade does not re-export it.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

The crate's SPDX expression is `(MIT OR Apache-2.0) AND OFL-1.1`: the Frust
sources are dual-licensed under [LICENSE-MIT](LICENSE-MIT) or
[LICENSE-APACHE](LICENSE-APACHE), and the bundled fonts are under the SIL Open
Font License 1.1.

Third-party material shipped in this package:

- Space Mono, OFL-1.1, Copyright 2016 The Space Mono Project Authors
  (<https://github.com/googlefonts/spacemono>): [fonts/space-mono/OFL.txt](fonts/space-mono/OFL.txt).
- IBM Plex Mono, OFL-1.1, Copyright (c) 2017 IBM Corp. with Reserved Font Name
  "Plex" (<https://github.com/IBM/plex>): [fonts/ibm-plex-mono/OFL.txt](fonts/ibm-plex-mono/OFL.txt).
