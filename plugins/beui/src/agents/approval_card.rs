//! Ports beUI's `approval-card` agent-interface part — the **review** variant.
//!
//! **Source:** `components/agents/approval-card/index.tsx` and its
//! `types.ts`, beUI monorepo rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | card `rounded-2xl bg-muted p-4 text-sm` | [`APPROVAL_CARD_RADIUS`], [`APPROVAL_CARD_PADDING`] |
//! | leading `size-5` status glyph | [`APPROVAL_CARD_GLYPH_BOX`] |
//! | title `text-base font-medium leading-5` | [`APPROVAL_CARD_TITLE_SIZE`] |
//! | status chip `rounded-full border px-2 py-0.5 text-[11px]` | [`APPROVAL_CARD_CHIP_HEIGHT`], [`ApprovalCardStatus::label`] |
//! | dismiss `size-5` cross | [`ApprovalCardView::dismissible`] |
//! | `description` paragraph `leading-5` | a wrapped run |
//! | `children` detail block `mt-3` | [`ApprovalCardView::content`] |
//! | action row `mt-4 gap-2`, `size="sm" rounded-full` | [`APPROVAL_CARD_BUTTON_HEIGHT`], [`ApprovalCardAction`] |
//! | `<AgentDisclosure open={interactive}>` | the body reveal |
//! | `{!interactive && <p>{result ?? statusLabel}</p>}` | [`ApprovalCardView::result`], cross-faded with the body |
//!
//! # Which variant this is
//!
//! Upstream's approval card is two components behind one export: a **review**
//! surface (title, description, detail block, approve / request-changes /
//! reject, collapsing into the recorded decision) and a **question wizard**
//! (`questions[]`, per-step single/multiple choice with a free-text answer,
//! progress dots, prev/next, auto-advance, `onSubmit(answers)`).
//!
//! This port ships the review surface. The wizard is **not ported**: it is a
//! multi-step form whose every step is a composition of the catalog's own
//! checkbox, radio and input controls — a container of live form widgets with
//! its own answer model — rather than a variant of this card's chrome, and
//! folding it in here would make one module two components. It is recorded as
//! a degradation rather than approximated: a half-wizard that collects answers
//! but cannot validate or auto-advance them would be worse than none.
//!
//! # The collapse into the decision
//!
//! `interactive` (pending or submitting) is what the card discloses. One lane
//! drives both halves of the morph: the body (description, detail block, action
//! row) is `reveal · natural height` and fades with it, and the recorded
//! outcome line fades in on `1 − reveal` as the body leaves. So a decision
//! visibly *collapses the card into its result* rather than swapping two
//! layouts, which is the component's whole gesture.
//!
//! Because the height is computed in `layout` from a lane advanced in `paint`,
//! `paint` asks for **layout** while the lane moves and once more on the frame
//! its value changed — the same guard the catalog's accordion documents.
//!
//! # One decision per consent episode, on one predicate
//!
//! [`ApprovalCardAction::Approve`], [`RequestChanges`](ApprovalCardAction) and
//! [`Reject`](ApprovalCardAction) fire **exactly once** per consent episode:
//! the press is up-inside, and the widget then latches until a new episode
//! opens. [`Dismiss`](ApprovalCardAction) is deliberately **not** latched — it
//! closes the surface rather than answering it, and is offered in every
//! status.
//!
//! What opens a new episode is **one predicate**, and it governs every piece
//! of episode state together — the in-flight `Down` capture, the roving
//! keyboard focus and the one-decision latch are cleared as a unit, never
//! separately. An episode is new when the status changes *or* when any
//! consent-bearing value changes: `title`, `description`, the offered button
//! set (including [`approve_label`](ApprovalCardView::approve_label)),
//! [`id`](ApprovalCardView::id), or the detail block's *identity* — its
//! presence, and the [`content_key`](ApprovalCardView::content_key) the
//! caller labels it with.
//!
//! So a payload swap delivered between `Down` and `Up` disarms the press
//! rather than letting the release fire against whatever is now displayed
//! (mirrors [`citations`](super::citations)'s own item-list invalidation), and
//! a fresh review swapped in under an unchanged `Pending` status releases the
//! latch too, so it is answerable rather than displayed-as-pending but
//! permanently dead.
//!
//! The detail block's *repaints* are deliberately not part of that predicate.
//! A [`content`](ApprovalCardView::content) child is an arbitrary view — a
//! streaming response repaints every frame it grows, a theme change repaints
//! every child there is — and disarming on a bubbled paint or layout flag
//! would make a press impossible to complete and reset the keyboard cursor on
//! every one of those frames, while saying nothing about whether the thing
//! being consented to changed. A caller whose detail block *does* change what
//! consent means labels it with [`content_key`](ApprovalCardView::content_key)
//! (a diff hash, a revision, a request id); the card compares that value, not
//! the child's flags.
//!
//! # Correlating an action with the review it answers
//!
//! [`ApprovalCardView::on_action_with_id`] hands the callback the identity of
//! the review actually shown: `Some(id)` when [`ApprovalCardView::id`] was
//! set, `None` when it was not. There is deliberately **no** derived fallback
//! — `title` is not an identity (two reviews can share one), and correlating
//! on a colliding value can route consent granted for one review to another.
//! **Routing consent without an explicit `id` is unsafe whenever more than
//! one review can be open**: an app that can have several in flight must set
//! [`id`](ApprovalCardView::id) per review, and must treat a `None` identity
//! as "this action cannot be correlated" rather than as a review name.
//!
//! # Degradations against the web original
//!
//! - **No question wizard**, above.
//! - **No rolling title swap.** Upstream rolls the title through
//!   `ActionSwapRollText` when it changes; a leaf card shaping its own runs
//!   re-shapes in place instead (the same degradation the
//!   [tool result](super::tool_result) records).
//! - **The detail block does not fade independently.** Upstream animates the
//!   `children` block on its own ramp inside the disclosure; here one reveal
//!   drives the whole body, so the block arrives with the action row rather
//!   than slightly before it.
//! - **The title is bounded, and can never reach the status chip.** `title` is
//!   agent-supplied, so it is wrapped into a column that reserves the chip's
//!   box (and the dismiss affordance's, when offered) out of its own, and
//!   painted under a clip of exactly that column. Agent glyphs therefore
//!   cannot overdraw the status indicator the decision is read against, and an
//!   unbreakable run that still overflows paints a trailing `…` cue rather
//!   than being cut with no sign anything was elided.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View,
    Widget, any, build_child, erase_callback_arg, rebuild_child, route_event_single,
    teardown_child, visit_children,
};

use crate::agents::code_block::{
    code_palette, draw_check, draw_cross, draw_spinner, spinner_angle,
};
use crate::agents::tool_approval::{
    CHIP_ROLE, CONTROL_ROLE, PROSE_ROLE, TRUNCATION_MARKER, WrappedRun, consent_text, prose_style,
    strong_style,
};
use crate::motion::Ramp;
use crate::press::{Lane, draw_focus_ring, inside, is_activation_key, presses};
use crate::style::{self, with_alpha};
use crate::text::{LabelRun, ThemeTextType};
use crate::tokens::motion::SPRING_PANEL;
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The card's corner radius (`rounded-2xl`).
pub const APPROVAL_CARD_RADIUS: f64 = style::RADIUS_2XL;

/// The card's padding, in logical px (`p-4`).
pub const APPROVAL_CARD_PADDING: f64 = 16.0;

/// The gap between the status glyph and the content column, in logical px
/// (`gap-3`).
pub const APPROVAL_CARD_GAP: f64 = 12.0;

/// The leading status glyph's box, in logical px (`size-5`).
pub const APPROVAL_CARD_GLYPH_BOX: f64 = 20.0;

/// The title's type size, in logical px (`text-base font-medium`).
pub const APPROVAL_CARD_TITLE_SIZE: f64 = style::TEXT_BASE;

/// The title's line height, in logical px (`leading-5`).
pub const APPROVAL_CARD_TITLE_LINE: f64 = 20.0;

/// The type-scale role the title (and its truncation cue) takes its family
/// from at layout. The chip, the buttons and the prose share
/// [`tool_approval`](super::tool_approval)'s roles.
const TITLE_ROLE: ThemeTextType = ThemeTextType::TitleMedium;

/// The status chip's type size, in logical px (`text-[11px]`).
pub const APPROVAL_CARD_CHIP_SIZE: f64 = 11.0;

/// The status chip's height, in logical px (`py-0.5`).
pub const APPROVAL_CARD_CHIP_HEIGHT: f64 = 20.0;

/// The status chip's horizontal padding, in logical px (`px-2`).
pub const APPROVAL_CARD_CHIP_PADDING_X: f64 = 8.0;

/// Alpha the status chip's fill is painted at (`bg-<hue>-500/10`).
pub const APPROVAL_CARD_CHIP_FILL_ALPHA: f32 = 0.1;

/// Alpha the status chip's hairline is painted at (`border-<hue>-500/30`).
pub const APPROVAL_CARD_CHIP_BORDER_ALPHA: f32 = 0.3;

