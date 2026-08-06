//! The toolchain bootstrap wizard: a shadowed,
//! centered two-pane modal — a left step tree (Prerequisites → Platforms ▾ →
//! Doctor rollup, fdemon's collapsed/expanded projection) and a right detail
//! pane listing the selected area's components + its guided fix commands, each
//! either copy-to-clipboard or "Run in session". The base workbench/welcome
//! layer is rendered with a *suppressed* `MouseCtx` (see `crate::ui::render`),
//! so only this panel's regions are live while it's open — the base-layer
//! suppression, plus the binding `[Esc] Close` title affordance.
//!
//! Layering: renders `&BootstrapWizard` and only *registers* interaction;
//! it never mutates the engine. fdemon's InstallWizard is a PATTERN source only
//! (BSL-1.1 — no verbatim copies).

use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{BootstrapNode, BootstrapWizard, Message, RegionId};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use frust_drive::doctor::ComponentStatus;

/// Modal width (columns) and left step-column width.
const MODAL_WIDTH: u16 = 74;
const MODAL_HEIGHT: u16 = 20;
const STEPS_WIDTH: u16 = 22;

/// Render the bootstrap wizard centered over `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    wizard: &BootstrapWizard,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let box_ = centered(area, MODAL_WIDTH, MODAL_HEIGHT);
    frame.render_widget(Clear, box_);

    let StatusChip(roll_glyph, _, roll_color) = status_chip(wizard.rollup(), theme);
    let title = Line::from(vec![
        Span::styled(
            format!(" {} Toolchain setup ", theme.icons.gear()),
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "— get this machine building Frust apps ",
            Style::default().fg(theme.muted()),
        ),
        Span::styled(format!("{roll_glyph} "), Style::default().fg(roll_color)),
    ]);
    let close_title = Line::from(Span::styled(
        " [Esc] Close ",
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

    // The `[Esc] Close` title affordance is clickable (binding for new modals).
    let close_w = " [Esc] Close ".chars().count() as u16;
    if close_w < box_.width {
        mouse.click(
            Rect::new(box_.right().saturating_sub(close_w + 1), box_.y, close_w, 1),
            RegionId::BootstrapClose,
            Message::CloseBootstrapWizard,
        );
    }

    if wizard.report.areas.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "\u{25cc} running toolchain preflight…",
                Style::default().fg(theme.muted()),
            )),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    }

    // Split inner into the left step column and the right detail pane, leaving
    // the last row for the footer hint.
    let content_h = inner.height.saturating_sub(1);
    let left = Rect::new(inner.x, inner.y, STEPS_WIDTH.min(inner.width), content_h);
    let right_x = inner.x + STEPS_WIDTH + 1;
    let right = if right_x < inner.right() {
        Rect::new(right_x, inner.y, inner.right() - right_x, content_h)
    } else {
        Rect::new(inner.x, inner.y, 0, 0)
    };

    render_steps(frame, left, wizard, theme, mouse);
    render_detail(frame, right, wizard, theme, mouse);

    // Footer keyhint row.
    let footer = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "\u{2191}\u{2193} steps · Space expand · Tab fix · Enter/r run · c copy · Esc close",
            Style::default().fg(theme.muted()),
        )),
        footer,
    );
}

/// The left step tree: one glyph+label row per visible node, the selected row
/// highlighted, each row a click target selecting it (a `Platforms` header
/// click toggles expansion).
fn render_steps(
    frame: &mut Frame,
    area: Rect,
    wizard: &BootstrapWizard,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.height == 0 {
        return;
    }
    let mut y = area.y;
    frame.render_widget(
        Paragraph::new(Line::styled(
            "STEPS",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(area.x, y, area.width, 1),
    );
    y += 1;

    let nodes = wizard.nodes();
    let selected = wizard.cursor.min(nodes.len() - 1);
    for (i, node) in nodes.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }
        let (glyph, gcolor) = status_chip(wizard.node_status(*node), theme).into_glyph();
        let (indent, label) = node_label(*node, wizard, theme);
        let is_sel = i == selected;
        let label_style = if is_sel {
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted())
        };
        let marker = if is_sel { "\u{25b8}" } else { " " };
        let line = Line::from(vec![
            Span::styled(
                format!("{marker}{indent}"),
                Style::default().fg(theme.accent()),
            ),
            Span::styled(format!("{glyph} "), Style::default().fg(gcolor)),
            Span::styled(label, label_style),
        ]);
        let row = Rect::new(area.x, y, area.width, 1);
        if is_sel {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.overlay())),
                row,
            );
        }
        frame.render_widget(Paragraph::new(line), row);
        mouse.click(
            row,
            RegionId::BootstrapStep(i),
            Message::BootstrapSelectStep(i),
        );
        y += 1;
    }
}

