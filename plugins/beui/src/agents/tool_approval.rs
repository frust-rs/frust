//! Ports beUI's `tool-approval` agent-interface part.
//!
//! **Source:** `components/agents/tool-approval.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | card `rounded-2xl border border-border/60 bg-muted/20 p-4` | [`TOOL_APPROVAL_RADIUS`], [`TOOL_APPROVAL_PADDING`], [`TOOL_APPROVAL_SURFACE_ALPHA`] |
//! | badge `size-8 rounded-xl border bg-background` | [`TOOL_APPROVAL_BADGE_BOX`], [`TOOL_APPROVAL_BADGE_RADIUS`] |
//! | shield / spinner / alert / cross / check | [`ToolApprovalStatus`]'s glyph |
//! | title `font-medium text-foreground` | the title run |
//! | tool `font-mono text-xs text-muted-foreground` | the mono tool run |
//! | status chip `rounded-full border px-2 py-0.5 text-[11px]` | [`TOOL_APPROVAL_CHIP_HEIGHT`], [`ToolApprovalStatus::label`] |
//! | `description` paragraph `leading-5` | a wrapped run |
//! | `View details` + `rotate: open ? 180 : 0` | [`ToolApprovalView::open`], the reveal-driven chevron |
//! | `<dl>` `grid-cols-[7rem_1fr]` `rounded-xl border bg-background/70 p-3` | [`TOOL_APPROVAL_LABEL_COLUMN`], the parameter panel |
//! | `AnimatePresence` around the pending action row | one lane on [`TOOL_APPROVAL_ACTIONS_ENTER`] / [`TOOL_APPROVAL_ACTIONS_EXIT`] |
//! | `Allow once` / `Always allow` / `Deny` | [`ToolApprovalDecision`] |
//!
//! # One decision per consent episode, on one predicate
//!
//! The three buttons are the whole point of the component, so the firing rule
//! is stated rather than left to the press machinery: a decision fires
//! **exactly once** per consent episode. The press itself is up-inside like
//! every control in this catalog, and on top of that the widget latches that a
//! decision has been reported and refuses the next one until a new episode
//! opens.
//!
//! What opens a new episode is **one predicate**, and it governs every piece
//! of episode state together — the in-flight `Down` capture, the roving
//! keyboard focus and the one-decision latch are cleared as a unit, never
//! separately. An episode is new when the status changes *or* when any
//! consent-bearing value changes: `tool`, `title`, `description`,
//! `parameters`, [`id`](ToolApprovalView::id), or the offered button set
//! ([`always_allow`](ToolApprovalView::always_allow)) — everything
//! agent/server-supplied that a decision is actually consent *for*.
//!
//! Both halves follow from that single rule. A payload swap delivered between
//! `Down` and `Up` disarms the press rather than letting the release fire
//! against whatever is now displayed (mirrors
//! [`citations`](super::citations)'s own item-list invalidation). And a fresh
//! request swapped in under an unchanged `Pending` status — an app advancing
//! a queue without cycling the status through a non-pending value — releases
//! the latch too, so it is answerable rather than displayed-as-pending but
//! permanently dead.
//!
//! # Correlating a decision with the request it answers
//!
//! [`ToolApprovalView::on_decision_with_id`] hands the decision callback the
//! identity of the request actually shown: `Some(id)` when
//! [`ToolApprovalView::id`] was set, `None` when it was not. There is
//! deliberately **no** derived fallback — `tool` is not an identity (two
//! concurrent `shell.exec` approvals would report the same string), and
//! correlating on a colliding value can route consent granted for one call to
//! another. **Routing consent without an explicit `id` is unsafe whenever
//! more than one request can be open**: an app that can have several in
//! flight must set [`id`](ToolApprovalView::id) per request, and must treat a
//! `None` identity as "this decision cannot be correlated" rather than as a
//! request name.
//!
//! The action row itself is staged by a lane carrying **two** ramps
//! ([`TOOL_APPROVAL_ACTIONS_ENTER`] in, the faster
//! [`TOOL_APPROVAL_ACTIONS_EXIT`] out): it plays a real exit when the status
//! stops being `Pending`, rather than vanishing on the rebuild. A lane rather
//! than the catalog's presence driver because a lane **rests where it is
//! built** — a card mounted already-pending shows its row without playing an
//! entrance nobody asked for, which is upstream's `initial={false}`; nothing
//! here unmounts, so the driver's exit-completion latch would have no owner to
//! report to.
//!
//! # Degradations against the web original
//!
//! - **Parameter values are text, not code blocks.** Upstream's
//!   `ToolApprovalCode` renders a value through shiki inside the `<dd>`. Here a
//!   value is one wrapped mono run in the catalog's code ink; the shiki
//!   degradation the [code block](super::code_block) records applies unchanged.
//! - **Values wrap, they do not scroll.** Upstream wraps too
//!   (`whitespace-pre-wrap break-words`, deliberately, since a narrow grid
//!   column has nowhere to scroll on touch), so this matches — but a very long
//!   single token is clipped rather than broken mid-word. The clip is never
//!   silent: an overflowing parameter *label* or *value* paints a trailing `…`
//!   cue rather than eliding consent-bearing content with no sign anything was
//!   cut.
//! - **Head text is bounded, and can never reach the status chip.** `title`
//!   and `tool` are agent/server-supplied too, so they are wrapped into a
//!   column that reserves the chip's width out of its own
//!   ([`ToolApprovalWidget`]'s head column) and painted under a clip of
//!   exactly that column. Agent glyphs therefore cannot overdraw — or deface —
//!   the `Approval required`/`Failed` indicator the decision is read against,
//!   and an unbreakable run that still overflows paints the same trailing `…`
//!   cue the parameter rows do.
//! - **The `open` disclosure is controlled.** Upstream keeps an internal
//!   `defaultOpen` and closes itself when the status leaves `pending`; this port
//!   reports the requested value and lets the app decide, the catalog's rule.

use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget,
    erase_callback_arg,
    text::{FontWeight, TextContext, TextLayout, TextStyle},
};

use crate::agents::code_block::{
    CODE_TEXT_SIZE, code_palette, code_style, draw_check, draw_chevron, draw_cross, draw_spinner,
    spinner_angle,
};
use crate::motion::Ramp;
use crate::press::{Lane, draw_focus_ring, inside, is_activation_key, presses};
use crate::style::{self, with_alpha};
use crate::text::{LabelRun, SHAPING_INK};
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The card's corner radius (`rounded-2xl`).
pub const TOOL_APPROVAL_RADIUS: f64 = style::RADIUS_2XL;

/// The card's padding, in logical px (`p-4`).
pub const TOOL_APPROVAL_PADDING: f64 = 16.0;

/// The gap between the badge and the content column, in logical px (`gap-3`).
pub const TOOL_APPROVAL_GAP: f64 = 12.0;

/// The status badge's box, in logical px (`size-8`).
pub const TOOL_APPROVAL_BADGE_BOX: f64 = 32.0;

/// The status badge's corner radius (`rounded-xl`).
pub const TOOL_APPROVAL_BADGE_RADIUS: f64 = style::RADIUS_XL;

/// Alpha the card surface is filled at (`bg-muted/20`).
pub const TOOL_APPROVAL_SURFACE_ALPHA: f32 = 0.2;

/// Alpha the card's hairline is painted at (`border-border/60`).
pub const TOOL_APPROVAL_BORDER_ALPHA: f32 = 0.6;

/// The title's type size, in logical px (`text-sm font-medium`).
pub const TOOL_APPROVAL_TITLE_SIZE: f64 = style::TEXT_SM;

/// The status chip's type size, in logical px (`text-[11px]`).
pub const TOOL_APPROVAL_CHIP_SIZE: f64 = 11.0;

/// The status chip's height, in logical px (`py-0.5` on `text-[11px]`).
pub const TOOL_APPROVAL_CHIP_HEIGHT: f64 = 20.0;

/// The status chip's horizontal padding, in logical px (`px-2`).
pub const TOOL_APPROVAL_CHIP_PADDING_X: f64 = 8.0;

/// Alpha the status chip's fill is painted at (`bg-<hue>-500/10`).
pub const TOOL_APPROVAL_CHIP_FILL_ALPHA: f32 = 0.1;

/// Alpha the status chip's hairline is painted at (`border-<hue>-500/30`).
pub const TOOL_APPROVAL_CHIP_BORDER_ALPHA: f32 = 0.3;

/// The description's line height, in logical px (`leading-5`).
pub const TOOL_APPROVAL_LINE_HEIGHT: f64 = 20.0;

/// The `View details` trigger's height, in logical px.
pub const TOOL_APPROVAL_DETAILS_HEIGHT: f64 = 20.0;

/// The parameter panel's padding, in logical px (`p-3`).
pub const TOOL_APPROVAL_PANEL_PADDING: f64 = 12.0;

/// The parameter panel's corner radius (`rounded-xl`).
pub const TOOL_APPROVAL_PANEL_RADIUS: f64 = style::RADIUS_XL;

/// The parameter label column's width, in logical px (`minmax(0,7rem)`).
pub const TOOL_APPROVAL_LABEL_COLUMN: f64 = 112.0;

/// The gap between a parameter's label and its value, in logical px (`gap-3`).
pub const TOOL_APPROVAL_PARAM_GAP: f64 = 12.0;

/// The gap between parameter rows, in logical px (`gap-2`).
pub const TOOL_APPROVAL_PARAM_ROW_GAP: f64 = 8.0;

/// The action row's height, in logical px (`py-3` around a `py-1.5` control).
pub const TOOL_APPROVAL_ACTIONS_HEIGHT: f64 = 52.0;

/// One action button's height, in logical px (`py-1.5` on `text-xs`).
pub const TOOL_APPROVAL_BUTTON_HEIGHT: f64 = 28.0;

