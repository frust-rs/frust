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
//! # Why three cases bypass `dialog()`/`drawer()`/`sheet()`
//!
//! `crates/frust-testing`'s recorder (`record_view`/`frame` in
//! `crates/frust-testing/src/frame.rs`) runs exactly one
//! rebuild/layout/paint pass — there is no second frame. Every modal-family
//! overlay (`dialog`, `drawer`, `sheet`) enters through
//! `frust_shadcn::overlay::modal::ModalWidget`'s `fade-in-0 zoom-in-95` (or
//! slide) ramp, and that ramp is *seeded and read inside the same paint
//! call*: `ModalWidget::paint` calls `AnimationController::forward()` and
//! then immediately `advance(now)` in the same pass, and `advance`'s very
//! first call after `forward()` always measures a zero elapsed delta
//! (`last_time` was just reset to `None`) regardless of how large a
//! `Case::time_ms` is fed in — the ramp only ever moves on a *second* paint,
//! which a single-shot recorder never takes. So on a fresh build every one of
//! these three panels would record at progress `0.0`: fully transparent,
//! scrim and panel both invisible. `ModalConfig` exposes exactly one public
//! escape hatch for this — `ModalEntrance::None` ("No entrance: the panel is
//! at rest on its first frame", `ModalWidget::build`'s `settled` flag seeds
//! `progress: 1.0` immediately) — but the sugared `dialog()`/`drawer()`/
//! `sheet()` constructors bake `FadeZoom`/`Slide` in and expose no builder to
//! override it. So these three cases call `frust_shadcn::overlay::modal`
//! directly with the *exact* chrome each component's own (private) `config()`
//! builds — same `ModalConfig::centered`/`ModalConfig::edge` call, same
//! corners/shadow/handle/drag/extent, same public `stack_slots` panel
//! layout, same `dialog_header`/`drawer_header`/`sheet_header`-family slot
//! helpers — with only the entrance swapped to `None`. Nothing here is
//! invented chrome; it is `dialog()`/`drawer()`/`sheet()`'s own composition,
//! open on its first (only) frame instead of stuck at zero.
//!
//! # The three cases that stay stuck (documented, not worked around)
//!
//! `dropdown-menu`, `select` and `tooltip` hit the identical
//! seed-and-read-in-one-paint trap, one layer down: their floating panels
//! (`frust_shadcn::components::popover::PanelWidget` behind `dropdown_menu`/
//! `select`, `TooltipLayerWidget` behind `tooltip`) run the same
//! `AnimationController::forward()`-then-`advance` dance, but neither
//! `PanelStyle::entrance` nor a `TooltipLayerWidget` ramp override is `pub`
//! outside `frust_shadcn` (`pub(crate)`) the way `ModalConfig::entrance` is.
//! There is no public seam to freeze either open on a single pass without
//! either adding one to the plugin crate (out of this task's write scope) or
//! forcing `Theme.motion.reduce_motion` (no per-subtree theme override
//! exists; the root theme is fixed once by `crate::theme::theme`, also out of
//! scope). These three cases still call the real, documented composition
//! (trigger + panel, panel pinned open where the component's own API allows
//! it), and a large `Case::time_ms` is still supplied per the task brief, but
//! the floating panel itself paints at effectively zero presence on the
//! recorded frame — only the trigger control is visually present. This is a
//! known, reported limitation, not a silent gap.

use frust_core::{AnyView, any};
use frust_shadcn::overlay::modal::{DRAWER_MAX_HEIGHT_FRACTION, MAX_WIDTH_LG};
use frust_shadcn::overlay::{
    HANDLE_RESERVE, ModalConfig, ModalCorners, ModalEntrance, ModalExtent, ModalLimit,
    OverlayAlign, OverlayAnchor, anchor, modal, stack_slots,
};
use frust_shadcn::{BubbleAlign, BubbleVariant, button};
use frust_shadcn::{
    ButtonVariant, DrawerSide, QuestionnaireShortcuts, SheetSide, SidebarCollapsible, SidebarSide,
    SidebarVariant, TooltipHover, bubble, card, card_content, card_footer, card_header, card_title,
    checkbox, dialog_description, dialog_footer, dialog_header, dialog_title, drawer_description,
    drawer_header, drawer_title, dropdown_menu, dropdown_menu_item, dropdown_menu_label,
    dropdown_menu_separator, field, input, label as shadcn_label, message, message_content,
    message_scroller, questionnaire, questionnaire_choice, questionnaire_item, select,
    select_option, select_trigger, sheet_description, sheet_footer, sheet_header, sheet_title,
    sidebar, sidebar_content, sidebar_group, sidebar_group_label, sidebar_header, sidebar_inset,
    sidebar_menu, sidebar_menu_button, sidebar_menu_item, sidebar_provider, table, table_cell,
    table_row, tabs, tabs_tab, tooltip, tooltip_trigger,
};
use frust_widgets::{
    Axis, Column, CrossAxisAlignment, EdgeInsets, FlexView, Row, SizedBox, icon, icons,
};
use kurbo::Size;

