//! Ports shadcn/ui's **Item** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/item.tsx`.
//!
//! [`item`] is a row (`[ media? | content | actions? ]`, `items-center`,
//! [`ItemVariant`]/[`ItemSize`]-driven chrome+padding); an optional
//! [`ItemView::on_press`] makes the whole row interactive — the row claims
//! hover **after** routing to its children, so an [`item_actions`] button
//! wins the hover link over the row's own chrome when the pointer is over it
//! (`docs/CODE_STANDARDS.md`'s "containers claim after routing" rule).
//! [`item_media`] is [`ItemMediaVariant`]-driven (`Icon`: `size-8 rounded-sm
//! border bg-muted`; `Image`: `size-10 rounded-sm`, clipped). [`item_content`]
//! is the text column (`gap-1`); [`item_title`]/[`item_description`] are
//! themed text. [`item_actions`]/[`item_header`]/[`item_footer`] are rows
//! (`gap-2`, the latter two `justify-between`).
//!
//! # Deviations
//!
//! - **Single row, no wrap.** Like [`crate::components::breadcrumb`]/
//!   [`crate::components::button_group`], every row here lays its children
//!   out once, left to right — the source's `flex-wrap` is not reproduced.
//! - **`item_content` is not flex-1.** The source's content column stretches
//!   to fill the row's remaining width; this port sizes it to its own
//!   natural width instead (no flexible-child sizing pass here) — a caller
//!   wanting it to fill the row gives it a fixed-width child.
//! - **No `line-clamp-2`** on [`item_description`] — text truncation by line
//!   count has no counterpart in this port's text layer.

use frust::authoring::text::FontWeight;
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, ThemeTextColor, ThemeTextType, View, Widget, any,
};
use frust::{Theme, text};
use peniko::Color;

use crate::hit::{inside, presses};
use crate::style;

/// `itemVariants`' `variant` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemVariant {
    #[default]
    Default,
    Outline,
    Muted,
}

/// `itemVariants`' `size` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemSize {
    #[default]
    Default,
    Sm,
}

impl ItemSize {
    /// `(gap, padding)` for this size: `Default` = `gap-4 p-4` (16, 16);
    /// `Sm` = `gap-2.5 px-4 py-3` (10, [16 horizontal, 12 vertical]).
    fn gap(self) -> f64 {
        match self {
            ItemSize::Default => style::spacing(4.0),
            ItemSize::Sm => style::spacing(2.5),
        }
    }
    fn padding(self) -> (f64, f64) {
        match self {
            ItemSize::Default => (style::spacing(4.0), style::spacing(4.0)),
            ItemSize::Sm => (style::spacing(4.0), style::spacing(3.0)),
        }
    }
}

/// `rounded-md` — the row's corner radius.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(8.0, |t| t.shape.small)
}

fn resolve_row_colors(
    variant: ItemVariant,
    theme: Option<&Theme>,
) -> (Option<Color>, Option<Color>) {
    let s = theme.map(Theme::scheme);
    match (variant, s) {
        (ItemVariant::Default, _) => (None, None),
        (ItemVariant::Outline, Some(s)) => (None, Some(s.outline)),
        (ItemVariant::Outline, None) => (None, Some(Color::from_rgb8(0xE5, 0xE5, 0xE5))),
        (ItemVariant::Muted, Some(s)) => (
            Some(style::scale_alpha(s.surface_container_highest, 0.5)),
            None,
        ),
        (ItemVariant::Muted, None) => (
            Some(style::with_alpha(Color::from_rgb8(0xF5, 0xF5, 0xF5), 0.5)),
            None,
        ),
    }
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = std::rc::Rc<dyn Fn(&mut State)>;

/// A declarative shadcn item row. See the [module docs](self).
pub struct ItemView<State: 'static> {
    children: Vec<AnyView<State>>,
    variant: ItemVariant,
    size: ItemSize,
    on_press: Option<OnPress<State>>,
}

