//! The value vocabulary of a supervised session: its identity
//! ([`SessionId`]), the parameters that define it ([`SessionSpec`] =
//! project × device × mode), its lifecycle [`SessionState`]
//! machine, the [`SessionEvent`]s the [`super::Supervisor`] forwards into
//! the engine's channel, and the [`LaunchPlan`] a spec resolves into before
//! it is spawned through the drive's `spawn_streaming` seam.
//!
//! A watched debug desktop session runs **hot** instead: [`SessionSpec::start_hot`]
//! resolves it through `frust-drive`'s hot-patch session start
//! (`hotpatch::session::start_desktop` — the fat build, then the fat image
//! spawned directly, never `cargo run`), and [`SessionSpec::hot_precondition`]
//! says which specs qualify. A watched debug Android device session runs hot
//! the same way through `hotpatch::android::start_android` (the fat build
//! outside Gradle, install, launch, an `adb forward` to the devtools
//! endpoint), and one on a booted iOS simulator through
//! `hotpatch::ios_sim::start_ios_sim` (the fat build through Xcode, `simctl
//! install`, `simctl launch`, the devtools endpoint on the host's
//! loopback). Unwatched sessions keep [`SessionSpec::launch_plan`].
//!
//! Everything here is plain data + pure functions — no threads, no tokio —
//! except `start_hot`, the one blocking entry, which `crate::runner` only
//! ever calls off the UI thread. The moving parts live in
//! [`super::supervisor`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::desktop_run;
use frust_drive::devices::{Device, Kind, Platform};
use frust_drive::hotpatch::android::{AndroidHotStart, AndroidStart, start_android};
use frust_drive::hotpatch::ios_sim::{IosSimHotStart, IosSimStart, start_ios_sim};
use frust_drive::hotpatch::session::{
    DesktopStart, HotSession, RestartReason, SessionHost, StartError, start_desktop,
};
use frust_drive::process::{ProcessRunner, StreamHandle};

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

