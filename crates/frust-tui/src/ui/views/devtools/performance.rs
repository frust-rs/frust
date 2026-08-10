//! The Performance tab body (workbook §B12): the frame-time chart over the
//! current window, the FPS/percentile/jank chip row, the selected frame's
//! per-phase breakdown bar, and the `service`/`log fallback` source badge.
//!
//! All the math (which source is live, the window, percentiles, jank) is
//! [`crate::engine`]'s — this module only lays the numbers out. `←`/`→`
//! scrub and `Tab` (chart↔breakdown focus) are keyboard-only; the one mouse
//! affordance is a chart-column click, registered here and routed through
//! the same [`Message::DevtoolsPerfSelectFrame`] the key does.
//!
//! **Not implemented as drawn (§B12 honesty note):** the mockup's breakdown
//! bar embeds each phase's label/ms/percent *inside* its own segment
//! (`═rebuild 8.2ms 49%═╦…`), which only fits at a handful of exact widths.
//! This renders a proportional colored bar on one row and a separate text
//! legend below it instead — same numbers, robust at any terminal width.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline, SparklineBar};

use crate::engine::{
    Message, PerfFocus, PerfFrame, PerfPhases, PerfStats, RegionId, SessionView, perf_stats,
    perf_window,
};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Chart row height (bars only — the axis/footer line is separate).
const CHART_HEIGHT: u16 = 3;

/// Render the Performance tab body into `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // The source badge itself is chrome the tab strip owns
    // (`ui::views::devtools::render_tab_strip`) — this only needs the window.
    let (_, window) = perf_window(
        &session.devtools.conn,
        &session.devtools.frames,
        &session.perf,
    );
    let focus = session.devtools.performance.focus;

    if window.is_empty() {
        // Reachable from `Service` (freshly connected, first batch hasn't
        // landed) or `NoData` (nothing at all yet) — `LogFallback` is never
        // chosen with an empty window (`select_perf_source` only picks it
        // once the log panel has samples), so one message covers both.
        let msg = "waiting for the first frame…";
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("  {msg}"),
                Style::default().fg(theme.muted()),
            )),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }

    // The pin is a frame identity, resolved to a position against *this*
    // frame's window (see `PerformanceTab::resolve`): an unresolvable pin
    // (its frame aged out between the engine's own retention pass and this
    // draw) reads as live rather than as some other frame.
    let resolved = session.devtools.performance.resolve(&window);
    let is_live = resolved.is_none();
    let selected_idx = resolved.unwrap_or(window.len() - 1);
    let selected = window[selected_idx];
    let stats = perf_stats(&window);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // chip row
            Constraint::Length(CHART_HEIGHT), // chart
            Constraint::Length(1),            // chart axis/footer
            Constraint::Length(1),            // "Frame #N · total …"
            Constraint::Length(1),            // breakdown bar
            Constraint::Length(1),            // breakdown legend
            Constraint::Min(0),
        ])
        .split(area);

    render_chips(frame, rows[0], stats, focus, theme);
    render_chart(frame, rows[1], &window, selected_idx, stats, theme, mouse);
    render_axis(frame, rows[2], window.len(), selected.n, is_live, theme);
    render_selected_header(frame, rows[3], selected, focus, theme);
    render_breakdown(frame, rows[4], rows[5], selected, theme);
}

