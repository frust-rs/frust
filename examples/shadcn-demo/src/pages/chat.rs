//! Chat: `message_scroller` over the `message` family — the stick-to-bottom
//! live edge, the detach-and-preserve behaviour, the scroll-to-end button, and
//! the re-stick.
//!
//! The page fills the shell's page slot rather than scrolling inside it: the
//! scroller owns its own offset and needs a **finite viewport height** to have
//! anything to scroll (given an unbounded one it would simply grow to its
//! content). So the slot's bounded height is split here — caption and composer
//! take their natural heights, the scroller takes the rest.
//!
//! `bubble()` implements `View<()>` only, so each pill mounts through
//! [`frust::component`] — the seam that hosts a state-independent subtree inside
//! any ambient state tree.

use frust::{
    AnyView, Component, CrossAxisAlignment, EdgeInsets, Padding, SizedBox, View, any, column,
    component, row,
};
use frust_shadcn::{
    BubbleAlign, BubbleVariant, ButtonVariant, MessageAlign, avatar, bubble, button, input,
    message, message_avatar, message_content, message_footer, message_group, message_header,
    message_scroller, message_scroller_item,
};

use crate::AppState;

/// One line of the transcript.
#[derive(Clone)]
pub struct Message {
    pub who: String,
    pub body: String,
    pub mine: bool,
}

impl Message {
    fn theirs(who: &str, body: &str) -> Self {
        Self {
            who: who.to_string(),
            body: body.to_string(),
            mine: false,
        }
    }

    fn mine(body: &str) -> Self {
        Self {
            who: "You".to_string(),
            body: body.to_string(),
            mine: true,
        }
    }
}

pub struct State {
    pub messages: Vec<Message>,
    pub draft: String,
    pub stuck: bool,
    /// How many canned arrivals have been simulated, so each press delivers the
    /// next one rather than repeating.
    pub arrivals: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            messages: seed(),
            draft: String::new(),
            stuck: true,
            arrivals: 0,
        }
    }
}

/// The canned opening transcript — long enough that the viewport starts
/// scrollable, so "stuck to the live edge" means something on the first frame.
fn seed() -> Vec<Message> {
    [
        (false, "Morning — did the drawer land?"),
        (true, "It did. Drag-to-close and snap points both."),
        (false, "And the exit ramp?"),
        (true, "Every modal reverses its entrance now."),
        (false, "Nice. What about the anchored family?"),
        (true, "Same, as long as the panel stays mounted."),
        (
            false,
            "So the demo keeps them mounted and passes .open(flag)?",
        ),
        (true, "Exactly that."),
        (false, "How is the sidebar holding up at 48px?"),
        (true, "Labels drop, glyphs stay, sub-lists take zero space."),
        (false, "Send me a screenshot when you get a chance."),
        (true, "Will do — right after the desktop gate."),
    ]
    .into_iter()
    .map(|(mine, body)| {
        if mine {
            Message::mine(body)
        } else {
            Message::theirs("Robin", body)
        }
    })
    .collect()
}

/// The replies "Simulate incoming" delivers, in order (a press is an arrival —
/// no timer is involved, and none is needed to prove the scroll behaviour).
const ARRIVALS: [&str; 6] = [
    "One more thing —",
    "the scroller detaches the moment you scroll up.",
    "Growth then preserves your viewport instead of jumping.",
    "The floating button only shows while you are detached.",
    "Pressing it glides back to the end and re-sticks.",
    "That is the whole contract.",
];

/// A chat pill. `bubble()` is `View<()>`, so it rides a component boundary.
struct Pill {
    body: String,
    variant: BubbleVariant,
    align: BubbleAlign,
}

impl Component for Pill {
    type State = ();

    fn init(&self) -> Self::State {}

    fn build(&self, _state: &mut Self::State) -> impl View<Self::State> {
        any(bubble(self.body.clone())
            .variant(self.variant)
            .align(self.align))
    }
}

/// One transcript row: an avatar plus a headed bubble for the other party, a
/// trailing-aligned bubble with a footer for the local user.
fn transcript_row(entry: &Message) -> AnyView<AppState> {
    if entry.mine {
        any(message_scroller_item(
            message(vec![message_content(vec![
                any(component(Pill {
                    body: entry.body.clone(),
                    variant: BubbleVariant::Default,
                    align: BubbleAlign::End,
                })),
                any(message_footer("Sent").align(MessageAlign::End)),
            ])])
            .align(MessageAlign::End),
        ))
    } else {
        any(message_scroller_item(message(vec![
            any(message_avatar(avatar::<AppState>().fallback("RB"))),
            any(message_content(vec![
                any(message_header(entry.who.clone())),
                any(component(Pill {
                    body: entry.body.clone(),
                    variant: BubbleVariant::Muted,
                    align: BubbleAlign::Start,
                })),
            ])),
        ])))
    }
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let stuck = state.stuck;
    let draft = state.draft.clone();
    let count = state.messages.len();

    let items: Vec<AnyView<AppState>> = state.messages.iter().map(transcript_row).collect();

    let header = frust::column()
        .child(crate::nav::heading("Chat"))
        .child(SizedBox(None, Some(8.0)))
        .child(crate::nav::caption(format!(
            "{count} messages \u{2014} {}. Scroll up to detach: new arrivals then \
             preserve your place and the floating button appears at the bottom \
             right. Press it (or scroll back into the bottom band) to re-stick.",
            if stuck {
                "stuck to the live edge"
            } else {
                "detached"
            }
        )))
        .child(SizedBox(None, Some(12.0)));

    // A `message_group` around the scroller's rows would double the column;
    // the scroller already stacks its items, so the group is used for the one
    // static example under the composer instead.
    let scroller = message_scroller(items)
        .button_label("Jump to the newest message")
        .on_stick_change(|s: &mut AppState, stuck: bool| {
            s.chat.stuck = stuck;
        });

    let composer = row()
        .flex(
            1,
            input(draft, |s: &mut AppState, v: String| {
                s.chat.draft = v;
            })
            .placeholder("Write a message\u{2026}")
            .on_submit(|s: &mut AppState, _v: String| send(s)),
        )
        .child(SizedBox::<AppState>(Some(8.0), None))
        .child(button("Send", |s: &mut AppState| send(s)))
        .child(SizedBox::<AppState>(Some(8.0), None))
        .child(
            button("Simulate incoming", |s: &mut AppState| {
                let index = s.chat.arrivals % ARRIVALS.len();
                s.chat.arrivals += 1;
                s.chat
                    .messages
                    .push(Message::theirs("Robin", ARRIVALS[index]));
            })
            .variant(ButtonVariant::Outline),
        )
        .cross_axis(CrossAxisAlignment::Center);

    Padding(
        EdgeInsets::all(24.0),
        column()
            .child(header)
            .flex(1, scroller)
            .child(SizedBox::<AppState>(None, Some(12.0)))
            .child(composer)
            .child(SizedBox::<AppState>(None, Some(12.0)))
            .child(message_group(vec![message(vec![message_content(vec![
                component(Pill {
                    body: "message_group / message / message_content compose the \
                               same parts outside the scroller too."
                        .to_string(),
                    variant: BubbleVariant::Outline,
                    align: BubbleAlign::Start,
                }),
            ])])]))
            .cross_axis(CrossAxisAlignment::Stretch),
    )
}

/// Append the draft as a local message and clear the composer.
fn send(state: &mut AppState) {
    let body = state.chat.draft.trim().to_string();
    if body.is_empty() {
        return;
    }
    state.chat.messages.push(Message::mine(&body));
    state.chat.draft.clear();
}
