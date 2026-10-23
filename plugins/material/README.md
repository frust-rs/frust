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

## Localized strings

Every label, hint and accessibility string a widget emits is carried by one theme
extension, `MaterialStrings` (English by default, attached by `baseline`). A widget
resolves each string in a fixed order: the explicit per-widget setter, then the
ambient theme's `MaterialStrings`, then English (a custom theme with no extension keeps
working). The setters added for this are `close_label` (dialogs), `dismiss_label`
(snackbar), `remove_label` (chips), `collapse_label` / `expand_label` (navigation rail),
`NavigationDrawerView::label`, `back_label` / `clear_label` (search), `clear_label`
(selection) and `search_hint` (dropdown). The date and time pickers default their
`DatePickerStrings` / `TimePickerStrings` from the theme's table when no `.strings(..)`
is set. `DROPDOWN_HINT`, `DROPDOWN_EMPTY_TEXT` and `DROPDOWN_SEARCH_HINT` stay public but
are now English-only values that no live read uses.

This crate takes no dependency on another plugin, so it ships the mechanism and the
English table only. [`frust-material-i18n`](../material-i18n) supplies localized tables
that follow the active `frust-i18n` locale. The date picker's typed-entry mode is still
the fixed `mm/dd/yyyy` form.

## Snackbar queue

`SnackbarController::show` requests a single message (replacing a pending one).
`enqueue` appends to a FIFO queue whose messages show one at a time, each with its own
duration and action; `clear_queue` drops the waiting messages and `queue_len` counts
them. The `show` slot is served before the queue.

## License

The crate's SPDX expression is `Apache-2.0 AND OFL-1.1`. It bundles
Apache-2.0-only ported code (material-color-utilities, androidx graphics-shapes
lineage, Material Icons path data) and OFL-1.1 fonts, so no MIT-only arm is
offered. The licence file beside the manifest is [LICENSE-APACHE](LICENSE-APACHE);
the texts for the ported third-party components are vendored under
[licenses/](licenses/) (`Apache-2.0.txt`, `MIT.txt`, `BSD-3-Clause.txt`) and
the fonts' `OFL.txt` files, with every copyright holder listed in
[NOTICE](NOTICE).

Third-party material shipped in this package:

- [NOTICE](NOTICE): attributions for the vendored and ported components
  (MIT, BSD-3-Clause and Apache-2.0 upstreams) and the fonts.
- [licenses/Apache-2.0.txt](licenses/Apache-2.0.txt): the Apache-2.0 text for the ported code.
- Roboto Flex, OFL-1.1, Copyright 2017 The Roboto Flex Project Authors:
  [fonts/roboto-flex/OFL.txt](fonts/roboto-flex/OFL.txt).
- Roboto Mono, OFL-1.1, Copyright 2015 The Roboto Mono Project Authors:
  [fonts/roboto-mono/OFL.txt](fonts/roboto-mono/OFL.txt).
- [FONTS-LICENSE](FONTS-LICENSE): font provenance, versions and the Roboto Flex modification record.
