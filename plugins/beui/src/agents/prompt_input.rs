//! Ports beUI's `prompt-input` agent-interface part.
//!
//! **Source:** `components/agents/prompt-input.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `prompt-input`: *"An auto-growing agent composer with prompt actions,
//! model selection, keyboard submission, and animated send and stop states."*
//!
//! # Editing is the baseline field's, never this widget's
//!
//! The editable is the framework's own [`frust::text_input`] in its multi-line
//! mode, exactly as [`input`](crate::components::input) wraps it for the pill
//! field: this widget owns a single [`ChildPod`] holding one and paints beUI's
//! composer chrome around it. Every editing concern — the text editor, IME and
//! focus publication, the caret, selection, the auto-grow up to `maxRows`, the
//! controlled-value reconcile — stays the baseline's.
//!
//! That also settles the keyboard contract for free, because the baseline field
//! already has it: `multiline(max_rows)` plus
//! `submit_on_enter(true)` is **Enter sends, Shift+Enter inserts a newline**,
//! which is upstream's `handleKeyDown` verbatim (`event.key === "Enter" &&
//! !event.shiftKey`). Nothing here re-implements it.
//!
//! # Geometry
//!
//! | class | here |
//! |---|---|
//! | `rounded-2xl border border-border/80 bg-background p-2` | [`style::RADIUS_2XL`], [`PROMPT_PADDING`] |
//! | `focus-within:border-foreground/25` | [`style::FOCUS_BORDER_ALPHA`]-scaled, crossfaded on a [`Lane`] |
//! | textarea `px-2 pt-1.5 text-sm leading-6` | [`PROMPT_FIELD_PADDING_X`] / [`PROMPT_FIELD_PADDING_Y`] |
//! | `rows={minRows}` … `maxRows * 24` | [`PROMPT_MIN_ROWS`] / [`PROMPT_MAX_ROWS`] × [`style::LINE_HEIGHT_INPUT`] |
//! | action row `mt-1 flex min-h-8` | [`PROMPT_ACTION_GAP`] / [`PROMPT_ACTION_ROW_HEIGHT`] |
//! | send button `ml-auto size-8 rounded-full` | [`PROMPT_BUTTON_EDGE`] |
//! | `disabled` → `opacity-60` | [`style::DISABLED_OPACITY_INPUT`] |
//!
//! # The send button's three states
//!
//! [`PromptInputSend`] is the resolved state, and it is **derived, never set**:
//! upstream computes `canSubmit = Boolean(value.trim()) && !disabled &&
//! !loading` and swaps the icon on `loading`, so the three states fall out of
//! the same three inputs here. The
//! [`Ready`](PromptInputSend::Ready) → [`Streaming`](PromptInputSend::Streaming)
//! swap is staged by a [`Presence`] on `SPRING_SWAP` — upstream's
//! `AnimatePresence mode="popLayout"` with `{opacity, y: ±3, scale: 0.8}` — so
//! the arrow leaves as the stop square arrives rather than one replacing the
//! other on a frame boundary.
//!
//! # Degradations against upstream
//!
//! - **No model selector and no prompt-actions popover.** Upstream mounts a
//!   `Select` and a `MorphPopover` in its action row. Both are shipped
//!   components ([`select`](crate::components::select),
//!   [`popover`](crate::components::popover)); wiring them in from here would
//!   make this widget the owner of two overlay hosts and three more callbacks,
//!   so the action row is left as space a caller composes into. The send button
//!   itself stays, because its state *is* the composer's state.
//! - **The auto-grow measurement is the field's, not a mirror div.** Upstream
//!   measures a hidden `whitespace-pre-wrap` copy to size the textarea. The
//!   baseline field measures its own wrapped content and caps at
//!   `max_visible_lines`, which is the same result without the mirror.
//! - **The field's background is the baseline's** — the same note
//!   [`input`](crate::components::input) records: the wrapped field resolves its
//!   own opaque `surface` fill, and beUI folds `surface` onto `--background`, so
//!   the two agree wherever the composer sits on the page background.
//! - **No leading-action slot.** Upstream's `leadingAction` node has no
//!   counterpart; compose beside the composer.

use std::rc::Rc;

use frust::authoring::{Action, ErasedArgCallback, erase_callback_arg};
use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any,
    build_child, erase_callback, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{Theme, text_input};

use crate::components::switch::TRACK_COLOR_RAMP;
use crate::motion::{Presence, Ramp};
use crate::press::{Lane, inside_inclusive as inside, is_activation_key, lerp_color, presses};
use crate::style::{self, disabled_tint, scale_alpha, with_alpha};
use crate::tokens::motion::{SPRING_PRESS, SPRING_SWAP};
use crate::tokens::{BEUI_LIGHT, BeuiPalette};

/// Padding inside the composer's frame, in logical px (`p-2`).
pub const PROMPT_PADDING: f64 = style::SPACING_UNIT * 2.0;

/// Horizontal padding inside the field, in logical px (`px-2`).
pub const PROMPT_FIELD_PADDING_X: f64 = style::SPACING_UNIT * 2.0;

