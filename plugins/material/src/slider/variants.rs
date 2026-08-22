//! The variant geometry the wavy and vertical sliders add on top of
//! [`super::core`]: the axis map that turns primary-axis geometry into widget
//! coordinates, and the traveling-sine-wave recipe that replaces the active
//! segment's flat fill. Like `core`, nothing here touches a `PaintScene`, a
//! `Theme`, or a callback — [`super`] and [`super::range`] own every paint
//! call.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/sliders/` — `m3e_sliders.dart` (`.wavy`/`.wavyCentered`/
//! `.vertical`/`.verticalCentered` constructors, `_phase`, `_amplitudeFactor`),
//! `components/m3e_slider_track_painter.dart` (`_drawWavyActive`,
//! `_trackBounds`, `_drawSegment`'s vertical arm),
//! `components/m3e_slider_build.dart` (`reverse`, the vertical stack body),
//! `styles/m3e_slider_theme.dart` (`amplitudeForProgress`),
//! `res/m3e_slider_tokens.dart` (`waveAmplitude`, `wavelength`,
//! `verticalHandleHeight`) (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Two spaces, and exactly one flip between them
//!
//! `core` computes in **painter space**: a primary-axis scalar where `0` is the
//! track's *minimum* end and `extent` its maximum, whichever screen edge each
//! lands on. [`AxisMap`] is the only thing that knows where those edges
//! actually are; it maps `(primary, cross)` into widget-local `(x, y)`,
//! transposing the two for a vertical slider and mirroring the primary axis
//! (`extent - primary`) for a bottom-up one. That single flip is this port's
//! transcription of the reference's `Transform.flip(flipY: true)` around its
//! track layer (`m3e_slider_build.dart:129-131`), and it is why no `core`
//! formula needs a vertical arm.
//!
//! The reference's flip wraps the *track layer only* — its thumb, value
//! indicator, and relocating icon are positioned in widget space from an
//! already-mirrored `thumbPrimary`. Two consumers therefore stay in widget
//! space here too and are mapped with [`AxisMap::widget_square`] rather than
//! the painter-space placements: the relocating icon (whose dock geometry
//! `core::icon_dock` resolves from `reverse` directly) and the value indicator.
//!
//! # The wave
//!
//! A wavy slider replaces the active segment's rounded rect with a stroked sine
//! path of the same thickness, inset half a stroke at each end so the round
//! caps land on the flat segment's own outer edges — the handle↔track gap is
//! identical either way (`_drawWavyActive`). Three inputs shape it:
//!
//! - **`wavelength`** — one full cycle's length in logical px ([`WAVELENGTH`]).
//! - **`wave_speed`** — travel in logical px per second, defaulting to
//!   `wavelength` (one cycle per second), so [`wave_phase`] advances `2π` per
//!   second at the default.
//! - **`amplitude`** — the peak cross-axis offset ([`WAVE_AMPLITUDE`]) scaled
//!   by an amplitude *factor*: an app-supplied constant, an app-supplied
//!   function of progress, or [`amplitude_ramp`], which flattens the wave
//!   entirely near both ends of the track.
//!
//! The phase advances off a repeating [`WaveClock`], not a wall clock. Its
//! controller only exposes a wrapping `0..1` phase within the current period,
//! so the clock counts wraps to recover a monotone elapsed-seconds reading —
//! the same shape [`crate::loading_indicator`] uses for its shape index, and
//! the equivalent of the reference's `lastElapsedDuration`, which likewise
//! accumulates across repeats.
//!
//! **Reduce-motion**: the wave is a perpetual decorative loop, so it requests
//! paced frames (`TickClass::CosmeticLoop`) and, under
//! `MotionScheme::reduce_motion`, freezes wherever its phase currently sits and
//! stops requesting frames — the crate convention
//! (`docs/WIDGETS_CODE_STANDARDS.md`), matching
//! [`crate::progress`]/[`crate::loading_indicator`]. The wave is still *drawn*;
//! only its travel stops.

use std::f64::consts::TAU;
use std::time::Duration;

use frust::authoring::CornerRadii;
use frust::{AnimationController, Curve, FrameTime};
use kurbo::{BezPath, Point, Rect, Size};

use crate::tokens::MaterialMotion;

