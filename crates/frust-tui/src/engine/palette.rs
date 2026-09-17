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
        always("Doctor", "d", Message::OpenDoctorPanel),
        always("Toolchain setup…", "i", Message::OpenBootstrapWizard),
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
        always("Refresh devices", "R", Message::RefreshDevices),
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
        always("Toggle mouse capture", "⌥m", Message::ToggleMouseCapture),
        gated(
            "Toggle follow-tail",
            "f",
            Message::ToggleFollow,
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
        // `d` is the session view's own DevTools toggle and the workbench's
        // doctor panel in the *other* context (no session open) — the
        // full-screen key-namespace swap workbook §B12 defines. Both keep
        // their key here because both are only ever reachable in their own
        // context; the palette and help overlay render this one registry, so
        // the pair shows exactly as the keyboard behaves.
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
    fn command_maps_to_its_existing_message() {
        let state = workbench();
        let build = commands(&state)
            .into_iter()
            .find(|c| c.title == "Build…")
            .unwrap();
        assert_eq!(build.message, Message::OpenBuildLauncher);
    }
}