/// Vertical padding inside the field, in logical px (`pt-1.5`).
pub const PROMPT_FIELD_PADDING_Y: f64 = style::SPACING_UNIT * 1.5;

/// The gap between the field and the action row, in logical px (`mt-1`).
pub const PROMPT_ACTION_GAP: f64 = style::SPACING_UNIT;

/// The action row's height, in logical px (`min-h-8`).
pub const PROMPT_ACTION_ROW_HEIGHT: f64 = style::HEIGHT_SM;

/// The send button's edge, in logical px (`size-8`).
pub const PROMPT_BUTTON_EDGE: f64 = style::HEIGHT_SM;

/// How many lines the field shows before it grows (`minRows = 2`).
pub const PROMPT_MIN_ROWS: usize = 2;

/// How many lines the field grows to before it scrolls (`maxRows = 8`).
pub const PROMPT_MAX_ROWS: usize = 8;

/// The placeholder upstream ships — `"Ask the agent to do something…"`.
pub const PROMPT_PLACEHOLDER: &str = "Ask the agent to do something…";

/// The send button's accessible name while it sends (`"Send prompt"`).
pub const PROMPT_SEND_LABEL: &str = "Send prompt";

/// The send button's accessible name while it stops (`"Stop generating"`).
pub const PROMPT_STOP_LABEL: &str = "Stop generating";

/// The composer's own accessible name (`aria-label = "Prompt"`).
pub const PROMPT_LABEL: &str = "Prompt";

/// Alpha of the composer's resting hairline (`border-border/80`).
const FRAME_BORDER_ALPHA: f32 = 0.8;

/// Alpha of the composer's focused hairline (`focus-within:border-foreground/25`).
const FRAME_FOCUS_ALPHA: f32 = 0.25;

/// The send arrow's icon edge, in logical px (`size-4`).
const SEND_ICON_SIZE: f64 = style::ICON_SIZE;

/// The stop square's edge, in logical px (`size-3`).
const STOP_ICON_SIZE: f64 = 12.0;

/// How far the swapping icon travels, in logical px (`y: ±3`).
const ICON_SWAP_RISE: f64 = 3.0;

/// The swapping icon's scale floor (`scale: 0.8`).
const ICON_SWAP_SCALE: f64 = 0.8;

/// Lucide's icon viewBox edge — `arrow-up` is authored on a 24-unit grid.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// Lucide's stroke width, on that same grid.
const LUCIDE_STROKE: f64 = 2.0;

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// What the composer's trailing button does right now — derived from the
/// value, `disabled` and `loading`, never set directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptInputSend {
    /// Nothing to send: an empty (or whitespace-only) prompt, or a disabled
    /// composer. The button is inert and dimmed.
    #[default]
    Disabled,
    /// There is a prompt to send; pressing submits it.
    Ready,
    /// A response is streaming; the button is a stop square and pressing it
    /// calls [`PromptInputView::on_stop`].
    Streaming,
}

impl PromptInputSend {
    /// Every state, in the order the button walks them.
    pub const ALL: [PromptInputSend; 3] = [
        PromptInputSend::Disabled,
        PromptInputSend::Ready,
        PromptInputSend::Streaming,
    ];

    /// Upstream's own derivation: `loading` wins, then
    /// `canSubmit = Boolean(value.trim()) && !disabled && !loading`.
    ///
    /// A `loading` composer with no stop handler is still
    /// [`Streaming`](Self::Streaming) *visually* — upstream keeps the square and
    /// only disables the press (`disabled={loading ? !onStop : !canSubmit}`) —
    /// so pressability is asked separately through
    /// [`is_pressable`](Self::is_pressable).
    pub fn resolve(value: &str, disabled: bool, loading: bool) -> Self {
        if loading {
            return PromptInputSend::Streaming;
        }
        if disabled || value.trim().is_empty() {
            return PromptInputSend::Disabled;
        }
        PromptInputSend::Ready
    }

    /// Whether a press does anything, given whether a stop handler exists.
    pub const fn is_pressable(self, has_stop: bool) -> bool {
        match self {
            PromptInputSend::Disabled => false,
            PromptInputSend::Ready => true,
            PromptInputSend::Streaming => has_stop,
        }
    }

    /// Whether the button shows the stop square rather than the send arrow.
    pub const fn shows_stop(self) -> bool {
        matches!(self, PromptInputSend::Streaming)
    }

    /// The accessible name upstream gives the button in this state.
    pub const fn label(self) -> &'static str {
        match self {
            PromptInputSend::Streaming => PROMPT_STOP_LABEL,
            _ => PROMPT_SEND_LABEL,
        }
    }
}

/// A view-held, typed text callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held, typed no-argument callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI prompt composer. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::agents::prompt_input::prompt_input;
///
/// #[derive(Default)]
/// struct Chat {
///     draft: String,
///     streaming: bool,
/// }
///
/// let composer = prompt_input(
///     Chat::default().draft,
///     |state: &mut Chat, text| state.draft = text,
/// )
/// .on_submit(|state: &mut Chat, _prompt| state.streaming = true)
/// .on_stop(|state: &mut Chat| state.streaming = false);
/// ```
pub struct PromptInputView<State: 'static> {
    value: String,
    placeholder: String,
    disabled: bool,
    loading: bool,
    min_rows: usize,
    max_rows: usize,
    on_change: OnText<State>,
    on_submit: Option<OnText<State>>,
    on_stop: Option<OnAction<State>>,
}

