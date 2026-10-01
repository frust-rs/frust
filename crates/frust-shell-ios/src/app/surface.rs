//! Surface + app lifecycle: the resolved-translucency sync (this shell's single
//! publish point for the app-facing RESOLVED surface-mode slot), the
//! create/resize/recreate accessors the FFI layer drives, pause/resume, and the
//! window-shape signals (insets, `WindowMetrics`) those transitions republish.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use frust_core::insets::{CornerInsets, WindowInsets};
use frust_reactive::{ReactiveRuntime, provide_context};
use frust_render::{RenderContext, SurfaceAlphaRequest, SurfacePhase, SurfaceRenderer};
use frust_shell_common::{
    AppTree, SurfaceSize, logical_corner_insets, logical_insets, publish_resolved_surface_mode,
    sanitize_scale,
};

use super::IosAppHandle;
use super::executor::FrameExecutor;

/// The construction-time half of the resolved-translucency contract, called
/// once from [`IosAppHandle::new`] before the first rebuild.
///
/// Threads the surface's RESOLVED translucency into the render root so the
/// platform-view hole-punch clears each Mode B slot's rect only when the
/// surface really came up translucent, not merely requested. At construction
/// the split's surface may still be installing render-side, so this reads the
/// request-seeded flag; `frame`'s per-frame
/// [`IosAppHandle::sync_translucent_resolved`] re-reads it and downgrades
/// within one frame of a fallback.
///
/// It also seeds the app-facing RESOLVED slot from that same value, so
/// app/plugin code reading `frust::resolved_surface_mode()` during the very
/// first rebuild sees a real verdict rather than `Unknown`. Re-published every
/// frame by `sync_translucent_resolved` — which is why this seed lives in this
/// module beside it: the two are the shell's only sanctioned writers of that
/// slot (`crates/frust/tests/surface_mode_conformance.rs`).
pub(super) fn seed_resolved_translucency(app: &mut dyn AppTree, translucent_resolved: &AtomicBool) {
    let resolved_translucent = crate::ffi_support::read_resolved_translucency(translucent_resolved);
    app.set_surface_translucent(resolved_translucent);
    publish_resolved_surface_mode(resolved_translucent);
}

/// Merge a fresh corner half (`frust_set_corner_insets`) into the stored
/// composite, keeping `current`'s edge half. `None` when the result equals
/// `current` — the caller's no-op guard.
///
/// The iOS composite [`WindowInsets`] is fed by two FFI calls carrying disjoint
/// halves (edges from `frust_set_insets`, corners from
/// `frust_set_corner_insets`); each push must replace only its own half.
fn merge_corner_insets(current: WindowInsets, corners: CornerInsets) -> Option<WindowInsets> {
    let merged = current.with_corner_insets(corners);
    (merged != current).then_some(merged)
}

/// Merge a fresh edge half (`frust_set_insets`: `view_padding` +
/// `view_insets`) into the stored composite, keeping `current.corner_insets`.
/// `None` when the result equals `current`. See [`merge_corner_insets`].
fn merge_edge_insets(current: WindowInsets, edges: WindowInsets) -> Option<WindowInsets> {
    let merged = edges.with_corner_insets(current.corner_insets);
    (merged != current).then_some(merged)
}

