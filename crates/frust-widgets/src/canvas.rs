//! The `CanvasView`/`CanvasWidget` leaf: declarative custom painting over the
//! [`PaintScene`] trait object, for an app that wants to paint custom graphics
//! (a chart, a node-and-edge graph, a game board) without hand-rolling its own
//! `View`/`Widget` pair.
//!
//! [`canvas`] takes a paint closure, `Fn(&mut dyn PaintScene, Size, &PaintCtx)`,
//! that runs every time this widget actually paints. The widget pushes a
//! translation onto the scene's transform stack (its own absolute origin) and a
//! clip to its laid-out bounds before invoking the closure, then pops both —
//! so the closure paints entirely in **local space**: `(0, 0)` is this widget's
//! own top-left corner, regardless of where it sits in the tree, and a draw
//! call past its own `size` is simply clipped rather than bleeding into a
//! sibling.
//!
//! `.size(Size)`/`.expand()` choose the sizing policy — `.expand()` is the
//! default, mirroring [`crate::platform_view`]'s builder, so an app that
//! forgets to call either still gets a sensible full-bleed canvas rather than
//! a degenerate zero-size one. `.on_hit(..)` narrows pointer hit-testing to an
//! arbitrary shape (a circular node, a hex cell) — defaulted to the ordinary
//! "inside the rectangular bounds" test every other baseline leaf uses (see
//! `button.rs`'s own `inside` precedent) — consulted on `Down` (to decide
//! whether this canvas claims the gesture at all, so a press outside a narrow
//! hit-shape falls through to whatever sits beneath it) and again on `Up` (so
//! a drag that left the shape resolves as no-op, matching the "fire on
//! up-inside only" convention [`crate::Button`] already uses). `.on_tap(..)`/
//! `.on_pointer(..)` are the two optional callbacks [`CanvasView::on_hit`]
//! gates: `on_tap` fires once per up-inside release; `on_pointer` fires with
//! the raw [`frust_core::PointerEvent`] on every phase the hit-test admits,
//! for a caller that wants the full gesture (drag included) rather than just
//! the tap summary.
//!
//! # Repaint-on-rebuild only, by default
//!
//! `CanvasWidget` requests no frame and no redraw of its own; it repaints
//! exactly when the ordinary `View` diff already says so (a changed `.size`,
//! or an ancestor's own dirtying). A paint closure's *captured* data can
//! change from one rebuild to the next with nothing about the `CanvasView`
//! itself comparably different — closures aren't comparable, so the widget
//! can't detect that on its own. [`CanvasView::repaint_key`] is the escape
//! hatch: pass any [`Hash`] fingerprint of whatever the closure reads (a tick
//! counter, a node list's generation), and a changed hash across a rebuild
//! requests [`ChangeFlags::PAINT`] even though nothing else about the view
//! changed. Leaving it unset means the canvas repaints only when something
//! else already triggers one (e.g. the first build, or a `.size` change).
//!
//! ```
//! use frust_core::{PaintCtx, PaintScene, View};
//! use frust_widgets::canvas;
//! use kurbo::{Point, Size};
//! use peniko::Color;
//!
//! struct State {
//!     tick: u32,
//! }
//!
//! fn graph(state: &State) -> impl View<State> + use<> {
//!     let tick = state.tick;
//!     canvas(move |scene: &mut dyn PaintScene, size: Size, _ctx: &PaintCtx| {
//!         scene.fill_rect(Point::ZERO, size, Color::from_rgb8(0x20, 0x20, 0x20));
//!     })
//!     .expand()
//!     .repaint_key(tick)
//! }
//! # let _ = graph;
//! ```

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerEvent, PointerPhase, View, Widget,
};
use kurbo::{Affine, Point, Size};

use crate::authoring::presses;

/// How a [`CanvasView`] resolves its layout size — the same shape
/// [`crate::platform_view::PlatformViewView`]'s `SlotSize` uses, with
/// `Expand` as the default for the same reason: a canvas most often wants to
/// fill whatever space its container gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CanvasSize {
    /// Fill the incoming constraints' maximum.
    Expand,
    /// A fixed size, clamped into the incoming constraints.
    Explicit(Size),
}

/// The paint closure a [`CanvasView`] carries: renders into `&mut dyn
/// PaintScene` (local space — see the [module docs](self)), given the
/// widget's laid-out `Size` and the pass's own `&PaintCtx` (frame time, theme,
/// and the rest of the paint-time context a themed/animated canvas needs).
type CanvasPaint = Rc<dyn Fn(&mut dyn PaintScene, Size, &PaintCtx)>;

