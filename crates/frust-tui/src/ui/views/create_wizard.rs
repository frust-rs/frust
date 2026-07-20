//! The create-project wizard modal (PLAN.md D6b): a shadowed, centered popup
//! over the welcome/workbench base layer, stepping name → directory →
//! architecture → an off-thread scaffold. The base layer is rendered with a
//! *suppressed* `MouseCtx` (see `crate::ui::render`), so only this modal's
//! regions are live while it is open — the D4 base-layer suppression.
//!
//! Layering (D2): renders `&CreateWizard` and only *registers* interaction; it
//! never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{ArchCard, CreateWizard, Message, RegionId, WizardStep};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 64;

/// Render the create wizard centered over `area`, registering its interactive
/// regions through `mouse` (the *live* ctx — the base layer beneath was drawn
/// suppressed).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    wizard: &CreateWizard,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let height = (inner_rows(wizard) + 3).min(area.height);
    let box_ = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            " New Frust project ",
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

    // ── Step indicator ───────────────────────────────────────────────────
    frame.render_widget(Paragraph::new(step_indicator(wizard, theme)), row_rect(y));
    advance(&mut y);
    advance(&mut y); // blank

    // ── Step body ────────────────────────────────────────────────────────
    match wizard.step {
        WizardStep::Name => {
            render_field(
                frame,
                row_rect(y),
                "Name",
                &wizard.name,
                "my_app",
                true,
                theme,
            );
            advance(&mut y);
            let hint = match &wizard.name_error {
                Some(err) => Span::styled(err.clone(), Style::default().fg(theme.error())),
                None => Span::styled(
                    "A lowercase crate name (a-z, 0-9, _).",
                    Style::default().fg(theme.muted()),
                ),
            };
            frame.render_widget(Paragraph::new(Line::from(hint)), row_rect(y));
            advance(&mut y);
            advance(&mut y); // spacer to keep the modal height steady across steps
        }
        WizardStep::Directory => {
            render_field(
                frame,
                row_rect(y),
                "Directory",
                &wizard.directory,
                "my_app",
                true,
                theme,
            );
            advance(&mut y);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "Where to scaffold (relative to the current directory).",
                    Style::default().fg(theme.muted()),
                )),
                row_rect(y),
            );
            advance(&mut y);
        }
        WizardStep::Arch => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "Architecture",
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                )),
                row_rect(y),
            );
            advance(&mut y);
            for (i, card) in wizard.arches.iter().enumerate() {
                if y + 1 >= inner.bottom() {
                    break;
                }
                let focused = wizard.arch_cursor == i;
                render_card(frame, row_rect(y), row_rect(y + 1), card, focused, theme);
                mouse.click(
                    Rect::new(inner.x, y, inner.width, 2),
                    RegionId::WizardArchCard(i),
                    Message::CreateWizardSelectArchAt(i),
                );
                advance(&mut y);
                advance(&mut y);
            }
        }
        WizardStep::Scaffolding => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{25cc} ", Style::default().fg(theme.warn())),
                    Span::styled(
                        format!("Scaffolding {}…", wizard.name),
                        Style::default().fg(theme.fg()),
                    ),
                ])),
                row_rect(y),
            );
            advance(&mut y);
        }
        WizardStep::Error => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{2717} ", Style::default().fg(theme.error())),
                    Span::styled(
                        wizard.error.clone().unwrap_or_default(),
                        Style::default().fg(theme.error()),
                    ),
                ])),
                row_rect(y),
            );
            advance(&mut y);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "Enter to retry · Esc to go back",
                    Style::default().fg(theme.muted()),
                )),
                row_rect(y),
            );
            advance(&mut y);
        }
    }

    // Pad the cursor down to the button row so buttons/hint sit at the bottom
    // regardless of body height.
    let buttons_y = inner.bottom().saturating_sub(2);
    y = y.max(inner.y).min(buttons_y);
    while y < buttons_y {
        advance(&mut y);
    }

    // ── Back / Next-or-Create / Cancel buttons ──────────────────────────
    if y < inner.bottom() {
        render_buttons(frame, mouse, row_rect(y), inner.x, wizard, theme);
        advance(&mut y);
    }

    // ── Keyhint row ─────────────────────────────────────────────────────
    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                wizard_hint(wizard.step),
                Style::default().fg(theme.muted()),
            )),
            row_rect(y),
        );
    }
}

/// The inner content height (rows) a given wizard step needs — the modal box is
/// this plus the border and top padding.
fn inner_rows(wizard: &CreateWizard) -> u16 {
    let body = match wizard.step {
        WizardStep::Name => 3,
        WizardStep::Directory => 2,
        WizardStep::Arch => 1 + wizard.arches.len() as u16 * 2,
        WizardStep::Scaffolding => 1,
        WizardStep::Error => 2,
    };
    // indicator + blank + body + blank + buttons + hint
    2 + body + 1 + 1 + 1
}

