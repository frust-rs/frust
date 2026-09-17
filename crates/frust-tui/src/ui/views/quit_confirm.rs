//! The quit-confirm dialog: a small shadowed, centered confirm popup over
//! the workbench asking before every running device app is force-stopped —
//! only shown when at least one tracked session is still live (see
//! `crate::engine::update`'s `RequestQuit` arm, which skips straight to
//! quitting when there is none). The base workbench layer is rendered with a
//! *suppressed* `MouseCtx` (see `crate::ui::render`), so only this dialog's
//! regions are live while it is open — the base-layer suppression, same as
//! the clean-confirm dialog.
//!
//! Layering: renders the live-session count and only *registers*
//! interaction; it never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{Message, RegionId};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal dimensions (columns/rows).
const MODAL_WIDTH: u16 = 56;
const MODAL_HEIGHT: u16 = 8;

/// Render the quit-confirm dialog centered over `area`, warning about
/// `running` live sessions (singular/plural body text).
pub fn render(frame: &mut Frame, area: Rect, running: usize, theme: &Theme, mouse: &mut MouseCtx) {
    let box_ = centered(area, MODAL_WIDTH, MODAL_HEIGHT.min(area.height));
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.warn()))
        .title(Span::styled(
            " Quit frust? ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;

    let body = if running == 1 {
        "1 running session will be stopped.".to_string()
    } else {
        format!("{running} running sessions will be stopped.")
    };
    frame.render_widget(
        Paragraph::new(Line::styled(body, Style::default().fg(theme.fg()))),
        row_rect(y),
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Device apps are force-stopped on quit.",
            Style::default().fg(theme.muted()),
        )),
        row_rect(y),
    );
    y += 2;

    let yes_label = " Quit ";
    let no_label = " Cancel ";
    let yes_style = Style::default()
        .fg(theme.bg())
        .bg(theme.warn())
        .add_modifier(Modifier::BOLD);
    let no_style = Style::default().fg(theme.fg()).bg(theme.overlay());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(yes_label, yes_style),
            Span::raw("   "),
            Span::styled(no_label, no_style),
        ])),
        row_rect(y),
    );
    let yes_w = yes_label.chars().count() as u16;
    mouse.click(
        Rect::new(inner.x, y, yes_w, 1),
        RegionId::QuitConfirmYes,
        Message::ConfirmQuit,
    );
    let no_x = inner.x + yes_w + 3;
    mouse.click(
        Rect::new(no_x, y, no_label.chars().count() as u16, 1),
        RegionId::QuitConfirmNo,
        Message::CloseQuitConfirm,
    );
    y += 1;

    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Enter/y quit · Esc/n cancel",
                Style::default().fg(theme.muted()),
            )),
            row_rect(y),
        );
    }
}
