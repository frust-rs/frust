// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/menus/components/` — `m3e_menu_content.dart` (the node walk
// and section labels), `m3e_menu_item.dart` (a row's box, ink and slots),
// `m3e_menu_divider.dart`, `m3e_menu_node_builders.dart` (the per-kind glyph
// and activation rules) and `m3e_menu_popup.dart`'s `_buildSurfaces` (the
// elevated-surface stack), retrieved 2026-08-19.
// Porting decisions: the reference builds a Flutter widget subtree per row;
// this port keeps rows as **data** and paints them, so one widget owns the
// hover latch, the press machine and the submenu chain (the shape
// `frust_shadcn`'s own menu list established, and what lets a row's ink be
// per-state — no themed text role can express it). The reference's
// `M3EMenuItemShape` (leading/middle/trailing corner shapes) is deliberately
// **not** ported: all four of its variants return the same `itemBorderRadius`
// (`m3e_menu_theme.dart:196-206`) and the popup passes `applyGroupShapes:
// false` (`m3e_menu_popup.dart:388`), so it resolves to one radius here.
// Placement and dismissal are the anchored host's, never this panel's.

//! The menu panel: the reusable item-list surface every Material menu family
//! paints — the anchored menu here, and the dropdown/button-group popups built
//! on top of it.
//!
//! # A stack of surfaces, not one card
//!
//! [`items::partition_surfaces`](super::items::partition_surfaces) splits the
//! node tree into elevated containers — one per
//! [`MenuGroup`](super::MenuGroup), one for each run of consecutive non-group
//! nodes — and the panel stacks them with
//! [`MENU_SECTION_GAP`](super::MENU_SECTION_GAP) between. Each surface fills,
//! rounds and lifts itself (`elevation` level 2, the M3 menu rung).
//!
//! # Rows are data
//!
//! A row is a [`Row`] record plus its shaped text runs and parsed icon paths,
//! not a child widget: the ink is per-kind *and* per-state (disabled at 38%,
//! destructive `error`, selected `onTertiaryContainer`, …), which no themed
//! text role can express, so the panel shapes its own runs and re-brushes them
//! at paint the way [`mod@crate::button`]'s label runs do. The **only** child
//! pods are the nested submenu panels, which have to be pods so events can be
//! routed into their own coordinate space.
//!
//! # Submenus
//!
//! The reference opens a submenu **on tap** and on nothing else —
//! `M3EMenuNodeBuilders.submenu` wires `onOpenSubmenu` to `M3EMenuItem.onTap`
//! (`m3e_menu_node_builders.dart:135`), with no hover trigger anywhere in the
//! family. That rule is ported as-is; hover-open is layered *over* it as a
//! desktop-pointer affordance (a touch stream carries no `Move`, so touch is
//! tap-open and a mouse is hover-open, with tap working on both). Moving onto
//! any other row closes whatever was open, and moving onto the open submenu
//! itself keeps it — the pointer has to be able to travel there.
//!
//! Two behaviors this composition does not reproduce, the same pair the
//! sibling shadcn port records:
//!
//! * **No collision handling for a submenu.** The anchored host places exactly
//!   one rect — the top-level panel — so a nested panel has no area to flip or
//!   clamp against and always opens to the trailing side of its row (the
//!   reference's `preferRight` default, `m3e_menu_placer.dart:212`).
//! * **No safe-triangle grace period.** The first move onto another row closes
//!   the submenu; there is no diagonal-travel allowance.
//!
//! # Dismissal is the host's
//!
//! The panel never dismisses itself: light dismiss and Escape belong to
//! [`crate::overlay::anchored`], which owns the enter/exit ramp too. What the
//! panel does own is the **chain**: handing it `open == false` closes every
//! open submenu on the spot, so a light dismiss takes the whole cascade with it
//! (the reference's `_dismiss` → `_removeSubmenu`, `m3e_menu_popup.dart:198`).

use frust::authoring::{
    Action, Toggled,
    text::{TextContext, TextLayout, TextOverflow, TextStyle},
};
use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CursorIcon, ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, ScrollDelta, SemanticsCtx, Size,
    TypedArgCallback, View, Widget, any, build_child, erase_callback_arg, rebuild_children,
    route_event_single, teardown_child, visit_children,
};
use frust::input::WHEEL_LINE_PX;
use frust::{IconData, Theme};

use super::items::{MenuAction, MenuNode, MenuSelection, partition_surfaces};
use super::{
    MENU_DIVIDER_THICKNESS, MENU_DIVIDER_V_PADDING, MENU_GROUP_LABEL_H_PADDING,
    MENU_GROUP_LABEL_V_PADDING, MENU_ICON_GAP, MENU_ICON_SIZE, MENU_ITEM_GAP, MENU_MAX_HEIGHT,
    MENU_MAX_WIDTH, MENU_MIN_WIDTH, MENU_ROW_H_PADDING, MENU_ROW_MIN_HEIGHT, MENU_SECTION_GAP,
    MENU_SUBMENU_GAP, MENU_SUPPORTING_V_PADDING, MENU_SURFACE_H_PADDING, MENU_SURFACE_V_PADDING,
    MenuColorStyle, MenuColors, SELECTED_CHECK_SCALE, container_radius, item_radius,
};
use crate::icons;
use crate::interaction::InteractionState;
use crate::overlay::{OverlayElevation, shadow};
use crate::press::presses;

/// The ink every run is *shaped* with; never painted — a run is re-brushed with
/// its resolved per-kind/per-state color at paint time, and holding the shaping
/// color constant keeps the shape cache from missing on a recolor (the contract
/// [`mod@crate::button`]'s own runs document).
const SHAPING_INK: Color = Color::BLACK;

/// A shaped, cached text run, re-brushed at paint. Mirrors
/// [`mod@crate::button`]'s `LabelRun` — the same lazily-shaped,
/// measured-against-`(style, max_width)` cache, minus the overflow-observer
/// seam a button needs and a menu row does not.
struct Run {
    content: String,
    layout: Option<TextLayout>,
    /// The `(style, max_width)` the cached `layout` was shaped for.
    shaped_for: Option<(TextStyle, Option<f64>)>,
    /// The `(style, width)` of the last natural (unconstrained) measurement.
    natural: Option<(TextStyle, f64)>,
}

impl Run {
    /// A run holding `content`, unshaped until the first measurement.
    fn new(content: impl Into<String>) -> Self {
        Run {
            content: content.into(),
            layout: None,
            shaped_for: None,
            natural: None,
        }
    }

