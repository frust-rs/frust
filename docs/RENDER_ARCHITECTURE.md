# Frust - RENDER Architecture

## Overview

RENDER covers two crates that turn a display list and styled text into pixels. `frust-render` is
the wgpu+Vello GPU backend that encodes the renderer-agnostic `frust-scene` display list and
presents it to a window surface, owning surface lifecycle, render-tier selection, and per-surface
fragment-shader effects. `frust-text` wraps Parley font matching/shaping into a renderer-agnostic
API (`TextContext`/`TextStyle`/`TextLayout`) producing `frust-scene` `GlyphRun`s, plus the
`TextEditor` engine the platform IME bridges drive. Both crates keep their heavy engine
dependencies (`vello`/`wgpu`, `parley`) out of their public surface so the GPU and text backends
stay swappable.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how RENDER relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-render::context` | `RenderContext` owns the wgpu device/adapter (incl. headless creation for Android pre-init); `SurfaceFactory`/`DetachedSurface` hand a surface across threads |
| `frust-render::renderer` | `SurfaceRenderer` drives per-frame encode/present on a dedicated render thread by default; `DeferredPresent` lets iOS present inside a platform-view transaction |
| `frust-render::lifecycle` | `SurfacePhase`/`FrameOutcome` state machine models a surface that can be destroyed at any time, shared by every shell |
| `frust-render::convert` | `encode_scene` converts a `frust_scene::Scene` into a `vello::Scene` |
| `frust-render::tier` | `select_render_tier` probes adapter capabilities to choose GPU (default) vs experimental CPU rendering, with an env/CLI override |
| `frust-text::context` | `TextContext` owns Parley's font context and a shape cache; `register_fonts` hot-swaps app fonts and invalidates it |
| `frust-text::layout` | `TextLayout` is a finished, measurable shaped block converting to `frust_scene` `GlyphRun`s |
| `frust-text::style` | `TextStyle` and related types form the styling vocabulary (the M3 type-scale surface) |
| `frust-text::editor` | `TextEditor` wraps Parley's `PlainEditor` for caret/selection/composition; the sole owner of UTF-16↔byte IME index conversion |

## Layer Dependencies

Both crates depend on `frust-scene` (CORE) for the `Scene`/`Command`/`GlyphRun` types — the stable
widget↔GPU seam neither crate may bypass. `frust-render` additionally depends on `vello` and
`wgpu`, and optionally on `vello_cpu` (exact-pinned) behind its non-default `cpu-tier` feature;
`frust-text` depends on `parley`. `kurbo` and `peniko` supply the geometry/color vocabulary shared
across both crates' public APIs and `frust-scene`'s. `frust-render` also depends on
`android_system_properties` on Android.

`frust-render` confines every `vello`/`wgpu` type behind its own API: the only two opaque wgpu
wrappers that ever leave the crate are `DetachedSurface` and `DeferredPresent`, used for
cross-thread/cross-transaction handoff — a bare `wgpu::Surface` or `wgpu::Device` never does. The
optional `cpu-tier` path is isolated behind the same `SceneSink` encode seam as the GPU path, so a
breaking `vello_cpu` bump cannot reach the default GPU path. `frust-text` mirrors this: `parley`
never appears outside `TextContext`/`TextStyle`/`TextLayout`, and `TextEditor` is the sole owner of
UTF-16↔byte index conversion — everything else in the crate works in byte offsets.

The scene-layer purity boundary these confinement rules enforce against `frust-scene`/`frust-core`
is a cross-unit rule; see [ARCHITECTURE.md](ARCHITECTURE.md).

## Data Flow

- `frust_scene::Scene` → `encode_scene` → `vello::Scene` → `SurfaceRenderer::encode()`/`present()`,
  direct-to-surface when supported else an intermediate-texture blit, driven from a dedicated
  render thread by default.
- `Command::ShaderQuad` instances render through a per-surface fragment-shader pre-pass into an
  offscreen texture composited into the scene ahead of the main encode.
- `SurfaceAlphaRequest` resolves the platform's compositing/alpha mode to pick the presentation
  path, feeding translucency state upstream to paint.
- Text: style + string → `TextContext` (cached shaping) → `TextLayout` → `GlyphRun`s via
  `to_scene_runs`, consumed by `SceneBuilder` as scene `Command`s. `TextContext::layout_bounded`
  additionally measures against a max line count and applies `TextOverflow` by truncating the shaped
  text (measure-and-truncate), since parley 0.11 exposes no native ellipsis primitive.
- `frust-render`'s shared command walk (`convert.rs`), which both the GPU (vello) and cpu-tier
  (vello_cpu) sinks run through, is where two scene-layer geometry primitives lower to
  backend-specific shapes: a dashed stroke (`Command::Path`'s `DashPattern`) is flattened to a plain
  path via kurbo's dash iterator *before* either sink runs, since vello honors a `kurbo::Stroke`'s
  dash fields but `vello_cpu` does not — pre-flattening once at decode time keeps the two tiers
  pixel-comparable; a per-corner blurred shadow (`Command::BlurredRoundedRect`'s `CornerRadii`)
  collapses to `CornerRadii::largest()`, since both sinks' blurred-rect primitive takes one radius
  (accepted approximation, see LIMITATIONS.md). `Command::RoundedRect`/`PushClipRounded` carry the
  same `CornerRadii` through to an exact `kurbo::RoundedRect` per corner — only the blur path
  collapses it.
- `TextContext::register_fonts` hot-swaps app-supplied fonts and invalidates cached shaping,
  forcing relayout upstream.
- Focus-routed Key/Ime events drive `TextEditor`, producing an `EditingState` (UTF-16 indexed)
  round-tripped through each platform's IME bridge.
- Optional cpu-tier: `vello_cpu` rasterizes into a `Pixmap` uploaded into the same intermediate
  target the GPU blit path uses.

## Key Types

| Type | Purpose |
|------|---------|
| `RenderContext` / `SurfaceRenderer` / `SurfaceFactory` / `DetachedSurface` / `DeferredPresent` | Device ownership, surface lifecycle/present, and the two sanctioned opaque wgpu wrappers for cross-thread handoff |
| `RenderTier` / `TierCaps` | GPU-vs-CPU render-backend selection, probed from adapter capabilities plus an override |
| `SurfacePhase` / `FrameOutcome` / `EncodeOutcome` / `AcquireOutcome` | The surface-can-be-destroyed-anytime lifecycle state machine shared by every shell |
| `encode_scene` | The sole function converting a `frust_scene::Scene` into a `vello::Scene` |
| `CornerRadii` / `DashPattern` (`frust-scene`) | Per-corner rounding and dash geometry carried by `RoundedRect`/`PushClipRounded`/`BlurredRoundedRect`/dashed-stroke commands; lowered to backend shapes in the shared command walk (see Data Flow) |
| `TextContext` / `TextStyle` / `TextLayout` / `TextOverflow` | Renderer-agnostic shaping surface: font/cache state, styling knobs, a finished measurable shaped block, and `layout_bounded`'s measure-and-truncate overflow mode |
| `TextEditor` / `EditingState` / `EditOp` | The Parley-based editing engine and its state-sync payload at the platform IME seam |
