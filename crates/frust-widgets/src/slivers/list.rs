//! `sliver_list`: a lazy, keyed list of rows that size themselves — Flutter's
//! `SliverList` (`rendering/sliver_list.dart`'s `RenderSliverList`, over
//! `sliver_multi_box_adaptor.dart`), materialised the way
//! [`sliver_fixed_extent_list`](crate::sliver_fixed_extent_list) materialises
//! its rows.
//!
//! # Rows, identity and extents
//!
//! Every row is keyed: `key_of(index)` is its identity, with
//! [`ListView::builder_keyed`](crate::ListView::builder_keyed)'s contract (a
//! duplicate key in one window trips a `debug_assert!`; release keeps the
//! first claim). A row is laid out tight across the axis and unbounded along
//! it, and its extent along the axis is cached under its key — `ListView`'s
//! *Variable extents* bookkeeping, shared through `slivers/reconcile.rs`: a row
//! never laid out is assumed [`SliverListView::estimated_item_extent`] long
//! (48 px unless set), so the list's scroll extent is
//! `Σ measured + estimate × unmeasured` and converges on the true total as rows
//! are visited (never less than the materialised rows' own end). Once the
//! last row is in the window the scroll extent is that row's laid-out end
//! exactly. A row that leaves the data drops its
//! measurement (the same departing-row and shrink rules `ListView` runs).
//!
//! # Materialised at rebuild time, positioned from an anchor
//!
//! Rows are built only in [`View::rebuild`], over the cache region the last
//! sliver layout asked for, with the fixed-extent list's one-frame catch-up
//! (paint requests a frame whenever the rows do not cover it). The window is
//! planned from an *anchor*: one row and the content offset it starts at.
//! Layout records it as the row the paint region's leading edge sits in;
//! rebuild walks from it to the leading edge over measured-or-estimated
//! extents (a bounded walk; a jump resolves in closed form) and widens to the
//! cache region. Layout places that row at its recorded offset and every other
//! row from it, both ways, by the rows' own laid-out extents — so a row
//! measuring differently than it was assumed moves the rows on its far side of
//! the anchor, never the row the user is looking at.
//!
//! # Corrections through the viewport
//!
//! What the anchor cannot absorb is reported as a
//! [`scroll_offset_correction`](SliverGeometry::scroll_offset_correction),
//! which the [`CustomScrollView`](crate::CustomScrollView) applies to its
//! scroll position before laying every sliver out again — the content moves
//! by the correction and so does the position, so nothing on screen moves:
//!
//! * **Keyed anchoring.** A rebuild whose data moved the anchor row — rows
//!   prepended or removed above it — finds the anchor's key again, moves it to
//!   its new index and shifts its offset by the rows it moved times the
//!   estimate (a prepended row is by definition unmeasured), and the next
//!   layout reports that shift. The key is looked up at its unchanged index,
//!   at its index shifted by the frame's net item-count change, and then by a
//!   bounded scan around them (`reconcile::locate_key`), so a mutation on both
//!   sides of the anchor in one frame (a prepend above plus an append below)
//!   is anchored as exactly as a one-sided one. An anchor row that left the
//!   data, or moved beyond the scan, anchors nothing: the content after the
//!   mutation simply moves. Anchoring applies while the list's start is at or
//!   above the viewport's leading edge (scrolled into, or flush with the top);
//!   a list starting further down the viewport lets a prepend push it down.
//! * **The leading edge.** Positions derived from an anchor can disagree with
//!   the list's start once rows above it are measured: row 0 must start at
//!   offset 0, and rows above the window need room. A layout finding row 0
//!   elsewhere, or a later first row at or before offset 0, shifts every row
//!   to fix it (row 0 to 0; any other first row to the room the extent model
//!   gives the rows before it) and reports the shift — while the list starts
//!   at or above the leading edge and the shifted position stays at or past
//!   the list's start. Otherwise there is nothing on screen positioned from
//!   the list's inside to hold still, and the shift is silent: a gap above
//!   row 0 wider than the scroll position (rows above measured shorter than
//!   estimated) closes with a visible shift, as Flutter's does.
//!
//! [`SliverListWidget::scroll_corrections`] counts the corrections reported.
//!
//! # Keep-alive
//!
//! Each row is built inside a keep-alive scope ([`keep_alive`](crate::keep_alive)'s
//! module docs): a row asking to be kept that leaves the window while its key
//! is still in the data is parked instead of torn down — not laid out,
//! painted, routed to or visited — and re-adopted when its key re-enters the
//! window. Parked rows are checked against the data every rebuild (released
//! when their key is gone) and torn down with the list.
//!
//! # Near edges and semantics
//!
//! [`SliverListView::on_near_start`] / [`SliverListView::on_near_end`] are the
//! fixed-extent list's edge-triggered pair, observed at layout. An item-count
//! change rearms both — except that a change the keyed anchoring absorbed by
//! shifting the position down (rows loaded above the anchor, typically in
//! answer to `on_near_start` itself) leaves the start edge as it was, so the
//! answer cannot immediately refire it. Semantics is a [`Role::List`] node
//! carrying the item count as its set size, over the materialised rows.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, ScrollAxis, ScrollDirection, SemanticsCtx, SliverConstraints,
    SliverGeometry, SliverWidget, View, Widget, any,
};
use kurbo::Size;

