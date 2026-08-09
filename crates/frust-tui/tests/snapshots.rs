//! TestBackend + insta snapshot suite for the render layer. Snapshots are the
//! plain-text cell grid (symbols only) so they are terminal- and
//! color-depth-independent; hover/pressed states are made text-visible where
//! they matter.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use frust_drive::devices::{Device, Kind, Platform};
use frust_drive::doctor::{Area, Component, ComponentStatus, DoctorReport, FixCommand, Status};
use frust_drive::plugin::{AddItem, AddOutcome, AddReport};
use frust_tui::engine::{
    AddPluginDialog, AddPluginStep, AppState, BootstrapState, BootstrapWizard, BuildLauncher,
    ContextTarget, CreateWizard, DeviceRow, DoctorCheck, DoctorState, LevelFilter, Message,
    Palette, RegionId, RunConfig, RunFocus, Screen, Scroll, SessionView, ToastKind, WizardStep,
    update,
};
use frust_tui::supervise::{PhaseLabel, SessionEvent, SessionEventKind, SessionId, SessionState};
use frust_tui::ui::mouse::{MouseCtx, MouseRegions};
use frust_tui::ui::theme::{ColorDepth, Theme};

/// Render `state` at `w`x`h` and return the cell-grid text, at `TrueColor`
/// depth (every existing snapshot's depth).
fn render_to_string(w: u16, h: u16, state: &AppState) -> String {
    render_to_string_at(w, h, state, ColorDepth::TrueColor)
}

/// [`render_to_string`] at an explicit [`ColorDepth`] — used by the shimmer
/// color-depth-degrade snapshot below. The cell grid captures symbols only
/// (see the module doc), so a non-`TrueColor` depth only actually matters
/// here insofar as it exercises the render path without panicking; the
/// per-depth color behavior itself is unit-tested in
/// `crates/frust-tui/src/ui/anim/shimmer.rs`.
fn render_to_string_at(w: u16, h: u16, state: &AppState, depth: ColorDepth) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::frust_dark_at(depth);
    let mut regions = MouseRegions::new();
    terminal
        .draw(|frame| {
            let mut ctx = MouseCtx::new(&mut regions);
            frust_tui::ui::render(frame, state, &theme, &mut ctx);
        })
        .expect("draw");
    buffer_to_string(terminal.backend().buffer())
}

