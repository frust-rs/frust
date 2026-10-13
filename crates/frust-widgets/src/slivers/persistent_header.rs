//! `sliver_persistent_header`: a header that shrinks from a maximum to a
//! minimum extent as it scrolls, and optionally stays pinned to the leading
//! edge or floats back in on a reverse scroll — Flutter's
//! `SliverPersistentHeader` (`rendering/sliver_persistent_header.dart`).
//!
//! The header is described by a [`SliverPersistentHeaderDelegate`]: a minimum
//! and a maximum extent, and a `build(shrink_offset, overlaps_content)` that
//! returns the header's content for how far it has shrunk. The header scrolls
//! `max_extent` px of content; its child is laid out tight at
//! `max(min_extent, max_extent − shrink_offset)` along the axis (tight, not
//! Flutter's loose `asBoxConstraints(maxExtent: ..)`, so the child's extent and
//! the geometry always agree) and at the viewport's cross extent across it.
//!
//! # The four behaviours
//!
//! * **Scrolling** (the default) — `RenderSliverScrollingPersistentHeader`:
//!   `shrink_offset = min(scroll_offset, max_extent)`, the paint extent is
//!   `max_extent − scroll_offset` and the child is pushed back by however much
//!   of it no longer fits, so it shrinks to `min_extent` and then scrolls away
//!   like a box.
//! * **Pinned** ([`SliverPersistentHeaderView::pinned`]) —
//!   `RenderSliverPinnedPersistentHeader`: the header never paints less than
//!   `min_extent` (space permitting) and stays at the leading edge. Its layout
//!   extent shrinks to `max_extent − scroll_offset` (down to zero) while its
//!   paint extent does not, so the viewport reports the difference to the next
//!   sliver as [`SliverConstraints::overlap`] and that sliver starts at the
//!   header's layout extent, under the header (an earlier sliver paints over a
//!   later one). The paint origin is the incoming `overlap`: zero for a first
//!   header, below an earlier pinned header otherwise, so pinned headers stack.
//!   `overlaps_content` is `overlap > 0`.
//! * **Floating** ([`SliverPersistentHeaderView::floating`]) —
//!   `RenderSliverFloatingPersistentHeader`: the header keeps an *effective*
//!   scroll offset of its own. Scrolling toward the content's end moves it with
//!   the content; scrolling back toward the start while the constraints'
//!   [`user_scroll_direction`](SliverConstraints::user_scroll_direction) is
//!   [`ScrollDirection::Forward`] reveals it at once, wherever the content is.
//!   `shrink_offset = min(effective, max_extent)` and `overlaps_content` is
//!   `effective < scroll_offset`. There is no snap: a half-revealed header stays
//!   half revealed when the scroll stops.
//! * **Floating and pinned** — `RenderSliverFloatingPinnedPersistentHeader`:
//!   floating, but never painting less than `min_extent`.
//!
//! The viewport reports a direction only while user input drives the motion (a
//! drag or its release fling); a wheel step, a controller jump or tween scrolls
//! with [`ScrollDirection::Idle`], so they never reveal a floating header — they
//! only move or hide it.
//!
//! # The shrink offset reaches the child at the next rebuild
//!
//! The child is a view, and views are built only in [`View::rebuild`], never
//! during layout. Layout therefore *records* the shrink offset and
//! `overlaps_content` it computed; paint requests a frame when they differ from
//! what the live child was built with; and the next rebuild calls the
//! delegate's `build` with the recorded values. Until then the child keeps its
//! previous content but is already laid out at the new extent — so the content
//! lags the geometry by one frame while the header resizes. A shrink-offset
//! change below [`SHRINK_REBUILD_TOLERANCE`] does not rebuild at all (the child
//! keeps the content built under the previous shrink offset), except a change
//! onto either end of the range, `0` or `max_extent`, so a header at rest is
//! always built at its exact end state.
//!
//! `build` must be a pure function of its two arguments and captured state:
//! rebuild reconstructs the previous child from the previous view's `build`
//! with the values the live child was built with (the reconciliation
//! [`ListView`](crate::ListView) uses for its rows), so it can run twice per
//! rebuild.
//!
//! Laid out as a box (outside a viewport) the header is unscrolled: its child is
//! laid out at `max_extent` along the vertical axis, clamped into the incoming
//! constraints.