/// The dismiss affordance's box, in logical px (`size-5`).
pub const APPROVAL_CARD_DISMISS_BOX: f64 = 20.0;

/// The gap above the description, in logical px (`mt-1`).
pub const APPROVAL_CARD_DESCRIPTION_GAP: f64 = 4.0;

/// The gap above the detail block, in logical px (`mt-3`).
pub const APPROVAL_CARD_CONTENT_GAP: f64 = 12.0;

/// The gap above the action row, in logical px (`mt-4`).
pub const APPROVAL_CARD_ACTIONS_GAP: f64 = 16.0;

/// One action button's height, in logical px (`size="sm"`).
pub const APPROVAL_CARD_BUTTON_HEIGHT: f64 = style::HEIGHT_SM;

/// The gap between action buttons, in logical px (`gap-2`).
pub const APPROVAL_CARD_BUTTON_GAP: f64 = style::GAP_MD;

/// The recorded-outcome line's height, in logical px (`text-sm` on `mt-1`).
pub const APPROVAL_CARD_RESULT_HEIGHT: f64 = 24.0;

/// The ramp the body reveal plays, in both directions.
pub const APPROVAL_CARD_REVEAL: Ramp = Ramp::spring(SPRING_PANEL);

/// Where a review is in its lifecycle (`ApprovalCardStatus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApprovalCardStatus {
    /// `"pending"` — waiting on the user.
    #[default]
    Pending,
    /// `"submitting"` — the decision is being sent.
    Submitting,
    /// `"approved"` — accepted.
    Approved,
    /// `"rejected"` — refused.
    Rejected,
    /// `"changes-requested"` — sent back for revision.
    ChangesRequested,
    /// `"answered"` — a response was recorded.
    Answered,
}

impl ApprovalCardStatus {
    /// The chip's word (`getStatusLabel`).
    pub const fn label(self) -> &'static str {
        match self {
            ApprovalCardStatus::Pending => "Input required",
            ApprovalCardStatus::Submitting => "Submitting",
            ApprovalCardStatus::Approved => "Approved",
            ApprovalCardStatus::Rejected => "Rejected",
            ApprovalCardStatus::ChangesRequested => "Changes requested",
            ApprovalCardStatus::Answered => "Response submitted",
        }
    }

    /// Whether the card is still being answered — what the body discloses on
    /// (`interactive` upstream).
    pub const fn is_interactive(self) -> bool {
        matches!(
            self,
            ApprovalCardStatus::Pending | ApprovalCardStatus::Submitting
        )
    }

    /// Whether a decision is in flight (`aria-busy`).
    pub const fn is_busy(self) -> bool {
        matches!(self, ApprovalCardStatus::Submitting)
    }

    /// Whether a decision may still be made — interactive and not already in
    /// flight.
    pub const fn accepts_decision(self) -> bool {
        matches!(self, ApprovalCardStatus::Pending)
    }
}

/// What the user did (`onApprove` / `onRequestChanges` / `onReject` /
/// `onDismiss`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalCardAction {
    /// Accept the proposal.
    Approve,
    /// Send it back for revision.
    RequestChanges,
    /// Refuse it.
    Reject,
    /// Close the card without answering it.
    Dismiss,
}

impl ApprovalCardAction {
    /// The button's label. [`Approve`](Self::Approve)'s is overridable through
    /// [`ApprovalCardView::approve_label`] (upstream's `approveLabel`).
    pub const fn label(self) -> &'static str {
        match self {
            ApprovalCardAction::Approve => "Approve",
            ApprovalCardAction::RequestChanges => "Request changes",
            ApprovalCardAction::Reject => "Reject",
            ApprovalCardAction::Dismiss => "Dismiss",
        }
    }

    /// Whether this action answers the card, and so falls under the
    /// one-decision-per-episode latch.
    pub const fn is_decision(self) -> bool {
        !matches!(self, ApprovalCardAction::Dismiss)
    }
}

/// A view-held one-argument callback, erased on build.
type OnArg<State, A> = Rc<dyn Fn(&mut State, A)>;

/// A declarative beUI approval card. See the [module docs](self).
pub struct ApprovalCardView<State: 'static> {
    title: String,
    description: Option<String>,
    content: Option<AnyView<State>>,
    /// The caller's label for what the detail block currently shows (see
    /// [`Self::content_key`]); `None` when the caller gave none.
    content_key: Option<String>,
    result: Option<String>,
    status: ApprovalCardStatus,
    approve_label: String,
    request_changes: bool,
    reject: bool,
    dismissible: bool,
    /// An explicit review identity (see [`Self::id`]). `None` when the caller
    /// set none — there is no derived fallback.
    id: Option<String>,
    on_action: Option<OnArg<State, ApprovalCardAction>>,
    on_action_with_id: Option<OnArg<State, (ApprovalCardAction, Option<String>)>>,
}

/// Create an approval card, pending and offering all three decisions.
///
/// **Controlled**: chain [`ApprovalCardView::status`] to drive the lifecycle
/// and [`ApprovalCardView::on_action`] to hear the user's answer.
pub fn approval_card<State: 'static>() -> ApprovalCardView<State> {
    ApprovalCardView {
        title: "Approval required".to_owned(),
        description: None,
        content: None,
        content_key: None,
        result: None,
        status: ApprovalCardStatus::default(),
        approve_label: ApprovalCardAction::Approve.label().to_owned(),
        request_changes: true,
        reject: true,
        dismissible: false,
        id: None,
        on_action: None,
        on_action_with_id: None,
    }
}

impl<State: 'static> ApprovalCardView<State> {
    /// The card's heading (`title`). Agent-supplied text is neutralised on
    /// the way in — see [`consent_text`].
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = consent_text(&title.into());
        self
    }

    /// The paragraph under the heading (`description`), neutralised like the
    /// title.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(consent_text(&description.into()));
        self
    }

    /// The detail block between the description and the action row
    /// (`children`) — a diff, a code block, a table, anything.
    ///
    /// A child's own repaints do **not** disarm the card's press, and its
    /// paint/layout flags say nothing about consent. So a card whose block has
    /// no identity **does not answer decisions at all**: the approve, request
    /// changes and reject affordances stay disabled until the block is keyed
    /// with [`Self::content_key`] (or given through [`Self::content_keyed`]),
    /// which is what turns a change of substance into a new consent episode.
    /// A dismiss still works. See the [module docs](self).
    pub fn content<V: View<State>>(mut self, content: V) -> Self {
        self.content = Some(any(content));
        self
    }

    /// A detail block together with the identity of what it shows — the
    /// form a consent-bearing block takes. Equivalent to
    /// [`Self::content`] followed by [`Self::content_key`].
    pub fn content_keyed<V: View<State>>(self, key: impl Into<String>, content: V) -> Self {
        self.content(content).content_key(key)
    }

    /// The identity of what the detail block currently shows — a diff hash, a
    /// revision, a request id: anything that changes exactly when the block
    /// starts meaning something different.
    ///
    /// A change to this value opens a new consent episode, the same way a new
    /// title does. It exists because an arbitrary child view's paint and
    /// layout flags say nothing about consent (a streaming block sets them
    /// every frame), so they are not consulted — see the [module docs](self).
    pub fn content_key(mut self, key: impl Into<String>) -> Self {
        self.content_key = Some(key.into());
        self
    }

    /// The line shown once the card has been answered (`result`). Defaults to
    /// the status's own word.
    pub fn result(mut self, result: impl Into<String>) -> Self {
        self.result = Some(result.into());
        self
    }

    /// Where the review is in its lifecycle (`status`).
    pub fn status(mut self, status: ApprovalCardStatus) -> Self {
        self.status = status;
        self
    }

    /// Rename the approve button (`approveLabel`).
    pub fn approve_label(mut self, label: impl Into<String>) -> Self {
        self.approve_label = label.into();
        self
    }

    /// Whether the *Request changes* button is offered (`onRequestChanges`).
    pub fn request_changes(mut self, offered: bool) -> Self {
        self.request_changes = offered;
        self
    }

    /// Whether the *Reject* button is offered (`onReject`).
    pub fn reject(mut self, offered: bool) -> Self {
        self.reject = offered;
        self
    }

    /// Whether the dismiss affordance is offered (`onDismiss`).
    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    /// An explicit identity for this review. Reported to
    /// [`Self::on_action_with_id`] as `Some(id)`; a card that never sets one
    /// reports `None`, since nothing else here is unique per review. See the
    /// [module docs](self)'s "Correlating an action with the review it
    /// answers" section — an app that can have more than one review open at
    /// once must set this.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Report the user's action. A decision fires at most once per interactive
    /// episode — see the [module docs](self).
    pub fn on_action<F: Fn(&mut State, ApprovalCardAction) + 'static>(
        mut self,
        on_action: F,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }

    /// Like [`Self::on_action`], but also reports the identity of the review
    /// actually shown when the action fired: `Some(id)` when [`Self::id`] was
    /// set, `None` when it was not — there is no derived fallback, because a
    /// `None` the caller can see is safer than a colliding one it cannot (see
    /// the [module docs](self)). Additive, so an existing `on_action` call
    /// site keeps compiling unchanged. Both may be set; both fire.
    pub fn on_action_with_id<F: Fn(&mut State, ApprovalCardAction, Option<String>) + 'static>(
        mut self,
        on_action: F,
    ) -> Self {
        self.on_action_with_id = Some(Rc::new(
            move |state: &mut State, (action, id): (ApprovalCardAction, Option<String>)| {
                on_action(state, action, id);
            },
        ));
        self
    }

    /// The decisions this card offers, in visual order.
    fn decisions(&self) -> Vec<ApprovalCardAction> {
        let mut decisions = vec![ApprovalCardAction::Approve];
        if self.request_changes {
            decisions.push(ApprovalCardAction::RequestChanges);
        }
        if self.reject {
            decisions.push(ApprovalCardAction::Reject);
        }
        decisions
    }

    /// The recorded-outcome line's text.
    fn result_text(&self) -> String {
        self.result
            .clone()
            .unwrap_or_else(|| self.status.label().to_owned())
    }
}

