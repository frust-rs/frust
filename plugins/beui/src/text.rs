//! The catalog's shared cached text runs.
//!
//! Every text-bearing component in this catalog shapes its own label rather
//! than nesting a `frust::text` child — the framework's baseline text widget
//! bakes its color into the layout, and a component wants either a color of
//! its own choosing ([`Label`]) or a color that changes on a repaint-only
//! event without a relayout ([`LabelRun`]). Both cache the shaped layout and
//! re-shape only when the text or the [`TextStyle`] it was shaped with
//! actually changes — the same contract `plugins/shadcn/src/text.rs`'s
//! `Label`/`LabelRun` pair establishes for that catalog.
//!
//! # Which one a component wants
//!
//! - [`Label`] — the color is part of the style, baked in at layout time.
//!   Used by `input`'s field/message labels (and, through it, `range_slider`'s
//!   and `wheel_picker`'s option labels): a control whose ink only changes on
//!   a rebuild.
//! - [`LabelRun`] — the run is shaped with a sentinel ink
//!   ([`SHAPING_INK`]) and every `GlyphRun` is re-brushed at paint time. Used
//!   by `button` (and, through it, `expanding_arrow_button`,
//!   `expandable_control`, `action_swap`), `animated_badge`, and each of the
//!   seven navigation components (`tabs`, `dock`, `animated_sidebar`,
//!   `bounce_sidebar`, `file_tree`, `bouncy_accordion`, `preview_rail`) — a
//!   control that recolors its text on hover, on a press, or across a state
//!   morph, all repaint-only changes a baked-ink run would otherwise force a
//!   relayout for.
//!
//! [`paint_glyph_run`] is the lower-level primitive both `paint` methods are
//! built on: it also covers `number`'s per-digit runs and
//! `text_animation`'s per-cell run, neither of which caches a whole shaped
//! string the way `Label`/`LabelRun` do (a rolling digit column and a
//! per-character cell each hold their own `TextLayout`s directly).
//!
//! [`label_style`] is `button`'s control-label style — medium weight in the
//! catalog's sans family — shared verbatim with its three siblings above.

use frust::authoring::text::{FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{Brush, Color, LayoutCtx, PaintScene, Point, Size};

/// The ink [`LabelRun`] shapes with. Never painted — keeping it constant keeps
/// the shape cache from missing when only the color changes.
pub(crate) const SHAPING_INK: Color = Color::BLACK;

/// A cached, lazily-shaped text run whose color is baked into the shaped
/// layout.
///
/// Shaping happens in [`layout`](Self::layout) (the only pass carrying a
/// [`TextContext`]) and the color is part of the style, so a color change
/// re-shapes. A run whose color animates should fade through a `push_layer`
/// alpha rather than through its style, or use [`LabelRun`] instead.
pub(crate) struct Label {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl Label {
    /// A run holding `content`, unshaped until the first [`Label::layout`].
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    /// Replace the content, invalidating the cached shape only if it actually
    /// changed. Reports whether anything changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) -> bool {
        let content = content.into();
        if self.content == content {
            return false;
        }
        self.content = content;
        self.layout = None;
        true
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

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Emit this run's glyphs at absolute `origin`, in the ink it was shaped
    /// with. A no-op before the first [`Label::layout`].
    pub(crate) fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A cached, lazily-shaped text run whose color is applied at **paint** time.
///
/// The shape/measure half matches [`Label`]; the difference is
/// [`paint`](Self::paint), which re-brushes each `GlyphRun` instead of
/// painting the ink the layout was shaped with — so a hover recolor or a
/// state morph costs a repaint rather than a relayout. Shape the run with
/// [`SHAPING_INK`] to keep the cache key free of the color.
pub(crate) struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl LabelRun {
    /// A run holding `content`, unshaped until the first [`LabelRun::layout`].
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_style: None,
        }
    }

    /// Replace the content, invalidating the cached shape only if it actually
    /// changed. Reports whether anything changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) -> bool {
        let content = content.into();
        if self.content == content {
            return false;
        }
        self.content = content;
        self.layout = None;
        true
    }

    /// The text this run holds.
    pub(crate) fn content(&self) -> &str {
        &self.content
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

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
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

/// Emit an already-shaped `run`'s glyphs at `origin` under `brush`, overriding
/// whatever ink it was shaped with. A no-op when `run` is `None` — the shape
/// [`number`](super::components::number)'s digit columns and
/// [`text_animation`](super::components::text_animation)'s per-character
/// cells need, since each holds a `Vec` of bare [`TextLayout`]s rather than
/// one cached [`LabelRun`].
pub(crate) fn paint_glyph_run(
    run: Option<&TextLayout>,
    origin: Point,
    brush: &Brush,
    scene: &mut dyn PaintScene,
) {
    let Some(run) = run else { return };
    for mut glyphs in run.to_scene_runs(origin) {
        glyphs.brush = brush.clone();
        scene.draw_glyph_run(glyphs);
    }
}

/// The catalog's control-label text style at `size`, in beUI's own family and
/// the medium weight every control label upstream carries (`font-medium`).
/// Shared by `button` and its three siblings (`expanding_arrow_button`,
/// `expandable_control`, `action_swap`).
pub(crate) fn label_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::sans_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(size as f32, SHAPING_INK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
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

    /// A `LayoutCtx` carrying a real text context, the only resource either
    /// run needs to shape.
    fn with_text_ctx<R>(f: impl FnOnce(&mut LayoutCtx) -> R) -> R {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        f(&mut ctx)
    }

    #[test]
    fn a_label_shapes_once_and_paints_the_baked_ink() {
        let ink = Color::from_rgb8(0x11, 0x22, 0x33);
        let mut label = Label::new("Email");
        let measured = with_text_ctx(|ctx| label.layout(ctx, &label_style_at(14.0, ink)));
        assert!(measured.width > 0.0 && measured.height > 0.0);
        assert_eq!(label.size(), measured);

        let mut scene = Inks::default();
        label.paint(Point::ZERO, &mut scene);
        assert_eq!(scene.0, vec![ink]);

        assert!(label.set_content("Password"), "a real change re-shapes");
        assert!(!label.set_content("Password"), "an identical one does not");
    }

    #[test]
    fn a_label_run_shapes_once_and_repaints_in_any_ink() {
        let mut run = LabelRun::new("Ship");
        let style = label_style(14.0);
        let measured = with_text_ctx(|ctx| run.layout(ctx, &style));
        assert!(measured.width > 0.0);
        assert_eq!(run.size(), measured);
        assert_eq!(run.content(), "Ship");

        let ink = Color::from_rgb8(1, 2, 3);
        let mut rec = Inks::default();
        run.paint(Point::ORIGIN, ink, &mut rec);
        assert!(rec.0.iter().all(|c| *c == ink));

        run.set_content("Ship");
        assert_eq!(run.size(), measured, "an identical string re-uses the run");
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
    fn paint_glyph_run_is_a_no_op_on_an_unshaped_run() {
        let mut scene = Inks::default();
        paint_glyph_run(None, Point::ZERO, &Brush::Solid(Color::WHITE), &mut scene);
        assert!(scene.0.is_empty());
    }

    /// A style for [`Label`], which bakes color into the shape (unlike
    /// [`label_style`], whose color is always [`SHAPING_INK`]).
    fn label_style_at(size: f32, color: Color) -> TextStyle {
        TextStyle {
            family: crate::tokens::sans_family(),
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(size, color)
        }
    }
}
