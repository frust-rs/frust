//! Pure, host-testable helpers shared by every platform shell's FFI layer.
//!
//! These carry no `jni`/`ndk`/GPU dependency so they compile and are unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls them
//! (a shell's FFI boundary and per-frame driver) is platform-gated.

use std::panic::{AssertUnwindSafe, catch_unwind};

use forgekit_core::insets::{EdgeInsets, WindowInsets};

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

/// Build a logical (density-independent) [`WindowInsets`] from the platform's
/// raw **physical**-px per-edge inset values and an already-[`sanitize_scale`]d
/// scale factor.
///
/// `physical` packs the two per-edge sets a shell reads from the platform, in
/// device px, in this fixed order:
///
/// ```text
/// [0] view_padding.left    [4] view_insets.left
/// [1] view_padding.top     [5] view_insets.top
/// [2] view_padding.right   [6] view_insets.right
/// [3] view_padding.bottom  [7] view_insets.bottom
/// ```
///
/// where `view_padding` is the system-UI occlusion (status/navigation bars,
/// cutout) and `view_insets` the fully-obscured area (the IME) — see
/// [`WindowInsets`]. Each value is divided by `scale` to convert device px to
/// logical px, mirroring [`logical_size`]'s HiDPI math, so the framework
/// receives insets in the same logical space it lays out in (spec §10.3's
/// logical-coordinate contract — the same discipline pointer events follow).
///
/// `scale` must already have passed through [`sanitize_scale`] (finite,
/// strictly positive); a shell's per-frame driver sanitizes the platform
/// density exactly once and reuses the identical result here and for
/// [`logical_size`] so the two never disagree on scale — this function trusts
/// that contract exactly as [`logical_size`] does.
#[inline]
pub fn logical_insets(physical: [f64; 8], scale: f64) -> WindowInsets {
    let to_logical = |px: f64| px / scale;
    WindowInsets::new(
        EdgeInsets::new(
            to_logical(physical[0]),
            to_logical(physical[1]),
            to_logical(physical[2]),
            to_logical(physical[3]),
        ),
        EdgeInsets::new(
            to_logical(physical[4]),
            to_logical(physical[5]),
            to_logical(physical[6]),
            to_logical(physical[7]),
        ),
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
    fn logical_insets_divides_each_edge_by_scale() {
        // A @2x display: a 48px status bar + 68px home-indicator padding, IME up.
        let insets = logical_insets([0.0, 48.0, 0.0, 68.0, 0.0, 0.0, 0.0, 680.0], 2.0);
        assert_eq!(
            insets,
            WindowInsets::new(
                EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
                EdgeInsets::new(0.0, 0.0, 0.0, 340.0),
            )
        );
        // The derived safe-area padding collapses the bottom while the IME is up.
        assert_eq!(insets.padding(), EdgeInsets::new(0.0, 24.0, 0.0, 0.0));
    }

    #[test]
    fn logical_insets_identity_at_scale_one() {
        let physical = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let insets = logical_insets(physical, 1.0);
        assert_eq!(insets.view_padding, EdgeInsets::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(insets.view_insets, EdgeInsets::new(5.0, 6.0, 7.0, 8.0));
    }

    #[test]
    fn logical_insets_uses_sanitized_scale_from_caller() {
        // Mirrors `logical_size`: the helper trusts an already-sanitized scale.
        let scale = sanitize_scale(f32::NAN); // -> 1.0
        let insets = logical_insets([0.0, 44.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], scale);
        assert_eq!(insets.view_padding, EdgeInsets::new(0.0, 44.0, 0.0, 0.0));
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
