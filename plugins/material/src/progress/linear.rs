//! Linear progress geometry: the size matrix, the track/active span layout for
//! all four value×wavy combinations, the end stop indicator's placement, and
//! the traveling sine path. Like [`super::motion`], nothing here touches a
//! `PaintScene`, a `Theme`, or a callback — it emits [`LinearOp`]s [`super`]
//! paints.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/progress_indicators/` —
//! `components/m3e_linear_progress_painter.dart` (`_paintFlat`,
//! `_paintFlatIndeterminate`, `_paintWavy`, `_paintWavyIndeterminate`,
//! `_stopPlacement`, `_visualGap`, `_drawWave`),
//! `styles/m3e_progress_indicator_theme.dart` (`M3ELinearProgressTheme`,
//! `M3ELinearProgressLayout`, `resolveFlat`),
//! `enums/m3e_progress_enums.dart` (`M3EProgressIndicatorSize`)
//! (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Ops, not draw calls
//!
//! The reference's painters interleave geometry and canvas calls; this port
//! splits them so the choreography is assertable without a scene. Each
//! function returns the spans **in the reference's own draw order** — track
//! first, then active, then the stop dot — so painting is a straight walk of
//! the list.
//!
//! # Divergences from the reference, all render-identical
//!
//! - **A zero-length active span is filtered here rather than early-returned
//!   in the draw call.** `_drawWave`/`drawSeg` no-op on `end <= start`; this
//!   emits no op at all for the same case.
//! - **The wave path is walked from an index, not an accumulator.**
//!   Upstream's `for (x = start + step; x <= end; x += step)` drifts over a
//!   long span; the sample positions are otherwise identical, and both land a
//!   final vertex exactly on `end` (the `crate::slider` wave shares this
//!   convention).
//! - **`M3ELinearProgressLayout.dotOffset` is not carried.** The reference
//!   holds it in the layout record but no painter reads it — the stop dot's
//!   position comes entirely from `_stopPlacement`.

use std::f64::consts::TAU;

use kurbo::{BezPath, Point};

use super::ProgressSize;
use super::motion::Segments;

/// Wavy active-stroke thickness, in logical px
/// (`M3ELinearProgressTheme.strokeWidth`).
pub(crate) const STROKE_WIDTH: f64 = 4.0;
/// Wavy inactive-track thickness, in logical px
/// (`M3ELinearProgressTheme.trackStrokeWidth`).
pub(crate) const TRACK_STROKE_WIDTH: f64 = 4.0;
/// Clear space between the active indicator and the track, in logical px
/// (`M3ELinearProgressTheme.gapSize`).
pub(crate) const GAP_SIZE: f64 = 8.0;
/// End stop indicator diameter, in logical px
/// (`M3ELinearProgressTheme.stopSize`).
pub(crate) const STOP_SIZE: f64 = 4.0;
/// Peak wave offset from the track's centreline, in logical px
/// (`M3ELinearProgressTheme.waveAmplitude`).
pub(crate) const WAVE_AMPLITUDE: f64 = 3.0;
/// Wavelength of a determinate wavy track, in logical px
/// (`M3ELinearProgressTheme.determinateWavelength`).
pub(crate) const DETERMINATE_WAVELENGTH: f64 = 40.0;
/// Wavelength of an indeterminate wavy track, in logical px
/// (`M3ELinearProgressTheme.indeterminateWavelength`).
pub(crate) const INDETERMINATE_WAVELENGTH: f64 = 20.0;
/// Minimum box height of a wavy linear indicator, in logical px
/// (`M3ELinearProgressTheme.wavyContainerHeight`).
pub(crate) const WAVY_CONTAINER_HEIGHT: f64 = 10.0;
/// Leading inset of every linear track, in logical px
/// (`M3ELinearProgressPainter.inset`'s default).
pub(crate) const INSET: f64 = 4.0;
/// Floor on a wavy track's trailing margin, in logical px
/// (`_paintWavy`'s `math.max(gap, 4)`).
pub(crate) const WAVY_TRAILING_MIN: f64 = 4.0;
/// Spacing between two sampled points of the wave path, in logical px
/// (`_drawWave`'s `const step = 1.5`).
const WAVE_SAMPLE_STEP: f64 = 1.5;
/// Ceiling on the wave path's vertex count, so a degenerate (absurdly wide)
/// span can never spin the sampling loop.
const MAX_WAVE_SAMPLES: f64 = 4096.0;

