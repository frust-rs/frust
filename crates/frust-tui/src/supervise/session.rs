//! The value vocabulary of a supervised session: its identity
//! ([`SessionId`]), the parameters that define it ([`SessionSpec`] =
//! project × device × mode, PLAN D6b), its lifecycle [`SessionState`]
//! machine, the [`SessionEvent`]s the [`super::Supervisor`] forwards into
//! the engine's channel, and the [`LaunchPlan`] a spec resolves into before
//! it is spawned through the drive's `spawn_streaming` seam.
//!
//! Everything here is plain data + pure functions — no threads, no tokio, no
//! process. The moving parts live in [`super::supervisor`].

use std::path::PathBuf;

use frust_drive::build_info::BuildInfo;
use frust_drive::devices::Device;

/// A unique per-session identifier, handed out monotonically by a single
/// [`super::Supervisor`]. Wrapped rather than a bare `u64` so it can't be
/// confused with any other counter the engine carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// Where a session runs: the desktop preview (`cargo run`, no device) or a
/// discovered Android/iOS device/emulator/simulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTarget {
    /// The desktop `cargo run` preview — the fallback target when no device
    /// is selected (see `docs/ARCHITECTURE.md`'s CLI flow).
    Desktop,
    /// A concrete device from `frust-drive`'s discovery set.
    Device(Device),
}

/// The parameters that identify and configure a supervised session, per PLAN
/// D6b: `(project root × device target × BuildInfo)`.
///
/// A spec resolves into a [`LaunchPlan`] via [`SessionSpec::launch_plan`]
/// before the supervisor spawns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// The project the session builds/runs (the directory holding its
    /// `frust.toml`).
    pub project_root: PathBuf,
    /// Which device (or the desktop preview) the session targets.
    pub target: DeviceTarget,
    /// The build mode/flavor/defines funnel (`docs/ARCHITECTURE.md`'s
    /// `BuildInfo` row) — desktop threads `mode` into the cargo profile arg
    /// and every define into the spawned env, matching on-device behavior.
    pub build: BuildInfo,
}

impl SessionSpec {
    /// Resolve this spec into the concrete streaming invocation the
    /// supervisor spawns.
    ///
    /// **Desktop** is fully wired here: `cargo run` in the project root,
    /// threading the build mode into the cargo profile arg and every
    /// `--define` into the spawned env (the same funnel `frust run`'s desktop
    /// fallback uses — see `docs/ARCHITECTURE.md`'s CLI flow). **Device**
    /// targets are *not* a single streamable command (their real path is the
    /// drive's multi-phase build → install → launch → logcat pipeline); that
    /// wiring lands in TUI2-04, which either hands a purpose-built
    /// [`LaunchPlan`] to [`super::Supervisor::start_with_plan`] or adds a
    /// multi-spec entry. Calling `launch_plan` for a device target today
    /// returns [`LaunchError::DeviceLaunchUnwired`].
    pub fn launch_plan(&self) -> Result<LaunchPlan, LaunchError> {
        match &self.target {
            DeviceTarget::Desktop => Ok(self.desktop_plan()),
            DeviceTarget::Device(device) => {
                Err(LaunchError::DeviceLaunchUnwired(device.name.clone()))
            }
        }
    }

    /// The `cargo run` desktop-preview plan for this spec.
    fn desktop_plan(&self) -> LaunchPlan {
        let mut args = vec!["run".to_string()];
        for arg in self.build.mode.cargo_profile_arg() {
            args.push((*arg).to_string());
        }

        // Every `--define KEY=VALUE` becomes an env var for the spawned
        // preview, matching on-device behavior. Sorted by key so the derived
        // plan (and any test scripting it) is deterministic despite the
        // defines living in a `HashMap`.
        let mut env: Vec<(String, String)> = self
            .build
            .defines
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env.sort();

        LaunchPlan {
            program: "cargo".to_string(),
            args,
            cwd: self.project_root.clone(),
            env,
        }
    }
}

/// A resolved device-session plan: the pieces the multi-phase device pipeline
/// (`frust-drive`'s `android_run`/`ios_run`) needs, owned so they can cross
/// onto the device supervision thread (see [`super::Supervisor::start`]'s
/// device dispatch). Built from a [`SessionSpec`] whose `target` is a
/// [`DeviceTarget::Device`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePlan {
    /// The project the session builds/runs.
    pub project_root: PathBuf,
    /// The concrete device the pipeline targets.
    pub device: Device,
    /// The build mode/flavor/defines funnel.
    pub build: BuildInfo,
}

