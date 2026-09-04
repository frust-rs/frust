//! Ports beUI's `chat-app` agent-interface part — the catalog's flagship agent
//! surface.
//!
//! **Source:** `components/agents/chat-app.tsx` and its usage preview
//! `components/previews/agents/chat-app-usage.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `chat-app`: *"A complete agent conversation workspace composing
//! navigation, messages, streaming, planning, approvals, tools, code, diffs,
//! generated media, sources, and prompt input."*
//!
//! # What upstream's `chat-app.tsx` actually is
//!
//! A **shell**, not an assembled chat. The file is ninety lines: it wraps its
//! children in an `AnimatedSidebarProvider`, sets `--sidebar-width`, and mounts
//! one effect (`ShellFit`) that folds the sidebar away while the shell's own box
//! is narrower than [`CHAT_APP_MIN_DOCKED_WIDTH`]. The conversation the registry
//! entry describes — scroller, message rows, tool cards, prompt input — lives in
//! the **usage preview** beside it, which is where the streaming demo driver
//! lives too.
//!
//! This module ports both halves, and says which is which:
//!
//! * [`chat_app`] is the shell, faithfully: two panes, the docked width, the
//!   fold threshold, and upstream's rule that a *controlled* `open` disables the
//!   fit entirely.
//! * [`chat_conversation`] is the assembled surface from the usage preview,
//!   composed from the catalog's own parts —
//!   [`message_scroller`](crate::agents::message_scroller),
//!   [`message`](crate::agents::message),
//!   [`message_bubble`](crate::agents::message_bubble),
//!   [`streaming_response`](crate::agents::streaming_response) and
//!   [`prompt_input`](crate::agents::prompt_input).
//! * [`ChatModel`] and [`MockChatDriver`] are the preview's own demo state
//!   machine — submit, wait, stream, stop — lifted into a plain, testable type
//!   so the surface can be driven end to end without a network.
//!
//! # The model owns the conversation; the widgets own nothing
//!
//! [`ChatModel`] is app state, not widget state: a host keeps one in its
//! `Component::State`, mutates it from the callbacks
//! [`ChatConversationView`] reports through, and steps [`ChatModel::advance`]
//! from its own frame loop. Every widget below is controlled — the composer
//! never keeps its own text, the transcript never invents a message. That is the
//! same split upstream draws with `useState` in the preview and props in the
//! component.
//!
//! # Degradations against the web original
//!
//! - **A user bubble is [`Soft`](crate::agents::message_bubble::MessageBubbleVariant::Soft),
//!   not upstream's `solid`.** A solid bubble is `bg-foreground text-background`
//!   — an *inverted* ink its content must paint in. The bubble's content is an
//!   independent child that resolves its own ink from the theme at paint time,
//!   and the catalog's bubble publishes no inverted-ink seam for it, so a solid
//!   default here would paint on-surface ink on an on-surface fill. The variant
//!   is a builder knob ([`ChatConversationView::user_variant`]) so a host that
//!   supplies its own inked content can still ask for it.
//! - **The shell fold reports nothing.** Upstream's `ShellFit` calls `setOpen`,
//!   which reaches `onOpenChange`. The fold here is decided in `layout`, which
//!   carries no `EventCtx`, so it moves the shell's own flag silently — the same
//!   posture [`crate::agents::todo_list`]'s completion collapse takes. A host
//!   that must know owns `open` itself, which upstream also treats as the
//!   opt-out.
//! - **The preview's tool/approval/diff/media cards are not wired in.** The
//!   usage file mounts nine more agent parts inside its transcript; every one of
//!   them is a view the host can drop into [`ChatModel`]'s rows through
//!   [`ChatConversationView::extra_rows`] rather than something this module
//!   hard-codes.
//! - **No off-canvas drawer.** The folded sidebar's width goes to zero in place;
//!   upstream's `collapsible="offcanvas"` slides a portalled sheet. That is
//!   [`crate::components::animated_sidebar`]'s own recorded gap, and this shell
//!   inherits it.

use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Rect, Role, SemanticsCtx, Size, View, Widget, any,
    build_child, rebuild_child, route_event, teardown_child, visit_children,
};

use crate::agents::message::{MessageFrom, message};
use crate::agents::message_bubble::{MessageBubbleAlign, MessageBubbleVariant, message_bubble};
use crate::agents::message_scroller::message_scroller;
use crate::agents::prompt_input::prompt_input;
use crate::agents::streaming_response::{StreamingResponseStatus, streaming_response};
use crate::components::animated_sidebar::SIDEBAR_MORPH_SPRING;
use crate::motion::Ramp;
use crate::press::Lane;
use crate::style;
use crate::tokens::{BEUI_LIGHT, BeuiPalette};

/// The docked sidebar's width, in logical px (`sidebarWidth = "17rem"`).
pub const CHAT_APP_SIDEBAR_WIDTH: f64 = 272.0;

/// The shell width below which the sidebar folds away, in logical px
/// (`MIN_DOCKED_WIDTH = 600`).
pub const CHAT_APP_MIN_DOCKED_WIDTH: f64 = 600.0;

/// The shell's corner radius (`rounded-2xl`).
pub const CHAT_APP_RADIUS: f64 = style::RADIUS_2XL;

/// The transcript viewport's padding (`px-3 py-5`).
pub const CHAT_APP_VIEWPORT_PADDING_X: f64 = 12.0;
/// The transcript viewport's vertical padding (`py-5`).
pub const CHAT_APP_VIEWPORT_PADDING_Y: f64 = 20.0;

/// The padding around the composer.
pub const CHAT_APP_COMPOSER_PADDING: f64 = 12.0;

/// The initials the assistant's avatar shows when the host names no other.
pub const CHAT_APP_ASSISTANT_AVATAR: &str = "AI";

/// The initials the user's avatar shows when the host names no other.
pub const CHAT_APP_USER_AVATAR: &str = "You";

/// The mock driver's default reveal rate, in characters per second — the
/// preview's own `92`.
pub const CHAT_STREAM_RATE: f64 = 92.0;

/// The mock driver's default wait before the first token, in ms — the
/// preview's own `420`.
pub const CHAT_FIRST_TOKEN_MS: u64 = 420;

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

// ---- The model -------------------------------------------------------------

/// Who sent a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatRole {
    /// The person typing.
    #[default]
    User,
    /// The agent replying.
    Assistant,
}

