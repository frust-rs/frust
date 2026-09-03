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
//! - **Per-`(program, quantized size)` target state**
//!   ([`ShaderEffects::ensure_target`]): an offscreen texture + view, a
//!   16-byte uniform buffer, and the bind group binding them, keyed by
//!   `(id, w, h)` where `w`/`h` are [`quantized_target_key`]'s output rather
//!   than the raw request. Quantizing before the key means a quad resizing
//!   pixel-by-pixel (a window drag, an animated scale) reuses one texture
//!   across an entire 256px band instead of minting a fresh one every frame —
//!   see [`quantized_target_key`]'s own doc for why 256px, the same quantum
//!   [`crate::pool::TexturePool`] already uses for exactly this reason.
//! - **Fullscreen-triangle pass encoding** ([`ShaderEffects::encode_pass`]),
//!   confined by `wgpu`'s viewport to the *requested* sub-rect of the
//!   (possibly larger, quantized) target: the shader always sees
//!   `frust_u.resolution` as the caller's exact requested extent, never the
//!   quantized texture's true size, so a quad samples back a target sized to
//!   itself even while sharing a texture with nearby sizes.
//! - **Age-based whole-id reap** ([`ShaderEffects::mark_seen`]/[`ShaderEffects::reap`]):
//!   a program id absent from every frame's live id set for
//!   [`MAX_UNSEEN_FRAMES`] consecutive frames has its pipeline, target(s), and
//!   any recorded compile failure dropped, rather than living until surface
//!   teardown.
//! - **Age-based per-target reap**, the same [`ShaderEffects::mark_seen`]
//!   call's other half: a `(id, quantized w, quantized h)` key absent from
//!   every frame's live *target* key set for [`MAX_UNSEEN_TARGET_FRAMES`]
//!   consecutive frames — shorter than the whole-id window, since a size a
//!   still-drawn program has resized away from is a narrower, more frequent
//!   event than the program vanishing outright — is dropped on its own,
//!   independent of its id's own age. Replaces an earlier same-frame-scoped
//!   eviction policy that reclaimed any other-size same-id target the moment
//!   a frame's live key set no longer named it; that policy fought
//!   quantization directly, since two nearby (but not identical) requests
//!   that shared one quantized texture would otherwise evict each other
//!   every single frame.
//! - **Churn detection** ([`should_warn_churn`], consulted by
//!   [`ShaderEffects::ensure_pipeline`]): a rate-limited `log::warn!` once the
//!   live compiled-program count crosses [`CHURN_WARN_THRESHOLD`] — a
//!   detection aid pointing at the `ShaderProgram::new` cache-once contract,
//!   not itself a bound on growth; the age-based reap above is what actually
//!   bounds it.
//! - **Per-id target count bound** ([`MAX_TARGETS_PER_ID`]): a program id may
//!   hold at most this many quantized target keys at once, evicting its own
//!   least-recently-seen key immediately — before the age-based reap above
//!   ever gets a turn — the moment a new key would push it past the cap. This
//!   bounds a single frame's worth of accumulation (a multi-band resize drag
//!   minting one key per band per frame) independent of the age window: the
//!   accepted peak per program is `MAX_TARGETS_PER_ID` × (the largest
//!   quantized target area it currently holds) × 4 bytes/texel, e.g. two
//!   1024×1024 `Rgba8Unorm` targets is 8 MiB, not the unbounded run of bands a
//!   fast drag could otherwise mint before any of them aged out.
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
//! read-back — the whole (possibly larger, quantized) attachment, never only
//! the sub-rect actually rendered into it. [`ShaderEffects::target_extent`]
//! answers with that sub-rect instead: the requested, device-clamped extent
//! actually rendered this call — the extent a registration must state — never
//! the whole texture's own (larger, quantized) extent, which is never itself
//! exposed for registration. Nothing is registered with a foreign renderer and
//! nothing is handed back on eviction: the pool owns the texture and a
//! consumer borrows it.
//!
//! ## Target identity
//!
//! A `targets` entry also carries a **generation**
//! ([`ShaderEffects::target_generation`]): a per-key counter bumped every time
//! [`ShaderEffects::ensure_target`] actually creates a new texture at that
//! key, rather than reusing an existing one. A caller that only compares
//! `(id, requested extent)` to decide whether to re-register cannot tell a
//! steady-state target apart from one silently recreated behind it — the
//! per-target reap above (or the per-id cap just above it) can drop and
//! recreate a target at the identical requested extent between two frames,
//! and a registration keyed on extent alone would then keep sampling the
//! *old* (still-alive, now-orphaned) texture forever. Comparing generation
//! too is what lets this crate's caller
//! (`frust_engine::effects::shader_quad::ShaderQuadPass::register`) tell the
//! two cases apart and rebind whenever either the extent or the generation
//! changed.
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

use crate::pool::{DEFAULT_MAX_UNUSED_FRAMES, quantize_extent};

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

/// How many consecutive frames a target *key* — `(program id, quantized
/// width, quantized height)`, one entry of [`ShaderEffects::ensure_target`]'s
/// own map — may go absent from every frame's live target-key set (see
/// [`ShaderEffects::mark_seen`]) before its GPU resources are dropped on
/// their own, independent of whether the program `id` itself is still being
/// drawn.
///
/// Deliberately shorter than [`MAX_UNSEEN_FRAMES`] and reused directly from
/// [`crate::pool::DEFAULT_MAX_UNUSED_FRAMES`] (60, ~1s at 60Hz) rather than a
/// third bespoke number: a size a still-live program has resized away from is
/// exactly the same shape [`crate::pool::TexturePool`]'s own aging window was
/// chosen for — a resize drag that revisits a quantized size seconds later
/// should still find it parked — while a whole program going silent (the
/// wider [`MAX_UNSEEN_FRAMES`] window) is a rarer, coarser event worth a more
/// generous grace period.
pub const MAX_UNSEEN_TARGET_FRAMES: u64 = DEFAULT_MAX_UNUSED_FRAMES;

