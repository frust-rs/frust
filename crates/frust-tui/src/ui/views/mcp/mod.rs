//! The MCP panel (workbook §B13): a shadowed, centered popup showing what the
//! embedded MCP server is doing plus the clients connected to it right now,
//! with a start/stop action. Same popup shape as the doctor panel (§B7), and
//! the same base-layer suppression — the workbench beneath renders with a
//! *suppressed* `MouseCtx` (see `crate::ui::render`), so only this panel's
//! regions are live while it is open.
//!
//! Layering: this view takes the *values* it draws — a [`McpStatus`], a
//! client snapshot, and the retained failure reason — rather than reaching
//! into `AppState` for them, so a caller (and a test) can render any server
//! state without owning a live server. It never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{Message, RegionId, hms_at};
use crate::supervise::McpStatus;
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use frust_mcp::ClientEntry;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 58;

/// How many client rows the panel draws before summarising the rest. A
/// workbench with more agents attached than this is not a list to scroll —
/// the count in the header stays exact either way.
const MAX_CLIENT_ROWS: usize = 8;

/// Render the MCP panel centered over `area`.
///
/// `clients` is one snapshot of the registry (`AppState::mcp_clients`), taken
/// by the caller so every row in a frame agrees; `error` is the retained
/// reason the server last stopped unexpectedly, which turns a stopped server
/// into §B13's *failed* state.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    status: &McpStatus,
    clients: &[ClientEntry],
    error: Option<&str>,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let headline = headline_lines(status, error, theme);
    let client_rows = client_lines(clients, theme);
    // headline + blank + clients + blank + buttons + hint, inside 2 borders
    // and 1 row of top padding.
    let inner_rows = headline.len() as u16 + client_rows.len() as u16 + 4;
    let height = (inner_rows + 3).min(area.height);
    let box_ = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            " MCP server ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;
    let mut put = |line: Line<'static>, y: &mut u16| {
        if *y < inner.bottom() {
            frame.render_widget(Paragraph::new(line), row_rect(*y));
        }
        *y += 1;
    };

    for line in headline {
        put(line, &mut y);
    }
    put(Line::from(""), &mut y);
    for line in client_rows {
        put(line, &mut y);
    }
    put(Line::from(""), &mut y);

    // ── Start/Stop + Close buttons ───────────────────────────────────────
    if y < inner.bottom() {
        let toggle_label = match status {
            McpStatus::Stopped => " Start ",
            // A server that is still binding stops the same way a listening
            // one does (the cancel token is live from the moment it spawns),
            // so the button never goes dead mid-start.
            McpStatus::Starting | McpStatus::Listening { .. } => " Stop ",
        };
        let close_label = " Close ";
        let buttons = Line::from(vec![
            Span::styled(
                toggle_label,
                Style::default()
                    .fg(theme.bg())
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                close_label,
                Style::default().fg(theme.fg()).bg(theme.overlay()),
            ),
        ]);
        frame.render_widget(Paragraph::new(buttons), row_rect(y));
        let toggle_w = toggle_label.chars().count() as u16;
        mouse.click(
            Rect::new(inner.x, y, toggle_w, 1),
            RegionId::McpPanelToggleServer,
            Message::ToggleMcpServer,
        );
        mouse.click(
            Rect::new(
                inner.x + toggle_w + 3,
                y,
                close_label.chars().count() as u16,
                1,
            ),
            RegionId::McpPanelClose,
            Message::CloseMcpPanel,
        );
        y += 1;
    }

    if y < inner.bottom() {
        let hint = match status {
            McpStatus::Stopped => "s start · Esc close",
            McpStatus::Starting | McpStatus::Listening { .. } => "s stop · Esc close",
        };
        frame.render_widget(
            Paragraph::new(Line::styled(hint, Style::default().fg(theme.muted()))),
            row_rect(y),
        );
    }
}

/// The §B13 headline block: one state row, plus the lines explaining it —
/// the offer while it is off, the reason while it is failed.
fn headline_lines(status: &McpStatus, error: Option<&str>, theme: &Theme) -> Vec<Line<'static>> {
    let note = |text: String| Line::styled(text, Style::default().fg(theme.muted()));
    let state = |glyph: &str, text: String, color: Color| {
        Line::from(vec![
            Span::styled(format!("{glyph} "), Style::default().fg(color)),
            Span::styled(text, Style::default().fg(theme.fg())),
        ])
    };
    match status {
        McpStatus::Stopped => match error {
            // A stopped server carrying a reason is §B13's *failed* state:
            // the reason is what makes a refused bind visibly different from
            // a server nobody started.
            Some(error) => {
                let mut lines = vec![state(
                    "\u{2717}",
                    "the MCP server stopped".to_string(),
                    theme.error(),
                )];
                lines.extend(
                    wrap(error, (MODAL_WIDTH - 6) as usize)
                        .into_iter()
                        .map(|line| Line::styled(line, Style::default().fg(theme.error()))),
                );
                lines
            }
            None => vec![
                state("\u{25cb}", "not running".to_string(), theme.muted()),
                note("start it to let an agent drive this workbench's".to_string()),
                note("own sessions".to_string()),
            ],
        },
        McpStatus::Starting => vec![
            state("\u{21bb}", "starting…".to_string(), theme.warn()),
            note("binding a loopback listener".to_string()),
        ],
        McpStatus::Listening { port, .. } => vec![
            state(
                "\u{25cf}",
                format!("listening on 127.0.0.1:{port}"),
                theme.success(),
            ),
            note("an agent configured for `frust mcp` reaches this".to_string()),
            note("workbench's own sessions at the same address".to_string()),
        ],
    }
}

