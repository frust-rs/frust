//! Teardown-race tests for [`frust_mcp::SessionEngine`]: what happens when a
//! stop or a shutdown overtakes a launch that is still starting.
//!
//! The contract under test is `frust_mcp`'s no-orphans one — a stopped or
//! shut-down engine leaves no live process and no host-side `adb forward`
//! behind. Every wait here is on something the engine itself produces (a
//! spawn, an output stream closing, a state transition); the only durations
//! are failure deadlines. See `common/mod.rs` for the rest of the harness
//! contract.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

mod common;

use anyhow::Result;
use common::{DEADLINE, DEBUG_DESKTOP_INVOCATION, TEST_PROJECT_ROOT, await_snapshot};
use frust_drive::build_info::BuildMode;
use frust_drive::process::{
    FakeProcessRunner, LineReceiver, Output, ProcessRunner, StreamHandle, TryRecvError,
};
use frust_mcp::SessionEngine;
use frust_mcp::engine::{RunTarget, SessionState};

/// A runner that hands out the scripted fake's streams and keeps a clone of
/// each one's line receiver.
///
/// That clone is how a test observes a **kill** without a real process: a
/// scripted hanging stream closes its line buffer only once
/// `StreamHandle::kill` has run, so "the receiver reported end-of-stream" is
/// exactly "somebody killed the process".
struct RecordingRunner {
    inner: FakeProcessRunner,
    spawned: Mutex<Sender<LineReceiver>>,
}

impl RecordingRunner {
    /// The runner plus the channel every spawned stream's receiver arrives on.
    fn new(inner: FakeProcessRunner) -> (Arc<Self>, Receiver<LineReceiver>) {
        let (tx, rx) = mpsc::channel();
        (
            Arc::new(Self {
                inner,
                spawned: Mutex::new(tx),
            }),
            rx,
        )
    }
}

impl ProcessRunner for RecordingRunner {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
        self.inner.run(cmd, args)
    }

    fn run_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<Output> {
        self.inner.run_streaming(cmd, args, cwd, env, on_line)
    }

    fn spawn_streaming(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<StreamHandle> {
        let handle = self.inner.spawn_streaming(cmd, args, cwd, env)?;
        let _ = self
            .spawned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .send(handle.lines.clone());
        Ok(handle)
    }
}

/// An engine over a [`RecordingRunner`] whose desktop launch replays `lines`
/// and then hangs, plus the spawned-stream channel.
fn recording_engine(lines: Vec<String>) -> (SessionEngine, Receiver<LineReceiver>) {
    let (runner, spawned) = RecordingRunner::new(
        FakeProcessRunner::new().with_hanging_stream(DEBUG_DESKTOP_INVOCATION, lines),
    );
    (
        SessionEngine::with_runner(TEST_PROJECT_ROOT, runner),
        spawned,
    )
}

/// Blocks (on a throwaway thread, so the wait is bounded) until `lines`
/// reports end-of-stream — which a scripted hanging stream only does once its
/// process has been killed.
fn assert_stream_killed(lines: LineReceiver) {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        while lines.recv().is_ok() {}
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(DEADLINE).expect(
        "the spawned process was never killed: its output stream is still open past the deadline",
    );
}

/// A `stop_app` issued before the launch has even reached its process must
/// still leave nothing running: whichever side of the race ends up owning the
/// stream handle is the side that kills it.
#[tokio::test]
async fn a_stop_racing_the_launch_still_kills_the_process() {
    let (engine, spawned) = recording_engine(vec!["Compiling frust v0.1.0".to_string()]);

    // Deliberately no state await between the two: this is the window where
    // the launch thread may not have spawned anything yet.
    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    engine.stop_app(id).expect("stop_app");

    // The desktop pipeline has no cancel check, so the process is spawned
    // regardless of who won — and must not survive the stop.
    let lines = spawned
        .recv_timeout(DEADLINE)
        .expect("the desktop launch spawns its process");
    assert_stream_killed(lines);

    let stopped = engine
        .session(id)
        .expect("the session outlives its process");
    assert_eq!(stopped.state, SessionState::Exited { success: false });

    // Nothing was left registered to launch a second time.
    assert!(spawned.try_recv().is_err());
}

