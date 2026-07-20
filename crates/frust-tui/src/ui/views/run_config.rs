//! The run-config modal (PLAN D5/D6b): a shadowed, centered popup over the
//! workbench with the target checklist + `BuildInfo` funnel (mode / flavor /
//! defines) and launch / cancel actions. The base workbench layer is rendered
//! with a *suppressed* `MouseCtx` (see `crate::ui::render`), so only this
//! modal's regions are live while it is open — the D4 base-layer suppression.
//!
//! Layering (D2): renders `&RunConfig` and only *registers* interaction; it
//! never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Alignment, Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{Message, RegionId, RunConfig, RunFocus};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use frust_drive::build_info::BuildMode;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 62;

/// Render the run-config modal centered over `area`, registering its
/// interactive regions through `mouse` (which is the *live* ctx — the base
/// layer beneath was drawn suppressed).
pub fn render(
    frame: &mut Frame,
    area: Rect,
    modal: &RunConfig,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    // wordmark(1)+blank(1)+"Targets:"(1)+targets+blank(1)+mode(1)+flavor(1)+
    // defines(1)+blank(1)+buttons(1)+hint(1), plus the border (2).
    let inner_rows = modal.targets.len() as u16 + 9;
    let height = (inner_rows + 2).min(area.height);
    let box_ = centered(area, MODAL_WIDTH, height);

    // Clear the popup footprint (and one row/col of shadow) so the workbench
    // behind it doesn't bleed through.
    frame.render_widget(Clear, box_);

    let project = modal
        .project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| modal.project_root.to_string_lossy().into_owned());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            format!(" Run · {project} "),
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::default().bg(theme.surface()))
        .shadow(Shadow::dark_shade().offset(Offset::new(2, 1)));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    // A running cursor down the inner area; each interactive row registers its
    // own single-row click rect.
    let mut y = inner.y;
    let row_rect = |y: u16| Rect::new(inner.x, y, inner.width, 1);
    let advance = |y: &mut u16| {
        *y += 1;
    };

    // ── Targets checklist ────────────────────────────────────────────────
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Targets",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )),
        row_rect(y),
    );
    advance(&mut y);

    for (i, target) in modal.targets.iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let focused = modal.focus == RunFocus::Target(i);
        let check = if target.selected { "[x]" } else { "[ ]" };
        let marker = if focused { "\u{25b8} " } else { "  " };
        let name_style = if focused {
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
        } else if target.selected {
            Style::default().fg(theme.fg())
        } else {
            Style::default().fg(theme.muted())
        };
        let line = Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(theme.accent())),
            Span::styled(
                format!("{check} "),
                Style::default().fg(if target.selected {
                    theme.success()
                } else {
                    theme.muted()
                }),
            ),
            Span::styled(target.label.clone(), name_style),
        ]);
        frame.render_widget(Paragraph::new(line), row_rect(y));
        mouse.click(
            row_rect(y),
            RegionId::RunTargetRow(i),
            Message::RunConfigToggleTargetAt(i),
        );
        advance(&mut y);
    }

    advance(&mut y); // blank

    // ── Mode selector ────────────────────────────────────────────────────
    let mode_focused = modal.focus == RunFocus::Mode;
    let mode_line = Line::from(vec![
        field_label("Mode", mode_focused, theme),
        Span::styled(
            format!("\u{2039} {} \u{203a}", mode_label(modal.mode)),
            if mode_focused {
                Style::default()
                    .fg(theme.bg())
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg())
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(mode_line), row_rect(y));
    mouse.click(
        row_rect(y),
        RegionId::RunModeRow,
        Message::RunConfigCycleMode(1),
    );
    advance(&mut y);

    // ── Flavor field ─────────────────────────────────────────────────────
    render_field(
        frame,
        mouse,
        row_rect(y),
        "Flavor",
        &modal.flavor,
        "(none)",
        modal.focus == RunFocus::Flavor,
        RegionId::RunFlavorRow,
        Message::RunConfigFocus(RunFocus::Flavor),
        theme,
    );
    advance(&mut y);

    // ── Defines field ────────────────────────────────────────────────────
    render_field(
        frame,
        mouse,
        row_rect(y),
        "Defines",
        &modal.defines,
        "KEY=VALUE …",
        modal.focus == RunFocus::Defines,
        RegionId::RunDefinesRow,
        Message::RunConfigFocus(RunFocus::Defines),
        theme,
    );
    advance(&mut y);

    advance(&mut y); // blank

    // ── Launch / Cancel buttons ──────────────────────────────────────────
    let launch_focused = modal.focus == RunFocus::Launch;
    let n = modal.targets.iter().filter(|t| t.selected).count();
    let launch_label = format!(" Launch ({n}) ");
    let launch_style = if launch_focused {
        Style::default()
            .fg(theme.bg())
            .bg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.bg())
            .bg(theme.success())
            .add_modifier(Modifier::BOLD)
    };
    let cancel_style = Style::default().fg(theme.fg()).bg(theme.overlay());
    let buttons = Line::from(vec![
        Span::styled(launch_label.clone(), launch_style),
        Span::raw("   "),
        Span::styled(" Cancel ", cancel_style),
    ]);
    frame.render_widget(Paragraph::new(buttons), row_rect(y));
    // Click rects for the two buttons (approximate widths from their labels).
    let launch_w = launch_label.chars().count() as u16;
    mouse.click(
        Rect::new(inner.x, y, launch_w, 1),
        RegionId::RunLaunch,
        Message::RunConfigLaunch,
    );
    let cancel_x = inner.x + launch_w + 3;
    mouse.click(
        Rect::new(cancel_x, y, 8, 1),
        RegionId::RunCancel,
        Message::CloseRunConfig,
    );
    advance(&mut y);

    // ── Keyhint row ──────────────────────────────────────────────────────
    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Space toggle · ←/→ mode · Tab next · Enter launch · Esc cancel",
                Style::default().fg(theme.muted()),
            ))
            .alignment(Alignment::Left),
            row_rect(y),
        );
    }
}

/// A field label span, brightened when the field is focused.
fn field_label(label: &str, focused: bool, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!("{label:<9}"),
        if focused {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted())
        },
    )
}

/// Render a labeled text field row (flavor/defines) and register its click
/// (focus) region.
#[allow(clippy::too_many_arguments)]
fn render_field(
    frame: &mut Frame,
    mouse: &mut MouseCtx,
    rect: Rect,
    label: &str,
    value: &str,
    placeholder: &str,
    focused: bool,
    id: RegionId,
    on_click: Message,
    theme: &Theme,
) {
    let value_span = if value.is_empty() {
        Span::styled(placeholder.to_string(), Style::default().fg(theme.muted()))
    } else {
        Span::styled(value.to_string(), Style::default().fg(theme.fg()))
    };
    let mut spans = vec![field_label(label, focused, theme), value_span];
    if focused {
        spans.push(Span::styled(
            "\u{2588}",
            Style::default().fg(theme.accent()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    mouse.click(rect, id, on_click);
}

/// The display label for a build mode.
fn mode_label(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}
