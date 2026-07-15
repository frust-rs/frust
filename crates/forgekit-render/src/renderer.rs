//! [`SurfaceRenderer`]: renders a [`forgekit_scene::Scene`] into one window's
//! swapchain each frame.
//!
//! Owns a `vello::Renderer` and a reusable `vello::Scene` alongside the
//! `RenderSurface`. vello 0.9 renders via compute into an intermediate
//! `Rgba8Unorm` texture, so each frame is: encode → `render_to_texture`
//! (intermediate) → blit (intermediate → acquired swapchain view) → present.

use anyhow::{Result, anyhow};

use crate::context::RenderContext;
use crate::convert;

/// Per-surface renderer: a vello `Renderer` + reused `Scene` + the vello
/// `RenderSurface` (swapchain config, intermediate target, blitter).
pub struct SurfaceRenderer {
    surface: vello::util::RenderSurface<'static>,
    renderer: vello::Renderer,
    /// Reused across frames; cleared with `reset()` each frame rather than
    /// reallocated (spec §7).
    scene: vello::Scene,
}

impl SurfaceRenderer {
    /// Wraps a freshly created surface + renderer with an empty reusable scene.
    /// Constructed by [`RenderContext::create_surface`](crate::RenderContext::create_surface).
    pub(crate) fn new(
        surface: vello::util::RenderSurface<'static>,
        renderer: vello::Renderer,
    ) -> Self {
        Self {
            surface,
            renderer,
            scene: vello::Scene::new(),
        }
    }

    /// Resizes the swapchain and intermediate target to `width` x `height`.
    ///
    /// Only the surface config and target texture are recreated — the
    /// `vello::Renderer` (and its compiled shader pipelines) is preserved
    /// across resizes. Zero dimensions are ignored (a minimized window keeps
    /// its last valid size).
    pub fn resize(&mut self, ctx: &mut RenderContext, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        ctx.inner.resize_surface(&mut self.surface, width, height);
    }

    /// Encodes `scene` and presents it to the surface, clearing to `base_color`.
    ///
    /// The internal `vello::Scene` is `reset()` and re-encoded from scratch
    /// every frame; nothing accumulates across calls.
    pub fn render(
        &mut self,
        ctx: &RenderContext,
        scene: &forgekit_scene::Scene,
        base_color: peniko::Color,
    ) -> Result<()> {
        self.scene.reset();
        convert::encode_scene(scene, &mut self.scene);

        let device_handle = &ctx.inner.devices[self.surface.dev_id];
        let params = vello::RenderParams {
            base_color,
            width: self.surface.config.width,
            height: self.surface.config.height,
            antialiasing_method: vello::AaConfig::Area,
        };

        self.renderer
            .render_to_texture(
                &device_handle.device,
                &device_handle.queue,
                &self.scene,
                &self.surface.target_view,
                &params,
            )
            .map_err(|e| anyhow!("vello render_to_texture failed: {e}"))?;

        use wgpu::CurrentSurfaceTexture;
        let surface_texture = match self.surface.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(t) | CurrentSurfaceTexture::Suboptimal(t) => t,
            // Transient states — skip this frame; the shell's next vsync tick
            // retries. Reconfigure when the swapchain is stale/lost so the
            // retry finds a valid surface (spec §8.1 surface-lifecycle intent).
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                ctx.inner.configure_surface(&self.surface);
                return Ok(());
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Validation => {
                return Err(anyhow!("swapchain acquire failed: validation error"));
            }
        };
        let target_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            device_handle
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("forgekit-render blit"),
                });
        self.surface.blitter.copy(
            &device_handle.device,
            &mut encoder,
            &self.surface.target_view,
            &target_view,
        );
        device_handle.queue.submit([encoder.finish()]);
        surface_texture.present();

        Ok(())
    }
}
