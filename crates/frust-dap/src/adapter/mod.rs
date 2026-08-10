//! # The orchestration adapter
//!
//! [`OrchestrationAdapter`] is the [`DapAdapter`] the session hands everything
//! past `initialize` to: build → deploy → launch → logs-as-`output`-events →
//! stop, plus the two custom requests (`frustRestart`, `frustWidgetTree`).
//!
//! ## One app per connection
//!
//! A DAP connection is a debug session, and a debug session is one app. The
//! engine is built **at launch time** (its project root is a launch argument,
//! not a server-wide setting) and a second `launch` on the same connection is
//! refused: relaunching is `frustRestart`, and a second app is a second debug
//! session.
//!
//! ## Sync engine, async adapter
//!
//! `frust_mcp::engine::SessionEngine` is sync by charter
//! (`docs/CLI_ARCHITECTURE.md`) — an `adb` call against an unresponsive device
//! has no wall-clock bound. Every engine call therefore goes through
//! [`tokio::task::spawn_blocking`], the same rule `frust-mcp`'s tool layer
//! follows, with one documented exception: the non-blocking state readers
//! (`session`, `devtools_client`) take a short lock and are read directly.
//!
//! ## Teardown
//!
//! `disconnect`/`terminate` (and the session's own `on_disconnect` hook, which
//! fires on every exit path) run one idempotent teardown: stop the session,
//! then `SessionEngine::shutdown`, which joins every detached teardown before
//! returning. That join is the point — a stop that returns while the app is
//! still being killed is not a stop.

mod pump;
mod resolve;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use frust_drive::build_info::BuildMode;
use frust_drive::process::ProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{SessionId, SessionSnapshot, SessionState};

use self::pump::Pumps;
use self::resolve::{mode_name, parse_mode, resolve_project_root, resolve_target};
use crate::protocol::types::{Capabilities, LaunchArguments, Thread, ThreadsResponseBody};
use crate::server::{AdapterResponse, DapAdapter, EventSender};

/// The process runner every session shells out through — the seam a test
/// injects `frust_drive::process::FakeProcessRunner` at, and the one
/// `frust-cli` builds exactly once.
pub type Runner = Arc<dyn ProcessRunner + Send + Sync>;

/// The single static thread `threads` answers with.
///
/// Orchestration-v1 does no stepping (native Rust stepping is lldb's job), but
/// a DAP client asks for threads regardless and treats an empty list as a
/// broken adapter.
const APP_THREAD_ID: i64 = 1;

/// What a request needing a launched app answers before there is one.
const NO_SESSION: &str =
    "no app has been launched on this connection; send a 'launch' request first.";

/// A `terminated` event that is sent at most once per connection.
///
/// Three paths can reach it — the app exiting on its own, a `terminate`
/// request, and the connection's teardown hook — and a client that sees two
/// `terminated` events for one session treats the second as a protocol fault.
#[derive(Debug, Clone, Default)]
pub(crate) struct TerminatedOnce(Arc<AtomicBool>);

impl TerminatedOnce {
    /// Emits `terminated` unless it has already been emitted.
    pub(crate) async fn emit(&self, events: &EventSender) {
        if self
            .0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = events.terminated().await;
        }
    }
}

/// The app this connection launched, and the tasks watching it.
struct Launched {
    engine: Arc<SessionEngine>,
    session: SessionId,
    pumps: Pumps,
}

/// The [`DapAdapter`] driving a `frust_mcp::engine::SessionEngine`.
pub struct OrchestrationAdapter {
    events: EventSender,
    runner: Runner,
    /// A caller-supplied engine, for tests that must observe the engine the
    /// adapter drives. `None` in production: the engine is built at launch
    /// time, from the launch request's own project root.
    injected: Option<Arc<SessionEngine>>,
    launched: Option<Launched>,
    /// Whether `launch` has ever been answered successfully. Distinct from
    /// `launched.is_some()`, which teardown clears: a connection that has
    /// stopped its app has still had its one launch.
    ever_launched: bool,
    terminated: TerminatedOnce,
}

impl OrchestrationAdapter {
    /// A fresh adapter for one DAP connection.
    ///
    /// No engine is built here: the project root it would need arrives with
    /// the `launch` request.
    pub fn new(events: EventSender, runner: Runner) -> Self {
        Self {
            events,
            runner,
            injected: None,
            launched: None,
            ever_launched: false,
            terminated: TerminatedOnce::default(),
        }
    }

