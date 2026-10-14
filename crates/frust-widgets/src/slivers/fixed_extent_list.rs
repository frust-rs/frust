//! `sliver_fixed_extent_list`: a lazy list of rows that all share one main-axis
//! extent — Flutter's `SliverFixedExtentList`
//! (`rendering/sliver_fixed_extent_list.dart`'s
//! `RenderSliverFixedExtentBoxAdaptor`), materialised the way
//! [`ListView`](crate::ListView) materialises its rows.
//!
//! # The cache window is the window
//!
//! Only the rows covering the cache region the viewport hands this sliver —
//! `[scroll_offset + cache_origin, … + remaining_cache_extent)` in the
//! sliver's own scroll terms — are materialised, and nothing more: the
//! viewport's [`cache_extent`](crate::CustomScrollView::cache_extent) *is* the
//! buffer, so there is no separate buffer constant as `ListView` has. Row `i`
//! spans `[i × extent, (i + 1) × extent)`, so the window is the index range
//! `[floor(start / extent), ceil(end / extent))` clamped to the item count,
//! with Flutter's precision tolerance absorbing float noise at a row boundary
//! (`getMinChildIndexForScrollOffset` / `getMaxChildIndexForScrollOffset`). A
//! sliver outside the cache region materialises nothing.
//!
//! # Materialised at rebuild time
//!
//! Rows are built only in [`View::rebuild`], never at layout — `ListView`'s
//! windowing invariant (`list_view.rs`'s *Windowed materialization at rebuild
//! time*). Rebuild reads the constraints the last sliver layout ran with off
//! the retained widget, recomputes the window from them and reconciles the
//! live rows to it through the reconcilers it shares with `ListView`
//! (`slivers/reconcile.rs`): rows leaving the window are torn down at once,
//! rows entering are built fresh, survivors keep their pods. The very first
//! build has no constraints yet and materialises nothing; whenever the
//! materialised rows do not cover the window the last layout asked for (the
//! first frame, a scroll, a constraint change), paint requests one more frame
//! so the next rebuild re-windows — the same one-frame convergence `ListView`
//! accepts.
//!
//! [`sliver_fixed_extent_list`] reconciles rows by item index,
//! [`sliver_fixed_extent_list_keyed`] by a caller-supplied stable key, with
//! exactly `ListView::builder` / `ListView::builder_keyed`'s contracts
//! (`list_view.rs`'s *Row identity*): a positional list reattaches a row's
//! state to whatever later sits at its index, a keyed one carries it with the
//! row. A keyed sliver does not re-anchor the scroll position on a prepend
//! (`ListView`'s keyed anchoring); a prepend above the viewport moves the
//! content down by the prepended rows.
//!
//! # Layout and geometry
//!
//! Every materialised row is laid out tight: `extent` along the axis, the
//! cross-axis extent across it, placed at `i × extent − scroll_offset`. The
//! geometry is closed form and a function of the constraints alone (the
//! layout-skip contract), never of which rows happen to be materialised: the
//! scroll and max-paint extents are `item_count × extent`, the paint and cache
//! extents the parts of that span inside the paint and cache regions. Paint
//! clips to the paint extent when rows are cut by it, and skips rows wholly
//! outside it (a row only cached is laid out, never painted).
//!
//! # Near-edge callbacks
//!
//! [`SliverFixedExtentListView::on_near_start`] and
//! [`SliverFixedExtentListView::on_near_end`] are `ListView`'s
//! edge-triggered pair: fired once per approach to within the threshold of the
//! list's start (its scroll offset) or end (the content left past the
//! viewport's trailing edge), rearmed past twice the threshold or by an
//! item-count change. A sliver learns its position only at layout, so that is
//! where the edges are observed — whenever the list moved against the viewport
//! (its scroll offset or the paint space before it changed), never on its
//! first layout, so mounting fires nothing, as on `ListView` where only motion
//! does. A fire is delivered on the next event that reaches the list: the
//! layout requests the `Housekeeping` broadcast that guarantees one.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use frust_core::sliver::PRECISION_ERROR_TOLERANCE;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, ScrollAxis, ScrollDirection, SemanticsCtx, SliverConstraints,
    SliverGeometry, SliverWidget, View, Widget, any,
};
use kurbo::Size;

