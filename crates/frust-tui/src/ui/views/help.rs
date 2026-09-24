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

/// Floor for the left-aligned keyhint field within each command column, in
/// display columns — not the actual width. The column is sized to the
/// registry's widest hint (via [`ratatui::text::Span::width`], display
/// columns rather than byte or `char` count, though for every hint in this
/// registry the two coincide) whenever that exceeds the floor — e.g.
/// `Alt+m` (5 columns) off macOS, which used to overflow a fixed 4-column
/// field and push that one row's title a column later than every other
/// row's (see [`hint_column_width`], and the regression test below). The
/// floor matches macOS's own widest hint (`^O`/`⌥m`, both 2 columns) plus
/// breathing room, so macOS's rendered overlay is unchanged by this file.
const MIN_HINT_WIDTH: usize = 4;

/// Columns of blank space between the overlay's two command columns.
const COLUMN_GAP: usize = 3;

/// A key-less command's hint renders as `—` rather than being left blank
/// (see the module doc comment on why key-less entries still show at all).
fn hint_of(hint: &str) -> &str {
    if hint.is_empty() { "—" } else { hint }
}

/// The keyhint column width (display columns) `commands` needs: the widest
/// hint in the registry (after the empty-hint-to-`—` substitution), floored
/// at [`MIN_HINT_WIDTH`].
fn hint_column_width(commands: &[palette::PaletteCommand]) -> usize {
    commands
        .iter()
        .map(|c| Span::raw(hint_of(c.hint)).width())
        .max()
        .unwrap_or(0)
        .max(MIN_HINT_WIDTH)
}

/// Builds the overlay's two-column line list plus the `(hint_width,
/// title_width)` the modal is sized from. Split out from [`render`] so the
/// column-alignment invariant — every row's title starts in the same place —
/// is unit-testable against a plain [`palette::PaletteCommand`] list, with no
/// `Frame` needed.
fn command_lines(
    commands: &[palette::PaletteCommand],
    theme: &Theme,
) -> (Vec<Line<'static>>, usize, usize) {
    let hint_width = hint_column_width(commands);
    let title_width = commands
        .iter()
        .map(|c| c.title.chars().count())
        .max()
        .unwrap_or(0);

    // Split the registry across two columns, left column carrying the extra
    // row when the count is odd.
    let rows_per_col = commands.len().div_ceil(2).max(1);
    let (left, right) = commands.split_at(rows_per_col.min(commands.len()));

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows_per_col);
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
    (lines, hint_width, title_width)
}

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
    let (mut lines, hint_width, title_width) = command_lines(&commands, theme);
    let rows_per_col = lines.len();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::palette::PaletteCommand;
    use crate::ui::theme::Theme;

    fn cmd(title: &'static str, hint: &'static str) -> PaletteCommand {
        PaletteCommand {
            title,
            hint,
            message: Message::RequestQuit,
            enabled: true,
            disabled_reason: None,
        }
    }

    /// Byte offset of the `n`th `char` in `s`, or `s.len()` past the end —
    /// `str` indexing is by byte, but our column math is by `char` (every
    /// hint here is single-width, so `char` count and display-column width
    /// coincide), and `—` alone is a multi-byte `char`.
    fn nth_char_byte(s: &str, n: usize) -> usize {
        s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
    }

    /// Regression test for the W4 fix this card lands: off macOS, the
    /// registry's `Alt+m` hint is 5 display columns wide — wider than the
    /// old fixed `const HINT_WIDTH: usize = 4` this file used to hard-code.
    /// Checked to fail against that pre-fix code: hardcoding
    /// `hint_column_width` back to `4` and rerunning this test fails the
    /// "Toggle mouse capture" row's title-position assertion below, because
    /// `format!("{:<4}", "Alt+m")` doesn't pad or truncate a 5-char value
    /// into a 4-wide field — it just leaves the field one column too wide,
    /// which pushes that one row's title a column later than every other
    /// row's in the same column.
    #[test]
    fn every_row_title_starts_in_the_same_column_off_macos() {
        let commands = vec![
            cmd("Run on device(s)…", "r"),
            cmd("Toggle mouse capture", "Alt+m"),
            cmd("Doctor", "i"),
            cmd("Stop session", "x"),
            cmd("Switch project…", "^O"),
            cmd("Quit", ""),
        ];
        let theme = Theme::frust_dark();
        let (lines, hint_width, title_width) = command_lines(&commands, &theme);

        assert!(
            hint_width >= Span::raw("Alt+m").width(),
            "hint column ({hint_width}) must fit `Alt+m`"
        );

        let rows_per_col = commands.len().div_ceil(2);
        let (left, right) = commands.split_at(rows_per_col);
        let right_title_at = hint_width + title_width + COLUMN_GAP + hint_width;

        assert_eq!(lines.len(), left.len());
        for (i, line) in lines.iter().enumerate() {
            let rendered = line.to_string();
            let left_title = left[i].title;
            let left_at = nth_char_byte(&rendered, hint_width);
            assert!(
                rendered[left_at..].starts_with(left_title),
                "row {i}: left title {left_title:?} does not start at column \
                 {hint_width} in {rendered:?}"
            );
            if let Some(right_cmd) = right.get(i) {
                let right_at = nth_char_byte(&rendered, right_title_at);
                assert!(
                    rendered[right_at..].starts_with(right_cmd.title),
                    "row {i}: right title {:?} does not start at column \
                     {right_title_at} in {rendered:?}",
                    right_cmd.title
                );
            }
        }
    }
}
