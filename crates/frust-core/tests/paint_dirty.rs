//! Per-pod paint dirtiness (`ChildPod::is_paint_dirty`, `PaintCtx::subtree_clean`):
//! every dirty source marks the whole path from the source to the root, a pass
//! that paints a subtree clears it, the kill switch
//! (`RenderRoot::set_retained_layers(false)`) makes the query answer `false`,
//! and a global paint input (theme, surface translucency) invalidates everything.
//!
//! Driven through a real `RenderRoot` with the same instrumented widgets as
//! `layout_skip.rs`; the container logs `subtree_clean` for each child pod
//! before painting it, the way a retained-layer boundary would.

use std::cell::RefCell;
use std::collections::HashMap;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Component, DiscardScene, EventCtx,
    EventResult, FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, RenderRoot, View, Widget, any, component,
};
use kurbo::{Point, Size};
use reactive_graph::owner::Owner;

// ---------------------------------------------------------------------------
// Instrumentation: a per-thread layout log and per-widget behaviour switches.
// Every test runs on its own thread, so the thread-locals never leak between
// tests.
// ---------------------------------------------------------------------------

/// What a leaf does when painted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PaintReq {
    Frame,
}

/// What a leaf does with a pointer `Down`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EventMode {
    /// Return `Handled`, request nothing.
    Consume,
    /// Call `EventCtx::request_layout`, return `Ignored`.
    RequestLayout,
}

thread_local! {
    static LAYOUT_LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static PAINT_REQ: RefCell<HashMap<&'static str, PaintReq>> = RefCell::new(HashMap::new());
    static CLEAN_LOG: RefCell<Vec<(&'static str, usize, bool)>> = const { RefCell::new(Vec::new()) };
    static EVENT_MODE: RefCell<HashMap<&'static str, EventMode>> = RefCell::new(HashMap::new());
}

fn log_layout(name: &'static str) {
    LAYOUT_LOG.with(|log| log.borrow_mut().push(name));
}

/// Drain the layout log, sorted, so assertions read as sets.
fn take_layouts() -> Vec<&'static str> {
    let mut names = LAYOUT_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()));
    names.sort_unstable();
    names
}

fn set_paint_req(name: &'static str, req: Option<PaintReq>) {
    PAINT_REQ.with(|map| match req {
        Some(req) => map.borrow_mut().insert(name, req),
        None => map.borrow_mut().remove(name),
    });
}

fn set_event_mode(name: &'static str, mode: EventMode) {
    EVENT_MODE.with(|map| map.borrow_mut().insert(name, mode));
}

// ---------------------------------------------------------------------------
// A counting leaf.
// ---------------------------------------------------------------------------

struct LeafWidget {
    name: &'static str,
    size: Size,
}

impl Widget for LeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout(self.name);
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        match PAINT_REQ.with(|map| map.borrow().get(self.name).copied()) {
            Some(PaintReq::Frame) => ctx.request_frame(),
            None => {}
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(pointer) = event else {
            return EventResult::Ignored;
        };
        if pointer.phase != PointerPhase::Down {
            return EventResult::Ignored;
        }
        match EVENT_MODE.with(|map| map.borrow().get(self.name).copied()) {
            Some(EventMode::Consume) => EventResult::Handled,
            Some(EventMode::RequestLayout) => {
                ctx.request_layout();
                EventResult::Ignored
            }
            None => EventResult::Ignored,
        }
    }
}

struct LeafView {
    name: &'static str,
    size: Size,
    /// Paint-only state: a change rebuilds with `PAINT` alone.
    tint: u32,
}

fn leaf(name: &'static str, w: f64) -> LeafView {
    LeafView {
        name,
        size: Size::new(w, 10.0),
        tint: 0,
    }
}

