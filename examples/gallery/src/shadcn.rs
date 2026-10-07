//! `Shadcn`-design cases — one per non-index page of the website's
//! `widgets/design-systems/shadcn/*.mdx` set: `button`, `card`, `dialog`,
//! `drawer`, `dropdown-menu`, `form-controls`, `input`, `message`,
//! `questionnaire`, `select`, `sheet`, `sidebar`, `table`, `tabs`, `tooltip`
//! (slug `shadcn/<stem>`, [`Design::Shadcn`]). See the crate docs and
//! `README.md` for the pure-`View`/slug-rule contract every case in this
//! registry follows, and [`crate::base::framed`]/[`crate::base::framed_in`]
//! for why none of these cases fills its own backdrop.
//!
//! Every case is a static composition of the real `frust_shadcn` catalog
//! (`plugins/shadcn/src/components/*`), built the way
//! `examples/shadcn-demo`'s own pages call them — see each case function's
//! doc comment for which page it mirrors.
//!
//! # Why two cases bypass `drawer()`/`sheet()`
//!
//! `crates/frust-testing`'s recorder (`record_view`/`frame` in
//! `crates/frust-testing/src/frame.rs`) captures one rebuild/layout/paint pass,
//! preceded by one DISCARDED warm pass when the case pins a non-zero
//! [`Case::time_ms`] (`Case::warm_frames` derives the count from that field).
//! The warm pass is what makes a modal recordable at all. Every modal-family
//! overlay (`dialog`, `drawer`, `sheet`) enters through
//! `frust_shadcn::overlay::modal::ModalWidget`'s `fade-in-0 zoom-in-95` (or
//! slide) ramp, and that ramp is *seeded and read inside the same paint call*:
//! `ModalWidget::paint` calls `AnimationController::forward()` and then
//! immediately `advance(now)` in the same pass, and `advance`'s very first call
//! after `forward()` always measures a zero elapsed delta (`last_time` was just
//! reset to `None`) regardless of how large a `time_ms` is fed in. The warm
//! pass supplies that first `advance`, leaving the captured one to see the
//! whole of `time_ms` as its delta; with no warm pass the panel records at
//! progress `0.0` — fully transparent, scrim and panel both invisible.
//!
//! For `dialog` that is the whole story. `ModalEntrance::FadeZoom` is
//! composited in paint, so `dialog_case` calls the plain sugared `dialog()`,
//! bypasses nothing, and pins `time_ms: 400` — past the 200 ms ramp
//! (`FADE_ZOOM_MS`) — and the poster records the settled panel it always
//! recorded, byte for byte.
//!
//! What that does **not** change is the browser: a live host resolves
//! `find_interactive(slug).unwrap_or(case.build)`
//! (`examples/web-gallery/src/main.rs`'s `case_constructor`), and
//! [`crate::interactive::shadcn`] shadows `shadcn/dialog` with a stateful twin
//! that already calls the sugared `dialog()`. So the entrance restored here is
//! the RECORDER's and any table-less host's, not `examples/web-gallery`'s —
//! which was already animating this case before this constructor was.
//!
//! `drawer` and `sheet` cannot take that route and still bypass their sugared
//! constructors. `ModalEntrance::Slide` — what `ModalConfig::edge` defaults to
//! — moves the panel in *layout*, and `ModalWidget::layout` reads the
//! `progress` the *previous* paint left behind. With the single warm pass the
//! registry derives, the captured pass therefore lays the panel out at progress
//! 0, off its own edge, and then paints it there at the full opacity its
//! now-settled ramp asks for: a poster that is scrim and nothing else
//! (measured, both variants). `ModalConfig` exposes exactly one public escape
//! hatch — `ModalEntrance::None` ("No entrance: the panel is at rest on its
//! first frame", `ModalWidget::build`'s `settled` flag seeds `progress: 1.0`
//! immediately) — but the sugared `drawer()`/`sheet()` constructors bake
//! `Slide` in and expose no builder to override it. So these two cases call
//! `frust_shadcn::overlay::modal` directly with the *exact* chrome each
//! component's own (private) `config()` builds — same `ModalConfig::edge` call,
//! same corners/shadow/handle/drag/extent, same public `stack_slots` panel
//! layout, same `drawer_header`/`sheet_header`-family slot helpers — with only
//! the entrance swapped to `None`. Nothing here is invented chrome; it is
//! `drawer()`/`sheet()`'s own composition, open on its first frame instead of
//! stuck at zero.
//!
//! What that pin costs is this module's own fidelity, and — unlike material's
//! sheet, which has no such twin — not the browser:
//! [`crate::interactive::shadcn`] shadows `shadcn/drawer` and `shadcn/sheet`
//! too, with stateful twins that call the sugared `drawer()`/`sheet()` and take
//! whatever entrance those constructors ship, and a live host resolves the twin
//! ahead of `Case::build`.
//! Measured in headless Chrome over a screencast of the page load: both twins
//! slide in over roughly 500 ms, across ~30 changing frames. Two warm passes
//! would let the pin come off here as well at no cost to the poster — but the
//! count is only half of that condition. `FrameSpec::warm_time` spaces the warm
//! schedule across `time_ms` instead of stepping it by the ramp's duration, so
//! the LAST warm pass lands at `time_ms` × (count − 1) / count, and it is that
//! pass, not the capture, that has to reach the 500 ms `Slide` for the
//! capture's own layout to read a settled `progress`. Measured at count 2:
//! `time_ms: 800` moves these posters, `1_000` and `1_200` leave them
//! byte-identical. Two passes therefore want `time_ms` ≥ 2 × duration, not
//! merely a count of two.
//!
//! # Three cases the RECORDER cannot open — they animate fine live
//!
//! `dropdown-menu`, `select` and `tooltip` hit the identical
//! seed-and-read-in-one-paint trap, one layer down: their floating panels
//! (`frust_shadcn::components::popover::PanelWidget` behind `dropdown_menu`/
//! `select`, `TooltipLayerWidget` behind `tooltip`) run the same
//! `AnimationController::forward()`-then-`advance` dance, but neither
//! `PanelStyle::entrance` nor a `TooltipLayerWidget` ramp override is `pub`
//! outside `frust_shadcn` (`pub(crate)`) the way `ModalConfig::entrance` is —
//! there is no `None` to pin them at. These three cases still call the real,
//! documented composition (trigger + panel, panel pinned open where the
//! component's own API allows it), and none of them pins a `Case::time_ms`, so
//! the floating panel paints at effectively zero presence on the recorded frame
//! — only the trigger control is visually present.
//!
//! Two things once written here are no longer true. A large `Case::time_ms` is
//! **not** supplied for these three: each carried `time_ms: 300` until
//! `c2a2b10b` ("drop decorative time_ms") removed it, precisely because it did
//! nothing. And the warm pass **does** now reach them — pinning a `time_ms`
//! records all three panels open (measured). What each of those posters should
//! then show is a composition question for the case itself, not a rider on the
//! entrance work, so all three stay at `Case::DEFAULT_TIME_MS` here.
//!
//! None of this is a LIVE limitation. `PanelWidget::ramp`
//! (`plugins/shadcn/src/components/popover.rs`) and `TooltipLayerWidget::ramp`
//! (`plugins/shadcn/src/components/tooltip.rs`, called from that widget's own
//! `paint`) both call `ctx.request_frame()` while their presence ramp is
//! running, so in a browser these entrances play out over many frames exactly
//! as the catalog documents. `TooltipTriggerWidget::paint` asks for frames too,
//! but for the hover *delay* (`HoverPhase::Opening`/`Closing`) and the latch
//! flip, not for the ramp — the 700 ms wait and the fade are two different
//! clocks, and only the recorder is missing both. Stuck here means stuck in the
//! RECORDER.

