//! The Android app runtime: [`AndroidAppHandle`], the state behind the opaque
//! JNI handle.
//!
//! This module is `#[cfg(target_os = "android")]`; it owns the same resources
//! the desktop shell's `ShellHandler` does — a [`RenderContext`],
//! [`SurfaceRenderer`], [`TextContext`], reusable [`Scene`], plus the app tree —
//! but is driven by Choreographer-posted JNI frames instead of a winit loop.
//! It contains no `unsafe`; the FFI boundary lives entirely in
//! [`crate::jni_glue`]. The `State`/`app_logic` erasure it drives
//! ([`AppTree`](forgekit_shell_common::AppTree)) is platform-agnostic and lives
//! in `forgekit-shell-common`.

use std::any::Any;

use forgekit_core::event::{InputEvent, PointerButton, PointerEvent, PointerPhase};
use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_shell_common::{AppTree, logical_size, sanitize_scale};
use forgekit_text::TextContext;
use kurbo::{Affine, Point, Size};
use ndk::native_window::NativeWindow;

use crate::ffi_support::TouchPhase;

/// Everything a running Android app needs across frames — the state behind the
/// opaque `jlong` handle the JVM passes back into every native call.
///
/// Field order is load-bearing for drop safety: `renderer` (which owns the
/// `wgpu::Surface` built from `window`'s raw pointer) is declared before
/// `window`, so on drop the surface is torn down before the [`NativeWindow`] it
/// borrows is released (spec §8.1: no surface outlives its window).
pub struct AndroidAppHandle {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    text_ctx: TextContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    app: Box<dyn AppTree>,
    /// The current surface's physical (pixel) size, updated on create/resize and
    /// divided by `scale` to lay out in logical pixels.
    physical: (u32, u32),
    /// Display density (`resources.displayMetrics.density`), the device pixel
    /// ratio the whole scene is scaled by so glyphs rasterise sharp.
    scale: f32,
    /// The acquired window backing the current surface. Dropped after `renderer`
    /// (see the struct doc): its `Drop` calls `ANativeWindow_release`.
    window: Option<NativeWindow>,
}

impl AndroidAppHandle {
    /// Assemble a handle with an already-created, `SurfaceReady` renderer.
    ///
    /// Called from [`crate::jni_glue`] after the surface has been built from the
    /// `NativeWindow`; runs the first rebuild so the tree exists before the first
    /// frame. No `unsafe` here — the window acquisition and surface creation
    /// happen at the FFI boundary.
    pub(crate) fn new(
        render_cx: RenderContext,
        renderer: SurfaceRenderer,
        window: NativeWindow,
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
            window: Some(window),
        }
    }

    /// The current surface's window, if any (used by [`crate::jni_glue`] to
    /// compare against a newly delivered `Surface` before deciding whether a
    /// `surfaceChanged` is a resize or a recreate).
    pub(crate) fn window(&self) -> Option<&NativeWindow> {
        self.window.as_ref()
    }

    /// Access to the render context + renderer for the FFI layer to drive an
    /// `unsafe` surface *creation* (the one lifecycle transition that crosses the
    /// raw-pointer boundary); the safe transitions have their own methods below.
    pub(crate) fn renderer_mut(&mut self) -> (&mut RenderContext, &mut SurfaceRenderer) {
        (&mut self.render_cx, &mut self.renderer)
    }

    /// Record the window + physical size backing a freshly (re)created surface.
    ///
    /// Assigning `window` last drops the previous [`NativeWindow`] (releasing it)
    /// — safe here because the previous surface was already torn down inside the
    /// preceding `on_surface_created_from_android_window` (spec §8.1).
    pub(crate) fn set_window(&mut self, window: NativeWindow, physical: (u32, u32)) {
        self.physical = physical;
        self.window = Some(window);
    }

    /// Resize the live surface in place (same window, new dimensions). Safe: the
    /// renderer's resize path touches no raw pointers.
    pub(crate) fn resize(&mut self, physical: (u32, u32)) {
        self.renderer
            .on_surface_changed(&self.render_cx, physical.0, physical.1);
        self.physical = physical;
    }

    /// Tear the surface down (`surfaceDestroyed`): drop the renderer's surface
    /// first (spec §8.1), then release the [`NativeWindow`] it borrowed.
    pub(crate) fn destroy_surface(&mut self) {
        self.renderer.on_surface_destroyed();
        self.window = None;
    }

    /// The current lifecycle phase (spec §8.1).
    pub(crate) fn phase(&self) -> SurfacePhase {
        self.renderer.phase()
    }

    /// Deliver one touch contact to the tree (spec §9), converting the incoming
    /// physical view-local coordinates into the logical space the tree lays out
    /// in — the same `sanitize_scale` value `frame()` uses, so hit-testing and
    /// layout never disagree.
    ///
    /// Single-pointer in v1: the Kotlin side forwards only the primary pointer,
    /// so every contact is a [`PointerButton::Primary`] event. The redraw the
    /// tree requests is implicit here — the Choreographer loop already posts a
    /// frame every vsync, so the mutated state is picked up on the next
    /// `frame()` without an explicit schedule (contrast the desktop shell's
    /// `request_redraw`).
    pub(crate) fn dispatch_touch(&mut self, phase: TouchPhase, x: f32, y: f32) {
        let scale = sanitize_scale(self.scale);
        let position = Point::new(x as f64 / scale, y as f64 / scale);
        let core_phase = match phase {
            TouchPhase::Down => PointerPhase::Down,
            TouchPhase::Move => PointerPhase::Move,
            TouchPhase::Up => PointerPhase::Up,
            TouchPhase::Cancel => PointerPhase::Cancel,
        };
        let event = InputEvent::Pointer(PointerEvent {
            phase: core_phase,
            position,
            button: PointerButton::Primary,
        });
        let _ = self.app.event(&event);
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path (spec §8) but driven by Choreographer.
    ///
    /// A no-op when the surface isn't `SurfaceReady` (Kotlin keeps posting frames
    /// across surface loss; this makes those cheap). On `FrameOutcome::SurfaceLost`
    /// the machine has already dropped the surface; recovery waits for the next
    /// `surfaceChanged`/`surfaceCreated` rather than recreating mid-frame.
    pub(crate) fn frame(&mut self) {
        if self.phase() != SurfacePhase::SurfaceReady {
            return;
        }

        self.app.rebuild();

        // Sanitize once per frame; layout and the paint transform below MUST
        // consume this identical value (an untrusted JNI `jfloat` density
        // must never let the two passes disagree — see `sanitize_scale`).
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
            // next Choreographer frame draws against the fresh configuration.
            Ok(FrameOutcome::Redraw) => {}
            // Surface lost: dropped by the machine; wait for surfaceChanged to
            // recreate it (Android pairs loss with a destroy/create cycle).
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("forgekit-shell-android: surface lost; awaiting surfaceChanged");
            }
            Ok(FrameOutcome::Rendered | FrameOutcome::Skipped) => {}
            Err(err) => log::error!("forgekit-shell-android: render error: {err:#}"),
        }
    }
}