impl ChatRole {
    /// The transcript row direction this role reads as.
    pub const fn from(self) -> MessageFrom {
        match self {
            ChatRole::User => MessageFrom::User,
            ChatRole::Assistant => MessageFrom::Assistant,
        }
    }
}

/// One turn of the conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    id: String,
    role: ChatRole,
    content: String,
    streaming: bool,
}

impl ChatMessage {
    /// A settled message from `role`.
    pub fn new(id: impl Into<String>, role: ChatRole, content: impl Into<String>) -> Self {
        ChatMessage {
            id: id.into(),
            role,
            content: content.into(),
            streaming: false,
        }
    }

    /// This message's identity, which the transcript diffs against.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Who sent it.
    pub fn role(&self) -> ChatRole {
        self.role
    }

    /// What it says so far.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Whether tokens are still arriving for it.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }
}

/// The preview's canned reply, revealed at a fixed rate after a fixed wait —
/// the demo driver, not a transport.
///
/// A real host swaps this for its own stream and calls
/// [`ChatModel::push_token`]/[`ChatModel::finish_reply`] instead of
/// [`ChatModel::advance`].
#[derive(Clone, Debug, PartialEq)]
pub struct MockChatDriver {
    reply: String,
    chars_per_second: f64,
    first_token_delay: Duration,
}

/// Create a driver that answers every prompt with `reply`, at
/// [`CHAT_STREAM_RATE`] after [`CHAT_FIRST_TOKEN_MS`].
pub fn mock_chat_driver(reply: impl Into<String>) -> MockChatDriver {
    MockChatDriver {
        reply: reply.into(),
        chars_per_second: CHAT_STREAM_RATE,
        first_token_delay: Duration::from_millis(CHAT_FIRST_TOKEN_MS),
    }
}

impl MockChatDriver {
    /// Reveal at `rate` characters per second instead.
    pub fn chars_per_second(mut self, rate: f64) -> Self {
        self.chars_per_second = rate.max(0.0);
        self
    }

    /// Wait `delay` before the first token instead.
    pub fn first_token_delay(mut self, delay: Duration) -> Self {
        self.first_token_delay = delay;
        self
    }

    /// The reply it streams.
    pub fn reply(&self) -> &str {
        &self.reply
    }

    /// How much of the reply has arrived `elapsed` into a run, in characters.
    fn revealed(&self, elapsed: Duration) -> usize {
        let streaming = elapsed.saturating_sub(self.first_token_delay);
        let chars = (streaming.as_secs_f64() * self.chars_per_second).floor();
        (chars.max(0.0) as usize).min(self.reply.chars().count())
    }
}

impl Default for MockChatDriver {
    fn default() -> Self {
        mock_chat_driver("Here is the patch: the validation gap is closed and the suite is green.")
    }
}

/// One reply in flight.
#[derive(Clone, Debug, PartialEq)]
struct ChatRun {
    /// How long the run has been going.
    elapsed: Duration,
    /// The assistant message this run is filling, once the first token has
    /// landed.
    reply_id: Option<String>,
}

/// The conversation an app owns: its turns, its composer text, and whichever
/// reply is in flight.
///
/// Plain state, no widget: every mutation is a method a host calls from an
/// event callback, and [`advance`](Self::advance) is stepped from the host's
/// own frame loop. See the [module docs](self)' *The model owns the
/// conversation*.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatModel {
    messages: Vec<ChatMessage>,
    input: String,
    run: Option<ChatRun>,
    driver: MockChatDriver,
    turns: u64,
}

impl ChatModel {
    /// An empty conversation driven by the default [`MockChatDriver`].
    pub fn new() -> Self {
        Self::default()
    }

    /// This conversation, answered by `driver`.
    pub fn with_driver(mut self, driver: MockChatDriver) -> Self {
        self.driver = driver;
        self
    }

    /// This conversation, seeded with `messages` (a transcript loaded from
    /// history).
    pub fn with_history(mut self, messages: Vec<ChatMessage>) -> Self {
        self.turns = messages.len() as u64;
        self.messages = messages;
        self
    }

    /// The turns so far, oldest first.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// The composer's text.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Replace the composer's text — what a host wires
    /// [`ChatConversationView::on_input`] to.
    pub fn set_input(&mut self, text: impl Into<String>) {
        self.input = text.into();
    }

    /// Whether a reply is pending or streaming, which is what puts the
    /// composer's button in its stop state.
    pub fn is_busy(&self) -> bool {
        self.run.is_some()
    }

    /// Whether a reply is *waiting* — accepted, but with no token yet.
    pub fn is_pending(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.reply_id.is_none())
    }

    /// The reply currently streaming, if any.
    pub fn streaming_message(&self) -> Option<&ChatMessage> {
        self.messages.iter().find(|message| message.streaming)
    }

    /// Send `text` as a user turn and start a reply.
    ///
    /// Refused — and reported as `false` — for an empty or whitespace-only
    /// prompt, or while a reply is already in flight, which is upstream's own
    /// `if (!value.trim() || pending || activeReply) return`.
    pub fn submit(&mut self, text: impl Into<String>) -> bool {
        let text = text.into();
        if text.trim().is_empty() || self.is_busy() {
            return false;
        }
        self.turns += 1;
        self.messages.push(ChatMessage::new(
            format!("user-{}", self.turns),
            ChatRole::User,
            text,
        ));
        self.input.clear();
        self.run = Some(ChatRun {
            elapsed: Duration::ZERO,
            reply_id: None,
        });
        true
    }

    /// Stop the reply where it is: whatever has arrived stays, the message stops
    /// being streamed, and the composer returns to its send state.
    ///
    /// Reports whether there was anything to stop.
    pub fn stop(&mut self) -> bool {
        if self.run.take().is_none() {
            return false;
        }
        for message in &mut self.messages {
            message.streaming = false;
        }
        true
    }

    /// Step the mock driver by `delta`, returning whether a reply is still in
    /// flight (and so whether the host owes another frame).
    pub fn advance(&mut self, delta: Duration) -> bool {
        let Some(mut run) = self.run.take() else {
            return false;
        };
        run.elapsed += delta;
        if run.elapsed < self.driver.first_token_delay {
            self.run = Some(run);
            return true;
        }

        let id = match &run.reply_id {
            Some(id) => id.clone(),
            None => {
                let id = format!("assistant-{}", self.turns);
                let mut message = ChatMessage::new(&id, ChatRole::Assistant, String::new());
                message.streaming = true;
                self.messages.push(message);
                run.reply_id = Some(id.clone());
                id
            }
        };

        let revealed = self.driver.revealed(run.elapsed);
        let total = self.driver.reply.chars().count();
        let content: String = self.driver.reply.chars().take(revealed).collect();
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            message.content = content;
            message.streaming = revealed < total;
        }
        if revealed >= total {
            return false;
        }
        self.run = Some(run);
        true
    }

    /// Append `token` to the reply in flight — the seam a real transport uses
    /// instead of [`advance`](Self::advance).
    ///
    /// Starts the reply message if this is its first token. A no-op when
    /// nothing is in flight.
    pub fn push_token(&mut self, token: &str) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let id = match &run.reply_id {
            Some(id) => id.clone(),
            None => {
                let id = format!("assistant-{}", self.turns);
                let mut message = ChatMessage::new(&id, ChatRole::Assistant, String::new());
                message.streaming = true;
                self.messages.push(message);
                run.reply_id = Some(id.clone());
                id
            }
        };
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            message.content.push_str(token);
        }
    }

    /// Close the reply in flight without truncating it — the transport's
    /// end-of-stream, as against [`stop`](Self::stop)'s user interruption.
    pub fn finish_reply(&mut self) {
        self.run = None;
        for message in &mut self.messages {
            message.streaming = false;
        }
    }
}

