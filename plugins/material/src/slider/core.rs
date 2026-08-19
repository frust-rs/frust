//! The variant-agnostic slider core: token constants, value↔geometry mapping,
//! track/thumb/dot/indicator geometry, the pointer drag state machine, and the
//! frame-clock haptic scheduler. [`super`] owns the `View`/`Widget` wiring and
//! every paint call; nothing in this module touches a `PaintScene`, a `Theme`,
//! or a callback.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/sliders/` — `m3e_sliders.dart`, `components/
//! m3e_slider_build.dart`, `m3e_slider_track_painter.dart`,
//! `m3e_slider_thumb.dart`, `m3e_slider_value_indicator.dart`,
//! `m3e_slider_track_icons_overlay.dart`, `utils/m3e_slider_math.dart`,
//! `utils/m3e_slider_track_paint_metrics.dart`, `utils/m3e_slider_dot_layout.dart`,
//! `utils/m3e_slider_dot_geometry.dart`, `res/m3e_slider_tokens.dart`,
//! `styles/m3e_slider_theme.dart` (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Primary-axis geometry, mapped once at paint
//!
//! Every geometry function here works in **primary-axis** coordinates — a
//! scalar offset along the track, `0.0` at the track's start and `extent` at
//! its end — never in `(x, y)`. The horizontal mapping (primary → x, cross →
//! y) lives entirely in [`super`]'s `paint`. This is the seam the vertical
//! variant needs: it swaps that one mapping (and feeds a reversed fraction),
//! with no change to any formula below.
//!
//! # Extension points the wavy/vertical/range variants hook into
//!
//! Wave-phase painting, vertical orientation, and the second thumb each hang
//! off a named seam here, so none of them reopens the state machine
//! ([`super::variants`] and [`super::range`] are the consumers):
//!
//! 1. **Wavy active segment** — [`TrackMetrics::segments`] emits the active
//!    span as a [`TrackSegment`] with [`SegmentRole::Active`]; a wavy variant
//!    paints *that one segment* as a stroked sine path instead of a rounded
//!    rect (the reference's own split: `m3e_slider_track_painter.dart:226-244`
//!    branches at exactly this point, leaving every other segment flat). The
//!    span endpoints are already the ones the reference's `_drawWavyActive`
//!    insets by half a stroke.
//! 2. **Vertical orientation** — the primary/cross split above, plus
//!    [`SliderGeometry::reverse`]: the reference's `reverse` flag
//!    (`m3e_slider_build.dart:82`) is `!topToBottom` on a vertical slider and
//!    RTL on a horizontal one. Every formula here stays value-oriented and the
//!    variant's axis map applies the one flip at paint, mirroring the
//!    reference's `Transform.flip` around its track layer;
//!    [`SliderGeometry::reverse`] is what the *widget*-space consumers
//!    ([`icon_dock`], [`ValueSpec::value_from_offset`]) take.
//! 3. **Second thumb (range)** — [`SliderGeometry::active_start_fraction`] is
//!    a real input to every formula (the reference's `activeStartFraction`),
//!    fixed at `0.0` for the single-thumb slider. The range variant feeds the
//!    low thumb's fraction there and selects [`SliderTrackKind::Range`], the
//!    third arm of the same three `centered`-branching spots
//!    ([`TrackMetrics::resolve`], [`TrackMetrics::segments`], [`dots`]) — the
//!    same three the reference's `_range` getter branches in.
//!
//! The drag machine ([`Drag`]) and the haptic scheduler ([`HapticScheduler`])
//! are thumb-agnostic: a range slider holds one [`Drag`] per thumb and one
//! shared scheduler.
//!
//! # Clamp discipline
//!
//! `f64::clamp` panics if `min > max` or either bound is NaN — a hard abort
//! under `panic = "abort"`, not a catchable error, so every `.clamp(..)` call
//! in this module tree that clamps against a *computed* (not literal) pair of
//! bounds must be able to show those bounds are always ordered and finite.
//! Two shapes cover every site:
//!
//! - **Bounds are literal `0.0`/`1.0`** ([`ValueSpec::fraction`],
//!   [`ValueSpec::value_from_fraction`], [`ValueSpec::value_from_offset`]'s
//!   own `fraction` clamp, [`TrackMetrics::resolve`]'s two
//!   `active_start_fraction`/`active_end_fraction` clamps,
//!   [`super::range::RangeSliderWidget::amplitude_factor`]'s progress clamp,
//!   [`super::variants::amplitude_factor`]'s ramp clamp) — trivially ordered,
//!   never a hazard.
//! - **Bounds are derived and explicitly ordered with `min()`/`max()`
//!   immediately before the clamp** — [`ValueSpec::snap`] (guarded by an
//!   early-return degenerate check, mirroring [`ValueSpec::fraction`]'s, for
//!   `self.min`/`self.max` themselves), [`icon_dock`] (`min_center`/
//!   `max_center` ordered before [`IconDock::center_at`]'s clamp, covering a
//!   track laid out narrower than its icon), and
//!   [`super::range::RangeSliderWidget::accept`] (the no-cross window per
//!   thumb, ordered right before its clamp rather than trusting `live`/`spec`
//!   to already be ordered — `super::range`'s `build`/`rebuild` normalizes
//!   the adopted pair into bounds too, so this is a total-by-construction
//!   backstop, not the only line of defense).
//!
//! A new clamp against a computed bound must fit one of these two shapes, not
//! introduce a third.

use std::time::Duration;

use frust::{FrameTime, SpringDesc};
use kurbo::{Point, Rect, Size};

use crate::interaction::HapticSignal;

// ---------------------------------------------------------------------------
// Tokens (`res/m3e_slider_tokens.dart`, `styles/m3e_slider_theme.dart`)
// ---------------------------------------------------------------------------
//
// No `Theme` token covers a fixed control dimension like a track thickness or
// a handle width (`ShapeScale` publishes corner radii, not control metrics),
// so these stay named module constants with their upstream source, exactly as
// `crate::switch`'s track/thumb metrics do.