/// The right detail pane: the selected area's header + component list, then a
/// "FIX IT" section with the guided commands (the selected one highlighted with
/// a runnable/guidance affordance).
fn render_detail(
    frame: &mut Frame,
    area: Rect,
    wizard: &BootstrapWizard,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut y = area.y;
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);

    // Header: the current node's area name + status word (or a summary for the
    // header/rollup nodes).
    let (header, header_status) = detail_header(wizard);
    let StatusChip(_, word, hcolor) = status_chip(header_status, theme);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                header,
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" — {word}"), Style::default().fg(hcolor)),
        ])),
        row(y),
    );
    y += 1;

    // Component list.
    for comp in wizard.detail_components() {
        if y >= area.bottom() {
            break;
        }
        let (glyph, gcolor) = status_chip(comp.status, theme).into_glyph();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(gcolor)),
                Span::styled(comp.name.clone(), Style::default().fg(theme.fg())),
                Span::styled(
                    format!("  {}", truncate(&comp.summary, summary_width(area.width))),
                    Style::default().fg(theme.muted()),
                ),
            ])),
            row(y),
        );
        y += 1;
    }

    let fixes = wizard.current_fixes();
    if fixes.is_empty() {
        return;
    }

    y += 1; // blank
    if y < area.bottom() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "FIX IT",
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " — one command per confirm",
                    Style::default().fg(theme.muted()),
                ),
            ])),
            row(y),
        );
        y += 1;
    }

    let sel = wizard.fix_cursor.min(fixes.len() - 1);
    for (i, (_, fix)) in fixes.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }
        let is_sel = i == sel;
        let marker = if is_sel { "\u{25b6} " } else { "  " };
        let text_color = if fix.auto_runnable {
            theme.fg()
        } else {
            theme.muted()
        };
        let mut style = Style::default().fg(text_color);
        if is_sel {
            style = style.add_modifier(Modifier::BOLD);
        }
        let r = row(y);
        if is_sel {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.overlay())),
                r,
            );
        }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.accent())),
                Span::styled(
                    truncate(&fix.display, area.width.saturating_sub(4) as usize),
                    style,
                ),
            ])),
            r,
        );
        mouse.click(r, RegionId::BootstrapFix(i), Message::BootstrapSelectFix(i));
        y += 1;
    }

    // Action row: "▶ Run in session" for a runnable fix, else a guidance note;
    // "c copy" always.
    if y + 1 < area.bottom() {
        y += 1;
        let runnable = wizard.runnable_selected_fix().is_some();
        let run_label = " \u{25b6} Run in session ";
        let run_style = if runnable {
            Style::default()
                .fg(theme.bg())
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted()).bg(theme.overlay())
        };
        let copy_label = " c copy ";
        let spans = vec![
            Span::styled(run_label, run_style),
            Span::raw("  "),
            Span::styled(
                copy_label,
                Style::default().fg(theme.fg()).bg(theme.overlay()),
            ),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), row(y));
        let run_w = run_label.chars().count() as u16;
        if runnable {
            mouse.click(
                Rect::new(area.x, y, run_w, 1),
                RegionId::BootstrapRunFix,
                Message::BootstrapRunFix,
            );
        }
        let copy_x = area.x + run_w + 2;
        mouse.click(
            Rect::new(copy_x, y, copy_label.chars().count() as u16, 1),
            RegionId::BootstrapCopyFix,
            Message::BootstrapCopyFix,
        );
    }
}

/// The detail-pane header text + the status it colors, per the selected node.
fn detail_header(wizard: &BootstrapWizard) -> (String, ComponentStatus) {
    match wizard.current_node() {
        BootstrapNode::Core { area, .. } => (
            wizard.report.areas[area].name.clone(),
            wizard.node_status(wizard.current_node()),
        ),
        BootstrapNode::Platform { area } => (
            wizard.report.areas[area].name.to_uppercase(),
            wizard.platform_display_status(area),
        ),
        BootstrapNode::PlatformsHeader => (
            "PLATFORMS".to_string(),
            wizard.node_status(BootstrapNode::PlatformsHeader),
        ),
        BootstrapNode::Rollup => ("DOCTOR ROLLUP".to_string(), wizard.rollup()),
    }
}

/// The left-tree label + indent for a node.
fn node_label(
    node: BootstrapNode,
    wizard: &BootstrapWizard,
    _theme: &Theme,
) -> (&'static str, String) {
    match node {
        BootstrapNode::Core { area, component } => (
            "",
            wizard.report.areas[area].components[component].name.clone(),
        ),
        BootstrapNode::PlatformsHeader => {
            let chevron = if wizard.platforms_expanded {
                "\u{25be}"
            } else {
                "\u{25b8}"
            };
            ("", format!("Platforms {chevron}"))
        }
        BootstrapNode::Platform { area } => ("  ", wizard.report.areas[area].name.clone()),
        BootstrapNode::Rollup => ("", "Doctor rollup".to_string()),
    }
}

/// The status glyph + word + color for a rollup/component status.
fn status_chip(status: ComponentStatus, theme: &Theme) -> StatusChip {
    match status {
        ComponentStatus::Ok => StatusChip("\u{2713}", "ok", theme.success()),
        ComponentStatus::Partial => StatusChip("!", "partial", theme.warn()),
        ComponentStatus::Missing => StatusChip("\u{2717}", "missing", theme.error()),
    }
}

/// A small carrier so a caller can pick either the (glyph,color) pair or the
/// full (glyph,word,color) triple without two lookup fns.
struct StatusChip(&'static str, &'static str, Color);

impl StatusChip {
    fn into_glyph(self) -> (&'static str, Color) {
        (self.0, self.2)
    }
}

/// The column budget for a component summary beside its name.
fn summary_width(area_width: u16) -> usize {
    area_width.saturating_sub(24) as usize
}

/// Truncate `s` to `max` columns with an ellipsis (a rough char-count budget —
/// summaries here are ASCII toolchain strings).
fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('\u{2026}');
    out
}
