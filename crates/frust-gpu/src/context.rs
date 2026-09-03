//! Instance, adapter and lazy logical-device creation: [`RenderContext`] and
//! [`DeviceHandle`].
//!
//! This is the crate's entry point — everything else in `frust-gpu` consumes a
//! [`DeviceHandle`] rather than reaching for a `wgpu::Adapter` itself. A
//! [`RenderContext`] owns one `wgpu::Instance` and at most one logical device,
//! created lazily on the first surface (or the first [`RenderContext::device`]
//! call) and then reused: a logical device is display-independent, so surface
//! loss and recreation (rotation, backgrounding) must not rebuild it.
//!
//! This is also the type `frust-render` re-exports as its own
//! `frust_render::RenderContext`, which is what every platform shell holds: the
//! device/surface foundation lives here, and the renderer above adds only the
//! render-path decisions specific to how a frame is drawn.
//!
//! Surface creation itself lives in [`crate::surface`] and the raw-pointer
//! constructors in [`crate::lifecycle`]; [`RenderContext::create_render_surface`]
//! is the one entry point that ties them to a device.
//!
//! # Pure decision vs. platform lookup
//!
//! Every environment-sensitive choice here is split into a pure function taking
//! the environment as an argument (`effective_instance_flags`,
//! [`effective_limits`], `device_features`, [`decide_log_action`]) plus a
//! separate lookup that answers what the environment actually is
//! (`is_android_emulator`, [`is_ios_simulator`]). Only the lookups are
//! platform-gated, so the policies stay unit-testable on any host with no GPU
//! and no mobile target in the loop — the same split
//! [`crate::caps::TierCaps::probe`]/[`crate::caps::TierCaps::fake`] gives
//! adapter capabilities.
//!
//! # Two mitigations worth knowing about
//!
//! - The Android **emulator** cannot survive `wgpu::InstanceFlags::DEBUG`, so
//!   the instance is built with those flags stripped there and nowhere else
//!   (`effective_instance_flags`).
//! - The iOS **Simulator** misreports its uniform-buffer alignment, so a device
//!   request made there is forced back up to 256 bytes ([`effective_limits`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{Result, anyhow};

use crate::caps::{DownlevelProfile, TierCaps};
use crate::surface::{
    ConfiguredSurface, SurfaceAlphaRequest, SurfaceFactory, resolve_alpha_mode,
    select_surface_format,
};

/// Default `wgpu::Device` debug label, used when a caller supplies no
/// [`ContextOptions::device_label`] of its own.
const DEFAULT_DEVICE_LABEL: &str = "frust-gpu device";

/// How a [`RenderContext`] should build its instance and request its device.
///
/// Every field has a working default, so `ContextOptions::default()` is the
/// ordinary construction — a field exists here only where a host genuinely has
/// a choice to make, and [`RenderContext::new`] takes the defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextOptions {
    /// Debug label attached to the created `wgpu::Device`, surfaced by graphics
    /// debuggers and in validation messages.
    pub device_label: String,
    /// Backends the `wgpu::Instance` is restricted to. `None` — the default —
    /// takes `wgpu::Backends::from_env()` (the `WGPU_BACKEND` knob) and falls
    /// back to every backend compiled in, which is what a shell wants. A
    /// headless caller that must pin one backend regardless of the ambient
    /// environment sets it explicitly.
    pub backends: Option<wgpu::Backends>,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            device_label: DEFAULT_DEVICE_LABEL.to_string(),
            backends: None,
        }
    }
}

/// A logical device, the adapter it was created from, the queue that executes
/// its command buffers, and the capabilities that adapter reported.
///
/// Cheap to clone: `wgpu`'s `Adapter`/`Device`/`Queue` are all `Arc`-backed
/// handles to one underlying object, so a clone is another handle to the *same*
/// device rather than a second device. Clone it freely to hand a subsystem the
/// device it needs instead of threading a `&RenderContext` borrow through it.
#[derive(Clone, Debug)]
pub struct DeviceHandle {
    /// The adapter the device was created from.
    pub adapter: wgpu::Adapter,
    /// The logical device.
    pub device: wgpu::Device,
    /// The queue that executes this device's command buffers.
    pub queue: wgpu::Queue,
    /// What [`Self::adapter`] reported at device-creation time. Captured once
    /// so downstream pipeline/atlas decisions read plain data instead of
    /// re-probing the adapter.
    pub caps: TierCaps,
    /// The first uncaptured error this device raised, latched by the handler
    /// installed in [`create_device`]. See [`Self::first_uncaptured_error`].
    first_uncaptured_error: Arc<OnceLock<String>>,
}

