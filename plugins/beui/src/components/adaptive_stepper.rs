//! Ports beUI's **Adaptive Stepper** — the fixed-footprint quantity control
//! whose minus/plus pills slide away at the ends while the value pill grows to
//! fill what they leave, and whose number rolls between steps.
//!
//! Source: `components/motion/adaptive-stepper.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `adaptive-stepper`: *"Composable numeric stepper whose fixed footprint
//! adapts at its minimum and maximum while the value rolls between steps."*
//!
//! | class / prop | here |
//! |---|---|
//! | `h-12 w-[13.5rem]` | [`STEPPER_HEIGHT`] × [`STEPPER_WIDTH`] |
//! | `LiquidItem width={48} height={48} radius={24}` | [`STEPPER_BUTTON`], [`STEPPER_RADIUS`] |
//! | action `x`: `-1 → 32 : 0`, `+1 → 136 : 168` | [`stepper_geometry`] |
//! | value `x`/`width`: `0/216`, `0/152`, `64/152`, `64/88` | [`stepper_geometry`] |
//! | `STEPPER_LIQUID_TRANSITION = { duration: 600, ease: [0.22, 1.3, 0.71, 1] }` | [`LIQUID_MS`] / [`LIQUID_EASE`] |
//! | `whileTap={{ scale: 0.94 }}` + `SPRING_PRESS` | [`BUTTON_PRESS_SCALE`] |
//! | hidden action `opacity 0, blur(2px)`, `duration 0.15` | [`HIDE_MS`], the blur dropped |
//! | value `text-lg font-semibold tabular-nums` | [`VALUE_TEXT_SIZE`], `FontWeight::SEMI_BOLD` |
//! | roll `enter 0.18 / exit 0.12`, `translateY(±32%)`, `opacity 0.35 → 1` | [`ROLL_ENTER_MS`] / [`ROLL_EXIT_MS`] / [`ROLL_SHIFT`] / [`ROLL_ENTER_OPACITY`] |
//! | `nextStep` / `cleanNumber` / `clamp` | [`next_step`] / [`clean_number`] / [`clamp_value`] |
//!
//! # Premise correction: this is a numeric stepper, not a step-flow container
//!
//! The name reads like a wizard, and it is not one. Upstream's `adaptive-stepper`
//! is a **quantity selector** — a `Minus` button, a rolling number and a `Plus`
//! button inside one 216 × 48 pill — and the *adaptive* half is its footprint:
//! at the minimum the minus button slides out and the number's pill grows over
//! the space it vacated, and symmetrically at the maximum. There is no step
//! sequence, no per-step content and no step indicator anywhere in the file.
//! This port is of what upstream ships; "animates both directions" is the value
//! roll, which reverses with the sign of the change.
//!
//! # Composable upstream, one widget here
//!
//! Upstream is four exported pieces over a context (`AdaptiveStepper`,
//! `AdaptiveStepperDecrement`, `AdaptiveStepperValue`, `AdaptiveStepperIncrement`)
//! whose whole job is to share the value, the direction and the two button refs.
//! The composition buys nothing here — the geometry is a fixed table keyed on
//! *at-min* / *at-max*, so the three items can only ever be arranged one way —
//! so the port is a single widget, the same call `radio` made about its group.
//!
//! # Controlled, never self-mutating
//!
//! A press reports the **requested** value through `on_change` and leaves
//! `value` untouched until the next rebuild feeds the app-confirmed one back
//! down, exactly as upstream's controlled arm behaves. An app that clamps or
//! rejects the request sees its own value win on the next pass.
//!
//! # Degradations
//!
//! - **No liquid metaball merge.** Upstream wraps the three items in a `Liquid`
//!   surface (an SVG blur + contrast filter) so they fuse into one gooey blob as
//!   they meet, and fills them with `--background` because the filter is what
//!   draws their edges. The scene has no such filter, so each pill is painted on
//!   its own — and carries the catalog's own 1px hairline, without which three
//!   `background`-filled pills on a `background` page would be invisible. The
//!   sliding and growing geometry is ported exactly; only the fusing is not.
//! - **No blur on the hidden button or the rolling digits.** `filter: blur(2px)`
//!   has no counterpart in this scene's paint vocabulary; the opacity halves of
//!   both are ported — the same call `input` made for its message row.
//! - **No focus restoration across an end.** Upstream moves focus onto the
//!   *other* button when the one under the pointer hides at an end
//!   (`decrementRef.current?.focus()`); frust has no "focus that widget" call to
//!   aim at a sibling, so the single widget simply keeps its own focus.
//! - **`aria-live` is not published.** The framework's semantics node carries no
//!   live-region flag, so the value change is reported as a plain numeric-value
//!   update rather than an announced one.
//!
//! # Addition: the arrow keys step
//!
//! Upstream's two `<button>`s each activate on the browser's own Space/Enter,
//! which needs two focus targets; this port is one widget, so it takes
//! `ArrowUp`/`ArrowRight` as increment and `ArrowDown`/`ArrowLeft` as decrement
//! instead. That is an adaptation, not a port — stated here rather than passed
//! off as upstream's.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget,
    erase_callback_arg,
};
use frust::{Curve, FrameTime, Theme};

use super::select::{panel_colors, stroke_frame};
use crate::motion::Ramp;
use crate::press::{Lane, inside_inclusive, press_scale, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType};
use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};
use crate::tokens::sans_family;

// ---- Metrics ---------------------------------------------------------------

/// The control's fixed width, in logical px (`w-[13.5rem]`).
pub const STEPPER_WIDTH: f64 = 216.0;

/// The control's fixed height, in logical px (`h-12`).
pub const STEPPER_HEIGHT: f64 = 48.0;

/// A button pill's edge, in logical px (`width={48} height={48}`).
pub const STEPPER_BUTTON: f64 = 48.0;

/// Every pill's corner radius, in logical px (`radius={24}`).
pub const STEPPER_RADIUS: f64 = 24.0;

