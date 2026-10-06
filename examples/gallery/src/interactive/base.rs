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
use frust_widgets::{
    Align, Alignment, Axis, CrossAxisAlignment, EdgeInsets, FlexView, GestureDetector, Padding,
    PlatformViewView, SizedBox, Stack, button, checkbox, container, inflexible, platform_view,
    radio, shield, slider, text, text_input,
};
use peniko::Color;

use super::{Entry, framed};
use crate::case::Case;

/// This catalog's slice of the side table [`super::entries`] concatenates.
///
/// The other 29 `Base` cases are deliberately absent: they are static layout,
/// styling, text and colour primitives that retained state adds nothing to,
/// and `scroll-view`/`list-view`/`physics` already respond live because a
/// scroll offset is widget-owned rather than app-owned.
pub const INTERACTIVE: &[Entry] = &[
    ("checkbox", checkbox_case),
    ("radio", radio_case),
    ("slider", slider_case),
    ("text-input", text_input_case),
    ("gesture-detector", gesture_detector_case),
    ("shield", shield_case),
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
            FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(checkbox(
                        state.notifications,
                        "Notifications",
                        |state: &mut CheckboxState, checked| state.notifications = checked,
                    )),
                    inflexible(SizedBox(Some(28.0), None)),
                    inflexible(checkbox(
                        state.marketing,
                        "Marketing emails",
                        |state: &mut CheckboxState, checked| state.marketing = checked,
                    )),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

/// The [`super::Build`] the table registers: a `()`-stated `AnyView` around
/// the stateful component, legal because `ComponentView<C>` implements
/// `View<Outer>` for every `Outer`. Every constructor below repeats this
/// three-line shape.
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
            FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(
                        radio(state.selected == 0, "Option A")
                            .on_select(|state: &mut RadioState| state.selected = 0),
                    ),
                    inflexible(SizedBox(Some(28.0), None)),
                    inflexible(
                        radio(state.selected == 1, "Option B")
                            .on_select(|state: &mut RadioState| state.selected = 1),
                    ),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

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
            FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(slider(state.value, |state: &mut SliderState, value| {
                        state.value = value;
                    })),
                    inflexible(SizedBox(Some(12.0), None)),
                    inflexible(text(format!("{percent}%")).size(13.0)),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

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
            FlexView::new(
                Axis::Vertical,
                vec![
                    inflexible(
                        SizedBox(Some(280.0), None).child(
                            text_input(state.note.clone(), |state: &mut TextInputState, value| {
                                state.note = value;
                            })
                            .placeholder("Write a note..."),
                        ),
                    ),
                    inflexible(SizedBox(None, Some(16.0))),
                    inflexible(SizedBox(Some(280.0), None).child(text_input(
                        state.phrase.clone(),
                        |state: &mut TextInputState, value| state.phrase = value,
                    ))),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

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
            FlexView::new(
                Axis::Vertical,
                vec![
                    inflexible(
                        GestureDetector(
                            container(text("Tap me").size(14.0).color(Color::WHITE))
                                .fill(ACCENT)
                                .radius(12.0)
                                .size_centered(140.0, 64.0),
                        )
                        .on_tap(|state: &mut GestureState| state.taps += 1)
                        .on_long_press(|state: &mut GestureState| state.long_presses += 1),
                    ),
                    inflexible(SizedBox(None, Some(12.0))),
                    inflexible(text(caption).size(12.0)),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

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
        any(Stack(vec![
            any(maybe_debug_fill(
                platform_view("dev.frust.MapFactory")
                    .interactive()
                    .size(Case::DEFAULT_SIZE.width, Case::DEFAULT_SIZE.height),
            )),
            any(Align(
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
            )),
            any(Align(
                Alignment::BOTTOM_RIGHT,
                Padding(
                    EdgeInsets::all(16.0),
                    shield(button("Recenter", |state: &mut ShieldState| {
                        state.taps += 1
                    })),
                ),
            )),
        ]))
    }
}

fn shield_case() -> AnyView<()> {
    any(component(ShieldCase))
}
