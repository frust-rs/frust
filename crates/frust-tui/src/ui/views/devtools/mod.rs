//! DevTools mode's chrome (workbook §B12): the four-tab strip, the five
//! connection-state screens, and the status row — rendered full-width in the
//! session pane, directly below the session tab bar, in place of the log view.
//!
//! Layering: like every view, this renders `&SessionView` and only *registers*
//! interaction (tab pills, the Retry button, the back affordance) through the
//! [`MouseCtx`]; it never mutates the engine. Which screen shows is
//! [`DevtoolsPhase`], derived once in the engine and matched exhaustively
//! here — a sixth screen cannot be added without this dispatch handling it.
//!
//! The tab *bodies* live beside this module: [`performance`] and
//! [`inspector`] render for real, System/Network are still placeholders
//! naming what will live there. Everything outside those bodies — the strip,
//! the empty states, the status row, the keyboard/mouse parity — is settled
//! here.

pub mod inspector;
pub mod performance;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::engine::{
    AppState, ConnState, DevtoolsPhase, DevtoolsTab, Message, PerfSource, RegionId, SessionView,
    perf_window,
};
use crate::ui::anim::spinner_char;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Render the DevTools surface for `session` into `area` (everything below
/// the session tab bar). `state` supplies the shared chrome the status row
/// mirrors from the log view (the animation clock behind the
/// discovering/connecting spinner, and the mouse-capture indicator).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    session: &SessionView,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let animation_frame = state.animation_frame;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg())),
        area,
    );
    let phase = session.devtools.phase();

    // Only the connected screen carries the tab strip; every empty state is
    // the centered message + the status row, per §B12's mockups.
    let strip_rows = if matches!(phase, DevtoolsPhase::Connected) {
        2
    } else {
        0
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(strip_rows),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    match phase {
        DevtoolsPhase::Connected => {
            render_tab_strip(frame, rows[0], session, theme, mouse);
            render_tab_body(frame, rows[1], session, theme, mouse);
        }
        DevtoolsPhase::Discovering => {
            empty_state(
                frame,
                rows[1],
                theme,
                EmptyState {
                    glyph: spinner_char(animation_frame).to_string(),
                    glyph_color: theme.accent(),
                    headline: "Waiting for a devtools discovery line…".to_string(),
                    headline_muted: true,
                    detail: vec![
                        "no “frust-devtools listening on …” line seen yet — appears".to_string(),
                        "the instant a debug/profile build logs it".to_string(),
                    ],
                },
            );
        }
        DevtoolsPhase::Connecting => {
            empty_state(
                frame,
                rows[1],
                theme,
                EmptyState {
                    glyph: spinner_char(animation_frame).to_string(),
                    glyph_color: theme.accent(),
                    headline: "Connecting to devtools service…".to_string(),
                    headline_muted: true,
                    detail: vec![format!("{} · handshake pending", endpoint(session))],
                },
            );
        }
        DevtoolsPhase::Failed => {
            let error = match &session.devtools.conn {
                ConnState::Failed { error } => error.clone(),
                // Unreachable by `phase`'s own derivation; a neutral line
                // rather than a panic if that ever changes.
                _ => "the connection is not available".to_string(),
            };
            let block = empty_state(
                frame,
                rows[1],
                theme,
                EmptyState {
                    glyph: "✗".to_string(),
                    glyph_color: theme.error(),
                    headline: "Could not reach devtools service".to_string(),
                    headline_muted: false,
                    detail: vec![error],
                },
            );
            // A session that has already ended has no service left to reach,
            // so it gets no Retry button — matching `r`'s own gate in
            // `crate::runner`'s DevTools key map.
            if !session.state.is_terminal() {
                render_retry_button(frame, rows[1], block, theme, mouse);
            }
        }
        DevtoolsPhase::Unavailable => {
            empty_state(
                frame,
                rows[1],
                theme,
                EmptyState {
                    glyph: "⚠".to_string(),
                    glyph_color: theme.warn(),
                    headline: "This build has no devtools service".to_string(),
                    headline_muted: false,
                    detail: vec![
                        "release builds compile the listener out entirely — rerun".to_string(),
                        "with `frust run` (debug) or `--profile` to inspect this session"
                            .to_string(),
                    ],
                },
            );
        }
    }

    render_status(frame, rows[2], state, session, phase, theme, mouse);
}

