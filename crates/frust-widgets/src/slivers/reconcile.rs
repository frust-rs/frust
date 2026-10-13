//! The windowed reconcilers a lazy list runs at rebuild time, shared by
//! [`ListView`](crate::ListView) and
//! [`sliver_fixed_extent_list`](crate::sliver_fixed_extent_list).
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
//! reconciliation (placing the pods, `ListView`'s measured-extent cache
//! hygiene) stays with the list.

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
                // A key with no live row: entering the window, or new data.
                new_children.push(crate::authoring::build_child(&(views.next)(index), ctx));
                structural = true;
            }
        }
        new_keys.push(index);
    }

    // The previous window's keys whose rows no slot claimed: they left the
    // window or the data (the caller tells the two apart if it needs to).
    let departed: Vec<(ChildKey, usize)> = prev_index_of
        .iter()
        .filter(|(_, prev_index)| old.contains_key(prev_index))
        .map(|(key, prev_index)| (*key, *prev_index))
        .collect();

    // Every pod no slot claimed is a key that left the window or the data.
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
