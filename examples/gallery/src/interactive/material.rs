//! Stateful constructors for the Material design system's cases (`material/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! # Why this catalog gains the most from retained state
//!
//! Material 3 Expressive spends most of its motion budget on *state changes*:
//! the navigation bar's two-spring indicator, the switch's independent track
//! and handle springs, the tab indicator's slide, the dropdown arrow's half
//! turn, the icon button's shape morph. A recorded case hands every callback
//! a `|_: &mut (), _| {}` and pins every value to a literal, so none of that
//! motion can play — not because the animation is missing but because the
//! value it interpolates between never moves. Each constructor below is the
//! same composition with somewhere for its callbacks to write; the motion is
//! the plugin's own and needs nothing from this file.
//!
//! # Seeding, and the three places a live case diverges from its poster
//!
//! Every constructor seeds [`frust_core::Component::init`] with the values its
//! recorded twin hardcodes, so the live frame opens on the poster it replaces.
//! Three cases cannot keep that promise *and* stay honest, and each diverges
//! in one named way:
//!
//! - `dropdown` opens with its arrow pointing **up**. The recorded frame draws
//!   a permanently-open panel under a field whose arrow points down — two
//!   halves of one control disagreeing about whether it is open. The live case
//!   holds one `open` flag that both halves read, and it is seeded `true`
//!   because the panel is the larger half of the composition.
//! - `menu` gains a readout line along the bottom edge naming the row that
//!   last fired. It renders **empty** until the first activation, so the
//!   opening frame is still the poster's, and it is bottom-aligned inside a
//!   frame-sized box rather than stacked under the panel so that adding it
//!   moves the panel by nothing.
//! - `snackbar`'s headline answers its own `Undo`. Everything else about that
//!   case is unchanged, including the bar being off-screen on frame one.
//!
//! # What is deliberately still static here
//!
//! - `dialog` and `sheet` pin `OverlayEntrance::None` to satisfy the one-frame
//!   recorder, which also disables the entrance a live host could play.
//!   Restoring it is an overlay-host change rather than a state change, and is
//!   not attempted here.
//! - `navigation-rail`'s expand/collapse toggle. The rail's expanded width is
//!   clamped to a 220dp minimum (`frust_material::RAIL_EXPANDED_MIN_WIDTH`)
//!   and this case is recorded in a 200dp-wide frame, so an expanded rail
//!   cannot be shown without widening the frame the poster fixed. Its
//!   destination selection is live; the menu button stays off.
//! - `navigation-drawer`'s open/closed pair. The case records
//!   `navigation_drawer_content`, the in-layout surface, which has no open
//!   state of its own — that belongs to `navigation_drawer`'s overlay host and
//!   its entrance ramp. Its destination selection is live.
//! - `app-bar`, `button`, `card`, `fab` and `toolbar` own no app state at all;
//!   their press feedback is widget-owned and already live.
//!
//! Nothing here is a `Case::build`, so none of it reaches the snapshot oracle
//! and no poster moves.

use frust_core::{AnyView, Component, View, any, component};
use frust_material::{
    ButtonGroupType, DatePickerState, DropdownItem, IconButtonVariant, MaterialDate, MenuSelection,
    NavBarLabelBehavior, NavigationRailType, SnackbarController, Tab, TabsVariant,
    TextFieldVariant, TimeOfDay, assist_chip, button_group_action, button_group_actions,
    calendar_date_picker, centered_slider, checkbox, drawer_destination, drawer_section,
    dropdown_field, dropdown_item, dropdown_panel, filled_button, filter_chip, icon_button, icons,
    input_chip, list_item, menu_entry, menu_group, menu_panel, nav_item, navigation_bar,
    navigation_drawer_content, navigation_rail, radio, rail_destination, rail_fab, rail_section,
    segment, segmented_button, slider, snackbar, snackbar_host, suggestion_chip, switch, tab, tabs,
    text_field, time_dial, wavy_slider,
};
use frust_widgets::{
    Align, Alignment, Column, CrossAxisAlignment, EdgeInsets, IconSource, Padding, Row, SizedBox,
    column, container, icon, row, stack, text,
};
use kurbo::Size;

use super::{Entry, framed, framed_in};

/// This catalog's slice of the side table [`super::entries`] concatenates, in
/// the registry's own slug order so the two read side by side.
///
/// The seven absent Material cases are the two overlay-entrance ones and the
/// five that own no app state — see the [module docs](self).
pub const INTERACTIVE: &[Entry] = &[
    ("material/button-group", button_group_case),
    ("material/chips", chips_case),
    ("material/date-picker", date_picker_case),
    ("material/dropdown", dropdown_case),
    ("material/icon-button", icon_button_case),
    ("material/list", list_case),
    ("material/menu", menu_case),
    ("material/navigation-bar", navigation_bar_case),
    ("material/navigation-drawer", navigation_drawer_case),
    ("material/navigation-rail", navigation_rail_case),
    ("material/selection-controls", selection_controls_case),
    ("material/slider", slider_case),
    ("material/snackbar", snackbar_case),
    ("material/tabs", tabs_case),
    ("material/text-field", text_field_case),
    ("material/time-picker", time_picker_case),
];

