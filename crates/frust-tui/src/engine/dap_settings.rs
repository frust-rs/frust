//! The DAP settings dialog's model: the workbench's persisted debug-adapter
//! preferences plus the focus/edit state of the dialog that shows them.
//!
//! Everything here is plain data + pure transitions — no threads, no
//! filesystem, no terminal — so the focus walk, the port validation, the IDE
//! selector and the auto-start decision are unit-testable without a TTY and
//! without an editor to be hosted by. `crate::ui::views::dap_settings` renders
//! it; `crate::runner` enacts the [`Effect`](super::update::Effect)s the pure
//! transitions request (starting the server, writing `tui.toml`, generating an
//! IDE config off-thread).
//!
//! Unlike the run-config modal, this state is **not** created when the dialog
//! opens and dropped when it closes: the preferences it holds are loaded once
//! at [`AppState::new`](super::AppState::new) and consulted at startup
//! (auto-start) and on every app launch and `DapListening` report
//! (auto-configure), whether or not anyone has opened the dialog. `AppState::dap_settings_open` is what
//! makes it a modal.

use std::path::PathBuf;

use frust_dap::ide_config::{ConfigAction, IdeConfigResult, ParentIde, WriteMode};

use super::persist::DapPrefs;

/// The explicit IDE overrides the selector offers, in cycle order after the
/// default `detected` entry.
///
/// Deliberately narrower than the full [`ParentIde`] set: this is exactly the
/// set `frust_dap::ide_config::parse_ide_name` can read back, and an override
/// that cannot survive a reload would be a setting that silently forgets
/// itself. Cursor and VS Code Insiders are absent for that reason and lose
/// nothing — both are generated for by the VS Code `launch.json` generator, so
/// picking `VS Code` writes exactly the file they read (they are still
/// *detected* under their own names, which is what the status line shows).
pub const IDE_OVERRIDES: [ParentIde; 5] = [
    ParentIde::VSCode,
    ParentIde::Neovim,
    ParentIde::Zed,
    ParentIde::Emacs,
    ParentIde::Helix,
];

/// Which control in the DAP settings dialog has keyboard focus. Ordered
/// top-to-bottom — the order [`DapSettings::focus_next`]/[`focus_prev`] walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DapFocus {
    /// The Start/Stop server action.
    Server,
    /// The port text field.
    Port,
    /// The "auto-start in an IDE terminal" checkbox.
    AutoStart,
    /// The "auto-configure the IDE" checkbox.
    AutoConfigure,
    /// The IDE selector (`←`/`→` cycles detected → each explicit override).
    Ide,
    /// The "Generate IDE config now" action.
    Generate,
}

/// The focus order, top to bottom.
const FOCUS_ORDER: [DapFocus; 6] = [
    DapFocus::Server,
    DapFocus::Port,
    DapFocus::AutoStart,
    DapFocus::AutoConfigure,
    DapFocus::Ide,
    DapFocus::Generate,
];

/// What committing the port field did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortCommit {
    /// The field parsed to the port already in effect (or nothing changed).
    Unchanged,
    /// The field parsed to a new port, now recorded (and to be persisted).
    Changed(u16),
    /// The field did not parse as a `u16`; the previous port was restored and
    /// the rejected text is carried here for the message the user sees.
    Rejected(String),
}

/// The outcome of one IDE-config generation attempt, as the dialog shows it.
///
/// One field covers success, skip, "there is no IDE", and failure, because the
/// status area shows exactly one of them — the most recent — and a failure is
/// no less a result than a written file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DapIdeReport {
    /// Generation ran for `ide` and reported what it did to which file.
    Written {
        /// Which IDE the config was written for.
        ide: ParentIde,
        /// What `frust_dap::ide_config::generate_ide_config` reported.
        result: IdeConfigResult,
    },
    /// The IDE is known but has no DAP client-config path at all (JetBrains'
    /// proprietary debugger) — `generate_ide_config` returns nothing for it.
    Unsupported(ParentIde),
    /// No IDE was detected and no override is set, so there was nothing to
    /// generate *for* — never a silent no-op.
    NoIde,
    /// Generation failed; the rendered reason is retained for display.
    Failed(String),
}

