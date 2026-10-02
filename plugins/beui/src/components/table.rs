//! Ports beUI's `table` component.
//!
//! **Source:** `components/motion/table/` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01 —
//! `index.tsx` (the virtualized body), `table-header.tsx` (the head row and its
//! sort affordance), `types.ts` (the column/props vocabulary), `utils.ts` and
//! `use-column-sort.ts`.
//!
//! | upstream | here |
//! |---|---|
//! | `useVirtualizer({ count, estimateSize: () => rowHeight, overscan })` | [`frust::ListView`], keyed with a variable extent |
//! | `rowHeight = 48`, `height = 440` | [`TABLE_ROW_HEIGHT`], [`TABLE_VIEWPORT_HEIGHT`] |
//! | `minColumnWidth = 64`, `<colgroup>` + `table-layout: fixed` | [`TABLE_MIN_COLUMN_WIDTH`], [`table_column_widths`] |
//! | head row `sticky top-0 bg-muted border-b font-medium text-muted-foreground` | the header band this widget paints itself |
//! | head cell `px-4`, body cell `truncate px-4 text-foreground` | [`TABLE_CELL_PADDING`] |
//! | sort chevron `rotate: desc ? 180 : 0`, `opacity: active ? 1 : 0.35`, `0.18s EASE_OUT` | [`TABLE_SORT_TIMING`], [`TABLE_SORT_IDLE_OPACITY`] |
//! | `toggleSort`: asc → desc → none | [`table_next_sort`] |
//! | row `border-border/60 border-b hover:bg-muted/50 data-[selected=true]:bg-primary/5` | [`TABLE_BORDER_ALPHA`], [`TABLE_HOVER_WASH`], [`TABLE_SELECTED_WASH`] |
//! | `onEndReached` at `rowHeight * 4` from the bottom | [`TableView::on_end_reached`], [`TABLE_END_REACHED_ROWS`] |
//! | `emptyState = "No data"` | [`TableView::empty_label`], [`TABLE_EMPTY_LABEL`] |
//!
//! # It rides `ListView`, and the header stays outside it
//!
//! Upstream's virtualization is `@tanstack/react-virtual` measuring a scroll
//! `div` and emitting padding rows; that library is not carried, and frust
//! already owns the substrate it stands for — [`frust::ListView`], whose
//! windowing contract (children materialize **only** at rebuild, inside
//! `[first − BUFFER, last + BUFFER]`) is what keeps a 10k-row table honest. Two
//! findings from that contract shaped this port:
//!
//! - **`ListView` has no sticky row.** There is no header concept in it at all:
//!   every slot is an item index, and item 0 scrolls like any other. So the head
//!   row is *not* row 0 — this widget paints it itself, above the list's box, and
//!   is therefore sticky by construction rather than by a CSS rule. That also
//!   sidesteps the second finding.
//! - **A uniform-extent list tight-constrains every row.**
//!   `ListView::builder`'s rows are laid out under
//!   `BoxConstraints::tight(width, item_extent)`, so a header row placed inside
//!   one could never be taller (or shorter) than a body row, whatever it asked
//!   for. `ListView::builder_keyed(..).estimated_item_extent(px)` is the other
//!   path: rows are laid out under `(width, 0)..(width, ∞)` and size themselves.
//!   This port takes that one — **keyed and variable-extent** — so a row keeps
//!   its hover/press state across a re-sort (index identity would reattach it to
//!   whatever now sits at the index) and so a caller's taller row height is
//!   honoured rather than clipped.
//!
//! Both paths hand a row a *tight width*, which is what lets
//! [`table_column_widths`] be a pure function rather than shared mutable state:
//! the header and every row resolve the same columns against the same width and
//! independently arrive at the same ladder. No row can see its siblings, and
//! none needs to.
//!
//! # Rows animate in, and cannot animate out
//!
//! A row's entrance is staged by [`crate::motion::Presence`], but only for the
//! rows that materialize on a frame where the row **count** changed. That gate
//! is the honest reading of the contract: a virtualized list cannot distinguish
//! "this row entered the data" from "this row scrolled into the window" — both
//! are a fresh pod from a builder call — so an ungated entrance would flash
//! every row a fling brings into view. On the frame a row is inserted, the
//! surviving keys around it keep their pods and exactly the new key builds
//! fresh, so exactly the inserted row plays. On a removal the departing key is
//! torn down by the list *immediately*: **there is no exit**, because the pod is
//! gone before a ramp could run and this widget has no way to keep it — the
//! kept-mounted trick `Presence` documents needs a mount to keep, and the
//! mounting decision here belongs to `ListView`.
//!
//! # What the shadcn port's boundaries settled
//!
//! Where upstream's markup generality has no frust analogue this follows the
//! sibling catalog's table rather than inventing a second answer
//! (`plugins/shadcn/src/components/table.rs`, and `docs/LIMITATIONS.md`'s
//! `shadcn-otp-table-button-api-gaps`):
//!
//! - **Head cells are label strings, not views**, so a select-all checkbox or a
//!   menu trigger cannot live in the header row. That is what removes upstream's
//!   whole `selectable` column: row selection survives as
//!   [`TableView::selected`] + [`TableView::on_row_press`], the header half does
//!   not.
//! - **Body cells are strings too**, read through one
//!   `cell(row, column) -> String` provider rather than a per-column
//!   `ReactNode` renderer — the provider shape is what keeps the data lazy, and
//!   an `AnyView` per cell would mean materializing 10k rows' worth of views to
//!   build the table at all.
//! - **Horizontal overflow scales instead of scrolling.** `ScrollView` is
//!   vertical-only, so columns wider than the available box are shrunk
//!   proportionally (see [`table_column_widths`]) rather than panned, exactly as
//!   the shadcn port records.
//!
//! # Degradations against the web original
//!
//! - **No column resize or reorder.** `resizable`/`reorderable` are pointer
//!   drags on a header grip that write a per-column width or order map;
//!   [`table_column_widths`] is deliberately a pure function of the column defs,
//!   so either would have to become owner state and a new controlled prop pair.
//! - **No editable cells and no column rename.** `editable`/`onCellEdit`/
//!   `onColumnRename` host an `<input>` inside a cell; cells here are shaped
//!   runs, with no view slot to put a field in.
//! - **No row or column menus.** `RowHandle`/`TableMenu` are portaled hover
//!   handles (insert before/after, delete) — a portal this tier does not have,
//!   over an icon set it does not carry.
//! - **No skeleton rows.** `loading` swaps the body for shimmering placeholders;
//!   [`TableView::on_end_reached`] ports the infinite-scroll signal itself, and
//!   an owner shows its own placeholder rows through the same `cell` provider.
//! - **One line per cell, clipped.** Upstream `truncate`s with an ellipsis; the
//!   catalog's shaped runs are single-line and are clipped to their column.
//! - **A bounded viewport is required.** Virtualization needs a viewport to
//!   window against, so in an unbounded-height context (inside a
//!   [`frust::scroll_view`], say) the body falls back to
//!   [`TABLE_VIEWPORT_HEIGHT`] — upstream's own `height` default — rather than
//!   growing to the content and materializing everything.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, ListView, Theme};

use crate::motion::{Presence, Ramp};
use crate::press::{Lane, presses};
use crate::style::{self, with_alpha};
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// A row's height, in logical px (`rowHeight = 48`). Also the head row's, which
/// upstream sizes from the same prop.
pub const TABLE_ROW_HEIGHT: f64 = 48.0;

/// The scroll viewport's height when the table is given no bounded one, in
/// logical px (`height = 440`).
pub const TABLE_VIEWPORT_HEIGHT: f64 = 440.0;

