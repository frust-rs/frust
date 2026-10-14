//! `CustomScrollView`: the sliver layout loop, cache extent, correction,
//! both axes, scrolling parity with `ScrollView` (alone and nested), the
//! semantics node, and the layout-skip audit over the viewport.
//!
//! The drag/fling/wheel/controller script and the audit harness mirror
//! `layout_skip_widgets.rs`'s, which exercise `ScrollView` and `ListView` the
//! same way, so the viewport is held to exactly what they are.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Curve, DiscardScene, FrameTime, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot,
    ScrollAxis, ScrollDelta, SliverConstraints, SliverGeometry, SliverWidget, View, Widget,
    WidgetId, any,
};
use frust_text::TextContext;
use frust_widgets::{
    AnimateTo, Clamping, Column, ScrollController, SizedBox, ViewportWidget, custom_scroll_view,
    scroll_view, sliver_box, text,
};
use kurbo::{Point, Size};

const FRAME_MS: f64 = 16.0;

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// What a [`Probe`] sliver saw and did.
#[derive(Default)]
struct Log {
    /// Every constraints value a sliver layout actually ran with.
    constraints: RefCell<Vec<SliverConstraints>>,
    layouts: Cell<u32>,
    paints: Cell<u32>,
    /// A correction the next sliver layout reports once.
    correction: Cell<Option<f64>>,
    /// Report a correction on every sliver layout.
    always_correct: Cell<bool>,
}

/// A sliver of a fixed `extent` that logs its layouts and paints, and reports
/// a scroll-offset correction on request. `generation` is its dirty channel:
/// a rebuild with a new one reports `LAYOUT`.
struct Probe {
    extent: f64,
    log: Rc<Log>,
    generation: u32,
}

fn probe(extent: f64, log: &Rc<Log>) -> Probe {
    Probe {
        extent,
        log: Rc::clone(log),
        generation: 0,
    }
}

struct ProbeWidget {
    extent: f64,
    log: Rc<Log>,
}

impl View<()> for Probe {
    type Element = ProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        ProbeWidget {
            extent: self.extent,
            log: Rc::clone(&self.log),
        }
    }

    fn rebuild(&self, prev: &Self, el: &mut ProbeWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        el.extent = self.extent;
        if prev.generation != self.generation || prev.extent != self.extent {
            ChangeFlags::LAYOUT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// How much of `[0, extent)` lies in `[a, a + len)`, clamped to `len`.
fn span_in(extent: f64, a: f64, len: f64) -> f64 {
    let b = a + len;
    (extent.clamp(a, b) - 0.0f64.clamp(a, b)).clamp(0.0, len)
}

impl Widget for ProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, self.extent))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.log.paints.set(self.log.paints.get() + 1);
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }
}

impl SliverWidget for ProbeWidget {
    fn layout_sliver(&mut self, _ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        self.log.layouts.set(self.log.layouts.get() + 1);
        self.log.constraints.borrow_mut().push(*sc);
        if self.log.always_correct.get() {
            return SliverGeometry {
                scroll_offset_correction: Some(1.0),
                ..SliverGeometry::ZERO
            };
        }
        if let Some(correction) = self.log.correction.take() {
            return SliverGeometry {
                scroll_offset_correction: Some(correction),
                ..SliverGeometry::ZERO
            };
        }
        let paint = span_in(self.extent, sc.scroll_offset, sc.remaining_paint_extent);
        let cache = span_in(
            self.extent,
            sc.scroll_offset + sc.cache_origin,
            sc.remaining_cache_extent,
        )
        .max(paint);
        SliverGeometry {
            cache_extent: cache,
            ..SliverGeometry::new(self.extent, paint, self.extent)
        }
    }
}

/// A plain box child `height` tall that counts its layouts and paints.
struct Counted {
    height: f64,
    log: Rc<Log>,
}

struct CountedWidget {
    height: f64,
    log: Rc<Log>,
}

impl View<()> for Counted {
    type Element = CountedWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CountedWidget {
        CountedWidget {
            height: self.height,
            log: Rc::clone(&self.log),
        }
    }

    fn rebuild(&self, _p: &Self, _el: &mut CountedWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for CountedWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.log.layouts.set(self.log.layouts.get() + 1);
        bc.constrain(Size::new(bc.max().width, self.height))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.log.paints.set(self.log.paints.get() + 1);
    }
}

