//! Stateful constructors for `Base`-design cases — see the
//! [module docs](super) for what this table is and why it sits beside the
//! registry rather than inside it.
//!
//! # Seeding, and the one place a live case diverges from its poster
//!
//! Every constructor here seeds [`Component::init`] with the values its
//! recorded twin hardcodes, so the live frame opens on the poster it replaces.
//! Three of these cases also carry a *caption* the static frame could not keep
//! honest — the slider's literal `40%` beside an unrelated `0.4`, the gesture
//! detector's "tap or long-press to fire" over a handler with nowhere to
//! write, and the shield's "button stays tappable" over the same. Those
//! captions become readouts of the state they describe. That divergence is
//! deliberate: a caption asserting an interaction is the one thing a live case
//! must not copy verbatim from a frame that cannot perform it.
//!
//! Nothing here is a `Case::build`, so none of it reaches the snapshot oracle
//! and no poster moves.

use frust_core::{AnyView, Component, View, any, component};
use frust_widgets::motion::{card_switcher, peer_card};
use frust_widgets::{
    Align, Alignment, CrossAxisAlignment, EdgeInsets, GestureDetector, Padding, PageController,
    PlatformViewView, SizedBox, button, checkbox, column, container, page_view, platform_view,
    radio, row, shield, slider, stack, text, text_input,
};
use kurbo::Size;
use peniko::Color;

use super::{Entry, framed};
use crate::case::Case;

/// This catalog's slice of the side table [`super::entries`] concatenates.
///
/// The other `Base` cases are deliberately absent: they are static layout,
/// styling, text and colour primitives that retained state adds nothing to,
/// and `scroll-view`/`list-view`/`physics` already respond live because a
/// scroll offset is widget-owned rather than app-owned. `page-view` is the
/// exception among the scrolling cases, for the reason its entry gives.
pub const INTERACTIVE: &[Entry] = &[
    ("checkbox", checkbox_case),
    ("radio", radio_case),
    ("slider", slider_case),
    ("text-input", text_input_case),
    ("gesture-detector", gesture_detector_case),
    ("shield", shield_case),
    ("page-view", page_view_case),
    ("card-switcher", card_switcher_case),
];

/// The gesture detector's visible target fill. Repeated from
/// `crate::base::interaction`, whose own `ACCENT` is private to it; the two
/// must stay in step, because a live frame composing differently from its
/// poster is precisely the defect this side table exists to avoid.
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// The shield label card's fill, repeated from `crate::base::painting`'s
/// private `CARD` for the reason [`ACCENT`] gives.
const CARD: Color = Color::from_rgb8(0x11, 0x18, 0x27);

// ---- checkbox ---------------------------------------------------------------

/// Retained state for the interactive `checkbox` case: one bool per box,
/// seeded to the same values the recorded case hardcodes so the live page
/// opens on the frame the poster shows.
struct CheckboxState {
    notifications: bool,
    marketing: bool,
}

/// The `checkbox` case with somewhere for its callbacks to write.
///
/// `checkbox` is controlled — it fires `on_toggle(state, !checked)` and never
/// flips its own `checked` — so the toggle only becomes visible if the value
/// handed back down on the next rebuild has changed. A `Component`'s `State`
/// is exactly that: plain retained data the handler mutates through
/// `EventCtx::state_mut`, and `build` re-runs on every rebuild pass, so the
/// mutation reaches the pixels on the next frame with no signal involved.
struct CheckboxCase;

impl Component for CheckboxCase {
    type State = CheckboxState;

