//! The Add Plugin dialog modal: a shadowed popup centered in the shell's body
//! band over the welcome/workbench base layer, stepping select-plugin →
//! toggle-features → an off-thread apply → a per-edit report
//! (or an error). The base layer is rendered with a *suppressed* `MouseCtx`
//! (see `crate::ui::render`), so only this modal's regions are live while it is
//! open — the base-layer suppression, plus the binding `[Esc] Close` title
//! affordance.
//!
//! The registry outgrew the modal, so the select step paints a cursor-following
//! window over the cards (`visible_entries`/`scroll_offset`) rather than the
//! whole list: the window is derived from this frame's geometry, never
//! persisted, and the *engine's* cursor still walks all of them.
//!
//! Layering: renders `&AddPluginDialog` and only *registers* interaction;
//! it never mutates the engine. Cloned in shape from `views::create_wizard`.

use frust_drive::plugin::AddOutcome;
use ratatui::Frame;
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow, Wrap};

use crate::engine::{
    AddPluginDialog, AddPluginStep, FeatureToggle, Message, PluginEntry, RegionId,
};
use crate::ui::layout::{Shell, centered};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// Modal width (columns) — matches the create wizard's `MODAL_WIDTH`.
const MODAL_WIDTH: u16 = 64;

/// The `[Esc] Close` title affordance text (a binding for new modals).
const CLOSE_LABEL: &str = " [Esc] Close ";

