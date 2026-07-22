//! Filled by task 20-glyph-status-feedback (Badge).
//!
//! [`badge`]/[`BadgeView`]: a small, lowercase, pill-shaped status indicator —
//! "connected"/"degraded"/"offline"/"read-only"/"v0.44.1" in the Glyph
//! reference build (`research/glyph-design-system.html` `.badge*` rules,
//! retrieved 2026-07-21) — with five semantic variants and an optional
//! leading dot.
//!
//! # Color resolution (documented three-tier precedence)
//!
//! Not every variant has all three tiers, since `ColorScheme` (spec's M3
//! baseline) carries no `success`/`warning` roles at all (see
//! `frust_theme::extensions`' module docs):
//!
//! - **Success/Warning** resolve `theme.extension::<StatusPalette>()` first
//!   (present on every built-in baseline —
//!   `Theme::m3_baseline`/`cupertino_baseline`/`glyph_baseline` each attach
//!   one), falling back to this module's literal Glyph-dark constants only in
//!   the defensive case where an app cleared the extension.
//! - **Error** resolves directly from `ColorScheme::error`/`error_container` —
//!   M3's baseline *does* carry an error role, so no extension is needed.
//! - **Neutral** resolves `surface_container_high`/`on_surface_variant` (the
//!   Glyph token module's own `bg-raised`/`fg-muted` mapping, see
//!   `frust_theme::glyph::color`) plus an `outline` border.
//! - **Accent** has no `ColorScheme` container role for a *faint* wash —
//!   `primary_container` is the opaque bright-fill role (used for filled
//!   buttons), not a translucent one — so this variant paints a true alpha
//!   wash over `primary` instead (`ACCENT_WASH_ALPHA` matches the source's
//!   `--amber-faint` alpha, `0.12`, exactly), unlike the pre-flattened
//!   Success/Warning/Error containers which assume a `surface`-level
//!   backdrop (see `frust_theme::glyph::color`'s "Alpha pre-flattening").
//!
//! With no theme threaded at all, every variant falls back to a literal Glyph
//! **dark** constant (this module's `BADGE_*` consts) — never a panic.
//!
//! # Text color and why this widget doesn't nest a `Text` child
//!
//! [`crate::text::TextView`] resolves its themed color from a fixed
//! four-variant `ThemeTextColor` role (`OnSurface`/`OnPrimary`/
//! `OnSurfaceVariant`/`OnPrimaryContainer`) chosen at `View::build`/`rebuild`
//! time — but `BuildCtx` (unlike `LayoutCtx`/`PaintCtx`) carries no theme, so a
//! nested `Text` view could never resolve an arbitrary per-variant color
//! (success green, warning amber, …) that isn't one of those four roles.
//! Rather than widen `TextView`'s internal role enum (out of this task's
//! scope — the plan's file list is this module alone), `BadgeWidget` shapes
//! and paints its own label glyph run directly via `frust_text`, mirroring
//! `crate::text::TextWidget`'s own shape (lazy shape-on-layout, glyph-run
//! paint) but resolving color from the variant/theme itself.
//!
//! Per the crate's layout-time-baked-color contract
//! (`docs/CODE_STANDARDS.md`'s Theming conventions), the resolved color feeds
//! into the cached [`TextStyle`] compared at each layout pass, so a theme
//! swap (which forces `ChangeFlags::LAYOUT` — see
//! `docs/ARCHITECTURE.md`'s Theme delivery) always re-shapes with the new
//! color.

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust_text::{FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle};
use frust_theme::{ShapeScale, StatusPalette, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

/// Corner-rounding tolerance for the neutral-variant hairline border (matches
/// the crate's other `RoundedRect::to_path` call sites, e.g.
/// `crate::cupertino::tabbar`'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// Horizontal padding, in logical px (`.badge{padding:4px 9px}`).
const BADGE_PAD_X: f64 = 9.0;
/// Vertical padding, in logical px.
const BADGE_PAD_Y: f64 = 4.0;
/// Gap between the leading dot and the label, in logical px
/// (`.badge{gap:6px}`).
const BADGE_GAP: f64 = 6.0;
/// Leading-dot diameter, in logical px (`.badge-dot{width:5px;height:5px}`).
const BADGE_DOT_SIZE: f64 = 5.0;
/// Label font size, in logical px (`.badge{font-size:10.5px}`).
const BADGE_FONT_SIZE: f32 = 10.5;
/// Label letter-spacing, in logical px (`.badge{letter-spacing:0.03em}` ==
/// `0.03 * 10.5`).
const BADGE_LETTER_SPACING: f32 = BADGE_FONT_SIZE * 0.03;
/// Neutral-variant border width, in logical px (`.badge{border:1px solid
/// transparent}` — only the neutral variant supplies a non-transparent
/// `border-color`).
const BADGE_BORDER_WIDTH: f64 = 1.0;
/// The Accent variant's wash alpha (`--amber-faint` == `rgba(255,182,39,0.12)`,
/// both Glyph brightnesses — see the module docs).
const BADGE_ACCENT_WASH_ALPHA: f32 = 0.12;

// ---- Unthemed fallback constants (Glyph **dark** values; see module docs) --

const BADGE_SUCCESS_FG: Color = Color::from_rgb8(0x5f, 0xd8, 0x8f);
const BADGE_SUCCESS_BG: Color = Color::from_rgb8(0x1f, 0x31, 0x30);
const BADGE_WARNING_FG: Color = Color::from_rgb8(0xf5, 0xc8, 0x60);
const BADGE_WARNING_BG: Color = Color::from_rgb8(0x31, 0x2f, 0x2a);
const BADGE_ERROR_FG: Color = Color::from_rgb8(0xff, 0x6b, 0x6b);
const BADGE_ERROR_BG: Color = Color::from_rgb8(0x32, 0x24, 0x2c);
const BADGE_NEUTRAL_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88);
const BADGE_NEUTRAL_BG: Color = Color::from_rgb8(0x1e, 0x23, 0x30);
const BADGE_NEUTRAL_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
const BADGE_ACCENT_FG: Color = Color::from_rgb8(0xff, 0xb6, 0x27);

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// `crate::cupertino::button`'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Which semantic variant a [`BadgeView`] renders as — see the module docs
/// for the color-resolution precedence per variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeVariant {
    /// A faint success-hued wash (e.g. "connected"/"live").
    Success,
    /// A faint warning-hued wash (e.g. "degraded").
    Warning,
    /// A faint error-hued wash (e.g. "offline"/"stopped").
    Error,
    /// A neutral raised-surface wash with a hairline border (e.g.
    /// "read-only"/"idle").
    Neutral,
    /// The Glyph accent (amber) as a faint wash (e.g. a version tag).
    Accent,
}

