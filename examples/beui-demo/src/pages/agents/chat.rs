//! Agents · Chat — the section's flagship: `chat_app`'s two-pane workspace with
//! `ai_sidebar` in the rail and the catalog's assembled conversation in the
//! inset, driven end to end by `chat_app`'s own `MockChatDriver`.
//!
//! Sending, streaming and stopping are real: the composer submits into a
//! [`ChatModel`], the driver reveals its canned reply at a fixed rate, and the
//! stop button truncates the reply where it stands. `todo_list`,
//! `agent_activity` and `image_generation` ride along as host-owned rows, which
//! is the seam the port offers for upstream's inline cards.
//!
//! See [`crate::pages::agents::primitives`] for the component boundary and the
//! frame pump that steps [`ChatModel::advance`] — beUI's model is stepped "from
//! the host's own frame loop", and a frust app has no ambient tick.

use std::time::Duration;

use frust::{
    AnyView, Color, Column, Component, CrossAxisAlignment, Row, SizedBox, View, any, colored_box,
    component, text,
};
use frust_beui::agents::agent_activity::{
    AgentActivityStatus, AgentStepStatus, activity_search, activity_step, activity_text,
    activity_tool, activity_trace, agent_activity,
};
use frust_beui::agents::ai_sidebar::{
    AiSidebarKind, AiSidebarMove, AiSidebarResource, ai_sidebar, ai_sidebar_move,
    ai_sidebar_resource,
};
use frust_beui::agents::chat_app::{
    ChatMessage, ChatModel, ChatRole, chat_app, chat_conversation, mock_chat_driver,
};
use frust_beui::agents::image_generation::{
    ImageGenerationSize, ImageGenerationStatus, image_generation,
};
use frust_beui::agents::message::{MessageFrom, message};
use frust_beui::agents::todo_list::{TodoListStatus, todo_item, todo_list};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};

use super::primitives::{BLOCK_GAP, FrameClock, heading};
use crate::AppState;
use crate::nav::caption;

// ---- Sample data -----------------------------------------------------------

/// How tall the workspace is given. The shell fills whatever box it is handed
/// and the gallery's page slot is a scroll view (unbounded height), so the page
/// pins one here — the same reason the primitives page's scroller box exists.
const WORKSPACE_HEIGHT: f64 = 620.0;

/// The reply the mock driver streams — upstream's own `chat-app-usage.tsx`
/// answer.
const REPLY: &str = "I\u{2019}ll keep the patch focused, preserve the current \
    checkout layout, and run the same validation path before preparing the \
    release.";

/// The driver's reveal rate, in characters per second — upstream's preview
/// value, and it must stay above zero or the reply never finishes arriving.
const REPLY_RATE: f64 = 92.0;

/// The image card's own beats, in ms since a regenerate press.
const MEDIA_GENERATING_MS: u64 = 420;
/// When the card starts refining.
const MEDIA_REFINING_MS: u64 = 1500;
/// When it settles.
const MEDIA_COMPLETE_MS: u64 = 2400;

/// The workspace's resource tree — upstream's `chat-app-usage.tsx` resources.
fn resources() -> Vec<AiSidebarResource> {
    vec![
        ai_sidebar_resource("release", "Release workspace", AiSidebarKind::Project).children(vec![
            ai_sidebar_resource("checkout", "Checkout audit", AiSidebarKind::File),
            ai_sidebar_resource("release-notes", "Release notes", AiSidebarKind::File),
            ai_sidebar_resource("references", "Research sources", AiSidebarKind::Bookmark),
        ]),
        ai_sidebar_resource("design", "Design system", AiSidebarKind::Folder).children(vec![
            ai_sidebar_resource("tokens", "Motion tokens", AiSidebarKind::File),
            ai_sidebar_resource("components", "Component inventory", AiSidebarKind::File),
        ]),
        ai_sidebar_resource("archive", "Archived runs", AiSidebarKind::Folder),
    ]
}

/// The transcript the workspace opens on — upstream's first turn, plus the
/// answer it was given.
fn history() -> Vec<ChatMessage> {
    vec![
        ChatMessage::new(
            "user-seed",
            ChatRole::User,
            "Audit the checkout flow, fix the validation gap, and prepare a \
             release-ready patch.",
        ),
        ChatMessage::new(
            "assistant-seed",
            ChatRole::Assistant,
            "Done. Validation now runs before submission, the failure output \
             stays inside the current flow, and the focused checks pass without \
             changing the layout.",
        ),
    ]
}

// ---- Page state ------------------------------------------------------------

