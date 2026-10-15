//! [`SurfaceRenderer`]: the surface-lifecycle state machine that
//! renders a [`frust_scene::Scene`] into one window's swapchain each frame.
//!
//! Wraps the pure lifecycle logic in [`frust_gpu::lifecycle`] around the
//! actual `wgpu` resources: it starts in [`SurfacePhase::NoSurface`], becomes
//! renderable on `on_surface_created`, resizes on `on_surface_changed`, tears
//! down on `on_surface_destroyed`, and drops to [`SurfacePhase::SurfaceLost`]
//! when the swapchain reports `Lost` mid-frame.
//!
//! The frame — there is one renderer and one path family — is:
//! encode copies the frame's scene, then acquire → the scene's fragment-shader
//! quads rendered into their own offscreen targets
//! (`frust_engine::ShaderQuadPass`, a no-op for a scene drawing none) → one
//! `EngineRenderer::encode` into the acquired swapchain view and its
//! surface-owned depth attachment
//! ([`RenderPath::EngineDirect`](crate::context::RenderPath::EngineDirect)) → submit → present →
//! `end_frame`. Every one of those passes goes into a single encoder, which is
//! what puts a shader quad's own pass ahead of the frame that samples it. No
//! intermediate and no blit. A swapchain whose compositor
//! genuinely reads it as STRAIGHT alpha takes the same shape with one pass
//! appended and one indirection added: the frame is encoded into a
//! surface-owned intermediate and un-premultiplied into the acquired view from
//! there, in the same encoder
//! ([`RenderPath::EngineDirectUnpremultiply`](crate::context::RenderPath::EngineDirectUnpremultiply)).
//! iOS's sole
//! translucent mode (`PostMultiplied`) does NOT take that arm: it is backed
//! by Metal, whose compositor reads a `PostMultiplied` swapchain premultiplied
//! regardless of the mode's name (an upstream wgpu-hal truth bug — see
//! [`crate::context::choose_engine_render_path`]), so it stays on the
//! `EngineDirect` arm above with no conversion pass at all.
//!
//! This crate remaps the v3 present spans on the engine arm — see
//! [`SurfaceRenderer::submit`].

use core::ffi::c_void;

use anyhow::{Result, anyhow};

use std::sync::OnceLock;
use std::time::Duration;

use kurbo::Affine;

use crate::context::{EngineSurface, RenderPath};
use frust_gpu::lifecycle::{
    AcquireAction, AcquireOutcome, AcquireStatus, EncodeOutcome, FrameOutcome, SurfaceEvent,
    SurfacePhase, decide_acquire, next_invalid_streak, next_phase,
};
use frust_gpu::{DetachedSurface, RenderContext, SurfaceAlphaRequest};

/// The live GPU resources of a [`SurfacePhase::SurfaceReady`] surface.
///
/// The tier backend is created per surface (it is device-bound) and, with the
/// `RenderSurface`, is dropped on every transition out of `SurfaceReady`,
/// upholding the "no `SurfaceTexture`/surface outlives a transition" invariant.
struct ReadySurface {
    surface: EngineSurface,
    /// The tier-specific renderer that produces this frame's pixels — on the
    /// engine arms it targets the acquired swapchain texture in `submit`.
    backend: TierBackend,
    /// The device's persisted `wgpu::PipelineCache` (handed to the engine's
    /// pipeline build) paired with the adapter fingerprint
    /// its data is framed under, so [`SurfaceRenderer::pipeline_cache_data`] can
    /// hand back a validatable blob. `None` on adapters without
    /// `PIPELINE_CACHE` (Metal/DX12) — see
    /// [`frust_gpu::RenderContext::create_pipeline_cache`].
    pipeline_cache: Option<(wgpu::PipelineCache, String)>,
}

/// The per-surface renderer.
///
/// The engine variant records into the acquired swapchain view (or, on the
/// un-premultiplying arm, into the surface's intermediate) in
/// [`SurfaceRenderer::submit`]. A one-variant enum — the experimental
/// `vello_cpu`-backed `Cpu` variant and its `cpu-tier` feature were retired
/// alongside the vello-classic renderer's own removal, and the cargo feature
/// that once gated this one went with the choice it described.
// The renderer is boxed: the whole `ReadySurface` already lives behind a
// `Box` (`SurfaceState::Ready`), so nothing on the hot path pays for its
// size.
enum TierBackend {
    /// The frust-owned engine path: a
    /// `frust_scene::Scene` compiled into sparse strips and recorded into the
    /// frame's own `wgpu::CommandEncoder` by
    /// [`frust_engine::EngineRenderer`], which never submits — the encoder is
    /// created and submitted in [`SurfaceRenderer::submit`].
    Engine {
        engine: Box<frust_engine::EngineRenderer>,
        /// How many of this surface's frames the engine has refused so far
        /// (a scheduler escalation, an unserveable frame, a capacity ceiling).
        ///
        /// Counted rather than logged per frame: a refusal that reproduces
        /// every frame would otherwise flood the log with one identical line
        /// per vsync. The count feeds [`frust_gpu::context::decide_log_action`] —
        /// the same latch the uncaptured-`wgpu`-error handler uses — so the
        /// first few refusals are reported in full, the latch is announced
        /// once, and the running total keeps surfacing on the periodic debug
        /// bump afterwards.
        refused_frames: u32,
        /// The frame's fragment-shader pre-pass: every
        /// `frust_scene::Command::ShaderQuad` this surface draws, rendered
        /// into a pooled offscreen target and registered with `engine` as a
        /// scene texture before the frame that samples it is compiled.
        ///
        /// Per surface, beside the renderer whose registry it writes and
        /// seeded from the same persisted `wgpu::PipelineCache`: its targets
        /// and compiled user programs are device-bound, so they die with the
        /// surface exactly as the engine's own resources do.
        shader_quads: Box<frust_engine::ShaderQuadPass>,
        /// This surface's GPU timestamp ring — real per-pass GPU time, rather
        /// than the CPU wall-clock spans around `encode`/`submit`.
        ///
        /// Created for every engine surface and *inert* unless the device was
        /// created with `wgpu::Features::TIMESTAMP_QUERY`, which is the only
        /// thing that decides whether a frame line reports `gpu_q=1`. Inert
        /// costs one empty `Vec` and a branch per pass — no query set, no
        /// buffers, no map — so there is nothing to feature-gate at this
        /// level.
        timestamps: frust_gpu::diag::TimestampRing,
    },
}

/// One frame's real GPU time, split by the spans `frust-engine` names
/// ([`frust_engine::EngineSpan`]).
///
/// The value [`SurfaceRenderer::gpu_pass_timings`] hands a shell so it can put
/// GPU cost on the frame line beside the CPU spans it measured itself. Plain
/// `Duration`s — no `wgpu` type crosses this crate's boundary
/// (`docs/CODE_STANDARDS.md`'s wgpu-leak anti-pattern).
///
/// [`Self::total`] is the sum of the four, which is the frame's *attributed*
/// GPU pass time: the queue and driver gaps between passes belong to no pass
/// and are deliberately not folded in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuPassTimings {
    /// Work recorded ahead of the frame's own passes — the glyph-atlas replay.
    pub prepass: Duration,
    /// The frame's own surface passes: clear, opaque strips, alpha strips and
    /// the hole punch.
    pub main: Duration,
    /// Off-screen layer pages and filter passes.
    pub composite: Duration,
    /// The present-side conversion pass a straight-alpha swapchain needs.
    pub blit: Duration,
}

impl GpuPassTimings {
    /// The sum of every span.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.prepass
            .saturating_add(self.main)
            .saturating_add(self.composite)
            .saturating_add(self.blit)
    }
}

/// The surface half of the lifecycle machine, parallel to [`SurfacePhase`].
///
/// `Ready` is boxed: the live GPU resources dwarf the empty variants, and
/// boxing keeps the common `NoSurface`/`Lost` states cheap to move.
enum SurfaceState {
    NoSurface,
    Ready(Box<ReadySurface>),
    Lost,
}

/// A frame that has been rendered and queue-submitted but **not yet
/// presented** — the deferred half of [`SurfaceRenderer::submit_deferred`].
///
/// Opaque by design: it wraps a `wgpu::SurfaceTexture` (and the
/// `wgpu::Queue` that presents it) behind private fields with no accessor,
/// exactly like [`DetachedSurface`](crate::DetachedSurface), so no `wgpu` type
/// is nameable outside this crate (`docs/CODE_STANDARDS.md`'s wgpu-leak
/// anti-pattern). It is `Send` — a `wgpu::SurfaceTexture` owns its swapchain
/// frame and borrows nothing, and a `wgpu::Queue` is a cheap `Send + Sync`
/// handle — which is the whole point: a shell can hand it from the render
/// thread to the thread that must issue the present.
///
/// **Why deferring the present is a contract, not a micro-optimisation.** On
/// iOS, a `CAMetalLayer` with `presentsWithTransaction = true` requires
/// `[drawable present]` to run on the thread committing the `CATransaction`
/// that carries the sibling views' geometry. wgpu-hal's Metal present already
/// implements exactly that shape when the layer has the flag set (submit a
/// present command buffer, `waitUntilScheduled`, then `drawable.present()`) —
/// it simply runs it on whichever thread calls [`Self::present`]. Under the
/// render-thread split that thread commits no transaction, so the drawable is
/// never handed to the compositor at all (measured: total loss of presentation).
/// Handing this value to the UI thread and
/// presenting *there* is what lets frust's surface land in the same transaction
/// as the platform-view geometry while the split stays on.
///
/// Dropping one without presenting is safe and deliberate: the drawable returns
/// to the layer's pool un-presented and that frame is simply skipped — the
/// depth-1 latest-wins discipline (a newer frame supersedes an un-presented
/// older one rather than blocking on it).
pub struct DeferredPresent {
    texture: wgpu::SurfaceTexture,
    /// The queue the present is scheduled on. Carried because wgpu 30 moved
    /// the present from `SurfaceTexture::present(self)` onto
    /// `Queue::present(texture)`, and this handle outlives the `RenderContext`
    /// borrow that produced it.
    queue: wgpu::Queue,
}

impl std::fmt::Debug for DeferredPresent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeferredPresent").finish_non_exhaustive()
    }
}

impl DeferredPresent {
    /// Present the deferred frame **on the calling thread**.
    ///
    /// Consumes the handle, so a frame can never be presented twice. With
    /// `presentsWithTransaction` set on the target `CAMetalLayer` this is the
    /// call that must run on the transaction-committing (UI) thread; on every
    /// other platform/configuration it is an ordinary present that happens to
    /// have been moved off the submit site.
    pub fn present(self) {
        self.queue.present(self.texture);
    }
}

/// Per-surface renderer and lifecycle state machine.
///
/// Holds the current surface state plus the cross-call stashes one frame
/// needs. Constructed empty with [`SurfaceRenderer::new`]; the shell brings it
/// online with [`on_surface_created`](Self::on_surface_created).
pub struct SurfaceRenderer {
    state: SurfaceState,
    /// Consecutive `AcquireStatus::Invalid` acquires retried via `Reconfigure`
    /// since the last successful acquire or surface (re)install — see
    /// `frust_gpu::lifecycle`'s `decide_acquire`/`MAX_INVALID_RECONFIGURES`.
    /// Reset on a successful acquire, on giving up (transitioning to
    /// `SurfaceLost`), and on installing a fresh surface.
    consecutive_invalid: u8,
    /// Persisted, framed `wgpu::PipelineCache` blob (see
    /// [`frust_gpu::pipeline_cache`]) a prior run wrote and the shell restored from
    /// disk via [`set_initial_pipeline_cache_data`](Self::set_initial_pipeline_cache_data),
    /// consumed at the next surface install to seed the engine's shader-pipeline
    /// compilation. `None` is a cold start. Only meaningful on adapters with
    /// `PIPELINE_CACHE` (any Vulkan adapter); validated and discarded elsewhere.
    initial_cache_data: Option<Vec<u8>>,
    /// The swapchain texture [`Self::acquire`] acquired and stashed for
    /// [`Self::submit`] to render into and present (the acquire/submit span split).
    /// `None` outside an in-flight `acquire`→`submit` pair — set only on an
    /// [`AcquireOutcome::Acquired`] result, taken by the next [`Self::submit`].
    /// A `wgpu::SurfaceTexture` is owned (it does not borrow the surface), so
    /// stashing it here between the two clock-separated calls keeps the wgpu type
    /// confined to `frust-render` while letting a shell time each span with its
    /// own clock (timing stays shell-owned — see `frust-shell-common::perf`).
    pending_present: Option<wgpu::SurfaceTexture>,
    /// The `base_color` [`Self::encode`] was called with, stashed since the
    /// render that consumes `base_color` runs in [`Self::submit`] (it targets
    /// the acquired swapchain texture, which does not exist until
    /// [`Self::acquire`]), so the color must survive the gap between `encode`
    /// and `submit`. `None` outside an in-flight engine frame.
    pending_base_color: Option<peniko::Color>,
    /// The engine tier's owned copy of the frame's scene — its counterpart to
    /// [`Self::scene`], and reused across frames for the same reason.
    ///
    /// `frust_engine::EngineRenderer::encode` needs the GPU resources only
    /// [`Self::submit`] has (the acquired swapchain view), so the borrowed
    /// `&Scene` [`Self::encode`] is handed has to survive the gap between the
    /// two calls. `clone_from` reuses this buffer's capacity, so a
    /// steady-state frame allocates nothing.
    ///
    /// **Measurement note.** The same span caveat applies: on this tier
    /// `encode_us` holds a memcpy and nothing else — the whole scene compile
    /// (strip building, paint encoding) plus command recording lands in
    /// `submit_us`. Compare whole frames across tiers, never `encode_us`
    /// against `encode_us`.
    engine_scene: frust_scene::Scene,
    /// The first rendered frame's submit decomposition, until the shell takes
    /// it (see [`Self::take_first_submit_breakdown`]).
    #[cfg(feature = "perf-trace")]
    first_submit: Option<FirstSubmitBreakdown>,
    /// Whether a first rendered frame has been measured already.
    #[cfg(feature = "perf-trace")]
    first_submit_seen: bool,
    /// What [`Self::take_retained_ledger`] last handed out: the engine's
    /// retained generation, or `Some(None)` for the empty ledger of a renderer
    /// with no engine. `None` until the first take and again at every surface
    /// episode's start or end, so the next take always publishes.
    retained_published: Option<Option<u64>>,
    /// The paint serial the shell named for the next scene
    /// ([`Self::note_scene_serial`]); `0` until one is noted.
    scene_serial: u64,
    /// The scene [`Self::encode`] took last, until a `submit` commits or
    /// drops it: its paint serial, and whether it records a retained page.
    /// `None` outside an in-flight frame.
    pending_serial: Option<(u64, bool)>,
    /// The paint serial of the last scene a `submit` committed — the
    /// [`ResidentPages::frame`] every published ledger carries. A refused or
    /// skipped frame does not advance it; it outlives surface episodes.
    committed_serial: u64,
    /// Whether a scene recording a retained page was committed since the
    /// ledger was last handed out: the UI waits for a ledger covering that
    /// scene before drawing the page, so the next
    /// [`Self::take_retained_ledger`] publishes even when the store itself did
    /// not change (a page re-recorded at the same extent).
    recording_committed: bool,
    /// The device scale (physical / logical) last handed to
    /// [`Self::note_device_scale`], kept as raw bits so a change is an exact
    /// comparison. Outlives surface episodes: a recreate at the same scale is
    /// not a scale change (the fresh engine's store is empty anyway).
    device_scale: Option<u64>,
    /// Whether frames render through the partial-repaint accumulator
    /// ([`Self::set_partial_repaint`]) — off by default.
    partial_repaint: bool,
    /// The damage stated since the last frame the accumulator presented
    /// ([`Self::set_frame_damage`]), carried across a frame that was refused
    /// or never presented so the next one repaints it too.
    owed_damage: OwedDamage,
    /// Full/confined frame counts and confined area, for the
    /// `frust-perf partial` trace line.
    #[cfg(feature = "perf-trace")]
    partial_trace: PartialTrace,
}

