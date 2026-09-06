//! End-to-end tests for the MCP tool families, driven over a real MCP
//! connection (initialize → tools/call) against a scripted engine and a
//! fixture devtools server — no device, no spawned process, no sleeps. See
//! `common/mod.rs` for the harness contract.
//!
//! The milestone these exist to pin is the whole agent path: launch an app,
//! inspect its widget tree, tap something by name, take a screenshot.

mod common;

use std::sync::Arc;

use common::devtools::{
    CANCEL_BUTTON_ID, COLUMN_ID, FIXTURE_TOKEN, Fixture, FixtureConfig, SAVE_BUTTON_ID,
    ScreenshotBehavior, fixture_frame,
};
use common::mcp::McpTestServer;
use common::{
    TEST_PROJECT_ROOT, await_snapshot, engine_with_hanging_desktop_stream,
    engine_with_two_desktop_streams,
};
use frust_devtools_protocol::{Capability, format_discovery_line};
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{SessionId, SessionState};
use serde_json::{Value, json};

/// The centre of the fixture tree's "Save" button — what a tap targeted by
/// label must land on.
const SAVE_CENTER: (f64, f64) = (60.0, 40.0);

/// Starts a connected session: a fixture devtools server, an engine whose
/// desktop launch announces it, an MCP server over that engine, and the
/// `run_app` → `devtools_connected` handshake already done.
async fn connected(config: FixtureConfig) -> (Fixture, Arc<SessionEngine>, McpTestServer, u64) {
    let fixture = Fixture::spawn(config);
    let engine = Arc::new(engine_with_hanging_desktop_stream(vec![
        format_discovery_line(fixture.addr.port(), Some(FIXTURE_TOKEN)),
    ]));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;

    let started = mcp.call_ok("run_app", json!({ "target": "desktop" })).await;
    let id = started["session"]["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("run_app returned no session id: {started}"));
    assert_eq!(started["session"]["target"], json!("desktop"));
    assert_eq!(started["session"]["mode"], json!("debug"));

    await_snapshot(&engine, SessionId(id), |s| {
        s.state == SessionState::DevtoolsConnected
    })
    .await;

    (fixture, engine, mcp, id)
}

/// Tears the whole harness down and proves the engine really dropped its
/// devtools connection (the fixture's accept loop only ends when it closes).
async fn teardown(fixture: Fixture, mcp: McpTestServer) {
    mcp.shutdown().await;
    fixture.join();
}

/// The milestone: launch → inspect → tap by label → screenshot, each step a
/// real MCP tool call.
#[tokio::test]
async fn the_milestone_path_launches_inspects_taps_and_screenshots() {
    let (fixture, engine, mcp, id) = connected(FixtureConfig::default()).await;

    // Inspect: the session reports itself connected, with what the app
    // declared.
    let sessions = mcp.call_ok("list_sessions", json!({})).await;
    let session = &sessions["sessions"][0];
    assert_eq!(session["id"], json!(id));
    assert_eq!(session["state"], json!("devtools_connected"));
    assert_eq!(session["devtools_app_name"], json!("fixture-app"));
    assert_eq!(
        session["devtools_capabilities"],
        json!(["widget_tree", "frame_stats", "input"])
    );

    // Find: exactly one widget_tree round trip for the call.
    let before = fixture.request_count("widget_tree");
    let found = mcp
        .call_ok("find_widgets", json!({ "label": "Save" }))
        .await;
    assert_eq!(
        fixture.request_count("widget_tree") - before,
        1,
        "find_widgets must fetch the tree exactly once"
    );
    assert_eq!(found["total_matches"], json!(1));
    assert_eq!(found["tree_nodes"], json!(5));
    assert_eq!(found["truncated"], json!(false));
    let widget = &found["widgets"][0];
    assert_eq!(widget["id"], json!(SAVE_BUTTON_ID));
    assert_eq!(widget["type_name"], json!("ButtonWidget"));
    assert_eq!(
        widget["bounds"],
        json!({"x":10.0,"y":20.0,"width":100.0,"height":40.0})
    );
    assert_eq!(
        widget["center"],
        json!({"x": SAVE_CENTER.0, "y": SAVE_CENTER.1})
    );

    // Tap: by label, landing on that centre — asserted from what the app
    // was actually sent, not from what the tool reported.
    let before = fixture.request_count("widget_tree");
    let tapped = mcp.call_ok("tap", json!({ "label": "Save" })).await;
    assert_eq!(
        fixture.request_count("widget_tree") - before,
        1,
        "a tap by query must fetch the tree exactly once"
    );
    assert_eq!(
        tapped["tapped"],
        json!({"x": SAVE_CENTER.0, "y": SAVE_CENTER.1})
    );
    assert_eq!(tapped["widget"]["id"], json!(SAVE_BUTTON_ID));
    let taps = fixture.requests_of("input_tap");
    assert_eq!(taps.len(), 1, "exactly one tap reached the app");
    assert_eq!(taps[0]["x"], json!(SAVE_CENTER.0));
    assert_eq!(taps[0]["y"], json!(SAVE_CENTER.1));

    // Screenshot: this app declared no screenshot capability and a desktop
    // session has no adb fallback, so the refusal has to say so plainly.
    let refused = mcp.call_err("screenshot", json!({})).await;
    let message = refused["error"].as_str().expect("an error message");
    assert!(
        message.contains("screenshot") && message.contains("not supported in v1"),
        "unhelpful screenshot refusal: {message}"
    );
    assert_eq!(refused["success"], json!(false));
    assert_eq!(
        fixture.request_count("screenshot"),
        0,
        "a capability the app never declared must not cost a round trip"
    );

    // Stopping through the tool leaves the session inspectable.
    let stopped = mcp.call_ok("stop_app", json!({})).await;
    assert_eq!(stopped["session"]["state"], json!("exited"));
    assert!(
        engine
            .session(SessionId(id))
            .expect("the session outlives its process")
            .state
            .is_terminal()
    );

    teardown(fixture, mcp).await;
}

/// An ambiguous widget query is answered with the candidates, so the agent
/// can narrow it without another exploratory call.
#[tokio::test]
async fn an_ambiguous_tap_query_lists_its_candidates() {
    let (fixture, _engine, mcp, _id) = connected(FixtureConfig::default()).await;

    let err = mcp.call_err("tap", json!({ "type_name": "Button" })).await;
    let message = err["error"].as_str().expect("an error message");
    assert!(
        message.contains("2 widgets matched"),
        "unhelpful ambiguity error: {message}"
    );
    let ids: Vec<Value> = err["candidates"]
        .as_array()
        .expect("candidates are listed")
        .iter()
        .map(|candidate| candidate["id"].clone())
        .collect();
    assert_eq!(ids, vec![json!(SAVE_BUTTON_ID), json!(CANCEL_BUTTON_ID)]);
    // Every candidate carries the centre needed to tap it directly instead.
    assert_eq!(
        err["candidates"][0]["center"],
        json!({"x": SAVE_CENTER.0, "y": SAVE_CENTER.1})
    );
    assert_eq!(
        fixture.request_count("input_tap"),
        0,
        "an ambiguous query must not tap anything"
    );

    teardown(fixture, mcp).await;
}

/// Coordinates and a query are alternatives, never a combination — and
/// neither is refused with the next step named.
#[tokio::test]
async fn tap_refuses_a_target_it_cannot_resolve() {
    let (fixture, _engine, mcp, _id) = connected(FixtureConfig::default()).await;

    let both = mcp
        .call_err("tap", json!({ "x": 1.0, "y": 2.0, "label": "Save" }))
        .await;
    assert!(
        both["error"]
            .as_str()
            .expect("message")
            .contains("not both"),
        "{both}"
    );

    let neither = mcp.call_err("tap", json!({})).await;
    assert!(
        neither["error"]
            .as_str()
            .expect("message")
            .contains("find_widgets"),
        "{neither}"
    );

    let half = mcp.call_err("tap", json!({ "x": 1.0 })).await;
    assert!(
        half["error"]
            .as_str()
            .expect("message")
            .contains("must be given together"),
        "{half}"
    );

    // A coordinate tap needs no tree at all.
    let before = fixture.request_count("widget_tree");
    let tapped = mcp.call_ok("tap", json!({ "x": 7.0, "y": 9.0 })).await;
    assert_eq!(tapped["tapped"], json!({"x": 7.0, "y": 9.0}));
    assert!(tapped.get("widget").is_none());
    assert_eq!(fixture.request_count("widget_tree"), before);
    let taps = fixture.requests_of("input_tap");
    assert_eq!(taps.last().expect("one tap")["x"], json!(7.0));

    teardown(fixture, mcp).await;
}

/// The widget-tree dump caps its depth and marks what it elided.
#[tokio::test]
async fn the_widget_tree_depth_cap_truncates_and_says_so() {
    let (fixture, _engine, mcp, _id) = connected(FixtureConfig::default()).await;

    let capped = mcp.call_ok("widget_tree", json!({ "depth": 1 })).await;
    assert_eq!(capped["depth"], json!(1));
    assert_eq!(capped["truncated"], json!(true));
    // Root plus its three children; the column's own child is elided.
    assert_eq!(capped["node_count"], json!(4));
    let column = capped["roots"][0]["children"]
        .as_array()
        .expect("children")
        .iter()
        .find(|child| child["id"] == json!(COLUMN_ID))
        .expect("the column is present");
    assert_eq!(column["children_truncated"], json!(1));
    assert!(column.get("children").is_none());

    let full = mcp.call_ok("widget_tree", json!({})).await;
    assert_eq!(full["truncated"], json!(false));
    assert_eq!(full["node_count"], json!(5));

    teardown(fixture, mcp).await;
}

/// The app's own screenshot is used when it declared the capability, and
/// comes back both as image content and as structured metadata.
#[tokio::test]
async fn a_declared_screenshot_capability_is_used_and_returned_as_an_image() {
    let (fixture, _engine, mcp, id) = connected(
        FixtureConfig::default()
            .with_capabilities(vec![
                Capability::WidgetTree,
                Capability::Input,
                Capability::Screenshot,
            ])
            .with_screenshot(ScreenshotBehavior::Png),
    )
    .await;

    let result = mcp.call_tool("screenshot", json!({})).await;
    assert_ne!(result["isError"], json!(true), "{result}");
    let content = &result["content"][0];
    assert_eq!(content["type"], json!("image"));
    assert_eq!(content["mimeType"], json!("image/png"));
    let structured = &result["structuredContent"];
    assert_eq!(structured["session_id"], json!(id));
    assert_eq!(structured["source"], json!("service"));
    assert_eq!(structured["png_bytes"], json!(8));
    assert_eq!(structured["png_base64"], content["data"]);

    teardown(fixture, mcp).await;
}

/// An app that declares the capability but rejects the call still produces
/// an actionable message, not a raw RPC fault.
#[tokio::test]
async fn a_service_side_not_supported_rejection_is_explained() {
    let (fixture, _engine, mcp, _id) = connected(
        FixtureConfig::default()
            .with_capabilities(vec![Capability::WidgetTree, Capability::Screenshot])
            .with_screenshot(ScreenshotBehavior::NotSupported),
    )
    .await;

    let err = mcp.call_err("screenshot", json!({})).await;
    let message = err["error"].as_str().expect("message");
    assert!(
        message.contains("does not support screenshot"),
        "unhelpful rejection: {message}"
    );
    assert_eq!(fixture.request_count("screenshot"), 1);

    teardown(fixture, mcp).await;
}

/// Frame stats pushed by the app aggregate through the performance tool.
#[tokio::test]
async fn performance_aggregates_the_frames_the_app_pushed() {
    let (fixture, engine, mcp, id) = connected(FixtureConfig::default()).await;
    await_snapshot(&engine, SessionId(id), |s| s.frames >= 1).await;

    let performance = mcp.call_ok("performance", json!({})).await;
    assert_eq!(performance["sample_count"], json!(1));
    assert_eq!(performance["dropped_samples"], json!(0));
    assert!(performance.get("note").is_none(), "{performance}");
    let stats = &performance["stats"];
    // Durations are reported rounded to two decimals, so the expectation is
    // derived the same way rather than hardcoded.
    let expected_ms = (fixture_frame().total_us as f64 / 10.0).round() / 100.0;
    assert_eq!(stats["frame_ms_p50"], json!(expected_ms));
    assert_eq!(stats["frame_ms_max"], json!(expected_ms));
    assert_eq!(stats["jank_frames"], json!(0));
    assert_eq!(stats["jank_threshold_ms"], json!(16.7));
    assert_eq!(stats["phase_means_ms"]["rebuild"], json!(0.1));

    teardown(fixture, mcp).await;
}

/// A session that has rendered nothing says so, rather than reporting a
/// zeroed frame budget.
#[tokio::test]
async fn performance_with_no_frames_says_so_instead_of_reporting_zeros() {
    let engine = Arc::new(engine_with_hanging_desktop_stream(vec![
        "Compiling frust v0.1.0".to_string(),
    ]));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;
    let started = mcp.call_ok("run_app", json!({ "target": "desktop" })).await;
    let id = started["session"]["id"].as_u64().expect("session id");
    await_snapshot(&engine, SessionId(id), |s| s.state == SessionState::Running).await;

    let performance = mcp.call_ok("performance", json!({})).await;
    assert_eq!(performance["sample_count"], json!(0));
    assert!(performance.get("stats").is_none(), "{performance}");
    let note = performance["note"].as_str().expect("a note");
    assert!(note.contains("no frame samples yet"), "{note}");

    mcp.shutdown().await;
}

/// Metrics never invent a reading: a desktop session cannot be sampled, and
/// the tool reports that reason instead of zeros.
#[tokio::test]
async fn metrics_report_an_unsampleable_target_as_unavailable() {
    let (fixture, _engine, mcp, _id) = connected(FixtureConfig::default()).await;

    let metrics = mcp.call_ok("metrics", json!({})).await;
    assert_eq!(metrics["system"]["available"], json!(false));
    let reason = metrics["system"]["unavailable_reason"]
        .as_str()
        .expect("a reason");
    assert!(reason.contains("Android-only"), "{reason}");
    assert!(metrics["system"].get("cpu_percent").is_none());
    assert!(metrics["system"].get("rss_bytes").is_none());

    // The app's own snapshot is a separate collector and does answer.
    assert_eq!(metrics["service"]["available"], json!(true));
    assert_eq!(metrics["service"]["rss_bytes"], json!(42 * 1024 * 1024));
    assert_eq!(metrics["service"]["uptime_ms"], json!(1_234));

    teardown(fixture, mcp).await;
}

/// Logs come back filtered and tailed, with the totals needed to tell a
/// truncated view from a complete one.
#[tokio::test]
async fn app_logs_filter_and_tail_the_session_ring() {
    let lines: Vec<String> = (0..10)
        .map(|i| {
            if i % 2 == 0 {
                format!("I/frust: step {i}")
            } else {
                format!("E/frust: failure {i}")
            }
        })
        .collect();
    let engine = Arc::new(engine_with_hanging_desktop_stream(lines));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;
    let started = mcp.call_ok("run_app", json!({ "target": "desktop" })).await;
    let id = started["session"]["id"].as_u64().expect("session id");
    await_snapshot(&engine, SessionId(id), |s| s.log_lines >= 10).await;

    let all = mcp.call_ok("app_logs", json!({})).await;
    assert_eq!(all["matched"], json!(10));
    assert_eq!(all["retained"], json!(10));
    assert_eq!(all["dropped"], json!(0));

    let errors = mcp
        .call_ok("app_logs", json!({ "level": "error", "limit": 2 }))
        .await;
    assert_eq!(errors["matched"], json!(5));
    assert_eq!(
        errors["lines"],
        json!(["E/frust: failure 7", "E/frust: failure 9"])
    );

    let filtered = mcp
        .call_ok("app_logs", json!({ "pattern": "STEP 4" }))
        .await;
    assert_eq!(filtered["lines"], json!(["I/frust: step 4"]));

    mcp.shutdown().await;
}

/// The app's devtools token never reaches an agent: it is redacted on the way
/// *into* the log ring, so no `app_logs` filter can dig it back out — while
/// the connect path, which reads the real token off the original line, still
/// handshook (the session reached `devtools_connected` above).
#[tokio::test]
async fn app_logs_never_hand_back_the_devtools_token() {
    let (fixture, _engine, mcp, id) = connected(FixtureConfig::default()).await;

    // Deliberately unfiltered and generous: whatever the ring holds is what an
    // agent could read.
    let logs = mcp.call_ok("app_logs", json!({ "limit": 2_000 })).await;
    assert_eq!(logs["session_id"], json!(id));
    let lines = logs["lines"].as_array().expect("the lines are listed");
    assert!(!lines.is_empty(), "the session logged nothing: {logs}");
    for line in lines {
        let text = line.as_str().expect("a log line");
        assert!(
            !text.contains(FIXTURE_TOKEN),
            "app_logs returned the devtools token: {text}"
        );
    }
    // The discovery line itself is still retained — only its token is gone,
    // so an agent can still see that the service announced itself.
    assert!(
        lines.iter().any(|line| line
            .as_str()
            .is_some_and(|text| text.contains("<redacted>"))),
        "the discovery line was dropped rather than redacted: {logs}"
    );
    // A targeted search for the token finds nothing either.
    let hunted = mcp
        .call_ok("app_logs", json!({ "pattern": FIXTURE_TOKEN }))
        .await;
    assert_eq!(hunted["matched"], json!(0));

    teardown(fixture, mcp).await;
}

/// Session resolution counts only *live* sessions: a single running session
/// resolves straight through a pile of ended ones, and with nothing running
/// the ambiguity says so rather than reporting dead sessions as running.
#[tokio::test]
async fn session_resolution_counts_only_live_sessions() {
    // Only the Debug desktop invocation is scripted, so a profile-mode launch
    // cannot spawn and lands terminal at once.
    let engine = Arc::new(engine_with_hanging_desktop_stream(vec![
        "Compiling frust v0.1.0".to_string(),
    ]));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;

    for _ in 0..3 {
        let failed = mcp
            .call_ok("run_app", json!({ "target": "desktop", "mode": "profile" }))
            .await;
        let id = failed["session"]["id"].as_u64().expect("session id");
        await_snapshot(&engine, SessionId(id), |s| s.state.is_terminal()).await;
    }

    let ambiguous = mcp.call_err("app_logs", json!({})).await;
    let message = ambiguous["error"].as_str().expect("a message");
    assert!(
        message.contains("no session is running") && message.contains("3 have ended"),
        "a dead session was reported as running: {message}"
    );
    assert_eq!(
        ambiguous["sessions"]
            .as_array()
            .expect("the ended sessions are listed")
            .len(),
        3
    );

    // One live session among them is the obvious one — no ambiguity at all.
    let started = mcp.call_ok("run_app", json!({ "target": "desktop" })).await;
    let live = started["session"]["id"].as_u64().expect("session id");
    await_snapshot(&engine, SessionId(live), |s| {
        s.state == SessionState::Running
    })
    .await;

    let logs = mcp.call_ok("app_logs", json!({})).await;
    assert_eq!(
        logs["session_id"],
        json!(live),
        "resolution must pick the only live session, not an ended one"
    );

    mcp.shutdown().await;
}

/// Every tool is listed with an input schema and a description an agent can
/// actually work from — the only documentation it ever gets.
#[tokio::test]
async fn every_tool_is_listed_with_a_usable_description() {
    let engine = Arc::new(SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(FakeProcessRunner::new()),
    ));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;

    let listed = mcp.list_tools().await;
    let tools = listed["tools"].as_array().expect("a tool list");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool name"))
        .collect();
    for expected in [
        "ping",
        "list_devices",
        "list_sessions",
        "run_app",
        "stop_app",
        "restart_app",
        "app_logs",
        "find_widgets",
        "tap",
        "scroll",
        "enter_text",
        "widget_props",
        "widget_tree",
        "performance",
        "metrics",
        "screenshot",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }

    for tool in tools {
        let name = tool["name"].as_str().expect("a tool name");
        let description = tool["description"]
            .as_str()
            .unwrap_or_else(|| panic!("tool {name} has no description"));
        assert!(
            description.len() > 40,
            "tool {name}'s description is too thin to use without docs: {description:?}"
        );
        assert_eq!(
            tool["inputSchema"]["type"],
            json!("object"),
            "tool {name} has no object input schema"
        );
    }

    // The argument schemas are derived, so a rename cannot silently drift
    // from what the tool actually reads.
    let tap = tools
        .iter()
        .find(|tool| tool["name"] == json!("tap"))
        .expect("tap is listed");
    let properties = tap["inputSchema"]["properties"]
        .as_object()
        .expect("tap takes arguments");
    for key in ["x", "y", "type_name", "label"] {
        assert!(properties.contains_key(key), "tap is missing `{key}`");
    }

    // `run_app` takes no client-suppliable project path — every session
    // launches at the server's own configured root (review-fix-1 F3: a
    // per-call override let any caller run `cargo` anywhere readable).
    let run_app = tools
        .iter()
        .find(|tool| tool["name"] == json!("run_app"))
        .expect("run_app is listed");
    let run_app_properties = run_app["inputSchema"]["properties"]
        .as_object()
        .expect("run_app takes arguments");
    assert!(
        !run_app_properties.contains_key("project"),
        "run_app must not accept a client-suppliable `project` path: {run_app_properties:?}"
    );

    mcp.shutdown().await;
}

