# frust-material-i18n

Localized Material widget strings that follow the active
[`frust-i18n`](../i18n) locale.

`frust-material` ships every label and accessibility string as a
`MaterialStrings` theme extension (English by default) and, as a design-system
plugin, cannot depend on `frust-i18n`. This bridge crate depends on both.

```rust
fn build(&self, state: &mut State) -> impl View<State> {
    // Tracked read of the locale: each `set_locale` rebuilds this component
    // and provides the matching table to everything below it.
    frust_material_i18n::provide_localized_strings(&state.i18n);
    /* the app tree */
}
```

- `provide_localized_strings(&I18n)` re-provides the ambient `Theme` with the
  active language's table (the Material baseline when no theme is provided).
- `localized_theme(theme, &I18n)` does the same for a theme you hand to the
  shell yourself (`set_app_theme`), which the paint-time readers (badge, FAB
  menu, snackbar, chips) see.
- `material_strings_for(&Locale, &I18n)` returns the table directly. An
  unsupported language gives `MaterialStrings::ENGLISH`.

Month and weekday names are read from CLDR (through ICU4X), not hand-written.

## Native review status

| Language | Status |
|----------|--------|
| en | reviewed |
| th | machine-drafted, pending native review |
| vi | machine-drafted, pending native review |
| ja | machine-drafted, pending native review |
| es | machine-drafted, pending native review |
| de | machine-drafted, pending native review |
| fr | machine-drafted, pending native review |

Weekday initials are hand-written per language; CLDR has no narrow weekday
length in ICU4X's field sets.

## License

MIT OR Apache-2.0.
