//! [`RenderContext`]: owns the wgpu instance and the logical device surfaces
//! render on.
//!
//! Historically this was a thin wrapper over `vello::util::RenderContext`.
//! That type creates its per-surface device internally with a hardcoded
//! `wgpu::Limits::default()` (its `new_device` is private and takes no
//! `required_limits` hook), which the **iOS Simulator's** macOS-Metal-backed
//! device cannot satisfy — `request_device` fails and vello surfaces it as
//! `NoCompatibleDevice` ("Couldn't find suitable device"). We therefore own
//! adapter/device creation ourselves (requesting the adapter's *actual* limits
//! plus the [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057)
//! simulator alignment mitigation) and reuse vello only for the surface
//! plumbing (`vello::util::RenderSurface`, the blitter, and the `Renderer`),
//! which all accept a plain `wgpu::Device`.
//!
//! Surface creation lives on the lifecycle state machine in
//! [`crate::SurfaceRenderer`] (spec §8.1): the shell mints an empty
//! `SurfaceRenderer` and drives it with `on_surface_created`/`on_surface_changed`/
//! `on_surface_destroyed`, each of which reaches back into this context for the
//! owning device.

use anyhow::{Result, anyhow};
use std::sync::OnceLock;
use wgpu::util::TextureBlitter;

use crate::renderer::SurfaceAlphaRequest;

/// The features vello's renderer opportunistically uses when the adapter
/// exposes them — mirrors `vello::util::RenderContext::new_device` so our
/// hand-rolled device request stays behaviourally identical to vello's, apart
/// from the `required_limits` we control.
fn vello_optional_features() -> wgpu::Features {
    wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE
}

/// Whether a `FRUST_*` boolean env flag is set to a non-zero value, checking
/// both the compile-time (`option_env!`) and runtime (`std::env::var`) halves —
/// the compile-time-or-runtime parsing every shipping flag (`FRUST_TRACE`,
/// `FRUST_NO_FRAME_GATE`, `FRUST_NO_RENDER_THREAD`, …) uses, so an Android app
/// process (which has no runtime env) still honours a baked-in value.
fn env_flag_enabled(name_compile_time: Option<&str>, name_runtime: Option<String>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(name_compile_time) || is_set_non_zero(name_runtime.as_deref())
}

/// Whether perf tracing (frust-perf logging) is enabled via the process-wide
/// `FRUST_TRACE` flag — mirroring the check in `frust-shell-common::perf`.
/// Cached to avoid repeated environment lookups.
///
/// Only compiled under the `perf-trace` feature (release-lean plan, task
/// 02) — the sole callers, [`probe_direct_to_surface_capability`]/
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

/// Whether the direct-to-surface render path is force-disabled via the
/// process-wide `FRUST_NO_DIRECT_SURFACE` flag — the fallback-proof safety valve
/// (deliverable 4) that pins a capable device onto the blit arm so the two arms
/// can be A/B'd on the same hardware. Same compile-time-or-runtime parsing as
/// `FRUST_TRACE`/`FRUST_NO_RENDER_THREAD` (see `docs/DEVELOPMENT.md`'s
/// Instrumentation table). Cached: read once per process.
fn direct_surface_force_blit() -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    *FORCED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_NO_DIRECT_SURFACE"),
            std::env::var("FRUST_NO_DIRECT_SURFACE").ok(),
        )
    })
}

/// Which per-frame render path a configured surface uses (see [`RenderPath`]).
///
/// Pure decision output, kept separate from the `wgpu` resources so the
/// selection logic ([`choose_render_path`]) is unit-testable without a GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderPathKind {
    /// vello renders straight into the acquired swapchain texture
    /// (`Rgba8Unorm` + `STORAGE_BINDING`); no intermediate texture, no blit.
    Direct,
    /// vello renders into an intermediate `Rgba8Unorm` target that is blitted to
    /// the swapchain each frame — the fallback for probe-refused/`Bgra8`-only
    /// surfaces and the `cpu-tier` path.
    Blit,
}

/// Pure direct-to-surface path policy (deliverable 2/4): choose [`Direct`] only
/// when the surface advertises **both** `Rgba8Unorm` and `STORAGE_BINDING` (the
/// exact requirements of vello 0.9's `render_to_texture` target) **and** the
/// blit arm is not force-selected; otherwise [`Blit`].
///
/// `force_blit` folds together the `FRUST_NO_DIRECT_SURFACE` safety valve and
/// the `cpu-tier`-selected case (the CPU tier uploads its pixmap into the
/// intermediate target, so it is blit-only by construction) — both resolved at
/// the call site. Host-testable: no `wgpu` state is touched here.
///
/// [`Direct`]: RenderPathKind::Direct
/// [`Blit`]: RenderPathKind::Blit
pub(crate) fn choose_render_path(
    has_rgba8unorm: bool,
    has_storage_binding: bool,
    force_blit: bool,
) -> RenderPathKind {
    if !force_blit && has_rgba8unorm && has_storage_binding {
        RenderPathKind::Direct
    } else {
        RenderPathKind::Blit
    }
}

/// Whether the shader-showcase fragment-shader pre-pass
/// (`crate::renderer::run_shader_prepass`) is force-disabled via the
/// process-wide `FRUST_NO_SHADER_EFFECTS` flag — the same
/// unproven-render-path safety valve shape as
/// [`direct_surface_force_blit`]: the pre-pass drives real GPU work
/// (pipeline compiles, offscreen targets, a vello image-override
/// registration) that per the shader-showcase feature's own task notes is
/// unproven on device, so this flag is the fallback-proof escape hatch that
/// reverts every `Command::ShaderQuad` to the existing placeholder-fill
/// lowering (`convert::encode_scene_with_shaders`'s miss path) with zero
/// pre-pass GPU work. Same compile-time-or-runtime parsing as
/// `FRUST_TRACE`/`FRUST_NO_DIRECT_SURFACE` (see `docs/DEVELOPMENT.md`'s
/// Instrumentation table). Cached: read once per process.
pub(crate) fn shader_effects_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_NO_SHADER_EFFECTS"),
            std::env::var("FRUST_NO_SHADER_EFFECTS").ok(),
        )
    })
}