// ---- frame sizes and layout helpers -----------------------------------------
//
// Every constant and helper below is repeated from `crate::material`, whose own
// copies are private to that module. The two must stay in step: a live case
// framed differently from its poster is precisely the defect this side table
// exists to avoid.

/// A single full-width bar, or one short row of components.
const BAR: Size = Size::new(400.0, 180.0);
/// Two stacked full-width bars, or a short list of them.
const STRIP: Size = Size::new(400.0, 280.0);
/// A panel or stacked-field column taller than
/// [`Case::DEFAULT_SIZE`](crate::case::Case::DEFAULT_SIZE).
const TALL: Size = Size::new(400.0, 340.0);
/// A full navigation/overlay surface that wants most of a phone viewport.
const SURFACE: Size = Size::new(400.0, 420.0);
/// The pickers' frame — both picker widths are 328pt, and the calendar's six
/// day rows plus its header need the height.
const PICKER: Size = Size::new(392.0, 400.0);
/// The navigation rail's frame: narrow, tall, and boxed to [`RAIL_W`].
const RAIL: Size = Size::new(200.0, 480.0);
/// The logical width [`NavigationRailCase`] boxes its rail to —
/// `frust_material::RAIL_COLLAPSED_WIDTH`.
const RAIL_W: f64 = 96.0;
/// The logical width every full-bleed component in this module is boxed to.
/// `framed`'s container hands its child loose constraints, so a bar, field or
/// list would otherwise shrink-wrap to its content.
const BLEED_W: f64 = 344.0;

/// `icon(..)` as an `AnyView`, at whatever state the surrounding case is bound
/// to — the state-generic mirror of `crate::material`'s own `()`-bound helper.
fn glyph<State: 'static>(source: IconSource) -> impl View<State> {
    icon(source)
}

/// A fixed-width box around a component that would otherwise shrink-wrap; see
/// [`BLEED_W`].
fn bleed<State: 'static, V: View<State>>(child: V) -> impl View<State> {
    SizedBox(Some(BLEED_W), None).child(child)
}

/// A vertical gap between stacked rows in a case.
fn gap<State: 'static>(height: f64) -> impl View<State> {
    SizedBox(None, Some(height))
}

/// A horizontal gap between side-by-side controls in a case.
fn hgap<State: 'static>(width: f64) -> impl View<State> {
    SizedBox(Some(width), None)
}

// ---- button-group -----------------------------------------------------------

/// Retained state for the interactive `button-group` case: the connected
/// group's chosen member and the segmented button's selected key set, each
/// seeded to what the recorded case pins.
struct ButtonGroupState {
    align: Option<usize>,
    span: Vec<&'static str>,
}

/// The `button-group` case with both of its selections app-owned.
///
/// The press squish and its neighbour spill already run live — they are
/// widget-owned interaction state. What the recorded case cannot do is *move*
/// the selection: `selected_index`/`selected` are props the group reads and
/// never writes, so the lit member stays wherever the literal put it. Holding
/// the index here is what lets the selection treatment travel between members.
struct ButtonGroupCase;

impl Component for ButtonGroupCase {
    type State = ButtonGroupState;

