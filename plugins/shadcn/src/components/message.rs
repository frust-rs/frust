//! Ports shadcn/ui's **Message** (the AI-chat message row — role, avatar,
//! content) from `tmp/ui/apps/v4/registry/new-york-v4/ui/message.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! The source is six pure-layout `div`s and nothing else — no state, no
//! handlers, no variants — so every part here is inert: it lays children out,
//! paints (at most) one token-colored box or one muted text run, routes events
//! straight through to its children, and claims nothing of its own.
//!
//! # The six parts
//!
//! | Source | Here | Class list it stands for |
//! |---|---|---|
//! | `MessageGroup` | [`message_group`] | `flex min-w-0 flex-col gap-2` |
//! | `Message` | [`message`] | `flex w-full min-w-0 gap-2 text-sm data-[align=end]:flex-row-reverse` |
//! | `MessageAvatar` | [`message_avatar`] | `w-fit min-w-8 shrink-0 self-end rounded-full bg-muted` |
//! | `MessageContent` | [`message_content`] | `flex w-full min-w-0 flex-col gap-2.5` |
//! | `MessageHeader` | [`message_header`] | `flex px-3 text-xs font-medium text-muted-foreground` |
//! | `MessageFooter` | [`message_footer`] | the header's classes + `group-data-[align=end]/message:justify-end` |
//!
//! # Composition, not re-implementation
//!
//! The parts compose the catalog's existing components through their **public**
//! API rather than restating their looks: a row's content column is a stack of
//! [`bubble`](crate::bubble)s (which resolve their own left/right pill alignment
//! from [`BubbleAlign`](crate::BubbleAlign)), and [`message_avatar`] wraps an
//! [`avatar`](crate::avatar) in the sized, self-end, `bg-muted` box the source's
//! own classes describe. Nothing here reaches into either.
//!
//! # Three resolved layout decisions
//!
//! - **`data-[align=end]:flex-row-reverse` is a placement rule, not a child
//!   order.** [`message`] measures its children in the order given (so the
//!   fixed-width avatar is measured before the width-filling content column,
//!   which is what lets the column take the remainder) and *places* them
//!   right-to-left under [`MessageAlign::End`].
//! - **The row bottom-aligns its children.** The source leaves the row at flex's
//!   default `stretch` and puts `self-end` on the avatar alone; frust containers
//!   have no stretch pass, so the row aligns every child to its bottom edge —
//!   which is `self-end` for the avatar and invisible for the content column
//!   (the tallest child, and therefore the one that sets the row height).
//! - **`group-has-data-[slot=message-footer]/message:-translate-y-8` is an
//!   opt-in.** That class lifts the avatar clear of a footer line, and it is
//!   driven by a *sibling's* presence — something no widget here can observe. It
//!   is exposed as [`MessageAvatarView::raised`] for the caller that adds a
//!   footer, and is a paint-time translation with no effect on layout, exactly
//!   like the CSS transform it ports.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, ThemeTextType,
    Vec2, View, ViewSeq, Widget, any, build_child, rebuild_child, rebuild_children, route_event,
    route_event_single, teardown_child, visit_children,
};

use crate::components::input::FALLBACK;
use crate::style;
use crate::text::{Label, themed_family};

/// Which side of the thread a message sits on: `Start` is shadcn's `default`
/// (the other party), `End` is `data-[align=end]` (the local user).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageAlign {
    /// `data-align="start"` — leading edge, children placed left to right.
    #[default]
    Start,
    /// `data-align="end"` — trailing edge, `flex-row-reverse`.
    End,
}

/// `gap-2` — between messages in a group, and between a row's parts.
const GAP: f64 = style::SPACING_UNIT * 2.0;
/// `gap-2.5` — between the blocks stacked inside a message's content column.
const CONTENT_GAP: f64 = style::SPACING_UNIT * 2.5;
/// `min-w-8` — the avatar box's floor width.
const AVATAR_MIN_WIDTH: f64 = style::SPACING_UNIT * 8.0;
/// `-translate-y-8` — how far [`MessageAvatarView::raised`] lifts the avatar.
const AVATAR_RAISE: f64 = style::SPACING_UNIT * 8.0;
/// `px-3` — the header/footer text inset.
const META_PAD_X: f64 = style::SPACING_UNIT * 3.0;

/// The `muted-foreground` token a header/footer line is inked with: themed
/// `on_surface_variant`, else the fallback table.
fn muted_foreground(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted_foreground, |t| t.scheme().on_surface_variant)
}

