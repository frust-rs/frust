use kurbo::Point;

use super::RenderRoot;
use crate::event::{EventOutcome, InputEvent, PointerButton, PointerEvent, PointerPhase};
use crate::semantics::{ROOT_NODE_ID, SemanticsCtx, SemanticsUpdate};
use crate::view::View;

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Collect the accessibility tree for the current frame,
    /// returning a [`SemanticsUpdate`] a platform adapter (`accesskit_*`)
    /// can consume.
    ///
    /// Pull-based and stateless: the shell calls this when a platform a11y client
    /// asks for the tree (or after a change), *never* per frame — this crate owns
    /// no scheduling. Must run **after** [`RenderRoot::layout`], since node bounds
    /// come from the pods' post-layout geometry.
    ///
    /// The result is always rooted at a synthetic [`accesskit::Role::Window`]
    /// node covering the window, whose children are whatever the root widget
    /// contributed. An unbuilt tree yields a bare window node with no children.
    pub fn semantics(&self) -> SemanticsUpdate {
        let mut ctx = SemanticsCtx::new(self.window_size, self.semantics_alloc.get());
        let window = self.window_size;
        let root_pod = self.root_id.and_then(|id| self.tree.pod(id));
        // The root pod is arena-backed (not a `ChildPod`), so it caches its stable
        // base id in `root_semantics_id` rather than in a pod — assigned on first
        // pass and reused thereafter, exactly like `ChildPod::semantics_base`.
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
                    // The root pod sits at its recorded origin (ZERO today) with
                    // its laid-out size; descend into that geometry and its stable
                    // id scope, mirroring `ChildPod::semantics_child`.
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
        // Persist the allocator's high-water mark so the next pass keeps handing
        // out fresh, never-reused bases to pods that first appear later.
        self.semantics_alloc.set(ctx.next_base());
        ctx.finish(root_node)
    }

    /// The current semantics generation — bumped by every rebuild/theme swap that
    /// could have changed the accessibility tree (the semantics dirty gate).
    ///
    /// A shell records the value it last pushed and compares; see
    /// [`RenderRoot::semantics_if_changed`].
    pub fn semantics_generation(&self) -> u64 {
        self.semantics_gen
    }

    /// Pull a fresh [`SemanticsUpdate`] **only if** the semantics tree may have
    /// changed since generation `last_seen`.
    ///
    /// Returns `None` when nothing relevant changed, letting a shell skip both the
    /// tree walk and the platform `accesskit_*` push. Call it post-layout (bounds
    /// must be valid). A shell threads its stored generation in and, on `Some`,
    /// updates it from [`RenderRoot::semantics_generation`]. v1 pushes the whole
    /// tree when it does recompute (stable ids make that valid); finer-grained
    /// diffing is a later optimization.
    pub fn semantics_if_changed(&self, last_seen: u64) -> Option<SemanticsUpdate> {
        (self.semantics_gen != last_seen).then(|| self.semantics())
    }

    /// Perform a platform accessibility action, returning the same
    /// [`EventOutcome`] the synthesized input produced.
    ///
    /// A platform `accesskit_*` adapter delivers an `ActionRequest(node_id,
    /// action)`; a shell forwards it here. v1 routes actions through the **normal
    /// event path** by synthesizing pointer events at the target node's absolute
    /// bounds center (recovered from a fresh semantics pass — the id → bounds map
    /// the pass produces), so *every* fire-on-up-inside widget is operable with
    /// **zero** widget-side changes:
    ///
    /// * [`accesskit::Action::Click`] → a `Down` then an `Up` at the center,
    ///   activating any button/switch/checkbox exactly as a real tap would.
    /// * [`accesskit::Action::Focus`] → a `Down` then a synthetic `Cancel` at
    ///   the center: the `Down` claims focus for a widget that opts in on `Down`
    ///   (the recorded-focus contract), and the `Cancel` releases the capture
    ///   that same `Down` opened without touching the recorded focus path — so
    ///   the action claims focus without leaving the widget permanently
    ///   capturing every later pointer event. A widget that does not claim focus
    ///   on `Down` is unaffected, and `Cancel` never fires an on-press callback.
    /// * any other action → ignored (a no-op [`EventOutcome`]); richer actions are
    ///   deferred.
    ///
    /// An unknown `node_id` (not in the current tree) is a benign no-op. Requires
    /// a prior [`RenderRoot::layout`] so the bounds are valid. Synthetic-pointer
    /// activation cannot drive widgets that require a real drag (e.g. a slider) —
    /// an accepted v1 limitation.
    pub fn perform_accessibility_action(
        &mut self,
        state: &mut State,
        node_id: accesskit::NodeId,
        action: accesskit::Action,
    ) -> EventOutcome {
        // Recover the node's absolute bounds from a fresh semantics pass (the
        // id → absolute-bounds map this action routing needs; recomputing keeps
        // it in step with the live tree without a stored cache).
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
                // A `Down` claims focus for a widget that opts in on `Down`; a
                // synthetic `Cancel` then releases the capture that `Down` also
                // opened (mirroring a real gesture steal), leaving the recorded
                // focus path intact — `Cancel` clears both `active` and
                // `capture_claimant` while never touching `focused`/`focus_active`.
                // Without the `Cancel`, the `Down` alone would leave the widget
                // permanently capturing every subsequent pointer event.
                let down = self.event(state, &synth(PointerPhase::Down));
                let cancel = self.event(state, &synth(PointerPhase::Cancel));
                EventOutcome {
                    handled: down.handled || cancel.handled,
                    needs_redraw: down.needs_redraw || cancel.needs_redraw,
                }
            }
            // Other actions are not modelled in v1: ignore rather than guess.
            _ => EventOutcome::default(),
        }
    }
}
