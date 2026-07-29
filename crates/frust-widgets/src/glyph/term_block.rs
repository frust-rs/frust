//! [`term_block`]/[`TermBlockView`]: a rounded terminal-style output block —
//! the Glyph design system's `.term-block`/`.term-head`/
//! `.term-body` rules (retrieved 2026-07-21): a three-dot header strip over a
//! monospace body of [`TermLine`]s, each typed `Prompt`/`Output`/`Comment`
//! with its own ink color.
//!
//! # Brightness invariance
//!
//! Every painted color here is either a [`GlyphInk`] field or one of this
//! module's own literal fallback constants — **never** a brightness-swapped
//! `ColorScheme`/`StatusPalette` role, even though the design system's own
//! CSS does source the header dots from `var(--error)`/`var(--warning)`/
//! `var(--success)` (brightness-swapped tokens) and the border/shadow from
//! brightness-swapped `var(--border...)` roles. This is a deliberate
//! deviation from literal source fidelity: this widget requires **identical
//! paint under Light and Dark [`Theme::glyph_baseline`]
//! brightness**, so the dot/border colors below are fixed to the **dark**
//! scheme's literal values rather than resolved dynamically —
//! consistent with [`GlyphInk`]'s own "stays dark in both brightnesses"
//! precedent (`frust_theme::glyph::color`'s module docs).
//!
//! [`GlyphInk`] itself resolves themed (`theme.extension::<GlyphInk>()`) with
//! an unthemed fallback of [`GlyphInk::default_ink`] — the catalog's normal
//! token-resolution convention, just applied to a token table that happens to
//! never vary by brightness.
//!
//! # Header dots and border
//!
//! The `border-bright` hairline (`rgba(242,234,217,.18)`) is pre-flattened
//! over [`GlyphInk::terminal_bg`] (`#1c1912`) into [`TERM_BORDER`] — the same
//! alpha-preflattening convention `frust_theme::glyph::color`'s module docs
//! use for opaque roles composited over a known background. The header strip
//! paints as a plain rectangle (not clipped to the block's own rounded
//! corners — the scene has no rounded-rect clip primitive), so its two top
//! corners sit square against the block's rounded corner arc: a small,
//! accepted v1 visual simplification, not a rounded clip.
//!
//! # Optional staggered line reveal
//!
//! [`TermBlockView::staggered`] opts into a one-shot, mount-time reveal built
//! on [`crate::motion::patterns::GlyphStagger::glyph`] (the
//! `StaggerSpec` idiom, explicitly documented there as a `TermBlock` consumer)
//! — 90ms per-line delay, 150ms `effects`-eased per-line fade-in — driven by
//! one `AnimationController` sized to
//! [`GlyphStagger::total_duration`](crate::motion::patterns::GlyphStagger::total_duration)`(n)`
//! and started with `.forward()` on build. `reduce_motion` is threaded
//! straight into
//! [`GlyphStagger::item_progress`](crate::motion::patterns::GlyphStagger::item_progress)'s
//! own collapse: every line reveals together on the shared timeline with no
//! per-line delay, a single fast synchronized fade rather than a cascade
//! (a documented contract — not an instantaneous jump to fully
//! shown). Lines render fully visible immediately, with no controller built
//! at all, only when `staggered` is left at its `false` default.