impl IosAppHandle {
    /// Re-read the live surface's RESOLVED translucency and push it into the
    /// render root, returning it for this frame's base-color choice.
    ///
    /// Two sources, one flag:
    /// - **Inline** (`FRUST_NO_RENDER_THREAD`): the renderer lives on this
    ///   thread, so its `surface_resolved_translucent()` is authoritative and
    ///   is copied into the shared flag here — never stale, even after a failed
    ///   recreate (the renderer still describes whatever surface is live).
    /// - **Split** (default): the render thread stored the resolution when it
    ///   installed (or self-healed) the surface; this is a plain atomic load.
    ///
    /// Cheap enough to call every frame: one enum match plus one atomic load,
    /// and `AppTree::set_surface_translucent` is no-op-if-unchanged (marking
    /// `ChangeFlags::PAINT` only on an actual flip — which is what makes a
    /// downgrade repaint without the punch).
    ///
    /// Also this shell's single publish point for the app-facing RESOLVED slot
    /// (`frust::resolved_surface_mode()`), mirroring Android: app
    /// code polls that slot during rebuild, so it must be current *before* the
    /// rebuild this frame leads into, and publishing here (rather than at the
    /// install/self-heal sites) keeps one UI-thread beat as the source for both
    /// the render root and the app. One uncontended `Mutex` store per frame.
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

    /// `frust_set_insets`: push the platform's per-edge insets onto the render
    /// root. Mirrors the Android shell's `set_insets` but
    /// with an **identity scale**: UIKit's `safeAreaInsets` and keyboard frame are
    /// already in **logical points** (the same space `dispatch_touch` passes
    /// through with no scale division — the documented iOS/Android coordinate
    /// asymmetry), so this uses `logical_insets(.., 1.0)` rather than dividing by
    /// the display scale like Android's physical-px path.
    ///
    /// `logical` is the eight-value pack [`logical_insets`] expects (`view_padding`
    /// then `view_insets`, each l/t/r/b — Swift assembles `view_padding` from
    /// `safeAreaInsets` and `view_insets` from the keyboard frame). No-op-guarded
    /// on `PartialEq`: a re-report of unchanged insets neither relayouts nor
    /// re-provides. On a real change `RenderRoot::set_insets` marks `LAYOUT |
    /// PAINT` pending, which the frame gate already treats as dirty —
    /// no new gate input needed. The continuous `CADisplayLink` loop repaints the
    /// next tick with no extra wake.
    ///
    /// Only the edge half is replaced: the stored `corner_insets` (delivered
    /// separately by [`Self::set_corner_insets`]) are carried over before the
    /// equality check, so a safe-area push never zeroes the window-control
    /// corners.
    pub(crate) fn set_insets(&mut self, logical: [f64; 8]) {
        // iOS insets are already logical points (see the method doc): identity
        // scale, unlike Android's device-px `sanitize_scale(self.scale)` divisor.
        let Some(insets) = merge_edge_insets(self.insets, logical_insets(logical, 1.0)) else {
            return; // no-op push — skip both the relayout and the re-provide
        };
        self.insets = insets;
        self.push_insets(insets);
        // The composite window-shape context carries a copy of these insets, so
        // an insets change is also a metrics change (self-guarded).
        self.push_window_metrics();
    }

