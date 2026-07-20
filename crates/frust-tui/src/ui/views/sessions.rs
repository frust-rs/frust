//! The session tab bar and the ANSI-aware log view (PLAN.md D5): tabs grouped
//! by project, per-session follow-tail / scroll / wrap / search-filter, level
//! colorize, and a copy-while-scrolling selection highlight.
//!
//! Layering (D2): every function here renders `&AppState` and only *registers*
//! interaction (tab clicks, the log scroll region) through the [`MouseCtx`] —
//! it never mutates the engine. The scroll/wrap/window math is factored into
//! pure helpers (`hard_wrap`, `display_window`) unit-tested below without a TTY.

use std::collections::VecDeque;

use ansi_to_tui::IntoText;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};

use crate::engine::{AppState, Message, RegionId, Scroll, SessionView, detect_level, line_matches};
use crate::supervise::SessionState;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Render the whole session workspace (tab bar + log view + log status + search
/// overlay) into `area`, registering the tab click targets and the log scroll
/// region. Called only when `state.sessions` is non-empty.
pub fn render_main(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let mut constraints = vec![
        Constraint::Length(1), // tab bar
        Constraint::Min(1),    // log view
        Constraint::Length(1), // log status / keyhints
    ];
    if state.search.open {
        constraints.push(Constraint::Length(1)); // search input overlay
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_tab_bar(frame, rows[0], state, theme, mouse);
    render_log(frame, rows[1], state, theme, mouse);
    render_log_status(frame, rows[2], state, theme, mouse);
    if state.search.open {
        render_search(frame, rows[3], state, theme);
    }
}

/// The status glyph + color for a session's lifecycle state.
fn status_style(s: &SessionState, theme: &Theme) -> (&'static str, Color) {
    match s {
        SessionState::Configuring => ("\u{25cc}", theme.muted()), // ◌
        SessionState::Building => ("\u{25d0}", theme.warn()),     // ◐
        SessionState::Installing => ("\u{25d1}", theme.warn()),   // ◑
        SessionState::Running => ("\u{25b6}", theme.success()),   // ▶
        SessionState::Exited(true) => ("\u{2713}", theme.muted()), // ✓
        SessionState::Exited(false) => ("\u{2717}", theme.error()), // ✗
        SessionState::Killed => ("\u{25a0}", theme.muted()),      // ■
    }
}

/// Render the tab bar: sessions grouped by project (a muted `name:` label per
/// group), each tab numbered `1`–`9` where jumpable, the active tab accented.
fn render_tab_bar(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface())),
        area,
    );

    let mut spans: Vec<Span<'static>> = Vec::new();
    // Column cursor (relative to `area.x`) so click rects land on the terminal.
    let mut col: u16 = 0;
    let push = |spans: &mut Vec<Span<'static>>, col: &mut u16, text: String, style: Style| {
        *col += text.chars().count() as u16;
        spans.push(Span::styled(text, style));
    };

    for (gi, (root, group)) in state.sessions_grouped().into_iter().enumerate() {
        if gi > 0 {
            push(&mut spans, &mut col, "  ".into(), Style::default());
        }
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        push(
            &mut spans,
            &mut col,
            format!("{name}: "),
            Style::default().fg(theme.muted()),
        );

        for (idx, session) in group {
            let active = state.active_session == Some(idx);
            let (glyph, glyph_color) = status_style(&session.state, theme);
            let number = if idx < 9 {
                format!("{} ", idx + 1)
            } else {
                String::new()
            };
            let label = format!(" {number}{glyph} {} ", session.target_label);

            let tab_x = area.x + col;
            let tab_style = if active {
                Style::default()
                    .fg(theme.bg())
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg()).bg(theme.overlay())
            };
            // The glyph keeps its status color on an inactive tab; on the
            // active (accent-filled) tab the whole label reads as one chip.
            let width = label.chars().count() as u16;
            if active {
                push(&mut spans, &mut col, label, tab_style);
            } else {
                // Split so the status glyph shows its own color.
                push(
                    &mut spans,
                    &mut col,
                    format!(" {number}"),
                    tab_style.fg(theme.muted()),
                );
                push(
                    &mut spans,
                    &mut col,
                    glyph.to_string(),
                    tab_style.fg(glyph_color),
                );
                push(
                    &mut spans,
                    &mut col,
                    format!(" {} ", session.target_label),
                    tab_style,
                );
            }
            push(&mut spans, &mut col, " ".into(), Style::default());
            col += 1; // trailing gap

            // Register the click target (clamped to the row).
            if tab_x < area.right() {
                let w = width.min(area.right().saturating_sub(tab_x));
                mouse.click(
                    Rect::new(tab_x, area.y, w, 1),
                    RegionId::SessionTab(idx),
                    Message::SelectTab(idx),
                );
            }
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Render the active session's log view: ANSI-parsed, level-colorized,
/// optionally wrapped, windowed by the scroll position, with the selection
/// highlighted. Registers the pointer-aware scroll region.
fn render_log(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.bg())),
        area,
    );

    let Some(session) = state.active_session() else {
        return;
    };

    // Wheel over the log scrolls it (not a global focus) — one line per notch.
    mouse.scroll(area, Message::LogScrollUp(1), Message::LogScrollDown(1));
    mouse.hover(area, RegionId::LogView);

    if session.log.is_empty() {
        let hint = match session.state {
            SessionState::Exited(_) | SessionState::Killed => "session ended · no output",
            _ => "waiting for output…",
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("  {hint}"),
                Style::default().fg(theme.muted()),
            )),
            area,
        );
        return;
    }

    let vis = visible_indices(session, state.search.filter.as_deref());
    if vis.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    "  no lines match /{}/",
                    state.search.filter.clone().unwrap_or_default()
                ),
                Style::default().fg(theme.muted()),
            )),
            area,
        );
        return;
    }

    let lines = display_window(session, &vis, state.wrap, area.width, area.height, theme);
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// The log status / keyhint line under the log view. Registers the
/// copy-built-artifacts click region when the active session reported any
/// (see `SessionView::built_artifact_paths`).
fn render_log_status(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface())),
        area,
    );
    let inner = Rect::new(area.x, area.y, area.width, 1);

    let Some(session) = state.active_session() else {
        return;
    };
    let follow = if session.is_following() {
        Span::styled("● follow", Style::default().fg(theme.success()))
    } else {
        Span::styled("‖ scrolled", Style::default().fg(theme.warn()))
    };
    let wrap = if state.wrap { "wrap:on" } else { "wrap:off" };
    let mut left = vec![
        follow,
        Span::styled("  ·  ", Style::default().fg(theme.border())),
        Span::styled(wrap.to_string(), Style::default().fg(theme.muted())),
    ];
    if let Some(f) = &state.search.filter {
        left.push(Span::styled("  ·  ", Style::default().fg(theme.border())));
        left.push(Span::styled(
            format!("/{f}/"),
            Style::default().fg(theme.accent()),
        ));
    }
    if session.selection.is_some() {
        left.push(Span::styled("  ·  ", Style::default().fg(theme.border())));
        left.push(Span::styled("y copy", Style::default().fg(theme.accent())));
    }

    const RIGHT_HINT: &str = "x stop · f follow · w wrap · / search";

    // Only add the built-artifacts segment (with its copy-path click region)
    // when it actually fits beside the right-aligned keyhint — both
    // `Paragraph`s share this one row, so an unchecked append can overlap and
    // garble both (narrow terminals, or a session with many artifacts). The
    // mouse action degrades gracefully when it doesn't fit; `c` still copies
    // via the keyboard regardless (CODE_STANDARDS' mouse-is-additive policy).
    let artifacts = session.built_artifact_paths();
    let mut copy_click: Option<(u16, u16)> = None; // (x, width), relative to `inner.x`
    if !artifacts.is_empty() {
        let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
        let prefix = format!("  ·  {} built · ", artifacts.len());
        let copy_label = "c copy path";
        let needed = used + prefix.chars().count() + copy_label.chars().count();
        if needed + RIGHT_HINT.chars().count() + 2 <= inner.width as usize {
            let prefix_len = prefix.chars().count();
            left.push(Span::styled(prefix, Style::default().fg(theme.success())));
            left.push(Span::styled(
                copy_label,
                Style::default().fg(theme.accent()),
            ));
            copy_click = Some((
                (used + prefix_len) as u16,
                copy_label.chars().count() as u16,
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(left)), inner);
    if let Some((x, w)) = copy_click {
        mouse.click(
            Rect::new(
                inner.x + x,
                inner.y,
                w.min(inner.right().saturating_sub(inner.x + x)),
                1,
            ),
            RegionId::CopyArtifactsAction,
            Message::CopyBuiltArtifacts,
        );
    }

    frame.render_widget(
        Paragraph::new(Line::styled(RIGHT_HINT, Style::default().fg(theme.muted())))
            .alignment(Alignment::Right),
        inner,
    );
}

/// The search-input overlay row (shown while the overlay is open).
fn render_search(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.overlay())),
        area,
    );
    let line = Line::from(vec![
        Span::styled(
            " / ",
            Style::default()
                .fg(theme.bg())
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", state.search.query),
            Style::default().fg(theme.fg()),
        ),
        Span::styled("\u{2588}", Style::default().fg(theme.accent())), // block caret
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ── Sidebar session list (grouped, with status glyphs) ──────────────────────

/// Render the sidebar SESSIONS section body as grouped lines with status
/// glyphs. Returns the lines (the caller composes them into the sidebar). The
/// active session gets the accent chevron.
pub fn sidebar_lines(state: &AppState, theme: &Theme) -> Vec<Line<'static>> {
    if state.sessions.is_empty() {
        return vec![Line::styled(
            "  none running",
            Style::default().fg(theme.muted()),
        )];
    }
    let mut lines = Vec::new();
    for (root, group) in state.sessions_grouped() {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        lines.push(Line::styled(
            format!("  {name}"),
            Style::default().fg(theme.muted()),
        ));
        for (idx, session) in group {
            let active = state.active_session == Some(idx);
            let (glyph, color) = status_style(&session.state, theme);
            let marker = if active { "\u{25b8}" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(format!("  {marker} "), Style::default().fg(theme.accent())),
                Span::styled(glyph.to_string(), Style::default().fg(color)),
                Span::styled(
                    format!(" {}", session.target_label),
                    if active {
                        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.muted())
                    },
                ),
            ]));
        }
    }
    lines
}

