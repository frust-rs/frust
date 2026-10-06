//! `dropdown_menu`: the anchored menu a trigger opens, plus the **shared menu
//! list** the context menu and the select list are built from.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/dropdown-menu.tsx` (shadcn/ui v4,
//! rev `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a
//! Radix `DropdownMenu` whose content is `min-w-[8rem] rounded-md border
//! bg-popover p-1 text-popover-foreground shadow-md` over rows of
//! `px-2 py-1.5 text-sm rounded-sm` items with `focus:bg-accent
//! focus:text-accent-foreground`, `data-[disabled]:opacity-50` items, `pl-8`
//! checkbox/radio items carrying a `left-2` indicator (`CheckIcon` / a filled
//! `CircleIcon`), `px-2 py-1.5 text-sm font-medium` labels, `-mx-1 my-1 h-px
//! bg-border` separators, `ml-auto text-xs` shortcuts, and `SubTrigger` rows
//! (`ChevronRightIcon`, `data-[state=open]:bg-accent`) opening a `shadow-lg`
//! sub-content panel.
//!
//! # Shape of the port
//!
//! Like [`crate::command`], the list is **data**: a [`DropdownMenuItem`] carries
//! a label, an optional shortcut, an indicator, a disabled flag and an optional
//! submenu, and the widget renders the rows. That is what lets one widget own the
//! hover latch, the keyboard model and the submenu chaining instead of inventing
//! a child protocol for each.
//!
//! Items are numbered **depth-first, pre-order** across the whole tree — a
//! sub-trigger takes an index of its own, then its children take the ones that
//! follow — and `on_select` reports that index, so a caller matches on a number
//! it can read straight off its own item list.
//!
//! # Submenus
//!
//! A submenu is a nested list inside the same widget: the sub-trigger's panel is
//! built up front (it is cheap and paints nothing while closed) and is painted,
//! placed and routed to only while its row is open. Hovering a sub-trigger opens
//! it, hovering any other row closes it, `ArrowRight`/`ArrowLeft` open and close
//! it from the keyboard, and the chain nests to any depth because a submenu is
//! the same view as its parent.
//!
//! Two upstream behaviors this composition cannot reproduce:
//!
//! * **No collision handling for a submenu.** The overlay host places exactly one
//!   rect — the top-level panel — so a nested panel has no area to flip or clamp
//!   against and always opens to the trailing side of its row.
//! * **No safe-triangle grace period.** Radix keeps a submenu open while the
//!   pointer travels diagonally toward it; here the first move onto another row
//!   closes it.
//!
//! # Keyboard
//!
//! `ArrowDown`/`ArrowUp` move the highlight over selectable rows only (labels,
//! separators and disabled items are skipped), `Enter` selects the highlighted
//! row (or opens its submenu), `ArrowRight`/`ArrowLeft` open/close a submenu, and
//! `Escape` is **left unhandled** so the enclosing [`crate::overlay::anchored`]
//! host dismisses on it — the same contract [`crate::command`] documents. The
//! keys arrive only once the list holds focus, which its own `Down` arm claims;
//! there is no focus-on-appear hook in the framework, so a menu opened by a click
//! on its trigger needs one press inside itself before the arrows work.

use std::cell::RefCell;
use std::rc::Rc;

use frust::authoring::{
    AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Role, ScrollDelta, SemanticsCtx, Size,
    ThemeTextColor, ThemeTextType, View, Widget, any, build_child, erase_callback_arg,
    rebuild_children, route_event_single, teardown_child, text::FontWeight, visit_children,
};
use frust::input::WHEEL_LINE_PX;
use frust::{
    Axis, CrossAxisAlignment, EdgeInsets, FlexView, Padding, SizedBox, Theme, flexible, inflexible,
    text,
};

use crate::components::native_select::draw_chevron;
use crate::components::popover::{MENU_PADDING, PanelHandle, PanelStyle, PanelView, panel};
use crate::hit::presses;
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, SIDE_OFFSET, anchored,
};
use crate::style::{self, PATH_TOLERANCE};
use crate::tokens::ShadcnTokens;

/// `px-2` — an item/label row's horizontal padding.
const ROW_PAD_X: f64 = 8.0;
/// `py-1.5` — an item/label row's vertical padding.
const ROW_PAD_Y: f64 = 6.0;
/// `pl-8`/`pr-8` — the gutter a row reserves for its indicator.
const INDICATOR_GUTTER: f64 = 32.0;
/// `left-2`/`right-2` — the indicator's own inset inside that gutter.
const INDICATOR_INSET: f64 = 8.0;
/// `my-1` — the margin above and below a separator's rule.
const SEPARATOR_MARGIN: f64 = 4.0;
/// `h-px` — a separator's rule.
const SEPARATOR_HEIGHT: f64 = 1.0;
/// `size-2` — the radio indicator's filled dot.
const RADIO_DOT: f64 = 8.0;
/// The gap between a sub-trigger's row and the panel it opens.
const SUBMENU_GAP: f64 = SIDE_OFFSET;
/// Quarter turn: the rotation that turns lucide's `chevron-down` into its
/// `chevron-right`.
const CHEVRON_RIGHT: f64 = -std::f64::consts::FRAC_PI_2;
/// Lucide's icon viewBox edge, and its nominal stroke width in the same units.
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Lucide's nominal stroke width, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

/// What a row draws in its indicator gutter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MenuIndicator {
    /// Nothing — a plain item.
    #[default]
    None,
    /// A `CheckIcon`, drawn only while checked (Radix's `ItemIndicator` renders
    /// nothing otherwise).
    Check(bool),
    /// A filled `CircleIcon`, drawn only while selected.
    Radio(bool),
}

impl MenuIndicator {
    /// Whether this indicator draws anything right now.
    fn is_set(self) -> bool {
        matches!(
            self,
            MenuIndicator::Check(true) | MenuIndicator::Radio(true)
        )
    }

    /// Whether this indicator reserves a gutter at all.
    fn reserves_gutter(self) -> bool {
        !matches!(self, MenuIndicator::None)
    }
}

/// Which side of a row its indicator gutter sits on: leading for the menus
/// (`pl-8`), trailing for the select list (`pr-8`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MenuIndicatorSide {
    /// `pl-8` with the indicator at `left-2`.
    #[default]
    Leading,
    /// `pr-8` with the indicator at `right-2`.
    Trailing,
}