    /// The run's width with nothing constraining it.
    fn natural_width(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> f64 {
        if let Some((cached, width)) = &self.natural
            && cached == style
        {
            return *width;
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let width = text_ctx.layout(&self.content, style, None).size().width;
        self.natural = Some((style.clone(), width));
        width
    }

    /// Shape (or reuse) the run in `style`, fitted to `max_width`, and return
    /// its measured size.
    ///
    /// A fitted run is a **single ellipsized line** — the reference's rows are
    /// `maxLines: 1, overflow: ellipsis` throughout (`m3e_menu_item.dart:135`).
    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: f64) -> Size {
        let key = (style.clone(), Some(max_width));
        if let Some(cached) = &self.layout
            && self.shaped_for.as_ref() == Some(&key)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout_bounded(
            &self.content,
            style,
            Some(max_width.max(0.0) as f32),
            Some(1),
            TextOverflow::Ellipsis,
        );
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(key);
        size
    }

    /// The shaped size, or zero for a run no layout pass has reached yet.
    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding the [`SHAPING_INK`] it
    /// was shaped with. A never-shaped run paints nothing.
    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

/// A row's icon slot, parsed once at build into its design-box path.
struct Glyph {
    path: BezPath,
    design: f64,
    /// The side length to paint at — [`MENU_ICON_SIZE`], or the reference's
    /// `iconSize * 0.9` for the implicit selected check.
    extent: f64,
}

impl Glyph {
    /// Parse `icon`'s path once, to be painted at `extent`.
    fn new(icon: frust::IconSource, extent: f64) -> Self {
        let (path, design) = IconData::from(icon).resolve();
        Glyph {
            path,
            design,
            extent,
        }
    }

    /// Paint the glyph with its top-left at `origin`, in `color`.
    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let scale = if self.design > 0.0 {
            self.extent / self.design
        } else {
            1.0
        };
        let scaled = Affine::scale(scale) * self.path.clone();
        scene.fill_path(origin, &scaled, &Brush::Solid(color));
    }
}

/// What a rendered row is, plus the payload its activation reports.
enum RowKind {
    /// A plain action row.
    Entry,
    /// A radio-style row carrying the value it would request.
    Selectable(String),
    /// A checkbox-style row carrying its *current* checked state.
    Toggleable(bool),
    /// A hairline rule.
    Divider,
    /// A row that opens a nested panel.
    Submenu,
}

impl RowKind {
    /// Whether this row is an *item* — everything but a divider. A disabled
    /// item is still an item; it just refuses the press.
    fn is_item(&self) -> bool {
        !matches!(self, RowKind::Divider)
    }
}

/// One rendered row: its kind, its resolved state, its shaped content, and the
/// vertical band it occupies in the panel's own content space.
struct Row {
    kind: RowKind,
    /// The depth-first index reported through `on_select`.
    index: usize,
    /// Which surface this row belongs to.
    surface: usize,
    enabled: bool,
    destructive: bool,
    /// Resolved selected state — a selectable's own flag OR-ed with a value
    /// match, or a toggleable's `checked`.
    selected: bool,
    label: Run,
    supporting: Option<Run>,
    shortcut: Option<Run>,
    leading: Option<Glyph>,
    trailing: Option<Glyph>,
    /// This row's submenu pod, when it has one.
    sub: Option<usize>,
    /// Top of the row's band, in content space.
    top: f64,
    /// The band's full height (the item box plus [`MENU_ITEM_GAP`]).
    height: f64,
}

/// One elevated menu container: its optional section label and the span of
/// [`Row`]s it holds.
struct Surface {
    label: Option<Run>,
    /// Top of the surface, in content space.
    top: f64,
    height: f64,
}

/// The rows and submenu views a node tree renders as — computed identically by
/// [`MenuPanelView::build`] and [`MenuPanelView::rebuild`], so the pod list and
/// the row list can never drift.
struct Plan {
    surfaces: Vec<PlannedSurface>,
    rows: Vec<PlannedRow>,
}

/// A planned surface: its section label and nothing else (geometry is layout's).
struct PlannedSurface {
    label: Option<String>,
}

/// A planned row: everything needed to build its [`Row`], plus the submenu
/// children it opens.
struct PlannedRow {
    node: MenuNode,
    index: usize,
    surface: usize,
    /// `(base index, children)` for a submenu row.
    submenu: Option<(usize, Vec<MenuNode>)>,
}

/// Walk `nodes` into surfaces and rows, numbering from `base`.
fn plan_tree(nodes: &[MenuNode], base: usize) -> Plan {
    let mut surfaces = Vec::new();
    let mut rows = Vec::new();
    let mut index = base;
    for group in partition_surfaces(nodes) {
        let surface = surfaces.len();
        surfaces.push(PlannedSurface {
            label: group.section_label().map(str::to_string),
        });
        for node in group.children() {
            let submenu = match node {
                MenuNode::Submenu(item) => Some((index + 1, item.children().to_vec())),
                _ => None,
            };
            index += node.span();
            // A nested group inside a group is not a surface of its own — the
            // reference partitions only the top level — so its children join
            // this surface as plain rows.
            match node {
                MenuNode::Group(inner) => {
                    let nested = plan_tree(inner.children(), index - node.span());
                    for mut row in nested.rows {
                        row.surface = surface;
                        rows.push(row);
                    }
                }
                _ => rows.push(PlannedRow {
                    node: node.clone(),
                    index: index - node.span(),
                    surface,
                    submenu,
                }),
            }
        }
    }
    Plan { surfaces, rows }
}

/// A declarative menu panel. See [`menu_panel`].
pub struct MenuPanelView<State: 'static> {
    nodes: Vec<MenuNode>,
    style: MenuColorStyle,
    selected: Option<String>,
    open: bool,
    base: usize,
    on_select: TypedArgCallback<State, MenuSelection>,
}

/// Build a menu panel over `nodes` — the item-list surface, without any
/// anchoring or dismissal of its own.
///
/// This is what [`menu`](super::menu) mounts inside
/// [`crate::overlay::anchored`], and what a dropdown or a button-group popup
/// mounts inside its own host. An app building a plain anchored menu wants
/// [`menu`](super::menu) instead.
///
/// `on_select(state, selection)` reports an activation; see [`MenuSelection`]
/// for what a row requests and the controlled-component contract behind it.
pub fn menu_panel<State: 'static, F: Fn(&mut State, MenuSelection) + 'static>(
    nodes: Vec<MenuNode>,
    on_select: F,
) -> MenuPanelView<State> {
    MenuPanelView {
        nodes,
        style: MenuColorStyle::default(),
        selected: None,
        open: true,
        base: 0,
        on_select: std::rc::Rc::new(on_select),
    }
}

impl<State: 'static> MenuPanelView<State> {
    /// Pick the standard or vibrant color resolution.
    pub fn color_style(mut self, style: MenuColorStyle) -> Self {
        self.style = style;
        self
    }

    /// Hand the panel the app-confirmed selected value its
    /// [`MenuSelectable`](super::MenuSelectable) rows check against.
    pub fn selected(mut self, value: Option<String>) -> Self {
        self.selected = value;
        self
    }

    /// Tell the panel whether its host is open. `false` closes every open
    /// submenu on the spot; the default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Number this panel's rows from `base` rather than `0` — how a submenu
    /// keeps the whole tree's depth-first numbering.
    pub fn base(mut self, base: usize) -> Self {
        self.base = base;
        self
    }

    /// The submenu panel views this tree opens, in row order.
    fn sub_views(&self, plan: &Plan) -> Vec<AnyView<State>> {
        plan.rows
            .iter()
            .filter_map(|row| row.submenu.as_ref())
            .map(|(base, children)| {
                any(MenuPanelView {
                    nodes: children.clone(),
                    style: self.style,
                    selected: self.selected.clone(),
                    open: self.open,
                    base: *base,
                    on_select: self.on_select.clone(),
                })
            })
            .collect()
    }

    /// Build the retained rows for `plan`, resolving each kind's implicit glyph
    /// and selected state (`m3e_menu_node_builders.dart:46-147`).
    fn build_rows(&self, plan: &Plan) -> Vec<Row> {
        let mut pod = 0usize;
        let mut rows = Vec::with_capacity(plan.rows.len());
        for planned in &plan.rows {
            let sub = planned.submenu.as_ref().map(|_| {
                let index = pod;
                pod += 1;
                index
            });
            rows.push(self.build_row(planned, sub));
        }
        rows
    }

    /// One row's retained record.
    fn build_row(&self, planned: &PlannedRow, sub: Option<usize>) -> Row {
        let icon = |slot: &Option<super::MenuIcon>| slot.map(|i| Glyph::new(i.0, MENU_ICON_SIZE));
        let (kind, label, supporting, shortcut, leading, trailing, enabled, destructive, selected) =
            match &planned.node {
                MenuNode::Entry(e) => (
                    RowKind::Entry,
                    Run::new(e.label.clone()),
                    e.supporting.clone().map(Run::new),
                    e.shortcut.clone().map(Run::new),
                    icon(&e.leading),
                    icon(&e.trailing),
                    e.enabled,
                    e.destructive,
                    false,
                ),
                MenuNode::Selectable(e) => {
                    let selected =
                        e.selected || self.selected.as_deref().is_some_and(|v| v == e.value);
                    // The reference falls back to a check glyph at 90% icon size
                    // only while selected (`m3e_menu_node_builders.dart:55`).
                    let leading = icon(&e.leading).or_else(|| {
                        selected.then(|| {
                            Glyph::new(icons::CHECK_ROUNDED, MENU_ICON_SIZE * SELECTED_CHECK_SCALE)
                        })
                    });
                    (
                        RowKind::Selectable(e.value.clone()),
                        Run::new(e.label.clone()),
                        e.supporting.clone().map(Run::new),
                        e.shortcut.clone().map(Run::new),
                        leading,
                        icon(&e.trailing),
                        e.enabled,
                        false,
                        selected,
                    )
                }
                MenuNode::Toggleable(e) => {
                    // The reference always draws a box, checked or blank
                    // (`m3e_menu_node_builders.dart:88`).
                    let leading = icon(&e.leading).or_else(|| {
                        let source = if e.checked {
                            icons::CHECK_BOX_ROUNDED
                        } else {
                            icons::CHECK_BOX_OUTLINE_BLANK_ROUNDED
                        };
                        Some(Glyph::new(source, MENU_ICON_SIZE))
                    });
                    (
                        RowKind::Toggleable(e.checked),
                        Run::new(e.label.clone()),
                        e.supporting.clone().map(Run::new),
                        e.shortcut.clone().map(Run::new),
                        leading,
                        icon(&e.trailing),
                        e.enabled,
                        false,
                        e.checked,
                    )
                }
                MenuNode::Submenu(e) => (
                    RowKind::Submenu,
                    Run::new(e.label.clone()),
                    None,
                    None,
                    icon(&e.leading),
                    // The chevron is fixed, never a caller's slot
                    // (`m3e_menu_node_builders.dart:128`).
                    Some(Glyph::new(icons::ARROW_RIGHT_ROUNDED, MENU_ICON_SIZE)),
                    e.enabled,
                    false,
                    false,
                ),
                // A group is never planned as a row of its own — `plan_tree`
                // turns it into a surface and flattens its children — so it
                // shares the divider's empty record rather than needing an
                // unreachable arm.
                MenuNode::Divider | MenuNode::Group(_) => (
                    RowKind::Divider,
                    Run::new(String::new()),
                    None,
                    None,
                    None,
                    None,
                    false,
                    false,
                    false,
                ),
            };
        Row {
            kind,
            index: planned.index,
            surface: planned.surface,
            enabled,
            destructive,
            selected,
            label,
            supporting,
            shortcut,
            leading,
            trailing,
            sub,
            top: 0.0,
            height: 0.0,
        }
    }

    /// The retained surfaces for `plan`.
    fn build_surfaces(&self, plan: &Plan) -> Vec<Surface> {
        plan.surfaces
            .iter()
            .map(|planned| Surface {
                label: planned.label.clone().map(Run::new),
                top: 0.0,
                height: 0.0,
            })
            .collect()
    }
}

