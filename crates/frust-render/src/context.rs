//! [`RenderContext`]: owns the wgpu instance and the logical device surfaces
//! render on.
//!
//! Adapter and device creation are frust's own rather than any upstream
//! helper's, because the device request has to name the adapter's *actual*
//! limits: a hardcoded `wgpu::Limits::default()` is what the **iOS
//! Simulator's** macOS-Metal-backed device cannot satisfy (`request_device`
//! fails outright). Owning the request is also what lets the
//! [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057) simulator
//! alignment mitigation apply ([`effective_limits`]).
//!
//! Surface creation lives on the lifecycle state machine in
//! [`crate::SurfaceRenderer`]: the shell mints an empty
//! `SurfaceRenderer` and drives it with `on_surface_created`/`on_surface_changed`/
//! `on_surface_destroyed`, each of which reaches back into this context for the
//! owning device.

use anyhow::{Result, anyhow};
#[cfg(feature = "perf-trace")]
use std::sync::OnceLock;

use crate::renderer::SurfaceAlphaRequest;

/// The `wgpu::Features` this crate opportunistically asks for when the adapter
/// exposes them.
///
/// `PIPELINE_CACHE` alone: it is what
/// [`create_pipeline_cache`](RenderContext::create_pipeline_cache) needs to
/// seed the engine's shader-pipeline compilation from a persisted blob. An
/// optional feature is only ever *added* when the adapter already offers it,
/// so this can never turn a working adapter into a failed device request.
pub(crate) fn optional_device_features() -> wgpu::Features {
    wgpu::Features::PIPELINE_CACHE
}

/// The `wgpu::Features` a `perf-trace` build should additionally ask for,
/// given what `adapter_features` the live adapter actually offers and
/// whether this build compiled the `perf-trace` feature in.
///
/// Mirrors `frust_gpu::context`'s own `required_features` — the identical
/// policy, applied to this crate's own hand-rolled device request rather than
/// `frust-gpu`'s: **empty** by default, and even under `perf_trace`,
/// `TIMESTAMP_QUERY` only when the adapter offers it. A required feature is a
/// hard device-creation failure on an adapter lacking it, so this can never
/// turn a working adapter into no adapter at all — the engine tier's
/// GPU-timestamp ring (`frust_gpu::diag::TimestampRing`) simply stays inert
/// (`gpu_q=0`) on a device that did not get the feature, exactly as it does
/// in a build that did not compile `perf-trace` in. A plain `bool` parameter
/// rather than reading `cfg!` internally, so both branches are unit-testable
/// regardless of which features this crate was compiled with.
fn perf_trace_features(adapter_features: wgpu::Features, perf_trace: bool) -> wgpu::Features {
    if perf_trace && adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY) {
        wgpu::Features::TIMESTAMP_QUERY
    } else {
        wgpu::Features::empty()
    }
}

/// Whether a `FRUST_*` boolean env flag is set to a non-zero value, checking
/// both the compile-time (`option_env!`) and runtime (`std::env::var`) halves —
/// the compile-time-or-runtime parsing every shipping flag (`FRUST_TRACE`,
/// `FRUST_NO_FRAME_GATE`, `FRUST_NO_RENDER_THREAD`, …) uses, so an Android app
/// process (which has no runtime env) still honours a baked-in value.
///
/// `FRUST_TRACE` (via [`perf_tracing_enabled`]) is the only such flag this
/// crate still reads, and only under the `perf-trace` feature — hence the
/// `allow` rather than a `cfg`, which would take the unit tests below with it.
#[cfg_attr(not(feature = "perf-trace"), allow(dead_code))]
fn env_flag_enabled(name_compile_time: Option<&str>, name_runtime: Option<String>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(name_compile_time) || is_set_non_zero(name_runtime.as_deref())
}

/// The precedence-resolved value of a `FRUST_*` string-valued env knob (e.g.
/// `FRUST_AA_MODE`), checking both the compile-time (`option_env!`) and
/// runtime (`std::env::var`) halves like [`env_flag_enabled`], but returning
/// the value itself rather than a bool.
///
/// **Runtime wins**: a non-empty runtime value is returned even when a
/// compile-time value is also set; an empty (`""`) runtime value is treated
/// as unset and falls through to the compile-time half; `None` when neither
/// half carries a non-empty value. Same spirit as `env_flag_enabled`'s
/// compile-time-or-runtime parsing, so an Android app process (no runtime
/// env) still honours a baked-in value.
pub(crate) fn env_str(
    compile_time: Option<&'static str>,
    runtime: Option<String>,
) -> Option<String> {
    fn non_empty(value: Option<String>) -> Option<String> {
        value.filter(|v| !v.is_empty())
    }
    non_empty(runtime).or_else(|| non_empty(compile_time.map(str::to_string)))
}

/// `FRUST_TRACE` flag — mirroring the check in `frust-shell-common::perf`.
/// Cached to avoid repeated environment lookups.
///
/// Only compiled under the `perf-trace` feature — the sole callers,
/// [`probe_direct_to_surface_capability`]/
/// [`log_render_path`], are themselves feature-gated with an inert
/// `#[cfg(not(feature = "perf-trace"))]` counterpart that skips this check
/// entirely, so a build without the feature contains neither this env read
/// nor the probe bodies it guards.
#[cfg(feature = "perf-trace")]
fn perf_tracing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_TRACE"),
            std::env::var("FRUST_TRACE").ok(),
        )
    })
}

/// Which per-frame render path a configured surface uses (see [`RenderPath`]).
///
/// Pure decision output, kept separate from the `wgpu` resources so the
/// selection logic ([`choose_engine_render_path`]) is unit-testable without a
/// GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderPathKind {
    /// The experimental CPU tier's only arm: `vello_cpu` rasterizes into a
    /// pixmap uploaded into an intermediate `Rgba8Unorm` target that is
    /// blitted to the swapchain each frame. Blit-only by construction — a
    /// rasterizer writing its pixels on the CPU has no direct arm to take.
    #[cfg(feature = "cpu-tier")]
    Blit,
    /// `frust-engine` renders into the acquired swapchain texture, which needs
    /// only `RENDER_ATTACHMENT` in whatever format the surface reports — no
    /// `Rgba8Unorm` requirement, no `STORAGE_BINDING`, no intermediate and no
    /// blit (ordinary render passes, no compute storage write), plus a
    /// depth attachment the surface carries alongside it
    /// ([`RenderPath::EngineDirect`]). Chosen by
    /// [`choose_engine_render_path`] for the
    /// [`Engine`](crate::RenderTier::Engine) tier alone.
    #[cfg(feature = "engine-tier")]
    EngineDirect,
    /// `frust-engine` renders into an intermediate of the surface's own and a
    /// full-screen fragment pass un-premultiplies it into the acquired
    /// swapchain texture ([`RenderPath::EngineDirectUnpremultiply`]) — the
    /// engine arm for a swapchain that stores STRAIGHT alpha and whose
    /// compositor genuinely reads it that way, which the engine's
    /// premultiplied output cannot be handed directly. `PostMultiplied` on
    /// `wgpu::Backend::Metal` (iOS's translucent mode included) is the one
    /// exception: [`compositor_expects_premultiplied`] answers true for it —
    /// an upstream wgpu-hal truth bug — so it takes [`EngineDirect`] instead,
    /// skipping this conversion entirely.
    ///
    /// [`EngineDirect`]: RenderPathKind::EngineDirect
    ///
    /// Still the engine family and still `RENDER_ATTACHMENT`-only on both
    /// textures: the conversion is an ordinary render pass into the swapchain
    /// (never a compute/storage write), so nothing about this arm needs a
    /// capability the tier refuses.
    #[cfg(feature = "engine-tier")]
    EngineDirectUnpremultiply,
}

/// The engine tier's render path for a surface on `backend` whose resolved
/// alpha mode is `alpha_mode` — one of the two engine arms.
///
/// The engine targets an ordinary render attachment in the surface's own
/// format, so no format/usage probe participates; what splits the two arms
/// apart is the engine's output convention against the swapchain's:
///
/// - `Opaque`/`Auto` ignore alpha entirely — [`RenderPathKind::EngineDirect`].
/// - `Inherit` (Android's translucent mode) and `PreMultiplied` expect
///   PREMULTIPLIED alpha, which is what the engine's strip pipelines already
///   write (the premultiplied convention every frust GPU path uses). Served
///   as-is on the same arm: no conversion, which would darken every
///   partial-alpha pixel by a second factor of `a`.
/// - `PostMultiplied` expects STRAIGHT alpha and would read `(C·a, a)` as
///   `(C, a)` — every partial-alpha pixel too dark. It takes
///   [`RenderPathKind::EngineDirectUnpremultiply`], where the frame lands in
///   an intermediate first and one fragment pass converts it on the way to
///   the swapchain — **except** on `wgpu::Backend::Metal`, where
///   [`compositor_expects_premultiplied`] answers true: the backend's own
///   `PostMultiplied` swapchain composites premultiplied regardless of its
///   name (an upstream wgpu-hal truth bug, documented on that predicate), so
///   handing it the straight-alpha conversion would double-correct. That
///   combination alone takes [`RenderPathKind::EngineDirect`] too — the same
///   arm `Inherit`/`PreMultiplied` take, and for the identical reason: the
///   engine's premultiplied output already matches what the compositor reads.
///   This also answers iOS, whose sole translucent mode is a Metal
///   `PostMultiplied` surface, removing the conversion pass's per-frame cost
///   there entirely.
///
/// Every resolved (backend, alpha mode) pair therefore has an engine arm:
/// there is no unserveable answer here and no translucency for this tier to
/// refuse.
///
/// Pure and (backend, mode)-only, so it is host-testable without a surface.
#[cfg(feature = "engine-tier")]
pub(crate) fn choose_engine_render_path(
    backend: wgpu::Backend,
    alpha_mode: wgpu::CompositeAlphaMode,
) -> RenderPathKind {
    if alpha_mode_is_straight_translucent(alpha_mode)
        && !compositor_expects_premultiplied(backend, alpha_mode)
    {
        RenderPathKind::EngineDirectUnpremultiply
    } else {
        RenderPathKind::EngineDirect
    }
}

