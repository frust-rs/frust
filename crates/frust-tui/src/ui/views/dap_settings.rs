//! The DAP settings dialog: a shadowed, centered popup over the workbench
//! showing what the embedded debug-adapter server is doing and the persisted
//! preferences that decide when it starts and what it tells an editor. Same
//! popup shape as the MCP panel and the run-config modal, and the same
//! base-layer suppression — the screen beneath renders with a *suppressed*
//! `MouseCtx` (see `crate::ui::render`), so only this dialog's regions are
//! live while it is open.
//!
//! Layering: this view takes the *values* it draws — the settings model, a
//! [`DapStatus`], the attached-editor count, and the retained server failure —
//! rather than reaching into `AppState`, so a caller (and a test) can render
//! any server state without owning a live server. It never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{DapFocus, DapSettings, Message, RegionId};
use crate::supervise::DapStatus;
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 64;

/// The label column width every row's label is padded to.
const LABEL_WIDTH: usize = 10;

/// Render the DAP settings dialog centered over `area`, registering its
/// interactive regions through `mouse` (the *live* ctx — the layer beneath was
/// drawn suppressed).
///
/// `clients` is one snapshot of the attached-editor count
/// (`AppState::dap_clients`), taken by the caller so every row in a frame
/// agrees; `error` is the retained reason the server last stopped
/// unexpectedly.
// One argument past clippy's threshold, and deliberately so: this dialog
// shows both a settings model and a live server, and bundling the three
// server values into a wrapper type would buy a smaller signature at the cost
// of a type that exists only to be destructured one line later.
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    settings: &DapSettings,
    status: &DapStatus,
    clients: usize,
    error: Option<&str>,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let status_lines = status_block(settings, status, error, theme);
    // server + port + 2 checkboxes + ide + generate/close (6), a blank, the
    // status block, and the keyhint row — inside 2 borders and 1 row of top
    // padding.
    let inner_rows = 6 + 1 + status_lines.len() as u16 + 1;
    let height = (inner_rows + 3).min(area.height);
    let box_ = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            " DAP server ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;

    // ── Server state + Start/Stop action ─────────────────────────────────
    if y < inner.bottom() {
        let (token, color) = state_token(status, error.is_some(), clients, theme);
        let action = match status {
            // A server still binding stops the same way a listening one does
            // (its cancel token is live from the moment it spawns), so the
            // action never goes dead mid-start.
            DapStatus::Stopped => " Start ",
            DapStatus::Starting | DapStatus::Listening { .. } => " Stop ",
        };
        let focused = settings.focus == DapFocus::Server;
        let action_style = if focused {
            Style::default()
                .fg(theme.bg())
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.bg())
                .bg(theme.success())
                .add_modifier(Modifier::BOLD)
        };
        let prefix = format!("{}{:<LABEL_WIDTH$}", marker(focused), "Server");
        let line = Line::from(vec![
            Span::styled(prefix.clone(), label_style(focused, theme)),
            Span::styled(format!("{token}  "), Style::default().fg(color)),
            Span::styled(action.to_string(), action_style),
        ]);
        frame.render_widget(Paragraph::new(line), row_rect(y));
        let action_x = inner.x + (prefix.chars().count() + token.chars().count() + 2) as u16;
        mouse.click(
            Rect::new(action_x, y, action.chars().count() as u16, 1),
            RegionId::DapSettingsToggleServer,
            Message::ToggleDapServer,
        );
        y += 1;
    }

    // ── Port field ───────────────────────────────────────────────────────
    if y < inner.bottom() {
        let focused = settings.focus == DapFocus::Port;
        let listening_on = match status {
            DapStatus::Listening { port, .. } => Some(*port),
            DapStatus::Stopped | DapStatus::Starting => None,
        };
        let invalid = settings.parsed_port().is_none();
        let mut spans = vec![
            Span::styled(
                format!("{}{:<LABEL_WIDTH$}", marker(focused), "Port"),
                label_style(focused, theme),
            ),
            Span::styled(
                settings.port_input.clone(),
                Style::default().fg(if invalid { theme.error() } else { theme.fg() }),
            ),
        ];
        if focused {
            spans.push(Span::styled(
                "\u{2588}",
                Style::default().fg(theme.accent()),
            ));
        }
        if invalid {
            spans.push(Span::styled(
                "  not a port (0–65535)",
                Style::default().fg(theme.error()),
            ));
        } else if settings.port_awaits_restart(listening_on) {
            spans.push(Span::styled(
                "  takes effect on restart",
                Style::default().fg(theme.warn()),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), row_rect(y));
        mouse.click(
            row_rect(y),
            RegionId::DapSettingsPortRow,
            Message::DapSettingsFocus(DapFocus::Port),
        );
        y += 1;
    }

    // ── Preference checkboxes ────────────────────────────────────────────
    y = checkbox_row(
        frame,
        mouse,
        row_rect,
        y,
        inner.bottom(),
        settings.focus == DapFocus::AutoStart,
        settings.auto_start_in_ide,
        "auto-start in an IDE terminal",
        RegionId::DapSettingsAutoStartRow,
        Message::DapSettingsToggleAutoStart,
        theme,
    );
    y = checkbox_row(
        frame,
        mouse,
        row_rect,
        y,
        inner.bottom(),
        settings.focus == DapFocus::AutoConfigure,
        settings.auto_configure_ide,
        "auto-configure the IDE on start",
        RegionId::DapSettingsAutoConfigureRow,
        Message::DapSettingsToggleAutoConfigure,
        theme,
    );

    // ── IDE selector ─────────────────────────────────────────────────────
    if y < inner.bottom() {
        let focused = settings.focus == DapFocus::Ide;
        let line = Line::from(vec![
            Span::styled(
                format!("{}{:<LABEL_WIDTH$}", marker(focused), "IDE"),
                label_style(focused, theme),
            ),
            Span::styled(
                format!("\u{2039} {} \u{203a}", settings.ide_label()),
                if focused {
                    Style::default()
                        .fg(theme.bg())
                        .bg(theme.accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg())
                },
            ),
        ]);
        frame.render_widget(Paragraph::new(line), row_rect(y));
        mouse.click(
            row_rect(y),
            RegionId::DapSettingsIdeRow,
            Message::DapSettingsCycleIde(1),
        );
        y += 1;
    }

    // ── Generate / Close actions ─────────────────────────────────────────
    if y < inner.bottom() {
        let focused = settings.focus == DapFocus::Generate;
        let generate = " Generate IDE config now ";
        let close = " Close ";
        let generate_style = if focused {
            Style::default()
                .fg(theme.bg())
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg()).bg(theme.overlay())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(generate.to_string(), generate_style),
                Span::raw("   "),
                Span::styled(
                    close.to_string(),
                    Style::default().fg(theme.fg()).bg(theme.overlay()),
                ),
            ])),
            row_rect(y),
        );
        let generate_w = generate.chars().count() as u16;
        mouse.click(
            Rect::new(inner.x, y, generate_w, 1),
            RegionId::DapSettingsGenerate,
            Message::DapSettingsGenerate,
        );
        mouse.click(
            Rect::new(inner.x + generate_w + 3, y, close.chars().count() as u16, 1),
            RegionId::DapSettingsClose,
            Message::CloseDapSettings,
        );
        y += 1;
    }

    // ── Status block + keyhints ──────────────────────────────────────────
    y += 1; // blank
    for line in status_lines {
        if y >= inner.bottom() {
            return;
        }
        frame.render_widget(Paragraph::new(line), row_rect(y));
        y += 1;
    }
    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Tab move · Space toggle · s start/stop · g gen · Esc close",
                Style::default().fg(theme.muted()),
            )),
            row_rect(y),
        );
    }
}

