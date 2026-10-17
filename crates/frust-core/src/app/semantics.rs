//! `RenderRoot`'s accessibility surface: the pull-based semantics tree, its
//! dirty gate, and platform actions routed through the normal event path.

use kurbo::Point;

use super::RenderRoot;
use crate::event::{EventOutcome, InputEvent, PointerButton, PointerEvent, PointerPhase};
use crate::semantics::{ROOT_NODE_ID, SemanticsCtx, SemanticsUpdate};
use crate::view::View;

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Collect the accessibility tree as a [`SemanticsUpdate`] for a platform
    /// `accesskit_*` adapter. Pull-based (the shell calls it on demand, never per
    /// frame) and valid only after [`RenderRoot::layout`], since bounds come from
    /// post-layout geometry. Always rooted at a synthetic window-sized
    /// [`accesskit::Role::Window`] node; an unbuilt tree yields that node alone.
    pub fn semantics(&self) -> SemanticsUpdate {
        let mut ctx = SemanticsCtx::new(self.window_size, self.semantics_alloc.get());
        let window = self.window_size;
        let root_pod = self.root_id.and_then(|id| self.tree.pod(id));
        // The root pod is no `ChildPod`, so its stable base id lives in
        // `root_semantics_id`: assigned on the first pass, reused thereafter.
        let root_widget_base = match self.root_semantics_id.get() {
            Some(id) => id,
            None => {
                let id = ctx.alloc_base();
                self.root_semantics_id.set(Some(id));
                id
            }
        };
        let root_node = ctx.push_container_with_id(
            ROOT_NODE_ID,
            accesskit::Role::Window,
            |node| {
                node.set_bounds(accesskit::Rect {
                    x0: 0.0,
                    y0: 0.0,
                    x1: window.width,
                    y1: window.height,
                });
            },
            |ctx| {
                if let Some(pod) = root_pod {
                    // Mirrors `ChildPod::semantics_child`.
                    ctx.descend_into_pod(
                        root_widget_base,
                        pod.origin().to_vec2(),
                        pod.size(),
                        |ctx| {
                            pod.widget().semantics(ctx);
                        },
                    );
                }
            },
        );
        // Persist the high-water mark so later pods never reuse a base.
        self.semantics_alloc.set(ctx.next_base());
        ctx.finish(root_node)
    }

    /// The semantics dirty gate: bumped by a rebuild that returned change flags,
    /// a theme swap, and an inset change. A shell records the value it last
    /// pushed and compares ([`RenderRoot::semantics_if_changed`]).
    pub fn semantics_generation(&self) -> u64 {
        self.semantics_gen
    }

    /// A fresh whole-tree [`SemanticsUpdate`] if the generation moved past
    /// `last_seen`, else `None` (the shell skips the walk and the push). Call it
    /// post-layout; on `Some`, store [`RenderRoot::semantics_generation`].
    pub fn semantics_if_changed(&self, last_seen: u64) -> Option<SemanticsUpdate> {
        (self.semantics_gen != last_seen).then(|| self.semantics())
    }

    /// Perform a platform accessibility action by synthesizing pointer input at
    /// the node's bounds center through [`RenderRoot::event`], returning the
    /// combined [`EventOutcome`]. Handles `Click` and `Focus` only:
    ///
    /// * [`accesskit::Action::Click`] → `Down` then `Up`, as a real tap.
    /// * [`accesskit::Action::Focus`] → `Down` then `Cancel`: the `Down` claims
    ///   focus for a widget that opts in on `Down`; the `Cancel` releases its
    ///   capture without touching focus or firing an on-press callback.
    ///
    /// Any other action, or a `node_id` not in the tree, is a no-op. Requires a
    /// prior [`RenderRoot::layout`]; a widget needing a real drag is not operable.
    pub fn perform_accessibility_action(
        &mut self,
        state: &mut State,
        node_id: accesskit::NodeId,
        action: accesskit::Action,
    ) -> EventOutcome {
        // A fresh pass keeps the bounds in step with the live tree, no cache.
        let update = self.semantics();
        let Some(bounds) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == node_id)
            .and_then(|(_, node)| node.bounds())
        else {
            return EventOutcome::default();
        };
        let center = Point::new((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
        let synth = |phase| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: center,
                button: PointerButton::Primary,
            })
        };
        match action {
            accesskit::Action::Click => {
                let down = self.event(state, &synth(PointerPhase::Down));
                let up = self.event(state, &synth(PointerPhase::Up));
                EventOutcome {
                    handled: down.handled || up.handled,
                    needs_redraw: down.needs_redraw || up.needs_redraw,
                }
            }
            accesskit::Action::Focus => {
                // `Cancel` clears `active` and `capture_claimant` but never
                // `focused`/`focus_active`; without it the widget keeps capture.
                let down = self.event(state, &synth(PointerPhase::Down));
                let cancel = self.event(state, &synth(PointerPhase::Cancel));
                EventOutcome {
                    handled: down.handled || cancel.handled,
                    needs_redraw: down.needs_redraw || cancel.needs_redraw,
                }
            }
            _ => EventOutcome::default(),
        }
    }
}
