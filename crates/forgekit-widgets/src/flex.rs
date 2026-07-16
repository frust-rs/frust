//! Flex layout container (spec §6.2): `Row`/`Column` over a main/cross axis.
//!
//! [`FlexView`]/[`FlexWidget`] are the declarative/retained pair (mirroring
//! [`crate::text`]'s `TextView`/`TextWidget`). A flex lays its children out along
//! a main [`Axis`], mirroring Flutter's `Flex`: inflexible children take their
//! natural main size first, then any remaining main-axis space is divided among
//! flexible children in proportion to their `flex` factor.
//!
//! Construct one with the [`Row`]/[`Column`] sugar (all children inflexible) or
//! [`FlexView::new`] with explicit [`FlexChild`]s built via [`flexible`] /
//! [`inflexible`] when some children should expand.

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, View, Widget, any,
};
use kurbo::{Point, Size};

use crate::ChildKey;

/// The axis a [`FlexView`] lays its children along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children are placed left-to-right; main = width, cross = height.
    Horizontal,
    /// Children are placed top-to-bottom; main = height, cross = width.
    Vertical,
}

impl Axis {
    /// The main-axis extent of `size`.
    fn main_of(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        }
    }

    /// The cross-axis extent of `size`.
    fn cross_of(self, size: Size) -> f64 {
        match self {
            Axis::Horizontal => size.height,
            Axis::Vertical => size.width,
        }
    }

    /// Build a [`Size`] from main/cross extents.
    fn size(self, main: f64, cross: f64) -> Size {
        match self {
            Axis::Horizontal => Size::new(main, cross),
            Axis::Vertical => Size::new(cross, main),
        }
    }

    /// Build a [`Point`] from main/cross coordinates.
    fn point(self, main: f64, cross: f64) -> Point {
        match self {
            Axis::Horizontal => Point::new(main, cross),
            Axis::Vertical => Point::new(cross, main),
        }
    }
}

/// How children are aligned along the cross axis (spec §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossAxisAlignment {
    /// Pack each child at the cross-axis start (top for a Row, left for a Column).
    Start,
    /// Center each child on the cross axis.
    Center,
    /// Stretch each child to fill the cross axis (tight cross constraint).
    Stretch,
}

/// How children are distributed along the main axis.
///
/// v1 ships only [`MainAxisAlignment::Start`] (leading-packed); the
/// space-between/around/center variants are deferred to a later phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainAxisAlignment {
    /// Pack children at the main-axis start with no leading gap.
    Start,
}

/// One child of a [`FlexView`]: an erased child view, its `flex` factor
/// (`0` = inflexible; `> 0` = takes a proportional share of the free main space),
/// and an optional [`ChildKey`] for keyed reconciliation.
///
/// `key` is `None` for the plain [`Row`]/[`Column`]/[`flexible`]/[`inflexible`]
/// sugar (positional reconciliation, unchanged) and `Some` only for children
/// built with [`keyed`], which opts the whole list into key-matched
/// reconciliation (spec §6.3) so reorders/inserts preserve widget state.
pub struct FlexChild<State: 'static> {
    view: AnyView<State>,
    flex: u32,
    key: Option<ChildKey>,
}

/// A flexible child taking `flex` proportional shares of the free main-axis space.
pub fn flexible<State: 'static, V: View<State>>(flex: u32, view: V) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex,
        key: None,
    }
}

/// An inflexible child, sized to its natural main-axis extent.
pub fn inflexible<State: 'static, V: View<State>>(view: V) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex: 0,
        key: None,
    }
}