/// The retained widget for a [`MenuPanelView`].
pub struct MenuPanelWidget {
    surfaces: Vec<Surface>,
    rows: Vec<Row>,
    /// The submenu panels, in row order — the only child pods this widget has.
    pods: Vec<ChildPod>,
    style: MenuColorStyle,
    open: bool,
    on_select: ErasedArgCallback<MenuSelection>,
    /// The latched hovered row (the hover-flag half of the catalog's three-part
    /// hover seam; `paint` self-corrects it from `PaintCtx::is_hovered`).
    hovered: Option<usize>,
    /// The row a live press is armed on.
    pressed: Option<usize>,
    /// Whether that press is currently *inside* its own row — a drag off the
    /// row drops the pressed wash without ending the gesture, so a drag back
    /// re-arms it (the `list_item` press machine's contract).
    press_inside: bool,
    captured: bool,
    /// The row whose submenu is open.
    open_sub: Option<usize>,
    scroll: f64,
    viewport: f64,
    content_height: f64,
    width: f64,
}

impl MenuPanelWidget {
    /// The panel's resolved width, in logical px.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// The row whose submenu is open, if any.
    pub fn open_submenu(&self) -> Option<usize> {
        self.open_sub
    }

    /// The panel's scroll offset, in logical px.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// The `(top, height)` band row `row` occupies in content space.
    #[cfg(test)]
    fn band(&self, row: usize) -> (f64, f64) {
        let row = &self.rows[row];
        (row.top, row.height)
    }

    /// Close this panel's own open submenu.
    ///
    /// The *cascade* closes with it without any recursion here: a nested panel
    /// is built from this one's `open` flag, so the same rebuild that clears
    /// this hands every descendant `open == false` and each clears its own (the
    /// reference's `_dismiss` → `_removeSubmenu`, one level per panel).
    fn close_chain(&mut self) {
        self.open_sub = None;
    }

    /// Open `row`'s submenu, if it has one and is enabled.
    fn open_submenu_at(&mut self, row: usize) -> bool {
        let opens = self
            .rows
            .get(row)
            .is_some_and(|r| r.enabled && r.sub.is_some());
        if opens {
            self.open_sub = Some(row);
        }
        opens
    }

    /// The pod holding the open submenu's panel, if one is open.
    fn open_sub_pod(&self) -> Option<usize> {
        let row = self.open_sub?;
        self.rows.get(row)?.sub
    }

    /// The open submenu's placed rect, in this panel's own space.
    fn open_sub_rect(&self) -> Option<Rect> {
        let pod = self.open_sub_pod()?;
        let pod = self.pods.get(pod)?;
        Some(Rect::from_origin_size(pod.origin(), pod.size()))
    }

