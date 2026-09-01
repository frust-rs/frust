//! [`SurfaceRenderer`]: the surface-lifecycle state machine that
//! renders a [`frust_scene::Scene`] into one window's swapchain each frame.
//!
//! Wraps the pure lifecycle logic in [`crate::lifecycle`] around the actual
//! `wgpu`/vello resources: it starts in [`SurfacePhase::NoSurface`], becomes
//! renderable on `on_surface_created`, resizes on `on_surface_changed`, tears
//! down on `on_surface_destroyed`, and drops to [`SurfacePhase::SurfaceLost`]
//! when the swapchain reports `Lost` mid-frame.
//!
//! vello 0.9 renders via compute into an `Rgba8Unorm` storage texture. Two
//! render paths follow from that (chosen per surface, see
//! [`crate::context::choose_render_path`]):
//!
//! - **Direct-to-surface** (surface supports `Rgba8Unorm` + `STORAGE_BINDING`):
//!   acquire → `render_to_texture` straight into the acquired swapchain texture
//!   → present. No intermediate, no blit.
//! - **Blit fallback** (`Bgra8`-only surfaces, or the `cpu-tier` path): encode →
//!   `render_to_texture` (intermediate) → acquire → blit (intermediate → acquired
//!   swapchain view) → present.
//!
//! The `engine-tier` path takes the direct arm's shape with `frust-engine`: encode
//! copies the frame's scene, then acquire → one `EngineRenderer::encode` into
//! the acquired swapchain view and its surface-owned depth attachment
//! ([`crate::context::RenderPath::EngineDirect`]) → submit → present →
//! `end_frame`. No intermediate, no blit, no vello and no shader pre-pass. A
//! swapchain whose compositor genuinely reads it as STRAIGHT alpha takes the
//! same shape with one pass appended and one indirection added: the frame is
//! encoded into a surface-owned intermediate and un-premultiplied into the
//! acquired view from there, in the same encoder
//! ([`crate::context::RenderPath::EngineDirectUnpremultiply`]). iOS's sole
//! translucent mode (`PostMultiplied`) does NOT take that arm: it is backed
//! by Metal, whose compositor reads a `PostMultiplied` swapchain premultiplied
//! regardless of the mode's name (an upstream wgpu-hal truth bug — see
//! [`crate::context::choose_engine_render_path`]), so it stays on the
//! `EngineDirect` arm above with no conversion pass at all.
//!
//! This arm remaps the v3 present spans — see [`SurfaceRenderer::submit`].
//!
//! Every Gpu-tier arm is one vello pass over the whole frame: a
//! `Command::PushSnapshot` bracket lowers inline in [`crate::convert`] (the
//! same lowering the engine tier's own compiler performs), so no arm here
//! carries a cache, a frame plan or a post-vello quad pass.

use core::ffi::c_void;

use anyhow::{Result, anyhow};

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use frust_scene::Command;
use kurbo::Affine;
use peniko::ImageData;

#[cfg(feature = "engine-tier")]
use crate::context::render_scaled;
use crate::context::{
    AaMode, ConfiguredSurface, DetachedSurface, RenderContext, RenderPath, aa_mode,
};
use crate::convert;
use crate::lifecycle::{
    AcquireAction, AcquireOutcome, AcquireStatus, EncodeOutcome, FrameOutcome, SurfaceEvent,
    SurfacePhase, decide_acquire, next_invalid_streak, next_phase,
};
use crate::shader_effects::{ShaderEffects, clamp_size};

/// Caller-requested alpha-compositing behavior for a surface configuration —
/// the `frust-render` public seam a shell picks
/// between an opaque (today's default) and a translucent-preferred surface;
/// the resolved `wgpu::CompositeAlphaMode` itself never crosses this boundary
/// (mirrors [`DetachedSurface`]'s opacity — see `docs/CODE_STANDARDS.md`'s
/// wgpu-leak anti-pattern), so this type stays `kurbo`/`peniko`-free too.
///
/// Resolution (`crate::context::resolve_alpha_mode`) happens inside
/// [`RenderContext::create_render_surface`](crate::context::RenderContext::create_render_surface),
/// validated against the live surface's `SurfaceCapabilities::alpha_modes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceAlphaRequest {
    /// Today's behavior for every existing caller: `wgpu::CompositeAlphaMode::Auto`,
    /// bit-for-bit unchanged.
    Opaque,
    /// Prefer a translucent alpha-compositing mode (Mode B platform views),
    /// trying
    /// `Inherit` (Android's only reported mode) then `PostMultiplied` (iOS's
    /// translucent mode) then `PreMultiplied`, in that order, against the
    /// surface's reported `alpha_modes`. Falls back to `Opaque`'s `Auto` (and
    /// a `log::warn!`) when none of those three is available — translucency
    /// is then unavailable on that surface.
    ///
    /// **This is a request, not an outcome.** Never key a paint contract off
    /// it: read
    /// [`SurfaceRenderer::surface_resolved_translucent`](SurfaceRenderer::surface_resolved_translucent)
    /// after the surface is installed instead — a fallback
    /// must degrade to the opaque Mode A contract, or the hole punch presents
    /// black rectangles on an opaque swapchain.
    TranslucentPreferred,
}

/// The live GPU resources of a [`SurfacePhase::SurfaceReady`] surface.
///
/// The tier backend is created per surface (it is device-bound) and, with the
/// `RenderSurface`, is dropped on every transition out of `SurfaceReady`,
/// upholding the "no `SurfaceTexture`/surface outlives a transition" invariant.
struct ReadySurface {
    surface: ConfiguredSurface,
    /// The tier-specific renderer that produces this frame's pixels. On the blit
    /// arm it fills the intermediate `surface.path` target; on the direct arm the
    /// GPU renderer targets the acquired swapchain texture in `submit`.
    backend: TierBackend,
    /// The device's persisted `wgpu::PipelineCache` (handed to vello's
    /// `RendererOptions` on the `Gpu` path) paired with the adapter fingerprint
    /// its data is framed under, so [`SurfaceRenderer::pipeline_cache_data`] can
    /// hand back a validatable blob. `None` on adapters without
    /// `PIPELINE_CACHE` (Metal/DX12) — see
    /// [`crate::context::RenderContext::create_pipeline_cache`].
    pipeline_cache: Option<(wgpu::PipelineCache, String)>,
    /// Offscreen WGSL fragment-shader effects (shader-showcase feature): the
    /// per-`Command::ShaderQuad` pipelines/targets the Gpu-tier shader pre-pass
    /// renders and registers as vello image overrides. Lives here so it is
    /// dropped with the surface on `on_surface_destroyed` and survives an
    /// `on_surface_changed` resize (its compiled pipelines persist like vello's
    /// own). Constructed with a clone of the surface's `pipeline_cache` so shader
    /// pipelines seed from the same persisted blob. Unused on the Cpu tier (that
    /// path runs no shader pre-pass — `ShaderQuad`s take the miss placeholder).
    shader_effects: ShaderEffects,
}

