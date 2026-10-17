use std::any::Any;

use kurbo::Point;

use super::{OverlayRoute, RenderRoot, advance_focus_epoch};
use crate::event::{
    ContactFrame, ContactPass, EventCtx, EventOutcome, EventResult, FileDropEvent, FileDropPhase,
    InputEvent, OverlayEvent, OverlayEventKind, PointerButton, PointerId, PointerPhase,
    RequestPass,
};
use crate::overlay::{OutsideTap, OverlayInput};
use crate::view::{ChangeFlags, View};

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Deliver an input event to the widget tree, returning what happened.
    ///
    /// Builds a root [`EventCtx`] over the (type-erased) `state`, dispatches to
    /// the root widget — which routes the event down through its container
    /// children — and folds the result into an [`EventOutcome`]. The outcome's
    /// `needs_redraw` is set whenever a widget consumed the event or explicitly
    /// requested a redraw; the shell turns that into a `window.request_redraw()`.
    ///
    /// Root capture bookkeeping mirrors the per-container `active`-child model: a
    /// `Down` whose dispatch requested capture marks a gesture in flight and
    /// latches the contact that sent it as the gesture's claimant; the
    /// claimant's `Up` and `Cancel` release it (never a window-leave, and never
    /// another contact's release).
    ///
    /// Pointer contacts are gated **first**, before anything else runs: an
    /// [`InputEvent::PointerContact`] is unwrapped into the plain
    /// [`InputEvent::Pointer`] every widget matches on, with
    /// [`EventCtx::pointer_id`](crate::event::EventCtx::pointer_id) reporting its
    /// id, and a contact the multi-contact contract does not route — an
    /// additional contact with nothing captured, or one the captor did not opt
    /// into — is dropped here with an empty outcome (see that variant's
    /// *Multi-contact contract*). A bare `InputEvent::Pointer` is the mouse.
    ///
    /// Root hover bookkeeping is the third recorded path, and the one this pass
    /// *derives* rather than merely mirrors: an **uncaptured** `Move` opens a hover
    /// pass (widgets on the hit-tested path may claim it — see
    /// [`EventCtx::claim_hover`](crate::event::EventCtx::claim_hover)), a
    /// `Down`/`Up`/`Cancel` ends whatever hover stood, and every other event leaves
    /// it alone. There is nothing to release and no generation to bump: the epoch
    /// advance strands the previous claimant's path by itself, and the outcome's
    /// `needs_redraw` carries the one repaint **no widget can ask for** — a hover
    /// that ended with nothing taking it. A hover that *begins* or *moves from one
    /// claimant to another* is repainted by the new claimant's own change-gated
    /// `request_redraw`, which is why keeping an internal hover flag is part of the
    /// consumer contract rather than an optimization (see `claim_hover`).
    /// **No shell change is required for hover** — the desktop shell already
    /// dispatches a `Move` on every cursor move.
    ///
    /// The cursor is hover's sibling channel and the fourth thing this pass
    /// resolves: any pointer `Move` (captured included) re-resolves
    /// [`RenderRoot::cursor`] from the pass's last
    /// [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor), defaulting to
    /// [`CursorIcon::Default`](crate::event::CursorIcon::Default) when nothing asked. It deliberately does **not**
    /// fold into the outcome's `needs_redraw`: applying a cursor is a platform
    /// call a desktop shell makes straight after this pass returns, with no frame
    /// involved, and folding it in would repaint the tree on every hover move.
    ///
    /// The **clipboard channel** rides the same bracket and is the fifth thing
    /// this pass resolves: whatever the dispatch asked for through
    /// [`EventCtx::write_clipboard`](crate::event::EventCtx::write_clipboard) and
    /// [`EventCtx::request_paste`](crate::event::EventCtx::request_paste) lands in
    /// [`RenderRoot::take_clipboard_write`] / [`RenderRoot::take_paste_request`],
    /// which a shell drains immediately after this returns, beside
    /// [`RenderRoot::cursor`] and [`RenderRoot::ime_state`]. Unlike the cursor,
    /// both commit on **every** pass rather than on a pointer `Move` alone — a
    /// copy can be answered from a key chord, a context-menu tap, or an
    /// [`InputEvent::EditCommand`] — and both are one-shot drains rather than
    /// standing levels. Like the cursor, neither folds into `needs_redraw`:
    /// talking to the host clipboard paints nothing (a `Cut` that mutates the
    /// document asks for its own redraw, for the mutation).
    ///
    /// # Reentrancy
    ///
    /// This pass **never rebuilds or repaints**. Event handlers mutate `state`
    /// synchronously through the context; the shell is expected to run a single
    /// [`RenderRoot::rebuild`] (then layout/paint) *after* the event pass returns,
    /// driven by the outcome. Rebuilding re-entrantly here would invalidate the
    /// widget references the dispatch still holds and turn the event→state→view
    /// feedback into recursion.
    ///
    /// [`RenderRoot::rebuild`] calls this itself with
    /// [`InputEvent::Housekeeping`] to flush deferred state-bearing callbacks.
    /// That is *sequential*, not re-entrant — the dispatch fully returns before
    /// the next diff starts — so the rule above is intact.
    ///
    /// # File-drag follow-up
    ///
    /// Every [`InputEvent::FileDrop`] whose phase is [`FileDropPhase::Drop`]
    /// or [`FileDropPhase::Cancel`] is followed, within this same call and
    /// whether or not its hit-tested dispatch was handled, by a
    /// [`FileDropPhase::Ended`] broadcast to the whole tree; the returned
    /// outcome folds both. A `Hover` and an `Ended` dispatched directly get
    /// no follow-up. See [`FileDropEvent`]'s *Broadcast follow-up* for why.
    pub fn event(&mut self, state: &mut State, event: &InputEvent) -> EventOutcome {
        let Some(root_id) = self.root_id else {
            return EventOutcome::default();
        };

        // The multi-contact gate (`InputEvent::PointerContact`'s contract). A
        // contact is unwrapped into the plain `Pointer` every widget matches on,
        // so everything below sees one pointer shape; a bare `Pointer` is the
        // mouse. Any other event dispatches under the contact the enclosing pass
        // carries (the overlay pre-pass re-enters this method with a broadcast
        // that must keep its gesture's id), or the mouse at the top level.
        let unwrapped;
        let (event, pointer_id) = match event {
            InputEvent::PointerContact {
                pointer_id,
                event: pointer,
            } => {
                unwrapped = InputEvent::Pointer(*pointer);
                (&unwrapped, *pointer_id)
            }
            InputEvent::Pointer(_) => (event, PointerId::MOUSE),
            _ => (event, crate::event::current_pointer_id()),
        };
        let secondary = if matches!(event, InputEvent::Pointer(_)) {
            match self.contact_route(pointer_id) {
                Some(secondary) => secondary,
                // Rule (b), or rule (c) for a captor that did not opt in: the
                // contact reaches nothing and moves no root state.
                None => return EventOutcome::default(),
            }
        } else {
            false
        };
        // Published for the whole dispatch: `EventCtx::new` seeds
        // `pointer_id()` from it (so the id survives a component boundary),
        // `capture_contacts` records its opt-in into it, and `ChildPod::set_active`
        // reads its `secondary` mark to keep every container's active link on
        // the claimant. Restored on drop, so a nested pass scopes its own.
        let contact_pass = ContactPass::enter(pointer_id, secondary);

        // The overlay pre-pass runs before every other thing this method does —
        // before the hover derivation, before the cursor bracket, before the
        // dispatch — because a pointer over a floated surface must reach none of
        // them: not the main tree's hit test, not its hover pass, and above all
        // not its blur rule. See `route_overlay`.
        let carried = match self.route_overlay(state, event) {
            OverlayRoute::Consumed(outcome) => return outcome,
            OverlayRoute::Continue(outcome) => outcome,
        };

        // Open a candidate focus session for this dispatch, BEFORE anything is
        // routed. A claim recorded below is stamped with this new epoch, which
        // nothing older carries — so the branch the session is leaving is
        // stranded by arithmetic rather than by a clearing sweep that would have
        // to visit it, and a floated pod no container owns is retired on exactly
        // the same terms as a field in the tree.
        //
        // Candidate, not committed: `self.focus_epoch` is left alone and the
        // pass settles it below, because whether this dispatch recorded anything
        // is only known once it has run. A `Move` over a focused field, a
        // housekeeping broadcast, a press inside a surface that claims nothing —
        // none of those may disturb a standing link.
        //
        // Both epochs are published because a claim recorded on the way back up
        // has to be observable to the container still unwinding around it (a
        // blur sweep asking which child kept focus, a portal noticing its surface
        // took the session) while a link recorded *before* this dispatch still
        // has to read live. See `crate::widget::set_live_focus_session`.
        //
        // After `route_overlay`, deliberately: an event that belongs to a floated
        // surface is re-dispatched through the front door and opens its own
        // candidate session there, and this call returns that outcome untouched.
        let live_focus_epoch = self.focus_epoch;
        let candidate_focus_epoch = advance_focus_epoch(live_focus_epoch);
        crate::widget::set_live_focus_session(
            self.root_identity,
            live_focus_epoch,
            candidate_focus_epoch,
        );
        let focus_active_before = self.focus_active;

        let Some(pod) = self.tree.pod_mut(root_id) else {
            // Nothing to dispatch into, but an outside-tap notification may
            // already have produced an outcome; returning it rather than the
            // default keeps that redraw.
            self.publish_focus_session();
            return carried;
        };

        // Hover is derived per **uncaptured** pointer `Move`: that pass, and only
        // that pass, may record a claim, so a captured drag can never paint hover
        // under the pointer. Every other pointer phase — `Down`, `Up`, `Cancel` —
        // is an epoch-advancing pass that *ends* whatever hover stood without
        // opening a new one: a press is not a hover, a touch `Down` must not
        // inherit one, and a lift is the only signal a touch contact leaving the
        // screen ever produces (no further `Move` follows it, so a tint claimed
        // during an uncaptured touch drag would otherwise stand indefinitely).
        // Ending on `Up` costs a mouse the hover tint between a click's release
        // and its next motion — the same standing-until-next-move class as the
        // press-then-hold-still gap, and traded deliberately for a touch link that
        // cannot outlive the finger (`docs/LIMITATIONS.md`'s
        // `hover-window-leave-standing`). A scroll, key, IME, or the housekeeping
        // broadcast leaves a live hover exactly as it was.
        //
        // A non-claimant contact delivered down a live capture (`secondary`) is
        // neither: it is not a gesture of its own, so it leaves hover exactly as
        // the claimant's gesture had it.
        let hover_pass = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move)
            && self.capture_claimant.is_none();
        let hover_ends = !secondary
            && matches!(
                event,
                InputEvent::Pointer(p)
                    if matches!(
                        p.phase,
                        PointerPhase::Down | PointerPhase::Up | PointerPhase::Cancel
                    )
            );
        let hover_epoch = self.hover_epoch;
        let hover_was_active = self.hover_active;
        let root_identity = self.root_identity;

        // The cursor pass is hover's pass widened by one case: **any** pointer
        // `Move`, captured included, because a captured `Move` routes only to the
        // capturing widget and that is exactly how a drag keeps its own cursor
        // while the pointer is outside its bounds. Every other pass leaves the
        // resolved cursor standing — notably `Down`/`Up`, whose handlers have no
        // reason to restate a cursor and whose reset would blink the shape back to
        // `Default` for the length of a click.
        //
        // The slot is cleared here rather than trusted to be empty: a `set_cursor`
        // from a dispatch no root drove (a reconciler's synthesized `Cancel`)
        // must not leak into this pass's resolution. The clear and the drain below
        // are one bracket (`CursorPass`) rather than two bare calls, so a dispatch
        // that re-entered this method could not silently eat the enclosing pass's
        // request — see that guard.
        //
        // The claimant's moves only: a second finger moving under a captured
        // drag says nothing about the drag's own cursor.
        let cursor_pass =
            !secondary && matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move);
        // The same bracket carries the clipboard channel (a write and a paste
        // request), which differs only in when it commits: every pass, not the
        // pointer-move subset, since a copy can be answered from a key chord or an
        // `EditCommand` that never moved a pointer. See `RequestPass`.
        let request_slot = RequestPass::enter();

        // Whether the root widget itself holds the live opt-in (rather than a
        // pod below it), and whether it opted in during this dispatch.
        let contacts_captor_is_root = self.capture_claimant.is_some()
            && self.capture_contacts
            && self.contacts_captor_is_root;
        let root_opted_in;

        // The claimant pod this dispatch records, if any (see `hover_link`).
        crate::widget::clear_hover_claimant();
        let (handled, needs_redraw, captured, hover_claimed, focus_req, focus_rel, ime) = {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, pod.origin(), pod.size());
            // Seed the root widget's focus flag so a leaf-root editable that holds
            // focus can observe `has_focus()`; deeper focus is threaded per-pod.
            ctx.set_has_focus(self.focus_active);
            // Same for the hover link, plus the live epoch every pod compares its
            // stamp against and the eligibility gate that decides whether a claim
            // is recordable at all this pass.
            ctx.set_hovered(hover_was_active);
            ctx.set_hover_epoch(hover_epoch);
            ctx.set_hover_eligible(hover_pass);
            // Stamped onto whichever pod records a claim, so that pod's
            // destructor can tell this root's link from another root's
            // identically-numbered epoch (see `root_identity`).
            ctx.set_hover_root(root_identity);
            // The root widget's own contact frame: attributes a
            // `capture_contacts` made by the root widget itself (not by a pod
            // below it) to the root, and marks the dispatch as running under the
            // live opt-in's holder when the root widget is that holder.
            let frame = ContactFrame::enter(contacts_captor_is_root);
            let result = if secondary && !contacts_captor_is_root {
                // A non-claimant contact walks the recorded active path
                // forward-only (see `ChildPod::event_child`): the root widget is
                // handed the inert carrier and the real event rides beside it,
                // so only the captor that opted in runs its pointer handling. A
                // walk that never reached a pod on the path (the root widget
                // does not forward broadcasts to its captured child) falls back
                // to the ordinary delivery.
                let widget = pod.widget_mut();
                let carrier = crate::event::secondary_walk_carrier();
                let (_, delivered) = crate::event::run_secondary_walk(event.clone(), || {
                    widget.event(&mut ctx, &carrier)
                });
                match delivered {
                    Some(result) => result,
                    None => crate::event::without_secondary_walk(|| widget.event(&mut ctx, event)),
                }
            } else {
                pod.widget_mut().event(&mut ctx, event)
            };
            root_opted_in = frame.close().0;
            let handled = matches!(result, EventResult::Handled);
            // The event fold: every dirtying channel a shell gating layout on the
            // pending flags must see (the mirror of `paint_main_tree`'s fold).
            // `EventCtx::request_layout`, and the path bit — a pod below the
            // root consumed the event or asked for a redraw, so it was marked and
            // only a pass that runs lays it out. The root widget sits in no pod,
            // so its own consumed event or redraw request is the same source.
            if ctx.needs_layout() || ctx.is_layout_path_dirty() || handled || ctx.needs_redraw() {
                self.pending |= ChangeFlags::LAYOUT;
            }
            (
                handled,
                ctx.needs_redraw() || handled,
                ctx.is_pointer_captured(),
                ctx.is_hover_claimed(),
                ctx.is_focus_requested(),
                ctx.is_focus_released(),
                ctx.take_ime_state(),
            )
        };

        // A published IME surface refreshes the stored one (persists past this
        // event, survives rebuild) until a blur clears it below. `store_ime_state`
        // bumps the focus/IME edge generation only if the surface actually moved
        // (a keystroke that changes nothing observable is not an edge).
        //
        // An **inactive** publish carries the same release intent here as it does
        // in `paint` (see that method's take path): a widget that publishes
        // `active: false` is ending the session, not describing it, so it takes
        // the full release. Reachable from this pass too — every paint-time
        // producer of an inactive surface is a container/widget whose `event` arm
        // can run first — and the two passes must not disagree about what an
        // inactive surface means. No `focus_active` guard is needed (unlike
        // `paint`, which must refuse to *resurrect* a cleared surface): releasing
        // an already-released root moves nothing and fires no edge.
        //
        // Ordering: this runs before the focus match below, so a dispatch that
        // both published an inactive surface and requested focus still ends up
        // focused — the later, more specific claim wins.
        //
        // The provenance rule `paint_overlays` applies to a paint-time publish,
        // mirrored onto this pass — the two must not disagree about who is
        // allowed to speak for the session, so a change to either belongs in
        // both.
        //
        // What the root can attribute here is bounded by how the event was
        // routed. A hit-tested or focus-routed dispatch reaches a publisher
        // through the tree, and the tree's own chain is what gated it (a
        // well-behaved editable publishes only while `has_focus`, which is now
        // stamp-gated) — so the rule for those is the liveness half alone: a
        // session must exist, or be opening in this very dispatch, for a publish
        // to describe anything. An `InputEvent::Overlay` is broadcast to the
        // whole tree and reaches every floated pod whether or not it holds
        // anything, so there the addressed surface must be the recorded owner of
        // the session, or be claiming it now — the same "the publisher holds the
        // link" question `paint_overlays` answers from the pods themselves.
        //
        // The *inactive* publish is refused on the same terms and for the same
        // reason it is at paint: ending a session is a claim about it too, and a
        // branch that owns none makes neither.
        let publish_attributable = match event {
            InputEvent::Overlay(overlay) => focus_req || self.focus_surface == Some(overlay.key),
            _ => self.focus_active || focus_req,
        };
        match ime {
            Some(_) if !publish_attributable => {}
            Some(ime) if !ime.active => {
                self.release_focus_session();
            }
            Some(ime) => self.store_ime_state(Some(ime)),
            None => {}
        }

        // Root-level capture path: a captured `Down` opens a gesture; `Up`/`Cancel`
        // close it. `Move` leaves the flag untouched so it survives the drag.
        //
        // Root-level focus path (the capture mirror): a `Down` that requested
        // focus opens the focus session; a `Down` that did not is a
        // blur-on-outside-tap and closes it (the per-container `focused` flags are
        // cleared by the routing helpers). Key/Ime/Scroll only adjust focus if the
        // dispatch explicitly requested or released it.
        //
        // Every arm mutates through `set_focus_active`/`store_ime_state` or the
        // paired `release_focus_session`, the change-guarded writers that own the
        // focus/IME edge generation: a `Down` on already-blurred chrome (the
        // commonest event of all) writes the same values back and must therefore
        // NOT fire an edge.
        //
        // Whether an arm below actually *honoured* a focus claim — the one thing
        // that makes the candidate epoch opened above the real one. Deliberately
        // not `focus_req` itself: the housekeeping arm ignores a claim by
        // contract, and a claim it ignored must leave the standing session (and
        // therefore the standing epoch) exactly where it was.
        let mut focus_claim_honoured = false;
        match event {
            // A non-claimant contact delivered down a live capture (rule (c) of
            // `InputEvent::PointerContact`'s contract): not a gesture of its own,
            // so it opens, moves and releases no capture and never blurs — its
            // `Up` must not end the claimant's gesture, and its `Down` is not a
            // tap outside anything. An explicit focus request or release from
            // its handler is honoured exactly as the scroll arm below honours
            // one.
            InputEvent::Pointer(_) if secondary => {
                if focus_req {
                    focus_claim_honoured = true;
                    self.set_focus_active(true);
                }
                if focus_rel {
                    self.release_focus_session();
                }
            }
            // Never reached: the gate at the top unwrapped every contact into a
            // plain `Pointer`, and routed it by the arms around this one.
            InputEvent::PointerContact { .. } => {}
            InputEvent::Pointer(pointer) => match pointer.phase {
                PointerPhase::Down => {
                    if captured {
                        // Rule (a): the contact whose `Down` captured is the
                        // claimant, and only its release ends the gesture. The
                        // opt-in is read from the pass rather than the root
                        // context's bubble so a captor below a component
                        // boundary (which mirrors capture but not the opt-in)
                        // is still heard.
                        self.capture_claimant = Some(pointer_id);
                        self.capture_contacts = contact_pass.contacts_requested();
                        self.contacts_captor_is_root = root_opted_in;
                    }
                    if focus_req {
                        focus_claim_honoured = true;
                        self.set_focus_active(true);
                        // A hit-tested press reached the claimant through the
                        // containers, so the session is the main tree's — the one
                        // attribution the event pass can make without help, and
                        // what makes the next paint seed the tree's links again
                        // the moment focus comes back to it (see `focus_surface`).
                        self.focus_surface = None;
                    } else {
                        // Blur: no widget on the tapped path took focus. The
                        // canonical release — flag and surface drop together, as
                        // one edge (see `release_focus_session_in`).
                        self.release_focus_session();
                    }
                }
                // Only ever the claimant's (or an uncaptured contact's) release:
                // any other contact's was routed to the `secondary` arm above or
                // dropped at the gate.
                PointerPhase::Up | PointerPhase::Cancel => {
                    self.capture_claimant = None;
                    self.capture_contacts = false;
                    self.contacts_captor_is_root = false;
                }
                PointerPhase::Move => {}
            },
            // A file drop is hit-tested exactly like `Scroll`/`Scale` above —
            // no capture, no claimant bookkeeping — so it joins their bucket
            // rather than getting one of its own: a target that calls
            // `request_focus`/`release_focus` from its `FileDrop` handler is
            // honoured on the same terms a scroll's would be, even though no
            // shipped drop target does today. Its `Ended` broadcast follow-up
            // lands here too: like an overlay event it is a broadcast that
            // stands for a real user gesture, so an explicit request is
            // honoured and nothing blurs (only a pointer `Down` blurs).
            InputEvent::Scroll { .. }
            | InputEvent::Scale(_)
            | InputEvent::FileDrop(_)
            | InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_) => {
                if focus_req {
                    // The candidate epoch opened above becomes this claim's, so
                    // a focus move driven from the keyboard retires the branch it
                    // supersedes on exactly the terms a press does — including a
                    // floated surface's link, which this arm could otherwise
                    // never reach. `focus_surface` is deliberately left alone:
                    // which branch a focus-routed claim came from is not
                    // something the root can attribute, so the next paint
                    // re-resolves it from the pods' own live links.
                    focus_claim_honoured = true;
                    self.set_focus_active(true);
                }
                if focus_rel {
                    self.release_focus_session();
                }
            }
            // A broadcast is not user input: it opens no gesture, claims no
            // focus, and blurs nothing. Deliberately inert here — a housekeeping
            // pass that moved the root's capture/focus bookkeeping would change
            // what the *next* real event does, which is exactly what this
            // mechanism must not do (see `InputEvent::Housekeeping`).
            //
            // The one thing a broadcast *can* still move is the IME surface, via
            // the publish handling above (which is pass-agnostic by design, and
            // was before this arm existed): a widget that publishes while
            // flushing has said something about its session either way, and an
            // inactive publish releases it. No shipped widget does — `TextInput`
            // ignores a broadcast outright, and both cleared-surface publishers
            // are paint-time — so this is a contract note, not live behavior.
            InputEvent::Housekeeping => {}
            // A floated surface's own input: a broadcast at the root, but a real
            // user gesture underneath, so it moves *some* of what a hit-tested
            // event moves and deliberately none of the rest.
            InputEvent::Overlay(overlay) => {
                // Honoured: a text field inside a popover may claim focus, and the
                // session it opens is an ordinary one.
                //
                // What it must not do is stack on top of the session it
                // supersedes. A hit-tested press has the containers' blur sweep
                // under it, which clears every focused child but the one the
                // press kept; a press inside a floated surface reaches no
                // container's hit test, so nothing retires the chain the claim
                // replaces and two branches go on believing they are focused —
                // one of them still describing its own IME surface to the shell.
                // The root retires it in the two places it has standing to: here,
                // when its record already names a *different* surface as the
                // owner, and in `paint_overlays`, which resolves that record from
                // the pods themselves and stops seeding the main tree's links the
                // pass after the session leaves it.
                //
                // The other repair — running the blur sweep for this event, with
                // the claiming surface's owner kept — was not taken: that sweep
                // belongs to the containers, which run it over the children they
                // hold when a pointer `Down` passes through them. The root has no
                // mutable route to a `focused` link below its own pod, so from
                // here it is not a sweep at all but a change to how every
                // container routes a broadcast.
                //
                // The record is deliberately not *written* here: an overlay-pass
                // focus request cannot be attributed at the root (see
                // `focus_surface`) — a field in the tree re-claiming its own
                // session through a floated toolbar raises exactly the flag an
                // editable inside the surface raises to take it away.
                if focus_req {
                    if self.focus_surface.is_some_and(|owner| owner != overlay.key) {
                        // A session another surface owns, which *is* attributable:
                        // retire it whole rather than let the new claim inherit
                        // the surface the old one published.
                        self.release_focus_session();
                    }
                    focus_claim_honoured = true;
                    self.set_focus_active(true);
                }
                // Honoured: an **explicit** `EventCtx::release_focus` from inside
                // the surface ends the session it asked to end — the same rule the
                // `Scroll`/`Key`/`Ime` arm applies.
                if focus_rel {
                    self.release_focus_session();
                }
                // NEVER the blur branch. A `Down` that claims no focus blurs the
                // tree only when it was hit-tested *in* the tree; a press inside a
                // floated surface is the one press that must not, or tapping a
                // selection toolbar would drop the very selection the toolbar acts
                // on — which is the whole reason these two are routed apart.
                //
                // Nor does it advance the hover epoch: `hover_pass`/`hover_ends`
                // above match `InputEvent::Pointer` only, so a live hover in the
                // main tree survives an overlay pass by construction rather than by
                // a check here.
                //
                // A capture requested from inside the surface IS honoured, exactly
                // as a hit-tested `Down`'s is: the gesture then owns every
                // follow-up, and because a live capture short-circuits the overlay
                // pre-pass those follow-ups arrive as ordinary `Pointer` events
                // routed by the capture path — which is what lets a drag begun
                // inside a surface continue outside it, and what closes it on the
                // `Up` through the arm above.
                //
                // Whatever phase claimed it, not `Down` alone. The pods record
                // their own `active` path on any phase, so a capture claimed on a
                // `Move` left the surface latched and the root's mirror clear:
                // the pre-pass kept hit-testing, the follow-ups kept missing the
                // latched surface, and the `Up` that would have released it never
                // arrived — leaving the surface free to divert every later
                // pointer event its owner is reached by. Mirroring the claim on
                // any phase is what routes those follow-ups, and that `Up`, back
                // down the capture path to the surface that opened it.
                //
                // The claimant is the contact this overlay event was routed for —
                // the pre-pass re-enters this method under the gesture's own
                // contact pass, so `pointer_id` is that gesture's id.
                if captured {
                    self.capture_claimant = Some(pointer_id);
                    self.capture_contacts = contact_pass.contacts_requested();
                    self.contacts_captor_is_root = root_opted_in;
                }
                // ...and released on the phase that ends the gesture, in the
                // same door it was claimed through. The claim is mirrored on any
                // phase (see above), so the release has to be too: an `Up` or
                // `Cancel` that arrives *as an overlay event* — which is what
                // happens when the surface was not holding the capture when the
                // gesture began, so the pre-pass kept hit-testing it — would
                // otherwise leave the root's latch standing on a gesture that is
                // over, and a standing latch short-circuits the pre-pass for
                // every floated surface until some unrelated pointer release
                // happens along.
                //
                // A gesture whose claim *did* short-circuit the pre-pass ends on
                // the ordinary pointer arm above instead, because that is the
                // door its follow-ups come in by; the pod's own `active` link is
                // released by whichever of the two the release arrived through
                // (see `frust-widgets`' `OverlaySlot::forward`).
                if let OverlayEventKind::Pointer(pointer) = &overlay.kind
                    && matches!(pointer.phase, PointerPhase::Up | PointerPhase::Cancel)
                    && self
                        .capture_claimant
                        .is_none_or(|claimant| claimant == pointer_id)
                {
                    self.capture_claimant = None;
                    self.capture_contacts = false;
                    self.contacts_captor_is_root = false;
                }
            }
        }

        // A takeover during the claimant's own dispatch: a container cancelled
        // the widget that opted into the gesture's other contacts and released
        // it from the active path (`EventCtx::release_captured_child`). The
        // opt-in goes with it — unless the container that took over opted in
        // itself afterwards, which the pass records — so the other contacts are
        // dropped from here on rather than routed to a captor that is gone. The
        // claimant latch is deliberately kept: the gesture is still the
        // claimant's, now held by the container that took it over (still on the
        // active path, as the released child's ancestor), and only the
        // claimant's own `Up`/`Cancel` ends it. Never on a non-claimant
        // contact's dispatch, in which no container can clear a recorded link
        // (`ChildPod::set_active`), so nothing could have left the path.
        if !secondary && self.capture_claimant.is_some() && contact_pass.capture_released() {
            self.capture_contacts = contact_pass.contacts_requested();
        }

        // Settle the session's identity — the one write that decides which
        // recorded links survive this dispatch, in one place, from what the
        // dispatch actually did.
        //
        // * A **live session that ended** strands everything: the chain it ran
        //   through, a floated surface's link, and anything this very pass
        //   stamped (a claim the same dispatch then released). Past the candidate,
        //   therefore, not merely onto it — this is the clearing sweep no
        //   container can be asked to run.
        // * A **claim the root honoured** commits the candidate, so the chain it
        //   stamped is the only one that reads live and every older link is
        //   stranded by arithmetic.
        // * Anything else leaves the session exactly where it was, which is what
        //   makes a `Move`, a housekeeping broadcast, or a press inside a surface
        //   that claimed nothing incapable of disturbing a standing link — and
        //   what strands a claim an arm declined to honour, since the candidate
        //   it was stamped with is then never published again.
        let session_ended = focus_active_before && !self.focus_active;
        self.focus_epoch = if session_ended {
            advance_focus_epoch(candidate_focus_epoch)
        } else if focus_claim_honoured {
            candidate_focus_epoch
        } else {
            live_focus_epoch
        };
        self.publish_focus_session();

        // Close the hover pass: advance the epoch (which strands every stamp this
        // pass did not renew, wherever in the tree it sits) and refresh the mirror.
        // A claim only counts on a `hover_pass` — an ineligible pass records none
        // anyway, but stating it here keeps the mirror true by construction rather
        // than by the eligibility gate alone.
        let hover_claimant = crate::widget::take_hover_claimant();
        if hover_pass || hover_ends {
            self.hover_epoch = self.hover_epoch.wrapping_add(1);
            self.hover_active = hover_pass && hover_claimed;
            // The link's identity, for the paint-epoch fold: the claimant pod,
            // or `0` for a claim the root widget made with no pod of its own.
            self.hover_link = self.hover_active.then(|| hover_claimant.unwrap_or(0));
            // Republish the link a dropping pod checks its stamp against, so a
            // rebuild that removes the claimant can report the severance the
            // epoch alone cannot strand (see `ChildPod`'s `Drop`). Epoch `0` while
            // nothing holds a link, which is what makes a stale stamp's drop —
            // the common case — cost one comparison and mark nothing. The
            // identity rides with it because epoch integers are per-root and
            // collide by construction (see `root_identity`).
            crate::event::set_live_hover_link(
                self.root_identity,
                if self.hover_active {
                    self.hover_epoch
                } else {
                    0
                },
            );
        }

        // Close the cursor pass: the last request of the pass wins, and its
        // absence resolves to `Default` — which is what makes the request
        // stateless (a widget that stops asking needs no clearing) and what a
        // pointer moving off every requesting widget resolves to. Drained
        // unconditionally so a request made on a non-cursor pass cannot survive
        // into the next one; only a cursor pass commits it.
        let requested = request_slot.take();
        if cursor_pass {
            self.cursor = requested.cursor.unwrap_or_default();
        }
        // The clipboard half of the same drain, committed on **every** pass: a copy
        // is answered from whatever event decoded it, and there is no
        // "clipboard pass" the way there is a cursor pass. Both fields are
        // one-shot (see their docs) — a write from this pass supersedes one still
        // standing undrained, and a request is raised but never lowered here, so a
        // shell that skipped a drain answers late rather than losing the paste.
        if let Some(text) = requested.clipboard_write {
            self.pending_clipboard_write = Some(text);
        }
        self.pending_paste_request |= requested.paste_request;
        // A hover that ended with *nothing* taking it needs one repaint no widget
        // can ask for: the pointer moved onto empty chrome (or a press/lift/cancel
        // cleared the link), so the old claimant's `event()` was never called and
        // the new state has no claimant to speak for it. Every other edge is the
        // consumer's own: a hover that *began* or *moved from one claimant to
        // another* is repainted by the new claimant's change-gated
        // `request_redraw`, and because a repaint is global, that one frame is also
        // what lets the widget losing the link drop its overlay from
        // `PaintCtx::is_hovered` (a retained page holding the loser's hovered
        // body is not drawn on it: the moved `hover_link` bumps the paint epoch,
        // see `painted_links`). This mirror is identity-free, so an A→B handoff
        // is `true`→`true` here and manufactures nothing — which is exactly why the
        // consumer's internal flag is normative (see `EventCtx::claim_hover`).
        let needs_redraw = needs_redraw || (hover_was_active && !self.hover_active);

        let mut outcome = EventOutcome {
            // Merge whatever the overlay pre-pass already produced: a
            // pass-through outside-tap notification ran before this dispatch and
            // its redraw is owed just as much as the dispatch's own.
            handled: handled || carried.handled,
            needs_redraw: needs_redraw || carried.needs_redraw,
        };

        // An OS file drag that just ended (`Drop`/`Cancel`) is followed by the
        // broadcast form, handled or not, so every widget holding state the
        // drag opened — a drop target's external session — hears the drag is
        // over: one whose release resolved over a region with no drop target
        // in it, and one whose release a *different* widget handled (two
        // independent drop-target groups the drag hovered in turn). See
        // `FileDropEvent`'s *Broadcast follow-up*. Re-entered through the
        // front door for the reason `route_overlay` re-enters: the follow-up
        // gets the same request bracket and outcome folding as any dispatch.
        // Shallow by construction: `Ended` is not one of the two phases that
        // trigger it.
        if let InputEvent::FileDrop(drop) = event
            && matches!(drop.phase, FileDropPhase::Drop | FileDropPhase::Cancel)
        {
            let ended = InputEvent::FileDrop(FileDropEvent {
                phase: FileDropPhase::Ended,
                position: Point::ZERO,
                paths: Vec::new(),
            });
            let follow_up = self.event(state, &ended);
            outcome.handled |= follow_up.handled;
            outcome.needs_redraw |= follow_up.needs_redraw;
        }
        outcome
    }

    /// Decide what one pointer contact does — the root half of
    /// [`InputEvent::PointerContact`]'s multi-contact contract.
    ///
    /// `Some(false)` routes it as the gesture's own pointer (rule (a): slot `0`
    /// with nothing captured, hit-tested; or the claimant of a live capture,
    /// down the captured path). `Some(true)` routes it as a **non-claimant**
    /// contact down a live capture's path, which happens only while the captor's
    /// opt-in stands (rule (c)): the dispatch then walks the recorded active
    /// path forward-only, so only the captor's own handler sees it (see
    /// [`crate::widget::ChildPod::event_child`]). `None` drops it: an additional
    /// contact with nothing captured (rule (b)), or one the captor did not opt
    /// into — or whose opt-in ended when a container took the gesture over from
    /// the captor ([`EventCtx::release_captured_child`]) — (rule (c)).
    fn contact_route(&self, pointer_id: PointerId) -> Option<bool> {
        match self.capture_claimant {
            None if pointer_id.slot == 0 => Some(false),
            None => None,
            Some(claimant) if claimant == pointer_id => Some(false),
            Some(_) if self.capture_contacts => Some(true),
            Some(_) => None,
        }
    }

    /// Hit-test the surfaces the last paint floated, **before** the main tree
    /// sees an uncaptured pointer, scroll, or scale — the routing half of the
    /// overlay portal (see [`crate::overlay`]).
    ///
    /// # What it does
    ///
    /// Walks [`RenderRoot::overlay_hits`] topmost-first (the `Tooltip` band before
    /// `Floating`, later registration before earlier), skipping
    /// [`OverlayInput::Transparent`] entries, and on the first rect containing the
    /// event's position re-dispatches it as
    /// [`InputEvent::Overlay`] — a broadcast carrying the owner's key and a
    /// **window-space** payload — then returns
    /// [`OverlayRoute::Consumed`]. The main tree never sees the original event.
    ///
    /// If nothing was hit and the event is a primary `Down`, every `Interactive`
    /// entry registered [`OutsideTap::Notify`] is told, topmost-first, with
    /// [`OverlayEventKind::OutsideDown`]; the press is then consumed iff any of
    /// them asked to consume it, and otherwise continues into today's dispatch.
    ///
    /// # What it deliberately does not do
    ///
    /// A **live capture short-circuits it entirely**: a gesture that has captured
    /// the pointer owns every follow-up until it ends, and re-hit-testing a drag
    /// that wandered over a floated surface would hand it to the wrong widget
    /// mid-gesture. That is also what lets a drag *begun* inside a surface
    /// continue outside it — the capture the overlay `Down` opened routes the
    /// follow-ups by the ordinary captured path.
    ///
    /// Housekeeping, `Key`, `Ime`, `EditCommand` and an overlay event already
    /// being routed pass straight through: a broadcast and a focus-routed event
    /// each reach their target with no hit test, so there is nothing here to
    /// redirect.
    ///
    /// A `FileDrop` passes straight through too, but for a different reason:
    /// it is hit-tested (see [`InputEvent::FileDrop`]), yet there is no
    /// [`OverlayEventKind`] carrier for it to ride a redirect through. A
    /// native OS drag has no floated-surface concept on the platform side —
    /// winit reports it against the window, not a particular surface — so a
    /// drop target living inside a popover is unreachable by one in v1 (see
    /// `docs/LIMITATIONS.md`). Its [`FileDropPhase::Ended`] follow-up passes
    /// through as well, as the broadcast it is.
    fn route_overlay(&mut self, state: &mut State, event: &InputEvent) -> OverlayRoute {
        if self.capture_claimant.is_some() || self.overlay_hits.is_empty() {
            return OverlayRoute::Continue(EventOutcome::default());
        }
        let (position, kind) = match event {
            // A contact is routed for its position and phase exactly like a
            // plain pointer (the root's gate has already unwrapped one, so the
            // second pattern is for totality); the re-entered dispatch below
            // keeps its id through the contact pass it runs under.
            InputEvent::Pointer(pointer) | InputEvent::PointerContact { event: pointer, .. } => {
                (pointer.position, OverlayEventKind::Pointer(*pointer))
            }
            InputEvent::Scroll { position, delta } => (
                *position,
                OverlayEventKind::Scroll {
                    position: *position,
                    delta: *delta,
                },
            ),
            InputEvent::Scale(scale) => (
                scale.focal,
                OverlayEventKind::Scale {
                    focal: scale.focal,
                    phase: scale.phase,
                    scale_delta: scale.scale_delta,
                    velocity: scale.velocity,
                },
            ),
            InputEvent::FileDrop(_)
            | InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Housekeeping
            | InputEvent::Overlay(_) => {
                return OverlayRoute::Continue(EventOutcome::default());
            }
        };

        // `overlay_hits` is in paint order, so walking it in reverse walks from
        // the surface painted last — the one the user sees on top — downward.
        // Cloned because each dispatch below needs `&mut self`; the list is one
        // small `Copy` struct per floated surface, and the clone never happens on
        // the overwhelmingly common no-overlays path (guarded above).
        let hits = self.overlay_hits.clone();
        for hit in hits.iter().rev() {
            // An `egui`-style transparent surface is painted above the app and
            // hit-tested by nothing: the pointer passes straight through to
            // whatever the main tree has underneath.
            if hit.input == OverlayInput::Transparent {
                continue;
            }
            if hit.contains(position) {
                let routed = InputEvent::Overlay(OverlayEvent {
                    key: hit.key,
                    kind: kind.clone(),
                });
                // Re-entering this method is deliberate and shallow: the routed
                // event is a broadcast, so it takes the branch above out
                // immediately and can never recurse further. Going back through
                // the front door is what gives the overlay dispatch the same
                // request bracket, focus bookkeeping and outcome folding every
                // other event gets, with one arm's worth of difference rather
                // than a second copy of the pass.
                return OverlayRoute::Consumed(self.event(state, &routed));
            }
        }

        // Nothing floated was hit. Only a **primary** press is a light-dismiss
        // signal: a secondary press is a context gesture (see
        // `docs/CODE_STANDARDS.md`'s Interaction Semantics), and a `Move`, `Up` or
        // scroll outside a surface says nothing about dismissing it.
        let dismissing = matches!(
            event,
            InputEvent::Pointer(pointer) | InputEvent::PointerContact { event: pointer, .. }
                if pointer.phase == PointerPhase::Down
                    && pointer.button == PointerButton::Primary
        );
        if !dismissing {
            return OverlayRoute::Continue(EventOutcome::default());
        }

        let mut carried = EventOutcome::default();
        let mut consumed = false;
        for hit in hits.iter().rev() {
            // A transparent surface takes no input at all, outside-taps included:
            // it is chrome the pointer does not know about.
            if hit.input == OverlayInput::Transparent {
                continue;
            }
            let OutsideTap::Notify { consume } = hit.outside_tap else {
                continue;
            };
            let routed = InputEvent::Overlay(OverlayEvent {
                key: hit.key,
                kind: OverlayEventKind::OutsideDown,
            });
            let outcome = self.event(state, &routed);
            carried.handled |= outcome.handled;
            carried.needs_redraw |= outcome.needs_redraw;
            // Every notified surface is told before any of them consumes:
            // dismissing one menu must not hide the press from a second surface
            // that also wanted to close.
            consumed |= consume;
        }
        if consumed {
            OverlayRoute::Consumed(carried)
        } else {
            OverlayRoute::Continue(carried)
        }
    }
}
