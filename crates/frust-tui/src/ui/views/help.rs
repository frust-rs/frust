//! The keyboard/help overlay (`?`, T05 / D5): a workbook-styled popup listing
//! every command's keyhint. Sourced from exactly the same
//! [`crate::engine::palette::commands`] table the command palette renders —
//! a single source of truth for the whole workbench's key vocabulary, never a
//! second hand-maintained key list to drift out of sync with the palette's.
//!
//! Layering (D2): renders `&AppState` and only *registers* interaction (a
//! click closes it, mirroring the doctor panel's Close button); it never
//! mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{AppState, Message, RegionId, palette};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 50;

/// Render the help overlay centered over `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    // The same registry + gating the palette ranks — filtered to entries
    // that actually carry a keyhint (a handful of palette-only actions, like
    // "Run on all devices", have none and would just show a blank hint here).
    let commands: Vec<_> = palette::commands(state)
        .into_iter()
        .filter(|c| !c.hint.is_empty())
        .collect();

    let content_rows = commands.len() as u16 + 2; // + blank + close hint
    let height = (content_rows + 3).min(area.height); // 2 borders + 1 top padding
    let box_ = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            " Keyboard Shortcuts ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    let mut lines: Vec<Line<'static>> = commands
        .iter()
        .map(|cmd| {
            Line::from(vec![
                Span::styled(
                    format!("{:<8}", cmd.hint),
                    Style::default().fg(theme.accent()),
                ),
                Span::styled(cmd.title.to_string(), Style::default().fg(theme.fg())),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "Esc close",
        Style::default().fg(theme.muted()),
    ));
    frame.render_widget(Paragraph::new(lines), inner);

    // A click anywhere in the modal closes it (mouse parity for `Esc`) — the
    // rows carry no per-command action of their own (this is a reference
    // panel, not a second palette).
    mouse.click(box_, RegionId::HelpClose, Message::CloseHelpOverlay);
}
