//! Ports beUI's **Range Slider** family — one value, five designs.
//!
//! Sources (beUI v2, rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01), registry slug `range-slider` and its examples:
//!
//! | variant | upstream file |
//! |---|---|
//! | [`RangeSliderVariant::Default`] | `components/motion/range-slider.tsx` |
//! | [`RangeSliderVariant::Bubble`] | `components/motion/range-slider-bubble.tsx` |
//! | [`RangeSliderVariant::Fluid`] | `components/motion/range-slider-fluid.tsx` |
//! | [`RangeSliderVariant::Ruler`] | `components/motion/range-slider-ruler.tsx` |
//! | [`RangeSliderVariant::Wave`] | `components/motion/range-slider-wave.tsx` |
//!
//! All five share `lib/hooks/use-slider.ts` — the value plumbing ported here as
//! [`snap_value`] plus this widget's pointer and key handling. Only the visuals
//! and the motion differ, which is why they are one widget with a variant enum
//! rather than five.
//!
//! # Single-valued, controlled
//!
//! Upstream's hook carries **one** number (`value`/`onValueChange`), not a list
//! — a beUI range slider is a single handle, unlike the sibling shadcn
//! catalog's multi-thumb one. A drag, a press and an arrow key all *report* the
//! snapped value; nothing is written locally, so the handle moves only once the
//! app feeds the new value back down.
//!
//! # Keyboard
//!
//! `use-slider`'s map: `ArrowRight`/`ArrowUp` +1 step, `ArrowLeft`/`ArrowDown`
//! −1 step, `Home`/`End` the ends. Its `PageUp`/`PageDown` (±10 steps) are
//! **unreachable here** — `NamedKey` enumerates only the keys with editing
//! semantics and carries no page keys, so there is nothing to match on. Every
//! value those two would reach is reachable by the arrows.
//!
//! # Premise corrections
//!
//! - **The ruler has no "magnified active region".** `range-slider-ruler.tsx`
//!   is a *scale that scrolls under a fixed needle* with drag momentum and a
//!   snap-to-tick settle; its ticks differ in height (major vs minor), not in
//!   magnification. The crest that grows around the handle is the **wave**
//!   variant's.
//! - **The registry's own `wave` example points at the ruler file.**
//!   `lib/registry.ts`'s `range-slider` examples list a `wave` entry whose
//!   `file` is `components/motion/range-slider-ruler.tsx`, while
//!   `range-slider-wave.tsx` exists and is the equalizer design the entry
//!   describes. This port follows the *files*, not that mapping.
//! - **`range-slider-bubble.tsx` is not listed in the registry at all**, though
//!   it ships beside the others; it is ported here as [`RangeSliderVariant::Bubble`].
//!
//! # Degradations
//!
//! - **The ruler's edge fade is dropped.** Upstream masks the strip with
//!   `[mask-image:linear-gradient(...)]`; this scene has no gradient mask, so
//!   ticks are hard-clipped at the strip's edges instead of fading out.
//! - **The ruler's momentum is a projected target, not Motion's inertia
//!   integrator.** A release projects `velocity · power · timeConstant`
//!   ([`RULER_INERTIA_POWER`], [`RULER_TIME_CONSTANT_MS`] — upstream's own
//!   numbers), snaps that to a tick, and settles there on
//!   [`SPRING_SNAP`]; the coast and the settle are one spring rather than a
//!   decay handed to a second one.
//! - **No `PageUp`/`PageDown`.** See the keyboard note above: the framework's
//!   `NamedKey` has no page keys to match on.
//! - **Velocity is sampled per painted frame**, since an `EventCtx` carries no
//!   clock. That is what feeds the bubble's lean and the ruler's fling, and it
//!   makes both frame-rate-quantised where upstream samples per pointer event.
//! - **The wave's bars are integrated, not ramped.** Motion's spring continues
//!   from its current position *and velocity* every time a render re-issues its
//!   target, which a `from → to` ramp cannot do against a target that moves
//!   each frame. The bars therefore carry velocity and are stepped against
//!   [`SPRING_BAR`] directly — see [`WaveBars`] — which is the same system, not
//!   a substitute for it.

use std::cmp::Ordering;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, Affine, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, erase_callback_arg,
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::motion::Ramp;
use crate::press::{Lane, inside_inclusive as inside, presses};
use crate::style;
use crate::text::Label as ShapedText;
use crate::tokens::motion::{SPRING_GLIDE, SPRING_PANEL, SPRING_PRESS};
use crate::tokens::sans_family;

/// Width used when the incoming constraints are horizontally unbounded — every
/// variant is `w-full`, which has no intrinsic width.
pub const UNBOUNDED_WIDTH: f64 = 200.0;

/// The focus ring every variant paints: `focus-visible:ring-4`.
pub const SLIDER_RING_WIDTH: f64 = 4.0;
/// Ring alpha for the outset rings (`ring-foreground/30`).
pub const SLIDER_RING_ALPHA: f32 = 0.3;
/// Ring alpha for the fluid variant's inset ring (`ring-inset
/// ring-foreground/40`), which is inset because the pill's `overflow-hidden`
/// would clip an outset one away.
pub const FLUID_RING_ALPHA: f32 = 0.4;

// ---- Default variant -------------------------------------------------------

/// Track height for the default variant, in logical px (`h-10`).
pub const DEFAULT_TRACK_HEIGHT: f64 = 40.0;
/// Thumb width, in logical px (`w-1.5`).
pub const DEFAULT_THUMB_WIDTH: f64 = 6.0;
/// Thumb height, in logical px (`h-5`).
pub const DEFAULT_THUMB_HEIGHT: f64 = 20.0;
/// Tick dot diameter, in logical px (`size-1`).
pub const DEFAULT_TICK_SIZE: f64 = 4.0;
/// Alpha of the default variant's fill (`bg-foreground/15`).
pub const DEFAULT_FILL_ALPHA: f32 = 0.15;
/// Alpha of a tick dot (`bg-foreground/25`).
pub const DEFAULT_TICK_ALPHA: f32 = 0.25;
/// The most steps upstream will draw dots for before giving up on them
/// (`steps <= 50`).
pub const DEFAULT_MAX_TICKS: usize = 50;
/// How far the thumb stretches vertically while dragged
/// (`scaleY: dragging ? 1.35 : 1`).
pub const DEFAULT_THUMB_STRETCH: f64 = 1.35;

/// The default thumb's grab spring — component-local, not one of the six
/// shared: `SPRING_BOUNCY = { stiffness: 500, damping: 14, mass: 0.7 }`
/// (`range-slider.tsx`), whose comment calls it *"bouncy grab feedback for the
/// thumb scale only"*.
pub const SPRING_BOUNCY: SpringDescription = SpringDescription {
    mass: 0.7,
    stiffness: 500.0,
    damping: 14.0,
};

// ---- Bubble variant --------------------------------------------------------

/// Overall height of the bubble variant, in logical px (`h-20`).
pub const BUBBLE_HEIGHT: f64 = 80.0;
/// Horizontal padding around the bubble variant's track, in logical px
/// (`px-5`) — room for the thumb to overhang at both ends.
pub const BUBBLE_PADDING_X: f64 = 20.0;
/// Bottom padding under the bubble variant's track, in logical px (`pb-5`).
pub const BUBBLE_PADDING_BOTTOM: f64 = 20.0;
/// Track thickness for the bubble variant, in logical px (`h-2`).
pub const BUBBLE_TRACK_HEIGHT: f64 = 8.0;
/// Thumb diameter, in logical px (`size-5`).
pub const BUBBLE_THUMB_SIZE: f64 = 20.0;
/// Thumb border width, in logical px (`border-2`).
pub const BUBBLE_THUMB_BORDER: f64 = 2.0;
/// How much the thumb grows while dragged (`scale: dragging ? 1.25 : 1`).
pub const BUBBLE_THUMB_SCALE: f64 = 1.25;
/// Gap between the track's bottom and the bubble's bottom, in logical px
/// (`bottom-6`).
pub const BUBBLE_GAP: f64 = 24.0;
/// Bubble inner padding, in logical px (`px-2.5 py-1`).
pub const BUBBLE_PADDING: (f64, f64) = (10.0, 4.0);
/// The bubble's tail: a rotated square of this edge (`size-2.5`).
pub const BUBBLE_TAIL: f64 = 10.0;
/// Drag speed, in track-percent per second, that maxes out the lean and the
/// squash (`FULL_TILT = 320`).
pub const FULL_TILT: f64 = 320.0;
/// How far the bubble leans at full tilt, in degrees (`v * 16`).
pub const BUBBLE_TILT_DEGREES: f64 = 16.0;
/// How much the bubble squashes across at full tilt (`1 + |v| * 0.18`).
pub const BUBBLE_SQUASH: f64 = 0.18;
/// How much the bubble stretches along at full tilt (`1 - |v| * 0.12`).
pub const BUBBLE_STRETCH: f64 = 0.12;

/// The bubble's lean spring — component-local: `SPRING_TILT = { stiffness: 260,
/// damping: 22, mass: 0.4 }` (`range-slider-bubble.tsx`), *"loose enough that
/// the bubble keeps leaning a beat after the pointer stops"*.
pub const SPRING_TILT: SpringDescription = SpringDescription {
    mass: 0.4,
    stiffness: 260.0,
    damping: 22.0,
};

/// The bubble's exit ramp: `exit={{ …, transition: { duration: 0.12 } }}`.
const BUBBLE_EXIT: Ramp = Ramp::eased(Duration::from_millis(120), crate::tokens::motion::EASE_OUT);

// ---- Fluid variant ---------------------------------------------------------

/// Height of the fluid variant, in logical px (`h-12`).
pub const FLUID_HEIGHT: f64 = 48.0;
/// Horizontal padding inside the fluid pill, in logical px (`px-5`).
pub const FLUID_PADDING_X: f64 = 20.0;
/// How much the whole fluid pill grows while dragged
/// (`scale: dragging ? 1.03 : 1`).
pub const FLUID_DRAG_SCALE: f64 = 1.03;

// ---- Ruler variant ---------------------------------------------------------

/// Height of the ruler's tick strip, in logical px (`h-12`).
pub const RULER_STRIP_HEIGHT: f64 = 48.0;
/// Padding above the ruler's readout, in logical px (`pt-1`).
pub const RULER_READOUT_PADDING_TOP: f64 = 4.0;
/// Padding below the ruler's readout, in logical px (`pb-3`).
pub const RULER_READOUT_PADDING_BOTTOM: f64 = 12.0;
/// Default pixels between two steps (`gap = 14`).
pub const RULER_GAP: f64 = 14.0;
/// Default "label every Nth step" (`majorEvery = 5`).
pub const RULER_MAJOR_EVERY: usize = 5;
/// Major tick height, in logical px (`h-7`).
pub const RULER_MAJOR_HEIGHT: f64 = 28.0;
/// Minor tick height, in logical px (`h-3.5`).
pub const RULER_MINOR_HEIGHT: f64 = 14.0;
/// Tick width, in logical px (`w-px`).
pub const RULER_TICK_WIDTH: f64 = 1.0;
/// How far the tick marks sit above the strip's bottom, in logical px
/// (`pb-[18px]`), which is the row the labels occupy.
pub const RULER_TICK_BOTTOM: f64 = 18.0;
/// Alpha of a major tick (`bg-foreground/70`).
pub const RULER_MAJOR_ALPHA: f32 = 0.7;
/// Alpha of a minor tick (`bg-foreground/45`).
pub const RULER_MINOR_ALPHA: f32 = 0.45;
/// The needle's height, in logical px (`h-9`).
pub const RULER_NEEDLE_HEIGHT: f64 = 36.0;
/// The needle's width, in logical px (`w-[3px]`).
pub const RULER_NEEDLE_WIDTH: f64 = 3.0;
/// How far the needle sits above the strip's bottom, in logical px
/// (`bottom-5`).
pub const RULER_NEEDLE_BOTTOM: f64 = 20.0;
/// Readout type size, in logical px (`text-3xl`).
pub const RULER_READOUT_SIZE: f64 = 30.0;
/// Tick-label type size, in logical px (`text-[10px]`).
pub const RULER_LABEL_SIZE: f64 = 10.0;
/// How far past an end the strip may be dragged, as a fraction of the drag
/// (`dragElastic={0.03}`).
pub const RULER_DRAG_ELASTIC: f64 = 0.03;
/// Motion's inertia `power` for the ruler's fling (`dragTransition={{ power:
/// 0.22, … }}`).
pub const RULER_INERTIA_POWER: f64 = 0.22;
/// Motion's inertia `timeConstant` for the ruler's fling, in milliseconds
/// (`{ …, timeConstant: 320 }`).
pub const RULER_TIME_CONSTANT_MS: f64 = 320.0;

