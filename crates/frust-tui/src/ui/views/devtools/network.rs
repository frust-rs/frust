//! The Network tab body (workbook §B12): rx/tx rate sparklines, totals since
//! sampling started, the Android-vs-desktop source/sampling chrome shared
//! with [`super::system`], and the honesty note — process-level counters
//! only ("device-wide"/"namespace-wide" scope; request-level logging needs
//! app-side instrumentation) — rendered directly in the tab body per §B12's
//! ask that this caveat live where the numbers do, not in a doc a user has
//! to go find.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Sparkline, SparklineBar};

use crate::engine::{MetricsState, NetRatePoint, SessionView, network_honesty_note};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Each direction's sparkline row height.
const CHART_HEIGHT: u16 = 2;

/// Render the Network tab body into `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    theme: &Theme,
    _mouse: &mut MouseCtx,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let metrics = &session.devtools.metrics;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // source + sampling status
            Constraint::Length(1),            // RX label
            Constraint::Length(CHART_HEIGHT), // RX sparkline
            Constraint::Length(1),            // TX label
            Constraint::Length(CHART_HEIGHT), // TX sparkline
            Constraint::Length(1),            // totals
            Constraint::Min(2),               // honesty note
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(super::metrics_status_line(metrics, theme)),
        rows[0],
    );
    render_rate_label(
        frame,
        rows[1],
        "RX",
        metrics.net_rates.back().map(|p| p.rx_bps),
        theme,
    );
    render_rate_chart(frame, rows[2], metrics, true, theme);
    render_rate_label(
        frame,
        rows[3],
        "TX",
        metrics.net_rates.back().map(|p| p.tx_bps),
        theme,
    );
    render_rate_chart(frame, rows[4], metrics, false, theme);
    render_totals_row(frame, rows[5], metrics, theme);
    render_honesty_note(frame, rows[6], metrics, theme);
}

fn render_rate_label(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    latest_bps: Option<f64>,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }
    let text = match latest_bps {
        Some(bps) => format!(" {label}     {}", fmt_rate(bps)),
        None => format!(" {label}     n/a — no samples yet"),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(theme.fg()))),
        area,
    );
}

fn render_rate_chart(
    frame: &mut Frame,
    area: Rect,
    metrics: &MetricsState,
    rx: bool,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    if metrics.net_rates.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  waiting for rate samples…",
                Style::default().fg(theme.muted()),
            )),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }
    let visible_len = (area.width as usize).min(metrics.net_rates.len());
    let offset = metrics.net_rates.len() - visible_len;
    let pick = |p: &NetRatePoint| if rx { p.rx_bps } else { p.tx_bps };
    let max = metrics
        .net_rates
        .iter()
        .map(pick)
        .fold(1.0_f64, f64::max)
        .max(1.0);
    let color: Color = if rx { theme.success() } else { theme.accent() };
    let bars: Vec<SparklineBar> = metrics
        .net_rates
        .iter()
        .skip(offset)
        .map(|p| {
            let value = (pick(p).max(0.0).round() as u64).max(1);
            SparklineBar::from(value).style(Some(Style::default().fg(color)))
        })
        .collect();
    frame.render_widget(
        Sparkline::default().data(bars).max(max.round() as u64),
        area,
    );
}

fn render_totals_row(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let text = if metrics.has_net_data() {
        format!(
            " totals since start   rx {}   tx {}",
            fmt_bytes(metrics.net_totals.rx_bytes),
            fmt_bytes(metrics.net_totals.tx_bytes)
        )
    } else {
        " totals since start   n/a — no samples yet".to_string()
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(theme.fg()))),
        area,
    );
}

/// The honesty note, with a leading `⚠` (§B12's mockup marks it a warning,
/// not a neutral caption) — the first wrapped row carries the glyph, every
/// continuation row indents to align under the text rather than repeating
/// it.
fn render_honesty_note(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let note = network_honesty_note(&metrics.identity);
    // `"⚠ "` prefix width reserved on every row so the wrapped text lines up
    // under the glyph rather than the continuation rows starting one column
    // further right.
    let width = area.width.saturating_sub(4) as usize;
    let lines: Vec<Line> = super::wrap_words(note, width.max(1))
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let prefix = if i == 0 { "⚠ " } else { "  " };
            Line::styled(format!(" {prefix}{row}"), Style::default().fg(theme.warn()))
        })
        .collect();
    let height = (lines.len() as u16).min(area.height);
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, area.y, area.width, height),
    );
}

/// Human-readable byte count — see `system.rs`'s identical copy for why each
/// view file owns its own small formatters.
fn fmt_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn fmt_rate(bps: f64) -> String {
    format!("{}/s", fmt_bytes(bps.max(0.0).round() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_rate_appends_per_second() {
        assert_eq!(fmt_rate(0.0), "0 B/s");
        assert_eq!(fmt_rate(2048.0), "2.0 KB/s");
    }
}
