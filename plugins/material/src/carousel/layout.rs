//! The carousel's two item-extent solvers: the M3E **weighted** layout (hero
//! and contained) and the **fixed-extent** layout (uncontained). Both are pure
//! main-axis math over a viewport extent and a scroll offset — nothing here
//! touches a `PaintScene`, a `Theme`, a `ChildPod`, or a callback.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/carousel/components/m3e_sliver_weighted_carousel_layout.dart`
//! and `m3e_sliver_fixed_extent_carousel.dart` (retrieved 2026-08-19), which are
//! themselves vendored from `m3_carousel` (MIT, © 2024 Paa) and derived from
//! Flutter's `CarouselView` — **BSD-3-Clause, © 2014 The Flutter Authors**; see
//! `plugins/material/NOTICE`'s BSD section.
//!
//! # Main-axis scalars, never `(x, y)`
//!
//! Every value here is a scalar along the scroll axis: `0.0` at the viewport's
//! leading edge, `viewport` at its trailing edge. [`super`]'s paint maps that
//! onto `(x, y)` once, which is the whole of what the vertical axis costs.
//!
//! # Two coordinate spaces
//!
//! * **Scroll space** — [`WeightedLayout::layout_offset`]/
//!   [`FixedLayout::layout_offset`] return the upstream `indexToLayoutOffset`
//!   value, an offset into the infinite scrolled content.
//! * **Viewport space** — [`Slot::offset`] is that value minus the scroll
//!   offset, i.e. where the item paints inside the viewport. This is the only
//!   space [`super`] ever sees.
//!
//! # Clamp discipline
//!
//! `f64::clamp` panics if `min > max` or a bound is `NaN` — a hard abort under
//! `panic = "abort"` — so this module never calls it against a computed pair
//! (the rule `slider/core.rs`'s *Clamp discipline* section establishes for the
//! crate). Every bound here is applied as an explicit `.max(lo)` then `.min(hi)`
//! with the ordering visible at the call site, and both constructors funnel
//! their `f64` inputs through [`finite`], so a `NaN`/infinite viewport or scroll
//! offset degrades to `0.0` instead of poisoning every extent downstream.

/// Flutter's `precisionErrorTolerance`: the slack within which a division
/// result counts as landing exactly on an item boundary. Same value and same
/// role as upstream (`_firstVisibleItemIndex`'s round-vs-floor branch).
pub(crate) const PRECISION_TOLERANCE: f64 = 1e-10;

/// Item weights for a hero carousel with the focal item at the leading edge
/// (`M3ECarousel._applyLayoutWeights`'s `[8, 2]`).
pub(crate) const HERO_START_WEIGHTS: &[u16] = &[8, 2];
/// Item weights for a centred hero carousel (`[2, 6, 2]`).
pub(crate) const HERO_CENTER_WEIGHTS: &[u16] = &[2, 6, 2];
/// Item weights for a hero carousel with the focal item at the trailing edge
/// (`[2, 8]`).
pub(crate) const HERO_END_WEIGHTS: &[u16] = &[2, 8];
/// Item weights for a contained carousel (`[5, 4, 1]`).
pub(crate) const CONTAINED_WEIGHTS: &[u16] = &[5, 4, 1];
/// Item weights for an extended contained carousel (`[4, 3, 2, 1]`).
pub(crate) const CONTAINED_EXTENDED_WEIGHTS: &[u16] = &[4, 3, 2, 1];

/// `value` if it is finite, `0.0` otherwise — the module's one NaN/infinity
/// gate (see the [module docs](self)' clamp discipline).
fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

/// One item's resolved main-axis geometry, in **viewport space**.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Slot {
    /// The item's index in the carousel's child list.
    pub(crate) index: usize,
    /// Main-axis offset of the item's leading edge from the viewport's leading
    /// edge, in logical px.
    pub(crate) offset: f64,
    /// The item's current main-axis extent, in logical px.
    pub(crate) extent: f64,
}

