//! The `rich_text` widget: one paragraph of mixed-style text.
//!
//! [`span`] describes a styled piece of text and [`rich_text`] joins pieces
//! into a [`RichTextView`], which materialises into a retained
//! [`RichTextWidget`]. The widget resolves every span's effective style at
//! layout exactly as [`crate::TextView`] does for a single run (themed colour
//! unless explicit, themed family roles, the theme's text scale for spans
//! without an explicit size), shapes the spans as one paragraph, and paints
//! underline rects, glyph runs, then line-through rects.
//!
//! Spans carry no tap or hit-testing of their own.

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust_text::{
    FontFamily, FontStyle, FontWeight, LocaleStamp, OverflowWrap, TextAlign, TextContext,
    TextDecoration, TextLayout, TextOverflow, TextSpan, TextStyle,
};
use frust_theme::Theme;
use kurbo::Size;
use peniko::Color;

use crate::text::{FamilySource, ThemeTextColor, ThemeTextType, paint_layout, resolve_style};

/// A styled piece of text inside a [`RichTextView`].
///
/// Carries the per-span subset of the [`crate::TextView`] builders. Colour,
/// family and size resolve per span with the same precedence as a plain text:
/// an explicit choice wins over the theme, and a span with no explicit size
/// follows the theme's text scale.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanView {
    content: String,
    style: TextStyle,
    color_explicit: bool,
    role: ThemeTextColor,
    family_explicit: bool,
    size_explicit: bool,
    family_source: Option<FamilySource>,
}

/// Create a span rendering `content` with default styling.
pub fn span(content: impl Into<String>) -> SpanView {
    SpanView {
        content: content.into(),
        style: TextStyle::default(),
        color_explicit: false,
        role: ThemeTextColor::OnSurface,
        family_explicit: false,
        size_explicit: false,
        family_source: None,
    }
}

impl SpanView {
    /// Set the font size, in logical pixels. Marks the size as explicit, so
    /// the theme's text scale no longer applies to this span.
    pub fn size(mut self, size: f32) -> Self {
        self.style.size = size;
        self.size_explicit = true;
        self
    }

    /// Set the glyph fill color. Marks the color as explicit, so it wins over
    /// any themed default.
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

    /// Set the font family (or fallback stack). Marks the family as explicit,
    /// so it wins over any [`Self::themed_family`] role.
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

    /// Tag the span with a BCP 47 locale (`"ja"`, `"th"`, `"zh-Hant"`). Touches
    /// only the locale; color, family and size keep resolving as before.
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.style.locale = Some(locale.into());
        self
    }

    /// Set one OpenType feature, appended after any already set; a later
    /// setting of the same tag wins.
    pub fn font_feature(mut self, tag: [u8; 4], value: u16) -> Self {
        self.style.features.push((tag, value));
        self
    }

    /// Replace the OpenType feature settings with `features`.
    pub fn font_features(mut self, features: impl IntoIterator<Item = ([u8; 4], u16)>) -> Self {
        self.style.features = features.into_iter().collect();
        self
    }

    /// Use tabular (fixed-width) figures, the `tnum` feature.
    pub fn tabular_figures(self) -> Self {
        self.font_feature(*b"tnum", 1)
    }

    /// Underline the span, keeping any other decoration already set.
    pub fn underline(mut self) -> Self {
        self.style.decoration.underline = true;
        self
    }

    /// Strike a line through the span, keeping any other decoration already
    /// set.
    pub fn line_through(mut self) -> Self {
        self.style.decoration.line_through = true;
        self
    }

    /// Replace the span's decoration wholesale.
    pub fn decoration(mut self, decoration: TextDecoration) -> Self {
        self.style.decoration = decoration;
        self
    }

    /// Resolve the span's family from the active theme's type-scale slot at
    /// layout, as [`crate::TextView::themed_family`] does.
    pub fn themed_family(mut self, role: ThemeTextType) -> Self {
        self.family_source = Some(FamilySource::Role(role));
        self
    }

    /// Resolve the span's family from the active theme's monospace family at
    /// layout, as [`crate::TextView::themed_mono_family`] does.
    pub fn themed_mono_family(mut self) -> Self {
        self.family_source = Some(FamilySource::Monospace);
        self
    }

    /// Set the themed default color role used when no explicit color was set.
    pub fn themed_role(mut self, role: ThemeTextColor) -> Self {
        self.role = role;
        self
    }

    fn effective_style(&self, theme: Option<&Theme>) -> TextStyle {
        resolve_style(
            &self.style,
            self.color_explicit,
            self.role,
            self.family_explicit,
            self.family_source,
            self.size_explicit,
            theme,
        )
    }
}