/// The `muted` token the avatar box is filled with: themed
/// `surface_container_highest`, else the fallback table.
fn muted(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted, |t| t.scheme().surface_container_highest)
}

/// Lay `children` out as a full-width column with `gap` between them, each
/// stretched to the column's width so it can self-align — the shape both
/// [`message_group`] (`gap-2`) and [`message_content`] (`gap-2.5`) have.
fn layout_column(
    children: &mut [ChildPod],
    gap: f64,
    ctx: &mut LayoutCtx,
    bc: &BoxConstraints,
) -> Size {
    let width = if bc.max().width.is_finite() {
        bc.max().width
    } else {
        0.0
    };
    let child_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
    let mut y = 0.0_f64;
    for (index, pod) in children.iter_mut().enumerate() {
        if index > 0 {
            y += gap;
        }
        let size = pod.layout_child(ctx, &child_bc);
        pod.set_origin(Point::new(0.0, y));
        y += size.height;
    }
    bc.constrain(Size::new(width, y))
}

// ---- MessageGroup ---------------------------------------------------------

/// A declarative `MessageGroup`: a column of [`message`] rows (`gap-2`).
pub struct MessageGroupView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Stack `children` (typically [`message`] rows) in a `gap-2` column.
///
/// The list is any [`ViewSeq`] — a tuple of mixed view types (`(a, b, c)`),
/// a `Vec`/array of one type, an `Option`, or `views(iter)` — erased once here,
/// so no element needs `any(..)`.
pub fn message_group<State: 'static, M>(
    children: impl ViewSeq<State, M>,
) -> MessageGroupView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    MessageGroupView { children }
}

/// The retained widget for a [`MessageGroupView`].
pub struct MessageGroupWidget {
    children: Vec<ChildPod>,
}

impl<State: 'static> View<State> for MessageGroupView<State> {
    type Element = MessageGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageGroupWidget {
        MessageGroupWidget {
            children: self.children.iter().map(|v| build_child(v, ctx)).collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut MessageGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MessageGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        layout_column(&mut self.children, GAP, ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.children, ctx, event)
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

    visit_children!(children);
}

// ---- Message --------------------------------------------------------------

/// A declarative `Message`: one chat row. See the [module docs](self).
pub struct MessageView<State: 'static> {
    children: Vec<AnyView<State>>,
    align: MessageAlign,
}

/// A message row over `children` — typically an avatar box and a content
/// column, in that (measurement) order.
///
/// The list is any [`ViewSeq`] — a tuple of mixed view types (`(a, b, c)`),
/// a `Vec`/array of one type, an `Option`, or `views(iter)` — erased once here,
/// so no element needs `any(..)`.
pub fn message<State: 'static, M>(children: impl ViewSeq<State, M>) -> MessageView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    MessageView {
        children,
        align: MessageAlign::default(),
    }
}

impl<State: 'static> MessageView<State> {
    /// Set which side of the thread the row sits on (default
    /// [`MessageAlign::Start`]).
    pub fn align(mut self, align: MessageAlign) -> Self {
        self.align = align;
        self
    }
}

/// The retained widget for a [`MessageView`].
pub struct MessageWidget {
    children: Vec<ChildPod>,
    align: MessageAlign,
}

impl<State: 'static> View<State> for MessageView<State> {
    type Element = MessageWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageWidget {
        MessageWidget {
            children: self.children.iter().map(|v| build_child(v, ctx)).collect(),
            align: self.align,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        );
        if element.align != self.align {
            element.align = self.align;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MessageWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MessageWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let available = bc.max().width;
        let mut remaining = available;
        let mut sizes = Vec::with_capacity(self.children.len());
        for (index, pod) in self.children.iter_mut().enumerate() {
            if index > 0 {
                remaining = (remaining - GAP).max(0.0);
            }
            let child_bc = BoxConstraints::new(Size::ZERO, Size::new(remaining, f64::INFINITY));
            let size = pod.layout_child(ctx, &child_bc);
            remaining = (remaining - size.width).max(0.0);
            sizes.push(size);
        }

        let used: f64 = sizes.iter().map(|s| s.width).sum::<f64>()
            + GAP * (self.children.len().saturating_sub(1)) as f64;
        let height = sizes.iter().fold(0.0_f64, |acc, s| acc.max(s.height));
        let width = if available.is_finite() {
            available
        } else {
            used
        };

        // `flex-row-reverse`: the same measurement order, placed from the
        // trailing edge instead of the leading one.
        let mut cursor = match self.align {
            MessageAlign::Start => 0.0,
            MessageAlign::End => width,
        };
        for (pod, size) in self.children.iter_mut().zip(sizes.iter()) {
            let x = match self.align {
                MessageAlign::Start => {
                    let x = cursor;
                    cursor += size.width + GAP;
                    x
                }
                MessageAlign::End => {
                    cursor -= size.width;
                    let x = cursor;
                    cursor -= GAP;
                    x
                }
            };
            pod.set_origin(Point::new(x, (height - size.height).max(0.0)));
        }
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.children, ctx, event)
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

    visit_children!(children);
}