impl<S: 'static> View<S> for LeafView {
    type Element = LeafWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
        LeafWidget {
            name: self.name,
            size: self.size,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LeafWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if self.size != prev.size {
            element.size = self.size;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else if self.tint != prev.tint {
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

// ---------------------------------------------------------------------------
// A counting vertical container. Hands every child the same fixed loose
// constraints (`child_max` square), independent of its own, so a window resize
// changes no child constraint and only the epoch can explain a relayout.
// ---------------------------------------------------------------------------

struct ColWidget {
    name: &'static str,
    child_max: f64,
    children: Vec<ChildPod>,
}

impl Widget for ColWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout(self.name);
        let child_bc = BoxConstraints::loose(Size::new(self.child_max, self.child_max));
        let (mut width, mut y) = (0.0f64, 0.0);
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
            width = width.max(size.width);
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for (i, pod) in self.children.iter_mut().enumerate() {
            let clean = ctx.subtree_clean(pod);
            CLEAN_LOG.with(|log| log.borrow_mut().push((self.name, i, clean)));
            pod.paint_child(ctx, scene);
        }
    }

    /// Routes to the child under the pointer but always reports `Ignored`, like
    /// a container forwarding a broadcast: the dirty path must reach the root
    /// through the pods' own bookkeeping, not through a propagated result.
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let position = event.position();
        for pod in &mut self.children {
            if pod.contains(position) {
                pod.event_child(ctx, event);
                break;
            }
        }
        EventResult::Ignored
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for pod in &self.children {
            visitor(pod);
        }
    }
}

struct ColView<S: 'static> {
    name: &'static str,
    child_max: f64,
    children: Vec<AnyView<S>>,
}

fn col<S: 'static>(name: &'static str, child_max: f64, children: Vec<AnyView<S>>) -> ColView<S> {
    ColView {
        name,
        child_max,
        children,
    }
}

impl<S: 'static> View<S> for ColView<S> {
    type Element = ColWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ColWidget {
        ColWidget {
            name: self.name,
            child_max: self.child_max,
            children: self
                .children
                .iter()
                .map(|child| ChildPod::new(Box::new(child.build(ctx))))
                .collect(),
        }
    }

    /// A same-length reconciler spelled the way `frust-widgets`' reconcilers
    /// are: rebuild each child in its pod and mark the pod layout-dirty when the
    /// child reports `LAYOUT`.
    fn rebuild(&self, prev: &Self, element: &mut ColWidget, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        assert_eq!(
            self.children.len(),
            prev.children.len(),
            "test views keep their arity"
        );
        let mut flags = ChangeFlags::NONE;
        if self.child_max != prev.child_max {
            element.child_max = self.child_max;
            flags |= ChangeFlags::LAYOUT;
        }
        for (i, pod) in element.children.iter_mut().enumerate() {
            let boxed = pod
                .widget_mut_for_rebuild()
                .downcast_mut::<Box<dyn Widget>>()
                .expect("children are built from AnyView");
            let child_flags = self.children[i].rebuild(&prev.children[i], boxed, ctx);
            if child_flags.contains(ChangeFlags::LAYOUT) {
                pod.mark_layout_dirty();
            } else if child_flags.contains(ChangeFlags::PAINT) {
                pod.mark_paint_dirty();
            }
            flags |= child_flags;
        }
        flags
    }
}

// ---------------------------------------------------------------------------
// A component boundary in the path, so the core-owned marking
// (`ComponentView::rebuild`, `ComponentWidget::event`) is exercised too.
// ---------------------------------------------------------------------------

struct Branch {
    target_w: f64,
    target_tint: u32,
}

impl Component for Branch {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        col::<()>(
            "mid",
            100.0,
            vec![any(LeafView {
                tint: self.target_tint,
                ..leaf("target", self.target_w)
            })],
        )
    }
}

/// The app shape every test drives:
///
/// ```text
/// root (Col, the root widget — no pod, laid out every pass)
/// ├── component(Branch) ─ mid (Col) ─ target (leaf, y 0..10)
/// └── sib (Col) ─ sib_leaf (leaf, y 10..20)
/// ```
#[derive(Clone, Copy)]
struct Shape {
    /// The root's `child_max` — the constraint every root child receives.
    root_child_max: f64,
    target_w: f64,
    target_tint: u32,
    sib_tint: u32,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            root_child_max: 200.0,
            target_w: 40.0,
            target_tint: 0,
            sib_tint: 0,
        }
    }
}

fn app(shape: Shape) -> ColView<()> {
    col(
        "root",
        shape.root_child_max,
        vec![
            any(component(Branch {
                target_w: shape.target_w,
                target_tint: shape.target_tint,
            })),
            any(col::<()>(
                "sib",
                100.0,
                vec![any(LeafView {
                    tint: shape.sib_tint,
                    ..leaf("sib_leaf", 30.0)
                })],
            )),
        ],
    )
}

