# Lab 15 — Text inside the engine (`frust-engine::text`)

**Concept:** [Lab 6](06-text-pipeline.md) ends where `frust-text` hands the display list a
positioned `GlyphRun`. This lab starts there. Inside the engine a glyph run is the one primitive
the compiler does not rasterize itself: each run is handed to `glifo` (outline fetch, scale, hint,
cache), which calls back into the compiler's own strip generator. Before that happens, a policy
decides — once per run, per frame — whether its glyphs are *sampled from the glyph atlas* or
*rasterized as outlines*. Almost everything interesting in this chapter is that decision.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `lower_glyph_run` — one `GlyphRun` → one `glifo::GlyphRunBuilder`; returns `GlyphRunOutcome` | `crates/frust-engine/src/text/mod.rs` | ≈188–234 |
| `glyph_atlas_policy`; `font_is_readable` (font gate); `font_has_color_glyphs` (COLR check) | `crates/frust-engine/src/text/mod.rs` | ≈262, ≈311, ≈399 |
| `EngineTextBackend` / `EngineGlyphSink` — glifo's backend/sink pair; `GlyphRunOutcome` | `crates/frust-engine/src/text/backend.rs` | ≈691, ≈186, ≈124 |
| `AtlasPolicy::classify_run` / `admit_run` / `would_overspend`; `RunRoute`, `OutlineReason` | `crates/frust-engine/src/text/atlas_policy.rs` | ≈1161, ≈1242, ≈1270; ≈650, ≈549 |
| `device_font_size` / `absorbed_scale` / `quantize_font_size` | `crates/frust-engine/src/text/atlas_policy.rs` | ≈507, ≈482, ≈529 |
| `glyph_texel_budget`, `glyph_texels_at`, `GLYPH_ATLAS_SHARE`, `SETTLE_FRAMES`, `MAX_CACHED_FONT_SIZE` | `crates/frust-engine/src/text/atlas_policy.rs` | ≈370, ≈414, ≈298, ≈258, ≈277 |
| `ColorGlyph` — COLR clip/fill stream → one shape + brush per layer | `crates/frust-engine/src/text/color.rs` | ≈92 |
| `SceneCompiler::for_caps` (hinting), `classify_runs`, `take_run_route`, `compile_glyph_run` | `crates/frust-engine/src/compile/mod.rs` | ≈530, ≈1413, ≈1489, ≈1514 |
| `CompileSpans` (`glyphs` is a subset of `walk`) | `crates/frust-engine/src/compile/mod.rs` | ≈207 |
| `ENCODE_TRACE_COLUMNS` — the `frust-perf enc` line's fields | `crates/frust-engine/src/renderer.rs` | ≈374 |
| `atlas_disabled` — reads `FRUST_ENGINE_NO_ATLAS` once per process | `crates/frust-engine/src/config.rs` | ≈74 |
| The glifo pin: `glifo = { version = "=0.3.0", … }` | `Cargo.toml` (workspace) | ≈298 |


## The four ideas

1. **One run, one glifo builder, one brush.** `compile_glyph_run` checks the run is non-empty and
   its font readable, encodes the run's brush **once**, then calls `lower_glyph_run`: one
   `glifo::GlyphRunBuilder` (exact-pinned `=0.3.0`) with default embolden, no variation coords, an
   explicit `.hint(..)`, fed the run's `{id, x, y}` glyphs. glifo drives `EngineTextBackend` (the
   retained `GlyphPrepCache` plus an `AtlasCacher`) and issues per-glyph commands to
   `EngineGlyphSink`, which rasterizes through the compiler's own `StripGenerator` and records one
   `EngineDraw` per glyph — all painted with that one encoded `Paint`. The route was decided
   *before* this walk: `SceneCompiler::compile` runs `classify_runs` (the `classify` lap), closes the
   atlas frame with `AtlasPolicy::build` (the `admit` lap), then walks (chapter 11 covers those
   phases); `take_run_route` hands each run its route, re-tested by `admit_run`.

