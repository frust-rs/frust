//! Ports beUI's **Input** — the pill field with a static label above it, a
//! focus ring that fades in, a shake when validation fails and a success check
//! that draws itself on.
//!
//! Source: `components/motion/input.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `input`: *"Text input with label, left/right icons, optional stable
//! error row, error shake and success check draw."*
//!
//! | class / prop | here |
//! |---|---|
//! | wrapper `flex flex-col gap-1.5` | the label / field / message column, [`ROW_GAP`] apart |
//! | label `px-1 text-sm font-medium text-foreground` | [`LABEL_PADDING_X`], [`style::TEXT_SM`], `FontWeight::MEDIUM` |
//! | field `relative h-11 overflow-hidden rounded-full border` | [`style::HEIGHT_INPUT`], [`style::RADIUS_CONTROL`], [`style::BORDER_WIDTH`] |
//! | `border-border` | `outline_variant` |
//! | focused `border-foreground/40 ring-2 ring-ring/40` | [`style::FOCUS_BORDER_ALPHA`], [`style::FOCUS_RING_OPACITY`] |
//! | error `border-destructive ring-2 ring-destructive/25` | `error` at [`style::ERROR_RING_OPACITY`] |
//! | `transition-colors duration-200` | [`switch::TRACK_COLOR_RAMP`] |
//! | `x: [0, -6, 6, -4, 4, -2, 0]`, `duration: 0.45` | [`ERROR_SHAKE_KEYFRAMES`] / [`ERROR_SHAKE_MS`] |
//! | input `pl-3.5 text-base leading-6 placeholder:text-muted-foreground/60` | the wrapped baseline field |
//! | success `h-5 w-5 right-3.5 text-(--color-success)` + `pathLength 0 → 1` | [`SUCCESS_ICON_SIZE`] / [`SUCCESS_ICON_INSET`] / [`SUCCESS_DRAW_MS`] |
//! | message `px-1 text-xs text-destructive` + `opacity/y` | [`style::TEXT_XS`], [`MESSAGE_RISE`] |
//! | `reserveErrorLine` → `min-h-4` | [`MESSAGE_RESERVED_HEIGHT`] |
//! | `disabled` → `opacity-60 cursor-not-allowed` | [`DISABLED_OPACITY`] + [`style::DISABLED_CURSOR`] |
//!
//! # Wrapping the baseline field, not forking it
//!
//! The editable itself is the framework's own [`frust::text_input`]: this
//! widget owns a single [`ChildPod`] holding one and paints beUI's chrome
//! around it. Every editing concern — the text editor, IME and focus
//! publication, the caret, selection, the controlled-value reconcile — stays
//! the baseline's. Three of its builder seams make its own chrome disappear so
//! this one can show: `border_width(0.0)` (leaving only its background fill),
//! `corner_radius` (baked to the pill this field paints) and
//! `padding(`[`style::PADDING_X_INPUT`]`, `[`FIELD_PADDING_Y`]`)`.
//!
//! # Reusable by a sibling control
//!
//! A control that is *a beUI field plus something* — a combobox, a search box —
//! reuses the chrome rather than re-deriving it: [`FieldChrome`] states the
//! interaction state, [`resolve_field_paint`] resolves it against the theme and
//! two `0 → 1` blend values, and [`paint_field_frame`] paints the result.
//! [`field_radius`] is the pill resolution, [`ShapedText`] the shaped-run cache
//! this catalog's text-bearing controls share. All five are crate-internal.
//!
//! # Premise correction: the label does not float
//!
//! Upstream's label is a **static row above the field** (`<label className="px-1
//! text-sm font-medium">`), not a floating/animated one — it neither moves nor
//! scales, and nothing in `input.tsx` animates it. This port keeps it static.
//!
//! # Degradations
//!
//! - **No `leftIcon`/`rightIcon` slots.** Upstream takes two `ReactNode` slots
//!   and re-pads the field to `pl-10`/`pr-10` for them. The port ships the
//!   success check (which upstream renders itself) and leaves caller-supplied
//!   affordances to composition beside the field.
//! - **Symmetric inner padding.** The baseline field's `padding(x, y)` seam is
//!   one horizontal value, so upstream's `pl-3.5` + `pr-10` (with a success
//!   check) becomes [`style::PADDING_X_INPUT`] on both sides; the check sits in
//!   the trailing padding rather than in reserved space.
//! - **The field's background is the baseline's.** Upstream's `<input>` is
//!   `bg-transparent` over an unpainted wrapper; the wrapped field resolves its
//!   own opaque `surface` fill and offers no seam to suppress it. beUI folds
//!   `surface` onto `--background`, so the two agree wherever the field sits on
//!   the page background, and differ on a card.
//! - **No blur on the message's enter/exit.** `filter: "blur(4px)"` has no
//!   counterpart in this scene's paint vocabulary; the opacity-and-rise is
//!   ported, the blur is dropped.
//! - **The success check's easing is Motion's `"easeOut"`, not beUI's own.**
//!   That is upstream's own inconsistency (every other transition in the file
//!   names the catalog curve), and the port keeps it: [`Curve::EaseOut`].

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, CursorIcon,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any, build_child, rebuild_child,
    route_event_single, teardown_child, visit_children,
};
use frust::{Curve, FrameTime, Theme, text_input};

use super::checkbox::{ICON_VIEWBOX, mark_path};
use super::switch;
use crate::motion::Ramp;
use crate::press::{Lane, inside_inclusive as inside, keyframes_at, lerp_color};
use crate::style;
use crate::text::Label as ShapedText;
use crate::tokens::{BeuiTokens, sans_family};

