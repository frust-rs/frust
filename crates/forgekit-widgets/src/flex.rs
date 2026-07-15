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

/// One child of a [`FlexView`]: an erased child view plus its `flex` factor
/// (`0` = inflexible; `> 0` = takes a proportional share of the free main space).
pub struct FlexChild<State: 'static> {
    view: AnyView<State>,
    flex: u32,
}

/// A flexible child taking `flex` proportional shares of the free main-axis space.
pub fn flexible<State: 'static, V: View<State>>(flex: u32, view: V) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex,
    }
}

/// An inflexible child, sized to its natural main-axis extent.
pub fn inflexible<State: 'static, V: View<State>>(view: V) -> FlexChild<State> {
    FlexChild {
        view: any(view),
        flex: 0,
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
            .map(|view| FlexChild { view, flex: 0 })
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
            .map(|view| FlexChild { view, flex: 0 })
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

        let common = prev.children.len().min(self.children.len());
        for i in 0..common {
            flags |= crate::rebuild_child(
                &prev.children[i].view,
                &self.children[i].view,
                &mut element.children[i],
                ctx,
            );
            if prev.children[i].flex != self.children[i].flex {
                element.flex[i] = self.children[i].flex;
                flags |= ChangeFlags::LAYOUT;
            }
        }

        if self.children.len() > prev.children.len() {
            for child in &self.children[prev.children.len()..] {
                element.children.push(crate::build_child(&child.view, ctx));
                element.flex.push(child.flex);
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if self.children.len() < prev.children.len() {
            for (offset, child) in prev.children[self.children.len()..].iter().enumerate() {
                let idx = self.children.len() + offset;
                crate::teardown_child(&child.view, &mut element.children[idx], ctx);
            }
            element.children.truncate(self.children.len());
            element.flex.truncate(self.children.len());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
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
}
