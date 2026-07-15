//! [`SurfaceRenderer`]: the surface-lifecycle state machine (spec §8.1) that
//! renders a [`forgekit_scene::Scene`] into one window's swapchain each frame.
//!
//! Wraps the pure lifecycle logic in [`crate::lifecycle`] around the actual
//! `wgpu`/vello resources: it starts in [`SurfacePhase::NoSurface`], becomes
//! renderable on `on_surface_created`, resizes on `on_surface_changed`, tears
//! down on `on_surface_destroyed`, and drops to [`SurfacePhase::SurfaceLost`]
//! when the swapchain reports `Lost` mid-frame.
//!
//! vello 0.9 renders via compute into an intermediate `Rgba8Unorm` texture, so
//! each frame is: encode → `render_to_texture` (intermediate) → blit
//! (intermediate → acquired swapchain view) → present.

use core::ffi::c_void;

use anyhow::{Result, anyhow};

use crate::context::RenderContext;
use crate::convert;
use crate::lifecycle::{
    AcquireAction, AcquireStatus, FrameOutcome, SurfaceEvent, SurfacePhase, decide_acquire,
    next_phase,
};

/// The live GPU resources of a [`SurfacePhase::SurfaceReady`] surface.
///
/// The `vello::Renderer` is created per surface (it is device-bound) and, with
/// the `RenderSurface`, is dropped on every transition out of `SurfaceReady`,
/// upholding the "no `SurfaceTexture`/surface outlives a transition" invariant.
struct ReadySurface {
    surface: vello::util::RenderSurface<'static>,
    /// Reused across frames while the surface lives; its compiled shader
    /// pipelines survive resizes (which recreate only the swapchain/target).
    renderer: vello::Renderer,
}

/// The surface half of the lifecycle machine, parallel to [`SurfacePhase`].
///
/// `Ready` is boxed: the live GPU resources dwarf the empty variants, and
/// boxing keeps the common `NoSurface`/`Lost` states cheap to move.
enum SurfaceState {
    NoSurface,
    Ready(Box<ReadySurface>),
    Lost,
}

/// Per-surface renderer and lifecycle state machine (spec §8.1).
///
/// Holds a device-independent, reusable `vello::Scene` (so it survives surface
/// transitions) plus the current [`SurfaceState`]. Constructed empty with
/// [`SurfaceRenderer::new`]; the shell brings it online with
/// [`on_surface_created`](Self::on_surface_created).
pub struct SurfaceRenderer {
    /// Reused across frames; `reset()` each frame rather than reallocated
    /// (spec §7). Device-independent, so it outlives surface transitions.
    scene: vello::Scene,
    state: SurfaceState,
}

impl Default for SurfaceRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceRenderer {
    /// Creates an empty renderer in [`SurfacePhase::NoSurface`].
    ///
    /// No GPU work happens here; the surface is created later by the shell via
    /// [`on_surface_created`](Self::on_surface_created) once a window/surface
    /// exists (deferred window creation on desktop, `surfaceCreated` on Android).
    pub fn new() -> Self {
        Self {
            scene: vello::Scene::new(),
            state: SurfaceState::NoSurface,
        }
    }

    /// The current lifecycle phase (spec §8.1).
    pub fn phase(&self) -> SurfacePhase {
        match self.state {
            SurfaceState::NoSurface => SurfacePhase::NoSurface,
            SurfaceState::Ready(_) => SurfacePhase::SurfaceReady,
            SurfaceState::Lost => SurfacePhase::SurfaceLost,
        }
    }

