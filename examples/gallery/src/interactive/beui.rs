//! Stateful constructors for the beUI design system's cases (`beui/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! # Seeding
//!
//! Every constructor seeds [`Component::init`] with the values its recorded
//! twin hardcodes, so the live frame opens on the poster it replaces — the
//! rule [`super::base`] sets out at length. Nothing here is a `Case::build`,
//! so none of it reaches the snapshot oracle and no poster moves.
//!
//! # Three cases that need no state, and what they need instead
//!
//! `crate::beui`'s module docs list five cases held back "for want of a
//! pointer or a live clock". A browser has both, so three of those five turn
//! out to need nothing from this table — and saying so is the finding. They
//! are deliberately absent from [`INTERACTIVE`]:
//!
//! - **`beui/tilt-card` is already fully interactive.** Its lean, its glare
//!   and its shadow are `PointerTracker` state inside the widget, driven by
//!   pointer *moves* with no press and no app value anywhere in the path.
//!   Measured live: one move onto the card repaints a region larger than the
//!   card's own rest rect, because the tilted plate and its shadow draw
//!   outside their layout box, and a move back off springs it flat again. A
//!   row here would add a `Component` around a case that already works.
//! - **`beui/button` needs *constructing*, not state.** `crate::beui`'s
//!   `button_case` builds six buttons and all six are `ButtonVariant::Base`,
//!   so `Metallic` and `Magnetic` are not merely inert — they are absent.
//!   Both are widget-owned once built (a drifting reflection off frame time,
//!   a pointer-tracked translation), so neither would hold a field in this
//!   table even after the case gained them. `Stateful` is the one variant
//!   that *is* app state, and it is still out of reach: its `Loading` phase
//!   suppresses activation, so a press can enter the machine and nothing in
//!   a `Component` can advance it out without a clock. Adding the two
//!   pointer/clock variants belongs to the recorded case, whose poster would
//!   then have to be re-recorded.
//! - **`beui/message-bubble` is a settled transcript by construction.**
//!   `animate_in` is left `false`, and turning it on is a prop on the
//!   recorded case, not a value an app owns — there is nothing for a
//!   `Component` to hold.
//!
//! `beui/code-block` is the fifth, and the registry already calls it
//! static-OK: its collapse toggle is a no-op at this case's three lines
//! (`CODE_BLOCK_COLLAPSED_LINES` is three), and its copy affordance is
//! *painted only when `on_copy` is wired* — so making it live would add a
//! glyph the poster does not have.
//!
//! # Where the two text-bearing fields validate, and why it is not shadcn's
//! answer
//!
//! [`super::shadcn`]'s `input` case checks on submit because a per-keystroke
//! verdict there destroys the focused control: `FieldView::shape` is the
//! presence triple of its label/description/error slots and a change to it
//! forces a full child rebuild. **beUI's wrapper does not have that
//! problem.** `InputWidget` owns exactly one `ChildPod` — the wrapped
//! baseline field — and its label and message are widget-held shaped runs,
//! not children; `InputView::rebuild` calls `rebuild_child` on that one
//! control unconditionally, and an error slot appearing only retargets a lane
//! and flags `LAYOUT | PAINT`. A per-keystroke verdict would keep focus here.
//!
//! [`InputCase`] still checks on submit, for two reasons that are this
//! catalog's rather than shadcn's: the message row is a real row in the
//! field's column, so a verdict that changes per keystroke re-centres the
//! whole frame under the caret; and the destructive shake is the component's
//! signature motion, which reads as a deliberate answer to Enter and as noise
//! when it fires on a letter. The stored value is still exactly what
//! `on_change` reported — see [`InputCase`]'s own note for why normalising it
//! would cost an IME composition, not merely a caret.

use frust_core::{AnyView, Component, View, any, component};
use frust_widgets::{Align, Alignment, GestureDetector, SizedBox, column, row, stack, text};
use kurbo::Size;
use peniko::Color;

use frust_beui::agents::chat_app::{ChatMessage, ChatModel, ChatRole, chat_conversation};
use frust_beui::agents::prompt_input::prompt_input;
use frust_beui::blocks::command_palette::{command_palette, command_palette_item};
use frust_beui::blocks::dynamic_island::{dynamic_island, dynamic_island_slot};
use frust_beui::blocks::otp_input::{OtpStatus, otp_input};
use frust_beui::blocks::wallet_card::{wallet_account, wallet_card};
use frust_beui::components::button::{ButtonTone, button};
use frust_beui::components::checkbox::checkbox;
use frust_beui::components::input::input;
use frust_beui::components::radio::radio;
use frust_beui::components::switch::switch;
use frust_beui::components::tabs::{TabsVariant, tabs, tabs_tab};

