//! Terminal lifecycle and the tokio event loop.
//!
//! Owns everything the pure engine and render layers deliberately don't: raw
//! mode + mouse capture, a panic hook that restores the terminal before the
//! default hook prints, the tokio runtime's `select!` over the crossterm
//! `EventStream` / the engine channel / a tick interval, and the dirty-frame
//! skip (D2: only `terminal.draw` when the state changed or something is
//! animating).

use std::io::{self, Stdout, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::EventStream;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton as CtMouseButton, MouseEventKind,
};
use frust_drive::process::RealProcessRunner;
use futures_util::StreamExt;
use ratatui::DefaultTerminal;

use crate::engine::{AppState, Effect, Engine, Message, RegionId, Screen};
use crate::supervise::Supervisor;
use crate::ui::mouse::MouseRegions;
use crate::ui::theme::Theme;

/// Frame tick cadence. Cheap because of the dirty-frame skip — a tick only
/// forces a draw while something is animating (nothing, in Phase 1).
const TICK: Duration = Duration::from_millis(50);

/// Lines a `PageUp`/`PageDown` scrolls the log view. A fixed step (the event
/// translator has no viewport height); a comfortable page on typical panes.
const PAGE_LINES: u64 = 10;

/// Run the TUI: set up the terminal, run the loop, and restore on the way out
/// (including on panic, via the installed hook).
pub async fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    install_panic_hook();
    if let Err(e) = enable_mouse_capture() {
        // Non-fatal: the whole UI has keyboard parity, so a terminal that
        // rejects mouse capture still works.
        eprintln!("frust-tui: mouse capture unavailable: {e}");
    }

    let result = run_loop(&mut terminal).await;

    let _ = disable_mouse_capture();
    ratatui::restore();
    result
}

/// The main event loop.
async fn run_loop(terminal: &mut DefaultTerminal) -> Result<()> {
    let theme = Theme::frust_dark();
    let mut engine = Engine::new(AppState::new());
    let mut rx = engine.take_receiver();
    // The session supervisor and the channel every supervised session feeds.
    // Sessions are *started* by later tasks (run-config, TUI2-04); this loop
    // wires the channel and the kill/copy effect path so those tasks only add
    // start calls. On return the supervisor's `Drop` stops+joins every session.
    let (mut supervisor, mut session_rx) = Supervisor::new(Arc::new(RealProcessRunner));
    let mut regions = MouseRegions::new();
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    let mut needs_redraw = true;

    while !engine.state.should_quit {
        if needs_redraw {
            regions.begin_frame();
            terminal
                .draw(|frame| {
                    let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
                    crate::ui::render(frame, &engine.state, &theme, &mut ctx);
                })
                .context("drawing a frame")?;
            needs_redraw = false;
        }

        tokio::select! {
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        for msg in translate_event(event, &engine.state, &regions) {
                            let out = engine.handle(msg);
                            needs_redraw |= out.redraw;
                            apply_effect(out.effect, &mut supervisor);
                        }
                    }
                    // A read error (rare) is logged and ignored — the loop
                    // keeps running rather than tearing the terminal down.
                    Some(Err(e)) => eprintln!("frust-tui: terminal read error: {e}"),
                    None => break,
                }
            }
            Some(msg) = rx.recv() => {
                let out = engine.handle(msg);
                needs_redraw |= out.redraw;
                apply_effect(out.effect, &mut supervisor);
            }
            Some(ev) = session_rx.recv() => {
                let out = engine.handle(Message::Session(ev));
                needs_redraw |= out.redraw;
                apply_effect(out.effect, &mut supervisor);
            }
            _ = tick.tick() => {
                if engine.state.animating() {
                    needs_redraw = true;
                }
            }
        }
    }

    Ok(())
}

/// Enact an engine-requested [`Effect`] — the runner owns the two side effects
/// the pure engine can't perform: killing a session through the supervisor, and
/// writing the system clipboard.
fn apply_effect(effect: Option<Effect>, supervisor: &mut Supervisor) {
    match effect {
        Some(Effect::StopSession(id)) => supervisor.stop(id),
        Some(Effect::Copy(text)) => copy_to_clipboard(&text),
        None => {}
    }
}