use super::adapter::{axis_point, finite_extent, non_negative};
use super::reconcile::{self, ItemViews, LiveRows};
use super::{cache_offset, paint_offset};
use crate::ChildKey;
use crate::authoring::ErasedCallback;

/// The debug-only tripwire message for two materialised rows claiming one
/// key — `ListView::builder_keyed`'s rule: no positional fallback, the first
/// slot claiming a key keeps the live row in release.
const DUPLICATE_KEY_MSG: &str = "sliver_fixed_extent_list_keyed produced a duplicate key inside \
     one window: row identity is ambiguous (release: the first slot claiming a key keeps the live \
     row, later duplicates build fresh)";

/// A view-held item builder, erased per row.
type ItemBuilder<State> = Rc<dyn Fn(usize) -> AnyView<State>>;

/// A view-held stable-key function (`item index -> row identity`).
type KeyOf = Rc<dyn Fn(usize) -> ChildKey>;

/// A view-held near-edge callback (erased on build).
type OnNearEdge<State> = Rc<dyn Fn(&mut State)>;

/// A lazy list of equal-extent rows. See the [module docs](self).
pub struct SliverFixedExtentListView<State: 'static> {
    item_count: usize,
    item_extent: f64,
    builder: ItemBuilder<State>,
    /// `None` reconciles rows by item index.
    key_of: Option<KeyOf>,
    on_near_start: Option<OnNearEdge<State>>,
    near_start_threshold: f64,
    on_near_end: Option<OnNearEdge<State>>,
    near_end_threshold: f64,
}

/// A lazy list of `item_count` rows, each `item_extent` logical px along the
/// scroll axis, whose row at `index` is `builder(index)` — reconciled by item
/// index, with [`ListView::builder`](crate::ListView::builder)'s contract.
/// See the [module docs](self).
///
/// # Panics
///
/// If `item_extent` is not positive and finite.
///
/// ```
/// use frust_widgets::{CustomScrollView, custom_scroll_view, sliver_fixed_extent_list, text};
/// let view: CustomScrollView<()> = custom_scroll_view(sliver_fixed_extent_list(
///     10_000,
///     48.0,
///     |i| text(format!("row {i}")),
/// ));
/// # let _ = view;
/// ```
pub fn sliver_fixed_extent_list<State: 'static, V: View<State>>(
    item_count: usize,
    item_extent: f64,
    builder: impl Fn(usize) -> V + 'static,
) -> SliverFixedExtentListView<State> {
    SliverFixedExtentListView::new(item_count, item_extent, None, builder)
}

/// [`sliver_fixed_extent_list`] reconciled by the stable key `key_of(index)`
/// instead of by item index, with
/// [`ListView::builder_keyed`](crate::ListView::builder_keyed)'s contract:
/// `key_of` runs once per materialised row per rebuild, and a duplicate key in
/// one window trips a `debug_assert!`. See the [module docs](self).
///
/// # Panics
///
/// If `item_extent` is not positive and finite.
///
/// ```
/// use frust_widgets::{
///     ChildKey, CustomScrollView, custom_scroll_view, sliver_fixed_extent_list_keyed, text,
/// };
/// let ids: Vec<u64> = (0..100).collect();
/// let keys = ids.clone();
/// let view: CustomScrollView<()> = custom_scroll_view(sliver_fixed_extent_list_keyed(
///     ids.len(),
///     48.0,
///     move |i| ChildKey::new(keys[i]),
///     move |i| text(format!("item {}", ids[i])),
/// ));
/// # let _ = view;
/// ```
pub fn sliver_fixed_extent_list_keyed<State: 'static, V: View<State>>(
    item_count: usize,
    item_extent: f64,
    key_of: impl Fn(usize) -> ChildKey + 'static,
    builder: impl Fn(usize) -> V + 'static,
) -> SliverFixedExtentListView<State> {
    SliverFixedExtentListView::new(item_count, item_extent, Some(Rc::new(key_of)), builder)
}

