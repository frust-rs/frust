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
//! This handle retains the Swift-owned `CAMetalLayer` as a raw `*mut c_void`
//! (`metal_layer`) so the shell can *recreate* the `wgpu::Surface` after a
//! `SurfaceLost` — iOS never destroys/recreates the layer itself (contrast
//! Android's window cycle), so without the retained pointer a lost surface would
//! be terminal (permanent black screen). Retaining the raw pointer is sound
//! because the layer's ownership stays with Swift and Swift guarantees it
//! outlives this handle: `forgekit_destroy` drops the handle (and with it the
//! `wgpu::Surface`) *before* the view/layer is released. The handle never frees
//! the layer — it only reads the pointer to hand it back to
//! `on_surface_created_from_metal_layer` at the FFI boundary.
//!
//! The `*mut c_void` field makes [`IosAppHandle`] `!Send`/`!Sync` by default,
//! which is exactly right: every `forgekit_*` call is on the UIKit main thread,
//! so the handle is never sent across threads and no auto-trait promise is made
//! about it.

use std::any::Any;
use std::ffi::c_void;

use forgekit_render::{FrameOutcome, RenderContext, SurfacePhase, SurfaceRenderer};
use forgekit_scene::{Scene, SceneBuilder};
use forgekit_shell_common::{AppTree, logical_size, sanitize_scale};
use forgekit_text::TextContext;
use kurbo::{Affine, Size};

/// Everything a running iOS app needs across frames — the state behind the opaque
/// handle Swift passes back into every C call.
///
/// The `renderer` owns the `wgpu::Surface` and is the only thing torn down on
/// drop; the `metal_layer` pointer it was built from is Swift-owned and merely
/// retained (never freed) here so a lost surface can be recreated (see the module
/// docs' *Layer lifetime contract*).
pub struct IosAppHandle {
    render_cx: RenderContext,
    renderer: SurfaceRenderer,
    text_ctx: TextContext,
    /// Reused across frames; `reset()` each frame rather than reallocated.
    scene: Scene,
    app: Box<dyn AppTree>,
    /// The Swift-owned `CAMetalLayer*` this handle's surface was built from,
    /// retained so the shell can recreate the surface after a `SurfaceLost` (iOS
    /// keeps the same layer for the app's whole lifetime). Read-only from Rust's
    /// side — never dropped/freed here (see the module docs' lifetime contract);
    /// only handed back to `on_surface_created_from_metal_layer` at the FFI
    /// boundary in [`crate::ffi_glue`], where the `unsafe` stays confined.
    metal_layer: *mut c_void,
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
    /// Consecutive failed surface-recreate attempts in the current `SurfaceLost`
    /// episode. Compared against `ffi_support::MAX_RECREATE_ATTEMPTS` so a
    /// persistently-failing recreate degrades to a logged stop instead of a
    /// per-CADisplayLink-frame retry storm; reset by a successful recreate
    /// ([`Self::set_surface`]).
    recreate_failures: u8,
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
        metal_layer: *mut c_void,
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
            metal_layer,
            physical,
            scale,
            paused: false,
            recreate_failures: 0,
        }
    }

    /// The retained Swift-owned `CAMetalLayer*` this handle's surface was built
    /// from, used by [`crate::ffi_glue`] to recreate the surface after a
    /// `SurfaceLost`. Returns the raw pointer by value (no borrow) so the FFI
    /// layer can read it before taking a `&mut` via [`renderer_mut`](Self::renderer_mut);
    /// the actual `unsafe` surface creation stays confined to `ffi_glue`.
    pub(crate) fn metal_layer(&self) -> *mut c_void {
        self.metal_layer
    }

    /// Access to the render context + renderer for the FFI layer to drive an
    /// `unsafe` surface *recreation* (the one lifecycle transition that crosses the
    /// raw-pointer boundary), mirroring the Android handle's accessor of the same
    /// name; the safe transitions have their own methods below.
    pub(crate) fn renderer_mut(&mut self) -> (&mut RenderContext, &mut SurfaceRenderer) {
        (&mut self.render_cx, &mut self.renderer)
    }

    /// The current surface's physical (pixel) size, used by [`crate::ffi_glue`] to
    /// recreate a lost surface at its last-known dimensions on a `render_frame`
    /// (a `resize` supplies fresh dimensions instead).
    pub(crate) fn physical(&self) -> (u32, u32) {
        self.physical
    }

    /// The current display scale, retained across a surface recreation on a
    /// `render_frame` (see [`physical`](Self::physical)).
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Record the physical size + scale backing a freshly recreated surface.
    ///
    /// The `metal_layer` never changes (iOS keeps the same layer for the app's
    /// lifetime), so — unlike Android's `set_window` — there is no window to swap:
    /// this only refreshes the layout inputs after a successful recreate in
    /// [`crate::ffi_glue`]. Safe: touches no raw pointers.
    pub(crate) fn set_surface(&mut self, physical: (u32, u32), scale: f32) {
        self.physical = physical;
        self.scale = scale;
        // A successful recreate ends the SurfaceLost episode; future losses get a
        // fresh retry budget.
        self.recreate_failures = 0;
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

    /// Whether the app is paused. `pub(crate)` so [`crate::ffi_glue`] can gate
    /// surface recreation on it — recreation is real Metal work and must obey the
    /// same backgrounded-no-GPU rule as frame submission.
    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    /// Consecutive failed recreate attempts this `SurfaceLost` episode (see the
    /// field doc).
    pub(crate) fn recreate_failures(&self) -> u8 {
        self.recreate_failures
    }

    /// Record one failed surface-recreate attempt (saturating; the predicate cap
    /// makes values past `MAX_RECREATE_ATTEMPTS` unreachable in practice).
    pub(crate) fn record_recreate_failure(&mut self) {
        self.recreate_failures = self.recreate_failures.saturating_add(1);
    }

    /// The current lifecycle phase (spec §8.1). `pub(crate)` so [`crate::ffi_glue`]
    /// can gate surface recreation on a `SurfaceLost` phase.
    pub(crate) fn phase(&self) -> SurfacePhase {
        self.renderer.phase()
    }

    /// Run one frame: rebuild → layout → paint → render, mirroring the desktop
    /// shell's `RedrawRequested` path (spec §8) but driven by the Swift
    /// `CADisplayLink`.
    ///
    /// A no-op unless the surface is `SurfaceReady` *and* the app is not paused
    /// (see [`crate::ffi_support::should_render_frame`]). On
    /// `FrameOutcome::SurfaceLost` the machine has already dropped the surface;
    /// this frame becomes a no-op, and the *next* `forgekit_render_frame`/
    /// `forgekit_resize` FFI entry recreates the surface from the retained
    /// `metal_layer` (see [`crate::ffi_glue`]) before rendering resumes.
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
            // Surface lost: dropped by the machine. The next render_frame/resize
            // FFI entry recreates it from the retained `metal_layer` (see
            // `ffi_glue`), bounded by the CADisplayLink cadence — not a busy loop.
            Ok(FrameOutcome::SurfaceLost) => {
                log::warn!("forgekit-shell-ios: surface lost; recreating on next frame/resize");
            }
            Ok(FrameOutcome::Rendered | FrameOutcome::Skipped) => {}
            Err(err) => log::error!("forgekit-shell-ios: render error: {err:#}"),
        }
    }
}
