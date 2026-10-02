//! Ports shadcn/ui's **Marker** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/marker.tsx`.
//!
//! A small inline annotation row — `[ icon? | content ]` at `text-sm
//! text-muted-foreground`, `min-h-4` (16px). [`MarkerVariant::Separator`]
//! flanks the icon+content block with two elastic hairlines (`--border`);
//! [`MarkerVariant::Border`] instead drops a hairline under the whole row
//! (`border-b border-border`) with an 8px gap above it (`pb-2`).
//!
//! `MarkerIcon`/`MarkerContent` are not ported as separate types: the source's
//! two spans carry no styling of their own beyond the parent's `[&_svg]`/
//! `wrap-break-word` selectors, which don't cross into frust's widget model
//! (there is no child-selector cascade), so [`marker`] takes the icon and
//! content directly instead of two child slots a caller assembles by hand.
//! `MarkerContent`'s embedded-link hover (`[a]:hover:text-foreground`) is out
//! of scope for the same reason — content here is a plain string, not nested
//! markup.

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, ThemeTextColor,
    ThemeTextType, View, Widget, any,
};
use frust::text;
use peniko::Color;

use crate::style;

/// `min-h-4` — the row's minimum height, in logical px.
const MIN_HEIGHT: f64 = 16.0;
/// `gap-2` — the gap between the icon and content, in logical px.
const ICON_GAP: f64 = style::SPACING_UNIT * 2.0;
/// `before:mr-1` / `after:ml-1` — the gap between a separator hairline and the
/// content block, in logical px.
const SEPARATOR_GAP: f64 = style::SPACING_UNIT;
/// `pb-2` — the [`MarkerVariant::Border`] gap above its bottom hairline.
const BORDER_PAD_BOTTOM: f64 = style::SPACING_UNIT * 2.0;
/// Every hairline's stroke width (Tailwind's `border`/a 1px pseudo-element).
const LINE_WIDTH: f64 = style::BORDER_WIDTH;

/// Unthemed-fallback hairline color (a theme resolves `colors.outline`).
const FALLBACK_LINE: Color = Color::from_rgb8(0xE5, 0xE5, 0xE5);

/// `markerVariants`' `variant` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkerVariant {
    /// A plain `[ icon? | content ]` row.
    #[default]
    Default,
    /// Flanks the row with two elastic hairlines — a labeled divider.
    Separator,
    /// A hairline under the whole row, with an 8px gap above it.
    Border,
}

/// A declarative shadcn marker row. See the [module docs](self).
pub struct MarkerView<State: 'static> {
    content: String,
    icon: Option<AnyView<State>>,
    variant: MarkerVariant,
}

/// Create a `Default`-variant marker with `content` as its label.
pub fn marker<State: 'static>(content: impl Into<String>) -> MarkerView<State> {
    MarkerView {
        content: content.into(),
        icon: None,
        variant: MarkerVariant::default(),
    }
}

impl<State: 'static> MarkerView<State> {
    /// Set the leading icon slot (`MarkerIcon`'s `size-4`, laid out at its own
    /// natural size).
    pub fn icon<V: View<State>>(mut self, icon: V) -> Self {
        self.icon = Some(any(icon));
        self
    }

    /// Set the [`MarkerVariant`].
    pub fn variant(mut self, variant: MarkerVariant) -> Self {
        self.variant = variant;
        self
    }

    fn content_view(&self) -> AnyView<State> {
        any(text(self.content.clone())
            .size(style::TEXT_SM as f32)
            .themed_family(ThemeTextType::BodyMedium)
            .themed_role(ThemeTextColor::OnSurfaceVariant))
    }
}

/// The retained widget for a [`MarkerView`]. Icon (if any) and content share
/// one `Vec` — icon is always slot 0 when present — so pointer/broadcast
/// routing goes through [`frust::authoring::route_event`] like any other
/// multi-child container.
pub struct MarkerWidget {
    children: Vec<ChildPod>,
    has_icon: bool,
    variant: MarkerVariant,
    /// `(line_before_width, line_after_width)`, non-zero only for
    /// [`MarkerVariant::Separator`].
    lines: (f64, f64),
    row_height: f64,
}

