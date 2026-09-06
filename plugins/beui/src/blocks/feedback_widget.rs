//! Ports beUI's `feedback-widget` composed block — the corner affordance that
//! morphs open into a feedback panel and back, with its own sending, thanks and
//! failure views.
//!
//! Source: `components/motion/feedback-widget.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `feedback-widget` (blocks): *"Corner trigger that morphs open into a
//! feedback popup with message entry and animated sending, success and retry
//! states."*
//!
//! | class / prop | here |
//! |---|---|
//! | shell closed `h-12 w-12 p-0`, `borderRadius: 40` | [`TRIGGER_SIZE`] / [`TRIGGER_RADIUS`] |
//! | shell open `w-[min(86vw,320px)] p-2`, `borderRadius: 20` | [`PANEL_MAX_WIDTH`] / [`PANEL_PADDING`] / [`PANEL_RADIUS`] |
//! | shell `border border-border bg-background shadow-lg` | `outline_variant` over `surface` |
//! | open morph `duration: 0.4, ease: [0.34, 1.25, 0.64, 1]` | [`MORPH_OPEN_MS`] / [`MORPH_OPEN_EASE`] |
//! | close morph `duration: 0.28, ease: [0.22, 1, 0.36, 1]` | [`MORPH_CLOSE_MS`] / [`MORPH_CLOSE_EASE`] |
//! | content `x: ±40, scale: 0.97`, fade `duration: 0.22` | [`MORPH_SLIDE`] / [`MORPH_SCALE`] / [`MORPH_FADE_MS`] |
//! | card `rounded-[20px] bg-border/60` | [`PANEL_RADIUS`], `outline_variant` at [`CARD_WASH_ALPHA`] |
//! | form card `min-h-[150px] px-4 py-3.5` | [`FORM_MIN_HEIGHT`] / [`CARD_PADDING_X`] / [`FORM_PADDING_Y`] |
//! | title `text-sm font-semibold` | [`style::TEXT_SM`], `FontWeight::SEMI_BOLD` |
//! | close `h-5 w-5 rounded-full bg-foreground/[0.07]`, `X h-3 w-3` | [`CLOSE_BOX`] / [`CLOSE_WASH_ALPHA`] / [`CLOSE_ICON`] |
//! | textarea `mt-2 rows={3} bg-transparent` | the wrapped baseline field, [`FIELD_GAP`] below the title |
//! | actions `gap-2 px-1 pt-2 pb-1`, two `flex-1` buttons | [`ACTIONS_GAP`] / [`ACTIONS_PADDING_X`] / [`ACTIONS_PADDING_TOP`] / [`ACTIONS_PADDING_BOTTOM`] |
//! | sent card `px-4 py-6`, circle `h-12 w-12 bg-(--color-success)` | [`SENT_PADDING_Y`] / [`STATUS_CIRCLE`] |
//! | sent check `h-5 w-5 text-white`, `pathLength 0 → 1, 0.35s, delay 0.15` | [`STATUS_ICON`] / [`CHECK_DRAW_MS`] / [`CHECK_DRAW_DELAY_MS`] |
//! | circle `spring stiffness 500 damping 22, delay 0.04` | [`CIRCLE_POP`] / [`CIRCLE_POP_DELAY_MS`] |
//! | sprinkles `8 × 26px, 0.6s, delay 0.18 + i × 0.02` | [`SPRINKLE_COUNT`] / [`SPRINKLE_RADIUS`] / [`SPRINKLE_MS`] / [`SPRINKLE_DELAY_MS`] / [`SPRINKLE_STEP_MS`] |
//! | error circle `bg-destructive/10 text-destructive` | `error_container` / `error` |
//! | view enter `y: 8 → 0, 0.24s`, exit `y: -8, 0.16s` | [`VIEW_RISE`] / [`VIEW_ENTER_MS`] / [`VIEW_EXIT_MS`] |
//! | trigger icon `rotate: 45 → 0` | [`TRIGGER_ICON_ROTATION`] |
//! | trigger `whileTap={{ scale: 0.92 }}` | [`TRIGGER_PRESS_SCALE`] |
//!
//! # Premise correction: there is no rating control
//!
//! Upstream's feedback panel is a **title, a message field and two buttons**.
//! It has no star row, no score, no rating of any kind — `feedback-widget.tsx`'s
//! whole payload is `{ message: string }`. A port that added one would be
//! inventing a control the source does not have, so this one does not; the
//! [`FeedbackWidgetView`] surface is the message and nothing else.
//!
//! # Controlled: the app owns the status
//!
//! Upstream keeps `status` in local state and drives it from an `async`
//! `onSubmit`, closing itself on a `setTimeout` [`SUCCESS_LINGER_MS`] after a
//! successful send. A frust widget owns neither an async runtime nor a timer
//! (`docs/CODE_STANDARDS.md`'s no-`Instant::now()` rule and the framework's
//! task-routing conventions), so the whole machine is a prop: the widget reports
//! [`FeedbackWidgetView::on_status_change`] with the status a press implies, and
//! the app's own task decides when `Sending` becomes `Sent` or `Error` and when
//! the linger expires. [`SUCCESS_LINGER_MS`] is exported so an app schedules the
//! same 1.6s upstream does.
//!
//! Dismissal is the app's on the same grounds: upstream binds a window
//! `keydown`/`pointerdown` pair to close on Escape or an outside click, and a
//! widget here sees neither (`docs/CODE_STANDARDS.md`'s focus/hit-test routing —
//! an unfocused, unhit widget is not delivered either event). The block reports
//! its own close affordances; an app that wants light-dismiss hosts the block
//! behind its own barrier.
//!
//! # It reserves its open box and paints inside it
//!
//! Upstream's shell is `position: absolute; bottom: 0` inside a
//! `pointer-events-none` wrapper, so it grows out of the corner without
//! disturbing the page. The frust analogue is: **this widget measures at its
//! open size and paints the morphing shell anchored to the
//! [`FeedbackPosition`] corner of that box**, so the morph is pure paint and no
//! rebuild relayouts the page around it. A press that lands in the reserved box
//! but outside the shell reports `Ignored`, which is what leaves whatever the
//! app stacked underneath still able to take it.
//!
//! # Degradations
//!
//! - **The message field is the framework's own, and paints its own fill.**
//!   Upstream's `<textarea className="bg-transparent">` sits directly on the
//!   tinted card; the wrapped field resolves an opaque `surface` fill and offers
//!   no seam to suppress it, the same degradation
//!   [`input`](crate::components::input) records. It is `multiline(3)`, matching
//!   `rows={3}`.
//! - **Closing does not blur the message field.** A container has no seam to
//!   release a descendant's focus session (`docs/CODE_STANDARDS.md` gates the
//!   orphan mark on a live focus chain a rebuild cannot see), so a panel closed
//!   while the field held focus leaves the session standing until the next press
//!   elsewhere blurs it.
//! - **No blur filters.** `filter: blur(2px)` on the morph and `blur(4px)` on
//!   the view swap have no counterpart in this scene's paint vocabulary; the
//!   slide, scale and fade are ported and the blur is dropped.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::scene::arc_path;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod,
    Color, ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene,
    Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any,
    build_child, erase_callback_arg, rebuild_child, route_event_single, teardown_child,
    visit_children,
};
use frust::{Curve, FrameTime, SpringDescription, Theme, text_input};

use crate::components::button::{ButtonSize, ButtonState, ButtonTone, ButtonVariant, button};
use crate::components::checkbox::{CHECK_POINTS, ICON_VIEWBOX, mark_path};
use crate::motion::{Presence, Ramp, Stagger};
use crate::press::{Lane, inside_inclusive, presses};
use crate::style;
use crate::text::Label;
use crate::tokens::motion::EASE_OUT;
use crate::tokens::{BeuiTokens, sans_family};

// ---- Ported metrics --------------------------------------------------------

/// The closed trigger's edge, in logical px (`h-12 w-12`).
pub const TRIGGER_SIZE: f64 = 48.0;
/// The closed shell's corner radius, in logical px (`borderRadius: 40`).
pub const TRIGGER_RADIUS: f64 = 40.0;
/// The open shell's corner radius, in logical px (`borderRadius: 20`).
pub const PANEL_RADIUS: f64 = 20.0;
/// The open shell's width ceiling, in logical px (`w-[min(86vw,320px)]`).
pub const PANEL_MAX_WIDTH: f64 = 320.0;
/// The fraction of the available width the panel will not exceed (`86vw`).
pub const PANEL_WIDTH_FRACTION: f64 = 0.86;
/// The open shell's inner padding, in logical px (`p-2`).
pub const PANEL_PADDING: f64 = 8.0;

