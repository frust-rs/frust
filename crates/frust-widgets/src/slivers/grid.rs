//! `sliver_grid`: a lazy grid of equal cells — Flutter's `SliverGrid`
//! (`rendering/sliver_grid.dart`'s `RenderSliverGrid` with
//! `SliverGridDelegateWithFixedCrossAxisCount` /
//! `SliverGridDelegateWithMaxCrossAxisExtent`), materialised the way
//! [`sliver_fixed_extent_list`](crate::sliver_fixed_extent_list) materialises
//! its rows.
//!
//! # Geometry
//!
//! A [`SliverGridDelegate`] turns the cross-axis extent into the grid's
//! layout: how many cells share a row, each cell's extent across
//! (`(cross − cross_spacing × (count − 1)) / count`) and along
//! (`across / child_aspect_ratio`) the axis, and the row stride along it
//! (`along + main_spacing`). Item `i` sits in row `i / count`, column
//! `i % count`, laid out tight to the cell. The scroll extent is
//! `rows × stride − main_spacing` — the trailing spacing is not content —
//! a function of the constraints and the item count alone, never of which
//! cells are materialised.
//!
//! # Materialised at rebuild time
//!
//! Exactly the fixed-extent list's rule, by row: the rows whose stride touches
//! the cache region the last layout asked for are materialised at the next
//! rebuild — the cells `[first_row × count, end_row × count)` clamped to the
//! item count — reconciled by item index ([`sliver_grid`]) or by a stable key
//! ([`sliver_grid_keyed`]) through the reconcilers `ListView` shares, with the
//! same one-frame catch-up. Paint clips when a row is cut and skips the rows
//! wholly outside the paint extent.

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

/// The debug-only tripwire message for two materialised cells claiming one
/// key.
const DUPLICATE_KEY_MSG: &str = "sliver_grid_keyed produced a duplicate key inside one window: \
     cell identity is ambiguous (release: the first slot claiming a key keeps the live cell, \
     later duplicates build fresh)";

/// A view-held item builder, erased per cell.
type ItemBuilder<State> = Rc<dyn Fn(usize) -> AnyView<State>>;

/// A view-held stable-key function (`item index -> cell identity`).
type KeyOf = Rc<dyn Fn(usize) -> ChildKey>;

/// How a [`sliver_grid`] divides its cross axis into cells — Flutter's two
/// stock `SliverGridDelegate`s. Spacings are logical px; `child_aspect_ratio`
/// is a cell's cross-axis extent over its main-axis extent (width over height
/// in a vertical grid).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SliverGridDelegate {
    /// Exactly `count` cells per row.
    FixedCrossAxisCount {
        count: usize,
        main_spacing: f64,
        cross_spacing: f64,
        child_aspect_ratio: f64,
    },
    /// As few cells per row as keep each one at most `max_cross_extent`
    /// across: `ceil(cross / (max_cross_extent + cross_spacing))`, at least
    /// one.
    MaxCrossAxisExtent {
        max_cross_extent: f64,
        main_spacing: f64,
        cross_spacing: f64,
        child_aspect_ratio: f64,
    },
}

impl Default for SliverGridDelegate {
    /// Two square cells per row, no spacing.
    fn default() -> Self {
        Self::FixedCrossAxisCount {
            count: 2,
            main_spacing: 0.0,
            cross_spacing: 0.0,
            child_aspect_ratio: 1.0,
        }
    }
}

/// One grid's resolved layout for a cross-axis extent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GridLayout {
    count: usize,
    cell_main: f64,
    cell_cross: f64,
    main_stride: f64,
    cross_stride: f64,
    main_spacing: f64,
}

impl SliverGridDelegate {
    /// Panic unless every parameter is usable (the variant docs).
    fn validate(&self) {
        let (main_spacing, cross_spacing, ratio) = match *self {
            Self::FixedCrossAxisCount {
                count,
                main_spacing,
                cross_spacing,
                child_aspect_ratio,
            } => {
                assert!(count > 0, "SliverGridDelegate count must be at least 1");
                (main_spacing, cross_spacing, child_aspect_ratio)
            }
            Self::MaxCrossAxisExtent {
                max_cross_extent,
                main_spacing,
                cross_spacing,
                child_aspect_ratio,
            } => {
                assert!(
                    max_cross_extent > 0.0 && max_cross_extent.is_finite(),
                    "SliverGridDelegate max_cross_extent must be positive and finite"
                );
                (main_spacing, cross_spacing, child_aspect_ratio)
            }
        };
        assert!(
            main_spacing >= 0.0 && main_spacing.is_finite(),
            "SliverGridDelegate main_spacing must be non-negative and finite"
        );
        assert!(
            cross_spacing >= 0.0 && cross_spacing.is_finite(),
            "SliverGridDelegate cross_spacing must be non-negative and finite"
        );
        assert!(
            ratio > 0.0 && ratio.is_finite(),
            "SliverGridDelegate child_aspect_ratio must be positive and finite"
        );
    }