// ── Pure log-window helpers (unit-tested below) ─────────────────────────────

/// The absolute indices of the log lines currently visible under `filter`
/// (all of them when `filter` is `None`), oldest first.
fn visible_indices(session: &SessionView, filter: Option<&str>) -> Vec<u64> {
    match filter {
        None => session.log.iter().map(|(i, _)| i).collect(),
        Some(q) => session
            .log
            .iter()
            .filter(|(_, s)| line_matches(s, q))
            .map(|(i, _)| i)
            .collect(),
    }
}

/// The position within `vis` of the bottom-most visible line for the current
/// scroll: the last match when following, else the newest match at or before
/// the absolute anchor.
fn bottom_pos(session: &SessionView, vis: &[u64]) -> usize {
    match session.scroll {
        Scroll::Follow => vis.len().saturating_sub(1),
        Scroll::Anchored(b) => match vis.binary_search(&b) {
            Ok(p) => p,
            Err(0) => 0,
            Err(p) => p - 1,
        },
    }
}

/// Build the ≤`height` display rows ending at the bottom-anchored line: walk
/// visible lines upward from `bottom_pos`, ANSI-parse + (hard-)wrap each, and
/// keep the last `height` display rows so the bottom line sits at the bottom of
/// the viewport. Exact wrap bounds by construction (we count the very rows we
/// render — no word-wrap/scroll-offset mismatch).
fn display_window(
    session: &SessionView,
    vis: &[u64],
    wrap: bool,
    width: u16,
    height: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if vis.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }
    let h = height as usize;
    let w = width as usize;
    let mut rows: VecDeque<Line<'static>> = VecDeque::new();
    let mut pos = bottom_pos(session, vis) as isize;
    while pos >= 0 && rows.len() < h {
        let abs = vis[pos as usize];
        let raw = session.log.get(abs).unwrap_or("");
        let styled = ansi_line(raw, abs, session, theme);
        let wrapped = if wrap {
            hard_wrap(&styled, w)
        } else {
            vec![styled]
        };
        for l in wrapped.into_iter().rev() {
            rows.push_front(l);
        }
        pos -= 1;
    }
    while rows.len() > h {
        rows.pop_front();
    }
    rows.into_iter().collect()
}

