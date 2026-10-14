//! The structural slivers inside a `CustomScrollView`: `sliver_padding`,
//! `sliver_fill_remaining` and the lazy `sliver_fixed_extent_list` (positional
//! and keyed), plus the layout-skip audit over all three.
//!
//! The frame harness and the scroll script mirror `viewport.rs`'s.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Curve, DiscardScene, FrameTime, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot,
    ScrollAxis, ScrollDelta, View, Widget, WidgetId, any,
};
use frust_text::TextContext;
use frust_widgets::{
    AnimateTo, ChildKey, EdgeInsets, ScrollController, SizedBox, ViewportWidget,
    custom_scroll_view, sliver_box, sliver_fill_remaining, sliver_fixed_extent_list,
    sliver_fixed_extent_list_keyed, sliver_padding,
};
use kurbo::{Point, Rect, Size};

const FRAME_MS: f64 = 16.0;

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// Which tiles are alive (built and not yet torn down), by id.
#[derive(Default)]
struct Ledger {
    live: RefCell<BTreeSet<u64>>,
    builds: Cell<u32>,
}

/// A leaf `extent` px along a vertical axis that records its build and
/// teardown in a [`Ledger`].
struct Tile {
    id: u64,
    extent: f64,
    ledger: Rc<Ledger>,
}

fn tile(id: u64, extent: f64, ledger: &Rc<Ledger>) -> Tile {
    Tile {
        id,
        extent,
        ledger: Rc::clone(ledger),
    }
}

struct TileWidget {
    id: u64,
    extent: f64,
}

impl View<()> for Tile {
    type Element = TileWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TileWidget {
        self.ledger.live.borrow_mut().insert(self.id);
        self.ledger.builds.set(self.ledger.builds.get() + 1);
        TileWidget {
            id: self.id,
            extent: self.extent,
        }
    }

    fn rebuild(&self, prev: &Self, el: &mut TileWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        el.extent = self.extent;
        if prev.id != self.id {
            // A positional row handed new content: the ledger follows it.
            let mut live = self.ledger.live.borrow_mut();
            live.remove(&el.id);
            live.insert(self.id);
            el.id = self.id;
        }
        if prev.extent != self.extent {
            ChangeFlags::LAYOUT
        } else {
            ChangeFlags::NONE
        }
    }

    fn teardown(&self, el: &mut TileWidget, _ctx: &mut BuildCtx<'_>) {
        self.ledger.live.borrow_mut().remove(&el.id);
    }
}

impl Widget for TileWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, self.extent))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
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
    fn bounds_of(&self, name: &str) -> Vec<Rect> {
        self.root
            .inspect()
            .iter()
            .filter(|n| n.type_name.contains(name))
            .map(|n| n.bounds)
            .collect()
    }

    /// The ids of the rows the first fixed-extent list holds, in window order.
    fn list_rows(&self) -> Vec<WidgetId> {
        self.root
            .inspect()
            .into_iter()
            .find(|n| n.type_name.contains("SliverFixedExtentListWidget"))
            .expect("a fixed-extent list")
            .children
    }
}

/// The drag/fling/overscroll/wheel/controller script `viewport.rs` runs.
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
// sliver_padding.
// ---------------------------------------------------------------------------

