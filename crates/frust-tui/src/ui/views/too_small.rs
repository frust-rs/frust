//! The too-small-terminal warning screen (D5 responsive breakpoints): shown
//! when the terminal is below the minimum workable size.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::ui::layout::centered;
use crate::ui::theme::Theme;

/// Minimum terminal width (columns) the shell renders in.
pub const MIN_WIDTH: u16 = 60;
/// Minimum terminal height (rows) the shell renders in.
pub const MIN_HEIGHT: u16 = 18;

/// `true` when `area` is too small to render the shell.
pub fn is_too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

/// Render the warning into `area`.
pub fn render(frame: &mut Frame, area: Rect, theme: &Theme) {
    let box_ = centered(area, area.width.min(40), 3);
    let lines = vec![
        Line::styled(
            "Terminal too small",
            Style::default()
                .fg(theme.warn())
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::styled(
            format!("Resize to at least {MIN_WIDTH}×{MIN_HEIGHT}"),
            Style::default().fg(theme.muted()),
        ),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), box_);
}