    /// `frust_set_corner_insets`: merge the iPadOS 26+ window control's corner
    /// footprint into the stored composite [`WindowInsets`] and push it on a
    /// real change.
    ///
    /// `logical` is the eight-value pack [`logical_corner_insets`] expects
    /// (`tl_w, tl_h, tr_w, tr_h, bl_w, bl_h, br_w, br_h`, physical corners),
    /// already in logical points — identity scale, for the same reason as
    /// [`Self::set_insets`]. Only the corner half is replaced; the edge half
    /// `frust_set_insets` delivered is kept. No-op-guarded on `PartialEq`:
    /// Swift pushes from every `viewDidLayoutSubviews`, so an unchanged value
    /// must cost nothing — no relayout, no re-provide, no log.
    ///
    /// On a real change, debug builds emit an **`info`**-level line (target
    /// `frust`), `frust-corner-insets tl={:.1}x{:.1} tr={:.1}x{:.1}
    /// bl={:.1}x{:.1} br={:.1}x{:.1}` (width x height), mirroring Android's
    /// `frust-insets` line. `info`, not `debug`: this shell's stderr logger
    /// installs `Info` unless `FRUST_LOG` overrides it, so a `debug!` line would
    /// never surface in a default debug build; release builds omit it.
    pub(crate) fn set_corner_insets(&mut self, logical: [f64; 8]) {
        let corners = logical_corner_insets(logical, 1.0);
        let Some(insets) = merge_corner_insets(self.insets, corners) else {
            return; // unchanged — the per-layout push is free
        };
        self.insets = insets;
        if cfg!(debug_assertions) {
            log::info!(
                target: "frust",
                "frust-corner-insets tl={:.1}x{:.1} tr={:.1}x{:.1} bl={:.1}x{:.1} br={:.1}x{:.1}",
                corners.top_left.width,
                corners.top_left.height,
                corners.top_right.width,
                corners.top_right.height,
                corners.bottom_left.width,
                corners.bottom_left.height,
                corners.bottom_right.width,
                corners.bottom_right.height,
            );
        }
        self.push_insets(insets);
        // The composite window-shape context carries a copy of these insets.
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
    ///   [`Self::frame`]: `frust_resize` → [`Self::resize`] /
    ///   [`Self::set_surface`] (drawable size + scale), `frust_set_insets` →
    ///   [`Self::set_insets`] (safe area / keyboard) and
    ///   `frust_set_corner_insets` → [`Self::set_corner_insets`] (window-control
    ///   corners). `frame` only reads what those already stored.
    ///
    /// Units: the size is converted from the drawable's physical px with the
    /// same `sanitize_scale`d display scale layout uses, and `self.insets` is
    /// already logical (UIKit hands over points, which
    /// [`Self::set_insets`] passes through `logical_insets(.., 1.0)`) — so the
    /// published metrics are logical throughout, matching Android and desktop.
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

    /// The retained Swift-owned `CAMetalLayer*` this handle's surface was built
    /// from, used by [`crate::ffi_glue`] to recreate the surface after a
    /// `SurfaceLost` in the inline path. Returns the raw pointer by value (no
    /// borrow) so the FFI layer can read it before taking a `&mut` via
    /// [`inline_renderer_mut`](Self::inline_renderer_mut); the actual `unsafe`
    /// surface creation stays confined to `ffi_glue`. (In the split the render
    /// thread keeps its own copy of the pointer and self-heals render-side.)
    pub(crate) fn metal_layer(&self) -> *mut c_void {
        self.metal_layer
    }

    /// The surface-alpha request this handle's surface was created with,
    /// read by [`crate::ffi_glue::recover_surface`]
    /// so an inline surface recreate requests the same alpha mode the initial
    /// surface did (see the field doc).
    pub(crate) fn surface_alpha(&self) -> SurfaceAlphaRequest {
        self.surface_alpha
    }

    /// Whether the render-thread split is engaged (as opposed to the inline
    /// fallback) — the FFI layer branches surface *recovery* on this: the split
    /// self-heals a lost surface render-side (the render loop recreates from the
    /// retained layer pointer), so the UI-side `should_recreate_surface` recovery
    /// in `frust_render_frame`/`frust_resize` applies only to the inline path.
    pub(crate) fn executor_is_split(&self) -> bool {
        matches!(self.executor, FrameExecutor::Split(_))
    }

    /// Whether the render thread signalled a fatal, unrecoverable first-surface
    /// install failure, read by
    /// [`crate::ffi_glue::render_frame`] to tell Swift to latch `initFailed` and
    /// invalidate its `CADisplayLink`. The split reads its shared `fatal` flag;
    /// the inline fallback never faults here — a failed first install returns
    /// `Err` from `create_handle`, so `frust_init` yields a null handle and no
    /// frame is ever driven — so it is always `false`.
    pub(crate) fn render_fatal(&self) -> bool {
        match &self.executor {
            FrameExecutor::Split(split) => split.fatal.load(Ordering::Acquire),
            FrameExecutor::Inline(_) => false,
        }
    }

    /// Access to the inline render context + renderer for the FFI layer to drive an
    /// `unsafe` surface *recreation* (the one lifecycle transition that crosses the
    /// raw-pointer boundary). `None` in the render-thread split — there the render
    /// thread owns the renderer and recovery is render-side (see
    /// [`crate::ffi_glue::render_loop`]).
    pub(crate) fn inline_renderer_mut(
        &mut self,
    ) -> Option<(&mut RenderContext, &mut SurfaceRenderer)> {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => Some((&mut inline.render_cx, &mut inline.renderer)),
            FrameExecutor::Split(_) => None,
        }
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
        // A surface recreation forces the frame gate's resume-warmup: the first
        // ticks against the fresh surface must run even before their change
        // signals are observable.
        self.frame_gate.note_resumed();
        // The recreate may land at new dimensions/scale — republish the
        // window-shape context (self-guarded, so a same-size recreate is silent).
        self.push_window_metrics();
        // The recreated surface re-resolved its alpha mode from scratch (review
        // M1) — on this (inline) path the renderer on this thread already holds
        // it, so a recreate that fell back to opaque degrades to Mode A right
        // here rather than punching black holes for the rest of the process.
        self.sync_translucent_resolved();
    }

