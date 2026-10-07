//! Ports shadcn/ui's **Alert**/**AlertTitle**/**AlertDescription** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/alert.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a `rounded-lg border px-4
//! py-3` panel, `variant` × 2 (`default`/`destructive`), with an optional
//! leading icon column and a title/description content stack.
//!
//! # Grid → manual two-column layout
//!
//! The source is a CSS grid (`grid-cols-[0_1fr]`, widening to `[16px_1fr]`
//! when an icon is present) with `gap-y-0.5` between every direct child row.
//! This port reproduces it by hand: [`alert`]'s `children` are stacked
//! vertically at a fixed left inset (0 with no icon, `16 + gap-x-3` with
//! one), [`GAP_Y`] apart — the same manual grid-as-stack technique this
//! catalog's `card` uses for its own slot family.
//!
//! # Ink inheritance
//!
//! Upstream's title/icon carry no color class of their own — they inherit
//! the alert's own `text-card-foreground`/`text-destructive`. Since a shaped
//! text run bakes its color at layout time (no CSS inheritance to lean on),
//! [`alert_title`] takes the alert's `variant` explicitly so its ink matches.
//! [`alert_description`] does too, but for a different reason: its own class
//! (`text-muted-foreground`) is overridden *only* under the destructive
//! variant (`*:data-[slot=alert-description]:text-destructive/90`), so the
//! variant selects between two inks rather than being inherited from one.
//! A caller supplying an icon colors it themselves (an arbitrary
//! [`frust::authoring::AnyView`] slot, the same "caller themes its own
//! content" contract this catalog's other multi-slot containers use).

use frust::authoring::text::FontWeight;
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, RoundedRect, SemanticsCtx, Shape,
    Size, View, ViewSeq, Widget, any, build_child, rebuild_child, rebuild_children, route_event,
    route_event_single, teardown_child,
};
use frust::authoring::{ThemeTextColor, ThemeTextType};
use frust::{Theme, text};

use crate::style::{BORDER_WIDTH, PATH_TOLERANCE, with_alpha};
use crate::tokens::ShadcnTokens;

/// Horizontal padding (`px-4`), in logical px.
const PAD_X: f64 = 16.0;
/// Vertical padding (`py-3`), in logical px.
const PAD_Y: f64 = 12.0;
/// Icon column width (`has-[>svg]:grid-cols-[16px_1fr]`), in logical px.
const ICON_COL: f64 = 16.0;
/// Gap between the icon column and content (`gap-x-3`), in logical px.
const ICON_GAP: f64 = 12.0;
/// Vertical gap between content rows (`gap-y-0.5`), in logical px.
const GAP_Y: f64 = 2.0;

/// cva `variant` axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlertVariant {
    /// `bg-card text-card-foreground`.
    #[default]
    Default,
    /// `bg-card text-destructive`, with a `text-destructive/90` description
    /// override (see the [module docs](self)).
    Destructive,
}

/// Create a title row for [`alert`]'s `children`, inheriting `variant`'s base
/// ink (see the [module docs](self)).
pub fn alert_title<State: 'static>(
    text_content: impl Into<String>,
    variant: AlertVariant,
) -> AnyView<State> {
    let role = match variant {
        AlertVariant::Default => ThemeTextColor::OnSurface,
        AlertVariant::Destructive => ThemeTextColor::Error,
    };
    any(text(text_content)
        .weight(FontWeight::MEDIUM)
        .themed_family(ThemeTextType::TitleMedium)
        .themed_role(role))
}

/// Create a description row for [`alert`]'s `children` — `text-muted-foreground`
/// under [`AlertVariant::Default`], `text-destructive/90` under
/// [`AlertVariant::Destructive`] (see the [module docs](self)).
pub fn alert_description<State: 'static>(
    text_content: impl Into<String>,
    variant: AlertVariant,
) -> AnyView<State> {
    match variant {
        AlertVariant::Default => any(text(text_content)
            .themed_family(ThemeTextType::BodyLarge)
            .themed_role(ThemeTextColor::OnSurfaceVariant)),
        AlertVariant::Destructive => {
            // No `ThemeTextColor` role reads as "error at 90% alpha", so this
            // row shapes an explicit color rather than a themed role — the
            // one deliberate exception in this catalog's title/description
            // helpers, since the alpha wash has no `ColorScheme` counterpart.
            let ink = with_alpha(Color::from_rgb8(0xE7, 0x00, 0x0B), 0.9);
            any(text(text_content)
                .themed_family(ThemeTextType::BodyLarge)
                .color(ink))
        }
    }
}

