# layer-bench — retained-layers composite-cost spike (THROWAWAY LAB)

A standalone measurement bench for the retained-layers feature's **Phase-0
device spike** (`workflow/plans/features/frust-retained-layers/`). It answers
the research sweep's one unanswered question with device numbers: does
compositing pre-rendered textures beat re-rasterizing the equivalent vector
content in vello 0.9's pipeline?

This is a **lab, not shipped API surface.** It deliberately reaches below the
`frust` facade into `frust-core`/`frust-scene` (the documented escape hatch) to
drive vello's composite primitives directly. See `src/lib.rs`'s top-of-file
note.

## The three scenes (tap anywhere to cycle: A → B → B+1relayer → A)

- **A — vector** (`Mode::Vector`): a Feedback-screen-equivalent vector scene
  (badges/tags/alerts/toasts/loaders, calibrated to comparable path/glyph
  counts as `examples/glyph-catalog/src/pages/feedback.rs`), re-rasterized every
  frame. The full-vector baseline.
- **B — composite** (`Mode::Composite`): the same screen area pre-rendered into
  `scene::NUM_LAYERS` offscreen `Rgba8Unorm` image quads (dedicated textures,
  composited via `draw_image` — the same vello path the real feature's
  texture-override composite lowers to), re-rendered **zero** times per frame,
  plus one small live vector spinner so the scene isn't trivially static.
- **B+1relayer** (`Mode::CompositeRelayer`): scene B, but one layer is a
  `ShaderQuad` re-rendered into its own dedicated target **every frame** via
  `frust-render`'s `shader_effects` pre-pass — the `render_to_texture`-style
  per-layer dispatch overhead the spike isolates. (B+1relayer − B = the cost of
  one dirty-boundary re-capture per frame.)

The active mode shows as a top-left HUD label so a device operator can confirm
it visually.

## Driving it

The **initial** mode comes from the compile-time `LAYER_BENCH_SCENE` define
(`a`|`b`|`c`), parsed like the framework's own `FRUST_*` switches — env vars
don't reach an Android process at runtime, so the define is the Android-safe
path. Tapping the screen cycles from there.

```bash
# desktop preview, mode B:
LAYER_BENCH_SCENE=b cargo run

# profiled APK, mode C, perf trace on (JAVA_HOME=/usr/lib/jvm/java-17-openjdk):
frust build apk --profile \
  --define LAYER_BENCH_SCENE=c \
  --define FRUST_TRACE=1 --define FRUST_TRACE_RAW=1
```

`LAYER_BENCH_DPR` (default `3.0`) sizes the offscreen textures to physical
resolution — set it to the target device's DPR for a fair composite-fill /
texture-memory comparison (e.g. `--define LAYER_BENCH_DPR=2.75` for the Xiaomi
12).

Every mode requests continuous frames, so with a `perf-trace` build
(debug/profile compile it in) `FRUST_TRACE=1 FRUST_TRACE_RAW=1` emit one
`frust-perf raw ...` line per frame at the refresh rate — the steady-state
per-frame series the spike compares (`submit_us` / `encode_us`).

## Tests

```bash
cargo test    # T0 scene-construction / command-count / geometry checks (no GPU)
```

## Standalone workspace

Like `examples/shadertoy`/`examples/glyph-catalog`, this is excluded from the
root Cargo workspace (own `Cargo.lock`, project-local `target/`); gate it from
its own directory, not with `-p` from the repo root.
