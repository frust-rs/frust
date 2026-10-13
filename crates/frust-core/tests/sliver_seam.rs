//! The sliver seam: `Widget::as_sliver` reaching a sliver through type erasure
//! and a component boundary, `ChildPod::layout_sliver_child` recording the
//! geometry and sizing the pod to the painted extent, a box widget answering
//! `None`, and the per-pod layout skip applying to sliver constraints exactly
//! as it does to box constraints.
//!
//! The skip tests drive a real `RenderRoot` whose root widget is a minimal
//! vertical viewport; each sliver logs its name on every `layout_sliver` call,
//! so a test asserts exactly which slivers a pass laid out.

use std::cell::RefCell;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Component, DiscardScene, FrameTime,
    LayoutCtx, PaintCtx, PaintScene, RenderRoot, ScrollAxis, ScrollDirection, SliverConstraints,
    SliverGeometry, SliverWidget, View, Widget, any, component,
};
use kurbo::{Point, Rect, Size};
use reactive_graph::owner::Owner;

thread_local! {
    static LAYOUT_LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
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

// ---------------------------------------------------------------------------
// A counting sliver: `extent` pixels of content, painting whatever part of it
// is past the scroll offset and inside the remaining paint extent.
// ---------------------------------------------------------------------------

struct CountingSliver {
    name: &'static str,
    extent: f64,
    /// When set, the next sliver layout reports this correction once.
    correction: Option<f64>,
}

impl Widget for CountingSliver {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout("box-layout");
        bc.constrain(Size::new(bc.max().width, self.extent))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }
}

impl SliverWidget for CountingSliver {
    fn layout_sliver(&mut self, _ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        log_layout(self.name);
        let paint = (self.extent - sc.scroll_offset).clamp(0.0, sc.remaining_paint_extent);
        SliverGeometry {
            scroll_offset_correction: self.correction.take(),
            ..SliverGeometry::new(self.extent, paint, self.extent)
        }
    }
}

fn sliver(name: &'static str, extent: f64) -> CountingSliver {
    CountingSliver {
        name,
        extent,
        correction: None,
    }
}

struct SliverView {
    name: &'static str,
    extent: f64,
    /// Layout-affecting state that changes no geometry: a change rebuilds with
    /// `LAYOUT` while the constraints the sliver sees stay equal.
    generation: u32,
}

fn sliver_view(name: &'static str, extent: f64) -> SliverView {
    SliverView {
        name,
        extent,
        generation: 0,
    }
}