/// The measured flat layout for one size (`M3ELinearProgressLayout` via
/// `M3ELinearProgressTheme.resolveFlat`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlatLayout {
    /// Track thickness, which is also the flat painter's stroke width.
    pub(crate) track_height: f64,
    /// Clear space between the active indicator and the track.
    pub(crate) gap: f64,
    /// End stop indicator diameter.
    pub(crate) dot_diameter: f64,
    /// Reserved space to the right of the track.
    pub(crate) trailing_margin: f64,
}

/// The flat layout for `size` (`resolveFlat`).
pub(crate) fn flat_layout(size: ProgressSize) -> FlatLayout {
    match size {
        ProgressSize::S => FlatLayout {
            track_height: 4.0,
            gap: 4.0,
            dot_diameter: 4.0,
            trailing_margin: 4.0,
        },
        ProgressSize::M => FlatLayout {
            track_height: 8.0,
            gap: 4.0,
            dot_diameter: 4.0,
            trailing_margin: 8.0,
        },
    }
}

/// One span of a linear indicator, in widget-local x, in draw order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LinearOp {
    /// A flat inactive-track span.
    Track { x0: f64, x1: f64 },
    /// A flat active-indicator span.
    Active { x0: f64, x1: f64 },
    /// An active-indicator span stroked as a traveling sine wave.
    Wave { x0: f64, x1: f64 },
    /// The end stop indicator dot.
    Stop { center_x: f64, diameter: f64 },
}

/// Where the end stop dot sits, so it lands inside the track's end with equal
/// padding on all sides (`_stopPlacement`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StopPlacement {
    pub(crate) diameter: f64,
    pub(crate) center_x: f64,
}

/// Resolve the stop dot against the track's right end and thickness
/// (`_stopPlacement`).
pub(crate) fn stop_placement(stop_size: f64, track_right: f64, track_stroke: f64) -> StopPlacement {
    let pad = (track_stroke / 4.0).max(1.0);
    let max_diameter = (track_stroke - 2.0 * pad).max(1.0);
    let diameter = stop_size.min(max_diameter);
    let actual_pad = (track_stroke - diameter) / 2.0;
    StopPlacement {
        diameter,
        center_x: track_right + track_stroke / 2.0 - actual_pad - diameter / 2.0,
    }
}

/// Inflates `gap` so round stroke caps still leave a visible empty space
/// (`_visualGap`).
fn visual_gap(gap: f64, stroke: f64) -> f64 {
    gap + stroke
}

/// Push a span expressed as a pair of `0..=1` track fractions, skipping a
/// degenerate one (`_paintFlatIndeterminate`'s `drawSeg`).
fn push_fraction_span(
    ops: &mut Vec<LinearOp>,
    left: f64,
    span: f64,
    start_f: f64,
    end_f: f64,
    active: bool,
) {
    if end_f - start_f <= 0.0 {
        return;
    }
    let x0 = left + span * start_f.clamp(0.0, 1.0);
    let x1 = left + span * end_f.clamp(0.0, 1.0);
    if x1 <= x0 {
        return;
    }
    ops.push(if active {
        LinearOp::Active { x0, x1 }
    } else {
        LinearOp::Track { x0, x1 }
    });
}