/// What a partial-repaint frame owes the accumulator (see
/// [`SurfaceRenderer::set_frame_damage`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwedDamage {
    /// Nothing stated since the last presented frame: the frame is full,
    /// since nothing says what changed.
    Unstated,
    /// The whole surface.
    Full,
    /// This rectangle, in device pixels — empty when nothing changed.
    Rect(frust_engine::renderer::ScissorRect),
}

impl OwedDamage {
    /// The damage after `damage` is also owed.
    fn with(self, damage: Option<frust_engine::renderer::ScissorRect>) -> Self {
        match (self, damage) {
            (OwedDamage::Full, _) | (_, None) => OwedDamage::Full,
            (OwedDamage::Unstated, Some(rect)) => OwedDamage::Rect(rect),
            (OwedDamage::Rect(owed), Some(rect)) if owed.width == 0 || owed.height == 0 => {
                OwedDamage::Rect(rect)
            }
            (OwedDamage::Rect(owed), Some(rect)) if rect.width == 0 || rect.height == 0 => {
                OwedDamage::Rect(owed)
            }
            (OwedDamage::Rect(owed), Some(rect)) => {
                OwedDamage::Rect(frust_engine::renderer::ScissorRect::from_corners(
                    owed.x.min(rect.x),
                    owed.y.min(rect.y),
                    owed.x1().max(rect.x1()),
                    owed.y1().max(rect.y1()),
                ))
            }
        }
    }

    /// The scissor a frame owing this confines itself to — `None` for a full
    /// frame. `forced_full` is a scene drawing a pixel source no paint
    /// dirtiness tracks (`context::scene_forces_full_frame`).
    fn scissor(self, forced_full: bool) -> Option<frust_engine::renderer::ScissorRect> {
        match self {
            OwedDamage::Rect(rect) if !forced_full => Some(rect),
            _ => None,
        }
    }
}

/// The `frust-perf partial` counters: how many partial-repaint frames were
/// full, how many were confined, and the confined frames' summed share of
/// the surface, emitted once per [`PARTIAL_TRACE_WINDOW`] frames.
#[cfg(feature = "perf-trace")]
#[derive(Debug, Default)]
struct PartialTrace {
    full: u32,
    confined: u32,
    /// Summed confined area as a fraction of the surface, in parts per million.
    area_ppm: u64,
}

/// Frames per `frust-perf partial` line.
#[cfg(feature = "perf-trace")]
const PARTIAL_TRACE_WINDOW: u32 = 120;

#[cfg(feature = "perf-trace")]
impl PartialTrace {
    /// Counts one presented partial-repaint frame of a `width` x `height`
    /// surface, confined to `scissor` or full, and logs the window when it
    /// fills (only under `FRUST_TRACE`).
    fn record(
        &mut self,
        scissor: Option<frust_engine::renderer::ScissorRect>,
        width: u32,
        height: u32,
    ) {
        match scissor {
            Some(rect) => {
                self.confined += 1;
                let surface = u64::from(width.max(1)) * u64::from(height.max(1));
                let area = u64::from(rect.width) * u64::from(rect.height);
                self.area_ppm += area * 1_000_000 / surface;
            }
            None => self.full += 1,
        }
        if self.full + self.confined >= PARTIAL_TRACE_WINDOW {
            if crate::context::perf_tracing_enabled() {
                let mean_ppm = self.area_ppm / u64::from(self.confined.max(1));
                log::info!(
                    "frust-perf partial frames={} full={} confined={} confined_area_pct={}.{:02}",
                    self.full + self.confined,
                    self.full,
                    self.confined,
                    mean_ppm / 10_000,
                    (mean_ppm % 10_000) / 100
                );
            }
            *self = Self::default();
        }
    }
}

/// One retained page the renderer holds resident: the key it was recorded
/// under (`Command::PushRetained`'s `key`), its rasterized extent in physical
/// pixels and the scale it was rasterized at.
///
/// Both are the page *as rasterized*: the bracket's rectangle mapped through
/// its transform with the presentation scale applied, so `scale` is the device
/// scale times the bracket's own `scale` (see `frust_engine::cache::retained`).
/// A page is reusable by a `DrawRetained` with the same linear transform at any
/// alpha and any translation, and by nothing else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResidentPage {
    /// The raw retained key.
    pub key: u64,
    /// The page's extent in physical pixels.
    pub extent_px: (u32, u32),
    /// The device scale the page was rasterized at.
    pub scale: f32,
}

/// The retained pages a [`SurfaceRenderer`] holds resident after its last
/// frame — the renderer's side of the resident-key ledger.
///
/// Plain data, so this crate needs no `frust-core` edge: a shell converts it
/// into `frust_core::RetainedLedger` (`RetainedLedger::from_entries`) and hands
/// it to the UI tree for the next paint, one frame stale by design.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResidentPages {
    /// Resident pages, in no particular order.
    pub pages: Vec<ResidentPage>,
    /// The paint serial ([`SurfaceRenderer::note_scene_serial`]) of the last
    /// scene whose retained-store commit the renderer completed before this
    /// was taken — what lets the UI tell a page holding its latest recording
    /// from one holding an older recording under the same key. A refused or
    /// failed frame does not advance it; the empty ledger a store clear,
    /// surface destroy or scale change leaves carries the last committed
    /// serial (`0` before any).
    pub frame: u64,
}

/// `engine`'s retained store as the plain data a shell publishes, stamped with
/// `frame`, the serial of the last committed scene — the one conversion
/// [`SurfaceRenderer::retained_ledger`] makes, kept free so it is testable
/// against a real engine with no surface in the loop.
fn resident_pages(engine: &frust_engine::EngineRenderer, frame: u64) -> ResidentPages {
    ResidentPages {
        pages: engine
            .retained_ledger()
            .map(|entry| ResidentPage {
                key: entry.key,
                extent_px: entry.extent_px,
                scale: entry.scale,
            })
            .collect(),
        frame,
    }
}

/// Whether `scene` records a retained page (opens a `PushRetained` bracket).
fn scene_records_retained(scene: &frust_scene::Scene) -> bool {
    scene
        .commands()
        .iter()
        .any(|command| matches!(command, frust_scene::Command::PushRetained { .. }))
}

/// Where the first rendered frame's `submit` time went.
///
/// The three parts partition the span: `encode` excludes `pipeline_wait`, and
/// `queue` is `Queue::submit` plus `Queue::present` (the present is absent
/// when the caller presents a deferred frame itself).
#[cfg(feature = "perf-trace")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FirstSubmitBreakdown {
    /// Time inside the pipeline cache resolving not-yet-held variants.
    pub pipeline_wait: std::time::Duration,
    /// The engine's encode, less `pipeline_wait`.
    pub encode: std::time::Duration,
    /// `Queue::submit` plus the present.
    pub queue: std::time::Duration,
}

/// The first-frame timing probe `submit_impl` threads through its arms. A
/// zero-sized no-op without `perf-trace`.
#[cfg(feature = "perf-trace")]
struct SubmitProbe {
    armed: bool,
    encode_start: Option<std::time::Instant>,
    queue_start: Option<std::time::Instant>,
    breakdown: FirstSubmitBreakdown,
}

#[cfg(feature = "perf-trace")]
impl SubmitProbe {
    fn new(armed: bool) -> Self {
        Self {
            armed,
            encode_start: None,
            queue_start: None,
            breakdown: FirstSubmitBreakdown::default(),
        }
    }

    fn encode_begin(&mut self, engine: &mut frust_engine::EngineRenderer) {
        if self.armed {
            // Discard whatever earlier work (the install's warm-up) accrued.
            let _ = engine.take_pipeline_wait();
            self.encode_start = Some(std::time::Instant::now());
        }
    }

    fn encode_end(&mut self, engine: &mut frust_engine::EngineRenderer) {
        if let Some(start) = self.encode_start.take() {
            let total = start.elapsed();
            self.breakdown.pipeline_wait = engine.take_pipeline_wait();
            self.breakdown.encode = total.saturating_sub(self.breakdown.pipeline_wait);
        }
    }

    fn queue_begin(&mut self) {
        if self.armed {
            self.queue_start = Some(std::time::Instant::now());
        }
    }

    fn queue_end(&mut self) {
        if let Some(start) = self.queue_start.take() {
            self.breakdown.queue = start.elapsed();
        }
    }
}

#[cfg(not(feature = "perf-trace"))]
struct SubmitProbe;

#[cfg(not(feature = "perf-trace"))]
impl SubmitProbe {
    fn new(_armed: bool) -> Self {
        Self
    }
    fn encode_begin(&mut self, _engine: &mut frust_engine::EngineRenderer) {}
    fn encode_end(&mut self, _engine: &mut frust_engine::EngineRenderer) {}
    fn queue_begin(&mut self) {}
    fn queue_end(&mut self) {}
}

impl Default for SurfaceRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceRenderer {
    /// Creates an empty renderer in [`SurfacePhase::NoSurface`].
    ///
    /// No GPU work happens here; the surface is created later by the shell via
    /// [`on_surface_created`](Self::on_surface_created) once a window/surface
    /// exists (deferred window creation on desktop, `surfaceCreated` on Android).
    pub fn new() -> Self {
        Self {
            state: SurfaceState::NoSurface,
            consecutive_invalid: 0,
            initial_cache_data: None,
            pending_present: None,
            pending_base_color: None,
            engine_scene: frust_scene::Scene::default(),
            #[cfg(feature = "perf-trace")]
            first_submit: None,
            #[cfg(feature = "perf-trace")]
            first_submit_seen: false,
            retained_published: None,
            scene_serial: 0,
            pending_serial: None,
            committed_serial: 0,
            recording_committed: false,
            device_scale: None,
            partial_repaint: false,
            owed_damage: OwedDamage::Unstated,
            #[cfg(feature = "perf-trace")]
            partial_trace: PartialTrace::default(),
        }
    }

    /// Switches the partial-repaint spike on or off (`FRUST_PARTIAL_REPAINT`,
    /// read by the shells; off by default, and off leaves every frame exactly
    /// as it was).
    ///
    /// On, every frame is presented from a persistent surface-owned
    /// accumulator: a frame confined to its damage
    /// ([`Self::set_frame_damage`]) renders into the arm's readable root with
    /// every surface pass scissored to it and copies that rectangle across,
    /// and any other frame renders straight into the accumulator — then the
    /// whole accumulator is presented through the arm's present pass. A frame
    /// is full when no damage was stated for it, when its damage is the whole
    /// surface, after a resize, a scale change or a surface recreate, when its
    /// base colour changed, and when its scene draws a caller-bound texture or
    /// a fragment-shader quad (pixel sources no paint dirtiness tracks).
    pub fn set_partial_repaint(&mut self, on: bool) {
        self.partial_repaint = on;
        if !on {
            self.owed_damage = OwedDamage::Unstated;
            if let SurfaceState::Ready(ready) = &mut self.state {
                ready.surface.partial.release();
            }
        }
    }

    /// Whether frames render through the partial-repaint accumulator.
    pub fn partial_repaint(&self) -> bool {
        self.partial_repaint
    }

    /// States the next frame's damage — what changed since the last frame
    /// this renderer presented — in the surface's device pixels: `None` for
    /// the whole surface, an empty rectangle for nothing. Rounded out to whole
    /// pixels and grown by one on every side, for anti-aliased edges.
    ///
    /// Owed until a frame is presented: damage stated for a frame that was
    /// refused or never reached the screen is repainted by the next one. Only
    /// read with [`Self::set_partial_repaint`] on.
    pub fn set_frame_damage(&mut self, damage: Option<kurbo::Rect>) {
        let rect = damage.map(|rect| {
            if rect.is_zero_area() || !rect.is_finite() {
                return frust_engine::renderer::ScissorRect::from_corners(0, 0, 0, 0);
            }
            let to_px = |value: f64| value.clamp(0.0, f64::from(u32::MAX)) as u32;
            frust_engine::renderer::ScissorRect::from_corners(
                to_px(rect.x0.floor() - 1.0),
                to_px(rect.y0.floor() - 1.0),
                to_px(rect.x1.ceil() + 1.0),
                to_px(rect.y1.ceil() + 1.0),
            )
        });
        self.owed_damage = self.owed_damage.with(rect);
    }