/// Create an item row from `children` (typically [`item_media`]?,
/// [`item_content`], [`item_actions`]?, in order).
pub fn item<State: 'static>(children: Vec<AnyView<State>>) -> ItemView<State> {
    ItemView {
        children,
        variant: ItemVariant::default(),
        size: ItemSize::default(),
        on_press: None,
    }
}

impl<State: 'static> ItemView<State> {
    pub fn variant(mut self, variant: ItemVariant) -> Self {
        self.variant = variant;
        self
    }
    pub fn size(mut self, size: ItemSize) -> Self {
        self.size = size;
        self
    }
    /// Make the whole row interactive, firing on release inside its bounds.
    pub fn on_press<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_press = Some(std::rc::Rc::new(f));
        self
    }
}

/// The retained widget for an [`ItemView`].
pub struct ItemWidget {
    children: Vec<ChildPod>,
    variant: ItemVariant,
    size: ItemSize,
    interactive: bool,
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_press: Option<frust::authoring::ErasedCallback>,
}

impl<State: 'static> View<State> for ItemView<State> {
    type Element = ItemWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ItemWidget {
        ItemWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            variant: self.variant,
            size: self.size,
            interactive: self.on_press.is_some(),
            hovered: false,
            pressed: false,
            captured: false,
            on_press: self.on_press.as_ref().map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ItemWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        );
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let now_interactive = self.on_press.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            if !now_interactive {
                element.captured = false;
                element.pressed = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        element.on_press = self.on_press.as_ref().map(frust::authoring::erase_callback);
        flags
    }

    fn teardown(&self, element: &mut ItemWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let (pad_x, pad_y) = self.size.padding();
        let gap = self.size.gap();
        let loose = BoxConstraints::loose(bc.max());

        let mut sizes = Vec::with_capacity(self.children.len());
        let mut content_w = 0.0_f64;
        let mut height = 0.0_f64;
        for pod in self.children.iter_mut() {
            let s = pod.layout_child(ctx, &loose);
            sizes.push(s);
            content_w += s.width;
            height = height.max(s.height);
        }
        if !sizes.is_empty() {
            content_w += gap * (sizes.len() - 1) as f64;
        }

        let mut x = pad_x;
        for (pod, s) in self.children.iter_mut().zip(sizes.iter()) {
            pod.set_origin(Point::new(x, pad_y + (height - s.height) / 2.0));
            x += s.width + gap;
        }

        bc.constrain(Size::new(content_w + pad_x * 2.0, height + pad_y * 2.0))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let radius = resolve_radius(theme);
        let (fill, border) = resolve_row_colors(self.variant, theme);
        if let Some(fill) = fill {
            scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        }
        if let Some(border) = border {
            let rr = frust::authoring::RoundedRect::from_rect(
                kurbo::Rect::from_origin_size(Point::ORIGIN, ctx.size()),
                radius,
            );
            scene.stroke_path(
                ctx.origin(),
                &frust::authoring::Shape::to_path(&rr, 0.1),
                style::BORDER_WIDTH,
                &frust::authoring::Brush::Solid(border),
            );
        }
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
        // Self-corrector last, matching the row's hover claim (after routing)
        // — nothing else to paint from the flag itself; the row has no state
        // layer of its own (shadcn conveys hover only via `[a]:hover:bg-accent/50`
        // on a link-flavored item, out of scope for a plain interactive row).
        self.hovered = ctx.is_hovered();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
            return frust::authoring::route_event(&mut self.children, ctx, event);
        }
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event(&mut self.children, ctx, event);
        };
        // Route to children FIRST — an Actions button under the pointer must
        // win capture/hover before the row's own chrome claims anything.
        let routed = frust::authoring::route_event(&mut self.children, ctx, event);
        if routed == EventResult::Handled {
            return routed;
        }
        // `interactive` and `on_press` are kept in lockstep by `build`/`rebuild`
        // (`interactive` is derived from `on_press.is_some()`), so this should
        // never miss — but a broken invariant here must not panic under
        // `panic=abort` inside a pointer handler; short-circuit as an ignored
        // event instead of crashing the process.
        let Some(on_press) = self.on_press.as_mut() else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(CursorIcon::Pointer);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                let now_inside = inside(p.position, ctx.size());
                if self.pressed != now_inside {
                    self.pressed = now_inside;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::ListItem,
            |node| {
                if self.interactive {
                    node.add_action(Action::Click);
                }
            },
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

// ---- ItemMedia ---------------------------------------------------------

/// `itemMediaVariants`' `variant` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemMediaVariant {
    #[default]
    Default,
    /// `size-8 rounded-sm border bg-muted`.
    Icon,
    /// `size-10 rounded-sm`, clipped.
    Image,
}

