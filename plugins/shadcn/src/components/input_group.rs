//! `input_group`: the container that glues a text control to addons behind one
//! shared border.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/input-group.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17). The group
//! itself is `relative flex w-full items-center rounded-md border border-input
//! shadow-xs h-9 has-[>textarea]:h-auto`, plus four alignment blocks
//! (`has-[>[data-align=block-start]]:h-auto …:flex-col`) and two state blocks
//! that are the *same* border/ring treatment `input.tsx` carries — which is why
//! this module paints its chrome through [`crate::components::input`]'s
//! [`resolve_field_border`]/[`paint_field_border`] rather than restating it.
//!
//! # Group focus: read, not guessed
//!
//! Upstream rings the *group* when the inner control has focus
//! (`has-[[data-slot=input-group-control]:focus-visible]:border-ring …:ring-[3px]`).
//! That reads directly here: focus is a recorded **path**, and
//! [`PaintCtx::has_focus`](frust::authoring::PaintCtx::has_focus) reports path
//! membership, so a group whose control holds focus reads `true` with no
//! child-state plumbing at all — the same way
//! [`PaintCtx::is_hovered`](frust::authoring::PaintCtx::is_hovered) reports the
//! hover path.
//!
//! # The control, and what "flush" means
//!
//! The group paints the border, so the control inside it must not: pass
//! [`crate::input`]`(…).flush(true)` (or [`crate::textarea`]`(…).flush(true)`),
//! the port of upstream's `InputGroupInput`/`InputGroupTextarea`
//! (`border-0 shadow-none focus-visible:ring-0`). Any other view works as the
//! control too — the group only lays out and rings whatever it is handed.
//!
//! **One gap:** upstream's addon has an `onClick` that focuses the inner input.
//! Focus is requested by a widget for *itself* and bubbles up as a path, so a
//! parent has no seam to focus a child pod on its behalf; clicking an addon here
//! therefore does not move focus into the control. The addon still shows the
//! `Text` cursor upstream's `cursor-text` asks for, so the affordance points at
//! the field rather than lying about a button.

use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, ErasedCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, ThemeTextColor, ThemeTextType, View, Widget, any, build_child,
    erase_callback, rebuild_child, route_event, route_event_single, teardown_child, visit_children,
};
use frust::{TextView, Theme, text};

use crate::components::input::{FALLBACK, FieldChrome, paint_field_border, resolve_field_border};
use crate::components::native_select::activates;
use crate::hit::{inside, presses};
use crate::style;
use crate::tokens::ShadcnTokens;

/// The offset upstream's `rounded-[calc(var(--radius)-5px)]` subtracts from the
/// base radius (`--radius` is [`crate::RADIUS_BASE`]).
const TIGHT_RADIUS_OFFSET: f64 = 5.0;

/// Alpha of the dark-mode ghost hover fill: `dark:hover:bg-accent/50`.
const DARK_ACCENT_ALPHA: f32 = 0.5;

/// Where an addon sits relative to the control — upstream's `data-align`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputGroupAlign {
    /// `order-first pl-3` — leading, on the control's row.
    #[default]
    InlineStart,
    /// `order-last pr-3` — trailing, on the control's row.
    InlineEnd,
    /// `order-first w-full px-3 pt-3` — a full-width row above the control (and
    /// `h-auto` on the group).
    BlockStart,
    /// `order-last w-full px-3 pb-3` — a full-width row below the control.
    BlockEnd,
}

impl InputGroupAlign {
    /// Whether this alignment puts the addon on its own full-width row.
    pub fn is_block(self) -> bool {
        matches!(
            self,
            InputGroupAlign::BlockStart | InputGroupAlign::BlockEnd
        )
    }

    /// The paint/DOM order upstream's `order-first`/`order-last` classes impose:
    /// block-start, inline-start, inline-end, block-end.
    fn order(self) -> u8 {
        match self {
            InputGroupAlign::BlockStart => 0,
            InputGroupAlign::InlineStart => 1,
            InputGroupAlign::InlineEnd => 2,
            InputGroupAlign::BlockEnd => 3,
        }
    }
}

