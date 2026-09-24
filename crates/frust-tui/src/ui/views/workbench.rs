//! The workbench shell: titlebar / sidebar / main / status.
//! Started as static content — project detection, sessions, and
//! interactivity layered on top afterward.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::engine::{AppState, ContextTarget, DeviceRow, DoctorState, DragKind, Message, RegionId};
use crate::supervise::{DapStatus, McpStatus};
use crate::ui::layout::{Shell, sidebar_main_at};
use crate::ui::mouse::MouseCtx;
use crate::ui::theme::Theme;
use crate::ui::views::sessions;
use frust_drive::devices::{Kind, Platform};
use frust_drive::doctor::{ComponentStatus, Status};

/// Render the workbench shell into `area`.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let shell = Shell::split(area);
    titlebar(frame, shell.titlebar, state, theme, mouse);

    // Responsive breakpoint: below `NARROW_WIDTH` the sidebar
    // collapses out of the inline layout, reachable instead as a
    // toggleable floating overlay (`s` / `AppState::sidebar_overlay_open`).
    let narrow = crate::ui::layout::is_narrow(area);
    if narrow && state.sidebar_overlay_open {
        // The body renders full-width but non-interactive beneath the
        // floating panel — the same base-layer suppression every
        // workbench-blocking modal uses, scoped to the body only (the
        // titlebar/status stay live underneath).
        let mut suppressed = MouseCtx::suppressed();
        render_main_area(frame, shell.body, state, theme, &mut suppressed);
        render_sidebar_overlay(frame, shell.body, state, theme, mouse);
    } else {
        let main = if narrow {
            shell.body
        } else {
            let (sidebar, main) = sidebar_main_at(shell.body, state.sidebar_width);
            render_sidebar(frame, sidebar, state, theme, mouse);
            // The sidebar's right border is a drag-to-resize splitter: a
            // left-press on that column starts a `SidebarSplitter`
            // drag whose absolute column maps to a new sidebar width via
            // `x - body_left`.
            if sidebar.width > 0 && sidebar.height > 0 {
                let splitter = Rect::new(
                    sidebar.right().saturating_sub(1),
                    sidebar.y,
                    1,
                    sidebar.height,
                );
                mouse.drag(
                    splitter,
                    DragKind::SidebarSplitter {
                        body_left: shell.body.x,
                    },
                );
            }
            main
        };
        render_main_area(frame, main, state, theme, mouse);
    }
    status(frame, shell.status, state, theme, narrow);
}

/// With sessions open, the main area is the tab bar + log view; otherwise the
/// static dashboard placeholder. Factored out of [`render`] so the
/// narrow-overlay branch and the normal inline-sidebar branch share it.
fn render_main_area(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    if state.sessions.is_empty() {
        render_dashboard(frame, area, state, theme);
    } else {
        sessions::render_main(frame, area, state, theme, mouse);
    }
}

/// The narrow-terminal sidebar overlay: a floating, bordered panel
/// pinned to the left edge of `body`, reusing [`render_sidebar`]'s exact
/// content/regions — the same sidebar, just not part of the inline layout.
fn render_sidebar_overlay(
    frame: &mut Frame,
    body: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let width = (crate::engine::SIDEBAR_DEFAULT_WIDTH + 6).min(body.width);
    let box_ = Rect::new(body.x, body.y, width, body.height);
    frame.render_widget(Clear, box_);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent()))
        .title(Span::styled(
            " Sidebar · Esc/s close ",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.surface()));
    let inner = block.inner(box_);
    frame.render_widget(block, box_);

    render_sidebar(frame, inner, state, theme, mouse);
}

