//! The gallery's shell: the real `frust_shadcn::sidebar` family driving page
//! selection, plus the inset's own top bar.
//!
//! The nav is a `sidebar_provider` composition — groups, labels, menu items
//! with an active state, a badge, per-item and per-group actions, a nested
//! sub-menu, the header/footer slots, the rail, and `sidebar_trigger` in the
//! content's top bar. The panel is `SidebarCollapsible::Icon`, so collapsing it
//! leaves the 48px icon rail rather than removing it: every menu button keeps
//! its glyph and drops its label, and labels/badges/actions/sub-lists take zero
//! space.
//!
//! Ctrl/Cmd+B is handled by the provider *after* routing, and key events are
//! focus-routed, so the shortcut fires whenever nothing focused has taken the
//! chord first — which is why the top bar says so on screen.

use frust::{
    CrossAxisAlignment, EdgeInsets, IconSource, Padding, SizedBox, any, icon, icons, row, text,
};
use frust_shadcn::{
    SidebarCollapsible, SidebarMenuButtonSize, SidebarView, separator, sidebar, sidebar_content,
    sidebar_footer, sidebar_group, sidebar_group_action, sidebar_group_label, sidebar_header,
    sidebar_menu, sidebar_menu_action, sidebar_menu_badge, sidebar_menu_button, sidebar_menu_item,
    sidebar_menu_sub, sidebar_menu_sub_button, sidebar_menu_sub_item, sidebar_separator,
    sidebar_trigger,
};

use crate::AppState;

/// The ten gallery pages, in nav order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Primitives,
    Controls,
    InputsTable,
    Layout,
    Overlays,
    Anchored,
    Chat,
    Questionnaire,
    DataTable,
    Theming,
}

impl Page {
    /// The page's nav label, reused as the top bar's title.
    pub fn title(self) -> &'static str {
        match self {
            Page::Primitives => "Primitives",
            Page::Controls => "Controls",
            Page::InputsTable => "Inputs & Table",
            Page::Layout => "Layout",
            Page::Overlays => "Overlays",
            Page::Anchored => "Anchored",
            Page::Chat => "Chat",
            Page::Questionnaire => "Questionnaire",
            Page::DataTable => "Data Table",
            Page::Theming => "Theming",
        }
    }
}

/// One nav row: a menu button that selects `page`, lit when it is the active
/// one and carrying the glyph an icon-mode rail shows on its own.
fn nav_button(page: Page, active: Page, glyph: IconSource) -> frust::AnyView<AppState> {
    any(
        sidebar_menu_button(page.title(), move |s: &mut AppState| s.page = page)
            .icon(icon(glyph).size(16.0))
            .active(page == active),
    )
}

/// The nav panel: header, three groups, footer, rail.
pub fn shell_sidebar(state: &AppState) -> SidebarView<AppState> {
    let active = state.page;
    let message_count = state.chat.messages.len();

    let gallery = sidebar_group(vec![
        any(sidebar_group_label("Gallery")),
        sidebar_menu(vec![
            sidebar_menu_item(vec![nav_button(Page::Primitives, active, icons::STAR)]),
            sidebar_menu_item(vec![nav_button(Page::Controls, active, icons::SETTINGS)]),
            sidebar_menu_item(vec![nav_button(Page::InputsTable, active, icons::EDIT)]),
            sidebar_menu_item(vec![nav_button(Page::Layout, active, icons::PANE_MARK)]),
        ]),
    ]);

    // The one nested list: `Anchored` reads as a sub-section of `Overlays`, so
    // it is a `sidebar_menu_sub` under it rather than a fourth top-level row.
    let overlays = sidebar_group(vec![
        any(sidebar_group_label("Overlays")),
        sidebar_menu(vec![
            any(sidebar_menu_item(vec![nav_button(
                Page::Overlays,
                active,
                icons::SCAN_MARK,
            )])),
            any(sidebar_menu_sub(vec![sidebar_menu_sub_item(vec![
                sidebar_menu_sub_button(Page::Anchored.title(), |s: &mut AppState| {
                    s.page = Page::Anchored
                })
                .icon(icon(icons::PUSH_PIN).size(14.0))
                .active(active == Page::Anchored),
            ])])),
        ]),
    ]);

    let patterns = sidebar_group(vec![
        // `sidebar_menu_item` is the composed row this port gives a label plus
        // its trailing action (upstream positions the action absolutely).
        any(sidebar_menu_item(vec![
            any(sidebar_group_label("Patterns")),
            any(sidebar_group_action(
                icon(icons::REFRESH).size(14.0),
                "Reset the pattern demos",
                |s: &mut AppState| {
                    s.chat = Default::default();
                    s.questionnaire = Default::default();
                    s.data_table = Default::default();
                },
            )),
        ])),
        sidebar_menu(vec![
            sidebar_menu_item(vec![
                nav_button(Page::Chat, active, icons::FORUM),
                any(sidebar_menu_action(
                    icon(icons::DELETE).size(14.0),
                    "Clear the transcript",
                    |s: &mut AppState| s.chat.messages.clear(),
                )),
                any(sidebar_menu_badge(message_count.to_string())),
            ]),
            sidebar_menu_item(vec![nav_button(
                Page::Questionnaire,
                active,
                icons::DONE_ALL,
            )]),
            sidebar_menu_item(vec![nav_button(
                Page::DataTable,
                active,
                icons::DESCRIPTION,
            )]),
        ]),
    ]);

    sidebar(sidebar_content(vec![
        gallery,
        any(sidebar_separator()),
        overlays,
        any(sidebar_separator()),
        patterns,
    ]))
    .header(sidebar_header(vec![sidebar_menu(vec![sidebar_menu_item(
        vec![
            sidebar_menu_button("shadcn demo", |_: &mut AppState| {})
                .icon(icon(icons::PALETTE).size(18.0))
                .size(SidebarMenuButtonSize::Lg),
        ],
    )])]))
    .footer(sidebar_footer(vec![sidebar_menu(vec![sidebar_menu_item(
        vec![nav_button(Page::Theming, active, icons::LIGHT_MODE)],
    )])]))
    .collapsible(SidebarCollapsible::Icon)
    .rail(true)
}

/// The inset's top bar: the trigger, the active page's title, and the shortcut
/// hint, over a hairline.
pub fn top_bar(page: Page, open: bool) -> impl frust::View<AppState> + use<> {
    let bar = row()
        .child(sidebar_trigger(open, |s: &mut AppState, next: bool| {
            s.sidebar_open = next;
        }))
        .child(SizedBox::<AppState>(Some(12.0), None))
        .flex(1, text(page.title().to_string()).size(16.0))
        .child(text("Ctrl/Cmd+B toggles the panel \u{2014} or drag its rail").size(12.0))
        .cross_axis(CrossAxisAlignment::Center);

    frust::column()
        .child(Padding(
            EdgeInsets {
                left: 16.0,
                top: 10.0,
                right: 16.0,
                bottom: 10.0,
            },
            bar,
        ))
        .child(separator())
}

/// A section heading used at the top of every gallery page.
pub fn heading(title: &str) -> impl frust::View<AppState> + use<> {
    row().child(text(title.to_string()).size(24.0))
}

/// A page's small print: the caption style every gallery section explains
/// itself in.
pub fn caption(body: impl Into<String>) -> frust::TextView {
    text(body.into()).size(12.0)
}
