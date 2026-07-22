//! Filled by task 24-glyph-content-widgets (StatCard).
//!
//! [`stat_card`]/[`StatCardView`]: a compact metric readout — an uppercase dim
//! **label**, a large display-family **value**, and an optional semantic
//! **delta** (a signed change badge) — inside the same surface/bright-border/
//! radius-lg chrome as [`super::card`] (the reference build's `§content`
//! `.stat-card`). A leaf display widget with no interaction and three
//! text runs, so — like [`super::badge`] — it shapes and paints its own glyph
//! runs rather than nesting [`crate::text::TextView`] children, and its `View`
//! impl is generic over any `State`.
//!
//! # Color resolution
//!
//! - **label** resolves `on_surface_variant` (Glyph `fg-muted`), rendered
//!   uppercase with the micro tracking.
//! - **value** resolves `on_surface` (Glyph `fg`) in the Space Mono display
//!   face at `700`.
//! - **delta** resolves a semantic hue by [`StatDelta`] direction —
//!   [`StatDelta::Up`] the `StatusPalette` success green, [`StatDelta::Down`]
//!   the `ColorScheme::error` red (M3's baseline already carries an error
//!   role, so no extension is needed for it — the same split
//!   [`super::badge`]'s Error/Success variants use).
//! - **background/border/radius** mirror [`super::card`] exactly
//!   (`surface_container` / `outline` / `shape.large`).
//!
//! Unthemed, every value falls back to a literal Glyph **dark** constant.

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust_text::{FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle};
use frust_theme::{ShapeScale, StatusPalette, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

/// Corner-rounding tolerance for the border stroke.
const PATH_TOLERANCE: f64 = 0.1;

/// Content padding on all four edges, in logical px.
const STAT_PADDING: f64 = 16.0;
/// Gap between the label and the value, in logical px.
const STAT_LABEL_GAP: f64 = 6.0;
/// Gap between the value and the delta, in logical px.
const STAT_DELTA_GAP: f64 = 6.0;
/// Border width, in logical px.
const STAT_BORDER_WIDTH: f64 = 1.0;

/// Label font size, in logical px (`.stat-label{font-size:10.5px}`).
const STAT_LABEL_SIZE: f32 = 10.5;
/// Label letter-spacing (`0.08em` uppercase tracking == `0.08 * 10.5`).
const STAT_LABEL_TRACKING: f32 = STAT_LABEL_SIZE * 0.08;
/// Value font size, in logical px (`.stat-value{font-size:24px;font-weight:700}`).
const STAT_VALUE_SIZE: f32 = 24.0;
/// Delta font size, in logical px.
const STAT_DELTA_SIZE: f32 = 11.5;

/// Unthemed corner-radius fallback — Glyph `--radius-lg` (16px).
const STAT_RADIUS_FALLBACK: f64 = 16.0;

// ---- Unthemed fallback constants (Glyph **dark** values) ---------------

const STAT_BG: Color = Color::from_rgb8(0x16, 0x1a, 0x23); // bg-surface
const STAT_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // outline / border-bright
const STAT_LABEL_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted
const STAT_VALUE_FG: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // fg
const STAT_UP_FG: Color = Color::from_rgb8(0x5f, 0xd8, 0x8f); // success
const STAT_DOWN_FG: Color = Color::from_rgb8(0xff, 0x6b, 0x6b); // error

/// The direction a [`StatCardView`]'s delta represents. Drives its semantic
/// color (up = success green, down = error red) and its leading arrow glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatDelta {
    /// A positive change (success-hued, `↑` prefix).
    Up,
    /// A negative change (error-hued, `↓` prefix).
    Down,
}

impl StatDelta {
    /// The leading arrow glyph for this direction.
    fn arrow(self) -> &'static str {
        match self {
            StatDelta::Up => "\u{2191}",   // ↑
            StatDelta::Down => "\u{2193}", // ↓
        }
    }
}

/// A declarative statistic card. See the [module docs](self).
pub struct StatCardView {
    label: String,
    value: String,
    delta: Option<(StatDelta, String)>,
}

/// Create a stat card with an uppercase `label` and a display `value`. Chain
/// [`StatCardView::delta`] to add a signed change badge.
pub fn stat_card(label: impl Into<String>, value: impl Into<String>) -> StatCardView {
    StatCardView {
        label: label.into(),
        value: value.into(),
        delta: None,
    }
}

impl StatCardView {
    /// Add a semantic delta badge (`direction` colors it; `text` is rendered
    /// after the direction's arrow glyph, e.g. `delta(StatDelta::Up, "12%")`
    /// → "↑ 12%").
    pub fn delta(mut self, direction: StatDelta, text: impl Into<String>) -> Self {
        self.delta = Some((direction, text.into()));
        self
    }
}

/// A minimal retained text run — see [`super::badge`]'s `GlyphLabel`.
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