/// The ruler's settle spring — component-local: `SPRING_SNAP = { stiffness:
/// 500, damping: 40, mass: 0.6 }` (`range-slider-ruler.tsx`), *"quick, no
/// overshoot past the tick"*.
pub const SPRING_SNAP: SpringDescription = SpringDescription {
    mass: 0.6,
    stiffness: 500.0,
    damping: 40.0,
};

// ---- Wave variant ----------------------------------------------------------

/// Height of the wave variant, in logical px (`h-20`).
pub const WAVE_HEIGHT: f64 = 80.0;
/// Height of a wave bar at full scale, in logical px (`h-14`).
pub const WAVE_BAR_HEIGHT: f64 = 56.0;
/// Gap between wave bars, in logical px (`gap-1`).
pub const WAVE_BAR_GAP: f64 = 4.0;
/// Default bar count (`BARS = 32`).
pub const WAVE_BARS: usize = 32;
/// Width of the crest in bars (`SPREAD = 2.6`).
pub const WAVE_SPREAD: f64 = 2.6;
/// The scale every bar keeps regardless of the crest (`0.22 + …`).
pub const WAVE_BASE_SCALE: f64 = 0.22;
/// How far the crest lifts a bar while dragging (`… * 0.78`).
pub const WAVE_CREST_DRAGGING: f64 = 0.78;
/// How far the crest lifts a bar at rest (`… * 0.6`).
pub const WAVE_CREST_IDLE: f64 = 0.6;
/// The flat scale every bar takes under reduced motion (`scaleY: 0.4`).
pub const WAVE_REDUCED_SCALE: f64 = 0.4;
/// Per-bar delay, in milliseconds per bar of distance from the crest
/// (`delay: distance * 0.012`).
pub const WAVE_DELAY_PER_BAR_MS: f64 = 12.0;
/// The ceiling on that delay, in milliseconds (`Math.min(…, 0.12)`).
pub const WAVE_DELAY_CAP_MS: f64 = 120.0;
/// Alpha of an unfilled bar (`bg-foreground/45`), which upstream notes clears
/// the 3:1 non-text contrast floor in both themes.
pub const WAVE_UNFILLED_ALPHA: f32 = 0.45;

/// The wave's per-bar spring — component-local: `SPRING_BAR = { stiffness: 420,
/// damping: 20, mass: 0.5 }` (`range-slider-wave.tsx`), *"soft enough that the
/// crest wobbles as it travels"*.
pub const SPRING_BAR: SpringDescription = SpringDescription {
    mass: 0.5,
    stiffness: 420.0,
    damping: 20.0,
};

/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback page — the light table's `--background`.
const FALLBACK_BACKGROUND: Color = crate::BEUI_LIGHT.background;
/// Unthemed fallback muted track — the light table's `--muted`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED_FOREGROUND: Color = crate::BEUI_LIGHT.muted_foreground;

/// Round to six decimals — `use-slider`'s own `toFixed(6)`, which is what keeps
/// a fractional step from accumulating float dust into a phantom extra tick.
fn tidy(value: f64) -> f64 {
    let scale = 1e6;
    (value * scale).round() / scale
}

/// The nearest legal value on `min..=max` for `step` — a direct port of
/// `snapSliderValue` (`lib/hooks/use-slider.ts`).
///
/// `max` counts as a candidate when the step does not divide the range, so a
/// pointer near the end does not snap back onto the last whole step. An empty
/// range has exactly one legal point; a non-positive step only needs a clamp.
pub fn snap_value(next: f64, min: f64, max: f64, step: f64) -> f64 {
    // `partial_cmp` rather than `!(max > min)`: a `NaN` bound has no ordering
    // at all, and this has to answer for one without inverting a comparison
    // that is neither true nor false.
    if !matches!(max.partial_cmp(&min), Some(Ordering::Greater)) {
        return min;
    }
    if !matches!(step.partial_cmp(&0.0), Some(Ordering::Greater)) {
        return next.clamp(min, max);
    }
    let whole = tidy((max - min) / step).floor();
    let last_whole = tidy(min + whole * step);
    let to_grid = (((next - min) / step).round() * step + min).clamp(min, last_whole);
    let snapped = if last_whole < max && (next - max).abs() <= (next - to_grid).abs() {
        max
    } else {
        to_grid
    };
    tidy(snapped)
}

/// How many decimals `step` implies — `String(step).split(".")[1]?.length ?? 0`,
/// which is what makes a `0.5` scale read `72.5` and a `1` scale read `72`.
pub fn step_decimals(step: f64) -> usize {
    let text = format!("{step}");
    text.split_once('.').map_or(0, |(_, frac)| frac.len())
}

/// The five ported designs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeSliderVariant {
    /// `range-slider.tsx`: tick dots on a rounded slab, with a vertical-bar
    /// thumb that stretches as it is grabbed.
    #[default]
    Default,
    /// `range-slider-bubble.tsx`: a value bubble that pops out of the thumb on
    /// grab and leans into the direction of travel.
    Bubble,
    /// `range-slider-fluid.tsx`: no thumb — the whole pill is the control, and
    /// the label inverts wherever the fill has covered it.
    Fluid,
    /// `range-slider-ruler.tsx`: the scale scrolls under a fixed needle, with
    /// momentum and a snap onto the nearest tick.
    Ruler,
    /// `range-slider-wave.tsx`: equalizer bars that peak around the handle and
    /// fall back once it passes.
    Wave,
}

impl RangeSliderVariant {
    /// Whether this design shows text, and therefore relayouts when the value
    /// changes.
    pub fn has_readout(self) -> bool {
        matches!(
            self,
            RangeSliderVariant::Bubble | RangeSliderVariant::Fluid | RangeSliderVariant::Ruler
        )
    }
}

/// A view-held, typed change callback (erased on build).
type OnValueChange<State> = Rc<dyn Fn(&mut State, f64)>;
/// A caller-supplied readout formatter.
type Format = Rc<dyn Fn(f64) -> String>;

/// A declarative beUI range slider. See the [module docs](self).
pub struct RangeSliderView<State: 'static> {
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    variant: RangeSliderVariant,
    disabled: bool,
    show_ticks: bool,
    label: Option<String>,
    unit: Option<String>,
    bars: usize,
    ruler_gap: f64,
    ruler_major_every: usize,
    format: Option<Format>,
    on_value_change: OnValueChange<State>,
}

/// Create a slider showing `value` that reports the snapped new value through
/// `on_value_change` — a **controlled** component (see the [module docs](self)).
///
/// Defaults match `use-slider`'s own: `min = 0`, `max = 100`, `step = 1`.
pub fn range_slider<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_value_change: F,
) -> RangeSliderView<State> {
    RangeSliderView {
        value,
        min: 0.0,
        max: 100.0,
        step: 1.0,
        variant: RangeSliderVariant::Default,
        disabled: false,
        show_ticks: true,
        label: None,
        unit: None,
        bars: WAVE_BARS,
        ruler_gap: RULER_GAP,
        ruler_major_every: RULER_MAJOR_EVERY,
        format: None,
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> RangeSliderView<State> {
    /// Pick the design.
    pub fn variant(mut self, variant: RangeSliderVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the value range. An inverted or empty range collapses to `min`, the
    /// way `use-slider` collapses it.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Set the quantization step; a non-positive step means "no grid, clamp
    /// only", again `use-slider`'s own behaviour.
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Disable the control: 50% opacity and fully inert
    /// (`pointer-events-none opacity-50`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Draw a tick dot at each step ([`RangeSliderVariant::Default`]'s
    /// `showTicks`, default on).
    pub fn show_ticks(mut self, show_ticks: bool) -> Self {
        self.show_ticks = show_ticks;
        self
    }

    /// Name the control. [`RangeSliderVariant::Fluid`] paints it on the track
    /// (its `label` prop); every variant reports it to assistive tech.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the unit shown beside [`RangeSliderVariant::Ruler`]'s readout, and
    /// folded into its announcement (`"72.5 kg"`).
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Set how many bars [`RangeSliderVariant::Wave`] draws (`bars`).
    pub fn bars(mut self, bars: usize) -> Self {
        self.bars = bars.max(1);
        self
    }

    /// Set [`RangeSliderVariant::Ruler`]'s pixels-per-step and label-every-Nth
    /// (`gap`, `majorEvery`).
    pub fn ruler(mut self, gap: f64, major_every: usize) -> Self {
        self.ruler_gap = gap.max(1.0);
        self.ruler_major_every = major_every.max(1);
        self
    }

    /// Format the readout the bubble, fluid and ruler designs show (`format`).
    ///
    /// The default is the value at [`step_decimals`] places, which is what
    /// upstream's ruler does and what its bubble falls back to.
    pub fn format<F: Fn(f64) -> String + 'static>(mut self, format: F) -> Self {
        self.format = Some(Rc::new(format));
        self
    }

    /// The value clamped into the effective range.
    fn current(&self) -> f64 {
        let (lo, hi) = effective_range(self.min, self.max);
        self.value.clamp(lo, hi)
    }

    /// The readout for the current value.
    fn readout(&self) -> String {
        let value = self.current();
        match &self.format {
            Some(format) => format(value),
            None => format!("{value:.*}", step_decimals(effective_step(self.step))),
        }
    }
}

/// `use-slider`'s range collapse: an inverted or empty range has one point.
fn effective_range(min: f64, max: f64) -> (f64, f64) {
    (min, if max > min { max } else { min })
}

/// `use-slider`'s step collapse: a non-positive step strides by one.
fn effective_step(step: f64) -> f64 {
    if step > 0.0 { step } else { 1.0 }
}

/// The resolved slider palette. Every variant paints from `foreground`,
/// `background` and `muted`; nothing here is per-variant.
struct SliderColors {
    /// `bg-foreground` and every alpha derived from it.
    ink: Color,
    /// `bg-background` — the bubble's thumb fill and the fluid label's inverse.
    page: Color,
    /// `bg-muted` — the track.
    muted: Color,
    /// `text-muted-foreground` — the ruler's unit and tick labels.
    dim: Color,
}

/// Resolve the palette, falling back to the vendored light table with no theme
/// threaded.
fn resolve_colors(theme: Option<&Theme>) -> SliderColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            SliderColors {
                ink: scheme.primary,
                page: scheme.surface,
                muted: scheme.surface_container_highest,
                dim: scheme.on_surface_variant,
            }
        }
        None => SliderColors {
            ink: FALLBACK_FOREGROUND,
            page: FALLBACK_BACKGROUND,
            muted: FALLBACK_MUTED,
            dim: FALLBACK_MUTED_FOREGROUND,
        },
    }
}