/// Resting handle length along the cross axis, in logical px
/// (`M3ESliderTokens.handleHeight`) — also the control's own layout height
/// (`M3ESliderTheme.height`).
pub(crate) const HANDLE_HEIGHT: f64 = 44.0;
/// Resting handle thickness, in logical px (`M3ESliderTokens.handleWidth`).
pub(crate) const HANDLE_WIDTH: f64 = 4.0;
/// Handle thickness while pressed, in logical px
/// (`M3ESliderTokens.pressedHandleWidth`) — the handle gets *thinner*, not
/// wider, under a press.
pub(crate) const PRESSED_HANDLE_WIDTH: f64 = 2.0;
/// Track thickness, in logical px (`M3ESliderTokens.activeTrackHeight`, equal
/// to `inactiveTrackHeight`).
pub(crate) const TRACK_HEIGHT: f64 = 16.0;
/// Clear space between the handle edge and each track segment, in logical px
/// (`M3ESliderTokens.thumbTrackGapSize`).
pub(crate) const HANDLE_GAP: f64 = 6.0;
/// Corner radius on the track ends that face the handle gap, in logical px
/// (`M3ESliderTokens.trackInsideCornerSize`) — the small half of the
/// asymmetric per-segment radii.
pub(crate) const TRACK_INSIDE_CORNER: f64 = 2.0;
/// Corner radius on the track's outer ends, in logical px
/// (`M3ESliderTokens.trackCornerRadius`). Fixed by the token table, *not*
/// derived from the track thickness, though it is clamped to half of it.
pub(crate) const TRACK_CORNER_RADIUS: f64 = 8.0;
/// Diameter of a track-end stop indicator, in logical px
/// (`M3ESliderTokens.stopIndicatorSize`).
pub(crate) const STOP_INDICATOR_SIZE: f64 = 4.0;
/// Diameter of a discrete tick marker, in logical px
/// (`M3ESliderTokens.tickSize`).
pub(crate) const TICK_SIZE: f64 = 4.0;
/// Clear space between a track end and the outer edge of its stop indicator,
/// in logical px (`M3ESliderTokens.stopIndicatorTrailingSpace`).
pub(crate) const STOP_INDICATOR_TRAILING_SPACE: f64 = 6.0;
/// Clear space between the track edge and the relocating icon's outer edge, in
/// logical px (`M3ESliderTokens.iconEdgeInset`).
pub(crate) const ICON_EDGE_INSET: f64 = 8.0;
/// Default edge length of the relocating end icon, in logical px
/// (`m3e_slider_build.dart:468`'s `widget.iconSize ?? 24`).
pub(crate) const ICON_SIZE_DEFAULT: f64 = 24.0;
/// Clear space between the value indicator's bottom edge and the track, in
/// logical px (`M3ESliderTokens.valueIndicatorActiveBottomSpace`).
pub(crate) const VALUE_INDICATOR_BOTTOM_SPACE: f64 = 12.0;
/// Content opacity applied to every *active* role while disabled
/// (`M3ESliderTokens.disabledActiveTrackOpacity`).
pub(crate) const DISABLED_ACTIVE_OPACITY: f32 = 0.38;
/// Container opacity applied to every *inactive* role while disabled
/// (`M3ESliderTokens.disabledInactiveTrackOpacity`).
pub(crate) const DISABLED_INACTIVE_OPACITY: f32 = 0.12;
/// Edge length of an inset track icon, in logical px
/// (`M3ESliderTrackIcons.size`'s default).
pub(crate) const TRACK_ICON_SIZE: f64 = 16.0;
/// Inset from a segment's own end to the icon it hosts, in logical px
/// (`m3e_slider_track_icons_overlay.dart:60-74`'s literal `4`).
pub(crate) const TRACK_ICON_INSET: f64 = 4.0;
/// Extra segment length a hosted icon needs beyond its own size before it is
/// painted at all (`m3e_slider_track_icons_overlay.dart:38-43`'s `icon + 8`).
pub(crate) const TRACK_ICON_MIN_SLACK: f64 = 8.0;
/// Extra handle gap while the control shows its focus outline, in logical px
/// (`m3e_slider_build.dart:73-75`'s `handleGap + 4`) — the track backs off to
/// clear the ring.
pub(crate) const FOCUS_HANDLE_GAP_EXTRA: f64 = 4.0;
/// Focus-outline stroke width, in logical px
/// (`m3e_slider_thumb.dart:53`'s `_focusStroke`).
pub(crate) const FOCUS_STROKE: f64 = 2.0;
/// **Total** growth (both edges together) of the focus outline over the thumb
/// box, in logical px (`m3e_slider_thumb.dart:54`'s `_focusInflate`, applied
/// there as `w + _focusInflate`) — so each edge grows by half of it. Reused
/// for the dragged/pressed halo; see [`focus_ring_rect`].
pub(crate) const FOCUS_INFLATE: f64 = 6.0;
/// Duration of the pressed-thumb width compression
/// (`m3e_slider_thumb.dart:77`'s `AnimatedContainer(duration: 100ms)`).
pub(crate) const PRESS_MORPH_DURATION: Duration = Duration::from_millis(100);
/// Minimum spacing between two continuous-drag haptic ticks
/// (`m3e_sliders.dart:613-621`'s `elapsedMilliseconds >= 60`).
pub(crate) const HAPTIC_MIN_INTERVAL: Duration = Duration::from_millis(60);
/// The relocating icon's dock spring: motor's
/// `MaterialSpringMotion.expressiveSpatialFast`
/// (`tmp/motor-1.1.0/lib/src/motion.dart:595-601`, stiffness `800`, damping
/// ratio `0.6`), the spec `m3e_sliders.dart:480-483` builds `_dockController`
/// on. Motor's `damping` is a damping *ratio*, the same ζ semantics
/// [`SpringDesc::damping_ratio`] uses, so it carries over with no conversion.
pub(crate) const DOCK_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 800.0,
    damping_ratio: 0.6,
};
/// Extra clearance beyond the two half-widths at which the relocating icon
/// counts as docked (`m3e_slider_build.dart:481`'s `+ 8`).
pub(crate) const ICON_DOCK_SLACK: f64 = 8.0;
/// Extra clearance beyond the two half-widths the docked icon parks at
/// (`m3e_slider_build.dart:486`'s `+ 12`).
pub(crate) const ICON_DOCK_OFFSET_SLACK: f64 = 12.0;
/// Offset from the thumb back to the value indicator's leading edge, in
/// logical px (`m3e_slider_build.dart:441`'s `thumbPrimary - 24`).
pub(crate) const VALUE_INDICATOR_LEAD: f64 = 24.0;
/// Height the value indicator is lifted by above its bottom space, in logical
/// px (`m3e_slider_build.dart:444`'s `-valueIndicatorBottomSpace - 24`).
pub(crate) const VALUE_INDICATOR_LIFT: f64 = 24.0;
/// Value-indicator container corner radius, in logical px
/// (`m3e_slider_value_indicator.dart:34`).
pub(crate) const VALUE_INDICATOR_RADIUS: f64 = 4.0;
/// Value-indicator horizontal padding, in logical px
/// (`m3e_slider_value_indicator.dart:37`).
pub(crate) const VALUE_INDICATOR_PAD_X: f64 = 8.0;
/// Value-indicator vertical padding, in logical px
/// (`m3e_slider_value_indicator.dart:37`).
pub(crate) const VALUE_INDICATOR_PAD_Y: f64 = 4.0;
/// Track length used when the incoming constraints are horizontally unbounded.
/// The reference has no equivalent (Flutter's `LayoutBuilder` would hand it an
/// infinite width and throw); this mirrors the baseline `frust::Slider`'s own
/// fallback so an unconstrained slider still measures something paintable.
pub(crate) const DEFAULT_EXTENT: f64 = 200.0;

/// Which track geometry a slider paints (`M3ESliderTrackKind`, with
/// `M3ESliderPaintMode.range` folded in as its third arm).
///
/// Upstream splits this across two enums — a `trackKind` (standard/centered)
/// and a paint `mode` (single/range) — but only three of the four combinations
/// exist: a range track is always the standard kind
/// (`m3e_range_slider_track.dart:100-101` hard-codes it, and Compose has no
/// centered range slider). One enum keeps the impossible fourth
/// unrepresentable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SliderTrackKind {
    /// Active track runs from the start edge to the thumb.
    #[default]
    Standard,
    /// Active track grows from the track's midpoint toward the thumb.
    Centered,
    /// Active track spans between two thumbs
    /// ([`super::range`]'s `M3ERangeSlider`).
    Range,
}

/// Axis-relative resting edge for the relocating end icon
/// (`M3ESliderIconPosition`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SliderIconPosition {
    /// The track's leading edge.
    Start,
    /// The track's trailing edge.
    #[default]
    End,
}

/// The value model: bounds plus an optional discrete step count
/// (`M3ESlider.min`/`max`/`divisions`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ValueSpec {
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) divisions: Option<u32>,
}

impl Default for ValueSpec {
    /// The reference's own defaults: `0..=1`, continuous
    /// (`m3e_sliders.dart:62-65`).
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            divisions: None,
        }
    }
}

impl ValueSpec {
    /// `0..=1` position of `value` within the bounds
    /// (`M3ESliderMath.fraction`). A degenerate (`max <= min`) range reads
    /// `0`, as upstream.
    pub(crate) fn fraction(&self, value: f64) -> f64 {
        if self.max <= self.min {
            return 0.0;
        }
        ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    /// A `0..=1` fraction mapped back into the bounds
    /// (`M3ESliderMath.valueFromFraction`).
    pub(crate) fn value_from_fraction(&self, fraction: f64) -> f64 {
        self.min + fraction.clamp(0.0, 1.0) * (self.max - self.min)
    }

    /// `value` clamped into the bounds and snapped to the nearest division
    /// step, or merely clamped when continuous (`M3ESliderMath.snap`).
    ///
    /// Mirrors [`Self::fraction`]'s degenerate guard rather than reaching
    /// `value.clamp(self.min, self.max)` unconditionally: `f64::clamp` panics
    /// when `min > max` or either bound is NaN, and unlike `fraction` (which
    /// only ever clamps against the literal `0.0..=1.0`), this method clamps
    /// against the bounds themselves — an inverted `.range(20.0, 10.0)`
    /// builder call or a NaN bound would otherwise crash a shipped app on the
    /// very next drag/press (`snap` sits on both the live drag path and
    /// `accept_value`). The degenerate case pins at `min`, the documented
    /// contract (see the [module docs](self)'s Clamp Discipline section).
    pub(crate) fn snap(&self, value: f64) -> f64 {
        if !(self.min.is_finite() && self.max.is_finite() && self.max > self.min) {
            return self.min;
        }
        let clamped = value.clamp(self.min, self.max);
        let Some(divisions) = self.divisions.filter(|d| *d > 0) else {
            return clamped;
        };
        let step = (self.max - self.min) / f64::from(divisions);
        self.min + ((clamped - self.min) / step).round() * step
    }

    /// The tick fractions a discrete slider marks — `divisions + 1` marks,
    /// endpoints inclusive (`M3ESliderMath.tickFractions`). Empty when
    /// continuous.
    pub(crate) fn tick_fractions(&self) -> Vec<f64> {
        let Some(divisions) = self.divisions.filter(|d| *d > 0) else {
            return Vec::new();
        };
        (0..=divisions)
            .map(|i| f64::from(i) / f64::from(divisions))
            .collect()
    }

    /// A primary-axis pointer offset mapped to a (snapped) value
    /// (`M3ESliderMath.valueFromOffset`). `reverse` mirrors the track, the
    /// flag a vertical or RTL variant sets.
    pub(crate) fn value_from_offset(&self, primary: f64, extent: f64, reverse: bool) -> f64 {
        if extent <= 0.0 {
            return self.min;
        }
        let mut fraction = (primary / extent).clamp(0.0, 1.0);
        if reverse {
            fraction = 1.0 - fraction;
        }
        self.snap(self.value_from_fraction(fraction))
    }
}

/// The reference's `_setValue` admission test (`m3e_sliders.dart:588-611`):
/// clamp, snap, and drop a change that lands on the value already held.
/// Returns the value to report, or `None` when there is nothing to report.
pub(crate) fn accept_value(spec: &ValueSpec, current: f64, raw: f64) -> Option<f64> {
    let next = spec.snap(raw);
    if next == current { None } else { Some(next) }
}

/// The resolved per-pass slider geometry inputs — everything
/// [`TrackMetrics::resolve`] needs, in primary-axis units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SliderGeometry {
    /// Track length along the primary axis.
    pub(crate) extent: f64,
    /// Where the active span begins, `0..=1`. Always `0.0` for a single-thumb
    /// slider; the range variant's low-thumb seam (see the [module docs](self)).
    pub(crate) active_start_fraction: f64,
    /// Where the active span ends (the thumb's own fraction), `0..=1`.
    pub(crate) active_end_fraction: f64,
    /// Whether the primary axis runs backwards (vertical/RTL seam).
    pub(crate) reverse: bool,
    pub(crate) kind: SliderTrackKind,
    /// The handle thickness the *gap* is measured from — the instantaneous
    /// pressed/resting value, never the animated one (see
    /// [`TrackMetrics::resolve`]).
    pub(crate) handle_thickness: f64,
    pub(crate) handle_gap: f64,
    pub(crate) track_thickness: f64,
    pub(crate) corner_radius: f64,
}