impl DeviceHandle {
    /// The **first** uncaptured `wgpu` error this device ever raised, or `None`
    /// if it has raised none.
    ///
    /// Latched, never overwritten: a frame loop polling this wants the error
    /// that started the trouble, not the last one in a storm the first one
    /// caused. It is also the only programmatic view of an uncaptured error a
    /// caller gets — the handler otherwise only logs (see
    /// [`decide_log_action`]) — so a host can degrade or report instead of
    /// silently rendering nothing every frame.
    ///
    /// Deliberately not clearable: "this device has seen an uncaptured error"
    /// is a property of the device, and a device that has raised one is not
    /// reliably recoverable by forgetting about it.
    pub fn first_uncaptured_error(&self) -> Option<&str> {
        self.first_uncaptured_error.get().map(String::as_str)
    }
}

/// Owns the `wgpu::Instance` and the single logical device this crate's
/// consumers render with.
///
/// A single `RenderContext` is shared across every surface a shell creates
/// (frust is single-window); the device is created lazily on the first surface
/// and reused across surface loss/recreation (rotation, backgrounding) since a
/// logical device is display-independent. [`RenderContext::new`] performs no
/// adapter enumeration at all, so a host may build one early (before it has a
/// window, or on a thread that will never render) and pay for the device only
/// at the first surface — or at an explicit
/// [`ensure_device_headless`](Self::ensure_device_headless) pre-init.
pub struct RenderContext {
    instance: wgpu::Instance,
    options: ContextOptions,
    /// `None` until a surface (or an explicit pre-init) creates the device.
    device: Option<DeviceHandle>,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderContext {
    /// Creates a context with a fresh wgpu `Instance` and no device yet
    /// (the device is created lazily on first surface creation).
    ///
    /// The instance flags come from the build configuration
    /// (`InstanceFlags::from_build_config`, which turns `DEBUG`/`VALIDATION` on
    /// in debug builds) plus the standard `WGPU_*` environment overrides. When
    /// actually running on an Android *emulator* they are then run through
    /// `effective_instance_flags`, which strips `DEBUG`/`VALIDATION`: the
    /// `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set object-name
    /// labels via `vkSetDebugUtilsObjectNameEXT`, and the emulator's gfxstream
    /// Vulkan HAL (`vulkan.ranchu.so`) segfaults inside that entry point during
    /// adapter enumeration (observed crash: `#00 vulkan.ranchu.so
    /// vk_common_SetDebugUtilsObjectNameEXT`) — the same class of debug-utils
    /// fragility a MoltenVK Vulkan backend is also known to have. Debug object
    /// labels are only a developer convenience, so dropping them on the
    /// emulator is a safe way to keep GPU bring-up alive there while leaving
    /// physical devices' validation safety net — and desktop behavior —
    /// untouched.
    pub fn new() -> Self {
        Self::with_options(ContextOptions::default())
    }

    /// [`Self::new`] with an explicit [`ContextOptions`] — a headless harness
    /// that must label its device or pin one backend regardless of the ambient
    /// environment. Every shell takes `new()`'s defaults instead.
    pub fn with_options(options: ContextOptions) -> Self {
        let backends = options
            .backends
            .unwrap_or_else(|| wgpu::Backends::from_env().unwrap_or_default());
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
            options,
            device: None,
        }
    }

    /// A cloneable [`SurfaceFactory`] sharing this context's wgpu `Instance`,
    /// for creating a [`DetachedSurface`](crate::surface::DetachedSurface) on
    /// the windowing/main thread when the context itself lives on the render
    /// thread (the render-thread split). The surface a clone produces stays
    /// compatible with the device this context creates, since both share one
    /// Arc-backed instance.
    pub fn surface_factory(&self) -> SurfaceFactory {
        SurfaceFactory::new(&self.instance)
    }

    /// This context's wgpu `Instance`, for the two raw-pointer surface
    /// constructors in [`crate::lifecycle`] (the mobile shells' path, which
    /// receives an `ANativeWindow*`/`CAMetalLayer*` rather than a window
    /// handle a [`SurfaceFactory`] could take).
    ///
    /// Hidden from the rendered docs rather than made private, on the same
    /// grounds as
    /// [`DetachedSurface::into_surface`](crate::surface::DetachedSurface::into_surface):
    /// the renderer crate above this one is the intended (and only) caller,
    /// and no layer above *it* may re-export this accessor.
    #[doc(hidden)]
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// The single logical device, panicking if no surface has created it yet.
    ///
    /// Only called from the renderer's install/resize/render paths, all of
    /// which run strictly after a `create_*_surface`, so the device is always
    /// present. Use [`Self::device`] for the fallible, creating form.
    ///
    /// Hidden from the rendered docs for the same reason as [`Self::instance`].
    #[doc(hidden)]
    pub fn device_handle(&self) -> &DeviceHandle {
        self.device
            .as_ref()
            .expect("device must be created before it is used (surface creation creates it)")
    }