use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, ScrollAxis, ScrollDirection, SemanticsCtx, SliverConstraints,
    SliverGeometry, SliverWidget, View, Widget, any,
};
use kurbo::{Point, Size};

use super::adapter::{axis_point, finite_extent};

/// How far, in logical px, the shrink offset may move away from the one the
/// live child was built with before the header asks for a rebuild (the
/// [module docs](self)' *The shrink offset reaches the child at the next
/// rebuild*).
pub const SHRINK_REBUILD_TOLERANCE: f64 = 0.5;

/// A view-held header builder (`shrink_offset, overlaps_content -> content`),
/// erased per build.
type HeaderBuilder<State> = Rc<dyn Fn(f64, bool) -> AnyView<State>>;

/// What a [`sliver_persistent_header`] shows: its extent range and how to
/// build its content for a shrink offset — Flutter's
/// `SliverPersistentHeaderDelegate`.
///
/// `build(shrink_offset, overlaps_content)` receives how far the header has
/// shrunk from `max_extent` (`0` fully expanded, up to `max_extent`) and
/// whether content is scrolling under it. An extent that is NaN, infinite or
/// negative is treated as `0.0`, and a `max_extent` below `min_extent` as
/// `min_extent`.
pub struct SliverPersistentHeaderDelegate<F> {
    /// The smallest extent the header shrinks to along the scroll axis.
    pub min_extent: f64,
    /// The extent of the fully expanded header along the scroll axis — also
    /// how much the header scrolls.
    pub max_extent: f64,
    /// Builds the header's content for a shrink offset; see the type docs.
    pub build: F,
}

impl<F> SliverPersistentHeaderDelegate<F> {
    /// A delegate from its three parts.
    pub fn new(min_extent: f64, max_extent: f64, build: F) -> Self {
        Self {
            min_extent,
            max_extent,
            build,
        }
    }
}

/// A sliver header with a min/max extent. See the [module docs](self).
pub struct SliverPersistentHeaderView<State: 'static> {
    min_extent: f64,
    max_extent: f64,
    builder: HeaderBuilder<State>,
    pinned: bool,
    floating: bool,
}

/// A header sliver that shrinks from `delegate.max_extent` to
/// `delegate.min_extent` as it scrolls, its content rebuilt by
/// `delegate.build` for the current shrink offset. It scrolls away like a box
/// unless made [`pinned`](SliverPersistentHeaderView::pinned) or
/// [`floating`](SliverPersistentHeaderView::floating). See the
/// [module docs](self).
///
/// ```
/// use frust_widgets::{
///     CustomScrollView, SliverPersistentHeaderDelegate, custom_scroll_view,
///     sliver_fixed_extent_list, sliver_persistent_header, text,
/// };
/// let header = sliver_persistent_header(SliverPersistentHeaderDelegate::new(
///     56.0,
///     200.0,
///     |shrink: f64, _overlaps: bool| text(format!("shrunk by {shrink:.0}")),
/// ))
/// .pinned(true);
/// let view: CustomScrollView<()> = custom_scroll_view((
///     header,
///     sliver_fixed_extent_list(100, 48.0, |i| text(format!("row {i}"))),
/// ));
/// # let _ = view;
/// ```
pub fn sliver_persistent_header<State, V, F>(
    delegate: SliverPersistentHeaderDelegate<F>,
) -> SliverPersistentHeaderView<State>
where
    State: 'static,
    V: View<State>,
    F: Fn(f64, bool) -> V + 'static,
{
    let min_extent = finite_extent(delegate.min_extent);
    let max_extent = finite_extent(delegate.max_extent).max(min_extent);
    let build = delegate.build;
    SliverPersistentHeaderView {
        min_extent,
        max_extent,
        builder: Rc::new(move |shrink, overlaps| any(build(shrink, overlaps))),
        pinned: false,
        floating: false,
    }
}

impl<State: 'static> SliverPersistentHeaderView<State> {
    /// Keep the header at the viewport's leading edge, never painting less
    /// than `min_extent` (`false` by default). See the [module docs](self)'
    /// *The four behaviours*.
    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Reveal the header as soon as the user scrolls back toward the content's
    /// start, wherever the content is (`false` by default; no snap). See the
    /// [module docs](self)' *The four behaviours*.
    pub fn floating(mut self, floating: bool) -> Self {
        self.floating = floating;
        self
    }

    fn child_view(&self, inputs: BuildInputs) -> AnyView<State> {
        (self.builder)(inputs.shrink_offset, inputs.overlaps_content)
    }
}

