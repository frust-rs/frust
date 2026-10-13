//! `sliver_persistent_header` inside a `CustomScrollView` above a
//! `sliver_fixed_extent_list`: the unpinned, pinned, floating and stacked
//! geometries, the overlap the next sliver sees, the shrink offset the content
//! is built with (and its one-frame latency), semantics while pinned, and the
//! layout-skip audit.
//!
//! The frame harness and the scroll script mirror `slivers_a.rs`'s.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Curve, DiscardScene, FrameTime, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot,
    ScrollDelta, SemanticsCtx, SliverConstraints, SliverGeometry, SliverWidget, View, Widget,
    WidgetId, any,
};
use frust_text::TextContext;
use frust_widgets::{
    AnimateTo, ScrollController, SizedBox, SliverPersistentHeaderDelegate,
    SliverPersistentHeaderView, ViewportWidget, custom_scroll_view, sliver_fixed_extent_list,
    sliver_persistent_header,
};
use kurbo::{Point, Rect, Size};

const FRAME_MS: f64 = 16.0;
const MIN: f64 = 60.0;
const MAX: f64 = 200.0;
const ROW: f64 = 50.0;
const ROWS: usize = 100;
const WINDOW: Size = Size::new(400.0, 600.0);

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// What the header content a delegate built currently holds.
#[derive(Default)]
struct Seen {
    /// The shrink offset the live content was built (or last rebuilt) with.
    shrink: Cell<f64>,
    overlaps: Cell<bool>,
    /// How many times the live content was handed a different shrink offset.
    changes: Cell<u32>,
}

/// Header content: fills its constraints, carries a `Label` semantics node,
/// and records the build inputs it holds in a [`Seen`].
struct Content {
    shrink: f64,
    overlaps: bool,
    label: &'static str,
    seen: Rc<Seen>,
}

struct ContentWidget {
    label: &'static str,
}

impl View<()> for Content {
    type Element = ContentWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ContentWidget {
        self.seen.shrink.set(self.shrink);
        self.seen.overlaps.set(self.overlaps);
        ContentWidget { label: self.label }
    }

    fn rebuild(&self, prev: &Self, el: &mut ContentWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        el.label = self.label;
        self.seen.shrink.set(self.shrink);
        self.seen.overlaps.set(self.overlaps);
        if prev.shrink != self.shrink {
            self.seen.changes.set(self.seen.changes.get() + 1);
        }
        if prev.shrink != self.shrink || prev.overlaps != self.overlaps {
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

impl Widget for ContentWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.label;
        ctx.push_node(Role::Label, |node| node.set_value(label));
    }
}

/// A header from `min`..`max` whose content records into `seen`.
fn header(
    min: f64,
    max: f64,
    label: &'static str,
    seen: &Rc<Seen>,
) -> SliverPersistentHeaderView<()> {
    let seen = Rc::clone(seen);
    sliver_persistent_header(SliverPersistentHeaderDelegate::new(
        min,
        max,
        move |shrink, overlaps| Content {
            shrink,
            overlaps,
            label,
            seen: Rc::clone(&seen),
        },
    ))
}

/// A list row `ROW` px long.
struct Tile;

struct TileWidget;

impl View<()> for Tile {
    type Element = TileWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TileWidget {
        TileWidget
    }