/// The parameters that identify and configure a supervised session:
/// `(project root × device target × BuildInfo)`.
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
    /// targets are *not* a single streamable command — their real path is the
    /// drive's multi-phase build → install → launch → logcat pipeline
    /// ([`DevicePlan`] handed to [`super::Supervisor::start_device`]), which
    /// [`super::Supervisor::start`] dispatches to directly rather than going
    /// through this method. Calling `launch_plan` on a device target
    /// directly (bypassing `Supervisor::start`) returns
    /// [`LaunchError::DeviceLaunchUnwired`].
    pub fn launch_plan(&self) -> Result<LaunchPlan, LaunchError> {
        match &self.target {
            DeviceTarget::Desktop => Ok(self.desktop_plan()),
            DeviceTarget::Device(device) => {
                Err(LaunchError::DeviceLaunchUnwired(device.name.clone()))
            }
        }
    }

    /// The `cargo run` desktop-preview plan for this spec.
    ///
    /// Resolved by `frust-drive`'s `desktop_run` — the workspace's single
    /// desktop launch-plan construction site, shared with `frust run`'s
    /// desktop fallback and `frust-mcp`'s session engine — and only reshaped
    /// into the [`LaunchPlan`] the supervisor spawns. The workbench holds no
    /// mode → cargo-args mapping of its own: a preview built without
    /// `frust/devtools` compiled in would leave DevTools waiting forever for a
    /// discovery line that can never be printed, and that decision belongs in
    /// one place for every front-end.
    fn desktop_plan(&self) -> LaunchPlan {
        let plan = desktop_run::desktop_plan(&self.project_root, &self.build);
        LaunchPlan {
            program: plan.program,
            args: plan.args,
            cwd: plan.cwd,
            env: plan.env,
        }
    }

    /// Whether this spec can run as a hot-patch session, or the failed
    /// precondition, worded for a toast: only the desktop preview, an
    /// Android device or an iOS simulator, and only a debug build
    /// (`start_desktop`, `start_android` and `start_ios_sim` refuse every
    /// other mode, and a physical iOS device has no hot-patch path at all).
    pub fn hot_precondition(&self) -> Result<(), String> {
        if let DeviceTarget::Device(device) = &self.target
            && !has_hot_device_path(device)
        {
            return Err(no_hot_device_path(device));
        }
        if self.build.mode != BuildMode::Debug {
            return Err(format!(
                "hot patching needs a debug build, not {:?}",
                self.build.mode
            ));
        }
        Ok(())
    }

    /// Start this spec as a hot-patch session, dispatched by target: on the
    /// desktop, `frust-drive`'s `start_desktop` builds the app fat, spawns
    /// the fat image directly and attaches to its devtools endpoint; on an
    /// Android device, `start_android` builds it fat outside Gradle,
    /// packages, installs and launches it, and attaches through `adb
    /// forward`; on an iOS simulator, `start_ios_sim` builds it fat
    /// through Xcode, installs and launches it with `simctl`, and attaches
    /// on the host's loopback; a physical iOS device answers
    /// `RestartRequired(BuilderUnsupported)`. **Blocks** for the whole fat
    /// build (and, on a device, the device pipeline); call it off the UI
    /// thread. `on_line` receives the build's diagnostics and the app's
    /// output up to its discovery line; the returned [`HotLaunch`]'s child
    /// carries the rest of the app's output and its lifetime. `cancel` is
    /// honoured by the device starts at each pipeline phase boundary (the
    /// desktop start has no cancel seam); a cancelled start answers
    /// [`StartError::Launch`].
    ///
    /// The tip package is the project root's own `[package]` (the TUI runs
    /// a project, not a workspace member of the caller's choosing); its bin
    /// is left to `start_desktop`, which takes the package's only bin, and
    /// on Android it is the app's `cdylib` crate.
    pub fn start_hot(
        &self,
        runner: Arc<dyn ProcessRunner + Send + Sync>,
        on_line: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<HotLaunch, StartError> {
        if let DeviceTarget::Device(device) = &self.target
            && !has_hot_device_path(device)
        {
            return Err(StartError::RestartRequired(
                RestartReason::BuilderUnsupported {
                    detail: no_hot_device_path(device),
                },
            ));
        }
        let package = package_name(&self.project_root).ok_or_else(|| {
            StartError::RestartRequired(RestartReason::BuilderUnsupported {
                detail: format!(
                    "`{}` names no [package]",
                    self.project_root.join("Cargo.toml").display()
                ),
            })
        })?;
        let host = SessionHost::current(runner)?;
        let device = match &self.target {
            DeviceTarget::Desktop => {
                let (session, child) = start_desktop(
                    &host,
                    &DesktopStart {
                        root: &self.project_root,
                        info: &self.build,
                        package: &package,
                        bin: None,
                    },
                    on_line,
                )?;
                return Ok(HotLaunch {
                    session,
                    child,
                    device: None,
                });
            }
            DeviceTarget::Device(device) => device,
        };
        if device.platform == Platform::Ios {
            return self.start_ios_sim(&host, &package, device, on_line, cancel);
        }
        let started = start_android(
            &host,
            &AndroidStart {
                root: &self.project_root,
                info: &self.build,
                package: &package,
                device,
            },
            on_line,
            cancel,
        )?;
        let Some(AndroidHotStart {
            session,
            launch,
            forward_port,
        }) = started
        else {
            return Err(StartError::Launch {
                detail: "the hot start was cancelled before the app was running".to_string(),
            });
        };
        Ok(HotLaunch {
            session,
            child: launch.stream,
            device: Some(DeviceApp {
                serial: device.id.clone(),
                package: launch.package,
                forward_port,
            }),
        })
    }

    /// [`Self::start_hot`] on the iOS simulator `device`.
    fn start_ios_sim(
        &self,
        host: &SessionHost<'_>,
        package: &str,
        device: &Device,
        on_line: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<HotLaunch, StartError> {
        let started = start_ios_sim(
            host,
            &IosSimStart {
                root: &self.project_root,
                info: &self.build,
                package,
                device,
            },
            on_line,
            cancel,
        )?;
        let Some(IosSimHotStart { session, launch }) = started else {
            return Err(StartError::Launch {
                detail: "the hot start was cancelled before the app was running".to_string(),
            });
        };
        Ok(HotLaunch {
            session,
            child: launch.stream,
            device: Some(DeviceApp {
                serial: device.id.clone(),
                package: launch.bundle_id,
                forward_port: None,
            }),
        })
    }
}

/// Whether `device` has a hot-patch path: an Android device or an iOS
/// simulator (a physical iOS device would need its patches code-signed).
fn has_hot_device_path(device: &Device) -> bool {
    match device.platform {
        Platform::Android => true,
        Platform::Ios => device.kind == Kind::Simulator,
    }
}

/// The refusal for a hot start on a device with no hot-patch path (a
/// physical iOS device).
fn no_hot_device_path(device: &Device) -> String {
    format!(
        "hot patching has no iOS device path, `{}` is an iOS device (only an iOS simulator runs hot)",
        device.name
    )
}

/// A started hot session ([`SessionSpec::start_hot`]): the session, the
/// app's output stream (its lifetime on the desktop, its logcat on
/// Android, its `simctl launch --console-pty` console on an iOS simulator),
/// and — for a device — what stopping the app takes beyond killing that
/// stream.
pub struct HotLaunch {
    pub session: HotSession,
    pub child: StreamHandle,
    pub device: Option<DeviceApp>,
}

/// A hot session's app on a device, as its start reported it: the
/// resolved device's id (an Android serial, a simulator udid), the app that
/// was launched (the *installed* Android package, the simulator bundle id),
/// and the host port `adb forward` allocated for the devtools endpoint
/// (when one was; never on a simulator, which shares the host's loopback).
/// Whoever ends the session stops the app — on Android removing that
/// forward and force-stopping the package, on a simulator `simctl
/// terminate` (`crate::runner`'s hot sessions do, by the spec's target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceApp {
    pub serial: String,
    pub package: String,
    pub forward_port: Option<u16>,
}