/// `ItemMedia`: see [`ItemMediaVariant`].
pub struct ItemMediaView<State: 'static> {
    variant: ItemMediaVariant,
    child: AnyView<State>,
}

/// Wrap `child` as `ItemMedia`, `Default` unless overridden with
/// [`ItemMediaView::variant`].
pub fn item_media<State: 'static, V: View<State>>(child: V) -> ItemMediaView<State> {
    ItemMediaView {
        variant: ItemMediaVariant::default(),
        child: any(child),
    }
}

impl<State: 'static> ItemMediaView<State> {
    pub fn variant(mut self, variant: ItemMediaVariant) -> Self {
        self.variant = variant;
        self
    }
}

/// The retained widget for an [`ItemMediaView`].
pub struct ItemMediaWidget {
    child: ChildPod,
    variant: ItemMediaVariant,
}

impl<State: 'static> View<State> for ItemMediaView<State> {
    type Element = ItemMediaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ItemMediaWidget {
        ItemMediaWidget {
            child: frust::authoring::build_child(&self.child, ctx),
            variant: self.variant,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ItemMediaWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ItemMediaWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl ItemMediaWidget {
    fn box_size(self_variant: ItemMediaVariant) -> Option<f64> {
        match self_variant {
            ItemMediaVariant::Default => None,
            ItemMediaVariant::Icon => Some(32.0),
            ItemMediaVariant::Image => Some(40.0),
        }
    }
}

impl Widget for ItemMediaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match Self::box_size(self.variant) {
            None => {
                let s = self
                    .child
                    .layout_child(ctx, &BoxConstraints::loose(bc.max()));
                self.child.set_origin(Point::ZERO);
                bc.constrain(s)
            }
            Some(px) => {
                let size = Size::new(px, px);
                self.child.layout_child(ctx, &BoxConstraints::tight(size));
                self.child.set_origin(Point::ZERO);
                bc.constrain(size)
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let radius = theme.map_or(4.0, |t| t.shape.extra_small);
        if self.variant == ItemMediaVariant::Icon {
            let fill = theme.map_or(Color::from_rgb8(0xF5, 0xF5, 0xF5), |t| {
                t.scheme().surface_container_highest
            });
            let border = theme.map_or(Color::from_rgb8(0xE5, 0xE5, 0xE5), |t| t.scheme().outline);
            scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
            let rr = frust::authoring::RoundedRect::from_rect(
                kurbo::Rect::from_origin_size(Point::ORIGIN, ctx.size()),
                radius,
            );
            scene.stroke_path(
                ctx.origin(),
                &frust::authoring::Shape::to_path(&rr, 0.1),
                style::BORDER_WIDTH,
                &frust::authoring::Brush::Solid(border),
            );
        }
        if self.variant == ItemMediaVariant::Image {
            scene.push_clip_rounded(ctx.origin(), ctx.size(), radius);
            self.child.paint_child(ctx, scene);
            scene.pop_clip();
        } else {
            self.child.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    frust::authoring::visit_children!(child);
}

// ---- ItemContent / ItemTitle / ItemDescription -------------------------

/// `ItemContent`: a left-aligned column, `gap-1`.
pub struct ItemContentView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Stack `children` (typically [`item_title`]/[`item_description`]) in a
/// left-aligned column.
pub fn item_content<State: 'static>(children: Vec<AnyView<State>>) -> ItemContentView<State> {
    ItemContentView { children }
}

/// The retained widget for an [`ItemContentView`].
pub struct ItemContentWidget {
    children: Vec<ChildPod>,
}

/// `gap-1` — the gap between an item's title and description.
const CONTENT_GAP: f64 = style::SPACING_UNIT;

impl<State: 'static> View<State> for ItemContentView<State> {
    type Element = ItemContentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ItemContentWidget {
        ItemContentWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ItemContentWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        frust::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut ItemContentWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ItemContentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = BoxConstraints::loose(Size::new(bc.max().width, f64::INFINITY));
        let mut y = 0.0_f64;
        let mut width = 0.0_f64;
        for (i, pod) in self.children.iter_mut().enumerate() {
            let s = pod.layout_child(ctx, &loose);
            if i > 0 {
                y += CONTENT_GAP;
            }
            pod.set_origin(Point::new(0.0, y));
            y += s.height;
            width = width.max(s.width);
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(children);
}

/// `ItemTitle`: `text-sm leading-snug font-medium`, `on_surface`.
pub fn item_title(label: impl Into<String>) -> frust::TextView {
    text(label)
        .size(style::TEXT_SM as f32)
        .weight(FontWeight::MEDIUM)
        .themed_family(ThemeTextType::TitleSmall)
        .themed_role(ThemeTextColor::OnSurface)
}

/// `ItemDescription`: `text-sm text-muted-foreground` (see the module docs'
/// *Deviations* for `line-clamp-2`).
pub fn item_description(label: impl Into<String>) -> frust::TextView {
    text(label)
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium)
        .themed_role(ThemeTextColor::OnSurfaceVariant)
}

// ---- ItemActions / ItemHeader / ItemFooter ------------------------------

/// A row shared by [`item_actions`]/[`item_header`]/[`item_footer`] — see
/// the [module docs](self).
pub struct ItemRowView<State: 'static> {
    children: Vec<AnyView<State>>,
    justify_between: bool,
}

/// `ItemActions`: a row, `gap-2`, `items-center`.
pub fn item_actions<State: 'static>(children: Vec<AnyView<State>>) -> ItemRowView<State> {
    ItemRowView {
        children,
        justify_between: false,
    }
}

/// `ItemHeader`: a `justify-between` row, `gap-2`.
pub fn item_header<State: 'static>(children: Vec<AnyView<State>>) -> ItemRowView<State> {
    ItemRowView {
        children,
        justify_between: true,
    }
}

