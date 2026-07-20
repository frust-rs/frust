//! Reusable render widgets. Phase 1 ships the enlarged, padded button (D5:
//! 3-row bordered target with hover + pressed states).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};

use super::theme::Theme;

/// Visual state of a [`big_button`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    /// Idle.
    Normal,
    /// Pointer over the button (hover glow: overlay fill, accent border).
    Hovered,
    /// Pressed (inverted flash).
    Pressed,
}

/// Draw an enlarged, padded, rounded-border button filling `area`.
///
/// The label is centered; the border brightens to `accent` on hover and the
/// fill inverts to `primary` on press (matching the workbook B1 behavior). A
/// leading `icon` (already resolved from [`Theme::icons`]) prefixes the label.
pub fn big_button(
    frame: &mut Frame,
    area: Rect,
    icon: &str,
    label: &str,
    state: ButtonState,
    theme: &Theme,
) {
    // Both a color change (for real terminals) AND a text-visible change (so
    // the plain-text snapshot suite actually distinguishes the states): hover
    // wraps the label in chevron affordances; press switches the border to a
    // thick rule.
    let (border_color, border_type, fill, text_color, bold) = match state {
        ButtonState::Normal => (
            theme.border(),
            BorderType::Rounded,
            theme.surface(),
            theme.fg(),
            false,
        ),
        ButtonState::Hovered => (
            theme.accent(),
            BorderType::Rounded,
            theme.overlay(),
            theme.fg(),
            true,
        ),
        ButtonState::Pressed => (
            theme.accent(),
            BorderType::Thick,
            theme.primary(),
            theme.bg(),
            true,
        ),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(2))
        .style(Style::default().bg(fill));

    let mut text_style = Style::default().fg(text_color);
    if bold {
        text_style = text_style.add_modifier(Modifier::BOLD);
    }

    let core = format!("{icon}  {label}");
    let label_line = if state == ButtonState::Hovered {
        Line::from(Span::styled(
            format!("\u{25b8}  {core}  \u{25c2}"),
            text_style,
        ))
    } else {
        Line::from(Span::styled(core, text_style))
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Vertically center the single label line inside the (padded) inner area.
    let mid = inner.y + inner.height / 2;
    let text_area = Rect::new(inner.x, mid, inner.width, 1);
    frame.render_widget(
        Paragraph::new(label_line).alignment(Alignment::Center),
        text_area,
    );
}