/// With more than one session running, a tool that needs one refuses to
/// guess — and an explicit `session_id` resolves it, on driving tools too.
#[tokio::test]
async fn several_sessions_force_an_explicit_choice() {
    let fixture = Fixture::spawn(FixtureConfig::default());
    let engine = Arc::new(engine_with_two_desktop_streams(
        vec![format_discovery_line(
            fixture.addr.port(),
            Some(FIXTURE_TOKEN),
        )],
        vec!["Compiling frust v0.1.0".to_string()],
    ));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;

    let connected = mcp.call_ok("run_app", json!({ "target": "desktop" })).await;
    let connected_id = connected["session"]["id"].as_u64().expect("session id");
    await_snapshot(&engine, SessionId(connected_id), |s| {
        s.state == SessionState::DevtoolsConnected
    })
    .await;
    let other = mcp
        .call_ok("run_app", json!({ "target": "desktop", "mode": "profile" }))
        .await;
    let other_id = other["session"]["id"].as_u64().expect("session id");
    await_snapshot(&engine, SessionId(other_id), |s| {
        s.state == SessionState::Running
    })
    .await;

    let ambiguous = mcp.call_err("find_widgets", json!({})).await;
    let message = ambiguous["error"].as_str().expect("a message");
    assert!(
        message.contains("2 sessions are running") && message.contains("session_id"),
        "unhelpful ambiguity error: {message}"
    );
    let ids: Vec<Value> = ambiguous["sessions"]
        .as_array()
        .expect("the sessions are listed")
        .iter()
        .map(|session| session["id"].clone())
        .collect();
    assert_eq!(ids, vec![json!(connected_id), json!(other_id)]);

    // Naming the session resolves it — including for a driving tool.
    let found = mcp
        .call_ok(
            "find_widgets",
            json!({ "label": "Save", "session_id": connected_id }),
        )
        .await;
    assert_eq!(found["session_id"], json!(connected_id));
    assert_eq!(found["total_matches"], json!(1));

    // The session without a devtools connection says so rather than
    // pretending the app is unreachable.
    let unconnected = mcp
        .call_err("find_widgets", json!({ "session_id": other_id }))
        .await;
    assert!(
        unconnected["error"]
            .as_str()
            .expect("a message")
            .contains("not connected to a devtools service"),
        "{unconnected}"
    );

    teardown(fixture, mcp).await;
}

/// With no session at all, a tool that needs one says what to do first
/// rather than failing obscurely.
#[tokio::test]
async fn tools_needing_a_session_name_the_missing_step() {
    let engine = Arc::new(SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(FakeProcessRunner::new()),
    ));
    let mcp = McpTestServer::start(Arc::clone(&engine)).await;

    for tool in ["find_widgets", "widget_tree", "performance", "app_logs"] {
        let err = mcp.call_err(tool, json!({})).await;
        let message = err["error"].as_str().expect("a message");
        assert!(
            message.contains("run_app"),
            "tool {tool} did not name the next step: {message}"
        );
    }

    let unknown = mcp
        .call_err("widget_tree", json!({ "session_id": 9_999 }))
        .await;
    assert!(
        unknown["error"]
            .as_str()
            .expect("a message")
            .contains("no such session: 9999"),
        "{unknown}"
    );

    mcp.shutdown().await;
}