    /// Test seam: an adapter that launches into `engine` rather than building
    /// one per launch request.
    ///
    /// The caller keeps its `Arc`, which is what lets a test assert on the
    /// engine the adapter actually drove (that the app was stopped, that the
    /// engine was shut down). A `launch` request's `projectRoot` is ignored on
    /// this path — the engine already has one, and it is not overridable by
    /// design.
    #[doc(hidden)]
    pub fn with_engine(events: EventSender, engine: Arc<SessionEngine>) -> Self {
        Self {
            events,
            runner: engine.runner(),
            injected: Some(engine),
            launched: None,
            ever_launched: false,
            terminated: TerminatedOnce::default(),
        }
    }

    /// `launch` — resolve the configuration, start the session, arm the pumps.
    ///
    /// Answers as soon as the engine hands back a session id, which it does
    /// immediately: the build/install/launch chain runs on the session's own
    /// thread and reports through `output` events. A launch that cannot even
    /// spawn lands as `SessionState::Failed`, which the exit watch surfaces as
    /// an `output` line plus `exited(1)` and `terminated`.
    async fn launch(&mut self, arguments: Option<serde_json::Value>) -> AdapterResponse {
        if self.ever_launched {
            return AdapterResponse::failure(
                "this connection has already launched an app; frust-dap supervises one app per \
                 debug session. Use 'frustRestart' to relaunch it, or start a second debug \
                 session for a second app.",
            );
        }

        let args = match arguments {
            Some(value) => match serde_json::from_value::<LaunchArguments>(value) {
                Ok(args) => args,
                Err(error) => {
                    return AdapterResponse::failure(format!(
                        "could not read the launch configuration: {error}"
                    ));
                }
            },
            None => LaunchArguments::default(),
        };

        let project_root = match resolve_project_root(args.project_root.as_deref()) {
            Ok(root) => root,
            Err(message) => return AdapterResponse::failure(message),
        };
        let mode = match parse_mode(args.mode.as_deref()) {
            Ok(mode) => mode,
            Err(message) => return AdapterResponse::failure(message),
        };

        let engine = match &self.injected {
            Some(engine) => Arc::clone(engine),
            None => Arc::new(SessionEngine::with_runner(
                project_root.clone(),
                Arc::clone(&self.runner),
            )),
        };
        let target = match resolve_target(&engine, args.device.as_deref()).await {
            Ok(target) => target,
            Err(message) => return AdapterResponse::failure(message),
        };

        let _ = self
            .events
            .output(
                "console",
                &format!(
                    "Launching {} in {} mode from {}\n",
                    target.label(),
                    mode_name(mode),
                    // The engine's own root, not the requested one: they are
                    // the same everywhere but the test seam, and a banner that
                    // names a directory the build did not use is worse than no
                    // banner at all.
                    engine.project_root().display()
                ),
            )
            .await;
        if mode == BuildMode::Release {
            // Not a refusal — a release build is a legitimate thing to deploy
            // — but the one custom request that needs the in-app service will
            // fail on it, and saying so now beats saying so later.
            let _ = self
                .events
                .output(
                    "console",
                    "Note: a release build compiles the devtools service out, so \
                     'frustWidgetTree' will not work for this session.\n",
                )
                .await;
        }

        let launching = Arc::clone(&engine);
        let launch_target = target.clone();
        let session =
            match tokio::task::spawn_blocking(move || launching.run_app(launch_target, mode)).await
            {
                Ok(session) => session,
                Err(error) => {
                    return AdapterResponse::failure(format!("the launch task failed: {error}"));
                }
            };

        let pumps = Pumps::spawn(
            Arc::clone(&engine),
            session,
            self.events.clone(),
            self.terminated.clone(),
        );
        self.ever_launched = true;
        self.launched = Some(Launched {
            engine,
            session,
            pumps,
        });
        AdapterResponse::ok()
    }