use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, Curve, LayoutCtx, PaintCtx,
    PaintScene, SemanticsCtx, View, Widget,
};
use frust_text::{
    FontFamily, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust_theme::{GlyphInk, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::motion::patterns::GlyphStagger;

// ---- Layout constants (from the design system's source CSS) --------------

/// Header strip horizontal padding (`.term-head{padding:9px 12px}`).
const HEAD_PAD_X: f64 = 12.0;
/// Header strip vertical padding (`.term-head{padding:9px 12px}`).
const HEAD_PAD_Y: f64 = 9.0;
/// Header dot diameter (`.term-head .dot{width:8px;height:8px}`).
const DOT_D: f64 = 8.0;
/// Gap between header dots (`.term-head{gap:6px}`).
const DOT_GAP: f64 = 6.0;
/// Total header strip height, derived from the two constants above.
const HEADER_HEIGHT: f64 = HEAD_PAD_Y * 2.0 + DOT_D;

/// Body padding (`.term-body{padding:14px 16px}`).
const BODY_PAD_X: f64 = 16.0;
const BODY_PAD_Y: f64 = 14.0;
/// Body font size (`.term-body{font-size:11.5px}`).
const FONT_SIZE: f32 = 11.5;
/// Body line height (`.term-body{line-height:1.8}`).
const LINE_HEIGHT: f32 = 1.8;
/// Border stroke width (`.term-block{border:1px solid ...}`).
const BORDER_WIDTH: f64 = 1.0;
/// Corner-rounding tolerance for the border stroke (matches
/// `crate::glyph::badge`'s constant of the same name).
const PATH_TOLERANCE: f64 = 0.1;

/// The prompt marker glyph, with its trailing space (`<span
/// class="term-prompt">$</span> <command>` — the space is plain HTML text
/// between the marker span and the command, reproduced here as part of the
/// marker run so the command text needs no leading space of its own).
const PROMPT_MARKER: &str = "$ ";

/// Unthemed corner-radius fallback — Glyph's canonical `--radius-md` (10px).
const FALLBACK_RADIUS: f64 = 10.0;

// ---- Fixed (never brightness-swapped) colors — see the module docs -------

/// Header dot colors, in source order (error, warning, success — the
/// classic traffic-light ordering). Fixed to the Glyph **dark** scheme's
/// literal values regardless of the active brightness (see the module docs).
const TERM_DOT_ERROR: Color = Color::from_rgb8(0xff, 0x6b, 0x6b);
const TERM_DOT_WARNING: Color = Color::from_rgb8(0xf5, 0xc8, 0x60);
const TERM_DOT_SUCCESS: Color = Color::from_rgb8(0x5f, 0xd8, 0x8f);

/// `border-bright` (`rgba(242,234,217,.18)`) pre-flattened over
/// [`GlyphInk::terminal_bg`] (`#1c1912`) — see the module docs.
const TERM_BORDER: Color = Color::from_rgb8(0x43, 0x3f, 0x36);

/// Which kind of line a [`TermLine`] is — each has its own ink color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermLineKind {
    /// A command line: the amber `$ ` marker plus [`GlyphInk::terminal_fg`]
    /// command text.
    Prompt,
    /// Command output, in [`GlyphInk::terminal_out`].
    Output,
    /// An annotation/comment, in [`GlyphInk::terminal_comment`] (the dimmest
    /// tier).
    Comment,
}

/// One line of a [`TermBlockView`]'s body. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub struct TermLine {
    kind: TermLineKind,
    text: String,
}

impl TermLine {
    /// A command line — rendered as an amber `$ ` marker followed by `text`.
    pub fn prompt(text: impl Into<String>) -> Self {
        Self {
            kind: TermLineKind::Prompt,
            text: text.into(),
        }
    }

    /// A command-output line.
    pub fn output(text: impl Into<String>) -> Self {
        Self {
            kind: TermLineKind::Output,
            text: text.into(),
        }
    }

    /// A comment/annotation line.
    pub fn comment(text: impl Into<String>) -> Self {
        Self {
            kind: TermLineKind::Comment,
            text: text.into(),
        }
    }
}

/// A declarative Glyph terminal block. See the [module docs](self).
pub struct TermBlockView {
    lines: Vec<TermLine>,
    staggered: bool,
}

/// Create a terminal block over `lines` (no staggered reveal by default — opt
/// in with [`TermBlockView::staggered`]).
pub fn term_block(lines: impl Into<Vec<TermLine>>) -> TermBlockView {
    TermBlockView {
        lines: lines.into(),
        staggered: false,
    }
}

