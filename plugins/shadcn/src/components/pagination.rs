//! `pagination`: the page navigator — previous/next, page numbers, ellipsis.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/pagination.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17): a
//! `role="navigation"` nav (`mx-auto flex w-full justify-center`) over a
//! `flex-row items-center gap-1` list, whose links are `buttonVariants` looks —
//! `variant: isActive ? "outline" : "ghost"`, `size: "icon"` for a page number
//! and `"default"` with `gap-1 px-2.5` for previous/next — plus a `size-9`
//! ellipsis holding a `MoreHorizontal` glyph.
//!
//! # The two button looks are ported, not imported
//!
//! A page link's chrome is `buttonVariants`' `outline`/`ghost` arms:
//!
//! - **outline** (the active page): `border bg-background shadow-xs
//!   hover:bg-accent`, `dark:border-input dark:bg-input/30`.
//! - **ghost** (every other item): no fill at rest, `hover:bg-accent`,
//!   `dark:hover:bg-accent/50`.
//!
//! Those class lists are transcribed here rather than reached for through the
//! catalog's own `button` module: a component's look is its own, and coupling two
//! components' internals so one can borrow the other's paint would make either
//! one's chrome unchangeable without the other. The duplication is deliberate and
//! small (two fills, a border, a radius) — see this crate's own docs on where a
//! shared vocabulary does live ([`crate::style`]).
//!
//! # Controlled, and what "disabled at the ends" means
//!
//! Every activation reports a **page number** through `on_select`; the widget
//! never moves `current`. Previous/next resolve to the nearest page on either side
//! of `current` *within the item list*, and when there is none the item is dimmed
//! and inert. Upstream leaves them permanently enabled (its links are `<a href>`s
//! that a router resolves); here the widget knows the page list, so pointing at a
//! page that does not exist would be a defect rather than a link.

use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, ThemeTextColor, View, Widget,
    any, build_child, erase_callback_arg, rebuild_child, route_event, teardown_child,
    text::FontWeight, visit_children,
};
use frust::{TextView, Theme, text};

use crate::components::input::FALLBACK;
use crate::components::native_select::{activates, draw_chevron};
use crate::hit::inside;
use crate::style::{self, PATH_TOLERANCE};
use crate::tokens::ShadcnTokens;

/// Gap between items: `gap-1`.
const ITEM_GAP: f64 = 4.0;
/// Horizontal padding on a previous/next item: `px-2.5`.
const EDGE_PAD_X: f64 = 10.0;
/// Gap between a previous/next item's chevron and its label: `gap-1`.
const EDGE_GAP: f64 = 4.0;
/// Alpha of the dark-mode ghost hover fill: `dark:hover:bg-accent/50`.
const DARK_ACCENT_ALPHA: f32 = 0.5;
/// Alpha of the dark-mode outline fill: `dark:bg-input/30`.
const DARK_OUTLINE_ALPHA: f32 = 0.3;
/// Lucide's `viewBox` edge, the denominator its coordinates are given in.
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Outer diameter of one `MoreHorizontal` dot, in `viewBox` units (`r="1"` with a
/// 2-unit stroke).
const DOT_DIAMETER: f64 = 4.0;
/// Distance from the glyph's centre to the outer dots' centres, in `viewBox`
/// units (`cx="5"`/`cx="19"` against a centre of 12).
const DOT_SPACING: f64 = 7.0;

/// One entry in a pagination list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaginationItem {
    /// A page number link (`PaginationLink`).
    Page(usize),
    /// The `PaginationEllipsis` gap marker.
    Ellipsis,
}

/// A view-held, typed page-selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative pagination navigator. See the [module docs](self).
pub struct PaginationView<State: 'static> {
    items: Vec<PaginationItem>,
    current: usize,
    previous_label: String,
    next_label: String,
    on_select: OnSelect<State>,
}

