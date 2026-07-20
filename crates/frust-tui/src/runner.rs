//! Terminal lifecycle and the tokio event loop.
//!
//! Owns everything the pure engine and render layers deliberately don't: raw
//! mode + mouse capture, a panic hook that restores the terminal before the
//! default hook prints, the tokio runtime's `select!` over the crossterm
//! `EventStream` / the engine channel / a tick interval, and the dirty-frame
//! skip (D2: only `terminal.draw` when the state changed or something is
//! animating).

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::EventStream;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton as CtMouseButton, MouseEventKind,
};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;

use crate::engine::{AppState, Engine, Message, RegionId};
use crate::ui::mouse::MouseRegions;
use crate::ui::theme::Theme;

/// Frame tick cadence. Cheap because of the dirty-frame skip — a tick only
/// forces a draw while something is animating (nothing, in Phase 1).
const TICK: Duration = Duration::from_millis(50);

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
                            needs_redraw |= engine.handle(msg).redraw;
                        }
                    }
                    // A read error (rare) is logged and ignored — the loop
                    // keeps running rather than tearing the terminal down.
                    Some(Err(e)) => eprintln!("frust-tui: terminal read error: {e}"),
                    None => break,
                }
            }
            Some(msg) = rx.recv() => {
                needs_redraw |= engine.handle(msg).redraw;
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

/// Translate one crossterm event into zero or more engine messages, using the
/// current frame's mouse regions for hit-testing.
fn translate_event(event: Event, state: &AppState, regions: &MouseRegions) -> Vec<Message> {
    match event {
        Event::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return vec![];
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Char('q') => vec![Message::Quit],
                // Ctrl+Q / Ctrl+C quit (no sessions to stop yet in Phase 1).
                KeyCode::Char('c') if ctrl => vec![Message::Quit],
                // Welcome keyboard parity: Enter / c activate the Create button.
                KeyCode::Char('c') => activate_create(state),
                KeyCode::Enter => activate_create(state),
                _ => vec![],
            }
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
