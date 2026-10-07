//! Drag section: three independent demos over one shared
//! [`DragCoordinator`] (`state.drag_coordinator`), each exercising a
//! different corner of `frust::drag` — type-gating keeps the three sessions
//! from ever being offered to each other's targets even though they share
//! one coordinator (see `frust::drag`'s module docs, *Target resolution*).
//!
//! # Kanban board
//!
//! Three columns ([`COLUMN_LABELS`]), each a scrollable
//! [`frust::drag_target`] (one [`frust::ScrollController`] per column, held
//! on [`crate::PlaygroundState::kanban_scroll_controllers`]) wrapped in
//! [`frust::auto_scroll_zone`] — the same composition that module's own doc
//! example shows. Every card is a [`frust::draggable`] carrying a
//! [`CardMove`] payload; dropping on a column's target calls [`move_card`],
//! relocating the card inside
//! [`crate::PlaygroundState::kanban_columns`] (a drop back onto the card's
//! own column is a no-op). The target's default highlight (a themed inset
//! stroke) is left unoverridden, so a hovered column visibly lights up.
//! Initiation, ghost, Esc-cancel, and keyboard lift/cycle/drop all come free
//! from `draggable()`/`drag_target()` themselves — see those modules' own
//! docs for the full contract. The card data lives on
//! [`crate::PlaygroundState`] rather than this page's own local state (the
//! `local_sig!` signals below): it is the one piece of this page's state a
//! drop actually *moves between containers*, so it needs the same
//! persistent-handle treatment `PlaygroundState::scroll_controller` already
//! gets for the "Scroll" section.
//!
//! # Reorderable list
//!
//! Ten rows over [`frust::reorderable_list`], keyed by a stable id (not
//! position) so a reorder carries each row's own retained state along with
//! it. [`frust::ReorderableListView::on_reorder`] reports the already
//! source-adjusted `(from, to)` pair (see that method's own module docs for
//! the index convention); this page applies it to the row order
//! ([`apply_reorder`]) and publishes it to a readout line.
//!
//! # Desktop file drop
//!
//! A single [`frust::drag_target`] over `Vec<PathBuf>` — the type an OS file
//! drag's `ExternalFiles` session carries (see `frust::drag`'s *OS file
//! drops* module docs). Every dropped path's file name (never its contents)
//! is appended to a running list. Untestable headlessly beyond construction
//! and mounting: desktop shells are the only ones that ever publish
//! `InputEvent::FileDrop` in the first place.

use std::path::PathBuf;

use frust::{
    Axis, ChildKey, Color, DragCoordinator, EdgeInsets, FlexChild, FlexView, Get, GetUntracked,
    Padding, RwSignal, ScrollController, Set, SizedBox, SourceFeedback, Theme, Update, View,
    auto_scroll_zone, column, container, drag_target, draggable, inflexible, reorderable_list,
    scroll_view, text, use_context,
};

use crate::PlaygroundState;

/// The kanban board's column headings, in [`crate::PlaygroundState::kanban_columns`]
/// index order.
const COLUMN_LABELS: [&str; 3] = ["Todo", "Doing", "Done"];

/// Each kanban column's fixed viewport size, in logical px — bounded the
/// same reason `pages::scroll_control`'s own `VIEWPORT_H` is: a nested
/// scrolling surface needs a real window to scroll within rather than the
/// unbounded height the outer page's own `scroll_view` would otherwise hand
/// it.
const COLUMN_WIDTH_PX: f64 = 180.0;
const COLUMN_HEIGHT_PX: f64 = 220.0;

/// Vertical gap between two cards in a column, and horizontal gap between
/// two columns.
const CARD_GAP_PX: f64 = 8.0;
const COLUMN_GAP_PX: f64 = 12.0;

/// How many rows the reorderable-list demo holds.
const REORDER_ROW_COUNT: usize = 10;

/// Each reorder row's fixed height, in logical px.
const REORDER_ROW_HEIGHT_PX: f64 = 40.0;

/// The file-drop zone's fixed height, in logical px.
const FILE_DROP_HEIGHT_PX: f64 = 96.0;

/// One kanban card: a stable identity ([`KanbanCard::id`]) carried as the
/// [`CardMove`] drag payload, and the label painted on its chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KanbanCard {
    pub id: u32,
    pub label: &'static str,
}

