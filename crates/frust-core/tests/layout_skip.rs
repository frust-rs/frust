//! The per-pod layout skip (`ChildPod::layout_child`): a clean pod under equal
//! constraints and an unchanged layout epoch is not laid out again, and every
//! dirty source — a rebuild returning `LAYOUT`, a paint-time `request_layout` /
//! `request_frame`, a consumed event, `EventCtx::request_layout` — marks the
//! whole path from the source up to the root, and reaches the root's pending
//! `LAYOUT`, so a shell running layout only on `take_change_flags()` lays the
//! marked path out.
//!
//! Driven through a real `RenderRoot` with counting widgets: each widget logs
//! its name on every `Widget::layout` call, so a test asserts exactly which
//! widgets a pass laid out.

use std::cell::RefCell;
use std::collections::HashMap;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Component, DiscardScene, EventCtx,
    EventResult, FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, RenderRoot, View, Widget, WindowEdgeInsets, WindowInsets, any,
    component,
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
    /// A paced (`CosmeticLoop`) frame request — a decorative loop.
    Paced,
    Layout,
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
            Some(PaintReq::Paced) => ctx.request_frame_paced(),
            Some(PaintReq::Layout) => ctx.request_layout(),
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
        for pod in &mut self.children {
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
}

impl Component for Branch {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        col::<()>("mid", 100.0, vec![any(leaf("target", self.target_w))])
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
    sib_tint: u32,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            root_child_max: 200.0,
            target_w: 40.0,
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

    /// The short widget type names of every currently layout-dirty node, in
    /// the pre-order `RenderRoot::inspect` reports.
    fn dirty_nodes(&self) -> Vec<&'static str> {
        let nodes = self.root.inspect();
        let dirty = self.root.inspect_layout_dirty();
        assert_eq!(nodes.len(), dirty.len(), "the dirty walk zips with inspect");
        nodes
            .iter()
            .zip(&dirty)
            .filter_map(|(node, (id, is_dirty))| {
                assert_eq!(node.id, *id, "same pre-order, same ids");
                is_dirty.then_some(short(node.type_name))
            })
            .collect()
    }
}

