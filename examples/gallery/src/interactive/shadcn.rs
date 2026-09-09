//! Stateful constructors for the shadcn design system's cases (`shadcn/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! # Seeding, and the promises this catalog was making
//!
//! Every constructor here seeds [`Component::init`] with the values its
//! recorded twin hardcodes, so the live frame opens on the poster it replaces
//! (the rule [`super::base`] sets out at length). This catalog needed that
//! discipline more than most, because four of its recorded cases carry *prose
//! instructing the reader to do something the frame could not do*:
//!
//! - `drawer`'s description — "Drag the panel toward its edge: past the
//!   halfway point (or with a flick) it keeps going and closes" — over a
//!   panel with `ModalConfig::drag(true)` but no dismiss handler. That is not
//!   a missing gesture: the drag runs, and `ModalWidget::settle_drag` springs
//!   it back *because* nothing was wired ("Dragging a non-dismissable panel",
//!   `plugins/shadcn/src/overlay/modal.rs`). One `on_dismiss` makes the
//!   sentence true.
//! - `sheet`'s footer — "Escape, the scrim, or the X." All three route
//!   through the same `ModalWidget::dismissable()` gate, and all three were
//!   dead for the same one reason.
//! - `table`'s caption — "1 of 2 row(s) selected" beside a `selected(true)`
//!   literal, two constants no click could put back in step. It is derived
//!   here, the way [`super::base`]'s slider caption is.
//! - `tooltip`'s "Hover me" trigger over a latch built fresh on every build
//!   and pinned open. Moving the latch into retained state is the entire fix;
//!   the 700ms delay was never missing, it was being overwritten.
//!
//! # The three cases the recorded catalog calls "stuck"
//!
//! `crate::shadcn`'s module docs describe `dropdown-menu`, `select` and
//! `tooltip` as permanently stuck at zero presence, because neither
//! `PanelStyle::entrance` nor a tooltip ramp override is `pub` outside
//! `frust_shadcn`. That diagnosis is exactly right **for the recorder** and
//! does not carry over here, so this module needs no plugin API change:
//!
//! - The trap is a single-shot paint, not a missing seam. `PanelWidget::ramp`
//!   (`plugins/shadcn/src/components/popover.rs`) and
//!   `TooltipTriggerWidget::paint` both call `ctx.request_frame()` while a
//!   ramp or a hover delay is running, so a *live* host paints the entrance
//!   out over the ~200ms it was written for. Only `record_view`'s one pass
//!   never takes the second frame.
//! - The public open seams already exist for the two click-driven panels:
//!   `DropdownMenuView::open`/`on_open_change` and `SelectView::open`/
//!   `on_open_change` write `PanelStyle::open` for the caller (the
//!   kept-mounted contract `examples/shadcn-demo`'s anchored page documents),
//!   and `TooltipHover` is a `Clone` handle over an `Rc<Cell<_>>` that only
//!   ever needed to outlive one build.
//!
//! All three therefore seed **closed**, which is also the frame the poster
//! shows: a panel recorded at zero presence and a panel that is honestly shut
//! are the same pixels, so the live handoff is silent and the case works
//! afterwards. Nothing here reaches into `frust_shadcn`'s private API, and
//! nothing here forces `Theme.motion.reduce_motion`.
//!
//! # What this module does not do
//!
//! `dialog`, `drawer` and `sheet` are *open/close state* here. Restoring the
//! entrance animation suppressed in their recorded twins
//! (`ModalEntrance::None`, pinned for the same single-paint reason) is a
//! separate task against `crate::shadcn`, and this module does not anticipate
//! it: it calls the sugared `dialog()`/`drawer()`/`sheet()` constructors,
//! whose own `config()` is what those recorded cases hand-roll anyway, and
//! lets the entrance be whatever the constructors ship.
//!
//! Nothing here is a `Case::build`, so none of it reaches the snapshot oracle
//! and no poster moves.

use frust_core::{AnyView, Component, any, component};
use frust_shadcn::overlay::{OverlayAlign, OverlayAnchor, anchor};
use frust_shadcn::{
    ButtonVariant, DrawerSide, QuestionnaireAnswer, QuestionnaireAnswerEvent,
    QuestionnaireShortcuts, SheetSide, SidebarCollapsible, SidebarSide, SidebarVariant,
    TooltipHover, button, checkbox, dialog, dialog_description, dialog_footer, dialog_header,
    dialog_title, drawer, drawer_description, drawer_header, drawer_title, dropdown_menu,
    dropdown_menu_item, dropdown_menu_label, dropdown_menu_separator, field, input,
    label as shadcn_label, questionnaire, questionnaire_choice, questionnaire_item, select,
    select_option, select_trigger, sheet, sheet_description, sheet_footer, sheet_header,
    sheet_title, sidebar, sidebar_content, sidebar_group, sidebar_group_label, sidebar_header,
    sidebar_inset, sidebar_menu, sidebar_menu_button, sidebar_menu_item, sidebar_provider, table,
    table_cell, table_row, tabs, tabs_tab, tooltip, tooltip_trigger,
};
use frust_widgets::{Column, CrossAxisAlignment, Row, SizedBox, Stack, icon, icons, text};
use kurbo::Size;