/// Rows the block's drop [`Shadow`] paints *below* the box (its `Offset`'s
/// `y`) — reserved on top of the box's own height so a full-height dialog's
/// shadow lands on the last body row instead of the status bar.
const SHADOW_ROWS: u16 = 1;

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
    // Chrome-safe placement: the dialog is both *clamped to* and *centered
    // inside* the shell's body band, never the whole frame. Clamping alone is
    // not enough — the shell's chrome is asymmetric (a 3-row titlebar, a 1-row
    // status bar) while `centered` splits the leftover rows evenly, so a
    // frame-centered box tall enough to fill the body still lands a row over
    // the titlebar. Deriving the band from `Shell::split` covers the welcome
    // base layer too: its own chrome is a subset of the workbench's, so this is
    // at worst one row conservative there and never an overdraw.
    let body = Shell::split(area).body;
    let height = inner_rows(dialog)
        .saturating_add(3)
        .min(body.height.saturating_sub(SHADOW_ROWS));
    let box_ = centered(body, MODAL_WIDTH.min(area.width), height);
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
            // The scroll window is derived from this frame's geometry rather
            // than persisted (matching the rest of this view), so the engine's
            // wrapping cursor stays the single source of truth.
            let visible = visible_entries(inner.height);
            let offset = scroll_offset(dialog.cursor, dialog.entries.len(), visible);
            let mut header = vec![Span::styled(
                "Choose a plugin to add to this project",
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            )];
            if dialog.entries.len() > visible {
                // Only meaningful when the list actually scrolls: which slice
                // of the registry the cards below are.
                let first = offset + 1;
                let last = (offset + visible).min(dialog.entries.len());
                header.push(Span::styled(
                    format!("  ({first}-{last} of {})", dialog.entries.len()),
                    Style::default().fg(theme.muted()),
                ));
            }
            frame.render_widget(Paragraph::new(Line::from(header)), row_rect(y));
            advance(&mut y);
            for (i, entry) in dialog.entries.iter().enumerate().skip(offset).take(visible) {
                // Reserve the trailing 2 rows for the button row + hint line
                // unconditionally force-appended after this loop (the "Pad
                // down to the button row" block below) — without this an
                // entry can render into what becomes that row, and the two
                // draws overlap in the terminal buffer (a shorter later
                // line never clears a longer prior one). `visible_entries`
                // already sizes the window to the same reservation; this stays
                // as the backstop for an inner box too short to hold even the
                // one card that function's `max(1)` floor asks for.
                if y + 1 >= inner.bottom().saturating_sub(2) {
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

/// How many two-row selection cards fit in an `inner_height`-row modal body:
/// the "Choose a plugin…" header row plus the two rows the button/hint block
/// reserves at the bottom come off first, and what is left splits in two.
///
/// Floored at 1 so a degenerate box still tries to draw the focused card — the
/// paint loop's own bottom guard drops it when even that does not fit.
fn visible_entries(inner_height: u16) -> usize {
    (usize::from(inner_height.saturating_sub(3)) / 2).max(1)
}

/// The absolute index of the first entry drawn, for a list of `len` entries
/// showing `visible` at a time with the selection on `cursor`.
///
/// The window is pushed just far enough down that `cursor` is its last row,
/// then clamped so it never runs past the end of the list — so the focused card
/// is drawn for *every* cursor position
/// [`crate::engine::AddPluginDialog::select_move`]'s wrap can produce, without
/// the render layer constraining the cursor (which would make the entries past
/// the window keyboard-unreachable).
fn scroll_offset(cursor: usize, len: usize, visible: usize) -> usize {
    cursor
        .saturating_sub(visible.saturating_sub(1))
        .min(len.saturating_sub(visible))
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
    // Wrapped rather than hard-clipped: `desc_rect` is one row, so this shows
    // the same leading slice either way, but on a whole-word boundary instead
    // of mid-word (`trim: false` keeps the six-column indent that aligns the
    // summary under the id). A registry summary longer than the modal's 64
    // columns still loses its tail — see this view's card layout, which is
    // fixed at two rows per entry.
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("      {text}"),
            Style::default().fg(color),
        ))
        .wrap(Wrap { trim: false }),
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

/// The report header's `<applied> applied, <already> already present[, <n>
/// applied at build]` clause — built from
/// [`frust_drive::plugin::AddReport::outcome_counts`]'s three-bucket shape
/// rather than [`frust_drive::plugin::AddReport::counts`]'s coarser
/// two-bucket one: that method's second bucket lumps
/// [`AddOutcome::AlreadyPresent`] and [`AddOutcome::AppliedAtBuild`]
/// together, and this view must not caption a desktop-lane item (never
/// written to a project file, applied fresh at every `frust build <os>`) as
/// "already present". The trailing clause is only appended when the report
/// actually carries an `AppliedAtBuild` item — a pure, standalone function so
/// the counting rule is unit-tested without a render pass and its
/// column-width constraints. This is the one counting rule the codebase
/// shares — `crate::engine::update`'s `AddPluginSucceeded` toast sites build
/// their text from the same `outcome_counts()` call.
fn report_summary_line(report: &frust_drive::plugin::AddReport) -> String {
    let (applied, already_present, applied_at_build) = report.outcome_counts();
    let mut summary = format!("{applied} applied, {already_present} already present");
    if applied_at_build > 0 {
        summary.push_str(&format!(", {applied_at_build} applied at build"));
    }
    summary
}

/// The per-edit report: a summary header ([`report_summary_line`]), one line
/// per edit (Applied ✓ / AlreadyPresent • / AppliedAtBuild »), and a muted
/// manual-notes reminder.
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
    let summary = report_summary_line(report);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{2713} ", Style::default().fg(theme.success())),
            Span::styled(
                format!("Added {}", report.plugin_id),
                Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {summary}"),
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
            // Distinct from both Applied (✓) and AlreadyPresent (•): this
            // edit was never made to a project file at all — it is recorded
            // and applied fresh by every `frust build <os>` instead.
            AddOutcome::AppliedAtBuild => ("\u{00bb}", theme.accent()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::plugin::{AddItem, AddReport};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use std::path::PathBuf;

    use crate::ui::mouse::MouseRegions;

    /// Render `dialog` alone (no base workbench layer beneath it) and return
    /// the cell grid as a plain string — enough to assert a specific glyph
    /// landed on a specific report row without pulling in the whole
    /// `AppState`/`frust_tui::ui::render` pipeline `tests/snapshots.rs`
    /// exercises for the dialog's other steps.
    fn render_dialog_to_string(dialog: &AddPluginDialog) -> String {
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = Theme::frust_dark();
        let mut regions = MouseRegions::new();
        terminal
            .draw(|frame| {
                let mut ctx = MouseCtx::new(&mut regions);
                let area = frame.area();
                render(frame, area, dialog, &theme, &mut ctx);
            })
            .expect("draw");
        let buf = terminal.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell(Position::new(x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    /// The select step's scroll window keeps the focused card drawn for every
    /// cursor the engine's *wrapping*
    /// [`crate::engine::AddPluginDialog::select_move`] can produce — the
    /// property that stops the cursor parking on an entry with no visible
    /// focus marker and no mouse region.
    #[test]
    fn the_scroll_window_always_contains_the_cursor() {
        for visible in 1..=13usize {
            for len in 1..=13usize {
                for cursor in 0..len {
                    let offset = scroll_offset(cursor, len, visible);
                    let last = offset + visible.min(len) - 1;
                    assert!(
                        (offset..=last).contains(&cursor),
                        "cursor {cursor} outside window {offset}..={last} \
                         (len {len}, visible {visible})"
                    );
                }
            }
        }
    }

    /// The window never scrolls past the end of the list (so the last screenful
    /// stays full) and never scrolls at all while everything fits.
    #[test]
    fn the_scroll_window_is_clamped_to_the_list() {
        // 13 registry entries, 9 rows of cards: the window pins at 4 once the
        // cursor reaches the tail, and cupertino (12) is the last row drawn.
        assert_eq!(scroll_offset(0, 13, 9), 0);
        assert_eq!(scroll_offset(8, 13, 9), 0);
        assert_eq!(scroll_offset(9, 13, 9), 1);
        assert_eq!(scroll_offset(12, 13, 9), 4);
        // Fewer entries than rows: no scrolling at all.
        assert_eq!(scroll_offset(2, 3, 9), 0);
    }

    /// [`visible_entries`]'s accounting: one header row, two reserved footer
    /// rows, two rows per card — and a floor of one card.
    #[test]
    fn visible_entries_reserves_the_header_and_footer_rows() {
        // The 100x30 workbench case: a 22-row inner box shows 9 cards.
        assert_eq!(visible_entries(22), 9);
        assert_eq!(visible_entries(5), 1);
        assert_eq!(visible_entries(3), 1);
        assert_eq!(visible_entries(0), 1);
    }

    /// The report step renders a distinct `»` glyph for
    /// [`AddOutcome::AppliedAtBuild`] — never `✓` (Applied) or `•`
    /// (AlreadyPresent) — and the header's "applied at build" clause only
    /// appears when the report actually carries one (see `render_report`'s
    /// own doc comment for why it can't just reuse `AddReport::counts()`'s
    /// two-bucket shape here).
    #[test]
    fn report_step_renders_a_distinct_glyph_for_applied_at_build() {
        let mut dialog = AddPluginDialog::new(PathBuf::from("/tmp/example"));
        dialog.succeed(AddReport {
            plugin_id: "desktop-test-plugin".to_string(),
            items: vec![
                AddItem {
                    description: "Cargo.toml dependency `frust-desktop-test-plugin`".to_string(),
                    outcome: AddOutcome::Applied,
                },
                AddItem {
                    description:
                        "Info.plist key `NSSupportsSuddenTermination` (applied at `frust build macos`)"
                            .to_string(),
                    outcome: AddOutcome::AppliedAtBuild,
                },
            ],
        });

        let rendered = render_dialog_to_string(&dialog);
        assert!(
            rendered.contains("\u{00bb} Info.plist key"),
            "expected the » glyph on the AppliedAtBuild row:\n{rendered}"
        );
        assert!(
            !rendered.contains("\u{2022} Info.plist key")
                && !rendered.contains("\u{2713} Info.plist key"),
            "the AppliedAtBuild row must not carry the Applied/AlreadyPresent glyphs:\n{rendered}"
        );
    }

    /// [`report_summary_line`]'s counting rule, standalone: the trailing
    /// "applied at build" clause appears only when the report carries an
    /// `AppliedAtBuild` item, with the right count — the header truthfulness
    /// contract `render_report`'s doc comment describes. Checked as a pure
    /// function rather than through a render pass, since the modal's fixed
    /// 64-column width truncates a header this long before it ever reaches
    /// the "already present"/"applied at build" clauses on screen.
    #[test]
    fn report_summary_line_includes_the_applied_at_build_clause_only_when_present() {
        let mixed = AddReport {
            plugin_id: "x".to_string(),
            items: vec![
                AddItem {
                    description: "a".to_string(),
                    outcome: AddOutcome::Applied,
                },
                AddItem {
                    description: "b".to_string(),
                    outcome: AddOutcome::AlreadyPresent,
                },
                AddItem {
                    description: "c".to_string(),
                    outcome: AddOutcome::AppliedAtBuild,
                },
            ],
        };
        assert_eq!(
            report_summary_line(&mixed),
            "1 applied, 1 already present, 1 applied at build"
        );

        let none_at_build = AddReport {
            plugin_id: "x".to_string(),
            items: vec![
                AddItem {
                    description: "a".to_string(),
                    outcome: AddOutcome::Applied,
                },
                AddItem {
                    description: "b".to_string(),
                    outcome: AddOutcome::AlreadyPresent,
                },
            ],
        };
        assert_eq!(
            report_summary_line(&none_at_build),
            "1 applied, 1 already present"
        );
    }

    /// A report with no `AppliedAtBuild` items must not grow the "applied at
    /// build" clause — the header stays the original two-bucket text.
    #[test]
    fn report_step_omits_applied_at_build_clause_when_absent() {
        let mut dialog = AddPluginDialog::new(PathBuf::from("/tmp/example"));
        dialog.succeed(AddReport {
            plugin_id: "secure-storage".to_string(),
            items: vec![
                AddItem {
                    description: "Cargo.toml dependency `frust-secure-storage`".to_string(),
                    outcome: AddOutcome::Applied,
                },
                AddItem {
                    description: "Info.plist key `NSFaceIDUsageDescription`".to_string(),
                    outcome: AddOutcome::AlreadyPresent,
                },
            ],
        });

        let rendered = render_dialog_to_string(&dialog);
        assert!(
            rendered.contains("1 applied, 1 already present")
                && !rendered.contains("applied at build"),
            "{rendered}"
        );
    }
}