fn titlebar(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme, mouse: &mut MouseCtx) {
    let project = state
        .project_root
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    let row0 = Rect::new(area.x, area.y, area.width, 1);

    // Built with a running column cursor (mirrors the tab-bar / run-config
    // field patterns) so the project-name + `▾` chevron's click rect lands
    // exactly on its rendered columns.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col: u16 = 0;
    let push = |spans: &mut Vec<Span<'static>>, col: &mut u16, text: String, style: Style| {
        *col += text.chars().count() as u16;
        spans.push(Span::styled(text, style));
    };

    push(
        &mut spans,
        &mut col,
        format!("{} Frust ", theme.icons.gear()),
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    );
    push(
        &mut spans,
        &mut col,
        "│ ".to_string(),
        Style::default().fg(theme.border()),
    );

    let switcher_x = area.x + col;
    push(
        &mut spans,
        &mut col,
        project,
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
    );
    push(
        &mut spans,
        &mut col,
        " ▾ ".to_string(),
        Style::default().fg(theme.accent()),
    );
    let switcher_w = col - (switcher_x - area.x);

    push(
        &mut spans,
        &mut col,
        "· frust 0.1.0 · ".to_string(),
        Style::default().fg(theme.muted()),
    );
    let chip_x = area.x + col;
    let (glyph, label, chip_color) = toolchain_chip(state, theme);
    push(
        &mut spans,
        &mut col,
        format!("{glyph} {label}"),
        Style::default().fg(chip_color),
    );
    let chip_w = col - (chip_x - area.x);

    frame.render_widget(Paragraph::new(Line::from(spans)), row0);

    if chip_x < area.right() {
        let w = chip_w.min(area.right().saturating_sub(chip_x));
        mouse.click(
            Rect::new(chip_x, area.y, w, 1),
            RegionId::DoctorChip,
            Message::OpenBootstrapWizard,
        );
    }

    if switcher_x < area.right() {
        let w = switcher_w.min(area.right().saturating_sub(switcher_x));
        mouse.click(
            Rect::new(switcher_x, area.y, w, 1),
            RegionId::ProjectSwitcherToggle,
            Message::ToggleProjectSwitcher,
        );
    }

    let actions = Line::from(vec![
        Span::styled(
            format!(" {} Run ", theme.icons.run()),
            Style::default().fg(theme.fg()),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!(" {} Build ", theme.icons.create()),
            Style::default().fg(theme.fg()),
        ),
    ]);
    frame.render_widget(Paragraph::new(actions).alignment(Alignment::Right), row0);

    // Orange underline motif under the wordmark.
    if area.height > 1 {
        let row1 = Rect::new(area.x, area.y + 1, 8.min(area.width), 1);
        frame.render_widget(
            Paragraph::new(Line::styled(
                "━━━━━━━━",
                Style::default().fg(theme.accent()),
            )),
            row1,
        );
    }
}

