//! `command`: the filterable command palette — a search field over a grouped,
//! keyboard-navigable list.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/command.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17), a `cmdk`
//! `Command` wrapper: a `rounded-md bg-popover` panel, a
//! `flex h-9 items-center gap-2 border-b px-3` input row led by a `size-4`
//! search icon, and a `max-h-[300px] overflow-y-auto` list of
//! `px-2 py-1.5 text-sm rounded-sm` items with `data-[selected=true]:bg-accent`,
//! `text-xs text-muted-foreground` group headings, `h-px bg-border` separators,
//! `ml-auto text-xs` shortcuts and a `py-6 text-center text-sm` empty state.
//!
//! # Shape of the port
//!
//! Upstream `cmdk` is a composition of primitives an app fills with arbitrary
//! JSX. Here the list is **data**: a [`CommandItem`] carries a label, an optional
//! shortcut, an optional group and a disabled flag, and [`command`] renders the
//! rows. That is what lets the widget own the filter, the highlight and the
//! keyboard contract without re-deriving a child protocol for each of them.
//!
//! * **Filter**: a case-insensitive substring match on the label, v1. No fuzzy
//!   scoring, no keyword field, no custom `filter` hook.
//! * **Groups and separators**: rows are grouped by [`CommandItem::group`] in
//!   item order, a heading opens each group, and a `h-px bg-border` separator is
//!   drawn *between* groups automatically. Upstream's explicit `CommandSeparator`
//!   has no equivalent: an automatic rule can never be left stranded by a filter
//!   that hides everything around it.
//! * **Empty**: when nothing matches, one `py-6 text-sm` row carries
//!   [`CommandView::empty`]'s text. Upstream centres it (`text-center`); the
//!   authoring seam exposes no text alignment, so this port leaves it
//!   start-aligned.
//!
//! # Controlled query, internal highlight
//!
//! The **query** is controlled like every other input in the catalog: it comes
//! in as a prop and every edit is reported through `on_query_change`. The
//! **highlight** is not app data — it is transient view state, like a pressed
//! flag — so the widget owns it, exactly as `cmdk` does, and resets it to the
//! first selectable row whenever the visible row set changes. Selection is
//! reported through `on_select` with the item's index **into the unfiltered
//! list**, so a caller can act on it without re-running the filter.
//!
//! # Keyboard
//!
//! The widget intercepts three keys *before* the search field sees them, and
//! forwards everything else (characters, Backspace, arrows-within-text):
//!
//! | key | effect |
//! |---|---|
//! | `ArrowDown`/`ArrowUp` | move the highlight to the next/previous selectable row (clamped, scrolling it into view) |
//! | `Enter` | select the highlighted row |
//! | `Escape` | **deliberately not handled and not forwarded** |
//!
//! Escape is the load-bearing one: the baseline field treats Escape as *blur*
//! and consumes it, which in a palette would swallow the very key that closes
//! it. Returning `Ignored` without forwarding lets the enclosing
//! [`crate::overlay::modal`] (or [`crate::overlay::anchored`]) host dismiss on it
//! — and leaves the field's focus alone, so the palette stays typable.
//!
//! # Two inherited limits
//!
//! * **No auto-focus on appear.** The framework has no focus-on-mount hook (the
//!   same gap `frust_material::dialog` documents for Escape), so the user must
//!   click the search field once before typing. Clicking anywhere else in the
//!   palette still claims focus for the *widget*, which is what makes the arrows
//!   and Enter work.
//! * **The search field paints an opaque `--background`.** It wraps
//!   [`frust::text_input`], whose background is the theme's `surface` role with
//!   no override seam — upstream's input is `bg-transparent` over the panel's
//!   `bg-popover`. The two tokens are identical inside a
//!   [`command_dialog`] (whose panel is `bg-background`); an inline [`command`]
//!   shows a `--background` strip behind its search row in a theme where the two
//!   differ. The same wrapping gap [`crate::input`] documents, and the reason
//!   this module ports the input's *look* (its own row chrome and metrics)
//!   rather than reusing that widget.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, ScrollDelta, SemanticsCtx, Size, ThemeTextColor,
    View, Widget, any, build_child, erase_callback_arg, rebuild_child, rebuild_children,
    route_event_single, teardown_child, text::FontWeight, visit_children,
};
use frust::input::WHEEL_LINE_PX;
use frust::{
    Axis, CrossAxisAlignment, EdgeInsets, FlexView, NavigatorController, Padding, PopResult,
    SizedBox, Theme, flexible, inflexible, text, text_input,
};

use crate::overlay::modal::MAX_WIDTH_LG;
use crate::overlay::{self, ModalConfig, ModalContent, ModalView, ModalWidget, modal};
use crate::style;
use crate::tokens::ShadcnTokens;