// ---- MessageAvatar --------------------------------------------------------

/// A declarative `MessageAvatar`: the sized, `bg-muted`, bottom-aligned box a
/// row's [`avatar`](crate::avatar) sits in.
pub struct MessageAvatarView<State: 'static> {
    child: AnyView<State>,
    raised: bool,
}

/// Wrap `child` (typically [`avatar`](crate::avatar)) in the message row's
/// avatar box: at least `min-w-8` wide, its content centered, filled `bg-muted`
/// behind whatever the child paints.
pub fn message_avatar<State: 'static, V: View<State>>(child: V) -> MessageAvatarView<State> {
    MessageAvatarView {
        child: any(child),
        raised: false,
    }
}

impl<State: 'static> MessageAvatarView<State> {
    /// Lift the avatar clear of a footer line
    /// (`group-has-data-[slot=message-footer]/message:-translate-y-8`).
    ///
    /// A paint-time translation only: the row's height and the box's own size
    /// are unchanged, exactly as a CSS transform leaves layout alone. Set it on
    /// a row that also carries a [`message_footer`] — see the [module
    /// docs](self) for why it cannot be detected here.
    pub fn raised(mut self, raised: bool) -> Self {
        self.raised = raised;
        self
    }
}

/// The retained widget for a [`MessageAvatarView`].
pub struct MessageAvatarWidget {
    child: ChildPod,
    raised: bool,
    /// The box's own size, kept for the `bg-muted` pill paint.
    size: Size,
}