/// One row of a menu list: an item, a label, or a separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropdownMenuItem {
    label: String,
    shortcut: Option<String>,
    disabled: bool,
    indicator: MenuIndicator,
    kind: MenuRowKind,
    submenu: Vec<DropdownMenuItem>,
}

/// Which of the three row shapes a [`DropdownMenuItem`] is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MenuRowKind {
    /// A selectable item.
    #[default]
    Item,
    /// A `px-2 py-1.5 font-medium` group label.
    Label,
    /// A `-mx-1 my-1 h-px bg-border` rule.
    Separator,
}

/// Create a selectable menu item labelled `label`.
pub fn dropdown_menu_item(label: impl Into<String>) -> DropdownMenuItem {
    DropdownMenuItem {
        label: label.into(),
        shortcut: None,
        disabled: false,
        indicator: MenuIndicator::None,
        kind: MenuRowKind::Item,
        submenu: Vec::new(),
    }
}

/// Create a group label (`DropdownMenuLabel`).
pub fn dropdown_menu_label(label: impl Into<String>) -> DropdownMenuItem {
    DropdownMenuItem {
        kind: MenuRowKind::Label,
        ..dropdown_menu_item(label)
    }
}

/// Create a separator rule (`DropdownMenuSeparator`).
pub fn dropdown_menu_separator() -> DropdownMenuItem {
    DropdownMenuItem {
        kind: MenuRowKind::Separator,
        ..dropdown_menu_item("")
    }
}

impl DropdownMenuItem {
    /// Set the trailing shortcut hint (`DropdownMenuShortcut`: `ml-auto text-xs
    /// text-muted-foreground`; its `tracking-widest` letter-spacing has no
    /// authoring-seam equivalent and is not applied).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Make the item unselectable: it still renders, but the highlight skips it
    /// and it reports no selection.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Turn the item into a `DropdownMenuCheckboxItem` showing `checked`.
    pub fn checked(mut self, checked: bool) -> Self {
        self.indicator = MenuIndicator::Check(checked);
        self
    }

    /// Turn the item into a `DropdownMenuRadioItem` showing `selected`.
    pub fn radio(mut self, selected: bool) -> Self {
        self.indicator = MenuIndicator::Radio(selected);
        self
    }

    /// Turn the item into a `DropdownMenuSubTrigger` opening `items`.
    pub fn submenu(mut self, items: Vec<DropdownMenuItem>) -> Self {
        self.submenu = items;
        self
    }

    /// The item's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// How many indices this item and its whole subtree consume in the
    /// depth-first numbering (see the [module docs](self)).
    fn span(&self) -> usize {
        1 + self
            .submenu
            .iter()
            .map(DropdownMenuItem::span)
            .sum::<usize>()
    }
}

/// The chrome differences between the menu list's three users.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MenuListStyle {
    /// Which side the indicator gutter sits on.
    pub indicator_side: MenuIndicatorSide,
    /// Whether labels take the select's `text-xs text-muted-foreground` instead
    /// of the menu's `text-sm font-medium`.
    pub muted_labels: bool,
    /// The list viewport's cap, past which it scrolls.
    pub max_height: Option<f64>,
    /// Whether the initial highlight lands on the row whose indicator is set
    /// (the select's "open on the current value") rather than the first row.
    pub highlight_indicated: bool,
}

impl MenuListStyle {
    /// The dropdown/context menu's own list chrome.
    pub fn menu() -> Self {
        MenuListStyle {
            indicator_side: MenuIndicatorSide::Leading,
            muted_labels: false,
            max_height: None,
            highlight_indicated: false,
        }
    }
}

/// A rendered row, parallel to the widget's row pods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuRow {
    /// A selectable item.
    Item {
        /// The depth-first index reported through `on_select`.
        index: usize,
        /// Whether the item refuses selection.
        disabled: bool,
        /// What the indicator gutter draws.
        indicator: MenuIndicator,
        /// Whether the row opens a submenu.
        submenu: bool,
    },
    /// A group label.
    Label,
    /// A separator rule.
    Separator,
}

impl MenuRow {
    /// Whether the highlight may land here.
    fn selectable(self) -> bool {
        matches!(
            self,
            MenuRow::Item {
                disabled: false,
                ..
            }
        )
    }

    /// Whether this row opens a submenu.
    fn has_submenu(self) -> bool {
        matches!(self, MenuRow::Item { submenu: true, .. })
    }
}

/// A view-held, typed selection callback (erased on build).
pub(crate) type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// The rows of one menu panel. See [`menu_list`].
pub(crate) struct MenuListView<State: 'static> {
    items: Vec<DropdownMenuItem>,
    style: MenuListStyle,
    base: usize,
    on_select: OnSelect<State>,
}

/// Build the row list for `items`, numbering them from `base` (see the [module
/// docs](self)).
pub(crate) fn menu_list<State: 'static>(
    items: Vec<DropdownMenuItem>,
    style: MenuListStyle,
    base: usize,
    on_select: OnSelect<State>,
) -> MenuListView<State> {
    MenuListView {
        items,
        style,
        base,
        on_select,
    }
}

/// Wrap `items` in the shared panel chrome as a complete menu panel.
pub(crate) fn menu_panel<State: 'static>(
    items: Vec<DropdownMenuItem>,
    list_style: MenuListStyle,
    panel_style: PanelHandle,
    on_select: OnSelect<State>,
) -> PanelView<State> {
    panel(menu_list(items, list_style, 0, on_select), panel_style)
}