use super::{Entry, framed, framed_in};

/// This catalog's slice of the side table [`super::entries`] concatenates.
///
/// Twelve of the catalog's fifteen cases. The three absent ones — `button`,
/// `card` and `message` — are a variant row, two static cards and a rendered
/// chat thread: compositions with no app-owned value to retain, whose own
/// affordances (a button's press feedback, the scroller's offset) are already
/// widget-owned and live without any of this.
pub const INTERACTIVE: &[Entry] = &[
    ("shadcn/dialog", dialog_case),
    ("shadcn/drawer", drawer_case),
    ("shadcn/dropdown-menu", dropdown_menu_case),
    ("shadcn/form-controls", form_controls_case),
    ("shadcn/input", input_case),
    ("shadcn/questionnaire", questionnaire_case),
    ("shadcn/select", select_case),
    ("shadcn/sheet", sheet_case),
    ("shadcn/sidebar", sidebar_case),
    ("shadcn/table", table_case),
    ("shadcn/tabs", tabs_case),
    ("shadcn/tooltip", tooltip_case),
];

// ---- frame sizes ------------------------------------------------------------
//
// Repeated from `crate::shadcn`, whose own frame constants are private to that
// module. A live case must be framed at the viewport its poster was recorded
// at or the two compositions diverge on the handoff — the same reason
// `super::base` repeats that module's private colour constants. These are
// declarations of the *same* value, not a second opinion about it; promoting
// one set and deleting the other is a follow-up.

/// A dialog panel frame (`crate::shadcn`'s `DIALOG`).
const DIALOG_FRAME: Size = Size::new(360.0, 260.0);
/// A bottom drawer, tall enough to show its interaction affordances
/// (`crate::shadcn`'s `DRAWER_TALL`).
const DRAWER_TALL: Size = Size::new(360.0, 420.0);
/// A dropdown menu or message thread preview (`crate::shadcn`'s `DROPDOWN`).
const DROPDOWN: Size = Size::new(360.0, 260.0);
/// A questionnaire or form layout needing extra height and width
/// (`crate::shadcn`'s `QUESTIONNAIRE`).
const QUESTIONNAIRE: Size = Size::new(420.0, 320.0);
/// A right-edge sheet that extends vertically (`crate::shadcn`'s
/// `SHEET_TALL`).
const SHEET_TALL: Size = Size::new(360.0, 380.0);
/// A floating sidebar with its main content area (`crate::shadcn`'s
/// `SIDEBAR_WIDE`).
const SIDEBAR_WIDE: Size = Size::new(480.0, 320.0);
/// A data table needing horizontal space for its columns (`crate::shadcn`'s
/// `TABLE_WIDE`).
const TABLE_WIDE: Size = Size::new(420.0, 220.0);
/// The `form-controls` frame — an inline literal in `crate::shadcn`, named
/// here so the number is stated once with the others.
const FORM_FRAME: Size = Size::new(360.0, 220.0);
/// The `input` frame, likewise inline in `crate::shadcn`.
const INPUT_FRAME: Size = Size::new(360.0, 200.0);
/// The `select` frame, likewise inline in `crate::shadcn`.
const SELECT_FRAME: Size = Size::new(360.0, 220.0);

// ---- form-controls ----------------------------------------------------------

/// Retained state for the interactive `form-controls` case: the checkbox and
/// the name field, seeded to what the recorded case hardcodes — accepted, and
/// an empty field behind its placeholder.
struct FormControlsState {
    accepted: bool,
    name: String,
}

/// The `form-controls` case with somewhere for its two controls to write.
///
/// `checkbox` and `input` are both controlled — each reports a requested value
/// and leaves its own untouched until the next rebuild hands one back down —
/// so a `Case::build`'s `|_: &mut (), _| {}` makes both inert. Nothing else
/// about the composition changes.
struct FormControlsCase;

impl Component for FormControlsCase {
    type State = FormControlsState;

