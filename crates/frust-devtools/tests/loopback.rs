//! End-to-end tests over a real loopback socket: a `FakeBackend` behind the
//! real [`Service`], driven by a plain blocking `std::net::TcpStream` client.
//!
//! The client is deliberately synchronous and tokio-free — it is the closest
//! stand-in for the tooling side, which depends on the protocol crate alone.
//!
//! **No sleeps.** Every wait is on something the server itself must produce: a
//! response line, a notification line, EOF, or a thread join. The one
//! exception is the backend-timeout test, which necessarily waits out a
//! (short, configured) timeout — and even there the assertion is on the
//! response that arrives, not on elapsed time.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::Duration;

use frust_devtools::{
    AppInfo, BackendError, DevtoolsBackend, Service, ServiceConfig, ServiceHandle,
};
use frust_devtools_protocol::{
    AckResult, Capability, DISCOVERY_PREFIX, FrameStats, HandshakeInfo, Incoming,
    InputScrollParams, InputTapParams, InputTextParams, Method, Notification, PROTOCOL_VERSION,
    RectPx, Request, Response, ResponseOutcome, RpcError, WidgetNode, WidgetProps,
    WidgetPropsParams, WidgetTreeDump, decode_line, encode_line, parse_discovery_line, serde_json,
    serde_json::Value,
};

// ───────────────────────────── fake backend ──────────────────────────────

#[derive(Debug, Default, PartialEq)]
struct Recorded {
    taps: Vec<(f64, f64)>,
    scrolls: Vec<(f64, f64, f64, f64)>,
    texts: Vec<String>,
}

/// A one-shot release gate: `wait` parks until some other thread `release`s.
/// Used to hold the backend thread inside a call, the way a frozen UI thread
/// would, without any sleeping.
#[derive(Default)]
struct Gate {
    released: Mutex<bool>,
    signal: Condvar,
}

impl Gate {
    fn wait(&self) {
        let mut released = self.released.lock().expect("gate mutex");
        while !*released {
            released = self.signal.wait(released).expect("gate wait");
        }
    }

    fn release(&self) {
        *self.released.lock().expect("gate mutex") = true;
        self.signal.notify_all();
    }
}

struct FakeBackend {
    recorded: Arc<Mutex<Recorded>>,
    /// When set, `widget_tree` parks on it — a stand-in for a wedged UI thread.
    freeze: Option<Arc<Gate>>,
    screenshot_supported: bool,
    /// When true, `inject_text` reports a backend-level rejection.
    reject_text: bool,
}

impl FakeBackend {
    fn new(recorded: Arc<Mutex<Recorded>>) -> Self {
        Self {
            recorded,
            freeze: None,
            screenshot_supported: false,
            reject_text: false,
        }
    }
}

impl DevtoolsBackend for FakeBackend {
    fn handshake_info(&self, app: &AppInfo) -> HandshakeInfo {
        let mut capabilities = vec![
            Capability::WidgetTree,
            Capability::FrameStats,
            Capability::Input,
            Capability::Metrics,
        ];
        if self.screenshot_supported {
            capabilities.push(Capability::Screenshot);
        }
        HandshakeInfo {
            app_name: app.app_name.clone(),
            frust_version: app.frust_version.clone(),
            protocol_version: PROTOCOL_VERSION,
            capabilities,
        }
    }

    fn widget_tree(&self) -> WidgetTreeDump {
        if let Some(gate) = &self.freeze {
            gate.wait();
        }
        WidgetTreeDump {
            roots: vec![WidgetNode {
                id: 1,
                type_name: "RootWidget".to_string(),
                debug_label: None,
                bounds: Some(RectPx {
                    x: 0.0,
                    y: 0.0,
                    width: 360.0,
                    height: 800.0,
                }),
                children: vec![WidgetNode {
                    id: 2,
                    type_name: "TextWidget".to_string(),
                    debug_label: Some("greeting".to_string()),
                    bounds: None,
                    children: Vec::new(),
                }],
            }],
        }
    }

    fn widget_props(&self, id: u64) -> Option<WidgetProps> {
        (id == 2).then(|| WidgetProps {
            id,
            entries: vec![("text".to_string(), "hello".to_string())],
        })
    }

    fn metrics_snapshot(&self) -> frust_devtools_protocol::MetricsSnapshot {
        frust_devtools_protocol::MetricsSnapshot {
            rss_bytes: Some(4_096),
            uptime_ms: 1_234,
        }
    }