/// Everything this page retains — see [`crate::pages::agents::primitives`] on
/// why it lives behind a component boundary.
pub struct State {
    clock: FrameClock,
    /// The conversation itself: turns, composer text, and the reply in flight.
    model: ChatModel,
    /// The rail's resource tree, as the four move commands have rearranged it.
    resources: Vec<AiSidebarResource>,
    /// Which resource the rail shows as active.
    active_resource: String,
    /// The plan's disclosure.
    plan_open: bool,
    /// The activity log's disclosure.
    activity_open: bool,
    /// How long the agent has spent streaming, in seconds — the log's duration.
    worked: f64,
    /// Where the generated-image card is.
    media: ImageGenerationStatus,
    /// How long its run has been going.
    media_elapsed: Duration,
    /// Whether that run is in flight.
    media_running: bool,
    /// The last move command the rail reported, and whether it was legal.
    moved: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            clock: FrameClock::default(),
            model: ChatModel::new()
                .with_driver(mock_chat_driver(REPLY).chars_per_second(REPLY_RATE))
                .with_history(history()),
            resources: resources(),
            active_resource: "checkout".to_string(),
            plan_open: true,
            activity_open: true,
            worked: 6.0,
            media: ImageGenerationStatus::Complete,
            media_elapsed: Duration::ZERO,
            media_running: false,
            moved: None,
        }
    }
}

impl State {
    /// Step the conversation and the image card by one frame's worth of shell
    /// time.
    fn advance(&mut self, delta: Duration) {
        if self.model.is_busy() {
            self.worked += delta.as_secs_f64();
        }
        self.model.advance(delta);

        if self.media_running {
            self.media_elapsed += delta;
            let ms = self.media_elapsed.as_millis() as u64;
            self.media = if ms < MEDIA_GENERATING_MS {
                ImageGenerationStatus::Queued
            } else if ms < MEDIA_REFINING_MS {
                ImageGenerationStatus::Generating
            } else if ms < MEDIA_COMPLETE_MS {
                ImageGenerationStatus::Refining
            } else {
                self.media_running = false;
                ImageGenerationStatus::Complete
            };
        }
    }

    /// Whether anything on the page still owes a frame.
    fn busy(&self) -> bool {
        self.model.is_busy() || self.media_running
    }

    /// Start the image card's run over.
    fn regenerate(&mut self) {
        self.media = ImageGenerationStatus::Queued;
        self.media_elapsed = Duration::ZERO;
        self.media_running = true;
    }
}

// ---- The two panes ---------------------------------------------------------

/// The shell's rail: the workspace's resource tree.
fn rail(state: &State) -> AnyView<State> {
    any(Column(vec![
        any(text("Agent workspace").size(13.0)),
        any(SizedBox(None, Some(4.0))),
        any(caption("Alt+Shift+Arrow moves a resource.")),
        any(SizedBox(None, Some(12.0))),
        any(ai_sidebar::<State>(state.resources.clone())
            .active_id(state.active_resource.clone())
            .default_expanded(vec!["release".to_string(), "design".to_string()])
            .label("Agent workspace")
            .on_activate(|s: &mut State, id: String| s.active_resource = id)
            .on_move(|s: &mut State, requested: AiSidebarMove| {
                let describe = format!(
                    "{} \u{2192} {:?} {}",
                    requested.item_id,
                    requested.position,
                    requested.target_id.clone().unwrap_or_default()
                );
                match ai_sidebar_move(&s.resources, &requested) {
                    Some(next) => {
                        s.resources = next;
                        s.moved = Some(format!("Moved: {describe}"));
                    }
                    None => s.moved = Some(format!("Refused: {describe}")),
                }
            })),
    ]))
}

/// The host-owned rows the conversation appends after its transcript: the
/// activity log, the plan, and the generated media.
fn workspace_rows(
    busy: bool,
    worked: f64,
    plan_open: bool,
    activity_open: bool,
    media: ImageGenerationStatus,
    turns: usize,
) -> Vec<AnyView<State>> {
    let plan = vec![
        todo_item("inspect", "Inspect the checkout flow").status(TodoListStatus::Completed),
        todo_item("patch", "Prepare the validation patch").status(TodoListStatus::Completed),
        todo_item("reply", "Answer the follow-up")
            .status(if busy {
                TodoListStatus::InProgress
            } else if turns > 2 {
                TodoListStatus::Completed
            } else {
                TodoListStatus::Pending
            })
            .detail("Streams through the mock driver"),
        todo_item("review", "Collect release approval").status(TodoListStatus::Pending),
    ];

    let activity = vec![
        activity_text(
            "reason",
            "Tracing the checkout submission path and the validation boundary.",
        ),
        activity_tool("read", "read", "checkout/submit.ts"),
        activity_search("search", "order validation failures").meta("3 results"),
        activity_step("patch", "Apply the validation patch").status(if busy {
            AgentStepStatus::Active
        } else {
            AgentStepStatus::Complete
        }),
        activity_trace("trace", "Re-ran the focused checks", false),
    ];

    vec![
        any(message::<State, _>(
            MessageFrom::Assistant,
            Column(vec![
                any(agent_activity::<State>(activity)
                    .status(if busy {
                        AgentActivityStatus::Working
                    } else {
                        AgentActivityStatus::Complete
                    })
                    .duration(worked)
                    .open(activity_open)
                    .collapse_on_complete(false)
                    .max_height(200.0)
                    .on_open_change(|s: &mut State, open| s.activity_open = open)),
                any(SizedBox(None, Some(12.0))),
                any(todo_list::<State>(plan)
                    .title("Release plan")
                    .open(plan_open)
                    .collapse_on_complete(false)
                    .max_height(200.0)
                    .on_open_change(|s: &mut State, open| s.plan_open = open)),
            ]),
        )
        .avatar_placeholder(true)),
        any(message::<State, _>(
            MessageFrom::Assistant,
            Column(vec![
                any(image_generation::<State>()
                    .media(
                        colored_box::<State>()
                            .fill(Color::from_rgb8(0x22, 0xc5, 0x5e))
                            .radius(20.0),
                    )
                    .status(media)
                    .prompt("a clear checkout confirmation screen")
                    .resolution("1280 \u{d7} 840")
                    .size(ImageGenerationSize::Compact)
                    .interactive(true)
                    .on_retry(|s: &mut State| s.regenerate())),
                any(SizedBox(None, Some(8.0))),
                any(button("Regenerate", |s: &mut State| s.regenerate())
                    .tone(ButtonTone::Ghost)
                    .size(ButtonSize::Sm)),
            ]),
        )
        .avatar_placeholder(true)),
    ]
}

