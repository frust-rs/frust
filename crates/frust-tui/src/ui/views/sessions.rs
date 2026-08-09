//! The session tab bar and the ANSI-aware log view: tabs grouped
//! by project, per-session follow-tail / scroll / wrap / search-filter /
//! level-filter, level+source+timestamp styling (workbook §B11), Rust
//! panic/backtrace fold rows, and a copy-while-scrolling selection highlight.
//!
//! Layering: every function here renders `&AppState` and only *registers*
//! interaction (tab clicks, the log scroll region, fold-row/filter-chip
//! clicks) through the [`MouseCtx`] — it never mutates the engine. The
//! wrap/window math is factored into pure helpers (`hard_wrap`,
//! `display_window`) unit-tested below without a TTY; which lines are visible
//! at all, and where the anchor sits among them, is engine state logic
//! ([`SessionView::visible_indices`]/[`SessionView::bottom_pos`]) shared with
//! the scroll mutators so a rendered row and a scroll step always mean the
//! same thing.
//!
//! §B11's badge/timestamp/source-tag prefix chrome and the ANSI-passthrough
//! rule live in [`build_row`]/[`line_prefix`] — the one styling authority
//! (`LineLevel`/`LogSource` classification itself lives in
//! [`crate::engine::logstyle`], computed once at push time, never re-derived
//! here).

use std::collections::VecDeque;

use ansi_to_tui::IntoText;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Sparkline};

use crate::engine::{
    AppState, ContextTarget, DragKind, LEVEL_FILTER_SEGMENTS, LevelFilter, LineMeta, LineRole,
    LogLevel, Message, PanicBlock, RegionId, SOURCE_TAG_WIDTH, SessionView,
};
use crate::supervise::SessionState;
use crate::ui::anim::{SPINNER_TICKS_PER_FRAME, spinner_char, themed_shimmer_spans};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// The badge/timestamp/source-tag prefix's total column width — one leading
/// space + a 1-column badge + one space + an 8-column `HH:MM:SS` timestamp +
/// one space + the (padded) source tag + one trailing space. A wrapped
/// continuation row and every non-first line of a panic block indent under
/// this boundary instead of repeating the chrome (workbook §B11).
const PREFIX_WIDTH: usize = 1 + 1 + 1 + 8 + 1 + SOURCE_TAG_WIDTH + 1;

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
    // The perf sparkline panel
    // renders only once its session has actually seen a `frust-perf` line
    // *and* the tab has toggled it open (`t`) — zero-noise otherwise.
    let active_session = state.active_session();
    let show_perf = active_session.is_some_and(|s| s.perf.visible && s.perf.has_data());
    // The transient build/install phase status line (workbook §B10) —
    // reserved only while the active session is actually `Building`/
    // `Installing`, so a streaming/terminal session's log pane loses no rows
    // to a hint it no longer needs.
    let show_phase = active_session
        .is_some_and(|s| matches!(s.state, SessionState::Building | SessionState::Installing));

    let mut constraints = vec![Constraint::Length(1)]; // tab bar
    if show_phase {
        constraints.push(Constraint::Length(1)); // phase status line
    }
    constraints.push(Constraint::Min(1)); // log view
    constraints.push(Constraint::Length(1)); // log status / keyhints
    if show_perf {
        constraints.push(Constraint::Length(perf_panel_height(
            active_session.unwrap(),
        )));
    }
    if state.search.open {
        constraints.push(Constraint::Length(1)); // search input overlay
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_tab_bar(frame, rows[0], state, theme, mouse);
    let mut next = 1;
    if show_phase {
        render_phase_line(
            frame,
            rows[next],
            active_session.unwrap(),
            state.animation_frame,
            theme,
        );
        next += 1;
    }
    render_log(frame, rows[next], state, theme, mouse);
    next += 1;
    render_log_status(frame, rows[next], state, theme, mouse);
    next += 1;
    if show_perf {
        render_perf_panel(frame, rows[next], active_session.unwrap(), theme);
        next += 1;
    }
    if state.search.open {
        render_search(frame, rows[next], state, theme);
    }
}

