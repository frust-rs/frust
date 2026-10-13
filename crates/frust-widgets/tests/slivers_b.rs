//! The lazy slivers inside a `CustomScrollView`: the keyed, variable-extent
//! `sliver_list` (measurement, keyed anchoring through the viewport's
//! correction loop, the channel-feed shape), plus the layout-skip audit over
//! it.
//!
//! The frame harness and the scroll script mirror `viewport.rs`'s.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Curve, DiscardScene, FrameTime, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot,
    ScrollDelta, View, Widget, WidgetId, any,
};
use frust_text::TextContext;
use frust_widgets::{
    AnimateTo, ChildKey, ScrollController, SizedBox, SliverListWidget, ViewportWidget,
    custom_scroll_view, sliver_box, sliver_list,
};
use kurbo::{Point, Rect, Size};

const FRAME_MS: f64 = 16.0;

/// The estimate every list here runs with.
const ESTIMATE: f64 = 50.0;

/// A row's true extent: 30 to 90 px, 60 on average — never the estimate's.
fn height(id: u64) -> f64 {
    30.0 + (id % 7) as f64 * 10.0
}

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// What the tiles did: which are alive, which were ever built, where each
/// last painted, and how many rebuilds each had seen when it did.
#[derive(Default)]
struct Ledger {
    live: RefCell<BTreeSet<u64>>,
    built: RefCell<BTreeSet<u64>>,
    builds: Cell<u32>,
    painted: RefCell<HashMap<u64, f64>>,
    rebuilds: RefCell<HashMap<u64, u32>>,
}

impl Ledger {
    /// Where row `id` last painted its top, in window space.
    fn top_of(&self, id: u64) -> f64 {
        *self.painted.borrow().get(&id).expect("the row painted")
    }
}

/// A leaf `extent` px along a vertical axis that records its life in a
/// [`Ledger`] and counts its own rebuilds in its retained state.
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
    rebuilds: u32,
    ledger: Rc<Ledger>,
}

impl View<()> for Tile {
    type Element = TileWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TileWidget {
        self.ledger.live.borrow_mut().insert(self.id);
        self.ledger.built.borrow_mut().insert(self.id);
        self.ledger.builds.set(self.ledger.builds.get() + 1);
        TileWidget {
            id: self.id,
            extent: self.extent,
            rebuilds: 0,
            ledger: Rc::clone(&self.ledger),
        }
    }