/// The `Frame Timing  FPS 58.2 · p50 …ms · p95 …ms · p99 …ms · jank N (P%)`
/// row. `stats` is `None` when every frame in the window was skipped (or the
/// window is somehow empty) — the numbers render as `n/a` rather than `0`,
/// which would misleadingly read as "measured, and it's zero".
fn render_chips(
    frame: &mut Frame,
    area: Rect,
    stats: Option<PerfStats>,
    focus: PerfFocus,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }
    let label_style = if focus == PerfFocus::Chart {
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![Span::styled(" Frame Timing  ", label_style)];
    match stats {
        Some(s) => {
            spans.push(Span::styled(
                format!("FPS {:.1}", s.fps),
                Style::default().fg(theme.success()),
            ));
            spans.push(sep(theme));
            spans.push(Span::styled(
                format!("p50 {}", fmt_ms(s.p50_us)),
                Style::default().fg(theme.muted()),
            ));
            spans.push(sep(theme));
            spans.push(Span::styled(
                format!("p95 {}", fmt_ms(s.p95_us)),
                Style::default().fg(theme.muted()),
            ));
            spans.push(sep(theme));
            spans.push(Span::styled(
                format!("p99 {}", fmt_ms(s.p99_us)),
                Style::default().fg(theme.muted()),
            ));
            spans.push(sep(theme));
            let jank_color = if s.jank_count > 0 {
                theme.warn()
            } else {
                theme.muted()
            };
            spans.push(Span::styled(
                format!("jank {} ({:.1}%)", s.jank_count, s.jank_pct),
                Style::default().fg(jank_color),
            ));
        }
        None => spans.push(Span::styled(
            "n/a — every frame in view was skipped",
            Style::default().fg(theme.muted()),
        )),
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn sep(theme: &Theme) -> Span<'static> {
    Span::styled(" · ", Style::default().fg(theme.muted()))
}

fn fmt_ms(us: u64) -> String {
    format!("{:.1}ms", us as f64 / 1000.0)
}

/// Jank-tier bar colors, chosen for visual separation rather than matching
/// any spec — a normal frame draws `success`, a jank frame (over the chip's
/// own threshold) draws `warn`, and a severe spike (2.5x the threshold)
/// draws `error`. A skipped frame ignores all of this and always draws
/// `muted` at the shortest possible height (§B12: "a dim column rather than
/// a height").
const SEVERE_JANK_MULTIPLIER: u64 = 2;

fn bar_style(f: PerfFrame, stats: Option<PerfStats>, theme: &Theme) -> Style {
    if f.skipped {
        return Style::default().fg(theme.muted());
    }
    let Some(stats) = stats else {
        return Style::default().fg(theme.success());
    };
    if f.total_us
        > stats
            .jank_threshold_us
            .saturating_mul(SEVERE_JANK_MULTIPLIER)
    {
        Style::default().fg(theme.error())
    } else if f.total_us > stats.jank_threshold_us {
        Style::default().fg(theme.warn())
    } else {
        Style::default().fg(theme.success())
    }
}

/// The sparkline itself, plus one click region per visible column
/// (`Message::DevtoolsPerfSelectFrame`) — keyboard parity for `←`/`→`.
fn render_chart(
    frame: &mut Frame,
    area: Rect,
    window: &[PerfFrame],
    selected_idx: usize,
    stats: Option<PerfStats>,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let visible_len = (area.width as usize).min(window.len());
    let offset = window.len() - visible_len;
    let visible = &window[offset..];
    let max_us = window.iter().map(|f| f.total_us).max().unwrap_or(1).max(1);
    // A skipped frame always draws the shortest possible glyph regardless of
    // `max_us`'s scale — a bare `value: 1` would round down to nothing once
    // `max_us` is large (the eighth-block granularity is `area.height * 8`
    // steps), which would make a skip indistinguishable from a gap instead
    // of §B12's "dim column".
    let skip_value = (max_us / (u64::from(CHART_HEIGHT) * 8)).max(1);

    let bars: Vec<SparklineBar> = visible
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let abs_index = offset + i;
            let value = if f.skipped {
                skip_value
            } else {
                f.total_us.max(1)
            };
            let mut style = bar_style(*f, stats, theme);
            if abs_index == selected_idx {
                style = style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
            }
            SparklineBar::from(value).style(Some(style))
        })
        .collect();

    frame.render_widget(Sparkline::default().data(bars).max(max_us), area);

    // Each column's region carries the frame *it drew* (`PerfFrame::n`), not
    // its position: the click is consumed a frame or more later, by which
    // time the window may have slid, and a positional payload would then
    // select whichever frame had moved into that column.
    for (i, f) in visible.iter().enumerate() {
        let x = area.x + i as u16;
        mouse.click(
            Rect::new(x, area.y, 1, area.height),
            RegionId::DevtoolsPerfColumn(f.n),
            Message::DevtoolsPerfSelectFrame(f.n),
        );
    }
}

