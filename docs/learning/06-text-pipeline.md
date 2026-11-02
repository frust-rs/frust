# Lab 6 — Text: String → Glyphs (`frust-text`)

**Concept:** Text is the pipeline's side road: a string is *shaped* (font
matching through frust's own font oracle, ligatures and bidi through
`parley_engine`), *line-broken* to a width by frust's own line layout, then
flattened into the positioned `GlyphRun`s you met in chapter 1. Frust's one
big trick here is a cache boundary: shaping is width-independent and
expensive; line-breaking is width-dependent and cheap — so they're split,
and only the cheap half re-runs when your window resizes.

## Where it lives (all under `crates/frust-text/src/` unless noted)

| Thing | File | Anchor |
|---|---|---|
| `TextContext::layout(text, style, max_width)` — the entry | `context.rs` | ≈378 |
| Font oracle — per-cluster OS fallback, one backend per OS (`apple.rs`, `fontconfig.rs`, …) | `oracle/mod.rs` | |
| The shaping call: `ShapeContext::shape(oracle, text, style)` → width-independent `Shaped`, via `parley_engine` | `shape/mod.rs` | ≈303 |
| Shape cache — 128-entry LRU (`DEFAULT_CAPACITY`), keyed `ShapeKey` (text + style) — **no width in the key** | `shape_cache.rs` | ≈31, ≈68, ≈241 |
| `ShapeCacheStats { shapes, line_breaks, hits, evictions }` | `shape_cache.rs` ≈39; exposed by `shape_cache_stats` at `context.rs` ≈489 |
| Line breaking over the shaped atoms (greedy), then `reorder`, `align`, truncation | `layout/break.rs` ≈87; `layout/reorder.rs`, `layout/align.rs`, `layout/truncate.rs` | |
| `TextLayout::to_scene_runs()` — conversion to `GlyphRun`s + origin translate | `layout/mod.rs` | ≈503 |
| Layout → `frust_scene::GlyphRun` coordinate contract | `convert.rs` | |
| `GlyphRun` → `glifo` lowering inside the engine | `crates/frust-engine/src/text/mod.rs` | `lower_glyph_run` ≈287 |
| `TextEditor` (frust's own editing engine for `TextInput`/IME, over the same layout; no `PlainEditor`) | `editor/mod.rs` (+ `cursor.rs`, `selection.rs`, `compose.rs`) | ≈191 |
| Where render meets text: GlyphRun compilation into strips + glyph atlas | `crates/frust-engine/src/compile/mod.rs` | `compile_glyph_run` ≈2020 |
| A widget using all of it: `Text`  | `crates/frust-widgets/src/text.rs` | |

The purity rule holds here too: no `parley_engine` or `fontique` type escapes
`frust-text` (`lib.rs` exports and module doc; only `shape/` names the engine). Widgets see `TextStyle` in, `GlyphRun` out.

## Experiments

### 6.1 — Shape text with no GPU, no window

`crates/frust-text/tests/emoji.rs` (≈56–128) is a complete standalone
pipeline run: `TextContext::new()` → `layout(..)` → `to_scene_runs()` →
assertions on glyph IDs and advances.

```bash
cargo test -p frust-text -- --nocapture
```

Copy it into a scratch test and poke: shape `"fi"` vs `"f\u{200B}i"` (does
the ligature split?), a mixed Arabic/Latin string (watch run count jump),
`letter_spacing` changes. Print `run.glyphs` — those `(id, x, y)` triples
are exactly what chapter 1's `Command::GlyphRun` will carry.

### 6.2 — Catch the shape cache working

The design claim: resizing a window re-runs **line-breaking only**, never
re-shaping. Verify it instead of believing it. In the desktop shell (or a
scratch spot in huddle), log `text_ctx.shape_cache_stats()` once a second,
then run huddle and drag-resize the window continuously:

- `line_breaks` should climb with every width change;
- `shapes` should stay flat (all hits) once the UI's strings are warm;
- now type into a `TextInput` — `shapes` climbs (new text = new key).

If you only do one experiment in this chapter, do this one — it's the
cache boundary made visible in two counters.

### 6.3 — From glyphs to pixels

Read `crates/frust-engine/src/compile/mod.rs`'s `compile_glyph_run` function
(≈2020): the route for each run is one `route: Option<RunRoute>` —
exactly two variants (`crates/frust-engine/src/text/atlas_policy.rs`
≈979–985): `RunRoute::Atlas(AtlasRun)` or `RunRoute::Outline(OutlineReason)`.
This route is decided once per run, per frame, by `classify_runs` in the
collect phase (`classify_runs`, ≈1813 in `compile/mod.rs`) and re-tested by `admit_run` when the
run is drawn (`atlas_policy.rs` ≈1849, called from `take_run_route`, `compile/mod.rs` ≈1995); it can only narrow an Atlas
route to Outline, never the reverse. On an Atlas route, glyphs sample the glyph
atlas, but a glyph the atlas had no room for still draws as an outline — so the
pixel path differs per glyph (see `AtlasPass::build`'s `unrasterizable` and `no_room` counts, `atlas_policy.rs` ≈2111 and ≈2126), not
one route per run for every glyph. See [`15-engine-text.md`](15-engine-text.md)
idea 1 for the route narrowing at draw time. An outline run does not leave the
CPU as bare outlines and positions for the GPU to rasterize — glifo drives
`EngineGlyphSink` (`crates/frust-engine/src/text/backend.rs`), which itself
rasterizes it into strips through the compiler's own `StripGenerator`, the same
machinery every other draw goes through. (COLR glyph faces never reach the
atlas; see `crates/frust-engine/tests/atlas_churn.rs`'s
`a_colour_face_never_reaches_the_atlas_and_its_glyphs_still_draw` test. Color
glyphs draw as outlines via `crates/frust-engine/src/text/color.rs`'s layer
recombination.)

### 6.4 — One theme-contract gotcha worth meeting early

`Text` bakes its resolved glyph *color* into the shaped layout at **layout**
time; paint just replays the brush (`docs/ARCHITECTURE.md`, Theme delivery).
That only stays correct because `RenderRoot::set_theme` forces
`ChangeFlags::LAYOUT`. Test your understanding: toggle
light/dark while huddle runs. Why would text color lag one frame if
`set_theme` marked only `PAINT`? (Chapter 7's Android layout-skip is the
context where this contract earns its keep.)

## What to notice before moving on

- `TextEditor` (`editor/mod.rs` ≈191) is the same shaping and line-layout pipeline plus edit ops and
  IME state-sync — read it when you touch `TextInput`, not before.
- Shaping cost is why S6 ("text shaping stress") exists in the benchmark
  suite — chapter 8 lets you measure what you just learned.

## Chapter 15 — Where GlyphRun enters the engine

The layout pipeline outputs positioned `GlyphRun`s; [`15-engine-text.md`](15-engine-text.md) follows a `GlyphRun` through compilation and rendering in the strip pipeline.