/// The narrowest a column may be resolved to before the whole ladder is scaled,
/// in logical px (`minColumnWidth = 64`).
pub const TABLE_MIN_COLUMN_WIDTH: f64 = 64.0;

/// A cell's horizontal padding, in logical px (`px-4` on head and body cells).
pub const TABLE_CELL_PADDING: f64 = 16.0;

/// The gap between a sortable header's label and its chevron, in logical px
/// (`gap-1`).
pub const TABLE_HEAD_GAP: f64 = 4.0;

/// The sort chevron's box, in logical px (`h-3.5 w-3.5`).
pub const TABLE_CHEVRON_SIZE: f64 = 14.0;

/// How many rows from the end [`TableView::on_end_reached`] fires at
/// (`scrollHeight - scrollTop - clientHeight < rowHeight * 4`).
pub const TABLE_END_REACHED_ROWS: f64 = 4.0;

/// The body's placeholder when there are no rows (`emptyState = "No data"`).
pub const TABLE_EMPTY_LABEL: &str = "No data";

// ---- Chrome ----------------------------------------------------------------

/// Alpha of a row's hairline (`border-border/60`).
pub const TABLE_BORDER_ALPHA: f32 = 0.6;

/// Alpha of a hovered row's wash (`hover:bg-muted/50`).
pub const TABLE_HOVER_WASH: f32 = 0.5;

/// Alpha of a selected row's wash (`data-[selected=true]:bg-primary/5`).
pub const TABLE_SELECTED_WASH: f32 = 0.05;

/// Opacity of an inactive column's sort chevron (`opacity: active ? 1 : 0.35`).
pub const TABLE_SORT_IDLE_OPACITY: f32 = 0.35;

/// The sort chevron's rotation/fade timing (`{ duration: 0.18, ease: EASE_OUT }`).
pub const TABLE_SORT_TIMING: Ramp = Ramp::eased(Duration::from_millis(180), EASE_OUT);

/// A row's entrance ramp — the catalog's default entrance timing, applied to a
/// motion upstream's table does not have (see the [module docs](self)).
pub const TABLE_ROW_ENTER: Ramp = Ramp::eased(Duration::from_millis(180), EASE_OUT);

/// How far a row rises through its entrance, in logical px.
pub const TABLE_ROW_ENTER_RISE: f64 = 8.0;

/// Stroke width of a painted chevron, in logical px.
const CHEVRON_STROKE: f64 = 1.75;

/// Unthemed fallback head fill — the light table's `--muted`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted;
/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_INK: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED_INK: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback accent — the light table's `--primary`.
const FALLBACK_PRIMARY: Color = crate::BEUI_LIGHT.primary;
/// Unthemed fallback surface — the light table's `--background`.
const FALLBACK_SURFACE: Color = crate::BEUI_LIGHT.background;

// ---- Public vocabulary -----------------------------------------------------

/// A cell's horizontal alignment — upstream's `TableColumn.align`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TableAlign {
    /// `"left"`: the default.
    #[default]
    Start,
    /// `"center"`.
    Center,
    /// `"right"`.
    End,
}

impl TableAlign {
    /// Where a `content`-wide run starts inside a `available`-wide cell box.
    fn offset(self, available: f64, content: f64) -> f64 {
        match self {
            TableAlign::Start => 0.0,
            TableAlign::Center => ((available - content) / 2.0).max(0.0),
            TableAlign::End => (available - content).max(0.0),
        }
    }
}

/// Which way a sorted column runs — upstream's `SortDirection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableSortDirection {
    /// `"asc"`: the chevron points up.
    Ascending,
    /// `"desc"`: the chevron is turned 180°.
    Descending,
}

/// Which column the table is sorted by, and how — upstream's `SortState`, keyed
/// by column *index* rather than by its string key (a frust column has no
/// separate identity from its position in the `Vec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableSort {
    /// The sorted column's index.
    pub column: usize,
    /// Which way it runs.
    pub direction: TableSortDirection,
}

impl TableSort {
    /// The ascending sort on `column`.
    pub const fn ascending(column: usize) -> Self {
        TableSort {
            column,
            direction: TableSortDirection::Ascending,
        }
    }

    /// The descending sort on `column`.
    pub const fn descending(column: usize) -> Self {
        TableSort {
            column,
            direction: TableSortDirection::Descending,
        }
    }
}

/// What pressing `column`'s header asks for next — upstream's `toggleSort`:
/// a new column starts ascending, an ascending one flips to descending, and a
/// descending one clears the sort entirely.
pub fn table_next_sort(current: Option<TableSort>, column: usize) -> Option<TableSort> {
    match current {
        Some(sort) if sort.column == column => match sort.direction {
            TableSortDirection::Ascending => Some(TableSort::descending(column)),
            TableSortDirection::Descending => None,
        },
        _ => Some(TableSort::ascending(column)),
    }
}

/// One column's definition — upstream's `TableColumn`, minus the parts with no
/// frust analogue (see the [module docs](self)' degradations).
#[derive(Clone, Debug, PartialEq)]
pub struct TableColumn {
    header: String,
    width: Option<f64>,
    align: TableAlign,
    sortable: bool,
}

/// Declare a column headed `header`, sharing the leftover width equally with
/// every other column that declares no width of its own.
pub fn table_column(header: impl Into<String>) -> TableColumn {
    TableColumn {
        header: header.into(),
        width: None,
        align: TableAlign::default(),
        sortable: false,
    }
}

impl TableColumn {
    /// Pin this column's width, in logical px (`column.width`).
    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width.max(0.0));
        self
    }

    /// Align this column's cells (`column.align`).
    pub fn align(mut self, align: TableAlign) -> Self {
        self.align = align;
        self
    }

    /// Let pressing this column's header sort by it (`column.sortable`).
    pub fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// This column's header label.
    pub fn header(&self) -> &str {
        &self.header
    }

    /// Whether this column can be sorted by.
    pub fn is_sortable(&self) -> bool {
        self.sortable
    }
}

/// Resolve every column's width against `available`, in logical px.
///
/// The three rules, in order, are upstream's `<colgroup>` + `table-layout:
/// fixed` behaviour restated arithmetically:
///
/// 1. a column that declared a width keeps it;
/// 2. the rest share whatever is left equally, provided each still clears
///    [`TABLE_MIN_COLUMN_WIDTH`] — otherwise every undeclared column falls back
///    to that minimum, which is upstream's own `minTableWidth` floor;
/// 3. if the ladder is then wider than `available` it is scaled down
///    **proportionally**, since there is no horizontal scroll to pan it with —
///    the boundary the sibling shadcn table records for the same reason.
///
/// Pure and total: no allocation beyond the result, defined for an empty column
/// list and for a non-positive width.
pub fn table_column_widths(columns: &[TableColumn], available: f64) -> Vec<f64> {
    if columns.is_empty() {
        return Vec::new();
    }
    let declared: f64 = columns.iter().filter_map(|column| column.width).sum();
    let flexible = columns
        .iter()
        .filter(|column| column.width.is_none())
        .count();
    let remaining = available - declared;
    let share = if flexible > 0 {
        remaining / flexible as f64
    } else {
        0.0
    };
    let mut widths: Vec<f64> = if share >= TABLE_MIN_COLUMN_WIDTH {
        columns
            .iter()
            .map(|column| column.width.unwrap_or(share))
            .collect()
    } else {
        columns
            .iter()
            .map(|column| column.width.unwrap_or(TABLE_MIN_COLUMN_WIDTH))
            .collect()
    };
    let total: f64 = widths.iter().sum();
    if total > available && total > 0.0 {
        let scale = (available / total).max(0.0);
        for width in &mut widths {
            *width *= scale;
        }
    }
    widths
}