    fn init(&self) -> Self::State {
        FormControlsState {
            accepted: true,
            name: String::new(),
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed_in(
            FORM_FRAME,
            Column(vec![
                any(Row(vec![
                    any(checkbox(
                        state.accepted,
                        |state: &mut FormControlsState, checked| state.accepted = checked,
                    )),
                    any(SizedBox(Some(8.0), None)),
                    any(shadcn_label("Accept the terms")),
                ])
                .cross_axis(CrossAxisAlignment::Center)),
                any(SizedBox(None, Some(20.0))),
                any(field(
                    input(
                        state.name.clone(),
                        |state: &mut FormControlsState, value| {
                            state.name = value;
                        },
                    )
                    .placeholder("Full name"),
                )
                .label("Name")
                .description("Shown on your public profile.")),
            ]),
        )
    }
}

fn form_controls_case() -> AnyView<()> {
    any(component(FormControlsCase))
}

// ---- input ------------------------------------------------------------------

/// Retained state for the interactive `input` case: one `String` per field,
/// plus the second field's validity.
///
/// The two strings are seeded empty, which is what the recorded case shows.
/// `required_invalid` is seeded `true` so the opening frame carries the
/// destructive ring, the destructive label and the error line the poster has —
/// and it is a *stored verdict*, not a predicate re-evaluated per rebuild, for
/// the reason [`InputCase`] gives.
struct InputState {
    message: String,
    required: String,
    required_invalid: bool,
}

/// The `input` case with a value round trip, and with its invalid state made
/// reachable instead of pinned.
///
/// The recorded case pins `.invalid(true)` beside a permanent "This field is
/// required." — a verdict nothing could ever change. Here the verdict is
/// state: `on_submit` (Enter) re-checks the field and sets it, so the error
/// can genuinely clear and genuinely come back.
///
/// # Why the check is on submit and not on every keystroke
///
/// Deriving the verdict per keystroke is the obvious design and it is wrong
/// here, measurably so. `FieldView::shape` is the presence triple of the
/// label/description/error slots, and the plugin's own comment on it reads
/// "a change forces a full child rebuild" — so the first character typed into
/// an empty field would drop the error slot, rebuild the field's children, and
/// take the *focused control* down with them. Measured in the browser: with a
/// per-keystroke predicate the first character lands, `document.activeElement`
/// falls from `INPUT#frust-ime-overlay` to `BODY`, and every later keystroke —
/// including Backspace — reaches nothing. The sibling message field on the
/// same page, whose slot shape never moves, keeps focus across characters and
/// deletes normally, which is what isolates the cause to the shape change
/// rather than to the shell.
///
/// Checking on submit keeps the slot shape fixed for the whole of an edit, so
/// typing and deleting behave; the one rebuild happens on Enter, where a
/// re-focus is what a form does anyway. It is also what a real form does:
/// shadcn's own `FieldError` is rendered from a submit result, not from a
/// keystroke.
///
/// # Storing what the editor reported
///
/// The check reads `trim()` but the change handler stores `value`
/// **verbatim** — [`super::base`]'s `TextInputCase` rule 3, which matters more
/// here than the Latin-keyboard case makes it look. `TextInputView::rebuild`
/// adopts the app's value only when it differs from the editor's own text, and
/// `set_controlled_value` then reapplies the whole editing state with the
/// caret collapsed to the end **and `composing: None`**: normalising in the
/// handler would therefore tear down an in-flight IME composition on every
/// keystroke, not merely move a caret.
struct InputCase;

impl Component for InputCase {
    type State = InputState;

    fn init(&self) -> Self::State {
        InputState {
            message: String::new(),
            required: String::new(),
            required_invalid: true,
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let invalid = state.required_invalid;
        let mut required = field(
            input(state.required.clone(), |state: &mut InputState, value| {
                state.required = value;
            })
            .invalid(invalid)
            .placeholder("Required")
            .on_submit(|state: &mut InputState, value: String| {
                state.required_invalid = value.trim().is_empty();
            }),
        )
        .label("Invalid example");
        if invalid {
            required = required.error("This field is required.");
        }
        framed_in(
            INPUT_FRAME,
            Column(vec![
                any(
                    input(state.message.clone(), |state: &mut InputState, value| {
                        state.message = value;
                    })
                    .placeholder("Write a message\u{2026}"),
                ),
                any(SizedBox(None, Some(24.0))),
                any(required),
            ]),
        )
    }
}

fn input_case() -> AnyView<()> {
    any(component(InputCase))
}

// ---- tabs -------------------------------------------------------------------

/// Retained state for the interactive `tabs` case: the active tab's value,
/// seeded to the one the recorded case pins.
struct TabsState {
    active: String,
}

/// The `tabs` case as an actual tab strip.
///
/// `tabs` is controlled on its `value`: pressing a tab reports the value it
/// wants through `on_change` and moves neither the underline nor the panel
/// until that value comes back down. One `String` of state is the whole
/// conversion — the body follows for free, because `TabsWidget` shows the
/// matching `tabs_tab`'s own child.
struct TabsCase;

impl Component for TabsCase {
    type State = TabsState;

