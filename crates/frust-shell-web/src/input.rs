//! Browser input on a focusable canvas.
//!
//! winit 0.30.13's web backend emits separate mouse and touch variants (no
//! unified pointer API at this pin), wheel deltas in line or pixel units, and
//! plain keyboard events; it emits no IME event at all, so composition is
//! bridged around it by [`crate::ime`]'s hidden-input overlay instead.
//!
//! The mouse/wheel/keyboard halves of that story are the winit-generic
//! [`crate::app_handler::InputState`] ported from `frust-shell-desktop`:
//! this crate's `WindowEvent` handler just calls into it. `WindowEvent::Touch`
//! has no such twin — `frust-shell-desktop` never receives one (winit reports
//! no touch on any desktop backend it targets), so touch mapping is
//! web-specific and lives here instead, in [`TouchTracker`].
//!
//! # Multi-contact contract
//!
//! A browser's `WindowEvent::Touch` carries a per-finger `id` (multiple
//! concurrent contacts are routine — two-finger scroll, pinch). [`TouchTracker`]
//! maps every winit touch `id` to a per-sequence **slot**: the first concurrent
//! contact opens slot 0, later contacts get slots 1, 2, etc., and all slots
//! reset to zero when the last contact ends. Each contact emits
//! [`InputEvent::PointerContact`] with [`PointerId::touch(slot)`], so the root
//! can route multi-finger gestures to widgets that opted into
//! [`EventCtx::capture_contacts`]. The slot semantics match the mobile shells'
//! multi-contact model (see `frust-shell-android`'s `dispatch_touch`) and close
//! the hybrid mouse+touch latch bug by making slot 0 contact delivery distinguish
//! touch from the mouse (which always reports [`PointerId::MOUSE`]): when no
//! capture holds, slot 0 is hit-tested like the mouse, and slot ≥1 contacts are
//! dropped at the root unless the slot-0 captor opted in.

use std::collections::HashMap;

use frust_core::event::{InputEvent, PointerButton, PointerEvent, PointerId, PointerPhase};
use winit::dpi::PhysicalPosition;
use winit::event::TouchPhase as WinitTouchPhase;

use crate::app_handler::physical_to_logical;

/// Map a winit [`WinitTouchPhase`] to our [`PointerPhase`] — a straight
/// rename, pulled out on its own so [`TouchTracker::touch`] can consult the
/// mapped phase without duplicating the match.
pub fn map_touch_phase(phase: WinitTouchPhase) -> PointerPhase {
    match phase {
        WinitTouchPhase::Started => PointerPhase::Down,
        WinitTouchPhase::Moved => PointerPhase::Move,
        WinitTouchPhase::Ended => PointerPhase::Up,
        WinitTouchPhase::Cancelled => PointerPhase::Cancel,
    }
}

/// Maps winit touch `id`s to per-sequence slots (0 for first concurrent contact,
/// 1+ for later, reset when all are up) and emits [`InputEvent::PointerContact`]
/// for each contact so the root can route multi-finger gestures to widgets that
/// opted into multi-contact capture — see the module doc for the multi-contact
/// contract.
#[derive(Debug, Default, Clone)]
pub struct TouchTracker {
    /// Maps each active winit `id` to its assigned slot.
    active_ids: HashMap<u64, u32>,
    /// The next slot to assign to a new `Started` contact.
    next_slot: u32,
}

impl TouchTracker {
    /// A fresh tracker: no contact currently owns the pointer.
    pub fn new() -> Self {
        Self::default()
    }