    /// Keep the scroll offset inside the scrollable range.
    fn clamp_scroll(&mut self) {
        let max = (self.content_height - self.viewport).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// Place the submenu pods for the current scroll offset.
    ///
    /// Called from `layout` and again from the scroll arm: an `EventCtx` can
    /// request a redraw but not a relayout, so the pods are repositioned in
    /// place rather than waiting for a pass a mere scroll never triggers.
    fn apply_scroll(&mut self) {
        for row in self.rows.iter() {
            let Some(pod) = row.sub.and_then(|i| self.pods.get_mut(i)) else {
                continue;
            };
            // Trailing side of the panel, the nested surface's *first row*
            // lined up with the triggering row (its own vertical padding is
            // what the offset cancels) — the reference's side placement,
            // top-aligned with the anchor row (`m3e_menu_placer.dart:249`).
            pod.set_origin(Point::new(
                self.width + MENU_SUBMENU_GAP,
                row.top - self.scroll - MENU_SURFACE_V_PADDING,
            ));
        }
    }

    /// The row under panel-local `pos`, if any.
    fn row_at(&self, pos: Point) -> Option<usize> {
        if pos.x < 0.0 || pos.x >= self.width || pos.y < 0.0 || pos.y >= self.viewport {
            return None;
        }
        let y = pos.y + self.scroll;
        self.rows
            .iter()
            .position(|row| y >= row.top && y < row.top + row.height)
    }

    /// Whether row `i` can be pressed at all.
    fn activatable(&self, i: usize) -> bool {
        self.rows
            .get(i)
            .is_some_and(|row| row.enabled && row.kind.is_item())
    }

    /// Report row `i`'s activation, or open its submenu.
    fn activate(&mut self, ctx: &mut EventCtx, i: usize) {
        let Some(row) = self.rows.get(i) else {
            return;
        };
        let action = match &row.kind {
            RowKind::Entry => MenuAction::Press,
            RowKind::Selectable(value) => MenuAction::Select(value.clone()),
            // Controlled: the row reports the state it *asks* for and leaves
            // its own `checked` alone until the caller feeds one back down.
            RowKind::Toggleable(checked) => MenuAction::Toggle(!checked),
            RowKind::Submenu => {
                self.open_submenu_at(i);
                ctx.request_redraw();
                return;
            }
            RowKind::Divider => return,
        };
        let selection = MenuSelection {
            index: row.index,
            label: row.label.content.clone(),
            action,
        };
        (self.on_select)(ctx, selection);
    }

    /// The `Widget::event` pointer arm: hover latching (with submenu
    /// open/close), press arming, and release-to-activate.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        match p.phase {
            PointerPhase::Move if !self.captured => {
                // A pointer travelling over the open submenu must not close it.
                if self.open_sub_rect().is_some_and(|r| r.contains(p.position)) {
                    return EventResult::Ignored;
                }
                let row = self.row_at(p.position);
                // Claimed from the uncaptured move arm on every qualifying
                // pass, per the catalog's hover seam.
                if row.is_some() {
                    ctx.claim_hover();
                }
                let hovered = row.filter(|i| self.activatable(*i));
                if hovered.is_some() {
                    ctx.set_cursor(CursorIcon::Pointer);
                }
                if self.hovered != hovered {
                    self.hovered = hovered;
                    match hovered {
                        // Hover-open, layered over the reference's tap-open
                        // rule (see the module docs).
                        Some(i) => {
                            if !self.open_submenu_at(i) {
                                self.open_sub = None;
                            }
                        }
                        // A divider, a disabled row or the panel's own padding
                        // is "anything else": it closes whatever was open
                        // rather than leaving a stale panel hanging.
                        None => self.open_sub = None,
                    }
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Move => {
                // Captured: the pressed wash follows the pointer in and out of
                // the armed row without ending the gesture.
                let inside = self.pressed.is_some() && self.row_at(p.position) == self.pressed;
                if self.press_inside != inside {
                    self.press_inside = inside;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(row) = self.row_at(p.position).filter(|i| self.activatable(*i)) else {
                    // The panel's own background: left for the host, which
                    // swallows a press inside its content without dismissing.
                    return EventResult::Ignored;
                };
                self.pressed = Some(row);
                self.press_inside = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let released = self.row_at(p.position);
                let armed = self.pressed;
                self.pressed = None;
                self.press_inside = false;
                self.captured = false;
                // Fire on up-inside only, the catalog's press contract.
                if let Some(row) = armed.filter(|row| released == Some(*row)) {
                    self.activate(ctx, row);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            // A `Cancel` arm touches no app state — only the flags the next
            // move re-establishes.
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = None;
                self.press_inside = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    /// The `Widget::event` scroll arm: the same `offset + dy` wheel convention
    /// every scrollable in this catalog shares, clamped to the content range.
    fn handle_scroll(
        &mut self,
        ctx: &mut EventCtx,
        position: Point,
        delta: &ScrollDelta,
    ) -> EventResult {
        if position.y < 0.0 || position.y >= self.viewport {
            return EventResult::Ignored;
        }
        let dy = match delta {
            ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
            ScrollDelta::Pixels(_, y) => *y,
        };
        let before = self.scroll;
        self.scroll += dy;
        self.clamp_scroll();
        if (self.scroll - before).abs() <= f64::EPSILON {
            return EventResult::Ignored;
        }
        self.apply_scroll();
        ctx.request_redraw();
        EventResult::Handled
    }
}

impl<State: 'static> View<State> for MenuPanelView<State> {
    type Element = MenuPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MenuPanelWidget {
        let plan = plan_tree(&self.nodes, self.base);
        let pods = self
            .sub_views(&plan)
            .iter()
            .map(|view| build_child(view, ctx))
            .collect();
        MenuPanelWidget {
            surfaces: self.build_surfaces(&plan),
            rows: self.build_rows(&plan),
            pods,
            style: self.style,
            open: self.open,
            on_select: erase_callback_arg(&self.on_select),
            hovered: None,
            pressed: None,
            press_inside: false,
            captured: false,
            open_sub: None,
            scroll: 0.0,
            viewport: 0.0,
            content_height: 0.0,
            width: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MenuPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let plan = plan_tree(&self.nodes, self.base);
        let mut flags = rebuild_children(
            &prev.sub_views(&plan_tree(&prev.nodes, prev.base)),
            &self.sub_views(&plan),
            &mut element.pods,
            ctx,
            |view: &AnyView<State>| view,
            |_| None,
        );
        // Rows carry shaped runs and parsed icon paths, so they are rebuilt
        // only on a real content change — a rebuild that re-supplies the same
        // tree keeps every cached layout.
        if prev.nodes != self.nodes
            || prev.selected != self.selected
            || prev.style != self.style
            || prev.base != self.base
        {
            element.surfaces = self.build_surfaces(&plan);
            element.rows = self.build_rows(&plan);
            element.style = self.style;
            element.hovered = None;
            element.pressed = None;
            element.press_inside = false;
            element.captured = false;
            element.close_chain();
            element.scroll = 0.0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            if !self.open {
                // A dismissal takes the whole cascade with it.
                element.close_chain();
                flags |= ChangeFlags::PAINT;
            }
        }
        element.on_select = erase_callback_arg(&self.on_select);
        flags
    }

    fn teardown(&self, element: &mut MenuPanelWidget, ctx: &mut BuildCtx<'_>) {
        let plan = plan_tree(&self.nodes, self.base);
        for (view, pod) in self.sub_views(&plan).iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// Everything a paint pass resolves from the theme once, then hands down to
/// each surface and row — so a single pass reads the theme exactly once and no
/// two rows can disagree about a role or a radius.
struct Chrome {
    colors: MenuColors,
    /// The level-2 elevation rung's `(blur, y_offset, color)`, if any.
    shadow: Option<(f64, f64, Color)>,
    container_radius: f64,
    item_radius: f64,
}

/// The type styles a panel's four text roles resolve to
/// (`m3e_menu_theme.dart:329-389`): `labelLarge` for a row label and a section
/// label, `labelMedium` for supporting and shortcut text.
struct Styles {
    label: TextStyle,
    supporting: TextStyle,
}

impl Styles {
    /// Resolve both roles from `theme`, or from the M3 token literals when no
    /// theme is threaded into the pass.
    fn resolve(theme: Option<&Theme>) -> Self {
        let (mut label, mut supporting) = match theme {
            Some(theme) => (
                theme.type_scale.label_large.clone(),
                theme.type_scale.label_medium.clone(),
            ),
            None => {
                let mut large = TextStyle::new(14.0, SHAPING_INK);
                large.letter_spacing = 0.1;
                let mut medium = TextStyle::new(12.0, SHAPING_INK);
                medium.letter_spacing = 0.5;
                (large, medium)
            }
        };
        label.color = SHAPING_INK;
        supporting.color = SHAPING_INK;
        Styles { label, supporting }
    }
}

impl Widget for MenuPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let styles = Styles::resolve(Theme::from_layout_ctx(ctx));

        // Pass 1 — natural widths, so the panel can size itself the way the
        // reference's placer does (min/max clamped, never anchor-driven here:
        // the host places a panel by the size this returns).
        let mut natural: f64 = 0.0;
        for i in 0..self.rows.len() {
            natural = natural.max(self.row_natural_width(ctx, i, &styles));
        }
        for surface in self.surfaces.iter_mut() {
            if let Some(label) = surface.label.as_mut() {
                let width = label.natural_width(ctx, &styles.label);
                natural = natural.max(width + 2.0 * MENU_GROUP_LABEL_H_PADDING);
            }
        }
        let cap = if bc.max().width.is_finite() {
            bc.max().width.min(MENU_MAX_WIDTH)
        } else {
            MENU_MAX_WIDTH
        };
        self.width = (natural + 2.0 * MENU_SURFACE_H_PADDING)
            .max(MENU_MIN_WIDTH.min(cap))
            .min(cap);

        // Pass 2 — shape every run against the resolved width and stack the
        // surfaces.
        let mut y = 0.0;
        for surface in 0..self.surfaces.len() {
            if surface > 0 {
                y += MENU_SECTION_GAP;
            }
            let top = y;
            y += MENU_SURFACE_V_PADDING;
            if let Some(label) = self.surfaces[surface].label.as_mut() {
                let width = (self.width - 2.0 * MENU_GROUP_LABEL_H_PADDING).max(0.0);
                let height = label.shape(ctx, &styles.label, width).height;
                y += height + 2.0 * MENU_GROUP_LABEL_V_PADDING;
            }
            for i in 0..self.rows.len() {
                if self.rows[i].surface != surface {
                    continue;
                }
                let height = self.row_layout(ctx, i, &styles);
                self.rows[i].top = y;
                self.rows[i].height = height;
                y += height;
            }
            y += MENU_SURFACE_V_PADDING;
            self.surfaces[surface].top = top;
            self.surfaces[surface].height = y - top;
        }
        self.content_height = y;

        let height_cap = if bc.max().height.is_finite() {
            bc.max().height.min(MENU_MAX_HEIGHT)
        } else {
            MENU_MAX_HEIGHT
        };
        self.viewport = self.content_height.min(height_cap);
        self.clamp_scroll();

        // The submenu panels size themselves, outside this panel's own box.
        let loose = BoxConstraints::loose(Size::new(MENU_MAX_WIDTH, MENU_MAX_HEIGHT));
        for pod in self.pods.iter_mut() {
            pod.layout_child(ctx, &loose);
        }
        self.apply_scroll();
        bc.constrain(Size::new(self.width, self.viewport))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: a pointer that left this panel sends it
        // nothing, so the latched row is cleared from path membership.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = Chrome {
            colors: MenuColors::resolve(theme, self.style),
            // The M3 menu rung, which the overlay seam already names.
            shadow: shadow(theme, OverlayElevation::Level2),
            container_radius: container_radius(theme),
            item_radius: item_radius(theme),
        };
        let origin = ctx.origin();

        // Only a panel that actually scrolls clips: a clip tight to the box
        // would cut the surfaces' own elevation shadows out of it, which is the
        // trap the reference's shadow padding avoids
        // (`m3e_menu_popup.dart:352`).
        let clipped = self.content_height > self.viewport + f64::EPSILON;
        if clipped {
            scene.push_clip(origin, Size::new(self.width, self.viewport));
        }

        for surface in 0..self.surfaces.len() {
            self.paint_surface(surface, origin, &chrome, scene);
        }

        if clipped {
            scene.pop_clip();
        }

        // The open submenu paints last and outside the clip, so it sits over
        // whatever is beside it.
        if let Some(pod) = self.open_sub_pod()
            && let Some(pod) = self.pods.get_mut(pod)
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
        // The open submenu owns the pass first: it is the innermost panel, and
        // anything it does not take falls back to this one.
        if let Some(pod) = self.open_sub_pod()
            && route_event_single(&mut self.pods[pod], ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        match event {
            InputEvent::Scroll { position, delta } => self.handle_scroll(ctx, *position, delta),
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            // Keys are the host's: Escape dismisses the whole chain there,
            // rather than this panel unwinding it one level at a time.
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let rows = &self.rows;
        let pods = &self.pods;
        let open_sub = self.open_sub_pod();
        ctx.push_container(
            Role::Menu,
            |_node| {},
            |ctx| {
                for row in rows.iter() {
                    let role = match row.kind {
                        RowKind::Divider => continue,
                        RowKind::Selectable(_) => Role::MenuItemRadio,
                        RowKind::Toggleable(_) => Role::MenuItemCheckBox,
                        RowKind::Entry | RowKind::Submenu => Role::MenuItem,
                    };
                    // Per-row bounds would need a `ChildPod` per row to descend
                    // through; rows are painted internally, so every node below
                    // shares the whole panel's bounds — the same v1 limitation
                    // `frust_glyph`'s own menu accepts.
                    ctx.push_node(role, |node| {
                        node.set_label(row.label.content.as_str());
                        if row.enabled {
                            node.add_action(Action::Click);
                        }
                        if matches!(row.kind, RowKind::Selectable(_) | RowKind::Toggleable(_)) {
                            node.set_toggled(if row.selected {
                                Toggled::True
                            } else {
                                Toggled::False
                            });
                        }
                    });
                }
                // Only the open submenu is reachable by input, so only it is
                // published — the input-parity carve-out for a container that
                // gates its children (`docs/CODE_STANDARDS.md`).
                if let Some(pod) = open_sub.and_then(|i| pods.get(i)) {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(pods);
}

impl MenuPanelWidget {
    /// The natural (unconstrained) width row `i` wants, including its own
    /// horizontal padding and every slot's advance.
    fn row_natural_width(&mut self, ctx: &mut LayoutCtx, i: usize, styles: &Styles) -> f64 {
        if !self.rows[i].kind.is_item() {
            return 0.0;
        }
        let mut text = {
            let row = &mut self.rows[i];
            row.label.natural_width(ctx, &styles.label)
        };
        if let Some(supporting) = self.rows[i].supporting.as_mut() {
            text = text.max(supporting.natural_width(ctx, &styles.supporting));
        }
        let mut width = 2.0 * MENU_ROW_H_PADDING + text;
        if self.rows[i].leading.is_some() {
            width += MENU_ICON_SIZE + MENU_ICON_GAP;
        }
        if let Some(shortcut) = self.rows[i].shortcut.as_mut() {
            width += MENU_ICON_GAP + shortcut.natural_width(ctx, &styles.supporting);
        }
        if self.rows[i].trailing.is_some() {
            width += MENU_ICON_GAP + MENU_ICON_SIZE;
        }
        width
    }

    /// The width available to row `i`'s label column, once its slots are paid
    /// for.
    fn row_text_width(&self, i: usize) -> f64 {
        let row = &self.rows[i];
        let mut width = self.width - 2.0 * MENU_SURFACE_H_PADDING - 2.0 * MENU_ROW_H_PADDING;
        if row.leading.is_some() {
            width -= MENU_ICON_SIZE + MENU_ICON_GAP;
        }
        if row.trailing.is_some() {
            width -= MENU_ICON_GAP + MENU_ICON_SIZE;
        }
        if let Some(shortcut) = &row.shortcut {
            width -= MENU_ICON_GAP + shortcut.size().width;
        }
        width.max(0.0)
    }

    /// Shape row `i` and return the full height of its band.
    fn row_layout(&mut self, ctx: &mut LayoutCtx, i: usize, styles: &Styles) -> f64 {
        if !self.rows[i].kind.is_item() {
            return 2.0 * MENU_DIVIDER_V_PADDING + MENU_DIVIDER_THICKNESS;
        }
        // The shortcut is shaped first: it is what the label column's own width
        // is measured against.
        if let Some(shortcut) = self.rows[i].shortcut.as_mut() {
            let natural = shortcut.natural_width(ctx, &styles.supporting);
            shortcut.shape(ctx, &styles.supporting, natural);
        }
        let text_width = self.row_text_width(i);
        let mut content = {
            let label = &mut self.rows[i].label;
            label.shape(ctx, &styles.label, text_width).height
        };
        let has_supporting = self.rows[i].supporting.is_some();
        if let Some(supporting) = self.rows[i].supporting.as_mut() {
            content += supporting.shape(ctx, &styles.supporting, text_width).height;
        }
        let padding = if has_supporting {
            2.0 * MENU_SUPPORTING_V_PADDING
        } else {
            0.0
        };
        MENU_ITEM_GAP + (content + padding).max(MENU_ROW_MIN_HEIGHT)
    }

    /// Paint surface `s`: its container, its section label, and its rows.
    fn paint_surface(&self, s: usize, origin: Point, chrome: &Chrome, scene: &mut dyn PaintScene) {
        let top = origin.y + self.surfaces[s].top - self.scroll;
        let box_origin = Point::new(origin.x, top);
        let box_size = Size::new(self.width, self.surfaces[s].height);
        if let Some((blur, y_offset, color)) = chrome.shadow {
            scene.draw_shadow(
                Point::new(box_origin.x, box_origin.y + y_offset),
                box_size,
                chrome.container_radius,
                blur,
                color,
            );
        }
        scene.fill_rounded_rect(
            box_origin,
            box_size,
            chrome.container_radius,
            chrome.colors.container,
        );
        if let Some(label) = &self.surfaces[s].label {
            label.paint(
                Point::new(
                    box_origin.x + MENU_GROUP_LABEL_H_PADDING,
                    box_origin.y + MENU_SURFACE_V_PADDING + MENU_GROUP_LABEL_V_PADDING,
                ),
                chrome.colors.supporting_content,
                scene,
            );
        }
        for i in 0..self.rows.len() {
            if self.rows[i].surface == s {
                self.paint_row(i, origin, chrome, scene);
            }
        }
    }

    /// Paint row `i`: its ink, its state layer, and every filled slot.
    fn paint_row(&self, i: usize, origin: Point, chrome: &Chrome, scene: &mut dyn PaintScene) {
        let colors = &chrome.colors;
        let row = &self.rows[i];
        let top = origin.y + row.top - self.scroll;
        if !row.kind.is_item() {
            // `m3e_menu_divider.dart`: a 1px rule inset by the row padding.
            scene.fill_rect(
                Point::new(
                    origin.x + MENU_SURFACE_H_PADDING + MENU_ROW_H_PADDING,
                    top + MENU_DIVIDER_V_PADDING,
                ),
                Size::new(
                    (self.width - 2.0 * MENU_SURFACE_H_PADDING - 2.0 * MENU_ROW_H_PADDING).max(0.0),
                    MENU_DIVIDER_THICKNESS,
                ),
                colors.divider,
            );
            return;
        }
        let item_origin = Point::new(origin.x + MENU_SURFACE_H_PADDING, top + MENU_ITEM_GAP / 2.0);
        let item_size = Size::new(
            (self.width - 2.0 * MENU_SURFACE_H_PADDING).max(0.0),
            row.height - MENU_ITEM_GAP,
        );
        if row.selected {
            scene.fill_rounded_rect(
                item_origin,
                item_size,
                chrome.item_radius,
                colors.selected_container,
            );
        }
        let mut state = InteractionState::new();
        // An open sub-trigger stays washed while its panel is up, so the
        // cascade reads as one connected surface.
        state.hovered = self.hovered == Some(i) || self.open_sub == Some(i);
        state.pressed = self.pressed == Some(i) && self.press_inside;
        let opacity = if row.enabled {
            state.resolve_opacity()
        } else {
            0.0
        };
        if opacity > 0.0 {
            scene.fill_rounded_rect(
                item_origin,
                item_size,
                chrome.item_radius,
                crate::overlay::with_alpha(colors.state_layer, opacity),
            );
        }
        let ink = colors.entry_foreground(row.enabled, row.destructive, row.selected);
        let icon_ink = colors.icon_foreground(row.enabled, row.destructive, row.selected);

        let mut x = item_origin.x + MENU_ROW_H_PADDING;
        let middle = item_origin.y + item_size.height / 2.0;
        if let Some(leading) = &row.leading {
            leading.paint(
                Point::new(x, middle - leading.extent / 2.0),
                icon_ink,
                scene,
            );
            x += MENU_ICON_SIZE + MENU_ICON_GAP;
        }
        let label_size = row.label.size();
        let supporting_size = row.supporting.as_ref().map_or(Size::ZERO, Run::size);
        let text_top = middle - (label_size.height + supporting_size.height) / 2.0;
        row.label.paint(Point::new(x, text_top), ink, scene);
        if let Some(supporting) = &row.supporting {
            supporting.paint(
                Point::new(x, text_top + label_size.height),
                colors.supporting_foreground(row.enabled, row.selected),
                scene,
            );
        }
        let mut right = item_origin.x + item_size.width - MENU_ROW_H_PADDING;
        if let Some(trailing) = &row.trailing {
            right -= MENU_ICON_SIZE;
            trailing.paint(
                Point::new(right, middle - trailing.extent / 2.0),
                icon_ink,
                scene,
            );
            right -= MENU_ICON_GAP;
        }
        if let Some(shortcut) = &row.shortcut {
            let size = shortcut.size();
            shortcut.paint(
                Point::new(right - size.width, middle - size.height / 2.0),
                colors.supporting_foreground(row.enabled, row.selected),
                scene,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::items::{
        menu_entry, menu_group, menu_selectable, menu_submenu, menu_toggleable,
    };
    use crate::menu::{MENU_CONTAINER_RADIUS, MENU_ITEM_RADIUS, MENU_SECTION_GAP};
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{BuildCtx, PointerButton, Rect as KRect, text::TextContext};
    use frust::{Brightness, FrameTime};
    use std::any::Any;

    const AREA: Size = Size::new(600.0, 800.0);

    /// What the panel reported, in order.
    #[derive(Default)]
    struct AppState {
        selections: Vec<MenuSelection>,
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        /// Origins of every filled icon path.
        paths: Vec<Point>,
        runs: usize,
        shadows: usize,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn fill_path(&mut self, origin: Point, _path: &BezPath, _brush: &Brush) {
            self.paths.push(origin);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.runs += 1;
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _std: f64, _c: Color) {
            self.shadows += 1;
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn pop_clip(&mut self) {}
    }

    /// The tree every structural test below renders: one of each node kind.
    fn every_kind() -> Vec<MenuNode> {
        vec![
            menu_entry("Cut").shortcut("Ctrl X").into(),
            MenuNode::Divider,
            menu_group(vec![
                menu_selectable("Grid", "grid").selected(true).into(),
                menu_selectable("List", "list").into(),
            ])
            .label("View")
            .into(),
            menu_toggleable("Word wrap", true).into(),
            menu_submenu("Share", vec![menu_entry("Copy link").into()]).into(),
        ]
    }

    fn view(nodes: Vec<MenuNode>) -> MenuPanelView<AppState> {
        menu_panel(nodes, |state: &mut AppState, selection| {
            state.selections.push(selection)
        })
    }

    fn build(view: &MenuPanelView<AppState>) -> MenuPanelWidget {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut MenuPanelWidget, area: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(area))
    }

    /// Build + lay out in one step: the state every event/paint test starts in.
    fn ready(nodes: Vec<MenuNode>) -> MenuPanelWidget {
        let mut w = build(&view(nodes));
        layout(&mut w, AREA);
        w
    }

    /// Rebuild `w` from `prev` to `next`, the way a live app's pass does.
    fn rebuild(
        w: &mut MenuPanelWidget,
        prev: &MenuPanelView<AppState>,
        next: &MenuPanelView<AppState>,
    ) {
        let mut counter = 0u64;
        View::<AppState>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter));
    }

    fn paint(w: &mut MenuPanelWidget) -> Recorder {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut rec = Recorder::default();
        let size = Size::new(w.width, w.viewport);
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut MenuPanelWidget, state: &mut AppState, event: &InputEvent) -> EventResult {
        let any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(any, Point::ZERO, Size::new(w.width, w.viewport));
        w.event(&mut ctx, event)
    }

    /// The centre of row `row`, in panel-local coordinates.
    fn row_center(w: &MenuPanelWidget, row: usize) -> Point {
        let (top, height) = w.band(row);
        Point::new(w.width / 2.0, top + height / 2.0 - w.scroll)
    }

    /// Press and release row `row`.
    fn click(w: &mut MenuPanelWidget, state: &mut AppState, row: usize) {
        let c = row_center(w, row);
        dispatch(w, state, &pointer(PointerPhase::Down, c.x, c.y));
        dispatch(w, state, &pointer(PointerPhase::Up, c.x, c.y));
    }

    /// Press and release inside the open submenu's first row.
    fn click_submenu_row(w: &mut MenuPanelWidget, state: &mut AppState) {
        let rect = w.open_sub_rect().expect("an open submenu is placed");
        let p = Point::new(
            rect.x0 + rect.width() / 2.0,
            rect.y0 + MENU_SURFACE_V_PADDING + MENU_ROW_MIN_HEIGHT / 2.0,
        );
        dispatch(w, state, &pointer(PointerPhase::Down, p.x, p.y));
        dispatch(w, state, &pointer(PointerPhase::Up, p.x, p.y));
    }

    // ---- structure --------------------------------------------------------

    #[test]
    fn all_six_kinds_render_across_the_surfaces_they_partition_into() {
        let mut w = ready(every_kind());
        // Entry, divider, two selectables, toggleable, submenu row.
        assert_eq!(w.rows.len(), 6);
        assert_eq!(
            w.surfaces.len(),
            3,
            "flat run, the labelled group, the tail"
        );
        assert!(w.surfaces[1].label.is_some(), "the group's section label");

        let rec = paint(&mut w);
        // One container per surface, each at the container radius, each lifted.
        let containers = rec
            .rrects
            .iter()
            .filter(|(_, _, radius, _)| *radius == MENU_CONTAINER_RADIUS)
            .count();
        assert_eq!(containers, 3);
        assert_eq!(rec.shadows, 3, "every surface takes the level-2 rung");
        // The divider is the one plain rect, at the hairline thickness.
        assert_eq!(rec.rects.len(), 1);
        assert_eq!(rec.rects[0].1.height, MENU_DIVIDER_THICKNESS);
        // Icons: the selected `Grid` check, the toggleable's box, the chevron.
        assert_eq!(rec.paths.len(), 3);
        // Every label, the section label and the shortcut shape real runs.
        assert!(rec.runs >= 7, "runs: {}", rec.runs);
    }

    #[test]
    fn the_surfaces_stack_with_the_section_gap_between_them() {
        let w = ready(every_kind());
        for pair in w.surfaces.windows(2) {
            assert_eq!(
                pair[1].top - (pair[0].top + pair[0].height),
                MENU_SECTION_GAP
            );
        }
        assert_eq!(w.surfaces[0].top, 0.0);
    }

    #[test]
    fn a_group_label_reserves_its_own_band_above_the_group_s_rows() {
        let labelled = ready(vec![
            menu_group(vec![menu_entry("a").into()])
                .label("Section")
                .into(),
        ]);
        let bare = ready(vec![menu_group(vec![menu_entry("a").into()]).into()]);
        assert!(
            labelled.surfaces[0].height > bare.surfaces[0].height,
            "{} vs {}",
            labelled.surfaces[0].height,
            bare.surfaces[0].height
        );
        assert!(labelled.rows[0].top > bare.rows[0].top);
    }

    #[test]
    fn a_row_is_at_least_the_reference_s_entry_height() {
        let w = ready(vec![menu_entry("Cut").into()]);
        assert_eq!(w.rows[0].height, MENU_ITEM_GAP + MENU_ROW_MIN_HEIGHT);
        // Supporting text never shrinks a row below the same floor.
        let tall = ready(vec![
            menu_entry("Cut").supporting("Removes the selection").into(),
        ]);
        assert!(tall.rows[0].height >= w.rows[0].height);
    }

    #[test]
    fn the_panel_width_clamps_into_the_reference_s_min_max_band() {
        let narrow = ready(vec![menu_entry("a").into()]);
        assert_eq!(
            narrow.width(),
            MENU_MIN_WIDTH,
            "short labels take the floor"
        );
        let wide = ready(vec![
            menu_entry("a label far longer than any menu should ever try to show inline")
                .shortcut("Ctrl Shift X")
                .into(),
        ]);
        assert_eq!(wide.width(), MENU_MAX_WIDTH, "long ones take the ceiling");
    }

    #[test]
    fn a_narrow_area_wins_over_the_reference_s_own_minimum() {
        let mut w = build(&view(vec![menu_entry("a").into()]));
        let size = layout(&mut w, Size::new(80.0, 800.0));
        assert_eq!(size.width, 80.0, "the panel never overflows its area");
    }

    #[test]
    fn a_tall_tree_caps_at_the_max_height_and_clips_while_scrolling() {
        let nodes: Vec<MenuNode> = (0..20)
            .map(|i| menu_entry(format!("row {i}")).into())
            .collect();
        let mut w = ready(nodes);
        assert_eq!(w.viewport, MENU_MAX_HEIGHT);
        assert!(w.content_height > MENU_MAX_HEIGHT);
        assert_eq!(paint(&mut w).clips, 1, "a scrolling panel clips");
        assert_eq!(
            paint(&mut ready(vec![menu_entry("a").into()])).clips,
            0,
            "…and one that fits never clips its own shadow out"
        );

        let mut state = AppState::default();
        let scrolled = dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 10.0),
                delta: ScrollDelta::Pixels(0.0, 120.0),
            },
        );
        assert_eq!(scrolled, EventResult::Handled);
        assert_eq!(w.scroll(), 120.0);
        // …and never past the end of the content.
        dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 10.0),
                delta: ScrollDelta::Pixels(0.0, 10_000.0),
            },
        );
        assert_eq!(w.scroll(), w.content_height - w.viewport);
    }

    // ---- selection semantics ---------------------------------------------

    #[test]
    fn an_entry_reports_a_press_at_its_depth_first_index() {
        let mut w = ready(every_kind());
        let mut state = AppState::default();
        click(&mut w, &mut state, 0);
        assert_eq!(state.selections.len(), 1);
        assert_eq!(state.selections[0].index, 0);
        assert_eq!(state.selections[0].label, "Cut");
        assert_eq!(state.selections[0].action, MenuAction::Press);
    }

    #[test]
    fn a_selectable_reports_its_value_and_never_flips_its_own_state() {
        let nodes = vec![
            menu_selectable("Grid", "grid").into(),
            menu_selectable("List", "list").into(),
        ];
        let mut w = build(&view(nodes.clone()).selected(Some("grid".into())));
        layout(&mut w, AREA);
        assert!(w.rows[0].selected, "the panel's value checks the row");
        assert!(!w.rows[1].selected);

        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        assert_eq!(state.selections[0].value(), Some("list"));
        assert!(
            w.rows[0].selected && !w.rows[1].selected,
            "controlled: the panel does not move the check itself"
        );

        // The app confirms, and the next rebuild moves it.
        rebuild(
            &mut w,
            &view(nodes.clone()).selected(Some("grid".into())),
            &view(nodes).selected(Some("list".into())),
        );
        layout(&mut w, AREA);
        assert!(!w.rows[0].selected && w.rows[1].selected);
    }

    #[test]
    fn a_toggleable_requests_the_opposite_of_its_current_state() {
        let mut w = ready(vec![
            menu_toggleable("Wrap", false).into(),
            menu_toggleable("Minimap", true).into(),
        ]);
        let mut state = AppState::default();
        click(&mut w, &mut state, 0);
        click(&mut w, &mut state, 1);
        assert_eq!(state.selections[0].checked(), Some(true));
        assert_eq!(state.selections[1].checked(), Some(false));
        assert!(
            !w.rows[0].selected && w.rows[1].selected,
            "controlled: the boxes stay as the caller last drew them"
        );
    }

    #[test]
    fn a_disabled_row_and_a_divider_report_nothing() {
        let mut w = ready(vec![
            menu_entry("Paste").enabled(false).into(),
            MenuNode::Divider,
        ]);
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y)),
            EventResult::Ignored
        );
        click(&mut w, &mut state, 1);
        assert!(state.selections.is_empty());
    }

    #[test]
    fn a_release_outside_the_armed_row_reports_nothing() {
        let mut w = ready(vec![menu_entry("a").into(), menu_entry("b").into()]);
        let mut state = AppState::default();
        let first = row_center(&w, 0);
        let second = row_center(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, first.x, first.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, second.x, second.y),
        );
        assert!(state.selections.is_empty(), "fires on up-inside only");
    }

    #[test]
    fn only_a_primary_press_arms_a_row() {
        let mut w = ready(vec![menu_entry("a").into()]);
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: c,
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &secondary),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, c.x, c.y)),
            EventResult::Ignored
        );
        assert!(state.selections.is_empty());
    }

    #[test]
    fn a_cancel_clears_the_press_without_reporting() {
        let mut w = ready(vec![menu_entry("a").into()]);
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Cancel, c.x, c.y)),
            EventResult::Handled
        );
        assert!(state.selections.is_empty());
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, c.x, c.y)),
            EventResult::Ignored,
            "the gesture is over"
        );
    }

    #[test]
    fn a_press_on_the_panel_s_own_background_is_left_for_the_host() {
        let mut w = ready(vec![menu_entry("a").into()]);
        let mut state = AppState::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 4.0, 1.0)),
            EventResult::Ignored,
            "the surface padding is not a row"
        );
    }

    // ---- submenus ---------------------------------------------------------

    #[test]
    fn a_tap_on_a_submenu_row_opens_its_panel_and_reports_nothing() {
        let mut w = ready(vec![
            menu_entry("Cut").into(),
            menu_submenu("Share", vec![menu_entry("Copy link").into()]).into(),
        ]);
        let mut state = AppState::default();
        assert_eq!(w.open_submenu(), None);
        click(&mut w, &mut state, 1);
        assert_eq!(w.open_submenu(), Some(1), "the reference's tap-open rule");
        assert!(state.selections.is_empty(), "opening is not selecting");

        // The nested panel is placed on the trailing side of its row.
        let rect = w.open_sub_rect().expect("an open submenu is placed");
        assert_eq!(rect.x0, w.width() + MENU_SUBMENU_GAP);
        assert_eq!(rect.y0, w.band(1).0 - MENU_SURFACE_V_PADDING);
        assert!(rect.width() > 0.0 && rect.height() > 0.0);
    }

    #[test]
    fn hovering_a_submenu_row_opens_it_and_hovering_elsewhere_closes_it() {
        let mut w = ready(vec![
            menu_entry("Cut").into(),
            menu_submenu("Share", vec![menu_entry("Copy link").into()]).into(),
        ]);
        let mut state = AppState::default();
        let sub = row_center(&w, 1);
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Move, sub.x, sub.y)
            ),
            EventResult::Ignored,
            "watching a move is not consuming it"
        );
        assert_eq!(w.open_submenu(), Some(1));

        // Travelling onto the open panel keeps it.
        let inside = w.open_sub_rect().expect("placed").center();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, inside.x, inside.y),
        );
        assert_eq!(
            w.open_submenu(),
            Some(1),
            "the pointer has to be able to get there"
        );

        // Any other row closes it.
        let other = row_center(&w, 0);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, other.x, other.y),
        );
        assert_eq!(w.open_submenu(), None);
    }

    #[test]
    fn a_submenu_row_reports_its_children_at_the_indices_that_follow_it() {
        let mut w = ready(vec![
            menu_entry("Cut").into(),
            menu_submenu(
                "Share",
                vec![menu_entry("Copy link").into(), menu_entry("Email").into()],
            )
            .into(),
            menu_entry("Paste").into(),
        ]);
        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        // Index 1 is the submenu row itself, 2 and 3 its children, 4 the tail.
        click_submenu_row(&mut w, &mut state);
        assert_eq!(state.selections.len(), 1, "the nested row reported");
        assert_eq!(state.selections[0].index, 2);
        assert_eq!(state.selections[0].label, "Copy link");
        assert_eq!(
            w.rows[2].index, 4,
            "the tail keeps counting past the subtree"
        );
    }

    #[test]
    fn a_disabled_submenu_row_opens_nothing() {
        let mut w = ready(vec![
            menu_submenu("Share", vec![menu_entry("Copy link").into()])
                .enabled(false)
                .into(),
        ]);
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        click(&mut w, &mut state, 0);
        assert_eq!(w.open_submenu(), None);
    }

    #[test]
    fn closing_the_panel_closes_the_whole_chain() {
        let nodes = vec![
            menu_submenu(
                "Share",
                vec![menu_submenu("More", vec![menu_entry("Deep").into()]).into()],
            )
            .into(),
        ];
        let mut w = ready(nodes.clone());
        let mut state = AppState::default();
        click(&mut w, &mut state, 0);
        assert_eq!(w.open_submenu(), Some(0));
        // Open the nested one too, through the child panel itself.
        click_submenu_row(&mut w, &mut state);
        assert_eq!(nested(&mut w, 0), Some(0), "the cascade is two deep");

        // The host's dismissal reaches the panel as `open == false`.
        rebuild(
            &mut w,
            &view(nodes.clone()).open(true),
            &view(nodes).open(false),
        );
        assert_eq!(w.open_submenu(), None);
        assert_eq!(
            nested(&mut w, 0),
            None,
            "every depth closed, not just the first"
        );
    }

    /// The open submenu of the panel behind pod `pod`.
    ///
    /// Two downcasts, not one: a submenu pod holds an `AnyView`'s element,
    /// which is itself a `Box<dyn Widget>` around the panel.
    fn nested(w: &mut MenuPanelWidget, pod: usize) -> Option<usize> {
        let boxed = w.pods[pod]
            .widget_mut()
            .downcast_mut::<Box<dyn Widget>>()
            .expect("an erased pod");
        (**boxed)
            .downcast_mut::<MenuPanelWidget>()
            .expect("the submenu panel")
            .open_submenu()
    }

    // ---- chrome -----------------------------------------------------------

    #[test]
    fn a_pressed_row_takes_a_state_layer_and_a_selected_one_its_own_fill() {
        let mut w = ready(vec![menu_selectable("Grid", "grid").selected(true).into()]);
        let colors = MenuColors::resolve(
            Some(&crate::baseline().with_brightness(Brightness::Light)),
            MenuColorStyle::Standard,
        );
        let washes = |rec: &Recorder| -> Vec<Color> {
            rec.rrects
                .iter()
                .filter(|(_, _, radius, _)| *radius == MENU_ITEM_RADIUS)
                .map(|(_, _, _, color)| *color)
                .collect()
        };
        let rec = paint(&mut w);
        assert_eq!(
            washes(&rec),
            vec![colors.selected_container],
            "the selected fill, and no wash yet"
        );

        // The press is what a paint pass can observe: a `Down` ends the hover
        // link, so the hover latch is cleared by `paint`'s own self-correction
        // while the pressed flag stands.
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        let rec = paint(&mut w);
        let painted = washes(&rec);
        assert_eq!(painted.len(), 2, "the fill plus the press wash over it");
        assert_eq!(painted[0], colors.selected_container);
        assert_eq!(
            painted[1].components[3],
            crate::interaction::PRESSED_OPACITY
        );
    }

    #[test]
    fn a_drag_off_the_armed_row_drops_its_wash_and_a_drag_back_re_arms_it() {
        let mut w = ready(vec![menu_entry("a").into(), menu_entry("b").into()]);
        let mut state = AppState::default();
        let first = row_center(&w, 0);
        let second = row_center(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, first.x, first.y),
        );
        assert!(w.press_inside);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, second.x, second.y),
        );
        assert!(!w.press_inside, "the wash follows the pointer out");
        assert_eq!(w.pressed, Some(0), "…but the gesture is still armed");
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, first.x, first.y),
        );
        assert!(w.press_inside, "and back in");
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, first.x, first.y),
        );
        assert_eq!(state.selections.len(), 1);
    }

    #[test]
    fn a_move_latches_the_hovered_row_for_the_next_paint() {
        let mut w = ready(vec![menu_entry("a").into(), menu_entry("b").into()]);
        let mut state = AppState::default();
        let second = row_center(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, second.x, second.y),
        );
        assert_eq!(w.hovered, Some(1));
        // The panel's own padding is not a row.
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, 2.0, 1.0));
        assert_eq!(w.hovered, None);
    }

    #[test]
    fn a_disabled_row_never_washes() {
        let mut w = ready(vec![menu_entry("Paste").enabled(false).into()]);
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        let rec = paint(&mut w);
        assert!(
            !rec.rrects
                .iter()
                .any(|(_, _, radius, _)| *radius == MENU_ITEM_RADIUS),
            "no fill and no wash"
        );
    }

    #[test]
    fn the_radii_resolve_off_the_theme_s_shape_scale() {
        // The reference's `containerRadius: 16` / `itemRadius: 12` land exactly
        // on the `large` / `medium` shape tokens, which is what lets a themed
        // pass read the token and the constants stand as bare fallbacks. The
        // radius-filtered assertions above depend on that coincidence holding.
        let theme = crate::baseline();
        assert_eq!(container_radius(Some(&theme)), theme.shape.large);
        assert_eq!(item_radius(Some(&theme)), theme.shape.medium);
        assert_eq!(container_radius(None), MENU_CONTAINER_RADIUS);
        assert_eq!(item_radius(None), MENU_ITEM_RADIUS);
        assert_eq!(theme.shape.large, MENU_CONTAINER_RADIUS);
        assert_eq!(theme.shape.medium, MENU_ITEM_RADIUS);
    }

    #[test]
    fn the_vibrant_style_paints_its_own_container() {
        let mut standard = ready(vec![menu_entry("a").into()]);
        let mut vibrant =
            build(&view(vec![menu_entry("a").into()]).color_style(MenuColorStyle::Vibrant));
        layout(&mut vibrant, AREA);
        let a = paint(&mut standard).rrects[0].3;
        let b = paint(&mut vibrant).rrects[0].3;
        assert_ne!(a, b);
        let theme = crate::baseline().with_brightness(Brightness::Light);
        assert_eq!(a, theme.scheme().surface_container_low);
        assert_eq!(b, theme.scheme().tertiary_container);
    }

    #[test]
    fn an_unthemed_pass_paints_the_baseline_light_roles() {
        // Nothing may panic without a theme threaded in, and the fallbacks are
        // the same values the themed pass resolves.
        let mut w = ready(vec![menu_entry("a").into()]);
        let mut rec = Recorder::default();
        let size = Size::new(w.width, w.viewport);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(
            rec.rrects[0].3,
            MenuColors::resolve(None, MenuColorStyle::Standard).container
        );
    }

    #[test]
    fn the_semantics_tree_is_a_menu_of_role_typed_rows() {
        use frust_core::RenderRoot;
        let mut root: RenderRoot<AppState, MenuPanelView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| {
            view(vec![
                menu_entry("Cut").into(),
                MenuNode::Divider,
                menu_selectable("Grid", "grid").selected(true).into(),
                menu_toggleable("Wrap", false).into(),
            ])
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(AREA, &mut tcx as &mut dyn Any);
        let nodes = root.semantics().nodes;
        let roles: Vec<_> = nodes.iter().map(|(_, n)| n.role()).collect();
        assert!(roles.contains(&Role::Menu));
        assert_eq!(
            roles.iter().filter(|r| **r == Role::MenuItem).count(),
            1,
            "the divider contributes nothing"
        );
        assert!(roles.contains(&Role::MenuItemRadio));
        assert!(roles.contains(&Role::MenuItemCheckBox));
        let radio = nodes
            .iter()
            .find(|(_, n)| n.role() == Role::MenuItemRadio)
            .expect("a radio row");
        assert_eq!(radio.1.label(), Some("Grid"));
        assert_eq!(radio.1.toggled(), Some(Toggled::True));
    }

    #[test]
    fn a_rebuild_that_re_supplies_the_same_tree_keeps_its_shaped_runs() {
        let nodes = every_kind();
        let mut w = ready(nodes.clone());
        let before = w.rows[0].label.size();
        assert!(before.width > 0.0);
        rebuild(&mut w, &view(nodes.clone()), &view(nodes));
        assert_eq!(
            w.rows[0].label.size(),
            before,
            "no re-shape, so the cached layout survives"
        );
    }

    #[test]
    fn a_rebuild_with_a_new_tree_replaces_the_rows_and_drops_the_chain() {
        let nodes = vec![menu_submenu("Share", vec![menu_entry("Copy link").into()]).into()];
        let mut w = ready(nodes.clone());
        let mut state = AppState::default();
        click(&mut w, &mut state, 0);
        assert_eq!(w.open_submenu(), Some(0));

        rebuild(
            &mut w,
            &view(nodes),
            &view(vec![menu_entry("Only this").into()]),
        );
        layout(&mut w, AREA);
        assert_eq!(w.rows.len(), 1);
        assert_eq!(w.open_submenu(), None);
        assert!(w.pods.is_empty(), "the submenu pod went with its row");
    }

    #[test]
    fn a_broadcast_reaches_every_submenu_pod_and_is_never_consumed() {
        let mut w = ready(vec![
            menu_submenu("Share", vec![menu_entry("Copy link").into()]).into(),
        ]);
        let mut state = AppState::default();
        let result = dispatch(&mut w, &mut state, &InputEvent::Housekeeping);
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn a_key_event_is_left_for_the_host_to_dismiss_on() {
        use frust::authoring::{Key, KeyEvent, Modifiers, NamedKey};
        let mut w = ready(vec![menu_entry("a").into()]);
        let mut state = AppState::default();
        let escape = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(dispatch(&mut w, &mut state, &escape), EventResult::Ignored);
    }

    #[test]
    fn a_paint_with_no_hover_link_clears_the_latched_row() {
        let mut w = ready(vec![menu_entry("a").into()]);
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        assert_eq!(w.hovered, Some(0));
        // `PaintCtx::for_test` reports no hover path, which is authoritative.
        paint(&mut w);
        assert_eq!(w.hovered, None);
    }

    #[test]
    fn every_painted_rect_stays_inside_the_panel_s_own_width() {
        let mut w = ready(every_kind());
        let rec = paint(&mut w);
        let panel = KRect::from_origin_size(Point::ORIGIN, Size::new(w.width, w.viewport));
        for (origin, size, _, _) in rec.rrects.iter() {
            assert!(origin.x >= panel.x0 - 0.001, "{origin:?}");
            assert!(origin.x + size.width <= panel.x1 + 0.001, "{origin:?}");
        }
    }
}
