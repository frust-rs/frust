//! Ports shadcn/ui's **Empty** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/empty.tsx`.
//!
//! A centered empty-state column: [`empty`] (`gap-6 p-6`, `md:p-12` not
//! modeled — see below), [`empty_header`] (`gap-2`), [`empty_content`]
//! (`gap-4`) share one internal layout (centered column, fixed gap/padding)
//! since none of the three differ in shape, only in the two numbers; each is
//! its own public constructor so a caller names the part it means.
//! [`empty_media`] adds `EmptyMedia`'s icon-chip treatment
//! ([`EmptyMediaVariant::Icon`]: `size-10 rounded-lg bg-muted`);
//! [`empty_title`]/[`empty_description`] are themed text (`text-lg
//! font-medium` / `text-sm text-muted-foreground`) — thin `TextView`
//! wrappers, no chrome of their own, matching the `TextView`-direct
//! precedent [`crate::components::marker`] documents.
//!
//! # Deviations
//!
//! - **No `border-dashed` rendering.** `PaintScene` has no dash-pattern
//!   primitive; painting a *solid* hairline in its place would misrepresent
//!   the source's dashed affordance rather than approximate it, so `empty`
//!   paints no border at all — a caller wanting a bordered empty state wraps
//!   it in an existing bordered container (e.g. a future `card`).
//! - **No `max-w-sm` clamp** on `empty_header`/`empty_content` — this port
//!   has no intrinsic-width-then-clamp layout pass; a caller constrains the
//!   surrounding layout instead.
//! - **`md:p-12` (the ≥768px breakpoint step) is not modeled** — frust has no
//!   breakpoint system; `empty` always pads `p-6` (24px).

use frust::authoring::text::FontWeight;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, ThemeTextColor,
    ThemeTextType, View, Widget, any,
};
use frust::{Theme, text};
use peniko::Color;

use crate::style;

/// `text-lg` — [`empty_title`]'s font size (18px; not in [`style`]'s ladder,
/// which stops at [`style::TEXT_BASE`]).
const TITLE_TEXT_SIZE: f64 = 18.0;
/// `p-6` — [`empty`]'s padding on every side.
const ROOT_PADDING: f64 = style::SPACING_UNIT * 6.0;
/// `gap-6` — [`empty`]'s row gap.
const ROOT_GAP: f64 = style::SPACING_UNIT * 6.0;
/// `gap-2` — [`empty_header`]'s row gap.
const HEADER_GAP: f64 = style::SPACING_UNIT * 2.0;
/// `gap-4` — [`empty_content`]'s row gap.
const CONTENT_GAP: f64 = style::SPACING_UNIT * 4.0;
/// `size-10` — [`EmptyMediaVariant::Icon`]'s chip diameter.
const MEDIA_ICON_SIZE: f64 = 40.0;

/// A centered flex column shared by [`empty`]/[`empty_header`]/
/// [`empty_content`] — see the [module docs](self).
pub struct EmptyColumnView<State: 'static> {
    children: Vec<AnyView<State>>,
    gap: f64,
    padding: f64,
}

/// `Empty`: the root centered column (`gap-6 p-6`).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn empty<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> EmptyColumnView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    EmptyColumnView {
        children,
        gap: ROOT_GAP,
        padding: ROOT_PADDING,
    }
}

/// `EmptyHeader`: a centered column (`gap-2`, no padding of its own).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn empty_header<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> EmptyColumnView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    EmptyColumnView {
        children,
        gap: HEADER_GAP,
        padding: 0.0,
    }
}

/// `EmptyContent`: a centered column (`gap-4`, no padding of its own).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn empty_content<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> EmptyColumnView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    EmptyColumnView {
        children,
        gap: CONTENT_GAP,
        padding: 0.0,
    }
}

/// The retained widget shared by every [`EmptyColumnView`] constructor.
pub struct EmptyColumnWidget {
    children: Vec<ChildPod>,
    gap: f64,
    padding: f64,
}

impl<State: 'static> View<State> for EmptyColumnView<State> {
    type Element = EmptyColumnWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> EmptyColumnWidget {
        EmptyColumnWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            gap: self.gap,
            padding: self.padding,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut EmptyColumnWidget,
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
        if element.gap != self.gap || element.padding != self.padding {
            element.gap = self.gap;
            element.padding = self.padding;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut EmptyColumnWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for EmptyColumnWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max_w = (bc.max().width - self.padding * 2.0).max(0.0);
        let loose = BoxConstraints::loose(Size::new(inner_max_w, f64::INFINITY));

        let mut sizes = Vec::with_capacity(self.children.len());
        let mut content_w = 0.0_f64;
        let mut y = self.padding;
        for (i, pod) in self.children.iter_mut().enumerate() {
            let s = pod.layout_child(ctx, &loose);
            if i > 0 {
                y += self.gap;
            }
            sizes.push((s, y));
            y += s.height;
            content_w = content_w.max(s.width);
        }
        y += self.padding;

        for (pod, (s, top)) in self.children.iter_mut().zip(sizes.iter()) {
            let x = self.padding + (content_w - s.width) / 2.0;
            pod.set_origin(Point::new(x, *top));
        }

        bc.constrain(Size::new(content_w + self.padding * 2.0, y))
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

/// `emptyMediaVariants`' `variant` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmptyMediaVariant {
    /// A transparent passthrough — the child paints itself.
    #[default]
    Default,
    /// A `size-10 rounded-lg bg-muted` icon chip.
    Icon,
}

/// `EmptyMedia`: a media/icon slot. See [`EmptyMediaVariant`].
pub struct EmptyMediaView<State: 'static> {
    variant: EmptyMediaVariant,
    child: AnyView<State>,
}

