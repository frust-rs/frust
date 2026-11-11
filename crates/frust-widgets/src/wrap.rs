//! Wrapping layout container: children flow along a main axis and break into
//! runs, mirroring Flutter's `Wrap`.
//!
//! [`WrapView`]/[`WrapWidget`] are the declarative/retained pair (mirroring
//! [`crate::flex`]'s `FlexView`/`FlexWidget`). Each child is measured with the
//! main axis loose up to the wrap's maximum main extent; a child that would
//! overflow the current run starts a new one. The wrap sizes to its longest run
//! along the main axis and to the summed run extents (plus `run_spacing`) along
//! the cross axis. Under an unbounded main axis nothing ever overflows, so the
//! result is a single run.
//!
//! ```
//! use frust_widgets::{WrapAlignment, WrapView, text, wrap};
//! # fn demo() -> WrapView<()> {
//! wrap()
//!     .spacing(8.0)
//!     .run_spacing(4.0)
//!     .alignment(WrapAlignment::Center)
//!     .child(text("one"))
//!     .child(text("two"))
//! # }
//! # let _ = demo();
//! ```

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, ViewSeq, Widget, any,
};
use kurbo::{Point, Rect, Size};

use crate::{Axis, ChildKey};

/// How runs (along the main axis) or the runs themselves (along the cross
/// axis) are distributed within the free space of a [`WrapView`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WrapAlignment {
    /// Pack against the start.
    #[default]
    Start,
    /// Pack against the end.
    End,
    /// Pack in the middle.
    Center,
    /// Free space split evenly between items; none before the first or after the
    /// last.
    SpaceBetween,
    /// Free space split evenly between items, half of one share at each end.
    SpaceAround,
    /// Free space split evenly between items and at both ends.
    SpaceEvenly,
}

/// How children are placed on the cross axis within their own run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WrapCrossAlignment {
    /// Align to the start of the run.
    #[default]
    Start,
    /// Align to the end of the run.
    End,
    /// Center within the run.
    Center,
}

impl WrapAlignment {
    /// The `(leading, between)` offsets for `count` items sharing `free` space.
    fn distribute(self, free: f64, count: usize) -> (f64, f64) {
        let free = free.max(0.0);
        let n = count as f64;
        match self {
            WrapAlignment::Start => (0.0, 0.0),
            WrapAlignment::End => (free, 0.0),
            WrapAlignment::Center => (free / 2.0, 0.0),
            WrapAlignment::SpaceBetween if count > 1 => (0.0, free / (n - 1.0)),
            WrapAlignment::SpaceBetween => (0.0, 0.0),
            WrapAlignment::SpaceAround if count > 0 => (free / n / 2.0, free / n),
            WrapAlignment::SpaceAround => (0.0, 0.0),
            WrapAlignment::SpaceEvenly => (free / (n + 1.0), free / (n + 1.0)),
        }
    }
}

fn main_of(axis: Axis, size: Size) -> f64 {
    match axis {
        Axis::Horizontal => size.width,
        Axis::Vertical => size.height,
    }
}

fn cross_of(axis: Axis, size: Size) -> f64 {
    match axis {
        Axis::Horizontal => size.height,
        Axis::Vertical => size.width,
    }
}

fn size_of(axis: Axis, main: f64, cross: f64) -> Size {
    match axis {
        Axis::Horizontal => Size::new(main, cross),
        Axis::Vertical => Size::new(cross, main),
    }
}

fn point_of(axis: Axis, main: f64, cross: f64) -> Point {
    match axis {
        Axis::Horizontal => Point::new(main, cross),
        Axis::Vertical => Point::new(cross, main),
    }
}

/// Keep a spacing only when it is finite and positive.
fn sanitize_spacing(spacing: f64) -> f64 {
    if spacing.is_finite() && spacing > 0.0 {
        spacing
    } else {
        0.0
    }
}

struct WrapChild<State: 'static> {
    view: AnyView<State>,
    key: Option<ChildKey>,
}

/// A declarative wrapping container. See the [module docs](self).
pub struct WrapView<State: 'static> {
    direction: Axis,
    spacing: f64,
    run_spacing: f64,
    alignment: WrapAlignment,
    run_alignment: WrapAlignment,
    cross_alignment: WrapCrossAlignment,
    children: Vec<WrapChild<State>>,
}

