//! The windowed reconcilers a lazy list runs at rebuild time, shared by
//! [`ListView`](crate::ListView), the lazy slivers
//! ([`sliver_fixed_extent_list`](crate::sliver_fixed_extent_list),
//! [`sliver_list`](crate::sliver_list), [`sliver_grid`](crate::sliver_grid)),
//! plus the measured-extent bookkeeping `ListView` and `sliver_list` share.
//!
//! A windowed list materialises only a contiguous, ascending run of item
//! indices — its *window* — and keeps one [`ChildPod`] per materialised index.
//! Each rebuild moves the window to a new `[start, end)` range and reconciles
//! the live pods to it, in one of two identity models (`list_view.rs`'s *Row
//! identity* is the contract):
//!
//! * [`reconcile_positional`] matches rows **by item index**: an index staying
//!   in the window keeps its live pod, rebuilt in place against its own
//!   previous view; indices leaving are torn down, indices entering are built
//!   fresh.
//! * [`reconcile_keyed`] matches rows **by stable key**: slots stay the
//!   contiguous index run, and identity lives beside them in a retained
//!   `key -> the index that row occupied last frame` map, so a row's pod (and
//!   all of its retained state) follows the row to a new index.
//!
//! Both rebuild a survivor against `prev(index it rendered last frame)` →
//! `next(index it renders now)`: the item builder is a pure function of the
//! index, so no per-window view cache is retained. Both report
//! `ChangeFlags::LAYOUT | ChangeFlags::PAINT` on any frame that built, tore
//! down, relocated or re-ranged a pod — a freshly built or relocated pod has
//! never been laid out where it now sits. What a list does around the
//! reconciliation (placing the pods, the measured-extent cache hygiene) stays
//! with the list.
//!
//! # Keep-alive retention
//!
//! [`reconcile_keyed_retaining`] is [`reconcile_keyed`] plus a side map of
//! rows kept alive outside the window ([`Retention`]): a departing pod that
//! asked to be kept (and whose key still names a row of the data) is parked
//! there with the view it was last built or rebuilt from instead of being torn
//! down, and a slot whose key is parked re-adopts that pod, rebuilt from the
//! parked view to this frame's. The parked view, not `prev(index)`, is the
//! diff base on re-adoption: the row may have changed in the data while it was
//! parked, and a rebuild diffing two later views would miss that change.
//!
//! # Measured extents
//!
//! A variable-extent list caches each laid-out row's extent under the row's
//! stable key ([`Measured`]) beside the running sum of those extents, so the
//! content extent `Σ measured + estimate × unmeasured` is O(1)
//! ([`estimated_total`]); [`record_measurement`], [`forget_measurement`] and
//! [`evict_measurements_past`] keep the sum exact. [`walk_to_offset`] is the
//! bounded prefix walk from a known `(index, y)` to an offset over those
//! extents. `list_view.rs`'s *Variable extents* is the contract they implement.
//!
//! # Locating a key
//!
//! [`probe_key`] tests the two index hypotheses a keyed list's anchoring and
//! cache hygiene rely on — the key's index unchanged, or shifted by the frame's
//! net item-count delta. [`locate_key`] adds a bounded outward scan once both
//! miss, which resolves a key a same-frame mutation moved by a different
//! amount (a prepend above plus an append below it) without keying all
//! `item_count` items.

use std::any::Any;
use std::collections::HashMap;
use std::ops::Range;

use frust_core::{AnyView, BuildCtx, ChangeFlags, ChildPod};

use crate::ChildKey;

/// The previous and the current frame's item builders: `index -> view`, each
/// a pure function of the index.
pub(crate) struct ItemViews<'a, State: 'static> {
    /// The builder the live pods were built (or last rebuilt) from.
    pub(crate) prev: &'a dyn Fn(usize) -> AnyView<State>,
    /// This frame's builder.
    pub(crate) next: &'a dyn Fn(usize) -> AnyView<State>,
}

/// A list's materialised window: its pods and, parallel to them, the item
/// index each renders — always a contiguous ascending run.
pub(crate) struct LiveRows<'a> {
    pub(crate) children: &'a mut Vec<ChildPod>,
    pub(crate) indices: &'a mut Vec<usize>,
}