/// The card's wash over `--border` (`bg-border/60`).
pub const CARD_WASH_ALPHA: f32 = 0.6;
/// The card's horizontal padding, in logical px (`px-4`).
pub const CARD_PADDING_X: f64 = 16.0;
/// The form card's vertical padding, in logical px (`py-3.5`).
pub const FORM_PADDING_Y: f64 = 14.0;
/// The form card's minimum height, in logical px (`min-h-[150px]`).
pub const FORM_MIN_HEIGHT: f64 = 150.0;
/// Gap between the title row and the message field, in logical px (`mt-2`).
pub const FIELD_GAP: f64 = 8.0;

/// The close affordance's box, in logical px (`h-5 w-5`).
pub const CLOSE_BOX: f64 = 20.0;
/// The close affordance's glyph edge, in logical px (`h-3 w-3`).
pub const CLOSE_ICON: f64 = 12.0;
/// The close affordance's wash over the ink (`bg-foreground/[0.07]`).
pub const CLOSE_WASH_ALPHA: f32 = 0.07;

/// Gap between the two action buttons, in logical px (`gap-2`).
pub const ACTIONS_GAP: f64 = 8.0;
/// The action row's horizontal padding, in logical px (`px-1`).
pub const ACTIONS_PADDING_X: f64 = 4.0;
/// The action row's top padding, in logical px (`pt-2`).
pub const ACTIONS_PADDING_TOP: f64 = 8.0;
/// The action row's bottom padding, in logical px (`pb-1`).
pub const ACTIONS_PADDING_BOTTOM: f64 = 4.0;

/// The sent card's vertical padding, in logical px (`py-6`).
pub const SENT_PADDING_Y: f64 = 24.0;
/// The error card's vertical padding, in logical px (`py-5`).
pub const ERROR_PADDING_Y: f64 = 20.0;
/// The status circle's edge, in logical px (`h-12 w-12`).
pub const STATUS_CIRCLE: f64 = 48.0;
/// The status glyph's edge inside the circle, in logical px (`h-5 w-5`).
pub const STATUS_ICON: f64 = style::ICON_SIZE_LG;
/// Gap under the status circle, in logical px (`mb-1` on the sent view,
/// `mt-3` on the error view's title).
pub const STATUS_GAP: f64 = 12.0;
/// Gap between a status view's title and its body line, in logical px
/// (`gap-1.5` / `mt-1`).
pub const STATUS_TITLE_GAP: f64 = 6.0;
/// Gap above the error view's retry button, in logical px (`mt-4`).
pub const RETRY_GAP: f64 = 16.0;

/// How long the shell takes to grow open, in milliseconds (`0.4`).
pub const MORPH_OPEN_MS: u64 = 400;
/// How long the shell takes to collapse, in milliseconds (`0.28`).
pub const MORPH_CLOSE_MS: u64 = 280;
/// The open morph's overshooting curve (`[0.34, 1.25, 0.64, 1]`).
pub const MORPH_OPEN_EASE: Curve = Curve::Cubic(0.34, 1.25, 0.64, 1.0);
/// The close morph's calmer curve (`[0.22, 1, 0.36, 1]`).
pub const MORPH_CLOSE_EASE: Curve = Curve::Cubic(0.22, 1.0, 0.36, 1.0);
/// How long the content crossfade takes, in milliseconds (`0.22`).
pub const MORPH_FADE_MS: u64 = 220;
/// How far the content slides in from, in logical px (`MORPH_SLIDE = 40`),
/// signed away from the anchored corner.
pub const MORPH_SLIDE: f64 = 40.0;
/// The scale the content morphs in from (`MORPH_SCALE = 0.97`).
pub const MORPH_SCALE: f64 = 0.97;

/// The scale the trigger shrinks to while pressed (`whileTap: scale 0.92`).
pub const TRIGGER_PRESS_SCALE: f64 = 0.92;
/// How far the trigger's glyph is turned as it arrives, in radians
/// (`rotate: 45 → 0`).
pub const TRIGGER_ICON_ROTATION: f64 = std::f64::consts::FRAC_PI_4;
/// The trigger glyph's edge, in logical px (`h-5 w-5`).
pub const TRIGGER_ICON: f64 = style::ICON_SIZE_LG;

/// How far a panel view rises into place, in logical px (`y: 8 → 0`).
pub const VIEW_RISE: f64 = 8.0;
/// How long a panel view's entrance takes, in milliseconds (`0.24`).
pub const VIEW_ENTER_MS: u64 = 240;
/// How long a panel view's exit takes, in milliseconds (`0.16`).
pub const VIEW_EXIT_MS: u64 = 160;

/// The spring the status circle pops in on (`stiffness: 500, damping: 22`).
pub const CIRCLE_POP: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 500.0,
    damping: 22.0,
};
/// How long the status circle waits before popping, in milliseconds
/// (`delay: 0.04`).
pub const CIRCLE_POP_DELAY_MS: f64 = 40.0;
/// How long the sent check draws itself in, in milliseconds (`duration: 0.35`).
pub const CHECK_DRAW_MS: f64 = 350.0;
/// How long the sent check waits before drawing, in milliseconds
/// (`delay: 0.15`).
pub const CHECK_DRAW_DELAY_MS: f64 = 150.0;

/// How many celebration sprinkles burst from the sent check
/// (`Array.from({ length: 8 })`).
pub const SPRINKLE_COUNT: usize = 8;
/// How far a sprinkle travels, in logical px (`Math.cos(angle) * 26`).
pub const SPRINKLE_RADIUS: f64 = 26.0;
/// A sprinkle's edge, in logical px (`h-1.5 w-1.5`).
pub const SPRINKLE_SIZE: f64 = 6.0;
/// How long one sprinkle's flight takes, in milliseconds (`duration: 0.6`).
pub const SPRINKLE_MS: u64 = 600;
/// How long the burst waits before it starts, in milliseconds (`delay: 0.18`).
pub const SPRINKLE_DELAY_MS: f64 = 180.0;
/// How much later each successive sprinkle leaves, in milliseconds
/// (`i * 0.02`).
pub const SPRINKLE_STEP_MS: u64 = 20;

/// How long upstream leaves the thanks view up before closing itself, in
/// milliseconds (`SUCCESS_DURATION_MS`).
///
/// Exported rather than implemented: the status is the app's (see the
/// [module docs](self)), so the linger is the app's timer too.
pub const SUCCESS_LINGER_MS: u64 = 1_600;

/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dim ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback page fill — the light table's `--background`.
const FALLBACK_BACKGROUND: Color = crate::BEUI_LIGHT.background;
/// Unthemed fallback error hue — the light table's `--destructive`.
const FALLBACK_DESTRUCTIVE: Color = crate::BEUI_LIGHT.destructive;

// ---- Public axes -----------------------------------------------------------

/// The block's lifecycle — upstream's `Status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeedbackStatus {
    /// Closed: only the corner trigger is on screen.
    #[default]
    Idle,
    /// Open on the entry form.
    Open,
    /// The form, inert, with the submit button in its loading state.
    Sending,
    /// The thanks view: the check draws, the sprinkles burst.
    Sent,
    /// The failure view: the alert circle and a retry button.
    Error,
}

impl FeedbackStatus {
    /// Whether the shell is open at all (`open = status !== "idle"`).
    pub const fn is_open(self) -> bool {
        !matches!(self, FeedbackStatus::Idle)
    }

    /// Whether a send is in flight (`busy = status === "sending"`), which
    /// disables every affordance the panel shows.
    pub const fn is_busy(self) -> bool {
        matches!(self, FeedbackStatus::Sending)
    }

    /// Which of the three panel views this status paints.
    const fn view(self) -> PanelView {
        match self {
            FeedbackStatus::Sent => PanelView::Sent,
            FeedbackStatus::Error => PanelView::Error,
            _ => PanelView::Form,
        }
    }
}

/// Which corner the block grows out of — upstream's `position`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeedbackPosition {
    /// `"bottom-right"`, the default.
    #[default]
    BottomRight,
    /// `"bottom-left"`.
    BottomLeft,
}

impl FeedbackPosition {
    /// Whether the shell is anchored to the left edge (`left = position === "bottom-left"`).
    const fn is_left(self) -> bool {
        matches!(self, FeedbackPosition::BottomLeft)
    }
}

/// The three things the panel can be showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PanelView {
    #[default]
    Form,
    Sent,
    Error,
}

// ---- View ------------------------------------------------------------------

/// A view-held, typed string callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held status callback.
type OnStatus<State> = Rc<dyn Fn(&mut State, FeedbackStatus)>;

/// A declarative beUI feedback widget. See the [module docs](self).
pub struct FeedbackWidgetView<State: 'static> {
    status: FeedbackStatus,
    message: String,
    position: FeedbackPosition,
    title: String,
    placeholder: String,
    cancel_label: String,
    submit_label: String,
    sent_title: String,
    sent_body: String,
    error_title: String,
    error_body: String,
    retry_label: String,
    on_status_change: OnStatus<State>,
    on_message_change: OnText<State>,
    on_submit: Option<OnText<State>>,
}