/// An empty horizontal wrap, ready for fluent children.
pub fn wrap<State: 'static>() -> WrapView<State> {
    WrapView {
        direction: Axis::Horizontal,
        spacing: 0.0,
        run_spacing: 0.0,
        alignment: WrapAlignment::Start,
        run_alignment: WrapAlignment::Start,
        cross_alignment: WrapCrossAlignment::Start,
        children: Vec::new(),
    }
}

impl<State: 'static> WrapView<State> {
    /// Append a child. Accepts any [`View`]; erasure happens internally.
    pub fn child<V: View<State>>(mut self, view: V) -> Self {
        self.children.push(WrapChild {
            view: any(view),
            key: None,
        });
        self
    }

    /// Append a child tagged with a stable [`ChildKey`], so its state survives a
    /// reorder. The all-or-nothing rule of [`crate::keyed`] applies.
    pub fn keyed<V: View<State>>(mut self, key: impl Into<ChildKey>, view: V) -> Self {
        self.children.push(WrapChild {
            view: any(view),
            key: Some(key.into()),
        });
        self
    }

    /// Append every element of `children` (any [`ViewSeq`]) as an unkeyed child.
    pub fn children<M>(mut self, children: impl ViewSeq<State, M>) -> Self {
        let mut erased = Vec::new();
        children.extend_views(&mut erased);
        self.children
            .extend(erased.into_iter().map(|view| WrapChild { view, key: None }));
        self
    }

    /// Apply `f` to the builder only when `cond` is true.
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    /// Apply `f` with the contained value when `opt` is `Some`.
    pub fn when_some<T>(self, opt: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match opt {
            Some(value) => f(self, value),
            None => self,
        }
    }

    /// Set the main axis children flow along (default horizontal).
    pub fn direction(mut self, direction: Axis) -> Self {
        self.direction = direction;
        self
    }

    /// Set the space between children within a run. A negative, NaN or infinite
    /// value is treated as `0`.
    pub fn spacing(mut self, spacing: f64) -> Self {
        self.spacing = sanitize_spacing(spacing);
        self
    }

    /// Set the space between runs. A negative, NaN or infinite value is treated
    /// as `0`.
    pub fn run_spacing(mut self, run_spacing: f64) -> Self {
        self.run_spacing = sanitize_spacing(run_spacing);
        self
    }

    /// Set how children are distributed along the main axis within each run.
    pub fn alignment(mut self, alignment: WrapAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Set how runs are distributed along the cross axis.
    pub fn run_alignment(mut self, run_alignment: WrapAlignment) -> Self {
        self.run_alignment = run_alignment;
        self
    }

    /// Set how children are placed on the cross axis within their run.
    pub fn cross_alignment(mut self, cross_alignment: WrapCrossAlignment) -> Self {
        self.cross_alignment = cross_alignment;
        self
    }
}

/// The retained widget for a [`WrapView`].
pub struct WrapWidget {
    direction: Axis,
    spacing: f64,
    run_spacing: f64,
    alignment: WrapAlignment,
    run_alignment: WrapAlignment,
    cross_alignment: WrapCrossAlignment,
    children: Vec<ChildPod>,
}

/// One line of children: the index range, its main extent (children plus
/// spacing) and its cross extent (tallest child).
struct Run {
    start: usize,
    end: usize,
    main: f64,
    cross: f64,
}

impl<State: 'static> View<State> for WrapView<State> {
    type Element = WrapWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> WrapWidget {
        let children = self
            .children
            .iter()
            .map(|child| crate::authoring::build_child(&child.view, ctx))
            .collect();
        WrapWidget {
            direction: self.direction,
            spacing: self.spacing,
            run_spacing: self.run_spacing,
            alignment: self.alignment,
            run_alignment: self.run_alignment,
            cross_alignment: self.cross_alignment,
            children,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut WrapWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.direction != self.direction
            || prev.spacing != self.spacing
            || prev.run_spacing != self.run_spacing
            || prev.alignment != self.alignment
            || prev.run_alignment != self.run_alignment
            || prev.cross_alignment != self.cross_alignment
        {
            element.direction = self.direction;
            element.spacing = self.spacing;
            element.run_spacing = self.run_spacing;
            element.alignment = self.alignment;
            element.run_alignment = self.run_alignment;
            element.cross_alignment = self.cross_alignment;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= crate::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |child| &child.view,
            |child| child.key,
        );
        flags
    }

    fn teardown(&self, element: &mut WrapWidget, ctx: &mut BuildCtx<'_>) {
        for (child, pod) in self.children.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(&child.view, pod, ctx);
        }
    }
}

