//! `table`: the bordered data table — header, body, footer, caption.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/table.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17): a
//! `w-full caption-bottom text-sm` table, `[&_tr]:border-b` header rows,
//! `[&_tr:last-child]:border-0` body, `border-t bg-muted/50 font-medium` footer,
//! `hover:bg-muted/50 data-[state=selected]:bg-muted` rows, `h-10 px-2
//! font-medium text-foreground` head cells, `p-2` cells, and a
//! `mt-4 text-sm text-muted-foreground` caption.
//!
//! # One widget, not eight
//!
//! Upstream's eight components (`Table`/`Header`/`Body`/`Footer`/`Row`/`Head`/
//! `Cell`/`Caption`) exist because HTML tables are eight elements; the *column
//! sizing* that makes them line up is the browser's table layout, not theirs. This
//! port keeps the composition in the API — [`table`] takes [`TableRow`]s, with
//! [`header`](TableView::header)/[`footer`](TableView::footer)/
//! [`caption`](TableView::caption) slots — and owns every cell in one widget,
//! because column widths have to be resolved across rows and no per-row widget
//! can see its siblings.
//!
//! Head and footer cells are text labels; body cells are arbitrary views
//! ([`table_cell`] is the text one). That is the one shape simplification: a head
//! cell upstream can hold a sort button, and here it cannot.
//!
//! # Row hover claims *after* routing
//!
//! A row's `hover:bg-muted/50` is container chrome over interactive content
//! (`docs/CODE_STANDARDS.md`'s hover-ordering rule): the `Move` is routed to the
//! cells first and the row claims only afterwards, so a cell-level control under
//! the pointer wins the claim while the row still reads hovered through the path.
//! The row also latches its own hovered index (the only frame source for hover
//! *gain*) and re-syncs it from [`PaintCtx::is_hovered`] every paint.
//!
//! # Horizontal overflow
//!
//! Upstream wraps the table in `overflow-x-auto`. The framework's `ScrollView` is
//! **vertical only**, so there is no horizontal-scroll analog to wrap it in:
//! instead, when the columns' natural widths exceed the available width this
//! widget scales them down proportionally (`w-full` in both directions), which
//! keeps every column visible and clipped by nothing. A table that needs true
//! horizontal panning is a gap, recorded rather than faked.
//!
//! [`PaintCtx::is_hovered`]: frust::authoring::PaintCtx::is_hovered

use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, ErasedCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, ThemeTextColor, View, Widget, any, build_child, erase_callback,
    rebuild_child, route_event, teardown_child, text::FontWeight, visit_children,
};
use frust::{TextView, Theme, text};

use crate::components::input::FALLBACK;
use crate::hit::inside;
use crate::style;

/// Cell padding: `p-2` on a body cell, `px-2` on a head cell.
const CELL_PAD: f64 = 8.0;
/// Head-row height: `h-10`.
const HEAD_HEIGHT: f64 = 40.0;
/// Gap between the table and its caption: `mt-4`.
const CAPTION_GAP: f64 = 16.0;
/// Alpha of the `muted` washes: `bg-muted/50` (row hover, footer).
const MUTED_WASH_ALPHA: f32 = 0.5;

/// The `border` token: themed `outline`, else the fallback table.
fn border_color(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.border, |t| t.scheme().outline)
}

/// The `muted` token: themed `surface_container_highest`, else the fallback table.
fn muted(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted, |t| t.scheme().surface_container_highest)
}

/// A `text-sm` table cell label in `foreground` — the text form of upstream's
/// `TableCell`. A body cell takes any view; this is the common one.
pub fn table_cell(label: impl Into<String>) -> TextView {
    text(label.into())
        .size(style::TEXT_SM as f32)
        .family(crate::tokens::sans_family())
        .themed_role(ThemeTextColor::OnSurface)
}

/// A `text-sm font-medium` head/footer label in `foreground`.
fn strong_cell(label: &str) -> TextView {
    table_cell(label.to_string()).weight(FontWeight::MEDIUM)
}