/// The left edge of each column, given the resolved `widths`.
fn column_offsets(widths: &[f64]) -> Vec<f64> {
    let mut offsets = Vec::with_capacity(widths.len());
    let mut x = 0.0;
    for width in widths {
        offsets.push(x);
        x += width;
    }
    offsets
}

/// A view-held cell provider: the text of row `row`'s column `column`.
type CellProvider = Rc<dyn Fn(usize, usize) -> String>;
/// A view-held row-identity provider.
type RowKeyProvider = Rc<dyn Fn(usize) -> u64>;
/// A view-held row-selection predicate.
type RowSelected = Rc<dyn Fn(usize) -> bool>;
/// A view-held, typed row callback (erased on build).
type OnRow<State> = Rc<dyn Fn(&mut State, usize)>;
/// A view-held, typed sort callback (erased on build).
type OnSort<State> = Rc<dyn Fn(&mut State, Option<TableSort>)>;
/// A view-held, typed edge callback.
type OnEdge<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI data table. See the [module docs](self).
pub struct TableView<State: 'static> {
    columns: Rc<Vec<TableColumn>>,
    row_count: usize,
    cell: CellProvider,
    row_key: Option<RowKeyProvider>,
    selected: Option<RowSelected>,
    row_height: f64,
    viewport: Option<f64>,
    sort: Option<TableSort>,
    empty_label: String,
    on_sort: Option<OnSort<State>>,
    on_row_press: Option<OnRow<State>>,
    on_end_reached: Option<OnEdge<State>>,
}

/// Build a table of `row_count` rows over `columns`, reading each cell's text
/// from `cell(row, column)`.
///
/// The provider is what keeps the table lazy: it is called only for the rows
/// [`frust::ListView`] has actually materialized, so a 10k-row table asks for a
/// screenful of cells per frame and never for the rest.
pub fn table<State: 'static, F: Fn(usize, usize) -> String + 'static>(
    columns: Vec<TableColumn>,
    row_count: usize,
    cell: F,
) -> TableView<State> {
    TableView {
        columns: Rc::new(columns),
        row_count,
        cell: Rc::new(cell),
        row_key: None,
        selected: None,
        row_height: TABLE_ROW_HEIGHT,
        viewport: None,
        sort: None,
        empty_label: TABLE_EMPTY_LABEL.to_string(),
        on_sort: None,
        on_row_press: None,
        on_end_reached: None,
    }
}

impl<State: 'static> TableView<State> {
    /// Give each row a stable id (upstream's `getRowId`), so a row keeps its
    /// hover/press state when a sort or a filter moves it to another index.
    ///
    /// Without this the row's identity is its index, which is correct for
    /// append-only and full-replace data and wrong for a re-sort — the same
    /// split [`frust::ListView`]'s two row-identity models draw.
    pub fn row_key<F: Fn(usize) -> u64 + 'static>(mut self, row_key: F) -> Self {
        self.row_key = Some(Rc::new(row_key));
        self
    }

    /// Mark rows selected (`selectedRowIds`), by index.
    pub fn selected<F: Fn(usize) -> bool + 'static>(mut self, selected: F) -> Self {
        self.selected = Some(Rc::new(selected));
        self
    }

    /// Set the row height, in logical px (`rowHeight`).
    pub fn row_height(mut self, row_height: f64) -> Self {
        self.row_height = row_height.max(1.0);
        self
    }

    /// Pin the scrolling body's height, in logical px (`height`). Without it the
    /// body takes whatever bounded height the table is given, falling back to
    /// [`TABLE_VIEWPORT_HEIGHT`].
    pub fn viewport_height(mut self, height: f64) -> Self {
        self.viewport = Some(height.max(0.0));
        self
    }

    /// The current sort (`sort`) — **controlled**: the table paints this and
    /// reports the next one through [`TableView::on_sort`], never sorting the
    /// data itself (it cannot; the data is behind the caller's provider).
    pub fn sort(mut self, sort: Option<TableSort>) -> Self {
        self.sort = sort;
        self
    }

    /// Replace the empty-body placeholder (`emptyState`).
    pub fn empty_label(mut self, label: impl Into<String>) -> Self {
        self.empty_label = label.into();
        self
    }

    /// Report the sort a header press asks for (`onSortChange`), already cycled
    /// through [`table_next_sort`].
    pub fn on_sort<F: Fn(&mut State, Option<TableSort>) + 'static>(mut self, on_sort: F) -> Self {
        self.on_sort = Some(Rc::new(on_sort));
        self
    }

    /// Report a press on a row, by index.
    pub fn on_row_press<F: Fn(&mut State, usize) + 'static>(mut self, on_press: F) -> Self {
        self.on_row_press = Some(Rc::new(on_press));
        self
    }

    /// Fire when the body scrolls within [`TABLE_END_REACHED_ROWS`] rows of the
    /// end (`onEndReached`) — the infinite-scroll signal, carried straight
    /// through to [`frust::ListView::on_near_end`].
    pub fn on_end_reached<F: Fn(&mut State) + 'static>(mut self, on_end_reached: F) -> Self {
        self.on_end_reached = Some(Rc::new(on_end_reached));
        self
    }

    /// The virtualized body: the keyed, variable-extent [`frust::ListView`] this
    /// table's rows live in.
    ///
    /// `enter` is passed to every row built on this pass — see the
    /// [module docs](self) on why only a count change stages an entrance.
    fn body(&self, enter: bool) -> AnyView<State> {
        let columns = Rc::clone(&self.columns);
        let cell = Rc::clone(&self.cell);
        let selected = self.selected.clone();
        let on_press = self.on_row_press.clone();
        let row_height = self.row_height;
        let row_count = self.row_count;
        let key_of = self.row_key.clone();

        let key = move |index: usize| match &key_of {
            Some(key_of) => ChildKey::new(key_of(index)),
            None => ChildKey::new(index),
        };
        let builder = move |index: usize| {
            let values = (0..columns.len())
                .map(|column| cell(index, column))
                .collect();
            any(TableRowView {
                columns: Rc::clone(&columns),
                values,
                row: index,
                selected: selected.as_ref().is_some_and(|is| is(index)),
                height: row_height,
                last: index + 1 == row_count,
                enter,
                on_press: on_press.clone(),
            })
        };

        let mut list = ListView::builder_keyed(row_count, row_height, key, builder)
            .estimated_item_extent(row_height);
        if let Some(on_end_reached) = self.on_end_reached.clone() {
            list = list.on_near_end(
                move |state: &mut State| on_end_reached(state),
                row_height * TABLE_END_REACHED_ROWS,
            );
        }
        any(list)
    }
}

/// The head band's resolved palette.
struct TableColors {
    /// The head row's fill (`bg-muted`).
    head: Color,
    /// The body's fill (`bg-background`).
    body: Color,
    /// Every hairline (`border-border`).
    border: Color,
    /// A head label and the empty placeholder (`text-muted-foreground`).
    muted: Color,
    /// An active head label and every body cell (`text-foreground`).
    ink: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> TableColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            TableColors {
                head: s.surface_container_highest,
                body: s.surface,
                border: s.outline_variant,
                muted: s.on_surface_variant,
                ink: s.on_surface,
            }
        }
        None => TableColors {
            head: FALLBACK_MUTED,
            body: FALLBACK_SURFACE,
            border: FALLBACK_BORDER,
            muted: FALLBACK_MUTED_INK,
            ink: FALLBACK_INK,
        },
    }
}