/// The per-surface renderer for the selected [`crate::RenderTier`].
///
/// On the blit arm both variants produce the same thing — pixels in the
/// intermediate `Rgba8Unorm` target — which the shared tail then blits to the
/// acquired swapchain texture. On the direct arm (GPU tier only) the `Gpu`
/// variant instead renders straight into the acquired swapchain texture in
/// [`SurfaceRenderer::submit`]. Without the `cpu-tier` feature this is
/// effectively a one-variant enum.
// The GPU variant holds a `vello::Renderer` inline (~1.2 KiB) while the CPU
// variant is boxed; the whole `ReadySurface` already lives behind a `Box`
// (`SurfaceState::Ready`), so the size asymmetry costs nothing on the hot path
// and boxing the GPU renderer would only add an indirection to every frame.
#[cfg_attr(
    any(feature = "cpu-tier", feature = "engine-tier"),
    allow(clippy::large_enum_variant)
)]
enum TierBackend {
    /// vello 0.9 GPU compute path: `render_to_texture` into the target view.
    /// Reused across frames; its compiled shader pipelines survive resizes
    /// (which recreate only the swapchain/target).
    Gpu { renderer: vello::Renderer },
    /// Experimental vello_cpu path (`cpu-tier` feature): rasterize headless
    /// into a pixmap, then upload it into the target texture. Boxed because it
    /// carries a reusable `RenderContext`/`Pixmap` that dwarfs the GPU variant.
    #[cfg(feature = "cpu-tier")]
    Cpu(Box<crate::cpu_tier::CpuTierRenderer>),
    /// The frust-owned engine path (`engine-tier` feature): a
    /// `frust_scene::Scene` compiled into sparse strips and recorded into the
    /// frame's own `wgpu::CommandEncoder` by
    /// [`frust_engine::EngineRenderer`], which never submits — the encoder is
    /// created and submitted in [`SurfaceRenderer::submit`].
    ///
    /// Boxed for the CPU variant's reason.
    #[cfg(feature = "engine-tier")]
    Engine {
        engine: Box<frust_engine::EngineRenderer>,
        /// How many of this surface's frames the engine has refused so far
        /// (a scheduler escalation, an unserveable frame, a capacity ceiling).
        ///
        /// Counted rather than logged per frame: a refusal that reproduces
        /// every frame would otherwise flood the log with one identical line
        /// per vsync. The count feeds [`crate::context::decide_log_action`] —
        /// the same latch the uncaptured-`wgpu`-error handler uses — so the
        /// first few refusals are reported in full, the latch is announced
        /// once, and the running total keeps surfacing on the periodic debug
        /// bump afterwards.
        refused_frames: u32,
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
/// Opaque by design: it wraps a `wgpu::SurfaceTexture` behind a private field
/// with no accessor, exactly like [`DetachedSurface`](crate::DetachedSurface),
/// so no `wgpu` type is nameable outside this crate
/// (`docs/CODE_STANDARDS.md`'s wgpu-leak anti-pattern). It is `Send` — a
/// `wgpu::SurfaceTexture` owns its swapchain frame and borrows nothing — which
/// is the whole point: a shell can hand it from the render thread to the thread
/// that must issue the present.
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
        self.texture.present();
    }
}

