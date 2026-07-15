//! Pure, host-testable helpers shared by every platform shell's FFI layer.
//!
//! These carry no `jni`/`ndk`/GPU dependency so they compile and are unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls them
//! (a shell's FFI boundary and per-frame driver) is platform-gated.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Sanitize a raw density (e.g. a JNI `jfloat`) into a scale safe to divide or
/// multiply by: finite and strictly positive, else the `1.0` fallback.
///
/// The platform-supplied `density` is untrusted input (spec §8.1's surface state
/// machine assumes a well-behaved platform, but density arrives through a
/// separate, unchecked scalar argument); a bogus value here must never propagate
/// into a `NaN`/infinite or zero-divide layout or paint transform.
///
/// A shell's per-frame driver must call this exactly once per frame and reuse
/// the identical result for both layout (via [`logical_size`]) and the paint
/// transform, so the two passes can never disagree on scale.
#[inline]
pub fn sanitize_scale(raw: f32) -> f64 {
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
pub fn logical_size(physical_width: u32, physical_height: u32, scale: f64) -> (f64, f64) {
    (
        physical_width as f64 / scale,
        physical_height as f64 / scale,
    )
}

/// Run `f`, catching any panic so it can never unwind across the FFI boundary
/// (undefined behaviour); on panic, log at `error` and return `default`.
///
/// Every `extern` entry point a platform shell defines routes its body through
/// this — a panic in a platform callback must be turned into a benign default,
/// not an unwind into platform-owned frames.
pub fn guard<T>(what: &str, default: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(_) => {
            log::error!("forgekit-shell: panic caught at FFI boundary in {what}");
            default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // caller — a shell's per-frame driver — must do this exactly once and
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
        let out = guard("boom", -1, || panic!("simulated FFI callback panic"));
        assert_eq!(
            out, -1,
            "a panic must be swallowed and the default returned"
        );
    }
}
