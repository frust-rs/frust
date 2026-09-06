//! GPU-free test fixtures shared by this crate's widget tests: a recording
//! [`PaintScene`] plus the layout/paint/event drivers built on it.
//!
//! This module is the sample's answer to "how does an out-of-tree design
//! system test its widgets?" — and the answer is **entirely through public
//! `frust` API**: [`LayoutCtx::with_resources`], [`PaintCtx::new`],
//! [`EventCtx::new`], [`BuildCtx::new`] and the `PaintScene` trait are all
//! reachable from `frust::authoring`, so a recording scene is a dozen lines
//! and needs no window, adapter or font file beyond the system fallback
//! `TextContext` resolves.
//!
//! `frust-widgets` ships richer fixtures behind its own non-default
//! `test-support` feature (a fixed-size leaf, an event probe), but reaching
//! them would mean depending on `frust-widgets` directly — which is exactly
//! the dependency this sample exists to prove unnecessary. Twelve lines of
//! recorder is the cheaper trade.
//!
//! # Known gap: `Widget::semantics` cannot be driven from out of tree
//!
//! There is deliberately no `semantics_of(widget)` helper here. Constructing a
//! [`SemanticsCtx`](frust::authoring::SemanticsCtx) needs `SemanticsCtx::new`,
//! which is `pub(crate)` in `frust-core`, and the only public producer of a
//! semantics pass is `frust_core::RenderRoot::semantics()` — and `RenderRoot`
//! is not re-exported by the `frust` facade. So an external design system can
//! *write* a `semantics` impl (the trait, `push_node`/`push_container`, `Role`,
//! `Node` and `Action` are all public and every widget here has one) but cannot
//! *assert* on its output. See this workspace's README for the finding.

use std::any::Any;

use frust::Theme;
use frust::authoring::text::TextContext;
use frust::authoring::{
    Affine, BezPath, BoxConstraints, Brush, Color, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, Point, PointerButton, PointerEvent, PointerPhase, Rect, Size, Widget,
};

/// Records the draw calls this crate's widgets actually make, so a test can
/// assert on resolved colors/radii without a GPU.
#[derive(Default)]
pub struct RecordingScene {
    /// `(origin, size, color)` per [`PaintScene::fill_rect`].
    pub rects: Vec<(Point, Size, Color)>,
    /// `(origin, size, radius, color)` per [`PaintScene::fill_rounded_rect`].
    pub rounded_rects: Vec<(Point, Size, f64, Color)>,
    /// The solid brush color of each [`PaintScene::stroke_path`].
    pub strokes: Vec<Color>,
    /// The solid brush color of each glyph run, in paint order.
    pub glyph_colors: Vec<Color>,
    /// The alpha of each [`PaintScene::push_layer`].
    pub layer_alphas: Vec<f32>,
    /// Each [`PaintScene::push_transform`].
    pub transforms: Vec<Affine>,
}

impl PaintScene for RecordingScene {
    fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
        self.rects.push((origin, size, color));
    }

    fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
        self.rounded_rects.push((origin, size, radius, color));
    }

    fn draw_text(&mut self, _origin: Point, _text: &str) {}

    fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
        if let Brush::Solid(color) = run.brush {
            self.glyph_colors.push(color);
        }
    }

    fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, brush: &Brush) {
        if let Brush::Solid(color) = brush {
            self.strokes.push(*color);
        }
    }

    fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
        self.layer_alphas.push(alpha);
    }

    fn push_transform(&mut self, transform: Affine) {
        self.transforms.push(transform);
    }
}

/// Lay `widget` out against a loose `max`-sized constraint, threading a real
/// [`TextContext`] (needed by any widget that shapes its own runs) and
/// optionally a [`Theme`].
pub fn layout_widget<W: Widget + ?Sized>(widget: &mut W, theme: Option<&Theme>, max: Size) -> Size {
    let mut text_ctx = TextContext::new();
    let mut ctx = LayoutCtx::with_resources(
        Some(&mut text_ctx as &mut dyn Any),
        theme.map(|t| t as &dyn Any),
    );
    widget.layout(&mut ctx, &BoxConstraints::loose(max))
}

/// Paint `widget` at the origin into a fresh [`RecordingScene`].
pub fn paint_widget<W: Widget + ?Sized>(
    widget: &mut W,
    size: Size,
    theme: Option<&Theme>,
) -> RecordingScene {
    let mut scene = RecordingScene::default();
    let mut ctx = match theme {
        Some(theme) => PaintCtx::new(Point::ZERO, size).with_theme(theme),
        None => PaintCtx::new(Point::ZERO, size),
    };
    widget.paint(&mut ctx, &mut scene);
    scene
}

/// Dispatch `event` to `widget` over app state `state`, the way the framework's
/// own event pass would.
pub fn dispatch<W: Widget + ?Sized, S: Any>(
    widget: &mut W,
    state: &mut S,
    size: Size,
    event: &InputEvent,
) -> EventResult {
    let state: &mut dyn Any = state;
    let mut ctx = EventCtx::new(state, Point::ZERO, size);
    widget.event(&mut ctx, event)
}

/// A primary-button pointer event at `(x, y)`.
pub fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

/// A point comfortably inside a `size`-sized widget.
pub fn center(size: Size) -> Point {
    Rect::from_origin_size(Point::ORIGIN, size).center()
}
