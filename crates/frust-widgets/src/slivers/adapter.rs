//! `sliver_box`: one box child inside a scrolling viewport — Flutter's
//! `SliverToBoxAdapter` (`rendering/sliver.dart`'s `RenderSliverToBoxAdapter`).
//!
//! The adapter lays its child out as a box, tight to the viewport's cross axis
//! and unbounded along the scroll axis
//! ([`SliverConstraints::as_box_constraints`]), and reports the child's main
//! extent as its scroll extent. It paints only the part of the child inside the
//! viewport's remaining paint space: the child is offset by `-scroll_offset`
//! inside a paint extent of `clamp(extent − scroll_offset, 0,
//! remaining_paint_extent)`, and clipped when that cuts it
//! ([`SliverGeometry::has_visual_overflow`]).
//!
//! A [`CustomScrollView`](crate::CustomScrollView) wraps every plain box child
//! in exactly this geometry on its own, so `sliver_box` is only needed to make
//! the adaptation explicit — or to lay a widget that *does* speak the sliver
//! protocol out as a box instead. The two share [`box_geometry`] and
//! [`box_child_origin`], so the explicit and the implicit adapter cannot drift.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, ScrollAxis, SemanticsCtx, SliverConstraints, SliverGeometry,
    SliverWidget, View, Widget, any,
};
use kurbo::{Point, Size};

/// `value` as a usable extent: NaN becomes `0.0`, a negative value clamps to
/// `0.0`, and `+∞` is kept (an unbounded extent is meaningful to the sliver
/// protocol, e.g. a shrink-wrapping viewport's remaining paint extent).
pub(crate) fn non_negative(value: f64) -> f64 {
    if value.is_nan() { 0.0 } else { value.max(0.0) }
}

/// `value` as a finite, non-negative extent: anything NaN or infinite becomes
/// `0.0`. What a box child's measured main extent is read through, so an
/// unbounded child scrolls nothing rather than poisoning the viewport's sums.
pub(crate) fn finite_extent(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// The point `main` px along `axis` and `cross` px across it.
pub(crate) fn axis_point(axis: ScrollAxis, main: f64, cross: f64) -> Point {
    match axis {
        ScrollAxis::Vertical => Point::new(cross, main),
        ScrollAxis::Horizontal => Point::new(main, cross),
    }
}

/// How much of the content span `[from, to)` lies inside the paint region
/// `sc` describes — Flutter's `RenderSliver.calculatePaintOffset`.
fn paint_offset_of(sc: &SliverConstraints, from: f64, to: f64) -> f64 {
    let a = sc.scroll_offset;
    let b = a + sc.remaining_paint_extent;
    let span = to.max(a).min(b) - from.max(a).min(b);
    span.max(0.0).min(sc.remaining_paint_extent)
}

/// How much of the content span `[from, to)` lies inside the cache region
/// `sc` describes — Flutter's `RenderSliver.calculateCacheOffset`, with the
/// region read as [`SliverConstraints::remaining_cache_extent`] documents it:
/// that much content starting at `cache_origin`.
fn cache_offset_of(sc: &SliverConstraints, from: f64, to: f64) -> f64 {
    let a = sc.scroll_offset + sc.cache_origin;
    let b = a + sc.remaining_cache_extent;
    let span = to.max(a).min(b) - from.max(a).min(b);
    span.max(0.0).min(sc.remaining_cache_extent)
}

/// The geometry of a box child `extent` px long along the scroll axis, laid
/// out under `sc` — `RenderSliverToBoxAdapter.performLayout`'s.
///
/// `extent` is read through [`finite_extent`]. The cache extent is never
/// reported below the paint extent, so float noise in a caller's running
/// subtraction cannot trip [`SliverGeometry::debug_assert_valid`].
pub(crate) fn box_geometry(extent: f64, sc: &SliverConstraints) -> SliverGeometry {
    let extent = finite_extent(extent);
    let paint_extent = paint_offset_of(sc, 0.0, extent);
    let cache_extent = cache_offset_of(sc, 0.0, extent).max(paint_extent);
    SliverGeometry {
        cache_extent,
        has_visual_overflow: extent > sc.remaining_paint_extent || sc.scroll_offset > 0.0,
        ..SliverGeometry::new(extent, paint_extent, extent)
    }
}

/// Where a box child sits relative to its sliver's paint start: scrolled back
/// by the sliver's own `scroll_offset`, so the part already scrolled past sits
/// before the paint start (and is clipped away).
pub(crate) fn box_child_origin(sc: &SliverConstraints) -> Point {
    axis_point(sc.axis, -sc.scroll_offset, 0.0)
}

/// A box child adapted to the sliver protocol. See the [module docs](self).
pub struct SliverBoxView<State: 'static> {
    child: AnyView<State>,
}