/// A declarative shadcn alert.
pub struct AlertView<State: 'static> {
    variant: AlertVariant,
    icon: Option<AnyView<State>>,
    children: Vec<AnyView<State>>,
}

/// Create an alert of `variant` wrapping `children` (typically
/// [`alert_title`]/[`alert_description`] rows).
///
/// The list is any [`ViewSeq`] — a tuple of mixed view types (`(a, b, c)`),
/// a `Vec`/array of one type, an `Option`, or `views(iter)` — erased once here,
/// so no element needs `any(..)`.
pub fn alert<State: 'static, M>(
    variant: AlertVariant,
    children: impl ViewSeq<State, M>,
) -> AlertView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    AlertView {
        variant,
        icon: None,
        children: erased,
    }
}

impl<State: 'static> AlertView<State> {
    /// Attach a leading icon slot — an arbitrary view the caller themes
    /// itself (see the [module docs](self)).
    pub fn icon(mut self, icon: impl View<State> + 'static) -> Self {
        self.icon = Some(any(icon));
        self
    }
}

/// The retained widget for an [`AlertView`].
pub struct AlertWidget {
    variant: AlertVariant,
    icon: Option<ChildPod>,
    children: Vec<ChildPod>,
}

impl<State: 'static> View<State> for AlertView<State> {
    type Element = AlertWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AlertWidget {
        AlertWidget {
            variant: self.variant,
            icon: self.icon.as_ref().map(|v| build_child(v, ctx)),
            children: self.children.iter().map(|v| build_child(v, ctx)).collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AlertWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        match (&prev.icon, &self.icon, &mut element.icon) {
            (Some(prev_v), Some(next_v), Some(pod)) => {
                flags |= rebuild_child(prev_v, next_v, pod, ctx);
            }
            (None, Some(next_v), slot @ None) => {
                *slot = Some(build_child(next_v, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_v), None, slot) => {
                if let Some(mut pod) = slot.take() {
                    teardown_child(prev_v, &mut pod, ctx);
                }
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }
        flags |= rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        );
        flags
    }

    fn teardown(&self, element: &mut AlertWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(v), Some(pod)) = (&self.icon, &mut element.icon) {
            teardown_child(v, pod, ctx);
        }
        for (v, pod) in self.children.iter().zip(element.children.iter_mut()) {
            teardown_child(v, pod, ctx);
        }
    }
}

/// The alert's fill: themed `colors.surface_container_low` (`bg-card`), else
/// the neutral-preset light fallback.
fn resolve_fill(theme: Option<&Theme>) -> Color {
    theme.map_or(Color::from_rgb8(0xFF, 0xFF, 0xFF), |t| {
        t.scheme().surface_container_low
    })
}