/// Create a controlled composer showing `value`, reporting each edit through
/// `on_change(state, new_text)`.
///
/// Controlled exactly like the baseline field it wraps: the widget never owns
/// the durable value, and an app that rejects or transforms the requested text
/// sees its own value win on the next rebuild.
pub fn prompt_input<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> PromptInputView<State> {
    PromptInputView {
        value: value.into(),
        placeholder: PROMPT_PLACEHOLDER.to_owned(),
        disabled: false,
        loading: false,
        min_rows: PROMPT_MIN_ROWS,
        max_rows: PROMPT_MAX_ROWS,
        on_change: Rc::new(on_change),
        on_submit: None,
        on_stop: None,
    }
}

impl<State: 'static> PromptInputView<State> {
    /// Replace the placeholder (default [`PROMPT_PLACEHOLDER`]).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Fired with the trimmed prompt when the send button is pressed or Enter
    /// submits.
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Fired when the stop square is pressed. Without one, a `loading`
    /// composer still shows the square but does not act on a press —
    /// upstream's `disabled={loading ? !onStop : !canSubmit}`.
    pub fn on_stop<F: Fn(&mut State) + 'static>(mut self, on_stop: F) -> Self {
        self.on_stop = Some(Rc::new(on_stop));
        self
    }

    /// Whether a response is streaming (`loading`): the button becomes a stop
    /// square and the field goes inert.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Disable the composer: the field refuses focus and the whole frame dims.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// How many lines the field shows before it grows (`minRows`), floored at
    /// one.
    pub fn min_rows(mut self, rows: usize) -> Self {
        self.min_rows = rows.max(1);
        self
    }

    /// How many lines the field grows to before it scrolls (`maxRows`),
    /// floored at one.
    pub fn max_rows(mut self, rows: usize) -> Self {
        self.max_rows = rows.max(1);
        self
    }

    /// The send state this composer resolves to.
    pub fn send_state(&self) -> PromptInputSend {
        PromptInputSend::resolve(&self.value, self.disabled, self.loading)
    }

    /// The wrapped baseline field, configured with its own chrome suppressed
    /// and the Enter contract set.
    // erasure: keep feeds build_child/rebuild_child/teardown_child, which take &AnyView
    fn field(&self) -> AnyView<State> {
        let on_change = self.on_change.clone();
        // The prompt and placeholder keep the baseline `text_input`'s own
        // family: that field has no themed-family seam.
        let mut field = text_input(self.value.clone(), move |state: &mut State, text| {
            on_change(state, text)
        })
        .placeholder(self.placeholder.clone())
        // A streaming composer is inert, exactly like a disabled one — the
        // action row's stop square is the only live control.
        .enabled(!self.disabled && !self.loading)
        .multiline(self.max_rows)
        // Enter sends, Shift+Enter inserts a newline: upstream's `handleKeyDown`
        // and the baseline field's own documented pairing.
        .submit_on_enter(true)
        .padding(PROMPT_FIELD_PADDING_X, PROMPT_FIELD_PADDING_Y)
        .border_width(0.0)
        .corner_radius(style::RADIUS_LG);
        if let Some(on_submit) = self.on_submit.clone() {
            field = field.on_submit(move |state: &mut State, text| {
                let prompt = text.trim();
                // Upstream's `submit` guard: an empty prompt never fires.
                if !prompt.is_empty() {
                    on_submit(state, prompt.to_owned());
                }
            });
        }
        any(field)
    }
}

/// The colours one composer resolves.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PromptPaint {
    /// The resting hairline (`border-border/80`).
    border: Color,
    /// The focused hairline (`border-foreground/25`).
    focused_border: Color,
    /// The send button's fill (`bg-primary`, beUI's ink).
    button: Color,
    /// The send icon's ink (`text-primary-foreground`).
    icon: Color,
}

impl PromptPaint {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        let (border, ink, on_ink) = match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                (scheme.outline_variant, scheme.primary, scheme.on_primary)
            }
            None => (
                FALLBACK.border,
                FALLBACK.primary,
                FALLBACK.primary_foreground,
            ),
        };
        PromptPaint {
            border: scale_alpha(border, FRAME_BORDER_ALPHA),
            focused_border: with_alpha(ink, FRAME_FOCUS_ALPHA),
            button: ink,
            icon: on_ink,
        }
    }
}

/// Lucide's `arrow-up` on a 24-unit grid, scaled to `size`: the shaft plus the
/// two head strokes.
fn arrow_up_path(size: f64) -> BezPath {
    let unit = size / LUCIDE_VIEWBOX;
    let at = |x: f64, y: f64| Point::new(x * unit, y * unit);
    let mut path = BezPath::new();
    // `m5 12 7-7 7 7` — the head.
    path.move_to(at(5.0, 12.0));
    path.line_to(at(12.0, 5.0));
    path.line_to(at(19.0, 12.0));
    // `M12 19V5` — the shaft.
    path.move_to(at(12.0, 19.0));
    path.line_to(at(12.0, 5.0));
    path
}