/// An optional custom hit-test: `Fn(local_point, widget_size) -> bool`. See
/// [`CanvasView::on_hit`].
type HitTest = Rc<dyn Fn(Point, Size) -> bool>;

/// A view-held tap callback (erased on build) — see [`CanvasView::on_tap`].
type TapCallback<State> = Rc<dyn Fn(&mut State)>;

/// A view-held pointer callback carrying the raw [`PointerEvent`] (erased on
/// build) — see [`CanvasView::on_pointer`].
type PointerCallback<State> = Rc<dyn Fn(&mut State, PointerEvent)>;

/// A declarative custom-painting canvas. See the [module docs](self).
pub struct CanvasView<State: 'static> {
    paint: CanvasPaint,
    hit: Option<HitTest>,
    on_tap: Option<TapCallback<State>>,
    on_pointer: Option<PointerCallback<State>>,
    size: CanvasSize,
    /// A hashed fingerprint of whatever external data the paint closure
    /// reads — see [`CanvasView::repaint_key`].
    repaint_key: Option<u64>,
}

/// Create a canvas that paints by calling `paint(scene, size, ctx)` every time
/// it actually repaints — full-bleed by default (see [`CanvasView::expand`]).
/// See the [module docs](self) for the local-space/clip contract and the
/// builder methods that round out the declaration.
pub fn canvas<State: 'static, F>(paint: F) -> CanvasView<State>
where
    F: Fn(&mut dyn PaintScene, Size, &PaintCtx) + 'static,
{
    CanvasView {
        paint: Rc::new(paint),
        hit: None,
        on_tap: None,
        on_pointer: None,
        size: CanvasSize::Expand,
        repaint_key: None,
    }
}

impl<State: 'static> CanvasView<State> {
    /// Force an explicit size, clamped into whatever constraints this
    /// widget's parent hands it. Overrides [`CanvasView::expand`] when called
    /// after it (last builder call wins, matching the crate's other
    /// size-mode builders — e.g. [`crate::platform_view`]).
    pub fn size(mut self, size: Size) -> Self {
        self.size = CanvasSize::Explicit(size);
        self
    }

    /// Fill the incoming constraints' maximum — the default (see the
    /// [module docs](self)), provided as an explicit builder call for
    /// readability at call sites that want to make the choice visible.
    pub fn expand(mut self) -> Self {
        self.size = CanvasSize::Expand;
        self
    }

    /// Narrow pointer hit-testing to `hit(local_point, size) -> bool`,
    /// consulted before [`CanvasView::on_tap`]/[`CanvasView::on_pointer`]
    /// dispatch (see the [module docs](self)). Unset, hit-testing falls back
    /// to the ordinary "inside the rectangular bounds" test every other
    /// baseline leaf uses.
    pub fn on_hit<F: Fn(Point, Size) -> bool + 'static>(mut self, hit: F) -> Self {
        self.hit = Some(Rc::new(hit));
        self
    }

    /// Fire `on_tap(state)` on an up-inside release (gated by
    /// [`CanvasView::on_hit`], see the [module docs](self)).
    pub fn on_tap<F: Fn(&mut State) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }

    /// Fire `on_pointer(state, event)` with the raw [`PointerEvent`] on every
    /// phase the hit-test admits (gated by [`CanvasView::on_hit`], see the
    /// [module docs](self)) — the escape hatch for a caller that wants the
    /// full gesture (drag included) rather than just the tap summary.
    pub fn on_pointer<F: Fn(&mut State, PointerEvent) + 'static>(mut self, on_pointer: F) -> Self {
        self.on_pointer = Some(Rc::new(on_pointer));
        self
    }

    /// Opt into paint-only dirtying driven by a dependency outside the
    /// ordinary `View` diff: hash any [`Hash`] fingerprint of the data the
    /// paint closure reads, and a changed hash across a rebuild requests
    /// [`ChangeFlags::PAINT`] even when nothing else about the view changed.
    /// See the [module docs](self#repaint-on-rebuild-only-by-default).
    pub fn repaint_key(mut self, key: impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        self.repaint_key = Some(hasher.finish());
        self
    }
}

