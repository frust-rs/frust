//! Ports shadcn/ui's **InputOTP** (the segmented one-time-passcode input)
//! from `tmp/ui/apps/v4/registry/new-york-v4/ui/input-otp.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! Upstream is a thin wrapper over the `input-otp` package: the registry file
//! contributes the four slots' class lists (`InputOTP` container,
//! `InputOTPGroup`, `InputOTPSlot`, `InputOTPSeparator`) while the package
//! contributes the behavior — one real `<input>` behind N painted boxes, an
//! active slot, and a fake caret. There is no package to wrap here, so the
//! behavior is re-implemented and the class lists translate like this:
//!
//! | class | here |
//! |---|---|
//! | slot `h-9 w-9` | [`INPUT_OTP_SLOT_SIZE`] |
//! | slot `text-sm` | [`style::TEXT_SM`], centred in the box |
//! | slot `border-y border-r`, `first:border-l` | one outline per group, [`style::BORDER_WIDTH`] over `outline_variant` |
//! | slot `first:rounded-l-md last:rounded-r-md` | [`ShadcnRadius::md`] on the group's outer corners only |
//! | slot `shadow-xs` | [`style::SHADOW_XS`] under each group |
//! | slot `dark:bg-input/30` | [`DARK_FILL_ALPHA`] over `outline_variant`, dark only |
//! | `data-[active=true]:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] on the active slot |
//! | caret `h-4 w-px bg-foreground animate-caret-blink duration-1000` | [`CARET_HEIGHT`]/[`CARET_WIDTH`] in `on_surface`, blinking at [`INPUT_OTP_BLINK_MS`] |
//! | container `flex items-center gap-2` | [`GROUP_GAP`] between groups and their separators |
//! | separator `MinusIcon` | lucide `M5 12h14` at [`style::ICON_SIZE`] |
//! | container `has-disabled:opacity-50` | [`style::disabled_tint`] over every painted color |
//!
//! # One control, not N
//!
//! Upstream is a single `<input>` with `maxLength`, so the whole group is **one**
//! tab stop and one focus target: a `Down` anywhere in it claims focus, and every
//! key arrives focus-routed at this one widget. The boxes are paint, not widgets
//! — there are no child pods here at all.
//!
//! # Editing model
//!
//! The value is a plain string of at most `length` chars with no holes, exactly
//! like the `<input>` upstream really is; `caret` is this widget's own insertion
//! index into it (`0..=len`), the one piece of state it owns:
//!
//! - a permitted character inserts at `caret` and advances it,
//! - `Backspace` deletes before `caret` and retreats, `Delete` deletes at it,
//! - `ArrowLeft`/`ArrowRight`/`Home`/`End` move it,
//! - and every rebuild clamps it to the confirmed value's length, so a rejected
//!   or transformed keystroke pulls the caret back with it.
//!
//! The **active slot** is `min(caret, length - 1)`; it carries the ring, and the
//! fake caret paints in it while it is empty (`hasFakeCaret` upstream is exactly
//! "this slot is active and has no char").
//!
//! # Controlled, never self-mutating
//!
//! `value` is a prop: an edit reports the *requested* string through `on_change`
//! and, when that string fills every slot, through
//! [`InputOtpView::on_complete`] as well. The widget leaves `value` alone until
//! the next `rebuild` feeds the confirmed string back down
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! # Paste support
//!
//! A pasted string is walked character by character, with `self.mode.accepts`
//! filtering each one. Accepted chars fill the slots in order from the active
//! slot forward through the same insertion path a typed key uses, firing
//! `on_change` once with the final code and `on_complete` once if the code
//! becomes complete. An OTP field does not copy out: `Copy`, `Cut`, and
//! `SelectAll` are consumed (Handled) with no effect.
//!
//! # What the port deliberately does not carry
//!
//! - **No `pattern` regex.** The source's `REGEXP_ONLY_DIGITS` /
//!   `REGEXP_ONLY_DIGITS_AND_CHARS` presets are the only two anyone passes, so
//!   they are an [`InputOtpMode`] enum rather than a regex engine.
//! - **No `aria-invalid` ring.** As on `checkbox`, validation chrome belongs to
//!   `field`.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CursorIcon, EditCommand,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget,
    erase_callback_arg, text::TextStyle,
};
use frust::{FrameTime, Theme};

use crate::hit::{inside, presses};
use crate::style::{self, PATH_TOLERANCE};
use crate::text::{LabelRun, SHAPING_INK};
use crate::tokens::ShadcnTokens;

/// Slot edge, in logical px (`h-9 w-9`).
pub const INPUT_OTP_SLOT_SIZE: f64 = 36.0;

/// The caret blink period, in ms (`animate-caret-blink duration-1000`).
///
/// The source's keyframes fade rather than flip (`0%,70%,100%` opaque,
/// `20%,50%` clear); this port paints the same one-second cadence as a plain
/// on/off half-cycle, which is what the framework's own caret does
/// (`frust_widgets`' `TextInput`) and what a non-animatable single-color fill
/// can express.
pub const INPUT_OTP_BLINK_MS: u64 = 1_000;

/// Fake-caret height, in logical px (`h-4`).
const CARET_HEIGHT: f64 = 16.0;
/// Fake-caret width, in logical px (`w-px`).
const CARET_WIDTH: f64 = 1.0;

/// Gap between groups and their separator (`gap-2` on the container).
const GROUP_GAP: f64 = 8.0;

/// Alpha of the slot fill in dark mode (`dark:bg-input/30`); light mode paints
/// no fill, the same split `checkbox` documents.
const DARK_FILL_ALPHA: f32 = 0.30;

/// The glyph a masked slot's digit is replaced with — [`InputOtpView::masked`].
/// Same choice `frust_widgets::TextInput` masks with (Android's
/// `inputType=textPassword` default, the one platform that publishes a
/// specific character); this port has no glyph the *upstream* library names,
/// since upstream carries no masking option at all.
const OTP_MASK_CHAR: char = '\u{2022}';

/// Side of the lucide viewBox the separator's coordinates are authored in.
const ICON_VIEWBOX: f64 = 24.0;
/// Lucide's uniform stroke width, in viewBox units.
const ICON_STROKE_VIEWBOX: f64 = 2.0;

/// Which characters the group accepts, standing in for the source's two
/// `pattern` presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputOtpMode {
    /// `REGEXP_ONLY_DIGITS` — ASCII `0-9`, the default every shadcn example
    /// uses.
    #[default]
    Digits,
    /// `REGEXP_ONLY_DIGITS_AND_CHARS` — ASCII digits and letters, case
    /// preserved (the package transforms nothing).
    Alphanumeric,
}