/// Whether THIS build actually contains `tier`'s renderer, i.e. whether the
/// cargo feature that compiles it in is on.
///
/// The guard [`RenderContext::create_device`] fails fast with, so a
/// [`crate::SurfaceRenderer`] never receives a tier it has no encode path for.
/// Exhaustive on purpose: a tier added later cannot compile until it has
/// stated its own answer here.
pub(crate) fn tier_compiled_in(tier: crate::tier::RenderTier) -> bool {
    match tier {
        crate::tier::RenderTier::Cpu => cfg!(feature = "cpu-tier"),
        crate::tier::RenderTier::Engine => cfg!(feature = "engine-tier"),
    }
}

/// Emits the one-per-process startup line naming the chosen render path and the
/// reason, mirroring the surface-alpha line's style and its `FRUST_TRACE`
/// gating. Logged once regardless of surface recreation.
///
/// Only compiled under the `perf-trace` feature; see the inert
/// `#[cfg(not(feature = "perf-trace"))]` counterpart
/// below.
#[cfg(feature = "perf-trace")]
fn log_render_path(path: RenderPathKind) {
    static LOGGED: OnceLock<()> = OnceLock::new();

    if !perf_tracing_enabled() {
        return;
    }

    LOGGED.get_or_init(|| {
        let (name, reason) = match path {
            #[cfg(feature = "cpu-tier")]
            RenderPathKind::Blit => (
                "blit",
                "vello_cpu's pixmap uploaded into an intermediate and blitted to the swapchain \
                 (cpu-tier)",
            ),
            #[cfg(feature = "engine-tier")]
            RenderPathKind::EngineDirect => (
                "engine-direct",
                "frust-engine into the acquired swapchain view (RENDER_ATTACHMENT, \
                 surface-reported format, surface-owned depth)",
            ),
            #[cfg(feature = "engine-tier")]
            RenderPathKind::EngineDirectUnpremultiply => (
                "engine-direct-unpremultiply",
                "frust-engine into a surface-owned intermediate, un-premultiplied into the \
                 swapchain by one fragment pass (straight-alpha translucent surface)",
            ),
        };
        log::info!("frust-perf render-path {name} ({reason})");
    });
}

/// Without the `perf-trace` feature, the render-path startup line is a
/// complete no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_render_path(_path: RenderPathKind) {}

/// Resolves a caller's [`SurfaceAlphaRequest`] against the live surface's
/// reported `alpha_modes`, choosing the actual `wgpu::CompositeAlphaMode` to
/// configure with. Kept crate-private and
/// `wgpu`-typed: `SurfaceAlphaRequest` is the public, `wgpu`-free seam; the
/// resolved mode itself never crosses `frust-render`'s boundary (the
/// `DetachedSurface` opacity precedent — `docs/CODE_STANDARDS.md`'s
/// wgpu-leak anti-pattern).
///
/// `Opaque` reproduces today's behavior bit-for-bit (`Auto`, unchanged for
/// every existing caller). `TranslucentPreferred` tries, in order, `Inherit`
/// (Android's only reported translucent mode), `PostMultiplied`
/// (iOS's translucent mode), then `PreMultiplied` — falling back to `Auto`
/// with a `log::warn!` when none of the three is in `capabilities.alpha_modes`
/// (translucency silently unavailable on that surface).
fn resolve_alpha_mode(
    request: SurfaceAlphaRequest,
    capabilities: &wgpu::SurfaceCapabilities,
) -> wgpu::CompositeAlphaMode {
    match request {
        SurfaceAlphaRequest::Opaque => wgpu::CompositeAlphaMode::Auto,
        SurfaceAlphaRequest::TranslucentPreferred => {
            const PREFERRED: [wgpu::CompositeAlphaMode; 3] = [
                wgpu::CompositeAlphaMode::Inherit,
                wgpu::CompositeAlphaMode::PostMultiplied,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ];
            PREFERRED
                .into_iter()
                .find(|mode| capabilities.alpha_modes.contains(mode))
                .unwrap_or_else(|| {
                    log::warn!(
                        "frust-render: translucency requested but unavailable \
                         (alpha_modes={:?}) — falling back to Auto (opaque)",
                        capabilities.alpha_modes
                    );
                    wgpu::CompositeAlphaMode::Auto
                })
        }
    }
}

/// Whether a **resolved** `wgpu::CompositeAlphaMode` actually composites the
/// surface's alpha against what is behind it — i.e. whether the surface really
/// came up translucent ("Mode B"), as opposed to what the caller *requested*.
///
/// This is the wgpu-free truth behind [`ConfiguredSurface::resolved_translucent`]
/// and, through it,
/// [`SurfaceRenderer::surface_resolved_translucent`](crate::SurfaceRenderer::surface_resolved_translucent):
/// [`resolve_alpha_mode`] can silently degrade a
/// [`SurfaceAlphaRequest::TranslucentPreferred`] to `Auto` when the platform
/// advertises no translucent mode, and a shell that kept keying its paint
/// contract off the *request* would then clear to `TRANSPARENT` and punch its
/// platform-view slots via `DestOut` against an OPAQUE swapchain — presenting
/// black rectangles. Keying off this instead degrades to
/// the Mode A contract (opaque base, no punch).
///
/// The three translucent modes are exactly [`resolve_alpha_mode`]'s preference
/// list — `Inherit` (Android), `PostMultiplied` (iOS), `PreMultiplied`;
/// `Opaque`/`Auto` ignore the surface's alpha entirely and are therefore *not*
/// translucent (`Auto` is what every opaque caller and every fallback resolves
/// to).
fn alpha_mode_is_translucent(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::Inherit
            | wgpu::CompositeAlphaMode::PostMultiplied
            | wgpu::CompositeAlphaMode::PreMultiplied
    )
}

/// Whether pixels presented to a swapchain configured with this composite
/// alpha mode must be **premultiplied** before present.
///
/// The two premultiplied-expecting modes are `PreMultiplied` and — the shipped
/// Android translucent case — `Inherit`: on Android the only reported
/// translucent mode is `Inherit`, under which SurfaceFlinger blends a
/// `TRANSLUCENT` SurfaceView premultiplied — measured on device: a 50%-alpha
/// `#FFF176` reaching SurfaceFlinger stored straight `#FFF176@128` instead of
/// premultiplied `#807B3B@128` composites over-bright over a Mode B hole.
///
/// `PostMultiplied` (iOS's translucent mode) expects STRAIGHT alpha, and
/// `Opaque`/`Auto` ignore alpha entirely. The engine's strip pipelines write
/// premultiplied, so this predicate is read the other way round now — through
/// [`alpha_mode_is_straight_translucent`], which is what selects the engine's
/// un-premultiplying arm.
#[cfg(feature = "engine-tier")]
fn alpha_mode_needs_premultiply(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::Inherit
    )
}

/// Whether a **resolved** alpha mode means "translucent, and the swapchain
/// stores STRAIGHT alpha" — the combination the engine tier has to convert
/// its premultiplied output for.
///
/// True for `PostMultiplied` alone: it is translucent
/// ([`alpha_mode_is_translucent`]) and expects straight alpha
/// ([`alpha_mode_needs_premultiply`] is false), which is iOS's translucent
/// mode. `Inherit`/`PreMultiplied` are translucent but premultiplied, so they
/// take [`RenderPathKind::EngineDirect`] as-is; `Opaque`/`Auto` ignore alpha
/// entirely.
///
/// Pure and mode-only, so the decision is host-testable without a surface.
#[cfg(feature = "engine-tier")]
fn alpha_mode_is_straight_translucent(mode: wgpu::CompositeAlphaMode) -> bool {
    alpha_mode_is_translucent(mode) && !alpha_mode_needs_premultiply(mode)
}

/// Whether `backend`'s compositor actually composites `mode` premultiplied,
/// **despite** [`alpha_mode_is_straight_translucent`] answering true for it —
/// an upstream wgpu-hal truth bug specific to one backend/mode pair, not a
/// property of the mode alone.
///
/// True for `wgpu::Backend::Metal` with `CompositeAlphaMode::PostMultiplied`
/// alone. wgpu-hal's Metal adapter advertises `PostMultiplied`
/// (`wgpu-hal-29.0.4/src/metal/adapter.rs:422-425`,
/// `composite_alpha_modes: [Opaque, PostMultiplied]`) but its surface
/// configuration implements the mode as nothing beyond
/// `render_layer.setOpaque(false)` (`wgpu-hal-29.0.4/src/metal/surface.rs:81-85`)
/// — it never asks Core Animation to treat the layer's content as straight
/// alpha, and Core Animation has no such mode: a `CAMetalLayer` ONLY
/// composites premultiplied (MoltenVK's own equivalent exposes
/// `OPAQUE | PRE_MULTIPLIED` for the identical layer, never a straight
/// option). So a Metal `PostMultiplied` swapchain reads back exactly like a
/// premultiplied one — `display = C·a + BG·(1−a)` — even though the mode's
/// name and wgpu's advertised contract say straight. Handing it the
/// spec-correct straight-alpha conversion
/// ([`RenderPathKind::EngineDirectUnpremultiply`]) therefore double-corrects:
/// the frame is un-premultiplied for a compositor that was going to
/// premultiply-composite it anyway, over-brightening every partial-alpha
/// pixel — device-visible only at fractional alpha (an indigo/navy wash,
/// black frames near a translucent split).
///
/// Every other backend keeps the ordinary reading: a genuinely-straight
/// `PostMultiplied` compositor exists on at least one other backend (e.g.
/// Vulkan's own `POST_MULTIPLIED` composite-alpha flag), so this predicate is
/// Metal-specific rather than blanket-disbelieving the mode everywhere.
///
/// Pure and (backend, mode)-only, so the decision is host-testable without a
/// live adapter.
#[cfg(feature = "engine-tier")]
fn compositor_expects_premultiplied(
    backend: wgpu::Backend,
    mode: wgpu::CompositeAlphaMode,
) -> bool {
    backend == wgpu::Backend::Metal && mode == wgpu::CompositeAlphaMode::PostMultiplied
}

