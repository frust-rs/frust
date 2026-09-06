//! The Inspector tab body (workbook §B12): the flattened widget tree on the
//! left and the selected node's props on the right — stacked instead of split
//! below [`SPLIT_WIDTH`] columns, §B12's narrow fallback.
//!
//! Everything drawn here comes off [`crate::engine::InspectorTab`]: the row
//! list is already flattened (indent, `▸`/`▾` state, child count), the props
//! cache is already matched against the selection, and the in-flight/error
//! states are already decided. This module only lays them out and registers
//! the two mouse affordances — a row click (select) and a `▸`/`▾` click
//! (expand/collapse), both with keyboard parity (`↑↓`/`j`/`k`, `→`/`←`).
//!
//! **Not implemented as drawn (§B12 honesty notes):** the mockup puts a
//! bare `r refresh` hint in the tab strip's right slot where the connection
//! badge lives; that badge is kept (it is the more load-bearing indicator)
//! and the Inspector's keyhints go in the status row instead. Tap-to-highlight
//! is explicitly out of v1 per §B12's own Notes.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use frust_devtools_protocol::RectPx;

use crate::engine::{InspectorFocus, InspectorRow, InspectorTab, Message, RegionId, SessionView};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;

/// §B12's responsive breakpoint for this tab: at or above this many columns
/// the props pane splits off to the right; below it, it stacks under the tree
/// (the same rule the workbench's own narrow fallback follows). Measured
/// against the *tab body*, which is already inset by the sidebar.
pub const SPLIT_WIDTH: u16 = 100;

/// Columns the props pane takes in the split layout — wide enough for
/// §B12's own worked example (`padding  EdgeInsets(12,12,12,12)`) to sit on
/// one row, so wrapping is the exception rather than the norm.
const PROPS_WIDTH: u16 = 40;

/// Rows the props pane takes when stacked under the tree (its rule row plus
/// the header, bounds, type and a few entries) — never more than half the
/// body, so the tree keeps the space on a short terminal.
const STACKED_PROPS_HEIGHT: u16 = 9;

/// Columns of indent per tree level.
const INDENT: usize = 2;

/// Width the props pane pads its keys to (§B12's `bounds   x12 y44 …`
/// alignment).
const PROPS_KEY_WIDTH: usize = 8;

/// Render the Inspector tab body into `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    session: &SessionView,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let inspector = &session.devtools.inspector;
    let (tree_area, props_area, vertical) = split(area);
    render_tree(frame, tree_area, inspector, theme, mouse);
    render_props(frame, props_area, inspector, theme, vertical);
}

/// Split `area` into the tree pane and the props pane, returning whether the
/// split was vertical (stacked) — the divider is drawn by the props pane
/// itself, as its first column (split) or first row (stacked).
fn split(area: Rect) -> (Rect, Rect, bool) {
    if area.width >= SPLIT_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(PROPS_WIDTH)])
            .split(area);
        (cols[0], cols[1], false)
    } else {
        let props_height = STACKED_PROPS_HEIGHT.min(area.height / 2);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(props_height)])
            .split(area);
        (rows[0], rows[1], true)
    }
}

// ── Tree pane ───────────────────────────────────────────────────────────────

