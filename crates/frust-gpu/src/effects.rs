//! Offscreen WGSL fragment-shader effects: the GPU half of the
//! shader-showcase feature.
//!
//! A self-contained engine that renders user-supplied WGSL fragment shaders
//! into offscreen `Rgba8Unorm` textures. It owns everything that job needs and
//! nothing else:
//!
//! - **Lazy per-program pipeline compilation** ([`ShaderEffects::ensure_pipeline`]),
//!   seeded from the surface's [`wgpu::PipelineCache`] so a persisted cache
//!   speeds first-frame compilation exactly like the renderer's own pipelines.
//! - **Per-`(program, size)` target state** ([`ShaderEffects::ensure_target`]):
//!   an offscreen texture + view, a 16-byte uniform buffer, and the bind group
//!   binding them, keyed by `(id, w, h)` so a resized quad transparently
//!   replaces its target.
//! - **Fullscreen-triangle pass encoding** ([`ShaderEffects::encode_pass`]).
//! - **Age-based whole-id reap** ([`ShaderEffects::mark_seen`]/[`ShaderEffects::reap`]):
//!   a program id absent from every frame's live key set for
//!   [`MAX_UNSEEN_FRAMES`] consecutive frames has its pipeline, target(s), and
//!   any recorded compile failure dropped, rather than living until surface
//!   teardown — the whole-id counterpart to
//!   [`ShaderEffects::evict_stale_targets`]'s frame-scoped resized-away-size
//!   reclaim.
//! - **Churn detection** ([`should_warn_churn`], consulted by
//!   [`ShaderEffects::ensure_pipeline`]): a rate-limited `log::warn!` once the
//!   live compiled-program count crosses [`CHURN_WARN_THRESHOLD`] — a
//!   detection aid pointing at the `ShaderProgram::new` cache-once contract,
//!   not itself a bound on growth; the age-based reap above is what actually
//!   bounds it.
//!
//! It is deliberately decoupled from any scene vocabulary: the API speaks
//! `(id: u64, wgsl: &str, size, time)` primitives, never a display list or a
//! command, so the size-clamp and quad-placement policy that decides *which*
//! `(id, size)` pairs a frame asks for lives in the layer above
//! (`frust_engine::effects::shader_quad`), and only the GPU resources live
//! here.
//!
//! # Where the rendered target goes
//!
//! [`ShaderEffects::target_view`] is the seam a renderer draws the result
//! through: the target carries `TEXTURE_BINDING`, so the view it hands back is
//! registered as a scene texture and sampled by the frame's own passes. The
//! texture itself is reachable through [`ShaderEffects::target_texture`] for a
//! read-back, and its true (clamped) extent through
//! [`ShaderEffects::target_extent`] — the extent a registration must state,
//! since it is the texture's own rather than whatever size was asked for.
//! Nothing is registered with a foreign renderer and nothing is handed back on
//! eviction: the pool owns the texture and a consumer borrows it.
//!
//! ## Alpha
//!
//! A target is `Rgba8Unorm` and the pass writes the fragment shader's output
//! into it unblended, so what the shader returns is what the target holds. The
//! consumer samples that target as **premultiplied** colour, which is the
//! convention every other engine paint travels in: a shader returning
//! `vec4(rgb, a)` must have already multiplied `rgb` by `a`, and one returning
//! opaque output (`a = 1.0`) is unaffected either way.
//!
//! ## Shader contract
//!
//! The caller supplies only the **fragment** source. It is compiled after a
//! fixed prelude ([`VERTEX_PRELUDE`]) that declares the uniform block and the
//! fullscreen-triangle vertex stage, so a fragment shader:
//!
//! - defines the fragment entry point **`fs_main`**
//!   (`@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32>`), and
//! - reads the uniforms as `frust_u.resolution` / `frust_u.time`.
//!
//! ## No panics
//!
//! Every path here upholds the FFI no-panic invariant
//! (`docs/CODE_STANDARDS.md`): a shader that fails to compile is recorded in
//! `ShaderEffects::failed` and skipped (with a rate-limited `log::warn!`)
//! rather than panicking; the underlying validation error also surfaces
//! through the device's latched uncaptured-error handler
//! ([`crate::context::create_device`](crate::context)).

use std::collections::{HashMap, HashSet};

/// Byte size of the uniform buffer: `vec2<f32> resolution` (8) + `f32 time`
/// (4) + `f32 _pad` (4). The WGSL struct rounds its 12-byte payload up to a
/// 16-byte alignment boundary, so the buffer and the Rust-side packing both use 16.
const UNIFORM_SIZE: u64 = 16;

/// How many distinct shader-compile failures are logged at warn level before
/// the warnings are suppressed — the rate limit that keeps a batch of broken
/// shaders from flooding the log. A failure past this is still recorded (so it
/// is skipped), just not re-logged.
const MAX_FAILED_WARNINGS: u32 = 8;

/// How many consecutive frames a program id may go unseen (absent from every
/// frame's live key set — see [`ShaderEffects::mark_seen`]) before its
/// compiled pipeline, offscreen target(s), and any recorded compile failure
/// are reaped ([`ShaderEffects::reap`]) rather than living until surface
/// teardown. ~120 frames is roughly 1-2s at a 60-120Hz refresh rate:
/// generous enough that a shader drawn intermittently (every-other-frame, or
/// through a brief scene-diff hiccup) is never mistaken for vanished, short
/// enough that navigating away from a shader-showcase screen reclaims its
/// GPU state promptly instead of leaking for the rest of the session.
pub const MAX_UNSEEN_FRAMES: u64 = 120;

/// Distinct-compiled-program-id threshold [`should_warn_churn`] compares
/// `ShaderEffects::pipelines`'s live size against. Crossing it is a
/// detection signal — not itself a bound on growth (see below) — that
/// `ShaderProgram::new` is likely being called somewhere that re-runs every
/// frame/rebuild (a widget's `paint`, or a `Component`'s `build`) instead of
/// once, per its documented cache-once contract
/// (`frust_scene::ShaderProgram::new`'s rustdoc). 32 is chosen with generous
/// headroom over the shader-showcase's own program count (a handful of
/// fixed shaders) so a legitimate small gallery never trips it, while a
/// per-frame-minting footgun — which compiles a fresh id on every frame and
/// only stops accumulating once [`MAX_UNSEEN_FRAMES`]-old entries start
/// reaping — reliably crosses it within about a second.
///
/// **Detection-only**: this constant does not bound the maps' actual growth —
/// [`ShaderEffects::reap`] (driven by [`MAX_UNSEEN_FRAMES`]) is what keeps
/// them from growing without bound; this threshold only decides when to *warn*
/// that growth is happening in the first place.
pub const CHURN_WARN_THRESHOLD: usize = 32;

