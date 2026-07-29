//! [`glyph_list`]/[`GlyphListView`] + [`GlyphListItem`]: a bordered, rounded
//! list container whose rows are **data** (not child views) — each a leading
//! glyph box (a 32px raised square holding an accent glyph char) + a title/sub
//! text column + an optional right-aligned meta + an optional chevron (the
//! Glyph design system's `.list`/`.list-item`).
//!
//! Because the rows are the widget's own data, it shapes/paints every text run
//! directly (like [`super::badge`]) and hit-tests presses itself: a press
//! captures the row under the pointer and fires `on_press(index)` on
//! release **inside that same row** (fire-on-up-inside), with a `Cancel`
//! clearing the pressed wash — the controlled, never-self-mutating contract
//! [`crate::material::navbar`] uses.
//!
//! # Token resolution
//!
//! - **container** = `surface_container` fill + `outline` border,
//!   `shape.medium` corner; rows divided by an `outline_variant` hairline.
//! - **glyph box** = `surface_container_high` (Glyph `bg-raised`) fill; the
//!   glyph char is `primary` (the accent).
//! - **title** = `on_surface`; **sub/meta/chevron** = `on_surface_variant`.
//! - **pressed wash** = `on_surface` at [`PRESS_WASH_ALPHA`].
//!
//! Unthemed, each falls back to the literal Glyph **dark** constant.

use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust_text::{FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle};
use frust_theme::{ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

/// Corner-rounding tolerance for the container border stroke.
const PATH_TOLERANCE: f64 = 0.1;
/// Horizontal padding inside a row, in logical px.
const ROW_PAD_X: f64 = 14.0;
/// Vertical padding inside a row, in logical px.
const ROW_PAD_Y: f64 = 12.0;
/// The leading glyph box edge, in logical px.
const GLYPH_BOX: f64 = 32.0;
/// Gap between the glyph box and the text column, in logical px.
const GLYPH_GAP: f64 = 12.0;
/// Gap between the title and sub rows, in logical px.
const TITLE_SUB_GAP: f64 = 2.0;
/// Gap between the text column / meta and the chevron, in logical px.
const CHEVRON_GAP: f64 = 12.0;
/// The chevron's arm length, in logical px.
const CHEVRON_ARM: f64 = 4.5;
/// The chevron stroke width, in logical px.
const CHEVRON_WIDTH: f64 = 1.5;
/// Container border width, in logical px.
const LIST_BORDER_WIDTH: f64 = 1.0;
/// Row divider (hairline) width, in logical px.
const DIVIDER_WIDTH: f64 = 1.0;
/// The pressed-row wash alpha over `on_surface`.
const PRESS_WASH_ALPHA: f32 = 0.06;

/// Title font size, in logical px.
const TITLE_SIZE: f32 = 13.0;
/// Sub-title font size, in logical px.
const SUB_SIZE: f32 = 11.5;
/// Meta font size, in logical px.
const META_SIZE: f32 = 11.5;
/// The glyph-box glyph font size, in logical px.
const GLYPH_SIZE: f32 = 14.0;

/// Unthemed corner-radius fallback — Glyph `--radius-md` (10px).
const LIST_RADIUS_FALLBACK: f64 = 10.0;
/// Glyph-box corner radius fallback — `--radius-sm` (6px).
const GLYPH_BOX_RADIUS_FALLBACK: f64 = 6.0;

// ---- Unthemed fallback constants (Glyph **dark** values) ---------------

const LIST_BG: Color = Color::from_rgb8(0x16, 0x1a, 0x23); // bg-surface
const LIST_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // outline
const LIST_DIVIDER: Color = Color::from_rgb8(0x2a, 0x2d, 0x33); // outline-variant
const GLYPH_BOX_BG: Color = Color::from_rgb8(0x1e, 0x23, 0x30); // bg-raised
const GLYPH_FG: Color = Color::from_rgb8(0xff, 0xb6, 0x27); // accent
const TITLE_FG: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // fg
const MUTED_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted
const PRESS_WASH_BASE: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // on_surface

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// One row of a [`GlyphListView`]: a glyph char, a title, and optional sub /
/// meta text + a trailing chevron. Pure data — see the [module docs](self).
#[derive(Clone)]
pub struct GlyphListItem {
    glyph: String,
    title: String,
    sub: Option<String>,
    meta: Option<String>,
    chevron: bool,
}

/// Create a list row with a leading `glyph` char and a `title`.
pub fn glyph_list_item(glyph: impl Into<String>, title: impl Into<String>) -> GlyphListItem {
    GlyphListItem {
        glyph: glyph.into(),
        title: title.into(),
        sub: None,
        meta: None,
        chevron: false,
    }
}

impl GlyphListItem {
    /// Add a sub-title line under the title.
    #[allow(clippy::should_implement_trait)] // `.sub(..)` is a builder, not arithmetic
    pub fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    /// Add right-aligned meta text.
    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// Show (or hide) the trailing chevron.
    pub fn chevron(mut self, chevron: bool) -> Self {
        self.chevron = chevron;
        self
    }
}

/// A view-held, typed row-press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative Glyph list. See the [module docs](self).
pub struct GlyphListView<State: 'static> {
    items: Vec<GlyphListItem>,
    on_press: Option<OnPress<State>>,
}

