//! Ports beUI's `otp-input` composed block — the segmented one-time-code field
//! whose digits roll into their slots, whose row shakes on a rejected code and
//! whose success check draws itself on.
//!
//! Source: `components/motion/otp-input.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `otp-input` (blocks): *"One-time-code input with a gliding focus ring,
//! digits that roll in per slot, error shake and a success check draw."*
//!
//! | class / prop | here |
//! |---|---|
//! | wrapper `inline-flex flex-col gap-2` | the label / slots / message column, [`OTP_ROW_GAP`] apart |
//! | label `text-sm font-medium text-foreground` | [`style::TEXT_SM`], `FontWeight::MEDIUM`, `on_surface` |
//! | slot `h-14 w-12 rounded-xl border` | [`OTP_SLOT_HEIGHT`] / [`OTP_SLOT_WIDTH`] / [`style::RADIUS_XL`] / [`style::BORDER_WIDTH`] |
//! | slot row `flex items-center gap-2` | [`OTP_SLOT_GAP`] |
//! | slot `text-xl font-semibold tabular-nums` | [`OTP_DIGIT_SIZE`], `FontWeight::SEMI_BOLD` |
//! | `border-border` / `text-muted-foreground` (empty) | `outline_variant` / `on_surface_variant` |
//! | `border-border-strong` / `text-foreground` (filled) | `outline` / `on_surface` |
//! | `border-foreground` (active) | `on_surface` |
//! | `border-destructive/60` (error) | `error` at [`OTP_STATUS_BORDER_ALPHA`] |
//! | `border-emerald-500/60` (success) | the `--success` token at [`OTP_STATUS_BORDER_ALPHA`] |
//! | `transition-colors duration-200` | [`switch::TRACK_COLOR_RAMP`] on every blend |
//! | caret `h-6 w-px bg-foreground`, `opacity: [1,1,0,0]` over 1s | [`OTP_CARET_HEIGHT`] / [`OTP_CARET_WIDTH`] / [`OTP_BLINK_MS`] |
//! | digit `y: 14 → 0`, `opacity: 0 → 1`, `duration: 0.22` | [`OTP_ROLL_RISE`] / [`OTP_ROLL_MS`] |
//! | `x: [0, -5, 5, -3, 3, -1, 0]`, `duration: 0.45` | [`OTP_SHAKE_KEYFRAMES`] / [`OTP_SHAKE_MS`] |
//! | check `-right-7 h-5 w-5 strokeWidth=3`, `pathLength 0 → 1` | [`OTP_CHECK_INSET`] / [`OTP_CHECK_SIZE`] / [`OTP_CHECK_DRAW_MS`] |
//! | check `spring stiffness 500 damping 28` | [`OTP_CHECK_POP`] |
//! | message `text-sm` | [`style::TEXT_SM`] in the status ink |
//! | `disabled && "opacity-50"` | [`style::DISABLED_OPACITY`] |
//!
//! # One control, not N
//!
//! Upstream paints N presentational boxes behind **one** transparent `<input>`
//! that owns focus, the soft keyboard and the caret. The same shape holds here:
//! this is a single focus target and a single widget, the slots are paint, and
//! there are no child pods at all — the sibling catalog's `input_otp` port
//! (`frust_shadcn::components::input_otp`) established that reading and this one
//! follows it.
//!
//! # Slots are holes, not a string
//!
//! Upstream's source of truth is a fixed-length array, *not* the joined string:
//! clearing a middle slot leaves an in-place hole rather than collapsing the
//! digits after it. That is preserved here — [`OtpInputWidget`] retains a
//! `Vec<Option<char>>` — with the controlled round-trip handled the way upstream
//! handles it: an incoming `value` is adopted only when it differs from the code
//! this widget last reported, so an app echoing its own `on_change` back does
//! not flatten a hole, while an app that *rejects* or transforms an edit still
//! wins on the next rebuild (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! # Typed entry only — no paste, no autofill
//!
//! Upstream has three insertion paths: keystrokes, `onPaste`, and the SMS
//! one-time-code autofill that arrives as a whole `onChange` value. **Only the
//! first exists here.** frust delivers no clipboard event a widget can read and
//! no one-time-code autofill signal, so the multi-digit `insert` arm has nothing
//! to feed it. This is the same limitation the sibling catalog's `input_otp`
//! port records, inherited unchanged; an app that needs a code pasted sets
//! `value` itself.
//!
//! # Degradations
//!
//! - **No blur on the digit roll.** `filter: "blur(4px)"` has no counterpart in
//!   this scene's paint vocabulary; the rise-and-fade is ported and the blur is
//!   dropped — the same call `input`'s message row makes.
//! - **No exit roll.** A cleared slot's digit leaves by fading in place rather
//!   than rolling upward: an exit needs the outgoing glyph kept alive alongside
//!   the incoming one, and a slot holds a single shaped run.
//! - **The "gliding focus ring" is the registry blurb, not the source.**
//!   `otp-input.tsx` swaps the active slot's border colour and paints a caret;
//!   nothing glides between slots. The port matches the source.
//! - **The hidden input's own affordances are absent** — `autoFocus`,
//!   `inputMode="numeric"` and `autoComplete="one-time-code"` are all properties
//!   of the `<input>` this port does not have. Naming the control for assistive
//!   tech is [`OtpInputView::aria_label`]'s job.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene,
    Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget,
    erase_callback_arg,
};
use frust::{Curve, FrameTime, SpringDescription, Theme};

use crate::components::checkbox::{ICON_VIEWBOX, mark_path};
use crate::components::switch;
use crate::motion::Ramp;
use crate::press::{Lane, inside_inclusive, keyframes_at, lerp_color, presses};
use crate::style;
use crate::text::{Label, LabelRun, SHAPING_INK};
use crate::tokens::motion::EASE_OUT;
use crate::tokens::{BeuiTokens, sans_family};

// ---- Ported metrics --------------------------------------------------------

/// Number of slots a bare [`otp_input`] renders (`length = 6`).
pub const OTP_DEFAULT_LENGTH: usize = 6;

/// A slot's height, in logical px (`h-14`).
pub const OTP_SLOT_HEIGHT: f64 = 56.0;
/// A slot's width, in logical px (`w-12`).
pub const OTP_SLOT_WIDTH: f64 = 48.0;
/// Gap between slots, in logical px (`gap-2` on the slot row).
pub const OTP_SLOT_GAP: f64 = 8.0;
/// Gap between the label, the slot row and the message, in logical px
/// (`flex-col gap-2`).
pub const OTP_ROW_GAP: f64 = 8.0;

/// A digit's type size, in logical px (`text-xl`).
pub const OTP_DIGIT_SIZE: f64 = 20.0;

/// The caret's height, in logical px (`h-6`).
pub const OTP_CARET_HEIGHT: f64 = 24.0;
/// The caret's width, in logical px (`w-px`).
pub const OTP_CARET_WIDTH: f64 = 1.0;
/// How far the caret sits in from a **filled** slot's trailing edge, in logical
/// px (`right-3`); an empty slot centres it instead.
pub const OTP_CARET_INSET: f64 = 12.0;
/// The caret's blink period, in milliseconds (`duration: 1` on the repeating
/// `opacity: [1, 1, 0, 0]` keyframe array).
pub const OTP_BLINK_MS: u64 = 1_000;

/// How far a digit rises into its slot, in logical px (`y: 14 → 0`).
pub const OTP_ROLL_RISE: f64 = 14.0;
/// How long a digit's roll takes, in milliseconds (`duration: 0.22`).
pub const OTP_ROLL_MS: u64 = 220;

/// The error shake's keyframes, in logical px — upstream's literal `x` array.
///
/// Deliberately **not** the field's array: this block shakes by
/// `[0, -5, 5, -3, 3, -1, 0]` where `input` shakes by `[0, -6, 6, -4, 4, -2, 0]`,
/// and collapsing the two would silently change one of them.
pub const OTP_SHAKE_KEYFRAMES: [f64; 7] = [0.0, -5.0, 5.0, -3.0, 3.0, -1.0, 0.0];
/// How long the error shake takes, in milliseconds (`duration: 0.45`).
pub const OTP_SHAKE_MS: f64 = 450.0;