/// The head-label style (`font-medium`), shared with the empty placeholder, in
/// the theme's `label_large` family.
fn head_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK)
    };
    themed_style(style, ThemeTextType::LabelLarge, theme)
}

/// The body-cell style (`text-sm`, regular weight), in the theme's
/// `body_medium` family.
fn cell_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK);
    themed_style(style, ThemeTextType::BodyMedium, theme)
}

/// Paint a chevron pointing up, turned `angle` radians about `centre`.
fn draw_chevron(scene: &mut dyn PaintScene, centre: Point, angle: f64, color: Color) {
    let arm = TABLE_CHEVRON_SIZE * 0.32;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, arm * 0.5));
    path.line_to(Point::new(0.0, -arm * 0.5));
    path.line_to(Point::new(arm, arm * 0.5));
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    scene.stroke_path(Point::ZERO, &path, CHEVRON_STROKE, &Brush::Solid(color));
    scene.pop_transform();
}

/// One header cell's retained state.
struct HeadCell {
    label: LabelRun,
    /// `0` while the column sorts ascending, `1` descending — the chevron's
    /// half-turn.
    turn: Lane,
    /// `0` while the column is unsorted, `1` while it is — the chevron's fade.
    active: Lane,
}

impl<State: 'static> View<State> for TableView<State> {
    type Element = TableWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TableWidget {
        let sort = self.sort;
        TableWidget {
            head: self
                .columns
                .iter()
                .enumerate()
                .map(|(index, column)| HeadCell {
                    label: LabelRun::new(column.header.clone()),
                    turn: Lane::at_rest(TABLE_SORT_TIMING, turn_of(sort, index)),
                    active: Lane::at_rest(TABLE_SORT_TIMING, active_of(sort, index)),
                })
                .collect(),
            columns: Rc::clone(&self.columns),
            empty: LabelRun::new(self.empty_label.clone()),
            row_count: self.row_count,
            row_height: self.row_height,
            viewport: self.viewport,
            sort,
            body: build_child(&self.body(false), ctx),
            widths: Vec::new(),
            size: Size::ZERO,
            head_height: self.row_height,
            hovered_column: None,
            armed_column: None,
            on_sort: self.on_sort.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TableWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_sort = self.on_sort.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        if prev.columns != self.columns {
            element.head = self
                .columns
                .iter()
                .enumerate()
                .map(|(index, column)| HeadCell {
                    label: LabelRun::new(column.header.clone()),
                    turn: Lane::at_rest(TABLE_SORT_TIMING, turn_of(self.sort, index)),
                    active: Lane::at_rest(TABLE_SORT_TIMING, active_of(self.sort, index)),
                })
                .collect();
            element.columns = Rc::clone(&self.columns);
            element.hovered_column = None;
            element.armed_column = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.sort != self.sort {
            element.sort = self.sort;
            for (index, cell) in element.head.iter_mut().enumerate() {
                cell.turn.retarget(turn_of(self.sort, index));
                cell.active.retarget(active_of(self.sort, index));
            }
            flags |= ChangeFlags::PAINT;
        }

        if element.empty.set_content(self.empty_label.clone()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // A changed row count is the one signal that separates "this row was
        // inserted" from "this row scrolled into view" — see the module docs.
        let grew = prev.row_count != self.row_count;
        element.row_count = self.row_count;
        element.row_height = self.row_height;
        element.viewport = self.viewport;
        flags |= rebuild_child(&prev.body(false), &self.body(grew), &mut element.body, ctx);
        flags
    }

    fn teardown(&self, element: &mut TableWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.body(false), &mut element.body, ctx);
    }
}

/// The chevron turn column `index` rests at under `sort`: `1` while it sorts
/// descending, `0` otherwise (an unsorted column keeps the ascending pose,
/// exactly as upstream's `rotate: active && desc ? 180 : 0` does).
fn turn_of(sort: Option<TableSort>, index: usize) -> f64 {
    match sort {
        Some(sort) if sort.column == index => {
            f64::from(sort.direction == TableSortDirection::Descending)
        }
        _ => 0.0,
    }
}

/// How active column `index` is under `sort` — the chevron's opacity lane.
fn active_of(sort: Option<TableSort>, index: usize) -> f64 {
    f64::from(sort.is_some_and(|sort| sort.column == index))
}

/// The retained widget for a [`TableView`].
pub struct TableWidget {
    head: Vec<HeadCell>,
    columns: Rc<Vec<TableColumn>>,
    /// The empty-body placeholder's run.
    empty: LabelRun,
    row_count: usize,
    row_height: f64,
    /// The body height the view pinned, when it pinned one.
    viewport: Option<f64>,
    sort: Option<TableSort>,
    /// The virtualized body.
    body: ChildPod,
    /// The resolved column ladder, from the last layout.
    widths: Vec<f64>,
    /// The size layout resolved.
    size: Size,
    /// The head band's height.
    head_height: f64,
    /// The header cell under the pointer, self-corrected at paint.
    hovered_column: Option<usize>,
    /// The header cell a `Down` armed.
    armed_column: Option<usize>,
    on_sort: Option<ErasedArgCallback<Option<TableSort>>>,
}

impl TableWidget {
    /// The resolved column ladder.
    pub fn column_widths(&self) -> &[f64] {
        &self.widths
    }

    /// The head band's box, in widget-local coordinates.
    pub fn head_rect(&self) -> Rect {
        Rect::from_origin_size(Point::ORIGIN, Size::new(self.size.width, self.head_height))
    }

    /// Header cell `index`'s box, in widget-local coordinates.
    fn head_cell_rect(&self, index: usize) -> Option<Rect> {
        let width = *self.widths.get(index)?;
        let x = column_offsets(&self.widths)[index];
        Some(Rect::from_origin_size(
            Point::new(x, 0.0),
            Size::new(width, self.head_height),
        ))
    }

    /// The sortable header cell under a widget-local `pos`, if any.
    fn hit_head(&self, pos: Point) -> Option<usize> {
        if pos.y < 0.0 || pos.y >= self.head_height {
            return None;
        }
        (0..self.head.len()).find(|index| {
            self.columns[*index].sortable
                && self
                    .head_cell_rect(*index)
                    .is_some_and(|rect| rect.contains(pos))
        })
    }

    /// Advance the header's lanes, reporting whether any is still moving.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        let mut moving = false;
        for cell in &mut self.head {
            if reduce_motion {
                cell.turn.snap();
                cell.active.snap();
            }
            moving |= cell.turn.advance(now);
            moving |= cell.active.advance(now);
        }
        moving
    }
}