// ---- The conversation ------------------------------------------------------

/// A view-held, typed text callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held, typed no-argument callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State)>;

/// A host-supplied factory for the rows appended after the transcript.
///
/// A factory rather than a `Vec`: `AnyView` is not `Clone`, and the transcript
/// is re-assembled on both sides of every `rebuild` (the previous view's rows
/// and this one's), so the rows have to be *buildable twice*.
type ExtraRows<State> = Rc<dyn Fn() -> Vec<AnyView<State>>>;

/// The assembled conversation surface: a pinned transcript over a composer.
/// See the [module docs](self).
pub struct ChatConversationView<State: 'static> {
    messages: Vec<ChatMessage>,
    input: String,
    busy: bool,
    placeholder: Option<String>,
    user_variant: MessageBubbleVariant,
    assistant_variant: MessageBubbleVariant,
    user_avatar: String,
    assistant_avatar: String,
    extra_rows: Option<ExtraRows<State>>,
    on_input: Option<OnText<State>>,
    on_submit: Option<OnText<State>>,
    on_stop: Option<OnAction<State>>,
}

/// Build the conversation surface for `model`.
///
/// The view is a pure function of the model, so a host rebuilds it every frame
/// and the widgets below diff themselves.
pub fn chat_conversation<State: 'static>(model: &ChatModel) -> ChatConversationView<State> {
    ChatConversationView {
        messages: model.messages().to_vec(),
        input: model.input().to_owned(),
        busy: model.is_busy(),
        placeholder: None,
        user_variant: MessageBubbleVariant::Soft,
        assistant_variant: MessageBubbleVariant::Ghost,
        user_avatar: CHAT_APP_USER_AVATAR.to_owned(),
        assistant_avatar: CHAT_APP_ASSISTANT_AVATAR.to_owned(),
        extra_rows: None,
        on_input: None,
        on_submit: None,
        on_stop: None,
    }
}

impl<State: 'static> ChatConversationView<State> {
    /// Replace the composer's placeholder.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Paint user bubbles in `variant` — see the [module docs](self)'
    /// *Degradations* on why the default is not upstream's `solid`.
    pub fn user_variant(mut self, variant: MessageBubbleVariant) -> Self {
        self.user_variant = variant;
        self
    }

    /// Paint assistant bubbles in `variant` (default
    /// [`Ghost`](MessageBubbleVariant::Ghost), upstream's own surface-free
    /// assistant turn).
    pub fn assistant_variant(mut self, variant: MessageBubbleVariant) -> Self {
        self.assistant_variant = variant;
        self
    }

    /// Replace the two avatars' initials.
    pub fn avatars(mut self, user: impl Into<String>, assistant: impl Into<String>) -> Self {
        self.user_avatar = user.into();
        self.assistant_avatar = assistant.into();
        self
    }

    /// Append host-owned rows after the transcript — the tool cards, approval
    /// cards and generated media upstream's preview mounts inline.
    ///
    /// Takes a factory, not a list: `AnyView` is not `Clone`, and the
    /// transcript is re-assembled on both sides of every `rebuild`, so the rows
    /// have to be buildable twice.
    pub fn extra_rows<F: Fn() -> Vec<AnyView<State>> + 'static>(mut self, rows: F) -> Self {
        self.extra_rows = Some(Rc::new(rows));
        self
    }

    /// Observe composer edits (`ChatModel::set_input`).
    pub fn on_input<F: Fn(&mut State, String) + 'static>(mut self, callback: F) -> Self {
        self.on_input = Some(Rc::new(callback));
        self
    }

    /// Observe submissions (`ChatModel::submit`).
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, callback: F) -> Self {
        self.on_submit = Some(Rc::new(callback));
        self
    }

    /// Observe the stop button (`ChatModel::stop`).
    pub fn on_stop<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_stop = Some(Rc::new(callback));
        self
    }

    /// The transcript view: one row per message, plus whatever the host
    /// appended.
    fn transcript(&self) -> AnyView<State> {
        let mut rows: Vec<AnyView<State>> = Vec::with_capacity(self.messages.len());
        for turn in &self.messages {
            let (variant, align, avatar) = match turn.role {
                ChatRole::User => (
                    self.user_variant,
                    MessageBubbleAlign::End,
                    self.user_avatar.clone(),
                ),
                ChatRole::Assistant => (
                    self.assistant_variant,
                    MessageBubbleAlign::Start,
                    self.assistant_avatar.clone(),
                ),
            };
            let status = if turn.streaming {
                StreamingResponseStatus::Streaming
            } else {
                StreamingResponseStatus::Complete
            };
            let bubble =
                message_bubble::<State, _>(streaming_response(turn.content.clone()).status(status))
                    .variant(variant)
                    .align(align);
            rows.push(any(
                message::<State, _>(turn.role.from(), bubble).avatar(avatar)
            ));
        }
        if let Some(extra) = &self.extra_rows {
            rows.extend(extra());
        }
        any(message_scroller(rows))
    }

    /// The composer view.
    fn composer(&self) -> AnyView<State> {
        let on_input = self.on_input.clone();
        let mut composer = prompt_input::<State, _>(self.input.clone(), move |state, text| {
            if let Some(callback) = &on_input {
                callback(state, text);
            }
        })
        .loading(self.busy);
        if let Some(placeholder) = &self.placeholder {
            composer = composer.placeholder(placeholder.clone());
        }
        if let Some(on_submit) = &self.on_submit {
            let on_submit = on_submit.clone();
            composer = composer.on_submit(move |state, text| on_submit(state, text));
        }
        if let Some(on_stop) = &self.on_stop {
            let on_stop = on_stop.clone();
            composer = composer.on_stop(move |state| on_stop(state));
        }
        any(composer)
    }
}

