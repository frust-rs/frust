//! The right-click context-menu popup (T04 / PLAN D4): a small bordered menu
//! anchored near the click, rendered on the top z-layer over the (still
//! visible) workbench. The base layer is drawn with a *suppressed* `MouseCtx`
//! (see `crate::ui::render`), so only the menu's own rows are hit-testable
//! while it's open.
//!
//! Layering (D2): renders `&ContextMenu` (built in `crate::engine::context_menu`
//! from the right-clicked target) and only *registers* interaction — it never
//! mutates the engine. Every entry re-dispatches an existing `Message`; the
//! entry → keyboard/palette parity table lives in that engine module.

use ratatui::Frame;
use ratatui::layout::{Alignment, Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Shadow};

use crate::engine::{ContextMenu, Message, RegionId};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Render `menu` as a popup anchored at its preferred `(x, y)`, clamped so the
/// whole box stays inside `area`. Highlights the cursor row and registers each
/// enabled row as a click target.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    menu: &ContextMenu,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let w = menu.width().min(area.width);
    let h = menu.height().min(area.height);
    if w == 0 || h == 0 {
        return;
    }
    // Anchor at the click, clamped so the box never spills off the frame.
    let x = menu.x.min(area.right().saturating_sub(w)).max(area.x);
    let y = menu.y.min(area.bottom().saturating_sub(h)).max(area.y);
    let box_ = Rect::new(x, y, w, h);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(1, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    for (i, entry) in menu.entries.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let is_selected = i == menu.cursor;
        if is_selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.overlay())),
                rect,
            );
        }

        let marker = if is_selected { "\u{25b8} " } else { "  " };
        let label_style = if !entry.enabled {
            Style::default().fg(theme.muted())
        } else if is_selected {
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.accent())),
                Span::styled(entry.label.to_string(), label_style),
            ])),
            rect,
        );

        if !entry.hint.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("{} ", entry.hint),
                    Style::default().fg(theme.muted()),
                )))
                .alignment(Alignment::Right),
                rect,
            );
        }

        // An enabled row is clickable (and hover-highlights); a disabled row
        // registers hover only (so the highlight still follows the pointer),
        // its activation gated in `update`.
        if entry.enabled {
            mouse.menu_item(rect, i, Message::ContextMenuActivateAt(i));
        } else {
            mouse.hover(rect, RegionId::ContextMenuItem(i));
        }
    }
}