/// Build a pagination navigator over `items`, marking `current` as the active
/// page and reporting every activation through `on_select(state, page)`.
pub fn pagination<State: 'static, I, F>(
    items: I,
    current: usize,
    on_select: F,
) -> PaginationView<State>
where
    I: IntoIterator<Item = PaginationItem>,
    F: Fn(&mut State, usize) + 'static,
{
    PaginationView {
        items: items.into_iter().collect(),
        current,
        previous_label: "Previous".to_string(),
        next_label: "Next".to_string(),
        on_select: Rc::new(on_select),
    }
}

impl<State: 'static> PaginationView<State> {
    /// Relabel the previous-page item (upstream's `Previous`).
    pub fn previous_label(mut self, label: impl Into<String>) -> Self {
        self.previous_label = label.into();
        self
    }

    /// Relabel the next-page item (upstream's `Next`).
    pub fn next_label(mut self, label: impl Into<String>) -> Self {
        self.next_label = label.into();
        self
    }

    /// The page the previous item targets: the highest page below `current`.
    fn previous_page(&self) -> Option<usize> {
        self.pages().filter(|page| *page < self.current).max()
    }

    /// The page the next item targets: the lowest page above `current`.
    fn next_page(&self) -> Option<usize> {
        self.pages().filter(|page| *page > self.current).min()
    }

    /// The page numbers in the item list, in list order.
    fn pages(&self) -> impl Iterator<Item = usize> + '_ {
        self.items.iter().filter_map(|item| match item {
            PaginationItem::Page(page) => Some(*page),
            PaginationItem::Ellipsis => None,
        })
    }

    /// The label views, in pod order: previous, every page number, next.
    fn label_views(&self) -> Vec<AnyView<State>> {
        let mut labels = vec![any(item_text(&self.previous_label))];
        for page in self.pages() {
            let label = item_text(&page.to_string());
            // `font-medium` comes from `buttonVariants`' base class list.
            labels.push(any(label.weight(FontWeight::MEDIUM)));
        }
        labels.push(any(item_text(&self.next_label)));
        labels
    }

    /// The rendered item list, in visual order.
    fn item_kinds(&self) -> Vec<ItemKind> {
        let mut kinds = vec![ItemKind::Previous];
        kinds.extend(self.items.iter().map(|item| match item {
            PaginationItem::Page(page) => ItemKind::Page(*page),
            PaginationItem::Ellipsis => ItemKind::Ellipsis,
        }));
        kinds.push(ItemKind::Next);
        kinds
    }
}

/// A pagination item's label: `text-sm` in `foreground`.
fn item_text(label: &str) -> TextView {
    text(label.to_string())
        .size(style::TEXT_SM as f32)
        .family(crate::tokens::sans_family())
        .themed_role(ThemeTextColor::OnSurface)
}

/// What a rendered item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    /// The previous-page item: a chevron plus a label.
    Previous,
    /// A page-number link.
    Page(usize),
    /// The ellipsis gap marker (inert, `aria-hidden` upstream).
    Ellipsis,
    /// The next-page item: a label plus a chevron.
    Next,
}

/// One laid-out item.
struct ItemGeom {
    kind: ItemKind,
    /// The item's label pod, if it has one (the ellipsis does not).
    label: Option<usize>,
    /// Resolved leading edge, relative to the widget's origin.
    x: f64,
    /// Resolved width.
    width: f64,
    /// Whether this item is the active page (`outline` rather than `ghost`).
    active: bool,
    /// The page an activation reports; `None` makes the item inert.
    target: Option<usize>,
}

/// The retained widget for a [`PaginationView`].
pub struct PaginationWidget {
    /// The label pods: previous, page numbers in order, next.
    labels: Vec<ChildPod>,
    items: Vec<ItemGeom>,
    /// The latched hovered item (self-corrected from `PaintCtx::is_hovered`).
    hovered: Option<usize>,
    pressed: Option<usize>,
    /// The item the keyboard acts on while the widget holds focus.
    focused: Option<usize>,
    captured: bool,
    on_select: ErasedArgCallback<usize>,
}