/// `n` rows `extent` px along a vertical axis, as plain box children.
fn rows(n: usize, extent: f64) -> Vec<AnyView<()>> {
    (0..n)
        .map(|_| any(SizedBox::<()>(None, Some(extent))))
        .collect()
}

fn tween(ms: f64) -> AnimateTo {
    AnimateTo {
        duration_ms: ms,
        curve: Curve::EaseInOut,
    }
}

// ---------------------------------------------------------------------------
// A frame-driving harness over any root view.
// ---------------------------------------------------------------------------

type Logic = Box<dyn FnMut(&mut ()) -> AnyView<()>>;

struct Harness {
    root: RenderRoot<(), AnyView<()>>,
    logic: Logic,
    window: Size,
    tcx: TextContext,
    ms: f64,
    frames: u32,
    /// Lay out only when the pending flags ask (the shells' frame gate).
    gated: bool,
    /// Gated: pods a frame the gate skipped left with stale geometry.
    gate_misses: u64,
}

impl Harness {
    fn new(window: Size, logic: impl FnMut(&mut ()) -> AnyView<()> + 'static) -> Self {
        let mut harness = Self {
            root: RenderRoot::new(),
            logic: Box::new(logic),
            window,
            tcx: TextContext::new(),
            ms: 0.0,
            frames: 0,
            gated: false,
            gate_misses: 0,
        };
        harness.frames(3);
        harness
    }

    /// The same, mounted one level down under a window-sized box, so the
    /// viewport sits in a pod the layout skip can reach.
    fn boxed(window: Size, mut logic: impl FnMut(&mut ()) -> AnyView<()> + 'static) -> Self {
        Self::new(window, move |state: &mut ()| {
            any(SizedBox::<()>(Some(window.width), Some(window.height)).child(logic(state)))
        })
    }

    fn frame(&mut self) {
        self.root.rebuild(&mut self.logic, &mut ());
        if !self.gated || self.root.take_change_flags().needs_layout() {
            self.root.layout_with_text(self.window, &mut self.tcx);
        } else {
            self.gate_misses += self.forced_pass_differs();
        }
        self.ms += FRAME_MS;
        self.root.paint(
            &mut DiscardScene,
            FrameTime::from_nanos((self.ms * 1_000_000.0) as u64),
        );
        self.frames += 1;
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// Every pod's `(origin relative to parent, size)`, keyed by id.
    fn geometry(&self) -> HashMap<WidgetId, (Point, Size)> {
        let nodes = self.root.inspect();
        let origins: HashMap<WidgetId, Point> =
            nodes.iter().map(|n| (n.id, n.bounds.origin())).collect();
        nodes
            .iter()
            .map(|n| {
                let parent = n
                    .parent
                    .and_then(|p| origins.get(&p).copied())
                    .unwrap_or(Point::ZERO);
                let origin = Point::new(n.bounds.x0 - parent.x, n.bounds.y0 - parent.y);
                (n.id, (origin, n.bounds.size()))
            })
            .collect()
    }

    /// Run a forced pass and count the pods it moved or resized.
    fn forced_pass_differs(&mut self) -> u64 {
        let before = self.geometry();
        self.root.set_layout_skip(false);
        self.root.layout_with_text(self.window, &mut self.tcx);
        self.root.set_layout_skip(true);
        let mut count = 0;
        for (id, (origin, size)) in self.geometry() {
            let Some((was_origin, was_size)) = before.get(&id) else {
                continue;
            };
            if (was_origin.x - origin.x).abs() > 1e-3
                || (was_origin.y - origin.y).abs() > 1e-3
                || (was_size.width - size.width).abs() > 1e-3
                || (was_size.height - size.height).abs() > 1e-3
            {
                count += 1;
            }
        }
        count
    }

    fn pointer(&mut self, phase: PointerPhase, at: Point) {
        self.root.event(
            &mut (),
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: at,
                button: PointerButton::Primary,
            }),
        );
    }

    /// Press at `from`, move to `to` in `steps` moves with a frame after each,
    /// and release.
    fn drag(&mut self, from: Point, to: Point, steps: u32) {
        self.pointer(PointerPhase::Down, from);
        self.frame();
        for i in 1..=steps {
            let t = f64::from(i) / f64::from(steps);
            self.pointer(PointerPhase::Move, from.lerp(to, t));
            self.frame();
        }
        self.pointer(PointerPhase::Up, to);
        self.frame();
    }

    fn wheel(&mut self, at: Point, delta: ScrollDelta) {
        self.pointer(PointerPhase::Move, at);
        self.root.event(
            &mut (),
            &InputEvent::Scroll {
                delta,
                position: at,
            },
        );
        self.frame();
    }

    /// The root view's own viewport widget (a harness built with
    /// [`Harness::new`] over a bare `CustomScrollView`).
    fn viewport(&self) -> &ViewportWidget {
        let id = self.root.root_id().expect("root built");
        let erased = (self.root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<Box<dyn Widget>>()
            .expect("an AnyView root holds a boxed widget");
        (&**erased as &dyn Any)
            .downcast_ref::<ViewportWidget>()
            .expect("the root is a ViewportWidget")
    }

    /// The window bounds of every laid-out widget whose type name contains
    /// `name`, in tree order.
    fn bounds_of(&self, name: &str) -> Vec<kurbo::Rect> {
        self.root
            .inspect()
            .iter()
            .filter(|n| n.type_name.contains(name))
            .map(|n| n.bounds)
            .collect()
    }
}