use frust_core::{AnyView, any};
use frust_shadcn::overlay::modal::DRAWER_MAX_HEIGHT_FRACTION;
use frust_shadcn::overlay::{
    HANDLE_RESERVE, ModalConfig, ModalCorners, ModalEntrance, ModalExtent, ModalLimit,
    OverlayAlign, OverlayAnchor, anchor, modal, stack_slots,
};
use frust_shadcn::{BubbleAlign, BubbleVariant, button};
use frust_shadcn::{
    ButtonVariant, DrawerSide, QuestionnaireShortcuts, SheetSide, SidebarCollapsible, SidebarSide,
    SidebarVariant, TooltipHover, bubble, card, card_content, card_footer, card_header, card_title,
    checkbox, dialog, dialog_description, dialog_footer, dialog_header, dialog_title,
    drawer_description, drawer_header, drawer_title, dropdown_menu, dropdown_menu_item,
    dropdown_menu_label, dropdown_menu_separator, field, input, label as shadcn_label, message,
    message_content, message_scroller, questionnaire, questionnaire_choice, questionnaire_item,
    select, select_option, select_trigger, sheet_description, sheet_footer, sheet_header,
    sheet_title, sidebar, sidebar_content, sidebar_group, sidebar_group_label, sidebar_header,
    sidebar_inset, sidebar_menu, sidebar_menu_button, sidebar_menu_item, sidebar_provider, table,
    table_cell, table_row, tabs, tabs_tab, tooltip, tooltip_trigger,
};
use frust_widgets::{CrossAxisAlignment, EdgeInsets, SizedBox, column, icon, icons, row};
use kurbo::Size;

