//! Android-specific FFI helper: the opaque-handle liveness check.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `forgekit-shell-common` and is imported at
//! its call sites. What remains here is JNI-handle specific: the sentinel check
//! for the `jlong` the JVM passes back into every native call.
//!
//! This carries no `jni`/`ndk`/GPU dependency so it compiles and is unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls it
//! ([`crate::jni_glue`]) is `#[cfg(target_os = "android")]`.

/// Whether an opaque handle from the JVM points at a live native side.
///
/// Kotlin initialises `handle` to `0` and every native call is guarded on it
/// (see `ForgeKitSurfaceView`), so `0` means "no native side yet / already
/// destroyed" and must be treated as a no-op rather than dereferenced.
#[inline]
pub(crate) fn handle_is_live(handle: i64) -> bool {
    handle != 0
}

/// A pointer gesture phase, decoupled from `forgekit_core::PointerPhase` so this
/// module stays host-testable (the core crate is Android-gated — see the crate's
/// `Cargo.toml`). [`crate::app`] maps this onto the core phase at the one
/// Android-only call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    /// A finger touched down.
    Down,
    /// A finger moved while down.
    Move,
    /// A finger lifted.
    Up,
    /// The gesture was cancelled by the platform.
    Cancel,
}

/// Map the action code the Kotlin `ForgeKitSurfaceView.onTouchEvent` sends into a
/// [`TouchPhase`].
///
/// The Kotlin side normalises `MotionEvent.actionMasked` into a fixed ABI —
/// `ACTION_DOWN`/`ACTION_POINTER_DOWN` → `0`, `ACTION_MOVE` → `1`,
/// `ACTION_UP`/`ACTION_POINTER_UP` → `2`, `ACTION_CANCEL` → `3` — so this side
/// never sees a raw Android constant. Any unrecognised code (a future action we
/// don't map, or a corrupt value) is treated as [`TouchPhase::Cancel`]: the safe
/// default, since it releases any in-flight capture rather than stranding a
/// gesture as perpetually "down".
#[inline]
pub(crate) fn touch_phase_from_action(action: i32) -> TouchPhase {
    match action {
        0 => TouchPhase::Down,
        1 => TouchPhase::Move,
        2 => TouchPhase::Up,
        // 3 is the explicit `ACTION_CANCEL`; everything else falls back to it.
        _ => TouchPhase::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_handle_is_not_live() {
        assert!(!handle_is_live(0));
    }

    #[test]
    fn nonzero_handle_is_live() {
        assert!(handle_is_live(1));
        assert!(handle_is_live(-1));
        assert!(handle_is_live(0x7fff_ffff_ffff_ffff));
    }

    #[test]
    fn touch_action_codes_map_to_phases() {
        assert_eq!(touch_phase_from_action(0), TouchPhase::Down);
        assert_eq!(touch_phase_from_action(1), TouchPhase::Move);
        assert_eq!(touch_phase_from_action(2), TouchPhase::Up);
        assert_eq!(touch_phase_from_action(3), TouchPhase::Cancel);
    }

    #[test]
    fn unknown_touch_action_falls_back_to_cancel() {
        // A future/corrupt code must release capture, not strand a "down".
        assert_eq!(touch_phase_from_action(4), TouchPhase::Cancel);
        assert_eq!(touch_phase_from_action(-1), TouchPhase::Cancel);
        assert_eq!(touch_phase_from_action(i32::MAX), TouchPhase::Cancel);
    }
}