/// The two direct-to-surface capability bits, read once from a surface's
/// [`wgpu::SurfaceCapabilities`]. Shared by the capability probe log and the
/// [`choose_render_path`] decision so the query is not duplicated (the probe
/// from task 05 is the source of this logic).
fn direct_surface_caps(capabilities: &wgpu::SurfaceCapabilities) -> (bool, bool) {
    let has_rgba8unorm = capabilities
        .formats
        .contains(&wgpu::TextureFormat::Rgba8Unorm);
    let has_storage_binding = capabilities
        .usages
        .contains(wgpu::TextureUsages::STORAGE_BINDING);
    (has_rgba8unorm, has_storage_binding)
}

/// Probes the surface's direct-to-surface capability and logs the result
/// (if perf tracing is enabled). Logs once per process.
///
/// Direct-to-surface rendering requires:
/// - Rgba8Unorm format in the surface's supported formats
/// - STORAGE_BINDING usage in the surface's supported usages
///
/// Only compiled under the `perf-trace` feature (release-lean plan, task
/// 02); see the inert `#[cfg(not(feature = "perf-trace"))]` counterpart
/// below.
#[cfg(feature = "perf-trace")]
fn probe_direct_to_surface_capability(capabilities: &wgpu::SurfaceCapabilities) {
    static LOGGED: OnceLock<()> = OnceLock::new();

    if !perf_tracing_enabled() {
        return;
    }

    LOGGED.get_or_init(|| {
        // Format the supported formats as a comma-separated list
        let formats_str = if capabilities.formats.is_empty() {
            "[]".to_string()
        } else {
            format!(
                "[{}]",
                capabilities
                    .formats
                    .iter()
                    .map(|f| format!("{:?}", f))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };

        // Reuse the shared capability read so the probe verdict and the live
        // path decision (`choose_render_path`) can never disagree.
        let (has_rgba8unorm, has_storage_binding) = direct_surface_caps(capabilities);

        // Determine the verdict
        let direct_to_surface_supported = has_rgba8unorm && has_storage_binding;
        let verdict = if direct_to_surface_supported {
            "YES"
        } else {
            "NO"
        };

        // Provide a reason for the verdict
        let reason = if direct_to_surface_supported {
            "Rgba8Unorm+STORAGE_BINDING".to_string()
        } else {
            let mut missing = Vec::new();
            if !has_rgba8unorm {
                missing.push("no Rgba8Unorm");
            }
            if !has_storage_binding {
                missing.push("no STORAGE_BINDING");
            }
            missing.join(", ")
        };

        // Log the surface capabilities probe
        log::info!(
            "frust-perf surface-caps formats={} usages={:?} direct_to_surface={} ({})",
            formats_str,
            capabilities.usages,
            verdict,
            reason
        );
    });
}

/// Without the `perf-trace` feature, the surface-caps probe is a complete
/// no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn probe_direct_to_surface_capability(_capabilities: &wgpu::SurfaceCapabilities) {}

/// Emits the one-per-process startup line naming the chosen render path and the
/// reason (deliverable 3), mirroring the surface-caps probe line style and its
/// `FRUST_TRACE` gating. Logged once regardless of surface recreation.
///
/// Only compiled under the `perf-trace` feature (release-lean plan, task
/// 02); see the inert `#[cfg(not(feature = "perf-trace"))]` counterpart
/// below.
#[cfg(feature = "perf-trace")]
fn log_render_path(
    path: RenderPathKind,
    has_rgba8unorm: bool,
    has_storage_binding: bool,
    force_blit: bool,
) {
    static LOGGED: OnceLock<()> = OnceLock::new();

    if !perf_tracing_enabled() {
        return;
    }

    LOGGED.get_or_init(|| {
        let (name, reason) = match path {
            RenderPathKind::Direct => ("direct", "Rgba8Unorm+STORAGE_BINDING".to_string()),
            RenderPathKind::Blit => {
                let reason = if force_blit {
                    // The safety valve wins even on a capable surface — say so, so
                    // an A/B run's log confirms the arm it is actually exercising.
                    "forced (FRUST_NO_DIRECT_SURFACE or cpu-tier)".to_string()
                } else {
                    let mut missing = Vec::new();
                    if !has_rgba8unorm {
                        missing.push("no Rgba8Unorm");
                    }
                    if !has_storage_binding {
                        missing.push("no STORAGE_BINDING");
                    }
                    missing.join(", ")
                };
                ("blit", reason)
            }
        };
        log::info!("frust-perf render-path {name} ({reason})");
    });
}

/// Without the `perf-trace` feature, the render-path startup line is a
/// complete no-op — no logging, no format-string bodies compiled in.
#[cfg(not(feature = "perf-trace"))]
#[inline]
fn log_render_path(
    _path: RenderPathKind,
    _has_rgba8unorm: bool,
    _has_storage_binding: bool,
    _force_blit: bool,
) {
}

/// Resolves a caller's [`SurfaceAlphaRequest`] against the live surface's
/// reported `alpha_modes`, choosing the actual `wgpu::CompositeAlphaMode` to
/// configure with (platform-views task 04). Kept crate-private and
/// `wgpu`-typed: `SurfaceAlphaRequest` is the public, `wgpu`-free seam; the
/// resolved mode itself never crosses `frust-render`'s boundary (the
/// `DetachedSurface` opacity precedent — `docs/CODE_STANDARDS.md`'s
/// wgpu-leak anti-pattern).
///
/// `Opaque` reproduces today's behavior bit-for-bit (`Auto`, unchanged for
/// every existing caller). `TranslucentPreferred` tries, in order, `Inherit`
/// (Android's only reported translucent mode per the spike), `PostMultiplied`
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
/// black rectangles (review finding M1). Keying off this instead degrades to
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
/// vello 0.9's fine stage accumulates in premultiplied space but
/// un-premultiplies at its final write — `rgba_sep = vec4(fg.rgb * (1/fg.a),
/// fg.a)` in `vello_shaders-0.9.0/shader/fine.wgsl` (the `textureStore` at
/// line ~1394) — so `render_to_texture`'s output carries **straight**
/// (non-premultiplied) alpha. A compositor that expects premultiplied alpha
/// then over-brightens every partial-alpha pixel by `1/a`. The two
/// premultiplied-expecting modes are `PreMultiplied` and — the shipped Android
/// translucent case — `Inherit`: on Android the only reported translucent
/// mode is `Inherit`, under which SurfaceFlinger blends a `TRANSLUCENT`
/// SurfaceView premultiplied (defect D3, `research/VERIFY.md`: a 50%-alpha
/// `#FFF176` reached SurfaceFlinger stored straight `#FFF176@128` instead of
/// premultiplied `#807B3B@128`, compositing over-bright over a Mode B hole).
///
/// `PostMultiplied` (iOS's translucent mode) expects straight alpha — vello's
/// output is already correct there — and `Opaque`/`Auto` ignore alpha
/// entirely, so all three take vello's output unchanged. See
/// [`PremultiplyPass`], the direct-path fix this gates.
fn alpha_mode_needs_premultiply(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::Inherit
    )
}