fn render_sidebar(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    mouse: &mut MouseCtx,
) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.border()))
        .padding(Padding::new(1, 1, 0, 0))
        .style(Style::default().bg(theme.surface()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let heading = |s: &str| {
        Line::styled(
            s.to_string(),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )
    };
    let item = |s: &str| Line::styled(format!("  {s}"), Style::default().fg(theme.muted()));

    // Build the whole sidebar as a stacked line list, remembering which rows
    // are interactive device rows (and the refresh affordance) so their
    // single-row click rects can be registered after layout.
    let mut lines = vec![heading("PROJECTS")];
    // `project_rows[k]` is the line index the k-th `state.projects` entry
    // drew at — local rows first, then (only when non-empty) a blank line, a
    // "PREVIOUS PROJECTS" heading, and the previous rows, mirroring
    // `device_rows` below. `state.projects` order is `[local..., previous...]`
    // (decision D6), so `project_rows`' own order already lines up with
    // `state.projects`' indices — no separate index remap needed.
    let mut project_rows: Vec<usize> = Vec::new();
    let local_count = state.local_count();
    if local_count == 0 {
        lines.push(Line::styled(
            "  (none detected)",
            Style::default().fg(theme.muted()),
        ));
    } else {
        for project in &state.projects[..local_count] {
            project_rows.push(lines.len());
            lines.push(project_line(state, project, theme));
        }
    }
    if local_count < state.projects.len() {
        lines.push(Line::from(""));
        lines.push(heading("PREVIOUS PROJECTS"));
        for project in &state.projects[local_count..] {
            project_rows.push(lines.len());
            lines.push(project_line(state, project, theme));
        }
    }
    lines.push(Line::from(""));

    // DEVICES header carries the refresh affordance on the right.
    let devices_header_row = lines.len();
    lines.push(devices_header_line(state, theme));
    let mut device_rows: Vec<usize> = Vec::new();
    if state.devices.is_empty() {
        let msg = if state.devices_refreshing {
            "scanning…"
        } else {
            "none found"
        };
        lines.push(item(msg));
    } else {
        for (i, row) in state.devices.iter().enumerate() {
            device_rows.push(lines.len());
            lines.push(device_line(state, i, row, theme));
        }
    }

    lines.push(Line::from(""));
    lines.push(heading("SESSIONS"));
    lines.extend(sessions::sidebar_lines(state, theme));
    lines.push(Line::from(""));
    lines.push(heading("ACTIONS"));
    let new_project_row = lines.len();
    lines.push(item("New project · n"));
    let add_plugin_row = lines.len();
    lines.push(item("Add plugin · a"));
    let doctor_row = lines.len();
    lines.push(item("Doctor · i"));
    let build_row = lines.len();
    lines.push(item("Build · b"));
    let clean_row = lines.len();
    // D7: `c` only means Clean while no session is active — with one active
    // it copies build artifacts instead (`runner::translate_key`'s
    // `has_active_session` predicate), so the row drops its own keyhint
    // there rather than claiming a key it doesn't currently honor. The click
    // still opens the confirm dialog either way (see `CleanAction` below).
    let clean_has_session = state.active_session().is_some();
    lines.push(item(if clean_has_session {
        "Clean"
    } else {
        "Clean · c"
    }));
    let mcp_row = lines.len();
    lines.push(mcp_line(state, theme));
    let dap_row = lines.len();
    lines.push(dap_line(state, theme));
    frame.render_widget(Paragraph::new(lines), inner);

    // Register the interactive regions now that row indices are known (a row
    // scrolled past the visible height registers nothing).
    let row_rect = |row: usize| -> Option<Rect> {
        let y = inner.y + row as u16;
        (y < inner.bottom()).then(|| Rect::new(inner.x, y, inner.width, 1))
    };
    for (i, &row) in project_rows.iter().enumerate() {
        if let Some(rect) = row_rect(row) {
            mouse.click(rect, RegionId::ProjectRow(i), Message::SwitchProject(i));
            mouse.context(rect, ContextTarget::ProjectRow(i));
        }
    }
    if let Some(rect) = row_rect(devices_header_row) {
        mouse.click(rect, RegionId::RefreshDevices, Message::RefreshDevices);
    }
    for (i, &row) in device_rows.iter().enumerate() {
        if let Some(rect) = row_rect(row) {
            mouse.click(rect, RegionId::DeviceRow(i), Message::SelectDeviceAt(i));
            mouse.context(rect, ContextTarget::DeviceRow(i));
        }
    }
    if let Some(rect) = row_rect(doctor_row) {
        mouse.click(rect, RegionId::DoctorAction, Message::OpenDoctorPanel);
    }
    if let Some(rect) = row_rect(build_row) {
        mouse.click(rect, RegionId::BuildAction, Message::OpenBuildLauncher);
    }
    if let Some(rect) = row_rect(clean_row) {
        mouse.click(rect, RegionId::CleanAction, Message::OpenCleanConfirm);
    }
    if let Some(rect) = row_rect(new_project_row) {
        mouse.click(rect, RegionId::NewProjectAction, Message::OpenCreateWizard);
    }
    if let Some(rect) = row_rect(add_plugin_row) {
        mouse.click(rect, RegionId::AddPluginAction, Message::OpenAddPlugin);
    }
    // The whole MCP row *toggles* the server (workbook §B13's switch-and-
    // readout row), exactly as `M` does; the panel behind it is `m` / the
    // palette, so no single row carries two different click meanings.
    if let Some(rect) = row_rect(mcp_row) {
        mouse.click(rect, RegionId::McpToggle, Message::ToggleMcpServer);
    }
    // The DAP row *opens the dialog* rather than toggling the server: unlike
    // MCP, the switch is not the whole surface — when the server starts by
    // itself, and what it writes into the editor's config, live there too.
    if let Some(rect) = row_rect(dap_row) {
        mouse.click(rect, RegionId::DapAction, Message::OpenDapSettings);
    }
}

/// The sidebar ACTIONS "MCP" row: the label, the server's current state, and
/// the `M` keyhint (workbook §B13). The state token carries the color —
/// muted off, warn while starting, success once listening, error after a
/// failure — since the row is the switch *and* the readout.
///
/// The connected-client count is shown only when there is one to show:
/// `(0)` beside a listening server would read as a problem rather than as an
/// idle server waiting for an agent.
fn mcp_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let (text, color) = mcp_state_token(&state.mcp_status(), state.mcp_error.is_some(), theme);
    Line::from(vec![
        Span::styled("  MCP ", Style::default().fg(theme.muted())),
        Span::styled(text, Style::default().fg(color)),
        Span::styled(" · M", Style::default().fg(theme.muted())),
    ])
}

