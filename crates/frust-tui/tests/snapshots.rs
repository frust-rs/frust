//! TestBackend + insta snapshot suite for the render layer (D2 testing
//! strategy). Snapshots are the plain-text cell grid (symbols only) so they
//! are terminal- and color-depth-independent; hover/pressed states are made
//! text-visible where they matter.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use frust_drive::devices::{Device, Kind, Platform};
use frust_drive::doctor::{Area, Component, ComponentStatus, DoctorReport, FixCommand, Status};
use frust_tui::engine::{
    AppState, BootstrapState, BootstrapWizard, BuildLauncher, ContextTarget, CreateWizard,
    DeviceRow, DoctorCheck, DoctorState, Message, Palette, RegionId, RunConfig, RunFocus, Screen,
    Scroll, SessionView, ToastKind, WizardStep, update,
};
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

/// F5 + D6b: the titlebar project-switcher dropdown open over a multi-project
/// workbench, the second project (bubblebench's sibling, huddle) highlighted
/// by the switcher cursor while bubblebench stays the active (accented) one.
#[test]
fn project_switcher_open_100x30() {
    let mut state = multi_project_workbench_state();
    state.project_switcher_open = true;
    state.project_switcher_cursor = 1;
    insta::assert_snapshot!(render_to_string(100, 30, &state));
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

// ── Devices panel + run-config modal (D6b) ──────────────────────────────────

fn device(id: &str, name: &str, platform: Platform, kind: Kind) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        platform,
        kind,
        os_version: None,
        connection_state: None,
    }
}

fn devices() -> Vec<DeviceRow> {
    vec![
        DeviceRow {
            device: device(
                "53f887ac",
                "OnePlus 9",
                Platform::Android,
                Kind::PhysicalDevice,
            ),
            selected: true,
        },
        DeviceRow {
            device: device(
                "emulator-5554",
                "Pixel 7",
                Platform::Android,
                Kind::Emulator,
            ),
            selected: false,
        },
        DeviceRow {
            device: device("AAAA", "iPhone SE", Platform::Ios, Kind::PhysicalDevice),
            selected: false,
        },
    ]
}

/// The devices sidebar populated: three targets, one multi-selected, the
/// cursor on the second, and the DEVICES header showing the select count.
fn devices_panel_state() -> AppState {
    let root = PathBuf::from("/tmp/huddle");
    AppState {
        screen: Screen::Workbench,
        project_root: Some(root.clone()),
        projects: vec![root],
        devices: devices(),
        device_cursor: 1,
        ..Default::default()
    }
}

#[test]
fn devices_panel_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &devices_panel_state()));
}

/// The run-config modal open over the workbench: desktop + the three devices
/// (OnePlus 9 pre-checked from the panel), a non-default mode/flavor, focus on
/// the defines field.
fn run_config_modal_state() -> AppState {
    let mut state = devices_panel_state();
    let mut modal = RunConfig::new(state.project_root.clone().unwrap(), &state.devices);
    modal.mode = frust_drive::build_info::BuildMode::Release;
    modal.flavor = "paid".into();
    modal.defines = "FRUST_TRACE=1".into();
    modal.focus = RunFocus::Defines;
    state.run_config = Some(modal);
    state
}

#[test]
fn run_config_modal_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &run_config_modal_state()));
}

// ── Create-project wizard (D6b) ─────────────────────────────────────────────

/// A wizard with a probe already resolved to `clean_signals` availability, on
/// `step`, with `name`/`directory` filled and `arch_cursor` set.
fn wizard_state(step: WizardStep, name: &str, clean_signals: bool, arch_cursor: usize) -> AppState {
    let mut wizard = CreateWizard::new();
    wizard.set_clean_signals_available(clean_signals);
    for c in name.chars() {
        wizard.input_char(c);
    }
    wizard.step = step;
    wizard.arch_cursor = arch_cursor;
    AppState {
        screen: Screen::Welcome,
        create_wizard: Some(wizard),
        ..Default::default()
    }
}