/// Peak offset of the wavy active track from its centreline, in logical px
/// (`M3ESliderTokens.waveAmplitude`).
pub(crate) const WAVE_AMPLITUDE: f64 = 3.0;
/// Length of one full sine cycle on a wavy active track, in logical px
/// (`M3ESliderTokens.wavelength`).
pub(crate) const WAVELENGTH: f64 = 40.0;
/// Period of the repeating wave-phase controller
/// (`m3e_sliders.dart:476-479`'s `M3EMotion.extraLong2`). Only the *rate* the
/// clock is sampled at, never a visible cycle: [`WaveClock`] accumulates across
/// wraps, so the phase never jumps at a period boundary.
pub(crate) const WAVE_PERIOD: Duration = MaterialMotion::EXTRA_LONG_2;
/// Primary-axis spacing between two sampled points of the wave path, in
/// logical px (`m3e_slider_track_painter.dart:357`'s `const step = 1.5`).
pub(crate) const WAVE_SAMPLE_STEP: f64 = 1.5;
/// Clear space between a vertical slider's cross edge and its value indicator,
/// in logical px (`m3e_slider_build.dart:441`'s `cross + 8`).
pub(crate) const VALUE_INDICATOR_SIDE_SPACE: f64 = 8.0;
/// How far above the thumb a vertical slider's value indicator starts, in
/// logical px (`m3e_slider_build.dart:443`'s `thumbPrimary - 12`) — a rough
/// vertical centring on the thumb, as upstream.
pub(crate) const VALUE_INDICATOR_SIDE_LIFT: f64 = 12.0;

/// Which way a slider's primary axis runs (`M3ESlider.axis`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SliderAxis {
    /// Primary axis is x (`Axis.horizontal`).
    #[default]
    Horizontal,
    /// Primary axis is y (`Axis.vertical`).
    Vertical,
}

/// Maps painter-space geometry onto a widget's own coordinates. See the
/// [module docs](self) for the two spaces this sits between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AxisMap {
    axis: SliderAxis,
    /// Track length along the primary axis.
    extent: f64,
    /// The control box's extent along the cross axis.
    cross: f64,
    /// Whether the primary axis runs backwards in widget space — a bottom-up
    /// vertical slider (`!topToBottom`), or RTL on a horizontal one, which
    /// this port does not wire yet but costs nothing to carry.
    reverse: bool,
}

impl AxisMap {
    /// The map for a control of `size`, on `axis`, running `reverse`d or not.
    pub(crate) fn new(axis: SliderAxis, size: Size, reverse: bool) -> Self {
        let (extent, cross) = match axis {
            SliderAxis::Horizontal => (size.width, size.height),
            SliderAxis::Vertical => (size.height, size.width),
        };
        Self {
            axis,
            extent,
            cross,
            reverse,
        }
    }

    /// Track length along the primary axis.
    pub(crate) fn extent(&self) -> f64 {
        self.extent
    }

    /// The cross-axis centreline the track and thumb are centred on.
    pub(crate) fn cross_center(&self) -> f64 {
        self.cross / 2.0
    }

    /// A painter-space primary coordinate in widget-space primary units.
    pub(crate) fn flip(&self, primary: f64) -> f64 {
        if self.reverse {
            self.extent - primary
        } else {
            primary
        }
    }

    /// The primary component of an incoming pointer position — x horizontally,
    /// y vertically. Widget space: `ValueSpec::value_from_offset` applies
    /// `reverse` itself, so this must *not* pre-flip.
    pub(crate) fn event_primary(&self, position: Point) -> f64 {
        match self.axis {
            SliderAxis::Horizontal => position.x,
            SliderAxis::Vertical => position.y,
        }
    }

    /// A painter-space `(primary, cross)` pair in widget-local coordinates.
    pub(crate) fn widget_point(&self, primary: f64, cross: f64) -> Point {
        let primary = self.flip(primary);
        match self.axis {
            SliderAxis::Horizontal => Point::new(primary, cross),
            SliderAxis::Vertical => Point::new(cross, primary),
        }
    }