impl<State: 'static> View<State> for PaginationView<State> {
    type Element = PaginationWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PaginationWidget {
        let labels = self
            .label_views()
            .iter()
            .map(|view| build_child(view, ctx))
            .collect();
        PaginationWidget {
            labels,
            items: self.geometry(),
            hovered: None,
            pressed: None,
            focused: None,
            captured: false,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PaginationWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let (prev_labels, labels) = (prev.label_views(), self.label_views());
        if prev_labels.len() != labels.len() {
            // The item list changed length: rebuild the label set (identity here is
            // positional, so there is nothing to reconcile against).
            for (pod, view) in element.labels.iter_mut().zip(&prev_labels) {
                teardown_child(view, pod, ctx);
            }
            element.labels = labels.iter().map(|view| build_child(view, ctx)).collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for ((pod, p), n) in element.labels.iter_mut().zip(&prev_labels).zip(&labels) {
                flags |= rebuild_child(p, n, pod, ctx);
            }
        }
        let items = self.geometry();
        if items.len() != element.items.len()
            || items
                .iter()
                .zip(&element.items)
                .any(|(n, p)| n.kind != p.kind || n.active != p.active || n.target != p.target)
        {
            element.items = items;
            element.hovered = None;
            element.pressed = None;
            element.focused = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_select = erase_callback_arg(&self.on_select);
        flags
    }

    fn teardown(&self, element: &mut PaginationWidget, ctx: &mut BuildCtx<'_>) {
        for (pod, view) in element.labels.iter_mut().zip(self.label_views()) {
            teardown_child(&view, pod, ctx);
        }
    }
}

impl<State: 'static> PaginationView<State> {
    /// The item list with its pod indices, active flags and activation targets
    /// resolved (geometry itself is filled in by `layout`).
    fn geometry(&self) -> Vec<ItemGeom> {
        // Label pods run `[previous, page…, next]`, so a page's slot is its
        // one-based position among the pages.
        let mut page_slot = 0usize;
        self.item_kinds()
            .into_iter()
            .map(|kind| {
                let (label, active, target) = match kind {
                    ItemKind::Previous => (Some(0), false, self.previous_page()),
                    ItemKind::Next => (Some(self.pages().count() + 1), false, self.next_page()),
                    ItemKind::Page(page) => {
                        page_slot += 1;
                        (Some(page_slot), page == self.current, Some(page))
                    }
                    ItemKind::Ellipsis => (None, false, None),
                };
                ItemGeom {
                    kind,
                    label,
                    x: 0.0,
                    width: 0.0,
                    active,
                    target,
                }
            })
            .collect()
    }
}

impl PaginationWidget {
    /// The item at widget-local `pos`, if any.
    fn item_at(&self, pos: Point, size: Size) -> Option<usize> {
        if !inside(pos, size) {
            return None;
        }
        self.items
            .iter()
            .position(|item| pos.x >= item.x && pos.x < item.x + item.width)
    }

    /// Whether item `index` can be activated.
    fn is_activatable(&self, index: usize) -> bool {
        self.items
            .get(index)
            .is_some_and(|item| item.target.is_some())
    }

    /// Fire `on_select` for item `index` if it targets a page.
    fn activate(&mut self, index: usize, ctx: &mut EventCtx) {
        if let Some(page) = self.items.get(index).and_then(|item| item.target) {
            (self.on_select)(ctx, page);
        }
    }
}