    /// `frustRestart` — stop the app and launch it again, same target and mode.
    ///
    /// The session id rolls, so the pumps are re-armed against the new one.
    async fn restart(&mut self) -> AdapterResponse {
        let events = self.events.clone();
        let terminated = self.terminated.clone();
        let Some(launched) = self.launched.as_mut() else {
            return AdapterResponse::failure(NO_SESSION);
        };

        let previous = launched.session;
        // The stop half of a restart makes the old session terminal; silencing
        // first is what keeps that from being reported as the app exiting.
        launched.pumps.silence();

        let restarting = Arc::clone(&launched.engine);
        let session =
            match tokio::task::spawn_blocking(move || restarting.restart_app(previous)).await {
                Ok(Ok(session)) => session,
                Ok(Err(error)) => {
                    return AdapterResponse::failure(format!(
                        "restarting session {previous} failed: {error:#}"
                    ));
                }
                Err(error) => {
                    return AdapterResponse::failure(format!(
                        "the restart task for session {previous} failed: {error}"
                    ));
                }
            };

        let pumps = Pumps::spawn(
            Arc::clone(&launched.engine),
            session,
            events.clone(),
            terminated,
        );
        let stale = std::mem::replace(&mut launched.pumps, pumps);
        stale.stop();
        launched.session = session;

        let _ = events
            .output(
                "console",
                &format!("Restarted the app (session {previous} → {session})\n"),
            )
            .await;
        AdapterResponse::success(Some(serde_json::json!({ "sessionId": session.0 })))
    }

    /// `frustWidgetTree` — one widget-tree dump from the app's devtools
    /// service, as the response body.
    async fn widget_tree(&self) -> AdapterResponse {
        let Some(launched) = self.launched.as_ref() else {
            return AdapterResponse::failure(NO_SESSION);
        };
        let session = launched.session;

        // A state reader, not a device call: it takes the session lock and
        // returns, so it needs no blocking task of its own.
        let Some(client) = launched.engine.devtools_client(session) else {
            return AdapterResponse::failure(no_devtools_message(
                session,
                launched.engine.session(session).as_ref(),
            ));
        };

        match tokio::task::spawn_blocking(move || client.widget_tree()).await {
            Ok(Ok(dump)) => match serde_json::to_value(&dump) {
                Ok(body) => AdapterResponse::success(Some(body)),
                Err(error) => AdapterResponse::failure(format!(
                    "the app's widget tree could not be serialized: {error}"
                )),
            },
            Ok(Err(error)) => AdapterResponse::failure(devtools_failure_message(
                frust_drive::devtools_client::is_not_supported(&error),
                frust_drive::devtools_client::is_unauthorized(&error),
                &format!("{error:#}"),
            )),
            Err(error) => AdapterResponse::failure(format!("the widget_tree task failed: {error}")),
        }
    }

    /// Stops the app and shuts the engine down. Idempotent: the second call
    /// has nothing to take and returns immediately.
    async fn tear_down(&mut self) {
        let Some(launched) = self.launched.take() else {
            return;
        };
        let Launched {
            engine,
            session,
            pumps,
        } = launched;
        // Nothing this teardown causes is news to the client — it asked.
        pumps.stop();

        let stopping = Arc::clone(&engine);
        match tokio::task::spawn_blocking(move || stopping.stop_app(session)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("DAP: stopping session {session} failed: {error:#}"),
            Err(error) => log::warn!("DAP: the stop task for session {session} failed: {error}"),
        }
        // `shutdown` joins every detached teardown before returning, which is
        // what makes "the app is stopped" true rather than merely requested.
        engine.shutdown().await;
    }
}

impl DapAdapter for OrchestrationAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities::frust_defaults()
    }

    async fn handle_request(
        &mut self,
        command: &str,
        arguments: Option<serde_json::Value>,
    ) -> AdapterResponse {
        match command {
            "launch" => self.launch(arguments).await,
            "configurationDone" => AdapterResponse::ok(),
            "threads" => threads_response(),
            "terminate" => {
                self.tear_down().await;
                // Sent before this request's own response, unavoidably: an
                // adapter cannot queue anything *after* the response the
                // session writes for it. Clients key on the event's arrival,
                // not on its position relative to the ack.
                self.terminated.emit(&self.events).await;
                AdapterResponse::ok()
            }
            // The session emits nothing of its own here and runs
            // `on_disconnect` after writing this response, which is what puts
            // the `terminated` event *after* the ack on this path.
            "disconnect" => {
                self.tear_down().await;
                AdapterResponse::ok()
            }
            "frustRestart" => self.restart().await,
            "frustWidgetTree" => self.widget_tree().await,
            other => AdapterResponse::unsupported(other),
        }
    }

    async fn on_disconnect(&mut self) {
        self.tear_down().await;
        self.terminated.emit(&self.events).await;
    }
}

