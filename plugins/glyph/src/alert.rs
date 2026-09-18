//! [`alert`]/[`AlertView`]: an icon + title + body inline banner with four
//! semantic variants (the Glyph design system's `.alert*` rules, retrieved
//! 2026-07-21) — "Layout refresh is every 3 seconds" (info),
//! "Session synced" (success), "Certificate expires in 6 days" (warning),
//! "Connection lost" (error) in the design system's demo section.
//!
//! # Color resolution
//!
//! The source cascades one `color` (the variant's accent hue) onto both
//! `.alert-icon` and `.alert-title` (neither sets its own `color`), while
//! `.alert-body` explicitly overrides to `--fg-muted`. This widget mirrors
//! that exactly:
//!
//! - **icon + title** resolve the variant's accent —
//!   `theme.extension::<StatusPalette>()` for Info/Success/Warning (present on
//!   every built-in baseline), `ColorScheme::error` directly for Error (M3's
//!   baseline already carries an error role, no extension needed) — falling
//!   back to a literal Glyph-dark constant with no theme threaded.
//! - **body** resolves `on_surface_variant` (`fg-muted`), themed or the
//!   literal fallback.
//! - **background** is the matching `*_container`/`error_container` faint
//!   wash (the opaque, pre-flattened container roles — see
//!   [`crate::tokens::color`]'s "Alpha pre-flattening").
//! - **border** is the *same* accent color as the icon/title at a true alpha
//!   (`ALERT_BORDER_ALPHA` = `0.3`, matching the source's `border-color:
//!   rgba(<hue>, 0.3)` exactly for both Glyph brightnesses — the light source
//!   spells out the light-scheme hue at the same alpha).
//!
//! # Why this widget doesn't nest `Text` children
//!
//! See `crate::badge`'s module docs — the same reasoning applies to
//! all three runs here (icon/title/body each need a per-variant or
//! `on_surface_variant` color the four fixed `ThemeTextColor` roles don't
//! cover), so `AlertWidget` shapes and paints its own runs directly via
//! `frust_text`.
//!
//! # Typeface
//!
//! Each run reads its family at layout from a live type-scale role, the one
//! whose Glyph family is the face that run has always painted: the icon from
//! `headlineSmall` (Space Mono), the title from `titleSmall` and the body from
//! `bodyMedium` (both IBM Plex Mono). Size, weight and line height stay this
//! module's own; unthemed, each falls back to the matching Glyph stack. The
//! family is part of each run's cached style, so a theme swap reshapes it
//! (see `crate::badge`'s Typeface section).

use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust::{ShapeScale, StatusPalette, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

/// Corner-rounding tolerance for the border stroke (matches
/// `crate::badge`'s constant of the same name).
const PATH_TOLERANCE: f64 = 0.1;

/// Outer padding, in logical px (`.alert{padding:14px 16px}`).
const ALERT_PAD_X: f64 = 16.0;
const ALERT_PAD_Y: f64 = 14.0;
/// Gap between the icon column and the title/body column, in logical px
/// (`.alert{gap:12px}`).
const ALERT_ICON_GAP: f64 = 12.0;
/// Gap between the title and body rows, in logical px
/// (`.alert-title{margin-bottom:2px}`).
const ALERT_TITLE_GAP: f64 = 2.0;
/// Shared font size for icon/title/body, in logical px
/// (`.alert{font-size:12.5px}` — none of the three roles override it).
const ALERT_FONT_SIZE: f32 = 12.5;
/// Prose line height (`.alert{line-height:1.6}`).
const ALERT_LINE_HEIGHT: f32 = 1.6;
/// Border width, in logical px (`.alert{border:1px solid}`).
const ALERT_BORDER_WIDTH: f64 = 1.0;
/// The border's alpha over its accent color (`rgba(<hue>,0.3)`, both
/// brightnesses — see the module docs).
const ALERT_BORDER_ALPHA: f32 = 0.3;

// ---- Unthemed fallback constants (Glyph **dark** values; see module docs) --

const ALERT_INFO_ACCENT: Color = Color::from_rgb8(0x5e, 0xc8, 0xd8); // cyan
const ALERT_INFO_BG: Color = Color::from_rgb8(0x20, 0x32, 0x3c);
const ALERT_SUCCESS_ACCENT: Color = Color::from_rgb8(0x5f, 0xd8, 0x8f);
const ALERT_SUCCESS_BG: Color = Color::from_rgb8(0x1f, 0x31, 0x30);
const ALERT_WARNING_ACCENT: Color = Color::from_rgb8(0xf5, 0xc8, 0x60);
const ALERT_WARNING_BG: Color = Color::from_rgb8(0x31, 0x2f, 0x2a);
const ALERT_ERROR_ACCENT: Color = Color::from_rgb8(0xff, 0x6b, 0x6b);
const ALERT_ERROR_BG: Color = Color::from_rgb8(0x32, 0x24, 0x2c);
const ALERT_BODY_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted
const ALERT_RADIUS_FALLBACK: f64 = 10.0; // --radius-md

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// `crate::badge`'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Which semantic variant an [`AlertView`] renders as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertVariant {
    /// A cyan-accented informational banner.
    Info,
    /// A success-accented banner.
    Success,
    /// A warning-accented banner.
    Warning,
    /// An error-accented banner.
    Error,
}