    /// Resize the live surface in place (rotation / bounds change). The
    /// `CAMetalLayer` survives an iOS rotation, so this is always a plain resize —
    /// no recreate path is needed (contrast the Android shell, where a new
    /// `Surface` object forces surface recreation). Safe: no raw pointers — inline
    /// reconfigures the renderer's swapchain directly; the split routes the
    /// layer-owned in-place resize as a
    /// [`RenderCommand::SurfaceChanged`](frust_shell_common::RenderCommand::SurfaceChanged)
    /// command.
    pub(crate) fn resize(&mut self, physical: (u32, u32), scale: f32) {
        match &mut self.executor {
            FrameExecutor::Inline(inline) => {
                inline
                    .renderer
                    .on_surface_changed(&inline.render_cx, physical.0, physical.1)
            }
            FrameExecutor::Split(split) => split.resize(SurfaceSize {
                width: physical.0,
                height: physical.1,
                scale: scale as f64,
            }),
        }
        self.physical = physical;
        self.scale = scale;
        // A resize (rotation / bounds change) forces the frame gate's
        // resume-warmup so the next frames re-layout/paint at the new size even
        // if no other change signal fires.
        self.frame_gate.note_resumed();
        // New logical size and/or scale: republish the window-shape context —
        // this is the path a device rotation takes, so it is what flips the
        // derived `Orientation` for app code (self-guarded, so a `frust_resize`
        // re-reporting identical dimensions publishes nothing).
        self.push_window_metrics();
    }

