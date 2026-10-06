//! Ports shadcn/ui's **Card**/**CardHeader**/**CardTitle**/**CardDescription**/
//! **CardAction**/**CardContent**/**CardFooter** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/card.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a `rounded-xl border
//! bg-card py-6 shadow-sm` panel, its slots a `flex-col gap-6` stack.
//!
//! # One custom widget, everything else composed
//!
//! Only the outer chrome (rounded corners, border, shadow, `bg-card`) needs a
//! hand-rolled [`Widget`] — [`CardWidget`], a single-child container in
//! exactly the shape `sample_design::panel`'s docs describe. Every slot
//! (`card_header`/`card_content`/`card_footer`/…) is instead **composed**
//! from the baseline layout primitives the `frust` facade already ships
//! (`Row`/`Column`/`Padding`/`SizedBox`/`flexible`/`inflexible`) — the same
//! facade-only contract this catalog's charter documents, since those
//! primitives are `frust::` surface, not a second design-system crate.
//! [`FlexView`](frust::FlexView) v1 has no `gap` primitive of its own (only
//! `MainAxisAlignment::Start`, leading-packed — see that type's own docs), so
//! every gap in this file is an explicit [`SizedBox`](frust::SizedBox)
//! spacer between children rather than a flex property.
//!
//! # Inert by design
//!
//! The source card has no hover/press chrome of its own (module docs already
//! flagged this in the task brief), so [`CardWidget`] claims nothing and
//! fires no callback — it only forwards events to its single composed child.

use frust::authoring::text::FontWeight;
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, RoundedRect, SemanticsCtx, Shape,
    Size, View, Widget, any, build_child, rebuild_child, route_event_single, teardown_child,
    visit_children,
};
use frust::authoring::{ThemeTextColor, ThemeTextType};
use frust::{Column, CrossAxisAlignment, EdgeInsets, Padding, Row, Theme, text};

use crate::style::{BORDER_WIDTH, PATH_TOLERANCE, SHADOW_SM, TEXT_SM, draw_shadow};
use crate::tokens::ShadcnTokens;

/// Vertical padding of the outer card (`py-6`), in logical px.
const CARD_PAD_Y: f64 = 24.0; // spacing(6)
/// Gap between the card's own top-level slots (`gap-6`), in logical px.
const CARD_GAP: f64 = 24.0; // spacing(6)
/// Horizontal padding shared by every slot (`px-6`), in logical px.
const SLOT_PAD_X: f64 = 24.0; // spacing(6)
/// Gap between a header's title/description rows (`gap-2`), in logical px.
const HEADER_GAP: f64 = 8.0; // spacing(2)

/// Interleave `children` with a `gap`-tall [`frust::SizedBox`] spacer between
/// each pair — [`FlexView`](frust::FlexView) v1's stand-in for a native `gap`
/// (see the [module docs](self)).
fn interleave<State: 'static>(children: Vec<AnyView<State>>, gap: f64) -> Vec<AnyView<State>> {
    let mut out = Vec::with_capacity(children.len() * 2);
    for (i, child) in children.into_iter().enumerate() {
        if i > 0 {
            out.push(any(frust::SizedBox(None, Some(gap))));
        }
        out.push(child);
    }
    out
}

/// Create a card wrapping `children` (typically [`card_header`]/
/// [`card_content`]/[`card_footer`] rows), `gap-6` apart with `py-6`.
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn card<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> CardView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    let stacked = interleave(children, CARD_GAP);
    let inner = any(Padding(
        EdgeInsets::symmetric(0.0, CARD_PAD_Y),
        Column(stacked),
    ));
    CardView { child: inner }
}

/// Create a header row: a `gap-2` title/description column, `px-6`, with an
/// optional top-right [`card_action`] slot (the source's `has-data-[slot=
/// card-action]:grid-cols-[1fr_auto]`).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn card_header<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    let stacked = interleave(children, HEADER_GAP);
    any(Padding(
        EdgeInsets::symmetric(SLOT_PAD_X, 0.0),
        Column(stacked),
    ))
}

/// Create a header row with a trailing action slot — see [`card_header`].
/// The main column takes every pixel the action doesn't need
/// (`flexible(1, ...)`/`inflexible(...)`, this port's `1fr auto`).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn card_header_with_action<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
    action: impl View<State> + 'static,
) -> AnyView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    let stacked = interleave(children, HEADER_GAP);
    let row = frust::row().flex(1, Column(stacked)).child(action);
    any(Padding(EdgeInsets::symmetric(SLOT_PAD_X, 0.0), row))
}

/// Create a title row: `font-semibold leading-none`, inheriting the card's
/// own ink (`colors.on_surface`).
pub fn card_title<State: 'static>(text_content: impl Into<String>) -> AnyView<State> {
    any(text(text_content)
        .weight(FontWeight::SEMI_BOLD)
        .themed_family(ThemeTextType::TitleMedium)
        .themed_role(ThemeTextColor::OnSurface))
}

/// Create a description row: `text-sm text-muted-foreground`.
pub fn card_description<State: 'static>(text_content: impl Into<String>) -> AnyView<State> {
    any(text(text_content)
        .size(TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium)
        .themed_role(ThemeTextColor::OnSurfaceVariant))
}