    fn init(&self) -> Self::State {
        TabsState {
            active: "account".to_string(),
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(tabs(
            state.active.clone(),
            vec![
                tabs_tab("account", "Account", text("Account settings go here.")),
                tabs_tab("password", "Password", text("Password settings go here.")),
            ],
            |state: &mut TabsState, value| state.active = value,
        ))
    }
}

fn tabs_case() -> AnyView<()> {
    any(component(TabsCase))
}

// ---- table ------------------------------------------------------------------

/// The rows the `table` case shows, repeated from the recorded case so the two
/// carry the same data.
const TABLE_ROWS: [(&str, &str); 2] = [("Alex Kim", "Admin"), ("Sam Lee", "Member")];

/// Retained state for the interactive `table` case: which rows are selected.
///
/// A `Vec<bool>` parallel to [`TABLE_ROWS`] rather than a set of indices: the
/// caption needs a count and each row needs its own flag, and both fall out of
/// the vector without a lookup. Seeded to the recorded case's single selected
/// row, which is what makes its caption's "1 of 2" true on the opening frame.
struct TableState {
    selected: Vec<bool>,
}

/// The `table` case with a caption that counts what is actually selected.
///
/// The recorded caption — "1 of 2 row(s) selected" — is a string beside a
/// `selected(true)` literal: two constants that agree only because someone
/// typed them to agree. Deriving the count is the cheapest available proof
/// that a click reached app state and came back down, exactly as
/// [`super::base`]'s slider percentage is; a row highlight on its own could be
/// widget-owned press feedback, a number that tracks it could not.
struct TableCase;

impl Component for TableCase {
    type State = TableState;

    fn init(&self) -> Self::State {
        TableState {
            selected: vec![true, false],
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let count = state.selected.iter().filter(|selected| **selected).count();
        let total = TABLE_ROWS.len();
        // The seed renders "1 of 2 row(s) selected — page 1 of 1", the recorded
        // caption's literal character for character, so the handoff is silent.
        let caption = format!("{count} of {total} row(s) selected \u{2014} page 1 of 1");
        let rows = TABLE_ROWS
            .iter()
            .enumerate()
            .map(|(index, (name, role))| {
                table_row(vec![any(table_cell(*name)), any(table_cell(*role))])
                    .selected(state.selected.get(index).copied().unwrap_or(false))
                    .on_click(move |state: &mut TableState| {
                        if let Some(selected) = state.selected.get_mut(index) {
                            *selected = !*selected;
                        }
                    })
            })
            .collect::<Vec<_>>();
        framed_in(
            TABLE_WIDE,
            table(rows).header(["Name", "Role"]).caption(caption),
        )
    }
}

fn table_case() -> AnyView<()> {
    any(component(TableCase))
}

// ---- questionnaire ----------------------------------------------------------

/// Retained state for the interactive `questionnaire` case: the item being
/// shown and one answer per item, seeded to the recorded case's `0` and its
/// single default (unanswered) answer.
struct QuestionnaireState {
    current: usize,
    answers: Vec<QuestionnaireAnswer>,
}

/// The `questionnaire` case with its three reporting seams wired — including
/// the keyboard shortcuts it was already advertising.
///
/// `questionnaire` is controlled on both `current` and `answers`, and reports
/// every interaction as a *requested* replacement: a choice press, an arrow
/// key, Skip, and — the point of this conversion — a
/// [`QuestionnaireShortcuts::Letters`] keypress all arrive through the same
/// `on_answer`/`on_navigate` pair. The recorded case sets
/// `.shortcuts(Letters)`, so it renders an A/B/C badge beside every choice,
/// over three no-op callbacks: the badges named keys that recorded nothing.
/// Storing the reported answer wires the pointer path and the keyboard path at
/// once.
///
/// The item list stays the recorded one — a single question — so the live
/// frame opens on the poster. That leaves `on_navigate` with nowhere to go,
/// and it is still wired rather than left a no-op: `current` is clamped to the
/// list so a stray Next cannot point past the end and blank the card.
/// `on_submit` returns the flow to its first item for the same reason — a
/// preview a reader can dead-end is worse than one that loops.
struct QuestionnaireCase;

impl Component for QuestionnaireCase {
    type State = QuestionnaireState;

