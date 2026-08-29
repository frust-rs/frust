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
| `frust-render::snapshot` | `SnapshotCache` rasterizes each outermost `PushSnapshot`/`PopSnapshot` bracket once per content/size change into a cached texture and returns the frame's `FramePlan` |
| `frust-render::compositor` | `Compositor` draws each `FramePlan` layer as one alpha-blended `CompositeTarget` quad, outside vello |
| `frust-render::tier` | `select_render_tier` probes adapter capabilities to choose GPU (default) vs experimental CPU rendering, with an env/CLI override |
| `frust-render::hybrid_tier` (crate-private, `#[cfg(feature = "hybrid-tier")]`) | A third `SceneSink` over `vello_hybrid`'s sparse-strip renderer, plus a `Blob`-id-keyed image-residency shim; the measurement spike backing `RenderTier::Hybrid` |
| `frust-render::headless` | Offscreen classic (vello/GPU) renderer for goldens/oracles: env-aware adapter resolution (`WGPU_ADAPTER_NAME`/`WGPU_BACKEND`), an expect-adapter/expect-backend fail-fast before any pixel is produced, row-padded readback, and the same limits/render-tier probe the live surface path resolves |
| `frust-text::context` | `TextContext` owns Parley's font context and a shape cache; `register_fonts` hot-swaps app fonts and invalidates it |
| `frust-text::layout` | `TextLayout` is a finished, measurable shaped block converting to `frust_scene` `GlyphRun`s |
| `frust-text::style` | `TextStyle` and related types form the styling vocabulary (the M3 type-scale surface) |
| `frust-text::editor` | `TextEditor` wraps Parley's `PlainEditor` for caret/selection/composition; the sole owner of UTF-16↔byte IME index conversion |

## Layer Dependencies

Both crates depend on `frust-scene` (CORE) for the `Scene`/`Command`/`GlyphRun` types — the stable
widget↔GPU seam neither crate may bypass. `frust-render` additionally depends on `vello` and
`wgpu`, and optionally on `vello_cpu` (exact-pinned) behind its non-default `cpu-tier` feature, and
optionally on `vello_hybrid`/`vello_common`/`glifo` (exact-pinned, same shape) behind its equally
non-default `hybrid-tier` feature; `frust-text` depends on `parley`. `kurbo` and `peniko` supply the
geometry/color vocabulary shared across both crates' public APIs and `frust-scene`'s. `frust-render`
also depends on `android_system_properties` on Android.

`frust-render` confines every `vello`/`wgpu` type behind its own API: the only two opaque wgpu
wrappers that ever leave the crate are `DetachedSurface` and `DeferredPresent`, used for
cross-thread/cross-transaction handoff — a bare `wgpu::Surface` or `wgpu::Device` never does. The
optional `cpu-tier` and `hybrid-tier` paths are each isolated behind the same `SceneSink` encode
seam as the GPU path, so a breaking `vello_cpu`/`vello_hybrid` bump cannot reach the default GPU
path. `frust-text` mirrors this: `parley` never appears outside
`TextContext`/`TextStyle`/`TextLayout`, and `TextEditor` is the sole owner of UTF-16↔byte index
conversion — everything else in the crate works in byte offsets.

The scene-layer purity boundary these confinement rules enforce against `frust-scene`/`frust-core`
is a cross-unit rule; see [ARCHITECTURE.md](ARCHITECTURE.md).

## Data Flow

- `frust_scene::Scene` → `encode_scene` → `vello::Scene` → `SurfaceRenderer::encode()`/`present()`,
  direct-to-surface when supported else an intermediate-texture blit, driven from a dedicated
  render thread by default.