/// The wave's per-bar state: a real mass-spring-damper per bar, chasing a
/// crest that each bar sees a little later than its neighbour.
///
/// A `from → to` [`Lane`] cannot express this. Upstream re-issues
/// `animate={{ scaleY: … }}` on every render and Motion's spring continues from
/// its current position *and velocity*; a ramp restarted every frame would
/// simply never leave its start. So the bars carry velocity and are integrated
/// against a moving target, which is the same system [`SPRING_BAR`] describes.
///
/// The per-bar delay (`delay: min(distance * 0.012, 0.12)`) is applied to the
/// *target*, not to the spring: bar `i` reads the head as it was `delay_i`
/// milliseconds ago, out of a short history. That is what makes the crest read
/// as a wave travelling down the track rather than as every bar breathing
/// together.
struct WaveBars {
    /// Each bar's current `scaleY`.
    scale: Vec<f64>,
    /// Each bar's velocity, in scale per second.
    velocity: Vec<f64>,
    /// Recent `(frame, head)` samples, trimmed to [`WAVE_DELAY_CAP_MS`].
    history: Vec<(FrameTime, f64)>,
    last: Option<FrameTime>,
}

/// The longest step the integrator will take in one go, in seconds — a long
/// gap (a backgrounded window) is walked in sub-steps rather than exploding a
/// stiff spring.
const WAVE_SUBSTEP_SECS: f64 = 0.008;

/// How close to its target, and how slow, a bar must be to count as settled.
const WAVE_REST_EPSILON: f64 = 1e-3;

impl WaveBars {
    /// Bars for `count` lanes, all resting at `value`.
    fn new(count: usize, value: f64) -> Self {
        Self {
            scale: vec![value; count],
            velocity: vec![0.0; count],
            history: Vec::new(),
            last: None,
        }
    }

    /// Resize to `count` bars, resting at `value`.
    fn resize(&mut self, count: usize, value: f64) {
        if self.scale.len() == count {
            return;
        }
        *self = WaveBars::new(count, value);
    }

    /// Land every bar on `value` at once and stop — the `reduce_motion` path.
    fn snap(&mut self, value: f64) {
        self.scale.fill(value);
        self.velocity.fill(0.0);
        self.history.clear();
        self.last = None;
    }

    /// The head as it was `delay_ms` before `now` — the newest sample at least
    /// that old, or the oldest one there is.
    fn delayed_head(&self, now: FrameTime, delay_ms: f64, fallback: f64) -> f64 {
        if self.history.is_empty() {
            return fallback;
        }
        let mut chosen = self.history[0].1;
        for (at, head) in &self.history {
            if now.saturating_sub(*at).as_secs_f64() * 1000.0 >= delay_ms {
                chosen = *head;
            } else {
                break;
            }
        }
        chosen
    }

    /// The scale a bar `distance` bars from the crest is heading for.
    fn target(distance: f64, lift: f64) -> f64 {
        let crest = (-(distance * distance) / (2.0 * WAVE_SPREAD * WAVE_SPREAD)).exp();
        WAVE_BASE_SCALE + crest * lift
    }

    /// Step every bar to `now` against a crest centred on `head`, returning
    /// whether anything is still moving.
    fn step(&mut self, now: FrameTime, head: f64, lift: f64) -> bool {
        self.history.push((now, head));
        self.history
            .retain(|(at, _)| now.saturating_sub(*at).as_secs_f64() * 1000.0 <= WAVE_DELAY_CAP_MS);

        let previous = self.last.replace(now);
        let dt = previous.map_or(0.0, |at| now.saturating_sub(at).as_secs_f64());
        let mass = SPRING_BAR.mass;
        let stiffness = SPRING_BAR.stiffness;
        let damping = SPRING_BAR.damping;

        let mut moving = false;
        for index in 0..self.scale.len() {
            let distance = (index as f64 - head).abs();
            let delay = (distance * WAVE_DELAY_PER_BAR_MS).min(WAVE_DELAY_CAP_MS);
            let seen = self.delayed_head(now, delay, head);
            let target = Self::target((index as f64 - seen).abs(), lift);

            let mut x = self.scale[index];
            let mut v = self.velocity[index];
            let mut remaining = dt;
            while remaining > 0.0 {
                let step = remaining.min(WAVE_SUBSTEP_SECS);
                let acceleration = (stiffness * (target - x) - damping * v) / mass;
                v += acceleration * step;
                x += v * step;
                remaining -= step;
            }
            if (target - x).abs() <= WAVE_REST_EPSILON && v.abs() <= WAVE_REST_EPSILON {
                x = target;
                v = 0.0;
            } else {
                moving = true;
            }
            self.scale[index] = x;
            self.velocity[index] = v;
        }
        moving
    }
}

/// A ruler tick: whether it is a labelled major one, how far along the strip it
/// sits, and its shaped label when it carries one.
struct RulerTick {
    major: bool,
    offset: f64,
    label: Option<ShapedText>,
}