impl DapIdeReport {
    /// The one-line status form the dialog and the toast both use:
    /// `<ide>: <created|updated|skipped> <path>`, or the honest absence.
    pub fn summary(&self) -> String {
        match self {
            Self::Written { ide, result } => {
                let action = match &result.action {
                    ConfigAction::Created => "created".to_string(),
                    ConfigAction::Updated => "updated".to_string(),
                    ConfigAction::Skipped(reason) => format!("skipped ({reason})"),
                };
                format!("{}: {action} {}", ide.display_name(), result.path.display())
            }
            Self::Unsupported(ide) => {
                format!("{}: no DAP config format to generate", ide.display_name())
            }
            Self::NoIde => "no IDE detected — nothing to configure".to_string(),
            Self::Failed(reason) => format!("config generation failed: {reason}"),
        }
    }

    /// Whether this report is a failure (the status line colors it, and the
    /// toast kind follows).
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// One persisted `[dap]` preference, as a just-changed value the runner writes
/// through [`super::persist::save_dap_setting`].
///
/// `enabled` has no variant because the dialog exposes no control for it: it
/// means "start the server on every launch", which is a file-level opt-in
/// today (see [`DapSettings::enabled`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DapSetting {
    /// `auto_start_in_ide`.
    AutoStartInIde(bool),
    /// `auto_configure_ide`.
    AutoConfigureIde(bool),
    /// `port`.
    Port(u16),
    /// `ide_override` (`None` clears the key back to "use the detected IDE").
    IdeOverride(Option<ParentIde>),
    /// `intro_seen` — the first-run listener notice has been shown once and
    /// must never be shown again (see [`DapSettings::intro_port`]).
    IntroSeen(bool),
}

/// The workbench's DAP preferences and the settings dialog's edit state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DapSettings {
    /// Start the embedded DAP server on every launch, IDE or not.
    ///
    /// Persisted and honoured at startup, but the dialog offers no control for
    /// it: the two checkboxes below cover the ordinary cases, and "always run a
    /// listener" is a deliberate, file-level opt-in
    /// (`~/.config/frust/tui.toml`'s `[dap].enabled`).
    pub enabled: bool,
    /// Start the server automatically when the workbench is running inside an
    /// IDE's terminal (the detected-IDE case).
    pub auto_start_in_ide: bool,
    /// Write the IDE's own DAP client config into an app's project when it is
    /// launched while the server is listening (and, on a fresh bind, into the
    /// active session's project) — adding the frust entry where it is missing,
    /// never rewriting one that is already there.
    pub auto_configure_ide: bool,
    /// The port the next start binds (`0` = OS-assigned).
    pub port: u16,
    /// The port field as typed. Committed into [`Self::port`] by
    /// [`Self::commit_port`]; an unparseable value never reaches `port`.
    pub port_input: String,
    /// The explicitly chosen IDE, or `None` to follow [`Self::detected_ide`].
    pub ide_override: Option<ParentIde>,
    /// The IDE detected from the environment at startup (one read, at
    /// [`AppState::new`](super::AppState::new)) — never re-sniffed per frame.
    pub detected_ide: Option<ParentIde>,
    /// Whether the one-time first-run notice below has already been shown on
    /// this machine (persisted as `[dap].intro_seen`).
    pub intro_seen: bool,
    /// The port a first-ever auto-start was about to bind, set only while the
    /// one-time notice is on screen (`None` the rest of the time).
    ///
    /// Lifecycle — deliberately narrow:
    /// - **Set** exactly once, by the first `Message::DapAutoStart` that would
    ///   otherwise have started a listener silently; the same transition opens
    ///   the dialog and persists `intro_seen = true`, so quitting without
    ///   acting still spends the notice.
    /// - **Cleared** when the dialog closes or the server starts — it is a
    ///   first-run greeting, not a retained status line.
    /// - **Never re-set.** `intro_seen` is burned only when the gate actually
    ///   fires, so a user whose first launches are outside an IDE still gets
    ///   the notice on their first launch *inside* one rather than having
    ///   spent it on a run that was never going to bind anything.
    pub intro_port: Option<u16>,
    /// Which control has focus while the dialog is open.
    pub focus: DapFocus,
    /// The most recent IDE-config generation outcome, retained (including a
    /// failure) until the next one replaces it.
    pub last_ide_config: Option<DapIdeReport>,
}