/// An action button's corner radius (`rounded-xl`).
pub const TOOL_APPROVAL_BUTTON_RADIUS: f64 = style::RADIUS_XL;

/// The ramp the parameter disclosure plays, in both directions.
pub const TOOL_APPROVAL_REVEAL: Ramp = Ramp::spring(SPRING_PANEL);

/// The ramp the action row enters on (`duration: 0.22, ease: EASE_OUT`).
pub const TOOL_APPROVAL_ACTIONS_ENTER: Ramp = Ramp::eased(Duration::from_millis(220), EASE_OUT);

/// The ramp the action row leaves on — faster than it arrived, the catalog's
/// standing asymmetry.
pub const TOOL_APPROVAL_ACTIONS_EXIT: Ramp = Ramp::eased(Duration::from_millis(140), EASE_OUT);

/// Where a tool call is in its approval lifecycle (`ToolApprovalStatus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolApprovalStatus {
    /// `"pending"` — waiting on the user. The only status with an action row.
    #[default]
    Pending,
    /// `"approving"` — the decision is being submitted.
    Approving,
    /// `"approved"` — allowed.
    Approved,
    /// `"denied"` — refused.
    Denied,
    /// `"running"` — the tool is executing.
    Running,
    /// `"complete"` — the tool finished.
    Complete,
    /// `"error"` — the tool failed.
    Error,
}

impl ToolApprovalStatus {
    /// The chip's word (`getStatusCopy`).
    pub const fn label(self) -> &'static str {
        match self {
            ToolApprovalStatus::Pending => "Approval required",
            ToolApprovalStatus::Approving => "Approving",
            ToolApprovalStatus::Approved => "Approved",
            ToolApprovalStatus::Denied => "Denied",
            ToolApprovalStatus::Running => "Running",
            ToolApprovalStatus::Complete => "Completed",
            ToolApprovalStatus::Error => "Failed",
        }
    }

    /// Whether the card is waiting on the user — the one status that shows the
    /// action row.
    pub const fn is_pending(self) -> bool {
        matches!(self, ToolApprovalStatus::Pending)
    }

    /// Whether work is in flight (`aria-busy`).
    pub const fn is_busy(self) -> bool {
        matches!(
            self,
            ToolApprovalStatus::Approving | ToolApprovalStatus::Running
        )
    }
}

/// What the user decided (`onApprove` / `onAlwaysAllow` / `onDeny`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolApprovalDecision {
    /// Allow this one call.
    AllowOnce,
    /// Allow this call and remember the grant.
    AlwaysAllow,
    /// Refuse the call.
    Deny,
}

impl ToolApprovalDecision {
    /// The button's label.
    pub const fn label(self) -> &'static str {
        match self {
            ToolApprovalDecision::AllowOnce => "Allow once",
            ToolApprovalDecision::AlwaysAllow => "Always allow",
            ToolApprovalDecision::Deny => "Deny",
        }
    }
}

/// One argument the tool would be called with (`ToolApprovalParameter`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolApprovalParameter {
    /// The argument's name.
    pub label: String,
    /// Its value, rendered in the code ink.
    pub value: String,
}

/// Build one parameter row.
pub fn tool_approval_parameter(
    label: impl Into<String>,
    value: impl Into<String>,
) -> ToolApprovalParameter {
    ToolApprovalParameter {
        label: label.into(),
        value: value.into(),
    }
}

/// A cached, lazily-shaped **wrapped** run.
///
/// The sibling `crate::text` runs shape with no width bound, which is right for
/// a control label and wrong for a paragraph: the description and the parameter
/// values here are prose in a fixed column. This carries the same
/// shape-once-then-reuse contract with `max_width` folded into the cache key,
/// since a width change re-runs line breaking.
pub(crate) struct WrappedRun {
    content: String,
    layout: Option<TextLayout>,
    shaped: Option<(TextStyle, u32)>,
    /// Whether the last [`layout`](Self::layout) pass produced a line wider
    /// than the `max_width` it was given — an unbreakable single token (no
    /// space to wrap on) that a caller's clip cuts off rather than fully
    /// showing. See [`Self::overflowed`].
    overflowed: bool,
}

impl WrappedRun {
    /// A run holding `content`, unshaped until the first
    /// [`layout`](Self::layout).
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped: None,
            overflowed: false,
        }
    }

    /// Shape (or reuse) the run in `style`, broken at `max_width`, and return
    /// its measured size.
    pub(crate) fn layout(
        &mut self,
        ctx: &mut LayoutCtx,
        style: &TextStyle,
        max_width: f64,
    ) -> Size {
        let width = max_width.max(0.0) as f32;
        let key = (style.clone(), width.to_bits());
        if let Some(cached) = &self.layout
            && self.shaped.as_ref() == Some(&key)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, Some(width));
        let size = laid.size();
        // parley lets an unbreakable run (no space to wrap on) overflow the
        // bound rather than force-break it, so a line wider than `width` here
        // is exactly the "clipped rather than broken mid-word" case this
        // module's docs record — see [`Self::overflowed`]'s callers.
        self.overflowed = size.width > f64::from(width) + 0.5;
        self.layout = Some(laid);
        self.shaped = Some(key);
        size
    }

    /// Whether the last shaped layout overflowed its `max_width` — an
    /// unbreakable run with no space to wrap on. A caller that clips this run
    /// (the head column, the parameter panel) should paint a visible
    /// truncation cue instead of silently cutting it off, since the run is
    /// consent-bearing content.
    pub(crate) fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// The measured size of the last shaped layout (`ZERO` before the first
    /// [`layout`](Self::layout)).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding the shaping ink.
    pub(crate) fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else { return };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

/// The prose style a description and a parameter label are shaped with.
pub(crate) fn prose_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::sans_family(),
        ..TextStyle::new(size as f32, SHAPING_INK)
    }
}

/// The style a title or a control label is shaped with.
pub(crate) fn strong_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::sans_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(size as f32, SHAPING_INK)
    }
}

/// A view-held one-argument callback, erased on build.
type OnArg<State, A> = Rc<dyn Fn(&mut State, A)>;

/// A declarative beUI tool-approval card. See the [module docs](self).
pub struct ToolApprovalView<State: 'static> {
    tool: String,
    title: String,
    description: Option<String>,
    parameters: Vec<ToolApprovalParameter>,
    status: ToolApprovalStatus,
    open: bool,
    always_allow: bool,
    /// An explicit request identity (see [`Self::id`]). `None` when the
    /// caller set none — there is no derived fallback.
    id: Option<String>,
    on_decision: Option<OnArg<State, ToolApprovalDecision>>,
    on_decision_with_id: Option<OnArg<State, (ToolApprovalDecision, Option<String>)>>,
    on_open_change: Option<OnArg<State, bool>>,
}

/// Create an approval card for `tool`, pending by default.
///
/// **Controlled**: chain [`ToolApprovalView::status`] to drive the lifecycle and
/// [`ToolApprovalView::on_decision`] to hear the user's answer.
pub fn tool_approval<State: 'static>(tool: impl Into<String>) -> ToolApprovalView<State> {
    ToolApprovalView {
        tool: tool.into(),
        title: "Allow this tool to run?".to_owned(),
        description: None,
        parameters: Vec::new(),
        status: ToolApprovalStatus::default(),
        open: false,
        always_allow: true,
        id: None,
        on_decision: None,
        on_decision_with_id: None,
        on_open_change: None,
    }
}

impl<State: 'static> ToolApprovalView<State> {
    /// The question the card asks (`title`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// The paragraph under the question (`description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The arguments the details disclosure lists (`parameters`).
    pub fn parameters(mut self, parameters: impl Into<Vec<ToolApprovalParameter>>) -> Self {
        self.parameters = parameters.into();
        self
    }

    /// Where the call is in its lifecycle (`status`).
    pub fn status(mut self, status: ToolApprovalStatus) -> Self {
        self.status = status;
        self
    }

    /// Whether the parameter details are disclosed (`open`).
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Whether the *Always allow* button is offered (`onAlwaysAllow`).
    pub fn always_allow(mut self, always_allow: bool) -> Self {
        self.always_allow = always_allow;
        self
    }

    /// An explicit identity for this request. Reported to
    /// [`Self::on_decision_with_id`] as `Some(id)`; a card that never sets one
    /// reports `None`, since nothing else here is unique per request. See the
    /// [module docs](self)'s "Correlating a decision with the request it
    /// answers" section — an app that can have more than one request open at
    /// once must set this.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Report the user's decision. Fires at most once per pending episode — see
    /// the [module docs](self).
    pub fn on_decision<F: Fn(&mut State, ToolApprovalDecision) + 'static>(
        mut self,
        on_decision: F,
    ) -> Self {
        self.on_decision = Some(Rc::new(on_decision));
        self
    }

    /// Like [`Self::on_decision`], but also reports the identity of the
    /// request actually shown when the decision fired: `Some(id)` when
    /// [`Self::id`] was set, `None` when it was not — there is no derived
    /// fallback, because a `None` the caller can see is safer than a
    /// colliding one it cannot (see the [module docs](self)). Additive, so an
    /// existing `on_decision` call site keeps compiling unchanged. Both may be
    /// set; both fire.
    pub fn on_decision_with_id<
        F: Fn(&mut State, ToolApprovalDecision, Option<String>) + 'static,
    >(
        mut self,
        on_decision: F,
    ) -> Self {
        self.on_decision_with_id = Some(Rc::new(
            move |state: &mut State, (decision, id): (ToolApprovalDecision, Option<String>)| {
                on_decision(state, decision, id);
            },
        ));
        self
    }

    /// Report the disclosure a press on *View details* asks for.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_change: F) -> Self {
        self.on_open_change = Some(Rc::new(on_change));
        self
    }

    /// The decisions this card offers, in visual order.
    fn decisions(&self) -> Vec<ToolApprovalDecision> {
        let mut decisions = vec![ToolApprovalDecision::AllowOnce];
        if self.always_allow {
            decisions.push(ToolApprovalDecision::AlwaysAllow);
        }
        decisions.push(ToolApprovalDecision::Deny);
        decisions
    }
}

