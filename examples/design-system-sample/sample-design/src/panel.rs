//! [`sample_panel`]/[`SamplePanelView`]: the design system's **single-child
//! container** — a padded surface card with a hairline border and a highlight
//! rule along its top edge.
//!
//! The second of the three authoring shapes (leaf: [`crate::badge`];
//! interactive: [`crate::chip`]). What a container has to get right, and what
//! this one therefore demonstrates:
//!
//! - the child lifecycle through the toolkit's
//!   [`build_child`]/[`rebuild_child`]/[`teardown_child`], never by touching
//!   `frust-core` primitives;
//! - `Widget::visit_children` via the `visit_children!` macro — a container
//!   that skips this is invisible to `WidgetTree::inspect`/devtools even though
//!   its children exist in the retained tree;
//! - `Widget::semantics` forwarding through
//!   [`ChildPod::semantics_child`](frust::authoring::ChildPod::semantics_child),
//!   here as a real `push_container` node so the panel groups its contents;
//! - event routing through
//!   [`route_event_single`](frust::authoring::route_event_single) rather than a
//!   hand-rolled hit test, which is what keeps a captured gesture reaching the
//!   child after the pointer has left its bounds.
//!
//! # Token resolution
//!
//! Fill/border resolve from the `ColorScheme`
//! (`surface_container_low`/`outline`), the radius from `theme.shape.medium`,
//! and the top rule from the
//! [`SampleAccents`](crate::tokens::SampleAccents) extension — all with an
//! unthemed fallback constant apiece, re-resolved every paint.

use frust::Theme;
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Rect, Role, RoundedRect, SemanticsCtx,
    Shape, Size, Vec2, View, Widget, any, build_child, rebuild_child, route_event_single,
    teardown_child, visit_children,
};

use crate::tokens::SampleAccents;

/// Padding around the child, in logical px (no `Theme` spacing token exists to
/// resolve a padding from — see [`crate::badge`]'s `PAD_X`).
const PAD: f64 = 12.0;
/// Height of the highlight rule along the top edge, in logical px.
const RULE_HEIGHT: f64 = 3.0;
/// Hairline border width, in logical px.
const BORDER_WIDTH: f64 = 1.0;
/// Flattening tolerance for the border's rounded-rect stroke path.
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed fallback fill (the Sample light-mode surface).
const FALLBACK_FILL: Color = Color::from_rgb8(0xFA, 0xF7, 0xF2);
/// Unthemed fallback border.
const FALLBACK_BORDER: Color = Color::from_rgb8(0xD8, 0xD1, 0xC5);
/// Unthemed fallback corner radius, in logical px.
const FALLBACK_RADIUS: f64 = 3.0;

/// A declarative Sample panel wrapping one child. See the [module docs](self).
pub struct SamplePanelView<State: 'static> {
    child: AnyView<State>,
}

/// Wrap `child` in a Sample panel.
pub fn sample_panel<State: 'static, V: View<State>>(child: V) -> SamplePanelView<State> {
    SamplePanelView { child: any(child) }
}

/// The retained widget for a [`SamplePanelView`].
pub struct SamplePanelWidget {
    child: ChildPod,
}

impl<State: 'static> View<State> for SamplePanelView<State> {
    type Element = SamplePanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SamplePanelWidget {
        SamplePanelWidget {
            child: build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SamplePanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut SamplePanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

/// `(fill, border)` for the current theme, or the fallback constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.surface_container_low, scheme.outline)
        }
        None => (FALLBACK_FILL, FALLBACK_BORDER),
    }
}

/// The panel's corner radius: `theme.shape.medium`, else the fallback.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(FALLBACK_RADIUS, |t| t.shape.medium)
}

/// The child's inset from the panel's own origin: uniform padding, plus the
/// top rule's height on the leading edge.
fn child_offset() -> Vec2 {
    Vec2::new(PAD, PAD + RULE_HEIGHT)
}

