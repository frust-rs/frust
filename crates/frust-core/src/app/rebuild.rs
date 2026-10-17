use kurbo::Rect;

use super::{MAX_PENDING_RESULT_FLUSH_PASSES, RenderRoot};
use crate::event::InputEvent;
use crate::tree::{InspectNode, WidgetPod, WidgetTree};
use crate::view::{BuildCtx, ChangeFlags, View, WidgetId};
use crate::widget::PlatformViewFrame;

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// The [`PlatformViewFrame`]s published during the most recent
    /// [`RenderRoot::paint`], in paint order.
    ///
    /// Replaced wholesale every pass (see the `platform_view_frames` field
    /// doc), so a pass with no publishers yields an empty slice — a shell
    /// never sees a stale frame for a slot that stopped painting.
    pub fn platform_view_frames(&self) -> &[PlatformViewFrame] {
        &self.platform_view_frames
    }

    /// The z-shield rects reported during the most recent [`RenderRoot::paint`]
    /// (see [`crate::widget::PaintCtx::report_input_shield`]), in paint order.
    ///
    /// Replaced wholesale every pass, exactly like
    /// [`RenderRoot::platform_view_frames`] — a shell feeds both into the same
    /// differ ingest call, and the differ intersects these against each
    /// interactive slot's rect.
    pub fn input_shields(&self) -> &[Rect] {
        &self.input_shields
    }

    /// Drain the slot ids whose `platform_view` widgets were torn down since the
    /// last call (`View::teardown` ran on them — see
    /// [`crate::widget::report_retired_slot`]).
    ///
    /// The prompt-teardown channel: a shell calls this once per frame, right
    /// after its rebuild, and retires each id in its platform-view differ
    /// (`PlatformViewState::retire`) so a disposed slot's native view goes away
    /// immediately instead of waiting out the differ's missing-streak
    /// heuristic. Draining is destructive, mirroring
    /// [`RenderRoot::take_change_flags`]: an id is reported exactly once, so a
    /// shell that drains and drops the result loses the prompt path (the
    /// missing-streak backstop still covers it).
    ///
    /// A merely *culled* slot (scrolled offscreen, a parent skipping paint)
    /// never appears here — culling doesn't run `teardown` — which is what
    /// keeps the camera keep-alive contract intact.
    pub fn take_retired_platform_views(&mut self) -> Vec<u64> {
        crate::widget::take_retired_slots()
    }

    /// Take (and clear) the dirtiness accumulated since the last call.
    ///
    /// A shell can consult this to skip the layout/paint passes when nothing has
    /// changed and no redraw was requested (a desktop optimisation; the mobile
    /// continuous-loop shells may ignore it and repaint every tick). Each
    /// [`RenderRoot::rebuild`] merges its result here; this drains it.
    pub fn take_change_flags(&mut self) -> ChangeFlags {
        let flags = self.pending;
        self.pending = ChangeFlags::NONE;
        flags
    }

    /// Non-draining peek at the dirtiness accumulated since the last
    /// [`RenderRoot::take_change_flags`] — `true` when any `LAYOUT`/`PAINT`
    /// bit is pending, without clearing it.
    ///
    /// Complements [`take_change_flags`](RenderRoot::take_change_flags) for a
    /// shell frame gate: the gate reads this as one of its
    /// "should this frame run" inputs *before* deciding, so a frame it chooses
    /// to skip leaves `pending` intact for the next non-skipped frame to drain
    /// and act on. Draining stays the job of `take_change_flags`, called only
    /// on a frame that actually runs its layout/paint passes. No behavioral
    /// change to rebuild/layout/paint.
    pub fn has_pending_change_flags(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The root widget id, once built.
    pub fn root_id(&self) -> Option<WidgetId> {
        self.root_id
    }

    /// Shared access to the retained tree (for the shell / tests).
    pub fn tree(&self) -> &WidgetTree {
        &self.tree
    }

    /// A read-only, pre-order snapshot of the retained tree for tooling: per
    /// node an id, its parent and children, the concrete widget's type name, an
    /// optional debug label, and its absolute border box in logical px.
    ///
    /// Computed on demand in O(nodes) and takes `&self` — no per-frame
    /// bookkeeping, no mutation, and nothing here participates in
    /// build/layout/paint. Bounds reflect the **last layout pass**, so call it
    /// after one (before the first, every rect is zero-sized).
    ///
    /// Scope: the walk covers the [`WidgetTree`] arena *and* the
    /// [`ChildPod`](crate::widget::ChildPod)s containers own, reached through
    /// [`Widget::visit_children`](crate::widget::Widget::visit_children) — so it
    /// is the real retained hierarchy, not just the arena (which holds little
    /// more than the root pod). A container that leaves that seam defaulted
    /// reads as a leaf.
    pub fn inspect(&self) -> Vec<InspectNode> {
        self.tree.inspect()
    }

    /// Per-node layout-dirty state, in exactly [`RenderRoot::inspect`]'s
    /// pre-order and with its ids — read-only, for tooling and tests. See
    /// [`WidgetTree::inspect_layout_dirty`].
    pub fn inspect_layout_dirty(&self) -> Vec<(WidgetId, bool)> {
        self.tree.inspect_layout_dirty()
    }

    /// Per-node paint-dirty state, read-only — see
    /// [`WidgetTree::inspect_paint_dirty`].
    pub fn inspect_paint_dirty(&self) -> Vec<(WidgetId, bool)> {
        self.tree.inspect_paint_dirty()
    }

    /// Run the build closure, then build (first call) or rebuild (subsequent calls)
    /// the root widget, returning what changed.
    ///
    /// the build closure is expected to be cheap and re-entrant: it is
    /// re-run in full every rebuild.
    ///
    /// # Deferred-callback flush
    ///
    /// The view diff itself is state-free (`rebuild_view` below takes no
    /// `State`), so a widget applying a structural op there — the navigator
    /// draining its queued `push`/`pop` is the shipped case — cannot run an app
    /// callback that needs `&mut State`. It instead queues the callback and calls
    /// [`mark_pending_result_flush`](crate::event::mark_pending_result_flush);
    /// this method drains that flag and dispatches an
    /// [`InputEvent::Housekeeping`] broadcast through the ordinary
    /// [`event`](RenderRoot::event) plumbing, where `state` *is* in scope. This
    /// is the only unconditional per-frame pass that holds `&mut State`, which is
    /// why the dispatch lives here and not in a shell (flushing on the next
    /// real input meant waiting seconds for a touch, or forever when the next
    /// touch went to chrome outside the navigator).
    ///
    /// A flushed callback mutates `State`, so the view built before it ran is
    /// stale — the build closure + `rebuild_view` cycle therefore re-runs after
    /// each flush, and the same frame shows the result. Results can queue further
    /// nav ops, so the loop is **bounded**; past the cap the flag is left standing
    /// and one more frame is requested rather than spinning (see
    /// `MAX_PENDING_RESULT_FLUSH_PASSES`, this module's private cap constant).
    ///
    /// The broadcast's [`EventOutcome`](crate::event::EventOutcome) is propagated, not discarded: a
    /// `needs_redraw` coming back from the dispatch folds into this rebuild's
    /// [`ChangeFlags::PAINT`] and the deferred frame request, so a callback
    /// whose only effect is [`EventCtx::request_redraw`](crate::event::EventCtx::request_redraw)
    /// — invisible to the re-diff, since no view-visible state changed — still
    /// wakes both the mobile frame gate and the desktop `Wait` loop.
    pub fn rebuild(
        &mut self,
        build: &mut impl FnMut(&mut State) -> V,
        state: &mut State,
    ) -> ChangeFlags {
        // Republish this root's focus session before the diff runs: a pod
        // severed by it compares its own stamp against the channel from its
        // destructor, and the channel mirrors one root at a time.
        self.publish_focus_session();
        let view = build(state);
        let mut flags = self.rebuild_view(view);

        // Deferred-callback convergence loop (see the method doc). Each pass:
        // drain the flag, run the queued callbacks against real state, then
        // re-diff so this frame reflects them.
        let mut passes = 0usize;
        while crate::event::take_pending_result_flush() {
            if passes >= MAX_PENDING_RESULT_FLUSH_PASSES {
                // Cap reached. Put the flag back — the work is still owed — and
                // ask for one more frame instead of spinning inside this one.
                // `pending |= PAINT` is what the mobile frame gate reads
                // (`has_pending_change_flags`); `deferred_frame` is what surfaces
                // on the next `paint` as `needs_frame`, which is how the desktop
                // `ControlFlow::Wait` loop learns to wake.
                crate::event::mark_pending_result_flush();
                flags |= ChangeFlags::PAINT;
                self.deferred_frame = true;
                break;
            }
            // The dispatch's own outcome is load-bearing, not noise: a flushed
            // callback whose *only* effect is `EventCtx::request_redraw` (no
            // signal write, no state the next build-closure run reads) leaves the
            // re-diff below reporting `ChangeFlags::NONE`, so nothing else in
            // this method would ever mark the frame dirty and the requested
            // redraw would be dropped on the floor. Fold it into exactly the
            // wake the cap branch above raises: `PAINT` reaches `self.pending`,
            // which is what the mobile frame gate reads
            // (`has_pending_change_flags`), and `deferred_frame` surfaces on the
            // next `paint` as `needs_frame`, which is how the desktop
            // `ControlFlow::Wait` loop learns to schedule a frame. Both
            // Housekeeping producers need it (a navigator pop-result callback
            // and `frust-widgets`' gesture long-press latch), and without it a
            // redraw-only effect waits for whatever input happens to arrive
            // next — exactly the failure this mechanism exists to remove.
            //
            // Non-empty flags also bump the semantics generation below, which
            // is correct: the callback just mutated real `State` through a live
            // `EventCtx`, so the accessibility tree may genuinely have changed,
            // and every other paint-class path here bumps it the same way (a
            // spurious bump costs one recompute of an unchanged tree, a missed
            // one strands a stale tree).
            let outcome = self.event(state, &InputEvent::Housekeeping);
            if outcome.needs_redraw {
                flags |= ChangeFlags::PAINT;
                self.deferred_frame = true;
            }
            let view = build(state);
            flags |= self.rebuild_view(view);
            passes += 1;
        }

        // Generic-unmount focus release. A reconciler that tears down (or
        // type-swaps, or clears the `focused` flag of) a child pod holding the
        // recorded focus path *on the live focus chain* has severed that path,
        // but runs over a `BuildCtx` with no `RenderRoot` in scope — so it raises
        // `mark_focus_orphaned` and this drain performs the release the
        // reconciler could not. "On the live chain" is what `rebuild_view`'s seed
        // buys: a mark means a live session lost its owner, never that some stale
        // flag deep in an already-blurred branch went away (see
        // `mark_focus_orphaned`). Without it the root's mirror stays standing over
        // a widget that no longer exists: `is_focus_active()` keeps reporting
        // true and `ime_state()` keeps handing the shell a surface for a dead
        // field, self-correcting only on the next event pass — which never
        // arrives on a screen the user has stopped touching (the pop-into-idle
        // case this whole seam exists for).
        //
        // Drained *after* the flush loop so one release covers every pass: a
        // flushed callback that navigates re-diffs, and either diff may orphan
        // the focus. `Housekeeping` claims no focus of its own (its root arm is
        // inert), so nothing the loop dispatched can be undone here.
        //
        // The release marks no `ChangeFlags` of its own: the structural change
        // that severed the path already flagged `LAYOUT | PAINT`, and the
        // generation bump is what wakes the mobile frame gate's
        // `focus_or_ime_changed` edge for the one repaint the release needs.
        if crate::event::take_focus_orphaned() {
            self.release_focus_session();
        }

        // Generic-unmount hover release, the same shape one channel over: a diff
        // that dropped the `ChildPod` holding the live hover link has severed a
        // path the epoch mechanism cannot strand, because stranding needs a hover
        // pass and the dead claimant will never see another one. Without this the
        // mirror stands over a widget that no longer exists — `is_hover_active()`
        // reporting a link nothing holds — and every surviving ancestor of the
        // claimant keeps painting hover chrome off its own still-matching stamp
        // until some later `Move` re-derives, which never comes on a pointer the
        // user has stopped moving.
        //
        // The mark is raised by the pod's destructor rather than by the
        // reconcilers (the stamp has no setter for a container to cooperate
        // through — see `mark_hover_orphaned`), which is what makes this cover
        // every removal route, including hand-rolled containers outside this
        // workspace. That reach is also why the mark is qualified by
        // `root_identity`: a destructor fires whenever a pod happens to die, so
        // an unqualified mark could be a second root's on this thread. Drained
        // after the flush loop for the focus release's reason: any pass of the
        // loop may re-diff, and one end covers them all.
        //
        // Unlike that release this one flags `PAINT` of its own. The reconciler
        // that dropped the claimant usually reported `LAYOUT | PAINT` already,
        // but "usually" is not a contract this drain can rest on: the destructor
        // route deliberately covers containers outside this workspace (that is
        // its whole reason for existing), and one of those can drop a pod while
        // reporting whatever flags it likes. Ending a hover always changes what
        // paints, so the correction states its own need for the frame it rides
        // on — idempotent where the reconciler already said so.
        if crate::event::take_hover_orphaned(self.root_identity) && self.hover_active {
            self.end_hover_link();
            flags |= ChangeFlags::PAINT;
        }

        self.pending |= flags;
        // A rebuild that changed layout/paint could have changed the semantics
        // tree (added/removed/relabelled nodes); bump the dirty gate a shell polls
        // via `semantics_if_changed`.
        if !flags.is_empty() {
            self.semantics_gen = self.semantics_gen.wrapping_add(1);
        }
        flags
    }

    /// The rebuild body, split out so [`RenderRoot::rebuild`] can accumulate the
    /// result into [`RenderRoot::pending`] in one place.
    ///
    /// # Seeding the diff's focus chain
    ///
    /// The root is where the effective focus chain ([`BuildCtx::has_focus`])
    /// starts: the root widget sits in no `ChildPod`, so its "link above" is the
    /// root's own session mirror. A reconciler deep in the diff ANDs its pod's
    /// `focused` flag onto this seed and marks an orphan only if the whole chain
    /// holds — which is why the seed is "is there a session to lose" rather than
    /// `focus_active` alone: an active surface parked without the flag is still a
    /// live session `release_focus_session` would move. With neither set there is
    /// nothing to release, so the seed is `false` and the diff marks nothing.
    fn rebuild_view(&mut self, view: V) -> ChangeFlags {
        // Read before the `&mut self.next_id` borrow below (disjoint fields, but
        // spelled out for the reader).
        let session_live = self.focus_active || self.ime_state.is_some();
        match (self.root_id, self.prev_view.take()) {
            // Reconcile against the previous view of the same type.
            (Some(root_id), Some(prev)) => {
                let mut ctx = BuildCtx::new(&mut self.next_id);
                ctx.set_has_focus(session_live);
                let flags = {
                    let pod = self
                        .tree
                        .pod_mut(root_id)
                        .expect("root pod present when root_id is set");
                    let element = pod
                        .widget_mut()
                        .downcast_mut::<V::Element>()
                        .expect("root widget type matches its originating view");
                    view.rebuild(&prev, element, &mut ctx)
                };
                if let Some(pod) = self.tree.pod_mut(root_id) {
                    pod.merge_flags(flags);
                }
                self.prev_view = Some(view);
                flags
            }
            // First build: materialise the widget and insert it as the root.
            _ => {
                let mut ctx = BuildCtx::new(&mut self.next_id);
                // A first build tears nothing down, so the seed is moot — set it
                // anyway so the rule is "the root always seeds the chain", with no
                // arm exempt.
                ctx.set_has_focus(session_live);
                let id = ctx.alloc_id();
                let element = view.build(&mut ctx);
                // `new_typed` boxes the element exactly like `new` would, and
                // additionally records `V::Element`'s type name for
                // introspection — the concrete type is only nameable here.
                let pod = WidgetPod::new_typed(id, element);
                let root_id = self.tree.insert_root(pod);
                self.root_id = Some(root_id);
                self.prev_view = Some(view);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }
}