/// The M3E weighted layout: items occupy successive slots sized by a weight
/// list, morphing between weights as the leading item scrolls off.
///
/// Faithful port of upstream's `_M3EWeightedCarouselLayoutMixin` with the
/// Flutter sliver plumbing (child recycling, geometry publication, scroll
/// corrections) dropped — this crate's carousel lays every child out once at
/// the largest slot and clips, so only the extent/offset math survives.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WeightedLayout<'w> {
    weights: &'w [u16],
    viewport: f64,
    shrink_extent: f64,
    scroll_offset: f64,
    consume_max_weight: bool,
}

impl<'w> WeightedLayout<'w> {
    /// A solver over `weights` for a `viewport`-wide carousel scrolled to
    /// `scroll_offset`.
    ///
    /// `consume_max_weight` is upstream's `consumeMaxWeight`: when set, the
    /// leading index is shifted back by the count of leading weights smaller
    /// than the maximum, so item 0 can itself expand into the max-weight slot
    /// (leaving a phantom gap before it).
    pub(crate) fn new(
        weights: &'w [u16],
        viewport: f64,
        shrink_extent: f64,
        scroll_offset: f64,
        consume_max_weight: bool,
    ) -> Self {
        Self {
            weights,
            viewport: finite(viewport).max(0.0),
            shrink_extent: finite(shrink_extent).max(0.0),
            scroll_offset: finite(scroll_offset).max(0.0),
            consume_max_weight,
        }
    }

    /// Whether the solver has anything to solve: a positive viewport and at
    /// least one positive weight. Every accessor below degrades to `0.0` when
    /// this is false, mirroring upstream's `viewportMainAxisExtent == 0` guards.
    fn is_degenerate(&self) -> bool {
        self.viewport <= 0.0 || self.weights.is_empty() || self.total_weight() <= 0.0
    }

    fn total_weight(&self) -> f64 {
        self.weights.iter().map(|w| f64::from(*w)).sum()
    }

    fn max_weight(&self) -> f64 {
        self.weights
            .iter()
            .map(|w| f64::from(*w))
            .fold(0.0, f64::max)
    }

    fn min_weight(&self) -> f64 {
        self.weights
            .iter()
            .map(|w| f64::from(*w))
            .fold(f64::INFINITY, f64::min)
    }