    /// Mark the app paused (`frust_pause`): subsequent `frame()`s are no-ops.
    ///
    /// In the render-thread split this **barriers on the render thread's ack
    /// before returning**: it sends a `Pause` command
    /// and blocks until the render thread has quiesced (moved to `Paused`, dropped
    /// any leftover scene, and acked). Only then does this return, so the app
    /// never backgrounds while the render thread might still submit Metal work —
    /// which can get a suspended process killed. The `self.paused` flag is the
    /// UI-side belt-and-suspenders (it also no-ops `frame()`); the ack is the real
    /// cross-thread guarantee. Set `paused` first so a `frame()` racing in cannot
    /// hand off a new scene while the barrier is in flight.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
        // Backgrounding path: hide every live
        // platform-view slot immediately rather than waiting out the
        // ordinary missing-streak Hide (paint doesn't run while paused, so
        // `ingest` never drives that path — see `PlatformViewState::suspend_all`).
        self.platform_views.suspend_all();
        // While backgrounded nothing is painted or presented, so a pairing
        // recorded before the pause would hold this hide behind a frame that
        // never lands. The gate is a smoothing device, not a correctness
        // barrier — drop it so the hide goes out on the next poll (the Android
        // shell's `suspend_platform_views` does the same).
        self.platform_view_due.clear();
        if let FrameExecutor::Split(split) = &mut self.executor {
            split.pause_barrier();
            // Present-sync: the barrier has returned, so the render
            // thread has quiesced and can park nothing more. Drop whatever it
            // parked last — a backgrounded app must issue no Metal work (the
            // same rule the barrier itself exists for), and the next foreground
            // frame produces a fresh drawable anyway.
            split.present.clear();
        }
    }

    /// Mark the app resumed (`frust_resume`): `frame()`s do work again. In the
    /// split, sends a fire-and-forget `Resume` so the render thread leaves `Paused`
    /// and resumes submitting.
    pub(crate) fn resume(&mut self) {
        self.paused = false;
        if let FrameExecutor::Split(split) = &mut self.executor {
            split.resume();
        }
        // Re-open the frame gate's resume-warmup: the first frames after
        // foregrounding must run unconditionally (a backgrounded app's change
        // signals may have been coalesced away).
        self.frame_gate.note_resumed();
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

    /// The inline renderer's current lifecycle phase, or `None` in the
    /// split (the render thread owns the phase). `pub(crate)` so [`crate::ffi_glue`]
    /// can gate the inline path's surface recreation on a `SurfaceLost` phase — the
    /// split self-heals render-side, so this returning `None` is exactly the "no
    /// UI-side recovery" signal there.
    pub(crate) fn inline_phase(&self) -> Option<SurfacePhase> {
        match &self.executor {
            FrameExecutor::Inline(inline) => Some(inline.renderer.phase()),
            FrameExecutor::Split(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::insets::{CornerInset, EdgeInsets};

    fn edges() -> WindowInsets {
        WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 20.0),
            EdgeInsets::new(0.0, 0.0, 0.0, 300.0),
        )
    }

    fn corners() -> CornerInsets {
        CornerInsets::new(
            CornerInset::new(80.0, 20.0),
            CornerInset::ZERO,
            CornerInset::ZERO,
            CornerInset::ZERO,
        )
    }

    #[test]
    fn edge_push_after_corner_push_keeps_corners() {
        let after_corners = merge_corner_insets(WindowInsets::default(), corners())
            .expect("a corner change is a change");
        let after_edges =
            merge_edge_insets(after_corners, edges()).expect("an edge change is a change");
        assert_eq!(after_edges.corner_insets, corners());
        assert_eq!(after_edges.view_padding, edges().view_padding);
        assert_eq!(after_edges.view_insets, edges().view_insets);
    }

    #[test]
    fn corner_push_after_edge_push_keeps_edges() {
        let after_edges = merge_edge_insets(WindowInsets::default(), edges())
            .expect("an edge change is a change");
        let after_corners =
            merge_corner_insets(after_edges, corners()).expect("a corner change is a change");
        assert_eq!(after_corners.view_padding, edges().view_padding);
        assert_eq!(after_corners.view_insets, edges().view_insets);
        assert_eq!(after_corners.corner_insets, corners());
    }

    #[test]
    fn unchanged_pushes_merge_to_none() {
        let current = edges().with_corner_insets(corners());
        assert_eq!(merge_corner_insets(current, corners()), None);
        // The edge half carries zero corners on the wire; the merge must still
        // see the composite as unchanged.
        assert_eq!(merge_edge_insets(current, edges()), None);
        assert_eq!(
            merge_corner_insets(WindowInsets::default(), CornerInsets::ZERO),
            None
        );
        assert_eq!(
            merge_edge_insets(WindowInsets::default(), WindowInsets::default()),
            None
        );
    }

    #[test]
    fn corner_push_to_zero_is_a_change() {
        let current = edges().with_corner_insets(corners());
        let cleared =
            merge_corner_insets(current, CornerInsets::ZERO).expect("clearing is a change");
        assert_eq!(cleared, edges());
    }
}
