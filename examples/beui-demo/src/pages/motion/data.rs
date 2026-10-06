//! Motion · Data: the two data surfaces — the [`animated_toast_stack`] and the
//! virtualized [`table`].
//!
//! The toast stack is its own top layer (it fills the area it is given and
//! positions its cards against a corner), so it mounts inside a bounded
//! [`frust::Stack`] stage here rather than over the whole window — the same
//! demo-sized staging `crate::pages::motion::overlays` uses, and for the same
//! reason: a page inside the gallery shell's scroll view has no bounded area of
//! its own to hand a full-area layer.
//!
//! The table's dataset is synthetic and derived, not stored: every cell is a
//! pure function of its row index, so the 1,000-row default (and the 5,000 the
//! end-reached signal grows it to) costs no memory beyond the sort order.
//!
//! State lives in a [`frust::component`], as on every Motion page (see
//! `crate::pages::motion::text`).

use std::rc::Rc;

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, Row, SizedBox, View, any, column, component,
    row, stack, text,
};
use frust_beui::components::animated_toast_stack::{
    AnimatedToastStackEntry, AnimatedToastStackPosition, AnimatedToastStackStatus, animated_toast,
    animated_toast_stack,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::table::{
    TableAlign, TableSort, TableSortDirection, table, table_column, table_next_sort,
};

use crate::AppState;
use crate::nav::{caption, heading};

/// The toast queue's five tones, with the copy each one is raised under.
const TOASTS: [(AnimatedToastStackStatus, &str, &str, Option<&str>); 5] = [
    (
        AnimatedToastStackStatus::Neutral,
        "Draft saved",
        "Your changes are stored locally.",
        None,
    ),
    (
        AnimatedToastStackStatus::Info,
        "New version available",
        "Reload to pick up build 482.",
        Some("Reload"),
    ),
    (
        AnimatedToastStackStatus::Loading,
        "Syncing workspace",
        "Uploading 12 changed files.",
        None,
    ),
    (
        AnimatedToastStackStatus::Success,
        "Deployment complete",
        "beui-demo is live on the preview channel.",
        Some("View"),
    ),
    (
        AnimatedToastStackStatus::Error,
        "Upload failed",
        "The connection dropped after 4.2 MB.",
        Some("Retry"),
    ),
];

/// Every corner the stack can anchor to.
const POSITIONS: [(AnimatedToastStackPosition, &str); 6] = [
    (AnimatedToastStackPosition::TopLeft, "top-left"),
    (AnimatedToastStackPosition::TopCenter, "top-center"),
    (AnimatedToastStackPosition::TopRight, "top-right"),
    (AnimatedToastStackPosition::BottomLeft, "bottom-left"),
    (AnimatedToastStackPosition::BottomCenter, "bottom-center"),
    (AnimatedToastStackPosition::BottomRight, "bottom-right"),
];

/// The table's synthetic name pool.
const FIRST_NAMES: [&str; 12] = [
    "Ada", "Grace", "Alan", "Edsger", "Barbara", "Ken", "Margaret", "Linus", "Radia", "Donald",
    "Frances", "Tony",
];

/// The table's synthetic surname pool.
const LAST_NAMES: [&str; 8] = [
    "Lovelace", "Hopper", "Turing", "Dijkstra", "Liskov", "Thompson", "Hamilton", "Perlman",
];

/// The table's teams.
const TEAMS: [&str; 6] = [
    "Engine", "Widgets", "Shells", "Tooling", "Design", "Research",
];

/// The table's statuses.
const STATUSES: [&str; 4] = ["Active", "Review", "Blocked", "Archived"];

/// The row count the table starts at, and the step each end-reached signal adds.
const INITIAL_ROWS: usize = 1_000;
/// One page of extra rows, appended when the list nears its end.
const ROW_PAGE: usize = 500;
/// The cap the demo stops growing at.
const MAX_ROWS: usize = 5_000;

/// A deterministic scramble of a row index — the dataset's whole source of
/// variety, so no row is ever stored.
fn scramble(index: usize) -> u64 {
    let mut x = (index as u64)
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^ (x >> 29)
}

/// Row `index`'s name.
fn row_name(index: usize) -> String {
    let hash = scramble(index);
    format!(
        "{} {}",
        FIRST_NAMES[(hash % FIRST_NAMES.len() as u64) as usize],
        LAST_NAMES[((hash >> 8) % LAST_NAMES.len() as u64) as usize]
    )
}

/// Row `index`'s team.
fn row_team(index: usize) -> &'static str {
    TEAMS[((scramble(index) >> 16) % TEAMS.len() as u64) as usize]
}

