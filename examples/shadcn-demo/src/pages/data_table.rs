//! Data Table: the full data-table recipe as **app code** over the catalog's
//! primitives — sorting, filtering, paging, selection and column visibility all
//! live in this page's own `State`, and `table` only renders what that state
//! resolves to.
//!
//! The page is a full-area `Stack`, not a scroll slot: its two menus (column
//! visibility, per-row actions) are anchored overlays, and an anchored host
//! needs bounded constraints. So the stack takes the page slot's own height,
//! the body scrolls *inside* it, and both menus are **kept mounted** with
//! `.open(flag)` so closing one plays its exit ramp.

use std::collections::{HashMap, HashSet};

use frust::{
    Axis, Column as ColumnOf, CrossAxisAlignment, FlexView, SizedBox, Stack, View, any, flexible,
    inflexible, text,
};
use frust_shadcn::overlay::{OverlayAnchor, anchor};
use frust_shadcn::{
    BadgeVariant, ButtonSize, ButtonVariant, PaginationItem, badge, button, checkbox,
    dropdown_menu, dropdown_menu_item, dropdown_menu_label, dropdown_menu_separator, input,
    pagination, separator, table, table_row,
};

use crate::AppState;

/// How many rows one page of the table shows.
const PAGE_SIZE: usize = 6;

/// The optional columns — `Name` is always shown, so it is not in this list.
const TOGGLEABLE: [Column; 3] = [Column::Role, Column::Status, Column::Amount];

/// A sortable/hideable column of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Name,
    Role,
    Status,
    Amount,
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Role => "Role",
            Column::Status => "Status",
            Column::Amount => "Amount",
        }
    }
}

/// A row's lifecycle state, rendered as a badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Active,
    Invited,
    Suspended,
}

impl Status {
    fn title(self) -> &'static str {
        match self {
            Status::Active => "Active",
            Status::Invited => "Invited",
            Status::Suspended => "Suspended",
        }
    }

    fn variant(self) -> BadgeVariant {
        match self {
            Status::Active => BadgeVariant::Default,
            Status::Invited => BadgeVariant::Secondary,
            Status::Suspended => BadgeVariant::Destructive,
        }
    }
}

/// One row of sample data. `id` is stable across deletes, sorts and filters —
/// it keys both the selection set and the per-row anchor.
#[derive(Clone)]
pub struct Person {
    pub id: u32,
    pub name: String,
    pub role: String,
    pub status: Status,
    pub amount: i64,
}

pub struct State {
    pub rows: Vec<Person>,
    pub filter: String,
    /// The active sort: the column and whether it is ascending.
    pub sort: Option<(Column, bool)>,
    /// Zero-based page index into the filtered, sorted rows.
    pub page: usize,
    pub selected: HashSet<u32>,
    pub visible: HashSet<Column>,
    pub columns_open: bool,
    pub columns_anchor: OverlayAnchor,
    /// Whether the row-actions menu is open, and the row it is anchored to. The
    /// row is kept while closed so the exit ramp plays where it opened.
    pub row_menu_open: bool,
    pub row_menu_row: u32,
    pub row_anchors: HashMap<u32, OverlayAnchor>,
    pub last_action: String,
}

impl Default for State {
    fn default() -> Self {
        let rows = seed();
        let row_anchors = rows
            .iter()
            .map(|person| (person.id, OverlayAnchor::new()))
            .collect();
        Self {
            rows,
            filter: String::new(),
            sort: Some((Column::Name, true)),
            page: 0,
            selected: HashSet::new(),
            visible: TOGGLEABLE.into_iter().collect(),
            columns_open: false,
            columns_anchor: OverlayAnchor::new(),
            row_menu_open: false,
            row_menu_row: 0,
            row_anchors,
            last_action: "(no row action yet)".to_string(),
        }
    }
}

impl State {
    /// The rows the current filter keeps, in the current sort order.
    fn view_rows(&self) -> Vec<Person> {
        let needle = self.filter.trim().to_lowercase();
        let mut rows: Vec<Person> = self
            .rows
            .iter()
            .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        if let Some((column, ascending)) = self.sort {
            rows.sort_by(|a, b| {
                let ordering = match column {
                    Column::Name => a.name.cmp(&b.name),
                    Column::Role => a.role.cmp(&b.role),
                    Column::Status => a.status.cmp(&b.status),
                    Column::Amount => a.amount.cmp(&b.amount),
                };
                if ascending {
                    ordering
                } else {
                    ordering.reverse()
                }
            });
        }
        rows
    }

    /// How many pages the filtered rows fill (never zero, so the pager always
    /// has a page 1 to sit on).
    fn page_count(&self, total: usize) -> usize {
        total.div_ceil(PAGE_SIZE).max(1)
    }
}