use crate::base::{framed, framed_in};
use crate::case::{Case, Design};

/// From `examples/shadcn-demo`'s primitives page — the variant row, wrapped
/// over two rows so the full variant set fits [`Case::DEFAULT_SIZE`]'s width.
fn button_case() -> AnyView<()> {
    framed(Column(vec![
        any(FlexView::new(
            Axis::Horizontal,
            vec![
                frust_widgets::inflexible(button("Default", |_: &mut ()| {})),
                frust_widgets::inflexible(SizedBox(Some(8.0), None)),
                frust_widgets::inflexible(
                    button("Secondary", |_: &mut ()| {}).variant(ButtonVariant::Secondary),
                ),
                frust_widgets::inflexible(SizedBox(Some(8.0), None)),
                frust_widgets::inflexible(
                    button("Outline", |_: &mut ()| {}).variant(ButtonVariant::Outline),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center)),
        any(SizedBox(None, Some(12.0))),
        any(FlexView::new(
            Axis::Horizontal,
            vec![
                frust_widgets::inflexible(
                    button("Destructive", |_: &mut ()| {}).variant(ButtonVariant::Destructive),
                ),
                frust_widgets::inflexible(SizedBox(Some(8.0), None)),
                frust_widgets::inflexible(
                    button("Link", |_: &mut ()| {}).variant(ButtonVariant::Link),
                ),
                frust_widgets::inflexible(SizedBox(Some(8.0), None)),
                frust_widgets::inflexible(button("Disabled", |_: &mut ()| {}).disabled(true)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center)),
    ]))
}

/// From `examples/shadcn-demo`'s layout page (a carousel slide card) and its
/// primitives page (a footer-actions card), stacked.
fn card_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 280.0),
        Column(vec![
            any(card(vec![
                card_header(vec![card_title("Slide 1")]),
                card_content(
                    frust_widgets::text("Drag the slide sideways, or use the outline arrows.")
                        .size(13.0),
                ),
            ])),
            any(SizedBox(None, Some(12.0))),
            any(card(vec![
                card_header(vec![card_title("Delete project?")]),
                card_footer(vec![
                    any(button("Cancel", |_: &mut ()| {}).variant(ButtonVariant::Ghost)),
                    any(SizedBox(Some(8.0), None)),
                    any(button("Save", |_: &mut ()| {})),
                ]),
            ])),
        ]),
    )
}

/// The dialog chrome, `dialog()`'s own composition
/// (`plugins/shadcn/src/components/dialog.rs`'s private `PANEL_PAD`/
/// `SLOT_GAP`/`config()`), entrance swapped to [`ModalEntrance::None`] — see
/// the module docs' "Why three cases bypass" section.
///
/// Content mirrors `examples/shadcn-demo`'s overlays page.
fn dialog_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 260.0),
        modal(
            stack_slots(
                vec![
                    dialog_header(vec![
                        dialog_title("Delete project?"),
                        dialog_description("This action cannot be undone."),
                    ]),
                    dialog_footer(vec![any(
                        button("Cancel", |_: &mut ()| {}).variant(ButtonVariant::Outline)
                    )]),
                ],
                16.0,
                EdgeInsets::all(24.0),
            ),
            ModalConfig::centered(MAX_WIDTH_LG)
                .close_button(true)
                .entrance(ModalEntrance::None),
        ),
    )
}

/// The bottom drawer chrome, `drawer()`'s own composition for
/// [`DrawerSide::Bottom`] (`plugins/shadcn/src/components/drawer.rs`'s
/// private `config()`/`content_pad()`), entrance swapped to
/// [`ModalEntrance::None`] — see the module docs' "Why three cases bypass"
/// section.
///
/// Content mirrors `examples/shadcn-demo`'s overlays page.
fn drawer_case() -> AnyView<()> {
    let side = DrawerSide::Bottom;
    framed_in(
        Size::new(360.0, 420.0),
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
    )
}

