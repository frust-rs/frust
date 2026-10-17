//! The sliver layout protocol for a scrolling viewport's children, mirroring
//! Flutter's `RenderSliver` (`rendering/sliver.dart`).
//!
//! A sliver answers [`SliverConstraints`] with a [`SliverGeometry`], so it can lay
//! out only its visible (or cached) content. A widget opts in through
//! [`Widget::as_sliver`]; a viewport lays it out with
//! [`ChildPod::layout_sliver_child`](crate::widget::ChildPod::layout_sliver_child),
//! which applies the per-pod layout skip, and wraps a `None` child in a box
//! adapter. Only forward growth is modelled.

use kurbo::Size;

use crate::layout::BoxConstraints;
use crate::widget::{LayoutCtx, Widget};

/// How far [`SliverGeometry::debug_assert_valid`] lets an extent overshoot its
/// bound: Flutter's `precisionErrorTolerance`, for float noise.
pub const PRECISION_ERROR_TOLERANCE: f64 = 1e-10;

/// The axis a viewport scrolls along (Flutter's `Axis`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollAxis {
    /// The main axis runs top to bottom; the cross axis is the width.
    #[default]
    Vertical,
    /// The main axis runs left to right; the cross axis is the height.
    Horizontal,
}

impl ScrollAxis {
    /// The [`Size`] with `main` along this axis and `cross` across it.
    pub fn size(self, main: f64, cross: f64) -> Size {
        match self {
            ScrollAxis::Vertical => Size::new(cross, main),
            ScrollAxis::Horizontal => Size::new(main, cross),
        }
    }

    /// `size`'s extent along this axis.
    pub fn main_extent(self, size: Size) -> f64 {
        match self {
            ScrollAxis::Vertical => size.height,
            ScrollAxis::Horizontal => size.width,
        }
    }

    /// `size`'s extent across this axis.
    pub fn cross_extent(self, size: Size) -> f64 {
        match self {
            ScrollAxis::Vertical => size.width,
            ScrollAxis::Horizontal => size.height,
        }
    }
}

/// Which way content grows from the viewport's leading edge (Flutter's
/// `GrowthDirection`); non-exhaustive so Flutter's `reverse` can be added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GrowthDirection {
    /// Down, or right.
    #[default]
    Forward,
}

/// The user's current scroll direction (Flutter's `ScrollDirection`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ScrollDirection {
    /// Not scrolling, or the direction is unknown.
    #[default]
    Idle,
    /// Scrolling toward the content's start (the scroll offset is decreasing).
    Forward,
    /// Scrolling toward the content's end (the scroll offset is increasing).
    Reverse,
}

/// What a viewport hands each sliver child (Flutter's `SliverConstraints`), in
/// logical px along [`axis`](Self::axis) unless named `cross_axis_*`. The layout
/// skip compares with `PartialEq`, so a NaN field always lays out again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliverConstraints {
    pub axis: ScrollAxis,
    /// How far the viewport's leading edge has scrolled into this sliver; `0`
    /// while its start is on screen (Flutter's `scrollOffset`).
    pub scroll_offset: f64,
    /// Flutter's `precedingScrollExtent`; may be infinite.
    pub preceding_scroll_extent: f64,
    /// Pixels an earlier sliver (a pinned header) already covers (Flutter's `overlap`).
    pub overlap: f64,
    /// Paint space earlier slivers left (Flutter's `remainingPaintExtent`); may
    /// be infinite for a shrink-wrapping viewport.
    pub remaining_paint_extent: f64,
    pub cross_axis_extent: f64,
    pub viewport_main_axis_extent: f64,
    /// Cache-area start relative to `scroll_offset`; never positive.
    pub cache_origin: f64,
    /// Content to provide from `cache_origin`, visible plus cache area.
    pub remaining_cache_extent: f64,
    pub user_scroll_direction: ScrollDirection,
}

impl SliverConstraints {
    /// Always [`GrowthDirection::Forward`].
    pub fn growth_direction(&self) -> GrowthDirection {
        GrowthDirection::Forward
    }

