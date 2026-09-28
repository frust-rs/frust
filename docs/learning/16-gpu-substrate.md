# Lab 16 — The GPU substrate (frust-gpu) and the render seam

**Concept:** `frust-gpu` is the layer directly above `wgpu`: it probes what an adapter can do, owns
the instance/device, configures swapchains, runs the surface lifecycle, and hands out shaders,
pipelines, pooled textures and a per-frame upload arena. It knows nothing about scenes or strips.
Its organising rule (the crate doc in `lib.rs`): **one function probes a live adapter's
capabilities**, and every decision above it is a pure function over plain values, testable GPU-free. `frust-render`
sits on top and makes the few routing decisions that are the renderer's business. Lab 4 read these
seams from above; this lab goes down into the substrate and runs its tests.

> **Two `TierCaps`.** `frust_gpu::TierCaps` (`crates/frust-gpu/src/caps.rs`) is the probed record
> this lab is about; `frust_render::TierCaps` (`crates/frust-render/src/tier.rs`, root re-export) is
> a different two-field struct (`downlevel_flags`, `adapter_name`) feeding only `engine_support`
> (open minor act_000001a0633d50b7). A bare `TierCaps` here means the `frust-gpu` one.

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `TierCaps::probe` (the only live-adapter read) / `fake`; `DownlevelProfile`, private `resolve` | `crates/frust-gpu/src/caps.rs` | ≈161 / ≈196; ≈42, ≈50 |
| `RenderContext`, `DeviceHandle`, `decide_log_action`, `optional_device_features`, `required_features`, `effective_limits` | `crates/frust-gpu/src/context.rs` | ≈136, ≈90, ≈774, ≈691, ≈711, ≈660 |
| `resolve_alpha_mode`, `select_surface_format`, `surface_config` | `crates/frust-gpu/src/surface.rs` | ≈90, ≈247, ≈287 |
| `alpha_mode_is_straight_translucent`, `compositor_expects_premultiplied`; `SurfaceFactory` → `DetachedSurface` → `ConfiguredSurface` | `crates/frust-gpu/src/surface.rs` | ≈182, ≈226; ≈327, ≈366, ≈386 |
| `SurfacePhase`, `next_phase`, `decide_acquire`, `FrameOutcome`, `AcquireOutcome` | `crates/frust-gpu/src/lifecycle.rs` | ≈52, ≈93, ≈158, ≈210, ≈252 |
| `create_android_surface` / `create_metal_surface` (unsafe) | `crates/frust-gpu/src/lifecycle.rs` | ≈290 / ≈334 |
| `ShaderLibrary::insert_wgsl`; `RenderPipelineDesc`, `AxisTable`, `PipelineCache::get_or_create` / `warm_up` | `crates/frust-gpu/src/shader.rs`; `pipeline.rs` | ≈61, ≈83; ≈165, ≈251, ≈867 / ≈893 |
| `adapter_cache_key`, `frame`, `unframe` | `crates/frust-gpu/src/pipeline_cache.rs` | ≈43, ≈56, ≈75 |
| `TexturePool`, `quantize_extent`, `effective_usage` | `crates/frust-gpu/src/pool.rs` | ≈244, ≈399, ≈424 |
| `HostBuffer` (bump arena); `CommandBuffer` (the encoder contract) | `crates/frust-gpu/src/arena.rs`; `encoder.rs` | ≈148; ≈73 |
| `HeadlessTarget::read_back`; `TimestampRing` (`pass_writes`, `abandon_frame`) | `crates/frust-gpu/src/headless.rs`; `diag.rs` | ≈94; ≈225, ≈442, ≈525 |
| `lint_wgsl_dir`, `lint_pipeline_layout`, `check_limits_against_webgl2` | `crates/frust-gpu/src/lint.rs` | ≈59, ≈203, ≈293 |
| `choose_engine_render_path` / `create_engine_surface` | `crates/frust-render/src/context.rs` | ≈152 / ≈480 |
| `engine_support`, `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` | `crates/frust-render/src/tier.rs` | ≈113, ≈50 |
| `SurfaceRenderer::submit_impl`; `ExternalPass` / `ExternalFrame` | `crates/frust-render/src/renderer.rs`; `external_pass.rs` | ≈1104; ≈144 / ≈180 |
| `HeadlessRenderer` | `crates/frust-render/src/headless.rs` | ≈205 |

## The five ideas