/// What [`reconcile_keyed`] decided, beyond the flags, for a caller that
/// keeps per-key state of its own.
pub(crate) struct KeyedOutcome {
    /// The change flags the reconciliation owes (`LAYOUT | PAINT` on any
    /// structural change, plus whatever the survivors' rebuilds reported).
    pub(crate) flags: ChangeFlags,
    /// Each new slot's key, parallel to the new window — `key_of` ran exactly
    /// once per slot to produce them.
    pub(crate) slot_keys: Vec<ChildKey>,
    /// Whether any slot of the new window matched a key the previous window
    /// held — an *exact* lookup, never an index hypothesis.
    pub(crate) any_survivor: bool,
    /// The previous window's keys whose pods no slot claimed (and which were
    /// torn down), each with the item index it rendered last frame.
    pub(crate) departed: Vec<(ChildKey, usize)>,
}

/// Reconcile `rows` to the window `range` **by item index**: an index that
/// stays in the window keeps its live pod and is rebuilt in place against its
/// own previous view, indices leaving are torn down, indices entering are
/// built fresh.
///
/// Because the window is a contiguous range, any change to it necessarily
/// builds or tears down at least one pod, so the structural report covers the
/// "window shifted" case as well as the materialisation one.
pub(crate) fn reconcile_positional<State: 'static>(
    views: ItemViews<'_, State>,
    rows: LiveRows<'_>,
    range: Range<usize>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let (start, end) = (range.start, range.end);
    let mut flags = ChangeFlags::NONE;
    // Move the live window out so surviving indices can be relocated by key.
    let old_keys = std::mem::take(rows.indices);
    let old_children = std::mem::take(rows.children);
    let mut old: HashMap<usize, ChildPod> = old_keys.into_iter().zip(old_children).collect();

    let mut new_children = Vec::with_capacity(end.saturating_sub(start));
    let mut new_keys = Vec::with_capacity(end.saturating_sub(start));
    let mut structural = false;

    for index in start..end {
        if let Some(mut pod) = old.remove(&index) {
            // Survivor: rebuild in place against its own previous view
            // (reconstructed from the pure builder) — state preserved.
            let prev_view = (views.prev)(index);
            let next_view = (views.next)(index);
            flags |= crate::authoring::rebuild_child(&prev_view, &next_view, &mut pod, ctx);
            new_children.push(pod);
        } else {
            new_children.push(crate::authoring::build_child(&(views.next)(index), ctx));
            structural = true;
        }
        new_keys.push(index);
    }

    // Indices that left the window are torn down (cancel-if-active inside
    // teardown_child unwinds an armed child).
    for (index, mut pod) in old.drain() {
        crate::authoring::teardown_child(&(views.prev)(index), &mut pod, ctx);
        structural = true;
    }

    *rows.children = new_children;
    *rows.indices = new_keys;

    if structural {
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    flags
}

/// Reconcile `rows` to the window `range` **by stable key**.
///
/// Slots stay the contiguous index run `range`; identity is matched through
/// the retained `key_index` map (`key -> the item index that row occupied last
/// frame`) instead, which this call replaces with the new window's. For each
/// slot: the key `key_of(index)` names the item index that row occupied last
/// frame, whose live pod is relocated into this slot and rebuilt against
/// `prev(prev_index)` → `next(index)`. Keys with no entry build fresh; pods no
/// slot claimed are torn down with the same cancel-if-active
/// [`crate::authoring::teardown_child`] path the positional reconciler uses.
///
/// A duplicate key is ambiguous and trips a `debug_assert!` carrying
/// `duplicate_key_msg`, mirroring [`crate::authoring::rebuild_children`]'s;
/// release builds carry on (first claim keeps the live row, later duplicates
/// build fresh) rather than panic.
pub(crate) fn reconcile_keyed<State: 'static>(
    views: ItemViews<'_, State>,
    rows: LiveRows<'_>,
    key_index: &mut HashMap<ChildKey, usize>,
    key_of: &dyn Fn(usize) -> ChildKey,
    range: Range<usize>,
    duplicate_key_msg: &str,
    ctx: &mut BuildCtx<'_>,
) -> KeyedOutcome {
    reconcile_keyed_retaining(
        views,
        rows,
        key_index,
        key_of,
        range,
        duplicate_key_msg,
        None,
        ctx,
    )
}

/// A row kept alive outside a keyed list's window: its pod, parked (never laid
/// out, painted or routed to), the view it was last built or rebuilt from, and
/// the item index its key last resolved to.
pub(crate) struct KeptRow {
    pub(crate) pod: ChildPod,
    /// The row's last view, an `AnyView<State>` erased to [`Any`] because the
    /// widget parking it is not generic over the state.
    pub(crate) view: Box<dyn Any>,
    pub(crate) index: usize,
}