/// The sample roster: eighteen plausible rows.
fn seed() -> Vec<Person> {
    const PEOPLE: [(&str, &str, Status, i64); 18] = [
        ("Ada Lovelace", "Mathematician", Status::Active, 4820),
        ("Alan Turing", "Computer scientist", Status::Active, 3990),
        ("Grace Hopper", "Rear Admiral", Status::Active, 5310),
        ("Katherine Johnson", "Aerospace", Status::Invited, 2740),
        ("Barbara Liskov", "Systems", Status::Active, 6120),
        ("Edsger Dijkstra", "Algorithms", Status::Suspended, 1880),
        ("Margaret Hamilton", "Flight software", Status::Active, 5975),
        ("Donald Knuth", "Typesetting", Status::Invited, 3140),
        ("Radia Perlman", "Networks", Status::Active, 4405),
        ("Ken Thompson", "Operating systems", Status::Suspended, 2260),
        ("Frances Allen", "Compilers", Status::Active, 5080),
        (
            "Leslie Lamport",
            "Distributed systems",
            Status::Invited,
            3620,
        ),
        ("Shafi Goldwasser", "Cryptography", Status::Active, 6740),
        ("Tony Hoare", "Languages", Status::Active, 2905),
        ("Jean Bartik", "ENIAC", Status::Invited, 1990),
        ("Bjarne Stroustrup", "Languages", Status::Suspended, 3475),
        ("Sophie Wilson", "Silicon", Status::Active, 5560),
        ("Linus Torvalds", "Kernels", Status::Active, 4110),
    ];
    PEOPLE
        .iter()
        .enumerate()
        .map(|(index, (name, role, status, amount))| Person {
            id: index as u32,
            name: (*name).to_string(),
            role: (*role).to_string(),
            status: *status,
            amount: *amount,
        })
        .collect()
}