/// The decrement pill's `x` while it is shown, in logical px.
pub const DECREMENT_X: f64 = 0.0;
/// The decrement pill's `x` once it has slid away at the minimum, in logical px.
pub const DECREMENT_X_HIDDEN: f64 = 32.0;
/// The increment pill's `x` while it is shown, in logical px.
pub const INCREMENT_X: f64 = 168.0;
/// The increment pill's `x` once it has slid away at the maximum, in logical px.
pub const INCREMENT_X_HIDDEN: f64 = 136.0;

/// The value pill's `x` with both buttons showing, in logical px.
pub const VALUE_X_CENTRED: f64 = 64.0;
/// The value pill's width with both buttons showing, in logical px.
pub const VALUE_WIDTH_NARROW: f64 = 88.0;
/// The value pill's width with exactly one button gone, in logical px.
pub const VALUE_WIDTH_WIDE: f64 = 152.0;
/// The value pill's width with both buttons gone, in logical px — the whole
/// footprint.
pub const VALUE_WIDTH_FULL: f64 = STEPPER_WIDTH;

/// How long a geometry change takes, in milliseconds (`duration: 600`).
pub const LIQUID_MS: u64 = 600;

/// The deliberately elastic separation curve
/// (`ease: [0.22, 1.3, 0.71, 1]`) — its `y` control point past `1` is what makes
/// a pill overshoot its slot before settling, and is upstream's own authored
/// value rather than one of the catalog's named curves.
pub const LIQUID_EASE: Curve = Curve::Cubic(0.22, 1.3, 0.71, 1.0);

/// The scale a pressed button shrinks to (`whileTap={{ scale: 0.94 }}`).
pub const BUTTON_PRESS_SCALE: f64 = 0.94;

/// How long a button's hide/show fade takes, in milliseconds
/// (`duration: 0.15`).
pub const HIDE_MS: u64 = 150;

/// The value's text size, in logical px (`text-lg`).
pub const VALUE_TEXT_SIZE: f64 = 18.0;

/// The glyph box of a button's icon, in logical px (`size-5`).
pub const ICON_BOX: f64 = style::ICON_SIZE_LG;

/// The icon's stroke width in the lucide 24-unit viewBox (`strokeWidth={2.5}`).
pub const ICON_STROKE_VIEWBOX: f64 = 2.5;

/// How long an entering digit takes, in milliseconds (`duration: 0.18`).
pub const ROLL_ENTER_MS: f64 = 180.0;

/// How long a leaving digit takes, in milliseconds (`duration: 0.12`).
pub const ROLL_EXIT_MS: f64 = 120.0;

/// How far a rolling digit travels, as a fraction of its own line box
/// (`translateY(±32%)`).
pub const ROLL_SHIFT: f64 = 0.32;

/// The opacity an entering digit starts at (`opacity: 0.35`).
pub const ROLL_ENTER_OPACITY: f64 = 0.35;

/// How many decimal places [`clean_number`] rounds to — upstream's
/// `Number(value.toFixed(10))`.
pub const CLEAN_DECIMALS: i32 = 10;

/// Opacity of a disabled stepper: `context.disabled && "opacity-50"`.
const DISABLED_OPACITY: f32 = style::DISABLED_OPACITY;

// ---- The numeric contract --------------------------------------------------

/// Round `value` to [`CLEAN_DECIMALS`] places — upstream's `cleanNumber`, which
/// exists so a run of fractional steps does not accumulate binary-float dust
/// into a value like `0.30000000000000004`.
pub fn clean_number(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let scale = 10f64.powi(CLEAN_DECIMALS);
    (value * scale).round() / scale
}

/// Clamp `value` into `[min, max]` — upstream's `clamp`, whose `Math.min` before
/// `Math.max` means `min` wins an inverted range.
pub fn clamp_value(value: f64, min: f64, max: f64) -> f64 {
    max.min(value).max(min)
}

/// The value one step away from `value` in `direction` (`1` up, anything else
/// down) — upstream's `nextStep`.
///
/// Steps land on the `min + n·step` lattice rather than on `value ± step`, so a
/// value nudged off the lattice by a controlled parent snaps back onto it. The
/// `1e-10` fudge upstream carries is kept: it is what stops a value that is
/// *exactly* on a lattice point from being read as a hair past it by binary
/// float, which would skip a step.
pub fn next_step(value: f64, direction: i32, min: f64, max: f64, step: f64) -> f64 {
    const FUDGE: f64 = 1e-10;
    if direction == 1 {
        let index = ((value - min) / step + FUDGE).floor() + 1.0;
        clean_number(max.min(min + index * step))
    } else {
        let index = ((value - min) / step - FUDGE).ceil() - 1.0;
        clean_number(min.max(min + index * step))
    }
}

/// Where the three pills sit for a given pair of end states, in logical px.
///
/// The whole of upstream's adaptivity: a button that has nothing left to do
/// slides *inward* out of the footprint, and the value pill takes the space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepperGeometry {
    /// The decrement pill's left edge.
    pub decrement_x: f64,
    /// The increment pill's left edge.
    pub increment_x: f64,
    /// The value pill's left edge.
    pub value_x: f64,
    /// The value pill's width.
    pub value_width: f64,
}

/// The geometry for a stepper sitting at (or between) its ends.
pub fn stepper_geometry(at_min: bool, at_max: bool) -> StepperGeometry {
    let (value_x, value_width) = match (at_min, at_max) {
        (true, true) => (0.0, VALUE_WIDTH_FULL),
        (true, false) => (0.0, VALUE_WIDTH_WIDE),
        (false, true) => (VALUE_X_CENTRED, VALUE_WIDTH_WIDE),
        (false, false) => (VALUE_X_CENTRED, VALUE_WIDTH_NARROW),
    };
    StepperGeometry {
        decrement_x: if at_min {
            DECREMENT_X_HIDDEN
        } else {
            DECREMENT_X
        },
        increment_x: if at_max {
            INCREMENT_X_HIDDEN
        } else {
            INCREMENT_X
        },
        value_x,
        value_width,
    }
}