/// Whether a `width` x `height` target fits the device's own
/// `max_texture_dimension_2d`.
///
/// The engine arms are the callers: the swapchain is a plain
/// `RENDER_ATTACHMENT` texture paired with a depth attachment of the identical
/// extent ([`RenderPath::EngineDirect`]) and, on the un-premultiplying arm, an
/// intermediate of that extent as well — so an over-ceiling surface would be
/// two or three texture creations wgpu refuses, a validation error
/// mid-configure rather than a diagnosis. Asked once, up front, against the
/// limits the device was actually requested with ([`effective_limits`], which
/// is what `wgpu::Device::limits` reports back), so the surface is refused with
/// a message naming the ceiling instead.
///
/// Pure and host-testable. Compiled with the engine arm alone, which is its
/// only caller.
#[cfg(feature = "engine-tier")]
pub(crate) fn extent_within_limits(width: u32, height: u32, max_dimension_2d: u32) -> bool {
    width <= max_dimension_2d && height <= max_dimension_2d
}

/// Permanent surface-caps + chosen-alpha-mode line, logged once
/// per surface configure (not once per process — a resize/recreate that picks
/// a different mode is worth a fresh line, unlike the render-path line above).
/// Gated exactly like [`log_render_path`] so a release build (no `perf-trace`
/// feature) stays string-free.
#[cfg(feature = "perf-trace")]
fn log_surface_alpha_caps(
    capabilities: &wgpu::SurfaceCapabilities,
    chosen: wgpu::CompositeAlphaMode,
) {
    if !perf_tracing_enabled() {
        return;
    }
    log::info!(
        "frust-render surface-caps: alpha_modes={:?} chosen={:?}",
        capabilities.alpha_modes,
        chosen
    );
}

/// Without the `perf-trace` feature, the surface-caps/alpha line is a
/// complete no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_surface_alpha_caps(
    _capabilities: &wgpu::SurfaceCapabilities,
    _chosen: wgpu::CompositeAlphaMode,
) {
}

/// A logical device plus the adapter it came from and the queue that executes
/// its command buffers.
pub(crate) struct DeviceHandle {
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
}

/// Owns the wgpu `Instance` and the single logical device the renderer uses.
///
/// A single `RenderContext` is shared across every surface a shell creates
/// (frust is single-window); the device is created lazily on the first
/// surface and reused across surface loss/recreation (rotation, backgrounding)
/// since a logical device is display-independent. It is passed into the
/// [`SurfaceRenderer`](crate::SurfaceRenderer) lifecycle and per-frame methods
/// so those operations reach the owning device.
pub struct RenderContext {
    pub(crate) instance: wgpu::Instance,
    /// Lazily created on the first surface; `None` until then.
    pub(crate) device: Option<DeviceHandle>,
    /// The tier [`ensure_device`](Self::ensure_device) selected for the live
    /// device. Defaults to [`RenderTier::Engine`](crate::RenderTier::Engine)
    /// and is only ever [`RenderTier::Cpu`](crate::RenderTier::Cpu) in a
    /// `cpu-tier`-feature build whose probe (or override) chose the CPU
    /// fallback — the [`SurfaceRenderer`](crate::SurfaceRenderer) reads it to
    /// pick the encode path.
    pub(crate) selected_tier: crate::tier::RenderTier,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A cheap, cloneable handle to a [`RenderContext`]'s wgpu `Instance`, used to
/// create a surface on a *different* thread than the one that owns the context
/// (the render-thread split).
///
/// # Why this exists
///
/// wgpu `Surface` creation reads the platform window handle, which several
/// windowing backends (notably winit on macOS/AppKit) only make available on
/// the main/UI thread. The render-thread split therefore cannot create the
/// surface where the renderer lives; instead the UI thread creates a
/// [`DetachedSurface`] via this factory and hands it across to the render
/// thread, which installs it with
/// [`SurfaceRenderer::on_surface_installed`](crate::SurfaceRenderer::on_surface_installed).
/// A `wgpu::Instance` is `Send + Sync + Clone` (Arc-backed) and a `Surface` it
/// produces stays compatible with any adapter/device the cloned instance
/// requests, so the two threads share one instance with no `unsafe`.
///
/// The mobile shells are unaffected: they receive a platform-created surface
/// pointer (`ANativeWindow`/`CAMetalLayer`) and keep using
/// [`SurfaceRenderer::on_surface_created_from_android_window`](crate::SurfaceRenderer::on_surface_created_from_android_window)
/// / `on_surface_created_from_metal_layer`.
#[derive(Clone)]
pub struct SurfaceFactory {
    instance: wgpu::Instance,
}

impl SurfaceFactory {
    /// Create a [`DetachedSurface`] from a window handle **on the calling
    /// thread** — call this on the thread the windowing backend requires
    /// (the main/UI thread for winit). The returned surface is `Send` and may
    /// then be moved to the render thread for
    /// [`install`](crate::SurfaceRenderer::on_surface_installed).
    ///
    /// This performs *only* the window-handle-dependent step (surface creation);
    /// the device, swapchain configuration, and blitter are all built later, on
    /// the installing thread, so nothing here touches the GPU device.
    pub fn create_detached_surface(
        &self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<DetachedSurface> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|e| anyhow!("frust-render: failed to create surface: {e}"))?;
        Ok(DetachedSurface { surface })
    }
}

/// A created-but-not-yet-installed wgpu `Surface`, produced by
/// [`SurfaceFactory::create_detached_surface`] on the windowing thread and
/// installed on the render thread via
/// [`SurfaceRenderer::on_surface_installed`](crate::SurfaceRenderer::on_surface_installed).
/// Opaque so the `wgpu` type stays confined to this crate; a
/// shell only moves it across a thread boundary. `Send` (a `wgpu::Surface` is
/// `Send + Sync`), which is the whole point.
pub struct DetachedSurface {
    surface: wgpu::Surface<'static>,
}

impl DetachedSurface {
    /// Consume the wrapper, yielding the raw surface for installation. Crate-
    /// private so the `wgpu` type never escapes `frust-render`.
    pub(crate) fn into_surface(self) -> wgpu::Surface<'static> {
        self.surface
    }
}

/// The per-frame render path a [`ConfiguredSurface`] carries — the resources
/// specific to the chosen arm (see [`RenderPathKind`]).
///
/// The engine arms render into the acquired swapchain view (plus, on the
/// un-premultiplying one, a surface-owned intermediate); the `cpu-tier`
/// [`Blit`](Self::Blit) arm owns the intermediate `Rgba8Unorm` target the CPU
/// rasterizer uploads into plus the `TextureBlitter` that copies it to the
/// swapchain each frame.
pub(crate) enum RenderPath {
    /// The `cpu-tier` arm: `vello_cpu` uploads its rasterized pixmap into
    /// `target_texture` (`COPY_DST`, see [`create_targets`]), then `blitter`
    /// draws `target_view` over the acquired swapchain texture each frame.
    /// Always the surface's own size — this tier's rasterizer encodes at
    /// identity into a pixmap of exactly the swapchain's dimensions.
    #[cfg(feature = "cpu-tier")]
    Blit {
        target_texture: wgpu::Texture,
        target_view: wgpu::TextureView,
        blitter: wgpu::util::TextureBlitter,
    },
    /// The engine tier's arm ([`RenderPathKind::EngineDirect`]): `frust-engine`
    /// records its passes straight into the acquired swapchain view, so there
    /// is no intermediate and no blitter — but the frame needs a depth
    /// attachment matching the target extent, and this is what owns it.
    ///
    /// Owned HERE rather than left to the engine's own lazily allocated
    /// attachment because the surface is what knows when the extent changed:
    /// [`RenderContext::resize_surface`] recreates it in the same step that
    /// reconfigures the swapchain, keeping the reallocation off the frame
    /// path, and the pairing can never disagree with the colour attachment it
    /// is attached beside.
    #[cfg(feature = "engine-tier")]
    EngineDirect {
        /// The `Depth24Plus` attachment this surface's frames test against,
        /// sized to the swapchain and recreated with it. Handed to the engine
        /// as `EngineTarget::depth`; the engine clears it each frame (it is
        /// not pre-cleared — nothing else writes it).
        depth: frust_engine::DepthTexture,
    },
    /// The engine tier's arm for a **straight-alpha translucent** swapchain
    /// (`PostMultiplied`, iOS's translucent mode —
    /// [`alpha_mode_is_straight_translucent`]): the engine renders the frame
    /// into `intermediate_view` and `present` un-premultiplies it into the
    /// acquired swapchain texture, `(rgb / max(a, ε), a)` in one full-screen
    /// fragment pass recorded into the frame's own encoder.
    ///
    /// The conversion is a *render* pass, not a compute dispatch: this tier
    /// runs where compute does not, and its swapchain carries no
    /// `STORAGE_BINDING` to write through.
    ///
    /// The intermediate is `Rgba8Unorm` (`frust_engine`'s own off-screen
    /// format, which is what the renderer's pipelines are warmed for on this
    /// arm) with `RENDER_ATTACHMENT | TEXTURE_BINDING`: a colour attachment for
    /// the frame, a sampled source for the pass. Only its view is stored — a
    /// `TextureView` refcounts its texture alive and nothing here needs the
    /// texture handle.
    #[cfg(feature = "engine-tier")]
    EngineDirectUnpremultiply {
        /// The depth attachment, exactly as [`Self::EngineDirect`] carries it —
        /// sized to the swapchain and recreated with it, since the intermediate
        /// the frame targets has the swapchain's own extent.
        depth: frust_engine::DepthTexture,
        /// The premultiplied intermediate the engine renders the frame into,
        /// recreated on resize like the blit arm's target.
        intermediate_view: wgpu::TextureView,
        /// The conversion, built once per surface configure for the
        /// swapchain's own format.
        present: frust_engine::gpu::present::UnpremultiplyPass,
    },
}