/// The truncation cue glyph a clipped consent-bearing run paints (`…`) —
/// shared with the sibling [approval card](super::approval_card), which
/// bounds its own head text the same way.
pub(crate) const TRUNCATION_MARKER: char = '\u{2026}';

/// One retained parameter row.
struct ParamRuns {
    label: WrappedRun,
    value: WrappedRun,
    height: f64,
    /// Whether `value`'s last shaped layout overflowed its column (see
    /// [`WrappedRun::overflowed`]) — the row paints `marker` when set.
    truncated: bool,
    /// The same, for the narrower label column: a parameter's *name* is
    /// consent-bearing too, so its cut is marked rather than silent.
    label_truncated: bool,
    /// The value column's truncation cue, laid out once and painted only when
    /// `truncated`.
    marker: LabelRun,
    /// The label column's truncation cue — its own run because the two
    /// columns are shaped in different styles.
    label_marker: LabelRun,
}

/// Which affordance the roving cursor sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApprovalTarget {
    /// The *View details* trigger.
    Details,
    /// One of the three decision buttons.
    Decision(ToolApprovalDecision),
}

/// The retained widget for a [`ToolApprovalView`].
pub struct ToolApprovalWidget {
    /// Wrapped, not free: the head runs are agent-supplied, so they are bound
    /// to [`Self::head_column_width`] and clipped to it in paint.
    tool: WrappedRun,
    tool_text: String,
    title: WrappedRun,
    title_text: String,
    /// Whether the head runs overflowed their bounded column and paint the
    /// truncation cue (see [`WrappedRun::overflowed`]).
    title_truncated: bool,
    tool_truncated: bool,
    /// The head column's truncation cue, shared by both head runs — only one
    /// of them is painted per line, and both shape in the same cue glyph.
    head_marker: LabelRun,
    description: Option<WrappedRun>,
    description_text: Option<String>,
    details_label: LabelRun,
    chip: LabelRun,
    parameters: Vec<ToolApprovalParameter>,
    params: Vec<ParamRuns>,
    buttons: Vec<(ToolApprovalDecision, LabelRun)>,
    status: ToolApprovalStatus,
    open: bool,
    always_allow: bool,
    /// The request identity a decision reports: [`ToolApprovalView::id`] when
    /// the caller set one, `None` otherwise. Never derived from `tool` — see
    /// the [module docs](self)'s correlation section.
    id: Option<String>,
    /// The parameter disclosure.
    reveal: Lane,
    /// The action row's enter/exit staging, on its two ramps.
    actions: Lane,
    /// Raised once a decision has been reported for the current consent
    /// episode; cleared with `captured` and `focused` whenever a new episode
    /// opens (see the [module docs](self)).
    decided: bool,
    width: f64,
    /// The measured heights the last layout pass resolved.
    title_height: f64,
    description_height: f64,
    params_height: f64,
    body_height: f64,
    focused: ApprovalTarget,
    hovered: Option<ApprovalTarget>,
    captured: Option<ApprovalTarget>,
    on_decision: Option<ErasedArgCallback<ToolApprovalDecision>>,
    on_decision_with_id: Option<ErasedArgCallback<(ToolApprovalDecision, Option<String>)>>,
    on_open_change: Option<ErasedArgCallback<bool>>,
}

