//! # The orchestration adapter
//!
//! [`OrchestrationAdapter`] is the [`DapAdapter`] the session hands everything
//! past `initialize` to: build → deploy → launch → logs-as-`output`-events →
//! stop, plus the two custom requests (`frustRestart`, `frustWidgetTree`).
//!
//! ## The backend is the host's, not the adapter's
//!
//! Every operation goes through the `frust_mcp::SessionBackend` the host
//! passed to [`crate::serve_embedded`] — the same one its own UI (and any MCP
//! client) drives. This adapter therefore *supervises* no sessions of its own:
//! it launches one into the host's world, watches it, and stops it again. The
//! project root is the host's too, and is **asked of the backend at launch
//! time** rather than remembered here: the host's open project can change
//! while this server runs, and the one thing worse than naming no directory is
//! naming a directory nothing builds from. A client's own
//! `launchArguments.projectRoot` is never honored (see [`resolve`]).
//!
//! ## One app per connection
//!
//! A DAP connection is a debug session, and a debug session is one app. A
//! second `launch` on the same connection is refused: relaunching is
//! `frustRestart`, and a second app is a second debug session.
//!
//! ## Sync backend, async adapter
//!
//! `SessionBackend` is sync by charter (`frust_mcp::backend`'s module doc) —
//! an `adb` call against an unresponsive device has no wall-clock bound. Every
//! backend call therefore goes through [`tokio::task::spawn_blocking`], the
//! same rule `frust-mcp`'s tool layer follows, with one documented exception:
//! the non-blocking state reader `session` takes a short lock and is read
//! directly.
//!
//! ## Teardown stops the app, not the world
//!
//! `disconnect`/`terminate` (and the session's own `on_disconnect` hook, which
//! fires on every exit path) run one idempotent teardown: stop the session
//! *this connection launched*, and nothing else. An editor closing its debug
//! session must not take the host's other sessions — or the host — down with
//! it, so there is no backend shutdown here; the host owns that lifetime.

mod pump;
mod resolve;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use frust_drive::build_info::BuildMode;
use frust_mcp::SharedBackend;
use frust_mcp::engine::{SessionId, SessionSnapshot, SessionState};

use self::pump::Pumps;
use self::resolve::{client_root_note, mode_name, parse_mode, resolve_target};
use crate::clients::DapClientGuard;
use crate::protocol::types::{
    Capabilities, InitializeRequestArguments, LaunchArguments, Thread, ThreadsResponseBody,
};
use crate::server::{AdapterResponse, DapAdapter, EventSender};

/// The single static thread `threads` answers with.
///
/// Orchestration-v1 does no stepping (native Rust stepping is lldb's job), but
/// a DAP client asks for threads regardless and treats an empty list as a
/// broken adapter.
const APP_THREAD_ID: i64 = 1;

/// What a request needing a launched app answers before there is one.
const NO_SESSION: &str =
    "no app has been launched on this connection; send a 'launch' request first.";

/// What a `launch` answers when the host has no open project to build in.
///
/// The host refuses to *start* a server with no project at all; this is the
/// other half — a project closed (or switched away to none) while the server
/// was already up. Refusing is the only honest answer: this adapter builds
/// from the host's project and has no root of its own to fall back on.
const NO_PROJECT: &str = "no project is open in the workbench — open one there and send 'launch' again; \
     every debug launch builds from the workbench's own project root.";

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
    session: SessionId,
    pumps: Pumps,
}

/// The [`DapAdapter`] driving the host's `frust_mcp::SessionBackend`.
pub struct OrchestrationAdapter {
    events: EventSender,
    backend: SharedBackend,
    /// This connection's entry in the host's client registry, dropped with the
    /// adapter. `None` for an adapter built outside [`crate::serve_embedded`]
    /// (a test driving one directly), which has no registry to appear in.
    client: Option<DapClientGuard>,
    launched: Option<Launched>,
    /// Whether `launch` has ever been answered successfully. Distinct from
    /// `launched.is_some()`, which teardown clears: a connection that has
    /// stopped its app has still had its one launch.
    ever_launched: bool,
    terminated: TerminatedOnce,
}

impl OrchestrationAdapter {
    /// A fresh adapter for one DAP connection, driving `backend` and building
    /// only from the project that backend reports at launch time.
    pub fn new(events: EventSender, backend: SharedBackend) -> Self {
        Self {
            events,
            backend,
            client: None,
            launched: None,
            ever_launched: false,
            terminated: TerminatedOnce::default(),
        }
    }

