//! The Material 3 Expressive progress indicators: linear and circular, each
//! flat or wavy, each determinate or indeterminate — the reference's four
//! named constructors over two widgets.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/progress_indicators/` (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! This module owns the `View`/`Widget` wiring, colour resolution, and every
//! paint call; [`motion`](mod@self::motion) owns the five animation
//! controllers and the curve tables that drive them, and
//! [`linear`](mod@self::linear)/[`circular`](mod@self::circular) own the
//! geometry those numbers feed.
//!
//! # Constructors: the reference's named-constructor split, kept
//!
//! `M3EProgressIndicator` ships four Dart named constructors over one widget
//! class (`m3e_progress_indicators.dart:29-100`). All four are sibling
//! functions here, so the mapping stays one-to-one:
//!
//! | Reference | Here |
//! |---|---|
//! | `M3EProgressIndicator.linear(...)` | [`linear_progress`] (alias [`LinearProgress`]) |
//! | `M3EProgressIndicator.linearWavy(...)` | [`linear_wavy_progress`] (alias [`LinearWavyProgress`]) |
//! | `M3EProgressIndicator.circular(...)` | [`circular_progress`] (alias [`CircularProgress`]) |
//! | `M3EProgressIndicator.circularWavy(...)` | [`circular_wavy_progress`] (alias [`CircularWavyProgress`]) |
//!
//! Every knob a constructor only accepts when it is meaningful — `amplitude`/
//! `amplitudeForProgress`/`wavelength`/`waveSpeed`/`trackStrokeWidth`/
//! `gapSize` on the wavy ones, `stopSize` on `linearWavy` — is a builder
//! method here instead, documented as inert where it has no use (the same
//! shape [`crate::slider`]'s port takes, for the same reason: a
//! constructor-gated builder would need a view type per variant).
//! [`LinearProgressView::wavy`]/[`CircularProgressView::wavy`] flip a flat
//! view to its wavy sibling, so `linear_progress(v).wavy()` and
//! `linear_wavy_progress(v)` are the same thing.
//!
//! # Controlled, like every other value-carrying widget here
//!
//! Both views carry the app's [`ProgressValue`] every frame —
//! `Determinate(f)` for a known `0.0..=1.0` fraction, `Indeterminate` for an
//! unknown-duration loop — and neither ever writes its own value
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics). There is no callback: a
//! progress indicator has no user interaction to report.
//!
//! # Indeterminate motion is the reference choreography, transcribed
//!
//! Indeterminate progress is **not** a single sweeping segment. Upstream runs
//! five independent animation controllers, and this port transcribes their
//! durations, curves, and timing tables exactly
//! ([`motion`](mod@self::motion) carries the per-constant citations):
//!
//! - **Linear** — one 1750 ms cycle drives *four* separately-delayed,
//!   separately-eased fractions (`firstHead`/`firstTail`/`secondHead`/
//!   `secondTail`, all on `Cubic(0.3, 0, 0.8, 0.15)`), painting **two**
//!   traveling lines with track and gaps between and around them.
//! - **Circular** — a 6000 ms global rotation (1080°) and a 4500 ms
//!   additional rotation (360°) sum into the arc's start angle, while a
//!   1300 ms controller ping-pongs the arc's own sweep between 10% and 87% of
//!   the circle. The classic and wavy circular variants share all three.
//!
//! Every controller advances from the frame clock during paint
//! (`docs/WIDGETS_CODE_STANDARDS.md`), never from stacked implicit animations
//! and never from a wall clock.
//!
//! # Wavy variants
//!
//! A wavy indicator replaces the flat active track with a sine path: linear
//! oscillates the track's centreline, circular oscillates the ring's radius.
//! Both take their amplitude as a *factor* in `0..=1` scaled by the family's
//! own peak offset (linear 3dp, circular 1.6dp), resolved in the reference's
//! precedence order — an app-supplied curve of progress, then an app-supplied
//! constant, then the default ramp, which flattens the wave entirely below
//! `0.1` and at/above `0.95` progress so it never fights the round caps.
//!
//! The phase itself never gates on progress upstream (`_needsWavePhase` is
//! `_isWavy` alone); the amplitude ramp is what stills the wave near either
//! end. This port keeps that rule and adds one documented divergence: when the
//! resolved amplitude is `0` the phase clock is not advanced and no frame is
//! requested, since a flat line's phase is unobservable and a perpetual
//! repaint for no visible motion is exactly what the paced-frame convention
//! exists to avoid.
//!
//! **Reduce-motion**: every loop here is perpetual and decorative, so it asks
//! for paced frames (`TickClass::CosmeticLoop`) and, under
//! `MotionScheme::reduce_motion`, freezes wherever it sits and stops
//! requesting frames — the crate convention, matching
//! [`crate::slider`]/[`crate::loading_indicator`].
//!
//! # Round caps are load-bearing
//!
//! Every span here is a *stroke*, not a filled rect: the reference paints all
//! of them with `StrokeCap.round`, and its gap arithmetic already budgets for
//! the half-stroke each cap adds past its endpoint (`_visualGap` inflates the
//! gap by a whole stroke; the wavy indeterminate arm insets by another half).
//! `PaintScene::stroke_line`/`stroke_path` reach `kurbo::Stroke::new`, whose
//! caps and joins are round by default — a backend that squared them would
//! shorten every span by a half stroke and reopen the gaps this module
//! deliberately closes.

mod circular;
mod linear;
mod motion;

use frust::Theme;
use frust::authoring::scene::arc_path;
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx,
    View, Widget,
};
use kurbo::{Point, Size};
use peniko::{Brush, Color};

pub use self::motion::AmplitudeCurve;

/// Unthemed active-indicator fallback (a theme resolves this from
/// `colors.primary`, the reference's `activeColor`).
const FALLBACK_ACTIVE: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Unthemed linear-track fallback — the M3 baseline light
/// `surface_container_highest`, which is the role the reference's linear
/// builder resolves its track from.
const FALLBACK_LINEAR_TRACK: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed circular-track fallback — the M3 baseline light
/// `secondary_container`, the role `M3ECircularProgressTheme.trackColor`
/// resolves.
const FALLBACK_CIRCULAR_TRACK: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);

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
    /// The clamped `0.0..=1.0` fraction, or `None` for
    /// [`ProgressValue::Indeterminate`].
    fn fraction(self) -> Option<f64> {
        match self {
            ProgressValue::Determinate(v) if v.is_nan() => Some(0.0),
            ProgressValue::Determinate(v) => Some(v.clamp(0.0, 1.0)),
            ProgressValue::Indeterminate => None,
        }
    }
}

