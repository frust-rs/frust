//! [`sample_badge`]/[`SampleBadgeView`]: the design system's **leaf** widget —
//! a paint-only pill with a highlight dot and a shaped label, no children.
//!
//! This is the first of the three authoring shapes the sample proves an
//! out-of-tree crate can reach through `frust::authoring` alone (leaf here,
//! single-child container in [`crate::panel`], interactive control in
//! [`crate::chip`]).
//!
//! # Token resolution
//!
//! - Fill/ink come from the active [`Theme`]'s `ColorScheme`
//!   (`surface_container`/`on_surface`), with a hairline `outline` border.
//! - The dot resolves through
//!   [`SampleAccents::resolve_highlight`](crate::tokens::SampleAccents::resolve_highlight)
//!   — the system's one written-down precedence ladder: **explicit builder
//!   value ([`SampleBadgeView::highlight`]) > theme extension > fallback
//!   constant**.
//! - The corner radius resolves from `theme.shape.small`, falling back to a
//!   local constant with no theme threaded.
//!
//! Every value is re-resolved each pass — the safe default. The one exception
//! is the label *color*, which is baked into the shaped [`TextLayout`] at
//! LAYOUT time (the framework's layout-time-baked-resolution contract): the
//! resolved color feeds the cached [`TextStyle`] compared at each layout pass,
//! so a theme swap — which forces `ChangeFlags::LAYOUT` — always re-shapes.
//!
//! # Why the label is shaped here rather than nested as a `Text` child
//!
//! `BuildCtx` carries no theme, so a nested baseline `Text` view could only
//! pick from its own fixed role enum; a leaf that wants an arbitrary
//! design-system ink shapes its own run instead. That is exactly what the
//! built-in Glyph badge does, and it is the reason `frust::authoring::text`
//! exists as a public seam.

use frust::Theme;
use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Point,
    Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, Vec2, View, Widget,
};

use crate::tokens::SampleAccents;

/// Horizontal padding inside the pill, in logical px. No `Theme` spacing token
/// exists to resolve a padding from (the framework publishes shape/type
/// scales, not a spacing scale), so this stays a named constant.
const PAD_X: f64 = 10.0;
/// Vertical padding inside the pill, in logical px. See [`PAD_X`].
const PAD_Y: f64 = 5.0;
/// Gap between the highlight dot and the label, in logical px.
const GAP: f64 = 6.0;
/// Highlight-dot diameter, in logical px.
const DOT_SIZE: f64 = 6.0;
/// Label size, in logical px.
const FONT_SIZE: f32 = 12.0;
/// Hairline border width, in logical px.
const BORDER_WIDTH: f64 = 1.0;
/// Flattening tolerance for the border's rounded-rect stroke path (matches the
/// framework's own rounded-rect stroke call sites).
const PATH_TOLERANCE: f64 = 0.1;

// ---- Unthemed fallback constants ------------------------------------------
//
// Every themed value above needs one: a widget must paint *something*
// legible when no theme is threaded into the pass (a unit test, a host that
// never seeded one). These are the Sample light-mode values.

/// Unthemed fallback fill.
const FALLBACK_FILL: Color = Color::from_rgb8(0xF1, 0xEC, 0xE3);
/// Unthemed fallback label ink.
const FALLBACK_INK: Color = Color::from_rgb8(0x1E, 0x1C, 0x19);
/// Unthemed fallback border.
const FALLBACK_BORDER: Color = Color::from_rgb8(0xD8, 0xD1, 0xC5);
/// Unthemed fallback corner radius, in logical px (the Sample shape opinion).
const FALLBACK_RADIUS: f64 = 3.0;

/// A declarative Sample badge. See the [module docs](self).
pub struct SampleBadgeView {
    label: String,
    highlight: Option<Color>,
}

/// Create a badge labelled `label`.
///
/// The dot's highlight hue comes from the theme's
/// [`SampleAccents`](crate::tokens::SampleAccents) extension unless
/// [`SampleBadgeView::highlight`] overrides it.
pub fn sample_badge(label: impl Into<String>) -> SampleBadgeView {
    SampleBadgeView {
        label: label.into(),
        highlight: None,
    }
}

impl SampleBadgeView {
    /// Override the dot's highlight hue — the **explicit** (top) rung of this
    /// system's resolution ladder, winning over the theme extension.
    pub fn highlight(mut self, color: Color) -> Self {
        self.highlight = Some(color);
        self
    }
}

/// A retained, lazily-shaped text run: shapes on first layout and re-shapes
/// whenever the content or the resolved style changes (including a color-only
/// change — that is what makes the layout-time-baked color contract hold).
struct Label {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl Label {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_style: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
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

