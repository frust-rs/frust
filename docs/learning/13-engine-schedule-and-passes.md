# Lab 13 — Schedule and passes

**Concept:** Lab 4 stopped at "`EngineRenderer::encode` records the frame's passes". Between the
compiled strips (lab 11) and the GPU sit a pure *scheduler* that decides how many render passes
a frame needs, and a *frame walk* that records them — plus a few the scheduler never sees — into
the caller's encoder. Learn both and every pass label in a GPU capture has a line of code behind it.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `Schedule::build` — recording → `Vec<Round>`; `make_room`, `cut_at` serve wide fans | `crates/frust-engine/src/schedule/mod.rs` | ≈458, ≈1022, ≈1069 |
| `Round`, `RoundTarget`, `RoundOp`, `Composite`, `FilterPass`, `PageTarget` | `crates/frust-engine/src/schedule/mod.rs` | ≈250–400 |
| `MAX_CHAIN_DEPTH`, `PING_PONG_GROUPS`, `MAX_LIVE_PAGES` | `crates/frust-engine/src/schedule/mod.rs` | ≈218–234 |
| `PageParity` (`Even`, `Odd`, `Spill`) | `crates/frust-engine/src/schedule/pages.rs` | ≈180–195 |
| `EngineRenderer::encode` → `encode_traced` → `record_frame` | `crates/frust-engine/src/renderer.rs` | ≈993, ≈1036, ≈1542 |
| `depth_disabled` — the `FRUST_ENGINE_NO_DEPTH` switch | `crates/frust-engine/src/config.rs` | ≈59 |
| `DEPTH_FORMAT`, `depth_state`, `EnginePipeline::ALL` (nine) | `crates/frust-engine/src/gpu/pipelines.rs` | ≈137, ≈452, ≈244 |
| `DEPTH_COMPARE`, `DepthAttachment` | `crates/frust-engine/src/gpu/depth.rs` | ≈69 |
| `MODULES`, `HELPERS` — how WGSL is assembled | `crates/frust-engine/src/gpu/shader_src.rs` | ≈37, ≈113 |
| `UnpremultiplyPass` — the present-side conversion | `crates/frust-engine/src/gpu/present.rs` | ≈79 |
| `EngineSpan::ALL` — the four GPU spans | `crates/frust-engine/src/diag.rs` | ≈46–68 |
| `EngineTarget`, `OutputAlpha` — the destination | `crates/frust-engine/src/lib.rs` | ≈61, ≈81 |
| The scheduler's tests | `crates/frust-engine/tests/schedule.rs`, `crates/frust-engine/tests/scrub_served.rs` | — |

## The four ideas

1. **The scheduler allocates nothing and touches no device.** `Schedule::build` walks the
   compiler's `CommandRecorder` and returns `Round`s in execution order. A `Round` is *one render
   pass over one target*: `RoundTarget::Root` (the frame's surface) or `RoundTarget::Page` (a
   pooled intermediate holding one isolated layer). Its `ops: Vec<RoundOp>` run in list order —
   `RoundOp::Draws(range)` or `RoundOp::Composite(Composite { layer, parity, bounds, opacity })`.
   A filter round carries no ops at all; its work is `Round::filter: Option<FilterPass>`. One
   target can take several rounds, each after the first loading what the previous one left
   (`PageTarget::continued`). Layers render deepest-first into pages that ping-pong between
   `PageParity::Even` and `PageParity::Odd`; a third, `PageParity::Spill`, exists for exactly one
   shape — the navigation transition. A layer at opacity 1.0 with no blend, mask or clip is
   *inlined* (spliced into its parent, no page); opacity 0 is *dropped*. So most frames contain no
   isolated layer and schedule to **one** round. The module doc's section *What this scheduler
   serves, and what it refuses* is the contract, and worth quoting:

   > The scheduler serves every layer tree [`MAX_LIVE_PAGES`] pooled pages are enough for:
   > isolated layers nested at most [`MAX_CHAIN_DEPTH`] deep; isolated layers *beside* each other
   > under one parent, a fan of any width […]; a chain hanging off an isolated ancestor's later
   > child, which is what a navigation transition records […]. Everything else — a layer that finds
   > no page even after a cut and the spill, a deeper chain, a filter this engine does not render,
   > a filter layer nested inside another recorded layer, a non-default blend mode, a layer mask or
   > layer clip path — is refused with [`EngineError::SchedulerEscalation`].

   `MAX_CHAIN_DEPTH` is 4; `MAX_LIVE_PAGES` is `PING_PONG_GROUPS + 1` = 3. A refused shape does
   **not** degrade to a simpler render: the next section of the same doc, *A refusal is a skipped
   frame, not a fallback*, says there is no second renderer. `frust-render` drops the frame,
   counts it, and logs it rate-limited (`log_engine_refusal` in
   `crates/frust-render/src/renderer.rs`), so the surface keeps its last presented image — which
   is why a shape that refuses *every* frame of a gesture reads as a frozen scrub, and why the
   spill page was added (`docs/LIMITATIONS.md`, `engine-scheduler-skip-on-escalation`).