/// The transient build/install phase status line (workbook §B10), shown
/// directly under the tab bar only while the active session is `Building` or
/// `Installing` (see [`render_main`]'s `show_phase` gate). A parsed phase
/// label shimmers (accent sweep over the muted base, via
/// [`themed_shimmer_spans`]); `Building` with no label parsed yet falls back
/// to a *plain* (unshimmered) `⠋ Building…` — shimmering a fallback string
/// would imply progress the parser doesn't actually have. `Installing`
/// always shimmers — there's no meaningfully "unknown" sub-case for install
/// the way there is for a multi-crate build.
fn render_phase_line(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    animation_frame: u64,
    theme: &Theme,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface())),
        area,
    );
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    match (&session.state, &session.current_phase) {
        (SessionState::Installing, Some(phase)) | (SessionState::Building, Some(phase)) => {
            spans.extend(themed_shimmer_spans(
                &phase.text(),
                animation_frame,
                theme,
                Modifier::empty(),
            ));
        }
        (SessionState::Installing, None) => {
            spans.extend(themed_shimmer_spans(
                "Installing\u{2026}",
                animation_frame,
                theme,
                Modifier::empty(),
            ));
        }
        (SessionState::Building, None) => {
            let glyph = spinner_char(animation_frame / SPINNER_TICKS_PER_FRAME);
            spans.push(Span::styled(
                format!("{glyph} Building\u{2026}"),
                Style::default().fg(theme.muted()),
            ));
        }
        _ => return, // render_main only calls this while Building/Installing
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The status glyph + color for a session's lifecycle state. `animation_frame`
/// drives an animated braille spinner (workbook §B10's tab-glyph vocabulary)
/// substituted for the static glyph while the session is actively `Building`
/// or `Installing`; every other state (including `Configuring`, which has no
/// per-toolchain phase to animate against) keeps its existing static glyph —
/// a terminal state's glyph in particular never spins, "freezing" on its own
/// static symbol once the build/install phase ends (§B10's failed row).
fn status_style(s: &SessionState, animation_frame: u64, theme: &Theme) -> (String, Color) {
    match s {
        SessionState::Configuring => ("\u{25cc}".to_string(), theme.muted()), // ◌
        SessionState::Building | SessionState::Installing => (
            spinner_char(animation_frame / SPINNER_TICKS_PER_FRAME).to_string(),
            theme.accent(),
        ),
        SessionState::Running => ("\u{25b6}".to_string(), theme.success()), // ▶
        SessionState::Exited(true) => ("\u{2713}".to_string(), theme.muted()), // ✓
        SessionState::Exited(false) => ("\u{2717}".to_string(), theme.error()), // ✗
        SessionState::Killed => ("\u{25a0}".to_string(), theme.muted()),    // ■
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
            let (glyph, glyph_color) = status_style(&session.state, state.animation_frame, theme);
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
                push(&mut spans, &mut col, glyph, tab_style.fg(glyph_color));
                push(
                    &mut spans,
                    &mut col,
                    format!(" {} ", session.target_label),
                    tab_style,
                );
            }
            push(&mut spans, &mut col, " ".into(), Style::default());
            col += 1; // trailing gap

            // Register the click + right-click-context target (clamped to row).
            if tab_x < area.right() {
                let w = width.min(area.right().saturating_sub(tab_x));
                let rect = Rect::new(tab_x, area.y, w, 1);
                mouse.click(rect, RegionId::SessionTab(idx), Message::SelectTab(idx));
                mouse.context(rect, ContextTarget::SessionTab(idx));
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
    // Right-click over the log pane opens its context menu (copy/follow/search).
    mouse.context(area, ContextTarget::LogView);

    if session.log.is_empty() {
        // Pre-first-line placeholder (workbook §B10): a transient session
        // (still `Configuring`/`Building`/`Installing`, no output at all
        // yet) gets a centered spinner + "waiting for X" message instead of
        // the plain top-left hint every other empty state uses — no empty
        // table chrome, and the first real line replaces it in place with no
        // layout jump (this branch returns before any row/scrollbar setup).
        if matches!(
            session.state,
            SessionState::Configuring | SessionState::Building | SessionState::Installing
        ) {
            let glyph = spinner_char(state.animation_frame / SPINNER_TICKS_PER_FRAME);
            let line = Line::from(vec![
                Span::styled(glyph.to_string(), Style::default().fg(theme.accent())),
                Span::styled(
                    format!(
                        "  Waiting for first output from {}\u{2026}",
                        session.target_label
                    ),
                    Style::default().fg(theme.muted()),
                ),
            ]);
            let mid_y = area.y + area.height / 2;
            frame.render_widget(
                Paragraph::new(line).alignment(Alignment::Center),
                Rect::new(area.x, mid_y, area.width, 1.min(area.height)),
            );
            return;
        }
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

    let vis = session.visible_indices(state.search.filter.as_deref());
    if vis.is_empty() {
        let hint = if let Some(f) = &state.search.filter {
            format!("  no lines match /{f}/")
        } else {
            format!(
                "  no lines at the \"{}\" level filter",
                session.level_filter.label()
            )
        };
        frame.render_widget(
            Paragraph::new(Line::styled(hint, Style::default().fg(theme.muted()))),
            area,
        );
        return;
    }

    let rows = display_window(session, &vis, state.wrap, area.width, area.height, theme);
    let lines: Vec<Line<'static>> = rows.iter().map(|(l, _)| l.clone()).collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);

    // A collapsed panic block's synthetic `▶ n frames…` row is its own click
    // target (mouse parity for `z` — see `Message::ToggleFold`).
    for (i, (_, fold_id)) in rows.iter().enumerate() {
        if let Some(block_start) = fold_id {
            let rect = Rect::new(area.x, area.y + i as u16, area.width, 1);
            mouse.click(
                rect,
                RegionId::LogFoldToggle(*block_start),
                Message::ToggleFold(*block_start),
            );
        }
    }

    render_scrollbar(frame, area, session, &vis, theme, mouse);
}

/// Draw the log-view scrollbar thumb and register its drag region.
///
/// Only shown when the visible line count overflows the viewport (a zero-noise
/// affordance on a short log). The thumb marks the bottom-anchored line's
/// position; grabbing anywhere on the track begins a `LogScrollbar` drag whose
/// row maps to a scroll fraction (see
/// [`crate::engine::SessionView::scroll_to_fraction`]). The whole rightmost
/// column is the track; the thumb is drawn over it, leaving content untouched
/// (log lines rarely reach the last column, so snapshot impact is one cell).
fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    vis: &[u64],
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.width == 0 || area.height <= 1 || vis.len() <= area.height as usize {
        return;
    }
    let track_x = area.right() - 1;
    let track_top = area.y;
    let track_height = area.height;

    // The fraction of the way down the log the bottom-visible entry sits —
    // measured in the same visible sequence `scroll_to_fraction` maps a drag
    // back onto, so thumb and content never disagree.
    let bottom = session.bottom_pos(vis);
    let denom = vis.len().saturating_sub(1).max(1) as f32;
    let frac = (bottom as f32 / denom).clamp(0.0, 1.0);
    let thumb_y = track_top + (frac * (track_height - 1) as f32).round() as u16;

    frame.render_widget(
        Paragraph::new(Line::styled(
            "\u{2588}", // █ thumb
            Style::default().fg(theme.accent()),
        )),
        Rect::new(track_x, thumb_y, 1, 1),
    );

    mouse.drag(
        Rect::new(track_x, track_top, 1, track_height),
        DragKind::LogScrollbar {
            track_top,
            track_height,
        },
    );
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

    // `l filter`/`z fold`/`t perf` are appended only while there's still
    // room beside the (always-shown) base hint and the `left` segments
    // already built above — the same fit-check spirit the built-artifacts
    // segment and the filter chip below use, so a narrow terminal degrades
    // one hint at a time (mouse/base-key parity never lost) instead of the
    // right-aligned hint silently overwriting the tail of `left`'s text.
    let used_left: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let mut right_hint = "x stop · f follow · w wrap · / search".to_string();
    let push_hint_if_it_fits = |hint: &mut String, extra: &str| {
        if used_left + hint.chars().count() + extra.chars().count() + 2 <= inner.width as usize {
            hint.push_str(extra);
        }
    };
    push_hint_if_it_fits(&mut right_hint, " · l filter");
    if session.has_panic_blocks() {
        push_hint_if_it_fits(&mut right_hint, " · z fold");
    }
    if session.perf.has_data() {
        push_hint_if_it_fits(&mut right_hint, " · t perf");
    }
    let right_hint = right_hint;

    // Level-filter chip (workbook §B11): a segmented pill, the active
    // segment accent-filled — `l`/`L` cycles it by key, a click jumps
    // straight to a segment. Skipped (mouse-only degrade; `l`/`L` still
    // work) when it wouldn't fit beside the right-aligned keyhint — the same
    // fit-check pattern the built-artifacts segment below uses.
    let mut chip_clicks: Vec<(u16, u16, LevelFilter)> = Vec::new();
    {
        let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
        let sep = "  ·  ";
        let mut chip_spans: Vec<Span<'static>> =
            vec![Span::styled(sep, Style::default().fg(theme.border()))];
        let mut col = sep.chars().count();
        let mut rects = Vec::new();
        for seg in LEVEL_FILTER_SEGMENTS {
            let label = format!(" {} ", seg.label());
            let width = label.chars().count();
            let style = if seg == session.level_filter {
                Style::default()
                    .fg(theme.bg())
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted())
            };
            rects.push((used + col, width, seg));
            chip_spans.push(Span::styled(label, style));
            col += width;
        }
        let hidden = level_filter_hidden_count(session);
        let hidden_note = if hidden > 0 {
            format!(" {hidden} hidden by filter")
        } else {
            String::new()
        };
        col += hidden_note.chars().count();
        if used + col + right_hint.chars().count() + 2 <= inner.width as usize {
            left.extend(chip_spans);
            if !hidden_note.is_empty() {
                left.push(Span::styled(
                    hidden_note,
                    Style::default().fg(theme.muted()),
                ));
            }
            for (x, w, seg) in rects {
                chip_clicks.push((x as u16, w as u16, seg));
            }
        }
    }

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
        if needed + right_hint.chars().count() + 2 <= inner.width as usize {
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
        Paragraph::new(Line::styled(right_hint, Style::default().fg(theme.muted())))
            .alignment(Alignment::Right),
        inner,
    );

    for (x, w, seg) in chip_clicks {
        mouse.click(
            Rect::new(
                inner.x + x,
                inner.y,
                w.min(inner.right().saturating_sub(inner.x + x)),
                1,
            ),
            RegionId::LevelFilterSegment(seg),
            Message::SetLevelFilter(seg),
        );
    }
}

/// The number of currently-retained lines hidden solely by the session's
/// active level filter (independent of the free-text search filter) — the
/// filter chip's "N hidden by filter" note (workbook §B11).
fn level_filter_hidden_count(session: &SessionView) -> usize {
    session
        .log
        .iter()
        .filter(|(abs, _)| {
            session
                .effective_level(*abs)
                .is_some_and(|lvl| !session.level_filter.allows(lvl))
        })
        .count()
}

/// The perf panel's row height: a sparkline row + a stats row, plus one more
/// once a startup summary has been seen (a session emits that line once,
/// near the start of its output).
fn perf_panel_height(session: &SessionView) -> u16 {
    let base = 2;
    base + u16::from(session.perf.last_startup.is_some())
}

/// The perf sparkline panel:
/// recent frame totals as a sparkline (only present once `FRUST_TRACE_RAW`
/// has produced per-frame samples), a p50/p95/p99 stats line from the latest
/// periodic summary, and the one-shot startup-span line when seen.
fn render_perf_panel(frame: &mut Frame, area: Rect, session: &SessionView, theme: &Theme) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface())),
        area,
    );
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut y = area.y;

    // Sparkline row.
    let samples: Vec<u64> = session.perf.samples().collect();
    if samples.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  no per-frame samples yet — set FRUST_TRACE_RAW=1 for the sparkline",
                Style::default().fg(theme.muted()),
            )),
            Rect::new(area.x, y, area.width, 1),
        );
    } else {
        let indent = 2.min(area.width);
        let spark_x = area.x + indent;
        frame.render_widget(
            Sparkline::default()
                .data(&samples)
                .style(Style::default().fg(theme.accent())),
            Rect::new(spark_x, y, area.width.saturating_sub(indent), 1),
        );
    }
    y += 1;
    if y >= area.bottom() {
        return;
    }

    // Stats row.
    let stats = match &session.perf.last_frame {
        Some(f) => format!(
            "  perf n={} p50={}ms p95={}ms p99={}ms skipped={} total_frames={}",
            f.n, f.total_p50_ms, f.total_p95_ms, f.total_p99_ms, f.skipped, f.total_frames
        ),
        None => "  perf: waiting for a frame summary…".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(stats, Style::default().fg(theme.fg()))),
        Rect::new(area.x, y, area.width, 1),
    );
    y += 1;
    if y >= area.bottom() {
        return;
    }

    // Startup-span row (only once seen).
    if let Some(startup) = &session.perf.last_startup {
        let spans: Vec<String> = startup
            .spans
            .iter()
            .map(|(name, ms)| format!("{name}={ms}ms"))
            .collect();
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("  startup: {}", spans.join(" ")),
                Style::default().fg(theme.muted()),
            )),
            Rect::new(area.x, y, area.width, 1),
        );
    }
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
            let (glyph, color) = status_style(&session.state, state.animation_frame, theme);
            let marker = if active { "\u{25b8}" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(format!("  {marker} "), Style::default().fg(theme.accent())),
                Span::styled(glyph, Style::default().fg(color)),
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

/// Build the ≤`height` display rows ending at the bottom-anchored entry: walk
/// the visible sequence upward from [`SessionView::bottom_pos`], ANSI-parse +
/// (hard-)wrap each, and keep the last `height` display rows so the bottom
/// entry sits at the bottom of the viewport. Exact wrap bounds by
/// construction (we count the very rows we render — no word-wrap/scroll-offset
/// mismatch).
///
/// `vis` is a [`SessionView::visible_indices`] sequence, which already folds a
/// collapsed panic block's whole body into one entry; this draws that entry as
/// the synthetic `▶ n frames…` row. The second element of each returned pair
/// is `Some(block_start)` for that row (the click target; [`render_log`]
/// registers it), `None` for an ordinary line.
fn display_window(
    session: &SessionView,
    vis: &[u64],
    wrap: bool,
    width: u16,
    height: u16,
    theme: &Theme,
) -> Vec<(Line<'static>, Option<u64>)> {
    if vis.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }
    let h = height as usize;
    let w = width as usize;
    let mut rows: VecDeque<(Line<'static>, Option<u64>)> = VecDeque::new();
    let mut pos = session.bottom_pos(vis) as isize;
    while pos >= 0 && rows.len() < h {
        let abs = vis[pos as usize];
        if let Some(block) = session.panic_block_covering(abs)
            && session.is_fold_collapsed(block.start)
        {
            let block_start = block.start;
            let row = fold_row(block, theme);
            let wrapped = if wrap {
                hard_wrap(&row, w, PREFIX_WIDTH)
            } else {
                vec![row]
            };
            for l in wrapped.into_iter().rev() {
                rows.push_front((l, Some(block_start)));
            }
            pos -= 1;
            continue;
        }
        let styled = build_row(session, abs, theme);
        let wrapped = if wrap {
            hard_wrap(&styled, w, PREFIX_WIDTH)
        } else {
            vec![styled]
        };
        for l in wrapped.into_iter().rev() {
            rows.push_front((l, None));
        }
        pos -= 1;
    }
    while rows.len() > h {
        rows.pop_front();
    }
    rows.into_iter().collect()
}

/// The `▶ n frames…` fold affordance row for a collapsed panic block —
/// clickable (mouse) and re-expandable via `z` (keyboard); see
/// [`crate::engine::Message::ToggleFold`]/[`ToggleNearestFold`].
///
/// [`ToggleNearestFold`]: crate::engine::Message::ToggleNearestFold
fn fold_row(block: &PanicBlock, theme: &Theme) -> Line<'static> {
    let mut spans = blank_prefix();
    spans.push(Span::styled(
        format!("\u{25b6} {} frames\u{2026} ", block.frame_count),
        Style::default().fg(theme.accent()),
    ));
    spans.push(Span::styled(
        "(RUST_BACKTRACE=1) \u{2014} click or z to expand",
        Style::default().fg(theme.muted()),
    ));
    Line::from(spans)
}