impl Widget for AlertWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let content_x = if self.icon.is_some() {
            ICON_COL + ICON_GAP
        } else {
            0.0
        };
        let inner_max_width = (bc.max().width - PAD_X * 2.0 - content_x).max(0.0);
        let inner_max = Size::new(inner_max_width, f64::INFINITY);

        if let Some(icon) = &mut self.icon {
            icon.layout_child(ctx, &BoxConstraints::loose(Size::new(ICON_COL, ICON_COL)));
            icon.set_origin(Point::new(PAD_X, PAD_Y));
        }

        let mut y = PAD_Y;
        for (i, child) in self.children.iter_mut().enumerate() {
            if i > 0 {
                y += GAP_Y;
            }
            let size = child.layout_child(ctx, &BoxConstraints::loose(inner_max));
            child.set_origin(Point::new(PAD_X + content_x, y));
            y += size.height;
        }

        let height = y + PAD_Y;
        bc.constrain(Size::new(bc.max().width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let fill = resolve_fill(theme);
        let border = theme.map_or(Color::from_rgb8(0xE5, 0xE5, 0xE5), |t| t.scheme().outline);
        let radius = ShadcnTokens::resolve_radius(None, theme).lg;

        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        let half = BORDER_WIDTH / 2.0;
        let size = ctx.size();
        let rr = RoundedRect::new(
            half,
            half,
            size.width - half,
            size.height - half,
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            ctx.origin(),
            &rr.to_path(PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(border),
        );

        if let Some(icon) = &mut self.icon {
            icon.paint_child(ctx, scene);
        }
        for child in &mut self.children {
            child.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let (InputEvent::Pointer(_), Some(icon)) = (event, &mut self.icon) {
            let handled = route_event_single(icon, ctx, event);
            if handled == EventResult::Handled {
                return handled;
            }
        }
        route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Alert,
            |_node| {},
            |ctx| {
                if let Some(icon) = &self.icon {
                    icon.semantics_child(ctx);
                }
                for child in &self.children {
                    child.semantics_child(ctx);
                }
            },
        );
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        if let Some(icon) = &self.icon {
            visitor(icon);
        }
        for child in &self.children {
            visitor(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    fn build(view: &AlertView<()>) -> AlertWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AlertWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)))
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
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
    }

    fn paint(w: &mut AlertWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn build_alert(variant: AlertVariant) -> AlertView<()> {
        alert::<(), _>(
            variant,
            vec![
                alert_title("Heads up", variant),
                alert_description("Something happened.", variant),
            ],
        )
    }

    #[test]
    fn no_icon_insets_content_flush_left() {
        let view = build_alert(AlertVariant::Default);
        let mut w = build(&view);
        let _ = layout(&mut w, None);
        assert_eq!(w.children[0].origin(), Point::new(PAD_X, PAD_Y));
    }

    #[test]
    fn an_icon_widens_the_content_inset() {
        let view = build_alert(AlertVariant::Default).icon(text("!"));
        let mut w = build(&view);
        let _ = layout(&mut w, None);
        assert_eq!(
            w.children[0].origin(),
            Point::new(PAD_X + ICON_COL + ICON_GAP, PAD_Y)
        );
    }

    #[test]
    fn rows_stack_with_the_gap_y_between_them() {
        let view = build_alert(AlertVariant::Default);
        let mut w = build(&view);
        let _ = layout(&mut w, None);
        let title_height = w.children[0].size().height;
        assert_eq!(w.children[1].origin().y, PAD_Y + title_height + GAP_Y);
    }

    #[test]
    fn unthemed_paint_draws_the_border_and_fill() {
        let view = build_alert(AlertVariant::Default);
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].2, ShadcnTokens::resolve_radius(None, None).lg);
        assert!(!rec.strokes.is_empty());
    }

    #[test]
    fn themed_paint_resolves_card_and_outline_roles() {
        let theme = crate::tokens::theme();
        let view = build_alert(AlertVariant::Default);
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_low);
        assert_eq!(rec.strokes[0], theme.scheme().outline);
    }

    #[test]
    fn visit_children_publishes_icon_and_rows() {
        let view = build_alert(AlertVariant::Default).icon(text("!"));
        let mut w = build(&view);
        let _ = layout(&mut w, None);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 3, "icon + title + description");
    }

    // ---- Typeface: the title and description follow the live theme ------

    /// Both variants: the destructive description's explicit color must not
    /// pin its family.
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_paint_in_the_theme_face() {
        for variant in [AlertVariant::Default, AlertVariant::Destructive] {
            crate::text::typeface_probe::assert_paints_in_the_theme_face(
                &format!("a {variant:?} alert's title and description"),
                |_: &mut ()| build_alert(variant),
            );
        }
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_and_description_follow_a_live_theme_swap() {
        for variant in [AlertVariant::Default, AlertVariant::Destructive] {
            crate::text::typeface_probe::assert_follows_a_live_theme_swap(
                &format!("a {variant:?} alert's title and description"),
                |_: &mut ()| build_alert(variant),
            );
        }
    }
}