impl KeptRow {
    /// Tear the parked row down through the view it was last rebuilt from.
    pub(crate) fn teardown<State: 'static>(mut self, ctx: &mut BuildCtx<'_>) {
        if let Some(view) = self.view.downcast_ref::<AnyView<State>>() {
            crate::authoring::teardown_child(view, &mut self.pod, ctx);
        }
    }
}

/// The keep-alive half of [`reconcile_keyed_retaining`] (the module docs'
/// *Keep-alive retention*).
pub(crate) struct Retention<'a> {
    /// The parked rows, by key.
    pub(crate) kept: &'a mut HashMap<ChildKey, KeptRow>,
    /// Whether a pod leaving the window asked to be kept alive.
    pub(crate) wants: &'a dyn Fn(&ChildPod) -> bool,
    /// Where a departing key sits in this frame's data, given the item index
    /// it rendered last frame — `None` once it left the data.
    pub(crate) locate: &'a dyn Fn(ChildKey, usize) -> Option<usize>,
}

/// [`reconcile_keyed`] with an optional keep-alive side map (the module docs'
/// *Keep-alive retention*): a slot whose key is parked in `retention` re-adopts
/// the parked pod, and a departing pod that asks to be kept — and whose key
/// [`Retention::locate`] still finds in the data — is parked instead of torn
/// down. [`KeyedOutcome::departed`] lists only the rows actually torn down.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile_keyed_retaining<State: 'static>(
    views: ItemViews<'_, State>,
    rows: LiveRows<'_>,
    key_index: &mut HashMap<ChildKey, usize>,
    key_of: &dyn Fn(usize) -> ChildKey,
    range: Range<usize>,
    duplicate_key_msg: &str,
    mut retention: Option<Retention<'_>>,
    ctx: &mut BuildCtx<'_>,
) -> KeyedOutcome {
    let (start, end) = (range.start, range.end);
    let capacity = end.saturating_sub(start);
    // One pass over the new window: each slot's key (so `key_of` runs exactly
    // once per slot), the map this frame will retain, and the duplicate check
    // — done up front, like the authoring reconciler's, so a debug build trips
    // before any pod has been moved.
    let mut slot_keys: Vec<ChildKey> = Vec::with_capacity(capacity);
    let mut next_index_of: HashMap<ChildKey, usize> = HashMap::with_capacity(capacity);
    let mut duplicate = false;
    for index in start..end {
        let key = key_of(index);
        duplicate |= next_index_of.insert(key, index).is_some();
        slot_keys.push(key);
    }
    debug_assert!(!duplicate, "{}", duplicate_key_msg);

    // Move the live window out so surviving rows can be relocated by key: the
    // previous frame's `key -> item index` map, and the pods by the item index
    // each rendered.
    let prev_index_of = std::mem::take(key_index);
    let old_keys = std::mem::take(rows.indices);
    let old_children = std::mem::take(rows.children);
    // A moved window range owes a layout pass on its own, without depending on
    // the match outcome below to prove it (the window is a contiguous run, so
    // first index + length pin it exactly).
    let window_shifted =
        old_keys.first().copied() != (capacity > 0).then_some(start) || old_keys.len() != capacity;
    let mut old: HashMap<usize, ChildPod> = old_keys.into_iter().zip(old_children).collect();

    let mut new_children = Vec::with_capacity(capacity);
    let mut new_keys = Vec::with_capacity(capacity);
    let mut flags = ChangeFlags::NONE;
    let mut structural = window_shifted;
    let mut any_survivor = false;

    for (index, key) in (start..end).zip(slot_keys.iter().copied()) {
        let survivor = prev_index_of
            .get(&key)
            .copied()
            .and_then(|prev_index| old.remove(&prev_index).map(|pod| (prev_index, pod)));
        match survivor {
            Some((prev_index, mut pod)) => {
                any_survivor = true;
                // The row survived under its key: relocate its live pod into
                // this slot and rebuild it in place against the view it
                // actually holds — `prev(prev_index)`, the index it rendered
                // last frame. That redirection is the whole point of the
                // retained map; state (hosted component, press, toggle) rides
                // along with the pod.
                let prev_view = (views.prev)(prev_index);
                let next_view = (views.next)(index);
                flags |= crate::authoring::rebuild_child(&prev_view, &next_view, &mut pod, ctx);
                if prev_index != index {
                    // Same row, different slot: its origin moves, so the frame
                    // owes a layout pass even though nothing was built or torn
                    // down.
                    structural = true;
                }
                new_children.push(pod);
            }
            None => {
                // A key with no live row: entering the window (re-adopting a
                // parked pod when it was kept alive), or new data.
                let parked = retention
                    .as_mut()
                    .and_then(|retention| retention.kept.remove(&key));
                let pod = match parked {
                    Some(KeptRow { mut pod, view, .. }) => {
                        let next_view = (views.next)(index);
                        match view.downcast_ref::<AnyView<State>>() {
                            Some(prev_view) => {
                                flags |= crate::authoring::rebuild_child(
                                    prev_view, &next_view, &mut pod, ctx,
                                );
                                pod
                            }
                            None => crate::authoring::build_child(&next_view, ctx),
                        }
                    }
                    None => crate::authoring::build_child(&(views.next)(index), ctx),
                };
                new_children.push(pod);
                structural = true;
            }
        }
        new_keys.push(index);
    }

    // The previous window's keys whose rows no slot claimed: they left the
    // window or the data. A row asking to be kept alive whose key the data
    // still holds is parked; every other one is torn down.
    let mut departed: Vec<(ChildKey, usize)> = Vec::new();
    for (key, prev_index) in prev_index_of {
        let Some(mut pod) = old.remove(&prev_index) else {
            continue;
        };
        structural = true;
        if let Some(retention) = retention.as_mut()
            && (retention.wants)(&pod)
            && let Some(index) = (retention.locate)(key, prev_index)
        {
            // A parked pod is routed nothing, so a gesture it captured could
            // never end: unwind it now, as a teardown would.
            if pod.is_active() {
                crate::authoring::cancel_pod(&mut pod);
                pod.set_active(false);
            }
            retention.kept.insert(
                key,
                KeptRow {
                    pod,
                    view: Box::new((views.prev)(prev_index)),
                    index,
                },
            );
            continue;
        }
        crate::authoring::teardown_child(&(views.prev)(prev_index), &mut pod, ctx);
        departed.push((key, prev_index));
    }

    // A pod no key maps to any more (the later slot of a duplicate key in
    // release) has no identity to keep: torn down.
    for (index, mut pod) in old.drain() {
        crate::authoring::teardown_child(&(views.prev)(index), &mut pod, ctx);
        structural = true;
    }

    *rows.children = new_children;
    *rows.indices = new_keys;
    *key_index = next_index_of;

    if structural {
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    KeyedOutcome {
        flags,
        slot_keys,
        any_survivor,
        departed,
    }
}

// --- Locating a key ----------------------------------------------------------

/// How far either side of its last index [`locate_key`] scans for a key once
/// both of [`probe_key`]'s hypotheses missed, in items. Bounds the scan's
/// `key_of` calls at `2 × KEY_SCAN_RADIUS`, paid only on a frame whose
/// mutation moved the key by something other than the net item-count delta.
pub(crate) const KEY_SCAN_RADIUS: usize = 64;

/// Where `key` sits in this frame's data, tested at the two hypotheses a keyed
/// list's anchoring relies on: `prev_index` unchanged, or shifted by the
/// frame's net item-count `delta` — at most two `key_of` calls. `None` when
/// neither holds (the key left the data, or a mutation moved it by some other
/// amount).
pub(crate) fn probe_key(
    key: ChildKey,
    prev_index: usize,
    delta: isize,
    item_count: usize,
    key_of: &dyn Fn(usize) -> ChildKey,
) -> Option<usize> {
    if prev_index < item_count && key_of(prev_index) == key {
        return Some(prev_index);
    }
    if delta != 0 {
        let candidate = prev_index as isize + delta;
        if candidate >= 0 && (candidate as usize) < item_count && key_of(candidate as usize) == key
        {
            return Some(candidate as usize);
        }
    }
    None
}

/// [`probe_key`], then — when both hypotheses miss — the nearest index within
/// [`KEY_SCAN_RADIUS`] of `prev_index` (shifted by `delta`, clamped to the
/// data) whose key is `key`, alternating below and above. `None` when the key
/// is not within reach: it left the data, or moved farther than the scan.
pub(crate) fn locate_key(
    key: ChildKey,
    prev_index: usize,
    delta: isize,
    item_count: usize,
    key_of: &dyn Fn(usize) -> ChildKey,
) -> Option<usize> {
    if item_count == 0 {
        return None;
    }
    if let Some(index) = probe_key(key, prev_index, delta, item_count, key_of) {
        return Some(index);
    }
    // Scan around both hypotheses' midpoint, so a two-sided mutation (some
    // rows above the key, some below) is reached in the fewest steps.
    let centre = (prev_index as isize + delta / 2).clamp(0, item_count as isize - 1);
    let matches = |candidate: isize| {
        candidate >= 0 && (candidate as usize) < item_count && key_of(candidate as usize) == key
    };
    for step in 0..=KEY_SCAN_RADIUS as isize {
        if matches(centre - step) {
            return Some((centre - step) as usize);
        }
        if step > 0 && matches(centre + step) {
            return Some((centre + step) as usize);
        }
    }
    None
}

// --- Measured extents ----------------------------------------------------------

/// How many items one prefix walk ([`walk_to_offset`]) steps over at most: a
/// scroll or a fling frame moves a handful; a gap farther than this many
/// estimated items is a jump or a data reset, resolved in closed form first.
pub(crate) const MAX_PREFIX_STEP: usize = 512;

/// One row's cached measurement in variable-extent mode, held under the row's
/// stable key (the module docs' *Measured extents*).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Measured {
    /// The item index this row occupied the last time it was measured — the
    /// only handle the cache has on "the current keying can no longer produce
    /// this entry" when `item_count` shrinks.
    pub(crate) index: usize,
    /// The row's laid-out main-axis extent (logical px).
    pub(crate) extent: f64,
}