use super::{Entry, framed, framed_in};

/// This catalog's slice of the side table [`super::entries`] concatenates, in
/// the registry's own slug order so the two read side by side.
///
/// Nine of the catalog's thirteen cases. The four absent ones are
/// `beui/button`, `beui/code-block`, `beui/message-bubble` and
/// `beui/tilt-card` — see the [module docs](self) for what each one needs
/// instead, which in three of the four is nothing at all.
pub const INTERACTIVE: &[Entry] = &[
    ("beui/chat", chat_case),
    ("beui/command-palette", command_palette_case),
    ("beui/dynamic-island", dynamic_island_case),
    ("beui/form-controls", form_controls_case),
    ("beui/input", input_case),
    ("beui/otp-input", otp_input_case),
    ("beui/prompt-input", prompt_input_case),
    ("beui/tabs", tabs_case),
    ("beui/wallet-card", wallet_card_case),
];

// ---- frame sizes and layout helpers -----------------------------------------
//
// Every frame constant below is repeated from `crate::beui`, which spells its
// own as inline literals at each `framed_in` call. The two must stay in step:
// a live case framed differently from its poster is precisely the defect this
// side table exists to avoid. These are declarations of the *same* value, not
// a second opinion about it.

/// The `chat` frame.
const CHAT: Size = Size::new(440.0, 396.0);
/// The `command-palette` frame.
const COMMAND_PALETTE: Size = Size::new(480.0, 360.0);
/// The `form-controls` frame.
const FORM_CONTROLS: Size = Size::new(360.0, 280.0);
/// The `input` frame.
const INPUT: Size = Size::new(360.0, 300.0);
/// The `otp-input` frame — [`crate::case::Case::DEFAULT_SIZE`], written out
/// the way the recorded case writes it.
const OTP: Size = Size::new(360.0, 240.0);
/// The `prompt-input` frame.
const PROMPT_INPUT: Size = Size::new(400.0, 240.0);
/// The `tabs` frame.
const TABS: Size = Size::new(400.0, 240.0);
/// The `wallet-card` frame.
const WALLET_CARD: Size = Size::new(400.0, 340.0);

/// The ink every `dynamic-island` slot child paints in, repeated from
/// `crate::beui`'s private `ISLAND_INK` for the reason the frame constants
/// give.
///
/// A fixed mid-neutral rather than a token, because the island's shell is
/// *inverted* chrome and a case cannot know which scheme it is being built
/// against — see that module's *inverted-chrome ink gap*.
const ISLAND_INK: Color = Color::from_rgb8(0x75, 0x75, 0x75);

/// A fixed vertical gap, at whatever state the surrounding case is bound to —
/// the state-generic mirror of `crate::beui`'s own `()`-bound helper.
fn gap_y<State: 'static>(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A fixed horizontal gap, likewise.
fn gap_x<State: 'static>(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

// ---- chat -------------------------------------------------------------------

/// The answer this case's assistant gives, whatever it is asked.
///
/// It names its own limitation rather than pretending to be a model, because
/// the limitation is the honest one: [`ChatModel`]'s mock driver reveals a
/// reply through `ChatModel::advance(delta)`, which wants a *per-frame* call,
/// and a [`Component`] has no frame hook to make it from. See [`ChatCase`].
const CHAT_REPLY: &str = "Answered whole rather than token by token \u{2014} this preview has no \
                          per-frame clock to stream one with.";

/// Retained state for the interactive `chat` case: the whole conversation.
///
/// One field, because [`ChatModel`] already *is* the case's state — turns,
/// composer text and the in-flight reply live in it together, and the view is
/// a pure function of it.
struct ChatState {
    model: ChatModel,
}

/// The `chat` case with a composer that composes and a transcript that grows.
///
/// The recorded case builds its [`ChatModel`] inside the case function, which
/// is what makes it inert twice over: the model is discarded and rebuilt on
/// every rebuild pass, and `on_input`/`on_submit` are never wired at all, so
/// the composer's own send affordance reports nothing. Moving the model into
/// retained state and wiring the seams is the whole conversion.
///
/// # What a submitted turn does, and the one thing it does not
///
/// `ChatModel::submit` pushes the user turn, clears the composer and opens a
/// run; a real transport would then feed `push_token` as bytes arrive. This
/// case has no transport, so it pushes [`CHAT_REPLY`] whole and closes the run
/// in the same handler. The alternative — leaving the run open so the
/// streaming caret plays — was rejected as the worse lie: nothing further
/// would ever arrive, and the composer would sit in its stop state waiting for
/// it.
///
/// Token-by-token streaming needs `ChatModel::advance` called once a frame,
/// and this side table has no seam for that: a `Component` is handed
/// `&mut State` on a rebuild, never a clock. That is a gap in the live host,
/// not in the plugin.
///
/// `on_stop` is deliberately unwired: with the reply closed in the same
/// handler that opened it, `is_busy` is never observably true, so a stop
/// button is never reachable and a handler for it would be dead.
struct ChatCase;

