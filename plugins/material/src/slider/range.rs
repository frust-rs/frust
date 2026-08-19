//! The Material 3 Expressive `RangeSlider`: two thumbs over one horizontal
//! track, selecting a `start..=end` span. Flat or wavy, like its single-thumb
//! sibling.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/sliders/` — `m3e_range_slider.dart`,
//! `components/m3e_range_slider_build.dart`,
//! `components/m3e_range_slider_track.dart`,
//! `models/m3e_slider_range.dart`, `models/m3e_slider_range_labels.dart`
//! (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Why a second widget
//!
//! Everything geometric is shared: the range track is
//! [`SliderTrackKind::Range`] fed through the same
//! [`core`](mod@super::core) formulas with the low thumb's fraction in
//! `active_start_fraction`, and every paint call routes through the same
//! helpers [`super`] uses. What cannot be shared is *state*: two thumbs need
//! two [`Drag`] machines, an arbitration flag naming which one a drag owns, and
//! a pair-shaped value and callback. Folding that into `SliderView` would put
//! an `Option<f64>` second thumb on every plain slider, so the reference's own
//! split into a second widget class is kept.
//!
//! # Thumb arbitration and the no-cross clamp
//!
//! A press picks the thumb whose *painted* position is nearer, ties going to
//! the low thumb (`_selectThumb`'s `distStart <= distEnd`), and that choice
//! holds for the whole drag — a pointer that runs past the other thumb keeps
//! dragging the one it grabbed. Neither thumb can cross the other: the low
//! thumb clamps to `min..=end`, the high one to `start..=max` before snapping
//! (`_setThumbValue`), so they can meet but never swap. Both are pinned by this
//! module's own tests.
//!
//! Two departures from the reference, both inherited from [`super`]'s core and
//! documented there: a `Cancel` reports nothing (it may not reach app state),
//! and a drag deduplicates against the last pair it reported rather than the
//! app-confirmed one, since a rebuild is a frame away rather than synchronous.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, Role, SemanticsCtx, View, Widget,
};
use kurbo::Size;

use super::core::{
    DEFAULT_EXTENT, Drag, DragCtx, HANDLE_GAP, HANDLE_HEIGHT, HANDLE_WIDTH, HapticCadence,
    HapticScheduler, PRESS_MORPH_DURATION, PRESSED_HANDLE_WIDTH, SliderGeometry, SliderTrackKind,
    TrackMetrics, ValueSpec, accept_value,
};
use super::variants::{AxisMap, SliderAxis, WaveClock, WaveOverrides};
use super::{
    AmplitudeCurve, Overrides, Run, label_large, paint_dots, paint_thumb, paint_track,
    paint_value_indicator, resolve_colors, wave_paint,
};
use crate::interaction::{HapticSignal, InteractionState, MaterialHaptics};
use crate::press::presses;

/// A `start..=end` pair of slider values, `start <= end` (`M3ESliderRange`).
///
/// Named for the pair rather than for a Rust `Range` (which is half-open and
/// carries iteration semantics this has none of), and kept a plain `Copy`
/// struct so an app can hold it in state and compare it directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    /// The low thumb's value.
    pub start: f64,
    /// The high thumb's value.
    pub end: f64,
}

impl SliderRange {
    /// A range from `start` to `end`.
    ///
    /// Upstream asserts `start <= end` (`m3e_slider_range.dart:13`); an
    /// inverted pair `debug_assert!`s here and otherwise orders itself, so a
    /// shipped app degrades to the sorted pair rather than panicking or
    /// painting an inside-out track.
    pub fn new(start: f64, end: f64) -> Self {
        debug_assert!(start <= end, "a slider range's start must be <= its end");
        Self {
            start: start.min(end),
            end: start.max(end),
        }
    }
}

/// Clamp `value` into `spec`'s bounds — the identity on every in-bounds,
/// finite pair, never quantizing onto the division grid. The adoption-step
/// counterpart to [`ValueSpec::snap`]: `snap` resolves a *drag* onto a step,
/// while this is what [`normalize_range`] uses to keep a controlled
/// component's already-app-chosen value exactly as supplied.
fn clamp_into_bounds(spec: &ValueSpec, value: f64) -> f64 {
    if !(spec.min.is_finite() && spec.max.is_finite() && spec.max > spec.min) {
        return if spec.min.is_finite() { spec.min } else { 0.0 };
    }
    if value.is_nan() {
        return spec.min;
    }
    value.clamp(spec.min, spec.max)
}

/// Clamp `values` into `spec`'s current bounds and re-sort the pair, the
/// build/rebuild adoption step that keeps `RangeSliderWidget::accept`'s
/// `live.start`/`live.end` provably within `spec`'s own bounds.
///
/// **Clamps, never snaps.** [`clamp_into_bounds`] is the identity on every
/// in-bounds, finite pair — an off-grid app value (`SliderRange::new(30.0,
/// 70.0)` against `.range(0.0, 100.0).divisions(4)`) must round-trip through
/// adoption unchanged, not quantize onto the division grid the way
/// [`ValueSpec::snap`] would: this is a controlled component's *adoption*
/// step, not a drag's *resolution* step, and snapping here silently
/// desynced the thumbs from the raw-value indicator labels and never
/// reported the correction through `on_changed`.
///
/// Without this clamp, an app-supplied pair the widget has not yet seen
/// against a freshly widened/narrowed `range()` — `range_slider(SliderRange
/// ::new(0.0, 0.0)).range(20.0, 80.0)`, or a persisted pair replayed against
/// fresh bounds — would otherwise reach `accept`'s no-cross window with
/// `live.end < spec.min`, an inverted bound `f64::clamp` panics on.
/// [`clamp_into_bounds`] already degrades a degenerate `spec` safely, so
/// this can never itself panic; see [`super::core`]'s Clamp Discipline
/// section.
fn normalize_range(spec: &ValueSpec, values: SliderRange) -> SliderRange {
    let start = clamp_into_bounds(spec, values.start);
    let end = clamp_into_bounds(spec, values.end);
    SliderRange {
        start: start.min(end),
        end: start.max(end),
    }
}

