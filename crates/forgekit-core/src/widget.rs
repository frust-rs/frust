//! Layer 2: the retained [`Widget`] trait and its layout/paint contexts
//! (spec §6).
//!
//! Widgets are the long-lived counterpart to [`crate::view::View`]s. A view is
//! rebuilt every frame; the widget it produced persists in the arena and is
//! mutated in place. Widgets participate in two passes:
//!
//! * **layout** — receive [`BoxConstraints`] and return a chosen [`Size`].
//! * **paint** — emit draw commands into a scene.

use std::any::Any;

use forgekit_scene::{GlyphRun, SceneBuilder};
use kurbo::{Point, Rect, Size};
use peniko::{Brush, Color};

use crate::layout::BoxConstraints;

/// The renderer-agnostic paint target a widget draws into.
///
/// This trait was introduced (task 02) as a **local stand-in** for
/// `forgekit_scene::SceneBuilder` while the scene crate was still a stub. Task
/// 08 reconciles the two *additively*: rather than churn the `Widget::paint`
/// signature (and every widget/test written against it), `SceneBuilder` now
/// [implements this trait](#impl-PaintScene-for-SceneBuilder), so widgets keep
/// painting through `&mut dyn PaintScene` while the shell hands them a real
/// `SceneBuilder` whose commands reach the GPU backend.
///
/// The original `fill_rect`/`draw_text` shape is retained for source
/// compatibility with existing recorder-style test scenes; real text rendering
/// goes through [`PaintScene::draw_glyph_run`], which carries shaped glyphs from
/// `forgekit-text`.
pub trait PaintScene {
    /// Emit a filled axis-aligned rectangle at `origin` with `size`.
    fn fill_rect(&mut self, origin: Point, size: Size);

    /// Emit a run of *unshaped* text anchored at `origin`.
    ///
    /// This records intent only — glyph shaping lives in `forgekit-text`
    /// (task 07). Real rendering uses [`PaintScene::draw_glyph_run`]; the
    /// `SceneBuilder` implementation treats this as a no-op.
    fn draw_text(&mut self, origin: Point, text: &str);

    /// Emit a run of already-shaped glyphs into the scene.
    ///
    /// Defaulted to a no-op so pre-existing recorder scenes (which predate the
    /// text pipeline) stay valid without modification; the `SceneBuilder`
    /// implementation overrides it to record a real glyph-run command.
    fn draw_glyph_run(&mut self, _run: GlyphRun) {}
}

/// Bridges the provisional [`PaintScene`] boundary onto the real
/// `forgekit_scene::SceneBuilder` (task 08 reconciliation).
///
/// Widgets paint through `&mut dyn PaintScene`; the desktop shell (task 08)
/// hands them a `SceneBuilder`, so filled rectangles and shaped glyph runs land
/// in the display list under the builder's current transform. Unshaped
/// [`PaintScene::draw_text`] is intentionally dropped here — text must be shaped
/// (by `forgekit-text`) into glyph runs before it can be drawn.
impl PaintScene for SceneBuilder<'_> {
    fn fill_rect(&mut self, origin: Point, size: Size) {
        let rect = Rect::new(
            origin.x,
            origin.y,
            origin.x + size.width,
            origin.y + size.height,
        );
        SceneBuilder::fill_rect(self, rect, Brush::Solid(Color::BLACK));
    }

    fn draw_text(&mut self, _origin: Point, _text: &str) {
        // Unshaped text is not renderable; real text arrives as glyph runs.
    }

    fn draw_glyph_run(&mut self, run: GlyphRun) {
        SceneBuilder::draw_glyph_run(self, run);
    }
}

/// Context passed to [`Widget::layout`].
///
/// Beyond the (still-empty) container seam, it optionally carries the shared,
/// heavyweight text-shaping context the render root threads down for text
/// layout (spec §10.3). The resource is **type-erased** (`&mut dyn Any`) so
/// `forgekit-core` stays independent of `forgekit-text` (and thus of parley);
/// text widgets recover it with [`LayoutCtx::text_context`].
pub struct LayoutCtx<'a> {
    text_ctx: Option<&'a mut dyn Any>,
}