    /// Attaches this connection's registry entry, so dropping the adapter
    /// deregisters the client.
    pub(crate) fn with_client_guard(mut self, guard: DapClientGuard) -> Self {
        self.client = Some(guard);
        self
    }

    /// `launch` — resolve the configuration, start the session, arm the pumps.
    ///
    /// Answers as soon as the backend hands back a session id, which it does
    /// immediately: the build/install/launch chain runs elsewhere and reports
    /// through `output` events. A launch that cannot even spawn lands as
    /// `SessionState::Failed`, which the event pump surfaces as an `output`
    /// line plus `exited(1)` and `terminated`.
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

        let mode = match parse_mode(args.mode.as_deref()) {
            Ok(mode) => mode,
            Err(message) => return AdapterResponse::failure(message),
        };
        // Once, here — the directory this launch builds in, and therefore the
        // one both the ignored-root note and the banner below must name.
        let Some(project_root) = self.project_root().await else {
            return AdapterResponse::failure(NO_PROJECT);
        };
        let ignored_note = client_root_note(args.project_root.as_deref(), &project_root);
        let target = match resolve_target(&self.backend, args.device.as_deref()).await {
            Ok(target) => target,
            Err(message) => return AdapterResponse::failure(message),
        };

        // An ignored client `projectRoot` is surfaced before the banner, so
        // the security-relevant deviation reads first and the banner that
        // follows names the directory the build actually used.
        if let Some(note) = &ignored_note {
            let _ = self.events.output("console", note).await;
        }
        let _ = self
            .events
            .output(
                "console",
                &format!(
                    "Launching {} in {} mode from {}\n",
                    target.label(),
                    mode_name(mode),
                    project_root.display()
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

        let launching = Arc::clone(&self.backend);
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
            Arc::clone(&self.backend),
            session,
            self.events.clone(),
            self.terminated.clone(),
        );
        self.ever_launched = true;
        self.launched = Some(Launched { session, pumps });
        AdapterResponse::ok()
    }

    /// The directory the backend would build in right now, or `None` when it
    /// has no project open (or could not answer at all).
    ///
    /// Asked once per launch and never stored: the host's project is the
    /// host's to change, and a remembered copy is exactly how a banner comes
    /// to name a directory the build never touched.
    async fn project_root(&self) -> Option<PathBuf> {
        let backend = Arc::clone(&self.backend);
        match tokio::task::spawn_blocking(move || backend.project_root()).await {
            Ok(root) => root,
            Err(error) => {
                log::warn!("DAP: the project-root lookup failed: {error}");
                None
            }
        }
    }

