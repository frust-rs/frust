//! Shared test harness: a raw-HTTP MCP client, a hand-rolled NDJSON devtools
//! fixture server, and the scripted engines both are driven against.
//!
//! **No process is ever really spawned** — every external invocation goes
//! through the injected `ProcessRunner`, per `docs/CODE_STANDARDS.md`.
//!
//! **The fixture server speaks the protocol leaf only.** It reimplements just
//! enough JSON-RPC framing to drive the client, exactly as `frust-drive`'s
//! own `devtools_client` tests do, and deliberately does *not* depend on
//! `frust-devtools` — the tooling-isolation charter (`docs/ARCHITECTURE.md`)
//! forbids that edge from this crate, dev-dependency or not.
//!
//! **No sleeps.** Every wait is on something the engine or the server itself
//! produces: a session state transition (through the session's own change
//! signal), or an HTTP response. The only durations here are failure
//! deadlines.

// Each integration-test binary includes this module and uses a subset of it.
#![allow(dead_code)]

pub mod devtools;
pub mod mcp;

use std::sync::Arc;
use std::time::Duration;

use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{SessionId, SessionSnapshot};

/// The exact `cargo run` invocation `frust_drive::desktop_run::desktop_plan`
/// resolves a Debug desktop session to — the key the scripted fake stream is
/// registered under. If the drive's mode→args funnel ever changes, this key
/// stops matching and every test here fails loudly rather than silently
/// launching nothing.
pub const DEBUG_DESKTOP_INVOCATION: &str =
    "cargo run --features frust/perf-trace --features frust/devtools";

/// The same invocation for a Profile-mode desktop session — the second,
/// distinctly-scriptable session a multi-session test needs.
pub const PROFILE_DESKTOP_INVOCATION: &str =
    "cargo run --profile profile --features frust/perf-trace --features frust/devtools";

/// How long a test waits for a condition the engine must produce before
/// declaring failure. Generous: it is a failure deadline, never a pacing
/// device.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// A project root no test ever writes to — the engine only ever passes it to
/// the fake runner.
pub const TEST_PROJECT_ROOT: &str = "/tmp/frust-mcp-engine-test";

/// An engine whose desktop launch replays `lines` and then hangs — a live
/// session that only ends when something kills it, like a real preview.
pub fn engine_with_hanging_desktop_stream(lines: Vec<String>) -> SessionEngine {
    SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(FakeProcessRunner::new().with_hanging_stream(DEBUG_DESKTOP_INVOCATION, lines)),
    )
}

/// An engine that can launch two independently scripted desktop sessions —
/// a Debug one replaying `debug_lines` and a Profile one replaying
/// `profile_lines`, both hanging afterwards.
pub fn engine_with_two_desktop_streams(
    debug_lines: Vec<String>,
    profile_lines: Vec<String>,
) -> SessionEngine {
    SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(
            FakeProcessRunner::new()
                .with_hanging_stream(DEBUG_DESKTOP_INVOCATION, debug_lines)
                .with_hanging_stream(PROFILE_DESKTOP_INVOCATION, profile_lines),
        ),
    )
}

/// Blocks (on the session's own change signal) until `predicate` holds,
/// failing the test with the last snapshot seen if it never does.
pub async fn await_snapshot(
    engine: &SessionEngine,
    id: SessionId,
    predicate: impl Fn(&SessionSnapshot) -> bool + Send + Clone + 'static,
) -> SessionSnapshot {
    let check = predicate.clone();
    let snapshot = engine
        .wait_for(id, DEADLINE, predicate)
        .await
        .expect("the session must exist");
    assert!(
        check(&snapshot),
        "condition not reached within {DEADLINE:?}; last snapshot: {snapshot:?}"
    );
    snapshot
}