impl Component for ChatCase {
    type State = ChatState;

    fn init(&self) -> Self::State {
        ChatState {
            model: ChatModel::new().with_history(vec![
                ChatMessage::new("m1", ChatRole::User, "Summarize the release notes."),
                ChatMessage::new(
                    "m2",
                    ChatRole::Assistant,
                    "Three changes landed: the strip pipeline, the beUI port, and the preview registry.",
                ),
                ChatMessage::new("m3", ChatRole::User, "Which one is the biggest?"),
                ChatMessage::new(
                    "m4",
                    ChatRole::Assistant,
                    "The beUI port \u{2014} 81 components across three catalogs.",
                ),
            ]),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            CHAT,
            SizedBox(Some(376.0), None).child(
                chat_conversation::<ChatState>(&state.model)
                    .placeholder("Ask a follow-up\u{2026}")
                    .avatars("You", "AI")
                    // The composer's text, stored exactly as reported — the
                    // rule `super::base`'s `TextInputCase` sets out.
                    .on_input(|state: &mut ChatState, text| state.model.set_input(text))
                    .on_submit(|state: &mut ChatState, text| {
                        if state.model.submit(text) {
                            state.model.push_token(CHAT_REPLY);
                            state.model.finish_reply();
                        }
                    }),
            ),
        )
    }
}

/// The [`super::Build`] the table registers: a `()`-stated `AnyView` around
/// the stateful component, legal because `ComponentView<C>` implements
/// `View<Outer>` for every `Outer`. Every constructor below repeats this
/// three-line shape.
fn chat_case() -> AnyView<()> {
    any(component(ChatCase))
}

// ---- command-palette --------------------------------------------------------

/// The commands the palette offers: label, group, and the shortcut hint (empty
/// for a row that has none).
///
/// A table rather than a built `Vec`, because the rows are needed twice — once
/// to build the items and once to name the one that was chosen, after the
/// items have been moved into the view.
const PALETTE_COMMANDS: [(&str, &str, &str); 4] = [
    ("New project", "Actions", "\u{2318}N"),
    ("Open preview", "Actions", "\u{2318}O"),
    ("Toggle theme", "View", ""),
    ("Go to settings", "View", ""),
];

/// Retained state for the interactive `command-palette` case: whether the
/// palette is up, the filter text, and the command that last ran.
///
/// `open` is seeded `true` and `query` empty, which is the recorded frame.
/// `last` is `None` until something is chosen, so the opening frame carries
/// the palette's own default placeholder rather than a readout.
struct CommandPaletteState {
    open: bool,
    query: String,
    last: Option<String>,
}

/// The `command-palette` case with a filter that filters and a dismissal that
/// dismisses.
///
/// # What the live check found, and what it did not fix
///
/// `crate::beui` recorded this case blank until the registry gained a warm
/// pass, because the panel and the scrim are both `Presence`-staged and a
/// single paint reports progress 0 for each. That is a *recorder* problem: a
/// live host paints continuously, so the entrance runs on page load with or
/// without this constructor, and the panel was already arriving correctly
/// before any of the state below existed. What was still pinned is everything
/// the app owns — the filter text (`""`, so the list could never be filtered),
/// the open flag, and an `on_select` writing to `()`.
///
/// # The `ESC` key cap has to be true
///
/// The panel paints a `kbd` reading `ESC`. Wiring `on_open_change` is what
/// makes that instruction followable — Escape, a backdrop press and a
/// selection all report `false` through it — and it is also what forces the
/// question of how the reader gets the palette *back*, since upstream's ⌘K
/// listener is a window-level binding a widget cannot own (the plugin's own
/// module docs say so, and offer `CommandPaletteController` for an app to bind
/// it from).
///
/// The answer is the shape the plugin documents for a hosted palette: a
/// [`Stack`] with the app underneath and the palette on top. The app here is
/// one button. Two details make it cost the poster nothing:
///
/// - The stack's arity never changes. Slot 0 holds the trigger while the
///   palette is shut and an empty [`SizedBox`] while it is up, so the palette
///   keeps slot 1 across every flip and its exit ramp is never torn down by a
///   positional reshuffle. An open palette therefore stacks over *nothing*,
///   which matters because the scrim is 40% alpha and would otherwise show a
///   button the poster does not have.
/// - The trigger mounts the moment `open` goes false, while the panel is still
///   sliding out. That overlap is correct rather than sloppy: the modal host
///   stays a barrier for the whole of its exit, so the trigger is visible
///   before it is pressable, exactly as it would be in an app.
///
/// # The chosen command reports through the placeholder
///
/// A selection needs a visible consequence, and the palette is a modal filling
/// its own frame — there is no margin to hang a readout line in the way
/// [`super::material`]'s `menu` case does. The field's placeholder is the
/// readout instead: `on_select` clears the query, and the empty field then
/// says what just ran. It costs no geometry, and it is absent until the first
/// selection, so the opening frame is still the recorded one.
struct CommandPaletteCase;