    /// Takes the first rendered frame's submit decomposition, once.
    ///
    /// `Some` from the first [`FrameOutcome::Rendered`] submit until taken;
    /// `None` before it, after it was taken, and on every later frame.
    #[cfg(feature = "perf-trace")]
    pub fn take_first_submit_breakdown(&mut self) -> Option<FirstSubmitBreakdown> {
        self.first_submit.take()
    }

    /// Drops the in-flight frame's cross-call stash: the engine arm's
    /// `base_color` and the encoded scene's serial.
    ///
    /// It describes one frame of one surface, so any event that ends a
    /// surface's life (`on_surface_destroyed`) or replaces it
    /// (`install_surface`) must drop it rather than let the next `submit`
    /// consume a stale colour. Idempotent.
    fn clear_pending_frame(&mut self) {
        self.pending_base_color = None;
        self.pending_serial = None;
    }

    /// Adopt `state` and reset everything else scoped to ONE surface: the
    /// `Invalid`-reconfigure streak and the in-flight frame's stash.
    ///
    /// The one place a surface episode begins or ends. `install_surface`
    /// enters through it with the freshly built [`SurfaceState::Ready`];
    /// `on_surface_destroyed` leaves through it with
    /// [`SurfaceState::NoSurface`]; and [`Self::acquire`]'s give-up arm leaves
    /// through it with [`SurfaceState::Lost`] — so a reset can never be
    /// written into one of those paths and forgotten in another. Everything
    /// reset here describes the surface being replaced or torn down: the
    /// streak belonged to it, and the stash describes one of its frames.
    /// Nothing else may assign `self.state`.
    ///
    /// The retained ledger last published is reset with them: a new episode's
    /// engine starts with an empty store and an ended one has none, so the
    /// next [`Self::take_retained_ledger`] publishes (the empty ledger, either
    /// way) whatever generation the old engine had reached.
    fn adopt_surface_state(&mut self, state: SurfaceState) {
        self.state = state;
        self.consecutive_invalid = 0;
        self.clear_pending_frame();
        self.retained_published = None;
    }

    /// Restores the persisted pipeline-cache blob a prior run produced via
    /// [`pipeline_cache_data`](Self::pipeline_cache_data), to seed the engine's
    /// shader-pipeline compilation at the next surface install and cut warm-start
    /// shader/pipeline compilation to near zero on Vulkan.
    ///
    /// Builder-style (a setter rather than a new `on_surface_created*`
    /// parameter) so the three surface-creation entry points — and every shell
    /// call site — keep their signatures; the shell (persistence lands in tasks
    /// 13/14) calls this once after [`SurfaceRenderer::new`] and before the
    /// first `on_surface_created*`. Has no effect in practice on adapters
    /// without `PIPELINE_CACHE` (Metal/DX12): the blob is validated against
    /// the live adapter at install time and discarded on any mismatch. Passing
    /// `None` clears any restored blob (a cold start).
    ///
    /// **Caller contract (provenance):** `data` must be bytes this renderer
    /// itself previously produced via
    /// [`pipeline_cache_data`](Self::pipeline_cache_data) and the shell
    /// persisted verbatim — never bytes from any other source. The framing
    /// check downstream (`frust_gpu::pipeline_cache::unframe`) proves the
    /// magic tag and the adapter fingerprint, which is provenance by
    /// convention rather than integrity: a deliberately forged blob passing
    /// that frame reaches `wgpu`'s `unsafe` pipeline-cache seam, whose
    /// contract makes foreign data undefined behaviour. This method stays
    /// safe because the obligation is a data-handling rule for the shell's
    /// persistence layer, not something a signature can enforce.
    pub fn set_initial_pipeline_cache_data(&mut self, data: Option<Vec<u8>>) {
        self.initial_cache_data = data;
    }

    /// The current pipeline-cache data to persist, framed with the adapter
    /// fingerprint so a later launch can validate it before reuse (see
    /// [`frust_gpu::pipeline_cache`]).
    ///
    /// `None` when there is no live cache to read — no surface installed, or an
    /// adapter without `PIPELINE_CACHE` (Metal/DX12) — or when the driver has
    /// produced nothing to hand back yet. The shell (tasks 13/14) writes the
    /// returned bytes to disk and feeds them back via
    /// [`set_initial_pipeline_cache_data`](Self::set_initial_pipeline_cache_data)
    /// on the next launch.
    pub fn pipeline_cache_data(&self) -> Option<Vec<u8>> {
        let SurfaceState::Ready(ready) = &self.state else {
            return None;
        };
        let (cache, key) = ready.pipeline_cache.as_ref()?;
        let data = cache.get_data()?;
        Some(frust_gpu::pipeline_cache::frame(key, &data))
    }

    /// How many pipelines the engine's warm-up worker has finished building,
    /// or `None` outside [`SurfacePhase::SurfaceReady`].
    ///
    /// Diagnostic: a shell logs it beside the persisted pipeline-cache size so
    /// a small blob can be read against how much of the warm-up had landed.
    pub fn pipeline_warm_up_finished(&self) -> Option<u64> {
        let SurfaceState::Ready(ready) = &self.state else {
            return None;
        };
        let TierBackend::Engine { engine, .. } = &ready.backend;
        Some(engine.warm_up_finished())
    }

    /// The retained pages the renderer holds resident after its last frame (see
    /// [`ResidentPages`]): the engine's retained-page store, read as plain
    /// data. Empty outside [`SurfacePhase::SurfaceReady`] — with no surface there
    /// is no engine and no page. Never blocks, and allocates nothing while the
    /// store is empty.
    pub fn retained_ledger(&self) -> ResidentPages {
        let SurfaceState::Ready(ready) = &self.state else {
            return ResidentPages {
                pages: Vec::new(),
                frame: self.committed_serial,
            };
        };
        let TierBackend::Engine { engine, .. } = &ready.backend;
        resident_pages(engine, self.committed_serial)
    }

    /// [`Self::retained_ledger`], but only when it changed since the last call
    /// — a page added, evicted or re-rendered at another extent, the store
    /// cleared, or the surface created, recreated or destroyed (each of which
    /// starts the next call over, so a fresh or torn-down surface always
    /// publishes its empty ledger) — or when a committed scene recorded a page
    /// while the store holds any: the UI draws a page only once a ledger
    /// covers the scene that recorded it, and a page re-recorded at the same
    /// extent changes nothing else. `None` when neither happened.
    ///
    /// What a shell calls after each frame's submit to publish the ledger
    /// without re-publishing an unchanged one every frame.
    pub fn take_retained_ledger(&mut self) -> Option<ResidentPages> {
        let (generation, holds_pages) = match &self.state {
            SurfaceState::Ready(ready) => {
                let TierBackend::Engine { engine, .. } = &ready.backend;
                (
                    Some(engine.retained_generation()),
                    engine.retained_ledger().next().is_some(),
                )
            }
            SurfaceState::NoSurface | SurfaceState::Lost => (None, false),
        };
        let recorded = std::mem::take(&mut self.recording_committed) && holds_pages;
        if self.retained_published == Some(generation) && !recorded {
            return None;
        }
        self.retained_published = Some(generation);
        Some(self.retained_ledger())
    }

    /// Name the paint serial (`frust_core::PaintOutcome::paint_serial`) of the
    /// scene the next [`Self::encode`] takes. A shell calls it before every
    /// encode, on whichever thread renders, with the serial that travelled
    /// with the scene; once that scene's frame is committed, every ledger this
    /// renderer publishes carries the serial as its [`ResidentPages::frame`].
    /// A shell that never calls it publishes ledgers stamped `0`.
    pub fn note_scene_serial(&mut self, serial: u64) {
        self.scene_serial = serial;
    }

    /// Whether a frame was refused since the last call because a
    /// `DrawRetained` named a page the engine did not hold (the engine then
    /// empties its store), clearing the flag.
    ///
    /// The refused frame presented nothing, so a shell seeing `true` owes the
    /// widget tree one more paint, against the emptied ledger it publishes
    /// alongside — otherwise an idle screen would keep the frame before the
    /// refused one until something else asked for a paint.
    pub fn take_retained_refusal(&mut self) -> bool {
        match &mut self.state {
            SurfaceState::Ready(ready) => {
                let TierBackend::Engine { engine, .. } = &mut ready.backend;
                engine.take_retained_refusal()
            }
            SurfaceState::NoSurface | SurfaceState::Lost => false,
        }
    }

    /// Drop every retained page the engine holds — what a device-scale change
    /// calls for, since every body is about to be laid out again. The next
    /// [`Self::take_retained_ledger`] reports the empty store. A no-op with no
    /// surface.
    pub fn clear_retained(&mut self) {
        if let SurfaceState::Ready(ready) = &mut self.state {
            let TierBackend::Engine { engine, .. } = &mut ready.backend;
            engine.clear_retained();
        }
    }

    /// Note the device scale (physical / logical, already sanitized by the
    /// shell) the coming frames are painted at — the one place a shell reports
    /// a scale change to the retained store.
    ///
    /// When `scale` differs from the last noted one (or none was noted yet),
    /// every resident page was rasterized for a scale no frame will name
    /// again, so the store is cleared ([`Self::clear_retained`]) and the next
    /// [`Self::take_retained_ledger`] publishes the empty ledger even when the
    /// store was empty already — the UI thread then paints every retained
    /// body again instead of naming pages that are gone. The same scale again
    /// is a no-op, so a shell may call this at every surface create and
    /// resize. Returns whether the scale changed.
    pub fn note_device_scale(&mut self, scale: f64) -> bool {
        let bits = scale.to_bits();
        if self.device_scale == Some(bits) {
            return false;
        }
        self.device_scale = Some(bits);
        self.clear_retained();
        self.retained_published = None;
        // Every pixel the partial-repaint accumulator holds was drawn at the
        // old scale.
        self.owed_damage = OwedDamage::Full;
        if let SurfaceState::Ready(ready) = &mut self.state {
            ready.surface.partial.invalidate();
        }
        true
    }

    /// Whether the **live** surface actually resolved to a translucent
    /// (alpha-compositing) mode — the truth a shell's Mode B paint contract
    /// must key off, replacing the `SurfaceAlphaRequest` it *asked* for.
    ///
    /// [`SurfaceAlphaRequest::TranslucentPreferred`] is a *preference*: the
    /// configure step resolves it against the platform's advertised alpha modes
    /// and silently degrades to an opaque swapchain (with a `log::warn!`) when
    /// none is available. A shell that clears its base color to `TRANSPARENT`
    /// and lets `platform_view` slots punch their rects on the strength of the
    /// request alone then presents black rectangles on that opaque swapchain.
    /// Reading this after every successful `on_surface_created*`/
    /// [`on_surface_installed`](Self::on_surface_installed) — and re-reading it
    /// after every recreate — is what makes a fallback degrade to the Mode A
    /// contract (opaque base, no punch) instead.
    ///
    /// `false` outside [`SurfacePhase::SurfaceReady`]: with no live surface
    /// there is nothing translucent to composite against, and `false` is the
    /// safe (Mode A) default — never punch a hole you can't prove is a window.
    ///
    /// Deliberately **not** named `is_surface_translucent`: `RenderRoot` (which
    /// sits on the other side of this same call chain) already owns a method by
    /// that name for the *pushed* flag, and a shell threads this value straight
    /// into it. The signature is `wgpu`-free, like every other value crossing
    /// this crate's boundary (`docs/CODE_STANDARDS.md`'s wgpu-leak
    /// anti-pattern).
    pub fn surface_resolved_translucent(&self) -> bool {
        match &self.state {
            SurfaceState::Ready(ready) => ready.surface.resolved_translucent(),
            SurfaceState::NoSurface | SurfaceState::Lost => false,
        }
    }

    /// The most recent frame's real GPU time per pass, or `None` when this
    /// surface produces no such measurement.
    ///
    /// `None` — the `gpu_q=0` case a shell reports — for every reason there is:
    /// no live surface, a device created without
    /// `wgpu::Features::TIMESTAMP_QUERY` (which a build that did not compile
    /// `perf-trace` in never asks for), and the first few frames of a surface,
    /// before the ring's first readback has landed.
    ///
    /// The reading lags the calling frame: a frame's queries are mapped
    /// without ever blocking the frame path, so what comes back is a recent
    /// frame's GPU cost rather than the one being recorded. In steady state one
    /// fresh reading lands per frame, so the series is complete and offset,
    /// not sparse — see [`frust_gpu::diag::TimestampRing`].
    pub fn gpu_pass_timings(&self) -> Option<GpuPassTimings> {
        let SurfaceState::Ready(ready) = &self.state else {
            return None;
        };
        let TierBackend::Engine { timestamps, .. } = &ready.backend;
        let reading = timestamps.latest()?;
        let span = |which: frust_engine::EngineSpan| reading.span(which.index());
        Some(GpuPassTimings {
            prepass: span(frust_engine::EngineSpan::Prepass),
            main: span(frust_engine::EngineSpan::Main),
            composite: span(frust_engine::EngineSpan::Composite),
            blit: span(frust_engine::EngineSpan::Blit),
        })
    }

    /// The current lifecycle phase.
    pub fn phase(&self) -> SurfacePhase {
        match self.state {
            SurfaceState::NoSurface => SurfacePhase::NoSurface,
            SurfaceState::Ready(_) => SurfacePhase::SurfaceReady,
            SurfaceState::Lost => SurfacePhase::SurfaceLost,
        }
    }