    fn init(&self) -> Self::State {
        CheckboxState {
            notifications: true,
            marketing: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            row()
                .child(checkbox(
                    state.notifications,
                    "Notifications",
                    |state: &mut CheckboxState, checked| state.notifications = checked,
                ))
                .child(SizedBox(Some(28.0), None))
                .child(checkbox(
                    state.marketing,
                    "Marketing emails",
                    |state: &mut CheckboxState, checked| state.marketing = checked,
                ))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

/// The [`super::Build`] the table registers: a `()`-stated `AnyView` around
/// the stateful component, legal because `ComponentView<C>` implements
/// `View<Outer>` for every `Outer`. Every constructor below repeats this
/// three-line shape.
// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn checkbox_case() -> AnyView<()> {
    any(component(CheckboxCase))
}

// ---- radio ------------------------------------------------------------------

/// Retained state for the interactive `radio` case: the index of the chosen
/// option, seeded to the one the recorded case pins lit.
struct RadioState {
    selected: usize,
}

/// The `radio` case as an actual group rather than two pinned booleans.
///
/// `radio` reports only *that* it was chosen — `on_select` is
/// `Fn(&mut State)`, carrying no value — and never sets its own `selected`, so
/// mutual exclusion is the app's job. Each button writes its own index and
/// reads `selected` back as a bool, which is why the state is one index rather
/// than a bool per button: two independent bools admit the both-lit and
/// none-lit states a radio group must not have.
struct RadioCase;

impl Component for RadioCase {
    type State = RadioState;

    fn init(&self) -> Self::State {
        RadioState { selected: 0 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            row()
                .child(
                    radio(state.selected == 0, "Option A")
                        .on_select(|state: &mut RadioState| state.selected = 0),
                )
                .child(SizedBox(Some(28.0), None))
                .child(
                    radio(state.selected == 1, "Option B")
                        .on_select(|state: &mut RadioState| state.selected = 1),
                )
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn radio_case() -> AnyView<()> {
    any(component(RadioCase))
}

// ---- slider -----------------------------------------------------------------

/// Retained state for the interactive `slider` case: the track position in
/// `0.0..=1.0`, seeded to the value the recorded case hardcodes.
struct SliderState {
    value: f64,
}

/// The `slider` case, with the percentage beside it reading the same state the
/// track does.
///
/// The recorded case pairs `slider(0.4, ..)` with the literal string `"40%"` —
/// two constants no drag could ever put back in step. Deriving the caption
/// from `state.value` makes it the cheapest available proof that a drag
/// reached the app's state and came back down: a moving thumb could be
/// widget-owned motion, a number that tracks it could not.
///
/// The row keeps the recorded geometry rather than reserving a fixed-width
/// slot for the readout, so the live frame opens on the poster's exact
/// composition; the cost is that the centered row re-centres by a couple of
/// pixels as the number gains or loses a digit.
struct SliderCase;

impl Component for SliderCase {
    type State = SliderState;

    fn init(&self) -> Self::State {
        SliderState { value: 0.4 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        // The seed renders "40%" — the recorded caption's literal, character
        // for character — so the poster-to-live handoff is silent.
        let percent = (state.value * 100.0).round();
        framed(
            row()
                .child(slider(state.value, |state: &mut SliderState, value| {
                    state.value = value;
                }))
                .child(SizedBox(Some(12.0), None))
                .child(text(format!("{percent}%")).size(13.0))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn slider_case() -> AnyView<()> {
    any(component(SliderCase))
}

// ---- text-input -------------------------------------------------------------

/// Retained state for the interactive `text-input` case: one `String` per
/// field, seeded to what the recorded case shows — an empty field behind its
/// placeholder, and the phrase.
struct TextInputState {
    note: String,
    phrase: String,
}

/// The `text-input` case with a value round trip — and the reference shape for
/// every other catalog's single-line field.
///
/// Focus, caret placement, caret blink and IME composition are already
/// widget-owned and need nothing from this state; the *value* was the only
/// missing half. Four rules make that half work, and the third is the one a
/// field with a caret cannot skip:
///
/// 1. **One `String` of state per field.** `on_change` reports the whole
///    requested text, never a delta, so the handler assigns rather than
///    appends.
/// 2. **Hand that state straight back down.** `text_input` is controlled: it
///    adopts the view's `value` on the next rebuild and never its own edit.
///    Without the write-back the field is frozen at its seed, which is exactly
///    the defect this case had.
/// 3. **Store what `on_change` handed you, unmodified.** `rebuild` applies the
///    incoming value *set-if-different* and leaves the editor — and its
///    selection — alone when the text already matches. Normalising in the
///    handler (trimming, case-folding) makes every keystroke differ from the
///    editor's own text, so the editor is reset and the caret collapses to the
///    end of the new value: a caret placed mid-line jumps away as you type. A
///    case that genuinely means to reject or transform input is the documented
///    exception, and it should expect that caret behaviour.
/// 4. **Clone down, move back.** `build` holds `&mut State`, so the value goes
///    down as a clone of the field and returns as an owned `String`; nothing
///    borrows across the callback, and the clone is one field's worth of text
///    per rebuild.
struct TextInputCase;

impl Component for TextInputCase {
    type State = TextInputState;

    fn init(&self) -> Self::State {
        TextInputState {
            note: String::new(),
            phrase: "Frust rocks".to_string(),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            column()
                .child(
                    SizedBox(Some(280.0), None).child(
                        text_input(state.note.clone(), |state: &mut TextInputState, value| {
                            state.note = value;
                        })
                        .placeholder("Write a note..."),
                    ),
                )
                .child(SizedBox(None, Some(16.0)))
                .child(SizedBox(Some(280.0), None).child(text_input(
                    state.phrase.clone(),
                    |state: &mut TextInputState, value| state.phrase = value,
                )))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn text_input_case() -> AnyView<()> {
    any(component(TextInputCase))
}

// ---- gesture-detector -------------------------------------------------------

/// Retained state for the interactive `gesture-detector` case: one counter per
/// recogniser, so the caption can name which of the two fired.
struct GestureState {
    taps: usize,
    long_presses: usize,
}

/// The `gesture-detector` case, with the caption reporting what actually
/// fired.
///
/// The recorded caption promises "tap or long-press to fire" over an `on_tap`
/// that writes to `()`. Counting the two recognisers *separately* is what
/// makes the promise checkable rather than merely kept: one press fires
/// exactly one of `on_tap`/`on_long_press` and never both, so a reader who
/// holds the target watches the second counter move while the first stands
/// still — a shared counter would leave the two gestures indistinguishable.
struct GestureDetectorCase;

impl Component for GestureDetectorCase {
    type State = GestureState;

    fn init(&self) -> Self::State {
        GestureState {
            taps: 0,
            long_presses: 0,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let caption = format!("{} taps, {} long-presses", state.taps, state.long_presses);
        framed(
            column()
                .child(
                    GestureDetector(
                        container(text("Tap me").size(14.0).color(Color::WHITE))
                            .fill(ACCENT)
                            .radius(12.0)
                            .size_centered(140.0, 64.0),
                    )
                    .on_tap(|state: &mut GestureState| state.taps += 1)
                    .on_long_press(|state: &mut GestureState| state.long_presses += 1),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(text(caption).size(12.0))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn gesture_detector_case() -> AnyView<()> {
    any(component(GestureDetectorCase))
}

// ---- shield -----------------------------------------------------------------

/// Retained state for the interactive `shield` case: how many presses reached
/// the shielded button.
struct ShieldState {
    taps: usize,
}

/// Apply [`PlatformViewView::debug_fill`] only in a debug build.
///
/// The builder method is itself `#[cfg(debug_assertions)]`-gated, so naming it
/// unconditionally does not merely no-op in release — it fails to compile the
/// crate, and with it the root workspace. This free function is the cfg split
/// that keeps both profiles building, mirroring the private helper of the same
/// shape in `crate::base::painting` that the recorded case uses.
fn maybe_debug_fill(view: PlatformViewView) -> PlatformViewView {
    #[cfg(debug_assertions)]
    {
        view.debug_fill()
    }
    #[cfg(not(debug_assertions))]
    {
        view
    }
}

/// The `shield` case, with the label counting the presses that got through.
///
/// `shield` exists to keep frust chrome tappable where it paints over an
/// `interactive` platform-view slot, and the recorded label asserts exactly
/// that — "button stays tappable" — above a button whose `on_press` writes to
/// `()`. Turning the assertion into a readout is the whole conversion: the
/// count is the only thing that moves, and the slot, the label card and the
/// button keep the recorded case's geometry, so the reader is looking at the
/// composition the poster showed.
struct ShieldCase;

impl Component for ShieldCase {
    type State = ShieldState;

    fn init(&self) -> Self::State {
        ShieldState { taps: 0 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let taps = state.taps;
        let unit = if taps == 1 { "time" } else { "times" };
        stack()
            .child(maybe_debug_fill(
                platform_view("dev.frust.MapFactory")
                    .interactive()
                    .size(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height),
            ))
            .child(Align(
                Alignment::TOP_LEFT,
                Padding(
                    EdgeInsets::all(16.0),
                    container(
                        text(format!("shield: button tapped {taps} {unit}"))
                            .color(Color::WHITE)
                            .size(13.0),
                    )
                    .fill(CARD)
                    .radius(8.0)
                    .size_centered(230.0, 34.0),
                ),
            ))
            .child(Align(
                Alignment::BOTTOM_RIGHT,
                Padding(
                    EdgeInsets::all(16.0),
                    shield(button("Recenter", |state: &mut ShieldState| {
                        state.taps += 1
                    })),
                ),
            ))
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn shield_case() -> AnyView<()> {
    any(component(ShieldCase))
}

// ---- page-view --------------------------------------------------------------

/// The `page-view` geometry and pages, repeated from `crate::base::scrolling`
/// (private to it) for the reason [`ACCENT`] gives: the viewport box, the
/// viewport fraction, the page it opens on, and each page's label and swatch.
const PAGE_VIEWPORT: (f64, f64) = (320.0, 180.0);
const PAGE_FRACTION: f64 = 0.85;
const PAGE_INITIAL: usize = 1;
const PAGES: [(&str, Color); 4] = [
    ("Page 1", Color::from_rgb8(0xBF, 0xDB, 0xFE)),
    ("Page 2", Color::from_rgb8(0xBB, 0xF7, 0xD0)),
    ("Page 3", Color::from_rgb8(0xFE, 0xF3, 0xC7)),
    ("Page 4", Color::from_rgb8(0xE9, 0xD5, 0xFF)),
];

/// The `page-view` caption ink, repeated from `crate::base::scrolling`'s
/// private `CAPTION`.
const PAGE_INK: Color = Color::from_rgb8(0x33, 0x33, 0x37);

/// One `page-view` page, as the recorded case draws it.
fn page_tile(index: usize) -> impl View<PageViewState> {
    let (label, swatch) = PAGES[index];
    Padding(
        EdgeInsets::symmetric(6.0, 0.0),
        container(Align(
            Alignment::CENTER,
            text(label).size(16.0).color(PAGE_INK),
        ))
        .fill(swatch)
        .radius(12.0),
    )
}

/// Retained state for the interactive `page-view` case: the pager's
/// controller, seeded to the poster's opening page and viewport fraction.
struct PageViewState {
    controller: PageController,
}

/// The `page-view` case with a controller that survives a rebuild.
///
/// Paging itself is widget-owned, so the recorded twin already swipes in a
/// browser — but only until the next rebuild. Its `Case::build` constructs a
/// fresh `PageController` on every pass, and a `page_view` rebuilt with a
/// different controller treats it as a new scroll position and jumps back to
/// that controller's initial page (`PageView::rebuild`'s `same` check), so the
/// page a reader swiped to snaps back to page 2. Holding the one controller in
/// retained state is the whole conversion: the same handle every pass, so the
/// view keeps its offset.
struct PageViewCase;

impl Component for PageViewCase {
    type State = PageViewState;

    fn init(&self) -> Self::State {
        PageViewState {
            controller: PageController::new()
                .initial_page(PAGE_INITIAL)
                .viewport_fraction(PAGE_FRACTION),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            SizedBox(Some(PAGE_VIEWPORT.0), Some(PAGE_VIEWPORT.1)).child(
                page_view((page_tile(0), page_tile(1), page_tile(2), page_tile(3)))
                    .controller(state.controller.clone()),
            ),
        )
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn page_view_case() -> AnyView<()> {
    any(component(PageViewCase))
}

// ---- card-switcher ----------------------------------------------------------

/// The `card-switcher` frame and pages, repeated from
/// `crate::base::animation` (private to it) for the reason [`ACCENT`] gives:
/// `(title, subtitle, swatch)` per page, in strip order.
const SWITCHER_FRAME: Size = Size::new(360.0, 320.0);
const FACE_INK: Color = Color::from_rgb8(0x1F, 0x29, 0x37);
const FACES: [(&str, &str, Color); 3] = [
    ("Inbox", "3 unread", Color::from_rgb8(0xBF, 0xDB, 0xFE)),
    (
        "Calendar",
        "Stand-up at 10:00",
        Color::from_rgb8(0xBB, 0xF7, 0xD0),
    ),
    (
        "Notes",
        "Last edited today",
        Color::from_rgb8(0xFE, 0xF3, 0xC7),
    ),
];

/// Retained state for the interactive `card-switcher` case: whether the stage
/// is open and which page is live (its strip position — the strip keeps
/// [`FACES`] order). Seeded to the poster: open, the middle page live.
struct SwitcherState {
    open: bool,
    live: usize,
}

/// The page peer card `peer` shows when `live` is the live page: the peers
/// are every page but the live one, in strip order.
fn peer_face(live: usize, peer: usize) -> usize {
    if peer >= live { peer + 1 } else { peer }
}

/// One peer page face, as the recorded case draws it.
fn face(index: usize) -> impl View<SwitcherState> {
    let (title, subtitle, swatch) = FACES[index];
    container(Align(
        Alignment::CENTER,
        column()
            .child(text(title).size(22.0).color(FACE_INK))
            .child(SizedBox(None, Some(6.0)))
            .child(text(subtitle).size(14.0).color(FACE_INK))
            .cross_axis(CrossAxisAlignment::Center),
    ))
    .fill(swatch)
}

/// The live page, as the recorded case draws it, with its button wired to
/// open the stage.
fn live_page(index: usize) -> impl View<SwitcherState> {
    let (title, subtitle, swatch) = FACES[index];
    container(Align(
        Alignment::CENTER,
        column()
            .child(text(title).size(22.0).color(FACE_INK))
            .child(SizedBox(None, Some(6.0)))
            .child(text(subtitle).size(14.0).color(FACE_INK))
            .child(SizedBox(None, Some(16.0)))
            .child(button("Switch page", |state: &mut SwitcherState| {
                state.open = true
            }))
            .cross_axis(CrossAxisAlignment::Center),
    ))
    .fill(swatch)
}

/// The `card-switcher` case as a working page switcher.
///
/// The stage never opens, closes or selects by itself — `on_close` and
/// `on_select` only report — so the conversion is two fields and their
/// writers. Closed, the live page fills the frame and its **Switch page**
/// button opens the stage; open, a tap on the centred card or the scrim
/// closes it, and a tap on a peer centres it and makes that page live.
///
/// A selection closes in the same handler rather than through the pattern's
/// `expanding` handshake: its step 4 needs a later write, which a pure-`View`
/// case has no clock to schedule, and closing at once with the chosen page
/// live is the handshake's documented shortcut (step 4 "any time after step
/// 2"). The strip already sits centred on that card, so the close ramp grows
/// it into the page.
struct CardSwitcherCase;

impl Component for CardSwitcherCase {
    type State = SwitcherState;

    fn init(&self) -> Self::State {
        SwitcherState {
            open: true,
            live: 1,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let live = state.live;
        let peers = (0..FACES.len())
            .filter(|&index| index != live)
            .map(|index| peer_card(face(index)));
        super::framed_in(
            SWITCHER_FRAME,
            SizedBox(Some(SWITCHER_FRAME.width), Some(SWITCHER_FRAME.height)).child(
                card_switcher(state.open, live_page(live))
                    .peers(peers)
                    .live_index(live)
                    .on_select(|state: &mut SwitcherState, peer: usize| {
                        state.live = peer_face(state.live, peer);
                        state.open = false;
                    })
                    .on_close(|state: &mut SwitcherState| state.open = false),
            ),
        )
    }
}

// erasure: keep stored as an `interactive::Build` fn pointer (`Entry`)
fn card_switcher_case() -> AnyView<()> {
    any(component(CardSwitcherCase))
}
