//! The welcome screen (D6b / workbook B1): a brand splash with exactly one
//! action — the single large Create button. Shown when no project is detected.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::engine::{AppState, Message, RegionId};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use crate::ui::widgets::{ButtonState, big_button};

/// The brand wordmark ("Frust") in a 2-row half-block face (matches workbook
/// B1). Hand-rolled rather than a `tui-big-text` dependency: deterministic in
/// snapshot tests and free of an extra pre-1.0 pin.
const WORDMARK: [&str; 2] = ["█▀▀ █▀█ █░█ █▀ ▀█▀", "█▀░ █▀▄ █▄█ ▄█ ░█░"];

/// The orange horizontal-rule motif under the wordmark.
const UNDERLINE: &str = "━━━━━━━━━━━━━━━━━━";

const TAGLINE: &str = "FEARLESS UI IN RUST";
const SUBTITLE: &str = "No Frust project found in this directory.";
const HINT: &str = "Enter · or click";

const BUTTON_WIDTH: u16 = 46;
const BUTTON_HEIGHT: u16 = 5;
const SPLASH_WIDTH: u16 = 48;
/// wordmark(2) + underline(1) + gap(1) + tagline(1) + gap(1) + subtitle(1) +
/// gap(1) + button(5) + gap(1) + hint(1)
const SPLASH_HEIGHT: u16 = 15;

/// Render the welcome screen into `area`, registering the Create button's
/// mouse region through `mouse`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let box_ = crate::ui::layout::centered(area, SPLASH_WIDTH, SPLASH_HEIGHT);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // wordmark line 1
            Constraint::Length(1), // wordmark line 2
            Constraint::Length(1), // underline
            Constraint::Length(1), // gap
            Constraint::Length(1), // tagline
            Constraint::Length(1), // gap
            Constraint::Length(1), // subtitle
            Constraint::Length(1), // gap
            Constraint::Length(BUTTON_HEIGHT),
            Constraint::Length(1), // gap
            Constraint::Length(1), // hint
        ])
        .split(box_);

    let cream = Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD);
    centered_line(frame, rows[0], Line::styled(WORDMARK[0], cream));
    centered_line(frame, rows[1], Line::styled(WORDMARK[1], cream));
    centered_line(
        frame,
        rows[2],
        Line::styled(UNDERLINE, Style::default().fg(theme.accent())),
    );
    centered_line(
        frame,
        rows[4],
        Line::styled(TAGLINE, Style::default().fg(theme.muted())),
    );
    centered_line(
        frame,
        rows[6],
        Line::styled(SUBTITLE, Style::default().fg(theme.muted())),
    );

    // The single large Create button, horizontally centered in its row band.
    let btn_area = crate::ui::layout::centered(rows[8], BUTTON_WIDTH, BUTTON_HEIGHT);
    let btn_state = if state.create_pressed {
        ButtonState::Pressed
    } else if state.hover == Some(RegionId::CreateButton) {
        ButtonState::Hovered
    } else {
        ButtonState::Normal
    };
    // Keyboard parity: Enter/c activate the same action (see runner).
    big_button(
        frame,
        btn_area,
        theme.icons.create(),
        "Create a new Frust project",
        btn_state,
        theme,
    );
    mouse.button(btn_area, RegionId::CreateButton);

    centered_line(
        frame,
        rows[10],
        Line::styled(HINT, Style::default().fg(theme.muted())),
    );
}

/// Render the titlebar toolchain chip for the welcome screen (right-aligned),
/// wired to the real startup-preflight state — the same chip logic
/// `views::workbench::titlebar` uses. Clicking it opens the bootstrap wizard
/// (D6a; keyboard parity: `i`), so a fresh machine can reach the toolchain
/// setup even before creating a project.
pub fn titlebar(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let row = Rect::new(area.x, area.y, area.width, 1);
    let (glyph, label, color) = super::workbench::toolchain_chip(state, theme);
    let prefix = "toolchain ";
    let chip_text = format!("{glyph} {label}");
    let chip = Line::from(vec![
        Span::styled(prefix.to_string(), Style::default().fg(theme.muted())),
        Span::styled(chip_text.clone(), Style::default().fg(color)),
    ]);
    frame.render_widget(Paragraph::new(chip).alignment(Alignment::Right), row);

    // The chip is right-aligned, so its click rect is the trailing columns.
    let chip_w = (prefix.chars().count() + chip_text.chars().count()) as u16;
    if chip_w <= area.width {
        let chip_x = area.right() - chip_w;
        mouse.click(
            Rect::new(chip_x, area.y, chip_w, 1),
            RegionId::DoctorChip,
            Message::OpenBootstrapWizard,
        );
    }
}

fn centered_line(frame: &mut Frame, area: Rect, line: Line) {
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}