/// The success check's edge, in logical px (`h-5 w-5`).
pub const OTP_CHECK_SIZE: f64 = style::ICON_SIZE_LG;
/// How far the success check's box sits **past** the slot row's trailing edge,
/// in logical px (`-right-7`).
pub const OTP_CHECK_INSET: f64 = 28.0;
/// The success check's stroke width in viewBox units (`strokeWidth={3}`).
pub const OTP_CHECK_STROKE_VIEWBOX: f64 = 3.0;
/// The success check's vertices, in viewBox units: `M5 13l4 4L19 7`.
pub const OTP_CHECK_POINTS: [Point; 3] = [
    Point::new(5.0, 13.0),
    Point::new(9.0, 17.0),
    Point::new(19.0, 7.0),
];
/// How long the success check draws itself in, in milliseconds
/// (`duration: 0.35, ease: EASE_OUT`).
pub const OTP_CHECK_DRAW_MS: f64 = 350.0;
/// How long the success check waits before drawing, in milliseconds
/// (`delay: 0.1`).
pub const OTP_CHECK_DRAW_DELAY_MS: f64 = 100.0;
/// The spring the success check pops in on (`stiffness: 500, damping: 28`).
///
/// Authored here rather than read from [`crate::tokens::motion`] because the
/// pairing is this file's own — none of the catalog's six springs carries it.
pub const OTP_CHECK_POP: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 500.0,
    damping: 28.0,
};
/// The scale the success check pops in from (`scale: 0.6 → 1`).
pub const OTP_CHECK_POP_FROM: f64 = 0.6;

/// Alpha the error/success border is painted at (`border-destructive/60`,
/// `border-emerald-500/60`).
pub const OTP_STATUS_BORDER_ALPHA: f32 = 0.6;

/// The glyph a masked slot paints instead of its digit (`mask ? "•" : char`).
pub const OTP_MASK_CHAR: char = '\u{2022}';

/// The digit-roll ramp.
const ROLL_RAMP: Ramp = Ramp::eased(Duration::from_millis(OTP_ROLL_MS), EASE_OUT);

/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback emphasized hairline — the light table's `--border-strong`.
const FALLBACK_BORDER_STRONG: Color = crate::BEUI_LIGHT.border_strong;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dim ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback error hue — the light table's `--destructive`.
const FALLBACK_DESTRUCTIVE: Color = crate::BEUI_LIGHT.destructive;

// ---- Public axes -----------------------------------------------------------

/// The external validation state a code is shown in — upstream's `OTPStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OtpStatus {
    /// No verdict yet: ordinary chrome, and the hint (if any) below.
    #[default]
    Idle,
    /// Rejected: the destructive border, the row shake, the error message.
    Error,
    /// Accepted: the success border, the check draw, the success message.
    Success,
}

impl OtpStatus {
    /// Whether this state draws the success check.
    const fn is_success(self) -> bool {
        matches!(self, OtpStatus::Success)
    }
}

// ---- View ------------------------------------------------------------------

/// A view-held, typed code callback (erased on build).
type OnCode<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI one-time-code field. See the [module docs](self).
pub struct OtpInputView<State: 'static> {
    value: String,
    length: usize,
    status: OtpStatus,
    label: Option<String>,
    hint: Option<String>,
    success_message: Option<String>,
    error_message: Option<String>,
    aria_label: String,
    mask: bool,
    disabled: bool,
    on_change: OnCode<State>,
    on_complete: Option<OnCode<State>>,
}

/// Create a [`OTP_DEFAULT_LENGTH`]-slot code field showing `value` and
/// reporting every requested code through `on_change` — a **controlled**
/// component (see the [module docs](self)).
pub fn otp_input<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> OtpInputView<State> {
    OtpInputView {
        value: value.into(),
        length: OTP_DEFAULT_LENGTH,
        status: OtpStatus::default(),
        label: None,
        hint: None,
        success_message: None,
        error_message: None,
        // `"aria-label": ariaLabel = "One-time passcode"`.
        aria_label: "One-time passcode".to_string(),
        mask: false,
        disabled: false,
        on_change: Rc::new(on_change),
        on_complete: None,
    }
}

impl<State: 'static> OtpInputView<State> {
    /// Set the slot count (`length`), clamped to at least one slot.
    pub fn length(mut self, length: usize) -> Self {
        self.length = length.max(1);
        self
    }

    /// Set the external validation state (`status`) — see [`OtpStatus`].
    pub fn status(mut self, status: OtpStatus) -> Self {
        self.status = status;
        self
    }

    /// Set the label row above the slots (`label`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the helper text shown below the slots while idle (`hint`).
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Set the message shown below the slots in [`OtpStatus::Success`]
    /// (`successMessage`).
    pub fn success_message(mut self, message: impl Into<String>) -> Self {
        self.success_message = Some(message.into());
        self
    }

    /// Set the message shown below the slots in [`OtpStatus::Error`]
    /// (`errorMessage`).
    pub fn error_message(mut self, message: impl Into<String>) -> Self {
        self.error_message = Some(message.into());
        self
    }

    /// Name the control for assistive tech (`aria-label`), replacing the
    /// upstream default `"One-time passcode"`.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = label.into();
        self
    }

    /// Paint [`OTP_MASK_CHAR`] in place of each digit (`mask`).
    pub fn mask(mut self, mask: bool) -> Self {
        self.mask = mask;
        self
    }

    /// Disable the control: 50% opacity, inert to pointer and key, and a
    /// not-allowed cursor (`disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Report the completed code the first time an edit fills every slot
    /// (`onComplete`). Fired *after* `on_change`, with the same string.
    pub fn on_complete<F: Fn(&mut State, String) + 'static>(mut self, on_complete: F) -> Self {
        self.on_complete = Some(Rc::new(on_complete));
        self
    }

    /// The message the current status puts below the slots.
    fn message(&self) -> Option<&String> {
        match self.status {
            OtpStatus::Success => self.success_message.as_ref(),
            OtpStatus::Error => self.error_message.as_ref(),
            OtpStatus::Idle => self.hint.as_ref(),
        }
    }
}

// ---- Widget ----------------------------------------------------------------

/// One painted slot: its shaped digit and the roll that brought it in.
struct Slot {
    /// The glyph on screen (empty while the slot is a hole).
    run: LabelRun,
    /// `0.0` empty .. `1.0` filled — drives both the digit's rise/fade and the
    /// slot's idle → filled border crossfade.
    fill: Lane,
}

impl Slot {
    fn empty() -> Self {
        Slot {
            run: LabelRun::new(String::new()),
            fill: Lane::at_rest(ROLL_RAMP, 0.0),
        }
    }
}

/// The retained widget for an [`OtpInputView`].
pub struct OtpInputWidget {
    /// The digit array — `None` is a hole, the shape upstream's fixed-length
    /// array has and a joined string does not.
    slots: Vec<Option<char>>,
    /// Paint state, one per slot; always the same length as `slots`.
    cells: Vec<Slot>,
    length: usize,
    status: OtpStatus,
    mask: bool,
    disabled: bool,
    aria_label: String,
    label: Option<Label>,
    message: Option<Label>,
    /// The slot the caret sits in (`active` upstream), always `< length`.
    active: usize,
    /// The code this widget last reported, so an app echoing it back does not
    /// flatten an in-place hole (upstream's `joinedRef`).
    reported: String,
    /// The status crossfade, `0.0` idle .. `1.0` at `blend_target`'s hue.
    status_blend: Lane,
    /// Which status `status_blend` is blending toward — the colour comes from
    /// this, the lane only says how far.
    blend_target: OtpStatus,
    /// An error that has just arrived and owes a shake; `paint` stamps the
    /// start (neither a `BuildCtx` nor an `EventCtx` carries a clock).
    shake_armed: bool,
    shake_start: Option<FrameTime>,
    /// The success check's pop/draw: armed by `rebuild`, stamped by `paint`.
    check_armed: bool,
    check_start: Option<FrameTime>,
    /// The frame the current caret blink cycle is measured from.
    blink_epoch: FrameTime,
    /// Set by any edit or caret move; the next paint records the epoch, since
    /// the event pass has no clock.
    blink_reset_pending: bool,
    /// Armed by a `Down` inside the slot row.
    captured: bool,
    /// The slot row's box, resolved by layout and read by the event pass.
    row: Rect,
    on_change: ErasedArgCallback<String>,
    on_complete: Option<ErasedArgCallback<String>>,
}

