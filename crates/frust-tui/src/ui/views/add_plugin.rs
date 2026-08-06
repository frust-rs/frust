//! The Add Plugin dialog modal: a shadowed, centered popup over the
//! welcome/workbench base layer, stepping select-plugin → toggle-features →
//! an off-thread apply → a per-edit report
//! (or an error). The base layer is rendered with a *suppressed* `MouseCtx`
//! (see `crate::ui::render`), so only this modal's regions are live while it is
//! open — the D4 base-layer suppression, plus the binding `[Esc] Close` title
//! affordance.
//!
//! Layering (D2): renders `&AddPluginDialog` and only *registers* interaction;
//! it never mutates the engine. Cloned in shape from `views::create_wizard`.

use frust_drive::plugin::AddOutcome;
use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{
    AddPluginDialog, AddPluginStep, FeatureToggle, Message, PluginEntry, RegionId,
};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal width (columns) — matches the create wizard's `MODAL_WIDTH`.
const MODAL_WIDTH: u16 = 64;

/// The `[Esc] Close` title affordance text (a binding for new modals — see the
/// design workbook Part B).
const CLOSE_LABEL: &str = " [Esc] Close ";

/// Render the Add Plugin dialog centered over `area`, registering its
/// interactive regions through `mouse` (the *live* ctx — the base layer beneath
/// was drawn suppressed).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    dialog: &AddPluginDialog,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let height = inner_rows(dialog).saturating_add(3).min(area.height);
    let box_ = centered(area, MODAL_WIDTH.min(area.width), height);
    frame.render_widget(Clear, box_);

    let title = Line::from(Span::styled(
        " Add plugin ",
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    ));
    let close_title = Line::from(Span::styled(
        CLOSE_LABEL,
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

    // Clickable `[Esc] Close` title affordance.
    let close_w = CLOSE_LABEL.chars().count() as u16;
    if close_w < box_.width {
        mouse.click(
            Rect::new(box_.right().saturating_sub(close_w + 1), box_.y, close_w, 1),
            RegionId::AddPluginClose,
            Message::CloseAddPlugin,
        );
    }

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let mut y = inner.y;
    let advance = |y: &mut u16| *y += 1;

    match dialog.step {
        AddPluginStep::Select => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "Choose a plugin to add to this project",
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                )),
                row_rect(y),
            );
            advance(&mut y);
            for (i, entry) in dialog.entries.iter().enumerate() {
                if y + 1 >= inner.bottom() {
                    break;
                }
                let focused = dialog.cursor == i;
                render_entry(frame, row_rect(y), row_rect(y + 1), entry, focused, theme);
                mouse.click(
                    Rect::new(inner.x, y, inner.width, 2),
                    RegionId::AddPluginCard(i),
                    Message::AddPluginSelectAt(i),
                );
                advance(&mut y);
                advance(&mut y);
            }
        }
        AddPluginStep::Options => {
            let id = dialog.selected_entry().map(|e| e.id.as_str()).unwrap_or("");
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Optional features",
                        Style::default()
                            .fg(theme.accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  ·  {id}"), Style::default().fg(theme.muted())),
                ])),
                row_rect(y),
            );
            advance(&mut y);
            if dialog.features.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        "No optional features — Apply adds the base dependency.",
                        Style::default().fg(theme.muted()),
                    )),
                    row_rect(y),
                );
                advance(&mut y);
            } else {
                for (i, feature) in dialog.features.iter().enumerate() {
                    if y + 1 >= inner.bottom() {
                        break;
                    }
                    let focused = dialog.feature_cursor == i;
                    render_feature(frame, row_rect(y), row_rect(y + 1), feature, focused, theme);
                    mouse.click(
                        Rect::new(inner.x, y, inner.width, 2),
                        RegionId::AddPluginFeature(i),
                        Message::AddPluginToggleFeatureAt(i),
                    );
                    advance(&mut y);
                    advance(&mut y);
                }
            }
        }
        AddPluginStep::Applying => {
            let id = dialog.selected_entry().map(|e| e.id.as_str()).unwrap_or("");
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{25cc} ", Style::default().fg(theme.warn())),
                    Span::styled(format!("Applying {id}…"), Style::default().fg(theme.fg())),
                ])),
                row_rect(y),
            );
            advance(&mut y);
        }
        AddPluginStep::Report => {
            render_report(frame, dialog, inner, &mut y, theme);
        }
        AddPluginStep::Error => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("\u{2717} ", Style::default().fg(theme.error())),
                    Span::styled(
                        dialog.error.clone().unwrap_or_default(),
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

    // Pad down to the button row so the buttons/hint sit at the bottom
    // regardless of body height.
    let buttons_y = inner.bottom().saturating_sub(2);
    y = y.max(inner.y).min(buttons_y);
    while y < buttons_y {
        advance(&mut y);
    }

    if y < inner.bottom() {
        render_buttons(frame, mouse, row_rect(y), inner.x, dialog, theme);
        advance(&mut y);
    }
    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                step_hint(dialog.step),
                Style::default().fg(theme.muted()),
            )),
            row_rect(y),
        );
    }
}

/// The inner content height (rows) a given step needs — the modal box is this
/// plus the border and top padding.
fn inner_rows(dialog: &AddPluginDialog) -> u16 {
    let body = match dialog.step {
        AddPluginStep::Select => 1 + dialog.entries.len() as u16 * 2,
        AddPluginStep::Options => {
            1 + (dialog.features.len().max(1) as u16)
                * if dialog.features.is_empty() { 1 } else { 2 }
        }
        AddPluginStep::Applying => 1,
        AddPluginStep::Report => {
            let items = dialog.report.as_ref().map(|r| r.items.len()).unwrap_or(0);
            // header + items + a manual-notes line
            1 + items as u16 + 1
        }
        AddPluginStep::Error => 2,
    };
    // body + blank + buttons + hint
    body + 1 + 1 + 1
}