/// Parse one raw (possibly ANSI-colored) log line into an owned styled `Line`,
/// applying a level colorize (only when the line carries no ANSI color of its
/// own) and the selection-highlight background (absolute-index selection, so it
/// tracks the same lines across incoming output).
fn ansi_line(raw: &str, abs: u64, session: &SessionView, theme: &Theme) -> Line<'static> {
    let text = raw
        .into_text()
        .unwrap_or_else(|_| Text::from(raw.to_string()));
    // Flatten to a single line (our lines are newline-stripped upstream, but a
    // stray embedded newline is joined rather than dropped).
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (li, line) in text.lines.into_iter().enumerate() {
        if li > 0 {
            spans.push(Span::raw(" "));
        }
        spans.extend(line.spans);
    }
    let mut out = Line::from(spans);

    let has_ansi_color = out.spans.iter().any(|s| s.style.fg.is_some());
    if !has_ansi_color && let Some(level) = detect_level(&crate::engine::strip_ansi(raw)) {
        out.style = out.style.fg(level_color(level, theme));
    }
    if session.selection.is_some_and(|s| s.contains(abs)) {
        out.style = out.style.bg(theme.overlay());
    }
    out
}

/// The foreground color for a detected log level.
fn level_color(level: crate::engine::LogLevel, theme: &Theme) -> Color {
    match level {
        crate::engine::LogLevel::Error => theme.error(),
        crate::engine::LogLevel::Warn => theme.warn(),
    }
}