/// `h-9` — the input row's height.
const INPUT_HEIGHT: f64 = style::HEIGHT_DEFAULT;
/// `px-3` — the input row's horizontal padding.
const INPUT_PAD_X: f64 = 12.0;
/// `gap-2` — the gap between the search icon and the field.
const INPUT_GAP: f64 = 8.0;
/// `p-1` — the list's padding.
const LIST_PAD: f64 = 4.0;
/// `max-h-[300px]` — the command list viewport's cap.
pub const COMMAND_MAX_LIST_HEIGHT: f64 = 300.0;
/// `px-2` — an item/heading row's horizontal padding.
const ROW_PAD_X: f64 = 8.0;
/// `py-1.5` — an item/heading row's vertical padding.
const ROW_PAD_Y: f64 = 6.0;
/// `py-6` — the empty row's vertical padding.
const EMPTY_PAD_Y: f64 = 24.0;
/// `h-px` — a separator row's height.
const SEPARATOR_HEIGHT: f64 = 1.0;
/// `opacity-50` on the search icon.
const SEARCH_ICON_OPACITY: f32 = 0.5;
/// Width used when the incoming constraints are horizontally unbounded — the
/// panel is normally sized by its host (a centred modal, an anchored popover).
const UNBOUNDED_WIDTH: f64 = 320.0;
/// Flattening tolerance for the search glyph's arc path.
const PATH_TOLERANCE: f64 = 0.1;
/// Lucide's icon viewBox edge, and its nominal stroke width in the same units.
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Lucide's nominal stroke width, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

/// One entry in a [`command`] list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItem {
    label: String,
    shortcut: Option<String>,
    group: Option<String>,
    disabled: bool,
}

/// Create an item labelled `label`.
pub fn command_item(label: impl Into<String>) -> CommandItem {
    CommandItem {
        label: label.into(),
        shortcut: None,
        group: None,
        disabled: false,
    }
}

impl CommandItem {
    /// Set the trailing shortcut hint (`ml-auto text-xs text-muted-foreground`).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Put the item in a named group — consecutive items sharing a group name
    /// are rendered under one heading (see the [module docs](self)).
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Make the item unselectable (`data-[disabled=true]`): it still matches the
    /// filter and still renders, but the highlight skips it and it reports no
    /// selection.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The item's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether this item survives `query` — a case-insensitive substring match
    /// on the label (v1; see the [module docs](self)).
    pub fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        self.label.to_lowercase().contains(&query.to_lowercase())
    }
}

/// A rendered row, parallel to the widget's row pods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandRow {
    /// A group heading.
    Heading,
    /// A `h-px bg-border` rule between two groups.
    Separator,
    /// A selectable row, carrying its index into the *unfiltered* item list.
    Item {
        /// The unfiltered index reported through `on_select`.
        index: usize,
        /// Whether the item refuses selection.
        disabled: bool,
    },
    /// The "no results" row.
    Empty,
}

impl CommandRow {
    /// Whether the highlight may land here.
    fn selectable(self) -> bool {
        matches!(
            self,
            CommandRow::Item {
                disabled: false,
                ..
            }
        )
    }
}

/// A view-held, typed query callback (erased on build).
type OnQuery<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held, typed selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative shadcn command palette. See the [module docs](self).
pub struct CommandView<State: 'static> {
    items: Vec<CommandItem>,
    query: String,
    placeholder: String,
    empty: String,
    on_query_change: OnQuery<State>,
    on_select: OnSelect<State>,
}

/// Create a command palette over `items`, filtered by the controlled `query`.
///
/// `on_query_change(state, text)` reports each edit of the search field;
/// `on_select(state, index)` reports an activation, with `index` counted into
/// `items` as given (not into the filtered rows).
pub fn command<State: 'static, F, G>(
    items: Vec<CommandItem>,
    query: impl Into<String>,
    on_query_change: F,
    on_select: G,
) -> CommandView<State>
where
    F: Fn(&mut State, String) + 'static,
    G: Fn(&mut State, usize) + 'static,
{
    CommandView {
        items,
        query: query.into(),
        placeholder: "Type a command or search...".to_string(),
        empty: "No results found.".to_string(),
        on_query_change: Rc::new(on_query_change),
        on_select: Rc::new(on_select),
    }
}