impl Default for DapSettings {
    fn default() -> Self {
        Self::from_prefs(DapPrefs::default(), None)
    }
}

impl DapSettings {
    /// Build the model from the persisted `[dap]` preferences plus the IDE
    /// detected for this process.
    pub fn from_prefs(prefs: DapPrefs, detected_ide: Option<ParentIde>) -> Self {
        Self {
            enabled: prefs.enabled,
            auto_start_in_ide: prefs.auto_start_in_ide,
            auto_configure_ide: prefs.auto_configure_ide,
            port: prefs.port,
            port_input: prefs.port.to_string(),
            ide_override: prefs.ide_override,
            detected_ide,
            intro_seen: prefs.intro_seen,
            intro_port: None,
            focus: DapFocus::Server,
            last_ide_config: None,
        }
    }

    /// Reset the dialog's transient edit state (focus at the top, the port
    /// field re-primed from the committed port) — what opening it does, so a
    /// previously-abandoned edit never greets the next opener.
    ///
    /// The first-run notice is transient state too: an ordinary open clears
    /// it, and the auto-start gate sets it *after* opening.
    pub fn reopen(&mut self) {
        self.focus = DapFocus::Server;
        self.port_input = self.port.to_string();
        self.intro_port = None;
    }

    /// The one-time first-run notice, when it is showing: what auto-start was
    /// about to do, and what the two ways out of it are.
    ///
    /// A configured port of `0` is OS-assigned, so it is named as such rather
    /// than printed as a port nothing will ever listen on.
    pub fn intro_notice(&self) -> Option<String> {
        let port = self.intro_port?;
        let where_ = if port == 0 {
            "an OS-assigned port".to_string()
        } else {
            format!("port {port}")
        };
        Some(format!(
            "frust detected an IDE terminal — auto-start would open the DAP \
             listener on {where_}. Press s to start now; auto-start stays on \
             for future launches."
        ))
    }

    /// Move focus to the next control (wrapping).
    pub fn focus_next(&mut self) {
        self.step_focus(1);
    }

    /// Move focus to the previous control (wrapping).
    pub fn focus_prev(&mut self) {
        self.step_focus(-1);
    }

    fn step_focus(&mut self, delta: isize) {
        let cur = FOCUS_ORDER
            .iter()
            .position(|f| *f == self.focus)
            .unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(FOCUS_ORDER.len() as isize) as usize;
        self.focus = FOCUS_ORDER[next];
    }

    /// Feed a character into the port field; a no-op on every other control.
    pub fn input_char(&mut self, c: char) {
        if self.focus == DapFocus::Port {
            self.port_input.push(c);
        }
    }

    /// Delete the last character of the port field; a no-op elsewhere.
    pub fn backspace(&mut self) {
        if self.focus == DapFocus::Port {
            self.port_input.pop();
        }
    }

    /// The port the field currently names: an empty field means the default
    /// ([`super::DEFAULT_DAP_PORT`]), anything that isn't a `u16` means
    /// nothing at all.
    pub fn parsed_port(&self) -> Option<u16> {
        let text = self.port_input.trim();
        if text.is_empty() {
            return Some(super::DEFAULT_DAP_PORT);
        }
        text.parse::<u16>().ok()
    }

