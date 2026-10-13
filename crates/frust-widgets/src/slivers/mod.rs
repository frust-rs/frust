//! Slivers: the children a [`CustomScrollView`](crate::CustomScrollView) lays
//! out with the sliver protocol ([`frust_core::SliverConstraints`] in,
//! [`frust_core::SliverGeometry`] out) instead of box constraints.
//!
//! Each sliver lives in its own submodule and is re-exported flat from here
//! and from the crate root:
//!
//! * [`sliver_box`] — one box child (`adapter`);
//! * [`sliver_padding`] — insets around a sliver (`padding`);
//! * [`sliver_fill_remaining`] — one box child filling what is left of the
//!   viewport (`fill_remaining`);
//! * [`sliver_fixed_extent_list`] / [`sliver_fixed_extent_list_keyed`] — a
//!   lazy list of equal-extent rows materialised over the cache window
//!   (`fixed_extent_list`), reconciled through the windowed reconcilers it
//!   shares with [`ListView`](crate::ListView) (`reconcile`);
//! * [`sliver_list`] — a lazy, keyed list of rows that size themselves, with
//!   scroll anchoring through the viewport's correction loop (`list`);
//! * [`keep_alive`] — keeps a lazy list's row alive outside its window
//!   (`keep_alive`);
//! * [`sliver_grid`] / [`sliver_grid_keyed`] — a lazy grid of equal cells
//!   laid out by a [`SliverGridDelegate`] (`grid`).

pub mod adapter;
pub mod fill_remaining;
pub mod fixed_extent_list;
pub mod grid;
pub mod keep_alive;
pub mod list;
pub mod padding;
pub(crate) mod reconcile;

pub use adapter::{SliverBoxView, SliverBoxWidget, sliver_box};
pub use fill_remaining::{
    SliverFillRemainingView, SliverFillRemainingWidget, sliver_fill_remaining,
};
pub use fixed_extent_list::{
    SliverFixedExtentListView, SliverFixedExtentListWidget, sliver_fixed_extent_list,
    sliver_fixed_extent_list_keyed,
};
pub use grid::{
    SliverGridDelegate, SliverGridView, SliverGridWidget, sliver_grid, sliver_grid_keyed,
};
pub use keep_alive::{KeepAliveHandle, KeepAliveView, KeepAliveWidget, keep_alive};
pub use list::{SliverListView, SliverListWidget, sliver_list};
pub use padding::{SliverPaddingView, SliverPaddingWidget, sliver_padding};

use frust_core::{EventCtx, SliverConstraints};

use crate::authoring::ErasedCallback;

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

/// One edge-triggered near-edge callback of a lazy sliver and its arming
/// state: fired once per approach to within `threshold` of its edge, rearmed
/// past `2 × threshold` (or by the owning list on a content change), delivered
/// on the next event that reaches the sliver.
pub(crate) struct NearEdge {
    pub(crate) callback: Option<ErasedCallback>,
    pub(crate) threshold: f64,
    /// Whether the edge fires on the next approach.
    pub(crate) armed: bool,
    /// A fire observed at layout, delivered on the next event.
    pub(crate) pending: bool,
}

impl NearEdge {
    pub(crate) fn new() -> Self {
        Self {
            callback: None,
            threshold: 0.0,
            armed: true,
            pending: false,
        }
    }

    /// Observe the sliver's `distance` from this edge; returns whether the
    /// edge just fired (and records it as pending).
    pub(crate) fn observe(&mut self, distance: f64) -> bool {
        if self.callback.is_none() {
            return false;
        }
        if distance > 2.0 * self.threshold {
            self.armed = true;
        }
        if self.armed && distance <= self.threshold {
            self.armed = false;
            self.pending = true;
            return true;
        }
        false
    }

    /// Run a pending fire's callback.
    pub(crate) fn deliver(&mut self, ctx: &mut EventCtx) {
        if std::mem::take(&mut self.pending)
            && let Some(callback) = self.callback.as_mut()
        {
            callback(ctx);
        }
    }
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