impl Component for CommandPaletteCase {
    type State = CommandPaletteState;

    fn init(&self) -> Self::State {
        CommandPaletteState {
            open: true,
            query: String::new(),
            last: None,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let items = PALETTE_COMMANDS
            .iter()
            .map(|(label, group, hint)| {
                let item = command_palette_item(*label).group(*group);
                if hint.is_empty() {
                    item
                } else {
                    item.hint(*hint)
                }
            })
            .collect();
        let mut palette = command_palette(
            items,
            state.query.clone(),
            // Stored verbatim: the palette's filter field is the wrapped
            // baseline editable, and normalising here would reset it.
            |state: &mut CommandPaletteState, text| state.query = text,
            |state: &mut CommandPaletteState, index| {
                state.last = PALETTE_COMMANDS
                    .get(index)
                    .map(|(label, _, _)| (*label).to_string());
                state.query.clear();
            },
        )
        .open(state.open)
        .label("Gallery command palette")
        .on_open_change(|state: &mut CommandPaletteState, open| state.open = open);
        if let Some(last) = &state.last {
            palette = palette.placeholder(format!("Ran {last} \u{2014} type another command"));
        }
        let backdrop: AnyView<CommandPaletteState> = if state.open {
            any(SizedBox(None, None))
        } else {
            any(Align(
                Alignment::CENTER,
                button("Open the palette", |state: &mut CommandPaletteState| {
                    state.open = true
                })
                .tone(ButtonTone::Secondary),
            ))
        };
        framed_in(COMMAND_PALETTE, stack().child(backdrop).child(palette))
    }
}

fn command_palette_case() -> AnyView<()> {
    any(component(CommandPaletteCase))
}

// ---- dynamic-island ---------------------------------------------------------

/// The island's views, in the order a tap walks them: the two slots, then the
/// compact pill, then round again.
const ISLAND_VIEWS: [Option<&str>; 3] = [Some("music"), Some("call"), None];

/// Retained state for the interactive `dynamic-island` case: which slot is
/// showing, or `None` for the compact pill. Seeded to `music`, the recorded
/// frame.
struct DynamicIslandState {
    view: Option<String>,
}

impl DynamicIslandState {
    /// Advance to the next view in [`ISLAND_VIEWS`], wrapping.
    fn advance(&mut self) {
        let current = self
            .view
            .as_deref()
            .and_then(|id| ISLAND_VIEWS.iter().position(|slot| *slot == Some(id)))
            .unwrap_or(ISLAND_VIEWS.len() - 1);
        let next = (current + 1) % ISLAND_VIEWS.len();
        self.view = ISLAND_VIEWS[next].map(str::to_string);
    }
}

/// The `dynamic-island` case with its morph reachable.
///
/// The morph *is* the component — the shell springs between the compact pill's
/// box and the active slot's while the outgoing content lifts out and the
/// incoming one lifts in — and a pinned `view` is the one thing that keeps it
/// from ever running. One `Option<String>` is the whole difference.
///
/// # The tap has to live inside a slot
///
/// The island publishes no callback at all: it is a `role="status"` readout,
/// and its own `Widget::event` says so — "the island itself handles nothing —
/// every control on it is the caller's own child inside the visible slot",
/// with events routed to the active slot's pod (or to the compact child when
/// no slot is up). So the affordance is a [`GestureDetector`] wrapped around
/// each child, which is a pass-through in layout and therefore costs the
/// recorded composition nothing. Advancing on a tap rather than offering
/// per-slot buttons is what keeps it that way: a button row inside the music
/// slot would resize the shell the poster froze.
struct DynamicIslandCase;

impl Component for DynamicIslandCase {
    type State = DynamicIslandState;

