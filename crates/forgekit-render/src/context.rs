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
    pub fn new() -> Self {
        Self {
            inner: vello::util::RenderContext::new(),
        }
    }
}