/// Which thumb a drag owns (`_M3ERangeThumb`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Thumb {
    Start,
    End,
}

/// A view-held, typed range callback (erased on build).
type OnRange<State> = Rc<dyn Fn(&mut State, SliderRange)>;

/// A declarative M3E range slider. See the [module docs](self).
pub struct RangeSliderView<State: 'static> {
    values: SliderRange,
    spec: ValueSpec,
    wavy: bool,
    wave: WaveOverrides,
    amplitude_for_progress: Option<AmplitudeCurve>,
    enabled: bool,
    labels: Option<(String, String)>,
    haptic: HapticSignal,
    overrides: Overrides,
    on_changed: OnRange<State>,
    on_change_end: Option<OnRange<State>>,
    /// See [`super::SliderView`]'s field of the same name — the counting seam
    /// this module's tests use in place of `MaterialHaptics`' set-once hook.
    haptic_sink: fn(HapticSignal),
}

/// Create a range slider at `values` that fires `on_changed(state, requested)`
/// as either thumb is dragged (`M3ERangeSlider`).
///
/// The value range is `0.0..=1.0` until [`RangeSliderView::range`] widens it,
/// and the slider is continuous until [`RangeSliderView::divisions`] steps it.
pub fn range_slider<State: 'static, F: Fn(&mut State, SliderRange) + 'static>(
    values: SliderRange,
    on_changed: F,
) -> RangeSliderView<State> {
    RangeSliderView {
        values,
        spec: ValueSpec::default(),
        wavy: false,
        wave: WaveOverrides::default(),
        amplitude_for_progress: None,
        enabled: true,
        labels: None,
        haptic: HapticSignal::None,
        overrides: Overrides::default(),
        on_changed: Rc::new(on_changed),
        on_change_end: None,
        haptic_sink: MaterialHaptics::fire,
    }
}

/// Create a range slider whose active span is a traveling sine wave
/// (`M3ERangeSlider.wavy`). Its amplitude ramp reads the *span* between the
/// thumbs as progress, not either thumb's own value
/// (`_M3ERangeSliderState._amplitudeFactor`).
pub fn wavy_range_slider<State: 'static, F: Fn(&mut State, SliderRange) + 'static>(
    values: SliderRange,
    on_changed: F,
) -> RangeSliderView<State> {
    RangeSliderView {
        wavy: true,
        ..range_slider(values, on_changed)
    }
}

impl<State: 'static> RangeSliderView<State> {
    /// Set the value bounds (`M3ERangeSlider.min`/`max`, defaulting to
    /// `0..=1`). An inverted or empty range `debug_assert!`s and otherwise
    /// degrades to a slider pinned at its minimum, as [`super::SliderView`].
    pub fn range(mut self, min: f64, max: f64) -> Self {
        debug_assert!(max > min, "a slider's max must be greater than its min");
        self.spec.min = min;
        self.spec.max = max;
        self
    }

    /// Step the slider into `divisions` equal intervals
    /// (`M3ERangeSlider.divisions`). `0` is treated as continuous.
    pub fn divisions(mut self, divisions: u32) -> Self {
        self.spec.divisions = Some(divisions);
        self
    }

    /// Fire `on_change_end(state, values)` when an interaction settles
    /// (`M3ERangeSlider.onChangeEnd`). A cancelled drag never fires it.
    pub fn on_change_end<F: Fn(&mut State, SliderRange) + 'static>(
        mut self,
        on_change_end: F,
    ) -> Self {
        self.on_change_end = Some(Rc::new(on_change_end));
        self
    }

    /// Override both value-indicator labels (`M3ERangeSlider.labels`); the one
    /// shown is the dragged thumb's.
    pub fn labels(mut self, start: impl Into<String>, end: impl Into<String>) -> Self {
        self.labels = Some((start.into(), end.into()));
        self
    }

    /// Gate interaction and switch to the disabled colour table
    /// (`M3ERangeSlider.enabled`).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Opt into haptic feedback on value changes (`M3ERangeSlider.haptic`).
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Length of one full wave cycle, in logical px
    /// (`M3ERangeSlider.wavelength`). Inert unless the slider is wavy.
    pub fn wavelength(mut self, wavelength: f64) -> Self {
        self.wave.wavelength = Some(wavelength);
        self
    }

    /// How fast the wave travels, in logical px per second
    /// (`M3ERangeSlider.waveSpeed`). Inert unless the slider is wavy.
    pub fn wave_speed(mut self, wave_speed: f64) -> Self {
        self.wave.wave_speed = Some(wave_speed);
        self
    }

    /// Fix the wave's amplitude factor (`M3ERangeSlider.amplitude`). Inert
    /// unless the slider is wavy; outranked by
    /// [`Self::amplitude_for_progress`].
    pub fn amplitude(mut self, amplitude: f64) -> Self {
        self.wave.amplitude = Some(amplitude);
        self
    }

    /// Drive the wave's amplitude factor from the span between the thumbs
    /// (`M3ERangeSlider.amplitudeForProgress`). Inert unless wavy.
    pub fn amplitude_for_progress<F: Fn(f64) -> f64 + 'static>(mut self, curve: F) -> Self {
        self.amplitude_for_progress = Some(Rc::new(curve));
        self
    }

    /// Thickness of both track segments, in logical px
    /// (`M3ERangeSlider.trackThickness`).
    pub fn track_thickness(mut self, thickness: f64) -> Self {
        self.overrides.track_thickness = Some(thickness);
        self
    }

    /// Outer corner radius of both track segments, in logical px
    /// (`M3ERangeSlider.cornerRadius`).
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.overrides.corner_radius = Some(radius);
        self
    }

    /// Length of each thumb along the cross axis, in logical px
    /// (`M3ERangeSlider.thumbLength`).
    pub fn thumb_length(mut self, length: f64) -> Self {
        self.overrides.thumb_length = Some(length);
        self
    }

    /// Diameter of the stop/tick markers, in logical px
    /// (`M3ERangeSlider.dotSize`).
    pub fn dot_size(mut self, size: f64) -> Self {
        self.overrides.dot_size = Some(size);
        self
    }

    /// Clear space between each track end and the outer edge of its marker, in
    /// logical px (`M3ERangeSlider.dotSpacing`).
    pub fn dot_spacing(mut self, spacing: f64) -> Self {
        self.overrides.dot_spacing = Some(spacing);
        self
    }

    /// Route fired haptics somewhere other than [`MaterialHaptics`] — the
    /// counting seam this module's own tests use.
    #[cfg(test)]
    fn haptic_sink(mut self, sink: fn(HapticSignal)) -> Self {
        self.haptic_sink = sink;
        self
    }

    /// The value indicator's text for `thumb` (`_indicatorLabel`).
    fn indicator_text(&self, thumb: Thumb) -> String {
        if let Some((start, end)) = &self.labels {
            return match thumb {
                Thumb::Start => start.clone(),
                Thumb::End => end.clone(),
            };
        }
        let value = match thumb {
            Thumb::Start => self.values.start,
            Thumb::End => self.values.end,
        };
        if self.spec.divisions.is_some() {
            format!("{}", value.round())
        } else {
            format!("{value:.2}")
        }
    }
}

