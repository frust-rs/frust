//! [`avatar`]/[`AvatarView`]: an initials box — a small rounded square with a
//! faint accent wash, an accent hairline border, and centered accent-colored
//! initials (the Glyph design system's `.avatar` rule). Purely a leaf
//! display widget (no interaction), so — like [`super::badge`] — it shapes and
//! paints its own glyph run rather than nesting a [`frust::TextView`],
//! and its `View` impl is generic over any `State`.
//!
//! # Color resolution
//!
//! By default the box is the Glyph **accent** family: background =
//! `primary` at [`AVATAR_WASH_ALPHA`] (a true alpha wash, matching the
//! source's faint-amber fill — see [`super::badge`]'s Accent variant for the
//! same "no opaque faint-container role exists" reasoning), border + initials =
//! `primary`. With no theme threaded, every value falls back to the literal
//! Glyph **dark** accent constants — never a panic.
//!
//! The [`AvatarView::accent`] builder recolors the whole box to a caller
//! hue (background becomes that hue's faint wash, border + text become the hue
//! itself); [`AvatarView::background`]/[`AvatarView::foreground`]/
//! [`AvatarView::border_color`] override any single channel outright (they win
//! over `accent`).

use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust::{ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

/// Corner-rounding tolerance for the hairline border (matches
/// [`super::badge`]'s constant of the same name).
const PATH_TOLERANCE: f64 = 0.1;

/// Default box edge length, in logical px (the Glyph design system's `.avatar`
/// square).
const AVATAR_DEFAULT_SIZE: f64 = 40.0;
/// The initials font size as a fraction of the box edge.
const AVATAR_FONT_FRACTION: f32 = 0.4;
/// Border width, in logical px (`.avatar{border:1px solid}`).
const AVATAR_BORDER_WIDTH: f64 = 1.0;
/// The accent-wash alpha over the accent color (matches [`super::badge`]'s
/// `--amber-faint` alpha exactly).
const AVATAR_WASH_ALPHA: f32 = 0.12;
/// Unthemed corner-radius fallback — Glyph's canonical `--radius-sm` (6px).
const AVATAR_RADIUS_FALLBACK: f64 = 6.0;

// ---- Unthemed fallback constants (Glyph **dark** accent; see module docs) --

/// Unthemed accent (Glyph dark `--amber` `#ffb627`).
const AVATAR_ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27);

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::badge`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A declarative initials avatar. See the [module docs](self).
pub struct AvatarView {
    initials: String,
    size: f64,
    accent: Option<Color>,
    background: Option<Color>,
    foreground: Option<Color>,
    border_color: Option<Color>,
}

/// Create an avatar showing `initials` (rendered uppercase, per the source's
/// `text-transform:uppercase`) at the default [`AVATAR_DEFAULT_SIZE`] edge.
pub fn avatar(initials: impl Into<String>) -> AvatarView {
    AvatarView {
        initials: initials.into(),
        size: AVATAR_DEFAULT_SIZE,
        accent: None,
        background: None,
        foreground: None,
        border_color: None,
    }
}

impl AvatarView {
    /// Override the box edge length (logical px).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0);
        self
    }

    /// Recolor the whole box to `accent`: background becomes its faint wash,
    /// border + initials become the hue itself. Individual channel overrides
    /// ([`AvatarView::background`]/[`AvatarView::foreground`]/
    /// [`AvatarView::border_color`]) still win over this.
    pub fn accent(mut self, accent: Color) -> Self {
        self.accent = Some(accent);
        self
    }

    /// Override the background fill outright.
    pub fn background(mut self, background: Color) -> Self {
        self.background = Some(background);
        self
    }

    /// Override the initials color outright.
    pub fn foreground(mut self, foreground: Color) -> Self {
        self.foreground = Some(foreground);
        self
    }

    /// Override the border color outright.
    pub fn border_color(mut self, border_color: Color) -> Self {
        self.border_color = Some(border_color);
        self
    }
}

/// A minimal retained text run — see [`super::badge`]'s `GlyphLabel` for the
/// full shape/rationale (duplicated per this crate's small-helper convention).
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
    laid_out_max_width: Option<f32>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
            laid_out_max_width: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        self.laid_out_max_width = max_width;
        size
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// The retained widget for an [`AvatarView`].
pub struct AvatarWidget {
    initials: GlyphLabel,
    initials_text: String,
    initials_size: Size,
    size: f64,
    accent: Option<Color>,
    background: Option<Color>,
    foreground: Option<Color>,
    border_color: Option<Color>,
}

impl<State: 'static> View<State> for AvatarView {
    type Element = AvatarWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AvatarWidget {
        AvatarWidget {
            initials: GlyphLabel::new(self.initials.to_uppercase()),
            initials_text: self.initials.clone(),
            initials_size: Size::ZERO,
            size: self.size,
            accent: self.accent,
            background: self.background,
            foreground: self.foreground,
            border_color: self.border_color,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AvatarWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.initials != self.initials {
            element.initials.set_content(self.initials.to_uppercase());
            element.initials_text = self.initials.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.accent != self.accent
            || prev.background != self.background
            || prev.foreground != self.foreground
            || prev.border_color != self.border_color
        {
            element.accent = self.accent;
            element.background = self.background;
            element.foreground = self.foreground;
            element.border_color = self.border_color;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

fn initials_style(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new((size as f32 * AVATAR_FONT_FRACTION).max(1.0), color)
    }
}

impl AvatarWidget {
    /// Resolve `(background, foreground, border)`. Precedence: an explicit
    /// per-channel override wins; else the `accent` builder's hue (or the
    /// themed `primary`, or [`AVATAR_ACCENT`] unthemed) drives all three.
    fn resolve_colors(&self, theme: Option<&Theme>) -> (Color, Color, Color) {
        let accent = self
            .accent
            .or_else(|| theme.map(|t| t.scheme().primary))
            .unwrap_or(AVATAR_ACCENT);
        let background = self
            .background
            .unwrap_or_else(|| with_alpha(accent, AVATAR_WASH_ALPHA));
        let foreground = self.foreground.unwrap_or(accent);
        let border = self.border_color.unwrap_or(accent);
        (background, foreground, border)
    }

    fn resolve_radius(&self, theme: Option<&Theme>) -> f64 {
        match theme {
            Some(theme) => ShapeScale::resolve(theme.shape.small, self.size, self.size),
            None => AVATAR_RADIUS_FALLBACK,
        }
    }
}

impl Widget for AvatarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, fg, _) = self.resolve_colors(theme);
        let style = initials_style(self.size, fg);
        self.initials_size = self.initials.layout(ctx, &style, None);
        bc.constrain(Size::new(self.size, self.size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, _fg, border) = self.resolve_colors(theme);
        let radius = self.resolve_radius(theme);
        let size = ctx.size();
        let o = ctx.origin();

        scene.fill_rounded_rect(o, size, radius, bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(o, &path, AVATAR_BORDER_WIDTH, &Brush::Solid(border));

        // `fg` was baked into the run at layout time; center it in the box.
        let text_origin = o + Vec2::new(
            (size.width - self.initials_size.width) / 2.0,
            (size.height - self.initials_size.height) / 2.0,
        );
        self.initials.paint(text_origin, scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // An image-like decorative label carrying the (natural-cased) initials.
        ctx.push_node(Role::Image, |node| {
            node.set_label(self.initials_text.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _path: &kurbo::BezPath, _width: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn build(view: &AvatarView) -> AvatarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint(widget: &mut AvatarWidget, theme: Option<&Theme>) -> Recorder {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_uses_accent_wash_border_and_text() {
        let mut w = build(&avatar("ed"));
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(AVATAR_ACCENT, AVATAR_WASH_ALPHA)
        );
        assert_eq!(rec.strokes[0], AVATAR_ACCENT);
        assert_eq!(rec.glyph_colors[0], AVATAR_ACCENT);
    }

    #[test]
    fn initials_render_uppercase_but_semantics_keep_casing() {
        let w = build(&avatar("ed"));
        assert_eq!(w.initials.content, "ED");
        assert_eq!(w.initials_text, "ed");
    }

    #[test]
    fn glyph_dark_and_light_resolve_primary_accent() {
        let dark = crate::baseline();
        let light = dark.clone().with_brightness(Brightness::Light);

        let mut wd = build(&avatar("ab"));
        let rec_d = layout_and_paint(&mut wd, Some(&dark));
        assert_eq!(rec_d.strokes[0], dark.scheme().primary);
        assert_eq!(
            rec_d.rrects[0].3,
            with_alpha(dark.scheme().primary, AVATAR_WASH_ALPHA)
        );

        let mut wl = build(&avatar("ab"));
        let rec_l = layout_and_paint(&mut wl, Some(&light));
        assert_eq!(rec_l.strokes[0], light.scheme().primary);
        assert_ne!(dark.scheme().primary, light.scheme().primary);
    }

    #[test]
    fn accent_override_recolors_all_three_channels() {
        let custom = Color::from_rgb8(0x10, 0x20, 0x30);
        let mut w = build(&avatar("zz").accent(custom));
        let rec = layout_and_paint(&mut w, Some(&crate::baseline()));
        assert_eq!(rec.rrects[0].3, with_alpha(custom, AVATAR_WASH_ALPHA));
        assert_eq!(rec.strokes[0], custom);
        assert_eq!(rec.glyph_colors[0], custom);
    }

    #[test]
    fn per_channel_overrides_win_over_accent() {
        let bg = Color::from_rgb8(1, 2, 3);
        let fg = Color::from_rgb8(4, 5, 6);
        let border = Color::from_rgb8(7, 8, 9);
        let mut w = build(
            &avatar("qq")
                .accent(Color::from_rgb8(0xff, 0x00, 0x00))
                .background(bg)
                .foreground(fg)
                .border_color(border),
        );
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].3, bg);
        assert_eq!(rec.strokes[0], border);
        assert_eq!(rec.glyph_colors[0], fg);
    }

    #[test]
    fn size_builder_sets_box_edge() {
        let mut w = build(&avatar("x").size(64.0));
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].1, Size::new(64.0, 64.0));
    }

    #[test]
    fn semantics_reports_image_role_and_initials() {
        fn logic(_s: &mut ()) -> AvatarView {
            avatar("ed")
        }
        let mut root: frust_core::RenderRoot<(), AvatarView> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(100.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Image)
            .expect("avatar contributes a Role::Image node");
        assert_eq!(node.label(), Some("ed"));
    }
}