/// The two values a header's content is built from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BuildInputs {
    shrink_offset: f64,
    overlaps_content: bool,
}

impl BuildInputs {
    const EXPANDED: BuildInputs = BuildInputs {
        shrink_offset: 0.0,
        overlaps_content: false,
    };
}

/// The retained widget for a [`SliverPersistentHeaderView`]: the content's
/// pod, the build inputs it holds and the ones the last layout asked for, and
/// a floating header's effective scroll offset.
pub struct SliverPersistentHeaderWidget {
    child: ChildPod,
    min_extent: f64,
    max_extent: f64,
    pinned: bool,
    floating: bool,
    /// The axis of the last sliver layout (vertical before the first).
    axis: ScrollAxis,
    /// What the live child was built with.
    built: BuildInputs,
    /// What the last layout computed — what the next rebuild builds with when
    /// it differs enough from `built` ([`Self::needs_rebuild`]).
    wanted: BuildInputs,
    /// A floating header's own scroll offset (Flutter's
    /// `_effectiveScrollOffset`): how far it is scrolled out, which a reverse
    /// scroll shrinks independently of the content.
    effective_scroll_offset: f64,
    /// The constraints' scroll offset at a floating header's last layout
    /// (Flutter's `_lastActualScrollOffset`); `None` before the first.
    last_scroll_offset: Option<f64>,
}