/// Copy `text` to the terminal's clipboard via an OSC 52 escape (broadly
/// supported, no clipboard-crate dependency). Best-effort: a terminal that
/// ignores OSC 52 simply drops it.
fn copy_to_clipboard(text: &str) {
    let payload = base64_encode(text.as_bytes());
    let seq = format!("\u{1b}]52;c;{payload}\u{07}");
    let mut out = stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

/// Minimal standard base64 (no dependency) for the OSC 52 clipboard payload.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Translate one crossterm event into zero or more engine messages, using the
/// current frame's mouse regions for hit-testing.
fn translate_event(event: Event, state: &AppState, regions: &MouseRegions) -> Vec<Message> {
    match event {
        Event::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return vec![];
            }
            translate_key(key.code, key.modifiers, state)
        }
        Event::Mouse(m) => {
            let (x, y) = (m.column, m.row);
            match m.kind {
                MouseEventKind::Moved => vec![Message::HoverChanged(regions.hover_at(x, y))],
                MouseEventKind::Down(CtMouseButton::Left) => {
                    if regions.hover_at(x, y) == Some(RegionId::CreateButton) {
                        vec![Message::CreatePressed]
                    } else {
                        vec![]
                    }
                }
                MouseEventKind::Up(CtMouseButton::Left) => {
                    if state.create_pressed {
                        if regions.hover_at(x, y) == Some(RegionId::CreateButton) {
                            vec![Message::CreateActivate]
                        } else {
                            vec![Message::CreateCancel]
                        }
                    } else if let Some(msg) = regions.click_at(x, y) {
                        vec![msg]
                    } else {
                        vec![]
                    }
                }
                MouseEventKind::ScrollDown => regions.scroll_at(x, y, true).into_iter().collect(),
                MouseEventKind::ScrollUp => regions.scroll_at(x, y, false).into_iter().collect(),
                _ => vec![],
            }
        }
        Event::Resize(w, h) => vec![Message::Resize(w, h)],
        _ => vec![],
    }
}

/// Translate one key press into engine messages, honoring the current mode
/// (search overlay open vs. normal) and screen (welcome vs. workbench).
///
/// Every log-view / tab action here has a mouse counterpart (tab click, wheel
/// scroll) — keyboard is the primary path, mouse additive (CODE_STANDARDS' TUI
/// keyboard-parity policy).
fn translate_key(code: KeyCode, mods: KeyModifiers, state: &AppState) -> Vec<Message> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let shift = mods.contains(KeyModifiers::SHIFT);

    // Ctrl+Q always quits, even while typing a search.
    if ctrl && matches!(code, KeyCode::Char('q')) {
        return vec![Message::Quit];
    }

    // While the search overlay is open, keys edit the query.
    if state.search.open {
        return match code {
            KeyCode::Esc => vec![Message::SearchCancel],
            KeyCode::Enter => vec![Message::SearchCommit],
            KeyCode::Backspace => vec![Message::SearchBackspace],
            KeyCode::Char(c) if !ctrl => vec![Message::SearchInput(c)],
            _ => vec![],
        };
    }

    let has_active_session = state.active_session().is_some();
    let active_running = state
        .active_session()
        .is_some_and(|s| !s.state.is_terminal());

    // Ctrl+C stops the active running session, else falls through to quit.
    if ctrl && matches!(code, KeyCode::Char('c')) {
        return if active_running {
            vec![Message::StopSession]
        } else {
            vec![Message::Quit]
        };
    }

    match code {
        // Global quit.
        KeyCode::Char('q') => vec![Message::Quit],

        // Welcome keyboard parity: Enter / c activate the Create button.
        KeyCode::Char('c') if matches!(state.screen, Screen::Welcome) => activate_create(state),
        KeyCode::Enter if matches!(state.screen, Screen::Welcome) => activate_create(state),

        // ── Log-view / tab controls (only meaningful with a session open) ──
        KeyCode::Char('x') if has_active_session => vec![Message::StopSession],
        KeyCode::Tab if has_active_session => vec![Message::NextTab],
        KeyCode::BackTab if has_active_session => vec![Message::PrevTab],
        KeyCode::Char(c @ '1'..='9') if has_active_session => {
            vec![Message::SelectTab(c as usize - '1' as usize)]
        }
        KeyCode::Char('/') if has_active_session => vec![Message::SearchOpen],
        KeyCode::Char('f') if has_active_session => vec![Message::ToggleFollow],
        KeyCode::Char('w') if has_active_session => vec![Message::ToggleWrap],
        KeyCode::Char('v') if has_active_session => vec![Message::SelectionBegin],
        KeyCode::Char('y') if has_active_session => vec![Message::CopySelection],
        KeyCode::Up if has_active_session && shift => vec![Message::SelectionExtendUp(1)],
        KeyCode::Down if has_active_session && shift => vec![Message::SelectionExtendDown(1)],
        KeyCode::Up if has_active_session => vec![Message::LogScrollUp(1)],
        KeyCode::Down if has_active_session => vec![Message::LogScrollDown(1)],
        KeyCode::PageUp if has_active_session => vec![Message::LogScrollUp(PAGE_LINES)],
        KeyCode::PageDown if has_active_session => vec![Message::LogScrollDown(PAGE_LINES)],
        KeyCode::Home if has_active_session => vec![Message::LogScrollToTop],
        KeyCode::End if has_active_session => vec![Message::LogScrollToBottom],
        KeyCode::Esc
            if state
                .active_session()
                .is_some_and(|s| s.selection.is_some()) =>
        {
            vec![Message::SelectionClear]
        }
        _ => vec![],
    }
}

