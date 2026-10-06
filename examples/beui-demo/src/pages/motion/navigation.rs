//! Motion · Navigation: the catalog's destination surfaces — [`tabs`] in all
//! three variants, the magnifying [`dock`], both sidebars, the [`file_tree`],
//! the [`bouncy_accordion`] and the [`preview_rail`].
//!
//! State lives in a [`frust::component`], as on every Motion page (see
//! `crate::pages::motion::text`), which is what makes each of these genuinely
//! controlled here: the indicator pill, the dot, the selection pill and the
//! open panel all follow values this page owns.
//!
//! Two of these fill whatever height they are handed (the sidebar rail, the
//! preview rail), and the gallery shell's scroll view hands a page an
//! unbounded one — so those sit inside a [`frust::SizedBox`] with a demo-sized
//! height rather than an app-sized one.
//!
//! Sample data is upstream's own preview copy: the three tab sets, the dock's
//! five apps plus its separator group, the five nav destinations, the
//! `app/components` project tree, the six-panel release accordion, and the
//! documentation rail.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, SizedBox, View, any, column, component, icon,
    icons, row, text,
};
use frust_beui::components::animated_sidebar::{animated_sidebar, sidebar_item};
use frust_beui::components::bounce_sidebar::{bounce_sidebar, bounce_sidebar_item};
use frust_beui::components::bouncy_accordion::{bouncy_accordion, bouncy_accordion_item};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::dock::{dock, dock_item, dock_separator};
use frust_beui::components::file_tree::{file_tree, file_tree_file, file_tree_folder};
use frust_beui::components::preview_rail::{
    PreviewRailOrientation, preview_rail, preview_rail_item,
};
use frust_beui::components::tabs::{TabsVariant, tabs, tabs_tab};

use crate::AppState;
use crate::nav::{caption, heading};

/// The dock's entries — upstream's five apps, a separator, then settings and a
/// link.
const DOCK_ITEMS: [&str; 5] = ["Home", "Mail", "Calendar", "Music", "Discover"];

/// The two sidebars' destinations — upstream's own five.
const DESTINATIONS: [&str; 5] = ["Overview", "Components", "Motion", "Templates", "Changelog"];

/// The accordion's panels, upstream's release queue.
const PANELS: [(&str, &str, &str); 6] = [
    (
        "brief",
        "Release Brief",
        "Collect launch notes, owners, and risks in one compact handoff before \
         the release window opens.",
    ),
    (
        "launch",
        "Launch Checklist",
        "Verify copy, links, analytics, rollback steps, and final approvals \
         without leaving the queue.",
    ),
    (
        "campaign",
        "Campaign Notes",
        "Keep channel-specific notes close to the task while preserving a calm \
         collapsed list.",
    ),
    (
        "calendar",
        "Rollout Calendar",
        "Plan announcements, staging checks, reminders, and quiet periods \
         around the same timeline.",
    ),
    (
        "ship",
        "Ship Build",
        "Track the current artifact, deploy status, and final sign-off before \
         marking the release complete.",
    ),
    (
        "archive",
        "Archive Assets",
        "Move final copy, images, and source files into the campaign folder \
         once the rollout is done.",
    ),
];

/// The preview rail's sections.
const SECTIONS: [(&str, &str); 6] = [
    (
        "Dashboard",
        "Return to your workspace overview and recent activity.",
    ),
    (
        "Components",
        "Browse the motion primitives this crate ports.",
    ),
    (
        "Blocks",
        "Explore composed, product-ready interface blocks.",
    ),
    (
        "Playground",
        "Tune motion values and preview behaviour live.",
    ),
    (
        "Documentation",
        "Read installation, usage, and API reference notes.",
    ),
    ("Changelog", "Review newly ported components and fixes."),
];

/// This page's retained selections.
pub struct State {
    pill_tab: String,
    segment_tab: String,
    underline_tab: String,
    dock_active: usize,
    dock_magnify: bool,
    sidebar_open: bool,
    sidebar_active: usize,
    bounce_active: usize,
    tree_selected: String,
    tree_expanded: Vec<String>,
    panel: Option<String>,
    rail_active: usize,
    rail_horizontal_active: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            pill_tab: "overview".to_string(),
            segment_tab: "week".to_string(),
            underline_tab: "all".to_string(),
            dock_active: 0,
            dock_magnify: true,
            sidebar_open: true,
            sidebar_active: 1,
            bounce_active: 1,
            tree_selected: "file-tree".to_string(),
            tree_expanded: vec!["app".to_string(), "components".to_string()],
            panel: Some("calendar".to_string()),
            rail_active: 1,
            rail_horizontal_active: 3,
        }
    }
}