impl Widget for TableWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let head = head_style(theme);
        for cell in &mut self.head {
            cell.label.layout(ctx, &head);
        }
        self.empty.layout(ctx, &head);

        let width = crate::overlay::finite_or_zero(bc.max().width);
        self.widths = table_column_widths(&self.columns, width);
        self.head_height = self.row_height;

        // Virtualization needs a viewport; an unbounded context has none to
        // offer, so the body takes upstream's own default height there.
        let body_height = match self.viewport {
            Some(height) => height,
            None if bc.max().height.is_finite() => (bc.max().height - self.head_height).max(0.0),
            None => TABLE_VIEWPORT_HEIGHT,
        };
        self.body
            .layout_child(ctx, &BoxConstraints::tight(Size::new(width, body_height)));
        self.body.set_origin(Point::new(0.0, self.head_height));

        self.size = bc.constrain(Size::new(width, self.head_height + body_height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() && self.hovered_column.is_some() {
            self.hovered_column = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let moving = self.advance(ctx.frame_time(), reduce_motion);
        let origin = ctx.origin();

        // The body's own surface, so a partly-filled viewport is not the page
        // showing through (`bg-background` on the table root).
        scene.fill_rect(origin, self.size, colors.body);

        // The head band: `bg-muted` with a `border-b`, painted by this widget
        // rather than scrolled as row 0 — which is what makes it sticky.
        let head = self.head_rect();
        scene.fill_rect(origin, Size::new(head.width(), head.height()), colors.head);
        scene.fill_rect(
            Point::new(origin.x, origin.y + head.height() - style::BORDER_WIDTH),
            Size::new(head.width(), style::BORDER_WIDTH),
            colors.border,
        );

        for index in 0..self.head.len() {
            let Some(rect) = self.head_cell_rect(index) else {
                continue;
            };
            let column = &self.columns[index];
            let cell = &self.head[index];
            let sorted = cell.active.value().clamp(0.0, 1.0);
            let hovered = self.hovered_column == Some(index);
            // `text-muted-foreground`, brightening to `text-foreground` on the
            // sorted column and under the pointer.
            let ink = crate::press::lerp_color(
                colors.muted,
                colors.ink,
                if hovered { 1.0 } else { sorted },
            );

            let label = cell.label.size();
            let chevron_span = if column.sortable {
                TABLE_HEAD_GAP + TABLE_CHEVRON_SIZE
            } else {
                0.0
            };
            let inner = (rect.width() - TABLE_CELL_PADDING * 2.0).max(0.0);
            let content = (label.width + chevron_span).min(inner);
            let start = rect.x0 + TABLE_CELL_PADDING + column.align.offset(inner, content);

            scene.push_clip(
                Point::new(origin.x + rect.x0 + TABLE_CELL_PADDING, origin.y + rect.y0),
                Size::new(inner, rect.height()),
            );
            cell.label.paint(
                Point::new(
                    origin.x + start,
                    origin.y + (self.head_height - label.height) / 2.0,
                ),
                ink,
                scene,
            );
            if column.sortable {
                let chevron_ink = with_alpha(
                    ink,
                    TABLE_SORT_IDLE_OPACITY + (1.0 - TABLE_SORT_IDLE_OPACITY) * sorted as f32,
                );
                draw_chevron(
                    scene,
                    Point::new(
                        origin.x + start + label.width + TABLE_HEAD_GAP + TABLE_CHEVRON_SIZE / 2.0,
                        origin.y + self.head_height / 2.0,
                    ),
                    std::f64::consts::PI * cell.turn.value().clamp(0.0, 1.0),
                    chevron_ink,
                );
            }
            scene.pop_clip();
        }

        if self.row_count == 0 {
            let size = self.empty.size();
            self.empty.paint(
                Point::new(
                    origin.x + (self.size.width - size.width) / 2.0,
                    origin.y + self.head_height + (self.size.height - self.head_height) / 2.0
                        - size.height / 2.0,
                ),
                colors.muted,
                scene,
            );
        } else {
            self.body.paint_child(ctx, scene);
        }

        if moving {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.body.event_child(ctx, event);
            return EventResult::Ignored;
        }
        // The rows own scrolling and their own presses; the header claims only
        // what the body declined (the container-claims-after-routing rule).
        if self.armed_column.is_none()
            && route_event_single(&mut self.body, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.hit_head(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed_column = Some(index);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.armed_column.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let over = self.hit_head(p.position);
                if over.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered_column != over {
                    self.hovered_column = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed_column.take() else {
                    return EventResult::Ignored;
                };
                if self.hit_head(p.position) == Some(armed)
                    && let Some(on_sort) = self.on_sort.as_mut()
                {
                    on_sort(ctx, table_next_sort(self.sort, armed));
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed_column.take().is_none() {
                    return EventResult::Ignored;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let sort = self.sort;
        ctx.push_container(
            Role::Table,
            |_| {},
            |ctx| {
                for (index, cell) in self.head.iter().enumerate() {
                    let sorted = sort.is_some_and(|sort| sort.column == index);
                    ctx.push_node(Role::ColumnHeader, |node| {
                        node.set_label(cell.label.content());
                        node.set_selected(sorted);
                    });
                }
                self.body.semantics_child(ctx);
            },
        );
    }

    visit_children!(body);
}

// ---- The virtualized row ---------------------------------------------------

/// One materialized body row. Private: a row is only ever produced by
/// [`TableView`]'s own builder, and its shape (the resolved-per-row width
/// ladder) is meaningless outside the table that declared the columns.
struct TableRowView<State: 'static> {
    columns: Rc<Vec<TableColumn>>,
    values: Vec<String>,
    row: usize,
    selected: bool,
    height: f64,
    last: bool,
    enter: bool,
    on_press: Option<OnRow<State>>,
}

impl<State: 'static> View<State> for TableRowView<State> {
    type Element = TableRowWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TableRowWidget {
        // A row that materialized on an ordinary scroll frame is already
        // present; only one built on a frame the row count changed enters.
        let mut presence = if self.enter {
            Presence::new(TABLE_ROW_ENTER, TABLE_ROW_ENTER)
        } else {
            Presence::new(TABLE_ROW_ENTER, TABLE_ROW_ENTER).collapsed()
        };
        presence.set_open(true);
        TableRowWidget {
            columns: Rc::clone(&self.columns),
            cells: self.values.iter().map(LabelRun::new).collect(),
            row: self.row,
            selected: self.selected,
            height: self.height,
            last: self.last,
            presence,
            widths: Vec::new(),
            size: Size::ZERO,
            hovered: false,
            armed: false,
            on_press: self.on_press.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TableRowWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = self.on_press.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;
        if prev.columns != self.columns || prev.values.len() != self.values.len() {
            element.columns = Rc::clone(&self.columns);
            element.cells = self.values.iter().map(LabelRun::new).collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (cell, value) in element.cells.iter_mut().zip(self.values.iter()) {
                if cell.set_content(value.clone()) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        if prev.selected != self.selected || prev.last != self.last || prev.row != self.row {
            element.selected = self.selected;
            element.last = self.last;
            element.row = self.row;
            flags |= ChangeFlags::PAINT;
        }
        if prev.height != self.height {
            element.height = self.height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for one table row.
struct TableRowWidget {
    columns: Rc<Vec<TableColumn>>,
    cells: Vec<LabelRun>,
    row: usize,
    selected: bool,
    height: f64,
    last: bool,
    presence: Presence,
    widths: Vec<f64>,
    size: Size,
    hovered: bool,
    armed: bool,
    on_press: Option<ErasedArgCallback<usize>>,
}

impl Widget for TableRowWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = cell_style(Theme::from_layout_ctx(ctx));
        for cell in &mut self.cells {
            cell.layout(ctx, &style);
        }
        // The list hands a row a tight width, so this resolves the same ladder
        // the header did without either seeing the other.
        let width = crate::overlay::finite_or_zero(bc.max().width);
        self.widths = table_column_widths(&self.columns, width);
        self.size = bc.constrain(Size::new(width, self.height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() && self.hovered {
            self.hovered = false;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        if reduce_motion {
            self.presence = self.presence.collapsed();
        }
        let presence = self.presence.advance(ctx.frame_time()).clamp(0.0, 1.0);
        let origin = ctx.origin();
        let rise = (1.0 - presence) * TABLE_ROW_ENTER_RISE;

        let entering = presence < 1.0;
        if entering {
            scene.push_layer(origin, self.size, presence as f32);
            scene.push_transform(Affine::translate((0.0, rise)));
        }

        let accent = theme.map_or(FALLBACK_PRIMARY, |t| t.scheme().primary);
        if self.selected {
            scene.fill_rect(origin, self.size, with_alpha(accent, TABLE_SELECTED_WASH));
        }
        if self.hovered {
            scene.fill_rect(origin, self.size, with_alpha(colors.head, TABLE_HOVER_WASH));
        }

        let offsets = column_offsets(&self.widths);
        for (index, cell) in self.cells.iter().enumerate() {
            let (Some(width), Some(x)) = (self.widths.get(index), offsets.get(index)) else {
                continue;
            };
            let column = &self.columns[index];
            let inner = (width - TABLE_CELL_PADDING * 2.0).max(0.0);
            let label = cell.size();
            let start = origin.x + x + TABLE_CELL_PADDING + column.align.offset(inner, label.width);
            scene.push_clip(
                Point::new(origin.x + x + TABLE_CELL_PADDING, origin.y),
                Size::new(inner, self.size.height),
            );
            cell.paint(
                Point::new(start, origin.y + (self.size.height - label.height) / 2.0),
                colors.ink,
                scene,
            );
            scene.pop_clip();
        }

        // `[&_tr:last-child]:border-0` — the body's own last row carries none.
        if !self.last {
            scene.fill_rect(
                Point::new(origin.x, origin.y + self.size.height - style::BORDER_WIDTH),
                Size::new(self.size.width, style::BORDER_WIDTH),
                with_alpha(colors.border, TABLE_BORDER_ALPHA),
            );
        }

        if entering {
            scene.pop_transform();
            scene.pop_layer();
        }
        if self.presence.is_animating() {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || self.on_press.is_none() {
                    return EventResult::Ignored;
                }
                self.armed = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.armed {
                    return EventResult::Handled;
                }
                let inside = crate::press::inside(p.position, self.size);
                if inside {
                    ctx.claim_hover();
                }
                if self.hovered != inside {
                    self.hovered = inside;
                    ctx.request_redraw();
                }
                // A row's hover wash must not stop the list from reading the
                // move as the start of a scroll drag.
                EventResult::Ignored
            }
            PointerPhase::Up => {
                if !std::mem::take(&mut self.armed) {
                    return EventResult::Ignored;
                }
                if crate::press::inside(p.position, self.size)
                    && let Some(on_press) = self.on_press.as_mut()
                {
                    on_press(ctx, self.row);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !std::mem::take(&mut self.armed) {
                    return EventResult::Ignored;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self
            .cells
            .iter()
            .map(LabelRun::content)
            .collect::<Vec<_>>()
            .join(", ");
        let selected = self.selected;
        ctx.push_node(Role::Row, |node| {
            node.set_label(label.as_str());
            node.set_selected(selected);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{
        PointerButton, PointerEvent, SemanticsUpdate, scene::GlyphRun, text::TextContext,
    };
    use std::any::Any;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    const WINDOW: Size = Size::new(600.0, 480.0);

    /// Records the paint calls the table's chrome is asserted through.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        clips: usize,
        strokes: usize,
        transforms: usize,
        layers: Vec<f32>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_clip(&mut self, _o: Point, _s: Size) {
            self.clips += 1;
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _t: Affine) {
            self.transforms += 1;
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// What the callbacks report.
    #[derive(Default)]
    struct Log {
        sorts: Vec<Option<TableSort>>,
        pressed: Vec<usize>,
        end_reached: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn columns() -> Vec<TableColumn> {
        vec![
            table_column("Name").sortable(true),
            table_column("Status"),
            table_column("Total").align(TableAlign::End).sortable(true),
        ]
    }

    fn view(rows: usize, sort: Option<TableSort>) -> TableView<Log> {
        table::<Log, _>(columns(), rows, |row, column| format!("r{row}c{column}"))
            .sort(sort)
            .on_sort(|state: &mut Log, next| state.sorts.push(next))
            .on_row_press(|state: &mut Log, row| state.pressed.push(row))
            .on_end_reached(|state: &mut Log| state.end_reached += 1)
    }

    fn build(view: &TableView<Log>) -> TableWidget {
        let mut counter = 0u64;
        View::<Log>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(widget: &mut TableWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::new(Size::ZERO, WINDOW))
    }

    fn laid_out(rows: usize, sort: Option<TableSort>) -> TableWidget {
        let mut widget = build(&view(rows, sort));
        layout(&mut widget);
        widget
    }

    fn paint_at(widget: &mut TableWidget, ms: f64, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, WINDOW, ft_ms(ms));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        widget.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(widget: &mut TableWidget, event: &InputEvent, state: &mut Log) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, WINDOW);
        widget.event(&mut ctx, event);
    }

    /// Undeclared columns share the remainder equally; a declared one keeps its
    /// own width and is not part of the share.
    #[test]
    fn the_ladder_shares_the_remainder_and_honours_declared_widths() {
        let even = table_column_widths(&columns(), 600.0);
        assert_eq!(even, vec![200.0, 200.0, 200.0]);

        let mixed = vec![
            table_column("Pinned").width(300.0),
            table_column("A"),
            table_column("B"),
        ];
        assert_eq!(
            table_column_widths(&mixed, 600.0),
            vec![300.0, 150.0, 150.0]
        );
        assert_eq!(
            table_column_widths(&mixed, 600.0).iter().sum::<f64>(),
            600.0,
            "the ladder fills the box exactly"
        );
        assert!(table_column_widths(&[], 600.0).is_empty());
    }

    /// Too little room floors every flexible column at the minimum and then
    /// scales the whole ladder down — there is no horizontal scroll to pan
    /// with, so nothing is ever left outside the box.
    #[test]
    fn an_overwide_ladder_shrinks_proportionally() {
        let tight = table_column_widths(&columns(), 100.0);
        assert!(
            (tight.iter().sum::<f64>() - 100.0).abs() < 1e-9,
            "scaled to fit, not overflowing: {tight:?}"
        );
        for width in &tight {
            assert!((width - 100.0 / 3.0).abs() < 1e-9, "shrunk in proportion");
        }

        let declared = vec![
            table_column("A").width(500.0),
            table_column("B").width(400.0),
        ];
        let scaled = table_column_widths(&declared, 600.0);
        assert!((scaled.iter().sum::<f64>() - 600.0).abs() < 1e-9);
        assert!(
            (scaled[0] / scaled[1] - 500.0 / 400.0).abs() < 1e-9,
            "the declared ratio survives the shrink"
        );
        // Degenerate boxes are defined rather than producing negatives.
        for width in table_column_widths(&columns(), 0.0) {
            assert_eq!(width, 0.0);
        }
    }

    /// The load-bearing property behind hosting the header outside the list:
    /// the head band and a virtualized row never see each other, and still line
    /// up, because both resolve the same pure ladder against the same tight
    /// width the list hands them.
    #[test]
    fn the_header_and_a_row_resolve_the_same_ladder_independently() {
        let mut table = laid_out(20, None);
        let mut tcx = TextContext::new();

        let row = TableRowView::<Log> {
            columns: Rc::new(columns()),
            values: vec!["a".into(), "b".into(), "c".into()],
            row: 3,
            selected: false,
            height: TABLE_ROW_HEIGHT,
            last: false,
            enter: false,
            on_press: None,
        };
        let mut counter = 0u64;
        let mut widget = View::<Log>::build(&row, &mut BuildCtx::new(&mut counter));
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        // Exactly what a keyed, variable-extent list hands a row: a tight width
        // and a free height.
        widget.layout(
            &mut ctx,
            &BoxConstraints::new(
                Size::new(WINDOW.width, 0.0),
                Size::new(WINDOW.width, f64::INFINITY),
            ),
        );
        assert_eq!(widget.widths, table.column_widths());
        assert_eq!(
            widget.size.height, TABLE_ROW_HEIGHT,
            "the row sizes itself under a free height"
        );
        let _ = layout(&mut table);
    }

    /// The head band is the table's own paint, above the list's box — which is
    /// what makes it sticky, `ListView` having no sticky-row concept.
    #[test]
    fn the_head_band_sits_above_the_scrolling_body() {
        let widget = laid_out(500, None);
        assert_eq!(widget.head_rect().height(), TABLE_ROW_HEIGHT);
        assert_eq!(widget.body.origin(), Point::new(0.0, TABLE_ROW_HEIGHT));
        assert_eq!(
            widget.body.size(),
            Size::new(WINDOW.width, WINDOW.height - TABLE_ROW_HEIGHT),
            "the body takes the rest of the bounded box"
        );
    }

    /// With no bounded height there is no viewport to window against, so the
    /// body falls back to upstream's own default rather than growing to the
    /// content.
    #[test]
    fn an_unbounded_context_falls_back_to_the_default_viewport() {
        let mut widget = build(&view(10_000, None));
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, f64::INFINITY)),
        );
        assert_eq!(size.height, TABLE_ROW_HEIGHT + TABLE_VIEWPORT_HEIGHT);

        // A pinned viewport wins over both.
        let mut pinned = build(&view(10_000, None).viewport_height(200.0));
        let size = layout(&mut pinned);
        assert_eq!(size.height, TABLE_ROW_HEIGHT + 200.0);
    }

    /// Upstream's `toggleSort` cycle, verbatim: a new column starts ascending,
    /// ascending flips to descending, descending clears.
    #[test]
    fn the_sort_cycle_is_ascending_then_descending_then_none() {
        assert_eq!(table_next_sort(None, 1), Some(TableSort::ascending(1)));
        assert_eq!(
            table_next_sort(Some(TableSort::ascending(1)), 1),
            Some(TableSort::descending(1))
        );
        assert_eq!(table_next_sort(Some(TableSort::descending(1)), 1), None);
        assert_eq!(
            table_next_sort(Some(TableSort::descending(1)), 2),
            Some(TableSort::ascending(2)),
            "a different column restarts the cycle"
        );
    }

    /// Pressing a sortable header reports the next sort and never mutates the
    /// view's own — the controlled-component rule.
    #[test]
    fn pressing_a_sortable_header_reports_the_next_sort() {
        let mut widget = laid_out(20, None);
        let head = widget.head_cell_rect(0).expect("first column").center();
        let mut log = Log::default();
        dispatch(&mut widget, &pointer(PointerPhase::Down, head), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, head), &mut log);
        assert_eq!(log.sorts, vec![Some(TableSort::ascending(0))]);
        assert_eq!(widget.sort, None, "the table did not sort itself");

        // With the ascending sort fed back down, the same press asks for
        // descending.
        let before = view(20, None);
        let after = view(20, Some(TableSort::ascending(0)));
        let mut counter = 0u64;
        View::<Log>::rebuild(
            &after,
            &before,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        layout(&mut widget);
        dispatch(&mut widget, &pointer(PointerPhase::Down, head), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, head), &mut log);
        assert_eq!(log.sorts[1], Some(TableSort::descending(0)));
    }

    /// A column that did not opt in is inert: no hit, no report.
    #[test]
    fn a_non_sortable_header_reports_nothing() {
        let mut widget = laid_out(20, None);
        let head = widget.head_cell_rect(1).expect("second column").center();
        assert_eq!(widget.hit_head(head), None);
        let mut log = Log::default();
        dispatch(&mut widget, &pointer(PointerPhase::Down, head), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, head), &mut log);
        assert!(log.sorts.is_empty());
    }

    /// The sorted column's chevron turns a half-circle and fades up from
    /// `opacity: 0.35`; the turn is animated, not instant.
    #[test]
    fn the_sort_chevron_turns_and_fades_on_the_sorted_column() {
        let mut widget = laid_out(20, None);
        assert_eq!(widget.head[0].turn.value(), 0.0);
        assert_eq!(widget.head[0].active.value(), 0.0);

        let before = view(20, None);
        let after = view(20, Some(TableSort::descending(0)));
        let mut counter = 0u64;
        View::<Log>::rebuild(
            &after,
            &before,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        layout(&mut widget);

        let (_, needs_frame) = paint_at(&mut widget, 0.0, None);
        assert!(needs_frame, "the chevron owes a frame mid-turn");
        assert!(widget.head[0].turn.value() < 1.0);
        paint_at(&mut widget, 400.0, None);
        assert_eq!(widget.head[0].turn.value(), 1.0, "a half turn, settled");
        assert_eq!(widget.head[0].active.value(), 1.0);
        assert_eq!(widget.head[1].active.value(), 0.0, "only the sorted column");

        // `reduce_motion` lands the same pose on the first frame.
        let mut widget = laid_out(20, None);
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        View::<Log>::rebuild(
            &after,
            &before,
            &mut widget,
            &mut BuildCtx::new(&mut counter),
        );
        layout(&mut widget);
        let (_, needs_frame) = paint_at(&mut widget, 0.0, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(widget.head[0].turn.value(), 1.0);
    }

    /// The acceptance criterion: a 10k-row table asks its cell provider for a
    /// screenful of rows and never for the rest — `ListView`'s windowing
    /// contract, observed through the provider it calls.
    #[test]
    fn ten_thousand_rows_materialize_only_a_window() {
        let seen: Rc<RefCell<BTreeSet<usize>>> = Rc::new(RefCell::new(BTreeSet::new()));
        let probe = Rc::clone(&seen);
        let mut root = frust_core::RenderRoot::new();
        let mut state = Log::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut Log| {
            let probe = Rc::clone(&probe);
            table::<Log, _>(columns(), 10_000, move |row, column| {
                probe.borrow_mut().insert(row);
                format!("r{row}c{column}")
            })
        };
        // Two passes: the first build windows against a zero viewport, the
        // second against the one layout has now measured (`ListView`'s
        // documented one-frame convergence).
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        let seen = seen.borrow();
        let viewport_rows = ((WINDOW.height - TABLE_ROW_HEIGHT) / TABLE_ROW_HEIGHT).ceil() as usize;
        assert!(!seen.is_empty(), "the first screenful is materialized");
        assert!(seen.contains(&0), "the window starts at the top");
        assert!(
            seen.len() <= viewport_rows + 8,
            "a screenful plus the list's buffer, not 10k: {} rows",
            seen.len()
        );
        assert!(
            !seen.contains(&9_999) && !seen.contains(&5_000),
            "nothing off-screen was ever built"
        );
    }

    /// An empty table paints its placeholder instead of an empty body.
    #[test]
    fn an_empty_table_paints_its_placeholder() {
        let mut widget = build(&view(0, None).empty_label("Nothing here"));
        layout(&mut widget);
        let (rec, _) = paint_at(&mut widget, 0.0, None);
        assert_eq!(widget.empty.content(), "Nothing here");
        assert!(
            rec.inks.contains(&FALLBACK_MUTED_INK),
            "the placeholder is drawn in the muted ink"
        );
    }

    /// A row washes for selection and for hover, keeps a hairline unless it is
    /// the last one, and reports a press by index.
    #[test]
    fn a_row_washes_and_reports_its_press() {
        let mut counter = 0u64;
        let mut tcx = TextContext::new();
        let row = TableRowView::<Log> {
            columns: Rc::new(columns()),
            values: vec!["a".into(), "b".into(), "c".into()],
            row: 7,
            selected: true,
            height: TABLE_ROW_HEIGHT,
            last: false,
            enter: false,
            on_press: Some(Rc::new(|state: &mut Log, row| state.pressed.push(row))),
        };
        let mut widget = View::<Log>::build(&row, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(
            &mut lctx,
            &BoxConstraints::tight(Size::new(WINDOW.width, TABLE_ROW_HEIGHT)),
        );

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0));
        widget.paint(&mut pctx, &mut rec);
        assert!(
            rec.rects
                .iter()
                .any(|(_, _, color)| color.components[3] > 0.0 && color.components[3] < 0.2),
            "the selection wash is painted"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(_, size, _)| size.height == style::BORDER_WIDTH),
            "the row hairline is painted"
        );
        assert!(rec.layers.is_empty(), "a scrolled-in row does not animate");

        let mut log = Log::default();
        let centre = Point::new(size.width / 2.0, size.height / 2.0);
        let mut ectx = EventCtx::new(&mut log as &mut dyn Any, Point::ZERO, size);
        widget.event(&mut ectx, &pointer(PointerPhase::Down, centre));
        let mut ectx = EventCtx::new(&mut log as &mut dyn Any, Point::ZERO, size);
        widget.event(&mut ectx, &pointer(PointerPhase::Up, centre));
        assert_eq!(log.pressed, vec![7]);
    }

    /// A row built on a frame the row count changed plays its entrance; one
    /// built on an ordinary scroll frame does not. That gate is the whole of
    /// the add/remove animation a virtualized list can honestly support.
    #[test]
    fn only_a_row_built_on_a_count_change_enters() {
        let mut counter = 0u64;
        let mut tcx = TextContext::new();
        let make = |enter: bool| TableRowView::<Log> {
            columns: Rc::new(columns()),
            values: vec!["a".into(), "b".into(), "c".into()],
            row: 0,
            selected: false,
            height: TABLE_ROW_HEIGHT,
            last: true,
            enter,
            on_press: None,
        };
        for (enter, animates) in [(true, true), (false, false)] {
            let view = make(enter);
            let mut widget = View::<Log>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size = widget.layout(
                &mut lctx,
                &BoxConstraints::tight(Size::new(WINDOW.width, TABLE_ROW_HEIGHT)),
            );
            let mut rec = Recorder::default();
            let mut pctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0));
            widget.paint(&mut pctx, &mut rec);
            assert_eq!(
                !rec.layers.is_empty(),
                animates,
                "enter = {enter} should animate = {animates}"
            );
            assert_eq!(pctx.needs_frame(), animates);
        }
    }

    /// The head band's own chrome: a `bg-muted` band with a hairline under it,
    /// and every column's label clipped to its cell.
    #[test]
    fn the_head_band_paints_its_fill_and_hairline() {
        // An empty body so the only clips and strokes recorded are the head's.
        let mut widget = laid_out(0, None);
        let (rec, _) = paint_at(&mut widget, 0.0, None);
        assert!(
            rec.rects.iter().any(|(o, s, c)| *o == Point::ZERO
                && s.height == TABLE_ROW_HEIGHT
                && *c == FALLBACK_MUTED),
            "the head band is filled"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(o, s, c)| o.y == TABLE_ROW_HEIGHT - style::BORDER_WIDTH
                    && s.height == style::BORDER_WIDTH
                    && *c == FALLBACK_BORDER),
            "the head band carries a border-b"
        );
        assert_eq!(rec.clips, 3, "one clip per head cell");
        assert_eq!(rec.strokes, 2, "one chevron per sortable column");
    }

    /// The themed palette reads the roles beUI folds `muted`/`border`/
    /// `foreground` onto, and the unthemed one the vendored light table.
    #[test]
    fn the_palette_reads_the_themed_roles() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let themed = resolve_colors(Some(&theme));
        assert_eq!(themed.head, scheme.surface_container_highest);
        assert_eq!(themed.border, scheme.outline_variant);
        assert_eq!(themed.ink, scheme.on_surface);
        assert_eq!(themed.muted, scheme.on_surface_variant);

        let unthemed = resolve_colors(None);
        assert_eq!(unthemed.head, crate::BEUI_LIGHT.muted);
        assert_eq!(unthemed.ink, crate::BEUI_LIGHT.foreground);
    }

    /// One column-header node per column, with the sorted one marked, plus the
    /// body's own subtree forwarded through the pod.
    #[test]
    fn semantics_publish_a_header_per_column_and_the_body() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = Log::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut Log| view(40, Some(TableSort::ascending(2)));
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let headers: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::ColumnHeader)
            .collect();
        assert_eq!(headers.len(), 3, "one node per column");
        assert_eq!(
            headers
                .iter()
                .filter(|(_, node)| node.is_selected() == Some(true))
                .count(),
            1,
            "exactly the sorted column is marked"
        );
        let rows = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::Row)
            .count();
        assert!(rows > 0 && rows < 40, "only the materialized rows: {rows}");
    }

    // ---- Typeface: headers, cells and the placeholder follow the theme -----

    use crate::text::typeface_probe::{
        Face, FaceRecorder, assert_all, assert_control, mono_role_theme, text_context,
    };

    /// A `rows`-row table under a real `RenderRoot`, painted through the
    /// typeface probe's face recorder. The shared probe builds once; the body
    /// needs a second build to window its rows against a measured viewport.
    struct FaceHarness {
        root: frust_core::RenderRoot<(), TableView<()>>,
        tcx: TextContext,
        clock_ms: f64,
    }

    impl FaceHarness {
        fn new(rows: usize) -> Self {
            let mut h = FaceHarness {
                root: frust_core::RenderRoot::new(),
                tcx: text_context(),
                clock_ms: 0.0,
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_: &mut ()| {
                table::<(), _>(columns(), rows, |row, column| format!("r{row}c{column}"))
            };
            h.root.rebuild(&mut logic, &mut ());
            h.root.layout_with_text(WINDOW, &mut h.tcx as &mut dyn Any);
            h.root.rebuild(&mut logic, &mut ());
            h
        }

        /// Lay out and paint twice, the second five seconds on once every
        /// entrance has settled, and return the settled paint's faces.
        fn frame(&mut self) -> Vec<Face> {
            let mut faces = Vec::new();
            for _ in 0..2 {
                self.root
                    .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
                let mut rec = FaceRecorder::default();
                self.root.paint(&mut rec, ft_ms(self.clock_ms));
                self.clock_ms += 5_000.0;
                faces = rec.faces;
            }
            faces
        }
    }

    /// An empty table paints its headers and the placeholder; a populated one
    /// its headers and one run per cell.
    const PROBE_CASES: [(usize, usize); 2] = [(0, 4), (3, 3 + 3 * 3)];

    #[test]
    fn table_text_paints_in_geist_under_the_beui_theme() {
        assert_control("the table's text", WINDOW);
        for (rows, runs) in PROBE_CASES {
            let what = format!("the {rows}-row table's text");
            let faces = FaceHarness::new(rows).frame();
            assert_eq!(faces.len(), runs, "{what}: {faces:?}");
            assert_all(&what, "under the beUI theme", &faces, Face::Geist);
        }
    }

    #[test]
    fn table_text_follows_a_live_theme_family_swap() {
        for (rows, _) in PROBE_CASES {
            let what = format!("the {rows}-row table's text");
            let mut h = FaceHarness::new(rows);
            assert_all(&what, "under the beUI theme", &h.frame(), Face::Geist);
            h.root.set_theme(Box::new(mono_role_theme()));
            assert_all(
                &what,
                "after a swap to Geist Mono roles",
                &h.frame(),
                Face::GeistMono,
            );
            h.root.set_theme(Box::new(crate::theme()));
            assert_all(&what, "after swapping back", &h.frame(), Face::Geist);
        }
    }
}