/// A declarative status badge. See the [module docs](self).
pub struct BadgeView {
    label: String,
    variant: BadgeVariant,
    dot: bool,
}

/// Create a badge labelled `label` (rendered lowercase, per the source's
/// `text-transform:lowercase`) in the given semantic `variant`.
pub fn badge(label: impl Into<String>, variant: BadgeVariant) -> BadgeView {
    BadgeView {
        label: label.into(),
        variant,
        dot: false,
    }
}

impl BadgeView {
    /// Show (or hide) the leading status dot, colored to match the label.
    pub fn dot(mut self, dot: bool) -> Self {
        self.dot = dot;
        self
    }
}

/// A minimal retained text run: shapes lazily during layout and paints via
/// glyph runs. A self-contained mirror of `crate::text::TextWidget`'s cache
/// shape (see the module docs for why `Badge`/`Tag`/`Alert` don't nest a
/// `Text` child).
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

    /// Replace the content, invalidating the cached shaped layout if it
    /// actually changed.
    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    /// Shape (or reuse the cached shape of) this run at `style`/`max_width`,
    /// returning its size. Re-shapes whenever either input differs from the
    /// last pass — including a color-only `style` change (the layout-time-baked
    /// color contract, see the module docs).
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

    /// Emit this run's glyphs at absolute `origin`. A no-op before the first
    /// [`GlyphLabel::layout`] call.
    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// The retained widget for a [`BadgeView`].
pub struct BadgeWidget {
    label: GlyphLabel,
    label_text: String,
    label_size: Size,
    variant: BadgeVariant,
    dot: bool,
}

impl<State: 'static> View<State> for BadgeView {
    type Element = BadgeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BadgeWidget {
        BadgeWidget {
            label: GlyphLabel::new(self.label.to_lowercase()),
            label_text: self.label.clone(),
            label_size: Size::ZERO,
            variant: self.variant,
            dot: self.dot,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BadgeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.to_lowercase());
            element.label_text = self.label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.dot != self.dot {
            element.dot = self.dot;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// The label's fixed style (family/weight/tracking are Glyph-authored
/// constants, not theme-resolved — see the module docs); only `color` varies.
fn badge_label_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        letter_spacing: BADGE_LETTER_SPACING,
        ..TextStyle::new(BADGE_FONT_SIZE, color)
    }
}