    /// The layout for `cross` px across — `getLayout` of Flutter's delegate.
    fn layout(&self, cross: f64) -> GridLayout {
        let (count, main_spacing, cross_spacing, ratio) = match *self {
            Self::FixedCrossAxisCount {
                count,
                main_spacing,
                cross_spacing,
                child_aspect_ratio,
            } => (count, main_spacing, cross_spacing, child_aspect_ratio),
            Self::MaxCrossAxisExtent {
                max_cross_extent,
                main_spacing,
                cross_spacing,
                child_aspect_ratio,
            } => {
                let count = (cross / (max_cross_extent + cross_spacing)).ceil();
                let count = if count.is_finite() {
                    count.max(1.0)
                } else {
                    1.0
                };
                (
                    count as usize,
                    main_spacing,
                    cross_spacing,
                    child_aspect_ratio,
                )
            }
        };
        let count = count.max(1);
        let usable = (cross - cross_spacing * (count - 1) as f64).max(0.0);
        let cell_cross = usable / count as f64;
        let cell_main = cell_cross / ratio;
        GridLayout {
            count,
            cell_main,
            cell_cross,
            main_stride: cell_main + main_spacing,
            cross_stride: cell_cross + cross_spacing,
            main_spacing,
        }
    }

    /// The main-axis offset of item `index`'s row from the grid's leading edge,
    /// in the grid's own sliver coordinates, for a grid `cross_extent` px across.
    ///
    /// **`0.0` is also the answer for an invalid or not-yet-measured cross
    /// extent** (zero, negative or non-finite), indistinguishable from row 0's
    /// offset: pass the grid's real laid-out width, never a placeholder.
    ///
    /// It is computed from the same layout the grid places its cells with, so
    /// it cannot drift from where the cell lands. To scroll a
    /// [`CustomScrollView`](crate::CustomScrollView) to a cell, add the extent
    /// of the slivers above the grid and pass the result to
    /// [`ScrollController::jump_to`](crate::ScrollController::jump_to) or
    /// [`ScrollController::animate_to`](crate::ScrollController::animate_to):
    ///
    /// ```
    /// use frust_widgets::{ScrollController, SliverGridDelegate};
    /// let delegate = SliverGridDelegate::FixedCrossAxisCount {
    ///     count: 4,
    ///     main_spacing: 8.0,
    ///     cross_spacing: 8.0,
    ///     child_aspect_ratio: 1.0,
    /// };
    /// let controller = ScrollController::new();
    /// let preceding_extent = 56.0; // the slivers above the grid
    /// let cross_extent = 400.0;
    /// controller.jump_to(preceding_extent + delegate.offset_of_index(42, cross_extent));
    /// ```
    ///
    /// Likewise `0.0` whenever the row stride is not a positive finite
    /// length, rather than panicking.
    pub fn offset_of_index(&self, index: usize, cross_extent: f64) -> f64 {
        if !(cross_extent > 0.0 && cross_extent.is_finite()) {
            return 0.0;
        }
        let grid = self.layout(cross_extent);
        if !(grid.main_stride > 0.0 && grid.main_stride.is_finite()) {
            return 0.0;
        }
        (index / grid.count) as f64 * grid.main_stride
    }
}

impl GridLayout {
    /// How many rows `item_count` cells fill.
    fn rows(&self, item_count: usize) -> usize {
        item_count.div_ceil(self.count)
    }

    /// The scroll extent of `item_count` cells.
    fn extent(&self, item_count: usize) -> f64 {
        let rows = self.rows(item_count);
        if rows == 0 {
            0.0
        } else {
            (rows as f64 * self.main_stride - self.main_spacing).max(0.0)
        }
    }