impl<S: 'static> View<S> for SliverView {
    type Element = CountingSliver;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CountingSliver {
        sliver(self.name, self.extent)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CountingSliver,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if self.extent != prev.extent || self.generation != prev.generation {
            element.extent = self.extent;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

// ---------------------------------------------------------------------------
// A plain box leaf, which answers `as_sliver()` with the default `None`.
// ---------------------------------------------------------------------------

struct BoxLeaf;

impl Widget for BoxLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout("box");
        bc.constrain(Size::new(bc.max().width, 40.0))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

struct BoxView;

impl<S: 'static> View<S> for BoxView {
    type Element = BoxLeaf;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BoxLeaf {
        BoxLeaf
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut BoxLeaf,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

// ---------------------------------------------------------------------------
// Components hosting a sliver and a box.
// ---------------------------------------------------------------------------

struct SliverComponent {
    extent: f64,
    generation: u32,
}

impl Component for SliverComponent {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        SliverView {
            generation: self.generation,
            ..sliver_view("comp", self.extent)
        }
    }
}

struct BoxComponent;

impl Component for BoxComponent {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        BoxView
    }
}

// ---------------------------------------------------------------------------
// A minimal vertical viewport: lays each child out as a sliver when it is one
// and as a box (tight across, unbounded along) otherwise, stacking the results
// from the top. With `as_boxes` set it lays every child out as a box instead.
// ---------------------------------------------------------------------------

struct ViewportWidget {
    scroll_offset: f64,
    as_boxes: bool,
    children: Vec<ChildPod>,
}

impl Widget for ViewportWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        log_layout("viewport");
        let size = bc.max();
        let axis = ScrollAxis::Vertical;
        let main = axis.main_extent(size);
        let cross = axis.cross_extent(size);
        let (mut preceding, mut painted) = (0.0f64, 0.0f64);
        for pod in &mut self.children {
            let remaining = (main - painted).max(0.0);
            let sc = SliverConstraints {
                axis,
                scroll_offset: (self.scroll_offset - preceding).max(0.0),
                preceding_scroll_extent: preceding,
                overlap: 0.0,
                remaining_paint_extent: remaining,
                cross_axis_extent: cross,
                viewport_main_axis_extent: main,
                cache_origin: 0.0,
                remaining_cache_extent: remaining,
                user_scroll_direction: ScrollDirection::Idle,
            };
            pod.set_origin(Point::new(0.0, painted));
            let geometry = if self.as_boxes {
                None
            } else {
                pod.layout_sliver_child(ctx, &sc)
            };
            match geometry {
                Some(geometry) => {
                    preceding += geometry.scroll_extent;
                    painted += geometry.layout_extent;
                }
                None => {
                    let child = pod.layout_child(ctx, &sc.as_box_constraints());
                    preceding += child.height;
                    painted += child.height;
                }
            }
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for pod in &self.children {
            visitor(pod);
        }
    }
}

struct ViewportView<S: 'static> {
    scroll_offset: f64,
    as_boxes: bool,
    children: Vec<AnyView<S>>,
}