/// Row `index`'s status.
fn row_status(index: usize) -> &'static str {
    STATUSES[((scramble(index) >> 24) % STATUSES.len() as u64) as usize]
}

/// Row `index`'s commit count.
fn row_commits(index: usize) -> u64 {
    (scramble(index) >> 32) % 900 + 12
}

/// How many days ago row `index` was last updated.
fn row_age(index: usize) -> u64 {
    (scramble(index) >> 40) % 90
}

/// One cell's text.
fn cell_text(index: usize, column: usize) -> String {
    match column {
        0 => row_name(index),
        1 => row_team(index).to_string(),
        2 => row_status(index).to_string(),
        3 => row_commits(index).to_string(),
        _ => match row_age(index) {
            0 => "today".to_string(),
            1 => "yesterday".to_string(),
            days => format!("{days} days ago"),
        },
    }
}

/// The display order for `row_count` rows under `sort` — the app's job, since
/// the table is controlled and reads its cells through a provider.
fn sorted_order(row_count: usize, sort: Option<TableSort>) -> Rc<Vec<usize>> {
    let mut order: Vec<usize> = (0..row_count).collect();
    if let Some(sort) = sort {
        match sort.column {
            3 => order.sort_by_key(|&i| row_commits(i)),
            4 => order.sort_by_key(|&i| row_age(i)),
            column => order.sort_by_key(|&i| cell_text(i, column)),
        }
        if sort.direction == TableSortDirection::Descending {
            order.reverse();
        }
    }
    Rc::new(order)
}

/// This page's retained queue, selection and sort.
pub struct State {
    toasts: Vec<AnimatedToastStackEntry>,
    next_toast_id: u64,
    toast_position: AnimatedToastStackPosition,
    toast_status: String,
    row_count: usize,
    sort: Option<TableSort>,
    order: Rc<Vec<usize>>,
    selected: Rc<Vec<usize>>,
    pages_loaded: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            toasts: Vec::new(),
            next_toast_id: 1,
            toast_position: AnimatedToastStackPosition::BottomRight,
            toast_status: "Raise a few, then hover the stack to expand it.".to_string(),
            row_count: INITIAL_ROWS,
            sort: None,
            order: sorted_order(INITIAL_ROWS, None),
            selected: Rc::new(Vec::new()),
            pages_loaded: 0,
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

/// A small outline button — this page's triggers.
fn knob(label: &str, on_press: impl Fn(&mut State) + 'static) -> AnyView<State> {
    any(button(label.to_string(), on_press)
        .tone(ButtonTone::Outline)
        .size(ButtonSize::Sm))
}

/// A toast status's own name — the trigger buttons' labels.
fn status_label(status: AnimatedToastStackStatus) -> &'static str {
    match status {
        AnimatedToastStackStatus::Neutral => "Neutral",
        AnimatedToastStackStatus::Info => "Info",
        AnimatedToastStackStatus::Loading => "Loading",
        AnimatedToastStackStatus::Success => "Success",
        AnimatedToastStackStatus::Error => "Error",
    }
}