/// The size ladder upstream's `InputGroupButton` overrides `Button` with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputGroupButtonSize {
    /// `h-6 px-2 rounded-[calc(var(--radius)-5px)]` — the default.
    #[default]
    Xs,
    /// `h-8 px-2.5 rounded-md`.
    Sm,
    /// `size-6 p-0 rounded-[calc(var(--radius)-5px)]`.
    IconXs,
    /// `size-8 p-0 rounded-md`.
    IconSm,
}

impl InputGroupButtonSize {
    /// This size's height, in logical px.
    pub fn height(self) -> f64 {
        match self {
            InputGroupButtonSize::Xs | InputGroupButtonSize::IconXs => style::HEIGHT_XS,
            InputGroupButtonSize::Sm | InputGroupButtonSize::IconSm => style::HEIGHT_SM,
        }
    }

    /// This size's horizontal padding (zero for the icon sizes, which are
    /// squares).
    pub fn padding_x(self) -> f64 {
        match self {
            InputGroupButtonSize::Xs => style::spacing(2.0),
            InputGroupButtonSize::Sm => style::spacing(2.5),
            InputGroupButtonSize::IconXs | InputGroupButtonSize::IconSm => 0.0,
        }
    }

    /// Whether this size is a square icon button (`size-6`/`size-8`).
    pub fn is_icon(self) -> bool {
        matches!(
            self,
            InputGroupButtonSize::IconXs | InputGroupButtonSize::IconSm
        )
    }

    /// This size's corner radius: `rounded-md`, or the tighter
    /// `calc(var(--radius)-5px)` the two `xs` rungs ask for.
    pub fn radius(self, theme: Option<&Theme>) -> f64 {
        let scale = ShadcnTokens::resolve_radius(None, theme);
        match self {
            InputGroupButtonSize::Xs | InputGroupButtonSize::IconXs => {
                (scale.lg - TIGHT_RADIUS_OFFSET).max(0.0)
            }
            InputGroupButtonSize::Sm | InputGroupButtonSize::IconSm => scale.md,
        }
    }
}

/// A declarative input group. See the [module docs](self).
pub struct InputGroupView<State: 'static> {
    control: AnyView<State>,
    addons: Vec<(InputGroupAlign, AnyView<State>)>,
    invalid: bool,
    disabled: bool,
}

/// Wrap `control` in an input group; add addons with
/// [`addon`](InputGroupView::addon).
pub fn input_group<State: 'static, V: View<State>>(control: V) -> InputGroupView<State> {
    InputGroupView {
        control: any(control),
        addons: Vec::new(),
        invalid: false,
        disabled: false,
    }
}

impl<State: 'static> InputGroupView<State> {
    /// Add an addon at `align`. Addons keep the order they are added in within
    /// each alignment.
    pub fn addon<V: View<State>>(mut self, align: InputGroupAlign, addon: V) -> Self {
        self.addons.push((align, any(addon)));
        self
    }

    /// Put the group in its `aria-invalid` state (the whole group's border, not
    /// just the control's).
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Dim the group's chrome and refuse the `Text` cursor. The control and any
    /// interactive addon are disabled through their own builders — upstream's
    /// `group-data-[disabled=true]` cascade has no analog here, and dimming a
    /// child a second time from the container would double-dim what it already
    /// dims itself.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Addon indices in upstream's `order-*` sequence (a stable sort, so addons
    /// added at the same alignment keep their relative order).
    fn ordered(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.addons.len()).collect();
        order.sort_by_key(|i| self.addons[*i].0.order());
        order
    }
}

/// A muted, `text-sm` label for an addon — upstream's `InputGroupText`.
pub fn input_group_text(label: impl Into<String>) -> TextView {
    text(label.into())
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium)
        .themed_role(ThemeTextColor::OnSurfaceVariant)
}

/// The retained widget for an [`InputGroupView`].
///
/// `children[0]` is the control and `children[1..]` are the addons, in the
/// `aligns`-parallel `order-*` sequence — one flat pod list so the whole group
/// routes through [`route_event`] (which hit-tests in reverse paint order, i.e.
/// addons before the control) rather than a hand-rolled dispatch.
pub struct InputGroupWidget {
    children: Vec<ChildPod>,
    aligns: Vec<InputGroupAlign>,
    invalid: bool,
    disabled: bool,
}