use super::adapter::{axis_point, finite_extent, non_negative};
use super::keep_alive::{RowScope, row_wants_keep_alive};
use super::reconcile::{self, ItemViews, KeptRow, LiveRows, Measured, Retention};
use super::{NearEdge, cache_offset, paint_offset};
use crate::ChildKey;

/// The debug-only tripwire message for two materialised rows claiming one key.
const DUPLICATE_KEY_MSG: &str = "sliver_list produced a duplicate key inside one window: row \
     identity is ambiguous (release: the first slot claiming a key keeps the live row, later \
     duplicates build fresh)";

/// The extent a row is assumed to have before it is first laid out, unless
/// [`SliverListView::estimated_item_extent`] sets one: the 48 px minimum
/// touch-target height, the smallest a tappable row is laid out at.
const DEFAULT_ESTIMATED_ITEM_EXTENT: f64 = 48.0;

/// A leading-edge mismatch at or below this many px is float noise from
/// summing extents both ways from the anchor: fixed silently, never reported.
const DRIFT_TOLERANCE: f64 = 1e-6;

/// A view-held item builder, erased per row (inside its keep-alive scope).
type ItemBuilder<State> = Rc<dyn Fn(usize) -> AnyView<State>>;

/// A view-held stable-key function (`item index -> row identity`).
type KeyOf = Rc<dyn Fn(usize) -> ChildKey>;

/// A view-held near-edge callback (erased on build).
type OnNearEdge<State> = Rc<dyn Fn(&mut State)>;

/// A lazy, keyed list of rows that size themselves. See the
/// [module docs](self).
pub struct SliverListView<State: 'static> {
    item_count: usize,
    estimate: f64,
    builder: ItemBuilder<State>,
    key_of: KeyOf,
    on_near_start: Option<OnNearEdge<State>>,
    near_start_threshold: f64,
    on_near_end: Option<OnNearEdge<State>>,
    near_end_threshold: f64,
}

/// A lazy list of `item_count` rows, the row at `index` being `builder(index)`
/// keyed by `key_of(index)`, each as long along the scroll axis as it lays
/// itself out. See the [module docs](self).
///
/// ```
/// use frust_widgets::{ChildKey, CustomScrollView, custom_scroll_view, sliver_list, text};
/// let ids: Vec<u64> = (0..5_000).collect();
/// let keys = ids.clone();
/// let view: CustomScrollView<()> = custom_scroll_view(
///     sliver_list(
///         ids.len(),
///         move |i| ChildKey::new(keys[i]),
///         move |i| text(format!("message {}", ids[i])),
///     )
///     .estimated_item_extent(64.0)
///     .on_near_start(|_: &mut ()| {}, 400.0),
/// );
/// # let _ = view;
/// ```
pub fn sliver_list<State: 'static, V: View<State>>(
    item_count: usize,
    key_of: impl Fn(usize) -> ChildKey + 'static,
    builder: impl Fn(usize) -> V + 'static,
) -> SliverListView<State> {
    SliverListView {
        item_count,
        estimate: DEFAULT_ESTIMATED_ITEM_EXTENT,
        builder: Rc::new(move |index| any(RowScope::new(builder(index)))),
        key_of: Rc::new(key_of),
        on_near_start: None,
        near_start_threshold: 0.0,
        on_near_end: None,
        near_end_threshold: 0.0,
    }
}