/// The last path segment of a type name, with generics dropped
/// (`a::b::ColWidget<..>` -> `ColWidget`).
fn short(type_name: &'static str) -> &'static str {
    let head = type_name.split('<').next().unwrap_or(type_name);
    head.rsplit("::").next().unwrap_or(head)
}

// ---------------------------------------------------------------------------
// (i) A clean pod under equal constraints is not laid out.
// ---------------------------------------------------------------------------

#[test]
fn a_clean_pod_under_equal_constraints_is_not_laid_out() {
    let mut h = Harness::new();
    assert!(h.dirty_nodes().is_empty(), "a laid-out tree is clean");

    // An identical rebuild reports nothing, so nothing below the root lays out.
    assert_eq!(h.rebuild(Shape::default()), ChangeFlags::NONE);
    h.layout();
    assert_eq!(
        take_layouts(),
        ["root"],
        "only the pod-less root re-lays out"
    );

    // A PAINT-only rebuild does not dirty layout either.
    let flags = h.rebuild(Shape {
        sib_tint: 1,
        ..Shape::default()
    });
    assert_eq!(flags, ChangeFlags::PAINT);
    assert!(h.dirty_nodes().is_empty(), "PAINT alone dirties no pod");
    h.layout();
    assert_eq!(take_layouts(), ["root"]);
}

// ---------------------------------------------------------------------------
// (ii) Changed constraints lay out.
// ---------------------------------------------------------------------------

#[test]
fn changed_constraints_lay_out() {
    let mut h = Harness::new();
    // The root hands its children new constraints; their pods are clean, but
    // the constraints differ, so both children lay out — and their own
    // children, still handed the same constraints, do not.
    h.rebuild(Shape {
        root_child_max: 150.0,
        ..Shape::default()
    });
    assert!(h.dirty_nodes().is_empty(), "no child reported LAYOUT");
    h.layout();
    assert_eq!(take_layouts(), ["mid", "root", "sib"]);
}

// ---------------------------------------------------------------------------
// (iii) An epoch bump lays out.
// ---------------------------------------------------------------------------

#[test]
fn an_epoch_bump_lays_out_everything() {
    let mut h = Harness::new();

    // A theme swap.
    let before = h.root.layout_epoch();
    h.root.set_theme(Box::new(()));
    assert_ne!(h.root.layout_epoch(), before, "set_theme bumps the epoch");
    h.layout();
    assert_eq!(take_layouts(), EVERYTHING);
    h.layout();
    assert_eq!(take_layouts(), ["root"], "the bump is consumed by one pass");

    // An inset change — and an unchanged inset push is not one.
    let insets = WindowInsets {
        view_padding: WindowEdgeInsets {
            top: 24.0,
            ..WindowEdgeInsets::default()
        },
        ..WindowInsets::default()
    };
    h.root.set_insets(insets);
    h.layout();
    assert_eq!(take_layouts(), EVERYTHING);
    h.root.set_insets(insets);
    h.layout();
    assert_eq!(take_layouts(), ["root"], "an unchanged inset bumps nothing");

    // A window resize: every child constraint here is independent of the
    // window, so only the epoch explains the relayout.
    h.root.layout(Size::new(320.0, 300.0));
    assert_eq!(take_layouts(), EVERYTHING);
    h.root.layout(Size::new(320.0, 300.0));
    assert_eq!(
        take_layouts(),
        ["root"],
        "an unchanged window bumps nothing"
    );
}

// ---------------------------------------------------------------------------
// (iv) A grandchild rebuild returning LAYOUT lays out its whole path and skips
// a clean sibling subtree.
// ---------------------------------------------------------------------------

#[test]
fn a_grandchild_layout_rebuild_lays_out_its_path_and_skips_a_clean_sibling() {
    let mut h = Harness::new();
    let flags = h.rebuild(Shape {
        target_w: 60.0,
        ..Shape::default()
    });
    assert!(flags.contains(ChangeFlags::LAYOUT));
    // The component pod, its child pod (mid) and the target's pod — and none of
    // the sibling subtree.
    assert_eq!(
        h.dirty_nodes(),
        ["ComponentWidget", "ColWidget", "LeafWidget"]
    );
    h.layout();
    assert_eq!(take_layouts(), ["mid", "root", "target"]);
    assert!(
        h.dirty_nodes().is_empty(),
        "a layout that ran clears the flag"
    );
}

// ---------------------------------------------------------------------------
// (v) Paint-time request_layout and request_frame dirty the path.
// ---------------------------------------------------------------------------

#[test]
fn paint_time_request_layout_dirties_the_path_and_the_pending_flags() {
    let mut h = Harness::new();
    set_paint_req("target", Some(PaintReq::Layout));
    h.paint();
    set_paint_req("target", None);
    assert_eq!(
        h.dirty_nodes(),
        ["ComponentWidget", "ColWidget", "LeafWidget"],
        "the requester and every pod above it"
    );
    assert!(h.root.take_change_flags().needs_layout());
    h.layout();
    assert_eq!(take_layouts(), ["mid", "root", "target"]);
}

#[test]
fn paint_time_request_frame_dirties_the_path() {
    let mut h = Harness::new();
    set_paint_req("sib_leaf", Some(PaintReq::Frame));
    h.paint();
    // An animating subtree is relaid out — conservatively — and the marked path
    // reaches the pending flags, so a gated shell runs the pass.
    assert_eq!(h.dirty_nodes(), ["ColWidget", "LeafWidget"]);
    assert!(h.root.take_change_flags().needs_layout());
    h.layout();
    assert_eq!(take_layouts(), ["root", "sib", "sib_leaf"]);
    // Still animating: the next paint dirties the path again.
    h.paint();
    assert!(h.root.take_change_flags().needs_layout());
    h.layout();
    assert_eq!(take_layouts(), ["root", "sib", "sib_leaf"]);
    // The paint the animation lands on asks for nothing, yet it is the one that
    // recorded the settled state: the path is laid out once more.
    set_paint_req("sib_leaf", None);
    h.paint();
    assert_eq!(h.dirty_nodes(), ["ColWidget", "LeafWidget"]);
    assert!(
        h.root.take_change_flags().needs_layout(),
        "the trailing landing paint folds LAYOUT once"
    );
    h.layout();
    assert_eq!(take_layouts(), ["root", "sib", "sib_leaf"]);
    // At rest: nothing below the root, and nothing pending.
    h.paint();
    assert!(h.dirty_nodes().is_empty());
    assert!(!h.root.has_pending_change_flags(), "the fold goes quiet");
    h.layout();
    assert_eq!(take_layouts(), ["root"]);
}

#[test]
fn a_paced_only_request_marks_the_path_but_forces_no_pass() {
    let mut h = Harness::new();
    set_paint_req("sib_leaf", Some(PaintReq::Paced));
    for _ in 0..3 {
        h.paint();
        // The per-pod rule is unchanged: the looping subtree is marked...
        assert_eq!(h.dirty_nodes(), ["ColWidget", "LeafWidget"]);
        // ...but nothing is pending, so the frame gate can keep pacing the
        // loop instead of running every vsync.
        assert!(
            !h.root.has_pending_change_flags(),
            "a paced-only frame folds no LAYOUT"
        );
    }
    // The next pass that does run lays the marked path out.
    h.layout();
    assert_eq!(take_layouts(), ["root", "sib", "sib_leaf"]);
    // The loop ends: the landing paint folds once, then goes quiet.
    set_paint_req("sib_leaf", None);
    h.paint();
    assert!(h.root.take_change_flags().needs_layout());
    h.layout();
    h.paint();
    assert!(!h.root.has_pending_change_flags());
}

// ---------------------------------------------------------------------------
// (vi) An event consumed by a leaf dirties its path.
// ---------------------------------------------------------------------------

#[test]
fn an_event_consumed_by_a_leaf_dirties_its_path() {
    let mut h = Harness::new();
    set_event_mode("sib_leaf", EventMode::Consume);
    // sib_leaf sits at y 10..20 (below the 10px-tall target).
    h.down_at(15.0);
    // The containers report `Ignored`; the path bit alone reaches the root —
    // and its pending flags.
    assert_eq!(h.dirty_nodes(), ["ColWidget", "LeafWidget"]);
    assert!(h.root.take_change_flags().needs_layout());
    h.layout();
    assert_eq!(take_layouts(), ["root", "sib", "sib_leaf"]);
}

#[test]
fn an_event_consumed_across_a_component_boundary_dirties_the_whole_path() {
    let mut h = Harness::new();
    set_event_mode("target", EventMode::Consume);
    h.down_at(5.0);
    assert_eq!(
        h.dirty_nodes(),
        ["ComponentWidget", "ColWidget", "LeafWidget"],
        "the path bit crosses the component's substituted context"
    );
    h.layout();
    assert_eq!(take_layouts(), ["mid", "root", "target"]);
}

#[test]
fn event_request_layout_dirties_the_path_and_folds_layout_into_pending() {
    let mut h = Harness::new();
    set_event_mode("target", EventMode::RequestLayout);
    h.down_at(5.0);
    assert_eq!(
        h.dirty_nodes(),
        ["ComponentWidget", "ColWidget", "LeafWidget"]
    );
    assert!(
        h.root.take_change_flags().needs_layout(),
        "EventCtx::request_layout reaches the root's pending flags"
    );
    h.layout();
    assert_eq!(take_layouts(), ["mid", "root", "target"]);
}

#[test]
fn an_ignored_event_dirties_nothing() {
    let mut h = Harness::new();
    // No mode set: every widget ignores the press and asks for nothing.
    h.down_at(15.0);
    assert!(h.dirty_nodes().is_empty());
    assert!(
        !h.root.take_change_flags().needs_layout(),
        "an ignored event asks for no pass"
    );
    h.layout();
    assert_eq!(take_layouts(), ["root"]);
}

// ---------------------------------------------------------------------------
// (vii) The kill switch lays out everything.
// ---------------------------------------------------------------------------

#[test]
fn set_layout_skip_false_lays_out_everything() {
    let mut h = Harness::new();
    assert!(h.root.layout_skip(), "the skip is on by default");
    h.root.set_layout_skip(false);
    h.layout();
    assert_eq!(take_layouts(), EVERYTHING);
    h.layout();
    assert_eq!(take_layouts(), EVERYTHING, "every pass, not just the first");
    h.root.set_layout_skip(true);
    h.layout();
    assert_eq!(take_layouts(), ["root"]);
}

// ---------------------------------------------------------------------------
// A context built outside a root is forced.
// ---------------------------------------------------------------------------

#[test]
fn a_standalone_layout_ctx_always_lays_out() {
    let mut pod = ChildPod::new(Box::new(LeafWidget {
        name: "solo",
        size: Size::new(10.0, 10.0),
    }));
    let bc = BoxConstraints::loose(Size::new(50.0, 50.0));
    let mut ctx = LayoutCtx::new();
    assert!(ctx.is_forced());
    pod.layout_child(&mut ctx, &bc);
    pod.layout_child(&mut ctx, &bc);
    assert_eq!(take_layouts(), ["solo", "solo"]);
    assert!(!pod.is_layout_dirty());
}

// ---------------------------------------------------------------------------
// A hand-rolled reconciler that reaches a grandchild pod through the child
// pod's `widget_mut` (the shape several design-system bars use: an item pod
// whose widget owns its own icon/label pods) is covered by `widget_mut`'s
// conservative mark, with no knowledge of the skip.
// ---------------------------------------------------------------------------

struct ItemWidget {
    inner: ChildPod,
}

impl Widget for ItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout("item");
        self.inner.layout_child(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.inner.paint_child(ctx, scene);
    }
}