// ---- The view --------------------------------------------------------------

/// A view-held, typed value callback (erased on build).
type OnChange<State> = Rc<dyn Fn(&mut State, f64)>;
/// A caller-supplied value formatter (`formatValueText`).
type Format = Rc<dyn Fn(f64) -> String>;

/// A declarative beUI adaptive stepper. See [`adaptive_stepper`].
pub struct AdaptiveStepperView<State: 'static> {
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    label: String,
    format: Option<Format>,
    on_change: OnChange<State>,
}

/// Create a controlled adaptive stepper showing `value` and reporting each
/// requested change through `on_change(state, requested)`.
///
/// Defaults match upstream: `min = 0`, `max = 10`, `step = 1`, and the label
/// `"Quantity"`.
pub fn adaptive_stepper<State: 'static, F: Fn(&mut State, f64) + 'static>(
    value: f64,
    on_change: F,
) -> AdaptiveStepperView<State> {
    AdaptiveStepperView {
        value,
        min: 0.0,
        max: 10.0,
        step: 1.0,
        disabled: false,
        label: String::from("Quantity"),
        format: None,
        on_change: Rc::new(on_change),
    }
}

impl<State: 'static> AdaptiveStepperView<State> {
    /// Set the lower end (default `0`). A non-finite value falls back to `0`,
    /// upstream's own guard.
    pub fn min(mut self, min: f64) -> Self {
        self.min = min;
        self
    }

    /// Set the upper end (default `10`). One at or below the lower end collapses
    /// the range onto it, upstream's own guard.
    pub fn max(mut self, max: f64) -> Self {
        self.max = max;
        self
    }

    /// Set the stride (default `1`). A non-positive or non-finite stride falls
    /// back to `1`, upstream's own guard.
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Disable the control: half opacity, inert, a not-allowed cursor.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech (`aria-label`, default `"Quantity"`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Render the value as text yourself — upstream's `formatValueText`, which
    /// is what turns `3` into `"3 items"` or a currency string.
    pub fn format<F: Fn(f64) -> String + 'static>(mut self, format: F) -> Self {
        self.format = Some(Rc::new(format));
        self
    }

    /// The guarded bounds and stride, upstream's own coercions applied.
    fn bounds(&self) -> (f64, f64, f64) {
        let lower = if self.min.is_finite() { self.min } else { 0.0 };
        let supplied = if self.max.is_finite() {
            self.max
        } else {
            lower
        };
        let upper = if supplied > lower { supplied } else { lower };
        let stride = if self.step.is_finite() && self.step > 0.0 {
            self.step
        } else {
            1.0
        };
        (lower, upper, stride)
    }

    /// The value this stepper actually shows: the supplied one, clamped.
    fn current(&self) -> f64 {
        let (lower, upper, _) = self.bounds();
        clamp_value(
            if self.value.is_finite() {
                self.value
            } else {
                lower
            },
            lower,
            upper,
        )
    }

    /// The value's text, through the caller's formatter when there is one.
    fn value_text(&self) -> String {
        let current = self.current();
        match &self.format {
            Some(format) => format(current),
            None => format!("{current}"),
        }
    }
}

/// Which of the two buttons a pointer or a key is aimed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Decrement,
    Increment,
}

/// A digit roll in flight: what it is replacing, and which way it goes.
struct Roll {
    /// The run being replaced, still painted while it leaves.
    previous: LabelRun,
    /// `1` when the value rose, `-1` when it fell — the sign upstream's
    /// `distance` carries.
    direction: f64,
    /// The frame it started on; a zero start means "staged, not yet latched".
    started: FrameTime,
}

/// The retained widget for an [`AdaptiveStepperView`].
pub struct AdaptiveStepperWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    disabled: bool,
    label: String,
    text: LabelRun,
    roll: Option<Roll>,
    /// The four geometry lanes, all on the elastic separation curve.
    decrement_x: Lane,
    increment_x: Lane,
    value_x: Lane,
    value_width: Lane,
    /// The two buttons' show/hide fades, `1.0` shown .. `0.0` gone.
    decrement_shown: Lane,
    increment_shown: Lane,
    /// The two buttons' press shrinks, `0.0` .. `1.0`.
    decrement_press: Lane,
    increment_press: Lane,
    hovered: Option<Side>,
    captured: Option<Side>,
    on_change: ErasedArgCallback<f64>,
}

/// The lane every geometry value follows.
fn liquid_lane(value: f64) -> Lane {
    Lane::at_rest(
        Ramp::eased(Duration::from_millis(LIQUID_MS), LIQUID_EASE),
        value,
    )
}

/// The type-scale role the value's family resolves from at layout.
const VALUE_ROLE: ThemeTextType = ThemeTextType::TitleMedium;

/// The value's text style: `text-lg font-semibold`. The family here is the
/// unthemed base; `layout` shapes in [`VALUE_ROLE`]'s family.
fn value_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::SEMI_BOLD,
        ..TextStyle::new(VALUE_TEXT_SIZE as f32, crate::text::SHAPING_INK)
    }
}

impl AdaptiveStepperWidget {
    /// Whether the value sits on the lower end.
    fn at_min(&self) -> bool {
        self.value <= self.min
    }

    /// Whether the value sits on the upper end.
    fn at_max(&self) -> bool {
        self.value >= self.max
    }

    /// Whether `side`'s button has anything left to do.
    fn active(&self, side: Side) -> bool {
        !self.disabled
            && match side {
                Side::Decrement => !self.at_min(),
                Side::Increment => !self.at_max(),
            }
    }

    /// `side`'s pill, in the widget's own space, as the lanes currently place it.
    fn button_rect(&self, side: Side) -> Rect {
        let x = match side {
            Side::Decrement => self.decrement_x.value(),
            Side::Increment => self.increment_x.value(),
        };
        Rect::from_origin_size(
            Point::new(x, 0.0),
            Size::new(STEPPER_BUTTON, STEPPER_HEIGHT),
        )
    }

