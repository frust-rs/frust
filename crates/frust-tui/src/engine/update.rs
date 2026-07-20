//! The pure TEA transition function.
//!
//! [`update`] is the *single mutation point*: it takes the model and one
//! message and returns an [`Outcome`] — whether the frame is now dirty, plus an
//! optional [`Effect`] the runner performs (the only I/O the pure core cannot
//! do itself: killing a session through the supervisor, or writing the
//! clipboard). It performs no I/O and reads no clock, so every transition is
//! unit-testable without a terminal (see the tests below).

use super::message::Message;
use super::session_view::SessionView;
use super::state::{AppState, CREATE_TOAST};
use crate::supervise::{SessionEvent, SessionEventKind, SessionId};

/// A side effect the (terminal/supervisor-owning) runner performs after a
/// transition — the pure core requests it, the runner enacts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Stop a session: route to `Supervisor::stop` (the group-kill).
    StopSession(SessionId),
    /// Copy text to the system clipboard (the runner emits an OSC 52 sequence).
    Copy(String),
}

/// What the loop must do after a transition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `true` when the visible state changed and the next frame must be drawn.
    /// The event loop ORs these across a turn and skips `terminal.draw` when it
    /// stays `false` (dirty-frame skip — helps CPU and screen readers).
    pub redraw: bool,
    /// A side effect for the runner to perform (stop a session, copy text).
    pub effect: Option<Effect>,
}

impl Outcome {
    /// No redraw, no effect.
    fn idle() -> Self {
        Self::default()
    }

    /// Redraw, no effect.
    fn redraw() -> Self {
        Self {
            redraw: true,
            effect: None,
        }
    }

    /// A redraw iff `dirty`.
    fn dirty(dirty: bool) -> Self {
        Self {
            redraw: dirty,
            effect: None,
        }
    }

    /// An effect with no redraw (the visible change follows from a later
    /// message — e.g. the supervisor's `Killed` event).
    fn effect(effect: Effect) -> Self {
        Self {
            redraw: false,
            effect: Some(effect),
        }
    }
}