/// The retained widget for an [`ApprovalCardView`].
pub struct ApprovalCardWidget {
    /// Wrapped, not free: the title is agent-supplied, so it is bound to
    /// [`Self::head_column_width`] and clipped to it in paint.
    title: WrappedRun,
    title_text: String,
    /// Whether the title overflowed its bounded column and paints the
    /// truncation cue (see [`WrappedRun::overflowed`]).
    title_truncated: bool,
    /// That cue, shaped once.
    head_marker: LabelRun,
    description: Option<WrappedRun>,
    description_text: Option<String>,
    result: WrappedRun,
    result_text: String,
    chip: LabelRun,
    buttons: Vec<(ApprovalCardAction, LabelRun)>,
    content: Option<ChildPod>,
    /// The caller's identity for what `content` shows — the only thing about
    /// the detail block the consent-episode predicate consults.
    content_key: Option<String>,
    status: ApprovalCardStatus,
    approve_label: String,
    request_changes: bool,
    reject: bool,
    dismissible: bool,
    /// The body disclosure: `1` interactive, `0` collapsed into the outcome.
    reveal: Lane,
    /// Raised once a decision has been reported for the current consent
    /// episode; cleared with `captured` and `focused` whenever a new episode
    /// opens (see the [module docs](self)).
    decided: bool,
    width: f64,
    head_height: f64,
    description_height: f64,
    content_height: f64,
    result_height: f64,
    body_natural: f64,
    focused: ApprovalCardAction,
    hovered: Option<ApprovalCardAction>,
    captured: Option<ApprovalCardAction>,
    /// The review identity an action reports: [`ApprovalCardView::id`] when
    /// the caller set one, `None` otherwise. Never derived from `title` — see
    /// the [module docs](self)'s correlation section.
    id: Option<String>,
    on_action: Option<ErasedArgCallback<ApprovalCardAction>>,
    on_action_with_id: Option<ErasedArgCallback<(ApprovalCardAction, Option<String>)>>,
}

/// The buttons a view offers, shaped.
fn build_buttons<State: 'static>(
    view: &ApprovalCardView<State>,
) -> Vec<(ApprovalCardAction, LabelRun)> {
    view.decisions()
        .into_iter()
        .map(|action| {
            let label = if action == ApprovalCardAction::Approve {
                view.approve_label.clone()
            } else {
                action.label().to_owned()
            };
            (action, LabelRun::new(label))
        })
        .collect()
}

/// The safe, least-permissive affordance keyboard activation defaults to:
/// `Reject` when offered, else `RequestChanges` when offered, else `Approve`
/// (the only decision left offered, and so the only default left to give).
fn safe_default_action(request_changes: bool, reject: bool) -> ApprovalCardAction {
    if reject {
        ApprovalCardAction::Reject
    } else if request_changes {
        ApprovalCardAction::RequestChanges
    } else {
        ApprovalCardAction::Approve
    }
}

