//! The titlebar project-switcher dropdown: a popup
//! overlaying the sidebar's PROJECTS section (same column range — full
//! sidebar width, flush with its left edge — so it never bleeds a stray
//! sliver of the section it's replacing on either side), listing every
//! local/previous project (decision D6, same split the sidebar renders) —
//! local rows first, then, only when both sections are non-empty, a
//! non-clickable "Previous projects" separator and the previous rows.
//! Opened via the titlebar `▾` chevron or `Ctrl+O`/`p`; the base workbench
//! layer renders with a *suppressed* `MouseCtx` while this is open (see
//! `crate::ui::render`) — the same base-layer suppression the run-config
//! modal uses.
//!
//! Layering: renders `&AppState` and only *registers* interaction (item
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

/// One rendered dropdown row: either a project (its `state.projects` index)
/// or the non-clickable "Previous projects" separator between the local and
/// previous sections (decision D6) — `state.project_switcher_cursor` and the
/// digit shortcuts stay indices into `state.projects` throughout; only the
/// *rendered* row offset shifts when a separator is inserted, so `Row::Item`
/// carries the same index the cursor already uses, unadjusted.
enum Row {
    Item(usize),
    Separator,
}

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

    let local_count = state.local_count();
    let previous_count = state.projects.len() - local_count;
    // A separator row is only meaningful between two non-empty sections.
    let has_separator = local_count > 0 && previous_count > 0;

    let mut rows: Vec<Row> = (0..local_count).map(Row::Item).collect();
    if has_separator {
        rows.push(Row::Separator);
    }
    rows.extend((local_count..state.projects.len()).map(Row::Item));

    let height = (rows.len() as u16 + 2).min(sidebar.height);
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

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows.len());
    for row in &rows {
        let i = match row {
            Row::Item(i) => *i,
            Row::Separator => {
                lines.push(Line::styled(
                    "   Previous projects",
                    Style::default().fg(theme.muted()),
                ));
                continue;
            }
        };
        let project = &state.projects[i];
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

    for (row_offset, row) in rows.iter().enumerate() {
        let row_y = inner.y + row_offset as u16;
        if row_y >= inner.bottom() {
            break;
        }
        if let Row::Item(i) = row {
            mouse.click(
                Rect::new(inner.x, row_y, inner.width, 1),
                RegionId::ProjectMenuItem(*i),
                Message::SwitchProject(*i),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::mouse::MouseRegions;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use std::path::PathBuf;

    fn switcher_state(projects: Vec<PathBuf>, local_count: usize, cursor: usize) -> AppState {
        AppState {
            screen: crate::engine::Screen::Workbench,
            project_root: projects.first().cloned(),
            local_project_count: local_count,
            project_switcher_open: true,
            project_switcher_cursor: cursor,
            projects,
            ..AppState::default()
        }
    }

    /// Render the dropdown into a plain string, registering its mouse
    /// regions into `regions` — mirrors `workbench`'s own render-test helper.
    fn render_to_string(state: &AppState, regions: &mut MouseRegions) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = Theme::frust_dark();
        terminal
            .draw(|frame| {
                let mut ctx = MouseCtx::new(regions);
                let area = frame.area();
                render(frame, area, state, &theme, &mut ctx);
            })
            .expect("draw");
        let buf = terminal.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell(Position::new(x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn a_separator_row_sits_between_local_and_previous_when_both_are_non_empty() {
        let state = switcher_state(
            vec![PathBuf::from("/tmp/local"), PathBuf::from("/tmp/prev")],
            1,
            0,
        );
        let mut regions = MouseRegions::new();
        let text = render_to_string(&state, &mut regions);
        assert!(text.contains("Previous projects"), "{text}");
    }

    #[test]
    fn no_separator_row_when_every_project_is_local() {
        let state = switcher_state(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")], 2, 0);
        let mut regions = MouseRegions::new();
        let text = render_to_string(&state, &mut regions);
        assert!(!text.contains("Previous projects"), "{text}");
    }

    /// The separator never registers a click region, and the cursor/digit
    /// vocabulary the row *after* it dispatches is still a `state.projects`
    /// index — only the rendered row offset moved.
    #[test]
    fn the_separator_row_is_never_clickable_and_indices_stay_into_projects() {
        let local_count = 1;
        let state = switcher_state(
            vec![PathBuf::from("/tmp/local"), PathBuf::from("/tmp/prev")],
            local_count,
            0,
        );
        let mut regions = MouseRegions::new();
        let _ = render_to_string(&state, &mut regions);
        let hits: Vec<usize> = (0..30)
            .filter_map(|y| match regions.click_at(3, y) {
                Some(Message::SwitchProject(i)) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(
            hits,
            vec![0, local_count],
            "exactly two clickable rows, no region for the separator"
        );
    }
}