    fn init(&self) -> Self::State {
        QuestionnaireState {
            current: 0,
            answers: vec![QuestionnaireAnswer::default()],
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed_in(
            QUESTIONNAIRE,
            questionnaire(
                vec![
                    questionnaire_item("framework", "Which framework brought you here?").choices(
                        vec![
                            questionnaire_choice("frust", "Frust"),
                            questionnaire_choice("flutter", "Flutter"),
                            questionnaire_choice("other", "Something else"),
                        ],
                    ),
                ],
                state.current,
                state.answers.clone(),
            )
            .shortcuts(QuestionnaireShortcuts::Letters)
            .on_answer(
                |state: &mut QuestionnaireState, event: QuestionnaireAnswerEvent| {
                    if let Some(answer) = state.answers.get_mut(event.item) {
                        *answer = event.answer;
                    }
                },
            )
            .on_navigate(|state: &mut QuestionnaireState, index: usize| {
                state.current = index.min(state.answers.len().saturating_sub(1));
            })
            .on_submit(|state: &mut QuestionnaireState| state.current = 0),
        )
    }
}

fn questionnaire_case() -> AnyView<()> {
    any(component(QuestionnaireCase))
}

// ---- select -----------------------------------------------------------------

/// The options the `select` case offers, repeated from the recorded case.
const FRUITS: [&str; 3] = ["Apple", "Banana", "Cherry"];

/// Retained state for the interactive `select` case: the shared trigger
/// anchor, the open flag, and the committed option.
///
/// The anchor is *state*, not a local. `OverlayAnchor` is an `Rc<Cell<Rect>>`
/// shared between the trigger that writes the rect and the panel that reads
/// it, and the recorded case builds a fresh one on every call — so each
/// rebuild hands the pair a cell nothing has written yet, and a panel opened
/// against it would place at the window origin. Seeding it once is what makes
/// the pairing survive a rebuild.
struct SelectState {
    anchor: OverlayAnchor,
    open: bool,
    selected: Option<usize>,
}

/// The `select` case as a working select: a trigger that opens the list, and a
/// list whose commit updates the trigger.
///
/// One of the three cases `crate::shadcn` calls "stuck". Its panel is **kept
/// mounted and handed the flag** (`.open(state.open)`) rather than wrapped in
/// an `if`, which is the contract `examples/shadcn-demo`'s anchored page
/// documents: an unmounted view cannot animate, so only a mounted, closed
/// panel can play the exit ramp. A closed panel is inert — it claims no hover,
/// consumes no press, publishes no semantics — and costs one layout.
///
/// Seeding `open: false` is *both* the honest opening state and the poster's:
/// the recorded case leaves the list permanently mounted at its default
/// `open: true` and the recorder catches it at zero presence, so the frame it
/// captured is a trigger with nothing beside it — the same pixels a shut panel
/// paints. `Some(2)` is likewise the recorded selection, pinned there twice
/// (once on the trigger, once on the list); here it is one value read by both.
struct SelectCase;

impl Component for SelectCase {
    type State = SelectState;

    fn init(&self) -> Self::State {
        SelectState {
            anchor: OverlayAnchor::new(),
            open: false,
            selected: Some(2),
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let handle = state.anchor.clone();
        let open = state.open;
        let selected = state.selected;
        let options = || FRUITS.iter().map(|label| select_option(*label)).collect();
        framed_in(
            SELECT_FRAME,
            Stack(vec![
                any(anchor(
                    &handle,
                    select_trigger::<SelectState>(&handle, options(), selected)
                        .placeholder("Pick a fruit")
                        .open(open)
                        .on_open_change(|state: &mut SelectState, open: bool| {
                            state.open = open;
                        }),
                )),
                any(select(
                    options(),
                    selected,
                    |state: &mut SelectState, index: usize| {
                        state.selected = Some(index);
                        state.open = false;
                    },
                )
                .anchor(&handle)
                .align(OverlayAlign::Start)
                .open(open)
                .on_open_change(|state: &mut SelectState, open: bool| {
                    state.open = open;
                })),
            ]),
        )
    }
}

fn select_case() -> AnyView<()> {
    any(component(SelectCase))
}

// ---- dropdown-menu ----------------------------------------------------------

/// Retained state for the interactive `dropdown-menu` case: the shared trigger
/// anchor, the open flag, and the two checkable rows.
///
/// The anchor is state for the reason [`SelectState`] gives — the recorded
/// case rebuilds one per build, so the panel and the trigger stop sharing a
/// rect the moment anything rebuilds.
struct DropdownMenuState {
    anchor: OverlayAnchor,
    open: bool,
    bold: bool,
    italic: bool,
}

/// The `dropdown-menu` case as a working menu: a trigger that toggles it, and
/// checkbox rows that actually toggle.
///
/// The second of the three "stuck" cases, kept mounted and handed the flag for
/// the reason [`SelectCase`] sets out, and seeded shut for the same reason its
/// poster shows only a trigger.
///
/// `Italic` gains the `.checked(..)` its recorded twin lacks — turning a plain
/// row into a `DropdownMenuCheckboxItem` — because a menu whose only checkable
/// row is the one already ticked cannot show a check *appearing*. That
/// divergence is invisible on the handoff: it is inside a panel that is shut
/// on the opening frame, so none of it is in the poster to diverge from.
///
/// The index `on_select` reports is depth-first over the whole item tree,
/// labels and separators included — hence 1 and 2 for the first two selectable
/// rows, which sit behind the "Actions" label at 0.
struct DropdownMenuCase;

impl Component for DropdownMenuCase {
    type State = DropdownMenuState;