/// `true` when `pos` falls within `size`'s rectangular bounds — the default
/// hit test when [`CanvasView::on_hit`] is unset (mirrors `button.rs`'s
/// `inside` helper, the same convention).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The retained widget for a [`CanvasView`]. See the [module docs](self).
pub struct CanvasWidget {
    paint: CanvasPaint,
    hit: Option<HitTest>,
    on_tap: Option<crate::authoring::ErasedCallback>,
    on_pointer: Option<crate::authoring::ErasedArgCallback<PointerEvent>>,
    size: CanvasSize,
    repaint_key: Option<u64>,
    /// Armed by a hit-passing `Down` (alongside `capture_pointer`), cleared
    /// on `Up`/`Cancel` — mirrors [`crate::Slider`]/[`crate::Button`]'s own
    /// `captured` flag.
    captured: bool,
}

impl CanvasWidget {
    /// Consult [`Self::hit`] if set, else fall back to [`inside`].
    fn hit_test(&self, point: Point, size: Size) -> bool {
        match &self.hit {
            Some(hit) => hit(point, size),
            None => inside(point, size),
        }
    }
}

impl<State: 'static> View<State> for CanvasView<State> {
    type Element = CanvasWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CanvasWidget {
        CanvasWidget {
            paint: self.paint.clone(),
            hit: self.hit.clone(),
            on_tap: self.on_tap.as_ref().map(crate::authoring::erase_callback),
            on_pointer: self
                .on_pointer
                .as_ref()
                .map(crate::authoring::erase_callback_arg),
            size: self.size,
            repaint_key: self.repaint_key,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CanvasWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        // The closure/hit/callbacks are reinstalled unconditionally every
        // rebuild — closures aren't comparable, matching every other
        // interactive widget's `erase_callback*` convention (see
        // `authoring::erase_callback`'s own doc comment).
        element.paint = self.paint.clone();
        element.hit = self.hit.clone();
        element.on_tap = self.on_tap.as_ref().map(crate::authoring::erase_callback);
        element.on_pointer = self
            .on_pointer
            .as_ref()
            .map(crate::authoring::erase_callback_arg);

        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // See the module docs' "Repaint-on-rebuild only" section: a changed
        // (or newly set/cleared) repaint_key is the only signal that content
        // behind the otherwise-incomparable paint closure actually changed.
        if prev.repaint_key != self.repaint_key {
            flags |= ChangeFlags::PAINT;
        }
        element.repaint_key = self.repaint_key;

        flags
    }
}

