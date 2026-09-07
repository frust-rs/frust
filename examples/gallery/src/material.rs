//! `Material` cases — one per non-index page under
//! `apps/website/widgets/design-systems/material/`, rendered under
//! [`crate::case::Design::Material`] (which [`crate::theme::theme`] resolves
//! to [`frust_material::baseline`], light or dark).
//!
//! Each case builds a static, representative composition of the real
//! `frust_material` component the page documents, following that page's
//! `## Example` block as closely as a pure, state-free `View` allows. Slugs
//! are the page's file stem prefixed with `material/`
//! (`material/button`, `material/text-field`, ...), per the crate docs' slug
//! rule.
//!
//! # Pure `View`, one frame — what that excludes
//!
//! [`Case::build`] is a plain `fn() -> `[`AnyView`]`<()>` recorded through a
//! single build/layout/paint pass (`crates/frust-testing/src/frame.rs`'s
//! `frame`), so every callback here is an empty closure over `()` and every
//! knob is a literal. Two consequences shape this module:
//!
//! - **No navigator.** The `show_*` entry points every overlay page also
//!   documents (`show_dialog`, `show_bottom_sheet`, `show_date_picker`,
//!   `show_time_picker`, `show_navigation_drawer`, ...) push a page onto a
//!   [`frust_widgets::nav::navigator`] host. A case therefore builds the
//!   pushed view itself — `calendar_date_picker(..)`, `time_dial(..)`,
//!   `navigation_drawer_content(..)` — never the pushing helper.
//! - **No entrance ramp.** A widget that ramps in from `progress == 0` on its
//!   first paint is captured before its ramp moves, i.e. invisible, for
//!   **any** [`Case::time_ms`]. See [`ENTRANCE_RAMP_NOTE`] for the three cases
//!   this reaches and how each answers it.
//!
//! Two pages therefore name a component whose *dialog* form is a modal this
//! recorder cannot show, and take that component's own non-modal surface
//! instead: `material/time-picker` records `time_dial` (what
//! `time_picker`/`show_time_picker` put inside the dialog) and
//! `material/date-picker` records `calendar_date_picker` (likewise, versus
//! `date_picker_dialog`).
//!
//! # The variant has to reach the pixels
//!
//! Same rule as [`crate::base`]: every case is recorded twice, once per
//! [`crate::Variant`], onto a clear of that variant's own theme `surface`, and
//! `crates/frust-testing/tests/widget_snapshots_bin.rs` fails a case whose two
//! PNGs come out identical. So nothing here paints a fixed opaque full-frame
//! fill — cases are framed through [`framed`]/[`framed_in`], which size but
//! never fill, captions stay at their themed `on_surface` default, and every
//! Material container resolves its own fill from the active
//! [`frust_material::MaterialTokens`] scheme, which differs by brightness.

use frust_core::{AnyView, any};
use frust_material::{
    AppBarDensity, ButtonGroupType, ButtonVariant, CardVariant, DatePickerState, DropdownItem,
    FabColor, FabSize, IconButtonVariant, MaterialDate, MenuSelection, NavBarLabelBehavior,
    NavigationRailType, OVERLAY_DIALOG_MAX_WIDTH, OVERLAY_HANDLE_RESERVE, OverlayEntrance,
    OverlayModalConfig, OverlaySide, SnackbarController, Tab, TabsVariant, TextFieldVariant,
    TimeOfDay, ToolbarColorStyle, app_bar, assist_chip, bottom_app_bar, button_group_action,
    button_group_actions, button_with_icon, calendar_date_picker, card, centered_slider,
    docked_toolbar, drawer_destination, drawer_section, dropdown_field, dropdown_item,
    dropdown_panel, elevated_button, fab, filled_button, filter_chip, floating_toolbar,
    icon_button, icons, input_chip, list_item, menu_entry, menu_group, menu_panel, nav_item,
    navigation_bar, navigation_drawer_content, navigation_rail, outlined_button, overlay_modal,
    radio, rail_destination, rail_fab, rail_section, segment, segmented_button, slider, snackbar,
    snackbar_host, suggestion_chip, switch, tab, tabs, text_button, text_field, time_dial,
    tonal_button, toolbar_action, wavy_slider,
};
use frust_widgets::{
    Column, CrossAxisAlignment, EdgeInsets, IconSource, Padding, Row, SizedBox, container, icon,
    text,
};
use kurbo::Size;