- Third render path, `hybrid-tier` builds only (`RenderTier::Hybrid`, override-only — see Key
  Types): `HybridDirect` renders straight into the acquired swapchain view at the surface-reported
  format, RENDER_ATTACHMENT — no intermediate, no blit, no `PremultiplyPass`, no snapshot
  cache/compositor (`vello_hybrid` owns its own depth texture; one `CommandEncoder` per frame).
  `tier_forces_blit`/`tier_compiled_in` (`context.rs`) are exhaustive per-tier matches over
  `RenderTier`, replacing the two former `!= RenderTier::Gpu` call sites so a tier added later can't
  fall through either check silently. On this path the whole render cost — CPU strip build *and*
  GPU execution — lands in `submit_us` (`encode_us` is only the scene copy into `vello_hybrid`'s own
  scene type); `vello_hybrid` itself outputs premultiplied alpha, so a premultiplied-expecting
  translucent surface (Android's `Inherit`/`PreMultiplied`) is correct as-is on this arm and raises
  no warning — only the straight-alpha translucent mode (iOS's `PostMultiplied`) is refused
  (`hybrid_translucency_refused`), resolving to Mode A with one warning. Every committed Pixel 5
  number is still an opaque-surface number regardless (see LIMITATIONS.md).
- `Command::ShaderQuad` instances render through a per-surface fragment-shader pre-pass into an
  offscreen texture composited into the scene ahead of the main encode.
- `SurfaceAlphaRequest` resolves the platform's compositing/alpha mode to pick the presentation
  path, feeding translucency state upstream to paint.
- Snapshot pre-pass (`snapshot.rs`): each frame's outermost `PushSnapshot`..`PopSnapshot`
  bracket fingerprints its body relative to `base = transform * Affine::translate(rect.origin)`
  (the bracket's own frame-relative origin, so a slide keeps the same fingerprint) and, on a
  fingerprint or size change, rasterizes it once into a cached `Rgba8Unorm` `TEXTURE_BINDING |
  STORAGE_BINDING` texture sized from the rect's own extent under the bracket's raster (device)
  scale, independent of where the rect sits; an unchanged bracket reuses its texture untouched,
  and an entry unused for 2 frames is evicted. A frame that punches a `ClearRect` after the first
  cached bracket lowers whole-frame inline, and a bracket whose texture would exceed 2x the
  surface's pixel area lowers on its own (see LIMITATIONS.md).
- Frame split (`FramePlan`, `snapshot.rs`): vello renders the commands before the first cached
  bracket (the pre segment) — skipped entirely when it draws nothing, letting the compositor's
  own render pass clear to the frame's `base_color` instead. The `Compositor` then draws each
  cached page as one alpha-blended quad after vello: straight blend on `Direct`/`Blit`,
  premultiplied on `DirectPremultiplied`'s swapchain after `PremultiplyPass`; each quad is
  scissored to its enclosing clips' intersection. Commands after the first cached bracket that
  still draw run through one extra transparent vello pass into a scratch texture, composited
  last as a full quad to keep z-order — content recorded BETWEEN two cached brackets lands above
  both (see LIMITATIONS.md). That trailing pass re-opens whatever `PushClip`/`PushClipRounded`/
  `PushLayer` groups were still open where the split falls: `FramePlan::trailing` is a
  `convert::Segment` (its command range plus those still-open group pushes, derived in one walk by
  `frame_split`/`open_group_pushes`) that `encode_range_with_overrides` re-opens first, then
  closes every group still open at the segment's end so each pass stays self-balanced; the
  scratch texture itself ages out after `MAX_UNUSED_FRAMES` (2) frames it goes unused
  (`Compositor::age_scratch`), the same boundary the page-texture cache evicts by.
- Kill switch: `FRUST_NO_SNAPSHOT_LAYERS` (compile-time `option_env!` or runtime env, cached once
  per surface — same compile-time-or-runtime shape as `FRUST_TRACE`; the row lives in
  [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) disables the cache; every bracket then lowers
  through `convert.rs`'s inline emulation, byte-identical to pre-cache behavior. A surface whose
  resolved alpha mode is translucent but not premultiplied (iOS's `PostMultiplied`) never enables
  the cache at all (`snapshot_cache_enabled`/`alpha_mode_is_straight_translucent`), same inline
  path. **Render-path A/B caveat** (also covers `FRUST_NO_DIRECT_SURFACE`/
  `FRUST_NO_SHADER_EFFECTS`): on the direct-to-surface arm the GPU render moves into `submit_us`
  (out of `encode_us`) and `acquire_us` precedes it rather than follows — account for this remap
  before comparing `submit_us` across arms (`SurfaceRenderer::submit`'s doc comment has the full
  v3 field mapping).
- Measurement knobs (`FRUST_AA_MODE`/`FRUST_RENDER_SCALE`, same compile-time-or-runtime shape as the
  kill switch above — rows in [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)): `FRUST_AA_MODE` picks
  the single `vello::AaSupport` mode the renderer compiles pipelines for and the
  `antialiasing_method` every pass requests, cached snapshot pages included, so a frame and its
  composited pages are always anti-aliased alike. `FRUST_RENDER_SCALE < 1` forces the blit arm
  exactly as `FRUST_NO_DIRECT_SURFACE` does (`blit_translucency_refused` included), sizes the
  intermediate at `ceil(w*s) x ceil(h*s)` (`context::scaled_size`, on creation and on resize), and
  encodes both of the frame's vello passes under the root that arm carries: `RenderPath::Blit`'s
  `root`, derived once by `context::blit_root` from the intermediate's actual `target_size` as a
  per-axis target/surface ratio, so a non-integral `w * s` maps exactly onto the ceil'd axis and
  leaves no unpainted edge strip. `convert::encode_range_with_overrides` pre-multiplies it onto
  every command's own transform, the range-scoped `ClearRect` hoist composing on top, after which
  the blit pass upscales through a `Linear`-sampled `TextureBlitter` (`context::blit_filter`). The
  shader pre-pass keeps its raw, unrooted override keys, so its full-size texture is drawn scaled
  into the smaller target; the snapshot-layer cache is refused while scaled (a cached page
  composites in the target's own device space), that refusal reading the same per-surface answer
  the root is built from (`ConfiguredSurface::render_scaled`, derived from the sizes the surface
  holds, not the process-global knob, and re-asked on every resize through
  `SnapshotCache::set_enabled`), so every bracket lowers inline. The cpu-tier is pinned at 1.0,
  and `encode_us` still carries the GPU render, now at the reduced size (the A/B caveat above is
  otherwise unchanged).
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
- Optional hybrid-tier: `hybrid_tier::HybridSink` runs the same shared command walk as the GPU/CPU
  sinks, rasterizing into `vello_hybrid` sparse strips; `RenderTier::Hybrid` is selectable only by
  an explicit override (`FRUST_RENDER_TIER=hybrid` / `frust run --render-tier hybrid`, see
  [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) — no adapter probe ever chooses it, since
  `HYBRID_REQUIRED_DOWNLEVEL_FLAGS` is empty (`tier.rs`): the tier needs neither
  `COMPUTE_SHADERS` nor `INDIRECT_EXECUTION`, the two flags the GPU tier requires and the iOS
  Simulator's Apple2 GPU family lacks.

## Key Types

| Type | Purpose |
|------|---------|
| `RenderContext` / `SurfaceRenderer` / `SurfaceFactory` / `DetachedSurface` / `DeferredPresent` | Device ownership, surface lifecycle/present, and the two sanctioned opaque wgpu wrappers for cross-thread handoff |
| `HeadlessRenderer` | Offscreen classic renderer reused across renders (one adapter/device/`vello::Renderer`); verifies its resolved adapter/backend against an expectation before rendering and returns plain RGBA8 bytes, never a `vello`/`wgpu` type |
| `RenderTier` / `TierCaps` | GPU-vs-CPU-vs-Hybrid render-backend selection, probed from adapter capabilities plus an override; `Hybrid` is override-only (see Data Flow) |
| `SurfacePhase` / `FrameOutcome` / `EncodeOutcome` / `AcquireOutcome` | The surface-can-be-destroyed-anytime lifecycle state machine shared by every shell |
| `encode_scene` | The sole function converting a `frust_scene::Scene` into a `vello::Scene` |
| `CompositeLayer` / `FramePlan` | One cached page's placement/texture for the compositor to draw, and the frame's vello-pass/compositor-layer/hole split those cached pages imply (see Data Flow) |
| `CornerRadii` / `DashPattern` (`frust-scene`) | Per-corner rounding and dash geometry carried by `RoundedRect`/`PushClipRounded`/`BlurredRoundedRect`/dashed-stroke commands; lowered to backend shapes in the shared command walk (see Data Flow) |
| `TextContext` / `TextStyle` / `TextLayout` / `TextOverflow` | Renderer-agnostic shaping surface: font/cache state, styling knobs, a finished measurable shaped block, and `layout_bounded`'s measure-and-truncate overflow mode |
| `TextEditor` / `EditingState` / `EditOp` | The Parley-based editing engine and its state-sync payload at the platform IME seam |