struct BarWidget {
    item: ChildPod,
}

impl Widget for BarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout("bar");
        self.item.layout_child(ctx, bc)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.item.paint_child(ctx, scene);
    }
}

struct BarView {
    leaf: LeafView,
}

impl View<()> for BarView {
    type Element = BarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BarWidget {
        let inner = ChildPod::new(Box::new(View::<()>::build(&self.leaf, ctx)));
        BarWidget {
            item: ChildPod::new(Box::new(ItemWidget { inner })),
        }
    }

    fn rebuild(&self, prev: &Self, element: &mut BarWidget, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        // Reaches the item's widget through the marking accessor and rebuilds
        // the item's own pod with a hand-spelled mark — the item pod itself is
        // never marked from any flags.
        let item = element
            .item
            .widget_mut()
            .downcast_mut::<ItemWidget>()
            .expect("the bar's pod holds an ItemWidget");
        let leaf = item
            .inner
            .widget_mut_for_rebuild()
            .downcast_mut::<LeafWidget>()
            .expect("the item's pod holds a LeafWidget");
        let flags = View::<()>::rebuild(&self.leaf, &prev.leaf, leaf, ctx);
        if flags.contains(ChangeFlags::LAYOUT) {
            item.inner.mark_layout_dirty();
        }
        flags
    }
}

