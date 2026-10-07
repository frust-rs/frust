//! Agents · Primitives — the six parts a chat transcript is assembled from:
//! `message_bubble`, `message`, `message_scroller`, `prompt_input`,
//! `streaming_response` and `loading_states`.
//!
//! Every interaction here is real and runs against this page's own retained
//! state: the scroller's transcript grows from a button, the composer is
//! controlled, and the streaming replay re-reveals a canned answer a character
//! at a time.
//!
//! # Why the page hosts a component
//!
//! The gallery's page slot hands each Agents page a bare `page()` with no state
//! (see [`crate::pages`]) and `AppState` carries no field for it, so the page
//! owns its state behind a [`frust::component`] boundary — the same seam
//! `examples/shadcn-demo`'s chat page uses for its state-independent pills, one
//! level up. One consequence worth knowing: navigating away tears the component
//! down, so the transcript and the composer start fresh on the way back.
//!
//! # The section's shared clock
//!
//! [`FrameClock`] and [`FrameClock::pump`] live here and are used by all three
//! Agents pages. beUI's agent parts are *controlled* end to end — a streaming
//! response grows because its caller appends to it, and
//! [`ChatModel::advance`](frust_beui::agents::chat_app::ChatModel::advance) is
//! stepped from "the host's own frame loop" — but a frust app has no ambient
//! tick: the shell's clock reaches a widget during `paint`
//! (`PaintCtx::frame_time`) and another frame only happens because some widget
//! asked for one. The pump is the smallest widget that closes that loop — it
//! paints nothing, republishes the shell's frame time into a shared cell, and
//! asks for the next frame while it is active — and the page reads the cell in
//! `build`, which the desktop shell re-runs every frame.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::{
    AnyView, Column, Component, CrossAxisAlignment, FrameTime, SizedBox, any, column, component,
    row, text,
};
use frust_beui::agents::loading_states::{LoadingStatesVariant, loading_states};
use frust_beui::agents::message::{MessageFrom, message, message_marker};
use frust_beui::agents::message_bubble::{
    MessageBubbleAlign, MessageBubbleSpacing, MessageBubbleVariant, message_bubble,
    message_bubble_group,
};
use frust_beui::agents::message_scroller::message_scroller;
use frust_beui::agents::prompt_input::{PromptInputSend, prompt_input};
use frust_beui::agents::streaming_response::{StreamingResponseStatus, streaming_response};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};

use crate::AppState;
use crate::nav::caption;

// ---- The section's shared frame pump ---------------------------------------

/// The longest step one [`FrameClock::tick`] reports, so a demo that has been
/// idle (or backgrounded) for a minute resumes at a sane rate instead of
/// jumping its whole run in a single frame.
const MAX_STEP: Duration = Duration::from_millis(50);

/// The shell's frame clock, shared between a [pump](FrameClock::pump) mounted
/// in the view tree and the page state that reads it. See the [module
/// docs](self).
#[derive(Clone, Default)]
pub(crate) struct FrameClock {
    /// Written by the pump's `paint`, read by the page's `build`.
    published: Rc<Cell<FrameTime>>,
    /// The value the last [`tick`](Self::tick) consumed.
    consumed: Option<FrameTime>,
}

impl FrameClock {
    /// How much shell time has passed since the previous tick, clamped to
    /// [`MAX_STEP`]. Reports `Duration::ZERO` on the first tick and on any
    /// build where the pump published nothing new.
    pub(crate) fn tick(&mut self) -> Duration {
        let now = self.published.get();
        let delta = match self.consumed {
            Some(previous) => now.saturating_sub(previous),
            None => Duration::ZERO,
        };
        self.consumed = Some(now);
        delta.min(MAX_STEP)
    }

    /// A zero-sized view that keeps frames coming while `active`.
    ///
    /// **Mount it beside the demo it drives, not once at the foot of the
    /// page.** A `Flex` inside a scroll view culls a child whose box lies more
    /// than one viewport past the fold from *paint*, and paint is the only pass
    /// that can ask for another frame — a pump parked at the bottom of a long
    /// gallery page would therefore go silent exactly while the reader is
    /// looking at the demo it is meant to be stepping. Several pumps may share
    /// one clock: they publish the same frame time, so whichever is on screen
    /// keeps the page running.
    pub(crate) fn pump(&self, active: bool) -> FramePumpView {
        FramePumpView {
            published: Rc::clone(&self.published),
            active,
        }
    }
}