    /// The cells covering the cache region of `sc` — whole rows, clamped to
    /// `item_count`; Flutter's `getMinChildIndexForScrollOffset` /
    /// `getMaxChildIndexForScrollOffset` over the row stride.
    fn cache_window(&self, sc: &SliverConstraints, item_count: usize) -> Range<usize> {
        if item_count == 0 || sc.remaining_cache_extent <= 0.0 || self.main_stride <= 0.0 {
            return 0..0;
        }
        let rows = self.rows(item_count) as f64;
        let start = (sc.scroll_offset + sc.cache_origin).max(0.0);
        let end = start + sc.remaining_cache_extent;
        let first_row = row_at(start, self.main_stride, f64::floor).clamp(0.0, rows);
        let end_row = if end.is_finite() {
            row_at(end, self.main_stride, f64::ceil).clamp(0.0, rows)
        } else {
            rows
        };
        let first = (first_row as usize * self.count).min(item_count);
        let end = (end_row as usize * self.count).min(item_count);
        first.min(end)..end
    }
}

/// `offset / stride` rounded by `round`, snapping to the nearest whole row
/// within [`PRECISION_ERROR_TOLERANCE`] so float noise at a row boundary
/// pulls in no neighbour.
fn row_at(offset: f64, stride: f64, round: fn(f64) -> f64) -> f64 {
    let actual = offset / stride;
    let nearest = actual.round();
    if (actual * stride - nearest * stride).abs() < PRECISION_ERROR_TOLERANCE {
        nearest
    } else {
        round(actual)
    }
}

/// A lazy grid of equal cells. See the [module docs](self).
pub struct SliverGridView<State: 'static> {
    item_count: usize,
    delegate: SliverGridDelegate,
    builder: ItemBuilder<State>,
    /// `None` reconciles cells by item index.
    key_of: Option<KeyOf>,
}

/// A lazy grid of `item_count` cells, the cell at `index` being
/// `builder(index)`, laid out by [`SliverGridView::delegate`] (two square
/// cells per row unless set) — reconciled by item index. See the
/// [module docs](self).
///
/// ```
/// use frust_widgets::{
///     CustomScrollView, SliverGridDelegate, custom_scroll_view, sliver_grid, text,
/// };
/// let view: CustomScrollView<()> = custom_scroll_view(
///     sliver_grid(1_000, |i| text(format!("cell {i}"))).delegate(
///         SliverGridDelegate::FixedCrossAxisCount {
///             count: 4,
///             main_spacing: 8.0,
///             cross_spacing: 8.0,
///             child_aspect_ratio: 1.0,
///         },
///     ),
/// );
/// # let _ = view;
/// ```
pub fn sliver_grid<State: 'static, V: View<State>>(
    item_count: usize,
    builder: impl Fn(usize) -> V + 'static,
) -> SliverGridView<State> {
    SliverGridView::new(item_count, None, builder)
}

/// [`sliver_grid`] reconciled by the stable key `key_of(index)` instead of by
/// item index, with
/// [`sliver_fixed_extent_list_keyed`](crate::sliver_fixed_extent_list_keyed)'s
/// contract.
///
/// ```
/// use frust_widgets::{ChildKey, CustomScrollView, custom_scroll_view, sliver_grid_keyed, text};
/// let view: CustomScrollView<()> = custom_scroll_view(sliver_grid_keyed(
///     100,
///     |i| ChildKey::new(i as u64),
///     |i| text(format!("photo {i}")),
/// ));
/// # let _ = view;
/// ```
pub fn sliver_grid_keyed<State: 'static, V: View<State>>(
    item_count: usize,
    key_of: impl Fn(usize) -> ChildKey + 'static,
    builder: impl Fn(usize) -> V + 'static,
) -> SliverGridView<State> {
    SliverGridView::new(item_count, Some(Rc::new(key_of)), builder)
}

impl<State: 'static> SliverGridView<State> {
    fn new<V: View<State>>(
        item_count: usize,
        key_of: Option<KeyOf>,
        builder: impl Fn(usize) -> V + 'static,
    ) -> Self {
        Self {
            item_count,
            delegate: SliverGridDelegate::default(),
            builder: Rc::new(move |index| any(builder(index))),
            key_of,
        }
    }

    /// Lay the cells out with `delegate`.
    ///
    /// # Panics
    ///
    /// If a count is zero, a spacing negative or not finite, or an extent or
    /// aspect ratio not positive and finite.
    pub fn delegate(mut self, delegate: SliverGridDelegate) -> Self {
        delegate.validate();
        self.delegate = delegate;
        self
    }
}