/// How many quantized target keys one program `id` may hold resident at once,
/// enforced immediately by [`ShaderEffects::ensure_target`] the moment a new
/// key would push `id` past it — evicting `id`'s own least-recently-seen key
/// ([`oldest_target_key_for_id`]) before minting the new one, rather than
/// waiting for [`MAX_UNSEEN_TARGET_FRAMES`]'s age-based reap to catch up.
///
/// 2 — the current quantized band and the immediately preceding one — is
/// chosen so a resize drag straddling one 256px boundary back and forth does
/// not thrash a target it only just evicted, while a drag through many bands
/// in one frame (an animated scale snapping across a wide range, a
/// multi-band resize) cannot accumulate one target per band with no pressure:
/// [`MAX_UNSEEN_TARGET_FRAMES`]'s window alone would let such a sweep mint
/// dozens of resident targets before any of them aged out. See the module
/// header's "Per-id target count bound" bullet for the accepted peak this
/// bounds.
pub const MAX_TARGETS_PER_ID: usize = 2;

/// The `(width, height)` a shader-effect target requested at `(w, h)` is
/// actually keyed and created at: `w`/`h` quantized up to the next multiple
/// of [`crate::pool::SIZE_QUANTUM`] (256px, via [`quantize_extent`]) and
/// never past `ceiling` (the caller's own device-validity floor —
/// [`ShaderEffects::ensure_target`] passes the device's real
/// `max_texture_dimension_2d`).
///
/// Reusing the pool's own quantum rather than inventing a second one means a
/// shader-quad target collapses a pixel-by-pixel resize drag onto the same
/// handful of keys `frust_gpu::pool::TexturePool` already collapses every
/// other scratch texture onto, so [`ShaderEffects::ensure_target`] mints a
/// fresh texture only when a request crosses a 256px boundary instead of on
/// every frame a quad's destination fractionally changes. Both axes are
/// floored at 1 (a zero-sized texture is invalid) and the result never
/// exceeds `ceiling`: [`quantize_extent`] alone passes an already-oversized
/// request through unclamped (so its own caller can detect and warn about
/// the clamp — see [`ShaderEffects::ensure_target`]'s doc comment), so the
/// final `min` here is what actually keeps a shader-effect target within the
/// device's real limit.
#[must_use]
pub fn quantized_target_key(w: u32, h: u32, ceiling: u32) -> (u32, u32) {
    let ceiling = ceiling.max(1);
    let (quantized_w, quantized_h) = quantize_extent(w.max(1), h.max(1), ceiling);
    (quantized_w.min(ceiling), quantized_h.min(ceiling))
}

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

/// The per-`(program, quantized size)` GPU state a shader effect renders
/// into: an offscreen `Rgba8Unorm` texture (`RENDER_ATTACHMENT |
/// TEXTURE_BINDING | COPY_SRC` — the attachment the pass writes, the binding
/// a consumer samples it through, and the copy source a read-back needs),
/// its view, the 16-byte uniform buffer, and the bind group wiring the
/// buffer to `@binding(0)`.
///
/// `used_w`/`used_h` are the *quantized*, device-ceiling-clamped extent the
/// texture was actually created at ([`quantized_target_key`]) — usually
/// larger than any single caller's requested size, which is why
/// [`ShaderEffects::encode_pass`] renders into a sub-rect of it rather than
/// the whole attachment, and [`ShaderEffects::target_extent`] answers with
/// that sub-rect rather than `used_w`/`used_h` themselves.
struct TargetEntry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The quantized, device-clamped width this entry's texture was actually
    /// created at.
    used_w: u32,
    /// The quantized, device-clamped height this entry's texture was
    /// actually created at.
    used_h: u32,
    /// This entry's own generation — see [`ShaderEffects::target_generation`]
    /// and the module header's "Target identity" section. Stamped once, from
    /// the struct's own generation counter, when the entry is created and
    /// never changed afterward; a key re-created later (after an
    /// eviction/reap) gets a fresh, strictly greater value.
    generation: u64,
}