/// The retained widget for a [`PromptInputView`].
pub struct PromptInputWidget {
    field: ChildPod,
    disabled: bool,
    loading: bool,
    send: PromptInputSend,
    has_stop: bool,
    min_rows: usize,
    max_rows: usize,

    /// The frame's focus crossfade, `0.0` idle .. `1.0` focused.
    focus: Lane,
    /// Whether the wrapped field held focus on the last paint (focus is only
    /// observable there).
    was_focused: bool,
    /// The button's press shrink, `0.0` at rest .. `1.0` pressed.
    press: Lane,
    /// The send → stop icon swap, `Absent` while sending, `Present` while
    /// streaming.
    swap: Presence,
    /// What the swap's last `advance` computed — the value paint staged the
    /// icon pair from, exposed for a caller (and a test) that wants it without
    /// a clock.
    swap_value: f64,
    /// Whether the swap has been staged onto the presence yet.
    staged: bool,

    button_hovered: bool,
    button_pressed: bool,
    button_captured: bool,

    /// The field row's box, resolved by layout.
    field_box: Rect,
    /// The button's box, resolved by layout.
    button_box: Rect,
    /// The whole frame's box, resolved by layout.
    frame: Size,

    on_submit_value: String,
    on_submit: Option<ErasedArgCallback<String>>,
    on_stop: Option<ErasedCallback>,
}

impl PromptInputWidget {
    /// What the trailing button does right now.
    pub fn send_state(&self) -> PromptInputSend {
        self.send
    }

    /// Whether a press on the button does anything.
    pub fn is_pressable(&self) -> bool {
        self.send.is_pressable(self.has_stop)
    }

    /// How far through the send → stop swap the button is, as of the last
    /// paint: `0.0` fully the arrow, `1.0` fully the square.
    pub fn swap_progress(&self) -> f64 {
        self.swap_value
    }

    /// The phase the icon swap is in.
    pub fn swap_phase(&self) -> crate::motion::PresencePhase {
        self.swap.phase()
    }

    /// The button's box inside the composer, as of the last layout.
    pub fn button_box(&self) -> Rect {
        self.button_box
    }

    /// The field row's box inside the composer, as of the last layout.
    pub fn field_box(&self) -> Rect {
        self.field_box
    }

    /// Whether `position` (widget-local) lands on a button that is currently
    /// interactive — an inert button takes no pointer, matching upstream's
    /// `disabled` attribute.
    fn hits_button(&self, position: Point) -> bool {
        self.is_pressable()
            && inside(
                position - self.button_box.origin().to_vec2(),
                self.button_box.size(),
            )
    }

    /// Fire whichever handler this state owns.
    fn activate(&mut self, ctx: &mut EventCtx) {
        match self.send {
            PromptInputSend::Ready => {
                let prompt = self.on_submit_value.trim().to_owned();
                if !prompt.is_empty()
                    && let Some(cb) = self.on_submit.as_mut()
                {
                    cb(ctx, prompt);
                }
            }
            PromptInputSend::Streaming => {
                if let Some(cb) = self.on_stop.as_mut() {
                    cb(ctx);
                }
            }
            PromptInputSend::Disabled => {}
        }
    }

    /// Paint the button's icon pair at `centre`, staged by the swap.
    fn paint_icons(&self, scene: &mut dyn PaintScene, centre: Point, ink: Color, swap: f64) {
        // The arrow leaves upward as the square arrives from below —
        // upstream's `{ opacity, y: ±3, scale: 0.8 }` pair.
        let send_alpha = (1.0 - swap).clamp(0.0, 1.0) as f32;
        if send_alpha > 0.0 {
            let scale = ICON_SWAP_SCALE + (1.0 - ICON_SWAP_SCALE) * (1.0 - swap);
            let rise = -ICON_SWAP_RISE * swap;
            let at = Point::new(
                centre.x - SEND_ICON_SIZE / 2.0,
                centre.y - SEND_ICON_SIZE / 2.0 + rise,
            );
            scene.push_transform(icon_transform(centre, scale));
            scene.stroke_path(
                at,
                &arrow_up_path(SEND_ICON_SIZE),
                LUCIDE_STROKE * SEND_ICON_SIZE / LUCIDE_VIEWBOX,
                &Brush::Solid(scale_alpha(ink, send_alpha)),
            );
            scene.pop_transform();
        }
        let stop_alpha = swap.clamp(0.0, 1.0) as f32;
        if stop_alpha > 0.0 {
            let scale = ICON_SWAP_SCALE + (1.0 - ICON_SWAP_SCALE) * swap;
            let rise = ICON_SWAP_RISE * (1.0 - swap);
            scene.push_transform(icon_transform(centre, scale));
            scene.fill_rounded_rect(
                Point::new(
                    centre.x - STOP_ICON_SIZE / 2.0,
                    centre.y - STOP_ICON_SIZE / 2.0 + rise,
                ),
                Size::new(STOP_ICON_SIZE, STOP_ICON_SIZE),
                style::RADIUS_SM / 2.0,
                scale_alpha(ink, stop_alpha),
            );
            scene.pop_transform();
        }
    }
}