    /// Brings the surface online (`surfaceCreated`/`resumed`): creates the
    /// swapchain and a device-bound renderer, transitioning to
    /// [`SurfacePhase::SurfaceReady`].
    ///
    /// Valid from any phase — calling it in `SurfaceLost` is how the shell
    /// recovers, and calling it in `SurfaceReady` replaces the surface (the old
    /// one is dropped first). `window` is any raw window handle the shell owns
    /// (`wgpu::SurfaceTarget`); no `winit` dependency is imposed here.
    ///
    /// Presentation uses vsync (`PresentMode::AutoVsync`), matching the
    /// vsync-driven frame pacing the platform shells provide.
    pub async fn on_surface_created(
        &mut self,
        ctx: &mut RenderContext,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        alpha: SurfaceAlphaRequest,
    ) -> Result<()> {
        let surface = crate::context::create_engine_surface_from_target(
            ctx,
            window,
            width.max(1),
            height.max(1),
            wgpu::PresentMode::AutoVsync,
            alpha,
        )
        .await
        .map_err(|e| anyhow!("frust-render: failed to create render surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Brings the surface online from a [`DetachedSurface`] that was created on
    /// the windowing/main thread via
    /// [`RenderContext::surface_factory`](crate::RenderContext::surface_factory),
    /// transitioning to [`SurfacePhase::SurfaceReady`] (the
    /// render-thread split).
    ///
    /// The desktop counterpart of [`on_surface_created`](Self::on_surface_created)
    /// for the split: `on_surface_created` reads the window handle *and*
    /// configures on one thread, but winit only yields that handle on the main
    /// thread — so the split creates the surface there
    /// ([`SurfaceFactory::create_detached_surface`](crate::SurfaceFactory::create_detached_surface))
    /// and hands the `Send` surface here, where the render thread that owns this
    /// renderer/context does the device + swapchain + blitter work. Presentation
    /// uses vsync (`PresentMode::AutoVsync`), matching `on_surface_created`.
    ///
    /// Valid from any phase (recreation after `SurfaceLost`/resume replaces the
    /// old surface — dropped first).
    pub async fn on_surface_installed(
        &mut self,
        ctx: &mut RenderContext,
        surface: DetachedSurface,
        width: u32,
        height: u32,
        alpha: SurfaceAlphaRequest,
    ) -> Result<()> {
        let surface = crate::context::create_engine_surface(
            ctx,
            surface.into_surface(),
            width.max(1),
            height.max(1),
            wgpu::PresentMode::AutoVsync,
            alpha,
        )
        .await
        .map_err(|e| anyhow!("frust-render: failed to configure detached surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Brings the surface online from a raw `ANativeWindow` pointer (Android
    /// `surfaceCreated`), transitioning to [`SurfacePhase::SurfaceReady`].
    ///
    /// Compiled unconditionally so a host `cargo check --target
    /// aarch64-linux-android` covers it. This is one of the framework's
    /// sanctioned unsafe entry points (see also
    /// [`on_surface_created_from_metal_layer`](Self::on_surface_created_from_metal_layer));
    /// the raw-pointer handling is isolated in
    /// [`frust_gpu::lifecycle::create_android_surface`].
    ///
    /// # Safety
    ///
    /// `window_ptr` must be a valid, acquired `ANativeWindow*` that outlives the
    /// surface (and all its `SurfaceTexture`s). See
    /// [`frust_gpu::lifecycle::create_android_surface`] for the full contract.
    pub async unsafe fn on_surface_created_from_android_window(
        &mut self,
        ctx: &mut RenderContext,
        window_ptr: *mut c_void,
        width: u32,
        height: u32,
        alpha: SurfaceAlphaRequest,
    ) -> Result<()> {
        // SAFETY: forwarded to the caller's `on_surface_created_from_android_window`
        // contract — `window_ptr` is a valid, acquired ANativeWindow* outliving
        // the surface.
        let raw =
            unsafe { frust_gpu::lifecycle::create_android_surface(ctx.instance(), window_ptr) }?;
        let surface = crate::context::create_engine_surface(
            ctx,
            raw,
            width.max(1),
            height.max(1),
            wgpu::PresentMode::AutoVsync,
            alpha,
        )
        .await
        .map_err(|e| anyhow!("frust-render: failed to configure Android surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Brings the surface online from a raw `CAMetalLayer*` pointer (iOS/macOS
    /// Swift shell surface creation), transitioning to
    /// [`SurfacePhase::SurfaceReady`].
    ///
    /// Only compiled on Apple targets (mirrors [`frust_gpu::lifecycle::create_metal_surface`]'s
    /// gating): the raw-pointer handling is isolated there, one of the
    /// framework's sanctioned unsafe boundaries alongside
    /// [`on_surface_created_from_android_window`](Self::on_surface_created_from_android_window).
    ///
    /// Presentation uses `Fifo` — the only present mode guaranteed on
    /// iOS/Metal (vsync-equivalent, matching the desktop/Android
    /// `AutoVsync` paths in spirit).
    ///
    /// # Safety
    ///
    /// `layer_ptr` must be a valid, live `CAMetalLayer*` that outlives the
    /// surface (and all its `SurfaceTexture`s). See
    /// [`frust_gpu::lifecycle::create_metal_surface`] for the full contract.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    pub async unsafe fn on_surface_created_from_metal_layer(
        &mut self,
        ctx: &mut RenderContext,
        layer_ptr: *mut c_void,
        width: u32,
        height: u32,
        alpha: SurfaceAlphaRequest,
    ) -> Result<()> {
        // SAFETY: forwarded to the caller's `on_surface_created_from_metal_layer`
        // contract — `layer_ptr` is a valid, live CAMetalLayer* outliving the
        // surface.
        let raw = unsafe { frust_gpu::lifecycle::create_metal_surface(ctx.instance(), layer_ptr) }?;
        let surface = crate::context::create_engine_surface(
            ctx,
            raw,
            width.max(1),
            height.max(1),
            wgpu::PresentMode::Fifo,
            alpha,
        )
        .await
        .map_err(|e| anyhow!("frust-render: failed to configure Metal surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Wraps a freshly created `RenderSurface` in a device-bound renderer and
    /// installs it as the live surface. Shared by the safe and Android paths.
    fn install_surface(&mut self, ctx: &RenderContext, surface: EngineSurface) -> Result<()> {
        // Seed the engine's shader-pipeline compilation from a persisted
        // `wgpu::PipelineCache` when the adapter supports it (Vulkan/Android) and
        // the shell restored a validated blob via
        // `set_initial_pipeline_cache_data`. Returns `None` on adapters without
        // `PIPELINE_CACHE` (Metal/DX12). Created before the backend below so
        // it can be handed to `EngineRenderer::new`.
        // SAFETY: `initial_cache_data` arrives only via
        // `set_initial_pipeline_cache_data`, whose documented caller contract
        // requires bytes this renderer itself produced through
        // `pipeline_cache_data` and the shell persisted verbatim — the
        // provenance `create_pipeline_cache`'s `# Safety` section requires.
        // This call site cannot itself prove that history; it relies on that
        // documented contract plus `unframe`'s magic-tag/adapter-fingerprint
        // check, which rejects every accidental mismatch (integrity against a
        // deliberate forgery is out of scope — see the setter's doc). `None`
        // is a cold start.
        let pipeline_cache =
            unsafe { ctx.create_pipeline_cache(self.initial_cache_data.as_deref()) };

        // Build the backend. There is exactly one renderer and no tier to
        // dispatch on: `context::create_engine_surface` has already refused an
        // adapter the engine cannot drive, before any surface is configured.
        let backend = {
            let device = &ctx.device_handle().device;
            // Built for the format this surface's FRAMES will target,
            // because the engine warms its strip pipelines for exactly one
            // of them: the swapchain's own on the direct arm, the engine's
            // off-screen format on the un-premultiplying one, whose frames
            // land in a surface-owned intermediate instead
            // ([`engine_target_format`]).
            //
            // A resize keeps that format (`EngineSurface::resize`
            // rewrites only the dimensions), so `on_surface_changed`
            // resizes the renderer in place; a FORMAT change arrives as a
            // fresh surface and lands back here, building a renderer
            // warmed for the new format — off the frame path, and seeded
            // from the same persisted `wgpu::PipelineCache` blob, so a
            // re-warm after a format change is a driver-cache hit rather
            // than a cold compile.
            let caps = frust_gpu::TierCaps::probe(&ctx.device_handle().adapter);
            let engine_format = engine_target_format(&surface.path, surface.config().format);
            let engine = frust_engine::EngineRenderer::new(
                device,
                &caps,
                engine_format,
                pipeline_cache.as_ref(),
            )
            .map_err(|e| anyhow!("frust-render: failed to create engine renderer: {e}"))?;
            // One line per process naming the renderer, next to the
            // `render-path`/`aa-mode` lines, so a capture proves which
            // renderer produced the frames it is timing.
            //
            // FIXED EVIDENCE MARKER: the `frust-render tier=engine (...)`
            // text below is byte-for-byte load-bearing — every device gate
            // greps logcat/console for it and `benchmarks/RESULTS.md`
            // quotes it verbatim. It outlived the retirement of the tier
            // *selection* deliberately (there is nothing left to select,
            // but the receipts still have to match), so do not reword,
            // re-case or re-punctuate it, and emit it unconditionally.
            static ENGINE_LOGGED: OnceLock<()> = OnceLock::new();
            ENGINE_LOGGED.get_or_init(|| {
                log::info!(
                    "frust-render tier=engine (frust-engine strip pipeline, format={:?} into \
                     a {:?} swapchain, {}x{}, adapter `{}`)",
                    engine_format,
                    surface.config().format,
                    surface.config().width,
                    surface.config().height,
                    caps.adapter_name
                );
            });
            // Built from the LIVE device rather than from `caps`: the
            // adapter offering `TIMESTAMP_QUERY` is not the same statement
            // as the device having been created with it, and creating a
            // query set the device never enabled is a validation error
            // rather than a missing measurement. The ring asks the device
            // itself and goes inert when the answer is no.
            let timestamps = frust_gpu::diag::TimestampRing::new(
                device,
                &ctx.device_handle().queue,
                frust_engine::EngineSpan::COUNT,
                frust_engine::diag::TIMESTAMP_RING_LABEL,
            );
            TierBackend::Engine {
                engine: Box::new(engine),
                refused_frames: 0,
                shader_quads: Box::new(frust_engine::ShaderQuadPass::new(pipeline_cache.clone())),
                timestamps,
            }
        };
        debug_assert_eq!(
            next_phase(self.phase(), SurfaceEvent::Created),
            SurfacePhase::SurfaceReady,
            "Created must reach SurfaceReady"
        );
        // Pair the live pipeline cache with the adapter fingerprint its data is
        // framed under, so `pipeline_cache_data()` can hand back a validatable
        // blob without re-reading the adapter. `None` when the adapter lacks
        // `PIPELINE_CACHE`.
        let pipeline_cache = pipeline_cache.map(|cache| (cache, ctx.adapter_cache_key()));
        // Dropping the previous `SurfaceState` here tears down any prior surface
        // before the new one goes live: no surface outlives a
        // transition.
        //
        // Through `adopt_surface_state` for the resets that come with it: a
        // freshly (re)installed surface starts a new `Invalid`-reconfigure
        // episode (any prior streak belonged to the surface just replaced),
        // and the in-flight frame's stashes describe a frame of that same
        // replaced surface (the composite one holding handles on textures
        // from the cache that died with it), so carrying either into the new
        // surface's first `submit` would paint the old frame's pages and
        // colour onto it.
        self.adopt_surface_state(SurfaceState::Ready(Box::new(ReadySurface {
            surface,
            backend,
            pipeline_cache,
        })));
        Ok(())
    }

    /// Resizes the swapchain (`surfaceChanged`/`Resized`).
    ///
    /// Only acts in [`SurfacePhase::SurfaceReady`]; a resize with no surface is
    /// dropped. The renderer (and its compiled pipelines) is preserved — only
    /// the surface config and its sized attachments are recreated. Zero
    /// dimensions are ignored (a minimized window keeps its last valid size).
    pub fn on_surface_changed(&mut self, ctx: &RenderContext, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let SurfaceState::Ready(ready) = &mut self.state {
            ready
                .surface
                .resize(&ctx.device_handle().device, width, height);
            match &mut ready.backend {
                TierBackend::Engine { engine, .. } => {
                    // The engine's own extent-sized resources (its intermediate
                    // pool's parked entries, and the depth attachment it would
                    // own if the surface did not) are re-established here,
                    // off the frame path. The surface's own depth attachment
                    // was recreated by `EngineSurface::resize` above, in
                    // the same step that reconfigured the swapchain.
                    //
                    // The pipelines survive: only a FORMAT change would need a
                    // renderer warmed for a different target, and a resize
                    // never changes one — `EngineSurface::resize` rewrites
                    // the dimensions alone. Asserted rather than assumed, since a
                    // silent divergence would compile a fresh pipeline on the
                    // frame path for every frame that followed. Compared
                    // against the format the ARM implies, not the swapchain's
                    // own, so the un-premultiplying arm (whose frames target
                    // the engine's off-screen format) is not reported as drift
                    // on every resize.
                    let expected =
                        engine_target_format(&ready.surface.path, ready.surface.config().format);
                    if engine.format() != expected {
                        log::warn!(
                            "frust-render: engine renderer warmed for {:?} but this surface's \
                             frames now target {:?} — a format change must arrive as a fresh \
                             surface, not a resize",
                            engine.format(),
                            expected
                        );
                    }
                    engine.resize(&ctx.device_handle().device, width, height);
                }
            }
        }
    }

    /// Tears the surface down (`surfaceDestroyed`/`suspended`), transitioning to
    /// [`SurfacePhase::NoSurface`].
    ///
    /// Drops the `RenderSurface` (and its renderer) so no surface or texture
    /// outlives the platform's underlying window. Idempotent.
    pub fn on_surface_destroyed(&mut self) {
        debug_assert_eq!(
            next_phase(self.phase(), SurfaceEvent::Destroyed),
            SurfacePhase::NoSurface,
            "Destroyed must reach NoSurface"
        );
        // Release the in-flight frame's stash while the surface that produced
        // it is still alive: it describes a frame of the surface that is
        // dying, and a stale `base_color` is wrong to hand the next `submit`.
        // Idempotent — a cleared stash clears again to nothing, which is what
        // the repeated-`Destroyed` contract needs.
        self.clear_pending_frame();
        if let SurfaceState::Ready(ready) = &mut self.state {
            match &mut ready.backend {
                // Nothing to release ahead of time: every texture this
                // backend holds (resource textures, the intermediate pool, its
                // own depth attachment) dies with the renderer below, and the
                // surface's own depth attachment — plus, on the
                // un-premultiplying arm, its intermediate — dies with the
                // surface beside it.
                TierBackend::Engine { .. } => {}
            }
        }
        // The same door `install_surface` enters by, which repeats the
        // (idempotent) stash reset above and also clears the
        // `Invalid`-reconfigure streak — behaviour-neutral here, since that
        // streak belonged to the surface just torn down and `install_surface`
        // resets it again.
        self.adopt_surface_state(SurfaceState::NoSurface);
    }

    /// Encodes `scene` and presents it, clearing to `base_color`; returns the
    /// [`FrameOutcome`] so the shell can react.
    ///
    /// This is a thin convenience wrapper over the two-phase seam
    /// [`Self::encode`] + [`Self::present`]: it encodes, and — unless the frame
    /// was skipped for want of a renderable surface — presents. A caller that
    /// wants to attribute GPU encode cost separately from the swapchain-acquire
    /// (vsync) wait — the render-thread-split decision hinges on that split —
    /// calls the two entry points directly and times each with
    /// its own clock (timing stays shell-owned; this crate reads no clock — see
    /// `frust-shell-common::perf`'s layering note).
    ///
    /// In [`SurfacePhase::NoSurface`]/[`SurfacePhase::SurfaceLost`] the frame is
    /// dropped ([`FrameOutcome::Skipped`]) — never panicking, never queueing.
    /// On an `Outdated` acquire the surface is reconfigured and
    /// [`FrameOutcome::Redraw`] asks the shell to try again; on `Lost` the
    /// surface is dropped, the machine moves to [`SurfacePhase::SurfaceLost`],
    /// and [`FrameOutcome::SurfaceLost`] tells the shell to recreate it.
    ///
    /// Nothing accumulates across calls: each frame re-encodes from `scene`.
    pub fn render(
        &mut self,
        ctx: &RenderContext,
        scene: &frust_scene::Scene,
        base_color: peniko::Color,
    ) -> Result<FrameOutcome> {
        match self.encode(ctx, scene, base_color)? {
            EncodeOutcome::Skipped => Ok(FrameOutcome::Skipped),
            EncodeOutcome::Encoded => self.present(ctx),
        }
    }

    /// Phase 1 of the frame — the **encode** span: take the frame's scene. It
    /// does **not** touch the swapchain, so a caller timing this call in
    /// isolation measures encode cost with no vsync wait folded in. This copies
    /// the display list and stashes `base_color`; the GPU render moves to
    /// [`Self::submit`] (it needs the acquired swapchain texture), so
    /// `encode_us` is a memcpy and nothing else.
    ///
    /// Returns [`EncodeOutcome::Skipped`] (no work done, nothing queued) in any
    /// phase but [`SurfacePhase::SurfaceReady`]; otherwise
    /// [`EncodeOutcome::Encoded`], after which [`Self::present`] finishes the
    /// frame. Nothing accumulates across frames.
    // `ctx` is unused on this tier — all of its GPU work happens in `submit`
    // — but the parameter stays in the public signature so a shell's call
    // site does not have to special-case it.
    #[allow(unused_variables)]
    pub fn encode(
        &mut self,
        ctx: &RenderContext,
        scene: &frust_scene::Scene,
        base_color: peniko::Color,
    ) -> Result<EncodeOutcome> {
        // Frames are dropped in every phase but SurfaceReady.
        if !self.phase().can_render() {
            return Ok(EncodeOutcome::Skipped);
        }
        // Disjoint field borrows: the live surface and the engine arms' colour
        // stash; `consecutive_invalid` belongs to `present`.
        let Self {
            state,
            pending_base_color,
            engine_scene,
            ..
        } = self;
        let SurfaceState::Ready(ready) = state else {
            // Unreachable: `can_render()` above guaranteed SurfaceReady.
            return Ok(EncodeOutcome::Skipped);
        };
        // Reborrow through the `Box` once so `ready.backend` and `ready.surface`
        // are disjoint field borrows of a plain `&mut ReadySurface` — the tier
        // `match` below mutates `backend` while reading `surface`, which the
        // borrow checker only allows on a single deref.
        let ready: &mut ReadySurface = ready;

        // Encode this frame's pixels. The engine renders in `submit`, which
        // is why its surface is configured on one of the two engine paths
        // (`context::choose_engine_render_path`): straight into the acquired
        // swapchain view, or into a surface-owned intermediate the same
        // `submit` un-premultiplies from. `RenderPath` has no other variant to
        // wire wrong — `submit_impl`'s own exhaustive match over `&surface.path`
        // is where a future third arm must be handled, as a compile error
        // rather than a runtime check here.
        // The frame's whole CPU-side encode on this arm: copy the display list
        // somewhere that outlives the borrow, since `EngineRenderer::encode`
        // compiles it itself in `submit`. `clone_from` reuses the buffer's
        // capacity, so a steady-state frame allocates nothing — see the
        // field's measurement note.
        engine_scene.clone_from(scene);
        // Carried to `submit`, where the render that consumes it runs.
        *pending_base_color = Some(base_color);
        // The scene's serial, committed by the `submit` that renders it.
        self.pending_serial = Some((self.scene_serial, scene_records_retained(scene)));

        Ok(EncodeOutcome::Encoded)
    }

    /// Phase 2 of the frame — the **present** span: a thin wrapper over the
    /// two-phase [`Self::acquire`] + [`Self::submit`] seam, kept for callers
    /// (and the [`Self::render`] convenience wrapper) that time present as one
    /// span. It acquires the swapchain texture (the blocking vsync/present
    /// wait) and, on success, renders/submits/presents it.
    ///
    /// A caller wanting the finer **acquire** (blocking vsync wait) vs
    /// **submit** (GPU render + queue-submit + present) attribution — to
    /// separate GPU saturation from render cost — calls
    /// [`Self::acquire`] and [`Self::submit`] directly, timing each with its own
    /// clock (timing stays shell-owned; this crate reads no clock — see
    /// `frust-shell-common::perf`'s layering note). `present`'s combined span
    /// equals `acquire` + `submit` by construction.
    ///
    /// Assumes [`Self::encode`] has already copied this frame's display list.
    /// In any phase but [`SurfacePhase::SurfaceReady`] the call is a
    /// no-op returning [`FrameOutcome::Skipped`]. On an `Outdated`
    /// acquire the surface is reconfigured and [`FrameOutcome::Redraw`] asks the
    /// shell to try again; on `Lost` the surface is dropped, the machine moves
    /// to [`SurfacePhase::SurfaceLost`], and [`FrameOutcome::SurfaceLost`] tells
    /// the shell to recreate it.
    pub fn present(&mut self, ctx: &RenderContext) -> Result<FrameOutcome> {
        match self.acquire(ctx)? {
            AcquireOutcome::Acquired => self.submit(ctx),
            AcquireOutcome::Reconfigured => Ok(FrameOutcome::Redraw),
            AcquireOutcome::Lost => Ok(FrameOutcome::SurfaceLost),
            AcquireOutcome::Skipped => Ok(FrameOutcome::Skipped),
        }
    }

    /// Phase 2a of the frame — the **acquire** sub-span: acquire the swapchain
    /// texture (the blocking vsync/present wait, per the surface's present mode),
    /// classify the result, and — on a usable acquire — stash the
    /// texture for [`Self::submit`] to blit into. Timing this call in isolation
    /// attributes the blocking present/vsync wait separately from [`Self::submit`]'s
    /// blit/queue-submit work — the split needed to separate GPU saturation
    /// from blit cost.
    ///
    /// Returns [`AcquireOutcome::Acquired`] when a texture was stashed (the
    /// caller must follow with [`Self::submit`]); otherwise a terminal outcome —
    /// [`AcquireOutcome::Reconfigured`] (`Outdated` acquire, surface reconfigured),
    /// [`AcquireOutcome::Lost`] (surface dropped, now `SurfaceLost`), or
    /// [`AcquireOutcome::Skipped`] (no renderable surface or a transient failure).
    /// In any phase but [`SurfacePhase::SurfaceReady`] it is a no-op returning
    /// [`AcquireOutcome::Skipped`].
    pub fn acquire(&mut self, ctx: &RenderContext) -> Result<AcquireOutcome> {
        // Frames are dropped in every phase but SurfaceReady.
        if !self.phase().can_render() {
            return Ok(AcquireOutcome::Skipped);
        }
        // Disjoint field borrows: the live surface, the consecutive-Invalid
        // counter, and the stash slot (the reusable scene belongs to `encode`).
        let Self {
            state,
            consecutive_invalid,
            pending_present,
            ..
        } = self;
        let SurfaceState::Ready(ready) = state else {
            // Unreachable: `can_render()` above guaranteed SurfaceReady.
            return Ok(AcquireOutcome::Skipped);
        };
        let ready: &mut ReadySurface = ready;

        use wgpu::CurrentSurfaceTexture as Cst;
        let acquired = ready.surface.gpu.surface().get_current_texture();
        let status = match &acquired {
            Cst::Success(_) | Cst::Suboptimal(_) => AcquireStatus::Usable,
            Cst::Outdated => AcquireStatus::Outdated,
            Cst::Lost => AcquireStatus::Lost,
            Cst::Timeout | Cst::Occluded => AcquireStatus::Transient,
            Cst::Validation => AcquireStatus::Invalid,
        };

        let action = decide_acquire(status, *consecutive_invalid);
        // Reset-on-success / increment-on-retry / reset-on-give-up (the same
        // discipline mirrored from iOS's `recreate_failures`) — pure and
        // unit-tested in `next_invalid_streak` itself.
        if status == AcquireStatus::Invalid && action == AcquireAction::Lose {
            // The cap was hit rather than a genuine `Lost` acquire: log once so
            // the giving-up transition is visible before the streak resets.
            log::warn!(
                "frust-render: giving up on Invalid-acquire reconfigure after \
                 {consecutive_invalid} consecutive attempts; surface lost"
            );
        }
        *consecutive_invalid = next_invalid_streak(status, action, *consecutive_invalid);

        let outcome = match action {
            AcquireAction::Present => {
                let surface_texture = match acquired {
                    Cst::Success(t) | Cst::Suboptimal(t) => t,
                    // `decide_acquire(Usable, _) == Present`, and only Success/
                    // Suboptimal classify as Usable — so this is unreachable.
                    // Report rather than panic to honour the no-panic invariant.
                    _ => {
                        return Err(anyhow!("frust-render: acquire classification desync"));
                    }
                };
                // Stash the acquired texture for `submit`; the blocking vsync wait
                // ended above, so timing stops here for the acquire sub-span.
                *pending_present = Some(surface_texture);
                AcquireOutcome::Acquired
            }
            AcquireAction::Reconfigure => {
                ready.surface.reconfigure(&ctx.device_handle().device);
                AcquireOutcome::Reconfigured
            }
            AcquireAction::Lose => {
                debug_assert_eq!(
                    next_phase(SurfacePhase::SurfaceReady, SurfaceEvent::Lost),
                    SurfacePhase::SurfaceLost,
                    "Lost must reach SurfaceLost from SurfaceReady"
                );
                AcquireOutcome::Lost
            }
            AcquireAction::Skip => AcquireOutcome::Skipped,
        };
        if matches!(outcome, AcquireOutcome::Lost) {
            // Drop the surface and its outstanding resources before returning
            // so the shell can recreate cleanly — through the same door
            // `install_surface` and `on_surface_destroyed` use, once the field
            // borrows above have ended. That also drops the in-flight frame's
            // stash: no `submit` will ever consume it now, and a stale
            // `base_color` describes a surface that no longer exists. The
            // streak was already reset above (`next_invalid_streak`,
            // reset-on-give-up); the door zeroing it again is a no-op. The
            // shell's own recovery path (recreate on
            // resize/redraw/surfaceChanged) then starts a fresh episode.
            self.adopt_surface_state(SurfaceState::Lost);
        }
        Ok(outcome)
    }

    /// Phase 2b of the frame — the **submit** sub-span: turn the swapchain
    /// texture [`Self::acquire`] stashed into a presented frame, then present it.
    /// Timing this call in isolation attributes the submit work separately from
    /// [`Self::acquire`]'s blocking vsync wait.
    ///
    /// What the submit span contains: the whole GPU render runs HERE,
    /// targeting the acquired swapchain texture (which does not exist until
    /// [`Self::acquire`]) or the surface's intermediate, then present. So the
    /// frame's GPU cost lands in `submit_us`; `encode_us` is a memcpy.
    ///
    /// **v3 wire mapping.** The `acquire_us`/`submit_us` field names and the
    /// wire format are unchanged (`perf.rs` is untouched). The swapchain
    /// **acquire** (the blocking vsync wait) happens in [`Self::acquire`] and
    /// is recorded in `acquire_us`, so acquire precedes the GPU render, which
    /// lives entirely in this span.
    ///
    /// Must follow an [`AcquireOutcome::Acquired`] result from [`Self::acquire`]
    /// on the same frame — it consumes the stashed texture. With nothing stashed
    /// (no prior `Acquired`, or the surface vanished between the two calls) it is
    /// a no-op returning [`FrameOutcome::Skipped`]; otherwise
    /// [`FrameOutcome::Rendered`].
    ///
    /// A caller that must issue the present itself, on another thread, calls
    /// [`Self::submit_deferred`] instead.
    pub fn submit(&mut self, ctx: &RenderContext) -> Result<FrameOutcome> {
        #[cfg(feature = "perf-trace")]
        let measuring = !self.first_submit_seen;
        let (outcome, presentable) = self.submit_impl(ctx)?;
        if let Some(surface_texture) = presentable {
            #[cfg(feature = "perf-trace")]
            let present_start =
                (measuring && self.first_submit.is_some()).then(std::time::Instant::now);
            ctx.device_handle().queue.present(surface_texture);
            #[cfg(feature = "perf-trace")]
            if let (Some(start), Some(breakdown)) = (present_start, self.first_submit.as_mut()) {
                breakdown.queue = breakdown.queue.saturating_add(start.elapsed());
            }
        }
        Ok(outcome)
    }

    /// [`Self::submit`] with the final present step handed back to the caller
    /// instead of issued here: identical GPU work (render + queue-submit), but
    /// the acquired swapchain frame is returned as an opaque, `Send`
    /// [`DeferredPresent`] the caller presents on a thread of its choosing.
    ///
    /// The iOS `presentsWithTransaction` contract is why this exists — see
    /// [`DeferredPresent`]'s docs for the full mechanism. Everything else is
    /// unchanged: same outcomes, same stash discipline, and
    /// `Some(DeferredPresent)` accompanies exactly the
    /// [`FrameOutcome::Rendered`] case (every other outcome carries `None`).
    /// Dropping the returned handle instead of presenting it skips that frame's
    /// present without error.
    pub fn submit_deferred(
        &mut self,
        ctx: &RenderContext,
    ) -> Result<(FrameOutcome, Option<DeferredPresent>)> {
        let (outcome, presentable) = self.submit_impl(ctx)?;
        Ok((
            outcome,
            // The queue is read INSIDE the map, never before it: `ctx`'s device
            // is created lazily by surface creation, and reading it eagerly
            // would panic on the no-surface path that returns `None` here.
            presentable.map(|texture| DeferredPresent {
                texture,
                queue: ctx.device_handle().queue.clone(),
            }),
        ))
    }

    /// Shared body of [`Self::submit`]/[`Self::submit_deferred`]: everything up
    /// to (but not including) `Queue::present`, handing the acquired frame
    /// back so each wrapper decides where the present happens.
    fn submit_impl(
        &mut self,
        ctx: &RenderContext,
    ) -> Result<(FrameOutcome, Option<wgpu::SurfaceTexture>)> {
        // Nothing acquired this frame (no prior `Acquired`): nothing to present.
        let Some(surface_texture) = self.pending_present.take() else {
            return Ok((FrameOutcome::Skipped, None));
        };
        // The stashed texture is owned, but the encoded frame (the engine's
        // copied scene) lives on `self` — if the surface vanished between
        // `acquire` and `submit`, drop the texture and skip rather than
        // present a stale frame.
        // Only the first rendered frame is measured; a refused or skipped one
        // leaves the probe armed for the next.
        #[cfg(feature = "perf-trace")]
        let mut probe = SubmitProbe::new(!self.first_submit_seen);
        #[cfg(not(feature = "perf-trace"))]
        let mut probe = SubmitProbe::new(false);
        let Self {
            state,
            pending_base_color,
            engine_scene,
            partial_repaint,
            owed_damage,
            #[cfg(feature = "perf-trace")]
            partial_trace,
            ..
        } = self;
        let SurfaceState::Ready(ready) = state else {
            return Ok((FrameOutcome::Skipped, None));
        };
        let ReadySurface {
            surface, backend, ..
        } = &mut **ready;
        // The partial-repaint plan (`Self::set_partial_repaint`): `None` with
        // the spike off — every frame below is then exactly what it always
        // was — otherwise the scissor the frame owes, `None` for a full one.
        let partial_damage = partial_repaint
            .then(|| owed_damage.scissor(crate::context::scene_forces_full_frame(engine_scene)));
        // Peeked, not taken: each arm takes it where it always has.
        let partial_base = pending_base_color.unwrap_or(peniko::Color::BLACK);
        let device_handle = ctx.device_handle();
        let swapchain_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // Read before the arm is borrowed mutably below (the direct arm's
        // backdrop root is created lazily, inside the match).
        let surface_format = surface.config().format;
        let (surface_width, surface_height) = (surface.config().width, surface.config().height);
        let engine_format = engine_target_format(&surface.path, surface_format);

        let partial_root = &mut surface.partial;
        match &mut surface.path {
            // The engine tier's whole frame: one encoder, one
            // `EngineRenderer::encode` into the acquired swapchain view and
            // the surface's own depth attachment, one submit, then the
            // engine's end-of-frame maintenance. The renderer records into the
            // encoder and never submits, so every pass of the frame lands in
            // this one command buffer, in order, against the texture about to
            // be presented.
            //
            // A frame whose scene carries a backdrop blur is the one exception
            // to "straight into the swapchain": it reads a region of its own
            // target back, which a `RENDER_ATTACHMENT`-only swapchain cannot
            // be read from, so it renders into the surface's backdrop root
            // (same format, same extent, so the same pipelines and depth
            // attachment) and one copy pass presents it — the frame's only
            // extra pass, paid on exactly the frames that blur
            // (`context::frame_renders_via_intermediate`).
            RenderPath::EngineDirect { depth, backdrop } => {
                let TierBackend::Engine {
                    engine,
                    refused_frames,
                    shader_quads,
                    timestamps,
                } = backend;
                // With partial repaint on, every frame is presented from the
                // surface's accumulator: a full frame renders straight into
                // it, a confined one into the backdrop root (whose present
                // pass the accumulator is presented through either way).
                let partial = partial_damage.map(|damage| {
                    partial_root.plan(
                        &device_handle.device,
                        surface_format,
                        surface_width,
                        surface_height,
                        damage,
                        partial_base,
                    )
                });
                let root = (partial.is_some()
                    || crate::context::frame_renders_via_intermediate(
                        crate::context::RenderPathKind::EngineDirect,
                        crate::context::scene_carries_backdrop(engine_scene),
                    ))
                .then(|| {
                    backdrop.prepare(
                        &device_handle.device,
                        surface_format,
                        surface_width,
                        surface_height,
                    )
                });
                let engine_view = match (&partial, root) {
                    (Some(frame), _) if frame.scissor.is_none() => frame.accumulator,
                    (_, Some((view, _))) => view,
                    _ => &swapchain_view,
                };
                if partial.is_some() || engine.frame_scissor().is_some() {
                    engine.set_frame_scissor(partial.as_ref().and_then(|frame| frame.scissor));
                }
                // Set in `encode`; a well-formed frame always encoded first —
                // the same fallback every other arm takes.
                let base_color = pending_base_color.take().unwrap_or(peniko::Color::BLACK);
                let mut encoder =
                    device_handle
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frust-render engine"),
                        });
                // Whatever the process registered ahead of the scene:
                // caller-supplied passes recorded into this SAME encoder and
                // binding their own targets into the engine's external-texture
                // registry, plus the unbind of any id whose pass was
                // unregistered since the last frame (see
                // `crate::external_pass`). Ahead of the shader quads for the
                // same two orderings they need, and a no-op — one lock, no
                // encoder work — on the process that registered none.
                crate::external_pass::run_external_passes(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine,
                );
                // The frame's shader quads, rendered into this SAME encoder
                // ahead of the scene's own passes and registered with the
                // engine before it compiles: a quad's target must already be
                // bound when the display list naming it is walked, and must be
                // written before the pass that samples it runs. One encoder
                // gives both orderings for free.
                shader_quads.prepare(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine_scene,
                    Affine::IDENTITY,
                    engine,
                );
                // Opens this frame's ring slot and harvests whatever an earlier
                // frame left mapped in it — the one place a reading becomes
                // visible to `gpu_pass_timings`. A no-op on an inert ring.
                timestamps.begin_frame(&device_handle.device);
                probe.encode_begin(engine);
                let result = engine.encode_traced(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine_scene,
                    frust_engine::EngineTarget {
                        view: engine_view,
                        format: surface_format,
                        width: surface_width,
                        height: surface_height,
                        // The surface's own attachment, recreated with the
                        // swapchain (`context::RenderPath::EngineDirect`).
                        // Nothing else writes it, so it is not pre-cleared:
                        // the frame's first depth-using pass clears it.
                        depth: Some(depth.view()),
                        // The engine's strip pipelines write premultiplied
                        // alpha, which is what this surface's swapchain
                        // expects — an opaque one ignores alpha, and a
                        // premultiplied-expecting translucent one takes it
                        // as-is. A swapchain whose compositor genuinely
                        // stores STRAIGHT alpha is the other arm below
                        // (`context::choose_engine_render_path`); a Metal
                        // `PostMultiplied` swapchain (iOS included) lands
                        // HERE despite its name, because Metal's own
                        // compositor reads that mode premultiplied regardless
                        // (`context::compositor_expects_premultiplied`).
                        output: frust_engine::OutputAlpha::Premultiplied,
                    },
                    base_color,
                    // The engine arm renders at the surface's own resolution.
                    Affine::IDENTITY,
                    frust_engine::FrameTimestamps::new(timestamps),
                );
                probe.encode_end(engine);
                match result {
                    Ok(()) => {
                        // A partial-repaint frame: the confined rectangle
                        // (as the engine applied it) copied into the
                        // accumulator, then the whole accumulator presented.
                        if let Some(frame) = partial.as_ref() {
                            if let (Some(scissor), Some((view, _))) =
                                (engine.last_frame_scissor(), root)
                            {
                                crate::context::copy_damage(
                                    &mut encoder,
                                    view,
                                    frame.accumulator,
                                    scissor,
                                );
                            }
                            if let Some((_, present)) = root {
                                present.record(
                                    &device_handle.device,
                                    &device_handle.queue,
                                    &mut encoder,
                                    frame.accumulator,
                                    &swapchain_view,
                                    (surface_width, surface_height),
                                    frust_engine::FrameTimestamps::new(timestamps)
                                        .writes(frust_engine::EngineSpan::Blit),
                                );
                            }
                        }
                        // The backdrop root's copy into the swapchain, in the
                        // frame's own encoder after every engine pass, charged
                        // to `blit` like the straight-alpha arm's conversion.
                        else if let Some((view, present)) = root {
                            present.record(
                                &device_handle.device,
                                &device_handle.queue,
                                &mut encoder,
                                view,
                                &swapchain_view,
                                (surface_width, surface_height),
                                frust_engine::FrameTimestamps::new(timestamps)
                                    .writes(frust_engine::EngineSpan::Blit),
                            );
                        }
                        // Recorded into the frame's own encoder, after every
                        // pass that wrote a query and before the one submit —
                        // a resolve in a second command buffer would race the
                        // passes it reads.
                        timestamps.resolve(&mut encoder);
                        probe.queue_begin();
                        device_handle.queue.submit([encoder.finish()]);
                        probe.queue_end();
                        timestamps.end_frame();
                        engine.end_frame(&device_handle.queue);
                        if partial.is_some() {
                            // The accumulator holds this frame whole now; the
                            // damage it owed is paid.
                            #[cfg(feature = "perf-trace")]
                            partial_trace.record(
                                engine.last_frame_scissor(),
                                surface_width,
                                surface_height,
                            );
                            partial_root.commit(base_color);
                            *owed_damage = OwedDamage::Unstated;
                        }
                    }
                    Err(error) => {
                        // A refused frame left the encoder exactly as it was
                        // found, so there is nothing to submit and the
                        // acquired texture holds no frame — presenting it
                        // would show undefined content. The frame is dropped
                        // instead (the texture goes with this `None`), and the
                        // refusal is counted rather than logged per vsync.
                        // The ring's slot is abandoned for the same reason
                        // nothing is presented: no pass ran, so there is no
                        // measurement to map.
                        timestamps.abandon_frame();
                        *refused_frames = refused_frames.saturating_add(1);
                        log_engine_refusal(*refused_frames, &error);
                        engine.end_frame(&device_handle.queue);
                        return Ok((FrameOutcome::Skipped, None));
                    }
                }
            }
            // The engine tier's straight-alpha arm: the same one-encoder frame
            // as above with one pass appended — `engine.encode` into the
            // surface's own intermediate, then the un-premultiplying fragment
            // pass from that intermediate into the acquired swapchain texture,
            // recorded into the SAME encoder so the conversion can never
            // execute against a frame that was not submitted with it. One
            // submit, then the engine's end-of-frame maintenance.
            //
            // The deferred-present contract is untouched: this is still
            // `submit_impl`, so a caller that hands the frame back
            // un-presented (`Self::submit_deferred`) gets a fully converted
            // swapchain texture to present inside its own transaction. (iOS
            // itself never reaches this arm — its sole translucent mode is a
            // Metal `PostMultiplied` swapchain, which
            // `context::choose_engine_render_path` now routes onto
            // `EngineDirect` above instead; this arm serves a genuinely
            // straight-alpha, non-Metal `PostMultiplied` compositor.)
            RenderPath::EngineDirectUnpremultiply {
                depth,
                intermediate_view,
                present,
            } => {
                let TierBackend::Engine {
                    engine,
                    refused_frames,
                    shader_quads,
                    timestamps,
                } = backend;
                // With partial repaint on, the frame is presented from the
                // accumulator: a full frame renders straight into it, a
                // confined one into this arm's own intermediate.
                let partial = partial_damage.map(|damage| {
                    partial_root.plan(
                        &device_handle.device,
                        engine_format,
                        surface_width,
                        surface_height,
                        damage,
                        partial_base,
                    )
                });
                let engine_view = match &partial {
                    Some(frame) if frame.scissor.is_none() => frame.accumulator,
                    _ => &*intermediate_view,
                };
                if partial.is_some() || engine.frame_scissor().is_some() {
                    engine.set_frame_scissor(partial.as_ref().and_then(|frame| frame.scissor));
                }
                let base_color = pending_base_color.take().unwrap_or(peniko::Color::BLACK);
                let mut encoder =
                    device_handle
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frust-render engine unpremultiply"),
                        });
                // Whatever the process registered ahead of the scene:
                // caller-supplied passes recorded into this SAME encoder and
                // binding their own targets into the engine's external-texture
                // registry, plus the unbind of any id whose pass was
                // unregistered since the last frame (see
                // `crate::external_pass`). Ahead of the shader quads for the
                // same two orderings they need, and a no-op — one lock, no
                // encoder work — on the process that registered none.
                crate::external_pass::run_external_passes(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine,
                );
                // The same pre-pass the direct arm records, for the same two
                // orderings — the conversion pass appended after the frame
                // changes neither of them.
                shader_quads.prepare(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine_scene,
                    Affine::IDENTITY,
                    engine,
                );
                timestamps.begin_frame(&device_handle.device);
                probe.encode_begin(engine);
                let result = engine.encode_traced(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine_scene,
                    frust_engine::EngineTarget {
                        // The frame lands in the intermediate, never in the
                        // swapchain: what the swapchain receives is the
                        // conversion's output.
                        view: engine_view,
                        format: engine_format,
                        width: surface_width,
                        height: surface_height,
                        depth: Some(depth.view()),
                        // The INTERMEDIATE's own convention, which is
                        // premultiplied like every other engine target — the
                        // strip pipelines are fixed premultiplied, so this is
                        // what keeps the frame's base colour in the same space
                        // as everything drawn over it. The SURFACE's straight
                        // alpha is what selected this arm
                        // (`UnpremultiplyPass::selected_by`) and is served by
                        // the pass below, not by relabelling this target.
                        output: frust_engine::OutputAlpha::Premultiplied,
                    },
                    base_color,
                    Affine::IDENTITY,
                    frust_engine::FrameTimestamps::new(timestamps),
                );
                probe.encode_end(engine);
                match result {
                    Ok(()) => {
                        // The conversion pass itself: charged to `blit` with a
                        // fresh pair from the same ring `encode_traced` just
                        // recorded the frame's other spans into — `None` on an
                        // inert ring (no `perf-trace`, or the device never got
                        // `TIMESTAMP_QUERY`), exactly like every other pass.
                        // A partial-repaint frame converts the accumulator,
                        // after the confined rectangle was copied into it.
                        let source = match partial.as_ref() {
                            Some(frame) => {
                                if let Some(scissor) = engine.last_frame_scissor() {
                                    crate::context::copy_damage(
                                        &mut encoder,
                                        intermediate_view,
                                        frame.accumulator,
                                        scissor,
                                    );
                                }
                                frame.accumulator
                            }
                            None => &*intermediate_view,
                        };
                        present.record(
                            &device_handle.device,
                            &mut encoder,
                            source,
                            &swapchain_view,
                            frust_engine::FrameTimestamps::new(timestamps)
                                .writes(frust_engine::EngineSpan::Blit),
                        );
                        timestamps.resolve(&mut encoder);
                        probe.queue_begin();
                        device_handle.queue.submit([encoder.finish()]);
                        probe.queue_end();
                        timestamps.end_frame();
                        engine.end_frame(&device_handle.queue);
                        if partial.is_some() {
                            // The accumulator holds this frame whole now; the
                            // damage it owed is paid.
                            #[cfg(feature = "perf-trace")]
                            partial_trace.record(
                                engine.last_frame_scissor(),
                                surface_width,
                                surface_height,
                            );
                            partial_root.commit(base_color);
                            *owed_damage = OwedDamage::Unstated;
                        }
                    }
                    Err(error) => {
                        // Identical to the direct arm's refusal, and for the
                        // same reason — with one extra consequence worth
                        // stating: the conversion pass is NOT recorded either,
                        // so the intermediate's stale contents are never
                        // converted onto the acquired texture. Nothing is
                        // submitted, the frame is dropped, and the previously
                        // presented content persists.
                        timestamps.abandon_frame();
                        *refused_frames = refused_frames.saturating_add(1);
                        log_engine_refusal(*refused_frames, &error);
                        engine.end_frame(&device_handle.queue);
                        return Ok((FrameOutcome::Skipped, None));
                    }
                }
            }
        }
        #[cfg(feature = "perf-trace")]
        if probe.armed && !self.first_submit_seen {
            self.first_submit_seen = true;
            self.first_submit = Some(probe.breakdown);
        }
        // The frame's retained-store commit completed: from here on the
        // ledger describes the store after this scene (see `committed_serial`).
        if let Some((serial, records)) = self.pending_serial.take() {
            self.committed_serial = serial;
            self.recording_committed |= records;
        }
        Ok((FrameOutcome::Rendered, Some(surface_texture)))
    }
}

/// The target format a surface's [`frust_engine::EngineRenderer`] must be
/// warmed for, given the arm it was configured on and the format its swapchain
/// reports.
///
/// The engine warms its strip pipelines for exactly one colour format, and a
/// pipeline whose colour target disagrees with its attachment is a validation
/// error rather than a mis-render. On [`RenderPath::EngineDirect`] the frame's
/// attachment IS the swapchain, so that is the format; on
/// [`RenderPath::EngineDirectUnpremultiply`] the frame's attachment is the
/// intermediate instead, whose format is the engine's own off-screen one
/// (`context::create_engine_intermediate` creates it with exactly this), and
/// only the conversion pass speaks the swapchain's format.
///
/// One answer for both the install site and the resize-time drift check
/// below, so the two cannot disagree about which format this surface's
/// renderer was built for.
fn engine_target_format(
    path: &RenderPath,
    surface_format: wgpu::TextureFormat,
) -> wgpu::TextureFormat {
    match path {
        RenderPath::EngineDirectUnpremultiply { .. } => {
            frust_engine::gpu::pipelines::INTERMEDIATE_FORMAT
        }
        RenderPath::EngineDirect { .. } => surface_format,
    }
}

/// Reports the `count`-th frame this surface's engine renderer refused, under
/// [`frust_gpu::context::decide_log_action`]'s latch.
///
/// A refusal is a per-frame event on a path that can reproduce every vsync — a
/// scheduler escalation on a layer shape the engine does not serve, or a
/// capacity ceiling a busy frame keeps hitting — so logging each one would
/// bury the log without adding information after the first few. The latch is
/// the one the uncaptured-`wgpu`-error handler already uses: the first few
/// refusals are logged in full, the latch is announced once naming the running
/// total, and the periodic debug bump keeps the counter visible afterwards.
/// Every line carries the count, so a capture read later says how many frames
/// were lost, not merely that some were.
fn log_engine_refusal(count: u32, error: &frust_engine::EngineError) {
    match frust_gpu::context::decide_log_action(count) {
        frust_gpu::context::LogAction::Log => {
            log::warn!(
                "frust-render: engine refused frame {count} on this surface — {error}; the \
                 frame is dropped rather than presented"
            );
        }
        frust_gpu::context::LogAction::SuppressionNotice => {
            log::warn!(
                "frust-render: further engine frame refusals suppressed (total so far: {count})"
            );
        }
        frust_gpu::context::LogAction::Silent { debug_bump } => {
            if debug_bump {
                log::debug!(
                    "frust-render: engine frame refusals now {count} on this surface (still \
                     suppressed)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes every test in this binary that creates a GPU device, the
    /// same guard the workspace's other GPU suites take: the NVIDIA Vulkan
    /// driver serializes `vkDestroyDevice` against other Vulkan work on a
    /// process-global mutex, and two tests tearing devices down at once
    /// have deadlocked inside it. Poison is ignored deliberately — one
    /// test's failure must not cascade into its siblings.
    static RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A device on the environment's adapter, with the limits production asks
    /// for, and that adapter's capabilities.
    fn test_device() -> (wgpu::Device, wgpu::Queue, frust_gpu::TierCaps) {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks
            // the GPU on a multi-adapter host instead of the run silently
            // landing on whichever one enumerates first.
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            println!("frust-render engine arm adapter: {:?}", adapter.get_info());
            let caps = frust_gpu::TierCaps::probe(&adapter);
            // The limits production asks for, not the defaults, so the target
            // this test builds is sized against the same ceiling
            // `context::extent_within_limits` refuses on.
            let limits = crate::context::effective_limits(
                adapter.limits(),
                crate::context::is_ios_simulator(),
            );
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-render engine arm test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: limits,
                    ..Default::default()
                })
                .await
                .expect("failed to create device");
            (device, queue, caps)
        })
    }

    use frust_engine::renderer::ScissorRect;

    #[test]
    fn partial_repaint_is_off_until_asked_and_owes_a_full_frame_when_nothing_was_stated() {
        let mut renderer = SurfaceRenderer::new();
        assert!(!renderer.partial_repaint(), "off by default");
        assert_eq!(renderer.owed_damage, OwedDamage::Unstated);
        assert_eq!(
            renderer.owed_damage.scissor(false),
            None,
            "unstated is full"
        );
        renderer.set_partial_repaint(true);
        assert!(renderer.partial_repaint());
        renderer.set_partial_repaint(false);
        assert!(!renderer.partial_repaint());
    }

    #[test]
    fn stated_damage_is_rounded_out_grown_by_a_pixel_and_owed_until_presented() {
        let mut renderer = SurfaceRenderer::new();
        renderer.set_frame_damage(Some(kurbo::Rect::new(10.2, 20.7, 30.5, 40.0)));
        assert_eq!(
            renderer.owed_damage.scissor(false),
            Some(ScissorRect::from_corners(9, 19, 32, 41))
        );
        // A second frame's damage before anything was presented: both owed.
        renderer.set_frame_damage(Some(kurbo::Rect::new(50.0, 50.0, 60.0, 60.0)));
        assert_eq!(
            renderer.owed_damage.scissor(false),
            Some(ScissorRect::from_corners(9, 19, 61, 61))
        );
        // A scene drawing an untracked pixel source forces the frame full.
        assert_eq!(renderer.owed_damage.scissor(true), None);
        // The whole surface absorbs everything.
        renderer.set_frame_damage(None);
        assert_eq!(renderer.owed_damage, OwedDamage::Full);
        renderer.set_frame_damage(Some(kurbo::Rect::new(0.0, 0.0, 1.0, 1.0)));
        assert_eq!(renderer.owed_damage, OwedDamage::Full);
    }

    #[test]
    fn nothing_changed_is_an_empty_scissor_that_any_real_damage_replaces() {
        let empty = OwedDamage::Unstated.with(Some(ScissorRect::from_corners(0, 0, 0, 0)));
        assert_eq!(
            empty.scissor(false),
            Some(ScissorRect::from_corners(0, 0, 0, 0))
        );
        let rect = ScissorRect::from_corners(4, 4, 8, 8);
        assert_eq!(empty.with(Some(rect)), OwedDamage::Rect(rect));
        assert_eq!(
            OwedDamage::Rect(rect).with(Some(ScissorRect::from_corners(0, 0, 0, 0))),
            OwedDamage::Rect(rect)
        );
    }

    #[test]
    fn a_device_scale_change_owes_a_full_frame() {
        let mut renderer = SurfaceRenderer::new();
        renderer.set_frame_damage(Some(kurbo::Rect::new(0.0, 0.0, 4.0, 4.0)));
        assert!(renderer.note_device_scale(2.0));
        assert_eq!(renderer.owed_damage, OwedDamage::Full);
    }

    /// The accumulator's frame plan on a real device: full until it holds a
    /// whole frame, confined after, full again for a changed base colour, a
    /// damage covering the target, an invalidation or a new extent.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn the_partial_root_plans_full_frames_until_it_holds_one() {
        let _serialized = RENDER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, _queue, _caps) = test_device();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let black = peniko::Color::BLACK;
        let rect = ScissorRect::from_corners(4, 4, 8, 8);
        let mut root = crate::context::PartialRoot::default();

        assert_eq!(
            root.plan(&device, format, 64, 64, Some(rect), black)
                .scissor,
            None,
            "an accumulator holding no frame takes a full one"
        );
        root.commit(black);
        assert_eq!(
            root.plan(&device, format, 64, 64, Some(rect), black)
                .scissor,
            Some(rect),
            "then frames are confined to their damage"
        );
        assert_eq!(
            root.plan(&device, format, 64, 64, Some(rect), peniko::Color::WHITE)
                .scissor,
            None,
            "a changed base colour repaints everything"
        );
        assert_eq!(
            root.plan(
                &device,
                format,
                64,
                64,
                Some(ScissorRect::from_corners(0, 0, 64, 64)),
                black
            )
            .scissor,
            None,
            "damage covering the target is a full frame"
        );
        assert_eq!(
            root.plan(&device, format, 64, 64, None, black).scissor,
            None
        );
        root.invalidate();
        assert_eq!(
            root.plan(&device, format, 64, 64, Some(rect), black)
                .scissor,
            None,
            "an invalidated accumulator takes a full frame"
        );
        root.commit(black);
        assert_eq!(
            root.plan(&device, format, 32, 64, Some(rect), black)
                .scissor,
            None,
            "a new extent is a new accumulator"
        );
    }

    #[test]
    fn starts_with_no_surface() {
        let renderer = SurfaceRenderer::new();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
    }

    /// One surface episode ending drops everything scoped to it: the
    /// `Invalid`-reconfigure streak and the stashed base colour, which
    /// described a frame of the surface being replaced.
    ///
    /// Asserted through `adopt_surface_state` itself — the single door
    /// `install_surface` and `on_surface_destroyed` both go through — rather
    /// than by re-typing their reset statements here. A previous version of
    /// this test did the latter and would have stayed green if either caller
    /// stopped resetting anything at all.
    #[test]
    fn adopting_a_surface_state_drops_the_previous_frames_stashes() {
        let mut renderer = SurfaceRenderer::new();
        renderer.pending_base_color = Some(peniko::Color::WHITE);
        renderer.consecutive_invalid = 3;

        renderer.adopt_surface_state(SurfaceState::NoSurface);

        assert!(renderer.pending_base_color.is_none());
        assert_eq!(renderer.consecutive_invalid, 0);
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        // Idempotent, as the repeated-`Destroyed` contract needs.
        renderer.adopt_surface_state(SurfaceState::NoSurface);
        assert!(renderer.pending_base_color.is_none());
        assert_eq!(renderer.consecutive_invalid, 0);
    }

    /// The give-up arm of `acquire` leaves through the same door: adopting
    /// [`SurfaceState::Lost`] drops the in-flight stash, so a surface that
    /// went away mid-frame cannot hand its base colour to the next one.
    #[test]
    fn adopting_the_lost_state_drops_the_dying_surfaces_stashes() {
        let mut renderer = SurfaceRenderer::new();
        renderer.pending_base_color = Some(peniko::Color::WHITE);

        renderer.adopt_surface_state(SurfaceState::Lost);

        assert_eq!(renderer.phase(), SurfacePhase::SurfaceLost);
        assert!(
            renderer.pending_base_color.is_none(),
            "a lost surface's base colour must not outlive it"
        );
    }

    #[test]
    fn resolved_translucent_is_false_without_a_live_surface() {
        // The Mode A default: with no surface installed
        // there is nothing proven translucent, so a shell reading this before
        // its first install keeps the opaque paint contract rather than
        // punching holes it can't back.
        let renderer = SurfaceRenderer::new();
        assert!(!renderer.surface_resolved_translucent());
    }

    #[test]
    fn deferred_present_is_send() {
        // The whole point of `DeferredPresent` is crossing a thread boundary
        // (render thread → the thread committing the CATransaction), so pin the
        // auto-trait: losing it would break `frust-shell-ios`'s present-sync
        // handoff at a distance, in a crate that can't see this type's fields.
        fn assert_send<T: Send>() {}
        assert_send::<DeferredPresent>();
    }

    #[test]
    fn submit_deferred_yields_no_frame_without_a_surface() {
        // No GPU needed: with nothing acquired (no surface at all) the deferred
        // submit skips exactly like `submit`, and hands back no present handle —
        // `Some(..)` accompanies only `Rendered`.
        let ctx = RenderContext::new();
        let mut renderer = SurfaceRenderer::new();

        let (outcome, deferred) = renderer
            .submit_deferred(&ctx)
            .expect("submit_deferred in NoSurface must not error");
        assert_eq!(outcome, FrameOutcome::Skipped);
        assert!(deferred.is_none());
    }

    #[test]
    fn render_is_skipped_without_a_surface() {
        // No GPU needed: a `NoSurface` renderer short-circuits before any wgpu
        // work. `RenderContext::new()` only builds a wgpu `Instance` (no device).
        let ctx = RenderContext::new();
        let mut renderer = SurfaceRenderer::new();
        let scene = frust_scene::Scene::new();

        let outcome = renderer
            .render(&ctx, &scene, peniko::Color::WHITE)
            .expect("render in NoSurface must not error");
        assert_eq!(outcome, FrameOutcome::Skipped);
    }

    /// Destroy is idempotent, reaches `NoSurface`, and drops the in-flight
    /// stash: a stale `base_color` describes the dead surface's frame, not
    /// the next one's.
    #[test]
    fn destroy_is_idempotent_and_resets_to_no_surface() {
        let mut renderer = SurfaceRenderer::new();
        renderer.pending_base_color = Some(peniko::Color::WHITE);

        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        assert!(
            renderer.pending_base_color.is_none(),
            "the dying surface's base colour must not reach the next submit"
        );

        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        assert!(renderer.pending_base_color.is_none());
    }

    #[test]
    fn resize_without_surface_is_a_no_op() {
        let ctx = RenderContext::new();
        let mut renderer = SurfaceRenderer::new();
        renderer.on_surface_changed(&ctx, 800, 600);
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
    }

    #[test]
    fn pipeline_cache_data_is_none_without_a_surface() {
        // No GPU needed: with no installed surface there is no live cache to
        // read back, regardless of whether a blob was restored.
        let mut renderer = SurfaceRenderer::new();
        assert_eq!(renderer.pipeline_cache_data(), None);
        renderer.set_initial_pipeline_cache_data(Some(vec![1, 2, 3, 4]));
        assert_eq!(renderer.pipeline_cache_data(), None);
    }

    #[test]
    fn set_initial_pipeline_cache_data_none_clears_the_blob() {
        // Setter is total and side-effect-free without a surface; clearing to
        // `None` (a cold start) is a no-op on the observable `NoSurface` state.
        let mut renderer = SurfaceRenderer::new();
        renderer.set_initial_pipeline_cache_data(Some(vec![9, 9, 9]));
        renderer.set_initial_pipeline_cache_data(None);
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        assert_eq!(renderer.pipeline_cache_data(), None);
    }

    /// The engine arm's real-device acceptance, headless.
    ///
    /// A windowed run is the arm's own acceptance and cannot happen on a box
    /// with no display server, so this drives the exact `EngineTarget` the
    /// engine arm of [`SurfaceRenderer::submit`] builds — the surface's
    /// reported format at `RENDER_ATTACHMENT`, the surface-owned `Depth24Plus`
    /// attachment, premultiplied output, the identity root — against an
    /// offscreen target of the same shape, under a `wgpu` validation error
    /// scope. What it proves is what the windowed run would: this seam's
    /// target construction is accepted by a real device and produces the
    /// frame's pixels. What it cannot prove is the swapchain half (acquire,
    /// present, alpha-mode compositing), which stays owed to a display.
    ///
    /// Run over BOTH surface formats, since which one a swapchain reports
    /// first is the platform's business and the engine warms its pipelines for
    /// exactly the one it is handed.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn engine_arm_records_a_frame_into_its_target_without_validation_errors() {
        let _serialized = RENDER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        const SIZE: u32 = 64;

        let (device, queue, caps) = test_device();

        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            builder.fill_rect(
                kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
                peniko::Brush::Solid(peniko::color::palette::css::RED),
            );
        }

        for format in frust_gpu::SURFACE_FORMATS {
            let target = frust_gpu::HeadlessTarget::new(&device, SIZE, SIZE, format);
            let depth = frust_engine::DepthTexture::new(&device, SIZE, SIZE);
            let mut engine = frust_engine::EngineRenderer::new(&device, &caps, format, None)
                .expect("failed to create the engine renderer");

            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            engine
                .encode(
                    &device,
                    &queue,
                    &mut encoder,
                    &scene,
                    frust_engine::EngineTarget {
                        view: target.view(),
                        format,
                        width: SIZE,
                        height: SIZE,
                        depth: Some(depth.view()),
                        output: frust_engine::OutputAlpha::Premultiplied,
                    },
                    peniko::Color::BLACK,
                    Affine::IDENTITY,
                )
                .expect("the engine refused a plain opaque frame");
            queue.submit([encoder.finish()]);
            engine.end_frame(&queue);

            // Pop the scope by polling the device, the shape every GPU suite
            // in this workspace uses: the pop resolves only once the queue has
            // been pumped.
            let error = {
                use std::task::{Context, Poll, Waker};
                let waker = Waker::noop();
                let mut cx = Context::from_waker(waker);
                let mut pop = std::pin::pin!(scope.pop());
                loop {
                    match pop.as_mut().poll(&mut cx) {
                        Poll::Ready(error) => break error,
                        Poll::Pending => {
                            let _ = device.poll(wgpu::PollType::wait_indefinitely());
                        }
                    }
                }
            };
            assert!(
                error.is_none(),
                "the engine arm's {format:?} target raised a validation error: {error:?}"
            );

            let pixels = target.read_back(&device, &queue);
            let index = (((SIZE / 2) * SIZE + (SIZE / 2)) * 4) as usize;
            let centre: [u8; 4] = pixels[index..index + 4]
                .try_into()
                .expect("a read-back row holds four bytes per pixel");
            // Channel order differs between the two formats, so the assertion
            // that holds for both is "opaque, and not the black it was cleared
            // to" — the red rect reached the target.
            assert_eq!(centre[3], 255, "{format:?}: the frame is not opaque");
            assert!(
                centre[0] != 0 || centre[1] != 0 || centre[2] != 0,
                "{format:?}: the target still holds its clear colour ({centre:?})"
            );
        }
    }

    /// The ledger a shell publishes, without a surface: empty, handed out once
    /// by `take_retained_ledger` and not again until something changes — and a
    /// surface episode starting or ending is such a change, so a fresh or
    /// torn-down surface always publishes its empty ledger.
    #[test]
    fn the_retained_ledger_is_published_once_per_change_without_a_surface() {
        let mut renderer = SurfaceRenderer::new();
        assert_eq!(renderer.retained_ledger(), ResidentPages::default());

        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default()),
            "the first take always publishes"
        );
        assert_eq!(
            renderer.take_retained_ledger(),
            None,
            "nothing changed since"
        );

        renderer.adopt_surface_state(SurfaceState::NoSurface);
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default()),
            "an episode ending publishes the empty ledger again"
        );
        assert!(!renderer.take_retained_refusal());
        // Nothing to clear, and nothing to fail.
        renderer.clear_retained();
        assert_eq!(renderer.take_retained_ledger(), None);