    /// Viewport extent per unit of weight (`extentUnit`).
    pub(crate) fn extent_unit(&self) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        self.viewport / self.total_weight()
    }

    /// The resting extent of the leading slot (`firstChildExtent`) — also the
    /// carousel's scroll stride, since one item step moves the content by
    /// exactly one leading slot.
    pub(crate) fn first_child_extent(&self) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        f64::from(self.weights[0]) * self.extent_unit()
    }

    /// The largest resting slot extent (`maxChildExtent`) — the size every child
    /// is laid out at, since a child's own content never resizes with the slot.
    pub(crate) fn max_child_extent(&self) -> f64 {
        self.max_weight() * self.extent_unit()
    }

    /// The smallest resting slot extent (`minChildExtent`).
    pub(crate) fn min_child_extent(&self) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        self.min_weight() * self.extent_unit()
    }

    /// The shrink extent actually applied: never larger than
    /// [`min_child_extent`](Self::min_child_extent), so the leading item's
    /// collapse stays smooth (`effectiveShrinkExtent`).
    fn effective_shrink_extent(&self) -> f64 {
        // Ordered explicitly rather than `clamp`: `min_child_extent()` is
        // computed, and both bounds must be provably ordered (module docs).
        self.shrink_extent.max(0.0).min(self.min_child_extent())
    }

    /// How many leading weights are smaller than the maximum — the shift
    /// `consumeMaxWeight` applies to the leading index.
    fn smaller_leading_weights(&self) -> i64 {
        let max = self.max_weight();
        self.weights
            .iter()
            .take_while(|w| f64::from(**w) < max)
            .count() as i64
    }

    /// The index of the slot the scroll offset currently sits in, before
    /// `consumeMaxWeight` shifts it — upstream's shared `actual`/`round` dance,
    /// which pins an offset that is within [`PRECISION_TOLERANCE`] of a boundary
    /// to that boundary instead of flooring just below it.
    fn raw_leading_index(&self) -> i64 {
        let first = self.first_child_extent();
        if first <= 0.0 {
            return 0;
        }
        let actual = self.scroll_offset / first;
        if !actual.is_finite() {
            return 0;
        }
        let round = actual.round();
        if (actual - round).abs() < PRECISION_TOLERANCE {
            round as i64
        } else {
            actual.floor() as i64
        }
    }

    /// The index of the first visible item (`_firstVisibleItemIndex`).
    ///
    /// Negative under `consumeMaxWeight`: upstream deliberately leaves room for
    /// phantom items before item 0 so the leading items can expand to the max
    /// weight.
    pub(crate) fn first_visible_index(&self) -> i64 {
        if self.is_degenerate() {
            return 0;
        }
        if self.consume_max_weight {
            self.raw_leading_index() - self.smaller_leading_weights()
        } else {
            self.raw_leading_index()
        }
    }

    /// How far the leading item has scrolled off the viewport's leading edge
    /// (`_firstVisibleItemOffscreenExtent`) — the progress every following
    /// item's morph is driven by.
    fn offscreen_extent(&self) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        self.scroll_offset - self.raw_leading_index() as f64 * self.first_child_extent()
    }

    /// The on-screen part of the leading item (`_distanceToLeadingEdge`).
    fn distance_to_leading_edge(&self) -> f64 {
        self.first_child_extent() - self.offscreen_extent()
    }

    /// The extent of the item in weight slot `slot` (`_extentWithinWeights`):
    /// its own weight's resting extent, plus the share of the previous (larger
    /// or smaller) weight it has grown into as the leading item scrolls off.
    fn extent_within_weights(&self, slot: usize) -> f64 {
        let first = self.first_child_extent();
        if first <= 0.0 || slot == 0 || slot >= self.weights.len() {
            return 0.0;
        }
        let curr = f64::from(self.weights[slot]);
        let prev = f64::from(self.weights[slot - 1]);
        let progress = self.offscreen_extent() / first;
        let max_weight = self.max_weight();
        let final_increase = if max_weight > 0.0 {
            (prev - curr) / max_weight
        } else {
            0.0
        };
        self.extent_unit() * curr + final_increase * progress * self.max_child_extent()
    }

    /// The total extent occupied by the items from the leading one up to (but
    /// not including) `index` — the forward walk `_extentBeyondWeights` and
    /// `indexToLayoutOffset` both accumulate.
    ///
    /// Upstream expresses this as mutual recursion between `_buildItemExtent`
    /// and `_extentBeyondWeights`; walking forward once is the same arithmetic
    /// without the exponential re-entry.
    fn total_before(&self, index: i64) -> f64 {
        let first = self.first_visible_index();
        let mut total = self.distance_to_leading_edge();
        let shrink = self.effective_shrink_extent();
        let mut i = first + 1;
        while i < index {
            let slot = (i - first) as usize;
            total += if slot < self.weights.len() {
                self.extent_within_weights(slot)
            } else {
                (self.viewport - total).max(shrink)
            };
            i += 1;
        }
        total
    }

    /// The main-axis extent of item `index` (`_buildItemExtent`).
    pub(crate) fn item_extent(&self, index: i64) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        let first = self.first_visible_index();
        let shrink = self.effective_shrink_extent();
        if index == first {
            return self.distance_to_leading_edge().max(shrink);
        }
        if index > first {
            let slot = (index - first) as usize;
            if slot < self.weights.len() {
                return self.extent_within_weights(slot);
            }
            return (self.viewport - self.total_before(index)).max(shrink);
        }
        // Already scrolled past: upstream pins these to the smallest slot.
        self.min_child_extent().max(shrink)
    }

    /// Item `index`'s leading edge in **scroll space** (`indexToLayoutOffset`).
    pub(crate) fn layout_offset(&self, index: i64) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        let first = self.first_visible_index();
        if index == first {
            let distance = self.distance_to_leading_edge();
            let shrink = self.effective_shrink_extent();
            if distance <= shrink {
                // Collapsed to the shrink floor: the item stops resizing and
                // starts sliding off instead.
                return self.scroll_offset - shrink + distance;
            }
            return self.scroll_offset;
        }
        self.scroll_offset + self.total_before(index)
    }

    /// Every item visible in the viewport, in **viewport space**, in index
    /// order. Walks forward from the leading item until the accumulated extent
    /// fills the viewport (upstream's `_maxIndexForFiniteChildren`).
    pub(crate) fn slots(&self, item_count: usize) -> Vec<Slot> {
        let mut out = Vec::new();
        if self.is_degenerate() || item_count == 0 {
            return out;
        }
        let first = self.first_visible_index();
        let count = item_count as i64;
        if first >= count {
            return out;
        }
        if first >= 0 {
            out.push(Slot {
                index: first as usize,
                offset: self.layout_offset(first) - self.scroll_offset,
                extent: self.item_extent(first),
            });
        }
        let shrink = self.effective_shrink_extent();
        let mut total = self.distance_to_leading_edge();
        let mut i = first + 1;
        while i < count && total < self.viewport {
            let slot = (i - first) as usize;
            let extent = if slot < self.weights.len() {
                self.extent_within_weights(slot)
            } else {
                (self.viewport - total).max(shrink)
            };
            if i >= 0 {
                out.push(Slot {
                    index: i as usize,
                    offset: total,
                    extent,
                });
            }
            total += extent;
            i += 1;
        }
        out
    }

    /// The furthest scroll offset with a full window of items still to show:
    /// the last `weights.len()` items exactly filling the viewport.
    ///
    /// Upstream leaves this to the sliver's own `estimateMaxScrollOffset`; the
    /// same bound is what its gesture stepper caps at
    /// (`M3ECarouselScrollHelper._containedStep`'s `childrenLength - 3`/`- 4`,
    /// and `_heroStep`'s equivalent limits).
    pub(crate) fn max_scroll_offset(&self, item_count: usize) -> f64 {
        let last_window = (item_count as f64 - self.weights.len() as f64).max(0.0);
        last_window * self.first_child_extent()
    }
}

