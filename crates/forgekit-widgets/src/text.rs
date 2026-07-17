//! The `Text` widget (spec §6.4): a leaf that lays out and paints a string.
//!
//! [`text`] is the declarative view-fn; it produces a [`TextView`] descriptor
//! that materialises into a retained [`TextWidget`]. The widget shapes its
//! content during the layout pass (via the shared `forgekit_text::TextContext`
//! threaded through [`LayoutCtx`]) and emits the resulting glyph runs during
//! paint.

use forgekit_core::accesskit::Role;
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use forgekit_text::{
    FontFamily, FontStyle, FontWeight, LineHeight, TextContext, TextLayout, TextStyle,
};
use forgekit_theme::Theme;
use kurbo::Size;
use peniko::Color;

/// Which themed color role a text's glyphs default to when no explicit
/// `.color()`/`.style()` was set and a theme is active.
///
/// A plain [`text`] defaults to [`ThemeTextColor::OnSurface`] (body text on the
/// app surface); a [`crate::Button`]'s label is built with
/// [`ThemeTextColor::OnPrimary`] so it reads correctly against the primary-filled
/// button. The role is only consulted when the color was *not* set explicitly —
/// an app-supplied `.color()` always wins (precedence: explicit > theme >
/// fallback). With no theme threaded in, the style's own color (black by default)
/// is the unthemed fallback.
// Every variant names an M3 "on_*" color role (`on_surface`/`on_primary`/
// `on_surface_variant`) — the shared `On` prefix reflects the token
// vocabulary, not a naming smell; a fourth role (`OnSecondaryContainer`, say)
// would keep the same shape.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ThemeTextColor {
    /// `colors.on_surface` — the default for standalone/body text.
    OnSurface,
    /// `colors.on_primary` — a label painted over a `primary`-filled surface
    /// (used by [`crate::Button`]).
    OnPrimary,
    /// `colors.on_surface_variant` — a de-emphasized label/caption on the app
    /// surface (used by [`crate::material::navbar`]'s unselected item labels
    /// and [`crate::material::appbar`]'s trailing action slots' guidance).
    OnSurfaceVariant,
}

/// A declarative description of a run of text.
///
/// Content does not read application state in v0 — `app_logic` interpolates the
/// string and hands the finished text in (spec §5). Styling is applied with the
/// [`TextView::size`]/[`TextView::color`]/[`TextView::weight`]/
/// [`TextView::family`]/[`TextView::italic`]/[`TextView::letter_spacing`]/
/// [`TextView::line_height`] builder methods, or in bulk with
/// [`TextView::style`].
pub struct TextView {
    content: String,
    style: TextStyle,
    /// Whether the app set the glyph color explicitly (via `.color()`/`.style()`).
    /// When `false` and a theme is active, the color resolves from `role`; an
    /// explicit color always wins (explicit > theme > fallback).
    color_explicit: bool,
    /// The themed default color role used when `color_explicit` is `false`.
    role: ThemeTextColor,
}

/// Create a text view rendering `content` with default styling (16px). Its glyph
/// color defaults to the active theme's `on_surface` role, falling back to black
/// when no theme is set; an explicit [`TextView::color`] overrides both.
pub fn text(content: impl Into<String>) -> TextView {
    TextView {
        content: content.into(),
        style: TextStyle::default(),
        color_explicit: false,
        role: ThemeTextColor::OnSurface,
    }
}

impl TextView {
    /// Set the font size, in logical pixels.
    pub fn size(mut self, size: f32) -> Self {
        self.style.size = size;
        self
    }

    /// Set the glyph fill color. Marks the color as explicitly set, so it wins
    /// over any themed default (explicit > theme > fallback).
    pub fn color(mut self, color: Color) -> Self {
        self.style.color = color;
        self.color_explicit = true;
        self
    }

    /// Set the font weight.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.style.weight = weight;
        self
    }

    /// Set the font family (or fallback stack).
    pub fn family(mut self, family: FontFamily) -> Self {
        self.style.family = family;
        self
    }

    /// Set the font style to italic.
    pub fn italic(mut self) -> Self {
        self.style.style = FontStyle::Italic;
        self
    }

    /// Set the extra spacing between letters, in logical pixels.
    pub fn letter_spacing(mut self, letter_spacing: f32) -> Self {
        self.style.letter_spacing = letter_spacing;
        self
    }

    /// Set the line height.
    pub fn line_height(mut self, line_height: LineHeight) -> Self {
        self.style.line_height = line_height;
        self
    }

    /// Replace the whole style in one call. Treated as an explicit color choice
    /// (the supplied style carries its own color), so it wins over the themed
    /// default.
    pub fn style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self.color_explicit = true;
        self
    }

    /// Set the themed default color role used when no explicit color was set
    /// (crate-internal: [`crate::Button`] labels their text `OnPrimary`).
    pub(crate) fn themed_role(mut self, role: ThemeTextColor) -> Self {
        self.role = role;
        self
    }
}