/// The retained widget for a [`ChatConversationView`].
pub struct ChatConversationWidget {
    /// The transcript and the composer, in that order — one list so
    /// [`route_event`] owns the capture and focus fast paths across both
    /// rather than this widget re-deriving them per pod.
    panes: Vec<ChildPod>,
    /// The transcript's box, resolved by layout.
    transcript_box: Rect,
    /// The composer's box, resolved by layout.
    composer_box: Rect,
}

/// Recover a pod's concrete widget.
///
/// Two hops, not one: a pod built from an [`AnyView`] stores its element
/// **double-boxed** (`build_child` wraps the `Box<dyn Widget>` the erased view
/// produced in a second box), so the first downcast unwraps that box and only
/// then does the concrete type appear. A pod built from a typed view has no
/// inner box, which is why the unwrap falls through rather than failing.
fn pod_widget<T: Widget>(pod: &ChildPod) -> Option<&T> {
    let widget = pod.widget();
    let unboxed = match (widget as &dyn std::any::Any).downcast_ref::<Box<dyn Widget>>() {
        Some(boxed) => boxed.as_ref(),
        None => widget,
    };
    (unboxed as &dyn std::any::Any).downcast_ref::<T>()
}

/// [`ChatConversationWidget::panes`]' transcript slot.
const TRANSCRIPT: usize = 0;
/// [`ChatConversationWidget::panes`]' composer slot.
const COMPOSER: usize = 1;

/// [`ChatAppWidget::panes`]' sidebar slot.
const SIDEBAR: usize = 0;
/// [`ChatAppWidget::panes`]' conversation slot.
const CONVERSATION: usize = 1;

impl ChatConversationWidget {
    /// The transcript viewport's box, as of the last layout.
    pub fn transcript_box(&self) -> Rect {
        self.transcript_box
    }

    /// The composer's box, as of the last layout.
    pub fn composer_box(&self) -> Rect {
        self.composer_box
    }

    /// Whether the transcript is still pinned to the live edge — the
    /// scroller's own answer, read through its pod.
    pub fn is_pinned(&self) -> bool {
        self.scroller().is_some_and(|scroller| scroller.is_pinned())
    }

    /// The composer's trailing button, in this widget's own coordinates — what
    /// a host (or a test) presses to stop a stream.
    pub fn composer_button_box(&self) -> Option<Rect> {
        let composer: &crate::agents::prompt_input::PromptInputWidget =
            pod_widget(&self.panes[COMPOSER])?;
        Some(composer.button_box() + self.composer_box.origin().to_vec2())
    }

    /// The transcript's scroller, recovered from its pod.
    fn scroller(&self) -> Option<&crate::agents::message_scroller::MessageScrollerWidget> {
        pod_widget(&self.panes[TRANSCRIPT])
    }
}

impl<State: 'static> View<State> for ChatConversationView<State> {
    type Element = ChatConversationWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ChatConversationWidget {
        ChatConversationWidget {
            panes: vec![
                build_child(&self.transcript(), ctx),
                build_child(&self.composer(), ctx),
            ],
            transcript_box: Rect::ZERO,
            composer_box: Rect::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ChatConversationWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.transcript(),
            &self.transcript(),
            &mut element.panes[TRANSCRIPT],
            ctx,
        );
        flags |= rebuild_child(
            &prev.composer(),
            &self.composer(),
            &mut element.panes[COMPOSER],
            ctx,
        );
        flags
    }

    fn teardown(&self, element: &mut ChatConversationWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.transcript(), &mut element.panes[TRANSCRIPT], ctx);
        teardown_child(&self.composer(), &mut element.panes[COMPOSER], ctx);
    }
}

impl Widget for ChatConversationWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            0.0
        };
        let inner = (width - CHAT_APP_COMPOSER_PADDING * 2.0).max(0.0);

        // The composer takes what it needs from the bottom; the transcript gets
        // the rest, which is what keeps the live edge above the keyboard.
        let composer = self.panes[COMPOSER].layout_child(
            ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(inner, height)),
        );
        let composer_top = (height - composer.height - CHAT_APP_COMPOSER_PADDING).max(0.0);
        self.composer_box = Rect::from_origin_size(
            Point::new(CHAT_APP_COMPOSER_PADDING, composer_top),
            Size::new(inner, composer.height),
        );
        self.panes[COMPOSER].set_origin(self.composer_box.origin());

        let transcript_height = (composer_top - CHAT_APP_VIEWPORT_PADDING_Y).max(0.0);
        let transcript_width = (width - CHAT_APP_VIEWPORT_PADDING_X * 2.0).max(0.0);
        self.panes[TRANSCRIPT].layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(transcript_width, transcript_height)),
        );
        self.transcript_box = Rect::from_origin_size(
            Point::new(CHAT_APP_VIEWPORT_PADDING_X, CHAT_APP_VIEWPORT_PADDING_Y),
            Size::new(transcript_width, transcript_height),
        );
        self.panes[TRANSCRIPT].set_origin(self.transcript_box.origin());

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pane in &mut self.panes {
            pane.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.panes, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                for pane in &self.panes {
                    pane.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(panes);
}

// ---- The shell -------------------------------------------------------------

/// The two-pane agent workspace shell. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::agents::chat_app::{ChatModel, chat_app, chat_conversation};
///
/// let model = ChatModel::new();
/// let shell = chat_app::<(), _, _>(text("[sidebar]"), chat_conversation::<()>(&model));
/// ```
pub struct ChatAppView<State: 'static> {
    sidebar: AnyView<State>,
    conversation: AnyView<State>,
    open: Option<bool>,
    default_open: bool,
    sidebar_width: f64,
    collapse_below: f64,
}