impl Widget for WrapWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let axis = self.direction;
        let max_main = main_of(axis, bc.max());
        let max_cross = cross_of(axis, bc.max());

        // Measure every child with the main axis loose up to the wrap's bound
        // (so an oversized child is clamped to it) and the cross axis loose.
        let child_bc =
            BoxConstraints::new(size_of(axis, 0.0, 0.0), size_of(axis, max_main, max_cross));
        let sizes: Vec<Size> = self
            .children
            .iter_mut()
            .map(|pod| pod.layout_child(ctx, &child_bc))
            .collect();

        // Break into runs: a child that would overflow a non-empty run starts a
        // new one. An unbounded main axis never overflows.
        let mut runs: Vec<Run> = Vec::new();
        let mut current: Option<Run> = None;
        for (i, size) in sizes.iter().enumerate() {
            let main = main_of(axis, *size);
            let cross = cross_of(axis, *size);
            match current.as_mut() {
                Some(run) if run.main + self.spacing + main <= max_main => {
                    run.main += self.spacing + main;
                    run.cross = run.cross.max(cross);
                    run.end = i + 1;
                }
                _ => {
                    if let Some(done) = current.take() {
                        runs.push(done);
                    }
                    current = Some(Run {
                        start: i,
                        end: i + 1,
                        main,
                        cross,
                    });
                }
            }
        }
        runs.extend(current);

        let content_main = runs.iter().map(|r| r.main).fold(0.0, f64::max);
        let content_cross = runs.iter().map(|r| r.cross).sum::<f64>()
            + self.run_spacing * runs.len().saturating_sub(1) as f64;
        let size = bc.constrain(size_of(axis, content_main, content_cross));
        let container_main = main_of(axis, size);
        let container_cross = cross_of(axis, size);

        let (run_leading, run_between) = self
            .run_alignment
            .distribute(container_cross - content_cross, runs.len());
        let mut cross_pos = run_leading;
        for run in &runs {
            let count = run.end - run.start;
            let (leading, between) = self.alignment.distribute(container_main - run.main, count);
            let mut main_pos = leading;
            for pod in &mut self.children[run.start..run.end] {
                let child_cross = cross_of(axis, pod.size());
                let offset = match self.cross_alignment {
                    WrapCrossAlignment::Start => 0.0,
                    WrapCrossAlignment::End => run.cross - child_cross,
                    WrapCrossAlignment::Center => (run.cross - child_cross) / 2.0,
                };
                pod.set_origin(point_of(axis, main_pos, cross_pos + offset));
                main_pos += main_of(axis, pod.size()) + self.spacing + between;
            }
            cross_pos += run.cross + self.run_spacing + run_between;
        }

        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Paint-time culling against the visible rect a scroll ancestor threads
        // (one viewport of warm margin each side). Focused children and any
        // child during a hero transition are exempt, as in `FlexWidget`.
        let cull = ctx
            .visible_rect()
            .map(|vr| vr.inflate(vr.width(), vr.height()));
        let hero_in_flight = ctx.hero_active();
        for pod in &mut self.children {
            if let Some(warm) = cull {
                let exempt = hero_in_flight || pod.is_focused();
                if !exempt {
                    let child_abs =
                        Rect::from_origin_size(ctx.origin() + pod.origin().to_vec2(), pod.size());
                    if !child_abs.overlaps(warm) {
                        continue;
                    }
                }
            }
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    // Lays out its children and paints nothing of its own.
    fn hit_test_self(&self) -> bool {
        false
    }

    crate::authoring::visit_children!(children);
}
