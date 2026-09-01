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
| `frust-render::tier` | `select_render_tier` probes adapter capabilities to choose GPU (default) vs experimental CPU rendering; `RenderTier::Engine` (the `frust-engine` strip pipeline) is a third variant reachable only through an explicit env/CLI override, never through the probe |
| `frust-render::headless` | Offscreen classic (vello/GPU) renderer for goldens/oracles: env-aware adapter resolution (`WGPU_ADAPTER_NAME`/`WGPU_BACKEND`), an expect-adapter/expect-backend fail-fast before any pixel is produced, row-padded readback, and the same limits/render-tier probe the live surface path resolves |
| `frust-text::context` | `TextContext` owns Parley's font context and a shape cache; `register_fonts` hot-swaps app fonts and invalidates it |
| `frust-text::layout` | `TextLayout` is a finished, measurable shaped block converting to `frust_scene` `GlyphRun`s |
| `frust-text::style` | `TextStyle` and related types form the styling vocabulary (the M3 type-scale surface) |
| `frust-text::editor` | `TextEditor` wraps Parley's `PlainEditor` for caret/selection/composition; the sole owner of UTF-16↔byte IME index conversion |

## GPU Substrate (`frust-gpu`, `frust-engine`)

`frust-gpu` is the `wgpu` adapter/device/surface substrate the frust-owned render engine
(`frust-engine`) builds on. It owns no scene display list, no shader pipeline for a specific
renderer, and no `vello`/`glifo` dependency — its job is everything directly above `wgpu` itself:
probing adapter capabilities, and turning that into instances, devices, surfaces, and pooled GPU
resources a renderer built on top can consume without re-deriving them. Neither crate has a spoke
of its own (see [DOC_POLICY.md](DOC_POLICY.md)); both document here because `frust-engine`'s render
pipeline is built on `frust-gpu` and reaches a surface only through RENDER.