/// Resolve `(background, foreground, border)` for `variant` — see the module
/// docs for the full precedence.
fn resolve_badge_colors(
    theme: Option<&Theme>,
    variant: BadgeVariant,
) -> (Color, Color, Option<Color>) {
    let Some(theme) = theme else {
        return unthemed_badge_colors(variant);
    };
    let scheme = theme.scheme();
    match variant {
        BadgeVariant::Success => match theme.extension::<StatusPalette>() {
            Some(status) => {
                let c = status.colors(theme.brightness);
                (c.success_container, c.success, None)
            }
            None => (BADGE_SUCCESS_BG, BADGE_SUCCESS_FG, None),
        },
        BadgeVariant::Warning => match theme.extension::<StatusPalette>() {
            Some(status) => {
                let c = status.colors(theme.brightness);
                (c.warning_container, c.warning, None)
            }
            None => (BADGE_WARNING_BG, BADGE_WARNING_FG, None),
        },
        BadgeVariant::Error => (scheme.error_container, scheme.error, None),
        BadgeVariant::Neutral => (
            scheme.surface_container_high,
            scheme.on_surface_variant,
            Some(scheme.outline),
        ),
        BadgeVariant::Accent => (
            with_alpha(scheme.primary, BADGE_ACCENT_WASH_ALPHA),
            scheme.primary,
            None,
        ),
    }
}

fn unthemed_badge_colors(variant: BadgeVariant) -> (Color, Color, Option<Color>) {
    match variant {
        BadgeVariant::Success => (BADGE_SUCCESS_BG, BADGE_SUCCESS_FG, None),
        BadgeVariant::Warning => (BADGE_WARNING_BG, BADGE_WARNING_FG, None),
        BadgeVariant::Error => (BADGE_ERROR_BG, BADGE_ERROR_FG, None),
        BadgeVariant::Neutral => (
            BADGE_NEUTRAL_BG,
            BADGE_NEUTRAL_FG,
            Some(BADGE_NEUTRAL_BORDER),
        ),
        BadgeVariant::Accent => (
            with_alpha(BADGE_ACCENT_FG, BADGE_ACCENT_WASH_ALPHA),
            BADGE_ACCENT_FG,
            None,
        ),
    }
}

/// The pill radius: themed `shape.full` resolved against the box (always a
/// true pill, since `full` is infinite in every baseline). Unthemed: half the
/// box height exactly, the same pill shape without a theme to resolve.
fn resolve_badge_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.full, size.width, size.height),
        None => size.height / 2.0,
    }
}

