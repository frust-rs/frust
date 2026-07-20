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

    // A right-click context menu (T04 / D4) floats on the top z-layer over the
    // still-visible base layer, which is drawn with a *suppressed* `MouseCtx`
    // so only the menu's own rows are hit-testable while it's open — the same
    // D4 base-layer suppression the workbench modals use.
    if let Some(menu) = &state.context_menu {
        let mut suppressed = MouseCtx::suppressed();
        render_base(frame, area, state, theme, &mut suppressed);
        views::context_menu::render(frame, area, menu, theme, mouse);
    } else {
        render_base(frame, area, state, theme, mouse);
    }

    // Toasts float in the status-bar area layer, over everything else (even a
    // modal — an error toast surfaces regardless). Render-only: the tick loop
    // ages them, this pass just paints the current stack.
    render_toasts(frame, area, state, theme);
}

/// The base layer under any context menu: the active modal (if any) over its
/// suppressed backdrop, else the current top-level screen.
///
/// `active_modal` is the single priority source (G3) shared with
/// `crate::runner::translate_key`'s key routing — an exhaustive match in
/// [`render_modal`] means a new modal variant that isn't handled fails to
/// compile rather than silently missing its suppression/overlay dispatch.
fn render_base(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if let Some(modal) = state.active_modal() {
        render_modal(frame, area, state, theme, mouse, modal);
    } else {
        match state.screen {
            Screen::Welcome => render_welcome(frame, area, state, theme, mouse),
            Screen::Workbench => views::workbench::render(frame, area, state, theme, mouse),
        }
    }
}

/// Render the auto-dismiss toast stack (D5) bottom-anchored just above the
/// 1-row status bar, oldest-to-newest top-to-bottom, each colored by kind. A
/// no-op when the stack is empty (the common case).
fn render_toasts(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    use crate::engine::ToastKind;

    let toasts = &state.toasts.items;
    if toasts.is_empty() || area.height < 2 {
        return;
    }

    // One row per toast, stacked directly above the status bar (the last row).
    let rows = (toasts.len() as u16).min(area.height.saturating_sub(1));
    let start_y = area.bottom().saturating_sub(1 + rows);
    for (i, toast) in toasts.iter().rev().take(rows as usize).enumerate() {
        let y = area.bottom().saturating_sub(2 + i as u16);
        if y < start_y {
            break;
        }
        let (glyph, color) = match toast.kind {
            ToastKind::Info => ("\u{2139}", theme.accent()),
            ToastKind::Success => ("\u{2713}", theme.success()),
            ToastKind::Warn => ("!", theme.warn()),
            ToastKind::Error => ("\u{2717}", theme.error()),
        };
        let text = format!(" {glyph} {} ", toast.text);
        let w = (text.chars().count() as u16).min(area.width);
        let x = area.right().saturating_sub(w);
        let rect = Rect::new(x, y, w, 1);
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(color).bg(theme.overlay()),
            )))
            .alignment(Alignment::Right)
            .style(Style::default().bg(theme.overlay())),
            rect,
        );
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
        // The command palette is the top-priority modal and can appear over
        // either top-level screen.
        ActiveModal::Palette(_) => {
            let mut suppressed = MouseCtx::suppressed();
            match state.screen {
                Screen::Welcome => render_welcome(frame, area, state, theme, &mut suppressed),
                Screen::Workbench => {
                    views::workbench::render(frame, area, state, theme, &mut suppressed)
                }
            }
            views::palette::render(frame, area, state, theme, mouse);
        }
        // The create wizard can appear over either screen, unlike the rest
        // (workbench-only).
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
        // The Add Plugin dialog can appear over either screen (reachable via the
        // welcome-screen `a`/palette, though it needs an open project to open).
        ActiveModal::AddPlugin(dialog) => {
            let mut suppressed = MouseCtx::suppressed();
            match state.screen {
                Screen::Welcome => render_welcome(frame, area, state, theme, &mut suppressed),
                Screen::Workbench => {
                    views::workbench::render(frame, area, state, theme, &mut suppressed)
                }
            }
            views::add_plugin::render(frame, area, dialog, theme, mouse);
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
        // The help overlay can appear over either top-level screen, like the
        // wizards above (both status bars carry the `? help` hint).
        ActiveModal::HelpOverlay => {
            let mut suppressed = MouseCtx::suppressed();
            match state.screen {
                Screen::Welcome => render_welcome(frame, area, state, theme, &mut suppressed),
                Screen::Workbench => {
                    views::workbench::render(frame, area, state, theme, &mut suppressed)
                }
            }
            views::help::render(frame, area, state, theme, mouse);
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
    let left = Line::from(Span::styled(
        "? help · ⌘ palette · a add plugin · q quit",
        Style::default().fg(theme.muted()),
    ));
    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(theme.surface())),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            mouse_indicator(state),
            Style::default().fg(theme.muted()),
        ))
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.surface())),
        area,
    );
}

/// The status-bar mouse-capture indicator (T04 / D4): a check while capture is
/// on, an "off" hint (with the `⌥m` toggle key) while it's off so users know
/// the terminal's own text selection is available. Shared by the welcome and
/// workbench status bars.
pub(crate) fn mouse_indicator(state: &AppState) -> &'static str {
    if state.mouse_capture {
        "[mouse ✓]"
    } else {
        "[mouse off · ⌥m]"
    }
}
