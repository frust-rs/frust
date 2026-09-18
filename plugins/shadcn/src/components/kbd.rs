//! Ports shadcn/ui's **Kbd**/**KbdGroup** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/kbd.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`) — a small `h-5` key-cap chip
//! (`bg-muted`, `rounded-sm`, `text-xs`) and a gap-1 row helper for chording
//! (`Ctrl` `+` `K`).
//!
//! # Deviation: mono, not the source's own sans
//!
//! The `.tsx` sets `font-sans` (kbd inherits the document body face upstream
//! — shadcn's own site has no special key-cap font). This port deliberately
//! uses [`crate::tokens::mono_family`] instead, by design: a key
//! cap conventionally reads as monospace, and the catalog bundles JetBrains
//! Mono for exactly this seam (`crate` docs, "Fonts") — an intentional
//! improvement over the source rather than a fidelity gap, called out here so
//! a future re-port against upstream doesn't "fix" it back.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, Vec2, View,
    Widget, build_child, rebuild_children, route_event, teardown_child, visit_children,
};

use crate::style::{SPACING_UNIT, TEXT_XS};
use crate::text::Label;
use crate::tokens::mono_family;

/// Key-cap height (`h-5`), in logical px.
const HEIGHT: f64 = 20.0;
/// Horizontal padding inside the cap (`px-1`), in logical px.
const PAD_X: f64 = SPACING_UNIT;
/// Minimum width (`min-w-5`), in logical px — keeps a single-glyph cap
/// (`K`) square-ish rather than needle-thin.
const MIN_WIDTH: f64 = 20.0;
/// Corner radius (`rounded-sm`), in logical px.
const RADIUS: f64 = 4.0;
/// Row gap between chorded keys (`gap-1`), in logical px.
const GROUP_GAP: f64 = SPACING_UNIT;

/// Unthemed fallback fill (a theme resolves this from
/// `colors.surface_container_highest`, shadcn's `--muted`).
const FALLBACK_FILL: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback ink (a theme resolves this from
/// `colors.on_surface_variant`, shadcn's `--muted-foreground`).
const FALLBACK_INK: Color = Color::from_rgb8(0x73, 0x73, 0x73);

/// A declarative shadcn key cap.
pub struct KbdView {
    key: String,
}

/// Create a key cap labelled `key` (e.g. `"K"`, `"⌘"`, `"Esc"`).
pub fn kbd(key: impl Into<String>) -> KbdView {
    KbdView { key: key.into() }
}

/// The retained widget for a [`KbdView`].
pub struct KbdWidget {
    label: Label,
    key_text: String,
}

impl<State: 'static> View<State> for KbdView {
    type Element = KbdWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> KbdWidget {
        KbdWidget {
            label: Label::new(self.key.clone()),
            key_text: self.key.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KbdWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.key != self.key {
            element.label.set_content(self.key.clone());
            element.key_text = self.key.clone();
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// `(fill, ink)` for the current theme, or the fallback constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.surface_container_highest, scheme.on_surface_variant)
        }
        None => (FALLBACK_FILL, FALLBACK_INK),
    }
}

/// The mono label style at the resolved ink.
///
/// Built here rather than nested as a themed `text` child because
/// `ThemeTextColor` has no `on_surface_variant` role paired with a forced mono
/// family — the cap shapes its own run instead (see [`crate::text`]).
fn label_style(ink: Color) -> TextStyle {
    TextStyle {
        // Explicit, not themed: `TypeScale` has no monospace role to resolve from.
        family: mono_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TEXT_XS as f32, ink)
    }
}

impl Widget for KbdWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, ink) = resolve_colors(theme);
        let style = label_style(ink);
        let label_size = self.label.layout(ctx, &style);

        let width = (label_size.width + PAD_X * 2.0).max(MIN_WIDTH);
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (fill, _) = resolve_colors(theme);
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), RADIUS, fill);

        // Text is centered both axes inside the cap (`items-center
        // justify-center` upstream).
        let text_size = self.label.size();
        let label_origin = ctx.origin()
            + Vec2::new(
                (ctx.size().width - text_size.width) / 2.0,
                (ctx.size().height - text_size.height) / 2.0,
            );
        self.label.paint(label_origin, scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Keyboard, |node| {
            node.set_label(self.key_text.as_str());
        });
    }
}