/// The drag/fling/overscroll/wheel/controller script both
/// `layout_skip_widgets.rs`'s scroll surfaces run, with `between` called after
/// every frame-advancing step.
fn scroll_script(h: &mut Harness, controller: &ScrollController) {
    h.drag(Point::new(200.0, 500.0), Point::new(200.0, 150.0), 8);
    h.frames(30);
    controller.jump_to(0.0);
    h.frame();
    h.drag(Point::new(200.0, 100.0), Point::new(200.0, 400.0), 6);
    h.frames(20);
    h.wheel(Point::new(200.0, 300.0), ScrollDelta::Pixels(0.0, 120.0));
    h.wheel(Point::new(200.0, 300.0), ScrollDelta::Pixels(0.0, -40.0));
    controller.jump_to(900.0);
    h.frames(3);
    controller.jump_to(450.0);
    controller.jump_to(460.0);
    h.frames(2);
    h.drag(Point::new(200.0, 500.0), Point::new(200.0, 100.0), 4);
    h.frames(3);
    controller.jump_to(200.0);
    h.frames(5);
    controller.animate_to(1200.0, tween(250.0));
    h.frames(25);
    controller.animate_to(0.0, tween(400.0));
    h.frames(5);
    controller.jump_to(700.0);
    h.frames(5);
    controller.jump_to(f64::INFINITY);
    h.frames(3);
}

// ---------------------------------------------------------------------------
// (i) Two sliver_box children in a short viewport.
// ---------------------------------------------------------------------------

#[test]
fn two_sliver_boxes_lay_out_in_sequence_with_the_remaining_extent_scrollable() {
    let controller = ScrollController::new();
    let c = controller.clone();
    let h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            sliver_box(SizedBox::<()>(None, Some(300.0))),
            sliver_box(SizedBox::<()>(None, Some(300.0))),
        ))
        .controller(c.clone()))
    });
    let w = h.viewport();
    let first = w.sliver_geometry(0).expect("child 0");
    let second = w.sliver_geometry(1).expect("child 1");
    assert_eq!(
        (first.scroll_extent, first.paint_extent, first.layout_extent),
        (300.0, 300.0, 300.0)
    );
    assert!(first.visible && !first.has_visual_overflow);
    assert_eq!(
        (
            second.scroll_extent,
            second.paint_extent,
            second.max_paint_extent
        ),
        (300.0, 200.0, 300.0)
    );
    assert!(
        second.visible && second.has_visual_overflow,
        "cut by the end"
    );
    assert_eq!(w.max_offset(), 100.0);
    assert_eq!(controller.max_offset(), 100.0);
    // Each sliver pod is sized to what it paints, placed at its paint start.
    let slivers = h.bounds_of("SliverBoxWidget");
    assert_eq!(slivers.len(), 2);
    assert_eq!(
        (slivers[0].origin(), slivers[0].size()),
        (Point::ZERO, Size::new(400.0, 300.0))
    );
    assert_eq!(
        (slivers[1].origin(), slivers[1].size()),
        (Point::new(0.0, 300.0), Size::new(400.0, 200.0))
    );
}