/// PascalCase alias for [`term_block`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn TermBlock(lines: impl Into<Vec<TermLine>>) -> TermBlockView {
    term_block(lines)
}

impl TermBlockView {
    /// Opt into the staggered per-line reveal (see the [module docs](self)).
    pub fn staggered(mut self, staggered: bool) -> Self {
        self.staggered = staggered;
        self
    }
}

/// A minimal retained text run — see `crate::glyph::badge`'s `GlyphLabel` for
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

/// The retained per-line state: the shaped marker (Prompt only)/content runs,
/// plus their last-measured sizes (reused between `layout` and `paint`).
struct TermLineRuns {
    kind: TermLineKind,
    text: String,
    marker: Option<GlyphLabel>,
    content: GlyphLabel,
    marker_size: Size,
    content_size: Size,
}

impl TermLineRuns {
    fn new(line: &TermLine) -> Self {
        Self {
            kind: line.kind,
            text: line.text.clone(),
            marker: matches!(line.kind, TermLineKind::Prompt)
                .then(|| GlyphLabel::new(PROMPT_MARKER)),
            content: GlyphLabel::new(line.text.clone()),
            marker_size: Size::ZERO,
            content_size: Size::ZERO,
        }
    }

    fn line_height(&self) -> f64 {
        self.content_size.height.max(self.marker_size.height)
    }
}

/// The retained widget for a [`TermBlockView`]. See the [module docs](self).
pub struct TermBlockWidget {
    lines: Vec<TermLineRuns>,
    staggered: bool,
    /// Drives the one-shot stagger reveal (`Some` only while `staggered` and
    /// not yet fully settled at `1.0` — kept around afterward too, since
    /// re-advancing a settled controller is a cheap no-op and simpler than
    /// tearing it down).
    stagger: Option<AnimationController>,
}

fn mono_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        line_height: LineHeight::FontSizeRelative(LINE_HEIGHT),
        ..TextStyle::new(FONT_SIZE, color)
    }
}

/// `GlyphInk`, themed (`theme.extension::<GlyphInk>()`) or the unthemed
/// [`GlyphInk::default_ink`] fallback.
fn resolve_ink(theme: Option<&Theme>) -> GlyphInk {
    theme
        .and_then(|t| t.extension::<GlyphInk>().copied())
        .unwrap_or_else(GlyphInk::default_ink)
}

/// The corner radius: themed `shape.medium`, or [`FALLBACK_RADIUS`] unthemed.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    theme.map(|t| t.shape.medium).unwrap_or(FALLBACK_RADIUS)
}

/// A line's content-run color, per [`TermLineKind`] — see the [module docs](self).
fn content_color(kind: TermLineKind, ink: &GlyphInk) -> Color {
    match kind {
        TermLineKind::Prompt => ink.terminal_fg,
        TermLineKind::Output => ink.terminal_out,
        TermLineKind::Comment => ink.terminal_comment,
    }
}

fn build_controller(n: usize) -> AnimationController {
    let total_ms = GlyphStagger::glyph().total_duration(n);
    let mut c = AnimationController::new(Duration::from_millis(total_ms.round().max(0.0) as u64))
        .with_curve(Curve::Linear);
    c.forward();
    c
}