/// The pump's view half — see [`FrameClock::pump`].
pub(crate) struct FramePumpView {
    published: Rc<Cell<FrameTime>>,
    active: bool,
}

/// The pump's retained widget: zero-sized, paints nothing.
pub(crate) struct FramePumpWidget {
    published: Rc<Cell<FrameTime>>,
    active: bool,
}

impl<State: 'static> View<State> for FramePumpView {
    type Element = FramePumpWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FramePumpWidget {
        FramePumpWidget {
            published: Rc::clone(&self.published),
            active: self.active,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FramePumpWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.published = Rc::clone(&self.published);
        if prev.active == self.active {
            return ChangeFlags::NONE;
        }
        element.active = self.active;
        // A pump that has just been switched on needs one paint to ask the
        // shell for the frame after it; without the flag the run would wait
        // for whatever repaint happened to come next.
        ChangeFlags::PAINT
    }
}

impl Widget for FramePumpWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if !self.active {
            return;
        }
        self.published.set(ctx.frame_time());
        ctx.request_frame();
    }
}

// ---- Shared page furniture -------------------------------------------------

/// A gallery page's title. `crate::nav::heading` is bound to [`AppState`] and
/// these pages build against their own component state, so the section carries
/// its own copy of the one line it is.
pub(crate) fn heading(title: &str) -> frust::TextView {
    text(title.to_string()).size(24.0)
}

/// A block heading inside a page.
pub(crate) fn sub_heading(title: &str) -> frust::TextView {
    text(title.to_string()).size(16.0)
}

/// The vertical rhythm every Agents page stacks its blocks on.
pub(crate) const BLOCK_GAP: f64 = 28.0;

// ---- Sample data -----------------------------------------------------------

/// One seeded transcript turn.
struct Turn {
    from: MessageFrom,
    body: &'static str,
}

/// The scroller's opening transcript — upstream's own
/// `message-scroller.preview.tsx` conversation, verbatim, and long enough that
/// the viewport starts scrollable so "pinned to the live edge" means something
/// on the first frame.
const SEED: [Turn; 10] = [
    Turn {
        from: MessageFrom::User,
        body: "What should the first release include?",
    },
    Turn {
        from: MessageFrom::Assistant,
        body: "Start with the smallest workflow that still feels complete.",
    },
    Turn {
        from: MessageFrom::User,
        body: "Include streaming and recovery states too.",
    },
    Turn {
        from: MessageFrom::Assistant,
        body: "Yes. Those states make the first version feel dependable.",
    },
    Turn {
        from: MessageFrom::User,
        body: "How should we present tool results?",
    },
    Turn {
        from: MessageFrom::Assistant,
        body: "Keep results close to the action that produced them.",
    },
    Turn {
        from: MessageFrom::User,
        body: "What about actions that need confirmation?",
    },
    Turn {
        from: MessageFrom::Assistant,
        body: "Pause the run, explain the impact, and ask before continuing.",
    },
    Turn {
        from: MessageFrom::User,
        body: "Can the transcript stay easy to navigate?",
    },
    Turn {
        from: MessageFrom::Assistant,
        body: "Use the rail to jump between turns without losing your place.",
    },
];

/// The replies "Simulate incoming" delivers, in order — a press is an arrival,
/// which is all that is needed to exercise the pin contract.
const ARRIVALS: [&str; 6] = [
    "One more thing \u{2014}",
    "the viewport unpins the moment you scroll away from the end.",
    "Growth then preserves your place instead of chasing the tail.",
    "The jump button only appears while you are unpinned.",
    "Pressing it glides back to the live edge and re-pins.",
    "That is the whole contract.",
];

/// The answer the streaming replay reveals.
const REPLY: &str = "A streaming response is a surface, not a timer: the caller \
    appends to it and the widget fades in only the glyphs past the settled \
    prefix, so a second append never re-animates what is already on screen.";