    /// Place a painter-space span of `thickness`, centred on the cross axis,
    /// carrying the asymmetric corner radii its two ends take. A reversed axis
    /// swaps which widget-space end each radius lands on, exactly as flipping
    /// the painted layer would.
    pub(crate) fn place_span(
        &self,
        start: f64,
        end: f64,
        thickness: f64,
        start_corner: f64,
        end_corner: f64,
    ) -> PlacedSpan {
        let (lo, hi) = self.widget_span(start, end);
        let (lo_corner, hi_corner) = if self.reverse {
            (end_corner, start_corner)
        } else {
            (start_corner, end_corner)
        };
        let cross0 = self.cross_center() - thickness / 2.0;
        let (origin, size, radii) = match self.axis {
            SliderAxis::Horizontal => (
                Point::new(lo, cross0),
                Size::new(hi - lo, thickness),
                CornerRadii::new(lo_corner, hi_corner, hi_corner, lo_corner),
            ),
            SliderAxis::Vertical => (
                Point::new(cross0, lo),
                Size::new(thickness, hi - lo),
                CornerRadii::new(lo_corner, lo_corner, hi_corner, hi_corner),
            ),
        };
        PlacedSpan {
            origin,
            size,
            radii,
        }
    }

    /// The thumb's box for a painter-space centre: `thickness` along the
    /// primary axis, `length` across it (`m3e_slider_build.dart:268-274`'s
    /// width/height swap, which is exactly this transposition).
    pub(crate) fn place_thumb(&self, primary: f64, thickness: f64, length: f64) -> Rect {
        let center = self.widget_point(primary, self.cross_center());
        match self.axis {
            SliderAxis::Horizontal => {
                super::core::thumb_rect(center.x, center.y, thickness, length)
            }
            SliderAxis::Vertical => Rect::from_center_size(center, Size::new(length, thickness)),
        }
    }

    /// The top-left corner of a `size`-square box centred on a painter-space
    /// primary coordinate (a stop/tick dot).
    pub(crate) fn place_square(&self, primary: f64, size: f64) -> Point {
        self.widget_square(self.flip(primary), size)
    }

    /// [`Self::place_square`] for a coordinate that is already in widget space
    /// — the relocating icon, whose dock resolves there (see the
    /// [module docs](self)).
    pub(crate) fn widget_square(&self, widget_primary: f64, size: f64) -> Point {
        let half = size / 2.0;
        let cross0 = self.cross_center() - half;
        match self.axis {
            SliderAxis::Horizontal => Point::new(widget_primary - half, cross0),
            SliderAxis::Vertical => Point::new(cross0, widget_primary - half),
        }
    }

    /// The top-left corner of a `size`-square box whose *leading* edge sits at
    /// a painter-space primary coordinate (an inset track icon).
    pub(crate) fn place_leading_square(&self, primary: f64, size: f64) -> Point {
        let (lo, _) = self.widget_span(primary, primary + size);
        let cross0 = self.cross_center() - size / 2.0;
        match self.axis {
            SliderAxis::Horizontal => Point::new(lo, cross0),
            SliderAxis::Vertical => Point::new(cross0, lo),
        }
    }

    /// The value indicator's top-left corner for a painter-space thumb. A
    /// horizontal slider floats it above the control
    /// (`core::value_indicator_origin`); a vertical one parks it beside the
    /// track's trailing cross edge (`m3e_slider_build.dart:441-444`). Both
    /// overflow the control's own box, as upstream's `Clip.none` stack does.
    pub(crate) fn value_indicator_origin(&self, thumb_primary: f64) -> Point {
        match self.axis {
            SliderAxis::Horizontal => super::core::value_indicator_origin(thumb_primary),
            SliderAxis::Vertical => Point::new(
                self.cross + VALUE_INDICATOR_SIDE_SPACE,
                self.flip(thumb_primary) - VALUE_INDICATOR_SIDE_LIFT,
            ),
        }
    }

    /// A painter-space span as an ordered widget-space `(low, high)` pair.
    fn widget_span(&self, start: f64, end: f64) -> (f64, f64) {
        if self.reverse {
            (self.flip(end), self.flip(start))
        } else {
            (start, end)
        }
    }
}

/// One track span placed in widget-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedSpan {
    pub(crate) origin: Point,
    pub(crate) size: Size,
    pub(crate) radii: CornerRadii,
}

/// The per-instance wave knobs, resolved against the token defaults
/// (`M3ESlider.wavelength`/`waveSpeed`/`amplitude`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WaveOverrides {
    pub(crate) wavelength: Option<f64>,
    pub(crate) wave_speed: Option<f64>,
    pub(crate) amplitude: Option<f64>,
}

impl WaveOverrides {
    /// Wave length in logical px (`resolved.wavelength`).
    pub(crate) fn wavelength(&self) -> f64 {
        self.wavelength.unwrap_or(WAVELENGTH)
    }