impl Widget for CanvasWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match self.size {
            CanvasSize::Explicit(size) => bc.constrain(size),
            CanvasSize::Expand => bc.max(),
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        // Local-space contract (see the module docs): translate so the
        // closure's own (0, 0) is this widget's top-left corner, and clip to
        // its bounds so an over-painting closure is clipped, not a bug to
        // chase.
        scene.push_transform(Affine::translate(origin.to_vec2()));
        scene.push_clip(Point::ZERO, size);
        (self.paint)(scene, size, ctx);
        scene.pop_clip();
        scene.pop_transform();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                // A hit-failing Down claims nothing: it falls through so
                // whatever sits beneath a narrow hit-shape (a sibling in a
                // `Stack`, a `PanZoomView` ancestor) still sees it.
                if !presses(p) || !self.hit_test(p.position, ctx.size()) {
                    return EventResult::Ignored;
                }
                self.captured = true;
                ctx.capture_pointer();
                if let Some(cb) = self.on_pointer.as_mut() {
                    cb(ctx, *p);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if let Some(cb) = self.on_pointer.as_mut() {
                    cb(ctx, *p);
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                // Fire on up-inside only (the same masonry semantics
                // `crate::Button` uses), "inside" being whatever hit-test is
                // active.
                if self.hit_test(p.position, ctx.size()) {
                    if let Some(cb) = self.on_pointer.as_mut() {
                        cb(ctx, *p);
                    }
                    if let Some(cb) = self.on_tap.as_mut() {
                        cb(ctx);
                    }
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Cancel only clears internal flags — never touches app
                // state (no callback), per the interaction-semantics
                // convention.
                self.captured = false;
                EventResult::Handled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{BuildCtx, PointerButton};
    use peniko::Color;
    use std::any::Any;
    use std::cell::RefCell;

    fn build<State: 'static>(view: &CanvasView<State>) -> CanvasWidget {
        let mut counter = 0u64;
        View::<State>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild<State: 'static>(
        prev: &CanvasView<State>,
        next: &CanvasView<State>,
        element: &mut CanvasWidget,
    ) -> ChangeFlags {
        let mut counter = 0u64;
        View::<State>::rebuild(next, prev, element, &mut BuildCtx::new(&mut counter))
    }

    /// Records every `PaintScene` call relevant to the tests below, in order.
    #[derive(Default)]
    struct Recorder {
        ops: Vec<Op>,
    }

    #[derive(Debug, PartialEq)]
    enum Op {
        Transform(Affine),
        PopTransform,
        Clip(Point, Size),
        PopClip,
        Fill(Point, Size, Color),
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.ops.push(Op::Fill(origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            self.ops.push(Op::Transform(transform));
        }
        fn pop_transform(&mut self) {
            self.ops.push(Op::PopTransform);
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.ops.push(Op::Clip(origin, size));
        }
        fn pop_clip(&mut self) {
            self.ops.push(Op::PopClip);
        }
    }

    fn pointer_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // -- layout -------------------------------------------------------------

    #[test]
    fn expand_fills_the_incoming_constraints_max() {
        let view = canvas::<(), _>(|_scene, _size, _ctx| {});
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, 200.0));
    }

    #[test]
    fn explicit_size_is_clamped_into_constraints() {
        let view = canvas::<(), _>(|_scene, _size, _ctx| {}).size(Size::new(1000.0, 1000.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, 200.0));
    }

    // -- paint: local space + laid-out size + clip ---------------------------

    #[test]
    fn paint_runs_the_closure_with_the_laid_out_size_translated_and_clipped() {
        let seen_size = Rc::new(RefCell::new(None));
        let seen_size_cb = seen_size.clone();
        let view = canvas::<(), _>(move |scene, size, _ctx| {
            *seen_size_cb.borrow_mut() = Some(size);
            scene.fill_rect(Point::new(1.0, 2.0), Size::new(3.0, 4.0), Color::BLACK);
        });
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let laid_out = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(80.0, 60.0)));
        assert_eq!(laid_out, Size::new(80.0, 60.0));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::new(10.0, 20.0), Size::new(80.0, 60.0));
        w.paint(&mut pctx, &mut rec);

        assert_eq!(
            *seen_size.borrow(),
            Some(Size::new(80.0, 60.0)),
            "the closure sees the laid-out size"
        );
        assert_eq!(
            rec.ops,
            vec![
                Op::Transform(Affine::translate((10.0, 20.0))),
                Op::Clip(Point::ZERO, Size::new(80.0, 60.0)),
                Op::Fill(Point::new(1.0, 2.0), Size::new(3.0, 4.0), Color::BLACK),
                Op::PopClip,
                Op::PopTransform,
            ],
            "the widget pushes its absolute origin as a transform and clips to \
             its bounds before the closure paints, then pops both"
        );
    }

    // -- repaint_key ----------------------------------------------------------

    #[test]
    fn unset_repaint_key_requests_no_paint_flag_on_an_otherwise_unchanged_rebuild() {
        let prev = canvas::<(), _>(|_scene, _size, _ctx| {});
        let mut w = build(&prev);
        let next = canvas::<(), _>(|_scene, _size, _ctx| {});
        let flags = rebuild(&prev, &next, &mut w);
        assert_eq!(
            flags,
            ChangeFlags::NONE,
            "no repaint_key set: an otherwise-unchanged rebuild requests nothing"
        );
    }

    #[test]
    fn a_changed_repaint_key_requests_paint() {
        let prev = canvas::<(), _>(|_scene, _size, _ctx| {}).repaint_key(1u32);
        let mut w = build(&prev);
        let next = canvas::<(), _>(|_scene, _size, _ctx| {}).repaint_key(2u32);
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_paint(), "a changed repaint_key requests PAINT");
    }

    #[test]
    fn an_unchanged_repaint_key_requests_no_paint() {
        let prev = canvas::<(), _>(|_scene, _size, _ctx| {}).repaint_key(7u32);
        let mut w = build(&prev);
        let next = canvas::<(), _>(|_scene, _size, _ctx| {}).repaint_key(7u32);
        let flags = rebuild(&prev, &next, &mut w);
        assert_eq!(
            flags,
            ChangeFlags::NONE,
            "an unchanged repaint_key requests nothing"
        );
    }

    // -- on_hit gates the tap callback ----------------------------------------

    #[derive(Default)]
    struct TapState {
        taps: u32,
        pointers: u32,
    }

    fn dispatch(w: &mut CanvasWidget, state: &mut TapState, event: &InputEvent, size: Size) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    #[test]
    fn with_no_on_hit_a_tap_inside_the_rect_fires() {
        let view = canvas::<TapState, _>(|_scene, _size, _ctx| {}).on_tap(|s: &mut TapState| {
            s.taps += 1;
        });
        let mut w = build(&view);
        let mut state = TapState::default();
        let size = Size::new(100.0, 100.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 10.0, 10.0),
            size,
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, 10.0, 10.0),
            size,
        );
        assert_eq!(state.taps, 1, "the default rectangular hit-test admits it");
    }

    #[test]
    fn on_hit_rejects_a_down_outside_its_shape_never_capturing() {
        // A circular hit-shape, radius 10, centered in a 100x100 box: a Down
        // at a corner (well outside the circle but inside the rect) must be
        // rejected, and no capture should ever open — the event is left for
        // whatever sits beneath this canvas.
        let view = canvas::<TapState, _>(|_scene, _size, _ctx| {})
            .on_hit(|p: Point, size: Size| {
                let center = Point::new(size.width / 2.0, size.height / 2.0);
                (p - center).hypot() <= 10.0
            })
            .on_tap(|s: &mut TapState| s.taps += 1)
            .on_pointer(|s: &mut TapState, _ev| s.pointers += 1);
        let mut w = build(&view);
        let mut state = TapState::default();
        let size = Size::new(100.0, 100.0);

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 1.0, 1.0),
            size,
        );
        assert!(!w.captured, "a hit-failing Down must not open a capture");
        assert_eq!(state.pointers, 0);

        // A follow-up Up (as if this canvas somehow still saw it) is a no-op
        // too, since nothing captured.
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, 1.0, 1.0),
            size,
        );
        assert_eq!(state.taps, 0);
        assert_eq!(state.pointers, 0);
    }

    #[test]
    fn on_hit_admits_a_down_inside_its_shape_and_the_tap_fires_on_up() {
        let view = canvas::<TapState, _>(|_scene, _size, _ctx| {})
            .on_hit(|p: Point, size: Size| {
                let center = Point::new(size.width / 2.0, size.height / 2.0);
                (p - center).hypot() <= 10.0
            })
            .on_tap(|s: &mut TapState| s.taps += 1)
            .on_pointer(|s: &mut TapState, _ev| s.pointers += 1);
        let mut w = build(&view);
        let mut state = TapState::default();
        let size = Size::new(100.0, 100.0);
        let center = (50.0, 50.0);

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, center.0, center.1),
            size,
        );
        assert!(w.captured, "a hit-passing Down opens a capture");
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, center.0, center.1),
            size,
        );
        assert_eq!(state.taps, 1, "the up-inside release fires the tap");
        assert_eq!(state.pointers, 2, "on_pointer rides along on Down and Up");
    }

    #[test]
    fn a_captured_press_released_outside_the_hit_shape_does_not_tap() {
        // Down inside the circle (captures), drag/release outside it: the
        // "fire on up-inside only" convention means no tap fires, mirroring
        // `crate::Button`'s own up-inside semantics.
        let view = canvas::<TapState, _>(|_scene, _size, _ctx| {})
            .on_hit(|p: Point, size: Size| {
                let center = Point::new(size.width / 2.0, size.height / 2.0);
                (p - center).hypot() <= 10.0
            })
            .on_tap(|s: &mut TapState| s.taps += 1);
        let mut w = build(&view);
        let mut state = TapState::default();
        let size = Size::new(100.0, 100.0);

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 50.0, 50.0),
            size,
        );
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Move, 90.0, 90.0),
            size,
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, 90.0, 90.0),
            size,
        );
        assert_eq!(state.taps, 0, "a release outside the hit-shape never taps");
        assert!(!w.captured, "capture still clears on Up regardless");
    }

    #[test]
    fn cancel_clears_captured_state_and_fires_nothing() {
        let view = canvas::<TapState, _>(|_scene, _size, _ctx| {}).on_tap(|s: &mut TapState| {
            s.taps += 1;
        });
        let mut w = build(&view);
        let mut state = TapState::default();
        let size = Size::new(100.0, 100.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 10.0, 10.0),
            size,
        );
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Cancel, 10.0, 10.0),
            size,
        );
        assert!(!w.captured, "Cancel disarms the press");
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, 10.0, 10.0),
            size,
        );
        assert_eq!(
            state.taps, 0,
            "an Up after Cancel is a no-op (not captured)"
        );
    }
}
