//! The sliver layout protocol: scroll-aware constraints and geometry for the
//! children of a scrolling viewport, mirroring Flutter's `RenderSliver`
//! protocol (`rendering/sliver.dart`).
//!
//! A box child answers [`BoxConstraints`] with a [`kurbo::Size`]. A sliver
//! answers [`SliverConstraints`] — where the viewport has scrolled to, how much
//! paint space is left, how wide the cross axis is — with a [`SliverGeometry`]
//! saying how much it scrolls, how much it painted, and where the next sliver
//! starts. That lets a sliver lay out only the part of its content that is
//! visible (or inside the cache area), which a box child cannot.
//!
//! The protocol is opt-in and lives beside [`Widget::layout`], never inside it:
//! a widget offers it through [`Widget::as_sliver`] (defaulted to `None`), and a
//! viewport lays a sliver child out through
//! [`ChildPod::layout_sliver_child`](crate::widget::ChildPod::layout_sliver_child),
//! which applies the same per-pod layout skip [`ChildPod::layout_child`]
//! does. A child that answers `None` is a plain box; the viewport wraps it in an
//! adapter.
//!
//! Only forward growth ([`GrowthDirection::Forward`]) is modelled: slivers grow
//! from the viewport's leading edge in the scroll axis' natural direction.
//!
//! [`ChildPod::layout_child`]: crate::widget::ChildPod::layout_child

use kurbo::Size;

use crate::layout::BoxConstraints;
use crate::widget::{LayoutCtx, Widget};

/// The tolerance [`SliverGeometry::debug_assert_valid`] allows an extent to
/// overshoot its bound by — Flutter's `precisionErrorTolerance`, absorbing the
/// floating-point noise a viewport's running subtraction accumulates.
pub const PRECISION_ERROR_TOLERANCE: f64 = 1e-10;

/// The axis a viewport scrolls along — Flutter's `Axis`, as a sliver's
/// `SliverConstraints.axis` reads it.
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

/// Which way content grows relative to the viewport's leading edge — Flutter's
/// `GrowthDirection`.
///
/// Only [`GrowthDirection::Forward`] exists: slivers are laid out from the
/// leading edge outward in the axis' natural direction. The type is
/// non-exhaustive so a reverse-growing (centre-anchored) viewport can add
/// Flutter's `reverse` without breaking a match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GrowthDirection {
    /// Content grows away from the viewport's leading edge, in the scroll
    /// axis' natural direction (down, or right).
    #[default]
    Forward,
}

/// The direction the user is currently scrolling in, relative to the content
/// — Flutter's `ScrollDirection` (`rendering/viewport_offset.dart`).
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

/// What a viewport hands each sliver child: its position in the scroll and the
/// space left to fill — Flutter's `SliverConstraints`.
///
/// All extents are logical pixels along [`axis`](SliverConstraints::axis)
/// unless named `cross_axis_*`. `PartialEq` is what
/// [`ChildPod::layout_sliver_child`](crate::widget::ChildPod::layout_sliver_child)'s
/// layout skip compares, so constraints carrying a NaN never compare equal and
/// always lay out again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliverConstraints {
    /// The axis the viewport scrolls along (Flutter's `axis`, derived there
    /// from `axisDirection`).
    pub axis: ScrollAxis,
    /// How far into this sliver the viewport's leading edge has scrolled: `0`
    /// while the sliver's start is on screen or ahead of it (Flutter's
    /// `scrollOffset`).
    pub scroll_offset: f64,
    /// The scroll extent consumed by every sliver before this one (Flutter's
    /// `precedingScrollExtent`); may be infinite after an unbounded sliver.
    pub preceding_scroll_extent: f64,
    /// How many pixels from this sliver's paint start an earlier sliver has
    /// already painted over, e.g. a pinned header (Flutter's `overlap`).
    pub overlap: f64,
    /// How many pixels of content the sliver should consider providing — the
    /// viewport's paint space not yet taken by earlier slivers (Flutter's
    /// `remainingPaintExtent`); may be infinite for a shrink-wrapping viewport.
    pub remaining_paint_extent: f64,
    /// The viewport's extent across the scroll axis (Flutter's
    /// `crossAxisExtent`).
    pub cross_axis_extent: f64,
    /// The viewport's extent along the scroll axis (Flutter's
    /// `viewportMainAxisExtent`).
    pub viewport_main_axis_extent: f64,
    /// Where the cache area starts relative to `scroll_offset`, never positive
    /// (Flutter's `cacheOrigin`).
    pub cache_origin: f64,
    /// How much content the sliver should provide starting at `cache_origin`,
    /// visible region plus cache area (Flutter's `remainingCacheExtent`).
    pub remaining_cache_extent: f64,
    /// The direction the user is scrolling in (Flutter's `userScrollDirection`).
    pub user_scroll_direction: ScrollDirection,
}

impl SliverConstraints {
    /// Which way content grows — always [`GrowthDirection::Forward`] (Flutter's
    /// `growthDirection`).
    pub fn growth_direction(&self) -> GrowthDirection {
        GrowthDirection::Forward
    }