    /// One winit touch contact — its stable per-finger `id`, phase, and
    /// device-physical location — mapped to an [`InputEvent::PointerContact`] with
    /// its assigned slot, or `None` when the contact should be dropped.
    ///
    /// - A `Started` assigns this `id` a new slot (starting at 0 when the tracker
    ///   is empty) and maps through as `Down`. A duplicate `Started` for an `id`
    ///   that is already active is dropped as a malformed event.
    /// - A `Moved`/`Ended`/`Cancelled` maps through only when `id` has been
    ///   assigned a slot; on `Ended`/`Cancelled` the id is cleared and the slot
    ///   is freed. When all contacts are up, `next_slot` resets to 0 so the next
    ///   gesture's first contact opens slot 0 again.
    /// - A `Moved`/`Ended`/`Cancelled` for an id that was never `Started` (or
    ///   whose `Started` was malformed and dropped) is dropped too — its `Started`
    ///   was never delivered, so delivering any of its later phases would hand the
    ///   tree an unpaired event.
    pub fn touch(
        &mut self,
        id: u64,
        phase: WinitTouchPhase,
        location: PhysicalPosition<f64>,
        scale: f64,
    ) -> Option<InputEvent> {
        let core_phase = map_touch_phase(phase);
        match core_phase {
            PointerPhase::Down => {
                if self.active_ids.contains_key(&id) {
                    return None; // Duplicate Started for the same id
                }
                let slot = self.next_slot;
                self.active_ids.insert(id, slot);
                self.next_slot += 1;
            }
            PointerPhase::Move => {
                if !self.active_ids.contains_key(&id) {
                    return None; // Move for an id that was never started
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if !self.active_ids.contains_key(&id) {
                    return None; // Up/Cancel for an id that was never started
                }
                // We need the slot before removing the id
            }
        }
        let position = physical_to_logical(location.x, location.y, scale);
        let slot = self.active_ids[&id];

        // Remove the id after getting the slot (for Up/Cancel only)
        if matches!(core_phase, PointerPhase::Up | PointerPhase::Cancel) {
            self.active_ids.remove(&id);
            // Reset the slot counter when all contacts are up
            if self.active_ids.is_empty() {
                self.next_slot = 0;
            }
        }

        Some(InputEvent::PointerContact {
            pointer_id: PointerId::touch(slot),
            event: PointerEvent {
                phase: core_phase,
                position,
                button: PointerButton::Primary,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{TouchTracker, WinitTouchPhase, map_touch_phase};
    use frust_core::event::{InputEvent, PointerButton, PointerEvent, PointerId, PointerPhase};
    use kurbo::Point;
    use winit::dpi::PhysicalPosition;

    #[test]
    fn map_touch_phase_maps_every_variant() {
        assert_eq!(
            map_touch_phase(WinitTouchPhase::Started),
            PointerPhase::Down
        );
        assert_eq!(map_touch_phase(WinitTouchPhase::Moved), PointerPhase::Move);
        assert_eq!(map_touch_phase(WinitTouchPhase::Ended), PointerPhase::Up);
        assert_eq!(
            map_touch_phase(WinitTouchPhase::Cancelled),
            PointerPhase::Cancel
        );
    }

    #[test]
    fn a_started_contact_is_assigned_slot_0_and_maps_to_down_at_the_logical_position() {
        let mut tracker = TouchTracker::new();
        let down = tracker
            .touch(
                7,
                WinitTouchPhase::Started,
                PhysicalPosition::new(200.0, 100.0),
                2.0,
            )
            .expect("a first contact is assigned slot 0");
        assert_eq!(
            down,
            InputEvent::PointerContact {
                pointer_id: PointerId::touch(0),
                event: PointerEvent {
                    phase: PointerPhase::Down,
                    position: Point::new(100.0, 50.0),
                    button: PointerButton::Primary,
                }
            }
        );
    }

    #[test]
    fn the_tracked_contact_s_move_and_end_both_map() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                7,
                WinitTouchPhase::Started,
                PhysicalPosition::new(200.0, 100.0),
                2.0,
            )
            .expect("the contact starts");

        let moved = tracker
            .touch(
                7,
                WinitTouchPhase::Moved,
                PhysicalPosition::new(220.0, 100.0),
                2.0,
            )
            .expect("the contact's move maps");
        assert!(matches!(
            moved,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                event: PointerEvent {
                    phase: PointerPhase::Move,
                    ..
                }
            }
        ));

        let ended = tracker
            .touch(
                7,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(220.0, 100.0),
                2.0,
            )
            .expect("the contact's end maps");
        assert!(matches!(
            ended,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                event: PointerEvent {
                    phase: PointerPhase::Up,
                    ..
                }
            }
        ));
    }

    #[test]
    fn two_concurrent_contacts_produce_two_different_slot_ids() {
        let mut tracker = TouchTracker::new();
        let down1 = tracker
            .touch(
                1,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("the first finger is assigned slot 0");
        assert!(matches!(
            down1,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                ..
            }
        ));

        // A second finger starting while the first is still down gets slot 1.
        let down2 = tracker
            .touch(
                2,
                WinitTouchPhase::Started,
                PhysicalPosition::new(10.0, 10.0),
                1.0,
            )
            .expect("the second finger is assigned slot 1");
        assert!(matches!(
            down2,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 1 },
                ..
            }
        ));