    fn init(&self) -> Self::State {
        DynamicIslandState {
            view: Some("music".to_string()),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let compact = GestureDetector(column().child(text("9:41").size(13.0).color(ISLAND_INK)))
            .on_tap(|state: &mut DynamicIslandState| state.advance());
        let slots = vec![
            dynamic_island_slot(
                "music",
                GestureDetector(
                    column()
                        .child(text("NOW PLAYING").size(10.0).color(ISLAND_INK))
                        .child(gap_y(4.0))
                        .child(
                            text("Weightless \u{b7} Marconi Union")
                                .size(13.0)
                                .color(ISLAND_INK),
                        ),
                )
                .on_tap(|state: &mut DynamicIslandState| state.advance()),
            ),
            dynamic_island_slot(
                "call",
                GestureDetector(column().child(text("INCOMING CALL").size(10.0).color(ISLAND_INK)))
                    .on_tap(|state: &mut DynamicIslandState| state.advance()),
            ),
        ];
        framed(dynamic_island(state.view.clone(), compact, slots).label("Live activity"))
    }
}

fn dynamic_island_case() -> AnyView<()> {
    any(component(DynamicIslandCase))
}

// ---- form-controls ----------------------------------------------------------

/// The `form-controls` radio group's first key.
const PLAN_MONTHLY: &str = "monthly";
/// Its second.
const PLAN_YEARLY: &str = "yearly";

/// Retained state for the interactive `form-controls` case: one flag per
/// control plus the radio group's chosen key, each seeded to the value the
/// recorded case hardcodes.
///
/// `partial` and `partial_mixed` are two fields for one checkbox because the
/// catalog's checkbox takes `checked` and `indeterminate` as independent props
/// and resolves neither — see [`FormControlsCase`].
struct FormControlsState {
    notifications: bool,
    partial: bool,
    partial_mixed: bool,
    plan: &'static str,
    dark_mode: bool,
}

/// The `form-controls` case — the cheapest state in this catalog and a large
/// share of its motion.
///
/// The switch is the reason: its track colour ramp and its handle travel are
/// separate lanes, so the handle arrives while the track is still crossfading.
/// The checkbox's tick draws itself on over its own duration and the radio's
/// inner dot scales in; none of it is reachable while every value is a
/// literal.
///
/// # The indeterminate box needs the app to resolve it
///
/// `checkbox` reports `on_checked_change(state, !checked)` and, in the
/// plugin's own words, offers "no third-value resolution" — an indeterminate
/// box reports `true` on a press and keeps painting its minus until the app
/// says otherwise. Clearing `partial_mixed` in the same handler is that
/// resolution: press once and the minus becomes a tick, which is the tri-state
/// behaviour the recorded frame can only pose in.
///
/// # The second switch stays dead on purpose
///
/// `switch(false).disabled(true)` is the row's *point* — it is there to show
/// the disabled treatment — and a disabled control fires nothing. Giving it a
/// field would be a field nothing could ever write, so it keeps its literal.
struct FormControlsCase;

impl Component for FormControlsCase {
    type State = FormControlsState;