// ---------------------------------------------------------------------------
// (ii) Scrolling into the first child.
// ---------------------------------------------------------------------------

#[test]
fn scrolling_to_250_lays_the_first_child_out_250_in_with_50_px_left_to_paint() {
    let controller = ScrollController::new();
    let log = Rc::new(Log::default());
    let (c, l) = (controller.clone(), Rc::clone(&log));
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            probe(300.0, &l),
            sliver_box(SizedBox::<()>(None, Some(300.0))),
            sliver_box(SizedBox::<()>(None, Some(300.0))),
        ))
        .controller(c.clone()))
    });
    controller.jump_to(250.0);
    h.frame();
    assert_eq!(controller.offset(), 250.0);
    let sc = *log.constraints.borrow().last().expect("laid out");
    assert_eq!(sc.scroll_offset, 250.0);
    assert_eq!(sc.remaining_paint_extent, 500.0);
    let w = h.viewport();
    assert_eq!(w.sliver_geometry(0).expect("child 0").paint_extent, 50.0);
    // The next child starts where the first stops painting, unscrolled.
    let second = w.sliver_geometry(1).expect("child 1");
    assert_eq!(second.paint_extent, 300.0);
    let boxes = h.bounds_of("SliverBoxWidget");
    assert_eq!(boxes[0].origin(), Point::new(0.0, 50.0));
    assert_eq!(boxes[1].origin(), Point::new(0.0, 350.0));
    assert_eq!(boxes[1].size(), Size::new(400.0, 150.0));
    // A sliver_box's own child sits at minus its scroll offset: the third
    // child is unscrolled, so its box child is at its paint start.
    let inner = h.bounds_of("SizedBoxWidget");
    assert_eq!(inner[1].origin(), Point::new(0.0, 350.0));
}

// ---------------------------------------------------------------------------
// (iii) Scroll-offset correction.
// ---------------------------------------------------------------------------

#[test]
fn a_scroll_offset_correction_shifts_the_position_once_and_converges() {
    let controller = ScrollController::new();
    let log = Rc::new(Log::default());
    let generation = Rc::new(Cell::new(0u32));
    let (c, l, g) = (controller.clone(), Rc::clone(&log), Rc::clone(&generation));
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        let mut sliver = probe(2000.0, &l);
        sliver.generation = g.get();
        any(custom_scroll_view(sliver).controller(c.clone()))
    });
    controller.jump_to(100.0);
    h.frame();
    assert_eq!(controller.offset(), 100.0);

    log.correction.set(Some(40.0));
    generation.set(1);
    let before = log.layouts.get();
    h.frame();
    assert_eq!(
        log.layouts.get() - before,
        2,
        "the correcting layout, then exactly one restart"
    );
    assert_eq!(controller.offset(), 140.0, "shifted by the correction once");
    let sc = *log.constraints.borrow().last().expect("laid out");
    assert_eq!(
        sc.scroll_offset, 140.0,
        "the restart ran at the corrected offset"
    );

    let settled = log.layouts.get();
    h.frames(3);
    assert_eq!(controller.offset(), 140.0);
    assert_eq!(
        log.layouts.get(),
        settled,
        "converged: nothing lays out again"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "scroll-offset correction")]
fn a_sliver_that_never_stops_correcting_trips_the_cycle_bound() {
    let log = Rc::new(Log::default());
    log.always_correct.set(true);
    let l = Rc::clone(&log);
    let _ = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view(probe(1000.0, &l)))
    });
}

// ---------------------------------------------------------------------------
// (iv) Cache extent.
// ---------------------------------------------------------------------------