/// The tree pane: one row per visible node, scrolled to keep the selection on
/// screen, with the empty/loading/failed states §B12's five connection
/// screens don't cover (they are about the *connection*; these are about the
/// snapshot).
fn render_tree(
    frame: &mut Frame,
    area: Rect,
    inspector: &InspectorTab,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let rows = inspector.rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_lines(inspector, theme)),
            Rect::new(area.x, area.y, area.width, area.height),
        );
        return;
    }

    let focused = inspector.focus == InspectorFocus::Tree;
    let height = area.height as usize;
    let offset = scroll_offset(rows.len(), inspector.selected_index(), height);
    for (i, row) in rows.iter().enumerate().skip(offset).take(height) {
        let y = area.y + (i - offset) as u16;
        let rect = Rect::new(area.x, y, area.width, 1);
        let selected = i == inspector.selected_index();
        if selected {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.overlay())),
                rect,
            );
        }
        frame.render_widget(
            Paragraph::new(tree_line(row, selected, focused, theme, area.width)),
            rect,
        );
        // Row click selects; the `▸`/`▾` cell on top of it toggles (a later
        // registration wins the hit test on the columns they share).
        mouse.click(
            rect,
            RegionId::DevtoolsInspectorRow(i),
            Message::DevtoolsInspectorSelectRow(i),
        );
        if row.has_children() {
            let twisty_x = area.x + 1 + (row.depth * INDENT) as u16;
            if twisty_x < area.right() {
                mouse.click(
                    Rect::new(twisty_x, y, 1, 1),
                    RegionId::DevtoolsInspectorTwisty(i),
                    Message::DevtoolsInspectorToggleNode(row.id),
                );
            }
        }
    }
}

/// The tree pane's message when there are no rows: whichever of "pulling",
/// "failed", "the app reported no widgets" and "not pulled yet" applies.
fn empty_lines(inspector: &InspectorTab, theme: &Theme) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.muted());
    if let Some(error) = inspector.error() {
        return vec![
            Line::from(vec![
                Span::styled(" ✗  ", Style::default().fg(theme.error())),
                Span::styled(
                    "Could not read the widget tree".to_string(),
                    Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(format!("    {error}"), muted),
            Line::styled("    r retries the pull".to_string(), muted),
        ];
    }
    if inspector.is_tree_pending() {
        return vec![Line::styled(
            " ⟳  pulling the widget tree…".to_string(),
            muted,
        )];
    }
    if inspector.is_loaded() {
        return vec![Line::styled(
            " the app reported an empty widget tree".to_string(),
            muted,
        )];
    }
    vec![Line::styled(
        " no snapshot yet — r pulls the widget tree".to_string(),
        muted,
    )]
}

/// One tree row: indent, the `▸`/`▾` affordance, the type glyph, the short
/// type name, the `debug_label`, a collapsed node's child count, and (on the
/// selected row only, §B12) its bounds pushed to the right edge.
fn tree_line(
    row: &InspectorRow,
    selected: bool,
    focused: bool,
    theme: &Theme,
    width: u16,
) -> Line<'static> {
    let name_style = if selected && focused {
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg())
    };
    let muted = Style::default().fg(theme.muted());

    let twisty = if !row.has_children() {
        " "
    } else if row.expanded {
        "▾"
    } else {
        "▸"
    };
    let mut spans = vec![
        Span::raw(format!(" {}", " ".repeat(row.depth * INDENT))),
        Span::styled(twisty.to_string(), Style::default().fg(theme.accent())),
        Span::styled(format!(" {} ", type_glyph(&row.type_name)), muted),
        Span::styled(short_type_name(&row.type_name), name_style),
    ];
    let mut used = 1 + row.depth * INDENT + 1 + 3 + short_type_name(&row.type_name).chars().count();
    if let Some(label) = &row.debug_label {
        let text = format!(" “{label}”");
        used += text.chars().count();
        spans.push(Span::styled(text, muted));
    }
    if row.has_children() && !row.expanded {
        // §B12's `ListView (3 children)` — what a collapsed node is hiding.
        let text = if row.child_count == 1 {
            " (1 child)".to_string()
        } else {
            format!(" ({} children)", row.child_count)
        };
        used += text.chars().count();
        spans.push(Span::styled(text, muted));
    }
    // Bounds ride along on the selected row only — every row would be noise,
    // and the props pane shows them for the selection anyway.
    if selected {
        let bounds = format!("{} ", fmt_bounds(row.bounds));
        let width = width as usize;
        // Drawn only with a blank column left between it and the row's text,
        // so a long type name is never visually run into its own bounds.
        if used + bounds.chars().count() < width {
            let pad = width - used - bounds.chars().count();
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(bounds, muted));
        }
    }
    Line::from(spans)
}

