//! Thread-local contact identity for multi-contact dispatch: which pointer a pass carries, the
//! `EventCtx::capture_contacts` opt-in, and the secondary-contact walk past the active chain.

use std::cell::Cell;

use crate::overlay::OverlayKey;

use super::ctx::EventResult;
use super::input::{InputEvent, OverlayEvent, OverlayEventKind};
use super::pointer::PointerId;

/// What the contact pass running on this thread knows; bracketed by [`ContactPass`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContactPassState {
    /// Whether a root opened a pass; an opt-in made outside one records nothing.
    open: bool,
    /// The contact being dispatched; seeds a fresh `EventCtx`'s `pointer_id`.
    pointer_id: PointerId,
    /// A non-claimant contact is travelling a live capture's path; while set,
    /// `ChildPod::set_active` refuses to drop a recorded active link.
    secondary: bool,
    /// A widget opted in during this pass, since the last release of the opted-in widget (which
    /// resets it).
    contacts_requested: bool,
    /// A container released the opted-in widget from the active path during this pass.
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
    /// The contact pass running on this thread, bracketed by [`ContactPass`]. A slot as well as an
    /// `EventCtx` field because a component dispatches through a fresh context; seeding from here
    /// keeps the contact id and the opt-in visible across that boundary.
    static CONTACT_PASS: Cell<ContactPassState> = const { Cell::new(ContactPassState::IDLE) };
}

/// The bracket around one root dispatch's contact identity. Dropping restores the enclosing
/// pass's state, so a dispatch re-entering `RenderRoot::event` (the overlay pre-pass) scopes its
/// own opt-in and hands the slot back on exit, unwind included.
pub(crate) struct ContactPass {
    saved: ContactPassState,
}

impl ContactPass {
    /// Open a pass dispatching `pointer_id`; `secondary` marks a non-claimant contact.
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

    /// Whether a widget opted into the other contacts during this pass and no later release in it
    /// took that widget off the active path.
    pub(crate) fn contacts_requested(&self) -> bool {
        CONTACT_PASS.with(|slot| slot.get().contacts_requested)
    }

    /// Whether a container released the opted-in widget from the active path during this pass.
    pub(crate) fn capture_released(&self) -> bool {
        CONTACT_PASS.with(|slot| slot.get().capture_released)
    }
}

impl Drop for ContactPass {
    fn drop(&mut self) {
        CONTACT_PASS.with(|slot| slot.set(self.saved));
    }
}

/// The contact the running pass dispatches; [`PointerId::MOUSE`] outside any pass.
pub(crate) fn current_pointer_id() -> PointerId {
    CONTACT_PASS.with(|slot| slot.get().pointer_id)
}

/// Whether the running pass delivers a non-claimant contact down a live capture's path.
pub(crate) fn in_secondary_contact_pass() -> bool {
    CONTACT_PASS.with(|slot| slot.get().secondary)
}

/// Record a `capture_contacts` call in the open pass, if any, and in the current frame.
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

/// Record a release that took the opted-in widget off the active path: the pass's opt-in is void
/// unless a later `capture_contacts` in the same pass records a fresh one.
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

/// What one widget dispatch (a `ChildPod::event_child` call, or the root's call into its root
/// widget) learned about the contact opt-in; bracketed by [`ContactFrame`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ContactFrameState {
    /// The dispatched widget itself opted in.
    opted_in: bool,
    /// A widget below it did (folded in as each nested frame closes).
    opted_in_below: bool,
    /// The dispatch runs inside the opt-in holder's handler: a release made here never takes the
    /// opt-in off the active path, because its holder stays on it.
    under_captor: bool,
}

thread_local! {
    /// The innermost widget dispatch's [`ContactFrameState`]; a slot for [`CONTACT_PASS`]'s
    /// component-boundary reason.
    static CONTACT_FRAME: Cell<ContactFrameState> =
        const { Cell::new(ContactFrameState { opted_in: false, opted_in_below: false, under_captor: false }) };
}

/// The bracket around one widget dispatch's opt-in bookkeeping: entering opens a clean frame,
/// and dropping restores the enclosing one with this frame's opt-in folded into its
/// `opted_in_below` (unwind included).
pub(crate) struct ContactFrame {
    saved: ContactFrameState,
}

impl ContactFrame {
    /// Open a frame; `captor` marks the dispatched widget as the live opt-in's holder.
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

    /// Close the frame: `(opted_in, opted_in_at_or_below)` for the dispatched widget.
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

/// Whether the running dispatch is inside the live opt-in holder's handler.
pub(super) fn under_contact_captor() -> bool {
    CONTACT_FRAME.with(|slot| slot.get().under_captor)
}

/// One level of a secondary-contact walk: the real event, in the space of the container being
/// handed the carrier, and what the walk delivered below it.
struct SecondaryWalkFrame {
    event: InputEvent,
    /// The result of the delivery below this container; `None` until one happened.
    delivered: Option<EventResult>,
}

thread_local! {
    /// The innermost secondary-contact walk level on this thread; each level saves and restores the
    /// enclosing one.
    static SECONDARY_WALK: std::cell::RefCell<Option<SecondaryWalkFrame>> =
        const { std::cell::RefCell::new(None) };

    /// The key the walk's carrier is addressed to — allocated from [`OverlayKey::next`], so no
    /// overlay owner holds it.
    static SECONDARY_WALK_KEY: OverlayKey = OverlayKey::next();
}

/// The inert event a container on the active chain is handed while a non-claimant contact walks
/// past: an overlay broadcast addressed to a key no owner holds, so the container's handler runs
/// but none of its pointer machinery does.
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

/// The real event a running walk carries past the container that routed `event`; `Some` only
/// when `event` is the carrier.
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

/// Run `f` as one walk level carrying `event`, returning `f`'s result and what the walk delivered
/// below it (`None` when the carrier reached no `ChildPod` on the active chain).
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

/// Run `f` with no walk in force — how the walk hands the real event to the widget it ends at.
pub(crate) fn without_secondary_walk<R>(f: impl FnOnce() -> R) -> R {
    let saved = SECONDARY_WALK.with(|slot| slot.borrow_mut().take());
    let _restore = SecondaryWalkRestore(saved);
    f()
}

/// Record that the walk level in force delivered the real event below its container.
pub(crate) fn note_secondary_delivered(result: EventResult) {
    SECONDARY_WALK.with(|slot| {
        if let Some(frame) = slot.borrow_mut().as_mut() {
            frame.delivered = Some(result);
        }
    });
}