    fn rebuild(&self, prev: &Self, el: &mut TileWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        el.rebuilds += 1;
        el.extent = self.extent;
        if prev.id != self.id {
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

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.ledger
            .painted
            .borrow_mut()
            .insert(self.id, ctx.origin().y);
        self.ledger
            .rebuilds
            .borrow_mut()
            .insert(self.id, self.rebuilds);
    }
}

fn tween(ms: f64) -> AnimateTo {
    AnimateTo {
        duration_ms: ms,
        curve: Curve::EaseInOut,
    }
}

/// The data behind a keyed list: one id per row.
type Ids = Rc<RefCell<Vec<u64>>>;

/// A `sliver_list` over `ids`, each row [`height`] tall and keyed by its id.
fn feed(ids: &Ids, ledger: &Rc<Ledger>) -> frust_widgets::SliverListView<()> {
    let rows = Rc::new(ids.borrow().clone());
    let keys = Rc::clone(&rows);
    let l = Rc::clone(ledger);
    sliver_list(
        rows.len(),
        move |i| ChildKey::new(keys[i]),
        move |i| tile(rows[i], height(rows[i]), &l),
    )
    .estimated_item_extent(ESTIMATE)
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

    /// Read the viewport's first `sliver_list`.
    fn list<R>(&self, read: impl FnOnce(&SliverListWidget) -> R) -> R {
        let mut read = Some(read);
        let mut out = None;
        self.viewport().visit_children(&mut |pod| {
            let widget: &dyn Any = pod.widget();
            let inner: &dyn Any = match widget.downcast_ref::<Box<dyn Widget>>() {
                Some(boxed) => &**boxed,
                None => widget,
            };
            if let Some(list) = inner.downcast_ref::<SliverListWidget>()
                && let Some(read) = read.take()
            {
                out = Some(read(list));
            }
        });
        out.expect("a sliver_list")
    }

    /// Read the first `sliver_list` anywhere in the tree (a harness built
    /// with [`Harness::boxed`]).
    fn list_nested<R>(&self, read: impl FnOnce(&SliverListWidget) -> R) -> R {
        fn find<R>(
            widget: &dyn Widget,
            read: &mut Option<impl FnOnce(&SliverListWidget) -> R>,
            out: &mut Option<R>,
        ) {
            let any: &dyn Any = widget;
            let inner: &dyn Any = match any.downcast_ref::<Box<dyn Widget>>() {
                Some(boxed) => &**boxed,
                None => any,
            };
            if let Some(list) = inner.downcast_ref::<SliverListWidget>() {
                if let Some(read) = read.take() {
                    *out = Some(read(list));
                }
                return;
            }
            widget.visit_children(&mut |pod| find(pod.widget(), read, out));
        }
        let id = self.root.root_id().expect("root built");
        let mut read = Some(read);
        let mut out = None;
        find(
            self.root.tree().pod(id).expect("root pod").widget(),
            &mut read,
            &mut out,
        );
        out.expect("a sliver_list")
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

/// A harness over one feed of `ids` with `controller`, 400 × 600.
fn feed_harness(ids: &Ids, ledger: &Rc<Ledger>, controller: &ScrollController) -> Harness {
    let (d, l, c) = (Rc::clone(ids), Rc::clone(ledger), controller.clone());
    Harness::new(Size::new(400.0, 600.0), move |_| {
        any(custom_scroll_view(feed(&d, &l)).controller(c.clone()))
    })
}

/// The first row whose top sits at or below the viewport's top edge — the
/// row a keyed mutation above it must leave where it is.
fn first_whole_row(ledger: &Ledger) -> (u64, f64) {
    ledger
        .live
        .borrow()
        .iter()
        .filter_map(|id| ledger.painted.borrow().get(id).map(|y| (*id, *y)))
        .filter(|(_, y)| *y >= 0.0 && *y < 600.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("a row on screen")
}

/// Run one frame with fresh paint records.
fn painted_frame(h: &mut Harness, ledger: &Ledger) {
    ledger.painted.borrow_mut().clear();
    h.frame();
}

// ---------------------------------------------------------------------------
// sliver_list: measurement and convergence.
// ---------------------------------------------------------------------------

#[test]
fn a_5000_row_variable_list_measures_only_built_rows_and_its_extent_converges() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((0..5000).collect()));
    let mut h = feed_harness(&ids, &ledger, &controller);
    let truth: f64 = (0..5000).map(height).sum();
    let model = |ledger: &Ledger| {
        let built = ledger.built.borrow();
        let measured: f64 = built.iter().copied().map(height).sum();
        measured + ESTIMATE * (5000 - built.len()) as f64
    };

    // The visible 600 px plus the 250 px cache: a few dozen rows at most.
    let built = ledger.built.borrow().len();
    assert!(built > 10 && built < 40, "{built} rows built");
    assert_eq!(h.list(|l| l.measured_count()), built);
    assert_eq!(h.list(|l| l.window().len()), ledger.live.borrow().len());
    assert!(
        (controller.max_offset() - (model(&ledger) - 600.0)).abs() < 1e-6,
        "Σ measured + estimate × unmeasured, less the viewport: {} vs {}",
        controller.max_offset(),
        model(&ledger) - 600.0
    );

    // Scroll through the whole list; every frame's extent is the model over
    // exactly the rows built so far, and it only ever approaches the truth.
    let mut error = (model(&ledger) - truth).abs();
    let mut offset = 0.0;
    while offset < truth - 600.0 {
        offset += 800.0;
        controller.jump_to(offset);
        h.frames(2);
        assert!(ledger.live.borrow().len() < 40, "the window stays lazy");
        let built = ledger.built.borrow().len();
        assert_eq!(
            h.list(|l| l.measured_count()),
            built,
            "only built rows measure"
        );
        if built < 5000 {
            assert!(
                (controller.max_offset() - (model(&ledger) - 600.0)).abs() < 1e-6,
                "at {offset}: max offset tracks the model"
            );
        }
        let now = (model(&ledger) - truth).abs();
        assert!(now <= error + 1e-6, "converging: {now} after {error}");
        error = now;
    }
    controller.jump_to(f64::INFINITY);
    h.frames(3);
    assert_eq!(ledger.built.borrow().len(), 5000);
    assert!((controller.max_offset() - (truth - 600.0)).abs() < 1e-6);
    assert!((controller.offset() - (truth - 600.0)).abs() < 1e-6);
    assert_eq!(h.list(|l| l.scroll_corrections()), 0, "nothing ever jumped");
}

#[test]
fn scrolling_back_over_rows_never_measured_corrects_the_leading_edge_invisibly() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((0..5000).collect()));
    let mut h = feed_harness(&ids, &ledger, &controller);
    // A jump lands among rows only ever estimated (50 px; they average 60).
    controller.jump_to(30_000.0);
    h.frames(4);
    let at = Point::new(200.0, 300.0);
    h.pointer(PointerPhase::Move, at);
    for step in 0.. {
        assert!(step < 500, "the start is reached");
        painted_frame(&mut h, &ledger);
        if ledger.painted.borrow().get(&0) == Some(&0.0) {
            break;
        }
        let (row, top) = first_whole_row(&ledger);
        let before = controller.offset();
        h.root.event(
            &mut (),
            &InputEvent::Scroll {
                delta: ScrollDelta::Pixels(0.0, -300.0),
                position: at,
            },
        );
        let wheeled = before - controller.offset();
        assert!(wheeled > 0.0, "the wheel scrolls up");
        painted_frame(&mut h, &ledger);
        // Every row moves by exactly the wheel's distance, whatever the
        // corrections the frame applied underneath.
        assert!(
            (ledger.top_of(row) - (top + wheeled)).abs() < 1e-6,
            "step {step}: row {row} moved from {top} to {}",
            ledger.top_of(row)
        );
    }
    assert!(
        h.list(|l| l.scroll_corrections()) > 0,
        "the rows above measured longer than estimated"
    );
    assert_eq!(controller.offset(), 0.0);
}