#[test]
fn a_child_in_the_cache_region_is_laid_out_but_never_painted() {
    let boxed = Rc::new(Log::default());
    let sliver = Rc::new(Log::default());
    let beyond = Rc::new(Log::default());
    let (b, s, f) = (Rc::clone(&boxed), Rc::clone(&sliver), Rc::clone(&beyond));
    let h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            sliver_box(SizedBox::<()>(None, Some(500.0))),
            // [500, 600) and [600, 700): inside the 250 px cache area.
            Counted {
                height: 100.0,
                log: Rc::clone(&b),
            },
            probe(100.0, &s),
            // [700, 1100): mostly past it.
            probe(400.0, &f),
        )))
    });
    let w = h.viewport();
    let cached_box = w.sliver_geometry(1).expect("child 1");
    let cached_sliver = w.sliver_geometry(2).expect("child 2");
    let past = w.sliver_geometry(3).expect("child 3");
    assert!(!cached_box.visible && !cached_sliver.visible);
    assert_eq!(cached_box.cache_extent, 100.0);
    assert_eq!(cached_sliver.cache_extent, 100.0);
    assert_eq!(past.cache_extent, 50.0, "only [700, 750) is cached");
    assert!(boxed.layouts.get() > 0 && sliver.layouts.get() > 0);
    assert_eq!(boxed.paints.get(), 0, "a cached box child is never painted");
    assert_eq!(sliver.paints.get(), 0, "a cached sliver is never painted");
    assert_eq!(beyond.paints.get(), 0);
    let sc = *sliver.constraints.borrow().last().expect("laid out");
    assert_eq!(sc.remaining_paint_extent, 0.0);
    assert_eq!(sc.remaining_cache_extent, 150.0);
}

// ---------------------------------------------------------------------------
// (v) Horizontal axis.
// ---------------------------------------------------------------------------

#[test]
fn a_horizontal_viewport_lays_out_across_and_scrolls_along_x() {
    let controller = ScrollController::new();
    let c = controller.clone();
    let mut h = Harness::new(Size::new(500.0, 400.0), move |_| {
        any(custom_scroll_view((
            sliver_box(SizedBox::<()>(Some(300.0), None)),
            sliver_box(SizedBox::<()>(Some(300.0), None)),
            sliver_box(SizedBox::<()>(Some(300.0), None)),
        ))
        .axis(ScrollAxis::Horizontal)
        .physics(Clamping::new())
        .controller(c.clone()))
    });
    let w = h.viewport();
    assert_eq!(w.axis(), ScrollAxis::Horizontal);
    assert_eq!(w.max_offset(), 400.0);
    let boxes = h.bounds_of("SliverBoxWidget");
    assert_eq!(
        (boxes[0].origin(), boxes[0].size()),
        (Point::ZERO, Size::new(300.0, 400.0))
    );
    assert_eq!(
        (boxes[1].origin(), boxes[1].size()),
        (Point::new(300.0, 0.0), Size::new(200.0, 400.0))
    );

    // Vertical travel is off-axis: nothing scrolls.
    h.drag(Point::new(250.0, 350.0), Point::new(250.0, 50.0), 6);
    h.frames(10);
    assert_eq!(controller.offset(), 0.0);
    // Leftward travel scrolls toward the end.
    h.drag(Point::new(450.0, 200.0), Point::new(250.0, 200.0), 6);
    h.frames(30);
    let dragged = controller.offset();
    assert!(dragged > 100.0, "the drag scrolled along x: {dragged}");
    let boxes = h.bounds_of("SliverBoxWidget");
    assert_eq!(boxes[0].origin(), Point::ZERO);
    assert_eq!(boxes[0].size().width, (300.0 - dragged).max(0.0));
    // The horizontal wheel delta scrolls; the vertical one does not.
    controller.jump_to(0.0);
    h.frame();
    h.wheel(Point::new(250.0, 200.0), ScrollDelta::Pixels(40.0, 300.0));
    assert_eq!(controller.offset(), 40.0);
}

/// Three 300 px columns in a 500 x 400 window, scrolling along x, with
/// `controller` attached.
fn three_columns(controller: &ScrollController) -> Harness {
    let c = controller.clone();
    Harness::new(Size::new(500.0, 400.0), move |_| {
        any(custom_scroll_view((
            sliver_box(SizedBox::<()>(Some(300.0), None)),
            sliver_box(SizedBox::<()>(Some(300.0), None)),
            sliver_box(SizedBox::<()>(Some(300.0), None)),
        ))
        .axis(ScrollAxis::Horizontal)
        .physics(Clamping::new())
        .controller(c.clone()))
    })
}

