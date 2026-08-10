//! The System tab body (workbook §B12): a CPU% sparkline, RSS (peak +
//! latest), thermal (one row per zone, honestly `n/a` when none have
//! reported), uptime, and the Android-vs-desktop source/sampling-state
//! chrome shared with [`super::network`].
//!
//! All the math is [`crate::engine::MetricsState`]'s — this module only lays
//! the numbers out, mirroring `performance.rs`'s split. There is no mouse
//! affordance beyond the tab strip's own pills (§B12 doesn't ask for one
//! here).
//!
//! **Not implemented as drawn (§B12 honesty note, `performance.rs`'s own
//! breakdown-bar precedent):** the mockup inlines the CPU sparkline right
//! after its percent on one row and puts the `source: …` label at the
//! *strip's* right edge; this renders the percent/label and the sparkline
//! on their own rows (a `ratatui::widgets::Sparkline` needs its own `Rect`,
//! the same reason `performance.rs`'s chip row and chart row are separate)
//! and folds the source label into this tab's own status row instead of
//! special-casing the shared strip per active tab. Same numbers, same
//! honesty, a layout that holds at any terminal width.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Sparkline, SparklineBar};

use crate::engine::{MetricsState, SessionView};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// CPU sparkline row height — matches `performance.rs`'s `CHART_HEIGHT`.
const CHART_HEIGHT: u16 = 3;

/// CPU-percent sparkline resolution: one Sparkline unit per this many
/// tenths of a percent, so a 0–100%+ range still resolves distinctly across
/// [`CHART_HEIGHT`]'s eighth-block granularity.
const CPU_SCALE: f32 = 10.0;

/// Render the System tab body into `area`.
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
            Constraint::Length(1),            // CPU label
            Constraint::Length(CHART_HEIGHT), // CPU sparkline
            Constraint::Length(1),            // RSS
            Constraint::Length(1),            // uptime
            Constraint::Min(1),               // thermal rows
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(super::metrics_status_line(metrics, theme)),
        rows[0],
    );
    render_cpu_label(frame, rows[1], metrics, theme);
    render_cpu_chart(frame, rows[2], metrics, theme);
    render_rss_row(frame, rows[3], metrics, theme);
    render_uptime_row(frame, rows[4], metrics, theme);
    render_thermal(frame, rows[5], metrics, theme);
}

fn render_cpu_label(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let text = match metrics.cpu.back() {
        Some(latest) => {
            let avg: f32 =
                metrics.cpu.iter().map(|p| p.percent).sum::<f32>() / metrics.cpu.len() as f32;
            format!(
                " CPU    {:.1}%   avg {:.1}% over {} samples",
                latest.percent,
                avg,
                metrics.cpu.len()
            )
        }
        None => " CPU    n/a — no samples yet".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(theme.fg()))),
        area,
    );
}

fn render_cpu_chart(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    if metrics.cpu.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  waiting for CPU samples…",
                Style::default().fg(theme.muted()),
            )),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }
    let visible_len = (area.width as usize).min(metrics.cpu.len());
    let offset = metrics.cpu.len() - visible_len;
    let visible = metrics.cpu.iter().skip(offset);
    let max_scaled = metrics
        .cpu
        .iter()
        .map(|p| (p.percent.max(0.0) * CPU_SCALE).round() as u64)
        .max()
        .unwrap_or(1)
        .max(1);
    let bars: Vec<SparklineBar> = visible
        .map(|p| {
            let value = ((p.percent.max(0.0) * CPU_SCALE).round() as u64).max(1);
            SparklineBar::from(value).style(Some(Style::default().fg(theme.accent())))
        })
        .collect();
    frame.render_widget(Sparkline::default().data(bars).max(max_scaled), area);
}

fn render_rss_row(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let text = match metrics.rss.back() {
        Some(latest) => {
            let peak = metrics
                .rss
                .iter()
                .map(|p| p.rss_bytes)
                .max()
                .unwrap_or(latest.rss_bytes);
            format!(
                " RSS    {}   peak {}",
                fmt_bytes(latest.rss_bytes),
                fmt_bytes(peak)
            )
        }
        None => " RSS    n/a — no samples yet".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(theme.fg()))),
        area,
    );
}

fn render_uptime_row(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let text = match metrics.latest_at_ms {
        Some(ms) => format!(" uptime {}", fmt_duration_ms(ms)),
        None => " uptime n/a".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::default().fg(theme.muted()))),
        area,
    );
}

fn render_thermal(frame: &mut Frame, area: Rect, metrics: &MetricsState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    if metrics.thermal.is_empty() {
        lines.push(Line::styled(
            " Thermal   n/a — no thermal zones reported",
            Style::default().fg(theme.muted()),
        ));
    } else {
        lines.push(Line::styled(" Thermal", Style::default().fg(theme.fg())));
        for (zone, point) in &metrics.thermal {
            lines.push(Line::styled(
                format!("   {zone}   {:.1}°C", point.millideg_c as f64 / 1000.0),
                Style::default().fg(theme.fg()),
            ));
        }
    }
    let height = (lines.len() as u16).min(area.height);
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, area.y, area.width, height),
    );
}

/// Human-readable byte count (binary units — `KB`/`MB`/`GB` at 1024
/// steps) — shared in shape with `network.rs`'s own copy (each view file
/// owns its small formatters, matching `performance.rs`'s `fmt_ms`
/// precedent).
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

/// `ms` (the sampler's own epoch — see `frust_drive::metrics::TimestampMs`)
/// as `Hh Mm Ss`, dropping leading zero units.
fn fmt_duration_ms(ms: u64) -> String {
    let total_s = ms / 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_bytes_scales_binary_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn fmt_duration_ms_drops_leading_zero_units() {
        assert_eq!(fmt_duration_ms(5_000), "5s");
        assert_eq!(fmt_duration_ms(65_000), "1m 5s");
        assert_eq!(fmt_duration_ms(3_661_000), "1h 1m 1s");
    }
}