/// How fast the replay reveals [`REPLY`], in characters per second — upstream's
/// own streaming-response preview rate.
const REPLY_RATE: f64 = 110.0;

/// The elapsed value a settled response sits at: past the end of [`REPLY`] at
/// [`REPLY_RATE`], so the block reads as a finished answer until Replay resets
/// it to zero. Stopping mid-run leaves it wherever it got to, which is what
/// stopping a stream means.
const REPLY_SETTLED: Duration = Duration::from_secs(60);

/// How long the composer's mock send stays in flight — upstream's own
/// `prompt-input.preview.tsx` delay.
const SEND_MS: u64 = 900;

// ---- Page state ------------------------------------------------------------

/// Everything this page retains. Mounted behind [`frust::component`], so it
/// lives exactly as long as the page is on screen.
pub struct State {
    clock: FrameClock,
    /// The scroller's transcript, oldest first: who sent each turn, and what
    /// they said.
    transcript: Vec<(MessageFrom, String)>,
    /// How many canned arrivals have been delivered.
    arrivals: usize,
    /// The scroller's own pin state, as it last reported it.
    pinned: bool,
    /// The composer's text.
    draft: String,
    /// Whether the composer's mock send is in flight.
    sending: bool,
    /// How long it has been in flight.
    send_elapsed: Duration,
    /// The last prompt the composer submitted.
    sent: Option<String>,
    /// Whether the streaming replay is running.
    streaming: bool,
    /// How far into the replay it is.
    stream_elapsed: Duration,
}

impl Default for State {
    fn default() -> Self {
        Self {
            clock: FrameClock::default(),
            transcript: SEED
                .iter()
                .map(|turn| (turn.from, turn.body.to_string()))
                .collect(),
            arrivals: 0,
            pinned: true,
            draft: String::new(),
            sending: false,
            send_elapsed: Duration::ZERO,
            sent: None,
            streaming: false,
            stream_elapsed: REPLY_SETTLED,
        }
    }
}

impl State {
    /// Step the two timed demos by one frame's worth of shell time.
    fn advance(&mut self, delta: Duration) {
        if self.sending {
            self.send_elapsed += delta;
            if self.send_elapsed >= Duration::from_millis(SEND_MS) {
                self.sending = false;
            }
        }
        if self.streaming {
            self.stream_elapsed += delta;
            if self.revealed() >= REPLY.chars().count() {
                self.streaming = false;
            }
        }
    }

    /// How much of [`REPLY`] has arrived, in characters.
    fn revealed(&self) -> usize {
        let chars = (self.stream_elapsed.as_secs_f64() * REPLY_RATE).floor();
        (chars.max(0.0) as usize).min(REPLY.chars().count())
    }
}

// ---- Blocks ----------------------------------------------------------------

/// The variant's own name, for the specimen labels — the enum carries no
/// display string of its own.
fn bubble_name(variant: MessageBubbleVariant) -> &'static str {
    match variant {
        MessageBubbleVariant::Solid => "Solid",
        MessageBubbleVariant::Soft => "Soft",
        MessageBubbleVariant::Tint => "Tint",
        MessageBubbleVariant::Outline => "Outline",
        MessageBubbleVariant::Ghost => "Ghost",
        MessageBubbleVariant::Danger => "Danger",
    }
}

/// One labelled bubble specimen.
fn bubble_sample(variant: MessageBubbleVariant) -> AnyView<State> {
    any(column()
        .child(caption(bubble_name(variant)))
        .child(SizedBox(None, Some(6.0)))
        .child(
            message_bubble::<State, _>(
                text(format!(
                    "{} \u{2014} the surface a message is painted on.",
                    bubble_name(variant)
                ))
                .size(14.0),
            )
            .variant(variant)
            .align(MessageBubbleAlign::Start),
        ))
}