/// The retained widget for a [`RangeSliderView`].
pub struct RangeSliderWidget {
    /// The app-confirmed pair (source of truth; adopted on `rebuild`).
    values: SliderRange,
    spec: ValueSpec,
    ticks: Vec<f64>,
    wavy: bool,
    wave: WaveOverrides,
    amplitude_for_progress: Option<AmplitudeCurve>,
    wave_clock: WaveClock,
    enabled: bool,
    overrides: Overrides,
    haptic: HapticSignal,
    /// One machine per thumb, and which of them the live drag owns. Both
    /// machines see every phase so a disarm is never missed; only the active
    /// one resolves a value.
    drag: Drag,
    active: Option<Thumb>,
    interaction: InteractionState,
    /// The last pair reported this drag — what a change deduplicates and
    /// no-cross-clamps against until `rebuild` confirms one.
    last_reported: Option<SliderRange>,
    /// `0.0` resting → `1.0` pressed: the handle width compression, shared by
    /// both thumbs (only one is ever pressed).
    press_anim: frust::AnimationController,
    press_target: bool,
    haptics: HapticScheduler,
    haptic_sink: fn(HapticSignal),
    /// The dragged thumb's shaped label, plus the thumb it was shaped for.
    indicator: Run,
    indicator_thumb: Thumb,
    /// Both labels, kept so a press can shape the right one without waiting
    /// for the app to hand one down.
    labels: (String, String),
    on_changed: frust::authoring::ErasedArgCallback<SliderRange>,
    on_change_end: Option<frust::authoring::ErasedArgCallback<SliderRange>>,
}

