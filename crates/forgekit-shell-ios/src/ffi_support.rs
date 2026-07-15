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
}