/// A focus marker column, so the focused control is identifiable without color.
fn marker(focused: bool) -> &'static str {
    if focused { "\u{25b8} " } else { "  " }
}

/// A row label's style, brightened when the row has focus.
fn label_style(focused: bool, theme: &Theme) -> Style {
    if focused {
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted())
    }
}

/// Draw one preference checkbox row and register its click region, returning
/// the next row.
#[allow(clippy::too_many_arguments)]
fn checkbox_row(
    frame: &mut Frame,
    mouse: &mut MouseCtx,
    row_rect: impl Fn(u16) -> Rect,
    y: u16,
    bottom: u16,
    focused: bool,
    checked: bool,
    label: &str,
    id: RegionId,
    msg: Message,
    theme: &Theme,
) -> u16 {
    if y >= bottom {
        return y;
    }
    let line = Line::from(vec![
        Span::styled(marker(focused).to_string(), label_style(focused, theme)),
        Span::styled(
            if checked { "[x] " } else { "[ ] " }.to_string(),
            Style::default().fg(if checked {
                theme.success()
            } else {
                theme.muted()
            }),
        ),
        Span::styled(
            label.to_string(),
            if focused {
                Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg())
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(line), row_rect(y));
    mouse.click(row_rect(y), id, msg);
    y + 1
}

/// The server-state token and its color: muted off, warn while binding,
/// success once listening (with the attached-editor count, only when there is
/// one to show — `(0)` beside a listening server reads as a problem rather
/// than as a server waiting for an editor), error after a failure.
fn state_token(status: &DapStatus, failed: bool, clients: usize, theme: &Theme) -> (String, Color) {
    match status {
        DapStatus::Stopped if failed => ("failed".to_string(), theme.error()),
        DapStatus::Stopped => ("not running".to_string(), theme.muted()),
        DapStatus::Starting => ("starting…".to_string(), theme.warn()),
        DapStatus::Listening { port, .. } if clients == 0 => {
            (format!("127.0.0.1:{port}"), theme.success())
        }
        DapStatus::Listening { port, .. } => (
            format!("127.0.0.1:{port} ({clients} attached)"),
            theme.success(),
        ),
    }
}

/// The status block under the controls: the retained server failure (if any)
/// and the most recent IDE-config outcome. Never a blank gap — with neither to
/// report it says what the dialog is for.
fn status_block(
    settings: &DapSettings,
    status: &DapStatus,
    error: Option<&str>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(error) = error {
        lines.push(Line::styled(
            truncate(&format!("\u{2717} {error}"), (MODAL_WIDTH - 6) as usize),
            Style::default().fg(theme.error()),
        ));
    }
    match &settings.last_ide_config {
        Some(report) => lines.push(Line::styled(
            truncate(&report.summary(), (MODAL_WIDTH - 6) as usize),
            Style::default().fg(if report.is_failure() {
                theme.error()
            } else {
                theme.muted()
            }),
        )),
        None if lines.is_empty() => lines.push(Line::styled(
            match status {
                DapStatus::Listening { .. } => "an editor can attach to the address above",
                DapStatus::Stopped | DapStatus::Starting => {
                    "start it to debug this project from your editor"
                }
            },
            Style::default().fg(theme.muted()),
        )),
        None => {}
    }
    lines
}

/// Clip `text` to `width` columns with an ellipsis — a generated config path
/// can be longer than the dialog, and a wrapped path is harder to read than a
/// clipped one.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let head: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DapIdeReport;
    use frust_dap::ide_config::{ConfigAction, IdeConfigResult, ParentIde};
    use std::path::PathBuf;

    fn theme() -> Theme {
        Theme::frust_dark()
    }

    fn text_of(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_state_token_names_the_loopback_address_and_only_a_real_client_count() {
        let theme = theme();
        assert_eq!(
            state_token(&DapStatus::Stopped, false, 0, &theme).0,
            "not running"
        );
        assert_eq!(
            state_token(&DapStatus::Stopped, true, 0, &theme).0,
            "failed",
            "a stopped server carrying a reason is the failed state"
        );
        assert_eq!(
            state_token(&DapStatus::Starting, false, 0, &theme).0,
            "starting…"
        );
        assert_eq!(
            state_token(
                &DapStatus::Listening {
                    port: 4849,
                    clients: 0
                },
                false,
                0,
                &theme
            )
            .0,
            "127.0.0.1:4849"
        );
        assert_eq!(
            state_token(
                &DapStatus::Listening {
                    port: 4849,
                    clients: 2
                },
                false,
                2,
                &theme
            )
            .0,
            "127.0.0.1:4849 (2 attached)"
        );
    }

    #[test]
    fn the_status_block_shows_the_last_config_result_and_a_retained_failure() {
        let theme = theme();
        let mut settings = DapSettings::default();
        let lines = status_block(&settings, &DapStatus::Stopped, None, &theme);
        assert_eq!(lines.len(), 1, "never a blank gap");
        assert!(text_of(&lines[0]).contains("start it"));

        settings.last_ide_config = Some(DapIdeReport::Written {
            ide: ParentIde::VSCode,
            result: IdeConfigResult {
                path: PathBuf::from("/tmp/p/.vscode/launch.json"),
                action: ConfigAction::Created,
            },
        });
        let lines = status_block(
            &settings,
            &DapStatus::Stopped,
            Some("binding 127.0.0.1:4849 failed: Address already in use"),
            &theme,
        );
        assert_eq!(lines.len(), 2);
        assert!(text_of(&lines[0]).contains("Address already in use"));
        assert!(text_of(&lines[1]).contains("VS Code: created"));
    }

    #[test]
    fn an_over_long_status_line_is_clipped_rather_than_overflowing() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd\u{2026}");
    }
}
