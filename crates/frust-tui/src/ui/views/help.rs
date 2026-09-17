//! The keyboard/help overlay (`?`): a shadowed, centered popup listing every
//! command's keyhint. Sourced from exactly the same
//! [`crate::engine::palette::commands`] table the command palette renders —
//! a single source of truth for the whole workbench's key vocabulary, never a
//! second hand-maintained key list to drift out of sync with the palette's.
//!
//! Every registry entry shows here, including the handful with no keyhint
//! (mouse/palette-only actions like "Run on all devices") — those render a
//! `—` hint rather than being filtered out, which used to hide a real command
//! from the one place a user looks up the whole vocabulary. The full registry
//! (2026: 27 commands) no longer fits one column on an 80x24 terminal without
//! clipping, so this renders two columns side by side instead of scrolling —
//! simpler to keep correct than a scroll offset, and it needs no new
//! `AppState` field.
//!
//! Layering: renders `&AppState` and only *registers* interaction (a
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

/// Width of the left-aligned keyhint field within each command column
/// (`^O`/`⌥m` are the widest real hints at two characters; a key-less
/// command's hint renders as `—`).
const HINT_WIDTH: usize = 4;

/// Columns of blank space between the overlay's two command columns.
const COLUMN_GAP: usize = 3;

/// Render the help overlay centered over `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    // The same registry + gating the palette ranks — nothing filtered out
    // (see the module doc comment on why key-less entries still render).
    let commands = palette::commands(state);
    let hint_width = HINT_WIDTH;
    let title_width = commands
        .iter()
        .map(|c| c.title.chars().count())
        .max()
        .unwrap_or(0);
    fn hint_of(hint: &str) -> &str {
        if hint.is_empty() { "—" } else { hint }
    }

    // Split the registry across two columns, left column carrying the extra
    // row when the count is odd.
    let rows_per_col = commands.len().div_ceil(2).max(1);
    let (left, right) = commands.split_at(rows_per_col.min(commands.len()));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows_per_col + 3);
    for (i, left_cmd) in left.iter().enumerate() {
        let mut spans = vec![
            Span::styled(
                format!("{:<hint_width$}", hint_of(left_cmd.hint)),
                Style::default().fg(theme.accent()),
            ),
            Span::styled(
                format!("{:<title_width$}", left_cmd.title),
                Style::default().fg(theme.fg()),
            ),
        ];
        if let Some(cmd) = right.get(i) {
            spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
            spans.push(Span::styled(
                format!("{:<hint_width$}", hint_of(cmd.hint)),
                Style::default().fg(theme.accent()),
            ));
            spans.push(Span::styled(
                cmd.title.to_string(),
                Style::default().fg(theme.fg()),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "— = palette/mouse only",
        Style::default().fg(theme.muted()),
    ));
    lines.push(Line::styled(
        "Esc close",
        Style::default().fg(theme.muted()),
    ));

    let col_width = hint_width + title_width;
    let content_width = col_width * 2 + COLUMN_GAP;
    let modal_width = (content_width + 6) as u16; // 2 borders + 2+2 h-padding
    let content_rows = (rows_per_col + 3) as u16; // + blank + note + close hint
    let height = content_rows + 3; // 2 borders + 1 top padding
    let box_ = centered(area, modal_width, height);
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

    frame.render_widget(Paragraph::new(lines), inner);

    // A click anywhere in the modal closes it (mouse parity for `Esc`) — the
    // rows carry no per-command action of their own (this is a reference
    // panel, not a second palette).
    mouse.click(box_, RegionId::HelpClose, Message::CloseHelpOverlay);
}
