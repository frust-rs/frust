//! Pure, host-testable helpers shared by every platform shell's FFI layer.
//!
//! These carry no `jni`/`ndk`/GPU dependency so they compile and are unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls them
//! (a shell's FFI boundary and per-frame driver) is platform-gated.

use std::panic::{AssertUnwindSafe, catch_unwind};

use frust_core::WindowMetrics;
use frust_core::insets::{EdgeInsets, WindowInsets};
use kurbo::Size;

/// Sanitize a raw density (e.g. a JNI `jfloat`) into a scale safe to divide or
/// multiply by: finite and strictly positive, else the `1.0` fallback.
///
/// The platform-supplied `density` is untrusted input (the surface state
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
/// HiDPI math: lay out in logical pixels, then scale the scene
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
/// [`WindowInsets`]. Each edge value is sanitized (non-finite or negative → 0.0)
/// **before** being divided by `scale` to convert device px to logical px,
/// mirroring [`logical_size`]'s HiDPI math and [`sanitize_scale`]'s defensive
/// posture. A platform-supplied inset must never propagate non-finite or negative
/// into the layout/paint passes. The framework receives insets in the same
/// logical space it lays out in (the logical-coordinate contract — the
/// same discipline pointer events follow).
///
/// `scale` must already have passed through [`sanitize_scale`] (finite,
/// strictly positive); a shell's per-frame driver sanitizes the platform
/// density exactly once and reuses the identical result here and for
/// [`logical_size`] so the two never disagree on scale — this function trusts
/// that contract exactly as [`logical_size`] does.
#[inline]
pub fn logical_insets(physical: [f64; 8], scale: f64) -> WindowInsets {
    let sanitize_edge = |px: f64| if px.is_finite() && px >= 0.0 { px } else { 0.0 };
    let to_logical = |px: f64| sanitize_edge(px) / scale;
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

/// Assemble the app-facing [`WindowMetrics`] from a shell's raw surface
/// dimensions, its already-[`sanitize_scale`]d scale, and the window's
/// already-**logical** [`WindowInsets`].
///
/// The one place the three shells agree on units. `physical` is the surface's
/// device-pixel size (Android's `nativeOnSurfaceChanged` pair, iOS's
/// `frust_resize` pair, winit's `inner_size()`), converted to logical px here
/// through [`logical_size`] — so [`WindowMetrics::size`] is logical on every
/// platform, exactly like the coordinates and insets that already cross this
/// boundary (`docs/CODE_STANDARDS.md`'s physical-at-FFI/logical-inside rule).
/// `insets` is passed through unchanged because each shell has *already*
/// resolved it to logical px via [`logical_insets`] (Android divides by its
/// display density, iOS uses the identity scale because UIKit hands over
/// points) — this must never re-divide it.
///
/// [`WindowMetrics::orientation`](frust_core::WindowMetrics) is derived from the
/// logical size by [`WindowMetrics::new`]; no platform callback carries an
/// orientation enum.
///
/// `scale` must already have passed through [`sanitize_scale`] (finite,
/// strictly positive) — the same trust contract [`logical_size`] and
/// [`logical_insets`] state, so a shell sanitizes the platform density once per
/// use and feeds the identical value to all three.
#[inline]
pub fn window_metrics(physical: (u32, u32), scale: f64, insets: WindowInsets) -> WindowMetrics {
    let (logical_w, logical_h) = logical_size(physical.0, physical.1, scale);
    WindowMetrics::new(Size::new(logical_w, logical_h), scale, insets)
}

/// Per-shell-instance change detector over the window's [`WindowMetrics`]: each
/// of the three shells owns one and drives it from its own resize/insets entry
/// points, publishing only what it reports as *changed*.
///
/// # Why this is not a per-frame push
///
/// Delivering `WindowMetrics` to app code means `provide_context`-ing it under
/// the reactive root owner, exactly as `Theme` and [`WindowInsets`] already are
/// — a plain map insert that notifies nothing and creates no subscription, so
/// re-providing does not itself wake a frame. The guard here exists for cost
/// at the FFI boundary instead: a shell that re-provided metrics
/// unconditionally once per frame would pay a lock write plus an allocation
/// every frame for no observable benefit, since the next rebuild (already
/// driven by the resize or inset change itself) is what actually picks the
/// new value up. This type makes the guarded path the only path:
/// [`poll`](Self::poll) returns `Some` **only** on an actual change, mirroring
/// [`ThemeOverrideWatcher`](crate::ThemeOverrideWatcher)'s poll-returns-`Option`
/// shape and `RenderRoot::set_insets`'s `PartialEq`-guarded no-op.
///
/// It is deliberately reactive-free (this crate ships no reactive dependency —
/// see `docs/ARCHITECTURE.md`'s Layer Dependencies): it decides *whether* to
/// publish and computes *what* to publish, and each shell owns the
/// `provide_context` call itself. That keeps the unit conversion and the
/// change-detection host-testable even though both mobile `app` modules are
/// target-gated and never host-compiled.
#[derive(Debug, Default)]
pub struct WindowMetricsPublisher {
    /// The metrics last reported as changed, or `None` before the first
    /// [`poll`](Self::poll) — so the seeding poll a shell runs before its very
    /// first rebuild always publishes.
    last: Option<WindowMetrics>,
}

impl WindowMetricsPublisher {
    /// A fresh publisher that has published nothing yet.
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Assemble the current [`WindowMetrics`] (see [`window_metrics`] for the
    /// unit contract) and return it **only if it differs** from the last one
    /// this publisher reported; `None` means the shell must not re-provide.
    ///
    /// A shell calls this from every point where one of the inputs actually
    /// moves — surface create/resize and the insets callback — never from its
    /// per-frame driver, which only *reads* values these entry points already
    /// stored.
    pub fn poll(
        &mut self,
        physical: (u32, u32),
        scale: f64,
        insets: WindowInsets,
    ) -> Option<WindowMetrics> {
        let metrics = window_metrics(physical, scale, insets);
        if self.last == Some(metrics) {
            return None;
        }
        self.last = Some(metrics);
        Some(metrics)
    }

    /// The metrics last published, or `None` before the first change-reporting
    /// [`poll`](Self::poll).
    pub fn last(&self) -> Option<WindowMetrics> {
        self.last
    }
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
            log::error!("frust-shell: panic caught at FFI boundary in {what}");
            default
        }
    }
}

