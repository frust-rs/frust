//! The session tab bar and the ANSI-aware log view: tabs grouped
//! by project, per-session follow-tail / scroll / wrap / search-filter /
//! level-filter, level+source+timestamp styling (workbook §B11), Rust
//! panic/backtrace fold rows, and a copy-while-scrolling selection highlight.
//!
//! Layering: every function here renders `&AppState` and only *registers*
//! interaction (tab clicks, the log scroll region, per-row/fold-row/
//! filter-chip clicks) through the [`MouseCtx`] — it never mutates the
//! engine. In particular the per-row click regions carry only the absolute
//! line they draw: what a click *means* (anchor, range end, or nothing at
//! all) is line-selection-mode state `crate::engine::update` owns. The
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
use crate::ui::anim::{
    SPINNER_TICKS_PER_FRAME, flash_alpha, flash_bg, spinner_char, themed_shimmer_spans,
};
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
    let active_session = state.active_session();

    // DevTools mode (workbook §B12) is a per-session *view swap*, not an
    // overlay: the session tab bar stays live above it (switching tabs must
    // never force a tab out of DevTools), and everything below it — the log
    // pane, its status row, the perf panel, the search overlay — is replaced
    // by the DevTools surface. `crate::runner`'s key routing performs the
    // matching namespace swap.
    if let Some(session) = active_session.filter(|s| s.devtools.open) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);
        render_tab_bar(frame, rows[0], state, theme, mouse);
        super::devtools::render(frame, rows[1], state, session, theme, mouse);
        return;
    }

    // The perf sparkline panel
    // renders only once its session has actually seen a `frust-perf` line
    // *and* the tab has toggled it open (`t`) — zero-noise otherwise.
    let show_perf = active_session.is_some_and(|s| s.perf.visible && s.perf.has_data());
    // The transient build/install phase status line (workbook §B10) —
    // reserved only while the active session is actually `Building`/
    // `Installing`, so a streaming/terminal session's log pane loses no rows
    // to a hint it no longer needs.
    // A hot patch in flight borrows the same row for its own indicator.
    let show_phase = active_session.is_some_and(|s| {
        matches!(s.state, SessionState::Building | SessionState::Installing) || s.hot_patching()
    });

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
    if session.hot_patching() {
        spans.extend(hot_patch_spans(session, animation_frame, theme));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
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

/// The runner's tick in milliseconds: an animation frame's length, for the
/// in-flight hot patch's elapsed time.
const TICK_MS: u64 = 50;

/// The in-flight hot patch's phase line (fdemon's shimmering `Reloading`):
/// a warn-coloured spinner, `Hot patching…` under the themed shimmer, then
/// the time since the request in muted seconds.
fn hot_patch_spans(
    session: &SessionView,
    animation_frame: u64,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let glyph = spinner_char(animation_frame / SPINNER_TICKS_PER_FRAME);
    let mut spans = vec![Span::styled(
        format!("{glyph} "),
        Style::default().fg(theme.warn()),
    )];
    spans.extend(themed_shimmer_spans(
        "Hot patching\u{2026}",
        animation_frame,
        theme,
        Modifier::BOLD,
    ));
    let frames = session
        .hot_patch_started
        .map_or(0, |start| animation_frame.wrapping_sub(start));
    let tenths = frames * TICK_MS / 100;
    spans.push(Span::styled(
        format!("  {}.{}s", tenths / 10, tenths % 10),
        Style::default().fg(theme.muted()),
    ));
    spans
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

/// The tab-label suffix marking a watched session (`SessionView::watch`).
const WATCH_GLYPH_SUFFIX: &str = " \u{27f3}"; // ⟳

/// The log status line's segment for a watched session.
const WATCH_STATUS: &str = "\u{27f3} watch"; // ⟳ watch

/// `style` with its background tinted by `session`'s live hot-patch flash
/// (`base` toward `theme.success()`, faded by frame — see
/// [`crate::ui::anim::flash`]); `style` itself, untouched, once the flash has
/// faded or when the session never patched, so idle rendering is unchanged.
fn flash_tint(
    style: Style,
    base: Color,
    session: &SessionView,
    animation_frame: u64,
    theme: &Theme,
) -> Style {
    let alpha = flash_alpha(session.hot_patch_flash, animation_frame);
    if alpha > 0.0 {
        style.bg(flash_bg(base, theme.success(), alpha))
    } else {
        style
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
    // The whole header pulses with the active session's hot-patch flash
    // (fdemon's header reload flash), on top of the patched tab's own tint.
    let header = match state.active_session() {
        Some(session) => flash_tint(
            Style::default().bg(theme.surface()),
            theme.surface(),
            session,
            state.animation_frame,
            theme,
        ),
        None => Style::default().bg(theme.surface()),
    };
    frame.render_widget(Block::default().style(header), area);

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
            let (glyph, glyph_color) = if session.hot_patching() {
                (
                    spinner_char(state.animation_frame / SPINNER_TICKS_PER_FRAME).to_string(),
                    theme.warn(),
                )
            } else {
                status_style(&session.state, state.animation_frame, theme)
            };
            let number = if idx < 9 {
                format!("{} ", idx + 1)
            } else {
                String::new()
            };
            // A watched session ("Watch: restart on save") carries a small
            // `⟳` after its label — hardcoded Unicode like the status glyphs
            // above (no Nerd Font variant in `Icons` for it).
            let watch_mark = if session.watch {
                WATCH_GLYPH_SUFFIX
            } else {
                ""
            };
            let label = format!(" {number}{glyph} {}{watch_mark} ", session.target_label);

            let tab_x = area.x + col;
            let tab_style = if active {
                Style::default()
                    .fg(theme.bg())
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg()).bg(theme.overlay())
            };
            // A just-completed hot patch tints the tab's background toward
            // success and fades back (`crate::ui::anim::flash`); idle tabs
            // keep their style untouched.
            let tab_style = flash_tint(
                tab_style,
                if active {
                    theme.accent()
                } else {
                    theme.overlay()
                },
                session,
                state.animation_frame,
                theme,
            );
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
                    format!(" {}{watch_mark} ", session.target_label),
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
    // Right-click over the log pane opens its context menu (copy/follow/
    // search). This pane-wide region is the empty space *below* the last
    // drawn row — it carries no row, and the per-row regions registered
    // further down (later push, so they win the overlap) carry theirs.
    mouse.context(area, ContextTarget::LogView { row: None });

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
    let lines: Vec<Line<'static>> = rows.iter().map(|r| r.line.clone()).collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);

    // Every drawn row is a click + right-click target carrying the absolute
    // line it draws — re-registered each frame, so a scrolled (or filtered,
    // or folded) viewport always maps a row to the line actually under it.
    // Registered before the fold affordance below, which shares the same
    // cells on a fold row and must win the tie (last pushed wins).
    for (i, row) in rows.iter().enumerate() {
        let rect = Rect::new(area.x, area.y + i as u16, area.width, 1);
        mouse.click(
            rect,
            RegionId::LogRow(row.abs),
            Message::LogRowClicked(row.abs),
        );
        mouse.context(rect, ContextTarget::LogView { row: Some(row.abs) });
    }

    // A collapsed panic block's synthetic `▶ n frames…` row is its own click
    // target (mouse parity for `z` — see `Message::ToggleFold`).
    for (i, row) in rows.iter().enumerate() {
        if let Some(block_start) = row.fold {
            let rect = Rect::new(area.x, area.y + i as u16, area.width, 1);
            mouse.click(
                rect,
                RegionId::LogFoldToggle(block_start),
                Message::ToggleFold(block_start),
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
    if session.watch {
        left.push(Span::styled("  ·  ", Style::default().fg(theme.border())));
        left.push(Span::styled(
            WATCH_STATUS.to_string(),
            flash_tint(
                Style::default().fg(theme.accent()),
                theme.surface(),
                session,
                state.animation_frame,
                theme,
            ),
        ));
    }

    let filter = session.level_filter;
    let hidden = level_filter_hidden_count(session);
    // The level-filter chip's worst-case (`ChipForm::Minimal`) width,
    // reserved out of the optional-hint budget below so `l filter`/`z
    // fold`/`t perf` can never crowd out the one indicator that must never
    // go invisible while a filter is active (see `select_chip_form`). Zero
    // while `filter == LevelFilter::All` — nothing to reserve for.
    let chip_reserved = if filter == LevelFilter::All {
        0
    } else {
        minimal_chip_width(filter, hidden)
    };

    // `l filter`/`z fold`/`t perf` are appended only while there's still
    // room beside the (always-shown) base hint, the `left` segments already
    // built above, and the level-filter chip's reserved worst-case width —
    // the same fit-check spirit the built-artifacts segment and the chip
    // itself use below, so a narrow terminal degrades one hint at a time
    // (mouse/base-key parity never lost) instead of the right-aligned hint
    // silently overwriting the tail of `left`'s text or crowding out the
    // filter-active indicator.
    let used_left: usize = left.iter().map(|s| s.content.chars().count()).sum();
    // The base hint itself must honor `chip_reserved` up front — not only
    // the optional `l filter`/`z fold`/`t perf` additions below — or the
    // *always-rendered* hint paints over the chip's reserved columns at
    // narrower widths, hiding the one indicator that must never go
    // invisible while a filter is active. `select_base_hint` picks the
    // widest degrade level that still leaves room for the chip's worst
    // case, mirroring `select_chip_form`'s own graduated-degrade shape.
    let base_hint_budget = (inner.width as usize).saturating_sub(used_left + chip_reserved + 2);
    let mut right_hint = select_base_hint(base_hint_budget).to_string();
    let push_hint_if_it_fits = |hint: &mut String, extra: &str| {
        if used_left + hint.chars().count() + extra.chars().count() + chip_reserved + 2
            <= inner.width as usize
        {
            hint.push_str(extra);
        }
    };
    // A currently-collapsed panic block outranks `l filter` for the
    // remaining space: its own fold row already says "click or z to
    // expand", but that row can scroll out of the viewport, so the
    // persistent `z fold` keyhint matters more here than `l filter` does —
    // `l filter` is the hint that yields when both can't fit. Once every
    // block on this session is expanded (nothing left to re-fold), `l
    // filter` regains its normal priority.
    let has_collapsed_fold = session.has_collapsed_panic_blocks();
    if has_collapsed_fold {
        push_hint_if_it_fits(&mut right_hint, " · z fold");
    }
    push_hint_if_it_fits(&mut right_hint, " · l filter");
    if !has_collapsed_fold && session.has_panic_blocks() {
        push_hint_if_it_fits(&mut right_hint, " · z fold");
    }
    if session.perf.has_data() {
        push_hint_if_it_fits(&mut right_hint, " · t perf");
    }
    let right_hint = right_hint;

    // Level-filter chip (workbook §B11): the widest form that fits beside
    // the right-aligned keyhint (`select_chip_form`) — the full segmented
    // pill when there's room, degrading through a compact single token and
    // then a minimal marker as the row narrows. `l`/`L` cycles the filter by
    // key regardless of which form (or none, at `LevelFilter::All`) renders.
    let mut chip_segment_clicks: Vec<(u16, u16, LevelFilter)> = Vec::new();
    let mut chip_cycle_click: Option<(u16, u16)> = None;
    let available_for_chip =
        (inner.width as usize).saturating_sub(used_left + right_hint.chars().count() + 2);
    if let Some(form) = select_chip_form(available_for_chip, filter, hidden) {
        let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
        match form {
            ChipForm::Full => {
                let mut chip_spans: Vec<Span<'static>> =
                    vec![Span::styled(CHIP_SEP, Style::default().fg(theme.border()))];
                let mut col = CHIP_SEP.chars().count();
                let mut rects = Vec::new();
                for seg in LEVEL_FILTER_SEGMENTS {
                    let label = format!(" {} ", seg.label());
                    let width = label.chars().count();
                    let style = if seg == filter {
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
                if hidden > 0 {
                    chip_spans.push(Span::styled(
                        format!(" {hidden} hidden by filter"),
                        Style::default().fg(theme.muted()),
                    ));
                }
                left.extend(chip_spans);
                chip_segment_clicks = rects
                    .into_iter()
                    .map(|(x, w, seg)| (x as u16, w as u16, seg))
                    .collect();
            }
            ChipForm::Compact => {
                left.push(Span::styled(CHIP_SEP, Style::default().fg(theme.border())));
                let x = used + CHIP_SEP.chars().count();
                let text = compact_chip_text(filter, hidden);
                let w = text.chars().count();
                left.push(Span::styled(
                    text,
                    Style::default()
                        .fg(theme.bg())
                        .bg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ));
                chip_cycle_click = Some((x as u16, w as u16));
            }
            ChipForm::Minimal => {
                left.push(Span::styled(
                    MINIMAL_CHIP_SEP,
                    Style::default().fg(theme.border()),
                ));
                let x = used + MINIMAL_CHIP_SEP.chars().count();
                let text = minimal_chip_text(filter, hidden);
                let w = text.chars().count();
                left.push(Span::styled(
                    text,
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ));
                chip_cycle_click = Some((x as u16, w as u16));
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

    for (x, w, seg) in chip_segment_clicks {
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
    if let Some((x, w)) = chip_cycle_click {
        mouse.click(
            Rect::new(
                inner.x + x,
                inner.y,
                w.min(inner.right().saturating_sub(inner.x + x)),
                1,
            ),
            RegionId::LevelFilterChip,
            Message::CycleLevelFilter(1),
        );
    }
}

// ── Level-filter chip: graduated fit degrade ────────────────────────────────
//
// Workbook §B11's binding note: the hidden-line count stays visible next to
// the chip so filtering never silently hides lines without a trace. A
// non-`All` filter must therefore always render *some* indicator — never the
// all-or-nothing "show the full pill or nothing at all" a narrow terminal
// used to fall back to.

/// Which form the level-filter chip renders in for the current row width —
/// widest-that-fits, chosen by [`select_chip_form`]. Every form carries the
/// hidden-line count (when nonzero) and a click region that changes the
/// filter; only [`LevelFilter::All`] renders no chip at all, since there is
/// nothing active to indicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChipForm {
    /// The full segmented pill — every [`LEVEL_FILTER_SEGMENTS`] entry, the
    /// active one accent-filled — plus " N hidden by filter". One click
    /// region per segment ([`RegionId::LevelFilterSegment`]), jumping
    /// straight to it.
    Full,
    /// A single clickable token: the active segment's label plus the hidden
    /// count, e.g. `" warn+ · 4 hidden "`. Click cycles the filter
    /// ([`RegionId::LevelFilterChip`], same step as the `l` key).
    Compact,
    /// A minimal marker: the active segment behind a caret, e.g.
    /// `"⏷warn+ ·4"` — the narrowest form and the floor: [`select_chip_form`]
    /// still returns this rather than omit the chip when even this doesn't
    /// cleanly fit, since a crowded row is recoverable and an invisible
    /// active filter is not.
    Minimal,
}

/// The separator preceding [`ChipForm::Full`]/[`ChipForm::Compact`] — matches
/// the `"  ·  "` separator used elsewhere on this row.
const CHIP_SEP: &str = "  \u{00b7}  ";
/// [`ChipForm::Minimal`]'s separator: one column instead of five — the last
/// form before "omit" would even be a choice, so it economizes on its own
/// framing too.
const MINIMAL_CHIP_SEP: &str = " ";

/// [`ChipForm::Compact`]'s exact rendered text (the single source both its
/// width math and its render call use, so the two can never drift).
fn compact_chip_text(filter: LevelFilter, hidden: usize) -> String {
    if hidden > 0 {
        format!(" {} \u{00b7} {hidden} hidden ", filter.label())
    } else {
        format!(" {} ", filter.label())
    }
}

/// [`ChipForm::Minimal`]'s exact rendered text (see [`compact_chip_text`]).
fn minimal_chip_text(filter: LevelFilter, hidden: usize) -> String {
    if hidden > 0 {
        format!("\u{23f7}{} \u{00b7}{hidden}", filter.label())
    } else {
        format!("\u{23f7}{}", filter.label())
    }
}

/// [`ChipForm::Full`]'s total column width (separator + every segment +
/// the optional hidden-count note), for the fit check in
/// [`select_chip_form`].
fn full_chip_width(hidden: usize) -> usize {
    let segs: usize = LEVEL_FILTER_SEGMENTS
        .iter()
        .map(|s| format!(" {} ", s.label()).chars().count())
        .sum();
    let note = if hidden > 0 {
        format!(" {hidden} hidden by filter").chars().count()
    } else {
        0
    };
    CHIP_SEP.chars().count() + segs + note
}

fn compact_chip_width(filter: LevelFilter, hidden: usize) -> usize {
    CHIP_SEP.chars().count() + compact_chip_text(filter, hidden).chars().count()
}

fn minimal_chip_width(filter: LevelFilter, hidden: usize) -> usize {
    MINIMAL_CHIP_SEP.chars().count() + minimal_chip_text(filter, hidden).chars().count()
}

/// The base keyhint's graduated degrade levels, widest first, each a prefix
/// of the previous with one segment dropped — mirrors [`select_chip_form`]'s
/// own shape so the *always-rendered* hint honestly fits instead of painting
/// a fixed string over the level-filter chip's reserved columns. Dropped
/// first: `w wrap` and `f follow` — both states are already shown on this
/// same row via `left`'s own follow/wrap segments, so their keybinding hints
/// are the most dispensable. Kept longest: `/ search` and `x stop` — neither
/// is hinted anywhere else on screen.
const BASE_HINT_LEVELS: [&str; 5] = [
    "x stop · f follow · w wrap · / search",
    "x stop · f follow · / search",
    "x stop · / search",
    "x stop",
    "",
];

/// Pick the widest [`BASE_HINT_LEVELS`] entry that fits `budget` columns —
/// pure so the paint-time fit decision is unit-testable without a terminal
/// (see the tests below). `budget` is the caller's
/// `inner.width - used_left - chip_reserved - 2`, so the level-filter chip's
/// worst-case reserved width is honored regardless of which level wins; the
/// last level is `""`, so this never fails to return something that fits.
fn select_base_hint(budget: usize) -> &'static str {
    BASE_HINT_LEVELS
        .iter()
        .find(|level| level.chars().count() <= budget)
        .copied()
        .unwrap_or("")
}

/// Pick the widest chip form that fits `available` columns — pure so the fit
/// math is unit-testable without a terminal (see the tests below).
/// `filter == LevelFilter::All` needs no chip (`None`, the only case
/// rendering nothing is correct); every other filter always gets *some*
/// form — [`ChipForm::Minimal`] is returned even when it doesn't cleanly
/// fit `available`, rather than falling back to `None` (workbook §B11's
/// binding note: an active filter must never go invisible).
fn select_chip_form(available: usize, filter: LevelFilter, hidden: usize) -> Option<ChipForm> {
    if filter == LevelFilter::All {
        return None;
    }
    Some(if full_chip_width(hidden) <= available {
        ChipForm::Full
    } else if compact_chip_width(filter, hidden) <= available {
        ChipForm::Compact
    } else {
        ChipForm::Minimal
    })
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

/// One drawn row of the log pane: the styled line, the absolute log-line
/// index it belongs to (a wrapped line's continuation rows repeat it, and a
/// collapsed panic block's fold row carries the block's oldest visible body
/// line), and the fold-block id when the row *is* that block's `▶ n frames…`
/// affordance.
struct DisplayRow {
    /// The styled row as it will be painted.
    line: Line<'static>,
    /// The fold block this row stands in for, if it is a fold affordance row.
    fold: Option<u64>,
    /// The absolute log-line index this row draws.
    abs: u64,
}

/// Build the ≤`height` display rows ending at the bottom-anchored entry: walk
/// the visible sequence upward from [`SessionView::bottom_pos`], ANSI-parse +
/// (hard-)wrap each, and keep the last `height` display rows so the bottom
/// entry sits at the bottom of the viewport. Exact wrap bounds by
/// construction (we count the very rows we render — no word-wrap/scroll-offset
/// mismatch).
///
/// `vis` is a [`SessionView::visible_indices`] sequence, which already folds a
/// collapsed panic block's whole body into one entry; this draws that entry as
/// the synthetic `▶ n frames…` row. Each returned [`DisplayRow`] carries the
/// absolute line index it draws (the per-row click target [`render_log`]
/// registers, shared by every wrapped continuation row of the same line) plus
/// that row's fold id, if it is a fold affordance.
fn display_window(
    session: &SessionView,
    vis: &[u64],
    wrap: bool,
    width: u16,
    height: u16,
    theme: &Theme,
) -> Vec<DisplayRow> {
    if vis.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }
    let h = height as usize;
    let w = width as usize;
    let mut rows: VecDeque<DisplayRow> = VecDeque::new();
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
            for line in wrapped.into_iter().rev() {
                rows.push_front(DisplayRow {
                    line,
                    fold: Some(block_start),
                    abs,
                });
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
        for line in wrapped.into_iter().rev() {
            rows.push_front(DisplayRow {
                line,
                fold: None,
                abs,
            });
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

    /// Render the session workspace for `state` into a plain string.
    fn render_main_to_string(state: &AppState) -> String {
        use crate::ui::mouse::MouseRegions;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Position;

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("test terminal");
        let mut regions = MouseRegions::new();
        terminal
            .draw(|frame| {
                let mut ctx = MouseCtx::new(&mut regions);
                let area = frame.area();
                render_main(frame, area, state, &theme(), &mut ctx);
            })
            .expect("draw");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                if let Some(cell) = buf.cell(Position::new(x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn a_watched_session_shows_the_watch_mark_on_its_tab_and_status_line() {
        let mut state = AppState::default();
        state.sessions.push(session(3));
        state.active_session = Some(0);
        let unwatched = render_main_to_string(&state);
        assert!(!unwatched.contains('\u{27f3}'), "{unwatched}");

        state.sessions[0].watch = true;
        let watched = render_main_to_string(&state);
        assert!(
            watched.contains(&format!("desktop{WATCH_GLYPH_SUFFIX}")),
            "the tab carries the mark:\n{watched}"
        );
        assert!(
            watched.contains(WATCH_STATUS),
            "the status line says so:\n{watched}"
        );
    }

    /// Render the session workspace for `state` under `theme` and hand back
    /// the raw cell buffer, so a test can compare cell styles between two
    /// renders.
    fn render_main_to_buffer(state: &AppState, theme: &Theme) -> ratatui::buffer::Buffer {
        use crate::ui::mouse::MouseRegions;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("test terminal");
        let mut regions = MouseRegions::new();
        terminal
            .draw(|frame| {
                let mut ctx = MouseCtx::new(&mut regions);
                let area = frame.area();
                render_main(frame, area, state, theme, &mut ctx);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// The position of the first cell of `needle` in `buf`'s row `y`.
    fn find_in_row(buf: &ratatui::buffer::Buffer, y: u16, needle: &str) -> Option<u16> {
        let row: Vec<String> = (buf.area.left()..buf.area.right())
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        let first = needle.chars().next()?.to_string();
        (0..row.len()).find_map(|i| {
            let hit = row[i] == first
                && needle
                    .chars()
                    .enumerate()
                    .all(|(k, c)| row.get(i + k).is_some_and(|cell| *cell == c.to_string()));
            hit.then_some(buf.area.left() + i as u16)
        })
    }

    /// Two running, watched sessions (tab 0 active, tab 1 inactive).
    fn two_watched_sessions() -> AppState {
        let mut state = AppState::default();
        for id in 0..2 {
            let mut s = SessionView::new(SessionId(id), PathBuf::from("/tmp/app"), "desktop");
            s.target_label = format!("tab{id}");
            s.state = SessionState::Running;
            s.watch = true;
            state.sessions.push(s);
        }
        state.active_session = Some(0);
        state
    }

    fn patched(id: u64) -> Message {
        Message::HotPatchOutcome {
            session: SessionId(id),
            outcome: frust_drive::hotpatch::session::Outcome::Patched {
                ms: 12,
                components: 1,
            },
        }
    }

    #[test]
    fn a_hot_patch_flashes_the_tab_and_watch_segment_then_fades_back_to_idle() {
        use crate::engine::update;
        use crate::ui::anim::FLASH_FRAMES;

        let theme = theme();
        let mut state = two_watched_sessions();
        let idle = render_main_to_buffer(&state, &theme);
        let tab = find_in_row(&idle, 0, "tab0").expect("active tab drawn");
        let status_y = (idle.area.top()..idle.area.bottom())
            .rev()
            .find(|&y| find_in_row(&idle, y, WATCH_STATUS).is_some())
            .expect("watch segment drawn");
        let watch = find_in_row(&idle, status_y, WATCH_STATUS).unwrap();

        update(&mut state, patched(0));
        let flashing = render_main_to_buffer(&state, &theme);
        assert_ne!(flashing[(tab, 0)].bg, idle[(tab, 0)].bg, "tab tinted");
        assert_ne!(
            flashing[(watch, status_y)].bg,
            idle[(watch, status_y)].bg,
            "watch segment tinted"
        );
        assert_eq!(flashing[(tab, 0)].symbol(), idle[(tab, 0)].symbol());
        assert_eq!(flashing[(tab, 0)].fg, idle[(tab, 0)].fg, "text untouched");

        for _ in 0..FLASH_FRAMES {
            update(&mut state, Message::Tick);
        }
        let faded = render_main_to_buffer(&state, &theme);
        assert_eq!(faded[(tab, 0)].bg, idle[(tab, 0)].bg, "tab back to idle");
        assert_eq!(
            faded[(watch, status_y)].bg,
            idle[(watch, status_y)].bg,
            "watch segment back to idle"
        );
    }

    /// Row `y` of `buf` as one string.
    fn buffer_row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn a_pending_hot_patch_spins_the_tab_and_shows_a_timed_phase_line() {
        let theme = theme();
        let mut state = two_watched_sessions();
        let idle = render_main_to_buffer(&state, &theme);
        assert!(!buffer_row(&idle, 1).contains("Hot patching"));

        state.animation_frame = 100;
        state.sessions[0].begin_hot_patch(100);
        state.animation_frame = 123;
        let busy = render_main_to_buffer(&state, &theme);
        let tabs = buffer_row(&busy, 0);
        let spinner = spinner_char(123 / SPINNER_TICKS_PER_FRAME);
        assert!(
            tabs.contains(&format!("{spinner} tab0")),
            "the tab glyph spins: {tabs}"
        );
        let phase = buffer_row(&busy, 1);
        assert!(phase.contains("Hot patching\u{2026}"), "{phase}");
        assert!(phase.contains("1.1s"), "23 frames of 50 ms: {phase}");

        state.sessions[0].end_hot_patch();
        let done = render_main_to_buffer(&state, &theme);
        assert!(!buffer_row(&done, 1).contains("Hot patching"));
        assert!(
            buffer_row(&done, 0).contains("\u{25b6} tab0"),
            "back to running"
        );
    }

    #[test]
    fn a_completed_patch_pulses_the_whole_header_then_fades() {
        use crate::engine::update;
        use crate::ui::anim::FLASH_FRAMES;

        let theme = theme();
        let mut state = two_watched_sessions();
        let idle = render_main_to_buffer(&state, &theme);
        // A header cell past every tab: the bare tab-bar background.
        let edge = idle.area.right() - 1;

        update(&mut state, patched(0));
        let pulsing = render_main_to_buffer(&state, &theme);
        assert_ne!(pulsing[(edge, 0)].bg, idle[(edge, 0)].bg, "header tinted");

        for _ in 0..FLASH_FRAMES {
            update(&mut state, Message::Tick);
        }
        let faded = render_main_to_buffer(&state, &theme);
        assert_eq!(faded[(edge, 0)].bg, idle[(edge, 0)].bg, "header idle again");
    }

    #[test]
    fn an_inactive_tab_flashes_alone_and_a_non_rgb_theme_does_not_tint() {
        use crate::engine::update;

        let theme = theme();
        let mut state = two_watched_sessions();
        let idle = render_main_to_buffer(&state, &theme);
        let tab0 = find_in_row(&idle, 0, "tab0").expect("active tab drawn");
        let tab1 = find_in_row(&idle, 0, "tab1").expect("inactive tab drawn");

        update(&mut state, patched(1));
        let flashing = render_main_to_buffer(&state, &theme);
        assert_ne!(flashing[(tab1, 0)].bg, idle[(tab1, 0)].bg, "patched tab");
        assert_eq!(flashing[(tab0, 0)].bg, idle[(tab0, 0)].bg, "other tab");

        let ansi = Theme::frust_dark_at(crate::ui::theme::ColorDepth::Ansi16);
        let mut idle_state = two_watched_sessions();
        idle_state.animation_frame = state.animation_frame;
        assert_eq!(
            render_main_to_buffer(&state, &ansi),
            render_main_to_buffer(&idle_state, &ansi),
            "a 16-colour palette degrades to no tint"
        );
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
        let texts: Vec<String> = rows.iter().map(|r| msg_only(&r.line)).collect();
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
        let texts: Vec<String> = rows.iter().map(|r| msg_only(&r.line)).collect();
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
        assert!(rows[0].line.style.bg.is_none());
        assert!(rows[1].line.style.bg.is_some());
        assert!(rows[2].line.style.bg.is_some());
        assert!(rows[3].line.style.bg.is_none());
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
        assert_eq!(rows[0].line.style.fg, Some(t.error()));
        assert!(row_text(&rows[0].line).starts_with(" E "));
        assert_eq!(rows[1].line.style.fg, Some(t.warn()));
        assert!(row_text(&rows[1].line).starts_with(" W "));
        assert_eq!(rows[2].line.style.fg, None); // info: plain, no tint
        assert!(row_text(&rows[2].line).starts_with("   ")); // blank badge column
    }

    #[test]
    fn gradle_source_tag_renders_and_strips_the_raw_marker() {
        let mut s = SessionView::new(SessionId(0), PathBuf::from("/tmp/a"), "desktop");
        s.push_line_at("[gradle] BUILD SUCCESSFUL".into(), "00:00:00");
        let vis = s.visible_indices(None);
        let rows = display_window(&s, &vis, false, 60, 1, &theme());
        let text = row_text(&rows[0].line);
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
        assert!(row_text(&rows[0].line).contains("panicked at"));
        assert!(row_text(&rows[1].line).contains("Option::unwrap"));
        assert_eq!(rows[2].fold, Some(0)); // the fold row's click target
        assert!(row_text(&rows[2].line).contains("1 frames"));
        assert!(row_text(&rows[3].line).contains("app: recovering"));
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
        assert!(row_text(&rows[2].line).contains("stack backtrace:"));
        assert!(row_text(&rows[3].line).contains("my_app::state::reduce"));
        assert!(row_text(&rows[4].line).contains("at src/state.rs"));
        assert!(rows.iter().all(|r| r.fold.is_none()));
    }

    // ── Base keyhint degrade: pure fit math ─────────────────────────────────

    #[test]
    fn select_base_hint_picks_the_widest_level_that_fits() {
        assert_eq!(select_base_hint(37), BASE_HINT_LEVELS[0]);
        assert_eq!(select_base_hint(36), BASE_HINT_LEVELS[1]);
        assert_eq!(select_base_hint(28), BASE_HINT_LEVELS[1]);
        assert_eq!(select_base_hint(27), BASE_HINT_LEVELS[2]);
        assert_eq!(select_base_hint(17), BASE_HINT_LEVELS[2]);
        assert_eq!(select_base_hint(16), BASE_HINT_LEVELS[3]);
        assert_eq!(select_base_hint(6), BASE_HINT_LEVELS[3]);
        assert_eq!(select_base_hint(5), BASE_HINT_LEVELS[4]);
        assert_eq!(select_base_hint(0), "");
        // Never panics/underflows at a huge budget either.
        assert_eq!(select_base_hint(1000), BASE_HINT_LEVELS[0]);
    }

    /// The terminal-width band where the sidebar is still inline (≥
    /// [`crate::ui::layout::NARROW_WIDTH`], 80 cols) but the log-status row
    /// is narrow enough that a fixed-width base hint used to run into the
    /// level-filter chip's reserved columns: terminal width 80..=93 with
    /// `SIDEBAR_DEFAULT_WIDTH = 26` and the log-status row's `used_left = 21`
    /// / `chip_reserved = 10` (a non-`All` filter with a single-digit hidden
    /// count — the `log_styling_level_filter_chip_survives_narrow_paint_80x30`
    /// snapshot's exact fixture). At every width in this band the resolved
    /// budget must select something *narrower* than the full 37-char level —
    /// painting the full string here is exactly the defect that let it run
    /// into the chip's reserved columns (the fix under test), so
    /// `select_base_hint` degrading away from `BASE_HINT_LEVELS[0]` across
    /// this whole band is the fit-decision proof, independent of the
    /// snapshot's exact pixels.
    #[test]
    fn select_base_hint_degrades_across_the_narrow_sidebar_band() {
        let used_left = 21;
        let chip_reserved = 10;
        let sidebar_and_divider = 26;
        for terminal_width in 80..=93u16 {
            let inner_width = (terminal_width - sidebar_and_divider) as usize;
            let budget = inner_width.saturating_sub(used_left + chip_reserved + 2);
            let hint = select_base_hint(budget);
            assert_ne!(
                hint, BASE_HINT_LEVELS[0],
                "terminal_width={terminal_width} inner_width={inner_width} budget={budget} \
                 must not select the full hint — it would run into the chip's reserved columns"
            );
            assert!(
                hint.chars().count() <= budget,
                "terminal_width={terminal_width}: {hint:?} ({} cols) exceeds budget {budget}",
                hint.chars().count()
            );
        }
    }

    // ── Level-filter chip degrade: pure fit math ────────────────────────────

    #[test]
    fn select_chip_form_omits_only_at_the_all_filter() {
        // Even zero available width doesn't matter — `All` has nothing
        // active to indicate, so no chip is the *correct* choice, not a
        // degrade.
        assert_eq!(select_chip_form(0, LevelFilter::All, 0), None);
        assert_eq!(select_chip_form(1000, LevelFilter::All, 4), None);
    }

    #[test]
    fn select_chip_form_picks_the_widest_form_that_fits() {
        let filter = LevelFilter::WarnPlus;
        let hidden = 4;
        let full = full_chip_width(hidden);
        let compact = compact_chip_width(filter, hidden);
        let minimal = minimal_chip_width(filter, hidden);
        // Sanity: the three forms are strictly ordered widest-to-narrowest
        // for this fixture — the degrade only makes sense if they are.
        assert!(full > compact && compact > minimal);

        assert_eq!(select_chip_form(full, filter, hidden), Some(ChipForm::Full));
        assert_eq!(
            select_chip_form(full - 1, filter, hidden),
            Some(ChipForm::Compact)
        );
        assert_eq!(
            select_chip_form(compact, filter, hidden),
            Some(ChipForm::Compact)
        );
        assert_eq!(
            select_chip_form(compact - 1, filter, hidden),
            Some(ChipForm::Minimal)
        );
        assert_eq!(
            select_chip_form(minimal, filter, hidden),
            Some(ChipForm::Minimal)
        );
    }

    #[test]
    fn select_chip_form_never_vanishes_for_an_active_filter_even_when_too_narrow() {
        // Zero columns available: even `ChipForm::Minimal` doesn't "fit"
        // cleanly, but the fix is that it renders anyway rather than
        // silently omitting the one indicator an active filter must show.
        assert_eq!(
            select_chip_form(0, LevelFilter::ErrorOnly, 12),
            Some(ChipForm::Minimal)
        );
    }

    #[test]
    fn chip_labels_always_carry_the_hidden_count_when_nonzero() {
        // Workbook §B11's binding note: the hidden-line count stays visible
        // next to the chip in every form, not only the full pill.
        assert!(compact_chip_text(LevelFilter::WarnPlus, 4).contains('4'));
        assert!(minimal_chip_text(LevelFilter::WarnPlus, 4).contains('4'));
        // ...and is omitted (not "0 hidden") when there's nothing hidden.
        assert!(!compact_chip_text(LevelFilter::WarnPlus, 0).contains("hidden"));
        assert!(!minimal_chip_text(LevelFilter::WarnPlus, 0).contains(char::is_numeric));
    }
}
