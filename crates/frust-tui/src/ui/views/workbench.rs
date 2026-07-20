//! The workbench shell (D5 / workbook B2): titlebar / sidebar / main / status.
//! Static content in Phase 1 — project detection, sessions, and interactivity
//! land in Phase 2.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::engine::{AppState, DeviceRow, Message, RegionId};
use crate::ui::layout::{Shell, sidebar_main};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use crate::ui::views::sessions;
use frust_drive::devices::{Kind, Platform};

/// Render the workbench shell into `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let shell = Shell::split(area);
    titlebar(frame, shell.titlebar, state, theme, mouse);

    let (sidebar, main) = sidebar_main(shell.body);
    render_sidebar(frame, sidebar, state, theme, mouse);
    // With sessions open, the main area is the tab bar + log view; otherwise the
    // static dashboard placeholder.
    if state.sessions.is_empty() {
        render_dashboard(frame, main, state, theme);
    } else {
        sessions::render_main(frame, main, state, theme, mouse);
    }
    status(frame, shell.status, state, theme);
}

fn titlebar(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme, mouse: &mut MouseCtx) {
    let project = state
        .project_root
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    let row0 = Rect::new(area.x, area.y, area.width, 1);

    // Built with a running column cursor (mirrors the tab-bar / run-config
    // field patterns) so the project-name + `▾` chevron's click rect lands
    // exactly on its rendered columns.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col: u16 = 0;
    let push = |spans: &mut Vec<Span<'static>>, col: &mut u16, text: String, style: Style| {
        *col += text.chars().count() as u16;
        spans.push(Span::styled(text, style));
    };

    push(
        &mut spans,
        &mut col,
        format!("{} Frust ", theme.icons.gear()),
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    );
    push(
        &mut spans,
        &mut col,
        "│ ".to_string(),
        Style::default().fg(theme.border()),
    );

    let switcher_x = area.x + col;
    push(
        &mut spans,
        &mut col,
        project,
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    );
    push(
        &mut spans,
        &mut col,
        " ▾ ".to_string(),
        Style::default().fg(theme.accent()),
    );
    let switcher_w = col - (switcher_x - area.x);

    push(
        &mut spans,
        &mut col,
        "· frust 0.1.0 · ".to_string(),
        Style::default().fg(theme.muted()),
    );
    push(
        &mut spans,
        &mut col,
        format!("{} toolchain", theme.icons.ok()),
        Style::default().fg(theme.success()),
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), row0);

    if switcher_x < area.right() {
        let w = switcher_w.min(area.right().saturating_sub(switcher_x));
        mouse.click(
            Rect::new(switcher_x, area.y, w, 1),
            RegionId::ProjectSwitcherToggle,
            Message::ToggleProjectSwitcher,
        );
    }

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

fn render_sidebar(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
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

    // Build the whole sidebar as a stacked line list, remembering which rows
    // are interactive device rows (and the refresh affordance) so their
    // single-row click rects can be registered after layout.
    let mut lines = vec![heading("PROJECTS")];
    let project_rows_start = lines.len();
    // `project_lines` collapses to a single "(none detected)" placeholder
    // when empty (never clickable); otherwise it's one line per project.
    let project_row_count = state.projects.len();
    lines.extend(project_lines(state, theme));
    lines.push(Line::from(""));

    // DEVICES header carries the refresh affordance on the right.
    let devices_header_row = lines.len();
    lines.push(devices_header_line(state, theme));
    let mut device_rows: Vec<usize> = Vec::new();
    if state.devices.is_empty() {
        let msg = if state.devices_refreshing {
            "scanning…"
        } else {
            "none found"
        };
        lines.push(item(msg));
    } else {
        for (i, row) in state.devices.iter().enumerate() {
            device_rows.push(lines.len());
            lines.push(device_line(state, i, row, theme));
        }
    }

    lines.push(Line::from(""));
    lines.push(heading("SESSIONS"));
    lines.extend(sessions::sidebar_lines(state, theme));
    lines.push(Line::from(""));
    lines.push(heading("ACTIONS"));
    let new_project_row = lines.len();
    lines.push(item("New project"));
    lines.push(item("Doctor · Settings"));
    frame.render_widget(Paragraph::new(lines), inner);

    // Register the interactive regions now that row indices are known (a row
    // scrolled past the visible height registers nothing).
    let row_rect = |row: usize| -> Option<Rect> {
        let y = inner.y + row as u16;
        (y < inner.bottom()).then(|| Rect::new(inner.x, y, inner.width, 1))
    };
    for i in 0..project_row_count {
        if let Some(rect) = row_rect(project_rows_start + i) {
            mouse.click(rect, RegionId::ProjectRow(i), Message::SwitchProject(i));
        }
    }
    if let Some(rect) = row_rect(devices_header_row) {
        mouse.click(rect, RegionId::RefreshDevices, Message::RefreshDevices);
    }
    for (i, &row) in device_rows.iter().enumerate() {
        if let Some(rect) = row_rect(row) {
            mouse.click(rect, RegionId::DeviceRow(i), Message::SelectDeviceAt(i));
        }
    }
    if let Some(rect) = row_rect(new_project_row) {
        mouse.click(rect, RegionId::NewProjectAction, Message::OpenCreateWizard);
    }
}