    fn rebuild(&self, _prev: &Self, _el: &mut TileWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for TileWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, ROW))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

/// A zero-extent sliver that records the `overlap` it is laid out with — what
/// the sliver after a header sees.
struct OverlapProbe {
    overlaps: Rc<RefCell<Vec<f64>>>,
}

struct OverlapProbeWidget {
    overlaps: Rc<RefCell<Vec<f64>>>,
}

impl View<()> for OverlapProbe {
    type Element = OverlapProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> OverlapProbeWidget {
        OverlapProbeWidget {
            overlaps: Rc::clone(&self.overlaps),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _el: &mut OverlapProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for OverlapProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.min()
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }
}

impl SliverWidget for OverlapProbeWidget {
    fn layout_sliver(&mut self, _ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        self.overlaps.borrow_mut().push(sc.overlap);
        SliverGeometry::ZERO
    }
}

fn list() -> impl View<()> {
    sliver_fixed_extent_list(ROWS, ROW, |_| Tile)
}

fn tween(ms: f64) -> AnimateTo {
    AnimateTo {
        duration_ms: ms,
        curve: Curve::EaseInOut,
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
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

    /// The boxed widget held by pod `id`.
    fn widget_of(&self, id: WidgetId) -> &dyn Any {
        let erased = (self.root.tree().pod(id).expect("a pod").widget() as &dyn Any)
            .downcast_ref::<Box<dyn Widget>>()
            .expect("an AnyView child holds a boxed widget");
        &**erased as &dyn Any
    }

    /// The root view's own viewport widget.
    fn viewport(&self) -> &ViewportWidget {
        let id = self.root.root_id().expect("root built");
        self.widget_of(id)
            .downcast_ref::<ViewportWidget>()
            .expect("the root is a ViewportWidget")
    }

    fn sliver(&self, index: usize) -> SliverGeometry {
        self.viewport().sliver_geometry(index).expect("a sliver")
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

    /// Whether a `Label` node valued `label` hangs directly under the
    /// viewport's `ScrollView` node.
    fn label_under_scroll_view(&mut self, label: &str) -> bool {
        let update = self.root.semantics();
        let (_, scroll_view) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ScrollView)
            .expect("a ScrollView node");
        update.nodes.iter().any(|(id, n)| {
            n.role() == Role::Label
                && n.value() == Some(label)
                && scroll_view.children().contains(id)
        })
    }
}

/// The drag/fling/overscroll/wheel/controller script `slivers_a.rs` runs.
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
    h.drag(Point::new(200.0, 300.0), Point::new(200.0, 420.0), 6);
    h.frames(10);
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
// Pinned.
// ---------------------------------------------------------------------------

#[test]
fn a_pinned_header_keeps_its_minimum_and_the_list_starts_at_its_layout_extent() {
    let controller = ScrollController::new();
    let seen = Rc::new(Seen::default());
    let overlaps = Rc::new(RefCell::new(Vec::new()));
    let (c, s, o) = (controller.clone(), Rc::clone(&seen), Rc::clone(&overlaps));
    let mut h = Harness::new(WINDOW, move |_| {
        any(custom_scroll_view((
            header(MIN, MAX, "Header", &s).pinned(true),
            OverlapProbe {
                overlaps: Rc::clone(&o),
            },
            list(),
        ))
        .controller(c.clone()))
    });
    let max_offset = MAX + ROWS as f64 * ROW - WINDOW.height;
    assert_eq!(h.viewport().max_offset(), max_offset);

    for offset in [0.0, 50.0, 140.25, 199.0, 200.0, 260.0, 1000.0, max_offset] {
        controller.jump_to(offset);
        h.frames(2);
        let g = h.sliver(0);
        let shrink = offset.min(MAX);
        assert!(g.paint_extent >= MIN, "at {offset}: {g:?}");
        assert_eq!(g.paint_origin, 0.0, "at {offset}");
        assert_eq!(g.paint_extent, (MAX - offset).max(MIN), "at {offset}");
        assert_eq!(g.layout_extent, (MAX - offset).max(0.0), "at {offset}");
        // The next sliver is told what the header paints past its layout extent.
        let overlap = *overlaps.borrow().last().expect("the probe laid out");
        assert!(
            close(overlap, g.paint_extent - g.layout_extent),
            "at {offset}: overlap {overlap}"
        );
        // The list starts at the header's layout extent, under the header.
        let list_bounds = h.bounds_of("SliverFixedExtentListWidget")[0];
        assert!(close(list_bounds.y0, g.layout_extent), "at {offset}");
        assert_eq!(
            h.bounds_of("SliverPersistentHeaderWidget")[0],
            Rect::new(0.0, 0.0, 400.0, g.paint_extent)
        );
        // The content was built with the shrink offset this geometry implies,
        // and laid out at the extent that shrink offset gives.
        assert!(close(seen.shrink.get(), shrink), "at {offset}");
        assert_eq!(
            h.bounds_of("ContentWidget")[0],
            Rect::new(0.0, 0.0, 400.0, MIN.max(MAX - shrink))
        );
        assert!(!seen.overlaps.get(), "a first header overlaps nothing");
    }
}

#[test]
fn semantics_of_a_pinned_header_stay_reachable_once_scrolled_past() {
    let controller = ScrollController::new();
    let (pinned, scrolling) = (Rc::new(Seen::default()), Rc::new(Seen::default()));
    let (c, p, s) = (
        controller.clone(),
        Rc::clone(&pinned),
        Rc::clone(&scrolling),
    );
    let mut h = Harness::new(WINDOW, move |_| {
        any(custom_scroll_view((
            header(MIN, MAX, "Pinned", &p).pinned(true),
            header(40.0, 80.0, "Scrolling", &s),
            list(),
        ))
        .controller(c.clone()))
    });
    assert!(h.label_under_scroll_view("Pinned"));
    assert!(h.label_under_scroll_view("Scrolling"));
    controller.jump_to(2000.0);
    h.frames(2);
    assert!(h.sliver(0).visible);
    assert!(
        h.label_under_scroll_view("Pinned"),
        "the pinned header is still on screen"
    );
    assert!(
        !h.label_under_scroll_view("Scrolling"),
        "the unpinned header scrolled away"
    );
}

#[test]
fn stacked_pinned_headers_paint_below_one_another() {
    let controller = ScrollController::new();
    let (a, b) = (Rc::new(Seen::default()), Rc::new(Seen::default()));
    let overlaps = Rc::new(RefCell::new(Vec::new()));
    let (c, sa, sb, o) = (
        controller.clone(),
        Rc::clone(&a),
        Rc::clone(&b),
        Rc::clone(&overlaps),
    );
    let mut h = Harness::new(WINDOW, move |_| {
        any(custom_scroll_view((
            header(50.0, 100.0, "A", &sa).pinned(true),
            header(40.0, 40.0, "B", &sb).pinned(true),
            OverlapProbe {
                overlaps: Rc::clone(&o),
            },
            list(),
        ))
        .controller(c.clone()))
    });
    controller.jump_to(500.0);
    h.frames(2);
    let (ga, gb) = (h.sliver(0), h.sliver(1));
    assert_eq!((ga.paint_extent, ga.layout_extent), (50.0, 0.0));
    assert_eq!(
        (gb.paint_origin, gb.paint_extent, gb.layout_extent),
        (50.0, 40.0, 0.0)
    );
    assert_eq!(*overlaps.borrow().last().expect("probe"), 90.0);
    let headers = h.bounds_of("SliverPersistentHeaderWidget");
    assert_eq!(headers[0], Rect::new(0.0, 0.0, 400.0, 50.0));
    assert_eq!(headers[1], Rect::new(0.0, 50.0, 400.0, 90.0));
    assert!(b.overlaps.get(), "B paints over content scrolled under A");
    assert_eq!(h.bounds_of("SliverFixedExtentListWidget")[0].y0, 0.0);
}

// ---------------------------------------------------------------------------
// Unpinned.
// ---------------------------------------------------------------------------

#[test]
fn an_unpinned_header_shrinks_then_scrolls_away_like_a_box() {
    let controller = ScrollController::new();
    let seen = Rc::new(Seen::default());
    let (c, s) = (controller.clone(), Rc::clone(&seen));
    let mut h = Harness::new(WINDOW, move |_| {
        any(custom_scroll_view((header(MIN, MAX, "Header", &s), list())).controller(c.clone()))
    });
    let g = h.sliver(0);
    assert_eq!(
        (g.scroll_extent, g.paint_extent, g.layout_extent),
        (MAX, MAX, MAX)
    );

    controller.jump_to(100.0);
    h.frames(2);
    let g = h.sliver(0);
    assert_eq!((g.paint_extent, g.layout_extent), (100.0, 100.0));
    assert_eq!(
        h.bounds_of("ContentWidget")[0],
        Rect::new(0.0, 0.0, 400.0, 100.0)
    );
    assert_eq!(h.bounds_of("SliverFixedExtentListWidget")[0].y0, 100.0);
    assert_eq!(seen.shrink.get(), 100.0);

    // Past `max − min` the content stops shrinking and is pushed off instead.
    controller.jump_to(170.0);
    h.frames(2);
    assert_eq!(h.sliver(0).paint_extent, 30.0);
    assert_eq!(
        h.bounds_of("ContentWidget")[0],
        Rect::new(0.0, -30.0, 400.0, 30.0)
    );

    controller.jump_to(300.0);
    h.frames(2);
    let g = h.sliver(0);
    assert!(!g.visible);
    assert_eq!(g.layout_extent, 0.0);
    assert_eq!(h.bounds_of("SliverFixedExtentListWidget")[0].y0, 0.0);
    assert_eq!(seen.shrink.get(), MAX);
}

// ---------------------------------------------------------------------------
// Floating.
// ---------------------------------------------------------------------------

/// Scroll far down, then press and drag back 40 px, still holding; returns the
/// harness, the scroll position, and the header's paint extent mid-drag.
fn reverse_drag_after_scrolling_far(floating: bool) -> (Harness, Rc<Seen>, f64, f64) {
    let controller = ScrollController::new();
    let seen = Rc::new(Seen::default());
    let (c, s) = (controller.clone(), Rc::clone(&seen));
    let mut h = Harness::new(WINDOW, move |_| {
        any(
            custom_scroll_view((header(MIN, MAX, "Header", &s).floating(floating), list()))
                .controller(c.clone()),
        )
    });
    controller.jump_to(1500.0);
    h.frames(3);
    assert_eq!(h.sliver(0).paint_extent, 0.0);
    h.pointer(PointerPhase::Down, Point::new(200.0, 300.0));
    h.frame();
    for step in 1..=8 {
        h.pointer(
            PointerPhase::Move,
            Point::new(200.0, 300.0 + 5.0 * f64::from(step)),
        );
        h.frame();
    }
    // One more frame so the content catches up with the last layout.
    h.frame();
    let offset = controller.offset();
    let paint = h.sliver(0).paint_extent;
    (h, seen, offset, paint)
}

#[test]
fn a_floating_header_reappears_on_a_small_reverse_scroll_before_reaching_the_top() {
    let (mut h, seen, offset, paint) = reverse_drag_after_scrolling_far(true);
    let scrolled_back = 1500.0 - offset;
    assert!(
        scrolled_back > 0.0 && scrolled_back <= 40.0,
        "a small reverse scroll, far from the top: {offset}"
    );
    assert!(
        close(paint, scrolled_back),
        "revealed by exactly the reverse scroll: {paint} vs {scrolled_back}"
    );
    let g = h.sliver(0);
    assert_eq!(
        g.layout_extent, 0.0,
        "the content does not make room for it"
    );
    assert!(
        seen.overlaps.get(),
        "content scrolls under the floated header"
    );
    assert!(close(seen.shrink.get(), MAX - paint));
    let content = h.bounds_of("ContentWidget")[0];
    assert!(
        close(content.y1, paint),
        "the content ends at the paint extent"
    );

    // Released, it stays where it is (no snap).
    h.pointer(PointerPhase::Up, Point::new(200.0, 340.0));
    h.frames(40);
    let resting = h.sliver(0).paint_extent;
    assert!(resting >= paint && resting < MAX, "{resting}");

    // Scrolling on toward the end takes it away again with the content.
    h.drag(Point::new(200.0, 400.0), Point::new(200.0, 300.0), 6);
    h.frames(40);
    assert!(h.sliver(0).paint_extent < resting);
}

#[test]
fn a_non_floating_header_stays_away_on_the_same_reverse_scroll() {
    let (_h, _seen, offset, paint) = reverse_drag_after_scrolling_far(false);
    assert!(offset > 1400.0 && offset < 1500.0);
    assert_eq!(paint, 0.0);
}

#[test]
fn a_floating_pinned_header_never_paints_below_its_minimum() {
    let controller = ScrollController::new();
    let seen = Rc::new(Seen::default());
    let (c, s) = (controller.clone(), Rc::clone(&seen));
    let mut h = Harness::new(WINDOW, move |_| {
        any(custom_scroll_view((
            header(MIN, MAX, "Header", &s).pinned(true).floating(true),
            list(),
        ))
        .controller(c.clone()))
    });
    controller.jump_to(1500.0);
    h.frames(3);
    let g = h.sliver(0);
    assert_eq!((g.paint_extent, g.layout_extent), (MIN, 0.0));
    h.drag(Point::new(200.0, 300.0), Point::new(200.0, 400.0), 8);
    h.frames(40);
    let g = h.sliver(0);
    assert!(g.paint_extent > MIN, "floated open past its minimum");
    assert!(controller.offset() > 1000.0);
}

// ---------------------------------------------------------------------------
// The shrink offset reaches the content at the next rebuild.
// ---------------------------------------------------------------------------

#[test]
fn the_content_follows_the_geometry_one_frame_later_and_ignores_sub_half_pixel_moves() {
    let controller = ScrollController::new();
    let seen = Rc::new(Seen::default());
    let (c, s) = (controller.clone(), Rc::clone(&seen));
    let mut h = Harness::new(WINDOW, move |_| {
        any(
            custom_scroll_view((header(MIN, MAX, "Header", &s).pinned(true), list()))
                .controller(c.clone()),
        )
    });
    assert_eq!(seen.shrink.get(), 0.0);

    // The jump applies at this frame's layout, after its rebuild ran.
    controller.jump_to(100.0);
    h.frame();
    assert_eq!(h.sliver(0).paint_extent, 100.0);
    assert_eq!(h.bounds_of("ContentWidget")[0].height(), 100.0);
    assert_eq!(seen.shrink.get(), 0.0, "the content is one frame behind");
    h.frame();
    assert_eq!(seen.shrink.get(), 100.0);
    let changes = seen.changes.get();

    // Below the tolerance: the content keeps the previous build.
    controller.jump_to(100.3);
    h.frames(3);
    assert!(close(h.bounds_of("ContentWidget")[0].height(), 99.7));
    assert_eq!(seen.shrink.get(), 100.0);
    assert_eq!(seen.changes.get(), changes);

    // At the tolerance: rebuilt.
    controller.jump_to(100.6);
    h.frames(2);
    assert_eq!(seen.shrink.get(), 100.6);
    assert_eq!(seen.changes.get(), changes + 1);

    // Back to the top: the end of the range is always rebuilt exactly.
    controller.jump_to(0.3);
    h.frames(2);
    controller.jump_to(0.0);
    h.frames(2);
    assert_eq!(seen.shrink.get(), 0.0);
}

// ---------------------------------------------------------------------------
// The layout-skip audit.
// ---------------------------------------------------------------------------

#[test]
fn the_layout_audit_finds_no_stale_pod_across_the_scroll_script() {
    for gated in [false, true] {
        let controller = ScrollController::new();
        let seen = Rc::new(Seen::default());
        let (c, s) = (controller.clone(), Rc::clone(&seen));
        let mut h = Harness::boxed(WINDOW, move |_| {
            any(custom_scroll_view((
                header(MIN, MAX, "Pinned", &s).pinned(true),
                header(30.0, 90.0, "Floating", &s).floating(true),
                header(20.0, 70.0, "Both", &s).floating(true).pinned(true),
                header(10.0, 50.0, "Scrolling", &s),
                list(),
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
