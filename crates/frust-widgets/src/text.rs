//! The `Text` widget: a leaf that lays out and paints a string.
//!
//! [`text`] is the declarative view-fn; it produces a [`TextView`] descriptor
//! that materialises into a retained [`TextWidget`]. The widget shapes its
//! content during the layout pass (via the shared `frust_text::TextContext`
//! threaded through [`LayoutCtx`]) and emits the resulting glyph runs during
//! paint.

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust_text::{
    FontFamily, FontStyle, FontWeight, LineHeight, TextAlign, TextContext, TextLayout,
    TextOverflow, TextStyle,
};
use frust_theme::{Theme, TypeScale};
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
///
/// Part of the widget-authoring toolkit — re-exported as
/// [`crate::authoring::ThemeTextColor`], which is the path a design system
/// outside this crate names it by.
// Every variant names an M3 "on_*" color role (`on_surface`/`on_primary`/
// `on_surface_variant`/`on_primary_container`) — the shared `On` prefix
// reflects the token vocabulary, not a naming smell.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeTextColor {
    /// `colors.on_surface` — the default for standalone/body text.
    OnSurface,
    /// `colors.on_primary` — a label painted over a `primary`-filled surface
    /// (used by [`crate::Button`]).
    OnPrimary,
    /// `colors.on_surface_variant` — a de-emphasized label/caption on the app
    /// surface (a navigation bar's unselected item labels, an app bar's
    /// trailing action slots).
    OnSurfaceVariant,
    /// `colors.on_primary_container` — a label painted over a
    /// `primary_container`-filled surface (an extended FAB's visible label).
    OnPrimaryContainer,
    /// `colors.error` — a label reading as a destructive/error action (used
    /// by [`crate::Button`]'s `ButtonStyle::Danger`).
    Error,
}

/// Which type-scale role a text's font FAMILY resolves from at layout, when
/// opted into with [`TextView::themed_family`] and a theme is active — one
/// variant per [`TypeScale`] slot (15 baseline roles plus their 15
/// `_emphasized` siblings).
///
/// Only the family is taken from the role: size, weight, line height and
/// tracking stay whatever the text view itself set. Resolving it at layout
/// rather than at build is what lets a text follow a live theme swap — an app
/// rewriting its type scale's family repaints every opted-in run in the new
/// face on the next layout, the same way a themed [`ThemeTextColor`] follows a
/// brightness flip. An explicit [`TextView::family`]/[`TextView::style`] always
/// wins (precedence: explicit > theme > fallback); with no theme, or no role
/// set, the style's own family (`FontFamily::SystemUi` by default) is used
/// unchanged.
///
/// Part of the widget-authoring toolkit — re-exported as
/// [`crate::authoring::ThemeTextType`], which is the path a design system
/// outside this crate names it by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeTextType {
    /// `type_scale.display_large`.
    DisplayLarge,
    /// `type_scale.display_medium`.
    DisplayMedium,
    /// `type_scale.display_small`.
    DisplaySmall,
    /// `type_scale.headline_large`.
    HeadlineLarge,
    /// `type_scale.headline_medium`.
    HeadlineMedium,
    /// `type_scale.headline_small`.
    HeadlineSmall,
    /// `type_scale.title_large`.
    TitleLarge,
    /// `type_scale.title_medium`.
    TitleMedium,
    /// `type_scale.title_small`.
    TitleSmall,
    /// `type_scale.body_large`.
    BodyLarge,
    /// `type_scale.body_medium`.
    BodyMedium,
    /// `type_scale.body_small`.
    BodySmall,
    /// `type_scale.label_large`.
    LabelLarge,
    /// `type_scale.label_medium`.
    LabelMedium,
    /// `type_scale.label_small`.
    LabelSmall,
    /// `type_scale.display_large_emphasized`.
    DisplayLargeEmphasized,
    /// `type_scale.display_medium_emphasized`.
    DisplayMediumEmphasized,
    /// `type_scale.display_small_emphasized`.
    DisplaySmallEmphasized,
    /// `type_scale.headline_large_emphasized`.
    HeadlineLargeEmphasized,
    /// `type_scale.headline_medium_emphasized`.
    HeadlineMediumEmphasized,
    /// `type_scale.headline_small_emphasized`.
    HeadlineSmallEmphasized,
    /// `type_scale.title_large_emphasized`.
    TitleLargeEmphasized,
    /// `type_scale.title_medium_emphasized`.
    TitleMediumEmphasized,
    /// `type_scale.title_small_emphasized`.
    TitleSmallEmphasized,
    /// `type_scale.body_large_emphasized`.
    BodyLargeEmphasized,
    /// `type_scale.body_medium_emphasized`.
    BodyMediumEmphasized,
    /// `type_scale.body_small_emphasized`.
    BodySmallEmphasized,
    /// `type_scale.label_large_emphasized`.
    LabelLargeEmphasized,
    /// `type_scale.label_medium_emphasized`.
    LabelMediumEmphasized,
    /// `type_scale.label_small_emphasized`.
    LabelSmallEmphasized,
}