/// The DEVICES section header, with the refresh affordance (a spinner-ish
/// glyph while a scan is in flight) right-aligned into the label.
fn devices_header_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let selected = state.selected_device_count();
    let count = if selected > 0 {
        format!("DEVICES ({selected})")
    } else {
        "DEVICES".to_string()
    };
    let refresh = if state.devices_refreshing {
        Span::styled(" \u{25cc} scanning", Style::default().fg(theme.warn()))
    } else {
        Span::styled(" \u{21bb} r", Style::default().fg(theme.muted()))
    };
    Line::from(vec![
        Span::styled(
            count,
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        refresh,
    ])
}

/// One device row: cursor marker, select checkbox, a connection-colored dot,
/// the device name, and a dim platform·kind tag.
fn device_line(state: &AppState, index: usize, row: &DeviceRow, theme: &Theme) -> Line<'static> {
    let under_cursor = state.device_cursor == index;
    let marker = if under_cursor { "\u{25b8}" } else { " " };
    let check = if row.selected { "[x]" } else { "[ ]" };
    let (dot_color, tag) = device_glyph(&row.device, theme);
    let name_style = if under_cursor {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    } else if row.selected {
        Style::default().fg(theme.fg())
    } else {
        Style::default().fg(theme.muted())
    };
    Line::from(vec![
        Span::styled(format!(" {marker} "), Style::default().fg(theme.accent())),
        Span::styled(
            format!("{check} "),
            Style::default().fg(if row.selected {
                theme.success()
            } else {
                theme.muted()
            }),
        ),
        Span::styled("\u{25cf} ", Style::default().fg(dot_color)),
        Span::styled(row.device.name.clone(), name_style),
        Span::styled(format!(" {tag}"), Style::default().fg(theme.muted())),
    ])
}

/// The connection dot color and the `platform·kind` tag for a device.
fn device_glyph(
    device: &frust_drive::devices::Device,
    theme: &Theme,
) -> (ratatui::style::Color, String) {
    let platform = match device.platform {
        Platform::Android => "android",
        Platform::Ios => "ios",
    };
    let kind = match device.kind {
        Kind::PhysicalDevice => "device",
        Kind::Emulator => "emulator",
        Kind::Simulator => "simulator",
    };
    // A physical iOS device reports a connection state; "disconnected" is a
    // paired-but-not-tunneled device (still targetable) — dim it.
    let color = match device.connection_state.as_deref() {
        Some("disconnected") => theme.warn(),
        _ => theme.success(),
    };
    (color, format!("{platform}·{kind}"))
}

/// One line per detected project (F5 bounded-walk detection); the active one
/// (`state.project_root`, first found for now — a full switcher is Phase 2)
/// gets the hover chevron and accent color, the rest render muted.
fn project_lines(state: &AppState, theme: &Theme) -> Vec<Line<'static>> {
    if state.projects.is_empty() {
        return vec![Line::styled(
            "  (none detected)",
            Style::default().fg(theme.muted()),
        )];
    }
    state
        .projects
        .iter()
        .map(|project| {
            let name = project
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| project.to_string_lossy().to_string());
            let active = state.project_root.as_deref() == Some(project.as_path());
            if active {
                Line::styled(
                    format!(" {} {name}", theme.icons.chevron()),
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::styled(format!("   {name}"), Style::default().fg(theme.muted()))
            }
        })
        .collect()
}

fn render_dashboard(frame: &mut Frame, area: Rect, _state: &AppState, theme: &Theme) {
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
            "No session running — start a run to open a log tab here.",
            Style::default().fg(theme.muted()),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn status(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    let running = state
        .sessions
        .iter()
        .filter(|s| !s.state.is_terminal())
        .count();
    let (dot, dot_color, label) = if running > 0 {
        ("●", theme.success(), format!("{running} running"))
    } else {
        ("●", theme.success(), "ready".to_string())
    };
    let left = Line::from(vec![
        Span::styled(format!("{dot} {label}"), Style::default().fg(dot_color)),
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
