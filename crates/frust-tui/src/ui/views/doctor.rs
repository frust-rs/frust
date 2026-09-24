//! The doctor panel: a shadowed, centered popup listing every
//! validator's status + actionable hints, with a re-run action and a route
//! into the toolchain bootstrap wizard (`t` / "Toolchain setup" — D5). The
//! base workbench layer is rendered with a *suppressed* `MouseCtx` (see
//! `crate::ui::render`), so only this panel's regions are live while it is
//! open — the base-layer suppression.
//!
//! Layering: renders `&DoctorState` and only *registers* interaction; it
//! never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Alignment, Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{DoctorState, Message, RegionId};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use frust_drive::doctor::Status;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 66;

/// Render the doctor panel centered over `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    doctor: &DoctorState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let inner_rows = check_rows(doctor) + 4; // heading(1)+blank(1)+checks+blank(1)+buttons(1)+hint(1)
    let height = (inner_rows + 2).min(area.height);
    let box_ = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            " Doctor ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;
    let advance = |y: &mut u16| *y += 1;

    if doctor.results.is_empty() {
        let label = if doctor.refreshing {
            "\u{25cc} checking toolchain…"
        } else {
            "no results yet — press r to check"
        };
        frame.render_widget(
            Paragraph::new(Line::styled(label, Style::default().fg(theme.muted()))),
            row_rect(y),
        );
        advance(&mut y);
    } else {
        for check in &doctor.results {
            if y >= inner.bottom() {
                break;
            }
            let (glyph, color) = status_glyph(check.status, theme);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{glyph} "), Style::default().fg(color)),
                    Span::styled(check.name.clone(), Style::default().fg(theme.fg())),
                ])),
                row_rect(y),
            );
            advance(&mut y);
            if check.status != Status::Pass {
                for message in &check.messages {
                    if y >= inner.bottom() {
                        break;
                    }
                    frame.render_widget(
                        Paragraph::new(Line::styled(
                            format!("    {message}"),
                            Style::default().fg(theme.muted()),
                        )),
                        row_rect(y),
                    );
                    advance(&mut y);
                }
            }
        }
        if doctor.refreshing {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "\u{25cc} re-checking…",
                    Style::default().fg(theme.warn()),
                )),
                row_rect(y),
            );
            advance(&mut y);
        }
    }

    advance(&mut y); // blank

    // ── Rerun / Close / Toolchain setup buttons ─────────────────────────
    if y < inner.bottom() {
        let rerun_label = " Re-run ";
        let close_label = " Close ";
        let toolchain_label = " Toolchain setup ";
        let rerun_style = Style::default()
            .fg(theme.bg())
            .bg(theme.accent())
            .add_modifier(Modifier::BOLD);
        let close_style = Style::default().fg(theme.fg()).bg(theme.overlay());
        let toolchain_style = Style::default().fg(theme.fg()).bg(theme.overlay());
        let buttons = Line::from(vec![
            Span::styled(rerun_label, rerun_style),
            Span::raw("   "),
            Span::styled(close_label, close_style),
            Span::raw("   "),
            Span::styled(toolchain_label, toolchain_style),
        ]);
        frame.render_widget(Paragraph::new(buttons), row_rect(y));
        let rerun_w = rerun_label.chars().count() as u16;
        mouse.click(
            Rect::new(inner.x, y, rerun_w, 1),
            RegionId::DoctorRerun,
            Message::RunDoctor,
        );
        let close_x = inner.x + rerun_w + 3;
        let close_w = close_label.chars().count() as u16;
        mouse.click(
            Rect::new(close_x, y, close_w, 1),
            RegionId::DoctorClose,
            Message::CloseDoctorPanel,
        );
        let toolchain_x = close_x + close_w + 3;
        mouse.click(
            Rect::new(toolchain_x, y, toolchain_label.chars().count() as u16, 1),
            RegionId::DoctorToolchainSetup,
            Message::OpenToolchainFromDoctor,
        );
        advance(&mut y);
    }

    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "r re-run · t toolchain · Esc close",
                Style::default().fg(theme.muted()),
            ))
            .alignment(Alignment::Left),
            row_rect(y),
        );
    }
}

/// The status glyph + color for a validator's result.
fn status_glyph(status: Status, theme: &Theme) -> (&'static str, Color) {
    match status {
        Status::Pass => ("\u{2713}", theme.success()),
        Status::Partial => ("!", theme.warn()),
        Status::Fail => ("\u{2717}", theme.error()),
    }
}

/// The number of rows the check list needs (one per check, plus one per
/// non-passing check's messages, plus one if a refresh is in flight) — used
/// to size the modal.
fn check_rows(doctor: &DoctorState) -> u16 {
    if doctor.results.is_empty() {
        return 1;
    }
    let mut rows = 0u16;
    for check in &doctor.results {
        rows += 1;
        if check.status != Status::Pass {
            rows += check.messages.len() as u16;
        }
    }
    if doctor.refreshing {
        rows += 1;
    }
    rows
}
