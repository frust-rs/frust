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

use core::ffi::c_void;

use anyhow::{Result, anyhow};

use std::collections::{HashMap, HashSet};

use frust_scene::Command;
use kurbo::Affine;
use peniko::ImageData;

use crate::context::{ConfiguredSurface, DetachedSurface, RenderContext, RenderPath};
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
    /// after the surface is installed instead (review finding M1 — a fallback
    /// must degrade to the opaque Mode A contract, or the hole punch presents
    /// black rectangles on an opaque swapchain).
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
    /// `PIPELINE_CACHE` (Metal/desktop) — see
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
    Gpu(vello::Renderer),
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
    /// `PIPELINE_CACHE` (Vulkan/Android); validated and discarded elsewhere.
    initial_cache_data: Option<Vec<u8>>,
    /// The swapchain texture [`Self::acquire`] acquired and stashed for
    /// [`Self::submit`] to blit into and present (Phase 11.A present-span split).
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
        }
    }

    /// Restores the persisted pipeline-cache blob a prior run produced via
    /// [`pipeline_cache_data`](Self::pipeline_cache_data), to seed vello's
    /// shader-pipeline compilation at the next surface install and cut warm-start
    /// shader/pipeline compilation to near zero on Vulkan (Android).
    ///
    /// Builder-style (a setter rather than a new `on_surface_created*`
    /// parameter) so the three surface-creation entry points — and every shell
    /// call site — keep their signatures; the shell (persistence lands in tasks
    /// 13/14) calls this once after [`SurfaceRenderer::new`] and before the
    /// first `on_surface_created*`. Has no effect in practice on adapters
    /// without `PIPELINE_CACHE` (Metal/desktop): the blob is validated against
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
    /// adapter without `PIPELINE_CACHE` (Metal/desktop) — or when the driver has
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
    /// must key off, replacing the `SurfaceAlphaRequest` it *asked* for
    /// (review finding M1).
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
        // `PIPELINE_CACHE` (Metal/desktop) — the path is then byte-identical to
        // before this existed. Created before the tier `match` so the `Gpu` arm
        // can clone it into `RendererOptions`; the `Cpu` tier runs no GPU
        // pipelines and leaves it unused (retained only so a later
        // `pipeline_cache_data()` still has the handle).
        let pipeline_cache = ctx.create_pipeline_cache(self.initial_cache_data.as_deref());

        // Pick the tier backend the context's probe selected (task 02 / task 06).
        // Only `Gpu` is reachable without the `cpu-tier` feature (the probe
        // never returns `Cpu` there, and `ensure_device` guards it), so the
        // default build creates a `vello::Renderer` exactly as before.
        let backend = match ctx.selected_tier() {
            crate::RenderTier::Gpu => {
                let device = &ctx.device_handle().device;
                // Narrow the compiled AA pipeline set to `Area` only (the sole
                // `AaConfig` `render()` ever requests — see the
                // `antialiasing_method: vello::AaConfig::Area` `RenderParams`
                // below): `RendererOptions::default()` compiles shader
                // permutations for every `AaConfig` (`AaSupport::all()`), ~3x
                // unnecessary pipeline compiles at init that contribute to the
                // slow, synchronous, main-thread launch-time shader compile
                // behind 6e Finding 5's iOS SIGKILL (6e-fix-1 task 03).
                // Verified against the vello 0.9.0 source
                // (`RendererOptions::antialiasing_support: AaSupport`,
                // `AaSupport::area_only()` — both public, non-`non_exhaustive`).
                let renderer_options = vello::RendererOptions {
                    antialiasing_support: vello::AaSupport::area_only(),
                    pipeline_cache: pipeline_cache.clone(),
                    ..Default::default()
                };
                let renderer = vello::Renderer::new(device, renderer_options)
                    .map_err(|e| anyhow!("frust-render: failed to create vello renderer: {e}"))?;
                TierBackend::Gpu(renderer)
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
            "Created must reach SurfaceReady (spec §8.1)"
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
        self.state = SurfaceState::Ready(Box::new(ReadySurface {
            surface,
            backend,
            pipeline_cache,
            shader_effects,
        }));
        // A freshly (re)installed surface starts a new `Invalid`-reconfigure
        // episode — any prior streak belonged to the surface just replaced.
        self.consecutive_invalid = 0;
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
            #[cfg(feature = "cpu-tier")]
            if let TierBackend::Cpu(cpu) = &mut ready.backend {
                // Keep the CPU pixmap's size in step with the swapchain; the
                // GPU renderer needs no resize (only the target texture, done
                // above), but the CPU tier's `RenderContext`/`Pixmap` are sized.
                cpu.resize(width, height);
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
            "Destroyed must reach NoSurface (spec §8.1)"
        );
        self.state = SurfaceState::NoSurface;
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
    ///   render, as it always has on this path.
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
            TierBackend::Gpu(renderer) => {
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
                // `docs/DEVELOPMENT.md`'s Instrumentation table) is the kill
                // switch for this still-unproven-on-device pre-pass: resolved
                // once here and threaded into `run_shader_prepass`, which
                // short-circuits to a zero-GPU-work no-op (skipping compile,
                // targets, and the mark_seen/reap age tracking too) before
                // touching `device`/`queue`/`renderer`/`shader_effects` at all —
                // every `Command::ShaderQuad` this frame then falls through to
                // `convert::encode_scene_with_shaders`'s existing miss
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
                match &ready.surface.path {
                    // Direct-to-surface (deliverable 1): the vello render targets the
                    // acquired swapchain texture, which does not exist until
                    // `acquire`. So `encode` does ONLY the CPU-side scene build here;
                    // the GPU `render_to_texture` moves to `submit`. See `submit`'s
                    // span-mapping comment for how this remaps the v3 spans.
                    RenderPath::Direct => {
                        vello_scene.reset();
                        convert::encode_scene_with_shaders(
                            scene,
                            vello_scene,
                            &shader_images,
                            adapter_max,
                        );
                        // Carry `base_color` to `submit`, where the render runs.
                        *pending_base_color = Some(base_color);
                    }
                    // Direct-premultiplied (translucent, premultiplied-expecting —
                    // defect D3): unlike the plain direct arm, the intermediate
                    // already exists at encode time, so vello renders into it now
                    // (like the blit arm); `submit`'s premultiply compute pass then
                    // writes `(rgb*a, a)` into the acquired swapchain texture.
                    RenderPath::DirectPremultiplied {
                        intermediate_view, ..
                    } => {
                        vello_scene.reset();
                        convert::encode_scene_with_shaders(
                            scene,
                            vello_scene,
                            &shader_images,
                            adapter_max,
                        );
                        let params = vello::RenderParams {
                            base_color,
                            width: ready.surface.config.width,
                            height: ready.surface.config.height,
                            antialiasing_method: vello::AaConfig::Area,
                        };
                        renderer
                            .render_to_texture(
                                &device_handle.device,
                                &device_handle.queue,
                                vello_scene,
                                intermediate_view,
                                &params,
                            )
                            .map_err(|e| {
                                anyhow!("frust-render: vello render_to_texture failed: {e}")
                            })?;
                    }
                    // Blit fallback: render into the intermediate `Rgba8Unorm` target
                    // now; the acquire/blit/present tail (see `submit`) copies it to
                    // the swapchain.
                    RenderPath::Blit { target_view, .. } => {
                        vello_scene.reset();
                        convert::encode_scene_with_shaders(
                            scene,
                            vello_scene,
                            &shader_images,
                            adapter_max,
                        );
                        let params = vello::RenderParams {
                            base_color,
                            width: ready.surface.config.width,
                            height: ready.surface.config.height,
                            antialiasing_method: vello::AaConfig::Area,
                        };
                        renderer
                            .render_to_texture(
                                &device_handle.device,
                                &device_handle.queue,
                                vello_scene,
                                target_view,
                                &params,
                            )
                            .map_err(|e| {
                                anyhow!("frust-render: vello render_to_texture failed: {e}")
                            })?;
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
    /// **submit** (blit + queue-submit + present) attribution — the S5
    /// GPU-saturation-vs-blit-cost question (Phase 11.A) — calls
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

        match action {
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
                Ok(AcquireOutcome::Acquired)
            }
            AcquireAction::Reconfigure => {
                ctx.configure_surface(&ready.surface);
                Ok(AcquireOutcome::Reconfigured)
            }
            AcquireAction::Lose => {
                debug_assert_eq!(
                    next_phase(SurfacePhase::SurfaceReady, SurfaceEvent::Lost),
                    SurfacePhase::SurfaceLost,
                    "Lost must reach SurfaceLost from SurfaceReady (spec §8.1)"
                );
                // Drop the surface and its outstanding resources before returning
                // so the shell can recreate cleanly. `consecutive_invalid`
                // was already reset above (`next_invalid_streak`); the shell's own
                // recovery path (recreate on resize/redraw/surfaceChanged) starts a
                // fresh episode.
                *state = SurfaceState::Lost;
                Ok(AcquireOutcome::Lost)
            }
            AcquireAction::Skip => Ok(AcquireOutcome::Skipped),
        }
    }

    /// Phase 2b of the frame — the **submit** sub-span: turn the swapchain
    /// texture [`Self::acquire`] stashed into a presented frame, then present it.
    /// Timing this call in isolation attributes the submit work separately from
    /// [`Self::acquire`]'s blocking vsync wait (Phase 11.A).
    ///
    /// What the submit span contains depends on the render path (deliverable 5,
    /// the v3 span mapping):
    ///
    /// - **Blit arm** (`Bgra8`-only/probe-refused/`cpu-tier`): the intermediate
    ///   target was already filled in [`Self::encode`], so `submit` = create the
    ///   swapchain view + `TextureBlitter::copy` + queue-submit + present. This is
    ///   the pre-11.C behavior, unchanged.
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
                TierBackend::Gpu(renderer) => {
                    let params = vello::RenderParams {
                        // Set in `encode`; a well-formed frame always encoded first.
                        base_color: pending_base_color.take().unwrap_or(peniko::Color::BLACK),
                        width: surface.config.width,
                        height: surface.config.height,
                        antialiasing_method: vello::AaConfig::Area,
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
            },
            // Direct-premultiplied (defect D3): vello already rendered the
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

/// The shader-showcase pre-pass (Gpu tier only): for every distinct
/// `Command::ShaderQuad` in `scene`, compile its program, render it into an
/// offscreen texture at the quad's physical size, register that texture with
/// vello as an image override, and collect a `(program id, clamped physical
/// size) → ImageData` map for `convert::encode_into` to lower each quad against.
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
/// the miss-everywhere input `convert::encode_scene_with_shaders` already
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

    #[test]
    fn resolved_translucent_is_false_without_a_live_surface() {
        // The Mode A default (review finding M1): with no surface installed
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

    #[test]
    fn destroy_is_idempotent_and_resets_to_no_surface() {
        let mut renderer = SurfaceRenderer::new();
        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
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
    /// ignored the result" — matching this task's "zero GPU work" acceptance
    /// criterion. `disabled` is passed directly rather than going through
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