/// Intermediate track geometry: `M3ESliderTrackPaintMetrics` and
/// `M3ESliderDotGeometry` unified.
///
/// Upstream carries the same nine formulas twice, once per file
/// (`m3e_slider_track_paint_metrics.dart:46-98` and
/// `m3e_slider_dot_geometry.dart:63-129` are line-for-line identical apart
/// from the three extra fields the dot side keeps). They are one struct here:
/// two copies of a formula are two chances to diverge, and both consumers
/// ([`TrackMetrics::segments`] and [`dots`]) live in this module.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrackMetrics {
    pub(crate) slider_start: f64,
    pub(crate) slider_end: f64,
    pub(crate) centered: bool,
    pub(crate) range: bool,
    pub(crate) corner: f64,
    pub(crate) inside_corner: f64,
    pub(crate) start_gap: f64,
    pub(crate) end_gap: f64,
    pub(crate) value_start: f64,
    pub(crate) value_end: f64,
    pub(crate) center_axis: f64,
    pub(crate) adjusted_value_start: f64,
    pub(crate) adjusted_value_end: f64,
    pub(crate) active_start: f64,
    pub(crate) active_end: f64,
}

/// Which colour role a [`TrackSegment`] takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SegmentRole {
    Active,
    Inactive,
}

/// One painted track span, in primary-axis units, with the asymmetric corner
/// radii its two ends take (`start` is the low-coordinate end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrackSegment {
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) start_corner: f64,
    pub(crate) end_corner: f64,
    pub(crate) role: SegmentRole,
}

impl TrackMetrics {
    /// Resolve the metrics for one paint pass, or `None` for a degenerate
    /// (zero-length) track — the reference's own `if (span <= 0) return`
    /// (`m3e_slider_track_painter.dart:133-135`).
    ///
    /// The gap is measured from `geometry.handle_thickness`, which the
    /// reference resolves *instantly* on press (`m3e_slider_build.dart:83-87`)
    /// even though the handle's own width animates over 100ms — the gap snaps
    /// while the handle glides. Transcribed as-is rather than "fixed": it is
    /// the reference's own shape.
    pub(crate) fn resolve(geometry: &SliderGeometry) -> Option<Self> {
        let slider_start = 0.0;
        let slider_end = geometry.extent;
        let span = slider_end - slider_start;
        if span <= 0.0 {
            return None;
        }
        let centered = geometry.kind == SliderTrackKind::Centered;
        let range = geometry.kind == SliderTrackKind::Range;
        let corner = geometry.corner_radius.min(geometry.track_thickness / 2.0);
        let start_gap = if centered || range {
            geometry.handle_thickness / 2.0 + geometry.handle_gap
        } else {
            0.0
        };
        let end_gap = geometry.handle_thickness / 2.0 + geometry.handle_gap;

        let value_start = slider_start + span * geometry.active_start_fraction.clamp(0.0, 1.0);
        let value_end = slider_start + span * geometry.active_end_fraction.clamp(0.0, 1.0);
        let center_axis = (slider_start + slider_end) / 2.0;

        let adjusted_value_end = if centered {
            value_end.min(center_axis)
        } else {
            value_start
        };
        let adjusted_value_start = if centered {
            value_end.max(center_axis)
        } else {
            value_end
        };
        let active_start = if centered {
            adjusted_value_end
                + if adjusted_value_end < center_axis {
                    start_gap
                } else {
                    0.0
                }
        } else if range {
            // The low thumb carves its own gap out of the active span, the
            // mirror of what the high thumb already does at `active_end`.
            value_start + start_gap
        } else {
            slider_start
        };
        let active_end = if centered {
            adjusted_value_start
                - if adjusted_value_start > center_axis {
                    end_gap
                } else {
                    0.0
                }
        } else {
            value_end - end_gap
        };

        Some(Self {
            slider_start,
            slider_end,
            centered,
            range,
            corner,
            inside_corner: TRACK_INSIDE_CORNER,
            start_gap,
            end_gap,
            value_start,
            value_end,
            center_axis,
            adjusted_value_start,
            adjusted_value_end,
            active_start,
            active_end,
        })
    }

    /// The track's painted spans, in the reference's own paint order: inactive
    /// leading (centered only), inactive trailing, then active
    /// (`m3e_slider_track_painter.dart:151-153`).
    ///
    /// The `rtl` flag mirrors which end of a span takes the outer radius — the
    /// reference's `_rtl` getter; a `reverse`d (vertical/RTL) variant passes
    /// it through from [`SliderGeometry::reverse`].
    pub(crate) fn segments(&self, rtl: bool) -> Vec<TrackSegment> {
        let mut out = Vec::with_capacity(3);
        let (outer, inner) = (self.corner, self.inside_corner);

        // Inactive leading (`_paintInactiveLeading`): centered and range
        // tracks only — a standard track's active span starts at the very
        // edge, so there is nothing leading it.
        if (self.centered || self.range)
            && self.adjusted_value_end > self.slider_start + self.start_gap + outer
        {
            let start = self.slider_start;
            let end = self.adjusted_value_end - self.start_gap;
            if end > start {
                out.push(TrackSegment {
                    start,
                    end,
                    start_corner: if rtl { inner } else { outer },
                    end_corner: if rtl { outer } else { inner },
                    role: SegmentRole::Inactive,
                });
            }
        }

        // Inactive trailing (`_paintInactiveTrailing`).
        if self.adjusted_value_start < self.slider_end - self.end_gap - outer {
            let start = self.adjusted_value_start + self.end_gap;
            let end = self.slider_end;
            if end > start {
                out.push(TrackSegment {
                    start,
                    end,
                    start_corner: if rtl { outer } else { inner },
                    end_corner: if rtl { inner } else { outer },
                    role: SegmentRole::Inactive,
                });
            }
        }

        // Active (`_paintActive`).
        let start_corner = if rtl || self.centered || self.range {
            inner
        } else {
            outer
        };
        let end_corner = if rtl && !self.centered && !self.range {
            outer
        } else {
            inner
        };
        if self.active_end - self.active_start > start_corner {
            out.push(TrackSegment {
                start: self.active_start,
                end: self.active_end,
                start_corner,
                end_corner,
                role: SegmentRole::Active,
            });
        }
        out
    }
}

/// One stop/tick marker along the track (`M3ESliderDotPlacement`), in
/// primary-axis units. `active` selects the tick colour role and is always
/// `false` for a track-end stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DotPlacement {
    pub(crate) primary: f64,
    pub(crate) size: f64,
    pub(crate) active: bool,
}

/// The dot sizing/inset inputs (`M3ESliderDotLayout.resolve`'s size trio).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DotSpec {
    pub(crate) stop_size: f64,
    pub(crate) tick_size: f64,
    /// Clear space between a track end and the outer edge of its marker.
    pub(crate) edge_inset: f64,
}