impl Widget for SamplePanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let offset = child_offset();
        let chrome = Size::new(offset.x + PAD, offset.y + PAD);
        let child_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(
                (bc.max().width - chrome.width).max(0.0),
                (bc.max().height - chrome.height).max(0.0),
            ),
        );
        let child_size = self.child.layout_child(ctx, &child_bc);
        self.child.set_origin(offset.to_point());
        bc.constrain(Size::new(
            child_size.width + chrome.width,
            child_size.height + chrome.height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (fill, border) = resolve_colors(theme);
        let radius = resolve_radius(theme);
        let highlight = SampleAccents::resolve_highlight(None, theme);

        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        // The highlight rule, inset by the radius so it reads as a rule rather
        // than a clipped band poking out of the rounded corners.
        scene.fill_rect(
            ctx.origin() + Vec2::new(radius, 0.0),
            Size::new((ctx.size().width - radius * 2.0).max(0.0), RULE_HEIGHT),
            highlight,
        );
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, ctx.size()), radius);
        scene.stroke_path(
            ctx.origin(),
            &rr.to_path(PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(border),
        );

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Never re-hit-test a captured child by hand: this helper owns the
        // capture/focus fast paths and the blur-on-outside-tap rule.
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A grouping node that still forwards: dropping the forward would take
        // the child's whole subtree out of the accessibility tree with no
        // compile-time or test signal.
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| self.child.semantics_child(ctx),
        );
    }

    // The read-only introspection seam — one line naming every pod this
    // container owns.
    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::badge::sample_badge;
    use crate::testing::{RecordingScene, layout_widget, paint_widget};
    use crate::tokens::sample_theme;

    fn build() -> SamplePanelWidget {
        let view: SamplePanelView<()> = sample_panel(sample_badge("ready"));
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn render(widget: &mut SamplePanelWidget, theme: Option<&Theme>) -> (Size, RecordingScene) {
        let size = layout_widget(widget, theme, Size::new(400.0, 300.0));
        let scene = paint_widget(widget, size, theme);
        (size, scene)
    }

    #[test]
    fn the_panel_wraps_its_child_with_padding_and_the_rule() {
        let mut w = build();
        let (size, _) = render(&mut w, None);
        let child = w.child.size();
        let offset = child_offset();
        assert_eq!(size.width, child.width + offset.x + PAD);
        assert_eq!(size.height, child.height + offset.y + PAD);
        assert_eq!(w.child.origin(), offset.to_point());
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_constants() {
        let mut w = build();
        let (_, rec) = render(&mut w, None);
        assert_eq!(rec.rounded_rects[0].3, FALLBACK_FILL);
        assert_eq!(rec.rounded_rects[0].2, FALLBACK_RADIUS);
        assert_eq!(rec.strokes[0], FALLBACK_BORDER);
    }

    #[test]
    fn themed_paint_resolves_the_scheme_shape_and_extension() {
        let theme = sample_theme();
        let accents = theme.extension::<SampleAccents>().expect("attached");
        let mut w = build();
        let (_, rec) = render(&mut w, Some(&theme));
        assert_eq!(rec.rounded_rects[0].3, theme.scheme().surface_container_low);
        assert_eq!(rec.rounded_rects[0].2, theme.shape.medium);
        assert_eq!(rec.strokes[0], theme.scheme().outline);
        // The top rule is the only plain `fill_rect` this widget emits.
        assert_eq!(rec.rects[0].2, accents.highlight(theme.brightness));
        assert_eq!(rec.rects[0].1.height, RULE_HEIGHT);
    }

    #[test]
    fn the_child_paints_through_the_panel() {
        // The child badge's own pill (a rounded rect) must appear after the
        // panel's chrome — proof the container actually forwards paint rather
        // than merely reserving space.
        let mut w = build();
        let (_, rec) = render(&mut w, None);
        assert!(
            rec.rounded_rects.len() > 2,
            "panel fill + the child badge's pill and dot, got {}",
            rec.rounded_rects.len()
        );
        assert!(!rec.glyph_colors.is_empty(), "the child's label was shaped");
    }

    #[test]
    fn visit_children_publishes_the_child_pod() {
        let mut w = build();
        let _ = render(&mut w, None);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1, "the panel owns exactly one pod and publishes it");
    }
}
