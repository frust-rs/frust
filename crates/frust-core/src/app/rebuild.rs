//! The rebuild pass on [`RenderRoot`] — build closure, view diff, deferred-callback flush and
//! unmount drains — plus the shell-facing drains and tree inspection that read its results.

use kurbo::Rect;

use super::{MAX_PENDING_RESULT_FLUSH_PASSES, RenderRoot};
use crate::event::InputEvent;
use crate::tree::{InspectNode, WidgetPod, WidgetTree};
use crate::view::{BuildCtx, ChangeFlags, View, WidgetId};
use crate::widget::PlatformViewFrame;

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// The [`PlatformViewFrame`]s the most recent paint published, in paint order (never stale).
    pub fn platform_view_frames(&self) -> &[PlatformViewFrame] {
        &self.platform_view_frames
    }

    /// The z-shield rects the most recent paint reported, in paint order (never stale).
    pub fn input_shields(&self) -> &[Rect] {
        &self.input_shields
    }

    /// Drain the slot ids whose `platform_view` widgets were torn down since the last call, so a
    /// shell retires their native views at once. Each id is reported exactly once; a merely culled
    /// slot never appears (culling runs no `teardown`).
    pub fn take_retired_platform_views(&mut self) -> Vec<u64> {
        crate::widget::take_retired_slots()
    }

    /// Take (and clear) the dirtiness accumulated since the last call.
    pub fn take_change_flags(&mut self) -> ChangeFlags {
        let flags = self.pending;
        self.pending = ChangeFlags::NONE;
        flags
    }

    /// Non-draining peek: whether any `LAYOUT`/`PAINT` bit is pending. A frame gate reads it before
    /// deciding, so a skipped frame leaves the flags for the next running frame to drain.
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

    /// A read-only pre-order snapshot of the retained tree, container-owned pods included, for
    /// tooling: per node its id, parent, children, type name, debug label and absolute border box.
    /// Bounds are the last layout pass's (all zero before the first).
    pub fn inspect(&self) -> Vec<InspectNode> {
        self.tree.inspect()
    }

    /// Per-node layout-dirty state in [`RenderRoot::inspect`]'s order and ids, for tooling/tests.
    pub fn inspect_layout_dirty(&self) -> Vec<(WidgetId, bool)> {
        self.tree.inspect_layout_dirty()
    }

    /// Per-node paint-dirty state, read-only — see [`WidgetTree::inspect_paint_dirty`].
    pub fn inspect_paint_dirty(&self) -> Vec<(WidgetId, bool)> {
        self.tree.inspect_paint_dirty()
    }

    /// Run the build closure, then build (first call) or rebuild the root widget, returning what
    /// changed. The closure re-runs in full every rebuild, so it must be cheap.
    ///
    /// # Deferred-callback flush
    ///
    /// While a structural op has raised `mark_pending_result_flush`, each pass dispatches one
    /// [`InputEvent::Housekeeping`] broadcast (where `&mut State` is in scope) and re-diffs, capped
    /// by `MAX_PENDING_RESULT_FLUSH_PASSES`: past it the mark stays raised and one more frame is
    /// requested. A broadcast's `needs_redraw` requests that frame too.
    pub fn rebuild(
        &mut self,
        build: &mut impl FnMut(&mut State) -> V,
        state: &mut State,
    ) -> ChangeFlags {
        // First: a pod the diff severs reads this root's session from its destructor.
        self.publish_focus_session();
        let view = build(state);
        let mut flags = self.rebuild_view(view);

        let mut passes = 0usize;
        while crate::event::take_pending_result_flush() {
            if passes >= MAX_PENDING_RESULT_FLUSH_PASSES {
                // The work is still owed: re-mark it and request a frame instead of spinning.
                crate::event::mark_pending_result_flush();
                flags |= ChangeFlags::PAINT;
                self.deferred_frame = true;
                break;
            }
            // A redraw-only callback re-diffs to `NONE`, so its outcome must raise the wake itself.
            let outcome = self.event(state, &InputEvent::Housekeeping);
            if outcome.needs_redraw {
                flags |= ChangeFlags::PAINT;
                self.deferred_frame = true;
            }
            let view = build(state);
            flags |= self.rebuild_view(view);
            passes += 1;
        }

        // Generic-unmount focus release, after the loop so one release covers every pass. Because
        // `rebuild_view` seeds the chain, a mark means a live session lost its owner, never a stale
        // flag in an already-blurred branch.
        if crate::event::take_focus_orphaned() {
            self.release_focus_session();
        }

        // Generic-unmount hover release. The dying pod's destructor raises the mark (qualified by
        // `root_identity`, as another root may share the thread), so this flags `PAINT` itself: a
        // container outside this workspace may drop a pod without reporting it.
        if crate::event::take_hover_orphaned(self.root_identity) && self.hover_active {
            self.end_hover_link();
            flags |= ChangeFlags::PAINT;
        }

        self.pending |= flags;
        // Any change may have changed the semantics tree.
        if !flags.is_empty() {
            self.semantics_gen = self.semantics_gen.wrapping_add(1);
        }
        flags
    }

    /// The rebuild body. Seeds the diff's focus chain ([`BuildCtx::has_focus`]) with "is there a
    /// session to lose" (`focus_active` or a standing IME surface), so a reconciler marks an orphan
    /// only when the whole chain down to it is live.
    fn rebuild_view(&mut self, view: V) -> ChangeFlags {
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
                // Moot on a first build, but the root always seeds the chain.
                ctx.set_has_focus(session_live);
                let id = ctx.alloc_id();
                let element = view.build(&mut ctx);
                // `new_typed` also records `V::Element`'s type name for introspection.
                let pod = WidgetPod::new_typed(id, element);
                let root_id = self.tree.insert_root(pod);
                self.root_id = Some(root_id);
                self.prev_view = Some(view);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }
}