/// The first visible row index for a `height`-row pane, keeping `selected` on
/// screen (and never scrolling past the end).
fn scroll_offset(len: usize, selected: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    if selected < height {
        0
    } else {
        (selected + 1 - height).min(len - height)
    }
}

/// §A8's glyph vocabulary, retyped onto *our* widget names (§B12: "not
/// Flutter's"): `▦` flex, `▣` container, `T` text, `≡` scrollable. Matched on
/// the short type name with its `Widget` suffix stripped, so both the
/// retained widget (`FlexWidget`) and a bare name (`Flex`) land the same;
/// anything unrecognized — buttons, controls, app-authored widgets — draws
/// the neutral `·` rather than being force-fitted into a family.
fn type_glyph(type_name: &str) -> &'static str {
    let short = short_type_name(type_name);
    let base = short.strip_suffix("Widget").unwrap_or(&short);
    match base {
        "Flex" | "Row" | "Column" => "▦",
        "Padding" | "Container" | "SizedBox" | "Align" | "Center" | "Card" | "Stack"
        | "SafeArea" | "DecoratedBox" => "▣",
        "Text" | "RichText" | "Label" | "TextInput" => "T",
        "ListView" | "List" | "Scroll" | "ScrollView" | "GridView" => "≡",
        _ => "·",
    }
}

/// The last path segment of a `type_name` — `frust_widgets::flex::FlexWidget`
/// → `FlexWidget`. Generic arguments are kept whole and never searched for a
/// separator (`a::B<c::D>` shortens to `B<c::D>`, not `D>`).
fn short_type_name(type_name: &str) -> String {
    let head = type_name
        .find('<')
        .map(|i| &type_name[..i])
        .unwrap_or(type_name);
    let start = head.rfind("::").map(|i| i + 2).unwrap_or(0);
    type_name[start..].to_string()
}

/// §B12's `x12 y44 w360 h220`, or a dim `n/a` when the wire carried no rect
/// (the field is optional — an unlaid-out node has none).
fn fmt_bounds(bounds: Option<RectPx>) -> String {
    match bounds {
        Some(r) => format!(
            "x{} y{} w{} h{}",
            fmt_px(r.x),
            fmt_px(r.y),
            fmt_px(r.width),
            fmt_px(r.height)
        ),
        None => "n/a".to_string(),
    }
}