use crate::base::{framed, framed_in};
use crate::case::{Case, Design};

/// The entrance-ramp limit, and which cases work around it how.
///
/// `frust_material`'s modal host (`OverlayModalWidget`), its bottom sheet
/// (`BottomSheetWidget`) and its snackbar host (`SnackbarHostWidget`) all seed
/// `progress = 0.0` at build and only leave zero once a *second* paint carries
/// a non-zero frame delta: `frust_core`'s `AnimationController` holds
/// `last_time: None` until its first `advance`, so that first call contributes
/// a zero delta whatever [`Case::time_ms`] says. The recorder paints exactly
/// one frame, so anything ramping in is captured *before* its ramp moves —
/// invisible, not merely early. Two responses, per what the public API allows:
///
/// - **[`dialog_case`] and [`sheet_case`]** mount the genuine
///   `overlay_modal` host with the genuine chrome config the components use
///   (`OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)`, the exact
///   config `frust_material::dialog` composes; `OverlayModalConfig::edge(
///   OverlaySide::Bottom).handle(true)`, the M3 sheet chrome), with the one
///   field a one-frame recording needs changed —
///   [`OverlayEntrance::None`], "the panel is at rest on its first frame".
///   `dialog()`/`bottom_sheet(..)` themselves take no entrance argument, so
///   their panel bodies (both private view types) are laid out here from the
///   same Material primitives their pages document.
/// - **[`snackbar_case`]** has no such seam: `snackbar_host` exposes no
///   entrance knob and its bar is drawn by the host widget itself, so the case
///   mounts the real host with a real pending [`snackbar`] message — the
///   documented mount contract — and records the frame before the bar slides
///   up. Its preview therefore shows the host's app content only.
///
/// This is the same single-build limit `crate::base::animation` documents for
/// `pattern-switcher`. Lifting it properly needs a warm-frame field on
/// [`Case`] the recorder rebuilds through before capturing, which `case.rs`
/// is outside this module's remit to add.
pub const ENTRANCE_RAMP_NOTE: &str = "a one-frame recorder cannot advance an entrance ramp off zero: dialog/sheet mount the real \
     overlay host with OverlayEntrance::None, snackbar records its pre-entrance frame";

/// A single full-width bar, or one short row of components, with room to
/// breathe around it.
const BAR: Size = Size::new(400.0, 180.0);
/// Two stacked full-width bars, or a short list of them.
const STRIP: Size = Size::new(400.0, 280.0);
/// A panel or stacked-field column taller than [`Case::DEFAULT_SIZE`].
const TALL: Size = Size::new(400.0, 340.0);
/// A full navigation/overlay surface that wants most of a phone viewport.
const SURFACE: Size = Size::new(400.0, 420.0);
/// The pickers' frame — `CALENDAR_WIDTH` and `DIALOG_PORTRAIT_WIDTH` are both
/// 328pt, and the calendar's six day rows plus its header need the height.
const PICKER: Size = Size::new(392.0, 400.0);
/// The navigation rail's frame: narrow, tall, and boxed to [`RAIL_W`] so the
/// rail reads as a rail rather than stretching across the viewport.
const RAIL: Size = Size::new(200.0, 480.0);

/// The logical width [`navigation_rail_case`] boxes its rail to —
/// `frust_material::RAIL_COLLAPSED_WIDTH`. A rail sizes to its constraint, and
/// a rail as wide as the frame is not a rail.
const RAIL_W: f64 = 96.0;