impl MarkerWidget {
    fn content_idx(&self) -> usize {
        if self.has_icon { 1 } else { 0 }
    }
}

impl<State: 'static> View<State> for MarkerView<State> {
    type Element = MarkerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MarkerWidget {
        let mut children = Vec::with_capacity(2);
        if let Some(icon) = self.icon.as_ref() {
            children.push(frust::authoring::build_child(icon, ctx));
        }
        children.push(frust::authoring::build_child(&self.content_view(), ctx));
        MarkerWidget {
            children,
            has_icon: self.icon.is_some(),
            variant: self.variant,
            lines: (0.0, 0.0),
            row_height: MIN_HEIGHT,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MarkerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if prev.icon.is_some() != self.icon.is_some() {
            // Icon presence changed: the child count itself changes, so tear
            // the whole list down (against `prev`, which still matches the
            // live pods) and rebuild fresh.
            if let Some(icon) = prev.icon.as_ref() {
                frust::authoring::teardown_child(icon, &mut element.children[0], ctx);
            }
            let prev_content_idx = if prev.icon.is_some() { 1 } else { 0 };
            frust::authoring::teardown_child(
                &prev.content_view(),
                &mut element.children[prev_content_idx],
                ctx,
            );

            let mut children = Vec::with_capacity(2);
            if let Some(icon) = self.icon.as_ref() {
                children.push(frust::authoring::build_child(icon, ctx));
            }
            children.push(frust::authoring::build_child(&self.content_view(), ctx));
            element.children = children;
            element.has_icon = self.icon.is_some();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            if let (Some(prev_icon), Some(next_icon)) = (prev.icon.as_ref(), self.icon.as_ref()) {
                flags |= frust::authoring::rebuild_child(
                    prev_icon,
                    next_icon,
                    &mut element.children[0],
                    ctx,
                );
            }
            let content_idx = element.content_idx();
            flags |= frust::authoring::rebuild_child(
                &prev.content_view(),
                &self.content_view(),
                &mut element.children[content_idx],
                ctx,
            );
        }

        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MarkerWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(icon) = self.icon.as_ref() {
            frust::authoring::teardown_child(icon, &mut element.children[0], ctx);
        }
        let content_idx = element.content_idx();
        frust::authoring::teardown_child(
            &self.content_view(),
            &mut element.children[content_idx],
            ctx,
        );
    }
}

fn resolve_line_color(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_LINE, |t| t.scheme().outline)
}