impl<State: 'static> View<State> for RangeSliderView<State> {
    type Element = RangeSliderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RangeSliderWidget {
        let labels = (
            self.indicator_text(Thumb::Start),
            self.indicator_text(Thumb::End),
        );
        let mut indicator = Run::new();
        indicator.set_content(&labels.1);
        RangeSliderWidget {
            values: normalize_range(&self.spec, self.values),
            spec: self.spec,
            ticks: self.spec.tick_fractions(),
            wavy: self.wavy,
            wave: self.wave,
            amplitude_for_progress: self.amplitude_for_progress.clone(),
            wave_clock: WaveClock::new(),
            enabled: self.enabled,
            overrides: self.overrides,
            haptic: self.haptic,
            drag: Drag::default(),
            active: None,
            interaction: InteractionState::new(),
            last_reported: None,
            press_anim: frust::AnimationController::new(PRESS_MORPH_DURATION)
                .with_curve(frust::Curve::EaseOut),
            press_target: false,
            haptics: HapticScheduler::default(),
            haptic_sink: self.haptic_sink,
            indicator,
            indicator_thumb: Thumb::End,
            labels,
            on_changed: frust::authoring::erase_callback_arg(&self.on_changed),
            on_change_end: self
                .on_change_end
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RangeSliderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_changed = frust::authoring::erase_callback_arg(&self.on_changed);
        element.on_change_end = self
            .on_change_end
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        element.haptic_sink = self.haptic_sink;
        element.amplitude_for_progress = self.amplitude_for_progress.clone();
        let mut flags = ChangeFlags::NONE;

        let labels = (
            self.indicator_text(Thumb::Start),
            self.indicator_text(Thumb::End),
        );
        if element.labels != labels {
            element.labels = labels;
            element.sync_indicator();
            // The shown label is shaped at layout time, so a changed one
            // forces a relayout rather than a bare repaint.
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.spec != self.spec {
            element.spec = self.spec;
            element.ticks = self.spec.tick_fractions();
            flags |= ChangeFlags::PAINT;
        }
        if prev.values != self.values || prev.spec != self.spec {
            // Re-normalize on either edge, not just a `values` write: a
            // widened/narrowed `range()` call can leave a previously-adopted
            // pair outside the new bounds even when the app's own `values`
            // field never moved (`normalize_range`'s doc comment has the
            // full repro).
            element.values = normalize_range(&self.spec, self.values);
            element.last_reported = None;
            flags |= ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // Disabling mid-drag disarms the machine, so a still-captured
                // pointer cannot keep reporting values.
                element.drag = Drag::default();
                element.active = None;
                element.interaction.set_pressed(false);
                element.interaction.set_dragged(false);
                element.last_reported = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.haptic != self.haptic {
            element.haptic = self.haptic;
        }
        if prev.wavy != self.wavy || prev.wave != self.wave {
            element.wavy = self.wavy;
            element.wave = self.wave;
            flags |= ChangeFlags::PAINT;
        }
        if prev.overrides != self.overrides {
            element.overrides = self.overrides;
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }
}

impl RangeSliderWidget {
    /// The pair a drag resolves against: the last one it reported, or the
    /// app-confirmed one.
    fn live(&self) -> SliderRange {
        self.last_reported.unwrap_or(self.values)
    }

    /// Both thumbs' painter-space positions over a track of `extent`.
    fn thumb_positions(&self, extent: f64) -> (f64, f64) {
        let values = self.live();
        (
            self.spec.fraction(values.start) * extent,
            self.spec.fraction(values.end) * extent,
        )
    }

    /// The thumb a press at `primary` grabs: the nearer of the two, ties to
    /// the low thumb (`_selectThumb`).
    fn nearer_thumb(&self, primary: f64, extent: f64) -> Thumb {
        let (start, end) = self.thumb_positions(extent);
        if (primary - start).abs() <= (primary - end).abs() {
            Thumb::Start
        } else {
            Thumb::End
        }
    }

    /// Apply a raw value to `thumb`: no-cross clamp, snap, then the
    /// deduplication test (`_setThumbValue`). Returns the pair to report, or
    /// `None` when there is nothing new — including when the no-cross window
    /// or `raw` itself cannot be trusted (see below).
    ///
    /// The no-cross window is ordered with `min()`/`max()` right before its
    /// clamp rather than trusted to already be ordered — `live.end < spec.min`
    /// (an unnormalized `live`) would otherwise hand `f64::clamp` an inverted
    /// bound and panic. `build`/`rebuild`'s `normalize_range` keeps `live`
    /// provably in-bounds against a *finite* `spec`, but `self.spec` here is
    /// the raw, unnormalized builder value: a directly non-finite bound
    /// (`.range(f64::NAN, 100.0)`) reaches this window as-is. `f64::min`/
    /// `f64::max` already return the non-NaN side when only one operand is
    /// NaN, so a single-NaN bound never panics the clamp below — it silently
    /// collapses the window to a single point instead, which is its own
    /// defect (a value clamped against a spec that was never well-formed
    /// isn't one worth reporting), not the "both sides NaN at once" case a
    /// total-by-construction argument would need to rule out. So the ordered
    /// window is checked finite before it is ever clamped against, and a NaN
    /// `raw` is rejected the same way — either makes the drag a no-op
    /// (`None`), never a panic and never a NaN into app state.
    fn accept(&self, thumb: Thumb, raw: f64) -> Option<SliderRange> {
        let live = self.live();
        let (clamped, current) = match thumb {
            // The clamp is what keeps the thumbs from swapping: each stops at
            // the other rather than passing it.
            Thumb::Start => {
                let (lo, hi) = (self.spec.min, live.end);
                let (wlo, whi) = (lo.min(hi), lo.max(hi));
                if !(wlo.is_finite() && whi.is_finite()) || raw.is_nan() {
                    return None;
                }
                (raw.clamp(wlo, whi), live.start)
            }
            Thumb::End => {
                let (lo, hi) = (live.start, self.spec.max);
                let (wlo, whi) = (lo.min(hi), lo.max(hi));
                if !(wlo.is_finite() && whi.is_finite()) || raw.is_nan() {
                    return None;
                }
                (raw.clamp(wlo, whi), live.end)
            }
        };
        let next = accept_value(&self.spec, current, clamped)?;
        Some(match thumb {
            Thumb::Start => SliderRange {
                start: next,
                end: live.end,
            },
            Thumb::End => SliderRange {
                start: live.start,
                end: next,
            },
        })
    }

    /// Feed one pointer primary to the active thumb, reporting through
    /// `on_changed` if it moves.
    fn update(&mut self, primary: f64, extent: f64, ctx: &mut EventCtx) {
        let Some(thumb) = self.active else {
            return;
        };
        let raw = self.spec.value_from_offset(primary, extent, false);
        let Some(values) = self.accept(thumb, raw) else {
            return;
        };
        if self.haptic != HapticSignal::None {
            let cadence = if self.spec.divisions.is_some() {
                HapticCadence::Immediate
            } else {
                HapticCadence::Throttled
            };
            self.haptics.queue(self.haptic, cadence);
        }
        self.last_reported = Some(values);
        (self.on_changed)(ctx, values);
    }

    /// Point the cached indicator run at whichever thumb is being dragged.
    fn sync_indicator(&mut self) {
        let text = match self.indicator_thumb {
            Thumb::Start => &self.labels.0,
            Thumb::End => &self.labels.1,
        };
        let text = text.clone();
        self.indicator.set_content(&text);
    }

    /// This frame's amplitude factor. Upstream reads the *span* between the
    /// thumbs as progress, so a narrow selection flattens the wave exactly as
    /// a nearly-empty single-thumb track does.
    fn amplitude_factor(&self) -> f64 {
        let values = self.live();
        let progress =
            (self.spec.fraction(values.end) - self.spec.fraction(values.start)).clamp(0.0, 1.0);
        super::variants::amplitude_factor(
            progress,
            self.wave.amplitude,
            self.amplitude_for_progress.as_deref(),
        )
    }

    /// The drag machine's per-event inputs.
    ///
    /// This widget takes the machine for its *flags* (pressed/dragging/
    /// capture) and resolves the value itself in [`Self::update`], because
    /// which thumb a pointer moves — and what it clamps against — is the range
    /// slider's own call. `current` is therefore only the baseline for a
    /// `DragOutcome::value` this widget discards.
    fn drag_ctx(&self, extent: f64) -> DragCtx {
        DragCtx {
            spec: self.spec,
            current: self.live().end,
            extent,
            reverse: false,
        }
    }
}

impl Widget for RangeSliderWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_large(Theme::from_layout_ctx(ctx));
        self.indicator.layout(ctx, &style);
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            DEFAULT_EXTENT
        };
        let height = HANDLE_HEIGHT.max(self.overrides.thumb_length());
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let colors = resolve_colors(theme, self.enabled);
        let focused = ctx.has_focus();
        let o = ctx.origin();
        // Horizontal only: Compose has no vertical range slider
        // (`m3e_range_slider.dart:33`), so the axis map is here purely to keep
        // one placement path for every slider in this module tree.
        let map = AxisMap::new(SliderAxis::Horizontal, ctx.size(), false);
        let extent = map.extent();

        let values = self.live();
        let (start_primary, end_primary) = self.thumb_positions(extent);
        let handle_thickness = if self.drag.pressed {
            PRESSED_HANDLE_WIDTH
        } else {
            HANDLE_WIDTH
        };
        let track_thickness = self.overrides.track_thickness();
        let geometry = SliderGeometry {
            extent,
            active_start_fraction: self.spec.fraction(values.start),
            active_end_fraction: self.spec.fraction(values.end),
            reverse: false,
            kind: SliderTrackKind::Range,
            handle_thickness,
            handle_gap: HANDLE_GAP
                + if focused {
                    super::core::FOCUS_HANDLE_GAP_EXTRA
                } else {
                    0.0
                },
            track_thickness,
            corner_radius: self.overrides.corner_radius(),
        };

        if self.drag.pressed != self.press_target {
            self.press_anim
                .animate_to(if self.drag.pressed { 1.0 } else { 0.0 });
            self.press_target = self.drag.pressed;
        }
        if self.press_anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let wave = self.wavy.then(|| {
            let factor = self.amplitude_factor();
            wave_paint(&mut self.wave_clock, self.wave, factor, ctx, reduce_motion)
        });
        if let Some(signal) = self.haptics.take(ctx.frame_time()) {
            (self.haptic_sink)(signal);
        }

        if let Some(metrics) = TrackMetrics::resolve(&geometry) {
            paint_track(scene, o, &map, &metrics, track_thickness, wave, &colors);
            let spec = super::core::DotSpec {
                stop_size: self.overrides.dot_size(),
                tick_size: self.overrides.tick_size(),
                edge_inset: self.overrides.dot_spacing(),
            };
            paint_dots(scene, o, &map, &metrics, &self.ticks, &spec, &colors);
        }

        // Only the dragged thumb compresses; the idle one keeps its resting
        // width (`m3e_range_slider_build.dart:249-268`'s per-thumb `pressed`).
        let pressed_thickness =
            HANDLE_WIDTH + (PRESSED_HANDLE_WIDTH - HANDLE_WIDTH) * self.press_anim.value_clamped();
        let length = self.overrides.thumb_length();
        let overlay = self.interaction.resolve_opacity();
        // The focus outline follows upstream's `_keyboardThumb` — the dragged
        // thumb, or the high one when nothing is being dragged.
        let focus_thumb = self.active.unwrap_or(Thumb::End);
        for (thumb, primary) in [(Thumb::Start, start_primary), (Thumb::End, end_primary)] {
            let dragged = self.active == Some(thumb);
            let thickness = if dragged {
                pressed_thickness
            } else {
                HANDLE_WIDTH
            };
            paint_thumb(
                scene,
                o,
                map.place_thumb(primary, thickness, length),
                focused && focus_thumb == thumb,
                if dragged { overlay } else { 0.0 },
                &colors,
            );
        }

        if self.drag.pressed {
            let primary = match self.indicator_thumb {
                Thumb::Start => start_primary,
                Thumb::End => end_primary,
            };
            paint_value_indicator(scene, o, &map, &self.indicator, primary, &colors);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let extent = ctx.size().width;
        let primary = p.position.x;
        match p.phase {
            PointerPhase::Down => {
                if !self.enabled || !presses(p) {
                    return EventResult::Ignored;
                }
                // Arbitrate first: the machine's own `down` reports nothing
                // here, since which thumb it moves is this widget's call.
                let thumb = self.nearer_thumb(primary, extent);
                self.active = Some(thumb);
                self.indicator_thumb = thumb;
                self.sync_indicator();
                let out = self.drag.down(primary, &self.drag_ctx(extent));
                debug_assert!(out.capture, "a press always opens a capture");
                ctx.capture_pointer();
                self.update(primary, extent, ctx);
                self.settle(ctx, false)
            }
            PointerPhase::Move => {
                if !self.drag.moved(primary, &self.drag_ctx(extent)).handled {
                    return EventResult::Ignored;
                }
                self.update(primary, extent, ctx);
                self.settle(ctx, false)
            }
            PointerPhase::Up => {
                if !self.drag.up().handled {
                    return EventResult::Ignored;
                }
                self.active = None;
                self.settle(ctx, true)
            }
            PointerPhase::Cancel => {
                // No callback, no state reach: see `core::Drag::cancel`.
                if !self.drag.cancel().handled {
                    return EventResult::Ignored;
                }
                self.active = None;
                self.settle(ctx, false)
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Two thumbs, one node: no platform role carries a pair, so the node
        // reports the track's own bounds with the high thumb as its numeric
        // value, and announces the pair as one string — upstream's own
        // fallback (`m3e_range_slider.dart:298-300`).
        ctx.push_node(Role::Slider, |node| {
            node.set_numeric_value(self.values.end);
            node.set_min_numeric_value(self.spec.min);
            node.set_max_numeric_value(self.spec.max);
            node.set_value(format!("{} – {}", self.values.start, self.values.end));
        });
    }
}

impl RangeSliderWidget {
    /// Mirror the drag flags into the interaction model, owe `on_change_end`
    /// when the interaction settled, and request the follow-up frame.
    fn settle(&mut self, ctx: &mut EventCtx, ended: bool) -> EventResult {
        if ended && let Some(on_change_end) = &mut self.on_change_end {
            let settled = self.last_reported.unwrap_or(self.values);
            on_change_end(ctx, settled);
        }
        self.interaction.set_pressed(self.drag.pressed);
        self.interaction.set_dragged(self.drag.dragging);
        ctx.request_redraw();
        EventResult::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{CornerRadii, PointerButton, PointerEvent};
    use kurbo::{BezPath, Point};
    use peniko::{Brush, Color};
    use std::any::Any;
    use std::sync::Mutex;

    const WIDTH: f64 = 200.0;
    const SIZE: Size = Size::new(WIDTH, HANDLE_HEIGHT);

    #[derive(Default)]
    struct Val {
        changes: Vec<SliderRange>,
        ends: Vec<SliderRange>,
    }

    fn view(start: f64, end: f64) -> RangeSliderView<Val> {
        range_slider::<Val, _>(
            SliderRange::new(start, end),
            |s: &mut Val, v: SliderRange| {
                s.changes.push(v);
            },
        )
        .on_change_end(|s: &mut Val, v: SliderRange| s.ends.push(v))
    }

    fn build(view: &RangeSliderView<Val>) -> RangeSliderWidget {
        let mut counter = 0u64;
        View::<Val>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn widget(start: f64, end: f64) -> RangeSliderWidget {
        build(&view(start, end))
    }

    fn ev(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, HANDLE_HEIGHT / 2.0),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut RangeSliderWidget, state: &mut Val, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, SIZE);
        w.event(&mut ctx, event)
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        radii: Vec<(Point, Size, CornerRadii, Color)>,
        strokes: Vec<(f64, Color)>,
        runs: Vec<Point>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.radii.push((o, s, radii, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((width, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.runs.push(Point::new(t.x, t.y));
        }
    }

    fn layout(w: &mut RangeSliderWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut ctx, &BoxConstraints::loose(SIZE))
    }

    fn paint(w: &mut RangeSliderWidget) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, SIZE);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // -- arbitration --------------------------------------------------------

    #[test]
    fn a_press_grabs_the_nearer_thumb_and_keeps_it_for_the_whole_drag() {
        let mut w = widget(0.25, 0.75);
        let mut state = Val::default();

        // 60px is nearer the low thumb (at 50) than the high one (at 150).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 60.0));
        assert_eq!(w.active, Some(Thumb::Start));
        assert_eq!(state.changes, vec![SliderRange::new(0.3, 0.75)]);

        // Dragging past the *other* thumb's resting position keeps the grab:
        // the low thumb simply stops where the high one is.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 180.0));
        assert_eq!(
            w.active,
            Some(Thumb::Start),
            "the grab survives the crossing"
        );
        assert_eq!(
            state.changes.last(),
            Some(&SliderRange::new(0.75, 0.75)),
            "clamped to the high thumb, never past it"
        );

        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 180.0));
        assert_eq!(w.active, None);
        assert_eq!(state.ends, vec![SliderRange::new(0.75, 0.75)]);
    }

    #[test]
    fn a_value_pair_outside_a_freshly_widened_range_normalizes_instead_of_panicking() {
        // The exact repro shape from the review finding: values sit outside
        // a `range()` call that only widens the bounds afterward. Before the
        // fix, `accept`'s no-cross window (`spec.min..=live.end` /
        // `live.start..=spec.max`) would be handed an inverted bound
        // (`live.end < spec.min`) on the very first press — an `f64::clamp`
        // panic, not a catchable error.
        let mut w = build(&view(0.0, 90.0).range(20.0, 80.0));
        assert_eq!(
            w.values,
            SliderRange::new(20.0, 80.0),
            "adoption clamps each side into the new bounds"
        );
        let mut state = Val::default();

        // Press+drag the low thumb (nearer the left edge, position 0).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0));
        assert_eq!(w.active, Some(Thumb::Start));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 60.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 60.0));
        assert!(
            state
                .changes
                .iter()
                .all(|v| v.start >= 20.0 && v.start <= v.end),
            "{:?}",
            state.changes
        );

        // Press+drag the high thumb (nearer the right edge, position 200)
        // from a fresh widget over the same out-of-bounds pair.
        let mut w = build(&view(0.0, 90.0).range(20.0, 80.0));
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 195.0));
        assert_eq!(w.active, Some(Thumb::End));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 150.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 150.0));
        assert!(
            state
                .changes
                .iter()
                .all(|v| v.end <= 80.0 && v.end >= v.start),
            "{:?}",
            state.changes
        );
    }

    #[test]
    fn a_rebuilds_narrowed_bounds_renormalize_a_stale_out_of_bounds_pair() {
        // "A persisted pair vs fresh bounds": the app's own `values` field
        // never changes across the rebuild, only `range()` does — the
        // narrower `if prev.values != self.values` check alone would miss
        // this edge and leave `element.values` stale and out of the new
        // bounds, an unnormalized `live` the next press's `accept` call
        // would panic on.
        let mut counter = 0u64;
        let prev = view(0.0, 90.0).range(0.0, 100.0);
        let mut w = build(&prev);
        assert_eq!(w.values, SliderRange::new(0.0, 90.0));

        let next = view(0.0, 90.0).range(20.0, 80.0);
        let flags = View::<Val>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.values, SliderRange::new(20.0, 80.0));
        assert!(flags.needs_paint());

        // The renormalized pair presses and drags without panicking.
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0));
    }

    // -- normalize_range / clamp_into_bounds ---------------------------------

    #[test]
    fn an_in_bounds_off_grid_pair_survives_adoption_verbatim() {
        // `normalize_range` must clamp into bounds, never snap onto the
        // division grid: 30/70 sit strictly inside `.range(0.0, 100.0)
        // .divisions(4)`'s bounds but off its 25-wide grid
        // (0/25/50/75/100) — snapping would silently turn this into 25/75,
        // desyncing the thumbs from the raw-value indicator labels with no
        // `on_changed` report.
        let mut w = build(&view(30.0, 70.0).range(0.0, 100.0).divisions(4));
        assert_eq!(
            w.values,
            SliderRange::new(30.0, 70.0),
            "an in-bounds off-grid pair must round-trip through adoption verbatim"
        );

        layout(&mut w);
        let rec = paint(&mut w);
        let thumbs: Vec<&(Point, Size, f64, Color)> = rec
            .rrects
            .iter()
            .filter(|(_, s, _, _)| s.height == HANDLE_HEIGHT)
            .collect();
        assert_eq!(thumbs.len(), 2);
        // fraction(30) over 0..100 = 0.3 of the 200px track = 60.0;
        // fraction(70) = 0.7 = 140.0 — the un-snapped painted positions.
        assert_eq!(thumbs[0].0.x, 60.0 - HANDLE_WIDTH / 2.0, "low thumb at 30");
        assert_eq!(
            thumbs[1].0.x,
            140.0 - HANDLE_WIDTH / 2.0,
            "high thumb at 70"
        );
    }

    #[test]
    fn normalize_range_clamps_an_out_of_bounds_pair_to_the_bounds() {
        let spec = ValueSpec {
            min: 20.0,
            max: 80.0,
            divisions: None,
        };
        assert_eq!(
            normalize_range(&spec, SliderRange::new(0.0, 90.0)),
            SliderRange::new(20.0, 80.0)
        );
    }

    #[test]
    fn normalize_range_pins_a_nan_side_to_min() {
        let spec = ValueSpec {
            min: 20.0,
            max: 80.0,
            divisions: None,
        };
        assert_eq!(
            normalize_range(
                &spec,
                SliderRange {
                    start: f64::NAN,
                    end: 50.0,
                }
            ),
            SliderRange::new(20.0, 50.0)
        );
    }

    // -- NaN robustness (accept/snap) ----------------------------------------

    #[test]
    fn a_nan_min_bound_never_panics_or_reports_a_nan_value() {
        // The refuted-unreachable case: a single NaN bound reaching `accept`'s
        // no-cross window, not both `lo`/`hi` at once. `.range()`'s own
        // `debug_assert!` guards the public builder path in a debug/test
        // build, but that guard compiles out entirely in release — `spec.min`
        // is set directly here (bypassing the builder) to exercise the
        // widget-level contract `accept`/`snap` must hold regardless of how a
        // NaN bound arrives.
        let mut w = widget(0.3, 0.7);
        w.spec.min = f64::NAN;
        w.spec.max = 100.0;
        let mut state = Val::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 60.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 90.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 90.0));

        assert!(
            state
                .changes
                .iter()
                .all(|v| !v.start.is_nan() && !v.end.is_nan()),
            "no reported value may carry a NaN: {:?}",
            state.changes
        );
        assert!(
            state
                .ends
                .iter()
                .all(|v| !v.start.is_nan() && !v.end.is_nan()),
            "{:?}",
            state.ends
        );
    }

    #[test]
    fn a_tie_goes_to_the_low_thumb_and_the_far_side_grabs_the_high_one() {
        let mut w = widget(0.25, 0.75);
        let mut state = Val::default();
        // Exactly between the two thumbs.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 100.0));
        assert_eq!(w.active, Some(Thumb::Start), "a tie goes to the low thumb");

        let mut w = widget(0.25, 0.75);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 140.0));
        assert_eq!(w.active, Some(Thumb::End));
        assert_eq!(state.changes.last(), Some(&SliderRange::new(0.25, 0.7)));
    }

    #[test]
    fn the_high_thumb_cannot_be_dragged_below_the_low_one() {
        let mut w = widget(0.25, 0.75);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 150.0));
        assert_eq!(w.active, Some(Thumb::End));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 0.0));
        assert_eq!(
            state.changes.last(),
            Some(&SliderRange::new(0.25, 0.25)),
            "it meets the low thumb and stops"
        );
        // And a further move reports nothing new at all.
        let before = state.changes.len();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, -40.0));
        assert_eq!(state.changes.len(), before);
    }

    #[test]
    fn callbacks_fire_in_order_and_settle_once() {
        let mut w = widget(0.2, 0.8);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 160.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 170.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 260.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 260.0));
        assert_eq!(
            state.changes,
            vec![SliderRange::new(0.2, 0.85), SliderRange::new(0.2, 1.0)],
            "the press landed on the value already held, so it reported nothing"
        );
        assert_eq!(state.ends, vec![SliderRange::new(0.2, 1.0)]);
        assert_eq!(
            w.values,
            SliderRange::new(0.2, 0.8),
            "a controlled slider never writes its own values"
        );
    }

    #[test]
    fn cancel_disarms_without_reporting_or_settling() {
        let mut w = widget(0.25, 0.75);
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 60.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 60.0));
        assert_eq!(w.active, None);
        assert!(!w.drag.dragging && !w.drag.pressed);
        assert!(state.ends.is_empty(), "a Cancel never reaches app state");
    }

    #[test]
    fn a_disabled_range_slider_ignores_every_phase() {
        let mut w = build(&view(0.25, 0.75).enabled(false));
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 60.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 90.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 90.0));
        assert!(state.changes.is_empty() && state.ends.is_empty());
        assert_eq!(w.active, None);
    }

    #[test]
    fn a_discrete_range_reports_only_when_a_thumb_crosses_a_step() {
        let mut w = build(&view(2.0, 8.0).range(0.0, 10.0).divisions(10));
        let mut state = Val::default();
        // 43px still snaps to the step the low thumb already sits on.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 43.0));
        assert!(state.changes.is_empty());
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 63.0));
        assert_eq!(state.changes, vec![SliderRange::new(3.0, 8.0)]);
    }

    // -- paint --------------------------------------------------------------

    #[test]
    fn the_active_span_runs_between_the_two_thumbs() {
        let mut w = widget(0.25, 0.75);
        layout(&mut w);
        let rec = paint(&mut w);
        assert_eq!(rec.radii.len(), 3, "two inactive ends plus the active span");
        let active = rec.radii[2];
        assert_eq!(active.0.x, 58.0, "low thumb at 50 plus its 8dp gap");
        assert_eq!(active.1.width, 84.0, "up to the high thumb's own gap");

        // Two thumbs, both at their resting width, plus the surviving stops.
        let thumbs: Vec<&(Point, Size, f64, Color)> = rec
            .rrects
            .iter()
            .filter(|(_, s, _, _)| s.height == HANDLE_HEIGHT)
            .collect();
        assert_eq!(thumbs.len(), 2);
        assert_eq!(thumbs[0].0.x, 50.0 - HANDLE_WIDTH / 2.0);
        assert_eq!(thumbs[1].0.x, 150.0 - HANDLE_WIDTH / 2.0);
    }

    #[test]
    fn only_the_dragged_thumb_shows_its_halo_and_indicator() {
        let mut w = widget(0.25, 0.75);
        layout(&mut w);
        let mut state = Val::default();
        assert!(paint(&mut w).runs.is_empty(), "no indicator at rest");

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 150.0));
        let rec = paint(&mut w);
        assert_eq!(rec.runs.len(), 1, "the dragged thumb's label");
        let halos = rec
            .rrects
            .iter()
            .filter(|(_, s, _, _)| s.width == HANDLE_WIDTH + super::super::core::FOCUS_INFLATE)
            .count();
        assert_eq!(halos, 1, "one halo, on the grabbed thumb only");
    }

    #[test]
    fn a_wavy_range_slider_strokes_its_active_span_and_paces_its_frames() {
        let mut w = build(&view(0.2, 0.8).amplitude(1.0));
        let mut wavy = build(&wavy_range_slider::<Val, _>(
            SliderRange::new(0.2, 0.8),
            |_s, _v| {},
        ));
        layout(&mut w);
        layout(&mut wavy);

        assert!(paint(&mut w).strokes.is_empty(), "a flat span fills");
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, SIZE);
        wavy.paint(&mut ctx, &mut rec);
        assert_eq!(rec.radii.len(), 2, "the active span is no longer a rect");
        assert_eq!(rec.strokes.len(), 1, "it is a stroked wave instead");
        assert_eq!(rec.strokes[0].0, super::super::core::TRACK_HEIGHT);
        assert!(ctx.needs_frame_paced_only(), "a wave is a cosmetic loop");
    }

    #[test]
    fn the_wave_amplitude_reads_the_span_between_the_thumbs() {
        // A 5%-wide selection is inside the ramp's flat zone; a wide one is
        // not (`amplitude_ramp`'s own boundaries).
        let narrow = build(&wavy_range_slider::<Val, _>(
            SliderRange::new(0.5, 0.55),
            |_s, _v| {},
        ));
        assert_eq!(narrow.amplitude_factor(), 0.0);
        let wide = build(&wavy_range_slider::<Val, _>(
            SliderRange::new(0.1, 0.8),
            |_s, _v| {},
        ));
        assert_eq!(wide.amplitude_factor(), 1.0);
    }

    // -- haptics ------------------------------------------------------------

    static RECORDED: Mutex<Vec<HapticSignal>> = Mutex::new(Vec::new());

    fn recording_sink(signal: HapticSignal) {
        RECORDED.lock().unwrap().push(signal);
    }

    #[test]
    fn haptics_are_opt_in_and_pace_off_the_frame_clock() {
        RECORDED.lock().unwrap().clear();
        let mut w = build(&view(0.2, 0.8).haptic_sink(recording_sink));
        let mut state = Val::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 170.0));
        assert!(
            !w.haptics.has_pending(),
            "nothing queued at HapticSignal::None"
        );

        let mut w = build(
            &view(0.2, 0.8)
                .haptic(HapticSignal::SliderTick)
                .haptic_sink(recording_sink),
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 170.0));
        assert!(w.haptics.has_pending(), "an opted-in change queues a tick");
        let fired = w.haptics.take(FrameTime::from_nanos(0)).expect("a tick");
        (w.haptic_sink)(fired);
        assert_eq!(*RECORDED.lock().unwrap(), vec![HapticSignal::SliderTick]);
        // Throttled: a second change inside 60ms of frame clock is dropped.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 172.0));
        assert!(
            w.haptics.take(FrameTime::from_nanos(10_000_000)).is_none(),
            "a continuous drag ticks at most once per 60ms"
        );
    }
}
