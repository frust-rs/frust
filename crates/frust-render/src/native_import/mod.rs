//! Platform importers for the external-texture frame bus: the per-platform
//! [`NativeImporter`](crate::texture_bus::NativeImporter) implementations that
//! wrap a `frust-gpu` zero-copy import (a CVPixelBuffer on Apple, an
//! AHardwareBuffer on Android) so a plugin's native frame draws as a scene
//! texture without a copy. Each platform half lives behind its own `cfg`; on
//! every other target the module is empty and [`install`] is a no-op.

#[cfg(target_vendor = "apple")]
pub mod apple;

#[cfg(target_os = "android")]
pub mod android;

/// Install the importers this platform supports for the given render context,
/// once per process. Safe to call every frame; later calls do nothing.
pub fn install(context: &frust_gpu::RenderContext) {
    #[cfg(target_os = "android")]
    android::install(context);
    #[cfg(target_vendor = "apple")]
    apple::install(context);
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    let _ = context;
}
