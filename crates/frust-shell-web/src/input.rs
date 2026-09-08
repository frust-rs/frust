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
//! # Why single-pointer, like the mobile shells
//!
//! `frust_core::event::PointerEvent` carries no contact id — it was designed
//! against the mobile shells' v1 contract, where Kotlin/the iOS bridge forward
//! only the primary finger (see `frust-shell-android`'s `dispatch_touch` and
//! its own `TouchPhase`). A browser's `WindowEvent::Touch` *does* carry a
//! per-finger `id` (multiple concurrent contacts are routine — two-finger
//! scroll, pinch), but there is nowhere in the framework's pointer vocabulary
//! to carry a second one. [`TouchTracker`] therefore adopts the same v1 rule
//! the mobile shells already ship: the first concurrent contact is tracked as
//! *the* pointer, and every other contact that starts while it is still down
//! is dropped outright (not merely unmapped — its own `Ended`/`Cancelled` is
//! dropped too, so it never produces an unpaired release). This keeps the
//! three host tiers that have touch at all telling `PointerEvent` the same
//! story, rather than this shell inventing a multi-touch protocol the
//! framework has no consumer for.

use frust_core::event::{InputEvent, PointerButton, PointerEvent, PointerPhase};
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

/// Tracks which single winit touch `id` (if any) is the pointer this shell is
/// currently forwarding, so a second concurrent finger is dropped rather than
/// stomping the first one's gesture — see the module doc for why
/// single-pointer is the deliberate v1 contract here, matching the mobile
/// shells.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TouchTracker {
    active_id: Option<u64>,
}

impl TouchTracker {
    /// A fresh tracker: no contact currently owns the pointer.
    pub fn new() -> Self {
        Self::default()
    }

    /// One winit touch contact — its stable per-finger `id`, phase, and
    /// device-physical location — mapped to an [`InputEvent::Pointer`], or
    /// `None` when the contact is not the one being tracked.
    ///
    /// - A `Started` while nothing is tracked adopts `id` as the pointer and
    ///   maps through as `Down`. A `Started` while another `id` is already
    ///   tracked is a second concurrent finger and is dropped.
    /// - A `Moved`/`Ended`/`Cancelled` maps through only when `id` matches the
    ///   tracked contact; the tracked id is cleared on `Ended`/`Cancelled` so a
    ///   later `Started` with a reused id (winit's own contract — ids may be
    ///   reused once a contact ends) is free to adopt the pointer again.
    /// - A `Moved`/`Ended`/`Cancelled` for an untracked (dropped-at-`Started`)
    ///   id is dropped too — its `Started` was never delivered, so delivering
    ///   any of its later phases would hand the tree an unpaired event.
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
                if self.active_id.is_some() {
                    return None;
                }
                self.active_id = Some(id);
            }
            PointerPhase::Move => {
                if self.active_id != Some(id) {
                    return None;
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if self.active_id != Some(id) {
                    return None;
                }
                self.active_id = None;
            }
        }
        let position = physical_to_logical(location.x, location.y, scale);
        Some(InputEvent::Pointer(PointerEvent {
            phase: core_phase,
            position,
            button: PointerButton::Primary,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{TouchTracker, WinitTouchPhase, map_touch_phase};
    use frust_core::event::{InputEvent, PointerButton, PointerEvent, PointerPhase};
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
    fn a_started_contact_is_tracked_and_maps_to_down_at_the_logical_position() {
        let mut tracker = TouchTracker::new();
        let down = tracker
            .touch(
                7,
                WinitTouchPhase::Started,
                PhysicalPosition::new(200.0, 100.0),
                2.0,
            )
            .expect("a first contact starts the tracked pointer");
        assert_eq!(
            down,
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(100.0, 50.0),
                button: PointerButton::Primary,
            })
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
            .expect("the tracked contact's move maps");
        assert!(matches!(
            moved,
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                ..
            })
        ));

        let ended = tracker
            .touch(
                7,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(220.0, 100.0),
                2.0,
            )
            .expect("the tracked contact's end maps");
        assert!(matches!(
            ended,
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Up,
                ..
            })
        ));
    }

    #[test]
    fn a_second_concurrent_contact_is_dropped_start_to_finish() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                1,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("the first finger is tracked");

        // A second finger starting while the first is still down is dropped.
        assert!(
            tracker
                .touch(
                    2,
                    WinitTouchPhase::Started,
                    PhysicalPosition::new(10.0, 10.0),
                    1.0,
                )
                .is_none()
        );
        // Its move and end are dropped too — its `Started` was never
        // delivered, so nothing about it should reach the tree.
        assert!(
            tracker
                .touch(
                    2,
                    WinitTouchPhase::Moved,
                    PhysicalPosition::new(12.0, 10.0),
                    1.0,
                )
                .is_none()
        );
        assert!(
            tracker
                .touch(
                    2,
                    WinitTouchPhase::Ended,
                    PhysicalPosition::new(12.0, 10.0),
                    1.0,
                )
                .is_none()
        );

        // The first finger's own gesture is unaffected by the dropped second one.
        assert!(
            tracker
                .touch(
                    1,
                    WinitTouchPhase::Moved,
                    PhysicalPosition::new(1.0, 1.0),
                    1.0,
                )
                .is_some()
        );
    }

    #[test]
    fn a_finished_contact_frees_the_tracker_for_a_reused_id() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                3,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("the first contact is tracked");
        tracker
            .touch(
                3,
                WinitTouchPhase::Ended,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("its end maps and frees the tracker");

        // winit may reuse a finger id once its contact has ended; a fresh
        // `Started` with the same id must be free to adopt the pointer again.
        assert!(
            tracker
                .touch(
                    3,
                    WinitTouchPhase::Started,
                    PhysicalPosition::new(5.0, 5.0),
                    1.0,
                )
                .is_some()
        );
    }

    #[test]
    fn cancel_also_frees_the_tracker() {
        let mut tracker = TouchTracker::new();
        tracker
            .touch(
                9,
                WinitTouchPhase::Started,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("the contact is tracked");
        let cancelled = tracker
            .touch(
                9,
                WinitTouchPhase::Cancelled,
                PhysicalPosition::new(0.0, 0.0),
                1.0,
            )
            .expect("a cancel for the tracked id maps");
        assert!(matches!(
            cancelled,
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Cancel,
                ..
            })
        ));

        assert!(
            tracker
                .touch(
                    9,
                    WinitTouchPhase::Started,
                    PhysicalPosition::new(1.0, 1.0),
                    1.0,
                )
                .is_some()
        );
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
}