impl<State: 'static> MenuListView<State> {
    /// Whether any item reserves an indicator gutter — the whole list keeps the
    /// same inset when one does, so labels and plain items stay aligned with the
    /// checkbox rows (upstream's `data-[inset]:pl-8`).
    fn has_gutter(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.indicator.reserves_gutter())
    }

    /// One row's padding.
    fn insets(&self, gutter: bool) -> EdgeInsets {
        let extra = if gutter { INDICATOR_GUTTER } else { ROW_PAD_X };
        let (left, right) = match self.style.indicator_side {
            MenuIndicatorSide::Leading => (extra, ROW_PAD_X),
            MenuIndicatorSide::Trailing => (ROW_PAD_X, extra),
        };
        EdgeInsets {
            left,
            top: ROW_PAD_Y,
            right,
            bottom: ROW_PAD_Y,
        }
    }

    /// The row views, their kinds, and the submenu panels their sub-trigger rows
    /// open (paired with the row they belong to).
    #[allow(clippy::type_complexity)]
    fn rows(
        &self,
    ) -> (
        Vec<AnyView<State>>,
        Vec<MenuRow>,
        Vec<(usize, AnyView<State>)>,
    ) {
        let gutter = self.has_gutter();
        let insets = self.insets(gutter);
        let mut views = Vec::new();
        let mut kinds = Vec::new();
        let mut subs = Vec::new();
        let mut index = self.base;
        for item in &self.items {
            match item.kind {
                MenuRowKind::Separator => {
                    views.push(any(SizedBox(
                        None,
                        Some(SEPARATOR_HEIGHT + 2.0 * SEPARATOR_MARGIN),
                    )));
                    kinds.push(MenuRow::Separator);
                }
                MenuRowKind::Label => {
                    views.push(label_view(&item.label, insets, self.style.muted_labels));
                    kinds.push(MenuRow::Label);
                }
                MenuRowKind::Item => {
                    let has_sub = !item.submenu.is_empty();
                    views.push(item_view(item, insets, has_sub));
                    kinds.push(MenuRow::Item {
                        index,
                        disabled: item.disabled,
                        indicator: item.indicator,
                        submenu: has_sub,
                    });
                    if has_sub {
                        let sub_style: PanelHandle = Rc::new(RefCell::new(PanelStyle::submenu()));
                        subs.push((
                            kinds.len() - 1,
                            any(panel(
                                menu_list(
                                    item.submenu.clone(),
                                    self.style,
                                    index + 1,
                                    self.on_select.clone(),
                                ),
                                sub_style,
                            )),
                        ));
                    }
                }
            }
            index += item.span();
        }
        (views, kinds, subs)
    }
}

/// A group label row: `px-2 py-1.5 text-sm font-medium`, or the select's
/// `text-xs text-muted-foreground`.
fn label_view<State: 'static>(label: &str, insets: EdgeInsets, muted: bool) -> AnyView<State> {
    let view = if muted {
        text(label.to_string())
            .size(style::TEXT_XS as f32)
            .themed_family(ThemeTextType::BodySmall)
            .themed_role(ThemeTextColor::OnSurfaceVariant)
    } else {
        text(label.to_string())
            .size(style::TEXT_SM as f32)
            .weight(FontWeight::MEDIUM)
            .themed_family(ThemeTextType::LabelLarge)
            .themed_role(ThemeTextColor::OnSurface)
    };
    any(Padding(insets, view))
}

/// An item row: the label, then the shortcut (or a sub-trigger's chevron gutter)
/// pushed to the trailing edge by an `ml-auto` spacer.
///
/// A disabled item takes the muted ink rather than upstream's `opacity-50`: the
/// authoring seam's themed text roles carry no alpha, and the muted role is the
/// catalog's dimmed ink (the same substitution [`crate::command`] makes).
fn item_view<State: 'static>(
    item: &DropdownMenuItem,
    insets: EdgeInsets,
    has_sub: bool,
) -> AnyView<State> {
    let label = text(item.label.clone())
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium);
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
                .themed_family(ThemeTextType::BodySmall)
                .themed_role(ThemeTextColor::OnSurfaceVariant),
        ));
    }
    if has_sub {
        // `ml-auto size-4`: the chevron is drawn by the widget, so the row only
        // reserves its box.
        children.push(inflexible(SizedBox(Some(style::ICON_SIZE), None)));
    }
    any(Padding(
        insets,
        FlexView::new(Axis::Horizontal, children).cross_axis(CrossAxisAlignment::Center),
    ))
}