/// The `[package] name` of `<root>/Cargo.toml`, when it has one — the tip
/// package a hot session builds and the one a hot watcher resolves the
/// workspace graph for.
pub(crate) fn package_name(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    package_name_in(&text)
}

/// [`package_name`] over a manifest's text.
fn package_name_in(manifest: &str) -> Option<String> {
    let doc = manifest.parse::<toml_edit::DocumentMut>().ok()?;
    doc.get("package")?
        .get("name")?
        .as_str()
        .map(str::to_string)
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
/// owned (rather than borrowed) lets it cross onto the spawn thread.
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
    /// A device target was given directly to [`SessionSpec::launch_plan`],
    /// which only resolves a [`DeviceTarget::Desktop`] into a [`LaunchPlan`]
    /// — a real device session instead goes through
    /// [`super::Supervisor::start_device`]'s [`DevicePlan`] path, which
    /// [`super::Supervisor::start`] dispatches to directly. Carries the
    /// device name for the message.
    #[error(
        "device launch for `{0}` is not wired yet — use `Supervisor::start_with_plan` \
         (TUI2-04 wires the device pipeline)"
    )]
    DeviceLaunchUnwired(String),
}

/// The lifecycle of a supervised session.
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
    /// A coalesced batch of output lines (each already stripped of its
    /// trailing newline by the drive's stream decoder), in order.
    ///
    /// The supervisor's drain thread coalesces a burst — after a blocking
    /// `recv` returns one line it drains every line already buffered via
    /// `try_recv` — into a single batch per send, so a flood of output costs
    /// one bounded-channel slot rather than one per line (see
    /// [`super::supervisor`]).
    Lines(Vec<String>),
    /// The **cumulative** count of output lines the drain thread dropped
    /// because the bounded engine channel was full (drop-newest overflow
    /// policy). Mirrors `frust_drive::process::LineReceiver::dropped_lines`:
    /// additive, never resets, `0` for a healthy session. Surfaced so the UI
    /// can show that a session's log is missing lines the consumer couldn't
    /// keep up with.
    Dropped(u64),
    /// A newly parsed build/install/launch phase label
    /// (`super::progress::phase_from_output_line`), latest-wins across a
    /// batch (see `super::supervisor::feed_lines`) — only ever sent while
    /// the session is still `Building`/`Installing`. The engine is the one
    /// that clears a session's displayed label once a later [`State`](Self::State)
    /// event reports a non-transient state; this event never carries a
    /// clearing `None` itself (see `super::progress`'s module docs).
    Phase(super::progress::PhaseLabel),
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
        assert_eq!(
            plan.args,
            vec![
                "run".to_string(),
                "--features".to_string(),
                "frust/perf-trace".to_string(),
                "--features".to_string(),
                "frust/devtools".to_string(),
            ]
        );
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
        assert_eq!(
            plan.args,
            vec![
                "run".to_string(),
                "--release".to_string(),
                "--features".to_string(),
                "lean".to_string(),
            ]
        );
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
    fn desktop_profile_plan_injects_frust_trace_when_missing() {
        let spec = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Desktop,
            build: build(BuildMode::Profile),
        };
        let plan = spec.launch_plan().unwrap();
        assert_eq!(
            plan.env,
            vec![("FRUST_TRACE".to_string(), "1".to_string())],
            "a --profile desktop run gets FRUST_TRACE=1 even though the \
             run-config modal bypasses BuildInfo::from_args's own injection"
        );
    }

    #[test]
    fn desktop_profile_plan_respects_a_user_provided_frust_trace_define() {
        let mut b = build(BuildMode::Profile);
        b.defines.insert("FRUST_TRACE".into(), "0".into());
        let spec = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Desktop,
            build: b,
        };
        let plan = spec.launch_plan().unwrap();
        assert_eq!(
            plan.env,
            vec![("FRUST_TRACE".to_string(), "0".to_string())],
            "an explicit user define always wins over the auto-inject"
        );
    }

    #[test]
    fn debug_and_release_desktop_plans_never_inject_frust_trace() {
        for mode in [BuildMode::Debug, BuildMode::Release] {
            let spec = SessionSpec {
                project_root: PathBuf::from("/tmp/app"),
                target: DeviceTarget::Desktop,
                build: build(mode),
            };
            let plan = spec.launch_plan().unwrap();
            assert!(plan.env.is_empty(), "{mode:?} must not inject FRUST_TRACE");
        }
    }

    #[test]
    fn only_a_debug_desktop_or_android_spec_can_run_hot() {
        let desktop = |mode| SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Desktop,
            build: build(mode),
        };
        assert_eq!(desktop(BuildMode::Debug).hot_precondition(), Ok(()));
        for mode in [BuildMode::Profile, BuildMode::Release] {
            let reason = desktop(mode).hot_precondition().unwrap_err();
            assert!(reason.contains("needs a debug build"), "{reason}");
        }

        let android = |mode| SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Device(fake_device(Platform::Android)),
            build: build(mode),
        };
        assert_eq!(android(BuildMode::Debug).hot_precondition(), Ok(()));
        for mode in [BuildMode::Profile, BuildMode::Release] {
            let reason = android(mode).hot_precondition().unwrap_err();
            assert!(reason.contains("needs a debug build"), "{reason}");
        }

        let ios = SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Device(fake_device(Platform::Ios)),
            build: build(BuildMode::Debug),
        };
        let reason = ios.hot_precondition().unwrap_err();
        assert!(reason.contains("no iOS device path"), "{reason}");

        let simulator = |mode| SessionSpec {
            project_root: PathBuf::from("/tmp/app"),
            target: DeviceTarget::Device(fake_simulator()),
            build: build(mode),
        };
        assert_eq!(simulator(BuildMode::Debug).hot_precondition(), Ok(()));
        for mode in [BuildMode::Profile, BuildMode::Release] {
            let reason = simulator(mode).hot_precondition().unwrap_err();
            assert!(reason.contains("needs a debug build"), "{reason}");
        }
    }

    /// A booted iOS simulator whose udid is a fake.
    fn fake_simulator() -> Device {
        Device {
            id: "FAKE-UDID".into(),
            name: "iPhone 15".into(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    /// An iOS simulator is dispatched to `start_ios_sim`, not refused like
    /// a physical iOS device: its own debug precondition answers a profile
    /// build, and a project with no `[package]` is refused before any
    /// build, like the desktop.
    #[test]
    fn an_ios_simulator_starts_hot_through_the_simulator_start() {
        let root = std::env::temp_dir().join(format!(
            "frust-tui-simulator-hot-start-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let runner: Arc<dyn ProcessRunner + Send + Sync> =
            Arc::new(frust_drive::process::FakeProcessRunner::new());
        let spec = SessionSpec {
            project_root: root.clone(),
            target: DeviceTarget::Device(fake_simulator()),
            build: build(BuildMode::Profile),
        };
        let err = spec
            .start_hot(Arc::clone(&runner), &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail })
                    if detail.contains("needs a debug build")
            ),
            "{err}"
        );

        let spec = SessionSpec {
            project_root: PathBuf::from("/nonexistent/frust-tui-hot-start"),
            build: build(BuildMode::Debug),
            ..spec
        };
        let err = spec
            .start_hot(runner, &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail })
                    if detail.contains("names no [package]")
            ),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A device whose serial/udid is a fake.
    fn fake_device(platform: Platform) -> Device {
        Device {
            id: "FAKE-SERIAL".into(),
            name: "Fake Phone".into(),
            platform,
            kind: Kind::PhysicalDevice,
            os_version: None,
            connection_state: None,
        }
    }

    /// An iOS device has no hot-patch path: `start_hot` refuses it with
    /// `BuilderUnsupported` before reading the project or running anything
    /// (the fake runner has no responses; a project with no `[package]`
    /// would otherwise refuse with "names no [package]").
    #[test]
    fn an_ios_device_refuses_to_start_hot_before_anything_runs() {
        let spec = SessionSpec {
            project_root: PathBuf::from("/nonexistent/frust-tui-hot-start"),
            target: DeviceTarget::Device(fake_device(Platform::Ios)),
            build: build(BuildMode::Debug),
        };
        let runner: Arc<dyn ProcessRunner + Send + Sync> =
            Arc::new(frust_drive::process::FakeProcessRunner::new());
        let err = spec
            .start_hot(runner, &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail })
                    if detail.contains("no iOS device path")
            ),
            "{err}"
        );
    }

    /// An Android device is dispatched to `start_android` — its own debug
    /// precondition answers a profile build, not the iOS refusal, and a
    /// project with no `[package]` is refused before any build, like the
    /// desktop.
    #[test]
    fn an_android_device_starts_hot_through_the_android_start() {
        let root = std::env::temp_dir().join(format!(
            "frust-tui-android-hot-start-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let runner: Arc<dyn ProcessRunner + Send + Sync> =
            Arc::new(frust_drive::process::FakeProcessRunner::new());
        let spec = SessionSpec {
            project_root: root.clone(),
            target: DeviceTarget::Device(fake_device(Platform::Android)),
            build: build(BuildMode::Profile),
        };
        let err = spec
            .start_hot(Arc::clone(&runner), &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail })
                    if detail.contains("needs a debug build")
            ),
            "{err}"
        );

        let spec = SessionSpec {
            project_root: PathBuf::from("/nonexistent/frust-tui-hot-start"),
            build: build(BuildMode::Debug),
            ..spec
        };
        let err = spec
            .start_hot(runner, &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail })
                    if detail.contains("names no [package]")
            ),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_tip_package_is_the_manifests_own_package_name() {
        assert_eq!(
            package_name_in("[package]\nname = \"huddle\"\nversion = \"0.1.0\"\n"),
            Some("huddle".to_string())
        );
        assert_eq!(
            package_name_in("[workspace]\nmembers = [\"a\"]\n"),
            None,
            "a virtual manifest has no tip package"
        );
        assert_eq!(package_name_in("not toml ["), None);
    }

    #[test]
    fn a_project_without_a_package_refuses_to_start_hot_before_any_build() {
        let spec = SessionSpec {
            project_root: PathBuf::from("/nonexistent/frust-tui-hot-start"),
            target: DeviceTarget::Desktop,
            build: build(BuildMode::Debug),
        };
        let runner: Arc<dyn ProcessRunner + Send + Sync> =
            Arc::new(frust_drive::process::FakeProcessRunner::new());
        let err = spec
            .start_hot(runner, &mut |_| {}, &AtomicBool::new(false))
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                StartError::RestartRequired(RestartReason::BuilderUnsupported { detail })
                    if detail.contains("names no [package]")
            ),
            "{err}"
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
