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
//! # One decision per interactive episode
//!
//! [`ApprovalCardAction::Approve`], [`RequestChanges`](ApprovalCardAction) and
//! [`Reject`](ApprovalCardAction) fire **exactly once** per interactive
//! episode: the press is up-inside, and the widget then latches until the app
//! moves the status, which is what it is expected to do in the handler. The
//! latch clears on any status change.
//! [`Dismiss`](ApprovalCardAction) is deliberately **not** latched — it closes
//! the surface rather than answering it, and is offered in every status.
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
use crate::agents::tool_approval::{WrappedRun, prose_style, strong_style};
use crate::motion::Ramp;
use crate::press::{Lane, draw_focus_ring, inside, is_activation_key, presses};
use crate::style::{self, with_alpha};
use crate::text::LabelRun;
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
    result: Option<String>,
    status: ApprovalCardStatus,
    approve_label: String,
    request_changes: bool,
    reject: bool,
    dismissible: bool,
    on_action: Option<OnArg<State, ApprovalCardAction>>,
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
        result: None,
        status: ApprovalCardStatus::default(),
        approve_label: ApprovalCardAction::Approve.label().to_owned(),
        request_changes: true,
        reject: true,
        dismissible: false,
        on_action: None,
    }
}

impl<State: 'static> ApprovalCardView<State> {
    /// The card's heading (`title`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// The paragraph under the heading (`description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The detail block between the description and the action row
    /// (`children`) — a diff, a code block, a table, anything.
    pub fn content<V: View<State>>(mut self, content: V) -> Self {
        self.content = Some(any(content));
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

    /// Report the user's action. A decision fires at most once per interactive
    /// episode — see the [module docs](self).
    pub fn on_action<F: Fn(&mut State, ApprovalCardAction) + 'static>(
        mut self,
        on_action: F,
    ) -> Self {
        self.on_action = Some(Rc::new(on_action));
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
    title: LabelRun,
    title_text: String,
    description: Option<WrappedRun>,
    description_text: Option<String>,
    result: WrappedRun,
    result_text: String,
    chip: LabelRun,
    buttons: Vec<(ApprovalCardAction, LabelRun)>,
    content: Option<ChildPod>,
    status: ApprovalCardStatus,
    approve_label: String,
    request_changes: bool,
    reject: bool,
    dismissible: bool,
    /// The body disclosure: `1` interactive, `0` collapsed into the outcome.
    reveal: Lane,
    /// Raised once a decision has been reported for the current interactive
    /// episode; cleared by any status change.
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
    on_action: Option<ErasedArgCallback<ApprovalCardAction>>,
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

impl<State: 'static> View<State> for ApprovalCardView<State> {
    type Element = ApprovalCardWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ApprovalCardWidget {
        ApprovalCardWidget {
            title: LabelRun::new(self.title.clone()),
            title_text: self.title.clone(),
            description: self.description.as_deref().map(WrappedRun::new),
            description_text: self.description.clone(),
            result: WrappedRun::new(self.result_text()),
            result_text: self.result_text(),
            chip: LabelRun::new(self.status.label()),
            buttons: build_buttons(self),
            content: self.content.as_ref().map(|view| build_child(view, ctx)),
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
            focused: ApprovalCardAction::Approve,
            hovered: None,
            captured: None,
            on_action: self.on_action.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ApprovalCardWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_action = self.on_action.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        if element.title_text != self.title {
            element.title = LabelRun::new(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.description_text != self.description {
            element.description = self.description.as_deref().map(WrappedRun::new);
            element.description_text = self.description.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.approve_label != self.approve_label
            || element.request_changes != self.request_changes
            || element.reject != self.reject
        {
            element.approve_label = self.approve_label.clone();
            element.request_changes = self.request_changes;
            element.reject = self.reject;
            element.buttons = build_buttons(self);
            element.focused = ApprovalCardAction::Approve;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.dismissible != self.dismissible {
            element.dismissible = self.dismissible;
            flags |= ChangeFlags::PAINT;
        }
        if element.status != self.status {
            element.status = self.status;
            element.chip = LabelRun::new(self.status.label());
            // A new episode: the one-decision latch clears with the status that
            // raised it.
            element.decided = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
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
                flags |= rebuild_child(before, after, pod, ctx);
            }
            (Some(before), None, Some(pod)) => {
                teardown_child(before, pod, ctx);
                element.content = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (_, Some(after), None) => {
                element.content = Some(build_child(after, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
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
    fn activate(&mut self, ctx: &mut EventCtx, target: ApprovalCardAction) {
        if target.is_decision() && (self.decided || !self.status.accepts_decision()) {
            return;
        }
        if let Some(on_action) = self.on_action.as_mut() {
            if target.is_decision() {
                self.decided = true;
            }
            on_action(ctx, target);
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

        let title = self
            .title
            .layout(ctx, &strong_style(APPROVAL_CARD_TITLE_SIZE));
        self.head_height = title
            .height
            .max(APPROVAL_CARD_TITLE_LINE)
            .max(APPROVAL_CARD_CHIP_HEIGHT);
        self.chip
            .layout(ctx, &strong_style(APPROVAL_CARD_CHIP_SIZE));
        for (_, label) in &mut self.buttons {
            label.layout(ctx, &strong_style(style::TEXT_XS));
        }

        self.description_height = match &mut self.description {
            Some(run) => run.layout(ctx, &prose_style(style::TEXT_SM), column).height,
            None => 0.0,
        };
        self.result_height = self
            .result
            .layout(ctx, &prose_style(style::TEXT_SM), column)
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

        let title_size = self.title.size();
        self.title.paint(
            Point::new(
                column_x,
                origin.y + APPROVAL_CARD_PADDING + (self.head_height - title_size.height) / 2.0,
            ),
            palette.plain,
            scene,
        );

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
                    if self.decided || !self.status.accepts_decision() {
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
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view(status: ApprovalCardStatus) -> ApprovalCardView<Reviewed> {
        approval_card::<Reviewed>()
            .title("Apply these changes?")
            .description("The agent rewrote three files and wants to commit them.")
            .content(text("src/lib.rs, src/main.rs, README.md"))
            .status(status)
            .dismissible(true)
            .on_action(|s: &mut Reviewed, action: ApprovalCardAction| {
                s.action = Some(action);
                s.actions += 1;
                if action == ApprovalCardAction::Dismiss {
                    s.dismissals += 1;
                }
            })
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
        assert_eq!(rec.clips.len(), 2, "the card clip plus the body clip");
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
        assert_eq!(rec.clips.len(), 2, "the card clip plus the outcome clip");
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
    /// activation keys fire the one under the cursor.
    #[test]
    fn the_arrows_rove_every_trigger_and_enter_fires_it() {
        let (mut w, size) = laid_out(ApprovalCardStatus::Pending);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Reviewed::default();
        assert_eq!(w.targets().len(), 4, "three decisions plus the dismiss");

        dispatch(&mut w, size, &key(NamedKey::ArrowRight), &mut state);
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
}