    /// `frustRestart` — stop the app and launch it again, same target and mode.
    ///
    /// The session id rolls, so the pumps are re-armed against the new one.
    async fn restart(&mut self) -> AdapterResponse {
        let events = self.events.clone();
        let terminated = self.terminated.clone();
        let backend = Arc::clone(&self.backend);
        let Some(launched) = self.launched.as_mut() else {
            return AdapterResponse::failure(NO_SESSION);
        };

        let previous = launched.session;
        // The stop half of a restart makes the old session terminal; silencing
        // first is what keeps that from being reported as the app exiting.
        launched.pumps.silence();

        let restarting = Arc::clone(&backend);
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

        let pumps = Pumps::spawn(backend, session, events.clone(), terminated);
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
    ///
    /// Asked of the **backend**, not of a devtools client resolved here: the
    /// host may own that connection itself (its own UI reads the same tree
    /// through it), in which case there is no client for this adapter to
    /// resolve and a pre-check on one would refuse a request the backend can
    /// answer perfectly well. A backend that serves no trees at all refuses in
    /// the devtools layer's own vocabulary, so the classification below reads
    /// it exactly like an app that declared no `widget_tree` capability.
    async fn widget_tree(&self) -> AdapterResponse {
        let Some(launched) = self.launched.as_ref() else {
            return AdapterResponse::failure(NO_SESSION);
        };
        let session = launched.session;

        let backend = Arc::clone(&self.backend);
        match tokio::task::spawn_blocking(move || backend.fetch_widget_tree(session)).await {
            Ok(Ok(body)) => AdapterResponse::success(Some(body)),
            Ok(Err(error)) => AdapterResponse::failure(widget_tree_failure_message(
                frust_drive::devtools_client::is_not_supported(&error),
                frust_drive::devtools_client::is_unauthorized(&error),
                &format!("{error:#}"),
                session,
                // A state reader, not a device call: it takes the session lock
                // and returns, so it needs no blocking task of its own.
                self.backend.session(session).as_ref(),
            )),
            Err(error) => AdapterResponse::failure(format!("the widget_tree task failed: {error}")),
        }
    }

    /// Stops the app this connection launched. Idempotent: the second call has
    /// nothing to take and returns immediately.
    ///
    /// The backend itself is untouched — it is the host's, it outlives every
    /// DAP client, and its other sessions are none of this connection's
    /// business.
    async fn tear_down(&mut self) {
        let Some(launched) = self.launched.take() else {
            return;
        };
        let Launched { session, pumps } = launched;
        // Nothing this teardown causes is news to the client — it asked.
        pumps.stop();

        let stopping = Arc::clone(&self.backend);
        match tokio::task::spawn_blocking(move || stopping.stop_app(session)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("DAP: stopping session {session} failed: {error:#}"),
            Err(error) => log::warn!("DAP: the stop task for session {session} failed: {error}"),
        }
    }
}

impl DapAdapter for OrchestrationAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities::frust_defaults()
    }

    fn on_initialize(&mut self, client: &InitializeRequestArguments) {
        if let Some(guard) = &self.client {
            guard.identify(client);
        }
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

/// Why `frustWidgetTree` failed, phrased for a developer reading the Debug
/// Console — keeping the two rejections a developer must treat differently
/// distinguishable, and naming the session's own state for everything else.
///
/// Takes the two classifications and the rendered chain rather than the error
/// itself: `anyhow` is not a dependency of this crate — the error value
/// arrives type-inferred from the backend's API and is never named here — so
/// classification happens at the one call site that holds it.
fn widget_tree_failure_message(
    not_supported: bool,
    unauthorized: bool,
    rendered: &str,
    session: SessionId,
    snapshot: Option<&SessionSnapshot>,
) -> String {
    if not_supported {
        return format!(
            "'widget_tree' is not supported for session {session} — the app declared no matching \
             capability at handshake, or this workbench does not serve widget trees at all. \
             ({rendered})"
        );
    }
    if unauthorized {
        return "the app's devtools service rejected 'widget_tree' as unauthorized — the \
                connection's token is stale. Send 'frustRestart' to reconnect."
            .to_owned();
    }
    format!(
        "widget_tree failed for session {session}: {rendered}.{} 'frustWidgetTree' needs a debug \
         or profile build with the devtools service running; check the Debug Console for the \
         discovery line.",
        session_detail(snapshot)
    )
}

/// What the session itself has to say about why it could not answer — its
/// state, plus the launch or devtools failure it recorded, if any.
fn session_detail(snapshot: Option<&SessionSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return " The session is no longer known to this workbench.".to_owned();
    };
    let detail = match (&snapshot.state, &snapshot.devtools_error) {
        (SessionState::Failed { reason }, _) => format!(" The launch failed: {reason}."),
        (_, Some(reason)) => format!(" The app reported: {reason}."),
        _ => String::new(),
    };
    format!(" (session state: {}){detail}", state_name(&snapshot.state))
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
    use frust_drive::process::{FakeProcessRunner, ProcessRunner};
    use frust_mcp::SessionEngine;
    use frust_mcp::engine::RunTarget;
    use tokio::io::{BufReader, DuplexStream};
    use tokio::task::JoinHandle;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::protocol::codec::{CodecError, read_message, write_message};
    use crate::protocol::types::{DapEvent, DapMessage, DapRequest, DapResponse};
    use crate::server::run_session;

    /// The process runner a test backend shells out through — always a
    /// scripted [`FakeProcessRunner`] here, so nothing ever runs for real.
    type Runner = Arc<dyn ProcessRunner + Send + Sync>;

    /// The exact invocation `frust_drive::desktop_run` resolves a Debug
    /// desktop session to — the key the scripted stream is registered under, so
    /// a drift in the drive's mode→args funnel fails these tests loudly rather
    /// than silently launching nothing.
    const DEBUG_DESKTOP_INVOCATION: &str =
        "cargo run --features frust/perf-trace --features frust/devtools";

    /// A project root no test ever writes to — it only ever reaches the fake
    /// runner.
    const TEST_PROJECT_ROOT: &str = "/tmp/frust-dap-adapter-test";

    /// A second root, sharing no prefix with [`TEST_PROJECT_ROOT`]: what a
    /// backend reports when the host is on a *different* project from the one
    /// a client names.
    const OTHER_PROJECT_ROOT: &str = "/tmp/frust-dap-adapter-elsewhere";

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

    /// The reference [`frust_mcp::SessionBackend`] a test drives the adapter
    /// over: a real [`SessionEngine`] on a scripted runner, exactly as an
    /// embedding host would hand its own supervisor in.
    fn backend_with(runner: Runner) -> Arc<SessionEngine> {
        Arc::new(SessionEngine::with_runner(TEST_PROJECT_ROOT, runner))
    }

    /// A session over a duplex pair, driving a real [`OrchestrationAdapter`]
    /// against a scripted runner.
    fn spawn_adapter_session(
        runner: Runner,
    ) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>) {
        spawn_adapter_session_with_backend(backend_with(runner))
    }

    /// The same, against a caller-owned backend the test can assert on.
    fn spawn_adapter_session_with_backend(
        backend: Arc<SessionEngine>,
    ) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>) {
        let (client, _cancel, handle) = spawn_adapter_session_with_backend_and_cancel(backend);
        (client, handle)
    }

    /// [`spawn_adapter_session_with_backend`], but also handing back the
    /// session's [`CancellationToken`] for the host-shutdown path.
    fn spawn_adapter_session_with_backend_and_cancel(
        backend: Arc<SessionEngine>,
    ) -> (
        TestClient,
        CancellationToken,
        JoinHandle<std::result::Result<(), CodecError>>,
    ) {
        let cancel = CancellationToken::new();
        let (client, handle) = spawn_session_with(
            move |events| OrchestrationAdapter::new(events, Arc::clone(&backend) as SharedBackend),
            cancel.clone(),
        );
        (client, cancel, handle)
    }

    fn spawn_session_with<F>(
        make_adapter: F,
        cancel: CancellationToken,
    ) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>)
    where
        F: FnOnce(EventSender) -> OrchestrationAdapter + Send + 'static,
    {
        let (server_reader, client_writer) = tokio::io::duplex(DUPLEX_BUFFER);
        let (client_reader, server_writer) = tokio::io::duplex(DUPLEX_BUFFER);

        let handle = tokio::spawn(async move {
            run_session(server_reader, server_writer, make_adapter, cancel).await
        });

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

    /// Both the ignored-`projectRoot` note and the banner name the root the
    /// **backend** reports at launch time.
    ///
    /// The backend here is on a different project from the one the client's
    /// launch configuration names, which is the shape a host whose project
    /// changed under a running server presents: the note must compare against
    /// (and the banner must print) the directory this launch actually builds
    /// in, never the client's request and never a root remembered from when
    /// the connection opened.
    #[tokio::test]
    async fn the_ignored_root_note_and_the_banner_name_the_backends_own_root() {
        let backend = Arc::new(SessionEngine::with_runner(
            OTHER_PROJECT_ROOT,
            hanging_desktop_runner(&["running"]),
        ));
        let (mut client, handle) = spawn_adapter_session_with_backend(backend);
        client.initialize().await;

        // The configuration names TEST_PROJECT_ROOT; the backend is elsewhere.
        client.request(2, "launch", launch_arguments()).await;
        let (response, before) = client.next_response().await;
        assert!(response.success, "{:?}", response.message);

        let console: String = before.iter().filter_map(output_text).collect();
        assert!(
            console.contains("Ignoring") && console.contains(TEST_PROJECT_ROOT),
            "the client's ignored root must be echoed back: {console}"
        );
        assert!(
            console.contains(&format!("from {OTHER_PROJECT_ROOT}")),
            "the banner must name the root the backend reports: {console}"
        );

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
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

    /// The teardown promise, both halves: by the time the session has ended
    /// the launched app is *stopped* (not scheduled to stop) and the backend
    /// is **still usable**.
    ///
    /// The first half is asserted on the backend the adapter actually drove —
    /// the engine records a terminal state only after `StreamHandle::kill`,
    /// the `wait` that reaps the child, and a bounded join of the session's
    /// threads (for a device target the same teardown issues `am
    /// force-stop`/`simctl terminate` first). The second is the embed
    /// contract: the backend belongs to the host, so an editor closing its
    /// debug session must not shut the workbench's session world down with it.
    #[tokio::test]
    async fn disconnect_stops_the_app_and_leaves_the_backend_alive() {
        let backend = backend_with(hanging_desktop_runner(&["running"]));
        let (mut client, handle) = spawn_adapter_session_with_backend(Arc::clone(&backend));
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

        let sessions = backend.sessions();
        let [session] = sessions.as_slice() else {
            panic!("expected exactly one session, got {}", sessions.len());
        };
        assert!(
            session.state.is_terminal(),
            "the launched app outlived the DAP session: {:?}",
            session.state
        );

        // A shut-down engine would refuse this; the host's world survives its
        // DAP client.
        let probe = backend.run_app(RunTarget::Desktop, BuildMode::Debug);
        assert!(
            !matches!(
                backend.session(probe).map(|s| s.state),
                Some(SessionState::Failed { .. })
            ),
            "the DAP teardown shut the host's backend down: {:?}",
            backend.session(probe).map(|s| s.state)
        );
        backend.shutdown().await;
    }

    /// The host-driven equivalent of the disconnect teardown: the embedder
    /// cancels the server, which fires the session's cancellation token, and
    /// by the time the session ends the launched app is *stopped* — the same
    /// guarantee, reached without any client request, and again without taking
    /// the backend with it.
    #[tokio::test]
    async fn cancellation_stops_the_app_and_leaves_the_backend_alive() {
        let backend = backend_with(hanging_desktop_runner(&["running"]));
        let (mut client, cancel, handle) =
            spawn_adapter_session_with_backend_and_cancel(Arc::clone(&backend));
        client.initialize().await;

        client.request(2, "launch", launch_arguments()).await;
        let (response, _) = client.next_response().await;
        assert!(response.success, "{:?}", response.message);
        // Real output means the process is provably live before the cancel.
        client.wait_for_output("running").await;

        // No disconnect/terminate request — just the token the host stops the
        // embedded server with.
        cancel.cancel();
        join(handle).await;

        let sessions = backend.sessions();
        let [session] = sessions.as_slice() else {
            panic!("expected exactly one session, got {}", sessions.len());
        };
        assert!(
            session.state.is_terminal(),
            "a cancelled session left the app running: {:?}",
            session.state
        );

        let probe = backend.run_app(RunTarget::Desktop, BuildMode::Debug);
        assert!(
            !matches!(
                backend.session(probe).map(|s| s.state),
                Some(SessionState::Failed { .. })
            ),
            "a cancelled DAP server shut the host's backend down: {:?}",
            backend.session(probe).map(|s| s.state)
        );
        backend.shutdown().await;
    }

    /// A `terminate` stops the app and reports `terminated`; the `disconnect`
    /// that follows it must not report a second one.
    #[tokio::test]
    async fn terminate_then_disconnect_reports_terminated_exactly_once() {
        let backend = backend_with(hanging_desktop_runner(&["running"]));
        let (mut client, handle) = spawn_adapter_session_with_backend(Arc::clone(&backend));
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
            backend
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
            message.contains("session state: running"),
            "the refusal must name what the session was doing: {message}"
        );
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
        let backend = backend_with(hanging_desktop_runner(&["running"]));
        let (mut client, handle) = spawn_adapter_session_with_backend(Arc::clone(&backend));
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
        assert_eq!(backend.sessions().len(), 2, "the old session is retained");

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

    /// The typed refusals stay distinguishable — including a backend that
    /// serves no widget trees at all, which arrives as the devtools layer's
    /// own `NOT_SUPPORTED` and must therefore read like one.
    #[test]
    fn a_widget_tree_failure_keeps_the_two_rejections_distinguishable() {
        let unsupported =
            widget_tree_failure_message(true, false, "not supported", SessionId(7), None);
        assert!(
            unsupported.contains("no matching capability"),
            "{unsupported}"
        );
        assert!(
            unsupported.contains("does not serve widget trees"),
            "{unsupported}"
        );

        let unauthorized = widget_tree_failure_message(false, true, "ignored", SessionId(7), None);
        assert!(unauthorized.contains("frustRestart"), "{unauthorized}");

        let plain =
            widget_tree_failure_message(false, false, "connection reset", SessionId(7), None);
        assert!(plain.contains("connection reset"), "{plain}");
        assert!(plain.contains("session 7"), "{plain}");
    }

    #[test]
    fn a_widget_tree_failure_names_the_state_and_the_apps_own_reason() {
        let unknown = widget_tree_failure_message(false, false, "boom", SessionId(7), None);
        assert!(unknown.contains("no longer known"), "{unknown}");

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
        let message =
            widget_tree_failure_message(false, false, "boom", SessionId(7), Some(&snapshot));
        assert!(message.contains("session state: running"), "{message}");
        assert!(message.contains("address in use"), "{message}");

        // A failed launch reports the launch failure instead — it is the
        // reason there was nothing to read a tree from.
        snapshot.state = SessionState::Failed {
            reason: "gradle exited 1".to_string(),
        };
        let message =
            widget_tree_failure_message(false, false, "boom", SessionId(7), Some(&snapshot));
        assert!(message.contains("session state: failed"), "{message}");
        assert!(message.contains("gradle exited 1"), "{message}");
    }
}