2. **The real pass order is `record_frame`'s, not a diagram's.** `encode` delegates to
   `encode_traced`, which compiles, runs `Schedule::build`, checks every page against the pool
   ceiling, and only then — past every fallible step — calls `record_frame`. So an `EngineError`
   anywhere leaves the caller's encoder untouched (lab 4, idea 3). Inside `record_frame`:

   | # | Pass (label) | Recorded when | Span |
   |---|---|---|---|
   | a | `frust-engine clear` — `LoadOp::Clear` to the base colour, no pipeline | always | Main |
   | b | `frust-engine opaque strips` — `LoadOp::Load`, depth **write** | `opaque_count > 0` and an opaque pipeline (depth on) | Main |
   | c | loop over rounds: **either** a filter pass (`frust-engine filter pass`) **or** a strip pass (`frust-engine alpha strips` on the surface, `frust-engine layer page` on a page) — never both | once per round | Composite for pages and filters, Main for the surface |
   | c′ | `frust-engine hole punch` after each *non-filter* round | the filter branch `continue`s past it (≈1726) | Main |
   | d | `frust-engine hole punch` after the loop (≈1873) | always *called* | Main |

   Two details the table compresses: (c′) and (d) record a pass only when the frame has a punch
   pipeline (a `ClearRect` was recorded and the base colour is not opaque, `let punches =` ≈1093)
   **and** that cut's instance count is non-zero (`PunchPass::record` returns early on
   `count == 0`); and a surface round with no segments records nothing, while a page round always
   records its pass — that is what clears the pooled page. The unpremultiply pass
   (`UnpremultiplyPass`, `unpremultiply.wgsl`) is **not** recorded here: `frust-render` records it
   after `encode_traced` succeeds, on its `EngineDirectUnpremultiply` arm (lab 16). `EngineTarget`
   (view, format, extent, optional caller depth, `OutputAlpha`) is all the walk knows about its
   destination.

3. **Depth is how painter order survives two passes.** The opaque pass draws only fully covered
   interior spans of surface draws, depth write on; the alpha passes test against it without
   writing (`DEPTH_FORMAT` is `Depth24Plus`, `depth_state` compares `LessEqual` and toggles only
   the write, `DEPTH_CLEAR` is the far plane 1.0). `strip.wgsl`'s vertex stage sets z to 1.0 minus
   the strip's `depth_index` (lab 11, painter order) over 2^24, so a translucent draw *behind* an
   opaque one fails the depth test per fragment instead of blending over it.
   `FRUST_ENGINE_NO_DEPTH=1` (`config::depth_disabled`, read once per process) is consulted at
   `let depth_enabled` (≈1072): the scratch builder never splits opaque spans out (`split_opaque`
   ≈2619), pass (b) disappears, and every instance goes through the surface rounds' blended pass
   in painter order via `StripAlpha` instead of `StripDepthAlpha`. A measured observation recorded
   outside the repo: a plain frame records **three** full-screen surface passes — the empty clear,
   the opaque pass with `LoadOp::Load`, and the round with `LoadOp::Load`. That is how the code is
   written today; do not "fix" it from this chapter.

