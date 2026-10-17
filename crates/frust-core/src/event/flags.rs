//! Thread-local rebuild flags a pass deep in a view diff raises for `RenderRoot::rebuild` to drain.

use std::cell::Cell;

thread_local! {
    /// A deferred state-bearing callback is queued; raised by [`mark_pending_result_flush`].
    /// Thread-local, not a global `AtomicBool`: the `!Send` callbacks it stands for run only on
    /// the queuing thread, so another thread's root must never drain it (true of every slot here).
    static PENDING_RESULT_FLUSH: Cell<bool> = const { Cell::new(false) };
}

/// Owe one [`InputEvent::Housekeeping`](crate::event::InputEvent::Housekeeping) broadcast before
/// the frame ends, for a callback queued needing `&mut State`. Idempotent.
pub fn mark_pending_result_flush() {
    PENDING_RESULT_FLUSH.with(|flag| flag.set(true));
}

/// Take (and clear) the [`mark_pending_result_flush`] flag; drained by `RenderRoot::rebuild`.
pub fn take_pending_result_flush() -> bool {
    PENDING_RESULT_FLUSH.with(|flag| flag.replace(false))
}

/// Peek the flag without draining it, for a mobile shell's frame gate deciding whether to run.
pub fn has_pending_result_flush() -> bool {
    PENDING_RESULT_FLUSH.with(|flag| flag.get())
}

thread_local! {
    /// A focused pod lost its identity in this thread's view diff; see [`mark_focus_orphaned`].
    static FOCUS_ORPHANED: Cell<bool> = const { Cell::new(false) };
}

/// Owe a full focus/IME session release: a rebuild severed the live focus path. Raise only when the
/// pod's `focused` flag AND [`BuildCtx::has_focus`](crate::view::BuildCtx::has_focus) hold — a
/// stale deeper flag must never release a field focused elsewhere. Idempotent.
pub fn mark_focus_orphaned() {
    FOCUS_ORPHANED.with(|flag| flag.set(true));
}

/// Take (and clear) the [`mark_focus_orphaned`] flag; drained by `RenderRoot::rebuild`.
pub fn take_focus_orphaned() -> bool {
    FOCUS_ORPHANED.with(|flag| flag.replace(false))
}

thread_local! {
    /// Which root owes a hover end because a pod holding its live link was dropped.
    static HOVER_ORPHANED: Cell<Option<u64>> = const { Cell::new(None) };

    /// The live hover link as `(root identity, epoch)`; `(0, 0)` when nothing holds one.
    static LIVE_HOVER_LINK: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// Owe `root` a hover end; raised from `Drop` by a pod whose stamp names the live link.
pub(crate) fn mark_hover_orphaned(root: u64) {
    HOVER_ORPHANED.with(|slot| slot.set(Some(root)));
}

/// Take (and clear) a mark raised for `root`; another root's mark is left standing.
pub(crate) fn take_hover_orphaned(root: u64) -> bool {
    HOVER_ORPHANED.with(|slot| {
        if slot.get() == Some(root) {
            slot.set(None);
            true
        } else {
            false
        }
    })
}

/// Publish `root`'s live hover epoch on this thread (`0` while nothing holds the link).
pub(crate) fn set_live_hover_link(root: u64, epoch: u64) {
    LIVE_HOVER_LINK.with(|slot| slot.set((root, epoch)));
}

/// Whether `(root, epoch)` is the live link; epoch `0` never matches.
pub(crate) fn live_hover_link_is(root: u64, epoch: u64) -> bool {
    epoch != 0 && LIVE_HOVER_LINK.with(|slot| slot.get()) == (root, epoch)
}
