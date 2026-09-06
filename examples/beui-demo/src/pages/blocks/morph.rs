//! Blocks · Morph: the five blocks whose whole point is a shape change — the
//! expandable tab shell, the reorderable morphing tabs, the notification stack,
//! the dynamic island and the swipeable list.
//!
//! Like the other Blocks pages this is a [`component`] over its own retained
//! [`State`]: every block here is controlled, and [`crate::AppState`] carries no
//! field for any of it.
//!
//! Two of the five carry a premise the porting cards got wrong, and the page's
//! captions state the ported behaviour rather than the card's:
//!
//! - the **notification stack** has no per-card dismiss and no grouping — it is
//!   an expand/collapse stack that animates its own height, and
//! - the **swipeable list** has no full-swipe dismiss: a row is open-left,
//!   open-right or closed, decided by distance thresholds.
//!
//! The **dynamic island**'s state set is the caller's (`call`/`timer`/`music`
//! below is this page's demo data, not the block's vocabulary) and its corner
//! radius is a constant the paint clamps, never an animated lane.

use frust::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, EdgeInsets, FlexView, IconSource,
    Padding, SizedBox, TextView, Theme, any, component, icon, icons, inflexible, text, use_context,
};
use frust_beui::blocks::dynamic_island::{dynamic_island, dynamic_island_slot};
use frust_beui::blocks::expandable_tabs::{expandable_tabs, expandable_tabs_item};
use frust_beui::blocks::morphing_tabs::{morphing_tabs, morphing_tabs_item};
use frust_beui::blocks::notification_stack::{notification, notification_stack};
use frust_beui::blocks::swipeable_list::{
    SwipeAction, SwipeActionEvent, SwipeActionTone, SwipeableListValue, swipe_action,
    swipeable_list, swipeable_row,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};

use crate::AppState;

// ---- The page's own state --------------------------------------------------

/// Everything the five blocks on this page are driven by.
pub struct State {
    /// The open expandable tab, or `None` for the closed, bar-only state.
    tabs_value: Option<String>,
    /// The open morphing-tab room.
    morph_value: Option<String>,
    /// The morphing tabs' live order — reordering is *uncontrolled* inside the
    /// block, so the page mirrors the reported order to keep its item list in
    /// step, and a closed tab is dropped from it.
    morph_order: Vec<String>,
    morph_log: String,
    notifications_expanded: bool,
    notifications_log: String,
    /// Which island state is showing, or `None` for the compact pill.
    island_view: Option<String>,
    /// Which list row is open, and toward which rail.
    swipe_value: Option<SwipeableListValue>,
    swipe_log: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tabs_value: None,
            morph_value: Some(ROOMS[0].0.to_string()),
            morph_order: ROOMS.iter().map(|(id, ..)| id.to_string()).collect(),
            morph_log: "drag a tab to reorder it".to_string(),
            notifications_expanded: false,
            notifications_log: "(collapsed)".to_string(),
            island_view: None,
            swipe_value: None,
            swipe_log: "(no action yet)".to_string(),
        }
    }
}

// ---- Page chrome -----------------------------------------------------------

/// The page's own title.
fn heading(title: &str) -> TextView {
    text(title.to_string()).size(24.0)
}

/// A block's title.
fn section(title: &str) -> TextView {
    text(title.to_string()).size(16.0)
}

/// A block's small print.
fn caption(body: impl Into<String>) -> TextView {
    text(body.into()).size(12.0)
}

/// A vertical gap.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal gap.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// A centred row of controls.
fn controls(children: Vec<AnyView<State>>) -> AnyView<State> {
    let mut spaced = Vec::with_capacity(children.len() * 2);
    for (index, child) in children.into_iter().enumerate() {
        if index > 0 {
            spaced.push(hgap(8.0));
        }
        spaced.push(child);
    }
    any(FlexView::new(
        Axis::Horizontal,
        spaced.into_iter().map(inflexible).collect(),
    )
    .cross_axis(CrossAxisAlignment::Center))
}

// ---- The expandable tab shell ----------------------------------------------

/// One tab's disclosed menu, upstream's own `Menu` of labelled rows.
fn tab_menu(rows: &[&str]) -> AnyView<State> {
    any(SizedBox(Some(274.0), None).child(Column(
        rows.iter()
            .map(|row| {
                any(Padding(
                    EdgeInsets {
                        left: 12.0,
                        top: 8.0,
                        right: 12.0,
                        bottom: 8.0,
                    },
                    text((*row).to_string()).size(14.0),
                ))
            })
            .collect(),
    )))
}