/// The retained widget for a [`StatCardView`].
pub struct StatCardWidget {
    label: GlyphLabel,
    label_text: String,
    label_size: Size,
    value: GlyphLabel,
    value_text: String,
    value_size: Size,
    delta_dir: Option<StatDelta>,
    delta: Option<GlyphLabel>,
    delta_size: Size,
}

impl<State: 'static> View<State> for StatCardView {
    type Element = StatCardWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> StatCardWidget {
        let (delta_dir, delta) = match &self.delta {
            Some((dir, text)) => (
                Some(*dir),
                Some(GlyphLabel::new(format!("{} {}", dir.arrow(), text))),
            ),
            None => (None, None),
        };
        StatCardWidget {
            label: GlyphLabel::new(self.label.to_uppercase()),
            label_text: self.label.clone(),
            label_size: Size::ZERO,
            value: GlyphLabel::new(self.value.clone()),
            value_text: self.value.clone(),
            value_size: Size::ZERO,
            delta_dir,
            delta,
            delta_size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut StatCardWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.to_uppercase());
            element.label_text = self.label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.value != self.value {
            element.value.set_content(self.value.clone());
            element.value_text = self.value.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.delta != self.delta {
            match &self.delta {
                Some((dir, text)) => {
                    let content = format!("{} {}", dir.arrow(), text);
                    element.delta_dir = Some(*dir);
                    match &mut element.delta {
                        Some(label) => label.set_content(content),
                        None => element.delta = Some(GlyphLabel::new(content)),
                    }
                }
                None => {
                    element.delta_dir = None;
                    element.delta = None;
                }
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

fn label_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        letter_spacing: STAT_LABEL_TRACKING,
        ..TextStyle::new(STAT_LABEL_SIZE, color)
    }
}

fn value_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace),
        weight: FontWeight::BOLD,
        ..TextStyle::new(STAT_VALUE_SIZE, color)
    }
}

fn delta_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(STAT_DELTA_SIZE, color)
    }
}

/// Resolve `(bg, border, label, value)` chrome/text colors.
fn resolve_chrome(theme: Option<&Theme>) -> (Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.surface_container,
                s.outline,
                s.on_surface_variant,
                s.on_surface,
            )
        }
        None => (STAT_BG, STAT_BORDER, STAT_LABEL_FG, STAT_VALUE_FG),
    }
}

/// Resolve a delta's semantic color for `direction`.
fn resolve_delta_color(theme: Option<&Theme>, direction: StatDelta) -> Color {
    match theme {
        Some(theme) => match direction {
            StatDelta::Up => match theme.extension::<StatusPalette>() {
                Some(status) => status.colors(theme.brightness).success,
                None => STAT_UP_FG,
            },
            StatDelta::Down => theme.scheme().error,
        },
        None => match direction {
            StatDelta::Up => STAT_UP_FG,
            StatDelta::Down => STAT_DOWN_FG,
        },
    }
}

fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.large, size.width, size.height),
        None => STAT_RADIUS_FALLBACK,
    }
}

