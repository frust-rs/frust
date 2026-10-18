//! `RenderRoot`'s event pass: the contact gate, the overlay route and the
//! dispatch into the root widget, with the capture, focus, hover and request
//! bookkeeping the root keeps across passes.

use std::any::Any;
use std::rc::Weak;

use kurbo::Point;

use super::{OverlayRoute, RenderRoot, advance_focus_epoch};
use crate::event::{
    ContactFrame, ContactPass, EventCtx, EventOutcome, EventResult, FileDropEvent, FileDropPhase,
    ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, OverlayEvent, OverlayEventKind,
    PointerButton, PointerId, PointerPhase, RequestPass,
};
use crate::overlay::{OutsideTap, OverlayHit, OverlayInput, OverlayKey, OverlayPod};
use crate::shortcuts::{DeliveryProbe, KeyForwardPass};
use crate::tree::WidgetPod;
use crate::view::{ChangeFlags, View};
use crate::widget::{ChildPod, FocusGrant};

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
    /// A traversal grant pass (`traverse_focus`); `root_grant` when the root widget is its target.
    grant: bool,
    root_grant: bool,
}

/// What the root widget's dispatch reported, read off its `EventCtx`.
struct RootDispatch {
    handled: bool,
    needs_redraw: bool,
    captured: bool,
    hover_claimed: bool,
    focus_req: bool,
    /// The claim came from a pod already on the live focus chain.
    focus_req_from_live: bool,
    focus_rel: bool,
    ime: Option<ImeState>,
    ime_published: bool,
    /// The root widget opted into the other contacts during this dispatch.
    root_opted_in: bool,
}

/// The root widget's stop id: no `ChildPod::inspect_id` is `0`.
const ROOT_STOP: u64 = 0;

/// One pod the focus-traversal walk can stop on.
struct FocusStop {
    /// The pod's `inspect_id`, or `ROOT_STOP`: the grant's target.
    id: u64,
    /// The floated surface holding the pod, whose owner the grant is addressed through.
    surface: Option<OverlayKey>,
    /// Whether the pod and every ancestor hold live focus links.
    current: bool,
}

/// The traversal walk's state: the floated pods the last paint drew, and the stops so far.
struct FocusWalk<'a> {
    floated: &'a [(OverlayPod, OverlayHit)],
    stops: Vec<FocusStop>,
}

impl FocusWalk<'_> {
    /// Visit `pod` and its traversable subtree in pre-order; `chain` is the ancestors' live focus.
    fn visit(&mut self, pod: &ChildPod, chain: bool, mut surface: Option<OverlayKey>) {
        let floated = self
            .floated
            .iter()
            .find(|(float, _)| std::ptr::eq(float.as_ptr(), pod));
        if let Some((_, hit)) = floated {
            // A transparent surface takes no input, so nothing in it is a stop either.
            if hit.input == OverlayInput::Transparent {
                return;
            }
            surface = Some(hit.key);
        }
        let chain = chain && pod.holds_live_focus();
        let widget = pod.widget();
        if widget.is_focusable() {
            let id = pod.inspect_id().0;
            self.stops.push(FocusStop {
                id,
                surface,
                current: chain,
            });
        }
        widget.visit_focus_children(&mut |child| self.visit(child, chain, surface));
    }
}

/// `Some(backward)` for a bare Tab or Shift+Tab, the keys focus traversal answers.
/// A plain Escape closes a modal pod; one with a modifier is a chord for the table.
fn is_bare_escape(key: &KeyEvent) -> bool {
    key.key == Key::Named(NamedKey::Escape) && key.modifiers == Modifiers::default()
}

fn traversal_direction(key: &KeyEvent) -> Option<bool> {
    let mods = key.modifiers;
    let bare = !(mods.ctrl || mods.alt || mods.meta);
    (key.key == Key::Named(NamedKey::Tab) && bare).then_some(mods.shift)
}

