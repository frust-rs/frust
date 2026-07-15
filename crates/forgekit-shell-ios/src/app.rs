//! The iOS app runtime: [`IosAppHandle`], the state behind the opaque C handle.
//!
//! This module is `#[cfg(target_os = "ios")]`; it owns the same resources the
//! desktop shell's `ShellHandler` and the Android shell's `AndroidAppHandle` do —
//! a [`RenderContext`], [`SurfaceRenderer`], [`TextContext`], reusable [`Scene`],
//! plus the app tree — but is driven by the generated Swift app's
//! `CADisplayLink`-posted `forgekit_render_frame` calls instead of a winit loop
//! or Choreographer. It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::ffi_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](forgekit_shell_common::AppTree)) is platform-agnostic and lives
//! in `forgekit-shell-common`.
//!
//! # Layer lifetime contract
//!
//! Unlike the Android handle, this handle owns **no** window/layer field: the
//! `CAMetalLayer` the surface is built from is owned by Swift (the `UIView`'s
//! backing layer). The only thing to drop is the `wgpu::Surface` inside
//! `renderer`. The contract is that the layer outlives the handle — guaranteed by
//! the Swift side calling `forgekit_destroy` (which drops this handle, and with
//! it the surface) *before* releasing the view/layer.

use std::any::Any;

use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_shell_common::{AppTree, logical_size, sanitize_scale};
use forgekit_text::TextContext;
use kurbo::{Affine, Size};

/// Everything a running iOS app needs across frames — the state behind the opaque
/// handle Swift passes back into every C call.
///
/// There is deliberately no window/layer field (see the module docs): the
/// `CAMetalLayer` is Swift-owned, and the `renderer` (which owns the
/// `wgpu::Surface` built from that layer) is the only thing torn down on drop.
pub struct IosAppHandle {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    text_ctx: TextContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    app: Box<dyn AppTree>,
    /// The current surface's physical (pixel) size, updated on create/resize and
    /// divided by `scale` to lay out in logical pixels.
    physical: (u32, u32),
    /// Display scale (`UIScreen.scale` / the layer's `contentsScale`), the device
    /// pixel ratio the whole scene is scaled by so glyphs rasterise sharp. Stored
    /// raw and sanitised once per frame.
    scale: f32,
    /// Set by `forgekit_pause`/`forgekit_resume`; while paused, `frame()` is a
    /// no-op — Metal command submission from a backgrounded iOS app can get the
    /// process killed (spec §10.2), so this is the Rust-side enforcement point.
    paused: bool,
}

impl IosAppHandle {
    /// Assemble a handle with an already-created, `SurfaceReady` renderer.
    ///
    /// Called from [`crate::ffi_glue`] after the surface has been built from the
    /// `CAMetalLayer`; runs the first rebuild so the tree exists before the first
    /// frame. No `unsafe` here — the surface creation happens at the FFI boundary.
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        physical: (u32, u32),
        scale: f32,
        mut app: Box<dyn AppTree>,
    ) -> Self {
        app.rebuild();
        Self {
            render_cx,
            renderer,
            text_ctx: TextContext::new(),
            scene: Scene::new(),
            app,
            physical,
            scale,
            paused: false,
        }
    }

    /// Resize the live surface in place (rotation / bounds change). The
    /// `CAMetalLayer` survives an iOS rotation, so this is always a plain resize —
    /// no recreate path is needed (contrast the Android shell, where a new
    /// `Surface` object forces surface recreation). Safe: the renderer's resize
    /// path touches no raw pointers.
    pub(crate) fn resize(&mut self, physical: (u32, u32), scale: f32) {
        self.renderer
            .on_surface_changed(&self.render_cx, physical.0, physical.1);
        self.physical = physical;
        self.scale = scale;
    }

    /// Mark the app paused (`forgekit_pause`): subsequent `frame()`s are no-ops.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
    }

    /// Mark the app resumed (`forgekit_resume`): `frame()`s do work again.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
    }

    /// The current lifecycle phase (spec §8.1).
    fn phase(&self) -> SurfacePhase {
        self.renderer.phase()
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path (spec §8) but driven by the Swift
    /// `CADisplayLink`.
    ///
    /// A no-op unless the surface is `SurfaceReady` *and* the app is not paused
    /// (see [`crate::ffi_support::should_render_frame`]). On
    /// `FrameOutcome::SurfaceLost` the machine has already dropped the surface;
    /// recovery waits for the next resize rather than recreating mid-frame.
    pub(crate) fn frame(&mut self) {
        let ready = self.phase() == SurfacePhase::SurfaceReady;
        if !crate::ffi_support::should_render_frame(ready, self.paused) {
            return;
        }

        self.app.rebuild();

        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted `f32` scale from the FFI
        // boundary must never let the two passes disagree — see `sanitize_scale`).
        let scale = sanitize_scale(self.scale);
        let (lw, lh) = logical_size(self.physical.0, self.physical.1, scale);
        let logical = Size::new(lw, lh);
        {
            let text_ctx: &mut dyn Any = &mut self.text_ctx;
            self.app.layout(logical, text_ctx);
        }

        self.scene.reset();
        {
            let mut builder = SceneBuilder::new(&mut self.scene);
            // HiDPI (spec task 08): lay out in logical pixels, then scale the
            // whole scene by the device pixel ratio for sharp glyphs.
            builder.push_transform(Affine::scale(scale));
            self.app.paint(&mut builder);
            builder.pop_transform();
        }

        match self
            .renderer
            .render(&self.render_cx, &self.scene, peniko::Color::WHITE)
        {
            // Stale swapchain (e.g. mid-rotation): reconfigured internally; the
            // next CADisplayLink frame draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => {}
            // Surface lost: dropped by the machine; wait for the next resize to
            // recreate it.
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("forgekit-shell-ios: surface lost; awaiting resize");
            }
            Ok(FrameOutcome::Rendered | FrameOutcome::Skipped) => {}
            Err(err) => log::error!("forgekit-shell-ios: render error: {err:#}"),
        }
    }
}