/// The kanban board's starting layout: three columns ([`COLUMN_LABELS`])
/// seeded with a handful of generic household-chore cards — enough to
/// demonstrate a cross-column drop without needing any app-domain data
/// model. Called once from [`crate::PlaygroundState::new`].
pub(crate) fn initial_kanban_columns() -> Vec<Vec<KanbanCard>> {
    vec![
        vec![
            KanbanCard {
                id: 0,
                label: "Water the plants",
            },
            KanbanCard {
                id: 1,
                label: "Buy groceries",
            },
            KanbanCard {
                id: 2,
                label: "Write postcards",
            },
        ],
        vec![KanbanCard {
            id: 3,
            label: "Paint the fence",
        }],
        vec![
            KanbanCard {
                id: 4,
                label: "Walk the dog",
            },
            KanbanCard {
                id: 5,
                label: "Pay the bills",
            },
        ],
    ]
}

/// The typed payload a kanban card's [`draggable()`](frust::draggable) source
/// hands to the coordinator — just the dragged card's [`KanbanCard::id`],
/// looked back up in [`move_card`] rather than carrying the whole card (the
/// coordinator's payload is a boxed `Any`; smaller is cheaper to box).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CardMove {
    id: u32,
}

/// Relocate the card with `id` into column `to_column`, wherever it
/// currently sits. A drop back onto the card's own column is a no-op,
/// mirroring [`frust::ReorderableListView::on_reorder`]'s own "dropping at
/// the current slot changes nothing" precedent. Silently does nothing if
/// `id` is not found anywhere (defensive: every registered source's payload
/// is a real card id, so this should never happen).
fn move_card(state: &mut PlaygroundState, id: u32, to_column: usize) {
    state.kanban_columns.update(|columns| {
        let Some((from_column, row_index)) =
            columns.iter().enumerate().find_map(|(index, column)| {
                column
                    .iter()
                    .position(|card| card.id == id)
                    .map(|row| (index, row))
            })
        else {
            return;
        };
        if from_column == to_column {
            return;
        }
        let card = columns[from_column].remove(row_index);
        columns[to_column].push(card);
    });
}

/// A `thread_local!`-cached, self-healing-across-a-disposed-owner page-local
/// signal — the established per-module precedent (`database.rs`/
/// `url_launcher.rs`/`platform_views.rs`/`auth_session.rs` each carry an
/// identical copy), used here for the reorder/file-drop readouts that, unlike
/// [`crate::PlaygroundState::kanban_columns`], nothing outside this page ever
/// needs to reach.
macro_rules! local_sig {
    ($name:ident, $ty:ty, $init:expr) => {
        fn $name() -> RwSignal<$ty> {
            thread_local! {
                static SLOT: std::cell::RefCell<Option<RwSignal<$ty>>> =
                    const { std::cell::RefCell::new(None) };
            }
            SLOT.with(|cell| {
                if let Some(sig) = *cell.borrow()
                    && sig.try_get_untracked().is_some()
                {
                    return sig;
                }
                let sig = RwSignal::new($init);
                *cell.borrow_mut() = Some(sig);
                sig
            })
        }
    };
}

// The reorder section's current row order (identities, not positions) — ten
// ids, initially 0..10 in order.
local_sig!(
    reorder_order_sig,
    Vec<u32>,
    (0..REORDER_ROW_COUNT as u32).collect()
);

// The reorder section's last reported (from, to) move, or `None` before the
// first drop that actually moved something.
local_sig!(reorder_last_move_sig, Option<(usize, usize)>, None);

// The file-drop section's accumulated dropped paths — only ever painted as
// file names (see `page`'s own listing), never read for content.
local_sig!(dropped_files_sig, Vec<PathBuf>, Vec::new());

/// Apply a reorderable-list drop's already source-adjusted `(from, to)` pair
/// (see [`frust::ReorderableListView::on_reorder`]'s own module docs for the
/// convention) to `order`: pull the id out of `from` and reinsert it at `to`.
fn apply_reorder(order: &mut Vec<u32>, from: usize, to: usize) {
    let moved = order.remove(from);
    order.insert(to, moved);
}

/// The live theme colors a kanban column needs, grouped so
/// [`kanban_column`] stays under clippy's argument-count limit.
#[derive(Clone, Copy)]
struct ColumnColors {
    accent: Color,
    muted: Color,
    card_fill: Color,
}