    /// The logical device, creating it on first call and returning the same one
    /// afterwards.
    ///
    /// Adapter selection goes through
    /// `wgpu::util::initialize_adapter_from_env_or_default`, so
    /// `WGPU_ADAPTER_NAME`/`WGPU_POWER_PREF` pick the adapter on a host with
    /// more than one — the only way to pin a specific GPU on a multi-adapter
    /// machine.
    ///
    /// # Errors
    ///
    /// When no adapter is available at all, or when the device request the
    /// adapter's own limits were computed for is nonetheless refused. Both are
    /// terminal for GPU rendering; neither is retryable by calling again.
    pub async fn device(&mut self) -> Result<&DeviceHandle> {
        self.ensure_device_headless().await?;
        Ok(self
            .device
            .as_ref()
            .expect("device was just created or already present"))
    }

    /// What the live device's adapter reported, or `None` while the device is
    /// still uncreated — capabilities are an adapter's answer, and no adapter
    /// has been selected before the first device creation.
    pub fn caps(&self) -> Option<&TierCaps> {
        self.device.as_ref().map(|handle| &handle.caps)
    }

    /// Whether the live device was created with `wgpu::Features::PIPELINE_CACHE`.
    ///
    /// wgpu only implements the persisted pipeline cache on Vulkan — every
    /// Vulkan adapter advertises it (Android, Linux, Windows-on-Vulkan);
    /// Metal and DX12 adapters never do, so it is absent there and
    /// [`create_pipeline_cache`](Self::create_pipeline_cache) returns `None` —
    /// the renderer then behaves exactly as it did before this path existed.
    /// Panics if no surface (and thus no device) has been created yet.
    pub fn pipeline_cache_supported(&self) -> bool {
        self.device_handle()
            .device
            .features()
            .contains(wgpu::Features::PIPELINE_CACHE)
    }