    /// The value pill, in the widget's own space.
    fn value_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::new(self.value_x.value(), 0.0),
            Size::new(self.value_width.value().max(0.0), STEPPER_HEIGHT),
        )
    }

    /// The button under a widget-local `pos`, if it is one that can be pressed.
    fn hit_button(&self, pos: Point) -> Option<Side> {
        [Side::Decrement, Side::Increment]
            .into_iter()
            .find(|side| self.active(*side) && self.button_rect(*side).contains(pos))
    }

    /// Re-aim every geometry lane at the slot the current value implies.
    fn retarget_geometry(&mut self) {
        let geometry = stepper_geometry(self.at_min(), self.at_max());
        self.decrement_x.retarget(geometry.decrement_x);
        self.increment_x.retarget(geometry.increment_x);
        self.value_x.retarget(geometry.value_x);
        self.value_width.retarget(geometry.value_width);
        self.decrement_shown
            .retarget(f64::from(u8::from(!self.at_min())));
        self.increment_shown
            .retarget(f64::from(u8::from(!self.at_max())));
    }

    /// The value one step away in `direction`, or `None` when that end is
    /// already reached (upstream's `if (disabled || currentValue <= lower)
    /// return`).
    fn stepped(&self, direction: i32) -> Option<f64> {
        if self.disabled {
            return None;
        }
        if direction == 1 && self.at_max() {
            return None;
        }
        if direction != 1 && self.at_min() {
            return None;
        }
        let next = next_step(self.value, direction, self.min, self.max, self.step);
        (next != self.value).then_some(next)
    }

    /// Ask for a step in `direction`, reporting the requested value.
    fn request(&mut self, ctx: &mut EventCtx, direction: i32) -> bool {
        let Some(next) = self.stepped(direction) else {
            return false;
        };
        (self.on_change)(ctx, next);
        true
    }

    /// The cursor this control asks for.
    fn cursor(&self) -> CursorIcon {
        if self.disabled {
            style::DISABLED_CURSOR
        } else {
            style::ACTIVE_CURSOR
        }
    }

    /// The two roll progresses at `now`: `(entering, leaving)`, plus whether the
    /// roll is still running.
    fn roll_progress(&self, now: FrameTime) -> (f64, f64, bool) {
        let Some(roll) = &self.roll else {
            return (1.0, 1.0, false);
        };
        let elapsed = now.saturating_sub(roll.started).as_secs_f64() * 1000.0;
        let enter = EASE_OUT.transform((elapsed / ROLL_ENTER_MS).clamp(0.0, 1.0));
        let leave = EASE_OUT.transform((elapsed / ROLL_EXIT_MS).clamp(0.0, 1.0));
        (enter, leave, elapsed < ROLL_ENTER_MS)
    }
}