/// Whether a configured surface must **refuse** translucency because its
/// chosen render path cannot deliver the premultiplied output the resolved
/// alpha mode expects (review finding M2 / AI-2, following on M1's
/// resolved-translucency seam above).
///
/// [`RenderPathKind::Blit`]'s `TextureBlitter::copy` is a plain texture copy
/// — there is no shader stage to premultiply in, unlike the
/// [`RenderPathKind::Direct`] arm's [`RenderPath::DirectPremultiplied`]
/// compute pass. So a **GPU-tier** surface forced onto the blit arm (no
/// `Rgba8Unorm`+`STORAGE_BINDING`, or `FRUST_NO_DIRECT_SURFACE`) that
/// resolves a premultiplied-expecting alpha mode (`Inherit`/`PreMultiplied`,
/// [`alpha_mode_needs_premultiply`]) would feed vello's straight-alpha blit
/// output straight to a premultiplied-expecting compositor — the same
/// over-bright fringing defect D3 fixed on the direct arm
/// (`research/VERIFY.md`). Rather than build an unverifiable
/// premultiplying blitter (blit targets lack `STORAGE_BINDING`, so it would
/// need new machinery), the adopted fix is refusal: such a surface resolves
/// NOT translucent, degrading the app to Mode A (opaque base, no
/// platform-view punch) instead of silently fringing.
///
/// **`cpu-tier` is expressly exempt** — it also forces the blit arm
/// (`force_blit` above), but its `vello_cpu` output is already
/// premultiplied (verified: `cpu_tier.rs:108-113`'s `PremulRgba8` sample
/// type), so cpu-tier Blit+`Inherit` is correct *today* and must keep
/// resolving translucent; forcing it opaque would be a self-inflicted
/// regression. The `tier` parameter is what gates this exemption.
///
/// Pure decision, kept separate from `create_render_surface`'s wgpu
/// resources so it is host-testable without a GPU, mirroring
/// [`choose_render_path`]'s split.
fn blit_translucency_refused(
    path_kind: RenderPathKind,
    alpha_mode: wgpu::CompositeAlphaMode,
    tier: crate::tier::RenderTier,
) -> bool {
    path_kind == RenderPathKind::Blit
        && alpha_mode_needs_premultiply(alpha_mode)
        && tier == crate::tier::RenderTier::Gpu
}

/// Permanent (non-spike) surface-caps + chosen-alpha-mode line, logged once
/// per surface configure (not once per process — a resize/recreate that picks
/// a different mode is worth a fresh line, unlike the direct-to-surface probe
/// above). Successor to the s0 spike's `frust-spike caps: ...` line
/// (`research/SPIKE.md` §1), minus the spike prefix. Gated exactly like
/// [`log_render_path`] so a release build (no `perf-trace` feature) stays
/// string-free.
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
/// its command buffers — the frust-owned equivalent of
/// `vello::util::DeviceHandle` (whose `adapter` field is private, which is why
/// we cannot reuse vello's device pool).
pub(crate) struct DeviceHandle {
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
}

/// Owns the wgpu `Instance` and the single logical device vello renders with.
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
    /// device (spec Phase 6 / PLAN.md D4). Defaults to [`RenderTier::Gpu`] and
    /// is only ever [`RenderTier::Cpu`] in a `cpu-tier`-feature build whose
    /// probe (or override) chose the CPU fallback — the
    /// [`SurfaceRenderer`](crate::SurfaceRenderer) reads it to pick the encode
    /// path. In a default (GPU-only) build a failed GPU probe errors out of
    /// `ensure_device` before this is ever set to anything but `Gpu`.
    pub(crate) selected_tier: crate::tier::RenderTier,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A cheap, cloneable handle to a [`RenderContext`]'s wgpu `Instance`, used to
/// create a surface on a *different* thread than the one that owns the context
/// (plan phase 11.B, the render-thread split).
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
/// [`SurfaceRenderer::on_surface_installed`](crate::SurfaceRenderer::on_surface_installed)
/// (plan phase 11.B). Opaque so the `wgpu` type stays confined to this crate; a
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