/// The wizard's name step over the welcome screen, mid-typing a valid name.
#[test]
fn wizard_name_step_80x24() {
    insta::assert_snapshot!(render_to_string(
        80,
        24,
        &wizard_state(WizardStep::Name, "my_app", false, 0)
    ));
}

/// The architecture step with the clean-signals sibling ABSENT — the
/// clean-signals card renders disabled-with-explanation.
#[test]
fn wizard_arch_step_sibling_absent_80x24() {
    insta::assert_snapshot!(render_to_string(
        80,
        24,
        &wizard_state(WizardStep::Arch, "my_app", false, 1)
    ));
}

/// The architecture step with the sibling PRESENT — the clean-signals card is
/// selectable.
#[test]
fn wizard_arch_step_sibling_present_80x24() {
    insta::assert_snapshot!(render_to_string(
        80,
        24,
        &wizard_state(WizardStep::Arch, "my_app", true, 1)
    ));
}

// ── Doctor panel + titlebar chip (TUI2-07) ──────────────────────────────────

/// A `Pass`/`Partial`/`Fail` mix — the panel and chip both key off the worst
/// (`overall`).
fn doctor_results() -> Vec<DoctorCheck> {
    vec![
        DoctorCheck {
            name: "Rust toolchain".to_string(),
            status: Status::Pass,
            messages: Vec::new(),
        },
        DoctorCheck {
            name: "Android SDK/NDK".to_string(),
            status: Status::Partial,
            messages: vec!["ANDROID_NDK_HOME not set".to_string()],
        },
        DoctorCheck {
            name: "Xcode".to_string(),
            status: Status::Fail,
            messages: vec!["Xcode not found. Run: xcode-select --install".to_string()],
        },
    ]
}