/// The [`animated_toast_stack`], with a trigger per status.
fn toasts(state: &State) -> AnyView<State> {
    let mut triggers: Vec<AnyView<State>> = Vec::new();
    for (index, (status, _, _, _)) in TOASTS.iter().enumerate() {
        if !triggers.is_empty() {
            triggers.push(hgap(8.0));
        }
        // The trigger is labelled by the *status* rather than by the toast's
        // title: five title-length buttons do not fit the stage at a 900px
        // window.
        triggers.push(knob(status_label(*status), move |s: &mut State| {
            let (status, title, description, action) = TOASTS[index];
            let id = s.next_toast_id;
            s.next_toast_id += 1;
            let mut entry = animated_toast(id, title)
                .description(description)
                .status(status);
            if let Some(label) = action {
                entry = entry.action(label);
            }
            s.toasts.push(entry);
            s.toast_status = format!("Raised \u{201c}{title}\u{201d} (id {id}).");
        }));
    }

    let position_row = |from: usize, to: usize| {
        let mut row: Vec<AnyView<State>> = Vec::new();
        for (position, label) in &POSITIONS[from..to] {
            if !row.is_empty() {
                row.push(hgap(8.0));
            }
            let position = *position;
            row.push(knob(label, move |s: &mut State| {
                s.toast_position = position
            }));
        }
        any(Row(row).cross_axis(CrossAxisAlignment::Center))
    };

    let panel = any(column()
        .child(text("Raise a toast").size(15.0))
        .child(gap(8.0))
        .child(Row(triggers).cross_axis(CrossAxisAlignment::Center))
        .child(gap(12.0))
        .child(
            row()
                .child(knob("Dismiss oldest", |s: &mut State| {
                    if !s.toasts.is_empty() {
                        s.toasts.remove(0);
                    }
                }))
                .child(hgap(8.0))
                .child(knob("Dismiss all", |s: &mut State| s.toasts.clear())),
        )
        .child(gap(12.0))
        .child(caption("Position"))
        .child(gap(6.0))
        .child(position_row(0, 3))
        .child(gap(6.0))
        .child(position_row(3, 6))
        .child(gap(12.0))
        .child(caption(state.toast_status.clone()))
        .child(gap(4.0))
        .child(caption(format!("Queued: {}", state.toasts.len())))
        .cross_axis(CrossAxisAlignment::Start));

    let toast_stack = any(
        animated_toast_stack(state.toasts.clone(), |s: &mut State, id| {
            s.toasts.retain(|toast| toast.id() != id);
            s.toast_status = format!("Dismissed id {id}.");
        })
        .position(state.toast_position)
        .max_visible(4)
        .on_action(|s: &mut State, id| {
            s.toast_status = format!("Action pressed on id {id}.");
        }),
    );

    demo(
        "animated_toast_stack",
        "Upstream's own file is a gap-separated list; the collapsed depth stack \
         is the sibling notification-stack's peek/inset geometry, so both \
         shapes here are shipped upstream and hover moves between them. Depth 0 \
         is the newest toast (upstream paints its oldest on top). The queue is \
         the owner's: there is no timer, because a countdown expiring during \
         paint has no route to app state. Swipe a card sideways past 72px to \
         dismiss it \u{2014} the velocity arm is not ported, a PointerEvent carries \
         none \u{2014} and no ramp blurs.",
        vec![any(
            SizedBox(Some(620.0), Some(380.0)).child(stack().child(panel).child(toast_stack))
        )],
    )
}

