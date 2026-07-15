//! [`RenderContext`]: owns the wgpu instance/device(s) and mints
//! [`SurfaceRenderer`]s for windows.
//!
//! Thin wrapper over `vello::util::RenderContext`, which already implements the
//! adapter/device-per-surface bookkeeping and the intermediate-texture + blit
//! presentation model vello 0.9 requires (there is no `render_to_surface`).

use anyhow::{Result, anyhow};

use crate::renderer::SurfaceRenderer;

/// Owns the wgpu `Instance` and the pool of devices vello renders with.
///
/// A single `RenderContext` is shared across all surfaces/windows a shell
/// creates; it is passed back into [`SurfaceRenderer::render`] and
/// [`SurfaceRenderer::resize`] so those operations reach the owning device.
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
    /// (devices are created lazily on first [`create_surface`](Self::create_surface)).
    pub fn new() -> Self {
        Self {
            inner: vello::util::RenderContext::new(),
        }
    }

    /// Creates a swapchain-backed renderer for `window` at `width` x `height`.
    ///
    /// `window` is any raw window handle the shell owns (`wgpu::SurfaceTarget`);
    /// no `winit` dependency is imposed here. The returned [`SurfaceRenderer`]
    /// carries its own reusable `vello::Renderer` and `vello::Scene`.
    ///
    /// Presentation uses vsync (`PresentMode::AutoVsync`), matching the
    /// vsync-driven frame pacing the platform shells provide (spec §8).
    pub async fn create_surface(
        &mut self,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<SurfaceRenderer> {
        let surface = self
            .inner
            .create_surface(window, width, height, wgpu::PresentMode::AutoVsync)
            .await
            .map_err(|e| anyhow!("failed to create render surface: {e}"))?;
        let device = &self.inner.devices[surface.dev_id].device;
        let renderer = vello::Renderer::new(device, vello::RendererOptions::default())
            .map_err(|e| anyhow!("failed to create vello renderer: {e}"))?;
        Ok(SurfaceRenderer::new(surface, renderer))
    }
}