/// The uncontained layout: every item shares one fixed main-axis extent, with
/// the leading and trailing items shrinking as they cross the viewport edges.
///
/// Faithful port of upstream's `_RenderSliverFixedExtentCarousel`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FixedLayout {
    item_extent: f64,
    shrink_extent: f64,
    viewport: f64,
    scroll_offset: f64,
}

impl FixedLayout {
    /// A solver for `item_extent`-wide items in a `viewport`-wide carousel
    /// scrolled to `scroll_offset`. `item_extent` is clamped to the viewport,
    /// mirroring `_CarouselViewState.build`'s own clamp.
    pub(crate) fn new(
        item_extent: f64,
        shrink_extent: f64,
        viewport: f64,
        scroll_offset: f64,
    ) -> Self {
        let viewport = finite(viewport).max(0.0);
        Self {
            // Ordered explicitly (module docs): `0.0 <= viewport` by the line
            // above, so this pair can never be inverted.
            item_extent: finite(item_extent).max(0.0).min(viewport),
            shrink_extent: finite(shrink_extent).max(0.0),
            viewport,
            scroll_offset: finite(scroll_offset).max(0.0),
        }
    }

    /// The resting extent of every item — also the carousel's scroll stride.
    pub(crate) fn stride(&self) -> f64 {
        self.item_extent
    }

    /// The trailing item's floor: at least the viewport remainder, so the last
    /// item entering the viewport grows smoothly instead of popping
    /// (`effectiveMinExtent`).
    fn effective_min_extent(&self) -> f64 {
        if self.item_extent <= 0.0 {
            return self.shrink_extent;
        }
        (self.viewport % self.item_extent).max(self.shrink_extent)
    }