/// The Create action, emitted only from the welcome screen (keyboard parity
/// with the button click).
fn activate_create(state: &AppState) -> Vec<Message> {
    if matches!(state.screen, crate::engine::Screen::Welcome) {
        vec![Message::CreateActivate]
    } else {
        vec![]
    }
}

/// Install a panic hook that restores the terminal before the previous hook
/// runs. Hooks fire in install order here (we call the saved one last), so the
/// terminal is always usable by the time a backtrace prints.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_mouse_capture();
        ratatui::restore();
        previous(info);
    }));
}

fn stdout() -> Stdout {
    io::stdout()
}

fn enable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(stdout(), EnableMouseCapture)
}

fn disable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(stdout(), DisableMouseCapture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, MouseEvent};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn q_quits() {
        let state = AppState::default();
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Char('q')), &state, &regions),
            vec![Message::Quit]
        );
    }

    #[test]
    fn ctrl_q_quits() {
        let state = AppState::default();
        let regions = MouseRegions::new();
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(translate_event(ev, &state, &regions), vec![Message::Quit]);
    }

    #[test]
    fn enter_and_c_activate_create_on_welcome() {
        let state = AppState::default(); // welcome
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Enter), &state, &regions),
            vec![Message::CreateActivate]
        );
        assert_eq!(
            translate_event(key(KeyCode::Char('c')), &state, &regions),
            vec![Message::CreateActivate]
        );
    }

    #[test]
    fn hover_move_reports_region_under_cursor() {
        let state = AppState::default();
        let mut regions = MouseRegions::new();
        {
            let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
            ctx.button(
                ratatui::layout::Rect::new(0, 0, 10, 3),
                RegionId::CreateButton,
            );
        }
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![Message::HoverChanged(Some(RegionId::CreateButton))]
        );
    }

    #[test]
    fn press_then_release_inside_activates() {
        let mut state = AppState::default();
        let mut regions = MouseRegions::new();
        {
            let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
            ctx.button(
                ratatui::layout::Rect::new(0, 0, 10, 3),
                RegionId::CreateButton,
            );
        }
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(CtMouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(down, &state, &regions),
            vec![Message::CreatePressed]
        );
        state.create_pressed = true;
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(CtMouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(up, &state, &regions),
            vec![Message::CreateActivate]
        );
    }

    #[test]
    fn release_outside_pressed_cancels() {
        let state = AppState {
            create_pressed: true,
            ..Default::default()
        };
        let regions = MouseRegions::new(); // nothing under cursor
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(CtMouseButton::Left),
            column: 50,
            row: 50,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(up, &state, &regions),
            vec![Message::CreateCancel]
        );
    }
}