impl ThemeTextType {
    /// The `scale` slot this role names.
    fn slot(self, scale: &TypeScale) -> &TextStyle {
        match self {
            Self::DisplayLarge => &scale.display_large,
            Self::DisplayMedium => &scale.display_medium,
            Self::DisplaySmall => &scale.display_small,
            Self::HeadlineLarge => &scale.headline_large,
            Self::HeadlineMedium => &scale.headline_medium,
            Self::HeadlineSmall => &scale.headline_small,
            Self::TitleLarge => &scale.title_large,
            Self::TitleMedium => &scale.title_medium,
            Self::TitleSmall => &scale.title_small,
            Self::BodyLarge => &scale.body_large,
            Self::BodyMedium => &scale.body_medium,
            Self::BodySmall => &scale.body_small,
            Self::LabelLarge => &scale.label_large,
            Self::LabelMedium => &scale.label_medium,
            Self::LabelSmall => &scale.label_small,
            Self::DisplayLargeEmphasized => &scale.display_large_emphasized,
            Self::DisplayMediumEmphasized => &scale.display_medium_emphasized,
            Self::DisplaySmallEmphasized => &scale.display_small_emphasized,
            Self::HeadlineLargeEmphasized => &scale.headline_large_emphasized,
            Self::HeadlineMediumEmphasized => &scale.headline_medium_emphasized,
            Self::HeadlineSmallEmphasized => &scale.headline_small_emphasized,
            Self::TitleLargeEmphasized => &scale.title_large_emphasized,
            Self::TitleMediumEmphasized => &scale.title_medium_emphasized,
            Self::TitleSmallEmphasized => &scale.title_small_emphasized,
            Self::BodyLargeEmphasized => &scale.body_large_emphasized,
            Self::BodyMediumEmphasized => &scale.body_medium_emphasized,
            Self::BodySmallEmphasized => &scale.body_small_emphasized,
            Self::LabelLargeEmphasized => &scale.label_large_emphasized,
            Self::LabelMediumEmphasized => &scale.label_medium_emphasized,
            Self::LabelSmallEmphasized => &scale.label_small_emphasized,
        }
    }
}

/// A declarative description of a run of text.
///
/// Content does not read application state in v0 — `app_logic` interpolates the
/// string and hands the finished text in. Styling is applied with the
/// [`TextView::size`]/[`TextView::color`]/[`TextView::weight`]/
/// [`TextView::family`]/[`TextView::italic`]/[`TextView::letter_spacing`]/
/// [`TextView::line_height`]/[`TextView::align`] builder methods, or in bulk
/// with [`TextView::style`]. [`TextView::themed_role`] and
/// [`TextView::themed_family`] instead defer the color and the font family to
/// the active theme, resolved at layout.
///
/// [`TextView::align`] positions each wrapped line within the layout's
/// width (centre/right-aligned paragraphs, not just the block's own
/// position within its parent). It applies to this static leaf only; a
/// live-edited `TextInput`'s text is always start-aligned — see
/// `frust_text::TextEditor`'s docs.
pub struct TextView {
    content: String,
    style: TextStyle,
    /// Whether the app set the glyph color explicitly (via `.color()`/`.style()`).
    /// When `false` and a theme is active, the color resolves from `role`; an
    /// explicit color always wins (explicit > theme > fallback).
    color_explicit: bool,
    /// The themed default color role used when `color_explicit` is `false`.
    role: ThemeTextColor,
    /// Whether the app set the font family explicitly (via
    /// `.family()`/`.style()`). When `true`, `family_role` is never consulted.
    family_explicit: bool,
    /// The type-scale role the family resolves from at layout, set via
    /// [`Self::themed_family`]. `None` (default) keeps the style's own family.
    family_role: Option<ThemeTextType>,
    /// The maximum number of lines to render, set via [`Self::max_lines`].
    /// `None` (default) is unbounded — today's behavior.
    max_lines: Option<usize>,
    /// How content past `max_lines` is handled, set via [`Self::overflow`].
    /// Only meaningful alongside `max_lines`; defaults to
    /// [`TextOverflow::Clip`], a no-op with no `max_lines` set.
    overflow: TextOverflow,
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
        family_explicit: false,
        family_role: None,
        max_lines: None,
        overflow: TextOverflow::default(),
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

    /// Set the font family (or fallback stack). Marks the family as explicitly
    /// set, so it wins over any [`Self::themed_family`] role.
    pub fn family(mut self, family: FontFamily) -> Self {
        self.style.family = family;
        self.family_explicit = true;
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

    /// Set the paragraph alignment (start/center/end/left/right/justify).
    ///
    /// Only visible once the text wraps to more than one line under a
    /// bounded width — a single-line layout's width already equals the
    /// line's own content width, so every alignment renders identically to
    /// the default ([`TextAlign::Start`]).
    pub fn align(mut self, align: TextAlign) -> Self {
        self.style.align = align;
        self
    }

    /// Replace the whole style in one call. Treated as an explicit color and
    /// family choice (the supplied style carries its own of both), so it wins
    /// over the themed color default and any [`Self::themed_family`] role.
    pub fn style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self.color_explicit = true;
        self.family_explicit = true;
        self
    }

