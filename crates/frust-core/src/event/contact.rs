use std::cell::Cell;

use crate::overlay::OverlayKey;

use super::ctx::EventResult;
use super::input::{InputEvent, OverlayEvent, OverlayEventKind};
use super::pointer::PointerId;

/// What the contact pass running on this thread knows about the pointer it is
/// dispatching — the state [`ContactPass`] brackets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContactPassState {
    /// Whether a root opened a contact pass at all. A
    /// [`EventCtx::capture_contacts`] made outside one (a widget driven with no
    /// root above it) records nothing here.
    open: bool,
    /// The contact being dispatched — what [`EventCtx::new`] seeds a fresh
    /// context's [`EventCtx::pointer_id`] with.
    pointer_id: PointerId,
    /// Whether the dispatch is a **non-claimant** contact travelling down a live
    /// capture's path (rule (c) of [`InputEvent::PointerContact`]'s contract).
    /// While set, [`crate::widget::ChildPod::set_active`] refuses to drop a
    /// recorded active link, so another contact's `Up`/`Cancel` cannot release
    /// the claimant's capture inside any container.
    secondary: bool,
    /// Whether a widget called [`EventCtx::capture_contacts`] during this pass
    /// — since the last [`EventCtx::release_captured_child`] that released the
    /// opted-in widget, which resets it (so the flag ends the pass naming only
    /// an opt-in that is still on the active path).
    contacts_requested: bool,
    /// Whether a container released the gesture's opted-in widget from the
    /// active path during this pass ([`EventCtx::release_captured_child`]).
    capture_released: bool,
}

impl ContactPassState {
    /// Nothing open: the mouse, no secondary contact, nothing requested.
    const IDLE: ContactPassState = ContactPassState {
        open: false,
        pointer_id: PointerId::MOUSE,
        secondary: false,
        contacts_requested: false,
        capture_released: false,
    };
}

thread_local! {
    /// The contact pass currently running on this thread — opened by
    /// [`crate::app::RenderRoot::event`] for each dispatch through a
    /// [`ContactPass`] guard.
    ///
    /// **A slot as well as a per-context field**, for the component boundary's
    /// sake: a [`crate::component`] element dispatches its subtree through a
    /// *fresh* [`EventCtx`] over its own local state, so anything carried only
    /// in the context would stop at it. Seeding [`EventCtx::new`] from here
    /// keeps [`EventCtx::pointer_id`] right below a component, and recording
    /// [`EventCtx::capture_contacts`] here lets the root see an opt-in made
    /// anywhere in the tree — the same reason the cursor request rides a slot
    /// (see [`CURSOR_REQUEST`]).
    ///
    /// Thread-local for its neighbours' UI-thread-affinity reason.
    static CONTACT_PASS: Cell<ContactPassState> = const { Cell::new(ContactPassState::IDLE) };
}

/// The bracket around one root dispatch's contact identity: entering publishes
/// which contact is being dispatched (and whether it is a non-claimant one),
/// dropping restores whatever the enclosing pass had — so a dispatch that
/// re-enters [`crate::app::RenderRoot::event`] (the overlay pre-pass does)
/// scopes its own opt-in and hands the slot back on exit, unwind included.
pub(crate) struct ContactPass {
    saved: ContactPassState,
}

impl ContactPass {
    /// Open a pass dispatching `pointer_id`; `secondary` marks a non-claimant
    /// contact routed down a live capture's path.
    pub(crate) fn enter(pointer_id: PointerId, secondary: bool) -> Self {
        let saved = CONTACT_PASS.with(|slot| {
            slot.replace(ContactPassState {
                open: true,
                pointer_id,
                secondary,
                contacts_requested: false,
                capture_released: false,
            })
        });
        Self { saved }
    }

    /// Whether a widget called [`EventCtx::capture_contacts`] during this pass
    /// (and no later [`EventCtx::release_captured_child`] in it released that
    /// widget from the active path).
    pub(crate) fn contacts_requested(&self) -> bool {
        CONTACT_PASS.with(|slot| slot.get().contacts_requested)
    }

    /// Whether a container released the gesture's opted-in widget from the
    /// active path during this pass ([`EventCtx::release_captured_child`]).
    pub(crate) fn capture_released(&self) -> bool {
        CONTACT_PASS.with(|slot| slot.get().capture_released)
    }
}

impl Drop for ContactPass {
    fn drop(&mut self) {
        CONTACT_PASS.with(|slot| slot.set(self.saved));
    }
}