        // Both fingers' moves are forwarded with their respective slots.
        let moved1 = tracker
            .touch(
                1,
                WinitTouchPhase::Moved,
                PhysicalPosition::new(1.0, 1.0),
                1.0,
            )
            .expect("the first finger's move maps");
        assert!(matches!(
            moved1,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                ..
            }
        ));

        let moved2 = tracker
            .touch(
                2,
                WinitTouchPhase::Moved,
                PhysicalPosition::new(12.0, 10.0),
                1.0,
            )
            .expect("the second finger's move maps");
        assert!(matches!(
            moved2,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 1 },
                ..
            }
        ));
    }

    #[test]
    fn a_finished_contact_resets_slots_for_a_new_gesture() {
        let mut tracker = TouchTracker::new();
        let down1 = tracker
            .touch(
                3,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("the first contact is assigned slot 0");
        assert!(matches!(
            down1,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                ..
            }
        ));

        tracker
            .touch(
                3,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("its end maps and resets the slot counter");

        // winit may reuse a finger id once its contact has ended; a fresh
        // `Started` with the same id opens a new gesture at slot 0 again.
        let down2 = tracker
            .touch(
                3,
                WinitTouchPhase::Started,
                PhysicalPosition::new(5.0, 5.0),
                1.0,
            )
            .expect("the reused id is assigned slot 0 again");
        assert!(matches!(
            down2,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                ..
            }
        ));
    }

    #[test]
    fn cancel_also_resets_slots_for_a_new_gesture() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                9,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("the contact is assigned slot 0");
        let cancelled = tracker
            .touch(
                9,
                WinitTouchPhase::Cancelled,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("a cancel for the contact maps");
        assert!(matches!(
            cancelled,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                event: PointerEvent {
                    phase: PointerPhase::Cancel,
                    ..
                }
            }
        ));

        let down = tracker
            .touch(
                9,
                WinitTouchPhase::Started,
                PhysicalPosition::new(1.0, 1.0),
                1.0,
            )
            .expect("the reused id is assigned slot 0 again");
        assert!(matches!(
            down,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                ..
            }
        ));
    }

    #[test]
    fn a_move_or_end_for_an_id_that_was_never_started_is_dropped() {
        // Nothing has been started at all yet — an id showing up for `Moved`
        // or `Ended` first (a malformed or out-of-order event stream) must not
        // be adopted as the tracked pointer by accident.
        let mut tracker = TouchTracker::new();
        assert!(
            tracker
                .touch(
                    4,
                    WinitTouchPhase::Moved,
                    PhysicalPosition::new(0.0, 0.0),
                    1.0,
                )
                .is_none()
        );
        assert!(
            tracker
                .touch(
                    4,
                    WinitTouchPhase::Ended,
                    PhysicalPosition::new(0.0, 0.0),
                    1.0,
                )
                .is_none()
        );
    }

    #[test]
    fn releasing_the_first_contact_keeps_the_second_s_slot_id() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                1,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("contact 1 assigned slot 0");
        let down2 = tracker
            .touch(
                2,
                WinitTouchPhase::Started,
                PhysicalPosition::new(10.0, 10.0),
                1.0,
            )
            .expect("contact 2 assigned slot 1");
        let slot_for_2 = match down2 {
            InputEvent::PointerContact {
                pointer_id: PointerId { slot, .. },
                ..
            } => slot,
            _ => panic!("expected PointerContact"),
        };
        assert_eq!(slot_for_2, 1);

        // Release the first contact.
        tracker
            .touch(
                1,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("contact 1 ends");

        // The second contact's slot is still intact.
        let moved2 = tracker
            .touch(
                2,
                WinitTouchPhase::Moved,
                PhysicalPosition::new(12.0, 10.0),
                1.0,
            )
            .expect("contact 2 move still maps");
        assert!(matches!(
            moved2,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 1 },
                ..
            }
        ));
    }

    #[test]
    fn a_third_contact_after_all_are_up_starts_at_slot_0() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                1,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("contact 1 assigned slot 0");
        let down2 = tracker
            .touch(
                2,
                WinitTouchPhase::Started,
                PhysicalPosition::new(10.0, 10.0),
                1.0,
            )
            .expect("contact 2 assigned slot 1");
        assert!(matches!(
            down2,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 1 },
                ..
            }
        ));

        // Release both contacts.
        tracker
            .touch(
                1,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("contact 1 ends");
        tracker
            .touch(
                2,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(10.0, 10.0),
                1.0,
            )
            .expect("contact 2 ends");

        // A new contact (even with a reused id) starts at slot 0 again.
        let down3 = tracker
            .touch(
                3,
                WinitTouchPhase::Started,
                PhysicalPosition::new(5.0, 5.0),
                1.0,
            )
            .expect("contact 3 assigned slot 0");
        assert!(matches!(
            down3,
            InputEvent::PointerContact {
                pointer_id: PointerId { source: _, slot: 0 },
                ..
            }
        ));
    }
}