#[test]
fn padding_shifts_and_sizes_its_sliver_including_when_scrolled_partly_off() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let (c, l) = (controller.clone(), Rc::clone(&ledger));
    let insets = EdgeInsets {
        left: 10.0,
        top: 20.0,
        right: 30.0,
        bottom: 40.0,
    };
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            sliver_padding(insets, sliver_box(tile(1, 100.0, &l))),
            SizedBox::<()>(None, Some(1000.0)),
        ))
        .controller(c.clone()))
    });
    let padding = h.viewport().sliver_geometry(0).expect("the padding");
    assert_eq!(
        (
            padding.scroll_extent,
            padding.paint_extent,
            padding.layout_extent,
            padding.max_paint_extent
        ),
        (160.0, 160.0, 160.0, 160.0)
    );
    assert_eq!(
        h.bounds_of("SliverPaddingWidget")[0],
        Rect::new(0.0, 0.0, 400.0, 160.0)
    );
    // The child sits inside the leading insets, narrowed by the cross ones.
    assert_eq!(
        h.bounds_of("TileWidget")[0],
        Rect::new(10.0, 20.0, 370.0, 120.0)
    );
    assert_eq!(h.viewport().max_offset(), 660.0);

    // Ten px in: half the leading inset has scrolled away.
    controller.jump_to(10.0);
    h.frame();
    let padding = h.viewport().sliver_geometry(0).expect("the padding");
    assert_eq!(
        (padding.scroll_extent, padding.paint_extent),
        (160.0, 150.0)
    );
    assert_eq!(
        h.bounds_of("TileWidget")[0],
        Rect::new(10.0, 10.0, 370.0, 110.0)
    );

    // Fifty px in: the leading inset and 30 px of the child are gone; the
    // child's sliver paints the 70 px left of it, its box scrolled back by 30.
    controller.jump_to(50.0);
    h.frame();
    let padding = h.viewport().sliver_geometry(0).expect("the padding");
    assert_eq!(
        (
            padding.scroll_extent,
            padding.paint_extent,
            padding.layout_extent
        ),
        (160.0, 110.0, 110.0)
    );
    assert!(padding.has_visual_overflow);
    assert_eq!(
        h.bounds_of("SliverBoxWidget")[0],
        Rect::new(10.0, 0.0, 370.0, 70.0)
    );
    assert_eq!(
        h.bounds_of("TileWidget")[0],
        Rect::new(10.0, -30.0, 370.0, 70.0)
    );

    // Past the child: only the trailing inset still paints.
    controller.jump_to(130.0);
    h.frame();
    let padding = h.viewport().sliver_geometry(0).expect("the padding");
    assert_eq!(padding.paint_extent, 30.0);
    assert!(padding.visible);

    // Scrolled wholly off: nothing paints, the next sliver starts at the top.
    controller.jump_to(200.0);
    h.frame();
    let padding = h.viewport().sliver_geometry(0).expect("the padding");
    assert_eq!(padding.paint_extent, 0.0);
    assert!(!padding.visible);
}

#[test]
fn horizontal_padding_takes_its_main_insets_from_left_and_right() {
    let mut h = Harness::new(Size::new(500.0, 300.0), |_| {
        any(custom_scroll_view((
            sliver_padding(
                EdgeInsets {
                    left: 15.0,
                    top: 5.0,
                    right: 25.0,
                    bottom: 10.0,
                },
                sliver_box(SizedBox::<()>(Some(100.0), None)),
            ),
            SizedBox::<()>(Some(1000.0), None),
        ))
        .axis(ScrollAxis::Horizontal))
    });
    h.frame();
    let padding = h.viewport().sliver_geometry(0).expect("the padding");
    assert_eq!(
        (padding.scroll_extent, padding.paint_extent),
        (140.0, 140.0)
    );
    let inner = h.bounds_of("SizedBoxWidget");
    assert_eq!(inner[0], Rect::new(15.0, 5.0, 115.0, 290.0));
}

// ---------------------------------------------------------------------------
// sliver_fill_remaining.
// ---------------------------------------------------------------------------

#[test]
fn fill_remaining_without_a_scroll_body_fills_exactly_the_rest() {
    let ledger = Rc::new(Ledger::default());
    let l = Rc::clone(&ledger);
    let h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            sliver_box(tile(1, 100.0, &l)),
            sliver_fill_remaining(tile(2, 50.0, &l)).has_scroll_body(false),
        )))
    });
    let fill = h.viewport().sliver_geometry(1).expect("the fill");
    assert_eq!((fill.scroll_extent, fill.paint_extent), (400.0, 400.0));
    assert_eq!(
        h.viewport().max_offset(),
        0.0,
        "a short child adds no scrolling"
    );
    let tiles = h.bounds_of("TileWidget");
    assert_eq!(tiles[1], Rect::new(0.0, 100.0, 400.0, 500.0));
}

