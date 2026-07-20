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

    match state.screen {
        Screen::Welcome => render_welcome(frame, area, state, theme, mouse),
        Screen::Workbench => views::workbench::render(frame, area, state, theme),
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