const WINDOW: Size = Size::new(300.0, 300.0);
/// Drain the clean log.
fn take_clean() -> Vec<(&'static str, usize, bool)> {
    CLEAN_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

const EVERYTHING: [&str; 5] = ["mid", "root", "sib", "sib_leaf", "target"];

struct Harness {
    root: RenderRoot<(), ColView<()>>,
    _owner: Owner,
}

impl Harness {
    /// Build, lay out and paint the default shape once, then drain the log and
    /// the pending flags, so each test starts from a fully clean tree.
    fn new() -> Self {
        let owner = Owner::new();
        owner.set();
        let mut harness = Self {
            root: RenderRoot::new(),
            _owner: owner,
        };
        harness.rebuild(Shape::default());
        harness.layout();
        harness.paint();
        assert_eq!(
            take_layouts(),
            EVERYTHING,
            "a fresh tree lays out every widget"
        );
        harness.root.take_change_flags();
        harness
    }

    fn rebuild(&mut self, shape: Shape) -> ChangeFlags {
        self.root.rebuild(&mut |_s: &mut ()| app(shape), &mut ())
    }

    fn layout(&mut self) {
        self.root.layout(WINDOW);
    }

    fn paint(&mut self) {
        self.root.paint(&mut DiscardScene, FrameTime::ZERO);
    }

    fn down_at(&mut self, y: f64) {
        self.root.event(
            &mut (),
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(5.0, y),
                button: PointerButton::Primary,
            }),
        );
    }

    /// The y of the top edge of every currently paint-dirty node, in the
    /// pre-order `RenderRoot::inspect` reports. The target path sits at
    /// `y == 0`, the sibling path at `y == 10`.
    fn dirty_ys(&self) -> Vec<f64> {
        let nodes = self.root.inspect();
        let dirty = self.root.inspect_paint_dirty();
        assert_eq!(nodes.len(), dirty.len(), "the dirty walk zips with inspect");
        nodes
            .iter()
            .zip(&dirty)
            .filter_map(|(node, (id, is_dirty))| {
                assert_eq!(node.id, *id, "same pre-order, same ids");
                is_dirty.then_some(node.bounds.y0)
            })
            .collect()
    }
}

// The target path (component pod, mid, target) sits at y 0; the sibling path
// (sib, sib_leaf) at y 10.
const TARGET_PATH: usize = 3;
const SIB_PATH: usize = 2;

fn assert_path(ys: &[f64], count: usize, sibling: bool) {
    assert_eq!(ys.len(), count, "dirty path length: {ys:?}");
    let want = if sibling { 10.0 } else { 0.0 };
    assert!(ys.iter().all(|y| *y == want), "wrong subtree dirty: {ys:?}");
}

#[test]
fn a_fully_painted_tree_is_clean() {
    let h = Harness::new();
    assert!(h.dirty_ys().is_empty(), "a painted tree is paint-clean");
}

#[test]
fn a_leaf_paint_rebuild_dirties_exactly_its_path() {
    let mut h = Harness::new();
    let flags = h.rebuild(Shape {
        sib_tint: 1,
        ..Shape::default()
    });
    assert_eq!(flags, ChangeFlags::PAINT);
    assert_path(&h.dirty_ys(), SIB_PATH, true);
    // A paint pass clears it again.
    h.paint();
    assert!(h.dirty_ys().is_empty());
}

#[test]
fn a_layout_rebuild_dirties_paint_on_its_path() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        target_w: 60.0,
        ..Shape::default()
    });
    assert_path(&h.dirty_ys(), TARGET_PATH, false);
}

#[test]
fn a_paint_time_frame_request_dirties_the_path_next_frame() {
    let mut h = Harness::new();
    set_paint_req("target", Some(PaintReq::Frame));
    h.paint();
    assert_path(&h.dirty_ys(), TARGET_PATH, false);
    take_clean();
    h.paint();
    let log = take_clean();
    assert!(
        log.contains(&("sib", 0, true)) && log.contains(&("root", 1, true)),
        "the sibling subtree stays clean: {log:?}"
    );
    assert!(
        log.contains(&("mid", 0, false)) && log.contains(&("root", 0, false)),
        "the animating path reports dirty: {log:?}"
    );
    // The request stops: the landing paint is still dirty, the one after is clean.
    set_paint_req("target", None);
    h.paint();
    assert_path(&h.dirty_ys(), TARGET_PATH, false);
    h.paint();
    assert!(h.dirty_ys().is_empty());
}

#[test]
fn a_clean_subtree_reports_clean_after_a_pass_that_painted_it() {
    let mut h = Harness::new();
    take_clean();
    h.paint();
    let log = take_clean();
    assert!(!log.is_empty());
    assert!(log.iter().all(|(_, _, clean)| *clean), "{log:?}");
}

