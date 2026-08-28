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
//! The two arms remap the v3 present spans — see [`SurfaceRenderer::submit`].
//!
//! On the Gpu tier every arm also runs [`crate::compositor`]'s quad pass after
//! vello, drawing the snapshot cache's cached pages onto the frame; a frame
//! that composites nothing (the common case, and every frame under the
//! `FRUST_NO_SNAPSHOT_LAYERS` kill switch) records no such pass and is
//! byte-identical to the arm above. When the composited frame's own vello
//! segment paints nothing at all ([`skips_main_pass`]) that pass is skipped
//! too and the quad pass clears the target instead — a page transition is then
//! one pass, not two.

use core::ffi::c_void;

use anyhow::{Result, anyhow};

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use frust_scene::Command;
use kurbo::Affine;
use peniko::ImageData;

use crate::compositor::{CompositeTarget, Compositor, OutputAlpha};
use crate::context::{
    AaMode, ConfiguredSurface, DetachedSurface, RenderContext, RenderPath, aa_mode,
};
use crate::convert;
use crate::lifecycle::{
    AcquireAction, AcquireOutcome, AcquireStatus, EncodeOutcome, FrameOutcome, SurfaceEvent,
    SurfacePhase, decide_acquire, next_invalid_streak, next_phase,
};
use crate::shader_effects::{ShaderEffects, clamp_size};
use crate::snapshot::{CompositeLayer, SnapshotCache};

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
#[cfg_attr(feature = "cpu-tier", allow(clippy::large_enum_variant))]
enum TierBackend {
    /// vello 0.9 GPU compute path: `render_to_texture` into the target view.
    /// Reused across frames; its compiled shader pipelines survive resizes
    /// (which recreate only the swapchain/target).
    Gpu {
        renderer: vello::Renderer,
        /// Cached rasterizations of `Command::PushSnapshot` bodies, driven by
        /// the snapshot pre-pass in [`SurfaceRenderer::encode`]. It lives in
        /// this variant rather than beside it in [`ReadySurface`] so the
        /// "GPU tier only" rule is structural: the `cpu-tier` path cannot
        /// name a cache, let alone build one. Its bodies are rasterized by the
        /// `renderer` next to it, which is why the two share a variant.
        snapshots: SnapshotCache,
        /// The quad pass that draws `snapshots`' cached pages onto the frame
        /// after vello (`crate::compositor`), built once per surface for that
        /// surface's composite attachment format.
        ///
        /// `None` only when the compositor refused that format, in which case
        /// `snapshots` was constructed disabled — the two are built together
        /// in [`SurfaceRenderer::install_surface`] precisely so a plan can
        /// never carry layers no one would draw.
        compositor: Option<Compositor>,
    },
    /// Experimental vello_cpu path (`cpu-tier` feature): rasterize headless
    /// into a pixmap, then upload it into the target texture. Boxed because it
    /// carries a reusable `RenderContext`/`Pixmap` that dwarfs the GPU variant.
    #[cfg(feature = "cpu-tier")]
    Cpu(Box<crate::cpu_tier::CpuTierRenderer>),
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
    /// The second reusable scene, holding a frame's TRAILING segment — the
    /// commands recorded after the first composited snapshot bracket (see
    /// [`crate::snapshot::FramePlan`]). Kept beside [`Self::scene`] rather
    /// than reset-and-reused as one buffer because both scenes are alive at
    /// once on the direct arm, where the pre-segment's render happens in
    /// `submit`. Untouched on frames with no trailing segment, which is the
    /// common case.
    trailing_scene: vello::Scene,
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
    /// This frame's composite work, stashed by [`Self::encode`] for
    /// [`Self::submit`] on the two arms whose composite attachment is the
    /// ACQUIRED swapchain texture (`Direct`, `DirectPremultiplied`), which
    /// does not exist until [`Self::acquire`]. The blit arm composites onto
    /// its intermediate inside `encode` and never sets this.
    ///
    /// Overwritten every `encode` and taken by every `submit`, so a frame
    /// whose present was skipped holds its page handles no longer than until
    /// the next one; [`Self::on_surface_destroyed`] clears it outright, since
    /// those handles belong to the cache dying with the surface.
    pending_composite: Option<PendingComposite>,
}