4. **Nine WGSL files, five programs, nine pipelines.** `crates/frust-engine/shaders/` holds nine
   files, and the code treats them in three different ways:

   | File | Role |
   |---|---|
   | `strip.wgsl` | The strip program: `vs_main` + `fs_main`. Header: "Derived from vello_sparse_shaders 0.2.0 (shaders/render.wesl) […] This module has four pipeline variants over the same two entry points" |
   | `clear.wgsl`, `copy.wgsl` | Intermediate-region clear and copy programs |
   | `filter.wgsl` | The filter dispatcher: one pass kind per round |
   | `unpremultiply.wgsl` | Frust's own present-side program, compiled per surface by `UnpremultiplyPass::new`, no prelude |
   | `helpers.wgsl` | Binding-free prelude `shader_src` prepends to strip, clear, copy and filter — not a pass |
   | `filters_blur.wgsl`, `filters_drop_shadow.wgsl` | Also preludes — prepended to `filter.wgsl` only (`FILTER_KERNELS`, `DROP_SHADOW_KERNELS`) |
   | `blend.wgsl` | Ported compose-and-mix module; compiled only by `crates/frust-engine/tests/layers.rs`. No runtime pipeline uses it |

   `MODULES` in `shader_src.rs` is four entries (strip, clear, copy, filter); `unpremultiply` is
   the fifth program. Layer composites do **not** go through `blend.wgsl`: a `Composite` op is one
   rect-strip instance through the strip program with `color_source = COLOR_SOURCE_LAYER`. The
   `strip.wgsl` header's "four variants" is stale — `EnginePipeline::ALL` has **nine** pipelines:
   six strip variants (`StripIntermediate`, `StripAlpha`, `StripDepthAlpha`, `StripOpaque`,
   `StripDestOut`, `StripDepthDestOut`) plus `Clear`, `Copy`, `Filter`. `Clear` and `Copy` are
   warmed up but `record_frame` never binds them — the clear pass is a load op. The rest of
   `crates/frust-engine/src/gpu/` in one line each: `targets.rs` pools intermediates under
   `MAX_INTERMEDIATE_DIMENSION` (8192), answering an over-ceiling request with a value;
   `bindings.rs` holds `ExternalTextures`, the host-owned textures (video frames, imports) drawn in
   per-texture runs; `depth.rs` owns the depth attachment and the caller-shares-it contract;
   `present.rs` is `UnpremultiplyPass` for iOS's straight-alpha swapchain. Timing: every pass is
   charged to one of `EngineSpan::ALL = [Prepass, Main, Composite, Blit]` — Prepass is the glyph
   atlas replay, Main the clear, opaque, alpha and punch passes, Composite every page and filter
   pass, Blit the unpremultiply pass. The ring is inert unless the build has `perf-trace` and the
   device got `TIMESTAMP_QUERY` (`crates/frust-gpu/src/diag.rs`).

## Experiments

All GPU-free except 13.5. Run the crate's tests per file, never bare — see the README's note
on `proptest_strips`.

### 13.1 — Count rounds from the scheduler's own tests

```bash
cargo test -p frust-engine --test schedule      # 33 passed
```

Read `nested_chain` and `sibling_fan` at the top of `crates/frust-engine/tests/schedule.rs`, then:
`a_depth_four_chain_alternates_page_groups_and_ends_at_the_surface` — a 4-deep chain is **5**
rounds (four pages, innermost first, `Even, Odd, Even, Odd`, then the root);
`two_sibling_opacity_layers_schedule_to_three_rounds` — two siblings are **3** rounds;
`a_three_wide_sibling_fan_cuts_the_root_round_and_reuses_the_first_page` — three siblings are
**5** (the root round is cut after the first pair). The general rule is in
`a_sibling_fan_of_any_width_costs_one_page_round_each_and_a_root_round_per_pair`: *n* page rounds
plus ⌈*n*÷2⌉ root rounds, never the spill page.

### 13.2 — Schedule your own recordings

Copy the `layer` and `draw` helpers from `crates/frust-engine/tests/schedule.rs` into a scratch
integration test (delete it afterwards), record a draw, `push_layer(layer(0.5), None)`, a draw, a
second `push_layer(layer(0.5), None)`, a draw, two `pop_layer()`s, run `Schedule::build` with
`TierCaps::fake(DownlevelProfile::Full)` and `PageConfig::default()`, and print each round. Real
output (`cargo test -p frust-engine --test <scratch> -- --nocapture`):