/// Create a Glyph list from `items`. Chain [`GlyphListView::on_press`] to make
/// rows interactive.
pub fn glyph_list<State: 'static>(items: Vec<GlyphListItem>) -> GlyphListView<State> {
    GlyphListView {
        items,
        on_press: None,
    }
}

impl<State: 'static> GlyphListView<State> {
    /// Fire `on_press(state, index)` on a release inside the row at `index`
    /// (fire-on-up-inside). The list never mutates itself; the app owns any
    /// resulting state change.
    pub fn on_press<F: Fn(&mut State, usize) + 'static>(mut self, on_press: F) -> Self {
        self.on_press = Some(Rc::new(on_press));
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

/// A retained row: its text runs plus the laid-out `(y, height)` band used for
/// hit-testing and painting.
struct Row {
    glyph: GlyphLabel,
    glyph_size: Size,
    title: GlyphLabel,
    title_text: String,
    title_size: Size,
    sub: Option<GlyphLabel>,
    sub_size: Size,
    meta: Option<GlyphLabel>,
    meta_size: Size,
    chevron: bool,
    y: f64,
    height: f64,
}

impl Row {
    fn from_item(item: &GlyphListItem) -> Self {
        Row {
            glyph: GlyphLabel::new(item.glyph.clone()),
            glyph_size: Size::ZERO,
            title: GlyphLabel::new(item.title.clone()),
            title_text: item.title.clone(),
            title_size: Size::ZERO,
            sub: item.sub.as_ref().map(GlyphLabel::new),
            sub_size: Size::ZERO,
            meta: item.meta.as_ref().map(GlyphLabel::new),
            meta_size: Size::ZERO,
            chevron: item.chevron,
            y: 0.0,
            height: 0.0,
        }
    }
}

/// The retained widget for a [`GlyphListView`].
pub struct GlyphListWidget {
    rows: Vec<Row>,
    on_press: Option<crate::ErasedArgCallback<usize>>,
    /// The row currently pressed (armed by a `Down`), if any.
    pressed_row: Option<usize>,
    /// The row that captured the active gesture, if any.
    captured_row: Option<usize>,
}

impl<State: 'static> View<State> for GlyphListView<State> {
    type Element = GlyphListWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GlyphListWidget {
        GlyphListWidget {
            rows: self.items.iter().map(Row::from_item).collect(),
            on_press: self.on_press.as_ref().map(crate::erase_callback_arg),
            pressed_row: None,
            captured_row: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphListWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // Rows are reconciled positionally (v1: no keyed-reconciliation
        // support here); any structural or content change rebuilds the row
        // set wholesale and drops a mid-gesture capture.
        let changed = prev.items.len() != self.items.len()
            || prev
                .items
                .iter()
                .zip(&self.items)
                .any(|(a, b)| !items_equal(a, b));
        if changed {
            element.rows = self.items.iter().map(Row::from_item).collect();
            element.pressed_row = None;
            element.captured_row = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Always refresh the callback (a cheap closure swap, never a structural
        // change) so the newest app state is captured.
        element.on_press = self.on_press.as_ref().map(crate::erase_callback_arg);
        flags
    }
}

fn items_equal(a: &GlyphListItem, b: &GlyphListItem) -> bool {
    a.glyph == b.glyph
        && a.title == b.title
        && a.sub == b.sub
        && a.meta == b.meta
        && a.chevron == b.chevron
}

fn title_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TITLE_SIZE, color)
    }
}

fn sub_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(SUB_SIZE, color)
    }
}

fn meta_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(META_SIZE, color)
    }
}