/// A declarative row of chorded key caps (`Ctrl` + `K`), `gap-1` apart.
pub struct KbdGroupView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Wrap `children` (typically [`kbd`] views and inline separator glyphs) in a
/// `gap-1` row.
pub fn kbd_group<State: 'static>(children: Vec<AnyView<State>>) -> KbdGroupView<State> {
    KbdGroupView { children }
}

/// The retained widget for a [`KbdGroupView`].
pub struct KbdGroupWidget {
    children: Vec<ChildPod>,
}

impl<State: 'static> View<State> for KbdGroupView<State> {
    type Element = KbdGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> KbdGroupWidget {
        let children = self.children.iter().map(|v| build_child(v, ctx)).collect();
        KbdGroupWidget { children }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KbdGroupWidget,
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

    fn teardown(&self, element: &mut KbdGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for KbdGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, bc.max().height);
        let mut x = 0.0;
        let mut height: f64 = 0.0;
        for (i, child) in self.children.iter_mut().enumerate() {
            if i > 0 {
                x += GROUP_GAP;
            }
            let size = child.layout_child(ctx, &BoxConstraints::loose(inner_max));
            child.set_origin(Point::new(x, 0.0));
            x += size.width;
            height = height.max(size.height);
        }
        bc.constrain(Size::new(x, height.max(HEIGHT)))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for child in &mut self.children {
            child.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::GenericContainer,
            |_node| {},
            |ctx| {
                for child in &self.children {
                    child.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(children);
}

/// PascalCase alias for [`kbd_group`], for symmetry with the source's
/// `KbdGroup`.
#[allow(non_snake_case)]
pub fn KbdGroup<State: 'static>(children: Vec<AnyView<State>>) -> KbdGroupView<State> {
    kbd_group(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    fn build(view: &KbdView) -> KbdWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut KbdWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let frust::authoring::Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn paint(w: &mut KbdWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn a_single_glyph_cap_clamps_to_the_min_width() {
        let mut w = build(&kbd("K"));
        let size = layout(&mut w, None);
        assert_eq!(size.width, MIN_WIDTH);
        assert_eq!(size.height, HEIGHT);
    }

    #[test]
    fn a_longer_key_widens_past_the_min() {
        let mut w = build(&kbd("Escape"));
        let size = layout(&mut w, None);
        assert!(size.width > MIN_WIDTH);
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_constants() {
        let mut w = build(&kbd("K"));
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, FALLBACK_FILL);
        assert_eq!(rec.rrects[0].2, RADIUS);
        assert_eq!(rec.glyph_colors[0], FALLBACK_INK);
    }

    #[test]
    fn themed_paint_resolves_muted_roles() {
        let theme = crate::tokens::theme();
        let mut w = build(&kbd("K"));
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_highest);
        assert_eq!(rec.glyph_colors[0], theme.scheme().on_surface_variant);
    }

    #[test]
    fn group_lays_out_children_left_to_right_with_the_group_gap() {
        let view: KbdGroupView<()> = kbd_group(vec![any(kbd("Ctrl")), any(kbd("K"))]);
        let mut w = build_group(&view);
        let size = layout_group(&mut w, None);
        assert_eq!(w.children.len(), 2);
        let first_width = w.children[0].size().width;
        assert_eq!(w.children[1].origin().x, first_width + GROUP_GAP);
        assert_eq!(size.height, HEIGHT);
    }

    fn build_group(view: &KbdGroupView<()>) -> KbdGroupWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_group(w: &mut KbdGroupWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    #[test]
    fn group_visit_children_publishes_every_pod() {
        let view: KbdGroupView<()> = kbd_group(vec![any(kbd("Ctrl")), any(kbd("K"))]);
        let mut w = build_group(&view);
        let _ = layout_group(&mut w, None);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 2);
    }
}
