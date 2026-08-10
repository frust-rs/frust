//! Session launch and log ingestion: one thread per session that drives the
//! `frust-drive` run pipeline to a live output stream, then drains that
//! stream for the session's whole life.
//!
//! The launch itself is *inside* this thread rather than awaited by the
//! caller, because a device launch is a build → install → launch chain that
//! takes minutes; [`crate::engine::SessionEngine::run_app`] hands back a
//! session id immediately and the state surface reports progress. The same
//! thread then becomes the drain loop, so line ordering across the
//! build phase and the app's own output is exactly what the pipeline
//! produced.
//!
//! Everything the engine learns about a session it learns from these lines:
//! the devtools discovery/failure lines (`frust-devtools-protocol`), and the
//! drive's own `Launching <pkg>…` / `Streaming logs (pid N)` phase markers —
//! the same two markers `frust-tui`'s DevTools metrics state parses, for the
//! same reason (the pid is not returned by `android_run::spawn_session`, only
//! logged, and the metrics sampler needs it paired with the package).

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::Result;
use frust_devtools_protocol::{parse_discovery_line, parse_failure_line, redact_discovery_token};
use frust_drive::android_run;
use frust_drive::build_info::BuildInfo;
use frust_drive::desktop_run;
use frust_drive::ios_run;
use frust_drive::process::LineReceiver;

use super::session::{RunTarget, Session, SessionState};
use super::{Runner, devtools, metrics};

/// The drive's `Launching <package>…` phase marker (`android_run`'s
/// `on_line`), carrying the *installed* package — which a flavor's
/// `applicationIdSuffix` can move away from `frust.toml`'s app id — as the
/// launch happens, rather than only once the pipeline returns.
const LAUNCH_MARKER_PREFIX: &str = "Launching ";

/// The drive's `Streaming logs (pid N)` marker, the only channel carrying an
/// Android session's pid.
const PID_MARKER_PREFIX: &str = "Streaming logs (pid ";

/// Spawns the session's launch/drain thread. The caller registers the
/// returned handle on the session for teardown to join.
pub(crate) fn spawn(session: Arc<Session>, runner: Runner) -> JoinHandle<()> {
    thread::spawn(move || run(session, runner))
}

fn run(session: Arc<Session>, runner: Runner) {
    let lines = match start(&session, &runner) {
        Ok(Some(lines)) => lines,
        // The pipeline observed the cancel flag during build/install/launch —
        // a stop that arrived before the app ever ran.
        Ok(None) => {
            session.set_state_if_live(SessionState::Exited { success: false });
            return;
        }
        Err(err) => {
            session.set_state_if_live(SessionState::Failed {
                reason: format!("{err:#}"),
            });
            return;
        }
    };
    session.set_running();

    while let Ok(line) = lines.recv() {
        ingest_line(&session, &runner, &line);
    }

    // The stream closed: the app's output hit EOF and the child was reaped.
    // A session being torn down keeps whatever state teardown recorded.
    if session.stopping() {
        return;
    }
    let success = session.wait_stream().unwrap_or(false);
    session.set_state_if_live(SessionState::Exited { success });
}

/// Drives the target's run pipeline to a live stream, storing the
/// [`frust_drive::process::StreamHandle`] on the session and returning a
/// receiver over its lines. `Ok(None)` means the session never reached a
/// stream this thread owns: either the pipeline observed the cancel flag
/// before the app started, or a stop overtook the spawn and the just-spawned
/// process was killed here.
fn start(session: &Arc<Session>, runner: &Runner) -> Result<Option<LineReceiver>> {
    // Flavor/defines/build-name are not part of this engine's launch surface
    // yet — a session runs the mode's plain funnel, which is what selects the
    // `frust/devtools` cargo feature the whole discovery flow depends on
    // (`BuildMode::cargo_features`).
    let info = BuildInfo {
        mode: session.mode,
        flavor: None,
        defines: HashMap::new(),
        build_name: None,
        build_number: None,
    };
    let root = session.project_root.clone();

    let handle = match &session.target {
        RunTarget::Desktop => Some(desktop_run::spawn_desktop_session(
            runner.as_ref(),
            &root,
            &info,
        )?),
        RunTarget::Android(device) => {
            let mut on_line = |line: &str| ingest_line(session, runner, line);
            android_run::spawn_session(
                runner.as_ref(),
                &root,
                device,
                &info,
                &mut on_line,
                &session.stop,
            )?
            .map(|launch| launch.stream)
        }
        RunTarget::IosSimulator(device) => {
            let mut on_line = |line: &str| ingest_line(session, runner, line);
            ios_run::spawn_session(
                runner.as_ref(),
                &root,
                device,
                &info,
                &mut on_line,
                &session.stop,
            )?
            .map(|launch| {
                // Record what was actually launched: killing the
                // `simctl launch` stream ends the foreground bridge, not the
                // app, so teardown needs this id to terminate it.
                session.note_ios_bundle_id(launch.bundle_id);
                launch.stream
            })
        }
    };

    let Some(handle) = handle else {
        return Ok(None);
    };
    let lines = handle.lines.clone();
    if let Some(mut orphaned) = session.set_stream(handle) {
        // A stop landed while this launch was still building/spawning: the
        // session's resource set is already torn down, so nothing else will
        // ever reap what we just spawned. Kill it here — both calls are
        // idempotent and block only until the reader thread reaps the child —
        // and report the launch as cancelled.
        orphaned.kill();
        orphaned.wait();
        return Ok(None);
    }
    Ok(Some(lines))
}