impl<State: 'static> View<State> for OtpInputView<State> {
    type Element = OtpInputWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> OtpInputWidget {
        let mut widget = OtpInputWidget {
            slots: Vec::new(),
            cells: Vec::new(),
            length: self.length,
            status: self.status,
            mask: self.mask,
            disabled: self.disabled,
            aria_label: self.aria_label.clone(),
            label: self.label.as_ref().map(Label::new),
            message: self.message().map(Label::new),
            active: 0,
            reported: String::new(),
            status_blend: Lane::at_rest(
                switch::TRACK_COLOR_RAMP,
                if self.status == OtpStatus::Idle {
                    0.0
                } else {
                    1.0
                },
            ),
            blend_target: self.status,
            // `<AnimatePresence initial={false}>`: a field that mounts errored
            // does not shake, and one that mounts successful is already drawn.
            shake_armed: false,
            shake_start: None,
            check_armed: false,
            check_start: None,
            blink_epoch: FrameTime::ZERO,
            blink_reset_pending: true,
            captured: false,
            row: Rect::ZERO,
            on_change: erase_callback_arg(&self.on_change),
            on_complete: self.on_complete.as_ref().map(erase_callback_arg),
        };
        widget.resize(self.length);
        widget.adopt(&self.value);
        // A code that mounts filled rests filled rather than rolling in.
        for cell in &mut widget.cells {
            cell.fill.snap();
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OtpInputWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so both adapters are reinstalled each pass.
        element.on_change = erase_callback_arg(&self.on_change);
        element.on_complete = self.on_complete.as_ref().map(erase_callback_arg);

        let mut flags = ChangeFlags::NONE;

        if prev.length != self.length {
            element.resize(self.length);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.mask != self.mask {
            element.mask = self.mask;
            element.reshape_slots();
            flags |= ChangeFlags::PAINT;
        }
        // The app is the source of truth — but only when it is saying something
        // new: an echo of this widget's own last report leaves the holes alone.
        if element.adopt(&self.value) {
            flags |= ChangeFlags::PAINT;
        }

        if prev.status != self.status {
            element.status = self.status;
            if self.status == OtpStatus::Idle {
                element.status_blend.retarget(0.0);
            } else {
                // A status swapping straight from one verdict to the other is
                // put back at idle first, so the new hue is genuinely
                // crossfaded to rather than snapping in place.
                if element.blend_target != self.status {
                    element.status_blend.retarget(0.0);
                    element.status_blend.snap();
                }
                element.blend_target = self.status;
                element.status_blend.retarget(1.0);
            }
            if self.status == OtpStatus::Error {
                element.shake_armed = true;
                element.shake_start = None;
            }
            if self.status.is_success() {
                element.check_armed = true;
                element.check_start = None;
            }
            flags |= ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            set_label(&mut element.label, self.label.as_deref());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.message() != self.message() || prev.status != self.status {
            set_label(&mut element.message, self.message().map(String::as_str));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.aria_label != self.aria_label {
            element.aria_label = self.aria_label.clone();
            flags |= ChangeFlags::PAINT;
        }

        flags
    }
}

/// Install `text` into a cached-run slot, re-shaping only when it changed.
fn set_label(slot: &mut Option<Label>, text: Option<&str>) {
    match (slot.as_mut(), text) {
        (Some(existing), Some(text)) => {
            existing.set_content(text);
        }
        (None, Some(text)) => *slot = Some(Label::new(text)),
        (_, None) => *slot = None,
    }
}

impl OtpInputWidget {
    /// The digits as one string, holes skipped — upstream's `slots.join("")`.
    fn joined(&self) -> String {
        self.slots.iter().flatten().collect()
    }

    /// Whether every slot carries a digit.
    fn is_complete(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }

    /// Grow or shrink to `length` slots, keeping the digits that survive.
    fn resize(&mut self, length: usize) {
        let length = length.max(1);
        self.length = length;
        self.slots.resize(length, None);
        while self.cells.len() < length {
            self.cells.push(Slot::empty());
        }
        self.cells.truncate(length);
        self.active = self.active.min(length - 1);
        self.reported = self.joined();
        self.reshape_slots();
    }

    /// Adopt an app-confirmed code, skipping the echo of this widget's own last
    /// report — the in-place-hole preservation the [module docs](self) explain.
    /// Reports whether anything changed.
    fn adopt(&mut self, value: &str) -> bool {
        if value == self.reported {
            return false;
        }
        let digits: Vec<char> = value.chars().filter(char::is_ascii_digit).collect();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            *slot = digits.get(index).copied();
        }
        self.reported = self.joined();
        self.retarget_fills();
        self.reshape_slots();
        true
    }

    /// Re-key every slot's shaped run from the digit it now holds.
    fn reshape_slots(&mut self) {
        for (cell, digit) in self.cells.iter_mut().zip(self.slots.iter()) {
            let glyph = match digit {
                Some(_) if self.mask => OTP_MASK_CHAR.to_string(),
                Some(c) => c.to_string(),
                None => String::new(),
            };
            cell.run.set_content(glyph);
        }
    }

    /// Point every fill lane at its slot's current occupancy.
    fn retarget_fills(&mut self) {
        for (cell, digit) in self.cells.iter_mut().zip(self.slots.iter()) {
            cell.fill.retarget(if digit.is_some() { 1.0 } else { 0.0 });
        }
    }

    /// Report the current digits through `on_change`, and through `on_complete`
    /// too on the empty → full transition (`!wasComplete && next.every(...)`).
    fn commit(&mut self, ctx: &mut EventCtx, was_complete: bool) {
        let next = self.joined();
        self.reported = next.clone();
        self.retarget_fills();
        self.reshape_slots();
        (self.on_change)(ctx, next.clone());
        if !was_complete
            && self.is_complete()
            && let Some(on_complete) = self.on_complete.as_mut()
        {
            on_complete(ctx, next);
        }
    }

    /// Write `digit` into the active slot and advance — upstream's single-digit
    /// `insert` (the multi-digit arm has no route in; see the module docs).
    fn insert(&mut self, ctx: &mut EventCtx, digit: char) {
        let was_complete = self.is_complete();
        self.slots[self.active] = Some(digit);
        self.active = (self.active + 1).min(self.length - 1);
        self.commit(ctx, was_complete);
    }

    /// Clear slot `index`, leaving a hole.
    fn clear_slot(&mut self, ctx: &mut EventCtx, index: usize) {
        let was_complete = self.is_complete();
        self.slots[index] = None;
        self.commit(ctx, was_complete);
    }

    /// `Backspace`: a filled slot clears in place; an empty one steps back and
    /// clears there.
    fn backspace(&mut self, ctx: &mut EventCtx) {
        if self.slots[self.active].is_some() {
            self.clear_slot(ctx, self.active);
        } else if self.active > 0 {
            let previous = self.active - 1;
            self.clear_slot(ctx, previous);
            self.active = previous;
        }
    }

    /// The slot a local x lands on, or `None` off the boxes entirely (in a gap,
    /// or past either end).
    fn slot_at(&self, x: f64) -> Option<usize> {
        (0..self.length).find(|index| {
            let left = slot_x(*index);
            x >= left && x < left + OTP_SLOT_WIDTH
        })
    }

    /// The slot a press at local `x` selects: upstream clamps to the first empty
    /// slot so a click cannot jump ahead of the code's own progress.
    fn press_target(&self, x: f64) -> usize {
        let cap = self
            .slots
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.length - 1);
        let landed = self
            .slot_at(x)
            .unwrap_or(if x < 0.0 { 0 } else { self.length - 1 });
        landed.min(cap)
    }

    /// The error shake's x offset at `now`, and whether it is still running.
    fn shake_offset(&self, now: FrameTime) -> (f64, bool) {
        let Some(start) = self.shake_start else {
            return (0.0, false);
        };
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
        if elapsed >= OTP_SHAKE_MS {
            return (0.0, false);
        }
        (
            keyframes_at(&OTP_SHAKE_KEYFRAMES, elapsed / OTP_SHAKE_MS),
            true,
        )
    }

    /// The success check's `(scale, drawn, owes_frame)` at `now`.
    fn check_progress(&self, now: FrameTime) -> (f64, f64, bool) {
        if !self.status.is_success() {
            return (1.0, 0.0, false);
        }
        let Some(start) = self.check_start else {
            // No run pending: a field that mounted successful is already drawn.
            return (1.0, 1.0, false);
        };
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
        let pop_t = Ramp::spring(OTP_CHECK_POP)
            .progress_clamped(Duration::from_secs_f64(elapsed.max(0.0) / 1000.0));
        let scale = OTP_CHECK_POP_FROM + (1.0 - OTP_CHECK_POP_FROM) * pop_t;
        let draw_elapsed = elapsed - OTP_CHECK_DRAW_DELAY_MS;
        let drawn = if draw_elapsed <= 0.0 {
            0.0
        } else {
            Curve::EaseOut.transform((draw_elapsed / OTP_CHECK_DRAW_MS).min(1.0))
        };
        let running = pop_t < 1.0 || draw_elapsed < OTP_CHECK_DRAW_MS;
        (scale, drawn, running)
    }

    /// Whether the caret is in its visible half-cycle at frame time `now`.
    fn caret_visible_at(&self, now: FrameTime) -> bool {
        let half = OTP_BLINK_MS as f64 / 2.0;
        let elapsed = now.saturating_sub(self.blink_epoch).as_secs_f64() * 1000.0;
        ((elapsed / half) as u64).is_multiple_of(2)
    }

    /// The width of the slot row.
    fn row_width(&self) -> f64 {
        self.length as f64 * OTP_SLOT_WIDTH + (self.length.saturating_sub(1)) as f64 * OTP_SLOT_GAP
    }

    /// The cursor this control asks for in its current state.
    fn cursor(&self) -> CursorIcon {
        if self.disabled {
            style::DISABLED_CURSOR
        } else {
            CursorIcon::Text
        }
    }
}

/// The local x of slot `index`'s left edge inside the row.
fn slot_x(index: usize) -> f64 {
    index as f64 * (OTP_SLOT_WIDTH + OTP_SLOT_GAP)
}

/// The digit style: `text-xl font-semibold`, shaped with [`SHAPING_INK`] and
/// re-brushed at paint so a status recolor costs no reshape.
fn digit_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::SEMI_BOLD,
        ..TextStyle::new(OTP_DIGIT_SIZE as f32, SHAPING_INK)
    }
}

