use std::cell::Cell;

thread_local! {
    /// The "a deferred state-bearing callback is queued somewhere in this
    /// thread's tree" flag, raised by [`mark_pending_result_flush`] and drained
    /// by [`take_pending_result_flush`].
    ///
    /// A side channel for the same reason [`crate::widget::report_retired_slot`]'s
    /// `RETIRED_SLOTS` list is one: the widget that queues the callback is deep
    /// inside a `View::rebuild` (a `BuildCtx` pass) with no
    /// [`crate::app::RenderRoot`] handle to reach, and — unlike a paint pass — no
    /// threaded per-frame sink.
    ///
    /// **Data-free on purpose.** Only the *fact* that a flush is owed rides here;
    /// the callbacks themselves stay in the widget that queued them. Those
    /// callbacks are `Rc<dyn Fn>` (`!Send`), so they can only ever be run on the
    /// thread that queued them — which is exactly why this is `thread_local`
    /// rather than a process-global `AtomicBool`. A global would let a
    /// [`RenderRoot`](crate::app::RenderRoot) on one thread *drain a mark raised
    /// on another*, broadcasting into a tree with nothing pending while the tree
    /// that actually owes the flush is left waiting — silently reintroducing the
    /// failure this broadcast exists to fix. UI-thread affinity is the same argument
    /// `frust-reactive`'s `CAN_POP_PROVIDER` and `frust-widgets`' `PAGE_REACH`
    /// make for their own `Rc`-backed state.
    static PENDING_RESULT_FLUSH: Cell<bool> = const { Cell::new(false) };
}

/// Record that a widget queued a callback needing `&mut State` during a
/// state-free pass, so [`crate::app::RenderRoot::rebuild`] dispatches an
/// [`InputEvent::Housekeeping`](crate::event::InputEvent::Housekeeping) broadcast before the frame ends.
///
/// Idempotent: marking twice in one pass owes exactly one broadcast, and the
/// broadcast reaches every widget that queued anything (see the variant's
/// routing contract).
///
/// Thread-affine: the mark is visible only to the thread that raised it, which
/// is also the only thread that can run the `!Send` callback it stands for.
pub fn mark_pending_result_flush() {
    PENDING_RESULT_FLUSH.with(|flag| flag.set(true));
}

/// Take (and clear) the [`mark_pending_result_flush`] flag.
///
/// Drained by [`crate::app::RenderRoot::rebuild`], which dispatches one
/// [`InputEvent::Housekeeping`](crate::event::InputEvent::Housekeeping) broadcast per `true` it takes. Destructive,
/// mirroring [`crate::app::RenderRoot::take_change_flags`]: a caller that drains
/// and drops the result loses that flush until something marks again.
pub fn take_pending_result_flush() -> bool {
    PENDING_RESULT_FLUSH.with(|flag| flag.replace(false))
}

/// Non-draining peek at the [`mark_pending_result_flush`] flag — whether a
/// deferred state-bearing callback is owed a [`InputEvent::Housekeeping`](crate::event::InputEvent::Housekeeping)
/// broadcast, without consuming the mark.
///
/// The [`take_change_flags`](crate::app::RenderRoot::take_change_flags) /
/// [`has_pending_change_flags`](crate::app::RenderRoot::has_pending_change_flags)
/// pairing, one layer down: the mobile shells read this while gathering their
/// frame-gate inputs (`FrameInputs::deferred_callbacks_pending`) *before*
/// deciding whether the frame runs at all, so a frame the gate would otherwise
/// skip still runs and reaches the [`crate::app::RenderRoot::rebuild`] that
/// drains the mark. Peeking must not consume it — draining stays that rebuild's
/// job.
///
/// Thread-affine like both of its neighbours: it reports only marks raised on
/// the calling thread (see the `PENDING_RESULT_FLUSH` doc for why the flag is
/// thread-local rather than a process-global `AtomicBool`).
pub fn has_pending_result_flush() -> bool {
    PENDING_RESULT_FLUSH.with(|flag| flag.get())
}