    fn init(&self) -> Self::State {
        DropdownMenuState {
            anchor: OverlayAnchor::new(),
            open: false,
            bold: true,
            italic: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let handle = state.anchor.clone();
        let open = state.open;
        framed_in(
            DROPDOWN,
            Stack(vec![
                any(anchor(
                    &handle,
                    button("Actions", |state: &mut DropdownMenuState| {
                        state.open = !state.open;
                    }),
                )),
                any(dropdown_menu(
                    vec![
                        dropdown_menu_label("Actions"),
                        dropdown_menu_item("Bold")
                            .checked(state.bold)
                            .shortcut("\u{2318}B"),
                        dropdown_menu_item("Italic")
                            .checked(state.italic)
                            .shortcut("\u{2318}I"),
                        dropdown_menu_separator(),
                        dropdown_menu_item("Disabled row").disabled(true),
                    ],
                    |state: &mut DropdownMenuState, index: usize| match index {
                        1 => state.bold = !state.bold,
                        2 => state.italic = !state.italic,
                        _ => {}
                    },
                )
                .anchor(&handle)
                .open(open)
                .on_open_change(|state: &mut DropdownMenuState, open: bool| {
                    state.open = open;
                })),
            ]),
        )
    }
}

fn dropdown_menu_case() -> AnyView<()> {
    any(component(DropdownMenuCase))
}

// ---- tooltip ----------------------------------------------------------------

/// Retained state for the interactive `tooltip` case: the hover latch, and
/// nothing else.
///
/// One field, and it is the whole conversion. [`TooltipHover`] is a `Clone`
/// handle over an `Rc<Cell<_>>` holding the phase (idle / opening-since /
/// open / closing-since) and the trigger's rect; the trigger writes it from
/// paint, the layer reads it from layout. Building it inside the case function
/// hands every rebuild a brand-new latch at phase `Idle`, which is why the
/// recorded case has to force it open — a hover would be forgotten before the
/// 700ms it is being timed against had elapsed.
struct TooltipState {
    hover: TooltipHover,
}

/// The `tooltip` case with a latch that survives rebuild — so the delay it
/// advertises is the delay it runs.
///
/// The third "stuck" case, and the only one whose fix is not an open flag.
/// There is nothing to drive from app state here, by design: the open moment
/// is "700ms after the pointer came to rest" and a resting pointer sends no
/// event, so `TooltipTriggerWidget` owns the decision, runs the clock off
/// `PaintCtx::frame_time`, and requests the frames it needs as it goes. That
/// machinery was never missing; it was being destroyed once per build and
/// overridden by `set_open(true)` in between.
///
/// So this constructor drops that `set_open(true)` and keeps the label exactly
/// as recorded — "A tooltip, 700ms after rest" is now a description of what
/// happens rather than a caption on a frozen frame. The opening frame is
/// unchanged: the recorded panel sits at zero presence, and a tooltip nobody
/// has hovered yet paints nothing at all.
///
/// The layer stays mounted permanently, which is its documented contract: it
/// paints nothing while closed and consumes no input ever, so it cannot eat
/// the press on the trigger underneath it.
struct TooltipCase;

impl Component for TooltipCase {
    type State = TooltipState;