impl Widget for BadgeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, fg, _) = resolve_badge_colors(theme, self.variant);
        let style = badge_label_style(fg);
        let label_size = self.label.layout(ctx, &style, None);
        self.label_size = label_size;

        let dot_extra = if self.dot {
            BADGE_DOT_SIZE + BADGE_GAP
        } else {
            0.0
        };
        let width = BADGE_PAD_X * 2.0 + dot_extra + label_size.width;
        let content_height = if self.dot {
            label_size.height.max(BADGE_DOT_SIZE)
        } else {
            label_size.height
        };
        let height = content_height + BADGE_PAD_Y * 2.0;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, fg, border) = resolve_badge_colors(theme, self.variant);
        let radius = resolve_badge_radius(theme, ctx.size());
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, bg);
        if let Some(border_color) = border {
            let rr =
                RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, ctx.size()), radius);
            let path = rr.to_path(PATH_TOLERANCE);
            scene.stroke_path(
                ctx.origin(),
                &path,
                BADGE_BORDER_WIDTH,
                &Brush::Solid(border_color),
            );
        }

        let mut x = BADGE_PAD_X;
        if self.dot {
            let y = (ctx.size().height - BADGE_DOT_SIZE) / 2.0;
            scene.fill_rounded_rect(
                ctx.origin() + Vec2::new(x, y),
                Size::new(BADGE_DOT_SIZE, BADGE_DOT_SIZE),
                BADGE_DOT_SIZE / 2.0,
                fg,
            );
            x += BADGE_DOT_SIZE + BADGE_GAP;
        }
        let label_y = (ctx.size().height - self.label_size.height) / 2.0;
        self.label
            .paint(ctx.origin() + Vec2::new(x, label_y), scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A live status indicator: `Role::Status` (not `Label`), carrying the
        // original (non-lowercased) label text so a screen reader announces
        // natural casing.
        ctx.push_node(Role::Status, |node| {
            node.set_label(self.label_text.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_theme::Brightness;
    use std::any::Any;

    /// Records fills/borders/glyph-run brushes, mirroring
    /// `crate::material::chips`'s `RRectRecorder` plus a glyph-run capture.
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
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn build_widget(variant: BadgeVariant, dot: bool) -> BadgeWidget {
        let view = badge("Connected", variant).dot(dot);
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    // `badge()` doesn't need a turbofish in practice (state is inferred from
    // the `View<State>` call site), but the test harness builds it directly.
    fn badge_helper(label: &str, variant: BadgeVariant) -> BadgeView {
        badge(label, variant)
    }

    fn layout_and_paint(widget: &mut BadgeWidget, theme: Option<&Theme>) -> Recorder {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)));
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_success_uses_fallback_constants() {
        let _ = badge_helper("x", BadgeVariant::Success); // exercise the fn form too
        let mut w = build_widget(BadgeVariant::Success, false);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].3, BADGE_SUCCESS_BG);
        assert_eq!(rec.glyph_colors[0], BADGE_SUCCESS_FG);
    }

    #[test]
    fn unthemed_neutral_has_a_border() {
        let mut w = build_widget(BadgeVariant::Neutral, false);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].3, BADGE_NEUTRAL_BG);
        assert_eq!(rec.strokes[0], BADGE_NEUTRAL_BORDER);
    }

    #[test]
    fn unthemed_accent_is_a_true_alpha_wash() {
        let mut w = build_widget(BadgeVariant::Accent, false);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(BADGE_ACCENT_FG, BADGE_ACCENT_WASH_ALPHA)
        );
        assert_eq!(rec.glyph_colors[0], BADGE_ACCENT_FG);
    }

    #[test]
    fn glyph_dark_theme_resolves_success_from_status_palette() {
        let theme = Theme::glyph_baseline();
        let mut w = build_widget(BadgeVariant::Success, false);
        let rec = layout_and_paint(&mut w, Some(&theme));
        let status = theme.extension::<StatusPalette>().unwrap();
        let colors = status.colors(Brightness::Dark);
        assert_eq!(rec.rrects[0].3, colors.success_container);
        assert_eq!(rec.glyph_colors[0], colors.success);
    }

    #[test]
    fn glyph_light_theme_resolves_warning_from_status_palette() {
        let theme = Theme::glyph_baseline().with_brightness(Brightness::Light);
        let mut w = build_widget(BadgeVariant::Warning, false);
        let rec = layout_and_paint(&mut w, Some(&theme));
        let status = theme.extension::<StatusPalette>().unwrap();
        let colors = status.colors(Brightness::Light);
        assert_eq!(rec.rrects[0].3, colors.warning_container);
        assert_eq!(rec.glyph_colors[0], colors.warning);
    }

    #[test]
    fn glyph_theme_resolves_error_from_color_scheme_directly() {
        let theme = Theme::glyph_baseline();
        let mut w = build_widget(BadgeVariant::Error, false);
        let rec = layout_and_paint(&mut w, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().error_container);
        assert_eq!(rec.glyph_colors[0], theme.scheme().error);
    }

    #[test]
    fn glyph_theme_resolves_radius_as_a_pill() {
        let theme = Theme::glyph_baseline();
        let mut w = build_widget(BadgeVariant::Success, false);
        let rec = layout_and_paint(&mut w, Some(&theme));
        let (_, size, radius, _) = rec.rrects[0];
        assert_eq!(radius, size.height / 2.0);
    }

    #[test]
    fn dot_widens_the_badge_and_paints_a_dot_colored_like_the_label() {
        let mut plain = build_widget(BadgeVariant::Success, false);
        let mut dotted = build_widget(BadgeVariant::Success, true);
        let rec_plain = layout_and_paint(&mut plain, None);
        let rec_dotted = layout_and_paint(&mut dotted, None);
        assert!(rec_dotted.rrects[0].1.width > rec_plain.rrects[0].1.width);
        // rrects[0] is the pill background; rrects[1] is the dot.
        assert_eq!(rec_dotted.rrects.len(), 2);
        assert_eq!(rec_dotted.rrects[1].3, BADGE_SUCCESS_FG);
    }

    #[test]
    fn label_is_rendered_lowercase() {
        let w = build_widget(BadgeVariant::Neutral, false);
        // Not directly observable via the recorder (glyph runs carry shaped
        // glyphs, not the source string), but the retained content is:
        assert_eq!(w.label.content, "connected");
        assert_eq!(w.label_text, "Connected");
    }

    #[test]
    fn semantics_reports_status_role_and_original_casing() {
        fn logic(_s: &mut ()) -> BadgeView {
            badge("Connected", BadgeVariant::Success)
        }
        let mut root: frust_core::RenderRoot<(), BadgeView> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Status)
            .expect("badge contributes a Role::Status node");
        assert_eq!(node.label(), Some("Connected"));
    }
}