thread_local! {
    /// The "a focused child pod lost its identity during this thread's view
    /// diff" flag, raised by [`mark_focus_orphaned`] and drained by
    /// [`take_focus_orphaned`].
    ///
    /// A side channel for exactly the reason [`PENDING_RESULT_FLUSH`] above is
    /// one: the reconciler that tears a focused pod down runs deep inside a
    /// `View::rebuild` (a [`crate::view::BuildCtx`] pass) with no
    /// [`RenderRoot`](crate::app::RenderRoot) handle to reach, so it cannot
    /// clear the root's `focus_active`/`ime_state` mirror itself — the
    /// long-standing desync `frust-widgets`' `cancel_active_children` documents.
    /// *Which* pods may raise it is narrowed by the pass's own focus chain
    /// ([`crate::view::BuildCtx::has_focus`]) — see [`mark_focus_orphaned`].
    ///
    /// **Data-free on purpose, and idempotent.** Only the *fact* that some
    /// focused pod died rides here; there is nothing useful to carry (the root
    /// keeps no id of the focused widget, only the boolean mirror). Several pods
    /// cleared in one diff owe exactly one release.
    ///
    /// Thread-local rather than a process-global `AtomicBool` for the same
    /// UI-thread-affinity reason: the tree that lost the focus, and the
    /// `RenderRoot` that must release the session, live on one thread. A global
    /// would let a root on one thread release a session another thread's tree
    /// still holds.
    static FOCUS_ORPHANED: Cell<bool> = const { Cell::new(false) };
}

/// Record that a structural rebuild severed the recorded focus path — a focused
/// [`ChildPod`](crate::widget::ChildPod) was torn down, type-swapped, or had its
/// `focused` flag cleared by a reconciler — so
/// [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild) releases the whole
/// focus/IME session before the frame ends.
///
/// # The invariant: a mark means a LIVE session lost its owner
///
/// Raise this only when the severed link was on the **live focus chain** — the
/// pod's own `focused` flag AND
/// [`BuildCtx::has_focus`](crate::view::BuildCtx::has_focus), the composed chain
/// from the root down to it. A `focused` flag on its own is not evidence of a
/// session: a container-routed blur clears the focus link at the nearest common
/// ancestor only, so flags deeper in the blurred branch legitimately stay set,
/// and marking on one of those releases whatever field is *actually* focused
/// elsewhere in the tree — the keyboard dropping mid-typing because an unrelated
/// list recycled a row. Every drain here performs a real, user-visible release;
/// it must never fire on speculation.
///
/// Raised by `frust-widgets`' reconcilers (`teardown_child`,
/// `cancel_active_children`, and the type-swap arms of both the keyed reconciler
/// and the single-child `rebuild_child`), all four through one shared gate
/// (`mark_orphan_if_live`), and by
/// [`ComponentView::rebuild`](crate::component::ComponentView)'s own swap arm,
/// which spells the identical gate by hand because this crate sits below
/// `frust-widgets`. A hand-rolled container that clears a focused pod itself
/// should raise it under the same condition.
///
/// Idempotent and thread-affine, exactly like [`mark_pending_result_flush`].
pub fn mark_focus_orphaned() {
    FOCUS_ORPHANED.with(|flag| flag.set(true));
}

/// Take (and clear) the [`mark_focus_orphaned`] flag.
///
/// Drained by [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild), which
/// performs one full focus/IME session release per `true` it takes. Destructive,
/// mirroring [`take_pending_result_flush`]: a caller that drains and drops the
/// result loses that release until something marks again.
///
/// A mark can only be raised *during* a view diff, and the diff's own
/// `RenderRoot::rebuild` drains it before returning, so the flag never survives
/// a frame — there is no peeking counterpart (unlike
/// [`has_pending_result_flush`], which a frame gate must consult before deciding
/// whether to run the rebuild that drains it at all).
pub fn take_focus_orphaned() -> bool {
    FOCUS_ORPHANED.with(|flag| flag.replace(false))
}