2. **Routing: `RunRoute::Atlas` or `RunRoute::Outline(reason)`.** A settled run goes to the atlas:
   glifo resolves or allocates a slot and each glyph becomes one image paint naming it
   (`set_paint_image` + `fill_rect` at the sink). Everything else is one outline per glyph.
   `classify_run` answers in this order, each an `OutlineReason`:
   - `Disabled` — `FRUST_ENGINE_NO_ATLAS` (or a disabled image residency; `glyph_atlas_policy`).
   - `NotCollecting` — the compiler is outside the `classify` lap's `Phase::Collect` window;
     unreachable for a correctly phased compile, since `classify_run` is only ever called from
     within it.
   - `ColorFont` — the face has a `COLR` table (idea 3).
   - `TransformUncacheable` — `device_font_size` returned `None`: not a positive uniform scale
     without skew. Refused outright, not as an optimisation: glifo probes its cache with the
     *unabsorbed* size before discovering it will not cache, so a rotated run could hit a bitmap
     rasterized unrotated and draw it.
   - `UnusableSize` — `quantize_font_size` refused it (NaN, infinite, ≤ 0).
   - `SizeTooLarge` — device size past `MAX_CACHED_FONT_SIZE` (128 px).
   - `SizeAnimating` — `observe_size` saw this font at a new device size within `SETTLE_FRAMES` (8).
   - `ResidencyFull` — `would_overspend`: `resident_texels` + this run's demand (distinct glyph ids ×
     `glyph_texels_at(size)`) exceeds `glyph_texel_budget` = array texels / `GLYPH_ATLAS_SHARE`
     (a half). Each glyph is priced at its full em square — a deliberate over-estimate, so glyphs
     never quietly eat the images' half of the shared allocator.

   The size watched is always the **device** size: `font_size` × the transform's absorbed uniform
   scale, which is what glifo keys on — never the raw display-list `font_size`. `admit_run` re-asks
   the budget as each route is consumed (glifo inserts nothing until a run is drawn) and can only
   narrow `Atlas` to `Outline`. (`glyph_entry_ceiling` only *reports* the budget.)

3. **Safety and colour.** `font_is_readable` checks the table directory and `head.unitsPerEm`
   itself, because glifo `unwrap`s both — a non-font blob would panic the frame path (E17 in
   `docs/CODE_STANDARDS.md`). A refused run draws nothing and counts into
   `CompiledFrame::skipped_glyphs`. A COLR face is refused the atlas *before insertion*: glifo would
   record its layers into the page recorder every glyph on that page shares, this tier's replay
   cannot lower them, and glifo 0.3.0 cannot withdraw the entries afterwards. On the outline path
   `ColorGlyph` recombines glifo's clip/fill stream into one shape + brush per layer and drops the
   **whole** glyph (`refuse`) when a layer has no exact engine spelling — missing, never
   half-painted. Hinting is split: `SceneCompiler::for_caps` sets `hint_text = !is_mobile_tier(caps)`
   (`new` defaults to `false`), and glifo hints only under a positive uniform scale on top of that.

4. **What it costs.** Atlas hits save rasterization, not the walk: open measurement
   act_000001a07572829c (a Zabin action item, quoted here, not re-run in this lab) found glyph
   lowering at 87–92% of the compile walk on text-heavy scenes even with every glyph an atlas hit.
   The counter is `CompileSpans::glyphs`, lapped around each `Command::GlyphRun` arm — a **subset**
   of `walk`, left out of `CompileSpans::total`. It reads non-zero only in a `perf-trace` build,
   where `EncodeTrace` logs (at `info`) one `frust-perf enc` line per 60 frames in
   `ENCODE_TRACE_COLUMNS` order: compare `glyphs_us` with `walk_us`, and read `glyph_draws=` /
   `atlas_glyphs=` for how many glyphs drew and how many sampled the atlas.

## Experiments

GPU-free except 15.5. Use a per-worktree `CARGO_TARGET_DIR`. Run the crate's tests per file,
never bare — see the README's note on `proptest_strips`.

### 15.1 — The outline path, end to end

```bash
cargo test -p frust-engine --test text_basic      # 21 passed
```

Its `compiler()` helper turns the atlas **off** (`ImageResidency::disabled`), so every case pins the
outline route: `every_inked_glyph_of_a_run_becomes_one_draw`,
`a_gradient_brushed_run_encodes_one_entry_and_one_ramp_for_the_whole_run` (encode-once), and
`a_font_blob_that_is_not_a_face_skips_its_run_instead_of_panicking` (the font gate). The
atlas-vs-outline pair is in `crates/frust-engine/tests/atlas_churn.rs`:
`a_settled_run_draws_every_glyph_out_of_the_atlas` (5 atlas draws, 4 entries — the two `l`s share a
key) versus `an_animating_size_never_reaches_the_atlas_through_the_compiler`.

### 15.2 — The routing and budget rules