/// Field width used when the incoming constraints are horizontally unbounded —
/// upstream's field is `w-full`, which has no intrinsic width, and the baseline
/// field's own fallback is this number.
pub const UNBOUNDED_WIDTH: f64 = 200.0;

/// Vertical gap between the label, the field and the message row, in logical px
/// (`gap-1.5`).
pub const ROW_GAP: f64 = 6.0;

/// Horizontal padding on the label and the message rows, in logical px
/// (`px-1`) — they sit a hair inside the pill's own curve.
pub const LABEL_PADDING_X: f64 = 4.0;

/// Vertical padding inside the field, in logical px — the `h-11` box less the
/// `leading-6` line, halved, which is what centres the caret.
pub const FIELD_PADDING_Y: f64 = (style::HEIGHT_INPUT - style::LINE_HEIGHT_INPUT) / 2.0;

/// The message row's reserved height when `reserveErrorLine` is set, in logical
/// px (`min-h-4`).
pub const MESSAGE_RESERVED_HEIGHT: f64 = 16.0;

/// How far the message rises into place, in logical px (`y: -4 → 0`).
pub const MESSAGE_RISE: f64 = 4.0;

/// How long the message's entrance and exit take, in milliseconds
/// (`transition={{ duration: 0.2 }}`).
pub const MESSAGE_FADE_MS: f64 = 200.0;

/// The error shake's keyframes, in logical px — upstream's literal `x` array.
///
/// Source: `animate(fieldRef, { x: [0, -6, 6, -4, 4, -2, 0] }, { duration: 0.45
/// })`, fired from an effect the frame a truthy `error` appears.
pub const ERROR_SHAKE_KEYFRAMES: [f64; 7] = [0.0, -6.0, 6.0, -4.0, 4.0, -2.0, 0.0];

/// How long the error shake takes, in milliseconds (`duration: 0.45`).
pub const ERROR_SHAKE_MS: f64 = 450.0;

/// The success check's edge, in logical px (`h-5 w-5`).
pub const SUCCESS_ICON_SIZE: f64 = style::ICON_SIZE_LG;
/// How far the success check sits in from the field's trailing edge, in logical
/// px (`right-3.5`).
pub const SUCCESS_ICON_INSET: f64 = style::PADDING_X_INPUT;
/// The success check's stroke width in viewBox units (`strokeWidth={2.5}`).
pub const SUCCESS_STROKE_VIEWBOX: f64 = 2.5;
/// The success check's vertices, in viewBox units: `M5 12.5l4.5 4.5L19 7.5`.
pub const SUCCESS_POINTS: [Point; 3] = [
    Point::new(5.0, 12.5),
    Point::new(9.5, 17.0),
    Point::new(19.0, 7.5),
];
/// How long the success check draws itself in, in milliseconds
/// (`transition={{ duration: 0.35, ease: "easeOut" }}`).
pub const SUCCESS_DRAW_MS: f64 = 350.0;

/// The message row's fade ramp.
const MESSAGE_RAMP: Ramp = Ramp::eased(Duration::from_millis(200), crate::tokens::motion::EASE_OUT);

/// Opacity of a disabled field: `disabled && "opacity-60"`.
const DISABLED_OPACITY: f32 = 0.6;

/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback error hue — the light table's `--destructive`.
const FALLBACK_DESTRUCTIVE: Color = crate::BEUI_LIGHT.destructive;

/// The interaction state a beUI field's chrome is painted from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldChrome {
    /// Whether the control (or, for a group, its inner control) holds focus.
    pub focused: bool,
    /// Whether the control is in its error state.
    pub error: bool,
    /// Whether the control is disabled.
    pub disabled: bool,
}

/// A beUI field's chrome, resolved off the theme and the two crossfades.
///
/// Resolved *before* the wrapped child paints and consumed after it, because
/// the child paint borrows the `PaintCtx` mutably while a live `&Theme` borrows
/// it immutably — so the theme reads happen up front and only owned colours
/// cross the child's paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FieldPaint {
    /// The 1px border colour, with the disabled treatment already applied.
    pub border: Color,
    /// The ring, if one is showing — already alpha'd, so a fading ring is a
    /// fading colour rather than a fading width.
    pub ring: Option<Color>,
}

/// The radius a beUI field paints: `rounded-full`, resolved against its own box.
pub(crate) fn field_radius(size: Size) -> f64 {
    style::resolve_radius(style::RADIUS_CONTROL, size.width, size.height)
}

/// Resolve a beUI field's border and ring.
///
/// `focus` and `error` are `0 → 1` crossfades on `transition-colors
/// duration-200`, so a field that has just been focused shows a partly-blended
/// border and a partly-faded ring rather than snapping. The error treatment
/// outranks the focus one — it is later in the class list, and it reports the
/// more important state.
pub(crate) fn resolve_field_paint(
    theme: Option<&Theme>,
    chrome: FieldChrome,
    focus: f64,
    error: f64,
) -> FieldPaint {
    let (idle, ink, destructive) = match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.outline_variant, scheme.primary, scheme.error)
        }
        None => (FALLBACK_BORDER, FALLBACK_FOREGROUND, FALLBACK_DESTRUCTIVE),
    };
    let ring_token = BeuiTokens::resolve_ring(None, theme);

    let focused_border = lerp_color(
        idle,
        style::with_alpha(ink, style::FOCUS_BORDER_ALPHA),
        focus,
    );
    let border = lerp_color(focused_border, destructive, error);

    // `ring-2` appears only in the focused or errored state; between them the
    // errored colour wins, on the same ladder as the border.
    let focus_ring = style::with_alpha(ring_token, style::FOCUS_RING_OPACITY * focus as f32);
    let error_ring = style::with_alpha(destructive, style::ERROR_RING_OPACITY * error as f32);
    let showing = focus.max(error) > 0.0;
    let ring = showing.then(|| lerp_color(focus_ring, error_ring, error));

    FieldPaint {
        border: style::disabled_tint(border, chrome.disabled, DISABLED_OPACITY),
        ring: ring.map(|c| style::disabled_tint(c, chrome.disabled, DISABLED_OPACITY)),
    }
}