    /// Travel speed in logical px per second — defaulting to the *wavelength*,
    /// i.e. one cycle per second (`m3e_slider_build.dart:94`).
    pub(crate) fn wave_speed(&self) -> f64 {
        self.wave_speed.unwrap_or_else(|| self.wavelength())
    }
}

/// The wave's phase in radians after `seconds` of travel
/// (`_M3ESliderState._phase`). A non-positive wavelength has no wave at all, so
/// its phase is `0`.
pub(crate) fn wave_phase(seconds: f64, wavelength: f64, wave_speed: f64) -> f64 {
    if wavelength <= 0.0 {
        return 0.0;
    }
    seconds * wave_speed / wavelength * TAU
}

/// The default amplitude factor for a given progress
/// (`M3ESliderTheme.amplitudeForProgress`): full amplitude through the middle
/// of the track, flat near either end, so the wave never fights the round caps
/// of a nearly-empty or nearly-full active span.
pub(crate) fn amplitude_ramp(progress: f64) -> f64 {
    if progress <= 0.1 || progress >= 0.95 {
        0.0
    } else {
        1.0
    }
}

/// The amplitude factor for one paint pass, in the reference's own precedence
/// order (`_M3ESliderState._amplitudeFactor`): an app-supplied function of
/// progress first, then an app-supplied constant, then [`amplitude_ramp`].
/// Always clamped to `0..=1`.
pub(crate) fn amplitude_factor(
    progress: f64,
    fixed: Option<f64>,
    for_progress: Option<&dyn Fn(f64) -> f64>,
) -> f64 {
    let raw = match (for_progress, fixed) {
        (Some(f), _) => f(progress),
        (None, Some(a)) => a,
        (None, None) => amplitude_ramp(progress),
    };
    raw.clamp(0.0, 1.0)
}

/// The traveling sine path for one active span, in widget-local coordinates,
/// or `None` when there is nothing to stroke (`_drawWavyActive`).
///
/// `start`/`end` are the flat segment's own painter-space ends; the path is
/// inset by half of `stroke` at each so the round caps land flush with them.
/// The phase is anchored to `start` (pre-inset) so the wave's travel stays
/// stable as the thumb moves and the visible span's length changes.
pub(crate) fn wave_path(
    map: &AxisMap,
    start: f64,
    end: f64,
    stroke: f64,
    amplitude: f64,
    wavelength: f64,
    phase: f64,
) -> Option<BezPath> {
    if end <= start || wavelength <= 0.0 {
        return None;
    }
    let half_stroke = stroke / 2.0;
    let path_start = start + half_stroke;
    let path_end = end - half_stroke;
    if path_end <= path_start {
        return None;
    }

    let cross_center = map.cross_center();
    let k = TAU / wavelength;
    let cross_at = |primary: f64| cross_center + amplitude * (phase + (primary - start) * k).sin();

    let mut path = BezPath::new();
    path.move_to(map.widget_point(path_start, cross_at(path_start)));
    // Sampled from an index rather than an accumulator so the step never
    // drifts over a long span; upstream's `x += step` loop is the same walk.
    let steps = ((path_end - path_start) / WAVE_SAMPLE_STEP).floor() as usize;
    for i in 1..=steps {
        let primary = path_start + WAVE_SAMPLE_STEP * i as f64;
        path.line_to(map.widget_point(primary, cross_at(primary)));
    }
    path.line_to(map.widget_point(path_end, cross_at(path_end)));
    Some(path)
}

/// The repeating phase clock behind a wavy track. See the [module docs](self)
/// for why it counts wraps.
#[derive(Debug)]
pub(crate) struct WaveClock {
    timer: AnimationController,
    /// Completed periods since the clock started.
    cycles: u64,
    /// Last sampled phase within the current period, `0..1`.
    phase: f64,
}

impl Default for WaveClock {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveClock {
    /// A clock at phase zero, already repeating.
    pub(crate) fn new() -> Self {
        let mut timer = AnimationController::new(WAVE_PERIOD).with_curve(Curve::Linear);
        timer.repeat();
        Self {
            timer,
            cycles: 0,
            phase: 0.0,
        }
    }