impl Widget for PaginationWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let height = style::HEIGHT_DEFAULT;
        let label_bc = BoxConstraints::new(Size::ZERO, Size::new(bc.max().width.max(0.0), height));
        // Measure every label, then size each item from its own class list:
        // `size-9` for a page number and the ellipsis, `h-9 px-2.5 gap-1` plus a
        // chevron for previous/next.
        for index in 0..self.items.len() {
            let Some(slot) = self.items[index].label else {
                self.items[index].width = height;
                continue;
            };
            let label = self.labels[slot].layout_child(ctx, &label_bc);
            self.items[index].width = match self.items[index].kind {
                ItemKind::Previous | ItemKind::Next => {
                    2.0 * EDGE_PAD_X + style::ICON_SIZE + EDGE_GAP + label.width
                }
                _ => height.max(label.width + 2.0 * EDGE_PAD_X),
            };
        }
        let total: f64 = self.items.iter().map(|item| item.width).sum::<f64>()
            + ITEM_GAP * (self.items.len().saturating_sub(1)) as f64;
        // `mx-auto … justify-center`: the row is centered in the available width.
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            total
        };
        let mut x = ((width - total) / 2.0).max(0.0);
        for index in 0..self.items.len() {
            self.items[index].x = x;
            if let Some(slot) = self.items[index].label {
                let label = self.labels[slot].size();
                let label_x = match self.items[index].kind {
                    // The chevron leads the previous item and trails the next one.
                    ItemKind::Previous => x + EDGE_PAD_X + style::ICON_SIZE + EDGE_GAP,
                    ItemKind::Next => x + EDGE_PAD_X,
                    _ => x + (self.items[index].width - label.width) / 2.0,
                };
                self.labels[slot].set_origin(Point::new(
                    label_x,
                    ((height - label.height) / 2.0).max(0.0),
                ));
            }
            x += self.items[index].width + ITEM_GAP;
        }
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: nothing tells this widget the pointer left.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let theme = Theme::from_paint_ctx(ctx);
        let dark = style::is_dark(theme);
        let radius = ShadcnTokens::resolve_radius(None, theme).md;
        let accent = theme.map_or(FALLBACK.accent, |t| t.scheme().primary_container);
        let hover_fill = if dark {
            style::with_alpha(accent, DARK_ACCENT_ALPHA)
        } else {
            accent
        };
        // `outline`: `bg-background` in light mode, `bg-input/30` in dark.
        let outline_fill = if dark {
            style::with_alpha(
                theme.map_or(FALLBACK.input, |t| t.scheme().outline_variant),
                DARK_OUTLINE_ALPHA,
            )
        } else {
            theme.map_or(FALLBACK.background, |t| t.scheme().surface)
        };
        let border = theme.map_or(FALLBACK.border, |t| t.scheme().outline);
        let muted = theme.map_or(FALLBACK.muted_foreground, |t| t.scheme().on_surface_variant);
        let focused_item = ctx.has_focus().then_some(self.focused).flatten();

        for index in 0..self.items.len() {
            let (kind, x, width, active, inert) = {
                let item = &self.items[index];
                (
                    item.kind,
                    item.x,
                    item.width,
                    item.active,
                    item.target.is_none() && item.kind != ItemKind::Ellipsis,
                )
            };
            let item_origin = Point::new(origin.x + x, origin.y);
            let item_size = Size::new(width, size.height);
            if active {
                style::draw_shadow(
                    scene,
                    item_origin,
                    item_size,
                    radius,
                    style::SHADOW_XS,
                    theme,
                );
                scene.fill_rounded_rect(item_origin, item_size, radius, outline_fill);
            }
            let hovered = self.hovered == Some(index) || self.pressed == Some(index);
            if hovered && !inert && kind != ItemKind::Ellipsis {
                scene.fill_rounded_rect(item_origin, item_size, radius, hover_fill);
            }
            if active {
                let rr = RoundedRect::from_rect(
                    Rect::from_origin_size(Point::ORIGIN, item_size),
                    radius,
                );
                scene.stroke_path(
                    item_origin,
                    &rr.to_path(PATH_TOLERANCE),
                    style::BORDER_WIDTH,
                    &Brush::Solid(border),
                );
            }
            if focused_item == Some(index) {
                style::draw_focus_ring(
                    scene,
                    item_origin,
                    item_size,
                    radius,
                    style::ring_color(None, theme),
                );
            }
            // The two chevrons and the ellipsis dots, dimmed when inert.
            let glyph = style::disabled_tint(muted, inert);
            let center_y = origin.y + size.height / 2.0;
            match kind {
                ItemKind::Previous => draw_chevron(
                    scene,
                    Point::new(
                        item_origin.x + EDGE_PAD_X + style::ICON_SIZE / 2.0,
                        center_y,
                    ),
                    style::ICON_SIZE,
                    std::f64::consts::FRAC_PI_2,
                    glyph,
                ),
                ItemKind::Next => draw_chevron(
                    scene,
                    Point::new(
                        item_origin.x + width - EDGE_PAD_X - style::ICON_SIZE / 2.0,
                        center_y,
                    ),
                    style::ICON_SIZE,
                    -std::f64::consts::FRAC_PI_2,
                    glyph,
                ),
                ItemKind::Ellipsis => {
                    draw_ellipsis(
                        scene,
                        Point::new(item_origin.x + width / 2.0, center_y),
                        style::ICON_SIZE,
                        muted,
                    );
                }
                ItemKind::Page(_) => {}
            }
        }
        // Labels last, over their item's chrome. An inert end item's label rides a
        // 50%-opacity layer (`disabled:opacity-50`) so the whole item — chevron
        // (dimmed above) and text alike — reads inert.
        for index in 0..self.items.len() {
            let Some(slot) = self.items[index].label else {
                continue;
            };
            let inert =
                self.items[index].target.is_none() && self.items[index].kind != ItemKind::Ellipsis;
            if inert {
                let pod_origin = origin + self.labels[slot].origin().to_vec2();
                scene.push_layer(
                    pod_origin,
                    self.labels[slot].size(),
                    style::DISABLED_OPACITY,
                );
                self.labels[slot].paint_child(ctx, scene);
                scene.pop_layer();
            } else {
                self.labels[slot].paint_child(ctx, scene);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Labels are text leaves and consume nothing, but a container forwards
        // regardless (broadcasts, and the pods stay live).
        let routed = route_event(&mut self.labels, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        if let InputEvent::Key(key) = event {
            let Some(index) = self.focused.filter(|i| self.is_activatable(*i)) else {
                return EventResult::Ignored;
            };
            if !activates(&key.key) {
                return EventResult::Ignored;
            }
            self.activate(index, ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if !self.captured {
                    // The container claims after routing, so a claiming child (none
                    // today, but a label is a real pod) is recorded first.
                    let over = self
                        .item_at(p.position, size)
                        .filter(|index| self.is_activatable(*index));
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    } else if self.item_at(p.position, size).is_some() {
                        ctx.set_cursor(style::DISABLED_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                let armed = self
                    .item_at(p.position, size)
                    .filter(|index| self.pressed == Some(*index));
                if self.pressed != armed {
                    self.pressed = armed;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                let Some(index) = self
                    .item_at(p.position, size)
                    .filter(|i| self.is_activatable(*i))
                else {
                    return EventResult::Ignored;
                };
                self.pressed = Some(index);
                self.focused = Some(index);
                self.captured = true;
                ctx.capture_pointer();
                // Focus is what paints the ring and routes Space/Enter here.
                ctx.request_focus();
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
                    && self.item_at(p.position, size) == Some(index)
                {
                    self.activate(index, ctx);
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
            Role::Navigation,
            |_node| {},
            |ctx| {
                for item in &self.items {
                    if item.kind == ItemKind::Ellipsis {
                        // `aria-hidden` upstream: a gap marker, not a target.
                        continue;
                    }
                    let label = item.label.map(|slot| &self.labels[slot]);
                    ctx.push_container(
                        Role::Link,
                        |node| {
                            if item.target.is_some() {
                                node.add_action(Action::Click);
                            } else {
                                node.set_disabled();
                            }
                            if item.active {
                                // `aria-current="page"`.
                                node.set_selected(true);
                            }
                        },
                        |ctx| {
                            if let Some(pod) = label {
                                pod.semantics_child(ctx);
                            }
                        },
                    );
                }
            },
        );
    }

    visit_children!(labels);
}

/// Paint lucide's `MoreHorizontal` (three dots) centered on `center`.
fn draw_ellipsis(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let diameter = DOT_DIAMETER * scale;
    for offset in [-DOT_SPACING, 0.0, DOT_SPACING] {
        let x = center.x + offset * scale - diameter / 2.0;
        scene.fill_rounded_rect(
            Point::new(x, center.y - diameter / 2.0),
            Size::new(diameter, diameter),
            diameter / 2.0,
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Key, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use frust::{Brightness, CursorIcon, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(
            &mut self,
            origin: Point,
            path: &frust::authoring::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn draw_shadow(
            &mut self,
            _origin: Point,
            _size: Size,
            _radius: f64,
            _std_dev: f64,
            _color: Color,
        ) {
            self.shadows += 1;
        }
    }

    impl Recorder {
        /// The item fills of `color`, by their x origin.
        fn fills(&self, color: Color) -> Vec<f64> {
            self.rrects
                .iter()
                .filter(|(_, _, _, c)| *c == color)
                .map(|(origin, _, _, _)| origin.x)
                .collect()
        }

        fn borders(&self) -> Vec<(Rect, Color)> {
            self.strokes
                .iter()
                .filter(|(_, w, _)| *w == style::BORDER_WIDTH)
                .map(|(bbox, _, color)| (*bbox, *color))
                .collect()
        }

        fn rings(&self) -> Vec<Rect> {
            self.strokes
                .iter()
                .filter(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .map(|(bbox, _, _)| *bbox)
                .collect()
        }
    }

    const WINDOW: Size = Size::new(500.0, 60.0);

    #[derive(Default)]
    struct AppState {
        selected: Vec<usize>,
    }

    /// `[prev] 1 2 3 … [next]` with page 2 current.
    fn nav() -> PaginationView<AppState> {
        pagination(
            [
                PaginationItem::Page(1),
                PaginationItem::Page(2),
                PaginationItem::Page(3),
                PaginationItem::Ellipsis,
            ],
            2,
            |s: &mut AppState, page| s.selected.push(page),
        )
    }

    fn build<S: 'static>(view: &PaginationView<S>) -> PaginationWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut PaginationWidget, size: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    struct Harness {
        root: RenderRoot<AppState, PaginationView<AppState>>,
        state: AppState,
        tcx: TextContext,
        current: usize,
    }

    impl Harness {
        fn new(current: usize) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                current,
            };
            h.pass();
            h
        }

        fn theme(&mut self, brightness: Brightness) {
            self.root
                .set_theme(Box::new(crate::theme().with_brightness(brightness)));
            self.pass();
        }

        fn pass(&mut self) {
            let current = self.current;
            let mut logic = move |_s: &mut AppState| {
                pagination(
                    [
                        PaginationItem::Page(1),
                        PaginationItem::Page(2),
                        PaginationItem::Page(3),
                        PaginationItem::Ellipsis,
                    ],
                    current,
                    |s: &mut AppState, page| s.selected.push(page),
                )
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64) -> bool {
            self.root
                .event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(x, style::HEIGHT_DEFAULT / 2.0),
                        button: PointerButton::Primary,
                    }),
                )
                .needs_redraw
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
    }

    /// The x-centre of item `index` at [`WINDOW`], from a standalone layout of the
    /// same view (the root exposes no read-back seam for a live widget).
    fn item_center(index: usize, current: usize) -> f64 {
        let view: PaginationView<AppState> = pagination(
            [
                PaginationItem::Page(1),
                PaginationItem::Page(2),
                PaginationItem::Page(3),
                PaginationItem::Ellipsis,
            ],
            current,
            |_s: &mut AppState, _page| {},
        );
        let mut w = build(&view);
        layout(&mut w, WINDOW);
        w.items[index].x + w.items[index].width / 2.0
    }

    #[test]
    fn the_row_is_h9_centered_with_size_9_pages_and_padded_end_items() {
        let mut w = build(&nav());
        let size = layout(&mut w, WINDOW);
        assert_eq!(size, Size::new(WINDOW.width, style::HEIGHT_DEFAULT));
        assert_eq!(w.items.len(), 6, "prev + 3 pages + ellipsis + next");
        assert_eq!(w.items[1].kind, ItemKind::Page(1));
        assert_eq!(w.items[1].width, style::HEIGHT_DEFAULT, "size-9");
        assert_eq!(w.items[4].kind, ItemKind::Ellipsis);
        assert_eq!(w.items[4].width, style::HEIGHT_DEFAULT, "size-9");
        // The end items carry a chevron plus their label.
        assert!(w.items[0].width > style::HEIGHT_DEFAULT);
        // `justify-center`: equal slack either side.
        let total_end = w.items[5].x + w.items[5].width;
        assert!((w.items[0].x - (WINDOW.width - total_end)).abs() < 1e-9);
        // `gap-1` between neighbours.
        assert_eq!(w.items[2].x - (w.items[1].x + w.items[1].width), ITEM_GAP);
    }

    #[test]
    fn the_active_page_gets_the_outline_look_and_the_rest_stay_ghost() {
        let mut h = Harness::new(2);
        h.theme(Brightness::Light);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let rec = h.paint();
        // Exactly one outline item: a background fill, a border and a shadow.
        assert_eq!(rec.fills(theme.scheme().surface).len(), 1);
        let borders = rec.borders();
        assert_eq!(borders.len(), 1, "only the active page is bordered");
        assert_eq!(borders[0].1, theme.scheme().outline);
        assert_eq!(rec.shadows, 1, "shadow-xs on the outline look only");
        // ...and it is page 2's box.
        let center = item_center(2, 2);
        assert!(
            borders[0]
                .0
                .contains(Point::new(center, style::HEIGHT_DEFAULT / 2.0))
        );
    }

    #[test]
    fn hovering_an_item_washes_accent_and_the_latch_clears_on_leave() {
        let mut h = Harness::new(2);
        h.theme(Brightness::Light);
        let accent = crate::theme()
            .with_brightness(Brightness::Light)
            .scheme()
            .primary_container;
        assert!(h.paint().fills(accent).is_empty(), "no wash at rest");

        let x = item_center(1, 2);
        assert!(h.pointer(PointerPhase::Move, x), "entry repaints");
        assert_eq!(h.paint().fills(accent).len(), 1);
        assert!(!h.pointer(PointerPhase::Move, x + 2.0), "unchanged latch");

        // Off the row: the paint-time read clears the latch.
        h.pointer(PointerPhase::Move, WINDOW.width - 1.0);
        assert!(h.paint().fills(accent).is_empty());
    }

    #[test]
    fn selecting_a_page_reports_it_on_up_inside() {
        let mut h = Harness::new(2);
        let x = item_center(3, 2);
        h.pointer(PointerPhase::Down, x);
        assert!(h.state.selected.is_empty(), "never on down");
        h.pointer(PointerPhase::Up, x);
        assert_eq!(h.state.selected, vec![3]);

        // Release over another item fires nothing.
        h.pointer(PointerPhase::Down, x);
        h.pointer(PointerPhase::Up, item_center(1, 2));
        assert_eq!(h.state.selected, vec![3]);

        // A cancelled press fires nothing.
        h.pointer(PointerPhase::Down, x);
        h.pointer(PointerPhase::Cancel, x);
        assert_eq!(h.state.selected, vec![3]);
    }

    #[test]
    fn previous_and_next_resolve_to_the_neighbouring_pages() {
        let view = nav();
        assert_eq!(view.previous_page(), Some(1));
        assert_eq!(view.next_page(), Some(3));

        let mut h = Harness::new(2);
        h.pointer(PointerPhase::Down, item_center(0, 2));
        h.pointer(PointerPhase::Up, item_center(0, 2));
        assert_eq!(h.state.selected, vec![1], "previous");
        h.pointer(PointerPhase::Down, item_center(5, 2));
        h.pointer(PointerPhase::Up, item_center(5, 2));
        assert_eq!(h.state.selected, vec![1, 3], "next");
    }

    #[test]
    fn an_end_item_with_no_page_is_inert_and_asks_not_allowed() {
        // On page 1 there is no previous page.
        let view: PaginationView<AppState> = pagination(
            [PaginationItem::Page(1), PaginationItem::Page(2)],
            1,
            |_s: &mut AppState, _page| {},
        );
        assert_eq!(view.previous_page(), None);
        assert_eq!(view.next_page(), Some(2));

        let mut h = Harness::new(1);
        let x = item_center(0, 1);
        h.pointer(PointerPhase::Move, x);
        assert_eq!(h.root.cursor(), CursorIcon::NotAllowed);
        h.pointer(PointerPhase::Down, x);
        h.pointer(PointerPhase::Up, x);
        assert!(h.state.selected.is_empty(), "an inert item never fires");
    }

    #[test]
    fn the_ellipsis_is_inert_and_draws_three_dots() {
        let mut h = Harness::new(2);
        let x = item_center(4, 2);
        h.pointer(PointerPhase::Down, x);
        h.pointer(PointerPhase::Up, x);
        assert!(h.state.selected.is_empty());
        // Three circular dots in `muted-foreground`.
        let rec = h.paint();
        let dots: Vec<_> = rec
            .rrects
            .iter()
            .filter(|(_, size, radius, c)| {
                *c == FALLBACK.muted_foreground && *radius == size.width / 2.0
            })
            .collect();
        assert_eq!(dots.len(), 3, "MoreHorizontal is three dots");
    }

    #[test]
    fn a_focused_item_rings_and_activates_on_space_or_enter() {
        let mut h = Harness::new(2);
        h.theme(Brightness::Light);
        assert!(h.paint().rings().is_empty());

        let x = item_center(3, 2);
        h.pointer(PointerPhase::Down, x);
        h.pointer(PointerPhase::Up, x);
        let rings = h.paint().rings();
        assert_eq!(rings.len(), 1, "only the focused item rings");
        assert!(rings[0].contains(Point::new(x, style::HEIGHT_DEFAULT / 2.0)));

        let before = h.state.selected.len();
        h.key(Key::Character(" ".to_string()));
        h.key(Key::Named(frust::authoring::NamedKey::Enter));
        assert_eq!(h.state.selected.len(), before + 2);
        h.key(Key::Named(frust::authoring::NamedKey::Tab));
        assert_eq!(
            h.state.selected.len(),
            before + 2,
            "an unrelated key does nothing"
        );
    }

    #[test]
    fn visit_children_publishes_every_label() {
        let w = build(&nav());
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 5, "previous + three pages + next");
    }

    #[test]
    fn semantics_is_a_navigation_of_links_with_the_current_page_selected() {
        let mut h = Harness::new(2);
        h.pass();
        let update = h.root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Navigation)
        );
        let links: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Link)
            .collect();
        assert_eq!(links.len(), 5, "the ellipsis contributes no node");
        assert_eq!(
            links
                .iter()
                .filter(|(_, n)| n.is_selected() == Some(true))
                .count(),
            1,
            "aria-current=page on exactly one link"
        );
    }
}