/// Retains one output line and acts on anything it announces. Called from
/// the pipeline's own `on_line` sink during build/install and from the drain
/// loop afterwards, so both phases are scanned identically.
pub(crate) fn ingest_line(session: &Arc<Session>, runner: &Runner, line: &str) {
    // The *retained* copy is redacted, never the one parsed below: a discovery
    // line carries the app's handshake token, and the ring is what `app_logs`
    // hands an agent back. Redacting at the ring's edge is what makes that
    // unreachable — the connect path keeps reading the real token off the
    // original line.
    session.push_log(redact_discovery_token(line).into_owned());
    if session.stopping() {
        return;
    }

    if let Some(discovery) = parse_discovery_line(line) {
        if session.note_discovery(discovery.clone()) {
            devtools::spawn_connect(session, runner, discovery);
        }
        return;
    }
    // Not a discovery line, but possibly the app reporting that its service
    // could not bind at all. Recorded verbatim: the common device cause is a
    // per-app network toggle (`docs/LIMITATIONS.md`
    // `devtools-android-per-app-network-toggle`), and an agent can only act
    // on that if it reads what the app actually said.
    if let Some(reason) = parse_failure_line(line) {
        session.note_devtools_failure(reason.to_string());
        return;
    }

    if let Some(pid) = parse_pid_marker(line) {
        if let Some((serial, pid, package)) = session.note_pid(pid) {
            metrics::spawn_sampler(session, runner, serial, pid, package);
        }
        return;
    }
    // iOS logs the same `Launching …` marker with a bundle id rather than an
    // Android package, so only an Android session reads it.
    if session.target.android_serial().is_some()
        && let Some(package) = parse_launch_marker(line)
        && let Some((serial, pid, package)) = session.note_android_package(package)
    {
        metrics::spawn_sampler(session, runner, serial, pid, package);
    }
}

/// Parses the drive's `Streaming logs (pid N)` marker into the pid.
fn parse_pid_marker(line: &str) -> Option<String> {
    let pid = line.strip_prefix(PID_MARKER_PREFIX)?.strip_suffix(')')?;
    (!pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())).then(|| pid.to_string())
}

/// Parses the drive's `Launching <package>…` marker into the package name.
fn parse_launch_marker(line: &str) -> Option<String> {
    let package = line.strip_prefix(LAUNCH_MARKER_PREFIX)?.strip_suffix('…')?;
    (!package.is_empty()).then(|| package.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use frust_drive::build_info::BuildMode;
    use frust_drive::process::{FakeProcessRunner, TryRecvError};

    use super::super::session::SessionId;
    use super::super::test_support::RecordingRunner;
    use super::*;

    /// The exact invocation `frust_drive::desktop_run` resolves a Debug
    /// desktop session to — the key the scripted stream is registered under.
    /// A drift in the drive's mode→args funnel fails the spawn (and this
    /// test) loudly rather than silently launching nothing.
    const DEBUG_DESKTOP_INVOCATION: &str =
        "cargo run --features frust/perf-trace --features frust/devtools";

    /// A launch that loses the race to teardown must kill the process it just
    /// spawned: the session's resource set is already gone, so nothing else
    /// ever would.
    #[test]
    fn a_launch_that_loses_to_teardown_kills_what_it_spawned() {
        let session = Arc::new(Session::new(
            SessionId(1),
            RunTarget::Desktop,
            BuildMode::Debug,
            PathBuf::from("/tmp/frust-mcp-launch-test"),
        ));
        let recorder = Arc::new(RecordingRunner::new(
            FakeProcessRunner::new()
                .with_hanging_stream(DEBUG_DESKTOP_INVOCATION, Vec::<String>::new()),
        ));
        let runner: Runner = Arc::clone(&recorder) as Runner;

        // The whole teardown ran while this launch was still spawning.
        session.request_stop();
        let _ = session.take_resources();

        let started = start(&session, &runner).expect("the desktop launch spawns");
        assert!(
            started.is_none(),
            "a launch whose stream was never stored must not report a live stream"
        );

        let lines = recorder.spawned();
        let [stream] = lines.as_slice() else {
            panic!("expected exactly one spawned process, got {}", lines.len());
        };
        // A scripted hanging stream closes its buffer only after `kill`, so a
        // disconnected receiver here *is* the kill — and `start` already
        // waited for it, hence no blocking read.
        assert_eq!(stream.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn pid_marker_parses_the_drive_line_verbatim() {
        assert_eq!(
            parse_pid_marker("Streaming logs (pid 4242)").as_deref(),
            Some("4242")
        );
        assert_eq!(parse_pid_marker("Streaming logs (pid )"), None);
        assert_eq!(parse_pid_marker("Streaming logs (pid abc)"), None);
        assert_eq!(parse_pid_marker("Installing on device…"), None);
    }

    #[test]
    fn launch_marker_parses_the_installed_package() {
        assert_eq!(
            parse_launch_marker("Launching com.example.app…").as_deref(),
            Some("com.example.app")
        );
        // The ellipsis is a single character, not three dots — a plain
        // `Launching x...` is a different line and must not match.
        assert_eq!(parse_launch_marker("Launching com.example.app..."), None);
        assert_eq!(parse_launch_marker("Launching …"), None);
    }
}