// ---------------------------------------------------------------------------
// sliver_list: keyed anchoring through the viewport's correction loop.
// ---------------------------------------------------------------------------

/// A feed of 1000 rows scrolled 5000 px in, settled.
fn scrolled_feed() -> (Harness, Rc<Ledger>, Ids, ScrollController) {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((0..1000).collect()));
    let mut h = feed_harness(&ids, &ledger, &controller);
    controller.jump_to(5000.0);
    h.frames(4);
    assert_eq!(controller.offset(), 5000.0);
    (h, ledger, ids, controller)
}

#[test]
fn prepending_three_rows_while_scrolled_keeps_the_anchored_row_in_place() {
    let (mut h, ledger, ids, controller) = scrolled_feed();
    painted_frame(&mut h, &ledger);
    let (anchored, top) = first_whole_row(&ledger);
    let corrections = h.list(|l| l.scroll_corrections());

    ids.borrow_mut().splice(0..0, [10_000, 10_001, 10_002]);
    painted_frame(&mut h, &ledger);
    assert_eq!(
        h.list(|l| l.scroll_corrections()),
        corrections + 1,
        "one correction, through the viewport's loop"
    );
    assert_eq!(
        controller.offset(),
        5000.0 + 3.0 * ESTIMATE,
        "the position moved by the prepended rows' estimate"
    );
    assert!(
        (ledger.top_of(anchored) - top).abs() < 1e-6,
        "the row held still"
    );

    // Converged: nothing corrects or moves on the frames after.
    h.frames(3);
    painted_frame(&mut h, &ledger);
    assert_eq!(h.list(|l| l.scroll_corrections()), corrections + 1);
    assert!((ledger.top_of(anchored) - top).abs() < 1e-6);
    assert!(
        !ledger.live.borrow().contains(&10_000),
        "the prepended rows are above the window"
    );
}