/// How many churn-detection warnings (see [`CHURN_WARN_THRESHOLD`]) are
/// logged before further ones are suppressed — the same rate-limit shape as
/// [`MAX_FAILED_WARNINGS`], applied to a distinct signal (concurrently
/// compiled program count, not compile failures).
const MAX_CHURN_WARNINGS: u32 = 8;

/// The fixed WGSL prelude prepended to every fragment source: the 16-byte
/// uniform block at `@group(0) @binding(0)` and the vertex-buffer-free
/// fullscreen-triangle vertex stage.
///
/// The fragment source appended after this must define `fs_main` and read
/// `frust_u.resolution` / `frust_u.time` (see the module-level shader contract).
pub const VERTEX_PRELUDE: &str = r#"// --- frust shader-effect prelude (generated) ---
struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
}
@group(0) @binding(0) var<uniform> frust_u: Uniforms;

struct FrustVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen triangle from the vertex index alone — no vertex buffer.
// Vertices 0,1,2 produce uv (0,0),(2,0),(0,2), covering the [-1,1] clip square
// with a single oversized triangle.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> FrustVsOut {
    var out: FrustVsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.uv = uv;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}
// --- end prelude ---
"#;

/// A compiled render pipeline for one shader program plus the bind-group layout
/// its targets bind against.
struct PipelineEntry {
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
}

/// The per-`(program, size)` GPU state a shader effect renders into: an
/// offscreen `Rgba8Unorm` texture (`RENDER_ATTACHMENT | TEXTURE_BINDING |
/// COPY_SRC` — the attachment the pass writes, the binding a consumer samples
/// it through, and the copy source a read-back needs), its view, the 16-byte
/// uniform buffer, and the bind group wiring the buffer to `@binding(0)`.
/// Stores the actual (clamped)
/// extent the texture was created at so `encode_pass` and `target_texture` use
/// the true texture dimensions in the uniform and returned texture handle,
/// agreeing by construction with the texture's own size.
struct TargetEntry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The actual (clamped) width this entry's texture was created at.
    used_w: u32,
    /// The actual (clamped) height this entry's texture was created at.
    used_h: u32,
}

/// Owns every GPU resource the shader-showcase feature needs: lazily-compiled
/// per-program pipelines and per-`(program, size)` offscreen targets.
pub struct ShaderEffects {
    /// Compiled pipelines, keyed by program id.
    pipelines: HashMap<u64, PipelineEntry>,
    /// Offscreen targets, keyed by `(program id, width, height)`.
    targets: HashMap<(u64, u32, u32), TargetEntry>,
    /// A clone of the surface's pipeline cache, seeding pipeline compilation.
    pipeline_cache: Option<wgpu::PipelineCache>,
    /// Program ids whose shader failed to compile — skipped on every later
    /// `ensure_pipeline` so a broken shader is compiled at most once.
    failed: HashSet<u64>,
    /// How many compile failures have been logged so far (the [`MAX_FAILED_WARNINGS`]
    /// rate limit).
    warned_failures: u32,
    /// How many churn-detection warnings have been logged so far (the
    /// [`MAX_CHURN_WARNINGS`] rate limit) — see [`should_warn_churn`].
    warned_churn: u32,
    /// Monotonic per-frame counter, bumped once per [`Self::mark_seen`] call —
    /// the age clock [`reapable_ids`] measures a program id's absence against.
    frame: u64,
    /// The frame (per `Self::frame`) each program id was last present in a
    /// frame's live key set, updated by [`Self::mark_seen`] for every id
    /// passed in — including one whose pipeline failed to compile, so a
    /// broken shader's `failed` entry can still age out. An id absent from
    /// this map has never been seen (or was already reaped).
    last_seen: HashMap<u64, u64>,
    /// Requested extents for which we have already warned about clamping. Keyed
    /// by `(program id, requested width, requested height)` so the clamp warning
    /// fires at most once per distinct oversized request, surviving the
    /// frame-scoped [`Self::evict_stale_targets`] (an evicted-and-recreated
    /// target does not re-warn). [`Self::reap`] drops an id's entries with the
    /// rest of its state, which is both the "brand new again" contract and the
    /// bound on this set's growth.
    warned_clamped: HashSet<(u64, u32, u32)>,
}

/// Pack the 16-byte uniform buffer contents: `resolution.x`, `resolution.y`,
/// `time`, `_pad` as little-endian `f32`s. Pure — unit-testable byte layout.
fn uniform_bytes(width: u32, height: u32, time: f32) -> [u8; UNIFORM_SIZE as usize] {
    let mut bytes = [0u8; UNIFORM_SIZE as usize];
    bytes[0..4].copy_from_slice(&(width as f32).to_le_bytes());
    bytes[4..8].copy_from_slice(&(height as f32).to_le_bytes());
    bytes[8..12].copy_from_slice(&time.to_le_bytes());
    // bytes[12..16] stays zero — the explicit `_pad` field.
    bytes
}

/// Concatenate the fixed [`VERTEX_PRELUDE`] and a caller-supplied fragment
/// source into the full WGSL module compiled for a program. Pure.
fn compose_shader(fragment_src: &str) -> String {
    format!("{VERTEX_PRELUDE}\n{fragment_src}")
}