/// The retained widget for a [`SliverGridView`]: the materialised cells, the
/// constraints rebuild windows against, and the delegate.
pub struct SliverGridWidget {
    /// The materialised cells, parallel to [`Self::indices`].
    children: Vec<ChildPod>,
    /// The item index each cell renders — a contiguous ascending run.
    indices: Vec<usize>,
    /// Cell identity for a keyed grid; empty for a positional one.
    key_index: HashMap<ChildKey, usize>,
    item_count: usize,
    delegate: SliverGridDelegate,
    /// The constraints the last sliver layout ran with. `None` until the
    /// first layout.
    constraints: Option<SliverConstraints>,
    /// The layout the last sliver layout resolved.
    grid: Option<GridLayout>,
    /// The paint extent of the last sliver layout.
    paint_extent: f64,
    /// Whether the last layout cut a row at the paint extent (so paint clips).
    clip: bool,
}

impl SliverGridWidget {
    fn new(item_count: usize, delegate: SliverGridDelegate) -> Self {
        Self {
            children: Vec::new(),
            indices: Vec::new(),
            key_index: HashMap::new(),
            item_count,
            delegate,
            constraints: None,
            grid: None,
            paint_extent: 0.0,
            clip: false,
        }
    }

    /// The item indices currently materialised, ascending.
    pub fn window(&self) -> &[usize] {
        &self.indices
    }

    /// How many items the grid holds.
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// The cells the last layout's cache region asks for — empty before the
    /// first layout.
    fn desired_window(&self) -> Range<usize> {
        match &self.constraints {
            Some(sc) => self
                .delegate
                .layout(sc.cross_axis_extent)
                .cache_window(sc, self.item_count),
            None => 0..0,
        }
    }

    /// Whether the materialised cells cover `range`.
    fn covers(&self, range: &Range<usize>) -> bool {
        if range.is_empty() {
            return true;
        }
        match (self.indices.first(), self.indices.last()) {
            (Some(&first), Some(&last)) => first <= range.start && last + 1 >= range.end,
            _ => false,
        }
    }
}

impl<State: 'static> View<State> for SliverGridView<State> {
    type Element = SliverGridWidget;

    /// No layout has run yet: nothing is materialised until the first rebuild
    /// after it.
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SliverGridWidget {
        SliverGridWidget::new(self.item_count, self.delegate)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverGridWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.item_count != self.item_count {
            element.item_count = self.item_count;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.delegate != self.delegate {
            element.delegate = self.delegate;
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
                // A positional frame owns no key identities.
                element.key_index.clear();
                reconcile::reconcile_positional(views, rows, range, ctx)
            }
        };
        flags
    }

    fn teardown(&self, element: &mut SliverGridWidget, ctx: &mut BuildCtx<'_>) {
        for (index, pod) in element.indices.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(&(self.builder)(*index), pod, ctx);
        }
    }
}

impl Widget for SliverGridWidget {
    /// Outside a viewport the grid lays out as if it were the only sliver of a
    /// viewport `bc.max()` long, scrolled to its start (the fixed-extent
    /// list's rule).
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
        let cell = self.grid.map_or(0.0, |grid| grid.cell_main);
        let paint_extent = self.paint_extent;
        for pod in &mut self.children {
            // A row only cached is laid out but never painted.
            let origin = pod.origin();
            let start = axis.main_extent(Size::new(origin.x, origin.y));
            if start + cell > 0.0 && start < paint_extent {
                pod.paint_child(ctx, scene);
            }
        }
        if self.clip {
            scene.pop_clip();
        }
        // The next rebuild re-windows to what this frame's layout asked for.
        if !self.covers(&self.desired_window()) {
            ctx.request_frame();
        }
    }

    /// The viewport only delivers a positional event inside this sliver's
    /// paint extent, where only painted cells sit.
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    /// [`frust_core::InspectDetail::Clip`] while the grid clips tiles past its paint extent.
    fn inspect_detail(&self) -> Option<frust_core::InspectDetail> {
        self.clip.then_some(frust_core::InspectDetail::Clip)
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }

    crate::authoring::visit_children!(children);
}

