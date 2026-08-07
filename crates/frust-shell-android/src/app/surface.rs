//! Surface lifecycle and window shape: every arm that reacts to the host's
//! surface being created, recreated, resized or destroyed, the
//! resolved-translucency sync (this module is the shell's only
//! `publish_resolved_surface_mode` caller — the construction seed and the
//! per-frame beat both live here), and the insets/window-metrics re-provides
//! those shape changes drive.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use frust_core::insets::WindowInsets;
use frust_reactive::{ReactiveRuntime, provide_context};
use frust_render::{RenderContext, SurfaceRenderer};
use frust_shell_common::{
    AppTree, SurfaceSize, logical_insets, publish_resolved_surface_mode, sanitize_scale,
};
use ndk::native_window::NativeWindow;

use super::AndroidAppHandle;
use super::executor::FrameExecutor;

/// Seed the two RESOLVED-translucency consumers at construction, before the
/// first rebuild ([`AndroidAppHandle::new`]).
///
/// Thread the surface's RESOLVED translucency into the render root so
/// the platform-view hole-punch clears each Mode B slot's rect — and
/// only when the surface really came up translucent, not merely
/// requested. At construction the split's surface may still be
/// installing render-side, so this reads the request-seeded flag; the
/// per-frame [`AndroidAppHandle::sync_translucent_resolved`] re-reads it every
/// frame and downgrades within one frame of a fallback.
///
/// The app-facing RESOLVED slot is seeded from that same value, so app/plugin
/// code reading `frust::resolved_surface_mode()` during the very first rebuild
/// sees a real verdict rather than `Unknown`. Re-published every frame by
/// `sync_translucent_resolved` — which is why this lives here beside it rather
/// than inline in the constructor: the slot has exactly one publishing module.
pub(super) fn seed_translucent_resolved(
    app: &mut dyn AppTree,
    translucent_resolved: &Arc<AtomicBool>,
) {
    let resolved_translucent = crate::ffi_support::read_resolved_translucency(translucent_resolved);
    app.set_surface_translucent(resolved_translucent);
    publish_resolved_surface_mode(resolved_translucent);
}

impl AndroidAppHandle {
    /// Re-read the live surface's RESOLVED translucency and push it into the
    /// render root, returning it for this frame's base-color choice.
    ///
    /// Two sources, one flag:
    /// - **Inline** (`FRUST_NO_RENDER_THREAD`): the renderer lives on this
    ///   thread, so its `surface_resolved_translucent()` is authoritative and
    ///   is copied into the shared flag here — never stale, even after a failed
    ///   reinstall (the renderer still describes whatever surface is live).
    /// - **Split** (default): the render thread stored the resolution when it
    ///   installed the surface; this is a plain atomic load.
    ///
    /// Cheap enough to call every frame: one enum match plus one atomic load,
    /// and `AppTree::set_surface_translucent` is no-op-if-unchanged (it marks
    /// `ChangeFlags::PAINT` only on an actual flip, which is exactly what makes
    /// a downgrade repaint without the punch).
    ///
    /// Also the shell's single publish point for the app-facing RESOLVED slot
    /// (`frust::resolved_surface_mode()`): app code polls that slot
    /// during rebuild, so it has to be current *before* the rebuild this frame
    /// leads into — publishing here rather than at the install sites keeps one
    /// UI-thread beat as the source for both consumers (the render root and the
    /// app), split and inline alike. One uncontended `Mutex` store per frame.
    pub(super) fn sync_translucent_resolved(&mut self) -> bool {
        if let FrameExecutor::Inline(inline) = &self.executor {
            crate::ffi_support::publish_resolved_translucency(
                &self.translucent_resolved,
                Some(inline.renderer.surface_resolved_translucent()),
            );
        }
        let resolved = crate::ffi_support::read_resolved_translucency(&self.translucent_resolved);
        publish_resolved_surface_mode(resolved);
        self.app.set_surface_translucent(resolved);
        resolved
    }

    /// The current surface's window, if any (used by [`crate::jni_glue`] to
    /// compare against a newly delivered `Surface` before deciding whether a
    /// `surfaceChanged` is a resize or a recreate).
    pub(crate) fn window(&self) -> Option<&NativeWindow> {
        self.window.as_ref()
    }

