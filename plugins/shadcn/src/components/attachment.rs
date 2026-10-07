//! Ports shadcn/ui's **Attachment** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/attachment.tsx`.
//!
//! [`attachment`] is a bordered card (`rounded-xl border bg-card`) laying its
//! children out horizontally or vertically per [`AttachmentOrientation`], at
//! one of three [`AttachmentSize`]s; its border reacts to
//! [`AttachmentState`] — [`AttachmentState::Idle`] and
//! [`AttachmentState::Error`] each get their own border treatment (see
//! *Deviations*). [`attachment_media`]/[`attachment_description`] take an
//! explicit `.error(bool)` flag a caller sets from the same state (see
//! *Deviations*) rather than this port modeling a CSS `group` context.
//!
//! # Deviations
//!
//! - **No dash pattern.** `PaintScene` has no dashed-stroke primitive (the
//!   same gap [`crate::components::empty`] documents); `Idle`'s
//!   `border-dashed` paints as an ordinary solid `outline` border instead.
//! - **No shimmer.** `AttachmentTitle`'s `Uploading`/`Processing` shimmer
//!   animation has no counterpart here — the title paints static.
//! - **State is explicit per-part, not group-derived.** The source drives
//!   `AttachmentMedia`/`AttachmentDescription`'s error tint from a CSS
//!   `group-data-[state=error]` selector on the ancestor `Attachment`; this
//!   port has no such cascade, so [`attachment_media`]/[`attachment_description`]
//!   each take their own `.error(bool)` a caller sets from the same
//!   [`AttachmentState`] it gave [`attachment`].
//! - **Text size does not vary with [`AttachmentSize`].**
//!   [`attachment_title`]/[`attachment_description`] are always `text-sm`/
//!   `text-xs` respectively, regardless of the container's size.
//! - **No `AttachmentTrigger`/scroll.** The invisible full-cover trigger
//!   button needs a `Button` component this catalog doesn't yet ship (a
//!   deferred follow-on); [`attachment_group`]'s horizontal-scroll
//!   affordance isn't modeled (a plain row, matching every other
//!   `*_group`/`*_list` container in this batch).

use frust::authoring::text::FontWeight;
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, RoundedRect, SemanticsCtx, Shape,
    Size, ThemeTextColor, ThemeTextType, View, Widget, any,
};
use frust::{Theme, text};
use peniko::Color;

use crate::style;

/// `attachmentVariants`' `size` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachmentSize {
    #[default]
    Default,
    Sm,
    Xs,
}

impl AttachmentSize {
    fn gap(self) -> f64 {
        match self {
            AttachmentSize::Default => style::spacing(2.0),
            AttachmentSize::Sm => style::spacing(2.5),
            AttachmentSize::Xs => style::spacing(1.5),
        }
    }
    fn padding(self) -> (f64, f64) {
        match self {
            AttachmentSize::Default => (style::spacing(2.5), style::spacing(2.0)),
            AttachmentSize::Sm => (style::spacing(2.0), style::spacing(1.5)),
            AttachmentSize::Xs => (style::spacing(1.5), style::spacing(1.0)),
        }
    }
    fn media_px(self) -> f64 {
        match self {
            AttachmentSize::Default => 40.0,
            AttachmentSize::Sm => 32.0,
            AttachmentSize::Xs => 28.0,
        }
    }
    fn min_width(self) -> f64 {
        160.0 // `min-w-40`, uniform across sizes.
    }
}

/// `attachmentVariants`' `orientation` axis. `Horizontal` = shadcn's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachmentOrientation {
    #[default]
    Horizontal,
    Vertical,
}

/// `Attachment`'s `state` prop. `Done` = shadcn's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachmentState {
    Idle,
    Uploading,
    Processing,
    Error,
    #[default]
    Done,
}

/// A declarative shadcn attachment card. See the [module docs](self).
pub struct AttachmentView<State: 'static> {
    children: Vec<AnyView<State>>,
    size: AttachmentSize,
    orientation: AttachmentOrientation,
    state: AttachmentState,
}

/// Create an attachment card from `children` (typically
/// [`attachment_media`]?, [`attachment_content`], [`attachment_actions`]?).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn attachment<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AttachmentView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    AttachmentView {
        children,
        size: AttachmentSize::default(),
        orientation: AttachmentOrientation::default(),
        state: AttachmentState::default(),
    }
}