impl<State: 'static> SliverListView<State> {
    /// Assume a row never laid out is `estimate_px` long along the scroll
    /// axis (48 px unless set) — what the scroll extent counts for every
    /// unmeasured row and what keyed anchoring shifts by per prepended row.
    ///
    /// # Panics
    ///
    /// If `estimate_px` is not positive and finite.
    pub fn estimated_item_extent(mut self, estimate_px: f64) -> Self {
        assert!(
            estimate_px > 0.0 && estimate_px.is_finite(),
            "sliver_list estimated_item_extent must be positive and finite"
        );
        self.estimate = estimate_px;
        self
    }

    /// Fire `callback` when the list's start comes within `threshold_px` of
    /// the viewport's leading edge — the "load older" edge. See the
    /// [module docs](self)' *Near edges and semantics*.
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
    /// [`SliverListView::on_near_start`].
    pub fn on_near_end<F: Fn(&mut State) + 'static>(
        mut self,
        callback: F,
        threshold_px: f64,
    ) -> Self {
        self.on_near_end = Some(Rc::new(callback));
        self.near_end_threshold = threshold_px;
        self
    }

    /// Reinstall the near-edge callbacks (closures are not comparable, so
    /// every build and rebuild does).
    fn install_edges(&self, widget: &mut SliverListWidget) {
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

/// A content position rows are placed from: item `index` starts at content
/// offset `y`, in the list's own scroll terms. `key` is the row's identity
/// when the position is a materialised row's, `None` when it was only walked
/// to over estimates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Anchor {
    key: Option<ChildKey>,
    index: usize,
    y: f64,
}

impl Anchor {
    const START: Self = Self {
        key: None,
        index: 0,
        y: 0.0,
    };
}

/// The rows one rebuild should materialise and the position they are placed
/// from.
struct Plan {
    range: Range<usize>,
    /// The row the leading edge falls in, and its content offset.
    reference: (usize, f64),
}