/// Create the shell around `sidebar` and `conversation`, docked at
/// [`CHAT_APP_SIDEBAR_WIDTH`] and folding below
/// [`CHAT_APP_MIN_DOCKED_WIDTH`].
pub fn chat_app<State: 'static, S: View<State>, C: View<State>>(
    sidebar: S,
    conversation: C,
) -> ChatAppView<State> {
    ChatAppView {
        sidebar: any(sidebar),
        conversation: any(conversation),
        open: None,
        default_open: true,
        sidebar_width: CHAT_APP_SIDEBAR_WIDTH,
        collapse_below: CHAT_APP_MIN_DOCKED_WIDTH,
    }
}

impl<State: 'static> ChatAppView<State> {
    /// Take ownership of the sidebar's docked state. A controlled shell never
    /// folds itself — upstream skips `ShellFit` entirely when `open` is passed.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Whether an uncontrolled shell starts docked (`defaultOpen`).
    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Dock the sidebar at `width` logical px instead of
    /// [`CHAT_APP_SIDEBAR_WIDTH`].
    pub fn sidebar_width(mut self, width: f64) -> Self {
        self.sidebar_width = width.max(0.0);
        self
    }

    /// Fold the sidebar below `width` logical px instead of
    /// [`CHAT_APP_MIN_DOCKED_WIDTH`].
    pub fn collapse_below(mut self, width: f64) -> Self {
        self.collapse_below = width.max(0.0);
        self
    }
}

/// The retained widget for a [`ChatAppView`].
pub struct ChatAppWidget {
    /// The sidebar and conversation panes, in that order — one list for the
    /// same reason [`ChatConversationWidget::panes`] is one.
    panes: Vec<ChildPod>,
    /// The app-confirmed docked flag, when the shell is controlled.
    controlled: Option<bool>,
    /// The shell's own flag, when it is not.
    internal_open: bool,
    /// Whether the shell was narrower than its threshold at the last layout;
    /// `None` until the first measurement, which is what makes the *opening*
    /// direction ignore mount (upstream's `first && !narrow` guard).
    narrow: Option<bool>,
    sidebar_width: f64,
    collapse_below: f64,
    /// How docked the sidebar is, `0` folded .. `1` full width. Written in
    /// `paint`, read by `layout`.
    fraction: f64,
    morph: Lane,
    /// The sidebar pane's box, resolved by layout.
    sidebar_box: Rect,
    /// The conversation pane's box, resolved by layout.
    conversation_box: Rect,
}

impl ChatAppWidget {
    /// Whether the sidebar is docked right now.
    pub fn is_open(&self) -> bool {
        self.controlled.unwrap_or(self.internal_open)
    }

    /// The sidebar pane's current width — `0` folded, the docked width open,
    /// and everything between while the morph runs.
    pub fn sidebar_width(&self) -> f64 {
        self.sidebar_width * self.fraction
    }

    /// The sidebar pane's box, as of the last layout.
    pub fn sidebar_box(&self) -> Rect {
        self.sidebar_box
    }

    /// The conversation pane's box, as of the last layout.
    pub fn conversation_box(&self) -> Rect {
        self.conversation_box
    }

    /// Whether the shell measured itself as too narrow to carry both panes.
    pub fn is_narrow(&self) -> bool {
        self.narrow.unwrap_or(false)
    }

    /// Apply upstream's `ShellFit` rule for a measured `width`.
    ///
    /// Only *crossings* act, so a manual toggle at either size stays put; and
    /// mount is not a crossing in the opening direction, because a shell that
    /// merely has room says nothing about whether the host wanted the sidebar
    /// open. A controlled shell is left alone entirely.
    fn fit(&mut self, width: f64) {
        let narrow = width < self.collapse_below;
        if self.narrow == Some(narrow) {
            return;
        }
        let first = self.narrow.is_none();
        self.narrow = Some(narrow);
        if self.controlled.is_some() {
            return;
        }
        if first && !narrow {
            return;
        }
        self.internal_open = !narrow;
    }
}

impl<State: 'static> View<State> for ChatAppView<State> {
    type Element = ChatAppWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ChatAppWidget {
        let open = self.open.unwrap_or(self.default_open);
        ChatAppWidget {
            panes: vec![
                build_child(&self.sidebar, ctx),
                build_child(&self.conversation, ctx),
            ],
            controlled: self.open,
            internal_open: open,
            narrow: None,
            sidebar_width: self.sidebar_width,
            collapse_below: self.collapse_below,
            fraction: if open { 1.0 } else { 0.0 },
            morph: Lane::at_rest(
                Ramp::spring(SIDEBAR_MORPH_SPRING),
                if open { 1.0 } else { 0.0 },
            ),
            sidebar_box: Rect::ZERO,
            conversation_box: Rect::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ChatAppWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.sidebar,
            &self.sidebar,
            &mut element.panes[SIDEBAR],
            ctx,
        );
        flags |= rebuild_child(
            &prev.conversation,
            &self.conversation,
            &mut element.panes[CONVERSATION],
            ctx,
        );
        element.controlled = self.open;
        if element.sidebar_width != self.sidebar_width
            || element.collapse_below != self.collapse_below
        {
            element.sidebar_width = self.sidebar_width;
            element.collapse_below = self.collapse_below;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.open != self.open {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ChatAppWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.sidebar, &mut element.panes[SIDEBAR], ctx);
        teardown_child(&self.conversation, &mut element.panes[CONVERSATION], ctx);
    }
}

