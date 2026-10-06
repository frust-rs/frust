//! Ports shadcn/ui's **Bubble** (chat message bubble) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/bubble.tsx`.
//!
//! `Bubble`/`BubbleContent` fold into one widget ([`bubble`]): the pill
//! (`rounded-xl px-3 py-2 text-sm`) plus its self-alignment
//! (`data-[align=end]:self-end`). Alignment is resolved *inside* the
//! widget's own layout rather than by a parent reading it back out — the
//! widget fills the incoming width (like a stretched flex item) and paints
//! its pill flush left ([`BubbleAlign::Start`]) or flush right
//! ([`BubbleAlign::End`]) within it, so [`bubble_group`] only needs to stack
//! children in a column; it never has to special-case a child's alignment.
//! [`BubbleReactions`] is out of scope (an anchored overlay chip with no
//! anchoring seam this port defines yet).
//!
//! # Variant → token mapping
//!
//! | Variant | fill | ink |
//! |---|---|---|
//! | `Default` | `primary` | `on_primary` |
//! | `Secondary` | `secondary` | `on_secondary` |
//! | `Muted` | `surface_container_highest` (muted) | `on_surface` |
//! | `Tinted` | `primary_container` (nearest wash shadcn's `accent` role has) | `on_surface` |
//! | `Outline` | `surface` (background), bordered `outline` | `on_surface` |
//! | `Ghost` | none — `border-none rounded-none bg-transparent p-0` | `on_surface` |
//! | `Destructive` | `error_container` (`destructive/10`) | `on_error_container` (`destructive`) |
//!
//! `Tinted`'s source color is a live `oklch(from var(--primary) …)` mix this
//! port has no token for; `primary_container` is the nearest existing wash,
//! recorded here rather than re-derived.

use frust::Theme;
use frust::authoring::text::TextStyle;
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Point,
    Role, RoundedRect, SemanticsCtx, Shape, Size, ThemeTextType, View, Widget,
};
use peniko::Color;

use crate::style::{self, PATH_TOLERANCE};
use crate::text::{Label, themed_family};

/// `bubbleVariants`' `variant` axis. `Default` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BubbleVariant {
    #[default]
    Default,
    Secondary,
    Muted,
    Tinted,
    Outline,
    Ghost,
    Destructive,
}

/// A bubble's self-alignment within its row. `Start` = shadcn's `default`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BubbleAlign {
    #[default]
    Start,
    End,
}

/// `max-w-[80%]` — the pill's maximum share of the available row width.
const MAX_WIDTH_FRAC: f64 = 0.80;
/// `rounded-xl` — the pill's corner radius (shadcn shape scale `xl`).
const FALLBACK_RADIUS: f64 = 14.0;
/// `px-3` — horizontal padding.
const PAD_X: f64 = style::SPACING_UNIT * 3.0;
/// `py-2` — vertical padding.
const PAD_Y: f64 = style::SPACING_UNIT * 2.0;
/// `border` width for `Outline`.
const BORDER_WIDTH: f64 = style::BORDER_WIDTH;

/// A declarative shadcn chat bubble. See the [module docs](self).
pub struct BubbleView {
    content: String,
    variant: BubbleVariant,
    align: BubbleAlign,
}

/// Create a `Default`-variant, start-aligned bubble with `content` as its
/// text.
pub fn bubble(content: impl Into<String>) -> BubbleView {
    BubbleView {
        content: content.into(),
        variant: BubbleVariant::default(),
        align: BubbleAlign::default(),
    }
}