/// A scale about `centre` — the transform an icon's `scale: 0.8` resolves to.
fn icon_transform(centre: Point, scale: f64) -> Affine {
    Affine::translate((centre.x, centre.y))
        * Affine::scale(scale)
        * Affine::translate((-centre.x, -centre.y))
}

impl<State: 'static> View<State> for PromptInputView<State> {
    type Element = PromptInputWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PromptInputWidget {
        let send = self.send_state();
        PromptInputWidget {
            field: build_child(&self.field(), ctx),
            disabled: self.disabled,
            loading: self.loading,
            send,
            has_stop: self.on_stop.is_some(),
            min_rows: self.min_rows,
            max_rows: self.max_rows,
            focus: Lane::at_rest(TRACK_COLOR_RAMP, 0.0),
            was_focused: false,
            press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            swap: Presence::symmetric(Ramp::spring(SPRING_SWAP)),
            swap_value: if send.shows_stop() { 1.0 } else { 0.0 },
            staged: false,
            button_hovered: false,
            button_pressed: false,
            button_captured: false,
            field_box: Rect::ZERO,
            button_box: Rect::ZERO,
            frame: Size::ZERO,
            on_submit_value: self.value.clone(),
            on_submit: self.on_submit.as_ref().map(erase_callback_arg),
            on_stop: self.on_stop.as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PromptInputWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapters.
        element.on_submit = self.on_submit.as_ref().map(erase_callback_arg);
        element.on_stop = self.on_stop.as_ref().map(erase_callback);
        element.has_stop = self.on_stop.is_some();
        element.on_submit_value = self.value.clone();

        let mut flags = rebuild_child(&prev.field(), &self.field(), &mut element.field, ctx);
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        if element.loading != self.loading {
            element.loading = self.loading;
            flags |= ChangeFlags::PAINT;
        }
        if element.min_rows != self.min_rows || element.max_rows != self.max_rows {
            element.min_rows = self.min_rows;
            element.max_rows = self.max_rows;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let send = self.send_state();
        if element.send != send {
            element.send = send;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut PromptInputWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.field(), &mut element.field, ctx);
    }
}

impl Widget for PromptInputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            style::HEIGHT_INPUT * 8.0
        };
        let inner_width = (width - PROMPT_PADDING * 2.0).max(0.0);

        // `rows={minRows}`: the field never shrinks below its own minimum, and
        // the baseline field caps its own growth at `max_rows`.
        let min_height =
            self.min_rows as f64 * style::LINE_HEIGHT_INPUT + PROMPT_FIELD_PADDING_Y * 2.0;
        let max_height = self.max_rows.max(self.min_rows) as f64 * style::LINE_HEIGHT_INPUT
            + PROMPT_FIELD_PADDING_Y * 2.0;
        let field = self.field.layout_child(
            ctx,
            &BoxConstraints::new(
                Size::new(inner_width, min_height),
                Size::new(inner_width, max_height),
            ),
        );
        self.field
            .set_origin(Point::new(PROMPT_PADDING, PROMPT_PADDING));
        self.field_box = Rect::from_origin_size(
            Point::new(PROMPT_PADDING, PROMPT_PADDING),
            Size::new(inner_width, field.height),
        );

        // `ml-auto`: the button sits at the action row's trailing edge.
        let row_top = self.field_box.max_y() + PROMPT_ACTION_GAP;
        self.button_box = Rect::from_origin_size(
            Point::new(
                width - PROMPT_PADDING - PROMPT_BUTTON_EDGE,
                row_top + (PROMPT_ACTION_ROW_HEIGHT - PROMPT_BUTTON_EDGE) / 2.0,
            ),
            Size::new(PROMPT_BUTTON_EDGE, PROMPT_BUTTON_EDGE),
        );

        self.frame = Size::new(width, row_top + PROMPT_ACTION_ROW_HEIGHT + PROMPT_PADDING);
        bc.constrain(self.frame)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The pod's focus path is authoritative; an inert composer never reads
        // focused.
        let inert = self.disabled || self.loading;
        let focused = ctx.has_focus() && !inert;
        if focused != self.was_focused {
            self.was_focused = focused;
            self.focus.retarget(if focused { 1.0 } else { 0.0 });
        }
        if !ctx.is_hovered() {
            self.button_hovered = false;
        }

        let (reduce_motion, paint) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                PromptPaint::resolve(theme),
            )
        };

        // The icon swap follows the resolved state; staged once so a composer
        // built streaming shows the square with no entrance.
        if !self.staged {
            self.staged = true;
            if reduce_motion {
                self.swap = self.swap.collapsed();
            }
            self.swap.set_open(self.send.shows_stop());
        } else {
            self.swap.set_open(self.send.shows_stop());
        }

        let now = ctx.frame_time();
        if reduce_motion {
            self.focus.snap();
            self.press.snap();
        }
        let mut owes_frame = self.focus.advance(now);
        owes_frame |= self.press.advance(now);
        let swap = self.swap.advance(now);
        self.swap_value = swap;
        owes_frame |= self.swap.is_animating();

        let origin = ctx.origin();
        let focus = self.focus.value().clamp(0.0, 1.0);
        let border = disabled_tint(
            lerp_color(paint.border, paint.focused_border, focus),
            self.disabled,
            style::DISABLED_OPACITY_INPUT,
        );

        // The field first: its own background fill is opaque, so the frame is
        // stroked over it (the same ordering `input` records).
        self.field.paint_child(ctx, scene);

        let radius = style::resolve_radius(style::RADIUS_2XL, self.frame.width, self.frame.height);
        let inset = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, self.frame).inset(-inset),
            (radius - inset).max(0.0),
        );
        scene.stroke_path(
            origin,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(border),
        );

        // The button: dimmed while inert, shrunk while pressed, hover-washed.
        let pressable = self.is_pressable();
        let mut fill = paint.button;
        if self.button_hovered && pressable {
            fill = scale_alpha(fill, style::HOVER_SOLID_ALPHA);
        }
        if !pressable {
            fill = scale_alpha(fill, style::DISABLED_OPACITY);
        }
        let button_at = Point::new(origin.x + self.button_box.x0, origin.y + self.button_box.y0);
        let centre = Point::new(
            button_at.x + PROMPT_BUTTON_EDGE / 2.0,
            button_at.y + PROMPT_BUTTON_EDGE / 2.0,
        );
        let press_scale = crate::press::press_scale(style::PRESS_SCALE, self.press.value());
        scene.push_transform(icon_transform(centre, press_scale));
        scene.fill_rounded_rect(
            button_at,
            Size::new(PROMPT_BUTTON_EDGE, PROMPT_BUTTON_EDGE),
            PROMPT_BUTTON_EDGE / 2.0,
            fill,
        );
        let icon_ink = if pressable {
            paint.icon
        } else {
            scale_alpha(paint.icon, style::DISABLED_OPACITY)
        };
        self.paint_icons(scene, centre, icon_ink, swap);
        scene.pop_transform();

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            let position = p.position;
            match p.phase {
                PointerPhase::Down if self.hits_button(position) => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    self.button_pressed = true;
                    self.button_captured = true;
                    self.press.retarget(1.0);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Move if self.button_captured => {
                    let over = self.hits_button(position);
                    if over {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.button_pressed != over {
                        self.button_pressed = over;
                        self.press.retarget(if over { 1.0 } else { 0.0 });
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    let over = self.hits_button(position);
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.button_hovered != over {
                        self.button_hovered = over;
                        ctx.request_redraw();
                    }
                }
                PointerPhase::Up if self.button_captured => {
                    let fired = self.button_pressed && self.hits_button(position);
                    self.button_pressed = false;
                    self.button_captured = false;
                    self.press.retarget(0.0);
                    if fired {
                        self.activate(ctx);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                PointerPhase::Cancel if self.button_captured => {
                    self.button_pressed = false;
                    self.button_captured = false;
                    self.press.retarget(0.0);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                _ => {}
            }
        }

        // Space/Enter on the button itself is *not* claimed here: the wrapped
        // field owns the keyboard while it holds focus, and Enter already
        // submits through it. A key that reaches this widget with no field
        // focus activates the button, which is what a keyboard walk expects.
        if let InputEvent::Key(key) = event
            && !ctx.has_focus()
            && self.is_pressable()
            && is_activation_key(key)
        {
            self.activate(ctx);
            return EventResult::Handled;
        }

        route_event_single(&mut self.field, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.send.label();
        let pressable = self.is_pressable();
        ctx.push_container(
            Role::Group,
            |node| node.set_label(PROMPT_LABEL),
            |ctx| {
                // The wrapped field contributes the `TextInput` node.
                self.field.semantics_child(ctx);
                ctx.push_node(Role::Button, |node| {
                    node.set_label(label);
                    if pressable {
                        node.add_action(Action::Click);
                    } else {
                        node.set_disabled();
                    }
                });
            },
        );
    }

    visit_children!(field);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    /// The box every composer test lays itself into.
    const BOX: Size = Size::new(360.0, 240.0);

    /// Records the rounded-rect fills, stroked paths and transforms a composer
    /// paints.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        strokes: Vec<Brush>,
        transforms: usize,
    }

    impl Recorder {
        /// The button's own fill — the only `PROMPT_BUTTON_EDGE`-square
        /// rounded rect a composer paints.
        fn button_fill(&self) -> Option<Color> {
            self.rounded
                .iter()
                .find(|(_, size, _)| (size.width - PROMPT_BUTTON_EDGE).abs() < 1e-6)
                .map(|(_, _, color)| *color)
        }

        /// The stop square's fill, when one was painted.
        fn stop_fill(&self) -> Option<Color> {
            self.rounded
                .iter()
                .find(|(_, size, _)| (size.width - STOP_ICON_SIZE).abs() < 1e-6)
                .map(|(_, _, color)| *color)
        }
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, brush: &Brush) {
            self.strokes.push(brush.clone());
        }
        fn push_transform(&mut self, _transform: Affine) {
            self.transforms += 1;
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {}
    }

    /// A composer over a plain string draft.
    fn view(value: &str) -> PromptInputView<String> {
        prompt_input(value, |state: &mut String, text| *state = text)
    }

    fn laid_out(view: &PromptInputView<String>) -> PromptInputWidget {
        let mut next_id = 0u64;
        let mut widget = View::<String>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut PromptInputWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
    }

    fn painted(widget: &mut PromptInputWidget, ms: u64, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn rebuild(widget: &mut PromptInputWidget, next: &PromptInputView<String>) {
        let mut next_id = 0u64;
        View::<String>::rebuild(next, next, widget, &mut BuildCtx::new(&mut next_id));
        relayout(widget);
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    fn dispatch(
        widget: &mut PromptInputWidget,
        state: &mut dyn Any,
        event: &InputEvent,
    ) -> EventResult {
        let mut ctx = EventCtx::new(state, Point::ZERO, BOX);
        widget.event(&mut ctx, event)
    }

    /// Upstream's `canSubmit` derivation, transcribed: `loading` wins, then an
    /// empty-after-trim prompt or a disabled composer is inert.
    #[test]
    fn the_send_state_is_derived_exactly_as_upstream_derives_it() {
        assert_eq!(PromptInputSend::ALL.len(), 3);
        assert_eq!(
            PromptInputSend::resolve("", false, false),
            PromptInputSend::Disabled
        );
        assert_eq!(
            PromptInputSend::resolve("   \n\t ", false, false),
            PromptInputSend::Disabled,
            "whitespace is not a prompt"
        );
        assert_eq!(
            PromptInputSend::resolve("hi", false, false),
            PromptInputSend::Ready
        );
        assert_eq!(
            PromptInputSend::resolve("hi", true, false),
            PromptInputSend::Disabled
        );
        assert_eq!(
            PromptInputSend::resolve("hi", false, true),
            PromptInputSend::Streaming,
            "`loading` wins over everything"
        );
        assert_eq!(
            PromptInputSend::resolve("", true, true),
            PromptInputSend::Streaming
        );
    }

    /// `disabled={loading ? !onStop : !canSubmit}`: a streaming composer with
    /// no stop handler still shows the square but does not act.
    #[test]
    fn pressability_follows_upstreams_disabled_expression() {
        assert!(!PromptInputSend::Disabled.is_pressable(true));
        assert!(PromptInputSend::Ready.is_pressable(false));
        assert!(PromptInputSend::Streaming.is_pressable(true));
        assert!(!PromptInputSend::Streaming.is_pressable(false));
        assert!(PromptInputSend::Streaming.shows_stop());
        assert!(!PromptInputSend::Ready.shows_stop());
        assert_eq!(PromptInputSend::Streaming.label(), PROMPT_STOP_LABEL);
        assert_eq!(PromptInputSend::Ready.label(), PROMPT_SEND_LABEL);
    }

    /// The state morphs as the composer's inputs change, and the swap runs
    /// through [`Presence`] rather than snapping between icons.
    #[test]
    fn the_send_button_morphs_between_its_states() {
        let mut widget = laid_out(&view(""));
        assert_eq!(widget.send_state(), PromptInputSend::Disabled);
        let (rec, _) = painted(&mut widget, 0, None);
        assert!(rec.stop_fill().is_none(), "the arrow, not the square");
        assert!(!rec.strokes.is_empty(), "the arrow is a stroked path");

        rebuild(&mut widget, &view("hello"));
        assert_eq!(widget.send_state(), PromptInputSend::Ready);
        painted(&mut widget, 10, None);
        assert!(widget.is_pressable());

        // Streaming: the swap starts, runs, and settles on the square.
        rebuild(
            &mut widget,
            &view("hello").loading(true).on_stop(|_: &mut String| {}),
        );
        assert_eq!(widget.send_state(), PromptInputSend::Streaming);
        let (_, needs_frame) = painted(&mut widget, 20, None);
        assert!(needs_frame, "the morph owes frames");
        assert_eq!(widget.swap_phase(), crate::motion::PresencePhase::Entering);

        let (rec, needs_frame) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame, "a settled morph stops asking");
        assert_eq!(widget.swap_phase(), crate::motion::PresencePhase::Present);
        assert!(rec.stop_fill().is_some(), "the square is up");
    }

    /// A press on a pressable button fires its handler; a press on an inert
    /// one does nothing at all.
    #[test]
    fn a_press_fires_only_when_the_button_is_pressable() {
        use std::cell::Cell;
        use std::rc::Rc as StdRc;

        let submitted = StdRc::new(Cell::new(0));
        let seen = submitted.clone();
        let composer = prompt_input("hello", |state: &mut String, text| *state = text)
            .on_submit(move |_: &mut String, _prompt| seen.set(seen.get() + 1));
        let mut widget = laid_out(&composer);
        painted(&mut widget, 0, None);

        let centre = widget.button_box().center();
        let press = |phase| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: centre,
                button: PointerButton::Primary,
            })
        };
        let mut state = String::from("hello");
        dispatch(&mut widget, &mut state, &press(PointerPhase::Down));
        dispatch(&mut widget, &mut state, &press(PointerPhase::Up));
        assert_eq!(submitted.get(), 1, "the prompt was sent");

        // Inert: an empty prompt takes no pointer at all.
        let empty = view("");
        let mut widget = laid_out(&empty);
        painted(&mut widget, 0, None);
        assert!(!widget.is_pressable());
        assert!(!widget.hits_button(widget.button_box().center()));
    }

    /// A streaming press calls the stop handler rather than the submit one.
    #[test]
    fn a_streaming_press_stops_rather_than_submits() {
        use std::cell::Cell;
        use std::rc::Rc as StdRc;

        let stopped = StdRc::new(Cell::new(0));
        let submitted = StdRc::new(Cell::new(0));
        let (s, t) = (stopped.clone(), submitted.clone());
        let composer = prompt_input("hello", |state: &mut String, text| *state = text)
            .loading(true)
            .on_submit(move |_: &mut String, _p| t.set(t.get() + 1))
            .on_stop(move |_: &mut String| s.set(s.get() + 1));
        let mut widget = laid_out(&composer);
        painted(&mut widget, 0, None);

        let centre = widget.button_box().center();
        let mut state = String::from("hello");
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            dispatch(
                &mut widget,
                &mut state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: centre,
                    button: PointerButton::Primary,
                }),
            );
        }
        assert_eq!(stopped.get(), 1);
        assert_eq!(submitted.get(), 0);
    }

    /// The field reserves `minRows` lines and never grows past `maxRows`, and
    /// the action row sits below it with the button on its trailing edge.
    #[test]
    fn the_composer_reserves_min_rows_and_puts_the_button_on_the_trailing_edge() {
        let widget = laid_out(&view(""));
        let field = widget.field_box();
        assert!(
            field.height() >= PROMPT_MIN_ROWS as f64 * style::LINE_HEIGHT_INPUT,
            "two rows reserved: {}",
            field.height()
        );
        let button = widget.button_box();
        assert!(button.y0 >= field.max_y() + PROMPT_ACTION_GAP - 1e-6);
        assert!(
            (button.max_x() - (BOX.width - PROMPT_PADDING)).abs() < 1e-6,
            "`ml-auto`"
        );
        assert_eq!(button.width(), PROMPT_BUTTON_EDGE);

        // A one-row composer is shorter than a four-row one.
        let tall = laid_out(&view("").min_rows(4));
        assert!(tall.field_box().height() > field.height());
    }

    /// A press shrinks the button and releasing returns it — the `whileTap`
    /// scale, on a lane.
    #[test]
    fn the_button_shrinks_while_pressed() {
        let mut widget = laid_out(&view("hello"));
        painted(&mut widget, 0, None);
        let centre = widget.button_box().center();
        let mut state = String::from("hello");
        dispatch(
            &mut widget,
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: centre,
                button: PointerButton::Primary,
            }),
        );
        let (_, needs_frame) = painted(&mut widget, 10, None);
        assert!(needs_frame, "the press shrink owes frames");
        painted(&mut widget, 40, None);
        assert!(widget.press.value() > 0.0);
    }

    /// `reduce_motion` collapses the focus crossfade, the press shrink and the
    /// icon morph: a streaming composer paints its square immediately.
    #[test]
    fn reduce_motion_collapses_every_lane() {
        let theme = reduced();
        let mut widget = laid_out(&view("hello").loading(true).on_stop(|_: &mut String| {}));
        let (rec, needs_frame) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame, "nothing is animating");
        assert!(rec.stop_fill().is_some(), "the square is already up");
    }

    /// An inert button is dimmed rather than hidden, so the composer's shape
    /// does not move as the prompt is typed.
    #[test]
    fn an_inert_button_is_dimmed_not_hidden() {
        let mut inert = laid_out(&view(""));
        let (rec, _) = painted(&mut inert, 0, None);
        let dim = rec.button_fill().expect("the button is painted");

        let mut ready = laid_out(&view("hello"));
        let (rec, _) = painted(&mut ready, 0, None);
        let full = rec.button_fill().expect("the button is painted");
        assert!(
            dim.components[3] < full.components[3],
            "{dim:?} should be dimmer than {full:?}"
        );
        assert_eq!(inert.button_box(), ready.button_box(), "same geometry");
    }

    /// A redundant rebuild leaves the resolved state alone; a value change
    /// that crosses the empty boundary repaints.
    #[test]
    fn a_rebuild_tracks_the_derived_state() {
        let empty = view("");
        let mut widget = laid_out(&empty);
        let mut next_id = 0u64;
        let flags = View::<String>::rebuild(
            &empty,
            &empty,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert!(!flags.contains(ChangeFlags::LAYOUT));
        assert_eq!(widget.send_state(), PromptInputSend::Disabled);

        let filled = view("hi");
        let flags = View::<String>::rebuild(
            &filled,
            &empty,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert!(flags.contains(ChangeFlags::PAINT));
        assert_eq!(widget.send_state(), PromptInputSend::Ready);
    }

    /// The composer publishes the wrapped field's node plus its own button.
    #[test]
    fn semantics_publishes_the_field_and_the_button() {
        let widget = laid_out(&view("hello"));
        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 1, "one wrapped field");
        assert_eq!(widget.send_state().label(), PROMPT_SEND_LABEL);
    }
}