/// Create a rich text from `spans`, laid out as one paragraph.
pub fn rich_text(spans: impl IntoIterator<Item = SpanView>) -> RichTextView {
    RichTextView {
        spans: spans.into_iter().collect(),
        align: None,
        overflow_wrap: None,
        max_lines: None,
        overflow: TextOverflow::default(),
    }
}

/// A declarative description of a paragraph of styled [`SpanView`]s.
///
/// The paragraph knobs ([`Self::align`], [`Self::overflow_wrap`],
/// [`Self::max_lines`], [`Self::overflow`]) apply to the whole paragraph. A
/// truncation ellipsis takes the style of the span it cuts.
#[derive(Clone, Debug, PartialEq)]
pub struct RichTextView {
    spans: Vec<SpanView>,
    align: Option<TextAlign>,
    overflow_wrap: Option<OverflowWrap>,
    max_lines: Option<usize>,
    overflow: TextOverflow,
}

impl RichTextView {
    /// Set the paragraph alignment; visible once the text wraps to more than
    /// one line.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = Some(align);
        self
    }

    /// Set what a word wider than the line does (CSS `overflow-wrap`).
    pub fn overflow_wrap(mut self, overflow_wrap: OverflowWrap) -> Self {
        self.overflow_wrap = Some(overflow_wrap);
        self
    }

    /// Cap the rendered line count. Content past the limit is handled per
    /// [`Self::overflow`]. `0` renders nothing.
    pub fn max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = Some(max_lines);
        self
    }

    /// Set how content past [`Self::max_lines`] is handled.
    pub fn overflow(mut self, overflow: TextOverflow) -> Self {
        self.overflow = overflow;
        self
    }
}

impl<State: 'static> View<State> for RichTextView {
    type Element = RichTextWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RichTextWidget {
        RichTextWidget {
            view: self.clone(),
            layout: None,
            laid_out_max_width: None,
            laid_out_span_styles: Vec::new(),
            laid_out_locale_stamp: LocaleStamp::default(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RichTextWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev == self {
            return ChangeFlags::NONE;
        }
        element.view = self.clone();
        element.layout = None;
        ChangeFlags::LAYOUT
    }
}

/// Retained widget for [`RichTextView`]: caches the shaped [`TextLayout`]
/// produced during layout for reuse in paint.
pub struct RichTextWidget {
    view: RichTextView,
    /// `None` until the first layout pass, or after a view change.
    layout: Option<TextLayout>,
    /// The width the cached layout was broken at.
    laid_out_max_width: Option<f32>,
    /// Every span's effective style, so a bare theme swap or text-scale
    /// change re-shapes without a view change. Kept before grouping:
    /// grouping depends on the theme-resolved styles, so two layouts with
    /// equal group styles can still split the spans differently.
    laid_out_span_styles: Vec<TextStyle>,
    /// The locale stamp read just before shaping.
    laid_out_locale_stamp: LocaleStamp,
}

impl RichTextWidget {
    /// Every span's effective style, with the paragraph knobs applied.
    fn effective_styles(&self, theme: Option<&Theme>) -> Vec<TextStyle> {
        self.view
            .spans
            .iter()
            .map(|span| {
                let mut style = span.effective_style(theme);
                if let Some(align) = self.view.align {
                    style.align = align;
                }
                if let Some(wrap) = self.view.overflow_wrap {
                    style.overflow_wrap = wrap;
                }
                style
            })
            .collect()
    }
}

/// A run of adjacent spans shaped as one: the index of the span whose style
/// they all share, and the spans' range.
struct SpanGroup {
    style: usize,
    spans: std::ops::Range<usize>,
}

/// Groups the non-empty spans (`contents`, with effective `styles`) for
/// shaping: empty spans are dropped, adjacent spans of equal style merge, and
/// past `limit` groups the excess folds into the last kept one (its text
/// takes that group's style) instead of overflowing the shaper's span index.
///
/// Text with no non-empty span keeps its first span so the empty layout is
/// still sized by that span's style.
fn group_spans<'a>(
    contents: impl ExactSizeIterator<Item = &'a str>,
    styles: &[TextStyle],
    limit: usize,
) -> Vec<SpanGroup> {
    let mut groups: Vec<SpanGroup> = Vec::new();
    let count = contents.len();
    for (index, content) in contents.enumerate() {
        if content.is_empty() {
            continue;
        }
        let full = groups.len() >= limit;
        match groups.last_mut() {
            Some(last) if styles[last.style] == styles[index] => last.spans.end = index + 1,
            Some(last) if full => last.spans.end = index + 1,
            _ => groups.push(SpanGroup {
                style: index,
                spans: index..index + 1,
            }),
        }
    }
    if groups.is_empty() && count > 0 {
        groups.push(SpanGroup {
            style: 0,
            spans: 0..1,
        });
    }
    groups
}