#[test]
fn fill_remaining_without_a_scroll_body_grows_the_scroll_extent_for_a_taller_child() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let (c, l) = (controller.clone(), Rc::clone(&ledger));
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            sliver_box(tile(1, 100.0, &l)),
            sliver_fill_remaining(tile(2, 800.0, &l)).has_scroll_body(false),
        ))
        .controller(c.clone()))
    });
    let fill = h.viewport().sliver_geometry(1).expect("the fill");
    assert_eq!((fill.scroll_extent, fill.paint_extent), (800.0, 400.0));
    assert!(fill.has_visual_overflow);
    assert_eq!(h.viewport().max_offset(), 400.0);
    assert_eq!(h.bounds_of("TileWidget")[1].height(), 800.0);
    controller.jump_to(f64::INFINITY);
    h.frame();
    assert_eq!(controller.offset(), 400.0);
    assert_eq!(
        h.bounds_of("TileWidget")[1],
        Rect::new(0.0, -300.0, 400.0, 500.0)
    );
}

#[test]
fn fill_remaining_with_a_scroll_body_fills_the_paint_space_left_as_earlier_slivers_scroll_away() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let (c, l) = (controller.clone(), Rc::clone(&ledger));
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        any(custom_scroll_view((
            sliver_box(tile(1, 100.0, &l)),
            sliver_fill_remaining(tile(2, 50.0, &l)),
        ))
        .controller(c.clone()))
    });
    let fill = h.viewport().sliver_geometry(1).expect("the fill");
    assert_eq!((fill.scroll_extent, fill.paint_extent), (500.0, 400.0));
    assert_eq!(
        h.viewport().max_offset(),
        100.0,
        "a whole viewport of scroll body"
    );
    assert_eq!(
        h.bounds_of("TileWidget")[1],
        Rect::new(0.0, 100.0, 400.0, 500.0)
    );
    controller.jump_to(100.0);
    h.frame();
    let fill = h.viewport().sliver_geometry(1).expect("the fill");
    assert_eq!(fill.paint_extent, 500.0);
    assert_eq!(
        h.bounds_of("TileWidget")[1],
        Rect::new(0.0, 0.0, 400.0, 500.0)
    );
}

// ---------------------------------------------------------------------------
// sliver_fixed_extent_list.
// ---------------------------------------------------------------------------

#[test]
fn a_ten_thousand_row_list_materialises_only_the_cache_window_and_scrolls_to_its_end() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let (c, l) = (controller.clone(), Rc::clone(&ledger));
    let mut h = Harness::new(Size::new(400.0, 600.0), move |_| {
        let l = Rc::clone(&l);
        any(
            custom_scroll_view(sliver_fixed_extent_list(10_000, 50.0, move |i| {
                tile(i as u64, 50.0, &l)
            }))
            .controller(c.clone()),
        )
    });
    let live = |ledger: &Ledger| ledger.live.borrow().iter().copied().collect::<Vec<_>>();
    // The visible 600 px plus the 250 px cache below: rows 0..17.
    assert_eq!(live(&ledger), (0..17).collect::<Vec<_>>());
    assert_eq!(h.list_rows().len(), 17);
    let list = h.viewport().sliver_geometry(0).expect("the list");
    assert_eq!(
        (list.scroll_extent, list.paint_extent, list.max_paint_extent),
        (500_000.0, 600.0, 500_000.0)
    );
    let rows = h.bounds_of("TileWidget");
    assert_eq!(rows[0], Rect::new(0.0, 0.0, 400.0, 50.0));
    assert_eq!(rows[16], Rect::new(0.0, 800.0, 400.0, 850.0));

    // Mid-list: [9750, 10850) — rows 195..217, every earlier row torn down.
    controller.jump_to(10_000.0);
    h.frames(2);
    assert_eq!(live(&ledger), (195..217).collect::<Vec<_>>());
    assert_eq!(h.list_rows().len(), 22);
    let rows = h.bounds_of("TileWidget");
    assert_eq!(rows[0].y0, -250.0);
    assert_eq!(rows[5], Rect::new(0.0, 0.0, 400.0, 50.0));

    // The end: the max extent is exactly the rows' total less the viewport.
    controller.jump_to(f64::INFINITY);
    h.frames(2);
    assert_eq!(controller.offset(), 499_400.0);
    assert_eq!(controller.max_offset(), 499_400.0);
    assert_eq!(live(&ledger), (9983..10_000).collect::<Vec<_>>());
    let rows = h.bounds_of("TileWidget");
    assert_eq!(rows.last().expect("the last row").y1, 600.0);
    assert!(
        ledger.builds.get() < 100,
        "only window rows were ever built: {}",
        ledger.builds.get()
    );
}