/// Paint a resolved field frame: the hairline border, then the ring outside it.
///
/// Called **after** the wrapped child has painted — the child's background fill
/// is opaque, so a border stroked before it would be covered.
pub(crate) fn paint_field_frame(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    paint: FieldPaint,
) {
    let radius = field_radius(size);
    let inset = style::BORDER_WIDTH / 2.0;
    let border = RoundedRect::from_rect(
        Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
        (radius - inset).max(0.0),
    );
    scene.stroke_path(
        origin,
        &Shape::to_path(&border, style::PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(paint.border),
    );
    if let Some(ring) = paint.ring {
        // Tailwind's `ring` is a non-inset box shadow at offset 0, so it sits
        // just outside the border box.
        let out = style::FOCUS_RING_WIDTH / 2.0;
        let outline = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(out),
            radius + out,
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&outline, style::PATH_TOLERANCE),
            style::FOCUS_RING_WIDTH,
            &Brush::Solid(ring),
        );
    }
}

/// A view-held, typed text callback (erased by the wrapped baseline field).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI text field. See the [module docs](self).
pub struct InputView<State: 'static> {
    value: String,
    placeholder: String,
    label: Option<String>,
    error: Option<String>,
    /// A truthy-but-messageless error (upstream's `error={true}`): the shake
    /// and the destructive chrome, with no message row.
    error_flag: bool,
    reserve_message_line: bool,
    success: bool,
    disabled: bool,
    password: bool,
    on_change: OnText<State>,
    on_submit: Option<OnText<State>>,
}

/// Create a controlled beUI text field showing `value`, reporting each edit
/// through `on_change(state, new_text)`.
///
/// Controlled exactly like the baseline field it wraps: the widget never owns
/// the durable value, and an app that rejects or transforms the requested text
/// in `on_change` sees its own value win on the next rebuild.
pub fn input<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> InputView<State> {
    InputView {
        value: value.into(),
        placeholder: String::new(),
        label: None,
        error: None,
        error_flag: false,
        reserve_message_line: false,
        success: false,
        disabled: false,
        password: false,
        on_change: Rc::new(on_change),
        on_submit: None,
    }
}

impl<State: 'static> InputView<State> {
    /// Set the static label row above the field (`label`). It does not float —
    /// see the [module docs](self).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the placeholder shown while the field is empty
    /// (`placeholder:text-muted-foreground/60`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the Enter handler (the baseline field's `on_submit`).
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Put the field in its error state with a message — upstream's
    /// `error={"…"}`: the destructive border and ring, the shake, and the
    /// message row.
    pub fn error(mut self, message: impl Into<String>) -> Self {
        self.error = Some(message.into());
        self.error_flag = true;
        self
    }

    /// Put the field in its error state with **no** message — upstream's
    /// `error={true}`: the destructive chrome and the shake, and nothing said.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.error_flag = invalid;
        if !invalid {
            self.error = None;
        }
        self
    }

    /// Reserve one message line even when there is no message
    /// (`reserveErrorLine`), so validation appearing does not shift what is
    /// below the field.
    pub fn reserve_message_line(mut self, reserve: bool) -> Self {
        self.reserve_message_line = reserve;
        self
    }

    /// Show the success check, drawn on over [`SUCCESS_DRAW_MS`] (`success`).
    pub fn success(mut self, success: bool) -> Self {
        self.success = success;
        self
    }

    /// Disable the field: inert (it refuses focus, so keyboard and IME editing
    /// are unreachable), dimmed, and asking for [`style::DISABLED_CURSOR`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mask the rendered glyphs (`type="password"` upstream; the baseline
    /// field's `obscured` mode, which also publishes the secret IME hint).
    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    /// The wrapped baseline field, configured with its own chrome suppressed.
    fn control(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        let mut field = text_input(self.value.clone(), move |state: &mut State, text| {
            on_change(state, text)
        })
        .placeholder(self.placeholder.clone())
        .enabled(!self.disabled)
        .obscured(self.password)
        .padding(style::PADDING_X_INPUT, FIELD_PADDING_Y)
        .border_width(0.0)
        .corner_radius(style::HEIGHT_INPUT / 2.0);
        if let Some(on_submit) = self.on_submit.clone() {
            field = field.on_submit(move |state: &mut State, text| on_submit(state, text));
        }
        any(field)
    }
}

/// The label's text style: `text-sm font-medium text-foreground`.
fn label_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: style::TEXT_SM as f32,
        color,
        ..TextStyle::default()
    }
}

/// The message's text style: `text-xs text-destructive`.
fn message_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_XS as f32,
        color,
        ..TextStyle::default()
    }
}