/// The expandable-tabs block: an icon bar whose active tab grows into a
/// labelled pill, disclosing its panel above.
fn expandable_tabs_block(state: &State) -> AnyView<State> {
    let items = vec![
        expandable_tabs_item(
            "launch",
            "Launch",
            icon(icons::SEND).size(16.0),
            tab_menu(&[
                "Release Brief",
                "Launch Checklist",
                "Campaign Notes",
                "Rollout Calendar",
                "Ship Build",
            ]),
        ),
        expandable_tabs_item(
            "inbox",
            "Inbox",
            icon(icons::FORUM).size(16.0),
            tab_menu(&["Client Feedback", "Team Requests", "Approval Notes"]),
        ),
        expandable_tabs_item(
            "flows",
            "Flows",
            icon(icons::REFRESH).size(16.0),
            tab_menu(&["Trigger Map", "Webhook Runs", "Retry Queue"]),
        ),
        expandable_tabs_item(
            "assets",
            "Assets",
            icon(icons::IMAGE).size(16.0),
            tab_menu(&[
                "Brand Kit",
                "Mockup Library",
                "Design Tokens",
                "Export Queue",
            ]),
        ),
        expandable_tabs_item(
            "status",
            "Status",
            icon(icons::DONE_ALL).size(16.0),
            tab_menu(&["Activation", "Conversion", "Incidents"]),
        ),
    ];

    any(Column(vec![
        any(section("expandable_tabs")),
        gap(6.0),
        any(caption(
            "An icon bar whose active tab expands to a labelled pill, with the panel above \
             morphing its height and sliding its content direction-aware on a switch. Pressing \
             the open tab again closes the shell back to the bar.",
        )),
        gap(10.0),
        any(expandable_tabs(
            state.tabs_value.clone(),
            items,
            |s: &mut State, value: Option<String>| s.tabs_value = value,
        )),
        gap(8.0),
        any(caption(format!(
            "Open tab: {}",
            state.tabs_value.as_deref().unwrap_or("(closed)")
        ))),
    ]))
}

// ---- The morphing tabs -----------------------------------------------------

/// The rooms the morphing tabs open, in their declared order:
/// `(id, title, eyebrow, detail)`.
const ROOMS: [(&str, &str, &str, &str); 3] = [
    (
        "room-2",
        "Room 2",
        "quiet focus",
        "A small space for the work that needs a little more air around it.",
    ),
    (
        "general",
        "General",
        "shared space",
        "The common room for notes, links and the ideas that are still finding their shape.",
    ),
    (
        "archive",
        "Archive",
        "kept close",
        "Past rooms stay available without competing with the conversations in motion.",
    ),
];

/// One room's panel.
fn room_panel(eyebrow: &str, title: &str, detail: &str) -> AnyView<State> {
    any(Padding(
        EdgeInsets {
            left: 28.0,
            top: 28.0,
            right: 28.0,
            bottom: 32.0,
        },
        Column(vec![
            any(text(eyebrow.to_string()).size(11.0)),
            gap(10.0),
            any(text(title.to_string()).size(32.0)),
            gap(12.0),
            any(SizedBox(Some(360.0), None).child(text(detail.to_string()).size(14.0))),
            gap(20.0),
            any(text("drag any room to reorder").size(12.0)),
        ]),
    ))
}

/// The morphing-tabs block: the selected tab grows into the room's surface, and
/// a pointer drag reorders the rail.
fn morphing_tabs_block(state: &State) -> AnyView<State> {
    let items = state
        .morph_order
        .iter()
        .filter_map(|id| {
            ROOMS.iter().find(|(room_id, ..)| room_id == id).map(
                |(room_id, title, eyebrow, detail)| {
                    morphing_tabs_item(*room_id, *title, room_panel(eyebrow, title, detail))
                },
            )
        })
        .collect();

    any(Column(vec![
        any(section("morphing_tabs")),
        gap(6.0),
        any(caption(
            "The selected tab grows into the room's own surface and the active shape glides as \
             tabs move. Reordering by pointer drag is part of the block (Alt + \u{2190}/\u{2192} \
             does the same from the keyboard); the order is uncontrolled inside it, so the page \
             mirrors what it reports.",
        )),
        gap(10.0),
        any(morphing_tabs(
            state.morph_value.clone(),
            items,
            |s: &mut State, value: String| {
                s.morph_log = format!("opened {value}");
                s.morph_value = Some(value);
            },
        )
        .on_order_change(|s: &mut State, order: Vec<String>| {
            s.morph_log = format!("order: {}", order.join(", "));
            s.morph_order = order;
        })
        .on_close(|s: &mut State, id: String| {
            s.morph_order.retain(|room| room != &id);
            if s.morph_value.as_deref() == Some(id.as_str()) {
                s.morph_value = s.morph_order.first().cloned();
            }
            s.morph_log = format!("closed {id}");
        })),
        gap(10.0),
        controls(vec![
            any(button("Reset rooms", |s: &mut State| {
                s.morph_order = ROOMS.iter().map(|(id, ..)| id.to_string()).collect();
                s.morph_value = Some(ROOMS[0].0.to_string());
                s.morph_log = "reset".to_string();
            })
            .tone(ButtonTone::Secondary)
            .size(ButtonSize::Sm)),
            any(caption(state.morph_log.clone())),
        ]),
    ]))
}

