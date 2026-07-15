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

impl RenderContext {
    /// Creates a context with a fresh wgpu `Instance` and no devices yet
    /// (devices are created lazily on first surface creation).
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
            inner: vello::util::RenderContext {
                instance,
                devices: Vec::new(),
            },
        }
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
}