```bash
cargo test -p frust-engine --lib text::atlas_policy   # running 0 tests — see below
cargo test -p frust-engine --test atlas_churn          # 47 passed, 2 ignored (GPU)
```

`crates/frust-engine/src/text/atlas_policy.rs` has no test module: the atlas_churn target compiles
that file into itself via a `#[path]` attribute (it is `pub(crate)` and pure). Read
`the_atlas_route_closes_once_glyph_residency_reaches_its_ceiling`,
`mixed_sizes_spend_one_shared_texel_budget_in_either_order`, `one_admitted_run_cannot_overshoot_the_budget_by_its_whole_self`
(why `admit_run` exists) and `a_scale_that_moves_reads_as_an_animation_though_the_font_size_never_does`.

### 15.3 — COLR and hinting

```bash
cargo test -p frust-engine --test text_color_hint     # 17 passed
```

Also outline-only (`outline_only`): `a_colour_glyph_paints_one_draw_per_layer_and_counts_as_one_glyph`
and `a_colour_glyph_costs_the_compiler_no_clip_at_all` pin the layer recombination;
`a_mobile_tier_compiler_draws_unhinted_output_byte_identical_to_hint_text_false` and
`a_desktop_tier_compiler_draws_hinted_output_byte_identical_to_hint_text_true` pin the hinting
rule. The COLR *atlas refusal* is atlas_churn's
`a_colour_face_never_reaches_the_atlas_and_its_glyphs_still_draw`.

### 15.4 — Watch a run change route (write your own; do not commit it)

In a scratch file under `crates/frust-engine/tests/`, copy `LATIN_FONT`, `HELLO = [5, 6, 7, 7, 8]`
and a 24 px `GlyphRun` from atlas_churn. Build **one** `FontHandle` (size history is keyed on the
blob id; a fresh handle reads as a first appearance) and compile the same scene each frame on a
`SceneCompiler::new(512, 256)`, calling `acknowledge_glyph_clears()`/`acknowledge_glyph_replay()`
after each; print `glyph_draws`, `atlas_glyph_draws`, `glyph_atlas_entries()`. A: identity root;
B: root `Affine::scale` 1.0→1.3, then held; C: `Affine::skew(0.3, 0.0)`. Real `--nocapture` output:

```text
-- A: same run, fixed root
fixed f0               glyph_draws=5 atlas_glyph_draws=5 entries=4 -> Atlas
fixed f1               glyph_draws=5 atlas_glyph_draws=5 entries=4 -> Atlas
fixed f2               glyph_draws=5 atlas_glyph_draws=5 entries=4 -> Atlas
-- B: same run, root scale changes every frame, then holds
scale 1.0 f0           glyph_draws=5 atlas_glyph_draws=5 entries=4 -> Atlas
scale 1.1 f1           glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
scale 1.2 f2           glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
scale 1.3 f3           glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f4            glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f5            glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f6            glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f7            glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f8            glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f9            glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f10           glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f11           glyph_draws=5 atlas_glyph_draws=0 entries=4 -> Outline
hold 1.3 f12           glyph_draws=5 atlas_glyph_draws=5 entries=9 -> Atlas
hold 1.3 f13           glyph_draws=5 atlas_glyph_draws=5 entries=9 -> Atlas
-- C: skewed root
skew 0.3 f0            glyph_draws=5 atlas_glyph_draws=0 entries=0 -> Outline
```

`font_size` never changed — only the root scale did — yet the run read as animating. The last change
was frame 3, so outlines hold through frame 11 (3 + `SETTLE_FRAMES`) and frame 12 caches. 9 entries,
not 8: at 1.3× the two `l`s land in different subpixel buckets. The skewed run never touches the
cache (`TransformUncacheable`).

### 15.5 — OPTIONAL (needs a GPU): real glyph pixels in the atlas

```bash
cargo test -p frust-engine --test atlas_render -- --ignored
```

Replays atlas pages on a real device and reads the layer back: an outline covers only its slot.

## What to notice before moving on

- Lab 6 §6.3 summarises the same routing from the frust-text side: COLR glyphs do **not** ride the
  atlas — they are refused it per face (`OutlineReason::ColorFont`) and drawn through `ColorGlyph`.
- Every refusal is a *route*, never a failure: outlines are correct pixels, only slower — which is
  why `FRUST_ENGINE_NO_ATLAS` is a safe bisection switch for a text-rendering defect.