/// A concrete, spawnable streaming invocation: the program, its args, the
/// working directory, and any extra environment for the child only. This is
/// what the supervisor feeds to `ProcessRunner::spawn_streaming`; keeping it
/// owned (rather than borrowed) lets it cross onto the spawn thread and lets
/// TUI2-04 build device plans however it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// The program to spawn (e.g. `cargo`).
    pub program: String,
    /// Its arguments, in order (e.g. `["run", "--release"]`).
    pub args: Vec<String>,
    /// The child's working directory.
    pub cwd: PathBuf,
    /// Extra environment for the child only (added/overridden, never touching
    /// the parent's environment) — e.g. `--define`d app config.
    pub env: Vec<(String, String)>,
}

/// Why a [`SessionSpec`] could not be turned into a [`LaunchPlan`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// A device target was given, but the multi-phase device pipeline is not
    /// yet wired into the single-launch API (TUI2-04). Carries the device
    /// name for the message.
    #[error(
        "device launch for `{0}` is not wired yet — use `Supervisor::start_with_plan` \
         (TUI2-04 wires the device pipeline)"
    )]
    DeviceLaunchUnwired(String),
}

/// The lifecycle of a supervised session (PLAN D2).
///
/// Forward-only through the build phases (`Configuring → Building →
/// Installing → Running`), terminating in exactly one of [`Exited`] (the
/// process ended on its own) or [`Killed`] (the supervisor stopped it). The
/// phase transitions are *inferred* from the streamed output where cheap and
/// stay tolerant — an unknown line never advances or wedges the machine (see
/// [`infer_state`]).
///
/// [`Exited`]: SessionState::Exited
/// [`Killed`]: SessionState::Killed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Initial state: the session is starting up but has produced no
    /// phase-identifying output yet.
    Configuring,
    /// A compile/Gradle/xcodebuild build is underway.
    Building,
    /// The built artifact is being installed onto the target.
    Installing,
    /// The app is running and streaming output (the desktop preview window is
    /// up, or logcat/console is streaming).
    Running,
    /// The process ended on its own; the flag mirrors its exit success
    /// (`false` for a non-zero exit).
    Exited(bool),
    /// The supervisor stopped the process ([`super::Supervisor::stop`]).
    Killed,
}

impl SessionState {
    /// The ordinal of a live build phase (`Configuring`=0 … `Running`=3), or
    /// `None` for a terminal state. Used by [`infer_state`] to keep phase
    /// transitions forward-only.
    fn phase_ordinal(&self) -> Option<u8> {
        match self {
            SessionState::Configuring => Some(0),
            SessionState::Building => Some(1),
            SessionState::Installing => Some(2),
            SessionState::Running => Some(3),
            SessionState::Exited(_) | SessionState::Killed => None,
        }
    }

    /// Whether this is a terminal state (`Exited`/`Killed`) — no further
    /// transitions follow.
    pub fn is_terminal(&self) -> bool {
        matches!(self, SessionState::Exited(_) | SessionState::Killed)
    }
}

/// An event emitted by a supervised session, tagged with its [`SessionId`] so
/// a single engine channel can carry every session's stream without crossing
/// them. All events for one session arrive in order (they are sent from that
/// session's single supervising thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    /// Which session produced this event.
    pub id: SessionId,
    /// What happened.
    pub kind: SessionEventKind,
}

/// The payload of a [`SessionEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEventKind {
    /// The session transitioned to a new lifecycle state.
    State(SessionState),
    /// A line of output arrived from the session's process (already stripped
    /// of its trailing newline by the drive's stream decoder).
    Line(String),
}

/// Tolerant, forward-only inference of a lifecycle phase from one streamed
/// output line.
///
/// Returns `Some(next)` only when `line` names a build phase strictly later
/// than `current`; an unknown line, a line naming an earlier/equal phase, or
/// any line at all once the machine is terminal returns `None` — the state
/// machine is never regressed or wedged by output it doesn't recognize.
///
/// The recognized markers are the drive pipelines' own phase lines
/// (`Building …`/`Installing on …`/`Launching …`/`Streaming logs …` — see
/// `frust-drive`'s `android_run`/`ios_run`) plus cargo's own
/// `Compiling …`/`Running …` for the desktop preview.
pub(crate) fn infer_state(current: &SessionState, line: &str) -> Option<SessionState> {
    let candidate = phase_from_line(line)?;
    match (current.phase_ordinal(), candidate.phase_ordinal()) {
        (Some(cur), Some(cand)) if cand > cur => Some(candidate),
        _ => None,
    }
}