/// The retained widget for an [`InputView`].
pub struct InputWidget {
    child: ChildPod,
    label: Option<ShapedText>,
    message: Option<ShapedText>,
    /// The message kept mounted through its exit, once the app has cleared it.
    message_leaving: bool,
    error: bool,
    reserve_message_line: bool,
    success: bool,
    disabled: bool,
    /// The focus crossfade, `0.0` idle .. `1.0` focused.
    focus: Lane,
    /// The error crossfade, `0.0` valid .. `1.0` errored.
    error_blend: Lane,
    /// The message row's presence, `0.0` gone .. `1.0` shown.
    message_presence: Lane,
    /// An error that has just appeared and owes a shake; `paint` stamps the
    /// start (neither a `BuildCtx` nor an `EventCtx` carries a clock).
    shake_armed: bool,
    /// The frame the running shake started on.
    shake_start: Option<FrameTime>,
    /// The success check's draw-on: armed by `rebuild`, stamped by `paint`.
    success_armed: bool,
    /// The frame the success check began drawing on.
    success_start: Option<FrameTime>,
    /// The field row's box, resolved by layout and read by the event pass.
    field: Rect,
    /// Whether the wrapped field held focus on the last paint, so the crossfade
    /// can be retargeted from `paint` (focus is only observable there).
    was_focused: bool,
}