/// Track-thickness variant of a **flat** linear indicator
/// (`M3EProgressIndicatorSize`). Inert on every other variant: the wavy linear
/// indicator sizes from its own stroke/amplitude, and the circular ones from
/// their diameter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgressSize {
    /// A 4dp track (`M3EProgressIndicatorSize.s`).
    S,
    /// An 8dp track (`M3EProgressIndicatorSize.m`), the default.
    #[default]
    M,
}

// -- Linear ---------------------------------------------------------------

/// The diffable half of a linear indicator's props — everything a `rebuild`
/// can compare by value. The amplitude curve is the one field a diff cannot
/// reach and is adopted every pass instead.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LinearProps {
    value: ProgressValue,
    wavy: bool,
    size: ProgressSize,
    color: Option<Color>,
    track_color: Option<Color>,
    stroke_width: Option<f64>,
    track_stroke_width: Option<f64>,
    gap_size: Option<f64>,
    stop_size: Option<f64>,
    amplitude: Option<f64>,
    wavelength: Option<f64>,
    wave_speed: Option<f64>,
}

impl LinearProps {
    fn new(value: ProgressValue, wavy: bool) -> Self {
        LinearProps {
            value,
            wavy,
            size: ProgressSize::default(),
            color: None,
            track_color: None,
            stroke_width: None,
            track_stroke_width: None,
            gap_size: None,
            stop_size: None,
            amplitude: None,
            wavelength: None,
            wave_speed: None,
        }
    }

    fn fraction(&self) -> Option<f64> {
        self.value.fraction()
    }

    /// Active-indicator stroke: the wavy variant's own token, or the flat
    /// variant's track height (which upstream passes as its `strokeWidth`).
    fn stroke(&self) -> f64 {
        if self.wavy {
            self.stroke_width.unwrap_or(linear::STROKE_WIDTH)
        } else {
            linear::flat_layout(self.size).track_height
        }
    }

    /// Inactive-track stroke, resolved the same way as [`Self::stroke`].
    fn track_stroke(&self) -> f64 {
        if self.wavy {
            self.track_stroke_width
                .unwrap_or(linear::TRACK_STROKE_WIDTH)
        } else {
            linear::flat_layout(self.size).track_height
        }
    }

    /// Wavelength in logical px, defaulting per determinacy the way
    /// `_buildLinearWavy` does (40 determinate, 20 indeterminate).
    fn wavelength(&self) -> f64 {
        self.wavelength.unwrap_or(if self.fraction().is_none() {
            linear::INDETERMINATE_WAVELENGTH
        } else {
            linear::DETERMINATE_WAVELENGTH
        })
    }

    /// Travel speed in logical px per second, defaulting to the wavelength —
    /// one cycle per second (`waveSpeed ?? wavelength`).
    fn wave_speed(&self) -> f64 {
        self.wave_speed.unwrap_or_else(|| self.wavelength())
    }

    fn wavy_spec(&self) -> linear::WavySpec {
        linear::WavySpec {
            stroke: self.stroke(),
            track_stroke: self.track_stroke(),
            gap: self.gap_size.unwrap_or(linear::GAP_SIZE),
            stop_size: self.stop_size.unwrap_or(linear::STOP_SIZE),
        }
    }
}

/// A linear progress indicator, flat or wavy. See the [module docs](self).
pub struct LinearProgressView {
    props: LinearProps,
    amplitude_for_progress: Option<AmplitudeCurve>,
}

/// Create a flat linear progress indicator driven by `value`
/// (`M3EProgressIndicator.linear`).
pub fn linear_progress(value: ProgressValue) -> LinearProgressView {
    LinearProgressView {
        props: LinearProps::new(value, false),
        amplitude_for_progress: None,
    }
}

/// Create a wavy linear progress indicator driven by `value`
/// (`M3EProgressIndicator.linearWavy`).
pub fn linear_wavy_progress(value: ProgressValue) -> LinearProgressView {
    LinearProgressView {
        props: LinearProps::new(value, true),
        amplitude_for_progress: None,
    }
}

/// PascalCase alias for [`linear_progress`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn LinearProgress(value: ProgressValue) -> LinearProgressView {
    linear_progress(value)
}

/// PascalCase alias for [`linear_wavy_progress`].
#[allow(non_snake_case)]
pub fn LinearWavyProgress(value: ProgressValue) -> LinearProgressView {
    linear_wavy_progress(value)
}

impl LinearProgressView {
    /// Switch to the wavy variant — the builder form of
    /// [`linear_wavy_progress`].
    pub fn wavy(mut self) -> Self {
        self.props.wavy = true;
        self
    }

    /// Track thickness variant (`M3EProgressIndicator.linearSize`). Inert on
    /// the wavy variant, which sizes from its stroke and amplitude instead.
    pub fn size(mut self, size: ProgressSize) -> Self {
        self.props.size = size;
        self
    }

    /// Active-indicator colour (`M3EProgressIndicator.color`), overriding the
    /// theme's `colors.primary`.
    pub fn color(mut self, color: Color) -> Self {
        self.props.color = Some(color);
        self
    }

    /// Inactive-track colour (`M3EProgressIndicator.trackColor`), overriding
    /// the theme's `colors.surface_container_highest`.
    pub fn track_color(mut self, color: Color) -> Self {
        self.props.track_color = Some(color);
        self
    }

    /// Active-indicator stroke thickness in logical px
    /// (`M3EProgressIndicator.strokeWidth`). Inert on the flat variant, whose
    /// stroke is its size matrix's track height.
    pub fn stroke_width(mut self, stroke: f64) -> Self {
        self.props.stroke_width = Some(stroke);
        self
    }

    /// Inactive-track stroke thickness in logical px
    /// (`M3EProgressIndicator.trackStrokeWidth`). Inert on the flat variant.
    pub fn track_stroke_width(mut self, stroke: f64) -> Self {
        self.props.track_stroke_width = Some(stroke);
        self
    }

    /// Clear space between the active indicator and the track, in logical px
    /// (`M3EProgressIndicator.gapSize`). Inert on the flat variant, which
    /// takes its gap from the size matrix.
    pub fn gap_size(mut self, gap: f64) -> Self {
        self.props.gap_size = Some(gap);
        self
    }

    /// End stop indicator diameter in logical px
    /// (`M3EProgressIndicator.stopSize`). Inert on the flat variant, which
    /// takes its dot diameter from the size matrix; the dot itself is painted
    /// by every determinate indicator and by none of the indeterminate ones.
    pub fn stop_size(mut self, size: f64) -> Self {
        self.props.stop_size = Some(size);
        self
    }

    /// Fix the wave's amplitude factor (`0..=1`,
    /// `M3EProgressIndicator.amplitude`), replacing the default end-of-track
    /// ramp. Inert unless the indicator is wavy; outranked by
    /// [`Self::amplitude_for_progress`].
    pub fn amplitude(mut self, amplitude: f64) -> Self {
        self.props.amplitude = Some(amplitude);
        self
    }