/// Adapt `child` — any box view — to a sliver: it scrolls by its own main-axis
/// extent and paints the part of itself inside the viewport. See the
/// [module docs](self).
///
/// ```
/// use frust_widgets::{CustomScrollView, custom_scroll_view, sliver_box, text};
/// let view: CustomScrollView<()> = custom_scroll_view((sliver_box(text("header")), text("body")));
/// # let _ = view;
/// ```
pub fn sliver_box<State: 'static, V: View<State>>(child: V) -> SliverBoxView<State> {
    SliverBoxView { child: any(child) }
}

/// The retained widget for a [`SliverBoxView`]: the child's pod, plus whether
/// the last sliver layout cut the child (so paint clips it).
pub struct SliverBoxWidget {
    child: ChildPod,
    /// Whether the last layout left part of the child outside this sliver's
    /// paint extent — [`SliverGeometry::has_visual_overflow`]. Always `false`
    /// after a plain box layout, which never cuts the child.
    clip: bool,
}

impl<State: 'static> View<State> for SliverBoxView<State> {
    type Element = SliverBoxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SliverBoxWidget {
        SliverBoxWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            clip: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverBoxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut SliverBoxWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for SliverBoxWidget {
    /// Outside a viewport the adapter is transparent: the child is laid out
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
    /// paint extent (or to a captured child), and that extent lies inside the
    /// child's own bounds, so the ordinary single-child routing applies.
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

impl SliverWidget for SliverBoxWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let size = self.child.layout_child(ctx, &sc.as_box_constraints());
        let geometry = box_geometry(sc.axis.main_extent(size), sc);
        self.child.set_origin(box_child_origin(sc));
        self.clip = geometry.has_visual_overflow;
        geometry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::ScrollDirection;

    fn constraints(scroll_offset: f64, remaining_paint_extent: f64) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 500.0,
            cache_origin: -scroll_offset.min(250.0),
            remaining_cache_extent: remaining_paint_extent + 250.0 + scroll_offset.min(250.0),
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    #[test]
    fn a_fully_visible_child_paints_its_whole_extent_uncut() {
        let sc = constraints(0.0, 500.0);
        let g = box_geometry(300.0, &sc);
        assert_eq!(g.scroll_extent, 300.0);
        assert_eq!(g.paint_extent, 300.0);
        assert_eq!(g.layout_extent, 300.0);
        assert_eq!(g.max_paint_extent, 300.0);
        assert_eq!(g.cache_extent, 300.0);
        assert!(g.visible && !g.has_visual_overflow);
        g.debug_assert_valid(&sc);
    }

    #[test]
    fn a_scrolled_child_paints_what_is_left_and_is_cut() {
        let sc = constraints(250.0, 500.0);
        let g = box_geometry(300.0, &sc);
        assert_eq!(g.paint_extent, 50.0);
        assert_eq!(g.hit_test_extent, 50.0);
        assert!(g.has_visual_overflow);
        assert_eq!(box_child_origin(&sc), Point::new(0.0, -250.0));
        g.debug_assert_valid(&sc);
    }

    #[test]
    fn a_child_in_the_cache_region_is_cached_but_invisible() {
        let sc = SliverConstraints {
            remaining_paint_extent: 0.0,
            remaining_cache_extent: 250.0,
            cache_origin: 0.0,
            ..constraints(0.0, 0.0)
        };
        let g = box_geometry(100.0, &sc);
        assert_eq!(g.paint_extent, 0.0);
        assert!(!g.visible);
        assert_eq!(g.cache_extent, 100.0);
        g.debug_assert_valid(&sc);
    }

    #[test]
    fn an_unbounded_or_nan_extent_scrolls_nothing() {
        let sc = constraints(0.0, 500.0);
        assert_eq!(box_geometry(f64::INFINITY, &sc), SliverGeometry::ZERO);
        assert_eq!(box_geometry(f64::NAN, &sc), SliverGeometry::ZERO);
        assert_eq!(box_geometry(-5.0, &sc), SliverGeometry::ZERO);
    }

    #[test]
    fn extents_sanitise_without_panicking() {
        assert_eq!(non_negative(f64::NAN), 0.0);
        assert_eq!(non_negative(-1.0), 0.0);
        assert_eq!(non_negative(f64::INFINITY), f64::INFINITY);
        assert_eq!(finite_extent(f64::INFINITY), 0.0);
        assert_eq!(finite_extent(12.5), 12.5);
    }
}