/// Track-end stop indicators plus discrete ticks
/// (`M3ESliderDotLayout.resolve`): a stop is dropped where the active span or
/// a handle gap already covers it, and a tick is dropped inside a gap.
pub(crate) fn dots(metrics: &TrackMetrics, ticks: &[f64], spec: &DotSpec) -> Vec<DotPlacement> {
    let mut out = Vec::new();
    // `edge_inset` is clear space to the marker's *outer* edge, so a centre
    // is inset by the spacing plus half the marker.
    let stop_radius = spec.stop_size / 2.0;
    let stop_start = metrics.slider_start + spec.edge_inset + stop_radius;
    let stop_end = metrics.slider_end - spec.edge_inset - stop_radius;
    if stop_end <= stop_start {
        return out;
    }
    for primary in [stop_start, stop_end] {
        if !on_active_or_gap(primary, metrics) {
            out.push(DotPlacement {
                primary,
                size: spec.stop_size,
                active: false,
            });
        }
    }

    if ticks.is_empty() {
        return out;
    }
    // Ticks share the stops' padded span.
    let (tick_start, tick_end) = (stop_start, stop_end);
    let center_gap_lo = metrics.center_axis - metrics.end_gap;
    let center_gap_hi = metrics.center_axis + metrics.end_gap;
    let start_gap_lo = metrics.value_start - metrics.start_gap;
    let start_gap_hi = metrics.value_start + metrics.start_gap;
    let end_gap_lo = metrics.value_end - metrics.end_gap;
    let end_gap_hi = metrics.value_end + metrics.end_gap;
    for (i, fraction) in ticks.iter().enumerate() {
        // The two ends belong to the stop indicators.
        if i == 0 || i == ticks.len() - 1 {
            continue;
        }
        let center = tick_start + (tick_end - tick_start) * fraction;
        if metrics.centered && center >= center_gap_lo && center <= center_gap_hi {
            continue;
        }
        // A range track's low thumb owns a gap of its own.
        if metrics.range && center >= start_gap_lo && center <= start_gap_hi {
            continue;
        }
        if center >= end_gap_lo && center <= end_gap_hi {
            continue;
        }
        out.push(DotPlacement {
            primary: center,
            size: spec.tick_size,
            active: center >= metrics.active_start && center <= metrics.active_end,
        });
    }
    out
}

/// Whether `primary` falls on the active span or inside a handle gap —
/// `M3ESliderDotLayout._onActiveOrGap`.
fn on_active_or_gap(primary: f64, metrics: &TrackMetrics) -> bool {
    if primary >= metrics.active_start && primary <= metrics.active_end {
        return true;
    }
    if metrics.range
        && primary >= metrics.value_start - metrics.start_gap
        && primary <= metrics.value_start + metrics.start_gap
    {
        return true;
    }
    if primary >= metrics.value_end - metrics.end_gap
        && primary <= metrics.value_end + metrics.end_gap
    {
        return true;
    }
    metrics.centered
        && metrics.start_gap > 0.0
        && primary >= metrics.value_end - metrics.start_gap
        && primary <= metrics.value_end + metrics.start_gap
}

/// The thumb's box, given its centre along the primary axis and the cross-axis
/// centreline. `thickness` is the animated (pressed-morph) width, `length` the
/// cross-axis extent (`m3e_slider_thumb.dart:72-74`).
pub(crate) fn thumb_rect(primary: f64, cross_center: f64, thickness: f64, length: f64) -> Rect {
    Rect::from_center_size(
        Point::new(primary, cross_center),
        Size::new(thickness, length),
    )
}

/// A thumb box's corner radius — half its longer side
/// (`m3e_slider_thumb.dart:74`).
pub(crate) fn thumb_radius(thumb: Rect) -> f64 {
    thumb.width().max(thumb.height()) / 2.0
}

/// The concentric focus outline (and, in this port, the dragged/pressed halo)
/// around a thumb box: [`FOCUS_INFLATE`] of total growth, half on each edge
/// (`m3e_slider_thumb.dart:95-104`).
pub(crate) fn focus_ring_rect(thumb: Rect) -> Rect {
    thumb.inflate(FOCUS_INFLATE / 2.0, FOCUS_INFLATE / 2.0)
}

/// The value indicator's top-left corner, in primary/cross-local units, for a
/// thumb at `primary` (`m3e_slider_build.dart:439-449`). The indicator sits
/// *above* the control's own box — a negative cross coordinate — the same way
/// the reference's `Clip.none` stack lets it overflow.
pub(crate) fn value_indicator_origin(primary: f64) -> Point {
    Point::new(
        primary - VALUE_INDICATOR_LEAD,
        -VALUE_INDICATOR_BOTTOM_SPACE - VALUE_INDICATOR_LIFT,
    )
}

/// The resolved relocating-icon dock geometry
/// (`m3e_slider_build.dart:457-523`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IconDock {
    /// Where the icon rests when the thumb is far away.
    pub(crate) resting_center: f64,
    /// Where it parks once the thumb is close enough to cover it.
    pub(crate) docked_target: f64,
    /// Whether the thumb is close enough to dock the icon right now.
    pub(crate) docked: bool,
    min_center: f64,
    max_center: f64,
}

/// Inputs for [`icon_dock`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IconDockInput {
    pub(crate) extent: f64,
    pub(crate) thumb_primary: f64,
    pub(crate) icon_size: f64,
    pub(crate) edge_inset: f64,
    pub(crate) handle_thickness: f64,
    pub(crate) position: SliderIconPosition,
    pub(crate) reverse: bool,
}

/// Resolve where the relocating end icon rests, where it docks, and whether it
/// is docked at this thumb position.
pub(crate) fn icon_dock(input: &IconDockInput) -> IconDock {
    let icon_half = input.icon_size / 2.0;
    let thumb_half = input.handle_thickness / 2.0;
    let near_end = (input.position == SliderIconPosition::End) != input.reverse;
    let resting_center = if near_end {
        input.extent - input.edge_inset - icon_half
    } else {
        input.edge_inset + icon_half
    };
    let dock_distance_limit = thumb_half + icon_half + ICON_DOCK_SLACK;
    let docked = (input.thumb_primary - resting_center).abs() <= dock_distance_limit;
    let dock_offset = thumb_half + icon_half + ICON_DOCK_OFFSET_SLACK;
    let docked_target = if input.reverse {
        input.thumb_primary + dock_offset
    } else {
        input.thumb_primary - dock_offset
    };
    // `icon_half` and `extent - icon_half` invert whenever the track is laid
    // out narrower than the icon (`extent < icon_size`, reachable since
    // `layout` never lower-bounds itself by `icon_size`) — `min()`/`max()`
    // order the pair so `IconDock::center_at`'s `f64::clamp` never sees an
    // inverted bound (see the [module docs](self)'s Clamp Discipline
    // section). The icon just centers on the track in that squeeze instead
    // of panicking.
    IconDock {
        resting_center,
        docked_target,
        docked,
        min_center: icon_half.min(input.extent - icon_half),
        max_center: icon_half.max(input.extent - icon_half),
    }
}

impl IconDock {
    /// The icon's centre at dock progress `t` (`0` resting, `1` docked),
    /// clamped inside the track (`m3e_slider_build.dart:493-497`).
    pub(crate) fn center_at(&self, t: f64) -> f64 {
        let raw = self.resting_center + (self.docked_target - self.resting_center) * t;
        raw.clamp(self.min_center, self.max_center)
    }

    /// Whether an icon at `center` sits over the active span, which selects
    /// its ink (`m3e_slider_build.dart:499-501`).
    pub(crate) fn over_active(&self, center: f64, thumb_primary: f64, reverse: bool) -> bool {
        if reverse {
            center >= thumb_primary
        } else {
            center <= thumb_primary
        }
    }
}

/// Which of the four inset track-icon slots a placement belongs to
/// (`M3ESliderTrackIcons`' four fields). Crate-internal: an app names the
/// slots by [`super::SliderTrackIcons`]' own fields instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackIconSlot {
    /// Leading end of the active segment.
    ActiveStart,
    /// Trailing end of the active segment.
    ActiveEnd,
    /// Leading end of the inactive segment.
    InactiveStart,
    /// Trailing end of the inactive segment.
    InactiveEnd,
}

impl TrackIconSlot {
    /// This slot's index into a caller's four-slot array, in
    /// `M3ESliderTrackIcons` field order.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::ActiveStart => 0,
            Self::ActiveEnd => 1,
            Self::InactiveStart => 2,
            Self::InactiveEnd => 3,
        }
    }
}

/// A placed inset track icon: the slot it fills and its box's leading edge
/// along the primary axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrackIconPlacement {
    pub(crate) slot: TrackIconSlot,
    pub(crate) primary: f64,
    pub(crate) active: bool,
}