impl Widget for StatCardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve every color up front (copying out of the borrowed theme)
        // before any `.layout` call needs `ctx` mutably.
        let theme = Theme::from_layout_ctx(ctx);
        let (_, _, label_fg, value_fg) = resolve_chrome(theme);
        let delta_color = self.delta_dir.map(|dir| resolve_delta_color(theme, dir));

        self.label_size = self.label.layout(ctx, &label_style(label_fg), None);
        self.value_size = self.value.layout(ctx, &value_style(value_fg), None);

        let mut content_height = self.label_size.height + STAT_LABEL_GAP + self.value_size.height;
        let mut content_width = self.label_size.width.max(self.value_size.width);

        if let (Some(color), Some(delta)) = (delta_color, self.delta.as_mut()) {
            self.delta_size = delta.layout(ctx, &delta_style(color), None);
            content_height += STAT_DELTA_GAP + self.delta_size.height;
            content_width = content_width.max(self.delta_size.width);
        } else {
            self.delta_size = Size::ZERO;
        }

        let width = content_width + STAT_PADDING * 2.0;
        let height = content_height + STAT_PADDING * 2.0;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, border, _, _) = resolve_chrome(theme);
        let radius = resolve_radius(theme, ctx.size());
        let o = ctx.origin();
        let size = ctx.size();

        scene.fill_rounded_rect(o, size, radius, bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(o, &path, STAT_BORDER_WIDTH, &Brush::Solid(border));

        let mut y = STAT_PADDING;
        self.label.paint(o + Vec2::new(STAT_PADDING, y), scene);
        y += self.label_size.height + STAT_LABEL_GAP;
        self.value.paint(o + Vec2::new(STAT_PADDING, y), scene);
        y += self.value_size.height + STAT_DELTA_GAP;
        if let Some(delta) = &self.delta {
            delta.paint(o + Vec2::new(STAT_PADDING, y), scene);
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A single readout: label as the accessible name, value (+ delta) as
        // its value/description so a screen reader announces the metric.
        let description = match (&self.delta_dir, &self.delta) {
            (Some(dir), Some(_)) => {
                let word = match dir {
                    StatDelta::Up => "up",
                    StatDelta::Down => "down",
                };
                format!("{}, {}", self.value_text, word)
            }
            _ => self.value_text.clone(),
        };
        ctx.push_node(Role::Label, |node| {
            node.set_label(self.label_text.as_str());
            node.set_description(description.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_theme::Brightness;
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
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn build(view: &StatCardView) -> StatCardWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint(widget: &mut StatCardWidget, theme: Option<&Theme>) -> Recorder {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_chrome_and_text_use_fallback_constants() {
        let mut w = build(&stat_card("Sessions", "1,284"));
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].3, STAT_BG);
        assert_eq!(rec.strokes[0], STAT_BORDER);
        assert_eq!(rec.glyph_colors[0], STAT_LABEL_FG); // label
        assert_eq!(rec.glyph_colors[1], STAT_VALUE_FG); // value
    }

    #[test]
    fn label_rendered_uppercase_but_semantics_keep_casing() {
        let w = build(&stat_card("Sessions", "1,284"));
        assert_eq!(w.label.content, "SESSIONS");
        assert_eq!(w.label_text, "Sessions");
    }

    #[test]
    fn delta_up_is_success_hued_dark_and_light() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(Brightness::Light);

        let mut wd = build(&stat_card("Uptime", "99.9%").delta(StatDelta::Up, "0.2%"));
        let rec_d = layout_and_paint(&mut wd, Some(&dark));
        let success_d = dark
            .extension::<StatusPalette>()
            .unwrap()
            .colors(Brightness::Dark)
            .success;
        assert_eq!(*rec_d.glyph_colors.last().unwrap(), success_d);

        let mut wl = build(&stat_card("Uptime", "99.9%").delta(StatDelta::Up, "0.2%"));
        let rec_l = layout_and_paint(&mut wl, Some(&light));
        let success_l = light
            .extension::<StatusPalette>()
            .unwrap()
            .colors(Brightness::Light)
            .success;
        assert_eq!(*rec_l.glyph_colors.last().unwrap(), success_l);
        assert_ne!(success_d, success_l);
    }

    #[test]
    fn delta_down_is_error_hued() {
        let theme = Theme::glyph_baseline();
        let mut w = build(&stat_card("Errors", "12").delta(StatDelta::Down, "3"));
        let rec = layout_and_paint(&mut w, Some(&theme));
        assert_eq!(*rec.glyph_colors.last().unwrap(), theme.scheme().error);
    }

    #[test]
    fn delta_glyph_runs_carry_arrow_prefix() {
        let w = build(&stat_card("x", "y").delta(StatDelta::Up, "5%"));
        assert_eq!(w.delta.as_ref().unwrap().content, "\u{2191} 5%");
    }

    #[test]
    fn no_delta_paints_only_label_and_value_runs() {
        let mut w = build(&stat_card("x", "y"));
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.glyph_colors.len(), 2);
        assert!(w.delta.is_none());
    }

    #[test]
    fn removing_delta_on_rebuild_clears_it() {
        let mut w = build(&stat_card("x", "y").delta(StatDelta::Up, "5%"));
        assert!(w.delta.is_some());
        let mut counter = 0u64;
        <StatCardView as View<()>>::rebuild(
            &stat_card("x", "y"),
            &stat_card("x", "y").delta(StatDelta::Up, "5%"),
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(w.delta.is_none());
        assert!(w.delta_dir.is_none());
    }

    #[test]
    fn glyph_light_chrome_differs_from_dark() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(Brightness::Light);
        let mut wd = build(&stat_card("x", "y"));
        let mut wl = build(&stat_card("x", "y"));
        let rec_d = layout_and_paint(&mut wd, Some(&dark));
        let rec_l = layout_and_paint(&mut wl, Some(&light));
        assert_eq!(rec_d.rrects[0].3, dark.scheme().surface_container);
        assert_eq!(rec_l.rrects[0].3, light.scheme().surface_container);
        assert_ne!(rec_d.rrects[0].3, rec_l.rrects[0].3);
    }

    #[test]
    fn semantics_reports_label_and_value_description() {
        fn logic(_s: &mut ()) -> StatCardView {
            stat_card("Sessions", "1,284").delta(StatDelta::Up, "12%")
        }
        let mut root: frust_core::RenderRoot<(), StatCardView> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Label)
            .expect("stat card contributes a Role::Label node");
        assert_eq!(node.label(), Some("Sessions"));
        assert_eq!(node.description(), Some("1,284, up"));
    }
}