/// Per-surface renderer and lifecycle state machine.
///
/// Holds a device-independent, reusable `vello::Scene` (so it survives surface
/// transitions) plus the current [`SurfaceState`]. Constructed empty with
/// [`SurfaceRenderer::new`]; the shell brings it online with
/// [`on_surface_created`](Self::on_surface_created).
pub struct SurfaceRenderer {
    /// Reused across frames; `reset()` each frame rather than reallocated.
    /// Device-independent, so it outlives surface transitions.
    scene: vello::Scene,
    state: SurfaceState,
    /// Consecutive `AcquireStatus::Invalid` acquires retried via `Reconfigure`
    /// since the last successful acquire or surface (re)install — see
    /// [`crate::lifecycle::decide_acquire`]/[`crate::lifecycle::MAX_INVALID_RECONFIGURES`].
    /// Reset on a successful acquire, on giving up (transitioning to
    /// `SurfaceLost`), and on installing a fresh surface.
    consecutive_invalid: u8,
    /// Persisted, framed `wgpu::PipelineCache` blob (see
    /// [`crate::pipeline_cache`]) a prior run wrote and the shell restored from
    /// disk via [`set_initial_pipeline_cache_data`](Self::set_initial_pipeline_cache_data),
    /// consumed at the next surface install to seed vello's shader-pipeline
    /// compilation. `None` is a cold start. Only meaningful on adapters with
    /// `PIPELINE_CACHE` (any Vulkan adapter); validated and discarded elsewhere.
    initial_cache_data: Option<Vec<u8>>,
    /// The swapchain texture [`Self::acquire`] acquired and stashed for
    /// [`Self::submit`] to blit into and present (the acquire/submit span split).
    /// `None` outside an in-flight `acquire`→`submit` pair — set only on an
    /// [`AcquireOutcome::Acquired`] result, taken by the next [`Self::submit`].
    /// A `wgpu::SurfaceTexture` is owned (it does not borrow the surface), so
    /// stashing it here between the two clock-separated calls keeps the wgpu type
    /// confined to `frust-render` while letting a shell time each span with its
    /// own clock (timing stays shell-owned — see `frust-shell-common::perf`).
    pending_present: Option<wgpu::SurfaceTexture>,
    /// The `base_color` [`Self::encode`] was called with, stashed for the
    /// **direct** render path only: there, the vello `render_to_texture` that
    /// consumes `base_color` runs in [`Self::submit`] (it targets the acquired
    /// swapchain texture, which does not exist until [`Self::acquire`]), so the
    /// color must survive the gap between `encode` and `submit`. Unused on the
    /// blit path (which renders inside `encode`, where `base_color` is a
    /// parameter). `None` outside an in-flight direct-path frame.
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
    #[cfg(feature = "engine-tier")]
    engine_scene: frust_scene::Scene,
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
            scene: vello::Scene::new(),
            state: SurfaceState::NoSurface,
            consecutive_invalid: 0,
            initial_cache_data: None,
            pending_present: None,
            pending_base_color: None,
            #[cfg(feature = "engine-tier")]
            engine_scene: frust_scene::Scene::default(),
        }
    }

    /// Drops the in-flight frame's cross-call stash: the direct arm's
    /// `base_color`.
    ///
    /// It describes one frame of one surface, so any event that ends a
    /// surface's life (`on_surface_destroyed`) or replaces it
    /// (`install_surface`) must drop it rather than let the next `submit`
    /// consume a stale colour. Idempotent.
    fn clear_pending_frame(&mut self) {
        self.pending_base_color = None;
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
    fn adopt_surface_state(&mut self, state: SurfaceState) {
        self.state = state;
        self.consecutive_invalid = 0;
        self.clear_pending_frame();
    }

    /// Restores the persisted pipeline-cache blob a prior run produced via
    /// [`pipeline_cache_data`](Self::pipeline_cache_data), to seed vello's
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
    pub fn set_initial_pipeline_cache_data(&mut self, data: Option<Vec<u8>>) {
        self.initial_cache_data = data;
    }

    /// The current pipeline-cache data to persist, framed with the adapter
    /// fingerprint so a later launch can validate it before reuse (see
    /// [`crate::pipeline_cache`]).
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
        Some(crate::pipeline_cache::frame(key, &data))
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
            SurfaceState::Ready(ready) => ready.surface.resolved_translucent,
            SurfaceState::NoSurface | SurfaceState::Lost => false,
        }
    }

    /// The most recent frame's real GPU time per pass, or `None` when this
    /// surface produces no such measurement.
    ///
    /// `None` — the `gpu_q=0` case a shell reports — for every reason there is:
    /// a tier other than the engine one, a build without the `engine-tier`
    /// feature, a device created without `wgpu::Features::TIMESTAMP_QUERY`
    /// (which a build that did not compile `perf-trace` in never asks for), and
    /// the first few frames of a surface, before the ring's first readback has
    /// landed.
    ///
    /// The reading lags the calling frame: a frame's queries are mapped
    /// without ever blocking the frame path, so what comes back is a recent
    /// frame's GPU cost rather than the one being recorded. In steady state one
    /// fresh reading lands per frame, so the series is complete and offset,
    /// not sparse — see [`frust_gpu::diag::TimestampRing`].
    pub fn gpu_pass_timings(&self) -> Option<GpuPassTimings> {
        #[cfg(feature = "engine-tier")]
        {
            let SurfaceState::Ready(ready) = &self.state else {
                return None;
            };
            let TierBackend::Engine { timestamps, .. } = &ready.backend else {
                return None;
            };
            let reading = timestamps.latest()?;
            let span = |which: frust_engine::EngineSpan| reading.span(which.index());
            Some(GpuPassTimings {
                prepass: span(frust_engine::EngineSpan::Prepass),
                main: span(frust_engine::EngineSpan::Main),
                composite: span(frust_engine::EngineSpan::Composite),
                blit: span(frust_engine::EngineSpan::Blit),
            })
        }
        #[cfg(not(feature = "engine-tier"))]
        {
            None
        }
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
    /// swapchain and a device-bound `vello::Renderer`, transitioning to
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
        let surface = ctx
            .create_surface(
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
        let surface = ctx
            .create_render_surface(
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
    /// [`crate::lifecycle::create_android_surface`].
    ///
    /// # Safety
    ///
    /// `window_ptr` must be a valid, acquired `ANativeWindow*` that outlives the
    /// surface (and all its `SurfaceTexture`s). See
    /// [`crate::lifecycle::create_android_surface`] for the full contract.
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
        let raw = unsafe { crate::lifecycle::create_android_surface(&ctx.instance, window_ptr) }?;
        let surface = ctx
            .create_render_surface(
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
    /// Only compiled on Apple targets (mirrors [`crate::lifecycle::create_metal_surface`]'s
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
    /// [`crate::lifecycle::create_metal_surface`] for the full contract.
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
        let raw = unsafe { crate::lifecycle::create_metal_surface(&ctx.instance, layer_ptr) }?;
        let surface = ctx
            .create_render_surface(
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
    fn install_surface(&mut self, ctx: &RenderContext, surface: ConfiguredSurface) -> Result<()> {
        // Seed vello's shader-pipeline compilation from a persisted
        // `wgpu::PipelineCache` when the adapter supports it (Vulkan/Android) and
        // the shell restored a validated blob via
        // `set_initial_pipeline_cache_data`. Returns `None` on adapters without
        // `PIPELINE_CACHE` (Metal/DX12) — the path is then byte-identical to
        // before this existed. Created before the tier `match` so the `Gpu` arm
        // can clone it into `RendererOptions`; the `Cpu` tier runs no GPU
        // pipelines and leaves it unused (retained only so a later
        // `pipeline_cache_data()` still has the handle).
        let pipeline_cache = ctx.create_pipeline_cache(self.initial_cache_data.as_deref());

        // Pick the tier backend the context's probe selected.
        // Only `Gpu` is reachable without the `cpu-tier` feature (the probe
        // never returns `Cpu` there, and `ensure_device` guards it), so the
        // default build creates a `vello::Renderer` exactly as before.
        let backend = match ctx.selected_tier() {
            crate::RenderTier::Gpu => {
                let device = &ctx.device_handle().device;
                // Narrow the compiled AA pipeline set to the one mode the
                // `FRUST_AA_MODE` knob selected (default: `Area`, the sole
                // `AaConfig` every production `RenderParams` site requests —
                // see the `antialiasing_method: aa_mode().to_vello()` sites in
                // `encode`/`submit_impl`):
                // `RendererOptions::default()` compiles shader permutations
                // for every `AaConfig` (`AaSupport::all()`), ~3x unnecessary
                // pipeline compiles at init that contribute to the slow,
                // synchronous, main-thread launch-time shader compile that can
                // trip the iOS launch watchdog (`docs/DEVELOPMENT.md`'s
                // dev-profile shader-stack override note).
                // Verified against the vello 0.9.0 source
                // (`RendererOptions::antialiasing_support: AaSupport`,
                // `AaSupport::area_only()` — both public, non-`non_exhaustive`).
                let mode = aa_mode();
                // One line per process naming the effective mode, so a device
                // capture proves which build ran (`docs/RENDER_DEVELOPMENT.md`
                // § Instrumentation (render path)). Logged once regardless of surface
                // recreation, mirroring `log_render_path`'s once-per-process
                // shape.
                // A non-default mode additionally warns: it is a measurement
                // instrument that changes what every pass renders, and a
                // capture read later should say so at a level that stands out
                // from an ordinary startup line. A default build (`Area`)
                // logs only the `info` line above, exactly as before.
                static AA_MODE_LOGGED: OnceLock<()> = OnceLock::new();
                AA_MODE_LOGGED.get_or_init(|| {
                    log::info!("frust-render aa-mode={mode}");
                    if mode != AaMode::Area {
                        log::warn!("frust-render measurement knob in effect: aa-mode={mode}");
                    }
                });
                let renderer_options = vello::RendererOptions {
                    antialiasing_support: mode.support(),
                    pipeline_cache: pipeline_cache.clone(),
                    ..Default::default()
                };
                let renderer = vello::Renderer::new(device, renderer_options)
                    .map_err(|e| anyhow!("frust-render: failed to create vello renderer: {e}"))?;
                TierBackend::Gpu { renderer }
            }
            #[cfg(feature = "cpu-tier")]
            crate::RenderTier::Cpu => TierBackend::Cpu(Box::new(
                crate::cpu_tier::CpuTierRenderer::new(surface.config.width, surface.config.height),
            )),
            #[cfg(not(feature = "cpu-tier"))]
            crate::RenderTier::Cpu => {
                return Err(anyhow!(
                    "frust-render: Cpu tier selected without the `cpu-tier` feature compiled in"
                ));
            }
            #[cfg(feature = "engine-tier")]
            crate::RenderTier::Engine => {
                let device = &ctx.device_handle().device;
                // Built for the format this surface's FRAMES will target,
                // because the engine warms its strip pipelines for exactly one
                // of them: the swapchain's own on the direct arm, the engine's
                // off-screen format on the un-premultiplying one, whose frames
                // land in a surface-owned intermediate instead
                // ([`engine_target_format`]).
                //
                // A resize keeps that format (`RenderContext::resize_surface`
                // rewrites only the dimensions), so `on_surface_changed`
                // resizes the renderer in place; a FORMAT change arrives as a
                // fresh surface and lands back here, building a renderer
                // warmed for the new format — off the frame path, and seeded
                // from the same persisted `wgpu::PipelineCache` blob handed to
                // vello, so a re-warm after a format change is a driver-cache
                // hit rather than a cold compile.
                let caps = frust_gpu::TierCaps::probe(&ctx.device_handle().adapter);
                let engine_format = engine_target_format(&surface.path, surface.config.format);
                let engine = frust_engine::EngineRenderer::new(
                    device,
                    &caps,
                    engine_format,
                    pipeline_cache.as_ref(),
                )
                .map_err(|e| anyhow!("frust-render: failed to create engine renderer: {e}"))?;
                // One line per process naming the tier, next to the
                // `render-path`/`aa-mode` lines, so a capture proves which
                // renderer produced the frames it is timing.
                static ENGINE_LOGGED: OnceLock<()> = OnceLock::new();
                ENGINE_LOGGED.get_or_init(|| {
                    log::info!(
                        "frust-render tier=engine (frust-engine strip pipeline, format={:?} into \
                         a {:?} swapchain, {}x{}, adapter `{}`)",
                        engine_format,
                        surface.config.format,
                        surface.config.width,
                        surface.config.height,
                        caps.adapter_name
                    );
                    // Both knobs instrument vello's own passes — the AA mode
                    // is a `vello::AaConfig` and the scale sizes the blit
                    // arm's intermediate — and this tier runs neither. Said
                    // once, so a matrix run that sets them does not read the
                    // resulting numbers as an answer about them.
                    if aa_mode() != AaMode::Area || render_scaled() {
                        log::warn!(
                            "frust-render: FRUST_AA_MODE/FRUST_RENDER_SCALE are vello-only \
                             instruments; the engine tier ignores both and renders at full \
                             surface resolution"
                        );
                    }
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
                    timestamps,
                }
            }
            #[cfg(not(feature = "engine-tier"))]
            crate::RenderTier::Engine => {
                return Err(anyhow!(
                    "frust-render: Engine tier selected without the `engine-tier` feature \
                     compiled in"
                ));
            }
        };
        debug_assert_eq!(
            next_phase(self.phase(), SurfaceEvent::Created),
            SurfacePhase::SurfaceReady,
            "Created must reach SurfaceReady"
        );
        // Seed the shader-showcase effects engine with a clone of the same
        // pipeline cache handed to vello, so its per-program shader pipelines
        // compile from the same persisted blob (a no-op `None` on adapters
        // without `PIPELINE_CACHE`). Cloned before `pipeline_cache` is consumed
        // into the fingerprint pair below.
        let shader_effects = ShaderEffects::new(pipeline_cache.clone());
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
            shader_effects,
        })));
        Ok(())
    }

    /// Resizes the swapchain (`surfaceChanged`/`Resized`).
    ///
    /// Only acts in [`SurfacePhase::SurfaceReady`]; a resize with no surface is
    /// dropped. The `vello::Renderer` (and its compiled pipelines) is preserved
    /// — only the surface config and target texture are recreated. Zero
    /// dimensions are ignored (a minimized window keeps its last valid size).
    pub fn on_surface_changed(&mut self, ctx: &RenderContext, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let SurfaceState::Ready(ready) = &mut self.state {
            ctx.resize_surface(&mut ready.surface, width, height);
            match &mut ready.backend {
                // Nothing sized to re-establish: the vello renderer's own
                // resources are size-independent, and the surface config and
                // target texture were recreated above.
                TierBackend::Gpu { .. } => {}
                #[cfg(feature = "cpu-tier")]
                TierBackend::Cpu(cpu) => {
                    // Keep the CPU pixmap's size in step with the swapchain;
                    // the GPU renderer needs no resize (only the target
                    // texture, done above), but the CPU tier's
                    // `RenderContext`/`Pixmap` are sized.
                    cpu.resize(width, height);
                }
                #[cfg(feature = "engine-tier")]
                TierBackend::Engine { engine, .. } => {
                    // The engine's own extent-sized resources (its intermediate
                    // pool's parked entries, and the depth attachment it would
                    // own if the surface did not) are re-established here,
                    // off the frame path. The surface's own depth attachment
                    // was recreated by `resize_surface` above, in the same
                    // step that reconfigured the swapchain.
                    //
                    // The pipelines survive: only a FORMAT change would need a
                    // renderer warmed for a different target, and a resize
                    // never changes one — `resize_surface` rewrites the
                    // dimensions alone. Asserted rather than assumed, since a
                    // silent divergence would compile a fresh pipeline on the
                    // frame path for every frame that followed. Compared
                    // against the format the ARM implies, not the swapchain's
                    // own, so the un-premultiplying arm (whose frames target
                    // the engine's off-screen format) is not reported as drift
                    // on every resize.
                    let expected =
                        engine_target_format(&ready.surface.path, ready.surface.config.format);
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
                // Nothing to release ahead of time: the vello renderer and
                // every texture it holds die with the state below.
                TierBackend::Gpu { .. } => {}
                #[cfg(feature = "cpu-tier")]
                TierBackend::Cpu(_) => {}
                // Nothing to release ahead of time: every texture this
                // backend holds (resource textures, the intermediate pool, its
                // own depth attachment) dies with the renderer below, and the
                // surface's own depth attachment — plus, on the
                // un-premultiplying arm, its intermediate — dies with the
                // surface beside it.
                #[cfg(feature = "engine-tier")]
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
    /// The internal `vello::Scene` is `reset()` and re-encoded every frame;
    /// nothing accumulates across calls.
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

    /// Phase 1 of the frame — the **encode** span: reset the internal
    /// `vello::Scene` and encode `scene` into it. It does **not** touch the
    /// swapchain, so a caller timing this call in isolation measures encode cost
    /// with no vsync wait folded in. What else happens here depends on the render
    /// path (see [`Self::submit`]'s span mapping):
    ///
    /// - **Blit arm**: also renders (clearing to `base_color`) into the
    ///   intermediate `Rgba8Unorm` target — so `encode_us` includes the GPU
    ///   render, as it always has on this path — before the blit.
    /// - **Direct arm**: does ONLY the CPU-side scene build and stashes
    ///   `base_color`; the GPU render moves to [`Self::submit`] (it needs the
    ///   acquired swapchain texture). `encode_us` is then just the CPU encode.
    ///
    /// Returns [`EncodeOutcome::Skipped`] (no work done, nothing queued) in any
    /// phase but [`SurfacePhase::SurfaceReady`]; otherwise
    /// [`EncodeOutcome::Encoded`], after which [`Self::present`] finishes the
    /// frame. The internal scene is `reset()` every call; nothing accumulates
    /// across frames.
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
        // Disjoint field borrows: the reusable scene, the live surface, and the
        // direct-path color stash; `consecutive_invalid` belongs to `present`.
        let Self {
            scene: vello_scene,
            state,
            pending_base_color,
            #[cfg(feature = "engine-tier")]
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

        let device_handle = ctx.device_handle();

        // Encode this frame's pixels, per tier and per render path.
        match &mut ready.backend {
            TierBackend::Gpu { renderer } => {
                // Shader pre-pass (shader-showcase): compile/render each
                // `Command::ShaderQuad` program into an offscreen texture and
                // register it as a vello image override, producing the
                // program-id → `ImageData` map `encode_into` lowers each quad
                // against. Runs (and submits its own encoder) BEFORE vello's
                // `render_to_texture` so the atlas copy reads a complete texture
                // — wgpu serializes queue submissions in order.
                //
                // `adapter_max` is captured once and threaded into both the
                // pre-pass (which keys the map by clamped physical size) and the
                // encode lowering (which recomputes the identical key per quad),
                // so the two derive the same `(id, w, h)` for every quad.
                //
                // `FRUST_NO_SHADER_EFFECTS` (`context::shader_effects_disabled`,
                // `docs/RENDER_DEVELOPMENT.md` § Instrumentation (render path))
                // is the kill switch for this still-unproven-on-device pre-pass:
                // resolved once here and threaded into `run_shader_prepass`, which
                // short-circuits to a zero-GPU-work no-op (skipping compile,
                // targets, and the mark_seen/reap age tracking too) before
                // touching `device`/`queue`/`renderer`/`shader_effects` at all —
                // every `Command::ShaderQuad` this frame then falls through to
                // `convert::encode_into_with_shaders`'s existing miss
                // placeholder, exactly like a CPU-tier or no-prepass caller.
                let adapter_max = device_handle.device.limits().max_texture_dimension_2d;
                let shader_images = run_shader_prepass(
                    &device_handle.device,
                    &device_handle.queue,
                    renderer,
                    &mut ready.shader_effects,
                    scene,
                    adapter_max,
                    crate::context::shader_effects_disabled(),
                );
                let width = ready.surface.config.width;
                let height = ready.surface.config.height;
                // The size of the texture THIS frame's vello passes render
                // into, and the root every one of them encodes under. They
                // are one decision, and the blit arm made it when it sized its
                // intermediate (`context::scaled_size`/`context::blit_root`):
                // the passes targeting a shrunken intermediate must both be
                // sized for it and be scaled into it — a `RenderParams` at the
                // surface size would render into a target that cannot hold it,
                // and an unscaled root would fill it with the frame's top-left
                // corner. Both values are READ from the arm rather than
                // re-derived here, so the frame is scaled by exactly the ratio
                // its target has. The direct arms are never scaled (scale < 1
                // forces the blit arm, `context::create_render_surface`), so
                // they keep the surface size and the identity root exactly as
                // before.
                let (pass_width, pass_height, root) = match &ready.surface.path {
                    RenderPath::Blit {
                        target_size, root, ..
                    } => (target_size.0, target_size.1, *root),
                    RenderPath::Direct | RenderPath::DirectPremultiplied { .. } => {
                        (width, height, Affine::IDENTITY)
                    }
                    // Unreachable inside the Gpu arm: the engine paths belong
                    // to the engine tier alone. Answered with the direct arms'
                    // values rather than a catch-all, so a path added later
                    // cannot fall through this match silently.
                    #[cfg(feature = "engine-tier")]
                    RenderPath::EngineDirect { .. }
                    | RenderPath::EngineDirectUnpremultiply { .. } => {
                        (width, height, Affine::IDENTITY)
                    }
                };
                let params =
                    |base_color, (target_width, target_height): (u32, u32)| vello::RenderParams {
                        base_color,
                        width: target_width,
                        height: target_height,
                        antialiasing_method: aa_mode().to_vello(),
                    };
                // The frame's one vello pass: the whole command list, under
                // the arm's own root, with each `Command::ShaderQuad` resolved
                // against this frame's override map and each
                // `Command::PushSnapshot` bracket lowered inline.
                vello_scene.reset();
                convert::encode_into_with_shaders(
                    scene,
                    root,
                    vello_scene,
                    &shader_images,
                    adapter_max,
                );

                match &ready.surface.path {
                    // Direct-to-surface: the vello render targets the
                    // acquired swapchain texture, which does not exist until
                    // `acquire`. So `encode` does ONLY the CPU-side scene build
                    // here; the GPU `render_to_texture` moves to `submit`.
                    // See `submit`'s span-mapping comment for how this remaps
                    // the v3 spans.
                    RenderPath::Direct => {
                        // Carry `base_color` to `submit`, where the render
                        // that consumes it runs.
                        *pending_base_color = Some(base_color);
                    }
                    // Direct-premultiplied (translucent, premultiplied-expecting):
                    // unlike the plain direct arm, the intermediate
                    // already exists at encode time, so vello renders into it now
                    // (like the blit arm); `submit`'s premultiply compute pass then
                    // writes `(rgb*a, a)` into the acquired swapchain texture.
                    RenderPath::DirectPremultiplied {
                        intermediate_view, ..
                    } => {
                        renderer
                            .render_to_texture(
                                &device_handle.device,
                                &device_handle.queue,
                                vello_scene,
                                intermediate_view,
                                // Never scaled: this arm exists only at
                                // scale 1.0 (see the `pass_*` binding).
                                &params(base_color, (width, height)),
                            )
                            .map_err(|e| {
                                anyhow!("frust-render: vello render_to_texture failed: {e}")
                            })?;
                    }
                    // Blit fallback: render into the intermediate `Rgba8Unorm` target
                    // now, before the acquire/blit/present tail (see `submit`) copies
                    // it to the swapchain. Nothing crosses the encode/submit gap on
                    // this arm.
                    RenderPath::Blit { target_view, .. } => {
                        renderer
                            .render_to_texture(
                                &device_handle.device,
                                &device_handle.queue,
                                vello_scene,
                                target_view,
                                // The intermediate's own size, which is
                                // the surface's unless the scale knob
                                // shrank it — the pixels this pass sweeps
                                // are exactly what the knob measures.
                                &params(base_color, (pass_width, pass_height)),
                            )
                            .map_err(|e| {
                                anyhow!("frust-render: vello render_to_texture failed: {e}")
                            })?;
                    }
                    // Unreachable inside the Gpu arm, for the `pass_*`
                    // binding's reason: the engine paths belong to the engine
                    // tier alone. Reported rather than rendered, since a vello
                    // frame reaching one would be a wiring bug with no correct
                    // arm to take.
                    #[cfg(feature = "engine-tier")]
                    RenderPath::EngineDirect { .. }
                    | RenderPath::EngineDirectUnpremultiply { .. } => {
                        return Err(anyhow!(
                            "frust-render: the engine render path requires the engine tier"
                        ));
                    }
                }
            }
            #[cfg(feature = "cpu-tier")]
            TierBackend::Cpu(cpu) => {
                // The CPU tier uploads its pixmap into the intermediate target, so
                // it is always configured on the blit arm (see
                // `context::choose_render_path`'s `force_blit`); a `Direct` path
                // here is a wiring bug.
                let RenderPath::Blit { target_texture, .. } = &ready.surface.path else {
                    return Err(anyhow!(
                        "frust-render: cpu-tier requires the blit render path"
                    ));
                };
                let width = ready.surface.config.width;
                let height = ready.surface.config.height;
                // Rasterize headless into the reusable pixmap (premultiplied
                // RGBA8), then upload it into the intermediate target the blit
                // reads from. `write_texture` needs no row padding (unlike a
                // buffer copy), so the tight `4 * width` stride is fine.
                //
                // The SURFACE's size, not a scaled one: this tier's
                // intermediate is never scaled
                // (`RenderContext::effective_render_scale`), because its
                // rasterizer encodes at identity into a pixmap of exactly
                // these dimensions.
                let pixels = cpu.render(scene, base_color, width, height);
                device_handle.queue.write_texture(
                    target_texture.as_image_copy(),
                    pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * width),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            #[cfg(feature = "engine-tier")]
            TierBackend::Engine { .. } => {
                // This tier renders in `submit` — the direct arm's split, which
                // is why its surface is configured on one of the two engine
                // paths (`context::choose_engine_render_path`): straight into
                // the acquired swapchain view, or into a surface-owned
                // intermediate the same `submit` un-premultiplies from. Any
                // other path here is a wiring bug, reported rather than
                // rendered.
                if !matches!(
                    &ready.surface.path,
                    RenderPath::EngineDirect { .. } | RenderPath::EngineDirectUnpremultiply { .. }
                ) {
                    return Err(anyhow!(
                        "frust-render: engine-tier requires the engine render path"
                    ));
                }
                // The frame's whole CPU-side encode on this arm: copy the
                // display list somewhere that outlives the borrow, since
                // `EngineRenderer::encode` compiles it itself in `submit`.
                // `clone_from` reuses the buffer's capacity, so a steady-state
                // frame allocates nothing — see the field's measurement note.
                engine_scene.clone_from(scene);
                // Carried to `submit` for the direct arm's reason: the render
                // that consumes it runs there. The shader pre-pass is GPU-tier
                // machinery this tier bypasses entirely (a
                // `Command::ShaderQuad` keeps the placeholder lowering
                // `convert` gives it).
                *pending_base_color = Some(base_color);
            }
        }

        Ok(EncodeOutcome::Encoded)
    }

    /// Phase 2 of the frame — the **present** span: a thin wrapper over the
    /// two-phase [`Self::acquire`] + [`Self::submit`] seam, kept for callers
    /// (and the [`Self::render`] convenience wrapper) that time present as one
    /// span. It acquires the swapchain texture (the blocking vsync/present
    /// wait) and, on success, blits/submits/presents it.
    ///
    /// A caller wanting the finer **acquire** (blocking vsync wait) vs
    /// **submit** (blit + queue-submit + present) attribution — to separate
    /// GPU saturation from blit cost — calls
    /// [`Self::acquire`] and [`Self::submit`] directly, timing each with its own
    /// clock (timing stays shell-owned; this crate reads no clock — see
    /// `frust-shell-common::perf`'s layering note). `present`'s combined span
    /// equals `acquire` + `submit` by construction.
    ///
    /// Assumes [`Self::encode`] has already filled the intermediate target this
    /// frame. In any phase but [`SurfacePhase::SurfaceReady`] the call is a
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
        let acquired = ready.surface.surface.get_current_texture();
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
                ctx.configure_surface(&ready.surface);
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
    /// What the submit span contains depends on the render path (the v3 span
    /// mapping):
    ///
    /// - **Blit arm** (`Bgra8`-only/probe-refused/`cpu-tier`/`FRUST_RENDER_SCALE`):
    ///   the intermediate target was already filled in [`Self::encode`], so
    ///   `submit` = create the swapchain view + `TextureBlitter::copy` +
    ///   queue-submit + present. This is the pre-direct-to-surface behavior,
    ///   unchanged. At a render scale below 1 the mapping is unchanged too —
    ///   `encode_us` still carries the GPU render, now over the reduced
    ///   intermediate, and the blit in this span additionally upscales it —
    ///   which is exactly what makes `encode_us` the field the fine-stage
    ///   cost-per-pixel measurement reads.
    /// - **Direct arm** (`Rgba8Unorm` + `STORAGE_BINDING`): the vello
    ///   `render_to_texture` runs HERE, targeting the acquired swapchain texture
    ///   directly (it does not exist until [`Self::acquire`]), then present — no
    ///   blit. So the GPU render cost that the blit arm records in `encode_us`
    ///   moves into `submit_us`; `encode_us` is then only the CPU scene build.
    ///
    /// **v3 wire mapping (unchanged fields, remapped work).** The
    /// `acquire_us`/`submit_us` field names and the wire format are unchanged
    /// (`perf.rs` is untouched). In the direct arm the swapchain **acquire** (the
    /// blocking vsync wait) still happens in [`Self::acquire`] and is recorded in
    /// `acquire_us` exactly as before — so acquire now precedes the GPU render
    /// (which moved to this span) instead of following it as in the blit arm. A
    /// benchmark comparing direct vs blit must account for this: the GPU render
    /// migrates `encode_us` → `submit_us`, `submit_us` sheds the blit, and
    /// `acquire_us` is unchanged but now sits *before* the render.
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
        let (outcome, presentable) = self.submit_impl(ctx)?;
        if let Some(surface_texture) = presentable {
            surface_texture.present();
        }
        Ok(outcome)
    }

    /// [`Self::submit`] with the final present step handed back to the caller
    /// instead of issued here: identical GPU work (render/blit + queue-submit),
    /// but the acquired swapchain frame is returned as an opaque, `Send`
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
            presentable.map(|texture| DeferredPresent { texture }),
        ))
    }

    /// Shared body of [`Self::submit`]/[`Self::submit_deferred`]: everything up
    /// to (but not including) `SurfaceTexture::present`, handing the acquired
    /// frame back so each wrapper decides where the present happens.
    fn submit_impl(
        &mut self,
        ctx: &RenderContext,
    ) -> Result<(FrameOutcome, Option<wgpu::SurfaceTexture>)> {
        // Nothing acquired this frame (no prior `Acquired`): nothing to present.
        let Some(surface_texture) = self.pending_present.take() else {
            return Ok((FrameOutcome::Skipped, None));
        };
        // The stashed texture is owned, but the encoded pixels (direct: the vello
        // renderer + scene; blit: the intermediate target + blitter) live on
        // `self` — if the surface vanished between `acquire` and `submit`, drop
        // the texture and skip rather than present a stale frame.
        let Self {
            scene: vello_scene,
            state,
            pending_base_color,
            #[cfg(feature = "engine-tier")]
            engine_scene,
            ..
        } = self;
        let SurfaceState::Ready(ready) = state else {
            return Ok((FrameOutcome::Skipped, None));
        };
        let ReadySurface {
            surface, backend, ..
        } = &mut **ready;
        let device_handle = ctx.device_handle();
        let swapchain_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        match &surface.path {
            // Direct-to-surface: render vello straight into the acquired swapchain
            // texture, then present. No intermediate, no blit pass.
            RenderPath::Direct => match backend {
                TierBackend::Gpu { renderer } => {
                    let params = vello::RenderParams {
                        // Set in `encode`; a well-formed frame always encoded first.
                        base_color: pending_base_color.take().unwrap_or(peniko::Color::BLACK),
                        width: surface.config.width,
                        height: surface.config.height,
                        antialiasing_method: aa_mode().to_vello(),
                    };
                    renderer
                        .render_to_texture(
                            &device_handle.device,
                            &device_handle.queue,
                            vello_scene,
                            &swapchain_view,
                            &params,
                        )
                        .map_err(|e| {
                            anyhow!("frust-render: vello render_to_texture failed: {e}")
                        })?;
                }
                // Unreachable: the direct arm is only ever configured for the GPU
                // tier (`choose_render_path` forces blit for cpu-tier).
                #[cfg(feature = "cpu-tier")]
                TierBackend::Cpu(_) => {
                    return Err(anyhow!(
                        "frust-render: direct render path requires the GPU tier"
                    ));
                }
                // Unreachable: the engine tier configures its own path
                // (`context::choose_engine_render_path`), never this one.
                #[cfg(feature = "engine-tier")]
                TierBackend::Engine { .. } => {
                    return Err(anyhow!(
                        "frust-render: direct render path requires the GPU tier"
                    ));
                }
            },
            // The engine tier's whole frame: one encoder, one
            // `EngineRenderer::encode` into the acquired swapchain view and
            // the surface's own depth attachment, one submit, then the
            // engine's end-of-frame maintenance. The renderer records into the
            // encoder and never submits, so every pass of the frame lands in
            // this one command buffer, in order, against the texture about to
            // be presented.
            #[cfg(feature = "engine-tier")]
            RenderPath::EngineDirect { depth } => {
                let TierBackend::Engine {
                    engine,
                    refused_frames,
                    timestamps,
                } = backend
                else {
                    return Err(anyhow!(
                        "frust-render: engine render path requires the engine tier"
                    ));
                };
                // Set in `encode`; a well-formed frame always encoded first —
                // the same fallback every other arm takes.
                let base_color = pending_base_color.take().unwrap_or(peniko::Color::BLACK);
                let mut encoder =
                    device_handle
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frust-render engine"),
                        });
                // Opens this frame's ring slot and harvests whatever an earlier
                // frame left mapped in it — the one place a reading becomes
                // visible to `gpu_pass_timings`. A no-op on an inert ring.
                timestamps.begin_frame(&device_handle.device);
                let result = engine.encode_traced(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine_scene,
                    frust_engine::EngineTarget {
                        view: &swapchain_view,
                        format: surface.config.format,
                        width: surface.config.width,
                        height: surface.config.height,
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
                    // The engine arm renders at the surface's own resolution:
                    // `FRUST_RENDER_SCALE` is a vello instrument that sizes
                    // the blit arm's intermediate, and this arm has none.
                    Affine::IDENTITY,
                    frust_engine::FrameTimestamps::new(timestamps),
                );
                match result {
                    Ok(()) => {
                        // Recorded into the frame's own encoder, after every
                        // pass that wrote a query and before the one submit —
                        // a resolve in a second command buffer would race the
                        // passes it reads.
                        timestamps.resolve(&mut encoder);
                        device_handle.queue.submit([encoder.finish()]);
                        timestamps.end_frame();
                        engine.end_frame(&device_handle.queue);
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
            #[cfg(feature = "engine-tier")]
            RenderPath::EngineDirectUnpremultiply {
                depth,
                intermediate_view,
                present,
            } => {
                let TierBackend::Engine {
                    engine,
                    refused_frames,
                    timestamps,
                } = backend
                else {
                    return Err(anyhow!(
                        "frust-render: engine render path requires the engine tier"
                    ));
                };
                let base_color = pending_base_color.take().unwrap_or(peniko::Color::BLACK);
                let mut encoder =
                    device_handle
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frust-render engine unpremultiply"),
                        });
                timestamps.begin_frame(&device_handle.device);
                let result = engine.encode_traced(
                    &device_handle.device,
                    &device_handle.queue,
                    &mut encoder,
                    engine_scene,
                    frust_engine::EngineTarget {
                        // The frame lands in the intermediate, never in the
                        // swapchain: what the swapchain receives is the
                        // conversion's output.
                        view: intermediate_view,
                        format: engine_target_format(&surface.path, surface.config.format),
                        width: surface.config.width,
                        height: surface.config.height,
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
                match result {
                    Ok(()) => {
                        // The conversion pass itself: charged to `blit` with a
                        // fresh pair from the same ring `encode_traced` just
                        // recorded the frame's other spans into — `None` on an
                        // inert ring (no `perf-trace`, or the device never got
                        // `TIMESTAMP_QUERY`), exactly like every other pass.
                        present.record(
                            &device_handle.device,
                            &mut encoder,
                            intermediate_view,
                            &swapchain_view,
                            frust_engine::FrameTimestamps::new(timestamps)
                                .writes(frust_engine::EngineSpan::Blit),
                        );
                        timestamps.resolve(&mut encoder);
                        device_handle.queue.submit([encoder.finish()]);
                        timestamps.end_frame();
                        engine.end_frame(&device_handle.queue);
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
            // Direct-premultiplied: vello already rendered the
            // straight-alpha frame into the intermediate in `encode`; premultiply
            // it into the acquired swapchain texture so a premultiplied-expecting
            // compositor (Android `Inherit`) blends it correctly. No blit, no
            // intermediate→swapchain copy beyond this single compute dispatch.
            RenderPath::DirectPremultiplied {
                intermediate_view,
                premultiply,
                ..
            } => {
                let mut encoder =
                    device_handle
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frust-render premultiply"),
                        });
                premultiply.record(
                    &device_handle.device,
                    &mut encoder,
                    intermediate_view,
                    &swapchain_view,
                    surface.config.width,
                    surface.config.height,
                );
                device_handle.queue.submit([encoder.finish()]);
            }
            // Blit fallback: copy the intermediate target (filled in `encode`)
            // into the swapchain texture and submit.
            RenderPath::Blit {
                target_view,
                blitter,
                ..
            } => {
                let mut encoder =
                    device_handle
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frust-render blit"),
                        });
                blitter.copy(
                    &device_handle.device,
                    &mut encoder,
                    target_view,
                    &swapchain_view,
                );
                device_handle.queue.submit([encoder.finish()]);
            }
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
/// One answer for both the install site and the resize-time drift check below,
/// so the two cannot disagree about which format this surface's renderer was
/// built for. A non-engine path answers the swapchain's format: it is not
/// reachable with an engine backend, and answering rather than panicking keeps
/// the wiring check that follows the reporting one.
#[cfg(feature = "engine-tier")]
fn engine_target_format(
    path: &RenderPath,
    surface_format: wgpu::TextureFormat,
) -> wgpu::TextureFormat {
    match path {
        RenderPath::EngineDirectUnpremultiply { .. } => {
            frust_engine::gpu::pipelines::INTERMEDIATE_FORMAT
        }
        RenderPath::EngineDirect { .. }
        | RenderPath::Direct
        | RenderPath::DirectPremultiplied { .. }
        | RenderPath::Blit { .. } => surface_format,
    }
}

/// Reports the `count`-th frame this surface's engine renderer refused, under
/// [`crate::context::decide_log_action`]'s latch.
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
#[cfg(feature = "engine-tier")]
fn log_engine_refusal(count: u32, error: &frust_engine::EngineError) {
    match crate::context::decide_log_action(count) {
        crate::context::LogAction::Log => {
            log::warn!(
                "frust-render: engine refused frame {count} on this surface — {error}; the \
                 frame is dropped rather than presented"
            );
        }
        crate::context::LogAction::SuppressionNotice => {
            log::warn!(
                "frust-render: further engine frame refusals suppressed (total so far: {count})"
            );
        }
        crate::context::LogAction::Silent { debug_bump } => {
            if debug_bump {
                log::debug!(
                    "frust-render: engine frame refusals now {count} on this surface (still \
                     suppressed)"
                );
            }
        }
    }
}

/// The shader-showcase pre-pass (Gpu tier only): for every distinct
/// `Command::ShaderQuad` in `scene`, compile its program, render it into an
/// offscreen texture at the quad's physical size, register that texture with
/// vello as an image override, and collect a `(program id, clamped physical
/// size) → ImageData` map for [`convert::encode_into_with_shaders`] to lower
/// each quad against.
///
/// The map is keyed by `(id, w, h)` — not `id` alone — so the same program drawn
/// at two different physical sizes in one frame resolves each quad to its own
/// size's texture; the encode side recomputes the identical `(w, h)` via
/// [`physical_size`]/[`clamp_size`] (with the same `adapter_max`) to look each
/// entry up.
///
/// Ordering: all quad passes share ONE command encoder,
/// submitted BEFORE the caller's `render_to_texture`, so wgpu's in-order queue
/// serialization guarantees each shader texture is complete before vello's
/// atlas copy reads it. Registration happens once per target (via
/// `ShaderEffects::ensure_registered`); the override is re-marked dirty every
/// frame the quad is present (the intended per-frame texture→atlas copy cost).
/// Eviction is frame-scoped ([`ShaderEffects::evict_stale_targets`], driven by
/// this frame's live key set): a resized-away size is reclaimed and unregistered
/// via `take_dropped_images`, but two sizes of one program live in the same
/// frame both survive. Never panics — a failed compile is recorded/skipped
/// inside `ShaderEffects` and simply yields no map entry (the quad then takes
/// the miss placeholder).
///
/// Whole-id reap ([`ShaderEffects::mark_seen`]/[`ShaderEffects::reap`]) runs
/// once per call, keyed on this frame's live *ids* (not just live keys) so a
/// program absent from the scene entirely for `MAX_UNSEEN_FRAMES` consecutive
/// frames has its pipeline, target(s), and `failed` record dropped instead of
/// living until surface teardown — the whole-id counterpart to the
/// frame-scoped resized-away-size reclaim above.
///
/// `disabled` is the resolved `FRUST_NO_SHADER_EFFECTS` kill switch
/// ([`crate::context::shader_effects_disabled`]), threaded in by the caller so
/// this stays unit-testable with a plain `bool` instead of reading the cached
/// process-global itself. When `true` this is a **full no-op**, checked before
/// any other work (the quad dedup, `ensure_pipeline`/`ensure_target`, and the
/// `mark_seen`/`reap` age tracking all stay untouched): the flag means the
/// pre-pass is off entirely, not merely "don't compile new programs", so a
/// disabled run must not silently age out `last_seen` state a later re-enable
/// would otherwise need. Returns an empty map either way, which is exactly
/// the miss-everywhere input [`convert::encode_into_with_shaders`] already
/// handles by falling through to the placeholder fill.
fn run_shader_prepass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut vello::Renderer,
    shader_effects: &mut ShaderEffects,
    scene: &frust_scene::Scene,
    adapter_max: u32,
    disabled: bool,
) -> HashMap<(u64, u32, u32), ImageData> {
    if disabled {
        return HashMap::new();
    }

    // Collect the distinct quads to render this frame, deduped by
    // (program id, clamped physical size) so a program drawn twice at the same
    // size is compiled/encoded once. `live` is this frame's full key set — what
    // frame-scoped eviction preserves against.
    let mut live: HashSet<(u64, u32, u32)> = HashSet::new();
    let mut quads: Vec<(u64, &str, u32, u32, f32)> = Vec::new();
    for command in scene.commands() {
        if let Command::ShaderQuad {
            program,
            dest,
            transform,
            time,
        } = command
        {
            let (w, h) = clamp_size(physical_size(*transform, *dest), adapter_max);
            if live.insert((program.id(), w, h)) {
                quads.push((program.id(), program.source(), w, h, *time));
            }
        }
    }

    let mut shader_images = HashMap::new();
    if !quads.is_empty() {
        // ONE encoder for every quad pass; submitted before the caller renders.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust shader-effect pre-pass"),
        });
        for (id, wgsl, w, h, time) in quads {
            shader_effects.ensure_pipeline(device, id, wgsl);
            shader_effects.ensure_target(device, id, w, h);
            shader_effects.encode_pass(&mut encoder, queue, id, (w, h), time);
            // Register on first use (stashed in the target entry) and mark dirty
            // every frame so vello recopies the texture into its atlas.
            if let Some(image) = shader_effects
                .ensure_registered(id, w, h, |tex| renderer.register_texture(tex.clone()))
            {
                renderer.mark_override_image_dirty(&image);
                shader_images.insert((id, w, h), image);
            }
        }
        queue.submit([encoder.finish()]);
    }

    // Age out whole ids that have stopped being drawn entirely (screen
    // navigation, a dynamic id, etc.) — computed from this frame's live ids
    // regardless of whether `quads` was empty, so a scene that stops drawing
    // shader quads altogether still ages out and reaps every previously-seen
    // id rather than only ones still present. Reaping a stale id drops its
    // real pipeline/target(s) and any `failed` record.
    let live_ids: HashSet<u64> = live.iter().map(|&(id, _, _)| id).collect();
    let reapable = shader_effects.mark_seen(&live_ids);
    shader_effects.reap(&reapable);

    // Reclaim the resized-away sizes of still-drawn programs (frame-scoped: two
    // live sizes of one id both survive), then hand back any evicted target's
    // image so vello stops treating its (now-dropped) texture as an override.
    shader_effects.evict_stale_targets(&live);
    for image in shader_effects.take_dropped_images() {
        renderer.unregister_texture(image);
    }

    shader_images
}

/// The physical-pixel extent of a shader quad: the axis-aligned bounding box of
/// `dest` mapped through `transform` (the root transform carries the DPI scale),
/// as `(width, height)` rounded to whole pixels. Sizing the shader target in
/// physical pixels keeps its rendered detail matched to the on-screen area. A
/// degenerate/negative extent yields `0`, which `clamp_size` then floors to `1`.
///
/// `pub(crate)` so the encode-side lowering ([`convert::encode_into_with_shaders`])
/// can recompute the identical `(w, h)` per quad — the shader-image map is keyed
/// by `(program id, clamped physical size)`, so the lookup must derive the same
/// size the pre-pass keyed the entry under.
pub(crate) fn physical_size(transform: Affine, dest: kurbo::Rect) -> (u32, u32) {
    let bbox = transform.transform_rect_bbox(dest);
    let to_u32 = |v: f64| {
        let v = v.round();
        if v.is_finite() && v > 0.0 {
            v as u32
        } else {
            0
        }
    };
    (to_u32(bbox.width()), to_u32(bbox.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn physical_size_applies_the_dpi_scale_from_the_transform() {
        // A 100x50 logical dest under a 2x DPI transform is a 200x100 physical
        // target; the origin offset does not affect the extent.
        let dest = kurbo::Rect::new(10.0, 20.0, 110.0, 70.0);
        let transform = Affine::scale(2.0);
        assert_eq!(physical_size(transform, dest), (200, 100));
    }

    #[test]
    fn physical_size_rounds_and_floors_degenerate_extents_to_zero() {
        // A zero-area dest yields (0, 0) — `clamp_size` later floors it to 1.
        let dest = kurbo::Rect::new(5.0, 5.0, 5.0, 5.0);
        assert_eq!(physical_size(Affine::IDENTITY, dest), (0, 0));
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

    /// End-to-end (real device) confirmation of the `FRUST_NO_SHADER_EFFECTS`
    /// kill switch's full-no-op contract: `run_shader_prepass(.., disabled:
    /// true)` returns zero map entries AND never calls `ensure_pipeline` — the
    /// actual compiled-pipeline cache stays empty, not just "the caller
    /// ignored the result" — confirming the kill switch is a true "zero GPU
    /// work" no-op. `disabled` is passed directly rather than going through
    /// `context::shader_effects_disabled()`'s process-cached `OnceLock`, so
    /// this test needs no env-var mutation (the flag-parsing itself is
    /// covered by `context::tests`' `env_flag_*` cases). The resulting empty
    /// map is exactly the input `convert::tests::
    /// shader_quad_miss_maps_to_placeholder_fill_rect_with_dest_and_transform`
    /// (and the new kill-switch-labeled test beside it) confirm lowers every
    /// `Command::ShaderQuad` to the placeholder fill via `RecordingSink`.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn shader_prepass_is_a_full_no_op_when_disabled() {
        pollster::block_on(run());

        async fn run() {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust renderer shader-disabled test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");
            let mut vello_renderer =
                vello::Renderer::new(&device, vello::RendererOptions::default())
                    .expect("failed to create vello renderer");
            let mut shader_effects = ShaderEffects::new(None);

            let program = frust_scene::ShaderProgram::new(
                "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> { \
                 return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }",
            );
            let mut scene = frust_scene::Scene::new();
            {
                let mut builder = frust_scene::SceneBuilder::new(&mut scene);
                builder.draw_shader(&program, kurbo::Rect::new(0.0, 0.0, 32.0, 32.0), 1.0);
            }

            let images = run_shader_prepass(
                &device,
                &queue,
                &mut vello_renderer,
                &mut shader_effects,
                &scene,
                4096,
                true, // FRUST_NO_SHADER_EFFECTS forced on
            );

            assert!(
                images.is_empty(),
                "a disabled pre-pass must yield zero map entries"
            );
            assert!(
                shader_effects.needs_compile(program.id()),
                "a disabled pre-pass must never call ensure_pipeline — zero GPU work"
            );
        }
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
    #[cfg(feature = "engine-tier")]
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render --features engine-tier -- --ignored`"]
    fn engine_arm_records_a_frame_into_its_target_without_validation_errors() {
        /// Serializes every test in this binary that creates a GPU device, the
        /// same guard the workspace's other GPU suites take: the NVIDIA Vulkan
        /// driver serializes `vkDestroyDevice` against other Vulkan work on a
        /// process-global mutex, and two tests tearing devices down at once
        /// have deadlocked inside it. Poison is ignored deliberately — one
        /// test's failure must not cascade into its siblings.
        static RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serialized = RENDER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        const SIZE: u32 = 64;

        let (device, queue, caps) = pollster::block_on(async {
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
        });

        let mut scene = frust_scene::Scene::new();
        {
            let mut builder = frust_scene::SceneBuilder::new(&mut scene);
            builder.fill_rect(
                kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
                peniko::Brush::Solid(peniko::color::palette::css::RED),
            );
        }

        for format in crate::context::SURFACE_FORMATS {
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
}