/// Hard-wrap a styled line to at most `width` columns per row, splitting spans
/// (and carrying the line-level style onto every wrapped row). Column width is
/// approximated as one column per char — exact for the ASCII-dominant log
/// output the view renders; a wide-char line simply wraps a touch early.
fn hard_wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line.clone()];
    }
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    let flush = |rows: &mut Vec<Line<'static>>, cur: &mut Vec<Span<'static>>, style: Style| {
        let mut l = Line::from(std::mem::take(cur));
        l.style = style;
        rows.push(l);
    };
    for span in &line.spans {
        let style = span.style;
        let mut buf = String::new();
        for ch in span.content.chars() {
            buf.push(ch);
            col += 1;
            if col >= width {
                cur.push(Span::styled(std::mem::take(&mut buf), style));
                flush(&mut rows, &mut cur, line.style);
                col = 0;
            }
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, style));
        }
    }
    if !cur.is_empty() || rows.is_empty() {
        flush(&mut rows, &mut cur, line.style);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::LineSelection;
    use crate::supervise::SessionId;
    use std::path::PathBuf;

    fn theme() -> Theme {
        Theme::frust_dark_at(crate::ui::theme::ColorDepth::TrueColor)
    }

    fn session(n: u64) -> SessionView {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/app"), "desktop");
        for i in 0..n {
            s.push_line(format!("line {i}"));
        }
        s
    }

    #[test]
    fn hard_wrap_splits_at_width_and_preserves_text() {
        let line = Line::from("abcdefghij".to_string());
        let rows = hard_wrap(&line, 4);
        let joined: Vec<String> = rows
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(joined, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn hard_wrap_carries_line_style_onto_every_row() {
        let mut line = Line::from("abcdef".to_string());
        line.style = Style::default().fg(Color::Red);
        let rows = hard_wrap(&line, 3);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.style.fg == Some(Color::Red)));
    }

    #[test]
    fn follow_window_shows_the_last_height_lines() {
        let s = session(20);
        let vis = visible_indices(&s, None);
        let rows = display_window(&s, &vis, false, 40, 5, &theme());
        assert_eq!(rows.len(), 5);
        let texts: Vec<String> = rows
            .iter()
            .map(|l| l.spans.iter().map(|x| x.content.as_ref()).collect())
            .collect();
        assert_eq!(
            texts,
            vec!["line 15", "line 16", "line 17", "line 18", "line 19"]
        );
    }

    #[test]
    fn anchored_window_keeps_the_anchor_at_the_bottom() {
        let mut s = session(20);
        s.scroll = Scroll::Anchored(9); // line 9 at the bottom
        let vis = visible_indices(&s, None);
        let rows = display_window(&s, &vis, false, 40, 3, &theme());
        let texts: Vec<String> = rows
            .iter()
            .map(|l| l.spans.iter().map(|x| x.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec!["line 7", "line 8", "line 9"]);
    }

    #[test]
    fn filter_restricts_visible_indices() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        s.push_line("hello world".into());
        s.push_line("ERROR boom".into());
        s.push_line("hello again".into());
        let vis = visible_indices(&s, Some("hello"));
        assert_eq!(vis, vec![0, 2]);
    }

    #[test]
    fn selection_applies_a_highlight_background() {
        let mut s = session(5);
        s.selection = Some(LineSelection {
            anchor: 1,
            cursor: 2,
        });
        let vis = visible_indices(&s, None);
        let rows = display_window(&s, &vis, false, 40, 5, &theme());
        // rows: line0..line4; lines 1 and 2 carry the overlay bg.
        assert!(rows[0].style.bg.is_none());
        assert!(rows[1].style.bg.is_some());
        assert!(rows[2].style.bg.is_some());
        assert!(rows[3].style.bg.is_none());
    }
}
