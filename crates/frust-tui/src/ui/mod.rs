//! The render layer: given `&AppState`, paints a frame and registers this
//! frame's mouse regions through a [`MouseCtx`]. It never mutates the engine —
//! interaction is expressed only as registered regions the loop turns into
//! `Message`s (D2 layering).

pub mod layout;
pub mod mouse;
pub mod theme;
pub mod views;
pub mod widgets;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::engine::{AppState, Screen};
use mouse::MouseCtx;
use theme::Theme;

/// Paint the whole UI for the current `state` and register this frame's mouse
/// regions.
pub fn render(frame: &mut Frame, state: &AppState, theme: &Theme, mouse: &mut MouseCtx) {
    let area = frame.area();

    // Whole-screen background fill (near-black brand bg).
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg())),
        area,
    );

    if views::too_small::is_too_small(area) {
        views::too_small::render(frame, area, theme);
        return;
    }

    // The create wizard is the top-priority modal and can appear over either
    // screen: the base layer draws suppressed (no live regions), the wizard on
    // top with the live ctx — the D4 base-layer suppression.
    if let Some(wizard) = &state.create_wizard {
        let mut suppressed = MouseCtx::suppressed();
        match state.screen {
            Screen::Welcome => render_welcome(frame, area, state, theme, &mut suppressed),
            Screen::Workbench => {
                views::workbench::render(frame, area, state, theme, &mut suppressed)
            }
        }
        views::create_wizard::render(frame, area, wizard, theme, mouse);
        return;
    }

    match state.screen {
        Screen::Welcome => render_welcome(frame, area, state, theme, mouse),
        Screen::Workbench => {
            if let Some(modal) = &state.run_config {
                // D4 base-layer suppression: the workbench beneath the modal
                // registers no mouse regions (pass `None`), so only the modal's
                // regions are live while it is open.
                let mut suppressed = MouseCtx::suppressed();
                views::workbench::render(frame, area, state, theme, &mut suppressed);
                views::run_config::render(frame, area, modal, theme, mouse);
            } else if state.project_switcher_open {
                // Same D4 base-layer suppression as the run-config modal above,
                // for the titlebar project-switcher dropdown.
                let mut suppressed = MouseCtx::suppressed();
                views::workbench::render(frame, area, state, theme, &mut suppressed);
                views::project_switcher::render(frame, area, state, theme, mouse);
            } else {
                views::workbench::render(frame, area, state, theme, mouse);
            }
        }
    }
}

fn render_welcome(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // titlebar chip band
            Constraint::Min(1),    // splash
            Constraint::Length(1), // status bar
        ])
        .split(area);

    views::welcome::titlebar(frame, rows[0], theme);
    views::welcome::render(frame, rows[1], state, theme, mouse);
    welcome_status(frame, rows[2], state, theme);
}

fn welcome_status(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let left = match &state.toast {
        Some(msg) => Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(theme.accent()),
        )),
        None => Line::from(Span::styled(
            "? help · ⌘ palette · q quit",
            Style::default().fg(theme.muted()),
        )),
    };
    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(theme.surface())),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            "[mouse ✓]",
            Style::default().fg(theme.muted()),
        ))
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.surface())),
        area,
    );
}