/// The `threads` response: one static thread named `app`.
fn threads_response() -> AdapterResponse {
    let body = ThreadsResponseBody {
        threads: vec![Thread {
            id: APP_THREAD_ID,
            name: "app".to_owned(),
        }],
    };
    match serde_json::to_value(body) {
        Ok(body) => AdapterResponse::success(Some(body)),
        Err(error) => AdapterResponse::failure(format!("could not build the thread list: {error}")),
    }
}

/// Why `frustWidgetTree` cannot run, phrased for a developer reading the
/// Debug Console — the same shape `frust-mcp`'s tool layer reports, since it
/// is the same gap with the same fixes.
fn no_devtools_message(session: SessionId, snapshot: Option<&SessionSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return format!("session {session} is no longer known to this adapter.");
    };
    let detail = match (&snapshot.state, &snapshot.devtools_error) {
        (SessionState::Failed { reason }, _) => format!(" The launch failed: {reason}"),
        (_, Some(reason)) => format!(" The app reported: {reason}"),
        _ => String::new(),
    };
    format!(
        "session {session} is not connected to a devtools service (state: {}).{detail} \
         'frustWidgetTree' needs a debug or profile build with the devtools service running; \
         check the Debug Console for the discovery line.",
        state_name(&snapshot.state)
    )
}

/// A devtools request failure, keeping the two rejections a developer must
/// treat differently distinguishable.
///
/// Takes the two classifications and the rendered chain rather than the error
/// itself: `anyhow` is not a dependency of this crate — the error value
/// arrives type-inferred from `frust-drive`'s API and is never named here — so
/// classification happens at the one call site that holds it.
fn devtools_failure_message(not_supported: bool, unauthorized: bool, rendered: &str) -> String {
    if not_supported {
        return "the app's devtools service does not support 'widget_tree' — it declared no \
                matching capability at handshake."
            .to_owned();
    }
    if unauthorized {
        return "the app's devtools service rejected 'widget_tree' as unauthorized — the \
                connection's token is stale. Send 'frustRestart' to reconnect."
            .to_owned();
    }
    format!("widget_tree failed: {rendered}")
}

