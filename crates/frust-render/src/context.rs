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
use vello::util::RenderSurface;
use wgpu::util::TextureBlitter;

/// The features vello's renderer opportunistically uses when the adapter
/// exposes them — mirrors `vello::util::RenderContext::new_device` so our
/// hand-rolled device request stays behaviourally identical to vello's, apart
/// from the `required_limits` we control.
fn vello_optional_features() -> wgpu::Features {
    wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE
}

/// Whether perf tracing (frust-perf logging) is enabled via the process-wide
/// `FRUST_TRACE` flag — mirroring the check in `frust-shell-common::perf`.
/// Cached to avoid repeated environment lookups.
fn perf_tracing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let compile_time = option_env!("FRUST_TRACE");
        let runtime = std::env::var("FRUST_TRACE").ok();
        fn is_set_non_zero(value: Option<&str>) -> bool {
            matches!(value, Some(v) if v != "0")
        }
        is_set_non_zero(compile_time) || is_set_non_zero(runtime.as_deref())
    })
}

/// Probes the surface's direct-to-surface capability and logs the result
/// (if perf tracing is enabled). Logs once per process.
///
/// Direct-to-surface rendering requires:
/// - Rgba8Unorm format in the surface's supported formats
/// - STORAGE_BINDING usage in the surface's supported usages
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

        // Check if Rgba8Unorm is in the supported formats
        let has_rgba8unorm = capabilities
            .formats
            .contains(&wgpu::TextureFormat::Rgba8Unorm);

        // Check if STORAGE_BINDING is in the supported usages
        let has_storage_binding = capabilities
            .usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING);

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
    ) -> Result<RenderSurface<'static>> {
        self.ensure_device(&surface).await?;
        let handle = self.device_handle();

        let capabilities = surface.get_capabilities(&handle.adapter);
        probe_direct_to_surface_capability(&capabilities);
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
            .ok_or_else(|| anyhow!("frust-render: no supported surface format (Rgba8/Bgra8)"))?;

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let (target_texture, target_view) = create_targets(width, height, &handle.device);
        let render_surface = RenderSurface {
            surface,
            config,
            // `dev_id` indexes vello's own device pool, which we do not use;
            // frust reads the device from `self.device` instead. Left at 0.
            dev_id: 0,
            format,
            target_texture,
            target_view,
            blitter: TextureBlitter::new(&handle.device, format),
        };
        self.configure_surface(&render_surface);
        Ok(render_surface)
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
    ) -> Result<RenderSurface<'static>> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|e| anyhow!("frust-render: failed to create surface: {e}"))?;
        self.create_render_surface(surface, width, height, present_mode)
            .await
    }

    /// (Re)configures the swapchain for `surface`'s current config.
    pub(crate) fn configure_surface(&self, surface: &RenderSurface<'static>) {
        surface
            .surface
            .configure(&self.device_handle().device, &surface.config);
    }

    /// Resizes `surface` in place: recreates the intermediate target texture
    /// and reconfigures the swapchain. Zero dimensions are rejected upstream.
    pub(crate) fn resize_surface(
        &self,
        surface: &mut RenderSurface<'static>,
        width: u32,
        height: u32,
    ) {
        let (target_texture, target_view) =
            create_targets(width, height, &self.device_handle().device);
        surface.target_texture = target_texture;
        surface.target_view = target_view;
        surface.config.width = width;
        surface.config.height = height;
        self.configure_surface(surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
