//! The clean-confirm dialog: a small shadowed, centered
//! confirm popup over the workbench before running `cargo clean` +
//! removing the generated Android/iOS build directories. The base workbench
//! layer is rendered with a *suppressed* `MouseCtx` (see `crate::ui::render`),
//! so only this dialog's regions are live while it is open — the
//! base-layer suppression.
//!
//! Layering: renders the target project path and only *registers*
//! interaction; it never mutates the engine.

use std::path::Path;

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

/// Render the clean-confirm dialog centered over `area`, for `project_root`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    project_root: &Path,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let box_ = centered(area, MODAL_WIDTH, MODAL_HEIGHT.min(area.height));
    frame.render_widget(Clear, box_);

    let project = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| project_root.to_string_lossy().into_owned());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.warn()))
        .title(Span::styled(
            " Clean ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;

    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("Remove build output for {project}?"),
            Style::default().fg(theme.fg()),
        )),
        row_rect(y),
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Line::styled(
            "cargo clean · android/app/build · android/.gradle · build/",
            Style::default().fg(theme.muted()),
        )),
        row_rect(y),
    );
    y += 2;

    let yes_label = " Clean ";
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
        RegionId::CleanConfirmYes,
        Message::ConfirmClean,
    );
    let no_x = inner.x + yes_w + 3;
    mouse.click(
        Rect::new(no_x, y, no_label.chars().count() as u16, 1),
        RegionId::CleanConfirmNo,
        Message::CloseCleanConfirm,
    );
    y += 1;

    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Enter/y clean · Esc cancel",
                Style::default().fg(theme.muted()),
            )),
            row_rect(y),
        );
    }
}
