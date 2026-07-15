//! iOS-specific FFI helpers: the pure, host-testable predicates the C-ABI
//! runtime is built from.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `forgekit-shell-common` and is imported at its
//! call sites. What remains here is iOS-handle specific but still pure: the
//! null-pointer sentinel check for the opaque handle the Swift side passes back
//! into every call, and the paused/ready gate that decides whether a frame does
//! any work.
//!
//! Neither carries a `render`/GPU dependency, so both compile and are unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls them
//! ([`crate::ffi_glue`], [`crate::app`]) is `#[cfg(target_os = "ios")]`.

use std::ffi::c_void;

use forgekit_render::SurfacePhase;

/// Whether an opaque handle from the Swift side is the null sentinel.
///
/// The generated Swift app initialises its handle to `nil`/`null` and every call
/// after `forgekit_init` fails is guarded on it, so a null handle means "no
/// native side yet / already destroyed" and must be treated as a no-op rather
/// than dereferenced.
#[inline]
pub(crate) fn handle_is_null(handle: *mut c_void) -> bool {
    handle.is_null()
}

/// Whether a frame should run its rebuild → layout → paint → render pass.
///
/// A frame does work only when the surface is `SurfaceReady` *and* the app is not
/// paused. The paused gate is the enforcement point for the iOS rule that Metal
/// command submission from a backgrounded app can get the process killed
/// (belt-and-suspenders with the Swift side pausing its `CADisplayLink`); the
/// ready gate mirrors the desktop/Android shells' "no rendering outside
/// SurfaceReady" (spec §8.1).
#[inline]
pub(crate) fn should_render_frame(surface_ready: bool, paused: bool) -> bool {
    surface_ready && !paused
}

/// Cap on consecutive failed surface-recreate attempts per `SurfaceLost` episode.
///
/// `render_frame` fires at CADisplayLink cadence (60–120 Hz); without a cap, a
/// persistently-failing recreate would retry blocking GPU work plus an ERROR log
/// every frame, forever — the same failure shape the generated Swift side latches
/// against for a failed `forgekit_init`. Three attempts is enough to ride out a
/// transient loss; after that the shell degrades to a single logged failure and
/// stops (a successful recreate resets the counter).
pub(crate) const MAX_RECREATE_ATTEMPTS: u8 = 3;

/// Whether a lost surface should be recreated before the next frame/resize.
///
/// Unlike Android (which pairs surface loss with a `surfaceDestroyed`/
/// `surfaceCreated` window cycle), iOS keeps the same `CAMetalLayer` for the app's
/// whole lifetime, so nothing external re-drives surface creation after a
/// `SurfaceLost`. The shell therefore self-heals: on the next `forgekit_resize`
/// or `forgekit_render_frame` it recreates the surface from the retained layer.
/// This predicate is that trigger, and it is deliberately narrow:
///
/// - only in [`SurfacePhase::SurfaceLost`] (a `NoSurface` handle never exists
///   post-init, and `SurfaceReady` needs no recovery);
/// - never while `paused` — recreation is real Metal work, and the paused gate
///   exists precisely because GPU submission from a backgrounded iOS app can get
///   the process killed. A loss that coincides with backgrounding recovers on the
///   first frame/resize after `forgekit_resume`;
/// - only while under [`MAX_RECREATE_ATTEMPTS`] consecutive failures, so a
///   persistently-failing recreate cannot become a per-frame retry storm.
#[inline]
pub(crate) fn should_recreate_surface(
    phase: SurfacePhase,
    paused: bool,
    failed_attempts: u8,
) -> bool {
    matches!(phase, SurfacePhase::SurfaceLost) && !paused && failed_attempts < MAX_RECREATE_ATTEMPTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_handle_is_null() {
        assert!(handle_is_null(std::ptr::null_mut()));
    }

    #[test]
    fn nonnull_handle_is_not_null() {
        // The address of a real local is enough — the pointer is never
        // dereferenced here, only tested against the null sentinel.
        let mut local = 0u8;
        let ptr = (&mut local as *mut u8).cast::<c_void>();
        assert!(!handle_is_null(ptr));
    }

    #[test]
    fn frame_runs_only_when_ready_and_not_paused() {
        assert!(should_render_frame(true, false));
    }

    #[test]
    fn paused_frame_is_a_noop_even_when_ready() {
        // The paused gate is the process-kill guard: a paused app must never
        // submit a frame, regardless of surface readiness.
        assert!(!should_render_frame(true, true));
    }

    #[test]
    fn frame_is_a_noop_when_surface_not_ready() {
        assert!(!should_render_frame(false, false));
        assert!(!should_render_frame(false, true));
    }

    #[test]
    fn lost_surface_is_recreated() {
        // The self-recovery trigger: `SurfaceLost`, unpaused, under the cap.
        assert!(should_recreate_surface(SurfacePhase::SurfaceLost, false, 0));
        assert!(should_recreate_surface(
            SurfacePhase::SurfaceLost,
            false,
            MAX_RECREATE_ATTEMPTS - 1
        ));
    }

    #[test]
    fn ready_and_nosurface_are_not_recreated() {
        assert!(!should_recreate_surface(
            SurfacePhase::SurfaceReady,
            false,
            0
        ));
        assert!(!should_recreate_surface(SurfacePhase::NoSurface, false, 0));
    }

    #[test]
    fn paused_app_never_recreates() {
        // The paused gate is the process-kill guard; recreation is GPU work and
        // must wait for resume, exactly like frame submission.
        assert!(!should_recreate_surface(SurfacePhase::SurfaceLost, true, 0));
    }

    #[test]
    fn recreate_stops_at_the_attempt_cap() {
        assert!(!should_recreate_surface(
            SurfacePhase::SurfaceLost,
            false,
            MAX_RECREATE_ATTEMPTS
        ));
        assert!(!should_recreate_surface(
            SurfacePhase::SurfaceLost,
            false,
            u8::MAX
        ));
    }
}