#[cfg(test)]
impl InputGroupWidget {
    /// The control's pod — the flat-list convention `children`'s doc records,
    /// named so the layout assertions read as geometry rather than indexing.
    fn control(&self) -> &ChildPod {
        &self.children[0]
    }

    /// Addon `index`'s pod, in `aligns` order.
    fn addon(&self, index: usize) -> &ChildPod {
        &self.children[index + 1]
    }
}

impl<State: 'static> View<State> for InputGroupView<State> {
    type Element = InputGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> InputGroupWidget {
        let order = self.ordered();
        let mut children = vec![build_child(&self.control, ctx)];
        for index in &order {
            children.push(build_child(&self.addons[*index].1, ctx));
        }
        InputGroupWidget {
            children,
            aligns: order.iter().map(|i| self.addons[*i].0).collect(),
            invalid: self.invalid,
            disabled: self.disabled,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut InputGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.control, &self.control, &mut element.children[0], ctx);
        let (prev_order, order) = (prev.ordered(), self.ordered());
        let same_shape = prev_order.len() == order.len()
            && order
                .iter()
                .zip(&prev_order)
                .all(|(n, p)| self.addons[*n].0 == prev.addons[*p].0);
        if same_shape {
            for ((pod, n), p) in element.children[1..]
                .iter_mut()
                .zip(&order)
                .zip(&prev_order)
            {
                flags |= rebuild_child(&prev.addons[*p].1, &self.addons[*n].1, pod, ctx);
            }
        } else {
            // The addon shape changed: tear the old set down against the views it
            // was built from, then rebuild fresh.
            for (pod, p) in element.children[1..].iter_mut().zip(&prev_order) {
                teardown_child(&prev.addons[*p].1, pod, ctx);
            }
            element.children.truncate(1);
            for index in &order {
                element
                    .children
                    .push(build_child(&self.addons[*index].1, ctx));
            }
            element.aligns = order.iter().map(|i| self.addons[*i].0).collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.invalid != self.invalid || element.disabled != self.disabled {
            element.invalid = self.invalid;
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut InputGroupWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.control, &mut element.children[0], ctx);
        for (pod, index) in element.children[1..].iter_mut().zip(self.ordered()) {
            teardown_child(&self.addons[index].1, pod, ctx);
        }
    }
}

