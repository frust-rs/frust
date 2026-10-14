//! `sliver_padding`: empty space around a sliver — Flutter's `SliverPadding`
//! (`rendering/sliver_padding.dart`'s `RenderSliverEdgeInsetsPadding`).
//!
//! The insets along the scroll axis (top and bottom on a vertical viewport,
//! left and right on a horizontal one) become scroll extent before and after
//! the child; the insets across it narrow the child's cross extent. The child
//! is laid out under the incoming constraints shifted past the leading inset:
//!
//! * `scroll_offset` less the leading inset (never negative), and
//!   `preceding_scroll_extent` plus it;
//! * `remaining_paint_extent` less the part of the leading inset that is
//!   painted, and a positive `overlap` less the same;
//! * `cross_axis_extent` less both cross insets.
//!
//! Its geometry comes back with the insets added: the scroll and max-paint
//! extents grow by both main-axis insets, the paint, layout, hit-test and
//! cache extents by the painted (or cached) part of each, all clamped to
//! what the incoming constraints leave. The child is placed after the painted
//! part of the leading inset and the leading cross inset. A correction the
//! child asks for is passed straight up.
//!
//! **The cache window is re-derived, not shifted.** Flutter hands the child
//! `cacheOrigin + leadingInset`, which slides the child's cache window by the
//! inset once the leading inset has scrolled away. Here the child's window is
//! the incoming window `[scroll_offset + cache_origin, … +
//! remaining_cache_extent)` intersected with the child's own span and
//! expressed from its scroll offset, so it lines up with the viewport's
//! (`viewport.rs`'s *The layout loop*) at every position; the remaining cache
//! extent is the same as Flutter's.
//!
//! A child that is not a sliver is laid out as a box through the geometry
//! [`sliver_box`](crate::sliver_box) gives it, and clipped to its paint extent
//! when that cuts it. Laid out as a box itself (outside a viewport),
//! `sliver_padding` behaves like [`Padding`](crate::Padding).

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, ScrollAxis, SemanticsCtx, SliverConstraints, SliverGeometry,
    SliverWidget, View, Widget, any,
};
use kurbo::{Point, Size};

use super::adapter::{axis_point, box_child_origin, box_geometry, finite_extent};
use super::{cache_offset, paint_offset};
use crate::EdgeInsets;

/// Insets around a sliver. See the [module docs](self).
pub struct SliverPaddingView<State: 'static> {
    insets: EdgeInsets,
    child: AnyView<State>,
}

/// Surround `sliver` with `insets`: the main-axis insets scroll before and
/// after it, the cross-axis insets narrow it. A NaN, infinite or negative
/// inset is treated as `0.0`. See the [module docs](self).
///
/// ```
/// use frust_widgets::{
///     CustomScrollView, EdgeInsets, custom_scroll_view, sliver_box, sliver_padding, text,
/// };
/// let view: CustomScrollView<()> =
///     custom_scroll_view(sliver_padding(EdgeInsets::all(16.0), sliver_box(text("body"))));
/// # let _ = view;
/// ```
pub fn sliver_padding<State: 'static, V: View<State>>(
    insets: EdgeInsets,
    sliver: V,
) -> SliverPaddingView<State> {
    SliverPaddingView {
        insets,
        child: any(sliver),
    }
}

/// The retained widget for a [`SliverPaddingView`].
pub struct SliverPaddingWidget {
    child: ChildPod,
    insets: EdgeInsets,
    /// Whether the last layout left the child visible — what paint reads to
    /// skip it. Always `true` after a box layout.
    child_visible: bool,
    /// The local rect (origin, size) a box child is clipped to when the last
    /// sliver layout cut it; `None` otherwise.
    clip: Option<(Point, Size)>,
}

/// `insets` with every edge read through [`finite_extent`].
fn sanitized(insets: EdgeInsets) -> EdgeInsets {
    EdgeInsets {
        left: finite_extent(insets.left),
        top: finite_extent(insets.top),
        right: finite_extent(insets.right),
        bottom: finite_extent(insets.bottom),
    }
}

/// The insets along `axis`: `(leading main, trailing main, leading cross,
/// both cross)`.
fn axis_insets(insets: &EdgeInsets, axis: ScrollAxis) -> (f64, f64, f64, f64) {
    match axis {
        ScrollAxis::Vertical => (
            insets.top,
            insets.bottom,
            insets.left,
            insets.left + insets.right,
        ),
        ScrollAxis::Horizontal => (
            insets.left,
            insets.right,
            insets.top,
            insets.top + insets.bottom,
        ),
    }
}

