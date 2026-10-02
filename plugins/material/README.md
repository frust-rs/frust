# frust-material

The Material 3 (Expressive) design-system catalog for [Frust](https://frust.dev):
a token system with runtime HCT seed-color generation, a feature-point
`RoundedPolygon`/`Morph` shape engine, generated Material Icons vector
constants, bundled Roboto Flex and Roboto Mono fonts, and the widget catalog
(buttons, selection controls, text field, menus, dialogs, sheets, navigation
bars, pickers, progress indicators and more) with its own overlay hosting seam.
The bundled fonts are behind the default-on `bundled-fonts` feature.

It is built on the `frust` facade's public `authoring` surface only. Applications
depend on `frust-material` directly, next to `frust`; the facade does not
re-export it.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

The crate's SPDX expression is `Apache-2.0 AND OFL-1.1`. It bundles
Apache-2.0-only ported code (material-color-utilities, androidx graphics-shapes
lineage, Material Icons path data) and OFL-1.1 fonts, so no MIT-only arm is
offered. The licence file beside the manifest is [LICENSE-APACHE](LICENSE-APACHE).

Third-party material shipped in this package:

- [NOTICE](NOTICE): attributions for the vendored and ported components
  (MIT, BSD-3-Clause and Apache-2.0 upstreams) and the fonts.
- [licenses/Apache-2.0.txt](licenses/Apache-2.0.txt): the Apache-2.0 text for the ported code.
- Roboto Flex, OFL-1.1, Copyright 2017 The Roboto Flex Project Authors:
  [fonts/roboto-flex/OFL.txt](fonts/roboto-flex/OFL.txt).
- Roboto Mono, OFL-1.1, Copyright 2015 The Roboto Mono Project Authors:
  [fonts/roboto-mono/OFL.txt](fonts/roboto-mono/OFL.txt).
- [FONTS-LICENSE](FONTS-LICENSE): font provenance, versions and the Roboto Flex modification record.