#[test]
fn a_horizontal_viewport_scrolls_its_core_along_x_and_a_diagonal_drag_moves_by_x_alone() {
    let straight = ScrollController::new();
    let mut flat = three_columns(&straight);
    assert_eq!(flat.viewport().axis(), ScrollAxis::Horizontal);
    flat.drag(Point::new(450.0, 200.0), Point::new(250.0, 200.0), 6);
    flat.frames(30);

    let diagonal = ScrollController::new();
    let mut slanted = three_columns(&diagonal);
    assert_eq!(slanted.viewport().axis(), ScrollAxis::Horizontal);
    slanted.drag(Point::new(450.0, 200.0), Point::new(250.0, 260.0), 6);
    slanted.frames(30);

    assert!(straight.offset() > 100.0, "the flat drag scrolled");
    assert!(
        (diagonal.offset() - straight.offset()).abs() < 1e-9,
        "the y component is off-axis: {} vs {}",
        diagonal.offset(),
        straight.offset()
    );
}

#[test]
fn a_rebuild_that_changes_the_axis_carries_it_into_the_core() {
    let horizontal = Rc::new(Cell::new(false));
    let flag = Rc::clone(&horizontal);
    let controller = ScrollController::new();
    let c = controller.clone();
    let mut h = Harness::new(Size::new(500.0, 400.0), move |_| {
        let view = custom_scroll_view((
            sliver_box(SizedBox::<()>(Some(300.0), Some(300.0))),
            sliver_box(SizedBox::<()>(Some(300.0), Some(300.0))),
            sliver_box(SizedBox::<()>(Some(300.0), Some(300.0))),
        ))
        .physics(Clamping::new())
        .controller(c.clone());
        any(if flag.get() {
            view.axis(ScrollAxis::Horizontal)
        } else {
            view
        })
    });
    assert_eq!(h.viewport().axis(), ScrollAxis::Vertical);
    h.drag(Point::new(250.0, 350.0), Point::new(250.0, 150.0), 6);
    h.frames(30);
    assert!(controller.offset() > 100.0, "the vertical drag scrolled");

    horizontal.set(true);
    controller.jump_to(0.0);
    h.frames(3);
    assert_eq!(h.viewport().axis(), ScrollAxis::Horizontal);
    assert_eq!(controller.offset(), 0.0);

    h.drag(Point::new(250.0, 350.0), Point::new(250.0, 50.0), 6);
    h.frames(30);
    assert_eq!(controller.offset(), 0.0, "the old axis no longer scrolls");
    h.drag(Point::new(450.0, 200.0), Point::new(250.0, 200.0), 6);
    h.frames(30);
    assert!(controller.offset() > 100.0, "the new axis scrolls");
}

// ---------------------------------------------------------------------------
// (vi) Scrolling parity with ScrollView.
// ---------------------------------------------------------------------------

#[test]
fn wheel_drag_fling_jump_and_animate_match_scroll_view_frame_for_frame() {
    let window = Size::new(400.0, 600.0);
    let reference = ScrollController::new();
    let viewport = ScrollController::new();
    let r = reference.clone();
    let mut scroll = Harness::new(window, move |_| {
        any(scroll_view(Column(rows(60, 50.0))).controller(r.clone()))
    });
    let v = viewport.clone();
    let mut sliver = Harness::new(window, move |_| {
        any(custom_scroll_view(rows(60, 50.0)).controller(v.clone()))
    });

    // Both run the shared script in lockstep, one step at a time, and are
    // compared after every frame through a recording wrapper.
    let trace = |h: &mut Harness, controller: &ScrollController| {
        let offsets = Rc::new(RefCell::new(Vec::new()));
        let record = Rc::clone(&offsets);
        let _sub = controller.on_change(move |info| record.borrow_mut().push(info.offset));
        scroll_script(h, controller);
        let max = controller.max_offset();
        let landed = controller.offset();
        drop(_sub);
        (offsets.take(), max, landed)
    };
    let (expected, expected_max, expected_end) = trace(&mut scroll, &reference);
    let (actual, actual_max, actual_end) = trace(&mut sliver, &viewport);
    assert_eq!(actual_max, expected_max);
    assert!(
        expected.len() > 50,
        "the script moved the reference surface"
    );
    assert_eq!(actual.len(), expected.len(), "the same number of moves");
    for (i, (a, e)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            (a - e).abs() < 1e-9,
            "move {i}: viewport {a}, scroll view {e}"
        );
    }
    assert_eq!(actual_end, expected_end);
    assert_eq!(actual_end, actual_max, "the last jump landed on the end");
}