/// The badge/timestamp/source-tag prefix chrome for a line that shows it
/// (workbook §B11's grammar: `badge · timestamp · source · message`).
fn line_prefix(meta: &LineMeta, theme: &Theme) -> Vec<Span<'static>> {
    let badge = meta
        .level
        .badge_char()
        .map(|c| c.to_string())
        .unwrap_or_else(|| " ".to_string());
    let badge_style = match meta.level {
        LogLevel::Error => Style::default()
            .fg(theme.error())
            .add_modifier(Modifier::BOLD),
        LogLevel::Warn => Style::default()
            .fg(theme.warn())
            .add_modifier(Modifier::BOLD),
        LogLevel::Debug => Style::default().fg(theme.muted()),
        LogLevel::Info => Style::default(),
    };
    vec![
        Span::raw(" "),
        Span::styled(badge, badge_style),
        Span::raw(" "),
        Span::styled(meta.timestamp.clone(), Style::default().fg(theme.muted())),
        Span::raw(" "),
        Span::styled(
            format!("{:<width$}", meta.source.tag(), width = SOURCE_TAG_WIDTH),
            Style::default().fg(theme.muted()),
        ),
        Span::raw(" "),
    ]
}

/// A blank prefix the same [`PREFIX_WIDTH`] as [`line_prefix`] — every
/// non-first line of a panic block (and every wrapped continuation row, via
/// [`hard_wrap`]'s `indent`) renders this instead of repeating the chrome, so
/// the whole grouped entry reads as one timestamped unit (workbook §B11).
fn blank_prefix() -> Vec<Span<'static>> {
    vec![Span::raw(" ".repeat(PREFIX_WIDTH))]
}