    /// The index of the first visible item.
    pub(crate) fn first_visible_index(&self) -> i64 {
        if self.item_extent <= 0.0 {
            return 0;
        }
        let actual = self.scroll_offset / self.item_extent;
        if !actual.is_finite() {
            return 0;
        }
        actual.floor() as i64
    }

    /// The index of the last item painted at `scroll_offset`
    /// (`getMaxChildIndexForScrollOffset`).
    fn max_child_index_for_scroll_offset(&self, scroll_offset: f64) -> i64 {
        if self.item_extent <= 0.0 {
            return 0;
        }
        let actual = scroll_offset / self.item_extent - 1.0;
        if !actual.is_finite() {
            return 0;
        }
        let round = actual.round();
        if ((actual - round) * self.item_extent).abs() < PRECISION_TOLERANCE {
            (round as i64).max(0)
        } else {
            (actual.ceil() as i64).max(0)
        }
    }

    /// The main-axis extent of item `index`.
    pub(crate) fn item_extent_at(&self, index: i64) -> f64 {
        if self.item_extent <= 0.0 {
            return 0.0;
        }
        let min = self.effective_min_extent();
        let offscreen = self.scroll_offset - self.first_visible_index() as f64 * self.item_extent;
        if index == self.first_visible_index() {
            return (self.item_extent - offscreen).max(min);
        }
        let trailing_offset = self.scroll_offset + self.viewport;
        if index == self.max_child_index_for_scroll_offset(trailing_offset) {
            // Ordered explicitly (module docs): `min` is capped by
            // `item_extent` below, so the pair can never invert.
            let raw = trailing_offset - self.item_extent * index as f64;
            return raw.max(min.min(self.item_extent)).min(self.item_extent);
        }
        self.item_extent
    }

    /// Item `index`'s leading edge in **scroll space**.
    pub(crate) fn layout_offset(&self, index: i64) -> f64 {
        if self.item_extent <= 0.0 {
            return 0.0;
        }
        let first = self.first_visible_index();
        if index == first {
            let min = self.effective_min_extent();
            if self.item_extent_at(index) <= min {
                // Collapsed to the floor: it slides off instead of shrinking.
                return self.item_extent * index as f64 - min + self.item_extent;
            }
            return self.scroll_offset;
        }
        self.item_extent * index as f64
    }

    /// Every item visible in the viewport, in **viewport space**, in index
    /// order.
    pub(crate) fn slots(&self, item_count: usize) -> Vec<Slot> {
        let mut out = Vec::new();
        if self.item_extent <= 0.0 || self.viewport <= 0.0 || item_count == 0 {
            return out;
        }
        let count = item_count as i64;
        let first = self.first_visible_index().max(0);
        let mut i = first;
        while i < count {
            let offset = self.layout_offset(i) - self.scroll_offset;
            if offset >= self.viewport {
                break;
            }
            out.push(Slot {
                index: i as usize,
                offset,
                extent: self.item_extent_at(i),
            });
            i += 1;
        }
        out
    }

    /// The furthest scroll offset: the content's end flush with the viewport's
    /// trailing edge, the ordinary scrollable-list bound.
    pub(crate) fn max_scroll_offset(&self, item_count: usize) -> f64 {
        (item_count as f64 * self.item_extent - self.viewport).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sum of every visible slot's extent, which must fill the viewport once
    /// enough items exist to do so.
    fn covered(slots: &[Slot]) -> f64 {
        slots.iter().map(|s| s.extent).sum()
    }

    // ---- weighted: resting extents at offset 0 ---------------------------

    #[test]
    fn contained_resting_extents_are_the_weight_shares_of_the_viewport() {
        // `[5, 4, 1]` over a 400px viewport: unit = 40px.
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 0.0, false);
        assert_eq!(l.extent_unit(), 40.0);
        assert_eq!(l.first_child_extent(), 200.0);
        assert_eq!(l.max_child_extent(), 200.0);
        assert_eq!(l.min_child_extent(), 40.0);

        let slots = l.slots(6);
        assert_eq!(slots.len(), 3, "three weighted slots fill the viewport");
        assert_eq!(
            slots[0],
            Slot {
                index: 0,
                offset: 0.0,
                extent: 200.0
            }
        );
        assert_eq!(
            slots[1],
            Slot {
                index: 1,
                offset: 200.0,
                extent: 160.0
            }
        );
        assert_eq!(
            slots[2],
            Slot {
                index: 2,
                offset: 360.0,
                extent: 40.0
            }
        );
        assert_eq!(covered(&slots), 400.0);
    }

