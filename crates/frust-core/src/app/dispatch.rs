//! `RenderRoot`'s event pass: the contact gate, the overlay route and the
//! dispatch into the root widget, with the capture, focus, hover and request
//! bookkeeping the root keeps across passes.

use std::any::Any;

use kurbo::Point;

use super::{OverlayRoute, RenderRoot, advance_focus_epoch};
use crate::event::{
    ContactFrame, ContactPass, EventCtx, EventOutcome, EventResult, FileDropEvent, FileDropPhase,
    ImeState, InputEvent, OverlayEvent, OverlayEventKind, PointerButton, PointerId, PointerPhase,
    RequestPass,
};
use crate::overlay::{OutsideTap, OverlayInput};
use crate::tree::WidgetPod;
use crate::view::{ChangeFlags, View};

/// Per-dispatch state the steps of `RenderRoot::event` share, read after the overlay route.
struct DispatchFrame {
    pointer_id: PointerId,
    /// A non-claimant contact down a live capture (rule (c)).
    secondary: bool,
    live_focus_epoch: u64,
    candidate_focus_epoch: u64,
    focus_active_before: bool,
    /// Only an uncaptured `Move` may claim hover.
    hover_pass: bool,
    /// A non-`secondary` `Down`/`Up`/`Cancel` ends hover.
    hover_ends: bool,
    hover_epoch: u64,
    hover_was_active: bool,
    root_identity: u64,
    /// Any claimant `Move`, captured included, so a drag keeps its cursor outside its bounds.
    cursor_pass: bool,
    /// The root widget itself holds the live contacts opt-in, rather than a pod below it.
    contacts_captor_is_root: bool,
}

/// What the root widget's dispatch reported, read off its `EventCtx`.
struct RootDispatch {
    handled: bool,
    needs_redraw: bool,
    captured: bool,
    hover_claimed: bool,
    focus_req: bool,
    focus_rel: bool,
    ime: Option<ImeState>,
    /// The root widget opted into the other contacts during this dispatch.
    root_opted_in: bool,
}

