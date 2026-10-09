//! The fuzzy command palette: a single
//! `Ctrl+P` / `:` launcher covering every workbench command, each carrying its
//! enabled/disabled-with-reason gate (the workbook disabled pattern).
//!
//! Everything here is plain data + pure functions: the [`Palette`] holds only
//! the live query + selection cursor, and [`commands`]/[`ranked`] derive the
//! command list *from* [`AppState`] on demand — a command is one `title`, one
//! keyhint, the **existing** [`Message`] it emits, and its enabled gate. The
//! palette NEVER duplicates command logic: executing an entry re-dispatches its
//! `Message` through the same `update` every other input path uses (see
//! `super::update::execute_palette`). `crate::ui::views::palette` renders it.

use super::message::Message;
use super::state::AppState;

/// The `(palette-open hint, mouse-toggle hint)` pair for a host, keyed off
/// `is_macos` so both arms are directly unit-testable without depending on
/// the machine actually running the test. macOS keeps the existing `⌘`/`⌥`
/// symbols; every other host spells the modifier out — `⌘`/`⌥` aren't
/// physical keys there — using the same `^X` notation the UI already uses
/// for Ctrl bindings (e.g. `^O`, the "Switch project…" hint below).
///
/// `pub(crate)` rather than private: [`crate::ui::theme::Theme`] is the glyph
/// set's carrier (see the module doc on [`commands`]'s "Toggle mouse
/// capture" row), so it calls this directly rather than the engine tracking
/// host identity for the UI layer.
pub(crate) fn key_glyphs_for(is_macos: bool) -> (&'static str, &'static str) {
    if is_macos {
        ("⌘ palette", "⌥m")
    } else {
        ("^P palette", "Alt+m")
    }
}

/// The open command palette's live state: the typed query and the highlighted
/// row (an index into [`ranked`], clamped). Opening it primes an empty query so
/// the full registry shows in its natural order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Palette {
    /// The live fuzzy query.
    pub query: String,
    /// The highlighted row (index into the ranked results, clamped).
    pub cursor: usize,
}

impl Palette {
    /// A freshly-opened palette (empty query, top selection).
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a character to the query, resetting the selection to the top (the
    /// ranking changes, so the old cursor is meaningless).
    pub fn input_char(&mut self, c: char) {
        self.query.push(c);
        self.cursor = 0;
    }

    /// Delete the last query character, resetting the selection to the top.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.cursor = 0;
    }
}

/// One palette command: a display title, a short keyhint, the **existing**
/// [`Message`] it emits, and its enabled/disabled-with-reason gate. A disabled
/// command still shows (muted, with its reason) but can't be executed.
///
/// `PartialEq` only, following [`Message`]'s own relaxation.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteCommand {
    /// The row label.
    pub title: &'static str,
    /// A short keybinding / context hint shown right-aligned.
    pub hint: &'static str,
    /// The message executing this row emits — routed through `update` unchanged.
    pub message: Message,
    /// Whether the command is currently runnable.
    pub enabled: bool,
    /// Why it's disabled (shown beside a disabled row), or `None` when enabled.
    pub disabled_reason: Option<&'static str>,
}