impl Widget for ChatAppWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            0.0
        };
        self.fit(width);

        let docked = (self.sidebar_width * self.fraction).min(width);
        self.sidebar_box = Rect::from_origin_size(Point::ORIGIN, Size::new(docked, height));
        self.conversation_box = Rect::from_origin_size(
            Point::new(docked, 0.0),
            Size::new((width - docked).max(0.0), height),
        );

        self.panes[SIDEBAR].layout_child(ctx, &BoxConstraints::tight(self.sidebar_box.size()));
        self.panes[SIDEBAR].set_origin(self.sidebar_box.origin());
        self.panes[CONVERSATION]
            .layout_child(ctx, &BoxConstraints::tight(self.conversation_box.size()));
        self.panes[CONVERSATION].set_origin(self.conversation_box.origin());

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, surface, border) = {
            let theme = Theme::from_paint_ctx(ctx);
            match theme {
                Some(theme) => (
                    theme.motion.reduce_motion,
                    theme.scheme().surface,
                    theme.scheme().outline_variant,
                ),
                None => (false, FALLBACK.background, FALLBACK.border),
            }
        };
        let origin = ctx.origin();
        let size = ctx.size();
        let radius = style::resolve_radius(CHAT_APP_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, surface);

        self.morph.retarget(if self.is_open() { 1.0 } else { 0.0 });
        if reduce_motion {
            self.morph.snap();
        }
        let travelling = self.morph.advance(ctx.frame_time());
        self.fraction = self.morph.value().clamp(0.0, 1.0);

        // `overflow-hidden`: a folding pane is clipped, not spilled.
        scene.push_clip_rounded(origin, size, radius);
        if self.sidebar_box.width() > 0.0 {
            self.panes[SIDEBAR].paint_child(ctx, scene);
        }
        self.panes[CONVERSATION].paint_child(ctx, scene);
        scene.pop_clip();
        crate::press::stroke_outline(scene, origin, size, radius, border);

        // The fold is a *width* animation, so it drives layout rather than a
        // bare repaint — the rule `animated_sidebar`'s own morph states.
        if travelling || self.fraction != self.morph.target() {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.panes, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                // A folded sidebar is not reachable by input, so it is omitted
                // rather than published with a zero-width box — the input-parity
                // carve-out `docs/CODE_STANDARDS.md` states for a container that
                // gates input.
                if self.sidebar_box.width() > 0.0 {
                    self.panes[SIDEBAR].semantics_child(ctx);
                }
                self.panes[CONVERSATION].semantics_child(ctx);
            },
        );
    }

    visit_children!(panes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Brush, Color, PointerButton, PointerEvent, PointerPhase};
    use frust::{FrameTime, text};
    use std::any::Any;

    /// The window every shell test lays itself into.
    const WINDOW: Size = Size::new(900.0, 600.0);
    /// The pane every conversation test lays itself into.
    const PANE: Size = Size::new(560.0, 480.0);

    /// The canned reply the driver streams in these tests.
    const REPLY: &str = "The validation gap is closed.";

    /// Records enough to prove something was painted.
    #[derive(Default)]
    struct Recorder {
        rounded: usize,
        rects: usize,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {
            self.rects += 1;
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {
            self.rounded += 1;
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// The app state the conversation reports into — a host's own shape.
    #[derive(Default)]
    struct App {
        model: ChatModel,
    }

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn model() -> ChatModel {
        ChatModel::new().with_driver(
            mock_chat_driver(REPLY)
                .chars_per_second(100.0)
                .first_token_delay(Duration::from_millis(400)),
        )
    }

    /// The conversation view a host builds every frame from its model.
    fn conversation(model: &ChatModel) -> ChatConversationView<App> {
        chat_conversation::<App>(model)
            .on_input(|app: &mut App, text| app.model.set_input(text))
            .on_submit(|app: &mut App, text| {
                app.model.submit(text);
            })
            .on_stop(|app: &mut App| {
                app.model.stop();
            })
    }

    fn build_conversation(view: &ChatConversationView<App>) -> ChatConversationWidget {
        let mut next_id = 0u64;
        View::<App>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn layout_conversation(widget: &mut ChatConversationWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::tight(PANE))
    }

    fn paint_conversation(widget: &mut ChatConversationWidget, ms: u64) -> Recorder {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, PANE, at(ms));
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        recorder
    }

    fn rebuild_conversation(
        widget: &mut ChatConversationWidget,
        from: &ChatConversationView<App>,
        to: &ChatConversationView<App>,
    ) {
        let mut next_id = 0u64;
        View::<App>::rebuild(to, from, widget, &mut BuildCtx::new(&mut next_id));
    }

    /// How many rows the transcript is showing.
    fn transcript_rows(widget: &ChatConversationWidget) -> usize {
        let scroller = widget.scroller().expect("the transcript is a scroller");
        let mut rows = 0;
        Widget::visit_children(scroller, &mut |_| rows += 1);
        rows
    }

    fn dispatch(
        widget: &mut ChatConversationWidget,
        phase: PointerPhase,
        position: Point,
        app: &mut App,
    ) {
        let mut ctx = EventCtx::new(app as &mut dyn Any, Point::ORIGIN, PANE);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position,
                button: PointerButton::Primary,
            }),
        );
    }

    fn build_shell(view: &ChatAppView<App>) -> ChatAppWidget {
        let mut next_id = 0u64;
        View::<App>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn layout_shell(widget: &mut ChatAppWidget, size: Size) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::tight(size))
    }

    fn paint_shell(widget: &mut ChatAppWidget, size: Size, ms: u64) -> bool {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, at(ms));
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        ctx.needs_layout()
    }

    // ---- The model ---------------------------------------------------------

    /// Upstream's own submit guard: no empty prompt, and no second turn while a
    /// reply is in flight.
    #[test]
    fn the_model_refuses_an_empty_prompt_and_a_second_turn_while_busy() {
        let mut chat = model();
        assert!(!chat.submit("   "), "whitespace is not a prompt");
        assert!(chat.messages().is_empty());
        assert!(!chat.is_busy());

        chat.set_input("audit the checkout flow");
        assert!(chat.submit("audit the checkout flow"));
        assert_eq!(chat.messages().len(), 1);
        assert_eq!(chat.messages()[0].role(), ChatRole::User);
        assert_eq!(chat.input(), "", "submitting clears the composer");
        assert!(chat.is_busy() && chat.is_pending());

        assert!(!chat.submit("and again"), "one reply at a time");
        assert_eq!(chat.messages().len(), 1);
    }

    /// The driver waits out its first-token delay, then reveals at its own rate
    /// and stops asking for frames when the reply is whole.
    #[test]
    fn the_mock_driver_waits_then_reveals_at_its_rate() {
        let mut chat = model();
        chat.submit("go");
        assert!(chat.advance(Duration::from_millis(200)));
        assert!(
            chat.streaming_message().is_none(),
            "no token has landed yet"
        );
        assert!(chat.is_pending());

        assert!(chat.advance(Duration::from_millis(300)));
        let live = chat.streaming_message().expect("the reply started");
        assert_eq!(live.role(), ChatRole::Assistant);
        assert!(live.is_streaming());
        let first = live.content().chars().count();
        assert!(
            first > 0 && first < REPLY.chars().count(),
            "partial: {first}"
        );

        assert!(chat.advance(Duration::from_millis(100)));
        let grown = chat.streaming_message().unwrap().content().chars().count();
        assert!(grown > first, "the reveal grows: {first} -> {grown}");

        assert!(!chat.advance(Duration::from_secs(5)), "the run is over");
        assert!(!chat.is_busy());
        assert!(chat.streaming_message().is_none());
        assert_eq!(chat.messages().last().unwrap().content(), REPLY);
        assert!(chat.submit("next"), "and the composer is free again");
    }

    /// Stopping keeps what arrived, clears the streaming flag, and frees the
    /// composer — upstream's `stop`.
    #[test]
    fn stopping_mid_stream_keeps_what_arrived() {
        let mut chat = model();
        chat.submit("go");
        chat.advance(Duration::from_millis(450));
        let caught = chat.streaming_message().unwrap().content().to_owned();
        assert!(!caught.is_empty() && caught != REPLY);

        assert!(chat.stop());
        assert!(!chat.is_busy());
        assert!(chat.streaming_message().is_none(), "nothing is streaming");
        assert_eq!(
            chat.messages().last().unwrap().content(),
            caught,
            "what arrived stays"
        );
        assert!(!chat.advance(Duration::from_secs(1)), "and nothing resumes");
        assert!(!chat.stop(), "a second stop has nothing to stop");
    }

    /// The transport seam: a host that owns a real stream pushes tokens and
    /// closes the reply itself, without the mock clock.
    #[test]
    fn push_token_and_finish_reply_drive_a_real_transport() {
        let mut chat = ChatModel::new();
        chat.push_token("ignored");
        assert!(
            chat.messages().is_empty(),
            "nothing in flight, nothing lands"
        );

        chat.submit("go");
        chat.push_token("Hello");
        chat.push_token(", world");
        let live = chat.streaming_message().expect("a reply is building");
        assert_eq!(live.content(), "Hello, world");

        chat.finish_reply();
        assert!(!chat.is_busy());
        assert!(chat.streaming_message().is_none());
        assert_eq!(chat.messages().last().unwrap().content(), "Hello, world");
    }

    /// A seeded transcript keeps its ids distinct from the ones a later turn
    /// mints.
    #[test]
    fn a_seeded_history_does_not_collide_with_later_turns() {
        let mut chat = ChatModel::new().with_history(vec![
            ChatMessage::new("history-1", ChatRole::User, "earlier"),
            ChatMessage::new("history-2", ChatRole::Assistant, "answered"),
        ]);
        chat.submit("now");
        let ids: Vec<&str> = chat.messages().iter().map(ChatMessage::id).collect();
        assert_eq!(ids, vec!["history-1", "history-2", "user-3"]);
    }

    // ---- The conversation, end to end --------------------------------------

    /// The whole surface, driven the way a host drives it: a prompt is sent, the
    /// reply streams into a row that grows, the transcript stays pinned to the
    /// live edge throughout, and a press on the composer's stop button — a real
    /// pointer press, routed through the composed widget tree — freezes it.
    #[test]
    fn a_prompt_streams_a_reply_stays_pinned_and_stops_mid_stream() {
        let mut app = App {
            model: model().with_history(vec![ChatMessage::new(
                "history-1",
                ChatRole::Assistant,
                "How can I help?",
            )]),
        };
        let mut view = conversation(&app.model);
        let mut widget = build_conversation(&view);
        layout_conversation(&mut widget);
        paint_conversation(&mut widget, 0);
        assert_eq!(transcript_rows(&widget), 1, "the seeded turn is showing");
        assert!(
            widget.is_pinned(),
            "a fresh transcript sits at the live edge"
        );

        // 1. The user sends. A host's own callback path: the model takes it.
        assert!(app.model.submit("audit the checkout flow"));
        let next = conversation(&app.model);
        rebuild_conversation(&mut widget, &view, &next);
        view = next;
        layout_conversation(&mut widget);
        paint_conversation(&mut widget, 16);
        assert_eq!(transcript_rows(&widget), 2, "the user's turn is on screen");
        assert!(app.model.is_pending(), "and a reply is owed");

        // 2. The reply starts streaming and the row grows frame by frame.
        let mut lengths = Vec::new();
        for frame in 1..=6u64 {
            app.model.advance(Duration::from_millis(100));
            let next = conversation(&app.model);
            rebuild_conversation(&mut widget, &view, &next);
            view = next;
            layout_conversation(&mut widget);
            paint_conversation(&mut widget, 16 + frame * 100);
            if let Some(live) = app.model.streaming_message() {
                lengths.push(live.content().chars().count());
            }
        }
        assert_eq!(transcript_rows(&widget), 3, "the reply has its own row");
        assert!(lengths.len() >= 3, "the reply streamed: {lengths:?}");
        assert!(
            lengths.windows(2).all(|pair| pair[1] >= pair[0]),
            "the reveal only grows: {lengths:?}"
        );
        assert!(
            *lengths.last().unwrap() < REPLY.chars().count(),
            "and is still mid-stream"
        );
        assert!(widget.is_pinned(), "the transcript followed the live edge");

        // 3. The user presses stop — through the composed prompt input, at the
        //    button box the composer actually laid out.
        let button = widget
            .composer_button_box()
            .expect("the composer publishes its button");
        assert!(
            widget.composer_box().contains(button.center()),
            "the button sits inside the composer: {button:?}"
        );
        let caught = app
            .model
            .streaming_message()
            .map(|m| m.content().to_owned())
            .expect("a reply was in flight");
        dispatch(&mut widget, PointerPhase::Down, button.center(), &mut app);
        dispatch(&mut widget, PointerPhase::Up, button.center(), &mut app);

        assert!(!app.model.is_busy(), "the stop reached the model");
        assert!(app.model.streaming_message().is_none());
        assert_eq!(
            app.model.messages().last().unwrap().content(),
            caught,
            "the reply froze where it was"
        );

        // 4. The surface settles: the frozen row is still there and still paints.
        let next = conversation(&app.model);
        rebuild_conversation(&mut widget, &view, &next);
        layout_conversation(&mut widget);
        let painted = paint_conversation(&mut widget, 2_000);
        assert_eq!(transcript_rows(&widget), 3);
        assert!(!painted.inks.is_empty(), "the transcript still paints text");
    }

    /// Typing reaches the model through the composer's own change callback.
    #[test]
    fn typing_in_the_composer_reaches_the_model() {
        let mut app = App { model: model() };
        // The composer is controlled: the host's callback is the only writer.
        let view = conversation(&app.model);
        let mut widget = build_conversation(&view);
        layout_conversation(&mut widget);

        app.model.set_input("hello");
        let next = conversation(&app.model);
        rebuild_conversation(&mut widget, &view, &next);
        layout_conversation(&mut widget);
        assert_eq!(app.model.input(), "hello");
        assert!(app.model.submit(app.model.input().to_owned()));
        assert_eq!(app.model.messages()[0].content(), "hello");
    }

    /// The conversation lays its composer along the bottom and gives the
    /// transcript everything above it.
    #[test]
    fn the_composer_takes_the_bottom_and_the_transcript_takes_the_rest() {
        let app = App { model: model() };
        let mut widget = build_conversation(&conversation(&app.model));
        let size = layout_conversation(&mut widget);
        assert_eq!(size, PANE);
        assert!(widget.composer_box().height() > 0.0);
        assert!(
            widget.composer_box().y0 > widget.transcript_box().y1 - 1.0,
            "the composer sits below the transcript"
        );
        assert!(
            widget.composer_box().y1 <= PANE.height,
            "and inside the pane"
        );
        // Both panes are published to an inspector.
        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 2);
    }

    // ---- The shell ---------------------------------------------------------

    /// Upstream's `ShellFit`: a shell that starts narrow folds immediately, a
    /// shell that starts wide is left alone, and only *crossings* act.
    #[test]
    fn the_shell_folds_below_its_threshold_and_docks_above_it() {
        let app = App { model: model() };
        let view = chat_app::<App, _, _>(text("[sidebar]"), conversation(&app.model));

        // Wide at mount: the fit says nothing, so `defaultOpen` stands.
        let mut wide = build_shell(&view);
        layout_shell(&mut wide, WINDOW);
        assert!(wide.is_open(), "a wide mount keeps its default");
        assert!(!wide.is_narrow());
        assert_eq!(wide.sidebar_box().width(), CHAT_APP_SIDEBAR_WIDTH);
        assert_eq!(
            wide.conversation_box().width(),
            WINDOW.width - CHAT_APP_SIDEBAR_WIDTH
        );

        // Narrowing across the threshold folds it.
        let narrow = Size::new(CHAT_APP_MIN_DOCKED_WIDTH - 40.0, WINDOW.height);
        layout_shell(&mut wide, narrow);
        assert!(wide.is_narrow());
        assert!(!wide.is_open(), "too narrow for two panes");
        paint_shell(&mut wide, narrow, 0);
        paint_shell(&mut wide, narrow, 5_000);
        layout_shell(&mut wide, narrow);
        assert_eq!(wide.sidebar_box().width(), 0.0, "the pane folded away");
        assert_eq!(wide.conversation_box().width(), narrow.width);

        // Widening back across it docks the sidebar again.
        layout_shell(&mut wide, WINDOW);
        assert!(wide.is_open(), "room for both again");

        // Narrow at mount *is* the fold condition, so it applies immediately.
        let mut small = build_shell(&view);
        layout_shell(&mut small, narrow);
        assert!(!small.is_open(), "a narrow mount folds at once");
    }

    /// A controlled shell never folds itself — upstream skips `ShellFit`
    /// entirely when `open` is passed.
    #[test]
    fn a_controlled_shell_never_folds_itself() {
        let app = App { model: model() };
        let view = chat_app::<App, _, _>(text("[sidebar]"), conversation(&app.model)).open(true);
        let mut widget = build_shell(&view);
        let narrow = Size::new(CHAT_APP_MIN_DOCKED_WIDTH - 100.0, WINDOW.height);
        layout_shell(&mut widget, narrow);
        assert!(widget.is_narrow(), "it still measures itself");
        assert!(widget.is_open(), "but the owner decides");
        assert_eq!(widget.sidebar_box().width(), CHAT_APP_SIDEBAR_WIDTH);
    }

    /// The fold is a width morph, so it drives **layout** while it runs and
    /// lands exactly on zero.
    #[test]
    fn the_fold_is_a_width_morph_that_drives_layout() {
        let app = App { model: model() };
        let open = chat_app::<App, _, _>(text("[sidebar]"), conversation(&app.model)).open(true);
        let shut = chat_app::<App, _, _>(text("[sidebar]"), conversation(&app.model)).open(false);
        let mut widget = build_shell(&open);
        layout_shell(&mut widget, WINDOW);
        paint_shell(&mut widget, WINDOW, 0);
        assert_eq!(widget.sidebar_width(), CHAT_APP_SIDEBAR_WIDTH);

        let mut next_id = 0u64;
        View::<App>::rebuild(&shut, &open, &mut widget, &mut BuildCtx::new(&mut next_id));
        let needs_layout = paint_shell(&mut widget, WINDOW, 10);
        assert!(needs_layout, "a width morph must drive layout");
        paint_shell(&mut widget, WINDOW, 60);
        let mid = widget.sidebar_width();
        assert!(
            mid > 0.0 && mid < CHAT_APP_SIDEBAR_WIDTH,
            "mid-morph: {mid}"
        );

        paint_shell(&mut widget, WINDOW, 5_000);
        assert_eq!(widget.sidebar_width(), 0.0, "and lands folded");
        layout_shell(&mut widget, WINDOW);
        assert_eq!(widget.conversation_box().width(), WINDOW.width);
    }

    /// The shell paints both panes and publishes both to an inspector.
    #[test]
    fn the_shell_publishes_and_paints_both_panes() {
        let app = App { model: model() };
        let view = chat_app::<App, _, _>(text("[sidebar]"), conversation(&app.model));
        let mut widget = build_shell(&view);
        layout_shell(&mut widget, WINDOW);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, at(0));
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        assert!(recorder.rounded > 0, "the shell paints its own surface");

        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 2);
    }

    /// The two roles map onto the transcript's own directions.
    #[test]
    fn the_roles_map_onto_the_transcripts_directions() {
        assert_eq!(ChatRole::User.from(), MessageFrom::User);
        assert_eq!(ChatRole::Assistant.from(), MessageFrom::Assistant);
        assert_eq!(mock_chat_driver("hi").reply(), "hi");
        assert_eq!(
            MockChatDriver::default().reply(),
            "Here is the patch: the validation gap is closed and the suite is green."
        );
    }
}