/// One card's chip: its label over a themed container fill.
fn card_chip(card: KanbanCard, colors: ColumnColors) -> impl View<PlaygroundState> {
    container(Padding(
        EdgeInsets::all(8.0),
        text(card.label).size(12.0).color(colors.accent),
    ))
    .fill(colors.card_fill)
    .radius(6.0)
}

/// Build column `to_column`: its heading, its cards (each a drag source
/// carrying a [`CardMove`]), inside a scrollable [`frust::drag_target`]
/// wrapped in [`frust::auto_scroll_zone`] — see the [module docs](self#kanban-board).
fn kanban_column(
    to_column: usize,
    label: &'static str,
    cards: &[KanbanCard],
    coordinator: DragCoordinator,
    controller: ScrollController,
    colors: ColumnColors,
) -> impl View<PlaygroundState> {
    let mut children: Vec<FlexChild<PlaygroundState>> = Vec::with_capacity(cards.len() * 2);
    for card in cards {
        let id = card.id;
        let chip = card_chip(*card, colors);
        let drag_card = draggable(chip, coordinator.clone(), move |_: &PlaygroundState| {
            CardMove { id }
        })
        .while_dragging(SourceFeedback::Dim);
        children.push(inflexible(drag_card));
        children.push(inflexible(SizedBox::<PlaygroundState>(
            None,
            Some(CARD_GAP_PX),
        )));
    }
    let column_body = Padding(
        EdgeInsets::all(8.0),
        FlexView::new(Axis::Vertical, children),
    );
    let scroll_surface = SizedBox::<PlaygroundState>(Some(COLUMN_WIDTH_PX), Some(COLUMN_HEIGHT_PX))
        .child(scroll_view(column_body).controller(controller.clone()));
    let target = drag_target::<CardMove, PlaygroundState, _>(scroll_surface, coordinator.clone())
        .on_drop(move |state: &mut PlaygroundState, moved: CardMove| {
            move_card(state, moved.id, to_column);
        });
    let zoned = auto_scroll_zone(target, coordinator, controller);
    column()
        .child(text(label).size(12.0).color(colors.muted))
        .child(SizedBox::<PlaygroundState>(None, Some(4.0)))
        .child(zoned)
}