    /// Cap the rendered line count. Content past the limit is handled per
    /// [`Self::overflow`] (default [`TextOverflow::Clip`]: extra lines are
    /// dropped, nothing else changes). `0` renders nothing.
    pub fn max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = Some(max_lines);
        self
    }

    /// Set how content past [`Self::max_lines`] is handled. A no-op without
    /// `max_lines` set — there is nothing to overflow past.
    pub fn overflow(mut self, overflow: TextOverflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Set the themed default color role used when no explicit color was set
    /// (e.g. [`crate::Button`] labels its text [`ThemeTextColor::OnPrimary`] so it
    /// reads against the primary-filled button).
    ///
    /// Part of the widget-authoring toolkit (see [`crate::authoring`]): it stays
    /// an inherent method here — an inherent impl cannot be added from another
    /// crate — while the role enum itself is re-exported from `authoring`.
    pub fn themed_role(mut self, role: ThemeTextColor) -> Self {
        self.role = role;
        self
    }

    /// Resolve the font family from the active theme's `type_scale` slot
    /// `role` at layout — the family only; size, weight, line height and
    /// tracking stay as this view sets them (see [`ThemeTextType`]).
    ///
    /// Opt-in: a text without it keeps its style's own family. An explicit
    /// [`Self::family`]/[`Self::style`] wins regardless of call order, and an
    /// explicit [`Self::color`] does not affect it — color and family resolve
    /// independently. With no theme threaded in, the style's own family is
    /// used unchanged.
    ///
    /// Part of the widget-authoring toolkit (see [`crate::authoring`]),
    /// alongside [`Self::themed_role`].
    pub fn themed_family(mut self, role: ThemeTextType) -> Self {
        self.family_role = Some(role);
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
            family_explicit: self.family_explicit,
            family_role: self.family_role,
            max_lines: self.max_lines,
            overflow: self.overflow,
            layout: None,
            laid_out_max_width: None,
            laid_out_style: None,
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
        // The themed family is resolved at layout and shaped with, exactly like
        // the themed color above — a role change must reshape.
        if prev.family_explicit != self.family_explicit || prev.family_role != self.family_role {
            element.family_explicit = self.family_explicit;
            element.family_role = self.family_role;
            element.layout = None;
            flags |= ChangeFlags::LAYOUT;
        }
        // Truncation inputs feed into shaping the same way content/style do
        // (`layout_bounded` reshapes on overflow) — a change here must
        // invalidate the cache exactly like those.
        if prev.max_lines != self.max_lines || prev.overflow != self.overflow {
            element.max_lines = self.max_lines;
            element.overflow = self.overflow;
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
    /// Whether the app set the font family explicitly — see [`TextView`].
    family_explicit: bool,
    /// The type-scale role the family resolves from when `family_explicit`
    /// is `false` — see [`TextView::themed_family`].
    family_role: Option<ThemeTextType>,
    /// The maximum rendered line count — see [`TextView::max_lines`].
    max_lines: Option<usize>,
    /// How content past `max_lines` is handled — see [`TextView::overflow`].
    overflow: TextOverflow,
    /// `None` until the first layout pass, or after a content/style change
    /// invalidates it.
    layout: Option<TextLayout>,
    /// The `max_width` the cached [`layout`](Self::layout) was shaped/broken at.
    /// A layout pass with the same width, same effective style, and a live
    /// cache reuses the shaped layout instead of re-shaping — the fix for a
    /// verified defect where `layout` re-shaped unconditionally
    /// every pass (see [`Widget::layout`]).
    laid_out_max_width: Option<f32>,
    /// The effective (themed-color- and themed-family-resolved) style the
    /// cached `layout` was shaped with. Compared alongside
    /// `laid_out_max_width` so a bare theme swap — which re-resolves the baked
    /// glyph color, and an opted-in family, at layout time without a view
    /// change — still forces a re-shape (the layout-time-baked contract, see
    /// `docs/CODE_STANDARDS.md` Theming).
    laid_out_style: Option<TextStyle>,
}

impl TextWidget {
    /// The style to shape with: the app's style, with two independent themed
    /// substitutions when a theme is active — the color replaced by the
    /// [`ThemeTextColor`] role unless the color was set explicitly, and the
    /// family replaced by the [`ThemeTextType`] role's family when one was
    /// opted into and the family was not set explicitly. With no theme the
    /// app's style is returned unchanged, which is what keeps the unthemed
    /// path pixel-identical to a text that never opted in. Resolving here,
    /// during layout, is what lets both follow a live theme swap.
    fn effective_style(&self, theme: Option<&Theme>) -> TextStyle {
        let mut style = self.style.clone();
        let Some(theme) = theme else {
            return style;
        };
        if !self.color_explicit {
            let scheme = theme.scheme();
            style.color = match self.role {
                ThemeTextColor::OnSurface => scheme.on_surface,
                ThemeTextColor::OnPrimary => scheme.on_primary,
                ThemeTextColor::OnSurfaceVariant => scheme.on_surface_variant,
                ThemeTextColor::OnPrimaryContainer => scheme.on_primary_container,
                ThemeTextColor::Error => scheme.error,
            };
        }
        if !self.family_explicit
            && let Some(role) = self.family_role
        {
            style.family = role.slot(&theme.type_scale).family.clone();
        }
        style
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
        // Resolve the themed color and family first so the theme borrow ends
        // before the (mutable) text-context borrow below.
        let style = self.effective_style(Theme::from_layout_ctx(ctx));

        // Reuse the cached shaped layout when nothing that affects shaping has
        // changed since the last pass. `layout` is `Some` only while content /
        // base style / color and family roles are unchanged (rebuild clears it
        // otherwise), so the remaining variables are the wrap width and the
        // effective (themed) style — both compared here. This is the fix for a
        // verified defect where every layout pass re-shaped unconditionally,
        // and it preserves the theme-swap contract: a live appearance flip
        // changes `style.color` (and a type-scale family swap changes an
        // opted-in `style.family`), which mismatches `laid_out_style` and
        // forces a re-shape even without a view change.
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(&style)
        {
            return bc.constrain(cached.size());
        }

        let text_ctx = ctx.text_context::<TextContext>();
        let layout = text_ctx.layout_bounded(
            &self.content,
            &style,
            max_width,
            self.max_lines,
            self.overflow,
        );
        let size = bc.constrain(layout.size());
        self.layout = Some(layout);
        self.laid_out_max_width = max_width;
        self.laid_out_style = Some(style);
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
            .align(TextAlign::Center)
            .color(Color::from_rgb8(1, 2, 3))
            .max_lines(2)
            .overflow(frust_text::TextOverflow::Ellipsis);

        assert_eq!(view.style.size, 20.0);
        assert_eq!(view.style.weight, FontWeight::MEDIUM);
        assert_eq!(view.style.style, FontStyle::Italic);
        assert_eq!(view.style.family, FontFamily::named("Inter"));
        assert_eq!(view.style.letter_spacing, 2.0);
        assert_eq!(view.style.line_height, LineHeight::Absolute(30.0));
        assert_eq!(view.style.align, TextAlign::Center);
        assert_eq!(view.style.color, Color::from_rgb8(1, 2, 3));
        assert_eq!(view.max_lines, Some(2));
        assert_eq!(view.overflow, frust_text::TextOverflow::Ellipsis);
    }

    #[test]
    fn default_max_lines_and_overflow_are_unbounded_clip() {
        let view = text("x");
        assert_eq!(view.max_lines, None);
        assert_eq!(view.overflow, frust_text::TextOverflow::Clip);
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

    // --- Themed color resolution ---

    use frust_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, Widget};
    use frust_scene::GlyphRun;
    use frust_text::TextContext;
    use frust_theme::Theme;
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
        let mut widget = View::<()>::build(&view, &mut frust_core::BuildCtx::new(&mut 0u64));
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
        let theme = Theme::neutral();
        assert_eq!(
            painted_color(text("x"), Some(&theme)),
            theme.scheme().on_surface
        );
    }

    #[test]
    fn on_primary_role_resolves_to_on_primary() {
        let theme = Theme::neutral();
        let view = text("x").themed_role(ThemeTextColor::OnPrimary);
        assert_eq!(painted_color(view, Some(&theme)), theme.scheme().on_primary);
    }

    #[test]
    fn on_surface_variant_role_resolves_to_on_surface_variant() {
        let theme = Theme::neutral();
        let view = text("x").themed_role(ThemeTextColor::OnSurfaceVariant);
        assert_eq!(
            painted_color(view, Some(&theme)),
            theme.scheme().on_surface_variant
        );
    }

    #[test]
    fn on_primary_container_role_resolves_to_on_primary_container() {
        let theme = Theme::neutral();
        let view = text("x").themed_role(ThemeTextColor::OnPrimaryContainer);
        assert_eq!(
            painted_color(view, Some(&theme)),
            theme.scheme().on_primary_container
        );
    }

    #[test]
    fn error_role_resolves_to_error() {
        let theme = Theme::neutral();
        let view = text("x").themed_role(ThemeTextColor::Error);
        assert_eq!(painted_color(view, Some(&theme)), theme.scheme().error);
    }

    #[test]
    fn explicit_color_wins_over_theme() {
        // Precedence: an app-set `.color()` beats the themed default.
        let theme = Theme::neutral();
        let custom = Color::from_rgb8(1, 2, 3);
        assert_eq!(painted_color(text("x").color(custom), Some(&theme)), custom);
    }

    // --- Cached-shape reuse (the verified defect) ---

    #[test]
    fn unchanged_layout_pass_skips_reshaping_entirely() {
        // The verified defect: `TextWidget::layout` re-shaped on every pass. Now
        // an unchanged pass reuses the cached `TextLayout` WITHOUT touching the
        // text context at all — observable as the shape cache seeing exactly one
        // shape and, critically, zero further lookups (`hits == 0`). A non-zero
        // `hits` would mean the widget still called into the context and only
        // the frust-text cache saved it; `hits == 0` proves the widget-level
        // skip.
        let view = text("Hello from Frust");
        let mut widget = View::<()>::build(&view, &mut frust_core::BuildCtx::new(&mut 0u64));
        let mut tcx = TextContext::new();
        let bc = BoxConstraints::loose(Size::new(200.0, 100.0));
        {
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            widget.layout(&mut lctx, &bc);
            widget.layout(&mut lctx, &bc);
            widget.layout(&mut lctx, &bc);
        }
        let stats = tcx.shape_cache_stats();
        assert_eq!(stats.shapes, 1, "shaping must run exactly once");
        assert_eq!(
            stats.hits, 0,
            "an unchanged layout pass must not consult the text context at all"
        );
        assert_eq!(stats.line_breaks, 0);
    }

    #[test]
    fn width_change_rebreaks_without_reshaping() {
        // A width change breaks the widget-level skip (the cached width differs),
        // so it re-lays-out through the context — but the frust-text shape cache
        // reuses the shaping and re-runs line-breaking only.
        let view = text("Hello from Frust, the pure Rust mobile UI toolkit");
        let mut widget = View::<()>::build(&view, &mut frust_core::BuildCtx::new(&mut 0u64));
        let mut tcx = TextContext::new();
        {
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
            widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(120.0, 100.0)));
        }
        let stats = tcx.shape_cache_stats();
        assert_eq!(stats.shapes, 1, "the width change must not re-shape");
        assert_eq!(stats.line_breaks, 1, "the width change re-breaks once");
    }

    // --- Paragraph alignment, end-to-end through the widget ---

    /// Builds, lays out, and paints `view` at `bc`, returning the painted
    /// glyph runs (unlike [`painted_color`], which discards everything but
    /// the brush).
    fn painted_runs(view: TextView, bc: BoxConstraints) -> Vec<GlyphRun> {
        let mut widget = View::<()>::build(&view, &mut frust_core::BuildCtx::new(&mut 0u64));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        widget.layout(&mut lctx, &bc);
        let mut rec = GlyphRunRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, bc.max());
        widget.paint(&mut pctx, &mut rec);
        rec.runs
    }

    /// A recording scene that captures every painted glyph run in full
    /// (unlike [`GlyphRecorder`], which keeps only the brush color).
    #[derive(Default)]
    struct GlyphRunRecorder {
        runs: Vec<GlyphRun>,
    }

    impl PaintScene for GlyphRunRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.runs.push(run);
        }
    }

    /// Groups `runs`' glyphs by line (glyphs on the same line share a `y` —
    /// `frust_text`'s coordinate contract) and returns each line's minimum
    /// `x` (its rendered left edge), in line order.
    fn line_min_x(runs: &[GlyphRun]) -> Vec<f32> {
        let mut by_y: Vec<(f32, f32)> = Vec::new();
        for run in runs {
            for g in &run.glyphs {
                match by_y.iter_mut().find(|(y, _)| (*y - g.y).abs() < 0.01) {
                    Some((_, min_x)) => *min_x = min_x.min(g.x),
                    None => by_y.push((g.y, g.x)),
                }
            }
        }
        by_y.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        by_y.into_iter().map(|(_, x)| x).collect()
    }

    #[test]
    fn text_view_align_centers_and_right_aligns_wrapped_lines() {
        // The end-to-end path the finding names: `Align(CENTER, text(..))`
        // must actually centre every line, not just the block. Asserts on
        // per-line origins (via the painted glyph runs), not on the style
        // merely being set.
        let content = "A\nBBBBBBBBBB";
        let bc = BoxConstraints::loose(Size::new(400.0, 200.0));

        let start_x = line_min_x(&painted_runs(text(content), bc));
        let center_x = line_min_x(&painted_runs(text(content).align(TextAlign::Center), bc));
        let right_x = line_min_x(&painted_runs(text(content).align(TextAlign::Right), bc));

        assert_eq!(start_x.len(), 2, "expected two hard-broken lines");
        assert_eq!(center_x.len(), 2);
        assert_eq!(right_x.len(), 2);

        assert!(
            start_x[0].abs() < 0.5 && start_x[1].abs() < 0.5,
            "default (start) alignment must hug the left edge: {start_x:?}"
        );
        assert!(
            center_x[0] > center_x[1] + 1.0,
            "the shorter line must center further right than the longer one: {center_x:?}"
        );
        assert!(
            right_x[0] > right_x[1] + 1.0,
            "the shorter line's right-aligned left edge must sit further right: {right_x:?}"
        );
    }

    // --- max_lines / TextOverflow, end-to-end through the widget ---

    use frust_text::TextOverflow;

    #[test]
    fn max_lines_truncates_wrapped_content_to_one_line() {
        let content = "Hello from Frust, the pure Rust mobile UI toolkit";
        let bc = BoxConstraints::loose(Size::new(80.0, 200.0));

        let unbounded = line_min_x(&painted_runs(text(content), bc));
        assert!(
            unbounded.len() > 1,
            "fixture sanity: expected this phrase to wrap at 80px, got {} line(s)",
            unbounded.len()
        );

        let bounded = line_min_x(&painted_runs(
            text(content).max_lines(1).overflow(TextOverflow::Ellipsis),
            bc,
        ));
        assert_eq!(
            bounded.len(),
            1,
            "max_lines(1) must render exactly one line"
        );
    }

    #[test]
    fn clip_overflow_also_drops_extra_lines() {
        // Clip is the default overflow — `.max_lines()` alone must already
        // cap the rendered line count, with no `.overflow()` call needed.
        let content = "Hello from Frust, the pure Rust mobile UI toolkit";
        let bc = BoxConstraints::loose(Size::new(80.0, 200.0));

        let clipped = line_min_x(&painted_runs(text(content).max_lines(1), bc));
        assert_eq!(clipped.len(), 1);
    }

    #[test]
    fn changing_max_lines_between_rebuilds_invalidates_the_cached_shape() {
        // `TextWidget::layout`'s fast path skips reshaping when nothing that
        // affects shaping changed; `max_lines`/`overflow` must be wired into
        // that invalidation the same way content/style already are, or a
        // rebuild that only changes the line cap would keep painting the
        // stale (differently-truncated) layout.
        let content = "Hello from Frust, the pure Rust mobile UI toolkit";
        let bc = BoxConstraints::loose(Size::new(80.0, 200.0));

        let view_a = text(content).max_lines(1).overflow(TextOverflow::Ellipsis);
        let mut widget = View::<()>::build(&view_a, &mut frust_core::BuildCtx::new(&mut 0u64));
        {
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            widget.layout(&mut lctx, &bc);
        }

        let view_b = text(content).max_lines(2).overflow(TextOverflow::Ellipsis);
        View::<()>::rebuild(
            &view_b,
            &view_a,
            &mut widget,
            &mut frust_core::BuildCtx::new(&mut 0u64),
        );

        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        widget.layout(&mut lctx, &bc);
        let mut rec = GlyphRunRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, bc.max());
        widget.paint(&mut pctx, &mut rec);
        assert_eq!(
            line_min_x(&rec.runs).len(),
            2,
            "the rebuild must have re-shaped against the new max_lines(2), \
             not replayed the max_lines(1) cache"
        );
    }

    // --- Theme-swap regression ---
    //
    // `effective_style` (above) resolves the themed color at LAYOUT time and
    // bakes it into the cached `TextLayout`'s glyph brush; `paint` only replays
    // the cached layout. Every shell calls layout unconditionally every frame
    // (none gates on `RenderRoot::take_change_flags` today — see
    // `frust_core::app`'s doc comment on that seam), so a bare `set_theme`
    // with no view change must still repaint with the new theme's color. This
    // test drives that exact shell contract end-to-end through a real
    // `RenderRoot` and is deliberately independent of the `set_theme`
    // change-flags fix in `frust-core::app` — temporarily reverting that fix
    // must not break this test, since it never consults `take_change_flags`.
    #[test]
    fn theme_swap_with_no_view_change_repaints_new_glyph_color() {
        use frust_core::{FrameTime, RenderRoot};

        fn logic(_state: &mut ()) -> TextView {
            text("label").themed_role(ThemeTextColor::OnPrimary)
        }

        let mut root: RenderRoot<(), TextView> = RenderRoot::new();
        let mut state = ();

        let mut theme_a = Theme::neutral();
        theme_a.brightness = frust_theme::Brightness::Light;
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
        let mut theme_b = Theme::neutral();
        theme_b.brightness = frust_theme::Brightness::Dark;
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

    // --- Themed family resolution ---
    //
    // Asserted on the painted runs' font bytes, not on the style a builder
    // stored: a family that is set but never reaches shaping is exactly the
    // failure this seam exists to close. One registered test face (Tuffy) and
    // one family nothing registers make a run's bytes say which family it
    // shaped against.
    //
    // Only Tuffy is registered here, never its "Helvetica"-renamed twin:
    // `register_fonts` also publishes process-wide (`frust_text`'s app-font
    // record seeds every later `TextContext`), and `textinput`'s
    // late-registration test needs "Helvetica" unregistered when it starts.

    /// The public-domain subsetted test face `frust-text`'s own registration
    /// tests use (included cross-crate, like `textinput`'s tests do).
    const TUFFY: &[u8] = include_bytes!("../../frust-text/tests/fonts/Tuffy-Subset.ttf");

    /// A family no test registers: it resolves to a host fallback face, whose
    /// bytes cannot be [`TUFFY`]'s (`textinput`'s unregistered-family control).
    const UNREGISTERED: &str = "Frust No Such Family";

    /// A text context with the Tuffy test face registered.
    fn tuffy_context() -> TextContext {
        let mut tcx = TextContext::new();
        tcx.register_fonts(TUFFY.to_vec())
            .expect("the Tuffy test face registers");
        tcx
    }

    /// A neutral theme whose `body_large`/`label_large` slots name the given
    /// families — every other slot keeps the neutral scale's generic stack.
    fn role_theme(body_large: &str, label_large: &str) -> Theme {
        let mut theme = Theme::neutral();
        theme.type_scale.body_large.family = FontFamily::named(body_large);
        theme.type_scale.label_large.family = FontFamily::named(label_large);
        theme
    }

    /// Builds, lays out (with `theme` threaded in when `Some`) and paints
    /// `view` against `tcx`, returning every painted glyph run.
    fn painted_runs_in(
        view: &TextView,
        theme: Option<&Theme>,
        tcx: &mut TextContext,
    ) -> Vec<GlyphRun> {
        let mut widget = View::<()>::build(view, &mut frust_core::BuildCtx::new(&mut 0u64));
        let mut lctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        let mut rec = GlyphRunRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 100.0));
        widget.paint(&mut pctx, &mut rec);
        rec.runs
    }

    /// Whether every run shaped against `face`, with at least one run painted.
    fn all_runs_are(runs: &[GlyphRun], face: &[u8]) -> bool {
        !runs.is_empty() && runs.iter().all(|r| r.font.font().data.as_ref() == face)
    }

    /// Whether no run shaped against `face`, with at least one run painted.
    fn no_run_is(runs: &[GlyphRun], face: &[u8]) -> bool {
        !runs.is_empty() && runs.iter().all(|r| r.font.font().data.as_ref() != face)
    }

    #[test]
    fn a_themed_family_resolves_the_theme_roles_family() {
        let mut tcx = tuffy_context();
        let body = text("Hello").themed_family(ThemeTextType::BodyLarge);
        let label = text("Hello").themed_family(ThemeTextType::LabelLarge);

        // Each role reads its own slot — the mapping is per role, not one
        // family for every opted-in text — so swapping which slot names Tuffy
        // swaps which text shapes in it.
        let body_is_tuffy = role_theme("Tuffy", UNREGISTERED);
        assert!(
            all_runs_are(
                &painted_runs_in(&body, Some(&body_is_tuffy), &mut tcx),
                TUFFY
            ),
            "BodyLarge must shape in the theme's body_large family (Tuffy)"
        );
        assert!(
            no_run_is(
                &painted_runs_in(&label, Some(&body_is_tuffy), &mut tcx),
                TUFFY
            ),
            "LabelLarge must not read the body_large slot"
        );

        let label_is_tuffy = role_theme(UNREGISTERED, "Tuffy");
        assert!(
            all_runs_are(
                &painted_runs_in(&label, Some(&label_is_tuffy), &mut tcx),
                TUFFY
            ),
            "LabelLarge must shape in the theme's label_large family (Tuffy)"
        );
        assert!(
            no_run_is(
                &painted_runs_in(&body, Some(&label_is_tuffy), &mut tcx),
                TUFFY
            ),
            "BodyLarge must not read the label_large slot"
        );

        // Fixture sanity: without opting in, the same text under the same
        // theme never picks up a type-scale family.
        assert!(
            no_run_is(
                &painted_runs_in(&text("Hello"), Some(&body_is_tuffy), &mut tcx),
                TUFFY
            ),
            "a text that never opted in must not pick up a type-scale family"
        );
    }

    #[test]
    fn an_explicit_family_or_style_wins_over_a_themed_family_in_either_order() {
        // The role names a family nothing registers; only the explicit
        // family can make these runs Tuffy.
        let theme = role_theme(UNREGISTERED, UNREGISTERED);
        let mut tcx = tuffy_context();
        let tuffy = FontFamily::named("Tuffy");
        let views = [
            text("Hello")
                .themed_family(ThemeTextType::BodyLarge)
                .family(tuffy.clone()),
            text("Hello")
                .family(tuffy.clone())
                .themed_family(ThemeTextType::BodyLarge),
            text("Hello")
                .style(TextStyle {
                    family: tuffy.clone(),
                    ..TextStyle::default()
                })
                .themed_family(ThemeTextType::BodyLarge),
        ];
        for (i, view) in views.iter().enumerate() {
            let runs = painted_runs_in(view, Some(&theme), &mut tcx);
            assert!(
                all_runs_are(&runs, TUFFY),
                "case {i}: an explicit family must win over the BodyLarge role"
            );
        }
    }

    #[test]
    fn an_explicit_color_does_not_suppress_the_themed_family() {
        // Color and family resolve independently: the explicit color keeps
        // its value AND the family still follows the role.
        let theme = role_theme("Tuffy", UNREGISTERED);
        let mut tcx = tuffy_context();
        let custom = Color::from_rgb8(1, 2, 3);
        let runs = painted_runs_in(
            &text("Hello")
                .color(custom)
                .themed_family(ThemeTextType::BodyLarge),
            Some(&theme),
            &mut tcx,
        );
        assert!(
            all_runs_are(&runs, TUFFY),
            "an explicit color must not stop the family resolving from BodyLarge"
        );
        assert!(
            runs.iter()
                .all(|r| matches!(r.brush, Brush::Solid(c) if c == custom)),
            "the explicit color must still win over the themed color role"
        );
    }

    #[test]
    fn without_a_theme_or_a_role_the_family_is_exactly_the_style_default() {
        // The opt-in property every unthemed app and benchmark relies on.
        let view = text("Hello").themed_family(ThemeTextType::BodyLarge);
        let widget = View::<()>::build(&view, &mut frust_core::BuildCtx::new(&mut 0u64));
        assert_eq!(
            widget.effective_style(None),
            TextStyle::default(),
            "no theme: a themed family must leave the style untouched (SystemUi)"
        );
        assert_eq!(widget.effective_style(None).family, FontFamily::SystemUi);

        let theme = role_theme("Tuffy", "Tuffy");
        let plain = View::<()>::build(&text("Hello"), &mut frust_core::BuildCtx::new(&mut 0u64));
        assert_eq!(
            plain.effective_style(Some(&theme)).family,
            FontFamily::SystemUi,
            "no role: a theme must not change the family of a text that never opted in"
        );

        // And what paints matches a text that never opted in at all.
        let mut tcx = tuffy_context();
        let opted_in = painted_runs_in(&view, None, &mut tcx);
        let never = painted_runs_in(&text("Hello"), None, &mut tcx);
        let faces = |runs: &[GlyphRun]| -> Vec<Vec<u8>> {
            runs.iter()
                .map(|r| r.font.font().data.as_ref().to_vec())
                .collect()
        };
        assert_eq!(faces(&opted_in), faces(&never));
    }

    #[test]
    fn changing_the_family_role_on_rebuild_invalidates_the_cached_shape() {
        let theme = role_theme(UNREGISTERED, "Tuffy");
        let mut tcx = tuffy_context();
        let bc = BoxConstraints::loose(Size::new(200.0, 100.0));
        let paint = |widget: &mut TextWidget, tcx: &mut TextContext| {
            let mut lctx =
                LayoutCtx::with_resources(Some(tcx as &mut dyn Any), Some(&theme as &dyn Any));
            widget.layout(&mut lctx, &bc);
            let mut rec = GlyphRunRecorder::default();
            widget.paint(&mut PaintCtx::new(Point::ZERO, bc.max()), &mut rec);
            rec.runs
        };

        let view_a = text("Hello").themed_family(ThemeTextType::BodyLarge);
        let mut widget = View::<()>::build(&view_a, &mut frust_core::BuildCtx::new(&mut 0u64));
        assert!(no_run_is(&paint(&mut widget, &mut tcx), TUFFY));

        let view_b = text("Hello").themed_family(ThemeTextType::LabelLarge);
        let flags = View::<()>::rebuild(
            &view_b,
            &view_a,
            &mut widget,
            &mut frust_core::BuildCtx::new(&mut 0u64),
        );
        assert!(flags.needs_layout(), "a role change must request layout");
        assert!(
            all_runs_are(&paint(&mut widget, &mut tcx), TUFFY),
            "the rebuild must re-shape in LabelLarge's family (Tuffy), not \
             replay the BodyLarge cache"
        );
    }

    #[test]
    fn a_type_scale_family_swap_with_no_view_change_reshapes() {
        // The live-theme contract a design system's font picker depends on:
        // a bare `set_theme` whose type scale names a new family repaints an
        // opted-in text in that family at the next layout. The effective
        // style — family included — keys both the widget's own cache and the
        // text context's shape cache, so the swap cannot replay stale shaping.
        use frust_core::{FrameTime, RenderRoot};

        fn logic(_state: &mut ()) -> TextView {
            text("label").themed_family(ThemeTextType::BodyLarge)
        }
        #[derive(Default)]
        struct FaceRecorder {
            faces: Vec<Vec<u8>>,
        }
        impl PaintScene for FaceRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn draw_glyph_run(&mut self, run: GlyphRun) {
                self.faces.push(run.font.font().data.as_ref().to_vec());
            }
        }
        let paint_faces = |root: &mut RenderRoot<(), TextView>, tcx: &mut TextContext| {
            root.layout_with_text(Size::new(200.0, 100.0), tcx as &mut dyn Any);
            let mut rec = FaceRecorder::default();
            root.paint(&mut rec, FrameTime::ZERO);
            rec.faces
        };

        let mut root: RenderRoot<(), TextView> = RenderRoot::new();
        let mut tcx = tuffy_context();
        root.set_theme(Box::new(role_theme(UNREGISTERED, UNREGISTERED)));
        root.rebuild(&mut logic, &mut ());
        let before = paint_faces(&mut root, &mut tcx);
        assert!(!before.is_empty() && before.iter().all(|f| f != TUFFY));

        root.set_theme(Box::new(role_theme("Tuffy", UNREGISTERED)));
        let after = paint_faces(&mut root, &mut tcx);
        assert!(
            !after.is_empty() && after.iter().all(|f| f == TUFFY),
            "a bare type-scale family swap must re-shape the opted-in text in the \
             new family"
        );
    }
}