use crate::base::{framed, framed_in};
use crate::case::{Case, Design};

/// A wider card frame: two stacked cards or a layout component that needs more
/// horizontal space.
const CARD_WIDE: Size = Size::new(360.0, 280.0);
/// A dialog or dropdown panel frame.
const DIALOG: Size = Size::new(360.0, 260.0);
/// A bottom drawer that extends taller to show interaction affordances.
const DRAWER_TALL: Size = Size::new(360.0, 420.0);
/// A dropdown menu or message thread preview.
const DROPDOWN: Size = Size::new(360.0, 260.0);
/// A questionnaire or form layout that needs extra height and width.
const QUESTIONNAIRE: Size = Size::new(420.0, 320.0);
/// A right-edge sheet that extends vertically.
const SHEET_TALL: Size = Size::new(360.0, 380.0);
/// A floating sidebar with main content area.
const SIDEBAR_WIDE: Size = Size::new(480.0, 320.0);
/// A data table that needs horizontal space for columns.
const TABLE_WIDE: Size = Size::new(420.0, 220.0);

/// From `examples/shadcn-demo`'s primitives page — the variant row, wrapped
/// over two rows so the full variant set fits [`Case::DEFAULT_SIZE`]'s width.
fn button_case() -> AnyView<()> {
    any(framed(
        column()
            .child(
                row()
                    .child(button("Default", |_: &mut ()| {}))
                    .child(SizedBox(Some(8.0), None))
                    .child(button("Secondary", |_: &mut ()| {}).variant(ButtonVariant::Secondary))
                    .child(SizedBox(Some(8.0), None))
                    .child(button("Outline", |_: &mut ()| {}).variant(ButtonVariant::Outline))
                    .cross_axis(CrossAxisAlignment::Center),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(
                row()
                    .child(
                        button("Destructive", |_: &mut ()| {}).variant(ButtonVariant::Destructive),
                    )
                    .child(SizedBox(Some(8.0), None))
                    .child(button("Link", |_: &mut ()| {}).variant(ButtonVariant::Link))
                    .child(SizedBox(Some(8.0), None))
                    .child(button("Disabled", |_: &mut ()| {}).disabled(true))
                    .cross_axis(CrossAxisAlignment::Center),
            ),
    ))
}

/// From `examples/shadcn-demo`'s layout page (a carousel slide card) and its
/// primitives page (a footer-actions card), stacked.
fn card_case() -> AnyView<()> {
    any(framed_in(
        CARD_WIDE,
        column()
            .child(card(vec![
                card_header(vec![card_title("Slide 1")]),
                card_content(
                    frust_widgets::text("Drag the slide sideways, or use the outline arrows.")
                        .size(13.0),
                ),
            ]))
            .child(SizedBox(None, Some(12.0)))
            .child(card(vec![
                card_header(vec![card_title("Delete project?")]),
                card_footer((
                    button("Cancel", |_: &mut ()| {}).variant(ButtonVariant::Ghost),
                    SizedBox(Some(8.0), None),
                    button("Save", |_: &mut ()| {}),
                )),
            ])),
    ))
}

/// The real `dialog()`, sugared constructor and all: the panel fades and zooms
/// in as the catalog documents, and `time_ms: 400` is what lands the poster on
/// the settled frame — see the module docs' "Why two cases bypass" section for
/// why this case is no longer one of them.
///
/// Content mirrors `examples/shadcn-demo`'s overlays page.
fn dialog_case() -> AnyView<()> {
    any(framed_in(
        DIALOG,
        dialog(vec![
            dialog_header(vec![
                dialog_title("Delete project?"),
                dialog_description("This action cannot be undone."),
            ]),
            dialog_footer(vec![
                button("Cancel", |_: &mut ()| {}).variant(ButtonVariant::Outline),
            ]),
        ]),
    ))
}

/// The bottom drawer chrome, `drawer()`'s own composition for
/// [`DrawerSide::Bottom`] (`plugins/shadcn/src/components/drawer.rs`'s private
/// `config()`/`content_pad()`), entrance swapped to [`ModalEntrance::None`]
/// because `Slide` is driven in layout, one pass behind the paint that advances
/// it — see the module docs' "Why two cases bypass" section.
///
/// Content mirrors `examples/shadcn-demo`'s overlays page.
fn drawer_case() -> AnyView<()> {
    let side = DrawerSide::Bottom;
    any(framed_in(
        DRAWER_TALL,
        modal(
            stack_slots(
                vec![drawer_header(vec![
                    drawer_title("Bottom drawer"),
                    drawer_description(
                        "Drag the panel toward its edge: past the halfway point (or with a \
                         flick) it keeps going and closes; short of it, it springs back open.",
                    ),
                ])],
                0.0,
                EdgeInsets {
                    left: 0.0,
                    top: HANDLE_RESERVE,
                    right: 0.0,
                    bottom: 0.0,
                },
            ),
            ModalConfig::edge(side.overlay_side())
                .corners(ModalCorners::Top)
                .shadow(None)
                .handle(side.has_handle())
                .drag(true)
                .extent(
                    ModalExtent::Content,
                    ModalLimit::Fraction(DRAWER_MAX_HEIGHT_FRACTION),
                )
                .entrance(ModalEntrance::None),
        ),
    ))
}

/// From `examples/shadcn-demo`'s anchored page. See the module docs' "Three
/// cases the RECORDER cannot open" section: this case pins no `Case::time_ms`,
/// so the panel's own fade/zoom entrance records at zero presence and only the
/// trigger is visually present in the recorded frame. A live host opens it on
/// interaction instead, entrance and all — see [`crate::interactive::shadcn`].
fn dropdown_menu_case() -> AnyView<()> {
    let dropdown_anchor = OverlayAnchor::new();
    any(framed_in(
        DROPDOWN,
        frust_widgets::stack()
            .child(anchor(&dropdown_anchor, button("Actions", |_: &mut ()| {})))
            .child(
                dropdown_menu(
                    vec![
                        dropdown_menu_label("Actions"),
                        dropdown_menu_item("Bold")
                            .checked(true)
                            .shortcut("\u{2318}B"),
                        dropdown_menu_item("Italic").shortcut("\u{2318}I"),
                        dropdown_menu_separator(),
                        dropdown_menu_item("Disabled row").disabled(true),
                    ],
                    |_: &mut (), _: usize| {},
                )
                .anchor(&dropdown_anchor),
            ),
    ))
}

/// From `examples/shadcn-demo`'s controls page (checkbox) and its inputs page
/// (a labelled, described `field` wrapping an `input`).
fn form_controls_case() -> AnyView<()> {
    any(framed_in(
        Size::new(360.0, 220.0),
        column()
            .child(
                row()
                    .child(checkbox(true, |_: &mut (), _: bool| {}))
                    .child(SizedBox(Some(8.0), None))
                    .child(shadcn_label("Accept the terms"))
                    .cross_axis(CrossAxisAlignment::Center),
            )
            .child(SizedBox(None, Some(20.0)))
            .child(
                field(input("", |_: &mut (), _: String| {}).placeholder("Full name"))
                    .label("Name")
                    .description("Shown on your public profile."),
            ),
    ))
}

/// From `examples/shadcn-demo`'s chat composer, and its inputs page's
/// invalid-`field` example.
fn input_case() -> AnyView<()> {
    any(framed_in(
        Size::new(360.0, 200.0),
        column()
            .child(input("", |_: &mut (), _: String| {}).placeholder("Write a message\u{2026}"))
            .child(SizedBox(None, Some(24.0)))
            .child(
                field(
                    input("", |_: &mut (), _: String| {})
                        .invalid(true)
                        .placeholder("Required"),
                )
                .label("Invalid example")
                .error("This field is required."),
            ),
    ))
}

/// From `examples/shadcn-demo`'s chat page: a `message_scroller` over a
/// couple of `message` rows, one per side of the thread.
fn message_case() -> AnyView<()> {
    any(framed_in(
        DROPDOWN,
        message_scroller((
            message(vec![message_content(vec![bubble(
                "Hey — got a minute to review the PR?",
            )])]),
            message(vec![message_content(vec![
                bubble("On it now.")
                    .variant(BubbleVariant::Secondary)
                    .align(BubbleAlign::End),
            ])])
            .align(frust_shadcn::MessageAlign::End),
        )),
    ))
}

/// From `examples/shadcn-demo`'s questionnaire page.
fn questionnaire_case() -> AnyView<()> {
    any(framed_in(
        QUESTIONNAIRE,
        questionnaire(
            vec![
                questionnaire_item("framework", "Which framework brought you here?").choices(vec![
                    questionnaire_choice("frust", "Frust"),
                    questionnaire_choice("flutter", "Flutter"),
                    questionnaire_choice("other", "Something else"),
                ]),
            ],
            0,
            vec![Default::default()],
        )
        .shortcuts(QuestionnaireShortcuts::Letters)
        .on_answer(|_: &mut (), _| {})
        .on_navigate(|_: &mut (), _| {})
        .on_submit(|_: &mut ()| {}),
    ))
}

/// From `examples/shadcn-demo`'s anchored page. See the module docs' "Three
/// cases the RECORDER cannot open" section: this case pins no `Case::time_ms`,
/// so the list panel's own fade/zoom entrance records at zero presence and only
/// the trigger is visually present in the recorded frame. A live host opens it
/// on interaction instead, entrance and all — see
/// [`crate::interactive::shadcn`].
fn select_case() -> AnyView<()> {
    let select_anchor = OverlayAnchor::new();
    let options = vec![
        select_option("Apple"),
        select_option("Banana"),
        select_option("Cherry"),
    ];
    any(framed_in(
        Size::new(360.0, 220.0),
        frust_widgets::stack()
            .child(anchor(
                &select_anchor,
                select_trigger::<()>(&select_anchor, options.clone(), Some(2))
                    .placeholder("Pick a fruit"),
            ))
            .child(
                select(options, Some(2), |_: &mut (), _: usize| {})
                    .anchor(&select_anchor)
                    .align(OverlayAlign::Start),
            ),
    ))
}

/// The right-edge sheet chrome, `sheet()`'s own composition
/// (`plugins/shadcn/src/components/sheet.rs`'s private `config()`), entrance
/// swapped to [`ModalEntrance::None`] because `Slide` is driven in layout, one
/// pass behind the paint that advances it — see the module docs' "Why two cases
/// bypass" section.
///
/// Content mirrors `examples/shadcn-demo`'s overlays page.
fn sheet_case() -> AnyView<()> {
    let side = SheetSide::Right;
    any(framed_in(
        SHEET_TALL,
        modal(
            stack_slots(
                vec![
                    sheet_header(vec![
                        sheet_title("Right sheet"),
                        sheet_description(
                            "Dismiss it and watch the panel slide back out the edge it came \
                             from before the page pops.",
                        ),
                    ]),
                    sheet_footer(vec![frust_widgets::text("Escape, the scrim, or the X.")]),
                ],
                16.0,
                EdgeInsets::all(0.0),
            ),
            ModalConfig::edge(side.overlay_side())
                .close_button(true)
                .entrance(ModalEntrance::None),
        ),
    ))
}

/// From `examples/shadcn-demo`'s layout page: a floating, icon-collapsible
/// sidebar on the right, with its inset. `SidebarWidget::build` settles the
/// collapse animation directly at its target width on a fresh build (no
/// ramp-in-one-paint trap here — see `plugins/shadcn/src/components/
/// sidebar.rs`'s "A panel built collapsed starts collapsed rather than
/// animating shut"), so this case needs no entrance workaround.
fn sidebar_case() -> AnyView<()> {
    any(framed_in(
        SIDEBAR_WIDE,
        sidebar_provider(
            sidebar(sidebar_content(vec![sidebar_group(vec![
                any(sidebar_group_label("Mail")),
                sidebar_menu(
                    ["Inbox", "Sent", "Drafts"]
                        .iter()
                        .enumerate()
                        .map(|(index, label)| {
                            any(sidebar_menu_item(vec![
                                sidebar_menu_button(*label, |_: &mut ()| {})
                                    .icon(icon(icons::FORUM).size(16.0))
                                    .active(index == 0),
                            ]))
                        })
                        .collect::<Vec<_>>(),
                ),
            ])]))
            .header(sidebar_header(vec![
                frust_widgets::text("Floating").size(14.0),
            ]))
            .side(SidebarSide::Right)
            .variant(SidebarVariant::Floating)
            .collapsible(SidebarCollapsible::Icon)
            .rail(true),
            sidebar_inset(vec![frust_widgets::text("Main content").size(14.0)]),
            true,
            |_: &mut (), _: bool| {},
        ),
    ))
}

/// From `examples/shadcn-demo`'s data-table page: a selectable row and the
/// table's caption.
fn table_case() -> AnyView<()> {
    let rows = vec![
        table_row(vec![any(table_cell("Alex Kim")), any(table_cell("Admin"))]).selected(true),
        table_row(vec![any(table_cell("Sam Lee")), any(table_cell("Member"))]),
    ];
    any(framed_in(
        TABLE_WIDE,
        table(rows)
            .header(["Name", "Role"])
            .caption("1 of 2 row(s) selected \u{2014} page 1 of 1"),
    ))
}

/// From `examples/shadcn-demo`'s controls page.
fn tabs_case() -> AnyView<()> {
    any(framed(tabs(
        "account",
        vec![
            tabs_tab(
                "account",
                "Account",
                frust_widgets::text("Account settings go here."),
            ),
            tabs_tab(
                "password",
                "Password",
                frust_widgets::text("Password settings go here."),
            ),
        ],
        |_: &mut (), _: String| {},
    )))
}

/// From `examples/shadcn-demo`'s anchored page. See the module docs' "Three
/// cases the RECORDER cannot open" section: this case pins no `Case::time_ms`,
/// so the panel's own fade-in entrance records at zero presence and only the
/// trigger is visually present in the recorded frame — the hover latch is still
/// pre-opened (`TooltipHover::set_open`) for fidelity to the real component
/// wiring. A live host opens it on interaction instead, entrance and all — see
/// [`crate::interactive::shadcn`].
fn tooltip_case() -> AnyView<()> {
    let hover = TooltipHover::new();
    hover.set_open(true);
    any(framed(
        frust_widgets::stack()
            .child(tooltip_trigger::<(), _>(
                &hover,
                button("Hover me", |_: &mut ()| {}),
            ))
            .child(tooltip::<()>(&hover, "A tooltip, 700ms after rest")),
    ))
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "shadcn/button",
        title: "Button",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: button_case,
    },
    Case {
        slug: "shadcn/card",
        title: "Card",
        size: CARD_WIDE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: card_case,
    },
    Case {
        slug: "shadcn/dialog",
        title: "Dialog",
        size: DIALOG,
        scale: Case::DEFAULT_SCALE,
        time_ms: 400,
        design: Design::Shadcn,
        build: dialog_case,
    },
    Case {
        slug: "shadcn/drawer",
        title: "Drawer",
        size: DRAWER_TALL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: drawer_case,
    },
    Case {
        slug: "shadcn/dropdown-menu",
        title: "Dropdown Menu",
        size: DROPDOWN,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: dropdown_menu_case,
    },
    Case {
        slug: "shadcn/form-controls",
        title: "Form Controls",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: form_controls_case,
    },
    Case {
        slug: "shadcn/input",
        title: "Input",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: input_case,
    },
    Case {
        slug: "shadcn/message",
        title: "Message",
        size: DROPDOWN,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: message_case,
    },
    Case {
        slug: "shadcn/questionnaire",
        title: "Questionnaire",
        size: QUESTIONNAIRE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: questionnaire_case,
    },
    Case {
        slug: "shadcn/select",
        title: "Select",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: select_case,
    },
    Case {
        slug: "shadcn/sheet",
        title: "Sheet",
        size: SHEET_TALL,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: sheet_case,
    },
    Case {
        slug: "shadcn/sidebar",
        title: "Sidebar",
        size: SIDEBAR_WIDE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: sidebar_case,
    },
    Case {
        slug: "shadcn/table",
        title: "Table",
        size: TABLE_WIDE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: table_case,
    },
    Case {
        slug: "shadcn/tabs",
        title: "Tabs",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: tabs_case,
    },
    Case {
        slug: "shadcn/tooltip",
        title: "Tooltip",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: tooltip_case,
    },
];