    fn init(&self) -> Self::State {
        TooltipState {
            hover: TooltipHover::new(),
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let hover = state.hover.clone();
        framed(Stack(vec![
            any(tooltip_trigger::<TooltipState, _>(
                &hover,
                button("Hover me", |_: &mut TooltipState| {}),
            )),
            any(tooltip::<TooltipState>(
                &hover,
                "A tooltip, 700ms after rest",
            )),
        ]))
    }
}

fn tooltip_case() -> AnyView<()> {
    any(component(TooltipCase))
}

// ---- sidebar ----------------------------------------------------------------

/// The sidebar's menu rows, repeated from the recorded case.
const SIDEBAR_ITEMS: [&str; 3] = ["Inbox", "Sent", "Drafts"];

/// Retained state for the interactive `sidebar` case: whether the panel is
/// expanded, and which row is current — both seeded to the recorded case's
/// pinned values (open, `Inbox`).
struct SidebarState {
    open: bool,
    active: usize,
}

/// The `sidebar` case with its two collapse affordances connected to
/// something.
///
/// The recorded case sets `.collapsible(SidebarCollapsible::Icon)` and
/// `.rail(true)` — an icon-width collapsed state, and a grabbable rail that
/// toggles into it — above a literal `true` and a `|_: &mut (), _: bool| {}`.
/// Both affordances render, neither can fire, and the panel's own width
/// animation never runs. `sidebar_provider` is controlled on exactly that
/// flag, so one bool restores the rail, the collapse ramp and the Ctrl/Cmd+B
/// toggle together.
///
/// The menu's current row is the second half: `sidebar_menu_button` reports a
/// press and never lights itself, so the recorded `index == 0` was the only
/// row that could ever be active.
struct SidebarCase;

impl Component for SidebarCase {
    type State = SidebarState;

    fn init(&self) -> Self::State {
        SidebarState {
            open: true,
            active: 0,
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let active = state.active;
        framed_in(
            SIDEBAR_WIDE,
            sidebar_provider(
                sidebar(sidebar_content(vec![sidebar_group(vec![
                    any(sidebar_group_label("Mail")),
                    sidebar_menu(
                        SIDEBAR_ITEMS
                            .iter()
                            .enumerate()
                            .map(|(index, label)| {
                                any(sidebar_menu_item(vec![any(sidebar_menu_button(
                                    *label,
                                    move |state: &mut SidebarState| {
                                        state.active = index;
                                    },
                                )
                                .icon(icon(icons::FORUM).size(16.0))
                                .active(index == active))]))
                            })
                            .collect(),
                    ),
                ])]))
                .header(sidebar_header(vec![any(text("Floating").size(14.0))]))
                .side(SidebarSide::Right)
                .variant(SidebarVariant::Floating)
                .collapsible(SidebarCollapsible::Icon)
                .rail(true),
                sidebar_inset(vec![any(text("Main content").size(14.0))]),
                state.open,
                |state: &mut SidebarState, open: bool| state.open = open,
            ),
        )
    }
}

fn sidebar_case() -> AnyView<()> {
    any(component(SidebarCase))
}

// ---- the modal family: dialog, drawer, sheet --------------------------------
//
// All three take the same shape, and it is worth stating once. Each is a
// full-area `Stack` holding *either* the panel or the button that reopens it,
// never both:
//
//     Stack(vec![ if open { panel } else { reopen button } ])
//
// The alternative — keeping the trigger mounted underneath — would put it
// behind a `bg-black/50` scrim, which is half transparent: the button would
// show through, dimmed, on the very first frame, and the live case would open
// on a composition its poster does not have. Swapping the two keeps the open
// frame the recorded one.
//
// The cost is the exit ramp: an unmounted view cannot animate out, so the
// panel leaves at once rather than sliding. That is not a choice made here.
// The sugared `dialog()`/`drawer()`/`sheet()` constructors expose no `.open()`
// flag at all (unlike the anchored family's), so a `Stack` mount is
// mount-on-open by construction, and the staged exit belongs to the navigator
// route (`show_dialog` and friends), which a registry case has no controller
// for.
//
// Each wires `on_dismiss` and not `on_close`: `on_close` is the staged hook
// fired from `paint`, so it is a plain `Fn()` with no state to write, while
// `on_dismiss` takes `&mut State` and — with no `on_close` present — fires
// immediately. That one call is also what turns `ModalWidget::dismissable()`
// on, and with it the scrim tap, the close X, Escape, and (for the drawer) a
// drag that continues past the threshold instead of springing back.

/// Retained state for the three modal cases: whether the panel is up.
///
/// One type for all three — the state really is just the flag, and three
/// identical one-field structs would say nothing the name of the case does
/// not already say.
struct ModalState {
    open: bool,
}

impl ModalState {
    /// The reopen affordance shown in the panel's place while it is down.
    fn reopen(label: &'static str) -> AnyView<ModalState> {
        any(button(label, |state: &mut ModalState| state.open = true))
    }
}

/// The `dialog` case with a dismiss that dismisses.
///
/// The panel is `dialog()`'s own composition — the recorded case hand-rolls
/// the identical `ModalConfig::centered(MAX_WIDTH_LG).close_button(true)` only
/// so it can append `.entrance(ModalEntrance::None)`, which is a recorder
/// workaround rather than part of the component. Calling the sugar keeps this
/// case honest about what an app writes.
///
/// The recorded footer's lone "Cancel" now cancels, as do the close X, the
/// scrim and Escape.
struct DialogCase;

impl Component for DialogCase {
    type State = ModalState;

