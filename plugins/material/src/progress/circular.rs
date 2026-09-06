//! Circular progress geometry: the ring's radius/gap resolution, the
//! track/active arc split shared by the classic and wavy painters, and the
//! radius-modulated wavy arc path. Like [`super::motion`], nothing here
//! touches a `PaintScene`, a `Theme`, or a callback.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/progress_indicators/` —
//! `components/m3e_circular_progress_painter.dart` (the classic track/active
//! arc split and its dual-gap treatment),
//! `components/m3e_circular_wavy_progress_painter.dart` (`waveK`, `gapAngle`,
//! `_paintIndeterminate`, `_paintDeterminate`, `_wavyArc`),
//! `styles/m3e_progress_indicator_theme.dart` (`M3ECircularProgressTheme`)
//! (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # One arc split, two painters
//!
//! The reference ships two painters whose gap arithmetic is character-for-
//! character identical (`appliedGap = min(activeSweep, gapAngle)`, then
//! `trackSweep = tau - activeSweep - 2 * appliedGap`) — the classic one
//! rotating its start angle and the wavy one rotating the whole canvas, which
//! about a shared centre is the same transform. [`arc_spans`] is therefore the
//! single split both variants use, and [`super`] decides only whether the
//! active span strokes as a plain arc or a wavy path.
//!
//! # Divergences from the reference, all render-identical
//!
//! - **The determinate wavy *track* is a true arc.** Upstream routes it
//!   through its wavy helper at amplitude `0`, i.e. 120 straight segments
//!   approximating a circle, while its own indeterminate arm draws a real
//!   `drawArc` for the same thing. This port draws the real arc in both.
//! - **A degenerate ring paints nothing.** Upstream divides by `radius` to get
//!   its gap angle with no guard; [`geometry`] returns `None` for a
//!   non-positive radius instead, so a box smaller than its own stroke can
//!   never produce an infinite or `NaN` angle.

use std::f64::consts::{PI, TAU};

use kurbo::{BezPath, Point, Size};

/// Diameter of a classic circular indicator, in logical px
/// (`M3ECircularProgressTheme.defaultSize`).
pub(crate) const DEFAULT_SIZE: f64 = 40.0;
/// Diameter of a wavy circular indicator, in logical px
/// (`M3ECircularProgressTheme.wavySize`).
pub(crate) const WAVY_SIZE: f64 = 48.0;
/// Active-arc stroke thickness, in logical px
/// (`M3ECircularProgressTheme.defaultStrokeWidth`).
pub(crate) const STROKE_WIDTH: f64 = 4.0;
/// Track-arc stroke thickness, in logical px
/// (`M3ECircularProgressTheme.trackStrokeWidth`).
pub(crate) const TRACK_STROKE_WIDTH: f64 = 4.0;
/// Clear space between the active arc and the track, in logical px
/// (`M3ECircularProgressTheme.gapSize`).
pub(crate) const GAP_SIZE: f64 = 8.0;
/// Peak wave offset from the ring's radius, in logical px
/// (`M3ECircularProgressTheme.waveAmplitude`).
pub(crate) const WAVE_AMPLITUDE: f64 = 1.6;
/// Wavelength along the ring, in logical px
/// (`M3ECircularProgressTheme.wavelength`).
pub(crate) const WAVELENGTH: f64 = 15.0;
/// Where every arc starts: straight up, at 12 o'clock
/// (both painters' `startAngle`).
pub(crate) const START_ANGLE: f64 = -PI / 2.0;
/// Straight-line samples the wavy arc is walked with (`_wavyArc`'s
/// `const steps = 120`).
const WAVY_ARC_STEPS: usize = 120;

/// The resolved ring a circular indicator paints on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    /// Ring centre in widget-local coordinates.
    pub(crate) center: Point,
    /// Ring radius, already reduced by the wave amplitude.
    pub(crate) radius: f64,
    /// Angular width of one gap between the active arc and the track.
    pub(crate) gap_angle: f64,
    /// Angular frequency of the wave along the ring, in radians per logical px
    /// of arc length.
    pub(crate) wave_k: f64,
}