    /// Tight to `cross_axis_extent` across the axis, unbounded along it
    /// (Flutter's `asBoxConstraints()`).
    pub fn as_box_constraints(&self) -> BoxConstraints {
        let cross = self.cross_axis_extent;
        BoxConstraints::new(
            self.axis.size(0.0, cross),
            self.axis.size(f64::INFINITY, cross),
        )
    }

    /// Flutter's `SliverConstraints.debugAssertIsValid` (with `isNormalized`);
    /// nothing without `debug_assertions`.
    pub fn debug_assert_valid(&self) {
        debug_assert!(
            self.scroll_offset >= 0.0,
            "SliverConstraints: scroll_offset {} is negative or NaN",
            self.scroll_offset
        );
        debug_assert!(
            self.cross_axis_extent >= 0.0,
            "SliverConstraints: cross_axis_extent {} is negative or NaN",
            self.cross_axis_extent
        );
        debug_assert!(
            self.viewport_main_axis_extent >= 0.0,
            "SliverConstraints: viewport_main_axis_extent {} is negative or NaN",
            self.viewport_main_axis_extent
        );
        debug_assert!(
            self.remaining_paint_extent >= 0.0,
            "SliverConstraints: remaining_paint_extent {} is negative or NaN",
            self.remaining_paint_extent
        );
        debug_assert!(
            self.remaining_cache_extent >= 0.0,
            "SliverConstraints: remaining_cache_extent {} is negative or NaN",
            self.remaining_cache_extent
        );
        debug_assert!(
            self.cache_origin <= 0.0,
            "SliverConstraints: cache_origin {} is positive or NaN",
            self.cache_origin
        );
        debug_assert!(
            !self.preceding_scroll_extent.is_nan(),
            "SliverConstraints: preceding_scroll_extent is NaN"
        );
        debug_assert!(!self.overlap.is_nan(), "SliverConstraints: overlap is NaN");
    }
}

/// What a sliver reports from layout (Flutter's `SliverGeometry`), in logical
/// px along the constraints' axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliverGeometry {
    /// The (estimated) total scrollable extent.
    pub scroll_extent: f64,
    /// The visible size along the axis.
    pub paint_extent: f64,
    /// First visible pixel relative to the layout position; negative paints
    /// over an earlier sliver.
    pub paint_origin: f64,
    /// Distance to the next sliver's first pixel; usually `paint_extent`.
    pub layout_extent: f64,
    /// The (estimated) paint extent given unlimited paint space.
    pub max_paint_extent: f64,
    /// Extent of the hit region from the paint start; usually `paint_extent`.
    pub hit_test_extent: f64,
    pub visible: bool,
    /// The sliver painted outside its paint extent; the viewport must clip.
    pub has_visual_overflow: bool,
    /// A non-zero scroll-offset adjustment after which the viewport re-runs
    /// layout, when an earlier extent estimate was wrong.
    pub scroll_offset_correction: Option<f64>,
    /// Cache extent consumed; at least `layout_extent`.
    pub cache_extent: f64,
}

impl SliverGeometry {
    /// An empty, invisible sliver.
    pub const ZERO: SliverGeometry = SliverGeometry {
        scroll_extent: 0.0,
        paint_extent: 0.0,
        paint_origin: 0.0,
        layout_extent: 0.0,
        max_paint_extent: 0.0,
        hit_test_extent: 0.0,
        visible: false,
        has_visual_overflow: false,
        scroll_offset_correction: None,
        cache_extent: 0.0,
    };

    /// Flutter's constructor defaults: layout, hit-test and cache extents equal
    /// `paint_extent`, `visible` is `paint_extent > 0`, the rest zero.
    pub fn new(scroll_extent: f64, paint_extent: f64, max_paint_extent: f64) -> Self {
        SliverGeometry {
            scroll_extent,
            paint_extent,
            layout_extent: paint_extent,
            max_paint_extent,
            hit_test_extent: paint_extent,
            visible: paint_extent > 0.0,
            cache_extent: paint_extent,
            ..SliverGeometry::ZERO
        }
    }

