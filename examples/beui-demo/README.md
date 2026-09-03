# beui-demo

The `frust-beui` design-system plugin's desktop gallery: eighteen pages that
show every ported component interactively — a home page with the upstream
attribution, nine Motion pages (text effects, buttons, controls, selection,
overlays, navigation, surfaces, data, and the WGSL shader backgrounds), three
Agents pages (chat primitives, agent-output panels, the assembled chat app
over a mock streaming driver), four Blocks pages (command surfaces, morphing
tabs and stacks, forms and upload, showcase blocks), and a Theming page.

Root-workspace member (covered by the `examples/*` glob in the root
`Cargo.toml`, not standalone) — desktop-only, no Android/iOS output. Mirrors
`examples/shadcn-demo`'s manifest shape exactly: no `[features]` table.

## Run

```
cargo run -p beui-demo
```

Opens a real winit window titled "beUI Gallery". `frust_beui::install()` runs
in `frust::app!`'s `setup` block, so the app starts themed with beUI's own
token tables (`frust_beui::theme()`) rather than the framework's neutral
fallback.

The window opens at the desktop shell's own default size (800×600 logical
px); set `FRUST_WINDOW_SIZE=<width>x<height>` before running for a roomier
gallery view of the sidebar-plus-content shell.

### The `frust run` device gotcha

If an Android device is attached, the workspace-root `frust run` auto-targets
it instead of a desktop window — this crate has no Android/iOS build output at
all, so that auto-target would fail or pick the wrong app. Use
`cargo run -p beui-demo` (or run `frust run --watch` from *inside* this
directory, which is desktop-only and has nothing else to prefer) for the
desktop preview.

## Nav shell

The rail is `frust_beui::components::animated_sidebar` — the catalog's own
motion-driven sidebar, collapsible via the top bar's
Collapse/Expand button. `animated_sidebar` ports upstream's panel-and-menu,
not a labelled-group header slot (see that module's own doc comment), so this
scaffold keeps the rail as one flat, springing list and folds each grouped
page's section into its own label (`Motion · Text`, `Agents · Chat`, ...)
rather than inventing a group-header widget here. `nav.rs`'s module docs carry
the same note; the upgrade to a genuinely grouped rail is future work for
whichever task next touches `animated_sidebar` itself.

Eighteen pages, in nav order:

- **Home** — title, upstream attribution (MIT, rev
  `10c283e433a8f4f0ac0736684d4426ab612b9f55`), and a category-overview card
  per section, each linking straight into that section's first page.
- **Motion** (9 pages: `text`, `buttons`, `controls`, `selection`, `overlays`,
  `navigation`, `surfaces`, `data`, `shader`) — the 41 motion components with
  every registry variant, plus the five `shader_background` variants on the
  engine's `ShaderQuad` seam.
- **Agents** (3 pages: `primitives`, `panels`, `chat`) — the 17 agent
  components, headlined by the chat app over its mock streaming driver.
- **Blocks** (4 pages: `command`, `morph`, `forms`, `showcase`) — the 22
  blocks, each with the port's premise corrections in its caption.
- **Theming** — the beUI palette across both brightnesses (`BEUI_LIGHT`,
  `BEUI_DARK`), a live light/dark toggle via `frust_beui`'s own
  [`theme_toggle`](../../plugins/beui/src/components/theme_toggle.rs), and a
  Geist/Geist Mono typography specimen. The motion-token visualizer is a
  stub — it names `frust_beui::tokens::motion`'s curve/spring surface without
  wiring a live timeline against it yet.

Content swaps under `frust::motion::switcher::pattern_switcher` with the
`FadeThrough` pattern — non-directional, the idiomatic choice for a
destination change with no spatial relationship. The top bar carries a second,
persistent copy of `theme_toggle` (reachable from every page, not just
Theming) alongside the active page's title.

## Theming today vs. TODO

- **Wired today:** `frust_beui::install()` seeds the beUI theme as the app
  default; both `theme_toggle` instances (top bar, Theming page) flip
  brightness live through `frust::set_app_theme`; every swatch, button, and
  text run on Home/Theming resolves through beUI's own tokens
  (`frust_beui::palette`/`sans_family`/`mono_family`) rather than a literal
  color authored in this crate.
- **TODO:** the motion-token visualizer (Theming page) is a named stub, not a
  live timeline; the nav rail is a flat list rather than a grouped/labelled
  one (see "Nav shell" above); every section page hosts its state in a
  page-local component, so a page starts fresh each time it is opened.

## Verifying it

This dev rig is headless — the binary is never run here. `cargo build -p
beui-demo` is the runtime check; a desktop container (dockur linux-native,
`run-on-desktop.sh`) runs `cargo run -p beui-demo` and captures the
screenshot separately.