/// Owns every GPU resource the shader-showcase feature needs: lazily-compiled
/// per-program pipelines and per-`(program, quantized size)` offscreen
/// targets.
pub struct ShaderEffects {
    /// Compiled pipelines, keyed by program id.
    pipelines: HashMap<u64, PipelineEntry>,
    /// Offscreen targets, keyed by `(program id, quantized width, quantized
    /// height)` — see [`quantized_target_key`].
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
    /// frame's live id set, updated by [`Self::mark_seen`] for every id
    /// passed in — including one whose pipeline failed to compile, so a
    /// broken shader's `failed` entry can still age out. An id absent from
    /// this map has never been seen (or was already reaped).
    last_seen: HashMap<u64, u64>,
    /// The frame (per `Self::frame`) each target *key* — `(program id,
    /// quantized width, quantized height)`, a `targets` key — was last
    /// present in a frame's live target-key set, updated by
    /// [`Self::mark_seen`] and by [`Self::ensure_target`] itself (stamped at
    /// creation, so a target created and never subsequently marked seen in
    /// the same call still has a valid age to start from rather than aging
    /// immediately). A key absent from this map has never been created (or
    /// was already reaped) — see [`MAX_UNSEEN_TARGET_FRAMES`].
    target_last_seen: HashMap<(u64, u32, u32), u64>,
    /// Requested extents for which we have already warned about clamping. Keyed
    /// by `(program id, requested width, requested height)` so the clamp warning
    /// fires at most once per distinct oversized request, surviving the
    /// target's own age-based reap (a re-created target at the same requested
    /// extent does not re-warn). [`Self::reap`] drops an id's entries with the
    /// rest of its state, which is both the "brand new again" contract and the
    /// bound on this set's growth.
    warned_clamped: HashSet<(u64, u32, u32)>,
    /// The generation the next `targets` entry [`Self::ensure_target`] creates
    /// will be stamped with — monotonically incremented on every actual
    /// texture creation (never on a cache hit), so two entries ever alive at
    /// the same key over time carry strictly increasing values. See
    /// [`TargetEntry::generation`] and the module header's "Target identity"
    /// section.
    next_generation: u64,
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

/// The target keys in `target_last_seen` whose last-seen frame is at least
/// `max_age` frames behind `current_frame` — the target-level counterpart of
/// [`reapable_ids`], keyed by `(id, quantized w, quantized h)` rather than by
/// program id alone. Pure: operates on the age map alone (no GPU state), so
/// the per-target reap policy is unit-testable exactly like `reapable_ids`.
///
/// Age-based rather than frame-scoped is what lets two distinct (quantized)
/// sizes of the same still-drawn program coexist within one frame: both keys
/// are marked seen by the same [`ShaderEffects::mark_seen`] call, so neither
/// ages — the two-sizes-one-frame case an earlier, frame-scoped eviction
/// policy had to special-case explicitly (evicting any other-size same-id
/// entry not named that exact frame) now falls out of the age clock by
/// construction. A size a quad has resized away from simply stops being
/// refreshed and ages out over [`MAX_UNSEEN_TARGET_FRAMES`] frames instead of
/// being reclaimed the instant it is not asked for.
fn reapable_target_ages(
    target_last_seen: &HashMap<(u64, u32, u32), u64>,
    current_frame: u64,
    max_age: u64,
) -> Vec<(u64, u32, u32)> {
    target_last_seen
        .iter()
        .filter(|&(_, &seen)| current_frame.saturating_sub(seen) >= max_age)
        .map(|(&key, _)| key)
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
/// exactly like [`reapable_target_ages`].
///
/// A vanished id (one whose shader quad stops appearing in the scene entirely
/// — screen navigation, a dynamic id, etc.) is a distinct case from a
/// resized-away *size* of a still-drawn id, which
/// [`reapable_target_ages`] reaps on its own (shorter) clock; this function
/// is the whole-id counterpart, aged on [`MAX_UNSEEN_FRAMES`] rather than
/// [`MAX_UNSEEN_TARGET_FRAMES`].
fn reapable_ids(last_seen: &HashMap<u64, u64>, current_frame: u64, max_age: u64) -> Vec<u64> {
    last_seen
        .iter()
        .filter(|&(_, &seen)| current_frame.saturating_sub(seen) >= max_age)
        .map(|(&id, _)| id)
        .collect()
}

/// The `targets` keys [`ShaderEffects::reap`] should remove for a given
/// `stale_ids` list — every key whose id is one of them. Pure key-selection,
/// mirroring [`reapable_target_ages`]'s shape, so `reap`'s target-removal
/// choice is unit-testable without a `wgpu::Device` (a real `TargetEntry`
/// can't be constructed without one).
fn reapable_target_keys<I>(target_keys: I, stale_ids: &HashSet<u64>) -> Vec<(u64, u32, u32)>
where
    I: IntoIterator<Item = (u64, u32, u32)>,
{
    target_keys
        .into_iter()
        .filter(|key| stale_ids.contains(&key.0))
        .collect()
}

/// The `targets` key belonging to `id` with the oldest `target_last_seen`
/// entry, or `None` if `id` holds none — the eviction candidate
/// [`ShaderEffects::ensure_target`] drops immediately when minting a new key
/// would push `id` past [`MAX_TARGETS_PER_ID`]. Pure: operates on the age map
/// alone, mirroring [`reapable_target_ages`]'s shape, so the eviction choice
/// is unit-testable without a device by simulating a sweep of mints against
/// `target_last_seen` alone. Ties (equal `seen` values) resolve to the key
/// [`HashMap::iter`] happens to visit last among them — the cap-enforcement
/// call site never mints two keys for one id in the same frame, so a tie
/// among genuinely distinct ages does not arise in practice.
fn oldest_target_key_for_id(
    target_last_seen: &HashMap<(u64, u32, u32), u64>,
    id: u64,
) -> Option<(u64, u32, u32)> {
    target_last_seen
        .iter()
        .filter(|&(&(key_id, _, _), _)| key_id == id)
        .min_by_key(|&(_, &seen)| seen)
        .map(|(&key, _)| key)
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
            target_last_seen: HashMap::new(),
            warned_clamped: HashSet::new(),
            next_generation: 0,
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
    /// the new frame, and every key in `live_target_keys` (this frame's
    /// distinct `(id, quantized w, quantized h)` target keys — see
    /// [`quantized_target_key`]) as last seen the same way. Ages out and
    /// drops any target key that has now gone unseen for at least
    /// [`MAX_UNSEEN_TARGET_FRAMES`] frames (its own, shorter clock —
    /// [`reapable_target_ages`]), then returns every program id that has now
    /// gone unseen for at least [`MAX_UNSEEN_FRAMES`] frames (via
    /// [`reapable_ids`]), ready to hand to [`Self::reap`].
    ///
    /// Called once per frame regardless of whether either set is empty — a
    /// scene that stops drawing shader quads entirely must still age out and
    /// eventually reap every previously-seen id and target, not just ones
    /// still present.
    pub fn mark_seen(
        &mut self,
        live_ids: &HashSet<u64>,
        live_target_keys: &HashSet<(u64, u32, u32)>,
    ) -> Vec<u64> {
        self.frame += 1;
        for &id in live_ids {
            self.last_seen.insert(id, self.frame);
        }
        for &key in live_target_keys {
            self.target_last_seen.insert(key, self.frame);
        }
        for key in
            reapable_target_ages(&self.target_last_seen, self.frame, MAX_UNSEEN_TARGET_FRAMES)
        {
            self.targets.remove(&key);
            self.target_last_seen.remove(&key);
        }
        reapable_ids(&self.last_seen, self.frame, MAX_UNSEEN_FRAMES)
    }

    /// Reap every resource for each id in `stale_ids` (a [`Self::mark_seen`]
    /// output): every `targets` entry whose key's id matches (and its
    /// `target_last_seen` row), plus the compiled `pipelines` entry, any
    /// `failed` record, and the `last_seen` row itself. Dropping `failed`
    /// alongside the rest means a program id that reappears after being
    /// reaped is treated as brand new: it recompiles cleanly rather than
    /// hitting a stale skip from a compile failure that happened frames ago
    /// (or never happened at all). The clamp-warning latch is dropped on the
    /// same trigger: a reaped id that returns warns afresh on its first
    /// oversized request, exactly like a brand-new program — and this reap is
    /// also what bounds the latch set's growth, the same way it bounds every
    /// other map here.
    pub fn reap(&mut self, stale_ids: &[u64]) {
        let stale_id_set: HashSet<u64> = stale_ids.iter().copied().collect();
        for key in reapable_target_keys(self.targets.keys().copied(), &stale_id_set) {
            self.targets.remove(&key);
        }
        self.target_last_seen
            .retain(|key, _| !stale_id_set.contains(&key.0));
        self.warned_clamped
            .retain(|key| !stale_id_set.contains(&key.0));
        for &id in stale_ids {
            self.pipelines.remove(&id);
            self.failed.remove(&id);
            self.last_seen.remove(&id);
        }
    }

    /// Ensure a target exists for `(id, w, h)`, creating it (at
    /// [`quantized_target_key`]'s quantized, device-clamped extent) if
    /// absent. Age-based reclaim of an unseen target — same-quantized-key
    /// requests aside — is handled separately by [`Self::mark_seen`], so two
    /// distinct (quantized) sizes of the same program can coexist across
    /// many frames rather than evicting each other every one.
    ///
    /// A no-op if program `id` has no compiled pipeline (never compiled, or
    /// compile-failed): a target is useless without the pipeline that owns its
    /// bind-group layout, so the caller compiles first. `w`/`h` are quantized
    /// and clamped to the device's own `max_texture_dimension_2d` ceiling
    /// (floored at 1) — a device-validity floor only, not the caller's size
    /// *policy*: a caller may still apply a tighter policy cap of its own
    /// (e.g. `frust_engine::effects::shader_quad`'s 8192) before calling, but
    /// an oversized request that reaches here creates a texture at the
    /// device ceiling instead of tripping `wgpu` validation, and warns once
    /// per distinct oversized `(id, requested_w, requested_h)` pair — the
    /// warning latches the first time this exact request is clamped by the
    /// ceiling (never merely because quantization rounded it up), then
    /// repeats only if the request changes. A freshly created entry's
    /// `target_last_seen` row is stamped at the current frame so it starts
    /// with a valid age even if the caller's own [`Self::mark_seen`] call for
    /// this frame has not run yet, and its `generation` is stamped from
    /// [`Self::next_generation`] — see the module header's "Target identity"
    /// section.
    ///
    /// Before minting a genuinely new key, enforces [`MAX_TARGETS_PER_ID`]:
    /// if `id` already holds the cap's worth of resident keys, its own
    /// least-recently-seen one ([`oldest_target_key_for_id`]) is evicted
    /// immediately, ahead of and independent of [`Self::mark_seen`]'s
    /// age-based reap — see [`MAX_TARGETS_PER_ID`]'s own doc for why.
    pub fn ensure_target(&mut self, device: &wgpu::Device, id: u64, w: u32, h: u32) {
        let (used_w, used_h, ceiling) = Self::target_key(device, w, h);
        let key = (id, used_w, used_h);
        if self.targets.contains_key(&key) {
            return;
        }

        let Some(pipeline_entry) = self.pipelines.get(&id) else {
            return;
        };

        // Warn only when the device's real ceiling actually constrained the
        // result below what was asked — never merely because quantization
        // rounded a request up to its (larger) texture's own size.
        if (w > ceiling || h > ceiling) && self.warned_clamped.insert((id, w, h)) {
            log::warn!(
                "frust-gpu: shader-effect target {id} requested {w}x{h}, clamped to \
                 {used_w}x{used_h} (the device's max_texture_dimension_2d ceiling {ceiling})",
            );
        }

        // A genuinely new key: enforce the per-id resident cap before
        // minting it, evicting the id's own oldest key immediately rather
        // than letting it ride until the age-based reap catches it.
        let resident_for_id = self.targets.keys().filter(|k| k.0 == id).count();
        if resident_for_id >= MAX_TARGETS_PER_ID
            && let Some(evict_key) = oldest_target_key_for_id(&self.target_last_seen, id)
        {
            self.targets.remove(&evict_key);
            self.target_last_seen.remove(&evict_key);
        }

        let generation = self.next_generation;
        self.next_generation += 1;

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
                generation,
            },
        );
        self.target_last_seen.insert(key, self.frame);
    }