// ---------------------------------------------------------------------------
// (vii) Nested inside a ScrollView.
// ---------------------------------------------------------------------------

/// A vertical scroll view: an 80 px header, a 300 px slot holding `inner`,
/// then twenty 50 px rows.
fn outer_with(inner: AnyView<()>, outer: ScrollController) -> impl View<()> {
    let mut content: Vec<AnyView<()>> = vec![any(SizedBox::<()>(None, Some(80.0)))];
    content.push(any(SizedBox::<()>(None, Some(300.0)).child(inner)));
    content.extend(rows(20, 50.0));
    scroll_view(Column(content))
        .physics(Clamping::new())
        .controller(outer)
}

/// The nested script: drag the inner, run it to its end, drag on into the
/// outer, then (outer back at the top) drag the inner back. Returns the
/// `(outer, inner)` offsets after each step.
fn nested_script(
    h: &mut Harness,
    outer: &ScrollController,
    inner: &ScrollController,
) -> Vec<(f64, f64)> {
    let mut seen = Vec::new();
    h.drag(Point::new(200.0, 350.0), Point::new(200.0, 150.0), 6);
    h.frames(25);
    seen.push((outer.offset(), inner.offset()));
    inner.jump_to(f64::INFINITY);
    h.frames(3);
    h.drag(Point::new(200.0, 350.0), Point::new(200.0, 150.0), 6);
    h.frames(25);
    seen.push((outer.offset(), inner.offset()));
    outer.jump_to(0.0);
    h.frames(2);
    h.drag(Point::new(200.0, 150.0), Point::new(200.0, 350.0), 6);
    h.frames(25);
    seen.push((outer.offset(), inner.offset()));
    seen
}

#[test]
fn nested_in_a_scroll_view_the_claim_arbitration_matches_scroll_view_in_scroll_view() {
    let window = Size::new(400.0, 600.0);
    let run = |as_viewport: bool| {
        let outer = ScrollController::new();
        let inner = ScrollController::new();
        let (o, i) = (outer.clone(), inner.clone());
        let mut h = Harness::new(window, move |_| {
            let nested = if as_viewport {
                any(custom_scroll_view(rows(40, 40.0))
                    .physics(Clamping::new())
                    .controller(i.clone()))
            } else {
                any(scroll_view(Column(rows(40, 40.0)))
                    .physics(Clamping::new())
                    .controller(i.clone()))
            };
            any(outer_with(nested, o.clone()))
        });
        nested_script(&mut h, &outer, &inner)
    };
    let expected = run(false);
    let actual = run(true);
    assert_eq!(actual, expected);
    // And what that arbitration is: the inner takes the first drag, the outer
    // the one the pinned inner cannot consume, the inner the drag back.
    assert_eq!(expected[0].0, 0.0);
    assert!(expected[0].1 > 0.0);
    assert!(expected[1].0 > 0.0);
    assert_eq!(expected[1].1, 1300.0, "the inner stayed pinned at its end");
    assert_eq!(expected[2].0, 0.0);
    assert!(expected[2].1 < 1300.0, "the inner took the drag back");
}

#[test]
fn a_horizontal_viewport_inside_a_vertical_scroll_view_never_holds_a_vertical_drag() {
    let outer = ScrollController::new();
    let carousel = ScrollController::new();
    let (o, c) = (outer.clone(), carousel.clone());
    let mut h = Harness::new(Size::new(400.0, 600.0), move |_| {
        let strip = custom_scroll_view(
            (0..8)
                .map(|_| SizedBox::<()>(Some(150.0), None))
                .collect::<Vec<_>>(),
        )
        .axis(ScrollAxis::Horizontal)
        .controller(c.clone());
        let mut content: Vec<AnyView<()>> = vec![any(SizedBox::<()>(None, Some(80.0)))];
        content.push(any(SizedBox::<()>(None, Some(200.0)).child(strip)));
        content.extend(rows(20, 50.0));
        any(scroll_view(Column(content)).controller(o.clone()))
    });
    // Mid-strip, the carousel could consume either direction of its own axis
    // under the default physics — but this drag is vertical.
    carousel.jump_to(300.0);
    h.frames(2);
    h.drag(Point::new(200.0, 250.0), Point::new(200.0, 50.0), 6);
    h.frames(30);
    assert!(outer.offset() > 0.0, "the outer took the vertical drag");
    assert_eq!(carousel.offset(), 300.0);
    // A horizontal drag over the strip is the carousel's.
    outer.jump_to(0.0);
    h.frames(2);
    h.drag(Point::new(300.0, 180.0), Point::new(100.0, 180.0), 6);
    h.frames(30);
    assert!(carousel.offset() > 300.0, "the carousel scrolled");
    assert_eq!(outer.offset(), 0.0);
}