impl<State: 'static> SliverFixedExtentListView<State> {
    fn new<V: View<State>>(
        item_count: usize,
        item_extent: f64,
        key_of: Option<KeyOf>,
        builder: impl Fn(usize) -> V + 'static,
    ) -> Self {
        assert!(
            item_extent > 0.0 && item_extent.is_finite(),
            "sliver_fixed_extent_list item_extent must be positive and finite"
        );
        Self {
            item_count,
            item_extent,
            builder: Rc::new(move |index| any(builder(index))),
            key_of,
            on_near_start: None,
            near_start_threshold: 0.0,
            on_near_end: None,
            near_end_threshold: 0.0,
        }
    }

    /// Fire `callback` when the list's start comes within `threshold_px` of
    /// the viewport's leading edge — the "load older" edge, edge-triggered and
    /// rearmed as [`ListView::on_near_start`](crate::ListView::on_near_start)
    /// is. See the [module docs](self)' *Near-edge callbacks*.
    pub fn on_near_start<F: Fn(&mut State) + 'static>(
        mut self,
        callback: F,
        threshold_px: f64,
    ) -> Self {
        self.on_near_start = Some(Rc::new(callback));
        self.near_start_threshold = threshold_px;
        self
    }

    /// Fire `callback` when the list's end comes within `threshold_px` of the
    /// viewport's trailing edge — the "load newer" edge, mirroring
    /// [`SliverFixedExtentListView::on_near_start`] (and
    /// [`ListView::on_near_end`](crate::ListView::on_near_end)).
    pub fn on_near_end<F: Fn(&mut State) + 'static>(
        mut self,
        callback: F,
        threshold_px: f64,
    ) -> Self {
        self.on_near_end = Some(Rc::new(callback));
        self.near_end_threshold = threshold_px;
        self
    }
}

/// One edge-triggered near-edge callback and its arming state.
struct NearEdge {
    callback: Option<ErasedCallback>,
    threshold: f64,
    /// Whether the edge fires on the next approach. Cleared when it fires;
    /// set again past `2 × threshold` or by an item-count change.
    armed: bool,
    /// A fire observed at layout, delivered on the next event.
    pending: bool,
}

impl NearEdge {
    fn new() -> Self {
        Self {
            callback: None,
            threshold: 0.0,
            armed: true,
            pending: false,
        }
    }

