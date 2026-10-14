//! `sliver_fill_remaining`: one box child sized to fill what is left of the
//! viewport — Flutter's `SliverFillRemaining` (`rendering/sliver_fill.dart`).
//! Meant as the last sliver of a [`CustomScrollView`](crate::CustomScrollView).
//!
//! Two modes, picked by [`SliverFillRemainingView::has_scroll_body`]:
//!
//! * **With a scroll body** (the default, Flutter's
//!   `RenderSliverFillRemainingWithScrollable`): the child is laid out tight
//!   to the remaining paint extent (less a negative overlap), and the sliver
//!   scrolls by a whole viewport extent — so the slivers before it can scroll
//!   away and the child grows to the full viewport as they do. A child placed
//!   beyond the paint space but inside the cache area is laid out up to the
//!   cached extent instead of at zero, so it is ready when it scrolls in.
//! * **Without** (`has_scroll_body(false)`, Flutter's
//!   `RenderSliverFillRemaining`): the child fills the viewport extent the
//!   slivers before it leave (`viewport_main_axis_extent −
//!   preceding_scroll_extent`, never negative), or its own extent when that is
//!   larger. The sliver's scroll extent is that extent, so it adds scrolling
//!   only when the child is taller than the space left. Where Flutter asks the
//!   child for its max intrinsic extent, the child here is laid out once with
//!   the space left as its *minimum* extent and an unbounded maximum, and the
//!   larger of the two is kept.
//!
//! Either way the child is placed at `−scroll_offset` and clipped to the paint
//! extent when cut, exactly as [`sliver_box`](crate::sliver_box) places its
//! child. Under a shrink-wrapping viewport (an infinite remaining paint
//! extent) there is no remainder to fill and the child keeps its own extent.
//! Laid out as a box itself (outside a viewport), the sliver is transparent:
//! the child is laid out under the incoming constraints.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, SliverConstraints, SliverGeometry, SliverWidget,
    View, Widget, any,
};
use kurbo::{Point, Size};

use super::adapter::{box_child_origin, box_geometry, finite_extent};
use super::{cache_offset, paint_offset};

/// A box child filling the rest of the viewport. See the [module docs](self).
pub struct SliverFillRemainingView<State: 'static> {
    child: AnyView<State>,
    has_scroll_body: bool,
}

/// Fill what is left of the viewport with `child` — with a scroll body by
/// default; see [`SliverFillRemainingView::has_scroll_body`] and the
/// [module docs](self).
///
/// ```
/// use frust_widgets::{CustomScrollView, custom_scroll_view, sliver_fill_remaining, text};
/// let view: CustomScrollView<()> = custom_scroll_view((
///     text("header"),
///     sliver_fill_remaining(text("the rest")).has_scroll_body(false),
/// ));
/// # let _ = view;
/// ```
pub fn sliver_fill_remaining<State: 'static, V: View<State>>(
    child: V,
) -> SliverFillRemainingView<State> {
    SliverFillRemainingView {
        child: any(child),
        has_scroll_body: true,
    }
}

impl<State: 'static> SliverFillRemainingView<State> {
    /// Whether the child is (or holds) its own scrollable body — `true` by
    /// default. `true` sizes the child to exactly the remaining paint extent
    /// and scrolls the sliver by a whole viewport; `false` sizes it to the
    /// space the earlier slivers leave, or its own extent when larger, and
    /// scrolls only by that. See the [module docs](self).
    pub fn has_scroll_body(mut self, has_scroll_body: bool) -> Self {
        self.has_scroll_body = has_scroll_body;
        self
    }
}

/// The retained widget for a [`SliverFillRemainingView`]: the child's pod,
/// the mode, and whether the last sliver layout cut the child (so paint
/// clips it).
pub struct SliverFillRemainingWidget {
    child: ChildPod,
    has_scroll_body: bool,
    /// Whether the last layout left part of the child outside this sliver's
    /// paint extent. Always `false` after a plain box layout.
    clip: bool,
}

impl<State: 'static> View<State> for SliverFillRemainingView<State> {
    type Element = SliverFillRemainingWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SliverFillRemainingWidget {
        SliverFillRemainingWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            has_scroll_body: self.has_scroll_body,
            clip: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverFillRemainingWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.has_scroll_body != self.has_scroll_body {
            element.has_scroll_body = self.has_scroll_body;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SliverFillRemainingWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl SliverFillRemainingWidget {
    /// The scroll-body mode: the child tight to the remaining paint extent,
    /// the sliver scrolling by a whole viewport. `None` when there is no
    /// finite remainder to fill (a shrink-wrapping viewport).
    fn layout_scroll_body(
        &mut self,
        ctx: &mut LayoutCtx,
        sc: &SliverConstraints,
    ) -> Option<SliverGeometry> {
        let extent = sc.remaining_paint_extent - sc.overlap.min(0.0);
        if !extent.is_finite() {
            return None;
        }
        let viewport = finite_extent(sc.viewport_main_axis_extent);
        let cache = cache_offset(sc, 0.0, viewport);
        // Beyond the paint space but inside the cache area: lay the child out
        // up to the cached extent, so it is ready when it scrolls in.
        let max_extent = if extent == 0.0 && cache > 0.0 {
            cache
        } else {
            extent
        };
        let axis = sc.axis;
        let cross = sc.cross_axis_extent;
        self.child.layout_child(
            ctx,
            &BoxConstraints::new(axis.size(extent, cross), axis.size(max_extent, cross)),
        );
        let paint_extent = paint_offset(sc, 0.0, extent);
        Some(SliverGeometry {
            cache_extent: cache.max(paint_extent),
            has_visual_overflow: extent > sc.remaining_paint_extent || sc.scroll_offset > 0.0,
            ..SliverGeometry::new(viewport, paint_extent, paint_extent)
        })
    }

    /// The no-scroll-body mode: the child fills the space the earlier slivers
    /// leave, or its own extent when that is larger.
    fn layout_remainder(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let axis = sc.axis;
        let cross = sc.cross_axis_extent;
        let remainder = finite_extent(sc.viewport_main_axis_extent - sc.preceding_scroll_extent);
        let size = self.child.layout_child(
            ctx,
            &BoxConstraints::new(axis.size(remainder, cross), axis.size(f64::INFINITY, cross)),
        );
        box_geometry(finite_extent(axis.main_extent(size)).max(remainder), sc)
    }
}

impl Widget for SliverFillRemainingWidget {
    /// Outside a viewport there is nothing to fill: the child is laid out
    /// under `bc` at the origin, uncut.
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        self.clip = false;
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.clip {
            scene.push_clip(ctx.origin(), ctx.size());
        }
        self.child.paint_child(ctx, scene);
        if self.clip {
            scene.pop_clip();
        }
    }

    /// The viewport only delivers a positional event inside this sliver's
    /// paint extent (or to a captured child), so the ordinary single-child
    /// routing applies.
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }

    crate::authoring::visit_children!(child);
}

impl SliverWidget for SliverFillRemainingWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let filled = if self.has_scroll_body {
            self.layout_scroll_body(ctx, sc)
        } else {
            Some(self.layout_remainder(ctx, sc))
        };
        let geometry = filled.unwrap_or_else(|| {
            // Nothing finite to fill: the child keeps its own extent.
            let size = self.child.layout_child(ctx, &sc.as_box_constraints());
            box_geometry(sc.axis.main_extent(size), sc)
        });
        self.child.set_origin(box_child_origin(sc));
        self.clip = geometry.has_visual_overflow;
        geometry
    }
}