```
== nested PushLayer(0.5) > PushLayer(0.5)
  3 rounds
  round 0: Page(layer 1, depth 2, Even, continued false)  ops=[Draws(2..3)]  released=[]
  round 1: Page(layer 0, depth 1, Odd, continued false)  ops=[Draws(1..2), Composite(layer 1, Even, opacity 0.5)]  released=[Even]
  round 2: Root  ops=[Draws(0..1), Composite(layer 0, Odd, opacity 0.5)]  released=[Odd]
== fan of five siblings
  8 rounds
  round 0: Page(layer 0, depth 1, Odd, continued false)  ops=[Draws(0..1)]  released=[]
  round 1: Page(layer 1, depth 1, Even, continued false)  ops=[Draws(1..2)]  released=[]
  round 2: Root  ops=[Composite(layer 0, Odd, opacity 0.5), Composite(layer 1, Even, opacity 0.5)]  released=[Odd, Even]
  round 3: Page(layer 2, depth 1, Odd, continued false)  ops=[Draws(2..3)]  released=[]
  round 4: Page(layer 3, depth 1, Even, continued false)  ops=[Draws(3..4)]  released=[]
  round 5: Root  ops=[Composite(layer 2, Odd, opacity 0.5), Composite(layer 3, Even, opacity 0.5)]  released=[Odd, Even]
  round 6: Page(layer 4, depth 1, Odd, continued false)  ops=[Draws(4..5)]  released=[]
  round 7: Root  ops=[Composite(layer 4, Odd, opacity 0.5)]  released=[Odd]
== no layers
  1 rounds
  round 0: Root  ops=[Draws(0..2)]  released=[]
== chain five deep
  REFUSED: scheduler escalation: 5 nested isolated layers, deeper than the 4-deep chain the simple scheduler serves
```

Five siblings never hold more than `Odd` and `Even`: the root round is cut every two composites,
each cut hands both pages back, and the surface's three rounds are one painter's-order walk.

### 13.3 — What a refused shape becomes

```bash
cargo test -p frust-engine --test scrub_served  # 2 passed, 1 ignored (needs a GPU)
```

`every_frame_of_the_scrub_schedules_without_refusing` sweeps forty transition opacities over the
shape the spill page exists for and asserts every frame is served *on* the spill;
`a_scrubbed_frame_never_holds_more_pages_live_than_the_bound` checks `MAX_LIVE_PAGES`. The file
header says why forty frames and not one: a refusal is a skipped frame with no fallback, so a
shape that refuses at fractional opacity freezes the surface for the whole gesture. The depth-5
chain in 13.2 is what a still-refused shape looks like: an `Err`, not a degraded picture.

### 13.4 — Decode `paint_and_rect_flag` (reading, not running)

Open `strip.wgsl`, read the `StripInstance` comment, then `fs_main`, and map the bits (the last
row is the whole of "compositing a page": coverage times opacity times one texel):

| Bits | Field | What `fs_main` does |
|---|---|---|
| 31 | `RECT_STRIP_FLAG` | rect strip: analytic AA from packed edge fractions; else coverage from `alphas_texture` (1.0 for a solid span) |
| 29–30 | `color_source` | 0 = `COLOR_SOURCE_PAYLOAD`: use payload and paint type; 1 = `COLOR_SOURCE_LAYER`: `textureLoad` the page |
| 26–28 | `paint_type` (payload source) | 0 solid (payload is RGBA8), 1 image, 2–4 linear, radial, sweep gradient, 5 blurred rounded rect |
| 0–25 | `paint_texture_idx` (`PAINT_TEXTURE_INDEX_MASK`) | record offset into `encoded_paints_texture` for every non-solid paint |
| 0–7 | opacity (layer source) | a `Composite`'s `opacity`, byte-scaled back to 0–1 |

### 13.5 — (Optional, needs a display and GPU) Depth off, measured

In `benchmarks/frust_bench`, compare `FRUST_TRACE=1 cargo run` against
`FRUST_ENGINE_NO_DEPTH=1 FRUST_TRACE=1 cargo run` on the default S1 scenario and read the
`encode` p95 (lab 8). GPU spans only appear from a `perf-trace` build on a device with
`TIMESTAMP_QUERY`. Expect pass (b) to vanish and the round's blended pass to carry every instance.

## What to notice before moving on

- The scheduler is plain data in, plain data out; every claim above about page counts is a
  CPU-only test you can run in under a second. Reach for `Schedule::build` before a GPU capture.
- A pass label in a capture maps to exactly one site in `record_frame`: clear, opaque strips,
  alpha strips, layer page, filter pass, hole punch. Anything else (atlas replay, unpremultiply,
  shader-quad prepass) is recorded outside it.
- When prose and code disagree — the `strip.wgsl` "four variants" header, a "blend.wgsl
  composites layers" reading — the code wins. Read `record_frame` again after every engine change.