/// The label style: `text-sm font-medium text-foreground`.
fn label_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: style::TEXT_SM as f32,
        color,
        ..TextStyle::default()
    }
}

/// The message style: `text-sm` in the status's own ink.
fn message_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        color,
        ..TextStyle::default()
    }
}

/// The palette a slot row paints from.
struct OtpColors {
    /// `--border`: an empty slot's hairline.
    border: Color,
    /// `--border-strong`: a filled slot's hairline.
    border_strong: Color,
    /// `--foreground`: the active slot's hairline, the caret, a filled digit.
    ink: Color,
    /// `--muted-foreground`: an empty slot's ink and the idle message.
    muted: Color,
    /// `--destructive`.
    destructive: Color,
    /// `--success`.
    success: Color,
}

impl OtpColors {
    /// The colour the given status blends the borders and the message toward.
    fn status_hue(&self, status: OtpStatus) -> Color {
        match status {
            OtpStatus::Error => self.destructive,
            OtpStatus::Success => self.success,
            OtpStatus::Idle => self.muted,
        }
    }
}

/// Resolve the palette, falling back to the light table with no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> OtpColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            OtpColors {
                border: scheme.outline_variant,
                border_strong: scheme.outline,
                ink: scheme.on_surface,
                muted: scheme.on_surface_variant,
                destructive: scheme.error,
                success: BeuiTokens::resolve(Some(theme)).success,
            }
        }
        None => OtpColors {
            border: FALLBACK_BORDER,
            border_strong: FALLBACK_BORDER_STRONG,
            ink: FALLBACK_FOREGROUND,
            muted: FALLBACK_MUTED,
            destructive: FALLBACK_DESTRUCTIVE,
            success: BeuiTokens::beui().success,
        },
    }
}