/// A pointer `Down`, bare or routed to a floated surface: what ends a keyboard-origin focus.
fn is_pointer_down(event: &InputEvent) -> bool {
    let pointer = match event {
        InputEvent::Pointer(pointer) => pointer,
        InputEvent::Overlay(OverlayEvent {
            kind: OverlayEventKind::Pointer(pointer),
            ..
        }) => pointer,
        _ => return false,
    };
    pointer.phase == PointerPhase::Down
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
    /// whole tree; an unhandled `Key` goes on (`dispatch_declined_key`); the outcome folds all.
    pub fn event(&mut self, state: &mut State, event: &InputEvent) -> EventOutcome {
        self.dispatch_pass(state, event, None).0
    }

    /// `event`'s body, as a traversal grant pass when `grant` is `Some(root_is_target)`; also
    /// returns whether a focus claim was honoured.
    fn dispatch_pass(
        &mut self,
        state: &mut State,
        event: &InputEvent,
        grant: Option<bool>,
    ) -> (EventOutcome, bool) {
        let Some(root_id) = self.root_id else {
            return (EventOutcome::default(), false);
        };
        self.trap_modal_focus();
        let (unwrapped, pointer_id) = unwrap_contact(event);
        let event = unwrapped.as_ref().unwrap_or(event);
        if is_pointer_down(event) {
            self.set_keyboard_focus(false);
        }
        let Some(secondary) = self.gate_contact(event, pointer_id) else {
            return (EventOutcome::default(), false);
        };
        // Published for the whole dispatch, file-drop follow-up included (`EventCtx::pointer_id`,
        // the contact opt-in, `ChildPod::set_active`); restored on drop at the end of this call.
        let contact_pass = ContactPass::enter(pointer_id, secondary);

        // A pointer over a floated surface reaches nothing below, not even the blur rule.
        let carried = match self.route_overlay(state, event) {
            OverlayRoute::Consumed(outcome) => return (outcome, false),
            OverlayRoute::Continue(outcome) => outcome,
        };
        let frame = self.open_frame(event, pointer_id, secondary, grant);
        let Some(pod) = self.tree.pod_mut(root_id) else {
            // Keep the outcome an outside-tap notification already produced.
            self.publish_focus_session();
            return (carried, false);
        };
        // Opened empty, so a request from a dispatch no root drove cannot leak in; closed by
        // `resolve_requests`, before the file-drop follow-up opens its own.
        let request_slot = RequestPass::enter();
        let forward_slot = KeyForwardPass::enter();
        let mut root = Self::dispatch_root(pod, &mut self.pending, &frame, state, event);
        // Release-then-commit: only a landing grant ends the old session (`traverse_focus`).
        if frame.grant && root.focus_req {
            self.release_focus_session();
        }
        root.ime_published = root.ime.is_some();
        self.publish_ime(event, root.focus_req, root.ime.take());
        let focus_claim_honoured = self.commit_event(event, &frame, &root, &contact_pass);
        self.resolve_focus_session(&frame, focus_claim_honoured);
        self.record_key_forward(forward_slot.take());
        drop(forward_slot);
        self.resolve_hover(&frame, root.hover_claimed);
        self.resolve_requests(request_slot, frame.cursor_pass);
        let carried = if root.handled {
            carried
        } else {
            self.dispatch_declined_key(state, event, carried)
        };
        let mut outcome = self.build_outcome(&frame, &root, carried);
        self.file_drop_follow_up(state, event, &mut outcome);
        (outcome, focus_claim_honoured)
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
    /// A grant's candidate skips the epoch its release would take. Writes no root field;
    /// publishes the live focus session.
    fn open_frame(
        &self,
        event: &InputEvent,
        pointer_id: PointerId,
        secondary: bool,
        grant: Option<bool>,
    ) -> DispatchFrame {
        let live_focus_epoch = self.focus_epoch;
        let mut candidate_focus_epoch = advance_focus_epoch(live_focus_epoch);
        if grant.is_some() && (self.focus_active || self.ime_state.is_some()) {
            candidate_focus_epoch = advance_focus_epoch(candidate_focus_epoch);
        }
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
            grant: grant.is_some(),
            root_grant: grant == Some(true),
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
        crate::widget::clear_focus_req_from_live();
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, pod.origin(), pod.size());
        // Seed the root widget's focus and hover state; deeper pods thread their own.
        ctx.set_has_focus(frame.focus_active_before);
        ctx.set_hovered(frame.hover_was_active);
        ctx.set_hover_epoch(frame.hover_epoch);
        ctx.set_hover_eligible(frame.hover_pass);
        // Tells a dropping pod's destructor this root's link apart (see `root_identity`).
        ctx.set_hover_root(frame.root_identity);
        if frame.root_grant {
            ctx.request_focus();
        }
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
            // A root-widget claim (no pod) re-claims whenever a session stands.
            focus_req_from_live: crate::widget::take_focus_req_from_live()
                .unwrap_or(frame.focus_active_before),
            focus_rel: ctx.is_focus_released(),
            ime: ctx.take_ime_state(),
            ime_published: false,
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
            // Hit-tested and focus-routed input, the file-drop `Ended` broadcast, and a
            // shortcut (a key press: its declarer may move focus, e.g. to a search field).
            InputEvent::Scroll { .. }
            | InputEvent::Scale(_)
            | InputEvent::FileDrop(_)
            | InputEvent::Key(_)
            | InputEvent::Ime(_)
            | InputEvent::EditCommand(_)
            | InputEvent::Shortcut(_) => {
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
    /// blurs; an `Up`/`Cancel` (only the claimant's reaches here) clears the latch. A claim whose
    /// pass published no `ImeState` drops the previous holder's surface, unless the claimant
    /// already held a live link (a chrome re-claim). Returns
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
                    if !root.ime_published && !root.focus_req_from_live {
                        self.store_ime_state(None);
                    }
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
    /// claim commits the candidate (outside a grant, clearing the keyboard flag); else the live
    /// epoch stays. Writes `focus_epoch`, publishes it.
    fn resolve_focus_session(&mut self, frame: &DispatchFrame, focus_claim_honoured: bool) {
        if focus_claim_honoured && !frame.grant {
            self.set_keyboard_focus(false);
        }
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

    /// Offer a `Key` the focus path declined, each delivery a full pass of its own, until one is
    /// answered: as `OverlayEventKind::Key` to the focus session's forward target, then (an
    /// Escape) to the topmost modal pod, then to the last paint's shortcut declarations in tier
    /// order (`ShortcutTable::candidates`), masked to the modal pod's own, then (a bare Tab or
    /// Shift+Tab, unless [`RenderRoot::set_tab_traversal`] is off) to `traverse_focus`. Only an
    /// interactive pod takes keys. Folds every delivery's outcome into `carried`.
    fn dispatch_declined_key(
        &mut self,
        state: &mut State,
        event: &InputEvent,
        mut carried: EventOutcome,
    ) -> EventOutcome {
        let InputEvent::Key(key) = event else {
            return carried;
        };
        let interactive = |hit: &&OverlayHit| hit.input == OverlayInput::Interactive;
        let modal = self.overlay_hits.iter().rev().find(|hit| hit.is_modal());
        let modal = modal.map(|hit| hit.key);
        let forward = self.live_key_forward().filter(|pod| {
            self.overlay_hits
                .iter()
                .filter(interactive)
                .any(|hit| hit.key == *pod)
        });
        let escape = modal.filter(|pod| is_bare_escape(key) && forward != Some(*pod));
        for pod in forward.into_iter().chain(escape) {
            let kind = OverlayEventKind::Key(key.clone());
            let delivery = InputEvent::Overlay(OverlayEvent { key: pod, kind });
            if self.deliver_declined(state, &delivery, &mut carried) {
                return carried;
            }
        }
        let top_overlay = modal.or_else(|| {
            let top = self.overlay_hits.iter().rev().find(interactive);
            top.map(|hit| hit.key)
        });
        for (id, overlay) in self.shortcuts.candidates(key, top_overlay, modal.is_some()) {
            // A floated pod is reached only through its owner's slot, never by a bare broadcast.
            let delivery = match overlay {
                None => InputEvent::Shortcut(id),
                Some(key) => InputEvent::Overlay(OverlayEvent {
                    key,
                    kind: OverlayEventKind::Shortcut(id),
                }),
            };
            if self.deliver_declined(state, &delivery, &mut carried) {
                return carried;
            }
        }
        if let Some(backward) = traversal_direction(key).filter(|_| self.tab_traversal) {
            self.traverse_focus(state, backward, &mut carried);
        }
        carried
    }

    /// Move focus to the next (`backward`: previous) stop in walk order, wrapping; with nothing
    /// focused, to the first (last). Answers the key (and sets the keyboard flag) only when a
    /// grant lands or the focused stop is the only one; a walk where none lands changes nothing.
    ///
    /// # The grant
    ///
    /// One pass delivers `InputEvent::Shortcut` carrying this root's private `focus_grant` id (bare
    /// for a main-tree stop, as `OverlayEventKind::Shortcut` through a floated stop's owner) under
    /// an open [`FocusGrant`], reaching what a shortcut reaches. The target pod (or root widget)
    /// records the claim like `EventCtx::request_focus`; only then is the old session released
    /// (`focus_epoch` advanced, IME surface and key forward dropped) and the claim committed, so a
    /// landing step advances the epoch twice. A missed stop is skipped, short of the focused one.
    fn traverse_focus(&mut self, state: &mut State, backward: bool, carried: &mut EventOutcome) {
        let stops = self.focus_stops();
        let len = stops.len();
        let current = stops.iter().rposition(|stop| stop.current);
        let next = match current {
            _ if len == 0 => return,
            None if backward => len - 1,
            None => 0,
            Some(i) if backward => (i + len - 1) % len,
            Some(i) => (i + 1) % len,
        };
        let step = |n: usize| if backward { len - n } else { n };
        let landed = current == Some(next)
            || (0..len)
                .map(|n| (next + step(n)) % len)
                .take_while(|&i| Some(i) != current)
                .any(|i| self.grant_focus(state, &stops[i]));
        if landed {
            carried.handled = true;
            carried.needs_redraw = true;
            self.set_keyboard_focus(true);
        }
    }

    /// One grant pass to `stop` (see `traverse_focus`); whether its claim was honoured.
    fn grant_focus(&mut self, state: &mut State, stop: &FocusStop) -> bool {
        let carrier = self.focus_grant;
        let delivery = match stop.surface {
            None => InputEvent::Shortcut(carrier),
            Some(key) => InputEvent::Overlay(OverlayEvent {
                key,
                kind: OverlayEventKind::Shortcut(carrier),
            }),
        };
        let _grant = FocusGrant::enter(carrier, stop.id);
        let _probe = DeliveryProbe::enter();
        let root_is_target = stop.id == ROOT_STOP;
        self.dispatch_pass(state, &delivery, Some(root_is_target)).1
    }

    /// The traversal walk: focusable pods in pre-order over `Widget::visit_focus_children`,
    /// from the main root (itself a stop when focusable), or from the topmost modal pod alone
    /// while one is registered.
    fn focus_stops(&self) -> Vec<FocusStop> {
        self.publish_focus_session();
        let floated: Vec<(OverlayPod, OverlayHit)> = self
            .overlay_pods
            .iter()
            .zip(&self.overlay_hits)
            .filter_map(|(weak, hit)| Some((weak.upgrade()?, *hit)))
            .collect();
        let mut walk = FocusWalk {
            floated: &floated,
            stops: Vec::new(),
        };
        let chain = self.focus_active;
        if let Some(modal) = self.overlay_hits.iter().rposition(|hit| hit.is_modal()) {
            // `overlay_pods` runs parallel to `overlay_hits`: both come from one paint's entries.
            if let Some(pod) = self.overlay_pods.get(modal).and_then(Weak::upgrade)
                && let Ok(pod) = pod.try_borrow()
            {
                walk.visit(&pod, chain, None);
            }
        } else if let Some(root) = self.root_id.and_then(|id| self.tree.pod(id)) {
            if root.widget().is_focusable() {
                walk.stops.push(FocusStop {
                    id: ROOT_STOP,
                    surface: None,
                    current: chain,
                });
            }
            root.widget()
                .visit_focus_children(&mut |pod| walk.visit(pod, chain, None));
        }
        walk.stops
    }

    /// One `dispatch_declined_key` delivery: run it, fold its outcome into `carried`, and
    /// report whether a widget answered it.
    fn deliver_declined(
        &mut self,
        state: &mut State,
        delivery: &InputEvent,
        carried: &mut EventOutcome,
    ) -> bool {
        let probe = DeliveryProbe::enter();
        let outcome = self.event(state, delivery);
        let answered = probe.handled() || outcome.handled;
        carried.needs_redraw |= outcome.needs_redraw || answered;
        carried.handled |= answered;
        answered
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
            | InputEvent::Overlay(_)
            | InputEvent::Shortcut(_) => {
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