impl<State: 'static> View<State> for ToolApprovalView<State> {
    type Element = ToolApprovalWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ToolApprovalWidget {
        ToolApprovalWidget {
            tool: WrappedRun::new(self.tool.clone()),
            tool_text: self.tool.clone(),
            title: WrappedRun::new(self.title.clone()),
            title_text: self.title.clone(),
            title_truncated: false,
            tool_truncated: false,
            head_marker: LabelRun::new(TRUNCATION_MARKER),
            description: self.description.as_deref().map(WrappedRun::new),
            description_text: self.description.clone(),
            details_label: LabelRun::new("View details"),
            chip: LabelRun::new(self.status.label()),
            params: build_params(&self.parameters),
            parameters: self.parameters.clone(),
            buttons: self
                .decisions()
                .into_iter()
                .map(|decision| (decision, LabelRun::new(decision.label())))
                .collect(),
            status: self.status,
            open: self.open,
            always_allow: self.always_allow,
            id: self.id.clone(),
            reveal: Lane::at_rest(TOOL_APPROVAL_REVEAL, if self.open { 1.0 } else { 0.0 }),
            // Rests where it is built: an already-pending card shows its row
            // without playing an entrance.
            actions: Lane::at_rest(
                TOOL_APPROVAL_ACTIONS_ENTER,
                if self.status.is_pending() { 1.0 } else { 0.0 },
            ),
            decided: false,
            width: 0.0,
            title_height: 0.0,
            description_height: 0.0,
            params_height: 0.0,
            body_height: 0.0,
            // The safe default: keyboard activation with no explicit
            // selection lands on the least-permissive decision, never one
            // that grants anything.
            focused: ApprovalTarget::Decision(ToolApprovalDecision::Deny),
            hovered: None,
            captured: None,
            on_decision: self.on_decision.as_ref().map(erase_callback_arg),
            on_decision_with_id: self.on_decision_with_id.as_ref().map(erase_callback_arg),
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ToolApprovalWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_decision = self.on_decision.as_ref().map(erase_callback_arg);
        element.on_decision_with_id = self.on_decision_with_id.as_ref().map(erase_callback_arg);
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;
        // The one predicate: whether this rebuild opens a new consent episode.
        // Every consent-bearing value feeds it, the status included, and it
        // resets `captured`, `focused` and `decided` together at the end —
        // see the [module docs](self).
        let mut new_episode = false;

        if element.tool_text != self.tool {
            element.tool = WrappedRun::new(self.tool.clone());
            element.tool_text = self.tool.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if element.title_text != self.title {
            element.title = WrappedRun::new(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if element.description_text != self.description {
            element.description = self.description.as_deref().map(WrappedRun::new);
            element.description_text = self.description.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if element.parameters != self.parameters {
            element.params = build_params(&self.parameters);
            element.parameters = self.parameters.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if element.id != self.id {
            element.id = self.id.clone();
            new_episode = true;
        }
        if element.always_allow != self.always_allow {
            element.always_allow = self.always_allow;
            element.buttons = self
                .decisions()
                .into_iter()
                .map(|decision| (decision, LabelRun::new(decision.label())))
                .collect();
            // What the buttons grant changed, so this is a new episode by the
            // same rule a swapped payload is — an in-flight press is disarmed
            // rather than released against a set it was not aimed at.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if element.status != self.status {
            element.status = self.status;
            element.chip = LabelRun::new(self.status.label());
            if self.status.is_pending() {
                element
                    .actions
                    .retarget_with(TOOL_APPROVAL_ACTIONS_ENTER, 1.0);
            } else {
                element
                    .actions
                    .retarget_with(TOOL_APPROVAL_ACTIONS_EXIT, 0.0);
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if new_episode {
            // One episode, one reset. Whatever `Down` armed is disarmed (the
            // next `Up` is a no-op wherever it lands), keyboard focus re-arms
            // at the safe default, and the one-decision latch is released so
            // the request now displayed can actually be answered.
            element.captured = None;
            element.focused = ApprovalTarget::Decision(ToolApprovalDecision::Deny);
            element.decided = false;
            flags |= ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Retargeted, never restarted: a rebuild re-passing the flag the lane is
        // already flying toward leaves its clock alone.
        element
            .reveal
            .retarget(if element.open { 1.0 } else { 0.0 });
        flags
    }
}

/// Build the shape-cache carriers for every parameter row.
fn build_params(parameters: &[ToolApprovalParameter]) -> Vec<ParamRuns> {
    parameters
        .iter()
        .map(|parameter| ParamRuns {
            label: WrappedRun::new(parameter.label.clone()),
            value: WrappedRun::new(parameter.value.clone()),
            height: 0.0,
            truncated: false,
            label_truncated: false,
            marker: LabelRun::new(TRUNCATION_MARKER),
            label_marker: LabelRun::new(TRUNCATION_MARKER),
        })
        .collect()
}

/// Paint the shield glyph the pending badge carries (`ShieldCheck`).
fn draw_shield(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let unit = box_size / 16.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(unit * 8.0, unit * 2.0));
    path.line_to(Point::new(unit * 13.0, unit * 4.0));
    path.line_to(Point::new(unit * 13.0, unit * 8.5));
    path.line_to(Point::new(unit * 8.0, unit * 14.0));
    path.line_to(Point::new(unit * 3.0, unit * 8.5));
    path.line_to(Point::new(unit * 3.0, unit * 4.0));
    path.close_path();
    scene.stroke_path(origin, &path, 1.3, &Brush::Solid(color));
}

/// Paint the alert glyph an errored badge carries (`CircleAlert`).
fn draw_alert(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let inset = box_size * 0.08;
    let rect = Rect::new(inset, inset, box_size - inset, box_size - inset);
    let circle = RoundedRect::from_rect(rect, (box_size - inset * 2.0) / 2.0);
    scene.stroke_path(
        origin,
        &Shape::to_path(&circle, style::PATH_TOLERANCE),
        1.3,
        &Brush::Solid(color),
    );
    let mut bar = BezPath::new();
    bar.move_to(Point::new(box_size * 0.5, box_size * 0.28));
    bar.line_to(Point::new(box_size * 0.5, box_size * 0.56));
    bar.move_to(Point::new(box_size * 0.5, box_size * 0.68));
    bar.line_to(Point::new(box_size * 0.5, box_size * 0.74));
    scene.stroke_path(origin, &bar, 1.4, &Brush::Solid(color));
}

impl ToolApprovalWidget {
    /// The content column's width.
    fn column_width(&self) -> f64 {
        (self.width - TOOL_APPROVAL_PADDING * 2.0 - TOOL_APPROVAL_BADGE_BOX - TOOL_APPROVAL_GAP)
            .max(0.0)
    }

    /// The content column's left edge, in widget-local space.
    fn column_x(&self) -> f64 {
        TOOL_APPROVAL_PADDING + TOOL_APPROVAL_BADGE_BOX + TOOL_APPROVAL_GAP
    }

    /// The status chip's painted width, from the last shaped chip label.
    fn chip_width(&self) -> f64 {
        self.chip.size().width + TOOL_APPROVAL_CHIP_PADDING_X * 2.0
    }

    /// The head column's width: the content column with the status chip's own
    /// box and the gap before it reserved out of it. Head text is
    /// agent-supplied, so it is shaped *and* clipped to this — the chip's
    /// space is not the head's to paint into.
    fn head_column_width(&self) -> f64 {
        (self.column_width() - self.chip_width() - style::GAP_MD).max(0.0)
    }

    /// Paint the head column's truncation cue at the right edge of `run`'s
    /// last line — the visible sign that consent-bearing head text was cut,
    /// the same contract a clipped parameter row carries.
    fn paint_head_marker(
        &self,
        scene: &mut dyn PaintScene,
        at: Point,
        run: Size,
        column: f64,
        ink: Color,
    ) {
        let marker = self.head_marker.size();
        self.head_marker.paint(
            Point::new(
                at.x + (column - marker.width).max(0.0),
                at.y + (run.height - marker.height).max(0.0),
            ),
            ink,
            scene,
        );
    }

    /// Whether the details disclosure exists at all.
    fn has_details(&self) -> bool {
        !self.parameters.is_empty()
    }

    /// The parameter panel's natural (fully-disclosed) height.
    fn params_natural(&self) -> f64 {
        if !self.has_details() {
            return 0.0;
        }
        let rows: f64 = self.params.iter().map(|row| row.height).sum();
        let gaps = TOOL_APPROVAL_PARAM_ROW_GAP * (self.params.len().saturating_sub(1)) as f64;
        rows + gaps + TOOL_APPROVAL_PANEL_PADDING * 2.0
    }

    /// The details block's height right now, tracking the reveal.
    fn params_band(&self) -> f64 {
        if !self.has_details() {
            return 0.0;
        }
        (self.reveal.value().clamp(0.0, 1.0) * (style::GAP_MD + self.params_natural())).max(0.0)
    }

    /// How present the action row is right now, `0` gone to `1` settled.
    fn actions_presence(&self) -> f64 {
        self.actions.value().clamp(0.0, 1.0)
    }

    /// The action row's height right now, tracking its presence.
    fn actions_height(&self) -> f64 {
        self.actions_presence() * TOOL_APPROVAL_ACTIONS_HEIGHT
    }

    /// The *View details* trigger's box, in widget-local space.
    fn details_rect(&self) -> Option<Rect> {
        if !self.has_details() {
            return None;
        }
        let mut y = TOOL_APPROVAL_PADDING + self.title_height;
        if self.description_height > 0.0 {
            y += style::GAP_MD + self.description_height;
        }
        y += style::GAP_MD;
        let width = self.details_label.size().width + style::GAP_SM + style::ICON_SIZE;
        Some(Rect::from_origin_size(
            Point::new(self.column_x(), y),
            Size::new(width, TOOL_APPROVAL_DETAILS_HEIGHT),
        ))
    }

    /// The parameter panel's box, in widget-local space.
    fn panel_rect(&self) -> Rect {
        let band = self.params_band();
        let top = TOOL_APPROVAL_PADDING + self.body_height - band + style::GAP_MD;
        Rect::from_origin_size(
            Point::new(self.column_x(), top),
            Size::new(self.column_width(), (band - style::GAP_MD).max(0.0)),
        )
    }

    /// The action row's box, in widget-local space.
    fn actions_rect(&self) -> Rect {
        let top = TOOL_APPROVAL_PADDING * 2.0 + self.body_height;
        Rect::from_origin_size(
            Point::new(0.0, top),
            Size::new(self.width, self.actions_height()),
        )
    }

    /// Decision `decision`'s button box, in widget-local space — `None` when it
    /// is not offered or the row has left.
    fn button_rect(&self, decision: ToolApprovalDecision) -> Option<Rect> {
        if self.actions_presence() <= 0.0 {
            return None;
        }
        let row = self.actions_rect();
        let mut x = row.x0 + TOOL_APPROVAL_PADDING;
        for (candidate, label) in &self.buttons {
            let width = label.size().width + style::PADDING_X_SM * 2.0;
            if *candidate == decision {
                return Some(Rect::from_origin_size(
                    Point::new(
                        x,
                        row.y0 + (TOOL_APPROVAL_ACTIONS_HEIGHT - TOOL_APPROVAL_BUTTON_HEIGHT) / 2.0,
                    ),
                    Size::new(width, TOOL_APPROVAL_BUTTON_HEIGHT),
                ));
            }
            x += width + style::GAP_MD;
        }
        None
    }

    /// The affordance under a widget-local `pos`, if any.
    fn hit(&self, pos: Point) -> Option<ApprovalTarget> {
        if self.status.is_pending() {
            for (decision, _) in &self.buttons {
                if let Some(rect) = self.button_rect(*decision)
                    && rect.contains(pos)
                {
                    return Some(ApprovalTarget::Decision(*decision));
                }
            }
        }
        if let Some(rect) = self.details_rect()
            && rect.contains(pos)
        {
            return Some(ApprovalTarget::Details);
        }
        None
    }

    /// The affordances the roving cursor can reach, in visual order.
    fn targets(&self) -> Vec<ApprovalTarget> {
        let mut targets = Vec::new();
        if self.has_details() {
            targets.push(ApprovalTarget::Details);
        }
        if self.status.is_pending() {
            targets.extend(
                self.buttons
                    .iter()
                    .map(|(decision, _)| ApprovalTarget::Decision(*decision)),
            );
        }
        targets
    }

    /// The next affordance `step` places from the focused one, wrapping.
    fn step_target(&self, step: isize) -> Option<ApprovalTarget> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let at = targets.iter().position(|t| *t == self.focused).unwrap_or(0);
        let next = (at as isize + step).rem_euclid(targets.len() as isize) as usize;
        targets.get(next).copied()
    }

    /// Report the decision `target` asks for.
    ///
    /// A decision fires at most once per pending episode — see the [module
    /// docs](self).
    fn activate(&mut self, ctx: &mut EventCtx, target: ApprovalTarget) {
        match target {
            ApprovalTarget::Details => {
                let next = !self.open;
                if let Some(on_change) = self.on_open_change.as_mut() {
                    on_change(ctx, next);
                }
            }
            ApprovalTarget::Decision(decision) => {
                if self.decided || !self.status.is_pending() {
                    return;
                }
                // Both callbacks, when set, report the same fired decision —
                // the plain one for an existing call site, the id-carrying
                // one for a caller that wants to know which request it
                // answers.
                let mut fired = false;
                if let Some(on_decision) = self.on_decision.as_mut() {
                    on_decision(ctx, decision);
                    fired = true;
                }
                if let Some(on_decision_with_id) = self.on_decision_with_id.as_mut() {
                    on_decision_with_id(ctx, (decision, self.id.clone()));
                    fired = true;
                }
                if fired {
                    self.decided = true;
                }
            }
        }
    }

    /// The hue `status` reads in.
    fn status_hue(status: ToolApprovalStatus, theme: Option<&Theme>) -> Color {
        let tokens = BeuiTokens::resolve(theme);
        let palette = code_palette(theme);
        match status {
            ToolApprovalStatus::Pending => tokens.warning,
            ToolApprovalStatus::Approving | ToolApprovalStatus::Running => palette.function,
            ToolApprovalStatus::Approved | ToolApprovalStatus::Complete => tokens.success,
            ToolApprovalStatus::Denied | ToolApprovalStatus::Error => {
                theme.map_or(BEUI_LIGHT.destructive, |t| t.scheme().error)
            }
        }
    }

    /// Paint the badge glyph for `status`.
    fn draw_badge_glyph(
        scene: &mut dyn PaintScene,
        status: ToolApprovalStatus,
        at: Point,
        box_size: f64,
        ink: Color,
        angle: f64,
    ) {
        match status {
            ToolApprovalStatus::Approving | ToolApprovalStatus::Running => {
                draw_spinner(scene, at, box_size, angle, ink);
            }
            ToolApprovalStatus::Error => draw_alert(scene, at, box_size, ink),
            ToolApprovalStatus::Denied => draw_cross(scene, at, box_size, ink),
            ToolApprovalStatus::Approved | ToolApprovalStatus::Complete => {
                draw_check(scene, at, box_size, ink);
            }
            ToolApprovalStatus::Pending => draw_shield(scene, at, box_size, ink),
        }
    }
}

impl Widget for ToolApprovalWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        let column = self.column_width();

        // The chip is shaped first because its box is reserved out of the head
        // column: the head is bounded by what is left, so it cannot be laid
        // into the status indicator's space in the first place.
        self.chip
            .layout(ctx, &strong_style(TOOL_APPROVAL_CHIP_SIZE));
        let head_column = self.head_column_width();
        let title_style = strong_style(TOOL_APPROVAL_TITLE_SIZE);
        let title_size = self.title.layout(ctx, &title_style, head_column);
        self.title_truncated = self.title.overflowed();
        let tool_size = self
            .tool
            .layout(ctx, &code_style(CODE_TEXT_SIZE), head_column);
        self.tool_truncated = self.tool.overflowed();
        if self.title_truncated || self.tool_truncated {
            self.head_marker.layout(ctx, &title_style);
        }
        self.title_height = (title_size.height + tool_size.height + style::spacing(0.5))
            .max(TOOL_APPROVAL_BADGE_BOX);
        self.details_label
            .layout(ctx, &strong_style(style::TEXT_XS));

        self.description_height = match &mut self.description {
            Some(run) => run.layout(ctx, &prose_style(style::TEXT_SM), column).height,
            None => 0.0,
        };

        let value_width = (column
            - TOOL_APPROVAL_PANEL_PADDING * 2.0
            - TOOL_APPROVAL_LABEL_COLUMN
            - TOOL_APPROVAL_PARAM_GAP)
            .max(0.0);
        let label_style = prose_style(style::TEXT_XS);
        let value_style = code_style(style::TEXT_XS);
        for row in &mut self.params {
            let label = row
                .label
                .layout(ctx, &label_style, TOOL_APPROVAL_LABEL_COLUMN);
            row.label_truncated = row.label.overflowed();
            if row.label_truncated {
                row.label_marker.layout(ctx, &label_style);
            }
            let value = row.value.layout(ctx, &value_style, value_width);
            row.truncated = row.value.overflowed();
            if row.truncated {
                row.marker.layout(ctx, &value_style);
            }
            row.height = label.height.max(value.height);
        }
        self.params_height = self.params_natural();

        for (_, label) in &mut self.buttons {
            label.layout(ctx, &strong_style(style::TEXT_XS));
        }

        let mut body = self.title_height;
        if self.description_height > 0.0 {
            body += style::GAP_MD + self.description_height;
        }
        if self.has_details() {
            body += style::GAP_MD + TOOL_APPROVAL_DETAILS_HEIGHT;
        }
        body += self.params_band();
        self.body_height = body;

        bc.constrain(Size::new(
            self.width,
            TOOL_APPROVAL_PADDING * 2.0 + body + self.actions_height(),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let hue = Self::status_hue(self.status, theme);
        let ring = with_alpha(
            BeuiTokens::resolve_ring(None, theme),
            style::FOCUS_RING_OPACITY,
        );
        let background = theme.map_or(BEUI_LIGHT.background, |t| t.scheme().surface);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let before = (self.reveal.value(), self.actions.value());
        let mut moving = false;
        if reduce_motion {
            self.reveal.snap();
            self.actions.snap();
        } else {
            moving |= self.reveal.advance(now);
            moving |= self.actions.advance(now);
        }
        let moving = moving || self.reveal.value() != before.0 || self.actions.value() != before.1;

        // ---- the card ---------------------------------------------------------
        scene.fill_rounded_rect(
            origin,
            size,
            TOOL_APPROVAL_RADIUS,
            with_alpha(palette.surface, TOOL_APPROVAL_SURFACE_ALPHA),
        );
        crate::components::popover::paint_panel_hairline(
            scene,
            origin,
            size,
            TOOL_APPROVAL_RADIUS,
            with_alpha(palette.comment, TOOL_APPROVAL_BORDER_ALPHA),
        );
        scene.push_clip_rounded(origin, size, TOOL_APPROVAL_RADIUS);

        // ---- the badge --------------------------------------------------------
        let badge_at = Point::new(
            origin.x + TOOL_APPROVAL_PADDING,
            origin.y + TOOL_APPROVAL_PADDING,
        );
        let badge_size = Size::new(TOOL_APPROVAL_BADGE_BOX, TOOL_APPROVAL_BADGE_BOX);
        scene.fill_rounded_rect(badge_at, badge_size, TOOL_APPROVAL_BADGE_RADIUS, background);
        crate::components::popover::paint_panel_hairline(
            scene,
            badge_at,
            badge_size,
            TOOL_APPROVAL_BADGE_RADIUS,
            with_alpha(palette.comment, TOOL_APPROVAL_BORDER_ALPHA),
        );
        let glyph_box = style::ICON_SIZE;
        Self::draw_badge_glyph(
            scene,
            self.status,
            Point::new(
                badge_at.x + (TOOL_APPROVAL_BADGE_BOX - glyph_box) / 2.0,
                badge_at.y + (TOOL_APPROVAL_BADGE_BOX - glyph_box) / 2.0,
            ),
            glyph_box,
            if self.status.is_pending() {
                palette.comment
            } else {
                hue
            },
            spinner_angle(now, reduce_motion),
        );
        if self.status.is_busy() && !reduce_motion {
            ctx.request_frame_paced();
        }

        // ---- the head ---------------------------------------------------------
        let column_x = origin.x + self.column_x();
        let chip_size = self.chip.size();
        let chip_width = chip_size.width + TOOL_APPROVAL_CHIP_PADDING_X * 2.0;
        let chip_at = Point::new(
            origin.x + size.width - TOOL_APPROVAL_PADDING - chip_width,
            origin.y + TOOL_APPROVAL_PADDING,
        );
        let chip_box = Size::new(chip_width, TOOL_APPROVAL_CHIP_HEIGHT);
        scene.fill_rounded_rect(
            chip_at,
            chip_box,
            TOOL_APPROVAL_CHIP_HEIGHT / 2.0,
            with_alpha(hue, TOOL_APPROVAL_CHIP_FILL_ALPHA),
        );
        crate::components::popover::paint_panel_hairline(
            scene,
            chip_at,
            chip_box,
            TOOL_APPROVAL_CHIP_HEIGHT / 2.0,
            with_alpha(hue, TOOL_APPROVAL_CHIP_BORDER_ALPHA),
        );
        self.chip.paint(
            Point::new(
                chip_at.x + TOOL_APPROVAL_CHIP_PADDING_X,
                chip_at.y + (TOOL_APPROVAL_CHIP_HEIGHT - chip_size.height) / 2.0,
            ),
            hue,
            scene,
        );

        // The head runs paint under a clip of exactly their reserved column,
        // so agent-supplied text can never reach the status chip painted just
        // above — and an unbreakable run that overran the column says so with
        // the same cue a clipped parameter carries.
        let head_column = self.head_column_width();
        let head_at = Point::new(column_x, origin.y + TOOL_APPROVAL_PADDING);
        scene.push_clip(head_at, Size::new(head_column, self.title_height));
        let title_size = self.title.size();
        self.title.paint(head_at, palette.plain, scene);
        let tool_at = Point::new(
            column_x,
            head_at.y + title_size.height + style::spacing(0.5),
        );
        self.tool.paint(tool_at, palette.comment, scene);
        if self.title_truncated {
            self.paint_head_marker(scene, head_at, title_size, head_column, palette.plain);
        }
        if self.tool_truncated {
            self.paint_head_marker(
                scene,
                tool_at,
                self.tool.size(),
                head_column,
                palette.comment,
            );
        }
        scene.pop_clip();

        let mut y = origin.y + TOOL_APPROVAL_PADDING + self.title_height;
        if let Some(description) = &self.description {
            y += style::GAP_MD;
            description.paint(Point::new(column_x, y), palette.comment, scene);
        }

        // ---- the details trigger and its panel --------------------------------
        if let Some(rect) = self.details_rect() {
            let at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let hovered = self.hovered == Some(ApprovalTarget::Details);
            let ink = if hovered {
                palette.plain
            } else {
                palette.comment
            };
            let measured = self.details_label.size();
            self.details_label.paint(
                Point::new(
                    at.x,
                    at.y + (TOOL_APPROVAL_DETAILS_HEIGHT - measured.height) / 2.0,
                ),
                ink,
                scene,
            );
            draw_chevron(
                scene,
                Point::new(
                    at.x + measured.width + style::GAP_SM + style::ICON_SIZE / 2.0,
                    at.y + TOOL_APPROVAL_DETAILS_HEIGHT / 2.0,
                ),
                style::ICON_SIZE,
                self.reveal.value().clamp(0.0, 1.0) * std::f64::consts::PI,
                ink,
            );
            if self.focused == ApprovalTarget::Details && ctx.has_focus() {
                draw_focus_ring(scene, at, rect.size(), style::RADIUS_MD, 0.0, ring);
            }
        }

        let band = self.params_band();
        if band > 0.5 {
            let panel = self.panel_rect();
            let at = Point::new(origin.x + panel.x0, origin.y + panel.y0);
            let panel_size = Size::new(panel.width(), panel.height());
            scene.fill_rounded_rect(
                at,
                panel_size,
                TOOL_APPROVAL_PANEL_RADIUS,
                with_alpha(background, 0.7),
            );
            crate::components::popover::paint_panel_hairline(
                scene,
                at,
                panel_size,
                TOOL_APPROVAL_PANEL_RADIUS,
                with_alpha(palette.comment, TOOL_APPROVAL_BORDER_ALPHA),
            );
            scene.push_clip_rounded(at, panel_size, TOOL_APPROVAL_PANEL_RADIUS);
            let mut row_y = at.y + TOOL_APPROVAL_PANEL_PADDING;
            for row in &self.params {
                row.label.paint(
                    Point::new(at.x + TOOL_APPROVAL_PANEL_PADDING, row_y),
                    palette.comment,
                    scene,
                );
                row.value.paint(
                    Point::new(
                        at.x + TOOL_APPROVAL_PANEL_PADDING
                            + TOOL_APPROVAL_LABEL_COLUMN
                            + TOOL_APPROVAL_PARAM_GAP,
                        row_y,
                    ),
                    palette.plain,
                    scene,
                );
                if row.label_truncated {
                    // A parameter's name is consent-bearing too, so its own
                    // cut is marked at the label column's right edge.
                    let marker_size = row.label_marker.size();
                    row.label_marker.paint(
                        Point::new(
                            at.x + TOOL_APPROVAL_PANEL_PADDING + TOOL_APPROVAL_LABEL_COLUMN
                                - marker_size.width,
                            row_y,
                        ),
                        palette.comment,
                        scene,
                    );
                }
                if row.truncated {
                    // The value's own unbreakable run overran the column and
                    // the panel clip above cuts it off — this is
                    // consent-bearing content, so the cut is marked rather
                    // than silently elided (module docs' "Values wrap, they
                    // do not scroll" degradation note).
                    let marker_size = row.marker.size();
                    row.marker.paint(
                        Point::new(
                            at.x + panel_size.width
                                - TOOL_APPROVAL_PANEL_PADDING
                                - marker_size.width,
                            row_y,
                        ),
                        palette.comment,
                        scene,
                    );
                }
                row_y += row.height + TOOL_APPROVAL_PARAM_ROW_GAP;
            }
            scene.pop_clip();
        }

        // ---- the action row ---------------------------------------------------
        let row_height = self.actions_height();
        if row_height > 0.5 {
            let row = self.actions_rect();
            let at = Point::new(origin.x + row.x0, origin.y + row.y0);
            scene.push_clip(at, Size::new(row.width(), row_height));
            let alpha = self.actions_presence() as f32;
            if alpha < 1.0 {
                scene.push_layer(at, Size::new(row.width(), row_height), alpha);
            }
            scene.fill_rect(
                at,
                Size::new(row.width(), style::BORDER_WIDTH),
                with_alpha(palette.comment, TOOL_APPROVAL_BORDER_ALPHA),
            );
            for (index, (decision, label)) in self.buttons.iter().enumerate() {
                let Some(rect) = self.button_rect(*decision) else {
                    continue;
                };
                let button_at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
                let hovered = self.hovered == Some(ApprovalTarget::Decision(*decision));
                let (fill, ink, hairline) = match index {
                    0 => (Some(palette.plain), background, None),
                    _ if *decision == ToolApprovalDecision::Deny => (
                        hovered.then(|| with_alpha(palette.plain, style::HOVER_WASH_ALPHA)),
                        palette.comment,
                        None,
                    ),
                    _ => (
                        Some(background),
                        palette.plain,
                        Some(with_alpha(palette.comment, TOOL_APPROVAL_BORDER_ALPHA)),
                    ),
                };
                if let Some(fill) = fill {
                    let fill = if hovered && index == 0 {
                        with_alpha(fill, fill.components[3] * style::HOVER_SOLID_ALPHA)
                    } else {
                        fill
                    };
                    scene.fill_rounded_rect(
                        button_at,
                        rect.size(),
                        TOOL_APPROVAL_BUTTON_RADIUS,
                        fill,
                    );
                }
                if let Some(hairline) = hairline {
                    crate::components::popover::paint_panel_hairline(
                        scene,
                        button_at,
                        rect.size(),
                        TOOL_APPROVAL_BUTTON_RADIUS,
                        hairline,
                    );
                }
                let measured = label.size();
                label.paint(
                    Point::new(
                        button_at.x + (rect.width() - measured.width) / 2.0,
                        button_at.y + (TOOL_APPROVAL_BUTTON_HEIGHT - measured.height) / 2.0,
                    ),
                    ink,
                    scene,
                );
                if self.focused == ApprovalTarget::Decision(*decision) && ctx.has_focus() {
                    draw_focus_ring(
                        scene,
                        button_at,
                        rect.size(),
                        TOOL_APPROVAL_BUTTON_RADIUS,
                        style::BORDER_WIDTH,
                        ring,
                    );
                }
            }
            if alpha < 1.0 {
                scene.pop_layer();
            }
            scene.pop_clip();
        }
        scene.pop_clip();

        // Both the reveal and the action row's presence feed the reported
        // height, so a bare frame request would let either freeze on the
        // intra-frame layout skip.
        if moving {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::ArrowDown) => Some(1),
                    Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowUp) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    let Some(next) = self.step_target(step) else {
                        return EventResult::Ignored;
                    };
                    self.focused = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) {
                    let target = self.focused;
                    if !self.targets().contains(&target) {
                        return EventResult::Ignored;
                    }
                    self.activate(ctx, target);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, ctx.size()) {
                        return EventResult::Ignored;
                    }
                    let Some(target) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(target);
                    self.focused = target;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_some() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = self.hit(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if self.hit(p.position) == Some(armed) {
                        self.activate(ctx, armed);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = format!(
            "{} ({}) — {}",
            self.title_text,
            self.tool_text,
            self.status.label()
        );
        ctx.push_node(Role::Group, |node| {
            node.set_label(label.as_str());
        });
        if self.has_details() {
            ctx.push_node(Role::Button, |node| {
                node.set_label("View details");
                node.set_expanded(self.open);
                node.add_action(Action::Click);
            });
        }
        if self.status.is_pending() {
            for (decision, _) in &self.buttons {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(decision.label());
                    if self.decided {
                        node.set_disabled();
                    } else {
                        node.add_action(Action::Click);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, CornerRadii, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rounded: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _r: f64, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, _r: CornerRadii, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, _r: f64) {
            self.clips.push((o, s));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Answered {
        decision: Option<ToolApprovalDecision>,
        decisions: u32,
        decision_id: Option<(ToolApprovalDecision, Option<String>)>,
        decision_id_calls: u32,
        open: Option<bool>,
        open_calls: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn params() -> Vec<ToolApprovalParameter> {
        vec![
            tool_approval_parameter("command", "rm -rf ./build"),
            tool_approval_parameter("cwd", "/home/agent/project"),
        ]
    }

    fn view(status: ToolApprovalStatus, open: bool) -> ToolApprovalView<Answered> {
        tool_approval::<Answered>("shell.exec")
            .title("Allow this tool to run?")
            .description("The agent wants to delete the build directory before rebuilding it.")
            .parameters(params())
            .status(status)
            .open(open)
            .on_decision(|s: &mut Answered, decision: ToolApprovalDecision| {
                s.decision = Some(decision);
                s.decisions += 1;
            })
            .on_decision_with_id(
                |s: &mut Answered, decision: ToolApprovalDecision, id: Option<String>| {
                    s.decision_id = Some((decision, id));
                    s.decision_id_calls += 1;
                },
            )
            .on_open_change(|s: &mut Answered, next: bool| {
                s.open = Some(next);
                s.open_calls += 1;
            })
    }

    fn build(v: &ToolApprovalView<Answered>) -> ToolApprovalWidget {
        let mut counter = 0u64;
        View::<Answered>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ToolApprovalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(420.0, 900.0)),
        )
    }

    fn laid_out(status: ToolApprovalStatus, open: bool) -> (ToolApprovalWidget, Size) {
        let mut w = build(&view(status, open));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut ToolApprovalWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn rebuild(
        w: &mut ToolApprovalWidget,
        from: (ToolApprovalStatus, bool),
        to: (ToolApprovalStatus, bool),
    ) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Answered>::rebuild(&view(to.0, to.1), &view(from.0, from.1), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ToolApprovalWidget, size: Size, event: &InputEvent, state: &mut Answered) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn press(w: &mut ToolApprovalWidget, size: Size, at: Point, state: &mut Answered) {
        dispatch(w, size, &pointer(PointerPhase::Down, at), state);
        dispatch(w, size, &pointer(PointerPhase::Up, at), state);
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// The model: every status carries its word, only `Pending` shows the
    /// action row, and only the two in-flight statuses report busy.
    #[test]
    fn every_status_carries_its_word_and_only_pending_is_actionable() {
        assert_eq!(ToolApprovalStatus::Pending.label(), "Approval required");
        assert_eq!(ToolApprovalStatus::Approving.label(), "Approving");
        assert_eq!(ToolApprovalStatus::Approved.label(), "Approved");
        assert_eq!(ToolApprovalStatus::Denied.label(), "Denied");
        assert_eq!(ToolApprovalStatus::Running.label(), "Running");
        assert_eq!(ToolApprovalStatus::Complete.label(), "Completed");
        assert_eq!(ToolApprovalStatus::Error.label(), "Failed");

        assert!(ToolApprovalStatus::default().is_pending());
        for status in [
            ToolApprovalStatus::Approving,
            ToolApprovalStatus::Approved,
            ToolApprovalStatus::Denied,
            ToolApprovalStatus::Running,
            ToolApprovalStatus::Complete,
            ToolApprovalStatus::Error,
        ] {
            assert!(!status.is_pending(), "{status:?} is not actionable");
        }
        assert!(ToolApprovalStatus::Approving.is_busy());
        assert!(ToolApprovalStatus::Running.is_busy());
        assert!(!ToolApprovalStatus::Pending.is_busy());

        assert_eq!(ToolApprovalDecision::AllowOnce.label(), "Allow once");
        assert_eq!(ToolApprovalDecision::AlwaysAllow.label(), "Always allow");
        assert_eq!(ToolApprovalDecision::Deny.label(), "Deny");
    }

    /// The hues split the seven statuses into the four families upstream's
    /// badge classes do.
    #[test]
    fn the_status_hues_group_the_lifecycle() {
        let theme = crate::theme();
        let hue = |s| ToolApprovalWidget::status_hue(s, Some(&theme));
        let tokens = BeuiTokens::resolve(Some(&theme));
        assert_eq!(hue(ToolApprovalStatus::Pending), tokens.warning);
        assert_eq!(
            hue(ToolApprovalStatus::Approving),
            hue(ToolApprovalStatus::Running)
        );
        assert_eq!(hue(ToolApprovalStatus::Approved), tokens.success);
        assert_eq!(
            hue(ToolApprovalStatus::Complete),
            hue(ToolApprovalStatus::Approved)
        );
        assert_eq!(hue(ToolApprovalStatus::Denied), theme.scheme().error);
        assert_eq!(
            hue(ToolApprovalStatus::Error),
            hue(ToolApprovalStatus::Denied)
        );
        assert_eq!(
            ToolApprovalWidget::status_hue(ToolApprovalStatus::Denied, None),
            BEUI_LIGHT.destructive,
            "the unthemed fallback is the vendored table"
        );
    }

    /// The whole chrome paints: the card, its hairline, the badge, the status
    /// chip, the head runs, the details trigger and the action row.
    #[test]
    fn a_pending_card_paints_its_badge_chip_details_and_actions() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        // card, badge, chip, three buttons (Deny is a ghost with no rest fill).
        assert!(rec.rounded.len() >= 5, "filled {} boxes", rec.rounded.len());
        assert!(
            rec.strokes >= 4,
            "card, badge and chip hairlines plus glyphs"
        );
        assert_eq!(
            rec.clips.len(),
            3,
            "the card clip, the head column's clip and the action-row clip"
        );
        assert_eq!(rec.transforms.len(), 1, "only the details chevron");
        // title, tool, chip, description, details label, three button labels.
        assert!(rec.inks.len() >= 8, "painted {} runs", rec.inks.len());
        assert!(
            rec.rects
                .iter()
                .any(|(_, s, _)| s.height == style::BORDER_WIDTH),
            "the action row's top border"
        );
    }

    /// A settled non-pending card has no action row at all, and is shorter for
    /// it.
    #[test]
    fn a_settled_decision_drops_the_action_row() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        let pending_height = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);

        rebuild(
            &mut w,
            (ToolApprovalStatus::Pending, false),
            (ToolApprovalStatus::Approved, false),
        );
        for step in 0..=40 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        assert_eq!(w.actions_presence(), 0.0);
        assert_eq!(w.actions_height(), 0.0);
        let settled = layout(&mut w).height;
        assert!(settled < pending_height, "the card never shrank");
        assert!(w.button_rect(ToolApprovalDecision::AllowOnce).is_none());
        assert_eq!(w.targets(), vec![ApprovalTarget::Details]);
    }

    /// The action row plays a real exit rather than vanishing on the rebuild —
    /// the whole reason the presence driver exists.
    #[test]
    fn the_action_row_plays_its_exit_before_it_goes() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.actions_presence(), 1.0);

        rebuild(
            &mut w,
            (ToolApprovalStatus::Pending, false),
            (ToolApprovalStatus::Denied, false),
        );
        let (rec, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a leaving row must ask for relayout");
        assert!(w.actions_presence() > 0.0, "still on screen mid-exit");
        let (rec_mid, _, _) = paint_at(&mut w, size, None, 170.0);
        assert!(
            rec_mid.layers.iter().any(|a| *a > 0.0 && *a < 1.0),
            "and composited partway out: {:?}",
            rec_mid.layers
        );
        assert!(!rec.layers.is_empty() || !rec_mid.layers.is_empty());

        paint_at(&mut w, size, None, 1_000.0);
        assert_eq!(w.actions_presence(), 0.0, "then it is gone");
    }

    /// A decision fires exactly once per pending episode, whichever button is
    /// pressed, and the card never writes its own status.
    #[test]
    fn a_decision_fires_exactly_once_per_pending_episode() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();

        let allow = w.button_rect(ToolApprovalDecision::AllowOnce).unwrap();
        press(&mut w, size, allow.center(), &mut state);
        assert_eq!(state.decision, Some(ToolApprovalDecision::AllowOnce));
        assert_eq!(state.decisions, 1);
        assert!(
            w.status.is_pending(),
            "the card never writes its own status"
        );

        // A second press before the app has responded is refused.
        press(&mut w, size, allow.center(), &mut state);
        let deny = w.button_rect(ToolApprovalDecision::Deny).unwrap();
        press(&mut w, size, deny.center(), &mut state);
        assert_eq!(state.decisions, 1, "the latch held");

        // The app answers, and a later pending episode is fresh again.
        rebuild(
            &mut w,
            (ToolApprovalStatus::Pending, false),
            (ToolApprovalStatus::Approving, false),
        );
        assert!(!w.decided, "the latch cleared with the status");
        rebuild(
            &mut w,
            (ToolApprovalStatus::Approving, false),
            (ToolApprovalStatus::Pending, false),
        );
        for step in 0..=40 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        let deny = w.button_rect(ToolApprovalDecision::Deny).unwrap();
        press(&mut w, size, deny.center(), &mut state);
        assert_eq!(state.decisions, 2);
        assert_eq!(state.decision, Some(ToolApprovalDecision::Deny));
    }

    /// A payload swap delivered between `Down` and `Up` disarms the in-flight
    /// press instead of firing a decision against whatever is now displayed —
    /// the tool-approval bait-and-switch.
    #[test]
    fn a_payload_swap_disarms_an_in_flight_press() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();

        let allow = w
            .button_rect(ToolApprovalDecision::AllowOnce)
            .unwrap()
            .center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, allow),
            &mut state,
        );
        assert!(w.captured.is_some(), "the press armed");

        // A different request lands mid-press — same status, different
        // content, exactly what a streaming agent surface can deliver.
        let swapped = tool_approval::<Answered>("shell.exec")
            .title("Allow this OTHER tool to run?")
            .description("A different agent turn swapped the payload mid-press.")
            .parameters(vec![tool_approval_parameter("cmd", "curl evil.example")])
            .status(ToolApprovalStatus::Pending)
            .open(false)
            .on_decision(|s: &mut Answered, decision: ToolApprovalDecision| {
                s.decision = Some(decision);
                s.decisions += 1;
            });
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Answered>::rebuild(
            &swapped,
            &view(ToolApprovalStatus::Pending, false),
            &mut w,
            &mut ctx,
        );

        assert!(w.captured.is_none(), "the payload swap disarmed the press");
        assert_eq!(
            w.focused,
            ApprovalTarget::Decision(ToolApprovalDecision::Deny),
            "focus re-armed at the safe default"
        );

        dispatch(&mut w, size, &pointer(PointerPhase::Up, allow), &mut state);
        assert_eq!(
            state.decisions, 0,
            "the swapped-in request never received a decision"
        );
    }

    /// A decision carries the identity of the request actually shown, not a
    /// bare enum the app must guess an owner for.
    #[test]
    fn a_decision_reports_the_id_of_the_request_shown() {
        let mut w = build(
            &tool_approval::<Answered>("shell.exec")
                .id("req-42")
                .title("Allow this tool to run?")
                .status(ToolApprovalStatus::Pending)
                .on_decision_with_id(
                    |s: &mut Answered, decision: ToolApprovalDecision, id: Option<String>| {
                        s.decision_id = Some((decision, id));
                        s.decision_id_calls += 1;
                    },
                ),
        );
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();
        let allow = w.button_rect(ToolApprovalDecision::AllowOnce).unwrap();
        press(&mut w, size, allow.center(), &mut state);
        assert_eq!(
            state.decision_id,
            Some((ToolApprovalDecision::AllowOnce, Some("req-42".to_owned())))
        );
        assert_eq!(state.decision_id_calls, 1);
    }

    /// With no explicit id there is no identity to report: the callback gets
    /// `None` rather than the tool name, which is shared by every call to the
    /// same tool and so cannot correlate consent.
    #[test]
    fn a_decision_without_an_explicit_id_reports_no_identity() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();
        let allow = w
            .button_rect(ToolApprovalDecision::AllowOnce)
            .unwrap()
            .center();
        press(&mut w, size, allow, &mut state);
        assert_eq!(
            state.decision_id,
            Some((ToolApprovalDecision::AllowOnce, None)),
            "the tool name is not an identity"
        );
        assert_eq!(state.decision_id_calls, 1);
    }

    /// A fresh request delivered under an unchanged `Pending` status opens a
    /// new consent episode: the latch the previous answer raised is released,
    /// so the request now displayed can be answered — exactly once, and the
    /// decision reports the *new* request's identity.
    #[test]
    fn a_request_swapped_in_at_the_same_status_can_still_be_answered() {
        let request = |id: &'static str, tool: &'static str| {
            tool_approval::<Answered>(tool)
                .id(id)
                .title("Allow this tool to run?")
                .status(ToolApprovalStatus::Pending)
                .on_decision(|s: &mut Answered, decision: ToolApprovalDecision| {
                    s.decision = Some(decision);
                    s.decisions += 1;
                })
                .on_decision_with_id(
                    |s: &mut Answered, decision: ToolApprovalDecision, id: Option<String>| {
                        s.decision_id = Some((decision, id));
                        s.decision_id_calls += 1;
                    },
                )
        };

        let first = request("req-a", "shell.exec");
        let mut w = build(&first);
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();

        let allow = w
            .button_rect(ToolApprovalDecision::AllowOnce)
            .unwrap()
            .center();
        press(&mut w, size, allow, &mut state);
        assert_eq!(state.decisions, 1);

        // The app has not moved the status yet — a queue advancing to its
        // next request while the answer round-trips.
        let second = request("req-b", "net.fetch");
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Answered>::rebuild(&second, &first, &mut w, &mut ctx);
        layout(&mut w);
        paint_at(&mut w, size, None, 16.0);

        let allow = w
            .button_rect(ToolApprovalDecision::AllowOnce)
            .unwrap()
            .center();
        press(&mut w, size, allow, &mut state);
        assert_eq!(state.decisions, 2, "the swapped-in request was answerable");
        assert_eq!(
            state.decision_id,
            Some((ToolApprovalDecision::AllowOnce, Some("req-b".to_owned()))),
            "and the decision named the request it answered"
        );

        // Still one decision per episode: the released latch re-armed.
        press(&mut w, size, allow, &mut state);
        assert_eq!(state.decisions, 2);
    }

    /// Changing what the buttons would grant is a new episode too: an
    /// in-flight press is disarmed rather than released against a set it was
    /// never aimed at.
    #[test]
    fn changing_the_offered_buttons_disarms_an_in_flight_press() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();
        let allow = w
            .button_rect(ToolApprovalDecision::AllowOnce)
            .unwrap()
            .center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, allow),
            &mut state,
        );

        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Answered>::rebuild(
            &view(ToolApprovalStatus::Pending, false).always_allow(false),
            &view(ToolApprovalStatus::Pending, false),
            &mut w,
            &mut ctx,
        );
        layout(&mut w);
        paint_at(&mut w, size, None, 16.0);

        dispatch(&mut w, size, &pointer(PointerPhase::Up, allow), &mut state);
        assert_eq!(
            state.decisions, 0,
            "the release fired nothing after the offered set changed"
        );
    }

    /// Agent-supplied head text is bounded by a column that reserves the
    /// status chip's box, painted under a clip of exactly that column, and
    /// marks its own overflow — so it can neither deface the status indicator
    /// nor be cut in silence.
    #[test]
    fn a_long_title_is_bounded_away_from_the_chip_and_marks_the_cut() {
        let mut short = build(
            &tool_approval::<Answered>("shell.exec")
                .title("Read config.yaml?")
                .status(ToolApprovalStatus::Pending),
        );
        let short_size = layout(&mut short);
        let (short_rec, _, _) = paint_at(&mut short, short_size, None, 0.0);

        // One unbreakable token, so the run overflows its bound rather than
        // wrapping — the case a clip would otherwise swallow in silence.
        let overlong = format!("Read{}", "a".repeat(300));
        let mut long = build(
            &tool_approval::<Answered>("shell.exec")
                .title(overlong)
                .status(ToolApprovalStatus::Pending),
        );
        let long_size = layout(&mut long);
        let (long_rec, _, _) = paint_at(&mut long, long_size, None, 0.0);

        let chip = long_rec
            .rounded
            .iter()
            .find(|(_, s, _)| s.height == TOOL_APPROVAL_CHIP_HEIGHT)
            .expect("the status chip painted");
        let (head_at, head_size) = long_rec.clips[1];
        assert!(
            head_at.x + head_size.width <= chip.0.x,
            "the head column ({head_at:?} + {head_size:?}) reaches the chip at {:?}",
            chip.0
        );
        assert!(
            long_rec.inks.len() > short_rec.inks.len(),
            "the cut head painted an extra glyph run for the marker"
        );
    }

    /// A parameter's *name* is consent-bearing too: an unbreakable label wider
    /// than its column paints the same truncation cue the value does.
    #[test]
    fn a_clipped_parameter_label_paints_the_truncation_marker() {
        let mut short = build(
            &tool_approval::<Answered>("shell.exec")
                .parameters(vec![tool_approval_parameter("cwd", "/tmp")])
                .status(ToolApprovalStatus::Pending)
                .open(true),
        );
        let short_size = layout(&mut short);
        let (short_rec, _, _) = paint_at(&mut short, short_size, None, 0.0);

        let long_label = "cwd".repeat(80);
        let mut long = build(
            &tool_approval::<Answered>("shell.exec")
                .parameters(vec![tool_approval_parameter(long_label, "/tmp")])
                .status(ToolApprovalStatus::Pending)
                .open(true),
        );
        let long_size = layout(&mut long);
        let (long_rec, _, _) = paint_at(&mut long, long_size, None, 0.0);

        assert!(
            long_rec.inks.len() > short_rec.inks.len(),
            "the truncated label painted an extra glyph run for the marker"
        );
    }

    /// Keyboard activation with no explicit selection lands on the safe,
    /// least-permissive decision — never one that grants anything.
    #[test]
    fn the_default_focus_is_the_safe_decision() {
        let (w, _) = laid_out(ToolApprovalStatus::Pending, false);
        assert_eq!(
            w.focused,
            ApprovalTarget::Decision(ToolApprovalDecision::Deny)
        );
    }

    /// An unbreakable parameter value wider than its column paints a visible
    /// truncation cue rather than being silently cut off by the panel clip.
    #[test]
    fn a_clipped_parameter_value_paints_the_truncation_marker() {
        let mut short = build(
            &tool_approval::<Answered>("shell.exec")
                .parameters(vec![tool_approval_parameter("hash", "deadbeef")])
                .status(ToolApprovalStatus::Pending)
                .open(true),
        );
        let short_size = layout(&mut short);
        assert!(!short.params[0].truncated);
        let (short_rec, _, _) = paint_at(&mut short, short_size, None, 0.0);

        let long_token = format!("0x{}", "a".repeat(200));
        let mut long = build(
            &tool_approval::<Answered>("shell.exec")
                .parameters(vec![tool_approval_parameter("hash", long_token)])
                .status(ToolApprovalStatus::Pending)
                .open(true),
        );
        let long_size = layout(&mut long);
        assert!(
            long.params[0].truncated,
            "the unbreakable token overflowed its column"
        );
        let (long_rec, _, _) = paint_at(&mut long, long_size, None, 0.0);

        assert!(
            long_rec.inks.len() > short_rec.inks.len(),
            "the truncated row painted an extra glyph run for the marker"
        );
    }

    /// A non-pending card refuses a decision outright, even at the coordinates
    /// the buttons used to occupy.
    #[test]
    fn a_non_pending_card_refuses_a_decision() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let allow = w.button_rect(ToolApprovalDecision::AllowOnce).unwrap();

        rebuild(
            &mut w,
            (ToolApprovalStatus::Pending, false),
            (ToolApprovalStatus::Running, false),
        );
        let mut state = Answered::default();
        press(&mut w, size, allow.center(), &mut state);
        assert_eq!(state.decisions, 0);
    }

    /// A press released off its armed button fires nothing — the up-inside
    /// rule.
    #[test]
    fn a_press_released_off_target_reports_nothing() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();
        let allow = w
            .button_rect(ToolApprovalDecision::AllowOnce)
            .unwrap()
            .center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, allow),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(allow.x, 4.0)),
            &mut state,
        );
        assert_eq!(state.decisions, 0);
    }

    /// The details disclosure morphs the card's own height and asks for
    /// relayout while it moves.
    #[test]
    fn disclosing_the_parameters_morphs_the_height() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        let closed = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.params_band(), 0.0);

        rebuild(
            &mut w,
            (ToolApprovalStatus::Pending, false),
            (ToolApprovalStatus::Pending, true),
        );
        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "an opening disclosure must ask for relayout");

        let mut tallest = closed;
        for step in 1..=60 {
            paint_at(&mut w, size, None, 100.0 + f64::from(step) * 25.0);
            tallest = tallest.max(layout(&mut w).height);
        }
        assert!(tallest > closed, "the card never grew");
        assert_eq!(w.reveal.value(), 1.0);
        assert!(w.params_natural() > 0.0, "the panel has real rows");
        let (rec, _, _) = paint_at(&mut w, size, None, 6_000.0);
        assert_eq!(
            rec.clips.len(),
            4,
            "card, head column, parameter panel, action row"
        );
    }

    /// A press on *View details* reports the disclosure it wants and never
    /// writes its own.
    #[test]
    fn the_details_trigger_reports_the_requested_disclosure() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();
        let details = w.details_rect().unwrap().center();
        press(&mut w, size, details, &mut state);
        assert_eq!(state.open, Some(true));
        assert_eq!(state.open_calls, 1);
        assert!(!w.open, "the widget never writes its own flag");
    }

    /// Keyboard: the arrows rove the details trigger and every offered button,
    /// and the activation keys fire the one under the cursor.
    #[test]
    fn the_arrows_rove_every_trigger_and_enter_fires_it() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Answered::default();
        assert_eq!(w.targets().len(), 4, "details plus three decisions");

        w.focused = ApprovalTarget::Details;
        dispatch(&mut w, size, &key(NamedKey::ArrowRight), &mut state);
        assert_eq!(
            w.focused,
            ApprovalTarget::Decision(ToolApprovalDecision::AllowOnce)
        );
        dispatch(&mut w, size, &key(NamedKey::ArrowRight), &mut state);
        assert_eq!(
            w.focused,
            ApprovalTarget::Decision(ToolApprovalDecision::AlwaysAllow)
        );
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.decision, Some(ToolApprovalDecision::AlwaysAllow));
        assert_eq!(state.decisions, 1);
    }

    /// Switching *Always allow* off drops it from the row and from the roving
    /// order.
    #[test]
    fn dropping_always_allow_drops_its_button() {
        let mut w = build(&view(ToolApprovalStatus::Pending, false).always_allow(false));
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.buttons.len(), 2);
        assert!(w.button_rect(ToolApprovalDecision::AlwaysAllow).is_none());
        assert_eq!(w.targets().len(), 3, "details plus two decisions");
    }

    /// A card with no parameters has no disclosure at all.
    #[test]
    fn a_card_with_no_parameters_has_no_details_trigger() {
        let mut w = build(&tool_approval::<Answered>("fs.read").title("Read a file?"));
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert!(!w.has_details());
        assert!(w.details_rect().is_none());
        assert_eq!(w.params_band(), 0.0);
        assert_eq!(w.params_natural(), 0.0);
    }

    /// `reduce_motion` lands the disclosure and the action row on the frame
    /// they are painted, and freezes the busy spinner.
    #[test]
    fn reduce_motion_lands_every_stage_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(ToolApprovalStatus::Pending, false);
        paint_at(&mut w, size, Some(&theme), 0.0);
        rebuild(
            &mut w,
            (ToolApprovalStatus::Pending, false),
            (ToolApprovalStatus::Approving, true),
        );
        paint_at(&mut w, size, Some(&theme), 100.0);
        assert_eq!(w.reveal.value(), 1.0);
        assert_eq!(w.actions_presence(), 0.0);

        layout(&mut w);
        let (_, needs_frame, still) = paint_at(&mut w, size, Some(&theme), 120.0);
        assert!(
            !needs_frame && !still,
            "a snapped card asks for nothing, spinner included"
        );
    }

    /// A busy card turns its badge spinner off a paced tick.
    #[test]
    fn a_busy_card_paces_its_spinner() {
        let (mut w, size) = laid_out(ToolApprovalStatus::Running, false);
        for step in 0..=40 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        layout(&mut w);
        let (_, needs_frame, _) = paint_at(&mut w, size, None, 2_000.0);
        assert!(needs_frame, "the spinner asks for its next tick");
    }
}