/// The retained widget for a [`SliverListView`]: the materialised rows and
/// their identities, the parked keep-alive rows, the measured extents, the
/// anchor, and the near-edge state.
pub struct SliverListWidget {
    /// The materialised rows, parallel to [`Self::indices`].
    children: Vec<ChildPod>,
    /// The item index each row renders — a contiguous ascending run.
    indices: Vec<usize>,
    /// Each row's key, parallel to [`Self::indices`].
    slot_keys: Vec<ChildKey>,
    /// `key -> the item index that row occupied` as of the last rebuild.
    key_index: HashMap<ChildKey, usize>,
    /// The rows kept alive outside the window, by key.
    kept: HashMap<ChildKey, KeptRow>,
    /// The view's key function, retained so layout and paint can look a
    /// row's measurement up without the view in scope.
    key_of: Option<KeyOf>,
    /// Every row laid out so far, by key.
    measured: HashMap<ChildKey, Measured>,
    /// The running Σ of [`Self::measured`]'s extents.
    measured_sum: f64,
    estimate: f64,
    item_count: usize,
    /// The position rows are placed from (the module docs' *Materialised at
    /// rebuild time, positioned from an anchor*).
    anchor: Anchor,
    /// Whether the last layout found the list's start at or above the
    /// viewport's leading edge — where keyed anchoring applies.
    anchoring: bool,
    /// A keyed-anchoring shift rebuild owes the scroll position, reported by
    /// the next layout.
    pending_correction: f64,
    /// How many scroll-offset corrections layout has reported.
    corrections: u64,
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

impl SliverListWidget {
    fn new(item_count: usize, estimate: f64, key_of: KeyOf) -> Self {
        Self {
            children: Vec::new(),
            indices: Vec::new(),
            slot_keys: Vec::new(),
            key_index: HashMap::new(),
            kept: HashMap::new(),
            key_of: Some(key_of),
            measured: HashMap::new(),
            measured_sum: 0.0,
            estimate,
            item_count,
            anchor: Anchor::START,
            anchoring: false,
            pending_correction: 0.0,
            corrections: 0,
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

    /// How many rows have a cached measured extent.
    pub fn measured_count(&self) -> usize {
        self.measured.len()
    }

    /// How many rows are parked, kept alive outside the window.
    pub fn kept_alive_count(&self) -> usize {
        self.kept.len()
    }

    /// How many scroll-offset corrections this list has reported to its
    /// viewport (the module docs' *Corrections through the viewport*).
    pub fn scroll_corrections(&self) -> u64 {
        self.corrections
    }

    /// The extent item `index` contributes: its cached measurement, else the
    /// estimate. One `key_of` call.
    fn extent_at(&self, index: usize) -> f64 {
        match self.key_of.as_ref() {
            Some(key_of) if index < self.item_count => self
                .measured
                .get(&key_of(index))
                .map_or(self.estimate, |measured| measured.extent),
            _ => self.estimate,
        }
    }

    /// The scroll extent the measurements alone imply.
    fn estimated_total(&self) -> f64 {
        reconcile::estimated_total(
            self.measured_sum,
            self.measured.len(),
            self.item_count,
            self.estimate,
        )
    }

    /// The rows the last layout's cache region asks for, its leading edge
    /// moved by `shift` (a correction not yet reported) — empty before the
    /// first layout.
    fn plan(&self, shift: f64) -> Plan {
        let count = self.item_count;
        let empty = |reference| Plan {
            range: 0..0,
            reference,
        };
        let Some(sc) = self.constraints.as_ref() else {
            return empty((self.anchor.index, self.anchor.y));
        };
        if count == 0 {
            return empty((0, 0.0));
        }
        let lead = sc.scroll_offset + shift;
        let (w, y_w) = reconcile::walk_to_offset(
            count,
            self.anchor.index,
            self.anchor.y,
            lead,
            self.estimate,
            |index| self.extent_at(index),
        );
        let start_of_cache = lead + sc.cache_origin;
        let end_of_cache = start_of_cache + sc.remaining_cache_extent;
        let past_the_end = w + 1 == count && y_w + self.extent_at(w) <= start_of_cache;
        if sc.remaining_cache_extent <= 0.0 || past_the_end {
            return empty((w, y_w));
        }
        let (mut start, mut y_start) = (w, y_w);
        while start > 0 && y_start > start_of_cache {
            start -= 1;
            y_start -= self.extent_at(start);
        }
        let (mut end, mut y_end) = (w, y_w);
        loop {
            y_end += self.extent_at(end);
            end += 1;
            if end >= count || y_end >= end_of_cache {
                break;
            }
        }
        Plan {
            range: start..end,
            reference: (w, y_w),
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
    /// the viewport since the last observation. Owes the flush that delivers
    /// a fire.
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

    /// The content offset item `index` starts at by the extent model alone:
    /// Σ measured-or-estimated extents of the items before it, walking at most
    /// [`reconcile::MAX_PREFIX_STEP`] items and counting any further ones at
    /// the estimate.
    fn room_before(&self, index: usize) -> f64 {
        let walked = index.min(reconcile::MAX_PREFIX_STEP);
        let estimated = (index - walked) as f64 * self.estimate;
        estimated
            + (index - walked..index)
                .map(|i| self.extent_at(i))
                .sum::<f64>()
    }

    /// Each row's content offset, placed from the anchor both ways by the
    /// rows' laid-out `extents`.
    fn place(&self, extents: &[f64]) -> Vec<f64> {
        let n = extents.len();
        let mut ys = vec![0.0; n];
        let Some(&first) = self.indices.first() else {
            return ys;
        };
        let from = match self
            .anchor
            .index
            .checked_sub(first)
            .filter(|slot| *slot < n)
        {
            Some(slot) => {
                ys[slot] = self.anchor.y;
                slot
            }
            None => {
                // An anchor outside the window (never left by rebuild): the
                // first row goes where the estimate puts it relative to it.
                let rows = first as f64 - self.anchor.index as f64;
                ys[0] = self.anchor.y + rows * self.estimate;
                0
            }
        };
        for slot in from + 1..n {
            ys[slot] = ys[slot - 1] + extents[slot - 1];
        }
        for slot in (0..from).rev() {
            ys[slot] = ys[slot + 1] - extents[slot];
        }
        ys
    }
}

/// Whether a sliver laid out under `sc` starts at or above the viewport's
/// leading edge — scrolled into, or flush with it (nothing before it painted)
/// — so the rows on screen are positioned from inside it.
fn starts_at_or_above_leading_edge(sc: &SliverConstraints) -> bool {
    sc.scroll_offset > 0.0 || sc.remaining_paint_extent >= sc.viewport_main_axis_extent
}

impl<State: 'static> View<State> for SliverListView<State> {
    type Element = SliverListWidget;

    /// No layout has run yet, so there is no cache region to window against:
    /// nothing is materialised until the first rebuild after it.
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SliverListWidget {
        let mut widget =
            SliverListWidget::new(self.item_count, self.estimate, Rc::clone(&self.key_of));
        self.install_edges(&mut widget);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SliverListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.install_edges(element);
        element.key_of = Some(Rc::clone(&self.key_of));
        let key_of: &dyn Fn(usize) -> ChildKey = &*self.key_of;
        let mut flags = ChangeFlags::NONE;
        let old_count = element.item_count;
        let count = self.item_count;
        let delta = count as isize - old_count as isize;
        if old_count != count {
            element.item_count = count;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.estimate != self.estimate {
            element.estimate = self.estimate;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Keyed anchoring: follow the anchor row to its index in this frame's
        // data before the window is planned around it.
        let anchor_before = element.anchor;
        let mut anchor_missed = false;
        let mut shifted_down = false;
        if let Some(key) = element.anchor.key {
            match reconcile::locate_key(key, element.anchor.index, delta, count, key_of) {
                Some(index) if index != element.anchor.index => {
                    let rows = index as f64 - element.anchor.index as f64;
                    let shift = rows * element.estimate;
                    element.anchor.index = index;
                    element.anchor.y += shift;
                    if element.anchoring {
                        element.pending_correction += shift;
                        shifted_down = shift > 0.0;
                    }
                }
                Some(_) => {}
                None => {
                    anchor_missed = true;
                    element.anchor.key = None;
                    element.anchor.index = element.anchor.index.min(count.saturating_sub(1));
                }
            }
        }

        if delta != 0 {
            // Content length changed: both edges may fire again — except the
            // start edge when the change was absorbed above the anchor.
            if !shifted_down {
                element.near_start.armed = true;
            }
            element.near_end.armed = true;
        }

        // A parked row whose key left the data is released.
        if !element.kept.is_empty() {
            let mut gone = Vec::new();
            for (key, row) in element.kept.iter_mut() {
                match reconcile::locate_key(*key, row.index, delta, count, key_of) {
                    Some(index) => row.index = index,
                    None => gone.push(*key),
                }
            }
            for key in gone {
                if let Some(row) = element.kept.remove(&key) {
                    row.teardown::<State>(ctx);
                }
                reconcile::forget_measurement(
                    &mut element.measured,
                    &mut element.measured_sum,
                    &key,
                );
            }
        }

        if count < old_count {
            reconcile::evict_measurements_past(
                &mut element.measured,
                &mut element.measured_sum,
                count,
            );
        }

        let plan = element.plan(element.pending_correction);
        let locate = |key: ChildKey, prev_index: usize| {
            reconcile::locate_key(key, prev_index, delta, count, key_of)
        };
        let outcome = reconcile::reconcile_keyed_retaining(
            ItemViews {
                prev: &*prev.builder,
                next: &*self.builder,
            },
            LiveRows {
                children: &mut element.children,
                indices: &mut element.indices,
            },
            &mut element.key_index,
            key_of,
            plan.range.clone(),
            DUPLICATE_KEY_MSG,
            Some(Retention {
                kept: &mut element.kept,
                wants: &row_wants_keep_alive,
                locate: &locate,
            }),
            ctx,
        );
        flags |= outcome.flags;

        // Measured-cache hygiene, `ListView`'s rules: a frame where neither
        // the anchor nor any previous row survived is a full replace;
        // otherwise a departing row whose key is gone from the data drops its
        // measurement.
        if anchor_missed && !outcome.any_survivor {
            element.measured.clear();
            element.measured_sum = 0.0;
        } else {
            for (key, prev_index) in &outcome.departed {
                if reconcile::probe_key(*key, *prev_index, delta, count, key_of).is_none() {
                    reconcile::forget_measurement(
                        &mut element.measured,
                        &mut element.measured_sum,
                        key,
                    );
                }
            }
        }
        element.slot_keys = outcome.slot_keys;

        // Rows are placed from the anchor while it is still materialised — it
        // was laid out where it sits, so nothing positioned from it moves —
        // else from the row the walk found the leading edge in.
        let (index, y) = if plan.range.contains(&element.anchor.index) {
            (element.anchor.index, element.anchor.y)
        } else {
            plan.reference
        };
        let key = index
            .checked_sub(plan.range.start)
            .and_then(|slot| element.slot_keys.get(slot).copied())
            .filter(|_| plan.range.contains(&index));
        element.anchor = Anchor { key, index, y };
        if element.anchor != anchor_before || element.pending_correction != 0.0 {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SliverListWidget, ctx: &mut BuildCtx<'_>) {
        for (index, pod) in element.indices.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(&(self.builder)(*index), pod, ctx);
        }
        for (_, row) in element.kept.drain() {
            row.teardown::<State>(ctx);
        }
    }
}

impl Widget for SliverListWidget {
    /// Outside a viewport the list lays out as if it were the only sliver of a
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
        // No viewport applies a correction here: the position stays put, and
        // a leading-edge shift (already applied to the rows) just lays out
        // again.
        self.pending_correction = 0.0;
        let corrections = self.corrections;
        let mut geometry = self.layout_sliver(ctx, &sc);
        for _ in 0..2 {
            if geometry.scroll_offset_correction.is_none() {
                break;
            }
            geometry = self.layout_sliver(ctx, &sc);
        }
        self.corrections = corrections;
        bc.constrain(axis.size(geometry.paint_extent, cross))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.clip {
            scene.push_clip(ctx.origin(), ctx.size());
        }
        let axis = self.constraints.map_or(ScrollAxis::Vertical, |sc| sc.axis);
        let paint_extent = self.paint_extent;
        for pod in &mut self.children {
            // A row only cached is laid out but never painted.
            let origin = pod.origin();
            let start = axis.main_extent(Size::new(origin.x, origin.y));
            let extent = axis.main_extent(pod.size());
            if start + extent > 0.0 && start < paint_extent {
                pod.paint_child(ctx, scene);
            }
        }
        if self.clip {
            scene.pop_clip();
        }
        // The next rebuild re-windows to what this frame's layout asked for;
        // make sure it runs. A fire observed at layout owes the same frame.
        let wanted = self.plan(0.0).range;
        if !self.covers(&wanted) || self.near_start.pending || self.near_end.pending {
            ctx.request_frame();
        }
    }

    /// Delivers a near-edge fire observed at layout, then routes the event to
    /// the materialised rows (parked rows receive nothing).
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.near_start.deliver(ctx);
        self.near_end.deliver(ctx);
        crate::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let count = self.item_count;
        ctx.push_container(
            Role::List,
            move |node| node.set_size_of_set(count),
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        Some(self)
    }

    crate::authoring::visit_children!(children);
}

impl SliverWidget for SliverListWidget {
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        // A keyed-anchoring shift rebuild recorded: the viewport moves the
        // position by it and lays out again.
        let pending = std::mem::take(&mut self.pending_correction);
        if pending.is_finite() && pending.abs() > DRIFT_TOLERANCE {
            self.corrections += 1;
            return SliverGeometry {
                scroll_offset_correction: Some(pending),
                ..SliverGeometry::ZERO
            };
        }

        let axis = sc.axis;
        let row = sc.as_box_constraints();
        let mut extents = Vec::with_capacity(self.children.len());
        for (slot, pod) in self.children.iter_mut().enumerate() {
            let extent = finite_extent(axis.main_extent(pod.layout_child(ctx, &row)));
            extents.push(extent);
            if let (Some(&key), Some(&index)) = (self.slot_keys.get(slot), self.indices.get(slot)) {
                reconcile::record_measurement(
                    &mut self.measured,
                    &mut self.measured_sum,
                    key,
                    index,
                    extent,
                );
            }
        }
        let mut ys = self.place(&extents);

        // The leading edge (the module docs' *Corrections through the
        // viewport*): row 0 starts at 0, and a window starting at a later row
        // leaves room before it.
        if let (Some(&first), Some(&y_first)) = (self.indices.first(), ys.first()) {
            let wanted = if first == 0 {
                0.0
            } else if y_first < DRIFT_TOLERANCE {
                // Rows above the window with no room left before the list's
                // start get their room back, as the measurements and the
                // estimate size it.
                self.room_before(first)
            } else {
                y_first
            };
            let drift = wanted - y_first;
            if drift != 0.0 {
                for y in &mut ys {
                    *y += drift;
                }
                self.anchor.y += drift;
                if drift.abs() > DRIFT_TOLERANCE
                    && starts_at_or_above_leading_edge(sc)
                    && sc.scroll_offset + drift >= 0.0
                {
                    self.corrections += 1;
                    return SliverGeometry {
                        scroll_offset_correction: Some(drift),
                        ..SliverGeometry::ZERO
                    };
                }
            }
        }

        let lead = sc.scroll_offset;
        for (pod, y) in self.children.iter_mut().zip(&ys) {
            pod.set_origin(axis_point(axis, y - lead, 0.0));
        }

        let count = self.item_count;
        let scroll_extent = match (self.indices.last(), ys.last(), extents.last()) {
            (Some(&last), Some(&y_last), Some(&extent_last)) => {
                let end = y_last + extent_last;
                if last + 1 >= count {
                    end
                } else {
                    self.estimated_total().max(end)
                }
            }
            _ => self.estimated_total(),
        };
        let scroll_extent = finite_extent(scroll_extent);
        let paint_extent = paint_offset(sc, 0.0, scroll_extent);
        let geometry = SliverGeometry {
            cache_extent: cache_offset(sc, 0.0, scroll_extent).max(paint_extent),
            has_visual_overflow: scroll_extent > sc.scroll_offset + sc.remaining_paint_extent
                || sc.scroll_offset > 0.0,
            ..SliverGeometry::new(scroll_extent, paint_extent, scroll_extent)
        };

        // The row the leading edge sits in anchors the next rebuild.
        if !ys.is_empty() {
            let slot = (0..ys.len())
                .find(|&slot| ys[slot] + extents[slot] > lead)
                .unwrap_or(ys.len() - 1);
            self.anchor = Anchor {
                key: self.slot_keys.get(slot).copied(),
                index: self.indices[slot],
                y: ys[slot],
            };
        }
        self.anchoring = starts_at_or_above_leading_edge(sc);
        self.constraints = Some(*sc);
        self.paint_extent = paint_extent;
        self.clip = geometry.has_visual_overflow;
        self.observe_edges(sc, scroll_extent);
        geometry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(count: usize) -> SliverListWidget {
        SliverListWidget::new(count, 50.0, Rc::new(|i| ChildKey::new(i as u64)))
    }

    fn constraints(scroll_offset: f64) -> SliverConstraints {
        SliverConstraints {
            axis: ScrollAxis::Vertical,
            scroll_offset,
            preceding_scroll_extent: 0.0,
            overlap: 0.0,
            remaining_paint_extent: 500.0,
            cross_axis_extent: 400.0,
            viewport_main_axis_extent: 500.0,
            cache_origin: -scroll_offset.min(100.0),
            remaining_cache_extent: 600.0 + scroll_offset.min(100.0),
            user_scroll_direction: ScrollDirection::Idle,
        }
    }

    #[test]
    fn the_plan_walks_from_the_anchor_over_measured_and_estimated_rows() {
        let mut w = widget(1000);
        assert!(w.plan(0.0).range.is_empty(), "no layout, no window");
        w.constraints = Some(constraints(0.0));
        // [0, 600) over 50 px estimates: rows 0..12.
        assert_eq!(w.plan(0.0).range, 0..12);
        // Rows 0..4 measured at 100: [0, 600) now holds rows 0..8.
        for i in 0..4u64 {
            reconcile::record_measurement(
                &mut w.measured,
                &mut w.measured_sum,
                ChildKey::new(i),
                i as usize,
                100.0,
            );
        }
        assert_eq!(w.plan(0.0).range, 0..8);
        assert_eq!(w.estimated_total(), 400.0 + 996.0 * 50.0);
        // Scrolled to 1000 from an anchor at row 4 (offset 400): the leading
        // edge falls in row 16 (400 + 12 × 50), the cache region is [900,
        // 1600).
        w.constraints = Some(constraints(1000.0));
        w.anchor = Anchor {
            key: None,
            index: 4,
            y: 400.0,
        };
        let plan = w.plan(0.0);
        assert_eq!(plan.reference, (16, 1000.0));
        assert_eq!(plan.range, 14..28);
        // A shift owed to the position moves the region with it.
        assert_eq!(w.plan(150.0).reference, (19, 1150.0));
    }

    #[test]
    fn rows_are_placed_both_ways_from_the_anchor() {
        let mut w = widget(100);
        w.indices = vec![10, 11, 12, 13];
        w.anchor = Anchor {
            key: None,
            index: 12,
            y: 600.0,
        };
        assert_eq!(
            w.place(&[30.0, 40.0, 50.0, 60.0]),
            vec![530.0, 560.0, 600.0, 650.0]
        );
    }
}