    #[test]
    fn hero_center_resting_extents_put_the_big_slot_in_the_middle() {
        // `[2, 6, 2]` over a 500px viewport: unit = 50px.
        let l = WeightedLayout::new(HERO_CENTER_WEIGHTS, 500.0, 0.0, 0.0, false);
        let slots = l.slots(5);
        assert_eq!(
            slots
                .iter()
                .map(|s| (s.offset, s.extent))
                .collect::<Vec<_>>(),
            vec![(0.0, 100.0), (100.0, 300.0), (400.0, 100.0)]
        );
    }

    #[test]
    fn hero_start_and_end_mirror_each_other() {
        let start = WeightedLayout::new(HERO_START_WEIGHTS, 500.0, 0.0, 0.0, false);
        let end = WeightedLayout::new(HERO_END_WEIGHTS, 500.0, 0.0, 0.0, false);
        assert_eq!(
            start.slots(4).iter().map(|s| s.extent).collect::<Vec<_>>(),
            vec![400.0, 100.0]
        );
        assert_eq!(
            end.slots(4).iter().map(|s| s.extent).collect::<Vec<_>>(),
            vec![100.0, 400.0]
        );
    }

    #[test]
    fn extended_contained_shows_four_descending_slots() {
        // `[4, 3, 2, 1]` over a 500px viewport: unit = 50px.
        let l = WeightedLayout::new(CONTAINED_EXTENDED_WEIGHTS, 500.0, 0.0, 0.0, false);
        assert_eq!(
            l.slots(8).iter().map(|s| s.extent).collect::<Vec<_>>(),
            vec![200.0, 150.0, 100.0, 50.0]
        );
    }

    #[test]
    fn a_second_viewport_width_rescales_every_extent_proportionally() {
        let narrow = WeightedLayout::new(CONTAINED_WEIGHTS, 200.0, 0.0, 0.0, false);
        let wide = WeightedLayout::new(CONTAINED_WEIGHTS, 800.0, 0.0, 0.0, false);
        for (n, w) in narrow.slots(6).iter().zip(wide.slots(6).iter()) {
            assert_eq!(n.extent * 4.0, w.extent);
            assert_eq!(n.offset * 4.0, w.offset);
        }
    }

    // ---- weighted: the morph across a scroll ------------------------------