/// The client block: a count header plus one row per connected client
/// (registry id + the wall-clock time it connected), or the honest empty
/// state. Never a fabricated row and never a blank gap.
fn client_lines(clients: &[ClientEntry], theme: &Theme) -> Vec<Line<'static>> {
    if clients.is_empty() {
        return vec![Line::styled(
            "no clients connected",
            Style::default().fg(theme.muted()),
        )];
    }
    let mut lines = vec![Line::styled(
        format!("Clients ({})", clients.len()),
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    )];
    for entry in clients.iter().take(MAX_CLIENT_ROWS) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  #{}", entry.id),
                Style::default().fg(theme.accent()),
            ),
            Span::styled(
                format!("  connected {}", hms_at(entry.connected_at)),
                Style::default().fg(theme.muted()),
            ),
        ]));
    }
    if clients.len() > MAX_CLIENT_ROWS {
        lines.push(Line::styled(
            format!("  … {} more", clients.len() - MAX_CLIENT_ROWS),
            Style::default().fg(theme.muted()),
        ));
    }
    lines
}

/// Wrap `text` to `width` columns on whitespace, breaking an
/// over-long word rather than overflowing the panel.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        for chunk in hard_split(word, width) {
            if current.is_empty() {
                current = chunk;
            } else if current.chars().count() + 1 + chunk.chars().count() <= width {
                current.push(' ');
                current.push_str(&chunk);
            } else {
                lines.push(std::mem::replace(&mut current, chunk));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Split one word into `width`-column pieces (a single piece when it already
/// fits) — a URL or a long path can exceed the panel on its own.
fn hard_split(word: &str, width: usize) -> Vec<String> {
    if word.chars().count() <= width || width == 0 {
        return vec![word.to_string()];
    }
    word.chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn theme() -> Theme {
        Theme::frust_dark()
    }

    fn client(id: u64, at_secs: u64) -> ClientEntry {
        ClientEntry {
            id,
            connected_at: UNIX_EPOCH + Duration::from_secs(at_secs),
            connected_since: Instant::now(),
        }
    }

    #[test]
    fn no_clients_renders_the_empty_state_not_a_blank_block() {
        let lines = client_lines(&[], &theme());
        assert_eq!(lines.len(), 1);
        assert_eq!(text_of(&lines[0]), "no clients connected");
    }

    #[test]
    fn each_client_renders_its_id_and_connect_time() {
        let lines = client_lines(&[client(0, 3_723), client(7, 0)], &theme());
        assert_eq!(text_of(&lines[0]), "Clients (2)");
        assert_eq!(text_of(&lines[1]), "  #0  connected 01:02:03");
        assert_eq!(text_of(&lines[2]), "  #7  connected 00:00:00");
    }

    #[test]
    fn an_over_long_client_list_is_summarised_with_an_exact_count() {
        let clients: Vec<ClientEntry> = (0..MAX_CLIENT_ROWS as u64 + 3)
            .map(|i| client(i, 0))
            .collect();
        let lines = client_lines(&clients, &theme());
        assert_eq!(text_of(&lines[0]), format!("Clients ({})", clients.len()));
        assert_eq!(text_of(lines.last().unwrap()), "  … 3 more");
    }

    #[test]
    fn a_stopped_server_with_a_reason_is_the_failed_headline() {
        let theme = theme();
        let lines = headline_lines(&McpStatus::Stopped, None, &theme);
        assert!(text_of(&lines[0]).contains("not running"));

        let lines = headline_lines(
            &McpStatus::Stopped,
            Some("binding 127.0.0.1:4848 failed: Address already in use (os error 98)"),
            &theme,
        );
        assert!(text_of(&lines[0]).contains("stopped"));
        let reason: String = lines[1..].iter().map(text_of).collect::<Vec<_>>().join(" ");
        assert!(reason.contains("Address already in use"), "{reason}");
    }

    #[test]
    fn a_listening_server_names_the_bound_loopback_port() {
        let lines = headline_lines(
            &McpStatus::Listening {
                port: 4848,
                clients: 1,
            },
            None,
            &theme(),
        );
        assert!(text_of(&lines[0]).contains("127.0.0.1:4848"));
    }

    #[test]
    fn wrapping_breaks_on_whitespace_and_splits_an_overlong_word() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("aaaaaaaaaa", 4), vec!["aaaa", "aaaa", "aa"]);
        assert!(wrap("", 10).is_empty());
    }

    /// The plain text of a rendered line (styles dropped) — the assertions
    /// here are about content, which the snapshot suite covers visually.
    fn text_of(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn hms_at_is_the_shared_wall_clock_format() {
        // Guards the panel against drifting from the log view's column.
        assert_eq!(hms_at(UNIX_EPOCH + Duration::from_secs(45_296)), "12:34:56");
        assert_eq!(hms_at(SystemTime::UNIX_EPOCH), "00:00:00");
    }
}