/// A logical-px number: whole values print as integers (§B12's mockup), a
/// fractional one keeps a decimal rather than being silently rounded.
fn fmt_px(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

// ── Props pane ──────────────────────────────────────────────────────────────

/// The props pane: the selected node's header, its bounds, its full type
/// path, then the `widget_props` entries — or the loading/failed/none state
/// standing in for them.
fn render_props(
    frame: &mut Frame,
    area: Rect,
    inspector: &InspectorTab,
    theme: &Theme,
    vertical: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // The divider belongs to this pane: a column down its left edge when
    // split, a rule across its top when stacked.
    let border = Style::default().fg(theme.border());
    let body = if vertical {
        frame.render_widget(
            Paragraph::new(Line::styled("─".repeat(area.width as usize), border)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        Rect::new(area.x, area.y + 1, area.width, area.height - 1)
    } else {
        let rule: Vec<Line<'static>> = (0..area.height)
            .map(|_| Line::styled("│", border))
            .collect();
        frame.render_widget(
            Paragraph::new(rule),
            Rect::new(area.x, area.y, 1, area.height),
        );
        Rect::new(
            area.x + 2,
            area.y,
            area.width.saturating_sub(3),
            area.height,
        )
    };
    if body.width == 0 || body.height == 0 {
        return;
    }

    let focused = inspector.focus == InspectorFocus::Props;
    let muted = Style::default().fg(theme.muted());
    let Some(row) = inspector.selected_row() else {
        frame.render_widget(
            Paragraph::new(Line::styled("no node selected".to_string(), muted)),
            body,
        );
        return;
    };

    let header_style = if focused {
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    };
    let header = format!("{} #{}", short_type_name(&row.type_name), row.id);
    let underline = "━".repeat(header.chars().count().min(body.width as usize));
    let mut lines = vec![
        Line::styled(header, header_style),
        Line::styled(underline, Style::default().fg(theme.accent())),
        kv("bounds", &fmt_bounds(row.bounds), theme),
        kv("type", &row.type_name, theme),
    ];
    if let Some(label) = &row.debug_label {
        lines.push(kv("label", label, theme));
    }
    lines.push(Line::styled(
        "─".repeat(body.width as usize),
        Style::default().fg(theme.border()),
    ));

    if let Some(error) = inspector.error() {
        lines.push(Line::styled(
            format!("✗ {error}"),
            Style::default().fg(theme.error()),
        ));
    } else if let Some(props) = inspector.selected_props() {
        if props.entries.is_empty() {
            lines.push(Line::styled(
                "this node reports no props".to_string(),
                muted,
            ));
        }
        for (key, value) in &props.entries {
            lines.push(kv(key, value, theme));
        }
    } else if inspector.is_props_pending() {
        lines.push(Line::styled("⟳ loading props…".to_string(), muted));
    } else {
        lines.push(Line::styled("no props pulled yet".to_string(), muted));
    }

    // Wrapped, not clipped: a full type path or a long prop value is exactly
    // what this pane exists to show, and a silently cut one would read as a
    // shorter path than the app actually has.
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// One `key      value` row, the key padded to [`PROPS_KEY_WIDTH`].
fn kv(key: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<PROPS_KEY_WIDTH$} "),
            Style::default().fg(theme.muted()),
        ),
        Span::styled(value.to_string(), Style::default().fg(theme.fg())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_type_name_takes_the_last_path_segment() {
        assert_eq!(
            short_type_name("frust_widgets::flex::FlexWidget"),
            "FlexWidget"
        );
        assert_eq!(short_type_name("TextWidget"), "TextWidget");
    }

    #[test]
    fn short_type_name_never_splits_inside_generic_arguments() {
        assert_eq!(
            short_type_name("frust_widgets::list_view::ListViewWidget<app::Row>"),
            "ListViewWidget<app::Row>"
        );
    }

    #[test]
    fn glyphs_map_our_widget_names_onto_the_a8_vocabulary() {
        for (name, glyph) in [
            ("frust_widgets::flex::FlexWidget", "▦"),
            ("Column", "▦"),
            ("frust_widgets::padding::PaddingWidget", "▣"),
            ("frust_widgets::stack::StackWidget", "▣"),
            ("frust_widgets::text::TextWidget", "T"),
            ("frust_widgets::list_view::ListViewWidget", "≡"),
            ("frust_widgets::scroll::ScrollWidget", "≡"),
            // Everything unrecognized takes the neutral glyph rather than
            // being force-fitted into a family.
            ("frust_widgets::button::ButtonWidget", "·"),
            ("my_app::FancyThing", "·"),
        ] {
            assert_eq!(type_glyph(name), glyph, "{name}");
        }
    }

    #[test]
    fn bounds_print_whole_numbers_whole_and_keep_a_fraction() {
        assert_eq!(
            fmt_bounds(Some(RectPx {
                x: 12.0,
                y: 44.0,
                width: 360.0,
                height: 220.5,
            })),
            "x12 y44 w360 h220.5"
        );
        assert_eq!(fmt_bounds(None), "n/a");
    }

    #[test]
    fn scroll_offset_keeps_the_selection_on_screen_without_overscrolling() {
        // Everything fits: no scroll at all.
        assert_eq!(scroll_offset(5, 4, 10), 0);
        // Selection inside the first window.
        assert_eq!(scroll_offset(40, 3, 10), 0);
        // Selection past it: the window follows, selection on the last row.
        assert_eq!(scroll_offset(40, 12, 10), 3);
        // Never past the end.
        assert_eq!(scroll_offset(40, 39, 10), 30);
        assert_eq!(scroll_offset(40, 5, 0), 0);
    }
}