/// Straight-alpha → premultiplied-alpha conversion compute pass for the direct
/// render path's translucent, premultiplied-expecting arm
/// ([`RenderPath::DirectPremultiplied`]).
///
/// vello 0.9 outputs **straight** alpha (see [`alpha_mode_needs_premultiply`]);
/// a `PreMultiplied`/`Inherit` swapchain needs it premultiplied. This pass
/// reads the straight vello output as a sampled texture and writes
/// `(rgb*a, a)` into the swapchain (a write-only `rgba8unorm` storage texture —
/// the same `STORAGE_BINDING` the direct path already configures the swapchain
/// with). Portable: sampled read + write-only storage, no in-place read-write
/// storage (which `rgba8unorm` does not support).
pub(crate) struct PremultiplyPass {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// WGSL for [`PremultiplyPass`]: premultiply every texel of a straight-alpha
/// source into a premultiplied destination. One invocation per pixel; the
/// bounds guard covers a surface whose dimensions are not a multiple of the
/// workgroup size.
const PREMULTIPLY_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn premultiply(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(dst);
    if gid.x >= dims.x || gid.y >= dims.y {
        return;
    }
    let coord = vec2<i32>(i32(gid.x), i32(gid.y));
    let c = textureLoad(src, coord, 0);
    textureStore(dst, coord, vec4<f32>(c.rgb * c.a, c.a));
}
"#;

impl PremultiplyPass {
    /// Compute workgroup edge (matches `@workgroup_size(8, 8)` in
    /// [`PREMULTIPLY_WGSL`]); dispatch counts round up against it.
    const WORKGROUP: u32 = 8;

    /// Builds the compute pipeline + bind-group layout. Cheap enough to build
    /// once per surface configure (a rare event), like the blit arm's
    /// [`TextureBlitter`].
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frust-render premultiply module"),
            source: wgpu::ShaderSource::Wgsl(PREMULTIPLY_WGSL.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frust-render premultiply binds"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frust-render premultiply layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("frust-render premultiply pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("premultiply"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            pipeline,
            bind_group_layout,
        }
    }