#[test]
fn the_kill_switch_makes_subtree_clean_false() {
    let mut h = Harness::new();
    assert!(h.root.retained_layers(), "on by default");
    h.root.set_retained_layers(false);
    take_clean();
    h.paint();
    let log = take_clean();
    assert!(!log.is_empty());
    assert!(log.iter().all(|(_, _, clean)| !*clean), "{log:?}");
    h.root.set_retained_layers(true);
    h.paint();
    assert!(take_clean().iter().all(|(_, _, clean)| *clean));
}

#[test]
fn layout_running_dirties_paint_and_a_skipped_layout_does_not() {
    let mut h = Harness::new();
    h.layout();
    assert!(h.dirty_ys().is_empty(), "a skipped layout dirties nothing");
    h.root.set_theme(Box::new(()));
    h.layout();
    assert_eq!(h.dirty_ys().len(), 5, "every pod's layout ran");
    h.paint();
    assert!(h.dirty_ys().is_empty());
}

#[test]
fn a_surface_translucency_flip_invalidates_every_retained_subtree() {
    let mut h = Harness::new();
    h.root.set_surface_translucent(true);
    // No pod is flagged (nothing was laid out), but the epoch covers them all.
    take_clean();
    h.paint();
    let log = take_clean();
    assert!(!log.is_empty());
    assert!(log.iter().all(|(_, _, clean)| !*clean), "{log:?}");
    take_clean();
    h.paint();
    assert!(take_clean().iter().all(|(_, _, clean)| *clean));
}

#[test]
fn a_consumed_event_dirties_the_path() {
    let mut h = Harness::new();
    set_event_mode("target", EventMode::Consume);
    h.down_at(5.0);
    assert_path(&h.dirty_ys(), TARGET_PATH, false);
}

#[test]
fn event_request_layout_dirties_the_path() {
    let mut h = Harness::new();
    set_event_mode("sib_leaf", EventMode::RequestLayout);
    h.down_at(15.0);
    assert_path(&h.dirty_ys(), SIB_PATH, true);
}

/// The component's own child pod (`mid`) is rebuilt inside `ComponentView::rebuild`
/// rather than by a reconciler: a PAINT-only rebuild below it must still mark it
/// paint-dirty (and only paint), or a retained layer there would draw stale.
#[test]
fn a_paint_only_component_child_rebuild_marks_paint_not_layout() {
    let mut h = Harness::new();
    let flags = h.rebuild(Shape {
        target_tint: 1,
        ..Shape::default()
    });
    assert_eq!(flags, ChangeFlags::PAINT);
    // component pod, its child pod (mid) and the target pod: the whole path,
    // the sibling untouched.
    assert_path(&h.dirty_ys(), TARGET_PATH, false);
    h.layout();
    assert_eq!(
        take_layouts(),
        ["root"],
        "PAINT alone dirties no layout: only the pod-less root runs"
    );
    h.paint();
    assert!(h.dirty_ys().is_empty());
}

#[test]
fn a_layout_component_child_rebuild_still_marks_layout_and_paint() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        target_w: 60.0,
        ..Shape::default()
    });
    assert_path(&h.dirty_ys(), TARGET_PATH, false);
    h.layout();
    assert_eq!(take_layouts(), ["mid", "root", "target"]);
}

// ---------------------------------------------------------------------------
// Damage (`RenderRoot::damage_rect`): what a paint pass painted differently,
// derived from the same dirtiness. The tree's absolute geometry: the target
// leaf at (0, 0)-(40, 10), the sibling leaf at (0, 10)-(30, 20).
// ---------------------------------------------------------------------------

use frust_core::widget::PaintDamage;
use kurbo::Rect;

const TARGET_RECT: Rect = Rect::new(0.0, 0.0, 40.0, 10.0);
const SIB_LEAF_RECT: Rect = Rect::new(0.0, 10.0, 30.0, 20.0);

#[test]
fn the_first_paint_damages_everything() {
    let h = Harness::new();
    assert_eq!(h.root.damage_rect(), PaintDamage::Full);
}

#[test]
fn a_frame_that_changed_nothing_is_clean() {
    let mut h = Harness::new();
    h.layout();
    h.paint();
    assert_eq!(h.root.damage_rect(), PaintDamage::Clean);
}