impl Widget for OtpInputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let tint =
            |color: Color| style::disabled_tint(color, self.disabled, style::DISABLED_OPACITY);

        let digit = digit_style();
        for cell in &mut self.cells {
            if !cell.run.content().is_empty() {
                cell.run.layout(ctx, &digit);
            }
        }

        let label_size = match &mut self.label {
            Some(label) => label.layout(ctx, &label_style(tint(colors.ink))),
            None => Size::ZERO,
        };
        let message_ink = tint(colors.status_hue(self.status));
        let message_size = match &mut self.message {
            Some(message) => message.layout(ctx, &message_style(message_ink)),
            None => Size::ZERO,
        };

        let mut y = 0.0;
        if self.label.is_some() {
            y += label_size.height + OTP_ROW_GAP;
        }
        self.row = Rect::from_origin_size(
            Point::new(0.0, y),
            Size::new(self.row_width(), OTP_SLOT_HEIGHT),
        );
        y += OTP_SLOT_HEIGHT;
        if self.message.is_some() {
            y += OTP_ROW_GAP + message_size.height;
        }

        // `inline-flex`: the column is as wide as its widest row.
        let width = self
            .row_width()
            .max(label_size.width)
            .max(message_size.width);
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        if self.blink_reset_pending {
            self.blink_epoch = now;
            self.blink_reset_pending = false;
        }
        let focused = ctx.has_focus() && !self.disabled;
        let origin = ctx.origin();

        // One scope for every theme read: a live `&Theme` borrows the context
        // immutably and the frame requests below need it mutably.
        let (colors, reduce) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };
        let tint =
            |color: Color| style::disabled_tint(color, self.disabled, style::DISABLED_OPACITY);

        let mut owes_frame = false;
        if reduce {
            self.status_blend.snap();
            for cell in &mut self.cells {
                cell.fill.snap();
            }
            self.shake_armed = false;
            self.shake_start = None;
            self.check_armed = false;
            self.check_start = None;
        } else {
            if self.shake_armed {
                self.shake_armed = false;
                self.shake_start = Some(now);
            }
            if self.check_armed {
                self.check_armed = false;
                self.check_start = Some(now);
            }
            owes_frame |= self.status_blend.advance(now);
            for cell in &mut self.cells {
                owes_frame |= cell.fill.advance(now);
            }
        }

        let (shake, shaking) = self.shake_offset(now);
        if !shaking {
            self.shake_start = None;
        }
        owes_frame |= shaking;
        let (check_scale, check_drawn, checking) = self.check_progress(now);
        if !checking {
            self.check_start = None;
        }
        owes_frame |= checking;

        let status = self.status_blend.value().clamp(0.0, 1.0);
        let status_hue = style::with_alpha(
            colors.status_hue(self.blend_target),
            OTP_STATUS_BORDER_ALPHA,
        );
        let statused = self.blend_target != OtpStatus::Idle;

        if let Some(label) = &self.label {
            label.paint(origin, scene);
        }

        // The shake moves the slot row only — upstream animates `slotsRef`, not
        // the label or the message.
        let row_origin = Point::new(origin.x + shake, origin.y + self.row.origin().y);
        let slot_size = Size::new(OTP_SLOT_WIDTH, OTP_SLOT_HEIGHT);
        let radius = style::RADIUS_XL;
        for index in 0..self.length {
            let fill = self.cells[index].fill.value().clamp(0.0, 1.0);
            let filled_border = lerp_color(colors.border, colors.border_strong, fill);
            // `isActive && !showSuccess && status !== "error"`: the active
            // slot's stronger border yields to a verdict.
            let base = if focused && index == self.active && !statused {
                colors.ink
            } else {
                filled_border
            };
            let border = if statused {
                lerp_color(base, status_hue, status)
            } else {
                base
            };

            let at = Point::new(row_origin.x + slot_x(index), row_origin.y);
            let inset = style::BORDER_WIDTH / 2.0;
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, slot_size).inset(-inset),
                (radius - inset).max(0.0),
            );
            scene.stroke_path(
                at,
                &Shape::to_path(&outline, style::PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(tint(border)),
            );

            // The digit rises into place and fades in as its slot fills.
            let cell = &self.cells[index];
            if !cell.run.content().is_empty() && fill > 0.0 {
                let size = cell.run.size();
                let rise = OTP_ROLL_RISE * (1.0 - fill);
                let ink = lerp_color(colors.muted, colors.ink, fill);
                let glyph = Point::new(
                    at.x + (OTP_SLOT_WIDTH - size.width) / 2.0,
                    at.y + (OTP_SLOT_HEIGHT - size.height) / 2.0 + rise,
                );
                scene.push_layer(at, slot_size, (fill as f32).clamp(0.0, 1.0));
                cell.run.paint(glyph, tint(ink), scene);
                scene.pop_layer();
            }

            // The caret marks the active slot: centred when empty, trailing the
            // digit when the slot is filled, and gone under a success verdict
            // (`isActive && !showSuccess`).
            if focused && index == self.active && !self.status.is_success() {
                if !reduce {
                    ctx.request_frame_paced_at(Duration::from_millis(OTP_BLINK_MS / 2));
                }
                // `reduce_motion` freezes the caret **visible** rather than
                // hidden — a position cue must stay legible, the one exception
                // the framework's own text field also makes.
                if reduce || self.caret_visible_at(now) {
                    let x = if self.slots[index].is_some() {
                        at.x + OTP_SLOT_WIDTH - OTP_CARET_INSET - OTP_CARET_WIDTH
                    } else {
                        at.x + (OTP_SLOT_WIDTH - OTP_CARET_WIDTH) / 2.0
                    };
                    scene.fill_rect(
                        Point::new(x, at.y + (OTP_SLOT_HEIGHT - OTP_CARET_HEIGHT) / 2.0),
                        Size::new(OTP_CARET_WIDTH, OTP_CARET_HEIGHT),
                        tint(colors.ink),
                    );
                }
            }
        }

        // The success check, `-right-7` of the row and vertically centred.
        if self.status.is_success() && check_drawn > 0.0 {
            let edge = OTP_CHECK_SIZE * check_scale;
            let centre = Point::new(
                row_origin.x + self.row.width() + OTP_CHECK_INSET,
                row_origin.y + OTP_SLOT_HEIGHT / 2.0,
            );
            let path = mark_path(&OTP_CHECK_POINTS, edge, check_drawn);
            scene.stroke_path(
                Point::new(centre.x - edge / 2.0, centre.y - edge / 2.0),
                &path,
                OTP_CHECK_STROKE_VIEWBOX * edge / ICON_VIEWBOX,
                &Brush::Solid(tint(colors.success)),
            );
        }

        if let Some(message) = &self.message {
            message.paint(
                Point::new(origin.x, origin.y + self.row.max_y() + OTP_ROW_GAP),
                scene,
            );
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled {
                    return EventResult::Ignored;
                }
                if self.handle_key(ctx, key) {
                    self.blink_reset_pending = true;
                    ctx.request_redraw();
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Pointer(p) => {
                let local = p.position - self.row.origin().to_vec2();
                let over_row = inside_inclusive(local, self.row.size());
                match p.phase {
                    PointerPhase::Down => {
                        if self.disabled || !presses(p) || !over_row {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        ctx.capture_pointer();
                        self.active = self.press_target(local.x);
                        self.blink_reset_pending = true;
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            // The hover/cursor pass: the claim is per-pass and
                            // the cursor request is stateless, so both are
                            // re-issued on every qualifying move.
                            if over_row {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag that
                        // wandered off the row.
                        ctx.set_cursor(self.cursor());
                        EventResult::Handled
                    }
                    PointerPhase::Up | PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // A `Cancel` arm clears internal flags only — never the
                        // digits, never a callback.
                        self.captured = false;
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One `<input>` upstream is one node here: the row is a single text
        // field carrying the whole code, not N boxes. A masked code reports the
        // bulleted mirror, never the digits — an assistive-tech client reads a
        // node's value verbatim.
        let role = if self.mask {
            Role::PasswordInput
        } else {
            Role::TextInput
        };
        let value: String = if self.mask {
            self.slots.iter().flatten().map(|_| OTP_MASK_CHAR).collect()
        } else {
            self.joined()
        };
        ctx.push_node(role, |node| {
            node.set_label(self.aria_label.as_str());
            node.set_value(value);
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Focus);
            }
        });
    }
}

impl OtpInputWidget {
    /// The keyboard surface — upstream's `onKeyDown` less its clipboard arms.
    /// Returns whether the key belonged to this control.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> bool {
        // `if (e.metaKey || e.ctrlKey || e.altKey) return`: a shortcut chord is
        // not an edit.
        let modifiers = &key.modifiers;
        if modifiers.ctrl || modifiers.alt || modifiers.meta {
            return false;
        }
        match &key.key {
            Key::Character(text) => {
                let Some(digit) = text.chars().next().filter(char::is_ascii_digit) else {
                    return false;
                };
                self.insert(ctx, digit);
                true
            }
            Key::Named(NamedKey::Backspace) => {
                self.backspace(ctx);
                true
            }
            Key::Named(NamedKey::Delete) => {
                self.clear_slot(ctx, self.active);
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.active = self.active.saturating_sub(1);
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.active = (self.active + 1).min(self.length - 1);
                true
            }
            Key::Named(NamedKey::Home) => {
                self.active = 0;
                true
            }
            Key::Named(NamedKey::End) => {
                self.active = self.length - 1;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, EventOutcome, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    /// Records the ops these tests assert on: stroked paths (slot outlines and
    /// the success check), plain fills (the caret), glyph runs and layer alphas.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        glyphs: Vec<(Point, Color)>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            let color = match run.brush {
                Brush::Solid(c) => c,
                _ => Color::TRANSPARENT,
            };
            self.glyphs.push((Point::new(t.x, t.y), color));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    impl Recorder {
        /// Every 1px slot outline of the pass, in paint order.
        fn borders(&self) -> Vec<(Rect, Color)> {
            self.strokes
                .iter()
                .filter(|(_, w, _)| *w == style::BORDER_WIDTH)
                .map(|(r, _, c)| (*r, *c))
                .collect()
        }

        /// The success check, if the pass drew one — the only stroke whose
        /// width is not the border's.
        fn check(&self) -> Option<(Rect, f64, Color)> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w != style::BORDER_WIDTH)
                .copied()
        }

        /// The caret fill, if the pass drew one.
        fn caret(&self) -> Option<(Point, Size, Color)> {
            self.rects
                .iter()
                .find(|(_, s, _)| s.width == OTP_CARET_WIDTH)
                .copied()
        }
    }

    /// What the app state records: the confirmed code plus every callback.
    #[derive(Default)]
    struct App {
        value: String,
        changes: Vec<String>,
        completes: Vec<String>,
    }

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn digit(c: char) -> InputEvent {
        key(Key::Character(c.to_string()))
    }

    fn named(k: NamedKey) -> InputEvent {
        key(Key::Named(k))
    }

    // ---- The bare-widget harness -------------------------------------------
    //
    // The editing model is asserted on the widget directly, because the slot
    // array (holes included) and the active index are exactly what a paint
    // recorder cannot see.

    /// A widget plus the controlled round trip an app performs around it.
    struct Bare {
        widget: OtpInputWidget,
        view: OtpInputView<App>,
        state: App,
        length: usize,
        status: OtpStatus,
        counter: u64,
    }

    impl Bare {
        fn new(length: usize) -> Self {
            let view = Self::view(String::new(), length, OtpStatus::Idle);
            let mut counter = 0u64;
            let widget = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut bare = Bare {
                widget,
                view,
                state: App::default(),
                length,
                status: OtpStatus::Idle,
                counter,
            };
            bare.layout();
            bare
        }

        fn view(value: String, length: usize, status: OtpStatus) -> OtpInputView<App> {
            otp_input::<App, _>(value, |s: &mut App, code| {
                s.changes.push(code.clone());
                s.value = code;
            })
            .length(length)
            .status(status)
            .on_complete(|s: &mut App, code| s.completes.push(code))
        }

        fn layout(&mut self) -> Size {
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            self.widget
                .layout(&mut lctx, &BoxConstraints::loose(Size::new(600.0, 400.0)))
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventResult {
            let size = Size::new(self.widget.row_width(), OTP_SLOT_HEIGHT);
            let state: &mut dyn Any = &mut self.state;
            let mut ctx = EventCtx::new(state, Point::ZERO, size);
            self.widget.event(&mut ctx, event)
        }

        /// The app's own rebuild: feed the confirmed value straight back down.
        fn round_trip(&mut self) {
            self.rebuild_with(self.state.value.clone(), self.length, self.status);
        }

        fn rebuild_with(&mut self, value: String, length: usize, status: OtpStatus) {
            let next = Self::view(value, length, status);
            let mut ctx = BuildCtx::new(&mut self.counter);
            View::<App>::rebuild(&next, &self.view, &mut self.widget, &mut ctx);
            self.view = next;
            self.length = length;
            self.status = status;
        }

        /// Type `c`, then let the app echo the result back.
        fn type_digit(&mut self, c: char) {
            self.dispatch(&digit(c));
            self.round_trip();
        }

        fn press_key(&mut self, k: NamedKey) {
            self.dispatch(&named(k));
            self.round_trip();
        }
    }

    // ---- Layout -------------------------------------------------------------

    #[test]
    fn a_bare_row_is_six_slots_wide_and_one_slot_tall() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        let size = bare.layout();
        let expected = OTP_DEFAULT_LENGTH as f64 * OTP_SLOT_WIDTH
            + (OTP_DEFAULT_LENGTH - 1) as f64 * OTP_SLOT_GAP;
        assert_eq!(size, Size::new(expected, OTP_SLOT_HEIGHT));
        assert_eq!(bare.widget.row, Rect::from_origin_size(Point::ORIGIN, size));
    }

    #[test]
    fn the_label_and_message_rows_add_their_own_height_and_gap() {
        let view: OtpInputView<App> = otp_input("", |_s: &mut App, _c| {})
            .label("Verification code")
            .hint("Check your messages");
        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(600.0, 400.0)));

        let label = w.label.as_ref().expect("a label was shaped").size().height;
        let message = w
            .message
            .as_ref()
            .expect("a message was shaped")
            .size()
            .height;
        assert!(label > 0.0 && message > 0.0);
        assert_eq!(
            size.height,
            label + OTP_ROW_GAP + OTP_SLOT_HEIGHT + OTP_ROW_GAP + message
        );
        assert_eq!(w.row.origin().y, label + OTP_ROW_GAP);
    }

    // ---- The editing model --------------------------------------------------

    #[test]
    fn typing_a_digit_fills_the_active_slot_and_advances() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        bare.type_digit('1');
        bare.type_digit('2');
        assert_eq!(bare.state.value, "12");
        assert_eq!(bare.state.changes, vec!["1".to_string(), "12".to_string()]);
        assert_eq!(bare.widget.active, 2);
        assert_eq!(bare.widget.slots[0], Some('1'));
        assert_eq!(bare.widget.slots[1], Some('2'));
    }

    #[test]
    fn a_non_digit_key_is_refused_and_leaves_the_code_alone() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        assert_eq!(bare.dispatch(&digit('a')), EventResult::Ignored);
        assert_eq!(bare.dispatch(&digit(' ')), EventResult::Ignored);
        assert!(bare.state.changes.is_empty());
        assert_eq!(bare.widget.active, 0);
    }

    #[test]
    fn a_modified_key_is_never_an_edit() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        let chord = InputEvent::Key(KeyEvent {
            key: Key::Character("1".into()),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        });
        assert_eq!(bare.dispatch(&chord), EventResult::Ignored);
        assert!(
            bare.state.changes.is_empty(),
            "ctrl+1 is a shortcut, not a 1"
        );
    }

    #[test]
    fn typing_past_the_last_slot_overwrites_it_rather_than_advancing() {
        let mut bare = Bare::new(2);
        bare.type_digit('1');
        bare.type_digit('2');
        bare.type_digit('3');
        assert_eq!(bare.state.value, "13");
        assert_eq!(bare.widget.active, 1);
    }

    #[test]
    fn backspace_clears_a_filled_slot_in_place_then_steps_back() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        bare.type_digit('1');
        bare.type_digit('2');
        // The active slot (2) is empty, so backspace steps back and clears 1.
        bare.press_key(NamedKey::Backspace);
        assert_eq!(bare.state.value, "1");
        assert_eq!(bare.widget.active, 1);
        // Slot 1 is empty again: it steps back once more.
        bare.press_key(NamedKey::Backspace);
        assert_eq!(bare.state.value, "");
        assert_eq!(bare.widget.active, 0);
        // Nothing left to delete — the key is still this control's.
        assert_eq!(
            bare.dispatch(&named(NamedKey::Backspace)),
            EventResult::Handled
        );
        assert_eq!(bare.state.value, "");
    }

    #[test]
    fn backspace_on_a_filled_active_slot_clears_without_stepping_back() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        bare.type_digit('1');
        bare.press_key(NamedKey::ArrowLeft);
        assert_eq!(bare.widget.active, 0);
        bare.press_key(NamedKey::Backspace);
        assert_eq!(bare.state.value, "");
        assert_eq!(bare.widget.active, 0, "a filled slot clears in place");
    }

    #[test]
    fn delete_leaves_an_in_place_hole_that_survives_the_round_trip() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        for c in ['1', '2', '3'] {
            bare.type_digit(c);
        }
        bare.press_key(NamedKey::Home);
        bare.press_key(NamedKey::ArrowRight);
        bare.press_key(NamedKey::Delete);

        // The joined code loses the middle digit...
        assert_eq!(bare.state.value, "13");
        // ...but the hole stays put rather than collapsing the 3 leftward, and
        // a second echo of the same value does not flatten it either.
        bare.round_trip();
        bare.round_trip();
        assert_eq!(bare.widget.slots[0], Some('1'));
        assert_eq!(bare.widget.slots[1], None, "the hole survives");
        assert_eq!(bare.widget.slots[2], Some('3'));
    }

    #[test]
    fn a_genuinely_new_app_value_wins_and_packs_from_the_left() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        for c in ['1', '2', '3'] {
            bare.type_digit(c);
        }
        // The app rejects the edit and says something else entirely.
        bare.rebuild_with("99".to_string(), OTP_DEFAULT_LENGTH, OtpStatus::Idle);
        assert_eq!(bare.widget.slots[0], Some('9'));
        assert_eq!(bare.widget.slots[1], Some('9'));
        assert_eq!(bare.widget.slots[2], None);
        // Non-digits in an app-supplied value are dropped, as upstream's
        // `sanitize` does.
        bare.rebuild_with("4a5".to_string(), OTP_DEFAULT_LENGTH, OtpStatus::Idle);
        assert_eq!(bare.widget.joined(), "45");
    }

    #[test]
    fn caret_keys_move_the_active_slot_without_editing() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        bare.press_key(NamedKey::End);
        assert_eq!(bare.widget.active, OTP_DEFAULT_LENGTH - 1);
        bare.press_key(NamedKey::ArrowRight);
        assert_eq!(
            bare.widget.active,
            OTP_DEFAULT_LENGTH - 1,
            "clamped at the last slot"
        );
        bare.press_key(NamedKey::Home);
        assert_eq!(bare.widget.active, 0);
        bare.press_key(NamedKey::ArrowLeft);
        assert_eq!(bare.widget.active, 0);
        assert!(bare.state.changes.is_empty(), "caret motion is not an edit");
    }

    #[test]
    fn on_complete_fires_once_on_the_empty_to_full_transition() {
        let mut bare = Bare::new(3);
        for c in ['1', '2', '3'] {
            bare.type_digit(c);
        }
        assert_eq!(bare.state.completes, vec!["123".to_string()]);
        // Editing an already-full code reports a change but not a completion.
        bare.type_digit('9');
        assert_eq!(bare.state.value, "129");
        assert_eq!(
            bare.state.completes.len(),
            1,
            "still exactly one completion"
        );
        // Emptying and refilling fires it a second time.
        bare.press_key(NamedKey::Backspace);
        bare.type_digit('7');
        assert_eq!(bare.state.completes.len(), 2);
    }

    #[test]
    fn a_press_cannot_jump_ahead_of_the_codes_own_progress() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        bare.type_digit('1');
        let far = slot_x(5) + OTP_SLOT_WIDTH / 2.0;
        bare.dispatch(&pointer(PointerPhase::Down, far, OTP_SLOT_HEIGHT / 2.0));
        assert_eq!(bare.widget.active, 1, "clamped to the first empty slot");

        for c in ['2', '3', '4', '5', '6'] {
            bare.type_digit(c);
        }
        bare.dispatch(&pointer(PointerPhase::Down, far, OTP_SLOT_HEIGHT / 2.0));
        assert_eq!(bare.widget.active, 5, "a full code clamps at the last slot");
    }

    #[test]
    fn a_press_in_a_gap_between_slots_still_lands_on_a_slot() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        for c in ['1', '2', '3'] {
            bare.type_digit(c);
        }
        // Dead centre of the gap between slot 0 and slot 1.
        let gap = OTP_SLOT_WIDTH + OTP_SLOT_GAP / 2.0;
        assert!(bare.widget.slot_at(gap).is_none(), "the gap is not a slot");
        bare.dispatch(&pointer(PointerPhase::Down, gap, OTP_SLOT_HEIGHT / 2.0));
        assert_eq!(bare.widget.active, 3, "clamped to the first empty slot");
    }

    #[test]
    fn a_non_primary_press_never_starts_an_interaction() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(10.0, 10.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(bare.dispatch(&secondary), EventResult::Ignored);
        assert!(!bare.widget.captured);
    }

    #[test]
    fn a_cancel_clears_the_capture_without_touching_the_code() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        bare.type_digit('1');
        bare.dispatch(&pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(bare.widget.captured);
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Cancel, 10.0, 10.0)),
            EventResult::Handled
        );
        assert!(!bare.widget.captured);
        assert_eq!(bare.widget.joined(), "1");
    }

    #[test]
    fn a_disabled_field_refuses_both_keys_and_presses() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        let disabled =
            Bare::view(String::new(), OTP_DEFAULT_LENGTH, OtpStatus::Idle).disabled(true);
        let mut ctx = BuildCtx::new(&mut bare.counter);
        View::<App>::rebuild(&disabled, &bare.view, &mut bare.widget, &mut ctx);
        bare.view = disabled;

        assert_eq!(bare.dispatch(&digit('1')), EventResult::Ignored);
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Down, 10.0, 10.0)),
            EventResult::Ignored
        );
        assert!(bare.state.changes.is_empty());
        assert!(!bare.widget.captured);
    }

    #[test]
    fn shrinking_the_length_drops_the_slots_past_the_new_end() {
        let mut bare = Bare::new(OTP_DEFAULT_LENGTH);
        for c in ['1', '2', '3', '4'] {
            bare.type_digit(c);
        }
        bare.rebuild_with(bare.state.value.clone(), 2, OtpStatus::Idle);
        assert_eq!(bare.widget.slots.len(), 2);
        assert_eq!(bare.widget.cells.len(), 2);
        assert!(bare.widget.active < 2);
        assert_eq!(bare.widget.joined(), "12");
    }

    // ---- The root-driven harness --------------------------------------------
    //
    // Focus (and therefore the caret and the active slot's border), the cursor
    // and semantics are only observable under a real `RenderRoot`.

    const WINDOW: Size = Size::new(560.0, 240.0);

    /// What the harness's rebuilt view carries.
    #[derive(Clone, Default)]
    struct Props {
        status: OtpStatus,
        label: Option<String>,
        hint: Option<String>,
        error_message: Option<String>,
        success_message: Option<String>,
        mask: bool,
        disabled: bool,
        length: Option<usize>,
    }

    struct Harness {
        root: RenderRoot<App, OtpInputView<App>>,
        state: App,
        tcx: TextContext,
        props: Props,
    }

    impl Harness {
        fn new(props: Props) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                tcx: TextContext::new(),
                props,
            };
            h.root.set_theme(Box::new(crate::theme()));
            h.pass();
            h
        }

        fn reduce_motion(&mut self) {
            let mut theme = crate::theme();
            theme.motion.reduce_motion = true;
            self.root.set_theme(Box::new(theme));
            self.pass();
        }

        fn pass(&mut self) {
            let props = self.props.clone();
            let mut logic = move |state: &mut App| {
                let mut view = otp_input::<App, _>(state.value.clone(), |s: &mut App, code| {
                    s.changes.push(code.clone());
                    s.value = code;
                })
                .status(props.status)
                .mask(props.mask)
                .disabled(props.disabled);
                if let Some(length) = props.length {
                    view = view.length(length);
                }
                if let Some(label) = &props.label {
                    view = view.label(label.clone());
                }
                if let Some(hint) = &props.hint {
                    view = view.hint(hint.clone());
                }
                if let Some(message) = &props.error_message {
                    view = view.error_message(message.clone());
                }
                if let Some(message) = &props.success_message {
                    view = view.success_message(message.clone());
                }
                view
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            let outcome = self.root.event(&mut self.state, event);
            self.pass();
            outcome
        }

        fn paint_at(&mut self, millis: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(millis));
            rec
        }

        fn paint(&mut self) -> Recorder {
            self.paint_at(0.0)
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }

        /// Press-and-release inside slot `index`, which also claims focus.
        fn press_slot(&mut self, index: usize) {
            let x = slot_x(index) + OTP_SLOT_WIDTH / 2.0;
            let y = OTP_SLOT_HEIGHT / 2.0;
            self.dispatch(&pointer(PointerPhase::Down, x, y));
            self.dispatch(&pointer(PointerPhase::Up, x, y));
        }

        fn type_digit(&mut self, c: char) {
            self.dispatch(&digit(c));
        }
    }

    // ---- Paint --------------------------------------------------------------

    #[test]
    fn every_slot_paints_one_hairline_outline() {
        let mut h = Harness::new(Props::default());
        let rec = h.paint();
        let borders = rec.borders();
        assert_eq!(borders.len(), OTP_DEFAULT_LENGTH);
        for (index, (rect, _)) in borders.iter().enumerate() {
            assert!(
                (rect.x0 - slot_x(index)).abs() < 1.0,
                "slot {index} sits at {rect:?}"
            );
        }
    }

    #[test]
    fn a_filled_slot_paints_its_digit_and_an_empty_one_paints_nothing() {
        let mut h = Harness::new(Props::default());
        h.press_slot(0);
        h.type_digit('7');
        // The first painted frame seeds the roll's clock; by 4s it has settled,
        // so the glyph composites at a true alpha of 1.
        h.paint_at(0.0);
        let rec = h.paint_at(4_000.0);
        assert_eq!(rec.glyphs.len(), 1, "one digit, five empty slots");
        assert!(rec.layers.iter().any(|a| (*a - 1.0).abs() < 1e-3));
    }

    #[test]
    fn a_digit_rolls_up_into_its_slot_rather_than_appearing() {
        let mut h = Harness::new(Props::default());
        h.press_slot(0);
        h.type_digit('7');
        // The first painted frame seeds the roll's clock; the next samples it.
        h.paint_at(0.0);
        let mid = h.paint_at(OTP_ROLL_MS as f64 / 2.0);
        let (rising, alpha) = (mid.glyphs[0].0, mid.layers[0]);
        assert!(alpha > 0.0 && alpha < 1.0, "mid-roll alpha: {alpha}");

        let settled = h.paint_at(4_000.0);
        assert!(
            rising.y > settled.glyphs[0].0.y,
            "the digit rises into place: {} -> {}",
            rising.y,
            settled.glyphs[0].0.y
        );
    }

    #[test]
    fn the_active_slot_carries_the_stronger_border_only_while_focused() {
        let mut h = Harness::new(Props::default());
        let unfocused = h.paint().borders()[0].1;
        h.press_slot(0);
        assert!(h.root.is_focus_active());
        let focused = h.paint().borders()[0].1;
        assert_ne!(
            unfocused, focused,
            "focus moves the active slot's border to the ink colour"
        );
        assert_eq!(focused, crate::theme().scheme().on_surface);
    }

    #[test]
    fn the_caret_blinks_in_the_active_slot_and_freezes_visible_under_reduce_motion() {
        let mut h = Harness::new(Props::default());
        h.press_slot(0);
        let lit = h.paint_at(0.0);
        let caret = lit.caret().expect("the caret is lit at the epoch");
        assert_eq!(caret.1, Size::new(OTP_CARET_WIDTH, OTP_CARET_HEIGHT));
        // An empty slot centres it.
        assert!((caret.0.x - (OTP_SLOT_WIDTH - OTP_CARET_WIDTH) / 2.0).abs() < 1e-6);

        let dark = h.paint_at(OTP_BLINK_MS as f64 * 0.75);
        assert!(dark.caret().is_none(), "the caret blinks off mid-cycle");

        h.reduce_motion();
        assert!(
            h.paint_at(OTP_BLINK_MS as f64 * 0.75).caret().is_some(),
            "reduced motion freezes the caret visible, not hidden"
        );
    }

    #[test]
    fn a_filled_active_slot_trails_the_caret_behind_its_digit() {
        let mut h = Harness::new(Props {
            length: Some(1),
            ..Props::default()
        });
        h.press_slot(0);
        h.type_digit('5');
        let caret = h.paint_at(0.0).caret().expect("the caret is lit");
        assert!(
            (caret.0.x - (OTP_SLOT_WIDTH - OTP_CARET_INSET - OTP_CARET_WIDTH)).abs() < 1e-6,
            "the caret trails the digit, at {caret:?}"
        );
    }

    #[test]
    fn the_cursor_is_the_text_beam_and_not_allowed_when_disabled() {
        let mut h = Harness::new(Props::default());
        h.dispatch(&pointer(PointerPhase::Move, 10.0, 10.0));
        assert_eq!(h.root.cursor(), CursorIcon::Text);

        let mut disabled = Harness::new(Props {
            disabled: true,
            ..Props::default()
        });
        disabled.dispatch(&pointer(PointerPhase::Move, 10.0, 10.0));
        assert_eq!(disabled.root.cursor(), style::DISABLED_CURSOR);
    }

    // ---- Status: shake, colour, check, message ------------------------------

    #[test]
    fn an_error_status_shakes_the_slot_row_and_lands_back_where_it_started() {
        let mut h = Harness::new(Props {
            label: Some("Code".to_string()),
            ..Props::default()
        });
        let resting = h.paint().borders()[0].0.x0;
        h.props.status = OtpStatus::Error;
        h.pass();

        h.paint_at(0.0);
        let mut displaced = false;
        for step in 1..=8 {
            let at = OTP_SHAKE_MS * f64::from(step) / 9.0;
            if (h.paint_at(at).borders()[0].0.x0 - resting).abs() > 0.5 {
                displaced = true;
            }
        }
        assert!(displaced, "the row is displaced somewhere in the shake");
        assert!(
            (h.paint_at(OTP_SHAKE_MS + 50.0).borders()[0].0.x0 - resting).abs() < 1e-6,
            "and lands back where it started"
        );
    }

    #[test]
    fn a_field_that_mounts_errored_does_not_shake() {
        let mut h = Harness::new(Props {
            status: OtpStatus::Error,
            ..Props::default()
        });
        let resting = h.paint_at(0.0).borders()[0].0.x0;
        for step in 1..=8 {
            let at = OTP_SHAKE_MS * f64::from(step) / 9.0;
            assert!(
                (h.paint_at(at).borders()[0].0.x0 - resting).abs() < 1e-6,
                "a mounted error is not announced with a shake"
            );
        }
    }

    #[test]
    fn the_error_border_crossfades_to_the_destructive_hue() {
        let mut h = Harness::new(Props::default());
        let idle = h.paint().borders()[0].1;
        h.props.status = OtpStatus::Error;
        h.pass();
        h.paint_at(0.0);
        let settled = h.paint_at(4_000.0).borders()[0].1;
        assert_ne!(idle, settled);
        assert_eq!(
            settled,
            style::with_alpha(crate::theme().scheme().error, OTP_STATUS_BORDER_ALPHA)
        );
    }

    #[test]
    fn a_success_status_draws_the_check_on_beside_the_row() {
        let mut h = Harness::new(Props::default());
        assert!(h.paint().check().is_none());
        h.props.status = OtpStatus::Success;
        h.pass();

        h.paint_at(0.0);
        assert!(
            h.paint_at(OTP_CHECK_DRAW_DELAY_MS / 2.0).check().is_none(),
            "the check waits out its delay"
        );
        let mid = h
            .paint_at(OTP_CHECK_DRAW_DELAY_MS + OTP_CHECK_DRAW_MS / 2.0)
            .check()
            .expect("mid-draw");
        let full = h.paint_at(4_000.0).check().expect("fully drawn");
        assert!(
            mid.0.width() < full.0.width(),
            "the path grows as it draws: {} -> {}",
            mid.0.width(),
            full.0.width()
        );
        let row_width = OTP_DEFAULT_LENGTH as f64 * OTP_SLOT_WIDTH
            + (OTP_DEFAULT_LENGTH - 1) as f64 * OTP_SLOT_GAP;
        assert!(full.0.x0 > row_width, "it sits clear of the row");
        assert_eq!(full.2, BeuiTokens::resolve(Some(&crate::theme())).success);
    }

    #[test]
    fn a_success_status_replaces_the_caret_with_its_check() {
        let mut h = Harness::new(Props::default());
        h.press_slot(0);
        assert!(h.paint_at(0.0).caret().is_some());
        h.props.status = OtpStatus::Success;
        h.pass();
        assert!(h.paint_at(0.0).caret().is_none());
    }

    #[test]
    fn the_message_row_follows_the_status() {
        let mut h = Harness::new(Props {
            hint: Some("Six digits".to_string()),
            error_message: Some("That code is wrong".to_string()),
            success_message: Some("Verified".to_string()),
            ..Props::default()
        });
        // Idle, error and success each put their own message under the row, so
        // the column keeps a message-row height throughout.
        let idle = h.paint().glyphs.len();
        h.props.status = OtpStatus::Error;
        h.pass();
        let error = h.paint().glyphs.len();
        h.props.status = OtpStatus::Success;
        h.pass();
        let success = h.paint().glyphs.len();
        assert!(idle > 0 && error > 0 && success > 0);
    }

    #[test]
    fn reduced_motion_lands_every_lane_at_once_and_never_shakes() {
        let mut h = Harness::new(Props::default());
        h.reduce_motion();
        h.press_slot(0);
        h.type_digit('1');
        h.props.status = OtpStatus::Error;
        h.pass();
        let rec = h.paint_at(0.0);
        // The digit is already at full alpha on its first painted frame...
        assert!(rec.layers.iter().any(|a| (*a - 1.0).abs() < 1e-3));
        // ...the border is already the destructive hue...
        assert_eq!(
            rec.borders()[0].1,
            style::with_alpha(crate::theme().scheme().error, OTP_STATUS_BORDER_ALPHA)
        );
        // ...and the row is not displaced (the stroke's own half-width inset is
        // the whole of the first slot's offset).
        assert!((rec.borders()[0].0.x0 - style::BORDER_WIDTH / 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_disabled_field_dims_every_colour_it_paints() {
        let mut h = Harness::new(Props::default());
        h.press_slot(0);
        h.type_digit('1');
        let enabled = h.paint_at(4_000.0).borders()[0].1;

        h.props.disabled = true;
        h.pass();
        let disabled = h.paint_at(4_000.0).borders()[0].1;
        assert!(
            disabled.components[3] < enabled.components[3],
            "the hairline dims: {enabled:?} -> {disabled:?}"
        );
    }

    // ---- Semantics ----------------------------------------------------------

    #[test]
    fn semantics_reports_one_text_input_carrying_the_whole_code() {
        let mut h = Harness::new(Props::default());
        h.press_slot(0);
        for c in ['1', '2'] {
            h.type_digit(c);
        }
        let update = h.semantics();
        let inputs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::TextInput)
            .collect();
        assert_eq!(inputs.len(), 1, "one control, not one per slot");
        assert_eq!(inputs[0].1.label(), Some("One-time passcode"));
        assert_eq!(inputs[0].1.value(), Some("12"));

        let disabled = Harness::new(Props {
            disabled: true,
            ..Props::default()
        });
        let update = disabled.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a Role::TextInput node");
        assert!(node.is_disabled());
    }

    #[test]
    fn a_masked_code_paints_and_announces_bullets_only() {
        let mut h = Harness::new(Props {
            mask: true,
            ..Props::default()
        });
        h.press_slot(0);
        h.type_digit('4');
        assert_eq!(h.state.value, "4", "the real digit is what the app holds");

        let update = h.semantics();
        assert!(
            !update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::TextInput),
            "masked never publishes the plain-text role"
        );
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::PasswordInput)
            .expect("a Role::PasswordInput node");
        assert_eq!(node.value(), Some(OTP_MASK_CHAR.to_string().as_str()));
    }
}