/// The Glyph design system's default icon glyph per variant ("i"/"✓"/"!"/"×").
fn default_icon(variant: AlertVariant) -> &'static str {
    match variant {
        AlertVariant::Info => "i",
        AlertVariant::Success => "\u{2713}", // ✓
        AlertVariant::Warning => "!",
        AlertVariant::Error => "\u{d7}", // ×
    }
}

/// A declarative alert banner. See the [module docs](self).
pub struct AlertView {
    variant: AlertVariant,
    icon: String,
    title: String,
    body: String,
}

/// Create an alert of `variant` with `title`/`body` text, defaulting to the
/// Glyph design system's icon glyph for that variant (override with
/// [`AlertView::icon`]).
pub fn alert(
    variant: AlertVariant,
    title: impl Into<String>,
    body: impl Into<String>,
) -> AlertView {
    AlertView {
        variant,
        icon: default_icon(variant).to_string(),
        title: title.into(),
        body: body.into(),
    }
}

impl AlertView {
    /// Override the default per-variant icon glyph.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }
}

/// A minimal retained text run — see `crate::badge`'s `GlyphLabel` for
/// the full shape/rationale (duplicated per this crate's existing
/// small-helper convention).
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

/// The retained widget for an [`AlertView`].
pub struct AlertWidget {
    variant: AlertVariant,
    icon: GlyphLabel,
    icon_size: Size,
    title: GlyphLabel,
    title_text: String,
    title_size: Size,
    body: GlyphLabel,
    body_text: String,
    body_size: Size,
}

/// `alert` is a leaf display widget with no callback, so — like
/// `frust::IconView` — this impl is generic over any `State` rather
/// than tied to one concrete app-state type.
impl<State: 'static> View<State> for AlertView {
    type Element = AlertWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AlertWidget {
        AlertWidget {
            variant: self.variant,
            icon: GlyphLabel::new(self.icon.clone()),
            icon_size: Size::ZERO,
            title: GlyphLabel::new(self.title.clone()),
            title_text: self.title.clone(),
            title_size: Size::ZERO,
            body: GlyphLabel::new(self.body.clone()),
            body_text: self.body.clone(),
            body_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AlertWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.icon != self.icon {
            element.icon.set_content(self.icon.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.title != self.title {
            element.title.set_content(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.body != self.body {
            element.body.set_content(self.body.clone());
            element.body_text = self.body.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// Glyph's display face stack (Space Mono): the icon's unthemed family.
fn display_face() -> FontFamily {
    FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace)
}

/// Glyph's UI face stack (IBM Plex Mono): the title's and body's unthemed
/// family.
fn ui_face() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

/// The icon's style; family from the theme's `headlineSmall` role.
fn icon_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(display_face, |t| t.type_scale.headline_small.family.clone()),
        weight: FontWeight::BOLD,
        line_height: LineHeight::FontSizeRelative(ALERT_LINE_HEIGHT),
        ..TextStyle::new(ALERT_FONT_SIZE, color)
    }
}

/// The title's style; family from the theme's `titleSmall` role.
fn title_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.title_small.family.clone()),
        weight: FontWeight::SEMI_BOLD,
        line_height: LineHeight::FontSizeRelative(ALERT_LINE_HEIGHT),
        ..TextStyle::new(ALERT_FONT_SIZE, color)
    }
}

/// The body's style; family from the theme's `bodyMedium` role.
fn body_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.body_medium.family.clone()),
        weight: FontWeight::REGULAR,
        line_height: LineHeight::FontSizeRelative(ALERT_LINE_HEIGHT),
        ..TextStyle::new(ALERT_FONT_SIZE, color)
    }
}