/// A view-held, typed row-activation callback (erased on build).
type OnRowClick<State> = Rc<dyn Fn(&mut State)>;

/// A declarative table row — upstream's `TableRow`. See [`table_row`].
pub struct TableRow<State: 'static> {
    cells: Vec<AnyView<State>>,
    selected: bool,
    on_click: Option<OnRowClick<State>>,
}

/// Build a body row from its cells.
pub fn table_row<State: 'static, I>(cells: I) -> TableRow<State>
where
    I: IntoIterator,
    I::Item: View<State>,
{
    TableRow {
        cells: cells.into_iter().map(any).collect(),
        selected: false,
        on_click: None,
    }
}

impl<State: 'static> TableRow<State> {
    /// Mark the row selected (`data-[state=selected]:bg-muted`).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Make the whole row activatable, firing on release inside the row (a cell
    /// that handles the press itself wins — see the [module docs](self)).
    pub fn on_click<F: Fn(&mut State) + 'static>(mut self, on_click: F) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }
}

/// Which band a rendered row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    /// A `thead` row: `h-10`, `font-medium`, `border-b`.
    Head,
    /// A `tbody` row: hoverable, selectable, `border-b` except the last.
    Body,
    /// The `tfoot` row: `border-t bg-muted/50 font-medium`.
    Foot,
}

/// One rendered row: its cell window in `cells`, its band, and its state.
struct RowGeom {
    kind: RowKind,
    /// Index of this row's first cell in [`TableWidget::cells`].
    start: usize,
    /// How many cells the row holds (may be shorter than the column count).
    len: usize,
    selected: bool,
    on_click: Option<ErasedCallback>,
    /// Resolved row height (layout output).
    height: f64,
    /// Resolved row top, relative to the widget's origin (layout output).
    top: f64,
}

/// A declarative table. See the [module docs](self).
pub struct TableView<State: 'static> {
    header: Vec<String>,
    rows: Vec<TableRow<State>>,
    footer: Vec<String>,
    caption: Option<String>,
}

/// Build a table over `rows`; add the header/footer/caption slots with the
/// builders on [`TableView`].
pub fn table<State: 'static, I: IntoIterator<Item = TableRow<State>>>(rows: I) -> TableView<State> {
    TableView {
        header: Vec::new(),
        rows: rows.into_iter().collect(),
        footer: Vec::new(),
        caption: None,
    }
}

impl<State: 'static> TableView<State> {
    /// Set the header row's labels (upstream's `TableHeader`/`TableHead`).
    pub fn header<I: IntoIterator<Item = S>, S: Into<String>>(mut self, header: I) -> Self {
        self.header = header.into_iter().map(Into::into).collect();
        self
    }

    /// Set the footer row's labels (upstream's `TableFooter`).
    pub fn footer<I: IntoIterator<Item = S>, S: Into<String>>(mut self, footer: I) -> Self {
        self.footer = footer.into_iter().map(Into::into).collect();
        self
    }

    /// Set the caption, rendered below the table (`caption-bottom`).
    pub fn caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    /// The rendered row shape — cell counts per band — which is what a rebuild
    /// has to compare to decide between reconciling in place and rebuilding.
    fn shape(&self) -> (usize, Vec<usize>, usize, bool) {
        (
            self.header.len(),
            self.rows.iter().map(|r| r.cells.len()).collect(),
            self.footer.len(),
            self.caption.is_some(),
        )
    }

    /// The caption's text view (`text-sm text-muted-foreground`).
    fn caption_view(&self) -> Option<AnyView<State>> {
        self.caption.as_ref().map(|caption| {
            any(table_cell(caption.clone()).themed_role(ThemeTextColor::OnSurfaceVariant))
        })
    }

    /// The head/footer label views this table generates, owned: head labels
    /// first, then footer labels.
    ///
    /// Split from [`bands`](Self::bands) because an `AnyView` is not `Clone` and
    /// the child helpers take it by reference: the generated views have to *live*
    /// somewhere for the duration of a pass, and that somewhere is the caller's
    /// stack, not a self-referential struct.
    fn generated(&self) -> Vec<AnyView<State>> {
        self.header
            .iter()
            .chain(&self.footer)
            .map(|label| any(strong_cell(label)))
            .collect()
    }

    /// Every cell view in render order, banded by row — borrowing the body cells
    /// from `self` and the head/footer labels from `generated`
    /// ([`generated`](Self::generated)).
    fn bands<'a>(
        &'a self,
        generated: &'a [AnyView<State>],
    ) -> Vec<(RowKind, Vec<&'a AnyView<State>>)> {
        let mut bands = Vec::new();
        let head = self.header.len();
        if head > 0 {
            bands.push((RowKind::Head, generated[..head].iter().collect()));
        }
        for row in &self.rows {
            bands.push((RowKind::Body, row.cells.iter().collect()));
        }
        if !self.footer.is_empty() {
            bands.push((RowKind::Foot, generated[head..].iter().collect()));
        }
        bands
    }
}

