//! The pure TEA transition function.
//!
//! [`update`] is the *single mutation point*: it takes the model and one
//! message and returns whether the frame is now dirty. It performs no I/O and
//! reads no clock, so every transition is unit-testable without a terminal
//! (see the tests below).

use super::message::Message;
use super::state::{AppState, CREATE_TOAST};

/// What the loop must do after a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// `true` when the visible state changed and the next frame must be drawn.
    /// The event loop ORs these across a turn and skips `terminal.draw` when it
    /// stays `false` (dirty-frame skip — helps CPU and screen readers).
    pub redraw: bool,
}

impl Outcome {
    const REDRAW: Outcome = Outcome { redraw: true };
    const IDLE: Outcome = Outcome { redraw: false };
}

/// Apply `msg` to `state`, returning whether a redraw is warranted.
pub fn update(state: &mut AppState, msg: Message) -> Outcome {
    match msg {
        Message::Quit => {
            state.should_quit = true;
            // No point drawing a frame we're about to tear down.
            Outcome::IDLE
        }
        // The skeleton animates nothing, so a bare tick never dirties a frame.
        Message::Tick => Outcome::IDLE,
        Message::Resize(_, _) => Outcome::REDRAW,
        Message::HoverChanged(next) => {
            if state.hover == next {
                Outcome::IDLE
            } else {
                state.hover = next;
                Outcome::REDRAW
            }
        }
        Message::CreatePressed => {
            if state.create_pressed {
                Outcome::IDLE
            } else {
                state.create_pressed = true;
                Outcome::REDRAW
            }
        }
        Message::CreateActivate => {
            state.create_pressed = false;
            state.toast = Some(CREATE_TOAST.to_string());
            Outcome::REDRAW
        }
        Message::CreateCancel => {
            if state.create_pressed {
                state.create_pressed = false;
                Outcome::REDRAW
            } else {
                Outcome::IDLE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::message::RegionId;
    use crate::engine::state::Screen;

    fn welcome() -> AppState {
        AppState::default()
    }

    #[test]
    fn quit_sets_flag_without_redraw() {
        let mut s = welcome();
        let out = update(&mut s, Message::Quit);
        assert!(s.should_quit);
        assert!(!out.redraw, "a quit does not need a paint");
    }

    #[test]
    fn tick_is_a_dirty_frame_skip_when_idle() {
        let mut s = welcome();
        let out = update(&mut s, Message::Tick);
        assert!(!out.redraw, "no animation → tick skips the draw");
    }

    #[test]
    fn hover_change_redraws_once_then_dedups() {
        let mut s = welcome();
        assert_eq!(s.hover, None);
        let first = update(&mut s, Message::HoverChanged(Some(RegionId::CreateButton)));
        assert!(first.redraw);
        assert_eq!(s.hover, Some(RegionId::CreateButton));
        // Same hover again: no state change, no redraw.
        let again = update(&mut s, Message::HoverChanged(Some(RegionId::CreateButton)));
        assert!(!again.redraw, "unchanged hover is a dirty-frame skip");
        // Clearing the hover redraws.
        let cleared = update(&mut s, Message::HoverChanged(None));
        assert!(cleared.redraw);
        assert_eq!(s.hover, None);
    }

    #[test]
    fn press_then_activate_shows_toast_and_clears_pressed() {
        let mut s = welcome();
        assert!(update(&mut s, Message::CreatePressed).redraw);
        assert!(s.create_pressed);
        // A second press while already pressed is a no-op.
        assert!(!update(&mut s, Message::CreatePressed).redraw);

        assert!(update(&mut s, Message::CreateActivate).redraw);
        assert!(!s.create_pressed);
        assert_eq!(s.toast.as_deref(), Some(CREATE_TOAST));
    }

    #[test]
    fn cancel_clears_pressed_only_when_set() {
        let mut s = welcome();
        assert!(!update(&mut s, Message::CreateCancel).redraw);
        s.create_pressed = true;
        assert!(update(&mut s, Message::CreateCancel).redraw);
        assert!(!s.create_pressed);
    }

    #[test]
    fn detect_picks_screen_from_marker() {
        // No filesystem marker → welcome.
        let s = AppState::default();
        assert_eq!(s.screen, Screen::Welcome);
    }
}