/// Contact gate, first half: unwrap a `PointerContact` into a bare `Pointer` (returned, for the
/// caller to hold) and name its contact. A bare `Pointer` is the mouse; other events keep the
/// enclosing pass's contact.
fn unwrap_contact(event: &InputEvent) -> (Option<InputEvent>, PointerId) {
    match event {
        InputEvent::PointerContact {
            pointer_id,
            event: pointer,
        } => (Some(InputEvent::Pointer(*pointer)), *pointer_id),
        InputEvent::Pointer(_) => (None, PointerId::MOUSE),
        _ => (None, crate::event::current_pointer_id()),
    }
}

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
        let (unwrapped, pointer_id) = unwrap_contact(event);
        let event = unwrapped.as_ref().unwrap_or(event);
        let Some(secondary) = self.gate_contact(event, pointer_id) else {
            return EventOutcome::default();
        };
        // Published for the whole dispatch, file-drop follow-up included (`EventCtx::pointer_id`,
        // the contact opt-in, `ChildPod::set_active`); restored on drop at the end of this call.
        let contact_pass = ContactPass::enter(pointer_id, secondary);

        // A pointer over a floated surface reaches nothing below, not even the blur rule.
        let carried = match self.route_overlay(state, event) {
            OverlayRoute::Consumed(outcome) => return outcome,
            OverlayRoute::Continue(outcome) => outcome,
        };
        let frame = self.open_frame(event, pointer_id, secondary);
        let Some(pod) = self.tree.pod_mut(root_id) else {
            // Keep the outcome an outside-tap notification already produced.
            self.publish_focus_session();
            return carried;
        };
        // Opened empty, so a request from a dispatch no root drove cannot leak in; closed by
        // `resolve_requests`, before the file-drop follow-up opens its own.
        let request_slot = RequestPass::enter();
        let mut root = Self::dispatch_root(pod, &mut self.pending, &frame, state, event);

        self.publish_ime(event, root.focus_req, root.ime.take());
        let focus_claim_honoured = self.commit_event(event, &frame, &root, &contact_pass);
        self.resolve_focus_session(&frame, focus_claim_honoured);
        self.resolve_hover(&frame, root.hover_claimed);
        self.resolve_requests(request_slot, frame.cursor_pass);
        let mut outcome = self.build_outcome(&frame, &root, carried);
        self.file_drop_follow_up(state, event, &mut outcome);
        outcome
    }

    /// Contact gate, second half: `Some(secondary)` for a routed pointer (see `contact_route`),
    /// `Some(false)` for any other event, `None` to drop the event. Writes nothing.
    fn gate_contact(&self, event: &InputEvent, pointer_id: PointerId) -> Option<bool> {
        if matches!(event, InputEvent::Pointer(_)) {
            // `None` is rule (b), or rule (c) for a captor that did not opt in.
            self.contact_route(pointer_id)
        } else {
            Some(false)
        }
    }

    /// Open the focus-epoch bracket (claims are stamped with a candidate epoch so older links
    /// strand by arithmetic; `resolve_focus_session` commits it or not) and read the pass flags.
    /// Writes no root field; publishes the live focus session.
    fn open_frame(
        &self,
        event: &InputEvent,
        pointer_id: PointerId,
        secondary: bool,
    ) -> DispatchFrame {
        let live_focus_epoch = self.focus_epoch;
        let candidate_focus_epoch = advance_focus_epoch(live_focus_epoch);
        crate::widget::set_live_focus_session(
            self.root_identity,
            live_focus_epoch,
            candidate_focus_epoch,
        );
        // `docs/LIMITATIONS.md`'s `hover-window-leave-standing` covers the ending rule.
        let is_move = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Move);
        let hover_ends = !secondary
            && matches!(
                event,
                InputEvent::Pointer(p)
                    if matches!(
                        p.phase,
                        PointerPhase::Down | PointerPhase::Up | PointerPhase::Cancel
                    )
            );
        DispatchFrame {
            pointer_id,
            secondary,
            live_focus_epoch,
            candidate_focus_epoch,
            focus_active_before: self.focus_active,
            hover_pass: is_move && self.capture_claimant.is_none(),
            hover_ends,
            hover_epoch: self.hover_epoch,
            hover_was_active: self.hover_active,
            root_identity: self.root_identity,
            cursor_pass: !secondary && is_move,
            contacts_captor_is_root: self.capture_claimant.is_some()
                && self.capture_contacts
                && self.contacts_captor_is_root,
        }
    }

    /// Dispatch `event` into the root widget (forward-only down the active path for a
    /// non-claimant contact) and read its context back. Writes `pending` (`LAYOUT`) when the
    /// dispatch dirtied anything; clears the thread-local hover claimant first.
    fn dispatch_root(
        pod: &mut WidgetPod,
        pending: &mut ChangeFlags,
        frame: &DispatchFrame,
        state: &mut State,
        event: &InputEvent,
    ) -> RootDispatch {
        // The claimant pod this dispatch records, if any (see `hover_link`).
        crate::widget::clear_hover_claimant();
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, pod.origin(), pod.size());
        // Seed the root widget's focus and hover state; deeper pods thread their own.
        ctx.set_has_focus(frame.focus_active_before);
        ctx.set_hovered(frame.hover_was_active);
        ctx.set_hover_epoch(frame.hover_epoch);
        ctx.set_hover_eligible(frame.hover_pass);
        // Tells a dropping pod's destructor this root's link apart (see `root_identity`).
        ctx.set_hover_root(frame.root_identity);
        // Attributes a `capture_contacts` made by the root widget itself to the root.
        let contact_frame = ContactFrame::enter(frame.contacts_captor_is_root);
        let result = if frame.secondary && !frame.contacts_captor_is_root {
            // A walk that reached no pod falls back (see `ChildPod::event_child`).
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
        let root_opted_in = contact_frame.close().0;
        let handled = matches!(result, EventResult::Handled);
        // Fold every dirtying channel into `LAYOUT` (mirror of `paint_main_tree`'s
        // fold); the root sits in no pod, so its own consumption counts too.
        if ctx.needs_layout() || ctx.is_layout_path_dirty() || handled || ctx.needs_redraw() {
            *pending |= ChangeFlags::LAYOUT;
        }
        RootDispatch {
            handled,
            needs_redraw: ctx.needs_redraw() || handled,
            captured: ctx.is_pointer_captured(),
            hover_claimed: ctx.is_hover_claimed(),
            focus_req: ctx.is_focus_requested(),
            focus_rel: ctx.is_focus_released(),
            ime: ctx.take_ime_state(),
            root_opted_in,
        }
    }

    /// IME publish: a hit-tested or focus-routed publish needs a live or opening session; an
    /// `Overlay` one must come from the session's owner surface (mirrors `paint_overlays`).
    /// Writes the IME state or releases the session; runs before the focus commit.
    fn publish_ime(&mut self, event: &InputEvent, focus_req: bool, ime: Option<ImeState>) {
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
    }

    /// Per-variant commit, then the takeover check. Returns whether a focus claim was honoured
    /// (an arm may ignore one); writes `capture_claimant`, `capture_contacts`,
    /// `contacts_captor_is_root`, `focus_surface` and, through its writers, the focus session.
    fn commit_event(
        &mut self,
        event: &InputEvent,
        frame: &DispatchFrame,
        root: &RootDispatch,
        contact_pass: &ContactPass,
    ) -> bool {
        let mut focus_claim_honoured = false;
        match event {
            // A non-claimant contact (rule (c)): no capture, no blur.
            InputEvent::Pointer(_) if frame.secondary => {
                focus_claim_honoured = self.commit_focus_request(root);
            }
            // Unreachable: the contact gate unwrapped every contact.
            InputEvent::PointerContact { .. } => {}
            InputEvent::Pointer(pointer) => {
                focus_claim_honoured =
                    self.commit_pointer(pointer.phase, frame.pointer_id, root, contact_pass);
            }
            // Hit-tested and focus-routed input, plus the file-drop `Ended` broadcast.
            InputEvent::Scroll { .. }
            | InputEvent::Scale(_)
            | InputEvent::FileDrop(_)
            | InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_) => {
                focus_claim_honoured = self.commit_focus_request(root);
            }
            // A broadcast opens no gesture, claims no focus and blurs nothing; only
            // the IME publish above can move state on it.
            InputEvent::Housekeeping => {}
            InputEvent::Overlay(overlay) => {
                focus_claim_honoured =
                    self.commit_overlay(overlay, frame.pointer_id, root, contact_pass);
            }
        }
        // Takeover: a container released the opted-in captor, so the opt-in goes
        // too; the claimant latch stays until the claimant's own `Up`/`Cancel`.
        if !frame.secondary && self.capture_claimant.is_some() && contact_pass.capture_released() {
            self.capture_contacts = contact_pass.contacts_requested();
        }
        focus_claim_honoured
    }

    /// Honour an explicit focus request, then an explicit release; nothing captures or blurs.
    /// Returns whether a claim was honoured. `focus_surface` stays: the root cannot attribute a
    /// focus-routed claim, so the next paint re-resolves it from the pods' links.
    fn commit_focus_request(&mut self, root: &RootDispatch) -> bool {
        if root.focus_req {
            self.set_focus_active(true);
        }
        if root.focus_rel {
            self.release_focus_session();
        }
        root.focus_req
    }

    /// Commit a claimant (or uncaptured) pointer: a `Down` latches a capture and claims focus or
    /// blurs; an `Up`/`Cancel` (only the claimant's reaches here) clears the latch. Returns
    /// whether a claim was honoured; writes the capture latch, `focus_surface` and focus session.
    fn commit_pointer(
        &mut self,
        phase: PointerPhase,
        pointer_id: PointerId,
        root: &RootDispatch,
        contact_pass: &ContactPass,
    ) -> bool {
        match phase {
            PointerPhase::Down => {
                if root.captured {
                    // Rule (a): the capturing contact is the claimant.
                    self.latch_capture(pointer_id, contact_pass, root.root_opted_in);
                }
                if root.focus_req {
                    self.set_focus_active(true);
                    // A hit-tested claim belongs to the main tree (see `focus_surface`).
                    self.focus_surface = None;
                    true
                } else {
                    // Blur: no widget on the tapped path took focus.
                    self.release_focus_session();
                    false
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                self.clear_capture();
                false
            }
            PointerPhase::Move => false,
        }
    }

    /// Commit a floated surface's input: a real gesture, but never the blur branch (a toolbar tap
    /// must not drop the selection it acts on), and `focus_surface` is not written. Returns
    /// whether a claim was honoured; writes the capture latch and the focus session.
    fn commit_overlay(
        &mut self,
        overlay: &OverlayEvent,
        pointer_id: PointerId,
        root: &RootDispatch,
        contact_pass: &ContactPass,
    ) -> bool {
        // No container blur sweep runs for a floated press, so a session
        // another surface owns is retired whole (`paint_overlays` does the rest).
        if root.focus_req {
            if self.focus_surface.is_some_and(|owner| owner != overlay.key) {
                self.release_focus_session();
            }
            self.set_focus_active(true);
        }
        if root.focus_rel {
            self.release_focus_session();
        }
        // Mirrored on any phase, so its follow-ups route down the capture path, even outside.
        if root.captured {
            self.latch_capture(pointer_id, contact_pass, root.root_opted_in);
        }
        // Released on an overlay `Up`/`Cancel` too, or the standing latch
        // would short-circuit the overlay route for every surface.
        if let OverlayEventKind::Pointer(pointer) = &overlay.kind
            && matches!(pointer.phase, PointerPhase::Up | PointerPhase::Cancel)
            && self
                .capture_claimant
                .is_none_or(|claimant| claimant == pointer_id)
        {
            self.clear_capture();
        }
        root.focus_req
    }

    /// Latch `pointer_id` as the capture claimant. The opt-in is read from the pass, so a captor
    /// below a component boundary counts. Writes the three capture fields.
    fn latch_capture(&mut self, pointer_id: PointerId, pass: &ContactPass, root_opted_in: bool) {
        self.capture_claimant = Some(pointer_id);
        self.capture_contacts = pass.contacts_requested();
        self.contacts_captor_is_root = root_opted_in;
    }

    /// Clear the capture latch and its contacts opt-in. Writes the three capture fields.
    fn clear_capture(&mut self) {
        self.capture_claimant = None;
        self.capture_contacts = false;
        self.contacts_captor_is_root = false;
    }

    /// Session resolution: an ended session strands everything (past the candidate); an honoured
    /// claim commits the candidate; else the live epoch stays. Writes `focus_epoch`, publishes it.
    fn resolve_focus_session(&mut self, frame: &DispatchFrame, focus_claim_honoured: bool) {
        let session_ended = frame.focus_active_before && !self.focus_active;
        self.focus_epoch = if session_ended {
            advance_focus_epoch(frame.candidate_focus_epoch)
        } else if focus_claim_honoured {
            frame.candidate_focus_epoch
        } else {
            frame.live_focus_epoch
        };
        self.publish_focus_session();
    }

    /// Hover resolution: advancing the epoch strands every stamp this pass did not renew; a
    /// claim counts only on a hover pass. Drains the hover claimant; writes `hover_epoch`,
    /// `hover_active`, `hover_link` and the live hover link.
    fn resolve_hover(&mut self, frame: &DispatchFrame, hover_claimed: bool) {
        let hover_claimant = crate::widget::take_hover_claimant();
        if frame.hover_pass || frame.hover_ends {
            self.hover_epoch = self.hover_epoch.wrapping_add(1);
            self.hover_active = frame.hover_pass && hover_claimed;
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
    }

    /// Request resolution: close `request_slot` and commit what it collected. The cursor commits
    /// only on a cursor pass (last request wins, none means `Default`); writes `cursor`,
    /// `pending_clipboard_write` and `pending_paste_request`.
    fn resolve_requests(&mut self, request_slot: RequestPass, cursor_pass: bool) {
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
    }

    /// The pass's outcome, folding in the overlay route's `carried` outside-tap outcome. A hover
    /// that ended with nothing taking it is the one repaint no widget can request.
    fn build_outcome(
        &self,
        frame: &DispatchFrame,
        root: &RootDispatch,
        carried: EventOutcome,
    ) -> EventOutcome {
        let needs_redraw = root.needs_redraw || (frame.hover_was_active && !self.hover_active);
        EventOutcome {
            handled: root.handled || carried.handled,
            needs_redraw: needs_redraw || carried.needs_redraw,
        }
    }

    /// File-drop follow-up: a `Drop`/`Cancel` re-enters with the `Ended` broadcast (see
    /// `FileDropEvent`) and folds its outcome into `outcome`; `Ended` triggers none.
    fn file_drop_follow_up(
        &mut self,
        state: &mut State,
        event: &InputEvent,
        outcome: &mut EventOutcome,
    ) {
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
