//! Pure, host-testable helpers shared by the JNI layer.
//!
//! These carry no `jni`/`ndk`/GPU dependency so they compile and are unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls them
//! ([`crate::jni_glue`], [`crate::app`]) is `#[cfg(target_os = "android")]`.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Whether an opaque handle from the JVM points at a live native side.
///
/// Kotlin initialises `handle` to `0` and every native call is guarded on it
/// (see `ForgeKitSurfaceView`), so `0` means "no native side yet / already
/// destroyed" and must be treated as a no-op rather than dereferenced.
#[inline]
pub(crate) fn handle_is_live(handle: i64) -> bool {
    handle != 0
}

/// Sanitize a raw density (`jfloat`) from JNI into a scale safe to divide or
/// multiply by: finite and strictly positive, else the `1.0` fallback.
///
/// The JVM-supplied `density` is untrusted input (spec §8.1's surface state
/// machine assumes a well-behaved platform, but density arrives through a
/// separate, unchecked `jfloat` argument); a bogus value here must never
/// propagate into a `NaN`/infinite or zero-divide layout or paint transform.
/// [`crate::app::AndroidAppHandle::frame`] must call this exactly once per
/// frame and reuse the identical result for both layout (`logical_size`) and
/// the paint transform, so the two passes can never disagree on scale.
#[inline]
pub(crate) fn sanitize_scale(raw: f32) -> f64 {
    if raw.is_finite() && raw > 0.0 {
        raw as f64
    } else {
        1.0
    }
}

/// Logical (density-independent) size from a physical pixel size and an
/// already-[`sanitize_scale`]d scale factor, mirroring the desktop shell's
/// HiDPI math (spec task 08): lay out in logical pixels, then scale the scene
/// by `scale` so glyphs re-rasterise sharp at physical resolution.
#[inline]
pub(crate) fn logical_size(physical_width: u32, physical_height: u32, scale: f64) -> (f64, f64) {
    (
        physical_width as f64 / scale,
        physical_height as f64 / scale,
    )
}

/// Run `f`, catching any panic so it can never unwind across the FFI boundary
/// (undefined behaviour); on panic, log at `error` and return `default`.
///
/// Every `extern "system"` JNI export routes its body through this (directly or
/// via [`crate::jni_glue`]) — a panic in a JVM callback must be turned into a
/// benign default, not an unwind into JNI-owned frames.
pub(crate) fn guard<T>(what: &str, default: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(_) => {
            log::error!("forgekit-shell-android: panic caught at FFI boundary in {what}");
            default
        }
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
    fn logical_size_divides_by_scale() {
        let (w, h) = logical_size(800, 600, 2.0);
        assert_eq!((w, h), (400.0, 300.0));
    }

    #[test]
    fn logical_size_identity_at_scale_one() {
        assert_eq!(logical_size(1080, 1920, 1.0), (1080.0, 1920.0));
    }

    #[test]
    fn sanitize_scale_passes_through_normal_values() {
        assert_eq!(sanitize_scale(2.0), 2.0);
        assert_eq!(sanitize_scale(1.0), 1.0);
        assert_eq!(sanitize_scale(0.75), 0.75_f64);
    }

    #[test]
    fn sanitize_scale_falls_back_on_bogus_values() {
        // Non-positive / non-finite densities must not divide-by-zero or NaN.
        for bad in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                sanitize_scale(bad),
                1.0,
                "scale {bad} should fall back to 1.0"
            );
        }
    }

    #[test]
    fn logical_size_falls_back_on_bogus_scale_once_sanitized() {
        // logical_size trusts its caller to have sanitized `scale` first (the
        // caller — AndroidAppHandle::frame — must do this exactly once and
        // reuse the result for both layout and paint).
        for bad in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
            let scale = sanitize_scale(bad);
            let (w, h) = logical_size(100, 200, scale);
            assert_eq!(
                (w, h),
                (100.0, 200.0),
                "scale {bad} should fall back to 1.0"
            );
        }
    }

    #[test]
    fn guard_returns_value_on_success() {
        assert_eq!(guard("ok", 0, || 42), 42);
    }

    #[test]
    fn guard_returns_default_on_panic() {
        let out = guard("boom", -1, || panic!("simulated JNI callback panic"));
        assert_eq!(
            out, -1,
            "a panic must be swallowed and the default returned"
        );
    }
}
