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

use crate::engine::{ActiveModal, AppState, Screen};
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

    // `active_modal` is the single priority source (G3) shared with
    // `crate::runner::translate_key`'s key routing — an exhaustive match
    // here means a new modal variant that isn't handled fails to compile
    // rather than silently missing its suppression/overlay dispatch.
    if let Some(modal) = state.active_modal() {
        render_modal(frame, area, state, theme, mouse, modal);
        return;
    }

    match state.screen {
        Screen::Welcome => render_welcome(frame, area, state, theme, mouse),
        Screen::Workbench => views::workbench::render(frame, area, state, theme, mouse),
    }
}

/// Render the currently-active modal (`state.active_modal()`) over its base
/// layer. Every arm shares the same D4 base-layer suppression: the chrome
/// beneath draws with no live mouse regions (a `MouseCtx::suppressed()`), so
/// only the topmost modal's regions are live — `translate_key`
/// (`crate::runner`) enforces the matching keyboard exclusivity.
fn render_modal(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
    modal: ActiveModal,
) {
    match modal {
        // The create wizard is the top-priority modal and can appear over
        // either screen, unlike the rest (workbench-only).
        ActiveModal::CreateWizard(wizard) => {
            let mut suppressed = MouseCtx::suppressed();
            match state.screen {
                Screen::Welcome => render_welcome(frame, area, state, theme, &mut suppressed),
                Screen::Workbench => {
                    views::workbench::render(frame, area, state, theme, &mut suppressed)
                }
            }
            views::create_wizard::render(frame, area, wizard, theme, mouse);
        }
        // The bootstrap wizard also appears over either screen (the toolchain
        // chip shows on the welcome splash too).
        ActiveModal::Bootstrap(wizard) => {
            let mut suppressed = MouseCtx::suppressed();
            match state.screen {
                Screen::Welcome => render_welcome(frame, area, state, theme, &mut suppressed),
                Screen::Workbench => {
                    views::workbench::render(frame, area, state, theme, &mut suppressed)
                }
            }
            views::bootstrap::render(frame, area, wizard, theme, mouse);
        }
        ActiveModal::RunConfig(modal) => {
            let mut suppressed = MouseCtx::suppressed();
            views::workbench::render(frame, area, state, theme, &mut suppressed);
            views::run_config::render(frame, area, modal, theme, mouse);
        }
        ActiveModal::ProjectSwitcher => {
            let mut suppressed = MouseCtx::suppressed();
            views::workbench::render(frame, area, state, theme, &mut suppressed);
            views::project_switcher::render(frame, area, state, theme, mouse);
        }
        ActiveModal::DoctorPanel => {
            let mut suppressed = MouseCtx::suppressed();
            views::workbench::render(frame, area, state, theme, &mut suppressed);
            views::doctor::render(frame, area, &state.doctor, theme, mouse);
        }
        ActiveModal::BuildLauncher(launcher) => {
            let mut suppressed = MouseCtx::suppressed();
            views::workbench::render(frame, area, state, theme, &mut suppressed);
            views::build_launcher::render(frame, area, launcher, theme, mouse);
        }
        ActiveModal::CleanConfirm(project_root) => {
            let mut suppressed = MouseCtx::suppressed();
            views::workbench::render(frame, area, state, theme, &mut suppressed);
            views::clean_confirm::render(frame, area, project_root, theme, mouse);
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

    views::welcome::titlebar(frame, rows[0], state, theme, mouse);
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
