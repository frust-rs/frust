//! `RenderRoot`'s event pass: the contact gate, the overlay route and the
//! dispatch into the root widget, with the capture, focus, hover and request
//! bookkeeping the root keeps across passes.

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
    /// Deliver an input event to the widget tree. The outcome's `handled` is the
    /// dispatch's result; `needs_redraw` is set when a widget consumed the event
    /// or asked for a redraw, and for a hover that ended with nothing taking it
    /// (the one repaint no widget can request). The pass never rebuilds or
    /// repaints: the shell runs [`RenderRoot::rebuild`] afterwards, which itself
    /// calls this sequentially with [`InputEvent::Housekeeping`].
    ///
    /// Beside the outcome it resolves the cursor ([`RenderRoot::cursor`], on a
    /// pointer `Move`), the clipboard write and paste request (every pass), the
    /// IME state and the focus epoch. A [`FileDropPhase::Drop`] or `Cancel` is
    /// followed in the same call by a [`FileDropPhase::Ended`] broadcast to the
    /// whole tree; the outcome folds both.
    pub fn event(&mut self, state: &mut State, event: &InputEvent) -> EventOutcome {
        let Some(root_id) = self.root_id else {
            return EventOutcome::default();
        };

        // Contact gate: unwrap a `PointerContact` (a bare `Pointer` is the mouse; other
        // events keep the enclosing pass's contact) and drop one the contract does not route.
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
                // Rule (b), or rule (c) for a captor that did not opt in.
                None => return EventOutcome::default(),
            }
        } else {
            false
        };
        // Published for the whole dispatch (`EventCtx::pointer_id`, the contact
        // opt-in, `ChildPod::set_active`); restored on drop.
        let contact_pass = ContactPass::enter(pointer_id, secondary);

        // Overlay route: a pointer over a floated surface reaches nothing below,
        // not even the main tree's blur rule (see `route_overlay`).
        let carried = match self.route_overlay(state, event) {
            OverlayRoute::Consumed(outcome) => return outcome,
            OverlayRoute::Continue(outcome) => outcome,
        };

        // Focus-epoch bracket: claims are stamped with a candidate epoch so older
        // links strand by arithmetic; session resolution below commits it or not.
        let live_focus_epoch = self.focus_epoch;
        let candidate_focus_epoch = advance_focus_epoch(live_focus_epoch);
        crate::widget::set_live_focus_session(
            self.root_identity,
            live_focus_epoch,
            candidate_focus_epoch,
        );
        let focus_active_before = self.focus_active;

        let Some(pod) = self.tree.pod_mut(root_id) else {
            // Keep the outcome an outside-tap notification already produced.
            self.publish_focus_session();
            return carried;
        };

        // Dispatch: only an uncaptured `Move` may claim hover; a non-`secondary`
        // `Down`/`Up`/`Cancel` ends it (`docs/LIMITATIONS.md`'s `hover-window-leave-standing`).
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

        // The cursor follows any claimant `Move`, captured included, so a drag
        // keeps its cursor outside its bounds.
        let cursor_pass =
            !secondary && matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move);
        // Opened empty, so a request from a dispatch no root drove cannot leak in.
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
            // Seed the root widget's focus and hover state; deeper pods thread their own.
            ctx.set_has_focus(self.focus_active);
            ctx.set_hovered(hover_was_active);
            ctx.set_hover_epoch(hover_epoch);
            ctx.set_hover_eligible(hover_pass);
            // Tells a dropping pod's destructor this root's link apart (see `root_identity`).
            ctx.set_hover_root(root_identity);
            // Attributes a `capture_contacts` made by the root widget itself to the root.
            let frame = ContactFrame::enter(contacts_captor_is_root);
            let result = if secondary && !contacts_captor_is_root {
                // A non-claimant contact walks the active path forward-only (see
                // `ChildPod::event_child`); a walk that reached no pod falls back.
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
            // Fold every dirtying channel into `LAYOUT` (mirror of `paint_main_tree`'s
            // fold); the root sits in no pod, so its own consumption counts too.
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

        // IME publish: a hit-tested or focus-routed publish needs a live or opening
        // session; an `Overlay` publish must come from the session's owner surface.
        // An inactive surface releases the session. Mirrors `paint_overlays`' rule;
        // runs before the focus commit, so a focus request in the same pass wins.
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

        // Per-variant commit: capture and focus bookkeeping per event kind, through
        // the change-guarded writers that own the focus/IME edge generation.
        // `focus_claim_honoured`, not `focus_req`, because an arm may ignore a claim.
        let mut focus_claim_honoured = false;
        match event {
            // A non-claimant contact (rule (c)): no capture, no blur; an explicit
            // focus request or release is honoured.
            InputEvent::Pointer(_) if secondary => {
                if focus_req {
                    focus_claim_honoured = true;
                    self.set_focus_active(true);
                }
                if focus_rel {
                    self.release_focus_session();
                }
            }
            // Unreachable: the contact gate unwrapped every contact.
            InputEvent::PointerContact { .. } => {}
            InputEvent::Pointer(pointer) => match pointer.phase {
                PointerPhase::Down => {
                    if captured {
                        // Rule (a): the capturing contact is the claimant. The opt-in is
                        // read from the pass so a captor below a component boundary counts.
                        self.capture_claimant = Some(pointer_id);
                        self.capture_contacts = contact_pass.contacts_requested();
                        self.contacts_captor_is_root = root_opted_in;
                    }
                    if focus_req {
                        focus_claim_honoured = true;
                        self.set_focus_active(true);
                        // A hit-tested claim belongs to the main tree (see `focus_surface`).
                        self.focus_surface = None;
                    } else {
                        // Blur: no widget on the tapped path took focus.
                        self.release_focus_session();
                    }
                }
                // Only the claimant's (or an uncaptured contact's) release reaches here.
                PointerPhase::Up | PointerPhase::Cancel => {
                    self.capture_claimant = None;
                    self.capture_contacts = false;
                    self.contacts_captor_is_root = false;
                }
                PointerPhase::Move => {}
            },
            // Hit-tested and focus-routed input, plus the file-drop `Ended`
            // broadcast: an explicit focus request is honoured; nothing blurs.
            InputEvent::Scroll { .. }
            | InputEvent::Scale(_)
            | InputEvent::FileDrop(_)
            | InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_) => {
                if focus_req {
                    // `focus_surface` stays: the root cannot attribute a focus-routed
                    // claim, so the next paint re-resolves it from the pods' links.
                    focus_claim_honoured = true;
                    self.set_focus_active(true);
                }
                if focus_rel {
                    self.release_focus_session();
                }
            }
            // A broadcast opens no gesture, claims no focus and blurs nothing; only
            // the IME publish above can move state on it.
            InputEvent::Housekeeping => {}
            // A floated surface's input: a real gesture, but never the blur branch (a
            // toolbar tap must not drop the selection it acts on). `focus_surface` is
            // not written: an overlay-pass claim cannot be attributed at the root.
            InputEvent::Overlay(overlay) => {
                // No container blur sweep runs for a floated press, so a session
                // another surface owns is retired whole (`paint_overlays` does the rest).
                if focus_req {
                    if self.focus_surface.is_some_and(|owner| owner != overlay.key) {
                        self.release_focus_session();
                    }
                    focus_claim_honoured = true;
                    self.set_focus_active(true);
                }
                // An explicit release ends the session, as in the `Key`/`Scroll` arm.
                if focus_rel {
                    self.release_focus_session();
                }
                // A capture from inside the surface is mirrored on any phase, so its
                // follow-ups route down the capture path, even outside the surface.
                if captured {
                    self.capture_claimant = Some(pointer_id);
                    self.capture_contacts = contact_pass.contacts_requested();
                    self.contacts_captor_is_root = root_opted_in;
                }
                // Released on an overlay `Up`/`Cancel` too, or the standing latch
                // would short-circuit the overlay route for every surface.
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

        // Takeover: a container released the opted-in captor, so the opt-in goes
        // too; the claimant latch stays until the claimant's own `Up`/`Cancel`.
        if !secondary && self.capture_claimant.is_some() && contact_pass.capture_released() {
            self.capture_contacts = contact_pass.contacts_requested();
        }

        // Session resolution: an ended session strands everything (past the
        // candidate); an honoured claim commits the candidate; else the live epoch stays.
        let session_ended = focus_active_before && !self.focus_active;
        self.focus_epoch = if session_ended {
            advance_focus_epoch(candidate_focus_epoch)
        } else if focus_claim_honoured {
            candidate_focus_epoch
        } else {
            live_focus_epoch
        };
        self.publish_focus_session();

        // Hover resolution: advancing the epoch strands every stamp this pass did
        // not renew; a claim counts only on a `hover_pass`.
        let hover_claimant = crate::widget::take_hover_claimant();
        if hover_pass || hover_ends {
            self.hover_epoch = self.hover_epoch.wrapping_add(1);
            self.hover_active = hover_pass && hover_claimed;
            // Link identity for the paint-epoch fold; `0` is a root-widget claim.
            self.hover_link = self.hover_active.then(|| hover_claimant.unwrap_or(0));
            // Republish the link a dropping pod checks (see `ChildPod`'s `Drop`);
            // epoch `0` while nothing holds one.
            crate::event::set_live_hover_link(
                self.root_identity,
                if self.hover_active {
                    self.hover_epoch
                } else {
                    0
                },
            );
        }

        // Request resolution: drained every pass; the cursor commits only on a
        // cursor pass, the last request winning and none meaning `Default`.
        let requested = request_slot.take();
        if cursor_pass {
            self.cursor = requested.cursor.unwrap_or_default();
        }
        // The clipboard commits every pass; a paste request is raised, never
        // lowered, so a shell that skips a drain answers late rather than never.
        if let Some(text) = requested.clipboard_write {
            self.pending_clipboard_write = Some(text);
        }
        self.pending_paste_request |= requested.paste_request;
        // Outcome: a hover that ended with nothing taking it is the one repaint no
        // widget can request (see `EventCtx::claim_hover`).
        let needs_redraw = needs_redraw || (hover_was_active && !self.hover_active);

        let mut outcome = EventOutcome {
            // Fold in the overlay route's outside-tap outcome.
            handled: handled || carried.handled,
            needs_redraw: needs_redraw || carried.needs_redraw,
        };

        // File-drop follow-up: `Drop`/`Cancel` re-enters with the `Ended` broadcast
        // (see `FileDropEvent`); `Ended` triggers none, so this is shallow.
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

    /// Route one pointer contact (the root half of [`InputEvent::PointerContact`]'s
    /// multi-contact contract): `Some(false)` as the gesture's own pointer (rule
    /// (a)), `Some(true)` as a non-claimant down a live capture whose captor opted
    /// in (rule (c)), `None` to drop it (rule (b), or rule (c) with no opt-in).
    fn contact_route(&self, pointer_id: PointerId) -> Option<bool> {
        match self.capture_claimant {
            None if pointer_id.slot == 0 => Some(false),
            None => None,
            Some(claimant) if claimant == pointer_id => Some(false),
            Some(_) if self.capture_contacts => Some(true),
            Some(_) => None,
        }
    }

    /// Hit-test the surfaces the last paint floated before the main tree sees an
    /// uncaptured pointer, scroll or scale (see [`crate::overlay`]): walk
    /// [`RenderRoot::overlay_hits`] topmost-first, skip [`OverlayInput::Transparent`],
    /// and re-dispatch the first hit as a window-space [`InputEvent::Overlay`],
    /// consuming the original. A missed primary `Down` notifies every
    /// [`OutsideTap::Notify`] surface with [`OverlayEventKind::OutsideDown`] and is
    /// consumed iff one asked. A live capture skips the route; `FileDrop`, key, IME,
    /// edit and broadcast events pass straight through.
    fn route_overlay(&mut self, state: &mut State, event: &InputEvent) -> OverlayRoute {
        if self.capture_claimant.is_some() || self.overlay_hits.is_empty() {
            return OverlayRoute::Continue(EventOutcome::default());
        }
        let (position, kind) = match event {
            // The gate already unwrapped contacts; the second pattern is for totality.
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

        // Reverse paint order is topmost-first. Cloned for the `&mut self`
        // dispatches below; never on the no-overlays path.
        let hits = self.overlay_hits.clone();
        for hit in hits.iter().rev() {
            // A transparent surface takes no input; the pointer passes through.
            if hit.input == OverlayInput::Transparent {
                continue;
            }
            if hit.contains(position) {
                let routed = InputEvent::Overlay(OverlayEvent {
                    key: hit.key,
                    kind: kind.clone(),
                });
                // Shallow re-entry through the front door: the routed broadcast
                // passes straight through the route and gets the full pass.
                return OverlayRoute::Consumed(self.event(state, &routed));
            }
        }

        // Only a primary press light-dismisses; a secondary press is a context
        // gesture (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
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
            // No outside-taps for a transparent surface either.
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
            // Tell every notified surface before consuming.
            consumed |= consume;
        }
        if consumed {
            OverlayRoute::Consumed(carried)
        } else {
            OverlayRoute::Continue(carried)
        }
    }
}