impl<State: 'static> CommandView<State> {
    /// Set the search field's placeholder.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the text shown when nothing matches the query.
    pub fn empty(mut self, empty: impl Into<String>) -> Self {
        self.empty = empty.into();
        self
    }

    /// The wrapped baseline search field, with its own chrome suppressed (this
    /// widget paints the row's rule and icon).
    fn input(&self) -> AnyView<State> {
        let on_change = self.on_query_change.clone();
        any(
            text_input(self.query.clone(), move |state: &mut State, text| {
                on_change(state, text)
            })
            .placeholder(self.placeholder.clone())
            .padding(0.0, 0.0)
            .border_width(0.0)
            .corner_radius(0.0)
            .focus_ring_width(0.0),
        )
    }

    /// The visible rows for the current query, as views plus their kinds.
    fn rows(&self) -> (Vec<AnyView<State>>, Vec<CommandRow>) {
        let mut views = Vec::new();
        let mut kinds = Vec::new();
        let mut group: Option<&str> = None;
        let mut first = true;
        for (index, item) in self.items.iter().enumerate() {
            if !item.matches(&self.query) {
                continue;
            }
            let item_group = item.group.as_deref();
            if item_group != group || first {
                if !first {
                    views.push(separator_view());
                    kinds.push(CommandRow::Separator);
                }
                if let Some(name) = item_group {
                    views.push(heading_view(name));
                    kinds.push(CommandRow::Heading);
                }
                group = item_group;
                first = false;
            }
            views.push(item_view(item));
            kinds.push(CommandRow::Item {
                index,
                disabled: item.disabled,
            });
        }
        if kinds.is_empty() {
            views.push(empty_view(&self.empty));
            kinds.push(CommandRow::Empty);
        }
        (views, kinds)
    }
}

/// A group heading row: `px-2 py-1.5 text-xs font-medium text-muted-foreground`.
fn heading_view<State: 'static>(name: &str) -> AnyView<State> {
    any(Padding(
        EdgeInsets::symmetric(ROW_PAD_X, ROW_PAD_Y),
        text(name.to_string())
            .size(style::TEXT_XS as f32)
            .weight(FontWeight::MEDIUM)
            .themed_role(ThemeTextColor::OnSurfaceVariant),
    ))
}

/// A separator row: `h-px`, painted as a `bg-border` rule by the widget.
fn separator_view<State: 'static>() -> AnyView<State> {
    any(SizedBox(None, Some(SEPARATOR_HEIGHT)))
}

/// The empty row: `py-6 text-sm`.
fn empty_view<State: 'static>(label: &str) -> AnyView<State> {
    any(Padding(
        EdgeInsets::symmetric(ROW_PAD_X, EMPTY_PAD_Y),
        text(label.to_string())
            .size(style::TEXT_SM as f32)
            .themed_role(ThemeTextColor::OnSurfaceVariant),
    ))
}

/// An item row: `px-2 py-1.5 text-sm`, with the shortcut pushed to the trailing
/// edge (`ml-auto text-xs text-muted-foreground`).
///
/// A disabled item takes the muted ink rather than upstream's `opacity-50`:
/// the authoring seam's themed text roles carry no alpha, and the muted role is
/// the catalog's dimmed ink.
fn item_view<State: 'static>(item: &CommandItem) -> AnyView<State> {
    let label = text(item.label.clone()).size(style::TEXT_SM as f32);
    let label = if item.disabled {
        label.themed_role(ThemeTextColor::OnSurfaceVariant)
    } else {
        label.themed_role(ThemeTextColor::OnSurface)
    };
    let mut children = vec![inflexible(label), flexible(1, SizedBox(None, None))];
    if let Some(shortcut) = &item.shortcut {
        children.push(inflexible(
            text(shortcut.clone())
                .size(style::TEXT_XS as f32)
                .themed_role(ThemeTextColor::OnSurfaceVariant),
        ));
    }
    any(Padding(
        EdgeInsets::symmetric(ROW_PAD_X, ROW_PAD_Y),
        FlexView::new(Axis::Horizontal, children).cross_axis(CrossAxisAlignment::Center),
    ))
}