impl<S: 'static> View<S> for ViewportView<S> {
    type Element = ViewportWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ViewportWidget {
        ViewportWidget {
            scroll_offset: self.scroll_offset,
            as_boxes: self.as_boxes,
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
    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ViewportWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        assert_eq!(self.children.len(), prev.children.len());
        let mut flags = ChangeFlags::NONE;
        if self.scroll_offset != prev.scroll_offset || self.as_boxes != prev.as_boxes {
            element.scroll_offset = self.scroll_offset;
            element.as_boxes = self.as_boxes;
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

/// The app every root-driven test lays out:
///
/// ```text
/// viewport (root widget, laid out every pass)
/// ├── a       (sliver, 250 px of content)
/// ├── comp    (component hosting a 200 px sliver)
/// └── box     (box leaf, 40 px)
/// ```
#[derive(Clone, Copy, Default)]
struct Shape {
    scroll_offset: f64,
    as_boxes: bool,
    a_generation: u32,
    comp_extent_extra: f64,
    comp_generation: u32,
}

fn app(shape: Shape) -> ViewportView<()> {
    ViewportView {
        scroll_offset: shape.scroll_offset,
        as_boxes: shape.as_boxes,
        children: vec![
            any(SliverView {
                generation: shape.a_generation,
                ..sliver_view("a", 250.0)
            }),
            any(component(SliverComponent {
                extent: 200.0 + shape.comp_extent_extra,
                generation: shape.comp_generation,
            })),
            any(BoxView),
        ],
    }
}

const WINDOW: Size = Size::new(400.0, 600.0);

struct Harness {
    root: RenderRoot<(), ViewportView<()>>,
    _owner: Owner,
}

impl Harness {
    /// Build, lay out and paint the default shape once, then drain the log.
    fn new() -> Self {
        let owner = Owner::new();
        owner.set();
        let mut harness = Self {
            root: RenderRoot::new(),
            _owner: owner,
        };
        harness.rebuild(Shape::default());
        harness.layout();
        harness.root.paint(&mut DiscardScene, FrameTime::ZERO);
        assert_eq!(
            take_layouts(),
            ["a", "box", "comp", "viewport"],
            "a fresh tree lays out every child under its own protocol"
        );
        harness.root.take_change_flags();
        harness
    }

    fn rebuild(&mut self, shape: Shape) {
        self.root.rebuild(&mut |_s: &mut ()| app(shape), &mut ());
    }

    fn layout(&mut self) {
        self.root.layout(WINDOW);
    }

    /// The absolute bounds of every node whose short type name is `name`, in
    /// pre-order.
    fn bounds_of(&self, name: &str) -> Vec<Rect> {
        self.root
            .inspect()
            .iter()
            .filter(|node| short(node.type_name) == name)
            .map(|node| node.bounds)
            .collect()
    }
}

/// The last path segment of a type name, with generics dropped.
fn short(type_name: &'static str) -> &'static str {
    let head = type_name.split('<').next().unwrap_or(type_name);
    head.rsplit("::").next().unwrap_or(head)
}

fn vertical(scroll_offset: f64, remaining: f64) -> SliverConstraints {
    SliverConstraints {
        axis: ScrollAxis::Vertical,
        scroll_offset,
        preceding_scroll_extent: 0.0,
        overlap: 0.0,
        remaining_paint_extent: remaining,
        cross_axis_extent: 400.0,
        viewport_main_axis_extent: 600.0,
        cache_origin: 0.0,
        remaining_cache_extent: remaining,
        user_scroll_direction: ScrollDirection::Idle,
    }
}

// ---------------------------------------------------------------------------
// `as_sliver` forwarding.
// ---------------------------------------------------------------------------

#[test]
fn a_sliver_answers_as_sliver_through_box_dyn_widget() {
    let mut boxed: Box<dyn Widget> = Box::new(sliver("a", 100.0));
    assert!(boxed.as_sliver().is_some(), "a boxed sliver stays a sliver");

    // The double box `ChildPod::new(Box::new(element))` stores resolves through
    // both layers.
    let mut double: Box<dyn Widget> = Box::new(boxed);
    assert!(double.as_sliver().is_some(), "a doubly boxed sliver too");
    let mut pod = ChildPod::new(double);
    assert!(pod.is_sliver());
    assert!(
        pod.is_layout_dirty(),
        "asking is_sliver is not a layout-dirty source"
    );
}

#[test]
fn a_box_widget_answers_none() {
    let mut boxed: Box<dyn Widget> = Box::new(BoxLeaf);
    assert!(boxed.as_sliver().is_none());

    let mut pod = ChildPod::new(Box::new(boxed));
    assert!(!pod.is_sliver());
    let geometry = pod.layout_sliver_child(&mut LayoutCtx::new(), &vertical(0.0, 600.0));
    assert_eq!(geometry, None, "a box child has no sliver geometry");
    assert_eq!(pod.sliver_geometry(), None);
    assert_eq!(
        pod.size(),
        Size::ZERO,
        "a refused sliver layout sizes nothing"
    );
    assert!(
        pod.is_layout_dirty(),
        "and leaves the pod for its box layout"
    );
    assert_eq!(take_layouts(), Vec::<&str>::new(), "nothing was laid out");
}

#[test]
fn a_component_forwards_as_sliver_to_its_child() {
    let owner = Owner::new();
    owner.set();
    let mut next_id = 0;
    let mut ctx = BuildCtx::new(&mut next_id);

    let view = component(SliverComponent {
        extent: 300.0,
        generation: 0,
    });
    let element = <_ as View<()>>::build(&view, &mut ctx);
    let mut boxed: Box<dyn Widget> = Box::new(element);
    assert!(
        boxed.as_sliver().is_some(),
        "a component hosting a sliver is a sliver"
    );

    let box_view = component(BoxComponent);
    let box_element = <_ as View<()>>::build(&box_view, &mut ctx);
    let mut box_boxed: Box<dyn Widget> = Box::new(box_element);
    assert!(
        box_boxed.as_sliver().is_none(),
        "a component hosting a box is not"
    );

    // Laid out as a sliver, the component lays its child out through its own
    // pod, so the inner pod is sized and recorded too.
    let mut pod = ChildPod::new(boxed);
    let geometry = pod
        .layout_sliver_child(&mut LayoutCtx::new(), &vertical(100.0, 600.0))
        .expect("the component is a sliver");
    assert_eq!(geometry.paint_extent, 200.0);
    assert_eq!(pod.size(), Size::new(400.0, 200.0));
    assert_eq!(take_layouts(), ["comp"]);

    let mut inner = Vec::new();
    pod.widget().visit_children(&mut |child: &ChildPod| {
        inner.push((child.size(), child.sliver_geometry()));
    });
    assert_eq!(inner, [(Size::new(400.0, 200.0), Some(geometry))]);
}

// ---------------------------------------------------------------------------
// `layout_sliver_child` geometry and sizing.
// ---------------------------------------------------------------------------

#[test]
fn layout_sliver_child_stores_geometry_and_sizes_the_pod_to_the_paint_extent() {
    let mut pod = ChildPod::new(Box::new(Box::new(sliver("a", 1000.0)) as Box<dyn Widget>));
    let mut ctx = LayoutCtx::new();

    let geometry = pod
        .layout_sliver_child(&mut ctx, &vertical(700.0, 600.0))
        .expect("a sliver");
    assert_eq!(geometry.scroll_extent, 1000.0);
    assert_eq!(geometry.paint_extent, 300.0);
    assert_eq!(pod.sliver_geometry(), Some(geometry));
    assert_eq!(pod.size(), Size::new(400.0, 300.0));
    assert!(!pod.is_layout_dirty());

    // The hit test covers the painted extent only.
    pod.set_origin(Point::new(0.0, 50.0));
    assert!(pod.contains(Point::new(10.0, 50.0)));
    assert!(pod.contains(Point::new(10.0, 349.0)));
    assert!(!pod.contains(Point::new(10.0, 351.0)));

    // Horizontal: the painted extent runs along x.
    let horizontal = SliverConstraints {
        axis: ScrollAxis::Horizontal,
        ..vertical(0.0, 250.0)
    };
    let geometry = pod
        .layout_sliver_child(&mut ctx, &horizontal)
        .expect("a sliver");
    assert_eq!(geometry.paint_extent, 250.0);
    assert_eq!(pod.size(), Size::new(250.0, 400.0));

    // A box layout afterwards drops the sliver record.
    pod.layout_child(&mut ctx, &BoxConstraints::tight(Size::new(10.0, 10.0)));
    assert_eq!(pod.sliver_geometry(), None);
    assert_eq!(take_layouts(), ["a", "a", "box-layout"]);
}

#[test]
fn a_scroll_offset_correction_leaves_the_pod_dirty() {
    let mut widget = sliver("a", 500.0);
    widget.correction = Some(-20.0);
    let mut pod = ChildPod::new(Box::new(Box::new(widget) as Box<dyn Widget>));
    let mut ctx = LayoutCtx::new();

    let geometry = pod
        .layout_sliver_child(&mut ctx, &vertical(0.0, 600.0))
        .expect("a sliver");
    assert_eq!(geometry.scroll_offset_correction, Some(-20.0));
    assert!(
        pod.is_layout_dirty(),
        "a correction must never be replayed from the cache"
    );

    let geometry = pod
        .layout_sliver_child(&mut ctx, &vertical(20.0, 600.0))
        .expect("a sliver");
    assert_eq!(geometry.scroll_offset_correction, None);
    assert!(!pod.is_layout_dirty());
}

// ---------------------------------------------------------------------------
// The layout skip under sliver constraints, through a real root.
// ---------------------------------------------------------------------------

#[test]
fn slivers_are_sized_to_their_paint_extent_in_the_tree() {
    let h = Harness::new();
    assert_eq!(
        h.bounds_of("CountingSliver"),
        [
            Rect::new(0.0, 0.0, 400.0, 250.0),
            Rect::new(0.0, 250.0, 400.0, 450.0),
        ]
    );
    assert_eq!(
        h.bounds_of("ComponentWidget"),
        [Rect::new(0.0, 250.0, 400.0, 450.0)]
    );
    assert_eq!(
        h.bounds_of("BoxLeaf"),
        [Rect::new(0.0, 450.0, 400.0, 490.0)]
    );
}

#[test]
fn a_clean_sliver_under_equal_constraints_is_not_laid_out() {
    let mut h = Harness::new();
    h.layout();
    assert_eq!(take_layouts(), ["viewport"], "only the pod-less root runs");
}

#[test]
fn changed_sliver_constraints_lay_out_again() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        scroll_offset: 100.0,
        ..Shape::default()
    });
    h.layout();
    // `a` sees a new scroll offset and `comp` a new remaining paint extent; the
    // box child's box constraints are unchanged, so it is skipped.
    assert_eq!(take_layouts(), ["a", "comp", "viewport"]);
    assert_eq!(
        h.bounds_of("CountingSliver"),
        [
            Rect::new(0.0, 0.0, 400.0, 150.0),
            Rect::new(0.0, 150.0, 400.0, 350.0),
        ]
    );
}

#[test]
fn a_dirty_sliver_lays_out_again_under_equal_constraints() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        a_generation: 1,
        ..Shape::default()
    });
    h.layout();
    assert_eq!(
        take_layouts(),
        ["a", "viewport"],
        "a rebuild returning LAYOUT dirties the sliver; its geometry is \
         unchanged, so nothing after it sees new constraints"
    );
}