/// Create a feedback widget in `status` holding `message`, reporting every
/// status a press implies through `on_status_change` — a **controlled**
/// component (see the [module docs](self)).
pub fn feedback_widget<State: 'static, F, G>(
    status: FeedbackStatus,
    message: impl Into<String>,
    on_status_change: F,
    on_message_change: G,
) -> FeedbackWidgetView<State>
where
    F: Fn(&mut State, FeedbackStatus) + 'static,
    G: Fn(&mut State, String) + 'static,
{
    FeedbackWidgetView {
        status,
        message: message.into(),
        position: FeedbackPosition::default(),
        // The component's own prop defaults, and the copy its three views ship.
        title: "Help us improve".to_string(),
        placeholder: "Share an idea or report a bug".to_string(),
        cancel_label: "Cancel".to_string(),
        submit_label: "Submit".to_string(),
        sent_title: "Thanks!".to_string(),
        sent_body: "Your feedback helps us build something better.".to_string(),
        error_title: "Something went wrong".to_string(),
        error_body: "We couldn't send your feedback. Please try again.".to_string(),
        retry_label: "Try again".to_string(),
        on_status_change: Rc::new(on_status_change),
        on_message_change: Rc::new(on_message_change),
        on_submit: None,
    }
}

impl<State: 'static> FeedbackWidgetView<State> {
    /// Select the corner the block grows out of (`position`).
    pub fn position(mut self, position: FeedbackPosition) -> Self {
        self.position = position;
        self
    }

    /// Set the form's headline (`title`), which is also the trigger's
    /// accessible name (`aria-label={title}`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the message field's placeholder (`placeholder`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the two action labels (`Cancel` / `Submit`).
    pub fn action_labels(mut self, cancel: impl Into<String>, submit: impl Into<String>) -> Self {
        self.cancel_label = cancel.into();
        self.submit_label = submit.into();
        self
    }

    /// Replace the thanks view's copy.
    pub fn sent_copy(mut self, title: impl Into<String>, body: impl Into<String>) -> Self {
        self.sent_title = title.into();
        self.sent_body = body.into();
        self
    }

    /// Replace the failure view's copy and its retry label.
    pub fn error_copy(
        mut self,
        title: impl Into<String>,
        body: impl Into<String>,
        retry: impl Into<String>,
    ) -> Self {
        self.error_title = title.into();
        self.error_body = body.into();
        self.retry_label = retry.into();
        self
    }

    /// Report a submit press with the current message (`onSubmit`).
    ///
    /// Fired alongside — not instead of — the
    /// [`FeedbackStatus::Sending`] status change, so an app can drive its task
    /// from this and its chrome from the status.
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Whether a submit is permitted right now — upstream's
    /// `disabled={busy || message.trim().length === 0}`.
    fn can_submit(&self) -> bool {
        !self.status.is_busy() && !self.message.trim().is_empty()
    }

    /// The wrapped message field, configured to match `rows={3}` with its own
    /// chrome suppressed (see the [module docs](self)).
    fn field_view(&self) -> AnyView<State> {
        let on_change = self.on_message_change.clone();
        any(
            text_input(self.message.clone(), move |state: &mut State, text| {
                on_change(state, text);
            })
            .placeholder(self.placeholder.clone())
            .enabled(!self.status.is_busy())
            .multiline(3)
            .submit_on_enter(false)
            .border_width(0.0)
            .corner_radius(style::RADIUS_MD)
            .padding(0.0, 0.0),
        )
    }

    /// The `Cancel` button — a secondary pill that reports
    /// [`FeedbackStatus::Idle`].
    fn cancel_view(&self) -> AnyView<State> {
        let on_status = self.on_status_change.clone();
        any(
            button::<State>(self.cancel_label.clone(), move |state: &mut State| {
                on_status(state, FeedbackStatus::Idle);
            })
            .tone(ButtonTone::Secondary)
            .size(ButtonSize::Md)
            .disabled(self.status.is_busy()),
        )
    }

    /// The primary button: `Submit` on the form (stateful, so the sending state
    /// is the button's own), `Try again` on the failure view.
    fn primary_view(&self) -> AnyView<State> {
        let on_status = self.on_status_change.clone();
        let on_submit = self.on_submit.clone();
        let message = self.message.clone();
        let armed = self.can_submit();
        let label = if self.status == FeedbackStatus::Error {
            self.retry_label.clone()
        } else {
            self.submit_label.clone()
        };
        any(button::<State>(label, move |state: &mut State| {
            if !armed {
                return;
            }
            on_status(state, FeedbackStatus::Sending);
            if let Some(on_submit) = &on_submit {
                on_submit(state, message.clone());
            }
        })
        .variant(ButtonVariant::Stateful)
        .state(if self.status.is_busy() {
            ButtonState::Loading
        } else {
            ButtonState::Idle
        })
        .loading_label("Sending")
        .size(ButtonSize::Md)
        .disabled(!armed))
    }
}

// ---- Widget ----------------------------------------------------------------

/// Which affordance a press landed on — the two this widget owns itself; the
/// buttons and the field are child pods and route their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The closed corner trigger.
    Trigger,
    /// The form's close affordance.
    Close,
}

/// The retained widget for a [`FeedbackWidgetView`].
pub struct FeedbackWidgetWidget {
    field: ChildPod,
    cancel: ChildPod,
    primary: ChildPod,
    status: FeedbackStatus,
    position: FeedbackPosition,
    /// The view the panel is showing, and the one it is crossfading away from.
    view: PanelView,
    previous_view: Option<PanelView>,
    title: Label,
    /// The title string, kept beside its shaped run because a cached run holds
    /// no readable copy and semantics needs the text.
    title_text: String,
    sent_title: Label,
    sent_body: Label,
    error_title: Label,
    error_body: Label,
    /// The shell's open/closed driver — the morph and the content crossfade are
    /// both staged from it.
    presence: Presence,
    /// What [`Presence::advance`] last reported.
    shown: f64,
    /// The view crossfade, `0.0` on the outgoing view .. `1.0` on the new one.
    swap: Lane,
    /// The trigger's press shrink.
    press: Lane,
    /// The thanks view's pop/draw/burst: armed by `rebuild`, stamped by `paint`.
    celebrate_armed: bool,
    celebrate_start: Option<FrameTime>,
    /// The affordance a `Down` armed.
    armed: Option<Target>,
    /// The latched hovered affordance, self-corrected at paint time.
    hovered: Option<Target>,
    /// The reserved box, and the boxes resolved inside it.
    reserved: Size,
    panel: Rect,
    trigger: Rect,
    close: Rect,
    on_status_change: ErasedArgCallback<FeedbackStatus>,
}

/// The enter/exit ramps the shell morphs on.
fn morph_presence(reduce: bool) -> Presence {
    let presence = Presence::new(
        Ramp::eased(Duration::from_millis(MORPH_OPEN_MS), MORPH_OPEN_EASE),
        Ramp::eased(Duration::from_millis(MORPH_CLOSE_MS), MORPH_CLOSE_EASE),
    );
    if reduce {
        presence.collapsed()
    } else {
        presence
    }
}

/// The ramp a panel view crossfades on.
const SWAP_RAMP: Ramp = Ramp::eased(Duration::from_millis(VIEW_ENTER_MS), EASE_OUT);
/// The ramp the trigger's press shrink runs on.
const PRESS_RAMP: Ramp = Ramp::eased(Duration::from_millis(VIEW_EXIT_MS), EASE_OUT);

