//! M3 progress indicators (Phase 6c, PLAN.md D5, task 10): linear (4dp) and
//! circular (40dp) variants, both determinate and indeterminate. The circular
//! variant needs the new `Command::Path`/`fill_path`/`stroke_path` primitive
//! (PLAN.md D2b, task 05). M3 Expressive **wavy** variants (Phase 6f, task
//! 06 — `LinearWavyProgressIndicator`/`CircularWavyProgressIndicator`,
//! `m3.material.io/components/progress-indicators/specs`,
//! `research/RESEARCH.md:82-83`) are additive and opt-in — see the Wavy
//! variants note below.
//!
//! [`LinearProgress`]/[`CircularProgress`] are both **controlled components**
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics): the app passes a
//! [`ProgressValue`] every frame — `Determinate(f)` for a known `0.0..=1.0`
//! fraction, or `Indeterminate` for an unknown-duration loop — and the widget
//! never has anything to report back (there is no callback; a progress
//! indicator has no user interaction). `rebuild` simply reconciles the
//! `element`'s stored value to whatever the view says, the same
//! set-if-different reconciliation `TextInput`'s `value` uses.
//!
//! **Track thickness/diameter** (research-cited, see
//! `research/RESEARCH.md`'s `m3-component-specs-idioms` section): linear
//! track thickness 4dp, indicator = `colors.primary`, track =
//! `colors.primary_container`; circular outer diameter 40dp, stroke 4dp,
//! indicator = `colors.primary`, track = transparent by default (so only the
//! indicator arc is painted, no ring behind it) — both cited to
//! `material-components-android`'s `ProgressIndicator.md`.
//!
//! **Indeterminate motion is a documented approximation, not the real M3
//! two-segment choreography.** Upstream's indeterminate linear/circular
//! motion runs two independently-eased segments with a multi-keyframe timing
//! spec (`LinearProgressIndicator`'s `firstLineHead`/`firstLineTail`/
//! `secondLineHead`/`secondLineTail` fractions, `CircularProgressIndicator`'s
//! rotating-plus-growing/shrinking arc) that is Compose-implementation
//! internal, not part of the publicly cited component spec this crate's
//! research ledger covers. This module ships a single-segment approximation
//! instead (see [`LINEAR_INDETERMINATE_SEGMENT_FRACTION`]/
//! [`CIRCULAR_INDETERMINATE_SWEEP`] below) — visually "a segment/arc loops
//! continuously", not upstream's exact choreography. Revisit if a future
//! phase needs pixel-accurate parity.
//!
//! # Wavy variants
//!
//! [`LinearProgressView::wavy`]/[`CircularProgressView::wavy`] (plus the
//! finer-grained [`LinearProgressView::wave_amplitude`]/`wave_wavelength`/
//! `wave_speed` and their circular counterparts) opt a progress indicator
//! into an M3 Expressive **wavy** active track: instead of a flat filled
//! rect/arc, the active track renders as a sine wave with a given
//! `amplitude`/`wavelength` (both logical px) whose phase advances over
//! `wave_speed` (a duration per full phase cycle), driven by the same
//! advance-during-paint [`AnimationController`] contract every animated
//! widget in this crate uses (`docs/CODE_STANDARDS.md`'s Theming & Animation
//! Conventions) — never a separate ticker. The **inactive** track always
//! stays flat, per spec. Both [`ProgressValue::Determinate`] and
//! [`ProgressValue::Indeterminate`] support wavy rendering: a determinate
//! wave spans the filled fraction of the track/arc; an indeterminate wave
//! spans the sweeping segment/arc from the existing single-segment
//! approximation above. **At `amplitude <= 0.0` (the un-opted-in default)
//! the render is byte-for-byte the existing flat path** — the wavy branch is
//! only taken when `amplitude > 0.0`, so a caller who never touches the wavy
//! builders sees no behavior change at all.
//!
//! [`WAVY_DEFAULT_AMPLITUDE`]/[`WAVY_DEFAULT_WAVELENGTH`]/
//! [`WAVY_DEFAULT_PERIOD`] are the values `.wavy()` opts in with.
//!
//! **Community-approximate**: `m3.material.io/components/progress-indicators/specs`
//! documents the wavy variant's existence and its `amplitude`/`wavelength`/
//! `waveSpeed` parameters (`research/RESEARCH.md:82-83`) but not their
//! default numeric values in a form this crate's research ledger captured;
//! the three `WAVY_DEFAULT_*` constants below are chosen to read clearly as
//! "a gentle wave" at this module's existing track dimensions (4dp linear
//! thickness / 40dp circular diameter), not a verified pixel-for-pixel port
//! of Compose's `WavyProgressIndicatorDefaults`.

use std::f64::consts::{PI, TAU};
use std::time::Duration;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, LayoutCtx, PaintCtx,
    PaintScene, SemanticsCtx, View, Widget,
};
use forgekit_scene::arc_path;
use forgekit_theme::Theme;
use kurbo::{BezPath, Point, Size};
use peniko::{Brush, Color};