/// Where each inset track icon goes, dropping any whose segment is too short
/// to host it (`m3e_slider_track_icons_overlay.dart:18-79`). `present` marks
/// which of the four slots the caller actually holds an icon for, indexed by
/// [`TrackIconSlot::index`].
///
/// A centered track offers only the two *active* slots upstream, so the other
/// two are never candidates there whatever `present` says.
pub(crate) fn track_icon_placements(
    extent: f64,
    fraction: f64,
    kind: SliderTrackKind,
    icon_size: f64,
    present: [bool; 4],
) -> Vec<TrackIconPlacement> {
    let active_len = fraction * extent;
    let inactive_len = (1.0 - fraction) * extent;
    let candidates: Vec<(TrackIconSlot, f64, bool)> = match kind {
        // Upstream reserves `M3ERangeSlider.trackIcons` "for parity" and never
        // renders it (`m3e_range_slider_build.dart:149-211` wraps no
        // `_TrackIconsOverlay`), so a range track hosts none either.
        SliderTrackKind::Range => Vec::new(),
        SliderTrackKind::Centered => vec![
            (
                TrackIconSlot::ActiveStart,
                extent / 2.0 - icon_size - TRACK_ICON_INSET,
                true,
            ),
            (
                TrackIconSlot::ActiveEnd,
                active_len + TRACK_ICON_INSET,
                true,
            ),
        ],
        SliderTrackKind::Standard => vec![
            (TrackIconSlot::ActiveStart, TRACK_ICON_INSET, true),
            (
                TrackIconSlot::ActiveEnd,
                active_len - icon_size - TRACK_ICON_INSET,
                true,
            ),
            (
                TrackIconSlot::InactiveStart,
                active_len + TRACK_ICON_INSET,
                false,
            ),
            (
                TrackIconSlot::InactiveEnd,
                extent - icon_size - TRACK_ICON_INSET,
                false,
            ),
        ],
    };

    candidates
        .into_iter()
        .filter(|(slot, _, _)| present[slot.index()])
        .filter(|(_, _, active)| {
            let len = if *active { active_len } else { inactive_len };
            len >= icon_size + TRACK_ICON_MIN_SLACK
        })
        .map(|(slot, primary, active)| TrackIconPlacement {
            slot,
            primary,
            active,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Drag state machine
// ---------------------------------------------------------------------------

/// The per-event inputs the drag machine resolves a value against.
///
/// [`ValueSpec`] is carried by value rather than borrowed: the widget builds
/// this from its own fields in the same expression that mutates its [`Drag`],
/// and a borrow of one field would lock the whole struct for that call.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DragCtx {
    pub(crate) spec: ValueSpec,
    /// The value the machine deduplicates against — the app-confirmed value,
    /// or the last one already reported this drag (see [`Drag::moved`]).
    pub(crate) current: f64,
    pub(crate) extent: f64,
    pub(crate) reverse: bool,
}

/// What the widget must do after feeding one pointer phase to the machine.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DragOutcome {
    /// Whether the event was consumed at all.
    pub(crate) handled: bool,
    /// Whether to open a pointer capture.
    pub(crate) capture: bool,
    /// A value to report through `on_changed`, already clamped and snapped.
    pub(crate) value: Option<f64>,
    /// Whether the interaction settled and `on_change_end` is owed.
    pub(crate) end: bool,
}

/// The slider's pointer state machine: `Down` arms and jumps, `Move` tracks,
/// `Up` settles, `Cancel` disarms (`m3e_slider_build.dart:362-405` plus
/// `m3e_sliders.dart:667-673`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Drag {
    /// The pressed *visual* state — thumb compression, value indicator.
    pub(crate) pressed: bool,
    /// Armed by a `Down` alongside the pointer capture; gates `Move`.
    pub(crate) dragging: bool,
}

impl Drag {
    /// A `Down` inside the control: press, arm, capture, and immediately jump
    /// to the tapped value (`_onTapDown`).
    pub(crate) fn down(&mut self, primary: f64, ctx: &DragCtx) -> DragOutcome {
        self.pressed = true;
        self.dragging = true;
        DragOutcome {
            handled: true,
            capture: true,
            value: self.resolve(primary, ctx),
            end: false,
        }
    }

    /// A captured `Move`: track the pointer (`_horizontalDragUpdate`). An
    /// uncaptured (hover) move is ignored outright.
    pub(crate) fn moved(&mut self, primary: f64, ctx: &DragCtx) -> DragOutcome {
        if !self.dragging {
            return DragOutcome::default();
        }
        DragOutcome {
            handled: true,
            capture: false,
            value: self.resolve(primary, ctx),
            end: false,
        }
    }

    /// An `Up`: settle and owe `on_change_end` (`_endInteraction`).
    pub(crate) fn up(&mut self) -> DragOutcome {
        if !self.dragging {
            return DragOutcome::default();
        }
        self.pressed = false;
        self.dragging = false;
        DragOutcome {
            handled: true,
            capture: false,
            value: None,
            end: true,
        }
    }

    /// A `Cancel` (platform gesture steal): disarm only.
    ///
    /// **A deliberate divergence.** The reference routes its drag-cancel to
    /// the same `_endInteraction` an `Up` takes, firing `onChangeEnd`
    /// (`m3e_slider_build.dart:344`/`357`). This framework forbids it: a
    /// `Cancel` arm may never reach app state, because a synthesized cancel
    /// can be delivered over a throwaway `()` state
    /// (`docs/CODE_STANDARDS.md`'s Interaction Semantics). So a cancel clears
    /// the flags and reports nothing.
    pub(crate) fn cancel(&mut self) -> DragOutcome {
        if !self.dragging {
            return DragOutcome::default();
        }
        self.pressed = false;
        self.dragging = false;
        DragOutcome {
            handled: true,
            capture: false,
            value: None,
            end: false,
        }
    }

    fn resolve(&self, primary: f64, ctx: &DragCtx) -> Option<f64> {
        let raw = ctx.spec.value_from_offset(primary, ctx.extent, ctx.reverse);
        accept_value(&ctx.spec, ctx.current, raw)
    }
}

// ---------------------------------------------------------------------------
// Haptics
// ---------------------------------------------------------------------------

/// How soon a queued haptic may fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HapticCadence {
    /// Every accepted change fires — a discrete slider's per-division tick
    /// (`m3e_sliders.dart:603-606`).
    Immediate,
    /// At most one tick per [`HAPTIC_MIN_INTERVAL`] — a continuous drag
    /// (`_maybeContinuousHaptic`).
    Throttled,
}

/// The slider's haptic pacing, driven by the **frame clock**.
///
/// The reference paces continuous-drag ticks off a `Stopwatch` read inside its
/// gesture callback (`m3e_sliders.dart:450`, `613-621`). Widget code in this
/// framework may not read a wall clock at all — time enters only as the
/// `FrameTime` a shell hands `PaintCtx` (`docs/WIDGETS_CODE_STANDARDS.md`,
/// `docs/REVIEW_FOCUS.md`'s theme-resolution hot spot) — so a change *queues*
/// a signal from the event pass and the next paint decides whether it fires.
/// Two consequences, both accepted:
///
/// - A tick lands on the next frame rather than inside the event, at most one
///   frame (~8–16ms) later than upstream. Every value change requests a
///   redraw, so that frame is always imminent.
/// - The elapsed test measures queue-to-*frame*, so it is marginally more
///   permissive than upstream's queue-to-queue test. It can never fire faster
///   than one tick per [`HAPTIC_MIN_INTERVAL`], which is the property that
///   matters.
///
/// A throttled signal that arrives too early is **dropped**, not deferred —
/// upstream's own shape (`_maybeContinuousHaptic` simply does nothing when the
/// stopwatch is short).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct HapticScheduler {
    last_fire: Option<FrameTime>,
    pending: Option<(HapticSignal, HapticCadence)>,
}

impl HapticScheduler {
    /// Queue `signal` to fire on the next paint, subject to `cadence`. A
    /// second queue before that paint replaces the first: one frame emits at
    /// most one tick.
    pub(crate) fn queue(&mut self, signal: HapticSignal, cadence: HapticCadence) {
        self.pending = Some((signal, cadence));
    }