    /// Observe the list's `distance` from this edge; returns whether the edge
    /// just fired (and records it as pending).
    fn observe(&mut self, distance: f64) -> bool {
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
    fn deliver(&mut self, ctx: &mut EventCtx) {
        if std::mem::take(&mut self.pending)
            && let Some(callback) = self.callback.as_mut()
        {
            callback(ctx);
        }
    }
}

/// The retained widget for a [`SliverFixedExtentListView`]: the materialised
/// rows, the constraints rebuild windows against, and the near-edge state.
pub struct SliverFixedExtentListWidget {
    /// The materialised rows, parallel to [`Self::indices`].
    children: Vec<ChildPod>,
    /// The item index each row renders — a contiguous ascending run.
    indices: Vec<usize>,
    /// Row identity for a keyed list: `key -> the item index that row
    /// occupied` as of the last rebuild. Empty for a positional list.
    key_index: HashMap<ChildKey, usize>,
    item_count: usize,
    item_extent: f64,
    /// The constraints the last sliver layout ran with — what rebuild windows
    /// against. `None` until the first layout.
    constraints: Option<SliverConstraints>,
    /// The paint extent of the last sliver layout.
    paint_extent: f64,
    /// Whether the last layout cut a row at the paint extent (so paint clips).
    clip: bool,
    /// Where the near edges were last observed — `(scroll_offset,
    /// remaining_paint_extent)`; `None` before the first layout.
    observed_at: Option<(f64, f64)>,
    near_start: NearEdge,
    near_end: NearEdge,
}

/// The first row index whose span reaches past `offset` — Flutter's
/// `getMinChildIndexForScrollOffset`, unclamped.
fn min_index(offset: f64, extent: f64) -> f64 {
    let actual = offset / extent;
    let round = actual.round();
    if (actual * extent - round * extent).abs() < PRECISION_ERROR_TOLERANCE {
        round
    } else {
        actual.floor()
    }
}

/// One past the last row index whose span starts before `offset` — Flutter's
/// `getMaxChildIndexForScrollOffset` plus one, unclamped.
fn end_index(offset: f64, extent: f64) -> f64 {
    let actual = offset / extent - 1.0;
    let round = actual.round();
    if (actual * extent - round * extent).abs() < PRECISION_ERROR_TOLERANCE {
        round.max(0.0) + 1.0
    } else {
        actual.ceil().max(0.0) + 1.0
    }
}

/// The row indices covering the cache region of `sc`, clamped to
/// `[0, item_count]` — the module docs' *The cache window is the window*.
fn cache_window(sc: &SliverConstraints, item_count: usize, item_extent: f64) -> Range<usize> {
    if item_count == 0 || sc.remaining_cache_extent <= 0.0 {
        return 0..0;
    }
    let count = item_count as f64;
    let start = (sc.scroll_offset + sc.cache_origin).max(0.0);
    let end = start + sc.remaining_cache_extent;
    let first = min_index(start, item_extent).clamp(0.0, count);
    let last = if end.is_finite() {
        end_index(end, item_extent).clamp(0.0, count)
    } else {
        count
    };
    let (first, last) = (first as usize, last as usize);
    first.min(last)..last
}

impl SliverFixedExtentListWidget {
    fn new(item_count: usize, item_extent: f64) -> Self {
        Self {
            children: Vec::new(),
            indices: Vec::new(),
            key_index: HashMap::new(),
            item_count,
            item_extent,
            constraints: None,
            paint_extent: 0.0,
            clip: false,
            observed_at: None,
            near_start: NearEdge::new(),
            near_end: NearEdge::new(),
        }
    }

    /// The item indices currently materialised, ascending.
    pub fn window(&self) -> &[usize] {
        &self.indices
    }

    /// How many items the list holds.
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// The rows the last layout's cache region asks for — empty before the
    /// first layout.
    fn desired_window(&self) -> Range<usize> {
        match &self.constraints {
            Some(sc) => cache_window(sc, self.item_count, self.item_extent),
            None => 0..0,
        }
    }

    /// Whether the materialised rows cover `range`.
    fn covers(&self, range: &Range<usize>) -> bool {
        if range.is_empty() {
            return true;
        }
        match (self.indices.first(), self.indices.last()) {
            (Some(&first), Some(&last)) => first <= range.start && last + 1 >= range.end,
            _ => false,
        }
    }

    /// Observe both near edges at `sc` — only once the list has moved against
    /// the viewport since the last observation (the module docs' *Near-edge
    /// callbacks*). Owes the flush that delivers a fire.
    fn observe_edges(&mut self, sc: &SliverConstraints, total: f64) {
        let at = (sc.scroll_offset, sc.remaining_paint_extent);
        let previous = self.observed_at.replace(at);
        if previous.is_none_or(|previous| previous == at) || self.item_count == 0 {
            return;
        }
        let to_end = non_negative(total - sc.scroll_offset - sc.remaining_paint_extent);
        let start = self.near_start.observe(sc.scroll_offset);
        let end = self.near_end.observe(to_end);
        if start || end {
            frust_core::mark_pending_result_flush();
        }
    }
}

impl<State: 'static> SliverFixedExtentListView<State> {
    /// Reinstall the near-edge callbacks (closures are not comparable, so
    /// every build and rebuild does).
    fn install_edges(&self, widget: &mut SliverFixedExtentListWidget) {
        widget.near_start.callback = self
            .on_near_start
            .as_ref()
            .map(crate::authoring::erase_callback);
        widget.near_start.threshold = self.near_start_threshold;
        widget.near_end.callback = self
            .on_near_end
            .as_ref()
            .map(crate::authoring::erase_callback);
        widget.near_end.threshold = self.near_end_threshold;
    }
}