    /// Encode one fullscreen-triangle pass for program `id`, requested at
    /// `requested` (the caller's exact device-space extent — never the
    /// quantized target's own, larger size), into its target, writing the
    /// uniform buffer (`resolution`, `time`) first. A no-op if the program's
    /// pipeline or its (quantized) target is missing. Adds no `queue.submit`
    /// — the caller owns encoder creation and submission ordering relative to
    /// the frame's own passes.
    ///
    /// Renders into the **sub-rect** `(0, 0)..requested` of the target via
    /// `wgpu`'s viewport, never the whole (possibly larger, quantized)
    /// attachment: the resolution uniform a shader reads is `requested`
    /// itself (clamped to what the target can actually hold, in the rare
    /// case the device ceiling shrank it below the request), so a quad
    /// samples back content sized exactly to itself even when its target is
    /// shared with, or larger than, other nearby-sized requests.
    pub fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        id: u64,
        requested: (u32, u32),
        time: f32,
    ) {
        let (req_w, req_h) = (requested.0.max(1), requested.1.max(1));
        let (used_w, used_h, _ceiling) = Self::target_key(device, req_w, req_h);
        let (Some(pipeline_entry), Some(target)) = (
            self.pipelines.get(&id),
            self.targets.get(&(id, used_w, used_h)),
        ) else {
            return;
        };

        // The sub-rect this pass actually renders into: the requested
        // extent, never larger than the (quantized, possibly device-clamped)
        // texture actually backing it.
        let render_w = req_w.min(target.used_w);
        let render_h = req_h.min(target.used_h);

        queue.write_buffer(
            &target.uniforms,
            0,
            &uniform_bytes(render_w, render_h, time),
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
        // Confine the fullscreen triangle to the requested sub-rect of the
        // (possibly larger) attachment, so a smaller quad sharing a bigger
        // quantized target only ever writes its own corner.
        pass.set_viewport(0.0, 0.0, render_w as f32, render_h as f32, 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }

    /// The offscreen texture program `id` was rendered into for a request at
    /// `(w, h)` (looked up by [`quantized_target_key`], `device`'s own
    /// ceiling), or `None` if that (quantized) target does not exist (never
    /// [`ensure_target`](Self::ensure_target)ed, or the pipeline compile
    /// failed).
    ///
    /// The read-back seam. A consumer that draws the result instead wants
    /// [`Self::target_view`], since a scene-texture registration takes a view;
    /// nothing is registered with a foreign renderer and nothing is handed
    /// back on eviction — the pool owns the texture and the borrow lives as
    /// long as the caller holds it. The returned texture is the whole
    /// (possibly larger, quantized) attachment — a read-back consumer wanting
    /// only the rendered sub-rect wants [`Self::target_extent`] alongside it.
    pub fn target_texture(
        &self,
        device: &wgpu::Device,
        id: u64,
        w: u32,
        h: u32,
    ) -> Option<&wgpu::Texture> {
        let (used_w, used_h, _ceiling) = Self::target_key(device, w, h);
        self.targets
            .get(&(id, used_w, used_h))
            .map(|entry| &entry.texture)
    }

    /// The full (quantized) extent view of program `id`'s `(w, h)`-requested
    /// target — the handle a consumer registers to sample the rendered result
    /// — or `None` on the same terms as [`Self::target_texture`].
    ///
    /// The same view the pass wrote through, rather than a fresh one per
    /// frame: a view is a handle onto the texture, so creating one per
    /// registration would churn a resource that never changes while its
    /// target lives. This is a whole-texture view even where only the
    /// `(0, 0)..target_extent` sub-rect was actually rendered this frame —
    /// see [`Self::target_extent`] for the sub-rect a registration should
    /// state instead.
    pub fn target_view(
        &self,
        device: &wgpu::Device,
        id: u64,
        w: u32,
        h: u32,
    ) -> Option<&wgpu::TextureView> {
        let (used_w, used_h, _ceiling) = Self::target_key(device, w, h);
        self.targets
            .get(&(id, used_w, used_h))
            .map(|entry| &entry.view)
    }

    /// The extent a `(w, h)` request at program `id` was actually *rendered*
    /// at this call — the sub-rect [`Self::encode_pass`] draws into, not the
    /// (quantized, possibly larger) texture [`Self::target_view`] hands back
    /// — or `None` on the same terms as [`Self::target_texture`] (no
    /// (quantized) target exists for this id/extent at all).
    ///
    /// Not `(w, h)` echoed back unconditionally: in the rare case the
    /// device's real ceiling constrains the target below what was asked
    /// (`w`/`h` past `device`'s `max_texture_dimension_2d`), the answer is
    /// clamped the same way [`Self::encode_pass`] itself clamps what it
    /// renders, so a registration built from this value and the target's
    /// view always agree on what the attachment actually holds at the
    /// caller's requested corner.
    pub fn target_extent(
        &self,
        device: &wgpu::Device,
        id: u64,
        w: u32,
        h: u32,
    ) -> Option<(u32, u32)> {
        let (used_w, used_h, _ceiling) = Self::target_key(device, w, h);
        let target = self.targets.get(&(id, used_w, used_h))?;
        Some((w.max(1).min(target.used_w), h.max(1).min(target.used_h)))
    }

    /// The generation program `id`'s `(w, h)`-requested target was created at
    /// — bumped by [`Self::ensure_target`] every time it actually creates a
    /// new texture at the resolved key, never on a cache hit — or `None` on
    /// the same terms as [`Self::target_texture`] (no target exists for this
    /// id/extent at all).
    ///
    /// The identity half of a registration alongside [`Self::target_extent`]:
    /// two targets can share an identical requested extent yet be genuinely
    /// different GPU resources (one reaped and recreated behind a caller that
    /// only compared extents), and this is what lets a caller (e.g.
    /// `frust_engine::effects::shader_quad::ShaderQuadPass::register`) tell
    /// them apart — see the module header's "Target identity" section.
    #[must_use]
    pub fn target_generation(&self, device: &wgpu::Device, id: u64, w: u32, h: u32) -> Option<u64> {
        let (used_w, used_h, _ceiling) = Self::target_key(device, w, h);
        self.targets
            .get(&(id, used_w, used_h))
            .map(|entry| entry.generation)
    }

    /// Derive the `(quantized w, quantized h, ceiling)` a `(w, h)` request
    /// resolves to on `device`: `device.limits().max_texture_dimension_2d` as
    /// [`quantized_target_key`]'s own ceiling parameter. The single place
    /// every method needing both the resolved key and the raw ceiling
    /// computes them — factored out of the five call sites that used to
    /// repeat this pair (hygiene).
    fn target_key(device: &wgpu::Device, w: u32, h: u32) -> (u32, u32, u32) {
        let ceiling = device.limits().max_texture_dimension_2d;
        let (used_w, used_h) = quantized_target_key(w, h, ceiling);
        (used_w, used_h, ceiling)
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
    fn quantized_target_key_rounds_up_to_the_256px_quantum() {
        assert_eq!(quantized_target_key(1, 1, 8192), (256, 256));
        assert_eq!(quantized_target_key(256, 256, 8192), (256, 256));
        assert_eq!(quantized_target_key(257, 4, 8192), (512, 256));
        assert_eq!(quantized_target_key(800, 300, 8192), (1024, 512));
    }

    #[test]
    fn quantized_target_key_never_exceeds_the_ceiling() {
        assert_eq!(
            quantized_target_key(10_000, 10_000, 2048),
            (2048, 2048),
            "an already-oversized request is clamped to the ceiling here, unlike quantize_extent alone"
        );
        assert_eq!(quantized_target_key(2000, 2000, 2000), (2000, 2000));
    }

    #[test]
    fn quantized_target_key_floors_zero_to_one_quantum() {
        assert_eq!(quantized_target_key(0, 0, 8192), (256, 256));
    }

    #[test]
    fn quantized_target_key_survives_a_degenerate_ceiling() {
        assert_eq!(quantized_target_key(100, 100, 0), (1, 1));
    }

    #[test]
    fn quantized_target_key_collapses_a_resize_drag_onto_one_key() {
        // Every width from 1 through 256 shares one key — the property the
        // whole quantization fix rests on: a pixel-by-pixel resize crosses
        // this boundary at most once per 256px.
        let mut keys: Vec<(u32, u32)> = (1..=256u32)
            .map(|w| quantized_target_key(w, w, 8192))
            .collect();
        keys.dedup();
        assert_eq!(keys, vec![(256, 256)]);
    }

    #[test]
    fn reapable_target_ages_selects_only_keys_unseen_for_at_least_max_age() {
        let target_last_seen: HashMap<(u64, u32, u32), u64> = [
            ((1, 256, 256), 0),
            ((2, 256, 256), 50),
            ((3, 256, 256), 100),
        ]
        .into_iter()
        .collect();
        // At frame 120: (1, ...) (age 120) and (2, ...) (age 70) are both
        // >= max_age 60; (3, ...) (age 20) is not.
        let mut stale = reapable_target_ages(&target_last_seen, 120, 60);
        stale.sort();
        assert_eq!(stale, vec![(1, 256, 256), (2, 256, 256)]);
    }

    #[test]
    fn reapable_target_ages_boundary_is_inclusive() {
        let target_last_seen: HashMap<(u64, u32, u32), u64> =
            [((1, 256, 256), 0)].into_iter().collect();
        assert!(reapable_target_ages(&target_last_seen, 60, 60).contains(&(1, 256, 256)));
        assert!(!reapable_target_ages(&target_last_seen, 59, 60).contains(&(1, 256, 256)));
    }

    #[test]
    fn reapable_target_ages_empty_when_every_key_seen_this_frame() {
        let mut target_last_seen: HashMap<(u64, u32, u32), u64> = HashMap::new();
        for frame in 1..=(MAX_UNSEEN_TARGET_FRAMES * 3) {
            target_last_seen.insert((1, 256, 256), frame);
            assert!(
                reapable_target_ages(&target_last_seen, frame, MAX_UNSEEN_TARGET_FRAMES).is_empty()
            );
        }
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
    fn oldest_target_key_for_id_picks_the_least_recently_seen_of_that_id_alone() {
        let target_last_seen: HashMap<(u64, u32, u32), u64> = [
            ((1, 256, 256), 10),
            ((1, 512, 512), 5),
            ((1, 768, 768), 20),
            // A different id's older entry must never be picked.
            ((2, 256, 256), 0),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            oldest_target_key_for_id(&target_last_seen, 1),
            Some((1, 512, 512))
        );
    }

    #[test]
    fn oldest_target_key_for_id_none_when_the_id_holds_nothing() {
        let target_last_seen: HashMap<(u64, u32, u32), u64> =
            [((2, 256, 256), 0)].into_iter().collect();
        assert_eq!(oldest_target_key_for_id(&target_last_seen, 1), None);
    }

    /// Device-free proof of [`MAX_TARGETS_PER_ID`]'s whole point: mirrors
    /// [`ShaderEffects::ensure_target`]'s own evict-before-insert order using
    /// the same production [`oldest_target_key_for_id`] helper, without a
    /// `wgpu::Device` — a sweep across 5 distinct quantized bands for one
    /// program id never leaves more than the cap resident.
    #[test]
    fn a_sweep_across_five_bands_leaves_at_most_the_cap_resident_for_one_id() {
        let id = 1u64;
        let mut resident: HashSet<(u64, u32, u32)> = HashSet::new();
        let mut target_last_seen: HashMap<(u64, u32, u32), u64> = HashMap::new();

        for band in 0..5u32 {
            let key = (id, band, band);
            if resident.len() >= MAX_TARGETS_PER_ID
                && let Some(evict) = oldest_target_key_for_id(&target_last_seen, id)
            {
                resident.remove(&evict);
                target_last_seen.remove(&evict);
            }
            resident.insert(key);
            target_last_seen.insert(key, u64::from(band));

            assert!(
                resident.len() <= MAX_TARGETS_PER_ID,
                "band {band}: must never exceed the per-id cap"
            );
        }

        assert_eq!(
            resident.len(),
            MAX_TARGETS_PER_ID,
            "exactly the cap remains resident after the sweep"
        );
        // The two most recent bands survive; the earlier three were evicted.
        assert!(resident.contains(&(id, 3, 3)));
        assert!(resident.contains(&(id, 4, 4)));
    }

    #[test]
    fn mark_seen_bumps_frame_and_reaps_ids_unseen_past_max_age() {
        let mut fx = ShaderEffects::new(None);
        // Frame 1: id 1 is live.
        assert!(
            fx.mark_seen(&[1].into_iter().collect(), &HashSet::new())
                .is_empty()
        );
        // Id 1 vanishes; keep marking an unrelated id (or nothing) live for
        // MAX_UNSEEN_FRAMES more frames — id 1 must not surface as reapable
        // until its age actually crosses the threshold.
        for _ in 0..(MAX_UNSEEN_FRAMES - 1) {
            assert!(fx.mark_seen(&HashSet::new(), &HashSet::new()).is_empty());
        }
        // One more frame crosses the threshold.
        let reapable = fx.mark_seen(&HashSet::new(), &HashSet::new());
        assert_eq!(reapable, vec![1]);
    }

    #[test]
    fn mark_seen_never_reaps_an_id_kept_live_every_frame() {
        let mut fx = ShaderEffects::new(None);
        for _ in 0..(MAX_UNSEEN_FRAMES * 2) {
            assert!(
                fx.mark_seen(&[1].into_iter().collect(), &HashSet::new())
                    .is_empty()
            );
        }
    }

    #[test]
    fn mark_seen_ages_a_target_key_on_its_own_shorter_clock_than_the_id() {
        let mut fx = ShaderEffects::new(None);
        let ids: HashSet<u64> = [1].into_iter().collect();
        let key = (1u64, 256u32, 256u32);

        // Frame 1: the id and its one target key are both live.
        fx.mark_seen(&ids, &[key].into_iter().collect());
        assert!(fx.target_last_seen.contains_key(&key));

        // The size is resized away from — id 1 stays live every frame, but
        // this exact target key is not — until its own (shorter)
        // MAX_UNSEEN_TARGET_FRAMES window elapses.
        for _ in 0..(MAX_UNSEEN_TARGET_FRAMES - 1) {
            fx.mark_seen(&ids, &HashSet::new());
            assert!(
                fx.target_last_seen.contains_key(&key),
                "must not age out before its own window elapses"
            );
        }
        fx.mark_seen(&ids, &HashSet::new());
        assert!(
            !fx.target_last_seen.contains_key(&key),
            "ages out on its own clock even though id 1 is still live every frame"
        );
    }

    #[test]
    fn mark_seen_keeps_two_target_sizes_of_one_id_live_in_the_same_frame() {
        // The regression the old frame-scoped eviction policy needed a
        // dedicated case for: one program (id 1) drawn at two distinct
        // (quantized) sizes in the same frame. Both keys are marked seen, so
        // neither ages — true by construction under the age-based policy,
        // not by a frame-scoped special case.
        let mut fx = ShaderEffects::new(None);
        let ids: HashSet<u64> = [1].into_iter().collect();
        let keys: HashSet<(u64, u32, u32)> = [(1, 256, 256), (1, 512, 512)].into_iter().collect();

        fx.mark_seen(&ids, &keys);

        assert!(fx.target_last_seen.contains_key(&(1, 256, 256)));
        assert!(fx.target_last_seen.contains_key(&(1, 512, 512)));
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
    fn reap_drops_the_stale_ids_target_last_seen_rows_but_not_others() {
        let mut fx = ShaderEffects::new(None);
        fx.target_last_seen.insert((1, 256, 256), 5);
        fx.target_last_seen.insert((2, 256, 256), 5);

        fx.reap(&[1]);

        assert!(!fx.target_last_seen.contains_key(&(1, 256, 256)));
        assert!(fx.target_last_seen.contains_key(&(2, 256, 256)));
    }

    /// End-to-end (real device) confirmation that a vanished id's *actual*
    /// compiled pipeline and offscreen target — not just the pure key-selection
    /// logic above — are dropped by `mark_seen`/`reap`, and that the id
    /// recompiles cleanly if redrawn afterward. The pure-logic tests above
    /// (`reapable_ids`/`reapable_target_keys`/`reapable_target_ages`/
    /// `mark_seen`/`reap`) cover the policy without a device; this covers the
    /// real `PipelineEntry`/`TargetEntry` removal a `wgpu::Device` requires to
    /// construct at all. Quantization + target-level aging get their own
    /// device test below (`a_resize_within_one_quantum_reuses_the_same_target_and_ages_out_once_unseen`).
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
            // With no target created for `(id, w, h)` (never `ensure_target`ed,
            // or a failed compile), the draw seam hands back `None` rather
            // than fabricating a texture.
            assert!(fx.target_texture(&device, 1, 4, 4).is_none());

            assert!(
                fx.mark_seen(&[1].into_iter().collect(), &HashSet::new())
                    .is_empty()
            );
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            fx.ensure_target(&device, 1, 4, 4);
            assert!(!fx.needs_compile(1), "compile must have succeeded");
            assert!(fx.pipelines.contains_key(&1));
            assert!(fx.target_texture(&device, 1, 4, 4).is_some());

            // id 1 stops being drawn: mark_seen with an empty live set every
            // frame until its age crosses MAX_UNSEEN_FRAMES.
            let mut reapable = Vec::new();
            for _ in 0..MAX_UNSEEN_FRAMES {
                reapable = fx.mark_seen(&HashSet::new(), &HashSet::new());
            }
            assert_eq!(reapable, vec![1]);
            fx.reap(&reapable);

            assert!(
                !fx.pipelines.contains_key(&1),
                "reap must drop the real compiled pipeline"
            );
            assert!(
                fx.target_texture(&device, 1, 4, 4).is_none(),
                "reap must drop the real offscreen target"
            );
            assert!(fx.needs_compile(1));

            // Re-drawn: recompiles cleanly, with no stale `failed` skip.
            assert!(
                fx.mark_seen(&[1].into_iter().collect(), &HashSet::new())
                    .is_empty()
            );
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            assert!(
                !fx.needs_compile(1),
                "a reaped id must recompile cleanly when redrawn"
            );
            assert!(fx.pipelines.contains_key(&1));
        }
    }

    /// End-to-end (real device) confirmation of the quantization + target-level
    /// aging fix: a resize drag that never crosses a 256px quantum boundary
    /// reuses one texture, crossing one mints a new one, and a size resized
    /// away from ages out on its own clock while the program id itself (and
    /// its still-live size) stay untouched.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-gpu -- --ignored`"]
    fn a_resize_within_one_quantum_reuses_the_same_target_and_ages_out_once_unseen() {
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
                    label: Some("frust-gpu effects quantization test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            const FRAGMENT: &str = "@fragment fn fs_main(in: FrustVsOut) -> \
                @location(0) vec4<f32> { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";

            let mut fx = ShaderEffects::new(None);
            fx.ensure_pipeline(&device, 1, FRAGMENT);

            // A resize drag through a range that never crosses a 256px
            // quantum boundary: every request lands on the same target.
            fx.ensure_target(&device, 1, 4, 4);
            let first = fx.target_texture(&device, 1, 4, 4).expect("created") as *const _;
            for size in [5u32, 64, 128, 200, 250] {
                fx.ensure_target(&device, 1, size, size);
                let texture = fx.target_texture(&device, 1, size, size).expect("reused");
                assert_eq!(
                    texture as *const _, first,
                    "size {size} must reuse the 256x256 target"
                );
            }

            // Crossing the boundary mints a distinct target.
            fx.ensure_target(&device, 1, 300, 300);
            let second = fx
                .target_texture(&device, 1, 300, 300)
                .expect("created past the quantum");
            assert_ne!(
                second as *const _, first,
                "a quantum crossing must create a new target"
            );

            // Only 300x300 (quantized to 512x512) is demanded from here on;
            // the small (256x256) target ages out on the target-level clock
            // while id 1 stays live and compiled throughout.
            let live_large: HashSet<(u64, u32, u32)> =
                [(1u64, 512u32, 512u32)].into_iter().collect();
            for _ in 0..MAX_UNSEEN_TARGET_FRAMES {
                fx.mark_seen(&[1].into_iter().collect(), &live_large);
            }
            assert!(
                fx.target_texture(&device, 1, 4, 4).is_none(),
                "the small target ages out once unseen"
            );
            assert!(
                fx.target_texture(&device, 1, 300, 300).is_some(),
                "the still-live size survives"
            );
            assert!(
                fx.pipelines.contains_key(&1),
                "the id itself is untouched by target-level aging"
            );
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
                .target_texture(&device, 1, requested, requested)
                .expect("an oversized request must still create a (clamped) target");
            assert_eq!(texture.width(), CEILING);
            assert_eq!(texture.height(), CEILING);
        }
    }

    /// Prove that extent coherence holds under quantization: an oversized
    /// request still creates a ceiling-sized texture, `target_extent` answers
    /// with the (also ceiling-clamped, here) sub-rect actually rendered, and
    /// repeating the same request reuses the existing entry (no second
    /// texture creation). Also proves the clamp warning fires only once per
    /// distinct `(id, requested_extent)` pair, survives the target's own
    /// age-based reap, and is dropped only by a whole-id `reap`.
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
            let quantized_key = (1u64, CEILING, CEILING);

            // Ensure the target for the first time.
            fx.ensure_target(&device, 1, requested, requested);
            let entry_1 = fx
                .targets
                .get(&quantized_key)
                .expect("target must be created");
            assert_eq!(entry_1.used_w, CEILING, "used_w must match ceiling");
            assert_eq!(entry_1.used_h, CEILING, "used_h must match ceiling");

            // The texture itself must have been created at the clamped extent.
            let texture_1 = fx
                .target_texture(&device, 1, requested, requested)
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
            let texture_2 = fx
                .target_texture(&device, 1, requested, requested)
                .expect("texture must still exist");
            let texture_ptr_2 = texture_2 as *const _;

            assert_eq!(
                texture_ptr_1, texture_ptr_2,
                "repeated request must reuse the same texture"
            );

            // `target_extent` answers the sub-rect a paint should map onto:
            // here the request is itself past the ceiling, so the sub-rect is
            // the ceiling too, agreeing with the texture's own size.
            let extent = fx
                .target_extent(&device, 1, requested, requested)
                .expect("registered");
            assert_eq!(extent, (CEILING, CEILING));

            // Verify the warning latched: warned_clamped must contain the (id,
            // requested_w, requested_h) key, showing we warned once.
            assert!(
                fx.warned_clamped.contains(&(1, requested, requested)),
                "warned_clamped must track the oversized request"
            );

            // Age the target out on its own clock (nothing demands id 1's
            // target at this size any more, though nothing else demands a
            // *different* size for it either — an empty live-key set every
            // frame is enough since target aging does not require the id to
            // vanish too).
            for _ in 0..MAX_UNSEEN_TARGET_FRAMES {
                fx.mark_seen(&HashSet::new(), &HashSet::new());
            }
            assert!(
                !fx.targets.contains_key(&quantized_key),
                "the unseen target ages out"
            );
            assert!(
                fx.warned_clamped.contains(&(1, requested, requested)),
                "the warn latch survives target-level aging"
            );

            // Recreate the same oversized request: the latch prevents a
            // second warning while it is still set.
            fx.ensure_target(&device, 1, requested, requested);
            let entry_3 = fx.targets.get(&quantized_key).expect("recreated");
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

    /// End-to-end (real device) confirmation of the target-identity fix: a
    /// target's generation stays put across a cache hit (the common,
    /// steady-state `ensure_target` call), but strictly increases when a key
    /// is genuinely recreated after being reaped — the signal
    /// `frust_engine::effects::shader_quad::ShaderQuadPass::register` needs
    /// to tell a frozen, orphaned old target apart from a fresh one at the
    /// same requested extent (see the module header's "Target identity"
    /// section and [`Self::target_generation`]'s own doc).
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-gpu -- --ignored`"]
    fn target_generation_bumps_only_when_a_target_is_actually_recreated() {
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
                    label: Some("frust-gpu effects target-generation test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            const FRAGMENT: &str = "@fragment fn fs_main(in: FrustVsOut) -> \
                @location(0) vec4<f32> { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }";

            let mut fx = ShaderEffects::new(None);
            fx.ensure_pipeline(&device, 1, FRAGMENT);

            fx.ensure_target(&device, 1, 4, 4);
            let first_generation = fx
                .target_generation(&device, 1, 4, 4)
                .expect("created target has a generation");

            // A cache hit (the same key already resident) must not bump it.
            fx.ensure_target(&device, 1, 4, 4);
            assert_eq!(
                fx.target_generation(&device, 1, 4, 4),
                Some(first_generation),
                "a cache hit leaves the generation untouched"
            );

            // Force the target out (as a whole-id reap would) and recreate it
            // at the identical requested extent.
            fx.reap(&[1]);
            fx.ensure_pipeline(&device, 1, FRAGMENT);
            fx.ensure_target(&device, 1, 4, 4);
            let second_generation = fx
                .target_generation(&device, 1, 4, 4)
                .expect("recreated target has a generation");

            assert!(
                second_generation > first_generation,
                "a target recreated at the same extent must carry a strictly \
                 greater generation ({second_generation} was not > {first_generation})"
            );
        }
    }
}