    /// Emit this run's glyphs at absolute `origin`; a no-op before the first
    /// [`Label::layout`].
    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// The retained widget for a [`SampleBadgeView`].
pub struct SampleBadgeWidget {
    label: Label,
    label_text: String,
    label_size: Size,
    highlight: Option<Color>,
}

impl<State: 'static> View<State> for SampleBadgeView {
    type Element = SampleBadgeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SampleBadgeWidget {
        SampleBadgeWidget {
            label: Label::new(self.label.clone()),
            label_text: self.label.clone(),
            label_size: Size::ZERO,
            highlight: self.highlight,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SampleBadgeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            element.label_text = self.label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.highlight != self.highlight {
            element.highlight = self.highlight;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// `(fill, ink, border)` for the current theme, or the fallback constants with
/// none threaded.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.surface_container, scheme.on_surface, scheme.outline)
        }
        None => (FALLBACK_FILL, FALLBACK_INK, FALLBACK_BORDER),
    }
}

/// The pill's corner radius: `theme.shape.small`, else the fallback constant.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(FALLBACK_RADIUS, |t| t.shape.small)
}

/// The label's style; only `color` varies (the sizes are this system's own
/// authored constants, not theme-resolved).
fn label_style(color: Color) -> TextStyle {
    TextStyle::new(FONT_SIZE, color)
}

impl Widget for SampleBadgeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, ink, _) = resolve_colors(theme);
        // The resolved ink is baked into the shaped run here, at LAYOUT time —
        // see the module docs' note on the theme-swap contract this depends on.
        let style = label_style(ink);
        self.label_size = self.label.layout(ctx, &style);

        let width = PAD_X * 2.0 + DOT_SIZE + GAP + self.label_size.width;
        let height = PAD_Y * 2.0 + self.label_size.height.max(DOT_SIZE);
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (fill, _, border) = resolve_colors(theme);
        let radius = resolve_radius(theme);
        let highlight = SampleAccents::resolve_highlight(self.highlight, theme);

        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, fill);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, ctx.size()), radius);
        scene.stroke_path(
            ctx.origin(),
            &rr.to_path(PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(border),
        );

        let dot_y = (ctx.size().height - DOT_SIZE) / 2.0;
        scene.fill_rounded_rect(
            ctx.origin() + Vec2::new(PAD_X, dot_y),
            Size::new(DOT_SIZE, DOT_SIZE),
            DOT_SIZE / 2.0,
            highlight,
        );

        let label_y = (ctx.size().height - self.label_size.height) / 2.0;
        self.label.paint(
            ctx.origin() + Vec2::new(PAD_X + DOT_SIZE + GAP, label_y),
            scene,
        );
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A live status indicator, not a plain label: `Role::Status` is what a
        // screen reader treats as "state of something", which is what a badge
        // reports.
        ctx.push_node(Role::Status, |node| {
            node.set_label(self.label_text.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{RecordingScene, layout_widget, paint_widget};
    use crate::tokens::{FALLBACK_HIGHLIGHT, sample_theme};

    fn build(highlight: Option<Color>) -> SampleBadgeWidget {
        let mut view = sample_badge("ready");
        if let Some(color) = highlight {
            view = view.highlight(color);
        }
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn render(widget: &mut SampleBadgeWidget, theme: Option<&Theme>) -> RecordingScene {
        let size = layout_widget(widget, theme, Size::new(400.0, 100.0));
        paint_widget(widget, size, theme)
    }

    #[test]
    fn unthemed_paint_uses_the_fallback_constants() {
        let mut w = build(None);
        let rec = render(&mut w, None);
        // Pill fill first, then the highlight dot.
        assert_eq!(rec.rounded_rects[0].3, FALLBACK_FILL);
        assert_eq!(rec.rounded_rects[0].2, FALLBACK_RADIUS);
        assert_eq!(rec.strokes[0], FALLBACK_BORDER);
        assert_eq!(rec.rounded_rects[1].3, FALLBACK_HIGHLIGHT);
        assert_eq!(rec.glyph_colors[0], FALLBACK_INK);
    }

    #[test]
    fn themed_paint_resolves_scheme_roles_and_the_shape_token() {
        let theme = sample_theme();
        let mut w = build(None);
        let rec = render(&mut w, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rounded_rects[0].3, scheme.surface_container);
        assert_eq!(rec.rounded_rects[0].2, theme.shape.small);
        assert_eq!(rec.strokes[0], scheme.outline);
        assert_eq!(rec.glyph_colors[0], scheme.on_surface);
    }

    #[test]
    fn the_dot_resolves_the_theme_extension() {
        let theme = sample_theme();
        let accents = theme.extension::<SampleAccents>().expect("attached");
        let mut w = build(None);
        let rec = render(&mut w, Some(&theme));
        assert_eq!(rec.rounded_rects[1].3, accents.highlight(theme.brightness));
    }

    #[test]
    fn an_explicit_highlight_beats_the_theme_extension() {
        let explicit = Color::from_rgb8(0x12, 0x34, 0x56);
        let theme = sample_theme();
        let mut w = build(Some(explicit));
        let rec = render(&mut w, Some(&theme));
        assert_eq!(rec.rounded_rects[1].3, explicit);
    }

    #[test]
    fn a_theme_swap_reshapes_the_label_with_the_new_ink() {
        // The layout-time-baked color contract: laying the same widget out
        // under a second theme must produce a run in that theme's ink, not the
        // first theme's cached one.
        let light = sample_theme();
        let mut dark = sample_theme();
        dark.brightness = frust::Brightness::Dark;

        let mut w = build(None);
        let first = render(&mut w, Some(&light));
        assert_eq!(first.glyph_colors[0], light.scheme().on_surface);

        let second = render(&mut w, Some(&dark));
        assert_eq!(second.glyph_colors[0], dark.scheme().on_surface);
        assert_ne!(second.glyph_colors[0], first.glyph_colors[0]);
    }
}