/// The contact the pass running on this thread is dispatching —
/// [`PointerId::MOUSE`] outside any pass.
pub(crate) fn current_pointer_id() -> PointerId {
    CONTACT_PASS.with(|slot| slot.get().pointer_id)
}

/// Whether the pass running on this thread is delivering a non-claimant
/// contact down a live capture's path (see [`ContactPassState::secondary`]).
pub(crate) fn in_secondary_contact_pass() -> bool {
    CONTACT_PASS.with(|slot| slot.get().secondary)
}

/// Record a [`EventCtx::capture_contacts`] call in the open pass, if any, and
/// in the dispatch frame that made it.
pub(super) fn note_contacts_requested() {
    CONTACT_PASS.with(|slot| {
        let mut state = slot.get();
        if state.open {
            state.contacts_requested = true;
            slot.set(state);
        }
    });
    CONTACT_FRAME.with(|slot| {
        let mut frame = slot.get();
        frame.opted_in = true;
        slot.set(frame);
    });
}

/// Record an [`EventCtx::release_captured_child`] that took the opted-in
/// widget off the active path: the pass's opt-in is void from here on, unless a
/// later [`EventCtx::capture_contacts`] in the same pass (the container that
/// took over opting in itself) records a fresh one.
pub(super) fn note_capture_released() {
    CONTACT_PASS.with(|slot| {
        let mut state = slot.get();
        if state.open {
            state.capture_released = true;
            state.contacts_requested = false;
            slot.set(state);
        }
    });
}

/// What one widget dispatch — a [`crate::widget::ChildPod::event_child`] call,
/// or the root's own call into its root widget — learned about the gesture's
/// contact opt-in. The state [`ContactFrame`] brackets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ContactFrameState {
    /// The dispatched widget itself called [`EventCtx::capture_contacts`].
    opted_in: bool,
    /// A widget below it did (folded in when each nested frame closes).
    opted_in_below: bool,
    /// The dispatch runs inside the handler of the widget that holds the live
    /// opt-in (that widget's own frame included): a release made here never
    /// takes the opt-in off the active path, because its holder stays on it.
    under_captor: bool,
}

thread_local! {
    /// The innermost widget dispatch's [`ContactFrameState`]. A slot rather
    /// than an [`EventCtx`] field for [`CONTACT_PASS`]'s reason: a component
    /// boundary dispatches through a fresh context, and the opt-in must still be
    /// attributed to the pod it was made under.
    static CONTACT_FRAME: Cell<ContactFrameState> =
        const { Cell::new(ContactFrameState { opted_in: false, opted_in_below: false, under_captor: false }) };
}

/// The bracket around one widget dispatch's contact opt-in bookkeeping: entering
/// opens a clean frame, [`ContactFrame::close`] reports whether the dispatched
/// widget opted in itself and whether anything at or below it did, and dropping
/// restores the enclosing frame with this one's opt-in folded into its
/// `opted_in_below` (unwind included).
pub(crate) struct ContactFrame {
    saved: ContactFrameState,
}

impl ContactFrame {
    /// Open a frame for one widget dispatch; `captor` marks the dispatched
    /// widget as the holder of the live opt-in.
    pub(crate) fn enter(captor: bool) -> Self {
        let saved = CONTACT_FRAME.with(|slot| {
            let saved = slot.get();
            slot.set(ContactFrameState {
                opted_in: false,
                opted_in_below: false,
                under_captor: saved.under_captor || captor,
            });
            saved
        });
        Self { saved }
    }

    /// Close the frame: `(opted_in, opted_in_at_or_below)` for the dispatched
    /// widget.
    pub(crate) fn close(self) -> (bool, bool) {
        let frame = CONTACT_FRAME.with(|slot| slot.get());
        (frame.opted_in, frame.opted_in || frame.opted_in_below)
    }
}

impl Drop for ContactFrame {
    fn drop(&mut self) {
        CONTACT_FRAME.with(|slot| {
            let frame = slot.get();
            let mut restored = self.saved;
            restored.opted_in_below |= frame.opted_in || frame.opted_in_below;
            slot.set(restored);
        });
    }
}

/// Whether the dispatch running on this thread is inside the handler of the
/// widget that holds the live contact opt-in (see
/// [`ContactFrameState::under_captor`]).
pub(super) fn under_contact_captor() -> bool {
    CONTACT_FRAME.with(|slot| slot.get().under_captor)
}