impl InputOtpMode {
    /// Whether `c` may be typed into a slot.
    fn accepts(self, c: char) -> bool {
        match self {
            InputOtpMode::Digits => c.is_ascii_digit(),
            InputOtpMode::Alphanumeric => c.is_ascii_alphanumeric(),
        }
    }
}

/// A view-held, typed change callback (erased on build).
type OnChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative shadcn OTP input. See the [module docs](self).
pub struct InputOtpView<State: 'static> {
    value: String,
    length: usize,
    groups: Vec<usize>,
    mode: InputOtpMode,
    disabled: bool,
    label: Option<String>,
    masked: bool,
    on_change: OnChange<State>,
    on_complete: Option<OnChange<State>>,
}

/// Create a `length`-slot OTP group showing `value` and reporting every
/// requested string through `on_change` — a **controlled** component (see the
/// [module docs](self)).
///
/// The slots form one group by default; [`InputOtpView::groups`] splits them
/// the way the source's `InputOTPGroup`/`InputOTPSeparator` anatomy does.
pub fn input_otp<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    length: usize,
    on_change: F,
) -> InputOtpView<State> {
    let length = length.max(1);
    InputOtpView {
        value: value.into(),
        length,
        groups: vec![length],
        mode: InputOtpMode::default(),
        disabled: false,
        label: None,
        masked: false,
        on_change: Rc::new(on_change),
        on_complete: None,
    }
}

impl<State: 'static> InputOtpView<State> {
    /// Split the slots into groups, separated by a minus glyph — the source's
    /// `<InputOTPGroup>…<InputOTPSeparator/>…<InputOTPGroup>` anatomy, e.g.
    /// `.groups(vec![3, 3])` for the canonical six-digit code.
    ///
    /// Counts summing to something other than the slot count are corrected to
    /// one group of every slot rather than dropping or inventing boxes.
    pub fn groups(mut self, groups: Vec<usize>) -> Self {
        let sum: usize = groups.iter().sum();
        self.groups = if sum == self.length && !groups.contains(&0) {
            groups
        } else {
            vec![self.length]
        };
        self
    }

    /// Set which characters the group accepts (see [`InputOtpMode`]).
    pub fn mode(mut self, mode: InputOtpMode) -> Self {
        self.mode = mode;
        self
    }

    /// Report the completed code once an edit fills every slot
    /// (`onComplete`). Fired *after* `on_change`, with the same string.
    pub fn on_complete<F: Fn(&mut State, String) + 'static>(mut self, on_complete: F) -> Self {
        self.on_complete = Some(Rc::new(on_complete));
        self
    }

    /// Disable the control: 50% opacity, inert to pointer and key, and a
    /// not-allowed cursor (`has-disabled:opacity-50 disabled:cursor-not-allowed`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Name the control for assistive tech — the group paints only the code
    /// itself, so this is the only source for the semantics node's label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Mask the code — for a passcode delivered over a shoulder-surfable
    /// screen, not upstream's own `input-otp` shape (the ported registry file
    /// carries no masking prop; upstream renders every digit visibly by
    /// design, which is why this defaults to `false`). Mirrors
    /// `frust_widgets`' `TextInputView::obscured` convention: the semantics
    /// node becomes [`Role::PasswordInput`] and reports the bulleted mirror,
    /// never the real code, to an assistive-tech client; each painted slot
    /// shows the same bullet in place of its digit, so the visible and
    /// announced states never disagree.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }
}

/// The resolved OTP palette.
struct OtpColors {
    /// The slot outline (`border-input`).
    border: Color,
    /// The code's glyphs and the caret (the inherited `foreground`).
    ink: Color,
    /// The slot fill: `None` in light mode, `Some(input/30)` in dark.
    fill: Option<Color>,
}

/// Resolve the palette, falling back to the `neutral` preset's light values
/// with no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> OtpColors {
    let scheme = theme.map(Theme::scheme);
    match scheme {
        Some(scheme) => OtpColors {
            border: scheme.outline_variant,
            ink: scheme.on_surface,
            fill: style::is_dark(theme)
                .then(|| style::scale_alpha(scheme.outline_variant, DARK_FILL_ALPHA)),
        },
        None => {
            let light = crate::tokens::color_scheme_light();
            OtpColors {
                border: light.outline_variant,
                ink: light.on_surface,
                fill: None,
            }
        }
    }
}

/// The style each slot's single glyph is shaped in (`text-sm`, the theme's own
/// sans stack), shaped with [`SHAPING_INK`] and re-brushed at paint.
fn slot_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.body_medium.family.clone()
    });
    TextStyle {
        family,
        ..TextStyle::new(style::TEXT_SM as f32, SHAPING_INK)
    }
}

/// Lucide's `MinusIcon` (`M5 12h14`), scaled to a `size`-square box at
/// `origin` — the `InputOTPSeparator` glyph.
fn minus_path(origin: Point, size: f64) -> BezPath {
    let s = size / ICON_VIEWBOX;
    let at = |x: f64, y: f64| Point::new(origin.x + x * s, origin.y + y * s);
    let mut path = BezPath::new();
    path.move_to(at(5.0, 12.0));
    path.line_to(at(19.0, 12.0));
    path
}