/// One reorder row's content: its id-derived label, fixed to
/// [`REORDER_ROW_HEIGHT_PX`] so the list's drop gaps land at predictable
/// positions.
fn reorder_row_view(id: u32, accent: Color) -> impl View<PlaygroundState> {
    SizedBox::<PlaygroundState>(None, Some(REORDER_ROW_HEIGHT_PX)).child(Padding(
        EdgeInsets::symmetric(12.0, 8.0),
        text(format!("Item {}", id + 1)).size(12.0).color(accent),
    ))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let accent = scheme.primary;
    let muted = scheme.on_surface_variant;
    let card_fill = scheme.surface_container_high;

    let coordinator = state.drag_coordinator.clone();
    let colors = ColumnColors {
        accent,
        muted,
        card_fill,
    };

    // --- Kanban board ---
    let columns = state.kanban_columns.get();
    let column_views: Vec<FlexChild<PlaygroundState>> = COLUMN_LABELS
        .iter()
        .enumerate()
        .flat_map(|(index, label)| {
            let controller = state.kanban_scroll_controllers[index].clone();
            let column = kanban_column(
                index,
                label,
                &columns[index],
                coordinator.clone(),
                controller,
                colors,
            );
            vec![
                inflexible(column),
                inflexible(SizedBox::<PlaygroundState>(Some(COLUMN_GAP_PX), None)),
            ]
        })
        .collect();
    let kanban_row = FlexView::new(Axis::Horizontal, column_views);

    // --- Reorderable list ---
    let reorder_rows: Vec<(ChildKey, _)> = reorder_order_sig()
        .get()
        .iter()
        .map(|&id| (ChildKey::new(id), reorder_row_view(id, accent)))
        .collect();
    let reorder_list = reorderable_list(coordinator.clone(), reorder_rows).on_reorder(
        |_state: &mut PlaygroundState, from, to| {
            reorder_order_sig().update(|order| apply_reorder(order, from, to));
            reorder_last_move_sig().set(Some((from, to)));
        },
    );
    let reorder_readout = match reorder_last_move_sig().get() {
        Some((from, to)) => format!("Last move: {from} -> {to}"),
        None => "No reorder yet".to_string(),
    };

    // --- Desktop file drop ---
    let dropped = dropped_files_sig().get();
    let file_drop_zone = SizedBox::<PlaygroundState>(None, Some(FILE_DROP_HEIGHT_PX)).child(
        container(Padding(
            EdgeInsets::all(12.0),
            text("Drop files here (desktop only)")
                .size(12.0)
                .color(muted),
        ))
        .fill(scheme.surface_container)
        .border(scheme.outline, 1.0),
    );
    let file_target =
        drag_target::<Vec<PathBuf>, PlaygroundState, _>(file_drop_zone, coordinator.clone())
            .on_drop(|_state: &mut PlaygroundState, paths: Vec<PathBuf>| {
                dropped_files_sig().update(|files| files.extend(paths));
            });
    let dropped_lines: Vec<FlexChild<PlaygroundState>> = if dropped.is_empty() {
        vec![inflexible(
            text("No files dropped yet").size(11.0).color(muted),
        )]
    } else {
        dropped
            .iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                inflexible(text(name).size(11.0).color(muted))
            })
            .collect()
    };

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(text("Drag").size(13.0).color(accent))
            .child(
                text(
                    "Three drag-and-drop demos sharing one coordinator: a kanban board \
                         (pointer drag or long-press on touch, edge auto-scroll, Esc cancels), \
                         a keyboard-reorderable list (Enter/Space lifts, arrow keys cycle, \
                         Enter/Space drops), and a desktop OS file-drop zone.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox::<PlaygroundState>(None, Some(12.0)))
            .child(kanban_row)
            .child(SizedBox::<PlaygroundState>(None, Some(16.0)))
            .child(text("Reorder").size(12.0).color(muted))
            .child(SizedBox::<PlaygroundState>(None, Some(4.0)))
            .child(reorder_list)
            .child(SizedBox::<PlaygroundState>(None, Some(8.0)))
            .child(text(reorder_readout).size(12.0).color(muted))
            .child(SizedBox::<PlaygroundState>(None, Some(16.0)))
            .child(text("File drop").size(12.0).color(muted))
            .child(SizedBox::<PlaygroundState>(None, Some(4.0)))
            .child(file_target)
            .child(SizedBox::<PlaygroundState>(None, Some(8.0)))
            .child(FlexView::new(Axis::Vertical, dropped_lines)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_reactive::ReactiveRuntime;
    use reactive_graph::owner::Owner;

    /// Install the reactive runtime + an ambient owner (needed for
    /// `RwSignal`/`PlaygroundState::new`) — mirrors `pages::keys`'/
    /// `pages::terminal`'s own identical `setup()`.
    fn setup() -> Owner {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let ambient = Owner::new();
        ambient.set();
        ambient
    }

    #[test]
    fn apply_reorder_moves_the_source_to_the_adjusted_destination() {
        let mut order = vec![10, 11, 12, 13];
        apply_reorder(&mut order, 0, 2);
        assert_eq!(order, vec![11, 12, 10, 13]);
    }

    #[test]
    fn apply_reorder_handles_a_move_toward_the_front() {
        let mut order = vec![10, 11, 12, 13];
        apply_reorder(&mut order, 3, 0);
        assert_eq!(order, vec![13, 10, 11, 12]);
    }

    #[test]
    fn move_card_relocates_a_card_into_the_target_column() {
        let _owner = setup();
        let mut state = PlaygroundState::new();
        move_card(&mut state, 0, 2);
        let columns = state.kanban_columns.get_untracked();
        assert!(
            !columns[0].iter().any(|card| card.id == 0),
            "card 0 must leave its original column"
        );
        assert!(
            columns[2].iter().any(|card| card.id == 0),
            "card 0 must land in the target column"
        );
    }

    #[test]
    fn move_card_onto_its_own_column_is_a_no_op() {
        let _owner = setup();
        let mut state = PlaygroundState::new();
        let before = state.kanban_columns.get_untracked();
        move_card(&mut state, 0, 0);
        let after = state.kanban_columns.get_untracked();
        assert_eq!(
            before, after,
            "dropping on the card's own column changes nothing"
        );
    }

    #[test]
    fn move_card_with_an_unknown_id_does_nothing() {
        let _owner = setup();
        let mut state = PlaygroundState::new();
        let before = state.kanban_columns.get_untracked();
        move_card(&mut state, 999, 1);
        let after = state.kanban_columns.get_untracked();
        assert_eq!(
            before, after,
            "an unknown card id must not panic or mutate anything"
        );
    }
}