impl<State: 'static> View<State> for SliverFixedExtentListView<State> {
    type Element = SliverFixedExtentListWidget;

    /// No layout has run yet, so there is no cache region to window against:
    /// nothing is materialised until the first rebuild after it.
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SliverFixedExtentListWidget {
        let mut widget = SliverFixedExtentListWidget::new(self.item_count, self.item_extent);
        self.install_edges(&mut widget);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverFixedExtentListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.install_edges(element);
        let mut flags = ChangeFlags::NONE;
        if element.item_count != self.item_count {
            element.item_count = self.item_count;
            // Content length changed: both edges may fire again.
            element.near_start.armed = true;
            element.near_end.armed = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.item_extent != self.item_extent {
            element.item_extent = self.item_extent;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let range = element.desired_window();
        let views = ItemViews {
            prev: &*prev.builder,
            next: &*self.builder,
        };
        let rows = LiveRows {
            children: &mut element.children,
            indices: &mut element.indices,
        };
        flags |= match self.key_of.as_ref() {
            Some(key_of) => {
                reconcile::reconcile_keyed(
                    views,
                    rows,
                    &mut element.key_index,
                    &**key_of,
                    range,
                    DUPLICATE_KEY_MSG,
                    ctx,
                )
                .flags
            }
            None => {
                // A positional frame owns no key identities: drop whatever map
                // a previous keyed frame left, so a list switched back to keyed
                // later cannot match against a window this path materialised.
                element.key_index.clear();
                reconcile::reconcile_positional(views, rows, range, ctx)
            }
        };
        flags
    }

    fn teardown(&self, element: &mut SliverFixedExtentListWidget, ctx: &mut BuildCtx<'_>) {
        for (index, pod) in element.indices.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(&(self.builder)(*index), pod, ctx);
        }
    }
}

impl Widget for SliverFixedExtentListWidget {
    /// Outside a viewport the list lays out as if it were the only sliver of a
    /// viewport `bc.max()` long, scrolled to its start: it materialises (at
    /// the next rebuild) and sizes itself to the rows that fit, or to all of
    /// them under an unbounded constraint.
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let axis = self.constraints.map_or(ScrollAxis::Vertical, |sc| sc.axis);
        let max = bc.max();
        let main = non_negative(axis.main_extent(max));
        let cross = finite_extent(axis.cross_extent(max));
        let sc = SliverConstraints {
            axis,
            scroll_offset: 0.0,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: main,
            cross_axis_extent: cross,
            viewport_main_axis_extent: main,
            cache_origin: 0.0,
            remaining_cache_extent: main,
            user_scroll_direction: ScrollDirection::Idle,
        };
        let geometry = self.layout_sliver(ctx, &sc);
        bc.constrain(axis.size(geometry.paint_extent, cross))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.clip {
            scene.push_clip(ctx.origin(), ctx.size());
        }
        let axis = self.constraints.map_or(ScrollAxis::Vertical, |sc| sc.axis);
        let (extent, paint_extent) = (self.item_extent, self.paint_extent);
        for pod in &mut self.children {
            // A row only cached is laid out but never painted.
            let origin = pod.origin();
            let start = axis.main_extent(Size::new(origin.x, origin.y));
            if start + extent > 0.0 && start < paint_extent {
                pod.paint_child(ctx, scene);
            }
        }
        if self.clip {
            scene.pop_clip();
        }
        // The next rebuild re-windows to what this frame's layout asked for;
        // make sure it runs. A fire observed at layout owes the same frame.
        let wanted = self.desired_window();
        if !self.covers(&wanted) || self.near_start.pending || self.near_end.pending {
            ctx.request_frame();
        }
    }

    /// Delivers a near-edge fire observed at layout, then routes the event to
    /// the rows: the viewport only delivers a positional event inside this
    /// sliver's paint extent, where only painted rows sit.
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.near_start.deliver(ctx);
        self.near_end.deliver(ctx);
        crate::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }

    crate::authoring::visit_children!(children);
}