/// The foreground color for a classified log level, or `None` for
/// [`LogLevel::Info`] (the workbook's "plain `fg`, no tint" default).
fn level_tint(level: LogLevel, theme: &Theme) -> Option<Color> {
    match level {
        LogLevel::Error => Some(theme.error()),
        LogLevel::Warn => Some(theme.warn()),
        LogLevel::Debug => Some(theme.muted()),
        LogLevel::Info => None,
    }
}

/// Parse one raw (possibly ANSI-colored) log line into an owned styled `Line`
/// carrying its badge/timestamp/source prefix chrome ([`line_prefix`]), a
/// level colorize (only when the line carries no ANSI color of its own — the
/// existing ansi-passthrough rule, now sourced from the precomputed
/// [`crate::engine::LineMeta`] rather than re-derived here), panic-block role
/// styling (dim frame / dimmer frame-location — fdemon's dim-vs-highlighted
/// distinction), and the selection-highlight background (absolute-index
/// selection, so it tracks the same lines across incoming output).
fn build_row(session: &SessionView, abs: u64, theme: &Theme) -> Line<'static> {
    let raw = session.log.get(abs).unwrap_or("");
    let meta = session.line_meta(abs);
    let strip = meta.map_or(0, |m| m.source_prefix_strip);
    let display_raw = raw.get(strip..).unwrap_or(raw);

    let text = display_raw
        .into_text()
        .unwrap_or_else(|_| Text::from(display_raw.to_string()));
    // Flatten to a single line (our lines are newline-stripped upstream, but a
    // stray embedded newline is joined rather than dropped).
    let mut msg_spans: Vec<Span<'static>> = Vec::new();
    for (li, line) in text.lines.into_iter().enumerate() {
        if li > 0 {
            msg_spans.push(Span::raw(" "));
        }
        msg_spans.extend(line.spans);
    }
    let has_ansi_color = msg_spans.iter().any(|s| s.style.fg.is_some());

    let mut out_style = Style::default();
    let prefix = if let Some(meta) = meta {
        if !has_ansi_color && let Some(color) = level_tint(meta.level, theme) {
            out_style = out_style.fg(color);
        }
        match meta.role {
            LineRole::BacktraceHeader => {
                msg_spans = vec![Span::styled(
                    "\u{25be} stack backtrace:",
                    Style::default().fg(theme.accent()),
                )];
            }
            LineRole::Frame => {
                out_style = Style::default().fg(theme.muted());
            }
            LineRole::FrameLocation => {
                out_style = Style::default()
                    .fg(theme.muted())
                    .add_modifier(Modifier::DIM);
            }
            LineRole::Normal | LineRole::PanicHeader | LineRole::PanicMessage => {}
        }
        if meta.role.shows_prefix_chrome() {
            line_prefix(meta, theme)
        } else {
            blank_prefix()
        }
    } else {
        blank_prefix()
    };

    let mut spans = prefix;
    spans.extend(msg_spans);
    let mut out = Line::from(spans);
    out.style = out_style;

    if session.selection.is_some_and(|s| s.contains(abs)) {
        out.style = out.style.bg(theme.overlay());
    }
    out
}