/// The `[1] Performance  [2] System  [3] Inspector  [4] Network` strip plus
/// the accent underline under the active tab. Every pill is a click region
/// (keyboard parity: `1`–`4`, `[`/`]`).
fn render_tab_strip(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.height == 0 {
        return;
    }
    let active = session.devtools.active_tab;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col: u16 = 0;
    // Where each pill starts and how wide it is, so the click rects land
    // exactly on the rendered columns (the titlebar/tab-bar pattern).
    let mut pills: Vec<(u16, u16)> = Vec::new();

    spans.push(Span::raw(" "));
    col += 1;
    for tab in DevtoolsTab::ALL {
        let label = format!("[{}] {}", tab.index() + 1, tab.title());
        let width = label.chars().count() as u16;
        let style = if tab == active {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted())
        };
        pills.push((col, width));
        spans.push(Span::styled(label, style));
        spans.push(Span::raw("  "));
        col += width + 2;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );

    // The service/log-fallback badge sits at the right of the strip — §B12's
    // Performance-tab indicator, driven off the same source truth table the
    // tab body draws from (`perf_window`'s first return value).
    let (source, _) = perf_window(
        &session.devtools.conn,
        &session.devtools.frames,
        &session.perf,
    );
    let (badge_text, badge_color) = match source {
        PerfSource::Service => ("● service ", theme.success()),
        PerfSource::LogFallback => ("◐ log fallback ", theme.warn()),
        PerfSource::NoData => ("○ no data ", theme.muted()),
    };
    let badge = Line::from(Span::styled(badge_text, Style::default().fg(badge_color)));
    frame.render_widget(
        Paragraph::new(badge).alignment(Alignment::Right),
        Rect::new(area.x, area.y, area.width, 1),
    );

    // The accent underline runs under the active pill only.
    if area.height > 1 {
        let (start, width) = pills[active.index()];
        if start < area.width {
            let width = width.min(area.width - start);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "━".repeat(width as usize),
                    Style::default().fg(theme.accent()),
                )),
                Rect::new(area.x + start, area.y + 1, width, 1),
            );
        }
    }

    for (tab, (start, width)) in DevtoolsTab::ALL.iter().zip(pills) {
        if start >= area.width {
            continue;
        }
        let width = width.min(area.width - start);
        mouse.click(
            Rect::new(area.x + start, area.y, width, 1),
            RegionId::DevtoolsTabPill(tab.index()),
            Message::DevtoolsTab(tab.index()),
        );
    }
}