/// The "Step N of 3 · <label>" indicator line.
fn step_indicator(wizard: &CreateWizard, theme: &Theme) -> Line<'static> {
    let (n, label): (u8, &str) = match wizard.step {
        WizardStep::Name => (1, "Name"),
        WizardStep::Directory => (2, "Directory"),
        WizardStep::Arch => (3, "Architecture"),
        WizardStep::Scaffolding => (3, "Scaffolding"),
        WizardStep::Error => (3, "Error"),
    };
    Line::from(vec![
        Span::styled(
            format!("Step {n} of 3"),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  ·  {label}"), Style::default().fg(theme.muted())),
    ])
}

/// A labeled text field row with a blinking caret when focused.
fn render_field(
    frame: &mut Frame,
    rect: Rect,
    label: &str,
    value: &str,
    placeholder: &str,
    focused: bool,
    theme: &Theme,
) {
    let label_span = Span::styled(
        format!("{label:<11}"),
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD),
    );
    let value_span = if value.is_empty() {
        Span::styled(placeholder.to_string(), Style::default().fg(theme.muted()))
    } else {
        Span::styled(value.to_string(), Style::default().fg(theme.fg()))
    };
    let mut spans = vec![label_span, value_span];
    if focused {
        spans.push(Span::styled(
            "\u{2588}",
            Style::default().fg(theme.accent()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
}

/// One architecture card: a label/marker line and a dim description (or the
/// disabled reason) line beneath it.
fn render_card(
    frame: &mut Frame,
    label_rect: Rect,
    desc_rect: Rect,
    card: &ArchCard,
    focused: bool,
    theme: &Theme,
) {
    let marker = if focused { "\u{25b8} " } else { "  " };
    let check = if focused { "(o) " } else { "( ) " };
    let label_style = if !card.enabled {
        Style::default().fg(theme.muted())
    } else if focused {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(theme.accent())),
            Span::styled(check.to_string(), Style::default().fg(theme.accent())),
            Span::styled(card.label.clone(), label_style),
        ])),
        label_rect,
    );
    // The description, or (when disabled) the reason it can't be chosen.
    let (text, color) = match &card.disabled_reason {
        Some(reason) => (format!("\u{2717} {reason}"), theme.warn()),
        None => (card.description.clone(), theme.muted()),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("      {text}"),
            Style::default().fg(color),
        )),
        desc_rect,
    );
}

/// Render the Back / Next-or-Create / Cancel buttons and register their click
/// regions.
fn render_buttons(
    frame: &mut Frame,
    mouse: &mut MouseCtx,
    rect: Rect,
    x0: u16,
    wizard: &CreateWizard,
    theme: &Theme,
) {
    // The primary action label per step.
    let primary = match wizard.step {
        WizardStep::Name | WizardStep::Directory => Some(" Next "),
        WizardStep::Arch => Some(" Create "),
        WizardStep::Error => Some(" Retry "),
        WizardStep::Scaffolding => None,
    };
    let back_enabled = wizard.step != WizardStep::Scaffolding;

    let back_label = " Back ";
    let back_style = if back_enabled {
        Style::default().fg(theme.fg()).bg(theme.overlay())
    } else {
        Style::default().fg(theme.muted()).bg(theme.overlay())
    };
    let cancel_style = Style::default().fg(theme.fg()).bg(theme.overlay());
    let primary_style = Style::default()
        .fg(theme.bg())
        .bg(theme.accent())
        .add_modifier(Modifier::BOLD);

    let mut spans = vec![Span::styled(back_label, back_style), Span::raw("  ")];
    let mut x = x0;
    if back_enabled {
        mouse.click(
            Rect::new(x, rect.y, back_label.chars().count() as u16, 1),
            RegionId::WizardBack,
            Message::CreateWizardBack,
        );
    }
    x += back_label.chars().count() as u16 + 2;

    if let Some(primary_label) = primary {
        spans.push(Span::styled(primary_label, primary_style));
        spans.push(Span::raw("  "));
        mouse.click(
            Rect::new(x, rect.y, primary_label.chars().count() as u16, 1),
            RegionId::WizardNext,
            Message::CreateWizardAdvance,
        );
        x += primary_label.chars().count() as u16 + 2;
    }

    let cancel_label = " Cancel ";
    spans.push(Span::styled(cancel_label, cancel_style));
    mouse.click(
        Rect::new(x, rect.y, cancel_label.chars().count() as u16, 1),
        RegionId::WizardCancel,
        Message::CloseCreateWizard,
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
}

/// The per-step keyhint line.
fn wizard_hint(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Name | WizardStep::Directory => "Type · Enter next · Esc back",
        WizardStep::Arch => "↑/↓ choose · Enter create · Esc back",
        WizardStep::Scaffolding => "Scaffolding…",
        WizardStep::Error => "Enter retry · Esc back",
    }
}