#[test]
fn removing_rows_above_the_anchor_corrects_likewise() {
    let (mut h, ledger, ids, controller) = scrolled_feed();
    painted_frame(&mut h, &ledger);
    let (anchored, top) = first_whole_row(&ledger);
    let corrections = h.list(|l| l.scroll_corrections());

    // Two rows far above the window.
    ids.borrow_mut().drain(10..12);
    painted_frame(&mut h, &ledger);
    assert_eq!(h.list(|l| l.scroll_corrections()), corrections + 1);
    assert_eq!(controller.offset(), 5000.0 - 2.0 * ESTIMATE);
    assert!((ledger.top_of(anchored) - top).abs() < 1e-6);

    // A measured row inside the cache region above the viewport.
    let above = *ledger
        .live
        .borrow()
        .iter()
        .find(|id| **id < anchored)
        .expect("a cached row above the anchor");
    let index = ids
        .borrow()
        .iter()
        .position(|id| *id == above)
        .expect("in the data");
    ids.borrow_mut().remove(index);
    painted_frame(&mut h, &ledger);
    assert_eq!(h.list(|l| l.scroll_corrections()), corrections + 2);
    assert!((ledger.top_of(anchored) - top).abs() < 1e-6);
    assert!(
        !ledger.live.borrow().contains(&above),
        "the removed row is gone"
    );
}

#[test]
fn a_mutation_on_both_sides_of_the_anchor_in_one_frame_is_anchored_exactly() {
    let (mut h, ledger, ids, controller) = scrolled_feed();
    painted_frame(&mut h, &ledger);
    let (anchored, top) = first_whole_row(&ledger);

    // Three rows above, five below — the net item-count change (8) is not
    // the anchor's own shift (3).
    {
        let mut ids = ids.borrow_mut();
        ids.splice(0..0, [20_000, 20_001, 20_002]);
        ids.extend(30_000..30_005);
    }
    painted_frame(&mut h, &ledger);
    assert_eq!(controller.offset(), 5000.0 + 3.0 * ESTIMATE);
    assert!((ledger.top_of(anchored) - top).abs() < 1e-6);
}

#[test]
fn a_list_below_the_leading_edge_lets_a_prepend_push_it_down() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((0..100).collect()));
    let (d, l, c) = (Rc::clone(&ids), Rc::clone(&ledger), controller.clone());
    let mut h = Harness::new(Size::new(400.0, 600.0), move |_| {
        let header = tile(1_000_000, 100.0, &l);
        any(custom_scroll_view((sliver_box(header), feed(&d, &l))).controller(c.clone()))
    });
    painted_frame(&mut h, &ledger);
    assert_eq!(
        ledger.top_of(0),
        100.0,
        "the first row sits under the header"
    );

    ids.borrow_mut().insert(0, 500);
    painted_frame(&mut h, &ledger);
    assert_eq!(
        controller.offset(),
        0.0,
        "no correction: the header stays put"
    );
    assert_eq!(ledger.top_of(500), 100.0);
    assert_eq!(ledger.top_of(0), 100.0 + height(500));
    assert_eq!(h.list(|l| l.scroll_corrections()), 0);
}

#[test]
fn outside_a_viewport_the_list_lays_out_as_a_box_and_absorbs_its_corrections() {
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((0..100).collect()));
    let (d, l) = (Rc::clone(&ids), Rc::clone(&ledger));
    let mut h = Harness::new(Size::new(400.0, 600.0), move |_| {
        any(SizedBox::<()>(Some(400.0), Some(300.0)).child(feed(&d, &l)))
    });
    painted_frame(&mut h, &ledger);
    assert_eq!(ledger.top_of(0), 0.0);
    ids.borrow_mut().splice(0..0, [900, 901]);
    h.frames(2);
    painted_frame(&mut h, &ledger);
    assert_eq!(
        ledger.top_of(900),
        0.0,
        "a box shows the list from its start"
    );
    assert_eq!(ledger.top_of(0), height(900) + height(901));
    assert_eq!(h.bounds_of("SliverListWidget")[0].height(), 300.0);
}

// ---------------------------------------------------------------------------
// sliver_list: the channel-feed shape.
// ---------------------------------------------------------------------------

/// The "loading older…" sentinel row's id.
const LOADING: u64 = u64::MAX;