/// The short name a [`SessionState`] reports as, matching what `frust-mcp`
/// shows an agent for the same session.
fn state_name(state: &SessionState) -> &'static str {
    match state {
        SessionState::Launching => "launching",
        SessionState::Running => "running",
        SessionState::DevtoolsConnected => "devtools_connected",
        SessionState::Exited { .. } => "exited",
        SessionState::Failed { .. } => "failed",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use frust_drive::build_info::BuildMode;
    use frust_drive::process::FakeProcessRunner;
    use frust_mcp::engine::RunTarget;
    use tokio::io::{BufReader, DuplexStream};
    use tokio::task::JoinHandle;

    use super::*;
    use crate::protocol::codec::{CodecError, read_message, write_message};
    use crate::protocol::types::{DapEvent, DapMessage, DapRequest, DapResponse};
    use crate::server::run_session;

    /// The exact invocation `frust_drive::desktop_run` resolves a Debug
    /// desktop session to — the key the scripted stream is registered under, so
    /// a drift in the drive's mode→args funnel fails these tests loudly rather
    /// than silently launching nothing.
    const DEBUG_DESKTOP_INVOCATION: &str =
        "cargo run --features frust/perf-trace --features frust/devtools";

    /// A project root no test ever writes to — it only ever reaches the fake
    /// runner.
    const TEST_PROJECT_ROOT: &str = "/tmp/frust-dap-adapter-test";

    /// A failure deadline, never a pacing device.
    const DEADLINE: Duration = Duration::from_secs(10);

    const DUPLEX_BUFFER: usize = 8192;

    /// A scripted DAP client over the far end of a duplex pair.
    struct TestClient {
        reader: BufReader<DuplexStream>,
        writer: DuplexStream,
    }

    impl TestClient {
        async fn request(&mut self, seq: i64, command: &str, arguments: Option<serde_json::Value>) {
            write_message(
                &mut self.writer,
                &DapMessage::Request(DapRequest {
                    seq,
                    command: command.to_owned(),
                    arguments,
                }),
            )
            .await
            .expect("client write");
        }

        async fn next(&mut self) -> DapMessage {
            tokio::time::timeout(DEADLINE, read_message(&mut self.reader))
                .await
                .expect("timed out waiting for a DAP message")
                .expect("client read")
                .expect("expected a message, got EOF")
        }

        /// The next response, collecting (and returning) every event that
        /// arrives ahead of it — an adapter is free to push events before its
        /// own answer, and the tests here care about both.
        async fn next_response(&mut self) -> (DapResponse, Vec<DapEvent>) {
            let mut events = Vec::new();
            loop {
                match self.next().await {
                    DapMessage::Response(response) => return (response, events),
                    DapMessage::Event(event) => events.push(event),
                    other => panic!("unexpected message from the server: {other:?}"),
                }
            }
        }

        /// Reads events until one satisfies `predicate`, returning everything
        /// seen on the way; fails on the deadline rather than hanging.
        async fn events_until(&mut self, predicate: impl Fn(&DapEvent) -> bool) -> Vec<DapEvent> {
            let mut seen = Vec::new();
            loop {
                match self.next().await {
                    DapMessage::Event(event) => {
                        let done = predicate(&event);
                        seen.push(event);
                        if done {
                            return seen;
                        }
                    }
                    other => panic!("expected an event, got {other:?}"),
                }
            }
        }

        /// Reads events until one carries `needle` in its `output` body.
        async fn wait_for_output(&mut self, needle: &str) -> Vec<DapEvent> {
            let needle = needle.to_owned();
            self.events_until(move |event| output_text(event).is_some_and(|t| t.contains(&needle)))
                .await
        }

        async fn initialize(&mut self) {
            self.request(1, "initialize", None).await;
            let (response, _) = self.next_response().await;
            assert!(response.success, "initialize must succeed");
            match self.next().await {
                DapMessage::Event(event) => assert_eq!(event.event, "initialized"),
                other => panic!("expected the initialized event, got {other:?}"),
            }
        }
    }

    /// The `output` body's text, for an event that carries one.
    fn output_text(event: &DapEvent) -> Option<&str> {
        (event.event == "output")
            .then(|| event.body.as_ref()?["output"].as_str())
            .flatten()
    }

    /// A session over a duplex pair, driving a real [`OrchestrationAdapter`]
    /// against a scripted runner.
    fn spawn_adapter_session(
        runner: Runner,
    ) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>) {
        spawn_session_with(move |events| OrchestrationAdapter::new(events, Arc::clone(&runner)))
    }

    /// The same, against a caller-owned engine the test can assert on.
    fn spawn_adapter_session_with_engine(
        engine: Arc<SessionEngine>,
    ) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>) {
        spawn_session_with(move |events| {
            OrchestrationAdapter::with_engine(events, Arc::clone(&engine))
        })
    }

    fn spawn_session_with<F>(
        make_adapter: F,
    ) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>)
    where
        F: FnOnce(EventSender) -> OrchestrationAdapter + Send + 'static,
    {
        let (server_reader, client_writer) = tokio::io::duplex(DUPLEX_BUFFER);
        let (client_reader, server_writer) = tokio::io::duplex(DUPLEX_BUFFER);

        let handle =
            tokio::spawn(
                async move { run_session(server_reader, server_writer, make_adapter).await },
            );

        (
            TestClient {
                reader: BufReader::new(client_reader),
                writer: client_writer,
            },
            handle,
        )
    }

    /// A runner whose desktop launch replays `lines` and then hangs — a live
    /// app that only ends when something kills it, like a real preview.
    fn hanging_desktop_runner(lines: &[&str]) -> Runner {
        Arc::new(
            FakeProcessRunner::new().with_hanging_stream(DEBUG_DESKTOP_INVOCATION, lines.to_vec()),
        )
    }

    /// A runner whose desktop launch replays `lines` and then exits — an app
    /// that ends on its own.
    fn exiting_desktop_runner(lines: &[&str], success: bool) -> Runner {
        Arc::new(FakeProcessRunner::new().with_stream(
            DEBUG_DESKTOP_INVOCATION,
            lines.to_vec(),
            success,
        ))
    }

    fn launch_arguments() -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "projectRoot": TEST_PROJECT_ROOT,
            "device": "desktop",
            "mode": "debug",
        }))
    }

    async fn join(handle: JoinHandle<std::result::Result<(), CodecError>>) {
        tokio::time::timeout(DEADLINE, handle)
            .await
            .expect("the session did not end")
            .expect("the session task panicked")
            .expect("session I/O");
    }

    // ── Launch ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_launch_streams_the_apps_output_as_stdout_events() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&[
            "hello from the app",
            "still running",
        ]));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (response, before) = client.next_response().await;
        assert!(response.success, "launch failed: {:?}", response.message);
        assert_eq!(response.command, "launch");
        assert!(
            before
                .iter()
                .any(|event| output_text(event).is_some_and(|t| t.contains("Launching desktop"))),
            "the launch banner must name the resolved target: {before:?}"
        );

        let events = client.wait_for_output("hello from the app").await;
        let app_line = events
            .iter()
            .find(|event| output_text(event).is_some_and(|t| t.contains("hello from the app")))
            .expect("the app's line arrived");
        let body = app_line.body.as_ref().expect("output body");
        assert_eq!(body["category"], "stdout", "app output is stdout-category");
        assert_eq!(
            body["output"], "hello from the app\n",
            "an engine log line carries no terminator of its own"
        );

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    /// An app that ends by itself is reported as `exited` (0/1, never a real
    /// code — the process seam does not expose one) and then `terminated`.
    #[tokio::test]
    async fn an_app_that_exits_on_its_own_reports_exited_then_terminated() {
        let (mut client, handle) =
            spawn_adapter_session(exiting_desktop_runner(&["work", "done"], true));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (response, _) = client.next_response().await;
        assert!(response.success, "{:?}", response.message);

        let events = client
            .events_until(|event| event.event == "terminated")
            .await;
        let exited = events
            .iter()
            .find(|event| event.event == "exited")
            .expect("an exited event precedes terminated");
        assert_eq!(exited.body.as_ref().expect("exited body")["exitCode"], 0);
        assert!(
            events
                .iter()
                .any(|event| output_text(event).is_some_and(|t| t.contains("done"))),
            "the app's last output must arrive before the exit events: {events:?}"
        );

        client.request(3, "disconnect", None).await;
        let (disconnect, more) = client.next_response().await;
        assert!(disconnect.success);
        assert!(
            !more.iter().any(|event| event.event == "terminated"),
            "terminated is reported once per connection: {more:?}"
        );
        join(handle).await;
    }

    #[tokio::test]
    async fn a_second_launch_on_the_same_connection_is_refused() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&["running"]));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (first, _) = client.next_response().await;
        assert!(first.success, "{:?}", first.message);

        client.request(3, "launch", launch_arguments()).await;
        let (second, _) = client.next_response().await;
        assert!(!second.success, "one app per connection");
        assert!(
            second
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("frustRestart"),
            "the refusal must name the way to relaunch: {:?}",
            second.message
        );

        client.request(4, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn an_unreadable_launch_configuration_is_refused_in_band() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&["running"]));
        client.initialize().await;

        client
            .request(2, "launch", Some(serde_json::json!({"mode": 7})))
            .await;
        let (response, _) = client.next_response().await;
        assert!(!response.success, "a non-string mode is not a mode");
        assert!(
            response
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("launch configuration"),
            "{:?}",
            response.message
        );

        // Refused, not consumed: the one launch this connection gets is still
        // available.
        client.request(3, "launch", launch_arguments()).await;
        let (retry, _) = client.next_response().await;
        assert!(retry.success, "{:?}", retry.message);

        client.request(4, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    // ── Teardown ────────────────────────────────────────────────────────────

    /// The teardown promise: by the time the session has ended, the launched
    /// app is *stopped*, not scheduled to stop.
    ///
    /// Both halves are asserted on the engine the adapter actually drove: the
    /// session is terminal (the engine records that only after
    /// `StreamHandle::kill`, the `wait` that reaps the child, and a bounded
    /// join of the session's threads — for a device target the same teardown
    /// issues `am force-stop`/`simctl terminate` first), and the engine itself
    /// is closed, which only `SessionEngine::shutdown` does.
    #[tokio::test]
    async fn disconnect_stops_the_app_and_shuts_the_engine_down() {
        let engine = Arc::new(SessionEngine::with_runner(
            TEST_PROJECT_ROOT,
            hanging_desktop_runner(&["running"]),
        ));
        let (mut client, handle) = spawn_adapter_session_with_engine(Arc::clone(&engine));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (response, _) = client.next_response().await;
        assert!(response.success, "{:?}", response.message);
        // Real output means the process is provably live before the stop.
        client.wait_for_output("running").await;

        client.request(3, "disconnect", None).await;
        let (disconnect, _) = client.next_response().await;
        assert!(disconnect.success);
        join(handle).await;

        let sessions = engine.sessions();
        let [session] = sessions.as_slice() else {
            panic!("expected exactly one session, got {}", sessions.len());
        };
        assert!(
            session.state.is_terminal(),
            "the launched app outlived the DAP session: {:?}",
            session.state
        );

        // A shut-down engine refuses new launches — the difference between
        // "the app was stopped" and "the engine was also torn down".
        let probe = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
        assert!(
            matches!(
                engine.session(probe).map(|s| s.state),
                Some(SessionState::Failed { .. })
            ),
            "the engine was not shut down: {:?}",
            engine.session(probe).map(|s| s.state)
        );
    }

    /// A `terminate` stops the app and reports `terminated`; the `disconnect`
    /// that follows it must not report a second one.
    #[tokio::test]
    async fn terminate_then_disconnect_reports_terminated_exactly_once() {
        let engine = Arc::new(SessionEngine::with_runner(
            TEST_PROJECT_ROOT,
            hanging_desktop_runner(&["running"]),
        ));
        let (mut client, handle) = spawn_adapter_session_with_engine(Arc::clone(&engine));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (launch, _) = client.next_response().await;
        assert!(launch.success, "{:?}", launch.message);
        client.wait_for_output("running").await;

        client.request(3, "terminate", None).await;
        let (terminate, events) = client.next_response().await;
        assert!(terminate.success);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "terminated")
                .count(),
            1,
            "terminate reports that the app ended: {events:?}"
        );
        assert!(
            engine
                .sessions()
                .iter()
                .all(|session| session.state.is_terminal()),
            "terminate must stop the app, not just answer"
        );

        client.request(4, "disconnect", None).await;
        let (disconnect, more) = client.next_response().await;
        assert!(disconnect.success, "a disconnect after terminate is fine");
        assert!(
            !more.iter().any(|event| event.event == "terminated"),
            "a second terminated event: {more:?}"
        );
        join(handle).await;
    }

    // ── Custom requests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn frust_widget_tree_before_devtools_connects_fails_in_band() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&["running"]));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (launch, _) = client.next_response().await;
        assert!(launch.success, "{:?}", launch.message);
        client.wait_for_output("running").await;

        client.request(3, "frustWidgetTree", None).await;
        let (response, _) = client.next_response().await;
        assert!(!response.success, "no devtools service is connected");
        let message = response.message.unwrap_or_default();
        assert!(message.contains("devtools"), "unhelpful: {message}");
        assert!(
            message.contains("debug or profile build"),
            "the message must say what would fix it: {message}"
        );

        // In band: the connection is still usable afterwards.
        client.request(4, "threads", None).await;
        let (threads, _) = client.next_response().await;
        assert!(threads.success);

        client.request(5, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn a_request_needing_a_session_before_launch_says_so() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&[]));
        client.initialize().await;

        for (seq, command) in [(2, "frustRestart"), (3, "frustWidgetTree")] {
            client.request(seq, command, None).await;
            let (response, _) = client.next_response().await;
            assert!(!response.success, "{command} needs a launched app");
            assert!(
                response
                    .message
                    .as_deref()
                    .unwrap_or_default()
                    .contains("'launch'"),
                "{command} must name the missing step: {:?}",
                response.message
            );
        }

        client.request(4, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    /// A restart rolls the session id, re-arms the pumps against the new
    /// session, and marks the boundary in the Debug Console — without
    /// reporting the stop half as the app exiting.
    #[tokio::test]
    async fn frust_restart_rolls_the_session_and_keeps_streaming() {
        let engine = Arc::new(SessionEngine::with_runner(
            TEST_PROJECT_ROOT,
            hanging_desktop_runner(&["running"]),
        ));
        let (mut client, handle) = spawn_adapter_session_with_engine(Arc::clone(&engine));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (launch, _) = client.next_response().await;
        assert!(launch.success, "{:?}", launch.message);
        client.wait_for_output("running").await;

        client.request(3, "frustRestart", None).await;
        let (restart, _) = client.next_response().await;
        assert!(restart.success, "{:?}", restart.message);
        let session = restart.body.expect("restart body")["sessionId"]
            .as_u64()
            .expect("the new session id");
        assert_eq!(session, 2, "the replacement session is a new one");

        // The replacement session streams too — and no exit was reported for
        // the session the restart itself stopped.
        let events = client.wait_for_output("running").await;
        assert!(
            !events
                .iter()
                .any(|event| event.event == "exited" || event.event == "terminated"),
            "a restart's own stop must not be reported as the app exiting: {events:?}"
        );
        assert_eq!(engine.sessions().len(), 2, "the old session is retained");

        client.request(4, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    // ── Lifecycle acks ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn threads_answers_one_static_app_thread() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&[]));
        client.initialize().await;

        client.request(2, "threads", None).await;
        let (response, _) = client.next_response().await;
        assert!(response.success);
        let body = response.body.expect("threads body");
        assert_eq!(body["threads"][0]["id"], APP_THREAD_ID);
        assert_eq!(body["threads"][0]["name"], "app");
        assert_eq!(body["threads"].as_array().map(Vec::len), Some(1));

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn configuration_done_is_acknowledged_and_unknown_customs_are_not() {
        let (mut client, handle) = spawn_adapter_session(hanging_desktop_runner(&[]));
        client.initialize().await;

        client.request(2, "configurationDone", None).await;
        let (ack, _) = client.next_response().await;
        assert!(ack.success);
        assert!(ack.body.is_none(), "configurationDone acks with no body");

        client.request(3, "frustWarpDrive", None).await;
        let (unknown, _) = client.next_response().await;
        assert!(!unknown.success);
        assert!(
            unknown
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("frustWarpDrive"),
            "the refusal must name the command: {:?}",
            unknown.message
        );

        client.request(4, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    // ── Message shaping ─────────────────────────────────────────────────────

    #[test]
    fn a_devtools_failure_keeps_the_two_rejections_distinguishable() {
        assert!(devtools_failure_message(true, false, "ignored").contains("declared no matching"));
        assert!(devtools_failure_message(false, true, "ignored").contains("frustRestart"));
        let plain = devtools_failure_message(false, false, "connection reset");
        assert!(plain.contains("connection reset"), "{plain}");
    }

    #[test]
    fn the_no_devtools_message_names_the_state_and_the_apps_own_reason() {
        let message = no_devtools_message(SessionId(7), None);
        assert!(message.contains("session 7"), "{message}");

        let mut snapshot = SessionSnapshot {
            id: SessionId(7),
            target: RunTarget::Desktop,
            mode: BuildMode::Debug,
            project_root: TEST_PROJECT_ROOT.into(),
            state: SessionState::Running,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            pid: None,
            android_package: None,
            ios_bundle_id: None,
            devtools_port: None,
            devtools_handshake: None,
            devtools_error: Some("service did not start: address in use".to_string()),
            log_lines: 0,
            dropped_log_lines: 0,
            frames: 0,
            dropped_frames: 0,
            metrics_sampling: false,
        };
        let message = no_devtools_message(SessionId(7), Some(&snapshot));
        assert!(message.contains("state: running"), "{message}");
        assert!(message.contains("address in use"), "{message}");

        // A failed launch reports the launch failure instead — it is the
        // reason there is nothing to connect to.
        snapshot.state = SessionState::Failed {
            reason: "gradle exited 1".to_string(),
        };
        let message = no_devtools_message(SessionId(7), Some(&snapshot));
        assert!(message.contains("state: failed"), "{message}");
        assert!(message.contains("gradle exited 1"), "{message}");
    }
}