    /// Advance to frame time `now`, counting a wrap when the period rolls
    /// over. A frozen (reduce-motion) pass simply does not call this.
    pub(crate) fn step(&mut self, now: FrameTime) {
        self.timer.advance(now);
        let phase = self.timer.value_clamped();
        if phase < self.phase {
            self.cycles = self.cycles.saturating_add(1);
        }
        self.phase = phase;
    }

    /// Elapsed travel time, monotone across period wraps.
    pub(crate) fn seconds(&self) -> f64 {
        (self.cycles as f64 + self.phase) * WAVE_PERIOD.as_secs_f64()
    }

    /// The current wave phase in radians for the resolved knobs.
    pub(crate) fn radians(&self, wave: &WaveOverrides) -> f64 {
        wave_phase(self.seconds(), wave.wavelength(), wave.wave_speed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    const EXTENT: f64 = 200.0;
    const CROSS: f64 = 44.0;
    const TRACK: f64 = 16.0;

    fn horizontal() -> AxisMap {
        AxisMap::new(
            SliderAxis::Horizontal,
            Size::new(EXTENT, CROSS),
            /* reverse */ false,
        )
    }

    /// The vertical default: `topToBottom: false`, so the minimum is at the
    /// bottom and sliding up increases the value.
    fn vertical_up() -> AxisMap {
        AxisMap::new(SliderAxis::Vertical, Size::new(CROSS, EXTENT), true)
    }

    fn vertical_down() -> AxisMap {
        AxisMap::new(SliderAxis::Vertical, Size::new(CROSS, EXTENT), false)
    }

    // -- axis mapping -------------------------------------------------------

    #[test]
    fn a_vertical_map_transposes_the_axes_and_reads_y_from_a_pointer() {
        let h = horizontal();
        let v = vertical_up();
        assert_eq!(h.extent(), EXTENT);
        assert_eq!(v.extent(), EXTENT, "extent is the height when vertical");
        assert_eq!(v.cross_center(), CROSS / 2.0);

        assert_eq!(h.widget_point(50.0, 22.0), Point::new(50.0, 22.0));
        // Value grows upward: painter 50 lands 50 from the *bottom*.
        assert_eq!(v.widget_point(50.0, 22.0), Point::new(22.0, 150.0));
        assert_eq!(
            vertical_down().widget_point(50.0, 22.0),
            Point::new(22.0, 50.0)
        );

        assert_eq!(h.event_primary(Point::new(30.0, 9.0)), 30.0);
        assert_eq!(
            v.event_primary(Point::new(9.0, 30.0)),
            30.0,
            "a pointer's primary is raw y — `value_from_offset` owns the flip"
        );
    }

    #[test]
    fn a_reversed_span_mirrors_its_position_and_its_corner_radii() {
        let ltr = horizontal().place_span(0.0, 92.0, TRACK, 8.0, 2.0);
        assert_eq!(ltr.origin, Point::new(0.0, 14.0));
        assert_eq!(ltr.size, Size::new(92.0, TRACK));
        assert_eq!(ltr.radii, CornerRadii::new(8.0, 2.0, 2.0, 8.0));

        // Same span on a top-down vertical slider: transposed, radii paired by
        // edge rather than by side.
        let down = vertical_down().place_span(0.0, 92.0, TRACK, 8.0, 2.0);
        assert_eq!(down.origin, Point::new(14.0, 0.0));
        assert_eq!(down.size, Size::new(TRACK, 92.0));
        assert_eq!(down.radii, CornerRadii::new(8.0, 8.0, 2.0, 2.0));

        // Bottom-up: the span moves to the far end and the radii swap with it.
        let up = vertical_up().place_span(0.0, 92.0, TRACK, 8.0, 2.0);
        assert_eq!(up.origin, Point::new(14.0, 108.0), "200 - 92");
        assert_eq!(up.size, Size::new(TRACK, 92.0));
        assert_eq!(up.radii, CornerRadii::new(2.0, 2.0, 8.0, 8.0));
    }

    #[test]
    fn a_thumb_and_a_dot_transpose_with_the_axis() {
        let thumb = horizontal().place_thumb(100.0, 4.0, CROSS);
        assert_eq!(thumb.size(), Size::new(4.0, CROSS));
        assert_eq!(thumb.center(), Point::new(100.0, 22.0));

        let thumb = vertical_up().place_thumb(100.0, 4.0, CROSS);
        assert_eq!(
            thumb.size(),
            Size::new(CROSS, 4.0),
            "thickness stays on the primary axis"
        );
        assert_eq!(thumb.center(), Point::new(22.0, 100.0));

        assert_eq!(
            horizontal().place_square(192.0, 4.0),
            Point::new(190.0, 20.0)
        );
        assert_eq!(
            vertical_up().place_square(192.0, 4.0),
            Point::new(20.0, 6.0)
        );
        // A widget-space coordinate (the relocating icon) is never re-flipped.
        assert_eq!(
            vertical_up().widget_square(192.0, 4.0),
            Point::new(20.0, 190.0)
        );
        // An inset track icon anchors its *leading* edge.
        assert_eq!(
            horizontal().place_leading_square(4.0, 16.0),
            Point::new(4.0, 14.0)
        );
        assert_eq!(
            vertical_up().place_leading_square(4.0, 16.0),
            Point::new(14.0, 180.0),
            "200 - (4 + 16)"
        );
    }

    #[test]
    fn the_value_indicator_floats_above_a_horizontal_slider_and_beside_a_vertical_one() {
        assert_eq!(
            horizontal().value_indicator_origin(100.0),
            super::super::core::value_indicator_origin(100.0)
        );
        let beside = vertical_up().value_indicator_origin(100.0);
        assert_eq!(beside.x, CROSS + VALUE_INDICATOR_SIDE_SPACE);
        assert_eq!(beside.y, 100.0 - VALUE_INDICATOR_SIDE_LIFT);
    }

    // -- wave ---------------------------------------------------------------

    #[test]
    fn the_amplitude_ramp_flattens_the_wave_near_both_ends() {
        assert_eq!(amplitude_ramp(0.0), 0.0);
        assert_eq!(amplitude_ramp(0.1), 0.0, "the ramp's own boundary");
        assert_eq!(amplitude_ramp(0.5), 1.0);
        assert_eq!(amplitude_ramp(0.94), 1.0);
        assert_eq!(amplitude_ramp(0.95), 0.0);
        assert_eq!(amplitude_ramp(1.0), 0.0);
    }

    #[test]
    fn an_explicit_amplitude_factor_outranks_the_ramp_and_clamps() {
        // A function of progress wins outright, even over a constant.
        let doubled = |p: f64| p * 2.0;
        assert_eq!(
            amplitude_factor(0.25, Some(0.9), Some(&doubled)),
            0.5,
            "the function is asked first"
        );
        assert_eq!(amplitude_factor(0.9, None, Some(&doubled)), 1.0, "clamped");
        assert_eq!(
            amplitude_factor(0.02, Some(0.4), None),
            0.4,
            "a constant beats the ramp"
        );
        assert_eq!(
            amplitude_factor(0.02, None, None),
            0.0,
            "and the ramp is the fallback"
        );
    }

    #[test]
    fn the_phase_advances_one_full_cycle_per_second_at_the_default_speed() {
        let wave = WaveOverrides::default();
        assert_eq!(wave.wavelength(), WAVELENGTH);
        assert_eq!(
            wave.wave_speed(),
            WAVELENGTH,
            "speed defaults to wavelength"
        );
        assert!((wave_phase(1.0, wave.wavelength(), wave.wave_speed()) - TAU).abs() < 1e-12);
        // Twice the speed, twice the travel; a dead wavelength has no phase.
        assert!((wave_phase(1.0, 40.0, 80.0) - 2.0 * TAU).abs() < 1e-12);
        assert_eq!(wave_phase(1.0, 0.0, 40.0), 0.0);
    }

    #[test]
    fn the_wave_clock_is_monotone_across_period_wraps() {
        let mut clock = WaveClock::new();
        assert_eq!(clock.seconds(), 0.0);
        // The first frame only seeds the controller's clock — every animation
        // here measures its own `dt` between two frames.
        clock.step(FrameTime::from_nanos(60_000_000));
        assert_eq!(clock.seconds(), 0.0, "the seeding frame does not travel");

        let mut last = 0.0;
        // 39 more frames of 60ms each = 2.34s, past two full 800ms periods.
        for frame in 2..=40u64 {
            clock.step(FrameTime::from_nanos(frame * 60_000_000));
            let now = clock.seconds();
            assert!(now > last, "frame {frame}: {now} must exceed {last}");
            last = now;
        }
        assert!((last - 2.34).abs() < 1e-9, "{last}");
        assert_eq!(clock.cycles, 2, "wraps counted, phase carries the rest");
        // And the radian phase inherits that monotonicity.
        assert!((clock.radians(&WaveOverrides::default()) - 2.34 * TAU).abs() < 1e-9);
    }

    #[test]
    fn the_wave_path_samples_a_sine_between_the_stroke_inset_ends() {
        let map = horizontal();
        // Phase 0, a 40px wavelength, and a span whose inset start is exactly
        // at a zero crossing: every sample is `sin` of its own offset.
        let path = wave_path(&map, 0.0, 92.0, TRACK, WAVE_AMPLITUDE, WAVELENGTH, 0.0)
            .expect("a span longer than its own stroke waves");
        let points: Vec<Point> = path
            .elements()
            .iter()
            .map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => *p,
                other => panic!("a sampled wave is moves and lines only: {other:?}"),
            })
            .collect();

        let k = TAU / WAVELENGTH;
        let expected = |x: f64| 22.0 + WAVE_AMPLITUDE * (x * k).sin();
        assert_eq!(
            points[0],
            Point::new(8.0, expected(8.0)),
            "inset half a stroke"
        );
        assert_eq!(
            *points.last().unwrap(),
            Point::new(84.0, expected(84.0)),
            "and ends half a stroke short of the segment"
        );
        // 76px of path at a 1.5px step: 50 interior samples plus both ends.
        assert_eq!(points.len(), 52);
        for (i, p) in points.iter().enumerate().take(51) {
            let x = 8.0 + WAVE_SAMPLE_STEP * i as f64;
            assert!((p.x - x).abs() < 1e-9, "sample {i} at {p:?}");
            assert!((p.y - expected(x)).abs() < 1e-9, "sample {i} at {p:?}");
        }
        // The amplitude is real: the wave leaves the centreline in both
        // directions inside one wavelength.
        let ys: Vec<f64> = points.iter().map(|p| p.y).collect();
        let hi = ys.iter().cloned().fold(f64::MIN, f64::max);
        let lo = ys.iter().cloned().fold(f64::MAX, f64::min);
        assert!((hi - (22.0 + WAVE_AMPLITUDE)).abs() < 0.02, "{hi}");
        assert!((lo - (22.0 - WAVE_AMPLITUDE)).abs() < 0.02, "{lo}");
    }