/// Run a shell-spawned render-thread body, catching any panic so the thread
/// **exits cleanly** instead of unwinding out of the thread closure — following
/// the same `catch_unwind` + `AssertUnwindSafe` + `error`-log convention as
/// [`guard`], but for a whole-thread closure rather than an FFI entry point.
///
/// A dev-build render-thread panic logs here and returns, which runs the
/// closure's owned `RenderReceiver`'s [`Drop`] — the receiver-liveness drain
/// (see `render_split`'s `RenderReceiver`) that fires any orphaned `Ack`'s
/// safety net, so a UI/main thread blocked on a `Pause`/`SurfaceDestroyed`
/// barrier unblocks rather than deadlocking. This is a **diagnosability** aid,
/// not the correctness anchor: correctness rests on the receiver-liveness drain,
/// which fires on *any* thread exit (clean early `return`, hang teardown, or
/// this caught panic).
///
/// **No-op under the release `panic = "abort"` profile** (root `Cargo.toml`):
/// there `catch_unwind` never catches — a panic aborts the whole process before
/// unwinding — so this wrapper matters only in dev / `panic = "unwind"` builds.
/// The barrier deadlock the caught panic would otherwise cause bites exactly
/// those non-abort builds (plus clean early-returns and hung threads, which this
/// wrapper does not touch — the drain covers those).
pub fn run_guarded_thread(what: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        log::error!(
            "frust-shell: panic caught in render thread {what}; thread exiting cleanly \
             (surface teardown + ack drain run via RenderReceiver drop)"
        );
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
    fn logical_insets_sanitizes_nan_edges_to_zero() {
        // Non-finite edge values must not propagate NaN into the layout/paint passes.
        let physical = [f64::NAN, 44.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let insets = logical_insets(physical, 2.0);
        assert_eq!(insets.view_padding, EdgeInsets::new(0.0, 22.0, 0.0, 0.0));
    }

    #[test]
    fn logical_insets_sanitizes_infinite_edges_to_zero() {
        // Non-finite edge values must not propagate Inf into the layout/paint passes.
        let physical = [
            f64::INFINITY,
            44.0,
            f64::NEG_INFINITY,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        let insets = logical_insets(physical, 2.0);
        assert_eq!(insets.view_padding, EdgeInsets::new(0.0, 22.0, 0.0, 0.0));
    }

    #[test]
    fn logical_insets_sanitizes_negative_edges_to_zero() {
        // Negative edge values must not propagate as insets (they're nonsensical).
        let physical = [-10.0, 44.0, 0.0, -5.0, 0.0, 0.0, 0.0, 0.0];
        let insets = logical_insets(physical, 2.0);
        assert_eq!(insets.view_padding, EdgeInsets::new(0.0, 22.0, 0.0, 0.0));
    }

    #[test]
    fn logical_insets_normal_values_unchanged() {
        // Valid finite, non-negative edges pass through normally.
        let physical = [8.0, 44.0, 16.0, 34.0, 0.0, 0.0, 0.0, 340.0];
        let insets = logical_insets(physical, 2.0);
        assert_eq!(insets.view_padding, EdgeInsets::new(4.0, 22.0, 8.0, 17.0));
        assert_eq!(insets.view_insets, EdgeInsets::new(0.0, 0.0, 0.0, 170.0));
    }

    // --- WindowMetrics: units, derived orientation, change detection ---------
    //
    // This is where the three shells' `WindowMetrics` publish path gets its
    // coverage: both mobile `app` modules are target-gated (never host-compiled)
    // and the desktop handler needs a live winit window, so the shared,
    // reactive-free helper below is the only host-testable surface — the same
    // reason `logical_insets`/`sanitize_scale` live in this module.

    use frust_core::Orientation;

    #[test]
    fn window_metrics_size_is_logical_on_every_shell() {
        // Android: `nativeOnSurfaceChanged` reports device pixels + density.
        // A 1080x2400 @3x phone surface lays out as 360x800 logical.
        let android = window_metrics((1080, 2400), 3.0, WindowInsets::default());
        assert_eq!(android.size, Size::new(360.0, 800.0));
        assert_eq!(android.scale, 3.0);

        // iOS: `frust_resize` reports the drawable's pixel size + UIScreen
        // scale, so the identical division applies (its *insets* are the only
        // already-logical input — see the insets test below).
        let ios = window_metrics((1170, 2532), 3.0, WindowInsets::default());
        assert_eq!(ios.size, Size::new(390.0, 844.0));

        // Desktop: winit `inner_size()` is physical too; a 2x HiDPI 1600x1200
        // window is 800x600 logical.
        let desktop = window_metrics((1600, 1200), 2.0, WindowInsets::default());
        assert_eq!(desktop.size, Size::new(800.0, 600.0));

        // Unit scale: physical and logical coincide (the non-HiDPI case).
        let unit = window_metrics((800, 600), 1.0, WindowInsets::default());
        assert_eq!(unit.size, Size::new(800.0, 600.0));
    }

    #[test]
    fn window_metrics_passes_already_logical_insets_through_untouched() {
        // The insets each shell hands in have ALREADY been through
        // `logical_insets` (Android divides by its density, iOS uses the
        // identity scale). Re-dividing them by `scale` here would halve a
        // status-bar inset on every @2x device — pin the pass-through.
        let logical = logical_insets([0.0, 48.0, 0.0, 68.0, 0.0, 0.0, 0.0, 0.0], 2.0);
        assert_eq!(logical.view_padding, EdgeInsets::new(0.0, 24.0, 0.0, 34.0));

        let metrics = window_metrics((800, 1600), 2.0, logical);
        assert_eq!(metrics.insets, logical);
        assert_eq!(
            metrics.insets.view_padding,
            EdgeInsets::new(0.0, 24.0, 0.0, 34.0)
        );
    }

    #[test]
    fn window_metrics_orientation_flips_when_width_and_height_cross_over() {
        // Derived from the LOGICAL size (no platform callback carries an
        // orientation enum), so a rotation reported purely as swapped surface
        // dimensions still flips it.
        let portrait = window_metrics((1080, 2400), 3.0, WindowInsets::default());
        assert_eq!(portrait.orientation, Orientation::Portrait);

        let landscape = window_metrics((2400, 1080), 3.0, WindowInsets::default());
        assert_eq!(landscape.orientation, Orientation::Landscape);

        // The crossover itself: an exact square reads as portrait (height >= width).
        let square = window_metrics((1000, 1000), 2.0, WindowInsets::default());
        assert_eq!(square.orientation, Orientation::Portrait);
    }

    #[test]
    fn window_metrics_uses_sanitized_scale_from_caller() {
        // Same trust contract as `logical_size`/`logical_insets`: the shell
        // sanitizes once and hands the result in.
        let scale = sanitize_scale(f32::NAN); // -> 1.0
        let metrics = window_metrics((400, 800), scale, WindowInsets::default());
        assert_eq!(metrics.size, Size::new(400.0, 800.0));
        assert_eq!(metrics.scale, 1.0);
    }

    #[test]
    fn window_metrics_publisher_reports_only_actual_changes() {
        // THE cost-guard anchor: a shell re-`provide_context`s only when this
        // returns `Some`. An unconditional per-frame re-provide would pay a
        // lock write plus an allocation every frame at the FFI boundary for
        // nothing observable, so every repeat below must be `None`.
        let mut pub_ = WindowMetricsPublisher::new();
        assert_eq!(pub_.last(), None, "nothing published before the first poll");

        // First poll (the shell's pre-first-rebuild seeding) always publishes.
        let first = pub_
            .poll((1080, 2400), 3.0, WindowInsets::default())
            .expect("the seeding poll must publish");
        assert_eq!(first.size, Size::new(360.0, 800.0));
        assert_eq!(pub_.last(), Some(first));

        // Steady state: the same inputs re-reported (a resize callback that
        // re-delivers unchanged dimensions, or a shell polling more than once)
        // must NOT re-provide.
        for _ in 0..100 {
            assert_eq!(
                pub_.poll((1080, 2400), 3.0, WindowInsets::default()),
                None,
                "unchanged metrics must never be re-provided"
            );
        }
        assert_eq!(
            pub_.last(),
            Some(first),
            "a no-op poll leaves the last value"
        );

        // A real rotation publishes once, then goes quiet again.
        let rotated = pub_
            .poll((2400, 1080), 3.0, WindowInsets::default())
            .expect("a rotation is a real change");
        assert_eq!(rotated.orientation, Orientation::Landscape);
        assert_eq!(pub_.poll((2400, 1080), 3.0, WindowInsets::default()), None);
    }

    #[test]
    fn window_metrics_publisher_detects_each_input_independently() {
        // Size, scale, and insets each move on their own platform callback
        // (`resize` vs. the insets report), so each must independently trip a
        // re-provide — and each must then settle.
        let base_insets = WindowInsets::default();
        let mut pub_ = WindowMetricsPublisher::new();
        assert!(pub_.poll((1080, 2400), 3.0, base_insets).is_some());

        // Size only (an in-place resize, e.g. a desktop window drag).
        assert!(pub_.poll((1080, 2000), 3.0, base_insets).is_some());
        assert!(pub_.poll((1080, 2000), 3.0, base_insets).is_none());

        // Scale only (a display-density config change at the same pixel size —
        // it changes the logical size too, but the point is the shell need not
        // special-case which input moved).
        assert!(pub_.poll((1080, 2000), 2.0, base_insets).is_some());
        assert!(pub_.poll((1080, 2000), 2.0, base_insets).is_none());

        // Insets only (the IME coming up: same size, same scale).
        let ime_up = logical_insets([0.0, 48.0, 0.0, 0.0, 0.0, 0.0, 0.0, 680.0], 2.0);
        assert!(pub_.poll((1080, 2000), 2.0, ime_up).is_some());
        assert!(pub_.poll((1080, 2000), 2.0, ime_up).is_none());

        // ...and back down again.
        assert!(pub_.poll((1080, 2000), 2.0, base_insets).is_some());
        assert!(pub_.poll((1080, 2000), 2.0, base_insets).is_none());
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

    #[test]
    fn run_guarded_thread_runs_the_body_to_completion() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let ran = AtomicBool::new(false);
        run_guarded_thread("ok", || ran.store(true, Ordering::SeqCst));
        assert!(ran.load(Ordering::SeqCst), "the body must run");
    }

    #[test]
    fn run_guarded_thread_swallows_a_panic() {
        // A render-thread panic must not unwind out of the wrapper (which would
        // unwind the thread closure); it is caught and logged, and control returns.
        run_guarded_thread("boom", || panic!("simulated render-thread panic"));
    }
}