    fn init(&self) -> Self::State {
        ButtonGroupState {
            align: Some(1),
            span: vec!["week"],
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            STRIP,
            column()
                .child(
                    button_group_actions(vec![
                        button_group_action("Left").icon(|| any(glyph(icons::FORMAT_ALIGN_LEFT))),
                        button_group_action("Center")
                            .icon(|| any(glyph(icons::FORMAT_ALIGN_CENTER))),
                        button_group_action("Right").icon(|| any(glyph(icons::FORMAT_ALIGN_RIGHT))),
                    ])
                    .group_type(ButtonGroupType::Connected)
                    .selected_index(state.align)
                    .on_select(|state: &mut ButtonGroupState, index| state.align = Some(index)),
                )
                .child(gap(24.0))
                .child(segmented_button(
                    vec![
                        segment("day").label("Day"),
                        segment("week").label("Week"),
                        segment("month").label("Month"),
                    ],
                    state.span.iter().copied(),
                    |state: &mut ButtonGroupState, span: Vec<&'static str>| state.span = span,
                ))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

/// The [`super::Build`] the table registers: a `()`-stated `AnyView` around the
/// stateful component, legal because `ComponentView<C>` implements
/// `View<Outer>` for every `Outer`. Every constructor below repeats this
/// three-line shape.
fn button_group_case() -> AnyView<()> {
    any(component(ButtonGroupCase))
}

// ---- chips ------------------------------------------------------------------

/// Retained state for the interactive `chips` case: the filter chip's own bool
/// and the input chips still present, seeded to the recorded row.
struct ChipsState {
    unread: bool,
    contacts: Vec<&'static str>,
}

/// The `chips` case with a filter that filters and a delete that deletes.
///
/// `input_chip`'s `on_deleted` is the one callback in this catalog whose
/// recorded no-op actively misleads: the chip draws a close affordance, the
/// reader presses it, and the chip stays. Backing the row with a list makes the
/// delete real. Removal is one-way — nothing in this composition claims to add
/// a contact back, and inventing an affordance that did would document a
/// control the page does not have.
struct ChipsCase;

impl Component for ChipsCase {
    type State = ChipsState;

    fn init(&self) -> Self::State {
        ChipsState {
            unread: true,
            contacts: vec!["Ada Lovelace"],
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        // The seeded single contact renders chip + gap + suggestion — the
        // recorded row, member for member.
        let mut second_row: Vec<AnyView<ChipsState>> = Vec::new();
        for name in &state.contacts {
            let name = *name;
            second_row.push(any(input_chip(name, |_: &mut ChipsState| {}).on_deleted(
                move |state: &mut ChipsState| state.contacts.retain(|held| *held != name),
            )));
            second_row.push(any(hgap(10.0)));
        }
        second_row.push(any(
            suggestion_chip("Reply", |_: &mut ChipsState| {}).elevated(true)
        ));
        framed(
            column()
                .child(
                    row()
                        .child(assist_chip("Add to calendar", |_: &mut ChipsState| {}))
                        .child(hgap(10.0))
                        .child(filter_chip(
                            "Unread",
                            state.unread,
                            |state: &mut ChipsState, selected| state.unread = selected,
                        )),
                )
                .child(gap(16.0))
                .child(Row(second_row))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn chips_case() -> AnyView<()> {
    any(component(ChipsCase))
}

// ---- date-picker ------------------------------------------------------------

/// The date the recorded case calls today. Read by both `init` and `build`
/// through this one function so the seeded selection and the `today` marker
/// cannot drift apart.
fn picker_today() -> MaterialDate {
    MaterialDate::new(2026, 9, 7)
}

/// Retained state for the interactive `date-picker` case: the whole
/// `DatePickerState` the calendar reports back, seeded to the recorded
/// selection.
struct DatePickerCaseState {
    picker: DatePickerState,
}

/// The `date-picker` case with its calendar state app-owned.
///
/// `calendar_date_picker` is controlled over a whole struct rather than a
/// single value: every day press, month step and sub-view switch arrives as a
/// complete next `DatePickerState`, and the widget changes nothing of its own.
/// Assigning it is therefore the entire conversion, and it unlocks three
/// interactions at once — picking a day, paging the month, and the year grid.
struct DatePickerCase;

impl Component for DatePickerCase {
    type State = DatePickerCaseState;

    fn init(&self) -> Self::State {
        DatePickerCaseState {
            picker: DatePickerState::new(Some(MaterialDate::new(2026, 9, 17)), picker_today()),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            PICKER,
            calendar_date_picker(
                state.picker.clone(),
                MaterialDate::new(2026, 1, 1),
                MaterialDate::new(2026, 12, 31),
                |state: &mut DatePickerCaseState, next| state.picker = next,
            )
            .today(picker_today()),
        )
    }
}

fn date_picker_case() -> AnyView<()> {
    any(component(DatePickerCase))
}

// ---- dropdown ---------------------------------------------------------------

/// The option list both dropdown surfaces read.
fn dropdown_items() -> Vec<DropdownItem> {
    vec![
        dropdown_item("Apple", "apple"),
        dropdown_item("Blackberry", "blackberry"),
        dropdown_item("Cherry", "cherry"),
        dropdown_item("Damson", "damson"),
    ]
}

/// Retained state for the interactive `dropdown` case: the chosen values, and
/// whether the panel is open.
struct DropdownState {
    selected: Vec<String>,
    open: bool,
}

/// The `dropdown` case as one control rather than two pinned surfaces.
///
/// The recorded case draws the field and the panel stacked, each handed
/// `"cherry"` separately and each with its own opinion about open-ness: the
/// panel is open (its default) under a field whose arrow points down. One
/// `open` flag read by both is what makes them one control — pressing the field
/// closes the panel and turns the arrow through its half circle, and a
/// single-select pick reports `on_open(false)` itself, so choosing a fruit
/// closes the panel exactly as it would in an app.
///
/// The panel is *unmounted* when closed rather than merely told so: a closed
/// panel still paints (its own `open` flag gates keyboard navigation, not
/// visibility), and a visible list that ignores presses is worse than no list.
/// The composition therefore reflows around the field when it closes, which is
/// the honest consequence of documenting an anchored overlay in-layout.
struct DropdownCase;

impl Component for DropdownCase {
    type State = DropdownState;

    fn init(&self) -> Self::State {
        DropdownState {
            selected: vec!["cherry".to_string()],
            open: true,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let mut column: Vec<AnyView<DropdownState>> = vec![any(bleed(
            dropdown_field(dropdown_items())
                .selected(state.selected.clone())
                .hint("Pick a fruit")
                .show_clear(true)
                .open(state.open)
                .on_open(|state: &mut DropdownState, open| state.open = open)
                .on_change(|state: &mut DropdownState, selected| state.selected = selected),
        ))];
        if state.open {
            column.push(any(gap(12.0)));
            column.push(any(dropdown_panel(
                dropdown_items(),
                |state: &mut DropdownState, selected| state.selected = selected,
            )
            .selected(state.selected.clone())
            .max_height(220.0)
            .on_open(|state: &mut DropdownState, open| state.open = open)));
        }
        framed_in(
            SURFACE,
            Column(column).cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn dropdown_case() -> AnyView<()> {
    any(component(DropdownCase))
}

// ---- icon-button ------------------------------------------------------------

/// Retained state for the interactive `icon-button` case: one bool per button,
/// seeded so only the filled one opens lit — the recorded row exactly.
struct IconButtonState {
    favorite: bool,
    filled_favorite: bool,
    bookmark: bool,
    shared: bool,
}

/// The `icon-button` case with every variant toggling.
///
/// `is_selected` is a caller-owned prop the button never writes, so the
/// recorded row's one `selected(true)` is as far as its selection ever gets.
/// All four take a bool here rather than only the pinned one: the case exists
/// to show the four variants side by side, and one live button beside three
/// dead ones answers the page's own question by half. Seeding the other three
/// `false` costs no pixels — the resting shape flips only while a toggle is
/// *selected*, and `Standard` is the one variant whose ink reads the flag at
/// all — so the live frame still opens on the poster.
struct IconButtonCase;

impl Component for IconButtonCase {
    type State = IconButtonState;

    fn init(&self) -> Self::State {
        IconButtonState {
            favorite: false,
            filled_favorite: true,
            bookmark: false,
            shared: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            row()
                .child(
                    icon_button(glyph(icons::FAVORITE), |state: &mut IconButtonState| {
                        state.favorite = !state.favorite
                    })
                    .selected(state.favorite)
                    .semantic_label("Favorite"),
                )
                .child(hgap(12.0))
                .child(
                    icon_button(glyph(icons::FAVORITE), |state: &mut IconButtonState| {
                        state.filled_favorite = !state.filled_favorite
                    })
                    .variant(IconButtonVariant::Filled)
                    .selected(state.filled_favorite),
                )
                .child(hgap(12.0))
                .child(
                    icon_button(glyph(icons::BOOKMARK), |state: &mut IconButtonState| {
                        state.bookmark = !state.bookmark
                    })
                    .variant(IconButtonVariant::Tonal)
                    .selected(state.bookmark),
                )
                .child(hgap(12.0))
                .child(
                    icon_button(glyph(icons::SHARE), |state: &mut IconButtonState| {
                        state.shared = !state.shared
                    })
                    .variant(IconButtonVariant::Outlined)
                    .selected(state.shared),
                )
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn icon_button_case() -> AnyView<()> {
    any(component(IconButtonCase))
}

// ---- list -------------------------------------------------------------------

/// Retained state for the interactive `list` case: the index of the selected
/// row, seeded to the one the recorded case lights.
struct ListState {
    selected: usize,
}

/// The `list` case as a single-selection list.
///
/// One index rather than a bool per row, for the reason the baseline catalog's
/// radio group gives: independent bools admit the none-selected and
/// all-selected states a single-selection list must not have. Each row's
/// `on_press` writes its own index and reads `selected` back as a bool, so the
/// selected container travels with the press.
struct ListCase;

impl Component for ListCase {
    type State = ListState;

    fn init(&self) -> Self::State {
        ListState { selected: 2 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            STRIP,
            column()
                .child(bleed(
                    list_item("Weekly sync")
                        .supporting("Tomorrow, 09:00")
                        .leading(icon(icons::SCHEDULE))
                        .trailing(icon(icons::CHEVRON_RIGHT))
                        .selected(state.selected == 0)
                        .on_press(|state: &mut ListState| state.selected = 0),
                ))
                .child(bleed(
                    list_item("Design review")
                        .supporting("Thursday, 14:30")
                        .leading(icon(icons::LABEL))
                        .trailing(icon(icons::CHEVRON_RIGHT))
                        .selected(state.selected == 1)
                        .on_press(|state: &mut ListState| state.selected = 1),
                ))
                .child(bleed(
                    list_item("Release notes")
                        .supporting("Draft shared with the team")
                        .leading(icon(icons::ARCHIVE))
                        .selected(state.selected == 2)
                        .on_press(|state: &mut ListState| state.selected = 2),
                ))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn list_case() -> AnyView<()> {
    any(component(ListCase))
}

// ---- menu -------------------------------------------------------------------

/// Retained state for the interactive `menu` case: the label of the row that
/// last fired, or `None` before any has.
struct MenuState {
    last: Option<String>,
}

/// The `menu` case with its activations reaching somewhere.
///
/// Every row in this panel is a plain entry, so what one reports is a press —
/// there is no selected value for the panel's own `selected` prop to reflect,
/// and hover and press feedback are already widget-owned. A readout is
/// therefore the only way an activation can show a consequence, and it is kept
/// honest two ways: it renders empty until the first press, so the opening
/// frame is the recorded one; and it is bottom-aligned inside a frame-sized box
/// rather than stacked beneath the panel, so the panel stays exactly where the
/// poster centres it.
struct MenuCase;

impl Component for MenuCase {
    type State = MenuState;

    fn init(&self) -> Self::State {
        MenuState { last: None }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let readout = match &state.last {
            Some(label) => format!("chose {label}"),
            None => String::new(),
        };
        framed_in(
            TALL,
            SizedBox(Some(TALL.width), Some(TALL.height)).child(
                stack()
                    .child(Align(
                        Alignment::CENTER,
                        menu_panel(
                            vec![
                                menu_entry("Archive").leading(icons::ARCHIVE).into(),
                                menu_entry("Copy")
                                    .leading(icons::CONTENT_COPY)
                                    .shortcut("Ctrl+C")
                                    .into(),
                                menu_group(vec![
                                    menu_entry("Share").leading(icons::SHARE).into(),
                                    menu_entry("Delete")
                                        .leading(icons::DELETE)
                                        .destructive(true)
                                        .into(),
                                ])
                                .into(),
                            ],
                            |state: &mut MenuState, selection: MenuSelection| {
                                state.last = Some(selection.label);
                            },
                        ),
                    ))
                    .child(Align(
                        Alignment::new(0.0, 1.0),
                        Padding(EdgeInsets::all(8.0), text(readout).size(12.0)),
                    )),
            ),
        )
    }
}

fn menu_case() -> AnyView<()> {
    any(component(MenuCase))
}

// ---- navigation-bar ---------------------------------------------------------

/// Retained state for the interactive `navigation-bar` case: the selected
/// destination, seeded to the recorded one.
struct NavigationBarState {
    selected: usize,
}

/// The `navigation-bar` case, and the clearest reason this batch exists.
///
/// The bar's indicator is a liquid two-spring travel between destinations: a
/// lead spring and a trail spring drive the pill's two edges independently, so
/// it stretches as it moves and settles back to its rest width. None of that is
/// reachable from a recorded frame, because the springs interpolate *between*
/// selected indices and the index is a literal. One `usize` of app state is the
/// whole difference.
struct NavigationBarCase;

impl Component for NavigationBarCase {
    type State = NavigationBarState;

    fn init(&self) -> Self::State {
        NavigationBarState { selected: 0 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            BAR,
            bleed(
                navigation_bar(
                    vec![
                        nav_item("Home").icon(glyph(icons::HOME)),
                        nav_item("Search").icon(glyph(icons::SEARCH)),
                        nav_item("Mail").icon(glyph(icons::MAIL)).badge_count(3),
                        nav_item("Profile").icon(glyph(icons::ACCOUNT_CIRCLE)),
                    ],
                    state.selected,
                    |state: &mut NavigationBarState, index| state.selected = index,
                )
                .label_behavior(NavBarLabelBehavior::AlwaysShow)
                .safe_area(false),
            ),
        )
    }
}

fn navigation_bar_case() -> AnyView<()> {
    any(component(NavigationBarCase))
}

// ---- navigation-drawer ------------------------------------------------------

/// Retained state for the interactive `navigation-drawer` case: the selected
/// destination across both sections, seeded to the recorded one.
struct NavigationDrawerState {
    selected: usize,
}

/// The `navigation-drawer` case with its destination selection live.
///
/// The index is numbered across sections rather than within one, which is what
/// the content view's own `on_select` reports — pressing `Labels` in the second
/// section therefore clears `Inbox` in the first, as a single-selection
/// destination list must.
///
/// Open/closed is not modelled: this case records the in-layout content
/// surface, which has none. See the [module docs](self).
struct NavigationDrawerCase;

impl Component for NavigationDrawerCase {
    type State = NavigationDrawerState;

    fn init(&self) -> Self::State {
        NavigationDrawerState { selected: 0 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            SURFACE,
            navigation_drawer_content(
                vec![
                    drawer_section(vec![
                        drawer_destination(glyph(icons::INBOX), "Inbox").badge_label("24"),
                        drawer_destination(glyph(icons::SEND), "Outbox"),
                        drawer_destination(glyph(icons::FAVORITE), "Favorites"),
                    ])
                    .header("Mail"),
                    drawer_section(vec![drawer_destination(glyph(icons::LABEL), "Labels")])
                        .header("Organize"),
                ],
                state.selected,
                |state: &mut NavigationDrawerState, index| state.selected = index,
            ),
        )
    }
}

fn navigation_drawer_case() -> AnyView<()> {
    any(component(NavigationDrawerCase))
}

// ---- navigation-rail --------------------------------------------------------

/// Retained state for the interactive `navigation-rail` case: the selected
/// destination, seeded to the recorded one.
struct NavigationRailState {
    selected: usize,
}

/// The `navigation-rail` case with its destination selection live.
///
/// The rail runs the same indicator springs the navigation bar does, along its
/// own axis. Its expand/collapse ramp stays out of reach at this case's frame
/// width — see the [module docs](self) — so the rail keeps
/// `NavigationRailType::AlwaysCollapse` and offers no menu button.
struct NavigationRailCase;

impl Component for NavigationRailCase {
    type State = NavigationRailState;

    fn init(&self) -> Self::State {
        NavigationRailState { selected: 1 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            RAIL,
            SizedBox(Some(RAIL_W), Some(440.0)).child(
                navigation_rail(
                    vec![rail_section(vec![
                        rail_destination(glyph(icons::HOME), "Home"),
                        rail_destination(glyph(icons::SEARCH), "Search"),
                        rail_destination(glyph(icons::BOOKMARK), "Saved"),
                        rail_destination(glyph(icons::TUNE), "Settings"),
                    ])],
                    state.selected,
                    |state: &mut NavigationRailState, index| state.selected = index,
                )
                .rail_type(NavigationRailType::AlwaysCollapse)
                .fab(rail_fab(
                    icons::ADD,
                    "Compose",
                    |_: &mut NavigationRailState| {},
                )),
            ),
        )
    }
}

fn navigation_rail_case() -> AnyView<()> {
    any(component(NavigationRailCase))
}

// ---- selection-controls -----------------------------------------------------

/// Retained state for the interactive `selection-controls` case: one bool per
/// checkbox and per switch plus the radio group's chosen key, each seeded to
/// the value the recorded case hardcodes.
///
/// The two switches carry separate bools because the recorded pair exists to
/// show both switch positions at once; collapsing them into one would lose
/// that.
struct SelectionControlsState {
    notifications: bool,
    digest: bool,
    plan: &'static str,
    sync: bool,
    backup: bool,
}

/// The `selection-controls` case — the cheapest state in this catalog and the
/// largest visual payoff.
///
/// The switch is the reason: its track and its handle run two *independent*
/// springs, so the handle overshoots while the track is still settling and the
/// two catch up with each other. A frame recorded at either end shows neither
/// spring. The checkbox's tick draw-on and the radio's inner-dot scale are the
/// same story at smaller scale.
///
/// The radio group holds the chosen key rather than a bool per button: `radio`
/// reports the value it carries and never sets its own, so mutual exclusion is
/// the app's job, and one key cannot express the both-lit state two bools
/// admit.
struct SelectionControlsCase;

impl Component for SelectionControlsCase {
    type State = SelectionControlsState;

    fn init(&self) -> Self::State {
        SelectionControlsState {
            notifications: true,
            digest: false,
            plan: "standard",
            sync: true,
            backup: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            column()
                .child(
                    row()
                        .child(checkbox(
                            state.notifications,
                            |state: &mut SelectionControlsState, checked| {
                                state.notifications = checked
                            },
                        ))
                        .child(hgap(8.0))
                        .child(text("Notifications").size(14.0))
                        .child(hgap(24.0))
                        .child(checkbox(
                            state.digest,
                            |state: &mut SelectionControlsState, checked| state.digest = checked,
                        ))
                        .child(hgap(8.0))
                        .child(text("Digest").size(14.0))
                        .cross_axis(CrossAxisAlignment::Center),
                )
                .child(gap(16.0))
                .child(
                    row()
                        .child(radio("standard", state.plan).label("Standard").on_changed(
                            |state: &mut SelectionControlsState, plan| state.plan = plan,
                        ))
                        .child(hgap(20.0))
                        .child(radio("express", state.plan).label("Express").on_changed(
                            |state: &mut SelectionControlsState, plan| state.plan = plan,
                        ))
                        .cross_axis(CrossAxisAlignment::Center),
                )
                .child(gap(16.0))
                .child(
                    row()
                        .child(switch(
                            state.sync,
                            |state: &mut SelectionControlsState, checked| state.sync = checked,
                        ))
                        .child(hgap(20.0))
                        .child(switch(
                            state.backup,
                            |state: &mut SelectionControlsState, checked| state.backup = checked,
                        ))
                        .cross_axis(CrossAxisAlignment::Center),
                )
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn selection_controls_case() -> AnyView<()> {
    any(component(SelectionControlsCase))
}

// ---- slider -----------------------------------------------------------------

/// Retained state for the interactive `slider` case: one value per track,
/// seeded to the four the recorded case hardcodes.
struct SliderState {
    volume: f64,
    steps: f64,
    balance: f64,
    wave: f64,
}

/// The four slider variants with their values app-owned.
///
/// The wavy slider's wave already travels in a recorded frame, because that
/// motion is time-driven rather than value-driven; everything else about these
/// four — the thumb, the active track, the stepped slider's tick snapping, and
/// the wave's own amplitude ramp behind the thumb — waits on a value the
/// recorded case cannot change. The row keeps the recorded geometry and adds no
/// readout: unlike the baseline catalog's slider, none of these four carries a
/// caption a drag could put out of step.
struct SliderCase;

impl Component for SliderCase {
    type State = SliderState;

    fn init(&self) -> Self::State {
        SliderState {
            volume: 0.4,
            steps: 3.0,
            balance: 0.65,
            wave: 0.5,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed(
            column()
                .child(
                    SizedBox(Some(280.0), None)
                        .child(slider(state.volume, |state: &mut SliderState, value| {
                            state.volume = value
                        })),
                )
                .child(gap(12.0))
                .child(
                    SizedBox(Some(280.0), None).child(
                        slider(state.steps, |state: &mut SliderState, value| {
                            state.steps = value
                        })
                        .range(0.0, 5.0)
                        .divisions(5),
                    ),
                )
                .child(gap(12.0))
                .child(SizedBox(Some(280.0), None).child(centered_slider(
                    state.balance,
                    |state: &mut SliderState, value| state.balance = value,
                )))
                .child(gap(12.0))
                .child(
                    SizedBox(Some(280.0), None)
                        .child(wavy_slider(state.wave, |state: &mut SliderState, value| {
                            state.wave = value
                        })),
                )
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn slider_case() -> AnyView<()> {
    any(component(SliderCase))
}

// ---- snackbar ---------------------------------------------------------------

/// Retained state for the interactive `snackbar` case: the controller the
/// button pushes into, and whether the last message's `Undo` was taken.
///
/// The controller is typed over this very state — that is the documented mount
/// shape, and it is what lets a message's own action write back here.
struct SnackbarState {
    toasts: SnackbarController<SnackbarState>,
    undone: bool,
}

/// The `snackbar` case with a button that actually shows the bar.
///
/// The recorded case mounts the real host with a message already requested and
/// captures the frame before the entrance ramp advances past zero, so the
/// poster shows the host's app content alone. A live host paints continuously,
/// so that ramp runs: this constructor deliberately does **not** pre-show
/// anything at `init` — the opening frame stays the poster's, and the bar
/// slides up when the button asks it to, which is also the host's own
/// documented mount example.
///
/// The headline answers `Undo` rather than a counter being bolted on, so the
/// action has a visible consequence without adding a line the recorded frame
/// does not have.
struct SnackbarCase;

impl Component for SnackbarCase {
    type State = SnackbarState;

    fn init(&self) -> Self::State {
        SnackbarState {
            toasts: SnackbarController::new(),
            undone: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let headline = if state.undone {
            "Archive undone"
        } else {
            "Archive the selected message"
        };
        framed_in(
            STRIP,
            snackbar_host(
                &state.toasts,
                container(
                    column()
                        .child(text(headline).size(14.0))
                        .child(gap(16.0))
                        .child(filled_button("Archive", |state: &mut SnackbarState| {
                            state.undone = false;
                            state.toasts.show(
                                snackbar("Message archived")
                                    .action("Undo", |state: &mut SnackbarState| {
                                        state.undone = true
                                    }),
                            );
                        }))
                        .cross_axis(CrossAxisAlignment::Center),
                )
                .size_centered(STRIP.width, STRIP.height),
            ),
        )
    }
}

fn snackbar_case() -> AnyView<()> {
    any(component(SnackbarCase))
}

// ---- tabs -------------------------------------------------------------------

/// Retained state for the interactive `tabs` case: the selected tab, seeded to
/// the recorded one.
struct TabsState {
    selected: usize,
}

/// The `tabs` case with a selection that moves.
///
/// The indicator slides between tabs on a change, so a pinned index shows a
/// reader exactly the one thing the widget does not do. One `usize`, and the
/// recorded frame's selected second tab becomes a starting point rather than
/// the whole story.
struct TabsCase;

impl Component for TabsCase {
    type State = TabsState;

    fn init(&self) -> Self::State {
        TabsState { selected: 1 }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let specs: Vec<Tab> = vec![
            tab().label("Overview").icon(icons::SPACE_DASHBOARD),
            tab().label("Activity").icon(icons::SCHEDULE),
            tab().label("Files").icon(icons::LIST),
        ];
        framed_in(
            BAR,
            bleed(
                tabs(specs, state.selected, |state: &mut TabsState, index| {
                    state.selected = index
                })
                .variant(TabsVariant::Primary),
            ),
        )
    }
}

fn tabs_case() -> AnyView<()> {
    any(component(TabsCase))
}

// ---- text-field -------------------------------------------------------------

/// The shortest password this case accepts, and therefore the boundary its
/// error text describes. The recorded case's own `"hunter2"` is one character
/// short of it, which is what makes the pinned error true at rest.
const MIN_PASSWORD_LEN: usize = 8;

/// Retained state for the interactive `text-field` case: one `String` per
/// field, seeded to what the recorded case shows.
struct TextFieldState {
    email: String,
    display_name: String,
    password: String,
}

/// The `text-field` case with a value round trip, and an error that means
/// something.
///
/// The value half follows the same four rules the baseline catalog's
/// `text-input` sets out, and the third is the one a field with a caret cannot
/// skip: **store what `on_change` handed you, unmodified**. A text field feeds
/// its editor set-if-different, so a handler that trims or case-folds makes
/// every keystroke differ from the editor's own text; the editor is then
/// reapplied wholesale, which collapses the caret to the end, drops an active
/// selection, and tears down an in-flight IME composition. On a Latin keyboard
/// only the caret jump is visible, which is exactly why the rule is written
/// down rather than discovered.
///
/// The error text is the other half. The recorded case pins
/// `error_text("Too short")` under a value it has no way to measure; here the
/// same text is derived from the password's own length, so it clears itself
/// once the field grows past [`MIN_PASSWORD_LEN`]. Deriving a *sibling prop*
/// from the stored value is not the normalisation the rule above forbids — the
/// stored value is still exactly what the widget reported.
struct TextFieldCase;

impl Component for TextFieldCase {
    type State = TextFieldState;

    fn init(&self) -> Self::State {
        TextFieldState {
            email: "ada@example.com".to_string(),
            display_name: String::new(),
            password: "hunter2".to_string(),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let mut password = text_field(
            state.password.clone(),
            |state: &mut TextFieldState, value| state.password = value,
        )
        .label("Password")
        .obscured(true);
        if state.password.chars().count() < MIN_PASSWORD_LEN {
            password = password.error_text("Too short");
        }
        framed_in(
            TALL,
            column()
                .child(
                    SizedBox(Some(280.0), None).child(
                        text_field(state.email.clone(), |state: &mut TextFieldState, value| {
                            state.email = value
                        })
                        .label("Email")
                        .supporting_text("We never share this"),
                    ),
                )
                .child(gap(16.0))
                .child(
                    SizedBox(Some(280.0), None).child(
                        text_field(
                            state.display_name.clone(),
                            |state: &mut TextFieldState, value| state.display_name = value,
                        )
                        .label("Display name")
                        .variant(TextFieldVariant::Outlined),
                    ),
                )
                .child(gap(16.0))
                .child(SizedBox(Some(280.0), None).child(password))
                .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

fn text_field_case() -> AnyView<()> {
    any(component(TextFieldCase))
}

// ---- time-picker ------------------------------------------------------------

/// Retained state for the interactive `time-picker` case: the time the dial
/// reports, seeded to the recorded one.
struct TimePickerState {
    value: TimeOfDay,
}

/// The `time-picker` case with its dial live.
///
/// One `TimeOfDay` covers all three of the dial's inputs — dragging the hand,
/// pressing a ring number, and the AM/PM selector, which reports as a 12-hour
/// shift of the same value — because the dial is controlled over the whole time
/// rather than over each field of it.
struct TimePickerCase;

impl Component for TimePickerCase {
    type State = TimePickerState;

    fn init(&self) -> Self::State {
        TimePickerState {
            value: TimeOfDay::new(10, 30),
        }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        framed_in(
            PICKER,
            time_dial(state.value, |state: &mut TimePickerState, value| {
                state.value = value
            })
            .use_24_hour(false),
        )
    }
}

fn time_picker_case() -> AnyView<()> {
    any(component(TimePickerCase))
}
