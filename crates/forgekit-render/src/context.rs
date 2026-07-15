//! [`RenderContext`]: owns the wgpu instance/device(s) that surfaces render on.
//!
//! Thin wrapper over `vello::util::RenderContext`, which already implements the
//! adapter/device-per-surface bookkeeping and the intermediate-texture + blit
//! presentation model vello 0.9 requires (there is no `render_to_surface`).
//!
//! Surface creation moved onto the lifecycle state machine in
//! [`crate::SurfaceRenderer`] (spec §8.1): the shell mints an empty
//! `SurfaceRenderer` and drives it with `on_surface_created`/`on_surface_changed`/
//! `on_surface_destroyed`, each of which reaches back into this context for the
//! owning device.

/// Owns the wgpu `Instance` and the pool of devices vello renders with.
///
/// A single `RenderContext` is shared across all surfaces/windows a shell
/// creates; it is passed into the [`SurfaceRenderer`](crate::SurfaceRenderer)
/// lifecycle and per-frame methods so those operations reach the owning device.
pub struct RenderContext {
    pub(crate) inner: vello::util::RenderContext,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderContext {
    /// Creates a context with a fresh wgpu `Instance` and no devices yet
    /// (devices are created lazily on first surface creation).
    ///
    /// Mirrors `vello::util::RenderContext::new()`'s instance setup, except on
    /// Android it strips the `DEBUG`/`VALIDATION` instance flags (which
    /// `InstanceFlags::from_build_config()` turns on in debug builds). The
    /// `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set object-name
    /// labels via `vkSetDebugUtilsObjectNameEXT`, and the Android emulator's
    /// gfxstream Vulkan HAL (`vulkan.ranchu.so`) segfaults inside that entry
    /// point — the same class of debug-utils fragility the RESEARCH.md MoltenVK
    /// caveat warns about. Debug object labels are only a developer convenience,
    /// so dropping them on Android is a safe way to keep GPU bring-up alive on
    /// the emulator (and MoltenVK) while leaving desktop behavior untouched.
    pub fn new() -> Self {
        let backends = wgpu::Backends::from_env().unwrap_or_default();
        #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
        let mut flags = wgpu::InstanceFlags::from_build_config().with_env();
        #[cfg(target_os = "android")]
        {
            // Strip DEBUG/VALIDATION on Android (see the doc comment above). This
            // is needed even though rendering may end up on GLES: wgpu still
            // *enumerates* every backend's adapters at instance/adapter-request
            // time, and the emulator's Vulkan HAL crashes in
            // `vkSetDebugUtilsObjectNameEXT` during that enumeration when DEBUG is
            // set — before any backend is ultimately chosen.
            //
            // Backend selection is deliberately left to wgpu's default
            // (`Backends::default()`): on the Apple-Silicon emulator that resolves
            // to the software GLES/MESA path, which renders correctly; on real
            // devices it resolves to hardware Vulkan. Forcing Vulkan here would
            // route the emulator through its gfxstream/MoltenVK Vulkan driver,
            // which segfaults in `gfxstream::vk::ResourceTracker::on_vkQueueSubmit`
            // (RESEARCH.md's MoltenVK caveat) — so we do NOT force it.
            flags.remove(wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION);
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends,
            flags,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        Self {
            inner: vello::util::RenderContext {
                instance,
                devices: Vec::new(),
            },
        }
    }
}