impl SliverPersistentHeaderWidget {
    fn new<State: 'static>(view: &SliverPersistentHeaderView<State>, child: ChildPod) -> Self {
        Self {
            child,
            min_extent: view.min_extent,
            max_extent: view.max_extent,
            pinned: view.pinned,
            floating: view.floating,
            axis: ScrollAxis::Vertical,
            built: BuildInputs::EXPANDED,
            wanted: BuildInputs::EXPANDED,
            effective_scroll_offset: 0.0,
            last_scroll_offset: None,
        }
    }

    /// The shrink offset the last layout computed: how far the header has
    /// shrunk from its maximum extent.
    pub fn shrink_offset(&self) -> f64 {
        self.wanted.shrink_offset
    }

    /// Whether the last layout found content scrolling under the header — the
    /// `overlaps_content` the delegate's `build` receives.
    pub fn overlaps_content(&self) -> bool {
        self.wanted.overlaps_content
    }

    /// Whether the live child was built with inputs too far from the ones the
    /// last layout computed (the [module docs](self)' *The shrink offset
    /// reaches the child at the next rebuild*).
    fn needs_rebuild(&self) -> bool {
        let (built, wanted) = (self.built, self.wanted);
        if built.overlaps_content != wanted.overlaps_content {
            return true;
        }
        if built.shrink_offset == wanted.shrink_offset {
            return false;
        }
        (built.shrink_offset - wanted.shrink_offset).abs() >= SHRINK_REBUILD_TOLERANCE
            || wanted.shrink_offset <= 0.0
            || wanted.shrink_offset >= self.max_extent
    }

    /// Lay the child out for `shrink_offset` — tight at
    /// `max(min_extent, max_extent − shrink_offset)` along the axis — record
    /// the build inputs, and return the child's main-axis extent.
    fn layout_child_at(
        &mut self,
        ctx: &mut LayoutCtx,
        sc: &SliverConstraints,
        shrink_offset: f64,
        overlaps_content: bool,
    ) -> f64 {
        self.wanted = BuildInputs {
            shrink_offset,
            overlaps_content,
        };
        let extent = self.min_extent.max(self.max_extent - shrink_offset);
        let tight = BoxConstraints::tight(sc.axis.size(extent, sc.cross_axis_extent));
        sc.axis.main_extent(self.child.layout_child(ctx, &tight))
    }

    /// Advance a floating header's effective scroll offset to `sc` —
    /// `RenderSliverFloatingPersistentHeader.performLayout` without the snap.
    ///
    /// Once the header is scrolled wholly out (effective offset at or past
    /// `max_extent`) the offset is kept at the constraints' scroll offset, the
    /// value Flutter's own non-floating branch settles on: every value past
    /// `max_extent` lays the header out identically, and pinning one keeps a
    /// second layout under the same constraints a no-op, as the layout-skip
    /// contract requires.
    fn advance_floating(&mut self, sc: &SliverConstraints) {
        let max = self.max_extent;
        let scroll_offset = sc.scroll_offset;
        match self.last_scroll_offset {
            Some(last) if scroll_offset < last || self.effective_scroll_offset < max => {
                let mut delta = last - scroll_offset;
                if sc.user_scroll_direction == ScrollDirection::Forward {
                    // Revealing: a header scrolled far out comes back from its
                    // own edge, not from wherever the content is.
                    self.effective_scroll_offset = self.effective_scroll_offset.min(max);
                } else if delta > 0.0 {
                    // Not a user scroll toward the start: shrinking is allowed,
                    // expanding is not.
                    delta = 0.0;
                }
                self.effective_scroll_offset =
                    (self.effective_scroll_offset - delta).clamp(0.0, scroll_offset);
            }
            _ => self.effective_scroll_offset = scroll_offset,
        }
        if self.effective_scroll_offset >= max {
            self.effective_scroll_offset = scroll_offset;
        }
        self.last_scroll_offset = Some(scroll_offset);
    }

    /// `RenderSliverScrollingPersistentHeader`'s layout.
    fn layout_scrolling(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let max = self.max_extent;
        let child_extent = self.layout_child_at(ctx, sc, sc.scroll_offset.min(max), false);
        let paint = max - sc.scroll_offset;
        self.place_child((paint.max(0.0) - child_extent).min(0.0), sc.axis);
        SliverGeometry {
            paint_origin: sc.overlap.min(0.0),
            has_visual_overflow: true,
            ..SliverGeometry::new(max, paint.clamp(0.0, sc.remaining_paint_extent), max)
        }
    }

    /// `RenderSliverPinnedPersistentHeader`'s layout.
    fn layout_pinned(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let max = self.max_extent;
        let child_extent =
            self.layout_child_at(ctx, sc, sc.scroll_offset.min(max), sc.overlap > 0.0);
        self.place_child(0.0, sc.axis);
        // The paint space left below the overlap — more than the remaining
        // paint extent under a negative (overscroll) overlap, which the clamp
        // to it keeps within the protocol.
        let remaining = (sc.remaining_paint_extent - sc.overlap).max(0.0);
        let paint = child_extent.min(remaining).min(sc.remaining_paint_extent);
        let layout = (max - sc.scroll_offset).clamp(0.0, remaining).min(paint);
        SliverGeometry {
            paint_origin: sc.overlap,
            layout_extent: layout,
            has_visual_overflow: false,
            cache_extent: if layout > 0.0 {
                layout - sc.cache_origin
            } else {
                layout
            },
            ..SliverGeometry::new(max, paint, max)
        }
    }

    /// `RenderSliverFloatingPersistentHeader`'s layout, and with `pinned`
    /// `RenderSliverFloatingPinnedPersistentHeader`'s.
    fn layout_floating(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        self.advance_floating(sc);
        let max = self.max_extent;
        let effective = self.effective_scroll_offset;
        let child_extent =
            self.layout_child_at(ctx, sc, effective.min(max), effective < sc.scroll_offset);
        let paint_unclamped = max - effective;
        let layout_unclamped = max - sc.scroll_offset;
        let (paint, layout, child_position) = if self.pinned {
            let min_allowed = self.min_extent.min(sc.remaining_paint_extent);
            let paint = paint_unclamped.clamp(min_allowed, sc.remaining_paint_extent);
            (paint, layout_unclamped.clamp(0.0, paint), 0.0)
        } else {
            (
                paint_unclamped.clamp(0.0, sc.remaining_paint_extent),
                layout_unclamped.clamp(0.0, sc.remaining_paint_extent),
                (paint_unclamped.max(0.0) - child_extent).min(0.0),
            )
        };
        self.place_child(child_position, sc.axis);
        SliverGeometry {
            paint_origin: sc.overlap.min(0.0),
            layout_extent: layout,
            has_visual_overflow: true,
            cache_extent: layout,
            ..SliverGeometry::new(max, paint, max)
        }
    }

    /// Place the child `main` px along `axis` from the header's paint start.
    fn place_child(&mut self, main: f64, axis: ScrollAxis) {
        self.child.set_origin(axis_point(axis, main, 0.0));
    }
}