impl<State: 'static> AttachmentView<State> {
    pub fn size(mut self, size: AttachmentSize) -> Self {
        self.size = size;
        self
    }
    pub fn orientation(mut self, orientation: AttachmentOrientation) -> Self {
        self.orientation = orientation;
        self
    }
    pub fn state(mut self, state: AttachmentState) -> Self {
        self.state = state;
        self
    }
}

/// The retained widget for an [`AttachmentView`].
pub struct AttachmentWidget {
    children: Vec<ChildPod>,
    size: AttachmentSize,
    orientation: AttachmentOrientation,
    state: AttachmentState,
}

impl<State: 'static> View<State> for AttachmentView<State> {
    type Element = AttachmentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AttachmentWidget {
        AttachmentWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            size: self.size,
            orientation: self.orientation,
            state: self.state,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AttachmentWidget,
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
        if element.size != self.size || element.orientation != self.orientation {
            element.size = self.size;
            element.orientation = self.orientation;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.state != self.state {
            element.state = self.state;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut AttachmentWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

fn resolve_card(theme: Option<&Theme>) -> Color {
    theme.map_or(Color::WHITE, |t| t.scheme().surface_container)
}

fn resolve_border(state: AttachmentState, theme: Option<&Theme>) -> Color {
    let (outline, error) = theme.map_or(
        (
            Color::from_rgb8(0xE5, 0xE5, 0xE5),
            Color::from_rgb8(0xE7, 0x00, 0x0B),
        ),
        |t| (t.scheme().outline, t.scheme().error),
    );
    match state {
        AttachmentState::Error => style::with_alpha(error, 0.30),
        _ => outline,
    }
}

impl Widget for AttachmentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let (pad_x, pad_y) = self.size.padding();
        let gap = self.size.gap();
        let loose = BoxConstraints::loose(bc.max());

        let mut sizes = Vec::with_capacity(self.children.len());
        for pod in self.children.iter_mut() {
            sizes.push(pod.layout_child(ctx, &loose));
        }

        let (content_w, content_h) = match self.orientation {
            AttachmentOrientation::Horizontal => {
                let mut w = sizes.iter().map(|s| s.width).sum::<f64>();
                if !sizes.is_empty() {
                    w += gap * (sizes.len() - 1) as f64;
                }
                let h = sizes.iter().map(|s| s.height).fold(0.0, f64::max);
                let mut x = pad_x;
                for (pod, s) in self.children.iter_mut().zip(sizes.iter()) {
                    pod.set_origin(Point::new(x, pad_y + (h - s.height) / 2.0));
                    x += s.width + gap;
                }
                (w, h)
            }
            AttachmentOrientation::Vertical => {
                let mut h = sizes.iter().map(|s| s.height).sum::<f64>();
                if !sizes.is_empty() {
                    h += gap * (sizes.len() - 1) as f64;
                }
                let w = sizes.iter().map(|s| s.width).fold(0.0, f64::max);
                let mut y = pad_y;
                for (pod, s) in self.children.iter_mut().zip(sizes.iter()) {
                    pod.set_origin(Point::new(pad_x, y));
                    y += s.height + gap;
                }
                (w, h)
            }
        };

        let mut width = content_w + pad_x * 2.0;
        if self.orientation == AttachmentOrientation::Horizontal {
            width = width.max(self.size.min_width());
        }
        let height = content_h + pad_y * 2.0;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let radius = theme.map_or(14.0, |t| t.shape.large);
        let fill = resolve_card(theme);
        let border = resolve_border(self.state, theme);

        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        let rr = RoundedRect::from_rect(
            kurbo::Rect::from_origin_size(Point::ORIGIN, ctx.size()),
            radius,
        );
        scene.stroke_path(
            ctx.origin(),
            &rr.to_path(0.1),
            style::BORDER_WIDTH,
            &Brush::Solid(border),
        );

        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::GenericContainer,
            |_| {},
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

// ---- AttachmentMedia -----------------------------------------------------

/// `attachmentMediaVariants`' `variant` axis. `Icon` = shadcn's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachmentMediaVariant {
    #[default]
    Icon,
    Image,
}

/// `AttachmentMedia`: a square icon/thumbnail slot.
pub struct AttachmentMediaView<State: 'static> {
    variant: AttachmentMediaVariant,
    size: AttachmentSize,
    error: bool,
    child: AnyView<State>,
}

/// Wrap `child` as `AttachmentMedia` at [`AttachmentSize::Default`], `Icon`
/// variant, unless overridden.
pub fn attachment_media<State: 'static, V: View<State>>(child: V) -> AttachmentMediaView<State> {
    AttachmentMediaView {
        variant: AttachmentMediaVariant::default(),
        size: AttachmentSize::default(),
        error: false,
        child: any(child),
    }
}