/// Same race, one step later: the launch is already streaming when the stop
/// lands. The stream is the one teardown itself owns here.
#[tokio::test]
async fn a_stop_after_the_stream_is_live_kills_it_too() {
    let (engine, spawned) = recording_engine(vec!["Compiling frust v0.1.0".to_string()]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, id, |s| s.state == SessionState::Running).await;
    let lines = spawned
        .recv_timeout(DEADLINE)
        .expect("the desktop launch spawns its process");

    engine.stop_app(id).expect("stop_app");
    assert_stream_killed(lines);
}

/// A burst of launches issued *while* the shutdown sweep is running: whether
/// a session is refused outright, swept by the sweep, or torn down by the
/// launch that lost the race, none of them may be left running.
#[tokio::test]
async fn launches_racing_shutdown_are_never_left_running() {
    const BURST: usize = 8;

    let (engine, spawned) = recording_engine(vec!["Compiling frust v0.1.0".to_string()]);
    let engine = Arc::new(engine);

    let launcher = {
        let engine = Arc::clone(&engine);
        tokio::task::spawn_blocking(move || {
            (0..BURST)
                .map(|_| engine.run_app(RunTarget::Desktop, BuildMode::Debug))
                .collect::<Vec<_>>()
        })
    };
    engine.shutdown().await;
    let ids = launcher.await.expect("the launch burst");

    for id in ids {
        let snapshot = engine
            .wait_for(id, DEADLINE, |s| s.state.is_terminal())
            .await
            .expect("every launched session is retained");
        assert!(
            snapshot.state.is_terminal(),
            "session {id} survived the shutdown race: {snapshot:?}"
        );
    }

    // Every process that did get spawned is dead — a session that reached a
    // terminal state has already killed and reaped whatever it owned.
    while let Ok(lines) = spawned.try_recv() {
        assert_stream_killed(lines);
    }
}

/// The stronger form of the burst test above: a single launch racing the
/// shutdown, asserted **the moment `shutdown` returns** rather than by a later
/// generous poll. Whichever way the race resolves — the launch's critical
/// section ran first and the sweep swept it, or the sweep ran first and the
/// launch was refused — there is nothing left alive when `shutdown().await`
/// hands control back.
#[tokio::test]
async fn a_launch_racing_shutdown_is_already_dead_when_shutdown_returns() {
    // No scripted lines at all: the stream's line buffer then closes for
    // exactly one reason, a kill, so a disconnected receiver is unambiguous.
    let (engine, spawned) = recording_engine(Vec::new());
    let engine = Arc::new(engine);

    let launcher = {
        let engine = Arc::clone(&engine);
        tokio::task::spawn_blocking(move || engine.run_app(RunTarget::Desktop, BuildMode::Debug))
    };
    engine.shutdown().await;
    let id = launcher.await.expect("the racing launch");

    // Read without waiting: anything still open here outlived the shutdown
    // that claimed to have finished.
    while let Ok(lines) = spawned.try_recv() {
        assert_eq!(
            lines.try_recv(),
            Err(TryRecvError::Disconnected),
            "a process the racing launch spawned was still alive when shutdown returned"
        );
    }
    let snapshot = engine
        .session(id)
        .expect("a session that got an id back is readable");
    assert!(
        snapshot.state.is_terminal(),
        "the racing launch is neither refused nor torn down: {snapshot:?}"
    );
}

/// After `shutdown`, a `run_app` racing it launches nothing and says why —
/// an app started past the sweep meant to end it would outlive the server.
#[tokio::test]
async fn run_app_after_shutdown_is_refused_with_the_reason() {
    let (engine, spawned) = recording_engine(vec!["Compiling frust v0.1.0".to_string()]);

    let first = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, first, |s| s.state == SessionState::Running).await;
    let live = spawned
        .recv_timeout(DEADLINE)
        .expect("the first session spawns its process");

    engine.shutdown().await;
    assert_stream_killed(live);

    let refused = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let snapshot = engine
        .session(refused)
        .expect("a refused launch is still reportable");
    match snapshot.state {
        SessionState::Failed { ref reason } => assert!(
            reason.contains("shutting down"),
            "the refusal must say the server is shutting down, got {reason:?}"
        ),
        ref other => panic!("expected a refusal, got {other:?}"),
    }
    // Refused means refused: no second process was spawned at all.
    assert!(
        spawned.try_recv().is_err(),
        "a refused run_app must not spawn anything"
    );
}