/// Resolve the ring for a box of `size` at `origin`, or `None` when the box
/// cannot fit a positive radius.
///
/// `amplitude` is the *resolved* peak wave offset (already scaled by the
/// amplitude factor), `0` for a classic indicator — the reference's wavy
/// painter shrinks the radius by it so the crests stay inside the box, while
/// its classic painter has no amplitude to subtract.
pub(crate) fn geometry(
    origin: Point,
    size: Size,
    stroke: f64,
    track_stroke: f64,
    gap_size: f64,
    amplitude: f64,
    wavelength: f64,
) -> Option<Geometry> {
    let max_stroke = stroke.max(track_stroke);
    let shortest = size.width.min(size.height);
    let radius = (shortest - max_stroke) / 2.0 - amplitude;
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    // `waveK = max(1, round(tau * radius / wavelength)) * tau / (tau * radius)`
    // — a whole number of cycles around the circumference, so the wave closes
    // on itself. The `tau`s cancel: it is `cycles / radius` radians of phase
    // per logical px of arc length.
    let cycles = if wavelength > 0.0 {
        (TAU * radius / wavelength).round().max(1.0)
    } else {
        1.0
    };
    Some(Geometry {
        center: Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0),
        radius,
        gap_angle: (gap_size + (stroke + track_stroke) / 2.0) / radius,
        wave_k: cycles / radius,
    })
}

/// The track and active arcs for one frame, each `(start_angle, sweep_angle)`
/// in radians, or `None` where the reference paints nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ArcSpans {
    pub(crate) track: Option<(f64, f64)>,
    pub(crate) active: Option<(f64, f64)>,
}

/// Split the ring into its track and active arcs (both painters' shared gap
/// arithmetic — see the [module docs](self)).
///
/// `progress` is `None` for indeterminate, where `indeterminate_sweep` (the
/// clock's ping-ponged sweep) stands in for `progress * tau`. A determinate
/// indicator at `1.0` is a complete ring with no track and no gaps.
pub(crate) fn arc_spans(
    progress: Option<f64>,
    start: f64,
    indeterminate_sweep: f64,
    gap_angle: f64,
) -> ArcSpans {
    let progress = progress.map(|p| if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) });
    if progress.is_some_and(|p| p >= 1.0) {
        return ArcSpans {
            track: None,
            active: Some((start, TAU)),
        };
    }
    let active_sweep = match progress {
        Some(p) => p * TAU,
        None => indeterminate_sweep.clamp(0.0, TAU),
    };
    let applied_gap = active_sweep.min(gap_angle);
    let track_sweep = TAU - active_sweep - applied_gap * 2.0;
    ArcSpans {
        track: (track_sweep > 0.0).then_some((start + active_sweep + applied_gap, track_sweep)),
        active: (active_sweep > 0.0).then_some((start, active_sweep)),
    }
}