#[test]
fn the_window_honours_the_cache_extent_and_the_slivers_before_the_list() {
    let ledger = Rc::new(Ledger::default());
    let l = Rc::clone(&ledger);
    let _h = Harness::new(Size::new(400.0, 600.0), move |_| {
        let l2 = Rc::clone(&l);
        any(custom_scroll_view((
            sliver_box(tile(1_000_000, 300.0, &l)),
            sliver_padding(
                EdgeInsets {
                    left: 0.0,
                    top: 100.0,
                    right: 0.0,
                    bottom: 0.0,
                },
                sliver_fixed_extent_list(100, 50.0, move |i| tile(i as u64, 50.0, &l2)),
            ),
        ))
        .cache_extent(0.0))
    });
    // The list starts at 400; 200 px of it are visible and nothing is cached.
    let rows: Vec<u64> = ledger
        .live
        .borrow()
        .iter()
        .copied()
        .filter(|id| *id < 1_000_000)
        .collect();
    assert_eq!(rows, (0..4).collect::<Vec<_>>());
}

#[test]
fn keyed_rows_keep_their_pods_across_a_prepend() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let data: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new((0..1000).collect()));
    let (c, l, d) = (controller.clone(), Rc::clone(&ledger), Rc::clone(&data));
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        let ids = d.borrow().clone();
        let keys = ids.clone();
        let l = Rc::clone(&l);
        any(custom_scroll_view(sliver_fixed_extent_list_keyed(
            ids.len(),
            50.0,
            move |i| ChildKey::new(keys[i]),
            move |i| tile(ids[i], 50.0, &l),
        ))
        .cache_extent(0.0)
        .controller(c.clone()))
    });
    controller.jump_to(1000.0);
    h.frames(2);
    // [1000, 1500): rows 20..30.
    assert_eq!(
        ledger.live.borrow().iter().copied().collect::<Vec<_>>(),
        (20..30).collect::<Vec<_>>()
    );
    let before = h.list_rows();
    assert_eq!(before.len(), 10);

    data.borrow_mut().insert(0, 5000);
    h.frames(2);
    // No anchoring: the window still covers indices 20..30, which now hold
    // ids 19..=28 — every id that stayed in the window kept its pod.
    assert_eq!(
        ledger.live.borrow().iter().copied().collect::<Vec<_>>(),
        (19..29).collect::<Vec<_>>()
    );
    let after = h.list_rows();
    assert_eq!(after.len(), 10);
    assert_eq!(
        after[1..],
        before[..9],
        "ids 20..=28 kept their pods, one slot down"
    );
    assert!(
        !before.contains(&after[0]),
        "id 19 entered the window fresh"
    );
}