/// The titlebar chip reflecting an all-`Pass` doctor state (the common case).
#[test]
fn titlebar_chip_ok_100x30() {
    let mut state = workbench_state();
    state.doctor = DoctorState {
        results: vec![DoctorCheck {
            name: "Rust toolchain".to_string(),
            status: Status::Pass,
            messages: Vec::new(),
        }],
        refreshing: false,
    };
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The doctor panel open over the workbench, showing a mixed
/// Pass/Partial/Fail result set with actionable hints under the non-passing
/// checks.
#[test]
fn doctor_panel_100x30() {
    let mut state = workbench_state();
    state.doctor = DoctorState {
        results: doctor_results(),
        refreshing: false,
    };
    state.doctor_panel_open = true;
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Build launcher (TUI2-07) ────────────────────────────────────────────────

/// The build-launcher modal open over the workbench: an Apk target with
/// split-per-ABI checked and a flavor set.
#[test]
fn build_launcher_apk_100x30() {
    let mut state = workbench_state();
    let mut launcher = BuildLauncher::new(state.project_root.clone().unwrap());
    launcher.split_per_abi = true;
    launcher.flavor = "paid".into();
    state.build_launcher = Some(launcher);
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Clean confirm dialog (TUI2-07) ──────────────────────────────────────────

/// The clean-confirm dialog open over the workbench.
#[test]
fn clean_confirm_100x30() {
    let mut state = workbench_state();
    state.clean_confirm = state.project_root.clone();
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Build artifact copy-path (TUI2-07) ──────────────────────────────────────

fn built_session_state() -> AppState {
    let root = "/tmp/huddle";
    let mut sess = session(
        0,
        root,
        "build apk",
        SessionState::Exited(true),
        &["Building `it.f0x.huddle`…"],
    );
    sess.push_line("Built: /tmp/huddle/android/app/build/outputs/apk/release/app.apk".to_string());
    AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(root)),
        projects: vec![PathBuf::from(root)],
        sessions: vec![sess],
        active_session: Some(0),
        ..Default::default()
    }
}

// ── Bootstrap wizard + toolchain chip (D6a) ─────────────────────────────────

fn comp(name: &str, status: ComponentStatus, fixes: Vec<FixCommand>) -> Component {
    Component {
        name: name.to_string(),
        status,
        summary: match status {
            ComponentStatus::Ok => "ok".to_string(),
            ComponentStatus::Partial => "partial".to_string(),
            ComponentStatus::Missing => "not found".to_string(),
        },
        fix_commands: fixes,
    }
}

fn runnable(display: &str, program: &str, args: &[&str]) -> FixCommand {
    FixCommand {
        display: display.to_string(),
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        auto_runnable: true,
        doc_link: None,
    }
}

fn guidance(display: &str, doc: &str) -> FixCommand {
    FixCommand {
        display: display.to_string(),
        program: String::new(),
        args: Vec::new(),
        auto_runnable: false,
        doc_link: Some(doc.to_string()),
    }
}

/// A macOS-shaped report: green core, an Android area with a runnable cargo-ndk
/// fix and a guidance-only JDK fix, green iOS, always-green Desktop.
fn partial_report() -> DoctorReport {
    DoctorReport {
        areas: vec![
            Area {
                name: "Prerequisites".to_string(),
                components: vec![comp("Rust toolchain", ComponentStatus::Ok, vec![])],
            },
            Area {
                name: "Android".to_string(),
                components: vec![
                    comp("Android Rust targets", ComponentStatus::Ok, vec![]),
                    comp(
                        "cargo-ndk",
                        ComponentStatus::Missing,
                        vec![runnable(
                            "cargo install cargo-ndk",
                            "cargo",
                            &["install", "cargo-ndk"],
                        )],
                    ),
                    comp(
                        "JDK",
                        ComponentStatus::Missing,
                        vec![guidance(
                            "Install a JDK 17+",
                            "https://developer.android.com/studio",
                        )],
                    ),
                ],
            },
            Area {
                name: "iOS".to_string(),
                components: vec![comp("Xcode", ComponentStatus::Ok, vec![])],
            },
            Area {
                name: "Desktop".to_string(),
                components: vec![comp("Desktop preview", ComponentStatus::Ok, vec![])],
            },
        ],
    }
}

fn all_green_report() -> DoctorReport {
    DoctorReport {
        areas: vec![
            Area {
                name: "Prerequisites".to_string(),
                components: vec![comp("Rust toolchain", ComponentStatus::Ok, vec![])],
            },
            Area {
                name: "Android".to_string(),
                components: vec![comp("cargo-ndk", ComponentStatus::Ok, vec![])],
            },
            Area {
                name: "Desktop".to_string(),
                components: vec![comp("Desktop preview", ComponentStatus::Ok, vec![])],
            },
        ],
    }
}

fn missing_core_report() -> DoctorReport {
    let mut r = partial_report();
    r.areas[0].components[0] = comp(
        "Rust toolchain",
        ComponentStatus::Missing,
        vec![guidance("Install Rust via rustup", "https://rustup.rs")],
    );
    r
}

/// Open the bootstrap wizard over the workbench from `report`, with the step
/// cursor at `cursor`.
fn bootstrap_state(report: DoctorReport, cursor: usize) -> AppState {
    let mut wizard = BootstrapWizard::from_report(report.clone());
    wizard.cursor = cursor;
    let mut state = workbench_state();
    state.bootstrap = BootstrapState {
        report: Some(report),
        refreshing: false,
        auto_shown: true,
    };
    state.bootstrap_wizard = Some(wizard);
    state
}

/// The wizard over an all-green toolchain (rollup Ok), cursor on the core step.
#[test]
fn bootstrap_wizard_green_100x30() {
    insta::assert_snapshot!(render_to_string(
        100,
        30,
        &bootstrap_state(all_green_report(), 0)
    ));
}

/// The wizard on the Android step (cursor 2), a runnable cargo-ndk fix selected
/// — the "▶ Run in session" affordance is live.
#[test]
fn bootstrap_wizard_partial_android_100x30() {
    insta::assert_snapshot!(render_to_string(
        100,
        30,
        &bootstrap_state(partial_report(), 2)
    ));
}

/// The wizard with a Missing core (Rust toolchain), cursor on the core step —
/// the gating case that blocks handback.
#[test]
fn bootstrap_wizard_missing_core_100x30() {
    insta::assert_snapshot!(render_to_string(
        100,
        30,
        &bootstrap_state(missing_core_report(), 0)
    ));
}

/// The titlebar chip reflecting a Partial report rollup (D6a) — the chip's real
/// source once a component report is cached.
#[test]
fn toolchain_chip_partial_from_report_100x30() {
    let mut state = workbench_state();
    state.bootstrap = BootstrapState {
        report: Some(partial_report()),
        refreshing: false,
        auto_shown: true,
    };
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// A completed build session's log view, showing the "N built · c copy path"
/// segment in the log status row — wide enough (130 cols) for the segment to
/// fit beside the right-aligned keyhint.
#[test]
fn session_log_built_artifacts_130x30() {
    insta::assert_snapshot!(render_to_string(130, 30, &built_session_state()));
}

// ── Command palette + toasts (D5) ────────────────────────────────────────────

/// The command palette open over the workbench with a "run" query: the ranked
/// list is filtered/scored, the top row selected, and disabled commands (no
/// session/devices) show their reason.
#[test]
fn palette_open_query_100x30() {
    let mut state = workbench_state();
    state.palette = Some(Palette {
        query: "run".to_string(),
        cursor: 0,
    });
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// A stack of toasts (one per kind) floating above the workbench status bar —
/// the auto-dismiss notice layer (rendered here at full TTL).
#[test]
fn toasts_stack_100x30() {
    let mut state = workbench_state();
    state.toasts.push(ToastKind::Success, "desktop: finished");
    state.toasts.push(ToastKind::Warn, "Pixel 7: stopped");
    state.toasts.push(ToastKind::Error, "build apk: failed");
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The same built session at the standard 100-col width: the built-artifacts
/// segment doesn't fit beside the keyhint, so it's omitted entirely (`c`
/// still copies via the keyboard) rather than overlapping/garbling either —
/// see `render_log_status`'s fit guard.
#[test]
fn session_log_built_artifacts_narrow_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &built_session_state()));
}

// ── Context menus + drag-to-resize (T04 / D4) ────────────────────────────────

/// A right-click context menu open over a session tab: the popup floats on the
/// top z-layer with target-specific entries (Select / Stop / Follow / Copy
/// path), the disabled Copy row muted; the workbench base layer stays visible
/// beneath it.
#[test]
fn context_menu_session_tab_100x30() {
    let mut state = single_session_state();
    update(
        &mut state,
        Message::OpenContextMenu {
            x: 34,
            y: 5,
            target: ContextTarget::SessionTab(0),
        },
    );
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The workbench with a drag-widened sidebar (T04 splitter): the sidebar
/// occupies more columns and the main area reflows to the narrower remainder,
/// exercising `sidebar_main_at` with a non-default width.
#[test]
fn workbench_resized_sidebar_100x30() {
    let mut state = workbench_state();
    state.sidebar_width = 40;
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// A session whose log overflows the viewport: the log-view scrollbar thumb
/// (T04) appears in the rightmost column, positioned near the bottom while the
/// view follows the tail. Zero-noise on a short log (see the other session
/// snapshots, which show no thumb).
#[test]
fn session_log_scrollbar_thumb_100x30() {
    let root = "/tmp/huddle";
    let owned: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
    let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
    let sess = session(0, root, "desktop", SessionState::Running, &borrowed);
    let state = AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(root)),
        projects: vec![PathBuf::from(root)],
        sessions: vec![sess],
        active_session: Some(0),
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}