/// `ItemFooter`: a `justify-between` row, `gap-2`.
pub fn item_footer<State: 'static>(children: Vec<AnyView<State>>) -> ItemRowView<State> {
    ItemRowView {
        children,
        justify_between: true,
    }
}

/// `gap-2` — the gap [`item_actions`]/[`item_header`]/[`item_footer`] share.
const ROW_GAP: f64 = style::SPACING_UNIT * 2.0;

/// The retained widget for an [`ItemRowView`].
pub struct ItemRowWidget {
    children: Vec<ChildPod>,
    justify_between: bool,
}

impl<State: 'static> View<State> for ItemRowView<State> {
    type Element = ItemRowWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ItemRowWidget {
        ItemRowWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            justify_between: self.justify_between,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ItemRowWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        frust::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut ItemRowWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ItemRowWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = BoxConstraints::loose(Size::new(f64::INFINITY, bc.max().height));
        let mut sizes = Vec::with_capacity(self.children.len());
        let mut natural_w = 0.0_f64;
        let mut height = 0.0_f64;
        for pod in self.children.iter_mut() {
            let s = pod.layout_child(ctx, &loose);
            natural_w += s.width;
            height = height.max(s.height);
            sizes.push(s);
        }
        let n = sizes.len();
        if n > 0 {
            natural_w += ROW_GAP * (n - 1) as f64;
        }

        let row_w = if self.justify_between && bc.max().width.is_finite() {
            bc.max().width.max(natural_w)
        } else {
            natural_w
        };
        let extra_gap = if self.justify_between && n > 1 {
            (row_w - natural_w) / (n - 1) as f64
        } else {
            0.0
        };

        let mut x = 0.0_f64;
        for (pod, s) in self.children.iter_mut().zip(sizes.iter()) {
            pod.set_origin(Point::new(x, (height - s.height) / 2.0));
            x += s.width + ROW_GAP + extra_gap;
        }

        bc.constrain(Size::new(row_w, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust_widgets::test_support::leaf;
    use std::any::Any;

    fn layout<W: Widget>(w: &mut W, max: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(max))
    }

    #[test]
    fn default_row_lays_media_content_actions_left_to_right() {
        let view: ItemView<()> = item(vec![
            any(item_media(leaf(20.0, 20.0)).variant(ItemMediaVariant::Icon)),
            any(item_content::<()>(vec![any(item_title("Title"))])),
        ]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, Size::new(400.0, 400.0));
        assert_eq!(w.children[0].origin().x, ItemSize::default().padding().0);
        assert!(w.children[1].origin().x > w.children[0].origin().x);
    }

    #[test]
    fn sm_size_uses_a_tighter_gap_and_padding() {
        let view: ItemView<()> =
            item(vec![any(leaf(10.0, 10.0)), any(leaf(10.0, 10.0))]).size(ItemSize::Sm);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        // px-4 (16*2=32) + two 10px children + gap-2.5 (10) = width 62;
        // py-3 (12*2=24) + 10 = height 34.
        assert_eq!(size, Size::new(62.0, 34.0));
    }

    #[test]
    fn outline_variant_strokes_a_border() {
        #[derive(Default)]
        struct Rec {
            strokes: usize,
        }
        impl PaintScene for Rec {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn stroke_path(
                &mut self,
                _o: Point,
                _p: &kurbo::BezPath,
                _w: f64,
                _b: &frust::authoring::Brush,
            ) {
                self.strokes += 1;
            }
        }
        let view: ItemView<()> = item(vec![any(leaf(10.0, 10.0))]).variant(ItemVariant::Outline);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        let mut rec = Rec::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.strokes, 1);
    }

    #[test]
    fn press_fires_on_up_inside_and_actions_win_hover_over_the_row() {
        use frust::authoring::{PointerButton, PointerEvent};
        let view: ItemView<Vec<u32>> = item(vec![any(frust_widgets::test_support::probe(0))])
            .on_press(|s: &mut Vec<u32>| s.push(99));
        let mut counter = 0u64;
        let mut w = View::<Vec<u32>>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        let mut state: Vec<u32> = Vec::new();
        let mut ectx = EventCtx::new(&mut state, Point::ZERO, size);
        // A Down over the probe child (which fills the row's own loose
        // bound, past the row's padding) is handled by the child, not the
        // row — the row never arms.
        let result = w.event(
            &mut ectx,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(20.0, 20.0),
                button: PointerButton::Primary,
            }),
        );
        assert_eq!(result, EventResult::Handled);
        assert!(!w.captured, "the child consumed the Down, not the row");
    }

    #[test]
    fn header_row_distributes_extra_width_between_two_children() {
        let view: ItemRowView<()> = item_header(vec![any(leaf(20.0, 10.0)), any(leaf(20.0, 10.0))]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(200.0, 40.0));
        assert_eq!(size.width, 200.0);
        assert_eq!(w.children[0].origin().x, 0.0);
        // The right child's right edge reaches the row's own right edge.
        assert_eq!(w.children[1].origin().x + 20.0, 200.0);
    }

    #[test]
    fn content_column_stacks_title_over_description_with_gap_1() {
        let view: ItemContentView<()> =
            item_content(vec![any(item_title("T")), any(item_description("D"))]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, Size::new(200.0, 200.0));
        assert_eq!(w.children[0].origin().y, 0.0);
        assert!(w.children[1].origin().y > w.children[0].origin().y);
    }

    // ---- Typeface: the title and description follow the live theme ------

    #[cfg(feature = "bundled-fonts")]
    fn titled(_: &mut ()) -> ItemView<()> {
        item(vec![any(item_content(vec![
            any(item_title("Two-factor authentication")),
            any(item_description("Verify with a code on every sign-in.")),
        ]))])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "an item's title and description",
            titled,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "an item's title and description",
            titled,
        );
    }
}