/// The sidebar ACTIONS "DAP" row: the label, the embedded debug-adapter
/// server's current state, and the `D` keyhint. The state token carries the
/// color the same way the MCP row's does — but this row is a *readout plus a
/// way in*, not the switch: clicking it opens the settings dialog, where the
/// switch lives beside the preferences that can flip it without being asked.
fn dap_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let (text, color) = dap_state_token(&state.dap_status(), state.dap_error.is_some(), theme);
    Line::from(vec![
        Span::styled("  DAP ", Style::default().fg(theme.muted())),
        Span::styled(text, Style::default().fg(color)),
        Span::styled(" · D", Style::default().fg(theme.muted())),
    ])
}

/// The DAP row's state token + its color — the same four states the MCP row
/// shows, and `failed` is derived the same way (a *stopped* server carrying a
/// retained reason).
fn dap_state_token(
    status: &DapStatus,
    failed: bool,
    theme: &Theme,
) -> (String, ratatui::style::Color) {
    match status {
        DapStatus::Stopped if failed => ("failed".to_string(), theme.error()),
        DapStatus::Stopped => ("off".to_string(), theme.muted()),
        DapStatus::Starting => ("start…".to_string(), theme.warn()),
        DapStatus::Listening { port, clients: 0 } => (format!(":{port}"), theme.success()),
        DapStatus::Listening { port, clients } => (format!(":{port} ({clients})"), theme.success()),
    }
}

/// The MCP row's state token + its color — the four §B13 states. `failed` is
/// a *stopped* server that carries a reason, so it is derived from the
/// retained error rather than from a state of its own.
fn mcp_state_token(
    status: &McpStatus,
    failed: bool,
    theme: &Theme,
) -> (String, ratatui::style::Color) {
    match status {
        McpStatus::Stopped if failed => ("failed".to_string(), theme.error()),
        McpStatus::Stopped => ("off".to_string(), theme.muted()),
        McpStatus::Starting => ("start…".to_string(), theme.warn()),
        McpStatus::Listening { port, clients: 0 } => (format!(":{port}"), theme.success()),
        McpStatus::Listening { port, clients } => (format!(":{port} ({clients})"), theme.success()),
    }
}

/// The titlebar toolchain chip's glyph/label/color: the component-level
/// bootstrap report's rollup when one is cached (its `Ok`/`Partial`/`Missing`
/// is the chip's real source), falling back to the flat doctor `overall`
/// until the first report lands, then to "checking…" before either
/// preflight completes. Shared with `views::welcome::titlebar`'s chip; clicking
/// it opens the bootstrap wizard.
pub(crate) fn toolchain_chip(
    state: &AppState,
    theme: &Theme,
) -> (&'static str, &'static str, Color) {
    match state.bootstrap.rollup() {
        Some(ComponentStatus::Ok) => (theme.icons.ok(), "toolchain", theme.success()),
        Some(ComponentStatus::Partial) => ("!", "partial", theme.warn()),
        Some(ComponentStatus::Missing) => ("\u{2717}", "missing", theme.error()),
        None => doctor_chip(&state.doctor, theme),
    }
}