/// Maps a single line to the build phase it announces, if any. Checks the
/// latest phases first so a more specific marker wins.
fn phase_from_line(line: &str) -> Option<SessionState> {
    // Cargo indents its status lines ("   Compiling", "    Running"); the
    // drive pipelines don't. Trimming the leading whitespace handles both.
    let l = line.trim_start();
    if l.starts_with("Streaming logs") || l.starts_with("Launching") || l.starts_with("Running") {
        Some(SessionState::Running)
    } else if l.starts_with("Installing") || l.starts_with("Installed") {
        Some(SessionState::Installing)
    } else if l.starts_with("Building") || l.starts_with("Compiling") {
        Some(SessionState::Building)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::build_info::BuildMode;
    use frust_drive::devices::{Kind, Platform};
    use std::collections::HashMap;

    fn build(mode: BuildMode) -> BuildInfo {
        BuildInfo {
            mode,
            flavor: None,
            defines: HashMap::new(),
            build_name: None,
            build_number: None,
        }
    }

    #[test]
    fn desktop_debug_plan_is_cargo_run() {
        let spec = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Desktop,
            build: build(BuildMode::Debug),
        };
        let plan = spec.launch_plan().unwrap();
        assert_eq!(plan.program, "cargo");
        assert_eq!(plan.args, vec!["run".to_string()]);
        assert_eq!(plan.cwd, PathBuf::from("/tmp/app"));
        assert!(plan.env.is_empty());
    }

    #[test]
    fn desktop_release_plan_threads_profile_arg() {
        let spec = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Desktop,
            build: build(BuildMode::Release),
        };
        let plan = spec.launch_plan().unwrap();
        assert_eq!(plan.args, vec!["run".to_string(), "--release".to_string()]);
    }

    #[test]
    fn desktop_plan_threads_defines_into_env_sorted() {
        let mut b = build(BuildMode::Debug);
        b.defines.insert("Z_KEY".into(), "1".into());
        b.defines.insert("A_KEY".into(), "2".into());
        let spec = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Desktop,
            build: b,
        };
        let plan = spec.launch_plan().unwrap();
        assert_eq!(
            plan.env,
            vec![
                ("A_KEY".to_string(), "2".to_string()),
                ("Z_KEY".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn device_target_is_not_wired_yet() {
        let device = Device {
            id: "emulator-5554".into(),
            name: "Pixel 7".into(),
            platform: Platform::Android,
            kind: Kind::Emulator,
            os_version: None,
            connection_state: None,
        };
        let spec = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Device(device),
            build: build(BuildMode::Debug),
        };
        assert_eq!(
            spec.launch_plan().unwrap_err(),
            LaunchError::DeviceLaunchUnwired("Pixel 7".to_string())
        );
    }

    #[test]
    fn infer_advances_forward_only_and_ignores_unknown_lines() {
        let s = SessionState::Configuring;
        assert_eq!(
            infer_state(&s, "   Compiling app v0.1.0"),
            Some(SessionState::Building)
        );
        // An unknown line does not advance the machine.
        assert_eq!(
            infer_state(&SessionState::Building, "warning: unused"),
            None
        );
        // Installing follows Building.
        assert_eq!(
            infer_state(&SessionState::Building, "Installing on Pixel 7…"),
            Some(SessionState::Installing)
        );
        // A late build-ish line never regresses from Running.
        assert_eq!(
            infer_state(&SessionState::Running, "Compiling something late"),
            None
        );
        // Launching/Streaming both mean Running.
        assert_eq!(
            infer_state(&SessionState::Installing, "Launching it.f0x.app…"),
            Some(SessionState::Running)
        );
        assert_eq!(
            infer_state(&SessionState::Building, "Streaming logs (pid 1234)"),
            Some(SessionState::Running)
        );
    }

    #[test]
    fn infer_never_transitions_out_of_a_terminal_state() {
        for terminal in [SessionState::Exited(true), SessionState::Killed] {
            assert_eq!(infer_state(&terminal, "Launching app"), None);
            assert!(terminal.is_terminal());
        }
    }
}