/// One plugin selection card: a marker/id line and a dim summary (or the
/// disabled reason) line beneath it — mirrors the create wizard's arch card.
fn render_entry(
    frame: &mut Frame,
    label_rect: Rect,
    desc_rect: Rect,
    entry: &PluginEntry,
    focused: bool,
    theme: &Theme,
) {
    let marker = if focused { "\u{25b8} " } else { "  " };
    let check = if focused { "(o) " } else { "( ) " };
    let label_style = if !entry.enabled {
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
            Span::styled(entry.id.clone(), label_style),
        ])),
        label_rect,
    );
    let (text, color) = match &entry.disabled_reason {
        Some(reason) => (format!("\u{2717} {reason}"), theme.warn()),
        None => (entry.summary.clone(), theme.muted()),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("      {text}"),
            Style::default().fg(color),
        )),
        desc_rect,
    );
}

/// One optional-feature checkbox row: a `[✓]`/`[ ]` box + feature id, and a dim
/// summary line (what it will add) beneath it.
fn render_feature(
    frame: &mut Frame,
    box_rect: Rect,
    desc_rect: Rect,
    feature: &FeatureToggle,
    focused: bool,
    theme: &Theme,
) {
    let marker = if focused { "\u{25b8} " } else { "  " };
    let check = if feature.selected {
        "[\u{2713}] "
    } else {
        "[ ] "
    };
    let check_style = if feature.selected {
        Style::default().fg(theme.success())
    } else {
        Style::default().fg(theme.accent())
    };
    let id_style = if focused {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(theme.accent())),
            Span::styled(check.to_string(), check_style),
            Span::styled(feature.id.clone(), id_style),
        ])),
        box_rect,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("      {}", feature.summary),
            Style::default().fg(theme.muted()),
        )),
        desc_rect,
    );
}

/// The per-edit report: a `<applied>/<already>` header, one line per edit
/// (Applied ✓ / AlreadyPresent •), and a muted manual-notes reminder.
fn render_report(
    frame: &mut Frame,
    dialog: &AddPluginDialog,
    inner: Rect,
    y: &mut u16,
    theme: &Theme,
) {
    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let Some(report) = &dialog.report else {
        return;
    };
    let (applied, already) = report.counts();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{2713} ", Style::default().fg(theme.success())),
            Span::styled(
                format!("Added {}", report.plugin_id),
                Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {applied} applied, {already} already present"),
                Style::default().fg(theme.muted()),
            ),
        ])),
        row_rect(*y),
    );
    *y += 1;

    for item in &report.items {
        if *y + 1 >= inner.bottom() {
            break;
        }
        let (glyph, color) = match item.outcome {
            AddOutcome::Applied => ("\u{2713}", theme.success()),
            AddOutcome::AlreadyPresent => ("\u{2022}", theme.muted()),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("  {glyph} "), Style::default().fg(color)),
                Span::styled(item.description.clone(), Style::default().fg(theme.fg())),
            ])),
            row_rect(*y),
        );
        *y += 1;
    }

    if *y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "See the plugin README for any remaining manual steps.",
                Style::default().fg(theme.muted()),
            )),
            row_rect(*y),
        );
        *y += 1;
    }
}

/// Render the Back / primary / Cancel buttons and register their click regions.
fn render_buttons(
    frame: &mut Frame,
    mouse: &mut MouseCtx,
    rect: Rect,
    x0: u16,
    dialog: &AddPluginDialog,
    theme: &Theme,
) {
    // The primary action label per step (`None` while applying — no primary).
    let primary = match dialog.step {
        AddPluginStep::Select => Some(" Next "),
        AddPluginStep::Options => Some(" Apply "),
        AddPluginStep::Error => Some(" Retry "),
        AddPluginStep::Report => Some(" Done "),
        AddPluginStep::Applying => None,
    };
    // Back is meaningful except while applying and on the report (which only
    // offers Done + Close).
    let back_enabled = !matches!(dialog.step, AddPluginStep::Applying | AddPluginStep::Report);

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
            RegionId::AddPluginBack,
            Message::AddPluginBack,
        );
    }
    x += back_label.chars().count() as u16 + 2;

    if let Some(primary_label) = primary {
        spans.push(Span::styled(primary_label, primary_style));
        spans.push(Span::raw("  "));
        mouse.click(
            Rect::new(x, rect.y, primary_label.chars().count() as u16, 1),
            RegionId::AddPluginApply,
            Message::AddPluginAdvance,
        );
        x += primary_label.chars().count() as u16 + 2;
    }

    let cancel_label = " Cancel ";
    spans.push(Span::styled(cancel_label, cancel_style));
    mouse.click(
        Rect::new(x, rect.y, cancel_label.chars().count() as u16, 1),
        RegionId::AddPluginCancel,
        Message::CloseAddPlugin,
    );

    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
}

/// The per-step keyhint line.
fn step_hint(step: AddPluginStep) -> &'static str {
    match step {
        AddPluginStep::Select => "↑/↓ choose · Enter next · Esc close",
        AddPluginStep::Options => "↑/↓ move · Space toggle · Enter apply · Esc back",
        AddPluginStep::Applying => "Applying…",
        AddPluginStep::Report => "Enter/Esc close",
        AddPluginStep::Error => "Enter retry · Esc back",
    }
}