/// The active tab's body. Performance and Inspector render for real; the
/// other two tabs' content arrives with their own tasks — until then a
/// low-key placeholder naming what will live there, so the chrome, the
/// routing, and the connection are reviewable on their own.
fn render_tab_body(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let tab = session.devtools.active_tab;
    match tab {
        DevtoolsTab::Performance => {
            performance::render(frame, area, session, theme, mouse);
            return;
        }
        DevtoolsTab::Inspector => {
            inspector::render(frame, area, session, theme, mouse);
            return;
        }
        DevtoolsTab::System | DevtoolsTab::Network => {}
    }
    let detail = match tab {
        DevtoolsTab::Performance | DevtoolsTab::Inspector => unreachable!("handled above"),
        DevtoolsTab::System => "CPU, RSS, thermal, and uptime",
        DevtoolsTab::Network => "process-level rx/tx byte counters",
    };
    let samples = session.devtools.frames.len();
    let lines = vec![
        Line::from(""),
        Line::styled(
            format!("  {} — not rendered yet", tab.title()),
            Style::default().fg(theme.muted()),
        ),
        Line::styled(format!("  {detail}"), Style::default().fg(theme.muted())),
        Line::from(""),
        Line::styled(
            format!("  {samples} frame samples collected"),
            Style::default().fg(theme.muted()),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// One branded empty state: a glyph + headline line, then muted detail lines,
/// centered in `area` the way the rest of the workbench's "nothing to show
/// yet" screens are.
struct EmptyState {
    glyph: String,
    glyph_color: ratatui::style::Color,
    headline: String,
    /// Whether the headline renders muted (a passive, will-resolve-itself
    /// state) or bright (something the user has to act on).
    headline_muted: bool,
    detail: Vec<String>,
}

/// Paints `state` centered in `area` and returns the rect it covered, so a
/// caller can place an action button directly under it.
fn empty_state(frame: &mut Frame, area: Rect, theme: &Theme, state: EmptyState) -> Rect {
    let empty = Rect::new(area.x, area.y, 0, 0);
    if area.height == 0 || area.width == 0 {
        return empty;
    }
    // The block is laid out against a bounded content width and wrapped here
    // rather than by the paragraph, so the rendered height is known exactly
    // and the block genuinely centers — an error string is arbitrary-length
    // (it comes off the wire), and truncating one would hide the reason a
    // connection failed.
    let content_width = usize::from(area.width.saturating_sub(4)).min(EMPTY_STATE_WIDTH);
    if content_width == 0 {
        return empty;
    }
    let headline_style = if state.headline_muted {
        Style::default().fg(theme.muted())
    } else {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{}  ", state.glyph),
            Style::default().fg(state.glyph_color),
        ),
        Span::styled(state.headline, headline_style),
    ])];
    for detail in state.detail {
        for row in wrap_words(&detail, content_width) {
            lines.push(Line::styled(row, Style::default().fg(theme.muted())));
        }
    }

    let height = (lines.len() as u16).min(area.height);
    let top = area.y + area.height.saturating_sub(height) / 2;
    let width = (content_width as u16 + EMPTY_STATE_GLYPH_INDENT).min(area.width);
    let left = area.x + (area.width - width) / 2;
    let rect = Rect::new(left, top, width, height);
    frame.render_widget(Paragraph::new(lines), rect);
    rect
}

/// Content width the empty states wrap their detail text to — a comfortable
/// reading measure that also leaves §B12's mockup proportions intact on a
/// wide terminal.
const EMPTY_STATE_WIDTH: usize = 62;

/// Extra columns the headline's `glyph + two spaces` prefix needs beyond the
/// wrapped detail measure.
const EMPTY_STATE_GLYPH_INDENT: u16 = 3;

/// Greedy word wrap to `width` columns, counting characters (the empty states
/// are plain prose plus an error string — no ANSI, no wide glyphs). A word
/// longer than `width` gets its own over-long row rather than being cut, so a
/// path or a URL in an error message stays selectable/copyable in full.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        let candidate = row.chars().count() + usize::from(!row.is_empty()) + word.chars().count();
        if !row.is_empty() && candidate > width {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        row.push_str(word);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// The failed screen's bordered Retry button, placed one row under the
/// message `block` it belongs to (keyboard parity: `r`). Omitted entirely
/// when the pane is too short for it — the key still works.
fn render_retry_button(
    frame: &mut Frame,
    area: Rect,
    block_rect: Rect,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    const WIDTH: u16 = 20;
    const HEIGHT: u16 = 3;
    if area.width < WIDTH {
        return;
    }
    let left = block_rect.x + (block_rect.width.saturating_sub(WIDTH)) / 2;
    let top = block_rect.bottom() + 1;
    if top + HEIGHT > area.bottom() {
        return;
    }
    let rect = Rect::new(left, top, WIDTH, HEIGHT);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(Line::styled(
            " ⟳  Retry",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )),
        inner,
    );
    mouse.click(rect, RegionId::DevtoolsRetry, Message::DevtoolsRetry);
}