impl Widget for InputGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // `px-3` on every edge an addon touches; `py-1.5` inside an inline addon.
        let pad = style::spacing(3.0);
        let inline_pad_y = style::spacing(1.5);
        let block_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new((width - 2.0 * pad).max(0.0), f64::INFINITY),
        );
        let inline_bc = BoxConstraints::new(Size::ZERO, Size::new(width, f64::INFINITY));

        // Block-start addons: full-width rows above the control.
        let mut top = 0.0;
        for index in 0..self.aligns.len() {
            if self.aligns[index] != InputGroupAlign::BlockStart {
                continue;
            }
            let size = self.children[index + 1].layout_child(ctx, &block_bc);
            self.children[index + 1].set_origin(Point::new(pad, top + pad));
            top += size.height + pad;
        }

        // Measure the inline addons to learn how much width is left for the
        // control, and how tall the row has to be.
        let mut lead = 0.0;
        let mut trail = 0.0;
        let mut row_height: f64 = 0.0;
        for index in 0..self.aligns.len() {
            let size = match self.aligns[index] {
                InputGroupAlign::InlineStart | InputGroupAlign::InlineEnd => {
                    self.children[index + 1].layout_child(ctx, &inline_bc)
                }
                _ => continue,
            };
            match self.aligns[index] {
                InputGroupAlign::InlineStart => lead += pad + size.width,
                _ => trail += pad + size.width,
            }
            row_height = row_height.max(size.height + 2.0 * inline_pad_y);
        }

        // The control is `flex-1` in an `items-center` row: measure it, then
        // stretch it to the row so no seam shows between it and the addons.
        let control_width = (width - lead - trail).max(0.0);
        let natural = self.children[0].layout_child(
            ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(control_width, f64::INFINITY)),
        );
        row_height = row_height.max(natural.height);
        if !self.aligns.iter().any(|a| a.is_block()) {
            // `h-9`, unless a block addon (or a taller control) forced `h-auto`.
            row_height = row_height.max(style::HEIGHT_DEFAULT);
        }
        self.children[0].layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(control_width, row_height)),
        );
        self.children[0].set_origin(Point::new(lead, top));

        // Place the inline addons, vertically centered in the row.
        let mut x_lead = 0.0;
        let mut x_trail = width;
        for index in 0..self.aligns.len() {
            let size = self.children[index + 1].size();
            let y = top + ((row_height - size.height) / 2.0).max(0.0);
            match self.aligns[index] {
                InputGroupAlign::InlineStart => {
                    self.children[index + 1].set_origin(Point::new(x_lead + pad, y));
                    x_lead += pad + size.width;
                }
                InputGroupAlign::InlineEnd => {
                    x_trail -= pad + size.width;
                    self.children[index + 1].set_origin(Point::new(x_trail + pad, y));
                }
                _ => {}
            }
        }

        // Block-end addons below the row.
        let mut bottom = top + row_height;
        for index in 0..self.aligns.len() {
            if self.aligns[index] != InputGroupAlign::BlockEnd {
                continue;
            }
            let size = self.children[index + 1].layout_child(ctx, &block_bc);
            self.children[index + 1].set_origin(Point::new(pad, bottom));
            bottom += size.height + pad;
        }

        bc.constrain(Size::new(width, bottom))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The group's focus is the *control's* focus, read off the recorded focus
        // path rather than plumbed out of the child (see the module docs).
        let chrome = FieldChrome {
            focused: ctx.has_focus() && !self.disabled,
            invalid: self.invalid,
            disabled: self.disabled,
        };
        let (origin, size) = (ctx.origin(), ctx.size());
        let theme = Theme::from_paint_ctx(ctx);
        let resolved = resolve_field_border(theme, chrome);
        style::draw_shadow(
            scene,
            origin,
            size,
            resolved.radius,
            style::SHADOW_XS,
            theme,
        );
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
        // Last, so the one shared border sits over every child's own fill — this
        // is what "flush" looks like: the children have no edges of their own.
        paint_field_border(scene, origin, size, resolved);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // `cursor-text` on the group, asked for *before* routing so an interactive
        // addon (a button asking for `Pointer`) still speaks last and wins — the
        // container is the fallback here, not the override.
        if let InputEvent::Pointer(p) = event
            && matches!(p.phase, PointerPhase::Move)
            && inside(p.position, ctx.size())
        {
            ctx.set_cursor(if self.disabled {
                style::DISABLED_CURSOR
            } else {
                CursorIcon::Text
            });
        }
        route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="group"` upstream; every child forwards, or its subtree drops out
        // of the accessibility tree.
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(children);
}

/// A declarative addon button. See [`input_group_button`].
pub struct InputGroupButtonView<State: 'static> {
    label: String,
    size: InputGroupButtonSize,
    disabled: bool,
    on_click: Rc<dyn Fn(&mut State)>,
}

/// A ghost-look button sized for an addon slot — upstream's `InputGroupButton`
/// (a `Button variant="ghost"` with its own size ladder and `shadow-none`).
///
/// The content is a text `label`, including for the two icon sizes (a square slot
/// for a short glyph). An addon takes an arbitrary view, so a button hosting real
/// icon artwork is an app-side composition rather than a knob here.
///
/// **One gap:** upstream's ghost hover swaps the *text* color too
/// (`hover:text-accent-foreground`). Text color is baked into the shaped layout
/// at layout time, so a paint-time hover cannot move it; only the background
/// swaps. In shadcn's monochrome presets the two inks differ by one step of the
/// neutral ramp.
pub fn input_group_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_click: F,
) -> InputGroupButtonView<State> {
    InputGroupButtonView {
        label: label.into(),
        size: InputGroupButtonSize::default(),
        disabled: false,
        on_click: Rc::new(on_click),
    }
}

impl<State: 'static> InputGroupButtonView<State> {
    /// Set the button's size rung.
    pub fn size(mut self, size: InputGroupButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Disable the button: inert, dimmed, and — unlike upstream's
    /// `disabled:pointer-events-none` button — still asking for
    /// [`style::DISABLED_CURSOR`], since a control inside a text field's chrome
    /// should say why it cannot be pressed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The label child: `text-sm` in `foreground`.
    fn label_view(&self) -> AnyView<State> {
        any(text(self.label.clone())
            .size(style::TEXT_SM as f32)
            .themed_family(ThemeTextType::LabelLarge)
            .themed_role(ThemeTextColor::OnSurface))
    }
}

/// The retained widget for an [`InputGroupButtonView`].
pub struct InputGroupButtonWidget {
    label: ChildPod,
    size: InputGroupButtonSize,
    disabled: bool,
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_click: ErasedCallback,
}

impl<State: 'static> View<State> for InputGroupButtonView<State> {
    type Element = InputGroupButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> InputGroupButtonWidget {
        InputGroupButtonWidget {
            label: build_child(&self.label_view(), ctx),
            size: self.size,
            disabled: self.disabled,
            hovered: false,
            pressed: false,
            captured: false,
            on_click: erase_callback(&self.on_click),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut InputGroupButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.label_view(),
            &self.label_view(),
            &mut element.label,
            ctx,
        );
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        element.on_click = erase_callback(&self.on_click);
        flags
    }

    fn teardown(&self, element: &mut InputGroupButtonWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.label_view(), &mut element.label, ctx);
    }
}

impl Widget for InputGroupButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let height = self.size.height();
        let label = self.label.layout_child(
            ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(bc.max().width, height)),
        );
        let width = if self.size.is_icon() {
            height
        } else {
            label.width + 2.0 * self.size.padding_x()
        };
        self.label.set_origin(Point::new(
            ((width - label.width) / 2.0).max(0.0),
            ((height - label.height) / 2.0).max(0.0),
        ));
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: it is what drops the wash on the frame the
        // pointer moves onto something else.
        self.hovered = ctx.is_hovered();
        let theme = Theme::from_paint_ctx(ctx);
        if self.hovered && !self.disabled {
            // `hover:bg-accent`, halved in dark mode (`dark:hover:bg-accent/50`).
            let accent = theme.map_or(FALLBACK.accent, |t| t.scheme().primary_container);
            let fill = if style::is_dark(theme) {
                style::with_alpha(accent, DARK_ACCENT_ALPHA)
            } else {
                accent
            };
            let radius = self.size.radius(theme);
            scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        }
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let routed = route_event_single(&mut self.label, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        if let InputEvent::Key(key) = event {
            if self.disabled || !activates(&key.key) {
                return EventResult::Ignored;
            }
            (self.on_click)(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        if self.disabled {
            if matches!(p.phase, PointerPhase::Move) && inside(p.position, ctx.size()) {
                ctx.set_cursor(style::DISABLED_CURSOR);
            }
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let over = inside(p.position, ctx.size());
                if !self.captured {
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                if self.pressed != over {
                    self.pressed = over;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                if inside(p.position, ctx.size()) {
                    (self.on_click)(ctx);
                }
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // A Cancel arm never touches app state — flags and a redraw only.
                self.captured = false;
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Button,
            |node| {
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.label.semantics_child(ctx),
        );
    }

    visit_children!(label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Brush, Color, Key, PointerButton, PointerEvent, Rect, Shape};
    use frust::{Brightness, FrameTime};
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
        fn border(&self) -> (Rect, f64, Color) {
            *self
                .strokes
                .iter()
                .find(|(_, w, _)| *w == style::BORDER_WIDTH)
                .expect("border")
        }

        fn borders(&self) -> usize {
            self.strokes
                .iter()
                .filter(|(_, w, _)| *w == style::BORDER_WIDTH)
                .count()
        }

        fn ring(&self) -> Option<Color> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .map(|(_, _, c)| *c)
        }

        fn has_fill(&self, color: Color) -> bool {
            self.rrects.iter().any(|(_, _, _, c)| *c == color)
        }
    }

    const WINDOW: Size = Size::new(300.0, 200.0);

    #[derive(Default)]
    struct AppState {
        text: String,
        clicks: u32,
    }

    /// A group holding a flush input plus the addons under test, driven through a
    /// real `RenderRoot` — the only harness where the group's *path* focus read
    /// (its whole point) is observable.
    struct Harness {
        root: RenderRoot<AppState, InputGroupView<AppState>>,
        state: AppState,
        tcx: TextContext,
        trailing_button: bool,
        block: bool,
    }

    impl Harness {
        fn new(trailing_button: bool, block: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                trailing_button,
                block,
            };
            h.pass();
            h
        }

        fn theme(&mut self, brightness: Brightness) {
            self.root
                .set_theme(Box::new(crate::theme().with_brightness(brightness)));
            self.pass();
        }

        fn pass(&mut self) -> Size {
            let (trailing_button, block) = (self.trailing_button, self.block);
            let mut logic = move |state: &mut AppState| {
                let mut group = input_group::<AppState, _>(
                    crate::input(state.text.clone(), |s: &mut AppState, t| s.text = t).flush(true),
                )
                .addon(InputGroupAlign::InlineStart, input_group_text("https://"));
                if trailing_button {
                    group = group.addon(
                        InputGroupAlign::InlineEnd,
                        input_group_button("Go", |s: &mut AppState| s.clicks += 1),
                    );
                }
                if block {
                    group = group.addon(InputGroupAlign::BlockEnd, input_group_text("optional"));
                }
                group
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
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

    fn build<S: 'static>(view: &InputGroupView<S>) -> InputGroupWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut InputGroupWidget, size: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    /// A flush field plus a leading text addon and a trailing button.
    fn group() -> InputGroupView<AppState> {
        input_group(crate::input("", |_s: &mut AppState, _t: String| {}).flush(true))
            .addon(InputGroupAlign::InlineStart, input_group_text("https://"))
            .addon(
                InputGroupAlign::InlineEnd,
                input_group_button::<AppState, _>("Go", |_s| {}),
            )
    }

    #[test]
    fn an_inline_group_is_h9_and_the_control_is_flush_between_the_addons() {
        let mut w = build(&group());
        let size = layout(&mut w, WINDOW);
        assert_eq!(size, Size::new(WINDOW.width, style::HEIGHT_DEFAULT));
        assert_eq!(
            w.aligns,
            vec![InputGroupAlign::InlineStart, InputGroupAlign::InlineEnd]
        );
        assert_eq!(w.addon(0).origin().x, style::spacing(3.0), "pl-3");
        // Flush: the control starts exactly where the leading addon ends and
        // stretches to the row height, so no seam shows between them.
        assert_eq!(
            w.control().origin().x,
            style::spacing(3.0) + w.addon(0).size().width
        );
        assert_eq!(w.control().size().height, style::HEIGHT_DEFAULT);
        assert_eq!(
            w.control().origin().x + w.control().size().width + style::spacing(3.0),
            w.addon(1).origin().x,
            "the trailing addon abuts the control's far edge"
        );
        // ...and the whole row stays inside the group.
        assert!(w.addon(1).origin().x + w.addon(1).size().width <= size.width);
        // The inline addons are vertically centered in the row.
        let addon_h = w.addon(1).size().height;
        assert_eq!(
            w.addon(1).origin().y,
            (style::HEIGHT_DEFAULT - addon_h) / 2.0
        );
    }

    #[test]
    fn a_block_addon_makes_the_group_auto_height_and_full_width() {
        let view: InputGroupView<AppState> =
            input_group(crate::input("", |_s: &mut AppState, _t: String| {}).flush(true))
                .addon(InputGroupAlign::BlockEnd, input_group_text("optional"));
        let mut w = build(&view);
        let size = layout(&mut w, WINDOW);
        assert!(
            size.height > style::HEIGHT_DEFAULT,
            "has-[>[data-align=block-end]]:h-auto, got {size:?}"
        );
        assert_eq!(size.width, WINDOW.width);
        // The block addon is a full-width row below the control's row.
        assert!(w.addon(0).origin().y >= style::HEIGHT_DEFAULT);
        assert_eq!(w.addon(0).origin().x, style::spacing(3.0), "px-3");
    }

    #[test]
    fn a_block_start_addon_is_ordered_above_the_control() {
        let view: InputGroupView<AppState> =
            input_group(crate::input("", |_s: &mut AppState, _t: String| {}).flush(true))
                .addon(InputGroupAlign::InlineEnd, input_group_text("end"))
                .addon(InputGroupAlign::BlockStart, input_group_text("top"));
        let mut w = build(&view);
        layout(&mut w, WINDOW);
        // `order-first` puts the block-start addon first in the pod order...
        assert_eq!(
            w.aligns,
            vec![InputGroupAlign::BlockStart, InputGroupAlign::InlineEnd]
        );
        // ...and above the control's row.
        assert!(w.addon(0).origin().y < w.control().origin().y);
    }

    #[test]
    fn the_group_paints_one_shared_border_and_shadow() {
        let mut h = Harness::new(true, false);
        let rec = h.paint();
        assert_eq!(rec.shadows, 1, "one shadow-xs for the whole group");
        let (bbox, _, color) = rec.border();
        assert_eq!(color, FALLBACK.input);
        assert_eq!(bbox.width(), WINDOW.width, "the group's own border box");
        assert_eq!(
            rec.borders(),
            1,
            "the flush control contributes no border of its own"
        );
    }

    #[test]
    fn focusing_the_inner_control_rings_the_group() {
        let mut h = Harness::new(false, false);
        h.theme(Brightness::Light);
        assert!(h.paint().ring().is_none());
        // A `Down` on the control focuses it; the group is on the recorded focus
        // path, so it reads focused with no child plumbing.
        h.pointer(PointerPhase::Down, 200.0, 18.0);
        h.pointer(PointerPhase::Up, 200.0, 18.0);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let ring = style::ring_color(None, Some(&theme));
        let rec = h.paint();
        assert_eq!(
            rec.ring(),
            Some(style::with_alpha(ring, style::FOCUS_RING_OPACITY))
        );
        assert_eq!(rec.border().2, ring, "and the border swaps with it");
    }

    #[test]
    fn an_invalid_group_borders_destructive_and_a_disabled_one_dims() {
        let invalid: InputGroupView<AppState> =
            input_group(crate::input("", |_s: &mut AppState, _t: String| {}).flush(true))
                .invalid(true);
        let mut w = build(&invalid);
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.border().2, FALLBACK.destructive);

        let disabled: InputGroupView<AppState> =
            input_group(crate::input("", |_s: &mut AppState, _t: String| {}).flush(true))
                .disabled(true);
        let mut w = build(&disabled);
        let size = layout(&mut w, WINDOW);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.border().2.components[3], style::DISABLED_OPACITY);
    }

    #[test]
    fn typing_in_the_group_round_trips_and_the_addon_button_fires() {
        let mut h = Harness::new(true, false);
        h.pointer(PointerPhase::Down, 200.0, 18.0);
        h.pointer(PointerPhase::Up, 200.0, 18.0);
        h.root.event(
            &mut h.state,
            &InputEvent::Key(frust::authoring::KeyEvent {
                key: Key::Character("q".to_string()),
                modifiers: frust::authoring::Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(h.state.text, "q", "the control is controlled by the app");

        // The trailing button sits at the right edge; a press there fires it, not
        // the field.
        h.pass();
        let x = WINDOW.width - 20.0;
        h.pointer(PointerPhase::Down, x, 18.0);
        h.pointer(PointerPhase::Up, x, 18.0);
        assert_eq!(h.state.clicks, 1);
    }

    #[test]
    fn the_addon_asks_for_the_text_cursor_and_a_button_child_overrides_it() {
        let mut h = Harness::new(true, false);
        // Over the leading text addon: the group's `cursor-text` stands.
        h.pointer(PointerPhase::Move, 20.0, 18.0);
        assert_eq!(h.root.cursor(), CursorIcon::Text);
        // Over the trailing button: the child spoke last and wins.
        h.pointer(PointerPhase::Move, WINDOW.width - 20.0, 18.0);
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);
    }

    #[test]
    fn the_button_size_ladder_matches_the_upstream_overrides() {
        for (size, height, icon) in [
            (InputGroupButtonSize::Xs, style::HEIGHT_XS, false),
            (InputGroupButtonSize::Sm, style::HEIGHT_SM, false),
            (InputGroupButtonSize::IconXs, style::HEIGHT_XS, true),
            (InputGroupButtonSize::IconSm, style::HEIGHT_SM, true),
        ] {
            assert_eq!(size.height(), height);
            assert_eq!(size.is_icon(), icon);
            let view: InputGroupButtonView<AppState> =
                input_group_button("x", |_s: &mut AppState| {}).size(size);
            let mut counter = 0u64;
            let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let measured = w.layout(&mut lctx, &BoxConstraints::loose(WINDOW));
            assert_eq!(measured.height, height);
            if icon {
                assert_eq!(measured.width, height, "an icon size is a square");
            } else {
                assert!(measured.width >= 2.0 * size.padding_x());
            }
        }
        // The two `xs` rungs use the tighter `calc(var(--radius)-5px)` radius.
        let theme = crate::theme();
        assert_eq!(
            InputGroupButtonSize::Xs.radius(Some(&theme)),
            crate::RADIUS_BASE - TIGHT_RADIUS_OFFSET
        );
        assert_eq!(
            InputGroupButtonSize::Sm.radius(Some(&theme)),
            ShadcnTokens::resolve_radius(None, Some(&theme)).md
        );
    }

    #[test]
    fn the_addon_button_washes_accent_on_hover_and_drops_it_on_leave() {
        let mut h = Harness::new(true, false);
        h.theme(Brightness::Light);
        let theme = crate::theme().with_brightness(Brightness::Light);
        let accent = theme.scheme().primary_container;
        assert!(!h.paint().has_fill(accent), "no wash at rest");

        h.pointer(PointerPhase::Move, WINDOW.width - 20.0, 18.0);
        assert!(h.paint().has_fill(accent), "hover:bg-accent");

        // Off the button: the wash goes with the hover link, corrected at paint.
        h.pointer(PointerPhase::Move, 20.0, 18.0);
        assert!(!h.paint().has_fill(accent));
    }

    #[test]
    fn visit_children_publishes_the_control_and_every_addon() {
        let view: InputGroupView<AppState> =
            input_group(crate::input("", |_s: &mut AppState, _t: String| {}).flush(true))
                .addon(InputGroupAlign::InlineStart, input_group_text("a"))
                .addon(InputGroupAlign::BlockEnd, input_group_text("b"));
        let w = build(&view);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 3, "the control plus both addons");
    }

    #[test]
    fn semantics_groups_the_control_with_its_addons() {
        let mut h = Harness::new(true, false);
        h.pass();
        let update = h.root.semantics();
        let (_, group) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("role=group");
        assert!(group.children().len() >= 2, "control + addons");
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::TextInput),
            "the inner control's node still reaches the tree"
        );
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Button),
            "the addon button's node too"
        );
    }

    // ---- Typeface: the addon text and button follow the live theme -------

    /// A text addon at each edge and a button addon, around a text-free
    /// control (the control is the caller's own view).
    #[cfg(feature = "bundled-fonts")]
    fn addons(_: &mut ()) -> InputGroupView<()> {
        input_group::<(), _>(frust::SizedBox(Some(160.0), Some(20.0)))
            .addon(InputGroupAlign::InlineStart, input_group_text("https://"))
            .addon(
                InputGroupAlign::InlineEnd,
                input_group_button("Go", |_: &mut ()| {}),
            )
            .addon(InputGroupAlign::BlockEnd, input_group_text("optional"))
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_addons_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "an input group's text and button addons",
            addons,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_addons_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "an input group's text and button addons",
            addons,
        );
    }
}