    fn init(&self) -> Self::State {
        ModalState { open: true }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let panel = if state.open {
            any(dialog(vec![
                dialog_header(vec![
                    dialog_title("Delete project?"),
                    dialog_description("This action cannot be undone."),
                ]),
                dialog_footer(vec![any(button("Cancel", |state: &mut ModalState| {
                    state.open = false
                })
                .variant(ButtonVariant::Outline))]),
            ])
            .label("Delete project?")
            .on_dismiss(|state: &mut ModalState| state.open = false))
        } else {
            ModalState::reopen("Open dialog")
        };
        framed_in(DIALOG_FRAME, Stack(vec![panel]))
    }
}

fn dialog_case() -> AnyView<()> {
    any(component(DialogCase))
}

/// The `drawer` case, with the drag its description promises.
///
/// The recorded description tells the reader to "drag the panel toward its
/// edge: past the halfway point (or with a flick) it keeps going and closes;
/// short of it, it springs back open". Half of that was already true —
/// `drawer()`'s own config sets `ModalConfig::drag(true)`, so the panel tracks
/// the pointer and a short drag does spring back. The other half was not, and
/// for a documented reason: `ModalWidget::settle_drag` continues into the exit
/// ramp only for a *dismissable* panel, and springs a non-dismissable one back
/// however far it was dragged ("Dragging a non-dismissable panel",
/// `plugins/shadcn/src/overlay/modal.rs`). The recorded case wires no dismiss
/// handler, so every drag sprang back and the sentence was false in exactly
/// the half a reader would test. `on_dismiss` is the whole repair; the
/// description is left word for word.
struct DrawerCase;

impl Component for DrawerCase {
    type State = ModalState;

    fn init(&self) -> Self::State {
        ModalState { open: true }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let panel = if state.open {
            any(drawer(
                DrawerSide::Bottom,
                vec![drawer_header(vec![
                    drawer_title("Bottom drawer"),
                    drawer_description(
                        "Drag the panel toward its edge: past the halfway point (or with a \
                             flick) it keeps going and closes; short of it, it springs back open.",
                    ),
                ])],
            )
            .label("Bottom drawer")
            .on_dismiss(|state: &mut ModalState| state.open = false))
        } else {
            ModalState::reopen("Open drawer")
        };
        framed_in(DRAWER_TALL, Stack(vec![panel]))
    }
}

fn drawer_case() -> AnyView<()> {
    any(component(DrawerCase))
}

/// The `sheet` case, with the three dismissals its footer names.
///
/// "Escape, the scrim, or the X." is a list of three gestures behind one gate:
/// `ModalWidget` runs the Escape branch and arms the scrim only when
/// `dismissable()` holds, and `dismissable()` is precisely "an `on_dismiss` or
/// an `on_close` is wired". The recorded case wires neither, so its footer
/// named three dead affordances rather than three separate bugs. The footer
/// text is left word for word.
struct SheetCase;

impl Component for SheetCase {
    type State = ModalState;

    fn init(&self) -> Self::State {
        ModalState { open: true }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        let panel = if state.open {
            any(sheet(vec![
                sheet_header(vec![
                    sheet_title("Right sheet"),
                    sheet_description(
                        "Dismiss it and watch the panel slide back out the edge it came \
                             from before the page pops.",
                    ),
                ]),
                sheet_footer(vec![any(text("Escape, the scrim, or the X."))]),
            ])
            .side(SheetSide::Right)
            .label("Right sheet")
            .on_dismiss(|state: &mut ModalState| state.open = false))
        } else {
            ModalState::reopen("Open sheet")
        };
        framed_in(SHEET_TALL, Stack(vec![panel]))
    }
}

fn sheet_case() -> AnyView<()> {
    any(component(SheetCase))
}