impl<State: 'static> View<State> for FeedbackWidgetView<State> {
    type Element = FeedbackWidgetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FeedbackWidgetWidget {
        let mut presence = morph_presence(false);
        presence.set_open(self.status.is_open());
        // `<AnimatePresence initial={false}>`: a block that mounts open is
        // simply open, it does not play its entrance.
        presence.advance(FrameTime::ZERO);
        let shown = presence.advance(FrameTime::from_nanos(
            Duration::from_millis(MORPH_OPEN_MS).as_nanos() as u64 + 1,
        ));
        FeedbackWidgetWidget {
            field: build_child(&self.field_view(), ctx),
            cancel: build_child(&self.cancel_view(), ctx),
            primary: build_child(&self.primary_view(), ctx),
            status: self.status,
            position: self.position,
            view: self.status.view(),
            previous_view: None,
            title: Label::new(&self.title),
            title_text: self.title.clone(),
            sent_title: Label::new(&self.sent_title),
            sent_body: Label::new(&self.sent_body),
            error_title: Label::new(&self.error_title),
            error_body: Label::new(&self.error_body),
            presence,
            shown,
            swap: Lane::at_rest(SWAP_RAMP, 1.0),
            press: Lane::at_rest(PRESS_RAMP, 0.0),
            celebrate_armed: false,
            celebrate_start: None,
            armed: None,
            hovered: None,
            reserved: Size::ZERO,
            panel: Rect::ZERO,
            trigger: Rect::ZERO,
            close: Rect::ZERO,
            on_status_change: erase_callback_arg(&self.on_status_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FeedbackWidgetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_status_change = erase_callback_arg(&self.on_status_change);

        let mut flags = rebuild_child(
            &prev.field_view(),
            &self.field_view(),
            &mut element.field,
            ctx,
        );
        flags |= rebuild_child(
            &prev.cancel_view(),
            &self.cancel_view(),
            &mut element.cancel,
            ctx,
        );
        flags |= rebuild_child(
            &prev.primary_view(),
            &self.primary_view(),
            &mut element.primary,
            ctx,
        );

        if prev.status != self.status {
            element.status = self.status;
            if prev.status.is_open() != self.status.is_open() {
                element.presence.set_open(self.status.is_open());
                if !self.status.is_open() {
                    // A block closed mid-press keeps no armed state behind.
                    element.armed = None;
                }
            }
            let view = self.status.view();
            if element.view != view {
                element.previous_view = Some(element.view);
                element.view = view;
                element.swap.retarget(0.0);
                element.swap.snap();
                element.swap.retarget(1.0);
            }
            if view == PanelView::Sent && prev.status.view() != PanelView::Sent {
                element.celebrate_armed = true;
                element.celebrate_start = None;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.position != self.position {
            element.position = self.position;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.title.set_content(&self.title)
            | element.sent_title.set_content(&self.sent_title)
            | element.sent_body.set_content(&self.sent_body)
            | element.error_title.set_content(&self.error_title)
            | element.error_body.set_content(&self.error_body)
        {
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut FeedbackWidgetWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.field_view(), &mut element.field, ctx);
        teardown_child(&self.cancel_view(), &mut element.cancel, ctx);
        teardown_child(&self.primary_view(), &mut element.primary, ctx);
    }
}

/// The palette the block paints from.
struct FeedbackColors {
    border: Color,
    ink: Color,
    muted: Color,
    background: Color,
    destructive: Color,
    destructive_wash: Color,
    success: Color,
    accent: Color,
}

fn resolve_colors(theme: Option<&Theme>) -> FeedbackColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            FeedbackColors {
                border: scheme.outline_variant,
                ink: scheme.on_surface,
                muted: scheme.on_surface_variant,
                background: scheme.surface,
                destructive: scheme.error,
                destructive_wash: scheme.error_container,
                success: BeuiTokens::resolve(Some(theme)).success,
                accent: scheme.primary_container,
            }
        }
        None => FeedbackColors {
            border: FALLBACK_BORDER,
            ink: FALLBACK_FOREGROUND,
            muted: FALLBACK_MUTED,
            background: FALLBACK_BACKGROUND,
            destructive: FALLBACK_DESTRUCTIVE,
            destructive_wash: style::with_alpha(FALLBACK_DESTRUCTIVE, 0.1),
            success: BeuiTokens::beui().success,
            accent: crate::BEUI_LIGHT.accent,
        },
    }
}

/// `font-semibold` at `size`.
fn semibold(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::SEMI_BOLD,
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

/// A plain run at `size`.
fn plain(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

impl FeedbackWidgetWidget {
    /// The action row's height: two `h-10` pills with their own padding.
    const ACTIONS_HEIGHT: f64 = ACTIONS_PADDING_TOP + style::HEIGHT_MD + ACTIONS_PADDING_BOTTOM;

    /// The card height the form view needs.
    fn form_card_height(&self) -> f64 {
        let title = self.title.size().height;
        let field = self.field.size().height.max(style::HEIGHT_MD);
        (FORM_PADDING_Y * 2.0 + title + FIELD_GAP + field).max(FORM_MIN_HEIGHT)
    }

    /// The card height the thanks view needs.
    fn sent_card_height(&self) -> f64 {
        SENT_PADDING_Y * 2.0
            + STATUS_CIRCLE
            + STATUS_GAP
            + self.sent_title.size().height
            + STATUS_TITLE_GAP
            + self.sent_body.size().height
    }

    /// The card height the failure view needs.
    fn error_card_height(&self) -> f64 {
        ERROR_PADDING_Y * 2.0
            + STATUS_CIRCLE
            + STATUS_GAP
            + self.error_title.size().height
            + STATUS_TITLE_GAP
            + self.error_body.size().height
            + RETRY_GAP
            + style::HEIGHT_MD
    }

    /// The whole panel's height for `view`.
    fn panel_height(&self, view: PanelView) -> f64 {
        let card = match view {
            PanelView::Form => self.form_card_height() + Self::ACTIONS_HEIGHT,
            PanelView::Sent => self.sent_card_height(),
            PanelView::Error => self.error_card_height(),
        };
        PANEL_PADDING * 2.0 + card
    }

    /// The shell's box right now: the trigger box, the open panel box, or the
    /// morph between them — anchored to the block's own corner throughout.
    fn shell_rect(&self) -> Rect {
        let t = self.shown.clamp(0.0, 1.0);
        let width = TRIGGER_SIZE + (self.panel.width() - TRIGGER_SIZE) * t;
        let height = TRIGGER_SIZE + (self.panel.height() - TRIGGER_SIZE) * t;
        let x = if self.position.is_left() {
            0.0
        } else {
            self.reserved.width - width
        };
        Rect::from_origin_size(
            Point::new(x, self.reserved.height - height),
            Size::new(width, height),
        )
    }

    /// The shell's corner radius right now (`borderRadius: open ? 20 : 40`).
    fn shell_radius(&self) -> f64 {
        let t = self.shown.clamp(0.0, 1.0);
        TRIGGER_RADIUS + (PANEL_RADIUS - TRIGGER_RADIUS) * t
    }

    /// How far a view's content slides in from, signed away from the corner.
    fn content_offset(&self) -> f64 {
        if self.position.is_left() {
            -MORPH_SLIDE
        } else {
            MORPH_SLIDE
        }
    }

    /// The thanks view's `(circle scale, check draw, sprinkle elapsed, owes a
    /// frame)` at `now`.
    fn celebration(&self, now: FrameTime) -> (f64, f64, f64, bool) {
        if self.view != PanelView::Sent {
            return (1.0, 0.0, 0.0, false);
        }
        let Some(start) = self.celebrate_start else {
            // Mounted already sent: everything is simply drawn.
            return (1.0, 1.0, f64::from(u16::MAX), false);
        };
        let elapsed = now.saturating_sub(start).as_secs_f64() * 1000.0;

        let pop_elapsed = (elapsed - CIRCLE_POP_DELAY_MS).max(0.0);
        let pop = Ramp::spring(CIRCLE_POP)
            .progress_clamped(Duration::from_secs_f64(pop_elapsed / 1000.0));

        let draw_elapsed = elapsed - CHECK_DRAW_DELAY_MS;
        let drawn = if draw_elapsed <= 0.0 {
            0.0
        } else {
            Curve::EaseOut.transform((draw_elapsed / CHECK_DRAW_MS).min(1.0))
        };

        let burst_end = SPRINKLE_DELAY_MS
            + (SPRINKLE_COUNT.saturating_sub(1) * SPRINKLE_STEP_MS as usize) as f64
            + SPRINKLE_MS as f64;
        let running = pop < 1.0 || draw_elapsed < CHECK_DRAW_MS || elapsed < burst_end;
        (pop, drawn, elapsed, running)
    }

    /// What a widget-local point lands on, if anything this widget owns.
    fn target_at(&self, at: Point) -> Option<Target> {
        if self.status.is_open() {
            if self.view == PanelView::Form && !self.status.is_busy() && self.close.contains(at) {
                return Some(Target::Close);
            }
            return None;
        }
        inside_inclusive(at - self.trigger.origin().to_vec2(), self.trigger.size())
            .then_some(Target::Trigger)
    }

    /// The stagger the sprinkle burst is timed by.
    fn sprinkle_stagger() -> Stagger {
        Stagger::eased(
            Duration::from_millis(SPRINKLE_STEP_MS),
            Duration::from_millis(SPRINKLE_MS),
            Curve::EaseOut,
        )
    }
}

impl Widget for FeedbackWidgetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);

        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            PANEL_MAX_WIDTH
        };
        let panel_width = PANEL_MAX_WIDTH
            .min(available * PANEL_WIDTH_FRACTION)
            .max(TRIGGER_SIZE);

        self.title
            .layout(ctx, &semibold(style::TEXT_SM, colors.ink));
        self.sent_title
            .layout(ctx, &semibold(style::TEXT_SM, colors.ink));
        self.sent_body
            .layout(ctx, &plain(style::TEXT_XS, colors.muted));
        self.error_title
            .layout(ctx, &semibold(style::TEXT_SM, colors.ink));
        self.error_body
            .layout(ctx, &plain(style::TEXT_XS, colors.muted));

        // The message field, laid out only while the form is what is showing —
        // a closed block gives its pods no box to be hit in.
        let card_width = panel_width - PANEL_PADDING * 2.0;
        let field_width = (card_width - CARD_PADDING_X * 2.0).max(1.0);
        let showing_form = self.status.is_open() && self.view == PanelView::Form;
        if showing_form {
            self.field.layout_child(
                ctx,
                &BoxConstraints::new(Size::ZERO, Size::new(field_width, f64::INFINITY)),
            );
        } else {
            self.field
                .layout_child(ctx, &BoxConstraints::tight(Size::ZERO));
        }

        // The two action buttons: `flex-1` each on the form, one centred pill
        // on the failure view.
        let action_width = ((card_width - ACTIONS_PADDING_X * 2.0 - ACTIONS_GAP) / 2.0).max(1.0);
        if showing_form {
            self.cancel.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(action_width, style::HEIGHT_MD)),
            );
            self.primary.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(action_width, style::HEIGHT_MD)),
            );
        } else if self.status.is_open() && self.view == PanelView::Error {
            self.cancel
                .layout_child(ctx, &BoxConstraints::tight(Size::ZERO));
            self.primary.layout_child(
                ctx,
                &BoxConstraints::new(
                    Size::new(0.0, style::HEIGHT_MD),
                    Size::new(field_width, style::HEIGHT_MD),
                ),
            );
        } else {
            self.cancel
                .layout_child(ctx, &BoxConstraints::tight(Size::ZERO));
            self.primary
                .layout_child(ctx, &BoxConstraints::tight(Size::ZERO));
        }

        // The block reserves the tallest panel it could show, so the morph
        // never relayouts the page around it.
        let reserved_height = [PanelView::Form, PanelView::Sent, PanelView::Error]
            .into_iter()
            .map(|view| self.panel_height(view))
            .fold(TRIGGER_SIZE, f64::max);
        self.reserved = Size::new(panel_width, reserved_height);

        // The open panel is as wide as the reserved box and as tall as the view
        // it is showing, anchored to the bottom of that box.
        let panel_height = self.panel_height(self.view);
        self.panel = Rect::from_origin_size(
            Point::new(0.0, reserved_height - panel_height),
            Size::new(panel_width, panel_height),
        );
        let trigger_x = if self.position.is_left() {
            0.0
        } else {
            panel_width - TRIGGER_SIZE
        };
        self.trigger = Rect::from_origin_size(
            Point::new(trigger_x, reserved_height - TRIGGER_SIZE),
            Size::new(TRIGGER_SIZE, TRIGGER_SIZE),
        );

        // The form's own furniture, placed against the resolved panel.
        let card = Rect::from_origin_size(
            Point::new(self.panel.x0 + PANEL_PADDING, self.panel.y0 + PANEL_PADDING),
            Size::new(card_width, self.form_card_height()),
        );
        self.close = Rect::from_origin_size(
            Point::new(
                card.max_x() - CARD_PADDING_X - CLOSE_BOX,
                card.y0 + FORM_PADDING_Y,
            ),
            Size::new(CLOSE_BOX, CLOSE_BOX),
        );
        self.field.set_origin(Point::new(
            card.x0 + CARD_PADDING_X,
            card.y0 + FORM_PADDING_Y + self.title.size().height + FIELD_GAP,
        ));
        let actions_y = card.max_y() + ACTIONS_PADDING_TOP;
        self.cancel
            .set_origin(Point::new(card.x0 + ACTIONS_PADDING_X, actions_y));
        match self.view {
            PanelView::Error => {
                let width = self.primary.size().width;
                self.primary.set_origin(Point::new(
                    card.x0 + (card_width - width) / 2.0,
                    self.panel.max_y() - PANEL_PADDING - ERROR_PADDING_Y - style::HEIGHT_MD,
                ));
            }
            _ => {
                self.primary.set_origin(Point::new(
                    card.x0 + ACTIONS_PADDING_X + action_width + ACTIONS_GAP,
                    actions_y,
                ));
            }
        }

        bc.constrain(self.reserved)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let origin = ctx.origin();

        let (colors, reduce) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };

        self.press.retarget(if self.armed == Some(Target::Trigger) {
            1.0
        } else {
            0.0
        });

        let mut owes_frame = false;
        if reduce {
            self.presence = self.presence.collapsed();
            self.swap.snap();
            self.press.snap();
            self.celebrate_armed = false;
            self.celebrate_start = None;
        } else {
            if self.celebrate_armed {
                self.celebrate_armed = false;
                self.celebrate_start = Some(now);
            }
            owes_frame |= self.swap.advance(now);
            owes_frame |= self.press.advance(now);
        }
        self.shown = self.presence.advance(now);
        owes_frame |= self.presence.is_animating();

        let swap = self.swap.value().clamp(0.0, 1.0);
        if swap >= 1.0 {
            self.previous_view = None;
        }
        let (pop, drawn, burst, celebrating) = self.celebration(now);
        owes_frame |= celebrating;

        // The shell itself, morphing between the trigger and the panel.
        let shell = self.shell_rect();
        let at = origin + shell.origin().to_vec2();
        let radius = style::resolve_radius(self.shell_radius(), shell.width(), shell.height());
        // `whileTap={{ scale: 0.92 }}` applies to the closed trigger only.
        let scale = 1.0 - (1.0 - TRIGGER_PRESS_SCALE) * self.press.value().clamp(0.0, 1.0);
        let box_at = Point::new(
            at.x + shell.width() * (1.0 - scale) / 2.0,
            at.y + shell.height() * (1.0 - scale) / 2.0,
        );
        let box_size = Size::new(shell.width() * scale, shell.height() * scale);

        scene.fill_rounded_rect(box_at, box_size, radius, colors.background);
        let half = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, box_size).inset(-half),
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            box_at,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(colors.border),
        );

        let shown = self.shown.clamp(0.0, 1.0);
        let offset = self.content_offset();

        // The closed trigger's glyph, sliding and turning in as the panel goes.
        if shown < 1.0 {
            let alpha = 1.0 - shown;
            let centre = Point::new(
                box_at.x + box_size.width / 2.0 - offset * shown,
                box_at.y + box_size.height / 2.0,
            );
            scene.push_layer(box_at, box_size, alpha as f32);
            scene.push_transform(Affine::rotate_about(TRIGGER_ICON_ROTATION * shown, centre));
            message_glyph(centre, TRIGGER_ICON, colors.ink, scene);
            scene.pop_transform();
            scene.pop_layer();
        }

        // The open panel's content: the outgoing view first, then the new one.
        if shown > 0.0 {
            scene.push_layer(box_at, box_size, shown as f32);
            let slide = offset * (1.0 - shown);
            if let Some(previous) = self.previous_view {
                self.paint_view(
                    previous,
                    origin,
                    slide,
                    1.0 - swap,
                    -VIEW_RISE * swap,
                    &colors,
                    (pop, drawn, burst),
                    scene,
                );
            }
            self.paint_view(
                self.view,
                origin,
                slide,
                swap,
                VIEW_RISE * (1.0 - swap),
                &colors,
                (pop, drawn, burst),
                scene,
            );
            scene.pop_layer();
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The pods first: the field and both buttons own their own gestures.
        // Routed one at a time rather than as a slice because they are three
        // named fields; a broadcast still reaches all three, since each call
        // reports `Ignored` for one.
        if route_event_single(&mut self.field, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if route_event_single(&mut self.cancel, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if route_event_single(&mut self.primary, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(target) = self.target_at(p.position) else {
                    // Inside the reserved box but outside the shell: whatever
                    // the app stacked underneath still gets it.
                    return EventResult::Ignored;
                };
                self.armed = Some(target);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.armed.is_none() {
                    let target = self.target_at(p.position);
                    if target != self.hovered {
                        self.hovered = target;
                        ctx.request_redraw();
                    }
                    if target.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                ctx.request_redraw();
                // Fire on up-inside only.
                if self.target_at(p.position) == Some(armed) {
                    let next = match armed {
                        Target::Trigger => FeedbackStatus::Open,
                        Target::Close => FeedbackStatus::Idle,
                    };
                    (self.on_status_change)(ctx, next);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                // A `Cancel` arm clears internal flags only — never a callback.
                self.armed = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !self.status.is_open() {
            ctx.push_node(Role::Button, |node| {
                node.set_label(self.title_text.as_str());
                node.add_action(Action::Click);
            });
            return;
        }
        ctx.push_container(
            Role::Dialog,
            |node| node.set_label(self.title_text.as_str()),
            |ctx| {
                self.field.semantics_child(ctx);
                self.cancel.semantics_child(ctx);
                self.primary.semantics_child(ctx);
            },
        );
    }

    visit_children!(field, cancel, primary);
}

impl FeedbackWidgetWidget {
    /// Paint one panel view at `alpha`, displaced by `slide`/`rise`.
    #[allow(clippy::too_many_arguments)]
    fn paint_view(
        &self,
        view: PanelView,
        origin: Point,
        slide: f64,
        alpha: f64,
        rise: f64,
        colors: &FeedbackColors,
        celebration: (f64, f64, f64),
        scene: &mut dyn PaintScene,
    ) {
        if alpha <= 0.0 {
            return;
        }
        let card_width = self.panel.width() - PANEL_PADDING * 2.0;
        let card_height = match view {
            PanelView::Form => self.form_card_height(),
            PanelView::Sent => self.sent_card_height(),
            PanelView::Error => self.error_card_height(),
        };
        // `scale: 0.97` about the card's own centre, folded into the offsets.
        let inset = card_width * (1.0 - MORPH_SCALE) * (1.0 - alpha) / 2.0;
        let card = Point::new(
            origin.x + self.panel.x0 + PANEL_PADDING + slide + inset,
            origin.y + self.panel.y0 + PANEL_PADDING + rise,
        );
        let size = Size::new(card_width - inset * 2.0, card_height);

        scene.push_layer(card, size, alpha as f32);
        scene.fill_rounded_rect(
            card,
            size,
            PANEL_RADIUS,
            style::scale_alpha(colors.border, CARD_WASH_ALPHA),
        );

        match view {
            PanelView::Form => self.paint_form(card, size, colors, scene),
            PanelView::Sent => self.paint_sent(card, size, colors, celebration, scene),
            PanelView::Error => self.paint_error(card, size, colors, scene),
        }
        scene.pop_layer();
    }

    /// The form card: the title row, the close affordance, and the field's box.
    fn paint_form(
        &self,
        card: Point,
        size: Size,
        colors: &FeedbackColors,
        scene: &mut dyn PaintScene,
    ) {
        self.title.paint(
            Point::new(card.x + CARD_PADDING_X, card.y + FORM_PADDING_Y),
            scene,
        );
        let close = Point::new(
            card.x + size.width - CARD_PADDING_X - CLOSE_BOX,
            card.y + FORM_PADDING_Y,
        );
        scene.fill_rounded_rect(
            close,
            Size::new(CLOSE_BOX, CLOSE_BOX),
            CLOSE_BOX / 2.0,
            style::with_alpha(colors.ink, CLOSE_WASH_ALPHA),
        );
        let centre = Point::new(close.x + CLOSE_BOX / 2.0, close.y + CLOSE_BOX / 2.0);
        let ink = if self.hovered == Some(Target::Close) {
            colors.ink
        } else {
            colors.muted
        };
        cross_glyph(centre, CLOSE_ICON * 0.7, ink, scene);
    }

    /// The thanks card: the popping success circle, the check drawing itself
    /// on, the sprinkle burst, and the two copy lines.
    fn paint_sent(
        &self,
        card: Point,
        size: Size,
        colors: &FeedbackColors,
        celebration: (f64, f64, f64),
        scene: &mut dyn PaintScene,
    ) {
        let (pop, drawn, burst) = celebration;
        let centre_x = card.x + size.width / 2.0;
        let circle_centre = Point::new(centre_x, card.y + SENT_PADDING_Y + STATUS_CIRCLE / 2.0);

        // The sprinkles burst from behind the circle.
        let stagger = Self::sprinkle_stagger();
        let elapsed = (burst - SPRINKLE_DELAY_MS).max(0.0);
        for index in 0..SPRINKLE_COUNT {
            let t = stagger.progress_clamped(
                Duration::from_secs_f64(elapsed / 1000.0),
                index,
                SPRINKLE_COUNT,
            );
            if t <= 0.0 || t >= 1.0 {
                continue;
            }
            let angle = index as f64 / SPRINKLE_COUNT as f64 * std::f64::consts::TAU;
            // `opacity: [0, 1, 0]`, `scale: [0, 1, 0.4]` across the flight.
            let fade = 1.0 - (t * 2.0 - 1.0).abs();
            let dot = SPRINKLE_SIZE
                * (if t < 0.5 {
                    t * 2.0
                } else {
                    1.0 - (t - 0.5) * 1.2
                });
            if dot <= 0.0 {
                continue;
            }
            let at = Point::new(
                circle_centre.x + angle.cos() * SPRINKLE_RADIUS * t - dot / 2.0,
                circle_centre.y + angle.sin() * SPRINKLE_RADIUS * t - dot / 2.0,
            );
            let color = if index % 2 == 0 {
                colors.success
            } else {
                colors.accent
            };
            scene.fill_rounded_rect(
                at,
                Size::new(dot, dot),
                dot / 2.0,
                style::with_alpha(color, fade as f32),
            );
        }

        let edge = STATUS_CIRCLE * pop.clamp(0.0, 1.0);
        if edge > 0.0 {
            scene.fill_rounded_rect(
                Point::new(circle_centre.x - edge / 2.0, circle_centre.y - edge / 2.0),
                Size::new(edge, edge),
                edge / 2.0,
                colors.success,
            );
        }
        if drawn > 0.0 {
            let icon = STATUS_ICON;
            let path = mark_path(&CHECK_POINTS, icon, drawn);
            scene.stroke_path(
                Point::new(circle_centre.x - icon / 2.0, circle_centre.y - icon / 2.0),
                &path,
                2.5 * icon / ICON_VIEWBOX,
                &Brush::Solid(Color::WHITE),
            );
        }

        let mut y = card.y + SENT_PADDING_Y + STATUS_CIRCLE + STATUS_GAP;
        let title = self.sent_title.size();
        self.sent_title
            .paint(Point::new(centre_x - title.width / 2.0, y), scene);
        y += title.height + STATUS_TITLE_GAP;
        let body = self.sent_body.size();
        self.sent_body
            .paint(Point::new(centre_x - body.width / 2.0, y), scene);
    }

    /// The failure card: the destructive circle, the copy, and the space the
    /// retry pod occupies.
    fn paint_error(
        &self,
        card: Point,
        size: Size,
        colors: &FeedbackColors,
        scene: &mut dyn PaintScene,
    ) {
        let centre_x = card.x + size.width / 2.0;
        let circle_centre = Point::new(centre_x, card.y + ERROR_PADDING_Y + STATUS_CIRCLE / 2.0);
        scene.fill_rounded_rect(
            Point::new(
                circle_centre.x - STATUS_CIRCLE / 2.0,
                circle_centre.y - STATUS_CIRCLE / 2.0,
            ),
            Size::new(STATUS_CIRCLE, STATUS_CIRCLE),
            STATUS_CIRCLE / 2.0,
            colors.destructive_wash,
        );
        alert_glyph(circle_centre, STATUS_ICON, colors.destructive, scene);

        let mut y = card.y + ERROR_PADDING_Y + STATUS_CIRCLE + STATUS_GAP;
        let title = self.error_title.size();
        self.error_title
            .paint(Point::new(centre_x - title.width / 2.0, y), scene);
        y += title.height + STATUS_TITLE_GAP;
        let body = self.error_body.size();
        self.error_body
            .paint(Point::new(centre_x - body.width / 2.0, y), scene);
    }
}

// ---- Glyphs ----------------------------------------------------------------

/// The lucide stroke width, in viewBox units.
const ICON_STROKE_VIEWBOX: f64 = 2.0;

/// The stroke width a `size`-square glyph is drawn at.
fn icon_stroke(size: f64) -> f64 {
    ICON_STROKE_VIEWBOX * size / ICON_VIEWBOX
}

/// A rounded speech box with a tail, centred on `centre` — lucide's
/// `MessageSquare`, the trigger's default glyph.
fn message_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let half = size / 2.0;
    let box_bottom = centre.y + half * 0.35;
    let rect = RoundedRect::new(
        centre.x - half,
        centre.y - half,
        centre.x + half,
        box_bottom,
        size * 0.16,
    );
    let stroke = icon_stroke(size);
    scene.stroke_path(
        Point::ZERO,
        &Shape::to_path(&rect, style::PATH_TOLERANCE),
        stroke,
        &Brush::Solid(color),
    );
    let mut tail = BezPath::new();
    tail.move_to(Point::new(centre.x - half * 0.45, box_bottom));
    tail.line_to(Point::new(centre.x - half * 0.45, centre.y + half));
    tail.line_to(Point::new(centre.x + half * 0.05, box_bottom));
    scene.stroke_path(Point::ZERO, &tail, stroke, &Brush::Solid(color));
}

/// Lucide's `X`, centred on `centre`.
fn cross_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let arm = size / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - arm, centre.y - arm));
    path.line_to(Point::new(centre.x + arm, centre.y + arm));
    path.move_to(Point::new(centre.x + arm, centre.y - arm));
    path.line_to(Point::new(centre.x - arm, centre.y + arm));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

/// A ring with an exclamation bar, centred on `centre` — lucide's `AlertCircle`.
fn alert_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let radius = size / 2.0 - icon_stroke(size) / 2.0;
    let stroke = icon_stroke(size);
    let ring = arc_path(centre, radius, 0.0, std::f64::consts::TAU);
    scene.stroke_path(Point::ZERO, &ring, stroke, &Brush::Solid(color));
    let mut bar = BezPath::new();
    bar.move_to(Point::new(centre.x, centre.y - radius * 0.5));
    bar.line_to(Point::new(centre.x, centre.y + radius * 0.15));
    bar.move_to(Point::new(centre.x, centre.y + radius * 0.5));
    bar.line_to(Point::new(centre.x, centre.y + radius * 0.55));
    scene.stroke_path(Point::ZERO, &bar, stroke, &Brush::Solid(color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    /// Records the ops these tests assert on.
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
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
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
        /// The shell's own fill — the first rounded rect of the pass.
        fn shell(&self) -> (Point, Size, f64, Color) {
            self.rrects[0]
        }

        /// Every circular fill of the pass (the status circle, the sprinkles,
        /// the close wash), by edge length.
        fn circles(&self) -> Vec<(Point, f64, Color)> {
            self.rrects
                .iter()
                .filter(|(_, s, r, _)| {
                    (s.width - s.height).abs() < 1e-9 && (*r - s.width / 2.0).abs() < 1e-9
                })
                .map(|(o, s, _, c)| (*o, s.width, *c))
                .collect()
        }
    }

    /// What the app state records.
    #[derive(Default)]
    struct App {
        message: String,
        statuses: Vec<FeedbackStatus>,
        submitted: Vec<String>,
    }

    const AVAILABLE: Size = Size::new(400.0, 600.0);

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn pointer(phase: PointerPhase, at: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: at,
            button: PointerButton::Primary,
        })
    }

    fn view(status: FeedbackStatus, message: &str) -> FeedbackWidgetView<App> {
        feedback_widget::<App, _, _>(
            status,
            message,
            |s: &mut App, next| s.statuses.push(next),
            |s: &mut App, text| s.message = text,
        )
        .on_submit(|s: &mut App, text| s.submitted.push(text))
    }

    /// A widget plus the rebuild an app performs around it.
    struct Bare {
        widget: FeedbackWidgetWidget,
        view: FeedbackWidgetView<App>,
        state: App,
        counter: u64,
        theme: Theme,
    }

    impl Bare {
        fn new(status: FeedbackStatus) -> Self {
            Self::with(view(status, ""))
        }

        fn with(view: FeedbackWidgetView<App>) -> Self {
            let mut counter = 0u64;
            let widget = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut bare = Bare {
                widget,
                view,
                state: App::default(),
                counter,
                theme: crate::theme(),
            };
            bare.layout();
            bare
        }

        fn layout(&mut self) -> Size {
            let mut tcx = TextContext::new();
            let lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let mut lctx = lctx.with_theme(&self.theme as &dyn Any);
            self.widget
                .layout(&mut lctx, &BoxConstraints::loose(AVAILABLE))
        }

        fn rebuild(&mut self, next: FeedbackWidgetView<App>) {
            let mut ctx = BuildCtx::new(&mut self.counter);
            View::<App>::rebuild(&next, &self.view, &mut self.widget, &mut ctx);
            self.view = next;
            self.layout();
        }

        fn paint_at(&mut self, millis: f64) -> Recorder {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ZERO, self.widget.reserved, ft_ms(millis))
                .with_theme(&self.theme as &dyn Any);
            self.widget.paint(&mut ctx, &mut rec);
            rec
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventResult {
            let state: &mut dyn Any = &mut self.state;
            let mut ctx = EventCtx::new(state, Point::ZERO, self.widget.reserved);
            self.widget.event(&mut ctx, event)
        }

        fn click(&mut self, at: Point) {
            self.dispatch(&pointer(PointerPhase::Down, at));
            self.dispatch(&pointer(PointerPhase::Up, at));
        }

        /// Settle every ramp the block is running.
        fn settle(&mut self) {
            self.paint_at(0.0);
            self.paint_at(10_000.0);
        }
    }

    // ---- Layout --------------------------------------------------------------

    #[test]
    fn the_block_reserves_its_tallest_panel_and_anchors_in_the_corner() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        let size = bare.layout();
        assert_eq!(
            size.width,
            PANEL_MAX_WIDTH.min(AVAILABLE.width * PANEL_WIDTH_FRACTION)
        );
        assert!(size.height >= bare.widget.panel_height(PanelView::Form));
        assert!(size.height >= bare.widget.panel_height(PanelView::Sent));
        assert!(size.height >= bare.widget.panel_height(PanelView::Error));

        // The trigger sits in the bottom-right of that reserved box...
        assert_eq!(bare.widget.trigger.max_y(), size.height);
        assert_eq!(bare.widget.trigger.max_x(), size.width);
        // ...and in the bottom-left for the other position.
        let mut left =
            Bare::with(view(FeedbackStatus::Idle, "").position(FeedbackPosition::BottomLeft));
        left.layout();
        assert_eq!(left.widget.trigger.x0, 0.0);
    }

    #[test]
    fn a_narrow_window_caps_the_panel_at_86_percent_of_it() {
        let mut bare = Bare::new(FeedbackStatus::Open);
        let mut tcx = TextContext::new();
        let lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let mut lctx = lctx.with_theme(&bare.theme as &dyn Any);
        let size = bare
            .widget
            .layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        assert!((size.width - 300.0 * PANEL_WIDTH_FRACTION).abs() < 1e-9);
    }

    #[test]
    fn a_closed_block_gives_its_pods_no_box_to_be_hit_in() {
        let bare = Bare::new(FeedbackStatus::Idle);
        assert_eq!(bare.widget.field.size(), Size::ZERO);
        assert_eq!(bare.widget.cancel.size(), Size::ZERO);
        assert_eq!(bare.widget.primary.size(), Size::ZERO);

        let open = Bare::new(FeedbackStatus::Open);
        assert!(open.widget.field.size().height > 0.0);
        assert!(open.widget.cancel.size().width > 0.0);
        assert!(open.widget.primary.size().width > 0.0);
        // The two action pills split the card's width evenly.
        assert_eq!(open.widget.cancel.size(), open.widget.primary.size());
    }

    #[test]
    fn the_failure_view_shows_one_centred_retry_and_no_cancel() {
        let bare = Bare::new(FeedbackStatus::Error);
        assert_eq!(bare.widget.cancel.size(), Size::ZERO);
        assert!(bare.widget.primary.size().width > 0.0);
        let centre = bare.widget.primary.origin().x + bare.widget.primary.size().width / 2.0;
        assert!((centre - bare.widget.panel.center().x).abs() < 1.0);
    }

    // ---- The morph -----------------------------------------------------------

    #[test]
    fn the_shell_grows_from_the_trigger_to_the_panel_and_keeps_its_corner() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        let closed = bare.widget.shell_rect();
        assert_eq!(closed.size(), Size::new(TRIGGER_SIZE, TRIGGER_SIZE));
        assert_eq!(closed.max_x(), bare.widget.reserved.width);
        assert_eq!(closed.max_y(), bare.widget.reserved.height);

        bare.rebuild(view(FeedbackStatus::Open, ""));
        bare.paint_at(0.0);
        let mid = bare.paint_at(MORPH_OPEN_MS as f64 / 2.0);
        let shell = bare.widget.shell_rect();
        assert!(
            shell.width() > TRIGGER_SIZE && shell.width() < bare.widget.panel.width(),
            "mid-morph: {shell:?}"
        );
        // The anchored corner does not move while it grows.
        assert!((shell.max_x() - bare.widget.reserved.width).abs() < 1e-9);
        assert!((shell.max_y() - bare.widget.reserved.height).abs() < 1e-9);
        assert!(!mid.layers.is_empty());

        bare.paint_at(MORPH_OPEN_MS as f64 + 50.0);
        let open = bare.widget.shell_rect();
        assert!((open.width() - bare.widget.panel.width()).abs() < 1e-6);
        assert!((open.height() - bare.widget.panel.height()).abs() < 1e-6);
    }

    #[test]
    fn the_shell_radius_travels_between_the_two_authored_values() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        assert_eq!(bare.widget.shell_radius(), TRIGGER_RADIUS);
        bare.rebuild(view(FeedbackStatus::Open, ""));
        bare.settle();
        assert_eq!(bare.widget.shell_radius(), PANEL_RADIUS);
    }

    #[test]
    fn the_trigger_glyph_is_turned_while_the_panel_is_arriving() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        let closed = bare.paint_at(10_001.0);
        assert!(
            closed
                .transforms
                .iter()
                .all(|t| t.as_coeffs()[1].abs() < 1e-9),
            "a resting trigger's glyph is not turned"
        );

        bare.rebuild(view(FeedbackStatus::Open, ""));
        bare.paint_at(0.0);
        let mid = bare.paint_at(MORPH_OPEN_MS as f64 / 4.0);
        assert!(
            mid.transforms.iter().any(|t| t.as_coeffs()[1].abs() > 1e-6),
            "the leaving glyph turns"
        );
    }

    #[test]
    fn a_block_that_mounts_open_does_not_play_its_entrance() {
        let mut bare = Bare::new(FeedbackStatus::Open);
        let shell = bare.widget.shell_rect();
        assert!((shell.width() - bare.widget.panel.width()).abs() < 1e-6);
        // ...and painting its first frame does not start one.
        bare.paint_at(0.0);
        assert!((bare.widget.shell_rect().width() - bare.widget.panel.width()).abs() < 1e-6);
    }

    // ---- Views ---------------------------------------------------------------

    #[test]
    fn a_view_change_crossfades_the_outgoing_one_out() {
        let mut bare = Bare::new(FeedbackStatus::Open);
        bare.settle();
        assert!(bare.widget.previous_view.is_none());

        bare.rebuild(view(FeedbackStatus::Sent, ""));
        assert_eq!(bare.widget.previous_view, Some(PanelView::Form));
        bare.paint_at(0.0);
        let mid = bare.paint_at(VIEW_ENTER_MS as f64 / 2.0);
        assert!(
            mid.layers.iter().filter(|a| **a > 0.0 && **a < 1.0).count() >= 2,
            "both views composite part-way through the swap"
        );

        bare.paint_at(VIEW_ENTER_MS as f64 + 50.0);
        assert!(
            bare.widget.previous_view.is_none(),
            "the outgoing view is dropped once the swap lands"
        );
    }

    #[test]
    fn the_thanks_view_pops_its_circle_then_draws_its_check() {
        let mut bare = Bare::new(FeedbackStatus::Open);
        bare.settle();
        bare.rebuild(view(FeedbackStatus::Sent, ""));

        bare.paint_at(0.0);
        // Before the circle's own delay nothing is drawn inside it.
        let early = bare.paint_at(CIRCLE_POP_DELAY_MS / 2.0);
        assert!(
            early
                .circles()
                .iter()
                .all(|(_, edge, _)| *edge < STATUS_CIRCLE),
            "the circle has not popped to full size yet"
        );

        let settled = bare.paint_at(4_000.0);
        let theme = crate::theme();
        let success = BeuiTokens::resolve(Some(&theme)).success;
        assert!(
            settled
                .circles()
                .iter()
                .any(|(_, edge, c)| (*edge - STATUS_CIRCLE).abs() < 1e-6 && *c == success),
            "the success circle lands at its full size"
        );
        // The check is a white stroke inside it.
        assert!(
            settled.strokes.iter().any(|(_, _, c)| *c == Color::WHITE),
            "the check draws in white"
        );
    }

    #[test]
    fn the_sprinkles_burst_only_between_their_delay_and_their_end() {
        let mut bare = Bare::new(FeedbackStatus::Open);
        bare.settle();
        bare.rebuild(view(FeedbackStatus::Sent, ""));
        bare.paint_at(0.0);

        let dots = |rec: &Recorder| {
            rec.circles()
                .iter()
                .filter(|(_, edge, _)| *edge > 0.0 && *edge <= SPRINKLE_SIZE)
                .count()
        };
        assert_eq!(dots(&bare.paint_at(SPRINKLE_DELAY_MS / 2.0)), 0);
        assert!(
            dots(&bare.paint_at(SPRINKLE_DELAY_MS + SPRINKLE_MS as f64 / 2.0)) > 0,
            "the burst is in flight"
        );
        assert_eq!(dots(&bare.paint_at(4_000.0)), 0, "and it is over");
    }

    #[test]
    fn the_failure_view_paints_the_destructive_circle_and_its_alert() {
        let mut bare = Bare::new(FeedbackStatus::Error);
        bare.settle();
        let rec = bare.paint_at(10_001.0);
        let wash = crate::theme().scheme().error_container;
        assert!(
            rec.circles()
                .iter()
                .any(|(_, edge, c)| (*edge - STATUS_CIRCLE).abs() < 1e-6 && *c == wash)
        );
        let destructive = crate::theme().scheme().error;
        assert!(rec.strokes.iter().any(|(_, _, c)| *c == destructive));
    }

    // ---- Presses -------------------------------------------------------------

    #[test]
    fn the_trigger_reports_open_on_up_inside_only() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        let trigger = bare.widget.trigger.center();
        bare.click(trigger);
        assert_eq!(bare.state.statuses, vec![FeedbackStatus::Open]);

        // A release that wandered off reports nothing.
        bare.dispatch(&pointer(PointerPhase::Down, trigger));
        bare.dispatch(&pointer(PointerPhase::Up, Point::new(2.0, 2.0)));
        assert_eq!(bare.state.statuses.len(), 1);
        assert!(bare.widget.armed.is_none());
    }

    #[test]
    fn a_press_in_the_reserved_box_but_off_the_shell_is_ignored() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        // The top-left corner of the reserved box is nowhere near the trigger.
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Down, Point::new(2.0, 2.0))),
            EventResult::Ignored
        );
        assert!(bare.widget.armed.is_none());
    }

    #[test]
    fn the_close_affordance_reports_idle_and_is_inert_while_sending() {
        let mut bare = Bare::new(FeedbackStatus::Open);
        bare.settle();
        let close = bare.widget.close.center();
        bare.click(close);
        assert_eq!(bare.state.statuses, vec![FeedbackStatus::Idle]);

        let mut sending = Bare::new(FeedbackStatus::Sending);
        sending.settle();
        assert_eq!(
            sending.dispatch(&pointer(PointerPhase::Down, close)),
            EventResult::Ignored,
            "the panel is inert mid-send"
        );
    }

    #[test]
    fn a_non_primary_press_never_arms_anything() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: bare.widget.trigger.center(),
            button: PointerButton::Secondary,
        });
        assert_eq!(bare.dispatch(&secondary), EventResult::Ignored);
        assert!(bare.widget.armed.is_none());
    }

    #[test]
    fn a_cancel_disarms_without_reporting() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        let trigger = bare.widget.trigger.center();
        bare.dispatch(&pointer(PointerPhase::Down, trigger));
        assert_eq!(bare.widget.armed, Some(Target::Trigger));
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Cancel, trigger)),
            EventResult::Handled
        );
        assert!(bare.widget.armed.is_none());
        assert!(bare.state.statuses.is_empty());
    }

    #[test]
    fn the_trigger_shrinks_while_it_is_held() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.settle();
        let resting = bare.paint_at(10_001.0).shell().1;
        bare.dispatch(&pointer(PointerPhase::Down, bare.widget.trigger.center()));
        bare.paint_at(10_002.0);
        let pressed = bare.paint_at(10_002.0 + VIEW_EXIT_MS as f64).shell().1;
        assert!(
            pressed.width < resting.width,
            "the trigger shrinks: {resting:?} -> {pressed:?}"
        );
        assert!((pressed.width / resting.width - TRIGGER_PRESS_SCALE).abs() < 1e-6);
    }

    // ---- Submission ----------------------------------------------------------

    #[test]
    fn submitting_is_refused_until_the_message_holds_something() {
        let empty = view(FeedbackStatus::Open, "   ");
        assert!(!empty.can_submit(), "whitespace is not a message");
        let filled = view(FeedbackStatus::Open, "It crashed");
        assert!(filled.can_submit());
        let sending = view(FeedbackStatus::Sending, "It crashed");
        assert!(!sending.can_submit(), "a send in flight is not resubmitted");
    }

    #[test]
    fn reduced_motion_lands_the_morph_at_once() {
        let mut bare = Bare::new(FeedbackStatus::Idle);
        bare.theme.motion.reduce_motion = true;
        bare.rebuild(view(FeedbackStatus::Open, ""));
        bare.paint_at(0.0);
        let shell = bare.widget.shell_rect();
        assert!((shell.width() - bare.widget.panel.width()).abs() < 1e-6);
        assert!((shell.height() - bare.widget.panel.height()).abs() < 1e-6);
    }

    #[test]
    fn the_status_axis_answers_its_own_questions() {
        assert!(!FeedbackStatus::Idle.is_open());
        for status in [
            FeedbackStatus::Open,
            FeedbackStatus::Sending,
            FeedbackStatus::Sent,
            FeedbackStatus::Error,
        ] {
            assert!(status.is_open());
        }
        assert!(FeedbackStatus::Sending.is_busy());
        assert!(!FeedbackStatus::Open.is_busy());
        assert_eq!(FeedbackStatus::Sending.view(), PanelView::Form);
        assert_eq!(FeedbackStatus::Sent.view(), PanelView::Sent);
        assert_eq!(FeedbackStatus::Error.view(), PanelView::Error);
    }
}