/// One level of a **secondary-contact walk**: the real event, in the coordinate
/// space of the container currently being handed the walk's carrier, and what
/// the walk delivered below that container.
struct SecondaryWalkFrame {
    /// The non-claimant contact's event, in the space of the container whose
    /// handler is running (i.e. what that container's own `ChildPod`s
    /// receive in their parent's space).
    event: InputEvent,
    /// The result of the delivery the walk made below this container, `None`
    /// until one happened.
    delivered: Option<EventResult>,
}

thread_local! {
    /// The secondary-contact walk running on this thread, if any — innermost
    /// level only; each level saves and restores its enclosing one.
    ///
    /// A slot for the same component-boundary reason as [`CONTACT_PASS`], and
    /// because the walk's real event has to cross a container's handler that is
    /// only ever handed the inert [`secondary_walk_carrier`].
    static SECONDARY_WALK: std::cell::RefCell<Option<SecondaryWalkFrame>> =
        const { std::cell::RefCell::new(None) };

    /// The [`OverlayKey`] the walk's carrier is addressed to — allocated once
    /// per thread from [`OverlayKey::next`], so it is distinct from every key an
    /// overlay owner holds and the carrier is ignored by all of them.
    static SECONDARY_WALK_KEY: OverlayKey = OverlayKey::next();
}

/// The event a container on the active chain is handed while a non-claimant
/// contact walks past it: an [`InputEvent::Overlay`] broadcast addressed to a
/// key no owner holds. Every container already forwards a broadcast to its
/// children before running any gesture, capture, focus or hit-test logic of its
/// own (the broadcast-first rule), and every widget ignores an overlay event
/// addressed to someone else — so a container's handler runs, but none of its
/// pointer machinery does.
pub(crate) fn secondary_walk_carrier() -> InputEvent {
    InputEvent::Overlay(OverlayEvent {
        key: SECONDARY_WALK_KEY.with(|key| *key),
        kind: OverlayEventKind::OutsideDown,
    })
}

/// Whether `event` is [`secondary_walk_carrier`]'s carrier.
fn is_secondary_walk_carrier(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Overlay(overlay)
        if overlay.key == SECONDARY_WALK_KEY.with(|key| *key))
}

/// The real event a secondary-contact walk is carrying past the container
/// that just routed `event` — `Some` only when a walk is running **and** `event`
/// is its carrier (a container that synthesizes an event of its own mid-walk
/// dispatches it normally).
pub(crate) fn secondary_walk_event(event: &InputEvent) -> Option<InputEvent> {
    if !is_secondary_walk_carrier(event) {
        return None;
    }
    SECONDARY_WALK.with(|slot| slot.borrow().as_ref().map(|frame| frame.event.clone()))
}

/// Restores the enclosing walk level on drop (unwind included).
struct SecondaryWalkRestore(Option<SecondaryWalkFrame>);

impl Drop for SecondaryWalkRestore {
    fn drop(&mut self) {
        let saved = self.0.take();
        SECONDARY_WALK.with(|slot| *slot.borrow_mut() = saved);
    }
}

/// Run `f` as one level of a secondary-contact walk carrying `event` (in the
/// space of the container `f` hands the carrier to), returning `f`'s result and
/// what the walk delivered below it — `None` when the carrier never reached a
/// `ChildPod` on the active chain.
pub(crate) fn run_secondary_walk<R>(
    event: InputEvent,
    f: impl FnOnce() -> R,
) -> (R, Option<EventResult>) {
    let saved = SECONDARY_WALK.with(|slot| {
        slot.borrow_mut().replace(SecondaryWalkFrame {
            event,
            delivered: None,
        })
    });
    let restore = SecondaryWalkRestore(saved);
    let result = f();
    let delivered =
        SECONDARY_WALK.with(|slot| slot.borrow().as_ref().and_then(|frame| frame.delivered));
    drop(restore);
    (result, delivered)
}

/// Run `f` with no secondary-contact walk in force — how the walk hands the
/// real event to the widget it ends at, whose own subtree then routes it the
/// ordinary way.
pub(crate) fn without_secondary_walk<R>(f: impl FnOnce() -> R) -> R {
    let saved = SECONDARY_WALK.with(|slot| slot.borrow_mut().take());
    let _restore = SecondaryWalkRestore(saved);
    f()
}

/// Record that the walk level in force delivered the real event below its
/// container, with `result`.
pub(crate) fn note_secondary_delivered(result: EventResult) {
    SECONDARY_WALK.with(|slot| {
        if let Some(frame) = slot.borrow_mut().as_mut() {
            frame.delivered = Some(result);
        }
    });
}