/// The row under the chart: `0` on the left, the window size and
/// live/selected indicator on the right.
fn render_axis(
    frame: &mut Frame,
    area: Rect,
    window_len: usize,
    selected_frame_n: u64,
    is_live: bool,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(" 0└{}", "─".repeat(area.width.saturating_sub(2) as usize)),
            Style::default().fg(theme.muted()),
        )),
        area,
    );
    let right = if is_live {
        format!("{window_len}-frame ring · live (frame #{selected_frame_n}) ")
    } else {
        format!("{window_len}-frame ring · frame #{selected_frame_n} selected ")
    };
    frame.render_widget(
        Paragraph::new(Line::styled(right, Style::default().fg(theme.muted())))
            .alignment(Alignment::Right),
        area,
    );
}

fn render_selected_header(
    frame: &mut Frame,
    area: Rect,
    selected: PerfFrame,
    focus: PerfFocus,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }
    let label_style = if focus == PerfFocus::Breakdown {
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    };
    let line = Line::from(vec![
        Span::styled(format!(" Frame #{}", selected.n), label_style),
        Span::styled(
            format!(" · total {}", fmt_ms(selected.total_us)),
            Style::default().fg(theme.muted()),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Phase colors, in wire order (rebuild/layout/paint/encode/acquire/submit)
/// — a fixed, arbitrary-but-consistent mapping over the existing theme
/// tokens (no per-purpose token exists for "phase N of 6").
fn phase_color(index: usize, theme: &Theme) -> Color {
    match index {
        0 => theme.accent(),
        1 => theme.primary(),
        2 => theme.success(),
        3 => theme.warn(),
        4 => theme.fg(),
        _ => theme.muted(),
    }
}

/// The breakdown bar (`bar_area`) + its text legend (`legend_area`) for
/// `selected`. When the source has no phase split (log fallback — see
/// [`PerfFrame::phases`]'s doc), both rows fold into one explanatory note.
fn render_breakdown(
    frame: &mut Frame,
    bar_area: Rect,
    legend_area: Rect,
    selected: PerfFrame,
    theme: &Theme,
) {
    let Some(phases) = selected.phases else {
        if bar_area.height > 0 {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    " breakdown unavailable — log fallback carries frame totals only",
                    Style::default().fg(theme.muted()),
                )),
                bar_area,
            );
        }
        return;
    };
    render_breakdown_bar(frame, bar_area, &phases, theme);
    render_breakdown_legend(frame, legend_area, &phases, theme);
}

fn render_breakdown_bar(frame: &mut Frame, area: Rect, phases: &PerfPhases, theme: &Theme) {
    if area.height == 0 || area.width < 2 {
        return;
    }
    let inner_width = area.width.saturating_sub(2) as usize; // 1-col margin each side
    if inner_width == 0 {
        return;
    }
    let segments = phases.segments();
    let total = phases.phase_total_us().max(1) as f64;
    let mut spans = vec![Span::raw(" ")];
    let mut used = 0usize;
    for (i, (_, us, _)) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let width = if is_last {
            inner_width.saturating_sub(used)
        } else {
            ((*us as f64 / total) * inner_width as f64).round() as usize
        };
        used += width;
        if width == 0 {
            continue;
        }
        spans.push(Span::styled(
            "█".repeat(width),
            Style::default().fg(phase_color(i, theme)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_breakdown_legend(frame: &mut Frame, area: Rect, phases: &PerfPhases, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let segments = phases.segments();
    let mut spans = vec![Span::raw(" ")];
    for (i, (label, us, pct)) in segments.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("{label} {} {pct:.0}%", fmt_ms(*us)),
            Style::default().fg(phase_color(i, theme)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ms_renders_one_decimal() {
        assert_eq!(fmt_ms(16_800), "16.8ms");
        assert_eq!(fmt_ms(0), "0.0ms");
    }

    #[test]
    fn breakdown_segments_fill_the_bar_exactly_once() {
        let phases = PerfPhases {
            rebuild_us: 8_200,
            layout_us: 3_000,
            paint_us: 2_100,
            encode_us: 1_400,
            acquire_us: 600,
            submit_us: 1_000,
        };
        // The last segment absorbs rounding, so summed widths always equal
        // `inner_width` exactly rather than drifting by a cell.
        let total = phases.phase_total_us() as f64;
        let inner_width = 40usize;
        let mut used = 0usize;
        for (i, (_, us, _)) in phases.segments().iter().enumerate() {
            let is_last = i == 5;
            let width = if is_last {
                inner_width - used
            } else {
                ((*us as f64 / total) * inner_width as f64).round() as usize
            };
            used += width;
        }
        assert_eq!(used, inner_width);
    }
}