impl<State: 'static> View<State> for ApprovalCardView<State> {
    type Element = ApprovalCardWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ApprovalCardWidget {
        ApprovalCardWidget {
            title: WrappedRun::new(self.title.clone()),
            title_text: self.title.clone(),
            title_truncated: false,
            head_marker: LabelRun::new(TRUNCATION_MARKER),
            description: self.description.as_deref().map(WrappedRun::new),
            description_text: self.description.clone(),
            result: WrappedRun::new(self.result_text()),
            result_text: self.result_text(),
            chip: LabelRun::new(self.status.label()),
            buttons: build_buttons(self),
            content: self.content.as_ref().map(|view| build_child(view, ctx)),
            content_key: self.content_key.clone(),
            status: self.status,
            approve_label: self.approve_label.clone(),
            request_changes: self.request_changes,
            reject: self.reject,
            dismissible: self.dismissible,
            reveal: Lane::at_rest(
                APPROVAL_CARD_REVEAL,
                if self.status.is_interactive() {
                    1.0
                } else {
                    0.0
                },
            ),
            decided: false,
            width: 0.0,
            head_height: 0.0,
            description_height: 0.0,
            content_height: 0.0,
            result_height: 0.0,
            body_natural: 0.0,
            focused: safe_default_action(self.request_changes, self.reject),
            hovered: None,
            captured: None,
            id: self.id.clone(),
            on_action: self.on_action.as_ref().map(erase_callback_arg),
            on_action_with_id: self.on_action_with_id.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ApprovalCardWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_action = self.on_action.as_ref().map(erase_callback_arg);
        element.on_action_with_id = self.on_action_with_id.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;
        // The one predicate: whether this rebuild opens a new consent episode.
        // Every consent-bearing value feeds it — compared by value, or by the
        // caller's own identity for the detail block, never by a child's
        // paint/layout flags — and it resets `captured`, `focused` and
        // `decided` together at the end. See the [module docs](self).
        let mut new_episode = false;

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
        if element.id != self.id {
            element.id = self.id.clone();
            new_episode = true;
        }
        if element.content_key != self.content_key {
            element.content_key = self.content_key.clone();
            new_episode = true;
        }
        if element.approve_label != self.approve_label
            || element.request_changes != self.request_changes
            || element.reject != self.reject
        {
            element.approve_label = self.approve_label.clone();
            element.request_changes = self.request_changes;
            element.reject = self.reject;
            element.buttons = build_buttons(self);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        if element.dismissible != self.dismissible {
            element.dismissible = self.dismissible;
            // The dismiss box is reserved out of the head column, so offering
            // or dropping it re-bounds the title.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.status != self.status {
            element.status = self.status;
            element.chip = LabelRun::new(self.status.label());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            new_episode = true;
        }
        let result = self.result_text();
        if element.result_text != result {
            element.result = WrappedRun::new(result.clone());
            element.result_text = result;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Retargeted, never restarted: a rebuild re-passing the state the lane
        // is already flying toward leaves its clock alone.
        element.reveal.retarget(if element.status.is_interactive() {
            1.0
        } else {
            0.0
        });

        match (
            prev.content.as_ref(),
            self.content.as_ref(),
            element.content.as_mut(),
        ) {
            (Some(before), Some(after), Some(pod)) => {
                // The child's own flags propagate for painting and layout, but
                // they are not consulted for consent: a streaming detail block
                // reports PAINT|LAYOUT on every frame it grows, and disarming
                // on that would make a press impossible to complete. Whether
                // the block now means something different is the caller's
                // `content_key`, compared above.
                flags |= rebuild_child(before, after, pod, ctx);
            }
            (Some(before), None, Some(pod)) => {
                teardown_child(before, pod, ctx);
                element.content = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                new_episode = true;
            }
            (_, Some(after), None) => {
                element.content = Some(build_child(after, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                new_episode = true;
            }
            _ => {}
        }
        if new_episode {
            // One episode, one reset. Whatever `Down` armed is disarmed (the
            // next `Up` is a no-op wherever it lands), keyboard focus re-arms
            // at the safe default, and the one-decision latch is released so
            // the review now displayed can actually be answered.
            element.captured = None;
            element.focused = safe_default_action(element.request_changes, element.reject);
            element.decided = false;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ApprovalCardWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(view), Some(pod)) = (self.content.as_ref(), element.content.as_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// Paint the question glyph an interactive card carries (`CircleHelp`).
fn draw_question(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let inset = box_size * 0.08;
    let rect = Rect::new(inset, inset, box_size - inset, box_size - inset);
    let circle = RoundedRect::from_rect(rect, (box_size - inset * 2.0) / 2.0);
    scene.stroke_path(
        origin,
        &Shape::to_path(&circle, style::PATH_TOLERANCE),
        1.3,
        &Brush::Solid(color),
    );
    let mut mark = BezPath::new();
    mark.move_to(Point::new(box_size * 0.36, box_size * 0.38));
    mark.line_to(Point::new(box_size * 0.5, box_size * 0.3));
    mark.line_to(Point::new(box_size * 0.62, box_size * 0.42));
    mark.line_to(Point::new(box_size * 0.5, box_size * 0.56));
    mark.move_to(Point::new(box_size * 0.5, box_size * 0.68));
    mark.line_to(Point::new(box_size * 0.5, box_size * 0.74));
    scene.stroke_path(origin, &mark, 1.3, &Brush::Solid(color));
}

impl ApprovalCardWidget {
    /// The content column's width.
    fn column_width(&self) -> f64 {
        (self.width - APPROVAL_CARD_PADDING * 2.0 - APPROVAL_CARD_GLYPH_BOX - APPROVAL_CARD_GAP)
            .max(0.0)
    }

    /// The content column's left edge, in widget-local space.
    fn column_x(&self) -> f64 {
        APPROVAL_CARD_PADDING + APPROVAL_CARD_GLYPH_BOX + APPROVAL_CARD_GAP
    }

    /// The status chip's painted width, from the last shaped chip label.
    fn chip_width(&self) -> f64 {
        self.chip.size().width + APPROVAL_CARD_CHIP_PADDING_X * 2.0
    }

    /// The head column's width: the content column with the status chip's box,
    /// the dismiss affordance's when offered, and the gap before them reserved
    /// out of it. The title is agent-supplied, so it is shaped *and* clipped
    /// to this — the chip's space is not the title's to paint into.
    fn head_column_width(&self) -> f64 {
        let dismiss = if self.dismissible {
            APPROVAL_CARD_DISMISS_BOX + style::GAP_MD
        } else {
            0.0
        };
        (self.column_width() - self.chip_width() - dismiss - style::GAP_MD).max(0.0)
    }

    /// How disclosed the body is, `0` collapsed to `1` interactive.
    fn disclosure(&self) -> f64 {
        self.reveal.value().clamp(0.0, 1.0)
    }

    /// The body's height right now.
    fn body_height(&self) -> f64 {
        self.disclosure() * self.body_natural
    }

    /// The recorded-outcome line's height right now — the complement of the
    /// body's, so the card collapses *into* it.
    fn outcome_height(&self) -> f64 {
        (1.0 - self.disclosure()) * (APPROVAL_CARD_DESCRIPTION_GAP + self.result_height)
    }

    /// The action row's top, in widget-local space, at full disclosure.
    fn actions_top(&self) -> f64 {
        let mut y = APPROVAL_CARD_PADDING + self.head_height;
        if self.description_height > 0.0 {
            y += APPROVAL_CARD_DESCRIPTION_GAP + self.description_height;
        }
        if self.content_height > 0.0 {
            y += APPROVAL_CARD_CONTENT_GAP + self.content_height;
        }
        y + APPROVAL_CARD_ACTIONS_GAP
    }

    /// The dismiss affordance's box, in widget-local space.
    fn dismiss_rect(&self) -> Option<Rect> {
        if !self.dismissible {
            return None;
        }
        Some(Rect::from_origin_size(
            Point::new(
                (self.width - APPROVAL_CARD_PADDING - APPROVAL_CARD_DISMISS_BOX).max(0.0),
                APPROVAL_CARD_PADDING,
            ),
            Size::new(APPROVAL_CARD_DISMISS_BOX, APPROVAL_CARD_DISMISS_BOX),
        ))
    }

    /// Action `action`'s button box, in widget-local space — `None` when it is
    /// not offered or the body has collapsed.
    fn button_rect(&self, action: ApprovalCardAction) -> Option<Rect> {
        if action == ApprovalCardAction::Dismiss {
            return self.dismiss_rect();
        }
        if self.disclosure() <= 0.0 {
            return None;
        }
        let mut x = self.column_x();
        let top = self.actions_top();
        for (candidate, label) in &self.buttons {
            let width = label.size().width + style::PADDING_X_SM * 2.0;
            if *candidate == action {
                return Some(Rect::from_origin_size(
                    Point::new(x, top),
                    Size::new(width, APPROVAL_CARD_BUTTON_HEIGHT),
                ));
            }
            x += width + APPROVAL_CARD_BUTTON_GAP;
        }
        None
    }

    /// The affordance under a widget-local `pos`, if any.
    fn hit(&self, pos: Point) -> Option<ApprovalCardAction> {
        if let Some(rect) = self.dismiss_rect()
            && rect.contains(pos)
        {
            return Some(ApprovalCardAction::Dismiss);
        }
        if !self.status.is_interactive() {
            return None;
        }
        for (action, _) in &self.buttons {
            if let Some(rect) = self.button_rect(*action)
                && rect.contains(pos)
            {
                return Some(*action);
            }
        }
        None
    }

    /// The affordances the roving cursor can reach, in visual order.
    fn targets(&self) -> Vec<ApprovalCardAction> {
        let mut targets = Vec::new();
        if self.status.is_interactive() {
            targets.extend(self.buttons.iter().map(|(action, _)| *action));
        }
        if self.dismissible {
            targets.push(ApprovalCardAction::Dismiss);
        }
        targets
    }

    /// The next affordance `step` places from the focused one, wrapping.
    fn step_target(&self, step: isize) -> Option<ApprovalCardAction> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let at = targets.iter().position(|t| *t == self.focused).unwrap_or(0);
        let next = (at as isize + step).rem_euclid(targets.len() as isize) as usize;
        targets.get(next).copied()
    }

    /// Report the action `target` asks for.
    ///
    /// A decision fires at most once per interactive episode; a dismiss is not
    /// a decision and is never latched — see the [module docs](self).
    /// Whether a decision can be answered right now: not once one has been
    /// given, not outside a pending status, and never while an unkeyed detail
    /// block is showing (its substance would be outside the consent episode).
    fn decisions_locked(&self) -> bool {
        self.decided
            || !self.status.accepts_decision()
            || (self.content.is_some() && self.content_key.is_none())
    }

    fn activate(&mut self, ctx: &mut EventCtx, target: ApprovalCardAction) {
        if target.is_decision() && self.decisions_locked() {
            return;
        }
        // Both callbacks, when set, report the same fired action — the plain
        // one for an existing call site, the id-carrying one for a caller
        // that wants to know which review it answers.
        let mut fired = false;
        if let Some(on_action) = self.on_action.as_mut() {
            on_action(ctx, target);
            fired = true;
        }
        if let Some(on_action_with_id) = self.on_action_with_id.as_mut() {
            on_action_with_id(ctx, (target, self.id.clone()));
            fired = true;
        }
        if fired && target.is_decision() {
            self.decided = true;
        }
    }

    /// The hue `status` reads in.
    fn status_hue(status: ApprovalCardStatus, theme: Option<&Theme>) -> Color {
        let tokens = BeuiTokens::resolve(theme);
        let palette = code_palette(theme);
        match status {
            ApprovalCardStatus::Pending => tokens.warning,
            ApprovalCardStatus::Submitting => palette.function,
            ApprovalCardStatus::Approved | ApprovalCardStatus::Answered => tokens.success,
            ApprovalCardStatus::ChangesRequested => tokens.warning,
            ApprovalCardStatus::Rejected => {
                theme.map_or(BEUI_LIGHT.destructive, |t| t.scheme().error)
            }
        }
    }

    /// Paint the leading glyph for `status`.
    fn draw_status_glyph(
        scene: &mut dyn PaintScene,
        status: ApprovalCardStatus,
        at: Point,
        box_size: f64,
        ink: Color,
        angle: f64,
    ) {
        match status {
            ApprovalCardStatus::Submitting => draw_spinner(scene, at, box_size, angle, ink),
            ApprovalCardStatus::Pending => draw_question(scene, at, box_size, ink),
            ApprovalCardStatus::Rejected => draw_cross(scene, at, box_size, ink),
            _ => draw_check(scene, at, box_size, ink),
        }
    }
}

impl Widget for ApprovalCardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        let column = self.column_width();

        // The chip is shaped first because its box is reserved out of the
        // head column: the title is bounded by what is left, so it cannot be
        // laid into the status indicator's space in the first place.
        self.chip
            .layout_themed(ctx, &strong_style(APPROVAL_CARD_CHIP_SIZE), CHIP_ROLE);
        let title_style = strong_style(APPROVAL_CARD_TITLE_SIZE);
        let head_column = self.head_column_width();
        let title = self
            .title
            .layout_themed(ctx, &title_style, TITLE_ROLE, head_column);
        self.title_truncated = self.title.overflowed();
        if self.title_truncated {
            self.head_marker
                .layout_themed(ctx, &title_style, TITLE_ROLE);
        }
        self.head_height = title
            .height
            .max(APPROVAL_CARD_TITLE_LINE)
            .max(APPROVAL_CARD_CHIP_HEIGHT);
        let button_style = strong_style(style::TEXT_XS);
        for (_, label) in &mut self.buttons {
            label.layout_themed(ctx, &button_style, CONTROL_ROLE);
        }

        let prose = prose_style(style::TEXT_SM);
        self.description_height = match &mut self.description {
            Some(run) => run.layout_themed(ctx, &prose, PROSE_ROLE, column).height,
            None => 0.0,
        };
        self.result_height = self
            .result
            .layout_themed(ctx, &prose, PROSE_ROLE, column)
            .height
            .max(APPROVAL_CARD_RESULT_HEIGHT);

        // The detail block is always laid out at its natural height and stays
        // laid out, so the reveal never re-measures it mid-flight.
        self.content_height = match &mut self.content {
            Some(pod) => {
                let inner = BoxConstraints::new(Size::ZERO, Size::new(column, bc.max().height));
                pod.layout_child(ctx, &inner).height
            }
            None => 0.0,
        };

        let mut body = 0.0;
        if self.description_height > 0.0 {
            body += APPROVAL_CARD_DESCRIPTION_GAP + self.description_height;
        }
        if self.content_height > 0.0 {
            body += APPROVAL_CARD_CONTENT_GAP + self.content_height;
        }
        body += APPROVAL_CARD_ACTIONS_GAP + APPROVAL_CARD_BUTTON_HEIGHT;
        self.body_natural = body;

        let column_x = self.column_x();
        let mut content_y = APPROVAL_CARD_PADDING + self.head_height;
        if self.description_height > 0.0 {
            content_y += APPROVAL_CARD_DESCRIPTION_GAP + self.description_height;
        }
        content_y += APPROVAL_CARD_CONTENT_GAP;
        if let Some(pod) = &mut self.content {
            pod.set_origin(Point::new(column_x, content_y));
        }

        bc.constrain(Size::new(
            self.width,
            APPROVAL_CARD_PADDING * 2.0
                + self.head_height
                + self.body_height()
                + self.outcome_height(),
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
        let card = theme.map_or(BEUI_LIGHT.muted, |t| t.scheme().surface_container);
        let background = theme.map_or(BEUI_LIGHT.background, |t| t.scheme().surface);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let before = self.reveal.value();
        let moving = if reduce_motion {
            self.reveal.snap();
            false
        } else {
            self.reveal.advance(now)
        };
        let disclosure = self.disclosure();

        scene.fill_rounded_rect(origin, size, APPROVAL_CARD_RADIUS, card);
        scene.push_clip_rounded(origin, size, APPROVAL_CARD_RADIUS);

        // ---- the head ---------------------------------------------------------
        let glyph_at = Point::new(
            origin.x + APPROVAL_CARD_PADDING,
            origin.y + APPROVAL_CARD_PADDING,
        );
        Self::draw_status_glyph(
            scene,
            self.status,
            glyph_at,
            APPROVAL_CARD_GLYPH_BOX,
            hue,
            spinner_angle(now, reduce_motion),
        );
        if self.status.is_busy() && !reduce_motion {
            ctx.request_frame_paced();
        }

        let column_x = origin.x + self.column_x();
        let mut right = origin.x + size.width - APPROVAL_CARD_PADDING;
        if let Some(rect) = self.dismiss_rect() {
            let at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let hovered = self.hovered == Some(ApprovalCardAction::Dismiss);
            draw_cross(
                scene,
                at,
                APPROVAL_CARD_DISMISS_BOX,
                if hovered {
                    palette.plain
                } else {
                    palette.comment
                },
            );
            if self.focused == ApprovalCardAction::Dismiss && ctx.has_focus() {
                draw_focus_ring(
                    scene,
                    at,
                    rect.size(),
                    APPROVAL_CARD_DISMISS_BOX / 2.0,
                    0.0,
                    ring,
                );
            }
            right -= APPROVAL_CARD_DISMISS_BOX + style::GAP_MD;
        }

        let chip_size = self.chip.size();
        let chip_width = chip_size.width + APPROVAL_CARD_CHIP_PADDING_X * 2.0;
        let chip_at = Point::new(right - chip_width, origin.y + APPROVAL_CARD_PADDING);
        let chip_box = Size::new(chip_width, APPROVAL_CARD_CHIP_HEIGHT);
        scene.fill_rounded_rect(
            chip_at,
            chip_box,
            APPROVAL_CARD_CHIP_HEIGHT / 2.0,
            with_alpha(hue, APPROVAL_CARD_CHIP_FILL_ALPHA),
        );
        crate::components::popover::paint_panel_hairline(
            scene,
            chip_at,
            chip_box,
            APPROVAL_CARD_CHIP_HEIGHT / 2.0,
            with_alpha(hue, APPROVAL_CARD_CHIP_BORDER_ALPHA),
        );
        self.chip.paint(
            Point::new(
                chip_at.x + APPROVAL_CARD_CHIP_PADDING_X,
                chip_at.y + (APPROVAL_CARD_CHIP_HEIGHT - chip_size.height) / 2.0,
            ),
            hue,
            scene,
        );

        // The title paints under a clip of exactly its reserved column, so
        // agent-supplied text can never reach the status chip beside it — and
        // an unbreakable run that overran the column says so with a trailing
        // cue rather than being cut in silence.
        let title_size = self.title.size();
        let head_column = self.head_column_width();
        let head_at = Point::new(column_x, origin.y + APPROVAL_CARD_PADDING);
        scene.push_clip(head_at, Size::new(head_column, self.head_height));
        let title_at = Point::new(
            column_x,
            head_at.y + (self.head_height - title_size.height) / 2.0,
        );
        self.title.paint(title_at, palette.plain, scene);
        if self.title_truncated {
            let marker = self.head_marker.size();
            self.head_marker.paint(
                Point::new(
                    title_at.x + (head_column - marker.width).max(0.0),
                    title_at.y + (title_size.height - marker.height).max(0.0),
                ),
                palette.plain,
                scene,
            );
        }
        scene.pop_clip();

        // ---- the body, under a clip of exactly the disclosed band -------------
        let head_bottom = origin.y + APPROVAL_CARD_PADDING + self.head_height;
        let body = self.body_height();
        if body > 0.5 {
            scene.push_clip(
                Point::new(origin.x, head_bottom),
                Size::new(size.width, body),
            );
            let alpha = disclosure as f32;
            if alpha < 1.0 {
                scene.push_layer(
                    Point::new(origin.x, head_bottom),
                    Size::new(size.width, body),
                    alpha,
                );
            }
            let mut y = head_bottom;
            if let Some(description) = &self.description {
                y += APPROVAL_CARD_DESCRIPTION_GAP;
                description.paint(Point::new(column_x, y), palette.comment, scene);
                y += self.description_height;
            }
            if let Some(pod) = &mut self.content {
                let _ = y;
                pod.paint_child(ctx, scene);
            }
            for (action, label) in &self.buttons {
                let Some(rect) = self.button_rect(*action) else {
                    continue;
                };
                let at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
                let hovered = self.hovered == Some(*action);
                let radius =
                    style::resolve_radius(style::RADIUS_CONTROL, rect.width(), rect.height());
                let (fill, ink, hairline) = match action {
                    ApprovalCardAction::Approve => (Some(palette.plain), card, None),
                    ApprovalCardAction::RequestChanges => (
                        Some(background),
                        palette.plain,
                        Some(with_alpha(palette.comment, 0.6)),
                    ),
                    _ => (
                        hovered.then(|| with_alpha(palette.plain, style::HOVER_WASH_ALPHA)),
                        palette.comment,
                        None,
                    ),
                };
                if let Some(fill) = fill {
                    let fill = if hovered && *action == ApprovalCardAction::Approve {
                        with_alpha(fill, fill.components[3] * style::HOVER_SOLID_ALPHA)
                    } else {
                        fill
                    };
                    scene.fill_rounded_rect(at, rect.size(), radius, fill);
                }
                if let Some(hairline) = hairline {
                    crate::components::popover::paint_panel_hairline(
                        scene,
                        at,
                        rect.size(),
                        radius,
                        hairline,
                    );
                }
                let measured = label.size();
                label.paint(
                    Point::new(
                        at.x + (rect.width() - measured.width) / 2.0,
                        at.y + (APPROVAL_CARD_BUTTON_HEIGHT - measured.height) / 2.0,
                    ),
                    ink,
                    scene,
                );
                if self.focused == *action && ctx.has_focus() {
                    draw_focus_ring(scene, at, rect.size(), radius, style::BORDER_WIDTH, ring);
                }
            }
            if alpha < 1.0 {
                scene.pop_layer();
            }
            scene.pop_clip();
        }

        // ---- the recorded outcome, arriving as the body leaves ----------------
        let outcome = self.outcome_height();
        if outcome > 0.5 {
            let alpha = (1.0 - disclosure) as f32;
            scene.push_clip(
                Point::new(origin.x, head_bottom + body),
                Size::new(size.width, outcome),
            );
            if alpha < 1.0 {
                scene.push_layer(
                    Point::new(origin.x, head_bottom + body),
                    Size::new(size.width, outcome),
                    alpha,
                );
            }
            self.result.paint(
                Point::new(column_x, head_bottom + body + APPROVAL_CARD_DESCRIPTION_GAP),
                palette.comment,
                scene,
            );
            if alpha < 1.0 {
                scene.pop_layer();
            }
            scene.pop_clip();
        }
        scene.pop_clip();

        // The reveal feeds the reported height, so a bare frame request would let
        // the collapse freeze on the intra-frame layout skip. Ask for layout
        // while it moves, and once more on the frame the value changed — which
        // covers the landing frame and the `reduce_motion` snap.
        if moving || self.reveal.value() != before {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            if let Some(pod) = &mut self.content {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The detail block is routed first, so an interactive body wins over the
        // card's own fallback handling.
        if let Some(pod) = &mut self.content
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
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
        let label = format!("{} — {}", self.title_text, self.status.label());
        ctx.push_node(Role::Group, |node| {
            node.set_label(label.as_str());
            node.set_expanded(self.status.is_interactive());
        });
        if self.status.is_interactive() {
            for (_, label) in &self.buttons {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(label.content());
                    if self.decisions_locked() {
                        node.set_disabled();
                    } else {
                        node.add_action(Action::Click);
                    }
                });
            }
            // Only a disclosed detail block is published: a collapsed one is
            // `inert` and `aria-hidden` upstream, and here it is not routed to.
            if let Some(pod) = &self.content {
                pod.semantics_child(ctx);
            }
        } else {
            ctx.push_node(Role::Status, |node| {
                node.set_label(self.result_text.as_str());
            });
        }
        if self.dismissible {
            ctx.push_node(Role::Button, |node| {
                node.set_label(ApprovalCardAction::Dismiss.label());
                node.add_action(Action::Click);
            });
        }
    }

    visit_children!(content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, CornerRadii, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
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
    struct Reviewed {
        action: Option<ApprovalCardAction>,
        actions: u32,
        dismissals: u32,
        action_id: Option<(ApprovalCardAction, Option<String>)>,
        action_id_calls: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view(status: ApprovalCardStatus) -> ApprovalCardView<Reviewed> {
        approval_card::<Reviewed>()
            .title("Apply these changes?")
            .description("The agent rewrote three files and wants to commit them.")
            .content_keyed("changeset-7", text("src/lib.rs, src/main.rs, README.md"))
            .status(status)
            .dismissible(true)
            .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                s.action = Some(action);
                s.actions += 1;
                if action == ApprovalCardAction::Dismiss {
                    s.dismissals += 1;
                }
            })
            .on_action_with_id(
                |s: &mut Reviewed, action: ApprovalCardAction, id: Option<String>| {
                    s.action_id = Some((action, id));
                    s.action_id_calls += 1;
                },
            )
    }

    fn build(v: &ApprovalCardView<Reviewed>) -> ApprovalCardWidget {
        let mut counter = 0u64;
        View::<Reviewed>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ApprovalCardWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(420.0, 900.0)),
        )
    }

    fn laid_out(status: ApprovalCardStatus) -> (ApprovalCardWidget, Size) {
        let mut w = build(&view(status));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut ApprovalCardWidget,
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

    fn rebuild(w: &mut ApprovalCardWidget, from: ApprovalCardStatus, to: ApprovalCardStatus) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Reviewed>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ApprovalCardWidget, size: Size, event: &InputEvent, state: &mut Reviewed) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn press(w: &mut ApprovalCardWidget, size: Size, at: Point, state: &mut Reviewed) {
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

    /// The model: every status carries its word, the two interactive ones are
    /// the ones that disclose, and only `Pending` accepts a decision.
    #[test]
    fn every_status_carries_its_word_and_only_pending_accepts_a_decision() {
        assert_eq!(ApprovalCardStatus::Pending.label(), "Input required");
        assert_eq!(ApprovalCardStatus::Submitting.label(), "Submitting");
        assert_eq!(ApprovalCardStatus::Approved.label(), "Approved");
        assert_eq!(ApprovalCardStatus::Rejected.label(), "Rejected");
        assert_eq!(
            ApprovalCardStatus::ChangesRequested.label(),
            "Changes requested"
        );
        assert_eq!(ApprovalCardStatus::Answered.label(), "Response submitted");

        assert!(ApprovalCardStatus::default().is_interactive());
        assert!(ApprovalCardStatus::Submitting.is_interactive());
        assert!(ApprovalCardStatus::Submitting.is_busy());
        assert!(!ApprovalCardStatus::Submitting.accepts_decision());
        assert!(ApprovalCardStatus::Pending.accepts_decision());
        for status in [
            ApprovalCardStatus::Approved,
            ApprovalCardStatus::Rejected,
            ApprovalCardStatus::ChangesRequested,
            ApprovalCardStatus::Answered,
        ] {
            assert!(!status.is_interactive(), "{status:?} is settled");
            assert!(!status.accepts_decision());
        }

        assert!(ApprovalCardAction::Approve.is_decision());
        assert!(ApprovalCardAction::RequestChanges.is_decision());
        assert!(ApprovalCardAction::Reject.is_decision());
        assert!(
            !ApprovalCardAction::Dismiss.is_decision(),
            "a dismiss closes rather than answers"
        );
        assert_eq!(
            ApprovalCardAction::RequestChanges.label(),
            "Request changes"
        );
    }

    /// The hues group the six statuses the way upstream's badge classes do.
    #[test]
    fn the_status_hues_group_the_lifecycle() {
        let theme = crate::theme();
        let hue = |s| ApprovalCardWidget::status_hue(s, Some(&theme));
        let tokens = BeuiTokens::resolve(Some(&theme));
        assert_eq!(hue(ApprovalCardStatus::Pending), tokens.warning);
        assert_eq!(
            hue(ApprovalCardStatus::ChangesRequested),
            hue(ApprovalCardStatus::Pending)
        );
        assert_eq!(hue(ApprovalCardStatus::Approved), tokens.success);
        assert_eq!(
            hue(ApprovalCardStatus::Answered),
            hue(ApprovalCardStatus::Approved)
        );
        assert_eq!(hue(ApprovalCardStatus::Rejected), theme.scheme().error);
        assert_eq!(
            ApprovalCardWidget::status_hue(ApprovalCardStatus::Rejected, None),
            BEUI_LIGHT.destructive,
            "the unthemed fallback is the vendored table"
        );
    }

    /// The whole chrome paints: the card, the glyph, the chip, the title, the
    /// description, the detail block and three buttons.
    #[test]
    fn a_pending_card_paints_its_head_body_and_actions() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        // card, chip, approve fill, request-changes fill (reject is a ghost).
        assert!(rec.rounded.len() >= 4, "filled {} boxes", rec.rounded.len());
        assert_eq!(
            rec.clips.len(),
            3,
            "the card clip, the head column's clip and the body clip"
        );
        assert!(rec.strokes >= 3, "the question glyph, the chip hairline");
        assert!(
            rec.transforms.is_empty(),
            "nothing rotates in a pending card"
        );
        // title, chip, description, detail block, three button labels.
        assert!(rec.inks.len() >= 7, "painted {} runs", rec.inks.len());
    }

    /// Answering collapses the body into the recorded outcome: the card
    /// shrinks, the body clip goes and the outcome clip arrives.
    #[test]
    fn answering_collapses_the_card_into_its_outcome() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        let interactive = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.disclosure(), 1.0);
        assert_eq!(w.outcome_height(), 0.0);

        rebuild(
            &mut w,
            ApprovalCardStatus::Pending,
            ApprovalCardStatus::Approved,
        );
        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a collapsing card must ask for relayout");

        let mut mid_layers = Vec::new();
        for step in 1..=40 {
            let (rec, _, _) = paint_at(&mut w, size, None, 100.0 + f64::from(step) * 25.0);
            mid_layers.extend(rec.layers.iter().copied());
            layout(&mut w);
        }
        assert!(
            mid_layers.iter().any(|a| *a > 0.0 && *a < 1.0),
            "the two halves crossfaded: {mid_layers:?}"
        );

        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(w.disclosure(), 0.0);
        let settled = layout(&mut w).height;
        assert!(settled < interactive, "the card never collapsed");
        let (rec, _, still) = paint_at(&mut w, size, None, 5_100.0);
        assert!(!still, "a settled card asks for no more layout");
        assert_eq!(
            rec.clips.len(),
            3,
            "the card clip, the head column's clip and the outcome clip"
        );
        assert!(w.button_rect(ApprovalCardAction::Approve).is_none());
    }

    /// Re-passing the status already being flown toward must not restart the
    /// collapse.
    #[test]
    fn a_redundant_rebuild_does_not_restart_the_collapse() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        rebuild(
            &mut w,
            ApprovalCardStatus::Pending,
            ApprovalCardStatus::Rejected,
        );
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 160.0);
        let mid = w.reveal.value();
        rebuild(
            &mut w,
            ApprovalCardStatus::Rejected,
            ApprovalCardStatus::Rejected,
        );
        paint_at(&mut w, size, None, 200.0);
        assert!(
            w.reveal.value() < mid,
            "the clock kept running: {mid} -> {}",
            w.reveal.value()
        );
    }

    /// A decision fires exactly once per interactive episode, and the card
    /// never writes its own status.
    #[test]
    fn a_decision_fires_exactly_once_per_interactive_episode() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();

        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        press(&mut w, size, approve, &mut state);
        assert_eq!(state.action, Some(ApprovalCardAction::Approve));
        assert_eq!(state.actions, 1);
        assert_eq!(w.status, ApprovalCardStatus::Pending, "never self-written");

        // A second press before the app has responded is refused, whichever
        // button it lands on.
        press(&mut w, size, approve, &mut state);
        let reject = w.button_rect(ApprovalCardAction::Reject).unwrap().center();
        press(&mut w, size, reject, &mut state);
        assert_eq!(state.actions, 1, "the latch held");

        rebuild(
            &mut w,
            ApprovalCardStatus::Pending,
            ApprovalCardStatus::Submitting,
        );
        assert!(!w.decided, "the latch cleared with the status");
        press(&mut w, size, approve, &mut state);
        assert_eq!(state.actions, 1, "but `submitting` accepts no decision");
    }

    /// A payload swap delivered between `Down` and `Up` disarms the in-flight
    /// press instead of firing an action against whatever is now displayed —
    /// the tool-approval bait-and-switch.
    #[test]
    fn a_payload_swap_disarms_an_in_flight_press() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();

        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, approve),
            &mut state,
        );
        assert!(w.captured.is_some(), "the press armed");

        // A different review lands mid-press — same status, different
        // content, exactly what a streaming agent surface can deliver.
        let swapped = approval_card::<Reviewed>()
            .title("Apply these OTHER changes?")
            .description("A different agent turn swapped the review mid-press.")
            .content_keyed("changeset-7", text("src/other.rs"))
            .status(ApprovalCardStatus::Pending)
            .dismissible(true)
            .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                s.action = Some(action);
                s.actions += 1;
            });
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Reviewed>::rebuild(
            &swapped,
            &view(ApprovalCardStatus::Pending),
            &mut w,
            &mut ctx,
        );

        assert!(w.captured.is_none(), "the payload swap disarmed the press");
        assert_eq!(
            w.focused,
            ApprovalCardAction::Reject,
            "focus re-armed at the safe default"
        );

        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, approve),
            &mut state,
        );
        assert_eq!(
            state.actions, 0,
            "the swapped-in review never received an action"
        );
    }

    /// An action carries the identity of the review actually shown, not a
    /// bare enum the app must guess an owner for.
    #[test]
    fn an_action_reports_the_id_of_the_review_shown() {
        let mut w = build(
            &approval_card::<Reviewed>()
                .id("review-7")
                .title("Apply these changes?")
                .status(ApprovalCardStatus::Pending)
                .on_action_with_id(
                    |s: &mut Reviewed, action: ApprovalCardAction, id: Option<String>| {
                        s.action_id = Some((action, id));
                        s.action_id_calls += 1;
                    },
                ),
        );
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap();
        press(&mut w, size, approve.center(), &mut state);
        assert_eq!(
            state.action_id,
            Some((ApprovalCardAction::Approve, Some("review-7".to_owned())))
        );
        assert_eq!(state.action_id_calls, 1);
    }

    /// With no explicit id there is no identity to report: the callback gets
    /// `None` rather than the title, which two open reviews can share and so
    /// cannot correlate consent.
    #[test]
    fn an_action_without_an_explicit_id_reports_no_identity() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        press(&mut w, size, approve, &mut state);
        assert_eq!(
            state.action_id,
            Some((ApprovalCardAction::Approve, None)),
            "the title is not an identity"
        );
        assert_eq!(state.action_id_calls, 1);
    }

    /// A fresh review delivered under an unchanged `Pending` status opens a
    /// new consent episode: the latch the previous answer raised is released,
    /// so the review now displayed can be answered — exactly once, and the
    /// action reports the *new* review's identity.
    #[test]
    fn a_review_swapped_in_at_the_same_status_can_still_be_answered() {
        let review = |id: &'static str, title: &'static str| {
            approval_card::<Reviewed>()
                .id(id)
                .title(title)
                .status(ApprovalCardStatus::Pending)
                .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                    s.action = Some(action);
                    s.actions += 1;
                })
                .on_action_with_id(
                    |s: &mut Reviewed, action: ApprovalCardAction, id: Option<String>| {
                        s.action_id = Some((action, id));
                        s.action_id_calls += 1;
                    },
                )
        };

        let first = review("review-a", "Apply these changes?");
        let mut w = build(&first);
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();

        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        press(&mut w, size, approve, &mut state);
        assert_eq!(state.actions, 1);

        // The app has not moved the status yet — a queue advancing to its
        // next review while the answer round-trips.
        let second = review("review-b", "Apply these OTHER changes?");
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Reviewed>::rebuild(&second, &first, &mut w, &mut ctx);
        layout(&mut w);
        paint_at(&mut w, size, None, 16.0);

        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        press(&mut w, size, approve, &mut state);
        assert_eq!(state.actions, 2, "the swapped-in review was answerable");
        assert_eq!(
            state.action_id,
            Some((ApprovalCardAction::Approve, Some("review-b".to_owned()))),
            "and the action named the review it answered"
        );

        // Still one decision per episode: the released latch re-armed.
        press(&mut w, size, approve, &mut state);
        assert_eq!(state.actions, 2);
    }

    /// A detail block with no identity is outside the consent episode, so a
    /// card showing one answers no decision until it is keyed.
    #[test]
    fn an_unkeyed_detail_block_makes_decisions_unanswerable() {
        let card = |keyed: bool| {
            let base = approval_card::<Reviewed>()
                .title("Apply these changes?")
                .status(ApprovalCardStatus::Pending)
                .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                    s.action = Some(action);
                    s.actions += 1;
                });
            if keyed {
                base.content_keyed("changeset-7", text("src/lib.rs"))
            } else {
                base.content(text("src/lib.rs"))
            }
        };
        for (keyed, expected) in [(false, 0), (true, 1)] {
            let view = card(keyed);
            let mut w = build(&view);
            let size = layout(&mut w);
            paint_at(&mut w, size, None, 0.0);
            let mut state = Reviewed::default();
            let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
            dispatch(
                &mut w,
                size,
                &pointer(PointerPhase::Down, approve),
                &mut state,
            );
            dispatch(
                &mut w,
                size,
                &pointer(PointerPhase::Up, approve),
                &mut state,
            );
            assert_eq!(
                state.actions, expected,
                "keyed = {keyed}: an unkeyed block answers nothing, a keyed one once"
            );
        }
    }

    /// A streaming detail block repaints and relayouts on every frame it
    /// grows, and none of that is consent-bearing: the press that was armed
    /// before the stream still completes, and the roving cursor stays where
    /// the user put it.
    #[test]
    fn a_streaming_detail_block_never_disarms_the_card() {
        let card = |body: &str| {
            approval_card::<Reviewed>()
                .title("Apply these changes?")
                .description("The agent rewrote three files and wants to commit them.")
                // One change set streaming in: the same key throughout.
                .content_keyed("changeset-7", text(body.to_owned()))
                .status(ApprovalCardStatus::Pending)
                .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                    s.action = Some(action);
                    s.actions += 1;
                })
        };
        let stream = [
            "src/lib.rs, src/m",
            "src/lib.rs, src/main.rs",
            "src/lib.rs, src/main.rs, READ",
            "src/lib.rs, src/main.rs, README.md",
        ];

        // The pointer leg: a press armed before the stream still completes.
        let mut first = card("src/lib.rs");
        let mut w = build(&first);
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, approve),
            &mut state,
        );
        for (step, body) in stream.iter().enumerate() {
            let next = card(body);
            let mut counter = 0u64;
            let mut ctx = BuildCtx::new(&mut counter);
            View::<Reviewed>::rebuild(&next, &first, &mut w, &mut ctx);
            first = next;
            layout(&mut w);
            paint_at(&mut w, size, None, 16.0 * (step + 1) as f64);
        }
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, approve),
            &mut state,
        );
        assert_eq!(
            state.actions, 1,
            "the press completed across the streaming rebuilds"
        );
        assert_eq!(state.action, Some(ApprovalCardAction::Approve));

        // The keyboard leg: the roving cursor the user moved is still there.
        let mut first = card("src/lib.rs");
        let mut w = build(&first);
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        dispatch(&mut w, size, &key(NamedKey::ArrowLeft), &mut state);
        for body in stream {
            let next = card(body);
            let mut counter = 0u64;
            let mut ctx = BuildCtx::new(&mut counter);
            View::<Reviewed>::rebuild(&next, &first, &mut w, &mut ctx);
            first = next;
            layout(&mut w);
            paint_at(&mut w, size, None, 16.0);
        }
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(
            state.action,
            Some(ApprovalCardAction::RequestChanges),
            "the cursor was reset to the safe default mid-stream"
        );
    }

    /// A detail block the caller labels *is* consent-bearing: a changed
    /// `content_key` opens a new episode and disarms an in-flight press, the
    /// same way a changed title does.
    #[test]
    fn a_changed_content_key_disarms_an_in_flight_press() {
        let card = |key: &'static str| {
            approval_card::<Reviewed>()
                .title("Apply these changes?")
                .content(text("src/lib.rs"))
                .content_key(key)
                .status(ApprovalCardStatus::Pending)
                .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                    s.action = Some(action);
                    s.actions += 1;
                })
        };
        let first = card("diff-rev-1");
        let mut w = build(&first);
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, approve),
            &mut state,
        );

        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Reviewed>::rebuild(&card("diff-rev-2"), &first, &mut w, &mut ctx);
        layout(&mut w);
        paint_at(&mut w, size, None, 16.0);

        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, approve),
            &mut state,
        );
        assert_eq!(
            state.actions, 0,
            "the release fired against a detail block that had changed"
        );
    }

    /// Agent-supplied title text is bounded by a column that reserves the
    /// status chip's and the dismiss affordance's boxes, painted under a clip
    /// of exactly that column, and marks its own overflow — so it can neither
    /// deface the status indicator nor be cut in silence.
    #[test]
    fn a_long_title_is_bounded_away_from_the_chip_and_marks_the_cut() {
        let mut short = build(
            &approval_card::<Reviewed>()
                .title("Apply these changes?")
                .dismissible(true),
        );
        let short_size = layout(&mut short);
        let (short_rec, _, _) = paint_at(&mut short, short_size, None, 0.0);

        // One unbreakable token, so the run overflows its bound rather than
        // wrapping — the case a clip would otherwise swallow in silence.
        let overlong = format!("Apply{}", "a".repeat(300));
        let mut long = build(
            &approval_card::<Reviewed>()
                .title(overlong)
                .dismissible(true),
        );
        let long_size = layout(&mut long);
        let (long_rec, _, _) = paint_at(&mut long, long_size, None, 0.0);

        let chip = long_rec
            .rounded
            .iter()
            .find(|(_, s, _)| s.height == APPROVAL_CARD_CHIP_HEIGHT)
            .expect("the status chip painted");
        let (head_at, head_size) = long_rec.clips[1];
        assert!(
            head_at.x + head_size.width <= chip.0.x,
            "the head column ({head_at:?} + {head_size:?}) reaches the chip at {:?}",
            chip.0
        );
        assert!(
            long_rec.inks.len() > short_rec.inks.len(),
            "the cut title painted an extra glyph run for the marker"
        );
    }

    /// Keyboard activation with no explicit selection lands on the safe,
    /// least-permissive decision — never `Approve`.
    #[test]
    fn the_default_focus_is_the_safe_decision() {
        let (w, _) = laid_out(ApprovalCardStatus::Pending);
        assert_eq!(w.focused, ApprovalCardAction::Reject);
    }

    /// A dismiss is not a decision: it fires from any status and is never
    /// latched.
    #[test]
    fn a_dismiss_is_never_latched() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        let dismiss = w.dismiss_rect().unwrap().center();
        press(&mut w, size, dismiss, &mut state);
        press(&mut w, size, dismiss, &mut state);
        assert_eq!(state.dismissals, 2);

        rebuild(
            &mut w,
            ApprovalCardStatus::Pending,
            ApprovalCardStatus::Approved,
        );
        for step in 0..=40 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        press(&mut w, size, dismiss, &mut state);
        assert_eq!(state.dismissals, 3, "still reachable once answered");
    }

    /// A press released off its armed button fires nothing — the up-inside
    /// rule.
    #[test]
    fn a_press_released_off_target_reports_nothing() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, approve),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(approve.x, 4.0)),
            &mut state,
        );
        assert_eq!(state.actions, 0);
    }

    /// Keyboard: the arrows rove every offered button plus the dismiss, and the
    /// activation keys fire the one under the cursor. Default focus starts on
    /// the safe (`Reject`) decision, not `Approve` — see
    /// `the_default_focus_is_the_safe_decision`.
    #[test]
    fn the_arrows_rove_every_trigger_and_enter_fires_it() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        assert_eq!(w.targets().len(), 4, "three decisions plus the dismiss");
        assert_eq!(w.focused, ApprovalCardAction::Reject, "the safe default");

        dispatch(&mut w, size, &key(NamedKey::ArrowLeft), &mut state);
        assert_eq!(w.focused, ApprovalCardAction::RequestChanges);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.action, Some(ApprovalCardAction::RequestChanges));
        assert_eq!(state.actions, 1);
        dispatch(&mut w, size, &key(NamedKey::ArrowLeft), &mut state);
        assert_eq!(w.focused, ApprovalCardAction::Approve);
    }

    /// Dropping the optional buttons drops them from the row and the roving
    /// order, and the approve label is overridable.
    #[test]
    fn the_offered_buttons_and_the_approve_label_are_the_callers() {
        let mut w = build(
            &approval_card::<Reviewed>()
                .title("Ship it?")
                .approve_label("Ship")
                .request_changes(false)
                .reject(false)
                .on_action(|s: &mut Reviewed, _| s.actions += 1),
        );
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.buttons.len(), 1);
        assert_eq!(w.buttons[0].1.content(), "Ship");
        assert!(w.button_rect(ApprovalCardAction::Reject).is_none());
        assert_eq!(w.targets(), vec![ApprovalCardAction::Approve]);
        assert!(w.dismiss_rect().is_none(), "not dismissible by default");
    }

    /// The outcome line is the caller's when given and the status's word
    /// otherwise, and it re-shapes when either changes.
    #[test]
    fn the_outcome_line_falls_back_to_the_status_word() {
        let plain = approval_card::<Reviewed>().status(ApprovalCardStatus::Rejected);
        assert_eq!(plain.result_text(), "Rejected");
        let spelled = approval_card::<Reviewed>()
            .status(ApprovalCardStatus::Rejected)
            .result("Rejected by Ed at 14:02");
        assert_eq!(spelled.result_text(), "Rejected by Ed at 14:02");

        let mut w = build(&view(ApprovalCardStatus::Pending));
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.result_text, "Input required");
        rebuild(
            &mut w,
            ApprovalCardStatus::Pending,
            ApprovalCardStatus::Answered,
        );
        assert_eq!(w.result_text, "Response submitted");
    }

    /// A card with no detail block lays out and routes as a leaf.
    #[test]
    fn a_card_with_no_detail_block_is_a_leaf() {
        let mut w = build(
            &approval_card::<Reviewed>()
                .title("Continue?")
                .on_action(|s: &mut Reviewed, _| s.actions += 1),
        );
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert!(w.content.is_none());
        assert_eq!(w.content_height, 0.0);
        assert_eq!(w.description_height, 0.0);
        let mut state = Reviewed::default();
        let approve = w.button_rect(ApprovalCardAction::Approve).unwrap().center();
        press(&mut w, size, approve, &mut state);
        assert_eq!(state.actions, 1);
    }

    /// `reduce_motion` lands the collapse on the frame it is painted and
    /// freezes the submitting spinner.
    #[test]
    fn reduce_motion_lands_the_collapse_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, Some(&theme), 0.0);
        rebuild(
            &mut w,
            ApprovalCardStatus::Pending,
            ApprovalCardStatus::Approved,
        );
        let (rec, _, needs_layout) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(needs_layout, "the snap still publishes its new height");
        assert_eq!(w.disclosure(), 0.0);
        assert!(rec.layers.is_empty(), "nothing is mid-crossfade");

        layout(&mut w);
        let (_, needs_frame, still) = paint_at(&mut w, size, Some(&theme), 120.0);
        assert!(!needs_frame && !still, "a snapped card asks for nothing");
    }

    /// A submitting card turns its glyph spinner off a paced tick.
    #[test]
    fn a_submitting_card_paces_its_spinner() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Submitting);
        paint_at(&mut w, size, None, 0.0);
        layout(&mut w);
        let (rec, needs_frame, _) = paint_at(&mut w, size, None, 50.0);
        assert!(needs_frame, "the spinner asks for its next tick");
        assert_eq!(rec.transforms.len(), 1, "one rotated spinner");
    }

    // ---- Typeface: the card's text follows the live theme -------------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(420.0, 600.0);

    /// A pending card over glyph-free content, so every painted run is the
    /// card's own: title, chip, description and the decision buttons.
    fn pending_probe(_: &mut ()) -> ApprovalCardView<()> {
        approval_card::<()>()
            .title("Apply these changes?")
            .description("The agent wants to commit them.")
            .content_keyed("changeset", frust::SizedBox::<()>(Some(40.0), Some(20.0)))
    }

    /// An answered card: title, chip and the outcome line.
    fn answered_probe(_: &mut ()) -> ApprovalCardView<()> {
        approval_card::<()>()
            .title("Apply these changes?")
            .status(ApprovalCardStatus::Approved)
            .result("Approved at 14:02")
    }

    #[test]
    fn card_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the pending card", pending_probe, PROBE_WINDOW);
        assert_paints_only_in_geist("the answered card", answered_probe, PROBE_WINDOW);
    }

    #[test]
    fn card_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the pending card", pending_probe, PROBE_WINDOW);
        assert_follows_a_live_family_swap("the answered card", answered_probe, PROBE_WINDOW);
    }
}