/// The retained widget for a [`TableView`].
pub struct TableWidget {
    /// Every cell pod, row-major in render order (head, body rows, footer).
    cells: Vec<ChildPod>,
    /// One entry per rendered row, indexing into `cells`.
    rows: Vec<RowGeom>,
    caption: Option<ChildPod>,
    /// The column count: the widest row.
    columns: usize,
    /// Resolved column widths (layout output).
    col_widths: Vec<f64>,
    /// The latched hovered body-row index (self-corrected at paint).
    hovered: Option<usize>,
    /// The body row a press is armed on.
    pressed: Option<usize>,
    captured: bool,
}

impl<State: 'static> View<State> for TableView<State> {
    type Element = TableWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TableWidget {
        let generated = self.generated();
        let mut cells = Vec::new();
        let mut rows = Vec::new();
        let mut body = 0usize;
        for (kind, band) in self.bands(&generated) {
            let start = cells.len();
            for cell in &band {
                cells.push(build_child(cell, ctx));
            }
            let (selected, on_click) = match kind {
                RowKind::Body => {
                    let row = &self.rows[body];
                    body += 1;
                    (row.selected, row.on_click.as_ref().map(erase_callback))
                }
                _ => (false, None),
            };
            rows.push(RowGeom {
                kind,
                start,
                len: band.len(),
                selected,
                on_click,
                height: 0.0,
                top: 0.0,
            });
        }
        TableWidget {
            columns: rows.iter().map(|r| r.len).max().unwrap_or(0),
            cells,
            rows,
            caption: self.caption_view().as_ref().map(|v| build_child(v, ctx)),
            col_widths: Vec::new(),
            hovered: None,
            pressed: None,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TableWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let prev_generated = prev.generated();
        let prev_bands = prev.bands(&prev_generated);
        if self.shape() != prev.shape() {
            // The row/column shape changed: tear the whole cell set down against
            // the previous views and rebuild. Cell identity is positional here, so
            // a shape change is not reconcilable in place.
            for (pod, cell) in element
                .cells
                .iter_mut()
                .zip(prev_bands.iter().flat_map(|(_, b)| b))
            {
                teardown_child(cell, pod, ctx);
            }
            if let (Some(pod), Some(view)) = (element.caption.as_mut(), prev.caption_view()) {
                teardown_child(&view, pod, ctx);
            }
            *element = View::<State>::build(self, ctx);
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let generated = self.generated();
        let bands = self.bands(&generated);
        let prev_cells = prev_bands.iter().flat_map(|(_, b)| b);
        let next_cells = bands.iter().flat_map(|(_, b)| b);
        for ((pod, p), n) in element.cells.iter_mut().zip(prev_cells).zip(next_cells) {
            flags |= rebuild_child(p, n, pod, ctx);
        }
        if let (Some(pod), Some(p), Some(n)) = (
            element.caption.as_mut(),
            prev.caption_view(),
            self.caption_view(),
        ) {
            flags |= rebuild_child(&p, &n, pod, ctx);
        }
        // Row state and callbacks refresh unconditionally (closures are not
        // comparable, and a selection flip is a repaint).
        let mut body = 0usize;
        for row in &mut element.rows {
            if row.kind != RowKind::Body {
                continue;
            }
            let view = &self.rows[body];
            body += 1;
            if row.selected != view.selected {
                row.selected = view.selected;
                flags |= ChangeFlags::PAINT;
            }
            row.on_click = view.on_click.as_ref().map(erase_callback);
        }
        flags
    }

    fn teardown(&self, element: &mut TableWidget, ctx: &mut BuildCtx<'_>) {
        let generated = self.generated();
        let bands = self.bands(&generated);
        for (pod, cell) in element
            .cells
            .iter_mut()
            .zip(bands.iter().flat_map(|(_, b)| b))
        {
            teardown_child(cell, pod, ctx);
        }
        if let (Some(pod), Some(view)) = (element.caption.as_mut(), self.caption_view()) {
            teardown_child(&view, pod, ctx);
        }
    }
}

impl TableWidget {
    /// The body-row index at widget-local `pos`, if any.
    fn row_at(&self, pos: Point, size: Size) -> Option<usize> {
        if !inside(pos, size) {
            return None;
        }
        self.rows.iter().enumerate().find_map(|(index, row)| {
            (row.kind == RowKind::Body && pos.y >= row.top && pos.y < row.top + row.height)
                .then_some(index)
        })
    }

    /// Whether row `index` is activatable.
    fn is_activatable(&self, index: usize) -> bool {
        self.rows
            .get(index)
            .is_some_and(|row| row.on_click.is_some())
    }
}

impl Widget for TableWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        if self.columns == 0 {
            return bc.constrain(Size::new(width, 0.0));
        }

        // Pass 1: each cell's natural width, folded into a per-column maximum.
        let loose = BoxConstraints::new(Size::ZERO, Size::new(width, f64::INFINITY));
        let mut natural = vec![0.0f64; self.columns];
        for row in &self.rows {
            for (column, column_width) in natural.iter_mut().enumerate().take(row.len) {
                let size = self.cells[row.start + column].layout_child(ctx, &loose);
                *column_width = column_width.max(size.width + 2.0 * CELL_PAD);
            }
        }
        // `w-full`: spare width is shared out equally, and an over-wide table is
        // scaled down proportionally (see the module docs on overflow).
        let total: f64 = natural.iter().sum();
        self.col_widths = if total <= 0.0 {
            vec![width / self.columns as f64; self.columns]
        } else if total < width {
            let slack = (width - total) / self.columns as f64;
            natural.iter().map(|w| w + slack).collect()
        } else {
            let scale = width / total;
            natural.iter().map(|w| w * scale).collect()
        };

        // Pass 2: place every cell inside its column, and measure the rows.
        let mut y = 0.0;
        for index in 0..self.rows.len() {
            let (start, len, kind) = {
                let row = &self.rows[index];
                (row.start, row.len, row.kind)
            };
            let mut content = 0.0f64;
            for column in 0..len {
                let cell_width = (self.col_widths[column] - 2.0 * CELL_PAD).max(0.0);
                let size = self.cells[start + column].layout_child(
                    ctx,
                    &BoxConstraints::new(Size::ZERO, Size::new(cell_width, f64::INFINITY)),
                );
                content = content.max(size.height);
            }
            let height = match kind {
                // `h-10` on a head cell, so the head row has a floor.
                RowKind::Head => HEAD_HEIGHT.max(content + 2.0 * CELL_PAD),
                _ => content + 2.0 * CELL_PAD,
            };
            let mut x = 0.0;
            for column in 0..len {
                let size = self.cells[start + column].size();
                self.cells[start + column].set_origin(Point::new(
                    x + CELL_PAD,
                    y + ((height - size.height) / 2.0).max(0.0),
                ));
                x += self.col_widths[column];
            }
            self.rows[index].top = y;
            self.rows[index].height = height;
            y += height;
        }

        // `caption-bottom`: below the table, `mt-4` away.
        if let Some(caption) = self.caption.as_mut() {
            let size = caption.layout_child(ctx, &loose);
            caption.set_origin(Point::new(0.0, y + CAPTION_GAP));
            y += CAPTION_GAP + size.height;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: the pointer leaving the table sends it no
        // event, so this is what clears the latched row.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let theme = Theme::from_paint_ctx(ctx);
        let border = border_color(theme);
        let muted = muted(theme);
        let wash = style::with_alpha(muted, MUTED_WASH_ALPHA);
        let last_body = self.rows.iter().rposition(|row| row.kind == RowKind::Body);

        for (index, row) in self.rows.iter().enumerate() {
            let top = origin.y + row.top;
            let row_size = Size::new(size.width, row.height);
            let fill = match row.kind {
                // `bg-muted/50` on the footer, and on a hovered or armed row;
                // `data-[state=selected]:bg-muted` wins over both.
                RowKind::Foot => Some(wash),
                RowKind::Body if row.selected => Some(muted),
                RowKind::Body if self.hovered == Some(index) || self.pressed == Some(index) => {
                    Some(wash)
                }
                _ => None,
            };
            if let Some(fill) = fill {
                scene.fill_rect(Point::new(origin.x, top), row_size, fill);
            }
            // `[&_tr]:border-b` on the head and body, `[&_tr:last-child]:border-0`
            // on the body's last row, `border-t` on the footer.
            let rule_y = match row.kind {
                RowKind::Foot => Some(top),
                RowKind::Body if Some(index) == last_body && !self.has_footer() => None,
                _ => Some(top + row.height - style::BORDER_WIDTH),
            };
            if let Some(rule_y) = rule_y {
                scene.fill_rect(
                    Point::new(origin.x, rule_y),
                    Size::new(size.width, style::BORDER_WIDTH),
                    border,
                );
            }
        }
        for pod in &mut self.cells {
            pod.paint_child(ctx, scene);
        }
        if let Some(caption) = self.caption.as_mut() {
            caption.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Cells first, always: a cell-level control (a checkbox, a link) owns the
        // event, and the row's own hover claim must come *after* the routing so
        // the cell's claim is the one recorded.
        let routed = route_event(&mut self.cells, ctx, event);
        if event.is_broadcast() {
            if let Some(caption) = self.caption.as_mut() {
                caption.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if routed == EventResult::Handled {
            return routed;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if !self.captured {
                    let row = self.row_at(p.position, size);
                    // The container claims as a fallback: a claiming cell was
                    // recorded first and wins, and this call is then a no-op that
                    // still leaves the row hovered through the path.
                    if row.is_some() {
                        ctx.claim_hover();
                    }
                    if let Some(index) = row
                        && self.is_activatable(index)
                    {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != row {
                        self.hovered = row;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                if let Some(armed) = self.pressed
                    && self.is_activatable(armed)
                {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                let over = self.row_at(p.position, size);
                let armed = over.filter(|index| self.pressed == Some(*index));
                if self.pressed != armed {
                    self.pressed = armed;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                let Some(index) = self
                    .row_at(p.position, size)
                    .filter(|i| self.is_activatable(*i))
                else {
                    return EventResult::Ignored;
                };
                self.pressed = Some(index);
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let armed = self.pressed.take();
                if let Some(index) = armed
                    && self.row_at(p.position, size) == Some(index)
                    && let Some(on_click) = self.rows[index].on_click.as_mut()
                {
                    on_click(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Flags and a redraw only: a Cancel arm never touches app state.
                self.captured = false;
                self.pressed = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Table,
            |_node| {},
            |ctx| {
                for row in &self.rows {
                    ctx.push_container(
                        Role::Row,
                        |node| {
                            if row.on_click.is_some() {
                                node.add_action(Action::Click);
                            }
                        },
                        |ctx| {
                            let cell_role = match row.kind {
                                RowKind::Head => Role::ColumnHeader,
                                _ => Role::Cell,
                            };
                            for pod in &self.cells[row.start..row.start + row.len] {
                                ctx.push_container(
                                    cell_role,
                                    |_node| {},
                                    |ctx| pod.semantics_child(ctx),
                                );
                            }
                        },
                    );
                }
                if let Some(caption) = self.caption.as_ref() {
                    caption.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(cells, caption);
}

impl TableWidget {
    /// Whether a footer row is rendered (which is what decides the body's last
    /// row keeps its rule).
    fn has_footer(&self) -> bool {
        self.rows.iter().any(|row| row.kind == RowKind::Foot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use frust::{Brightness, CursorIcon, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A recording scene capturing the plain rects the table paints: row washes
    /// and 1px rules.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    impl Recorder {
        /// The 1px rules, by their y origin.
        fn rules(&self, color: Color) -> Vec<f64> {
            self.rects
                .iter()
                .filter(|(_, size, c)| size.height == style::BORDER_WIDTH && *c == color)
                .map(|(origin, _, _)| origin.y)
                .collect()
        }

        /// The washes (anything taller than a rule), by `(y, color)`.
        fn washes(&self) -> Vec<(f64, Color)> {
            self.rects
                .iter()
                .filter(|(_, size, _)| size.height > style::BORDER_WIDTH)
                .map(|(origin, _, color)| (origin.y, *color))
                .collect()
        }
    }

    const WINDOW: Size = Size::new(400.0, 300.0);

    #[derive(Default)]
    struct AppState {
        clicked: Vec<usize>,
    }

    fn simple() -> TableView<AppState> {
        table([
            table_row([table_cell("INV001"), table_cell("Paid")])
                .on_click(|s: &mut AppState| s.clicked.push(0)),
            table_row([table_cell("INV002"), table_cell("Pending")])
                .on_click(|s: &mut AppState| s.clicked.push(1)),
        ])
        .header(["Invoice", "Status"])
    }

    fn build<S: 'static>(view: &TableView<S>) -> TableWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut TableWidget, size: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    fn paint(w: &mut TableWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        if let Some(theme) = theme {
            let mut ctx = ctx.with_theme(theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
        } else {
            w.paint(&mut ctx, &mut rec);
        }
        rec
    }

    struct Harness {
        root: RenderRoot<AppState, TableView<AppState>>,
        state: AppState,
        tcx: TextContext,
    }

    impl Harness {
        fn new() -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            let mut logic = |_s: &mut AppState| simple();
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) -> bool {
            self.root
                .event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: PointerButton::Primary,
                    }),
                )
                .needs_redraw
        }

        /// The y-centre of body row `index` (row 0 is the header).
        fn body_y(&self, index: usize) -> f64 {
            let row = row_geometry()
                .into_iter()
                .filter(|(kind, _, _)| *kind == RowKind::Body)
                .nth(index)
                .expect("body row");
            row.1 + row.2 / 2.0
        }
    }

    /// `(kind, top, height)` per rendered row of [`simple`] at [`WINDOW`].
    ///
    /// A standalone build/layout of the same view under the same constraints, so
    /// it reproduces the harness's geometry exactly — the root exposes no seam to
    /// read a live widget's own fields back out.
    fn row_geometry() -> Vec<(RowKind, f64, f64)> {
        let mut w = build(&simple());
        layout(&mut w, WINDOW);
        w.rows
            .iter()
            .map(|row| (row.kind, row.top, row.height))
            .collect()
    }

    #[test]
    fn columns_fill_the_width_and_the_head_row_is_h10() {
        let mut w = build(&simple());
        let size = layout(&mut w, WINDOW);
        assert_eq!(size.width, WINDOW.width);
        assert_eq!(w.columns, 2);
        let total: f64 = w.col_widths.iter().sum();
        assert!((total - WINDOW.width).abs() < 1e-9, "w-full: {total}");
        assert_eq!(w.rows[0].kind, RowKind::Head);
        assert_eq!(w.rows[0].height, HEAD_HEIGHT);
        // Body rows are content + `p-2` top and bottom, and stack under the head.
        assert_eq!(w.rows[1].top, HEAD_HEIGHT);
        assert!(w.rows[1].height > 2.0 * CELL_PAD);
        assert_eq!(size.height, w.rows[2].top + w.rows[2].height);
        // Cells are inset by the cell padding and land in their own column.
        assert_eq!(w.cells[0].origin().x, CELL_PAD);
        assert_eq!(w.cells[1].origin().x, w.col_widths[0] + CELL_PAD);
    }

    #[test]
    fn an_overwide_table_scales_its_columns_instead_of_overflowing() {
        // Long labels in a narrow box: the natural widths exceed the width, so
        // every column is scaled down and the table still fits (there is no
        // horizontal scroll to hand it to — see the module docs).
        let wide: TableView<AppState> = table([table_row([
            table_cell("a rather long first cell value"),
            table_cell("and an equally long second one"),
        ])]);
        let mut w = build(&wide);
        let size = layout(&mut w, Size::new(120.0, 200.0));
        assert_eq!(size.width, 120.0);
        let total: f64 = w.col_widths.iter().sum();
        assert!((total - 120.0).abs() < 1e-9, "scaled to fit: {total}");
    }

    #[test]
    fn rules_sit_under_the_head_and_every_body_row_but_the_last() {
        let mut w = build(&simple());
        let size = layout(&mut w, WINDOW);
        let rec = paint(&mut w, size, None);
        let rules = rec.rules(FALLBACK.border);
        // Head row + first body row, and *not* the last body row.
        assert_eq!(rules.len(), 2, "got {rules:?}");
        assert_eq!(rules[0], HEAD_HEIGHT - style::BORDER_WIDTH);
        assert_eq!(
            rules[1],
            w.rows[1].top + w.rows[1].height - style::BORDER_WIDTH
        );
    }

    #[test]
    fn a_footer_washes_muted_keeps_a_top_rule_and_restores_the_bodys_last_rule() {
        let view: TableView<AppState> = table([table_row([table_cell("INV001")])])
            .header(["Invoice"])
            .footer(["Total"]);
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        let rec = paint(&mut w, size, None);
        let footer = w.rows.last().expect("footer row");
        assert_eq!(footer.kind, RowKind::Foot);
        assert_eq!(
            rec.washes(),
            vec![(
                footer.top,
                style::with_alpha(FALLBACK.muted, MUTED_WASH_ALPHA)
            )],
            "bg-muted/50 on the footer only"
        );
        let rules = rec.rules(FALLBACK.border);
        assert!(
            rules.contains(&footer.top),
            "border-t on the footer, got {rules:?}"
        );
        assert_eq!(rules.len(), 3, "head, body (no longer last), footer top");
    }

    #[test]
    fn a_selected_row_washes_muted_at_full_strength() {
        let view: TableView<AppState> = table([
            table_row([table_cell("a")]),
            table_row([table_cell("b")]).selected(true),
        ]);
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.washes(),
            vec![(w.rows[1].top, theme.scheme().surface_container_highest)]
        );
    }

    #[test]
    fn hovering_a_row_latches_washes_and_clears_when_the_pointer_leaves() {
        let mut h = Harness::new();
        assert!(h.paint().washes().is_empty(), "no wash at rest");

        let y = h.body_y(0);
        assert!(h.pointer(PointerPhase::Move, 40.0, y), "entry repaints");
        let washes = h.paint().washes();
        assert_eq!(washes.len(), 1, "the hovered row washes: {washes:?}");
        // An unchanged latch asks for nothing more.
        assert!(!h.pointer(PointerPhase::Move, 60.0, y));

        // Row to row: the latch changes, so the frame is asked for.
        let next = h.body_y(1);
        assert!(h.pointer(PointerPhase::Move, 40.0, next));
        assert_eq!(h.paint().washes().len(), 1);

        // Off the table entirely: the paint-time read is what clears it.
        h.pointer(PointerPhase::Move, 40.0, WINDOW.height - 1.0);
        assert!(h.paint().washes().is_empty());
    }

    #[test]
    fn a_row_fires_on_up_inside_and_asks_for_the_pointer_cursor() {
        let mut h = Harness::new();
        let y = h.body_y(1);
        h.pointer(PointerPhase::Move, 40.0, y);
        assert_eq!(h.root.cursor(), CursorIcon::Pointer);

        h.pointer(PointerPhase::Down, 40.0, y);
        assert!(h.state.clicked.is_empty(), "never on down");
        h.pointer(PointerPhase::Up, 40.0, y);
        assert_eq!(h.state.clicked, vec![1]);

        // Release on a *different* row does not fire either callback.
        h.pointer(PointerPhase::Down, 40.0, y);
        h.pointer(PointerPhase::Up, 40.0, h.body_y(0));
        assert_eq!(h.state.clicked, vec![1]);

        // A cancelled press fires nothing.
        h.pointer(PointerPhase::Down, 40.0, y);
        h.pointer(PointerPhase::Cancel, 40.0, y);
        assert_eq!(h.state.clicked, vec![1]);
    }

    #[test]
    fn the_head_row_is_not_hoverable_and_not_activatable() {
        let mut h = Harness::new();
        let head = row_geometry()[0];
        assert_eq!(head.0, RowKind::Head);
        let y = head.1 + head.2 / 2.0;
        h.pointer(PointerPhase::Move, 40.0, y);
        assert!(h.paint().washes().is_empty(), "no hover wash on the head");
        h.pointer(PointerPhase::Down, 40.0, y);
        h.pointer(PointerPhase::Up, 40.0, y);
        assert!(h.state.clicked.is_empty());
    }

    #[test]
    fn a_caption_sits_below_the_table_mt_4_away() {
        let view: TableView<AppState> =
            table([table_row([table_cell("a")])]).caption("A list of invoices.");
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        let caption = w.caption.as_ref().expect("caption pod");
        let last = w.rows.last().expect("row");
        assert_eq!(caption.origin().y, last.top + last.height + CAPTION_GAP);
        assert_eq!(size.height, caption.origin().y + caption.size().height);
    }

    #[test]
    fn a_shape_change_rebuilds_the_cells() {
        let mut counter = 0u64;
        let view = simple();
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.cells.len(), 6, "2 head + 2 rows x 2");
        let next: TableView<AppState> = table([table_row([
            table_cell("INV001"),
            table_cell("Paid"),
            table_cell("$250"),
        ])])
        .header(["Invoice", "Status", "Amount"]);
        let flags =
            View::<AppState>::rebuild(&next, &view, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.columns, 3);
        assert_eq!(w.cells.len(), 6, "3 head + 3 body");
    }

    #[test]
    fn visit_children_publishes_every_cell_and_the_caption() {
        let view: TableView<AppState> = table([table_row([table_cell("a"), table_cell("b")])])
            .header(["A", "B"])
            .caption("c");
        let w = build(&view);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 5, "4 cells + the caption");
    }

    #[test]
    fn semantics_is_a_table_of_rows_with_column_headers_and_cells() {
        let mut h = Harness::new();
        h.pass();
        let update = h.root.semantics();
        assert!(update.nodes.iter().any(|(_, n)| n.role() == Role::Table));
        assert_eq!(
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == Role::Row)
                .count(),
            3,
            "head + two body rows"
        );
        assert_eq!(
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == Role::ColumnHeader)
                .count(),
            2
        );
        assert!(
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == Role::Row)
                .any(|(_, n)| n.supports_action(Action::Click)),
            "an activatable row exposes the Click action"
        );
    }
}