/// A configured swapchain surface plus the render-path resources for the arm it
/// was configured for. Crate-private; the wrapped `wgpu` types never escape
/// `frust-render`.
pub(crate) struct ConfiguredSurface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) path: RenderPath,
    /// Whether this surface **actually** came up translucent — the wgpu-free
    /// projection of `config.alpha_mode` through [`alpha_mode_is_translucent`],
    /// computed once at configure time (the mode never changes for a live
    /// surface; a resize reconfigures with the same `config`). No arm refuses
    /// translucency any more: every resolved alpha mode has an engine arm
    /// ([`choose_engine_render_path`]), straight-alpha translucency included,
    /// and `cpu-tier`'s `vello_cpu` output is already premultiplied.
    ///
    /// Stored as a plain `bool` rather than re-derived from `config.alpha_mode`
    /// at each read so the value a shell observes through
    /// [`SurfaceRenderer::surface_resolved_translucent`](crate::SurfaceRenderer::surface_resolved_translucent)
    /// crosses this crate's boundary with no `wgpu` type in the signature
    /// (`docs/CODE_STANDARDS.md`'s wgpu-leak anti-pattern).
    pub(crate) resolved_translucent: bool,
}

/// Every swapchain format [`RenderContext::create_render_surface`] can
/// configure a surface with: whichever of the two the platform reports first.
/// The engine builds its strip pipelines for the format it is handed, so
/// neither is privileged.
pub(crate) const SURFACE_FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Bgra8Unorm,
];

/// Given the build-config-derived instance flags and whether the process is
/// currently running on an Android emulator, decides the flags wgpu's
/// `Instance` should actually be created with.
///
/// Pure decision logic, kept separate from the platform property lookup in
/// [`is_android_emulator`] so it can be unit-tested on any host without an
/// Android target.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn effective_instance_flags(flags: wgpu::InstanceFlags, is_emulator: bool) -> wgpu::InstanceFlags {
    if is_emulator {
        flags - (wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION)
    } else {
        flags
    }
}

/// Given a base `wgpu::Limits` and whether the process is currently running
/// on an iOS Simulator, decides the `Limits` a device request should
/// actually use.
///
/// Mitigates [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057): the
/// iOS Simulator is macOS-Metal-backed and requires 256-byte
/// `min_uniform_buffer_offset_alignment`, but wgpu 29's Metal backend reports
/// the (lower) iOS-device value, which trips Metal API validation on the
/// simulator. Physical iOS devices are unaffected and pass `base` through
/// unchanged; a `base` whose alignment is already `>= 256` is left alone
/// (never lowered).
///
/// Pure decision logic, mirroring [`effective_instance_flags`]'s split of
/// pure decision vs. platform lookup. It is fed the *adapter's* real limits (so
/// the device request never over-asks and fails on the simulator) and, on the
/// simulator, has its uniform-buffer alignment forced up to 256 — see
/// [`RenderContext::ensure_device`], the live call site.
pub(crate) fn effective_limits(base: wgpu::Limits, is_ios_simulator: bool) -> wgpu::Limits {
    const IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT: u32 = 256;
    if is_ios_simulator
        && base.min_uniform_buffer_offset_alignment
            < IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT
    {
        wgpu::Limits {
            min_uniform_buffer_offset_alignment: IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT,
            ..base
        }
    } else {
        base
    }
}

/// Whether this binary is running on the iOS Simulator (`aarch64-apple-ios-sim`
/// / `x86_64-apple-ios` under the simulator), which sets `target_abi = "sim"`.
/// Compile-time constant: the simulator mitigation only needs to apply to
/// simulator builds, never physical-device or desktop ones.
pub(crate) const fn is_ios_simulator() -> bool {
    cfg!(all(target_os = "ios", target_abi = "sim"))
}

/// Number of uncaptured `wgpu` errors logged at error level per device before
/// the handler latches into suppression. A single flaky frame under a driver
/// hiccup (e.g. the Android emulator's SwiftShader path — see the
/// `on_uncaptured_error` doc below) is expected to surface a handful of
/// errors; past this the process is either wedged in a genuine per-frame error
/// storm or the driver is fundamentally broken, and re-logging every single
/// one would flood the log without adding information.
const MAX_LOGGED_UNCAPTURED_ERRORS: u32 = 5;

/// How often (in error count) a latched handler bumps a debug-level "still
/// happening" line once past [`MAX_LOGGED_UNCAPTURED_ERRORS`] and the one
/// suppression notice. Debug level (not error) because this is diagnostic
/// noise for someone actively investigating, not an actionable signal.
const UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD: u32 = 100;

/// What the `on_uncaptured_error` handler should do for the `count`-th
/// uncaptured error (1-indexed) it has observed on a given device.
///
/// Pure decision logic, split out of the handler closure in [`RenderContext::ensure_device`]
/// so the latch discipline — log the first few, announce the latch once, then
/// go quiet except an occasional debug bump — is unit-testable without a GPU
/// or a real `wgpu::Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogAction {
    /// One of the first [`MAX_LOGGED_UNCAPTURED_ERRORS`]: log the error itself
    /// at error level.
    Log,
    /// The first error past the cap: log one suppression notice (naming the
    /// running total) instead of the error itself.
    SuppressionNotice,
    /// Past the cap and past the suppression notice: stay silent, except a
    /// periodic debug-level count bump when `debug_bump` is set.
    Silent { debug_bump: bool },
}

/// Pure latch policy for the uncaptured-error handler (see [`LogAction`]).
pub(crate) fn decide_log_action(count: u32) -> LogAction {
    if count <= MAX_LOGGED_UNCAPTURED_ERRORS {
        LogAction::Log
    } else if count == MAX_LOGGED_UNCAPTURED_ERRORS + 1 {
        LogAction::SuppressionNotice
    } else {
        LogAction::Silent {
            debug_bump: count.is_multiple_of(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD),
        }
    }
}

/// Detects whether the current process is running on an Android emulator
/// (goldfish/ranchu), as opposed to a physical device, via the standard
/// `ro.kernel.qemu` system property (`"1"` on emulators, unset/absent on
/// real hardware). A failed property read (`None`) is treated as "not an
/// emulator" so physical devices — and any environment where the property
/// can't be read — default to keeping validation on.
#[cfg(target_os = "android")]
fn is_android_emulator() -> bool {
    android_system_properties::AndroidSystemProperties::new()
        .get("ro.kernel.qemu")
        .as_deref()
        == Some("1")
}

/// Creates the intermediate render target the `cpu-tier` rasterizer uploads
/// into, and the blit pass then copies to the acquired surface texture.
#[cfg(feature = "cpu-tier")]
fn create_targets(
    width: u32,
    height: u32,
    device: &wgpu::Device,
) -> (wgpu::Texture, wgpu::TextureView) {
    // Exactly the two usages this arm needs: `COPY_DST` for the
    // `write_texture` upload of `vello_cpu`'s rasterized pixmap, and
    // `TEXTURE_BINDING` for the blitter's sampled full-screen draw.
    // `Rgba8Unorm` is renderable and sampleable on every wgpu backend.
    let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    let target_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-render cpu-tier target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage,
        format: wgpu::TextureFormat::Rgba8Unorm,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (target_texture, target_view)
}