/// The full command registry, each gated against the current `state` (the
/// workbook disabled pattern): session commands need a session, run/build/clean
/// need an open project, "run on all devices" additionally needs discovered
/// devices. The order here is the tie-break order [`ranked`] falls back to for
/// equal fuzzy scores.
///
/// Deliberately omitted (no existing `Message` to wire to): "open project by
/// path" (needs a path-input flow not in the tree) and a "help" overlay (not
/// yet landed). Each returns with no palette entry rather than a dead command.
/// The mouse-capture toggle is now wired below.
pub fn commands(state: &AppState) -> Vec<PaletteCommand> {
    let has_project = state.project_root.is_some();
    let has_devices = !state.devices.is_empty();
    let has_session = state.active_session().is_some();
    // `R` restarts an app session and refreshes devices everywhere else
    // (`crate::runner`'s `translate_key`), so exactly one of the two rows
    // below carries the `R` hint at a time.
    let has_app_session = state.active_session().is_some_and(|s| s.target.is_some());
    // "Watch: hot patch on save" is the `frust run --watch` loop, which runs
    // on the desktop preview, Android devices and iOS simulators — a
    // physical iOS device (or ad-hoc) session gates it off.
    let has_watchable_session = state.active_session().is_some_and(|s| {
        s.target
            .as_ref()
            .is_some_and(super::session_view::SessionTarget::supports_watch)
    });
    let has_selection = state
        .active_session()
        .is_some_and(|s| s.selection.is_some());
    let running = state
        .active_session()
        .is_some_and(|s| !s.state.is_terminal());
    let has_projects = !state.projects.is_empty();
    // The Inspector's own refresh only means anything with its tab open on a
    // live connection (§B12) — everywhere else `r` is a different command.
    let inspector_live = state.active_session().is_some_and(|s| {
        s.devtools.open
            && s.devtools.active_tab == super::devtools::DevtoolsTab::Inspector
            && s.devtools.phase() == super::devtools::DevtoolsPhase::Connected
    });

    let gated = |title, hint, message, enabled, reason: &'static str| PaletteCommand {
        title,
        hint,
        message,
        enabled,
        disabled_reason: (!enabled).then_some(reason),
    };
    let always = |title, hint, message| PaletteCommand {
        title,
        hint,
        message,
        enabled: true,
        disabled_reason: None,
    };

    vec![
        gated(
            "Run on device(s)…",
            "r",
            Message::OpenRunConfig,
            has_project,
            "open a project first",
        ),
        gated(
            "Run on all devices",
            "",
            Message::RunOnAllDevices,
            has_project && has_devices,
            if has_project {
                "no devices discovered"
            } else {
                "open a project first"
            },
        ),
        gated(
            "Stop session",
            "x",
            Message::StopSession,
            running,
            "no running session",
        ),
        // The keyboard twin of MCP `restart_app` / DAP `frustRestart` — an
        // app session only (`target.is_some()`), unlike "Stop session" above,
        // which cares only whether it's still running: a restart needs a
        // retained launch spec to relaunch, which only an app session has.
        gated(
            "Restart session",
            "R",
            Message::RestartSession,
            has_app_session,
            "no session to restart",
        ),
        // A toggle: the same row turns it off again. Enabled only for a
        // desktop, Android or iOS simulator app session
        // (`Message::ToggleWatch`'s own refusal covers the `W` key on
        // anything else). A save hot-patches a session that runs hot and
        // restarts any other.
        gated(
            "Watch: hot patch on save",
            "W",
            Message::ToggleWatch,
            has_watchable_session,
            "desktop, Android and iOS simulator app sessions only",
        ),
        gated(
            "Close tab",
            "X",
            Message::CloseActiveTab,
            has_session,
            "no active session",
        ),
        gated(
            "Build…",
            "b",
            Message::OpenBuildLauncher,
            has_project,
            "open a project first",
        ),
        gated(
            "Clean…",
            "c",
            Message::OpenCleanConfirm,
            has_project,
            "open a project first",
        ),
        // Doctor panel is bound to `i` (from either screen); toolchain
        // setup moved off `i` onto the panel's own `t` key / "Toolchain
        // setup" button (see `views::doctor::render`), so it keeps a palette
        // row but no top-level keyhint of its own.
        always("Doctor", "i", Message::OpenDoctorPanel),
        always("Toolchain setup…", "", Message::OpenBootstrapWizard),
        always("New project…", "n", Message::OpenCreateWizard),
        gated(
            "Add plugin…",
            "a",
            Message::OpenAddPlugin,
            has_project,
            "open a project first",
        ),
        gated(
            "Switch project…",
            "^O",
            Message::ToggleProjectSwitcher,
            has_projects,
            "no projects detected",
        ),
        always(
            "Refresh devices",
            if has_app_session { "" } else { "R" },
            Message::RefreshDevices,
        ),
        // Workbook §B13. Both are always enabled: the embedded server serves
        // *this* workbench whether or not a project is open (a `run_app` with
        // none open reports that itself), and the panel is readable in every
        // state, including "not running".
        always("MCP server…", "m", Message::OpenMcpPanel),
        always("Start/stop MCP server", "M", Message::ToggleMcpServer),
        // The DAP pair. The dialog is the whole surface (server switch,
        // preferences, IDE config), so `D` opens it rather than toggling; the
        // toggle stays reachable from here (and from inside the dialog, `s`)
        // without claiming a second top-level key.
        always("DAP server…", "D", Message::OpenDapSettings),
        always("Start/stop DAP server", "", Message::ToggleDapServer),
        always(
            "Toggle mouse capture",
            // The registry itself carries only the host-honest default (see
            // `key_glyphs_for`'s doc); a render-time caller that needs the
            // exact glyph set an in-hand `Theme` picked (e.g. the help
            // overlay, so its snapshot fixtures stay host-independent)
            // substitutes this row's hint from the theme instead of reading
            // it here — see `crate::ui::views::help::render`.
            key_glyphs_for(cfg!(target_os = "macos")).1,
            Message::ToggleMouseCapture,
        ),
        gated(
            "Toggle follow-tail",
            "f",
            Message::ToggleFollow,
            has_session,
            "no active session",
        ),
        gated(
            "Toggle line wrap",
            "w",
            Message::ToggleWrap,
            has_session,
            "no active session",
        ),
        gated(
            "Search logs…",
            "/",
            Message::SearchOpen,
            has_session,
            "no active session",
        ),
        // The line-selection pair. "Copy selection" is gated on there being a
        // selection at all rather than on a session, because `y` outside the
        // mode has nothing to copy — the palette says so instead of running a
        // command that would silently do nothing.
        //
        // "Select lines…" is *not* additionally gated on the DevTools pane
        // being closed, even though that pane owns `v`'s own key and refuses
        // the mode: the palette stays reachable over DevTools (global
        // `Ctrl+P`), and disabling this row there would hide the row's own
        // "why" from a keyboard-only user. `update`'s `Message::SelectEnter`
        // arm is the single choke point instead — it silently refuses (no
        // toast) while the active session's DevTools pane is open, so
        // executing this row there is a no-op rather than a lie.
        gated(
            "Select lines…",
            "v",
            Message::SelectEnter,
            has_session,
            "no active session",
        ),
        gated(
            "Copy selection",
            "y",
            Message::CopySelection,
            has_selection,
            "no lines selected",
        ),
        gated(
            "Cycle log level filter",
            "l",
            Message::CycleLevelFilter(1),
            has_session,
            "no active session",
        ),
        gated(
            "Toggle nearest backtrace fold",
            "z",
            Message::ToggleNearestFold,
            has_session,
            "no active session",
        ),
        // `d` is the session view's own DevTools toggle and means nothing
        // else — with no session open it claims no key at all, since
        // Doctor moved to its own unconditional `i` key.
        gated(
            "DevTools",
            "d",
            Message::DevtoolsToggle,
            has_session,
            "no active session",
        ),
        // §B12's Inspector `r`. The palette is the one place this command is
        // reachable from outside the tab itself, so its gate names the exact
        // context it needs rather than the looser "no active session".
        gated(
            "Refresh widget tree",
            "r",
            Message::DevtoolsInspectorRefresh,
            inspector_live,
            "open DevTools' Inspector tab first",
        ),
        always("Quit", "q", Message::RequestQuit),
    ]
}