    /// Whether a signal is waiting for the next paint to resolve it — a
    /// coverage seam (the widget itself only ever drains the queue), so it is
    /// compiled out of a real build.
    #[cfg(test)]
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Resolve any queued signal against the frame clock, returning the one to
    /// fire.
    pub(crate) fn take(&mut self, now: FrameTime) -> Option<HapticSignal> {
        let (signal, cadence) = self.pending.take()?;
        let eligible = match cadence {
            HapticCadence::Immediate => true,
            HapticCadence::Throttled => match self.last_fire {
                None => true,
                Some(last) => now.saturating_sub(last) >= HAPTIC_MIN_INTERVAL,
            },
        };
        if !eligible {
            return None;
        }
        self.last_fire = Some(now);
        Some(signal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_UNIT: ValueSpec = ValueSpec {
        min: 0.0,
        max: 1.0,
        divisions: None,
    };

    fn spec(min: f64, max: f64, divisions: Option<u32>) -> ValueSpec {
        ValueSpec {
            min,
            max,
            divisions,
        }
    }

    fn geometry(fraction: f64, kind: SliderTrackKind) -> SliderGeometry {
        SliderGeometry {
            extent: 200.0,
            active_start_fraction: 0.0,
            active_end_fraction: fraction,
            reverse: false,
            kind,
            handle_thickness: HANDLE_WIDTH,
            handle_gap: HANDLE_GAP,
            track_thickness: TRACK_HEIGHT,
            corner_radius: TRACK_CORNER_RADIUS,
        }
    }

    // -- value model --------------------------------------------------------

    #[test]
    fn fraction_maps_bounds_and_clamps_outside_them() {
        let s = spec(10.0, 20.0, None);
        assert_eq!(s.fraction(10.0), 0.0, "min edge");
        assert_eq!(s.fraction(15.0), 0.5);
        assert_eq!(s.fraction(20.0), 1.0, "max edge");
        assert_eq!(s.fraction(-5.0), 0.0, "below min clamps");
        assert_eq!(s.fraction(999.0), 1.0, "above max clamps");
        // A degenerate range reads 0 rather than dividing by zero.
        assert_eq!(spec(5.0, 5.0, None).fraction(5.0), 0.0);
    }

    #[test]
    fn value_from_fraction_round_trips_through_fraction() {
        let s = spec(-40.0, 60.0, None);
        for f in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let v = s.value_from_fraction(f);
            assert!((s.fraction(v) - f).abs() < 1e-12, "fraction {f} round trip");
        }
        assert_eq!(s.value_from_fraction(0.0), -40.0);
        assert_eq!(s.value_from_fraction(1.0), 60.0);
    }

    #[test]
    fn discrete_snapping_lands_on_steps_including_both_edges() {
        let s = spec(0.0, 10.0, Some(5)); // steps of 2
        assert_eq!(s.snap(0.0), 0.0, "min edge stays put");
        assert_eq!(s.snap(10.0), 10.0, "max edge stays put");
        assert_eq!(s.snap(0.9), 0.0, "rounds down");
        assert_eq!(s.snap(1.1), 2.0, "rounds up");
        assert_eq!(s.snap(-3.0), 0.0, "clamps below min before snapping");
        assert_eq!(s.snap(99.0), 10.0, "clamps above max before snapping");
        // Continuous: clamp only, no snap.
        assert_eq!(spec(0.0, 10.0, None).snap(3.3), 3.3);
        // Zero divisions is treated as continuous, like upstream's `<= 0`.
        assert_eq!(spec(0.0, 10.0, Some(0)).snap(3.3), 3.3);
    }

    #[test]
    fn snap_degrades_to_the_minimum_instead_of_panicking_on_a_bad_bound() {
        // `.range(20.0, 10.0)` (`max < min`) — `f64::clamp` would panic on
        // `min > max`; this is the exact repro from the review finding.
        let inverted = spec(20.0, 10.0, None);
        assert_eq!(inverted.snap(15.0), 20.0, "pins at min, never panics");
        assert_eq!(inverted.snap(-5.0), 20.0);

        // Equal bounds: not itself panic-prone for `clamp`, but shares
        // `fraction`'s degenerate treatment (and sidesteps a `0/0` division
        // step when discrete).
        assert_eq!(spec(5.0, 5.0, Some(4)).snap(5.0), 5.0);

        // NaN either bound — `f64::clamp` panics on a NaN min or max too.
        assert_eq!(spec(0.0, f64::NAN, None).snap(3.0), 0.0, "NaN max");
        assert!(
            spec(f64::NAN, 10.0, None).snap(3.0).is_nan(),
            "NaN min propagates rather than panicking"
        );

        // A well-formed range is unaffected by the guard.
        assert_eq!(spec(0.0, 10.0, None).snap(4.0), 4.0);
    }

    #[test]
    fn tick_fractions_are_divisions_plus_one_marks_endpoints_included() {
        assert_eq!(
            spec(0.0, 1.0, Some(4)).tick_fractions(),
            vec![0.0, 0.25, 0.5, 0.75, 1.0]
        );
        assert!(SPEC_UNIT.tick_fractions().is_empty(), "continuous has none");
        assert!(spec(0.0, 1.0, Some(0)).tick_fractions().is_empty());
    }

    #[test]
    fn value_from_offset_maps_position_snaps_and_honours_reverse() {
        let s = spec(0.0, 100.0, None);
        assert_eq!(s.value_from_offset(0.0, 200.0, false), 0.0);
        assert_eq!(s.value_from_offset(100.0, 200.0, false), 50.0);
        assert_eq!(s.value_from_offset(200.0, 200.0, false), 100.0);
        assert_eq!(s.value_from_offset(-40.0, 200.0, false), 0.0, "clamps");
        assert_eq!(s.value_from_offset(9000.0, 200.0, false), 100.0, "clamps");
        assert_eq!(s.value_from_offset(10.0, 0.0, false), 0.0, "zero extent");
        // The vertical/RTL seam mirrors the axis.
        assert_eq!(s.value_from_offset(0.0, 200.0, true), 100.0);
        assert_eq!(s.value_from_offset(150.0, 200.0, true), 25.0);
        // Discrete offsets land on a step.
        let d = spec(0.0, 10.0, Some(10));
        assert_eq!(d.value_from_offset(37.0, 200.0, false), 2.0);
    }

    #[test]
    fn accept_value_drops_a_change_that_lands_on_the_held_value() {
        let s = spec(0.0, 10.0, Some(10));
        assert_eq!(accept_value(&s, 3.0, 3.2), None, "snaps back onto 3");
        assert_eq!(accept_value(&s, 3.0, 3.6), Some(4.0));
        assert_eq!(accept_value(&SPEC_UNIT, 0.5, 0.5), None);
        assert_eq!(accept_value(&SPEC_UNIT, 0.5, 0.6), Some(0.6));
        assert_eq!(accept_value(&SPEC_UNIT, 0.5, 9.0), Some(1.0), "clamped");
    }

    // -- track anatomy ------------------------------------------------------

    #[test]
    fn a_zero_length_track_resolves_no_metrics() {
        let mut g = geometry(0.5, SliderTrackKind::Standard);
        g.extent = 0.0;
        assert!(TrackMetrics::resolve(&g).is_none());
    }

    #[test]
    fn a_standard_track_paints_the_inactive_remainder_then_the_active_span() {
        let g = geometry(0.5, SliderTrackKind::Standard);
        let m = TrackMetrics::resolve(&g).expect("metrics");
        let segments = m.segments(false);
        assert_eq!(segments.len(), 2, "inactive trailing + active");

        let inactive = segments[0];
        let active = segments[1];
        assert_eq!(active.role, SegmentRole::Active);
        assert_eq!(inactive.role, SegmentRole::Inactive);

        // gap = handle_thickness/2 + handle_gap = 2 + 6 = 8, on both sides of
        // the thumb at 100.
        assert_eq!(active.start, 0.0);
        assert_eq!(active.end, 92.0);
        assert_eq!(inactive.start, 108.0);
        assert_eq!(inactive.end, 200.0);
        assert_eq!(
            inactive.start - active.end,
            2.0 * (HANDLE_WIDTH / 2.0 + HANDLE_GAP)
        );

        // Asymmetric radii: outer end full, inner (gap-facing) end small.
        assert_eq!(active.start_corner, TRACK_CORNER_RADIUS);
        assert_eq!(active.end_corner, TRACK_INSIDE_CORNER);
        assert_eq!(inactive.start_corner, TRACK_INSIDE_CORNER);
        assert_eq!(inactive.end_corner, TRACK_CORNER_RADIUS);
    }

    #[test]
    fn the_outer_corner_clamps_to_half_the_track_thickness() {
        let mut g = geometry(0.5, SliderTrackKind::Standard);
        g.track_thickness = 6.0;
        let m = TrackMetrics::resolve(&g).expect("metrics");
        assert_eq!(m.corner, 3.0, "clamped from the 8dp token");
    }

    #[test]
    fn a_thumb_at_either_end_drops_the_segment_it_would_swallow() {
        // At the minimum the active span is shorter than its own corner, so
        // it is not painted at all; only the inactive remainder is.
        let m = TrackMetrics::resolve(&geometry(0.0, SliderTrackKind::Standard)).expect("metrics");
        let segments = m.segments(false);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].role, SegmentRole::Inactive);