// ---------------------------------------------------------------------------
// (viii) Semantics.
// ---------------------------------------------------------------------------

#[test]
fn the_semantics_node_is_a_scroll_view_with_its_range_over_its_children() {
    let controller = ScrollController::new();
    let c = controller.clone();
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            text("first"),
            sliver_box(SizedBox::<()>(None, Some(400.0))),
            text("last"),
            SizedBox::<()>(None, Some(600.0)),
        ))
        .controller(c.clone()))
    });
    controller.jump_to(120.0);
    h.frame();
    let max = controller.max_offset();
    assert!(max > 500.0);
    let update = h.root.semantics();
    let (_, node) = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::ScrollView)
        .expect("a ScrollView node");
    assert_eq!(node.scroll_y(), Some(120.0));
    assert_eq!(node.scroll_y_min(), Some(0.0));
    assert_eq!(node.scroll_y_max(), Some(max));
    assert!(
        !node.children().is_empty(),
        "the slivers' nodes hang under it"
    );

    let mut wide = Harness::new(Size::new(500.0, 400.0), |_| {
        any(custom_scroll_view(
            (0..4)
                .map(|_| SizedBox::<()>(Some(300.0), None))
                .collect::<Vec<_>>(),
        )
        .axis(ScrollAxis::Horizontal))
    });
    wide.frame();
    let update = wide.root.semantics();
    let (_, node) = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::ScrollView)
        .expect("a ScrollView node");
    assert_eq!(node.scroll_x_max(), Some(700.0));
    assert_eq!(node.scroll_y_max(), None);
}

// ---------------------------------------------------------------------------
// (ix) The layout-skip audit over the viewport.
// ---------------------------------------------------------------------------

#[test]
fn the_layout_audit_finds_no_stale_pod_across_the_scroll_script() {
    for gated in [false, true] {
        let controller = ScrollController::new();
        let log = Rc::new(Log::default());
        let (c, l) = (controller.clone(), Rc::clone(&log));
        let mut h = Harness::boxed(Size::new(400.0, 600.0), move |_| {
            any(custom_scroll_view((
                rows(10, 50.0),
                sliver_box(Column(rows(4, 60.0))),
                probe(900.0, &l),
                rows(30, 50.0),
            ))
            .controller(c.clone()))
        });
        h.root.set_layout_audit(true);
        h.gated = gated;
        scroll_script(&mut h, &controller);
        assert!(h.frames >= 60, "the script ran {} frames", h.frames);
        assert!(controller.offset() > 0.0);
        assert_eq!(
            h.root.layout_audit_mismatches() + h.gate_misses,
            0,
            "gated: {gated}"
        );
    }
}

/// A sanity check on the probe itself: a geometry it reports satisfies the
/// protocol at every position the script visits.
#[test]
fn the_probe_reports_valid_geometry() {
    let log = Rc::new(Log::default());
    let mut widget = ProbeWidget { extent: 300.0, log };
    for scroll_offset in [0.0, 100.0, 299.0, 300.0, 500.0] {
        let sc = SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 500.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 500.0,
            cache_origin: -scroll_offset.min(250.0),
            remaining_cache_extent: 750.0 + scroll_offset.min(250.0),
            user_scroll_direction: frust_core::ScrollDirection::Idle,
        };
        let mut ctx = LayoutCtx::new();
        widget.layout_sliver(&mut ctx, &sc).debug_assert_valid(&sc);
    }
}