impl<'a> LayoutCtx<'a> {
    /// Create a layout context with no shared resources.
    ///
    /// Used by leaf-only unit tests and by containers that never lay out text.
    pub fn new() -> LayoutCtx<'static> {
        LayoutCtx { text_ctx: None }
    }

    /// Create a layout context carrying the shared text-shaping context.
    ///
    /// The render root builds this so text widgets can shape their content
    /// during the layout pass; the concrete type is erased to keep this crate
    /// free of a `forgekit-text` dependency.
    pub fn with_text_context(text_ctx: &'a mut dyn Any) -> Self {
        LayoutCtx {
            text_ctx: Some(text_ctx),
        }
    }

    /// Recover the shared text-shaping context as `&mut T`.
    ///
    /// Panics if no context was threaded into this pass, or if its concrete
    /// type differs from `T` — both are shell-wiring bugs, not runtime-data
    /// conditions.
    pub fn text_context<T: Any>(&mut self) -> &mut T {
        self.text_ctx
            .as_deref_mut()
            .expect("no text context threaded into this layout pass")
            .downcast_mut::<T>()
            .expect("threaded layout resource is not the expected text-context type")
    }
}

impl Default for LayoutCtx<'static> {
    fn default() -> Self {
        Self::new()
    }
}

/// Context passed to [`Widget::paint`].
///
/// Carries the widget's resolved geometry (as stored in its pod after layout) so
/// paint code can position itself in the parent coordinate space.
pub struct PaintCtx {
    origin: Point,
    size: Size,
}

impl PaintCtx {
    /// Create a paint context for a widget at `origin` with `size`.
    pub fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// The widget's origin in its parent's coordinate space.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The widget's resolved size.
    pub fn size(&self) -> Size {
        self.size
    }
}

/// A retained UI element living in the widget tree.
///
/// `Widget: Any` so the render root can downcast a boxed widget back to the
/// concrete element type its originating view produced (needed during rebuild).
/// Implementors get [`Widget::downcast_mut`] for free — no boilerplate method to
/// write.
pub trait Widget: Any {
    /// Choose a size within `bc` and return it. The chosen size must satisfy
    /// `bc` (callers may additionally clamp via [`BoxConstraints::constrain`]).
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size;

    /// Emit draw commands for this widget into `scene`.
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene);
}

impl dyn Widget {
    /// Attempt to downcast this trait object to a concrete widget type.
    ///
    /// Uses trait upcasting (`Widget: Any`, stable since Rust 1.86; the
    /// workspace MSRV is 1.88) — no `unsafe`.
    pub fn downcast_mut<W: Widget>(&mut self) -> Option<&mut W> {
        let any: &mut dyn Any = self;
        any.downcast_mut::<W>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A leaf widget that paints a filled box of a fixed intrinsic size.
    struct FixedBox {
        intrinsic: Size,
    }

    impl Widget for FixedBox {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.intrinsic)
        }

        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size());
        }
    }

    /// A scene recorder used to assert paint output without any GPU dependency.
    #[derive(Default)]
    struct RecordingScene {
        rects: Vec<(Point, Size)>,
        texts: Vec<(Point, String)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, origin: Point, text: &str) {
            self.texts.push((origin, text.to_string()));
        }
    }

    #[test]
    fn widget_layout_respects_constraints() {
        let mut w = FixedBox {
            intrinsic: Size::new(1000.0, 1000.0),
        };
        let mut ctx = LayoutCtx::new();
        let bc = BoxConstraints::loose(Size::new(200.0, 100.0));
        assert_eq!(w.layout(&mut ctx, &bc), Size::new(200.0, 100.0));
    }

    #[test]
    fn widget_paint_emits_into_scene() {
        let mut w = FixedBox {
            intrinsic: Size::new(50.0, 20.0),
        };
        let mut scene = RecordingScene::default();
        let mut ctx = PaintCtx::new(Point::new(5.0, 7.0), Size::new(50.0, 20.0));
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.rects, vec![(Point::new(5.0, 7.0), Size::new(50.0, 20.0))]);
    }

    #[test]
    fn downcast_recovers_concrete_widget() {
        let mut boxed: Box<dyn Widget> = Box::new(FixedBox {
            intrinsic: Size::new(3.0, 4.0),
        });
        let concrete = boxed.downcast_mut::<FixedBox>().expect("downcast");
        assert_eq!(concrete.intrinsic, Size::new(3.0, 4.0));
    }
}
