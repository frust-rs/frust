# Lab 4 — Scene → GPU (`frust-render`)

**Concept:** `frust-render` is the one crate allowed to speak vello/wgpu. It
does four jobs, each in its own file: translate commands (`convert.rs`), run
the surface lifecycle + per-frame encode/present (`renderer.rs` +
`lifecycle.rs`), pick a render tier (`tier.rs` + `context.rs`), and persist
compiled GPU pipelines (`pipeline_cache.rs`). This lab reads the seams and
then flips them.

## Where it lives (all under `crates/frust-render/src/`)

| Thing | File | Anchor |
|---|---|---|
| `SceneSink` trait (12 methods — the backend abstraction) | `convert.rs` | ≈20–62 |
| `encode_scene` / `encode_into` — the `Command` match | `convert.rs` | ≈64–128 |
| `impl SceneSink for vello::Scene` | `convert.rs` | ≈130–242 |
| `SurfacePhase` (`NoSurface`/`SurfaceReady`/`SurfaceLost`) | `lifecycle.rs` | ≈46–63 |
| `FrameOutcome` / `EncodeOutcome` | `lifecycle.rs` | ≈204–235 |
| `SurfaceRenderer::encode()` — reset vello scene, translate, `render_to_texture` | `renderer.rs` | ≈436–515 |
| `SurfaceRenderer::present()` — acquire swapchain, blit, present | `renderer.rs` | ≈531–624 |
| `TierBackend::Gpu(vello::Renderer) / Cpu(..)` | `renderer.rs` | ≈56–66 |
| Device creation + tier probe | `context.rs` | ≈410–520 |
| `RenderTier`, `GPU_REQUIRED_DOWNLEVEL_FLAGS` (`COMPUTE_SHADERS \| INDIRECT_EXECUTION`) | `tier.rs` | ≈23–60 |
| `FRUST_RENDER_TIER` parsing | `tier.rs` | ≈187–216 |
| CPU tier: `vello_cpu` behind the same `SceneSink` | `cpu_tier.rs` | ≈65–316 |
| Pipeline-cache framing (`frame`/`unframe` + adapter fingerprint) | `pipeline_cache.rs` | ≈22–85 |

## The three ideas

1. **One trait, two backends.** `encode_into` walks `scene.commands()` and
   calls `SceneSink` methods; `vello::Scene` implements the sink by calling
   `self.fill(..)` / `self.stroke(..)` / `self.draw_glyphs(..)`
   (`convert.rs` ≈130–242), and `CpuSink` implements the *same trait* over
   `vello_cpu::RenderContext` (`cpu_tier.rs` ≈173). This is the
   scene-purity rule paying rent: a whole second rasterizer cost one file.
2. **Encode and present are split on purpose.** `encode()` does the real
   work (translate + `Renderer::render_to_texture` into an intermediate
   `Rgba8Unorm` target, `renderer.rs` ≈479); `present()` only acquires the
   swapchain texture, blits, and presents. The split is what lets
   `FrameStats` distinguish GPU work from vsync wait (chapter 3), and it's
   the seam a future render thread would cut along.
3. **Rendering is a state machine, not an assumption.** Surfaces die
   (window close, Android rotation). Everything routes through
   `SurfacePhase`; rendering outside `SurfaceReady` is a no-op, and
   `FrameOutcome::SurfaceLost` triggers recreation. When you get a black
   window someday, this enum is where you look first.

## Experiments

### 4.1 — Read one command's full journey

Pick `Command::Path` (your chapter-2 sparkline emits these). Follow it:
`convert.rs` ≈117–125 (match arm, `PathStyle::Fill`/`Stroke` split) →
`fill_path`/`stroke_path` sink methods → the vello impl (≈130–242) calling
`vello::Scene::fill(Fill::NonZero, transform, brush, None, &path)`. Total
distance from widget to vello: two function calls. Now do the same for
`GlyphRun` — note it becomes `draw_glyphs(font).font_size(..).brush(..).draw(..)`.

### 4.2 — Run the whole pipeline with no GPU at all

The CPU tier compiles behind a non-default feature:

```bash
cargo test -p frust-render --features cpu-tier
```

Then read `cpu_tier.rs::CpuTierRenderer::render` (≈113–145): resize pixmap →
replay commands through `CpuSink` → `render_to_pixmap` → hand back raw
`u8` pixels that `present()` uploads to the same intermediate texture the GPU
path uses. Same `Scene`, same sink trait, zero shaders.

Force it live (desktop preview, from a generated app or huddle with the
feature enabled): `FRUST_RENDER_TIER=cpu cargo run` — or watch it *refuse*:
`FRUST_RENDER_TIER=gpu` on a capable adapter is a no-op win, but the override
never forces GPU onto an incapable adapter (`tier.rs::select_render_tier`
≈106–181 — read the refusal branch).

### 4.3 — The GPU smoke test is your headless playground

```bash
cargo test -p frust-render -- --ignored
```

Read `crates/frust-render/tests/gpu_smoke.rs`: it builds a scene, encodes,
and reads pixels back with no window — test tier T3. This file is the
smallest end-to-end harness in the repo; copy it whenever you want to ask
"what does vello actually output for these commands?" without a shell in the
way.

### 4.4 — See vello's own noise

`FRUST_LOG=debug cargo run` (`benchmarks/frust_bench`) surfaces the vello/wgpu log lines
the desktop logger normally suppresses (`crates/frust-shell-desktop/src/logger.rs`
≈103). This is the dial you'll want turned when chapter 5 makes you
curious what the renderer is complaining about.

## What to notice before moving on

- `renderer.rs` ≈479 — `renderer.render_to_texture(&device, &queue,
  vello_scene, &target_view, &params)` — is the single line where this
  entire repo hands control to vello. Everything below it is chapter 5.
- The pipeline cache (`pipeline_cache.rs`) is Vulkan-only and fingerprinted
  per adapter; on your Mac it's a silent no-op. File it away for Android
  cold-start work.