impl<State: 'static> View<State> for AdaptiveStepperView<State> {
    type Element = AdaptiveStepperWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AdaptiveStepperWidget {
        let (min, max, step) = self.bounds();
        let value = self.current();
        let geometry = stepper_geometry(value <= min, value >= max);
        AdaptiveStepperWidget {
            value,
            min,
            max,
            step,
            disabled: self.disabled,
            label: self.label.clone(),
            text: LabelRun::new(self.value_text()),
            roll: None,
            decrement_x: liquid_lane(geometry.decrement_x),
            increment_x: liquid_lane(geometry.increment_x),
            value_x: liquid_lane(geometry.value_x),
            value_width: liquid_lane(geometry.value_width),
            decrement_shown: Lane::at_rest(
                Ramp::eased(Duration::from_millis(HIDE_MS), EASE_OUT),
                f64::from(u8::from(value > min)),
            ),
            increment_shown: Lane::at_rest(
                Ramp::eased(Duration::from_millis(HIDE_MS), EASE_OUT),
                f64::from(u8::from(value < max)),
            ),
            decrement_press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            increment_press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            hovered: None,
            captured: None,
            on_change: erase_callback_arg(&self.on_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AdaptiveStepperWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_change = erase_callback_arg(&self.on_change);
        let mut flags = ChangeFlags::NONE;

        let (min, max, step) = self.bounds();
        if element.min != min || element.max != max || element.step != step {
            element.min = min;
            element.max = max;
            element.step = step;
            flags |= ChangeFlags::PAINT;
        }
        let value = self.current();
        if element.value != value {
            let direction = if value > element.value { 1.0 } else { -1.0 };
            element.value = value;
            let previous = std::mem::replace(&mut element.text, LabelRun::new(self.value_text()));
            element.roll = Some(Roll {
                previous,
                direction,
                started: FrameTime::from_nanos(0),
            });
            element.retarget_geometry();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if prev.format.is_some() != self.format.is_some()
            || element.text.set_content(self.value_text())
        {
            // A formatter change with the same number is a re-shape, not a roll.
            element.text.set_content(self.value_text());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = None;
                element.hovered = None;
                element.decrement_press.retarget(0.0);
                element.increment_press.retarget(0.0);
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.label != self.label {
            element.label = self.label.clone();
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for AdaptiveStepperWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = value_style();
        self.text.layout_themed(ctx, &style, VALUE_ROLE);
        if let Some(roll) = &mut self.roll {
            roll.previous.layout_themed(ctx, &style, VALUE_ROLE);
        }
        // `inline-block h-12 w-[13.5rem]`: the footprint never changes — the
        // whole point of the component.
        bc.constrain(Size::new(STEPPER_WIDTH, STEPPER_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() || self.disabled {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = panel_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();

        if let Some(roll) = &mut self.roll
            && roll.started.as_nanos() == 0
        {
            roll.started = now;
        }

        let mut owes_frame = false;
        if reduce {
            self.decrement_x.snap();
            self.increment_x.snap();
            self.value_x.snap();
            self.value_width.snap();
            self.decrement_shown.snap();
            self.increment_shown.snap();
            self.decrement_press.snap();
            self.increment_press.snap();
            self.roll = None;
        } else {
            owes_frame |= self.decrement_x.advance(now);
            owes_frame |= self.increment_x.advance(now);
            owes_frame |= self.value_x.advance(now);
            owes_frame |= self.value_width.advance(now);
            owes_frame |= self.decrement_shown.advance(now);
            owes_frame |= self.increment_shown.advance(now);
            owes_frame |= self.decrement_press.advance(now);
            owes_frame |= self.increment_press.advance(now);
        }
        let (enter, leave, rolling) = self.roll_progress(now);
        owes_frame |= rolling;
        if !rolling {
            self.roll = None;
        }

        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);

        // The value pill first: the two buttons slide over it at the ends.
        let value_rect = self.value_rect() + origin.to_vec2();
        let radius = style::resolve_radius(STEPPER_RADIUS, value_rect.width(), value_rect.height());
        scene.fill_rounded_rect(
            value_rect.origin(),
            value_rect.size(),
            radius,
            tint(colors.surface),
        );
        stroke_frame(
            scene,
            value_rect.origin(),
            value_rect.size(),
            radius,
            tint(colors.border),
        );

        // The digits, clipped to their own pill so a rolling one never escapes.
        scene.push_clip_rounded(value_rect.origin(), value_rect.size(), radius);
        let line = self.text.size();
        let shift = line.height * ROLL_SHIFT;
        let centre = Point::new(
            value_rect.center().x - line.width / 2.0,
            value_rect.center().y - line.height / 2.0,
        );
        if let Some(roll) = &self.roll {
            let previous = roll.previous.size();
            let alpha = (1.0 - leave).clamp(0.0, 1.0) as f32;
            if alpha > 0.0 {
                let at = Point::new(
                    value_rect.center().x - previous.width / 2.0,
                    value_rect.center().y - previous.height / 2.0 - roll.direction * shift * leave,
                );
                scene.push_layer(at, previous, alpha);
                roll.previous.paint(at, tint(colors.ink), scene);
                scene.pop_layer();
            }
        }
        let direction = self.roll.as_ref().map_or(0.0, |roll| roll.direction);
        let at = Point::new(centre.x, centre.y + direction * shift * (1.0 - enter));
        let alpha =
            (ROLL_ENTER_OPACITY + (1.0 - ROLL_ENTER_OPACITY) * enter).clamp(0.0, 1.0) as f32;
        if alpha < 1.0 {
            scene.push_layer(at, line, alpha);
            self.text.paint(at, tint(colors.ink), scene);
            scene.pop_layer();
        } else {
            self.text.paint(at, tint(colors.ink), scene);
        }
        scene.pop_clip();

        // The two buttons.
        for side in [Side::Decrement, Side::Increment] {
            let shown = match side {
                Side::Decrement => self.decrement_shown.value(),
                Side::Increment => self.increment_shown.value(),
            }
            .clamp(0.0, 1.0);
            let rect = self.button_rect(side) + origin.to_vec2();
            let radius = style::resolve_radius(STEPPER_RADIUS, rect.width(), rect.height());
            let press = match side {
                Side::Decrement => self.decrement_press.value(),
                Side::Increment => self.increment_press.value(),
            };
            let scale = press_scale(BUTTON_PRESS_SCALE, press);
            let pivot = rect.center();
            scene.push_transform(
                Affine::translate(pivot.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-pivot.to_vec2()),
            );
            // The pill itself stays put while its glyph fades — upstream animates
            // only the `<motion.span>` inside the button.
            scene.fill_rounded_rect(rect.origin(), rect.size(), radius, tint(colors.surface));
            stroke_frame(
                scene,
                rect.origin(),
                rect.size(),
                radius,
                tint(colors.border),
            );
            if self.hovered == Some(side) && shown >= 1.0 {
                scene.fill_rounded_rect(rect.origin(), rect.size(), radius, tint(colors.wash));
            }
            if shown > 0.0 {
                let layered = shown < 1.0;
                if layered {
                    scene.push_layer(rect.origin(), rect.size(), shown as f32);
                }
                draw_sign(
                    scene,
                    rect.center(),
                    side == Side::Increment,
                    tint(colors.ink),
                );
                if layered {
                    scene.pop_layer();
                }
            }
            scene.pop_transform();
        }

        // Paint-only animation (the footprint is fixed), so a bare frame request
        // is the right one — never `request_layout`.
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                let Some(direction) = step_key(key) else {
                    return EventResult::Ignored;
                };
                if self.request(ctx, direction) {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if self.disabled || !presses(p) || !inside_inclusive(p.position, size) {
                            return EventResult::Ignored;
                        }
                        let Some(side) = self.hit_button(p.position) else {
                            return EventResult::Ignored;
                        };
                        self.captured = Some(side);
                        match side {
                            Side::Decrement => self.decrement_press.retarget(1.0),
                            Side::Increment => self.increment_press.retarget(1.0),
                        }
                        ctx.capture_pointer();
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        let Some(side) = self.captured else {
                            let hovered = self.hit_button(p.position);
                            if hovered.is_some() {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            if self.hovered != hovered {
                                self.hovered = hovered;
                                ctx.request_redraw();
                            }
                            return EventResult::Ignored;
                        };
                        // Captured: let the press scale go off the button, so a
                        // drag away reads as a cancelled press.
                        ctx.set_cursor(self.cursor());
                        let want = f64::from(u8::from(self.button_rect(side).contains(p.position)));
                        let lane = match side {
                            Side::Decrement => &mut self.decrement_press,
                            Side::Increment => &mut self.increment_press,
                        };
                        if lane.target() != want {
                            lane.retarget(want);
                            ctx.request_redraw();
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        let Some(side) = self.captured.take() else {
                            return EventResult::Ignored;
                        };
                        match side {
                            Side::Decrement => self.decrement_press.retarget(0.0),
                            Side::Increment => self.increment_press.retarget(0.0),
                        }
                        if self.button_rect(side).contains(p.position) {
                            let direction = if side == Side::Increment { 1 } else { -1 };
                            self.request(ctx, direction);
                        }
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        let Some(side) = self.captured.take() else {
                            return EventResult::Ignored;
                        };
                        // Internal state only — never the callback.
                        match side {
                            Side::Decrement => self.decrement_press.retarget(0.0),
                            Side::Increment => self.increment_press.retarget(0.0),
                        }
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |node| {
                node.set_label(self.label.as_str());
                node.set_numeric_value(self.value);
                node.set_min_numeric_value(self.min);
                node.set_max_numeric_value(self.max);
                node.set_numeric_value_step(self.step);
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| {
                for (side, label) in [
                    (Side::Decrement, "Decrease value"),
                    (Side::Increment, "Increase value"),
                ] {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(label);
                        if self.active(side) {
                            node.add_action(Action::Click);
                        } else {
                            // `aria-hidden` + `disabled` at an end, or while the
                            // whole control is disabled.
                            node.set_disabled();
                        }
                    });
                }
            },
        );
    }
}

/// The step a key asks for, if it asks for one — the port's own keyboard arm
/// (see the [module docs](self)' addition note).
fn step_key(key: &KeyEvent) -> Option<i32> {
    match &key.key {
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowRight) => Some(1),
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowLeft) => Some(-1),
        _ => None,
    }
}

/// Paint a lucide `Minus` (or `Plus`, with `cross`) centred on `centre`.
fn draw_sign(scene: &mut dyn PaintScene, centre: Point, cross: bool, color: Color) {
    let arm = ICON_BOX * 0.3;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - arm, centre.y));
    path.line_to(Point::new(centre.x + arm, centre.y));
    if cross {
        path.move_to(Point::new(centre.x, centre.y - arm));
        path.line_to(Point::new(centre.x, centre.y + arm));
    }
    scene.stroke_path(
        Point::ZERO,
        &path,
        ICON_STROKE_VIEWBOX * ICON_BOX / 24.0,
        &Brush::Solid(color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Modifiers, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
    }

    #[derive(Default)]
    struct App {
        requested: Vec<f64>,
    }

    const BOX: Size = Size::new(STEPPER_WIDTH, STEPPER_HEIGHT);

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn view(value: f64) -> AdaptiveStepperView<App> {
        adaptive_stepper::<App, _>(value, |s: &mut App, next| s.requested.push(next))
            .min(0.0)
            .max(10.0)
            .step(1.0)
            .label("Quantity")
    }

    fn build(view: &AdaptiveStepperView<App>) -> AdaptiveStepperWidget {
        let mut counter = 0u64;
        View::<App>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AdaptiveStepperWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)))
    }

    fn paint_at(w: &mut AdaptiveStepperWidget, theme: Option<&Theme>, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, BOX, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
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

    fn dispatch(w: &mut AdaptiveStepperWidget, app: &mut App, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = app;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        w.event(&mut ctx, event)
    }

    /// Rebuild with a new value, the way a controlled parent would.
    fn revalue(w: &mut AdaptiveStepperWidget, from: f64, to: f64) {
        let prev = view(from);
        let next = view(to);
        let mut counter = 0u64;
        View::<App>::rebuild(&next, &prev, w, &mut BuildCtx::new(&mut counter));
    }

    // ---- Metrics and arithmetic -------------------------------------------

    #[test]
    fn the_authored_metrics_are_the_upstream_ones() {
        assert_eq!(STEPPER_WIDTH, 216.0, "w-[13.5rem]");
        assert_eq!(STEPPER_HEIGHT, 48.0, "h-12");
        assert_eq!(STEPPER_BUTTON, 48.0);
        assert_eq!(STEPPER_RADIUS, 24.0);
        assert_eq!(LIQUID_MS, 600);
        assert_eq!(BUTTON_PRESS_SCALE, 0.94, "whileTap scale");
        assert_eq!(ROLL_SHIFT, 0.32, "translateY(32%)");
        assert_eq!(ROLL_ENTER_OPACITY, 0.35);
    }

    /// The elastic separation curve really does overshoot — the one property
    /// that makes it worth naming rather than reusing `EASE_OUT`.
    #[test]
    fn the_liquid_curve_overshoots_before_it_settles() {
        let peak = (0..=200)
            .map(|i| LIQUID_EASE.transform(i as f64 / 200.0))
            .fold(0.0_f64, f64::max);
        assert!(
            peak > 1.0,
            "the y control point past 1 should overshoot: {peak}"
        );
        assert_eq!(LIQUID_EASE.transform(0.0), 0.0);
        assert!((LIQUID_EASE.transform(1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn stepping_lands_on_the_lattice_and_stops_at_the_ends() {
        assert_eq!(next_step(0.0, 1, 0.0, 10.0, 1.0), 1.0);
        assert_eq!(next_step(3.0, -1, 0.0, 10.0, 1.0), 2.0);
        assert_eq!(next_step(10.0, 1, 0.0, 10.0, 1.0), 10.0, "clamped at max");
        assert_eq!(next_step(0.0, -1, 0.0, 10.0, 1.0), 0.0, "clamped at min");

        // A value off the lattice snaps onto it rather than adding a stride.
        assert_eq!(next_step(2.4, 1, 0.0, 10.0, 1.0), 3.0);
        assert_eq!(next_step(2.4, -1, 0.0, 10.0, 1.0), 2.0);

        // Fractional strides stay clean instead of accumulating float dust.
        assert_eq!(next_step(0.2, 1, 0.0, 1.0, 0.1), 0.3);
        assert_eq!(next_step(0.3, -1, 0.0, 1.0, 0.1), 0.2);

        // A non-zero lower end shifts the lattice with it.
        assert_eq!(next_step(5.0, 1, 5.0, 9.0, 2.0), 7.0);
        assert_eq!(next_step(7.0, 1, 5.0, 9.0, 2.0), 9.0);
    }

    #[test]
    fn clean_number_and_clamp_match_their_upstream_shapes() {
        assert_eq!(clean_number(0.1 + 0.2), 0.3);
        assert_eq!(clean_number(3.0), 3.0);
        assert!(clean_number(f64::NAN).is_nan());
        assert_eq!(clamp_value(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp_value(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp_value(11.0, 0.0, 10.0), 10.0);
        assert_eq!(
            clamp_value(5.0, 10.0, 0.0),
            10.0,
            "min wins an inverted range"
        );
    }

    /// The adaptivity itself: four states, four layouts, and the footprint
    /// unchanged in every one.
    #[test]
    fn the_geometry_table_is_the_upstream_one() {
        let both = stepper_geometry(false, false);
        assert_eq!(both.decrement_x, 0.0);
        assert_eq!(both.increment_x, 168.0);
        assert_eq!(both.value_x, 64.0);
        assert_eq!(both.value_width, 88.0);

        let at_min = stepper_geometry(true, false);
        assert_eq!(at_min.decrement_x, 32.0, "the minus slides inward");
        assert_eq!(at_min.value_x, 0.0);
        assert_eq!(at_min.value_width, 152.0);

        let at_max = stepper_geometry(false, true);
        assert_eq!(at_max.increment_x, 136.0);
        assert_eq!(at_max.value_x, 64.0);
        assert_eq!(at_max.value_width, 152.0);

        let both_ends = stepper_geometry(true, true);
        assert_eq!(both_ends.value_x, 0.0);
        assert_eq!(both_ends.value_width, STEPPER_WIDTH);
    }

    // ---- Layout and paint -------------------------------------------------

    #[test]
    fn the_footprint_is_fixed_whatever_the_constraints() {
        let mut w = build(&view(5.0));
        assert_eq!(layout(&mut w), BOX);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        assert_eq!(
            w.layout(&mut lctx, &BoxConstraints::loose(Size::new(900.0, 900.0))),
            BOX
        );
    }

    #[test]
    fn a_stepper_at_its_minimum_hides_the_minus_and_widens_the_value() {
        let mut w = build(&view(0.0));
        layout(&mut w);
        assert!(w.at_min() && !w.at_max());
        assert_eq!(w.value_rect().width(), VALUE_WIDTH_WIDE);
        assert_eq!(w.button_rect(Side::Decrement).x0, DECREMENT_X_HIDDEN);
        assert_eq!(w.decrement_shown.value(), 0.0, "its glyph is gone");
        assert_eq!(w.increment_shown.value(), 1.0);
        assert!(!w.active(Side::Decrement));
        assert!(w.active(Side::Increment));
    }

    #[test]
    fn a_stepper_at_both_ends_gives_the_value_the_whole_footprint() {
        let mut w = build(
            &adaptive_stepper::<App, _>(4.0, |_: &mut App, _| {})
                .min(4.0)
                .max(4.0),
        );
        layout(&mut w);
        assert_eq!(w.value_rect().x0, 0.0);
        assert_eq!(w.value_rect().width(), STEPPER_WIDTH);
    }

    #[test]
    fn a_value_change_springs_the_geometry_toward_its_new_slot() {
        let mut w = build(&view(1.0));
        layout(&mut w);
        assert_eq!(w.value_rect().x0, VALUE_X_CENTRED);

        revalue(&mut w, 1.0, 0.0);
        assert_eq!(w.value_x.target(), 0.0, "the pill is heading left");
        assert_eq!(w.value_width.target(), VALUE_WIDTH_WIDE);

        // Mid-flight it is genuinely between the two slots...
        paint_at(&mut w, None, 0.0);
        let (_, owes) = paint_at(&mut w, None, 200.0);
        assert!(owes, "an unsettled geometry owes a frame");
        let travelling = w.value_rect().width();
        assert!(
            travelling > VALUE_WIDTH_NARROW && travelling <= VALUE_WIDTH_FULL,
            "mid-flight width: {travelling}"
        );

        // ...and it lands exactly on it.
        let (_, owes) = paint_at(&mut w, None, 5_000.0);
        assert!(!owes, "a settled stepper owes no frame");
        assert_eq!(w.value_rect().width(), VALUE_WIDTH_WIDE);
        assert_eq!(w.value_rect().x0, 0.0);
    }

    /// Both directions roll, and they roll opposite ways — the acceptance
    /// property this component's motion is about.
    #[test]
    fn the_value_rolls_in_the_direction_it_changed() {
        let mut w = build(&view(5.0));
        layout(&mut w);

        revalue(&mut w, 5.0, 6.0);
        assert_eq!(w.roll.as_ref().expect("an upward roll").direction, 1.0);
        paint_at(&mut w, None, 0.0);
        let (mid, owes) = paint_at(&mut w, None, 60.0);
        assert!(owes, "a running roll owes a frame");
        assert!(
            mid.layers.iter().any(|a| *a > 0.0 && *a < 1.0),
            "both digits are partly transparent mid-roll: {:?}",
            mid.layers
        );

        // Past the enter ramp the roll is finished and dropped.
        paint_at(&mut w, None, 5_000.0);
        assert!(w.roll.is_none());

        revalue(&mut w, 6.0, 5.0);
        assert_eq!(
            w.roll.as_ref().expect("a downward roll").direction,
            -1.0,
            "the other direction"
        );
    }

    #[test]
    fn a_press_scales_the_button_it_landed_on() {
        let mut w = build(&view(5.0));
        layout(&mut w);
        let mut app = App::default();
        let plus = w.button_rect(Side::Increment).center();
        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Down, plus.x, plus.y),
        );
        assert_eq!(w.captured, Some(Side::Increment));
        assert_eq!(w.increment_press.target(), 1.0);
        assert_eq!(w.decrement_press.target(), 0.0);

        paint_at(&mut w, None, 0.0);
        let (rec, _) = paint_at(&mut w, None, 40.0);
        assert_eq!(rec.transforms.len(), 2, "one transform per button");
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn a_release_on_a_button_reports_the_requested_value_without_self_mutating() {
        let mut w = build(&view(5.0));
        layout(&mut w);
        let mut app = App::default();

        let plus = w.button_rect(Side::Increment).center();
        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Down, plus.x, plus.y),
        );
        dispatch(&mut w, &mut app, &pointer(PointerPhase::Up, plus.x, plus.y));
        assert_eq!(app.requested, vec![6.0]);
        assert_eq!(w.value, 5.0, "controlled: the widget never moves itself");

        let minus = w.button_rect(Side::Decrement).center();
        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Down, minus.x, minus.y),
        );
        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Up, minus.x, minus.y),
        );
        assert_eq!(app.requested, vec![6.0, 4.0]);
    }

    #[test]
    fn a_release_off_the_button_and_a_cancel_report_nothing() {
        let mut w = build(&view(5.0));
        layout(&mut w);
        let mut app = App::default();
        let plus = w.button_rect(Side::Increment).center();

        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Down, plus.x, plus.y),
        );
        dispatch(&mut w, &mut app, &pointer(PointerPhase::Up, 100.0, 24.0));
        assert!(app.requested.is_empty());

        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Down, plus.x, plus.y),
        );
        dispatch(
            &mut w,
            &mut app,
            &pointer(PointerPhase::Cancel, plus.x, plus.y),
        );
        assert!(app.requested.is_empty());
    }

    /// A button with nothing left to do is not a hit at all — upstream's
    /// `disabled={hidden}` plus `pointer-events-none`.
    #[test]
    fn a_button_at_its_end_takes_no_press() {
        let mut w = build(&view(0.0));
        layout(&mut w);
        let mut app = App::default();
        let minus = w.button_rect(Side::Decrement).center();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                &pointer(PointerPhase::Down, minus.x, minus.y)
            ),
            EventResult::Ignored
        );
        assert!(app.requested.is_empty());
    }

    #[test]
    fn a_disabled_stepper_reports_nothing_from_pointer_or_keyboard() {
        let mut w = build(&view(5.0).disabled(true));
        layout(&mut w);
        let mut app = App::default();
        let plus = w.button_rect(Side::Increment).center();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                &pointer(PointerPhase::Down, plus.x, plus.y)
            ),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(&mut w, &mut app, &key(NamedKey::ArrowUp)),
            EventResult::Ignored
        );
        assert!(app.requested.is_empty());
    }

    #[test]
    fn the_arrow_keys_step_both_ways_and_stop_at_the_ends() {
        let mut w = build(&view(5.0));
        layout(&mut w);
        let mut app = App::default();
        for (named, want) in [
            (NamedKey::ArrowUp, 6.0),
            (NamedKey::ArrowRight, 6.0),
            (NamedKey::ArrowDown, 4.0),
            (NamedKey::ArrowLeft, 4.0),
        ] {
            assert_eq!(
                dispatch(&mut w, &mut app, &key(named)),
                EventResult::Handled
            );
            assert_eq!(app.requested.last().copied(), Some(want));
        }
        assert_eq!(
            dispatch(&mut w, &mut app, &key(NamedKey::Escape)),
            EventResult::Ignored,
            "every other key falls through"
        );

        // At an end the key is declined rather than reporting a no-op change.
        let mut ends = build(&view(10.0));
        layout(&mut ends);
        let mut app = App::default();
        assert_eq!(
            dispatch(&mut ends, &mut app, &key(NamedKey::ArrowUp)),
            EventResult::Ignored
        );
        assert!(app.requested.is_empty());
    }

    #[test]
    fn a_formatter_renders_the_value_and_a_bare_stepper_prints_the_number() {
        let plain = view(3.0);
        assert_eq!(plain.value_text(), "3");
        let fractional = adaptive_stepper::<App, _>(2.5, |_: &mut App, _| {}).step(0.5);
        assert_eq!(fractional.value_text(), "2.5");
        let formatted = view(3.0).format(|value| format!("{value} items"));
        assert_eq!(formatted.value_text(), "3 items");
    }

    /// A value the app hands down outside the range is shown clamped, and a
    /// non-finite one falls back to the lower end.
    #[test]
    fn a_supplied_value_is_clamped_into_the_range() {
        assert_eq!(view(99.0).current(), 10.0);
        assert_eq!(view(-4.0).current(), 0.0);
        assert_eq!(view(f64::NAN).current(), 0.0);

        // Degenerate bounds collapse onto the lower end rather than inverting.
        let collapsed = adaptive_stepper::<App, _>(5.0, |_: &mut App, _| {})
            .min(4.0)
            .max(1.0);
        assert_eq!(collapsed.bounds(), (4.0, 4.0, 1.0));
        assert_eq!(collapsed.current(), 4.0);

        // A non-positive stride falls back to 1.
        let bad_step = adaptive_stepper::<App, _>(1.0, |_: &mut App, _| {}).step(0.0);
        assert_eq!(bad_step.bounds().2, 1.0);
    }

    #[test]
    fn reduce_motion_lands_the_geometry_and_the_roll_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut w = build(&view(1.0));
        layout(&mut w);
        revalue(&mut w, 1.0, 0.0);
        let (_, owes) = paint_at(&mut w, Some(&theme), 0.0);
        assert!(!owes, "a collapsed stepper owes no frame");
        assert!(w.roll.is_none(), "no roll to play");
        assert_eq!(w.value_rect().width(), VALUE_WIDTH_WIDE);
        assert_eq!(w.button_rect(Side::Decrement).x0, DECREMENT_X_HIDDEN);
    }

    // ---- Typeface: the value's family follows the live theme ---------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// A stepper mid-range, so its value paints between both buttons.
    fn probe_view(_: &mut ()) -> AdaptiveStepperView<()> {
        adaptive_stepper::<(), _>(3.0, |_: &mut (), _| {})
            .min(0.0)
            .max(10.0)
            .label("Quantity")
    }

    #[test]
    fn the_value_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the stepper's value", probe_view, BOX);
    }

    #[test]
    fn the_value_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the stepper's value", probe_view, BOX);
    }
}