    /// Flutter's `SliverGeometry.debugAssertIsValid` plus the paint-extent check
    /// against `constraints`; nothing without `debug_assertions`.
    pub fn debug_assert_valid(&self, constraints: &SliverConstraints) {
        debug_assert!(
            self.scroll_extent >= 0.0,
            "SliverGeometry: scroll_extent {} is negative or NaN",
            self.scroll_extent
        );
        debug_assert!(
            self.paint_extent >= 0.0,
            "SliverGeometry: paint_extent {} is negative or NaN",
            self.paint_extent
        );
        debug_assert!(
            self.layout_extent >= 0.0,
            "SliverGeometry: layout_extent {} is negative or NaN",
            self.layout_extent
        );
        debug_assert!(
            self.cache_extent >= 0.0,
            "SliverGeometry: cache_extent {} is negative or NaN",
            self.cache_extent
        );
        debug_assert!(
            self.hit_test_extent >= 0.0,
            "SliverGeometry: hit_test_extent {} is negative or NaN",
            self.hit_test_extent
        );
        debug_assert!(
            self.layout_extent - self.paint_extent <= PRECISION_ERROR_TOLERANCE,
            "SliverGeometry: layout_extent {} exceeds paint_extent {}",
            self.layout_extent,
            self.paint_extent
        );
        debug_assert!(
            self.paint_extent - self.max_paint_extent <= PRECISION_ERROR_TOLERANCE,
            "SliverGeometry: max_paint_extent {} is less than paint_extent {}",
            self.max_paint_extent,
            self.paint_extent
        );
        debug_assert!(
            self.paint_extent - constraints.remaining_paint_extent <= PRECISION_ERROR_TOLERANCE,
            "SliverGeometry: paint_extent {} exceeds the constraints' remaining_paint_extent {}",
            self.paint_extent,
            constraints.remaining_paint_extent
        );
        debug_assert!(
            self.layout_extent - self.cache_extent <= PRECISION_ERROR_TOLERANCE,
            "SliverGeometry: cache_extent {} is less than layout_extent {}",
            self.cache_extent,
            self.layout_extent
        );
        if let Some(correction) = self.scroll_offset_correction {
            debug_assert!(
                correction != 0.0 && correction.is_finite(),
                "SliverGeometry: scroll_offset_correction {correction} is zero or not finite \
                 (use None for no correction)"
            );
        }
    }
}