impl<State: 'static> View<State> for InputView<State> {
    type Element = InputWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> InputWidget {
        let error = self.error_flag;
        InputWidget {
            child: build_child(&self.control(), ctx),
            label: self.label.as_ref().map(ShapedText::new),
            message: self.error.as_ref().map(ShapedText::new),
            message_leaving: false,
            error,
            reserve_message_line: self.reserve_message_line,
            success: self.success,
            disabled: self.disabled,
            focus: Lane::at_rest(switch::TRACK_COLOR_RAMP, 0.0),
            error_blend: Lane::at_rest(switch::TRACK_COLOR_RAMP, if error { 1.0 } else { 0.0 }),
            message_presence: Lane::at_rest(
                MESSAGE_RAMP,
                if self.error.is_some() { 1.0 } else { 0.0 },
            ),
            // `<AnimatePresence initial={false}>`: a field that mounts errored
            // does not shake, and one that mounts successful is already drawn.
            shake_armed: false,
            shake_start: None,
            success_armed: false,
            success_start: None,
            field: Rect::ZERO,
            was_focused: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut InputWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.control(), &self.control(), &mut element.child, ctx);

        if prev.label != self.label {
            match (&mut element.label, &self.label) {
                (Some(existing), Some(text)) => {
                    existing.set_content(text.clone());
                }
                (slot @ None, Some(text)) => *slot = Some(ShapedText::new(text.clone())),
                (slot, None) => *slot = None,
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.error_flag != self.error_flag {
            element.error = self.error_flag;
            element
                .error_blend
                .retarget(if self.error_flag { 1.0 } else { 0.0 });
            if self.error_flag {
                // The shake fires the frame a truthy `error` appears.
                element.shake_armed = true;
                element.shake_start = None;
            }
            flags |= ChangeFlags::PAINT;
        }

        if prev.error != self.error {
            match (&mut element.message, &self.error) {
                (Some(existing), Some(text)) => {
                    existing.set_content(text.clone());
                    element.message_leaving = false;
                }
                (slot @ None, Some(text)) => *slot = Some(ShapedText::new(text.clone())),
                (Some(_), None) => element.message_leaving = true,
                (None, None) => {}
            }
            element
                .message_presence
                .retarget(if self.error.is_some() { 1.0 } else { 0.0 });
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.reserve_message_line != self.reserve_message_line {
            element.reserve_message_line = self.reserve_message_line;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.success != self.success {
            element.success = self.success;
            if self.success {
                element.success_armed = true;
                element.success_start = None;
            }
            flags |= ChangeFlags::PAINT;
        }

        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut InputWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.control(), &mut element.child, ctx);
    }
}

impl InputWidget {
    /// The error shake's x offset at `now`, and whether it is still running.
    fn shake_offset(&self, now: FrameTime) -> (f64, bool) {
        let Some(start) = self.shake_start else {
            return (0.0, false);
        };
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
        if elapsed >= ERROR_SHAKE_MS {
            return (0.0, false);
        }
        (
            keyframes_at(&ERROR_SHAKE_KEYFRAMES, elapsed / ERROR_SHAKE_MS),
            true,
        )
    }

    /// The success check's draw progress at `now`, and whether it owes a frame.
    fn success_progress(&self, now: FrameTime) -> (f64, bool) {
        if !self.success {
            return (0.0, false);
        }
        let Some(start) = self.success_start else {
            // No run pending: a field that mounted successful is simply drawn.
            return (1.0, false);
        };
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;
        if elapsed >= SUCCESS_DRAW_MS {
            return (1.0, false);
        }
        (Curve::EaseOut.transform(elapsed / SUCCESS_DRAW_MS), true)
    }

    /// The message row's height at the current presence — reserved, measured,
    /// or nothing.
    fn message_height(&self) -> f64 {
        let measured = self
            .message
            .as_ref()
            .filter(|_| !self.message_leaving || self.message_presence.value() > 0.0)
            .map_or(0.0, |m| m.size().height);
        if self.reserve_message_line {
            measured.max(MESSAGE_RESERVED_HEIGHT)
        } else {
            measured
        }
    }
}

impl Widget for InputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (ink, destructive) = match theme {
            Some(theme) => (theme.scheme().on_surface, theme.scheme().error),
            None => (FALLBACK_FOREGROUND, FALLBACK_DESTRUCTIVE),
        };
        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);

        let label_size = match &mut self.label {
            Some(label) => label.layout(ctx, &label_style(tint(ink))),
            None => Size::ZERO,
        };
        if let Some(message) = &mut self.message {
            message.layout(ctx, &message_style(destructive));
        }

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };

        let mut y = 0.0;
        if self.label.is_some() {
            y += label_size.height + ROW_GAP;
        }
        self.field =
            Rect::from_origin_size(Point::new(0.0, y), Size::new(width, style::HEIGHT_INPUT));
        // Tight constraints are what pin the wrapped field to `h-11`: the
        // baseline would otherwise report its own text-derived height, leaving
        // its opaque background smaller than this field's border box.
        self.child
            .layout_child(ctx, &BoxConstraints::tight(self.field.size()));
        self.child.set_origin(self.field.origin());
        y += style::HEIGHT_INPUT;

        let message_height = self.message_height();
        if message_height > 0.0 {
            y += ROW_GAP + message_height;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The pod's focus path is authoritative (a container-routed blur never
        // reaches this widget's `event`); a disabled field never reads focused.
        let focused = ctx.has_focus() && !self.disabled;
        if focused != self.was_focused {
            self.was_focused = focused;
            self.focus.retarget(if focused { 1.0 } else { 0.0 });
        }

        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();

        let mut owes_frame = false;
        if reduce {
            self.focus.snap();
            self.error_blend.snap();
            self.message_presence.snap();
            self.shake_armed = false;
            self.shake_start = None;
            self.success_armed = false;
            self.success_start = None;
        } else {
            if self.shake_armed {
                self.shake_armed = false;
                self.shake_start = Some(now);
            }
            if self.success_armed {
                self.success_armed = false;
                self.success_start = Some(now);
            }
            owes_frame |= self.focus.advance(now);
            owes_frame |= self.error_blend.advance(now);
            owes_frame |= self.message_presence.advance(now);
        }
        let (shake, shaking) = self.shake_offset(now);
        if !shaking {
            self.shake_start = None;
        }
        owes_frame |= shaking;
        let (drawn, drawing) = self.success_progress(now);
        if !drawing {
            self.success_start = None;
        }
        owes_frame |= drawing;

        let chrome = FieldChrome {
            focused,
            error: self.error,
            disabled: self.disabled,
        };
        let paint = resolve_field_paint(
            theme,
            chrome,
            self.focus.value().clamp(0.0, 1.0),
            self.error_blend.value().clamp(0.0, 1.0),
        );
        let success_color = style::disabled_tint(
            BeuiTokens::resolve(theme).success,
            self.disabled,
            DISABLED_OPACITY,
        );
        let origin = ctx.origin();

        if let Some(label) = &self.label {
            label.paint(Point::new(origin.x + LABEL_PADDING_X, origin.y), scene);
        }

        // The shake moves the field row only — upstream animates `fieldRef`,
        // not the label or the message.
        let field_origin = origin + self.field.origin().to_vec2();
        scene.push_transform(Affine::translate((shake, 0.0)));
        self.child.paint_child(ctx, scene);
        paint_field_frame(scene, field_origin, self.field.size(), paint);

        if self.success && drawn > 0.0 {
            let path = mark_path(&SUCCESS_POINTS, SUCCESS_ICON_SIZE, drawn);
            let icon = Point::new(
                field_origin.x + self.field.width() - SUCCESS_ICON_INSET - SUCCESS_ICON_SIZE,
                field_origin.y + (style::HEIGHT_INPUT - SUCCESS_ICON_SIZE) / 2.0,
            );
            scene.stroke_path(
                icon,
                &path,
                SUCCESS_STROKE_VIEWBOX * SUCCESS_ICON_SIZE / ICON_VIEWBOX,
                &Brush::Solid(success_color),
            );
        }
        scene.pop_transform();

        let presence = self.message_presence.value().clamp(0.0, 1.0);
        if let Some(message) = &self.message
            && presence > 0.0
        {
            let row_y = origin.y + self.field.max_y() + ROW_GAP;
            let rise = MESSAGE_RISE * (1.0 - presence);
            let size = message.size();
            scene.push_layer(
                Point::new(origin.x, row_y - MESSAGE_RISE),
                Size::new(size.width.max(1.0), size.height + MESSAGE_RISE),
                presence as f32,
            );
            message.paint(Point::new(origin.x + LABEL_PADDING_X, row_y - rise), scene);
            scene.pop_layer();
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let result = route_event_single(&mut self.child, ctx, event);
        // The cursor is asked for from the uncaptured `Move` arm only, and
        // after routing so this widget's shape wins over the child's (the
        // baseline field asks for none of its own): `Text` over the editable,
        // `NotAllowed` when disabled — and nothing at all off the field row,
        // which is what keeps the label and message rows cursor-neutral.
        if let InputEvent::Pointer(p) = event
            && matches!(p.phase, PointerPhase::Move)
            && inside(
                p.position - self.field.origin().to_vec2(),
                self.field.size(),
            )
        {
            ctx.set_cursor(if self.disabled {
                style::DISABLED_CURSOR
            } else {
                CursorIcon::Text
            });
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Chrome only: the wrapped field contributes the `TextInput`/
        // `PasswordInput` node, its value and its disabled/read-only state.
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, PointerButton, PointerEvent};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// Records the ops these tests assert on: rounded-rect fills (the wrapped
    /// field's background), stroked paths (border, ring, success check), glyph
    /// run origins, layer alphas and transform pushes.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        glyphs: Vec<Point>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
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
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
    }

    impl Recorder {
        /// The single 1px border stroke of the pass.
        fn border(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w == style::BORDER_WIDTH)
                .expect("a 1px border was stroked")
        }

        /// The ring stroke of the pass, if any.
        fn ring(&self) -> Option<(Rect, f64, Color)> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .copied()
        }

        /// The success check, if the pass drew one — the only stroke whose
        /// width is neither the border's nor the ring's.
        fn check(&self) -> Option<(Rect, f64, Color)> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w != style::BORDER_WIDTH && *w != style::FOCUS_RING_WIDTH)
                .copied()
        }
    }

    /// The window the harness lays out in.
    const WINDOW: Size = Size::new(260.0, 160.0);

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    /// What the harness's rebuilt view carries.
    #[derive(Clone, Default)]
    struct Props {
        label: Option<String>,
        error: Option<String>,
        invalid: bool,
        reserve: bool,
        success: bool,
        disabled: bool,
    }

    /// A field driven through a real `RenderRoot`, which is what makes the
    /// focus-path read (`PaintCtx::has_focus`) — and therefore the ring —
    /// observable at all.
    struct Harness {
        root: RenderRoot<String, InputView<String>>,
        state: String,
        tcx: TextContext,
        props: Props,
    }

    impl Harness {
        fn new(props: Props) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: String::new(),
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
            let mut logic = move |state: &mut String| {
                let mut view = input::<String, _>(state.clone(), |s: &mut String, t| *s = t)
                    .placeholder("Email")
                    .invalid(props.invalid)
                    .reserve_message_line(props.reserve)
                    .success(props.success)
                    .disabled(props.disabled);
                if let Some(label) = &props.label {
                    view = view.label(label.clone());
                }
                if let Some(error) = &props.error {
                    view = view.error(error.clone());
                }
                view
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn paint_at(&mut self, millis: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(millis));
            rec
        }

        fn paint(&mut self) -> Recorder {
            self.paint_at(0.0)
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }

        fn focus_the_field(&mut self) {
            self.pointer(PointerPhase::Down, 40.0, 20.0);
            self.pointer(PointerPhase::Up, 40.0, 20.0);
        }
    }

    fn build<S: 'static>(view: &InputView<S>) -> InputWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut InputWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    // ---- Layout -----------------------------------------------------------

    #[test]
    fn a_bare_field_is_h11_and_the_wrapped_control_fills_it() {
        let view: InputView<String> = input("hi", |_s: &mut String, _t| {});
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        assert_eq!(size, Size::new(200.0, style::HEIGHT_INPUT));
        assert_eq!(w.child.size(), size);
        assert_eq!(w.child.origin(), Point::ORIGIN);
        assert_eq!(w.field, Rect::from_origin_size(Point::ORIGIN, size));
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_baseline_default() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {});
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 400.0)),
        );
        assert_eq!(size.width, UNBOUNDED_WIDTH);
    }

    #[test]
    fn a_label_pushes_the_field_down_by_its_height_plus_the_gap() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {}).label("Email");
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        let label_height = w.label.as_ref().expect("a label").size().height;
        assert!(label_height > 0.0, "the label shaped to something");
        assert!((w.field.y0 - (label_height + ROW_GAP)).abs() < 1e-9);
        assert!((size.height - (label_height + ROW_GAP + style::HEIGHT_INPUT)).abs() < 1e-9);
        assert_eq!(w.child.origin(), w.field.origin());
    }

    #[test]
    fn a_message_adds_a_row_and_a_reserved_line_holds_the_height_without_one() {
        let with_message: InputView<String> = input("", |_s: &mut String, _t| {}).error("Required");
        let mut w = build(&with_message);
        let tall = layout(&mut w, 200.0);
        let message_height = w.message.as_ref().expect("a message").size().height;
        assert!((tall.height - (style::HEIGHT_INPUT + ROW_GAP + message_height)).abs() < 1e-9);

        // Reserved but empty: the row is `min-h-4` tall so nothing below moves
        // when the message arrives.
        let reserved: InputView<String> =
            input("", |_s: &mut String, _t| {}).reserve_message_line(true);
        let mut w = build(&reserved);
        let held = layout(&mut w, 200.0);
        assert!(
            (held.height - (style::HEIGHT_INPUT + ROW_GAP + MESSAGE_RESERVED_HEIGHT)).abs() < 1e-9
        );

        // Unreserved and empty: no row at all.
        let bare: InputView<String> = input("", |_s: &mut String, _t| {});
        let mut w = build(&bare);
        assert_eq!(layout(&mut w, 200.0).height, style::HEIGHT_INPUT);
    }

    // ---- Chrome -----------------------------------------------------------

    #[test]
    fn an_idle_field_strokes_the_border_token_at_the_pill_radius_and_no_ring() {
        let mut h = Harness::new(Props::default());
        let rec = h.paint();
        let (bbox, width, color) = rec.border();
        assert_eq!(width, style::BORDER_WIDTH);
        assert_eq!(
            color,
            crate::theme().scheme().outline_variant,
            "border-border"
        );
        assert!((bbox.height() - style::HEIGHT_INPUT).abs() < 2.0 * style::BORDER_WIDTH);
        assert!(rec.ring().is_none(), "no ring at rest");
        // The wrapped baseline field painted its own background.
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == crate::theme().scheme().surface),
            "the wrapped baseline field is there"
        );
        assert_eq!(field_radius(Size::new(200.0, style::HEIGHT_INPUT)), 22.0);
    }

    #[test]
    fn focus_blends_the_border_toward_foreground_and_fades_the_ring_in() {
        let mut h = Harness::new(Props::default());
        h.focus_the_field();
        // The first painted frame seeds the crossfade's clock: the ring is
        // present but has not reached its token alpha.
        h.paint_at(0.0);
        let mid = h.paint_at(60.0);
        let ring = mid.ring().expect("a fading ring");
        assert!(
            ring.2.components[3] > 0.0 && ring.2.components[3] < style::FOCUS_RING_OPACITY,
            "mid-fade ring alpha {}",
            ring.2.components[3]
        );

        let settled = h.paint_at(3_000.0);
        let (bbox, width, color) = settled.ring().expect("a settled ring");
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        let expected = style::with_alpha(
            BeuiTokens::resolve_ring(None, Some(&crate::theme())),
            style::FOCUS_RING_OPACITY,
        );
        assert_eq!(color, expected, "ring-ring/40");
        assert!(bbox.x0 < 0.0, "a non-inset ring sits outside the box");
        assert_eq!(
            settled.border().2,
            style::with_alpha(crate::theme().scheme().primary, style::FOCUS_BORDER_ALPHA),
            "border-foreground/40"
        );
    }

    #[test]
    fn an_error_paints_a_destructive_border_and_ring_even_unfocused() {
        let mut h = Harness::new(Props {
            invalid: true,
            ..Props::default()
        });
        let rec = h.paint_at(3_000.0);
        let theme = crate::theme();
        assert_eq!(rec.border().2, theme.scheme().error, "border-destructive");
        let ring = rec.ring().expect("ring-destructive/25");
        assert_eq!(
            ring.2,
            style::with_alpha(theme.scheme().error, style::ERROR_RING_OPACITY)
        );
    }

    #[test]
    fn the_error_treatment_outranks_the_focus_one() {
        let theme = crate::theme();
        let chrome = FieldChrome {
            focused: true,
            error: true,
            disabled: false,
        };
        let paint = resolve_field_paint(Some(&theme), chrome, 1.0, 1.0);
        assert_eq!(paint.border, theme.scheme().error);
        assert_eq!(
            paint.ring,
            Some(style::with_alpha(
                theme.scheme().error,
                style::ERROR_RING_OPACITY
            ))
        );
    }

    #[test]
    fn a_disabled_field_dims_its_chrome_and_refuses_focus() {
        let mut h = Harness::new(Props {
            disabled: true,
            ..Props::default()
        });
        let idle = h.paint();
        let enabled = crate::theme().scheme().outline_variant;
        assert!(
            (idle.border().2.components[3] - enabled.components[3] * DISABLED_OPACITY).abs() < 1e-6,
            "opacity-60 on the chrome"
        );

        h.focus_the_field();
        assert!(h.paint_at(3_000.0).ring().is_none(), "inert, so no ring");
    }

    #[test]
    fn unthemed_chrome_falls_back_to_the_vendored_light_table() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {});
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.border().2, FALLBACK_BORDER);
    }

    // ---- Motion -----------------------------------------------------------

    #[test]
    fn an_error_appearing_shakes_the_field_row_and_then_stops() {
        let mut h = Harness::new(Props::default());
        assert_eq!(
            h.paint().transforms[0].translation().x,
            0.0,
            "a valid field sits still"
        );

        h.props.invalid = true;
        h.pass();
        // The first painted frame stamps the shake's start.
        assert_eq!(h.paint_at(0.0).transforms[0].translation().x, 0.0);
        let shaken = h.paint_at(ERROR_SHAKE_MS / 6.0);
        assert!(
            shaken.transforms[0].translation().x < 0.0,
            "the first keyframe leans left"
        );
        assert_eq!(
            h.paint_at(ERROR_SHAKE_MS + 1.0).transforms[0]
                .translation()
                .x,
            0.0,
            "and it comes to rest"
        );
    }

    #[test]
    fn a_field_that_mounts_errored_does_not_shake() {
        let mut h = Harness::new(Props {
            invalid: true,
            ..Props::default()
        });
        assert_eq!(
            h.paint_at(ERROR_SHAKE_MS / 6.0).transforms[0]
                .translation()
                .x,
            0.0
        );
    }

    #[test]
    fn the_success_check_draws_itself_on_and_settles_whole() {
        let mut h = Harness::new(Props::default());
        assert!(h.paint().check().is_none(), "nothing to draw");

        h.props.success = true;
        h.pass();
        assert!(
            h.paint_at(0.0).check().is_none(),
            "the pen has not moved on the stamping frame"
        );

        let mid = h.paint_at(SUCCESS_DRAW_MS * 0.3).check().expect("a prefix");
        let full = mark_path(&SUCCESS_POINTS, SUCCESS_ICON_SIZE, 1.0).bounding_box();
        assert!(mid.0.width() < full.width());

        let (bbox, width, color) = h
            .paint_at(SUCCESS_DRAW_MS + 1.0)
            .check()
            .expect("the whole check");
        assert!((bbox.width() - full.width()).abs() < 1e-9);
        assert!((width - SUCCESS_STROKE_VIEWBOX * SUCCESS_ICON_SIZE / ICON_VIEWBOX).abs() < 1e-9);
        assert_eq!(color, BeuiTokens::beui().success, "text-(--color-success)");
        // It sits `right-3.5` in from the trailing edge, vertically centred.
        assert!((bbox.x1 - (WINDOW.width - SUCCESS_ICON_INSET)).abs() < SUCCESS_ICON_SIZE);
    }

    #[test]
    fn the_message_rises_into_place_and_is_kept_mounted_through_its_exit() {
        let mut h = Harness::new(Props {
            error: Some("Required".into()),
            invalid: true,
            ..Props::default()
        });
        // A message that mounts with the field is simply there.
        let rec = h.paint();
        assert_eq!(rec.layers, vec![1.0]);
        let settled_y = *rec.glyphs.last().expect("the message glyphs");

        h.props.error = None;
        h.props.invalid = false;
        h.pass();
        h.paint_at(0.0);
        let mid = h.paint_at(MESSAGE_FADE_MS / 2.0);
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "mid-fade alpha {alpha}");
        let leaving = *mid.glyphs.last().expect("still mounted while it leaves");
        assert!(leaving.y < settled_y.y, "and it rises as it goes");

        let gone = h.paint_at(3_000.0);
        assert!(gone.layers.is_empty(), "nothing left to composite");
    }

    #[test]
    fn reduce_motion_lands_every_treatment_at_once() {
        let mut h = Harness::new(Props::default());
        h.reduce_motion();
        h.focus_the_field();
        let rec = h.paint_at(0.0);
        let expected = style::with_alpha(
            BeuiTokens::resolve_ring(None, Some(&crate::theme())),
            style::FOCUS_RING_OPACITY,
        );
        assert_eq!(rec.ring().expect("a ring, at once").2, expected);
        assert_eq!(rec.transforms[0].translation().x, 0.0, "and no shake");
    }

    // ---- Editing, cursor, semantics ---------------------------------------

    #[test]
    fn typing_round_trips_through_the_callback_and_never_self_mutates() {
        let mut h = Harness::new(Props::default());
        h.focus_the_field();
        let key = |h: &mut Harness, ch: char| {
            h.root.event(
                &mut h.state,
                &InputEvent::Key(frust::authoring::KeyEvent {
                    key: frust::authoring::Key::Character(ch.to_string()),
                    modifiers: frust::authoring::Modifiers::default(),
                    repeat: false,
                }),
            );
        };
        key(&mut h, 'a');
        key(&mut h, 'b');
        assert_eq!(h.state, "ab", "each edit reported through on_change");

        // The app rejecting an edit wins: the next rebuild feeds its own value
        // back down, which is the controlled contract the baseline implements.
        h.state = "frozen".to_string();
        h.pass();
        assert_eq!(h.state, "frozen");
    }

    #[test]
    fn the_cursor_is_text_over_the_field_row_only() {
        let mut h = Harness::new(Props {
            label: Some("Email".into()),
            ..Props::default()
        });
        let field_y = h.root.layout(WINDOW);
        let _ = field_y;
        h.pointer(PointerPhase::Move, 40.0, 4.0);
        assert_eq!(h.root.cursor(), CursorIcon::Default, "over the label row");

        h.pointer(PointerPhase::Move, 40.0, 40.0);
        assert_eq!(h.root.cursor(), CursorIcon::Text);

        let mut disabled = Harness::new(Props {
            disabled: true,
            ..Props::default()
        });
        disabled.pointer(PointerPhase::Move, 40.0, 20.0);
        assert_eq!(disabled.root.cursor(), style::DISABLED_CURSOR);
    }

    #[test]
    fn semantics_forwards_the_wrapped_fields_node() {
        let h = Harness::new(Props::default());
        let update = h.root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == frust::authoring::Role::TextInput),
            "the wrapped editable's node reaches the tree through the chrome"
        );
    }

    #[test]
    fn visit_children_publishes_the_wrapped_field() {
        let view: InputView<String> = input("", |_s: &mut String, _t| {});
        let w = build(&view);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }

    #[test]
    fn dark_mode_resolves_its_own_border_and_ring() {
        let mut h = Harness::new(Props::default());
        h.root
            .set_theme(Box::new(crate::theme().with_brightness(Brightness::Dark)));
        h.pass();
        let dark = crate::theme().with_brightness(Brightness::Dark);
        assert_eq!(h.paint().border().2, dark.scheme().outline_variant);
        assert_ne!(
            BeuiTokens::resolve_ring(None, Some(&dark)),
            BeuiTokens::resolve_ring(None, Some(&crate::theme())),
            "the ring follows brightness with no component involvement"
        );
    }

    // ---- The reusable seam ------------------------------------------------

    #[test]
    fn the_field_seam_resolves_and_paints_without_a_widget() {
        let theme = crate::theme();
        let idle = resolve_field_paint(Some(&theme), FieldChrome::default(), 0.0, 0.0);
        assert_eq!(idle.border, theme.scheme().outline_variant);
        assert!(idle.ring.is_none(), "no ring outside focus or error");

        let mut rec = Recorder::default();
        let size = Size::new(120.0, style::HEIGHT_INPUT);
        paint_field_frame(&mut rec, Point::ORIGIN, size, idle);
        assert_eq!(rec.strokes.len(), 1, "the border alone");
        assert_eq!(rec.border().1, style::BORDER_WIDTH);
    }
    // `ShapedText` (aliasing `crate::text::Label`) carries its own leaf test
    // in `crate::text`'s test module now.
}
