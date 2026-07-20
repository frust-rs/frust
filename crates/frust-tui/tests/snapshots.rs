//! TestBackend + insta snapshot suite for the render layer (D2 testing
//! strategy). Snapshots are the plain-text cell grid (symbols only) so they
//! are terminal- and color-depth-independent; hover/pressed states are made
//! text-visible where they matter.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use frust_tui::engine::{AppState, RegionId, Screen, Scroll, SessionView};
use frust_tui::supervise::{SessionId, SessionState};
use frust_tui::ui::mouse::{MouseCtx, MouseRegions};
use frust_tui::ui::theme::{ColorDepth, Theme};

/// Render `state` at `w`x`h` and return the cell-grid text.
fn render_to_string(w: u16, h: u16, state: &AppState) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::frust_dark_at(ColorDepth::TrueColor);
    let mut regions = MouseRegions::new();
    terminal
        .draw(|frame| {
            let mut ctx = MouseCtx::new(&mut regions);
            frust_tui::ui::render(frame, state, &theme, &mut ctx);
        })
        .expect("draw");
    buffer_to_string(terminal.backend().buffer())
}

/// Dump a buffer as a newline-separated grid of cell symbols (version-stable,
/// no dependency on `TestBackend`'s Display impl).
fn buffer_to_string(buf: &Buffer) -> String {
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

fn workbench_state() -> AppState {
    let root = PathBuf::from("/tmp/huddle");
    AppState {
        screen: Screen::Workbench,
        project_root: Some(root.clone()),
        projects: vec![root],
        ..Default::default()
    }
}

/// F5: multiple detected projects — the sidebar lists every one, the active
/// (first) one highlighted with the hover chevron.
fn multi_project_workbench_state() -> AppState {
    let bubblebench = PathBuf::from("/tmp/frust/examples/bubblebench");
    let huddle = PathBuf::from("/tmp/frust/examples/huddle");
    AppState {
        screen: Screen::Workbench,
        project_root: Some(bubblebench.clone()),
        projects: vec![bubblebench, huddle],
        ..Default::default()
    }
}

#[test]
fn welcome_screen_80x24() {
    let state = AppState::default();
    assert_eq!(state.screen, Screen::Welcome);
    insta::assert_snapshot!(render_to_string(80, 24, &state));
}

#[test]
fn welcome_screen_hover_80x24() {
    let state = AppState {
        hover: Some(RegionId::CreateButton),
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string(80, 24, &state));
}

#[test]
fn welcome_screen_pressed_80x24() {
    let state = AppState {
        create_pressed: true,
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string(80, 24, &state));
}

#[test]
fn workbench_shell_80x24() {
    insta::assert_snapshot!(render_to_string(80, 24, &workbench_state()));
}

#[test]
fn workbench_shell_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &workbench_state()));
}

#[test]
fn workbench_multi_project_sidebar_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &multi_project_workbench_state()));
}

#[test]
fn too_small_terminal_40x10() {
    let state = AppState::default();
    insta::assert_snapshot!(render_to_string(40, 10, &state));
}

// ── Session workspace (tab bar + log view) ──────────────────────────────────

/// A session on `project`/`target`, seeded with `lines`, in `state`.
fn session(id: u64, project: &str, target: &str, s: SessionState, lines: &[&str]) -> SessionView {
    let mut sv = SessionView::new(SessionId(id), PathBuf::from(project), target);
    sv.state = s;
    for l in lines {
        sv.push_line((*l).to_string());
    }
    sv
}

/// One running desktop session with a mix of plain, error, and warning lines —
/// following the tail (the default).
fn single_session_state() -> AppState {
    let root = "/tmp/huddle";
    let sess = session(
        0,
        root,
        "desktop",
        SessionState::Running,
        &[
            "   Compiling huddle v0.1.0",
            "    Finished dev profile",
            "     Running `target/debug/huddle`",
            "app: booting up",
            "warning: unused variable `x`",
            "app: frame 1 rendered",
            "error: texture upload failed",
            "app: recovering",
        ],
    );
    AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(root)),
        projects: vec![PathBuf::from(root)],
        sessions: vec![sess],
        active_session: Some(0),
        ..Default::default()
    }
}

#[test]
fn session_log_follow_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &single_session_state()));
}

#[test]
fn session_log_scrolled_100x30() {
    let mut state = single_session_state();
    // Freeze the view with line 2 ("Running …") at the bottom — scrolled up
    // off the tail, so the follow indicator flips to "scrolled".
    state.sessions[0].scroll = Scroll::Anchored(2);
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

#[test]
fn session_log_filtered_100x30() {
    let mut state = single_session_state();
    state.search.filter = Some("app".to_string());
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

#[test]
fn session_search_overlay_100x30() {
    let mut state = single_session_state();
    state.search.open = true;
    state.search.query = "err".to_string();
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// Two projects, three sessions — the tab bar and sidebar group by project.
fn multi_session_state() -> AppState {
    let huddle = "/tmp/frust/examples/huddle";
    let bubble = "/tmp/frust/examples/bubblebench";
    AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(huddle)),
        projects: vec![PathBuf::from(huddle), PathBuf::from(bubble)],
        sessions: vec![
            session(0, huddle, "desktop", SessionState::Running, &["huddle: up"]),
            session(
                1,
                huddle,
                "Pixel 7",
                SessionState::Building,
                &["Building it.f0x.huddle…"],
            ),
            session(
                2,
                bubble,
                "desktop",
                SessionState::Exited(false),
                &["error: bubble crashed"],
            ),
        ],
        active_session: Some(1),
        ..Default::default()
    }
}

#[test]
fn session_tabs_grouped_120x36() {
    insta::assert_snapshot!(render_to_string(120, 36, &multi_session_state()));
}