/// Cache one row's laid-out extent under its stable key, keeping `sum` (the
/// running Σ of the cache's extents) exact.
pub(crate) fn record_measurement(
    cache: &mut HashMap<ChildKey, Measured>,
    sum: &mut f64,
    key: ChildKey,
    index: usize,
    extent: f64,
) {
    match cache.insert(key, Measured { index, extent }) {
        Some(previous) => *sum += extent - previous.extent,
        None => *sum += extent,
    }
}

/// Drop one row's measurement (its key left the data), keeping `sum` exact.
pub(crate) fn forget_measurement(
    cache: &mut HashMap<ChildKey, Measured>,
    sum: &mut f64,
    key: &ChildKey,
) {
    if let Some(previous) = cache.remove(key) {
        *sum -= previous.extent;
    }
}

/// Drop the measurements the current keying provably cannot produce any more:
/// entries recorded at an index at or past `item_count` (the just-shrunk end).
/// A scan of the cache, for a frame whose item count shrank only. A stale
/// recorded index can evict a still-live row early; it is re-measured on its
/// next visit, which is the conservative direction.
pub(crate) fn evict_measurements_past(
    cache: &mut HashMap<ChildKey, Measured>,
    sum: &mut f64,
    item_count: usize,
) {
    if cache.is_empty() {
        return;
    }
    let mut dropped = 0.0;
    cache.retain(|_, entry| {
        let live = entry.index < item_count;
        if !live {
            dropped += entry.extent;
        }
        live
    });
    *sum -= dropped;
}