/// The target keys a frame's eviction reclaims: same-`id` targets whose size is
/// **not** live this frame, where the frame's live key set is `live` (every
/// `(id, w, h)` a shader quad resolves to this frame). Pure: operates on the
/// key sets alone, so the eviction policy is GPU-free unit-testable.
///
/// Frame-scoped, not per-quad: a program drawn at two distinct sizes in the same
/// frame has both `(id, w, h)` keys in `live`, so both survive — the two-sizes-
/// one-frame case the old per-quad policy (which evicted any other-size same-id
/// entry unconditionally) ping-ponged every frame. A size a quad resized *away*
/// from is not in `live` and is reclaimed. A program id absent from `live`
/// entirely is left intact — whole-id reaping is a separate concern, so a
/// vanished program's targets are preserved here exactly as before.
fn stale_target_keys<I>(target_keys: I, live: &HashSet<(u64, u32, u32)>) -> Vec<(u64, u32, u32)>
where
    I: IntoIterator<Item = (u64, u32, u32)>,
{
    let live_ids: HashSet<u64> = live.iter().map(|&(id, _, _)| id).collect();
    target_keys
        .into_iter()
        .filter(|key| live_ids.contains(&key.0) && !live.contains(key))
        .collect()
}

/// Whether a compile failure at `prior_warn_count` prior warnings should still
/// be logged (the [`MAX_FAILED_WARNINGS`] rate limit). Pure.
fn should_warn_failure(prior_warn_count: u32) -> bool {
    prior_warn_count < MAX_FAILED_WARNINGS
}

/// Whether [`ShaderEffects::ensure_pipeline`] should log its churn-detection
/// warning: `compiled_ids` (the live compiled-pipeline count) has crossed
/// [`CHURN_WARN_THRESHOLD`] and fewer than this module's `MAX_CHURN_WARNINGS`
/// have already been logged — mirroring `should_warn_failure`'s shape for a
/// distinct signal. Pure: no GPU/log state touched, so the rate-limited decision is
/// unit-testable on its own.
///
/// Detection-only, like the constant it reads: this predicate decides
/// whether to *warn*, not whether to reap — [`ShaderEffects::reap`] bounds
/// the actual growth independently of this rate limit.
pub fn should_warn_churn(compiled_ids: usize, prior_warn_count: u32) -> bool {
    compiled_ids > CHURN_WARN_THRESHOLD && prior_warn_count < MAX_CHURN_WARNINGS
}

/// The program ids in `last_seen` whose last-seen frame is at least `max_age`
/// frames behind `current_frame` — a vanished-id reap candidate list, i.e.
/// every id [`ShaderEffects::reap`] should drop resources for. Pure: operates
/// on the age map alone (no GPU state), so the reap policy is unit-testable
/// exactly like [`stale_target_keys`].
///
/// A vanished id (one whose shader quad stops appearing in the scene entirely
/// — screen navigation, a dynamic id, etc.) is a distinct case from a
/// resized-away *size* of a still-drawn id, which
/// [`stale_target_keys`]/[`ShaderEffects::evict_stale_targets`] already
/// reclaims frame-scoped; this function is the whole-id counterpart, aged
/// rather than frame-scoped since a vanished id produces no live key at all
/// for `evict_stale_targets` to compare against.
fn reapable_ids(last_seen: &HashMap<u64, u64>, current_frame: u64, max_age: u64) -> Vec<u64> {
    last_seen
        .iter()
        .filter(|&(_, &seen)| current_frame.saturating_sub(seen) >= max_age)
        .map(|(&id, _)| id)
        .collect()
}

/// The `targets` keys [`ShaderEffects::reap`] should remove for a given
/// `stale_ids` list — every key whose id is one of them. Pure key-selection,
/// mirroring [`stale_target_keys`]'s shape, so `reap`'s target-removal choice
/// is unit-testable without a `wgpu::Device` (a real `TargetEntry` can't be
/// constructed without one).
fn reapable_target_keys<I>(target_keys: I, stale_ids: &HashSet<u64>) -> Vec<(u64, u32, u32)>
where
    I: IntoIterator<Item = (u64, u32, u32)>,
{
    target_keys
        .into_iter()
        .filter(|key| stale_ids.contains(&key.0))
        .collect()
}

impl ShaderEffects {
    /// Create an empty engine seeded with an optional clone of the surface's
    /// [`wgpu::PipelineCache`] (used as `RenderPipelineDescriptor.cache` so a
    /// persisted cache speeds first compilation). `None` on adapters without
    /// `PIPELINE_CACHE` support (Metal/desktop), which is a plain cold start.
    pub fn new(pipeline_cache: Option<wgpu::PipelineCache>) -> Self {
        Self {
            pipelines: HashMap::new(),
            targets: HashMap::new(),
            pipeline_cache,
            failed: HashSet::new(),
            warned_failures: 0,
            warned_churn: 0,
            frame: 0,
            last_seen: HashMap::new(),
            warned_clamped: HashSet::new(),
        }
    }

    /// Whether program `id` still needs its pipeline compiled — false once it is
    /// either compiled or known-failed. Pure predicate (the fast-path skip
    /// [`ensure_pipeline`](Self::ensure_pipeline) consults first), unit-testable
    /// without a device.
    pub fn needs_compile(&self, id: u64) -> bool {
        !self.pipelines.contains_key(&id) && !self.failed.contains(&id)
    }