impl<State: 'static> AttachmentMediaView<State> {
    pub fn variant(mut self, variant: AttachmentMediaVariant) -> Self {
        self.variant = variant;
        self
    }
    /// Match the enclosing [`attachment`]'s size (no cascade — see the
    /// module docs' *Deviations*).
    pub fn size(mut self, size: AttachmentSize) -> Self {
        self.size = size;
        self
    }
    /// Set from the enclosing attachment's `state == Error`.
    pub fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }
}

pub struct AttachmentMediaWidget {
    child: ChildPod,
    size: AttachmentSize,
    error: bool,
}

impl<State: 'static> View<State> for AttachmentMediaView<State> {
    type Element = AttachmentMediaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AttachmentMediaWidget {
        AttachmentMediaWidget {
            child: frust::authoring::build_child(&self.child, ctx),
            size: self.size,
            error: self.error,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AttachmentMediaWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.error != self.error {
            element.error = self.error;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut AttachmentMediaWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AttachmentMediaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let px = self.size.media_px();
        let size = Size::new(px, px);
        self.child.layout_child(ctx, &BoxConstraints::tight(size));
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let radius = if self.size == AttachmentSize::Xs {
            theme.map_or(8.0, |t| t.shape.small)
        } else {
            theme.map_or(14.0, |t| t.shape.large)
        };
        let fill = if self.error {
            theme.map_or(
                style::with_alpha(Color::from_rgb8(0xE7, 0x00, 0x0B), 0.10),
                |t| t.scheme().error_container,
            )
        } else {
            theme.map_or(Color::from_rgb8(0xF5, 0xF5, 0xF5), |t| {
                t.scheme().surface_container_highest
            })
        };
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    frust::authoring::visit_children!(child);
}

// ---- AttachmentContent / Title / Description -----------------------------

/// `AttachmentContent`: a left-aligned column, tight gap.
pub struct AttachmentContentView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Stack `children` (typically [`attachment_title`]/
/// [`attachment_description`]) in a left-aligned column.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn attachment_content<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AttachmentContentView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    AttachmentContentView { children }
}

pub struct AttachmentContentWidget {
    children: Vec<ChildPod>,
}

/// The tight gap between an attachment's title and description.
const CONTENT_GAP: f64 = 2.0;

impl<State: 'static> View<State> for AttachmentContentView<State> {
    type Element = AttachmentContentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AttachmentContentWidget {
        AttachmentContentWidget {
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
        element: &mut AttachmentContentWidget,
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

    fn teardown(&self, element: &mut AttachmentContentWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for AttachmentContentWidget {
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

/// `AttachmentTitle`: `font-medium truncate` (no shimmer — see *Deviations*).
pub fn attachment_title(label: impl Into<String>) -> frust::TextView {
    text(label)
        .size(style::TEXT_SM as f32)
        .weight(FontWeight::MEDIUM)
        .themed_family(ThemeTextType::TitleSmall)
        .themed_role(ThemeTextColor::OnSurface)
}

/// `AttachmentDescription`: `text-xs text-muted-foreground`, or
/// `text-destructive/80` when `error` (see *Deviations*).
pub fn attachment_description(label: impl Into<String>, error: bool) -> frust::TextView {
    let role = if error {
        ThemeTextColor::Error
    } else {
        ThemeTextColor::OnSurfaceVariant
    };
    text(label)
        .size(style::TEXT_XS as f32)
        .themed_family(ThemeTextType::BodySmall)
        .themed_role(role)
}

// ---- AttachmentActions / AttachmentGroup ---------------------------------

/// A row shared by [`attachment_actions`]/[`attachment_group`].
pub struct AttachmentRowView<State: 'static> {
    children: Vec<AnyView<State>>,
    gap: f64,
}

/// `AttachmentActions`: a row of controls (no fixed gap in the source beyond
/// the parent's own `gap`; this port uses `gap-2`, 8px).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn attachment_actions<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AttachmentRowView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    AttachmentRowView {
        children,
        gap: style::SPACING_UNIT * 2.0,
    }
}

/// `AttachmentGroup`: a row of attachments, `gap-3` (12px). See the module
/// docs for the scroll-affordance deviation.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn attachment_group<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AttachmentRowView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    AttachmentRowView {
        children,
        gap: style::SPACING_UNIT * 3.0,
    }
}

pub struct AttachmentRowWidget {
    children: Vec<ChildPod>,
    gap: f64,
}

impl<State: 'static> View<State> for AttachmentRowView<State> {
    type Element = AttachmentRowWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AttachmentRowWidget {
        AttachmentRowWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            gap: self.gap,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AttachmentRowWidget,
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
        if element.gap != self.gap {
            element.gap = self.gap;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut AttachmentRowWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for AttachmentRowWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = BoxConstraints::loose(Size::new(f64::INFINITY, bc.max().height));
        let mut x = 0.0_f64;
        let mut height = 0.0_f64;
        let mut sizes = Vec::with_capacity(self.children.len());
        for pod in self.children.iter_mut() {
            let s = pod.layout_child(ctx, &loose);
            height = height.max(s.height);
            sizes.push(s);
        }
        for (i, (pod, s)) in self.children.iter_mut().zip(sizes.iter()).enumerate() {
            if i > 0 {
                x += self.gap;
            }
            pod.set_origin(Point::new(x, (height - s.height) / 2.0));
            x += s.width;
        }
        bc.constrain(Size::new(x, height))
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

    #[derive(Default)]
    struct Recorder {
        rounded_rects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rounded_rects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, b: &Brush) {
            if let Brush::Solid(c) = b {
                self.strokes.push(*c);
            }
        }
    }

    #[test]
    fn default_horizontal_enforces_the_160px_min_width() {
        let view: AttachmentView<()> = attachment(vec![leaf(10.0, 10.0)]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        assert_eq!(size.width, 160.0);
    }

    #[test]
    fn error_state_paints_a_destructive_border() {
        let view: AttachmentView<()> =
            attachment(vec![leaf(10.0, 10.0)]).state(AttachmentState::Error);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.strokes.len(), 1);
        assert!((rec.strokes[0].components[3] - 0.30).abs() < 1e-6);
    }

    #[test]
    fn vertical_orientation_stacks_children_top_to_bottom() {
        let view: AttachmentView<()> = attachment(vec![leaf(10.0, 10.0), leaf(10.0, 10.0)])
            .orientation(AttachmentOrientation::Vertical);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, Size::new(400.0, 400.0));
        assert_eq!(w.children[0].origin().y, w.size.padding().1);
        assert!(w.children[1].origin().y > w.children[0].origin().y);
    }

    #[test]
    fn media_error_flag_switches_to_the_destructive_wash() {
        let view: AttachmentMediaView<()> = attachment_media(leaf(10.0, 10.0)).error(true);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(100.0, 100.0));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert!((rec.rounded_rects[0].3.components[3] - 0.10).abs() < 1e-6);
    }

    #[test]
    fn actions_row_lays_out_left_to_right() {
        let view: AttachmentRowView<()> =
            attachment_actions(vec![leaf(10.0, 10.0), leaf(10.0, 10.0)]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, Size::new(200.0, 40.0));
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 10.0 + style::SPACING_UNIT * 2.0);
    }

    // ---- Typeface: the title and descriptions follow the live theme ------

    /// A title plus both description inks, stacked.
    #[cfg(feature = "bundled-fonts")]
    fn titled(_: &mut ()) -> AttachmentView<()> {
        attachment(vec![
            any(attachment_title("report.pdf")),
            any(attachment_description("2.4 MB", false)),
            any(attachment_description("Upload failed", true)),
        ])
        .orientation(AttachmentOrientation::Vertical)
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_descriptions_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "an attachment's title and descriptions",
            titled,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_descriptions_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "an attachment's title and descriptions",
            titled,
        );
    }
}