/// Paint lucide's `search` glyph (a circle plus a handle) centred on `center`,
/// `extent` px on a side.
fn draw_search(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    // `circle cx=11 cy=11 r=8` + `path m21 21-4.35-4.35`, viewBox-relative to
    // its centre (12, 12).
    let ring = kurbo::Circle::new(Point::new(-scale, -scale), 8.0 * scale);
    let mut path: BezPath = kurbo::Shape::to_path(&ring, PATH_TOLERANCE);
    path.move_to(Point::new(9.0 * scale, 9.0 * scale));
    path.line_to(Point::new(4.65 * scale, 4.65 * scale));
    scene.stroke_path(center, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// The retained widget for a [`CommandView`].
pub struct CommandWidget {
    /// `[search field, row 0, row 1, …]` — one flat list so
    /// [`visit_children`](frust::authoring::visit_children) publishes them all.
    pods: Vec<ChildPod>,
    rows: Vec<CommandRow>,
    /// Each row's `(top, height)` inside the list's own scrollable content.
    offsets: Vec<(f64, f64)>,
    /// The highlighted row, if any row is selectable.
    highlight: Option<usize>,
    /// The row the pointer is over, if any.
    hovered: Option<usize>,
    /// The list's scroll offset, in logical px.
    scroll: f64,
    /// The list viewport's height and its scrollable content height.
    viewport: f64,
    content_height: f64,
    on_select: ErasedArgCallback<usize>,
}

impl CommandWidget {
    /// The highlighted row index, for a caller (or a test) inspecting the
    /// palette's keyboard state.
    pub fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// The list's scroll offset, in logical px.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// The first selectable row at or after `from`, searching in `step`
    /// direction.
    fn seek(&self, from: usize, step: isize) -> Option<usize> {
        let mut i = from as isize;
        while i >= 0 && (i as usize) < self.rows.len() {
            if self.rows[i as usize].selectable() {
                return Some(i as usize);
            }
            i += step;
        }
        None
    }

    /// Reset the highlight to the first selectable row.
    fn reset_highlight(&mut self) {
        self.highlight = self.seek(0, 1);
    }

    /// Move the highlight one selectable row in `step`'s direction, clamped at
    /// both ends (`cmdk`'s own non-looping default).
    fn move_highlight(&mut self, step: isize) {
        let Some(current) = self.highlight else {
            self.reset_highlight();
            return;
        };
        let next = current as isize + step;
        if next < 0 || next as usize >= self.rows.len() {
            return;
        }
        if let Some(target) = self.seek(next as usize, step) {
            self.highlight = Some(target);
            self.scroll_into_view(target);
        }
    }

    /// Scroll so row `index` is inside the viewport.
    fn scroll_into_view(&mut self, index: usize) {
        let Some(&(top, height)) = self.offsets.get(index) else {
            return;
        };
        if top < self.scroll {
            self.scroll = top;
        } else if top + height > self.scroll + self.viewport {
            self.scroll = top + height - self.viewport;
        }
        self.clamp_scroll();
        self.apply_scroll();
    }

    /// Keep the scroll offset inside the scrollable range.
    fn clamp_scroll(&mut self) {
        let max = (self.content_height - self.viewport).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// Place the row pods for the current scroll offset.
    ///
    /// Called from `layout` and again from the event arms that scroll: an
    /// `EventCtx` can request a redraw but not a relayout, so the rows are
    /// repositioned in place rather than waiting for a layout pass that a mere
    /// scroll never triggers.
    fn apply_scroll(&mut self) {
        let list_top = INPUT_HEIGHT + style::BORDER_WIDTH + LIST_PAD;
        for (pod, (top, _)) in self.pods.iter_mut().skip(1).zip(self.offsets.iter()) {
            pod.set_origin(Point::new(LIST_PAD, list_top + top - self.scroll));
        }
    }

    /// Report the selection of row `index`, if it is selectable.
    fn select(&mut self, ctx: &mut EventCtx, row: usize) {
        if let Some(CommandRow::Item {
            index,
            disabled: false,
        }) = self.rows.get(row).copied()
        {
            (self.on_select)(ctx, index);
        }
    }

    /// The list viewport's rect in the widget's own space.
    fn list_rect(&self, size: Size) -> Rect {
        Rect::new(
            0.0,
            INPUT_HEIGHT + style::BORDER_WIDTH,
            size.width,
            size.height,
        )
    }

    /// The row under widget-local `pos`, if the pointer is inside the list.
    fn row_at(&self, pos: Point, size: Size) -> Option<usize> {
        if !self.list_rect(size).contains(pos) {
            return None;
        }
        // The pods carry the resolved (scrolled) positions, so this is a plain
        // vertical hit test against them.
        self.rows.iter().enumerate().find_map(|(i, _)| {
            let pod = self.pods.get(i + 1)?;
            let top = pod.origin().y;
            (pos.y >= top && pos.y < top + pod.size().height).then_some(i)
        })
    }
}

impl<State: 'static> View<State> for CommandView<State> {
    type Element = CommandWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CommandWidget {
        let (views, rows) = self.rows();
        let mut pods = vec![build_child(&self.input(), ctx)];
        pods.extend(views.iter().map(|view| build_child(view, ctx)));
        let mut widget = CommandWidget {
            pods,
            rows,
            offsets: Vec::new(),
            highlight: None,
            hovered: None,
            scroll: 0.0,
            viewport: 0.0,
            content_height: 0.0,
            on_select: erase_callback_arg(&self.on_select),
        };
        widget.reset_highlight();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CommandWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.input(), &self.input(), &mut element.pods[0], ctx);

        let (prev_views, _) = prev.rows();
        let (next_views, next_rows) = self.rows();
        let prev_refs: Vec<&AnyView<State>> = prev_views.iter().collect();
        let next_refs: Vec<&AnyView<State>> = next_views.iter().collect();
        // The input pod is index 0, so the row pods reconcile as their own list.
        let mut row_pods: Vec<ChildPod> = element.pods.drain(1..).collect();
        flags |= rebuild_children(
            &prev_refs,
            &next_refs,
            &mut row_pods,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );
        element.pods.extend(row_pods);

        if element.rows != next_rows {
            // The visible set changed (a new query, new items): the highlight is
            // transient view state, so it returns to the first selectable row and
            // the list scrolls back to the top — `cmdk`'s own behavior.
            element.rows = next_rows;
            element.reset_highlight();
            element.scroll = 0.0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_select = erase_callback_arg(&self.on_select);
        flags
    }

    fn teardown(&self, element: &mut CommandWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.input(), &mut element.pods[0], ctx);
        let (views, _) = self.rows();
        for (view, pod) in views.iter().zip(element.pods.iter_mut().skip(1)) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for CommandWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };

        // The input row: the search icon leads, the field takes the rest.
        let lead = INPUT_PAD_X + style::ICON_SIZE + INPUT_GAP;
        let field_width = (width - lead - INPUT_PAD_X).max(0.0);
        self.pods[0].layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(field_width, INPUT_HEIGHT)),
        );
        self.pods[0].set_origin(Point::new(lead, 0.0));

        // The list: every row measured at the padded width, stacked.
        let row_width = (width - 2.0 * LIST_PAD).max(0.0);
        let row_bc = BoxConstraints::new(
            Size::new(row_width, 0.0),
            Size::new(row_width, f64::INFINITY),
        );
        self.offsets.clear();
        let mut y = 0.0;
        for pod in self.pods.iter_mut().skip(1) {
            let size = pod.layout_child(ctx, &row_bc);
            self.offsets.push((y, size.height));
            y += size.height;
        }
        self.content_height = y;
        self.viewport = y.min(COMMAND_MAX_LIST_HEIGHT);
        self.clamp_scroll();
        self.apply_scroll();

        let list_top = INPUT_HEIGHT + style::BORDER_WIDTH + LIST_PAD;
        bc.constrain(Size::new(width, list_top + self.viewport + LIST_PAD))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: a pointer that left this widget sends it
        // nothing, so the latched row is cleared from the path membership.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let (panel, border, accent, ink, radius, row_radius) = {
            let theme = Theme::from_paint_ctx(ctx);
            let radii = ShadcnTokens::resolve_radius(None, theme);
            (
                overlay::popover(theme),
                overlay::border(theme),
                overlay::accent(theme),
                overlay::foreground(theme),
                radii.md,
                radii.sm,
            )
        };

        // `rounded-md bg-popover`.
        scene.fill_rounded_rect(origin, size, radius, panel);

        // The input row: a `size-4` search icon at 50%, then the field, then the
        // row's `border-b`.
        draw_search(
            scene,
            Point::new(
                origin.x + INPUT_PAD_X + style::ICON_SIZE / 2.0,
                origin.y + INPUT_HEIGHT / 2.0,
            ),
            style::ICON_SIZE,
            style::with_alpha(ink, SEARCH_ICON_OPACITY),
        );
        self.pods[0].paint_child(ctx, scene);
        scene.fill_rect(
            Point::new(origin.x, origin.y + INPUT_HEIGHT),
            Size::new(size.width, style::BORDER_WIDTH),
            border,
        );

        // The list, clipped to its viewport (`overflow-y-auto`).
        let list = self.list_rect(size);
        scene.push_clip(
            Point::new(origin.x, origin.y + list.y0),
            Size::new(size.width, list.height()),
        );
        for (i, row) in self.rows.iter().enumerate() {
            let Some(pod) = self.pods.get_mut(i + 1) else {
                continue;
            };
            let pod_origin = Point::new(origin.x + pod.origin().x, origin.y + pod.origin().y);
            let pod_size = pod.size();
            match row {
                CommandRow::Separator => {
                    // `-mx-1 h-px bg-border`: the rule spans the panel, cancelling
                    // the list's own `p-1`.
                    scene.fill_rect(
                        Point::new(origin.x, pod_origin.y),
                        Size::new(size.width, SEPARATOR_HEIGHT),
                        border,
                    );
                }
                // `data-[selected=true]:bg-accent rounded-sm` — the highlight
                // and the hovered row read the same, as they do in `cmdk`.
                CommandRow::Item { .. } if self.highlight == Some(i) || self.hovered == Some(i) => {
                    scene.fill_rounded_rect(pod_origin, pod_size, row_radius, accent);
                }
                _ => {}
            }
            pod.paint_child(ctx, scene);
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in self.pods.iter_mut() {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            match &key.key {
                // Not handled *and* not forwarded — the field would consume it as
                // a blur, and the host must see it to dismiss (module docs).
                Key::Named(NamedKey::Escape) => return EventResult::Ignored,
                Key::Named(NamedKey::ArrowDown) => {
                    self.move_highlight(1);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    self.move_highlight(-1);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(row) = self.highlight {
                        self.select(ctx, row);
                        return EventResult::Handled;
                    }
                    return EventResult::Ignored;
                }
                _ => return route_event_single(&mut self.pods[0], ctx, event),
            }
        }
        // Everything else the field owns goes to the field (IME, and pointer
        // events inside the input row).
        if matches!(event, InputEvent::Ime(_)) {
            return route_event_single(&mut self.pods[0], ctx, event);
        }
        let size = ctx.size();
        if let InputEvent::Scroll { position, delta } = event {
            if !self.list_rect(size).contains(*position) {
                return EventResult::Ignored;
            }
            let dy = match delta {
                ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                ScrollDelta::Pixels(_, y) => *y,
            };
            let before = self.scroll;
            self.scroll += dy;
            self.clamp_scroll();
            if (self.scroll - before).abs() > f64::EPSILON {
                self.apply_scroll();
                ctx.request_redraw();
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        // The input row is the field's; the list is this widget's.
        if p.position.y < INPUT_HEIGHT || self.pods[0].is_active() {
            return route_event_single(&mut self.pods[0], ctx, event);
        }
        let row = self.row_at(p.position, size);
        match p.phase {
            PointerPhase::Move => {
                // Claimed after the field routing above (the claim-ordering rule).
                if row.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                let hovered = row.filter(|i| self.rows[*i].selectable());
                if self.hovered != hovered {
                    self.hovered = hovered;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                let Some(row) = row.filter(|i| self.rows[*i].selectable()) else {
                    return EventResult::Ignored;
                };
                // Focus is what routes the arrows and Enter here afterwards.
                ctx.request_focus();
                self.highlight = Some(row);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(row) = row.filter(|i| self.rows[*i].selectable()) else {
                    return EventResult::Ignored;
                };
                if self.highlight == Some(row) {
                    self.select(ctx, row);
                }
                EventResult::Handled
            }
            // A `Cancel` arm touches no app state — there is no press flag of its
            // own to clear here beyond the highlight, which is view state the next
            // move re-establishes.
            PointerPhase::Cancel => EventResult::Handled,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The search field publishes its own `TextInput` node…
        if let Some(field) = self.pods.first() {
            field.semantics_child(ctx);
        }
        // …and the rows sit under one list container.
        ctx.push_container(
            Role::ListBox,
            |_node| {},
            |ctx| {
                for pod in self.pods.iter().skip(1) {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(pods);
}

/// A declarative shadcn command dialog — [`command`] inside a centred modal
/// panel. See [`command_dialog`].
pub struct CommandDialogView<State: 'static> {
    inner: ModalView<State>,
}

/// The command-dialog panel's chrome: the dialog's centred `max-w-lg` panel with
/// `p-0 overflow-hidden`, and no close X (see [`command_dialog`]).
fn dialog_config() -> ModalConfig {
    ModalConfig::centered(MAX_WIDTH_LG).close_button(false)
}

/// Wrap `command` in a centred modal panel — upstream's `CommandDialog`.
///
/// # The close X is off by default
///
/// Upstream defaults `showCloseButton` to `true` here, but the panel is `p-0`,
/// so the `top-4 right-4` X lands on top of the search row's trailing edge.
/// This port defaults it off and leaves it one call away
/// ([`CommandDialogView::close_button`]); Escape and a scrim tap both still
/// dismiss.
pub fn command_dialog<State: 'static>(command: CommandView<State>) -> CommandDialogView<State> {
    CommandDialogView {
        inner: modal(command, dialog_config()),
    }
}

impl<State: 'static> CommandDialogView<State> {
    /// Label the dialog's accessibility node (upstream's screen-reader-only
    /// `DialogTitle`, "Command Palette").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Show or hide the close X (default `false` — see [`command_dialog`]'s
    /// own note on why).
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.inner.config = self.inner.config.close_button(close_button);
        self
    }

    /// Set the dismiss callback — a scrim tap or Escape.
    /// [`show_command_dialog`] wires this to `controller.pop()`.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.inner = self.inner.on_dismiss(on_dismiss);
        self
    }
}

impl<State: 'static> View<State> for CommandDialogView<State> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

impl<State: 'static> ModalContent<State> for CommandDialogView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.inner = self.inner.on_modal_dismiss(on_dismiss);
        self
    }
}