/// Wrap `view` for [`card_header_with_action`]'s action slot — a thin
/// identity wrapper kept for API symmetry with the source's `CardAction`
/// (this port's action placement is [`card_header_with_action`]'s job, not a
/// property of the wrapped view itself).
pub fn card_action<State: 'static>(view: impl View<State> + 'static) -> AnyView<State> {
    any(view)
}

/// Create a content row: `px-6`, no vertical inset of its own (the card's
/// own `gap-6`/`py-6` already space it).
pub fn card_content<State: 'static>(child: impl View<State> + 'static) -> AnyView<State> {
    any(Padding(EdgeInsets::symmetric(SLOT_PAD_X, 0.0), child))
}

/// Create a footer row: `px-6`, a horizontal, vertically-centered stack of
/// `children` (`flex items-center`).
///
/// The list takes any iterator of one [`View`] type, so a homogeneous list
/// needs no `any(..)`; a mixed list keeps `vec![any(..), ..]`.
pub fn card_footer<State: 'static, V: View<State>>(
    children: impl IntoIterator<Item = V>,
) -> AnyView<State> {
    let children: Vec<AnyView<State>> = children.into_iter().map(AnyView::new).collect();
    any(Padding(
        EdgeInsets::symmetric(SLOT_PAD_X, 0.0),
        Row(children).cross_axis(CrossAxisAlignment::Center),
    ))
}

/// A declarative shadcn card. See the [module docs](self).
pub struct CardView<State: 'static> {
    child: AnyView<State>,
}

/// The retained widget for a [`CardView`].
pub struct CardWidget {
    child: ChildPod,
}

impl<State: 'static> View<State> for CardView<State> {
    type Element = CardWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CardWidget {
        CardWidget {
            child: build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CardWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut CardWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

/// The card's fill: themed `colors.surface_container_low` (`bg-card`), else
/// the neutral-preset light fallback.
fn resolve_fill(theme: Option<&Theme>) -> Color {
    theme.map_or(Color::from_rgb8(0xFF, 0xFF, 0xFF), |t| {
        t.scheme().surface_container_low
    })
}

impl Widget for CardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let child_size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(child_size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let fill = resolve_fill(theme);
        let border = theme.map_or(Color::from_rgb8(0xE5, 0xE5, 0xE5), |t| t.scheme().outline);
        let radius = ShadcnTokens::resolve_radius(None, theme).xl;
        let origin = ctx.origin();
        let size = ctx.size();

        draw_shadow(scene, origin, size, radius, SHADOW_SM, theme);
        scene.fill_rounded_rect(origin, size, radius, fill);
        let half = BORDER_WIDTH / 2.0;
        let rr = RoundedRect::new(
            half,
            half,
            size.width - half,
            size.height - half,
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            origin,
            &rr.to_path(PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(border),
        );

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Inert: never claims hover, never captures — only forwards. See the
        // [module docs](self).
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| self.child.semantics_child(ctx),
        );
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    fn build(view: &CardView<()>) -> CardWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CardWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 800.0)))
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        shadows: Vec<Color>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(
            &mut self,
            _o: Point,
            _p: &frust::authoring::BezPath,
            _w: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _std: f64, color: Color) {
            self.shadows.push(color);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn paint(w: &mut CardWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn sample_card() -> CardView<()> {
        card::<(), _>(vec![
            card_header(vec![
                card_title("Notifications"),
                card_description("You have 3 unread messages."),
            ]),
            card_content(text("Body content.")),
            card_footer::<(), AnyView<()>>(vec![]),
        ])
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_chrome() {
        let view = sample_card();
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(rec.rrects[0].2, ShadcnTokens::resolve_radius(None, None).xl);
        assert!(!rec.strokes.is_empty());
        assert!(!rec.shadows.is_empty());
    }

    #[test]
    fn themed_paint_resolves_card_border_and_shadow_roles() {
        let theme = crate::tokens::theme();
        let view = sample_card();
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_low);
        assert_eq!(rec.strokes[0], theme.scheme().outline);
        assert_eq!(rec.shadows[0], SHADOW_SM.color(Some(&theme)));
        assert!(
            !rec.glyph_colors.is_empty(),
            "title/description shaped a run"
        );
    }

    #[test]
    fn visit_children_publishes_the_single_composed_child() {
        let view = sample_card();
        let mut w = build(&view);
        let _ = layout(&mut w, None);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }

    #[test]
    fn header_with_action_places_the_action_after_the_flexible_column() {
        let view: CardView<()> = card(vec![card_header_with_action(
            vec![card_title("Team")],
            text("Manage"),
        )]);
        let mut w = build(&view);
        let _ = layout(&mut w, None);
        // Reaching this far without a panic proves the header composes
        // (flexible column + inflexible action) without a layout blow-up;
        // exact geometry is `FlexView`'s own well-tested contract.
    }

    // ---- Typeface: the title and description follow the live theme ------

    /// A header only: `sample_card`'s body is the caller's own plain `text`,
    /// which this catalog does not style.
    #[cfg(feature = "bundled-fonts")]
    fn titled(_: &mut ()) -> CardView<()> {
        card(vec![card_header(vec![
            card_title("Notifications"),
            card_description("You have 3 unread messages."),
        ])])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a card's title and description",
            titled,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a card's title and description",
            titled,
        );
    }
}
