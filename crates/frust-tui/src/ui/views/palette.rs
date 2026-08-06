//! The fuzzy command palette: a shadowed, centered
//! modal with a query prompt over a ranked command list, each row an enabled
//! command (executes on click/Enter) or a disabled-with-reason one (muted, not
//! runnable — the workbook disabled pattern). The base screen is rendered with
//! a *suppressed* `MouseCtx` (see `crate::ui::render`), so only this panel's
//! regions are live while it's open — the D4 base-layer suppression, plus the
//! binding `[Esc] Close` title affordance.
//!
//! Layering (D2): renders `&AppState` (the query + the ranked registry derived
//! from it) and only *registers* interaction; it never mutates the engine. The
//! command list comes from `crate::engine::palette` — this view carries no
//! command knowledge of its own.

use ratatui::Frame;
use ratatui::layout::{Alignment, Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::palette::{self, PaletteCommand};
use crate::engine::{AppState, Message, RegionId};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal dimensions (columns/rows).
const MODAL_WIDTH: u16 = 60;
const MODAL_HEIGHT: u16 = 18;

/// Render the command palette centered over `area`, reading the live query +
/// selection from `state.palette` and the ranked registry from
/// [`palette::ranked`].
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let box_ = centered(area, MODAL_WIDTH, MODAL_HEIGHT.min(area.height));
    frame.render_widget(Clear, box_);

    let query = state
        .palette
        .as_ref()
        .map(|p| p.query.as_str())
        .unwrap_or("");
    let ranked = palette::ranked(state);
    let selected = state
        .palette
        .as_ref()
        .map(|p| p.cursor)
        .unwrap_or(0)
        .min(ranked.len().saturating_sub(1));

    let title = Line::from(Span::styled(
        " \u{2318} Command palette ",
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    ));
    let close_title = Line::from(Span::styled(
        " [Esc] Close ",
        Style::default().fg(theme.muted()),
    ))
    .right_aligned();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(title)
        .title(close_title)
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    // Clickable `[Esc] Close` title affordance (binding for new modals).
    let close_w = " [Esc] Close ".chars().count() as u16;
    if close_w < box_.width {
        mouse.click(
            Rect::new(box_.right().saturating_sub(close_w + 1), box_.y, close_w, 1),
            RegionId::PaletteClose,
            Message::ClosePalette,
        );
    }

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let row = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;

    // Prompt row: a chevron, the live query, and a cursor block.
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{276f} ", Style::default().fg(theme.accent())),
            Span::styled(
                query.to_string(),
                Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("\u{2588}", Style::default().fg(theme.accent())),
        ])),
        row(y),
    );
    y += 1;

    // Underline rule.
    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "\u{2500}".repeat(inner.width as usize),
                Style::default().fg(theme.border()),
            )),
            row(y),
        );
        y += 1;
    }

    // Leave the last inner row for the footer hint.
    let list_bottom = inner.bottom().saturating_sub(1);

    if ranked.is_empty() {
        if y < list_bottom {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "  no matching command",
                    Style::default().fg(theme.muted()),
                )),
                row(y),
            );
        }
    } else {
        for (i, cmd) in ranked.iter().enumerate() {
            if y >= list_bottom {
                break;
            }
            render_command_row(frame, row(y), cmd, i == selected, theme, mouse, i);
            y += 1;
        }
    }

    // Footer keyhint row.
    let footer = row(inner.bottom().saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Line::styled(
            "\u{2191}\u{2193} move · Enter run · type to filter · Esc close",
            Style::default().fg(theme.muted()),
        )),
        footer,
    );
}

/// One command row: a selection marker + title on the left, and the keyhint (or
/// the disabled reason, for a gated command) on the right. Enabled rows register
/// a click executing them; a disabled row renders muted and is not clickable.
fn render_command_row(
    frame: &mut Frame,
    rect: Rect,
    cmd: &PaletteCommand,
    is_selected: bool,
    theme: &Theme,
    mouse: &mut MouseCtx,
    index: usize,
) {
    if is_selected {
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.overlay())),
            rect,
        );
    }

    let marker = if is_selected { "\u{25b8} " } else { "  " };
    let title_style = if !cmd.enabled {
        Style::default().fg(theme.muted())
    } else if is_selected {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent())),
            Span::styled(cmd.title.to_string(), title_style),
        ])),
        rect,
    );

    // Right-aligned keyhint, or the disabled reason for a gated command.
    let (right_text, right_style) = match cmd.disabled_reason {
        Some(reason) => (format!("{reason} "), Style::default().fg(theme.warn())),
        None if !cmd.hint.is_empty() => {
            (format!("{} ", cmd.hint), Style::default().fg(theme.muted()))
        }
        None => (String::new(), Style::default()),
    };
    if !right_text.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(right_text, right_style)))
                .alignment(Alignment::Right),
            rect,
        );
    }

    if cmd.enabled {
        mouse.click(
            rect,
            RegionId::PaletteRow(index),
            Message::PaletteExecuteAt(index),
        );
    }
}