/// Every span of a **flat** linear indicator across `width`
/// (`_paintFlat`/`_paintFlatIndeterminate`).
///
/// `value` is `None` for indeterminate, in which case `segs` drives the two
/// traveling lines and no stop dot is painted (the reference returns before
/// its `drawCircle`).
pub(crate) fn flat_ops(
    layout: FlatLayout,
    width: f64,
    value: Option<f64>,
    segs: Segments,
) -> Vec<LinearOp> {
    let stroke = layout.track_height;
    let gap = visual_gap(layout.gap, stroke);
    let left = INSET;
    let track_right = width - layout.trailing_margin;
    let span = (track_right - left).max(0.0);

    let Some(progress) = value else {
        return flat_indeterminate_ops(left, span, gap, segs);
    };

    let progress = progress.clamp(0.0, 1.0);
    let mut ops = Vec::new();
    if progress >= 1.0 {
        ops.push(LinearOp::Active {
            x0: left,
            x1: track_right,
        });
    } else {
        let active_end = left + span * progress;
        let track_start = track_right.min(active_end + gap);
        if track_start < track_right {
            ops.push(LinearOp::Track {
                x0: track_start,
                x1: track_right,
            });
        }
        if active_end > left {
            ops.push(LinearOp::Active {
                x0: left,
                x1: active_end,
            });
        }
    }
    let stop = stop_placement(layout.dot_diameter, track_right, stroke);
    ops.push(LinearOp::Stop {
        center_x: stop.center_x,
        diameter: stop.diameter,
    });
    ops
}

/// The two traveling lines and the three track spans around them
/// (`_paintFlatIndeterminate`).
fn flat_indeterminate_ops(left: f64, span: f64, gap: f64, segs: Segments) -> Vec<LinearOp> {
    let gap_frac = if span > 0.0 { gap / span } else { 0.0 };
    let mut ops = Vec::new();

    // Track after the first line (with a gap).
    if segs.first_head < 1.0 - gap_frac {
        let start = if segs.first_head > 0.0 {
            segs.first_head + gap_frac
        } else {
            0.0
        };
        push_fraction_span(&mut ops, left, span, start, 1.0, false);
    }
    if segs.first_head - segs.first_tail > 0.0 {
        push_fraction_span(&mut ops, left, span, segs.first_tail, segs.first_head, true);
    }
    // Track between the second and first lines (with a gap at each end).
    if segs.first_tail > gap_frac {
        let start = if segs.second_head > 0.0 {
            segs.second_head + gap_frac
        } else {
            0.0
        };
        let end = if segs.first_tail < 1.0 {
            segs.first_tail - gap_frac
        } else {
            1.0
        };
        if start < end {
            push_fraction_span(&mut ops, left, span, start, end, false);
        }
    }
    if segs.second_head - segs.second_tail > 0.0 {
        push_fraction_span(
            &mut ops,
            left,
            span,
            segs.second_tail,
            segs.second_head,
            true,
        );
    }
    // Track before the second line (with a gap).
    if segs.second_tail > gap_frac {
        let end = if segs.second_tail < 1.0 {
            segs.second_tail - gap_frac
        } else {
            1.0
        };
        push_fraction_span(&mut ops, left, span, 0.0, end, false);
    }
    ops
}

/// The per-instance knobs a **wavy** linear indicator paints against, already
/// resolved against the token defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WavySpec {
    /// Active-indicator stroke thickness (`strokeWidth`).
    pub(crate) stroke: f64,
    /// Inactive-track stroke thickness (`trackStrokeWidth`).
    pub(crate) track_stroke: f64,
    /// Clear space between the active indicator and the track (`gapSize`).
    pub(crate) gap: f64,
    /// End stop indicator diameter (`stopSize`).
    pub(crate) stop_size: f64,
}