#[test]
fn a_one_leaf_paint_change_damages_exactly_that_leaf() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        sib_tint: 1,
        ..Shape::default()
    });
    h.layout();
    h.paint();
    // The path above the leaf is dirty too, but its dirt is the leaf's.
    assert_eq!(h.root.damage_rect(), PaintDamage::Rect(SIB_LEAF_RECT));
    // And the pass after it is clean again.
    h.layout();
    h.paint();
    assert_eq!(h.root.damage_rect(), PaintDamage::Clean);
}

#[test]
fn a_leaf_behind_a_component_boundary_damages_exactly_that_leaf() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        target_tint: 1,
        ..Shape::default()
    });
    h.layout();
    h.paint();
    assert_eq!(h.root.damage_rect(), PaintDamage::Rect(TARGET_RECT));
}

#[test]
fn a_layout_change_damages_the_old_and_new_bounds_of_every_pod_it_laid_out() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        target_w: 60.0,
        ..Shape::default()
    });
    h.layout();
    h.paint();
    // The target grew from 40 to 60 wide; its path laid out with it, and the
    // sibling neither moved nor changed.
    assert_eq!(
        h.root.damage_rect(),
        PaintDamage::Rect(Rect::new(0.0, 0.0, 60.0, 10.0))
    );
}

#[test]
fn a_global_paint_input_damages_everything() {
    let mut h = Harness::new();
    h.root.set_surface_translucent(true);
    h.paint();
    assert_eq!(h.root.damage_rect(), PaintDamage::Full);
    h.paint();
    assert_eq!(h.root.damage_rect(), PaintDamage::Clean);
}

/// Paint `h` the way a shell does, under its device `scale`.
fn paint_scaled(h: &mut Harness, scale: f64) {
    let mut scene = frust_scene::Scene::new();
    let mut builder = frust_scene::SceneBuilder::new(&mut scene);
    builder.push_transform(kurbo::Affine::scale(scale));
    h.root.paint(&mut builder, FrameTime::ZERO);
}

#[test]
fn damage_is_reported_in_the_scenes_device_space() {
    let mut h = Harness::new();
    // The first pass under a new device scale lands every pod somewhere else:
    // each one damages where it was and where it is.
    paint_scaled(&mut h, 2.0);
    assert_eq!(
        h.root.damage_rect(),
        PaintDamage::Rect(Rect::new(0.0, 0.0, 80.0, 40.0))
    );
    // Under the settled scale, a leaf's change comes back through it.
    h.rebuild(Shape {
        sib_tint: 1,
        ..Shape::default()
    });
    h.layout();
    paint_scaled(&mut h, 2.0);
    assert_eq!(
        h.root.damage_rect(),
        PaintDamage::Rect(Rect::new(0.0, 20.0, 60.0, 40.0))
    );
}

#[test]
fn damage_publishing_is_opt_in_and_drained_once() {
    let mut h = Harness::new();
    // Off (the default): a paint publishes nothing.
    assert_eq!(frust_core::app::take_published_damage(), None);
    frust_core::app::set_damage_publishing(true);
    h.rebuild(Shape {
        sib_tint: 1,
        ..Shape::default()
    });
    h.layout();
    h.paint();
    // Other tests in this binary may paint while publishing is on, so only
    // the presence of a union is asserted, not its exact extent.
    assert!(frust_core::app::take_published_damage().is_some());
    frust_core::app::set_damage_publishing(false);
    h.paint();
    assert_eq!(frust_core::app::take_published_damage(), None);
}

#[test]
fn paint_damage_unions_with_full_absorbing_and_clean_as_identity() {
    let a = PaintDamage::Rect(TARGET_RECT);
    let b = PaintDamage::Rect(SIB_LEAF_RECT);
    assert_eq!(PaintDamage::Clean.union(a), a);
    assert_eq!(a.union(PaintDamage::Clean), a);
    assert_eq!(a.union(PaintDamage::Full), PaintDamage::Full);
    assert_eq!(
        a.union(b),
        PaintDamage::Rect(Rect::new(0.0, 0.0, 40.0, 20.0))
    );
}

// ---------------------------------------------------------------------------
// Hover and focus links: both are read at paint time from links the root moves
// without visiting the pod that loses one, so a link change makes every
// subtree unclean on the next paint — and a pointer move that keeps the link
// where it was moves nothing.
// ---------------------------------------------------------------------------

/// A leaf that claims hover on every uncaptured `Move` over it and takes focus
/// on a `Down` over it — the two paint-time links a retained page could freeze.
struct LinkLeafWidget {
    size: Size,
}

impl Widget for LinkLeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) if p.phase == PointerPhase::Move => {
                ctx.claim_hover();
                EventResult::Ignored
            }
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