/// The whole content's estimated extent: `Σ measured + estimate × unmeasured`
/// — O(1), the sum being maintained as rows are measured.
pub(crate) fn estimated_total(
    measured_sum: f64,
    measured_count: usize,
    item_count: usize,
    estimate: f64,
) -> f64 {
    let unmeasured = item_count.saturating_sub(measured_count);
    measured_sum + estimate * unmeasured as f64
}

/// Walk from a known prefix point — item `index` starts at content offset `y`
/// — to `target`, returning the first item `target` falls inside (clamped to
/// the data) and that item's content offset, summing `extent_at` (a row's
/// measured extent, or the estimate) on the way.
///
/// O(step), at most [`MAX_PREFIX_STEP`] steps: a gap farther than that many
/// estimated items resolves its bulk in closed form against `estimate` first.
/// The walk itself never re-pins item 0 to offset 0; a caller that owes that
/// does it on the result.
pub(crate) fn walk_to_offset(
    item_count: usize,
    index: usize,
    y: f64,
    target: f64,
    estimate: f64,
    extent_at: impl Fn(usize) -> f64,
) -> (usize, f64) {
    debug_assert!(item_count > 0, "walk_to_offset needs a non-empty list");
    let mut index = index.min(item_count - 1);
    let mut y = y;

    let gap = target - y;
    if estimate > 0.0 && gap.abs() > estimate * MAX_PREFIX_STEP as f64 {
        // A data reset or a programmatic jump, never a scroll: cover the bulk
        // against the estimate so the walk below stays bounded.
        let jump = (gap / estimate).trunc();
        let landed = (index as f64 + jump).clamp(0.0, (item_count - 1) as f64);
        y += (landed - index as f64) * estimate;
        index = landed as usize;
    }

    let mut steps = 0usize;
    while y > target && index > 0 && steps < MAX_PREFIX_STEP {
        index -= 1;
        y -= extent_at(index);
        steps += 1;
    }
    while index + 1 < item_count && steps < MAX_PREFIX_STEP {
        let extent = extent_at(index);
        if y + extent <= target {
            y += extent;
            index += 1;
            steps += 1;
        } else {
            break;
        }
    }
    (index, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn locate_key_resolves_both_hypotheses_and_a_two_sided_move() {
        let key = ChildKey::new(100u64);
        // Three rows prepended above key 100: the shifted hypothesis, in at
        // most two `key_of` calls.
        let shifted: Vec<u64> = (1000..1003).chain(0..200).collect();
        let calls = Cell::new(0u32);
        let key_of_shifted = |i: usize| {
            calls.set(calls.get() + 1);
            ChildKey::new(shifted[i])
        };
        assert_eq!(
            probe_key(key, 100, 3, shifted.len(), &key_of_shifted),
            Some(103)
        );
        assert_eq!(calls.get(), 2);
        // Three prepended above and five appended below: the net delta (8) is
        // not the key's own shift (3), so only the scan finds it.
        let both: Vec<u64> = (1000..1003).chain(0..200).chain(5000..5005).collect();
        let key_of = |i: usize| ChildKey::new(both[i]);
        assert_eq!(probe_key(key, 100, 8, both.len(), &key_of), None);
        assert_eq!(locate_key(key, 100, 8, both.len(), &key_of), Some(103));
        // Gone from the data, or no data at all: no match.
        assert_eq!(
            locate_key(ChildKey::new(9999u64), 100, 8, both.len(), &key_of),
            None
        );
        assert_eq!(locate_key(key, 0, 0, 0, &key_of), None);
    }

    #[test]
    fn the_measured_cache_keeps_its_sum_exact() {
        let mut cache = HashMap::new();
        let mut sum = 0.0;
        record_measurement(&mut cache, &mut sum, ChildKey::new(1u64), 0, 40.0);
        record_measurement(&mut cache, &mut sum, ChildKey::new(2u64), 1, 60.0);
        record_measurement(&mut cache, &mut sum, ChildKey::new(1u64), 0, 45.0);
        assert_eq!(sum, 105.0);
        assert_eq!(estimated_total(sum, cache.len(), 10, 50.0), 505.0);
        forget_measurement(&mut cache, &mut sum, &ChildKey::new(1u64));
        assert_eq!(sum, 60.0);
        evict_measurements_past(&mut cache, &mut sum, 1);
        assert!(cache.is_empty());
        assert_eq!(sum, 0.0);
    }

    #[test]
    fn the_prefix_walk_steps_over_measured_extents_and_jumps_in_closed_form() {
        let extent = |i: usize| if i.is_multiple_of(2) { 40.0 } else { 60.0 };
        // From item 0 at 0: 130 lies in item 2, which starts at 100.
        assert_eq!(walk_to_offset(100, 0, 0.0, 130.0, 50.0, extent), (2, 100.0));
        // Backwards from item 4 (at 200) to 50: item 1 starts at 40.
        assert_eq!(walk_to_offset(100, 4, 200.0, 50.0, 50.0, extent), (1, 40.0));
        // Past the end clamps to the last item; nothing re-pins item 0.
        assert_eq!(walk_to_offset(3, 0, 0.0, 10_000.0, 50.0, extent).0, 2);
        assert_eq!(walk_to_offset(3, 2, 70.0, 0.0, 50.0, extent), (0, -30.0));
        // A jump far beyond the step bound lands by the estimate.
        let (index, y) = walk_to_offset(1_000_000, 0, 0.0, 5_000_000.0, 50.0, |_| 50.0);
        assert_eq!((index, y), (100_000, 5_000_000.0));
    }
}
