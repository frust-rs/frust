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
use vello::util::RenderSurface;
use wgpu::util::TextureBlitter;

/// The features vello's renderer opportunistically uses when the adapter
/// exposes them — mirrors `vello::util::RenderContext::new_device` so our
/// hand-rolled device request stays behaviourally identical to vello's, apart
/// from the `required_limits` we control.
fn vello_optional_features() -> wgpu::Features {
    wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE
}

/// A logical device plus the adapter it came from and the queue that executes
/// its command buffers — the forgekit-owned equivalent of
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
/// (forgekit is single-window); the device is created lazily on the first
/// surface and reused across surface loss/recreation (rotation, backgrounding)
/// since a logical device is display-independent. It is passed into the
/// [`SurfaceRenderer`](crate::SurfaceRenderer) lifecycle and per-frame methods
/// so those operations reach the owning device.
pub struct RenderContext {
    pub(crate) instance: wgpu::Instance,
    /// Lazily created on the first surface; `None` until then.
    pub(crate) device: Option<DeviceHandle>,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self::new()
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

/// The [`wgpu::DownlevelFlags`] the vello 0.9 render pipeline cannot run
/// without. vello unconditionally allocates its working buffers with
/// `BufferUsages::INDIRECT` (see `wgpu_engine.rs` — "TODO: only some buffers
/// will need indirect"), and wgpu rejects creating any INDIRECT-usage buffer
/// on a device whose adapter lacks [`wgpu::DownlevelFlags::INDIRECT_EXECUTION`].
///
/// The **iOS Simulator** is the platform where this bites: wgpu-hal 29 gates
/// `INDIRECT_EXECUTION` on the `iOS_GPUFamily3_v1`/`macOS_GPUFamily1_v1`
/// Metal feature sets, but the simulator only exposes the `Apple2` GPU family,
/// so the flag is never set. This is a wgpu-29/vello-0.9-level simulator
/// limitation (a sibling of [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057))
/// that cannot be patched under the version pin — hence we detect it up front
/// and fail surface creation with a clear diagnostic instead of letting vello
/// panic inside `create_buffer` on every frame.
const REQUIRED_DOWNLEVEL_FLAGS: wgpu::DownlevelFlags = wgpu::DownlevelFlags::INDIRECT_EXECUTION;

/// Pure check: does `flags` include everything the vello pipeline requires?
/// Split out from the adapter query so it is unit-testable without a GPU.
fn supports_required_downlevel_flags(flags: wgpu::DownlevelFlags) -> bool {
    flags.contains(REQUIRED_DOWNLEVEL_FLAGS)
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
    let target_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("forgekit-render vello target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
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
        }
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

    /// Lazily create the logical device compatible with `surface`, requesting
    /// the adapter's own limits (never `Limits::default()`, which the iOS
    /// Simulator cannot satisfy) plus the #7057 alignment mitigation. Reuses
    /// an already-created device when it is compatible with `surface`, so
    /// surface loss/recreation (rotation, backgrounding) never rebuilds it.
    async fn ensure_device(&mut self, surface: &wgpu::Surface<'static>) -> Result<()> {
        if let Some(existing) = &self.device
            && existing.adapter.is_surface_supported(surface)
        {
            return Ok(());
        }
        let adapter =
            wgpu::util::initialize_adapter_from_env_or_default(&self.instance, Some(surface))
                .await
                .map_err(|e| anyhow!("forgekit-render: no compatible GPU adapter: {e}"))?;

        // Fail fast (with a diagnostic) if the adapter cannot run the vello
        // pipeline, instead of letting vello panic in `create_buffer` every
        // frame. The iOS Simulator is the known offender — see
        // `REQUIRED_DOWNLEVEL_FLAGS`.
        let downlevel = adapter.get_downlevel_capabilities();
        if !supports_required_downlevel_flags(downlevel.flags) {
            let missing = REQUIRED_DOWNLEVEL_FLAGS - downlevel.flags;
            return Err(anyhow!(
                "forgekit-render: GPU adapter `{}` lacks downlevel flags required by the vello \
                 renderer ({missing:?}); this is the known wgpu-29/vello-0.9 iOS Simulator \
                 limitation (INDIRECT_EXECUTION is unavailable on the simulator's Apple2 GPU \
                 family). Run on a physical device.",
                adapter.get_info().name,
            ));
        }

        let required_features = adapter.features() & vello_optional_features();
        let required_limits = effective_limits(adapter.limits(), is_ios_simulator());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("forgekit-render device"),
                required_features,
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("forgekit-render: failed to create GPU device: {e}"))?;

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
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            log::error!("forgekit-render: uncaptured wgpu error: {error}");
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
            .ok_or_else(|| anyhow!("forgekit-render: no supported surface format (Rgba8/Bgra8)"))?;

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
            // forgekit reads the device from `self.device` instead. Left at 0.
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
            .map_err(|e| anyhow!("forgekit-render: failed to create surface: {e}"))?;
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
    fn downlevel_check_requires_indirect_execution() {
        // A fully-capable adapter passes.
        assert!(supports_required_downlevel_flags(
            wgpu::DownlevelFlags::all()
        ));
        // The iOS Simulator shape: everything except INDIRECT_EXECUTION.
        let sim_flags = wgpu::DownlevelFlags::all() - wgpu::DownlevelFlags::INDIRECT_EXECUTION;
        assert!(!supports_required_downlevel_flags(sim_flags));
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
}