impl<State: 'static> View<State> for MessageAvatarView<State> {
    type Element = MessageAvatarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageAvatarWidget {
        MessageAvatarWidget {
            child: build_child(&self.child, ctx),
            raised: self.raised,
            size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageAvatarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.raised != self.raised {
            element.raised = self.raised;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MessageAvatarWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for MessageAvatarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let child = self
            .child
            .layout_child(ctx, &BoxConstraints::new(Size::ZERO, bc.max()));
        let width = child.width.max(AVATAR_MIN_WIDTH);
        // `items-center justify-center`, plus the opt-in `-translate-y-8`.
        let lift = if self.raised { AVATAR_RAISE } else { 0.0 };
        self.child
            .set_origin(Point::new((width - child.width) / 2.0, -lift));
        self.size = bc.constrain(Size::new(width, child.height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let fill = muted(Theme::from_paint_ctx(ctx));
        let lift = if self.raised { AVATAR_RAISE } else { 0.0 };
        let origin = ctx.origin() - Vec2::new(0.0, lift);
        // `rounded-full`: a pill on the short axis, which is a circle for the
        // square avatar this box is sized around.
        scene.fill_rounded_rect(origin, self.size, self.size.height / 2.0, fill);
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

// ---- MessageContent -------------------------------------------------------

/// A declarative `MessageContent`: the full-width column a row's blocks
/// (header, bubbles, footer) stack in (`gap-2.5`).
pub struct MessageContentView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Stack `children` in the message's content column.
///
/// Each child is stretched to the column's width, so a child that self-aligns
/// — a [`bubble`](crate::bubble) under
/// [`BubbleAlign::End`](crate::BubbleAlign::End), a [`message_footer`] under
/// [`MessageAlign::End`] — resolves the source's
/// `group-data-[align=end]/message:*:self-end` itself rather than being placed
/// by this column.
///
/// The list is any [`ViewSeq`] — a tuple of mixed view types (`(a, b, c)`),
/// a `Vec`/array of one type, an `Option`, or `views(iter)` — erased once here,
/// so no element needs `any(..)`.
pub fn message_content<State: 'static, M>(
    children: impl ViewSeq<State, M>,
) -> MessageContentView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    MessageContentView { children }
}

/// The retained widget for a [`MessageContentView`].
pub struct MessageContentWidget {
    children: Vec<ChildPod>,
}

impl<State: 'static> View<State> for MessageContentView<State> {
    type Element = MessageContentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageContentWidget {
        MessageContentWidget {
            children: self.children.iter().map(|v| build_child(v, ctx)).collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageContentWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut MessageContentWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MessageContentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        layout_column(&mut self.children, CONTENT_GAP, ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.children, ctx, event)
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

    visit_children!(children);
}

// ---- MessageHeader / MessageFooter ----------------------------------------

/// A declarative `MessageHeader`: one muted `text-xs font-medium` line inset
/// `px-3` above a message's blocks (a name, a timestamp).
pub struct MessageHeaderView {
    text: String,
}

/// A header line reading `text`.
pub fn message_header(text: impl Into<String>) -> MessageHeaderView {
    MessageHeaderView { text: text.into() }
}

/// A declarative `MessageFooter`: the header's line, below the blocks and
/// trailing-aligned on an [`MessageAlign::End`] row.
pub struct MessageFooterView {
    text: String,
    align: MessageAlign,
}

/// A footer line reading `text`.
pub fn message_footer(text: impl Into<String>) -> MessageFooterView {
    MessageFooterView {
        text: text.into(),
        align: MessageAlign::default(),
    }
}

impl MessageFooterView {
    /// Match the row's own alignment: [`MessageAlign::End`] is the source's
    /// `group-data-[align=end]/message:justify-end`.
    pub fn align(mut self, align: MessageAlign) -> Self {
        self.align = align;
        self
    }
}

/// The retained widget behind [`MessageHeaderView`] and [`MessageFooterView`] —
/// one muted, inset line, differing only in which edge it sits against.
pub struct MessageMetaWidget {
    label: Label,
    text: String,
    align: MessageAlign,
    /// The line's own box, resolved at layout for the paint-time placement.
    size: Size,
}

impl MessageMetaWidget {
    fn new(text: String, align: MessageAlign) -> Self {
        MessageMetaWidget {
            label: Label::new(text.clone()),
            text,
            align,
            size: Size::ZERO,
        }
    }

    fn sync(&mut self, text: &str, align: MessageAlign) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.text != text {
            self.label.set_content(text.to_owned());
            self.text = text.to_owned();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if self.align != align {
            self.align = align;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl<State: 'static> View<State> for MessageHeaderView {
    type Element = MessageMetaWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MessageMetaWidget {
        MessageMetaWidget::new(self.text.clone(), MessageAlign::Start)
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut MessageMetaWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.sync(&self.text, MessageAlign::Start)
    }
}

impl<State: 'static> View<State> for MessageFooterView {
    type Element = MessageMetaWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MessageMetaWidget {
        MessageMetaWidget::new(self.text.clone(), self.align)
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut MessageMetaWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.sync(&self.text, self.align)
    }
}

impl Widget for MessageMetaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let style = themed_family(
            TextStyle {
                weight: FontWeight::MEDIUM,
                ..TextStyle::new(style::TEXT_XS as f32, muted_foreground(theme))
            },
            theme,
            ThemeTextType::LabelMedium,
        );
        let label = self.label.layout(ctx, &style);
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            label.width + META_PAD_X * 2.0
        };
        self.size = bc.constrain(Size::new(width, label.height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let label = self.label.size();
        let x = match self.align {
            MessageAlign::Start => META_PAD_X,
            MessageAlign::End => (self.size.width - META_PAD_X - label.width).max(META_PAD_X),
        };
        self.label.paint(ctx.origin() + Vec2::new(x, 0.0), scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Label, |node| {
            node.set_label(self.text.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::avatar::avatar;
    use crate::components::bubble::{BubbleAlign, bubble};
    use frust::Brightness;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent, PointerPhase};
    use frust_core::RenderRoot;
    use std::any::Any;

    const ROW: Size = Size::new(320.0, 400.0);

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        glyphs: Vec<Point>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            // A shaped run carries its origin as the translation of its own
            // transform (`TextLayout::to_scene_runs`).
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
    }

    fn build<V: View<()>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout<W: Widget>(widget: &mut W, box_size: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::loose(box_size))
    }

    fn paint<W: Widget>(widget: &mut W, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ORIGIN, size);
        match theme {
            Some(theme) => {
                let mut ctx = ctx.with_theme(theme as &dyn Any);
                widget.paint(&mut ctx, &mut rec);
            }
            None => widget.paint(&mut ctx, &mut rec),
        }
        rec
    }

    /// A row of the shape every chat screen builds: the avatar box (fixed) and
    /// the content column (fills the rest).
    fn row(align: MessageAlign) -> MessageView<()> {
        message((
            message_avatar(avatar::<()>().fallback("ED")),
            message_content(vec![bubble("hello").align(match align {
                MessageAlign::Start => BubbleAlign::Start,
                MessageAlign::End => BubbleAlign::End,
            })]),
        ))
        .align(align)
    }

    #[test]
    fn the_group_stacks_rows_in_a_gap_2_column() {
        let view: MessageGroupView<()> =
            message_group(vec![row(MessageAlign::Start), row(MessageAlign::End)]);
        let mut widget = build(&view);
        let size = layout(&mut widget, ROW);
        assert_eq!(size.width, ROW.width, "the group fills its width");
        let (first, second) = (&widget.children[0], &widget.children[1]);
        assert_eq!(first.origin(), Point::ORIGIN);
        assert_eq!(second.origin().y, first.size().height + GAP);
        assert_eq!(second.size().width, ROW.width, "rows are stretched");
    }

    #[test]
    fn a_start_row_places_the_avatar_first_and_the_column_takes_the_rest() {
        let mut widget = build(&row(MessageAlign::Start));
        let size = layout(&mut widget, ROW);
        let (avatar_pod, content_pod) = (&widget.children[0], &widget.children[1]);
        assert_eq!(avatar_pod.origin().x, 0.0);
        assert_eq!(content_pod.origin().x, avatar_pod.size().width + GAP);
        assert!(
            avatar_pod.size().width >= AVATAR_MIN_WIDTH,
            "min-w-8: {:?}",
            avatar_pod.size()
        );
        assert!(
            (content_pod.size().width - (ROW.width - avatar_pod.size().width - GAP)).abs() < 1e-9,
            "the column fills the remainder"
        );
        assert_eq!(size.width, ROW.width);
    }

    #[test]
    fn an_end_row_reverses_the_placement_but_not_the_measurement() {
        let mut start = build(&row(MessageAlign::Start));
        layout(&mut start, ROW);
        let mut end = build(&row(MessageAlign::End));
        layout(&mut end, ROW);

        // Same two children, same widths — only their x positions swap ends.
        assert_eq!(end.children[0].size(), start.children[0].size());
        assert_eq!(end.children[1].size(), start.children[1].size());
        let avatar_pod = &end.children[0];
        assert!(
            (avatar_pod.origin().x + avatar_pod.size().width - ROW.width).abs() < 1e-9,
            "the avatar sits against the trailing edge"
        );
        assert_eq!(end.children[1].origin().x, 0.0);
    }

    #[test]
    fn the_row_bottom_aligns_its_children() {
        // A tall content column and a short avatar box: `self-end` puts the
        // avatar's bottom edge on the row's.
        let view: MessageView<()> = message((
            message_avatar(avatar::<()>().fallback("ED")),
            message_content(vec![bubble("one"), bubble("two"), bubble("three")]),
        ));
        let mut widget = build(&view);
        let size = layout(&mut widget, ROW);
        let avatar_pod = &widget.children[0];
        assert!(avatar_pod.origin().y > 0.0, "pushed down to the bottom");
        assert!(
            (avatar_pod.origin().y + avatar_pod.size().height - size.height).abs() < 1e-9,
            "flush with the row's bottom edge"
        );
        assert_eq!(
            widget.children[1].origin().y,
            0.0,
            "the tallest child sets it"
        );
    }

    #[test]
    fn the_avatar_box_fills_muted_and_raises_without_resizing() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let view: MessageAvatarView<()> = message_avatar(avatar::<()>().fallback("ED"));
        let mut plain = build(&view);
        let size = layout(&mut plain, ROW);
        let rec = paint(&mut plain, size, Some(&theme));
        let (origin, box_size, radius, color) = rec.rrects[0];
        assert_eq!(color, theme.scheme().surface_container_highest, "bg-muted");
        assert_eq!(origin, Point::ORIGIN, "unraised");
        assert_eq!(radius, box_size.height / 2.0, "rounded-full");

        let raised_view: MessageAvatarView<()> =
            message_avatar(avatar::<()>().fallback("ED")).raised(true);
        let mut raised = build(&raised_view);
        let raised_size = layout(&mut raised, ROW);
        assert_eq!(raised_size, size, "a transform never resizes the box");
        let raised_rec = paint(&mut raised, raised_size, Some(&theme));
        assert_eq!(raised_rec.rrects[0].0, Point::new(0.0, -AVATAR_RAISE));
        assert_eq!(raised.child.origin().y, -AVATAR_RAISE);
    }

    #[test]
    fn the_content_column_stretches_its_blocks_and_gaps_them_by_2_5() {
        let view: MessageContentView<()> = message_content((
            message_header("Ed"),
            bubble("hello"),
            message_footer("just now"),
        ));
        let mut widget = build(&view);
        let size = layout(&mut widget, ROW);
        assert_eq!(size.width, ROW.width);
        let tops: Vec<f64> = widget.children.iter().map(|p| p.origin().y).collect();
        assert_eq!(tops[0], 0.0);
        assert!((tops[1] - (widget.children[0].size().height + CONTENT_GAP)).abs() < 1e-9);
        assert!(
            widget
                .children
                .iter()
                .all(|p| (p.size().width - ROW.width).abs() < 1e-9),
            "every block is stretched so it can self-align"
        );
        assert_eq!(CONTENT_GAP, 10.0, "gap-2.5");
    }

    #[test]
    fn a_meta_line_is_muted_inset_text_that_swaps_edges_with_the_align() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let header: MessageHeaderView = message_header("Ed");
        let mut widget = build(&header);
        let size = layout(&mut widget, ROW);
        assert_eq!(size.width, ROW.width, "max-w-full");
        let rec = paint(&mut widget, size, Some(&theme));
        assert_eq!(rec.glyphs[0].x, META_PAD_X, "px-3 from the leading edge");

        let footer: MessageFooterView = message_footer("just now").align(MessageAlign::End);
        let mut footer_widget = build(&footer);
        let footer_size = layout(&mut footer_widget, ROW);
        let footer_rec = paint(&mut footer_widget, footer_size, Some(&theme));
        let label = footer_widget.label.size();
        assert!(
            (footer_rec.glyphs[0].x - (ROW.width - META_PAD_X - label.width)).abs() < 1e-9,
            "justify-end, still inset px-3"
        );
    }

    #[test]
    fn the_parts_are_inert_but_still_route_to_their_children() {
        // A press inside the row reaches an interactive descendant; nothing in
        // this module consumes or claims it.
        #[derive(Default)]
        struct AppState {
            presses: u32,
        }
        let mut root: RenderRoot<AppState, MessageGroupView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| {
            message_group(vec![message(vec![message_content(vec![crate::button(
                "send",
                |s: &mut AppState| s.presses += 1,
            )])])])
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(ROW, &mut tcx as &mut dyn Any);
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            root.event(
                &mut state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(20.0, 10.0),
                    button: PointerButton::Primary,
                }),
            );
        }
        assert_eq!(state.presses, 1, "the routed press reached the button");
    }

    #[test]
    fn every_part_forwards_its_children_to_the_semantics_tree() {
        #[derive(Default)]
        struct AppState;
        let mut root: RenderRoot<AppState, MessageGroupView<AppState>> = RenderRoot::new();
        let mut state = AppState;
        let mut tcx = TextContext::new();
        let mut logic = |_s: &mut AppState| {
            message_group(vec![
                message((
                    message_avatar(avatar::<AppState>().fallback("ED")),
                    message_content((message_header("Ed"), message_footer("just now"))),
                ))
                .align(MessageAlign::End),
            ])
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(ROW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let labels: Vec<String> = update
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(|l| l.to_string()))
            .collect();
        for expected in ["Ed", "just now"] {
            assert!(
                labels.iter().any(|l| l == expected),
                "{expected} in {labels:?}"
            );
        }
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::GenericContainer),
            "the containers contribute their own nodes"
        );
    }

    // ---- Typeface: the header and footer follow the live theme ------------

    /// A row whose content column holds only a header and a footer line, so
    /// every painted glyph run is one this module shapes.
    #[cfg(feature = "bundled-fonts")]
    fn meta_lines(_: &mut ()) -> MessageGroupView<()> {
        message_group(vec![message(vec![message_content((
            message_header("Ed"),
            message_footer("just now"),
        ))])])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_header_and_footer_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a message's header and footer",
            meta_lines,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_header_and_footer_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a message's header and footer",
            meta_lines,
        );
    }
}