/// A circular arc whose radius oscillates sinusoidally with arc length
/// (`_wavyArc`).
///
/// The phase is anchored to `start_angle`, so a rotating indeterminate arc
/// carries its wave around with it rather than swimming through a fixed
/// standing pattern.
pub(crate) fn wavy_arc_path(
    geometry: &Geometry,
    start_angle: f64,
    sweep_angle: f64,
    amplitude: f64,
    phase: f64,
) -> BezPath {
    let mut path = BezPath::new();
    for i in 0..=WAVY_ARC_STEPS {
        let t = i as f64 / WAVY_ARC_STEPS as f64;
        let angle = start_angle + sweep_angle * t;
        let arc_length = geometry.radius * (angle - start_angle).abs();
        let r = geometry.radius + amplitude * (phase + arc_length * geometry.wave_k).sin();
        let point = Point::new(
            geometry.center.x + r * angle.cos(),
            geometry.center.y + r * angle.sin(),
        );
        if i == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    fn ring(amplitude: f64) -> Geometry {
        geometry(
            Point::ZERO,
            Size::new(DEFAULT_SIZE, DEFAULT_SIZE),
            STROKE_WIDTH,
            TRACK_STROKE_WIDTH,
            GAP_SIZE,
            amplitude,
            WAVELENGTH,
        )
        .expect("a 40dp box fits a ring")
    }

    // -- geometry -----------------------------------------------------------

    #[test]
    fn the_radius_insets_by_half_the_widest_stroke() {
        let g = ring(0.0);
        assert_eq!(g.radius, (DEFAULT_SIZE - STROKE_WIDTH) / 2.0);
        assert_eq!(g.center, Point::new(20.0, 20.0));
    }

    #[test]
    fn a_wave_amplitude_shrinks_the_radius_so_crests_stay_inside_the_box() {
        let flat = ring(0.0);
        let wavy = ring(WAVE_AMPLITUDE);
        assert_eq!(wavy.radius, flat.radius - WAVE_AMPLITUDE);
    }

    #[test]
    fn the_gap_angle_is_the_gap_plus_half_of_both_strokes_over_the_radius() {
        let g = ring(0.0);
        let expected = (GAP_SIZE + (STROKE_WIDTH + TRACK_STROKE_WIDTH) / 2.0) / g.radius;
        assert!((g.gap_angle - expected).abs() < 1e-12);
    }

    #[test]
    fn the_wave_closes_on_a_whole_number_of_cycles_around_the_ring() {
        let g = ring(WAVE_AMPLITUDE);
        let cycles = g.wave_k * g.radius;
        assert!(
            (cycles - cycles.round()).abs() < 1e-9,
            "waveK must be a whole cycle count over the radius, got {cycles}"
        );
        // The phase advanced over one full circumference is a whole number of
        // turns, which is what makes the wave seamless at 12 o'clock.
        let full_turn_phase = TAU * g.radius * g.wave_k;
        assert!((full_turn_phase / TAU - cycles).abs() < 1e-9);
    }

    #[test]
    fn a_box_smaller_than_its_own_stroke_resolves_no_ring() {
        assert!(
            geometry(
                Point::ZERO,
                Size::new(3.0, 3.0),
                STROKE_WIDTH,
                TRACK_STROKE_WIDTH,
                GAP_SIZE,
                0.0,
                WAVELENGTH,
            )
            .is_none()
        );
    }

    #[test]
    fn a_degenerate_wavelength_still_resolves_a_ring() {
        let g = geometry(
            Point::ZERO,
            Size::new(DEFAULT_SIZE, DEFAULT_SIZE),
            STROKE_WIDTH,
            TRACK_STROKE_WIDTH,
            GAP_SIZE,
            0.0,
            0.0,
        )
        .expect("a zero wavelength must not sink the ring");
        assert!((g.wave_k * g.radius - 1.0).abs() < 1e-12, "one cycle floor");
    }

    // -- the arc split ------------------------------------------------------

    #[test]
    fn a_determinate_arc_sweeps_proportionally_from_twelve_oclock() {
        let g = ring(0.0);
        let spans = arc_spans(Some(0.25), START_ANGLE, 0.0, g.gap_angle);
        let (start, sweep) = spans.active.expect("a quarter turn is painted");
        assert_eq!(start, START_ANGLE);
        assert!((sweep - TAU / 4.0).abs() < 1e-12);
        let (track_start, track_sweep) = spans.track.expect("three quarters of track remain");
        assert!((track_start - (START_ANGLE + sweep + g.gap_angle)).abs() < 1e-12);
        assert!((track_sweep - (TAU - sweep - 2.0 * g.gap_angle)).abs() < 1e-12);
    }

    #[test]
    fn a_complete_determinate_ring_has_no_track_and_no_gaps() {
        let spans = arc_spans(Some(1.0), START_ANGLE, 0.0, ring(0.0).gap_angle);
        assert_eq!(spans.active, Some((START_ANGLE, TAU)));
        assert_eq!(spans.track, None);
    }

    #[test]
    fn a_zero_determinate_ring_is_all_track() {
        let spans = arc_spans(Some(0.0), START_ANGLE, 0.0, ring(0.0).gap_angle);
        assert_eq!(spans.active, None);
        let (_, track_sweep) = spans.track.expect("the whole ring is track");
        assert!(
            (track_sweep - TAU).abs() < 1e-12,
            "with no active arc there is nothing to gap against"
        );
    }

    #[test]
    fn a_determinate_value_is_clamped_at_both_ends() {
        let gap = ring(0.0).gap_angle;
        assert_eq!(
            arc_spans(Some(4.0), START_ANGLE, 0.0, gap).active,
            Some((START_ANGLE, TAU))
        );
        assert_eq!(arc_spans(Some(-4.0), START_ANGLE, 0.0, gap).active, None);
        assert_eq!(
            arc_spans(Some(f64::NAN), START_ANGLE, 0.0, gap).active,
            None
        );
    }

    #[test]
    fn an_indeterminate_arc_takes_the_clocks_sweep_and_carries_its_rotation() {
        let g = ring(0.0);
        let rotation = 1.0_f64;
        let sweep = 0.5 * TAU;
        let spans = arc_spans(None, START_ANGLE + rotation, sweep, g.gap_angle);
        assert_eq!(spans.active, Some((START_ANGLE + rotation, sweep)));
        let (track_start, track_sweep) = spans.track.expect("half the ring is track");
        assert!((track_start - (START_ANGLE + rotation + sweep + g.gap_angle)).abs() < 1e-12);
        assert!((track_sweep - (TAU - sweep - 2.0 * g.gap_angle)).abs() < 1e-12);
    }

    #[test]
    fn the_gap_never_exceeds_the_active_sweep_it_is_carved_out_of() {
        let g = ring(0.0);
        // A sweep far narrower than the gap: the applied gap collapses onto it
        // rather than eating into the track twice over.
        let tiny = g.gap_angle / 4.0;
        let spans = arc_spans(None, START_ANGLE, tiny, g.gap_angle);
        let (_, track_sweep) = spans.track.expect("almost all track");
        assert!((track_sweep - (TAU - tiny - 2.0 * tiny)).abs() < 1e-12);
    }

    // -- the wavy arc path --------------------------------------------------

    #[test]
    fn the_wavy_arc_walks_the_reference_step_count() {
        let g = ring(WAVE_AMPLITUDE);
        let path = wavy_arc_path(&g, START_ANGLE, TAU, WAVE_AMPLITUDE, 0.0);
        assert_eq!(path.elements().len(), WAVY_ARC_STEPS + 1);
        assert!(matches!(path.elements().first(), Some(PathEl::MoveTo(_))));
    }

    #[test]
    fn the_wavy_arc_oscillates_between_the_radius_and_the_amplitude() {
        let g = ring(WAVE_AMPLITUDE);
        let path = wavy_arc_path(&g, START_ANGLE, TAU, WAVE_AMPLITUDE, 0.0);
        let radii: Vec<f64> = path
            .elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some((*p - g.center).hypot()),
                _ => None,
            })
            .collect();
        let max = radii.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min = radii.iter().cloned().fold(f64::INFINITY, f64::min);
        assert!(max <= g.radius + WAVE_AMPLITUDE + 1e-9);
        assert!(min >= g.radius - WAVE_AMPLITUDE - 1e-9);
        assert!(
            max - min > WAVE_AMPLITUDE,
            "a full turn must reach both a crest and a trough"
        );
    }

    #[test]
    fn a_zero_amplitude_wavy_arc_is_a_plain_circle() {
        let g = ring(0.0);
        let path = wavy_arc_path(&g, START_ANGLE, TAU, 0.0, 0.0);
        for el in path.elements() {
            if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                assert!(((*p - g.center).hypot() - g.radius).abs() < 1e-9);
            }
        }
    }
}