/// The constraints the child of a padding with `before` px of leading main
/// inset and `cross_padding` px of cross insets is laid out under, given the
/// padding's own `sc`. See the [module docs](self).
fn child_constraints(sc: &SliverConstraints, before: f64, cross_padding: f64) -> SliverConstraints {
    let before_paint = paint_offset(sc, 0.0, before);
    let overlap = if sc.overlap > 0.0 {
        (sc.overlap - before_paint).max(0.0)
    } else {
        sc.overlap
    };
    let scroll_offset = (sc.scroll_offset - before).max(0.0);
    // The incoming cache window, in the child's own scroll terms and cut to
    // its start.
    let window_start = sc.scroll_offset + sc.cache_origin;
    let window_end = window_start + sc.remaining_cache_extent;
    let child_window_start = (window_start - before).max(0.0);
    SliverConstraints {
        scroll_offset,
        preceding_scroll_extent: sc.preceding_scroll_extent + before,
        overlap,
        remaining_paint_extent: (sc.remaining_paint_extent - before_paint).max(0.0),
        cross_axis_extent: (sc.cross_axis_extent - cross_padding).max(0.0),
        cache_origin: (child_window_start - scroll_offset).min(0.0),
        remaining_cache_extent: (window_end - before - child_window_start).max(0.0),
        ..*sc
    }
}

/// The padding's geometry around a child that reported `child` under the
/// constraints [`child_constraints`] derived from `sc`.
fn padded_geometry(
    sc: &SliverConstraints,
    child: &SliverGeometry,
    before: f64,
    after: f64,
) -> SliverGeometry {
    let main_padding = before + after;
    let child_end = before + child.scroll_extent;
    let before_paint = paint_offset(sc, 0.0, before);
    let after_paint = paint_offset(sc, child_end, child_end + after);
    let padding_paint = before_paint + after_paint;
    let padding_cache =
        cache_offset(sc, 0.0, before) + cache_offset(sc, child_end, child_end + after);
    let paint_extent = (before_paint + child.paint_extent.max(child.layout_extent + after_paint))
        .min(sc.remaining_paint_extent);
    let layout_extent = (padding_paint + child.layout_extent).min(paint_extent);
    SliverGeometry {
        scroll_extent: main_padding + child.scroll_extent,
        paint_extent,
        paint_origin: child.paint_origin,
        layout_extent,
        max_paint_extent: main_padding + child.max_paint_extent,
        hit_test_extent: (padding_paint + child.paint_extent)
            .max(before_paint + child.hit_test_extent),
        visible: paint_extent > 0.0,
        has_visual_overflow: child.has_visual_overflow,
        scroll_offset_correction: None,
        cache_extent: (padding_cache + child.cache_extent)
            .min(sc.remaining_cache_extent)
            .max(layout_extent),
    }
}

