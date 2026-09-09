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
//! # Pure `View`, one captured frame — what that excludes
//!
//! [`Case::build`] is a plain `fn() -> `[`AnyView`]`<()>` recorded through one
//! captured build/layout/paint pass (`crates/frust-testing/src/frame.rs`'s
//! `frame`), so every callback here is an empty closure over `()` and every
//! knob is a literal. Two consequences shape this module:
//!
//! - **No navigator.** The `show_*` entry points every overlay page also
//!   documents (`show_dialog`, `show_bottom_sheet`, `show_date_picker`,
//!   `show_time_picker`, `show_navigation_drawer`, ...) push a page onto a
//!   [`frust_widgets::nav::navigator`] host. A case therefore builds the
//!   pushed view itself — `calendar_date_picker(..)`, `time_dial(..)`,
//!   `navigation_drawer_content(..)` — never the pushing helper.
//! - **An entrance ramp needs a warm pass.** A widget that ramps in from
//!   `progress == 0` on its first paint is captured before its ramp moves —
//!   invisible, not merely early — unless the case pins a non-zero
//!   [`Case::time_ms`], which is also how it asks the recorder for the
//!   discarded warm paint that lets that timestamp mean anything
//!   ([`Case::warm_frames`]). That reaches a ramp composited in paint; it does
//!   not reach one driven in *layout*. See the "entrance-ramp limit" section
//!   below for the three cases this module has to answer and which answer each
//!   one takes.
//!
//! Two pages therefore record a component's own non-modal surface rather than
//! the dialog form its page's `show_*` helper pushes onto a navigator:
//! `material/time-picker` records `time_dial` (what
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

// # The entrance-ramp limit, and how cases answer it
//
// `frust_material`'s modal host (`OverlayModalWidget`), its bottom sheet
// (`BottomSheetWidget`) and its snackbar host (`SnackbarHostWidget`) all seed
// `progress = 0.0` at build and only leave zero once a *later* paint carries a
// non-zero frame delta: `frust_core`'s `AnimationController` holds
// `last_time: None` until its first `advance`, so that first call only seeds
// the clock, whatever [`Case::time_ms`] says. A case that pins a non-zero
// `time_ms` therefore also buys the discarded warm paint that supplies that
// first `advance` ([`Case::warm_frames`]), leaving the captured pass to see the
// whole of `time_ms` as one delta — and a `Duration` ramp handed a delta at or
// past its own duration lands on exactly `1.0`, not a hair short. A case that
// pins nothing still records its ramp at zero. Three cases here reach this, and
// the single warm pass the registry derives answers only one:
//
// - **[`dialog_case`]** mounts the genuine `overlay_modal` host with the exact
//   chrome config `frust_material::dialog` composes
//   (`OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)`) — entrance
//   included, since [`OverlayEntrance::FadeScale`] is composited in *paint*,
//   which one warm pass is enough for. The case pins `time_ms: 400` instead of
//   pinning the entrance off, so the live catalog page animates and the poster
//   still records settled (verified byte-identical to the pinned one). This
//   module registers no [`crate::interactive`] twin for `material/dialog`, so
//   `Case::build` IS what a live host renders
//   (`find_interactive(slug).unwrap_or(case.build)`) — measured in headless
//   Chrome, the panel fades and scales across ~13 frames of the page load.
// - **[`sheet_case`]** keeps [`OverlayEntrance::None`], because the warm pass
//   cannot stand in for the pin here. [`OverlayEntrance::Slide`] — what
//   `OverlayModalConfig::edge` defaults to — moves the panel in *layout*, and
//   `OverlayModalWidget::layout` reads the `progress` the *previous* paint left
//   behind. With one warm pass the captured pass therefore lays the panel out
//   at progress 0, off its own edge, and then paints it there at the full
//   opacity its now-settled ramp asks for: a poster that is scrim and nothing
//   else (measured, both variants). Two warm passes fix it exactly — but the
//   count alone is not the condition, and a follow-up scoped off the count
//   alone would fail. `FrameSpec::warm_time` spaces the warm schedule across
//   `time_ms` rather than stepping it by the ramp's duration, so the LAST warm
//   pass lands at `time_ms` × (count − 1) / count, and it is that pass, not the
//   capture, that has to reach the entrance's own duration for the capture's
//   LAYOUT to read a settled `progress`. Measured at count 2 against the 500 ms
//   `Slide`: `time_ms: 800` moves all six of those PNGs, while `1_000` and
//   `1_200` leave them byte-identical. Two passes therefore want `time_ms` ≥
//   2 × duration — a `warm_frames: 2` bolted onto a 400 ms `time_ms` would fail
//   for that reason rather than disproving the fix. So this waits on a per-case
//   warm count, the follow-up [`Case::warm_frames`] already names, arriving
//   with a `time_ms` to match. This one is paid
//   for live as well, unlike shadcn's edge cases: there is no
//   [`crate::interactive`] twin for `material/sheet` either, so a browser
//   renders this pinned constructor and the sheet is simply open on arrival
//   (measured: one painted frame, then no further frames at all).
//   Independently of the entrance, this case mounts the overlay-modal host with
//   the M3 sheet chrome preset
//   (`OverlayModalConfig::edge(OverlaySide::Bottom).handle(true)`) because
//   `BottomSheetView` **exposes no settled-entrance seam** at all — its own
//   `build()` call seeds the widget at `progress = 0.0` and only advances it
//   via spring on a later paint. So rather than the genuine `bottom_sheet(..)`
//   widget we **approximate** its at-rest chrome, matching three duplicated
//   magic numbers: **28dp corner radius** (`shape.extra_large`), **32×4dp drag
//   handle** (visual indicator), and **48dp handle touch target**. The risk is
//   that if `BottomSheetWidget` drifts (a corner radius bump, a different
//   handle size, changes to the scrim or panel background) the approximation
//   becomes stale without automatic discovery. This limitation is documented
//   alongside other known gaps.
// - **[`snackbar_case`]** pins no `time_ms`, so it takes no warm pass and
//   records the frame before the bar slides up: its preview shows the host's
//   app content only. `snackbar_host` exposes no entrance knob and its bar is
//   drawn by the host widget itself, so the case mounts the real host with a
//   real pending [`snackbar`] message — the documented mount contract. One warm
//   pass *does* settle the bar (measured), so opting in is one line here plus
//   one in `case.rs`'s allowlist; it moves a published poster, which is a
//   decision of its own rather than a rider on the entrance work.
//
// [`crate::base::animation`] documents the other half of the same seam: three
// cases warming buys nothing for, because what they stage on is a rebuild with
// *different inputs*, and every `Case::build` pass returns the same tree.

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