impl<State: 'static> View<State> for TextView {
    type Element = TextWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TextWidget {
        TextWidget {
            content: self.content.clone(),
            style: self.style.clone(),
            color_explicit: self.color_explicit,
            role: self.role,
            layout: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.content != self.content {
            element.content = self.content.clone();
            element.layout = None; // invalidate the cached shaping
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.style != self.style {
            element.style = self.style.clone();
            element.layout = None;
            flags |= ChangeFlags::LAYOUT;
        }
        // The themed default color feeds into shaping (the glyph brush is baked at
        // layout), so a change to the explicit-flag or role invalidates the cache.
        if prev.color_explicit != self.color_explicit || prev.role != self.role {
            element.color_explicit = self.color_explicit;
            element.role = self.role;
            element.layout = None;
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }
}

/// Retained widget for [`TextView`]: caches the shaped [`TextLayout`] produced
/// during layout for reuse in paint.
pub struct TextWidget {
    content: String,
    style: TextStyle,
    /// Whether the app set the glyph color explicitly — see [`TextView`].
    color_explicit: bool,
    /// The themed default color role used when `color_explicit` is `false`.
    role: ThemeTextColor,
    /// `None` until the first layout pass, or after a content/style change
    /// invalidates it.
    layout: Option<TextLayout>,
}

impl TextWidget {
    /// The style to shape with: the app's style unchanged when the color was set
    /// explicitly (or no theme is active), otherwise the app's style with its
    /// color replaced by the themed [`ThemeTextColor`] role. Resolving the color
    /// here (during layout, where the glyph brush is baked) is what keeps the
    /// unthemed path pixel-identical to before this retrofit.
    fn effective_style(&self, theme: Option<&Theme>) -> TextStyle {
        if self.color_explicit {
            return self.style.clone();
        }
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                let color = match self.role {
                    ThemeTextColor::OnSurface => scheme.on_surface,
                    ThemeTextColor::OnPrimary => scheme.on_primary,
                    ThemeTextColor::OnSurfaceVariant => scheme.on_surface_variant,
                };
                let mut style = self.style.clone();
                style.color = color;
                style
            }
            None => self.style.clone(),
        }
    }
}