    /// The adapter fingerprint a persisted pipeline-cache blob is tagged with
    /// (see [`crate::pipeline_cache`]). Panics if no device has been created yet.
    pub fn adapter_cache_key(&self) -> String {
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
    /// Hidden from the rendered docs for the same reason as [`Self::instance`].
    ///
    /// # Safety
    ///
    /// This is the sole sanctioned unsafe site in this module, and the
    /// obligation is the caller's, not something this method can fully close
    /// on its own: `unframe`'s magic-tag-plus-adapter-fingerprint check proves
    /// only that `blob` was framed by `frust-gpu`'s own framing for *this*
    /// adapter — provenance by convention, not a proof of the actual wgpu
    /// contract on [`wgpu::Device::create_pipeline_cache`], which requires a
    /// non-`None` `data` to have come from a prior `PipelineCache::get_data()`
    /// on a `pipeline_cache_key`-compatible adapter. The caller must ensure
    /// `blob` is exactly that: a blob previously produced by this driver's own
    /// pipeline-cache output for this adapter, as persisted by `frust-render`'s
    /// caching layer (`SurfaceRenderer::pipeline_cache_data`/
    /// `set_initial_pipeline_cache_data`). A forged blob that nonetheless
    /// passes the framing/fingerprint check is undefined behaviour per wgpu's
    /// contract — `fallback: true` only covers a residual *internal* mismatch
    /// wgpu itself detects, not a blob that misleads it into misbehaving.
    #[doc(hidden)]
    pub unsafe fn create_pipeline_cache(&self, blob: Option<&[u8]>) -> Option<wgpu::PipelineCache> {
        if !self.pipeline_cache_supported() {
            return None;
        }
        let handle = self.device_handle();
        let key = crate::pipeline_cache::adapter_cache_key(&handle.adapter.get_info());
        let data = blob.and_then(|b| crate::pipeline_cache::unframe(b, &key));
        log::debug!(
            "frust-gpu: creating wgpu PipelineCache (seed: {})",
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
                    label: Some("frust-gpu pipeline cache"),
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
    ///
    /// Hidden from the rendered docs for the same reason as [`Self::instance`]:
    /// the renderer above calls it to have a device in hand before it sizes its
    /// own per-surface attachments; nothing higher may.
    #[doc(hidden)]
    pub async fn ensure_device(&mut self, surface: &wgpu::Surface<'static>) -> Result<()> {
        if let Some(existing) = &self.device
            && existing.adapter.is_surface_supported(surface)
        {
            return Ok(());
        }
        self.device = Some(create_device(&self.instance, &self.options, Some(surface)).await?);
        Ok(())
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
    /// This is the sole production caller that passes `None` below; desktop/iOS
    /// create their device through the surface path and never invoke it.
    pub async fn ensure_device_headless(&mut self) -> Result<()> {
        if self.device.is_some() {
            return Ok(());
        }
        self.device = Some(create_device(&self.instance, &self.options, None).await?);
        Ok(())
    }

    /// Builds a configured [`ConfiguredSurface`] from a raw wgpu `Surface`,
    /// creating the logical device if needed.
    ///
    /// The result is renderer-agnostic: the swapchain, the configuration it was
    /// brought up with, and the resolved alpha facts. Whatever per-frame render
    /// path a renderer pairs with it is that renderer's own business — see
    /// `frust_render::context`'s `EngineSurface`.
    ///
    /// Hidden from the rendered docs for the same reason as [`Self::instance`].
    #[doc(hidden)]
    pub async fn create_render_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
        alpha: SurfaceAlphaRequest,
    ) -> Result<ConfiguredSurface> {
        self.ensure_device(&surface).await?;
        let handle = self.device_handle();

        let capabilities = surface.get_capabilities(&handle.adapter);
        // Resolved from the request and the surface's reported caps alone; the
        // renderer above reads it back off the returned configuration to pick
        // its own per-frame path.
        let alpha_mode = resolve_alpha_mode(alpha, &capabilities);
        // Whichever supported format the surface reports FIRST — the surface's
        // own preference order, not this module's (see `select_surface_format`).
        let format = select_surface_format(&capabilities)?;

        Ok(ConfiguredSurface::configure(
            surface,
            &handle.device,
            format,
            alpha_mode,
            (width, height),
            present_mode,
        ))
    }
}

/// Selects an adapter, probes it, and requests the logical device — the body
/// both [`RenderContext::ensure_device`] and
/// [`RenderContext::ensure_device_headless`] share.
///
/// `compatible_surface` filters adapter selection to one that can present to the
/// given surface; `None` (the pre-init path, and every headless caller) selects
/// wgpu's default adapter — see the singular-adapter note on
/// `ensure_device_headless`. The capability probe, limits mitigation, feature
/// request and uncaptured-error handler are identical either way: the surface
/// only ever affected adapter selection, never the device it yields.
///
/// A free function rather than a method so it borrows the instance and options
/// separately from the `device` field the callers assign into.
async fn create_device(
    instance: &wgpu::Instance,
    options: &ContextOptions,
    compatible_surface: Option<&wgpu::Surface<'static>>,
) -> Result<DeviceHandle> {
    let adapter = wgpu::util::initialize_adapter_from_env_or_default(instance, compatible_surface)
        .await
        .map_err(|e| anyhow!("frust-gpu: no compatible GPU adapter: {e}"))?;

    let caps = TierCaps::probe(&adapter);

    // The device request is built from the resolved downlevel profile, not
    // unconditionally from the adapter's raw limits — see `base_device_limits`
    // for the shared derivation, and `effective_limits` for the iOS Simulator
    // alignment mitigation layered on top of it.
    let base_limits = base_device_limits(caps.downlevel_profile, adapter.limits());
    let required_limits = effective_limits(base_limits, is_ios_simulator());
    let required_features =
        device_features(adapter.features(), &caps, cfg!(feature = "perf-trace"));

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(&options.device_label),
            required_features,
            required_limits,
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow!("frust-gpu: failed to create GPU device: {e}"))?;

    // Route wgpu's uncaptured errors to the log and a latch instead of its
    // default handler, which aborts the process by panicking ("handling wgpu
    // errors as fatal by default"). A UI framework must survive a driver's
    // *transient* GPU error and recover on a later frame rather than crash —
    // e.g. the Android emulator's SwiftShader path can raise a one-off
    // swapchain-acquire validation error under load, which used to wedge the
    // app into a per-frame panic loop (the surface stayed ready and every
    // subsequent render re-hit the fatal handler). Pairing this with the
    // `Invalid`-acquire → reconfigure recovery (see [`crate::lifecycle`]) lets
    // the swapchain rebuild and rendering resume. Across a mobile FFI boundary
    // the default handler's abort means killing the host app outright. Genuine
    // API misuse is still surfaced — loudly, at error level — just without
    // killing the process.
    //
    // Two pieces of state, both per-device (captured fresh each time this
    // closure is installed, i.e. once per logical device) and both behind `Arc`
    // because `on_uncaptured_error`'s handler must be `Fn`, not `FnMut`:
    //
    // - `error_count` drives the log latch ([`decide_log_action`]), so a device
    //   wedged in a genuine per-frame error storm (as opposed to a one-off
    //   driver hiccup) cannot flood the log forever.
    // - `first_error` latches the first error's text for the frame loop to
    //   poll via [`DeviceHandle::first_uncaptured_error`]. `OnceLock` gives
    //   exactly first-write-wins with no lock held across the handler body.
    let error_count = Arc::new(AtomicU32::new(0));
    let first_error: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let latch = Arc::clone(&first_error);
    device.on_uncaptured_error(Arc::new(move |error| {
        let count = error_count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = latch.set(error.to_string());
        match decide_log_action(count) {
            LogAction::Log => {
                log::error!("frust-gpu: uncaptured wgpu error: {error}");
            }
            LogAction::SuppressionNotice => {
                log::error!(
                    "frust-gpu: further uncaptured wgpu errors suppressed \
                     (total so far: {count})"
                );
            }
            LogAction::Silent { debug_bump } => {
                if debug_bump {
                    log::debug!(
                        "frust-gpu: uncaptured wgpu error count now {count} \
                         (still suppressed)"
                    );
                }
            }
        }
    }));

    Ok(DeviceHandle {
        adapter,
        device,
        queue,
        caps,
        first_uncaptured_error: first_error,
    })
}

/// Given the resolved downlevel profile and the adapter's own reported
/// limits, the `wgpu::Limits` a device request should ask for — the pure half
/// of the derivation this module's `create_device` uses, extracted so both it
/// and [`test_device_limits`] share exactly one implementation rather than two
/// that could drift apart.
///
/// Under `DownlevelProfile::WebGl2` (a real `Gl` backend, or
/// `FRUST_ENGINE_DOWNLEVEL=1` rehearsing it) this asks for the GLES-3.0/WebGL2
/// downlevel default shape — or the override would only relabel a full
/// desktop device rather than actually exercising it — with
/// `using_resolution` folding in `adapter_limits`' own texture-dimension
/// limits so the request never asks for a resolution the adapter cannot
/// satisfy (the swapchain may need more than the downlevel default allows)
/// while every other WebGL2 default limit is requested as-is. Under
/// `DownlevelProfile::Full` — every shipping device — `adapter_limits` is
/// returned unchanged: feeding the *adapter's* limits (rather than
/// `Limits::default()`) is what keeps the request from over-asking and
/// failing on a constrained mobile adapter — or on the iOS Simulator, whose
/// macOS-Metal-backed device refuses `Limits::default()` outright.
///
/// Pure decision logic over plain values, mirroring [`effective_limits`]'s
/// split of pure decision vs. platform lookup: `adapter_limits` is already
/// `wgpu::Adapter::limits()`'s output by the time this runs, so the function
/// itself needs no adapter and is unit-testable on any host with no GPU.
fn base_device_limits(profile: DownlevelProfile, adapter_limits: wgpu::Limits) -> wgpu::Limits {
    if profile == DownlevelProfile::WebGl2 {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits)
    } else {
        adapter_limits
    }
}

/// The `wgpu::Limits` a test fixture's own `request_device` call should ask
/// for, given `adapter` and its already-probed `caps` — exactly the
/// derivation [`create_device`] uses for the production device request
/// (shared via [`base_device_limits`] plus [`effective_limits`]), so a
/// fixture requesting these limits can never over-ask relative to what the
/// production path would request for the very same adapter.
///
/// This is the fix for the iOS Simulator's constrained Apple2 Metal profile
/// (15 inter-stage shader variables, where `wgpu::Limits::default()` demands
/// 16): a fixture that hard-codes `Limits::default()` panics with
/// `LimitsExceeded` there even though the production path — which always
/// requests the adapter's own limits — never would. Every fixture already
/// probes `TierCaps::probe(&adapter)` before requesting its device, so `caps`
/// is available at the same call site this replaces.
///
/// Public (not test-only/`#[cfg(test)]`) because the fixtures that need it
/// live in other crates' `tests/` integration binaries and
/// `frust-testing`'s own `EngineOracle`, none of which can reach a
/// `#[cfg(test)]` item in this crate.
pub fn test_device_limits(adapter: &wgpu::Adapter, caps: &TierCaps) -> wgpu::Limits {
    let base_limits = base_device_limits(caps.downlevel_profile, adapter.limits());
    effective_limits(base_limits, is_ios_simulator())
}

/// Given the build-config-derived instance flags and whether the process is
/// currently running on an Android emulator, decides the flags wgpu's
/// `Instance` should actually be created with.
///
/// The `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set object-name
/// labels via `vkSetDebugUtilsObjectNameEXT`, and the emulator's gfxstream
/// Vulkan HAL (`vulkan.ranchu.so`) segfaults inside that entry point during
/// adapter enumeration (observed crash: `#00 vulkan.ranchu.so
/// vk_common_SetDebugUtilsObjectNameEXT`) — the same class of debug-utils
/// fragility a MoltenVK Vulkan backend is also known to have. Debug object
/// labels are only a developer convenience, so dropping them on the emulator
/// keeps GPU bring-up alive there while leaving physical devices' validation
/// safety net — and desktop behaviour — untouched.
///
/// Pure decision logic, kept separate from the platform property lookup in
/// `is_android_emulator` so it is unit-testable on any host without an Android
/// target.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn effective_instance_flags(flags: wgpu::InstanceFlags, is_emulator: bool) -> wgpu::InstanceFlags {
    if is_emulator {
        flags - (wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION)
    } else {
        flags
    }
}