#[test]
fn positional_rows_are_reused_by_index() {
    let ledger = Rc::new(Ledger::default());
    let offset = Rc::new(Cell::new(0u64));
    let (l, o) = (Rc::clone(&ledger), Rc::clone(&offset));
    let mut h = Harness::new(Size::new(400.0, 500.0), move |_| {
        let l = Rc::clone(&l);
        let shift = o.get();
        any(
            custom_scroll_view(sliver_fixed_extent_list(100, 50.0, move |i| {
                tile(i as u64 + shift, 50.0, &l)
            }))
            .cache_extent(0.0),
        )
    });
    let before = h.list_rows();
    offset.set(1000);
    h.frame();
    assert_eq!(
        h.list_rows(),
        before,
        "the same pods, rebuilt with new content"
    );
    assert_eq!(
        ledger.live.borrow().iter().copied().collect::<Vec<_>>(),
        (1000..1010).collect::<Vec<_>>()
    );
}

#[test]
fn near_edges_fire_once_per_approach_and_never_on_mount() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let starts = Rc::new(Cell::new(0u32));
    let ends = Rc::new(Cell::new(0u32));
    let (c, l, s, e) = (
        controller.clone(),
        Rc::clone(&ledger),
        Rc::clone(&starts),
        Rc::clone(&ends),
    );
    let mut h = Harness::new(Size::new(400.0, 600.0), move |_| {
        let l = Rc::clone(&l);
        let (s, e) = (Rc::clone(&s), Rc::clone(&e));
        any(custom_scroll_view(
            sliver_fixed_extent_list(1000, 50.0, move |i| tile(i as u64, 50.0, &l))
                .on_near_start(move |_: &mut ()| s.set(s.get() + 1), 200.0)
                .on_near_end(move |_: &mut ()| e.set(e.get() + 1), 300.0),
        )
        .controller(c.clone()))
    });
    assert_eq!((starts.get(), ends.get()), (0, 0), "mounting fires nothing");

    // 50 000 px of rows in a 600 px viewport: 1 400 px from the end, then 200.
    controller.jump_to(48_000.0);
    h.frames(2);
    assert_eq!(ends.get(), 0);
    controller.jump_to(49_200.0);
    h.frames(2);
    assert_eq!(ends.get(), 1, "the end came within 300 px");
    controller.jump_to(49_300.0);
    h.frames(2);
    assert_eq!(ends.get(), 1, "edge-triggered: no refire while near");
    controller.jump_to(40_000.0);
    h.frames(2);
    controller.jump_to(49_400.0);
    h.frames(2);
    assert_eq!(ends.get(), 2, "rearmed past twice the threshold");

    assert_eq!(starts.get(), 0);
    controller.jump_to(100.0);
    h.frames(2);
    assert_eq!(starts.get(), 1, "the start came within 200 px");
}

// ---------------------------------------------------------------------------
// The layout-skip audit.
// ---------------------------------------------------------------------------

#[test]
fn the_layout_audit_finds_no_stale_pod_across_the_scroll_script() {
    for gated in [false, true] {
        let controller = ScrollController::new();
        let ledger = Rc::new(Ledger::default());
        let (c, l) = (controller.clone(), Rc::clone(&ledger));
        let mut h = Harness::boxed(Size::new(400.0, 600.0), move |_| {
            let (l1, l2) = (Rc::clone(&l), Rc::clone(&l));
            any(custom_scroll_view((
                sliver_padding(
                    EdgeInsets::symmetric(12.0, 30.0),
                    sliver_fixed_extent_list(40, 45.0, move |i| tile(i as u64, 45.0, &l1)),
                ),
                sliver_fixed_extent_list_keyed(30, 60.0, ChildKey::new, move |i| {
                    tile(1000 + i as u64, 60.0, &l2)
                }),
                sliver_fill_remaining(tile(5000, 80.0, &l)).has_scroll_body(false),
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