thread_local! {
    /// Which root owes a hover end because a pod holding its LIVE hover link was
    /// dropped by this thread's view diff — raised by [`mark_hover_orphaned`] and
    /// drained by [`take_hover_orphaned`]. `None` when nothing is owed.
    ///
    /// Hover's analog of [`FOCUS_ORPHANED`], and a side channel for the same
    /// missing-handle reason: the reconciler that drops the claimant's
    /// [`ChildPod`](crate::widget::ChildPod) runs inside a
    /// [`View::rebuild`](crate::view::View::rebuild) with no
    /// [`RenderRoot`](crate::app::RenderRoot) to clear the root's hover mirror
    /// with. Idempotent for the same reason too: several pods severed in one diff
    /// owe exactly one hover end.
    ///
    /// **Root-qualified rather than data-free**, which is where it diverges from
    /// its focus neighbour. Focus is raised *and* drained inside one root's own
    /// `rebuild`, so a bare bool cannot reach a second root. A hover mark comes
    /// from a destructor, which fires whenever a pod happens to die — including
    /// while another root on the same thread is the one that rebuilds next — so
    /// the mark carries the identity of the root whose link died and only that
    /// root's drain consumes it. Two roots' epoch counters legitimately collide
    /// (each starts at `1` and advances per hover pass), so the identity, not the
    /// epoch, is what keeps them apart.
    ///
    /// One slot, so two roots severed between the same pair of rebuilds leave the
    /// later mark standing and the earlier root's mirror to lapse on its own next
    /// hover pass — the pre-existing degradation, never a release of a link that
    /// is still held.
    static HOVER_ORPHANED: Cell<Option<u64>> = const { Cell::new(None) };

    /// The hover link standing on this thread right now as `(root identity,
    /// epoch)`, or `(0, 0)` when nothing holds one — published by
    /// [`RenderRoot::event`](crate::app::RenderRoot::event) whenever it closes a
    /// hover pass, and read by a dropping pod to tell a live link from a stale
    /// stamp ([`live_hover_link_is`]).
    ///
    /// The root would otherwise be unreachable from a destructor, and the
    /// distinction is the whole invariant: pods carrying *stale* stamps are
    /// dropped constantly (any recycled list row that was hovered at some point),
    /// and ending the hover on one of those would drop the chrome of whatever is
    /// hovered now.
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason. It mirrors
    /// **one** root, so a second `RenderRoot` driving passes on the same thread
    /// overwrites it — costing the first root's hover the drop-time check (its
    /// link then lapses on the next `Move`, the pre-existing behaviour) rather
    /// than corrupting anything. The root identity in the pair is what makes that
    /// last clause true: a pod of the overwritten root can no longer match the
    /// published link by an epoch integer the two roots happen to share, so it
    /// marks nothing instead of ending the *other* root's live hover.
    static LIVE_HOVER_LINK: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// Record that a [`ChildPod`](crate::widget::ChildPod) holding the **live** hover
/// link was dropped, so [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild)
/// ends the hover before the frame ends.
///
/// # Why a destructor, and not the reconcilers
///
/// [`mark_focus_orphaned`]'s callers are the reconcilers themselves, because a
/// focused pod's link is a flag they own and clear (`set_focused`). Hover has no
/// such flag and no setter: the link is an epoch stamp
/// ([`ChildPod::hover_epoch`](crate::widget::ChildPod::hover_epoch)) that only
/// [`ChildPod::event_child`](crate::widget::ChildPod::event_child) may write,
/// deliberately, so that no container can record or clear a hover by hand. A
/// container therefore *cannot* report its own severance, and hand-rolled
/// containers outside this workspace could never opt in. The pod reports instead,
/// from `Drop`, which covers every removal route — a truncated `Vec`, a
/// `None`-ed `Option`, a keyed reconciler's dropped entry, a whole subtree torn
/// down — with nothing to remember to call.
///
/// # The invariant: a mark means the LIVE link lost its owner
///
/// Exactly [`mark_focus_orphaned`]'s invariant, enforced by the stamp comparison
/// instead of a chain: a pod marks only when its own stamp is non-zero *and*
/// names the published link — same root identity, same epoch
/// ([`live_hover_link_is`]). A stale stamp — the far commoner case, since a
/// stamp is never cleared, only stranded by the next epoch advance — marks
/// nothing, and so does a stamp from a *different* root that happens to carry the
/// same epoch integer.
///
/// `root` is the identity the claim was stamped with (the root that ran the hover
/// pass), so only that root's [`take_hover_orphaned`] consumes the mark.
///
/// Idempotent and thread-affine, exactly like [`mark_focus_orphaned`].
pub(crate) fn mark_hover_orphaned(root: u64) {
    HOVER_ORPHANED.with(|slot| slot.set(Some(root)));
}

/// Take (and clear) a [`mark_hover_orphaned`] mark raised for `root`.
///
/// Drained by [`RenderRoot::rebuild`](crate::app::RenderRoot::rebuild), which
/// ends its own standing hover link per `true` it takes. Destructive for the
/// matching root only, mirroring [`take_focus_orphaned`]: a mark another root
/// raised is left standing rather than consumed, which is what keeps two roots on
/// one thread from ending each other's hover.
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

/// Publish the hover link standing on this thread — `root`'s live epoch while one
/// of its widgets holds the link, epoch `0` while none does.
///
/// Called by [`RenderRoot::event`](crate::app::RenderRoot::event) as it closes a
/// hover pass, and by the rebuild-time end that [`take_hover_orphaned`] drives.
pub(crate) fn set_live_hover_link(root: u64, epoch: u64) {
    LIVE_HOVER_LINK.with(|slot| slot.set((root, epoch)));
}

/// Whether `(root, epoch)` is the hover link standing on this thread — what a
/// dropping [`ChildPod`](crate::widget::ChildPod) compares its own stamp against
/// (see [`mark_hover_orphaned`]). Epoch `0` is "no link" and never matches.
pub(crate) fn live_hover_link_is(root: u64, epoch: u64) -> bool {
    epoch != 0 && LIVE_HOVER_LINK.with(|slot| slot.get()) == (root, epoch)
}