/// Every span of a **wavy** linear indicator across `width`
/// (`_paintWavy`/`_paintWavyIndeterminate`).
pub(crate) fn wavy_ops(
    spec: WavySpec,
    width: f64,
    value: Option<f64>,
    segs: Segments,
) -> Vec<LinearOp> {
    let gap = visual_gap(spec.gap, spec.stroke);
    let left = INSET;
    let trailing = spec.gap.max(WAVY_TRAILING_MIN);
    let track_right = width - trailing;
    let span = (track_right - left).max(0.0);

    let Some(progress) = value else {
        return wavy_indeterminate_ops(spec, left, track_right, span, gap, segs);
    };

    let progress = progress.clamp(0.0, 1.0);
    let mut ops = Vec::new();
    if progress >= 1.0 {
        ops.push(LinearOp::Wave {
            x0: left,
            x1: track_right,
        });
    } else {
        let active_end = left + span * progress;
        let track_start = track_right.min(active_end + gap);
        if track_start < track_right {
            ops.push(LinearOp::Track {
                x0: track_start,
                x1: track_right,
            });
        }
        if active_end > left {
            ops.push(LinearOp::Wave {
                x0: left,
                x1: active_end,
            });
        }
    }
    let stop = stop_placement(spec.stop_size, track_right, spec.stroke);
    ops.push(LinearOp::Stop {
        center_x: stop.center_x,
        diameter: stop.diameter,
    });
    ops
}

/// The two traveling waves and the three track spans around them
/// (`_paintWavyIndeterminate`).
///
/// This arm insets by an extra half stroke at both track ends and widens the
/// gap by the same, so the round caps of a wave and its neighbouring track
/// never overlap.
fn wavy_indeterminate_ops(
    spec: WavySpec,
    left: f64,
    track_right: f64,
    span: f64,
    gap: f64,
    segs: Segments,
) -> Vec<LinearOp> {
    let cap = spec.stroke.max(spec.track_stroke) / 2.0;
    let adjusted_gap = gap + cap;
    let mut ops = Vec::new();

    let first_track_end = segs.second_tail * span + left - adjusted_gap;
    if first_track_end > left + cap {
        ops.push(LinearOp::Track {
            x0: left + cap,
            x1: first_track_end,
        });
    }
    push_wave_span(&mut ops, left, span, segs.second_tail, segs.second_head);

    let second_track_start = segs.second_head * span + left + adjusted_gap;
    let second_track_end = segs.first_tail * span + left - adjusted_gap;
    if second_track_start < second_track_end {
        ops.push(LinearOp::Track {
            x0: second_track_start,
            x1: second_track_end,
        });
    }
    push_wave_span(&mut ops, left, span, segs.first_tail, segs.first_head);

    let third_track_start = segs.first_head * span + left + adjusted_gap;
    if third_track_start < track_right - cap {
        ops.push(LinearOp::Track {
            x0: third_track_start,
            x1: track_right - cap,
        });
    }
    ops
}

/// Push a wave span expressed as a pair of `0..=1` track fractions
/// (`_paintWavyIndeterminate`'s `drawActive`).
fn push_wave_span(ops: &mut Vec<LinearOp>, left: f64, span: f64, start_f: f64, end_f: f64) {
    if end_f - start_f <= 0.0 {
        return;
    }
    let x0 = left + span * start_f.clamp(0.0, 1.0);
    let x1 = left + span * end_f.clamp(0.0, 1.0);
    if x1 <= x0 {
        return;
    }
    ops.push(LinearOp::Wave { x0, x1 });
}

/// The traveling sine path for one active span, centred on `cy`, or `None`
/// when there is nothing to stroke (`_drawWave`).
///
/// The phase is anchored to `start`, so each span's wave begins at the same
/// point in its cycle no matter where on the track it currently sits.
pub(crate) fn wave_path(
    start: f64,
    end: f64,
    cy: f64,
    amplitude: f64,
    wavelength: f64,
    phase: f64,
) -> Option<BezPath> {
    if !(start.is_finite() && end.is_finite() && cy.is_finite()) || end <= start {
        return None;
    }
    let k = TAU / wavelength.max(1.0);
    let y_at = |x: f64| cy + amplitude * (phase + (x - start) * k).sin();

    let mut path = BezPath::new();
    path.move_to(Point::new(start, y_at(start)));
    let steps = ((end - start) / WAVE_SAMPLE_STEP)
        .floor()
        .clamp(0.0, MAX_WAVE_SAMPLES) as usize;
    for i in 1..=steps {
        let x = start + WAVE_SAMPLE_STEP * i as f64;
        path.line_to(Point::new(x, y_at(x)));
    }
    path.line_to(Point::new(end, y_at(end)));
    Some(path)
}