    fn inject_tap(&self, params: InputTapParams) -> Result<(), BackendError> {
        self.recorded
            .lock()
            .expect("recorded mutex")
            .taps
            .push((params.x, params.y));
        Ok(())
    }

    fn inject_scroll(&self, params: InputScrollParams) -> Result<(), BackendError> {
        self.recorded
            .lock()
            .expect("recorded mutex")
            .scrolls
            .push((params.x, params.y, params.dx, params.dy));
        Ok(())
    }

    fn inject_text(&self, text: &str) -> Result<(), BackendError> {
        if self.reject_text {
            return Err(BackendError::invalid_request("no focused text input"));
        }
        self.recorded
            .lock()
            .expect("recorded mutex")
            .texts
            .push(text.to_string());
        Ok(())
    }

    fn screenshot(&self) -> Result<frust_devtools_protocol::ScreenshotResult, BackendError> {
        if self.screenshot_supported {
            Ok(frust_devtools_protocol::ScreenshotResult {
                png_base64: "iVBORw0KGgo=".to_string(),
            })
        } else {
            Err(BackendError::not_supported(
                "no readback path on this shell",
            ))
        }
    }
}

// ────────────────────────── log capture (discovery) ───────────────────────

static LOG_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOGGER_INSTALLED: Once = Once::new();

struct CaptureLogger;

impl log::Log for CaptureLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        LOG_LINES
            .lock()
            .expect("log capture mutex")
            .push(record.args().to_string());
    }

    fn flush(&self) {}
}

/// `log::set_logger` (not `set_boxed_logger`): the workspace `log` pin carries
/// no `std` feature, so the boxed variant is compiled out.
static CAPTURE_LOGGER: CaptureLogger = CaptureLogger;