/// An inflexible child tagged with a stable [`ChildKey`], for a list whose items
/// reorder, insert, or delete between frames (spec §6.3).
///
/// Attaching a key to *any* child opts the whole [`FlexView`] into keyed
/// reconciliation: on the next rebuild, children are matched to their live
/// widgets by key rather than by position, so a shuffled or grown list preserves
/// each surviving row's widget and its internal state (a scroll offset, a text
/// buffer, a toggle) instead of rebuilding whatever now sits at that index. Keys
/// are all-or-nothing per list and must be unique within it (see
/// [`ChildKey`]).
///
/// Use it inside [`FlexView::new`] alongside (or instead of) [`inflexible`]:
///
/// ```
/// use forgekit_widgets::{Axis, FlexView, keyed, text};
/// # struct Item { id: u64, label: String }
/// # fn demo(items: &[Item]) -> FlexView<()> {
/// FlexView::new(
///     Axis::Vertical,
///     items.iter().map(|item| keyed(item.id, text(item.label.clone()))).collect(),
/// )
/// # }
/// ```
///
/// v1 keyed children are inflexible; combining a key with a `flex` factor is a
/// future extension.
pub fn keyed<State: 'static, V: View<State>>(
    key: impl Into<ChildKey>,
    view: V,
) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex: 0,
        key: Some(key.into()),
    }
}

/// A declarative flex container. See the [module docs](self).
pub struct FlexView<State: 'static> {
    direction: Axis,
    cross: CrossAxisAlignment,
    main: MainAxisAlignment,
    children: Vec<FlexChild<State>>,
}

impl<State: 'static> FlexView<State> {
    /// Create a flex laying `children` out along `direction`, cross-aligned to
    /// the start and main-aligned to the start.
    pub fn new(direction: Axis, children: Vec<FlexChild<State>>) -> Self {
        Self {
            direction,
            cross: CrossAxisAlignment::Start,
            main: MainAxisAlignment::Start,
            children,
        }
    }

    /// Set the cross-axis alignment.
    pub fn cross_axis(mut self, cross: CrossAxisAlignment) -> Self {
        self.cross = cross;
        self
    }

    /// Set the main-axis alignment.
    pub fn main_axis(mut self, main: MainAxisAlignment) -> Self {
        self.main = main;
        self
    }
}

/// A horizontal flex (`Axis::Horizontal`) of inflexible children — the common
/// sugar. Use [`FlexView::new`] with [`flexible`] children when some should expand.
#[allow(non_snake_case)]
pub fn Row<State: 'static>(children: Vec<AnyView<State>>) -> FlexView<State> {
    FlexView::new(
        Axis::Horizontal,
        children
            .into_iter()
            .map(|view| FlexChild {
                view,
                flex: 0,
                key: None,
            })
            .collect(),
    )
}

/// A vertical flex (`Axis::Vertical`) of inflexible children — the common sugar.
/// Use [`FlexView::new`] with [`flexible`] children when some should expand.
#[allow(non_snake_case)]
pub fn Column<State: 'static>(children: Vec<AnyView<State>>) -> FlexView<State> {
    FlexView::new(
        Axis::Vertical,
        children
            .into_iter()
            .map(|view| FlexChild {
                view,
                flex: 0,
                key: None,
            })
            .collect(),
    )
}

/// The retained widget for a [`FlexView`]. Holds a parallel `children`/`flex`
/// pair (same length) so layout can index both without a per-child wrapper.
pub struct FlexWidget {
    direction: Axis,
    cross: CrossAxisAlignment,
    main: MainAxisAlignment,
    children: Vec<ChildPod>,
    flex: Vec<u32>,
}

/// Build the box constraints for one flex child.
///
/// `main_min..main_max` bound the main axis (inflexible children get
/// `0..∞`; flexible children a tight `share..share`). Under `stretch` the cross
/// axis is tight at `cross_bound`; otherwise it is loose up to `cross_max`.
fn child_constraints(
    axis: Axis,
    main_min: f64,
    main_max: f64,
    cross_bound: f64,
    stretch: bool,
    cross_max: f64,
) -> BoxConstraints {
    let cross_min = if stretch { cross_bound } else { 0.0 };
    let cross_hi = if stretch { cross_bound } else { cross_max };
    BoxConstraints::new(
        axis.size(main_min, cross_min),
        axis.size(main_max, cross_hi),
    )
}