impl SliverWidget for SliverGridWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        let axis = sc.axis;
        let grid = self.delegate.layout(sc.cross_axis_extent);
        let cell = BoxConstraints::tight(axis.size(grid.cell_main, grid.cell_cross));
        for (index, pod) in self.indices.iter().zip(self.children.iter_mut()) {
            let (row, column) = (index / grid.count, index % grid.count);
            pod.layout_child(ctx, &cell);
            pod.set_origin(axis_point(
                axis,
                row as f64 * grid.main_stride - sc.scroll_offset,
                column as f64 * grid.cross_stride,
            ));
        }
        let total = grid.extent(self.item_count);
        let paint_extent = paint_offset(sc, 0.0, total);
        let geometry = SliverGeometry {
            cache_extent: cache_offset(sc, 0.0, total).max(paint_extent),
            has_visual_overflow: total > sc.scroll_offset + sc.remaining_paint_extent
                || sc.scroll_offset > 0.0,
            ..SliverGeometry::new(total, paint_extent, total)
        };
        self.constraints = Some(*sc);
        self.grid = Some(grid);
        self.paint_extent = paint_extent;
        self.clip = geometry.has_visual_overflow;
        geometry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use frust_core::RenderRoot;

    use crate::test_support::leaf;
    use crate::viewport::ViewportWidget;
    use crate::{CustomScrollView, custom_scroll_view};

    fn constraints(scroll_offset: f64, cross: f64) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 600.0,
            cross_axis_extent: cross,
            viewport_main_axis_extent: 600.0,
            cache_origin: 0.0,
            remaining_cache_extent: 600.0,
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    #[test]
    fn both_delegates_resolve_the_same_four_column_layout() {
        let fixed = SliverGridDelegate::FixedCrossAxisCount {
            count: 4,
            main_spacing: 10.0,
            cross_spacing: 8.0,
            child_aspect_ratio: 2.0,
        };
        let max = SliverGridDelegate::MaxCrossAxisExtent {
            max_cross_extent: 120.0,
            main_spacing: 10.0,
            cross_spacing: 8.0,
            child_aspect_ratio: 2.0,
        };
        let expected = GridLayout {
            count: 4,
            cell_main: 50.0,
            cell_cross: 100.0,
            main_stride: 60.0,
            cross_stride: 108.0,
            main_spacing: 10.0,
        };
        assert_eq!(fixed.layout(424.0), expected);
        assert_eq!(max.layout(424.0), expected);
        // Ten cells fill three rows; the trailing spacing is not content.
        assert_eq!(expected.extent(10), 170.0);
        assert_eq!(expected.extent(0), 0.0);
        // A narrow viewport still gets one column.
        assert_eq!(max.layout(50.0).count, 1);
    }

    #[test]
    fn the_window_is_the_whole_rows_the_cache_region_touches() {
        let grid = SliverGridDelegate::FixedCrossAxisCount {
            count: 4,
            main_spacing: 10.0,
            cross_spacing: 8.0,
            child_aspect_ratio: 2.0,
        }
        .layout(424.0);
        // [0, 600): rows 0..10 (row 10 starts exactly at 600).
        assert_eq!(grid.cache_window(&constraints(0.0, 424.0), 1000), 0..40);
        // [6000, 6600): rows 100..110.
        assert_eq!(
            grid.cache_window(&constraints(6000.0, 424.0), 1000),
            400..440
        );
        // Mid-row edges round outward; the last row is partial.
        assert_eq!(grid.cache_window(&constraints(30.0, 424.0), 1000), 0..44);
        assert_eq!(grid.cache_window(&constraints(0.0, 424.0), 10), 0..10);
        assert!(grid.cache_window(&constraints(0.0, 424.0), 0).is_empty());
    }

    #[test]
    #[should_panic(expected = "count must be at least 1")]
    fn a_zero_column_delegate_is_refused() {
        let _ = sliver_grid::<(), _>(10, |_| crate::text("cell")).delegate(
            SliverGridDelegate::FixedCrossAxisCount {
                count: 0,
                main_spacing: 0.0,
                cross_spacing: 0.0,
                child_aspect_ratio: 1.0,
            },
        );
    }

    /// Mount a `count`-item grid as the only sliver of a viewport `cross` px
    /// wide and 2000 px tall, unscrolled, and lay it out.
    fn mount(
        item_count: usize,
        delegate: SliverGridDelegate,
        cross: f64,
    ) -> RenderRoot<(), CustomScrollView<()>> {
        // Leaves rather than text: a bare layout pass threads no text context.
        let mut logic = move |_: &mut ()| {
            custom_scroll_view(sliver_grid(item_count, |_| leaf(1.0, 1.0)).delegate(delegate))
        };
        let mut root = RenderRoot::new();
        // The first rebuild materialises nothing (no layout has run yet); the
        // second materialises the window the first layout asked for.
        for _ in 0..2 {
            root.rebuild(&mut logic, &mut ());
            root.layout(Size::new(cross, 2000.0));
        }
        root
    }

    /// Each materialised cell's `(item index, main-axis origin)` from the last
    /// layout, the origin relative to the grid's leading edge.
    fn placed_rows(root: &RenderRoot<(), CustomScrollView<()>>) -> Vec<(usize, f64)> {
        let id = root.root_id().expect("root built");
        let viewport = (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ViewportWidget>()
            .expect("the root is the viewport");
        let mut placed = Vec::new();
        viewport.visit_children(&mut |sliver: &ChildPod| {
            let boxed = (sliver.widget() as &dyn Any)
                .downcast_ref::<Box<dyn Widget>>()
                .expect("AnyView-erased sliver: double-boxed ChildPod element");
            let grid = (boxed.as_ref() as &dyn Any)
                .downcast_ref::<SliverGridWidget>()
                .expect("the viewport's child is the grid");
            for (index, pod) in grid.indices.iter().zip(grid.children.iter()) {
                placed.push((*index, pod.origin().y));
            }
        });
        placed
    }

    #[test]
    fn offset_of_index_matches_the_placed_cell_origin() {
        let delegates = [
            SliverGridDelegate::FixedCrossAxisCount {
                count: 4,
                main_spacing: 10.0,
                cross_spacing: 8.0,
                child_aspect_ratio: 2.0,
            },
            SliverGridDelegate::FixedCrossAxisCount {
                count: 3,
                main_spacing: 0.0,
                cross_spacing: 0.0,
                child_aspect_ratio: 1.0,
            },
            SliverGridDelegate::MaxCrossAxisExtent {
                max_cross_extent: 120.0,
                main_spacing: 6.0,
                cross_spacing: 4.0,
                child_aspect_ratio: 1.5,
            },
        ];
        for delegate in delegates {
            for cross in [200.0, 424.0, 640.0] {
                // Ten items: with four or three columns the last row is partial
                // and index 5 sits mid-grid.
                let root = mount(10, delegate, cross);
                let placed = placed_rows(&root);
                assert_eq!(
                    placed.len(),
                    10,
                    "{delegate:?} at {cross}: every cell materialised"
                );
                for (index, origin) in placed {
                    assert_eq!(
                        origin,
                        delegate.offset_of_index(index, cross),
                        "{delegate:?} at {cross}: item {index}"
                    );
                }
            }
        }
    }

    #[test]
    fn offset_of_index_places_first_mid_and_last_partial_rows() {
        let delegate = SliverGridDelegate::FixedCrossAxisCount {
            count: 4,
            main_spacing: 10.0,
            cross_spacing: 8.0,
            child_aspect_ratio: 2.0,
        };
        // 424 px across: 4 cells of 100 px, 60 px row stride.
        assert_eq!(delegate.offset_of_index(0, 424.0), 0.0);
        assert_eq!(delegate.offset_of_index(3, 424.0), 0.0);
        assert_eq!(delegate.offset_of_index(5, 424.0), 60.0);
        assert_eq!(delegate.offset_of_index(9, 424.0), 120.0);
    }

    #[test]
    fn offset_of_index_returns_zero_for_degenerate_inputs() {
        let fixed = SliverGridDelegate::FixedCrossAxisCount {
            count: 4,
            main_spacing: 10.0,
            cross_spacing: 8.0,
            child_aspect_ratio: 2.0,
        };
        assert_eq!(fixed.offset_of_index(9, 0.0), 0.0);
        assert_eq!(fixed.offset_of_index(9, -5.0), 0.0);
        assert_eq!(fixed.offset_of_index(9, f64::NAN), 0.0);
        assert_eq!(fixed.offset_of_index(9, f64::INFINITY), 0.0);
        // An infinite aspect ratio collapses the cell to zero height; with no
        // main spacing the stride is zero.
        let flat = SliverGridDelegate::FixedCrossAxisCount {
            count: 1,
            main_spacing: 0.0,
            cross_spacing: 0.0,
            child_aspect_ratio: f64::INFINITY,
        };
        assert_eq!(flat.offset_of_index(3, 100.0), 0.0);
        let max = SliverGridDelegate::MaxCrossAxisExtent {
            max_cross_extent: 120.0,
            main_spacing: 0.0,
            cross_spacing: 0.0,
            child_aspect_ratio: 1.0,
        };
        assert_eq!(max.offset_of_index(7, 0.0), 0.0);
    }
}