    /// Commit the port field into [`Self::port`].
    ///
    /// A rejected value restores the field to the port still in effect rather
    /// than leaving unusable text behind — the caller surfaces the rejection.
    pub fn commit_port(&mut self) -> PortCommit {
        match self.parsed_port() {
            Some(port) if port == self.port => {
                self.port_input = port.to_string();
                PortCommit::Unchanged
            }
            Some(port) => {
                self.port = port;
                self.port_input = port.to_string();
                PortCommit::Changed(port)
            }
            None => {
                let rejected = self.port_input.trim().to_string();
                self.port_input = self.port.to_string();
                PortCommit::Rejected(rejected)
            }
        }
    }

    /// Cycle the IDE selector by `delta` (`+1`/`-1`) through
    /// `detected → each of [`IDE_OVERRIDES`] → detected`.
    pub fn cycle_ide(&mut self, delta: isize) {
        let len = IDE_OVERRIDES.len() as isize + 1;
        let cur = match self.ide_override {
            None => 0,
            Some(ide) => IDE_OVERRIDES
                .iter()
                .position(|c| *c == ide)
                .map(|i| i as isize + 1)
                .unwrap_or(0),
        };
        let next = (cur + delta).rem_euclid(len);
        self.ide_override = (next > 0).then(|| IDE_OVERRIDES[(next - 1) as usize]);
    }

    /// The IDE a generation would target: the explicit override, else whatever
    /// was detected.
    pub fn effective_ide(&self) -> Option<ParentIde> {
        self.ide_override.or(self.detected_ide)
    }

    /// The selector's display label.
    pub fn ide_label(&self) -> String {
        match self.ide_override {
            Some(ide) => ide.display_name().to_string(),
            None => match self.detected_ide {
                Some(ide) => format!("detected: {}", ide.display_name()),
                None => "detected: none".to_string(),
            },
        }
    }

    /// Whether a port edit is waiting on a restart: the server is listening on
    /// a different port than the one now configured. The dialog says so rather
    /// than restarting the server under the user.
    pub fn port_awaits_restart(&self, listening_on: Option<u16>) -> bool {
        matches!(listening_on, Some(bound) if bound != self.port)
    }
}

/// Whether the workbench should start its DAP server at launch.
///
/// This is `frust_dap::should_auto_start_dap`'s decision with the IDE
/// detection **injected** rather than read from the process environment: the
/// library reads the real environment on every call, which a test cannot vary
/// without mutating process-global state (`std::env::set_var` is `unsafe` in
/// this edition, and `frust-tui` holds no sanctioned `unsafe` zone). The
/// detected value is sniffed once, at [`AppState::new`](super::AppState::new),
/// and stored on [`DapSettings::detected_ide`]; parity with the library
/// function is asserted in this module's tests.
pub fn should_auto_start(
    enabled: bool,
    auto_start_in_ide: bool,
    detected: Option<ParentIde>,
) -> bool {
    enabled || (auto_start_in_ide && detected.is_some())
}

/// The name `[dap].ide_override` stores an override under — the inverse of
/// `frust_dap::ide_config::parse_ide_name`. `None` for a [`ParentIde`] that
/// name cannot round-trip (which the selector never offers — see
/// [`IDE_OVERRIDES`]), so a value that could not be read back is never
/// written.
pub fn persisted_ide_name(ide: ParentIde) -> Option<&'static str> {
    match ide {
        ParentIde::VSCode => Some("vscode"),
        ParentIde::Neovim => Some("neovim"),
        ParentIde::Zed => Some("zed"),
        ParentIde::Emacs => Some("emacs"),
        ParentIde::Helix => Some("helix"),
        ParentIde::VSCodeInsiders
        | ParentIde::Cursor
        | ParentIde::IntelliJ
        | ParentIde::AndroidStudio => None,
    }
}