        renderer.on_surface_destroyed();
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default()),
            "a surface destroy publishes the empty ledger, once"
        );
        assert_eq!(renderer.take_retained_ledger(), None);
    }

    /// The serial a shell names is committed only by a frame that rendered: a
    /// frame skipped for want of a surface leaves the ledger's serial where it
    /// was, an episode ending drops the in-flight one, and the empty ledger a
    /// destroy publishes still carries the last committed serial.
    #[test]
    fn only_a_rendered_frame_commits_its_scene_serial() {
        let mut renderer = SurfaceRenderer::new();
        let ctx = RenderContext::new();
        renderer.note_scene_serial(9);
        let outcome = renderer
            .encode(&ctx, &frust_scene::Scene::new(), peniko::Color::BLACK)
            .expect("no surface is not an error");
        assert_eq!(outcome, EncodeOutcome::Skipped);
        assert_eq!(renderer.pending_serial, None, "nothing was encoded");
        assert_eq!(renderer.take_retained_ledger().map(|l| l.frame), Some(0));

        // As if scene 4 had been committed and scene 5 was in flight.
        renderer.committed_serial = 4;
        renderer.pending_serial = Some((5, true));
        renderer.on_surface_destroyed();
        assert_eq!(renderer.pending_serial, None, "the in-flight scene is gone");
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages {
                pages: Vec::new(),
                frame: 4,
            }),
            "the empty ledger names the last committed scene"
        );
    }

    #[test]
    fn a_scene_records_a_retained_page_only_through_a_push_retained_bracket() {
        let rect = kurbo::Rect::new(0.0, 0.0, 4.0, 4.0);
        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            builder.draw_retained(3, rect, 1.0, 1.0);
        }
        assert!(!scene_records_retained(&scene), "a draw records nothing");
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            builder.push_retained(3, rect, 1.0, 1.0);
            builder.pop_retained();
        }
        assert!(scene_records_retained(&scene));
    }

    /// A device-scale change clears the store and owes an EMPTY publish (even
    /// over an already-empty store); the same scale again changes nothing.
    #[test]
    fn a_device_scale_change_clears_and_republishes_the_empty_ledger() {
        let mut renderer = SurfaceRenderer::new();
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default())
        );

        assert!(renderer.note_device_scale(2.0), "the first scale noted");
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default()),
            "a scale change publishes the empty ledger"
        );

        assert!(!renderer.note_device_scale(2.0), "same scale: a no-op");
        assert_eq!(renderer.take_retained_ledger(), None, "and owes nothing");

        assert!(renderer.note_device_scale(3.0));
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default())
        );
        assert_eq!(renderer.take_retained_ledger(), None);

        // A surface episode ending is not a scale change: the scale survives
        // it, so a recreate at the same scale owes nothing beyond the
        // episode's own empty publish.
        renderer.on_surface_destroyed();
        assert_eq!(
            renderer.take_retained_ledger(),
            Some(ResidentPages::default())
        );
        assert!(!renderer.note_device_scale(3.0));
        assert_eq!(renderer.take_retained_ledger(), None);
    }

    /// What `retained_ledger` reports for a live engine, on a real device: the
    /// key a `PushRetained` body was recorded under, at the extent and scale
    /// its rectangle covered.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn the_retained_ledger_reports_what_the_engine_holds() {
        let _serialized = RENDER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        const SIZE: u32 = 64;
        const KEY: u64 = 41;
        let (device, queue, caps) = test_device();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = frust_gpu::HeadlessTarget::new(&device, SIZE, SIZE, format);
        let mut engine = frust_engine::EngineRenderer::new(&device, &caps, format, None)
            .expect("failed to create the engine renderer");

        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            builder.push_transform(Affine::translate((8.0, 8.0)) * Affine::scale(2.0));
            builder.push_retained(KEY, kurbo::Rect::new(0.0, 0.0, 16.0, 12.0), 0.5, 1.0);
            builder.fill_rect(
                kurbo::Rect::new(0.0, 0.0, 16.0, 12.0),
                peniko::Brush::Solid(peniko::color::palette::css::RED),
            );
            builder.pop_retained();
            builder.pop_transform();
        }
        assert!(resident_pages(&engine, 0).pages.is_empty());

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        engine
            .encode(
                &device,
                &queue,
                &mut encoder,
                &scene,
                frust_engine::EngineTarget {
                    view: target.view(),
                    format,
                    width: SIZE,
                    height: SIZE,
                    depth: None,
                    output: frust_engine::OutputAlpha::Premultiplied,
                },
                peniko::Color::BLACK,
                Affine::IDENTITY,
            )
            .expect("the engine refused a retained frame");
        queue.submit([encoder.finish()]);
        engine.end_frame(&queue);

        let resident = resident_pages(&engine, 7);
        assert_eq!(
            resident.pages,
            vec![ResidentPage {
                key: KEY,
                extent_px: (32, 24),
                scale: 2.0,
            }],
            "the body's page is reported at its rasterized extent and scale"
        );
        assert_eq!(resident.frame, 7, "stamped with the committed serial");
    }
}