/// The header cell for `column`: a ghost button that cycles ascending →
/// descending → ascending, carrying the active direction in its label.
fn sort_button(column: Column, sort: Option<(Column, bool)>) -> frust::AnyView<AppState> {
    let arrow = match sort {
        Some((active, true)) if active == column => " \u{2191}",
        Some((active, false)) if active == column => " \u{2193}",
        _ => "",
    };
    any(button(
        format!("{}{arrow}", column.title()),
        move |s: &mut AppState| {
            s.data_table.sort = match s.data_table.sort {
                Some((active, ascending)) if active == column => Some((column, !ascending)),
                _ => Some((column, true)),
            };
            s.data_table.page = 0;
        },
    )
    .variant(ButtonVariant::Ghost)
    .size(ButtonSize::Sm))
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let rows = state.view_rows();
    let total = rows.len();
    let page_count = state.page_count(total);
    let current = state.page.min(page_count - 1);
    let page_rows: Vec<Person> = rows
        .iter()
        .skip(current * PAGE_SIZE)
        .take(PAGE_SIZE)
        .cloned()
        .collect();

    let visible: Vec<Column> = TOGGLEABLE
        .into_iter()
        .filter(|c| state.visible.contains(c))
        .collect();
    let selected_here = rows
        .iter()
        .filter(|p| state.selected.contains(&p.id))
        .count();
    let all_selected = total > 0 && selected_here == total;
    let some_selected = selected_here > 0 && !all_selected;
    let filter = state.filter.clone();
    let sort = state.sort;

    // --- The toolbar: the filter field and the column-visibility trigger ---
    let toolbar = FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(
                1,
                any(input(filter, |s: &mut AppState, v: String| {
                    s.data_table.filter = v;
                    s.data_table.page = 0;
                })
                .placeholder("Filter names\u{2026}")),
            ),
            inflexible(any(SizedBox::<AppState>(Some(8.0), None))),
            inflexible(any(anchor(
                &state.columns_anchor,
                button("Columns \u{25BE}", |s: &mut AppState| {
                    s.data_table.columns_open = !s.data_table.columns_open;
                })
                .variant(ButtonVariant::Outline),
            ))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center);

    // --- The header row ---
    //
    // `TableView::header` takes plain strings, so a header that carries controls
    // (the tri-state select-all box, the sort buttons) is authored as the first
    // *body* row instead, marked selected for the `bg-muted` wash that reads as
    // a head band. Doing it this way also keeps it in the same column-width
    // solve as the rows under it.
    let mut header_cells: Vec<frust::AnyView<AppState>> = vec![any(checkbox(
        all_selected,
        move |s: &mut AppState, _next: bool| {
            let ids: Vec<u32> = s.data_table.view_rows().iter().map(|p| p.id).collect();
            let everything_selected =
                !ids.is_empty() && ids.iter().all(|id| s.data_table.selected.contains(id));
            if everything_selected {
                for id in ids {
                    s.data_table.selected.remove(&id);
                }
            } else {
                s.data_table.selected.extend(ids);
            }
        },
    )
    .indeterminate(some_selected))];
    header_cells.push(sort_button(Column::Name, sort));
    for column in &visible {
        header_cells.push(sort_button(*column, sort));
    }
    header_cells.push(any(text("Actions").size(13.0)));

    let mut table_rows = vec![table_row(header_cells).selected(true)];

    // --- The body rows ---
    for person in &page_rows {
        let id = person.id;
        let is_selected = state.selected.contains(&id);
        let mut cells: Vec<frust::AnyView<AppState>> = vec![any(checkbox(
            is_selected,
            move |s: &mut AppState, next: bool| {
                if next {
                    s.data_table.selected.insert(id);
                } else {
                    s.data_table.selected.remove(&id);
                }
            },
        ))];
        cells.push(any(text(person.name.clone()).size(13.0)));
        for column in &visible {
            cells.push(match column {
                Column::Role => any(text(person.role.clone()).size(13.0)),
                Column::Status => {
                    any(badge(person.status.title()).variant(person.status.variant()))
                }
                Column::Amount => any(text(format!("{} \u{20AC}", person.amount)).size(13.0)),
                Column::Name => any(text(person.name.clone()).size(13.0)),
            });
        }
        let row_anchor = state
            .row_anchors
            .get(&id)
            .cloned()
            .unwrap_or_else(OverlayAnchor::new);
        cells.push(any(anchor(
            &row_anchor,
            button("\u{22EE}", move |s: &mut AppState| {
                s.data_table.row_menu_row = id;
                s.data_table.row_menu_open = true;
            })
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSm),
        )));
        table_rows.push(table_row(cells).selected(is_selected).on_click(
            move |s: &mut AppState| {
                if !s.data_table.selected.remove(&id) {
                    s.data_table.selected.insert(id);
                }
            },
        ));
    }

    let pages: Vec<PaginationItem> = (1..=page_count).map(PaginationItem::Page).collect();

    let body = ColumnOf(vec![
        any(crate::nav::heading("Data Table")),
        any(SizedBox(None, Some(8.0))),
        any(crate::nav::caption(
            "Every behaviour here is app code over `table`: the sort comparators, \
             the name filter, the page slice, the selection set and the column \
             set all live in this page's own state. Row click toggles selection; \
             a cell that handles its own press (the checkbox, the \u{22EE} button) \
             wins over the row.",
        )),
        any(SizedBox(None, Some(16.0))),
        any(toolbar),
        any(SizedBox(None, Some(12.0))),
        any(table(table_rows).caption(format!(
            "{selected_here} of {total} row(s) selected \u{2014} page {} of {page_count}",
            current + 1
        ))),
        any(SizedBox(None, Some(16.0))),
        any(pagination(
            pages,
            current + 1,
            |s: &mut AppState, page: usize| {
                s.data_table.page = page.saturating_sub(1);
            },
        )),
        any(SizedBox(None, Some(16.0))),
        any(separator()),
        any(SizedBox(None, Some(8.0))),
        any(crate::nav::caption(format!(
            "Last row action \u{2014} {}",
            state.last_action
        ))),
    ]);

    // --- The two anchored menus, kept mounted so they animate out ---
    let mut layers: Vec<frust::AnyView<AppState>> = vec![crate::scroll_slot(any(body))];

    let column_items = TOGGLEABLE
        .into_iter()
        .map(|column| dropdown_menu_item(column.title()).checked(state.visible.contains(&column)))
        .collect::<Vec<_>>();
    layers.push(any(dropdown_menu(
        std::iter::once(dropdown_menu_label("Toggle columns"))
            .chain(column_items)
            .collect(),
        |s: &mut AppState, index: usize| {
            // Index 0 is the label; the toggles follow it in order.
            if let Some(column) = index.checked_sub(1).and_then(|i| TOGGLEABLE.get(i))
                && !s.data_table.visible.remove(column)
            {
                s.data_table.visible.insert(*column);
            }
        },
    )
    .anchor(&state.columns_anchor)
    .open(state.columns_open)
    .on_open_change(|s: &mut AppState, open: bool| {
        s.data_table.columns_open = open;
    })));

    let menu_row = state.row_menu_row;
    let menu_anchor = state
        .row_anchors
        .get(&menu_row)
        .cloned()
        .unwrap_or_else(OverlayAnchor::new);
    layers.push(any(dropdown_menu(
        vec![
            dropdown_menu_item("View details"),
            dropdown_menu_item("Copy name").shortcut("\u{2318}C"),
            dropdown_menu_separator(),
            dropdown_menu_item("Delete row"),
        ],
        move |s: &mut AppState, index: usize| {
            let name = s
                .data_table
                .rows
                .iter()
                .find(|p| p.id == menu_row)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            s.data_table.last_action = match index {
                0 => format!("viewed {name}"),
                1 => format!("copied {name}"),
                3 => {
                    s.data_table.rows.retain(|p| p.id != menu_row);
                    s.data_table.selected.remove(&menu_row);
                    format!("deleted {name}")
                }
                _ => s.data_table.last_action.clone(),
            };
            s.data_table.row_menu_open = false;
        },
    )
    .anchor(&menu_anchor)
    .open(state.row_menu_open)
    .on_open_change(|s: &mut AppState, open: bool| {
        s.data_table.row_menu_open = open;
    })));

    Stack(layers)
}