1. **One probe, then plain data.** `TierCaps::probe` reads info, downlevel caps, limits and
   features off a real `wgpu::Adapter` once. `DownlevelProfile` is derived, never chosen: `WebGl2`
   when the backend is `wgpu::Backend::Gl` *or* the `FRUST_ENGINE_DOWNLEVEL` override is non-zero
   (read once, compile-time or runtime), else `Full`. Under `WebGl2` the limits are clamped toward
   `wgpu::Limits::downlevel_webgl2_defaults()` and `has_storage_buffers` is forced off, so the
   override really rehearses the browser ceiling. Derived fields: `transient_saves_memory` (wgpu's
   `Option<bool>`, `None` folded to `false`), `atlas_format` (always `Rgba8Unorm`), and
   `resource_texture_dim = min(max_texture_dimension_2d, 4096)`. Everything downstream takes the
   value, so `TierCaps::fake` drives it in tests.
2. **Context and surface policy are pure functions with thin wrappers.** `RenderContext` owns one
   `wgpu::Instance` and lazily creates one `DeviceHandle` (on the first surface or a headless
   pre-init). `DeviceHandle` is a cheap clone of Arc-backed handles and carries the
   `first_uncaptured_error` latch, which the handler installed via `on_uncaptured_error` sets.
   `decide_log_action` logs the first 5 errors, emits one suppression notice, then goes silent
   apart from a debug-level count bump every 100th error. For device features,
   `optional_device_features` (just `PIPELINE_CACHE`) is narrowed to the adapter, and
   `required_features` adds `TIMESTAMP_QUERY` only for a `perf-trace` build on an adapter that has
   it. `effective_limits` raises the uniform-offset alignment to 256 on the iOS Simulator, which
   misreports it. `effective_instance_flags` strips `DEBUG`/`VALIDATION` when
   `is_android_emulator` says so.

   On the surface side:
   - `resolve_alpha_mode` maps `Opaque` to `Auto`. `TranslucentPreferred` gets the first of
     `Inherit`, `PostMultiplied`, `PreMultiplied` the surface reports, else `Auto` with a warning.
   - `select_surface_format` takes the first format, **in the surface's own order**, that is in
     `SURFACE_FORMATS` (`Rgba8Unorm`/`Bgra8Unorm`).
   - `surface_config` asks for `RENDER_ATTACHMENT` only, with a frame latency of 2.
   - `alpha_mode_is_straight_translucent` is true for `PostMultiplied` alone.
     `compositor_expects_premultiplied` is true for `Metal` + `PostMultiplied` alone: Core
     Animation composites premultiplied whatever the mode is called. That is an upstream wgpu-hal
     truth bug, recorded here as a platform fact.

   `SurfaceFactory` makes a `DetachedSurface` on the windowing thread. That value is `Send` and
   crosses to the render thread, where it becomes a `ConfiguredSurface`.
3. **The lifecycle is a state machine.** `SurfacePhase {NoSurface, SurfaceReady, SurfaceLost}`
   moves only through `next_phase`, a total table that ignores the current phase: `Created` goes to
   `SurfaceReady`, `Destroyed` to `NoSurface`, `Lost` to `SurfaceLost`. `decide_acquire` maps an
   `AcquireStatus` and a consecutive-`Invalid` counter to an action; `Invalid` reconfigures up to
   `MAX_INVALID_RECONFIGURES` (3) times, then gives up as `Lose`. `AcquireOutcome {Acquired,
   Reconfigured, Lost, Skipped}` is the acquire half of the present seam, and `FrameOutcome
   {Rendered, Skipped, Redraw, SurfaceLost}` is what a shell reacts to. Raw pointers become surfaces
   only in `create_android_surface` (`ANativeWindow*`) and `create_metal_surface` (`CAMetalLayer*`,
   Apple targets only). Each one isolates a single safety contract; the crate's other `unsafe fn` is
   `RenderContext::create_pipeline_cache`.