`frust-engine` is that render engine: a stateless-per-frame compiler that walks a `frust_scene::Scene`
into sparse strips (vello_common/glifo's rasterizer core), packs them into the GPU layouts its
ported WGSL reads, and records a frame's passes into a caller-owned `wgpu::CommandEncoder`. It
depends on `frust-gpu` for device/pipeline/pool substrate and on `frust-scene` for the display list;
every rendering path returns an `EngineError` rather than panicking (E17).

### Module Structure (`frust-gpu`)

| Module | Responsibility |
|--------|-----------------|
| `frust-gpu::caps` | `TierCaps` is plain adapter data (`TierCaps::probe` from a real `wgpu::Adapter`, or `TierCaps::fake` with none) driving pipeline/atlas decisions; `DownlevelProfile` is `Full` or `WebGl2` — `WebGl2` whenever the backend is `Gl` or the `FRUST_ENGINE_DOWNLEVEL` override rehearses the browser ceiling on a desktop adapter |
| `frust-gpu::context` | `Context` owns one `wgpu::Instance` and creates its one logical `DeviceHandle` lazily on first `Context::device().await`, then reuses it (a logical device is display-independent and must survive surface loss/recreation); `DeviceHandle` is cheap to clone since wgpu's `Adapter`/`Device`/`Queue` are themselves `Arc`-backed handles to the same underlying device; instance flags strip `DEBUG` on the Android emulator, which cannot survive it; a device request on the iOS Simulator is forced back up to a 256-byte uniform alignment since the Simulator misreports its own; an installed `on_uncaptured_error` handler latches the first error into `DeviceHandle::first_uncaptured_error` (never overwritten) while logging a bounded, periodically-resumed count of the rest |
| `frust-gpu::lifecycle` | `SurfacePhase`/`AcquireOutcome`/`FrameOutcome` model a surface Android can destroy and recreate at any time (rotation, backgrounding), driven by shell lifecycle callbacks; also holds the crate's two sanctioned-`unsafe` surface constructors, `create_android_surface` (raw `ANativeWindow*`) and `create_metal_surface` (raw `CAMetalLayer*`), each isolating its safety contract in one function |
| `frust-gpu::surface` | `SurfaceFactory` creates a `DetachedSurface` from a window handle on the thread the windowing backend requires, handed off `Send` to the render thread for `ConfiguredSurface`; alpha-mode policy (`resolve_alpha_mode`, `select_surface_format`, `surface_config`) is pure functions over plain values, so it is host-testable with neither a GPU nor a window server; every swapchain is `RENDER_ATTACHMENT`-only |
| `frust-gpu::texture` | `TextureDesc`/`Texture`/`RenderTarget`/`Attachment` vocabulary; `TextureId` is a caller-assigned handle for an external binding, `SceneTextureId` is minted per-`Texture` off a process-wide atomic counter (the same mechanism `frust_scene::ShaderProgram::id` uses) and is what a future `TextureRegistry` resolves `Command::SceneTexture` against |
| `frust-gpu::shader` + `frust-gpu::pipeline` | `ShaderLibrary` compiles each WGSL module once at start-up behind an opaque `ShaderId`; `RenderPipelineDesc` packs its five render-state axes into a `u64` key so `PipelineCache::get_or_create` compiles a variant at most once — a request still queued on `PipelineCache::warm_up`'s worker is stolen and built inline rather than waited on (Impeller's eager-steal queue) |
| `frust-gpu::pipeline_cache` | Frames a persisted `wgpu::PipelineCache` blob with a magic tag plus adapter fingerprint before it reaches the unsafe `create_pipeline_cache`, rejecting a mismatched/foreign blob as "no cache"; the framing is kept byte-identical to `frust-render`'s own `pipeline_cache` module (same magic, header, adapter-key) so a blob persisted under one path validates under the other, and a drift-guard test enforces it |
| `frust-gpu::pool` + `frust-gpu::arena` | `TexturePool` recycles per-frame scratch textures keyed on a 256-px-quantized extent, aged out after 60 unused frames (not `frust-render`'s 2-frame compositor-scratch window, which is wrong for a size that keeps changing across a resize drag), applying `wgpu::TextureUsages::TRANSIENT` only where `TierCaps::transient_saves_memory` says it helps; `HostBuffer` bump-allocates every small per-frame GPU upload into one buffer, flushed with a single `write_buffer` and reset at frame start, growing geometrically and never shrinking |
| `frust-gpu::encoder` | `CommandBuffer` wraps one `wgpu::CommandEncoder` under a single-submit borrowing contract: a caller records render passes and staged uploads only, never submits, and holds no borrow past return (glyph-atlas uploads are the one sanctioned exception, submitting their own encoder ahead of the scene pass) |
| `frust-gpu::headless` | `HeadlessTarget` is an offscreen `RENDER_ATTACHMENT \| COPY_SRC` target with no swapchain, for engine-seam tests and tooling; `read_back` strips wgpu's mandatory row padding so an arbitrary width reads back exact |
| `frust-gpu::diag` | `TimestampRing` — real per-pass GPU time via `timestamp_writes` at pass-boundary only (never `write_timestamp` inside an encoder/pass, unsupported on tile-based mobile GPUs); inert (`gpu_q=0`) unless the device actually carries `wgpu::Features::TIMESTAMP_QUERY` (a `perf-trace` build only, and only when the adapter offers it); a ring of independent slots so nothing on the frame path ever blocks on a map, and a refused frame calls `abandon_frame` rather than mapping its slot |
| `frust-gpu::lint` | WGSL-directory and pipeline-layout-descriptor scans enforcing the engine's downlevel design rules (E1–E18: no compute, no storage buffers/textures, uniform/bind-group/vertex-attribute ceilings) as design-rule tripwires, run against `frust-engine`'s own shader directory; plus `check_limits_against_webgl2` for adapter-limit conformance |

### Design Rule

`TierCaps::probe` is the only place a live `wgpu::Adapter` is ever consulted. Every policy decision
built on top of it — tier/pipeline/pool/surface choices alike — is a pure function over the plain
`TierCaps` value, so it is unit-testable via `TierCaps::fake` with no GPU in the loop. `context` and
`surface` follow the identical pure-decision/platform-lookup split: a small set of named,
platform-gated lookups (`is_android_emulator`, `is_ios_simulator`) answer what the environment is,
and everything downstream of them is a plain-value decision.

### Module Structure (`frust-engine`)

| Module | Responsibility |
|--------|-----------------|
| `frust-engine::compile` | `SceneCompiler` is a stateless walk over `frust_scene::Scene::commands` producing a `CompiledFrame` (strips, draws, encoded paints, LUT requests); `dash_path`/`well_formed`/`dash_cycle_is_normalizable` are public only as a cross-crate parity seam — `frust-testing` pins its CPU-oracle copies against them; a fast-rect path writes strip coverage directly for an axis-aligned, pixel-aligned rectangle, a dash pattern is pre-expanded to a plain path before stroking, and `DepthCounter` hands out the frame's monotonic painter-order depths; `GlyphRun` compiles through `compile_glyph_run` (see `frust-engine::text` below), and only `ShaderQuad` remains recognised-but-skipped — every other command, including a clip/layer/snapshot/clear bracket's own rectangle, radii, alpha and scale, is refused up front by `check_geometry` on the same terms as a draw's geometry |
| `frust-engine::compile::clip` | `ClipStack` lowers every `PushClip`/`PushClipRounded` to one of two shapes on one stack, never to an intermediate texture: a rectangle `fast_rect`'s axis-aligned, pixel-aligned rule admits becomes a `RectU16` scissor intersected into the enclosing one and applied by rewriting each draw's own strip coverage after generation; every other clip rasterizes once into a coverage mask through `vello_common::clip::ClipContext`, whose own nesting supplies the intersection; an unbalanced `PopClip` is ignored rather than underflowing either stack |
| `frust-engine::compile::layers` + `clear` | One `GroupStack` bracket stack serves `PushClip`/`PushLayer`/`PushSnapshot` alike, so whichever `Pop*` arrives closes the innermost open bracket; a `PushLayer` at `alpha >= 1.0` lowers to its rectangle's clip alone, below that it also records an isolated layer for the scheduler; a `PushSnapshot` bracket takes the display list's own inline emulation — a presentation scale composed as a correction ahead of every inner command's transform, sub-unity alpha as a nested layer — with only the outermost bracket's parameters honoured; `ClearRect` is hoisted past every open bracket to the frame root as a `ClearPunch` in `CompiledFrame::clears`, kept outside `draws()` so a target that disregards alpha can drop the punch pass and read the frame unchanged |
| `frust-engine::cache` | `GradientCache` keys ramp residency by a gradient's colour-affecting properties (stops, colour space, hue direction), not its geometry, so two placements of the same gradient share one entry; ramps live in one packed `Rgba8Unorm` byte buffer compacted on LRU eviction, each cached ramp naming its record by texel offset |
| `frust-engine::text` + `text::backend`/`atlas_policy`/`color` | `lower_glyph_run` drives one `glifo::GlyphRunBuilder` per `GlyphRun` through `EngineTextBackend`/`EngineGlyphSink`, `glifo`'s own backend/sink pair, built over the compiler's `StripGenerator`; `atlas_policy::AtlasPolicy` routes each run once per frame to `RunRoute::Atlas` (a settled run resolves through the glyph atlas and each glyph draws as one region-addressed image paint naming its slot) or `RunRoute::Outline`, one per glyph, for one of: an animating or oversized size — the guard and `glyph_entry_ceiling` both watch the run's *device*-space size (`font_size` with the transform's absorbed uniform scale), the same quantity `glifo` keys, never the display list's raw `font_size`; an unusable size; a transform `glifo` will not absorb into a positive uniform scale, refused outright because `glifo` probes its cache with the unabsorbed size before checking absorbability, so a stale hit would draw the wrong bitmap (e.g. unrotated) rather than refuse; a COLR-carrying face, refused before insertion because this tier cannot replay `glifo`'s recorded COLR command stream and `glifo` 0.3.0 has no way to withdraw an entry afterwards; glyph residency past its texel share — one running total, `resident_texels` (each entry charged at `glyph_texels_at` of its own size) against `glyph_texel_budget` (`GLYPH_ATLAS_SHARE` of the shared atlas array), asked at classify and again at route consumption, maintained as a deliberate per-entry over-estimate since `glifo` 0.3.0 prices no resident population outside a debug build; or `FRUST_ENGINE_NO_ATLAS`; the run's brush is encoded once and shared by every glyph in it; hinting is `hint(false)` (classic-tier parity) on a mobile `TierCaps`, on by `SceneCompiler::for_caps`'s device-class read otherwise; `font_is_readable` gates a run's face before `glifo` ever parses it, keeping `glifo`'s font-table `unwrap`s off the frame path (E17); `text::color` recombines a COLR glyph's clip/fill layer stream into one shape and brush per layer, dropping the whole glyph rather than half-painting it when a layer has no exact engine spelling |
| `frust-engine::cache::images` + `frust-engine::gpu::atlas` | `ImageResidency` keys an atlas rectangle on a `peniko::Blob`'s process-unique id over `vello_common`'s `ImageCache`/`MultiAtlasManager`, so the same decoded image reuses its rectangle across every frame it is drawn on; residency is budgeted mobile `(1024, 1024)` x4 layers or desktop `(2048, 2048)` x8 layers by `AtlasBudget::for_caps`, and a source over its layer's own extent is minified to fit with an aspect-preserving box filter rather than refused; an entry unseen for `MAX_UNSEEN_FRAMES` is reaped and its rectangle reported for clearing; residency is committed only once the frame's own eviction/upload plan is serviced (`ImageResidency::acknowledge_plan`), so a frame refused after compiling re-offers the same pending plan on the next one rather than recording an atlas rectangle resident with unwritten texels. The same `ImageCache` is also the *glyph* atlas's only allocator (`crate::text::atlas_policy`), one id space ruling out an image/glyph slot collision by construction; the acknowledge/re-offer contract covers the glyph atlas too — `SceneCompiler::acknowledge_glyph_clears`/`acknowledge_glyph_replay` mirror `ImageResidency`'s plan/acknowledge shape, so a frame compiled but never encoded loses neither its pending clears nor its pending replay — and `glifo`'s own eviction pass defers only once the oldest unreplayed recording is older than the entry-age window (serial-based, not on every outstanding replay), so a recorded command can never outlive the slot it names. `AtlasRenderer` replays `glifo`'s recorded glyph-page commands into each dirty layer on its own `wgpu::CommandEncoder`, submitted strictly ahead of the scene pass — the sanctioned single-submit exception `frust-gpu::encoder` already names, now actually exercised by the frame path. `AtlasArray` owns one `Rgba8Unorm` `D2Array` texture the strip shader samples, growing it a layer at a time (never shrinking) and tracking growth with a generation counter a caller can compare against |
| `frust-engine::gpu` | The GPU-side data layouts the strip shaders read, byte-compatible with the sparse-strip reference renderer's own layouts: `GpuStrip` (the per-instance quad), `GpuEncodedPaint` family (gradient/image/blur records in the encoded-paint texture), and `GpuConfig` (the per-draw uniform block); sizing/addressing/packing are pure functions over plain values, returning `EngineError` where the reference asserts |
| `frust-engine::gpu::present` | `UnpremultiplyPass` — the crate's only pass writing a swapchain format directly, host-driven at present time: every engine pipeline blends and writes premultiplied alpha, so a straight-alpha swapchain (a non-Metal `PostMultiplied` surface — see Data Flow) needs one fragment-only conversion pass on the way out; a `Rgba8Unorm`/premultiplied-expecting swapchain skips it entirely |
| `frust-engine::diag` | `EngineSpan` names the four GPU spans one engine frame splits into (`Prepass`/`Main`/`Composite`/`Blit`) — the only place that naming lives, so `frust-gpu`'s substrate ring never has to know what an index means; `LABEL_PREFIX` labels every GPU object the crate creates for capture tooling |
| `frust-engine::gpu::pipelines` + `shaders/` | Nine pipelines (`EnginePipeline::ALL`) — six strip variants (intermediate, alpha, depth-tested alpha, opaque, and a destination-out pair the hole-punch pass draws with) differing only in target/blend/depth state, plus clear and copy — each a plain `frust_gpu::RenderPipelineDesc` warmed up before any frame needs it; the glyph-atlas render pass draws through `ATLAS_STRIP_PIPELINE`, which is `StripIntermediate` itself under a second name rather than its own pipeline object; the ninth is `EnginePipeline::Filter` — its own WGSL program, an 8-attribute/32-byte instance layout, two bind groups (the filter-data texture; the source page plus a sampler), writing `INTERMEDIATE_FORMAT` unblended and undepthed — and the engine's first sampler (bilinear, clamp-to-edge, no mip chain), which the filter kernels alone read a texture through rather than `textureLoad`; WGSL is ported with hand-inlined imports (no WESL resolver at this MSRV) and stays within the four-bind-group WebGL2 ceiling |
| `frust-engine::renderer` | `EngineRenderer::new`/`encode`/`encode_traced`/`resize`/`end_frame`/`bind_texture`/`unbind_texture` — the public seam a host drives one surface's frames through; `encode` is `encode_traced` with an inert timestamp sink (`FrameTimestamps::inert`), so a build with no `perf-trace`/no `TIMESTAMP_QUERY` records byte-identical work either way — `encode_traced` additionally asks its sink for each pass's `timestamp_writes`, charged to the pass's [`EngineSpan`](crate::diag::EngineSpan). `encode` schedules the compiled frame into rounds and records clear→opaque(depth, hoisted once ahead of every round rather than re-run per round)→each round's own pass (a pooled page per isolated layer, a finished page composited into its parent as one instanced quad, painter-order depth carried per composite; a wide sibling fan or a page-group shortage [cuts](crate::schedule::cut_at) an open round early rather than escalating), with a surface round additionally cut at each recorded `ClearRect`'s own painter-order depth so its destination-out punch pass lands between the ops before and after it, never merely trailing (a punch past every op of the frame still lands last) — into the caller's own `wgpu::CommandEncoder`, never submitting it, and leaving no pass open on return; the punch pass is skipped whole on a target that disregards alpha; with no depth attachment available the two draw passes collapse into one blended painter-order pass, a correctness requirement rather than a fallback. A filter round draws no strip: it runs one instanced quad through `EnginePipeline::Filter` for one pass of the layer's filter sequence, recorded into that same caller-owned encoder in scheduled order — an ordinary round in every respect but its pipeline, and explicitly not a third own-encoder carve-out. Two carve-outs to "never submits", both strictly ahead of the scene pass: growing the image atlas array submits one copy-only maintenance command buffer of its own, ahead of the frame's own writes — `wgpu` flushes a submit's queued writes before that submit's command buffers, so the growth copy must precede the frame's atlas uploads rather than ride inside them — and, when the glyph atlas has pending work, `AtlasRenderer` replays each dirty page's recorded commands on its own encoder, one submit per dirty page (see `frust-engine::cache::images` + `gpu::atlas` above) |
| `frust-engine::filters` + `filters::blur`/`drop_shadow` | Pure kernel/pass-planning data for the two filters the engine renders: `blur::blur_passes` plans a Gaussian blur as `n_decimations` downscales, one `BlurH`/`BlurV` pair at the decimated size, then the matching upscales — always an even step count, checked rather than assumed, so the result lands back in the page the layer's contents were rendered into; `drop_shadow::drop_shadow_passes` wraps that same sequence in a leading `Offset` and a trailing `Colorize` pass. `LayerFilter::Blur`/`LayerFilter::DropShadow` is engine-internal vocabulary `push_filter_layer` opens a layer with — the counterpart of `CommandRecorder::push_layer` for a filtered one — deliberately with no `frust_scene` command of its own, since the scene seam for a filter layer is a later plan; this module proves the engine can render one. The device-side halves live beside every other engine pipeline's: the WGSL program is three preludes assembled by `gpu::shader_src::FILTER` (`HELPERS`, `FILTER_KERNELS`, `DROP_SHADOW_KERNELS`), the pipeline is `gpu::pipelines::EnginePipeline::Filter`, and the frame's parameter blocks plus the bilinear sampler the kernels read through are `gpu::targets`' |
| `frust-engine::schedule` + `schedule::pages` | `Schedule::build` turns a recording into `Vec<Round>`, generally innermost isolated layer first and the frame's own surface last — round cutting can push a cut Root round out ahead of a later page round — bottom-up over a chain of at most `MAX_CHAIN_DEPTH` (4) nested isolated layers; each layer's page comes from one of two ping-pong groups keyed on its depth's parity, plus one bounded spill page (`PageParity::Spill`) a *regular* layer falls back to instead of escalating once both groups are already holding a page a later round composites — a cut never takes it and neither does a filter layer, so it is a narrowing of the refused set rather than a step toward the general scheduler — bounding any served shape to `MAX_LIVE_PAGES` (3) live intermediate pages at once. Served: a chain of nested isolated layers of any depth up to the bound; a sibling fan of any width under one parent — a page-group-hungry sibling cuts the parent's open round early (round cutting: the ops so far emit as a round of their own, both pages return to the pool, the next sibling takes one), and same-parity sequential reuse of one layer's own page loads rather than clears it (`PageTarget::continued`) across a cut; a chain hanging off an isolated ancestor's later child (the navigation-scrub shape the spill page exists for), served on that one spill page; and a single-primitive Gaussian blur or shadow-only drop-shadow [filter](crate::filters) layer recorded directly under the frame's own surface, planned as its contents' round plus one round per pass of the filter's sequence (`filter_rounds`), holding both page groups for the length of that sequence. Still refused with `SchedulerEscalation { reason }`: a chain past `MAX_CHAIN_DEPTH`; a layer whose own round would need to sample two live pages while a third is still owed to a round above it — a fourth live page, past what the one bounded spill extends to; a filter kind other than a single Gaussian blur or shadow-only drop shadow (a flood, a standalone offset, and a drop shadow that composites the original back over itself are reserved but not served); a filter layer recorded inside another layer; a non-default blend mode; and a mask/layer clip path. A filter layer's page is sized by `filter_page_size` — coverage grown by `FILTER_ATLAS_PADDING` twice per axis with all of the growth landing on the far side (the layer renders at the page's own origin, so the near side carries no margin and the kernels bound their own taps instead), quantized to the substrate pool's grid (E13), then checked against two ceilings separately: the adapter's `max_texture_size` refuses `IntermediateTextureTooLarge`, the pool's own configured budget refuses `IntermediateTextureLimitReached`. A refusal is never rendered wrong and is never a route to a second renderer — the engine tier carries none; the caller treats it as a skipped frame. The general algorithm this narrows from is a stub behind the `full-scheduler` feature |
| `frust-engine::gpu::depth` | `DepthAttachment` is `Depth24Plus`, either caller-supplied (paired with `set_depth_pre_cleared` so the frame loads rather than clears a populated buffer) or engine-owned and lazily allocated, reallocated only when the target extent changes |
| `frust-engine::gpu::targets` | A per-renderer `IntermediateTargets` pool for off-screen layers/scratch copies, capping any request at `min(adapter max, 8192)` and answering an over-ceiling request with `IntermediateTexture::TooLarge` rather than a device error |

`tests/structure.rs` is a host-only structural guard, no GPU device or adapter created: G1 runs `frust-gpu`'s WGSL-directory lint against `frust-engine`'s shipped shaders, G2 checks the engine's downlevel limits profile against the WebGL2 ceiling, and E17 greps `cache/`, `compile/`, `filters/`, `gpu/`, `text/`, `schedule/` and `renderer.rs` for a bare `unwrap`/`expect`/`panic!` outside test code.

## Layer Dependencies

Both crates depend on `frust-scene` (CORE) for the `Scene`/`Command`/`GlyphRun` types — the stable
widget↔GPU seam neither crate may bypass. `frust-render` additionally depends on `vello` and
`wgpu`, and optionally on `vello_cpu` (exact-pinned) behind its non-default `cpu-tier` feature, and
optionally on `frust-engine`/`frust-gpu` (workspace-internal, unpinned) behind its equally
non-default `engine-tier` feature; `frust-text` depends on `parley`. `kurbo` and `peniko` supply the
geometry/color vocabulary shared across both crates' public APIs and `frust-scene`'s. `frust-render`
also depends on `android_system_properties` on Android.

`frust-render` confines every `vello`/`wgpu` type behind its own API except at three deliberate
seams: `encode_scene` exposes a raw `vello::Scene` for a shell driving its own renderer, and
`SurfaceRenderer::on_surface_created` and `SurfaceFactory::create_detached_surface` each take a raw
`wgpu::SurfaceTarget` (the render-thread split's surface-creation entry points — a shell must hand
over a window either way). Beyond those three, the only two opaque wgpu wrappers that ever leave
the crate are `DetachedSurface` and `DeferredPresent`, used for cross-thread/cross-transaction
handoff — a bare `wgpu::Surface` or `wgpu::Device` never does. The
optional `cpu-tier` path is isolated behind the same `SceneSink` encode seam as the GPU path, so a
breaking `vello_cpu` bump cannot reach the default GPU path; the optional `engine-tier` path is
isolated a different way — its own `TierBackend::Engine` arm in `renderer.rs`, sharing neither
`convert.rs`'s command walk nor a `SceneSink` — so a breaking `frust-engine`/`frust-gpu` bump
likewise cannot reach the default GPU path. `frust-text` mirrors this: `parley` never appears outside
`TextContext`/`TextStyle`/`TextLayout`, and `TextEditor` is the sole owner of UTF-16↔byte index
conversion — everything else in the crate works in byte offsets.

The scene-layer purity boundary these confinement rules enforce against `frust-scene`/`frust-core`
is a cross-unit rule; see [ARCHITECTURE.md](ARCHITECTURE.md).

`frust-gpu` sits outside that graph: it depends on `wgpu`/`kurbo`/`peniko` only, not on
`frust-scene`/`frust-render`/`frust-text`. `frust-engine` depends on `frust-gpu` + `frust-scene`,
plus `glifo` (exact-pinned) for glyph outline fetch/scale/cache — `frust-engine::text` is its sole
adapter onto that dependency (see above); nothing above RENDER depends on any of the three — the
same engine-tier boundary [ARCHITECTURE.md](ARCHITECTURE.md) records at the cross-unit level.

## Data Flow

- `frust_scene::Scene` → `encode_scene` → `vello::Scene` → `SurfaceRenderer::encode()`/`present()`,
  direct-to-surface when supported else an intermediate-texture blit, driven from a dedicated
  render thread by default.
- Third render path, `engine-tier` builds only (`RenderTier::Engine`, override-only — see Key
  Types): `context::choose_engine_render_path` resolves every (backend, alpha mode) pair to one of
  two arms — the engine tier refuses no surface's translucency. `RenderPathKind::EngineDirect`
  serves `Opaque`/`Auto`, Android's `Inherit`/`PreMultiplied` (the engine's strip pipelines already
  write premultiplied alpha, so these need no conversion pass), and `PostMultiplied` on
  `wgpu::Backend::Metal` alone: Core Animation's `CAMetalLayer` composites premultiplied
  regardless of the mode's advertised name, an upstream wgpu-hal truth bug
  (`compositor_expects_premultiplied`; `engine-metal-postmultiplied-truth-bug` in LIMITATIONS.md),
  so handing Metal's `PostMultiplied` swapchain (iOS's sole translucent mode) the spec-correct
  straight-alpha conversion would double-correct it. Every other backend's genuinely-straight
  `PostMultiplied` (e.g. Vulkan's own flag) takes `RenderPathKind::EngineDirectUnpremultiply`
  instead: the frame renders into an intermediate and
  `gpu::present::UnpremultiplyPass` converts it into the swapchain on the way out.
  `EngineDirect` renders straight into the acquired swapchain view, `RENDER_ATTACHMENT` only in
  whatever format the surface reports — no intermediate, no blit, plus a `Depth24Plus` attachment
  configured alongside the swapchain; `tier_forces_blit` excludes `Engine` for that reason.
  `TierBackend::Engine` wires `EngineRenderer::new` from a `frust_gpu::TierCaps::probe` of the live
  adapter, the surface's own configured format, and the same persisted `PipelineCache` handed to
  vello, so a re-warm after a format change hits the driver cache rather than cold-compiling.
  `submit`'s `EngineDirect` arm records the whole frame — `engine.encode` (or, under `perf-trace`,
  `engine.encode_traced` threading a borrowed `frust_gpu::diag::TimestampRing` sink) into one
  caller-owned `wgpu::CommandEncoder`, one `queue.submit`, then `engine.end_frame` — never opening a
  second encoder; an `EngineError` from `encode` leaves that encoder untouched, so nothing is
  submitted and the acquired texture is dropped rather than presented: the frame is
  `FrameOutcome::Skipped`, a `refused_frames` counter increments, `log_engine_refusal` logs through
  the same rate-limited `decide_log_action` latch every other refusal path uses, and the previous
  frame's swapchain content simply persists on screen. A refused frame recorded no pass, so it took
  no timestamp either — the host abandons that frame's ring slot rather than mapping it. There is no
  fallback renderer on this tier — a refused frame is never retried through vello or the CPU tier;
  which renderer draws a surface is decided once at configure time, not per frame.
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
- Optional engine-tier: `RenderTier::Engine` is selectable only by an explicit override
  (`FRUST_RENDER_TIER=engine` / `frust run --render-tier engine`, see
  [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) — `select_render_tier` never returns it from a
  probe, since `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` is deliberately empty (`tier.rs`): the tier needs
  neither `COMPUTE_SHADERS` nor `INDIRECT_EXECUTION`, the two flags the GPU tier requires and the
  iOS Simulator's Apple2 GPU family lacks, so it runs on exactly the adapters the GPU tier refuses.
  An override onto a build without the `engine-tier` feature is refused with a diagnosis naming the
  missing feature, never silently rendered through vello under an engine label.

## Key Types

| Type | Purpose |
|------|---------|
| `RenderContext` / `SurfaceRenderer` / `SurfaceFactory` / `DetachedSurface` / `DeferredPresent` | Device ownership, surface lifecycle/present, and the two sanctioned opaque wgpu wrappers for cross-thread handoff |
| `HeadlessRenderer` | Offscreen classic renderer reused across renders (one adapter/device/`vello::Renderer`); verifies its resolved adapter/backend against an expectation before rendering and returns plain RGBA8 bytes, never a `vello`/`wgpu` type |
| `RenderTier` / `TierCaps` | GPU-vs-CPU-vs-Engine render-backend selection, probed from adapter capabilities plus an override; `Engine` is override-only (see Data Flow) |
| `SurfacePhase` / `FrameOutcome` / `EncodeOutcome` / `AcquireOutcome` | The surface-can-be-destroyed-anytime lifecycle state machine shared by every shell |
| `encode_scene` | The sole function converting a `frust_scene::Scene` into a `vello::Scene` |
| `CompositeLayer` / `FramePlan` | One cached page's placement/texture for the compositor to draw, and the frame's vello-pass/compositor-layer/hole split those cached pages imply (see Data Flow) |
| `CornerRadii` / `DashPattern` (`frust-scene`) | Per-corner rounding and dash geometry carried by `RoundedRect`/`PushClipRounded`/`BlurredRoundedRect`/dashed-stroke commands; lowered to backend shapes in the shared command walk (see Data Flow) |
| `TextContext` / `TextStyle` / `TextLayout` / `TextOverflow` | Renderer-agnostic shaping surface: font/cache state, styling knobs, a finished measurable shaped block, and `layout_bounded`'s measure-and-truncate overflow mode |
| `TextEditor` / `EditingState` / `EditOp` | The Parley-based editing engine and its state-sync payload at the platform IME seam |
