# frust-gallery

The shared widget/page **case registry** both the static snapshot generator
(`crates/frust-testing`'s widget-snapshots bin) and the future browser
gallery app (`examples/web-gallery`, wasm) read. It is a plain library —
dev-only, never shipped in an app's own dependency graph.

Its own manifest declares no `frust-render`/`frust-gpu`/`wgpu` dependency:
a `Case` records a pure `View` construction, never a rendered frame. The
renderer graph is still a **transitive** dependency, though — each of the
five design-system plugins this crate uses for [`theme()`](src/theme.rs)
depends on the `frust` facade, and the facade carries `frust-render` ->
`frust-gpu` -> `wgpu` (`cargo tree -p frust-gallery -e normal -i wgpu`).
So `wasm32` compilability is not established today; it depends on that
graph being made wasm-clean, which the web-shell work owns.

## Adding a case

1. Pick a module for it. Base (framework-only, no design-system plugin)
   cases live under `src/base/`, one module per catalog category; a design
   system's own cases get their own `src/<design>/` module as that design
   system's case batch lands.
2. Write a `fn() -> frust_core::AnyView<()>` that builds the view tree for
   the case, wrapping the concrete `View` with `frust_core::any(..)`. Frame
   it with `base::framed` (or `base::framed_in` for a module recording at
   its own viewport) rather than rolling your own wrapper — see "Let the
   variant reach the pixels" below.
3. Register the case. `src/base/mod.rs` accepts both module shapes:
   - a module that exposes one `pub(super) const <NAME>: Case` per case
     (`basics`, `text`, `layout`, `styling`) adds each constant to the
     `SINGLES` array;
   - a module that exposes a `pub const CASES: &[Case]` slice (everything
     else) adds that slice to the `PARTS` array.
   Either way give the case a slug (see "Slug rule" below), a title, and a
   `design` tag, and leave `size`/`scale`/`time_ms` at `Case::DEFAULT_SIZE` /
   `Case::DEFAULT_SCALE` / `Case::DEFAULT_TIME_MS` unless the case has a
   specific reason to differ (e.g. `layout`'s roomier 480x320 frame, or an
   animation case pinning a later point on the timeline via `time_ms`).
   `base::cases()` concatenates `PARTS` — including `SINGLES` — once at
   runtime, and `cases()` in [`src/lib.rs`](src/lib.rs) concatenates every
   module's contribution (today just `base::cases()`; a new top-level module
   adds itself there when it lands).

`tests/registry.rs` enforces that every slug in the registry is unique and
that every case's `build` constructor actually runs without panicking.

## Let the variant reach the pixels

Every case is recorded twice, once per `Variant`, and the recorder clears
each pass to that variant's own theme `surface`
(`crates/frust-testing/src/snapshot.rs`'s `base_color`). A case that paints a
fixed, opaque colour across its whole frame overwrites that clear and makes
its light and dark PNGs byte-identical — the exact split these previews
exist to show. So:

- frame with `base::framed`/`base::framed_in`, which size but never fill;
- leave body and label text at its themed default (`text(..)` resolves
  `on_surface`) instead of hard-coding a glyph colour;
- where a widget genuinely needs a contrasting swatch, size that swatch
  smaller than the frame, and pick swatch and ink colours that stay legible
  against a light *and* a dark surface;
- remember that a scrollable viewport (`list_view`, `scroll_view`) hands its
  content a tight cross axis, so a row's own width cannot create that margin
  — box the viewport itself instead.

## The pure-`View` constraint

A `Case::build` function is a plain `fn() -> frust_core::AnyView<()>` — it
must NOT reach for the reactive runtime (`use_signal`, `use_context`, an
`Owner`) or a wall clock. Both consumers of this registry
(`crates/frust-testing`'s `record_view` today, the wasm gallery app later)
build a `RenderRoot<(), V>` with **no runtime attached at all**: `rebuild`
just calls `build()` again on every pass, there is no `Owner` for a signal
subscription to register against, and there is no clock — a case that reads
current time or a reactive context would panic (or silently misbehave)
instead of recording a deterministic frame.

Any animation timing a case needs comes from [`Case::time_ms`] instead: a
fixed frame timestamp the recorder hands the widget tree, which an animating
widget differences itself against rather than sampling a live clock.

## Slug rule

A case's slug is the corresponding website page's file stem — `button`,
`icon-button`, `animated-opacity`, and so on. A design-system's own variant
of a Base page prefixes that stem with its own tag: `material/button`,
`cupertino/button`, `glyph/button`, `shadcn/button`, `beui/button`. Slugs are
unique across the whole registry.

## Dev-only

Like `crates/frust-testing`, this crate is test/tooling infrastructure, not
something an app depends on. It is a root-workspace member (covered by the
root manifest's `examples/*` glob) rather than a standalone workspace: it
scaffolds no per-project Android/iOS build output and carries no
out-of-tree git dependency, so none of the reasons `examples/huddle` /
`examples/playground` / friends give for excluding themselves apply here.

## Consumers

- `crates/frust-testing`'s widget-snapshots bin walks `cases()`, records
  each one through `record_view` under both light and dark themes (via
  [`theme()`](src/theme.rs)), and writes the result out as a PNG plus a
  manifest.
- `examples/web-gallery` (a future wasm app) will read the same registry to
  render an interactive, in-browser widget gallery.