/// `message_bubble`: every variant, both alignments, and the two group
/// spacings consecutive turns are stacked at.
fn bubbles() -> AnyView<State> {
    let mut column: Vec<AnyView<State>> = vec![
        any(sub_heading("message_bubble")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "Six variants. The entrance is a spring pop substituted from the \
             catalog's own token table (upstream's per-component spring is not \
             one of the six), and the content fades a beat after the surface.",
        )),
        any(SizedBox(None, Some(12.0))),
    ];
    for variant in MessageBubbleVariant::ALL {
        column.push(bubble_sample(variant));
        column.push(any(SizedBox(None, Some(10.0))));
    }

    column.push(any(caption(
        "message_bubble_group stacks consecutive turns: Compact for one \
         sender's run, Default across a change of sender. The trailing bubble \
         is End-aligned.",
    )));
    column.push(any(SizedBox(None, Some(10.0))));
    column.push(any(message_bubble_group::<State, _>(vec![
        any(message_bubble::<State, _>(
            text("Compact spacing, first line.").size(14.0),
        )),
        any(message_bubble::<State, _>(
            text("Compact spacing, second line.").size(14.0),
        )),
    ])
    .spacing(MessageBubbleSpacing::Compact)));
    column.push(any(SizedBox(None, Some(10.0))));
    column.push(any(message_bubble_group::<State, _>(vec![
        any(message_bubble::<State, _>(
            text("Default spacing \u{2014} a change of sender.").size(14.0),
        )),
        any(
            message_bubble::<State, _>(text("And the reply.").size(14.0))
                .variant(MessageBubbleVariant::Solid)
                .align(MessageBubbleAlign::End),
        ),
    ])
    .spacing(MessageBubbleSpacing::Default)));

    any(Column(column))
}

/// `message`: the row a bubble sits in — avatar, sender metadata, content
/// slot, footer — plus the centred marker.
fn message_rows() -> AnyView<State> {
    any(column()
        .child(sub_heading("message"))
        .child(SizedBox(None, Some(6.0)))
        .child(caption(
            "The row, not the surface: an avatar (initials, not an image slot), \
             a name/timestamp header, any content, and a footer. A user row is \
             mirrored and anchors its entrance to the trailing edge.",
        ))
        .child(SizedBox(None, Some(12.0)))
        .child(message_marker("Today"))
        .child(SizedBox(None, Some(12.0)))
        .child(message::<State, _>(
                MessageFrom::Assistant,
                message_bubble::<State, _>(
                    text("The row hands its content slot whatever you give it \u{2014} a bubble here, a tool card elsewhere.")
                        .size(14.0),
                ),
            )
            .avatar("AI")
            .name("beUI Agent")
            .timestamp("10:24"))
        .child(SizedBox(None, Some(12.0)))
        .child(message::<State, _>(
                MessageFrom::User,
                message_bubble::<State, _>(text("And a sent row mirrors itself.").size(14.0))
                    .variant(MessageBubbleVariant::Solid)
                    .align(MessageBubbleAlign::End),
            )
            .avatar("You")
            .name("You")
            .timestamp("10:25")
            .footer("Sent"))
        .child(SizedBox(None, Some(12.0)))
        .child(message::<State, _>(
                MessageFrom::Assistant,
                message_bubble::<State, _>(
                    text("A placeholder avatar keeps a follow-up row aligned with the one above it.")
                        .size(14.0),
                ),
            )
            .avatar_placeholder(true)))
}

