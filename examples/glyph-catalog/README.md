# glyph-catalog

A standalone Frust example app showcasing the **entire Glyph design system** —
every token scale, all 21 `frust::glyph` widgets, the `Button` styles and
baseline form controls under the Glyph theme, and all of `frust::motion`'s
transition patterns — faithful to the three vendored reference builds (dark,
light, motion).

It doubles as a living demo of `PatternSwitcher`, the navigator's Glyph page
transitions, the brightness swap, and reduced-motion collapse.

## Running

```bash
# Desktop preview (the manual visual gate):
cargo run

# On a device (Android / iOS):
frust run -d <device-id>
```

This is a standalone package (its own `[workspace]` root and `Cargo.lock`,
excluded from the Frust root workspace — the same shape as
`examples/bubblebench`), so build/test/gate it from **this directory**, never
with `-p` from the repo root:

```bash
cargo build --locked && cargo test \
  && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

## Shape

- `src/lib.rs` — the shell: `CatalogApp` (the root `Component`) and its state,
  the root `navigator` whose home page is a header (title + brightness and
  reduce-motion toggles) + a 7-section glyph `tabs` strip + a `pattern_switcher`
  (GlyphSlide) hosting the current section in a `scroll_view`, all under a
  `toast_host` overlay.
- `src/pages/` — one module per section (foundations, buttons+forms, feedback,
  navigation, content, overlays, motion). Each exposes
  `page(&CatalogState) -> AnyView<CatalogState>` (the fixed page-fn contract
  documented in `src/pages/mod.rs`); the scaffold ships placeholders that the
  later `c02`–`c08` tasks fill in.
- `tests/smoke.rs` — a headless `RenderRoot` build/layout/paint smoke test
  proving the shell and every stub page mount.

## Theming

Every shell seeds `Theme::glyph_baseline()` by default (dark-first). The header
toggles rebuild the theme via a `ThemeBuilder` over the Glyph baseline
(`.brightness(...)` + `.map_motion(...)` for the reduced-motion flag) and force
it app-wide with `set_app_theme`.