/// The status row: the mode + connection label on the left, this screen's
/// keyhints beside it, and the shared mouse indicator on the right. The
/// whole label is a click region back to the log (keyboard parity: `Esc`).
fn render_status(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    session: &SessionView,
    phase: DevtoolsPhase,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface())),
        area,
    );
    let (label, label_color) = match phase {
        DevtoolsPhase::Unavailable => ("devtools: unavailable".to_string(), theme.warn()),
        DevtoolsPhase::Discovering => ("devtools: discovering".to_string(), theme.muted()),
        DevtoolsPhase::Connecting => ("devtools: connecting".to_string(), theme.muted()),
        DevtoolsPhase::Connected => (
            format!(
                "devtools: {}",
                session.devtools.active_tab.title().to_lowercase()
            ),
            theme.success(),
        ),
        DevtoolsPhase::Failed => ("devtools: failed".to_string(), theme.error()),
    };
    let retryable = matches!(phase, DevtoolsPhase::Failed) && !session.state.is_terminal();
    // Longest keyhint first, then progressively shorter ones — the graduated
    // degrade the log status row's filter chip established, so a narrow pane
    // drops detail instead of colliding with the mouse indicator. Every
    // screen but the Inspector has one hint short enough to always fit.
    let hints: &[&str] = match phase {
        // The connected hint is per-tab where a tab claims keys of its own
        // (§B12 draws each tab's own footer); the rest show the strip-level
        // keys they all share.
        DevtoolsPhase::Connected => match session.devtools.active_tab {
            DevtoolsTab::Inspector => &[
                "↑↓/jk move · →/Enter expand · ← collapse · Tab tree↔props · r refresh · Esc back",
                "↑↓ move · →← expand · Tab panes · r refresh · Esc back",
                "→← expand · r refresh · Esc back",
            ],
            DevtoolsTab::Performance | DevtoolsTab::System | DevtoolsTab::Network => {
                &["[1234][ ] tabs · Esc back to log"]
            }
        },
        _ if retryable => &["r retry · Esc back to log"],
        _ => &["Esc back to log"],
    };

    let label_width = label.chars().count() as u16;
    let indicator = crate::ui::mouse_indicator(state);
    // ` ` + label + ` │ ` + hint, with the right-aligned indicator's columns
    // reserved (it paints over this row afterwards).
    let available = area
        .width
        .saturating_sub(label_width + 4 + indicator.chars().count() as u16);
    let hint = hints
        .iter()
        .find(|hint| hint.chars().count() as u16 <= available)
        .or_else(|| hints.last())
        .copied()
        .unwrap_or_default();

    let spans = vec![
        Span::styled(" ".to_string(), Style::default()),
        Span::styled(label, Style::default().fg(label_color)),
        Span::styled(" │ ".to_string(), Style::default().fg(theme.border())),
        Span::styled(hint.to_string(), Style::default().fg(theme.muted())),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface())),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(indicator, Style::default().fg(theme.muted())))
            .alignment(Alignment::Right)
            .style(Style::default().bg(theme.surface())),
        area,
    );

    let width = (label_width + 1).min(area.width);
    mouse.click(
        Rect::new(area.x, area.y, width, 1),
        RegionId::DevtoolsBack,
        Message::DevtoolsClose,
    );
}

/// The `host:port` the connecting screen shows — the discovered port, on the
/// loopback address every connection actually targets (an Android session's
/// device port is mapped to a host one by the bridge, so this is the port the
/// app announced, which is what the user sees in its log).
fn endpoint(session: &SessionView) -> String {
    match &session.devtools.discovered {
        Some(discovery) => format!("127.0.0.1:{}", discovery.port),
        None => "127.0.0.1".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_words_breaks_on_word_boundaries_within_the_measure() {
        let rows = wrap_words(
            "connection refused after 3s — the service may have exited",
            24,
        );
        assert!(
            rows.iter().all(|r| r.chars().count() <= 24),
            "no row exceeds the measure: {rows:?}"
        );
        assert_eq!(rows.concat().replace(' ', "").len(), {
            let joined: String = rows.join(" ");
            joined.replace(' ', "").len()
        });
        assert!(rows.len() > 1, "long text actually wraps");
    }

    #[test]
    fn wrap_words_keeps_an_over_long_word_whole() {
        // A path or URL in an error message must stay intact rather than be
        // cut mid-token.
        let rows = wrap_words("at /a/very/long/path/that/exceeds/the/measure", 10);
        assert!(rows.iter().any(|r| r.contains("/a/very/long/path")));
    }

    #[test]
    fn wrap_words_on_empty_text_is_one_empty_row() {
        assert_eq!(wrap_words("", 20), vec![String::new()]);
    }
}