/// A vertical spacer.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// One component's block: its name, a one-line note (where a ported
/// degradation is stated), and the live instances.
fn demo(title: &str, note: &str, body: Vec<AnyView<State>>) -> AnyView<State> {
    let mut children = vec![
        any(text(title.to_string()).size(16.0)),
        gap(4.0),
        any(caption(note.to_string())),
        gap(12.0),
    ];
    children.extend(body);
    children.push(gap(32.0));
    any(Column(children).cross_axis(CrossAxisAlignment::Start))
}

/// A labelled specimen: the caption above the live component.
fn labelled(label: &str, body: AnyView<State>) -> AnyView<State> {
    any(column()
        .child(caption(label.to_string()))
        .child(gap(8.0))
        .child(body)
        .cross_axis(CrossAxisAlignment::Start))
}

/// A small outline button — this page's knobs.
fn knob(label: &str, on_press: impl Fn(&mut State) + 'static) -> AnyView<State> {
    any(button(label.to_string(), on_press)
        .tone(ButtonTone::Outline)
        .size(ButtonSize::Sm))
}

/// [`tabs`] in all three variants.
fn tab_sets(state: &State) -> AnyView<State> {
    let pill = any(tabs(
        state.pill_tab.clone(),
        vec![
            tabs_tab(
                "overview",
                "Overview",
                caption("High-level summary of the release."),
            ),
            tabs_tab("activity", "Activity", caption("Recent events.")),
            tabs_tab("settings", "Settings", caption("Preferences.")),
        ],
        |s: &mut State, v: String| s.pill_tab = v,
    )
    .variant(TabsVariant::Pill));

    let segment = any(tabs(
        state.segment_tab.clone(),
        vec![
            tabs_tab("day", "Day", caption("24 hours of events.")),
            tabs_tab("week", "Week", caption("Seven days of events.")),
            tabs_tab("month", "Month", caption("A calendar month.")),
        ],
        |s: &mut State, v: String| s.segment_tab = v,
    )
    .variant(TabsVariant::Segment));

    let underline = any(tabs(
        state.underline_tab.clone(),
        vec![
            tabs_tab("all", "All", caption("Every issue.")),
            tabs_tab("open", "Open", caption("Still open.")),
            tabs_tab("closed", "Closed", caption("Already closed.")),
            tabs_tab("archived", "Archived", caption("Archived.")).disabled(true),
        ],
        |s: &mut State, v: String| s.underline_tab = v,
    )
    .variant(TabsVariant::Underline));

    demo(
        "tabs",
        "The indicator is a rect spring, not a cross-fade: it remembers the \
         rect it was showing and springs x and width onto the new trigger's, so \
         a wide tab stretches into a narrow one. It runs on upstream's own \
         weightier inline spring rather than the shared layout token, because \
         that overshoot is this component's character. Arrow keys move focus \
         and activate; the disabled tab is a catalog convention upstream has \
         no equivalent for.",
        vec![any(row()
            .child(labelled(
                "Pill",
                any(SizedBox(Some(240.0), None).child(pill)),
            ))
            .child(hgap(20.0))
            .child(labelled(
                "Segment",
                any(SizedBox(Some(220.0), None).child(segment)),
            ))
            .child(hgap(20.0))
            .child(labelled(
                "Underline \u{b7} last tab disabled",
                any(SizedBox(Some(250.0), None).child(underline)),
            ))
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// The [`dock`], with and without its magnification.
fn docks(state: &State) -> AnyView<State> {
    let glyphs = [
        icons::HOME,
        icons::SEND,
        icons::SCHEDULE,
        icons::MIC,
        icons::STAR,
    ];

    let entries = |active: usize, live: bool| {
        let mut items: Vec<_> = DOCK_ITEMS
            .iter()
            .enumerate()
            .map(|(index, label)| {
                dock_item(icon(glyphs[index]).size(20.0))
                    .label(*label)
                    .active(live && index == active)
            })
            .collect();
        items.push(dock_separator());
        items.push(
            dock_item(icon(icons::SETTINGS).size(20.0))
                .label("Settings")
                .active(live && active == 6),
        );
        items.push(
            dock_item(icon(icons::LINK).size(20.0))
                .label("Disabled")
                .disabled(true),
        );
        items
    };

    let magnifying = any(
        dock(entries(state.dock_active, true), |s: &mut State, index| {
            s.dock_active = index;
        })
        .magnify(state.dock_magnify),
    );

    let plain = any(
        dock(entries(state.dock_active, false), |_: &mut State, _| {})
            .magnify(false)
            .size(36.0),
    );

    demo(
        "dock",
        "Upstream's bar is static \u{2014} its only motion is the shared active \
         pill's travel \u{2014} so the macOS-style magnification is an addition, \
         kept behind .magnify() with the plain bar on the right. Icons grow \
         upward about their bottom edge out of a baseline that never moves, \
         and the whole effect is paint-only: a cursor sweep costs repaints, \
         not relayouts.",
        vec![
            any(row()
                .child(labelled("Magnifying \u{b7} 44px items", magnifying))
                .child(hgap(32.0))
                .child(labelled("magnify: off \u{b7} 36px items", plain))
                .cross_axis(CrossAxisAlignment::Start)),
            gap(14.0),
            knob(
                if state.dock_magnify {
                    "Magnify: on"
                } else {
                    "Magnify: off"
                },
                |s: &mut State| s.dock_magnify = !s.dock_magnify,
            ),
        ],
    )
}

/// [`animated_sidebar`] and [`bounce_sidebar`], side by side.
fn sidebars(state: &State) -> AnyView<State> {
    let glyphs = [
        icons::HOME,
        icons::TAG,
        icons::STAR,
        icons::DESCRIPTION,
        icons::SCHEDULE,
    ];

    let rail_items: Vec<_> = DESTINATIONS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let item = sidebar_item(icon(glyphs[index]).size(18.0), *label)
                .active(index == state.sidebar_active)
                .disabled(index == 4);
            if index == 1 { item.badge("12") } else { item }
        })
        .collect();

    let rail = any(SizedBox(None, Some(280.0)).child(animated_sidebar(
        state.sidebar_open,
        rail_items,
        |s: &mut State, index| s.sidebar_active = index,
    )));

    let bounce_items: Vec<_> = DESTINATIONS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            bounce_sidebar_item(icon(glyphs[index]).size(16.0), *label).disabled(index == 4)
        })
        .collect();

    let bounce = any(SizedBox(Some(220.0), None).child(bounce_sidebar(
        state.bounce_active,
        bounce_items,
        |s: &mut State, index| s.bounce_active = index,
    )));

    demo(
        "animated_sidebar \u{b7} bounce_sidebar",
        "The rail ports upstream's desktop panel and menu \u{2014} the width morph, \
         the staggered label reveal and the shared pill. Its mobile drawer, its \
         Ctrl/Cmd-B trigger, its submenus and its non-default geometries are \
         the app's business or need machinery outside one widget. The bounce \
         sidebar is a different component, not a bouncier rail: its items carry \
         no motion at all, and the bounce is the dot's own bowed arc between \
         rows, softened as the jump grows.",
        vec![
            any(row()
                .child(labelled(
                    "Rail \u{b7} collapsible, badge, disabled row",
                    rail,
                ))
                .child(hgap(48.0))
                .child(labelled("Bounce \u{b7} the dot arcs between rows", bounce))
                .cross_axis(CrossAxisAlignment::Start)),
            gap(14.0),
            knob(
                if state.sidebar_open {
                    "Collapse the rail"
                } else {
                    "Expand the rail"
                },
                |s: &mut State| s.sidebar_open = !s.sidebar_open,
            ),
        ],
    )
}