/// From `examples/shadcn-demo`'s anchored page. See the module docs' "The
/// three cases that stay stuck" section: the panel's own fade/zoom entrance
/// cannot be forced open on this recorder's single paint pass, so only the
/// trigger is visually present in the recorded frame.
fn dropdown_menu_case() -> AnyView<()> {
    let dropdown_anchor = OverlayAnchor::new();
    framed_in(
        Size::new(360.0, 260.0),
        frust_widgets::Stack(vec![
            any(anchor(&dropdown_anchor, button("Actions", |_: &mut ()| {}))),
            any(dropdown_menu(
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
            .anchor(&dropdown_anchor)),
        ]),
    )
}

/// From `examples/shadcn-demo`'s controls page (checkbox) and its inputs page
/// (a labelled, described `field` wrapping an `input`).
fn form_controls_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 220.0),
        Column(vec![
            any(Row(vec![
                any(checkbox(true, |_: &mut (), _: bool| {})),
                any(SizedBox(Some(8.0), None)),
                any(shadcn_label("Accept the terms")),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
            any(SizedBox(None, Some(20.0))),
            any(
                field(input("", |_: &mut (), _: String| {}).placeholder("Full name"))
                    .label("Name")
                    .description("Shown on your public profile."),
            ),
        ]),
    )
}

/// From `examples/shadcn-demo`'s chat composer, and its inputs page's
/// invalid-`field` example.
fn input_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 200.0),
        Column(vec![
            any(input("", |_: &mut (), _: String| {}).placeholder("Write a message\u{2026}")),
            any(SizedBox(None, Some(24.0))),
            any(field(
                input("", |_: &mut (), _: String| {})
                    .invalid(true)
                    .placeholder("Required"),
            )
            .label("Invalid example")
            .error("This field is required.")),
        ]),
    )
}

/// From `examples/shadcn-demo`'s chat page: a `message_scroller` over a
/// couple of `message` rows, one per side of the thread.
fn message_case() -> AnyView<()> {
    framed_in(
        Size::new(360.0, 260.0),
        message_scroller(vec![
            any(message(vec![any(message_content(vec![any(bubble(
                "Hey — got a minute to review the PR?",
            ))]))])),
            any(
                message(vec![any(message_content(vec![any(bubble("On it now.")
                    .variant(BubbleVariant::Secondary)
                    .align(BubbleAlign::End))]))])
                .align(frust_shadcn::MessageAlign::End),
            ),
        ]),
    )
}

/// From `examples/shadcn-demo`'s questionnaire page.
fn questionnaire_case() -> AnyView<()> {
    framed_in(
        Size::new(420.0, 320.0),
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
    )
}

/// From `examples/shadcn-demo`'s anchored page. See the module docs' "The
/// three cases that stay stuck" section: the list panel's own fade/zoom
/// entrance cannot be forced open on this recorder's single paint pass, so
/// only the trigger is visually present in the recorded frame.
fn select_case() -> AnyView<()> {
    let select_anchor = OverlayAnchor::new();
    let options = vec![
        select_option("Apple"),
        select_option("Banana"),
        select_option("Cherry"),
    ];
    framed_in(
        Size::new(360.0, 220.0),
        frust_widgets::Stack(vec![
            any(anchor(
                &select_anchor,
                select_trigger::<()>(&select_anchor, options.clone(), Some(2))
                    .placeholder("Pick a fruit"),
            )),
            any(select(options, Some(2), |_: &mut (), _: usize| {})
                .anchor(&select_anchor)
                .align(OverlayAlign::Start)),
        ]),
    )
}

/// The right-edge sheet chrome, `sheet()`'s own composition
/// (`plugins/shadcn/src/components/sheet.rs`'s private `config()`), entrance
/// swapped to [`ModalEntrance::None`] — see the module docs' "Why three cases
/// bypass" section.
///
/// Content mirrors `examples/shadcn-demo`'s overlays page.
fn sheet_case() -> AnyView<()> {
    let side = SheetSide::Right;
    framed_in(
        Size::new(360.0, 380.0),
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
                    sheet_footer(vec![any(frust_widgets::text(
                        "Escape, the scrim, or the X.",
                    ))]),
                ],
                16.0,
                EdgeInsets::all(0.0),
            ),
            ModalConfig::edge(side.overlay_side())
                .close_button(true)
                .entrance(ModalEntrance::None),
        ),
    )
}