/// Apply `msg` to `state`, returning whether a redraw is warranted and any
/// effect the runner must perform.
pub fn update(state: &mut AppState, msg: Message) -> Outcome {
    match msg {
        Message::Quit => {
            state.should_quit = true;
            // No point drawing a frame we're about to tear down.
            Outcome::idle()
        }
        // The skeleton animates nothing, so a bare tick never dirties a frame.
        Message::Tick => Outcome::idle(),
        Message::Resize(_, _) => Outcome::redraw(),
        Message::HoverChanged(next) => {
            if state.hover == next {
                Outcome::idle()
            } else {
                state.hover = next;
                Outcome::redraw()
            }
        }
        Message::CreatePressed => {
            if state.create_pressed {
                Outcome::idle()
            } else {
                state.create_pressed = true;
                Outcome::redraw()
            }
        }
        Message::CreateActivate => {
            state.create_pressed = false;
            state.toast = Some(CREATE_TOAST.to_string());
            Outcome::redraw()
        }
        Message::CreateCancel => {
            if state.create_pressed {
                state.create_pressed = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        Message::Session(ev) => on_session_event(state, ev),
        Message::RegisterSession {
            id,
            project_root,
            target_label,
        } => {
            if state.session_index(id).is_some() {
                return Outcome::idle();
            }
            state
                .sessions
                .push(SessionView::new(id, project_root, target_label));
            // Auto-select the first session that appears.
            if state.active_session.is_none() {
                state.active_session = Some(state.sessions.len() - 1);
            }
            Outcome::redraw()
        }
        Message::NextTab => cycle_tab(state, 1),
        Message::PrevTab => cycle_tab(state, -1),
        Message::SelectTab(i) => {
            if i < state.sessions.len() && state.active_session != Some(i) {
                state.active_session = Some(i);
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::StopSession => match state.active_session().map(|s| s.id) {
            Some(id) => Outcome::effect(Effect::StopSession(id)),
            None => Outcome::idle(),
        },
        Message::ToggleFollow => with_active(state, |s| s.toggle_follow()),
        Message::ToggleWrap => {
            state.wrap = !state.wrap;
            Outcome::redraw()
        }
        Message::LogScrollUp(n) => with_active(state, |s| s.scroll_up(n)),
        Message::LogScrollDown(n) => with_active(state, |s| s.scroll_down(n)),
        Message::LogScrollToTop => with_active(state, |s| s.scroll_to_top()),
        Message::LogScrollToBottom => with_active(state, |s| s.scroll_to_bottom()),

        Message::SearchOpen => {
            state.search.open = true;
            state.search.query = state.search.filter.clone().unwrap_or_default();
            Outcome::redraw()
        }
        Message::SearchInput(c) => {
            if state.search.open {
                state.search.query.push(c);
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::SearchBackspace => {
            if state.search.open {
                state.search.query.pop();
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::SearchCommit => {
            if !state.search.open {
                return Outcome::idle();
            }
            state.search.open = false;
            let q = std::mem::take(&mut state.search.query);
            state.search.filter = (!q.is_empty()).then_some(q);
            // Jump to the latest match so the committed filter shows fresh output.
            if let Some(s) = state.active_session_mut() {
                s.scroll_to_bottom();
            }
            Outcome::redraw()
        }
        Message::SearchCancel => {
            if state.search.open {
                state.search.open = false;
                state.search.query.clear();
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        Message::SelectionBegin => with_active(state, |s| s.begin_selection()),
        Message::SelectionExtendUp(n) => with_active(state, |s| s.extend_selection_up(n)),
        Message::SelectionExtendDown(n) => with_active(state, |s| s.extend_selection_down(n)),
        Message::SelectionClear => with_active(state, |s| s.clear_selection()),
        Message::CopySelection => match state.active_session().and_then(|s| s.selected_text()) {
            Some(text) => Outcome {
                redraw: false,
                effect: Some(Effect::Copy(text)),
            },
            None => Outcome::idle(),
        },
    }
}

/// Route one supervisor event into the matching session's view-model.
///
/// Redraw policy honors the dirty-frame skip: a **line** for a background
/// (non-active) session is buffered without a repaint; a line for the *active*
/// followed session, and *any* state change (the sidebar/tab status glyph),
/// redraw. An event for an unknown id is dropped (the session must be
/// registered first — see [`Message::RegisterSession`]).
fn on_session_event(state: &mut AppState, ev: SessionEvent) -> Outcome {
    let Some(idx) = state.session_index(ev.id) else {
        return Outcome::idle();
    };
    let is_active = state.active_session == Some(idx);
    let session = &mut state.sessions[idx];
    match ev.kind {
        SessionEventKind::Line(line) => {
            let following = session.is_following();
            session.push_line(line);
            Outcome::dirty(is_active && following)
        }
        SessionEventKind::State(s) => {
            session.state = s;
            Outcome::redraw()
        }
    }
}

/// Move the active tab by `delta` (wrapping), redrawing on a real change.
fn cycle_tab(state: &mut AppState, delta: isize) -> Outcome {
    let n = state.sessions.len();
    if n == 0 {
        return Outcome::idle();
    }
    let cur = state.active_session.unwrap_or(0) as isize;
    let next = (cur + delta).rem_euclid(n as isize) as usize;
    if state.active_session == Some(next) {
        Outcome::idle()
    } else {
        state.active_session = Some(next);
        Outcome::redraw()
    }
}

/// Apply `f` to the active session (if any) and redraw; idle when none.
fn with_active(state: &mut AppState, f: impl FnOnce(&mut SessionView)) -> Outcome {
    match state.active_session_mut() {
        Some(s) => {
            f(s);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::message::RegionId;
    use crate::engine::session_view::Scroll;
    use crate::engine::state::Screen;
    use crate::supervise::SessionState;
    use std::path::PathBuf;

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
        let again = update(&mut s, Message::HoverChanged(Some(RegionId::CreateButton)));
        assert!(!again.redraw, "unchanged hover is a dirty-frame skip");
        let cleared = update(&mut s, Message::HoverChanged(None));
        assert!(cleared.redraw);
        assert_eq!(s.hover, None);
    }

    #[test]
    fn press_then_activate_shows_toast_and_clears_pressed() {
        let mut s = welcome();
        assert!(update(&mut s, Message::CreatePressed).redraw);
        assert!(s.create_pressed);
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
        let s = AppState::default();
        assert_eq!(s.screen, Screen::Welcome);
    }

    // ── Session wiring ──────────────────────────────────────────────────────

    fn register(state: &mut AppState, id: u64, project: &str, label: &str) -> SessionId {
        let id = SessionId(id);
        update(
            state,
            Message::RegisterSession {
                id,
                project_root: PathBuf::from(project),
                target_label: label.to_string(),
            },
        );
        id
    }

    fn line(id: SessionId, s: &str) -> Message {
        Message::Session(SessionEvent {
            id,
            kind: SessionEventKind::Line(s.to_string()),
        })
    }

    #[test]
    fn register_adds_session_and_auto_selects_first() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/huddle", "desktop");
        assert_eq!(st.sessions.len(), 1);
        assert_eq!(st.active_session, Some(0));
        // A duplicate register is a no-op.
        let out = update(
            &mut st,
            Message::RegisterSession {
                id: a,
                project_root: PathBuf::from("/tmp/huddle"),
                target_label: "desktop".into(),
            },
        );
        assert!(!out.redraw);
        assert_eq!(st.sessions.len(), 1);
    }

    #[test]
    fn active_session_line_redraws_background_line_does_not() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        // `a` is active (auto-selected first).
        assert_eq!(st.active_session, Some(0));
        // A line for the active, following session repaints.
        assert!(update(&mut st, line(a, "hello")).redraw);
        // A line for the background session is buffered but skips the draw.
        assert!(!update(&mut st, line(b, "quiet")).redraw);
        // Yet it was retained.
        let bi = st.session_index(b).unwrap();
        assert_eq!(st.sessions[bi].log.len(), 1);
    }

    #[test]
    fn a_state_change_always_redraws_the_glyph() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let out = update(
            &mut st,
            Message::Session(SessionEvent {
                id: a,
                kind: SessionEventKind::State(SessionState::Running),
            }),
        );
        assert!(out.redraw);
        assert_eq!(st.sessions[0].state, SessionState::Running);
    }

    #[test]
    fn tab_cycling_wraps_and_select_jumps() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        register(&mut st, 1, "/tmp/b", "desktop");
        register(&mut st, 2, "/tmp/c", "desktop");
        assert_eq!(st.active_session, Some(0));
        assert!(update(&mut st, Message::NextTab).redraw);
        assert_eq!(st.active_session, Some(1));
        update(&mut st, Message::NextTab);
        update(&mut st, Message::NextTab); // wraps 2 -> 0
        assert_eq!(st.active_session, Some(0));
        assert!(update(&mut st, Message::PrevTab).redraw); // wraps 0 -> 2
        assert_eq!(st.active_session, Some(2));
        assert!(update(&mut st, Message::SelectTab(1)).redraw);
        assert_eq!(st.active_session, Some(1));
        // Out-of-range select is a no-op.
        assert!(!update(&mut st, Message::SelectTab(9)).redraw);
    }

    #[test]
    fn stop_routes_the_active_session_id_as_an_effect() {
        let mut st = welcome();
        let a = register(&mut st, 7, "/tmp/a", "desktop");
        let out = update(&mut st, Message::StopSession);
        assert_eq!(out.effect, Some(Effect::StopSession(a)));
        assert!(!out.redraw, "the Killed event will redraw, not the request");
        // With no session, stop is a no-op.
        let mut empty = welcome();
        assert_eq!(update(&mut empty, Message::StopSession).effect, None);
    }

    #[test]
    fn scroll_and_follow_route_to_the_active_session() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for i in 0..10 {
            update(&mut st, line(a, &format!("line {i}")));
        }
        assert!(st.active_session().unwrap().is_following());
        update(&mut st, Message::LogScrollUp(3));
        assert!(matches!(
            st.active_session().unwrap().scroll,
            Scroll::Anchored(_)
        ));
        update(&mut st, Message::ToggleFollow);
        // toggle from Anchored -> Follow
        assert!(st.active_session().unwrap().is_following());
    }

    #[test]
    fn search_open_type_commit_filters_and_cancel_restores() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        update(&mut st, Message::SearchOpen);
        assert!(st.search.open);
        update(&mut st, Message::SearchInput('e'));
        update(&mut st, Message::SearchInput('r'));
        update(&mut st, Message::SearchBackspace);
        update(&mut st, Message::SearchInput('r'));
        assert_eq!(st.search.query, "er");
        update(&mut st, Message::SearchCommit);
        assert!(!st.search.open);
        assert_eq!(st.search.filter.as_deref(), Some("er"));
        // Re-open then cancel leaves the committed filter intact.
        update(&mut st, Message::SearchOpen);
        update(&mut st, Message::SearchInput('x'));
        update(&mut st, Message::SearchCancel);
        assert!(!st.search.open);
        assert_eq!(st.search.filter.as_deref(), Some("er"));
    }

    #[test]
    fn copy_selection_emits_effect_with_the_selected_text() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for i in 0..5 {
            update(&mut st, line(a, &format!("line {i}")));
        }
        update(&mut st, Message::SelectionBegin); // anchors newest (line 4)
        update(&mut st, Message::SelectionExtendUp(2)); // -> lines 2..=4
        let out = update(&mut st, Message::CopySelection);
        assert_eq!(
            out.effect,
            Some(Effect::Copy("line 2\nline 3\nline 4".to_string()))
        );
    }
}
