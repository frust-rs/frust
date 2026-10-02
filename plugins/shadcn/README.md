# frust-shadcn

The shadcn/ui design-system catalog for [Frust](https://frust.dev). It is a port
of shadcn/ui v4 onto Frust's `View`/`Widget` tree: seven vendored base token
presets folded onto the baseline `ColorScheme`, a `ShadcnTokens` extension, an
overlay hosting seam for anchored and modal panels, and the component catalog,
with bundled Inter and JetBrains Mono fonts behind the default-on
`bundled-fonts` feature.

It is built on the `frust` facade's public `authoring` surface only. Applications
depend on `frust-shadcn` directly, next to `frust`; the facade does not re-export it.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

The crate's SPDX expression is `(MIT OR Apache-2.0) AND OFL-1.1`: the Frust
sources are dual-licensed under [LICENSE-MIT](LICENSE-MIT) or
[LICENSE-APACHE](LICENSE-APACHE), and the bundled fonts are under the SIL Open
Font License 1.1.

Third-party material shipped in this package:

- shadcn/ui v4, MIT, Copyright (c) 2023 shadcn: [LICENSE-shadcn-ui.md](LICENSE-shadcn-ui.md).
- Inter, OFL-1.1, Copyright (c) 2016 The Inter Project Authors
  (<https://github.com/rsms/inter>): [fonts/inter/OFL.txt](fonts/inter/OFL.txt).
- JetBrains Mono, OFL-1.1, Copyright 2020 The JetBrains Mono Project Authors
  (<https://github.com/JetBrains/JetBrainsMono>): [fonts/jetbrains-mono/OFL.txt](fonts/jetbrains-mono/OFL.txt).