/// Push `build`'s command dialog as a transparent navigator page and register
/// `on_result` for the value it pops with — [`crate::show_dialog`]'s shape, over
/// the palette.
pub fn show_command_dialog<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> CommandDialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    crate::overlay::show_modal(controller, build, on_result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::modal::tests::{Recorder, WINDOW, escape, pointer};
    use frust::authoring::{KeyEvent, Modifiers, PointerButton, PointerEvent, text::TextContext};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        query: String,
        selected: Vec<usize>,
        dismissed: u32,
    }

    fn items() -> Vec<CommandItem> {
        vec![
            command_item("Calendar").group("Suggestions"),
            command_item("Search Emoji").group("Suggestions"),
            command_item("Calculator")
                .group("Suggestions")
                .disabled(true),
            command_item("Profile").group("Settings").shortcut("⌘P"),
            command_item("Billing").group("Settings").shortcut("⌘B"),
        ]
    }

    fn view(query: &str) -> CommandView<AppState> {
        command(
            items(),
            query,
            |s: &mut AppState, text| s.query = text,
            |s: &mut AppState, index| s.selected.push(index),
        )
    }

    const PANEL: Size = Size::new(320.0, 400.0);

    struct Harness {
        root: RenderRoot<AppState, CommandView<AppState>>,
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
            h.root
                .set_theme(Box::new(crate::theme().with_brightness(Brightness::Light)));
            h.pass();
            h
        }

        fn pass(&mut self) {
            let mut logic = |state: &mut AppState| view(&state.query.clone());
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(PANEL, &mut self.tcx as &mut dyn Any);
        }

        fn key(&mut self, key: Key) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key,
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }
    }

    fn build(v: &CommandView<AppState>) -> CommandWidget {
        let mut counter = 0u64;
        View::<AppState>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CommandWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(PANEL))
    }

    fn dispatch(w: &mut CommandWidget, state: &mut AppState, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(PANEL.width, 200.0));
        w.event(&mut ctx, event)
    }

    fn key_event(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn the_rows_are_grouped_headed_and_separated() {
        let mut w = build(&view(""));
        layout(&mut w);
        // Suggestions: heading + 3 items; Settings: separator + heading + 2.
        assert_eq!(
            w.rows,
            vec![
                CommandRow::Heading,
                CommandRow::Item {
                    index: 0,
                    disabled: false
                },
                CommandRow::Item {
                    index: 1,
                    disabled: false
                },
                CommandRow::Item {
                    index: 2,
                    disabled: true
                },
                CommandRow::Separator,
                CommandRow::Heading,
                CommandRow::Item {
                    index: 3,
                    disabled: false
                },
                CommandRow::Item {
                    index: 4,
                    disabled: false
                },
            ]
        );
        assert_eq!(w.highlight, Some(1), "the first selectable row");
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_match() {
        let mut w = build(&view("cal"));
        layout(&mut w);
        assert_eq!(
            w.rows,
            vec![
                CommandRow::Heading,
                CommandRow::Item {
                    index: 0,
                    disabled: false
                },
                CommandRow::Item {
                    index: 2,
                    disabled: true
                },
            ],
            "`Calendar` and `Calculator`, in their own group"
        );

        // Nothing matches: one empty row and no highlight.
        let mut w = build(&view("zzz"));
        layout(&mut w);
        assert_eq!(w.rows, vec![CommandRow::Empty]);
        assert_eq!(w.highlight, None);
        assert!(command_item("Calendar").matches("CAL"));
        assert!(command_item("Calendar").matches(""));
    }

    #[test]
    fn typing_reports_the_query_and_reruns_the_filter() {
        let mut h = Harness::new();
        // Focus the field, then type into it.
        h.pointer(PointerPhase::Down, 100.0, INPUT_HEIGHT / 2.0);
        h.pointer(PointerPhase::Up, 100.0, INPUT_HEIGHT / 2.0);
        for ch in ['b', 'i'] {
            h.key(Key::Character(ch.to_string()));
        }
        assert_eq!(h.state.query, "bi");
        h.pass();
        // Only `Billing` survives — one heading plus one item.
        assert_eq!(h.state.selected, Vec::<usize>::new());
        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(
            h.state.selected,
            vec![4],
            "Enter selects the only surviving row"
        );
    }

    #[test]
    fn the_arrows_move_the_highlight_over_selectable_rows_only() {
        let mut w = build(&view(""));
        layout(&mut w);
        let mut state = AppState::default();
        assert_eq!(w.highlight, Some(1));
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(w.highlight, Some(2));
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(
            w.highlight,
            Some(6),
            "the disabled item, the separator and the heading are all skipped"
        );
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowDown)),
        );
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(w.highlight, Some(7), "clamped at the last row");

        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowUp)),
        );
        assert_eq!(w.highlight, Some(6));

        // Enter reports the highlighted item's *unfiltered* index.
        dispatch(&mut w, &mut state, &key_event(Key::Named(NamedKey::Enter)));
        assert_eq!(state.selected, vec![3]);
    }

    #[test]
    fn escape_is_neither_handled_nor_forwarded() {
        let mut w = build(&view(""));
        layout(&mut w);
        let mut state = AppState::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &escape()),
            EventResult::Ignored,
            "the host dismisses on Escape; the field must not eat it"
        );
    }

    #[test]
    fn a_click_on_a_row_selects_it_and_a_disabled_row_is_inert() {
        let mut w = build(&view(""));
        let size = layout(&mut w);
        let mut state = AppState::default();
        let row_center = |w: &CommandWidget, i: usize| {
            let pod = &w.pods[i + 1];
            Point::new(
                pod.origin().x + pod.size().width / 2.0,
                pod.origin().y + pod.size().height / 2.0,
            )
        };
        let p = row_center(&w, 2);
        assert!(p.y < size.height);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, p.x, p.y));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, p.x, p.y));
        assert_eq!(state.selected, vec![1]);

        // The disabled row takes neither the highlight nor a selection.
        let disabled = row_center(&w, 3);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, disabled.x, disabled.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, disabled.x, disabled.y),
        );
        assert_eq!(state.selected, vec![1]);
        assert_eq!(w.highlight, Some(2));
    }

    #[test]
    fn hovering_a_row_latches_it_and_asks_for_the_pointer_cursor() {
        let mut h = Harness::new();
        let (x, y) = {
            let mut point = Point::ZERO;
            h.root.paint(&mut Recorder::default(), FrameTime::ZERO);
            // The second row (the first item) sits just under the input row.
            point.x = 100.0;
            point.y = INPUT_HEIGHT + style::BORDER_WIDTH + LIST_PAD + 40.0;
            (point.x, point.y)
        };
        h.pointer(PointerPhase::Move, x, y);
        assert_eq!(h.root.cursor(), frust::CursorIcon::Pointer);
    }

    #[test]
    fn the_list_is_capped_and_scrolls_the_highlight_into_view() {
        // Enough items to overflow the 300px viewport.
        let many: Vec<CommandItem> = (0..40).map(|i| command_item(format!("Item {i}"))).collect();
        let v: CommandView<AppState> = command(
            many,
            "",
            |_s: &mut AppState, _t| {},
            |s: &mut AppState, i| s.selected.push(i),
        );
        let mut w = build(&v);
        let size = layout(&mut w);
        assert!(w.content_height > COMMAND_MAX_LIST_HEIGHT);
        assert_eq!(w.viewport, COMMAND_MAX_LIST_HEIGHT);
        assert_eq!(
            size.height,
            INPUT_HEIGHT + style::BORDER_WIDTH + 2.0 * LIST_PAD + COMMAND_MAX_LIST_HEIGHT
        );

        let mut state = AppState::default();
        assert_eq!(w.scroll(), 0.0);
        for _ in 0..20 {
            dispatch(
                &mut w,
                &mut state,
                &key_event(Key::Named(NamedKey::ArrowDown)),
            );
        }
        assert!(w.scroll() > 0.0, "the highlight scrolled into view");
        assert!(w.scroll() <= w.content_height - w.viewport);

        // A wheel scroll moves the same offset, baseline `offset + dy` convention
        // (frust-widgets' `ScrollView`/`ListView`). Reset to the top first, since
        // the ArrowDown loop above already left `scroll` at its max — a wheel
        // scroll from there would just clamp back to max and hide the growth.
        w.scroll = 0.0;
        w.apply_scroll();
        let before = w.scroll();
        dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(100.0, INPUT_HEIGHT + 20.0),
                delta: ScrollDelta::Lines(0.0, 1.0),
            },
        );
        assert!(w.scroll() > before);
    }

    #[test]
    fn the_panel_paints_popover_chrome_a_rule_and_an_accent_highlight() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let mut w = build(&view(""));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        // `bg-popover` panel, then the highlighted row's `bg-accent`.
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_high);
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().primary_container),
            "the highlighted row paints `bg-accent`"
        );
        // The input row's `border-b` and the between-group separator.
        let rules: Vec<_> = rec
            .rects
            .iter()
            .filter(|(_, s, c)| s.height == style::BORDER_WIDTH && *c == theme.scheme().outline)
            .collect();
        assert_eq!(rules.len(), 2);
        // The search glyph, drawn at 50% ink.
        assert!(
            rec.strokes
                .iter()
                .any(|(_, _, c)| c.components[3] == SEARCH_ICON_OPACITY)
        );
    }

    #[test]
    fn semantics_publishes_the_field_and_a_listbox_of_rows() {
        let mut h = Harness::new();
        h.pass();
        let update = h.root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::TextInput),
            "the search field's node"
        );
        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListBox)
            .expect("a ListBox node");
        assert!(!list.children().is_empty());
    }

    #[test]
    fn the_command_dialog_wraps_the_palette_in_a_centred_panel() {
        let v = command_dialog(view("")).on_dismiss(|s: &mut AppState| s.dismissed += 1);
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&v, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        let panel = w.panel_rect();
        assert!(panel.width() <= MAX_WIDTH_LG);
        assert!(w.close_rect().is_none(), "no close X over the search row");

        // Escape reaches the modal through the palette and dismisses it.
        let mut state = AppState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, &escape());
        assert_eq!(state.dismissed, 1);
    }
}
