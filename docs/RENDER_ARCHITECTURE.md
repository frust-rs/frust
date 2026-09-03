# Frust - RENDER Architecture

## Overview

RENDER covers two crates that turn a display list and styled text into pixels. `frust-render` is
the wgpu GPU backend that presents the renderer-agnostic `frust-scene` display list to a window
surface, driving the frust-owned `frust-engine` strip renderer (the only renderer this crate
contains — see GPU Substrate below) and owning the render-path decision, the engine's adapter
capability gate, and per-surface fragment-shader effects. `frust-text` wraps Parley font
matching/shaping into a renderer-agnostic API (`TextContext`/`TextStyle`/`TextLayout`) producing
`frust-scene` `GlyphRun`s, plus the `TextEditor` engine the platform IME bridges drive. Both crates
keep their heavy engine dependencies (`wgpu`, `parley`) out of their public surface so the GPU and
text backends stay swappable.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how RENDER relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-render::context` | The render-path decision (`choose_engine_render_path` over `RenderPathKind::{EngineDirect, EngineDirectUnpremultiply}`), the engine's per-surface resources (depth attachment; on the unpremultiply arm, the intermediate + `UnpremultiplyPass`), and the engine's capability check ahead of surface creation. The device/surface foundation itself (`RenderContext`, `SurfaceFactory`/`DetachedSurface`) is `frust-gpu`'s, re-exported here (see GPU Substrate below) |
| `frust-render::renderer` | `SurfaceRenderer` drives per-frame encode/present on a dedicated render thread by default; `DeferredPresent` lets iOS present inside a platform-view transaction |
| `frust-render::tier` | `engine_support` checks a real adapter's downlevel flags against `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` (deliberately empty) and refuses one that cannot run the engine — the only renderer this crate contains, so there is no tier to select between |
| `frust-render::headless` | `HeadlessRenderer`, an offscreen `frust-engine` renderer for goldens/oracles: env-aware adapter resolution (`WGPU_ADAPTER_NAME`/`WGPU_BACKEND`), an expect-adapter/expect-backend fail-fast before any pixel is produced, row-padded readback, and the same capability gate the live surface path resolves |
| `frust-render::external_pass` | The reachable half of the GPU Seam (see below): a process-wide registry of caller-supplied `ExternalPass`es, each handed the live frame's device/queue/encoder (`ExternalFrame`) once per frame, ahead of the scene pass and its shader-quad pre-pass, into the same encoder the scene then records into; drained only by `SurfaceRenderer::submit_impl` on the engine tier, never by `HeadlessRenderer` |
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
| `frust-gpu::context` | `RenderContext` owns one `wgpu::Instance` and creates its one logical `DeviceHandle` lazily on first surface creation (`create_render_surface`) or an explicit `device()`/`ensure_device_headless` pre-init, then reuses it (a logical device is display-independent and must survive surface loss/recreation); requests `wgpu::Features::PIPELINE_CACHE` opportunistically (`optional_device_features` — narrowed to what the adapter actually advertises, never a hard requirement); `DeviceHandle` is cheap to clone since wgpu's `Adapter`/`Device`/`Queue` are themselves `Arc`-backed handles to the same underlying device; instance flags strip `DEBUG` on the Android emulator, which cannot survive it; a device request on the iOS Simulator is forced back up to a 256-byte uniform alignment since the Simulator misreports its own (`effective_limits`, kept until upstream [gfx-rs/wgpu#10189](https://github.com/gfx-rs/wgpu/pull/10189) ships); an installed `on_uncaptured_error` handler latches the first error into `DeviceHandle::first_uncaptured_error` (never overwritten) while logging a bounded, periodically-resumed count of the rest |
| `frust-gpu::lifecycle` | `SurfacePhase`/`AcquireOutcome`/`FrameOutcome` model a surface Android can destroy and recreate at any time (rotation, backgrounding), driven by shell lifecycle callbacks; also holds the crate's two sanctioned-`unsafe` surface constructors, `create_android_surface` (raw `ANativeWindow*`) and `create_metal_surface` (raw `CAMetalLayer*`), each isolating its safety contract in one function |
| `frust-gpu::surface` | `SurfaceFactory` creates a `DetachedSurface` from a window handle on the thread the windowing backend requires, handed off `Send` to the render thread for `ConfiguredSurface`; alpha-mode policy (`resolve_alpha_mode`, `select_surface_format` — the surface's own first-reported supported format, `surface_config`) is pure functions over plain values, so it is host-testable with neither a GPU nor a window server; every swapchain is `RENDER_ATTACHMENT`-only. Also owns the two platform-fact predicates `frust-render::context` routes on: `alpha_mode_is_straight_translucent` and `compositor_expects_premultiplied` (the Metal `PostMultiplied` truth-bug carve-out, see Data Flow) |
| `frust-gpu::texture` | `TextureDesc`/`Texture`/`RenderTarget`/`Attachment` vocabulary; `TextureId` is a caller-assigned handle for an external binding, `SceneTextureId` is minted per-`Texture` off a process-wide atomic counter (the same mechanism `frust_scene::ShaderProgram::id` uses) and is what `frust-engine`'s `gpu::bindings::ExternalTextures` registry resolves `Command::SceneTexture`/`Command::ShaderQuad` against; `SceneTextureId::for_shader_program` mints a distinct id space by reserving the top bit, so a shader-effect target's id can never collide with an ordinary mint |
| `frust-gpu::shader` + `frust-gpu::pipeline` | `ShaderLibrary` compiles each WGSL module once at start-up behind an opaque `ShaderId`; `RenderPipelineDesc` packs its five render-state axes into a `u64` key so `PipelineCache::get_or_create` compiles a variant at most once — a request still queued on `PipelineCache::warm_up`'s worker is stolen and built inline rather than waited on (Impeller's eager-steal queue) |
| `frust-gpu::pipeline_cache` | Frames a persisted `wgpu::PipelineCache` blob with a magic tag plus adapter fingerprint before it reaches the unsafe `create_pipeline_cache`, rejecting a mismatched/foreign blob as "no cache"; the only copy of this framing (`frust-render` used to carry an identical one, kept in lockstep by a drift-guard test, and now re-exports this module instead) |
| `frust-gpu::pool` + `frust-gpu::arena` | `TexturePool` recycles per-frame scratch textures keyed on a 256-px-quantized extent, aged out after 60 unused frames — a size that keeps changing across a resize drag needs a longer window than a 2-frame one would give it — applying `wgpu::TextureUsages::TRANSIENT` only where `TierCaps::transient_saves_memory` says it helps; `HostBuffer` bump-allocates every small per-frame GPU upload into one buffer, flushed with a single `write_buffer` and reset at frame start, growing geometrically and never shrinking |
| `frust-gpu::encoder` | `CommandBuffer` wraps one `wgpu::CommandEncoder` under a single-submit borrowing contract: a caller records render passes and staged uploads only, never submits, and holds no borrow past return (glyph-atlas uploads are the one sanctioned exception, submitting their own encoder ahead of the scene pass). Two rules govern a caller sharing this encoder with the engine (see GPU seam below): (1) depth-clear ownership — whichever pass records first clears the shared depth attachment and every later pass loads it, with comparison (`LessEqual`), far plane (`1.0`) and extent agreed between the two; colour is not shared the same way, since the frame clears its own colour target unconditionally, so content that must stay visible is recorded after the frame rather than before it; (2) atlas uploads may submit their own encoder ahead of the scene pass — the one sanctioned exception to "never submits" |
| `frust-gpu::effects` | `ShaderEffects` — the GPU half of the shader-showcase feature: lazy per-program pipeline compilation seeded from the surface's persisted `wgpu::PipelineCache`, per-`(program, size)` offscreen target state, fullscreen-triangle pass encoding, and age-based reap. The size-clamp/quad-placement policy that decides which `(id, size)` pairs a frame asks for is `frust-engine::effects::shader_quad`'s (see Module Structure (`frust-engine`) below) |
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
| `frust-engine::compile` | `SceneCompiler` is a stateless walk over `frust_scene::Scene::commands` producing a `CompiledFrame` (strips, draws, encoded paints, LUT requests); `dash_path`/`well_formed`/`dash_cycle_is_normalizable` are public only as a cross-crate parity seam — `frust-testing` pins its CPU-oracle copies against them; a fast-rect path writes strip coverage directly for an axis-aligned, pixel-aligned rectangle, a dash pattern is pre-expanded to a plain path before stroking, and `DepthCounter` hands out the frame's monotonic painter-order depths; `GlyphRun` compiles through `compile_glyph_run` (see `frust-engine::text` below). Every command now compiles: `Command::SceneTexture` and `Command::ShaderQuad` both lower through `compile::external`'s external-texture path (see below) rather than being skipped, and a clip/layer/snapshot/clear bracket's own rectangle, radii, alpha and scale is refused up front by `check_geometry` on the same terms as a draw's geometry |
| `frust-engine::compile::external` | `ExternalExtents` tracks the texel size the host registered for each externally bound id, mirroring the renderer's own texture registry (see `gpu::bindings` below); `encode_scene_texture` composes `Command::SceneTexture`'s (and `Command::ShaderQuad`'s) natural-pixels-onto-`dest` mapping the same way `Command::Image` does and always marks the entry possibly-transparent — an external paint is never claimed opaque, so it always blends rather than occludes through the depth-writing pass; an unregistered id draws nothing and is warned once per id, debug-logged thereafter (`ExternalSkip::Unregistered`) |
| `frust-engine::compile::clip` | `ClipStack` lowers every `PushClip`/`PushClipRounded` to one of two shapes on one stack, never to an intermediate texture: a rectangle `fast_rect`'s axis-aligned, pixel-aligned rule admits becomes a `RectU16` scissor intersected into the enclosing one and applied by rewriting each draw's own strip coverage after generation; every other clip rasterizes once into a coverage mask through `vello_common::clip::ClipContext`, whose own nesting supplies the intersection; an unbalanced `PopClip` is ignored rather than underflowing either stack |
| `frust-engine::compile::layers` + `clear` | One `GroupStack` bracket stack serves `PushClip`/`PushLayer`/`PushSnapshot` alike, so whichever `Pop*` arrives closes the innermost open bracket; a `PushLayer` at `alpha >= 1.0` lowers to its rectangle's clip alone, below that it also records an isolated layer for the scheduler; a `PushSnapshot` bracket takes the display list's own inline emulation — a presentation scale composed as a correction ahead of every inner command's transform, sub-unity alpha as a nested layer — with only the outermost bracket's parameters honoured; `ClearRect` is hoisted past every open bracket to the frame root as a `ClearPunch` in `CompiledFrame::clears`, kept outside `draws()` so a target that disregards alpha can drop the punch pass and read the frame unchanged |
| `frust-engine::cache` | `GradientCache` keys ramp residency by a gradient's colour-affecting properties (stops, colour space, hue direction), not its geometry, so two placements of the same gradient share one entry; ramps live in one packed `Rgba8Unorm` byte buffer compacted on LRU eviction, each cached ramp naming its record by texel offset |
| `frust-engine::text` + `text::backend`/`atlas_policy`/`color` | `lower_glyph_run` drives one `glifo::GlyphRunBuilder` per `GlyphRun` through `EngineTextBackend`/`EngineGlyphSink`, `glifo`'s own backend/sink pair, built over the compiler's `StripGenerator`; `atlas_policy::AtlasPolicy` routes each run once per frame to `RunRoute::Atlas` (a settled run resolves through the glyph atlas and each glyph draws as one region-addressed image paint naming its slot) or `RunRoute::Outline`, one per glyph, for one of: an animating or oversized size — the guard and `glyph_entry_ceiling` both watch the run's *device*-space size (`font_size` with the transform's absorbed uniform scale), the same quantity `glifo` keys, never the display list's raw `font_size`; an unusable size; a transform `glifo` will not absorb into a positive uniform scale, refused outright because `glifo` probes its cache with the unabsorbed size before checking absorbability, so a stale hit would draw the wrong bitmap (e.g. unrotated) rather than refuse; a COLR-carrying face, refused before insertion because this tier cannot replay `glifo`'s recorded COLR command stream and `glifo` 0.3.0 has no way to withdraw an entry afterwards; glyph residency past its texel share — one running total, `resident_texels` (each entry charged at `glyph_texels_at` of its own size) against `glyph_texel_budget` (`GLYPH_ATLAS_SHARE` of the shared atlas array), asked at classify and again at route consumption, maintained as a deliberate per-entry over-estimate since `glifo` 0.3.0 prices no resident population outside a debug build; or `FRUST_ENGINE_NO_ATLAS`; the run's brush is encoded once and shared by every glyph in it; hinting is `hint(false)` (classic-tier parity) on a mobile `TierCaps`, on by `SceneCompiler::for_caps`'s device-class read otherwise; `font_is_readable` gates a run's face before `glifo` ever parses it, keeping `glifo`'s font-table `unwrap`s off the frame path (E17); `text::color` recombines a COLR glyph's clip/fill layer stream into one shape and brush per layer, dropping the whole glyph rather than half-painting it when a layer has no exact engine spelling |
| `frust-engine::cache::images` + `frust-engine::gpu::atlas` | `ImageResidency` keys an atlas rectangle on a `peniko::Blob`'s process-unique id over `vello_common`'s `ImageCache`/`MultiAtlasManager`, so the same decoded image reuses its rectangle across every frame it is drawn on; residency is budgeted mobile `(1024, 1024)` x4 layers or desktop `(2048, 2048)` x8 layers by `AtlasBudget::for_caps`, and a source over its layer's own extent is minified to fit with an aspect-preserving box filter rather than refused; an entry unseen for `MAX_UNSEEN_FRAMES` is reaped and its rectangle reported for clearing; residency is committed only once the frame's own eviction/upload plan is serviced (`ImageResidency::acknowledge_plan`), so a frame refused after compiling re-offers the same pending plan on the next one rather than recording an atlas rectangle resident with unwritten texels. The same `ImageCache` is also the *glyph* atlas's only allocator (`crate::text::atlas_policy`), one id space ruling out an image/glyph slot collision by construction; the acknowledge/re-offer contract covers the glyph atlas too — `SceneCompiler::acknowledge_glyph_clears`/`acknowledge_glyph_replay` mirror `ImageResidency`'s plan/acknowledge shape, so a frame compiled but never encoded loses neither its pending clears nor its pending replay — and `glifo`'s own eviction pass defers only once the oldest unreplayed recording is older than the entry-age window (serial-based, not on every outstanding replay), so a recorded command can never outlive the slot it names. `AtlasRenderer` replays `glifo`'s recorded glyph-page commands into each dirty layer on its own `wgpu::CommandEncoder`, submitted strictly ahead of the scene pass — the sanctioned single-submit exception `frust-gpu::encoder` already names, now actually exercised by the frame path. `AtlasArray` owns one `Rgba8Unorm` `D2Array` texture the strip shader samples, growing it a layer at a time (never shrinking) and tracking growth with a generation counter a caller can compare against |
| `frust-engine::gpu::bindings` | `ExternalTextures<V>` is the registry of caller-owned texture views, keyed by the opaque `u64` id a display list names, written only through `EngineRenderer::bind_texture`/`unbind_texture`; `ExternalRuns` batches one frame's distinct external textures into slots so a pass splits its instances into maximal same-texture runs, re-binding group 1's texture at each run boundary |
| `frust-engine::gpu` | The GPU-side data layouts the strip shaders read, byte-compatible with the sparse-strip reference renderer's own layouts: `GpuStrip` (the per-instance quad), `GpuEncodedPaint` family (gradient/image/blur records in the encoded-paint texture), and `GpuConfig` (the per-draw uniform block); sizing/addressing/packing are pure functions over plain values, returning `EngineError` where the reference asserts |
| `frust-engine::gpu::present` | `UnpremultiplyPass` — the crate's only pass writing a swapchain format directly, host-driven at present time: every engine pipeline blends and writes premultiplied alpha, so a straight-alpha swapchain (a non-Metal `PostMultiplied` surface — see Data Flow) needs one fragment-only conversion pass on the way out; a `Rgba8Unorm`/premultiplied-expecting swapchain skips it entirely |
| `frust-engine::diag` | `EngineSpan` names the four GPU spans one engine frame splits into (`Prepass`/`Main`/`Composite`/`Blit`) — the only place that naming lives, so `frust-gpu`'s substrate ring never has to know what an index means; `LABEL_PREFIX` labels every GPU object the crate creates for capture tooling |
| `frust-engine::gpu::pipelines` + `shaders/` | Nine pipelines (`EnginePipeline::ALL`) — six strip variants (intermediate, alpha, depth-tested alpha, opaque, and a destination-out pair the hole-punch pass draws with) differing only in target/blend/depth state, plus clear and copy — each a plain `frust_gpu::RenderPipelineDesc` warmed up before any frame needs it; the glyph-atlas render pass draws through `ATLAS_STRIP_PIPELINE`, which is `StripIntermediate` itself under a second name rather than its own pipeline object; the ninth is `EnginePipeline::Filter` — its own WGSL program, an 8-attribute/32-byte instance layout, two bind groups (the filter-data texture; the source page plus a sampler), writing `INTERMEDIATE_FORMAT` unblended and undepthed — and the engine's first sampler (bilinear, clamp-to-edge, no mip chain), which the filter kernels alone read a texture through rather than `textureLoad`; WGSL is ported with hand-inlined imports (no WESL resolver at this MSRV) and stays within the four-bind-group WebGL2 ceiling |
| `frust-engine::renderer` | `EngineRenderer::new`/`encode`/`encode_traced`/`resize`/`end_frame`/`bind_texture`/`unbind_texture` — the public seam a host drives one surface's frames through; `bind_texture`/`unbind_texture` register/remove a caller-owned `wgpu::TextureView` under a `SceneTextureId` (`gpu::bindings::ExternalTextures`), and a pass splits into runs at each external-texture change, re-binding group 1 once per run rather than once per pass. The `Command::ShaderQuad` pre-pass (`effects::shader_quad::ShaderQuadPass`, see below) is the caller's (`frust-render`), not this module's: it records into the same `wgpu::CommandEncoder` ahead of calling `encode`/`encode_traced`, never a carve-out encoder of its own. `encode` is `encode_traced` with an inert timestamp sink (`FrameTimestamps::inert`), so a build with no `perf-trace`/no `TIMESTAMP_QUERY` records byte-identical work either way — `encode_traced` additionally asks its sink for each pass's `timestamp_writes`, charged to the pass's `diag::EngineSpan`. `encode` schedules the compiled frame into rounds and records clear→opaque(depth, hoisted once ahead of every round rather than re-run per round)→each round's own pass (a pooled page per isolated layer, a finished page composited into its parent as one instanced quad, painter-order depth carried per composite; a wide sibling fan or a page-group shortage cuts (`schedule::cut_at`) an open round early rather than escalating), with a surface round additionally cut at each recorded `ClearRect`'s own painter-order depth so its destination-out punch pass lands between the ops before and after it, never merely trailing (a punch past every op of the frame still lands last) — into the caller's own `wgpu::CommandEncoder`, never submitting it, and leaving no pass open on return; the punch pass is skipped whole on a target that disregards alpha; with no depth attachment available the two draw passes collapse into one blended painter-order pass, a correctness requirement rather than a fallback. A filter round draws no strip: it runs one instanced quad through `EnginePipeline::Filter` for one pass of the layer's filter sequence, recorded into that same caller-owned encoder in scheduled order — an ordinary round in every respect but its pipeline, and explicitly not a third own-encoder carve-out. Two carve-outs to "never submits", both strictly ahead of the scene pass: growing the image atlas array submits one copy-only maintenance command buffer of its own, ahead of the frame's own writes — `wgpu` flushes a submit's queued writes before that submit's command buffers, so the growth copy must precede the frame's atlas uploads rather than ride inside them — and, when the glyph atlas has pending work, `AtlasRenderer` replays each dirty page's recorded commands on its own encoder, one submit per dirty page (see `frust-engine::cache::images` + `gpu::atlas` above) |
| `frust-engine::effects::shader_quad` | `ShaderQuadPass::prepare` renders each frame's `Command::ShaderQuad` programs into pooled `frust_gpu::effects::ShaderEffects` targets as a pre-pass recorded into the caller's own frame encoder — the same encoder, ahead of the scene pass, not a separate submit — and registers each rendered target under `SceneTextureId::for_shader_program(id)` (the id space's reserved top bit) so the compiler lowers the quad through the same external-texture path `SceneTexture` takes; a target is sampled as premultiplied colour, and `FRUST_ENGINE_NO_SHADER_EFFECTS` (`config::shader_effects_disabled`) turns the whole path off |
| `frust-engine::filters` + `filters::blur`/`drop_shadow` | Pure kernel/pass-planning data for the two filters the engine renders: `blur::blur_passes` plans a Gaussian blur as `n_decimations` downscales, one `BlurH`/`BlurV` pair at the decimated size, then the matching upscales — always an even step count, checked rather than assumed, so the result lands back in the page the layer's contents were rendered into; `drop_shadow::drop_shadow_passes` wraps that same sequence in a leading `Offset` and a trailing `Colorize` pass. `LayerFilter::Blur`/`LayerFilter::DropShadow` is engine-internal vocabulary `push_filter_layer` opens a layer with — the counterpart of `CommandRecorder::push_layer` for a filtered one — deliberately with no `frust_scene` command of its own, since the scene seam for a filter layer is a later plan; this module proves the engine can render one. The device-side halves live beside every other engine pipeline's: the WGSL program is three preludes assembled by `gpu::shader_src::FILTER` (`HELPERS`, `FILTER_KERNELS`, `DROP_SHADOW_KERNELS`), the pipeline is `gpu::pipelines::EnginePipeline::Filter`, and the frame's parameter blocks plus the bilinear sampler the kernels read through are `gpu::targets`' |
| `frust-engine::schedule` + `schedule::pages` | `Schedule::build` turns a recording into `Vec<Round>`, generally innermost isolated layer first and the frame's own surface last — round cutting can push a cut Root round out ahead of a later page round — bottom-up over a chain of at most `MAX_CHAIN_DEPTH` (4) nested isolated layers; each layer's page comes from one of two ping-pong groups keyed on its depth's parity, plus one bounded spill page (`PageParity::Spill`) a *regular* layer falls back to instead of escalating once both groups are already holding a page a later round composites — a cut never takes it and neither does a filter layer, so it is a narrowing of the refused set rather than a step toward the general scheduler — bounding any served shape to `MAX_LIVE_PAGES` (3) live intermediate pages at once. Served: a chain of nested isolated layers of any depth up to the bound; a sibling fan of any width under one parent — a page-group-hungry sibling cuts the parent's open round early (round cutting: the ops so far emit as a round of their own, both pages return to the pool, the next sibling takes one), and same-parity sequential reuse of one layer's own page loads rather than clears it (`PageTarget::continued`) across a cut; a chain hanging off an isolated ancestor's later child (the navigation-scrub shape the spill page exists for), served on that one spill page; and a single-primitive Gaussian blur or shadow-only drop-shadow filter layer (`frust-engine::filters`) recorded directly under the frame's own surface, planned as its contents' round plus one round per pass of the filter's sequence (`filter_rounds`), holding both page groups for the length of that sequence. Still refused with `SchedulerEscalation { reason }`: a chain past `MAX_CHAIN_DEPTH`; a layer whose own round would need to sample two live pages while a third is still owed to a round above it — a fourth live page, past what the one bounded spill extends to; a filter kind other than a single Gaussian blur or shadow-only drop shadow (a flood, a standalone offset, and a drop shadow that composites the original back over itself are reserved but not served); a filter layer recorded inside another layer; a non-default blend mode; and a mask/layer clip path. A filter layer's page is sized by `filter_page_size` — coverage grown by `FILTER_ATLAS_PADDING` twice per axis with all of the growth landing on the far side (the layer renders at the page's own origin, so the near side carries no margin and the kernels bound their own taps instead), quantized to the substrate pool's grid (E13), then checked against two ceilings separately: the adapter's `max_texture_size` refuses `IntermediateTextureTooLarge`, the pool's own configured budget refuses `IntermediateTextureLimitReached`. A refusal is never rendered wrong and is never a route to a second renderer — the engine tier carries none; the caller treats it as a skipped frame. The general algorithm this narrows from is a stub behind the `full-scheduler` feature |
| `frust-engine::gpu::depth` | `DepthAttachment` is `Depth24Plus`, either caller-supplied (paired with `set_depth_pre_cleared` so the frame loads rather than clears a populated buffer) or engine-owned and lazily allocated, reallocated only when the target extent changes |
| `frust-engine::gpu::targets` | A per-renderer `IntermediateTargets` pool for off-screen layers/scratch copies, capping any request at `min(adapter max, 8192)` and answering an over-ceiling request with `IntermediateTexture::TooLarge` rather than a device error |

`tests/structure.rs` is a host-only structural guard, no GPU device or adapter created: G1 runs `frust-gpu`'s WGSL-directory lint against `frust-engine`'s shipped shaders, G2 checks the engine's downlevel limits profile against the WebGL2 ceiling, and E17 greps `cache/`, `compile/`, `filters/`, `gpu/`, `text/`, `schedule/` and `renderer.rs` for a bare `unwrap`/`expect`/`panic!` outside test code.

### GPU Seam (`frust::gpu`)

The facade's non-default `gpu` feature (see [ARCHITECTURE.md](ARCHITECTURE.md)) re-exports the
GPU-substrate vocabulary flat as `frust::gpu` (`Context`/`Texture`/`TextureDesc`/`RenderTarget`/
`SceneTextureId`/`CommandBuffer`/`ShaderLibrary`/`RenderPipelineDesc`/`DeviceHandle`, plus
`ExternalPass`/`ExternalFrame`/`register_external_pass`/`unregister_external_pass`, see below) so an
app, or a future 3D-rendering crate sitting beside the facade, can reach the render engine's texture
seam without a direct `frust-render`/`frust-gpu` dependency. `frust::gpu::with_context` reads back the
shell's own live `DeviceHandle` through `frust-shell-common`'s process-wide install-once slot (see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)) rather than creating a second device; it answers
`None` until a shell has published one.

A pass records only into attachments it owns, so merely sharing the frame's encoder owes none of
`frust-gpu::encoder`'s caller rules above; every externally bound texture the engine draws —
`Command::SceneTexture` and `Command::ShaderQuad` alike — is always blended, never claimed opaque,
since the engine never reads the caller's texels to know. A host that goes further and also shares
the frame's *depth* attachment (`EngineTarget::depth` plus `EngineRenderer::set_depth_pre_cleared`,
proven by `frust-engine`'s `shared_encoder` test suite, not by `ExternalPass`) owes the depth-clear
rule above. 3D content belongs in a **separate crate** built over this seam (shared encoder, and, for
occlusion against the 2D frame, shared depth, `SceneTexture`), not inside `frust-engine` itself — the
shape Flutter settled on after removing Impeller Scene in favour of a standalone `flutter_scene` over
Flutter GPU.

`register_external_pass`/`unregister_external_pass` (`frust-render::external_pass`) make the
encoder-sharing half of that seam reachable today, not just theorized: a process-wide registry of
caller-supplied `ExternalPass`es, each handed the live frame's device/queue/encoder (`ExternalFrame`)
once per frame, ahead of the engine's own scene pass and its shader-quad pre-pass, into the very
`wgpu::CommandEncoder` the scene then records into and the renderer submits once — reachable from a
crate depending on `frust` alone (the `gpu` feature), with no direct `frust-render`/`frust-gpu` edge.
A pass renders into its own target, never the frame's — the frame's unconditional colour clear never
touches it — and binds that target under a `SceneTextureId` (`ExternalFrame::bind_texture`) for a
`Command::SceneTexture` naming the same id to composite. `ExternalFrame` exposes only the frame's
device, queue, encoder and frame index — never its depth view or `set_depth_pre_cleared` — so a pass
never shares the frame's depth attachment and never owes the depth-clear rule; its composite is
blended like every external texture (`engine-scene-texture-always-blended` in LIMITATIONS.md).

## Layer Dependencies

Both crates depend on `frust-scene` (CORE) for the `Scene`/`Command`/`GlyphRun` types — the stable
widget↔GPU seam neither crate may bypass. `frust-render` additionally depends on `wgpu`, and
unconditionally on `frust-engine` (the only renderer it contains) and `frust-gpu` (the device/surface
foundation, workspace-internal and unpinned like `frust-engine`) — neither is a cargo feature, so
every build of this crate carries both; `frust-text` depends on `parley`. `kurbo` and `peniko` supply
the geometry/color vocabulary shared across both crates' public APIs and `frust-scene`'s.
`frust-render` also depends on `android_system_properties` on Android.

`frust-render` confines every `wgpu` type behind its own API except three deliberate seams:
`SurfaceRenderer::on_surface_created` and `SurfaceFactory::create_detached_surface` (the latter
`frust-gpu`'s, re-exported here) each take a raw `wgpu::SurfaceTarget` (the render-thread split's
surface-creation entry points — a shell must hand over a window either way); `external_pass`'s
`ExternalFrame` is the third, and the one that points outward rather than in — it hands a
caller-registered `ExternalPass` the frame's own `wgpu::Device`/`Queue`/`CommandEncoder` so a pass can
record its own work into the same encoder ahead of the scene (see GPU Seam above), unconditional in
this crate's public API regardless of the `gpu` feature below. Beyond those three, the only two
opaque wgpu wrappers that ever leave the crate are `DetachedSurface` and `DeferredPresent`, used for
cross-thread/cross-transaction handoff — a bare `wgpu::Surface` or `wgpu::Device` never does.
`DeviceHandle` is a fourth named exception, but stays behind this crate's own `gpu` feature rather
than the unconditional re-export, so a default build gains no new public symbol from it (see GPU Seam
above).
`frust-text` mirrors this: `parley` never appears outside `TextContext`/`TextStyle`/
`TextLayout`, and `TextEditor` is the sole owner of UTF-16↔byte index conversion — everything else
in the crate works in byte offsets.

The scene-layer purity boundary these confinement rules enforce against `frust-scene`/`frust-core`
is a cross-unit rule; see [ARCHITECTURE.md](ARCHITECTURE.md).

`frust-gpu` sits outside that graph: it depends on `wgpu`/`kurbo`/`peniko` only, not on
`frust-scene`/`frust-render`/`frust-text`. `frust-engine` depends on `frust-gpu` + `frust-scene`,
plus `glifo` (exact-pinned) for glyph outline fetch/scale/cache — `frust-engine::text` is its sole
adapter onto that dependency (see above); nothing above RENDER depends on any of the three — the
same GPU-substrate layer boundary [ARCHITECTURE.md](ARCHITECTURE.md) records at the cross-unit level.

## Data Flow

- `frust_scene::Scene` → `frust-engine`'s `SceneCompiler::compile` → GPU strip/paint layouts →
  `EngineRenderer::encode` records the frame's passes into `SurfaceRenderer`'s caller-owned
  `wgpu::CommandEncoder`, one `queue.submit`, driven from a dedicated render thread by default.
- `context::choose_engine_render_path` resolves every (backend, alpha mode) pair to one of two
  arms — the engine refuses no surface's translucency. `RenderPathKind::EngineDirect` serves
  `Opaque`/`Auto`, Android's `Inherit`/`PreMultiplied` (the engine's strip pipelines already write
  premultiplied alpha, so these need no conversion pass), and `PostMultiplied` on
  `wgpu::Backend::Metal` alone: Core Animation's `CAMetalLayer` composites premultiplied
  regardless of the mode's advertised name, an upstream wgpu-hal truth bug
  (`compositor_expects_premultiplied`, now `frust-gpu::surface`'s; `engine-metal-postmultiplied-truth-bug`
  in LIMITATIONS.md), so handing Metal's `PostMultiplied` swapchain (iOS's sole translucent mode)
  the spec-correct straight-alpha conversion would double-correct it. Every other backend's
  genuinely-straight `PostMultiplied` (e.g. Vulkan's own flag) takes
  `RenderPathKind::EngineDirectUnpremultiply` instead: the frame renders into a surface-owned
  intermediate and `gpu::present::UnpremultiplyPass` converts it into the swapchain on the way out.
  `EngineDirect` renders straight into the acquired swapchain view, `RENDER_ATTACHMENT` only in
  whatever format the surface reports — no intermediate, no blit, plus a `Depth24Plus` attachment
  configured alongside the swapchain (both attachments are recreated on resize, matched to the new
  extent). An `EngineError` from `encode` leaves the caller's encoder untouched, so nothing is
  submitted and the acquired texture is dropped rather than presented: the frame is
  `FrameOutcome::Skipped`, a `refused_frames` counter increments, and the previous frame's
  swapchain content simply persists on screen. There is no fallback renderer — a refused frame is
  never retried through anything else; which render path a surface takes is decided once at
  configure time, not per frame.
- `Command::ShaderQuad` renders: `effects::shader_quad::ShaderQuadPass` runs the fragment-shader
  pre-pass into a pooled `frust_gpu::effects::ShaderEffects` target and the compiler lowers the quad
  through the same external-texture path `Command::SceneTexture` takes (see GPU Seam above). The
  remaining gap is the golden-oracle suite, which still cannot compare a shader-quad case;
  `FRUST_ENGINE_NO_SHADER_EFFECTS` is the kill switch for the whole path.
- `SurfaceAlphaRequest` resolves the platform's compositing/alpha mode to pick the presentation
  path, feeding translucency state upstream to paint.
- `context::engine_support` (`frust-render::tier`) is asked once per surface creation, and again by
  `HeadlessRenderer::new`: it refuses an adapter missing a flag in `ENGINE_REQUIRED_DOWNLEVEL_FLAGS`
  — deliberately empty, since the engine's downlevel design rules (no compute, no storage buffer,
  no indirect draw) need neither `COMPUTE_SHADERS` nor `INDIRECT_EXECUTION`, the pair the deleted
  vello-classic tier required and the iOS Simulator's Apple2 GPU family lacks. There is no other
  renderer to select between — an incapable adapter simply cannot render.
- Text: style + string → `TextContext` (cached shaping) → `TextLayout` → `GlyphRun`s via
  `to_scene_runs`, consumed by `SceneBuilder` as scene `Command`s. `TextContext::layout_bounded`
  additionally measures against a max line count and applies `TextOverflow` by truncating the shaped
  text (measure-and-truncate), since parley 0.11 exposes no native ellipsis primitive.
- `frust-engine`'s compiler (`compile/mod.rs`) lowers two scene-layer geometry primitives to
  GPU-specific shapes: a dashed stroke (`Command::Path`'s `DashPattern`) is pre-expanded to a plain
  path before stroking; a per-corner blurred shadow (`Command::BlurredRoundedRect`'s `CornerRadii`)
  collapses to `CornerRadii::largest()`, since the strip shader's blurred-rect primitive takes one
  radius (accepted approximation, see LIMITATIONS.md). `Command::RoundedRect`/`PushClipRounded`
  carry the same `CornerRadii` through to an exact `kurbo::RoundedRect` per corner — only the blur
  path collapses it.
- `TextContext::register_fonts` hot-swaps app-supplied fonts and invalidates cached shaping,
  forcing relayout upstream.
- Focus-routed Key/Ime events drive `TextEditor`, producing an `EditingState` (UTF-16 indexed)
  round-tripped through each platform's IME bridge.

## Key Types

| Type | Purpose |
|------|---------|
| `RenderContext` / `SurfaceFactory` / `DetachedSurface` (`frust-gpu`, re-exported) / `SurfaceRenderer` / `DeferredPresent` | Device ownership and surface lifecycle (`frust-gpu`), the per-surface renderer and present owner (`frust-render`), and the two sanctioned opaque wgpu wrappers for cross-thread handoff |
| `HeadlessRenderer` | Offscreen `frust-engine` renderer reused across renders (one adapter/device); verifies its resolved adapter/backend against an expectation before rendering and returns plain RGBA8 bytes, never a `wgpu` type; its output is premultiplied alpha |
| `TierCaps` / `engine_support` | Plain adapter-capability data and the one capability question left: whether an adapter can run the engine (see Data Flow) — there is no tier selection any more |
| `SurfacePhase` / `FrameOutcome` / `EncodeOutcome` / `AcquireOutcome` | The surface-can-be-destroyed-anytime lifecycle state machine shared by every shell |
| `CornerRadii` / `DashPattern` (`frust-scene`) | Per-corner rounding and dash geometry carried by `RoundedRect`/`PushClipRounded`/`BlurredRoundedRect`/dashed-stroke commands; lowered to GPU-specific shapes in the engine's compiler (see Data Flow) |
| `TextContext` / `TextStyle` / `TextLayout` / `TextOverflow` | Renderer-agnostic shaping surface: font/cache state, styling knobs, a finished measurable shaped block, and `layout_bounded`'s measure-and-truncate overflow mode |
| `TextEditor` / `EditingState` / `EditOp` | The Parley-based editing engine and its state-sync payload at the platform IME seam |