#[test]
fn a_rebuild_through_widget_mut_dirties_the_pod_it_reached_through() {
    let owner = Owner::new();
    owner.set();
    let mut root: RenderRoot<(), ColView<()>> = RenderRoot::new();
    let shape = |w: f64| {
        move |_s: &mut ()| {
            col::<()>(
                "root",
                200.0,
                vec![any(BarView {
                    leaf: leaf("icon", w),
                })],
            )
        }
    };
    root.rebuild(&mut shape(20.0), &mut ());
    root.layout(WINDOW);
    assert_eq!(take_layouts(), ["bar", "icon", "item", "root"]);

    // The icon grows: the item pod was reached only through `widget_mut`, yet
    // the whole path down to the icon lays out.
    root.rebuild(&mut shape(30.0), &mut ());
    root.layout(WINDOW);
    assert_eq!(take_layouts(), ["bar", "icon", "item", "root"]);

    // A no-op rebuild still dirties the item pod (conservatively), but the bar
    // above it reported nothing, so the pass never reaches it.
    root.rebuild(&mut shape(30.0), &mut ());
    root.layout(WINDOW);
    assert_eq!(take_layouts(), ["root"]);
}

#[test]
fn widget_mut_marks_the_pod_and_widget_mut_for_rebuild_does_not() {
    let mut pod = ChildPod::new(Box::new(LeafWidget {
        name: "solo",
        size: Size::new(10.0, 10.0),
    }));
    pod.layout_child(
        &mut LayoutCtx::new(),
        &BoxConstraints::loose(Size::new(50.0, 50.0)),
    );
    take_layouts();
    assert!(!pod.is_layout_dirty());
    pod.widget_mut_for_rebuild();
    assert!(!pod.is_layout_dirty());
    pod.widget_mut();
    assert!(pod.is_layout_dirty());
}