#[test]
fn a_sliver_behind_a_component_follows_the_same_rule() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        comp_generation: 1,
        ..Shape::default()
    });
    h.layout();
    assert_eq!(take_layouts(), ["comp", "viewport"]);

    h.rebuild(Shape {
        comp_generation: 1,
        comp_extent_extra: 50.0,
        ..Shape::default()
    });
    h.layout();
    // The component's sliver grew, which moves the box child down but hands it
    // the same box constraints.
    assert_eq!(take_layouts(), ["comp", "viewport"]);
    assert_eq!(
        h.bounds_of("BoxLeaf"),
        [Rect::new(0.0, 500.0, 400.0, 540.0)]
    );
}

#[test]
fn a_moved_epoch_lays_every_sliver_out() {
    let mut h = Harness::new();
    // A window resize bumps the epoch. The slivers' remaining paint extent
    // moves with the taller window, but the box child's box constraints depend
    // on the unchanged cross extent alone, so only the epoch explains its
    // layout.
    h.root.layout(Size::new(400.0, 700.0));
    assert_eq!(take_layouts(), ["a", "box", "comp", "viewport"]);
    h.root.layout(Size::new(400.0, 700.0));
    assert_eq!(take_layouts(), ["viewport"]);
}

#[test]
fn a_forced_pass_lays_every_sliver_out() {
    let mut h = Harness::new();
    h.root.set_layout_skip(false);
    h.layout();
    assert_eq!(take_layouts(), ["a", "box", "comp", "viewport"]);
}

#[test]
fn switching_protocol_always_lays_out() {
    let mut h = Harness::new();
    h.rebuild(Shape {
        as_boxes: true,
        ..Shape::default()
    });
    h.layout();
    assert_eq!(
        take_layouts(),
        ["box-layout", "box-layout", "viewport"],
        "the slivers are laid out as boxes; the box leaf's box constraints are \
         unchanged, so its cached layout is reused"
    );

    h.rebuild(Shape::default());
    h.layout();
    assert_eq!(
        take_layouts(),
        ["a", "comp", "viewport"],
        "back under sliver constraints equal to the first pass's, the box \
         layout in between still forces a sliver layout"
    );
}