    /// Drive the wave's amplitude factor from the current progress
    /// (`M3EProgressIndicator.amplitudeForProgress`), the highest-precedence
    /// of the three amplitude sources. The return is clamped to `0..=1`.
    /// Consulted only by a **determinate** wavy indicator — upstream's
    /// indeterminate arm takes the constant (or `1`) and never calls the
    /// curve.
    pub fn amplitude_for_progress<F: Fn(f64) -> f64 + 'static>(mut self, curve: F) -> Self {
        self.amplitude_for_progress = Some(std::rc::Rc::new(curve));
        self
    }

    /// Length of one full wave cycle in logical px
    /// (`M3EProgressIndicator.wavelength`, defaulting to 40 determinate / 20
    /// indeterminate). Inert unless the indicator is wavy.
    pub fn wavelength(mut self, wavelength: f64) -> Self {
        self.props.wavelength = Some(wavelength);
        self
    }

    /// How fast the wave travels, in logical px per second
    /// (`M3EProgressIndicator.waveSpeed`, defaulting to the wavelength — one
    /// cycle per second). Inert unless the indicator is wavy.
    pub fn wave_speed(mut self, wave_speed: f64) -> Self {
        self.props.wave_speed = Some(wave_speed);
        self
    }
}

impl<State: 'static> View<State> for LinearProgressView {
    type Element = LinearProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LinearProgressWidget {
        LinearProgressWidget {
            props: self.props,
            amplitude_for_progress: self.amplitude_for_progress.clone(),
            cycle: motion::LinearClock::new(),
            wave: motion::WaveClock::new(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LinearProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // A closure a diff cannot compare: adopted every pass, like
        // `crate::slider`'s. A wavy indicator repaints every frame anyway, so
        // an adopted curve is on screen by the next one.
        element.amplitude_for_progress = self.amplitude_for_progress.clone();
        if prev.props == self.props {
            return ChangeFlags::NONE;
        }
        let resized = prev.props.wavy != self.props.wavy
            || prev.props.size != self.props.size
            || prev.props.stroke_width != self.props.stroke_width
            // A wavy indicator's box height follows its amplitude, which
            // follows its value — so a value change is layout-affecting there
            // and paint-only everywhere else.
            || (self.props.wavy && prev.props.value != self.props.value);
        element.props = self.props;
        if resized {
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else {
            ChangeFlags::PAINT
        }
    }
}

/// The retained widget for a [`LinearProgressView`]. See the
/// [module docs](self).
pub struct LinearProgressWidget {
    props: LinearProps,
    amplitude_for_progress: Option<AmplitudeCurve>,
    /// The 1750 ms two-segment indeterminate cycle. Idle while determinate.
    cycle: motion::LinearClock,
    /// The wave phase clock. Idle while flat or at zero amplitude.
    wave: motion::WaveClock,
}

impl LinearProgressWidget {
    /// The resolved amplitude factor for this frame, in the reference's own
    /// precedence order.
    fn amplitude_factor(&self) -> f64 {
        motion::amplitude_factor(
            self.props.fraction(),
            self.props.amplitude,
            self.amplitude_for_progress.as_deref(),
        )
    }

    /// The `(active, track)` colours: themed `colors.primary`/
    /// `colors.surface_container_highest`, an explicit builder override, or
    /// the unthemed fallbacks.
    fn colors(&self, theme: Option<&Theme>) -> (Color, Color) {
        (
            self.props
                .color
                .or_else(|| theme.map(|t| t.scheme().primary))
                .unwrap_or(FALLBACK_ACTIVE),
            self.props
                .track_color
                .or_else(|| theme.map(|t| t.scheme().surface_container_highest))
                .unwrap_or(FALLBACK_LINEAR_TRACK),
        )
    }
}

impl Widget for LinearProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Upstream is a full-width `SizedBox`; take every finite pixel offered
        // and fall back to the minimum under an unbounded constraint.
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        let height = if self.props.wavy {
            linear::wavy_height(self.props.stroke(), self.amplitude_factor())
        } else {
            linear::flat_layout(self.props.size).track_height
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let (active, track) = self.colors(theme);
        let size = ctx.size();
        let origin = ctx.origin();
        let fraction = self.props.fraction();
        let factor = self.amplitude_factor();
        let amplitude = if self.props.wavy {
            linear::WAVE_AMPLITUDE * factor
        } else {
            0.0
        };

        // Advance-during-paint, one clock per live loop.
        let mut animating = false;
        if fraction.is_none() {
            if !reduce_motion {
                self.cycle.step(ctx.frame_time());
            }
            animating = true;
        }
        if amplitude > 0.0 {
            if !reduce_motion {
                self.wave.step(ctx.frame_time());
            }
            animating = true;
        }
        if animating && !reduce_motion {
            ctx.request_frame_paced();
        }

        let segments = self.cycle.segments();
        let ops = if self.props.wavy {
            linear::wavy_ops(self.props.wavy_spec(), size.width, fraction, segments)
        } else {
            linear::flat_ops(
                linear::flat_layout(self.props.size),
                size.width,
                fraction,
                segments,
            )
        };

        let cy = origin.y + size.height / 2.0;
        let stroke = self.props.stroke();
        let track_stroke = self.props.track_stroke();
        let phase = self
            .wave
            .radians(self.props.wavelength(), self.props.wave_speed());
        for op in ops {
            match op {
                linear::LinearOp::Track { x0, x1 } => scene.stroke_line(
                    Point::new(origin.x + x0, cy),
                    Point::new(origin.x + x1, cy),
                    track_stroke,
                    track,
                ),
                linear::LinearOp::Active { x0, x1 } => scene.stroke_line(
                    Point::new(origin.x + x0, cy),
                    Point::new(origin.x + x1, cy),
                    stroke,
                    active,
                ),
                linear::LinearOp::Wave { x0, x1 } => {
                    if let Some(path) = linear::wave_path(
                        origin.x + x0,
                        origin.x + x1,
                        cy,
                        amplitude,
                        self.props.wavelength(),
                        phase,
                    ) {
                        scene.stroke_path(Point::ZERO, &path, stroke, &Brush::Solid(active));
                    }
                }
                linear::LinearOp::Stop { center_x, diameter } => scene.fill_rounded_rect(
                    Point::new(origin.x + center_x - diameter / 2.0, cy - diameter / 2.0),
                    Size::new(diameter, diameter),
                    diameter / 2.0,
                    active,
                ),
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        push_progress_node(ctx, self.props.fraction());
    }
}

// -- Circular -------------------------------------------------------------

/// The diffable half of a circular indicator's props. See [`LinearProps`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct CircularProps {
    value: ProgressValue,
    wavy: bool,
    diameter: Option<f64>,
    color: Option<Color>,
    track_color: Option<Color>,
    stroke_width: Option<f64>,
    track_stroke_width: Option<f64>,
    gap_size: Option<f64>,
    amplitude: Option<f64>,
    wavelength: Option<f64>,
    wave_speed: Option<f64>,
}

impl CircularProps {
    fn new(value: ProgressValue, wavy: bool) -> Self {
        CircularProps {
            value,
            wavy,
            diameter: None,
            color: None,
            track_color: None,
            stroke_width: None,
            track_stroke_width: None,
            gap_size: None,
            amplitude: None,
            wavelength: None,
            wave_speed: None,
        }
    }

    fn fraction(&self) -> Option<f64> {
        self.value.fraction()
    }

    /// Box diameter: the wavy variant is 48dp, the classic one 40dp
    /// (`circular.wavySize` / `circular.defaultSize`).
    fn diameter(&self) -> f64 {
        self.diameter.unwrap_or(if self.wavy {
            circular::WAVY_SIZE
        } else {
            circular::DEFAULT_SIZE
        })
    }

    fn stroke(&self) -> f64 {
        self.stroke_width.unwrap_or(circular::STROKE_WIDTH)
    }

    fn track_stroke(&self) -> f64 {
        self.track_stroke_width
            .unwrap_or(circular::TRACK_STROKE_WIDTH)
    }

    fn gap(&self) -> f64 {
        self.gap_size.unwrap_or(circular::GAP_SIZE)
    }

    fn wavelength(&self) -> f64 {
        self.wavelength.unwrap_or(circular::WAVELENGTH)
    }

    fn wave_speed(&self) -> f64 {
        self.wave_speed.unwrap_or_else(|| self.wavelength())
    }
}

/// A circular progress indicator, classic or wavy. See the
/// [module docs](self).
pub struct CircularProgressView {
    props: CircularProps,
    amplitude_for_progress: Option<AmplitudeCurve>,
}

/// Create a classic circular progress indicator driven by `value`
/// (`M3EProgressIndicator.circular`).
pub fn circular_progress(value: ProgressValue) -> CircularProgressView {
    CircularProgressView {
        props: CircularProps::new(value, false),
        amplitude_for_progress: None,
    }
}

/// Create a wavy circular progress indicator driven by `value`
/// (`M3EProgressIndicator.circularWavy`).
pub fn circular_wavy_progress(value: ProgressValue) -> CircularProgressView {
    CircularProgressView {
        props: CircularProps::new(value, true),
        amplitude_for_progress: None,
    }
}

/// PascalCase alias for [`circular_progress`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn CircularProgress(value: ProgressValue) -> CircularProgressView {
    circular_progress(value)
}

/// PascalCase alias for [`circular_wavy_progress`].
#[allow(non_snake_case)]
pub fn CircularWavyProgress(value: ProgressValue) -> CircularProgressView {
    circular_wavy_progress(value)
}

impl CircularProgressView {
    /// Switch to the wavy variant — the builder form of
    /// [`circular_wavy_progress`].
    pub fn wavy(mut self) -> Self {
        self.props.wavy = true;
        self
    }

    /// Outer diameter in logical px (`M3EProgressIndicator.size`), overriding
    /// the 40dp classic / 48dp wavy default.
    pub fn diameter(mut self, diameter: f64) -> Self {
        self.props.diameter = Some(diameter);
        self
    }

    /// Active-arc colour (`M3EProgressIndicator.color`), overriding the
    /// theme's `colors.primary`.
    pub fn color(mut self, color: Color) -> Self {
        self.props.color = Some(color);
        self
    }

    /// Track-arc colour (`M3EProgressIndicator.trackColor`), overriding the
    /// theme's `colors.secondary_container`.
    pub fn track_color(mut self, color: Color) -> Self {
        self.props.track_color = Some(color);
        self
    }

    /// Active-arc stroke thickness in logical px
    /// (`M3EProgressIndicator.strokeWidth`).
    pub fn stroke_width(mut self, stroke: f64) -> Self {
        self.props.stroke_width = Some(stroke);
        self
    }

    /// Track-arc stroke thickness in logical px
    /// (`M3EProgressIndicator.trackStrokeWidth`).
    pub fn track_stroke_width(mut self, stroke: f64) -> Self {
        self.props.track_stroke_width = Some(stroke);
        self
    }

    /// Clear space between the active arc and the track, in logical px
    /// (`M3EProgressIndicator.gapSize`). Applied twice — once at each end of
    /// the active arc — so the ring reads as two separated arcs.
    pub fn gap_size(mut self, gap: f64) -> Self {
        self.props.gap_size = Some(gap);
        self
    }

    /// Fix the wave's amplitude factor (`0..=1`,
    /// `M3EProgressIndicator.amplitude`). Inert unless the indicator is wavy;
    /// outranked by [`Self::amplitude_for_progress`].
    pub fn amplitude(mut self, amplitude: f64) -> Self {
        self.props.amplitude = Some(amplitude);
        self
    }

    /// Drive the wave's amplitude factor from the current progress
    /// (`M3EProgressIndicator.amplitudeForProgress`) — see
    /// [`LinearProgressView::amplitude_for_progress`] for the full contract.
    pub fn amplitude_for_progress<F: Fn(f64) -> f64 + 'static>(mut self, curve: F) -> Self {
        self.amplitude_for_progress = Some(std::rc::Rc::new(curve));
        self
    }

    /// Length of one full wave cycle along the ring, in logical px
    /// (`M3EProgressIndicator.wavelength`, defaulting to 15). Rounded to a
    /// whole number of cycles around the circumference so the wave closes on
    /// itself. Inert unless the indicator is wavy.
    pub fn wavelength(mut self, wavelength: f64) -> Self {
        self.props.wavelength = Some(wavelength);
        self
    }

    /// How fast the wave travels, in logical px per second
    /// (`M3EProgressIndicator.waveSpeed`, defaulting to the wavelength).
    /// Inert unless the indicator is wavy.
    pub fn wave_speed(mut self, wave_speed: f64) -> Self {
        self.props.wave_speed = Some(wave_speed);
        self
    }
}