/// The registry filtered to the palette's live query and ranked best-first
/// (ties fall back to registry order). An empty query keeps the whole registry
/// in its natural order. Disabled commands are ranked alongside enabled ones —
/// they show, muted, but execution is gated (see `execute_palette`).
pub fn ranked(state: &AppState) -> Vec<PaletteCommand> {
    let query = state
        .palette
        .as_ref()
        .map(|p| p.query.as_str())
        .unwrap_or("");
    let mut scored: Vec<(i32, usize, PaletteCommand)> = commands(state)
        .into_iter()
        .enumerate()
        .filter_map(|(i, cmd)| fuzzy_score(query, cmd.title).map(|score| (score, i, cmd)))
        .collect();
    // Higher score first; equal scores keep registry order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, cmd)| cmd).collect()
}

/// A simple subsequence fuzzy scorer (no dependency): `needle`'s chars must
/// appear in `haystack` in order, case-insensitively. Returns `None` on no
/// match, else a score rewarding consecutive runs, word-boundary starts, and
/// earlier matches. An empty needle matches everything at a neutral score.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let hay: Vec<char> = haystack.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i32;
    let mut ni = 0usize;
    let mut prev: Option<usize> = None;
    for (hi, &hc) in hay.iter().enumerate() {
        if ni >= needle.len() {
            break;
        }
        if hc == needle[ni] {
            score += 1;
            // Consecutive-run bonus.
            if prev == Some(hi.wrapping_sub(1)) {
                score += 5;
            }
            // Word-boundary bonus (start, or after a non-alphanumeric).
            if hi == 0 || !hay[hi - 1].is_alphanumeric() {
                score += 10;
            }
            // Earlier-position bonus — a function of the match position alone,
            // never the haystack length (so a longer title can't out-rank a
            // shorter one on an identical prefix match).
            score += (16 - (hi as i32).min(16)) / 4;
            prev = Some(hi);
            ni += 1;
        }
    }
    (ni == needle.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::state::{AppState, Screen};
    use std::path::PathBuf;

    fn workbench() -> AppState {
        let root = PathBuf::from("/tmp/huddle");
        AppState {
            screen: Screen::Workbench,
            project_root: Some(root.clone()),
            projects: vec![root],
            ..AppState::default()
        }
    }

    #[test]
    fn empty_query_matches_everything_in_registry_order() {
        let cmd = ["run", "Run on device(s)…"];
        assert_eq!(fuzzy_score("", cmd[0]), Some(0));
        let state = workbench();
        let ranked = ranked(&state);
        assert_eq!(ranked.len(), commands(&state).len());
        assert_eq!(ranked[0].title, "Run on device(s)…");
    }

    #[test]
    fn fuzzy_ranks_a_prefix_word_start_above_a_scattered_match() {
        let prefix = fuzzy_score("doc", "Doctor").unwrap();
        let scattered = fuzzy_score("doc", "Run on device(s)…").unwrap_or(i32::MIN);
        assert!(prefix > scattered, "{prefix} !> {scattered}");
    }

    #[test]
    fn fuzzy_rejects_a_non_subsequence() {
        assert_eq!(fuzzy_score("zzz", "Doctor"), None);
    }

    #[test]
    fn identical_prefix_match_is_independent_of_title_length() {
        // Two titles sharing the "Run" prefix must score identically on "run"
        // (the position bonus is length-independent), so registry order — not
        // the longer title — breaks the tie.
        let short = fuzzy_score("run", "Run on device(s)…").unwrap();
        let long = fuzzy_score("run", "Run on all devices").unwrap();
        assert_eq!(short, long);
    }

    #[test]
    fn run_query_ranks_the_enabled_primary_run_command_first() {
        let mut state = workbench();
        state.palette = Some(Palette {
            query: "run".to_string(),
            cursor: 0,
        });
        let ranked = ranked(&state);
        assert_eq!(ranked[0].title, "Run on device(s)…");
        assert!(ranked[0].enabled);
    }

    #[test]
    fn query_filters_and_ranks_by_title() {
        let mut state = workbench();
        state.palette = Some(Palette {
            query: "clean".to_string(),
            cursor: 0,
        });
        let ranked = ranked(&state);
        assert!(!ranked.is_empty());
        assert_eq!(ranked[0].title, "Clean…");
    }

    #[test]
    fn disabled_gating_reflects_state() {
        // Welcome screen: no project → run/build/clean disabled; no session →
        // stop/follow/search disabled; doctor/create/quit always enabled.
        let state = AppState::default();
        let by_title = |title: &str| {
            commands(&state)
                .into_iter()
                .find(|c| c.title == title)
                .expect("command present")
        };
        assert!(!by_title("Run on device(s)…").enabled);
        assert!(by_title("Run on device(s)…").disabled_reason.is_some());
        assert!(!by_title("Stop session").enabled);
        assert!(by_title("Doctor").enabled);
        assert!(by_title("Quit").enabled);
    }

    #[test]
    fn restart_session_row_is_enabled_only_for_an_app_session() {
        use crate::engine::{DevtoolsLaunch, SessionTarget, SessionView};
        use crate::supervise::SessionId;

        let by_title = |state: &AppState| {
            commands(state)
                .into_iter()
                .find(|c| c.title == "Restart session")
                .expect("command present")
        };

        // No session at all.
        let no_session = workbench();
        let row = by_title(&no_session);
        assert!(!row.enabled);
        assert_eq!(row.disabled_reason, Some("no session to restart"));

        // An ad-hoc session (build/clean) has no launch spec worth
        // relaunching, so it does not enable the row either.
        let mut ad_hoc = workbench();
        ad_hoc.sessions.push(SessionView::new(
            SessionId(0),
            PathBuf::from("/tmp/huddle"),
            "build",
        ));
        ad_hoc.active_session = Some(0);
        assert!(!by_title(&ad_hoc).enabled);

        // An app session (a target) enables it.
        let mut app_session = workbench();
        app_session.sessions.push(SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/huddle"),
            "desktop",
            DevtoolsLaunch::unavailable(),
            Some(SessionTarget::Desktop),
        ));
        app_session.active_session = Some(0);
        let row = by_title(&app_session);
        assert!(row.enabled);
        assert_eq!(row.message, Message::RestartSession);
    }

    #[test]
    fn watch_row_is_enabled_only_for_a_desktop_or_android_app_session() {
        use crate::engine::{DevtoolsLaunch, SessionTarget, SessionView};
        use crate::supervise::SessionId;
        use frust_drive::devices::Platform;

        let by_title = |state: &AppState| {
            commands(state)
                .into_iter()
                .find(|c| c.title == "Watch: hot patch on save")
                .expect("command present")
        };
        let with_session = |target: Option<SessionTarget>| {
            let mut state = workbench();
            state.sessions.push(SessionView::with_devtools(
                SessionId(0),
                PathBuf::from("/tmp/huddle"),
                "s",
                DevtoolsLaunch::unavailable(),
                target,
            ));
            state.active_session = Some(0);
            state
        };

        let row = by_title(&workbench());
        assert!(!row.enabled);
        assert_eq!(
            row.disabled_reason,
            Some("desktop, Android and iOS simulator app sessions only")
        );
        assert!(!by_title(&with_session(None)).enabled, "ad-hoc session");
        let ios = with_session(Some(SessionTarget::Device {
            id: "FAKE-UDID".into(),
            name: "iPhone".into(),
            platform: Platform::Ios,
        }));
        assert!(!by_title(&ios).enabled, "physical iOS device session");
        let simulator = by_title(&with_session(Some(SessionTarget::Simulator {
            id: "FAKE-UDID".into(),
            name: "iPhone 15".into(),
        })));
        assert!(simulator.enabled, "iOS simulator session");
        assert_eq!(simulator.message, Message::ToggleWatch);
        let android = by_title(&with_session(Some(SessionTarget::Device {
            id: "emu-1".into(),
            name: "Pixel".into(),
            platform: Platform::Android,
        })));
        assert!(android.enabled, "Android device session");
        assert_eq!(android.message, Message::ToggleWatch);

        let row = by_title(&with_session(Some(SessionTarget::Desktop)));
        assert!(row.enabled);
        assert_eq!(row.message, Message::ToggleWatch);
        assert_eq!(row.hint, "W");
        assert!(
            commands(&workbench())
                .iter()
                .all(|c| c.title != "Watch: restart on save"),
            "the row was renamed, not duplicated"
        );
    }

    #[test]
    fn the_r_hint_sits_on_refresh_devices_only_without_an_app_session() {
        use crate::engine::{DevtoolsLaunch, SessionTarget, SessionView};
        use crate::supervise::SessionId;

        let hint = |state: &AppState, title: &str| {
            commands(state)
                .into_iter()
                .find(|c| c.title == title)
                .expect("command present")
                .hint
        };

        let no_session = workbench();
        assert_eq!(hint(&no_session, "Refresh devices"), "R");

        let mut ad_hoc = workbench();
        ad_hoc.sessions.push(SessionView::new(
            SessionId(0),
            PathBuf::from("/tmp/huddle"),
            "build",
        ));
        ad_hoc.active_session = Some(0);
        assert_eq!(hint(&ad_hoc, "Refresh devices"), "R");

        let mut app_session = workbench();
        app_session.sessions.push(SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/huddle"),
            "desktop",
            DevtoolsLaunch::unavailable(),
            Some(SessionTarget::Desktop),
        ));
        app_session.active_session = Some(0);
        assert_eq!(hint(&app_session, "Refresh devices"), "");
        assert_eq!(hint(&app_session, "Restart session"), "R");
    }

    #[test]
    fn command_maps_to_its_existing_message() {
        let state = workbench();
        let build = commands(&state)
            .into_iter()
            .find(|c| c.title == "Build…")
            .unwrap();
        assert_eq!(build.message, Message::OpenBuildLauncher);
    }

    #[test]
    fn key_glyphs_keep_the_mac_symbols_on_macos() {
        assert_eq!(key_glyphs_for(true), ("⌘ palette", "⌥m"));
    }

    #[test]
    fn key_glyphs_spell_the_modifiers_out_off_macos() {
        // ⌘/⌥ aren't physical keys off macOS, so the hint uses the same
        // `^X` Ctrl notation already in use (`^O`) plus a spelled-out `Alt+`.
        assert_eq!(key_glyphs_for(false), ("^P palette", "Alt+m"));
    }
}