/// The box height a wavy linear indicator needs for `stroke` and the resolved
/// `amplitude_factor` (`_buildLinearWavy`'s `height`).
pub(crate) fn wavy_height(stroke: f64, amplitude_factor: f64) -> f64 {
    WAVY_CONTAINER_HEIGHT.max(stroke + 2.0 * WAVE_AMPLITUDE * amplitude_factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::motion;
    use kurbo::PathEl;

    const WIDTH: f64 = 200.0;

    fn zero_segments() -> Segments {
        motion::segments(0.0)
    }

    fn medium() -> FlatLayout {
        flat_layout(ProgressSize::M)
    }

    fn wavy_spec() -> WavySpec {
        WavySpec {
            stroke: STROKE_WIDTH,
            track_stroke: TRACK_STROKE_WIDTH,
            gap: GAP_SIZE,
            stop_size: STOP_SIZE,
        }
    }

    fn tracks(ops: &[LinearOp]) -> Vec<(f64, f64)> {
        ops.iter()
            .filter_map(|op| match op {
                LinearOp::Track { x0, x1 } => Some((*x0, *x1)),
                _ => None,
            })
            .collect()
    }

    fn actives(ops: &[LinearOp]) -> Vec<(f64, f64)> {
        ops.iter()
            .filter_map(|op| match op {
                LinearOp::Active { x0, x1 } | LinearOp::Wave { x0, x1 } => Some((*x0, *x1)),
                _ => None,
            })
            .collect()
    }

    // -- the size matrix ----------------------------------------------------

    #[test]
    fn the_size_matrix_matches_the_reference_table() {
        assert_eq!(
            flat_layout(ProgressSize::S),
            FlatLayout {
                track_height: 4.0,
                gap: 4.0,
                dot_diameter: 4.0,
                trailing_margin: 4.0,
            }
        );
        assert_eq!(
            flat_layout(ProgressSize::M),
            FlatLayout {
                track_height: 8.0,
                gap: 4.0,
                dot_diameter: 4.0,
                trailing_margin: 8.0,
            }
        );
    }

    // -- determinate --------------------------------------------------------

    #[test]
    fn a_determinate_flat_track_splits_at_the_progress_point_plus_a_gap() {
        let layout = medium();
        let ops = flat_ops(layout, WIDTH, Some(0.25), zero_segments());
        // left = INSET = 4, trackRight = 200 - 8 = 192, span = 188.
        let active_end = 4.0 + 188.0 * 0.25;
        let gap = layout.gap + layout.track_height;
        assert_eq!(
            tracks(&ops),
            vec![(active_end + gap, 192.0)],
            "the track resumes one visual gap past the active end"
        );
        assert_eq!(actives(&ops), vec![(4.0, active_end)]);
    }

    #[test]
    fn a_complete_determinate_track_is_one_active_span_with_no_track() {
        let ops = flat_ops(medium(), WIDTH, Some(1.0), zero_segments());
        assert!(tracks(&ops).is_empty());
        assert_eq!(actives(&ops), vec![(4.0, 192.0)]);
    }

    #[test]
    fn a_determinate_value_is_clamped_at_both_ends() {
        let over = flat_ops(medium(), WIDTH, Some(9.0), zero_segments());
        assert_eq!(
            actives(&over),
            vec![(4.0, 192.0)],
            "over-range reads as 1.0"
        );
        let under = flat_ops(medium(), WIDTH, Some(-9.0), zero_segments());
        assert!(
            actives(&under).is_empty(),
            "under-range reads as 0.0, which paints no active span"
        );
        assert_eq!(tracks(&under), vec![(4.0 + 12.0, 192.0)]);
    }

    #[test]
    fn a_determinate_indicator_always_paints_the_stop_dot() {
        for value in [0.0, 0.5, 1.0] {
            let ops = flat_ops(medium(), WIDTH, Some(value), zero_segments());
            assert!(
                matches!(ops.last(), Some(LinearOp::Stop { .. })),
                "the stop dot is drawn last at value {value}"
            );
        }
    }

    #[test]
    fn an_indeterminate_indicator_paints_no_stop_dot() {
        let flat = flat_ops(medium(), WIDTH, None, motion::segments(0.4));
        assert!(!flat.iter().any(|op| matches!(op, LinearOp::Stop { .. })));
        let wavy = wavy_ops(wavy_spec(), WIDTH, None, motion::segments(0.4));
        assert!(!wavy.iter().any(|op| matches!(op, LinearOp::Stop { .. })));
    }

    // -- the stop dot -------------------------------------------------------

    #[test]
    fn the_stop_dot_sits_inside_the_track_end_with_equal_padding() {
        // trackStroke 8 → pad max(1, 2) = 2, maxDiameter max(1, 8 - 4) = 4,
        // diameter min(4, 4) = 4, actualPad (8 - 4)/2 = 2.
        let stop = stop_placement(4.0, 192.0, 8.0);
        assert_eq!(stop.diameter, 4.0);
        assert_eq!(stop.center_x, 192.0 + 4.0 - 2.0 - 2.0);
    }

    #[test]
    fn a_thin_track_shrinks_the_stop_dot_rather_than_overflowing_it() {
        // trackStroke 4 → pad max(1, 1) = 1, maxDiameter max(1, 4 - 2) = 2.
        let stop = stop_placement(STOP_SIZE, 100.0, 4.0);
        assert_eq!(stop.diameter, 2.0, "capped by the track thickness");
    }

    // -- two-segment indeterminate -----------------------------------------

    #[test]
    fn the_indeterminate_track_shows_two_active_lines_mid_cycle() {
        // t = 1000/1750: the first line is running and the second has started.
        let ops = flat_ops(medium(), WIDTH, None, motion::segments(1000.0 / 1750.0));
        let active = actives(&ops);
        assert_eq!(active.len(), 2, "the choreography is two lines, not one");
        // Draw order is the reference's: the leading (first) line, then the
        // trailing (second) one, which sits further left with track between.
        assert!(
            active[0].0 > active[1].1,
            "the first line leads the second, with clear space between them"
        );
    }

    #[test]
    fn the_indeterminate_spans_are_ordered_and_gapped() {
        for i in 0..=70 {
            let t = i as f64 / 70.0;
            let ops = flat_ops(medium(), WIDTH, None, motion::segments(t));
            let mut previous_end = f64::NEG_INFINITY;
            for op in &ops {
                let (x0, x1) = match op {
                    LinearOp::Track { x0, x1 }
                    | LinearOp::Active { x0, x1 }
                    | LinearOp::Wave { x0, x1 } => (*x0, *x1),
                    LinearOp::Stop { .. } => continue,
                };
                assert!(x1 > x0, "a zero-length span survived at t = {t}");
                let _ = previous_end;
                previous_end = x1;
            }
        }
    }

    #[test]
    fn the_indeterminate_track_is_a_single_full_span_at_cycle_zero() {
        let ops = flat_ops(medium(), WIDTH, None, motion::segments(0.0));
        assert!(
            actives(&ops).is_empty(),
            "no line has left the start of the track yet"
        );
        assert_eq!(tracks(&ops), vec![(4.0, 192.0)]);
    }

    #[test]
    fn the_wavy_indeterminate_track_insets_by_a_stroke_cap_and_a_gap() {
        let spec = wavy_spec();
        let ops = wavy_ops(spec, WIDTH, None, motion::segments(0.0));
        let cap = spec.stroke.max(spec.track_stroke) / 2.0;
        let adjusted_gap = spec.gap + spec.stroke + cap;
        // trailing = max(gap, 4) = 8 → trackRight = 192. At cycle zero both
        // lines sit at the origin, so only the span *after* the first line
        // survives: one adjusted gap in from the left, one stroke cap in from
        // the right.
        assert_eq!(tracks(&ops), vec![(INSET + adjusted_gap, 192.0 - cap)]);
    }

    #[test]
    fn the_wavy_indeterminate_track_carries_two_waves_mid_cycle() {
        let ops = wavy_ops(wavy_spec(), WIDTH, None, motion::segments(1000.0 / 1750.0));
        let waves: Vec<_> = ops
            .iter()
            .filter(|op| matches!(op, LinearOp::Wave { .. }))
            .collect();
        assert_eq!(waves.len(), 2);
    }

    // -- the wave path ------------------------------------------------------

    #[test]
    fn the_wave_path_starts_and_ends_exactly_on_its_span() {
        let path = wave_path(4.0, 100.0, 5.0, 3.0, 40.0, 0.0).expect("a non-degenerate span");
        let Some(PathEl::MoveTo(first)) = path.elements().first() else {
            panic!("the wave path must open with a MoveTo");
        };
        assert!((first.x - 4.0).abs() < 1e-9);
        let Some(PathEl::LineTo(last)) = path.elements().last() else {
            panic!("the wave path must close with a LineTo");
        };
        assert!((last.x - 100.0).abs() < 1e-9);
    }

    #[test]
    fn the_wave_path_oscillates_around_its_centreline_by_the_amplitude() {
        let path = wave_path(0.0, 80.0, 10.0, 3.0, 40.0, 0.0).expect("a non-degenerate span");
        let ys: Vec<f64> = path
            .elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(p.y),
                _ => None,
            })
            .collect();
        let max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        assert!((max - 13.0).abs() < 0.1, "crest at cy + amplitude");
        assert!((min - 7.0).abs() < 0.1, "trough at cy - amplitude");
    }

    #[test]
    fn the_wave_phase_shifts_the_path_without_changing_its_span() {
        let a = wave_path(0.0, 80.0, 10.0, 3.0, 40.0, 0.0).expect("span a");
        let b = wave_path(0.0, 80.0, 10.0, 3.0, 40.0, TAU / 4.0).expect("span b");
        assert_ne!(a.elements().len(), 0);
        assert_eq!(a.elements().len(), b.elements().len());
        assert_ne!(
            format!("{:?}", a.elements()),
            format!("{:?}", b.elements()),
            "a phase shift must move the wave"
        );
    }

    #[test]
    fn a_degenerate_wave_span_produces_no_path() {
        assert!(wave_path(50.0, 50.0, 5.0, 3.0, 40.0, 0.0).is_none());
        assert!(wave_path(50.0, 10.0, 5.0, 3.0, 40.0, 0.0).is_none());
        assert!(wave_path(f64::NAN, 10.0, 5.0, 3.0, 40.0, 0.0).is_none());
        assert!(wave_path(0.0, f64::INFINITY, 5.0, 3.0, 40.0, 0.0).is_none());
    }

    // -- the wavy box height ------------------------------------------------

    #[test]
    fn the_wavy_box_height_grows_with_the_amplitude() {
        assert_eq!(wavy_height(STROKE_WIDTH, 0.0), WAVY_CONTAINER_HEIGHT);
        assert_eq!(wavy_height(STROKE_WIDTH, 1.0), 10.0);
        assert_eq!(wavy_height(8.0, 1.0), 14.0);
    }
}