/// The chip glyph/label/color for the flat doctor `overall` status (the
/// pre-report fallback): "checking…" (muted) before the startup preflight's
/// first result, else "ok"/"partial"/"missing" per [`DoctorState::overall`].
pub(crate) fn doctor_chip(
    doctor: &DoctorState,
    theme: &Theme,
) -> (&'static str, &'static str, Color) {
    match doctor.overall() {
        None => ("\u{25cc}", "checking…", theme.muted()),
        Some(Status::Pass) => (theme.icons.ok(), "toolchain", theme.success()),
        Some(Status::Partial) => ("!", "partial", theme.warn()),
        Some(Status::Fail) => ("\u{2717}", "missing", theme.error()),
    }
}

/// The DEVICES section header, with the refresh affordance (a spinner-ish
/// glyph while a scan is in flight) right-aligned into the label.
fn devices_header_line(state: &AppState, theme: &Theme) -> Line<'static> {
    let selected = state.selected_device_count();
    let count = if selected > 0 {
        format!("DEVICES ({selected})")
    } else {
        "DEVICES".to_string()
    };
    let refresh = if state.devices_refreshing {
        Span::styled(" \u{25cc} scanning", Style::default().fg(theme.warn()))
    } else {
        Span::styled(" \u{21bb} r", Style::default().fg(theme.muted()))
    };
    Line::from(vec![
        Span::styled(
            count,
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        refresh,
    ])
}

/// One device row: cursor marker, select checkbox, a connection-colored dot,
/// the device name, and a dim platform·kind tag.
fn device_line(state: &AppState, index: usize, row: &DeviceRow, theme: &Theme) -> Line<'static> {
    let under_cursor = state.device_cursor == index;
    let marker = if under_cursor { "\u{25b8}" } else { " " };
    let check = if row.selected { "[x]" } else { "[ ]" };
    let (dot_color, tag) = device_glyph(&row.device, theme);
    let name_style = if under_cursor {
        Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD)
    } else if row.selected {
        Style::default().fg(theme.fg())
    } else {
        Style::default().fg(theme.muted())
    };
    Line::from(vec![
        Span::styled(format!(" {marker} "), Style::default().fg(theme.accent())),
        Span::styled(
            format!("{check} "),
            Style::default().fg(if row.selected {
                theme.success()
            } else {
                theme.muted()
            }),
        ),
        Span::styled("\u{25cf} ", Style::default().fg(dot_color)),
        Span::styled(row.device.name.clone(), name_style),
        Span::styled(format!(" {tag}"), Style::default().fg(theme.muted())),
    ])
}

/// The connection dot color and the `platform·kind` tag for a device.
fn device_glyph(
    device: &frust_drive::devices::Device,
    theme: &Theme,
) -> (ratatui::style::Color, String) {
    let platform = match device.platform {
        Platform::Android => "android",
        Platform::Ios => "ios",
    };
    let kind = match device.kind {
        Kind::PhysicalDevice => "device",
        Kind::Emulator => "emulator",
        Kind::Simulator => "simulator",
    };
    // A physical iOS device reports a connection state; "disconnected" is a
    // paired-but-not-tunneled device (still targetable) — dim it.
    let color = match device.connection_state.as_deref() {
        Some("disconnected") => theme.warn(),
        _ => theme.success(),
    };
    (color, format!("{platform}·{kind}"))
}

/// One sidebar/switcher-dropdown row for `project` (used for both the
/// "PROJECTS" and "PREVIOUS PROJECTS" sections — the row itself carries no
/// marker of which section it's in); the active one (`state.project_root`)
/// gets the hover chevron and accent color, the rest render muted.
fn project_line(state: &AppState, project: &std::path::Path, theme: &Theme) -> Line<'static> {
    let name = project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project.to_string_lossy().to_string());
    let active = state.project_root.as_deref() == Some(project);
    if active {
        Line::styled(
            format!(" {} {name}", theme.icons.chevron()),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Line::styled(format!("   {name}"), Style::default().fg(theme.muted()))
    }
}