/// Hard-wrap a styled line to at most `width` columns per row, splitting spans
/// (and carrying the line-level style onto every wrapped row). Column width is
/// approximated as one column per char — exact for the ASCII-dominant log
/// output the view renders; a wide-char line simply wraps a touch early. Every
/// row after the first indents `indent` columns (workbook §B11: a wrapped
/// continuation lines up under the message column, not column 0) instead of
/// repeating the first row's prefix chrome.
fn hard_wrap(line: &Line<'static>, width: usize, indent: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line.clone()];
    }
    let indent = indent.min(width.saturating_sub(1));
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    let mut first_row = true;
    let flush =
        |rows: &mut Vec<Line<'static>>, cur: &mut Vec<Span<'static>>, style: Style, first: bool| {
            let mut spans = Vec::new();
            if !first && indent > 0 {
                spans.push(Span::raw(" ".repeat(indent)));
            }
            spans.extend(std::mem::take(cur));
            let mut l = Line::from(spans);
            l.style = style;
            rows.push(l);
        };
    let row_budget = |first: bool| {
        if first {
            width
        } else {
            width.saturating_sub(indent).max(1)
        }
    };
    for span in &line.spans {
        let style = span.style;
        let mut buf = String::new();
        for ch in span.content.chars() {
            buf.push(ch);
            col += 1;
            if col >= row_budget(first_row) {
                cur.push(Span::styled(std::mem::take(&mut buf), style));
                flush(&mut rows, &mut cur, line.style, first_row);
                first_row = false;
                col = 0;
            }
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, style));
        }
    }
    if !cur.is_empty() || rows.is_empty() {
        flush(&mut rows, &mut cur, line.style, first_row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{LineSelection, LogSource, Scroll};
    use crate::supervise::SessionId;
    use std::path::PathBuf;

    fn theme() -> Theme {
        Theme::frust_dark_at(crate::ui::theme::ColorDepth::TrueColor)
    }

    /// A session seeded with `n` plain lines, each pushed with a
    /// deterministic synthetic timestamp (`push_line_at`) rather than the
    /// real wall clock — every render-level test below asserts on exact row
    /// text, which must never depend on the moment the test happened to run.
    fn session(n: u64) -> SessionView {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/app"), "desktop");
        for i in 0..n {
            s.push_line_at(format!("line {i}"), format!("12:00:{:02}", i % 60));
        }
        s
    }

    /// A row's full rendered text (prefix chrome + message), concatenating
    /// every span's content.
    fn row_text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A row's message text only — the fixed-width badge/timestamp/source
    /// prefix chrome ([`PREFIX_WIDTH`]) stripped off.
    fn msg_only(l: &Line<'static>) -> String {
        row_text(l).chars().skip(PREFIX_WIDTH).collect()
    }

    #[test]
    fn hard_wrap_splits_at_width_and_preserves_text() {
        let line = Line::from("abcdefghij".to_string());
        let rows = hard_wrap(&line, 4, 0);
        let joined: Vec<String> = rows.iter().map(row_text).collect();
        assert_eq!(joined, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn hard_wrap_carries_line_style_onto_every_row() {
        let mut line = Line::from("abcdef".to_string());
        line.style = Style::default().fg(Color::Red);
        let rows = hard_wrap(&line, 3, 0);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.style.fg == Some(Color::Red)));
    }

    #[test]
    fn hard_wrap_indents_continuation_rows_under_the_message_column() {
        let line = Line::from("abcdefghij".to_string());
        let rows = hard_wrap(&line, 6, 3);
        let joined: Vec<String> = rows.iter().map(row_text).collect();
        // First row: full 6-col budget ("abcdef"). Continuation rows: 3 cols
        // of indent + a (6-3)=3-col message budget each.
        assert_eq!(joined, vec!["abcdef", "   ghi", "   j"]);
    }

    #[test]
    fn follow_window_shows_the_last_height_lines() {
        let s = session(20);
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 5, &theme());
        assert_eq!(rows.len(), 5);
        let texts: Vec<String> = rows.iter().map(|(l, _)| msg_only(l)).collect();
        assert_eq!(
            texts,
            vec!["line 15", "line 16", "line 17", "line 18", "line 19"]
        );
    }

    #[test]
    fn anchored_window_keeps_the_anchor_at_the_bottom() {
        let mut s = session(20);
        s.scroll = Scroll::Anchored(9); // line 9 at the bottom
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 3, &theme());
        let texts: Vec<String> = rows.iter().map(|(l, _)| msg_only(l)).collect();
        assert_eq!(texts, vec!["line 7", "line 8", "line 9"]);
    }

    #[test]
    fn filter_restricts_visible_indices() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        s.push_line_at("hello world".into(), "00:00:00");
        s.push_line_at("ERROR boom".into(), "00:00:01");
        s.push_line_at("hello again".into(), "00:00:02");
        let vis = s.visible_indices(Some("hello"));
        assert_eq!(vis, vec![0, 2]);
    }

    #[test]
    fn level_filter_restricts_visible_indices() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        s.push_line_at("plain".into(), "00:00:00"); // Info
        s.push_line_at("warning: careful".into(), "00:00:01"); // Warn
        s.push_line_at("error: boom".into(), "00:00:02"); // Error
        s.set_level_filter(LevelFilter::WarnPlus);
        assert_eq!(s.visible_indices(None), vec![1, 2]);
        s.set_level_filter(LevelFilter::ErrorOnly);
        assert_eq!(s.visible_indices(None), vec![2]);
    }

    #[test]
    fn selection_applies_a_highlight_background() {
        let mut s = session(5);
        s.selection = Some(LineSelection {
            anchor: 1,
            cursor: 2,
        });
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 5, &theme());
        // rows: line0..line4; lines 1 and 2 carry the overlay bg.
        assert!(rows[0].0.style.bg.is_none());
        assert!(rows[1].0.style.bg.is_some());
        assert!(rows[2].0.style.bg.is_some());
        assert!(rows[3].0.style.bg.is_none());
    }

    // ── Log styling: levels/sources render, panic-block folding ────────────

    #[test]
    fn each_level_gets_its_badge_and_a_no_ansi_message_tint() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        s.push_line_at("error: boom".into(), "00:00:00");
        s.push_line_at("warning: careful".into(), "00:00:01");
        s.push_line_at("plain info".into(), "00:00:02");
        let t = theme();
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 3, &t);
        assert_eq!(rows[0].0.style.fg, Some(t.error()));
        assert!(row_text(&rows[0].0).starts_with(" E "));
        assert_eq!(rows[1].0.style.fg, Some(t.warn()));
        assert!(row_text(&rows[1].0).starts_with(" W "));
        assert_eq!(rows[2].0.style.fg, None); // info: plain, no tint
        assert!(row_text(&rows[2].0).starts_with("   ")); // blank badge column
    }

    #[test]
    fn gradle_source_tag_renders_and_strips_the_raw_marker() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        s.push_line_at("[gradle] BUILD SUCCESSFUL".into(), "00:00:00");
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 1, &theme());
        let text = row_text(&rows[0].0);
        assert!(text.trim_end().ends_with("BUILD SUCCESSFUL"));
        // The tag renders exactly once — the drive pipeline's raw `"[gradle]
        // "` marker was stripped before ANSI-parsing the message, so it
        // never doubles up with our own rendered source-tag chrome.
        assert_eq!(text.matches(LogSource::Gradle.tag()).count(), 1);
    }

    fn panic_lines() -> Vec<&'static str> {
        vec![
            "thread 'main' panicked at src/main.rs:42:9:",
            "called `Option::unwrap()` on a `None` value",
            "stack backtrace:",
            "   0: my_app::state::reduce",
            "             at src/state.rs:88:13",
            "app: recovering",
        ]
    }

    #[test]
    fn a_collapsed_panic_block_renders_as_one_fold_row() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        for (i, l) in panic_lines().into_iter().enumerate() {
            s.push_line_at(l.to_string(), format!("00:00:{i:02}"));
        }
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 10, &theme());
        // header + message + one fold row + the trailing "app: recovering"
        // line — the backtrace header/frame/location lines are absorbed.
        assert_eq!(rows.len(), 4);
        assert!(row_text(&rows[0].0).contains("panicked at"));
        assert!(row_text(&rows[1].0).contains("Option::unwrap"));
        assert_eq!(rows[2].1, Some(0)); // the fold row's click target
        assert!(row_text(&rows[2].0).contains("1 frames"));
        assert!(row_text(&rows[3].0).contains("app: recovering"));
    }

    #[test]
    fn an_expanded_panic_block_renders_every_frame_line() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        for (i, l) in panic_lines().into_iter().enumerate() {
            s.push_line_at(l.to_string(), format!("00:00:{i:02}"));
        }
        s.toggle_fold(0);
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 10, &theme());
        // header + message + backtrace-header + frame + location + trailer.
        assert_eq!(rows.len(), 6);
        assert!(row_text(&rows[2].0).contains("stack backtrace:"));
        assert!(row_text(&rows[3].0).contains("my_app::state::reduce"));
        assert!(row_text(&rows[4].0).contains("at src/state.rs"));
        assert!(rows.iter().all(|(_, fold)| fold.is_none()));
    }
}