#[test]
fn the_channel_feed_shape_pages_older_rows_in_without_moving_the_reader() {
    // A chat feed: keyed by message id, an estimate, "load older" on
    // approaching the start, a sentinel row shown while a page loads, and the
    // page replacing the sentinel when it lands.
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((1000..1100).collect()));
    let fired = Rc::new(Cell::new(0u32));
    let (d, l, c, f) = (
        Rc::clone(&ids),
        Rc::clone(&ledger),
        controller.clone(),
        Rc::clone(&fired),
    );
    let mut h = Harness::new(Size::new(400.0, 600.0), move |_| {
        let loader = Rc::clone(&d);
        let f = Rc::clone(&f);
        any(custom_scroll_view(feed(&d, &l).on_near_start(
            move |_: &mut ()| {
                f.set(f.get() + 1);
                loader.borrow_mut().insert(0, LOADING);
            },
            400.0,
        ))
        .controller(c.clone()))
    });
    // Opened at the newest message.
    controller.jump_to(f64::INFINITY);
    h.frames(4);
    assert!(controller.offset() > 4000.0);
    assert_eq!(fired.get(), 0);

    // Scrolling up into the threshold asks for older messages once.
    controller.jump_to(300.0);
    h.frames(3);
    assert_eq!(fired.get(), 1);
    assert_eq!(ids.borrow()[0], LOADING, "the sentinel row is shown");
    painted_frame(&mut h, &ledger);
    let (anchored, top) = first_whole_row(&ledger);
    let offset = controller.offset();

    // The page lands: the sentinel is replaced by twenty older messages.
    {
        let mut ids = ids.borrow_mut();
        ids.remove(0);
        ids.splice(0..0, 980..1000);
    }
    painted_frame(&mut h, &ledger);
    assert_eq!(controller.offset(), offset + 19.0 * ESTIMATE);
    assert!(
        (ledger.top_of(anchored) - top).abs() < 1e-6,
        "the reader stays put"
    );
    h.frames(3);
    assert_eq!(fired.get(), 1, "answering the fire does not refire it");

    // Scrolling back up into the threshold asks again.
    controller.jump_to(100.0);
    h.frames(3);
    assert_eq!(fired.get(), 2);
}

// ---------------------------------------------------------------------------
// Semantics.
// ---------------------------------------------------------------------------

#[test]
fn the_list_is_a_list_node_carrying_its_item_count() {
    let controller = ScrollController::new();
    let ledger = Rc::new(Ledger::default());
    let ids: Ids = Rc::new(RefCell::new((0..5000).collect()));
    let h = feed_harness(&ids, &ledger, &controller);
    let update = h.root.semantics();
    let (_, node) = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == Role::List)
        .expect("a List node");
    assert_eq!(node.size_of_set(), Some(5000));
}

// ---------------------------------------------------------------------------
// The layout-skip audit.
// ---------------------------------------------------------------------------

#[test]
fn the_layout_audit_finds_no_stale_pod_across_the_scroll_script() {
    for gated in [false, true] {
        let controller = ScrollController::new();
        let ledger = Rc::new(Ledger::default());
        let ids: Ids = Rc::new(RefCell::new((0..60).collect()));
        let (c, l, d) = (controller.clone(), Rc::clone(&ledger), Rc::clone(&ids));
        let mut h = Harness::boxed(Size::new(400.0, 600.0), move |_| {
            let l1 = Rc::clone(&l);
            let rows = Rc::new(d.borrow().clone());
            let keys = Rc::clone(&rows);
            any(custom_scroll_view((
                sliver_list(
                    rows.len(),
                    move |i| ChildKey::new(keys[i]),
                    move |i| tile(rows[i], height(rows[i]), &l1),
                )
                .estimated_item_extent(ESTIMATE),
                SizedBox::<()>(None, Some(400.0)),
            ))
            .controller(c.clone()))
        });
        h.root.set_layout_audit(true);
        h.gated = gated;
        scroll_script(&mut h, &controller);
        assert!(h.frames >= 60, "the script ran {} frames", h.frames);
        assert!(controller.offset() > 0.0);

        // Keyed mutations above, below and on both sides of the anchor, each
        // corrected through the viewport.
        controller.jump_to(1500.0);
        h.frames(3);
        ids.borrow_mut().splice(0..0, [500, 501, 502]);
        h.frames(2);
        ids.borrow_mut().drain(1..3);
        h.frames(2);
        {
            let mut ids = ids.borrow_mut();
            ids.insert(0, 600);
            ids.extend([700, 701]);
        }
        h.frames(2);
        assert!(h.list_nested(|l| l.scroll_corrections()) >= 3);
        assert_eq!(
            h.root.layout_audit_mismatches() + h.gate_misses,
            0,
            "gated: {gated}"
        );
    }
}