/// Resolve `(accent, container-bg)` for `variant` — see the module docs.
fn resolve_alert_colors(theme: Option<&Theme>, variant: AlertVariant) -> (Color, Color) {
    let Some(theme) = theme else {
        return unthemed_alert_colors(variant);
    };
    let scheme = theme.scheme();
    let status = theme.extension::<StatusPalette>();
    match variant {
        AlertVariant::Info => match status {
            Some(status) => {
                let c = status.colors(theme.brightness);
                (c.info, c.info_container)
            }
            None => (scheme.tertiary, scheme.tertiary_container),
        },
        AlertVariant::Success => match status {
            Some(status) => {
                let c = status.colors(theme.brightness);
                (c.success, c.success_container)
            }
            None => (ALERT_SUCCESS_ACCENT, ALERT_SUCCESS_BG),
        },
        AlertVariant::Warning => match status {
            Some(status) => {
                let c = status.colors(theme.brightness);
                (c.warning, c.warning_container)
            }
            None => (ALERT_WARNING_ACCENT, ALERT_WARNING_BG),
        },
        AlertVariant::Error => (scheme.error, scheme.error_container),
    }
}

fn unthemed_alert_colors(variant: AlertVariant) -> (Color, Color) {
    match variant {
        AlertVariant::Info => (ALERT_INFO_ACCENT, ALERT_INFO_BG),
        AlertVariant::Success => (ALERT_SUCCESS_ACCENT, ALERT_SUCCESS_BG),
        AlertVariant::Warning => (ALERT_WARNING_ACCENT, ALERT_WARNING_BG),
        AlertVariant::Error => (ALERT_ERROR_ACCENT, ALERT_ERROR_BG),
    }
}

fn resolve_body_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface_variant,
        None => ALERT_BODY_FG,
    }
}

fn resolve_alert_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.medium, size.width, size.height),
        None => ALERT_RADIUS_FALLBACK,
    }
}

impl Widget for AlertWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve every style up front so the immutable theme borrow ends
        // before the `&mut ctx` shaping calls below.
        let theme = Theme::from_layout_ctx(ctx);
        let (accent, _) = resolve_alert_colors(theme, self.variant);
        let muted = resolve_body_color(theme);
        let (icon_style, title_style, body_style) = (
            icon_style(theme, accent),
            title_style(theme, accent),
            body_style(theme, muted),
        );

        self.icon_size = self.icon.layout(ctx, &icon_style, None);

        let max_width = bc.max().width;
        let text_col_max_width = if max_width.is_finite() {
            Some(
                (max_width - ALERT_PAD_X * 2.0 - self.icon_size.width - ALERT_ICON_GAP).max(0.0)
                    as f32,
            )
        } else {
            None
        };

        self.title_size = self.title.layout(ctx, &title_style, text_col_max_width);
        self.body_size = self.body.layout(ctx, &body_style, text_col_max_width);

        let text_col_height = self.title_size.height + ALERT_TITLE_GAP + self.body_size.height;
        let content_height = text_col_height.max(self.icon_size.height);
        let height = content_height + ALERT_PAD_Y * 2.0;

        let width = if max_width.is_finite() {
            max_width
        } else {
            ALERT_PAD_X * 2.0
                + self.icon_size.width
                + ALERT_ICON_GAP
                + self.title_size.width.max(self.body_size.width)
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (accent, bg) = resolve_alert_colors(theme, self.variant);
        let radius = resolve_alert_radius(theme, ctx.size());

        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, ctx.size()), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        let border = with_alpha(accent, ALERT_BORDER_ALPHA);
        scene.stroke_path(
            ctx.origin(),
            &path,
            ALERT_BORDER_WIDTH,
            &Brush::Solid(border),
        );

        let icon_y = ALERT_PAD_Y + (self.text_col_height() - self.icon_size.height).max(0.0) / 2.0;
        self.icon
            .paint(ctx.origin() + Vec2::new(ALERT_PAD_X, icon_y), scene);

        let text_x = ALERT_PAD_X + self.icon_size.width + ALERT_ICON_GAP;
        self.title
            .paint(ctx.origin() + Vec2::new(text_x, ALERT_PAD_Y), scene);
        let body_y = ALERT_PAD_Y + self.title_size.height + ALERT_TITLE_GAP;
        self.body
            .paint(ctx.origin() + Vec2::new(text_x, body_y), scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A non-modal inline banner: `Role::Alert` (distinct from
        // `Role::AlertDialog`, which the catalog's modal dialogs use).
        ctx.push_node(Role::Alert, |node| {
            node.set_label(self.title_text.as_str());
            node.set_description(self.body_text.as_str());
        });
    }
}