    fn init(&self) -> Self::State {
        FormControlsState {
            notifications: true,
            partial: false,
            partial_mixed: true,
            plan: PLAN_MONTHLY,
            dark_mode: true,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            FORM_CONTROLS,
            column()
                .child(
                    row()
                        .child(
                            checkbox(
                                state.notifications,
                                |state: &mut FormControlsState, checked| {
                                    state.notifications = checked
                                },
                            )
                            .label("Notifications"),
                        )
                        .child(gap_x(10.0))
                        .child(text("Notifications").size(14.0))
                        .child(gap_x(24.0))
                        .child(
                            checkbox(state.partial, |state: &mut FormControlsState, checked| {
                                state.partial = checked;
                                state.partial_mixed = false;
                            })
                            .indeterminate(state.partial_mixed)
                            .label("Partial"),
                        )
                        .child(gap_x(10.0))
                        .child(text("Partial").size(14.0)),
                )
                .child(gap_y(18.0))
                .child(
                    row()
                        .child(
                            radio(
                                state.plan == PLAN_MONTHLY,
                                |state: &mut FormControlsState| state.plan = PLAN_MONTHLY,
                            )
                            .label("Monthly"),
                        )
                        .child(gap_x(10.0))
                        .child(text("Monthly").size(14.0))
                        .child(gap_x(24.0))
                        .child(
                            radio(
                                state.plan == PLAN_YEARLY,
                                |state: &mut FormControlsState| state.plan = PLAN_YEARLY,
                            )
                            .label("Yearly"),
                        )
                        .child(gap_x(10.0))
                        .child(text("Yearly").size(14.0)),
                )
                .child(gap_y(18.0))
                .child(
                    row()
                        .child(
                            switch(state.dark_mode, |state: &mut FormControlsState, checked| {
                                state.dark_mode = checked
                            })
                            .label("Dark mode"),
                        )
                        .child(gap_x(10.0))
                        .child(text("Dark mode").size(14.0))
                        .child(gap_x(24.0))
                        .child(
                            switch(false, |_: &mut FormControlsState, _: bool| {})
                                .disabled(true)
                                .label("Beta features"),
                        )
                        .child(gap_x(10.0))
                        .child(text("Beta features").size(14.0)),
                ),
        )
    }
}

fn form_controls_case() -> AnyView<()> {
    any(component(FormControlsCase))
}

// ---- input ------------------------------------------------------------------

/// The handles this case's imaginary directory has already given away, lower
/// case. `"nope"` is the recorded field's own value, which is what makes its
/// pinned error true at rest.
const TAKEN_HANDLES: [&str; 2] = ["nope", "admin"];

/// The verdict on the handle field, as of the last submission.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HandleVerdict {
    /// Nothing has been asked yet: ordinary chrome, no message row.
    Unchecked,
    /// The handle is spoken for: destructive chrome, the shake, the message.
    Taken,
    /// The handle is available: the success check draws itself on.
    Free,
}

/// Retained state for the interactive `input` case: one `String` per field,
/// plus the handle's verdict.
///
/// The three strings are seeded to what the recorded case shows — a filled
/// address, an empty field behind its placeholder, and the taken handle — and
/// the verdict is seeded [`HandleVerdict::Taken`] so the opening frame carries
/// the destructive ring and the message line the poster has.
struct InputState {
    email: String,
    workspace: String,
    handle: String,
    handle_verdict: HandleVerdict,
}

/// The `input` case with a value round trip, and with the invalid state made
/// reachable instead of pinned.
///
/// The recorded case pins `.error("That handle is taken.")` beside a literal
/// `"nope"` — two constants that agree only because someone typed them to
/// agree. Here the verdict is state, so the error can genuinely clear and
/// genuinely come back, and the field's own success check becomes reachable as
/// its other half.
///
/// # Why the check is on submit
///
/// Not for [`super::shadcn`]'s reason. That catalog's field wrapper rebuilds
/// all of its children when a slot appears, which takes the focused control
/// down with them; beUI's holds the editable in a single `ChildPod` and keeps
/// its label and message as widget-owned shaped runs, so a per-keystroke
/// verdict here would keep focus (see the [module docs](self)). Two other
/// things argue for submit anyway:
///
/// - The message row is a row in the field's own column. A verdict that
///   changes as you type adds and removes it per keystroke, and the frame —
///   which centres its content — re-centres under the caret each time.
/// - The destructive shake is the component's signature. Fired by Enter it
///   reads as an answer; fired by a letter it reads as a twitch.
///
/// So `on_change` stores and nothing else, and Enter is what re-checks — the
/// same division [`super::shadcn`]'s case lands on from the other direction,
/// and what a real form does with a name-availability check.
///
/// # Storing what the editor reported
///
/// The check reads `trim()` and lower-cases, but the change handler stores
/// `value` **verbatim** — [`super::base`]'s `TextInputCase` rule 3.
/// `TextInputView::rebuild` adopts the app's value only when it differs from
/// the editor's own text, and `set_controlled_value` then reapplies the whole
/// editing state with the caret collapsed to the end **and `composing: None`**:
/// normalising in the handler would drop a selection and tear down an
/// in-flight IME composition on every keystroke, not merely move a caret.
/// Deriving a sibling prop from the stored value — which is all the verdict
/// is — is not that.
struct InputCase;

impl Component for InputCase {
    type State = InputState;