/// Blank out every `HH:MM:SS`-shaped substring in `s` — the log view's
/// per-line timestamp column is stamped from the real wall clock
/// ([`frust_tui::engine::logstyle::now_hms`]) when a test goes through the
/// real [`SessionView::push_line`]/[`update`] path rather than the
/// deterministic [`SessionView::push_line_at`] seam, so a snapshot exercising
/// that path must redact it to stay reproducible across runs/days.
fn redact_clock(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let looks_like_clock = i + 8 <= chars.len()
            && chars[i].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
            && chars[i + 2] == ':'
            && chars[i + 3].is_ascii_digit()
            && chars[i + 4].is_ascii_digit()
            && chars[i + 5] == ':'
            && chars[i + 6].is_ascii_digit()
            && chars[i + 7].is_ascii_digit();
        if looks_like_clock {
            out.push_str("--:--:--");
            i += 8;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
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

/// Multiple detected projects — the sidebar lists every one, the active
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

/// The titlebar project-switcher dropdown open over a multi-project
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

/// A session on `project`/`target`, seeded with `lines`, in `state`. Each
/// line is pushed with a deterministic synthetic timestamp
/// (`push_line_at`, never the real wall clock via `push_line`) — the log
/// view now renders a timestamp column on every line (workbook §B11), and a
/// snapshot must never depend on the moment the test happened to run.
fn session(id: u64, project: &str, target: &str, s: SessionState, lines: &[&str]) -> SessionView {
    let mut sv = SessionView::new(SessionId(id), PathBuf::from(project), target);
    sv.state = s;
    for (i, l) in lines.iter().enumerate() {
        sv.push_line_at((*l).to_string(), format!("12:00:{:02}", i % 60));
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

/// Goes through the real `update()` path — `Message::RegisterSession`
/// followed by several `Message::Session(Lines(..))` batches, as the
/// supervisor would deliver them — rather than constructing a `SessionView`
/// directly like every other session snapshot above. This is the regression
/// coverage for the seeded-`Anchored(0)` follow bug: a session used to be
/// seeded not-following on an empty log (honoring a persisted global
/// default), which was indistinguishable from a user having frozen at line 1
/// once the first line landed — the render window then stuck at one row
/// forever. With per-session follow there is no such seed: the rendered
/// window must track the tail as lines arrive.
#[test]
fn registered_session_tracks_tail_through_update_100x30() {
    let mut state = workbench_state();
    let id = SessionId(0);
    update(
        &mut state,
        Message::RegisterSession {
            id,
            project_root: PathBuf::from("/tmp/huddle"),
            target_label: "desktop".to_string(),
        },
    );
    for batch in 0..5 {
        let lines = (0..20)
            .map(|i| format!("line {}", batch * 20 + i))
            .collect();
        update(
            &mut state,
            Message::Session(SessionEvent {
                id,
                kind: SessionEventKind::Lines(lines),
            }),
        );
    }
    assert!(
        state.active_session().unwrap().is_following(),
        "a never-scrolled session stays in Follow through incoming lines"
    );
    // This test goes through the real `update()`/`push_line` path (the real
    // wall clock), unlike every other session snapshot's `session()` helper
    // — redact the per-line timestamp column so the snapshot stays
    // reproducible across runs/days.
    insta::assert_snapshot!(redact_clock(&render_to_string(100, 30, &state)));
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

// ── Build-phase progress (workbook §B10) ─────────────────────────────────────

/// A `Building` session with a parsed phase label (`animation_frame` pinned
/// for a deterministic spinner/shimmer-sweep frame): the tab glyph spins, and
/// the transient status line under the tab bar shows the shimmered
/// `Compiling frust-core (41/210)` label instead of a plain fallback.
#[test]
fn session_build_phase_known_100x30() {
    let root = "/tmp/huddle";
    let mut sess = session(
        0,
        root,
        "Pixel 7",
        SessionState::Building,
        &["   Compiling frust-core v0.3.1 (41/210)"],
    );
    sess.current_phase = Some(PhaseLabel::Compiling {
        crate_name: "frust-core".to_string(),
        progress: Some((41, 210)),
    });
    let state = AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(root)),
        projects: vec![PathBuf::from(root)],
        sessions: vec![sess],
        active_session: Some(0),
        animation_frame: 4,
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// Same scene as [`session_build_phase_known_100x30`], rendered at
/// `ColorDepth::Xterm256` — the depth `themed_shimmer_spans` degrades to a
/// ramp-bucketed sweep for (fixes review Major M4, see
/// `crates/frust-tui/src/ui/anim/shimmer.rs`). The cell grid captures
/// symbols only (module doc), so this snapshot's text is expected to be
/// byte-identical to the TrueColor one — it exists to prove the render path
/// doesn't panic/differ structurally at this depth; the actual per-depth
/// color behavior is unit-tested in `shimmer.rs`.
#[test]
fn session_build_phase_known_xterm256_100x30() {
    let root = "/tmp/huddle";
    let mut sess = session(
        0,
        root,
        "Pixel 7",
        SessionState::Building,
        &["   Compiling frust-core v0.3.1 (41/210)"],
    );
    sess.current_phase = Some(PhaseLabel::Compiling {
        crate_name: "frust-core".to_string(),
        progress: Some((41, 210)),
    });
    let state = AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(root)),
        projects: vec![PathBuf::from(root)],
        sessions: vec![sess],
        active_session: Some(0),
        animation_frame: 4,
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string_at(100, 30, &state, ColorDepth::Xterm256));
}

/// The pre-first-line placeholder (workbook §B10): a transient session with
/// no output at all yet shows a centered spinner + "waiting for first output"
/// message in the log pane, in place of the ordinary top-left log hint.
#[test]
fn session_empty_log_transient_placeholder_100x30() {
    let root = "/tmp/huddle";
    let sess = session(0, root, "Pixel 7", SessionState::Building, &[]);
    let state = AppState {
        screen: Screen::Workbench,
        project_root: Some(PathBuf::from(root)),
        projects: vec![PathBuf::from(root)],
        sessions: vec![sess],
        active_session: Some(0),
        animation_frame: 4,
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Devices panel + run-config modal ─────────────────────────────────────────

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

// ── Create-project wizard ────────────────────────────────────────────────────

/// A wizard on `step`, with `name`/`directory` filled and `arch_cursor` set.
/// `clean_signals` is threaded through `set_clean_signals_available` for
/// completeness, but no arch card is sibling-gated today (`clean-signals` is
/// git+rev-pinned to its public repo), so it has no visible effect.
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

/// The architecture step: the clean-signals card is selectable (no sibling
/// checkout gates it — `clean-signals` is git+rev-pinned to its public repo).
/// One snapshot covers both `clean_signals` probe outcomes since
/// neither changes the rendering — the former sibling-absent/-present pair
/// collapsed into this single case when the gating was retired.
#[test]
fn wizard_arch_step_clean_signals_enabled_80x24() {
    insta::assert_snapshot!(render_to_string(
        80,
        24,
        &wizard_state(WizardStep::Arch, "my_app", true, 1)
    ));
}

// ── Add plugin dialog ────────────────────────────────────────────────────────

/// An Add Plugin dialog over a workbench, on `step`, with the sibling probe
/// resolved to `sibling_available` and the given `cursor`. `sibling_available`
/// is threaded through `set_sibling_available` for completeness, but no
/// registry entry is sibling-gated today (`clean-signals-frust` was the sole
/// `requires_sibling` user before `clean-signals` moved to a git+rev pin),
/// so it has no visible effect.
fn add_plugin_state(step: AddPluginStep, sibling_available: bool, cursor: usize) -> AppState {
    let mut dialog = AddPluginDialog::new(PathBuf::from("/tmp/huddle"));
    dialog.set_sibling_available(sibling_available);
    dialog.cursor = cursor;
    dialog.step = step;
    let mut state = workbench_state();
    state.add_plugin = Some(dialog);
    state
}

/// The select step: every registry card is selectable, including
/// clean-signals-frust — no card is sibling-gated (see `add_plugin_state`'s
/// doc). This snapshot replaces the former disabled-card case: that scenario
/// can no longer occur since clean-signals-frust's registry entry dropped
/// its sibling requirement.
#[test]
fn add_plugin_select_step_100x30() {
    insta::assert_snapshot!(render_to_string(
        100,
        30,
        &add_plugin_state(AddPluginStep::Select, false, 0)
    ));
}

/// The options step for secure-storage, showing its `[ ]` biometric-gate
/// feature checkbox.
#[test]
fn add_plugin_options_checkboxes_100x30() {
    let mut state = workbench_state();
    let mut dialog = AddPluginDialog::new(PathBuf::from("/tmp/huddle"));
    dialog.set_sibling_available(true);
    // Select secure-storage and advance into its options step.
    let idx = dialog
        .entries
        .iter()
        .position(|e| e.id == "secure-storage")
        .unwrap();
    dialog.cursor = idx;
    dialog.advance();
    dialog.toggle_feature(); // check the biometric gate so a `[✓]` shows
    state.add_plugin = Some(dialog);
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The report step, a mix of Applied / AlreadyPresent line items.
#[test]
fn add_plugin_report_100x30() {
    let mut state = workbench_state();
    let mut dialog = AddPluginDialog::new(PathBuf::from("/tmp/huddle"));
    dialog.succeed(AddReport {
        plugin_id: "secure-storage".to_string(),
        items: vec![
            AddItem {
                description: "Cargo.toml dependency `frust-secure-storage`".to_string(),
                outcome: AddOutcome::Applied,
            },
            AddItem {
                description: "AndroidManifest.xml permission `android.permission.USE_BIOMETRIC`"
                    .to_string(),
                outcome: AddOutcome::Applied,
            },
            AddItem {
                description: "Info.plist key `NSFaceIDUsageDescription`".to_string(),
                outcome: AddOutcome::AlreadyPresent,
            },
        ],
    });
    state.add_plugin = Some(dialog);
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Doctor panel + titlebar chip ──────────────────────────────────────────────

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

// ── Build launcher ────────────────────────────────────────────────────────────

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

// ── Clean confirm dialog ──────────────────────────────────────────────────────

/// The clean-confirm dialog open over the workbench.
#[test]
fn clean_confirm_100x30() {
    let mut state = workbench_state();
    state.clean_confirm = state.project_root.clone();
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Build artifact copy-path ──────────────────────────────────────────────────

fn built_session_state() -> AppState {
    let root = "/tmp/huddle";
    let mut sess = session(
        0,
        root,
        "build apk",
        SessionState::Exited(true),
        &["Building `it.f0x.huddle`…"],
    );
    sess.push_line_at(
        "Built: /tmp/huddle/android/app/build/outputs/apk/release/app.apk".to_string(),
        "12:00:01",
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

// ── Bootstrap wizard + toolchain chip ─────────────────────────────────────────

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

/// The titlebar chip reflecting a Partial report rollup — the chip's real
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

// ── Command palette + toasts ──────────────────────────────────────────────────

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

// ── Context menus + drag-to-resize ────────────────────────────────────────────

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

/// The workbench with a drag-widened sidebar (drag-to-resize splitter): the
/// sidebar occupies more columns and the main area reflows to the narrower
/// remainder, exercising `sidebar_main_at` with a non-default width.
#[test]
fn workbench_resized_sidebar_100x30() {
    let mut state = workbench_state();
    state.sidebar_width = 40;
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

// ── Help overlay ───────────────────────────────────────────────────────────────

/// The keyboard/help overlay open over the workbench, listing every
/// palette-sourced command's keyhint (single source of truth).
#[test]
fn help_overlay_workbench_100x30() {
    let mut state = workbench_state();
    state.help_open = true;
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The same overlay reachable from the welcome screen too (`?` works from
/// either top-level screen, like the bootstrap/create wizards).
#[test]
fn help_overlay_welcome_80x24() {
    let state = AppState {
        help_open: true,
        ..Default::default()
    };
    insta::assert_snapshot!(render_to_string(80, 24, &state));
}

// ── Perf sparkline panel ──────────────────────────────────────────────────────

/// A session with `frust-perf raw`/`frame`/`startup` lines already parsed and
/// the panel toggled open: sparkline + p50/p95 stats + the startup summary.
#[test]
fn session_perf_panel_100x30() {
    let root = "/tmp/huddle";
    let mut sess = session(
        0,
        root,
        "desktop",
        SessionState::Running,
        &["app: booting up"],
    );
    sess.push_line_at(
        "frust-perf startup app_created=2ms first_frame_presented=45ms".to_string(),
        "12:00:01",
    );
    for i in 0..40 {
        sess.push_line_at(
            format!(
                "frust-perf raw n={i} total_us={} rebuild_us=200 layout_us=300 paint_us=400 \
                 encode_us=500 present_us=600 skipped=0",
                2000 + i * 37 % 4000
            ),
            format!("12:00:{:02}", (i + 2) % 60),
        );
    }
    sess.push_line_at(
        "frust-perf frame n=40 total_p50_ms=8 total_p95_ms=14 total_p99_ms=22 \
         rebuild_p95_ms=1 layout_p95_ms=2 paint_p95_ms=2 encode_p95_ms=3 present_p95_ms=2 \
         over_60hz=1 over_120hz=5 skipped=0 total_frames=40"
            .to_string(),
        "12:00:42",
    );
    sess.perf.toggle();
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

/// The same session with perf data present but the panel *not* toggled
/// open — zero-noise: no sparkline row, no `t perf` hint change beyond the
/// keyhint itself (the row reservation is absent).
#[test]
fn session_perf_data_present_but_panel_closed_100x30() {
    let root = "/tmp/huddle";
    let mut sess = session(0, root, "desktop", SessionState::Running, &[]);
    sess.push_line_at(
        "frust-perf frame n=1 total_p50_ms=8 total_p95_ms=14 total_p99_ms=22 skipped=0 \
         total_frames=1"
            .to_string(),
        "12:00:01",
    );
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

// ── Responsive breakpoints ────────────────────────────────────────────────────

/// A narrow (< `NARROW_WIDTH`) terminal: the sidebar collapses out of the
/// inline layout entirely, the main area (and, once open, a session log
/// view) taking the full width; the status bar carries the `s sidebar`
/// keyhint that only shows once narrow.
#[test]
fn workbench_narrow_sidebar_collapsed_70x24() {
    insta::assert_snapshot!(render_to_string(70, 24, &workbench_state()));
}

/// The same narrow terminal with the sidebar overlay toggled open: a
/// floating, bordered panel over the (now non-interactive) main area.
#[test]
fn workbench_narrow_sidebar_overlay_open_70x24() {
    let mut state = workbench_state();
    state.sidebar_overlay_open = true;
    insta::assert_snapshot!(render_to_string(70, 24, &state));
}

/// A session whose log overflows the viewport: the log-view scrollbar thumb
/// appears in the rightmost column, positioned near the bottom while the
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

// ── Log styling: levels/sources/timestamps, backtrace folding, level filter
// (workbook §B11) ─────────────────────────────────────────────────────────

/// One line per level (error/warn/info/debug) crossed with a source
/// (cargo/rustc, Gradle, logcat, `frust-perf`, plain app output) — the
/// badge/timestamp/source-tag prefix grammar and the ANSI-passthrough rule
/// (the last line carries its own ANSI color and must render in *that*
/// color, untouched by the level tint).
fn log_styling_state() -> AppState {
    let root = "/tmp/huddle";
    let mut sess = SessionView::new(SessionId(0), PathBuf::from(root), "desktop");
    let lines: &[(&str, &str)] = &[
        (
            "12:04:22",
            "[gradle] FAILURE: Build failed with an exception.",
        ),
        (
            "12:04:24",
            "E/ActivityThread( 1234): Failed to find provider info",
        ),
        ("12:04:25", "Session ready, listening on :5037"),
        (
            "12:04:26",
            "2026-08-09T12:04:26Z DEBUG frust_tui: poll: 0 pending signals",
        ),
        (
            "12:04:27",
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 \
             encode_us=10 present_us=10 skipped=0",
        ),
        (
            "12:04:28",
            "\u{1b}[36mconnected\u{1b}[0m to ws://127.0.0.1:9229",
        ),
    ];
    for (ts, l) in lines {
        sess.push_line_at((*l).to_string(), (*ts).to_string());
    }
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
fn log_styling_all_levels_and_sources_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &log_styling_state()));
}

/// A session with a Rust panic + backtrace, seeded with the exact shape
/// `RUST_BACKTRACE=1` produces. The block starts folded by default (`start`
/// == absolute index `0`, the panic-header line).
fn panic_backtrace_state() -> AppState {
    let root = "/tmp/huddle";
    let mut sess = SessionView::new(SessionId(0), PathBuf::from(root), "desktop");
    let lines: &[(&str, &str)] = &[
        ("12:05:02", "thread 'main' panicked at src/main.rs:42:9:"),
        ("12:05:02", "called `Option::unwrap()` on a `None` value"),
        ("12:05:02", "stack backtrace:"),
        ("12:05:02", "   0: my_app::state::reduce"),
        ("12:05:02", "             at src/state.rs:88:13"),
        ("12:05:02", "   1: my_app::widget::on_tap"),
        ("12:05:02", "             at src/widget.rs:206:21"),
        ("12:05:03", "app: recovering"),
    ];
    for (ts, l) in lines {
        sess.push_line_at((*l).to_string(), (*ts).to_string());
    }
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
fn log_styling_panic_backtrace_folded_100x30() {
    insta::assert_snapshot!(render_to_string(100, 30, &panic_backtrace_state()));
}

#[test]
fn log_styling_panic_backtrace_expanded_100x30() {
    let mut state = panic_backtrace_state();
    // The block's id is the panic-header line's absolute index (`0`) — the
    // same key a fold-row click (`Message::ToggleFold`) or `z`
    // (`Message::ToggleNearestFold`) would toggle.
    state.sessions[0].toggle_fold(0);
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The level-filter chip active at `warn+`: info lines are hidden, the
/// active segment is accent-filled, and the hidden count shows beside it.
/// Wide enough (180 cols) for the chip to fit beside the right-aligned
/// keyhint — see `render_log_status`'s fit guard (the same one the
/// built-artifacts segment uses).
#[test]
fn log_styling_level_filter_active_180x30() {
    let mut state = log_styling_state();
    state.sessions[0].set_level_filter(LevelFilter::WarnPlus);
    insta::assert_snapshot!(render_to_string(180, 30, &state));
}

/// The same active filter + hidden lines at 100 columns — too narrow for the
/// full segmented pill (that needs ~135 cols; see `render_log_status`'s
/// graduated chip degrade), so the chip renders in its minimal marker form
/// instead of vanishing outright: an active filter must never go invisible
/// (workbook §B11's binding note — the hidden-line count stays visible next
/// to the chip so filtering never silently hides lines without a trace).
#[test]
fn log_styling_level_filter_degraded_100x30() {
    let mut state = log_styling_state();
    state.sessions[0].set_level_filter(LevelFilter::WarnPlus);
    insta::assert_snapshot!(render_to_string(100, 30, &state));
}

/// The degraded (minimal-form) chip is still a real click target: clicking
/// it registers the same `Message::CycleLevelFilter(1)` the `l` key sends —
/// mouse parity holds even in the narrowest form (see
/// `render_log_status`'s graduated chip degrade; `RegionId::LevelFilterChip`
/// is the degraded forms' shared region id, distinct from the full pill's
/// per-segment `RegionId::LevelFilterSegment`).
#[test]
fn log_styling_level_filter_degraded_chip_click_cycles_the_filter() {
    let mut state = log_styling_state();
    state.sessions[0].set_level_filter(LevelFilter::WarnPlus);

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::frust_dark_at(ColorDepth::TrueColor);
    let mut regions = MouseRegions::new();
    terminal
        .draw(|frame| {
            let mut ctx = MouseCtx::new(&mut regions);
            frust_tui::ui::render(frame, &state, &theme, &mut ctx);
        })
        .expect("draw");

    // The minimal chip's `⏷warn+ ·4` marker renders on the log status row —
    // located here rather than hardcoded, so a chrome-width change elsewhere
    // on the row can't silently make this test click the wrong cell.
    let text = buffer_to_string(terminal.backend().buffer());
    let (x, y) = text
        .lines()
        .enumerate()
        .find_map(|(y, line)| {
            line.chars()
                .position(|c| c == '\u{23f7}')
                .map(|x| (x as u16, y as u16))
        })
        .expect("the degraded chip's caret glyph is on screen");

    assert_eq!(regions.hover_at(x, y), Some(RegionId::LevelFilterChip));
    assert_eq!(regions.click_at(x, y), Some(Message::CycleLevelFilter(1)));
}