impl Widget for TextWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Wrap to the available width when it is bounded (it is, under the
        // root's window-sized loose constraints); an unbounded max width means
        // "lay out on a single line per hard break".
        let max_width = {
            let w = bc.max().width;
            if w.is_finite() { Some(w as f32) } else { None }
        };
        // Resolve the themed color first so the theme borrow ends before the
        // (mutable) text-context borrow below.
        let style = self.effective_style(Theme::from_layout_ctx(ctx));
        let text_ctx = ctx.text_context::<TextContext>();
        let layout = text_ctx.layout(&self.content, &style, max_width);
        let size = bc.constrain(layout.size());
        self.layout = Some(layout);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(ctx.origin()) {
                scene.draw_glyph_run(run);
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A static-text leaf: a Label node. accesskit guidance is that a
        // Role::Label node carries its text in `value` (not `label`), so a
        // screen reader announces the run's content.
        ctx.push_node(Role::Label, |node| {
            node.set_value(self.content.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_methods_compose() {
        let view = text("x")
            .size(20.0)
            .weight(FontWeight::MEDIUM)
            .italic()
            .family(FontFamily::named("Inter"))
            .letter_spacing(2.0)
            .line_height(LineHeight::Absolute(30.0))
            .color(Color::from_rgb8(1, 2, 3));

        assert_eq!(view.style.size, 20.0);
        assert_eq!(view.style.weight, FontWeight::MEDIUM);
        assert_eq!(view.style.style, FontStyle::Italic);
        assert_eq!(view.style.family, FontFamily::named("Inter"));
        assert_eq!(view.style.letter_spacing, 2.0);
        assert_eq!(view.style.line_height, LineHeight::Absolute(30.0));
        assert_eq!(view.style.color, Color::from_rgb8(1, 2, 3));
    }

    #[test]
    fn bulk_style_setter_replaces_whole_style() {
        let custom = TextStyle {
            size: 40.0,
            ..TextStyle::default()
        };
        let view = text("x").style(custom.clone());
        assert_eq!(view.style, custom);
    }

    // --- Themed color resolution (task 07) ---

    use forgekit_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, Widget};
    use forgekit_scene::GlyphRun;
    use forgekit_text::TextContext;
    use forgekit_theme::Theme;
    use kurbo::{Point, Size};
    use peniko::Brush;
    use std::any::Any;

    /// A recording scene that captures each glyph run's solid brush color.
    #[derive(Default)]
    struct GlyphRecorder {
        colors: Vec<Color>,
    }

    impl PaintScene for GlyphRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.colors.push(color);
            }
        }
    }

    /// Lay out and paint `view`, returning the brush color the single glyph run
    /// carried. `theme` is threaded into layout (where the glyph brush is baked)
    /// when `Some`.
    fn painted_color(view: TextView, theme: Option<&Theme>) -> Color {
        let mut widget = View::<()>::build(&view, &mut forgekit_core::BuildCtx::new(&mut 0u64));
        let mut tcx = TextContext::new();
        let theme_any = theme.map(|t| t as &dyn Any);
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme_any);
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        let mut rec = GlyphRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 100.0));
        widget.paint(&mut pctx, &mut rec);
        *rec.colors.first().expect("one glyph run painted")
    }

    #[test]
    fn unthemed_text_keeps_black_default() {
        // Parity: with no theme threaded in, the glyph color is exactly the
        // TextStyle default (black) — unchanged from before the retrofit.
        assert_eq!(painted_color(text("x"), None), Color::BLACK);
    }

    #[test]
    fn themed_text_defaults_to_on_surface() {
        let theme = Theme::m3_baseline();
        assert_eq!(
            painted_color(text("x"), Some(&theme)),
            theme.scheme().on_surface
        );
    }

    #[test]
    fn on_primary_role_resolves_to_on_primary() {
        let theme = Theme::m3_baseline();
        let view = text("x").themed_role(ThemeTextColor::OnPrimary);
        assert_eq!(painted_color(view, Some(&theme)), theme.scheme().on_primary);
    }

    #[test]
    fn on_surface_variant_role_resolves_to_on_surface_variant() {
        let theme = Theme::m3_baseline();
        let view = text("x").themed_role(ThemeTextColor::OnSurfaceVariant);
        assert_eq!(
            painted_color(view, Some(&theme)),
            theme.scheme().on_surface_variant
        );
    }

    #[test]
    fn explicit_color_wins_over_theme() {
        // Precedence: an app-set `.color()` beats the themed default.
        let theme = Theme::m3_baseline();
        let custom = Color::from_rgb8(1, 2, 3);
        assert_eq!(painted_color(text("x").color(custom), Some(&theme)), custom);
    }

    // --- Theme-swap regression (review F1) ---
    //
    // `effective_style` (above) resolves the themed color at LAYOUT time and
    // bakes it into the cached `TextLayout`'s glyph brush; `paint` only replays
    // the cached layout. Every shell calls layout unconditionally every frame
    // (none gates on `RenderRoot::take_change_flags` today — see
    // `forgekit_core::app`'s doc comment on that seam), so a bare `set_theme`
    // with no view change must still repaint with the new theme's color. This
    // test drives that exact shell contract end-to-end through a real
    // `RenderRoot` and is deliberately independent of the `set_theme`
    // change-flags fix in `forgekit-core::app` — temporarily reverting that fix
    // must not break this test, since it never consults `take_change_flags`.
    #[test]
    fn theme_swap_with_no_view_change_repaints_new_glyph_color() {
        use forgekit_core::{FrameTime, RenderRoot};

        fn logic(_state: &mut ()) -> TextView {
            text("label").themed_role(ThemeTextColor::OnPrimary)
        }

        let mut root: RenderRoot<(), TextView> = RenderRoot::new();
        let mut state = ();

        let mut theme_a = Theme::m3_baseline();
        theme_a.brightness = forgekit_theme::Brightness::Light;
        let color_a = theme_a.scheme().on_primary;
        root.set_theme(Box::new(theme_a));
        root.rebuild(&mut logic, &mut state);

        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 100.0), &mut tcx as &mut dyn Any);
        let mut rec = GlyphRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(
            *rec.colors.first().expect("glyph run painted"),
            color_a,
            "sanity: first paint reflects theme A's on_primary role"
        );

        // Swap to a theme whose on_primary genuinely differs (dark scheme), with
        // NO view change (same `logic`, so `rebuild` diffs identical views) —
        // mirroring a live appearance flip. Every shell re-lays-out/repaints
        // unconditionally on the next frame regardless of `rebuild`'s own
        // ChangeFlags, so drive layout/paint again here without a view change.
        let mut theme_b = Theme::m3_baseline();
        theme_b.brightness = forgekit_theme::Brightness::Dark;
        let color_b = theme_b.scheme().on_primary;
        assert_ne!(
            color_a, color_b,
            "fixture sanity: themes must actually differ"
        );
        root.set_theme(Box::new(theme_b));

        root.layout_with_text(Size::new(200.0, 100.0), &mut tcx as &mut dyn Any);
        let mut rec2 = GlyphRecorder::default();
        root.paint(&mut rec2, FrameTime::ZERO);
        assert_eq!(
            *rec2.colors.first().expect("glyph run painted"),
            color_b,
            "a bare theme swap (no view change) must re-resolve the themed glyph \
             color at the next layout, since the color is baked into the cached \
             TextLayout at layout time, not read fresh at paint time"
        );
    }
}