/// Paint lucide's `check` glyph (`M20 6 9 17l-5-5`) centred on `center`.
pub(crate) fn draw_check(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    // viewBox-relative to its centre (12, 12).
    path.move_to(Point::new(8.0 * scale, -6.0 * scale));
    path.line_to(Point::new(-3.0 * scale, 5.0 * scale));
    path.line_to(Point::new(-8.0 * scale, 0.0));
    scene.stroke_path(center, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint a filled `size-2` dot — the radio item's `CircleIcon fill-current`.
fn draw_dot(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let circle = kurbo::Circle::new(Point::ORIGIN, extent / 2.0);
    let path: BezPath = kurbo::Shape::to_path(&circle, PATH_TOLERANCE);
    scene.fill_path(center, &path, &Brush::Solid(color));
}

impl PanelStyle {
    /// `min-w-[8rem] p-1 shadow-lg` — a submenu panel's chrome.
    pub(crate) fn submenu() -> Self {
        PanelStyle {
            shadow: style::SHADOW_LG,
            ..PanelStyle::menu()
        }
    }
}

/// The retained widget for a [`MenuListView`].
pub(crate) struct MenuListWidget {
    /// `[row 0, row 1, …, submenu 0, submenu 1, …]` — one flat list so
    /// [`visit_children`](frust::authoring::visit_children) publishes them all.
    pods: Vec<ChildPod>,
    rows: Vec<MenuRow>,
    /// `(row index, pod index)` for each sub-trigger's panel.
    subs: Vec<(usize, usize)>,
    /// Each row's `(top, height)` inside the scrollable content.
    offsets: Vec<(f64, f64)>,
    highlight: Option<usize>,
    hovered: Option<usize>,
    /// The row whose submenu is open, if any.
    open_sub: Option<usize>,
    scroll: f64,
    viewport: f64,
    content_height: f64,
    /// The width every row was laid out at — where a submenu's panel starts.
    width: f64,
    style: MenuListStyle,
    on_select: ErasedArgCallback<usize>,
}

impl MenuListWidget {
    /// The highlighted row — introspection for this crate's own tests, since the
    /// list is never handed out as a public element type.
    #[cfg(test)]
    pub(crate) fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    /// The row whose submenu is open.
    #[cfg(test)]
    pub(crate) fn open_submenu(&self) -> Option<usize> {
        self.open_sub
    }

    /// The list's scroll offset, in logical px.
    #[cfg(test)]
    pub(crate) fn scroll(&self) -> f64 {
        self.scroll
    }

    /// The pod index of `row`'s submenu panel.
    fn sub_pod(&self, row: usize) -> Option<usize> {
        self.subs
            .iter()
            .find_map(|(r, pod)| (*r == row).then_some(*pod))
    }

    /// The first selectable row at or after `from`, searching in `step`'s
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

    /// Reset the highlight to the list's opening row.
    fn reset_highlight(&mut self) {
        self.highlight = if self.style.highlight_indicated {
            self.rows
                .iter()
                .position(|row| {
                    matches!(row, MenuRow::Item { indicator, disabled: false, .. } if indicator.is_set())
                })
                .or_else(|| self.seek(0, 1))
        } else {
            self.seek(0, 1)
        };
    }

    /// Move the highlight one selectable row in `step`'s direction, clamped at
    /// both ends.
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
            self.close_submenu();
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

    /// Place the row pods (and any open submenu) for the current scroll offset.
    ///
    /// Called from `layout` and again from the event arms that scroll: an
    /// `EventCtx` can request a redraw but not a relayout, so the rows are
    /// repositioned in place rather than waiting for a layout pass a mere scroll
    /// never triggers.
    fn apply_scroll(&mut self) {
        let width = self.width;
        for (i, (top, _)) in self.offsets.iter().enumerate() {
            if let Some(pod) = self.pods.get_mut(i) {
                pod.set_origin(Point::new(0.0, top - self.scroll));
            }
        }
        for &(row, pod_index) in self.subs.iter() {
            let Some(&(top, _)) = self.offsets.get(row) else {
                continue;
            };
            if let Some(pod) = self.pods.get_mut(pod_index) {
                // Trailing side of the panel, its own top lined up with the row's
                // (the panel's `p-1` is what the padding cancels).
                pod.set_origin(Point::new(
                    width + MENU_PADDING + SUBMENU_GAP,
                    top - self.scroll - MENU_PADDING,
                ));
            }
        }
    }

    /// Open `row`'s submenu, if it has one.
    fn open_submenu_at(&mut self, row: usize) -> bool {
        if !self
            .rows
            .get(row)
            .copied()
            .is_some_and(MenuRow::has_submenu)
        {
            return false;
        }
        if self.open_sub != Some(row) {
            self.open_sub = Some(row);
        }
        true
    }

    /// Close whatever submenu is open.
    fn close_submenu(&mut self) {
        self.open_sub = None;
    }

    /// Report the selection of row `index`, if it is selectable.
    fn select(&mut self, ctx: &mut EventCtx, row: usize) {
        if let Some(MenuRow::Item {
            index,
            disabled: false,
            ..
        }) = self.rows.get(row).copied()
        {
            (self.on_select)(ctx, index);
        }
    }

    /// The row under widget-local `pos`, if any.
    fn row_at(&self, pos: Point, size: Size) -> Option<usize> {
        if pos.x < 0.0 || pos.x >= size.width || pos.y < 0.0 || pos.y >= size.height {
            return None;
        }
        self.rows.iter().enumerate().find_map(|(i, _)| {
            let pod = self.pods.get(i)?;
            let top = pod.origin().y;
            (pos.y >= top && pos.y < top + pod.size().height).then_some(i)
        })
    }

    /// The `Widget::event` key arm: arrow/enter/escape navigation, split out of
    /// `event` itself so the dispatcher stays a plain read of the event shape.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> EventResult {
        match &key.key {
            // Neither handled nor forwarded: the host dismisses on it.
            Key::Named(NamedKey::Escape) => {
                if self.open_sub.is_some() {
                    self.close_submenu();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.move_highlight(1);
                ctx.request_redraw();
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_highlight(-1);
                ctx.request_redraw();
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some(row) = self.highlight
                    && self.open_submenu_at(row)
                {
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if self.open_sub.is_some() {
                    self.close_submenu();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            Key::Named(NamedKey::Enter) => {
                let Some(row) = self.highlight else {
                    return EventResult::Ignored;
                };
                if self.open_submenu_at(row) {
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                self.select(ctx, row);
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    /// The `Widget::event` scroll arm: the same `offset + dy` wheel convention
    /// every scrollable in this catalog shares, clamped to the content range.
    fn handle_scroll(
        &mut self,
        ctx: &mut EventCtx,
        size: Size,
        position: Point,
        delta: &ScrollDelta,
    ) -> EventResult {
        if position.y < 0.0 || position.y >= size.height {
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
        EventResult::Ignored
    }

    /// The `Widget::event` pointer arm: hover latching (with submenu
    /// open/close), press-to-highlight, and release-to-select.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, size: Size, p: &PointerEvent) -> EventResult {
        // Only a primary press operates the list. The whole gesture is refused,
        // `Up` included, because a release resolves against the highlight rather
        // than a capture flag — a secondary release would otherwise select
        // whatever row a hover had highlighted. The hover pass (`Move`), which
        // is also what opens a submenu, is untouched.
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Ignored;
        }
        let row = self.row_at(p.position, size);
        match p.phase {
            PointerPhase::Move => {
                // Claimed after the submenu routing above (the claim-ordering
                // rule): a hovered row inside an open submenu wins the claim.
                if row.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                let hovered = row.filter(|i| self.rows[*i].selectable());
                if self.hovered != hovered {
                    self.hovered = hovered;
                    match hovered {
                        Some(i) => {
                            self.highlight = Some(i);
                            // Hovering a sub-trigger opens it; hovering anything
                            // else closes whatever was open.
                            if !self.open_submenu_at(i) {
                                self.close_submenu();
                            }
                        }
                        // A non-selectable row (a separator, a group label, or a
                        // disabled item) is "anything else" too — closes whatever
                        // was open rather than leaving a stale submenu hanging
                        // open over an unrelated row.
                        None => self.close_submenu(),
                    }
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
                if self.highlight == Some(row) && !self.open_submenu_at(row) {
                    self.select(ctx, row);
                }
                EventResult::Handled
            }
            // A `Cancel` arm touches no app state: the highlight is view state
            // the next move re-establishes.
            PointerPhase::Cancel => EventResult::Handled,
        }
    }
}

impl<State: 'static> View<State> for MenuListView<State> {
    type Element = MenuListWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MenuListWidget {
        let (views, rows, subs) = self.rows();
        let mut pods: Vec<ChildPod> = views.iter().map(|view| build_child(view, ctx)).collect();
        let mut sub_index = Vec::new();
        for (row, view) in &subs {
            sub_index.push((*row, pods.len()));
            pods.push(build_child(view, ctx));
        }
        let mut widget = MenuListWidget {
            pods,
            rows,
            subs: sub_index,
            offsets: Vec::new(),
            highlight: None,
            hovered: None,
            open_sub: None,
            scroll: 0.0,
            viewport: 0.0,
            content_height: 0.0,
            width: 0.0,
            style: self.style,
            on_select: erase_callback_arg(&self.on_select),
        };
        widget.reset_highlight();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MenuListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let (prev_views, _, prev_subs) = prev.rows();
        let (next_views, next_rows, next_subs) = self.rows();
        let prev_all: Vec<&AnyView<State>> = prev_views
            .iter()
            .chain(prev_subs.iter().map(|(_, v)| v))
            .collect();
        let next_all: Vec<&AnyView<State>> = next_views
            .iter()
            .chain(next_subs.iter().map(|(_, v)| v))
            .collect();
        let mut flags = rebuild_children(
            &prev_all,
            &next_all,
            &mut element.pods,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );
        element.subs = next_subs
            .iter()
            .enumerate()
            .map(|(i, (row, _))| (*row, next_views.len() + i))
            .collect();
        if element.rows != next_rows || element.style != self.style {
            element.rows = next_rows;
            element.style = self.style;
            element.reset_highlight();
            element.close_submenu();
            element.scroll = 0.0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_select = erase_callback_arg(&self.on_select);
        flags
    }

    fn teardown(&self, element: &mut MenuListWidget, ctx: &mut BuildCtx<'_>) {
        let (views, _, subs) = self.rows();
        for (view, pod) in views
            .iter()
            .chain(subs.iter().map(|(_, v)| v))
            .zip(element.pods.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MenuListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let available = bc.max().width;
        // Pass 1 — natural widths, measured under an unbounded main axis so a
        // row's `ml-auto` spacer contributes nothing (a flex with a bounded main
        // axis and a flexible child fills it).
        let mut natural: f64 = 0.0;
        let probe = BoxConstraints::new(Size::ZERO, Size::new(f64::INFINITY, f64::INFINITY));
        for (i, pod) in self.pods.iter_mut().enumerate() {
            if i >= self.rows.len() {
                break;
            }
            natural = natural.max(pod.layout_child(ctx, &probe).width);
        }
        let width = if available.is_finite() {
            natural.min(available)
        } else {
            natural
        };
        self.width = width;

        // Pass 2 — the real layout, every row tight to the resolved width.
        let row_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        self.offsets.clear();
        let mut y = 0.0;
        for i in 0..self.rows.len() {
            let size = self.pods[i].layout_child(ctx, &row_bc);
            self.offsets.push((y, size.height));
            y += size.height;
        }
        self.content_height = y;
        self.viewport = match self.style.max_height {
            Some(cap) => y.min(cap),
            None => y,
        };
        self.clamp_scroll();

        // The submenu panels size themselves, outside this list's own box.
        let loose = BoxConstraints::loose(bc.max());
        for i in 0..self.subs.len() {
            let pod_index = self.subs[i].1;
            if let Some(pod) = self.pods.get_mut(pod_index) {
                pod.layout_child(ctx, &loose);
            }
        }
        self.apply_scroll();
        bc.constrain(Size::new(width, self.viewport))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: a pointer that left this widget sends it
        // nothing, so the latched row is cleared from the path membership.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let (accent, ink, accent_ink, muted, border, row_radius) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                crate::overlay::accent(theme),
                crate::overlay::foreground(theme),
                crate::overlay::accent_foreground(theme),
                crate::overlay::muted_foreground(theme),
                crate::overlay::border(theme),
                ShadcnTokens::resolve_radius(None, theme).sm,
            )
        };

        scene.push_clip(origin, size);
        for i in 0..self.rows.len() {
            let row = self.rows[i];
            let Some(pod) = self.pods.get_mut(i) else {
                continue;
            };
            let pod_origin = Point::new(origin.x + pod.origin().x, origin.y + pod.origin().y);
            let pod_size = pod.size();
            match row {
                MenuRow::Separator => {
                    // `-mx-1 my-1 h-px bg-border`: the rule spans the panel,
                    // cancelling its `p-1`.
                    scene.fill_rect(
                        Point::new(origin.x - MENU_PADDING, pod_origin.y + SEPARATOR_MARGIN),
                        Size::new(size.width + 2.0 * MENU_PADDING, SEPARATOR_HEIGHT),
                        border,
                    );
                }
                MenuRow::Item { indicator, .. } => {
                    // `focus:bg-accent` — the highlight and the hovered row read
                    // the same, and an open sub-trigger stays washed
                    // (`data-[state=open]:bg-accent`).
                    let washed = self.highlight == Some(i)
                        || self.hovered == Some(i)
                        || self.open_sub == Some(i);
                    if washed {
                        scene.fill_rounded_rect(pod_origin, pod_size, row_radius, accent);
                    }
                    let glyph = if washed { accent_ink } else { ink };
                    if indicator.is_set() {
                        let x = match self.style.indicator_side {
                            MenuIndicatorSide::Leading => {
                                pod_origin.x + INDICATOR_INSET + style::ICON_SIZE / 2.0
                            }
                            MenuIndicatorSide::Trailing => {
                                pod_origin.x + pod_size.width
                                    - INDICATOR_INSET
                                    - style::ICON_SIZE / 2.0
                            }
                        };
                        let center = Point::new(x, pod_origin.y + pod_size.height / 2.0);
                        match indicator {
                            MenuIndicator::Check(_) => {
                                draw_check(scene, center, style::ICON_SIZE, glyph)
                            }
                            MenuIndicator::Radio(_) => draw_dot(scene, center, RADIO_DOT, glyph),
                            MenuIndicator::None => {}
                        }
                    }
                    if row.has_submenu() {
                        // `ml-auto size-4` in `text-muted-foreground`.
                        draw_chevron(
                            scene,
                            Point::new(
                                pod_origin.x + pod_size.width - ROW_PAD_X - style::ICON_SIZE / 2.0,
                                pod_origin.y + pod_size.height / 2.0,
                            ),
                            style::ICON_SIZE,
                            CHEVRON_RIGHT,
                            if washed { accent_ink } else { muted },
                        );
                    }
                }
                MenuRow::Label => {}
            }
            pod.paint_child(ctx, scene);
        }
        scene.pop_clip();

        // The open submenu paints last and outside the list's own clip, so it
        // sits over whatever is beside it.
        if let Some(row) = self.open_sub
            && let Some(pod_index) = self.sub_pod(row)
            && let Some(pod) = self.pods.get_mut(pod_index)
        {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in self.pods.iter_mut() {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // An open submenu owns the pass first: it is the innermost panel, and a
        // key it does not use falls back to this list.
        let open_sub_pod = self.open_sub.and_then(|row| self.sub_pod(row));
        if let Some(pod_index) = open_sub_pod {
            // A key goes to the open submenu unconditionally: it is the innermost
            // panel, and Radix moves focus into a sub-content on open. Routing it
            // through the focus-path helper instead would drop it, since nothing
            // has pressed inside the submenu to record that path.
            let routed = if event.is_focus_routed() {
                self.pods[pod_index].event_child(ctx, event)
            } else {
                route_event_single(&mut self.pods[pod_index], ctx, event)
            };
            if routed == EventResult::Handled {
                return EventResult::Handled;
            }
        }

        if let InputEvent::Key(key) = event {
            return self.handle_key(ctx, key);
        }

        let size = ctx.size();
        if let InputEvent::Scroll { position, delta } = event {
            return self.handle_scroll(ctx, size, *position, delta);
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        self.handle_pointer(ctx, size, p)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Menu,
            |_node| {},
            |ctx| {
                for pod in self.pods.iter() {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(pods);
}

/// A declarative shadcn dropdown menu. See [`dropdown_menu`].
pub struct DropdownMenuView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    style: PanelHandle,
    placement: OverlayPlacement,
}

/// Build a dropdown-menu panel over `items`, to be mounted while the app's own
/// open flag is set — or kept mounted with the flag handed to
/// [`DropdownMenuView::open`] for an exit ramp (see [`crate::popover`] for both
/// mount contracts).
///
/// `on_select(state, index)` reports an activation, with `index` counted
/// depth-first over the item tree (see the [module docs](self)).
pub fn dropdown_menu<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<DropdownMenuItem>,
    on_select: F,
) -> DropdownMenuView<State> {
    let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::menu()));
    let content = menu_panel(
        items,
        MenuListStyle::menu(),
        style.clone(),
        Rc::new(on_select),
    );
    // A menu lines up with the trigger's leading edge, not its centre.
    let placement = OverlayPlacement::default().align(OverlayAlign::Start);
    DropdownMenuView {
        inner: anchored(content).placement(placement),
        style,
        placement,
    }
}

impl<State: 'static> DropdownMenuView<State> {
    /// Anchor the menu to the rect `anchor` carries.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the menu opens on (`side`, default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.apply_placement()
    }

    /// Set the cross-axis alignment (`align`, default `start`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.apply_placement()
    }

    /// Set the gap between trigger and menu (`sideOffset`, default `4`).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self.apply_placement()
    }

    /// Hand a **kept-mounted** menu the app's open flag, so closing it plays the
    /// panel's exit ramp instead of vanishing (see [`crate::popover`]).
    ///
    /// The default is `true`: a mounted menu is an open one. A selection still
    /// reports through `on_select` on the release that made it — only the pixels
    /// linger, and an open submenu fades out inside its parent.
    pub fn open(mut self, open: bool) -> Self {
        self.style.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: a press outside the menu or a focus-routed
    /// Escape reports `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }

    /// Re-hand the current placement to the host.
    fn apply_placement(mut self) -> Self {
        self.inner = self.inner.placement(self.placement);
        self
    }
}

impl<State: 'static> View<State> for DropdownMenuView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::MIN_MENU_WIDTH;
    use crate::components::popover::tests::{
        Recorder, WINDOW, escape, ft_ms, key_event, light, pointer,
    };
    use frust::authoring::Rect;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        selected: Vec<usize>,
        opens: Vec<bool>,
    }

    fn items() -> Vec<DropdownMenuItem> {
        vec![
            dropdown_menu_label("My Account"),
            dropdown_menu_item("Profile").shortcut("⇧⌘P"),
            dropdown_menu_item("Billing").disabled(true),
            dropdown_menu_separator(),
            dropdown_menu_item("Status Bar").checked(true),
            dropdown_menu_item("Invite users").submenu(vec![
                dropdown_menu_item("Email"),
                dropdown_menu_item("Message"),
            ]),
            dropdown_menu_item("Log out"),
        ]
    }

    fn list_view() -> MenuListView<AppState> {
        menu_list(
            items(),
            MenuListStyle::menu(),
            0,
            Rc::new(|s: &mut AppState, i| s.selected.push(i)),
        )
    }

    fn build(view: &MenuListView<AppState>) -> MenuListWidget {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut MenuListWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(WINDOW))
    }

    fn dispatch(w: &mut MenuListWidget, state: &mut AppState, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let size = Size::new(w.width.max(1.0), w.viewport.max(1.0));
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    /// The vertical centre of row `i`, in the list's own space.
    fn row_center(w: &MenuListWidget, i: usize) -> Point {
        let pod = &w.pods[i];
        Point::new(
            pod.origin().x + pod.size().width / 2.0,
            pod.origin().y + pod.size().height / 2.0,
        )
    }

    #[test]
    fn the_rows_are_items_labels_and_separators_numbered_depth_first() {
        let mut w = build(&list_view());
        layout(&mut w);
        assert_eq!(
            w.rows,
            vec![
                MenuRow::Label,
                MenuRow::Item {
                    index: 1,
                    disabled: false,
                    indicator: MenuIndicator::None,
                    submenu: false
                },
                MenuRow::Item {
                    index: 2,
                    disabled: true,
                    indicator: MenuIndicator::None,
                    submenu: false
                },
                MenuRow::Separator,
                MenuRow::Item {
                    index: 4,
                    disabled: false,
                    indicator: MenuIndicator::Check(true),
                    submenu: false
                },
                MenuRow::Item {
                    index: 5,
                    disabled: false,
                    indicator: MenuIndicator::None,
                    submenu: true
                },
                // The submenu's two items take 6 and 7, so `Log out` is 8.
                MenuRow::Item {
                    index: 8,
                    disabled: false,
                    indicator: MenuIndicator::None,
                    submenu: false
                },
            ]
        );
        assert_eq!(w.highlight(), Some(1), "the first selectable row");
        assert_eq!(w.subs.len(), 1, "one submenu panel, built up front");
    }

    #[test]
    fn the_arrows_traverse_selectable_rows_and_enter_selects_one() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(
            w.highlight(),
            Some(4),
            "the disabled item and the separator are skipped"
        );
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowUp)),
        );
        assert_eq!(w.highlight(), Some(1));
        dispatch(&mut w, &mut state, &key_event(Key::Named(NamedKey::Enter)));
        assert_eq!(state.selected, vec![1], "the depth-first index");

        // Clamped at the top.
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowUp)),
        );
        assert_eq!(w.highlight(), Some(1));
    }

    #[test]
    fn escape_is_left_to_the_host_unless_a_submenu_is_open() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &escape()),
            EventResult::Ignored,
            "the host dismisses on Escape"
        );
        w.highlight = Some(5);
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.open_submenu(), Some(5));
        assert_eq!(
            dispatch(&mut w, &mut state, &escape()),
            EventResult::Handled,
            "the open submenu takes it first"
        );
        assert_eq!(w.open_submenu(), None);
    }

    #[test]
    fn a_sub_trigger_chains_a_panel_on_hover_and_arrow_right_and_closes_on_arrow_left() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        let sub_row = 5;
        let p = row_center(&w, sub_row);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, p.x, p.y));
        assert_eq!(w.open_submenu(), Some(sub_row), "hover opens it");

        // Hovering another row closes it again.
        let other = row_center(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, other.x, other.y),
        );
        assert_eq!(w.open_submenu(), None);

        w.highlight = Some(sub_row);
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.open_submenu(), Some(sub_row));
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(w.open_submenu(), None);

        // A sub-trigger never reports a selection of its own.
        w.highlight = Some(sub_row);
        dispatch(&mut w, &mut state, &key_event(Key::Named(NamedKey::Enter)));
        assert_eq!(state.selected, Vec::<usize>::new());
        assert_eq!(w.open_submenu(), Some(sub_row));
    }

    #[test]
    fn hovering_a_non_selectable_row_closes_an_open_submenu() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        let sub_row = 5;
        let p = row_center(&w, sub_row);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, p.x, p.y));
        assert_eq!(w.open_submenu(), Some(sub_row), "hover opens it");

        // Row 3 is the separator between the disabled row and `Status Bar` —
        // hovering it is "anything else" too, and must close the submenu the
        // same way hovering a plain item does.
        let separator = row_center(&w, 3);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, separator.x, separator.y),
        );
        assert_eq!(
            w.open_submenu(),
            None,
            "hovering a separator closes the open submenu"
        );
    }

    #[test]
    fn a_submenu_row_reports_its_own_depth_first_index() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        w.highlight = Some(5);
        dispatch(
            &mut w,
            &mut state,
            &key_event(Key::Named(NamedKey::ArrowRight)),
        );
        assert!(w.sub_pod(5).is_some(), "a panel of its own");
        // The chained list takes the pass first, so its own highlight answers
        // Enter — the parent's `Invite users` row never selects.
        dispatch(&mut w, &mut state, &key_event(Key::Named(NamedKey::Enter)));
        assert_eq!(state.selected, vec![6], "the submenu's first item");
    }

    #[test]
    fn a_click_selects_a_row_and_a_disabled_row_is_inert() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        let p = row_center(&w, 1);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, p.x, p.y));
        assert_eq!(state.selected, Vec::<usize>::new(), "never on down");
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, p.x, p.y));
        assert_eq!(state.selected, vec![1]);

        let disabled = row_center(&w, 2);
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
        assert_eq!(state.selected, vec![1], "the disabled row reports nothing");
    }

    #[test]
    fn hovering_a_row_latches_it_and_asks_for_the_pointer_cursor() {
        let mut w = build(&list_view());
        layout(&mut w);
        let mut state = AppState::default();
        let p = row_center(&w, 1);
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(w.width, w.viewport));
        w.event(&mut ctx, &pointer(PointerPhase::Move, p.x, p.y));
        assert_eq!(w.hovered, Some(1));
        assert!(
            ctx.needs_redraw(),
            "the latch's changed-return asks for one"
        );
    }

    #[test]
    fn the_rows_paint_the_accent_wash_the_indicators_and_the_rule() {
        let theme = light();
        let mut w = build(&list_view());
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, ft_ms(0.0)).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().primary_container),
            "the highlighted row's bg-accent"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(_, s, c)| s.height == SEPARATOR_HEIGHT && *c == theme.scheme().outline),
            "the separator rule"
        );
        // The check glyph on the checked row plus the sub-trigger's chevron.
        assert!(rec.strokes.len() >= 2);
    }

    #[test]
    fn a_capped_list_scrolls_the_highlight_into_view_and_on_a_wheel() {
        let many: Vec<DropdownMenuItem> = (0..40)
            .map(|i| dropdown_menu_item(format!("Item {i}")))
            .collect();
        let style = MenuListStyle {
            max_height: Some(120.0),
            ..MenuListStyle::menu()
        };
        let view: MenuListView<AppState> = menu_list(
            many,
            style,
            0,
            Rc::new(|s: &mut AppState, i| s.selected.push(i)),
        );
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size.height, 120.0, "the viewport is capped");
        assert!(w.content_height > size.height);

        let mut state = AppState::default();
        assert_eq!(w.scroll(), 0.0);
        for _ in 0..10 {
            dispatch(
                &mut w,
                &mut state,
                &key_event(Key::Named(NamedKey::ArrowDown)),
            );
        }
        assert!(w.scroll() > 0.0, "the highlight scrolled into view");
        let max_scroll = w.content_height - w.viewport;
        assert!(w.scroll() <= max_scroll);

        // Walk the highlight back to the first row through the same public key
        // route used above — `scroll_into_view` lands `scroll` back at 0.0
        // (the row's own top offset), so there is no need to poke the private
        // field directly.
        for _ in 0..10 {
            dispatch(
                &mut w,
                &mut state,
                &key_event(Key::Named(NamedKey::ArrowUp)),
            );
        }
        assert_eq!(w.scroll(), 0.0, "walked back to the top row");

        // A wheel scroll moves the same offset, baseline `offset + dy` convention
        // (frust-widgets' `ScrollView`/`ListView`): a positive `y` delta scrolls
        // down (increases the offset).
        let scroll_at = Point::new(20.0, 20.0);
        let before = w.scroll();
        dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: scroll_at,
                delta: ScrollDelta::Lines(0.0, 1.0),
            },
        );
        assert!(w.scroll() > before, "a positive delta scrolls down");

        // Wheel past the max clamps at the bottom rather than overshooting.
        dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: scroll_at,
                delta: ScrollDelta::Lines(0.0, 1000.0),
            },
        );
        assert_eq!(w.scroll(), max_scroll, "clamped at the bottom");

        // Wheel back up past the top clamps at 0 rather than going negative.
        dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: scroll_at,
                delta: ScrollDelta::Lines(0.0, -1000.0),
            },
        );
        assert_eq!(w.scroll(), 0.0, "clamped at the top");
    }

    #[test]
    fn the_panel_is_at_least_the_menu_floor_wide() {
        let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::menu()));
        let view: PanelView<AppState> = menu_panel(
            vec![dropdown_menu_item("Ok")],
            MenuListStyle::menu(),
            style,
            Rc::new(|_s: &mut AppState, _i| {}),
        );
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(WINDOW));
        assert_eq!(size.width, MIN_MENU_WIDTH, "min-w-[8rem]");
    }

    /// `duration-200`, the shared ramp the panel exits over.
    const RAMP_MS: f64 = 200.0;

    /// A kept-mounted menu: always in the tree, handed the app's flag, which is
    /// the mount an exit ramp needs (see [`crate::overlay::anchored`]).
    struct KeptHarness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
        open: bool,
        clock: f64,
    }

    impl KeptHarness {
        fn new() -> Self {
            let anchor = OverlayAnchor::new();
            anchor.set(Rect::new(20.0, 20.0, 100.0, 56.0));
            let mut h = KeptHarness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor,
                open: true,
                clock: 0.0,
            };
            h.root.set_theme(Box::new(light()));
            h.pass();
            h
        }

        fn pass(&mut self) {
            let anchor = self.anchor.clone();
            let open = self.open;
            let mut logic = move |_s: &mut AppState| {
                frust::stack().child(
                    dropdown_menu(items(), |s: &mut AppState, i| s.selected.push(i))
                        .anchor(&anchor)
                        .open(open)
                        .on_open_change(|s: &mut AppState, o| s.opens.push(o)),
                )
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        fn paint_at(&mut self, ms: f64) -> Recorder {
            self.clock = ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        /// The centre of `Profile`, the first selectable row — under the panel's
        /// `p-1` and the `My Account` group label above it.
        fn first_item_row(&self) -> Point {
            let anchor = self.anchor.rect();
            Point::new(
                anchor.x0 + 20.0,
                anchor.y1 + SIDE_OFFSET + MENU_PADDING + 24.0 + 14.0,
            )
        }

        /// Whether the panel's `bg-popover` box was drawn.
        fn painted(rec: &Recorder) -> bool {
            let theme = light();
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().surface_container_high)
        }
    }

    #[test]
    fn a_kept_mounted_menu_paints_out_its_exit_and_consumes_nothing_meanwhile() {
        let mut h = KeptHarness::new();
        h.paint_at(RAMP_MS * 2.0);
        assert!(KeptHarness::painted(&h.paint_at(RAMP_MS * 2.0)));
        let panel = h.first_item_row();

        // The app closes it; the widget stays mounted and ramps out.
        h.open = false;
        h.pass();
        let start = h.clock;
        let mid = h.paint_at(start + RAMP_MS / 2.0);
        assert!(KeptHarness::painted(&mid), "still on screen");
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);

        // A press over the closing panel reaches the page under it.
        let before = h.state.opens.len();
        let outcome = h
            .root
            .event(&mut h.state, &pointer(PointerPhase::Down, panel.x, panel.y));
        assert!(
            outcome.handled,
            "a closing menu swallows a press that lands on it"
        );
        assert_eq!(h.state.selected, Vec::<usize>::new(), "and selects nothing");
        assert_eq!(h.state.opens.len(), before, "and dismisses nothing");

        assert!(
            !KeptHarness::painted(&h.paint_at(start + RAMP_MS * 2.0)),
            "gone at settle"
        );
    }

    #[test]
    fn a_kept_mounted_menu_that_reopens_is_live_again() {
        let mut h = KeptHarness::new();
        h.paint_at(RAMP_MS * 2.0);
        h.open = false;
        h.pass();
        h.paint_at(h.clock + RAMP_MS * 2.0);
        assert!(!KeptHarness::painted(&h.paint_at(h.clock)));

        h.open = true;
        h.pass();
        let reopened = h.paint_at(h.clock + RAMP_MS * 2.0);
        assert!(KeptHarness::painted(&reopened), "the entrance ran again");
        let row = h.first_item_row();
        h.root
            .event(&mut h.state, &pointer(PointerPhase::Down, row.x, row.y));
        h.root
            .event(&mut h.state, &pointer(PointerPhase::Up, row.x, row.y));
        assert_eq!(h.state.selected, vec![1], "`Profile`, the first item row");
    }

    #[test]
    fn the_menu_mounts_through_the_host_and_closes_on_a_press_outside() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(20.0, 20.0, 100.0, 56.0));
        let a = anchor.clone();
        let mut root: RenderRoot<AppState, frust::StackView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        root.set_theme(Box::new(light()));
        let mut logic = move |_s: &mut AppState| {
            frust::stack().child(
                dropdown_menu(items(), |s: &mut AppState, i| s.selected.push(i))
                    .anchor(&a)
                    .on_open_change(|s: &mut AppState, open| s.opens.push(open)),
            )
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));

        root.event(&mut state, &pointer(PointerPhase::Down, 380.0, 580.0));
        assert_eq!(state.opens, vec![false], "the light dismiss");
    }

    // ---- Typeface: the rows follow the live theme -------------------------

    /// Every text row the menu builds: both group-label treatments, an item
    /// with a shortcut, and a disabled item.
    #[cfg(feature = "bundled-fonts")]
    fn menu_rows(_: &mut ()) -> frust::FlexView<()> {
        let insets = EdgeInsets::symmetric(ROW_PAD_X, ROW_PAD_Y);
        frust::column()
            .child(label_view("My Account", insets, false))
            .child(label_view("Theme", insets, true))
            .child(item_view(
                &dropdown_menu_item("Profile").shortcut("Ctrl+P"),
                insets,
                false,
            ))
            .child(item_view(
                &dropdown_menu_item("Billing").disabled(true),
                insets,
                true,
            ))
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_rows_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face("the menu's rows", menu_rows);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_rows_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap("the menu's rows", menu_rows);
    }
}