/// `message_scroller`: the pin-to-the-live-edge viewport, in a box with a
/// finite height so it has something to scroll.
fn scroller(state: &State) -> AnyView<State> {
    let rows: Vec<AnyView<State>> = state
        .transcript
        .iter()
        .map(|&(from, ref body)| {
            let bubble = message_bubble::<State, _>(text(body.clone()).size(14.0))
                .variant(if from.is_mirrored() {
                    MessageBubbleVariant::Solid
                } else {
                    MessageBubbleVariant::Soft
                })
                .align(if from.is_mirrored() {
                    MessageBubbleAlign::End
                } else {
                    MessageBubbleAlign::Start
                });
            any(message::<State, _>(from, bubble)
                .avatar(if from.is_mirrored() { "You" } else { "AI" })
                .animate_in(true))
        })
        .collect();

    any(column()
        .child(sub_heading("message_scroller"))
        .child(SizedBox(None, Some(6.0)))
        .child(caption(
            "The scroller owns its own offset \u{2014} the baseline scroll view \
             publishes neither an offset read nor a write seam, and pinning has \
             to act on a rebuild, which dispatches no event. It needs a bounded \
             height to have anything to scroll, and the gallery's page slot is \
             itself a scroll view, so the demo pins it to 300px here.",
        ))
        .child(SizedBox(None, Some(10.0)))
        .child(caption(format!(
            "{} rows \u{2014} {}. Scroll away from the end to unpin: arrivals \
             then preserve your place and the jump button appears. Press it, or \
             scroll back inside the 56px band, to re-pin.",
            state.transcript.len(),
            if state.pinned {
                "pinned to the live edge"
            } else {
                "unpinned"
            }
        )))
        .child(SizedBox(None, Some(12.0)))
        .child(
            SizedBox(None, Some(300.0)).child(message_scroller::<State, _>(rows).on_pin_change(
                |s: &mut State, pinned| {
                    s.pinned = pinned;
                },
            )),
        )
        .child(SizedBox(None, Some(12.0)))
        .child(
            button("Simulate incoming", |s: &mut State| {
                let index = s.arrivals % ARRIVALS.len();
                s.arrivals += 1;
                s.transcript
                    .push((MessageFrom::Assistant, ARRIVALS[index].to_string()));
            })
            .tone(ButtonTone::Outline)
            .size(ButtonSize::Sm),
        ))
}

/// `prompt_input`: the composer, controlled, with a live send state and a
/// working stop button.
fn composer(state: &State) -> AnyView<State> {
    let send_state = PromptInputSend::resolve(&state.draft, false, state.sending);
    let sent = match &state.sent {
        Some(prompt) => format!("Last submission: \u{201c}{prompt}\u{201d}"),
        None => "Nothing submitted yet.".to_string(),
    };

    any(column()
        .child(sub_heading("prompt_input"))
        .child(SizedBox(None, Some(6.0)))
        .child(caption(
            "Controlled: the widget never owns the text. Submitting starts a \
             900ms mock send, during which the send affordance becomes a stop \
             button. Upstream's model selector and prompt-actions popover are \
             not folded in \u{2014} they are shipped components a caller composes \
             beside the composer.",
        ))
        .child(SizedBox(None, Some(12.0)))
        .child(
            prompt_input::<State, _>(state.draft.clone(), |s: &mut State, value| {
                s.draft = value;
            })
            .placeholder("Ask a follow-up\u{2026}")
            .min_rows(1)
            .max_rows(4)
            .loading(state.sending)
            .on_submit(|s: &mut State, value: String| {
                s.sent = Some(value);
                s.draft.clear();
                s.sending = true;
                s.send_elapsed = Duration::ZERO;
            })
            .on_stop(|s: &mut State| {
                s.sending = false;
            }),
        )
        .child(SizedBox(None, Some(10.0)))
        .child(caption(format!(
            "Send state: {}. {sent}",
            send_state.label()
        )))
        .child(state.clock.pump(state.sending)))
}

/// `streaming_response`: the replay, plus a settled and a failed instance.
fn streaming(state: &State) -> AnyView<State> {
    let revealed: String = REPLY.chars().take(state.revealed()).collect();
    let status = if state.streaming {
        StreamingResponseStatus::Streaming
    } else {
        StreamingResponseStatus::Complete
    };

    any(column()
        .child(sub_heading("streaming_response"))
        .child(SizedBox(None, Some(6.0)))
        .child(caption(
            "Append-driven, not per-character: the whole answer is one wrapped \
             run and only the glyphs past the settled prefix fade in, so an \
             append never re-animates what is already on screen. The block \
             cursor is the port's own still-streaming affordance; markdown is \
             the caller's.",
        ))
        .child(SizedBox(None, Some(12.0)))
        .child(streaming_response(revealed).status(status).size(14.0))
        .child(SizedBox(None, Some(12.0)))
        .child(
            row()
                .child(
                    button("Replay", |s: &mut State| {
                        s.streaming = true;
                        s.stream_elapsed = Duration::ZERO;
                    })
                    .tone(ButtonTone::Outline)
                    .size(ButtonSize::Sm),
                )
                .child(SizedBox(Some(8.0), None))
                .child(
                    button("Stop", |s: &mut State| {
                        s.streaming = false;
                    })
                    .tone(ButtonTone::Ghost)
                    .size(ButtonSize::Sm),
                )
                .cross_axis(CrossAxisAlignment::Center),
        )
        .child(SizedBox(None, Some(16.0)))
        .child(caption(
            "Error status \u{2014} the body paints in the destructive ink.",
        ))
        .child(SizedBox(None, Some(6.0)))
        .child(
            streaming_response("The stream ended before the answer did.")
                .status(StreamingResponseStatus::Error)
                .size(14.0),
        )
        .child(state.clock.pump(state.streaming)))
}