impl AlertWidget {
    fn text_col_height(&self) -> f64 {
        self.title_size.height + ALERT_TITLE_GAP + self.body_size.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn build(variant: AlertVariant) -> AlertWidget {
        let view: AlertView = alert(variant, "Title", "Body text");
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint(widget: &mut AlertWidget, theme: Option<&Theme>) -> Recorder {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_info_uses_fallback_constants() {
        let mut w = build(AlertVariant::Info);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].3, ALERT_INFO_BG);
        assert_eq!(rec.glyph_colors[0], ALERT_INFO_ACCENT); // icon
        assert_eq!(rec.glyph_colors[1], ALERT_INFO_ACCENT); // title
        assert_eq!(rec.glyph_colors[2], ALERT_BODY_FG); // body
        assert_eq!(
            rec.strokes[0],
            with_alpha(ALERT_INFO_ACCENT, ALERT_BORDER_ALPHA)
        );
    }

    #[test]
    fn glyph_dark_theme_resolves_success_from_status_palette() {
        let theme = crate::baseline();
        let mut w = build(AlertVariant::Success);
        let rec = layout_and_paint(&mut w, Some(&theme));
        let status = theme.extension::<StatusPalette>().unwrap();
        let colors = status.colors(theme.brightness);
        assert_eq!(rec.rrects[0].3, colors.success_container);
        assert_eq!(rec.glyph_colors[1], colors.success); // title
        assert_eq!(
            rec.strokes[0],
            with_alpha(colors.success, ALERT_BORDER_ALPHA)
        );
    }

    #[test]
    fn glyph_light_theme_resolves_warning_border_alpha() {
        let theme = crate::baseline().with_brightness(frust::Brightness::Light);
        let mut w = build(AlertVariant::Warning);
        let rec = layout_and_paint(&mut w, Some(&theme));
        let status = theme.extension::<StatusPalette>().unwrap();
        let colors = status.colors(theme.brightness);
        assert_eq!(
            rec.strokes[0],
            with_alpha(colors.warning, ALERT_BORDER_ALPHA)
        );
    }

    #[test]
    fn glyph_theme_resolves_error_from_color_scheme_directly() {
        let theme = crate::baseline();
        let mut w = build(AlertVariant::Error);
        let rec = layout_and_paint(&mut w, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().error_container);
        assert_eq!(rec.glyph_colors[1], theme.scheme().error);
    }

    #[test]
    fn default_icon_glyphs_match_source_per_variant() {
        assert_eq!(default_icon(AlertVariant::Info), "i");
        assert_eq!(default_icon(AlertVariant::Success), "\u{2713}");
        assert_eq!(default_icon(AlertVariant::Warning), "!");
        assert_eq!(default_icon(AlertVariant::Error), "\u{d7}");
    }

    #[test]
    fn semantics_reports_alert_role_title_and_body() {
        fn logic(_s: &mut ()) -> AlertView {
            alert(AlertVariant::Info, "Title", "Body text")
        }
        let mut root: frust_core::RenderRoot<(), AlertView> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Alert)
            .expect("alert contributes a Role::Alert node");
        assert_eq!(node.label(), Some("Title"));
        assert_eq!(node.description(), Some("Body text"));
    }

    // ---- Typeface: each run's family follows its type-scale role ----------

    #[cfg(feature = "bundled-fonts")]
    fn info_alert(_: &mut ()) -> AlertView {
        alert(AlertVariant::Info, "Title", "Body text")
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_paint_in_their_role_faces_under_the_glyph_theme() {
        use crate::badge::typeface_probe::{Face, painted_faces};
        let faces = painted_faces(info_alert, crate::baseline(), Size::new(400.0, 200.0));
        // Icon, title, body.
        assert_eq!(faces, [Face::SpaceMono, Face::PlexMono, Face::PlexMono]);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_follow_a_live_theme_family_swap() {
        use crate::badge::typeface_probe::{Face, faces_across_a_live_swap};
        let (before, after) = faces_across_a_live_swap(info_alert, Size::new(400.0, 200.0));
        assert_eq!(before, [Face::SpaceMono, Face::PlexMono, Face::PlexMono]);
        assert_eq!(after, [Face::PlexMono, Face::SpaceMono, Face::SpaceMono]);
    }
}