/// Everything an off-thread IDE-config generation needs — resolved by the pure
/// engine, enacted by the runner (the generation does file I/O, so it never
/// runs on the transition path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeConfigRequest {
    /// The IDE to generate for (already resolved through
    /// [`DapSettings::effective_ide`] — never `None`, since "no IDE" is
    /// reported without asking the runner for anything).
    pub ide: ParentIde,
    /// The port the generated config points the editor at.
    pub port: u16,
    /// The project whose config files are written.
    pub project_root: PathBuf,
    /// What happens to a config file that already exists:
    /// [`WriteMode::Refresh`] for the dialog's explicit "generate now",
    /// [`WriteMode::IfAbsent`] for the automatic on-launch write.
    pub mode: WriteMode,
}

impl IdeConfigRequest {
    /// Whether the runner should post `report` back as a
    /// `Message::DapIdeConfig` (which stores it for the dialog and toasts it).
    ///
    /// The explicit path reports every ending. The automatic path stays quiet
    /// about a skip — an entry already present (or Helix, which never has
    /// anything to write) is the ordinary case on every launch, not news —
    /// and reports only a created/updated file or a failure.
    pub fn reports(&self, report: &DapIdeReport) -> bool {
        match self.mode {
            WriteMode::Refresh => true,
            WriteMode::IfAbsent => !matches!(
                report,
                DapIdeReport::Written {
                    result: IdeConfigResult {
                        action: ConfigAction::Skipped(_),
                        ..
                    },
                    ..
                }
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_dap::ide_config::parse_ide_name;
    use std::path::PathBuf;

    fn settings() -> DapSettings {
        DapSettings::default()
    }

    #[test]
    fn defaults_match_the_documented_dap_table() {
        let s = settings();
        assert!(!s.enabled, "a listener on every launch is opt-in");
        assert!(s.auto_start_in_ide);
        assert!(s.auto_configure_ide);
        assert_eq!(s.port, super::super::DEFAULT_DAP_PORT);
        assert_eq!(s.port_input, super::super::DEFAULT_DAP_PORT.to_string());
        assert_eq!(s.ide_override, None);
        assert_eq!(s.focus, DapFocus::Server);
    }

    #[test]
    fn focus_walks_every_control_and_wraps_both_ways() {
        let mut s = settings();
        for expected in FOCUS_ORDER.iter().skip(1) {
            s.focus_next();
            assert_eq!(s.focus, *expected);
        }
        s.focus_next();
        assert_eq!(s.focus, DapFocus::Server, "wraps forward");
        s.focus_prev();
        assert_eq!(s.focus, DapFocus::Generate, "wraps backward");
    }

    #[test]
    fn text_input_only_edits_the_port_field() {
        let mut s = settings();
        s.focus = DapFocus::AutoStart;
        s.input_char('9');
        assert_eq!(s.port_input, super::super::DEFAULT_DAP_PORT.to_string());
        s.focus = DapFocus::Port;
        s.port_input.clear();
        for c in "5005".chars() {
            s.input_char(c);
        }
        assert_eq!(s.port_input, "5005");
        s.backspace();
        assert_eq!(s.port_input, "500");
    }

    #[test]
    fn an_empty_port_field_means_the_default_port() {
        let mut s = settings();
        s.port = 5005;
        s.port_input = "   ".to_string();
        assert_eq!(s.parsed_port(), Some(super::super::DEFAULT_DAP_PORT));
        assert_eq!(
            s.commit_port(),
            PortCommit::Changed(super::super::DEFAULT_DAP_PORT)
        );
        assert_eq!(s.port, super::super::DEFAULT_DAP_PORT);
    }

    #[test]
    fn a_non_u16_port_is_rejected_and_the_committed_one_restored() {
        let mut s = settings();
        for bad in ["70000", "-1", "80a", "4.9", "port"] {
            s.port_input = bad.to_string();
            assert_eq!(s.parsed_port(), None, "{bad} must not parse as a u16");
            assert_eq!(s.commit_port(), PortCommit::Rejected(bad.to_string()));
            assert_eq!(s.port, super::super::DEFAULT_DAP_PORT);
            assert_eq!(s.port_input, super::super::DEFAULT_DAP_PORT.to_string());
        }
    }

    #[test]
    fn committing_the_same_port_reports_unchanged() {
        let mut s = settings();
        s.port_input = super::super::DEFAULT_DAP_PORT.to_string();
        assert_eq!(s.commit_port(), PortCommit::Unchanged);
    }

    #[test]
    fn the_ide_selector_cycles_detected_then_every_override_and_wraps() {
        let mut s = settings();
        assert_eq!(s.ide_override, None);
        for expected in IDE_OVERRIDES {
            s.cycle_ide(1);
            assert_eq!(s.ide_override, Some(expected));
        }
        s.cycle_ide(1);
        assert_eq!(s.ide_override, None, "wraps back to `detected`");
        s.cycle_ide(-1);
        assert_eq!(s.ide_override, Some(*IDE_OVERRIDES.last().unwrap()));
    }

    #[test]
    fn an_override_wins_over_detection_and_the_label_says_which() {
        let mut s = settings();
        s.detected_ide = Some(ParentIde::Cursor);
        assert_eq!(s.effective_ide(), Some(ParentIde::Cursor));
        assert_eq!(s.ide_label(), "detected: Cursor");
        s.ide_override = Some(ParentIde::Zed);
        assert_eq!(s.effective_ide(), Some(ParentIde::Zed));
        assert_eq!(s.ide_label(), "Zed");
        s.ide_override = None;
        s.detected_ide = None;
        assert_eq!(s.effective_ide(), None);
        assert_eq!(s.ide_label(), "detected: none");
    }

    /// Every override the selector offers must round-trip through the
    /// persistence seam — otherwise a chosen IDE would silently forget itself
    /// on the next launch.
    #[test]
    fn every_offered_override_round_trips_through_the_persisted_name() {
        for ide in IDE_OVERRIDES {
            let name = persisted_ide_name(ide).expect("an offered override must have a name");
            assert_eq!(parse_ide_name(name).unwrap(), ide);
        }
    }

    #[test]
    fn an_ide_the_persisted_name_cannot_round_trip_is_never_written() {
        assert_eq!(persisted_ide_name(ParentIde::Cursor), None);
        assert_eq!(persisted_ide_name(ParentIde::IntelliJ), None);
    }

    /// The auto-start truth table: an explicit `enabled` always wins; the IDE
    /// case needs both the preference and a detected IDE; neither means no.
    #[test]
    fn auto_start_truth_table() {
        assert!(should_auto_start(true, false, None), "explicitly enabled");
        assert!(should_auto_start(true, true, Some(ParentIde::VSCode)));
        assert!(
            should_auto_start(false, true, Some(ParentIde::VSCode)),
            "auto-start with an IDE detected"
        );
        assert!(
            !should_auto_start(false, true, None),
            "auto-start with no IDE detected"
        );
        assert!(
            !should_auto_start(false, false, Some(ParentIde::VSCode)),
            "an IDE alone is not an opt-in"
        );
        assert!(!should_auto_start(false, false, None));
    }

    /// Parity with the library decision this one injects detection into: fed
    /// whatever *this* environment detects, the two must agree on every
    /// flag combination — so the injected form can never drift from
    /// `frust_dap::should_auto_start_dap`'s semantics.
    #[test]
    fn the_injected_decision_matches_the_library_decision_for_this_environment() {
        let detected = frust_dap::ide_config::detect_parent_ide();
        for enabled in [false, true] {
            for auto in [false, true] {
                assert_eq!(
                    should_auto_start(enabled, auto, detected),
                    frust_dap::ide_config::should_auto_start_dap(enabled, auto),
                    "enabled={enabled} auto_start_in_ide={auto}"
                );
            }
        }
    }

    #[test]
    fn a_port_edit_awaits_a_restart_only_while_a_different_port_is_bound() {
        let mut s = settings();
        assert!(!s.port_awaits_restart(None), "nothing is listening");
        assert!(!s.port_awaits_restart(Some(s.port)));
        s.port = 5005;
        assert!(s.port_awaits_restart(Some(super::super::DEFAULT_DAP_PORT)));
    }

    #[test]
    fn reopening_the_dialog_drops_an_abandoned_port_edit_and_a_stale_notice() {
        let mut s = settings();
        s.focus = DapFocus::Generate;
        s.port_input = "nonsense".to_string();
        s.intro_port = Some(4849);
        s.reopen();
        assert_eq!(s.focus, DapFocus::Server);
        assert_eq!(s.port_input, s.port.to_string());
        assert_eq!(s.intro_port, None);
    }

    /// The first-run notice exists only while a port is pending, names that
    /// port, and never claims a port for an OS-assigned bind.
    #[test]
    fn the_first_run_notice_shows_only_while_pending_and_names_the_port() {
        let mut s = settings();
        assert!(!s.intro_seen, "a fresh install has not seen the notice");
        assert_eq!(s.intro_notice(), None);

        s.intro_port = Some(4849);
        let notice = s.intro_notice().expect("a pending notice");
        assert!(notice.contains("IDE terminal"));
        assert!(notice.contains("port 4849"));
        assert!(notice.contains("Press s"));

        s.intro_port = Some(0);
        assert!(
            s.intro_notice()
                .expect("a pending notice")
                .contains("an OS-assigned port"),
            "port 0 is not a port anything listens on"
        );
    }

    #[test]
    fn a_report_summarises_as_ide_action_path() {
        let written = DapIdeReport::Written {
            ide: ParentIde::VSCode,
            result: IdeConfigResult {
                path: PathBuf::from("/tmp/p/.vscode/launch.json"),
                action: ConfigAction::Created,
            },
        };
        assert_eq!(
            written.summary(),
            "VS Code: created /tmp/p/.vscode/launch.json"
        );
        assert!(!written.is_failure());

        let skipped = DapIdeReport::Written {
            ide: ParentIde::Helix,
            result: IdeConfigResult {
                path: PathBuf::from("/tmp/p/.helix/languages.toml"),
                action: ConfigAction::Skipped("helix requires a spawnable adapter".to_string()),
            },
        };
        assert!(skipped.summary().contains("skipped (helix requires"));

        assert_eq!(
            DapIdeReport::NoIde.summary(),
            "no IDE detected — nothing to configure"
        );
        let failed = DapIdeReport::Failed("permission denied".to_string());
        assert!(failed.is_failure());
        assert!(failed.summary().contains("permission denied"));
        assert!(
            DapIdeReport::Unsupported(ParentIde::IntelliJ)
                .summary()
                .contains("IntelliJ")
        );
    }

    /// The automatic path reports only what changed on disk (or failed); the
    /// explicit path reports everything, skips included.
    #[test]
    fn only_the_explicit_path_reports_a_skip() {
        let written = |action| DapIdeReport::Written {
            ide: ParentIde::Zed,
            result: IdeConfigResult {
                path: PathBuf::from("/tmp/p/.zed/debug.json"),
                action,
            },
        };
        let request = |mode| IdeConfigRequest {
            ide: ParentIde::Zed,
            port: 4849,
            project_root: PathBuf::from("/tmp/p"),
            mode,
        };
        let skipped = written(ConfigAction::Skipped(
            frust_dap::ide_config::ENTRY_PRESENT_REASON.to_string(),
        ));
        let failed = DapIdeReport::Failed("permission denied".to_string());

        let auto = request(WriteMode::IfAbsent);
        assert!(!auto.reports(&skipped));
        assert!(auto.reports(&written(ConfigAction::Created)));
        assert!(auto.reports(&written(ConfigAction::Updated)));
        assert!(auto.reports(&failed));

        let explicit = request(WriteMode::Refresh);
        assert!(explicit.reports(&skipped));
        assert!(explicit.reports(&written(ConfigAction::Created)));
        assert!(explicit.reports(&failed));
    }
}
