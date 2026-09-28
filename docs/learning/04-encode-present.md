# Lab 4 — Scene → GPU (`frust-render` / `frust-gpu` / `frust-engine`)

**Concept:** Three crates now share the job `frust-render` alone used to do. `frust-gpu` owns the
wgpu device/surface/pipeline-cache foundation; `frust-engine` compiles a scene into GPU strip data
and records the GPU passes; `frust-render` decides which per-frame render path a surface takes and
drives the whole thing per frame — it contains exactly one renderer (`frust-engine`'s strip
pipeline), not a choice between several. This lab reads the seams and then flips them.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `RenderContext` — the wgpu instance/device, created lazily | `crates/frust-gpu/src/context.rs` | ≈136 |
| `ConfiguredSurface` / `SurfaceFactory` / surface lifecycle | `crates/frust-gpu/src/surface.rs`, `lifecycle.rs` | — |
| Persisted pipeline-cache framing (`frame`/`unframe` + adapter fingerprint) | `crates/frust-gpu/src/pipeline_cache.rs` | ≈56/≈75 |
| `choose_engine_render_path` — which of the two engine arms a surface takes | `crates/frust-render/src/context.rs` | ≈152 |
| `create_engine_surface` — configures a surface, checks capability, picks the arm | `crates/frust-render/src/context.rs` | ≈480 |
| `engine_support`/`ENGINE_REQUIRED_DOWNLEVEL_FLAGS` — the (empty) capability gate | `crates/frust-render/src/tier.rs` | ≈113/≈50 |
| `SurfaceRenderer::submit` — per-frame encode/present | `crates/frust-render/src/renderer.rs` | ≈1064 |
| `SceneCompiler::compile` — `Scene` → `CompiledFrame` (strips/draws/paints) | `crates/frust-engine/src/compile/mod.rs` | ≈737 |
| `EngineRenderer::encode`/`encode_traced`/`record_frame` — records the frame's rounds into the caller's encoder | `crates/frust-engine/src/renderer.rs` | ≈993/≈1036/≈1542 |
| `GpuStrip` — one strip's GPU-ready draw data | `crates/frust-engine/src/gpu/strips.rs` | ≈108 |

## The three ideas

1. **Foundation and renderer are different crates now.** `frust-gpu` knows nothing about scenes or
   strips — it hands out a device, a configured surface, a persisted pipeline cache. `frust-engine`
   knows nothing about surfaces or presentation — it compiles a `Scene` and records passes into a
   caller-owned `wgpu::CommandEncoder`. `frust-render` is the seam: it decides the render path
   (`choose_engine_render_path`) and owns `SurfaceRenderer`, the thing a shell actually drives.
   This is the scene-purity rule from lab 1, one layer further down.
2. **There is no tier to flip any more.** The old CPU tier and classic (vello) tier are both
   gone; `frust-engine`'s strip pipeline is the only renderer `frust-render` contains, and it is
   not a cargo feature. `SurfaceRenderer::submit` records the whole frame — compile, GPU passes,
   `queue.submit` — in one caller-owned encoder, never opening a second one (two sanctioned
   exceptions: growing the image atlas, and replaying a dirty glyph-atlas page, both strictly
   ahead of the scene pass).
3. **A refused frame is not a crash — it's a value.** `SceneCompiler::compile` returns
   `Result<CompiledFrame, EngineError>`; malformed geometry (`NaN`/`inf`) refuses rather than
   hangs. `EngineRenderer::encode` refusing anything mid-frame leaves the caller's encoder
   untouched — nothing submits, and the previous frame's swapchain content simply persists. There
   is no second renderer to retry through.

## Experiments

### 4.1 — Read one command's full journey

Pick `Command::Path` (your chapter-2 sparkline emits these). Follow it into
`crates/frust-engine/src/compile/mod.rs`'s `compile` match arm, through to the strip-generator call
that turns it into `GpuStrip` instances (`crates/frust-engine/src/gpu/mod.rs`'s layouts). Now do
the same for `GlyphRun` — note it routes through `crate::text::lower_glyph_run` instead
(`crates/frust-engine/src/text/mod.rs`).

### 4.2 — Watch the capability gate refuse an adapter

`ENGINE_REQUIRED_DOWNLEVEL_FLAGS` (`tier.rs`) is deliberately empty — read `engine_support`'s
refusal arm and its four unit tests (same file) to see the one question left: whether *any* flag is
ever required. There is no override to force a renderer any more — `frust run --render-tier` was
removed along with the last tier (`crates/frust-cli/src/cli.rs`'s
`rejects_the_retired_render_tier_flag` test is the fossil that proves it: it asserts the flag is
now a parse error).

### 4.3 — The GPU smoke test is your headless playground

```bash
cargo test -p frust-render --test gpu_smoke -- --ignored
```

Read `crates/frust-render/tests/gpu_smoke.rs`: it builds a scene, encodes through
`frust_render::HeadlessRenderer` (engine-backed, `crates/frust-render/src/headless.rs`), and reads
pixels back with no window — test tier T3. Its pixels are premultiplied alpha. Copy this file
whenever you want to ask "what does the engine actually output for these commands?" without a
shell in the way.

### 4.4 — See the engine's own noise

`FRUST_LOG=debug cargo run` (`benchmarks/frust_bench`) surfaces wgpu's own log lines the desktop
logger normally passes through unfiltered (`crates/frust-shell-desktop/src/logger.rs`) — the
logger applies no target-based filtering at all (its `enabled()` only checks level; see the
`targets_are_never_suppressed` test in that file), so `debug` simply widens the level filter that
was already there.

## What to notice before moving on

- `crates/frust-engine/src/renderer.rs`'s `EngineRenderer::encode` — the single function that
  walks a `CompiledFrame` into GPU passes — is where this repo hands control to the strip
  pipeline. `crates/frust-engine/shaders/` (eight pass shaders — `strip`, `clear`, `copy`,
  `blend`, `filter`, `filters_blur`, `filters_drop_shadow`, `unpremultiply` — plus a `helpers.wgsl`
  prelude `crates/frust-engine/src/gpu/shader_src.rs` prepends at load time) is short enough to
  read cold from here if you want to go straight to the GPU side.
- The pipeline cache (`frust-gpu::pipeline_cache`) is Vulkan-only and fingerprinted per adapter; on
  your Mac it's a silent no-op. File it away for Android cold-start work.
- This is the hand-off point: the next six chapters take the engine apart piece by piece —
  [11-engine-compiler.md](11-engine-compiler.md) (`Scene` → `CompiledFrame`, strips, fast-rect,
  depth, refusal), [12-engine-clips-layers-punch.md](12-engine-clips-layers-punch.md),
  [13-engine-schedule-and-passes.md](13-engine-schedule-and-passes.md) (rounds/pages, pass order,
  the shaders above, `FRUST_ENGINE_NO_DEPTH`), [14-engine-paints-images-filters.md](14-engine-paints-images-filters.md),
  [15-engine-text.md](15-engine-text.md), and [16-gpu-substrate.md](16-gpu-substrate.md).
