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

use kurbo::{Point, Size};

use crate::layout::BoxConstraints;

/// The renderer-agnostic paint target a widget draws into.
///
/// This is a **local trait boundary** standing in for
/// `forgekit_scene::SceneBuilder`, which task 03 (`03-scene-display-list`) owns.
/// At the time this task was implemented the `forgekit-scene` crate is still a
/// stub with no builder type, so paint code is written against this trait
/// instead of a concrete scene type. When tasks 02 and 03 are merged together,
/// task 08 reconciles the two — most likely by making `forgekit_scene`'s builder
/// implement this trait (or by replacing it outright). Keep the surface minimal
/// so that reconciliation stays cheap.
pub trait PaintScene {
    /// Emit a filled axis-aligned rectangle at `origin` with `size`.
    fn fill_rect(&mut self, origin: Point, size: Size);

    /// Emit a run of text anchored at `origin`.
    ///
    /// Text shaping/glyph layout is out of scope for layer 2 (it lives in
    /// `forgekit-text`, task 07); this records intent only.
    fn draw_text(&mut self, origin: Point, text: &str);
}

/// Context passed to [`Widget::layout`].
///
/// Minimal for v0 (only leaf widgets exist until task 08). It exists as a stable
/// seam so container widgets can later reach child pods without a signature
/// change.
pub struct LayoutCtx {
    _private: (),
}

impl LayoutCtx {
    /// Create a fresh layout context.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for LayoutCtx {
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