/// [`file_tree`] and [`bouncy_accordion`].
fn disclosure(state: &State) -> AnyView<State> {
    let nodes = vec![
        file_tree_folder(
            "app",
            "app",
            vec![
                file_tree_folder(
                    "components",
                    "components",
                    vec![
                        file_tree_file("file-tree", "file-tree.tsx"),
                        file_tree_file("button", "button.tsx"),
                    ],
                ),
                file_tree_file("page", "page.tsx"),
                file_tree_file("styles", "globals.css"),
            ],
        ),
        file_tree_folder(
            "public",
            "public",
            vec![
                file_tree_file("logo", "logo.svg"),
                file_tree_file("grid", "grid.svg"),
            ],
        ),
        file_tree_file("readme", "README.md"),
        file_tree_folder("vendor", "vendor", vec![file_tree_file("dep", "dep.rs")]).disabled(true),
    ];

    let tree = any(SizedBox(Some(280.0), None).child(
        file_tree(nodes)
            .selected(state.tree_selected.clone())
            .expanded(state.tree_expanded.clone())
            .on_select(|s: &mut State, value| s.tree_selected = value)
            .on_toggle(|s: &mut State, value| {
                if let Some(at) = s.tree_expanded.iter().position(|v| *v == value) {
                    s.tree_expanded.remove(at);
                } else {
                    s.tree_expanded.push(value);
                }
            }),
    ));

    let panels: Vec<_> = PANELS
        .iter()
        .enumerate()
        .map(|(index, (value, title, body))| {
            bouncy_accordion_item(*value, *title, caption(body.to_string())).disabled(index == 5)
        })
        .collect();

    let accordion = any(SizedBox(Some(360.0), None).child(bouncy_accordion(
        state.panel.clone(),
        panels,
        |s: &mut State, value| s.panel = value,
    )));

    demo(
        "file_tree \u{b7} bouncy_accordion",
        "Both animate a height their own layout reports, so paint asks for a \
         relayout while the reveal runs. The tree is one leaf widget \u{2014} every \
         row is painted, none is a child view \u{2014} which is what lets rows wipe \
         in and the rows below slide without any child reconciliation; only one \
         fold animates at a time, and starting a second lands the first. The \
         accordion's springs are Motion's perceptual {duration, bounce} form \
         converted once so each settles in upstream's stated time with \
         upstream's overshoot.",
        vec![any(row()
            .child(labelled("Project tree \u{b7} vendor/ disabled", tree))
            .child(hgap(48.0))
            .child(labelled(
                "Release queue \u{b7} last panel disabled",
                accordion,
            ))
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// [`preview_rail`] in both orientations.
fn rails(state: &State) -> AnyView<State> {
    let items = || {
        SECTIONS
            .iter()
            .map(|(label, body)| preview_rail_item(*label, caption(body.to_string())))
            .collect::<Vec<_>>()
    };

    let vertical = any(SizedBox(Some(340.0), Some(230.0)).child(
        preview_rail(items(), state.rail_active, |s: &mut State, index| {
            s.rail_active = index
        })
        .highlight_active(true),
    ));

    let horizontal = any(SizedBox(Some(340.0), Some(230.0)).child(
        preview_rail(
            items(),
            state.rail_horizontal_active,
            |s: &mut State, index| s.rail_horizontal_active = index,
        )
        .orientation(PreviewRailOrientation::Horizontal),
    ));

    demo(
        "preview_rail",
        "A rail of hairline ticks with one floating card for whichever tick is \
         pointed at \u{2014} not a rail of cards. The falloff is upstream's literal \
         ladder (1, 0.68, 0.44, then a flat 0.25), and what animates is the \
         move between two ladders. The card's entrance drops its blur, an \
         outgoing card gets no exit (a swap replays the entrance), and a press \
         pins a card with a second press un-pinning it.",
        vec![any(row()
            .child(labelled(
                "Vertical \u{b7} active tick highlighted",
                vertical,
            ))
            .child(hgap(48.0))
            .child(labelled("Horizontal", horizontal))
            .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// The page's interactive body.
struct NavigationPage;

impl Component for NavigationPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(column()
            .child(tab_sets(state))
            .child(docks(state))
            .child(sidebars(state))
            .child(disclosure(state))
            .child(rails(state))
            .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> AnyView<AppState> {
    any(column()
        .child(heading("Motion \u{b7} Navigation"))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "Destination surfaces: the three tab strips, the magnifying dock, \
             both sidebars, the file tree, the bouncy accordion, and the \
             preview rail in both orientations.",
        ))
        .child(SizedBox(None, Some(24.0)))
        .child(component(NavigationPage))
        .cross_axis(CrossAxisAlignment::Start))
}