fn glyph_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace),
        weight: FontWeight::BOLD,
        ..TextStyle::new(GLYPH_SIZE, color)
    }
}

/// Resolve `(bg, border, divider, box_bg, glyph, title, muted)`.
#[allow(clippy::type_complexity)]
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.surface_container,
                s.outline,
                s.outline_variant,
                s.surface_container_high,
                s.primary,
                s.on_surface,
                s.on_surface_variant,
            )
        }
        None => (
            LIST_BG,
            LIST_BORDER,
            LIST_DIVIDER,
            GLYPH_BOX_BG,
            GLYPH_FG,
            TITLE_FG,
            MUTED_FG,
        ),
    }
}

fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.medium, size.width, size.height),
        None => LIST_RADIUS_FALLBACK,
    }
}

fn resolve_box_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.small, GLYPH_BOX, GLYPH_BOX),
        None => GLYPH_BOX_RADIUS_FALLBACK,
    }
}

impl GlyphListWidget {
    /// The row index containing local `y`, if any.
    fn row_at(&self, y: f64) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| y >= r.y && y < r.y + r.height)
    }
}

impl Widget for GlyphListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, _, _, _, glyph_c, title_c, muted_c) = resolve_colors(theme);

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            280.0 // a sensible intrinsic width when unconstrained
        };
        let text_col_x = ROW_PAD_X + GLYPH_BOX + GLYPH_GAP;

        let mut y = 0.0f64;
        for row in self.rows.iter_mut() {
            row.glyph_size = row.glyph.layout(ctx, &glyph_style(glyph_c), None);
            let meta_w = if let Some(meta) = row.meta.as_mut() {
                row.meta_size = meta.layout(ctx, &meta_style(muted_c), None);
                row.meta_size.width + CHEVRON_GAP
            } else {
                0.0
            };
            let chevron_w = if row.chevron {
                CHEVRON_ARM + CHEVRON_GAP
            } else {
                0.0
            };
            let text_col_max =
                ((width - text_col_x - ROW_PAD_X - meta_w - chevron_w).max(20.0)) as f32;
            row.title_size = row
                .title
                .layout(ctx, &title_style(title_c), Some(text_col_max));
            let text_col_h = if let Some(sub) = row.sub.as_mut() {
                row.sub_size = sub.layout(ctx, &sub_style(muted_c), Some(text_col_max));
                row.title_size.height + TITLE_SUB_GAP + row.sub_size.height
            } else {
                row.sub_size = Size::ZERO;
                row.title_size.height
            };
            let content_h = text_col_h.max(GLYPH_BOX);
            let height = content_h + ROW_PAD_Y * 2.0;
            row.y = y;
            row.height = height;
            y += height;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, border, divider, box_bg, _glyph_c, _title_c, muted_c) = resolve_colors(theme);
        let press_base = theme
            .map(|t| t.scheme().on_surface)
            .unwrap_or(PRESS_WASH_BASE);
        let radius = resolve_radius(theme, ctx.size());
        let box_radius = resolve_box_radius(theme);
        let o = ctx.origin();
        let size = ctx.size();

        scene.fill_rounded_rect(o, size, radius, bg);

        let text_col_x = ROW_PAD_X + GLYPH_BOX + GLYPH_GAP;
        for (i, row) in self.rows.iter().enumerate() {
            let row_origin = o + Vec2::new(0.0, row.y);

            // Pressed wash.
            if self.pressed_row == Some(i) {
                scene.fill_rect(
                    row_origin,
                    Size::new(size.width, row.height),
                    with_alpha(press_base, PRESS_WASH_ALPHA),
                );
            }

            // Divider above every row but the first.
            if i > 0 {
                scene.stroke_line(
                    Point::new(o.x, row_origin.y),
                    Point::new(o.x + size.width, row_origin.y),
                    DIVIDER_WIDTH,
                    divider,
                );
            }

            // Glyph box + centered glyph char.
            let box_y = row.y + (row.height - GLYPH_BOX) / 2.0;
            scene.fill_rounded_rect(
                o + Vec2::new(ROW_PAD_X, box_y),
                Size::new(GLYPH_BOX, GLYPH_BOX),
                box_radius,
                box_bg,
            );
            let gx = ROW_PAD_X + (GLYPH_BOX - row.glyph_size.width) / 2.0;
            let gy = box_y + (GLYPH_BOX - row.glyph_size.height) / 2.0;
            row.glyph.paint(o + Vec2::new(gx, gy), scene);

            // Text column (title over optional sub), vertically centered.
            let text_col_h = if row.sub.is_some() {
                row.title_size.height + TITLE_SUB_GAP + row.sub_size.height
            } else {
                row.title_size.height
            };
            let text_top = row.y + (row.height - text_col_h) / 2.0;
            row.title.paint(o + Vec2::new(text_col_x, text_top), scene);
            if let Some(sub) = &row.sub {
                sub.paint(
                    o + Vec2::new(text_col_x, text_top + row.title_size.height + TITLE_SUB_GAP),
                    scene,
                );
            }

            // Chevron (far right), then meta to its left.
            let mut right_x = size.width - ROW_PAD_X;
            if row.chevron {
                let cx = o.x + right_x - CHEVRON_ARM;
                let cy = row_origin.y + row.height / 2.0;
                draw_chevron(scene, Point::new(cx, cy), muted_c);
                right_x -= CHEVRON_ARM + CHEVRON_GAP;
            }
            if let Some(meta) = &row.meta {
                let mx = right_x - row.meta_size.width;
                let my = row.y + (row.height - row.meta_size.height) / 2.0;
                meta.paint(o + Vec2::new(mx, my), scene);
            }
        }

        // Container border on top of the row content.
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(o, &path, LIST_BORDER_WIDTH, &Brush::Solid(border));
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        if self.on_press.is_none() {
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down => {
                if let Some(idx) = self.row_at(p.position.y) {
                    self.pressed_row = Some(idx);
                    self.captured_row = Some(idx);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            PointerPhase::Move => {
                let Some(captured) = self.captured_row else {
                    return EventResult::Ignored;
                };
                // The wash follows the pointer in/out of the captured row.
                let inside_row = self.row_at(p.position.y) == Some(captured);
                let new_pressed = if inside_row { Some(captured) } else { None };
                if new_pressed != self.pressed_row {
                    self.pressed_row = new_pressed;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(captured) = self.captured_row.take() else {
                    return EventResult::Ignored;
                };
                let fired = self.row_at(p.position.y) == Some(captured);
                self.pressed_row = None;
                if fired && let Some(cb) = self.on_press.as_mut() {
                    cb(ctx, captured);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured_row.take().is_some() {
                    self.pressed_row = None;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::List,
            |_| {},
            |ctx| {
                for row in &self.rows {
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(row.title_text.as_str());
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }
}

/// Draw a right-pointing chevron (`>`) centered at `center` in `color`.
fn draw_chevron(scene: &mut dyn PaintScene, center: Point, color: Color) {
    let top = Point::new(center.x - CHEVRON_ARM, center.y - CHEVRON_ARM);
    let tip = Point::new(center.x + CHEVRON_ARM, center.y);
    let bottom = Point::new(center.x - CHEVRON_ARM, center.y + CHEVRON_ARM);
    scene.stroke_line(top, tip, CHEVRON_WIDTH, color);
    scene.stroke_line(tip, bottom, CHEVRON_WIDTH, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        lines: usize,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _w: f64, _c: Color) {
            self.lines += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, _b: &Brush) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn sample_items() -> Vec<GlyphListItem> {
        vec![
            glyph_list_item("$", "deploy")
                .sub("staged")
                .meta("2m")
                .chevron(true),
            glyph_list_item("#", "rollback").sub("ready").chevron(true),
        ]
    }

    fn build<S: 'static>(view: &GlyphListView<S>) -> GlyphListWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout<S: 'static>(w: &mut GlyphListWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let _ = std::marker::PhantomData::<S>;
        w.layout(
            &mut lctx,
            &BoxConstraints::tight(Size::new(320.0, f64::INFINITY)),
        )
    }

    fn paint(w: &mut GlyphListWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_container_and_glyph_use_fallback_constants() {
        let view: GlyphListView<()> = glyph_list(sample_items());
        let mut w = build(&view);
        let size = layout::<()>(&mut w, None);
        let rec = paint(&mut w, size, None);
        // First rounded rect is the container fill.
        assert_eq!(rec.rrects[0].3, LIST_BG);
        // A glyph-box fill (raised bg) appears among the rounded rects.
        assert!(rec.rrects.iter().any(|(_, _, _, c)| *c == GLYPH_BOX_BG));
        // Accent glyph chars are drawn.
        assert!(rec.glyph_colors.contains(&GLYPH_FG));
    }

    #[test]
    fn glyph_dark_and_light_container_differ() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);
        let view: GlyphListView<()> = glyph_list(sample_items());
        let mut wd = build(&view);
        let sd = layout::<()>(&mut wd, Some(&dark));
        let rec_d = paint(&mut wd, sd, Some(&dark));
        let mut wl = build(&view);
        let sl = layout::<()>(&mut wl, Some(&light));
        let rec_l = paint(&mut wl, sl, Some(&light));
        assert_eq!(rec_d.rrects[0].3, dark.scheme().surface_container);
        assert_eq!(rec_l.rrects[0].3, light.scheme().surface_container);
        assert_ne!(rec_d.rrects[0].3, rec_l.rrects[0].3);
    }

    fn ev(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(20.0, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    // The concrete state type must stay `Vec<usize>` for the erased callback's
    // `Any` downcast — a slice would not round-trip.
    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut GlyphListWidget, state: &mut Vec<usize>, e: &InputEvent) -> EventResult {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(320.0, 500.0));
        w.event(&mut ctx, e)
    }

    #[test]
    fn press_fires_on_up_inside_with_correct_index() {
        let view: GlyphListView<Vec<usize>> =
            glyph_list(sample_items()).on_press(|s: &mut Vec<usize>, i| s.push(i));
        let mut w = build(&view);
        layout::<Vec<usize>>(&mut w, None);
        let row1_mid = w.rows[1].y + w.rows[1].height / 2.0;

        let mut fired = Vec::new();
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Down, row1_mid));
        assert_eq!(w.pressed_row, Some(1));
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Up, row1_mid));
        assert_eq!(fired, vec![1]);
        assert_eq!(w.pressed_row, None);
        assert_eq!(w.captured_row, None);
    }

    #[test]
    fn up_outside_the_pressed_row_does_not_fire() {
        let view: GlyphListView<Vec<usize>> =
            glyph_list(sample_items()).on_press(|s: &mut Vec<usize>, i| s.push(i));
        let mut w = build(&view);
        layout::<Vec<usize>>(&mut w, None);
        let row0_mid = w.rows[0].y + w.rows[0].height / 2.0;
        let row1_mid = w.rows[1].y + w.rows[1].height / 2.0;

        let mut fired = Vec::new();
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Down, row0_mid));
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Move, row1_mid));
        assert_eq!(w.pressed_row, None, "wash clears when leaving the row");
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Up, row1_mid));
        assert!(fired.is_empty());
    }

    #[test]
    fn cancel_clears_pressed_and_captured() {
        let view: GlyphListView<Vec<usize>> =
            glyph_list(sample_items()).on_press(|s: &mut Vec<usize>, i| s.push(i));
        let mut w = build(&view);
        layout::<Vec<usize>>(&mut w, None);
        let row0_mid = w.rows[0].y + w.rows[0].height / 2.0;

        let mut fired = Vec::new();
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Down, row0_mid));
        assert_eq!(w.captured_row, Some(0));
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Cancel, row0_mid));
        assert_eq!(w.pressed_row, None);
        assert_eq!(w.captured_row, None);
        dispatch(&mut w, &mut fired, &ev(PointerPhase::Up, row0_mid));
        assert!(fired.is_empty());
    }

    #[test]
    fn non_interactive_list_ignores_presses() {
        let view: GlyphListView<Vec<usize>> = glyph_list(sample_items());
        let mut w = build(&view);
        layout::<Vec<usize>>(&mut w, None);
        let mut fired = Vec::new();
        let y = w.rows[0].y + 4.0;
        let r = dispatch(&mut w, &mut fired, &ev(PointerPhase::Down, y));
        assert_eq!(r, EventResult::Ignored);
        assert_eq!(w.pressed_row, None);
    }

    #[test]
    fn semantics_is_a_list_with_a_node_per_row() {
        fn logic(_s: &mut ()) -> GlyphListView<()> {
            glyph_list(vec![
                glyph_list_item("$", "deploy"),
                glyph_list_item("#", "rollback"),
            ])
        }
        let mut root: frust_core::RenderRoot<(), GlyphListView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(320.0, 400.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("list contributes a Role::List node");
        assert_eq!(list.children().len(), 2);
        let labels: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::ListItem)
            .filter_map(|(_, n)| n.label())
            .collect();
        assert!(labels.contains(&"deploy"));
        assert!(labels.contains(&"rollback"));
    }
}