    /// Records the straight→premultiplied compute pass into `encoder`: reads
    /// `src_view` (vello's straight output) and writes premultiplied pixels
    /// into `dst_view` (the acquired swapchain texture). The caller submits
    /// `encoder`.
    pub(crate) fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        dst_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frust-render premultiply bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(dst_view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("frust-render premultiply pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(
            width.div_ceil(Self::WORKGROUP),
            height.div_ceil(Self::WORKGROUP),
            1,
        );
    }
}

/// The per-frame render path a [`ConfiguredSurface`] carries — the resources
/// specific to the chosen arm (see [`RenderPathKind`]).
///
/// The [`Direct`](Self::Direct) arm holds nothing extra: vello renders straight
/// into the acquired swapchain texture, so there is no intermediate target or
/// blitter to keep. The [`Blit`](Self::Blit) arm owns the intermediate
/// `Rgba8Unorm` target vello renders into plus the [`TextureBlitter`] that
/// copies it to the swapchain each frame.
pub(crate) enum RenderPath {
    /// Direct-to-surface: the swapchain is configured `Rgba8Unorm` +
    /// `STORAGE_BINDING` and vello's `render_to_texture` targets its acquired
    /// texture directly (deliverable 1). Eliminates the intermediate texture and
    /// the per-frame blit pass.
    Direct,
    /// Direct-to-surface for a **premultiplied-expecting translucent** swapchain
    /// (`Inherit`/`PreMultiplied` alpha mode — see
    /// [`alpha_mode_needs_premultiply`], platform-views defect D3). vello's
    /// output is straight-alpha, but such a compositor blends premultiplied, so
    /// the frame cannot be presented straight from `render_to_texture`. vello
    /// instead renders into `intermediate_view` (in `encode`, where it already
    /// exists — unlike the swapchain), then [`Self::DirectPremultiplied`]'s
    /// `premultiply` compute pass writes `(rgb*a, a)` into the acquired
    /// swapchain texture (in `submit`). Opaque and `PostMultiplied` surfaces
    /// never take this arm (they stay [`Self::Direct`], byte-identical). Still
    /// the direct family (`STORAGE_BINDING` swapchain, no `RENDER_ATTACHMENT`
    /// blit) — the extra pass is one compute dispatch over the frame, paid only
    /// by the translucent surfaces that need it.
    ///
    /// Only `intermediate_view` is stored, not the backing `wgpu::Texture`: a
    /// `TextureView` refcounts its texture alive internally, and unlike the blit
    /// arm's `target_texture` (which cpu-tier writes into via `write_texture`)
    /// nothing here needs the texture handle — this arm is GPU-tier only.
    DirectPremultiplied {
        intermediate_view: wgpu::TextureView,
        premultiply: PremultiplyPass,
    },
    /// Blit fallback: vello renders into `target_view`, then `blitter` copies
    /// `target_texture` into the acquired swapchain texture each frame. Also the
    /// `cpu-tier` upload target (`COPY_DST`, see [`create_targets`]).
    Blit {
        target_texture: wgpu::Texture,
        target_view: wgpu::TextureView,
        blitter: TextureBlitter,
    },
}

/// A configured swapchain surface plus the render-path resources for the arm it
/// was configured for — frust's replacement for `vello::util::RenderSurface`
/// (whose fields always include an intermediate target + blitter, which the
/// direct arm does not use). Crate-private; the wrapped `wgpu` types never
/// escape `frust-render`.
pub(crate) struct ConfiguredSurface {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) path: RenderPath,
    /// Whether this surface **actually** came up translucent — the wgpu-free
    /// projection of `config.alpha_mode` through [`alpha_mode_is_translucent`],
    /// computed once at configure time (the mode never changes for a live
    /// surface; a resize reconfigures with the same `config`), *except* when
    /// [`blit_translucency_refused`] forces it to `false` (review finding M2:
    /// a GPU-tier blit-fallback surface cannot deliver the premultiplied
    /// output such an alpha mode expects — `cpu-tier` is exempt).
    ///
    /// Stored as a plain `bool` rather than re-derived from `config.alpha_mode`
    /// at each read so the value a shell observes through
    /// [`SurfaceRenderer::surface_resolved_translucent`](crate::SurfaceRenderer::surface_resolved_translucent)
    /// crosses this crate's boundary with no `wgpu` type in the signature
    /// (`docs/CODE_STANDARDS.md`'s wgpu-leak anti-pattern).
    pub(crate) resolved_translucent: bool,
}

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
fn effective_limits(base: wgpu::Limits, is_ios_simulator: bool) -> wgpu::Limits {
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
const fn is_ios_simulator() -> bool {
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

/// Creates the intermediate render target vello draws into (it renders via a
/// compute shader, which cannot bind a swapchain texture directly, so each
/// frame is blitted from this target to the acquired surface texture).
/// Replicates `vello::util::create_targets` (which is private).
fn create_targets(
    width: u32,
    height: u32,
    device: &wgpu::Device,
) -> (wgpu::Texture, wgpu::TextureView) {
    // The GPU tier renders into this via a storage binding, then blits it to
    // the swapchain. The CPU tier (`cpu-tier` feature) instead uploads its
    // rasterized pixmap into it with `write_texture`, which needs `COPY_DST` —
    // added only under the feature so default GPU-only builds keep the exact
    // usage set they had before.
    #[allow(unused_mut)]
    let mut usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
    #[cfg(feature = "cpu-tier")]
    {
        usage |= wgpu::TextureUsages::COPY_DST;
    }
    let target_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frust-render vello target"),
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

impl RenderContext {
    /// Creates a context with a fresh wgpu `Instance` and no device yet
    /// (the device is created lazily on first surface creation).
    ///
    /// Mirrors `vello::util::RenderContext::new()`'s instance setup, except
    /// when actually running on an Android *emulator* it strips the
    /// `DEBUG`/`VALIDATION` instance flags (which
    /// `InstanceFlags::from_build_config()` turns on in debug builds). The
    /// `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set
    /// object-name labels via `vkSetDebugUtilsObjectNameEXT`, and the
    /// emulator's gfxstream Vulkan HAL (`vulkan.ranchu.so`) segfaults inside
    /// that entry point during adapter enumeration — the same class of
    /// debug-utils fragility the RESEARCH.md MoltenVK caveat warns about
    /// (see task 25's summary: `#00 vulkan.ranchu.so
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
            selected_tier: crate::tier::RenderTier::Gpu,
        }
    }

    /// A cloneable [`SurfaceFactory`] sharing this context's wgpu `Instance`,
    /// for creating a [`DetachedSurface`] on the windowing/main thread when the
    /// context itself lives on the render thread (plan phase 11.B, the
    /// render-thread split). The surface a clone produces stays compatible with
    /// the device this context creates, since both share one Arc-backed
    /// instance.
    pub fn surface_factory(&self) -> SurfaceFactory {
        SurfaceFactory {
            instance: self.instance.clone(),
        }
    }

    /// The render tier the live device was created for (see
    /// [`selected_tier`](Self::selected_tier) field). `Gpu` until a surface
    /// (and thus a device) has been created.
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
    /// wgpu 29 only implements the persisted pipeline cache on Vulkan (Android);
    /// Metal/desktop adapters never advertise the feature, so it is absent there
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
    /// rebuilt (task 19, spec §14 phase 7.E).
    async fn ensure_device(&mut self, surface: &wgpu::Surface<'static>) -> Result<()> {
        if let Some(existing) = &self.device
            && existing.adapter.is_surface_supported(surface)
        {
            return Ok(());
        }
        self.create_device(Some(surface)).await
    }

    /// Create the logical device **before any surface exists** (task 19, spec
    /// §14 phase 7.E), so the wgpu instance/adapter/device bring-up can run on a
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

        // Consult the tier probe (spec Phase 6, PLAN.md D4) instead of a
        // bespoke downlevel check, so a failed GPU probe surfaces through the
        // one diagnostic path `select_render_tier` owns (shared with its own
        // unit tests). An explicit override (`FRUST_RENDER_TIER`, or
        // `frust run --render-tier` setting it for the spawned process)
        // wins *among available tiers* — a `Cpu` override always applies, but a
        // `Gpu` override onto an adapter that lacks the required downlevel flags
        // is refused (`select_render_tier` returns `Unavailable`), so the
        // `Unavailable` arm below fails fast rather than handing vello a device
        // it panics on. When the `cpu-tier` feature is compiled in, a failed
        // GPU probe now selects the experimental `Cpu` tier (vello_cpu, see the
        // `cpu_tier` module + `SurfaceRenderer`) instead of failing — the
        // device is still created (only vello's compute path is unavailable;
        // the blit the CPU pixmap rides is a basic render pipeline). Without
        // the feature, `select_render_tier` never returns `Available(Cpu)`, so
        // a failed probe still fails fast here exactly like the prior bespoke
        // check did, just through the unified diagnosis.
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
        // A `Cpu` selection is only reachable in a `cpu-tier` build; guard
        // against a stray one in a default build so the SurfaceRenderer never
        // sees a tier it has no encode path for.
        #[cfg(not(feature = "cpu-tier"))]
        if tier != crate::tier::RenderTier::Gpu {
            return Err(anyhow!(selection.diagnosis));
        }
        self.selected_tier = tier;

        let required_features = adapter.features() & vello_optional_features();
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

    /// Builds a configured [`RenderSurface`] from a raw wgpu `Surface`,
    /// creating the logical device if needed. Replaces
    /// `vello::util::RenderContext::create_render_surface` so we control the
    /// device's `required_limits`.
    pub(crate) async fn create_render_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        alpha: SurfaceAlphaRequest,
    ) -> Result<ConfiguredSurface> {
        self.ensure_device(&surface).await?;
        // The tier probe (task 06) already ran in `ensure_device`, so
        // `selected_tier()` is authoritative here: the `cpu-tier` path uploads
        // its pixmap into the intermediate target, so it is blit-only and forces
        // the blit arm alongside the `FRUST_NO_DIRECT_SURFACE` safety valve.
        let force_blit =
            direct_surface_force_blit() || self.selected_tier() != crate::tier::RenderTier::Gpu;
        let handle = self.device_handle();

        let capabilities = surface.get_capabilities(&handle.adapter);
        probe_direct_to_surface_capability(&capabilities);
        let (has_rgba8unorm, has_storage_binding) = direct_surface_caps(&capabilities);
        let path_kind = choose_render_path(has_rgba8unorm, has_storage_binding, force_blit);
        log_render_path(path_kind, has_rgba8unorm, has_storage_binding, force_blit);

        let alpha_mode = resolve_alpha_mode(alpha, &capabilities);
        log_surface_alpha_caps(&capabilities, alpha_mode);

        // Direct arm: the swapchain itself is the vello render target, so it must
        // be `Rgba8Unorm` (vello's `render_to_texture` target format) and carry
        // `STORAGE_BINDING` (vello renders via a compute storage write). Blit arm:
        // any supported `Rgba8/Bgra8` swapchain works — the blitter converts the
        // intermediate `Rgba8Unorm` target into it — and only `RENDER_ATTACHMENT`
        // is needed.
        let (format, usage) = match path_kind {
            RenderPathKind::Direct => (
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::STORAGE_BINDING,
            ),
            RenderPathKind::Blit => {
                let format = capabilities
                    .formats
                    .iter()
                    .copied()
                    .find(|it| {
                        matches!(
                            it,
                            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                        )
                    })
                    .ok_or_else(|| {
                        anyhow!("frust-render: no supported surface format (Rgba8/Bgra8)")
                    })?;
                (format, wgpu::TextureUsages::RENDER_ATTACHMENT)
            }
        };

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
            // A premultiplied-expecting translucent swapchain (`Inherit`/
            // `PreMultiplied`) cannot take vello's straight output directly —
            // it needs a premultiply pass (defect D3). Such surfaces get an
            // intermediate vello renders into plus the compute pass that
            // premultiplies it into the swapchain. Opaque/`PostMultiplied`
            // surfaces skip all of this and stay byte-identical on `Direct`.
            RenderPathKind::Direct if alpha_mode_needs_premultiply(alpha_mode) => {
                let (_intermediate_texture, intermediate_view) =
                    create_targets(width, height, &handle.device);
                RenderPath::DirectPremultiplied {
                    intermediate_view,
                    premultiply: PremultiplyPass::new(&handle.device),
                }
            }
            RenderPathKind::Direct => RenderPath::Direct,
            RenderPathKind::Blit => {
                let (target_texture, target_view) = create_targets(width, height, &handle.device);
                RenderPath::Blit {
                    target_texture,
                    target_view,
                    blitter: TextureBlitter::new(&handle.device, format),
                }
            }
        };
        let resolved_translucent =
            if blit_translucency_refused(path_kind, alpha_mode, self.selected_tier()) {
                log::warn!(
                    "frust-render: GPU-tier blit-fallback surface cannot deliver premultiplied \
                 output (alpha_mode={alpha_mode:?}) — refusing translucency, app degrades to \
                 Mode A"
                );
                false
            } else {
                // The RESOLVED translucency, not the request: a
                // `TranslucentPreferred` that fell back to `Auto` above lands here
                // as `false`, which is what the shells' paint contract keys off
                // (review finding M1 — see `alpha_mode_is_translucent`).
                alpha_mode_is_translucent(alpha_mode)
            };
        let configured = ConfiguredSurface {
            surface,
            config,
            path,
            resolved_translucent,
        };
        self.configure_surface(&configured);
        Ok(configured)
    }

    /// Builds a [`RenderSurface`] from anything convertible into a wgpu
    /// `SurfaceTarget` (the desktop `winit` window path). Replaces
    /// `vello::util::RenderContext::create_surface`.
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

    /// Resizes `surface` in place and reconfigures the swapchain. The blit arm
    /// and the direct-premultiplied arm each recreate their intermediate target
    /// texture at the new size; the plain direct arm has no intermediate, so
    /// only the swapchain config changes. Zero dimensions are rejected upstream.
    pub(crate) fn resize_surface(&self, surface: &mut ConfiguredSurface, width: u32, height: u32) {
        surface.config.width = width;
        surface.config.height = height;
        match &mut surface.path {
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
            RenderPath::DirectPremultiplied {
                intermediate_view, ..
            } => {
                let (_new_texture, new_view) =
                    create_targets(width, height, &self.device_handle().device);
                *intermediate_view = new_view;
            }
            RenderPath::Direct => {}
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
    fn direct_path_chosen_only_when_both_caps_present_and_not_forced() {
        // The GO population (OP9/Xiaomi 12): Rgba8Unorm + STORAGE_BINDING, no
        // force → direct-to-surface.
        assert_eq!(
            choose_render_path(true, true, false),
            RenderPathKind::Direct
        );
    }

    #[test]
    fn force_blit_overrides_a_capable_surface() {
        // FRUST_NO_DIRECT_SURFACE (or cpu-tier) pins a fully-capable surface onto
        // the blit arm — the fallback-proof safety valve / A-B mechanism.
        assert_eq!(choose_render_path(true, true, true), RenderPathKind::Blit);
    }

    #[test]
    fn missing_either_cap_falls_back_to_blit() {
        // iPhone SE (Bgra8-only): no Rgba8Unorm → blit.
        assert_eq!(choose_render_path(false, true, false), RenderPathKind::Blit);
        // No STORAGE_BINDING → blit.
        assert_eq!(choose_render_path(true, false, false), RenderPathKind::Blit);
        // Neither → blit.
        assert_eq!(
            choose_render_path(false, false, false),
            RenderPathKind::Blit
        );
        // Neither, and forced → still blit (force never resurrects direct).
        assert_eq!(choose_render_path(false, false, true), RenderPathKind::Blit);
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
        // Android's shape (spike): `Inherit` is the only reported mode.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Inherit
        );
    }

    #[test]
    fn translucent_preferred_picks_post_multiplied_when_inherit_absent() {
        // iOS's shape (spike): `[Opaque, PostMultiplied]` — no `Inherit`.
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
        // The wgpu-free projection the shells' paint contract keys off
        // (review finding M1): exactly `resolve_alpha_mode`'s preference list.
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

    #[test]
    fn forced_mismatch_translucent_request_resolves_not_translucent() {
        // The review's mandatory forced-mismatch bar (M1): a surface whose
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

    #[test]
    fn premultiply_gated_on_premultiplied_expecting_modes() {
        // The two premultiplied-expecting modes need the pass (defect D3):
        // `Inherit` is Android's translucent mode (SurfaceFlinger blends
        // premultiplied); `PreMultiplied` is explicit.
        assert!(alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::Inherit
        ));
        assert!(alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::PreMultiplied
        ));
        // Straight-expecting (iOS `PostMultiplied`) and alpha-ignoring
        // (`Opaque`/`Auto`, every opaque surface) take vello's output unchanged
        // — the pass must never touch them, or the opaque path stops being
        // byte-identical.
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::Opaque
        ));
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::Auto
        ));
    }

    #[test]
    fn gpu_tier_blit_refuses_premultiplied_expecting_translucency() {
        // Review finding M2 (AI-2): a GPU-tier surface forced onto the blit
        // arm (no Rgba8Unorm+STORAGE_BINDING) that resolves a
        // premultiplied-expecting alpha mode (Android's `Inherit`) must
        // refuse translucency — the blit arm's plain `TextureBlitter::copy`
        // has no premultiply stage, so presenting it straight would
        // reproduce defect D3.
        assert!(blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::Inherit,
            crate::tier::RenderTier::Gpu,
        ));
        assert!(blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::PreMultiplied,
            crate::tier::RenderTier::Gpu,
        ));
    }

    #[test]
    fn cpu_tier_blit_stays_translucent_capable() {
        // The regression guard that matters most: cpu-tier ALSO forces the
        // blit arm (`force_blit`), but `vello_cpu`'s output is already
        // premultiplied (`cpu_tier.rs:108-113`'s `PremulRgba8`), so refusal
        // must NOT fire for it — forcing it opaque would be a
        // self-inflicted regression on a path that is correct today.
        assert!(!blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::Inherit,
            crate::tier::RenderTier::Cpu,
        ));
        assert!(!blit_translucency_refused(
            RenderPathKind::Blit,
            wgpu::CompositeAlphaMode::PreMultiplied,
            crate::tier::RenderTier::Cpu,
        ));
    }

    #[test]
    fn blit_refusal_never_fires_for_non_premultiplied_modes_or_direct_path() {
        // Straight-expecting/alpha-ignoring modes never need refusal, on
        // either path kind.
        for mode in [
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!blit_translucency_refused(
                RenderPathKind::Blit,
                mode,
                crate::tier::RenderTier::Gpu
            ));
        }
        // The Direct arm is unaffected regardless of tier/mode — it already
        // has its own premultiply pass (`RenderPath::DirectPremultiplied`),
        // so byte-identical direct-path behavior is preserved.
        for mode in [
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::Auto,
        ] {
            assert!(!blit_translucency_refused(
                RenderPathKind::Direct,
                mode,
                crate::tier::RenderTier::Gpu
            ));
        }
    }

    /// The exact defect-D3 arithmetic the [`PremultiplyPass`] shader performs,
    /// as an always-run reference (no GPU): a 50%-alpha `#FFF176` painted over
    /// a Mode B hole must reach a premultiplied-expecting compositor stored
    /// premultiplied (`~#807B3B@128`), NOT straight (`#FFF176@128` — the
    /// over-bright value the device measured before this fix,
    /// `research/VERIFY.md` D3). Encodes the predicted-correct pixel the device
    /// re-check (task 04) must confirm.
    #[test]
    fn d3_premultiply_math_matches_verify_predictions() {
        // `premultiply` in PREMULTIPLY_WGSL is `rgb * a` in normalized [0,1];
        // in 8-bit that is `round(c * a / 255)`.
        fn premul8(c: u8, a: u8) -> u8 {
            ((u32::from(c) * u32::from(a) + 127) / 255) as u8
        }

        // 50%-alpha #FFF176 (the spike's ghost button). a=128 ≈ 0.502.
        let (r, g, b, a) = (0xFF, 0xF1, 0x76, 128);
        let premul = [premul8(r, a), premul8(g, a), premul8(b, a), a];
        // Straight (the WRONG, over-bright value) keeps rgb at full intensity.
        assert_eq!([r, g, b, a], [0xFF, 0xF1, 0x76, 128]);
        // Premultiplied is materially darker per channel — this is the fix.
        assert_eq!(premul, [0x80, 0x79, 0x3B, 128]);
        // Within a couple of LSB of VERIFY D3's stated `~#807B3B@128`.
        assert!((i16::from(premul[1]) - 0x7B).abs() <= 2);

        // A second point on the curve: 25%-alpha opaque-magenta debris fill
        // (D3's `#E4..FF`-class over-bright). a=64 ≈ 0.251.
        let a = 64;
        assert_eq!(
            [premul8(0xFF, a), premul8(0x00, a), premul8(0xFF, a), a],
            [0x40, 0x00, 0x40, 64]
        );
        // Fully-opaque pixels are premultiply-invariant — this is why the
        // opaque path stays byte-identical and in-scene blends over opaque
        // content already composite correctly (VERIFY D3).
        assert_eq!(
            [premul8(0xFF, 255), premul8(0xF1, 255), premul8(0x76, 255)],
            [0xFF, 0xF1, 0x76]
        );
    }

    /// Real-GPU end-to-end confirmation of the defect-D3 fix on this host's
    /// Vulkan adapter: vello's `render_to_texture` emits **straight** alpha, and
    /// [`PremultiplyPass`] converts it to premultiplied — the exact operation
    /// [`RenderPath::DirectPremultiplied`] inserts before presenting a
    /// premultiplied-expecting translucent swapchain (Android `Inherit`). No
    /// on-screen surface exists headlessly, so this drives the two GPU halves
    /// (vello output + the compute pass) directly against owned textures rather
    /// than through `submit`.
    ///
    /// Encodes VERIFY D3's `#FFF176@128` prediction: asserts (a) vello stores
    /// the fill STRAIGHT (`r≈255`, proving the diagnosis — the output really is
    /// non-premultiplied), then (b) the pass stores it PREMULTIPLIED
    /// (`r≈128 = round(255 * 128/255)`, proving the fix), each within a couple
    /// of LSB of the value derived from vello's own readback.
    #[test]
    #[ignore = "requires a GPU; run locally with `cargo test -p frust-render -- --ignored`"]
    fn premultiply_pass_converts_vello_straight_output_to_premultiplied() {
        pollster::block_on(run());

        async fn run() {
            const SIZE: u32 = 64; // 64*4 = 256-byte rows: no copy-padding math.

            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no compatible GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust premultiply test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create device");

            let make_tex = |label: &str, extra: wgpu::TextureUsages| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | extra,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_formats: &[],
                })
            };
            // vello's straight output target (also the premultiply pass's read
            // source), and the pass's write destination (the "swapchain").
            let straight_tex = make_tex("straight (vello)", wgpu::TextureUsages::empty());
            let straight_view = straight_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let premul_tex = make_tex("premultiplied (out)", wgpu::TextureUsages::empty());
            let premul_view = premul_tex.create_view(&wgpu::TextureViewDescriptor::default());

            // A 50%-alpha #FFF176 fill over a fully-transparent base — the
            // spike's ghost button over a Mode B hole (alpha-0 behind).
            let mut fk_scene = frust_scene::Scene::new();
            {
                let mut builder = frust_scene::SceneBuilder::new(&mut fk_scene);
                builder.fill_rect(
                    kurbo::Rect::new(0.0, 0.0, SIZE as f64, SIZE as f64),
                    peniko::Brush::Solid(peniko::Color::from_rgba8(0xFF, 0xF1, 0x76, 128)),
                );
            }
            let mut vello_scene = vello::Scene::new();
            crate::encode_scene(&fk_scene, &mut vello_scene);
            let mut renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
                .expect("failed to create vello renderer");
            renderer
                .render_to_texture(
                    &device,
                    &queue,
                    &vello_scene,
                    &straight_view,
                    &vello::RenderParams {
                        base_color: peniko::Color::TRANSPARENT,
                        width: SIZE,
                        height: SIZE,
                        antialiasing_method: vello::AaConfig::Area,
                    },
                )
                .expect("render_to_texture failed");

            // Run the fix: straight → premultiplied into the destination.
            let pass = PremultiplyPass::new(&device);
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            pass.record(
                &device,
                &mut encoder,
                &straight_view,
                &premul_view,
                SIZE,
                SIZE,
            );
            queue.submit([encoder.finish()]);

            let straight = read_centre(&device, &queue, &straight_tex, SIZE).await;
            let premul = read_centre(&device, &queue, &premul_tex, SIZE).await;

            // (a) vello stored STRAIGHT alpha: rgb near full intensity, a≈128.
            // This is the diagnosis — the un-premultiply in fine.wgsl.
            assert!(
                straight[0] >= 250 && (i16::from(straight[3]) - 128).abs() <= 2,
                "expected vello straight output (~#FFF176@128), got {straight:?}"
            );
            // (b) the pass stored PREMULTIPLIED alpha, derived from vello's own
            // straight readback so the assertion is robust to vello's rounding:
            // premul_c ≈ round(straight_c * a/255).
            let a = u32::from(straight[3]);
            for ch in 0..3 {
                let expected = ((u32::from(straight[ch]) * a + 127) / 255) as i16;
                assert!(
                    (i16::from(premul[ch]) - expected).abs() <= 2,
                    "channel {ch}: premultiplied {} not within 2 LSB of expected {expected} \
                     (straight {straight:?} -> premul {premul:?})",
                    premul[ch]
                );
            }
            // Alpha is unchanged by premultiply; red must have visibly darkened
            // (255 -> ~128), proving the pass actually ran.
            assert_eq!(premul[3], straight[3], "premultiply must not change alpha");
            assert!(
                premul[0] < 160,
                "red should be premultiplied down toward 128, got {}",
                premul[0]
            );
        }

        /// Copies the centre texel of `tex` back to the CPU as `[r,g,b,a]`.
        async fn read_centre(
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            tex: &wgpu::Texture,
            size: u32,
        ) -> [u8; 4] {
            let bytes_per_row = size * 4;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust premultiply readback"),
                size: u64::from(bytes_per_row * size),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(size),
                    },
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);

            let slice = buffer.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll failed");
            rx.recv()
                .expect("map channel closed")
                .expect("buffer map failed");
            let data = slice.get_mapped_range();
            let centre = ((size / 2) * bytes_per_row + (size / 2) * 4) as usize;
            [
                data[centre],
                data[centre + 1],
                data[centre + 2],
                data[centre + 3],
            ]
        }
    }
}