    fn init(&self) -> Self::State {
        InputState {
            email: "ada@example.com".to_string(),
            workspace: String::new(),
            handle: "nope".to_string(),
            handle_verdict: HandleVerdict::Taken,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let mut handle = input(state.handle.clone(), |state: &mut InputState, value| {
            state.handle = value;
        })
        .label("Handle")
        .on_submit(|state: &mut InputState, value: String| {
            let asked = value.trim().to_lowercase();
            state.handle_verdict = if asked.is_empty() {
                HandleVerdict::Unchecked
            } else if TAKEN_HANDLES.contains(&asked.as_str()) {
                HandleVerdict::Taken
            } else {
                HandleVerdict::Free
            };
        });
        match state.handle_verdict {
            HandleVerdict::Taken => handle = handle.error("That handle is taken."),
            HandleVerdict::Free => handle = handle.success(true),
            HandleVerdict::Unchecked => {}
        }
        framed_in(
            INPUT,
            column()
                .child(
                    SizedBox(Some(280.0), None).child(
                        input(state.email.clone(), |state: &mut InputState, value| {
                            state.email = value;
                        })
                        .label("Email"),
                    ),
                )
                .child(gap_y(14.0))
                .child(
                    SizedBox(Some(280.0), None).child(
                        input(state.workspace.clone(), |state: &mut InputState, value| {
                            state.workspace = value;
                        })
                        .label("Workspace")
                        .placeholder("acme-inc"),
                    ),
                )
                .child(gap_y(14.0))
                .child(SizedBox(Some(280.0), None).child(handle)),
        )
    }
}

fn input_case() -> AnyView<()> {
    any(component(InputCase))
}

// ---- otp-input --------------------------------------------------------------

/// The code this case accepts. Its first three digits are the ones the
/// recorded field already shows, so the reader is three keystrokes from either
/// verdict.
const OTP_EXPECTED: &str = "428913";

/// Retained state for the interactive `otp-input` case: the digits entered so
/// far and the verdict on them, seeded to the recorded half-filled row in
/// [`OtpStatus::Idle`].
struct OtpState {
    code: String,
    status: OtpStatus,
}

/// The `otp-input` case with both of its ramps reachable.
///
/// `crate::beui` records this case `Idle` because "`Success`/`Error` are ramps
/// (the check draw, the row shake) that a single-frame recording would capture
/// at progress 0". A live host has the frames; what it lacked was anything
/// able to *set* the status, since the recorded case hands `on_change` a `()`
/// and never wires `on_complete` at all.
///
/// The verdict arrives on completion rather than on a button, because that is
/// where the component puts it: `on_complete` fires once on the empty-to-full
/// transition, after `on_change` in the same commit, so the handler order
/// resolves cleanly — the edit clears any stale verdict and the completion
/// writes the new one over it.
///
/// The error message names the accepted code. That is deliberate: without it
/// the shake is the only verdict a reader can ever reach, and the check draw —
/// the slower and more characteristic of the two — stays as unreachable as it
/// was in the recorded frame. Both messages sit in the row the hint already
/// occupies (the widget shows exactly one of hint/success/error), so neither
/// adds a row and the `Idle` frame is unchanged.
struct OtpInputCase;

impl Component for OtpInputCase {
    type State = OtpState;

    fn init(&self) -> Self::State {
        OtpState {
            code: "428".to_string(),
            status: OtpStatus::Idle,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            OTP,
            otp_input(state.code.clone(), |state: &mut OtpState, value| {
                state.code = value;
                state.status = OtpStatus::Idle;
            })
            .length(6)
            .label("Verification code")
            .hint("Enter the six digits we sent you.")
            .status(state.status)
            .success_message("Code accepted.")
            .error_message("Not the code we sent \u{2014} it was 428913.")
            .on_complete(|state: &mut OtpState, code: String| {
                state.status = if code == OTP_EXPECTED {
                    OtpStatus::Success
                } else {
                    OtpStatus::Error
                };
            }),
        )
    }
}

fn otp_input_case() -> AnyView<()> {
    any(component(OtpInputCase))
}

// ---- prompt-input -----------------------------------------------------------

/// Retained state for the interactive `prompt-input` case: the draft in the
/// composer, and whether a reply is in flight.
///
/// The draft is seeded to the recorded case's own pinned text, which is what
/// makes the send affordance read as enabled on the opening frame.
struct PromptInputState {
    draft: String,
    sending: bool,
}

/// The `prompt-input` case with a send that sends and a stop that stops.
///
/// The recorded case pins a draft "so the send affordance reads as enabled",
/// and the affordance then does nothing — the case wires neither `on_submit`
/// nor `on_stop`, and the composer's whole state machine is derived from props
/// it cannot change. Two fields put all three of its states in reach:
/// `Disabled` when the draft is blank, `Ready` when it is not, and `Streaming`
/// while a reply is pending, with the arrow and the square swapping on the
/// catalog's own swap spring between them.
///
/// The chain closes on itself and strands nothing: sending clears the draft
/// and puts the button in its stop state (which also makes the field inert,
/// upstream's own behaviour), stopping returns it, and the emptied draft then
/// reads `Disabled` — the honest state for a composer with nothing in it.
struct PromptInputCase;

impl Component for PromptInputCase {
    type State = PromptInputState;