/// The logical width every full-bleed component in this module is boxed to.
/// `framed`'s container hands its child LOOSE constraints, so a bar, field or
/// list that would otherwise stretch to fill a window instead shrink-wraps to
/// its content and sits narrower than the frame — this pins them to a
/// phone-ish column with the cleared surface still visible around it.
const BLEED_W: f64 = 344.0;

/// `icon(..)` as an `AnyView<()>` — the shape every Material slot that takes a
/// leading/trailing glyph expects.
fn glyph(source: IconSource) -> AnyView<()> {
    any(icon(source))
}

/// A fixed-width box around a component that would otherwise shrink-wrap; see
/// [`BLEED_W`].
fn bleed<V: frust_core::View<()>>(child: V) -> AnyView<()> {
    any(SizedBox(Some(BLEED_W), None).child(child))
}

/// A vertical gap between stacked rows in a case.
fn gap(height: f64) -> AnyView<()> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal gap between side-by-side controls in a case.
fn hgap(width: f64) -> AnyView<()> {
    any(SizedBox(Some(width), None))
}

fn app_bar_case() -> AnyView<()> {
    framed_in(
        STRIP,
        Column(vec![
            bleed(
                app_bar("Inbox")
                    .leading(any(icon_button(glyph(icons::MENU), |_: &mut ()| {})))
                    .actions(vec![
                        any(icon_button(glyph(icons::SEARCH), |_: &mut ()| {})),
                        any(icon_button(glyph(icons::MORE_VERT), |_: &mut ()| {})),
                    ])
                    .density(AppBarDensity::Regular),
            ),
            gap(24.0),
            bleed(
                bottom_app_bar()
                    .actions(vec![
                        any(icon_button(glyph(icons::MENU), |_: &mut ()| {})),
                        any(icon_button(glyph(icons::SEARCH), |_: &mut ()| {})),
                        any(icon_button(glyph(icons::EDIT), |_: &mut ()| {})),
                    ])
                    .fab(any(
                        fab(glyph(icons::ADD), |_: &mut ()| {}).size(FabSize::Small)
                    )),
            ),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn button_group_case() -> AnyView<()> {
    framed_in(
        STRIP,
        Column(vec![
            any(button_group_actions(vec![
                button_group_action("Left").icon(|| glyph(icons::FORMAT_ALIGN_LEFT)),
                button_group_action("Center").icon(|| glyph(icons::FORMAT_ALIGN_CENTER)),
                button_group_action("Right").icon(|| glyph(icons::FORMAT_ALIGN_RIGHT)),
            ])
            .group_type(ButtonGroupType::Connected)
            .selected_index(Some(1))
            .on_select(|_: &mut (), _: usize| {})),
            gap(24.0),
            any(segmented_button(
                vec![
                    segment("day").label("Day"),
                    segment("week").label("Week"),
                    segment("month").label("Month"),
                ],
                ["week"],
                |_: &mut (), _: Vec<&'static str>| {},
            )),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn button_case() -> AnyView<()> {
    framed(
        Column(vec![
            any(Row(vec![
                any(filled_button("Filled", |_: &mut ()| {})),
                hgap(12.0),
                any(tonal_button("Tonal", |_: &mut ()| {})),
                hgap(12.0),
                any(elevated_button("Elevated", |_: &mut ()| {})),
            ])),
            gap(16.0),
            any(Row(vec![
                any(outlined_button("Outlined", |_: &mut ()| {})),
                hgap(12.0),
                any(text_button("Text", |_: &mut ()| {})),
                hgap(12.0),
                any(button_with_icon(glyph(icons::ADD), "Icon", |_: &mut ()| {})
                    .variant(ButtonVariant::Filled)),
            ])),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// One card's body — a headline plus a supporting line, boxed so the three
/// cards in [`card_case`] measure alike.
fn card_body(title: &'static str) -> AnyView<()> {
    any(container(
        Column(vec![
            any(text(title).size(14.0)),
            any(text("Body").size(12.0)),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
    .size_centered(76.0, 64.0))
}

fn card_case() -> AnyView<()> {
    framed_in(
        BAR,
        Row(vec![
            any(card(CardVariant::Elevated, card_body("Elevated"))),
            hgap(12.0),
            any(card(CardVariant::Filled, card_body("Filled"))),
            hgap(12.0),
            any(card(CardVariant::Outlined, card_body("Outlined")).on_press(|_: &mut ()| {})),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn chips_case() -> AnyView<()> {
    framed(
        Column(vec![
            any(Row(vec![
                any(assist_chip("Add to calendar", |_: &mut ()| {})),
                hgap(10.0),
                any(filter_chip("Unread", true, |_: &mut (), _: bool| {})),
            ])),
            gap(16.0),
            any(Row(vec![
                any(input_chip("Ada Lovelace", |_: &mut ()| {}).on_deleted(|_: &mut ()| {})),
                hgap(10.0),
                any(suggestion_chip("Reply", |_: &mut ()| {}).elevated(true)),
            ])),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn date_picker_case() -> AnyView<()> {
    let today = MaterialDate::new(2026, 9, 7);
    let selected = MaterialDate::new(2026, 9, 17);
    framed_in(
        PICKER,
        calendar_date_picker(
            DatePickerState::new(Some(selected), today),
            MaterialDate::new(2026, 1, 1),
            MaterialDate::new(2026, 12, 31),
            |_: &mut (), _: DatePickerState| {},
        )
        .today(today),
    )
}

/// The M3 dialog on its `overlay_modal` host, pinned at rest — see
/// [`ENTRANCE_RAMP_NOTE`] for why the panel body is laid out here rather than
/// handed to `frust_material::dialog`.
fn dialog_case() -> AnyView<()> {
    framed_in(
        TALL,
        overlay_modal(
            Padding(
                EdgeInsets::all(24.0),
                Column(vec![
                    any(text("Delete draft?").size(24.0)),
                    gap(16.0),
                    any(text(
                        "This draft will be permanently removed from every signed-in device.",
                    )
                    .size(14.0)),
                    gap(24.0),
                    any(Row(vec![
                        any(text_button("Cancel", |_: &mut ()| {})),
                        hgap(8.0),
                        any(filled_button("Delete", |_: &mut ()| {})),
                    ])),
                ]),
            ),
            OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH).entrance(OverlayEntrance::None),
        ),
    )
}

/// The option list both dropdown surfaces in [`dropdown_case`] read.
fn dropdown_items() -> Vec<DropdownItem> {
    vec![
        dropdown_item("Apple", "apple"),
        dropdown_item("Blackberry", "blackberry"),
        dropdown_item("Cherry", "cherry"),
        dropdown_item("Damson", "damson"),
    ]
}

fn dropdown_case() -> AnyView<()> {
    framed_in(
        SURFACE,
        Column(vec![
            bleed(
                dropdown_field(dropdown_items())
                    .selected(vec!["cherry".to_string()])
                    .hint("Pick a fruit")
                    .show_clear(true),
            ),
            gap(12.0),
            any(
                dropdown_panel(dropdown_items(), |_: &mut (), _: Vec<String>| {})
                    .selected(vec!["cherry".to_string()])
                    .max_height(220.0),
            ),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The three FAB sizes and a colour spread. `extended_fab` is deliberately
/// absent: `FabWidget` seeds its label reveal at `0` and only calls
/// `label_anim.forward()` at build, so an already-extended FAB paints its
/// collapsed pill under a full-width label on the recorder's single frame —
/// the same one-frame limit as [`ENTRANCE_RAMP_NOTE`], reached here through a
/// widget with no `entrance`-style seam to opt out of.
fn fab_case() -> AnyView<()> {
    framed(
        Row(vec![
            any(fab(glyph(icons::ADD), |_: &mut ()| {}).size(FabSize::Small)),
            hgap(16.0),
            any(fab(glyph(icons::EDIT), |_: &mut ()| {})
                .size(FabSize::Medium)
                .color(FabColor::Tertiary)),
            hgap(16.0),
            any(fab(glyph(icons::SEND), |_: &mut ()| {})
                .size(FabSize::Large)
                .color(FabColor::Secondary)),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn icon_button_case() -> AnyView<()> {
    framed(
        Row(vec![
            any(icon_button(glyph(icons::FAVORITE), |_: &mut ()| {}).semantic_label("Favorite")),
            hgap(12.0),
            any(icon_button(glyph(icons::FAVORITE), |_: &mut ()| {})
                .variant(IconButtonVariant::Filled)
                .selected(true)),
            hgap(12.0),
            any(icon_button(glyph(icons::BOOKMARK), |_: &mut ()| {})
                .variant(IconButtonVariant::Tonal)),
            hgap(12.0),
            any(icon_button(glyph(icons::SHARE), |_: &mut ()| {})
                .variant(IconButtonVariant::Outlined)),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn list_case() -> AnyView<()> {
    framed_in(
        STRIP,
        Column(vec![
            bleed(
                list_item("Weekly sync")
                    .supporting("Tomorrow, 09:00")
                    .leading(icon(icons::SCHEDULE))
                    .trailing(icon(icons::CHEVRON_RIGHT))
                    .on_press(|_: &mut ()| {}),
            ),
            bleed(
                list_item("Design review")
                    .supporting("Thursday, 14:30")
                    .leading(icon(icons::LABEL))
                    .trailing(icon(icons::CHEVRON_RIGHT))
                    .on_press(|_: &mut ()| {}),
            ),
            bleed(
                list_item("Release notes")
                    .supporting("Draft shared with the team")
                    .leading(icon(icons::ARCHIVE))
                    .selected(true)
                    .on_press(|_: &mut ()| {}),
            ),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn menu_case() -> AnyView<()> {
    framed_in(
        TALL,
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
            |_: &mut (), _: MenuSelection| {},
        ),
    )
}

fn navigation_bar_case() -> AnyView<()> {
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
                0,
                |_: &mut (), _: usize| {},
            )
            .label_behavior(NavBarLabelBehavior::AlwaysShow),
        ),
    )
}

fn navigation_drawer_case() -> AnyView<()> {
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
            0,
            |_: &mut (), _: usize| {},
        ),
    )
}

fn navigation_rail_case() -> AnyView<()> {
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
                1,
                |_: &mut (), _: usize| {},
            )
            .rail_type(NavigationRailType::AlwaysCollapse)
            .fab(rail_fab(icons::ADD, "Compose", |_: &mut ()| {})),
        ),
    )
}

fn selection_controls_case() -> AnyView<()> {
    framed(
        Column(vec![
            any(Row(vec![
                any(frust_material::checkbox(true, |_: &mut (), _: bool| {})),
                hgap(8.0),
                any(text("Notifications").size(14.0)),
                hgap(24.0),
                any(frust_material::checkbox(false, |_: &mut (), _: bool| {})),
                hgap(8.0),
                any(text("Digest").size(14.0)),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
            gap(16.0),
            any(Row(vec![
                any(radio("standard", "standard")
                    .label("Standard")
                    .on_changed(|_: &mut (), _: &'static str| {})),
                hgap(20.0),
                any(radio("express", "standard")
                    .label("Express")
                    .on_changed(|_: &mut (), _: &'static str| {})),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
            gap(16.0),
            any(Row(vec![
                any(switch(true, |_: &mut (), _: bool| {})),
                hgap(20.0),
                any(switch(false, |_: &mut (), _: bool| {})),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The M3 bottom-sheet chrome on its `overlay_modal` host, pinned at rest —
/// see [`ENTRANCE_RAMP_NOTE`] for why this is not `bottom_sheet(..)`.
fn sheet_case() -> AnyView<()> {
    framed_in(
        TALL,
        overlay_modal(
            Padding(
                EdgeInsets {
                    left: 24.0,
                    top: OVERLAY_HANDLE_RESERVE,
                    right: 24.0,
                    bottom: 24.0,
                },
                container(
                    Column(vec![
                        any(text("Share via").size(16.0)),
                        gap(16.0),
                        any(Row(vec![
                            any(icon_button(glyph(icons::MAIL), |_: &mut ()| {})),
                            hgap(12.0),
                            any(icon_button(glyph(icons::MESSAGE), |_: &mut ()| {})),
                            hgap(12.0),
                            any(icon_button(glyph(icons::CONTENT_COPY), |_: &mut ()| {})),
                            hgap(12.0),
                            any(icon_button(glyph(icons::SHARE), |_: &mut ()| {})),
                        ])),
                    ])
                    .cross_axis(CrossAxisAlignment::Center),
                )
                .size_centered(TALL.width - 48.0, 100.0),
            ),
            OverlayModalConfig::edge(OverlaySide::Bottom)
                .handle(true)
                .entrance(OverlayEntrance::None),
        ),
    )
}

fn slider_case() -> AnyView<()> {
    framed(
        Column(vec![
            any(SizedBox(Some(280.0), None).child(slider(0.4, |_: &mut (), _: f64| {}))),
            gap(12.0),
            any(SizedBox(Some(280.0), None).child(
                slider(3.0, |_: &mut (), _: f64| {})
                    .range(0.0, 5.0)
                    .divisions(5),
            )),
            gap(12.0),
            any(SizedBox(Some(280.0), None).child(centered_slider(0.65, |_: &mut (), _: f64| {}))),
            gap(12.0),
            any(SizedBox(Some(280.0), None).child(wavy_slider(0.5, |_: &mut (), _: f64| {}))),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The snackbar host with a message already requested — the page's own mount
/// contract (`snackbar_host(&state.toasts, app_view)` at the root, a
/// `controller.show(..)` from an action). The bar itself is still off-screen
/// on this frame; see [`ENTRANCE_RAMP_NOTE`].
fn snackbar_case() -> AnyView<()> {
    let toasts: SnackbarController<()> = SnackbarController::new();
    toasts.show(snackbar("Message archived").action("Undo", |_: &mut ()| {}));
    framed_in(
        STRIP,
        snackbar_host(
            &toasts,
            container(
                Column(vec![
                    any(text("Archive the selected message").size(14.0)),
                    gap(16.0),
                    any(filled_button("Archive", |_: &mut ()| {})),
                ])
                .cross_axis(CrossAxisAlignment::Center),
            )
            .size_centered(STRIP.width, STRIP.height),
        ),
    )
}

/// The three tabs [`tabs_case`] shows, built the way the page's example does.
fn tab_specs() -> Vec<Tab> {
    vec![
        tab().label("Overview").icon(icons::SPACE_DASHBOARD),
        tab().label("Activity").icon(icons::SCHEDULE),
        tab().label("Files").icon(icons::LIST),
    ]
}

fn tabs_case() -> AnyView<()> {
    framed_in(
        BAR,
        bleed(tabs(tab_specs(), 1, |_: &mut (), _: usize| {}).variant(TabsVariant::Primary)),
    )
}

fn text_field_case() -> AnyView<()> {
    framed_in(
        TALL,
        Column(vec![
            any(SizedBox(Some(280.0), None).child(
                text_field("ada@example.com", |_: &mut (), _: String| {})
                    .label("Email")
                    .supporting_text("We never share this"),
            )),
            gap(16.0),
            any(SizedBox(Some(280.0), None).child(
                text_field("", |_: &mut (), _: String| {})
                    .label("Display name")
                    .variant(TextFieldVariant::Outlined),
            )),
            gap(16.0),
            any(SizedBox(Some(280.0), None).child(
                text_field("hunter2", |_: &mut (), _: String| {})
                    .label("Password")
                    .obscured(true)
                    .error_text("Too short"),
            )),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The time picker's own dial surface. `time_picker(..)` composes an
/// `OverlayModalWidget` (it *is* the dialog), which a one-frame recording
/// cannot bring on screen — see [`ENTRANCE_RAMP_NOTE`]; `time_dial` is the
/// non-modal ring that dialog wraps.
fn time_picker_case() -> AnyView<()> {
    framed_in(
        PICKER,
        time_dial(TimeOfDay::new(10, 30), |_: &mut (), _: TimeOfDay| {}).use_24_hour(false),
    )
}

fn toolbar_case() -> AnyView<()> {
    framed_in(
        STRIP,
        Column(vec![
            bleed(docked_toolbar().actions(vec![
                toolbar_action(icons::FORMAT_ALIGN_LEFT, |_: &mut ()| {}).label("Left"),
                toolbar_action(icons::FORMAT_ALIGN_CENTER, |_: &mut ()| {}).label("Center"),
                toolbar_action(icons::FORMAT_ALIGN_RIGHT, |_: &mut ()| {}).label("Right"),
            ])),
            gap(24.0),
            any(floating_toolbar()
                .color_style(ToolbarColorStyle::Vibrant)
                .actions(vec![
                    toolbar_action(icons::EDIT, |_: &mut ()| {}).label("Edit"),
                    toolbar_action(icons::SHARE, |_: &mut ()| {}).label("Share"),
                    toolbar_action(icons::DELETE, |_: &mut ()| {}).label("Delete"),
                ])),
        ])
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// This module's slice of the registry [`crate::cases`] concatenates — one
/// case per non-index page under
/// `apps/website/widgets/design-systems/material/`.
pub const CASES: &[Case] = &[
    Case {
        slug: "material/app-bar",
        title: "Material App Bar",
        size: STRIP,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: app_bar_case,
    },
    Case {
        slug: "material/button-group",
        title: "Material Button Group",
        size: STRIP,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: button_group_case,
    },
    Case {
        slug: "material/button",
        title: "Material Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: button_case,
    },
    Case {
        slug: "material/card",
        title: "Material Card",
        size: BAR,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: card_case,
    },
    Case {
        slug: "material/chips",
        title: "Material Chips",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: chips_case,
    },
    Case {
        slug: "material/date-picker",
        title: "Material Date Picker",
        size: PICKER,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: date_picker_case,
    },
    Case {
        slug: "material/dialog",
        title: "Material Dialog",
        size: TALL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: dialog_case,
    },
    Case {
        slug: "material/dropdown",
        title: "Material Dropdown",
        size: SURFACE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: dropdown_case,
    },
    Case {
        slug: "material/fab",
        title: "Material FAB",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: fab_case,
    },
    Case {
        slug: "material/icon-button",
        title: "Material Icon Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: icon_button_case,
    },
    Case {
        slug: "material/list",
        title: "Material List",
        size: STRIP,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: list_case,
    },
    Case {
        slug: "material/menu",
        title: "Material Menu",
        size: TALL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: menu_case,
    },
    Case {
        slug: "material/navigation-bar",
        title: "Material Navigation Bar",
        size: BAR,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: navigation_bar_case,
    },
    Case {
        slug: "material/navigation-drawer",
        title: "Material Navigation Drawer",
        size: SURFACE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: navigation_drawer_case,
    },
    Case {
        slug: "material/navigation-rail",
        title: "Material Navigation Rail",
        size: RAIL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: navigation_rail_case,
    },
    Case {
        slug: "material/selection-controls",
        title: "Material Selection Controls",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: selection_controls_case,
    },
    Case {
        slug: "material/sheet",
        title: "Material Sheet",
        size: TALL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: sheet_case,
    },
    Case {
        slug: "material/slider",
        title: "Material Slider",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: slider_case,
    },
    Case {
        slug: "material/snackbar",
        title: "Material Snackbar",
        size: STRIP,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: snackbar_case,
    },
    Case {
        slug: "material/tabs",
        title: "Material Tabs",
        size: BAR,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: tabs_case,
    },
    Case {
        slug: "material/text-field",
        title: "Material Text Field",
        size: TALL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: text_field_case,
    },
    Case {
        slug: "material/time-picker",
        title: "Material Time Picker",
        size: PICKER,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: time_picker_case,
    },
    Case {
        slug: "material/toolbar",
        title: "Material Toolbar",
        size: STRIP,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Material,
        build: toolbar_case,
    },
];