    /// Box constraints for a box child laid out inside this sliver: tight to
    /// `cross_axis_extent` across the axis, unbounded along it (Flutter's
    /// `asBoxConstraints()` with its default extents).
    pub fn as_box_constraints(&self) -> BoxConstraints {
        let cross = self.cross_axis_extent;
        BoxConstraints::new(
            self.axis.size(0.0, cross),
            self.axis.size(f64::INFINITY, cross),
        )
    }

    /// Debug-assert the constraints are well formed — Flutter's
    /// `SliverConstraints.debugAssertIsValid` (including `isNormalized`).
    ///
    /// Checks that `scroll_offset`, `cross_axis_extent`,
    /// `viewport_main_axis_extent`, `remaining_paint_extent` and
    /// `remaining_cache_extent` are non-negative (NaN fails), that
    /// `cache_origin` is not positive, and that `preceding_scroll_extent` and
    /// `overlap` are not NaN. Compiles to nothing without `debug_assertions`.
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

/// What a sliver reports back from layout: how far it scrolls, how much of
/// the viewport it painted, and where the next sliver starts — Flutter's
/// `SliverGeometry`.
///
/// All extents are logical pixels along the constraints' axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliverGeometry {
    /// The (estimated) total scrollable extent of the sliver's content
    /// (Flutter's `scrollExtent`).
    pub scroll_extent: f64,
    /// How much of the remaining paint extent the sliver painted — its visible
    /// size along the axis (Flutter's `paintExtent`).
    pub paint_extent: f64,
    /// Where the sliver's first visible pixel sits relative to its layout
    /// position, e.g. negative to paint over an earlier sliver (Flutter's
    /// `paintOrigin`).
    pub paint_origin: f64,
    /// The distance from this sliver's first visible pixel to the next sliver's
    /// — usually `paint_extent`; less for a sliver the next one should overlap
    /// (Flutter's `layoutExtent`).
    pub layout_extent: f64,
    /// The (estimated) paint extent the sliver would provide given infinite
    /// remaining paint extent (Flutter's `maxPaintExtent`).
    pub max_paint_extent: f64,
    /// The distance from the paint start to the end of the region that accepts
    /// hits — usually `paint_extent` (Flutter's `hitTestExtent`).
    pub hit_test_extent: f64,
    /// Whether the sliver should be painted at all — usually
    /// `paint_extent > 0` (Flutter's `visible`).
    pub visible: bool,
    /// Whether the sliver painted outside its paint extent, so the viewport
    /// must clip it (Flutter's `hasVisualOverflow`).
    pub has_visual_overflow: bool,
    /// A non-zero amount the viewport must adjust its scroll offset by before
    /// re-running the whole layout, when the sliver found its earlier extent
    /// estimate wrong (Flutter's `scrollOffsetCorrection`).
    pub scroll_offset_correction: Option<f64>,
    /// How much of the remaining cache extent the sliver consumed — at least
    /// `layout_extent` (Flutter's `cacheExtent`).
    pub cache_extent: f64,
}

impl SliverGeometry {
    /// The geometry of an empty sliver: no extent of any kind and not visible
    /// (Flutter's `SliverGeometry.zero`).
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

    /// A geometry with Flutter's constructor defaults for everything but the
    /// three extents: `layout_extent` and `hit_test_extent` equal
    /// `paint_extent`, `cache_extent` equals `layout_extent`, `visible` is
    /// `paint_extent > 0`, and the rest is zero.
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

    /// Debug-assert the geometry is well formed and fits `constraints` —
    /// Flutter's `SliverGeometry.debugAssertIsValid` plus the paint-extent
    /// check of `RenderSliver.debugAssertDoesMeetConstraints`.
    ///
    /// Checks that `scroll_extent`, `paint_extent`, `layout_extent`,
    /// `cache_extent` and `hit_test_extent` are non-negative (NaN fails); that
    /// `layout_extent <= paint_extent`, `paint_extent <= max_paint_extent`,
    /// `paint_extent <= constraints.remaining_paint_extent` and
    /// `layout_extent <= cache_extent`, each within
    /// [`PRECISION_ERROR_TOLERANCE`]; and that a `scroll_offset_correction` is
    /// finite and non-zero. Compiles to nothing without `debug_assertions`.
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

/// A widget that can be laid out as a sliver inside a scrolling viewport —
/// Flutter's `RenderSliver`'s layout half (`performLayout` against
/// `SliverConstraints`, producing `geometry`).
///
/// A sliver is still a [`Widget`]: it paints, handles events and contributes
/// semantics like any other, in a local space whose size is
/// `(cross_axis_extent, paint_extent)` in axis terms (what
/// [`ChildPod::layout_sliver_child`](crate::widget::ChildPod::layout_sliver_child)
/// records as the pod's size). A type implementing this trait opts in by
/// overriding [`Widget::as_sliver`] to return `Some(self)`; its box
/// [`Widget::layout`] still answers when it is laid out outside a viewport.
///
/// The layout-skip contract on [`Widget::layout`] applies unchanged: the
/// result of `layout_sliver` must be a function of `sc`, the context's global
/// inputs, and state that only changes through a channel that dirties the pod.
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