impl<State: 'static> View<State> for SliverPersistentHeaderView<State> {
    type Element = SliverPersistentHeaderWidget;

    /// No layout has run yet: the content is built fully expanded.
    fn build(&self, ctx: &mut BuildCtx<'_>) -> SliverPersistentHeaderWidget {
        let child = crate::authoring::build_child(&self.child_view(BuildInputs::EXPANDED), ctx);
        SliverPersistentHeaderWidget::new(self, child)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverPersistentHeaderWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.floating != self.floating {
            // A header that starts or stops floating starts from the content's
            // position.
            element.last_scroll_offset = None;
        }
        if element.min_extent != self.min_extent
            || element.max_extent != self.max_extent
            || element.pinned != self.pinned
            || element.floating != self.floating
        {
            element.min_extent = self.min_extent;
            element.max_extent = self.max_extent;
            element.pinned = self.pinned;
            element.floating = self.floating;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let built = element.built;
        let next = if element.needs_rebuild() {
            element.wanted
        } else {
            built
        };
        let prev_child = prev.child_view(built);
        let next_child = self.child_view(next);
        flags |= crate::authoring::rebuild_child(&prev_child, &next_child, &mut element.child, ctx);
        element.built = next;
        flags
    }

    fn teardown(&self, element: &mut SliverPersistentHeaderWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child_view(element.built), &mut element.child, ctx);
    }
}

impl Widget for SliverPersistentHeaderWidget {
    /// Outside a viewport the header is unscrolled: the child is laid out at
    /// `max_extent` along the axis of the last sliver layout (vertical before
    /// one), clamped into `bc`, and at `bc`'s largest finite cross extent.
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let axis = self.axis;
        let cross = finite_extent(axis.cross_extent(bc.max())).max(axis.cross_extent(bc.min()));
        let tight = bc.tighten(axis.size(self.max_extent, cross));
        let size = self.child.layout_child(ctx, &tight);
        self.child.set_origin(Point::ZERO);
        self.wanted = BuildInputs::EXPANDED;
        size
    }

    /// Paints the child, and requests the frame whose rebuild brings the
    /// content up to the shrink offset this frame's layout computed.
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
        if self.needs_rebuild() {
            ctx.request_frame();
        }
    }

    /// The viewport delivers a positional event only inside the header's
    /// hit-test extent (or to a captured child), so the ordinary single-child
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

