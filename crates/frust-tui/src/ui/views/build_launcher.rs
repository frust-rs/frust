//! The build-launcher modal: a shadowed, centered
//! popup over the workbench with the artifact-kind selector + `BuildInfo`
//! funnel (mode/flavor/defines) plus kind-conditional flags (split-per-ABI,
//! iOS simulator/codesign, `.ipa` export method) and launch/cancel actions.
//! The base workbench layer is rendered with a *suppressed* `MouseCtx` (see
//! `crate::ui::render`), so only this modal's regions are live while it is
//! open — the D4 base-layer suppression, the same shape `run_config` uses.
//!
//! Layering (D2): renders `&BuildLauncher` and only *registers* interaction;
//! it never mutates the engine.

use ratatui::Frame;
use ratatui::layout::{Alignment, Offset, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Shadow};

use crate::engine::{ArtifactKind, BuildFocus, BuildLauncher, Message, RegionId};
use crate::ui::layout::centered;
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use frust_drive::build_info::BuildMode;

/// Modal width (columns).
const MODAL_WIDTH: u16 = 62;

/// Render the build-launcher modal centered over `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    launcher: &BuildLauncher,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let inner_rows = 3 + kind_row_count(launcher.kind) + 6; // kind+mode+flavor+defines(4) + kind rows + blank+buttons+hint
    let height = (inner_rows + 2).min(area.height);
    let box_ = centered(area, MODAL_WIDTH, height);
    frame.render_widget(Clear, box_);

    let project = launcher
        .project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| launcher.project_root.to_string_lossy().into_owned());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            format!(" Build · {project} "),
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

    // ── Artifact kind ────────────────────────────────────────────────────
    let kind_focused = launcher.focus == BuildFocus::Kind;
    let kind_line = Line::from(vec![
        field_label("Kind", kind_focused, theme),
        Span::styled(
            format!("\u{2039} {} \u{203a}", launcher.kind.label()),
            selector_style(kind_focused, theme),
        ),
    ]);
    frame.render_widget(Paragraph::new(kind_line), row_rect(y));
    mouse.click(
        row_rect(y),
        RegionId::BuildKindRow,
        Message::BuildCycleKind(1),
    );
    advance(&mut y);

    // ── Mode ─────────────────────────────────────────────────────────────
    let mode_focused = launcher.focus == BuildFocus::Mode;
    let mode_line = Line::from(vec![
        field_label("Mode", mode_focused, theme),
        Span::styled(
            format!("\u{2039} {} \u{203a}", mode_label(launcher.mode)),
            selector_style(mode_focused, theme),
        ),
    ]);
    frame.render_widget(Paragraph::new(mode_line), row_rect(y));
    mouse.click(
        row_rect(y),
        RegionId::BuildModeRow,
        Message::BuildCycleMode(1),
    );
    advance(&mut y);

    // ── Flavor / defines ─────────────────────────────────────────────────
    render_field(
        frame,
        mouse,
        row_rect(y),
        "Flavor",
        &launcher.flavor,
        "(none)",
        launcher.focus == BuildFocus::Flavor,
        RegionId::BuildFlavorRow,
        Message::BuildFocus(BuildFocus::Flavor),
        theme,
    );
    advance(&mut y);
    render_field(
        frame,
        mouse,
        row_rect(y),
        "Defines",
        &launcher.defines,
        "KEY=VALUE …",
        launcher.focus == BuildFocus::Defines,
        RegionId::BuildDefinesRow,
        Message::BuildFocus(BuildFocus::Defines),
        theme,
    );
    advance(&mut y);

    // ── Kind-conditional rows ────────────────────────────────────────────
    match launcher.kind {
        ArtifactKind::Apk => {
            render_toggle(
                frame,
                mouse,
                row_rect(y),
                "Split per ABI",
                launcher.split_per_abi,
                launcher.focus == BuildFocus::SplitPerAbi,
                RegionId::BuildSplitPerAbiRow,
                Message::BuildToggleSplitPerAbi,
                theme,
            );
            advance(&mut y);
        }
        ArtifactKind::Appbundle => {}
        ArtifactKind::Ios => {
            render_toggle(
                frame,
                mouse,
                row_rect(y),
                "Simulator",
                launcher.simulator,
                launcher.focus == BuildFocus::Simulator,
                RegionId::BuildSimulatorRow,
                Message::BuildToggleSimulator,
                theme,
            );
            advance(&mut y);
            render_toggle(
                frame,
                mouse,
                row_rect(y),
                "No codesign",
                launcher.no_codesign,
                launcher.focus == BuildFocus::NoCodesign,
                RegionId::BuildNoCodesignRow,
                Message::BuildToggleNoCodesign,
                theme,
            );
            advance(&mut y);
        }
        ArtifactKind::Ipa => {
            render_field(
                frame,
                mouse,
                row_rect(y),
                "Export",
                &launcher.export_method,
                "app-store-connect",
                launcher.focus == BuildFocus::ExportMethod,
                RegionId::BuildExportMethodRow,
                Message::BuildFocus(BuildFocus::ExportMethod),
                theme,
            );
            advance(&mut y);
        }
    }

    advance(&mut y); // blank

    // ── Launch / Cancel buttons ──────────────────────────────────────────
    let launch_focused = launcher.focus == BuildFocus::Launch;
    let launch_label = " Build ";
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
        Span::styled(launch_label, launch_style),
        Span::raw("   "),
        Span::styled(" Cancel ", cancel_style),
    ]);
    frame.render_widget(Paragraph::new(buttons), row_rect(y));
    let launch_w = launch_label.chars().count() as u16;
    mouse.click(
        Rect::new(inner.x, y, launch_w, 1),
        RegionId::BuildLaunchButton,
        Message::BuildLaunch,
    );
    let cancel_x = inner.x + launch_w + 3;
    mouse.click(
        Rect::new(cancel_x, y, 8, 1),
        RegionId::BuildCancelButton,
        Message::CloseBuildLauncher,
    );
    advance(&mut y);

    // ── Keyhint row ──────────────────────────────────────────────────────
    if y < inner.bottom() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Space toggle · ←/→ cycle · Tab next · Enter build · Esc cancel",
                Style::default().fg(theme.muted()),
            ))
            .alignment(Alignment::Left),
            row_rect(y),
        );
    }
}

/// The number of extra rows the current kind's conditional section needs.
fn kind_row_count(kind: ArtifactKind) -> u16 {
    match kind {
        ArtifactKind::Apk | ArtifactKind::Ipa => 1,
        ArtifactKind::Appbundle => 0,
        ArtifactKind::Ios => 2,
    }
}

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

fn selector_style(focused: bool, theme: &Theme) -> Style {
    if focused {
        Style::default()
            .fg(theme.bg())
            .bg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg())
    }
}

/// Render a labeled text field row and register its click (focus) region.
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

/// Render a labeled boolean toggle row and register its click region.
#[allow(clippy::too_many_arguments)]
fn render_toggle(
    frame: &mut Frame,
    mouse: &mut MouseCtx,
    rect: Rect,
    label: &str,
    value: bool,
    focused: bool,
    id: RegionId,
    on_click: Message,
    theme: &Theme,
) {
    let check = if value { "[x]" } else { "[ ]" };
    let line = Line::from(vec![
        field_label(label, focused, theme),
        Span::raw(" "),
        Span::styled(
            check.to_string(),
            Style::default().fg(if value {
                theme.success()
            } else {
                theme.muted()
            }),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), rect);
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