impl Widget for RichTextWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max_width = {
            let w = bc.max().width;
            if w.is_finite() { Some(w as f32) } else { None }
        };
        let all_styles = self.effective_styles(Theme::from_layout_ctx(ctx));
        let locale_stamp = LocaleStamp::for_style(None);
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_span_styles == all_styles
            && self.laid_out_locale_stamp == locale_stamp
        {
            return bc.constrain(cached.size());
        }

        let groups = group_spans(
            self.view.spans.iter().map(|s| s.content.as_str()),
            &all_styles,
            usize::from(u16::MAX),
        );
        let styles: Vec<TextStyle> = groups.iter().map(|g| all_styles[g.style].clone()).collect();
        let texts: Vec<String> = groups
            .iter()
            .map(|g| {
                self.view.spans[g.spans.clone()]
                    .iter()
                    .map(|s| s.content.as_str())
                    .collect()
            })
            .collect();
        let spans: Vec<TextSpan<'_>> = texts
            .iter()
            .zip(&styles)
            .map(|(text, style)| TextSpan::new(text, style))
            .collect();
        let text_ctx = ctx.text_context::<TextContext>();
        let layout = text_ctx.layout_rich_bounded(
            &spans,
            max_width,
            self.view.max_lines,
            self.view.overflow,
        );
        let size = bc.constrain(layout.size());
        self.layout = Some(layout);
        self.laid_out_max_width = max_width;
        self.laid_out_span_styles = all_styles;
        self.laid_out_locale_stamp = locale_stamp;
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            paint_layout(layout, ctx.origin(), scene);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let value: String = self.view.spans.iter().map(|s| s.content.as_str()).collect();
        ctx.push_node(Role::Label, |node| {
            node.set_value(value);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::GlyphRun;
    use kurbo::Point;
    use peniko::Brush;
    use std::any::Any;

    #[derive(Debug, PartialEq)]
    enum Op {
        Rect(Point, Size, Color),
        Glyphs(Brush, f32),
    }

    #[derive(Default)]
    struct Recorder(Vec<Op>);

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.0.push(Op::Rect(o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.0.push(Op::Glyphs(run.brush, run.font_size));
        }
    }

    fn lay_out(
        view: &RichTextView,
        theme: Option<&Theme>,
        width: f64,
        tcx: &mut TextContext,
    ) -> (RichTextWidget, Size) {
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut 0u64));
        let size = layout_widget(&mut widget, theme, width, tcx);
        (widget, size)
    }

    fn layout_widget(
        widget: &mut RichTextWidget,
        theme: Option<&Theme>,
        width: f64,
        tcx: &mut TextContext,
    ) -> Size {
        let mut lctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    fn paint(widget: &mut RichTextWidget) -> Vec<Op> {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, 400.0));
        widget.paint(&mut pctx, &mut rec);
        rec.0
    }

    fn painted(view: &RichTextView, theme: Option<&Theme>) -> Vec<Op> {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(view, theme, 300.0, &mut tcx);
        paint(&mut widget)
    }

    /// The style of each span group the widget last laid out, regrouped
    /// from the per-span styles it keys reuse on.
    fn group_styles(widget: &RichTextWidget) -> Vec<TextStyle> {
        let styles = &widget.laid_out_span_styles;
        group_spans(
            widget.view.spans.iter().map(|s| s.content.as_str()),
            styles,
            usize::from(u16::MAX),
        )
        .iter()
        .map(|g| styles[g.style].clone())
        .collect()
    }

    fn glyph_colors(ops: &[Op]) -> Vec<Color> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Glyphs(Brush::Solid(c), _) => Some(*c),
                _ => None,
            })
            .collect()
    }

    fn glyph_sizes(ops: &[Op]) -> Vec<f32> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Glyphs(_, s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    fn red() -> Color {
        Color::from_rgb8(200, 20, 20)
    }
    fn green() -> Color {
        Color::from_rgb8(20, 160, 20)
    }
    fn blue() -> Color {
        Color::from_rgb8(20, 20, 200)
    }

    #[test]
    fn three_spans_paint_their_own_colours() {
        let view = rich_text([
            span("one ").color(red()),
            span("two ").color(green()),
            span("three").color(blue()),
        ]);
        let colors = glyph_colors(&painted(&view, None));
        assert_eq!(colors, vec![red(), green(), blue()]);
    }

    #[test]
    fn a_bold_span_shapes_with_its_own_weight() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("plain "), span("bold").weight(FontWeight::BOLD)]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        // The weight is part of the style each span is shaped with; the
        // neighbouring span keeps the default.
        let groups = group_styles(&widget);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].weight, FontWeight::default());
        assert_eq!(groups[1].weight, FontWeight::BOLD);
        assert_eq!(paint(&mut widget).len(), 2, "one run per span");
    }

    #[test]
    fn only_the_underlined_span_gets_an_underline_rect() {
        let plain = painted(&rich_text([span("aaaa "), span("bbbb")]), None);
        assert!(plain.iter().all(|op| !matches!(op, Op::Rect(..))));

        let ops = painted(&rich_text([span("aaaa "), span("bbbb").underline()]), None);
        let rects: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Rect(o, s, _) => Some((*o, *s)),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 1, "{ops:?}");
        let (origin, size) = rects[0];
        // The first span's text occupies the left of the line; the underline
        // starts after it and does not cover the whole paragraph width.
        assert!(origin.x > 0.0, "starts under the second span: {origin:?}");
        assert!(size.width > 0.0 && size.height > 0.0);
        assert!(matches!(ops[0], Op::Rect(..)), "underline paints first");
    }

    #[test]
    fn line_through_paints_after_the_glyph_runs() {
        let ops = painted(
            &rich_text([span("aaaa "), span("bbbb").line_through()]),
            None,
        );
        assert!(matches!(ops.last(), Some(Op::Rect(..))), "{ops:?}");
    }

    fn themed(brightness: frust_theme::Brightness) -> Theme {
        Theme::neutral().with_brightness(brightness)
    }

    #[test]
    fn themed_colour_follows_a_brightness_flip_but_explicit_colour_does_not() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("themed "), span("fixed").color(red())]);
        let light = themed(frust_theme::Brightness::Light);
        let dark = themed(frust_theme::Brightness::Dark);
        assert_ne!(light.scheme().on_surface, dark.scheme().on_surface);

        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, Some(&light), 300.0, &mut tcx);
        let before = glyph_colors(&paint(&mut widget));
        assert_eq!(before, vec![light.scheme().on_surface, red()]);

        layout_widget(&mut widget, Some(&dark), 300.0, &mut tcx);
        let after = glyph_colors(&paint(&mut widget));
        assert_eq!(after, vec![dark.scheme().on_surface, red()]);
    }

    #[test]
    fn semantics_value_is_the_concatenated_text() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut root: frust_core::RenderRoot<(), RichTextView> = frust_core::RenderRoot::new();
        let mut logic = |_: &mut ()| rich_text([span("Hello, "), span("rich "), span("world")]);
        root.rebuild(&mut logic, &mut ());
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 200.0), &mut tcx as &mut dyn std::any::Any);
        let update = root.semantics();
        let window = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.root)
            .expect("window node")
            .1
            .clone();
        let label = update
            .nodes
            .iter()
            .find(|(id, _)| *id == window.children()[0])
            .expect("rich text node")
            .1
            .clone();
        assert_eq!(label.role(), frust_core::accesskit::Role::Label);
        assert_eq!(label.value(), Some("Hello, rich world"));
    }

    #[test]
    fn max_lines_with_ellipsis_truncates() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let words = "word ".repeat(40);
        let open = rich_text([span(words.clone()), span("tail").weight(FontWeight::BOLD)]);
        let capped = open.clone().max_lines(1).overflow(TextOverflow::Ellipsis);
        let mut tcx = TextContext::new();
        let (_, full) = lay_out(&open, None, 120.0, &mut tcx);
        let (_, cut) = lay_out(&capped, None, 120.0, &mut tcx);
        assert!(full.height > cut.height * 1.5, "{full:?} vs {cut:?}");
    }

    #[test]
    fn text_scale_enlarges_only_default_sized_spans() {
        // A pinned size differing from the default, so the two spans stay
        // separate groups.
        let view = rich_text([span("auto "), span("pinned").size(17.0)]);
        let base = Theme::neutral();
        let authored = base.type_scale.clone();
        let scaled = base.clone().with_text_scale(&authored, 1.5);

        let plain = glyph_sizes(&painted(&view, Some(&base)));
        let big = glyph_sizes(&painted(&view, Some(&scaled)));
        assert_eq!(plain.len(), 2);
        assert_eq!(big.len(), 2);
        assert!((big[0] / plain[0] - 1.5).abs() < 0.1, "{plain:?} {big:?}");
        assert!((big[1] - plain[1]).abs() < 0.01, "explicit size is pinned");
    }

    #[test]
    fn a_text_scale_change_that_regroups_spans_relays_out() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        // At scale 1.0 the default size equals the pinned 16, so the groups
        // are [ab:16, c:24]; at scale 1.5 it is 24 and they are [a:16, bc:24].
        // Both group-style vectors are [S16, S24].
        let view = rich_text([span("a").size(16.0), span("b"), span("c").size(24.0)]);
        let base = Theme::neutral();
        let authored = base.type_scale.clone();
        let scaled = base.clone().with_text_scale(&authored, 1.5);

        let mut tcx = TextContext::new();
        let (mut reused, _) = lay_out(&view, Some(&base), 300.0, &mut tcx);
        let reused_size = layout_widget(&mut reused, Some(&scaled), 300.0, &mut tcx);
        let (mut fresh, fresh_size) = lay_out(&view, Some(&scaled), 300.0, &mut tcx);
        assert_eq!(
            glyph_sizes(&paint(&mut reused)),
            glyph_sizes(&paint(&mut fresh))
        );
        assert_eq!(reused_size, fresh_size, "\"b\" lays out at 24px");
    }

    #[test]
    fn an_unchanged_span_list_reuses_the_layout() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("alpha ").color(red()), span("beta").underline()]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        let shapes = tcx.shape_cache_stats().shapes;
        assert_eq!(shapes, 1);

        // An identical rebuild changes nothing and an extra pass skips the
        // text context entirely.
        let again = view.clone();
        let flags = View::<()>::rebuild(&again, &view, &mut widget, &mut BuildCtx::new(&mut 0u64));
        assert_eq!(flags, ChangeFlags::NONE);
        layout_widget(&mut widget, None, 300.0, &mut tcx);
        let stats = tcx.shape_cache_stats();
        assert_eq!(stats.shapes, 1, "no re-shape");
        assert_eq!(stats.hits, 0, "the widget skipped the text context");
    }

    #[test]
    fn a_changed_span_list_requests_layout_and_reshapes() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("alpha "), span("beta")]);
        let next = rich_text([span("alpha "), span("gamma")]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        let flags = View::<()>::rebuild(&next, &view, &mut widget, &mut BuildCtx::new(&mut 0u64));
        assert!(flags.contains(ChangeFlags::LAYOUT));
        layout_widget(&mut widget, None, 300.0, &mut tcx);
        assert_eq!(tcx.shape_cache_stats().shapes, 2);
    }

    #[test]
    fn empty_spans_drop_and_equal_neighbours_merge() {
        let view = rich_text([
            span("one "),
            span(""),
            span("two ").color(red()),
            span("three").color(red()),
            span("").color(green()),
            span("four"),
        ]);
        {
            let _slot = crate::test_support::LocaleSlotGuard::hold();
            let mut tcx = TextContext::new();
            let (widget, _) = lay_out(&view, None, 300.0, &mut tcx);
            assert_eq!(group_styles(&widget).len(), 3);
        }
        let colors = glyph_colors(&painted(&view, None));
        assert_eq!(colors.len(), 3);
        assert_eq!(colors[1], red());
    }

    #[test]
    fn grouping_folds_the_excess_into_the_last_kept_group() {
        let styles: Vec<TextStyle> = (0..6)
            .map(|i| TextStyle::new(10.0 + (i % 2) as f32, Color::BLACK))
            .collect();
        let texts = ["a"; 6];
        let groups = group_spans(texts.iter().copied(), &styles, 4);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[3].style, 3);
        assert_eq!(
            groups[3].spans,
            3..6,
            "the excess joins the last kept group"
        );
        let none = group_spans(["", ""].into_iter(), &styles, 4);
        assert_eq!(none.len(), 1, "all-empty text keeps its first span");
    }

    #[test]
    fn more_spans_than_the_shaper_indexes_degrade_instead_of_panicking() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        // Alternating explicit sizes, so no neighbours merge.
        let count = usize::from(u16::MAX) + 4_000;
        let view =
            rich_text((0..count).map(|i| span("a").size(if i % 2 == 0 { 12.0 } else { 13.0 })));
        let mut tcx = TextContext::new();
        let (widget, size) = lay_out(&view, None, 300.0, &mut tcx);
        assert_eq!(group_styles(&widget).len(), usize::from(u16::MAX));
        assert!(size.width > 0.0);
    }
}