    fn init(&self) -> Self::State {
        PromptInputState {
            draft: "Draft a changelog entry for the beUI port".to_string(),
            sending: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            PROMPT_INPUT,
            SizedBox(Some(320.0), None).child(
                prompt_input::<PromptInputState, _>(
                    state.draft.clone(),
                    // Verbatim, for the reason `InputCase` gives at length.
                    |state: &mut PromptInputState, value| state.draft = value,
                )
                .placeholder("Ask a follow-up\u{2026}")
                .min_rows(2)
                .max_rows(4)
                .loading(state.sending)
                .on_submit(|state: &mut PromptInputState, _: String| {
                    state.draft.clear();
                    state.sending = true;
                })
                .on_stop(|state: &mut PromptInputState| state.sending = false),
            ),
        )
    }
}

fn prompt_input_case() -> AnyView<()> {
    any(component(PromptInputCase))
}

// ---- tabs -------------------------------------------------------------------

/// Retained state for the interactive `tabs` case: the active tab's value,
/// seeded to the one the recorded case pins.
struct TabsState {
    active: String,
}

/// The `tabs` case with a strip that swaps, and a sentence that becomes true.
///
/// `tabs` is controlled on its `value`: pressing a tab reports the value it
/// wants and moves neither the pill nor the panel until that value comes back
/// down. One `String` is the whole conversion — the panel follows for free,
/// because the widget shows the matching `tabs_tab`'s own child, and the pill
/// travels on its own spring between the two positions.
///
/// It also settles the case's own copy. The recorded panel reads "The active
/// panel swaps under the pill." over a strip that cannot swap: an instruction
/// the reader could not follow, which is exactly what [`super::base`]'s rule
/// about captions is about. The sentence is kept verbatim here because it is
/// now a description rather than a claim.
struct TabsCase;

impl Component for TabsCase {
    type State = TabsState;

    fn init(&self) -> Self::State {
        TabsState {
            active: "overview".to_string(),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            TABS,
            SizedBox(Some(340.0), None).child(
                tabs(
                    state.active.clone(),
                    vec![
                        tabs_tab(
                            "overview",
                            "Overview",
                            column()
                                .child(text("Overview").size(15.0))
                                .child(gap_y(6.0))
                                .child(text("The active panel swaps under the pill.").size(13.0)),
                        ),
                        tabs_tab("activity", "Activity", text("Activity").size(15.0)),
                        tabs_tab("settings", "Settings", text("Settings").size(15.0)),
                    ],
                    |state: &mut TabsState, value| state.active = value,
                )
                .variant(TabsVariant::Pill),
            ),
        )
    }
}

fn tabs_case() -> AnyView<()> {
    any(component(TabsCase))
}

// ---- wallet-card ------------------------------------------------------------

/// Retained state for the interactive `wallet-card` case: the selected
/// account's id, seeded to the recorded one.
struct WalletCardState {
    account: String,
}

/// The `wallet-card` case with a switcher that switches.
///
/// Almost everything this card does is already its own: the switcher panel and
/// the search picker open and close on widget-held `Presence` drivers, the
/// privacy eye masks the balance from widget state, the balance's per-grapheme
/// cascade runs off frame time, and the unread halo pulses on absolute time.
/// The live check bears that out — the recorded poster needed a warm pass to
/// show the balance at all, and a live page paints it settled with nothing
/// from this table.
///
/// One thing is not the widget's: `account_id` is a **controlled override**.
/// The widget refuses to move its own selection while it is set
/// (`if self.config.account_id.is_none()`) and `rebuild` re-forces the index
/// from it every pass, so the recorded `.account_id("main")` pins the card to
/// one account — picking another in the fan reports the change, then snaps
/// straight back. One `String` is the entire conversion.
///
/// The four action buttons and the copy affordance stay unwired: none has a
/// consequence this composition could show, and inventing one would document
/// an affordance the card does not have.
struct WalletCardCase;

impl Component for WalletCardCase {
    type State = WalletCardState;

    fn init(&self) -> Self::State {
        WalletCardState {
            account: "main".to_string(),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let accounts = vec![
            wallet_account(
                "main",
                "Main Wallet",
                "0x8f3Cb1a29e4D7c6F1B2a3E9d0C4b5A6f7D8e9C0b",
            ),
            wallet_account(
                "trading",
                "Trading",
                "0x1a2B3c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9a0B",
            ),
        ];
        framed_in(
            WALLET_CARD,
            wallet_card::<WalletCardState>(accounts, 12_480.25)
                .account_id(state.account.clone())
                .balance_prefix("$")
                .change(2.4)
                .has_notifications(true)
                .on_account_change(|state: &mut WalletCardState, id| state.account = id),
        )
    }
}

fn wallet_card_case() -> AnyView<()> {
    any(component(WalletCardCase))
}