/// A progress indicator's controlled value (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProgressValue {
    /// A known progress fraction. Clamped to `0.0..=1.0` wherever it is read
    /// (layout/paint/semantics), so an out-of-range app value never panics or
    /// paints outside the track.
    Determinate(f64),
    /// An unknown-duration, looping progress animation.
    Indeterminate,
}

impl ProgressValue {
    /// The clamped `0.0..=1.0` fraction, or `None` for [`ProgressValue::Indeterminate`].
    fn determinate_fraction(self) -> Option<f64> {
        match self {
            ProgressValue::Determinate(v) => Some(v.clamp(0.0, 1.0)),
            ProgressValue::Indeterminate => None,
        }
    }
}

// -- Wavy (shared by linear + circular) -----------------------------------

/// Wavy-progress wave configuration, shared by [`LinearProgressView`]/
/// [`CircularProgressView`] (see the [module docs](self)'s Wavy variants
/// note). `amplitude` `<= 0.0` — the default — means "not wavy": every
/// paint path branches on this to reproduce the flat-track render exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WaveParams {
    /// Wave crest height, in logical px. `<= 0.0` degenerates to the flat
    /// track.
    amplitude: f64,
    /// Crest-to-crest spacing, in logical px. Never read while
    /// `amplitude <= 0.0`.
    wavelength: f64,
    /// Duration for the wave phase to advance one full cycle (`wave_speed`).
    period: Duration,
}

impl Default for WaveParams {
    fn default() -> Self {
        WaveParams {
            amplitude: 0.0,
            wavelength: WAVY_DEFAULT_WAVELENGTH,
            period: WAVY_DEFAULT_PERIOD,
        }
    }
}

/// Default wavy-indicator crest amplitude, in logical px, applied by
/// `.wavy()`. **Community-approximate** — see the [module docs](self)'s Wavy
/// variants note.
const WAVY_DEFAULT_AMPLITUDE: f64 = 3.0;
/// Default wavy-indicator wavelength (crest-to-crest spacing), in logical
/// px, applied by `.wavy()`. **Community-approximate** — see the
/// [module docs](self)'s Wavy variants note.
const WAVY_DEFAULT_WAVELENGTH: f64 = 20.0;
/// Default wavy-indicator wave phase period (`wave_speed`) applied by
/// `.wavy()`. **Community-approximate** — see the [module docs](self)'s
/// Wavy variants note.
const WAVY_DEFAULT_PERIOD: Duration = Duration::from_millis(1800);

/// Straight-line samples per full wavelength used to approximate the sine
/// wave — the wavy counterpart of [`forgekit_scene::arc_path`]'s curve
/// tolerance, chosen to read as smooth at the module's track dimensions
/// without generating an excessive path element count.
const WAVE_SAMPLES_PER_WAVELENGTH: f64 = 12.0;

// -- Linear ------------------------------------------------------------

/// Linear track thickness, in logical px (source: androidx
/// `ProgressIndicator.md`, `app:trackThickness` default — see the
/// [module docs](self)).
const LINEAR_TRACK_HEIGHT: f64 = 4.0;

/// Unthemed indicator fill fallback (mirrors [`super::switch`]/`button.rs`'s
/// unthemed-primary convention; a theme resolves this from `colors.primary`).
const LINEAR_FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Unthemed track fallback (a theme resolves this from
/// `colors.primary_container`).
const LINEAR_TRACK: Color = Color::from_rgb8(0xD6, 0xE4, 0xFF);

/// Default period of the indeterminate linear sweep loop.
///
/// **Community-approximate**: this is a single-segment stand-in for
/// upstream's real multi-keyframe indeterminate timing (see the
/// [module docs](self)'s Indeterminate motion note) — not a cited spec value.
const LINEAR_INDETERMINATE_PERIOD: Duration = Duration::from_millis(1800);

/// The indeterminate sweeping segment's width, as a fraction of the track's
/// full width.
///
/// **Community-approximate**: chosen to read clearly as "a segment sweeping
/// across the track" at typical progress-bar widths — not a cited spec value
/// (see the [module docs](self)'s Indeterminate motion note).
const LINEAR_INDETERMINATE_SEGMENT_FRACTION: f64 = 0.35;

/// A determinate/indeterminate linear progress indicator. See the
/// [module docs](self).
pub struct LinearProgressView {
    value: ProgressValue,
    wave: WaveParams,
}

/// Create a linear progress indicator driven by `value`.
pub fn linear_progress(value: ProgressValue) -> LinearProgressView {
    LinearProgressView {
        value,
        wave: WaveParams::default(),
    }
}

/// PascalCase alias for [`linear_progress`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn LinearProgress(value: ProgressValue) -> LinearProgressView {
    linear_progress(value)
}

impl LinearProgressView {
    /// Opt into the M3 Expressive wavy active track at the
    /// [`WAVY_DEFAULT_AMPLITUDE`]/[`WAVY_DEFAULT_WAVELENGTH`]/
    /// [`WAVY_DEFAULT_PERIOD`] defaults — see the [module docs](self)'s Wavy
    /// variants note.
    pub fn wavy(mut self) -> Self {
        self.wave.amplitude = WAVY_DEFAULT_AMPLITUDE;
        self
    }