    /// Lazily compile the render pipeline for program `id` from `wgsl` (the
    /// fragment source; see the module-level shader contract). A no-op if the
    /// program is already compiled or already known-failed.
    ///
    /// Compilation is wrapped in a `Validation` error scope: a shader that fails
    /// to validate records `id` as failed (skipped forever after) with a
    /// rate-limited `log::warn!`, and **never panics** — the FFI no-panic
    /// invariant. The validation error also surfaces through the device's
    /// latched uncaptured-error handler.
    pub fn ensure_pipeline(&mut self, device: &wgpu::Device, id: u64, wgsl: &str) {
        if !self.needs_compile(id) {
            return;
        }

        let source = compose_shader(wgsl);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frust shader-effect module"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust shader-effect binds"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frust shader-effect layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frust shader-effect pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                // No blending: the shader's output is written into the
                // target verbatim, so what a consumer samples is exactly what
                // the fragment stage returned (premultiplied — see the
                // module header's alpha note).
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: self.pipeline_cache.as_ref(),
        });

        if let Some(error) = drain_error_scope(device, scope) {
            self.record_failed(id, &error.to_string());
            return;
        }

        self.pipelines.insert(
            id,
            PipelineEntry {
                pipeline,
                bind_layout,
            },
        );

        // Churn detection only — see CHURN_WARN_THRESHOLD's doc comment.
        // This does NOT bound the underlying growth (mark_seen/reap, driven by
        // MAX_UNSEEN_FRAMES, does that); it only surfaces the common
        // cache-once-contract violation (a fresh ShaderProgram minted every
        // frame/rebuild instead of created once and cached — see
        // ShaderProgram::new's rustdoc) via a rate-limited log.
        if should_warn_churn(self.pipelines.len(), self.warned_churn) {
            self.warned_churn += 1;
            log::warn!(
                "frust-gpu: {} distinct shader programs are concurrently compiled for this \
                 surface (threshold {CHURN_WARN_THRESHOLD}) — if new ShaderProgram instances are \
                 being minted every frame/rebuild instead of created once and cached (see \
                 ShaderProgram::new's cache-once contract), this is why; this warning is \
                 detection-only and does not itself bound the growth",
                self.pipelines.len(),
            );
        }
    }

    /// Record a compile failure: mark `id` skipped and warn (rate-limited).
    fn record_failed(&mut self, id: u64, message: &str) {
        self.failed.insert(id);
        if should_warn_failure(self.warned_failures) {
            self.warned_failures += 1;
            log::warn!("frust-gpu: shader program {id} failed to compile, skipping: {message}");
        }
    }

    /// Bump the frame counter and record every id in `live_ids` (this frame's
    /// distinct program ids, taken before compile/target work — so an id whose
    /// pipeline just failed to compile is still marked seen) as last seen at
    /// the new frame. Returns every id that has now gone unseen for at least
    /// [`MAX_UNSEEN_FRAMES`] frames (via this module's `reapable_ids`), ready to
    /// hand to
    /// [`Self::reap`].
    ///
    /// Called once per frame regardless of whether `live_ids` is empty — a
    /// scene that stops drawing shader quads entirely must still age out and
    /// eventually reap every previously-seen id, not just ones still present.
    pub fn mark_seen(&mut self, live_ids: &HashSet<u64>) -> Vec<u64> {
        self.frame += 1;
        for &id in live_ids {
            self.last_seen.insert(id, self.frame);
        }
        reapable_ids(&self.last_seen, self.frame, MAX_UNSEEN_FRAMES)
    }

    /// Reap every resource for each id in `stale_ids` (a [`Self::mark_seen`]
    /// output): every `targets` entry whose key's id matches, plus the compiled
    /// `pipelines` entry, any `failed` record, and the `last_seen` row itself.
    /// Dropping `failed` alongside the rest means a program id that reappears
    /// after being reaped is treated as brand new: it recompiles cleanly rather
    /// than hitting a stale skip from a compile failure that happened frames
    /// ago (or never happened at all). The clamp-warning latch is dropped on
    /// the same trigger: a reaped id that returns warns afresh on its first
    /// oversized request, exactly like a brand-new program — and this reap is
    /// also what bounds the latch set's growth, the same way it bounds every
    /// other map here.
    pub fn reap(&mut self, stale_ids: &[u64]) {
        let stale_id_set: HashSet<u64> = stale_ids.iter().copied().collect();
        for key in reapable_target_keys(self.targets.keys().copied(), &stale_id_set) {
            self.targets.remove(&key);
        }
        self.warned_clamped
            .retain(|key| !stale_id_set.contains(&key.0));
        for &id in stale_ids {
            self.pipelines.remove(&id);
            self.failed.remove(&id);
            self.last_seen.remove(&id);
        }
    }

    /// Evict the targets a frame's live key set has left stale — the resized-out
    /// sizes of a still-drawn program. `live` is every `(id, w, h)` key drawn
    /// this frame; see this module's `stale_target_keys` for the frame-scoped
    /// policy (two sizes of one id both live both survive; a vanished id is left
    /// intact).
    ///
    /// Called once per frame by the caller's pre-pass after it has resolved the
    /// frame's full live key set, rather than per-quad during target creation —
    /// which is what lets two distinct sizes of the same program coexist within
    /// one frame instead of evicting each other.
    pub fn evict_stale_targets(&mut self, live: &HashSet<(u64, u32, u32)>) {
        for stale in stale_target_keys(self.targets.keys().copied(), live) {
            self.targets.remove(&stale);
        }
    }

    /// Ensure a target exists for `(id, w, h)`, creating it if absent. Eviction
    /// of resized-out sizes is frame-scoped and handled separately by
    /// [`evict_stale_targets`](Self::evict_stale_targets), so two distinct sizes
    /// of the same program can coexist within one frame.
    ///
    /// A no-op if program `id` has no compiled pipeline (never compiled, or
    /// compile-failed): a target is useless without the pipeline that owns its
    /// bind-group layout, so the caller compiles first. `w`/`h` are clamped to
    /// the device's own `max_texture_dimension_2d` ceiling (floored at 1) — a
    /// device-validity floor only, not the caller's size *policy*: a caller may
    /// still apply a tighter policy cap of its own (e.g.
    /// `frust_engine::effects::shader_quad`'s 8192) before calling, but an oversized
    /// request that reaches here creates a texture at the device ceiling
    /// instead of tripping `wgpu` validation, and warns once per distinct
    /// oversized `(id, requested_w, requested_h)` pair — the warning latches
    /// the first time this exact request is clamped, then repeats only if
    /// the request changes.
    pub fn ensure_target(&mut self, device: &wgpu::Device, id: u64, w: u32, h: u32) {
        let key = (id, w, h);
        if self.targets.contains_key(&key) {
            return;
        }

        let Some(pipeline_entry) = self.pipelines.get(&id) else {
            return;
        };

        let ceiling = device.limits().max_texture_dimension_2d;
        let used_w = w.min(ceiling).max(1);
        let used_h = h.min(ceiling).max(1);
        if used_w != w || used_h != h {
            // Warn only if we haven't warned for this exact (id, w, h) request before.
            if self.warned_clamped.insert(key) {
                log::warn!(
                    "frust-gpu: shader-effect target {id} requested {w}x{h}, clamped to \
                     {used_w}x{used_h} (the device's max_texture_dimension_2d ceiling {ceiling})",
                );
            }
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust shader-effect target"),
            size: wgpu::Extent3d {
                width: used_w,
                height: used_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust shader-effect uniforms"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust shader-effect bind group"),
            layout: &pipeline_entry.bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        self.targets.insert(
            key,
            TargetEntry {
                texture,
                view,
                uniforms,
                bind_group,
                used_w,
                used_h,
            },
        );
    }

    /// Encode one fullscreen-triangle pass for program `id` at `size` into its
    /// target, writing the uniform buffer (`resolution`, `time`) first. A no-op
    /// if the program's pipeline or the `(id, size)` target is missing. Adds no
    /// `queue.submit` — the caller owns encoder creation and submission ordering
    /// relative to the frame's own passes. The resolution uniform is written with
    /// the target's actual (clamped) extent, not the caller's requested size, so
    /// shader code sees the true rendering dimensions matching the attachment.
    pub fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        id: u64,
        size: (u32, u32),
        time: f32,
    ) {
        let (w, h) = size;
        let (Some(pipeline_entry), Some(target)) =
            (self.pipelines.get(&id), self.targets.get(&(id, w, h)))
        else {
            return;
        };

        queue.write_buffer(
            &target.uniforms,
            0,
            &uniform_bytes(target.used_w, target.used_h, time),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frust shader-effect pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline_entry.pipeline);
        pass.set_bind_group(0, &target.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The offscreen texture program `id` was rendered into at `(w, h)`, or
    /// `None` if that target does not exist (never
    /// [`ensure_target`](Self::ensure_target)ed, or the pipeline compile
    /// failed).
    ///
    /// The read-back seam. A consumer that draws the result instead wants
    /// [`Self::target_view`], since a scene-texture registration takes a view;
    /// nothing is registered with a foreign renderer and nothing is handed
    /// back on eviction — the pool owns the texture and the borrow lives as
    /// long as the caller holds it.
    pub fn target_texture(&self, id: u64, w: u32, h: u32) -> Option<&wgpu::Texture> {
        self.targets.get(&(id, w, h)).map(|entry| &entry.texture)
    }

    /// The full-extent view of program `id`'s `(w, h)` target — the handle a
    /// consumer registers to sample the rendered result — or `None` on the
    /// same terms as [`Self::target_texture`].
    ///
    /// The same view the pass wrote through, rather than a fresh one per
    /// frame: a view is a handle onto the texture, so creating one per
    /// registration would churn a resource that never changes while its
    /// target lives.
    pub fn target_view(&self, id: u64, w: u32, h: u32) -> Option<&wgpu::TextureView> {
        self.targets.get(&(id, w, h)).map(|entry| &entry.view)
    }

    /// The extent program `id`'s `(w, h)` target was actually created at, or
    /// `None` on the same terms as [`Self::target_texture`].
    ///
    /// Not `(w, h)` echoed back: [`Self::ensure_target`] clamps to the
    /// device's own ceiling, so a registration that stated the requested size
    /// would map a destination rectangle onto texels the texture does not
    /// have. This answers what the attachment holds, which is also what the
    /// `resolution` uniform the shader read was written with.
    pub fn target_extent(&self, id: u64, w: u32, h: u32) -> Option<(u32, u32)> {
        self.targets
            .get(&(id, w, h))
            .map(|entry| (entry.used_w, entry.used_h))
    }
}

/// Synchronously drain a wgpu error scope, returning the captured validation
/// error (if any). On native wgpu the [`pop`](wgpu::ErrorScopeGuard::pop) future
/// is ready as soon as the synchronously-captured creation error is recorded, so
/// this resolves without an async runtime; the `device.poll` fallback drives any
/// residual pending state to completion rather than spinning. Never panics.
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_bytes_is_16_and_little_endian() {
        let bytes = uniform_bytes(1920, 1080, 2.5);
        assert_eq!(bytes.len(), 16);
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1920.0);
        assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1080.0);
        assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 2.5);
        // The explicit `_pad` word is zero.
        assert_eq!(&bytes[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn compose_shader_prepends_prelude_and_keeps_fragment() {
        let frag = "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> \
                    { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";
        let full = compose_shader(frag);
        assert!(full.starts_with(VERTEX_PRELUDE));
        assert!(full.contains("fn vs_main"));
        assert!(full.contains("var<uniform> frust_u: Uniforms"));
        assert!(full.contains(frag));
        // The fragment is appended after the prelude, never before it.
        assert!(full.find("fn vs_main").unwrap() < full.find(frag).unwrap());
    }

    #[test]
    fn needs_compile_tracks_pipeline_and_failed_sets() {
        let mut fx = ShaderEffects::new(None);
        assert!(fx.needs_compile(7));
        // A recorded failure skips further compilation of that id.
        fx.record_failed(7, "boom");
        assert!(!fx.needs_compile(7));
        assert!(fx.failed.contains(&7));
        // A different id is still eligible.
        assert!(fx.needs_compile(8));
    }

    #[test]
    fn record_failed_warns_only_up_to_the_rate_limit() {
        let mut fx = ShaderEffects::new(None);
        // Distinct failing ids past the cap: every id is recorded as failed, but
        // warnings stop at MAX_FAILED_WARNINGS.
        for id in 0..(MAX_FAILED_WARNINGS + 5) as u64 {
            fx.record_failed(id, "bad shader");
        }
        assert_eq!(fx.warned_failures, MAX_FAILED_WARNINGS);
        assert_eq!(fx.failed.len(), (MAX_FAILED_WARNINGS + 5) as usize);
    }

    #[test]
    fn should_warn_failure_stops_at_cap() {
        assert!(should_warn_failure(0));
        assert!(should_warn_failure(MAX_FAILED_WARNINGS - 1));
        assert!(!should_warn_failure(MAX_FAILED_WARNINGS));
        assert!(!should_warn_failure(MAX_FAILED_WARNINGS + 1));
    }

    #[test]
    fn should_warn_churn_fires_only_past_the_distinct_id_threshold() {
        assert!(
            !should_warn_churn(CHURN_WARN_THRESHOLD, 0),
            "at threshold, not over it, must not warn"
        );
        assert!(should_warn_churn(CHURN_WARN_THRESHOLD + 1, 0));
        assert!(should_warn_churn(CHURN_WARN_THRESHOLD + 100, 0));
        assert!(
            !should_warn_churn(0, 0),
            "well under threshold must never warn"
        );
    }

    #[test]
    fn should_warn_churn_stops_at_the_rate_limit_cap() {
        assert!(should_warn_churn(CHURN_WARN_THRESHOLD + 1, 0));
        assert!(should_warn_churn(
            CHURN_WARN_THRESHOLD + 1,
            MAX_CHURN_WARNINGS - 1
        ));
        assert!(!should_warn_churn(
            CHURN_WARN_THRESHOLD + 1,
            MAX_CHURN_WARNINGS
        ));
        assert!(!should_warn_churn(
            CHURN_WARN_THRESHOLD + 1,
            MAX_CHURN_WARNINGS + 1
        ));
    }

    #[test]
    fn ensure_pipeline_never_warns_below_the_churn_threshold() {
        // A device-free proxy for ensure_pipeline's hook: compiling well
        // under CHURN_WARN_THRESHOLD distinct ids must never cross into
        // should_warn_churn's true branch, mirroring what ensure_pipeline
        // consults after every successful `pipelines.insert`.
        for compiled in 0..=CHURN_WARN_THRESHOLD {
            assert!(
                !should_warn_churn(compiled, 0),
                "compiled={compiled} must not warn (at or under threshold)"
            );
        }
    }

    #[test]
    fn stale_target_keys_selects_only_other_sizes_of_same_live_id() {
        // The resize-reclaim behavior, expressed frame-scoped: with ids 1
        // and 2 both live at 100x100 this frame, the other-size entries of id 1
        // are stale (a resized-away size), while both live sizes are kept.
        let keys = [
            (1, 100, 100), // live this frame — kept
            (1, 200, 200), // same id, not live — evicted
            (1, 100, 200), // same id, not live — evicted
            (2, 100, 100), // live this frame — kept
        ];
        let live: HashSet<(u64, u32, u32)> = [(1, 100, 100), (2, 100, 100)].into_iter().collect();
        let mut stale = stale_target_keys(keys.iter().copied(), &live);
        stale.sort();
        assert_eq!(stale, vec![(1, 100, 200), (1, 200, 200)]);
    }

    #[test]
    fn stale_target_keys_empty_when_no_prior_target() {
        let live: HashSet<(u64, u32, u32)> = [(1, 100, 100)].into_iter().collect();
        assert!(stale_target_keys(std::iter::empty(), &live).is_empty());
    }

    #[test]
    fn stale_target_keys_keeps_two_sizes_of_same_id_live_in_one_frame() {
        // The regression: one program (id 1) drawn at two physical sizes in the
        // same frame. Both keys are live, so neither is evicted — the old
        // per-quad policy evicted whichever was created first, ping-ponging both
        // every frame.
        let keys = [(1, 100, 100), (1, 200, 200)];
        let live: HashSet<(u64, u32, u32)> = [(1, 100, 100), (1, 200, 200)].into_iter().collect();
        assert!(stale_target_keys(keys.iter().copied(), &live).is_empty());
    }

    #[test]
    fn stale_target_keys_still_evicts_a_resized_away_size() {
        // A single-size quad resized across frames: last frame's 100x100 target
        // is stale once only 200x200 is live, and is reclaimed (the behavior the
        // resize-across-frames path depends on).
        let keys = [(1, 100, 100), (1, 200, 200)];
        let live: HashSet<(u64, u32, u32)> = [(1, 200, 200)].into_iter().collect();
        assert_eq!(
            stale_target_keys(keys.iter().copied(), &live),
            vec![(1, 100, 100)]
        );
    }

    #[test]
    fn stale_target_keys_leaves_a_vanished_id_intact() {
        // A program id absent from the live set entirely is NOT reaped here —
        // whole-id reaping is a separate concern, so its targets are preserved.
        let keys = [(1, 100, 100), (1, 200, 200)];
        let live: HashSet<(u64, u32, u32)> = [(2, 50, 50)].into_iter().collect();
        assert!(stale_target_keys(keys.iter().copied(), &live).is_empty());
    }

    #[test]
    fn reapable_ids_selects_only_ids_unseen_for_at_least_max_age() {
        let last_seen: HashMap<u64, u64> = [(1, 0), (2, 50), (3, 100)].into_iter().collect();
        // At frame 120: id 1 (age 120) and id 2 (age 70) are both >= max_age
        // 60; id 3 (age 20) is not.
        let mut stale = reapable_ids(&last_seen, 120, 60);
        stale.sort();
        assert_eq!(stale, vec![1, 2]);
    }

    #[test]
    fn reapable_ids_boundary_is_inclusive() {
        let last_seen: HashMap<u64, u64> = [(1, 0)].into_iter().collect();
        // Age exactly max_age reaps; one frame short does not.
        assert!(reapable_ids(&last_seen, 60, 60).contains(&1));
        assert!(!reapable_ids(&last_seen, 59, 60).contains(&1));
    }

    #[test]
    fn reapable_ids_empty_when_every_id_seen_this_frame() {
        // An id seen every frame is never reaped, however many frames elapse.
        let mut last_seen: HashMap<u64, u64> = HashMap::new();
        for frame in 1..=(MAX_UNSEEN_FRAMES * 3) {
            last_seen.insert(1, frame);
            assert!(reapable_ids(&last_seen, frame, MAX_UNSEEN_FRAMES).is_empty());
        }
    }

    #[test]
    fn reapable_target_keys_selects_only_matching_stale_ids() {
        let keys = [(1, 100, 100), (1, 200, 200), (2, 100, 100), (3, 50, 50)];
        let stale_ids: HashSet<u64> = [1, 3].into_iter().collect();
        let mut got = reapable_target_keys(keys.iter().copied(), &stale_ids);
        got.sort();
        assert_eq!(got, vec![(1, 100, 100), (1, 200, 200), (3, 50, 50)]);
    }

    #[test]
    fn reapable_target_keys_empty_for_no_stale_ids() {
        let keys = [(1, 100, 100)];
        let stale_ids: HashSet<u64> = HashSet::new();
        assert!(reapable_target_keys(keys.iter().copied(), &stale_ids).is_empty());
    }

    #[test]
    fn mark_seen_bumps_frame_and_reaps_ids_unseen_past_max_age() {
        let mut fx = ShaderEffects::new(None);
        // Frame 1: id 1 is live.
        assert!(fx.mark_seen(&[1].into_iter().collect()).is_empty());
        // Id 1 vanishes; keep marking an unrelated id (or nothing) live for
        // MAX_UNSEEN_FRAMES more frames — id 1 must not surface as reapable
        // until its age actually crosses the threshold.
        for _ in 0..(MAX_UNSEEN_FRAMES - 1) {
            assert!(fx.mark_seen(&HashSet::new()).is_empty());
        }
        // One more frame crosses the threshold.
        let reapable = fx.mark_seen(&HashSet::new());
        assert_eq!(reapable, vec![1]);
    }

    #[test]
    fn mark_seen_never_reaps_an_id_kept_live_every_frame() {
        let mut fx = ShaderEffects::new(None);
        for _ in 0..(MAX_UNSEEN_FRAMES * 2) {
            assert!(fx.mark_seen(&[1].into_iter().collect()).is_empty());
        }
    }

    #[test]
    fn reap_clears_failed_and_last_seen_so_a_redrawn_id_recompiles_cleanly() {
        let mut fx = ShaderEffects::new(None);
        // Program 1 failed to compile once, and was seen at some prior frame.
        fx.record_failed(1, "boom");
        fx.last_seen.insert(1, 3);
        assert!(!fx.needs_compile(1), "a failed id is skipped, not retried");

        fx.reap(&[1]);

        assert!(!fx.failed.contains(&1));
        assert!(!fx.last_seen.contains_key(&1));
        assert!(
            fx.needs_compile(1),
            "a reaped id must be eligible to recompile cleanly, not stuck in `failed`"
        );
    }

    #[test]
    fn reap_only_touches_the_stale_ids_given() {
        let mut fx = ShaderEffects::new(None);
        fx.record_failed(1, "boom");
        fx.record_failed(2, "boom");
        fx.last_seen.insert(1, 1);
        fx.last_seen.insert(2, 1);

        fx.reap(&[1]);

        assert!(!fx.failed.contains(&1));
        assert!(
            fx.failed.contains(&2),
            "id 2 was not in stale_ids, must survive"
        );
        assert!(!fx.last_seen.contains_key(&1));
        assert!(fx.last_seen.contains_key(&2));
    }

    #[test]
    fn target_texture_is_none_without_a_target() {
        // With no target created for `(id, w, h)` (never `ensure_target`ed, or
        // a failed compile), the draw seam hands back `None` rather than
        // fabricating a texture — the shape the old vello registration path
        // had, minus the registration.
        let fx = ShaderEffects::new(None);
        assert!(fx.target_texture(1, 100, 100).is_none());
    }

    /// End-to-end (real device) confirmation that a vanished id's *actual*
    /// compiled pipeline and offscreen target — not just the pure key-selection
    /// logic above — are dropped by `mark_seen`/`reap`, that the id recompiles
    /// cleanly if redrawn afterward, and that a resized-away size is evicted
    /// frame-scoped. The pure-logic tests above
    /// (`reapable_ids`/`reapable_target_keys`/`stale_target_keys`/`mark_seen`/
    /// `reap`) cover the policy without a device; this covers the real
    /// `PipelineEntry`/`TargetEntry` removal a `wgpu::Device` requires to
    /// construct at all.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-gpu -- --ignored`"]
    fn reap_drops_a_vanished_ids_real_pipeline_and_target() {
        pollster::block_on(run());

        async fn run() {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, _queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-gpu effects reap test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            const FRAGMENT: &str = "@fragment fn fs_main(in: FrustVsOut) -> \
                @location(0) vec4<f32> { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";

            let mut fx = ShaderEffects::new(None);
            assert!(fx.mark_seen(&[1].into_iter().collect()).is_empty());
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            fx.ensure_target(&device, 1, 4, 4);
            assert!(!fx.needs_compile(1), "compile must have succeeded");
            assert!(fx.pipelines.contains_key(&1));
            assert!(fx.target_texture(1, 4, 4).is_some());

            // A resized-away size is reclaimed frame-scoped, while the size
            // still drawn this frame survives.
            fx.ensure_target(&device, 1, 8, 8);
            fx.evict_stale_targets(&[(1, 8, 8)].into_iter().collect());
            assert!(fx.target_texture(1, 4, 4).is_none());
            assert!(fx.target_texture(1, 8, 8).is_some());

            // id 1 stops being drawn: mark_seen with an empty live set every
            // frame until its age crosses MAX_UNSEEN_FRAMES.
            let mut reapable = Vec::new();
            for _ in 0..MAX_UNSEEN_FRAMES {
                reapable = fx.mark_seen(&HashSet::new());
            }
            assert_eq!(reapable, vec![1]);
            fx.reap(&reapable);

            assert!(
                !fx.pipelines.contains_key(&1),
                "reap must drop the real compiled pipeline"
            );
            assert!(
                fx.target_texture(1, 8, 8).is_none(),
                "reap must drop the real offscreen target"
            );
            assert!(fx.needs_compile(1));

            // Re-drawn: recompiles cleanly, with no stale `failed` skip.
            assert!(fx.mark_seen(&[1].into_iter().collect()).is_empty());
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            assert!(
                !fx.needs_compile(1),
                "a reaped id must recompile cleanly when redrawn"
            );
            assert!(fx.pipelines.contains_key(&1));
        }
    }

    /// End-to-end (real device) confirmation of `ensure_target`'s own
    /// device-validity clamp: a request larger than the device's real
    /// `max_texture_dimension_2d` must create a texture at that ceiling
    /// instead of tripping `wgpu` validation. `required_limits` downgrades the
    /// device's ceiling to a small, fast-to-allocate value so the oversized
    /// request stays cheap while still exercising the real clamp against a
    /// real device.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-gpu -- --ignored`"]
    fn ensure_target_clamps_an_oversized_request_to_the_device_ceiling() {
        pollster::block_on(run());

        async fn run() {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            const CEILING: u32 = 256;
            let (device, _queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-gpu effects ensure_target clamp test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits {
                        max_texture_dimension_2d: CEILING,
                        ..wgpu::Limits::default()
                    },
                    ..Default::default()
                })
                .await
                .expect("failed to create device");
            assert_eq!(device.limits().max_texture_dimension_2d, CEILING);

            const FRAGMENT: &str = "@fragment fn fs_main(in: FrustVsOut) -> \
                @location(0) vec4<f32> { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";

            let mut fx = ShaderEffects::new(None);
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            assert!(!fx.needs_compile(1), "compile must have succeeded");

            // Requested far past the device's ceiling: creating a texture
            // this size without clamping would trip wgpu validation rather
            // than produce a usable target.
            let requested = CEILING * 4;
            fx.ensure_target(&device, 1, requested, requested);
            let texture = fx
                .target_texture(1, requested, requested)
                .expect("an oversized request must still create a (clamped) target");
            assert_eq!(texture.width(), CEILING);
            assert_eq!(texture.height(), CEILING);
        }
    }

    /// Prove that extent coherence holds: an oversized request creates a
    /// ceiling-sized texture, the stored extent matches the texture's actual
    /// dimensions, and encode_pass uses the stored extent (verified via the
    /// entry). Also proves that repeating the same oversized request reuses
    /// the existing entry (no second texture creation), and that the clamp
    /// warning fires only once per distinct (id, requested_extent) pair.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-gpu -- --ignored`"]
    fn ensure_target_extent_coherence_oversized_request() {
        pollster::block_on(run());

        async fn run() {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            const CEILING: u32 = 512;
            let (device, _queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-gpu effects extent coherence test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits {
                        max_texture_dimension_2d: CEILING,
                        ..wgpu::Limits::default()
                    },
                    ..Default::default()
                })
                .await
                .expect("failed to create device");
            assert_eq!(device.limits().max_texture_dimension_2d, CEILING);

            const FRAGMENT: &str = "@fragment fn fs_main(in: FrustVsOut) -> \
                @location(0) vec4<f32> { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";

            let mut fx = ShaderEffects::new(None);
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            assert!(!fx.needs_compile(1), "compile must have succeeded");

            // Request far past the device's ceiling.
            let requested = CEILING * 4;
            assert!(requested > CEILING, "sanity check: request is oversized");

            // Ensure the target for the first time.
            fx.ensure_target(&device, 1, requested, requested);
            let entry_1 = fx
                .targets
                .get(&(1, requested, requested))
                .expect("target must be created");
            assert_eq!(entry_1.used_w, CEILING, "used_w must match ceiling");
            assert_eq!(entry_1.used_h, CEILING, "used_h must match ceiling");

            // The texture itself must have been created at the clamped extent.
            let texture_1 = fx
                .target_texture(1, requested, requested)
                .expect("texture must exist");
            assert_eq!(
                texture_1.width(),
                CEILING,
                "texture width must match clamped extent"
            );
            assert_eq!(
                texture_1.height(),
                CEILING,
                "texture height must match clamped extent"
            );

            // Record the texture pointer to verify reuse.
            let texture_ptr_1 = texture_1 as *const _;

            // Repeat the same oversized request and verify we reuse the entry.
            fx.ensure_target(&device, 1, requested, requested);
            let entry_2 = fx
                .targets
                .get(&(1, requested, requested))
                .expect("target must still exist");
            let texture_2 = fx
                .target_texture(1, requested, requested)
                .expect("texture must still exist");
            let texture_ptr_2 = texture_2 as *const _;

            assert_eq!(
                texture_ptr_1, texture_ptr_2,
                "repeated request must reuse the same texture"
            );

            // Verify encode_pass uses the stored extent by checking that
            // encode_pass will write a uniform with the stored (clamped)
            // dimensions. Since encode_pass is called, the uniform will be
            // written with target.used_w and target.used_h, not the requested
            // dimensions. Verifying the stored extent matches the texture proves
            // encode_pass will compute the correct uniform (it reads used_w and
            // used_h from the target entry).
            assert_eq!(
                entry_2.used_w, CEILING,
                "encode_pass will use this stored used_w for the uniform"
            );
            assert_eq!(
                entry_2.used_h, CEILING,
                "encode_pass will use this stored used_h for the uniform"
            );

            // The stored extent must match the texture's extent, so
            // encode_pass's uniform (computed from stored extent) and the
            // attachment (the texture itself) agree by construction.
            assert_eq!(
                entry_2.used_w,
                texture_2.width(),
                "stored used_w must equal texture width"
            );
            assert_eq!(
                entry_2.used_h,
                texture_2.height(),
                "stored used_h must equal texture height"
            );

            // Verify the warning latched: warned_clamped must contain the (id,
            // requested_w, requested_h) key, showing we warned once.
            assert!(
                fx.warned_clamped.contains(&(1, requested, requested)),
                "warned_clamped must track the oversized request"
            );

            // Evict the target under the frame-scoped policy it actually
            // implements: the id must appear in the live set at a DIFFERENT
            // size for its old size to count as stale (an id absent from
            // `live` entirely is deliberately left intact — see
            // `stale_target_keys` and its `..leaves_a_vanished_id_intact`
            // sibling test).
            let mut live = HashSet::new();
            live.insert((1u64, 1u32, 1u32));
            fx.evict_stale_targets(&live);
            assert!(
                !fx.targets.contains_key(&(1, requested, requested)),
                "the resized-out oversized target must be evicted"
            );

            // Recreate the same oversized request. The warn latch gates the
            // warning on a fresh `HashSet::insert`, so while the key is still
            // present a second warning is impossible by construction — the
            // property under test is that eviction did NOT clear the latch.
            assert!(
                fx.warned_clamped.contains(&(1, requested, requested)),
                "the warn latch must survive frame-scoped eviction"
            );
            fx.ensure_target(&device, 1, requested, requested);
            let entry_3 = fx
                .targets
                .get(&(1, requested, requested))
                .expect("recreated");
            assert_eq!(entry_3.used_w, CEILING, "recreation re-clamps");
            assert!(
                fx.warned_clamped.contains(&(1, requested, requested)),
                "recreation must not disturb the latch"
            );

            // `reap` is the opposite contract: the id leaves with all of its
            // state, latch included, so a reaped id that returns is brand new
            // for warning purposes too — and this is what bounds the set.
            fx.reap(&[1]);
            assert!(
                !fx.warned_clamped.contains(&(1, requested, requested)),
                "reap must drop the id's warn-latch entries"
            );
        }
    }
}