impl SliverWidget for SliverPersistentHeaderWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        self.axis = sc.axis;
        if self.floating {
            self.layout_floating(ctx, sc)
        } else if self.pinned {
            self.layout_pinned(ctx, sc)
        } else {
            self.layout_scrolling(ctx, sc)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;

    fn constraints(scroll_offset: f64, overlap: f64) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap,
            remaining_paint_extent: 500.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 500.0,
            cache_origin: -scroll_offset.min(250.0),
            remaining_cache_extent: 750.0 + scroll_offset.min(250.0),
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    fn header(pinned: bool, floating: bool) -> SliverPersistentHeaderWidget {
        let view: SliverPersistentHeaderView<()> = sliver_persistent_header(
            SliverPersistentHeaderDelegate::new(60.0, 200.0, |_: f64, _: bool| leaf(10.0, 10.0)),
        )
        .pinned(pinned)
        .floating(floating);
        let mut counter = 0;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn layout(widget: &mut SliverPersistentHeaderWidget, sc: &SliverConstraints) -> SliverGeometry {
        let mut ctx = LayoutCtx::new();
        let geometry = widget.layout_sliver(&mut ctx, sc);
        geometry.debug_assert_valid(sc);
        geometry
    }

    #[test]
    fn extents_are_sanitised() {
        let view: SliverPersistentHeaderView<()> = sliver_persistent_header(
            SliverPersistentHeaderDelegate::new(f64::NAN, -5.0, |_: f64, _: bool| leaf(1.0, 1.0)),
        );
        assert_eq!((view.min_extent, view.max_extent), (0.0, 0.0));
        let view: SliverPersistentHeaderView<()> = sliver_persistent_header(
            SliverPersistentHeaderDelegate::new(80.0, 40.0, |_: f64, _: bool| leaf(1.0, 1.0)),
        );
        assert_eq!((view.min_extent, view.max_extent), (80.0, 80.0));
    }

    #[test]
    fn a_scrolling_header_shrinks_then_scrolls_away() {
        let mut w = header(false, false);
        let g = layout(&mut w, &constraints(100.0, 0.0));
        assert_eq!(
            (g.scroll_extent, g.paint_extent, g.layout_extent),
            (200.0, 100.0, 100.0)
        );
        assert_eq!(w.shrink_offset(), 100.0);
        assert_eq!(w.child.origin(), Point::ZERO);
        let g = layout(&mut w, &constraints(170.0, 0.0));
        assert_eq!(g.paint_extent, 30.0);
        assert_eq!(w.child.origin(), Point::new(0.0, -30.0));
        let g = layout(&mut w, &constraints(400.0, 0.0));
        assert!(!g.visible);
        assert_eq!(w.shrink_offset(), 200.0);
    }

    #[test]
    fn a_pinned_header_keeps_its_minimum_and_reports_the_rest_as_overlap() {
        let mut w = header(true, false);
        for scroll_offset in [0.0, 50.0, 140.0, 199.0, 200.0, 1000.0] {
            let g = layout(&mut w, &constraints(scroll_offset, 0.0));
            assert!(g.paint_extent >= 60.0, "at {scroll_offset}");
            assert_eq!(g.paint_origin, 0.0);
            assert_eq!(g.layout_extent, (200.0 - scroll_offset).max(0.0));
        }
        // Below an earlier pinned header it paints after that header's overlap.
        let g = layout(&mut w, &constraints(300.0, 50.0));
        assert_eq!(
            (g.paint_origin, g.paint_extent, g.layout_extent),
            (50.0, 60.0, 0.0)
        );
        assert!(w.overlaps_content());
    }

    #[test]
    fn a_floating_header_only_expands_under_a_forward_user_scroll() {
        let mut w = header(false, true);
        assert_eq!(layout(&mut w, &constraints(1000.0, 0.0)).paint_extent, 0.0);
        // Back 40 px without a user direction: still hidden.
        assert_eq!(layout(&mut w, &constraints(960.0, 0.0)).paint_extent, 0.0);
        // Back 40 px more under a forward user scroll: 40 px revealed.
        let forward = SliverConstraints {
            user_scroll_direction: ScrollDirection::Forward,
            ..constraints(920.0, 0.0)
        };
        let g = layout(&mut w, &forward);
        assert_eq!((g.paint_extent, g.layout_extent), (40.0, 0.0));
        assert_eq!(w.shrink_offset(), 160.0);
        assert!(w.overlaps_content());
        // The same constraints again change nothing.
        assert_eq!(layout(&mut w, &forward), g);
        // Forward toward the end hides it again with the content.
        let g = layout(&mut w, &constraints(940.0, 0.0));
        assert_eq!(g.paint_extent, 20.0);
    }

    #[test]
    fn a_floating_pinned_header_never_paints_below_its_minimum() {
        let mut w = header(true, true);
        let g = layout(&mut w, &constraints(1000.0, 0.0));
        assert_eq!((g.paint_extent, g.layout_extent), (60.0, 0.0));
        let g = layout(
            &mut w,
            &SliverConstraints {
                user_scroll_direction: ScrollDirection::Forward,
                ..constraints(900.0, 0.0)
            },
        );
        assert_eq!(g.paint_extent, 100.0);
    }

    #[test]
    fn a_sub_tolerance_shrink_change_keeps_the_previous_build() {
        let mut w = header(true, false);
        w.built = BuildInputs {
            shrink_offset: 100.0,
            overlaps_content: false,
        };
        layout(&mut w, &constraints(100.4, 0.0));
        assert!(!w.needs_rebuild());
        layout(&mut w, &constraints(100.5, 0.0));
        assert!(w.needs_rebuild());
        // Reaching either end always rebuilds.
        w.built.shrink_offset = 199.8;
        layout(&mut w, &constraints(400.0, 0.0));
        assert!(w.needs_rebuild());
        w.built.shrink_offset = 0.2;
        layout(&mut w, &constraints(0.0, 0.0));
        assert!(w.needs_rebuild());
    }
}