4. **Nothing compiles on the frame; nothing allocates twice.**
   - Shaders and pipelines. `ShaderLibrary` is append-only, `insert_wgsl` is idempotent per name,
     and callers hold an opaque `ShaderId`. A `RenderPipelineDesc` packs into a `u64` key: program
     16 b / format 12 b / blend 12 b / depth 12 b / `log2(sample_count)` 4 b. The wide axes are
     interned through `AxisTable`, so distinct values never collide. When a request hits
     `PipelineCache::get_or_create` for a variant still queued on `warm_up`'s worker, it **steals**
     the job and builds it inline (Impeller's `PerformJobEagerly`).
   - The driver cache. `frame` lays a blob out as `MAGIC · key_len · key · payload_len · payload`,
     keyed by `adapter_cache_key`. `unframe` returns `None` on any mismatch, so the unsafe
     `create_pipeline_cache` never sees a foreign byte.
   - Textures. `TexturePool` rounds extents up to a 256 px quantum (`quantize_extent`, never past
     the ceiling) and ages free entries out after `DEFAULT_MAX_UNUSED_FRAMES` (60). It adds
     `TRANSIENT_ATTACHMENT` only when the caps say it saves memory *and* the usage is exactly
     `RENDER_ATTACHMENT`.
   - Uploads. `HostBuffer` bump-allocates every small per-frame upload and sends it with one
     `write_buffer`. It grows by `next_power_of_two` and never shrinks.
   - Recording. `CommandBuffer` is the single-submit contract: a renderer records only, never
     submits, and holds no borrow past return. Whoever records first owns the depth clear, and
     later passes load (`LessEqual`, far plane 1.0). Glyph-atlas uploads are the one sanctioned
     own-encoder submit.
   - Readback and timing. `HeadlessTarget::read_back` strips wgpu's row padding. `TimestampRing`
     uses `timestamp_writes` at pass boundaries only, because in-pass timestamps are unavailable on
     tile-based mobile GPUs. It is inert without `TIMESTAMP_QUERY`, and `abandon_frame` handles a
     refused frame.
5. **The lint, and the seam above.** `lint.rs` defines exactly six rule ids:
   - `lint_wgsl_dir` (via `check_wgsl_file`) checks **E1** (no `@compute`), **E2** (no
     `var<storage` / `texture_storage_`), and **E5** (no `firstTrailingBit`/`firstLeadingBit`/
     `countLeadingZeros`, and no `textureLoad(` in a file declaring a `texture_depth*` binding).
   - `lint_pipeline_layout` checks **E6** (uniform binding ≤ 16 KiB) and **E8** (≤ 4 bind groups,
     ≤ 8 vertex buffers, ≤ 16 attributes, stride ≤ 255, sample count 1).
   - `check_limits_against_webgl2` checks **E7** (`max_texture_dimension_2d`) plus E8 limits.

   No other E-number appears in `lint.rs`, though its module doc and `docs/RENDER_ARCHITECTURE.md`
   say "E1–E18". **E17** (panic-free frame path) is enforced separately by
   `crates/frust-engine/tests/structure.rs`. The engine's `shaders/` directory is linted by
   `crates/frust-gpu/tests/downlevel_rules.rs` and again by `structure.rs`'s G1.

   Above the substrate sits `frust-render`'s `create_engine_surface`. It gates the adapter through
   `engine_support` (over the deliberately empty `ENGINE_REQUIRED_DOWNLEVEL_FLAGS`), then calls
   `choose_engine_render_path` **once, at configure time**. The result is
   `RenderPathKind::EngineDirect`, unless the mode is straight-translucent and the compositor does
   not expect premultiplied. That case gets `EngineDirectUnpremultiply`: render into an
   intermediate, then run an `UnpremultiplyPass`. Each frame, `SurfaceRenderer::submit_impl` first
   drains registered `ExternalPass`es (`run_external_passes`), handing each an `ExternalFrame`
   with the device, queue and the frame's own encoder, and records the scene after them in that
   same encoder. `HeadlessRenderer` is the windowless version, used for goldens and
   `crates/frust-render/tests/gpu_smoke.rs`.

## Experiments

All GPU-free unless marked; set a per-worktree `CARGO_TARGET_DIR` (e.g.
`/data/cache/target-wt-t2-07`). A `--lib <name>` filter is a substring match (`surface` also picks
up lifecycle/lint/pipeline tests naming it), so read each result's module prefix.

### 16.1 — Caps without a GPU

`cargo test -p frust-gpu --lib caps` passes 11 tests, with 1 ignored (the real-adapter test). Then
write a scratch integration test that prints `TierCaps::fake` for both profiles. Put it in
a new file under `crates/frust-gpu/tests/` and **delete it afterwards**. Real output at `f1cebf5e`:

```text
Full:   backend=Vulkan max_tex_2d=8192 resource_texture_dim=4096 atlas_format=Rgba8Unorm storage=true  ubo=65536
WebGl2: backend=Gl     max_tex_2d=2048 resource_texture_dim=2048 atlas_format=Rgba8Unorm storage=false ubo=16384
```

The 4096 cap only matters on `Full`, because WebGL2 is already under it.

### 16.2 — The Metal carve-out, proven

`cargo test -p frust-gpu --lib surface` passes 23 tests. The proof is
`surface::tests::compositor_expects_premultiplied_is_metal_post_multiplied_only`: it sweeps every
`wgpu::Backend` × alpha mode and allows exactly one `true`.
`the_truth_bug_only_ever_contradicts_a_straight_translucent_mode` pins that the carve-out only
overrides a mode that was straight-translucent to begin with. The routing that consumes it is
tested in `crates/frust-render/src/context.rs` by `metal_post_multiplied_skips_the_unpremultiply_arm`
and `non_metal_post_multiplied_keeps_the_unpremultiply_arm`.

### 16.3 — Walk the lifecycle table

`cargo test -p frust-gpu --lib lifecycle` passes 12 tests. Read `next_phase` first and notice that
`_current` is unused, which is what makes the table total. Then read
`invalid_reconfigures_under_the_cap` and `invalid_gives_up_at_the_cap` against `decide_acquire`, and
the `invalid_streak_*` tests against `next_invalid_streak`.

### 16.4 — Key packing and a corrupted cache blob

`cargo test -p frust-gpu --lib pipeline` passes 22 tests, with 2 ignored (GPU).
`cargo test -p frust-gpu --test pipeline_cache` passes 10.
- `each_key_axis_is_its_own_variant` changes one axis at a time.
- `an_unusable_sample_count_has_no_key_and_is_not_cached` gives `sample_count: 3` no key.
- `a_render_thread_request_steals_a_queued_job_and_the_queue_skips_it` is the eager steal.
- In `crates/frust-gpu/tests/pipeline_cache.rs`, `wrong_magic_is_rejected`,
  `truncated_blob_is_rejected`, `trailing_garbage_is_rejected` and
  `lying_payload_length_is_rejected` each corrupt a framed blob differently, and `unframe` answers
  `None` to every one.

### 16.5 — Make the lint fire

`cargo test -p frust-gpu --test downlevel_rules` passes 9 tests. Next, write a `.wgsl` file
containing `@compute @workgroup_size(64)` and `fn main() {}` into a **temp dir**, never under
`crates/frust-engine/shaders/`. Pass it to `frust_gpu::lint_wgsl_dir` from a scratch test. Real
result:

```text
E1 line Some(1): scratch.wgsl — Compute shaders not allowed: rule E1 (no compute)
```

### 16.6 — Quantisation and the bump arena

`cargo test -p frust-gpu --test pool_and_arena` passes 10 tests, with 1 ignored (GPU).
- `a_resize_storm_collapses_onto_few_pooled_textures` drives 400 resize frames and asserts that
  no more than 40 textures are created.
- `the_keep_alive_window_is_what_costs_the_extra_allocations` prices the 60-frame window.
- `arena_grows_only_and_keeps_its_high_water_mark` shows one big frame grows the buffer and a
  later small frame does not shrink it.

### 16.7 — The downlevel adapter that passes

`cargo test -p frust-render --lib tier` passes 4 tests. `a_downlevel_adapter_still_runs_the_engine`
feeds three flag sets: `DownlevelFlags::empty()`, everything except `COMPUTE_SHADERS`, and
everything except `INDIRECT_EXECUTION`. All three get `Ok`.
`a_missing_required_flag_is_refused_by_name` keeps the refusal arm alive through
`engine_support_with`.

### 16.8 — OPTIONAL (GPU): the real thing

`cargo test -p frust-render --test gpu_smoke -- --ignored` passed 4 tests on a desktop GPU. It
renders through `HeadlessRenderer`, and `unaligned_width_reads_back_without_row_padding` checks
`read_back`'s padding strip. `(cd benchmarks/frust_bench && FRUST_ENGINE_DOWNLEVEL=1 cargo run)`
runs the whole app with `TierCaps::probe` clamped to WebGL2 (not run for this lab).

## What to notice

- Count `crates/frust-gpu/src` functions taking a `wgpu::Adapter`/`Device` against those taking
  plain values: the pure ones hold the policy, and every experiment above ran them GPU-free.
- `choose_engine_render_path` runs once per surface, not per frame; a per-frame alpha branch is a bug.
- The E-numbers are a design vocabulary, not a checklist the code fully implements. Trust
  `lint.rs` and `structure.rs` over any prose that says "E1–E18".