impl SliverWidget for SliverFixedExtentListWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let axis = sc.axis;
        let extent = self.item_extent;
        let total = self.item_count as f64 * extent;
        let row = BoxConstraints::tight(axis.size(extent, sc.cross_axis_extent));
        for (index, pod) in self.indices.iter().zip(self.children.iter_mut()) {
            pod.layout_child(ctx, &row);
            pod.set_origin(axis_point(
                axis,
                *index as f64 * extent - sc.scroll_offset,
                0.0,
            ));
        }
        let paint_extent = paint_offset(sc, 0.0, total);
        let geometry = SliverGeometry {
            cache_extent: cache_offset(sc, 0.0, total).max(paint_extent),
            has_visual_overflow: total > sc.scroll_offset + sc.remaining_paint_extent
                || sc.scroll_offset > 0.0,
            ..SliverGeometry::new(total, paint_extent, total)
        };
        self.constraints = Some(*sc);
        self.paint_extent = paint_extent;
        self.clip = geometry.has_visual_overflow;
        self.observe_edges(sc, total);
        geometry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constraints(scroll_offset: f64, cache: f64) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 500.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 500.0,
            cache_origin: -scroll_offset.min(cache),
            remaining_cache_extent: 500.0 + cache + scroll_offset.min(cache),
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    #[test]
    fn the_window_is_exactly_the_rows_the_cache_region_touches() {
        // [0, 750): rows 0..15 of 50 px.
        assert_eq!(cache_window(&constraints(0.0, 250.0), 10_000, 50.0), 0..15);
        // [750, 1750) at offset 1000: rows 15..35.
        assert_eq!(
            cache_window(&constraints(1000.0, 250.0), 10_000, 50.0),
            15..35
        );
        // A boundary a hair off a row edge does not pull in a neighbour.
        let sc = SliverConstraints {
            scroll_offset: 1000.0 + 1e-12,
            ..constraints(1000.0, 250.0)
        };
        assert_eq!(cache_window(&sc, 10_000, 50.0), 15..35);
        // Mid-row edges round outward.
        assert_eq!(
            cache_window(&constraints(1010.0, 250.0), 10_000, 50.0),
            15..36
        );
        // Clamped to the item count, and empty past it or with no cache space.
        assert_eq!(cache_window(&constraints(0.0, 250.0), 4, 50.0), 0..4);
        assert_eq!(cache_window(&constraints(5000.0, 250.0), 4, 50.0), 4..4);
        let none = SliverConstraints {
            remaining_cache_extent: 0.0,
            ..constraints(0.0, 250.0)
        };
        assert!(cache_window(&none, 100, 50.0).is_empty());
        assert!(cache_window(&constraints(0.0, 250.0), 0, 50.0).is_empty());
        // An unbounded region takes every row.
        let all = SliverConstraints {
            remaining_cache_extent: f64::INFINITY,
            ..constraints(0.0, 250.0)
        };
        assert_eq!(cache_window(&all, 7, 50.0), 0..7);
    }

    #[test]
    fn a_near_edge_fires_once_per_approach() {
        let mut edge = NearEdge::new();
        assert!(!edge.observe(0.0), "no callback, no fire");
        edge.callback = Some(Box::new(|_: &mut EventCtx| {}));
        edge.threshold = 100.0;
        assert!(edge.observe(50.0));
        assert!(edge.pending);
        assert!(!edge.observe(20.0), "disarmed");
        assert!(!edge.observe(150.0), "not yet past twice the threshold");
        assert!(!edge.observe(80.0));
        assert!(!edge.observe(250.0));
        assert!(edge.observe(90.0), "rearmed past twice the threshold");
    }
}
