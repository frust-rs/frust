# Lab 6 — Text: String → Glyphs (`frust-text`)

**Concept:** Text is the pipeline's side road: a string is *shaped* (font
matching, ligatures, bidi — Parley's job), *line-broken* to a width, then
flattened into the positioned `GlyphRun`s you met in chapter 1. Frust's one
big trick here is a cache boundary: shaping is width-independent and
expensive; line-breaking is width-dependent and cheap — so they're split,
and only the cheap half re-runs when your window resizes.

## Where it lives (all under `crates/frust-text/src/` unless noted)

| Thing | File | Anchor |
|---|---|---|
| `TextContext::layout(text, style, max_width)` — the entry | `context.rs` | ≈356 |
| The Parley shaping call (`ranged_builder` → `builder.build(text)`) | `context.rs` | ≈372 |
| Shape cache — 128-entry LRU, keyed `(text, style)` — **no width in the key** | `shape_cache.rs` | ≈10–226 |
| `ShapeCacheStats { shapes, line_breaks, hits, evictions }` | `shape_cache.rs` ≈33; exposed at `context.rs` ≈672 |
| `TextLayout::to_scene_runs()` — memoized conversion + origin re-translate | `layout.rs` | ≈89 |
| Parley layout → `frust_scene::GlyphRun` lowering | `crates/frust-engine/src/text/mod.rs` | `lower_glyph_run` ≈188 |
| `TextEditor` (Parley `PlainEditor` wrapper for `TextInput`/IME) | `editor.rs` | ≈160–287 |
| Where render meets text: GlyphRun compilation into strips + glyph atlas | `crates/frust-engine/src/compile/mod.rs` | `compile_glyph_run` ≈1514 |
| A widget using all of it: `Text`  | `crates/frust-widgets/src/text.rs` | |

The purity rule holds here too: no Parley type escapes `frust-text`
(`lib.rs` module doc). Widgets see `TextStyle` in, `GlyphRun` out.

## Experiments

### 6.1 — Shape text with no GPU, no window

`crates/frust-text/tests/emoji.rs` (≈54–116) is a complete standalone
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
(≈1514–1591): the sink maps each `frust_scene::GlyphRun` to a set of `RunRoute`
entries that classify glyphs into strips (atlas, color, fallback) and route
them through the glyph atlas for coordinate and outline caching. The engine
handles both shape complexity — outlines ride the atlas's own coverage —
and atlas pressure — color glyphs and uncacheable large runs render direct
to the frame. Note what's here: no rasterization — the engine ships outlines
and positions. (COLR glyph faces never reach the atlas; see `crates/frust-engine/tests/atlas_churn.rs`'s `a_colour_face_never_reaches_the_atlas_and_its_glyphs_still_draw` test. Color glyphs draw as outlines via `crates/frust-engine/src/text/color.rs`'s layer recombination.)

### 6.4 — One theme-contract gotcha worth meeting early

`Text` bakes its resolved glyph *color* into the shaped layout at **layout**
time; paint just replays the brush (`docs/ARCHITECTURE.md`, Theme delivery).
That only stays correct because `RenderRoot::set_theme` forces
`ChangeFlags::LAYOUT`. Test your understanding: toggle
light/dark while huddle runs. Why would text color lag one frame if
`set_theme` marked only `PAINT`? (Chapter 7's Android layout-skip is the
context where this contract earns its keep.)

## What to notice before moving on

- `TextEditor` (≈160–287) is the same shaping pipeline plus edit ops and
  IME state-sync — read it when you touch `TextInput`, not before.
- Shaping cost is why S6 ("text shaping stress") exists in the benchmark
  suite — chapter 8 lets you measure what you just learned.

## Chapter 15 — Where GlyphRun enters the engine

The layout pipeline outputs positioned `GlyphRun`s; [`15-engine-text.md`](15-engine-text.md) follows a `GlyphRun` through compilation and rendering in the strip pipeline.
