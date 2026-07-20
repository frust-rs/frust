//! The titlebar project-switcher dropdown (PLAN.md D6b / F5): a popup
//! overlaying the sidebar's PROJECTS section (same column range — full
//! sidebar width, flush with its left edge — so it never bleeds a stray
//! sliver of the section it's replacing on either side), listing every
//! detected/recent project. Opened via the titlebar `▾` chevron or
//! `Ctrl+O`/`p`; the base workbench layer renders with a *suppressed*
//! `MouseCtx` while this is open (see `crate::ui::render`) — the same D4
//! base-layer suppression the run-config modal uses.
//!
//! Layering (D2): renders `&AppState` and only *registers* interaction (item
//! clicks); it never mutates the engine.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::engine::{AppState, Message, RegionId};
use crate::ui::layout::{Shell, sidebar_main};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Render the dropdown over the sidebar's PROJECTS section, registering one
/// click region per listed project.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if state.projects.is_empty() {
        return;
    }

    // Re-derive the sidebar's rect from the same layout constants the
    // workbench shell uses, so the dropdown always lands exactly over the
    // PROJECTS section regardless of terminal size.
    let shell = Shell::split(area);
    let (sidebar, _main) = sidebar_main(shell.body);

    let height = (state.projects.len() as u16 + 2).min(sidebar.height);
    if height < 3 {
        // Not enough room to show the border plus at least one row.
        return;
    }
    let box_ = Rect::new(sidebar.x, sidebar.y, sidebar.width, height);

    // Clear the popup footprint so the workbench behind it doesn't bleed
    // through (same as the run-config modal).
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .style(Style::default().bg(theme.surface()));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(state.projects.len());
    for (i, project) in state.projects.iter().enumerate() {
        let name = project
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.to_string_lossy().into_owned());
        let active = state.project_root.as_deref() == Some(project.as_path());
        let cursor = state.project_switcher_cursor == i;
        let marker = if cursor {
            format!(" {} ", theme.icons.chevron())
        } else {
            "   ".to_string()
        };
        let style = if active {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else if cursor {
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted())
        };
        lines.push(Line::styled(format!("{marker}{name}"), style));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    for i in 0..state.projects.len() {
        let row_y = inner.y + i as u16;
        if row_y >= inner.bottom() {
            break;
        }
        mouse.click(
            Rect::new(inner.x, row_y, inner.width, 1),
            RegionId::ProjectMenuItem(i),
            Message::SwitchProject(i),
        );
    }
}