impl<State: 'static> View<State> for FlexView<State> {
    type Element = FlexWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FlexWidget {
        let mut children = Vec::with_capacity(self.children.len());
        let mut flex = Vec::with_capacity(self.children.len());
        for child in &self.children {
            children.push(crate::build_child(&child.view, ctx));
            flex.push(child.flex);
        }
        FlexWidget {
            direction: self.direction,
            cross: self.cross,
            main: self.main,
            children,
            flex,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FlexWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.direction != self.direction || prev.cross != self.cross || prev.main != self.main {
            element.direction = self.direction;
            element.cross = self.cross;
            element.main = self.main;
            flags |= ChangeFlags::LAYOUT;
        }

        // Reconcile the child pods through the shared helper (build/rebuild/
        // teardown + the structural-change capture/focus cancellation). Keyed
        // children (`|child| child.key`) opt the list into key-matched
        // reconciliation; an all-unkeyed list stays positional.
        flags |= crate::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |child| &child.view,
            |child| child.key,
        );

        // Rebuild the parallel `flex` sidecar to match the reconciled children's
        // new order and length in one shot — the keyed path may have reordered
        // them, so an index-wise diff no longer tracks a given child. Comparing
        // against the previous sidecar keeps the layout-dirty signal a factor
        // change (or length/order change) still deserves.
        let new_flex: Vec<u32> = self.children.iter().map(|child| child.flex).collect();
        if new_flex != element.flex {
            element.flex = new_flex;
            flags |= ChangeFlags::LAYOUT;
        }

        flags
    }

    fn teardown(&self, element: &mut FlexWidget, ctx: &mut BuildCtx<'_>) {
        for (child, pod) in self.children.iter().zip(element.children.iter_mut()) {
            crate::teardown_child(&child.view, pod, ctx);
        }
    }
}