fn render_dashboard(frame: &mut Frame, area: Rect, _state: &AppState, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(theme.bg()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::styled(
            "Dashboard",
            Style::default().fg(theme.fg()).add_modifier(Modifier::BOLD),
        ),
        Line::styled("━━━━━━━━━━", Style::default().fg(theme.accent())),
        Line::from(""),
        Line::styled(
            "No session running — start a run to open a log tab here.",
            Style::default().fg(theme.muted()),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Line-selection mode's status-row hint, widest form first, each dropping
/// one segment of the previous — the same graduated degrade the log status
/// row's own keyhint uses, and for the same reason: this row shares its cells
/// with the right-aligned mouse indicator, so a fixed string would paint over
/// it on a narrow terminal and garble both. `{n}` is the selected line count.
/// Kept longest: the mode name and the two keys that end it.
const SELECT_HINT_LEVELS: [&str; 4] = [
    "SELECT · {n} lines · ↑↓/jk extend · click sets anchor/range · y copy · Esc exit",
    "SELECT · {n} lines · ↑↓/jk extend · y copy · Esc exit",
    "SELECT · {n} lines · y copy · Esc exit",
    "SELECT · {n} lines",
];

/// The widest [`SELECT_HINT_LEVELS`] form for `lines` selected that fits
/// `budget` columns — pure, so the fit decision is unit-testable without a
/// terminal. The narrowest level is returned even when it does not fit:
/// a crowded mode indicator is recoverable, an invisible one is not.
fn select_hint(lines: u64, budget: usize) -> String {
    SELECT_HINT_LEVELS
        .iter()
        .map(|level| level.replace("{n}", &lines.to_string()))
        .find(|hint| hint.chars().count() <= budget)
        .unwrap_or_else(|| {
            SELECT_HINT_LEVELS[SELECT_HINT_LEVELS.len() - 1].replace("{n}", &lines.to_string())
        })
}

fn status(frame: &mut Frame, area: Rect, state: &AppState, theme: &Theme, narrow: bool) {
    let running = state.live_session_count();
    let (dot, dot_color, label) = if running > 0 {
        ("●", theme.success(), format!("{running} running"))
    } else {
        ("●", theme.success(), "ready".to_string())
    };
    // Line-selection mode takes the hint row over for as long as it is
    // engaged on the *active* session (the mode is per session), the same way
    // the DevTools pane swaps the log view's own hints: while a range is
    // being picked, the global `r`/`b`/`d` hints name keys the mode swallows,
    // so showing them would be a lie. Accent-styled, so the row also reads as
    // a mode indicator and not just a different list of keys.
    //
    // Keyed on `select_mode` alone: the mode always ends the moment its
    // selection would otherwise become empty (`SessionView::push_line_at`'s
    // eviction clamp), so `select_mode && selection.is_none()` never reaches
    // here. The count is the selected range's **visible** entries, not its
    // raw `hi - lo + 1` span — the same rule `y` copies under (see
    // `LineSelection`'s doc).
    let select = state
        .active_session()
        .filter(|s| s.select_mode)
        .map(|s| s.selected_visible_count(state.search.filter.as_deref()));
    let used_left = format!("{dot} {label}").chars().count() + "  │  ".chars().count();
    let indicator = crate::ui::mouse_indicator(state).chars().count();
    let budget = (area.width as usize).saturating_sub(used_left + indicator + 2);
    let (hint, hint_style) = match select {
        Some(lines) => (
            select_hint(lines, budget),
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        None => {
            // D5: `d` means DevTools only, with an active session, and
            // nothing otherwise — the same `has_active_session` predicate
            // `runner::translate_key` gates it on. Doctor lives on its own
            // unconditional `i` (never claims a key the keyboard doesn't
            // currently honor), so the hint shows whichever of the two
            // actually applies.
            let d_hint = if state.active_session().is_some() {
                "d devtools"
            } else {
                "i doctor"
            };
            let mut hint = format!("r run · b build · {d_hint} · ⌘ palette · ? help");
            if narrow {
                // The narrow-breakpoint sidebar-overlay toggle only matters
                // (and only shows) once the sidebar has actually collapsed
                // out of the inline layout — a zero-noise hint otherwise.
                hint.push_str(" · s sidebar");
            }
            (hint, Style::default().fg(theme.muted()))
        }
    };
    let left = Line::from(vec![
        Span::styled(format!("{dot} {label}"), Style::default().fg(dot_color)),
        Span::styled("  │  ", Style::default().fg(theme.border())),
        Span::styled(hint, hint_style),
    ]);
    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(theme.surface())),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            crate::ui::mouse_indicator(state),
            Style::default().fg(theme.muted()),
        ))
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.surface())),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mode indicator degrades one segment at a time instead of
    /// overwriting the right-aligned mouse indicator beside it — and never
    /// degrades to nothing.
    #[test]
    fn the_select_hint_picks_the_widest_form_that_fits() {
        assert_eq!(
            select_hint(3, 80),
            "SELECT · 3 lines · ↑↓/jk extend · click sets anchor/range · y copy · Esc exit"
        );
        assert_eq!(
            select_hint(3, 60),
            "SELECT · 3 lines · ↑↓/jk extend · y copy · Esc exit"
        );
        assert_eq!(select_hint(3, 40), "SELECT · 3 lines · y copy · Esc exit");
        assert_eq!(select_hint(12, 20), "SELECT · 12 lines");
        assert_eq!(
            select_hint(12, 0),
            "SELECT · 12 lines",
            "the count is the floor, never an empty indicator"
        );
    }

    /// The sidebar ACTIONS "MCP" row is the switch *and* the readout
    /// (workbook §B13), so its four states must each read differently — and
    /// a listening server with nobody attached must not read as `(0)`.
    #[test]
    fn the_mcp_row_reads_its_four_states_distinctly() {
        let theme = Theme::frust_dark();
        let token = |status: &McpStatus, failed: bool| mcp_state_token(status, failed, &theme).0;

        assert_eq!(token(&McpStatus::Stopped, false), "off");
        assert_eq!(token(&McpStatus::Stopped, true), "failed");
        assert_eq!(token(&McpStatus::Starting, false), "start…");
        assert_eq!(
            token(
                &McpStatus::Listening {
                    port: 4848,
                    clients: 0
                },
                false
            ),
            ":4848"
        );
        assert_eq!(
            token(
                &McpStatus::Listening {
                    port: 4848,
                    clients: 2
                },
                false
            ),
            ":4848 (2)"
        );
    }

    /// Every state's row still fits the *default* sidebar (26 columns, less
    /// its border and padding) — the row is a status readout, and a truncated
    /// port number would be worse than no readout at all.
    #[test]
    fn the_mcp_row_fits_the_default_sidebar_width() {
        let theme = Theme::frust_dark();
        let inner_width = (crate::engine::SIDEBAR_DEFAULT_WIDTH - 3) as usize;
        for (status, failed) in [
            (McpStatus::Stopped, false),
            (McpStatus::Stopped, true),
            (McpStatus::Starting, false),
            (
                McpStatus::Listening {
                    port: 65535,
                    clients: 12,
                },
                false,
            ),
        ] {
            let (token, _) = mcp_state_token(&status, failed, &theme);
            // The row is `"  MCP " + token + " · M"` (see `mcp_line`).
            let width = "  MCP ".chars().count() + token.chars().count() + " · M".chars().count();
            assert!(
                width <= inner_width,
                "`{token}` row is {width} cols, sidebar inner is {inner_width}"
            );
        }
    }

    /// The DAP row is a readout (the switch lives in the dialog behind it),
    /// but it reads its states exactly the way the MCP row does — the two sit
    /// next to each other, so a reader must not have to learn two vocabularies.
    #[test]
    fn the_dap_row_reads_its_states_the_same_way_the_mcp_row_does() {
        let theme = Theme::frust_dark();
        let dap = |status: &DapStatus, failed: bool| dap_state_token(status, failed, &theme).0;
        let mcp = |status: &McpStatus, failed: bool| mcp_state_token(status, failed, &theme).0;

        assert_eq!(
            dap(&DapStatus::Stopped, false),
            mcp(&McpStatus::Stopped, false)
        );
        assert_eq!(
            dap(&DapStatus::Stopped, true),
            mcp(&McpStatus::Stopped, true)
        );
        assert_eq!(
            dap(&DapStatus::Starting, false),
            mcp(&McpStatus::Starting, false)
        );
        assert_eq!(
            dap(
                &DapStatus::Listening {
                    port: 4849,
                    clients: 0
                },
                false
            ),
            ":4849"
        );
        assert_eq!(
            dap(
                &DapStatus::Listening {
                    port: 4849,
                    clients: 2
                },
                false
            ),
            ":4849 (2)"
        );
    }

    /// …and fits the default sidebar in every state, for the same reason.
    #[test]
    fn the_dap_row_fits_the_default_sidebar_width() {
        let theme = Theme::frust_dark();
        let inner_width = (crate::engine::SIDEBAR_DEFAULT_WIDTH - 3) as usize;
        for (status, failed) in [
            (DapStatus::Stopped, false),
            (DapStatus::Stopped, true),
            (DapStatus::Starting, false),
            (
                DapStatus::Listening {
                    port: 65535,
                    clients: 12,
                },
                false,
            ),
        ] {
            let (token, _) = dap_state_token(&status, failed, &theme);
            // The row is `"  DAP " + token + " · D"` (see `dap_line`).
            let width = "  DAP ".chars().count() + token.chars().count() + " · D".chars().count();
            assert!(
                width <= inner_width,
                "`{token}` row is {width} cols, sidebar inner is {inner_width}"
            );
        }
    }

    // ── PROJECTS / PREVIOUS PROJECTS sidebar split (decision D6) ──────────

    use crate::ui::mouse::{MouseCtx, MouseRegions};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use std::path::PathBuf;

    fn sidebar_state(projects: Vec<PathBuf>, local_count: usize) -> AppState {
        AppState {
            screen: crate::engine::Screen::Workbench,
            project_root: projects.first().cloned(),
            local_project_count: local_count,
            projects,
            ..AppState::default()
        }
    }

    /// Render just the sidebar into a plain string, registering its mouse
    /// regions into `regions` — mirrors `add_plugin`'s
    /// `render_dialog_to_string` helper.
    fn render_sidebar_to_string(state: &AppState, regions: &mut MouseRegions) -> String {
        let backend = TestBackend::new(30, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let theme = Theme::frust_dark();
        terminal
            .draw(|frame| {
                let mut ctx = MouseCtx::new(regions);
                let area = Rect::new(0, 0, 30, 30);
                render_sidebar(frame, area, state, &theme, &mut ctx);
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

    #[test]
    fn both_headings_appear_in_order_when_previous_projects_exist() {
        let state = sidebar_state(
            vec![PathBuf::from("/tmp/local"), PathBuf::from("/tmp/prev")],
            1,
        );
        let mut regions = MouseRegions::new();
        let text = render_sidebar_to_string(&state, &mut regions);
        let projects_at = text.find("PROJECTS").expect("PROJECTS heading present");
        let previous_at = text
            .find("PREVIOUS PROJECTS")
            .expect("PREVIOUS PROJECTS heading present");
        assert!(
            projects_at < previous_at,
            "PROJECTS must render before PREVIOUS PROJECTS:\n{text}"
        );
        assert!(text.contains("local"), "{text}");
        assert!(text.contains("prev"), "{text}");
    }

    #[test]
    fn no_previous_heading_when_every_project_is_local() {
        let state = sidebar_state(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")], 2);
        let mut regions = MouseRegions::new();
        let text = render_sidebar_to_string(&state, &mut regions);
        assert!(
            !text.contains("PREVIOUS PROJECTS"),
            "no previous projects means no heading:\n{text}"
        );
    }

    #[test]
    fn clicking_the_first_previous_row_dispatches_switch_project_at_local_count() {
        let local_count = 1;
        let state = sidebar_state(
            vec![PathBuf::from("/tmp/local"), PathBuf::from("/tmp/prev")],
            local_count,
        );
        let mut regions = MouseRegions::new();
        let _ = render_sidebar_to_string(&state, &mut regions);
        // Scan straight down the sidebar's first column for the row that
        // dispatches `SwitchProject(local_count)` — the first PREVIOUS row.
        let hit = (0..30).find(|&y| {
            matches!(regions.click_at(2, y), Some(Message::SwitchProject(i)) if i == local_count)
        });
        assert!(
            hit.is_some(),
            "no row dispatched SwitchProject({local_count})"
        );
    }
}