impl<State: 'static> View<State> for InputOtpView<State> {
    type Element = InputOtpWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> InputOtpWidget {
        let mut widget = InputOtpWidget {
            value: String::new(),
            length: self.length,
            groups: self.groups.clone(),
            mode: self.mode,
            disabled: self.disabled,
            label: self.label.clone(),
            masked: self.masked,
            caret: 0,
            slots: Vec::new(),
            blink_epoch: FrameTime::ZERO,
            blink_reset_pending: true,
            captured: false,
            on_change: erase_callback_arg(&self.on_change),
            on_complete: self.on_complete.as_ref().map(erase_callback_arg),
        };
        widget.adopt_value(&self.value);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut InputOtpWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so both adapters are reinstalled every pass.
        element.on_change = erase_callback_arg(&self.on_change);
        element.on_complete = self.on_complete.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;
        if prev.length != self.length || prev.groups != self.groups {
            element.length = self.length;
            element.groups = self.groups.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.masked != self.masked {
            element.masked = self.masked;
            // The slot runs are keyed from `masked` too (bullets vs. digits),
            // so a bare toggle needs the same re-key `value` changing gets.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.value != self.value || flags.needs_layout() {
            // The app is the source of truth: adopt the confirmed code, and pull
            // the caret back to it if the app rejected or shortened an edit.
            element.adopt_value(&self.value);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.mode != self.mode {
            element.mode = self.mode;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for an [`InputOtpView`].
pub struct InputOtpWidget {
    /// The app-confirmed code (source of truth; adopted on `rebuild`), never
    /// longer than `length` chars.
    value: String,
    length: usize,
    /// Slot counts per group; sums to `length`.
    groups: Vec<usize>,
    mode: InputOtpMode,
    disabled: bool,
    label: Option<String>,
    /// See [`InputOtpView::masked`].
    masked: bool,
    /// The insertion index into `value` (`0..=value.len()`) — the only state
    /// this widget owns.
    caret: usize,
    /// One cached run per slot, re-brushed at paint (the ink follows the theme
    /// without a reshape).
    slots: Vec<LabelRun>,
    /// The frame time the current blink cycle is measured from.
    blink_epoch: FrameTime,
    /// Set by any edit or caret move; the next paint records the epoch from its
    /// own frame clock, since the event pass has none.
    blink_reset_pending: bool,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`.
    captured: bool,
    on_change: frust::authoring::ErasedArgCallback<String>,
    on_complete: Option<frust::authoring::ErasedArgCallback<String>>,
}

impl InputOtpWidget {
    /// Adopt an app-confirmed code: truncate it to `length`, re-key the slot
    /// runs, and clamp the caret into it.
    ///
    /// A masked group's runs carry [`OTP_MASK_CHAR`] rather than the real
    /// digit — `self.value` (and everything the caret/editing math reads) is
    /// always the real code, so masking touches only what gets painted.
    fn adopt_value(&mut self, value: &str) {
        self.value = value.chars().take(self.length).collect();
        let chars: Vec<char> = self.value.chars().collect();
        self.slots.clear();
        for index in 0..self.length {
            let glyph = chars.get(index).map(|c| {
                if self.masked {
                    OTP_MASK_CHAR.to_string()
                } else {
                    c.to_string()
                }
            });
            self.slots.push(LabelRun::new(glyph.unwrap_or_default()));
        }
        self.caret = self.caret.min(chars.len());
    }

    /// The slot the ring and the fake caret belong to: the caret's own slot,
    /// pinned to the last one once the code is full (upstream's `isActive`).
    fn active_slot(&self) -> usize {
        self.caret.min(self.length - 1)
    }

    /// Whether the fake caret shows at all: the active slot is empty
    /// (upstream's `hasFakeCaret`).
    fn has_fake_caret(&self) -> bool {
        self.value.chars().nth(self.active_slot()).is_none()
    }

    /// The local x of slot `index`'s left edge, walking the groups and their
    /// separators.
    fn slot_x(&self, index: usize) -> f64 {
        let mut x = 0.0;
        let mut seen = 0usize;
        for (group, count) in self.groups.iter().copied().enumerate() {
            if index < seen + count {
                return x + (index - seen) as f64 * INPUT_OTP_SLOT_SIZE;
            }
            seen += count;
            x += count as f64 * INPUT_OTP_SLOT_SIZE;
            if group + 1 < self.groups.len() {
                x += GROUP_GAP + style::ICON_SIZE + GROUP_GAP;
            }
        }
        x
    }

    /// The total width of the group row.
    fn row_width(&self) -> f64 {
        let slots = self.length as f64 * INPUT_OTP_SLOT_SIZE;
        let separators = self.groups.len().saturating_sub(1) as f64;
        slots + separators * (GROUP_GAP * 2.0 + style::ICON_SIZE)
    }

    /// The slot a local x lands on, or `None` off the boxes entirely (in a
    /// gap, or past either end).
    fn slot_at(&self, x: f64) -> Option<usize> {
        (0..self.length).find(|index| {
            let left = self.slot_x(*index);
            x >= left && x < left + INPUT_OTP_SLOT_SIZE
        })
    }

    /// Whether the caret is in its visible half-cycle at frame time `now`.
    fn caret_visible_at(&self, now: FrameTime) -> bool {
        let half = INPUT_OTP_BLINK_MS as f64 / 2.0;
        let elapsed = now.saturating_sub(self.blink_epoch).as_secs_f64() * 1000.0;
        ((elapsed / half) as u64).is_multiple_of(2)
    }

    /// Restart the blink so the caret is lit from the next painted frame. The
    /// epoch itself is recorded in `paint`, the only pass with a clock.
    fn reset_blink(&mut self) {
        self.blink_reset_pending = true;
    }

    /// Report `next` through `on_change`, and through `on_complete` too once it
    /// fills every slot. Never touches `value`.
    fn report(&mut self, ctx: &mut EventCtx, next: String) {
        let complete = next.chars().count() == self.length;
        (self.on_change)(ctx, next.clone());
        if complete && let Some(on_complete) = self.on_complete.as_mut() {
            on_complete(ctx, next);
        }
    }

    /// Insert `c` at the caret and report the result. A full code takes nothing.
    fn insert(&mut self, ctx: &mut EventCtx, c: char) -> bool {
        let mut chars: Vec<char> = self.value.chars().collect();
        if chars.len() >= self.length {
            return false;
        }
        let at = self.caret.min(chars.len());
        chars.insert(at, c);
        self.caret = at + 1;
        self.report(ctx, chars.into_iter().collect());
        true
    }

    /// Delete the char at `at` and report the result.
    fn delete_at(&mut self, ctx: &mut EventCtx, at: usize) -> bool {
        let mut chars: Vec<char> = self.value.chars().collect();
        if at >= chars.len() {
            return false;
        }
        chars.remove(at);
        self.caret = at;
        self.report(ctx, chars.into_iter().collect());
        true
    }

    /// Move the caret to `at`, clamped into the filled prefix.
    fn move_caret(&mut self, at: usize) {
        self.caret = at.min(self.value.chars().count());
    }

    /// Paste `text` by filtering accepted characters and filling slots from the
    /// active slot forward, the same way individual keystrokes would. Reports
    /// once with the final value.
    fn paste(&mut self, ctx: &mut EventCtx, text: &str) -> bool {
        let mut chars: Vec<char> = self.value.chars().collect();
        let mut at = self.caret.min(chars.len());
        let mut changed = false;

        for c in text.chars() {
            if !self.mode.accepts(c) {
                continue;
            }
            if chars.len() >= self.length {
                break;
            }
            chars.insert(at, c);
            at += 1;
            changed = true;
        }

        if changed {
            self.caret = at;
            self.report(ctx, chars.into_iter().collect());
            true
        } else {
            false
        }
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

impl Widget for InputOtpWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = slot_style(Theme::from_layout_ctx(ctx));
        for slot in &mut self.slots {
            // An empty slot never reaches the shaper: it has nothing to measure
            // and nothing to paint.
            if !slot.content().is_empty() {
                slot.layout(ctx, &style);
            }
        }
        bc.constrain(Size::new(self.row_width(), INPUT_OTP_SLOT_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        if self.blink_reset_pending {
            self.blink_epoch = now;
            self.blink_reset_pending = false;
        }
        let focused = ctx.has_focus();
        let origin = ctx.origin();
        // One scope for every theme read: the `&Theme` borrows the context and
        // the frame request below needs it mutably.
        let (colors, radius, ring, reduce_motion) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                ShadcnTokens::resolve_radius(None, theme).md,
                style::ring_color(None, theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };
        let tint = |color: Color| style::disabled_tint(color, self.disabled);
        let slot_size = Size::new(INPUT_OTP_SLOT_SIZE, INPUT_OTP_SLOT_SIZE);
        let active = self.active_slot();

        let mut seen = 0usize;
        for (group, count) in self.groups.iter().copied().enumerate() {
            let group_origin = Point::new(origin.x + self.slot_x(seen), origin.y);
            let group_size = Size::new(count as f64 * INPUT_OTP_SLOT_SIZE, INPUT_OTP_SLOT_SIZE);

            style::draw_shadow(
                scene,
                group_origin,
                group_size,
                radius,
                style::SHADOW_XS,
                Theme::from_paint_ctx(ctx),
            );
            if let Some(fill) = colors.fill {
                scene.fill_rounded_rect(group_origin, group_size, radius, tint(fill));
            }

            // `border-y border-r first:border-l`: one outline around the group,
            // rounded on its outer corners, plus a hairline per shared seam.
            let inset = style::BORDER_WIDTH / 2.0;
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, group_size).inset(-inset),
                (radius - inset).max(0.0),
            );
            scene.stroke_path(
                group_origin,
                &Shape::to_path(&outline, PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(tint(colors.border)),
            );
            for seam in 1..count {
                let x = seam as f64 * INPUT_OTP_SLOT_SIZE;
                let mut path = BezPath::new();
                path.move_to(Point::new(x, 0.0));
                path.line_to(Point::new(x, INPUT_OTP_SLOT_SIZE));
                scene.stroke_path(
                    group_origin,
                    &path,
                    style::BORDER_WIDTH,
                    &Brush::Solid(tint(colors.border)),
                );
            }

            for offset in 0..count {
                let index = seen + offset;
                let slot_origin = Point::new(origin.x + self.slot_x(index), origin.y);

                // The active slot's two-part focus treatment: the border swaps to
                // the ring color and the 3px ring sits outside its own box
                // (`data-[active=true]`, which upstream keys off focus).
                if focused && !self.disabled && index == active {
                    let box_outline = RoundedRect::from_rect(
                        Rect::from_origin_size(Point::ORIGIN, slot_size).inset(-inset),
                        (radius - inset).max(0.0),
                    );
                    scene.stroke_path(
                        slot_origin,
                        &Shape::to_path(&box_outline, PATH_TOLERANCE),
                        style::BORDER_WIDTH,
                        &Brush::Solid(ring),
                    );
                    style::draw_focus_ring(scene, slot_origin, slot_size, radius, ring);
                }

                let run = &self.slots[index];
                if !run.content().is_empty() {
                    let text = run.size();
                    let at = Point::new(
                        slot_origin.x + (INPUT_OTP_SLOT_SIZE - text.width) / 2.0,
                        slot_origin.y + (INPUT_OTP_SLOT_SIZE - text.height) / 2.0,
                    );
                    run.paint(at, tint(colors.ink), scene);
                }
            }
            seen += count;

            // The separator between this group and the next.
            if group + 1 < self.groups.len() {
                let x = origin.x + self.slot_x(seen - 1) + INPUT_OTP_SLOT_SIZE + GROUP_GAP;
                let y = origin.y + (INPUT_OTP_SLOT_SIZE - style::ICON_SIZE) / 2.0;
                let path = minus_path(Point::ORIGIN, style::ICON_SIZE);
                scene.stroke_path(
                    Point::new(x, y),
                    &path,
                    ICON_STROKE_VIEWBOX * style::ICON_SIZE / ICON_VIEWBOX,
                    &Brush::Solid(tint(colors.ink)),
                );
            }
        }

        // The fake caret, in the active slot while it is empty. `reduce_motion`
        // freezes it **visible** and stops requesting frames — a caret is a
        // position cue, the same exception the framework's own text field makes.
        if focused && !self.disabled && self.has_fake_caret() {
            if !reduce_motion {
                ctx.request_frame_paced_at(Duration::from_millis(INPUT_OTP_BLINK_MS / 2));
            }
            if reduce_motion || self.caret_visible_at(now) {
                let slot_origin = Point::new(origin.x + self.slot_x(active), origin.y);
                scene.fill_rect(
                    Point::new(
                        slot_origin.x + (INPUT_OTP_SLOT_SIZE - CARET_WIDTH) / 2.0,
                        slot_origin.y + (INPUT_OTP_SLOT_SIZE - CARET_HEIGHT) / 2.0,
                    ),
                    Size::new(CARET_WIDTH, CARET_HEIGHT),
                    tint(colors.ink),
                );
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled {
                    return EventResult::Ignored;
                }
                let handled = match &key.key {
                    Key::Character(text) => {
                        let Some(c) = text.chars().next().filter(|c| self.mode.accepts(*c)) else {
                            return EventResult::Ignored;
                        };
                        self.insert(ctx, c);
                        true
                    }
                    Key::Named(NamedKey::Backspace) => {
                        self.caret > 0 && self.delete_at(ctx, self.caret - 1)
                    }
                    Key::Named(NamedKey::Delete) => self.delete_at(ctx, self.caret),
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.move_caret(self.caret.saturating_sub(1));
                        true
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        self.move_caret(self.caret + 1);
                        true
                    }
                    Key::Named(NamedKey::Home) => {
                        self.move_caret(0);
                        true
                    }
                    Key::Named(NamedKey::End) => {
                        self.move_caret(self.length);
                        true
                    }
                    _ => return EventResult::Ignored,
                };
                // A key that changed nothing (a `Backspace` at the start, an
                // `ArrowRight` at the end) still belongs to this control.
                if handled {
                    self.reset_blink();
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if self.disabled || !presses(p) || !inside(p.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        ctx.capture_pointer();
                        // A press positions the caret on the slot it landed on
                        // (clamped into the filled prefix), the way clicking a
                        // text field would.
                        if let Some(slot) = self.slot_at(p.position.x) {
                            self.move_caret(slot);
                        }
                        self.reset_blink();
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            // The hover/cursor pass: claim on every qualifying
                            // move (the claim is per-pass, never sticky) and
                            // re-ask for the cursor, which is stateless too.
                            if inside(p.position, size) {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            return EventResult::Ignored;
                        }
                        // Captured: re-ask so the shape survives a drag that
                        // wandered outside the row.
                        ctx.set_cursor(self.cursor());
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // A Cancel arm clears internal flags only — never the
                        // value, never a callback.
                        self.captured = false;
                        EventResult::Handled
                    }
                }
            }
            InputEvent::EditCommand(cmd) => {
                if self.disabled {
                    return EventResult::Ignored;
                }
                match cmd {
                    EditCommand::Paste(text) => {
                        if self.paste(ctx, text) {
                            self.reset_blink();
                            ctx.request_redraw();
                        }
                        EventResult::Handled
                    }
                    EditCommand::Copy | EditCommand::Cut | EditCommand::SelectAll => {
                        // An OTP field does not copy out; these commands are
                        // consumed with no effect.
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One `<input>` upstream is one node here: the group is a single text
        // field carrying the whole code, not N boxes.
        //
        // Masked mirrors `frust_widgets::TextInput::obscured`'s node shape:
        // `Role::PasswordInput` plus the bulleted mirror as the reported
        // value, never the real code — an assistive-tech client reads the
        // node value verbatim, so publishing the real code there would defeat
        // the masking the painted slots already carry.
        let role = if self.masked {
            Role::PasswordInput
        } else {
            Role::TextInput
        };
        let value = if self.masked {
            self.value.chars().map(|_| OTP_MASK_CHAR).collect()
        } else {
            self.value.clone()
        };
        ctx.push_node(role, |node| {
            if let Some(label) = &self.label {
                node.set_label(label.as_str());
            }
            node.set_value(value);
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Focus);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        EventOutcome, KeyEvent, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::{Brightness, FrameTime};
    use std::any::Any;

    const LENGTH: usize = 6;

    /// Records the fills, stroked paths, glyph inks and shadows this widget
    /// emits.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: usize,
        glyphs: Vec<(Point, Color)>,
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
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _sd: f64, _c: Color) {
            self.shadows += 1;
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            let color = match run.brush {
                Brush::Solid(c) => c,
                _ => Color::TRANSPARENT,
            };
            let at = run.transform.translation();
            self.glyphs.push((Point::new(at.x, at.y), color));
        }
    }

    impl Recorder {
        /// The caret fill, if the pass painted one (the only `fill_rect` this
        /// widget emits).
        fn caret(&self) -> Option<(Point, Size, Color)> {
            self.rects.first().copied()
        }
    }

    #[derive(Default)]
    struct Codes {
        value: String,
        changes: Vec<String>,
        completed: Vec<String>,
    }

    fn view(value: &str) -> InputOtpView<Codes> {
        input_otp::<Codes, _>(value, LENGTH, |s: &mut Codes, v: String| {
            s.changes.push(v.clone());
            s.value = v;
        })
        .on_complete(|s: &mut Codes, v: String| s.completed.push(v))
        .label("one-time code")
    }

    fn build(v: &InputOtpView<Codes>) -> InputOtpWidget {
        let mut counter = 0u64;
        View::<Codes>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn laid_out(v: &InputOtpView<Codes>) -> (InputOtpWidget, Size) {
        let mut w = build(v);
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut ctx, &BoxConstraints::loose(Size::new(600.0, 600.0)));
        (w, size)
    }

    /// Paint an **unfocused** widget directly — everything that does not depend
    /// on the focus flag, which only a real root can seed.
    fn paint_at(w: &mut InputOtpWidget, size: Size, theme: Option<&Theme>, ms: f64) -> Recorder {
        let mut rec = Recorder::default();
        let ctx = PaintCtx::for_test(Point::ZERO, size, FrameTime::from_nanos((ms * 1e6) as u64));
        let mut ctx = match theme {
            Some(t) => ctx.with_theme(t),
            None => ctx,
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn key(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn typed(c: char) -> InputEvent {
        key(Key::Character(c.to_string()))
    }

    fn pointer(phase: PointerPhase, x: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, INPUT_OTP_SLOT_SIZE / 2.0),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(
        w: &mut InputOtpWidget,
        state: &mut Codes,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    /// Feed the app-confirmed value back down, the way a real frame would.
    fn confirm(
        w: &mut InputOtpWidget,
        prev: &InputOtpView<Codes>,
        value: &str,
    ) -> InputOtpView<Codes> {
        let next = view(value);
        let mut counter = 0u64;
        View::<Codes>::rebuild(&next, prev, w, &mut BuildCtx::new(&mut counter));
        next
    }

    // ---- Geometry ----------------------------------------------------------

    #[test]
    fn one_group_is_a_flush_row_of_slots() {
        let (w, size) = laid_out(&view(""));
        assert_eq!(
            size,
            Size::new(LENGTH as f64 * INPUT_OTP_SLOT_SIZE, INPUT_OTP_SLOT_SIZE)
        );
        for index in 0..LENGTH {
            assert_eq!(w.slot_x(index), index as f64 * INPUT_OTP_SLOT_SIZE);
        }
    }

    #[test]
    fn grouped_slots_reserve_a_gapped_separator_between_them() {
        let (w, size) = laid_out(&view("").groups(vec![3, 3]));
        let separator = GROUP_GAP * 2.0 + style::ICON_SIZE;
        assert_eq!(
            size.width,
            LENGTH as f64 * INPUT_OTP_SLOT_SIZE + separator,
            "one separator's worth of gap"
        );
        assert_eq!(w.slot_x(2), 2.0 * INPUT_OTP_SLOT_SIZE);
        assert_eq!(w.slot_x(3), 3.0 * INPUT_OTP_SLOT_SIZE + separator);

        // A group list that does not add up is corrected rather than obeyed.
        let (bad, _) = laid_out(&view("").groups(vec![2, 2]));
        assert_eq!(bad.groups, vec![LENGTH]);
        let (zero, _) = laid_out(&view("").groups(vec![0, 6]));
        assert_eq!(zero.groups, vec![LENGTH]);
    }

    #[test]
    fn a_value_longer_than_the_slot_count_is_truncated() {
        let (w, _) = laid_out(&view("1234567890"));
        assert_eq!(w.value, "123456");
        assert_eq!(w.slots.len(), LENGTH);
    }

    #[test]
    fn masked_paints_bullets_over_every_filled_slot_and_the_real_code_stays_the_value() {
        let (w, _) = laid_out(&view("12").masked(true));
        assert_eq!(w.value, "12", "the real code is untouched by masking");
        assert_eq!(w.slots[0].content(), "\u{2022}");
        assert_eq!(w.slots[1].content(), "\u{2022}");
        assert_eq!(
            w.slots[2].content(),
            "",
            "an empty slot stays empty, not a bullet"
        );

        // Unmasked (the default) still shows the real digit.
        let (w, _) = laid_out(&view("12"));
        assert_eq!(w.slots[0].content(), "1");
        assert_eq!(w.slots[1].content(), "2");
    }

    // ---- Paint -------------------------------------------------------------

    #[test]
    fn each_group_paints_one_outline_its_seams_and_a_shadow() {
        let theme = crate::theme();
        let (mut w, size) = laid_out(&view("12").groups(vec![3, 3]));
        let rec = paint_at(&mut w, size, Some(&theme), 0.0);

        assert_eq!(rec.shadows, 2, "`shadow-xs` per group");
        // Per group: one outline + two seams, with the separator painted
        // between the two groups' runs.
        assert_eq!(rec.strokes.len(), 2 * 3 + 1);
        let border = theme.scheme().outline_variant;
        let chrome = [0, 1, 2, 4, 5, 6];
        assert!(chrome.iter().all(|i| rec.strokes[*i].2 == border));
        // The separator is a flat run between the two groups.
        let (bbox, _, ink) = rec.strokes[3];
        assert!(bbox.height() < 1e-9, "`M5 12h14`");
        assert!(bbox.x0 > 3.0 * INPUT_OTP_SLOT_SIZE);
        assert_eq!(ink, theme.scheme().on_surface);

        assert_eq!(rec.glyphs.len(), 2, "one run per filled slot");
        assert!(
            rec.glyphs
                .iter()
                .all(|(_, c)| *c == theme.scheme().on_surface)
        );
        assert!(rec.rrects.is_empty(), "light mode authors no slot fill");
    }

    #[test]
    fn dark_mode_washes_every_group() {
        let theme = crate::theme().with_brightness(Brightness::Dark);
        let (mut w, size) = laid_out(&view("").groups(vec![3, 3]));
        let rec = paint_at(&mut w, size, Some(&theme), 0.0);
        assert_eq!(rec.rrects.len(), 2, "`dark:bg-input/30` per group");
        let input = theme.scheme().outline_variant;
        assert!(
            (rec.rrects[0].3.components[3] - input.components[3] * DARK_FILL_ALPHA).abs() < 1e-6
        );
    }

    #[test]
    fn a_disabled_group_dims_every_painted_color() {
        let theme = crate::theme();
        let (mut enabled, size) = laid_out(&view("12"));
        let (mut disabled, _) = laid_out(&view("12").disabled(true));
        let on = paint_at(&mut enabled, size, Some(&theme), 0.0);
        let off = paint_at(&mut disabled, size, Some(&theme), 0.0);
        assert_eq!(
            off.strokes[0].2.components[3],
            on.strokes[0].2.components[3] * style::DISABLED_OPACITY
        );
        assert_eq!(
            off.glyphs[0].1.components[3],
            on.glyphs[0].1.components[3] * style::DISABLED_OPACITY
        );
    }

    // ---- Editing -----------------------------------------------------------

    #[test]
    fn typing_fills_forward_and_reports_without_self_mutating() {
        let (mut w, size) = laid_out(&view(""));
        let mut state = Codes::default();
        let mut prev = view("");
        for (i, c) in "12345".chars().enumerate() {
            dispatch(&mut w, &mut state, size, &typed(c));
            assert_eq!(state.changes.last().unwrap().chars().count(), i + 1);
            let confirmed = state.value.clone();
            prev = confirm(&mut w, &prev, &confirmed);
        }
        assert_eq!(w.value, "12345");
        assert_eq!(w.caret, 5);
        assert!(state.completed.is_empty(), "five of six slots");

        dispatch(&mut w, &mut state, size, &typed('6'));
        assert_eq!(state.changes.last().unwrap(), "123456");
        assert_eq!(state.completed, vec!["123456".to_string()]);

        // A full code takes nothing more.
        let confirmed = state.value.clone();
        confirm(&mut w, &prev, &confirmed);
        let before = state.changes.len();
        dispatch(&mut w, &mut state, size, &typed('7'));
        assert_eq!(state.changes.len(), before);
    }

    #[test]
    fn only_permitted_characters_are_taken() {
        let (mut w, size) = laid_out(&view(""));
        let mut state = Codes::default();
        for c in ['a', ' ', '-'] {
            assert_eq!(
                dispatch(&mut w, &mut state, size, &typed(c)),
                EventResult::Ignored,
                "digits-only rejects {c:?}"
            );
        }
        assert!(state.changes.is_empty());

        let (mut alpha, size) = laid_out(&view("").mode(InputOtpMode::Alphanumeric));
        dispatch(&mut alpha, &mut state, size, &typed('a'));
        assert_eq!(state.changes.last().unwrap(), "a");
        assert_eq!(
            dispatch(&mut alpha, &mut state, size, &typed('-')),
            EventResult::Ignored
        );
    }

    #[test]
    fn backspace_clears_back_and_delete_clears_forward() {
        let (mut w, size) = laid_out(&view("123"));
        let mut state = Codes::default();
        w.caret = 3;
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::Backspace)),
        );
        assert_eq!(state.changes.last().unwrap(), "12");
        assert_eq!(w.caret, 2);
        assert_eq!(w.value, "123", "the app owns the value");

        let prev = view("123");
        confirm(&mut w, &prev, "12");
        assert_eq!(w.caret, 2);

        // At the start, backspace has nothing to take.
        w.caret = 0;
        let before = state.changes.len();
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &key(Key::Named(NamedKey::Backspace))
            ),
            EventResult::Handled,
            "the key still belongs to this control"
        );
        assert_eq!(state.changes.len(), before);

        dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::Delete)));
        assert_eq!(state.changes.last().unwrap(), "2");
    }

    #[test]
    fn arrows_move_the_active_slot_within_the_filled_prefix() {
        let (mut w, size) = laid_out(&view("123"));
        let mut state = Codes::default();
        w.caret = 3;
        assert_eq!(w.active_slot(), 3);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(w.caret, 2);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.caret, 3);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.caret, 3, "the caret never leaves the filled prefix");
        dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::Home)));
        assert_eq!(w.caret, 0);
        dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::End)));
        assert_eq!(w.caret, 3);
        assert!(state.changes.is_empty(), "caret motion reports nothing");

        // Typing mid-code inserts at the caret.
        w.caret = 1;
        dispatch(&mut w, &mut state, size, &typed('9'));
        assert_eq!(state.changes.last().unwrap(), "1923");
    }

    #[test]
    fn a_full_code_pins_the_active_slot_to_the_last_box() {
        let (w, _) = laid_out(&view("123456"));
        assert_eq!(w.caret, 0, "a freshly built widget starts at the front");
        let (mut w2, size) = laid_out(&view("123456"));
        let mut state = Codes::default();
        dispatch(&mut w2, &mut state, size, &key(Key::Named(NamedKey::End)));
        assert_eq!(w2.caret, LENGTH);
        assert_eq!(w2.active_slot(), LENGTH - 1);
        assert!(!w2.has_fake_caret());
    }

    #[test]
    fn a_press_positions_the_caret_and_a_disabled_group_is_inert() {
        let (mut w, size) = laid_out(&view("123"));
        let mut state = Codes::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, INPUT_OTP_SLOT_SIZE * 1.5),
        );
        assert!(w.captured);
        assert_eq!(w.caret, 1);
        dispatch(&mut w, &mut state, size, &pointer(PointerPhase::Up, 0.0));
        assert!(!w.captured);

        // Past the filled prefix, the caret stops at the end of it.
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, INPUT_OTP_SLOT_SIZE * 5.5),
        );
        assert_eq!(w.caret, 3);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, 0.0),
        );
        assert!(!w.captured, "Cancel disarms");

        let (mut disabled, size) = laid_out(&view("123").disabled(true));
        for event in [pointer(PointerPhase::Down, 10.0), typed('4')] {
            assert_eq!(
                dispatch(&mut disabled, &mut state, size, &event),
                EventResult::Ignored
            );
        }
        assert!(state.changes.is_empty());
    }

    #[test]
    fn rebuild_adopts_the_confirmed_code_and_pulls_the_caret_back() {
        let (mut w, _) = laid_out(&view("1234"));
        w.caret = 4;
        let prev = view("1234");
        confirm(&mut w, &prev, "1");
        assert_eq!(w.value, "1");
        assert_eq!(w.caret, 1, "a rejected edit takes the caret with it");
    }

    #[test]
    fn paste_fills_slots_skipping_rejected_characters() {
        let (mut w, size) = laid_out(&view(""));
        let mut state = Codes::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &InputEvent::EditCommand(EditCommand::Paste("12 34-56".into())),
        );
        assert_eq!(state.changes.last().unwrap(), "123456");
        assert_eq!(state.completed, vec!["123456".to_string()]);
        // Confirm the value through rebuild to complete the cycle
        let prev = view("");
        confirm(&mut w, &prev, "123456");
        assert_eq!(w.value, "123456");
    }

    #[test]
    fn paste_stops_when_slots_are_full() {
        let (mut w, size) = laid_out(&view(""));
        let mut state = Codes::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &InputEvent::EditCommand(EditCommand::Paste("1234567".into())),
        );
        assert_eq!(state.changes.last().unwrap(), "123456");
        // The app owns the value: confirm it through rebuild
        let prev = view("");
        confirm(&mut w, &prev, "123456");
        assert_eq!(w.value, "123456");
    }

    #[test]
    fn paste_into_partial_code_fills_from_the_active_slot() {
        let (mut w, size) = laid_out(&view("12"));
        let mut state = Codes::default();
        w.caret = 2; // After the "12"
        dispatch(
            &mut w,
            &mut state,
            size,
            &InputEvent::EditCommand(EditCommand::Paste("34 56".into())),
        );
        assert_eq!(state.changes.last().unwrap(), "123456");
        assert_eq!(state.completed, vec!["123456".to_string()]);
        // Confirm the value through rebuild
        let prev = view("12");
        confirm(&mut w, &prev, "123456");
        assert_eq!(w.value, "123456");
    }

    #[test]
    fn copy_cut_selectall_are_consumed_with_no_effect() {
        let (mut w, size) = laid_out(&view("123"));
        let mut state = Codes::default();
        let cmds = [
            ("Copy", EditCommand::Copy),
            ("Cut", EditCommand::Cut),
            ("SelectAll", EditCommand::SelectAll),
        ];
        for (name, cmd) in cmds {
            let before = state.changes.len();
            let result = dispatch(&mut w, &mut state, size, &InputEvent::EditCommand(cmd));
            assert_eq!(result, EventResult::Handled);
            assert_eq!(state.changes.len(), before, "no effect from {}", name);
        }
    }

    #[test]
    fn paste_with_disabled_field_is_ignored() {
        let (mut w, size) = laid_out(&view("").disabled(true));
        let mut state = Codes::default();
        let result = dispatch(
            &mut w,
            &mut state,
            size,
            &InputEvent::EditCommand(EditCommand::Paste("123456".into())),
        );
        assert_eq!(result, EventResult::Ignored);
        assert!(state.changes.is_empty());
    }

    // ---- Root-driven: cursor, focus, semantics ------------------------------

    /// The only harness that can exercise focus: `PaintCtx` seeds `has_focus`
    /// from the root's own focus path, so the ring and the caret are visible to
    /// a test only through a real `RenderRoot`.
    struct Harness {
        root: frust_core::RenderRoot<Codes, InputOtpView<Codes>>,
        state: Codes,
        tcx: TextContext,
        disabled: bool,
        masked: bool,
    }

    impl Harness {
        fn new(disabled: bool) -> Self {
            Self::themed(disabled, "", crate::theme())
        }

        fn themed(disabled: bool, value: &str, theme: Theme) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Codes {
                    value: value.to_string(),
                    ..Codes::default()
                },
                tcx: TextContext::new(),
                disabled,
                masked: false,
            };
            h.root.set_theme(Box::new(theme));
            h.pass();
            h
        }

        /// A harness whose group is [`InputOtpView::masked`].
        fn masked(value: &str) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Codes {
                    value: value.to_string(),
                    ..Codes::default()
                },
                tcx: TextContext::new(),
                disabled: false,
                masked: true,
            };
            h.root.set_theme(Box::new(crate::theme()));
            h.pass();
            h
        }

        /// Focus the group with a press, leaving the caret at the end of the
        /// confirmed value (a press past the filled prefix clamps to it).
        fn focus(&mut self) {
            let x = LENGTH as f64 * INPUT_OTP_SLOT_SIZE - 1.0;
            self.dispatch(&pointer(PointerPhase::Down, x));
            self.dispatch(&pointer(PointerPhase::Up, x));
        }

        fn frame(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root
                .paint(&mut rec, FrameTime::from_nanos((ms * 1e6) as u64));
            rec
        }

        fn pass(&mut self) {
            let disabled = self.disabled;
            let masked = self.masked;
            let mut logic = move |s: &mut Codes| {
                let value = s.value.clone();
                view(&value).disabled(disabled).masked(masked)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(Size::new(400.0, 200.0), &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn the_cursor_is_the_text_beam_and_not_allowed_when_disabled() {
        let mut h = Harness::new(false);
        h.dispatch(&pointer(PointerPhase::Move, 10.0));
        assert_eq!(h.root.cursor(), CursorIcon::Text);

        let mut disabled = Harness::new(true);
        disabled.dispatch(&pointer(PointerPhase::Move, 10.0));
        assert_eq!(disabled.root.cursor(), style::DISABLED_CURSOR);

        let mut away = Harness::new(false);
        away.dispatch(&pointer(PointerPhase::Move, 380.0));
        assert_eq!(away.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn a_press_claims_focus_so_keys_route_to_the_group() {
        let mut h = Harness::new(false);
        h.dispatch(&pointer(PointerPhase::Down, 10.0));
        assert!(h.root.is_focus_active());
        h.dispatch(&pointer(PointerPhase::Up, 10.0));
        h.dispatch(&typed('7'));
        assert_eq!(h.state.value, "7");
    }

    #[test]
    fn the_active_slot_rings_only_while_focused() {
        let mut h = Harness::themed(false, "12", crate::theme());
        let resting = h.frame(0.0);
        assert!(
            !resting
                .strokes
                .iter()
                .any(|(_, width, _)| *width == style::FOCUS_RING_WIDTH),
            "no ring without focus"
        );

        h.focus();
        let focused = h.frame(0.0);
        let ring = focused
            .strokes
            .iter()
            .find(|(_, width, _)| *width == style::FOCUS_RING_WIDTH)
            .expect("the active slot rings");
        // Two chars confirmed, so the third box is the active one.
        let left = 2.0 * INPUT_OTP_SLOT_SIZE;
        assert!(ring.0.x0 < left && ring.0.x1 > left + INPUT_OTP_SLOT_SIZE);
        assert_eq!(ring.2.components[3], style::FOCUS_RING_OPACITY);

        // ...and the slot's own border swapped to the themed ring color
        // (`data-[active=true]:border-ring`, the other half of the treatment).
        let ring_color = style::ring_color(None, Some(&crate::theme()));
        assert!(
            focused
                .strokes
                .iter()
                .any(|(_, width, color)| *width == style::BORDER_WIDTH && *color == ring_color),
            "the active slot's border is the ring color, not the fallback"
        );
    }

    #[test]
    fn the_fake_caret_blinks_in_the_empty_active_slot() {
        let mut h = Harness::themed(false, "12", crate::theme());
        h.focus();

        let (caret_origin, caret_size, _) =
            h.frame(0.0).caret().expect("a caret in the empty slot");
        assert_eq!(caret_size, Size::new(CARET_WIDTH, CARET_HEIGHT));
        let expected_x = 2.0 * INPUT_OTP_SLOT_SIZE + (INPUT_OTP_SLOT_SIZE - CARET_WIDTH) / 2.0;
        assert!((caret_origin.x - expected_x).abs() < 1e-9);

        // Half a period later it is dark, and a period later lit again.
        let half = INPUT_OTP_BLINK_MS as f64 / 2.0;
        assert!(h.frame(half + 1.0).caret().is_none());
        assert!(h.frame(INPUT_OTP_BLINK_MS as f64 + 1.0).caret().is_some());

        // A full code has no empty slot to put a caret in.
        let mut full = Harness::themed(false, "123456", crate::theme());
        full.focus();
        assert!(full.frame(0.0).caret().is_none());

        // ...and neither does a disabled control.
        let mut disabled = Harness::themed(true, "12", crate::theme());
        disabled.focus();
        assert!(disabled.frame(0.0).caret().is_none());
    }

    #[test]
    fn reduce_motion_freezes_the_caret_lit() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut h = Harness::themed(false, "", theme);
        h.focus();
        for ms in [0.0, INPUT_OTP_BLINK_MS as f64 / 2.0 + 1.0, 5_000.0] {
            assert!(h.frame(ms).caret().is_some(), "frozen visible at {ms}ms");
        }
    }

    #[test]
    fn semantics_reports_one_text_input_carrying_the_whole_code() {
        let mut h = Harness::new(false);
        h.state.value = "1234".into();
        h.pass();
        let update = h.semantics();
        let inputs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::TextInput)
            .collect();
        assert_eq!(inputs.len(), 1, "one control, not one per slot");
        assert_eq!(inputs[0].1.label(), Some("one-time code"));
        assert_eq!(inputs[0].1.value(), Some("1234"));

        let disabled = Harness::new(true);
        let update = disabled.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a Role::TextInput node");
        assert!(node.is_disabled());
    }

    #[test]
    fn masked_reports_a_password_node_with_the_bulleted_code() {
        let h = Harness::masked("1234");
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
        assert_eq!(node.label(), Some("one-time code"));
        assert_eq!(
            node.value(),
            Some("\u{2022}\u{2022}\u{2022}\u{2022}"),
            "the real code never reaches the semantics tree"
        );

        // Unmasked (the default) stays exactly what it always was.
        let h = Harness::new(false);
        let update = h.semantics();
        assert!(
            !update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::PasswordInput)
        );
    }
}