impl<State: 'static> View<State> for CircularProgressView {
    type Element = CircularProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CircularProgressWidget {
        CircularProgressWidget {
            props: self.props,
            amplitude_for_progress: self.amplitude_for_progress.clone(),
            spin: motion::CircularClock::new(),
            wave: motion::WaveClock::new(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CircularProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.amplitude_for_progress = self.amplitude_for_progress.clone();
        if prev.props == self.props {
            return ChangeFlags::NONE;
        }
        let resized =
            prev.props.diameter() != self.props.diameter() || prev.props.wavy != self.props.wavy;
        element.props = self.props;
        if resized {
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else {
            ChangeFlags::PAINT
        }
    }
}

/// The retained widget for a [`CircularProgressView`]. See the
/// [module docs](self).
pub struct CircularProgressWidget {
    props: CircularProps,
    amplitude_for_progress: Option<AmplitudeCurve>,
    /// The rotation/sweep choreography. Idle while determinate.
    spin: motion::CircularClock,
    /// The wave phase clock. Idle while classic or at zero amplitude.
    wave: motion::WaveClock,
}

impl CircularProgressWidget {
    fn amplitude_factor(&self) -> f64 {
        motion::amplitude_factor(
            self.props.fraction(),
            self.props.amplitude,
            self.amplitude_for_progress.as_deref(),
        )
    }

    /// The `(active, track)` colours: themed `colors.primary`/
    /// `colors.secondary_container`, an explicit builder override, or the
    /// unthemed fallbacks.
    fn colors(&self, theme: Option<&Theme>) -> (Color, Color) {
        (
            self.props
                .color
                .or_else(|| theme.map(|t| t.scheme().primary))
                .unwrap_or(FALLBACK_ACTIVE),
            self.props
                .track_color
                .or_else(|| theme.map(|t| t.scheme().secondary_container))
                .unwrap_or(FALLBACK_CIRCULAR_TRACK),
        )
    }
}

impl Widget for CircularProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let diameter = self.props.diameter();
        bc.constrain(Size::new(diameter, diameter))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let (active, track) = self.colors(theme);
        let fraction = self.props.fraction();
        let amplitude = if self.props.wavy {
            circular::WAVE_AMPLITUDE * self.amplitude_factor()
        } else {
            0.0
        };

        let mut animating = false;
        if fraction.is_none() {
            if !reduce_motion {
                self.spin.step(ctx.frame_time());
            }
            animating = true;
        }
        if amplitude > 0.0 {
            if !reduce_motion {
                self.wave.step(ctx.frame_time());
            }
            animating = true;
        }
        if animating && !reduce_motion {
            ctx.request_frame_paced();
        }

        let Some(geometry) = circular::geometry(
            ctx.origin(),
            ctx.size(),
            self.props.stroke(),
            self.props.track_stroke(),
            self.props.gap(),
            amplitude,
            self.props.wavelength(),
        ) else {
            // A box too small to hold a positive radius paints nothing rather
            // than dividing by it — see `circular`'s divergence note.
            return;
        };

        let (rotation, sweep) = if fraction.is_none() {
            (self.spin.rotation(), self.spin.sweep())
        } else {
            (0.0, 0.0)
        };
        let spans = circular::arc_spans(
            fraction,
            circular::START_ANGLE + rotation,
            sweep,
            geometry.gap_angle,
        );

        if let Some((start, span)) = spans.track {
            let path = arc_path(geometry.center, geometry.radius, start, span);
            scene.stroke_path(
                Point::ZERO,
                &path,
                self.props.track_stroke(),
                &Brush::Solid(track),
            );
        }
        if let Some((start, span)) = spans.active {
            let path = if self.props.wavy {
                let phase = self
                    .wave
                    .radians(self.props.wavelength(), self.props.wave_speed());
                circular::wavy_arc_path(&geometry, start, span, amplitude, phase)
            } else {
                arc_path(geometry.center, geometry.radius, start, span)
            };
            scene.stroke_path(
                Point::ZERO,
                &path,
                self.props.stroke(),
                &Brush::Solid(active),
            );
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        push_progress_node(ctx, self.props.fraction());
    }
}

/// Report the indicator to the accessibility tree.
///
/// A determinate indicator reports its numeric value/range; an indeterminate
/// one leaves `numeric_value` unset entirely — the ARIA/accesskit convention
/// for "progress with no known completion fraction".
fn push_progress_node(ctx: &mut SemanticsCtx, fraction: Option<f64>) {
    ctx.push_node(Role::ProgressIndicator, |node| {
        if let Some(fraction) = fraction {
            node.set_numeric_value(fraction);
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use kurbo::{BezPath, PathEl};

    const LINEAR_WIDTH: f64 = 200.0;

    fn secs(t: f64) -> FrameTime {
        FrameTime::from_nanos((t * 1e9) as u64)
    }

    fn build_linear(view: LinearProgressView) -> LinearProgressWidget {
        let mut counter = 0u64;
        <LinearProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn build_circular(view: CircularProgressView) -> CircularProgressWidget {
        let mut counter = 0u64;
        <CircularProgressView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    // -- recording scene ----------------------------------------------------

    #[derive(Default)]
    struct RecordingScene {
        lines: Vec<(Point, Point, f64, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(BezPath, f64)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, color: Color) {
            self.lines.push((p0, p1, width, color));
        }
        fn stroke_path(&mut self, _origin: Point, path: &BezPath, width: f64, _brush: &Brush) {
            self.strokes.push((path.clone(), width));
        }
    }

    /// Paint at the widget's *current* clock state. `PaintCtx::for_test` lives
    /// behind `frust-core`'s `test-support` feature, which this crate's
    /// dev-dependency does not enable, so a clock-dependent test steps the
    /// widget's own controllers first (`crate::slider`'s convention) — a
    /// `FrameTime::ZERO` paint then yields a zero delta and leaves them where
    /// they were put.
    fn paint(w: &mut dyn Widget, size: Size) -> RecordingScene {
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        scene
    }

    fn linear_size(w: &LinearProgressWidget) -> Size {
        Size::new(LINEAR_WIDTH, w.props.stroke())
    }

    fn paint_linear(w: &mut LinearProgressWidget) -> RecordingScene {
        let size = linear_size(w);
        paint(w, size)
    }

    fn paint_circular(w: &mut CircularProgressWidget) -> RecordingScene {
        let size = circular_size(w);
        paint(w, size)
    }

    fn actives(scene: &RecordingScene, active: Color) -> Vec<(f64, f64)> {
        scene
            .lines
            .iter()
            .filter(|(_, _, _, c)| *c == active)
            .map(|(p0, p1, _, _)| (p0.x, p1.x))
            .collect()
    }

    fn tracks(scene: &RecordingScene, track: Color) -> Vec<(f64, f64)> {
        scene
            .lines
            .iter()
            .filter(|(_, _, _, c)| *c == track)
            .map(|(p0, p1, _, _)| (p0.x, p1.x))
            .collect()
    }

    // -- linear: determinate ------------------------------------------------

    #[test]
    fn linear_determinate_fills_proportionally_to_value() {
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.25)));
        let scene = paint_linear(&mut w);
        // left = 4, trackRight = 200 - 8 = 192, span = 188.
        let active = actives(&scene, FALLBACK_ACTIVE);
        assert_eq!(active.len(), 1);
        assert!((active[0].1 - (4.0 + 188.0 * 0.25)).abs() < 1e-9);
        assert_eq!(tracks(&scene, FALLBACK_LINEAR_TRACK).len(), 1);
    }

    #[test]
    fn linear_determinate_clamps_out_of_range_values() {
        let mut over = build_linear(linear_progress(ProgressValue::Determinate(1.5)));
        let scene = paint_linear(&mut over);
        assert_eq!(actives(&scene, FALLBACK_ACTIVE), vec![(4.0, 192.0)]);
        assert!(tracks(&scene, FALLBACK_LINEAR_TRACK).is_empty());

        let mut under = build_linear(linear_progress(ProgressValue::Determinate(-0.5)));
        let scene = paint_linear(&mut under);
        assert!(
            actives(&scene, FALLBACK_ACTIVE).is_empty(),
            "a clamped-to-zero fraction paints no active span"
        );
    }

    #[test]
    fn linear_determinate_paints_the_stop_indicator() {
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.5)));
        let scene = paint_linear(&mut w);
        assert_eq!(scene.rrects.len(), 1, "exactly one stop dot");
        let (origin, size, radius, color) = scene.rrects[0];
        assert_eq!(size, Size::new(4.0, 4.0));
        assert_eq!(radius, 2.0, "a rounded rect at half its side is a circle");
        assert_eq!(color, FALLBACK_ACTIVE);
        // trackStroke 8 → placement centre 192 + 4 - 2 - 2 = 192.
        assert!((origin.x + 2.0 - 192.0).abs() < 1e-9);
    }

    #[test]
    fn linear_indeterminate_paints_no_stop_indicator() {
        let mut w = build_linear(linear_progress(ProgressValue::Indeterminate));
        let scene = paint_linear(&mut w);
        assert!(scene.rrects.is_empty());
    }

    #[test]
    fn linear_flat_size_matrix_drives_the_box_height() {
        let mut small =
            build_linear(linear_progress(ProgressValue::Determinate(0.5)).size(ProgressSize::S));
        let mut medium = build_linear(linear_progress(ProgressValue::Determinate(0.5)));
        let bc = BoxConstraints::new(Size::ZERO, Size::new(LINEAR_WIDTH, 100.0));
        assert_eq!(small.layout(&mut LayoutCtx::new(), &bc).height, 4.0);
        assert_eq!(medium.layout(&mut LayoutCtx::new(), &bc).height, 8.0);
    }

    // -- linear: the two-segment indeterminate choreography ------------------

    #[test]
    fn linear_indeterminate_paints_two_traveling_lines_not_one() {
        let mut w = build_linear(linear_progress(ProgressValue::Indeterminate));
        w.cycle.step(secs(0.0));
        w.cycle.step(secs(1.0));
        let scene = paint_linear(&mut w);
        let active = actives(&scene, FALLBACK_ACTIVE);
        assert_eq!(
            active.len(),
            2,
            "the reference choreography runs two segments at once"
        );
        assert!(
            active[0].0 > active[1].1,
            "the leading line is painted first, with track between the two"
        );
    }

    #[test]
    fn linear_indeterminate_segment_positions_match_the_curve_table() {
        // Drive the cycle to t = 1000/1750 and check the painted spans against
        // the transcribed segment table evaluated independently.
        let mut w = build_linear(linear_progress(ProgressValue::Indeterminate));
        w.cycle.step(secs(0.0));
        w.cycle.step(secs(1.0));
        let t = 1000.0 / 1750.0;
        assert!((w.cycle.cycle() - t).abs() < 1e-9);
        let expected = motion::segments(t);

        let scene = paint_linear(&mut w);
        let active = actives(&scene, FALLBACK_ACTIVE);
        let span = 188.0;
        let to_fraction = |x: f64| (x - 4.0) / span;
        // Painted in draw order: the first (leading) line, then the second.
        assert!((to_fraction(active[0].0) - expected.first_tail).abs() < 1e-9);
        assert!((to_fraction(active[0].1) - expected.first_head).abs() < 1e-9);
        assert!((to_fraction(active[1].0) - expected.second_tail).abs() < 1e-9);
        assert!((to_fraction(active[1].1) - expected.second_head).abs() < 1e-9);
    }

    #[test]
    fn linear_indeterminate_requests_a_paced_frame_every_paint() {
        let mut w = build_linear(linear_progress(ProgressValue::Indeterminate));
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, linear_size(&w));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(ctx.needs_frame());
            assert!(
                ctx.needs_frame_paced_only(),
                "a perpetual decorative loop must be pace-able by the frame gate"
            );
        }
    }

    #[test]
    fn linear_determinate_flat_requests_no_frame_at_all() {
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.4)));
        let mut ctx = PaintCtx::new(Point::ZERO, linear_size(&w));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame(), "nothing is animating");
    }

    #[test]
    fn linear_indeterminate_reduce_motion_freezes_and_stops_requesting_frames() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build_linear(linear_progress(ProgressValue::Indeterminate));
        w.cycle.step(secs(0.0));
        w.cycle.step(secs(1.0));
        let frozen = w.cycle.cycle();
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, linear_size(&w)).with_theme(&theme);
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(!ctx.needs_frame());
        }
        assert_eq!(w.cycle.cycle(), frozen, "and the cycle stands still");
    }

    // -- linear: wavy --------------------------------------------------------

    #[test]
    fn linear_wavy_determinate_strokes_a_wave_instead_of_a_flat_line() {
        let mut w = build_linear(linear_wavy_progress(ProgressValue::Determinate(0.5)));
        let scene = paint(&mut w, Size::new(LINEAR_WIDTH, 10.0));
        assert_eq!(scene.strokes.len(), 1, "the active span is a stroked path");
        assert_eq!(
            actives(&scene, FALLBACK_ACTIVE).len(),
            0,
            "and not a flat line"
        );
        assert_eq!(tracks(&scene, FALLBACK_LINEAR_TRACK).len(), 1);
        assert_eq!(scene.rrects.len(), 1, "the stop dot still lands");
    }

    #[test]
    fn linear_wavy_flattens_near_both_ends_of_the_track() {
        // The default ramp is 0 below 0.1 and at/above 0.95, so the wave is a
        // straight line there — every sampled vertex sits on the centreline.
        for value in [0.05, 0.99] {
            let mut w = build_linear(linear_wavy_progress(ProgressValue::Determinate(value)));
            let scene = paint(&mut w, Size::new(LINEAR_WIDTH, 10.0));
            let cy = 5.0;
            for (path, _) in &scene.strokes {
                for el in path.elements() {
                    if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                        assert!(
                            (p.y - cy).abs() < 1e-9,
                            "the ramp must flatten the wave at value {value}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn linear_wavy_amplitude_overrides_beat_the_ramp() {
        let mut w =
            build_linear(linear_wavy_progress(ProgressValue::Determinate(0.99)).amplitude(1.0));
        let scene = paint(&mut w, Size::new(LINEAR_WIDTH, 10.0));
        let off_centre = scene.strokes[0]
            .0
            .elements()
            .iter()
            .any(|el| matches!(el, PathEl::LineTo(p) if (p.y - 5.0).abs() > 0.5));
        assert!(off_centre, "an explicit amplitude replaces the ramp");
    }

    #[test]
    fn linear_wavy_amplitude_curve_outranks_the_constant() {
        let mut w = build_linear(
            linear_wavy_progress(ProgressValue::Determinate(0.5))
                .amplitude(1.0)
                .amplitude_for_progress(|_| 0.0),
        );
        let scene = paint(&mut w, Size::new(LINEAR_WIDTH, 10.0));
        for el in scene.strokes[0].0.elements() {
            if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                assert!((p.y - 5.0).abs() < 1e-9, "the curve flattened the wave");
            }
        }
    }

    #[test]
    fn linear_wavy_box_height_follows_the_amplitude() {
        let bc = BoxConstraints::new(Size::ZERO, Size::new(LINEAR_WIDTH, 100.0));
        let mut waving = build_linear(linear_wavy_progress(ProgressValue::Determinate(0.5)));
        assert_eq!(
            waving.layout(&mut LayoutCtx::new(), &bc).height,
            linear::WAVY_CONTAINER_HEIGHT
        );
        let mut tall =
            build_linear(linear_wavy_progress(ProgressValue::Determinate(0.5)).stroke_width(8.0));
        assert_eq!(tall.layout(&mut LayoutCtx::new(), &bc).height, 14.0);
    }

    #[test]
    fn linear_wavy_advances_its_phase_and_freezes_under_reduce_motion() {
        let mut w = build_linear(linear_wavy_progress(ProgressValue::Determinate(0.5)));
        w.wave.step(secs(0.0));
        w.wave.step(secs(0.25));
        let moved = w.wave.seconds();
        assert!(moved > 0.0);

        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LINEAR_WIDTH, 10.0)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
        assert_eq!(w.wave.seconds(), moved, "the phase stands still");
        assert_eq!(
            scene.strokes.len(),
            1,
            "the wave is still drawn, just still"
        );
    }

    #[test]
    fn linear_wavy_at_zero_amplitude_requests_no_frame() {
        // The one documented divergence: a flat wave has no observable phase,
        // so the clock and its frame request are both skipped.
        let mut w =
            build_linear(linear_wavy_progress(ProgressValue::Determinate(0.5)).amplitude(0.0));
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LINEAR_WIDTH, 10.0));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
    }

    #[test]
    fn linear_wavy_indeterminate_runs_both_clocks() {
        let mut w = build_linear(linear_wavy_progress(ProgressValue::Indeterminate));
        w.cycle.step(secs(0.0));
        w.cycle.step(secs(1.0));
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LINEAR_WIDTH, 10.0));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(ctx.needs_frame_paced_only());
        assert_eq!(scene.strokes.len(), 2, "two traveling waves, as flat does");
    }

    // -- linear: controlled contract ----------------------------------------

    #[test]
    fn rebuild_reconciles_the_view_supplied_value() {
        let prev = linear_progress(ProgressValue::Determinate(0.1));
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.1)));
        let next = linear_progress(ProgressValue::Determinate(0.9));
        let mut counter = 0u64;
        let flags = <LinearProgressView as View<()>>::rebuild(
            &next,
            &prev,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(flags.contains(ChangeFlags::PAINT));
        assert!(
            !flags.needs_layout(),
            "a flat indicator's box does not depend on its value"
        );
        let scene = paint_linear(&mut w);
        let active = actives(&scene, FALLBACK_ACTIVE);
        assert!((active[0].1 - (4.0 + 188.0 * 0.9)).abs() < 1e-9);
    }

    #[test]
    fn rebuild_of_a_wavy_value_relayouts_because_the_box_can_change() {
        let prev = linear_wavy_progress(ProgressValue::Determinate(0.05));
        let mut w = build_linear(linear_wavy_progress(ProgressValue::Determinate(0.05)));
        let next = linear_wavy_progress(ProgressValue::Determinate(0.5));
        let mut counter = 0u64;
        let flags = <LinearProgressView as View<()>>::rebuild(
            &next,
            &prev,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(flags.needs_layout());
    }

    #[test]
    fn rebuild_with_identical_props_reports_nothing() {
        let prev = linear_progress(ProgressValue::Determinate(0.4));
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.4)));
        let next = linear_progress(ProgressValue::Determinate(0.4));
        let mut counter = 0u64;
        let flags = <LinearProgressView as View<()>>::rebuild(
            &next,
            &prev,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(flags, ChangeFlags::NONE);
    }

    // -- linear: colours -----------------------------------------------------

    #[test]
    fn linear_colours_follow_the_theme_then_an_explicit_override() {
        let theme = crate::baseline();
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.5)));
        let mut ctx = PaintCtx::new(Point::ZERO, linear_size(&w)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!actives(&scene, theme.scheme().primary).is_empty());
        assert!(!tracks(&scene, theme.scheme().surface_container_highest).is_empty());

        let pink = Color::from_rgb8(0xFF, 0x00, 0x99);
        let mut w = build_linear(linear_progress(ProgressValue::Determinate(0.5)).color(pink));
        let mut ctx = PaintCtx::new(Point::ZERO, linear_size(&w)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert!(!actives(&scene, pink).is_empty(), "an override wins");
    }

    // -- circular ------------------------------------------------------------

    fn circular_size(w: &CircularProgressWidget) -> Size {
        let d = w.props.diameter();
        Size::new(d, d)
    }

    #[test]
    fn circular_default_diameters_differ_by_variant() {
        let bc = BoxConstraints::new(Size::ZERO, Size::new(200.0, 200.0));
        let mut classic = build_circular(circular_progress(ProgressValue::Indeterminate));
        let mut wavy = build_circular(circular_wavy_progress(ProgressValue::Indeterminate));
        assert_eq!(
            classic.layout(&mut LayoutCtx::new(), &bc),
            Size::new(circular::DEFAULT_SIZE, circular::DEFAULT_SIZE)
        );
        assert_eq!(
            wavy.layout(&mut LayoutCtx::new(), &bc),
            Size::new(circular::WAVY_SIZE, circular::WAVY_SIZE)
        );
    }

    #[test]
    fn circular_determinate_paints_a_track_and_an_active_arc() {
        let mut w = build_circular(circular_progress(ProgressValue::Determinate(0.5)));
        let scene = paint_circular(&mut w);
        assert_eq!(scene.strokes.len(), 2, "track first, then the active arc");
    }

    #[test]
    fn circular_complete_is_a_single_full_ring() {
        let mut w = build_circular(circular_progress(ProgressValue::Determinate(1.0)));
        let scene = paint_circular(&mut w);
        assert_eq!(scene.strokes.len(), 1, "no track survives a complete ring");
    }

    #[test]
    fn circular_zero_progress_is_all_track() {
        let mut w = build_circular(circular_progress(ProgressValue::Determinate(0.0)));
        let scene = paint_circular(&mut w);
        assert_eq!(scene.strokes.len(), 1);
    }

    #[test]
    fn circular_indeterminate_rotation_and_sweep_match_the_clock() {
        let mut w = build_circular(circular_progress(ProgressValue::Indeterminate));
        w.spin.step(secs(0.0));
        w.spin.step(secs(0.65));
        // Pinned in `motion`: 169 deg of rotation and a 0.485-turn sweep.
        assert!((w.spin.rotation() - 169.0_f64.to_radians()).abs() < 1e-9);
        assert!((w.spin.sweep() - 0.485 * std::f64::consts::TAU).abs() < 1e-9);

        let scene = paint_circular(&mut w);
        assert_eq!(scene.strokes.len(), 2, "a rotating arc plus its track");
        // The active arc's first vertex must sit on the rotated start angle.
        let geometry = circular::geometry(
            Point::ZERO,
            circular_size(&w),
            circular::STROKE_WIDTH,
            circular::TRACK_STROKE_WIDTH,
            circular::GAP_SIZE,
            0.0,
            circular::WAVELENGTH,
        )
        .expect("a 40dp ring");
        let start = circular::START_ANGLE + w.spin.rotation();
        let expected = Point::new(
            geometry.center.x + geometry.radius * start.cos(),
            geometry.center.y + geometry.radius * start.sin(),
        );
        let Some(PathEl::MoveTo(actual)) = scene.strokes[1].0.elements().first() else {
            panic!("an arc path opens with a MoveTo");
        };
        assert!((actual.x - expected.x).abs() < 1e-6);
        assert!((actual.y - expected.y).abs() < 1e-6);
    }

    #[test]
    fn circular_indeterminate_requests_a_paced_frame_every_paint() {
        let mut w = build_circular(circular_progress(ProgressValue::Indeterminate));
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, circular_size(&w));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(ctx.needs_frame());
            assert!(ctx.needs_frame_paced_only());
        }
    }

    #[test]
    fn circular_indeterminate_reduce_motion_freezes_and_stops_requesting_frames() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build_circular(circular_progress(ProgressValue::Indeterminate));
        w.spin.step(secs(0.0));
        w.spin.step(secs(0.65));
        let frozen = w.spin.rotation();
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, circular_size(&w)).with_theme(&theme);
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert!(!ctx.needs_frame());
            assert_eq!(scene.strokes.len(), 2, "still paints a frozen ring");
        }
        assert_eq!(w.spin.rotation(), frozen);
    }

    #[test]
    fn circular_wavy_determinate_modulates_its_radius() {
        let mut w = build_circular(circular_wavy_progress(ProgressValue::Determinate(0.5)));
        let size = circular_size(&w);
        let scene = paint(&mut w, size);
        let geometry = circular::geometry(
            Point::ZERO,
            size,
            circular::STROKE_WIDTH,
            circular::TRACK_STROKE_WIDTH,
            circular::GAP_SIZE,
            circular::WAVE_AMPLITUDE,
            circular::WAVELENGTH,
        )
        .expect("a 48dp ring");
        let radii: Vec<f64> = scene.strokes[1]
            .0
            .elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some((*p - geometry.center).hypot()),
                _ => None,
            })
            .collect();
        let max = radii.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min = radii.iter().cloned().fold(f64::INFINITY, f64::min);
        assert!(
            max - min > circular::WAVE_AMPLITUDE,
            "a wavy arc must reach both a crest and a trough"
        );
    }

    #[test]
    fn circular_wavy_flattens_near_both_ends_of_the_ring() {
        let mut w = build_circular(circular_wavy_progress(ProgressValue::Determinate(0.99)));
        let size = circular_size(&w);
        let scene = paint(&mut w, size);
        let geometry = circular::geometry(
            Point::ZERO,
            size,
            circular::STROKE_WIDTH,
            circular::TRACK_STROKE_WIDTH,
            circular::GAP_SIZE,
            0.0,
            circular::WAVELENGTH,
        )
        .expect("a 48dp ring");
        for el in scene.strokes[0].0.elements() {
            if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                assert!(((*p - geometry.center).hypot() - geometry.radius).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn circular_wavy_indeterminate_paints_a_wavy_arc_and_a_flat_track() {
        let mut w = build_circular(circular_wavy_progress(ProgressValue::Indeterminate));
        w.spin.step(secs(0.0));
        w.spin.step(secs(0.65));
        let scene = paint_circular(&mut w);
        assert_eq!(scene.strokes.len(), 2);
        // The wavy helper walks a fixed 121 vertices; the track is a real arc.
        assert_eq!(scene.strokes[1].0.elements().len(), 121);
        assert!(scene.strokes[0].0.elements().len() < 121);
    }

    #[test]
    fn circular_in_a_box_too_small_for_its_stroke_paints_nothing() {
        let mut w = build_circular(circular_progress(ProgressValue::Determinate(0.5)));
        let scene = paint(&mut w, Size::new(2.0, 2.0));
        assert!(scene.strokes.is_empty());
    }

    #[test]
    fn circular_colours_follow_the_theme_then_an_explicit_override() {
        let theme = crate::baseline();
        let mut w = build_circular(circular_progress(ProgressValue::Determinate(0.5)));
        let mut ctx = PaintCtx::new(Point::ZERO, circular_size(&w)).with_theme(&theme);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.strokes.len(), 2);
        // The colour rides the brush, not the recorder's tuple, so re-resolve
        // it through the widget's own ladder instead.
        assert_eq!(
            w.colors(Some(&theme)),
            (theme.scheme().primary, theme.scheme().secondary_container)
        );
        assert_eq!(
            w.colors(None),
            (FALLBACK_ACTIVE, FALLBACK_CIRCULAR_TRACK),
            "unthemed falls back to the M3 baseline roles"
        );
    }

    // -- semantics -----------------------------------------------------------
    //
    // `SemanticsCtx::new`/`finish` are `pub(crate)` to `frust-core` (only
    // `RenderRoot::semantics` — a public API — can produce a real
    // `SemanticsUpdate`), so these drive a single-widget tree through a real
    // `RenderRoot` rebuild + layout + semantics pass.

    fn semantics_root_node<V>(logic: impl FnMut(&mut ()) -> V) -> frust::authoring::Node
    where
        V: View<()> + 'static,
    {
        let mut root: frust_core::RenderRoot<(), V> = frust_core::RenderRoot::new();
        let mut state = ();
        let mut logic = logic;
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
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
        assert_eq!(node.numeric_value(), None);
    }

    #[test]
    fn circular_semantics_role_and_value() {
        let node = semantics_root_node(|_| circular_progress(ProgressValue::Determinate(0.75)));
        assert_eq!(node.role(), Role::ProgressIndicator);
        assert_eq!(node.numeric_value(), Some(0.75));
    }
}