impl<State: 'static> View<State> for SliverPaddingView<State> {
    type Element = SliverPaddingWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SliverPaddingWidget {
        SliverPaddingWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            insets: sanitized(self.insets),
            child_visible: false,
            clip: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverPaddingWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        let insets = sanitized(self.insets);
        if element.insets != insets {
            element.insets = insets;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SliverPaddingWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for SliverPaddingWidget {
    /// Outside a viewport the padding is a plain [`Padding`](crate::Padding):
    /// the child is laid out under `bc` deflated by the insets, at the
    /// leading insets.
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let EdgeInsets {
            left,
            top,
            right,
            bottom,
        } = self.insets;
        let (h, v) = (left + right, top + bottom);
        let child_bc = BoxConstraints::new(
            Size::new(
                (bc.min().width - h).max(0.0),
                (bc.min().height - v).max(0.0),
            ),
            Size::new(
                (bc.max().width - h).max(0.0),
                (bc.max().height - v).max(0.0),
            ),
        );
        let size = self.child.layout_child(ctx, &child_bc);
        self.child.set_origin(Point::new(left, top));
        self.child_visible = true;
        self.clip = None;
        bc.constrain(Size::new(size.width + h, size.height + v))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !self.child_visible {
            return;
        }
        if let Some((origin, size)) = self.clip {
            scene.push_clip(ctx.origin() + origin.to_vec2(), size);
        }
        self.child.paint_child(ctx, scene);
        if self.clip.is_some() {
            scene.pop_clip();
        }
    }

    /// The viewport delivers a positional event only inside this sliver's
    /// hit-test extent, so the ordinary single-child routing applies.
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

impl SliverWidget for SliverPaddingWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let axis = sc.axis;
        let (before, after, cross_before, cross_padding) = axis_insets(&self.insets, axis);
        let child_sc = child_constraints(sc, before, cross_padding);
        let (child, is_box) = match self.child.layout_sliver_child(ctx, &child_sc) {
            Some(geometry) => (geometry, false),
            None => {
                let size = self.child.layout_child(ctx, &child_sc.as_box_constraints());
                (box_geometry(axis.main_extent(size), &child_sc), true)
            }
        };
        self.child_visible = child.visible;
        if let Some(correction) = child.scroll_offset_correction {
            self.clip = None;
            return SliverGeometry {
                scroll_offset_correction: Some(correction),
                ..SliverGeometry::ZERO
            };
        }
        let start = axis_point(axis, paint_offset(sc, 0.0, before), cross_before);
        if is_box {
            self.child
                .set_origin(start + box_child_origin(&child_sc).to_vec2());
            self.clip = child.has_visual_overflow.then(|| {
                (
                    start,
                    axis.size(child.paint_extent, child_sc.cross_axis_extent),
                )
            });
        } else {
            self.child.set_origin(start);
            self.clip = None;
        }
        padded_geometry(sc, &child, before, after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::ScrollDirection;

    /// A vertical sliver's constraints `scroll_offset` into it, with the
    /// viewport's 250 px cache window.
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
    fn the_child_starts_after_the_leading_inset() {
        let sc = constraints(0.0, 500.0);
        let child = child_constraints(&sc, 20.0, 30.0);
        assert_eq!(child.scroll_offset, 0.0);
        assert_eq!(child.remaining_paint_extent, 480.0);
        assert_eq!(child.cross_axis_extent, 370.0);
        assert_eq!(child.preceding_scroll_extent, 20.0);
        assert_eq!(child.cache_origin, 0.0);
        assert_eq!(child.remaining_cache_extent, 730.0);
        child.debug_assert_valid();
    }

    #[test]
    fn past_the_leading_inset_the_cache_window_matches_the_incoming_one() {
        let sc = constraints(300.0, 500.0);
        let child = child_constraints(&sc, 20.0, 0.0);
        assert_eq!(child.scroll_offset, 280.0);
        // The incoming window starts at 50; the child's at 30 of its own,
        // which is 50 of the padding's.
        assert_eq!(child.scroll_offset + child.cache_origin + 20.0, 50.0);
        assert_eq!(child.remaining_cache_extent, sc.remaining_cache_extent);
        child.debug_assert_valid();
    }

    #[test]
    fn geometry_adds_the_insets_and_clamps_to_the_paint_space() {
        let sc = constraints(0.0, 500.0);
        let child_sc = child_constraints(&sc, 20.0, 0.0);
        let child = box_geometry(100.0, &child_sc);
        let g = padded_geometry(&sc, &child, 20.0, 30.0);
        assert_eq!(g.scroll_extent, 150.0);
        assert_eq!(g.paint_extent, 150.0);
        assert_eq!(g.layout_extent, 150.0);
        assert_eq!(g.max_paint_extent, 150.0);
        assert_eq!(g.hit_test_extent, 150.0);
        g.debug_assert_valid(&sc);

        // Scrolled 50 in: the leading inset and 30 px of the child are gone.
        let sc = constraints(50.0, 500.0);
        let child_sc = child_constraints(&sc, 20.0, 0.0);
        let child = box_geometry(100.0, &child_sc);
        assert_eq!(child.paint_extent, 70.0);
        let g = padded_geometry(&sc, &child, 20.0, 30.0);
        assert_eq!(g.scroll_extent, 150.0);
        assert_eq!(g.paint_extent, 100.0);
        assert!(g.has_visual_overflow);
        g.debug_assert_valid(&sc);

        // Cut by the end of a short paint space.
        let sc = constraints(0.0, 60.0);
        let child_sc = child_constraints(&sc, 20.0, 0.0);
        let child = box_geometry(100.0, &child_sc);
        let g = padded_geometry(&sc, &child, 20.0, 30.0);
        assert_eq!(g.paint_extent, 60.0);
        assert_eq!(g.layout_extent, 60.0);
        g.debug_assert_valid(&sc);
    }

    #[test]
    fn horizontal_insets_swap_main_and_cross() {
        let insets = EdgeInsets {
            left: 1.0,
            top: 2.0,
            right: 3.0,
            bottom: 4.0,
        };
        assert_eq!(
            axis_insets(&insets, ScrollAxis::Vertical),
            (2.0, 4.0, 1.0, 4.0)
        );
        assert_eq!(
            axis_insets(&insets, ScrollAxis::Horizontal),
            (1.0, 3.0, 2.0, 6.0)
        );
    }

    #[test]
    fn bad_insets_sanitise_to_zero() {
        let insets = sanitized(EdgeInsets {
            left: f64::NAN,
            top: -3.0,
            right: f64::INFINITY,
            bottom: 2.0,
        });
        assert_eq!(
            insets,
            EdgeInsets {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 2.0,
            }
        );
    }
}
