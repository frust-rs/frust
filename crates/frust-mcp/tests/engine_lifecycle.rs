//! End-to-end tests for [`frust_mcp::SessionEngine`]: a desktop session
//! launched through a scripted `FakeProcessRunner`, whose output announces a
//! devtools service that the shared NDJSON fixture server on real loopback
//! answers. See `common/mod.rs` for the harness contract (no real processes,
//! protocol leaf only, no sleeps).

use std::sync::Arc;

mod common;

use common::devtools::{FIXTURE_TOKEN, Fixture, FixtureConfig, fixture_frame};
use common::{TEST_PROJECT_ROOT, await_snapshot, engine_with_hanging_desktop_stream};
use frust_devtools_protocol::{Capability, format_discovery_line, format_failure_line};
use frust_drive::build_info::BuildMode;
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{RunTarget, SessionState, TERMINAL_SESSION_CAP};

/// The headline lifecycle: launch → the app announces a devtools service →
/// the engine connects and handshakes → `stop_app` kills and joins cleanly.
#[tokio::test]
async fn desktop_session_connects_devtools_then_stops_cleanly() {
    let fixture = Fixture::spawn(FixtureConfig::default());
    let engine = engine_with_hanging_desktop_stream(vec![
        "Compiling frust v0.1.0".to_string(),
        format_discovery_line(fixture.addr.port(), Some(FIXTURE_TOKEN)),
    ]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let snapshot =
        await_snapshot(&engine, id, |s| s.state == SessionState::DevtoolsConnected).await;

    // The handshake's declared capabilities are recorded on the session, so a
    // tool can check "does this app support X?" without a round trip.
    let handshake = snapshot
        .devtools_handshake
        .as_ref()
        .expect("a connected session carries its handshake");
    assert_eq!(handshake.app_name, "fixture-app");
    assert_eq!(
        snapshot.devtools_capabilities(),
        Some(
            [
                Capability::WidgetTree,
                Capability::FrameStats,
                Capability::Input
            ]
            .as_slice()
        )
    );
    // Desktop connects directly to the discovered port — no `adb forward`.
    assert_eq!(snapshot.devtools_port, Some(fixture.addr.port()));
    assert!(snapshot.devtools_error.is_none());
    // The client is reachable for the tool layer.
    assert!(engine.devtools_client(id).is_some());
    // Desktop has no pid seam, so metrics can never start — the tool layer
    // must render that honestly rather than as a zeroed reading.
    assert!(!snapshot.metrics_sampling);

    engine.stop_app(id).expect("stop_app");
    let stopped = engine
        .session(id)
        .expect("the session outlives its process");
    assert_eq!(stopped.state, SessionState::Exited { success: false });

    drop(engine);
    // The fixture server's accept loop ends when the client's socket closes;
    // joining it proves the engine really did drop the connection.
    fixture.join();
}

/// A session whose app reports that its devtools service could not start must
/// carry the reason **verbatim** — an agent can only act on the per-app
/// network-toggle case if it reads exactly what the app said.
#[tokio::test]
async fn devtools_failure_line_surfaces_verbatim() {
    const REASON: &str = "Connection refused (os error 111)";
    let engine = engine_with_hanging_desktop_stream(vec![
        "Compiling frust v0.1.0".to_string(),
        format_failure_line(REASON),
    ]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let snapshot = await_snapshot(&engine, id, |s| s.devtools_error.is_some()).await;

    assert_eq!(snapshot.devtools_error.as_deref(), Some(REASON));
    // The app itself is fine — only its debug service is missing.
    assert_eq!(snapshot.state, SessionState::Running);
    assert!(snapshot.devtools_handshake.is_none());

    engine.stop_app(id).expect("stop_app");
}

/// Every line the session emits is retained, and `logs` hands back the most
/// recent `n` in arrival order. (Eviction past the ring's capacity is pinned
/// at the real `LOG_RING_CAP` by the ring's own unit test — driving 10k lines
/// through a scripted stream would race two independently drop-oldest rings.)
#[tokio::test]
async fn log_ring_retains_lines_and_serves_a_tail() {
    let lines: Vec<String> = (0..12).map(|i| format!("line {i}")).collect();
    let engine = engine_with_hanging_desktop_stream(lines.clone());

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let snapshot = await_snapshot(&engine, id, |s| s.log_lines >= 12).await;
    assert_eq!(snapshot.dropped_log_lines, 0);

    assert_eq!(engine.logs(id, None).expect("logs"), lines);
    assert_eq!(
        engine.logs(id, Some(3)).expect("logs"),
        vec![
            "line 9".to_string(),
            "line 10".to_string(),
            "line 11".to_string()
        ]
    );

    engine.stop_app(id).expect("stop_app");
}

/// A frame-stats notification pushed by the fixture lands in the session's
/// ring, field for field.
#[tokio::test]
async fn frame_stats_notifications_land_in_the_session_ring() {
    let fixture = Fixture::spawn(FixtureConfig::default());
    let engine = engine_with_hanging_desktop_stream(vec![format_discovery_line(
        fixture.addr.port(),
        Some(FIXTURE_TOKEN),
    )]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let snapshot = await_snapshot(&engine, id, |s| s.frames >= 1).await;
    assert_eq!(snapshot.dropped_frames, 0);
    assert_eq!(
        engine.frame_ring(id).expect("frame ring"),
        vec![fixture_frame()]
    );

    engine.stop_app(id).expect("stop_app");
    drop(engine);
    fixture.join();
}

/// A launch the runner cannot even spawn ends as `Failed`, carrying the
/// drive's own error chain rather than hanging in `Launching` forever.
#[tokio::test]
async fn an_unspawnable_launch_fails_with_a_reason() {
    // No stream registered for the desktop invocation at all.
    let engine = SessionEngine::with_runner(TEST_PROJECT_ROOT, Arc::new(FakeProcessRunner::new()));

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let snapshot = await_snapshot(&engine, id, |s| s.state.is_terminal()).await;

    match snapshot.state {
        SessionState::Failed { ref reason } => {
            assert!(
                reason.contains("cargo"),
                "the failure must name what could not be spawned, got {reason:?}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// `shutdown` tears down every live session, so a cancelled MCP server leaves
/// nothing running behind it.
#[tokio::test]
async fn shutdown_stops_every_session() {
    let engine = engine_with_hanging_desktop_stream(vec!["Compiling frust v0.1.0".to_string()]);
    let first = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    let second = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, first, |s| s.state == SessionState::Running).await;
    await_snapshot(&engine, second, |s| s.state == SessionState::Running).await;

    engine.shutdown().await;

    for snapshot in engine.sessions() {
        assert!(
            snapshot.state.is_terminal(),
            "session {} survived shutdown: {snapshot:?}",
            snapshot.id
        );
    }
}

/// A long-lived server's session history is bounded: terminal sessions are
/// retained for reading back, but only the most recent `TERMINAL_SESSION_CAP`
/// of them — and a still-running session is never evicted to make room,
/// whatever its age (evicting one would leave a live app with no handle to
/// stop it).
#[tokio::test]
async fn terminal_sessions_are_bounded_and_a_live_one_is_never_evicted() {
    // Only the Debug desktop invocation is scripted, so the profile-mode
    // launches below cannot spawn and land terminal (`Failed`) at once.
    let engine = engine_with_hanging_desktop_stream(vec!["Compiling frust v0.1.0".to_string()]);
    let live = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, live, |s| s.state == SessionState::Running).await;

    let mut ended = Vec::new();
    for _ in 0..(TERMINAL_SESSION_CAP + 3) {
        let id = engine.run_app(RunTarget::Desktop, BuildMode::Profile);
        // Awaited one at a time: each session must actually *be* terminal
        // before the next insert, which is when retention is enforced.
        await_snapshot(&engine, id, |s| s.state.is_terminal()).await;
        ended.push(id);
    }

    assert!(
        engine.session(live).is_some(),
        "the live session was evicted for capacity"
    );
    assert!(
        engine.session(ended[0]).is_none(),
        "the oldest terminal session was retained past the cap"
    );
    assert!(
        engine
            .session(*ended.last().expect("a launched session"))
            .is_some(),
        "the newest terminal session must still be readable"
    );
    // The live session, the capped terminal set, and at most the newest
    // terminal session (whose own insert swept before it ended).
    assert!(
        engine.sessions().len() <= TERMINAL_SESSION_CAP + 2,
        "session retention is unbounded: {} retained",
        engine.sessions().len()
    );

    engine.shutdown().await;
}

/// `restart_app` stops the old session and starts a new one with the same
/// target, mode, and project root.
#[tokio::test]
async fn restart_replaces_the_session_with_an_identical_spec() {
    let engine = engine_with_hanging_desktop_stream(vec!["Compiling frust v0.1.0".to_string()]);
    let first = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    await_snapshot(&engine, first, |s| s.state == SessionState::Running).await;

    let second = engine.restart_app(first).expect("restart_app");
    assert_ne!(first, second);
    let restarted = await_snapshot(&engine, second, |s| s.state == SessionState::Running).await;
    assert_eq!(restarted.mode, BuildMode::Debug);
    assert_eq!(restarted.project_root, engine.project_root().to_path_buf());
    assert!(matches!(restarted.target, RunTarget::Desktop));

    // The old id keeps reporting its own final state rather than vanishing.
    let old = engine.session(first).expect("the old session is retained");
    assert!(old.state.is_terminal());

    engine.shutdown().await;
}

/// An unknown id is an error, never a panic — an agent replaying a stale
/// session id must get a clean refusal.
#[tokio::test]
async fn unknown_session_ids_are_refused_cleanly() {
    let engine = SessionEngine::new(TEST_PROJECT_ROOT);
    let unknown = frust_mcp::engine::SessionId(9_999);
    assert!(engine.session(unknown).is_none());
    assert!(engine.logs(unknown, None).is_none());
    assert!(engine.frame_ring(unknown).is_none());
    assert!(engine.latest_metrics(unknown).is_none());
    assert!(engine.devtools_client(unknown).is_none());
    assert!(engine.stop_app(unknown).is_err());
    assert!(engine.restart_app(unknown).is_err());
}
