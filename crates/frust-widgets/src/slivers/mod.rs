//! Slivers: the children a [`CustomScrollView`](crate::CustomScrollView) lays
//! out with the sliver protocol ([`frust_core::SliverConstraints`] in,
//! [`frust_core::SliverGeometry`] out) instead of box constraints.
//!
//! Each sliver lives in its own submodule and is re-exported flat from here
//! and from the crate root:
//!
//! * [`sliver_box`] — one box child (`adapter`);
//! * [`sliver_padding`] — insets around a sliver (`padding`).

pub mod adapter;
pub mod padding;

pub use adapter::{SliverBoxView, SliverBoxWidget, sliver_box};
pub use padding::{SliverPaddingView, SliverPaddingWidget, sliver_padding};

use frust_core::SliverConstraints;

/// How much of the content span `[from, to)` (in the sliver's own scroll
/// terms) lies inside the paint region `sc` describes — Flutter's
/// `RenderSliver.calculatePaintOffset`.
pub(crate) fn paint_offset(sc: &SliverConstraints, from: f64, to: f64) -> f64 {
    let a = sc.scroll_offset;
    let b = a + sc.remaining_paint_extent;
    let span = to.max(a).min(b) - from.max(a).min(b);
    span.max(0.0).min(sc.remaining_paint_extent)
}

/// How much of the content span `[from, to)` lies inside the cache region
/// `sc` describes — Flutter's `RenderSliver.calculateCacheOffset`, with the
/// region read as [`SliverConstraints::remaining_cache_extent`] documents it:
/// that much content starting `cache_origin` before the scroll offset.
pub(crate) fn cache_offset(sc: &SliverConstraints, from: f64, to: f64) -> f64 {
    let a = sc.scroll_offset + sc.cache_origin;
    let b = a + sc.remaining_cache_extent;
    let span = to.max(a).min(b) - from.max(a).min(b);
    span.max(0.0).min(sc.remaining_cache_extent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{ScrollAxis, ScrollDirection};

    fn constraints(scroll_offset: f64) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 500.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 500.0,
            cache_origin: -scroll_offset.min(250.0),
            remaining_cache_extent: 750.0 + scroll_offset.min(250.0),
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    #[test]
    fn paint_and_cache_offsets_measure_the_overlap_with_each_region() {
        let sc = constraints(100.0);
        assert_eq!(paint_offset(&sc, 0.0, 50.0), 0.0);
        assert_eq!(paint_offset(&sc, 0.0, 150.0), 50.0);
        assert_eq!(paint_offset(&sc, 550.0, 700.0), 50.0);
        assert_eq!(paint_offset(&sc, 0.0, f64::INFINITY), 500.0);
        assert_eq!(cache_offset(&sc, 0.0, 50.0), 50.0);
        assert_eq!(cache_offset(&sc, 800.0, 900.0), 50.0);
        assert_eq!(cache_offset(&sc, 900.0, 1000.0), 0.0);
    }
}
