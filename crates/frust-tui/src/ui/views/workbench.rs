//! The workbench shell (D5 / workbook B2): titlebar / sidebar / main / status.
//! Static content in Phase 1 — project detection, sessions, and interactivity
//! land in Phase 2.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::engine::AppState;
use crate::ui::layout::{Shell, sidebar_main};
use crate::ui::theme::Theme;

/// Render the workbench shell into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let shell = Shell::split(area);
    titlebar(frame, shell.titlebar, state, theme);

    let (sidebar, main) = sidebar_main(shell.body);
    render_sidebar(frame, sidebar, theme);
    render_main(frame, main, state, theme);
    status(frame, shell.status, theme);
}

fn titlebar(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let project = state
        .project_root
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    let row0 = Rect::new(area.x, area.y, area.width, 1);
    let left = Line::from(vec![
        Span::styled(
            format!("{} Frust ", theme.icons.gear()),
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ),
        Span::styled("│ ", Style::default().fg(theme.border())),
        Span::styled(
            project,
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ▾ ", Style::default().fg(theme.accent())),
        Span::styled("· frust 0.1.0 · ", Style::default().fg(theme.muted())),
        Span::styled(
            format!("{} toolchain", theme.icons.ok()),
            Style::default().fg(theme.success()),
        ),
    ]);
    frame.render_widget(Paragraph::new(left), row0);

    let actions = Line::from(vec![
        Span::styled(
            format!(" {} Run ", theme.icons.run()),
            Style::default().fg(theme.fg()),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!(" {} Build ", theme.icons.create()),
            Style::default().fg(theme.fg()),
        ),
    ]);
    frame.render_widget(Paragraph::new(actions).alignment(Alignment::Right), row0);

    // Orange underline motif under the wordmark.
    if area.height > 1 {
        let row1 = Rect::new(area.x, area.y + 1, 8.min(area.width), 1);
        frame.render_widget(
            Paragraph::new(Line::styled(
                "━━━━━━━━",
                Style::default().fg(theme.accent()),
            )),
            row1,
        );
    }
}

fn render_sidebar(frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.border()))
        .padding(Padding::new(1, 1, 0, 0))
        .style(Style::default().bg(theme.surface()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let heading = |s: &str| {
        Line::styled(
            s.to_string(),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )
    };
    let item = |s: &str| Line::styled(format!("  {s}"), Style::default().fg(theme.muted()));

    let lines = vec![
        heading("PROJECTS"),
        item("(none detected)"),
        Line::from(""),
        heading("DEVICES"),
        item("(refresh in Phase 2)"),
        Line::from(""),
        heading("SESSIONS"),
        item("none running"),
        Line::from(""),
        heading("ACTIONS"),
        item("Doctor · Settings"),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_main(frame: &mut Frame, area: Rect, _state: &AppState, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(theme.bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::styled(
            "Dashboard",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ),
        Line::styled("━━━━━━━━━━", Style::default().fg(theme.accent())),
        Line::from(""),
        Line::styled(
            "Workbench shell — sessions, devices, and run flows arrive in Phase 2.",
            Style::default().fg(theme.muted()),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn status(frame: &mut Frame, area: Rect, theme: &Theme) {
    let left = Line::from(vec![
        Span::styled("● ready", Style::default().fg(theme.success())),
        Span::styled("  │  ", Style::default().fg(theme.border())),
        Span::styled(
            "r run · b build · d doctor · ⌘ palette · ? help",
            Style::default().fg(theme.muted()),
        ),
    ]);
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