// ---------------------------------------------------------------------------
// Through the shell gate: layout runs only when `take_change_flags()` asks for
// it, the way every shell drives the root. A widget whose size changes only
// through a consumed event, a bare frame request or the trailing landing paint
// — never through a `LAYOUT` flag of its own — is still laid out at its new
// size on the next gated pass.
// ---------------------------------------------------------------------------

/// What a `GrowWidget` does with a pointer `Down`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GrowOnDown {
    /// Take the next height, return `Handled` and request a redraw.
    Consume,
    /// Take the next height, request a redraw only, return `Ignored`.
    Redraw,
}

thread_local! {
    /// Heights the next `Down` adopts, front first.
    static GROW_ON_DOWN: RefCell<Vec<(GrowOnDown, f64)>> = const { RefCell::new(Vec::new()) };
    /// One step per paint, front first: the height that paint records and
    /// whether it asks for another frame. Empty: the paint asks for nothing.
    static GROW_PAINT: RefCell<Vec<(f64, bool)>> = const { RefCell::new(Vec::new()) };
}

/// A leaf whose reported height is widget state changed by its own event and
/// paint — the shape of a growing multiline field or a paint-fed layout
/// animation. Its view never reports `LAYOUT` after the build.
struct GrowWidget {
    height: f64,
}

