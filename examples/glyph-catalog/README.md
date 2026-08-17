# glyph-catalog

A standalone Frust example app showcasing the **entire Glyph design system**,
and *only* the Glyph design system — every token scale, all 21 `frust_glyph`
widgets, the `Button` styles and baseline form controls under the Glyph
theme, and all of `frust::motion`'s transition patterns — faithful to the
three vendored reference builds (dark, light, motion).

It doubles as a living demo of `PatternSwitcher`, the navigator's Glyph page
transitions, the brightness swap, and reduced-motion collapse.

Plugin, native-widgets, platform-view-compositing, and responsive-layout
demos are **not** part of this catalog's charter — those live in
[`examples/playground`](../playground) instead.

## What it shows

A header (app title + a brightness toggle + a reduce-motion toggle) sits above
a 9-tab section strip, each tab hosting one catalog section in a
`pattern_switcher` (`GlyphSlide`) so switching tabs plays a themed slide
transition:

1. **Foundations** — live color/type/radius token specimens (every swatch
   re-resolves from the current theme, never a hardcoded hex).
2. **Buttons + Forms** — every `ButtonStyle`, `.small()`, a loading demo, plus
   the baseline form controls (`text_input`, `checkbox`, `radio`, `slider`)
   rendered under the Glyph theme, alongside the Glyph catalog's own
   `frust_glyph::toggle`.
3. **Feedback** — badges, dismissible tags, alerts, toast triggers, and the
   progress/skeleton/dots loaders.
4. **Navigation** — a standalone tabs demo, segmented control, breadcrumb, two
   `glyph_nav_bar` strips (character glyphs vs. vector icons), and avatars.
5. **Content** — cards, a stat-card grid, a list (doubling as the data-table
   stand-in), an accordion, an empty state, a staggered terminal block, and a
   tooltip.
6. **Overlays** — a confirmation dialog, a live-filtered command palette, and
   a `SlideUp` bottom sheet, each pushed through the shared navigator.
7. **Motion** — all 11 reference motion demos (press feedback, toggle spring,
   tab indicator, accordion height, modal, bottom sheet, command palette,
   toast, staggered log reveal, a composable screen-transition picker, and a
   boot sequence), plus a live `theme.motion` duration/easing token table and
   a reduced-motion status note.
8. **Interactions** — eight moments tied to a simulated muxr event stream: a
   connection heartbeat, a session-attach card morph, a new-session boot
   sequence, a token-revoke character scramble, a long-press charge ring, a
   pull-to-refresh rain burst, a live-output waveform, and a copy-to-clipboard
   burst.
9. **AppBar** — an inline anatomy diagram plus six pushed full-screen
   variations of the catalog's own root `frust_glyph::app_bar`: scroll collapse,
   back-nav title crossfade, an overflow menu, selection mode, and a
   connection banner.

A toast host overlays every section (Feedback's and Motion's toast triggers
both queue into the same shared FIFO).

## Running

```bash
# Desktop preview (the manual visual gate):
cargo run

# On a device (Android / iOS):
frust run -d <device-id>
```

This is a standalone package (its own `[workspace]` root and `Cargo.lock`,
excluded from the Frust root workspace — the same shape as
`examples/huddle`), so build/test/gate it from **this directory**, never
with `-p` from the repo root:

```bash
cargo build --locked && cargo test \
  && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

### Screenshots / visual gate

No automated pixel-diff or headless-screencap tooling exists in this repo
(the same convention `docs/DEVELOPMENT.md` documents for every other example);
capturing one screenshot per section is therefore a **manual gate** for a
person at a desk, not something a sandboxed, display-less CI
run could produce:

1. `cargo run` from this directory (desktop preview).
2. Click each of the 9 tabs; toggle brightness (◐/◑) and reduce-motion
   (▶/⏸) at each tab, confirming every specimen/demo repaints live (no stale
   hardcoded color, no residual animation once reduced-motion is on).
3. A screenshot tool of the reviewer's choice (`cmd+shift+4` on macOS, or an
   OS screenshot utility) captures each of the 9 states for a visual diff
   against the vendored reference builds.

The automated gate this repo *does* ship — `tests/smoke.rs`'s
`every_page_mounts_at_every_size_and_brightness` — headlessly proves every
real section builds/lays out/paints without panicking at 2 viewport sizes ×
both brightnesses (see that test's doc comment); it is a structural-soundness
proof, not a substitute for the visual gate above.

## Theming

This app's `setup` block seeds `frust_glyph::baseline()` as the app-wide
default theme (dark-first) — a shell's own built-in fallback is
`Theme::neutral()` now, so an explicit install is required. The header
toggles rebuild the theme via a `ThemeBuilder` over the Glyph baseline
(`.brightness(...)` + `.map_motion(...)` for the reduced-motion flag) and force
it app-wide with `set_app_theme`. Toggling reduced-motion collapses every
transition pattern used across the catalog (the section `pattern_switcher`,
the Motion page's own demo switchers, and every navigator push) to a short
linear crossfade — a framework-level guarantee, not something this app
implements itself.

## Coverage

Cross-referenced against the reference-build inventory
(the vendored dark/light/motion builds) — ✓ = live in the app, N/A = documented gap
with reason (see each page's own module docs for the full rationale).

### §01 Foundations (`pages/foundations.rs`)

| Item | Status |
|---|---|
| Background ramp (bg-void → bg-hover, 6) | ✓ |
| Accent (amber) | ✓ |
| Semantic (cyan/success/warning/error + faint, 8) | ✓ |
| Text: fg, fg-muted | ✓ |
| Text: fg-dim, fg-faintest | N/A — no `ColorScheme` role (`frust_glyph::tokens::color`'s own documented leftover); painting one would require a hardcoded hex, which the token-not-hardcode convention forbids |
| Borders (border, border-bright) | ✓ |
| `StatusPalette` extension row | ✓ |
| `GlyphInk` extension row (brightness-invariant) | ✓ |
| Type scale (6 roles) | ✓ |
| Radius scale (4/6/10/16/full) | ✓ |

### §02 Buttons (`pages/buttons_forms.rs`)

| Item | Status |
|---|---|
| Primary / Secondary / Ghost / Danger / Icon | ✓ |
| `.small()` | ✓ |
| Loading demo | ✓ (checkbox-driven, not a timed auto-reset — no async-sleep primitive available without an out-of-scope dependency) |
| Disabled state | N/A as a dedicated builder — `ButtonView` has no `.enabled(false)` seam; a statically-`.loading(true)` button stands in, captioned honestly |
| Press feedback (0.96 scale) | ✓ (captioned; built into `Button` itself) |

### §03 Form Controls (`pages/buttons_forms.rs`)

| Item | Status |
|---|---|
| Text input (prompt-style) | ✓ |
| Textarea (`.multiline`) | ✓ |
| Checkbox | ✓ |
| Radio pair | ✓ |
| Toggle switch (spring) | ✓ — `frust_glyph::toggle`, the Glyph catalog's own authored toggle-spring switch (`plugins/glyph/src/toggle.rs`), not a `frust`/`frust-widgets` baseline item |
| Select dropdown | N/A — no `frust` widget exists for it |
| Slider with live readout | ✓ |

### §04 Status + Feedback (`pages/feedback.rs`)

| Item | Status |
|---|---|
| Badges: Success / Warning / Error (dotted) | ✓ |
| Badges: Neutral, Accent | ✓ |
| Tags (dismissible) | ✓ |
| Alerts (Info/Success/Warning/Error) | ✓ |
| Toast triggers + 2.4s auto-dismiss caption | ✓ |
| Toast per-variant coloring | N/A — framework gap: `frust_glyph::toast_host`'s queue is `Vec<String>` with no variant field, so every toast paints as `ToastVariant::Plain` regardless of trigger; a future `frust-glyph` change, flagged in-page |
| Progress bar | ✓ (fixed 65%, the documented fallback option) |
| Skeleton, dots loader | ✓ |

### §05 Navigation (+ Avatar) (`pages/navigation.rs`)

| Item | Status |
|---|---|
| Tabs (sliding indicator) | ✓ |
| Segmented control | ✓ |
| Breadcrumb | ✓ |
| Bottom nav | ✓ — mapped to `glyph_nav_bar` (no dedicated bottom-nav widget; captioned char-vs-icon item faces, the iOS tofu-risk story) |
| Avatar (3 sizes/accents) | ✓ |

### §06 Content + Data (`pages/content.rs`)

| Item | Status |
|---|---|
| Standard card (title/desc/footer) | ✓ |
| Stat card grid (4-up, up/down deltas) | ✓ |
| List item (glyph/title/sub/meta/chevron) | ✓ |
| Data table | N/A — no dedicated Frust widget; `glyph_list` stands in, captioned as such |
| Terminal/code block (staggered) | ✓ |
| Accordion (3 items, 1 open) | ✓ |
| Empty state | ✓ |
| Tooltip (+ brightness-invariant ink caption) | ✓ |

### §07 Overlays (`pages/overlays.rs`)

| Item | Status |
|---|---|
| Confirmation modal | ✓ |
| Command palette (live filter) | ✓ |
| Bottom sheet (`SlideUp` push) | ✓ |
| Empty state | N/A here — owned by the Content section, not duplicated |

### Motion build — 11 demos (`pages/motion.rs`)

| # | Demo | Status |
|---|---|---|
| 01 | Press feedback | ✓ |
| 02 | Toggle spring | ✓ |
| 03 | Sliding tab indicator | ✓ |
| 04 | Accordion height | ✓ |
| 05 | Modal | ✓ |
| 06 | Bottom sheet | ✓ |
| 07 | Command palette | ✓ |
| 08 | Toast | ✓ |
| 09 | Staggered log reveal | ✓ |
| 10 | Screen transition (+ composable pattern picker: FadeThrough / SharedAxis::X / FadeScale / GlyphSlide) | ✓ |
| 11 | Boot sequence | ✓, at `term_block`'s standard ~90ms/line stagger — N/A for the reference's slower ~260ms/line cadence: no custom `GlyphStagger { per_item_delay }` is exposed on `term_block` (only `.staggered(bool)`); labeled as such in-page |
| — | Duration/easing token table (5 durations, 3 easings) | ✓ |
| — | Reduced-motion note | ✓ |

### Components the reference inventory listed as "no direct Frust widget"

An earlier research pass (written before this catalog's pages were filled in)
flagged 11 components as having no direct `frust`
widget. In practice the `frust-glyph` catalog plugin already ships a
purpose-built widget for all but one:

| Component | Actual status |
|---|---|
| Tag (dismissible) | ✓ `frust_glyph::tag` — covered, §04 |
| Stat card | ✓ `frust_glyph::stat_card` — covered, §06 |
| Terminal/code block | ✓ `frust_glyph::term_block` — covered, §06 |
| Breadcrumb | ✓ `frust_glyph::breadcrumb` — covered, §05 |
| Avatar | ✓ `frust_glyph::avatar` — covered, §05 |
| Tooltip | ✓ `frust_glyph::tooltip` — covered, §06 |
| Empty state | ✓ `frust_glyph::empty_state` — covered, §06 |
| Command palette | ✓ `frust_glyph::command_palette` — covered, §07/Motion |
| Bottom navigation | ✓ (mapped to `frust_glyph::glyph_nav_bar`, no dedicated bottom-nav widget — see §05 above) |
| Boot sequence animation | ✓, approximated (see Motion #11 above) |
| Staggered log reveal | ✓ `frust_glyph::term_block(...).staggered(true)` — covered, Motion #09 |

### Coverage audit result

Walking every item in the reference inventory against the shipped pages found
**no missed item requiring a new polish fix** — every checklist entry is
either a live ✓ or a documented N/A-with-reason above (each already
cross-checked against the actual widget source, and
re-confirmed here against the merged tree). No cross-page code changes were
needed; the full own-dir gate (`cargo build --locked && cargo
test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`)
passes clean against all seven filled pages (see `tests/smoke.rs` for the
automated per-page/per-size/per-brightness sweep).