/// From `examples/shadcn-demo`'s layout page: a floating, icon-collapsible
/// sidebar on the right, with its inset. `SidebarWidget::build` settles the
/// collapse animation directly at its target width on a fresh build (no
/// ramp-in-one-paint trap here — see `plugins/shadcn/src/components/
/// sidebar.rs`'s "A panel built collapsed starts collapsed rather than
/// animating shut"), so this case needs no entrance workaround.
fn sidebar_case() -> AnyView<()> {
    framed_in(
        Size::new(480.0, 320.0),
        sidebar_provider(
            sidebar(sidebar_content(vec![sidebar_group(vec![
                any(sidebar_group_label("Mail")),
                sidebar_menu(
                    ["Inbox", "Sent", "Drafts"]
                        .iter()
                        .enumerate()
                        .map(|(index, label)| {
                            any(sidebar_menu_item(vec![any(sidebar_menu_button(
                                *label,
                                |_: &mut ()| {},
                            )
                            .icon(icon(icons::FORUM).size(16.0))
                            .active(index == 0))]))
                        })
                        .collect(),
                ),
            ])]))
            .header(sidebar_header(vec![any(
                frust_widgets::text("Floating").size(14.0)
            )]))
            .side(SidebarSide::Right)
            .variant(SidebarVariant::Floating)
            .collapsible(SidebarCollapsible::Icon)
            .rail(true),
            sidebar_inset(vec![any(frust_widgets::text("Main content").size(14.0))]),
            true,
            |_: &mut (), _: bool| {},
        ),
    )
}

/// From `examples/shadcn-demo`'s data-table page: a selectable row and the
/// table's caption.
fn table_case() -> AnyView<()> {
    let rows = vec![
        table_row(vec![any(table_cell("Alex Kim")), any(table_cell("Admin"))]).selected(true),
        table_row(vec![any(table_cell("Sam Lee")), any(table_cell("Member"))]),
    ];
    framed_in(
        Size::new(420.0, 220.0),
        table(rows)
            .header(["Name", "Role"])
            .caption("1 of 2 row(s) selected \u{2014} page 1 of 1"),
    )
}

/// From `examples/shadcn-demo`'s controls page.
fn tabs_case() -> AnyView<()> {
    framed(tabs(
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
    ))
}

/// From `examples/shadcn-demo`'s anchored page. See the module docs' "The
/// three cases that stay stuck" section: the panel's own fade-in entrance
/// cannot be forced open on this recorder's single paint pass, so only the
/// trigger is visually present in the recorded frame — the hover latch is
/// still pre-opened (`TooltipHover::set_open`) for fidelity to the real
/// component wiring.
fn tooltip_case() -> AnyView<()> {
    let hover = TooltipHover::new();
    hover.set_open(true);
    framed(frust_widgets::Stack(vec![
        any(tooltip_trigger::<(), _>(
            &hover,
            button("Hover me", |_: &mut ()| {}),
        )),
        any(tooltip::<()>(&hover, "A tooltip, 700ms after rest")),
    ]))
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
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: card_case,
    },
    Case {
        slug: "shadcn/dialog",
        title: "Dialog",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: dialog_case,
    },
    Case {
        slug: "shadcn/drawer",
        title: "Drawer",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: drawer_case,
    },
    Case {
        slug: "shadcn/dropdown-menu",
        title: "Dropdown Menu",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: 300,
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
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: message_case,
    },
    Case {
        slug: "shadcn/questionnaire",
        title: "Questionnaire",
        size: Case::DEFAULT_SIZE,
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
        time_ms: 300,
        design: Design::Shadcn,
        build: select_case,
    },
    Case {
        slug: "shadcn/sheet",
        title: "Sheet",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: sheet_case,
    },
    Case {
        slug: "shadcn/sidebar",
        title: "Sidebar",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Shadcn,
        build: sidebar_case,
    },
    Case {
        slug: "shadcn/table",
        title: "Table",
        size: Case::DEFAULT_SIZE,
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
        time_ms: 300,
        design: Design::Shadcn,
        build: tooltip_case,
    },
];
