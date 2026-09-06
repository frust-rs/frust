# frust-gallery

The shared widget/page **case registry** both the static snapshot generator
(`crates/frust-testing`'s widget-snapshots bin) and the future browser
gallery app (`examples/web-gallery`, wasm) read. It is a plain library —
dev-only, never shipped in an app's own dependency graph — and it is
desktop-free: no `frust-render`/`frust-gpu`/`wgpu`/`vello` dependency, so it
can later compile to `wasm32` unchanged.

## Adding a case

1. Pick a module for it. Base (framework-only, no design-system plugin)
   cases live in `src/base/mod.rs` today; a design-system's own cases get
   their own `src/<design>/mod.rs` module as that design system's case batch
   lands.
2. Write a `fn() -> frust_core::AnyView<()>` that builds the view tree for
   the case, wrapping the concrete `View` with `frust_core::any(..)`.
3. Add a `Case { .. }` entry to that module's `pub const CASES: &[Case]`
   slice, giving it a slug (see "Slug rule" below), a title, and a `design`
   tag. Leave `size`/`scale`/`time_ms` at `Case::DEFAULT_SIZE` /
   `Case::DEFAULT_SCALE` / `Case::DEFAULT_TIME_MS` unless the case has a
   specific reason to differ (e.g. an animation case that wants to pin a
   later point on the timeline via `time_ms`).
4. Make sure the module's `CASES` slice is concatenated into
   [`cases()`](src/lib.rs) (already true for `base::CASES`; a new module adds
   itself there when it lands).

`tests/registry.rs` enforces that every slug in the registry is unique and
that every case's `build` constructor actually runs without panicking.

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

- `crates/frust-testing`'s widget-snapshots bin (g1-02) walks `cases()`,
  records each one through `record_view` under both light and dark themes
  (via [`theme()`]), and writes the result out as a PNG plus a manifest.
- `examples/web-gallery` (a future wasm app) will read the same registry to
  render an interactive, in-browser widget gallery.