/// The virtualized [`table`].
fn tables(state: &State) -> AnyView<State> {
    let columns = vec![
        table_column("Name").sortable(true).width(200.0),
        table_column("Team").sortable(true),
        table_column("Status").sortable(true),
        table_column("Commits")
            .sortable(true)
            .align(TableAlign::End)
            .width(110.0),
        table_column("Updated")
            .sortable(true)
            .align(TableAlign::End),
    ];

    let order = state.order.clone();
    let key_order = state.order.clone();
    let selected_order = state.order.clone();
    let selected = state.selected.clone();

    let view = any(table(columns, state.row_count, move |row, column| {
        order
            .get(row)
            .map_or_else(String::new, |&source| cell_text(source, column))
    })
    .row_key(move |row| key_order.get(row).copied().unwrap_or(row) as u64)
    .selected(move |row| {
        selected_order
            .get(row)
            .is_some_and(|source| selected.contains(source))
    })
    .sort(state.sort)
    .viewport_height(440.0)
    .on_sort(|s: &mut State, sort| {
        s.sort = sort;
        s.order = sorted_order(s.row_count, sort);
    })
    .on_row_press(|s: &mut State, row| {
        let Some(&source) = s.order.get(row) else {
            return;
        };
        let mut selected = (*s.selected).clone();
        if let Some(at) = selected.iter().position(|&i| i == source) {
            selected.remove(at);
        } else {
            selected.push(source);
        }
        s.selected = Rc::new(selected);
    })
    .on_end_reached(|s: &mut State| {
        if s.row_count < MAX_ROWS {
            s.row_count = (s.row_count + ROW_PAGE).min(MAX_ROWS);
            s.pages_loaded += 1;
            s.order = sorted_order(s.row_count, s.sort);
        }
    }));

    demo(
        "table",
        "A synthetic dataset read through one cell(row, column) provider, so \
         only the rows ListView has materialized are ever asked for. It rides \
         ListView keyed with a variable extent, which is what lets a row keep \
         its hover state across a re-sort; the head row is painted above the \
         list rather than being row 0, since ListView has no sticky row. Press \
         a header to cycle asc \u{2192} desc \u{2192} none, press a row to select it, and \
         scroll to the end to fire on_end_reached. Rows animate in but cannot \
         animate out (the list tears a departing key down immediately), cells \
         are single-line strings, and horizontal overflow scales rather than \
         scrolling.",
        vec![
            any(SizedBox(Some(720.0), None).child(view)),
            gap(12.0),
            any(row()
                .child(caption(format!(
                    "{} rows \u{b7} {} selected \u{b7} {} extra page(s) loaded \u{b7} sort: {}",
                    state.row_count,
                    state.selected.len(),
                    state.pages_loaded,
                    match state.sort {
                        None => "none".to_string(),
                        Some(sort) => format!(
                            "column {} {}",
                            sort.column,
                            match sort.direction {
                                TableSortDirection::Ascending => "asc",
                                TableSortDirection::Descending => "desc",
                            }
                        ),
                    }
                )))
                .child(hgap(16.0))
                .child(knob("Clear selection", |s: &mut State| {
                    s.selected = Rc::new(Vec::new())
                }))
                .child(hgap(8.0))
                .child(knob("Sort by commits", |s: &mut State| {
                    let sort = table_next_sort(s.sort, 3);
                    s.sort = sort;
                    s.order = sorted_order(s.row_count, sort);
                }))
                .child(hgap(8.0))
                .child(knob("Reset rows", |s: &mut State| {
                    s.row_count = INITIAL_ROWS;
                    s.pages_loaded = 0;
                    s.order = sorted_order(s.row_count, s.sort);
                }))
                .cross_axis(CrossAxisAlignment::Center)),
        ],
    )
}

/// The page's interactive body.
struct DataPage;

impl Component for DataPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(column()
            .child(toasts(state))
            .child(tables(state))
            .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> AnyView<AppState> {
    any(column()
        .child(heading("Motion \u{b7} Data"))
        .child(SizedBox(None, Some(8.0)))
        .child(caption(
            "The two data surfaces: the toast stack, with a trigger per status \
             and every anchor corner, and the virtualized table over a \
             1,000-row synthetic dataset that grows to 5,000 as you reach its \
             end.",
        ))
        .child(SizedBox(None, Some(24.0)))
        .child(component(DataPage))
        .cross_axis(CrossAxisAlignment::Start))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is controlled, so the *app* owns the row order — a bug here is
    /// a bug in the demo, not in the component.
    #[test]
    fn the_sort_order_is_always_a_permutation_of_every_row() {
        for sort in [
            None,
            Some(TableSort::ascending(0)),
            Some(TableSort::descending(2)),
            Some(TableSort::ascending(3)),
            Some(TableSort::descending(4)),
        ] {
            let order = sorted_order(64, sort);
            let mut seen = (*order).clone();
            seen.sort_unstable();
            assert_eq!(seen, (0..64).collect::<Vec<_>>(), "sort: {sort:?}");
        }
    }

    #[test]
    fn a_descending_sort_is_the_ascending_one_reversed() {
        let ascending = sorted_order(64, Some(TableSort::ascending(3)));
        let descending = sorted_order(64, Some(TableSort::descending(3)));
        let mut reversed = (*ascending).clone();
        reversed.reverse();
        assert_eq!(*descending, reversed);
    }

    #[test]
    fn the_commit_sort_runs_by_commit_count() {
        let order = sorted_order(64, Some(TableSort::ascending(3)));
        let counts: Vec<u64> = order.iter().map(|&index| row_commits(index)).collect();
        assert!(counts.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn every_column_yields_a_cell() {
        for column in 0..5 {
            assert!(!cell_text(7, column).is_empty(), "column {column}");
        }
    }
}