/// The composite half of a frame, carried from [`SurfaceRenderer::encode`] to
/// [`SurfaceRenderer::submit`]: the cached pages to draw, in scene order, and
/// the trailing segment's already-rendered scratch (`None` when the frame has
/// no trailing segment). Both are refcounted `wgpu` handles, not pixels.
struct PendingComposite {
    layers: Vec<CompositeLayer>,
    trailing: Option<wgpu::TextureView>,
    /// The frame's base colour when `encode` SKIPPED the main vello pass (its
    /// segment painted nothing, see [`skips_main_pass`]), making the composite
    /// pass the only thing that writes the target: it clears to this instead
    /// of loading. `None` on an ordinary frame, where vello wrote the backdrop.
    clear: Option<peniko::Color>,
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
            trailing_scene: vello::Scene::new(),
            state: SurfaceState::NoSurface,
            consecutive_invalid: 0,
            initial_cache_data: None,
            pending_present: None,
            pending_base_color: None,
            pending_composite: None,
        }
    }

    /// Drops the in-flight frame's cross-call stashes: the direct arm's
    /// `base_color` and this frame's composite work.
    ///
    /// Both describe one frame of one surface — the composite one holds
    /// refcounted handles on that surface's snapshot textures — so any event
    /// that ends a surface's life (`on_surface_destroyed`) or replaces it
    /// (`install_surface`) must drop them rather than let the next `submit`
    /// consume a stale pair. Idempotent.
    fn clear_pending_frame(&mut self) {
        self.pending_base_color = None;
        self.pending_composite = None;
    }

    /// Adopt `state` and reset everything else scoped to ONE surface: the
    /// `Invalid`-reconfigure streak and the in-flight frame's stashes.
    ///
    /// The one place a surface episode begins or ends. `install_surface`
    /// enters through it with the freshly built [`SurfaceState::Ready`];
    /// `on_surface_destroyed` leaves through it with
    /// [`SurfaceState::NoSurface`]; and [`Self::acquire`]'s give-up arm leaves
    /// through it with [`SurfaceState::Lost`] — so a reset can never be
    /// written into one of those paths and forgotten in another. Everything
    /// reset here describes the surface being replaced or torn down: the
    /// streak belonged to it, and the stashes describe one of its frames (the
    /// composite one holding refcounted handles on snapshot textures that die
    /// with it). Nothing else may assign `self.state`.
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
                // `encode`/`submit_impl` and `snapshot.rs`'s `SnapshotCache::render`):
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
                // The compositor is built for the attachment its quad pass
                // actually draws onto, which is the arm's own vello target:
                // the acquired swapchain texture on both direct arms (always
                // `Rgba8Unorm` — the direct arm configures it so), the
                // intermediate on the blit arm (likewise `Rgba8Unorm`, even
                // when the swapchain behind it is `Bgra8Unorm`).
                let composite_format = match &surface.path {
                    RenderPath::Blit { .. } => wgpu::TextureFormat::Rgba8Unorm,
                    RenderPath::Direct | RenderPath::DirectPremultiplied { .. } => {
                        surface.config.format
                    }
                };
                let compositor = Compositor::new(device, composite_format, pipeline_cache.as_ref());
                // A translucent surface whose swapchain stores STRAIGHT alpha
                // (iOS's `PostMultiplied`) gets no snapshot cache at all — see
                // `snapshot_cache_enabled`. Logged once per process, naming
                // the reason, because "snapshot layers silently off on this
                // device" is otherwise indistinguishable from the kill switch.
                let straight_alpha_translucent = surface.straight_alpha_translucent();
                if straight_alpha_translucent {
                    static LOGGED: OnceLock<()> = OnceLock::new();
                    LOGGED.get_or_init(|| {
                        log::info!(
                            "frust-render: snapshot layers disabled on this surface — a \
                             translucent swapchain storing straight alpha has no exact \
                             composite arithmetic; brackets lower inline"
                        );
                    });
                }
                // The same one-per-process note for the other silent refusal:
                // a scaled frame gets no snapshot cache either, and "off"
                // would otherwise look identical to the kill switch in a
                // device capture taken to compare the two.
                //
                // Asked of the SURFACE, not of the knob: this must be the same
                // truth the frame is encoded under (`RenderPath::Blit`'s
                // `root`), and only the surface knows whether its own
                // intermediate ended up smaller — a cpu-tier surface is pinned
                // to full resolution however the knob is set.
                let render_scaled = surface.render_scaled();
                if render_scaled {
                    static LOGGED: OnceLock<()> = OnceLock::new();
                    LOGGED.get_or_init(|| {
                        log::info!(
                            "frust-render: snapshot layers disabled on this surface — render \
                             scale < 1: cached pages are composited in the target's device \
                             space; brackets lower inline"
                        );
                    });
                }
                TierBackend::Gpu {
                    renderer,
                    // The four refusals are resolved here, per surface, and
                    // held by the cache itself — so the pre-pass never
                    // re-reads a process-global on the hot path and a test
                    // can build either state directly. `on_surface_changed`
                    // re-resolves them through the same
                    // `snapshot_cache_enabled_for`: the scaled-frame refusal
                    // is a function of the surface's live geometry, not a
                    // constant of the process.
                    snapshots: SnapshotCache::new(snapshot_cache_enabled_for(
                        &surface,
                        compositor.is_some(),
                    )),
                    compositor,
                }
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
    /// — only the surface config and target texture are recreated, and the
    /// snapshot cache's switch is re-resolved against the new geometry
    /// ([`snapshot_cache_enabled_for`]). Zero dimensions are ignored (a
    /// minimized window keeps its last valid size).
    pub fn on_surface_changed(&mut self, ctx: &RenderContext, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let SurfaceState::Ready(ready) = &mut self.state {
            ctx.resize_surface(&mut ready.surface, width, height);
            match &mut ready.backend {
                // The snapshot cache's switch is re-resolved against the
                // resized surface: the scaled-frame refusal compares the
                // surface's size with its intermediate's, and the resize
                // above recomputed both (`RenderContext::resize_surface`).
                // Frozen at install it could disagree with the `root` the
                // next frame is encoded under; `set_enabled` drops the
                // entries on the way off, so a page rasterized for one
                // device space is never composited in another.
                TierBackend::Gpu {
                    snapshots,
                    compositor,
                    ..
                } => snapshots.set_enabled(snapshot_cache_enabled_for(
                    &ready.surface,
                    compositor.is_some(),
                )),
                #[cfg(feature = "cpu-tier")]
                TierBackend::Cpu(cpu) => {
                    // Keep the CPU pixmap's size in step with the swapchain;
                    // the GPU renderer needs no resize (only the target
                    // texture, done above), but the CPU tier's
                    // `RenderContext`/`Pixmap` are sized.
                    cpu.resize(width, height);
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
        // Release the in-flight frame's stashes, then the snapshot cache's
        // textures, while the renderer that rasterized them is still alive:
        // all of it dies with the state below, so this buys a deterministic
        // order rather than fixing a leak. Both stashes go, not just the
        // composite one: they describe a frame of the surface that is dying,
        // and a stale `base_color` is as wrong to hand the next `submit` as a
        // stale composite. Idempotent — a cleared cache and cleared stashes
        // clear again to nothing, which is what the repeated-`Destroyed`
        // contract needs.
        self.clear_pending_frame();
        if let SurfaceState::Ready(ready) = &mut self.state {
            match &mut ready.backend {
                TierBackend::Gpu { snapshots, .. } => snapshots.clear(),
                #[cfg(feature = "cpu-tier")]
                TierBackend::Cpu(_) => {}
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
    ///   render, as it always has on this path — and composites this frame's
    ///   cached snapshot pages onto that same target, before the blit.
    /// - **Direct arm**: does ONLY the CPU-side scene build and stashes
    ///   `base_color`; the GPU render moves to [`Self::submit`] (it needs the
    ///   acquired swapchain texture), and so does the composite pass.
    ///   `encode_us` is then just the CPU encode, plus the trailing segment's
    ///   render on the frames that have one.
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
            trailing_scene,
            state,
            pending_base_color,
            pending_composite,
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
            TierBackend::Gpu {
                renderer,
                snapshots,
                compositor,
            } => {
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
                // `convert::encode_range_with_overrides`'s existing miss
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
                // Snapshot pre-pass (snapshot layers): rasterize each
                // outermost `Command::PushSnapshot` body whose content or size
                // changed into its own cached texture, and take this frame's
                // plan — the ordered pages to composite, the bracket keys the
                // encode walk must skip as HOLES, and the two-segment split of
                // the command list those two imply
                // (`crate::snapshot::FramePlan`).
                //
                // Runs on EVERY render path (Direct, DirectPremultiplied,
                // Blit) and before the main encode, for the same reason the
                // shader pre-pass does: its `render_to_texture` calls submit
                // their own work, and wgpu serializes queue submissions, so
                // this frame's own passes see complete textures. An unchanged
                // bracket performs no GPU work at all here — the steady state
                // of an animating page transition is zero rasterizations and
                // one blended quad.
                //
                // `FRUST_NO_SNAPSHOT_LAYERS` (resolved into the cache at
                // surface install and on every resize,
                // `docs/RENDER_DEVELOPMENT.md` § Instrumentation (render path))
                // makes this a zero-GPU-work no-op returning the
                // whole-scene plan — no layers, no holes, no trailing segment
                // — so every bracket falls through to the inline emulation
                // `convert` performed before the cache existed and the
                // compositor pass below never runs.
                let width = ready.surface.config.width;
                let height = ready.surface.config.height;
                let plan = snapshots.prepare(
                    &device_handle.device,
                    &device_handle.queue,
                    renderer,
                    scene,
                    adapter_max,
                    // The surface's own pixel size, the yardstick each
                    // bracket's texture area is budgeted against: a page is a
                    // subtree of THIS surface, so a texture much larger than
                    // it is a bracket to lower inline, not to cache.
                    (width, height),
                );
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
                };
                let params =
                    |base_color, (target_width, target_height): (u32, u32)| vello::RenderParams {
                        base_color,
                        width: target_width,
                        height: target_height,
                        antialiasing_method: aa_mode().to_vello(),
                    };
                // A pre segment that paints nothing buys no vello pass: the
                // compositor's own pass clears the frame to `base_color`
                // instead (see `skips_main_pass`). The measured page
                // transition lands here — its pre segment is the app root's
                // `PushClip` and nothing else, and a vello pass over it still
                // costs a full fine-stage sweep of the surface (~14.5 ms on an
                // Adreno 620) to draw not one pixel.
                let clear =
                    skips_main_pass(plan.pre_draws, plan.layers.len()).then_some(base_color);

                // The frame's MAIN vello pass: the commands before the first
                // composited bracket, with every composited bracket skipped.
                // That is the whole scene whenever nothing composited, so a
                // frame with no cache hits builds exactly the scene it always
                // did. Still encoded when the pass is skipped — a structural
                // segment is a handful of commands, and keeping the CPU side
                // unconditional keeps one scene-building path.
                vello_scene.reset();
                convert::encode_range_with_overrides(
                    scene,
                    // The pre segment starts at the scene root, so its own
                    // prefix is empty: it opens every group it needs itself.
                    &plan.pre,
                    root,
                    vello_scene,
                    &shader_images,
                    adapter_max,
                    &plan.holes,
                );

                // The TRAILING segment, present only when the scene draws
                // something after the first composited bracket that is not
                // itself composited: one extra vello pass into the
                // compositor's transparent scratch, which the quad pass then
                // draws last so that content keeps its z-order above the
                // pages. This is the only extra vello pass the mechanism can
                // ever add — there is no third segment.
                let trailing = match (&plan.trailing, compositor.as_mut()) {
                    (Some(segment), Some(compositor)) => {
                        let scratch = compositor
                            .ensure_scratch(&device_handle.device, pass_width, pass_height)
                            .clone();
                        trailing_scene.reset();
                        convert::encode_range_with_overrides(
                            scene,
                            // The segment carries BOTH its range and the
                            // clips/layers still open where it starts — the
                            // app root's clip around both pages, a scroll
                            // viewport's around a switcher. Forwarding one
                            // value is what makes forgetting the second
                            // impossible; without them this pass would draw
                            // unclipped and pop groups it never pushed.
                            segment,
                            // The SAME root as the pre segment: the two passes
                            // are composited onto one another, so a different
                            // root here would misplace this one against the
                            // pixels it draws over.
                            root,
                            trailing_scene,
                            &shader_images,
                            adapter_max,
                            &plan.holes,
                        );
                        renderer
                            .render_to_texture(
                                &device_handle.device,
                                &device_handle.queue,
                                trailing_scene,
                                &scratch,
                                // Transparent, not the frame's base color:
                                // everything this segment does not paint must
                                // composite through to the pages under it.
                                &params(
                                    peniko::color::palette::css::TRANSPARENT,
                                    (pass_width, pass_height),
                                ),
                            )
                            .map_err(|e| {
                                anyhow!("frust-render: vello render_to_texture failed: {e}")
                            })?;
                        Some(scratch)
                    }
                    _ => None,
                };
                // Age the compositor's trailing scratch on EVERY frame, not
                // only the ones that used it: a full-surface `Rgba8Unorm`
                // texture is ~10 MB on a 1080x2400 phone, and a scene that
                // has stopped drawing anything after its first composited
                // bracket (the common page-transition shape) would otherwise
                // hold it until the surface died. Costs a counter increment
                // on a frame with no scratch at all.
                if let Some(compositor) = compositor.as_mut() {
                    compositor.age_scratch();
                }

                match &ready.surface.path {
                    // Direct-to-surface: the vello render targets the
                    // acquired swapchain texture, which does not exist until
                    // `acquire`. So `encode` does ONLY the CPU-side scene build
                    // here; the GPU `render_to_texture` — and, after it, the
                    // composite pass onto that same texture — move to `submit`.
                    // See `submit`'s span-mapping comment for how this remaps
                    // the v3 spans.
                    RenderPath::Direct => {
                        // Carry `base_color` and this frame's pages to
                        // `submit`, where both are consumed — including the
                        // decision to skip the render there entirely.
                        *pending_base_color = Some(base_color);
                        *pending_composite = Some(PendingComposite {
                            layers: plan.layers,
                            trailing,
                            clear,
                        });
                    }
                    // Direct-premultiplied (translucent, premultiplied-expecting):
                    // unlike the plain direct arm, the intermediate
                    // already exists at encode time, so vello renders into it now
                    // (like the blit arm); `submit`'s premultiply compute pass then
                    // writes `(rgb*a, a)` into the acquired swapchain texture, and
                    // the composite pass follows it there — onto premultiplied
                    // pixels, which is why that arm draws its own pipeline variant.
                    RenderPath::DirectPremultiplied {
                        intermediate_view, ..
                    } => {
                        // Skipped exactly as on the other two arms. The
                        // premultiply pass in `submit` then reads a stale
                        // intermediate, but its output is discarded by the
                        // composite pass's own `LoadOp::Clear` — the frame is
                        // whatever this pass's clear plus its quads make it.
                        if clear.is_none() {
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
                        *pending_composite = Some(PendingComposite {
                            layers: plan.layers,
                            trailing,
                            clear,
                        });
                    }
                    // Blit fallback: render into the intermediate `Rgba8Unorm` target
                    // now and composite this frame's pages straight onto it, before
                    // the acquire/blit/present tail (see `submit`) copies it to the
                    // swapchain. Nothing crosses the encode/submit gap on this arm.
                    RenderPath::Blit { target_view, .. } => {
                        if clear.is_none() {
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
                        if let Some(compositor) = compositor.as_mut() {
                            compositor.composite(
                                &device_handle.device,
                                &device_handle.queue,
                                CompositeTarget {
                                    view: target_view,
                                    // The target the quads land in, not the
                                    // swapchain behind it (unreachable while
                                    // scaled — the cache is refused then — but
                                    // the size must still name this target).
                                    size: (pass_width, pass_height),
                                    output: OutputAlpha::Straight,
                                    clear,
                                },
                                &plan.layers,
                                trailing.as_ref(),
                            );
                        }
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
            // stashes: a `pending_composite` holds refcounted handles on the
            // dying cache's page textures (and the trailing scratch), and no
            // `submit` will ever consume it now, so leaving it would keep those
            // textures alive until the shell's next install. The streak was
            // already reset above (`next_invalid_streak`, reset-on-give-up);
            // the door zeroing it again is a no-op. The shell's own recovery
            // path (recreate on resize/redraw/surfaceChanged) then starts a
            // fresh episode.
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
            pending_composite,
            ..
        } = self;
        // Taken unconditionally: a frame that reached `acquire` consumes its
        // composite here or not at all, and holding cached-page handles past
        // that would outlive the plan they came from.
        let pending_composite = pending_composite.take();
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
            // texture, composite this frame's cached pages onto it, then present.
            // No intermediate, no blit pass.
            RenderPath::Direct => match backend {
                TierBackend::Gpu {
                    renderer,
                    compositor,
                    ..
                } => {
                    let params = vello::RenderParams {
                        // Set in `encode`; a well-formed frame always encoded first.
                        base_color: pending_base_color.take().unwrap_or(peniko::Color::BLACK),
                        width: surface.config.width,
                        height: surface.config.height,
                        antialiasing_method: aa_mode().to_vello(),
                    };
                    // `encode` decided this frame's pre segment paints
                    // nothing, so the composite pass below clears the acquired
                    // texture to the base colour and draws the pages onto it —
                    // the whole frame, without a vello pass.
                    let clear = pending_composite.as_ref().and_then(|pending| pending.clear);
                    if clear.is_none() {
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
                    // Straight alpha: the swapchain this arm presents is
                    // opaque (a premultiplied-expecting translucent surface
                    // takes `DirectPremultiplied` below instead).
                    if let Some(compositor) = compositor.as_mut()
                        && let Some(pending) = pending_composite.as_ref()
                    {
                        compositor.composite(
                            &device_handle.device,
                            &device_handle.queue,
                            CompositeTarget {
                                view: &swapchain_view,
                                size: (surface.config.width, surface.config.height),
                                output: OutputAlpha::Straight,
                                clear,
                            },
                            &pending.layers,
                            pending.trailing.as_ref(),
                        );
                    }
                }
                // Unreachable: the direct arm is only ever configured for the GPU
                // tier (`choose_render_path` forces blit for cpu-tier).
                #[cfg(feature = "cpu-tier")]
                TierBackend::Cpu(_) => {
                    return Err(anyhow!(
                        "frust-render: direct render path requires the GPU tier"
                    ));
                }
            },
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
                // The pages go on AFTER the premultiply pass, onto the
                // premultiplied swapchain it just wrote — with the
                // premultiplied output variant, so a composited page carries
                // the same `(rgb*a, a)` convention every other pixel on that
                // surface now does.
                if let TierBackend::Gpu { compositor, .. } = backend
                    && let Some(compositor) = compositor.as_mut()
                    && let Some(pending) = pending_composite.as_ref()
                {
                    compositor.composite(
                        &device_handle.device,
                        &device_handle.queue,
                        CompositeTarget {
                            view: &swapchain_view,
                            size: (surface.config.width, surface.config.height),
                            output: OutputAlpha::Premultiplied,
                            clear: pending.clear,
                        },
                        &pending.layers,
                        pending.trailing.as_ref(),
                    );
                }
            }
            // Blit fallback: copy the intermediate target (filled — and
            // composited onto — in `encode`) into the swapchain texture and
            // submit.
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

/// Whether a surface gets a live [`SnapshotCache`] at all, from the four
/// facts that can refuse it — resolved once per surface at install and held by
/// the cache itself, so the hot path re-reads none of them.
///
/// - `disabled`: the `FRUST_NO_SNAPSHOT_LAYERS` kill switch
///   ([`crate::context::snapshot_layers_disabled`]);
/// - `has_compositor`: the compositor refused this surface's attachment format
///   ([`Compositor::new`]) — a composited page nothing draws is a missing
///   page, so the bracket must lower inline instead;
/// - `straight_alpha_translucent`: the surface came up TRANSLUCENT with a
///   swapchain that stores straight alpha
///   ([`ConfiguredSurface::straight_alpha_translucent`], iOS's
///   `PostMultiplied`). The compositor's straight arm blends
///   `dst * (1 - a) + rgb * a`, which is the straight-alpha `over` only when
///   the destination alpha is 1; over a partly transparent destination it
///   stores premultiplied colour into a straight-alpha swapchain and the
///   platform composites it over-bright. The premultiplied arm's algebra
///   closes for every destination alpha, but it is not what such a swapchain
///   holds. Rather than composite wrong pixels on a surface frust cannot test
///   on every device, that surface keeps the inline path outright — the same
///   refusal shape `context::blit_translucency_refused` already takes.
/// - `render_scaled`: the surface renders the frame into a smaller
///   intermediate ([`ConfiguredSurface::render_scaled`]). A cached page
///   leaves vello as a finished texture and the compositor places its quad in
///   the TARGET's device space, so honouring a scaled root would mean
///   rescaling every quad, every scissor rect and every page's own raster —
///   for a measurement instrument, not a shipping mode. The knob's whole
///   point is to time vello's fine stage over a known pixel count anyway,
///   which a frame that composites cached pages instead of rendering them no
///   longer measures. So brackets lower inline through the root-scaled encode,
///   exactly as under `FRUST_NO_SNAPSHOT_LAYERS`. Unlike the other three this
///   one can change while the surface lives — a resize recomputes the
///   intermediate — so it is re-asked on every `on_surface_changed`.
///
/// Any one of the four is enough to refuse; a plain opaque, unscaled surface
/// with a compositor is the only combination that composites.
fn snapshot_cache_enabled(
    disabled: bool,
    has_compositor: bool,
    straight_alpha_translucent: bool,
    render_scaled: bool,
) -> bool {
    !disabled && has_compositor && !straight_alpha_translucent && !render_scaled
}

/// [`snapshot_cache_enabled`] asked of `surface` as it is configured RIGHT
/// NOW — the one derivation both the install site and
/// [`SurfaceRenderer::on_surface_changed`] call, so the cache's switch can
/// never disagree with the geometry the surface was last configured with:
/// `render_scaled` is the surface's live size against its intermediate's
/// ([`ConfiguredSurface::render_scaled`]), which a resize recomputes.
fn snapshot_cache_enabled_for(surface: &ConfiguredSurface, has_compositor: bool) -> bool {
    snapshot_cache_enabled(
        crate::context::snapshot_layers_disabled(),
        has_compositor,
        surface.straight_alpha_translucent(),
        surface.render_scaled(),
    )
}

/// Whether this frame can skip its MAIN vello pass altogether: its pre segment
/// paints nothing (`crate::snapshot::FramePlan::pre_draws`) AND the compositor
/// has at least one page to draw, so the composite pass can clear the target to
/// the frame's base colour and compose the whole frame by itself.
///
/// Both halves matter. Without a page there is no compositor pass at all, so
/// dropping the vello pass would present an undefined frame; and a pre segment
/// that paints must be rendered whatever else the frame does. An ordinary
/// frame (nothing composited, the kill switch, the cpu tier) therefore always
/// answers `false` and behaves exactly as it did before this seam existed.
fn skips_main_pass(pre_draws: bool, layers: usize) -> bool {
    !pre_draws && layers > 0
}

/// The shader-showcase pre-pass (Gpu tier only): for every distinct
/// `Command::ShaderQuad` in `scene`, compile its program, render it into an
/// offscreen texture at the quad's physical size, register that texture with
/// vello as an image override, and collect a `(program id, clamped physical
/// size) → ImageData` map for [`convert::encode_range_with_overrides`] to lower
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
/// the miss-everywhere input [`convert::encode_range_with_overrides`] already
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

    /// The main vello pass is skipped only when a composite pass will both
    /// clear the target and cover it: a structure-only pre segment with pages
    /// to draw. Every other frame renders exactly as it always did.
    #[test]
    fn only_a_structure_only_pre_segment_with_pages_skips_the_main_pass() {
        assert!(
            skips_main_pass(false, 1),
            "a PushClip-only pre segment with a page to draw needs no vello pass"
        );
        assert!(
            !skips_main_pass(false, 0),
            "with no page there is no composite pass to clear the frame"
        );
        assert!(
            !skips_main_pass(true, 2),
            "a pre segment that paints must still be rendered"
        );
        assert!(!skips_main_pass(true, 0), "an ordinary frame is untouched");
    }

    /// The snapshot cache is live only for a surface all four refusals pass:
    /// the kill switch off, a compositor for the attachment format, a
    /// destination the straight blend is exact for, and an unscaled frame.
    #[test]
    fn the_snapshot_cache_is_enabled_only_for_a_surface_that_can_composite() {
        assert!(snapshot_cache_enabled(false, true, false, false));
        assert!(
            !snapshot_cache_enabled(true, true, false, false),
            "FRUST_NO_SNAPSHOT_LAYERS refuses on its own"
        );
        assert!(
            !snapshot_cache_enabled(false, false, false, false),
            "a composited page nothing draws is a missing page"
        );
        assert!(
            !snapshot_cache_enabled(false, true, true, false),
            "a translucent straight-alpha swapchain has no exact composite \
             arithmetic — brackets lower inline"
        );
        assert!(
            !snapshot_cache_enabled(false, true, false, true),
            "FRUST_RENDER_SCALE refuses on its own: a cached page is \
             composited in the target's device space, which a scaled root \
             never reaches"
        );
        assert!(!snapshot_cache_enabled(true, false, true, true));
    }

    /// One surface episode ending drops everything scoped to it: the
    /// `Invalid`-reconfigure streak, the stashed base colour, and the
    /// composite work (which holds handles on snapshot textures that died
    /// with that surface).
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
        renderer.pending_composite = Some(PendingComposite {
            layers: Vec::new(),
            trailing: None,
            clear: Some(peniko::Color::BLACK),
        });
        renderer.consecutive_invalid = 3;

        renderer.adopt_surface_state(SurfaceState::NoSurface);

        assert!(renderer.pending_base_color.is_none());
        assert!(renderer.pending_composite.is_none());
        assert_eq!(renderer.consecutive_invalid, 0);
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        // Idempotent, as the repeated-`Destroyed` contract needs.
        renderer.adopt_surface_state(SurfaceState::NoSurface);
        assert!(renderer.pending_composite.is_none());
        assert_eq!(renderer.consecutive_invalid, 0);
    }

    /// The give-up arm of `acquire` leaves through the same door: adopting
    /// [`SurfaceState::Lost`] drops both in-flight stashes, so a surface that
    /// went away mid-frame does not keep its cached page textures alive (via
    /// the composite stash's refcounted views) until the shell's next install.
    #[test]
    fn adopting_the_lost_state_drops_the_dying_surfaces_stashes() {
        let mut renderer = SurfaceRenderer::new();
        renderer.pending_base_color = Some(peniko::Color::WHITE);
        renderer.pending_composite = Some(PendingComposite {
            layers: Vec::new(),
            trailing: None,
            clear: None,
        });

        renderer.adopt_surface_state(SurfaceState::Lost);

        assert_eq!(renderer.phase(), SurfacePhase::SurfaceLost);
        assert!(renderer.pending_base_color.is_none());
        assert!(
            renderer.pending_composite.is_none(),
            "a lost surface's composite stash must not outlive it"
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

    /// Destroy is idempotent, reaches `NoSurface`, and — the half that used
    /// to be open-coded — drops BOTH in-flight stashes, not just the
    /// composite one. A stale `base_color` describes the dead surface's frame
    /// exactly as much as a stale composite does.
    #[test]
    fn destroy_is_idempotent_and_resets_to_no_surface() {
        let mut renderer = SurfaceRenderer::new();
        renderer.pending_base_color = Some(peniko::Color::WHITE);
        renderer.pending_composite = Some(PendingComposite {
            layers: Vec::new(),
            trailing: None,
            clear: Some(peniko::Color::BLACK),
        });

        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        assert!(
            renderer.pending_base_color.is_none(),
            "the dying surface's base colour must not reach the next submit"
        );
        assert!(renderer.pending_composite.is_none());

        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        assert!(renderer.pending_base_color.is_none());
        assert!(renderer.pending_composite.is_none());
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
}