fn install_logger() {
    LOGGER_INSTALLED.call_once(|| {
        log::set_logger(&CAPTURE_LOGGER).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

fn captured_lines() -> Vec<String> {
    LOG_LINES.lock().expect("log capture mutex").clone()
}

// ───────────────────────────── harness + client ───────────────────────────

struct Harness {
    handle: ServiceHandle,
    recorded: Arc<Mutex<Recorded>>,
}

impl Harness {
    fn start(backend: FakeBackend, config: ServiceConfig) -> Self {
        install_logger();
        let recorded = Arc::clone(&backend.recorded);
        let handle =
            Service::start_with_config(backend, AppInfo::new("fake-app", "0.1.0-test"), config)
                .expect("service starts on loopback");
        Self { handle, recorded }
    }

    fn client(&self) -> Client {
        Client::connect(self.handle.port())
    }
}

fn default_harness() -> Harness {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    Harness::start(FakeBackend::new(recorded), ServiceConfig::default())
}

/// A blocking NDJSON client — what tooling looks like from the service's side.
struct Client {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Client {
    fn connect(port: u16) -> Self {
        let writer = TcpStream::connect(("127.0.0.1", port)).expect("connect to the service");
        // A safety net only: every read below is of something the server is
        // required to produce, so a timeout here means a real defect, and a
        // failed test is a far better outcome than a hung one.
        writer
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let reader = BufReader::new(writer.try_clone().expect("clone the socket"));
        Self { writer, reader }
    }

    fn send_raw(&mut self, line: &str) {
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .expect("write a request line");
    }

    fn send(&mut self, req: &Request) {
        self.send_raw(&encode_line(req));
    }

    /// Next decoded line, or `None` at EOF.
    fn next_incoming(&mut self) -> Option<Incoming> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("read a line");
        if read == 0 {
            return None;
        }
        Some(decode_line(&line).expect("the server only ever writes valid protocol lines"))
    }

    /// The next line, which must be a response — an unexpected notification
    /// here is a failure, not something to skip past, since a client that
    /// never subscribed must never be pushed to.
    fn next_response(&mut self) -> Response {
        match self.next_incoming().expect("a response, not EOF") {
            Incoming::Response(resp) => resp,
            Incoming::Notification(n) => panic!("unexpected notification: {}", n.method),
            Incoming::Request(r) => panic!("unexpected request: {}", r.method),
        }
    }

    fn next_notification(&mut self) -> Notification {
        match self.next_incoming().expect("a notification, not EOF") {
            Incoming::Notification(n) => n,
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    fn call(&mut self, id: u64, method: Method, params: Value) -> Response {
        self.send(&Request::new(id, method.as_str(), params));
        let resp = self.next_response();
        assert_eq!(resp.id, id, "response id must correlate to the request");
        resp
    }
}

fn expect_result(resp: Response) -> Value {
    match resp.outcome {
        ResponseOutcome::Success { result } => result,
        ResponseOutcome::Error { error } => {
            panic!(
                "expected a success, got error {}: {}",
                error.code, error.message
            )
        }
    }
}

fn expect_error(resp: Response) -> RpcError {
    match resp.outcome {
        ResponseOutcome::Error { error } => error,
        ResponseOutcome::Success { result } => panic!("expected an error, got result {result}"),
    }
}

/// Typed params → wire `Value`. A macro rather than a generic function so this
/// test needs no `serde` dependency of its own — the bound is satisfied at each
/// concrete call site (see the crate's dependency charter in `Cargo.toml`).
macro_rules! params {
    ($value:expr) => {
        serde_json::to_value($value).expect("params serialize")
    };
}

fn sample_stats(n: u64) -> FrameStats {
    FrameStats {
        n,
        total_us: 16_000,
        rebuild_us: 1_000,
        layout_us: 2_000,
        paint_us: 3_000,
        encode_us: 4_000,
        acquire_us: 500,
        submit_us: 5_500,
        skipped: false,
    }
}

// ──────────────────────────────── the tests ───────────────────────────────

#[test]
fn handshake_reports_identity_protocol_version_and_capabilities() {
    let harness = default_harness();
    let mut client = harness.client();

    let result = expect_result(client.call(1, Method::Handshake, Value::Null));
    let info: HandshakeInfo = serde_json::from_value(result).expect("a HandshakeInfo result");

    assert_eq!(info.app_name, "fake-app");
    assert_eq!(info.frust_version, "0.1.0-test");
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert!(info.capabilities.contains(&Capability::WidgetTree));
    assert!(!info.capabilities.contains(&Capability::Screenshot));
}

#[test]
fn handshake_needs_no_params_field_at_all() {
    let harness = default_harness();
    let mut client = harness.client();

    // A hand-written client that omits `params` entirely must still be
    // answered — the envelope defaults it to null.
    client.send_raw(r#"{"jsonrpc":"2.0","id":9,"method":"handshake"}"#);
    let resp = client.next_response();
    assert_eq!(resp.id, 9);
    expect_result(resp);
}

#[test]
fn widget_tree_round_trips_the_backend_dump() {
    let harness = default_harness();
    let mut client = harness.client();

    let result = expect_result(client.call(1, Method::WidgetTree, Value::Null));
    let dump: WidgetTreeDump = serde_json::from_value(result).expect("a WidgetTreeDump result");

    assert_eq!(dump.roots.len(), 1);
    assert_eq!(dump.roots[0].type_name, "RootWidget");
    assert_eq!(dump.roots[0].children[0].id, 2);
    assert_eq!(
        dump.roots[0].children[0].debug_label.as_deref(),
        Some("greeting")
    );
}

#[test]
fn widget_props_answers_a_live_id_and_rejects_a_stale_one() {
    let harness = default_harness();
    let mut client = harness.client();

    let result =
        expect_result(client.call(1, Method::WidgetProps, params!(WidgetPropsParams { id: 2 })));
    let props: WidgetProps = serde_json::from_value(result).expect("a WidgetProps result");
    assert_eq!(
        props.entries,
        vec![("text".to_string(), "hello".to_string())]
    );

    let error = expect_error(client.call(
        2,
        Method::WidgetProps,
        params!(WidgetPropsParams { id: 999 }),
    ));
    assert_eq!(error.code, RpcError::INVALID_PARAMS);
}

#[test]
fn metrics_snapshot_round_trips() {
    let harness = default_harness();
    let mut client = harness.client();

    let result = expect_result(client.call(1, Method::MetricsSnapshot, Value::Null));
    let metrics: frust_devtools_protocol::MetricsSnapshot =
        serde_json::from_value(result).expect("a MetricsSnapshot result");
    assert_eq!(metrics.rss_bytes, Some(4_096));
    assert_eq!(metrics.uptime_ms, 1_234);
}

#[test]
fn input_methods_ack_and_the_backend_observes_them() {
    let harness = default_harness();
    let mut client = harness.client();

    for (id, method, params) in [
        (
            1,
            Method::InputTap,
            params!(InputTapParams { x: 10.0, y: 20.0 }),
        ),
        (
            2,
            Method::InputScroll,
            params!(InputScrollParams {
                x: 1.0,
                y: 2.0,
                dx: 3.0,
                dy: -4.0,
            }),
        ),
        (
            3,
            Method::InputText,
            params!(InputTextParams {
                text: "typed".to_string(),
            }),
        ),
    ] {
        let result = expect_result(client.call(id, method, params));
        let ack: AckResult = serde_json::from_value(result).expect("an AckResult");
        assert_eq!(ack, AckResult { ok: true }, "{method} should ack");
    }

    // The ack is written only after the backend call returned, so by now the
    // backend has definitely recorded all three — no synchronization needed.
    let recorded = harness.recorded.lock().expect("recorded mutex");
    assert_eq!(recorded.taps, vec![(10.0, 20.0)]);
    assert_eq!(recorded.scrolls, vec![(1.0, 2.0, 3.0, -4.0)]);
    assert_eq!(recorded.texts, vec!["typed".to_string()]);
}

#[test]
fn a_backend_rejection_becomes_an_invalid_params_error() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let backend = FakeBackend {
        reject_text: true,
        ..FakeBackend::new(recorded)
    };
    let harness = Harness::start(backend, ServiceConfig::default());
    let mut client = harness.client();

    let error = expect_error(client.call(
        1,
        Method::InputText,
        params!(InputTextParams {
            text: "x".to_string(),
        }),
    ));
    assert_eq!(error.code, RpcError::INVALID_PARAMS);
    assert_eq!(error.message, "no focused text input");
}

#[test]
fn malformed_params_are_rejected_before_the_backend_is_asked() {
    let harness = default_harness();
    let mut client = harness.client();

    let error = expect_error(client.call(1, Method::InputTap, serde_json::json!({"x": 1.0})));
    assert_eq!(error.code, RpcError::INVALID_PARAMS);
    assert!(
        harness
            .recorded
            .lock()
            .expect("recorded mutex")
            .taps
            .is_empty(),
        "a request that never decoded must never reach the backend"
    );
}

#[test]
fn screenshot_is_not_supported_by_default_and_says_so() {
    let harness = default_harness();
    let mut client = harness.client();

    let error = expect_error(client.call(1, Method::Screenshot, Value::Null));
    assert_eq!(error.code, RpcError::NOT_SUPPORTED);
}

#[test]
fn a_backend_that_implements_screenshot_answers_it() {
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let backend = FakeBackend {
        screenshot_supported: true,
        ..FakeBackend::new(recorded)
    };
    let harness = Harness::start(backend, ServiceConfig::default());
    let mut client = harness.client();

    let info: HandshakeInfo = serde_json::from_value(expect_result(client.call(
        1,
        Method::Handshake,
        Value::Null,
    )))
    .expect("a HandshakeInfo result");
    assert!(info.capabilities.contains(&Capability::Screenshot));

    let result = expect_result(client.call(2, Method::Screenshot, Value::Null));
    assert_eq!(result["png_base64"], "iVBORw0KGgo=");
}

#[test]
fn an_unknown_method_is_method_not_found_and_the_connection_survives() {
    let harness = default_harness();
    let mut client = harness.client();

    client.send(&Request::new(1, "teleport_widget", Value::Null));
    let error = expect_error(client.next_response());
    assert_eq!(error.code, RpcError::METHOD_NOT_FOUND);
    assert!(error.message.contains("teleport_widget"));

    // Still usable afterwards.
    expect_result(client.call(2, Method::Handshake, Value::Null));
}

#[test]
fn frame_stats_as_a_request_is_an_invalid_request_not_a_missing_method() {
    let harness = default_harness();
    let mut client = harness.client();

    let error = expect_error(client.call(1, Method::FrameStats, Value::Null));
    assert_eq!(error.code, RpcError::INVALID_REQUEST);
}

#[test]
fn a_malformed_line_with_a_recoverable_id_gets_a_parse_error_reply() {
    let harness = default_harness();
    let mut client = harness.client();

    // Valid JSON, correlatable id, unusable envelope (`method` is a number).
    client.send_raw(r#"{"jsonrpc":"2.0","id":4,"method":17}"#);
    let resp = client.next_response();
    assert_eq!(resp.id, 4);
    assert_eq!(expect_error(resp).code, RpcError::PARSE_ERROR);
}

#[test]
fn undecodable_lines_are_skipped_and_the_connection_keeps_serving() {
    let harness = default_harness();
    let mut client = harness.client();

    // None of these carries an id to correlate a reply to: the only correct
    // handling is to log and move on.
    client.send_raw("not json at all");
    client.send_raw("[1,2,3]");
    client.send_raw(r#"{"jsonrpc":"2.0"}"#);
    client.send_raw("");

    // The next well-formed request is answered normally — proving the four
    // above produced no reply and did not close the connection.
    let resp = client.call(1, Method::Handshake, Value::Null);
    assert_eq!(resp.id, 1);
    expect_result(resp);
}

#[test]
fn subscribing_then_publishing_delivers_a_frame_stats_notification() {
    let harness = default_harness();
    let mut client = harness.client();

    let result = expect_result(client.call(1, Method::FrameStatsSubscribe, Value::Null));
    let ack: AckResult = serde_json::from_value(result).expect("an AckResult");
    assert!(ack.ok);

    // The subscription is installed before the ack is written, so anything
    // published from here on is guaranteed to reach this client.
    harness.handle.publish_frame_stats(sample_stats(7));

    let notification = client.next_notification();
    assert_eq!(notification.method, Method::FrameStats.as_str());
    let stats: FrameStats =
        serde_json::from_value(notification.params).expect("a FrameStats payload");
    assert_eq!(stats, sample_stats(7));
}

#[test]
fn an_unsubscribed_client_receives_no_notifications() {
    let harness = default_harness();
    let mut subscriber = harness.client();
    let mut bystander = harness.client();

    expect_result(subscriber.call(1, Method::FrameStatsSubscribe, Value::Null));
    harness.handle.publish_frame_stats(sample_stats(1));
    assert_eq!(subscriber.next_notification().method, "frame_stats");

    // The bystander's only inbound line is its own response: if a stray
    // notification had been pushed to it, `next_response` would panic on it.
    expect_result(bystander.call(1, Method::Handshake, Value::Null));
}

#[test]
fn every_current_subscriber_receives_each_published_frame() {
    let harness = default_harness();
    let mut first = harness.client();
    let mut second = harness.client();

    expect_result(first.call(1, Method::FrameStatsSubscribe, Value::Null));
    expect_result(second.call(1, Method::FrameStatsSubscribe, Value::Null));

    harness.handle.publish_frame_stats(sample_stats(11));
    harness.handle.publish_frame_stats(sample_stats(12));

    for client in [&mut first, &mut second] {
        for expected in [11, 12] {
            let notification = client.next_notification();
            let stats: FrameStats =
                serde_json::from_value(notification.params).expect("a FrameStats payload");
            assert_eq!(stats.n, expected);
        }
    }
}

#[test]
fn publishing_far_past_the_queue_depth_never_blocks_the_publisher() {
    // A four-deep queue and a client that has subscribed but reads nothing:
    // the full-queue path. `publish_frame_stats` must return regardless —
    // this test completing at all is the assertion (a blocking publisher
    // would hang here, not fail).
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let harness = Harness::start(
        FakeBackend::new(recorded),
        ServiceConfig {
            frame_stats_capacity: 4,
            ..ServiceConfig::default()
        },
    );
    let mut client = harness.client();
    expect_result(client.call(1, Method::FrameStatsSubscribe, Value::Null));

    for n in 0..10_000 {
        harness.handle.publish_frame_stats(sample_stats(n));
    }

    // The connection is still alive and still serving: dropped frames are a
    // loss, never an error.
    let notification = client.next_notification();
    assert_eq!(notification.method, "frame_stats");
}

#[test]
fn the_discovery_line_is_formatted_so_the_protocol_parser_recovers_the_port() {
    let harness = default_harness();
    let port = harness.handle.port();

    let discovery: Vec<u16> = captured_lines()
        .iter()
        .filter_map(|line| parse_discovery_line(line))
        .collect();

    assert!(
        discovery.contains(&port),
        "no logged line parsed back to port {port}; captured: {:?}",
        captured_lines()
    );
    // ...and it really is built from the shared constant, not a lookalike.
    assert!(
        captured_lines()
            .iter()
            .any(|line| line.contains(DISCOVERY_PREFIX) && line.contains(&port.to_string()))
    );
}

#[test]
fn the_service_binds_loopback_only() {
    let harness = default_harness();
    // Reachable on 127.0.0.1 (every other test proves this)...
    drop(harness.client());
    // ...and the bind is narrow: another listener can take the same port on a
    // different address, which is only possible because the service did not
    // bind the wildcard. (A host without a second loopback alias configured
    // cannot answer the question either way, so it skips rather than fails.)
    match std::net::TcpListener::bind(("127.0.0.2", harness.handle.port())) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => {}
        Err(e) => panic!("the service appears to have bound more than 127.0.0.1: {e}"),
    }
}

#[test]
fn shutdown_closes_live_connections_and_stops_accepting() {
    let harness = default_harness();
    let mut client = harness.client();
    expect_result(client.call(1, Method::Handshake, Value::Null));

    let port = harness.handle.port();
    harness.handle.shutdown();

    // The live connection sees EOF, not a hang and not garbage.
    assert!(
        client.next_incoming().is_none(),
        "connection must be closed"
    );

    // And a fresh connection is either refused outright or immediately closed.
    match TcpStream::connect(("127.0.0.1", port)) {
        Err(_) => {}
        Ok(stream) => {
            let mut late = Client {
                writer: stream.try_clone().expect("clone"),
                reader: BufReader::new(stream),
            };
            late.writer
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("read timeout");
            assert!(
                late.next_incoming().is_none(),
                "a connection accepted after shutdown must not be served"
            );
        }
    }
}

#[test]
fn dropping_the_handle_shuts_the_service_down() {
    let harness = default_harness();
    let mut client = harness.client();
    expect_result(client.call(1, Method::Handshake, Value::Null));

    drop(harness);
    assert!(
        client.next_incoming().is_none(),
        "connection must be closed"
    );
}

#[test]
fn a_wedged_backend_times_out_the_caller_and_never_blocks_handshake() {
    let gate = Arc::new(Gate::default());
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let backend = FakeBackend {
        freeze: Some(Arc::clone(&gate)),
        ..FakeBackend::new(recorded)
    };
    let harness = Harness::start(
        backend,
        ServiceConfig {
            backend_timeout: Duration::from_millis(150),
            ..ServiceConfig::default()
        },
    );

    // Client A wedges the backend thread inside `widget_tree`.
    let mut stuck = harness.client();
    stuck.send(&Request::new(1, Method::WidgetTree.as_str(), Value::Null));

    // Client B is answered anyway: `handshake` is served from the startup
    // cache and never touches the backend.
    let mut other = harness.client();
    let info: HandshakeInfo =
        serde_json::from_value(expect_result(other.call(1, Method::Handshake, Value::Null)))
            .expect("a HandshakeInfo result");
    assert_eq!(info.app_name, "fake-app");

    // Client A gets an error rather than hanging forever.
    let error = expect_error(stuck.next_response());
    assert_eq!(error.code, RpcError::INTERNAL_ERROR);
    assert!(
        error.message.contains("timed out"),
        "unexpected message: {}",
        error.message
    );

    // Let the backend thread out before tearing down.
    gate.release();
}

#[test]
fn concurrent_clients_are_served_independently() {
    let harness = default_harness();
    let mut first = harness.client();
    let mut second = harness.client();
    let mut third = harness.client();

    // Interleaved, with distinct ids: each response must land on its own
    // connection with its own id.
    first.send(&Request::new(10, Method::WidgetTree.as_str(), Value::Null));
    second.send(&Request::new(
        20,
        Method::MetricsSnapshot.as_str(),
        Value::Null,
    ));
    third.send(&Request::new(30, Method::Handshake.as_str(), Value::Null));

    assert_eq!(first.next_response().id, 10);
    assert_eq!(second.next_response().id, 20);
    assert_eq!(third.next_response().id, 30);
}

#[test]
fn requests_pipelined_on_one_connection_are_answered_in_order() {
    let harness = default_harness();
    let mut client = harness.client();

    for id in 1..=5 {
        client.send(&Request::new(
            id,
            Method::MetricsSnapshot.as_str(),
            Value::Null,
        ));
    }
    for id in 1..=5 {
        assert_eq!(client.next_response().id, id);
    }
}