/// Detects whether the current process is running on an Android emulator
/// (goldfish/ranchu), as opposed to a physical device, via the standard
/// `ro.kernel.qemu` system property (`"1"` on emulators, unset/absent on real
/// hardware). A failed property read is treated as "not an emulator" so
/// physical devices — and any environment where the property cannot be read —
/// default to keeping validation on.
#[cfg(target_os = "android")]
fn is_android_emulator() -> bool {
    android_system_properties::AndroidSystemProperties::new()
        .get("ro.kernel.qemu")
        .as_deref()
        == Some("1")
}

/// The uniform-buffer offset alignment the iOS Simulator's Metal validation
/// actually enforces, regardless of what the adapter reports.
const IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT: u32 = 256;

/// Given a base `wgpu::Limits` and whether the process is currently running on
/// an iOS Simulator, decides the `Limits` a device request should actually use.
///
/// Mitigates [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057): the iOS
/// Simulator is macOS-Metal-backed and requires 256-byte
/// `min_uniform_buffer_offset_alignment`, but wgpu's Metal backend reports the
/// (lower) iOS-device value, which trips Metal API validation on the simulator.
/// Physical iOS devices are unaffected and pass `base` through unchanged; a
/// `base` whose alignment is already at or above 256 is left alone, never
/// lowered.
///
/// Upstream [gfx-rs/wgpu PR #10189](https://github.com/gfx-rs/wgpu/pull/10189)
/// makes this unnecessary — drop it once a pinned wgpu release contains it.
///
/// Pure decision logic, mirroring `effective_instance_flags`'s split of pure
/// decision vs. platform lookup. It is fed the profile-resolved base limits
/// (see this module's `create_device`) — the adapter's own limits under
/// `DownlevelProfile::Full`, the WebGL2 downlevel defaults resolution-folded
/// with the adapter otherwise — so the device request never over-asks.
pub fn effective_limits(base: wgpu::Limits, is_ios_simulator: bool) -> wgpu::Limits {
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

/// Whether this binary is running on the iOS Simulator
/// (`aarch64-apple-ios-sim` / `x86_64-apple-ios` under the simulator), which
/// sets `target_abi = "sim"`. Compile-time constant: the simulator mitigation
/// only needs to apply to simulator builds, never physical-device or desktop
/// ones.
pub const fn is_ios_simulator() -> bool {
    cfg!(all(target_os = "ios", target_abi = "sim"))
}

/// The `wgpu::Features` a device request opportunistically asks for when the
/// adapter exposes them.
///
/// `PIPELINE_CACHE` alone: it is what
/// [`create_pipeline_cache`](RenderContext::create_pipeline_cache) needs to
/// seed the renderer's shader-pipeline compilation from a persisted blob. An
/// optional feature is only ever *added* when the adapter already offers it,
/// so this can never turn a working adapter into a failed device request.
pub fn optional_device_features() -> wgpu::Features {
    wgpu::Features::PIPELINE_CACHE
}

/// The `wgpu::Features` a device request must genuinely *require*, given what
/// the adapter reported ([`TierCaps`]) and whether this build compiled the
/// `perf-trace` feature in.
///
/// The policy is deliberately minimal: **empty** by default. A required feature
/// is a hard device-creation failure on any adapter lacking it, so asking for
/// something the crate does not actually need converts a working device into no
/// device at all. `TIMESTAMP_QUERY` is the single exception — it is what the
/// GPU timing probes ([`crate::diag::TimestampRing`]) are built on, so a
/// `perf-trace` build asks for it, and even then only when the adapter offers
/// it. A device that did not get the feature leaves the ring inert (`gpu_q=0`),
/// exactly as a build without `perf-trace` does.
///
/// A plain `bool` parameter rather than reading `cfg!` internally, so both
/// branches are unit-testable regardless of which features this crate was
/// compiled with.
fn required_features(caps: &TierCaps, perf_trace: bool) -> wgpu::Features {
    if perf_trace && caps.has_timestamp_query {
        wgpu::Features::TIMESTAMP_QUERY
    } else {
        wgpu::Features::empty()
    }
}

/// The complete `required_features` set a device request is made with: the
/// opportunistic set ([`optional_device_features`]) narrowed to what
/// `adapter_features` actually offers, plus the genuinely required set
/// ([`required_features`]).
///
/// Both halves are adapter-conditioned, so this can never turn a working
/// adapter into a failed device request — which is why the policy is split out
/// of [`create_device`] as a pure function over plain values.
fn device_features(
    adapter_features: wgpu::Features,
    caps: &TierCaps,
    perf_trace: bool,
) -> wgpu::Features {
    (adapter_features & optional_device_features()) | required_features(caps, perf_trace)
}

/// Number of uncaptured `wgpu` errors logged at error level per device before
/// the handler latches into suppression. A single flaky frame under a driver
/// hiccup (e.g. the Android emulator's SwiftShader path) is expected to surface
/// a handful of errors; past this the process is either wedged in a genuine
/// per-frame error storm or the driver is fundamentally broken, and re-logging
/// every single one would flood the log without adding information.
const MAX_LOGGED_UNCAPTURED_ERRORS: u32 = 5;

/// How often (in error count) a latched handler bumps a debug-level "still
/// happening" line once past [`MAX_LOGGED_UNCAPTURED_ERRORS`] and the one
/// suppression notice. Debug level (not error) because this is diagnostic noise
/// for someone actively investigating, not an actionable signal.
const UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD: u32 = 100;

/// What the uncaptured-error handler should do for the `count`-th uncaptured
/// error (1-indexed) it has observed on a given device.
///
/// Also the latch the renderer above reuses for its own per-frame event that
/// can reproduce every vsync (an engine frame refusal), so the two report at
/// the same cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogAction {
    /// One of the first `MAX_LOGGED_UNCAPTURED_ERRORS`: log the error itself
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
///
/// Split out of the handler closure in this module's `create_device` so the
/// discipline — log the first few, announce the latch once, then go quiet
/// except an occasional debug bump — is unit-testable without a GPU or a real
/// `wgpu::Error`.
pub fn decide_log_action(count: u32) -> LogAction {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::DownlevelProfile;

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

    #[test]
    fn default_build_requires_no_device_features() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert!(caps.has_timestamp_query, "fixture precondition");
        assert_eq!(required_features(&caps, false), wgpu::Features::empty());
    }

    #[test]
    fn perf_trace_build_requires_timestamp_query_when_offered() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert_eq!(
            required_features(&caps, true),
            wgpu::Features::TIMESTAMP_QUERY
        );
    }

    #[test]
    fn perf_trace_build_requires_nothing_when_adapter_lacks_timestamp_query() {
        // An adapter that cannot do timestamp queries must still yield a
        // device: a required feature it lacks would fail the request outright,
        // so `perf-trace` degrades to no probes rather than to no GPU.
        let caps = TierCaps::fake(DownlevelProfile::WebGl2);
        assert!(!caps.has_timestamp_query, "fixture precondition");
        assert_eq!(required_features(&caps, true), wgpu::Features::empty());
    }

    #[test]
    fn pipeline_cache_is_requested_whenever_the_adapter_offers_it() {
        // The shipped Android/Vulkan warm-start path: `PIPELINE_CACHE` is
        // opportunistically added, which is what makes
        // `RenderContext::pipeline_cache_supported` true there and the
        // persisted-blob seed possible at all. Dropping it would silently kill
        // the warm start on every Vulkan adapter.
        let caps = TierCaps::fake(DownlevelProfile::Full);
        let adapter = wgpu::Features::PIPELINE_CACHE | wgpu::Features::DEPTH_CLIP_CONTROL;
        assert_eq!(
            device_features(adapter, &caps, false),
            wgpu::Features::PIPELINE_CACHE,
            "an offered optional feature is taken, and nothing else is"
        );
    }

    #[test]
    fn an_adapter_without_pipeline_cache_is_never_asked_for_it() {
        // Metal/DX12: the feature is absent, so the request must not name it —
        // a required feature the adapter lacks is a hard device-creation
        // failure, i.e. no GPU at all rather than merely no persisted cache.
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert_eq!(
            device_features(wgpu::Features::empty(), &caps, false),
            wgpu::Features::empty()
        );
    }

    #[test]
    fn a_perf_trace_build_asks_for_both_halves_when_both_are_offered() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert!(caps.has_timestamp_query, "fixture precondition");
        assert_eq!(
            device_features(wgpu::Features::PIPELINE_CACHE, &caps, true),
            wgpu::Features::PIPELINE_CACHE | wgpu::Features::TIMESTAMP_QUERY
        );
    }

    #[test]
    fn base_device_limits_full_profile_passes_adapter_limits_through_unclamped() {
        // The iOS Simulator shape: an adapter reporting fewer inter-stage
        // shader variables than `wgpu::Limits::default()` demands (15 vs 16).
        // Under `Full`, `base_device_limits` must request no more than what
        // the adapter itself reported, never `Limits::default()`.
        let adapter_limits = wgpu::Limits {
            max_inter_stage_shader_variables: 15,
            ..wgpu::Limits::default()
        };
        let limits = base_device_limits(DownlevelProfile::Full, adapter_limits.clone());
        assert!(limits.max_inter_stage_shader_variables <= 15);
        assert_eq!(limits, adapter_limits);
    }

    #[test]
    fn base_device_limits_webgl2_profile_never_exceeds_the_constrained_adapter() {
        // The WebGL2 downlevel-default shape already asks for 15 inter-stage
        // shader variables (lower than `Limits::default()`'s 16), so folding
        // in a constrained adapter's own limits must still land at or below
        // what that adapter reports.
        let adapter_limits = wgpu::Limits {
            max_inter_stage_shader_variables: 15,
            ..wgpu::Limits::default()
        };
        let limits = base_device_limits(DownlevelProfile::WebGl2, adapter_limits);
        assert!(limits.max_inter_stage_shader_variables <= 15);
    }

    #[test]
    fn default_options_label_the_device_and_leave_backends_to_the_environment() {
        let options = ContextOptions::default();
        assert_eq!(options.device_label, DEFAULT_DEVICE_LABEL);
        assert_eq!(options.backends, None);
    }

    #[test]
    fn a_fresh_context_has_no_device_and_therefore_no_caps() {
        // Construction must not enumerate adapters, so this is a host test, not
        // a GPU one: it passes on a machine with no usable GPU at all.
        let context = RenderContext::new();
        assert!(context.caps().is_none());
    }

    #[test]
    #[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu -- --ignored` \
                (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
    fn gpu_device_creation_reports_adapter_caps() {
        pollster::block_on(async {
            let mut context = RenderContext::new();
            let handle = context.device().await.expect("device creation");

            let info = handle.adapter.get_info();
            println!(
                "frust-gpu adapter: name={:?} backend={:?} device_type={:?} \
                 driver={:?} driver_info={:?} vendor={:#06x} device={:#06x}",
                info.name,
                info.backend,
                info.device_type,
                info.driver,
                info.driver_info,
                info.vendor,
                info.device
            );
            println!("frust-gpu caps: {:#?}", handle.caps);
            println!(
                "frust-gpu device limits: max_texture_dimension_2d={} \
                 min_uniform_buffer_offset_alignment={}",
                handle.device.limits().max_texture_dimension_2d,
                handle.device.limits().min_uniform_buffer_offset_alignment
            );

            assert_eq!(handle.caps.adapter_name, info.name);
            assert_eq!(handle.caps.backend, info.backend);
            assert!(!handle.caps.adapter_name.is_empty());
            assert!(handle.caps.max_texture_dimension_2d > 0);
            assert!(handle.caps.resource_texture_dim > 0);
            // Nothing has been submitted, so the latch must still be empty.
            assert_eq!(handle.first_uncaptured_error(), None);

            // With `FRUST_ENGINE_DOWNLEVEL=1` set for the whole test process
            // (the override is read once and cached in a `OnceLock`), both the
            // probed caps and the created device must actually report the
            // clamped WebGL2 shape rather than a desktop backend merely
            // relabelled `WebGl2`. Without it, this rig's real backend
            // (Vulkan/Metal/Dx12) must report `Full` unchanged, proving the
            // knob rehearses the downlevel shape rather than always forcing
            // it.
            let downlevel_env_set = std::env::var("FRUST_ENGINE_DOWNLEVEL").is_ok_and(|v| v != "0");
            if downlevel_env_set {
                assert_eq!(handle.caps.downlevel_profile, DownlevelProfile::WebGl2);
                assert!(!handle.caps.has_storage_buffers);
                assert!(handle.caps.max_texture_dimension_2d <= 2048);
                assert_eq!(handle.caps.min_uniform_buffer_offset_alignment, 256);
                assert!(handle.device.limits().max_texture_dimension_2d > 0);
            } else {
                assert_eq!(handle.caps.downlevel_profile, DownlevelProfile::Full);
            }

            let caps = context.caps().cloned().expect("caps after device creation");
            assert_eq!(caps.adapter_name, info.name);

            // The device is created once and reused: a second call must hand
            // back the same logical device, not build another one.
            let again = context.device().await.expect("device reuse");
            assert_eq!(again.caps, caps);
        });
    }
}