/// The M3 dialog on its `overlay_modal` host, entrance included: the panel
/// fades and scales in exactly as `frust_material::dialog` composes it, and
/// `time_ms: 400` is what lands the poster on the settled frame — see the
/// module docs' entrance-ramp section.
///
/// The panel *body* is still laid out here from the page's own Material
/// primitives rather than handed to the `dialog()` builder. The entrance used
/// to force that and no longer does; swapping in the builder is a composition
/// change with its own poster consequences, so it stays a separate question.
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
            OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
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
/// the same one-frame entrance-ramp limit documented above, reached here
/// through a widget with no `entrance`-style seam to opt out of.
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

/// The M3 bottom-sheet at-rest chrome, approximated via the `overlay_modal`
/// host. The genuine `bottom_sheet(..)` widget seeds at `progress = 0.0`, only
/// advances via spring on a later paint, and exposes no settled-entrance seam,
/// so this case uses [`OverlayModalConfig::edge(OverlaySide::Bottom)`] preset
/// chrome instead. This duplicates three material magic numbers: 28dp corner
/// radius, 32×4dp drag handle, and 48dp handle touch target. If
/// `BottomSheetWidget` drifts, this approximation becomes stale — the drift
/// risk is documented as a known limitation.
///
/// The entrance stays pinned to [`OverlayEntrance::None`]. Unlike
/// [`dialog_case`], a warm pass cannot stand in for the pin: the edge preset's
/// `Slide` is driven in layout, one pass behind the paint that advances it —
/// see the module docs' entrance-ramp section for the measured result.
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
/// `controller.show(..)` from an action). The bar itself is still off-screen on
/// this frame: the case pins no [`Case::time_ms`], so the recorder takes no
/// warm pass for it and the entrance ramp never advances past zero (see the
/// module docs' entrance-ramp section).
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
/// cannot bring on screen (entrance ramp limit — see module docs above);
/// `time_dial` is the non-modal ring that dialog wraps.
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
        time_ms: 400,
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