    /// Brings the surface online (`surfaceCreated`/`resumed`): creates the
    /// swapchain and a device-bound `vello::Renderer`, transitioning to
    /// [`SurfacePhase::SurfaceReady`].
    ///
    /// Valid from any phase — calling it in `SurfaceLost` is how the shell
    /// recovers, and calling it in `SurfaceReady` replaces the surface (the old
    /// one is dropped first). `window` is any raw window handle the shell owns
    /// (`wgpu::SurfaceTarget`); no `winit` dependency is imposed here.
    ///
    /// Presentation uses vsync (`PresentMode::AutoVsync`), matching the
    /// vsync-driven frame pacing the platform shells provide (spec §8).
    pub async fn on_surface_created(
        &mut self,
        ctx: &mut RenderContext,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let surface = ctx
            .inner
            .create_surface(
                window,
                width.max(1),
                height.max(1),
                wgpu::PresentMode::AutoVsync,
            )
            .await
            .map_err(|e| anyhow!("forgekit-render: failed to create render surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Brings the surface online from a raw `ANativeWindow` pointer (Android
    /// `surfaceCreated`), transitioning to [`SurfacePhase::SurfaceReady`].
    ///
    /// Compiled unconditionally so a host `cargo check --target
    /// aarch64-linux-android` covers it. This is one of the framework's
    /// sanctioned unsafe entry points (see also
    /// [`on_surface_created_from_metal_layer`](Self::on_surface_created_from_metal_layer));
    /// the raw-pointer handling is isolated in
    /// [`crate::lifecycle::create_android_surface`].
    ///
    /// # Safety
    ///
    /// `window_ptr` must be a valid, acquired `ANativeWindow*` that outlives the
    /// surface (and all its `SurfaceTexture`s). See
    /// [`crate::lifecycle::create_android_surface`] for the full contract.
    pub async unsafe fn on_surface_created_from_android_window(
        &mut self,
        ctx: &mut RenderContext,
        window_ptr: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<()> {
        // SAFETY: forwarded to the caller's `on_surface_created_from_android_window`
        // contract — `window_ptr` is a valid, acquired ANativeWindow* outliving
        // the surface.
        let raw =
            unsafe { crate::lifecycle::create_android_surface(&ctx.inner.instance, window_ptr) }?;
        let surface = ctx
            .inner
            .create_render_surface(
                raw,
                width.max(1),
                height.max(1),
                wgpu::PresentMode::AutoVsync,
            )
            .await
            .map_err(|e| anyhow!("forgekit-render: failed to configure Android surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Brings the surface online from a raw `CAMetalLayer*` pointer (iOS/macOS
    /// Swift shell surface creation), transitioning to
    /// [`SurfacePhase::SurfaceReady`].
    ///
    /// Only compiled on Apple targets (mirrors [`crate::lifecycle::create_metal_surface`]'s
    /// gating): the raw-pointer handling is isolated there, one of the
    /// framework's sanctioned unsafe boundaries alongside
    /// [`on_surface_created_from_android_window`](Self::on_surface_created_from_android_window).
    ///
    /// Presentation uses `Fifo` — the only present mode guaranteed on
    /// iOS/Metal (spec §8; vsync-equivalent, matching the desktop/Android
    /// `AutoVsync` paths in spirit).
    ///
    /// # Safety
    ///
    /// `layer_ptr` must be a valid, live `CAMetalLayer*` that outlives the
    /// surface (and all its `SurfaceTexture`s). See
    /// [`crate::lifecycle::create_metal_surface`] for the full contract.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    pub async unsafe fn on_surface_created_from_metal_layer(
        &mut self,
        ctx: &mut RenderContext,
        layer_ptr: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<()> {
        // SAFETY: forwarded to the caller's `on_surface_created_from_metal_layer`
        // contract — `layer_ptr` is a valid, live CAMetalLayer* outliving the
        // surface.
        let raw =
            unsafe { crate::lifecycle::create_metal_surface(&ctx.inner.instance, layer_ptr) }?;
        let surface = ctx
            .inner
            .create_render_surface(raw, width.max(1), height.max(1), wgpu::PresentMode::Fifo)
            .await
            .map_err(|e| anyhow!("forgekit-render: failed to configure Metal surface: {e}"))?;
        self.install_surface(ctx, surface)
    }

    /// Wraps a freshly created `RenderSurface` in a device-bound renderer and
    /// installs it as the live surface. Shared by the safe and Android paths.
    fn install_surface(
        &mut self,
        ctx: &RenderContext,
        surface: vello::util::RenderSurface<'static>,
    ) -> Result<()> {
        let device = &ctx.inner.devices[surface.dev_id].device;
        let renderer = vello::Renderer::new(device, vello::RendererOptions::default())
            .map_err(|e| anyhow!("forgekit-render: failed to create vello renderer: {e}"))?;
        debug_assert_eq!(
            next_phase(self.phase(), SurfaceEvent::Created),
            SurfacePhase::SurfaceReady,
            "Created must reach SurfaceReady (spec §8.1)"
        );
        // Dropping the previous `SurfaceState` here tears down any prior surface
        // before the new one goes live (spec §8.1: no surface outlives a
        // transition).
        self.state = SurfaceState::Ready(Box::new(ReadySurface { surface, renderer }));
        Ok(())
    }

    /// Resizes the swapchain (`surfaceChanged`/`Resized`).
    ///
    /// Only acts in [`SurfacePhase::SurfaceReady`]; a resize with no surface is
    /// dropped. The `vello::Renderer` (and its compiled pipelines) is preserved
    /// — only the surface config and target texture are recreated. Zero
    /// dimensions are ignored (a minimized window keeps its last valid size).
    pub fn on_surface_changed(&mut self, ctx: &RenderContext, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let SurfaceState::Ready(ready) = &mut self.state {
            ctx.inner.resize_surface(&mut ready.surface, width, height);
        }
    }

    /// Tears the surface down (`surfaceDestroyed`/`suspended`), transitioning to
    /// [`SurfacePhase::NoSurface`].
    ///
    /// Drops the `RenderSurface` (and its renderer) so no surface or texture
    /// outlives the platform's underlying window (spec §8.1). Idempotent.
    pub fn on_surface_destroyed(&mut self) {
        debug_assert_eq!(
            next_phase(self.phase(), SurfaceEvent::Destroyed),
            SurfacePhase::NoSurface,
            "Destroyed must reach NoSurface (spec §8.1)"
        );
        self.state = SurfaceState::NoSurface;
    }

    /// Encodes `scene` and presents it, clearing to `base_color`; returns the
    /// [`FrameOutcome`] so the shell can react.
    ///
    /// In [`SurfacePhase::NoSurface`]/[`SurfacePhase::SurfaceLost`] the frame is
    /// dropped ([`FrameOutcome::Skipped`]) — never panicking, never queueing
    /// (spec §8.1). On an `Outdated` acquire the surface is reconfigured and
    /// [`FrameOutcome::Redraw`] asks the shell to try again; on `Lost` the
    /// surface is dropped, the machine moves to [`SurfacePhase::SurfaceLost`],
    /// and [`FrameOutcome::SurfaceLost`] tells the shell to recreate it.
    ///
    /// The internal `vello::Scene` is `reset()` and re-encoded every frame;
    /// nothing accumulates across calls.
    pub fn render(
        &mut self,
        ctx: &RenderContext,
        scene: &forgekit_scene::Scene,
        base_color: peniko::Color,
    ) -> Result<FrameOutcome> {
        // Frames are dropped in every phase but SurfaceReady (spec §8.1).
        if !self.phase().can_render() {
            return Ok(FrameOutcome::Skipped);
        }
        // Disjoint field borrows: the reusable scene and the live surface.
        let Self {
            scene: vello_scene,
            state,
        } = self;
        let SurfaceState::Ready(ready) = state else {
            // Unreachable: `can_render()` above guaranteed SurfaceReady.
            return Ok(FrameOutcome::Skipped);
        };

        vello_scene.reset();
        convert::encode_scene(scene, vello_scene);

        let device_handle = &ctx.inner.devices[ready.surface.dev_id];
        let params = vello::RenderParams {
            base_color,
            width: ready.surface.config.width,
            height: ready.surface.config.height,
            antialiasing_method: vello::AaConfig::Area,
        };
        ready
            .renderer
            .render_to_texture(
                &device_handle.device,
                &device_handle.queue,
                vello_scene,
                &ready.surface.target_view,
                &params,
            )
            .map_err(|e| anyhow!("forgekit-render: vello render_to_texture failed: {e}"))?;

        use wgpu::CurrentSurfaceTexture as Cst;
        let acquired = ready.surface.surface.get_current_texture();
        let status = match &acquired {
            Cst::Success(_) | Cst::Suboptimal(_) => AcquireStatus::Usable,
            Cst::Outdated => AcquireStatus::Outdated,
            Cst::Lost => AcquireStatus::Lost,
            Cst::Timeout | Cst::Occluded => AcquireStatus::Transient,
            Cst::Validation => AcquireStatus::Invalid,
        };

        match decide_acquire(status) {
            AcquireAction::Present => {
                let surface_texture = match acquired {
                    Cst::Success(t) | Cst::Suboptimal(t) => t,
                    // `decide_acquire(Usable) == Present`, and only Success/
                    // Suboptimal classify as Usable — so this is unreachable.
                    // Report rather than panic to honour the no-panic invariant.
                    _ => {
                        return Err(anyhow!("forgekit-render: acquire classification desync"));
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
                ready.surface.blitter.copy(
                    &device_handle.device,
                    &mut encoder,
                    &ready.surface.target_view,
                    &target_view,
                );
                device_handle.queue.submit([encoder.finish()]);
                surface_texture.present();
                Ok(FrameOutcome::Rendered)
            }
            AcquireAction::Reconfigure => {
                ctx.inner.configure_surface(&ready.surface);
                Ok(FrameOutcome::Redraw)
            }
            AcquireAction::Lose => {
                debug_assert_eq!(
                    next_phase(SurfacePhase::SurfaceReady, SurfaceEvent::Lost),
                    SurfacePhase::SurfaceLost,
                    "Lost must reach SurfaceLost from SurfaceReady (spec §8.1)"
                );
                // Drop the surface and its outstanding resources before returning
                // (spec §8.1) so the shell can recreate cleanly.
                *state = SurfaceState::Lost;
                Ok(FrameOutcome::SurfaceLost)
            }
            AcquireAction::Skip => Ok(FrameOutcome::Skipped),
            AcquireAction::Fail => Err(anyhow!(
                "forgekit-render: swapchain acquire failed (validation error)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_no_surface() {
        let renderer = SurfaceRenderer::new();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
    }

    #[test]
    fn render_is_skipped_without_a_surface() {
        // No GPU needed: a `NoSurface` renderer short-circuits before any wgpu
        // work. `RenderContext::new()` only builds a wgpu `Instance` (no device).
        let ctx = RenderContext::new();
        let mut renderer = SurfaceRenderer::new();
        let scene = forgekit_scene::Scene::new();

        let outcome = renderer
            .render(&ctx, &scene, peniko::Color::WHITE)
            .expect("render in NoSurface must not error");
        assert_eq!(outcome, FrameOutcome::Skipped);
    }

    #[test]
    fn destroy_is_idempotent_and_resets_to_no_surface() {
        let mut renderer = SurfaceRenderer::new();
        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
        renderer.on_surface_destroyed();
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
    }

    #[test]
    fn resize_without_surface_is_a_no_op() {
        let ctx = RenderContext::new();
        let mut renderer = SurfaceRenderer::new();
        renderer.on_surface_changed(&ctx, 800, 600);
        assert_eq!(renderer.phase(), SurfacePhase::NoSurface);
    }
}