/// The shell's inset: the assembled conversation, wired to the model.
fn conversation(state: &State) -> AnyView<State> {
    let busy = state.model.is_busy();
    let worked = state.worked;
    let plan_open = state.plan_open;
    let activity_open = state.activity_open;
    let media = state.media;
    let turns = state.model.messages().len();

    any(chat_conversation::<State>(&state.model)
        .placeholder("Ask the agent to continue\u{2026}")
        .avatars("You", "AI")
        .extra_rows(move || workspace_rows(busy, worked, plan_open, activity_open, media, turns))
        .on_input(|s: &mut State, value: String| s.model.set_input(value))
        .on_submit(|s: &mut State, value: String| {
            s.model.submit(value);
        })
        .on_stop(|s: &mut State| {
            s.model.stop();
        }))
}

// ---- The page --------------------------------------------------------------

/// The page's component: owns [`State`], steps the model, and mounts the shell.
struct Chat;

impl Component for Chat {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        let delta = state.clock.tick();
        state.advance(delta);

        let status = if state.model.is_pending() {
            "waiting for the first token".to_string()
        } else if state.model.is_busy() {
            "streaming the reply \u{2014} press stop to truncate it".to_string()
        } else {
            format!("idle \u{2014} {} turns", state.model.messages().len())
        };
        let moved = state
            .moved
            .clone()
            .unwrap_or_else(|| "No move requested yet.".to_string());

        any(Column(vec![
            any(heading("Agents \u{b7} Chat")),
            any(SizedBox(None, Some(8.0))),
            any(caption(
                "chat_app is the shell; the conversation inside it is the \
                 catalog's own assembled surface over a ChatModel the page \
                 owns. A user bubble is Soft rather than upstream's inverted \
                 solid \u{2014} the bubble's content resolves its own ink from the \
                 theme and there is no inverted-ink seam to hand it. The rail \
                 folds itself away below 600px and reports nothing when it does, \
                 since the fold is decided in layout, which carries no event \
                 context.",
            )),
            any(SizedBox(None, Some(10.0))),
            any(caption(
                "The nine cards upstream's preview mounts inline are host rows \
                 here, appended after the transcript rather than interleaved \
                 with it \u{2014} extra_rows is the seam the port offers, and it \
                 takes a factory rather than a list because the transcript is \
                 re-assembled on both sides of every rebuild.",
            )),
            any(SizedBox(None, Some(BLOCK_GAP))),
            any(SizedBox(None, Some(WORKSPACE_HEIGHT))
                .child(chat_app::<State, _, _>(rail(state), conversation(state)))),
            // The pump sits beside the workspace, not at the foot of the page:
            // see `FrameClock::pump` on why paint-time culling makes placement
            // load-bearing.
            any(state.clock.pump(state.busy())),
            any(SizedBox(None, Some(12.0))),
            any(Row(vec![
                any(caption(format!(
                    "Driver: {status}. Active resource: {}.",
                    state.active_resource
                ))),
                any(SizedBox(Some(12.0), None)),
                any(button("Reset the conversation", |s: &mut State| {
                    s.model = ChatModel::new()
                        .with_driver(mock_chat_driver(REPLY).chars_per_second(REPLY_RATE))
                        .with_history(history());
                    s.worked = 6.0;
                })
                .tone(ButtonTone::Ghost)
                .size(ButtonSize::Sm)),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
            any(SizedBox(None, Some(8.0))),
            any(caption(moved)),
        ]))
    }
}

/// The Agents · Chat page.
pub fn page() -> AnyView<AppState> {
    any(component(Chat))
}