    #[test]
    fn a_half_item_scroll_morphs_every_slot_between_its_weights() {
        // Half of the 200px leading slot scrolled off.
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 100.0, false);
        let slots = l.slots(6);
        // Leading item: shrunk by exactly what scrolled off, and pinned at the
        // viewport's leading edge.
        assert_eq!(
            slots[0],
            Slot {
                index: 0,
                offset: 0.0,
                extent: 100.0
            }
        );
        // Slot 1 grew halfway from weight 4 toward weight 5 (`finalIncrease`
        // (5-4)/5 = 0.2 of the 200px max extent, at progress 0.5 → +20px).
        assert_eq!(slots[1].extent, 180.0);
        // Slot 2 grew from weight 1 toward weight 4: (4-1)/5 = 0.6 → +60px.
        assert_eq!(slots[2].extent, 100.0);
        // And the newly entering item takes whatever viewport is left.
        assert_eq!(slots[3].extent, 20.0);
        assert_eq!(covered(&slots), 400.0);
    }

    #[test]
    fn scrolling_one_whole_item_reproduces_the_resting_layout_one_index_along() {
        let rest = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 0.0, false);
        let stepped = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 200.0, false);
        let rest_slots = rest.slots(6);
        let stepped_slots = stepped.slots(6);
        assert_eq!(stepped_slots[0].index, 1, "item 1 now leads");
        for (r, s) in rest_slots.iter().zip(stepped_slots.iter()) {
            assert_eq!(r.offset, s.offset);
            assert!((r.extent - s.extent).abs() < 1e-9);
        }
    }

    #[test]
    fn slot_extents_stay_monotone_across_a_continuous_scroll() {
        // Slot 1 only ever grows toward the max weight as the leading item
        // scrolls off; slot 0 only ever shrinks.
        let mut prev_leading = f64::INFINITY;
        let mut prev_second = 0.0;
        for step in 0..=20 {
            let offset = f64::from(step) * 10.0;
            let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, offset, false);
            let slots = l.slots(6);
            if offset >= 200.0 {
                break;
            }
            assert!(slots[0].extent <= prev_leading + 1e-9);
            assert!(slots[1].extent >= prev_second - 1e-9);
            assert!(
                (covered(&slots) - 400.0).abs() < 1e-9,
                "the visible slots always tile the viewport exactly"
            );
            prev_leading = slots[0].extent;
            prev_second = slots[1].extent;
        }
    }

    #[test]
    fn the_shrink_extent_floors_the_leading_item_and_slides_it_off() {
        // 30px shrink floor, scrolled 180 of the 200px leading slot off: the
        // leading item stops shrinking at 30px and its offset goes negative.
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 30.0, 180.0, false);
        let slots = l.slots(6);
        assert_eq!(slots[0].extent, 30.0);
        assert_eq!(
            slots[0].offset, -10.0,
            "it slides off rather than shrinking"
        );
    }

    #[test]
    fn the_shrink_extent_can_never_exceed_the_smallest_slot() {
        // A 1000px shrink request against a 40px min slot resolves to 40px.
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 1000.0, 199.0, false);
        assert_eq!(l.item_extent(0), 40.0);
    }

    #[test]
    fn consume_max_weight_leaves_a_phantom_slot_before_item_zero() {
        let l = WeightedLayout::new(HERO_CENTER_WEIGHTS, 500.0, 0.0, 0.0, true);
        assert_eq!(l.first_visible_index(), -1, "one smaller leading weight");
        let slots = l.slots(5);
        assert_eq!(slots[0].index, 0);
        assert_eq!(slots[0].offset, 100.0, "the phantom leading slot's width");
        assert_eq!(slots[0].extent, 300.0, "item 0 occupies the max slot");
    }

    #[test]
    fn without_consume_max_weight_item_zero_leads_the_smallest_slot() {
        let l = WeightedLayout::new(HERO_CENTER_WEIGHTS, 500.0, 0.0, 0.0, false);
        assert_eq!(l.first_visible_index(), 0);
        assert_eq!(l.slots(5)[0].offset, 0.0);
    }

    // ---- weighted: bounds and degenerate inputs ----------------------------

    #[test]
    fn max_scroll_offset_stops_with_a_full_window_showing() {
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 0.0, false);
        // 6 items, 3 slots → the last window leads with item 3.
        assert_eq!(l.max_scroll_offset(6), 600.0);
        // Fewer items than slots → nothing to scroll.
        assert_eq!(l.max_scroll_offset(2), 0.0);
    }

    #[test]
    fn a_zero_viewport_degrades_to_zero_everywhere_instead_of_dividing_by_it() {
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 0.0, 0.0, 50.0, false);
        assert_eq!(l.extent_unit(), 0.0);
        assert_eq!(l.first_child_extent(), 0.0);
        assert_eq!(l.item_extent(0), 0.0);
        assert_eq!(l.layout_offset(0), 0.0);
        assert!(l.slots(5).is_empty());
    }

    #[test]
    fn non_finite_inputs_degrade_instead_of_poisoning_every_extent() {
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, f64::NAN, f64::NAN, f64::INFINITY, false);
        assert_eq!(l.extent_unit(), 0.0);
        assert!(l.slots(5).is_empty());

        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, f64::NAN, false);
        assert!(l.slots(5).iter().all(|s| s.extent.is_finite()));
    }

    #[test]
    fn an_empty_weight_list_is_degenerate_rather_than_a_panic() {
        let l = WeightedLayout::new(&[], 400.0, 0.0, 100.0, false);
        assert_eq!(l.first_child_extent(), 0.0);
        assert!(l.slots(5).is_empty());
        assert_eq!(l.max_scroll_offset(5), 0.0);
    }

    #[test]
    fn an_all_zero_weight_list_is_degenerate_rather_than_a_division_by_zero() {
        let l = WeightedLayout::new(&[0, 0], 400.0, 0.0, 100.0, false);
        assert_eq!(l.extent_unit(), 0.0);
        assert!(l.slots(5).is_empty());
    }

    #[test]
    fn a_scroll_offset_within_precision_tolerance_pins_to_the_boundary() {
        let exact = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 200.0, false);
        let fuzzy = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 200.0 - 1e-12, false);
        assert_eq!(exact.first_visible_index(), fuzzy.first_visible_index());
    }

    #[test]
    fn fewer_items_than_slots_produces_only_the_items_that_exist() {
        let l = WeightedLayout::new(CONTAINED_WEIGHTS, 400.0, 0.0, 0.0, false);
        let slots = l.slots(2);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].extent, 200.0);
        assert_eq!(slots[1].extent, 160.0);
    }

    // ---- fixed extent (uncontained) ---------------------------------------

    #[test]
    fn uncontained_items_all_rest_at_the_item_extent() {
        let l = FixedLayout::new(100.0, 0.0, 350.0, 0.0);
        let slots = l.slots(10);
        assert_eq!(
            slots[0],
            Slot {
                index: 0,
                offset: 0.0,
                extent: 100.0
            }
        );
        assert_eq!(
            slots[1],
            Slot {
                index: 1,
                offset: 100.0,
                extent: 100.0
            }
        );
        assert_eq!(slots[2].extent, 100.0);
        // The item straddling the trailing edge takes the remainder.
        assert_eq!(slots[3].offset, 300.0);
        assert_eq!(slots[3].extent, 50.0);
        assert_eq!(slots.len(), 4);
    }

    #[test]
    fn the_uncontained_leading_item_shrinks_as_it_scrolls_off() {
        let l = FixedLayout::new(100.0, 0.0, 300.0, 40.0);
        let slots = l.slots(10);
        assert_eq!(slots[0].index, 0);
        assert_eq!(slots[0].offset, 0.0);
        assert_eq!(slots[0].extent, 60.0, "40px of it scrolled off");
        assert_eq!(slots[1].offset, 60.0);
    }

    #[test]
    fn the_uncontained_item_extent_is_capped_at_the_viewport() {
        let l = FixedLayout::new(900.0, 0.0, 300.0, 0.0);
        assert_eq!(l.stride(), 300.0);
        assert_eq!(l.slots(3).len(), 1);
    }

    #[test]
    fn uncontained_max_scroll_offset_is_the_ordinary_list_bound() {
        let l = FixedLayout::new(100.0, 0.0, 350.0, 0.0);
        assert_eq!(l.max_scroll_offset(10), 650.0);
        assert_eq!(l.max_scroll_offset(2), 0.0);
    }

    #[test]
    fn a_zero_extent_uncontained_layout_degrades_instead_of_dividing_by_zero() {
        let l = FixedLayout::new(0.0, 0.0, 300.0, 20.0);
        assert_eq!(l.item_extent_at(0), 0.0);
        assert_eq!(l.layout_offset(0), 0.0);
        assert!(l.slots(5).is_empty());
    }

    #[test]
    fn non_finite_uncontained_inputs_degrade_to_zero() {
        let l = FixedLayout::new(f64::NAN, 0.0, f64::NAN, f64::NAN);
        assert_eq!(l.stride(), 0.0);
        assert!(l.slots(5).is_empty());
    }
}