impl Widget for GrowWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout("grow");
        bc.constrain(Size::new(20.0, self.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let step = GROW_PAINT.with(|steps| {
            let mut steps = steps.borrow_mut();
            (!steps.is_empty()).then(|| steps.remove(0))
        });
        if let Some((height, frame)) = step {
            self.height = height;
            if frame {
                ctx.request_frame();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(pointer) = event else {
            return EventResult::Ignored;
        };
        if pointer.phase != PointerPhase::Down {
            return EventResult::Ignored;
        }
        let next = GROW_ON_DOWN.with(|downs| {
            let mut downs = downs.borrow_mut();
            (!downs.is_empty()).then(|| downs.remove(0))
        });
        match next {
            Some((mode, height)) => {
                self.height = height;
                ctx.request_redraw();
                match mode {
                    GrowOnDown::Consume => EventResult::Handled,
                    GrowOnDown::Redraw => EventResult::Ignored,
                }
            }
            None => EventResult::Ignored,
        }
    }
}

struct GrowView;

impl View<()> for GrowView {
    type Element = GrowWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GrowWidget {
        GrowWidget { height: 10.0 }
    }

    fn rebuild(&self, _prev: &Self, _el: &mut GrowWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// `root (Col) ─ box (Col) ─ grow`: the grower sits two pods down, so a
/// pass reaches it only through a marked path.
struct Gated {
    root: RenderRoot<(), ColView<()>>,
    _owner: Owner,
}

impl Gated {
    fn new() -> Self {
        let owner = Owner::new();
        owner.set();
        let mut gated = Self {
            root: RenderRoot::new(),
            _owner: owner,
        };
        // The first frame always lays out (every shell forces it).
        gated.rebuild();
        gated.root.layout(WINDOW);
        gated.root.paint(&mut DiscardScene, FrameTime::ZERO);
        gated.root.take_change_flags();
        take_layouts();
        gated
    }

    fn rebuild(&mut self) {
        self.root.rebuild(
            &mut |_s: &mut ()| {
                col(
                    "root",
                    200.0,
                    vec![any(col::<()>("box", 100.0, vec![any(GrowView)]))],
                )
            },
            &mut (),
        );
    }

    /// One shell frame: rebuild, lay out only if the pending flags ask, paint.
    /// Returns whether layout ran and whether the paint asked for a frame.
    fn frame(&mut self) -> (bool, bool) {
        self.rebuild();
        let laid_out = self.root.take_change_flags().needs_layout();
        if laid_out {
            self.root.layout(WINDOW);
        }
        let needs_frame = self
            .root
            .paint(&mut DiscardScene, FrameTime::ZERO)
            .needs_frame;
        (laid_out, needs_frame)
    }

    fn down(&mut self) {
        self.root.event(
            &mut (),
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(5.0, 5.0),
                button: PointerButton::Primary,
            }),
        );
    }

    fn grow_height(&self) -> f64 {
        self.root
            .inspect()
            .iter()
            .find(|n| n.type_name.contains("GrowWidget"))
            .map(|n| n.bounds.height())
            .expect("the grower is in the tree")
    }
}

#[test]
fn a_handled_only_event_reaches_the_shell_gate() {
    let mut g = Gated::new();
    assert_eq!(g.grow_height(), 10.0);

    // Consumed, redraw requested, size changed in widget state — no LAYOUT
    // anywhere.
    GROW_ON_DOWN.with(|d| d.borrow_mut().push((GrowOnDown::Consume, 30.0)));
    g.down();
    assert!(
        g.root.has_pending_change_flags(),
        "the consumed event leaves the gate something to drain"
    );
    let (laid_out, _) = g.frame();
    assert!(laid_out, "take_change_flags reported needs_layout");
    assert_eq!(take_layouts(), ["box", "grow", "root"]);
    assert_eq!(g.grow_height(), 30.0, "the new size was laid out");

    // A redraw request alone (the event ignored) is the same source.
    GROW_ON_DOWN.with(|d| d.borrow_mut().push((GrowOnDown::Redraw, 45.0)));
    g.down();
    let (laid_out, _) = g.frame();
    assert!(laid_out);
    assert_eq!(g.grow_height(), 45.0);

    // Quiet afterwards: nothing pending, no pass.
    take_layouts();
    let (laid_out, needs_frame) = g.frame();
    assert!(!laid_out && !needs_frame, "an idle frame runs no layout");
}

#[test]
fn a_request_frame_only_paint_and_its_landing_reach_the_shell_gate() {
    let mut g = Gated::new();
    // Two animating paints, then the landing one that asks for nothing.
    GROW_PAINT.with(|p| {
        p.borrow_mut()
            .extend([(20.0, true), (30.0, true), (40.0, false)])
    });

    // Frame 1: nothing pending yet, so no pass; the paint records 20 and asks
    // for a frame.
    let (laid_out, needs_frame) = g.frame();
    assert!(!laid_out && needs_frame);
    assert_eq!(g.grow_height(), 10.0);

    // Frame 2: the frame request folded LAYOUT; the pass lays out 20.
    let (laid_out, needs_frame) = g.frame();
    assert!(laid_out, "a bare request_frame reached the gate");
    assert_eq!(take_layouts(), ["box", "grow", "root"]);
    assert_eq!(g.grow_height(), 20.0);
    assert!(needs_frame);

    // Frame 3: lays out 30; its paint lands on 40 and asks for nothing.
    let (laid_out, needs_frame) = g.frame();
    assert!(laid_out);
    assert_eq!(g.grow_height(), 30.0);
    assert!(!needs_frame, "the landing paint asks for no frame");

    // Frame 4: the trailing landing paint folded LAYOUT once; the settled
    // value is laid out.
    let (laid_out, needs_frame) = g.frame();
    assert!(laid_out, "the trailing landing paint reached the gate");
    assert_eq!(g.grow_height(), 40.0);
    assert!(!needs_frame);

    // Quiescent within two frames of the landing: nothing pending, no frame
    // request, and the next frame runs no pass.
    assert!(
        !g.root.has_pending_change_flags(),
        "the trailing fold went quiet"
    );
    take_layouts();
    let (laid_out, needs_frame) = g.frame();
    assert!(!laid_out && !needs_frame);
    assert!(take_layouts().is_empty());
}