/// Wrap `child` as `EmptyMedia`, `Default` variant unless overridden with
/// [`EmptyMediaView::variant`].
pub fn empty_media<State: 'static, V: View<State>>(child: V) -> EmptyMediaView<State> {
    EmptyMediaView {
        variant: EmptyMediaVariant::default(),
        child: any(child),
    }
}

impl<State: 'static> EmptyMediaView<State> {
    /// Set the [`EmptyMediaVariant`].
    pub fn variant(mut self, variant: EmptyMediaVariant) -> Self {
        self.variant = variant;
        self
    }
}

/// The retained widget for an [`EmptyMediaView`].
pub struct EmptyMediaWidget {
    child: ChildPod,
    variant: EmptyMediaVariant,
}

impl<State: 'static> View<State> for EmptyMediaView<State> {
    type Element = EmptyMediaWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> EmptyMediaWidget {
        EmptyMediaWidget {
            child: frust::authoring::build_child(&self.child, ctx),
            variant: self.variant,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut EmptyMediaWidget,
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

    fn teardown(&self, element: &mut EmptyMediaWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

fn resolve_muted(theme: Option<&Theme>) -> Color {
    theme.map_or(Color::from_rgb8(0xF5, 0xF5, 0xF5), |t| {
        t.scheme().surface_container_highest
    })
}

impl Widget for EmptyMediaWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match self.variant {
            EmptyMediaVariant::Default => {
                let s = self
                    .child
                    .layout_child(ctx, &BoxConstraints::loose(bc.max()));
                self.child.set_origin(Point::ZERO);
                bc.constrain(s)
            }
            EmptyMediaVariant::Icon => {
                let size = Size::new(MEDIA_ICON_SIZE, MEDIA_ICON_SIZE);
                let child_size = self.child.layout_child(ctx, &BoxConstraints::loose(size));
                self.child.set_origin(Point::new(
                    (size.width - child_size.width) / 2.0,
                    (size.height - child_size.height) / 2.0,
                ));
                bc.constrain(size)
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.variant == EmptyMediaVariant::Icon {
            let theme = Theme::from_paint_ctx(ctx);
            let fill = resolve_muted(theme);
            scene.fill_rounded_rect(ctx.origin(), ctx.size(), style::spacing(2.0), fill);
        }
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

/// `EmptyTitle`: `text-lg font-medium tracking-tight`, `on_surface`.
pub fn empty_title(label: impl Into<String>) -> frust::TextView {
    text(label)
        .size(TITLE_TEXT_SIZE as f32)
        .weight(FontWeight::MEDIUM)
        .themed_family(ThemeTextType::TitleMedium)
        .themed_role(ThemeTextColor::OnSurface)
}

/// `EmptyDescription`: `text-sm text-muted-foreground`.
pub fn empty_description(label: impl Into<String>) -> frust::TextView {
    text(label)
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium)
        .themed_role(ThemeTextColor::OnSurfaceVariant)
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
    fn root_pads_24px_and_centers_children() {
        let view: EmptyColumnView<()> = empty(vec![any(leaf(20.0, 10.0)), any(leaf(40.0, 10.0))]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        // width = padding*2 + widest child (40) = 88; height = padding*2 + two
        // children + one gap = 48 + 10 + 10 + 24 = 92.
        assert_eq!(size, Size::new(88.0, 92.0));
        // The narrower first child is centered within the content width.
        assert_eq!(w.children[0].origin().x, ROOT_PADDING + (40.0 - 20.0) / 2.0);
    }

    #[test]
    fn header_uses_gap_2_with_no_padding() {
        let view: EmptyColumnView<()> =
            empty_header(vec![any(leaf(10.0, 10.0)), any(leaf(10.0, 10.0))]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(400.0, 400.0));
        assert_eq!(size, Size::new(10.0, 10.0 + HEADER_GAP + 10.0));
    }

    #[test]
    fn media_default_is_a_transparent_passthrough() {
        let view: EmptyMediaView<()> = empty_media(leaf(16.0, 16.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(100.0, 100.0));
        assert_eq!(size, Size::new(16.0, 16.0));
    }

    #[test]
    fn media_icon_variant_is_a_40px_chip_with_a_centered_child() {
        let view: EmptyMediaView<()> =
            empty_media(leaf(16.0, 16.0)).variant(EmptyMediaVariant::Icon);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Size::new(100.0, 100.0));
        assert_eq!(size, Size::new(MEDIA_ICON_SIZE, MEDIA_ICON_SIZE));
        assert_eq!(w.child.origin(), Point::new(12.0, 12.0));
    }

    #[test]
    fn title_and_description_are_themed_text_views() {
        // The title's larger font (18px vs 14px) lays out to a taller line —
        // the observable proof the two helpers really carry different sizes,
        // since `TextView`'s resolved style isn't itself public.
        let mut c1 = 0u64;
        let mut c2 = 0u64;
        let mut title_w =
            View::<()>::build(&empty_title("No results"), &mut BuildCtx::new(&mut c1));
        let mut desc_w =
            View::<()>::build(&empty_description("Try again"), &mut BuildCtx::new(&mut c2));
        let title_h = layout(&mut title_w, Size::new(400.0, 100.0)).height;
        let desc_h = layout(&mut desc_w, Size::new(400.0, 100.0)).height;
        assert!(
            title_h > desc_h,
            "the empty title renders larger than the description"
        );
    }

    // ---- Typeface: the title and description follow the live theme ------

    #[cfg(feature = "bundled-fonts")]
    fn header(_: &mut ()) -> EmptyColumnView<()> {
        empty_header(vec![
            any(empty_title("No projects yet")),
            any(empty_description("Create a project to get started.")),
        ])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "an empty state's title and description",
            header,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "an empty state's title and description",
            header,
        );
    }
}