// ---- The notification stack ------------------------------------------------

/// The notification-stack block: a stacked summary that springs into a readable
/// list and animates its own height doing it.
fn notification_block(state: &State) -> AnyView<State> {
    let items = vec![
        notification("import-failed", "Orders import failed")
            .description("42s \u{b7} TimeoutError at Step 2")
            .trailing("retried 2"),
        notification("sla-breach", "SLA breach").description("2m 11s \u{b7} Data enrichment"),
        notification("sync-fixed", "Product sync auto-fixed")
            .description("5m \u{b7} 404 on GET /products"),
        notification("digest-ready", "Weekly digest ready")
            .description("12m \u{b7} 84 events summarised")
            .trailing("new"),
    ];

    any(Column(vec![
        any(section("notification_stack")),
        gap(6.0),
        any(caption(
            "Compact cards that spring from a stacked summary into a readable list on hover, \
             focus or press, the stack animating its own height as it goes. There is no per-card \
             dismiss and no grouping in this block: it expands and collapses, and a second press \
             on the open stack reports \u{201c}view all\u{201d} rather than closing it.",
        )),
        gap(10.0),
        any(notification_stack(
            items,
            state.notifications_expanded,
            |s: &mut State, expanded: bool| {
                s.notifications_expanded = expanded;
                s.notifications_log = if expanded {
                    "expanded".to_string()
                } else {
                    "(collapsed)".to_string()
                };
            },
        )
        .max_visible(3)
        .collapsed_label("Notifications")
        .expanded_label("View all")
        .on_view_all(|s: &mut State| s.notifications_log = "view all pressed".to_string())),
        gap(8.0),
        any(caption(format!("Stack: {}", state.notifications_log))),
    ]))
}

// ---- The dynamic island ----------------------------------------------------

/// The island's demo states — the caller's vocabulary, not the block's.
const ISLAND_STATES: [(&str, &str); 3] = [("call", "Call"), ("timer", "Timer"), ("music", "Music")];

/// The island block: a pill that morphs between the caller's live-activity
/// views, springing its shell and crossfading its content.
fn island_block(state: &State) -> AnyView<State> {
    // The shell paints `on_surface`, so its content is inked in `surface` — the
    // island is inverted chrome and a child paints its own text.
    let ink = use_context::<Theme>()
        .unwrap_or_else(frust_beui::theme)
        .scheme()
        .surface;

    let compact = Column(vec![any(text("9:41").size(13.0).color(ink))]);
    let slots = vec![
        dynamic_island_slot(
            "call",
            Column(vec![
                any(text("INCOMING CALL").size(10.0).color(ink)),
                gap(4.0),
                any(text("Saurabh").size(14.0).color(ink)),
            ]),
        ),
        dynamic_island_slot(
            "timer",
            Column(vec![
                any(text("TIMER").size(10.0).color(ink)),
                gap(4.0),
                any(text("2:34").size(20.0).color(ink)),
            ]),
        ),
        dynamic_island_slot(
            "music",
            Column(vec![
                any(text("NOW PLAYING").size(10.0).color(ink)),
                gap(4.0),
                any(text("Weightless \u{b7} Marconi Union")
                    .size(13.0)
                    .color(ink)),
            ]),
        ),
    ];

    let mut buttons: Vec<AnyView<State>> = vec![any(button("Compact", |s: &mut State| {
        s.island_view = None;
    })
    .tone(if state.island_view.is_none() {
        ButtonTone::Primary
    } else {
        ButtonTone::Outline
    })
    .size(ButtonSize::Sm))];
    for (id, label) in ISLAND_STATES {
        let selected = state.island_view.as_deref() == Some(id);
        buttons.push(any(button(label, move |s: &mut State| {
            s.island_view = Some(id.to_string());
        })
        .tone(if selected {
            ButtonTone::Primary
        } else {
            ButtonTone::Outline
        })
        .size(ButtonSize::Sm)));
    }

    any(Column(vec![
        any(section("dynamic_island")),
        gap(6.0),
        any(caption(
            "An iOS-style pill that morphs between live-activity views, its shell springing to \
             each one's size while the content crossfades. The state set is supplied by the \
             caller — the three below are this page's demo data — and the corner radius is a \
             constant the paint clamps to half the shorter edge, never an animated lane.",
        )),
        gap(10.0),
        any(dynamic_island(state.island_view.clone(), compact, slots).label("Live activity")),
        gap(10.0),
        controls(buttons),
    ]))
}