/// `loading_states`: all four variants, running.
fn loading() -> AnyView<State> {
    any(column()
        .child(sub_heading("loading_states"))
        .child(SizedBox(None, Some(6.0)))
        .child(caption(
            "Four indicators in one leaf. Reasoning leads with the dot row \
             rather than upstream's ASCII loader and ships the swap transition \
             only \u{2014} the cascade and scramble variants already exist, \
             better, as text_animation. The Progress timer counts from the frame \
             it first painted, since framework-tier code reads no wall clock.",
        ))
        .child(SizedBox(None, Some(12.0)))
        .child(caption("Dots \u{2014} the typing indicator"))
        .child(SizedBox(None, Some(6.0)))
        .child(loading_states().variant(LoadingStatesVariant::Dots))
        .child(SizedBox(None, Some(14.0)))
        .child(caption("Shimmer \u{2014} thinking-shimmer"))
        .child(SizedBox(None, Some(6.0)))
        .child(
            loading_states()
                .variant(LoadingStatesVariant::Shimmer)
                .label("Thinking\u{2026}"),
        )
        .child(SizedBox(None, Some(14.0)))
        .child(caption(
            "Progress \u{2014} activity glyph, verb and live timer",
        ))
        .child(SizedBox(None, Some(6.0)))
        .child(
            loading_states()
                .variant(LoadingStatesVariant::Progress)
                .label("Reviewing the checkout flow"),
        )
        .child(SizedBox(None, Some(14.0)))
        .child(caption(
            "Reasoning \u{2014} phrases swapping on an interval",
        ))
        .child(SizedBox(None, Some(6.0)))
        .child(
            loading_states()
                .variant(LoadingStatesVariant::Reasoning)
                .phrases([
                    "Thinking",
                    "Reading the request",
                    "Working through the details",
                    "Preparing the answer",
                ]),
        ))
}

// ---- The page --------------------------------------------------------------

/// The page's component: owns [`State`], steps the two timed demos, and
/// assembles the six blocks.
struct Primitives;

impl Component for Primitives {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        // The desktop shell re-runs `build` on every frame it schedules, and
        // the pump below is what keeps those frames coming while a demo runs,
        // so this is the page's frame loop.
        let delta = state.clock.tick();
        state.advance(delta);

        any(column()
            .child(heading("Agents \u{b7} Primitives"))
            .child(SizedBox(None, Some(8.0)))
            .child(caption(
                "The parts a transcript is assembled from. Agent-supplied text \
                 is neutralised on ingestion, so a caption or a bubble body \
                 cannot smuggle control characters into the surface.",
            ))
            .child(SizedBox(None, Some(BLOCK_GAP)))
            .child(bubbles())
            .child(SizedBox(None, Some(BLOCK_GAP)))
            .child(message_rows())
            .child(SizedBox(None, Some(BLOCK_GAP)))
            .child(scroller(state))
            .child(SizedBox(None, Some(BLOCK_GAP)))
            .child(composer(state))
            .child(SizedBox(None, Some(BLOCK_GAP)))
            .child(streaming(state))
            .child(SizedBox(None, Some(BLOCK_GAP)))
            .child(loading()))
    }
}

/// The Agents · Primitives page.
pub fn page() -> AnyView<AppState> {
    any(component(Primitives))
}