/// Creates the intermediate the engine renders a frame into on
/// [`RenderPath::EngineDirectUnpremultiply`], returning its view alone (the
/// view refcounts the texture alive, and nothing on this arm needs the texture
/// handle).
///
/// The usage is exactly what the two consumers need — `RENDER_ATTACHMENT` for
/// the frame's own passes, `TEXTURE_BINDING` for the un-premultiplying pass to
/// sample it — so the arm asks a downlevel adapter for nothing it lacks (this
/// tier has neither a compute stage nor a storage texture anywhere in it; its
/// downlevel design rules refuse both).
///
/// The format is the engine's own off-screen format rather than the surface's:
/// the renderer on this arm is warmed for it (`renderer::engine_target_format`),
/// and only the final pass has to speak the swapchain's format.
#[cfg(feature = "engine-tier")]
fn create_engine_intermediate(width: u32, height: u32, device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-render engine intermediate"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        format: frust_engine::gpu::pipelines::INTERMEDIATE_FORMAT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl RenderContext {
    /// Creates a context with a fresh wgpu `Instance` and no device yet
    /// (the device is created lazily on first surface creation).
    ///
    /// When actually running on an Android *emulator* the instance setup strips the
    /// `DEBUG`/`VALIDATION` instance flags (which
    /// `InstanceFlags::from_build_config()` turns on in debug builds). The
    /// `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set
    /// object-name labels via `vkSetDebugUtilsObjectNameEXT`, and the
    /// emulator's gfxstream Vulkan HAL (`vulkan.ranchu.so`) segfaults inside
    /// that entry point during adapter enumeration — the same class of
    /// debug-utils fragility a MoltenVK Vulkan backend is also known to have
    /// (observed crash: `#00 vulkan.ranchu.so
    /// vk_common_SetDebugUtilsObjectNameEXT`). Debug object labels are only a
    /// developer convenience, so dropping them on the emulator is a safe way
    /// to keep GPU bring-up alive there while leaving physical devices'
    /// validation safety net — and desktop behavior — untouched.
    pub fn new() -> Self {
        let backends = wgpu::Backends::from_env().unwrap_or_default();
        let build_flags = wgpu::InstanceFlags::from_build_config().with_env();
        #[cfg(target_os = "android")]
        let flags = effective_instance_flags(build_flags, is_android_emulator());
        #[cfg(not(target_os = "android"))]
        let flags = build_flags;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends,
            flags,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        Self {
            instance,
            device: None,
            selected_tier: crate::tier::RenderTier::default(),
        }
    }

    /// A cloneable [`SurfaceFactory`] sharing this context's wgpu `Instance`,
    /// for creating a [`DetachedSurface`] on the windowing/main thread when the
    /// context itself lives on the render thread (the
    /// render-thread split). The surface a clone produces stays compatible with
    /// the device this context creates, since both share one Arc-backed
    /// instance.
    pub fn surface_factory(&self) -> SurfaceFactory {
        SurfaceFactory {
            instance: self.instance.clone(),
        }
    }

    /// The render tier the live device was created for (see
    /// [`selected_tier`](Self::selected_tier) field). The default tier
    /// (`Engine`) until a surface — and thus a device — has been created.
    pub(crate) fn selected_tier(&self) -> crate::tier::RenderTier {
        self.selected_tier
    }

    /// The single logical device, panicking if no surface has created it yet.
    ///
    /// Only called from [`crate::SurfaceRenderer`]'s install/resize/render
    /// paths, all of which run strictly after a `create_*_surface`, so the
    /// device is always present.
    pub(crate) fn device_handle(&self) -> &DeviceHandle {
        self.device
            .as_ref()
            .expect("device must be created before it is used (surface creation creates it)")
    }

    /// Whether the live device was created with `wgpu::Features::PIPELINE_CACHE`.
    ///
    /// wgpu 29 only implements the persisted pipeline cache on Vulkan — every
    /// Vulkan adapter advertises it (Android, Linux, Windows-on-Vulkan);
    /// Metal and DX12 adapters never do, so it is absent there
    /// and [`create_pipeline_cache`](Self::create_pipeline_cache) returns `None`
    /// — the renderer then behaves exactly as it did before this path existed.
    /// Panics if no surface (and thus no device) has been created yet.
    pub(crate) fn pipeline_cache_supported(&self) -> bool {
        self.device_handle()
            .device
            .features()
            .contains(wgpu::Features::PIPELINE_CACHE)
    }

    /// The adapter fingerprint a persisted pipeline-cache blob is tagged with
    /// (see [`crate::pipeline_cache`]). Panics if no device has been created yet.
    pub(crate) fn adapter_cache_key(&self) -> String {
        crate::pipeline_cache::adapter_cache_key(&self.device_handle().adapter.get_info())
    }

    /// Creates a `wgpu::PipelineCache` for the live device, seeded from a
    /// previously persisted, framed `blob` when it validates for this adapter.
    ///
    /// Returns `None` when the device lacks `PIPELINE_CACHE` support (Metal/
    /// desktop) — the renderer then runs its original, cache-less path. A `blob`
    /// that fails framing/adapter validation
    /// ([`crate::pipeline_cache::unframe`]) is discarded and the cache starts
    /// empty; a `None` `blob` is a cold start.
    ///
    /// # Safety
    ///
    /// This is the sole sanctioned unsafe site in this module. The wgpu contract
    /// on [`wgpu::Device::create_pipeline_cache`] is that a non-`None` `data`
    /// must have come from a prior `PipelineCache::get_data()` on a
    /// `pipeline_cache_key`-compatible adapter. We uphold it two ways: (1) the
    /// caller-supplied blob is `unframe`d against *this* adapter's fingerprint
    /// **before** the unsafe call, so foreign or post-driver-update data never
    /// reaches it; (2) `fallback: true` makes wgpu fall back to an empty cache
    /// for any data it still rejects internally, rather than misbehaving.
    /// Corrupt/mismatched data is therefore a silently-ignored cache miss, never
    /// undefined behaviour.
    pub(crate) fn create_pipeline_cache(&self, blob: Option<&[u8]>) -> Option<wgpu::PipelineCache> {
        if !self.pipeline_cache_supported() {
            return None;
        }
        let handle = self.device_handle();
        let key = crate::pipeline_cache::adapter_cache_key(&handle.adapter.get_info());
        let data = blob.and_then(|b| crate::pipeline_cache::unframe(b, &key));
        log::debug!(
            "frust-render: creating wgpu PipelineCache (seed: {})",
            if data.is_some() {
                "persisted blob"
            } else {
                "empty"
            }
        );
        // SAFETY: see this method's `# Safety` section — `data` has already been
        // validated against this adapter's fingerprint by `unframe`, and
        // `fallback: true` turns any residual internal mismatch into a
        // fall-back-to-empty cache rather than UB.
        let cache = unsafe {
            handle
                .device
                .create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                    label: Some("frust-render pipeline cache"),
                    data,
                    fallback: true,
                })
        };
        Some(cache)
    }

    /// Lazily create the logical device compatible with `surface`, requesting
    /// the adapter's own limits (never `Limits::default()`, which the iOS
    /// Simulator cannot satisfy) plus the #7057 alignment mitigation. Reuses
    /// an already-created device when it is compatible with `surface`, so
    /// surface loss/recreation (rotation, backgrounding) never rebuilds it — and
    /// so a device the [`ensure_device_headless`](Self::ensure_device_headless)
    /// pre-init created before any surface existed is adopted here rather than
    /// rebuilt.
    async fn ensure_device(&mut self, surface: &wgpu::Surface<'static>) -> Result<()> {
        if let Some(existing) = &self.device
            && existing.adapter.is_surface_supported(surface)
        {
            return Ok(());
        }
        self.create_device(Some(surface)).await
    }

    /// Create the logical device **before any surface exists**, so the wgpu
    /// instance/adapter/device bring-up can run on a
    /// background thread kicked at native-library load (`JNI_OnLoad`) and be
    /// joined by `nativeInit` instead of running serially after `surfaceCreated`.
    /// Idempotent: a no-op when a device already exists.
    ///
    /// # Android singular-adapter assumption
    ///
    /// Requesting the adapter with `compatible_surface: None` picks wgpu's
    /// default adapter rather than one filtered to a specific surface. On Android
    /// the Vulkan backend exposes a single physical device, so the adapter chosen
    /// here is the same one a later surface-filtered request would pick, and
    /// [`ensure_device`](Self::ensure_device)'s `is_surface_supported` reuse check
    /// accepts it — the surface created at `nativeInit` reuses this device with no
    /// rebuild. On a hypothetical multi-adapter device where the pre-init adapter
    /// did *not* support the eventual surface, `ensure_device` simply rebuilds the
    /// device against that surface (still correct, just without the overlap win).
    /// This is the sole caller that passes `None` below; desktop/iOS create their
    /// device through the surface path and never invoke this.
    pub async fn ensure_device_headless(&mut self) -> Result<()> {
        if self.device.is_some() {
            return Ok(());
        }
        self.create_device(None).await
    }

    /// Shared device-creation body for both the surface-bound
    /// ([`ensure_device`](Self::ensure_device)) and pre-surface
    /// ([`ensure_device_headless`](Self::ensure_device_headless)) paths.
    ///
    /// `compatible_surface` filters adapter selection to one that can present to
    /// the given surface; `None` (the pre-init path) selects wgpu's default
    /// adapter (see the singular-adapter note on `ensure_device_headless`). The
    /// tier probe, limits mitigation, uncaptured-error handler, and device
    /// request are identical either way — the surface only ever affected adapter
    /// selection, never the device it yields.
    async fn create_device(
        &mut self,
        compatible_surface: Option<&wgpu::Surface<'static>>,
    ) -> Result<()> {
        let adapter =
            wgpu::util::initialize_adapter_from_env_or_default(&self.instance, compatible_surface)
                .await
                .map_err(|e| anyhow!("frust-render: no compatible GPU adapter: {e}"))?;

        // Consult the tier probe rather than a bespoke downlevel check, so a
        // refused adapter surfaces through the one diagnostic path
        // `select_render_tier` owns (shared with its own unit tests). An
        // explicit override (`FRUST_RENDER_TIER`, or `frust run --render-tier`
        // setting it for the spawned process) wins *among available tiers*.
        // The engine tier requires no downlevel flag at all, so in a default
        // build this always resolves `Available(Engine)`; the `Unavailable`
        // arm below is reached only by a build that compiled no renderer in.
        let downlevel = adapter.get_downlevel_capabilities();
        let caps = crate::tier::TierCaps {
            downlevel_flags: downlevel.flags,
            adapter_name: adapter.get_info().name,
        };
        let override_tier = crate::tier::render_tier_override_from_env();
        let selection = crate::tier::select_render_tier(&caps, override_tier);
        let tier = match selection.outcome {
            crate::tier::TierOutcome::Available(tier) => tier,
            crate::tier::TierOutcome::Unavailable { .. } => {
                return Err(anyhow!(selection.diagnosis));
            }
        };
        // A tier is only reachable in a build that compiled its renderer in;
        // guard against a stray selection so the SurfaceRenderer never sees a
        // tier it has no encode path for. Asked per tier
        // ([`tier_compiled_in`]).
        if !tier_compiled_in(tier) {
            return Err(anyhow!(selection.diagnosis));
        }
        self.selected_tier = tier;

        let required_features = (adapter.features() & optional_device_features())
            | perf_trace_features(adapter.features(), cfg!(feature = "perf-trace"));
        let required_limits = effective_limits(adapter.limits(), is_ios_simulator());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-render device"),
                required_features,
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("frust-render: failed to create GPU device: {e}"))?;

        // Route wgpu's uncaptured errors to the log instead of its default
        // handler, which aborts the process by panicking ("Handling wgpu errors
        // as fatal by default"). A UI framework must survive a driver's
        // *transient* GPU error and recover on a later frame rather than crash —
        // e.g. the Android emulator's SwiftShader path can raise a one-off
        // swapchain-acquire validation error under load, which used to wedge the
        // app into a per-frame panic loop (the surface stayed `SurfaceReady` and
        // every subsequent `render` re-hit the fatal handler). Pairing this with
        // the `Invalid`-acquire → reconfigure recovery (see [`crate::lifecycle`])
        // lets the swapchain rebuild and rendering resume. Genuine API misuse is
        // still surfaced — loudly, at error level — just without killing the
        // process across the FFI boundary.
        //
        // The handler latches rather than logging unbounded: a device stuck in a
        // genuine per-frame error storm (as opposed to a one-off driver hiccup)
        // would otherwise flood the log forever. `error_count` is per-device
        // (captured fresh each time this closure is installed, i.e. once per
        // logical device), `Arc<AtomicU32>` because `on_uncaptured_error`'s
        // handler must be `Fn`, not `FnMut` — see [`decide_log_action`] for the
        // pure latch policy this defers to.
        let error_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        device.on_uncaptured_error(std::sync::Arc::new(move |error| {
            let count = error_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            match decide_log_action(count) {
                LogAction::Log => {
                    log::error!("frust-render: uncaptured wgpu error: {error}");
                }
                LogAction::SuppressionNotice => {
                    log::error!(
                        "frust-render: further uncaptured wgpu errors suppressed \
                         (total so far: {count})"
                    );
                }
                LogAction::Silent { debug_bump } => {
                    if debug_bump {
                        log::debug!(
                            "frust-render: uncaptured wgpu error count now {count} \
                             (still suppressed)"
                        );
                    }
                }
            }
        }));

        self.device = Some(DeviceHandle {
            adapter,
            device,
            queue,
        });
        Ok(())
    }

    /// Builds a configured [`ConfiguredSurface`] from a raw wgpu `Surface`,
    /// creating the logical device if needed.
    pub(crate) async fn create_render_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        alpha: SurfaceAlphaRequest,
    ) -> Result<ConfiguredSurface> {
        self.ensure_device(&surface).await?;
        // The tier probe already ran in `ensure_device`, so `selected_tier()`
        // is authoritative here.
        let tier = self.selected_tier();
        let handle = self.device_handle();

        let capabilities = surface.get_capabilities(&handle.adapter);
        // Resolved BEFORE the path decision: the engine arm's path answer is a
        // function of the resolved alpha mode ([`choose_engine_render_path`]).
        // The mode itself is derived from the request and the surface's
        // reported caps alone.
        let alpha_mode = resolve_alpha_mode(alpha, &capabilities);
        // Each tier decides its own path. The engine reads the adapter's
        // backend here rather than inside `choose_engine_render_path` itself,
        // so that function stays a pure value decision, host-testable with no
        // live adapter ([`compositor_expects_premultiplied`]); the `cpu-tier`
        // rasterizer is blit-only by construction and has nothing to decide.
        //
        // An arm is unreachable in a build that did not compile its renderer
        // in — `create_device`'s [`tier_compiled_in`] guard refuses such a tier
        // before any surface reaches here — but each still answers rather than
        // panicking, so a wiring bug is reported.
        let path_kind = match tier {
            #[cfg(feature = "engine-tier")]
            crate::tier::RenderTier::Engine => {
                choose_engine_render_path(handle.adapter.get_info().backend, alpha_mode)
            }
            #[cfg(not(feature = "engine-tier"))]
            crate::tier::RenderTier::Engine => {
                return Err(anyhow!(
                    "frust-render: the engine tier was selected without the `engine-tier` \
                     feature compiled in"
                ));
            }
            #[cfg(feature = "cpu-tier")]
            crate::tier::RenderTier::Cpu => RenderPathKind::Blit,
            #[cfg(not(feature = "cpu-tier"))]
            crate::tier::RenderTier::Cpu => {
                return Err(anyhow!(
                    "frust-render: the CPU tier was selected without the `cpu-tier` feature \
                     compiled in"
                ));
            }
        };
        log_render_path(path_kind);
        log_surface_alpha_caps(&capabilities, alpha_mode);

        // Every arm draws into the swapchain through an ordinary render pass —
        // the engine's own passes, or the blit arm's sampled full-screen draw —
        // so `RENDER_ATTACHMENT` is the whole usage set, in whichever supported
        // format the surface reports first.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|it| SURFACE_FORMATS.contains(it))
            .ok_or_else(|| anyhow!("frust-render: no supported surface format (Rgba8/Bgra8)"))?;
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT;

        // The extent is checked once, up front, for every texture sized against
        // it below (the engine's depth attachment, and its intermediate on the
        // un-premultiplying arm): an over-ceiling surface is refused with a
        // diagnosis naming the limit, rather than handed to wgpu as texture
        // creations it rejects ([`extent_within_limits`]).
        #[cfg(feature = "engine-tier")]
        {
            let max_dimension_2d = handle.device.limits().max_texture_dimension_2d;
            if !extent_within_limits(width, height, max_dimension_2d) {
                return Err(anyhow!(
                    "frust-render: engine tier refuses a {width}x{height} surface — the device's \
                     max_texture_dimension_2d is {max_dimension_2d}, and both the swapchain and \
                     the depth attachment paired with it are sized from this extent"
                ));
            }
        }

        let config = wgpu::SurfaceConfiguration {
            usage,
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        let path = match path_kind {
            // The `cpu-tier` arm: the intermediate the rasterizer uploads into,
            // plus the blitter that copies it over the acquired swapchain
            // texture each frame. Always the surface's own size.
            #[cfg(feature = "cpu-tier")]
            RenderPathKind::Blit => {
                let (target_texture, target_view) = create_targets(width, height, &handle.device);
                RenderPath::Blit {
                    target_texture,
                    target_view,
                    blitter: wgpu::util::TextureBlitter::new(&handle.device, format),
                }
            }
            // The engine's direct arm carries exactly one per-surface resource:
            // the depth attachment its opaque pass establishes and its alpha
            // pass tests against, sized to the swapchain that was just
            // configured (the extent was checked against the device ceiling
            // above).
            //
            // `Inherit`/`PreMultiplied` land here correct and untouched, since
            // `frust-engine` already writes premultiplied. A Metal
            // `PostMultiplied` swapchain (iOS included) lands here too, for the
            // identical reason: `compositor_expects_premultiplied` answers true
            // for it, since Metal's own compositor reads that mode's swapchain
            // premultiplied regardless of its name (upstream wgpu-hal truth bug
            // — see that predicate's doc).
            #[cfg(feature = "engine-tier")]
            RenderPathKind::EngineDirect => RenderPath::EngineDirect {
                depth: frust_engine::DepthTexture::new(&handle.device, width, height),
            },
            // The opposite combination — a swapchain that stores STRAIGHT
            // alpha AND whose compositor genuinely reads it that way (every
            // `PostMultiplied` backend except Metal, `choose_engine_render_path`'s
            // one exception) — takes the same depth attachment plus the two
            // resources the conversion needs: the intermediate the frame is
            // rendered into, and the pass that un-premultiplies it into the
            // swapchain, built for the swapchain's own format.
            //
            // No persisted driver cache is threaded into that build: a
            // `wgpu::PipelineCache` exists on Vulkan alone, and this arm never
            // serves a Metal surface (Metal's `PostMultiplied` takes the arm
            // above instead) — the one pipeline it compiles is paid once per
            // surface configure either way.
            #[cfg(feature = "engine-tier")]
            RenderPathKind::EngineDirectUnpremultiply => RenderPath::EngineDirectUnpremultiply {
                depth: frust_engine::DepthTexture::new(&handle.device, width, height),
                intermediate_view: create_engine_intermediate(width, height, &handle.device),
                present: frust_engine::gpu::present::UnpremultiplyPass::new(
                    &handle.device,
                    format,
                    None,
                ),
            },
        };
        // The RESOLVED translucency, not the request: a `TranslucentPreferred`
        // that fell back to `Auto` above lands here as `false`, which is what
        // the shells' paint contract keys off (see `alpha_mode_is_translucent`).
        // No arm refuses translucency any more — the engine serves every
        // resolved mode, and `vello_cpu`'s output is already premultiplied.
        let resolved_translucent = alpha_mode_is_translucent(alpha_mode);
        let configured = ConfiguredSurface {
            surface,
            config,
            path,
            resolved_translucent,
        };
        self.configure_surface(&configured);
        Ok(configured)
    }

    /// Builds a [`ConfiguredSurface`] from anything convertible into a wgpu
    /// `SurfaceTarget` (the desktop `winit` window path).
    pub(crate) async fn create_surface(
        &mut self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        alpha: SurfaceAlphaRequest,
    ) -> Result<ConfiguredSurface> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|e| anyhow!("frust-render: failed to create surface: {e}"))?;
        self.create_render_surface(surface, width, height, present_mode, alpha)
            .await
    }

    /// (Re)configures the swapchain for `surface`'s current config.
    pub(crate) fn configure_surface(&self, surface: &ConfiguredSurface) {
        surface
            .surface
            .configure(&self.device_handle().device, &surface.config);
    }

    /// Resizes `surface` in place and reconfigures the swapchain. The
    /// `cpu-tier` blit arm recreates its intermediate target texture at the new
    /// size, and each engine arm recreates the depth attachment (plus, on the
    /// un-premultiplying arm, the intermediate) that has to match the
    /// swapchain's extent. Zero dimensions are rejected upstream.
    pub(crate) fn resize_surface(&self, surface: &mut ConfiguredSurface, width: u32, height: u32) {
        surface.config.width = width;
        surface.config.height = height;
        match &mut surface.path {
            #[cfg(feature = "cpu-tier")]
            RenderPath::Blit {
                target_texture,
                target_view,
                ..
            } => {
                let (new_texture, new_view) =
                    create_targets(width, height, &self.device_handle().device);
                *target_texture = new_texture;
                *target_view = new_view;
            }
            // A depth attachment must match its colour attachment's extent
            // exactly, so the resize recreates it here — in the same step that
            // reconfigures the swapchain, keeping the allocation off the frame
            // path. Skipped when the extent is unchanged
            // (`DepthTexture::matches`), which is what a reconfigure with the
            // same size costs on the other arm too.
            //
            // An over-ceiling extent keeps the existing attachment rather than
            // creating one wgpu would refuse: the swapchain reconfigure below
            // is what fails on such a size, and a frame encoded against a
            // mismatched depth attachment is refused by the engine rather than
            // submitted ([`extent_within_limits`], the same ceiling
            // `create_render_surface` refuses on).
            #[cfg(feature = "engine-tier")]
            RenderPath::EngineDirect { depth } => {
                let device = &self.device_handle().device;
                let max_dimension_2d = device.limits().max_texture_dimension_2d;
                if !extent_within_limits(width, height, max_dimension_2d) {
                    log::warn!(
                        "frust-render: engine tier cannot resize to {width}x{height} — the \
                         device's max_texture_dimension_2d is {max_dimension_2d}; keeping the \
                         previous depth attachment"
                    );
                } else if !depth.matches(width, height) {
                    *depth = frust_engine::DepthTexture::new(device, width, height);
                }
            }
            // The un-premultiplying arm recreates its intermediate alongside
            // that same depth attachment, both under the same ceiling: the
            // frame renders into the intermediate and the conversion writes the
            // swapchain, so all three have to agree on one extent. The pass
            // itself survives — it holds a pipeline built for the surface's
            // format, which a resize never changes, and its bind group names
            // the source view per frame rather than at build time.
            #[cfg(feature = "engine-tier")]
            RenderPath::EngineDirectUnpremultiply {
                depth,
                intermediate_view,
                ..
            } => {
                let device = &self.device_handle().device;
                let max_dimension_2d = device.limits().max_texture_dimension_2d;
                if !extent_within_limits(width, height, max_dimension_2d) {
                    log::warn!(
                        "frust-render: engine tier cannot resize to {width}x{height} — the \
                         device's max_texture_dimension_2d is {max_dimension_2d}; keeping the \
                         previous depth attachment and intermediate"
                    );
                } else {
                    if !depth.matches(width, height) {
                        *depth = frust_engine::DepthTexture::new(device, width, height);
                    }
                    *intermediate_view = create_engine_intermediate(width, height, device);
                }
            }
        }
        self.configure_surface(surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_unset_both_halves_is_disabled() {
        // Neither the compile-time (`option_env!`) nor runtime (`std::env::var`)
        // half is set — every `FRUST_NO_*`/`FRUST_TRACE` flag's default state.
        assert!(!env_flag_enabled(None, None));
    }

    #[test]
    fn env_flag_compile_time_non_zero_enables() {
        assert!(env_flag_enabled(Some("1"), None));
    }

    #[test]
    fn env_flag_compile_time_zero_is_disabled() {
        // "0" is the explicit opt-out spelling (mirrors `FRUST_TRACE`'s doc).
        assert!(!env_flag_enabled(Some("0"), None));
    }

    #[test]
    fn env_flag_runtime_non_zero_enables() {
        assert!(env_flag_enabled(None, Some("1".to_string())));
    }

    #[test]
    fn env_flag_runtime_zero_is_disabled() {
        assert!(!env_flag_enabled(None, Some("0".to_string())));
    }

    #[test]
    fn env_flag_either_half_set_non_zero_enables() {
        // Compile-time-or-runtime: an Android app process (no runtime env) still
        // honours a baked-in compile-time value, and vice versa.
        assert!(env_flag_enabled(Some("1"), Some("0".to_string())));
        assert!(env_flag_enabled(Some("0"), Some("1".to_string())));
    }

    #[test]
    fn perf_trace_features_off_asks_for_nothing() {
        assert_eq!(
            perf_trace_features(wgpu::Features::TIMESTAMP_QUERY, false),
            wgpu::Features::empty()
        );
    }

    #[test]
    fn perf_trace_features_on_with_the_adapter_offering_it_asks_for_timestamp_query() {
        assert_eq!(
            perf_trace_features(wgpu::Features::TIMESTAMP_QUERY, true),
            wgpu::Features::TIMESTAMP_QUERY
        );
    }

    #[test]
    fn perf_trace_features_on_without_the_adapter_offering_it_asks_for_nothing() {
        // Never asks for a feature the adapter does not have, even under
        // `perf_trace` — a required feature the adapter lacks is a hard
        // device-creation failure.
        assert_eq!(
            perf_trace_features(wgpu::Features::empty(), true),
            wgpu::Features::empty()
        );
    }

    #[test]
    fn env_str_unset_both_halves_is_none() {
        assert_eq!(env_str(None, None), None);
    }

    #[test]
    fn env_str_runtime_non_empty_beats_compile_time() {
        // Runtime wins even when a compile-time value is also present.
        assert_eq!(
            env_str(Some("msaa8"), Some("msaa16".to_string())),
            Some("msaa16".to_string())
        );
    }

    #[test]
    fn env_str_empty_runtime_falls_through_to_compile_time() {
        // An empty runtime value ("") is treated as unset, not a real override.
        assert_eq!(
            env_str(Some("msaa8"), Some(String::new())),
            Some("msaa8".to_string())
        );
    }

    #[test]
    fn env_str_compile_time_only() {
        assert_eq!(env_str(Some("msaa16"), None), Some("msaa16".to_string()));
    }

    #[test]
    fn env_str_runtime_only() {
        assert_eq!(
            env_str(None, Some("area".to_string())),
            Some("area".to_string())
        );
    }

    #[test]
    fn a_tier_is_only_selectable_in_a_build_that_compiled_it() {
        // Each tier tracks its own feature — nothing inherits a blanket
        // permission, and nothing inherits a blanket refusal.
        assert_eq!(
            tier_compiled_in(crate::tier::RenderTier::Cpu),
            cfg!(feature = "cpu-tier")
        );
        assert_eq!(
            tier_compiled_in(crate::tier::RenderTier::Engine),
            cfg!(feature = "engine-tier")
        );
    }

    /// Every composite alpha mode a surface can resolve to, so a routing claim
    /// is made across the whole space rather than the modes it was written for.
    #[cfg(feature = "engine-tier")]
    const EVERY_ALPHA_MODE: [wgpu::CompositeAlphaMode; 5] = [
        wgpu::CompositeAlphaMode::Auto,
        wgpu::CompositeAlphaMode::Opaque,
        wgpu::CompositeAlphaMode::Inherit,
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
    ];

    #[cfg(feature = "engine-tier")]
    #[test]
    fn every_alpha_mode_routes_to_an_engine_arm() {
        // The completeness claim: this tier serves the whole space, with no
        // (backend, mode) pair left unserveable and no translucency refused.
        // A mode added to `wgpu` later would land on one of the two arms
        // rather than on a refusal, and this is where a wrong answer for it
        // shows up.
        for backend in wgpu::Backend::ALL {
            for mode in EVERY_ALPHA_MODE {
                let kind = choose_engine_render_path(backend, mode);
                assert!(
                    matches!(
                        kind,
                        RenderPathKind::EngineDirect | RenderPathKind::EngineDirectUnpremultiply
                    ),
                    "{backend:?}/{mode:?} routed to {kind:?}, which is not an engine arm"
                );
            }
        }
    }

    #[cfg(feature = "engine-tier")]
    #[test]
    fn engine_serves_opaque_and_premultiplied_modes_directly() {
        // Premultiplied output meets a premultiplied-expecting swapchain, and
        // an opaque one ignores alpha entirely: all four are served straight
        // into the swapchain, with no conversion pass in between — on every
        // backend, since `compositor_expects_premultiplied` only ever fires
        // for `PostMultiplied`.
        for backend in wgpu::Backend::ALL {
            for mode in [
                wgpu::CompositeAlphaMode::Auto,
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::Inherit,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ] {
                assert_eq!(
                    choose_engine_render_path(backend, mode),
                    RenderPathKind::EngineDirect,
                    "{mode:?} on {backend:?} took a conversion it does not need"
                );
            }
        }
    }

    #[cfg(feature = "engine-tier")]
    #[test]
    fn straight_alpha_translucency_routes_through_the_unpremultiply_arm() {
        // Off Metal, a straight-alpha translucent mode (`PostMultiplied`
        // alone) disagrees with the engine's output convention exactly as
        // advertised, and is the only mode that does.
        for backend in wgpu::Backend::ALL {
            if backend == wgpu::Backend::Metal {
                continue;
            }
            for mode in EVERY_ALPHA_MODE {
                assert_eq!(
                    choose_engine_render_path(backend, mode)
                        == RenderPathKind::EngineDirectUnpremultiply,
                    alpha_mode_is_straight_translucent(mode),
                    "the conversion arm must be exactly the straight-alpha translucent modes on \
                     {backend:?}, not {mode:?}"
                );
            }
        }
    }

    #[cfg(feature = "engine-tier")]
    #[test]
    fn metal_post_multiplied_skips_the_unpremultiply_arm() {
        // The upstream wgpu-hal truth bug this predicate corrects for: Metal's
        // `PostMultiplied` composites premultiplied despite advertising
        // straight alpha (wgpu-hal-29.0.4's metal/adapter.rs advertises the
        // mode, metal/surface.rs implements it as nothing beyond
        // `setOpaque(false)`, and `CAMetalLayer` only ever composites
        // premultiplied), so handing it the straight-alpha conversion would
        // double-correct. It must take the same arm as `Inherit`/
        // `PreMultiplied` instead — this also answers iOS, whose sole
        // translucent mode is Metal `PostMultiplied`.
        assert_eq!(
            choose_engine_render_path(
                wgpu::Backend::Metal,
                wgpu::CompositeAlphaMode::PostMultiplied
            ),
            RenderPathKind::EngineDirect,
        );
        // Every other Metal mode is unaffected by the exception — same answer
        // as any other backend.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ] {
            assert_eq!(
                choose_engine_render_path(wgpu::Backend::Metal, mode),
                RenderPathKind::EngineDirect,
                "{mode:?} on Metal took a conversion it does not need"
            );
        }
    }

    #[cfg(feature = "engine-tier")]
    #[test]
    fn non_metal_post_multiplied_keeps_the_unpremultiply_arm() {
        // Genuinely-straight `PostMultiplied` compositors exist off Metal
        // (e.g. Vulkan's own `POST_MULTIPLIED` composite-alpha flag), so the
        // Metal exception above must not spread to any other backend.
        for backend in wgpu::Backend::ALL {
            if backend == wgpu::Backend::Metal {
                continue;
            }
            assert_eq!(
                choose_engine_render_path(backend, wgpu::CompositeAlphaMode::PostMultiplied),
                RenderPathKind::EngineDirectUnpremultiply,
                "{backend:?} PostMultiplied must keep the unpremultiply conversion"
            );
        }
    }

    #[cfg(feature = "engine-tier")]
    #[test]
    fn compositor_expects_premultiplied_is_metal_post_multiplied_only() {
        // Direct guard on the predicate itself, across the whole (backend,
        // mode) space — the two routing tests above exercise it only through
        // `choose_engine_render_path`.
        for backend in wgpu::Backend::ALL {
            for mode in EVERY_ALPHA_MODE {
                let expected = backend == wgpu::Backend::Metal
                    && mode == wgpu::CompositeAlphaMode::PostMultiplied;
                assert_eq!(
                    compositor_expects_premultiplied(backend, mode),
                    expected,
                    "compositor_expects_premultiplied({backend:?}, {mode:?})"
                );
            }
        }
    }

    #[cfg(feature = "engine-tier")]
    #[test]
    fn extent_limits_refuse_only_an_over_ceiling_axis() {
        assert!(extent_within_limits(4096, 4096, 4096));
        assert!(extent_within_limits(1, 1, 4096));
        // Either axis alone is enough to refuse.
        assert!(!extent_within_limits(4097, 4096, 4096));
        assert!(!extent_within_limits(4096, 4097, 4096));
    }

    #[test]
    fn emulator_strips_debug_and_validation() {
        let build_flags = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        let flags = effective_instance_flags(build_flags, true);
        assert!(!flags.contains(wgpu::InstanceFlags::DEBUG));
        assert!(!flags.contains(wgpu::InstanceFlags::VALIDATION));
    }

    #[test]
    fn physical_device_keeps_debug_and_validation() {
        let build_flags = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        let flags = effective_instance_flags(build_flags, false);
        assert!(flags.contains(wgpu::InstanceFlags::DEBUG));
        assert!(flags.contains(wgpu::InstanceFlags::VALIDATION));
    }

    #[test]
    fn emulator_with_no_debug_flags_stays_empty() {
        let flags = effective_instance_flags(wgpu::InstanceFlags::empty(), true);
        assert!(flags.is_empty());
    }

    #[test]
    fn ios_simulator_bumps_alignment_to_256() {
        let base = wgpu::Limits::default();
        let limits = effective_limits(base.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        // Nothing else about the base limits should change.
        assert_eq!(
            wgpu::Limits {
                min_uniform_buffer_offset_alignment: base.min_uniform_buffer_offset_alignment,
                ..limits.clone()
            },
            base
        );
    }

    #[test]
    fn non_simulator_leaves_limits_untouched() {
        let base = wgpu::Limits::default();
        let limits = effective_limits(base.clone(), false);
        assert_eq!(limits, base);
    }

    #[test]
    fn base_already_at_or_above_256_is_not_lowered() {
        let base = wgpu::Limits {
            min_uniform_buffer_offset_alignment: 512,
            ..wgpu::Limits::default()
        };
        let limits = effective_limits(base.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 512);
        assert_eq!(limits, base);
    }

    #[test]
    fn simulator_alignment_uses_real_adapter_limits_not_defaults() {
        // A low-alignment adapter (the #7057 shape: Metal reports a lower
        // alignment than the simulator driver actually enforces) is bumped to
        // 256 while every other adapter-reported limit is preserved — the
        // whole point of feeding *adapter* limits rather than `Limits::default`.
        let adapter = wgpu::Limits {
            min_uniform_buffer_offset_alignment: 64,
            max_texture_dimension_2d: 4096,
            ..wgpu::Limits::default()
        };
        let limits = effective_limits(adapter.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        assert_eq!(limits.max_texture_dimension_2d, 4096);
    }

    #[test]
    fn first_n_uncaptured_errors_log() {
        for count in 1..=MAX_LOGGED_UNCAPTURED_ERRORS {
            assert_eq!(
                decide_log_action(count),
                LogAction::Log,
                "expected Log at count={count}"
            );
        }
    }

    #[test]
    fn nplus1_uncaptured_error_suppresses() {
        assert_eq!(
            decide_log_action(MAX_LOGGED_UNCAPTURED_ERRORS + 1),
            LogAction::SuppressionNotice
        );
    }

    #[test]
    fn further_uncaptured_errors_stay_silent_between_debug_bumps() {
        let past_notice = MAX_LOGGED_UNCAPTURED_ERRORS + 2;
        assert_eq!(
            decide_log_action(past_notice),
            LogAction::Silent { debug_bump: false }
        );
    }

    #[test]
    fn uncaptured_error_debug_bump_is_periodic() {
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD),
            LogAction::Silent { debug_bump: true }
        );
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD * 2),
            LogAction::Silent { debug_bump: true }
        );
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD + 1),
            LogAction::Silent { debug_bump: false }
        );
    }

    /// Builds a synthetic `wgpu::SurfaceCapabilities` reporting only the given
    /// `alpha_modes` — the rest of the struct is irrelevant to
    /// `resolve_alpha_mode`, which reads only that one field.
    fn caps_with_alpha_modes(
        alpha_modes: &[wgpu::CompositeAlphaMode],
    ) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            alpha_modes: alpha_modes.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn opaque_request_always_resolves_to_auto() {
        // `Opaque` never consults `alpha_modes` — bit-for-bit today's behavior
        // regardless of what the surface reports.
        for modes in [
            [wgpu::CompositeAlphaMode::Inherit].as_slice(),
            &[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
            &[wgpu::CompositeAlphaMode::Opaque],
        ] {
            let caps = caps_with_alpha_modes(modes);
            assert_eq!(
                resolve_alpha_mode(SurfaceAlphaRequest::Opaque, &caps),
                wgpu::CompositeAlphaMode::Auto
            );
        }
    }

    #[test]
    fn translucent_preferred_picks_inherit_first() {
        // Android's observed shape: `Inherit` is the only reported mode.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Inherit
        );
    }

    #[test]
    fn translucent_preferred_picks_post_multiplied_when_inherit_absent() {
        // iOS's observed shape: `[Opaque, PostMultiplied]` — no `Inherit`.
        let caps = caps_with_alpha_modes(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::PostMultiplied
        );
    }

    #[test]
    fn translucent_preferred_falls_back_to_auto_when_none_available() {
        // A surface reporting only `Opaque` (no `Inherit`/`PostMultiplied`/
        // `PreMultiplied`) can't satisfy translucency — fall back to `Auto`
        // rather than erroring.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Opaque]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Auto
        );
    }

    #[test]
    fn translucent_preferred_falls_back_to_pre_multiplied_last() {
        // Neither `Inherit` nor `PostMultiplied` present, but `PreMultiplied`
        // is — the third preference in the resolution order.
        let caps = caps_with_alpha_modes(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::PreMultiplied
        );
    }

    #[test]
    fn resolved_translucency_is_true_only_for_the_three_translucent_modes() {
        // The wgpu-free projection the shells' paint contract keys off:
        // exactly `resolve_alpha_mode`'s preference list.
        for mode in [
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ] {
            assert!(alpha_mode_is_translucent(mode), "{mode:?}");
        }
        // `Auto` is what BOTH an opaque request and a failed translucent
        // resolution land on — neither composites alpha.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
        ] {
            assert!(!alpha_mode_is_translucent(mode), "{mode:?}");
        }
    }

    /// The straight-alpha translucency case, over every mode
    /// `resolve_alpha_mode` can produce: `PostMultiplied` alone is translucent
    /// AND straight-alpha, so it is the only one the engine tier converts for.
    #[test]
    fn only_post_multiplied_is_translucent_with_straight_alpha() {
        assert!(alpha_mode_is_straight_translucent(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
        // Translucent but premultiplied: `DirectPremultiplied` on the vello
        // arm, and the engine's own premultiplied output served as-is.
        for mode in [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ] {
            assert!(
                alpha_mode_needs_premultiply(mode),
                "{mode:?} must take the premultiply pass"
            );
            assert!(!alpha_mode_is_straight_translucent(mode), "{mode:?}");
        }
        // Not translucent at all: the opaque destination the straight blend is
        // exact for.
        for mode in [
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!alpha_mode_is_straight_translucent(mode), "{mode:?}");
        }
    }

    #[test]
    fn forced_mismatch_translucent_request_resolves_not_translucent() {
        // A surface whose
        // advertised capabilities carry NO translucent mode, asked for
        // translucency. The request is honored as far as it can be (`Auto`),
        // but the RESOLVED translucency — the value
        // `SurfaceRenderer::surface_resolved_translucent` hands the shells — is
        // `false`, so the shells keep the opaque (Mode A) paint contract: an
        // opaque base color and no `ClearRect` punch (the encode-level half of
        // this claim lives in `convert.rs`'s
        // `mode_a_scene_encodes_no_clear_rect_even_with_a_slot_sized_region`).
        for modes in [
            [wgpu::CompositeAlphaMode::Opaque].as_slice(),
            &[wgpu::CompositeAlphaMode::Auto],
            &[],
        ] {
            let caps = caps_with_alpha_modes(modes);
            let resolved = resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps);
            assert_eq!(resolved, wgpu::CompositeAlphaMode::Auto, "{modes:?}");
            assert!(
                !alpha_mode_is_translucent(resolved),
                "a fallback-to-opaque surface must never report translucent ({modes:?})"
            );
        }
    }

    #[test]
    fn happy_path_translucent_request_resolves_translucent() {
        // The shipped configs stay unchanged: Android (`Inherit`-only) and iOS
        // (`[Opaque, PostMultiplied]`) both resolve to a translucent mode, so
        // the punch + transparent base keep running exactly as today.
        for modes in [
            [wgpu::CompositeAlphaMode::Inherit].as_slice(),
            &[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
        ] {
            let resolved = resolve_alpha_mode(
                SurfaceAlphaRequest::TranslucentPreferred,
                &caps_with_alpha_modes(modes),
            );
            assert!(alpha_mode_is_translucent(resolved), "{modes:?}");
        }
        // An opaque request never reports translucent, whatever the surface
        // advertises.
        assert!(!alpha_mode_is_translucent(resolve_alpha_mode(
            SurfaceAlphaRequest::Opaque,
            &caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]),
        )));
    }
}