impl<State: 'static> View<State> for TermBlockView {
    type Element = TermBlockWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TermBlockWidget {
        TermBlockWidget {
            lines: self.lines.iter().map(TermLineRuns::new).collect(),
            staggered: self.staggered,
            stagger: self.staggered.then(|| build_controller(self.lines.len())),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TermBlockWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.lines != self.lines {
            element.lines = self.lines.iter().map(TermLineRuns::new).collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            if self.staggered {
                element.stagger = Some(build_controller(self.lines.len()));
            }
        }
        if prev.staggered != self.staggered {
            element.staggered = self.staggered;
            element.stagger = self.staggered.then(|| build_controller(self.lines.len()));
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for TermBlockWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let ink = resolve_ink(theme);

        let max_width = bc.max().width;
        let text_max_width = if max_width.is_finite() {
            Some((max_width - BODY_PAD_X * 2.0).max(0.0) as f32)
        } else {
            None
        };

        let mut max_line_w = 0.0f64;
        let mut body_height = 0.0f64;
        for line in &mut self.lines {
            let marker_size = match &mut line.marker {
                Some(m) => m.layout(ctx, &mono_style(ink.terminal_prompt), None),
                None => Size::ZERO,
            };
            let content_max = text_max_width.map(|w| (w - marker_size.width as f32).max(0.0));
            let content_size = line.content.layout(
                ctx,
                &mono_style(content_color(line.kind, &ink)),
                content_max,
            );
            line.marker_size = marker_size;
            line.content_size = content_size;
            max_line_w = max_line_w.max(marker_size.width + content_size.width);
            body_height += content_size.height.max(marker_size.height);
        }

        let header_natural_width = HEAD_PAD_X * 2.0 + DOT_D * 3.0 + DOT_GAP * 2.0;
        let width = if max_width.is_finite() {
            max_width
        } else {
            (max_line_w + BODY_PAD_X * 2.0).max(header_natural_width)
        };
        let height = HEADER_HEIGHT + body_height + BODY_PAD_Y * 2.0;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let ink = resolve_ink(theme);
        let radius = resolve_radius(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        scene.fill_rounded_rect(origin, size, radius, ink.terminal_bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(origin, &path, BORDER_WIDTH, &Brush::Solid(TERM_BORDER));

        scene.fill_rect(
            origin,
            Size::new(size.width, HEADER_HEIGHT),
            ink.terminal_head,
        );
        let dot_y = origin.y + (HEADER_HEIGHT - DOT_D) / 2.0;
        for (i, color) in [TERM_DOT_ERROR, TERM_DOT_WARNING, TERM_DOT_SUCCESS]
            .into_iter()
            .enumerate()
        {
            let dot_x = origin.x + HEAD_PAD_X + i as f64 * (DOT_D + DOT_GAP);
            scene.fill_rounded_rect(
                Point::new(dot_x, dot_y),
                Size::new(DOT_D, DOT_D),
                DOT_D / 2.0,
                color,
            );
        }

        let n = self.lines.len();
        let animating = self.staggered
            && self
                .stagger
                .as_mut()
                .map(|c| c.advance(ctx.frame_time()))
                .unwrap_or(false);
        let overall = self
            .stagger
            .as_ref()
            .map(|c| c.value_clamped())
            .unwrap_or(1.0);
        let stagger_spec = GlyphStagger::glyph();

        let mut y = origin.y + HEADER_HEIGHT + BODY_PAD_Y;
        for (i, line) in self.lines.iter().enumerate() {
            let line_h = line.line_height();
            let alpha = if self.staggered {
                stagger_spec.item_progress(overall, i, n, reduce_motion) as f32
            } else {
                1.0
            };
            if alpha > 0.0 {
                scene.push_layer(
                    Point::new(origin.x, y),
                    Size::new(size.width, line_h),
                    alpha,
                );
                let mut x = origin.x + BODY_PAD_X;
                if let Some(marker) = &line.marker {
                    marker.paint(Point::new(x, y), scene);
                    x += line.marker_size.width;
                }
                line.content.paint(Point::new(x, y), scene);
                scene.pop_layer();
            }
            y += line_h;
        }

        if animating {
            ctx.request_frame();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Plain text content — see the module docs.
        let text = self
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        ctx.push_node(Role::Label, |node| {
            node.set_label(text.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BuildCtx, FrameTime, LayoutCtx};
    use frust_text::TextContext;
    use std::any::Any;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
        layers: Vec<(Point, Size, f32)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rrects.push((o, s, 0.0, c));
        }
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
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }
        fn pop_layer(&mut self) {}
    }

    fn build(lines: Vec<TermLine>, staggered: bool) -> TermBlockWidget {
        let view: TermBlockView = term_block(lines).staggered(staggered);
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint(widget: &mut TermBlockWidget, theme: Option<&Theme>) -> Recorder {
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

    fn demo_lines() -> Vec<TermLine> {
        vec![
            TermLine::prompt("zellij attach dev-main"),
            TermLine::output("attached to session \"dev-main\" \u{b7} 3 panes"),
            TermLine::comment("# build finished in 8.2s"),
        ]
    }

    #[test]
    fn unthemed_line_colors_match_glyph_ink_default() {
        let mut w = build(demo_lines(), false);
        let rec = layout_and_paint(&mut w, None);
        let ink = GlyphInk::default_ink();
        // Prompt line contributes marker ("$ ") + content runs.
        assert_eq!(rec.glyph_colors[0], ink.terminal_prompt); // "$ " marker
        assert_eq!(rec.glyph_colors[1], ink.terminal_fg); // command text
        assert_eq!(rec.glyph_colors[2], ink.terminal_out); // output line
        assert_eq!(rec.glyph_colors[3], ink.terminal_comment); // comment line
    }

    #[test]
    fn unthemed_dot_and_bg_colors_use_fallback_constants() {
        let mut w = build(demo_lines(), false);
        let rec = layout_and_paint(&mut w, None);
        let ink = GlyphInk::default_ink();
        assert_eq!(rec.rrects[0].3, ink.terminal_bg); // background
        assert_eq!(rec.strokes[0], TERM_BORDER);
        // rrects[1] is the header strip (`fill_rect`, radius 0.0); the three
        // dots follow at [2..=4].
        assert_eq!(rec.rrects[1].3, ink.terminal_head);
        assert_eq!(rec.rrects[2].3, TERM_DOT_ERROR);
        assert_eq!(rec.rrects[3].3, TERM_DOT_WARNING);
        assert_eq!(rec.rrects[4].3, TERM_DOT_SUCCESS);
    }

    #[test]
    fn brightness_invariance_recording_scene_identical_dark_vs_light() {
        // Identical paint under Light and Dark
        // glyph_baseline() brightness.
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);

        let mut w_dark = build(demo_lines(), false);
        let rec_dark = layout_and_paint(&mut w_dark, Some(&dark));
        let mut w_light = build(demo_lines(), false);
        let rec_light = layout_and_paint(&mut w_light, Some(&light));

        assert_eq!(rec_dark.rrects, rec_light.rrects);
        assert_eq!(rec_dark.strokes, rec_light.strokes);
        assert_eq!(rec_dark.glyph_colors, rec_light.glyph_colors);
    }

    #[test]
    fn themed_ink_resolves_from_glyph_baseline_extension() {
        let theme = Theme::glyph_baseline();
        let mut w = build(demo_lines(), false);
        let rec = layout_and_paint(&mut w, Some(&theme));
        let ink = theme.extension::<GlyphInk>().unwrap();
        assert_eq!(rec.rrects[0].3, ink.terminal_bg);
        assert_eq!(rec.glyph_colors[0], ink.terminal_prompt);
    }

    #[test]
    fn non_staggered_lines_render_at_full_opacity_immediately() {
        let mut w = build(demo_lines(), false);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.layers.len(), 3, "one push_layer per line");
        for (_, _, alpha) in &rec.layers {
            assert_eq!(*alpha, 1.0);
        }
    }

    #[test]
    fn staggered_reveal_matches_glyph_stagger_math() {
        let lines = demo_lines();
        let n = lines.len();
        let mut w = build(lines, true);
        // Layout first so line sizes are populated (paint reads them for
        // line-height stacking); the stagger controller itself is seeded on
        // the first `advance` inside `paint`.
        {
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        }
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 200.0));
        let mut rec = Recorder::default();
        w.paint(&mut ctx, &mut rec);
        // First paint seeds the controller's clock (elapsed 0): every item's
        // progress is 0 at the very start.
        let expected = GlyphStagger::glyph().item_progress(0.0, 0, n, false);
        assert_eq!(rec.layers.len(), 0, "alpha 0 items are skipped, not pushed");
        assert!(expected.abs() < 1e-9);
        assert!(ctx.needs_frame());
    }

    #[test]
    fn staggered_reveal_advances_and_settles_fully_visible() {
        let lines = demo_lines();
        let n = lines.len();
        let mut w = build(lines, true);
        {
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        }
        // Seed the clock.
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 200.0));
        w.paint(&mut ctx, &mut Recorder::default());

        // Advance far past the whole stagger timeline (`GlyphStagger`'s
        // `total_duration`) via a second `RenderRoot`-free paint at a later
        // synthetic frame time — done by re-invoking the widget's own
        // controller directly through another `paint` call using a
        // manufactured `PaintCtx` at a later `frame_time`.
        let total_ms = GlyphStagger::glyph().total_duration(n);
        let mut ctx2 = PaintCtx::new(Point::ZERO, Size::new(400.0, 200.0));
        // `PaintCtx::new` seeds `frame_time` at `FrameTime::default()`
        // (effectively zero); advance the controller directly to confirm the
        // math instead, since `PaintCtx` in this harness has no way to inject
        // an arbitrary later `frame_time` without a full `RenderRoot`.
        let ctrl = w
            .stagger
            .as_mut()
            .expect("staggered widget carries a controller");
        ctrl.advance(ft_ms(total_ms + 50.0));
        let overall = ctrl.value_clamped();
        assert_eq!(overall, 1.0, "settled at the end of the timeline");
        let mut rec = Recorder::default();
        w.paint(&mut ctx2, &mut rec);
        assert_eq!(rec.layers.len(), n, "every line fully revealed");
        for (_, _, alpha) in &rec.layers {
            assert!((*alpha - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn reduce_motion_synchronizes_lines_to_a_single_fade_no_cascade() {
        // `GlyphStagger::item_progress`'s documented contract: under
        // `reduce_motion` every item tracks the shared `overall` progress
        // directly (no per-item delay) instead of cascading — a single fast
        // fade, not an instantaneous full reveal.
        let mut theme = Theme::glyph_baseline();
        theme.motion.reduce_motion = true;
        let lines = demo_lines();
        let n = lines.len();
        let mut w = build(lines, true);
        {
            let mut tcx = TextContext::new();
            let mut lctx =
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
            w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        }
        // Advance the controller to the timeline midpoint directly, then
        // paint at that same seeded frame time (a second `advance` at an
        // identical timestamp is a zero-delta no-op, so the value holds).
        let total_ms = GlyphStagger::glyph().total_duration(n);
        {
            let ctrl = w
                .stagger
                .as_mut()
                .expect("staggered widget carries a controller");
            ctrl.advance(ft_ms(0.0));
            ctrl.advance(ft_ms(total_ms / 2.0));
        }
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 200.0)).with_theme(&theme);
        let mut rec = Recorder::default();
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.layers.len(), n);
        let alphas: Vec<f32> = rec.layers.iter().map(|(_, _, a)| *a).collect();
        // Every line shares the same alpha (synchronized), and it's strictly
        // between 0 and 1 (mid-fade, not yet settled).
        for a in &alphas {
            assert!((*a - alphas[0]).abs() < 1e-6, "lines must be synchronized");
        }
        assert!(alphas[0] > 0.0 && alphas[0] < 1.0, "mid-fade, not settled");
    }

    #[test]
    fn semantics_reports_joined_line_text() {
        fn logic(_s: &mut ()) -> TermBlockView {
            term_block(demo_lines())
        }
        let mut root: frust_core::RenderRoot<(), TermBlockView> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 400.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Label)
            .expect("term block contributes a Role::Label node");
        assert_eq!(
            node.label(),
            Some(
                "zellij attach dev-main\nattached to session \"dev-main\" \u{b7} 3 panes\n# build finished in 8.2s"
            )
        );
    }
}