    #[test]
    fn a_shifted_phase_shifts_the_whole_wave() {
        let map = horizontal();
        let at = |phase: f64| {
            let path =
                wave_path(&map, 0.0, 92.0, TRACK, WAVE_AMPLITUDE, WAVELENGTH, phase).expect("path");
            match path.elements()[0] {
                PathEl::MoveTo(p) => p.y,
                _ => unreachable!("a path starts with a move"),
            }
        };
        let k = TAU / WAVELENGTH;
        assert!((at(0.0) - (22.0 + WAVE_AMPLITUDE * (8.0 * k).sin())).abs() < 1e-9);
        assert!(
            (at(TAU) - at(0.0)).abs() < 1e-9,
            "a full cycle of phase is the same wave again"
        );
        assert!((at(TAU / 2.0) - (22.0 - WAVE_AMPLITUDE * (8.0 * k).sin())).abs() < 1e-9);
    }

    #[test]
    fn a_vertical_wave_puts_the_sine_on_x() {
        let map = vertical_up();
        let path =
            wave_path(&map, 0.0, 92.0, TRACK, WAVE_AMPLITUDE, WAVELENGTH, 0.0).expect("path");
        let PathEl::MoveTo(first) = path.elements()[0] else {
            unreachable!("a path starts with a move")
        };
        let k = TAU / WAVELENGTH;
        assert_eq!(first.y, EXTENT - 8.0, "primary maps to y, mirrored");
        assert!((first.x - (22.0 + WAVE_AMPLITUDE * (8.0 * k).sin())).abs() < 1e-9);
    }

    #[test]
    fn a_span_shorter_than_its_own_stroke_has_no_wave() {
        let map = horizontal();
        assert!(wave_path(&map, 0.0, TRACK - 0.1, TRACK, 3.0, WAVELENGTH, 0.0).is_none());
        assert!(wave_path(&map, 50.0, 20.0, TRACK, 3.0, WAVELENGTH, 0.0).is_none());
        assert!(
            wave_path(&map, 0.0, 92.0, TRACK, 3.0, 0.0, 0.0).is_none(),
            "a dead wavelength paints nothing rather than dividing by zero"
        );
    }
}