/// A widget that lays out as a sliver (Flutter's `RenderSliver` layout half).
///
/// Opt in by returning `Some(self)` from [`Widget::as_sliver`]; it paints in a
/// `(cross_axis_extent, paint_extent)` local space. [`Widget::layout`]'s
/// layout-skip contract applies to `layout_sliver` unchanged.
pub trait SliverWidget: Widget {
    /// Lay the sliver out under `sc` and report its geometry, which must satisfy
    /// [`SliverGeometry::debug_assert_valid`] against `sc`.
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constraints(axis: ScrollAxis) -> SliverConstraints {
        SliverConstraints {
            axis,
            scroll_offset: 0.0,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 600.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 600.0,
            cache_origin: 0.0,
            remaining_cache_extent: 850.0,
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    #[test]
    fn axis_packs_main_and_cross() {
        assert_eq!(ScrollAxis::Vertical.size(10.0, 20.0), Size::new(20.0, 10.0));
        assert_eq!(
            ScrollAxis::Horizontal.size(10.0, 20.0),
            Size::new(10.0, 20.0)
        );
        let size = Size::new(3.0, 7.0);
        assert_eq!(ScrollAxis::Vertical.main_extent(size), 7.0);
        assert_eq!(ScrollAxis::Vertical.cross_extent(size), 3.0);
        assert_eq!(ScrollAxis::Horizontal.main_extent(size), 3.0);
        assert_eq!(ScrollAxis::Horizontal.cross_extent(size), 7.0);
    }

    #[test]
    fn box_constraints_are_tight_across_and_unbounded_along() {
        let bc = constraints(ScrollAxis::Vertical).as_box_constraints();
        assert_eq!(bc.min(), Size::new(400.0, 0.0));
        assert_eq!(bc.max(), Size::new(400.0, f64::INFINITY));
        let bc = constraints(ScrollAxis::Horizontal).as_box_constraints();
        assert_eq!(bc.min(), Size::new(0.0, 400.0));
        assert_eq!(bc.max(), Size::new(f64::INFINITY, 400.0));
    }

    #[test]
    fn growth_is_forward() {
        assert_eq!(
            constraints(ScrollAxis::Vertical).growth_direction(),
            GrowthDirection::Forward
        );
    }

    #[test]
    fn new_applies_the_flutter_defaults() {
        let g = SliverGeometry::new(1000.0, 600.0, 1000.0);
        assert_eq!(g.layout_extent, 600.0);
        assert_eq!(g.hit_test_extent, 600.0);
        assert_eq!(g.cache_extent, 600.0);
        assert!(g.visible);
        assert!(!SliverGeometry::new(1000.0, 0.0, 1000.0).visible);
        g.debug_assert_valid(&constraints(ScrollAxis::Vertical));
        SliverGeometry::ZERO.debug_assert_valid(&constraints(ScrollAxis::Vertical));
    }

    #[test]
    fn valid_constraints_pass() {
        constraints(ScrollAxis::Vertical).debug_assert_valid();
        SliverConstraints {
            remaining_paint_extent: f64::INFINITY,
            preceding_scroll_extent: f64::INFINITY,
            cache_origin: -250.0,
            ..constraints(ScrollAxis::Horizontal)
        }
        .debug_assert_valid();
    }

    #[test]
    fn equal_constraints_compare_equal_and_nan_never_does() {
        let sc = constraints(ScrollAxis::Vertical);
        assert_eq!(sc, sc);
        let moved = SliverConstraints {
            scroll_offset: 5.0,
            ..sc
        };
        assert_ne!(sc, moved);
        let nan = SliverConstraints {
            overlap: f64::NAN,
            ..sc
        };
        assert_ne!(nan, nan);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "scroll_offset")]
    fn negative_scroll_offset_is_rejected() {
        SliverConstraints {
            scroll_offset: -1.0,
            ..constraints(ScrollAxis::Vertical)
        }
        .debug_assert_valid();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "cache_origin")]
    fn positive_cache_origin_is_rejected() {
        SliverConstraints {
            cache_origin: 1.0,
            ..constraints(ScrollAxis::Vertical)
        }
        .debug_assert_valid();
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "remaining_paint_extent")]
    fn paint_past_the_remaining_extent_is_rejected() {
        SliverGeometry::new(1000.0, 601.0, 1000.0)
            .debug_assert_valid(&constraints(ScrollAxis::Vertical));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "exceeds paint_extent")]
    fn layout_past_paint_is_rejected() {
        SliverGeometry {
            layout_extent: 200.0,
            ..SliverGeometry::new(1000.0, 100.0, 1000.0)
        }
        .debug_assert_valid(&constraints(ScrollAxis::Vertical));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "max_paint_extent")]
    fn paint_past_max_paint_is_rejected() {
        SliverGeometry::new(1000.0, 300.0, 200.0)
            .debug_assert_valid(&constraints(ScrollAxis::Vertical));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "scroll_offset_correction")]
    fn zero_correction_is_rejected() {
        SliverGeometry {
            scroll_offset_correction: Some(0.0),
            ..SliverGeometry::ZERO
        }
        .debug_assert_valid(&constraints(ScrollAxis::Vertical));
    }

    #[test]
    fn tolerance_absorbs_float_noise() {
        SliverGeometry::new(1000.0, 600.0 + 1e-12, 1000.0)
            .debug_assert_valid(&constraints(ScrollAxis::Vertical));
    }
}