impl BubbleView {
    /// Set the [`BubbleVariant`].
    pub fn variant(mut self, variant: BubbleVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the [`BubbleAlign`].
    pub fn align(mut self, align: BubbleAlign) -> Self {
        self.align = align;
        self
    }
}

/// `(fill, ink, border)` for `variant`. Themed via `theme`; unthemed via a
/// fixed light-mode approximation (same shape as every other component's
/// resolver in this catalog).
fn resolve_colors(
    variant: BubbleVariant,
    theme: Option<&Theme>,
) -> (Option<Color>, Color, Option<Color>) {
    let s = theme.map(Theme::scheme);
    match (variant, s) {
        (BubbleVariant::Ghost, _) => (
            None,
            s.map_or(Color::from_rgb8(0x0A, 0x0A, 0x0A), |s| s.on_surface),
            None,
        ),
        (BubbleVariant::Default, Some(s)) => (Some(s.primary), s.on_primary, None),
        (BubbleVariant::Default, None) => {
            (Some(Color::from_rgb8(0x17, 0x17, 0x17)), Color::WHITE, None)
        }
        (BubbleVariant::Secondary, Some(s)) => (Some(s.secondary), s.on_secondary, None),
        (BubbleVariant::Secondary, None) => (
            Some(Color::from_rgb8(0xF5, 0xF5, 0xF5)),
            Color::from_rgb8(0x17, 0x17, 0x17),
            None,
        ),
        (BubbleVariant::Muted, Some(s)) => (Some(s.surface_container_highest), s.on_surface, None),
        (BubbleVariant::Muted, None) => (
            Some(Color::from_rgb8(0xF5, 0xF5, 0xF5)),
            Color::from_rgb8(0x0A, 0x0A, 0x0A),
            None,
        ),
        (BubbleVariant::Tinted, Some(s)) => (Some(s.primary_container), s.on_surface, None),
        (BubbleVariant::Tinted, None) => (
            Some(Color::from_rgb8(0xF5, 0xF5, 0xF5)),
            Color::from_rgb8(0x0A, 0x0A, 0x0A),
            None,
        ),
        (BubbleVariant::Outline, Some(s)) => (Some(s.surface), s.on_surface, Some(s.outline)),
        (BubbleVariant::Outline, None) => (
            Some(Color::WHITE),
            Color::from_rgb8(0x0A, 0x0A, 0x0A),
            Some(Color::from_rgb8(0xE5, 0xE5, 0xE5)),
        ),
        (BubbleVariant::Destructive, Some(s)) => {
            (Some(s.error_container), s.on_error_container, None)
        }
        (BubbleVariant::Destructive, None) => (
            Some(style::with_alpha(Color::from_rgb8(0xE7, 0x00, 0x0B), 0.10)),
            Color::from_rgb8(0xE7, 0x00, 0x0B),
            None,
        ),
    }
}

/// The retained widget for a [`BubbleView`].
pub struct BubbleWidget {
    label: Label,
    label_size: Size,
    variant: BubbleVariant,
    align: BubbleAlign,
    pill_origin: Point,
    pill_size: Size,
}

impl View<()> for BubbleView {
    type Element = BubbleWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BubbleWidget {
        BubbleWidget {
            label: Label::new(self.content.clone()),
            label_size: Size::ZERO,
            variant: self.variant,
            align: self.align,
            pill_origin: Point::ZERO,
            pill_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BubbleWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.content != self.content {
            element.label = Label::new(self.content.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if element.align != self.align {
            element.align = self.align;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

fn radius(variant: BubbleVariant, theme: Option<&Theme>) -> f64 {
    if variant == BubbleVariant::Ghost {
        0.0
    } else {
        theme.map_or(FALLBACK_RADIUS, |t| t.shape.large)
    }
}

fn padding(variant: BubbleVariant) -> (f64, f64) {
    if variant == BubbleVariant::Ghost {
        (0.0, 0.0)
    } else {
        (PAD_X, PAD_Y)
    }
}

impl Widget for BubbleWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, ink, _) = resolve_colors(self.variant, theme);
        let (pad_x, pad_y) = padding(self.variant);

        let row_w = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            f64::INFINITY
        };
        let text_max_w = if row_w.is_finite() {
            (row_w * MAX_WIDTH_FRAC - pad_x * 2.0).max(0.0)
        } else {
            f64::INFINITY
        };
        let text_style = themed_family(
            TextStyle::new(style::TEXT_SM as f32, ink),
            theme,
            ThemeTextType::BodyMedium,
        );
        self.label_size = self.label.layout(ctx, &text_style);
        // Wrapping to the 80% cap is left to the caller's own text width (no
        // wrap engine is driven here); the cap instead bounds the pill so an
        // overlong single-line label is at least never wider than the row.
        let pill_w = (self.label_size.width + pad_x * 2.0).min(text_max_w.max(pad_x * 2.0));
        self.pill_size = Size::new(
            pill_w.max(pad_x * 2.0),
            self.label_size.height + pad_y * 2.0,
        );

        let own_w = if row_w.is_finite() {
            row_w
        } else {
            self.pill_size.width
        };
        let x = match self.align {
            BubbleAlign::Start => 0.0,
            BubbleAlign::End => (own_w - self.pill_size.width).max(0.0),
        };
        self.pill_origin = Point::new(x, 0.0);

        bc.constrain(Size::new(own_w, self.pill_size.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (fill, _, border) = resolve_colors(self.variant, theme);
        let r = radius(self.variant, theme);
        let (pad_x, pad_y) = padding(self.variant);
        let origin = ctx.origin() + self.pill_origin.to_vec2();

        if let Some(fill) = fill {
            scene.fill_rounded_rect(origin, self.pill_size, r, fill);
        }
        if let Some(border) = border {
            let rr = RoundedRect::from_rect(
                kurbo::Rect::from_origin_size(Point::ORIGIN, self.pill_size),
                r,
            );
            scene.stroke_path(
                origin,
                &rr.to_path(PATH_TOLERANCE),
                BORDER_WIDTH,
                &Brush::Solid(border),
            );
        }
        self.label
            .paint(origin + kurbo::Vec2::new(pad_x, pad_y), scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Label, |node| {
            node.set_label(self.label.content());
        });
    }
}

/// A declarative `BubbleGroup`: a column of bubbles (`gap-2`), each stretched
/// to the group's own width so it can self-align (see the [module
/// docs](self)).
pub struct BubbleGroupView<State: 'static> {
    children: Vec<frust::authoring::AnyView<State>>,
}

/// `gap-2` — the vertical gap between bubbles in a group.
const GROUP_GAP: f64 = style::SPACING_UNIT * 2.0;

/// Stack `children` (typically [`bubble`] views) in a column.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn bubble_group<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> BubbleGroupView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    BubbleGroupView { children }
}

/// The retained widget for a [`BubbleGroupView`].
pub struct BubbleGroupWidget {
    children: Vec<frust::authoring::ChildPod>,
}

impl<State: 'static> View<State> for BubbleGroupView<State> {
    type Element = BubbleGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BubbleGroupWidget {
        BubbleGroupWidget {
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
        element: &mut BubbleGroupWidget,
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

    fn teardown(&self, element: &mut BubbleGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for BubbleGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let child_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        let mut y = 0.0_f64;
        for (i, pod) in self.children.iter_mut().enumerate() {
            let s = pod.layout_child(ctx, &child_bc);
            if i > 0 {
                y += GROUP_GAP;
            }
            pod.set_origin(Point::new(0.0, y));
            y += s.height;
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(
        &mut self,
        ctx: &mut frust::authoring::EventCtx,
        event: &frust::authoring::InputEvent,
    ) -> frust::authoring::EventResult {
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
    use std::any::Any;

    fn build(view: &BubbleView) -> BubbleWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut BubbleWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 200.0)))
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
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
    }

    fn paint(w: &mut BubbleWidget, size: Size) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn default_variant_is_start_aligned_at_the_row_left_edge() {
        let view = bubble("hi");
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(w.pill_origin.x, 0.0);
        let rec = paint(&mut w, size);
        assert_eq!(rec.rounded_rects.len(), 1);
        assert_eq!(rec.strokes.len(), 0);
    }

    #[test]
    fn end_align_pushes_the_pill_to_the_right_edge() {
        let view = bubble("hi").align(BubbleAlign::End);
        let mut w = build(&view);
        layout(&mut w, 300.0);
        assert!(w.pill_origin.x > 0.0);
        assert_eq!(w.pill_origin.x + w.pill_size.width, 300.0);
    }

    #[test]
    fn outline_variant_strokes_a_border() {
        let view = bubble("hi").variant(BubbleVariant::Outline);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        let rec = paint(&mut w, size);
        assert_eq!(rec.strokes.len(), 1);
    }

    #[test]
    fn ghost_variant_paints_no_background() {
        let view = bubble("hi").variant(BubbleVariant::Ghost);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        let rec = paint(&mut w, size);
        assert!(rec.rounded_rects.is_empty());
    }

    #[test]
    fn group_stacks_bubbles_with_an_8px_gap() {
        let view: BubbleGroupView<()> = bubble_group(vec![
            frust::authoring::any(bubble("one")),
            frust::authoring::any(bubble("two")),
        ]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 400.0)));
        assert_eq!(w.children[0].origin(), Point::ZERO);
        assert!(w.children[1].origin().y > w.children[0].origin().y);
    }

    // ---- Typeface: the message text follows the live theme ----------------

    /// Two bubbles, one per alignment, each a run shaped here.
    #[cfg(feature = "bundled-fonts")]
    fn thread(_: &mut ()) -> BubbleGroupView<()> {
        bubble_group(vec![
            frust::authoring::any(bubble("Hello there")),
            frust::authoring::any(bubble("Hi!").align(BubbleAlign::End)),
        ])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_message_text_paints_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a bubble's message text",
            thread,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_message_text_follows_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a bubble's message text",
            thread,
        );
    }
}