    /// Set the wave crest amplitude, in logical px. A value `<= 0.0`
    /// degenerates the render back to the flat track (see the
    /// [module docs](self)'s Wavy variants note); any positive value opts in
    /// even without calling [`Self::wavy`] first.
    pub fn wave_amplitude(mut self, amplitude: f64) -> Self {
        self.wave.amplitude = amplitude;
        self
    }

    /// Set the wave's crest-to-crest spacing, in logical px.
    pub fn wave_wavelength(mut self, wavelength: f64) -> Self {
        self.wave.wavelength = wavelength;
        self
    }

    /// Set `wave_speed`: the duration for the wave phase to advance one full
    /// cycle.
    pub fn wave_speed(mut self, period: Duration) -> Self {
        self.wave.period = period;
        self
    }
}

impl<State: 'static> View<State> for LinearProgressView {
    type Element = LinearProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LinearProgressWidget {
        let mut widget = LinearProgressWidget {
            value: self.value,
            wave: self.wave,
            indeterminate: AnimationController::new(LINEAR_INDETERMINATE_PERIOD)
                .with_curve(Curve::Linear),
            wave_phase: AnimationController::new(self.wave.period).with_curve(Curve::Linear),
        };
        if matches!(widget.value, ProgressValue::Indeterminate) {
            widget.indeterminate.repeat();
        }
        if widget.wave.amplitude > 0.0 {
            widget.wave_phase.repeat();
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LinearProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut changed = false;
        if prev.value != self.value {
            element.value = self.value;
            match self.value {
                ProgressValue::Indeterminate => {
                    if !element.indeterminate.is_animating() {
                        element.indeterminate.repeat();
                    }
                }
                ProgressValue::Determinate(_) => element.indeterminate.stop(),
            }
            changed = true;
        }
        if prev.wave != self.wave {
            element.wave = self.wave;
            // A wave-param change resets the phase controller (a period
            // change can't be applied to an in-flight `AnimationController`
            // — see `docs/CODE_STANDARDS.md`'s advance-during-paint
            // convention, which owns no external timer to retune).
            element.wave_phase =
                AnimationController::new(self.wave.period).with_curve(Curve::Linear);
            if self.wave.amplitude > 0.0 {
                element.wave_phase.repeat();
            }
            changed = true;
        }
        if changed {
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// The retained widget for a [`LinearProgressView`]. See the [module docs](self).
pub struct LinearProgressWidget {
    value: ProgressValue,
    wave: WaveParams,
    /// Drives the indeterminate sweep loop (`0.0..=1.0`, repeating). Idle
    /// (never advanced) while [`ProgressValue::Determinate`].
    indeterminate: AnimationController,
    /// Drives the wavy active track's phase (`0.0..=1.0`, repeating). Idle
    /// while `wave.amplitude <= 0.0`.
    wave_phase: AnimationController,
}

/// Build a stroke path tracing a sine wave across `[0, width]`, vertically
/// centered in `height` — the linear counterpart of [`wavy_arc_path`] below.
/// Straight-line segments approximate the curve at
/// [`WAVE_SAMPLES_PER_WAVELENGTH`] samples per wavelength (see the
/// [module docs](self)'s Wavy variants note).
fn linear_wave_path(
    origin: Point,
    width: f64,
    height: f64,
    amplitude: f64,
    wavelength: f64,
    phase: f64,
) -> BezPath {
    let mid_y = origin.y + height / 2.0;
    let wavelength = wavelength.max(1.0);
    let wave_y = |x: f64| mid_y - amplitude * (TAU * (x / wavelength + phase)).sin();

    let step = (wavelength / WAVE_SAMPLES_PER_WAVELENGTH).max(1.0);
    let mut path = BezPath::new();
    path.move_to(Point::new(origin.x, wave_y(0.0)));
    let mut x = step;
    while x < width {
        path.line_to(Point::new(origin.x + x, wave_y(x)));
        x += step;
    }
    // Always land the final sample exactly at `width`, so a determinate
    // fill/indeterminate segment's wave stops precisely at its clamped
    // boundary regardless of how `width` divides by `step`.
    path.line_to(Point::new(origin.x + width, wave_y(width)));
    path
}

impl LinearProgressWidget {
    /// The `(indicator, track)` fill colors: themed `colors.primary`/
    /// `colors.primary_container`, or the unthemed [`LINEAR_FILL`]/
    /// [`LINEAR_TRACK`] constants.
    fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
        match theme {
            Some(theme) => (theme.scheme().primary, theme.scheme().primary_container),
            None => (LINEAR_FILL, LINEAR_TRACK),
        }
    }
}

impl Widget for LinearProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        bc.constrain(Size::new(width, LINEAR_TRACK_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (indicator, track) = Self::resolve_colors(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let radius = size.height / 2.0;

        scene.fill_rounded_rect(origin, size, radius, track);

        let wavy = self.wave.amplitude > 0.0;
        let phase = if wavy {
            self.wave_phase.advance(ctx.frame_time());
            self.wave_phase.value_clamped()
        } else {
            0.0
        };

        match self.value {
            ProgressValue::Determinate(_) => {
                let frac = self.value.determinate_fraction().unwrap_or(0.0);
                let fill_width = size.width * frac;
                if fill_width > 0.0 {
                    if wavy {
                        let path = linear_wave_path(
                            origin,
                            fill_width,
                            size.height,
                            self.wave.amplitude,
                            self.wave.wavelength,
                            phase,
                        );
                        scene.stroke_path(
                            Point::ZERO,
                            &path,
                            size.height,
                            &Brush::Solid(indicator),
                        );
                    } else {
                        scene.fill_rounded_rect(
                            origin,
                            Size::new(fill_width, size.height),
                            radius,
                            indicator,
                        );
                    }
                }
            }
            ProgressValue::Indeterminate => {
                self.indeterminate.advance(ctx.frame_time());
                let t = self.indeterminate.value_clamped();
                let segment_w = size.width * LINEAR_INDETERMINATE_SEGMENT_FRACTION;
                // Sweep the segment's leading edge from off the left edge to
                // off the right edge, so it visibly enters and exits the
                // track (see the module docs' Indeterminate motion note).
                let travel = size.width + segment_w;
                let x = -segment_w + t * travel;
                let clipped_x = x.max(0.0);
                let clipped_w = (x + segment_w).min(size.width) - clipped_x;
                if clipped_w > 0.0 {
                    if wavy {
                        let path = linear_wave_path(
                            Point::new(origin.x + clipped_x, origin.y),
                            clipped_w,
                            size.height,
                            self.wave.amplitude,
                            self.wave.wavelength,
                            phase,
                        );
                        scene.stroke_path(
                            Point::ZERO,
                            &path,
                            size.height,
                            &Brush::Solid(indicator),
                        );
                    } else {
                        scene.fill_rounded_rect(
                            Point::new(origin.x + clipped_x, origin.y),
                            Size::new(clipped_w, size.height),
                            radius,
                            indicator,
                        );
                    }
                }
                ctx.request_frame();
            }
        }

        if wavy {
            ctx.request_frame();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ProgressIndicator, |node| {
            // A determinate indicator reports its numeric value/range; an
            // indeterminate one leaves numeric_value unset entirely — the
            // ARIA/accesskit convention for "progress with no known
            // completion fraction" (no min/max/value = indeterminate).
            if let Some(frac) = self.value.determinate_fraction() {
                node.set_numeric_value(frac);
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(1.0);
            }
        });
    }
}

// -- Circular ------------------------------------------------------------

/// Circular indicator's outer diameter, in logical px (source: androidx
/// `ProgressIndicator.md`'s default `app:indicatorSize` — see the
/// [module docs](self)).
const CIRCULAR_DIAMETER: f64 = 40.0;
/// Circular indicator's stroke width, in logical px (same source as
/// [`CIRCULAR_DIAMETER`]).
const CIRCULAR_STROKE: f64 = 4.0;

/// The arc's starting angle: straight up (12 o'clock), matching
/// `kurbo::Arc`'s convention (0 = positive x-axis, positive = clockwise in a
/// y-down space — see `forgekit-scene::arc_path`'s docs) rotated a quarter
/// turn counter-clockwise from the positive x-axis.
const START_ANGLE: f64 = -PI / 2.0;

/// Default period of one full indeterminate rotation.
///
/// **Community-approximate**: a single-segment stand-in for upstream's real
/// growing/shrinking-arc indeterminate timing (see the [module docs](self)'s
/// Indeterminate motion note) — not a cited spec value.
const CIRCULAR_INDETERMINATE_PERIOD: Duration = Duration::from_millis(1500);

/// The indeterminate spinner's fixed sweep angle (a full turn minus a quarter
/// turn — i.e. a 270° arc), in radians.
///
/// **Community-approximate**: chosen to read clearly as a rotating spinner
/// arc, matching common Material-spinner reimplementations — not a cited spec
/// value (see the [module docs](self)'s Indeterminate motion note).
const CIRCULAR_INDETERMINATE_SWEEP: f64 = 0.75 * TAU;

/// A determinate/indeterminate circular progress indicator. See the
/// [module docs](self).
pub struct CircularProgressView {
    value: ProgressValue,
    wave: WaveParams,
}

/// Create a circular progress indicator driven by `value`.
pub fn circular_progress(value: ProgressValue) -> CircularProgressView {
    CircularProgressView {
        value,
        wave: WaveParams::default(),
    }
}

/// PascalCase alias for [`circular_progress`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn CircularProgress(value: ProgressValue) -> CircularProgressView {
    circular_progress(value)
}

impl CircularProgressView {
    /// Opt into the M3 Expressive wavy active track at the
    /// [`WAVY_DEFAULT_AMPLITUDE`]/[`WAVY_DEFAULT_WAVELENGTH`]/
    /// [`WAVY_DEFAULT_PERIOD`] defaults — see the [module docs](self)'s Wavy
    /// variants note.
    pub fn wavy(mut self) -> Self {
        self.wave.amplitude = WAVY_DEFAULT_AMPLITUDE;
        self
    }

    /// Set the wave crest amplitude, in logical px. A value `<= 0.0`
    /// degenerates the render back to the flat arc (see the
    /// [module docs](self)'s Wavy variants note); any positive value opts in
    /// even without calling [`Self::wavy`] first.
    pub fn wave_amplitude(mut self, amplitude: f64) -> Self {
        self.wave.amplitude = amplitude;
        self
    }

    /// Set the wave's crest-to-crest spacing, interpreted as an arc-length
    /// period, in logical px.
    pub fn wave_wavelength(mut self, wavelength: f64) -> Self {
        self.wave.wavelength = wavelength;
        self
    }

    /// Set `wave_speed`: the duration for the wave phase to advance one full
    /// cycle.
    pub fn wave_speed(mut self, period: Duration) -> Self {
        self.wave.period = period;
        self
    }
}

impl<State: 'static> View<State> for CircularProgressView {
    type Element = CircularProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CircularProgressWidget {
        let mut widget = CircularProgressWidget {
            value: self.value,
            wave: self.wave,
            indeterminate: AnimationController::new(CIRCULAR_INDETERMINATE_PERIOD)
                .with_curve(Curve::Linear),
            wave_phase: AnimationController::new(self.wave.period).with_curve(Curve::Linear),
        };
        if matches!(widget.value, ProgressValue::Indeterminate) {
            widget.indeterminate.repeat();
        }
        if widget.wave.amplitude > 0.0 {
            widget.wave_phase.repeat();
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CircularProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut changed = false;
        if prev.value != self.value {
            element.value = self.value;
            match self.value {
                ProgressValue::Indeterminate => {
                    if !element.indeterminate.is_animating() {
                        element.indeterminate.repeat();
                    }
                }
                ProgressValue::Determinate(_) => element.indeterminate.stop(),
            }
            changed = true;
        }
        if prev.wave != self.wave {
            element.wave = self.wave;
            // See LinearProgressView::rebuild's matching comment: a
            // period change resets the phase controller rather than retuning
            // it in place.
            element.wave_phase =
                AnimationController::new(self.wave.period).with_curve(Curve::Linear);
            if self.wave.amplitude > 0.0 {
                element.wave_phase.repeat();
            }
            changed = true;
        }
        if changed {
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// The retained widget for a [`CircularProgressView`]. See the [module docs](self).
pub struct CircularProgressWidget {
    value: ProgressValue,
    wave: WaveParams,
    /// Drives the indeterminate rotation loop (`0.0..=1.0`, repeating). Idle
    /// (never advanced) while [`ProgressValue::Determinate`].
    indeterminate: AnimationController,
    /// Drives the wavy active track's phase (`0.0..=1.0`, repeating). Idle
    /// while `wave.amplitude <= 0.0`.
    wave_phase: AnimationController,
}

/// Build a stroke path tracing a circular arc whose radius oscillates
/// sinusoidally with angle — the circular counterpart of
/// [`linear_wave_path`] above. `wavelength` (an arc-length period) is
/// converted to an angular period via `wavelength / radius`, so the crest
/// spacing reads consistently with a `linear_wave_path` call given the same
/// `wavelength` (see the [module docs](self)'s Wavy variants note).
fn wavy_arc_path(
    center: Point,
    radius: f64,
    start_angle: f64,
    sweep_angle: f64,
    amplitude: f64,
    wavelength: f64,
    phase: f64,
) -> BezPath {
    let angular_wavelength = (wavelength.max(1.0) / radius.max(1.0)).max(f64::EPSILON);
    let samples = ((sweep_angle.abs() / angular_wavelength) * WAVE_SAMPLES_PER_WAVELENGTH)
        .ceil()
        .clamp(2.0, 4096.0) as usize;

    let mut path = BezPath::new();
    for i in 0..=samples {
        let t = i as f64 / samples as f64;
        let angle = start_angle + sweep_angle * t;
        let r = radius + amplitude * (TAU * (angle / angular_wavelength + phase)).sin();
        let p = Point::new(center.x + r * angle.cos(), center.y + r * angle.sin());
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path
}

impl CircularProgressWidget {
    /// The indicator stroke color: themed `colors.primary`, or the unthemed
    /// [`LINEAR_FILL`] constant (circular shares linear's unthemed indicator
    /// color — the track itself is transparent by default per the
    /// [module docs](self), so there is no circular track color to resolve).
    fn resolve_color(theme: Option<&Theme>) -> Color {
        match theme {
            Some(theme) => theme.scheme().primary,
            None => LINEAR_FILL,
        }
    }
}

impl Widget for CircularProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let color = Self::resolve_color(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let radius = ((size.width.min(size.height)) - CIRCULAR_STROKE) / 2.0;
        let center = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let brush = Brush::Solid(color);

        let wavy = self.wave.amplitude > 0.0;
        let phase = if wavy {
            self.wave_phase.advance(ctx.frame_time());
            self.wave_phase.value_clamped()
        } else {
            0.0
        };

        match self.value {
            ProgressValue::Determinate(_) => {
                let frac = self.value.determinate_fraction().unwrap_or(0.0);
                if frac > 0.0 {
                    let sweep = TAU * frac;
                    let path = if wavy {
                        wavy_arc_path(
                            center,
                            radius,
                            START_ANGLE,
                            sweep,
                            self.wave.amplitude,
                            self.wave.wavelength,
                            phase,
                        )
                    } else {
                        arc_path(center, radius, START_ANGLE, sweep)
                    };
                    scene.stroke_path(Point::ZERO, &path, CIRCULAR_STROKE, &brush);
                }
            }
            ProgressValue::Indeterminate => {
                self.indeterminate.advance(ctx.frame_time());
                let t = self.indeterminate.value_clamped();
                let start = START_ANGLE + t * TAU;
                let path = if wavy {
                    wavy_arc_path(
                        center,
                        radius,
                        start,
                        CIRCULAR_INDETERMINATE_SWEEP,
                        self.wave.amplitude,
                        self.wave.wavelength,
                        phase,
                    )
                } else {
                    arc_path(center, radius, start, CIRCULAR_INDETERMINATE_SWEEP)
                };
                scene.stroke_path(Point::ZERO, &path, CIRCULAR_STROKE, &brush);
                ctx.request_frame();
            }
        }

        if wavy {
            ctx.request_frame();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ProgressIndicator, |node| {
            if let Some(frac) = self.value.determinate_fraction() {
                node.set_numeric_value(frac);
                node.set_min_numeric_value(0.0);
                node.set_max_numeric_value(1.0);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::BuildCtx;
    use kurbo::PathEl;

    fn build_linear(value: ProgressValue) -> LinearProgressWidget {
        let view = linear_progress(value);
        let mut counter = 0u64;
        <LinearProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn build_circular(value: ProgressValue) -> CircularProgressWidget {
        let view = circular_progress(value);
        let mut counter = 0u64;
        <CircularProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    // -- shared recording scene -----------------------------------------

    #[derive(Default)]
    struct RecordingScene {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(kurbo::BezPath, f64)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(
            &mut self,
            _origin: Point,
            path: &kurbo::BezPath,
            width: f64,
            _brush: &Brush,
        ) {
            self.strokes.push((path.clone(), width));
        }
    }

    fn paint_at(w: &mut dyn Widget, origin: Point, size: Size) -> RecordingScene {
        let mut ctx = PaintCtx::new(origin, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    // -- linear: determinate geometry ------------------------------------

    #[test]
    fn linear_determinate_fills_proportionally_to_value() {
        let mut w = build_linear(ProgressValue::Determinate(0.25));
        let scene = paint_at(&mut w, Point::ZERO, Size::new(200.0, LINEAR_TRACK_HEIGHT));
        // rrects[0] is the track (full width), rrects[1] is the fill.
        assert_eq!(scene.rrects.len(), 2);
        let (_, track_size, _, _) = scene.rrects[0];
        assert_eq!(track_size.width, 200.0);
        let (_, fill_size, _, _) = scene.rrects[1];
        assert_eq!(fill_size.width, 50.0, "25% of 200 = 50");
    }

    #[test]
    fn linear_determinate_clamps_out_of_range_values() {
        let mut over = build_linear(ProgressValue::Determinate(1.5));
        let scene = paint_at(
            &mut over,
            Point::ZERO,
            Size::new(100.0, LINEAR_TRACK_HEIGHT),
        );
        let (_, fill_size, _, _) = scene.rrects[1];
        assert_eq!(fill_size.width, 100.0, "clamped to 1.0");

        let mut under = build_linear(ProgressValue::Determinate(-0.5));
        let scene = paint_at(
            &mut under,
            Point::ZERO,
            Size::new(100.0, LINEAR_TRACK_HEIGHT),
        );
        // No fill rect painted at all for a clamped-to-zero fraction.
        assert_eq!(scene.rrects.len(), 1, "only the track, no zero-width fill");
    }

    #[test]
    fn linear_determinate_paints_zero_progress_with_no_fill() {
        let mut w = build_linear(ProgressValue::Determinate(0.0));
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
        assert_eq!(scene.rrects.len(), 1);
    }

    // -- linear: indeterminate requests frames continuously ---------------

    #[test]
    fn linear_indeterminate_requests_a_frame_every_paint() {
        // `PaintCtx::new` in a bare-core test defaults to `FrameTime::ZERO`
        // (no shell clock threaded in); `AnimationController::repeat`'s drive
        // still reports "still animating" on every `advance` regardless of
        // elapsed time, so `request_frame` fires on every paint call — the
        // "requests frames continuously" contract this test asserts.
        let mut w = build_linear(ProgressValue::Indeterminate);
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(
                ctx.needs_frame(),
                "indeterminate must keep requesting frames"
            );
        }
    }

    // -- controlled contract: never self-mutates --------------------------

    #[test]
    fn rebuild_reconciles_the_view_supplied_value() {
        let view = linear_progress(ProgressValue::Determinate(0.1));
        let mut w = build_linear(ProgressValue::Determinate(0.1));
        let next = linear_progress(ProgressValue::Determinate(0.9));
        let mut counter = 0u64;
        let flags = <LinearProgressView as View<()>>::rebuild(
            &next,
            &view,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(flags.contains(ChangeFlags::PAINT));
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
        let (_, fill_size, _, _) = scene.rrects[1];
        assert_eq!(
            fill_size.width, 90.0,
            "widget reflects the new view value, not a self-mutated one"
        );
    }

    // -- linear: wavy -------------------------------------------------------

    fn build_linear_view(view: LinearProgressView) -> LinearProgressWidget {
        let mut counter = 0u64;
        <LinearProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn linear_wavy_at_zero_amplitude_degenerates_to_the_flat_variant() {
        let mut flat = build_linear(ProgressValue::Determinate(0.4));
        let flat_scene = paint_at(
            &mut flat,
            Point::ZERO,
            Size::new(200.0, LINEAR_TRACK_HEIGHT),
        );

        let view = linear_progress(ProgressValue::Determinate(0.4))
            .wavy()
            .wave_amplitude(0.0);
        let mut wavy = build_linear_view(view);
        let wavy_scene = paint_at(
            &mut wavy,
            Point::ZERO,
            Size::new(200.0, LINEAR_TRACK_HEIGHT),
        );

        assert_eq!(
            flat_scene.rrects, wavy_scene.rrects,
            "amplitude 0 must paint byte-for-byte the same rects as the flat variant"
        );
        assert!(
            wavy_scene.strokes.is_empty(),
            "amplitude 0 must not emit a wave stroke"
        );
    }

    #[test]
    fn linear_wave_params_reach_the_render_state() {
        let view = linear_progress(ProgressValue::Determinate(0.5))
            .wave_amplitude(6.0)
            .wave_wavelength(30.0)
            .wave_speed(Duration::from_millis(900));
        let w = build_linear_view(view);
        assert_eq!(w.wave.amplitude, 6.0);
        assert_eq!(w.wave.wavelength, 30.0);
        assert_eq!(w.wave.period, Duration::from_millis(900));
    }

    #[test]
    fn linear_wavy_determinate_strokes_a_wave_instead_of_filling_a_rect() {
        let view = linear_progress(ProgressValue::Determinate(0.5)).wavy();
        let mut w = build_linear_view(view);
        let scene = paint_at(&mut w, Point::ZERO, Size::new(200.0, LINEAR_TRACK_HEIGHT));
        assert_eq!(
            scene.rrects.len(),
            1,
            "only the flat inactive track rect; the active fill is now a stroke"
        );
        assert_eq!(scene.strokes.len(), 1);
    }

    #[test]
    fn linear_wavy_determinate_clamps_the_wave_to_the_clamped_fraction() {
        let over = linear_progress(ProgressValue::Determinate(1.5)).wavy();
        let mut w = build_linear_view(over);
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
        assert_eq!(scene.strokes.len(), 1);
        let (path, _) = &scene.strokes[0];
        match path.elements().last() {
            Some(PathEl::LineTo(p)) => {
                assert!(
                    (p.x - 100.0).abs() < 1e-9,
                    "an out-of-range value still clamps the wave's rightmost sample to the track width"
                );
            }
            other => panic!("expected the wave path's last element to be a LineTo, got {other:?}"),
        }

        let under = linear_progress(ProgressValue::Determinate(-0.5)).wavy();
        let mut w = build_linear_view(under);
        let scene = paint_at(&mut w, Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
        assert_eq!(
            scene.strokes.len(),
            0,
            "clamped-to-zero fraction paints no wave stroke, mirroring the flat variant"
        );
    }

    #[test]
    fn linear_wavy_indeterminate_still_requests_frames() {
        // Mirrors `linear_indeterminate_requests_a_frame_every_paint`: at the
        // bare-core `FrameTime::ZERO` default, the sweeping segment starts
        // fully off-track (no fill/stroke on the very first paint either
        // way — see that test's comment), so this only asserts the shared
        // "keeps requesting frames while wavy" contract, not a specific
        // stroke count.
        let view = linear_progress(ProgressValue::Indeterminate).wavy();
        let mut w = build_linear_view(view);
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, LINEAR_TRACK_HEIGHT));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(
                ctx.needs_frame(),
                "a wavy indeterminate indicator must keep requesting frames"
            );
        }
    }

    // -- circular: determinate geometry -----------------------------------

    #[test]
    fn circular_determinate_sweep_is_proportional_to_value() {
        let mut half = build_circular(ProgressValue::Determinate(0.5));
        let scene = paint_at(
            &mut half,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert_eq!(scene.strokes.len(), 1);
        let (path, width) = &scene.strokes[0];
        assert_eq!(*width, CIRCULAR_STROKE);
        // A half sweep (PI radians) produces more curve segments than a
        // quarter sweep at the same tolerance.
        let mut quarter = build_circular(ProgressValue::Determinate(0.25));
        let quarter_scene = paint_at(
            &mut quarter,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        let (quarter_path, _) = &quarter_scene.strokes[0];
        let curve_count = |p: &kurbo::BezPath| {
            p.elements()
                .iter()
                .filter(|el| matches!(el, PathEl::CurveTo(..) | PathEl::QuadTo(..)))
                .count()
        };
        assert!(curve_count(path) >= curve_count(quarter_path));
        assert!(!path.elements().is_empty());
    }

    #[test]
    fn circular_zero_progress_paints_no_stroke() {
        let mut w = build_circular(ProgressValue::Determinate(0.0));
        let scene = paint_at(
            &mut w,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert!(scene.strokes.is_empty());
    }

    // -- circular: indeterminate requests frames continuously -------------

    #[test]
    fn circular_indeterminate_requests_a_frame_every_paint() {
        let mut w = build_circular(ProgressValue::Indeterminate);
        for _ in 0..3 {
            let mut ctx =
                PaintCtx::new(Point::ZERO, Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(ctx.needs_frame());
            assert_eq!(
                scene.strokes.len(),
                1,
                "always paints exactly one arc segment"
            );
        }
    }

    // -- circular: wavy ------------------------------------------------------

    fn build_circular_view(view: CircularProgressView) -> CircularProgressWidget {
        let mut counter = 0u64;
        <CircularProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn circular_wavy_at_zero_amplitude_degenerates_to_the_flat_variant() {
        let mut flat = build_circular(ProgressValue::Determinate(0.5));
        let flat_scene = paint_at(
            &mut flat,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );

        let view = circular_progress(ProgressValue::Determinate(0.5))
            .wavy()
            .wave_amplitude(0.0);
        let mut wavy = build_circular_view(view);
        let wavy_scene = paint_at(
            &mut wavy,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );

        assert_eq!(flat_scene.strokes.len(), wavy_scene.strokes.len());
        assert_eq!(
            flat_scene.strokes[0], wavy_scene.strokes[0],
            "amplitude 0 must stroke byte-for-byte the same arc path as the flat variant"
        );
    }

    #[test]
    fn circular_wave_params_reach_the_render_state() {
        let view = circular_progress(ProgressValue::Determinate(0.5))
            .wave_amplitude(4.0)
            .wave_wavelength(15.0)
            .wave_speed(Duration::from_millis(1200));
        let w = build_circular_view(view);
        assert_eq!(w.wave.amplitude, 4.0);
        assert_eq!(w.wave.wavelength, 15.0);
        assert_eq!(w.wave.period, Duration::from_millis(1200));
    }

    #[test]
    fn circular_wavy_determinate_strokes_a_wavy_arc() {
        let view = circular_progress(ProgressValue::Determinate(0.5)).wavy();
        let mut w = build_circular_view(view);
        let scene = paint_at(
            &mut w,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert_eq!(scene.strokes.len(), 1);

        let flat_scene = paint_at(
            &mut build_circular(ProgressValue::Determinate(0.5)),
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert_ne!(
            scene.strokes[0].0, flat_scene.strokes[0].0,
            "a positive amplitude must produce a different path than the flat arc"
        );
    }

    #[test]
    fn circular_wavy_zero_progress_paints_no_stroke() {
        let view = circular_progress(ProgressValue::Determinate(0.0)).wavy();
        let mut w = build_circular_view(view);
        let scene = paint_at(
            &mut w,
            Point::ZERO,
            Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER),
        );
        assert!(scene.strokes.is_empty());
    }

    #[test]
    fn circular_wavy_indeterminate_still_requests_frames_and_strokes_a_wave() {
        let view = circular_progress(ProgressValue::Indeterminate).wavy();
        let mut w = build_circular_view(view);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(CIRCULAR_DIAMETER, CIRCULAR_DIAMETER));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame());
        assert_eq!(scene.strokes.len(), 1);
    }

    // -- semantics -----------------------------------------------------------
    //
    // `SemanticsCtx::new`/`finish` are `pub(crate)` to `forgekit-core` (only
    // `RenderRoot::semantics` — a public API — can produce a real
    // `SemanticsUpdate`), so these drive a single-widget tree through a real
    // `RenderRoot` rebuild + layout + semantics pass, mirroring
    // `tests/semantics_tree.rs`'s integration-test pattern at unit scale.

    fn semantics_root_node<V>(logic: impl FnMut(&mut ()) -> V) -> forgekit_core::accesskit::Node
    where
        V: View<()> + 'static,
    {
        let mut root: forgekit_core::RenderRoot<(), V> = forgekit_core::RenderRoot::new();
        let mut state = ();
        let mut logic = logic;
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        // The progress indicator is a single leaf node under the window root.
        let (_, node) = update
            .nodes
            .iter()
            .find(|(id, _)| *id != update.root)
            .expect("the progress indicator contributes exactly one node");
        node.clone()
    }

    #[test]
    fn linear_determinate_semantics_reports_numeric_value_and_range() {
        let node = semantics_root_node(|_| linear_progress(ProgressValue::Determinate(0.42)));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(node.numeric_value(), Some(0.42));
        assert_eq!(node.min_numeric_value(), Some(0.0));
        assert_eq!(node.max_numeric_value(), Some(1.0));
    }

    #[test]
    fn indeterminate_semantics_omits_numeric_value() {
        let node = semantics_root_node(|_| linear_progress(ProgressValue::Indeterminate));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(
            node.numeric_value(),
            None,
            "indeterminate carries no numeric value"
        );
    }

    #[test]
    fn circular_semantics_role_and_value() {
        let node = semantics_root_node(|_| circular_progress(ProgressValue::Determinate(0.75)));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(node.numeric_value(), Some(0.75));
    }
}
