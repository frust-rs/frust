//! The catalog's two retained text runs.
//!
//! A shadcn component that draws leaf text shapes its own run rather than
//! nesting a `frust::text` child: the framework's baseline text widget bakes its
//! color into the layout, and a component wants either a color of its own
//! choosing ([`Label`]) or a color that changes on hover without a relayout
//! ([`LabelRun`]). Both cache the shaped layout and re-shape only when the text
//! or the [`TextStyle`] it was shaped with actually changes.
//!
//! # Which one a component wants
//!
//! - [`Label`] — the color is part of the style, baked in at layout time. The
//!   right choice when the ink only changes on a rebuild (a variant swap, a
//!   disabled flag): `badge`, `button`, `kbd`, `label`, `bubble`.
//! - [`LabelRun`] — the run is shaped with a sentinel ink and every `GlyphRun`
//!   is re-brushed at paint time. The right choice when the ink depends on
//!   something that only ever requests a *repaint* (hover, a selected item),
//!   since a baked color would otherwise lag a relayout behind: `tabs`,
//!   `toggle`, `toggle_group`, `breadcrumb`.

use frust::authoring::{
    Brush, Color, LayoutCtx, PaintScene, Point, Size,
    text::{TextContext, TextLayout, TextStyle},
};

/// The ink a [`LabelRun`] is *shaped* with. Never painted: every run is
/// re-brushed with the state's resolved color, and keeping the shaping color
/// constant keeps the shape cache from missing on a color change.
pub(crate) const SHAPING_INK: Color = Color::BLACK;

/// A cached, lazily-shaped text run whose color is baked into the shaped
/// layout.
pub(crate) struct Label {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl Label {
    /// A run holding `content`, unshaped until the first [`layout`](Self::layout).
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_style: None,
        }
    }

    /// Replace the text, dropping the cached layout only if it actually changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    /// The text this run holds — the accessible name a component's `semantics`
    /// reports, without retaining a second copy of the string.
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
    }

    /// Shape (or reuse) the run in `style` and return its measured size.
    pub(crate) fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_style = Some(style.clone());
        size
    }

    /// Paint the run at `origin` in the color it was shaped with. A run that has
    /// never been laid out paints nothing.
    pub(crate) fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A cached, lazily-shaped text run whose color is applied at paint time.
///
/// The shape/measure half matches [`Label`]; the difference is
/// [`paint`](Self::paint), which re-brushes each `GlyphRun` instead of painting
/// the ink the layout was shaped with — so a hover recolor costs a repaint
/// rather than a relayout. Shape the run with [`SHAPING_INK`] to keep the cache
/// key free of the color.
pub(crate) struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl LabelRun {
    /// A run holding `content`, unshaped until the first [`layout`](Self::layout).
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    /// Replace the text, dropping the cached layout only if it actually changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    /// Shape (or reuse) the run in `style` and return its measured size.
    pub(crate) fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        size
    }

    /// The text this run holds — the accessible name a component's `semantics`
    /// reports, without retaining a second copy of the string.
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
    }

    /// Paint the run at `origin` in `color`, overriding whatever ink it was
    /// shaped with. A run that has never been laid out paints nothing.
    pub(crate) fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for mut run in layout.to_scene_runs(origin) {
                run.brush = Brush::Solid(color);
                scene.draw_glyph_run(run);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{BoxConstraints, scene::GlyphRun};
    use std::any::Any;

    /// Records the ink every glyph run is drawn with.
    #[derive(Default)]
    struct Inks(Vec<Color>);

    impl PaintScene for Inks {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.0.push(color);
            }
        }
    }

    /// A `LayoutCtx` carrying a real text context, the only resource either run
    /// needs to shape.
    fn with_text_ctx<R>(f: impl FnOnce(&mut LayoutCtx) -> R) -> R {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        f(&mut ctx)
    }

    fn style(size: f32, ink: Color) -> TextStyle {
        TextStyle::new(size, ink)
    }

    #[test]
    fn a_label_measures_its_text_and_paints_the_baked_ink() {
        let ink = Color::from_rgb8(0x11, 0x22, 0x33);
        let mut label = Label::new("Save");
        let size = with_text_ctx(|ctx| label.layout(ctx, &style(14.0, ink)));
        assert!(size.width > 0.0 && size.height > 0.0);

        let mut scene = Inks::default();
        label.paint(Point::ZERO, &mut scene);
        assert_eq!(scene.0, vec![ink]);
    }

    #[test]
    fn an_unshaped_run_paints_nothing() {
        let mut scene = Inks::default();
        Label::new("Save").paint(Point::ZERO, &mut scene);
        LabelRun::new("Save").paint(Point::ZERO, Color::WHITE, &mut scene);
        assert!(scene.0.is_empty());
        assert_eq!(LabelRun::new("Save").size(), Size::ZERO);
    }

    #[test]
    fn set_content_invalidates_the_cache_only_on_a_real_change() {
        let s = style(14.0, SHAPING_INK);
        let mut run = LabelRun::new("Tab");
        let first = with_text_ctx(|ctx| run.layout(ctx, &s));

        run.set_content("Tab");
        assert!(run.layout.is_some(), "an identical string keeps the layout");
        run.set_content("A much longer tab label");
        assert!(run.layout.is_none(), "a changed string drops it");

        let second = with_text_ctx(|ctx| run.layout(ctx, &s));
        assert!(second.width > first.width);
    }

    /// The reason [`LabelRun`] exists: recoloring is a paint-time operation, so
    /// the shaped layout must survive it untouched.
    #[test]
    fn recoloring_a_run_reuses_the_shaped_layout() {
        let s = style(14.0, SHAPING_INK);
        let mut run = LabelRun::new("Bold");
        let shaped = with_text_ctx(|ctx| run.layout(ctx, &s));

        for ink in [Color::WHITE, Color::from_rgb8(0xE7, 0x00, 0x0B)] {
            let mut scene = Inks::default();
            run.paint(Point::ZERO, ink, &mut scene);
            assert_eq!(scene.0, vec![ink], "the shaping ink is never painted");
        }
        // Re-laying out in the same style is a cache hit: same size, and the
        // measurement never went back through shaping.
        assert_eq!(with_text_ctx(|ctx| run.layout(ctx, &s)), shaped);
        assert_eq!(run.size(), shaped);
    }

    #[test]
    fn a_style_change_reshapes_but_an_identical_style_does_not() {
        let mut label = Label::new("Save");
        let small = with_text_ctx(|ctx| label.layout(ctx, &style(12.0, SHAPING_INK)));
        let large = with_text_ctx(|ctx| label.layout(ctx, &style(24.0, SHAPING_INK)));
        assert!(large.width > small.width);
        assert_eq!(
            with_text_ctx(|ctx| label.layout(ctx, &style(24.0, SHAPING_INK))),
            large
        );
        // A color-only change is still a style change for a baked-ink `Label`.
        let recolored =
            with_text_ctx(|ctx| label.layout(ctx, &style(24.0, Color::from_rgb8(1, 2, 3))));
        assert_eq!(recolored, large, "same metrics, freshly shaped");

        // Sanity: the sizes are usable as layout input.
        let bc = BoxConstraints::new(Size::ZERO, Size::new(400.0, 400.0));
        assert_eq!(bc.constrain(large), large);
    }
}