impl<State: 'static> View<State> for RangeSliderView<State> {
    type Element = RangeSliderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RangeSliderWidget {
        let (lo, hi) = effective_range(self.min, self.max);
        let fraction = fraction_of(self.current(), lo, hi);
        RangeSliderWidget {
            value: self.current(),
            min: lo,
            max: hi,
            step: effective_step(self.step),
            variant: self.variant,
            disabled: self.disabled,
            show_ticks: self.show_ticks,
            label: self.label.clone(),
            unit: self.unit.clone(),
            bars: self.bars,
            ruler_gap: self.ruler_gap,
            ruler_major_every: self.ruler_major_every,
            pos: Lane::at_rest(Ramp::spring(SPRING_GLIDE), fraction),
            grab: Lane::at_rest(grab_ramp(self.variant), 0.0),
            bubble: Lane::at_rest(Ramp::spring(SPRING_PANEL), 0.0),
            lean: Lane::at_rest(Ramp::spring(SPRING_TILT), 0.0),
            wave: WaveBars::new(self.bars, WAVE_BASE_SCALE),
            probe: None,
            velocity: 0.0,
            dragging: false,
            drag_anchor: None,
            readout: ShapedText::new(self.readout()),
            unit_text: self.unit.as_ref().map(ShapedText::new),
            fluid_label_on_track: ShapedText::new(self.label.clone().unwrap_or_default()),
            fluid_label_on_fill: ShapedText::new(self.label.clone().unwrap_or_default()),
            fluid_value_on_track: ShapedText::new(self.readout()),
            fluid_value_on_fill: ShapedText::new(self.readout()),
            ticks: Vec::new(),
            ticks_key: None,
            size: Size::ZERO,
            on_value_change: erase_callback_arg(&self.on_value_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RangeSliderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;
        let (lo, hi) = effective_range(self.min, self.max);
        let step = effective_step(self.step);

        if prev.min != self.min || prev.max != self.max || prev.step != self.step {
            element.min = lo;
            element.max = hi;
            element.step = step;
            element.ticks_key = None;
            // The confirmed value didn't move, but its fraction within the
            // range did — re-derive `pos` against the new bounds the same
            // way a confirmed-value change does, or the painted thumb keeps
            // sitting at its old fraction of a range that no longer applies.
            let fraction = fraction_of(self.current(), lo, hi);
            if element.dragging && element.variant == RangeSliderVariant::Ruler {
                element.pos.retarget(fraction);
                element.pos.snap();
            } else {
                element.pos.retarget(fraction);
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.current() != self.current() {
            // The app is the source of truth: adopt the confirmed value and
            // glide toward it.
            element.value = self.current();
            let fraction = fraction_of(self.current(), lo, hi);
            if element.dragging && element.variant == RangeSliderVariant::Ruler {
                // The ruler's strip is under the pointer: it owns `pos` until
                // the gesture ends, so a confirmed value must not fight it.
                element.pos.retarget(fraction);
                element.pos.snap();
            } else {
                element.pos.retarget(fraction);
            }
            flags |= ChangeFlags::PAINT;
            if self.variant.has_readout() {
                flags |= ChangeFlags::LAYOUT;
            }
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            element.grab.retarget_with(grab_ramp(self.variant), 0.0);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-drag keeps no armed state behind.
                element.dragging = false;
                element.drag_anchor = None;
                element.grab.retarget(0.0);
                element.bubble.retarget(0.0);
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.show_ticks != self.show_ticks {
            element.show_ticks = self.show_ticks;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            let text = self.label.clone().unwrap_or_default();
            element.fluid_label_on_track.set_content(text.clone());
            element.fluid_label_on_fill.set_content(text);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.unit != self.unit {
            element.unit = self.unit.clone();
            match (&mut element.unit_text, &self.unit) {
                (Some(existing), Some(text)) => {
                    existing.set_content(text.clone());
                }
                (slot @ None, Some(text)) => *slot = Some(ShapedText::new(text.clone())),
                (slot, None) => *slot = None,
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.bars != self.bars {
            element.bars = self.bars;
            element.wave.resize(self.bars, WAVE_BASE_SCALE);
            flags |= ChangeFlags::PAINT;
        }
        if prev.ruler_gap != self.ruler_gap || prev.ruler_major_every != self.ruler_major_every {
            element.ruler_gap = self.ruler_gap;
            element.ruler_major_every = self.ruler_major_every;
            element.ticks_key = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // The formatter is a closure, so the rendered text is the only
        // comparable thing: re-shape when it differs.
        let readout = self.readout();
        if element.readout.set_content(readout.clone()) {
            element.fluid_value_on_track.set_content(readout.clone());
            element.fluid_value_on_fill.set_content(readout);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// The spring a variant's grab feedback rides: the default thumb's own bouncy
/// one, and [`SPRING_PRESS`] for the bubble's thumb and the fluid pill, which
/// is what each upstream file names.
fn grab_ramp(variant: RangeSliderVariant) -> Ramp {
    match variant {
        RangeSliderVariant::Default => Ramp::spring(SPRING_BOUNCY),
        _ => Ramp::spring(SPRING_PRESS),
    }
}

/// `value` as a `0.0..=1.0` fraction of `lo..=hi`.
fn fraction_of(value: f64, lo: f64, hi: f64) -> f64 {
    if hi > lo {
        ((value - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The retained widget for a [`RangeSliderView`].
pub struct RangeSliderWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    variant: RangeSliderVariant,
    disabled: bool,
    show_ticks: bool,
    label: Option<String>,
    unit: Option<String>,
    bars: usize,
    ruler_gap: f64,
    ruler_major_every: usize,
    /// The spring-smoothed handle position, `0.0` .. `1.0` — upstream's
    /// `useSpring(target, SPRING_GLIDE)`.
    pos: Lane,
    /// The grab feedback: the default thumb's `scaleY`, the bubble thumb's
    /// `scale`, the fluid pill's `scale`.
    grab: Lane,
    /// The bubble's presence, `0.0` gone .. `1.0` out.
    bubble: Lane,
    /// The bubble's signed lean, `-1.0` .. `1.0`.
    lean: Lane,
    /// The wave's per-bar scales.
    wave: WaveBars,
    /// The last painted `(fraction, frame)`, for the velocity estimate.
    probe: Option<(f64, FrameTime)>,
    /// The most recent velocity estimate, in fraction per millisecond.
    velocity: f64,
    dragging: bool,
    /// The ruler's grab: the pointer x and the fraction the strip sat at.
    drag_anchor: Option<(f64, f64)>,
    readout: ShapedText,
    unit_text: Option<ShapedText>,
    fluid_label_on_track: ShapedText,
    fluid_label_on_fill: ShapedText,
    fluid_value_on_track: ShapedText,
    fluid_value_on_fill: ShapedText,
    ticks: Vec<RulerTick>,
    /// The scale the cached ticks were built for.
    ticks_key: Option<(u64, u64, u64, usize)>,
    /// The box layout resolved, which the event pass maps a pointer into.
    size: Size,
    on_value_change: frust::authoring::ErasedArgCallback<f64>,
}

/// The ruler readout's style: `text-3xl font-semibold tabular-nums`.
fn readout_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::SEMI_BOLD,
        size: RULER_READOUT_SIZE as f32,
        color,
        ..TextStyle::default()
    }
}

/// The bubble readout's style: `text-sm font-medium tabular-nums`.
fn bubble_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: style::TEXT_SM as f32,
        color,
        ..TextStyle::default()
    }
}

/// The ruler unit's style: `text-sm text-muted-foreground`.
fn unit_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        color,
        ..TextStyle::default()
    }
}

/// A ruler tick label's style: `text-[10px] tabular-nums text-muted-foreground`.
fn tick_label_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: RULER_LABEL_SIZE as f32,
        color,
        ..TextStyle::default()
    }
}

impl RangeSliderWidget {
    /// The displayed fraction — the spring-smoothed position paint reads.
    fn fraction(&self) -> f64 {
        self.pos.value().clamp(0.0, 1.0)
    }

    /// The value the app has confirmed, as a fraction.
    fn target_fraction(&self) -> f64 {
        fraction_of(self.value, self.min, self.max)
    }

    /// Snap `value` onto the grid and report it, unless it equals the confirmed
    /// one (so holding the handle still costs nothing).
    fn commit(&mut self, ctx: &mut EventCtx, value: f64) -> bool {
        let snapped = snap_value(value, self.min, self.max, self.step);
        if snapped == self.value {
            return false;
        }
        (self.on_value_change)(ctx, snapped);
        true
    }

    /// Commit the value a local `x` maps to, through the variant's own track
    /// span — `commitFromX`.
    fn commit_from_x(&mut self, ctx: &mut EventCtx, x: f64) -> bool {
        let (start, span) = self.track_span();
        if span <= 0.0 {
            return false;
        }
        let ratio = ((x - start) / span).clamp(0.0, 1.0);
        self.commit(ctx, self.min + ratio * (self.max - self.min))
    }

    /// The `(start, width)` of the pressable track, in local x. Every design
    /// but the bubble uses its whole box; the bubble's track is inset by the
    /// padding that leaves room for the thumb to overhang.
    fn track_span(&self) -> (f64, f64) {
        match self.variant {
            RangeSliderVariant::Bubble => (
                BUBBLE_PADDING_X,
                (self.size.width - 2.0 * BUBBLE_PADDING_X).max(0.0),
            ),
            _ => (0.0, self.size.width),
        }
    }

    /// How many whole steps the range holds, and the leftover fraction of a
    /// step past the last whole one — the shape both the tick list and the
    /// ruler's strip length are derived from.
    fn span_steps(&self) -> (f64, f64) {
        let span = tidy((self.max - self.min) / self.step);
        let whole = span.floor();
        (whole, span - whole)
    }

    /// The ruler strip's full travel, in logical px.
    fn ruler_max_offset(&self) -> f64 {
        tidy((self.max - self.min) / self.step) * self.ruler_gap
    }

    /// Nudge the value by `steps` steps and report it — the arrow/page keys.
    fn nudge(&mut self, ctx: &mut EventCtx, steps: f64) -> bool {
        let next = self.value + self.step * steps;
        self.commit(ctx, next)
    }

    /// Rebuild the ruler's tick list if the scale it was built for changed.
    fn sync_ticks(&mut self) {
        let key = (
            self.min.to_bits(),
            self.max.to_bits(),
            self.step.to_bits(),
            self.ruler_major_every,
        );
        if self.ticks_key == Some(key) {
            return;
        }
        self.ticks_key = Some(key);
        self.ticks.clear();
        let (whole, remainder) = self.span_steps();
        let decimals = step_decimals(self.step);
        let count = whole.max(0.0) as usize;
        for index in 0..=count {
            let value = tidy(self.min + index as f64 * self.step);
            let major = index % self.ruler_major_every == 0;
            self.ticks.push(RulerTick {
                major,
                offset: index as f64 * self.ruler_gap,
                label: major.then(|| ShapedText::new(format!("{value:.decimals$}"))),
            });
        }
        // A range the step does not divide gets a tick of its own at `max`, so
        // the scale never runs past the value the slider can report.
        if remainder > 0.0 {
            self.ticks.push(RulerTick {
                major: true,
                offset: self.ruler_max_offset(),
                label: Some(ShapedText::new(format!("{:.decimals$}", self.max))),
            });
        }
    }
}

/// The resolved palette plus the disabled treatment, bundled so each painter
/// takes one ink rather than a colour table and two closures.
struct Ink<'a> {
    colors: &'a SliderColors,
    disabled: bool,
}

impl Ink<'_> {
    /// `color` with the disabled treatment applied.
    fn tint(&self, color: Color) -> Color {
        style::disabled_tint(color, self.disabled, style::DISABLED_OPACITY)
    }

    /// The focus ring's colour at `alpha` — every variant rings in `foreground`.
    fn ring(&self, alpha: f32) -> Color {
        self.tint(style::with_alpha(self.colors.ink, alpha))
    }
}

/// Paint a focus ring of `width` around (or just inside) a box.
fn draw_ring(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    width: f64,
    color: Color,
    inset: bool,
) {
    let offset = if inset { -width / 2.0 } else { width / 2.0 };
    let rect = Rect::from_origin_size(Point::ORIGIN, size).inset(offset);
    let ring = RoundedRect::from_rect(rect, (radius + offset).max(0.0));
    scene.stroke_path(
        origin,
        &Shape::to_path(&ring, style::PATH_TOLERANCE),
        width,
        &Brush::Solid(color),
    );
}

impl Widget for RangeSliderWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let disabled = self.disabled;
        let tint =
            move |color: Color| style::disabled_tint(color, disabled, style::DISABLED_OPACITY);

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };

        let height = match self.variant {
            RangeSliderVariant::Default => DEFAULT_TRACK_HEIGHT,
            RangeSliderVariant::Bubble => BUBBLE_HEIGHT,
            RangeSliderVariant::Wave => WAVE_HEIGHT,
            RangeSliderVariant::Fluid => {
                self.fluid_label_on_track
                    .layout(ctx, &bubble_style(tint(colors.ink)));
                self.fluid_label_on_fill
                    .layout(ctx, &bubble_style(tint(colors.page)));
                self.fluid_value_on_track
                    .layout(ctx, &bubble_style(tint(colors.ink)));
                self.fluid_value_on_fill
                    .layout(ctx, &bubble_style(tint(colors.page)));
                FLUID_HEIGHT
            }
            RangeSliderVariant::Ruler => {
                let readout = self.readout.layout(ctx, &readout_style(tint(colors.ink)));
                let unit = match &mut self.unit_text {
                    Some(text) => text.layout(ctx, &unit_style(tint(colors.dim))),
                    None => Size::ZERO,
                };
                self.sync_ticks();
                let label_style = tick_label_style(tint(colors.dim));
                for tick in &mut self.ticks {
                    if let Some(label) = &mut tick.label {
                        label.layout(ctx, &label_style);
                    }
                }
                RULER_READOUT_PADDING_TOP
                    + readout.height.max(unit.height)
                    + RULER_READOUT_PADDING_BOTTOM
                    + RULER_STRIP_HEIGHT
            }
        };

        if self.variant == RangeSliderVariant::Bubble {
            self.readout.layout(ctx, &bubble_style(tint(colors.page)));
        }

        self.size = bc.constrain(Size::new(width, height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();
        let now = ctx.frame_time();

        let mut owes_frame = false;
        if reduce {
            self.pos.retarget(self.target_fraction());
            self.pos.snap();
            self.grab.snap();
            self.bubble.snap();
            self.lean.snap();
        } else {
            owes_frame |= self.pos.advance(now);
            owes_frame |= self.grab.advance(now);
            owes_frame |= self.bubble.advance(now);
            owes_frame |= self.lean.advance(now);
        }

        // The velocity estimate every lean and fling reads, sampled per painted
        // frame because an `EventCtx` carries no clock.
        let fraction = self.fraction();
        if let Some((last, at)) = self.probe {
            let dt = now.saturating_sub(at).as_secs_f64() * 1000.0;
            if dt > 0.0 {
                self.velocity = (fraction - last) / dt;
            }
        }
        self.probe = Some((fraction, now));

        // The bubble's lean rides its own spring off that velocity.
        if !reduce {
            let percent_per_second = self.velocity * 100.0 * 1000.0;
            let target = (-percent_per_second / FULL_TILT).clamp(-1.0, 1.0);
            if (self.lean.target() - target).abs() > 1e-3 {
                self.lean.retarget(target);
                owes_frame = true;
            }
        }

        // The wave's bars chase the head, each on its own delay.
        if self.variant == RangeSliderVariant::Wave {
            self.wave.resize(self.bars, WAVE_BASE_SCALE);
            if reduce {
                self.wave.snap(WAVE_REDUCED_SCALE);
            } else {
                let head = fraction * (self.bars.max(1) as f64 - 1.0);
                let lift = if self.dragging {
                    WAVE_CREST_DRAGGING
                } else {
                    WAVE_CREST_IDLE
                };
                owes_frame |= self.wave.step(now, head, lift);
            }
        }

        let origin = ctx.origin();
        let size = ctx.size();
        let ink = Ink {
            colors: &colors,
            disabled: self.disabled,
        };

        match self.variant {
            RangeSliderVariant::Default => self.paint_default(scene, origin, size, &ink, focused),
            RangeSliderVariant::Bubble => {
                self.paint_bubble(scene, origin, size, &ink, focused, reduce)
            }
            RangeSliderVariant::Fluid => self.paint_fluid(scene, origin, size, &ink, focused),
            RangeSliderVariant::Ruler => self.paint_ruler(scene, origin, size, &ink, focused),
            RangeSliderVariant::Wave => self.paint_wave(scene, origin, size, &ink, focused),
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // `pointer-events-none` on a disabled slider: not a pointer target, and
        // `tabIndex={-1}`, so it takes no keys either.
        if self.disabled {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        match event {
            InputEvent::Key(key) => {
                let steps = match &key.key {
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowUp) => 1.0,
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowDown) => -1.0,
                    Key::Named(NamedKey::Home) => {
                        let min = self.min;
                        self.commit(ctx, min);
                        return EventResult::Handled;
                    }
                    Key::Named(NamedKey::End) => {
                        let max = self.max;
                        self.commit(ctx, max);
                        return EventResult::Handled;
                    }
                    _ => return EventResult::Ignored,
                };
                // A key press takes the scale back from any running momentum,
                // which is what keeps a coasting ruler from swallowing it.
                self.drag_anchor = None;
                self.nudge(ctx, steps);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, size) {
                        return EventResult::Ignored;
                    }
                    self.dragging = true;
                    self.grab.retarget(1.0);
                    self.bubble.retarget(1.0);
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    if self.variant == RangeSliderVariant::Ruler {
                        // The ruler is grabbed, not jumped to: the scale keeps
                        // its value and starts travelling under the needle.
                        self.drag_anchor = Some((p.position.x, self.fraction()));
                    } else {
                        self.commit_from_x(ctx, p.position.x);
                    }
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if !self.dragging {
                        if inside(p.position, size) {
                            ctx.claim_hover();
                            ctx.set_cursor(CursorIcon::Grab);
                        }
                        return EventResult::Ignored;
                    }
                    // The captured arm re-asks every move, which keeps
                    // `Grabbing` alive outside the widget's own bounds.
                    ctx.set_cursor(CursorIcon::Grabbing);
                    if let Some((anchor_x, anchor_fraction)) = self.drag_anchor {
                        let travel = self.ruler_max_offset();
                        if travel > 0.0 {
                            let raw = anchor_fraction - (p.position.x - anchor_x) / travel;
                            // `dragElastic`: past an end the strip keeps only a
                            // sliver of the drag.
                            let eased = if raw < 0.0 {
                                raw * RULER_DRAG_ELASTIC
                            } else if raw > 1.0 {
                                1.0 + (raw - 1.0) * RULER_DRAG_ELASTIC
                            } else {
                                raw
                            };
                            self.pos.retarget(eased);
                            self.pos.snap();
                            let value = self.min + eased.clamp(0.0, 1.0) * (self.max - self.min);
                            self.commit(ctx, value);
                            ctx.request_redraw();
                        }
                    } else {
                        self.commit_from_x(ctx, p.position.x);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if !self.dragging {
                        return EventResult::Ignored;
                    }
                    self.dragging = false;
                    self.grab.retarget(0.0);
                    self.bubble.retarget_with(BUBBLE_EXIT, 0.0);
                    if self.drag_anchor.take().is_some() {
                        // The fling: project where the coast would land, snap
                        // that to a tick, and let the settle spring take it.
                        let projected = self.fraction()
                            + self.velocity * RULER_INERTIA_POWER * RULER_TIME_CONSTANT_MS;
                        let value = self.min + projected.clamp(0.0, 1.0) * (self.max - self.min);
                        let snapped = snap_value(value, self.min, self.max, self.step);
                        self.pos.retarget_with(
                            Ramp::spring(SPRING_SNAP),
                            fraction_of(snapped, self.min, self.max),
                        );
                        self.commit(ctx, snapped);
                    } else {
                        self.commit_from_x(ctx, p.position.x);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if !self.dragging {
                        return EventResult::Ignored;
                    }
                    // Internal state only — a cancel keeps whatever value the
                    // app last confirmed.
                    self.dragging = false;
                    self.drag_anchor = None;
                    self.grab.retarget(0.0);
                    self.bubble.retarget_with(BUBBLE_EXIT, 0.0);
                    self.pos.retarget(self.target_fraction());
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Slider, |node| {
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
            node.set_numeric_value(self.value);
            node.set_min_numeric_value(self.min);
            node.set_max_numeric_value(self.max);
            node.set_numeric_value_step(self.step);
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
                node.add_action(Action::SetValue);
            }
        });
    }
}

/// The per-variant painting. Split off the `Widget` impl so each design reads
/// as its own transcription of its own upstream file.
impl RangeSliderWidget {
    /// `range-slider.tsx`: a rounded slab, a flat wash up to the value, tick
    /// dots on the thumb's own travel span, and a vertical-bar thumb.
    fn paint_default(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        ink: &Ink<'_>,
        focused: bool,
    ) {
        let fraction = self.fraction();
        scene.fill_rounded_rect(origin, size, style::RADIUS_LG, ink.tint(ink.colors.muted));

        // `overflow-hidden` on the track is what keeps a full fill from
        // squaring off the slab's corners.
        scene.push_clip_rounded(origin, size, style::RADIUS_LG);
        let filled = fraction * size.width;
        if filled > 0.0 {
            scene.fill_rect(
                origin,
                Size::new(filled, size.height),
                ink.tint(style::with_alpha(ink.colors.ink, DEFAULT_FILL_ALPHA)),
            );
        }
        scene.pop_clip();

        // Ticks are inset by half the thumb's width — that inset is exactly the
        // span the thumb's own centre travels, so a dot sits where it lands.
        let inset = DEFAULT_THUMB_WIDTH / 2.0;
        let span = (size.width - DEFAULT_THUMB_WIDTH).max(0.0);
        let (whole, _) = self.span_steps();
        let steps = whole.max(0.0) as usize;
        if self.show_ticks && steps > 0 && steps <= DEFAULT_MAX_TICKS && self.max > self.min {
            let dot = ink.tint(style::with_alpha(ink.colors.ink, DEFAULT_TICK_ALPHA));
            for index in 0..=steps {
                let at = (index as f64 * self.step) / (self.max - self.min);
                let cx = inset + at * span;
                scene.fill_rounded_rect(
                    Point::new(
                        origin.x + cx - DEFAULT_TICK_SIZE / 2.0,
                        origin.y + size.height / 2.0 - DEFAULT_TICK_SIZE / 2.0,
                    ),
                    Size::new(DEFAULT_TICK_SIZE, DEFAULT_TICK_SIZE),
                    DEFAULT_TICK_SIZE / 2.0,
                    dot,
                );
            }
        }

        let stretch = 1.0 + (DEFAULT_THUMB_STRETCH - 1.0) * self.grab.value().max(0.0);
        let height = DEFAULT_THUMB_HEIGHT * stretch;
        let thumb = Point::new(
            origin.x + fraction * span,
            origin.y + (size.height - height) / 2.0,
        );
        let thumb_size = Size::new(DEFAULT_THUMB_WIDTH, height);
        scene.fill_rounded_rect(
            thumb,
            thumb_size,
            style::RADIUS_SM,
            ink.tint(ink.colors.ink),
        );
        if focused {
            // `ring-inset` on the thumb, not on the track.
            draw_ring(
                scene,
                thumb,
                thumb_size,
                style::RADIUS_SM,
                SLIDER_RING_WIDTH,
                ink.ring(SLIDER_RING_ALPHA),
                true,
            );
        }
    }

    /// The bubble variant's track box, in local coordinates.
    fn bubble_track(&self, size: Size) -> Rect {
        let (start, width) = self.track_span();
        let y = size.height - BUBBLE_PADDING_BOTTOM - BUBBLE_TRACK_HEIGHT;
        Rect::from_origin_size(Point::new(start, y), Size::new(width, BUBBLE_TRACK_HEIGHT))
    }

    /// `range-slider-bubble.tsx`: a hairline pill, a round thumb that grows on
    /// grab, and a value bubble that leans into the direction of travel.
    fn paint_bubble(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        ink: &Ink<'_>,
        focused: bool,
        reduce: bool,
    ) {
        let fraction = self.fraction();
        let track = self.bubble_track(size);
        let track_origin = origin + track.origin().to_vec2();
        let radius = BUBBLE_TRACK_HEIGHT / 2.0;
        scene.fill_rounded_rect(
            track_origin,
            track.size(),
            radius,
            ink.tint(ink.colors.muted),
        );

        let filled = fraction * track.width();
        if filled > 0.0 {
            scene.push_clip_rounded(track_origin, track.size(), radius);
            scene.fill_rect(
                track_origin,
                Size::new(filled, track.height()),
                ink.tint(ink.colors.ink),
            );
            scene.pop_clip();
        }

        if focused {
            // The hit area, not the hairline, is what carries the ring:
            // `-inset-y-5 inset-x-0 rounded-full focus-visible:ring-4`.
            let hit = Rect::from_origin_size(
                Point::new(track_origin.x, track_origin.y - BUBBLE_PADDING_BOTTOM),
                Size::new(track.width(), track.height() + 2.0 * BUBBLE_PADDING_BOTTOM),
            );
            draw_ring(
                scene,
                hit.origin(),
                hit.size(),
                style::resolve_radius(style::RADIUS_CONTROL, hit.width(), hit.height()),
                SLIDER_RING_WIDTH,
                ink.ring(SLIDER_RING_ALPHA),
                false,
            );
        }

        let centre = Point::new(
            track_origin.x + fraction * track.width(),
            track_origin.y + track.height() / 2.0,
        );
        let scale = 1.0 + (BUBBLE_THUMB_SCALE - 1.0) * self.grab.value().max(0.0);
        let diameter = BUBBLE_THUMB_SIZE * scale;
        let thumb = Point::new(centre.x - diameter / 2.0, centre.y - diameter / 2.0);
        let thumb_size = Size::new(diameter, diameter);
        scene.fill_rounded_rect(thumb, thumb_size, diameter / 2.0, ink.tint(ink.colors.page));
        let border_inset = BUBBLE_THUMB_BORDER / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, thumb_size).inset(-border_inset),
            (diameter / 2.0 - border_inset).max(0.0),
        );
        scene.stroke_path(
            thumb,
            &Shape::to_path(&outline, style::PATH_TOLERANCE),
            BUBBLE_THUMB_BORDER,
            &Brush::Solid(ink.tint(ink.colors.ink)),
        );

        let presence = self.bubble.value().clamp(0.0, 1.0);
        if presence <= 0.0 {
            return;
        }
        let text = self.readout.size();
        let box_size = Size::new(
            text.width + 2.0 * BUBBLE_PADDING.0,
            text.height + 2.0 * BUBBLE_PADDING.1,
        );
        // `bottom-6` measured from the track's own bottom edge.
        let bottom = track_origin.y + track.height() - BUBBLE_GAP;
        let pivot = Point::new(centre.x, bottom);
        let box_origin = Point::new(centre.x - box_size.width / 2.0, bottom - box_size.height);

        // The entrance: `scale 0.4 → 1`, `y 10 → 0`, both about the tail.
        let enter_scale = 0.4 + 0.6 * presence;
        let rise = 10.0 * (1.0 - presence);
        // The lean: one signed value drives the rotation, the squash and the
        // stretch, all about the tail (`originY: 1`).
        let lean = if reduce {
            0.0
        } else {
            self.lean.value().clamp(-1.0, 1.0)
        };
        let tilt = (lean * BUBBLE_TILT_DEGREES).to_radians();
        let squash = 1.0 + lean.abs() * BUBBLE_SQUASH;
        let stretch = 1.0 - lean.abs() * BUBBLE_STRETCH;

        scene.push_layer(
            Point::new(
                box_origin.x - box_size.width,
                box_origin.y - box_size.height,
            ),
            Size::new(box_size.width * 3.0, box_size.height * 3.0),
            presence as f32,
        );
        scene.push_transform(
            Affine::translate((0.0, rise))
                * Affine::translate(pivot.to_vec2())
                * Affine::rotate(tilt)
                * Affine::scale_non_uniform(squash * enter_scale, stretch * enter_scale)
                * Affine::translate(-pivot.to_vec2()),
        );
        scene.fill_rounded_rect(
            box_origin,
            box_size,
            style::RADIUS_XL,
            ink.tint(ink.colors.ink),
        );
        // The tail: a square turned 45° under the bubble's bottom edge.
        let tail = Rect::from_center_size(
            Point::new(pivot.x, pivot.y),
            Size::new(BUBBLE_TAIL, BUBBLE_TAIL),
        );
        scene.push_transform(
            Affine::translate(pivot.to_vec2())
                * Affine::rotate(std::f64::consts::FRAC_PI_4)
                * Affine::translate(-pivot.to_vec2()),
        );
        scene.fill_rounded_rect(
            tail.origin(),
            tail.size(),
            style::RADIUS_SM * 0.75,
            ink.tint(ink.colors.ink),
        );
        scene.pop_transform();
        self.readout.paint(
            Point::new(
                box_origin.x + BUBBLE_PADDING.0,
                box_origin.y + BUBBLE_PADDING.1,
            ),
            scene,
        );
        scene.pop_transform();
        scene.pop_layer();
    }

    /// `range-slider-fluid.tsx`: no thumb — the whole pill is the control, and
    /// the label reads inverted wherever the fill has covered it.
    fn paint_fluid(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        ink: &Ink<'_>,
        focused: bool,
    ) {
        let fraction = self.fraction();
        let radius = style::resolve_radius(style::RADIUS_CONTROL, size.width, size.height);
        let scale = 1.0 + (FLUID_DRAG_SCALE - 1.0) * self.grab.value().max(0.0);
        let centre = origin + (size.to_vec2() / 2.0);
        scene.push_transform(
            Affine::translate(centre.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-centre.to_vec2()),
        );

        scene.fill_rounded_rect(origin, size, radius, ink.tint(ink.colors.muted));

        let label_y = origin.y + (size.height - self.fluid_label_on_track.size().height) / 2.0;
        let value_size = self.fluid_value_on_track.size();
        let value_x = origin.x + size.width - FLUID_PADDING_X - value_size.width;
        let value_y = origin.y + (size.height - value_size.height) / 2.0;
        self.fluid_label_on_track
            .paint(Point::new(origin.x + FLUID_PADDING_X, label_y), scene);
        self.fluid_value_on_track
            .paint(Point::new(value_x, value_y), scene);

        // The fill and a second copy of the same two runs, both clipped to the
        // value: the text inverts as the fill covers it and lines up glyph for
        // glyph with the copy underneath. The clip's rounded trailing edge is
        // the liquid cap.
        let filled = fraction * size.width;
        if filled > 0.0 {
            let cap = style::resolve_radius(style::RADIUS_CONTROL, filled, size.height);
            scene.push_clip_rounded(origin, Size::new(filled, size.height), cap);
            scene.fill_rounded_rect(origin, size, radius, ink.tint(ink.colors.ink));
            self.fluid_label_on_fill
                .paint(Point::new(origin.x + FLUID_PADDING_X, label_y), scene);
            self.fluid_value_on_fill
                .paint(Point::new(value_x, value_y), scene);
            scene.pop_clip();
        }

        if focused {
            // `ring-inset`: an outset ring would be clipped away by the pill's
            // own `overflow-hidden`.
            draw_ring(
                scene,
                origin,
                size,
                radius,
                SLIDER_RING_WIDTH,
                ink.ring(FLUID_RING_ALPHA),
                true,
            );
        }
        scene.pop_transform();
    }

    /// The ruler's `(readout row height, strip box)` for a laid-out size.
    fn ruler_rows(&self, size: Size) -> (f64, Rect) {
        let readout = self.readout.size();
        let unit = self.unit_text.as_ref().map_or(Size::ZERO, ShapedText::size);
        let row = readout.height.max(unit.height);
        let strip_y = RULER_READOUT_PADDING_TOP + row + RULER_READOUT_PADDING_BOTTOM;
        (
            row,
            Rect::from_origin_size(
                Point::new(0.0, strip_y),
                Size::new(size.width, (size.height - strip_y).max(0.0)),
            ),
        )
    }

    /// `range-slider-ruler.tsx`: the scale scrolls under a fixed needle.
    fn paint_ruler(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        ink: &Ink<'_>,
        focused: bool,
    ) {
        let (row, strip) = self.ruler_rows(size);
        let readout = self.readout.size();
        let unit = self.unit_text.as_ref().map_or(Size::ZERO, ShapedText::size);
        let gap = if unit.width > 0.0 {
            style::spacing(1.0)
        } else {
            0.0
        };
        let total = readout.width + gap + unit.width;
        let x = origin.x + (size.width - total) / 2.0;
        let top = origin.y + RULER_READOUT_PADDING_TOP;
        self.readout
            .paint(Point::new(x, top + (row - readout.height)), scene);
        if let Some(unit_text) = &self.unit_text {
            // `items-baseline`: the unit sits on the readout's own baseline,
            // approximated here by aligning their bottom edges.
            unit_text.paint(
                Point::new(x + readout.width + gap, top + (row - unit.height)),
                scene,
            );
        }

        let strip_origin = origin + strip.origin().to_vec2();
        let bottom = strip_origin.y + strip.height();
        scene.push_clip(strip_origin, strip.size());
        let travel = self.ruler_max_offset();
        let offset = -self.fraction() * travel;
        let centre = origin.x + size.width / 2.0;
        for tick in &self.ticks {
            let tick_x = centre + tick.offset + offset;
            if tick_x < strip_origin.x - self.ruler_gap
                || tick_x > strip_origin.x + strip.width() + self.ruler_gap
            {
                continue;
            }
            let (height, alpha) = if tick.major {
                (RULER_MAJOR_HEIGHT, RULER_MAJOR_ALPHA)
            } else {
                (RULER_MINOR_HEIGHT, RULER_MINOR_ALPHA)
            };
            scene.fill_rounded_rect(
                Point::new(
                    tick_x - RULER_TICK_WIDTH / 2.0,
                    bottom - RULER_TICK_BOTTOM - height,
                ),
                Size::new(RULER_TICK_WIDTH, height),
                RULER_TICK_WIDTH / 2.0,
                ink.tint(style::with_alpha(ink.colors.ink, alpha)),
            );
            if let Some(label) = &tick.label {
                let label_size = label.size();
                label.paint(
                    Point::new(tick_x - label_size.width / 2.0, bottom - label_size.height),
                    scene,
                );
            }
        }
        // The needle: the read head the scale travels under.
        scene.fill_rounded_rect(
            Point::new(
                centre - RULER_NEEDLE_WIDTH / 2.0,
                bottom - RULER_NEEDLE_BOTTOM - RULER_NEEDLE_HEIGHT,
            ),
            Size::new(RULER_NEEDLE_WIDTH, RULER_NEEDLE_HEIGHT),
            RULER_NEEDLE_WIDTH / 2.0,
            ink.tint(ink.colors.ink),
        );
        scene.pop_clip();

        if focused {
            draw_ring(
                scene,
                origin,
                size,
                style::RADIUS_2XL,
                SLIDER_RING_WIDTH,
                ink.ring(SLIDER_RING_ALPHA),
                false,
            );
        }
    }

    /// The wave variant's bar width for a laid-out size.
    fn wave_bar_width(&self, size: Size) -> f64 {
        let bars = self.bars.max(1) as f64;
        ((size.width - (bars - 1.0) * WAVE_BAR_GAP) / bars).max(0.0)
    }

    /// `range-slider-wave.tsx`: equalizer bars that peak around the handle.
    fn paint_wave(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        ink: &Ink<'_>,
        focused: bool,
    ) {
        let bars = self.bars.max(1);
        let bar_width = self.wave_bar_width(size);
        let head = self.fraction() * (bars as f64 - 1.0);
        let last_filled = head.round();
        let filled = ink.tint(ink.colors.ink);
        let unfilled = ink.tint(style::with_alpha(ink.colors.ink, WAVE_UNFILLED_ALPHA));
        for index in 0..bars {
            let scale = self
                .wave
                .scale
                .get(index)
                .copied()
                .unwrap_or(WAVE_BASE_SCALE)
                .max(0.0);
            let height = WAVE_BAR_HEIGHT * scale;
            let x = origin.x + index as f64 * (bar_width + WAVE_BAR_GAP);
            let y = origin.y + (size.height - height) / 2.0;
            scene.fill_rounded_rect(
                Point::new(x, y),
                Size::new(bar_width, height),
                bar_width.min(height) / 2.0,
                if (index as f64) <= last_filled {
                    filled
                } else {
                    unfilled
                },
            );
        }
        if focused {
            draw_ring(
                scene,
                origin,
                size,
                style::RADIUS_XL,
                SLIDER_RING_WIDTH,
                ink.ring(SLIDER_RING_ALPHA),
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
        scene::GlyphRun,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    /// Records the fills, clips, stroked paths, layer alphas, transform pushes
    /// and glyph origins this widget emits.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
        glyphs: Vec<Point>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, _radius: f64) {
            self.clips.push((origin, size));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
    }

    impl Recorder {
        /// The ring stroke of the pass, if any.
        fn ring(&self) -> Option<(Rect, f64, Color)> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == SLIDER_RING_WIDTH)
                .copied()
        }
    }

    #[derive(Default)]
    struct Values {
        last: Option<f64>,
        count: u32,
    }

    const WIDTH: f64 = 240.0;

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn view(value: f64, variant: RangeSliderVariant) -> RangeSliderView<Values> {
        range_slider::<Values, _>(value, |s: &mut Values, v: f64| {
            s.last = Some(v);
            s.count += 1;
        })
        .variant(variant)
        .label("volume")
    }

    /// Build + lay out a slider at [`WIDTH`], returning it with its size.
    fn laid_out(view: &RangeSliderView<Values>) -> (RangeSliderWidget, Size) {
        let mut counter = 0u64;
        let mut w = View::<Values>::build(view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 400.0)),
        );
        (w, size)
    }

    fn paint_at(
        w: &mut RangeSliderWidget,
        size: Size,
        theme: Option<&Theme>,
        millis: f64,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(millis));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    /// Paint far enough ahead that every lane has settled.
    fn paint_settled(w: &mut RangeSliderWidget, size: Size) -> Recorder {
        paint_at(w, size, None, 0.0);
        paint_at(w, size, None, 5_000.0).0
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        w: &mut RangeSliderWidget,
        size: Size,
        state: &mut Values,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    // ---- The value model --------------------------------------------------

    #[test]
    fn snap_value_walks_the_grid_and_clamps_to_the_range() {
        assert_eq!(snap_value(37.0, 0.0, 100.0, 1.0), 37.0);
        assert_eq!(snap_value(37.4, 0.0, 100.0, 5.0), 35.0);
        assert_eq!(snap_value(37.6, 0.0, 100.0, 5.0), 40.0);
        // Out of range in both directions.
        assert_eq!(snap_value(-40.0, 0.0, 100.0, 1.0), 0.0);
        assert_eq!(snap_value(400.0, 0.0, 100.0, 1.0), 100.0);
        // A fractional step does not accumulate float dust.
        assert_eq!(snap_value(0.30000000000000004, 0.0, 1.0, 0.1), 0.3);
        assert_eq!(snap_value(72.4, 40.0, 120.0, 0.5), 72.5);
    }

    #[test]
    fn a_range_the_step_does_not_divide_still_reaches_its_max() {
        // 0..10 by 4: whole ticks at 0, 4, 8 — and `max` is its own candidate,
        // so a pointer near the end lands on 10 rather than back on 8.
        assert_eq!(snap_value(9.6, 0.0, 10.0, 4.0), 10.0);
        assert_eq!(snap_value(8.4, 0.0, 10.0, 4.0), 8.0);
        assert_eq!(snap_value(10.0, 0.0, 10.0, 4.0), 10.0);
    }

    #[test]
    fn a_degenerate_range_or_step_is_clamped_rather_than_divided_by() {
        // An empty or inverted range has exactly one legal point.
        assert_eq!(snap_value(50.0, 10.0, 10.0, 1.0), 10.0);
        assert_eq!(snap_value(50.0, 10.0, 0.0, 1.0), 10.0);
        // A non-positive step means "no grid, clamp only".
        assert_eq!(snap_value(37.3, 0.0, 100.0, 0.0), 37.3);
        assert_eq!(snap_value(137.3, 0.0, 100.0, -1.0), 100.0);
    }

    #[test]
    fn step_decimals_reads_the_places_the_step_implies() {
        assert_eq!(step_decimals(1.0), 0);
        assert_eq!(step_decimals(0.5), 1);
        assert_eq!(step_decimals(0.25), 2);
        assert_eq!(step_decimals(10.0), 0);
    }

    // ---- Layout: every variant is constructible ---------------------------

    #[test]
    fn every_variant_lays_itself_out_at_its_own_authored_height() {
        for (variant, height) in [
            (RangeSliderVariant::Default, DEFAULT_TRACK_HEIGHT),
            (RangeSliderVariant::Bubble, BUBBLE_HEIGHT),
            (RangeSliderVariant::Fluid, FLUID_HEIGHT),
            (RangeSliderVariant::Wave, WAVE_HEIGHT),
        ] {
            let (_, size) = laid_out(&view(50.0, variant));
            assert_eq!(size, Size::new(WIDTH, height), "{variant:?}");
        }
        // The ruler's height is its own readout row plus the strip.
        let (w, size) = laid_out(&view(50.0, RangeSliderVariant::Ruler).unit("kg"));
        let readout = w.readout.size().height;
        assert!(readout > 0.0);
        assert!(
            (size.height
                - (RULER_READOUT_PADDING_TOP
                    + readout
                    + RULER_READOUT_PADDING_BOTTOM
                    + RULER_STRIP_HEIGHT))
                .abs()
                < 1e-9
        );
        assert_eq!(RangeSliderVariant::default(), RangeSliderVariant::Default);
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_documented_default() {
        let mut counter = 0u64;
        let mut w = View::<Values>::build(
            &view(50.0, RangeSliderVariant::Default),
            &mut BuildCtx::new(&mut counter),
        );
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 400.0)),
        );
        assert_eq!(size.width, UNBOUNDED_WIDTH);
    }

    // ---- Default variant --------------------------------------------------

    #[test]
    fn the_default_variant_fills_to_the_value_and_dots_every_step() {
        let (mut w, size) = laid_out(&view(25.0, RangeSliderVariant::Default).step(25.0));
        let rec = paint_settled(&mut w, size);
        // Track, then the tick dots, then the thumb.
        assert_eq!(rec.rrects[0].1, size);
        assert_eq!(rec.rrects[0].2, style::RADIUS_LG, "rounded-lg");
        let fill = rec.rects[0];
        assert!((fill.1.width - WIDTH * 0.25).abs() < 1e-6, "25% filled");
        assert_eq!(fill.2.components[3], DEFAULT_FILL_ALPHA);

        let dots: Vec<_> = rec
            .rrects
            .iter()
            .filter(|(_, s, ..)| *s == Size::new(DEFAULT_TICK_SIZE, DEFAULT_TICK_SIZE))
            .collect();
        assert_eq!(dots.len(), 5, "0, 25, 50, 75, 100");
        // The first dot sits on the thumb's own starting centre.
        assert!((dots[0].0.x + DEFAULT_TICK_SIZE / 2.0 - DEFAULT_THUMB_WIDTH / 2.0).abs() < 1e-9);

        let thumb = rec.rrects.last().expect("the thumb");
        assert_eq!(
            thumb.1,
            Size::new(DEFAULT_THUMB_WIDTH, DEFAULT_THUMB_HEIGHT)
        );
        assert!((thumb.0.x - 0.25 * (WIDTH - DEFAULT_THUMB_WIDTH)).abs() < 1e-6);
    }

    #[test]
    fn a_step_that_would_litter_the_track_draws_no_dots_at_all() {
        // 200 steps is past `steps <= 50`, so upstream draws none.
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Default).step(0.5));
        let rec = paint_settled(&mut w, size);
        assert!(
            !rec.rrects
                .iter()
                .any(|(_, s, ..)| *s == Size::new(DEFAULT_TICK_SIZE, DEFAULT_TICK_SIZE))
        );
        // ...and `show_ticks(false)` silences them at any step count.
        let (mut w, size) = laid_out(
            &view(50.0, RangeSliderVariant::Default)
                .step(25.0)
                .show_ticks(false),
        );
        let rec = paint_settled(&mut w, size);
        assert!(
            !rec.rrects
                .iter()
                .any(|(_, s, ..)| *s == Size::new(DEFAULT_TICK_SIZE, DEFAULT_TICK_SIZE))
        );
    }

    #[test]
    fn grabbing_the_default_thumb_stretches_it_and_releasing_lets_it_go() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Default));
        let mut state = Values::default();
        let rest = *paint_settled(&mut w, size).rrects.last().expect("thumb");
        assert_eq!(rest.1.height, DEFAULT_THUMB_HEIGHT);

        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 120.0, 20.0),
        );
        paint_at(&mut w, size, None, 0.0);
        let (grabbed, owes) = paint_at(&mut w, size, None, 40.0);
        assert!(owes);
        let stretched = grabbed.rrects.last().expect("thumb").1.height;
        assert!(
            stretched > DEFAULT_THUMB_HEIGHT
                && stretched <= DEFAULT_THUMB_HEIGHT * DEFAULT_THUMB_STRETCH * 1.2,
            "stretched to {stretched}"
        );
    }

    // ---- Drag and keyboard math -------------------------------------------

    #[test]
    fn a_press_maps_the_pointer_onto_the_track_and_reports_the_snapped_value() {
        let (mut w, size) = laid_out(&view(0.0, RangeSliderVariant::Default));
        let mut state = Values::default();
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, WIDTH * 0.25, 20.0),
        );
        assert_eq!(state.last, Some(25.0));
        assert_eq!(w.value, 0.0, "the app owns the value");

        // A drag keeps reporting, and a release commits where it ended.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, WIDTH * 0.6, 20.0),
        );
        assert_eq!(state.last, Some(60.0));
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Up, WIDTH * 3.0, 20.0),
        );
        assert_eq!(state.last, Some(100.0), "clamped, not extrapolated");
    }

    #[test]
    fn the_bubble_variant_maps_its_pointer_through_the_inset_track() {
        let track = WIDTH - 2.0 * BUBBLE_PADDING_X;
        let (mut w, size) = laid_out(&view(0.0, RangeSliderVariant::Bubble));
        let mut state = Values::default();
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, BUBBLE_PADDING_X + track / 2.0, 60.0),
        );
        assert_eq!(state.last, Some(50.0), "the track, not the padded box");

        // Pressing inside the leading padding maps to `min`, not to a negative
        // ratio the clamp would have to rescue.
        let (mut parked, size) = laid_out(&view(50.0, RangeSliderVariant::Bubble));
        dispatch(
            &mut parked,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 4.0, 60.0),
        );
        assert_eq!(state.last, Some(0.0));
    }

    #[test]
    fn the_arrow_and_end_keys_walk_the_grid() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Default).step(5.0));
        let mut state = Values::default();
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowRight));
        assert_eq!(state.last, Some(55.0));
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(state.last, Some(45.0));
        dispatch(&mut w, size, &mut state, &key(NamedKey::Home));
        assert_eq!(state.last, Some(0.0));
        dispatch(&mut w, size, &mut state, &key(NamedKey::End));
        assert_eq!(state.last, Some(100.0));
        // An unrelated key is not a nudge.
        assert_eq!(
            dispatch(&mut w, size, &mut state, &key(NamedKey::Tab)),
            EventResult::Ignored
        );
    }

    #[test]
    fn a_key_at_the_end_of_the_range_reports_nothing() {
        let (mut w, size) = laid_out(&view(100.0, RangeSliderVariant::Default));
        let mut state = Values::default();
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowRight));
        assert_eq!(state.count, 0, "a held value costs nothing");
        dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowLeft));
        assert_eq!(state.last, Some(99.0));
    }

    #[test]
    fn a_disabled_slider_is_inert_to_pointer_and_key() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Default).disabled(true));
        let mut state = Values::default();
        assert_eq!(
            dispatch(
                &mut w,
                size,
                &mut state,
                &pointer(PointerPhase::Down, 60.0, 20.0)
            ),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(&mut w, size, &mut state, &key(NamedKey::ArrowRight)),
            EventResult::Ignored
        );
        assert_eq!(state.count, 0);
        assert!(!w.dragging);
    }

    #[test]
    fn a_secondary_press_and_a_cancel_never_move_the_value() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Default));
        let mut state = Values::default();
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(60.0, 20.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, size, &mut state, &secondary),
            EventResult::Ignored
        );
        assert_eq!(state.count, 0);

        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 60.0, 20.0),
        );
        let after_press = state.count;
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Cancel, 200.0, 20.0),
        );
        assert_eq!(state.count, after_press, "a cancel reports nothing new");
        assert!(!w.dragging);
    }

    // ---- Bubble -----------------------------------------------------------

    #[test]
    fn the_bubble_pops_out_on_grab_and_is_gone_at_rest() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Bubble));
        let mut state = Values::default();
        assert!(
            paint_settled(&mut w, size).layers.is_empty(),
            "no bubble until it is grabbed"
        );

        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 120.0, 60.0),
        );
        paint_at(&mut w, size, None, 0.0);
        let (mid, owes) = paint_at(&mut w, size, None, 40.0);
        assert!(owes);
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha <= 1.0, "popping at {alpha}");
        assert!(!mid.glyphs.is_empty(), "and it carries the readout");

        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Up, 120.0, 60.0),
        );
        let gone = paint_settled(&mut w, size);
        assert!(gone.layers.is_empty(), "and it goes away on release");
    }

    #[test]
    fn the_bubble_leans_when_the_handle_is_travelling() {
        let (mut w, size) = laid_out(&view(0.0, RangeSliderVariant::Bubble));
        let mut state = Values::default();
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 30.0, 60.0),
        );
        paint_at(&mut w, size, None, 0.0);
        // Drive the position across the track between two frames: that is a
        // large velocity, so the lean saturates.
        w.value = 100.0;
        w.pos.retarget(1.0);
        paint_at(&mut w, size, None, 16.0);
        paint_at(&mut w, size, None, 32.0);
        assert!(
            w.lean.target().abs() > 0.5,
            "a fast travel leans hard: {}",
            w.lean.target()
        );
        assert!(
            w.lean.target() < 0.0,
            "and it leans against the direction of travel, as upstream maps it"
        );
    }

    // ---- Fluid ------------------------------------------------------------

    #[test]
    fn the_fluid_pill_clips_its_fill_and_paints_the_label_twice() {
        let (mut w, size) = laid_out(&view(40.0, RangeSliderVariant::Fluid));
        let rec = paint_settled(&mut w, size);
        let clip = rec.clips[0];
        assert!((clip.1.width - WIDTH * 0.4).abs() < 1e-6, "clipped to 40%");
        assert_eq!(clip.1.height, FLUID_HEIGHT);
        // Two label runs and two value runs: one on the track, one on the fill.
        assert_eq!(rec.glyphs.len(), 4, "the copy underneath and the inverse");
        assert_eq!(rec.rrects[0].2, FLUID_HEIGHT / 2.0, "a pill");
    }

    #[test]
    fn a_fluid_pill_at_zero_clips_nothing_and_paints_one_copy() {
        let (mut w, size) = laid_out(&view(0.0, RangeSliderVariant::Fluid));
        let rec = paint_settled(&mut w, size);
        assert!(rec.clips.is_empty(), "no sub-pixel hairline of a fill");
        assert_eq!(rec.glyphs.len(), 2);
    }

    // ---- Wave -------------------------------------------------------------

    #[test]
    fn the_wave_peaks_around_the_handle_and_fills_up_to_it() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Wave).bars(16));
        let rec = paint_settled(&mut w, size);
        assert_eq!(rec.rrects.len(), 16, "one rounded bar each");

        let bars: Vec<f64> = rec.rrects.iter().map(|(_, s, ..)| s.height).collect();
        let head = (0.5_f64 * 15.0).round() as usize;
        let tallest = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .expect("a tallest bar")
            .0;
        assert!(
            tallest.abs_diff(head) <= 1,
            "the crest sits on the handle: {tallest} vs {head}"
        );
        // Every bar keeps the base scale even far from the crest.
        assert!(
            bars.iter()
                .all(|h| *h >= WAVE_BAR_HEIGHT * WAVE_BASE_SCALE - 1e-6)
        );

        // Bars up to the head are filled; the rest are the /45 wash.
        let filled = rec.rrects[head].3;
        let after = rec.rrects[head + 1].3;
        assert_eq!(filled.components[3], 1.0);
        assert_eq!(after.components[3], WAVE_UNFILLED_ALPHA);

        // The bars tile the width with a `gap-1` between them.
        let bar_width = rec.rrects[0].1.width;
        assert!(
            (16.0 * bar_width + 15.0 * WAVE_BAR_GAP - WIDTH).abs() < 1e-9,
            "bars plus gaps fill the row"
        );
    }

    #[test]
    fn the_wave_travels_toward_a_new_crest_rather_than_jumping() {
        let mut counter = 0u64;
        let prev = view(0.0, RangeSliderVariant::Wave).bars(12);
        let mut w = View::<Values>::build(&prev, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 400.0)),
        );
        paint_settled(&mut w, size);
        let settled = w.wave.scale.clone();
        assert_eq!(tallest_bar(&settled), 0, "the crest starts on the handle");

        let next = view(100.0, RangeSliderVariant::Wave).bars(12);
        View::<Values>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        // Walk a second of frames: the bars are asked for frames while they
        // travel, and the crest ends up under the new handle position.
        let mut owed = false;
        let mut moved_midway = false;
        let mut at = 5_000.0;
        for frame in 0..80 {
            at += 16.0;
            let (_, owes) = paint_at(&mut w, size, None, at);
            owed |= owes;
            if frame == 20 {
                moved_midway = w
                    .wave
                    .scale
                    .iter()
                    .zip(&settled)
                    .any(|(a, b)| (a - b).abs() > 1e-3);
            }
        }
        assert!(owed, "the bars asked for their own frames");
        assert!(moved_midway, "and they were mid-travel, not snapped");
        assert!(
            tallest_bar(&w.wave.scale) >= 10,
            "the crest arrived at the far end: {:?}",
            tallest_bar(&w.wave.scale)
        );
    }

    /// The index of the tallest bar.
    fn tallest_bar(bars: &[f64]) -> usize {
        bars.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    #[test]
    fn reduce_motion_flattens_the_wave_and_owes_no_frame() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Wave).bars(8));
        let (rec, owes) = paint_at(&mut w, size, Some(&theme), 0.0);
        assert!(!owes, "reduce_motion asks for no animation frame");
        for (_, bar, ..) in &rec.rrects {
            assert!((bar.height - WAVE_BAR_HEIGHT * WAVE_REDUCED_SCALE).abs() < 1e-9);
        }
    }

    // ---- Ruler ------------------------------------------------------------

    #[test]
    fn the_ruler_builds_major_and_minor_ticks_plus_a_needle() {
        let (mut w, size) = laid_out(
            &view(50.0, RangeSliderVariant::Ruler)
                .step(10.0)
                .ruler(RULER_GAP, 5),
        );
        assert_eq!(w.ticks.len(), 11, "0..100 by 10");
        assert!(w.ticks[0].major && w.ticks[5].major && w.ticks[10].major);
        assert!(!w.ticks[1].major);
        assert!(w.ticks[0].label.is_some() && w.ticks[1].label.is_none());
        assert!((w.ticks[1].offset - RULER_GAP).abs() < 1e-9);

        let rec = paint_settled(&mut w, size);
        // The needle is the only 3px-wide mark, and it sits on the centre line.
        let needle = rec
            .rrects
            .iter()
            .find(|(_, s, ..)| s.width == RULER_NEEDLE_WIDTH)
            .expect("the needle");
        assert!((needle.0.x + RULER_NEEDLE_WIDTH / 2.0 - WIDTH / 2.0).abs() < 1e-9);
        assert_eq!(needle.1.height, RULER_NEEDLE_HEIGHT);
        // The tick strip is clipped to its own row.
        assert_eq!(rec.clips[0].1.height, RULER_STRIP_HEIGHT);
    }

    #[test]
    fn a_range_the_step_does_not_divide_gets_a_tick_of_its_own_at_max() {
        let (w, _) = laid_out(
            &view(0.0, RangeSliderVariant::Ruler)
                .range(0.0, 10.0)
                .step(4.0),
        );
        // Whole ticks at 0, 4, 8 — then one more, at 10.
        assert_eq!(w.ticks.len(), 4);
        let last = w.ticks.last().expect("the max tick");
        assert!(last.major && last.label.is_some());
        assert!((last.offset - (10.0 / 4.0) * RULER_GAP).abs() < 1e-9);
    }

    #[test]
    fn the_ruler_scrolls_under_the_needle_and_settles_on_a_tick() {
        let (mut w, size) = laid_out(&view(50.0, RangeSliderVariant::Ruler).step(10.0));
        let mut state = Values::default();
        paint_settled(&mut w, size);

        // A grab does not jump the value the way a track press does.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 120.0, 60.0),
        );
        assert_eq!(state.count, 0, "the scale is grabbed, not jumped to");

        // Dragging the strip left advances the value.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, 120.0 - 2.0 * RULER_GAP, 60.0),
        );
        assert_eq!(state.last, Some(70.0), "two steps of travel");

        // A release settles on the nearest tick, and the value stays legal.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Up, 120.0 - 2.0 * RULER_GAP, 60.0),
        );
        let landed = state.last.expect("a settled value");
        assert_eq!(landed, snap_value(landed, 0.0, 100.0, 10.0));
        assert!(!w.dragging);
    }

    #[test]
    fn dragging_the_ruler_past_an_end_keeps_only_a_sliver_of_the_travel() {
        let (mut w, size) = laid_out(&view(0.0, RangeSliderVariant::Ruler).step(10.0));
        let mut state = Values::default();
        paint_settled(&mut w, size);
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Down, 120.0, 60.0),
        );
        // Drag hard the wrong way, well past `min`.
        dispatch(
            &mut w,
            size,
            &mut state,
            &pointer(PointerPhase::Move, 120.0 + 10.0 * RULER_GAP, 60.0),
        );
        assert_eq!(state.count, 0, "the value cannot go below min");
        assert!(
            w.pos.value() < 0.0 && w.pos.value() >= -RULER_DRAG_ELASTIC,
            "one full travel of over-drag keeps exactly its elastic share: {}",
            w.pos.value()
        );
    }

    // ---- Focus, cursor, semantics -----------------------------------------

    /// One slider under a real `RenderRoot` — the only harness that can
    /// exercise focus and the cursor.
    struct Harness {
        root: RenderRoot<Values, RangeSliderView<Values>>,
        state: Values,
        tcx: TextContext,
    }

    impl Harness {
        fn new(variant: RangeSliderVariant) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: Values::default(),
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Values| view(50.0, variant);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root
                .layout_with_text(Size::new(WIDTH, 200.0), &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn every_variant_paints_its_own_focus_ring() {
        for variant in [
            RangeSliderVariant::Default,
            RangeSliderVariant::Bubble,
            RangeSliderVariant::Fluid,
            RangeSliderVariant::Ruler,
            RangeSliderVariant::Wave,
        ] {
            let mut h = Harness::new(variant);
            assert!(h.paint().ring().is_none(), "{variant:?} at rest");
            h.dispatch(&pointer(PointerPhase::Down, 120.0, 20.0));
            assert!(h.root.is_focus_active(), "{variant:?} takes focus");
            let rec = h.paint();
            let (_, width, color) = rec.ring().unwrap_or_else(|| panic!("{variant:?} rings"));
            assert_eq!(width, SLIDER_RING_WIDTH);
            let alpha = if variant == RangeSliderVariant::Fluid {
                FLUID_RING_ALPHA
            } else {
                SLIDER_RING_ALPHA
            };
            assert_eq!(color.components[3], alpha, "{variant:?}");
        }
    }

    #[test]
    fn a_move_asks_for_grab_and_a_drag_for_grabbing() {
        let mut h = Harness::new(RangeSliderVariant::Default);
        h.dispatch(&pointer(PointerPhase::Move, 120.0, 20.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grab);

        h.dispatch(&pointer(PointerPhase::Down, 120.0, 20.0));
        h.dispatch(&pointer(PointerPhase::Move, 130.0, 20.0));
        assert_eq!(h.root.cursor(), CursorIcon::Grabbing);
    }

    #[test]
    fn semantics_reports_a_slider_node_with_its_range_and_step() {
        let h = Harness::new(RangeSliderVariant::Default);
        let update = h.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Slider)
            .expect("a Role::Slider node");
        assert_eq!(node.label(), Some("volume"));
        assert_eq!(node.numeric_value(), Some(50.0));
        assert_eq!(node.min_numeric_value(), Some(0.0));
        assert_eq!(node.max_numeric_value(), Some(100.0));
        assert_eq!(node.numeric_value_step(), Some(1.0));
        assert!(node.supports_action(Action::Increment));
        assert!(node.supports_action(Action::SetValue));
    }

    #[test]
    fn disabled_paint_halves_every_painted_alpha() {
        let theme = crate::theme();
        let (mut enabled, size) = laid_out(&view(50.0, RangeSliderVariant::Default));
        let (mut disabled, _) = laid_out(&view(50.0, RangeSliderVariant::Default).disabled(true));
        let on = paint_at(&mut enabled, size, Some(&theme), 5_000.0).0;
        let off = paint_at(&mut disabled, size, Some(&theme), 5_000.0).0;
        assert!(
            (off.rrects[0].3.components[3]
                - on.rrects[0].3.components[3] * style::DISABLED_OPACITY)
                .abs()
                < 1e-6,
            "opacity-50, not the form controls' 60"
        );
    }

    /// A range change alone (the confirmed value unchanged) still moves the
    /// painted thumb: its fraction of the range shifted even though the raw
    /// value didn't.
    #[test]
    fn a_range_change_alone_re_derives_the_painted_fraction() {
        let mut counter = 0u64;
        let prev = view(50.0, RangeSliderVariant::Default).range(0.0, 100.0);
        let mut w = View::<Values>::build(&prev, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(WIDTH, 400.0)),
        );
        paint_settled(&mut w, size);
        assert!((w.pos.value() - 0.5).abs() < 1e-9, "50 of 0..100 is 0.5");

        // Narrowing the range without touching the value moves the fraction:
        // 50 of 0..200 is a quarter, not a half.
        let next = view(50.0, RangeSliderVariant::Default).range(0.0, 200.0);
        View::<Values>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        paint_settled(&mut w, size);
        assert!(
            (w.pos.value() - 0.25).abs() < 1e-9,
            "the thumb did not follow the range change: {}",
            w.pos.value()
        );
    }
}