    /// Whether the render-thread split is engaged (as opposed to the inline
    /// fallback) — the FFI layer branches surface (re)creation on this: the split
    /// routes it through owned channel commands (safe), the inline path drives the
    /// `unsafe` surface creation directly with [`Self::inline_renderer_mut`].
    pub(crate) fn executor_is_split(&self) -> bool {
        matches!(self.executor, FrameExecutor::Split(_))
    }

    /// Whether the render thread signalled a fatal, unrecoverable first-surface
    /// install failure, read by
    /// [`crate::jni_glue::native_on_frame`] to tell Kotlin to stop the
    /// Choreographer loop. The split reads its shared `fatal` flag; the inline
    /// fallback never faults here — a failed first install returns `Err` from
    /// `create_handle`, so `nativeInit` yields the `0` handle and no frame is ever
    /// driven — so it is always `false`.
    pub(crate) fn render_fatal(&self) -> bool {
        match &self.executor {
            FrameExecutor::Split(split) => split.fatal.load(Ordering::Acquire),
            FrameExecutor::Inline(_) => false,
        }
    }

    /// Access to the inline render context + renderer for the FFI layer to drive
    /// an `unsafe` surface *creation* (the one lifecycle transition that crosses
    /// the raw-pointer boundary). `None` in the render-thread split — there the
    /// render thread owns the renderer and surface (re)creation is a channel
    /// command (see [`Self::split_recreate_surface`] / the render loop).
    pub(crate) fn inline_renderer_mut(
        &mut self,
    ) -> Option<(&mut RenderContext, &mut SurfaceRenderer)> {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => Some((&mut inline.render_cx, &mut inline.renderer)),
            FrameExecutor::Split(_) => None,
        }
    }

    /// Recreate the surface against `new_window` in the render-thread split
    /// (the inline path drives its own `unsafe` recreation in the FFI layer).
    ///
    /// The window-release ordering is the split's real hazard: the render thread
    /// holds the OLD surface built from the OLD window's raw pointer, so the UI
    /// thread must not release the old [`NativeWindow`] until the render thread has
    /// dropped that surface. So this **first** blocks on a barriered
    /// `SurfaceDestroyed` (the render thread drops the old surface and acks),
    /// **then** releases the old window, **then** hands the new window's pointer
    /// across for the render thread to build a fresh surface — the new window is
    /// retained here to keep its `ANativeWindow` alive for that new surface.
    pub(crate) fn split_recreate_surface(
        &mut self,
        new_window: NativeWindow,
        physical: (u32, u32),
        density: f32,
    ) {
        // Read the new window's raw pointer before it is moved into `self.window`
        // below (moving the wrapper leaves the underlying `ANativeWindow` — and
        // thus this pointer — unchanged).
        let ptr = new_window.ptr().as_ptr().cast::<c_void>();
        if let FrameExecutor::Split(split) = &mut self.executor {
            // Barrier: the render thread drops the old surface and acks before we
            // release the old window.
            split.destroy_surface_barrier();
        }
        // Old surface gone → releasing the old `NativeWindow`
        // (`ANativeWindow_release` via its `Drop`) is now safe.
        self.window = None;
        self.physical = physical;
        self.scale = density;
        self.window = Some(new_window);
        if let FrameExecutor::Split(split) = &mut self.executor {
            split.send_surface_created(
                ptr,
                SurfaceSize {
                    width: physical.0,
                    height: physical.1,
                    scale: density as f64,
                },
            );
        }
        // Surface (re)creation: force the next frame to run + lay out at the new
        // dimensions and open the resume-warmup window.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
        // The new surface may come up at new dimensions/density — republish the
        // window-shape context (self-guarded, so a same-size recreate is silent).
        self.push_window_metrics();
        // The old surface (and everything Kotlin's `FrustSurfaceView` composited
        // behind it) is gone — replay Create+Update for every currently-live
        // platform-view slot so the native side rebuilds its whole
        // sibling-view hierarchy from scratch rather than assuming any prior
        // placement survived. The replay supersedes every held batch, and the
        // frames those batches were paired with belong to the surface that just
        // went away — so the pairing goes with it.
        self.platform_view_state.reset_for_surface_recreate();
        self.platform_view_due.clear();
        // ...and with it the tail's hold: neither stage may outlive the
        // frames it refers to (mirroring the pairing above).
        self.sync_tail.clear();
        // The new surface re-resolves its alpha mode from scratch;
        // the render thread stores the outcome when it installs. Re-push
        // whatever is known now — the next `frame` re-reads it, so a
        // downgrade lands within one frame of the install.
        self.sync_translucent_resolved();
    }

    /// Record the window + physical size backing a freshly (re)created surface.
    ///
    /// Assigning `window` last drops the previous [`NativeWindow`] (releasing it)
    /// — safe here because the previous surface was already torn down inside the
    /// preceding `on_surface_created_from_android_window`.
    ///
    /// `density` is this configuration's `displayMetrics.density`: stored raw
    /// and re-sanitized at every use (layout/paint/insets),
    /// so a config change that alters the device pixel ratio takes effect on the
    /// next frame. Stored raw for the same reason `nativeInit`'s `scale` is —
    /// `sanitize_scale` runs once per frame at the point of use.
    pub(crate) fn set_window(&mut self, window: NativeWindow, physical: (u32, u32), density: f32) {
        self.physical = physical;
        self.scale = density;
        self.window = Some(window);
        // Surface (re)creation: force the next frame to run (and lay out at the
        // new dimensions) and open the gate's resume-warmup window — the first
        // ticks after a surface swap must not be gated away.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
        // See `split_recreate_surface`'s matching call: a new surface means a
        // fresh native-view hierarchy on the Kotlin side, and the
        // release-gate pairing refers to the old surface's frames.
        //
        // A recreate can also land at new dimensions/density — republish the
        // window-shape context (self-guarded, so a same-size recreate is silent).
        self.push_window_metrics();
        self.platform_view_state.reset_for_surface_recreate();
        self.platform_view_due.clear();
        // ...and with it the tail's hold: neither stage may outlive the
        // frames it refers to (mirroring the pairing above).
        self.sync_tail.clear();
        // Inline recreate: the renderer on this thread already holds the new
        // surface, so this reads its freshly RESOLVED translucency
        // — a recreate that fell back to opaque degrades to Mode A here rather
        // than punching black holes for the rest of the process.
        self.sync_translucent_resolved();
    }

    /// Resize the live surface in place (same window, new dimensions). Safe: no
    /// raw pointers — inline reconfigures the renderer's swapchain directly; the
    /// split sends a `RenderCommand::SurfaceChanged` command.
    ///
    /// `density` re-sanitizes and stores the display's device pixel ratio for
    /// this configuration (see [`Self::set_window`]).
    pub(crate) fn resize_surface(&mut self, physical: (u32, u32), density: f32) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => {
                inline
                    .renderer
                    .on_surface_changed(&inline.render_cx, physical.0, physical.1)
            }
            FrameExecutor::Split(split) => split.resize(SurfaceSize {
                width: physical.0,
                height: physical.1,
                scale: density as f64,
            }),
        }
        self.physical = physical;
        self.scale = density;
        // In-place resize: force the next frame to run and relayout at the new
        // size, and open the warmup window — same rationale as
        // `set_window`.
        self.surface_dirty = true;
        self.frame_gate.note_resumed();
        // New logical size and/or density: republish the window-shape context
        // (self-guarded, so a `surfaceChanged` re-reporting identical
        // dimensions publishes nothing).
        self.push_window_metrics();
    }

    /// `nativeOnInsetsChanged`: convert the platform's physical-px per-edge insets
    /// to logical px with the stored scale and push them onto the render root.
    /// Mirrors [`Self::set_appearance`]'s two-path
    /// delivery shape but for insets: `AppTree::set_insets` threads them into
    /// layout/paint (widget path — a `SafeArea`'s `LayoutCtx::window_insets`), and
    /// [`Self::push_insets`] re-`provide_context`s them for app code
    /// (`use_context::<WindowInsets>()` in `Component::build`).
    ///
    /// `physical` is the eight-value pack `logical_insets` expects (`view_padding`
    /// then `view_insets`, each l/t/r/b — see [`logical_insets`]). No-op-guarded on
    /// `PartialEq`: a shell that re-reports unchanged insets neither relayouts nor
    /// re-provides. On a real change, `RenderRoot::set_insets` marks `LAYOUT |
    /// PAINT` pending, which the frame gate already treats as
    /// dirty (`change_flags_pending`) — no new gate input needed. The continuous
    /// Choreographer loop repaints the next tick with no extra wake.
    pub(crate) fn set_insets(&mut self, physical: [f64; 8]) {
        let scale = sanitize_scale(self.scale);
        let insets = logical_insets(physical, scale);
        if insets == self.insets {
            return; // no-op push — skip both the relayout and the re-provide
        }
        self.insets = insets;
        self.push_insets(insets);
        // The composite window-shape context carries a copy of these insets, so
        // an insets change is also a metrics change (self-guarded).
        self.push_window_metrics();
    }

    /// Push the current [`WindowInsets`] to both delivery paths — into the render
    /// root (`AppTree::set_insets`, the widget/layout path) and
    /// re-`provide_context`ed under the process-wide root [`ReactiveRuntime`]'s
    /// owner for app-side `use_context::<WindowInsets>()` reads. Mirrors
    /// [`Self::push_theme`]'s shape exactly (the theme re-provide precedent).
    fn push_insets(&mut self, insets: WindowInsets) {
        self.app.set_insets(insets);
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(insets)),
            None => provide_context(insets),
        }
    }

    /// Re-`provide_context` the window's [`WindowMetrics`](frust_core::WindowMetrics)
    /// for app-side `use_context::<WindowMetrics>()` reads — **only when it
    /// actually changed** (`WindowMetricsPublisher::poll` returns `None`
    /// otherwise).
    ///
    /// Mirrors [`Self::push_insets`]/[`Self::push_theme`]'s re-provide shape,
    /// with two deliberate differences:
    ///
    /// - **Guarded, never per-frame.** `WindowMetrics` is delivered *alongside*
    ///   the standalone `WindowInsets` context (which keeps working untouched),
    ///   not through it, and it is not pushed into the render root at all — so
    ///   nothing else rate-limits it. `provide_context` is a plain map insert
    ///   that notifies nothing, but an unconditional per-frame re-provide would
    ///   still pay a lock write plus an allocation every frame across the FFI
    ///   boundary for no observable benefit. The publisher's change detection
    ///   is what keeps a static window quiet.
    /// - **Called from the entry points where the inputs move**, not from
    ///   [`Self::frame`]: `nativeOnSurfaceChanged` → [`Self::resize_surface`] /
    ///   [`Self::set_window`] / [`Self::split_recreate_surface`] (size + density)
    ///   and `nativeOnInsetsChanged` → [`Self::set_insets`] (insets). `frame`
    ///   only reads what those already stored.
    ///
    /// Units: the size is converted from the surface's physical px with the same
    /// `sanitize_scale`d density every other consumer uses, and `self.insets` is
    /// already logical (`logical_insets` ran in [`Self::set_insets`]) — so the
    /// published metrics are logical throughout, matching iOS and desktop.
    fn push_window_metrics(&mut self) {
        let scale = sanitize_scale(self.scale);
        let Some(metrics) = self.window_metrics.poll(self.physical, scale, self.insets) else {
            return; // unchanged — no re-provide, no app-wide rebuild
        };
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(metrics)),
            None => provide_context(metrics),
        }
    }

    /// This handle's device pixel ratio, sanitized the same way every other
    /// scale-consuming call site does (`dispatch_touch`/`set_insets`/
    /// `frame`'s layout step) — the one value `crate::jni_glue`'s
    /// `nativePlatformViewCommands` needs to convert the differ's logical-px
    /// rects to physical px at the FFI boundary, without exposing the raw
    /// `scale` field (private to this module) across the `jni_glue`/`app`
    /// module boundary.
    pub(crate) fn sanitized_scale(&self) -> f64 {
        sanitize_scale(self.scale)
    }

    pub(crate) fn destroy_surface(&mut self) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => inline.renderer.on_surface_destroyed(),
            FrameExecutor::Split(split) => split.destroy_surface_barrier(),
        }
        // Barrier complete (split) / surface dropped (inline): releasing the
        // window is now safe.
        self.window = None;
    }
}