impl Widget for FlexWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let axis = self.direction;
        let max_main = axis.main_of(bc.max());
        let max_cross = axis.cross_of(bc.max());
        let stretch = self.cross == CrossAxisAlignment::Stretch;
        // Stretch needs a finite cross bound to stretch to; fall back to zero
        // under an (unusual) unbounded cross constraint.
        let cross_bound = if max_cross.is_finite() {
            max_cross
        } else {
            0.0
        };

        // Pass 1: lay out inflexible children under an unbounded main axis,
        // accumulating the space they consume and the total flex weight.
        let mut allocated_main = 0.0f64;
        let mut total_flex = 0u32;
        let mut max_child_cross = 0.0f64;
        for (i, pod) in self.children.iter_mut().enumerate() {
            if self.flex[i] == 0 {
                let cbc =
                    child_constraints(axis, 0.0, f64::INFINITY, cross_bound, stretch, max_cross);
                let size = pod.layout_child(ctx, &cbc);
                allocated_main += axis.main_of(size);
                max_child_cross = max_child_cross.max(axis.cross_of(size));
            } else {
                total_flex += self.flex[i];
            }
        }

        // Pass 2: divide the remaining main-axis space among flexible children in
        // proportion to their flex factor, laying each out under a tight main
        // constraint equal to its share.
        let free = if max_main.is_finite() {
            (max_main - allocated_main).max(0.0)
        } else {
            0.0
        };
        if total_flex > 0 {
            for (i, pod) in self.children.iter_mut().enumerate() {
                if self.flex[i] > 0 {
                    let share = free * (self.flex[i] as f64) / (total_flex as f64);
                    let cbc =
                        child_constraints(axis, share, share, cross_bound, stretch, max_cross);
                    let size = pod.layout_child(ctx, &cbc);
                    max_child_cross = max_child_cross.max(axis.cross_of(size));
                }
            }
        }

        // Main extent fills the constraint when flexible children are present (and
        // bounded); otherwise it shrink-wraps to the sum of the children.
        let main_size = if total_flex > 0 && max_main.is_finite() {
            max_main
        } else {
            allocated_main
        };
        // Cross extent fills under stretch, else shrink-wraps to the widest child.
        let cross_size = if stretch && max_cross.is_finite() {
            max_cross
        } else {
            max_child_cross
        };

        // Position children sequentially along the main axis (MainAxisAlignment
        // v1 = Start → no leading gap), cross-aligned per CrossAxisAlignment.
        let leading = match self.main {
            MainAxisAlignment::Start => 0.0,
        };
        let mut main_pos = leading;
        for pod in &mut self.children {
            let child_cross = axis.cross_of(pod.size());
            let cross_pos = match self.cross {
                CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => 0.0,
                CrossAxisAlignment::Center => (cross_size - child_cross) / 2.0,
            };
            pod.set_origin(axis.point(main_pos, cross_pos));
            main_pos += axis.main_of(pod.size());
        }

        bc.constrain(axis.size(main_size, cross_size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Paint in child order (first child painted first / bottom-most).
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Hit-test in reverse paint order (topmost/last-painted child first).
        crate::route_event(&mut self.children, ctx, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf};

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build<S: 'static>(view: &FlexView<S>) -> FlexWidget {
        let mut counter = 0u64;
        view.build(&mut ctx(&mut counter))
    }

    #[test]
    fn distributes_free_space_by_flex_factors() {
        // Two flexible children, factors 2:1, under a 300px-wide bound.
        // free = 300 (no inflexible children) → shares 200 and 100.
        let view: FlexView<()> = FlexView::new(
            Axis::Horizontal,
            vec![
                flexible(2, leaf(1000.0, 20.0)),
                flexible(1, leaf(1000.0, 20.0)),
            ],
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        assert_eq!(w.children[0].size().width, 200.0);
        assert_eq!(w.children[1].size().width, 100.0);
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 200.0);
        // Flexible children present → main axis fills the 300px bound.
        assert_eq!(size.width, 300.0);
    }

    #[test]
    fn mixes_inflexible_and_flexible() {
        // One inflexible 50px child, one flexible child, under 200px.
        // free = 200 - 50 = 150 → the flexible child takes all 150.
        let view: FlexView<()> = FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(leaf(50.0, 20.0)),
                flexible(1, leaf(1000.0, 20.0)),
            ],
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));

        assert_eq!(w.children[0].size().width, 50.0);
        assert_eq!(w.children[1].size().width, 150.0);
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 50.0);
        assert_eq!(size.width, 200.0);
    }

    #[test]
    fn shrink_wraps_main_axis_without_flexible_children() {
        // No flexible children → main extent is the sum of child widths (60),
        // not the 300px bound.
        let view: FlexView<()> = Row(vec![
            leaf(40.0, 10.0).into_any(),
            leaf(20.0, 10.0).into_any(),
        ]);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        assert_eq!(size.width, 60.0);
        assert_eq!(w.children[1].origin().x, 40.0);
    }

    #[test]
    fn cross_axis_stretch_tightens_children() {
        // Stretch → every child gets a tight cross constraint = the 80px bound,
        // overriding its 10px intrinsic height.
        let view: FlexView<()> =
            Row(vec![leaf(30.0, 10.0).into_any()]).cross_axis(CrossAxisAlignment::Stretch);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 80.0)));
        assert_eq!(w.children[0].size().height, 80.0);
        assert_eq!(size.height, 80.0);
    }

    #[test]
    fn cross_axis_start_packs_at_zero() {
        // Start → the shorter child sits at cross 0. cross_size = tallest = 40.
        let view: FlexView<()> = Row(vec![
            leaf(10.0, 40.0).into_any(),
            leaf(10.0, 20.0).into_any(),
        ])
        .cross_axis(CrossAxisAlignment::Start);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(size.height, 40.0);
        assert_eq!(w.children[1].origin().y, 0.0);
    }

    #[test]
    fn cross_axis_center_centers_shorter_children() {
        // Center → cross_size = 40; the 20px-tall child is centered at (40-20)/2.
        let view: FlexView<()> = Row(vec![
            leaf(10.0, 40.0).into_any(),
            leaf(10.0, 20.0).into_any(),
        ])
        .cross_axis(CrossAxisAlignment::Center);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 100.0)));
        assert_eq!(w.children[1].origin().y, 10.0);
    }

    #[test]
    fn column_lays_out_along_vertical_axis() {
        // A Column stacks children top-to-bottom: main = height.
        let view: FlexView<()> = Column(vec![
            leaf(30.0, 15.0).into_any(),
            leaf(30.0, 25.0).into_any(),
        ]);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 300.0)));
        assert_eq!(size.height, 40.0); // 15 + 25
        assert_eq!(w.children[0].origin().y, 0.0);
        assert_eq!(w.children[1].origin().y, 15.0);
    }

    #[test]
    fn paints_children_in_order() {
        let view: FlexView<()> = Row(vec![
            leaf(20.0, 20.0).into_any(),
            leaf(20.0, 20.0).into_any(),
        ]);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));

        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 200.0));
        w.paint(&mut pctx, &mut scene);
        // Child 0 painted at x=0, child 1 at x=20 — in child order.
        assert_eq!(scene.rects[0].0, Point::new(0.0, 0.0));
        assert_eq!(scene.rects[1].0, Point::new(20.0, 0.0));
    }

    #[test]
    fn children_vec_diff_adds_removes_and_type_swaps() {
        // Start with two Leaf children.
        let mut counter = 0u64;
        let prev: FlexView<()> = Row(vec![
            leaf(10.0, 10.0).into_any(),
            leaf(10.0, 10.0).into_any(),
        ]);
        let mut w = prev.build(&mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);

        // Grow to three.
        let grown: FlexView<()> = Row(vec![
            leaf(10.0, 10.0).into_any(),
            leaf(10.0, 10.0).into_any(),
            leaf(10.0, 10.0).into_any(),
        ]);
        let flags = grown.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 3);
        assert!(flags.needs_layout());

        // Shrink to one.
        let shrunk: FlexView<()> = Row(vec![leaf(10.0, 10.0).into_any()]);
        shrunk.rebuild(&grown, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 1);
        assert_eq!(w.flex.len(), 1);

        // Type-swap the sole child (Leaf → the other test widget via AnyView).
        let swapped: FlexView<()> = Row(vec![crate::test_support::swap_leaf().into_any()]);
        swapped.rebuild(&shrunk, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 1);
        // The swapped widget reports a distinctive size, proving the swap took.
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(w.children[0].size(), Size::new(7.0, 7.0));
    }

    // --- Capture-vs-rebuild fixtures (review R5) ---------------------------
    //
    // A vertical list of fixed 50x20 rows, each of which captures on `Down` and
    // "fires" (records its id into the `Vec<u32>` app state) only on an `Up`
    // while still armed. `Cancel` disarms WITHOUT touching app state — which is
    // what makes the rebuild-path synthetic cancel (driven over a `()` dummy
    // state) sound; a Cancel arm that read state would panic on the `()`
    // downcast, so these tests also guard that contract.

    use std::any::Any;
    use std::cell::Cell;
    use std::rc::Rc;

    use forgekit_core::{EventCtx, PointerButton, PointerEvent, PointerPhase, any};

    const ROW_W: f64 = 50.0;
    const ROW_H: f64 = 20.0;

    /// A row that captures on `Down` and fires its id on up-inside.
    struct Captor {
        id: u32,
    }
    /// Retained widget for [`Captor`].
    struct CaptorWidget {
        id: u32,
        armed: bool,
    }

    /// Erase a [`Captor`] tagged `id` into an `AnyView<Vec<u32>>`.
    fn captor(id: u32) -> AnyView<Vec<u32>> {
        any(Captor { id })
    }

    impl View<Vec<u32>> for Captor {
        type Element = CaptorWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CaptorWidget {
            CaptorWidget {
                id: self.id,
                armed: false,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CaptorWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.id = self.id;
            ChangeFlags::NONE
        }
    }

    impl Widget for CaptorWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            match p.phase {
                PointerPhase::Down => {
                    self.armed = true;
                    ctx.capture_pointer();
                    EventResult::Handled
                }
                PointerPhase::Move => EventResult::Handled,
                PointerPhase::Up => {
                    if self.armed {
                        ctx.state_mut::<Vec<u32>>().push(self.id);
                    }
                    self.armed = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // Clears armed WITHOUT reading app state (g2 contract).
                    self.armed = false;
                    EventResult::Handled
                }
            }
        }
    }

    /// A row that records into a shared cell that it saw *any* event — used to
    /// prove a freshly type-swapped widget receives nothing until a new `Down`.
    struct Recorder {
        seen: Rc<Cell<u32>>,
    }
    /// Retained widget for [`Recorder`].
    struct RecorderWidget {
        seen: Rc<Cell<u32>>,
    }

    impl View<Vec<u32>> for Recorder {
        type Element = RecorderWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RecorderWidget {
            RecorderWidget {
                seen: self.seen.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut RecorderWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for RecorderWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            self.seen.set(self.seen.get() + 1);
            EventResult::Handled
        }
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// Dispatch one event to the flex over a `Vec<u32>` fire-log state.
    ///
    /// The log stays a `Vec<u32>` (not a slice) because it is erased as
    /// `&mut dyn Any` and recovered by the widgets via `state_mut::<Vec<u32>>()`.
    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut FlexWidget, log: &mut Vec<u32>, event: &InputEvent) {
        let state: &mut dyn Any = log;
        let mut ectx = EventCtx::new(state, Point::ZERO, Size::new(ROW_W, ROW_H * 8.0));
        w.event(&mut ectx, event);
    }

    /// Lay a Captor/Recorder column out so rows sit at y = i * ROW_H.
    fn layout_column(w: &mut FlexWidget) {
        let mut lctx = LayoutCtx::new();
        w.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(ROW_W, ROW_H * 8.0)),
        );
    }

    /// Y within row `i` (its vertical midpoint).
    fn row_y(i: usize) -> f64 {
        i as f64 * ROW_H + ROW_H / 2.0
    }

    #[test]
    fn structural_insert_cancels_inflight_drag_no_fire() {
        // (Scenario 1) Drag armed in row 1; a rebuild inserts a row above
        // (length grows). On Up: NO callback fires, and the original row's
        // gesture state is cleared.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = Column(vec![captor(0), captor(1), captor(2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active(), "row 1 captured the pointer");

        // Rebuild: insert a new row at the top → positions shift, length grows.
        let inserted: FlexView<Vec<u32>> = Column(vec![captor(9), captor(0), captor(1), captor(2)]);
        inserted.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children.iter().all(|p| !p.is_active()),
            "structural change cleared every active path"
        );

        // Re-layout for the new row count, then release. Nothing is armed.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert!(
            log.is_empty(),
            "no row fires on Up after an in-flight cancel"
        );
    }

    #[test]
    fn structural_truncation_of_active_row_unwinds_without_panic() {
        // (Scenario 2) Drag armed in row 2; a rebuild truncates the list to two
        // rows, dropping the active row. teardown_child cancels it: no panic, no
        // fire.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = Column(vec![captor(0), captor(1), captor(2), captor(3)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(2)));
        assert!(w.children[2].is_active());

        // Truncate to two rows — the active row 2 is dropped.
        let truncated: FlexView<Vec<u32>> = Column(vec![captor(0), captor(1)]);
        truncated.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);
        assert_eq!(w.flex.len(), 2);
        assert!(w.children.iter().all(|p| !p.is_active()));

        // A release lands nowhere armed → no fire, no panic.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(2)));
        assert!(log.is_empty());
    }

    #[test]
    fn type_swap_at_active_index_clears_without_notifying_fresh_widget() {
        // (Scenario 3) A type swap at the active index clears the stale capture
        // but does NOT deliver anything to the fresh widget — it must see nothing
        // until a new Down.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = Column(vec![captor(0), captor(1)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // Swap row 1 from Captor to a Recorder (a different concrete type).
        let seen = Rc::new(Cell::new(0u32));
        let swapped: FlexView<Vec<u32>> =
            Column(vec![captor(0), any(Recorder { seen: seen.clone() })]);
        swapped.rebuild(&prev, &mut w, &mut ctx(&mut counter));

        assert!(!w.children[1].is_active(), "stale capture path dropped");
        assert_eq!(seen.get(), 0, "fresh widget received no synthetic event");

        // A brand-new Down now reaches the fresh widget.
        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(seen.get(), 1, "fresh widget responds to a new gesture");
    }

    #[test]
    fn content_only_rebuild_preserves_captured_drag() {
        // (Scenario 4, the critical negative test) A structural-change-free
        // rebuild (same length, same types) must NOT break a captured drag: the
        // active path survives and the release still fires on the captured row.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = Column(vec![captor(0), captor(1)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // An ordinary every-frame rebuild: same structure, content only.
        let same: FlexView<Vec<u32>> = Column(vec![captor(0), captor(1)]);
        same.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_active(),
            "content-only rebuild must NOT clear an in-flight capture"
        );

        // The captured drag completes and fires on the still-armed row.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert_eq!(log, vec![1], "captured row fires on Up as normal");
    }

    // --- Keyed reconciliation fixtures (task 58) --------------------------
    //
    // A stateful probe row: `CounterWidget` holds an internal `count` that starts
    // at 0 on build and increments on every `Down`, pushing the post-increment
    // value into the `Vec<u32>` app state. Its rebuild deliberately does NOT reset
    // `count`, so the pushed sequence reveals whether a reconciliation *relocated*
    // the live widget (count continues) or *rebuilt* it from scratch (count resets
    // to 1). This is the probe the reorder-preserves-state test turns on.

    /// A stateful counter row view tagged with `id`.
    struct Counter {
        id: u32,
    }
    /// Retained widget for [`Counter`]: `count` survives an in-place rebuild.
    struct CounterWidget {
        id: u32,
        count: u32,
    }

    impl View<Vec<u32>> for Counter {
        type Element = CounterWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterWidget {
            CounterWidget {
                id: self.id,
                count: 0,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CounterWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // Adopt the new id but preserve the accumulated count — a relocated
            // widget must keep its internal state.
            element.id = self.id;
            ChangeFlags::NONE
        }
    }

    impl Widget for CounterWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            if p.phase == PointerPhase::Down {
                self.count += 1;
                ctx.state_mut::<Vec<u32>>().push(self.count);
                EventResult::Handled
            } else {
                EventResult::Ignored
            }
        }
    }

    /// A keyed inflexible counter child.
    fn kcounter(key: u64, id: u32) -> FlexChild<Vec<u32>> {
        keyed(key, Counter { id })
    }

    /// Build a vertical keyed column of counter rows.
    fn keyed_column(children: Vec<FlexChild<Vec<u32>>>) -> FlexView<Vec<u32>> {
        FlexView::new(Axis::Vertical, children)
    }

    #[test]
    fn keyed_reorder_preserves_widget_state() {
        // THE CRITICAL TEST. Two keyed counter rows; drive row A's internal count
        // up, reorder the list, then drive A again — its count must continue from
        // where it left off, proving the reorder relocated A's live widget rather
        // than rebuilding whatever now sits at A's old index.
        let mut counter = 0u64;
        let prev = keyed_column(vec![kcounter(1, 1), kcounter(2, 2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Row A (key 1) at index 0: three Downs → its internal count reaches 3.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert_eq!(log, vec![1, 2, 3], "count accumulates on the original row");
        log.clear();

        // Reorder: [B, A]. Row A moves to index 1.
        let reordered = keyed_column(vec![kcounter(2, 2), kcounter(1, 1)]);
        reordered.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);
        layout_column(&mut w);

        // Drive row A at its NEW index (1). If its widget was relocated, the count
        // continues to 4; a from-scratch rebuild would reset it to 1.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(
            log,
            vec![4],
            "reordered row kept its internal state (4, not a reset 1)"
        );
    }

    #[test]
    fn keyed_insert_above_preserves_existing_widget_state() {
        // Inserting a new keyed row above the existing ones must not rebuild them:
        // the surviving rows relocate (state preserved), only the new key builds.
        let mut counter = 0u64;
        let prev = keyed_column(vec![kcounter(1, 1), kcounter(2, 2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Row with key 2 (index 1): two Downs → count 2.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(log, vec![1, 2]);
        log.clear();

        // Insert a fresh key 9 at the top: [9, 1, 2]. Key 2 shifts to index 2.
        let inserted = keyed_column(vec![kcounter(9, 9), kcounter(1, 1), kcounter(2, 2)]);
        inserted.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 3);
        layout_column(&mut w);

        // Key 2 at its new index (2) continues its count to 3, not a reset 1.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(2)));
        assert_eq!(log, vec![3], "shifted row preserved its state");
        log.clear();

        // The freshly-built key 9 (index 0) starts its own count at 1.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert_eq!(log, vec![1], "newly-inserted key builds a fresh widget");
    }

    #[test]
    fn keyed_swap_preserves_both_widgets() {
        // A straight two-row swap must preserve *both* rows' state.
        let mut counter = 0u64;
        let prev = keyed_column(vec![kcounter(1, 1), kcounter(2, 2)]);
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // A (key 1, idx 0) → count 1; B (key 2, idx 1) → count 1 then 2.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(log, vec![1, 1, 2]);
        log.clear();

        // Swap → [B, A].
        let swapped = keyed_column(vec![kcounter(2, 2), kcounter(1, 1)]);
        swapped.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        layout_column(&mut w);

        // B now at idx 0 continues to 3; A now at idx 1 continues to 2.
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert_eq!(log, vec![3, 2], "both swapped rows kept their state");
    }

    #[test]
    fn keyed_same_order_rebuild_is_not_structural() {
        // A same-keys, same-order keyed rebuild is the content-only case: it must
        // NOT clear an in-flight capture (mirrors the positional negative test).
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(2u64, Captor { id: 1 })],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // Same keys, same order → not structural.
        let same: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(2u64, Captor { id: 1 })],
        );
        same.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children[1].is_active(),
            "same-order keyed rebuild must not clear an in-flight capture"
        );

        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert_eq!(log, vec![1], "captured row fires on Up as normal");
    }

    #[test]
    fn keyed_reorder_cancels_inflight_gesture_no_fire() {
        // An in-flight capture across a keyed reorder is cancelled (g5 pattern):
        // no callback fires on the later Up.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(2u64, Captor { id: 1 })],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Arm the capture in the key-1 row (index 0).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert!(w.children[0].is_active());

        // Reorder → [key2, key1]. The reorder is structural → cancel.
        let reordered: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(2u64, Captor { id: 1 }), keyed(1u64, Captor { id: 0 })],
        );
        reordered.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children.iter().all(|p| !p.is_active()),
            "keyed reorder cleared every active path"
        );

        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert!(log.is_empty(), "no row fires on Up after a reorder cancel");
    }

    #[test]
    fn keyed_removed_active_key_unwinds_without_fire() {
        // Removing a key whose row holds an in-flight capture tears it down via
        // the cancel-if-active path: no fire, no panic.
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(1u64, Captor { id: 0 }),
                keyed(2u64, Captor { id: 1 }),
                keyed(3u64, Captor { id: 2 }),
            ],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Arm the key-2 row (index 1).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(1)));
        assert!(w.children[1].is_active());

        // Drop key 2 → [key1, key3].
        let removed: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![keyed(1u64, Captor { id: 0 }), keyed(3u64, Captor { id: 2 })],
        );
        removed.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.children.len(), 2);
        assert_eq!(w.flex.len(), 2);
        assert!(w.children.iter().all(|p| !p.is_active()));

        layout_column(&mut w);
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 10.0, row_y(1)));
        assert!(log.is_empty(), "removed active row does not fire on Up");
    }

    #[test]
    fn keyed_reorder_clears_focus() {
        // Focus is the second recorded path: any keyed reorder clears it, exactly
        // like the capture path (conservative v1 — see cancel_active_children).
        let mut counter = 0u64;
        let prev: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(1u64, FocusRow { id: 1 }),
                keyed(2u64, FocusRow { id: 2 }),
            ],
        );
        let mut w = prev.build(&mut ctx(&mut counter));
        layout_column(&mut w);

        let mut log: Vec<u32> = Vec::new();
        // Focus the key-1 row (index 0).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, row_y(0)));
        assert!(w.children[0].is_focused());

        // Reorder → [key2, key1]. Structural → focus chain cleared.
        let reordered: FlexView<Vec<u32>> = FlexView::new(
            Axis::Vertical,
            vec![
                keyed(2u64, FocusRow { id: 2 }),
                keyed(1u64, FocusRow { id: 1 }),
            ],
        );
        reordered.rebuild(&prev, &mut w, &mut ctx(&mut counter));
        assert!(
            w.children.iter().all(|p| !p.is_focused()),
            "keyed reorder cleared the focus path"
        );
    }

    /// A focus-taking row: requests focus on `Down`, records `id` on a Key event.
    struct FocusRow {
        id: u32,
    }
    /// Retained widget for [`FocusRow`].
    struct FocusRowWidget {
        id: u32,
    }

    impl View<Vec<u32>> for FocusRow {
        type Element = FocusRowWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FocusRowWidget {
            FocusRowWidget { id: self.id }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut FocusRowWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.id = self.id;
            ChangeFlags::NONE
        }
    }

    impl Widget for FocusRowWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(ROW_W, ROW_H))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.request_focus();
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }
}