struct LinkLeaf(f64);

impl<S: 'static> View<S> for LinkLeaf {
    type Element = LinkLeafWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LinkLeafWidget {
        LinkLeafWidget {
            size: Size::new(self.0, 10.0),
        }
    }

    fn rebuild(&self, _: &Self, _: &mut LinkLeafWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// ```text
/// links (Col, the root widget)
/// ├── held (Col) ─ inner leaf (y 0..10)   <- stands for a boundary's child
/// └── outer leaf (y 10..20)
/// ```
fn links_app() -> ColView<()> {
    col(
        "links",
        200.0,
        vec![
            any(col::<()>("held", 100.0, vec![any(LinkLeaf(40.0))])),
            any(LinkLeaf(30.0)),
        ],
    )
}

struct LinkHarness {
    root: RenderRoot<(), ColView<()>>,
    _owner: Owner,
}

impl LinkHarness {
    fn new() -> Self {
        let owner = Owner::new();
        owner.set();
        let mut h = Self {
            root: RenderRoot::new(),
            _owner: owner,
        };
        h.root.rebuild(&mut |_: &mut ()| links_app(), &mut ());
        h.root.layout(WINDOW);
        h.root.paint(&mut DiscardScene, FrameTime::ZERO);
        h
    }

    fn pointer(&mut self, phase: PointerPhase, y: f64) {
        self.root.event(
            &mut (),
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(5.0, y),
                button: PointerButton::Primary,
            }),
        );
    }

    /// Lay out and paint, answering whether the root found the `held` pod
    /// clean before painting it — what a boundary over it would ask.
    fn held_clean(&mut self) -> bool {
        take_clean();
        self.root.layout(WINDOW);
        self.root.paint(&mut DiscardScene, FrameTime::ZERO);
        let log = take_clean();
        log.iter()
            .find(|(name, i, _)| *name == "links" && *i == 0)
            .map(|(_, _, clean)| *clean)
            .expect("the root painted the held pod")
    }
}

#[test]
fn a_hover_moving_off_a_held_subtree_makes_it_unclean_and_a_same_claimant_move_does_not() {
    let mut h = LinkHarness::new();
    assert!(h.held_clean(), "settled");

    // Hover begins inside the held subtree.
    h.pointer(PointerPhase::Move, 5.0);
    assert!(
        !h.held_clean(),
        "a hover beginning re-records every boundary"
    );
    assert!(h.held_clean(), "and the paint after it is clean again");

    // The pointer moves within the same claimant: the epoch advanced, the link
    // did not.
    h.pointer(PointerPhase::Move, 6.0);
    assert!(
        h.held_clean(),
        "a move that keeps the claimant changes nothing"
    );

    // The hover moves to a claimant OUTSIDE the held subtree: the Move routes
    // along the new hit path only, never visiting the held pod.
    h.pointer(PointerPhase::Move, 15.0);
    assert!(
        !h.held_clean(),
        "the held subtree painted its claimant hovered; that page must not be drawn now"
    );
    assert!(h.held_clean());

    // Moving within the new claimant leaves it clean.
    h.pointer(PointerPhase::Move, 16.0);
    assert!(h.held_clean());

    // Hover ending (the pointer over nothing that claims) is a link change too.
    h.pointer(PointerPhase::Move, 150.0);
    assert!(!h.held_clean(), "a hover that ended re-records");
    h.pointer(PointerPhase::Move, 160.0);
    assert!(h.held_clean(), "and no link standing is no change either");
}

#[test]
fn a_focus_moving_off_a_held_subtree_makes_it_unclean() {
    let mut h = LinkHarness::new();
    h.pointer(PointerPhase::Down, 5.0);
    h.pointer(PointerPhase::Up, 5.0);
    assert!(!h.held_clean(), "focus taken inside it");
    assert!(h.held_clean());

    // Focus moves to the outer leaf; the held pod is not on that dispatch's path.
    h.pointer(PointerPhase::Down, 15.0);
    h.pointer(PointerPhase::Up, 15.0);
    assert!(
        !h.held_clean(),
        "the held subtree painted its leaf focused; that page must not be drawn now"
    );
    assert!(h.held_clean());

    // A dispatch that records no focus claim (the housekeeping broadcast)
    // leaves the session, and so the link, standing.
    h.root.event(&mut (), &InputEvent::Housekeeping);
    assert!(
        h.held_clean(),
        "a dispatch that moves no focus changes nothing"
    );
}