impl Widget for MarkerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        let loose = BoxConstraints::loose(bc.max());
        let content_idx = self.content_idx();

        let icon_size = if self.has_icon {
            self.children[0].layout_child(ctx, &loose)
        } else {
            Size::ZERO
        };
        let content_size = self.children[content_idx].layout_child(ctx, &loose);

        let block_w =
            icon_size.width + if self.has_icon { ICON_GAP } else { 0.0 } + content_size.width;
        self.row_height = icon_size.height.max(content_size.height).max(MIN_HEIGHT);

        self.lines = if self.variant == MarkerVariant::Separator {
            let remaining = (available - block_w - SEPARATOR_GAP * 2.0).max(0.0);
            (remaining / 2.0, remaining / 2.0)
        } else {
            (0.0, 0.0)
        };

        let mut x = self.lines.0
            + if self.lines.0 > 0.0 {
                SEPARATOR_GAP
            } else {
                0.0
            };
        if self.has_icon {
            self.children[0].set_origin(Point::new(x, (self.row_height - icon_size.height) / 2.0));
            x += icon_size.width + ICON_GAP;
        }
        self.children[content_idx]
            .set_origin(Point::new(x, (self.row_height - content_size.height) / 2.0));

        let total_height = match self.variant {
            MarkerVariant::Border => self.row_height + BORDER_PAD_BOTTOM,
            _ => self.row_height,
        };
        bc.constrain(Size::new(available.max(block_w), total_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let line_color = resolve_line_color(theme);
        let origin = ctx.origin();
        let mid_y = self.row_height / 2.0;

        if self.variant == MarkerVariant::Separator {
            let (before, after) = self.lines;
            if before > 0.0 {
                scene.stroke_line(
                    origin + kurbo::Vec2::new(0.0, mid_y),
                    origin + kurbo::Vec2::new(before, mid_y),
                    LINE_WIDTH,
                    line_color,
                );
            }
            if after > 0.0 {
                let start_x = ctx.size().width - after;
                scene.stroke_line(
                    origin + kurbo::Vec2::new(start_x, mid_y),
                    origin + kurbo::Vec2::new(start_x + after, mid_y),
                    LINE_WIDTH,
                    line_color,
                );
            }
        } else if self.variant == MarkerVariant::Border {
            let y = self.row_height + BORDER_PAD_BOTTOM;
            scene.stroke_line(
                origin + kurbo::Vec2::new(0.0, y),
                origin + kurbo::Vec2::new(ctx.size().width, y),
                LINE_WIDTH,
                line_color,
            );
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, BezPath, Brush};
    use std::any::Any;

    /// A minimal recording scene: just the two calls this widget's paint
    /// actually makes (stroked hairlines; child text/backgrounds are opaque
    /// to this test).
    #[derive(Default)]
    struct LineRecorder {
        lines: Vec<(Point, Point)>,
    }

    impl PaintScene for LineRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn stroke_line(&mut self, p0: Point, p1: Point, _width: f64, _color: Color) {
            self.lines.push((p0, p1));
        }
        fn push_transform(&mut self, _t: Affine) {}
    }

    fn build<S: 'static>(view: &MarkerView<S>) -> MarkerWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut MarkerWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 100.0)))
    }

    fn paint(w: &mut MarkerWidget, size: Size) -> LineRecorder {
        let mut rec = LineRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn default_variant_paints_no_lines() {
        let view: MarkerView<()> = marker("Section");
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        let rec = paint(&mut w, size);
        assert!(rec.lines.is_empty());
    }

    #[test]
    fn separator_variant_flanks_the_content_with_two_lines() {
        let view: MarkerView<()> = marker("OR").variant(MarkerVariant::Separator);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        let rec = paint(&mut w, size);
        assert_eq!(rec.lines.len(), 2, "one hairline on each side");
    }

    #[test]
    fn border_variant_paints_one_line_under_the_row_with_the_pb_2_gap() {
        let view: MarkerView<()> = marker("Section").variant(MarkerVariant::Border);
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        assert_eq!(size.height, w.row_height + BORDER_PAD_BOTTOM);
        assert!(size.height >= MIN_HEIGHT + BORDER_PAD_BOTTOM);
        let rec = paint(&mut w, size);
        assert_eq!(rec.lines.len(), 1);
    }

    #[test]
    fn row_height_is_at_least_min_h_4() {
        let view: MarkerView<()> = marker("x");
        let mut w = build(&view);
        let size = layout(&mut w, 200.0);
        assert!(size.height >= MIN_HEIGHT);
    }

    #[test]
    fn an_icon_slot_is_placed_before_the_content() {
        let view: MarkerView<()> =
            marker("Step").icon(frust_widgets::test_support::leaf(10.0, 10.0));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        assert_eq!(w.children.len(), 2);
        assert_eq!(w.children[0].origin().x, 0.0);
        assert!(w.children[1].origin().x > w.children[0].origin().x);
    }

    // ---- Typeface: the content follows the live theme ---------------------

    /// A separator marker: two hairlines (paths, no glyphs) around the content.
    #[cfg(feature = "bundled-fonts")]
    fn divider(_: &mut ()) -> MarkerView<()> {
        marker("Or continue with").variant(MarkerVariant::Separator)
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_content_paints_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face("a marker's content", divider);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_content_follows_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a marker's content",
            divider,
        );
    }
}