        // At the maximum the trailing inactive span is gone.
        let m = TrackMetrics::resolve(&geometry(1.0, SliderTrackKind::Standard)).expect("metrics");
        let segments = m.segments(false);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].role, SegmentRole::Active);
    }

    #[test]
    fn a_centered_track_grows_from_the_midpoint_and_keeps_both_inactive_ends() {
        let g = geometry(0.75, SliderTrackKind::Centered);
        let m = TrackMetrics::resolve(&g).expect("metrics");
        assert_eq!(m.center_axis, 100.0);
        let segments = m.segments(false);
        assert_eq!(
            segments.len(),
            3,
            "inactive leading, inactive trailing, active"
        );
        assert_eq!(segments[0].role, SegmentRole::Inactive);
        assert_eq!(segments[1].role, SegmentRole::Inactive);
        assert_eq!(segments[2].role, SegmentRole::Active);

        // Active runs from the midpoint to the thumb less its gap; both its
        // ends face a gap, so both take the small radius. The gap on the
        // midpoint side is carved out of the *inactive* span, which stops 8
        // short of the centre — upstream's own asymmetry
        // (`m3e_slider_track_paint_metrics.dart:78-85`: the gap is added only
        // on the side the active span is *not* on).
        let active = segments[2];
        assert_eq!(active.start, 100.0, "the midpoint itself");
        assert_eq!(active.end, 142.0, "thumb at 150 less the 8dp gap");
        assert_eq!(active.start_corner, TRACK_INSIDE_CORNER);
        assert_eq!(active.end_corner, TRACK_INSIDE_CORNER);
        assert_eq!(segments[0].start, 0.0);
        assert_eq!(segments[0].end, 92.0, "stops short of the midpoint gap");
        assert_eq!(segments[1].start, 158.0, "thumb at 150 plus the gap");
        assert_eq!(segments[1].end, 200.0);
    }

    #[test]
    fn a_centered_track_below_the_midpoint_mirrors_the_active_span() {
        let m = TrackMetrics::resolve(&geometry(0.25, SliderTrackKind::Centered)).expect("metrics");
        let segments = m.segments(false);
        let active = segments
            .iter()
            .find(|s| s.role == SegmentRole::Active)
            .expect("an active span left of centre");
        assert_eq!(active.start, 58.0, "thumb at 50 plus the gap");
        assert_eq!(active.end, 100.0, "the midpoint itself");
    }

    /// A range track over the same 200px extent, thumbs at `low`/`high`.
    fn range_geometry(low: f64, high: f64) -> SliderGeometry {
        SliderGeometry {
            active_start_fraction: low,
            active_end_fraction: high,
            ..geometry(high, SliderTrackKind::Range)
        }
    }

    #[test]
    fn a_range_track_spans_between_two_thumbs_and_keeps_both_inactive_ends() {
        let m = TrackMetrics::resolve(&range_geometry(0.25, 0.75)).expect("metrics");
        let segments = m.segments(false);
        assert_eq!(
            segments.len(),
            3,
            "inactive leading, inactive trailing, active"
        );
        // Both thumbs carve the same 8dp gap out of the active span.
        let active = segments[2];
        assert_eq!(active.role, SegmentRole::Active);
        assert_eq!(active.start, 58.0, "low thumb at 50 plus its gap");
        assert_eq!(active.end, 142.0, "high thumb at 150 less its gap");
        assert_eq!(
            active.start_corner, TRACK_INSIDE_CORNER,
            "both ends face a gap"
        );
        assert_eq!(active.end_corner, TRACK_INSIDE_CORNER);
        assert_eq!((segments[0].start, segments[0].end), (0.0, 42.0));
        assert_eq!((segments[1].start, segments[1].end), (158.0, 200.0));
    }

    #[test]
    fn a_range_tracks_dots_clear_both_thumb_gaps() {
        let ticks = spec(0.0, 1.0, Some(4)).tick_fractions();
        let m = TrackMetrics::resolve(&range_geometry(0.25, 0.75)).expect("metrics");
        let placed = dots(&m, &ticks, &dot_spec());
        // Both end stops survive (the active span is in the middle); the ticks
        // at 0.25 and 0.75 sit inside the two thumb gaps, leaving 0.5.
        let primaries: Vec<f64> = placed.iter().map(|d| d.primary).collect();
        assert_eq!(primaries, vec![8.0, 192.0, 100.0], "{placed:?}");
        assert!(
            placed[2].active,
            "the surviving tick lies on the active span"
        );
    }

    #[test]
    fn a_range_track_hosts_no_inset_icons() {
        assert!(
            track_icon_placements(
                200.0,
                0.5,
                SliderTrackKind::Range,
                TRACK_ICON_SIZE,
                [true; 4]
            )
            .is_empty()
        );
    }

    #[test]
    fn rtl_mirrors_which_end_of_each_segment_takes_the_outer_radius() {
        let m = TrackMetrics::resolve(&geometry(0.5, SliderTrackKind::Standard)).expect("metrics");
        let ltr = m.segments(false);
        let rtl = m.segments(true);
        assert_eq!(ltr.len(), rtl.len());
        for (l, r) in ltr.iter().zip(rtl.iter()) {
            assert_eq!((l.start, l.end), (r.start, r.end), "spans are unmirrored");
            assert_eq!(l.start_corner, r.end_corner);
            assert_eq!(l.end_corner, r.start_corner);
        }
    }

    #[test]
    fn a_pressed_handle_narrows_the_gap_instantly() {
        let mut g = geometry(0.5, SliderTrackKind::Standard);
        g.handle_thickness = PRESSED_HANDLE_WIDTH;
        let m = TrackMetrics::resolve(&g).expect("metrics");
        assert_eq!(m.end_gap, PRESSED_HANDLE_WIDTH / 2.0 + HANDLE_GAP);
        assert_eq!(m.segments(false)[1].end, 93.0, "active reaches 1dp further");
    }

    // -- dots ---------------------------------------------------------------

    fn dot_spec() -> DotSpec {
        DotSpec {
            stop_size: STOP_INDICATOR_SIZE,
            tick_size: TICK_SIZE,
            edge_inset: STOP_INDICATOR_TRAILING_SPACE,
        }
    }

    #[test]
    fn a_continuous_track_keeps_only_the_stop_the_active_span_does_not_cover() {
        let m = TrackMetrics::resolve(&geometry(0.5, SliderTrackKind::Standard)).expect("metrics");
        let placed = dots(&m, &[], &dot_spec());
        assert_eq!(placed.len(), 1, "the leading stop is under the active span");
        // inset 6 + half the 4dp dot = 8 from the trailing end.
        assert_eq!(placed[0].primary, 192.0);
        assert_eq!(placed[0].size, STOP_INDICATOR_SIZE);
        assert!(!placed[0].active, "an end stop always takes the stop role");
    }

    #[test]
    fn a_thumb_at_the_maximum_leaves_no_stop_at_all() {
        let m = TrackMetrics::resolve(&geometry(1.0, SliderTrackKind::Standard)).expect("metrics");
        assert!(
            dots(&m, &[], &dot_spec()).is_empty(),
            "both ends sit on the active span or in its gap"
        );
    }

    #[test]
    fn discrete_ticks_skip_the_endpoints_and_the_handle_gap() {
        let ticks = spec(0.0, 1.0, Some(4)).tick_fractions();
        let m = TrackMetrics::resolve(&geometry(0.5, SliderTrackKind::Standard)).expect("metrics");
        let placed = dots(&m, &ticks, &dot_spec());
        // Five marks over the padded 8..192 span: both endpoints belong to the
        // stop indicators (the leading one is under the active span), and the
        // middle tick at 100 sits inside the thumb gap.
        assert_eq!(placed.len(), 3, "{placed:?}");
        assert_eq!(placed[0].primary, 192.0, "the surviving end stop, first");
        assert_eq!(placed[1].primary, 54.0, "tick at 0.25");
        assert_eq!(placed[2].primary, 146.0, "tick at 0.75");
        assert!(placed[1].active, "0.25 lies on the active span");
        assert!(!placed[2].active, "0.75 lies past the thumb");
    }

    #[test]
    fn a_longer_active_span_flips_more_ticks_to_the_active_role() {
        let ticks = spec(0.0, 1.0, Some(4)).tick_fractions();
        let m = TrackMetrics::resolve(&geometry(0.9, SliderTrackKind::Standard)).expect("metrics");
        let placed = dots(&m, &ticks, &dot_spec());
        assert_eq!(
            placed.iter().filter(|d| d.active).count(),
            3,
            "every interior tick is inside a 90%-long active span: {placed:?}"
        );
    }

    // -- thumb / indicator / dock -------------------------------------------

    #[test]
    fn the_thumb_box_is_centred_on_its_value_and_pill_rounded() {
        let thumb = thumb_rect(100.0, 22.0, HANDLE_WIDTH, HANDLE_HEIGHT);
        assert_eq!(thumb.x0, 98.0);
        assert_eq!(thumb.x1, 102.0);
        assert_eq!(thumb.y0, 0.0);
        assert_eq!(thumb.y1, 44.0);
        assert_eq!(thumb_radius(thumb), HANDLE_HEIGHT / 2.0);

        let ring = focus_ring_rect(thumb);
        assert_eq!(ring.width(), HANDLE_WIDTH + FOCUS_INFLATE);
        assert_eq!(ring.height(), HANDLE_HEIGHT + FOCUS_INFLATE);
    }

    #[test]
    fn the_value_indicator_floats_above_the_control_box() {
        let origin = value_indicator_origin(100.0);
        assert_eq!(origin.x, 76.0);
        assert_eq!(
            origin.y,
            -(VALUE_INDICATOR_BOTTOM_SPACE + VALUE_INDICATOR_LIFT)
        );
        assert!(origin.y < 0.0, "it overflows the control's own box");
    }

    fn dock_input(thumb_primary: f64) -> IconDockInput {
        IconDockInput {
            extent: 200.0,
            thumb_primary,
            icon_size: ICON_SIZE_DEFAULT,
            edge_inset: ICON_EDGE_INSET,
            handle_thickness: HANDLE_WIDTH,
            position: SliderIconPosition::End,
            reverse: false,
        }
    }

    #[test]
    fn the_end_icon_rests_at_the_trailing_edge_until_the_thumb_nears_it() {
        let far = icon_dock(&dock_input(20.0));
        assert_eq!(far.resting_center, 180.0, "extent - inset - half the icon");
        assert!(!far.docked);
        assert_eq!(far.center_at(0.0), 180.0, "resting while undocked");

        // dock limit = 2 + 12 + 8 = 22.
        let near = icon_dock(&dock_input(160.0));
        assert!(near.docked, "within the dock distance limit");
        assert_eq!(near.docked_target, 160.0 - (2.0 + 12.0 + 12.0));
        assert_eq!(near.center_at(1.0), 134.0);
    }

    #[test]
    fn a_docked_icon_stays_inside_the_track_and_reports_which_segment_it_is_over() {
        let dock = icon_dock(&dock_input(4.0));
        assert_eq!(dock.center_at(1.0), 12.0, "clamped to half the icon");
        assert!(
            !dock.over_active(180.0, 4.0, false),
            "resting past the thumb is over the inactive segment"
        );
        assert!(dock.over_active(2.0, 4.0, false));
    }

    #[test]
    fn a_track_narrower_than_its_icon_docks_without_panicking() {
        // `extent` (10px) is well under `ICON_SIZE_DEFAULT` (24px), the exact
        // shape a slider laid out inside a tight constraint produces —
        // `icon_half` (12) would otherwise exceed `extent - icon_half` (-2),
        // inverting `IconDock`'s clamp bounds.
        let mut input = dock_input(4.0);
        input.extent = 10.0;
        let dock = icon_dock(&input);
        assert_eq!(
            dock.min_center, -2.0,
            "ordered: extent - icon_half < icon_half"
        );
        assert_eq!(dock.max_center, 12.0);
        // Both the resting and docked targets fall outside the (now narrow
        // and inverted-were-it-not-ordered) window, so every dock progress
        // pins at the same clamped edge instead of panicking.
        assert_eq!(dock.center_at(0.0), dock.min_center);
        assert_eq!(dock.center_at(1.0), dock.min_center);
        assert!(dock.center_at(0.5).is_finite());
    }

    #[test]
    fn a_start_positioned_icon_rests_at_the_leading_edge() {
        let mut input = dock_input(180.0);
        input.position = SliderIconPosition::Start;
        assert_eq!(icon_dock(&input).resting_center, 20.0);
    }

    // -- inset track icons --------------------------------------------------

    #[test]
    fn inset_track_icons_place_per_segment_and_drop_out_of_a_short_one() {
        let all = [true; 4];
        let placed =
            track_icon_placements(200.0, 0.5, SliderTrackKind::Standard, TRACK_ICON_SIZE, all);
        assert_eq!(placed.len(), 4);
        assert_eq!(placed[0].primary, TRACK_ICON_INSET);
        assert_eq!(
            placed[1].primary,
            100.0 - TRACK_ICON_SIZE - TRACK_ICON_INSET
        );
        assert_eq!(placed[2].primary, 104.0);
        assert_eq!(
            placed[3].primary,
            200.0 - TRACK_ICON_SIZE - TRACK_ICON_INSET
        );
        assert!(placed[0].active && placed[1].active);
        assert!(!placed[2].active && !placed[3].active);

        // A 10px-long active segment cannot host a 16px icon plus its slack.
        let squeezed =
            track_icon_placements(200.0, 0.05, SliderTrackKind::Standard, TRACK_ICON_SIZE, all);
        assert!(squeezed.iter().all(|p| !p.active), "{squeezed:?}");
    }

    #[test]
    fn a_slot_with_no_icon_is_never_placed() {
        let only_active_start = [true, false, false, false];
        let placed = track_icon_placements(
            200.0,
            0.5,
            SliderTrackKind::Standard,
            TRACK_ICON_SIZE,
            only_active_start,
        );
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].slot, TrackIconSlot::ActiveStart);
    }

    #[test]
    fn a_centered_track_hosts_active_slot_icons_only() {
        let placed = track_icon_placements(
            200.0,
            0.75,
            SliderTrackKind::Centered,
            TRACK_ICON_SIZE,
            [true; 4],
        );
        assert_eq!(placed.len(), 2);
        assert_eq!(placed[0].slot, TrackIconSlot::ActiveStart);
        assert_eq!(
            placed[0].primary,
            100.0 - TRACK_ICON_SIZE - TRACK_ICON_INSET
        );
        assert_eq!(placed[1].slot, TrackIconSlot::ActiveEnd);
        assert_eq!(placed[1].primary, 150.0 + TRACK_ICON_INSET);
    }

    // -- drag machine -------------------------------------------------------

    fn drag_ctx(spec: ValueSpec, current: f64) -> DragCtx {
        DragCtx {
            spec,
            current,
            extent: 200.0,
            reverse: false,
        }
    }

    #[test]
    fn down_move_up_reports_values_then_settles() {
        let spec = spec(0.0, 100.0, None);
        let mut drag = Drag::default();

        let down = drag.down(50.0, &drag_ctx(spec, 0.0));
        assert!(down.handled && down.capture);
        assert_eq!(down.value, Some(25.0));
        assert!(!down.end);
        assert!(drag.pressed && drag.dragging);

        let moved = drag.moved(150.0, &drag_ctx(spec, 25.0));
        assert_eq!(moved.value, Some(75.0));
        assert!(!moved.capture, "the capture is already open");

        let up = drag.up();
        assert!(up.end, "settling owes on_change_end");
        assert_eq!(up.value, None, "and reports no new value");
        assert!(!drag.pressed && !drag.dragging);
    }

    #[test]
    fn an_uncaptured_move_or_up_is_ignored_outright() {
        let spec = ValueSpec::default();
        let mut drag = Drag::default();
        assert_eq!(
            drag.moved(100.0, &drag_ctx(spec, 0.0)),
            DragOutcome::default()
        );
        assert_eq!(drag.up(), DragOutcome::default());
        assert_eq!(drag.cancel(), DragOutcome::default());
        assert!(!drag.pressed, "a hover move never presses");
    }

    #[test]
    fn cancel_disarms_without_owing_a_change_end() {
        let spec = ValueSpec::default();
        let mut drag = Drag::default();
        drag.down(100.0, &drag_ctx(spec, 0.0));
        let cancelled = drag.cancel();
        assert!(cancelled.handled);
        assert!(!cancelled.end, "a Cancel arm may never reach app state");
        assert!(!drag.pressed && !drag.dragging);
    }

    #[test]
    fn a_move_that_lands_on_the_held_value_reports_nothing() {
        let spec = spec(0.0, 10.0, Some(10));
        let mut drag = Drag::default();
        drag.down(100.0, &drag_ctx(spec, 0.0));
        // 101px still snaps to 5 on a 10-division, 200px track.
        assert_eq!(drag.moved(101.0, &drag_ctx(spec, 5.0)).value, None);
        assert_eq!(drag.moved(121.0, &drag_ctx(spec, 5.0)).value, Some(6.0));
    }

    // -- haptic scheduler ---------------------------------------------------

    fn ft_ms(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    #[test]
    fn the_haptic_scheduler_paces_continuous_ticks_and_lets_discrete_ones_through() {
        // Continuous: the first tick fires, then one per 60ms of frame clock.
        let mut sched = HapticScheduler::default();
        assert!(!sched.has_pending());
        sched.queue(HapticSignal::SliderTick, HapticCadence::Throttled);
        assert!(sched.has_pending());
        assert_eq!(sched.take(ft_ms(0)), Some(HapticSignal::SliderTick));
        assert!(!sched.has_pending(), "taking drains the queue");
        assert_eq!(
            sched.take(ft_ms(100)),
            None,
            "nothing queued, nothing fires"
        );

        // Step frames at 16ms; only the frames at/after each 60ms boundary
        // fire, and an ineligible signal is dropped rather than deferred.
        let mut fired = 0;
        for frame in 1..=12 {
            sched.queue(HapticSignal::SliderTick, HapticCadence::Throttled);
            if sched.take(ft_ms(frame * 16)).is_some() {
                fired += 1;
            }
        }
        assert_eq!(
            fired, 3,
            "12 frames * 16ms = 192ms after the first tick: 64/128/192"
        );

        // Discrete: every queued tick fires, whatever the clock says.
        let mut sched = HapticScheduler::default();
        let mut fired = 0;
        for frame in 0..12 {
            sched.queue(HapticSignal::SliderTick, HapticCadence::Immediate);
            if sched.take(ft_ms(frame)).is_some() {
                fired += 1;
            }
        }
        assert_eq!(fired, 12, "a per-division tick is never throttled");
    }

    #[test]
    fn a_second_queue_before_the_next_paint_collapses_into_one_tick() {
        let mut sched = HapticScheduler::default();
        sched.queue(HapticSignal::Light, HapticCadence::Immediate);
        sched.queue(HapticSignal::Heavy, HapticCadence::Immediate);
        assert_eq!(sched.take(ft_ms(0)), Some(HapticSignal::Heavy));
        assert_eq!(sched.take(ft_ms(1)), None);
    }
}