// ---- The swipeable list ----------------------------------------------------

/// A row's leading mark.
fn row_mark(glyph: IconSource) -> AnyView<State> {
    any(SizedBox(Some(40.0), Some(40.0)).child(icon(glyph).size(18.0)))
}

/// The leading rail: what a drag to the right reveals.
fn left_actions() -> Vec<SwipeAction<State>> {
    vec![
        swipe_action("done", "Done", icon(icons::CHECK).size(16.0)).tone(SwipeActionTone::Success),
        swipe_action("pin", "Pin", icon(icons::PUSH_PIN).size(16.0)).tone(SwipeActionTone::Primary),
    ]
}

/// The trailing rail: what a drag to the left reveals.
fn right_actions() -> Vec<SwipeAction<State>> {
    vec![
        swipe_action("later", "Later", icon(icons::SCHEDULE).size(16.0))
            .tone(SwipeActionTone::Warning),
        swipe_action("trash", "Trash", icon(icons::DELETE).size(16.0))
            .tone(SwipeActionTone::Danger),
    ]
}

/// The swipeable-list block: an inbox whose rows open onto their rails.
fn swipeable_block(state: &State) -> AnyView<State> {
    let rows = [
        (
            "brief",
            "Launch brief",
            "Finalize the announcement copy",
            "9:41",
            icons::DESCRIPTION,
        ),
        (
            "feedback",
            "Client feedback",
            "Three comments need a response",
            "11:08",
            icons::FORUM,
        ),
        (
            "review",
            "Design review",
            "Sign off on the new gallery shell",
            "13:20",
            icons::STAR,
        ),
    ];
    let items = rows
        .iter()
        .map(|(id, title, description, meta, glyph)| {
            swipeable_row(*id)
                .title(*title)
                .description(*description)
                .meta(*meta)
                .leading(row_mark(*glyph))
                .left_actions(left_actions())
                .right_actions(right_actions())
        })
        .collect();

    any(Column(vec![
        any(section("swipeable_list")),
        gap(6.0),
        any(caption(
            "Drag a row left or right to reveal its rail. A row is open-left, open-right or \
             closed — there is no full-swipe dismiss in this block, and the open/close decision \
             is a distance threshold, not a fling. Opening one row closes the other, and pressing \
             an action closes the row it belongs to.",
        )),
        gap(10.0),
        any(swipeable_list(
            items,
            state.swipe_value.clone(),
            |s: &mut State, value: Option<SwipeableListValue>| s.swipe_value = value,
        )
        .on_action(|s: &mut State, event: SwipeActionEvent| {
            s.swipe_log = format!("{} on {}", event.action_id, event.item_id);
        })),
        gap(8.0),
        any(caption(format!("Last action: {}", state.swipe_log))),
    ]))
}

// ---- The page --------------------------------------------------------------

/// The Blocks · Morph page.
#[derive(Default)]
struct MorphPage;

impl Component for MorphPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> AnyView<State> {
        any(Column(vec![
            any(heading("Blocks \u{b7} Morph")),
            gap(8.0),
            any(caption(
                "Five blocks whose shape is the component: a tab shell that discloses, a tab set \
                 that reorders, a stack that unstacks, a pill that becomes a panel, and rows that \
                 open onto their rails.",
            )),
            gap(24.0),
            expandable_tabs_block(state),
            gap(24.0),
            morphing_tabs_block(state),
            gap(24.0),
            notification_block(state),
            gap(24.0),
            island_block(state),
            gap(24.0),
            swipeable_block(state),
        ]))
    }
}

/// The Blocks · Morph page, hosted over its own retained [`State`].
pub fn page() -> AnyView<AppState> {
    any(component(MorphPage))
}
