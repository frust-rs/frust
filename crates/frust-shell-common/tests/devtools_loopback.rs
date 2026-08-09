//! End-to-end cover for the shell side of devtools: a real
//! [`frust_shell_common::devtools`] service over a real loopback socket, with a
//! stand-in "UI thread" driving [`pump`] exactly the way a shell's frame loop
//! does.
//!
//! This is the only host-runnable check of the UI-thread hop. The two mobile
//! shells' own `DevtoolsUi` impls live in `#[cfg(target_os = ...)]` modules that
//! never compile on the host, and the desktop one needs a live winit loop — so
//! the shared machinery underneath all three (queue → wake → pump → reply, and
//! injection reaching the UI as real `InputEvent`s) is proven here instead.
//!
//! Everything process-global in that module is once-per-process
//! (`Service`/`Bridge` are `OnceLock`s), so this file is deliberately ONE test:
//! a second one would either race the first's service or silently no-op.

#![cfg(feature = "devtools")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use frust_core::InspectNode;
use frust_core::event::{InputEvent, PointerPhase};
use frust_core::view::WidgetId;
use frust_devtools_protocol::{
    HandshakeParams, Incoming, Method, Request, Response, ResponseOutcome, RpcError, decode_line,
    encode_line, serde_json, serde_json::Value,
};
use frust_shell_common::devtools::{self, DevtoolsUi};
use kurbo::Rect;

/// A one-shot-per-cycle wake latch: the devtools bridge signals it from the
/// backend thread, the stand-in UI thread parks on it. Exactly the role the
/// desktop shell's `EventLoopProxy` plays.
#[derive(Default)]
struct Wake {
    pending: Mutex<bool>,
    signal: Condvar,
}

impl Wake {
    fn raise(&self) {
        *self.pending.lock().expect("wake mutex") = true;
        self.signal.notify_all();
    }

    /// Park until raised (or the bounded wait elapses, so a defect fails the
    /// test instead of hanging it).
    fn wait(&self) {
        let pending = self.pending.lock().expect("wake mutex");
        let (mut pending, _) = self
            .signal
            .wait_timeout(pending, Duration::from_millis(200))
            .expect("wake wait");
        *pending = false;
    }
}

/// The stand-in for a shell: a fixed tree snapshot plus a record of every event
/// the pump delivered.
struct TestUi {
    nodes: Vec<InspectNode>,
    events: Arc<Mutex<Vec<InputEvent>>>,
}

impl DevtoolsUi for TestUi {
    fn inspect(&self) -> Vec<InspectNode> {
        self.nodes.clone()
    }

    fn dispatch(&mut self, event: InputEvent) {
        self.events.lock().expect("events mutex").push(event);
    }
}

fn node(id: u64, parent: Option<u64>, children: &[u64], depth: usize, size: f64) -> InspectNode {
    InspectNode {
        id: WidgetId(id),
        parent: parent.map(WidgetId),
        type_name: "TestWidget",
        debug_label: None,
        bounds: Rect::new(0.0, 0.0, size, size),
        children: children.iter().copied().map(WidgetId).collect(),
        depth,
    }
}

/// A blocking NDJSON client — the same shape the tooling side uses.
struct Client {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Client {
    fn connect(port: u16) -> Self {
        let writer = TcpStream::connect(("127.0.0.1", port)).expect("connect to the service");
        // A safety net only: every read below is of a line the server is
        // required to produce, so a timeout means a real defect — and a failed
        // test is a far better outcome than a hung one.
        writer
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let reader = BufReader::new(writer.try_clone().expect("clone the socket"));
        Self { writer, reader }
    }

    fn call(&mut self, id: u64, method: Method, params: Value) -> Response {
        let request = Request::new(id, method.as_str(), params);
        self.writer
            .write_all(format!("{}\n", encode_line(&request)).as_bytes())
            .expect("write a request line");
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read a line");
        match decode_line(&line).expect("the server only writes valid protocol lines") {
            Incoming::Response(response) => {
                assert_eq!(response.id, id, "response id must correlate");
                response
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }
}

fn result_of(response: Response) -> Value {
    match response.outcome {
        ResponseOutcome::Success { result } => result,
        ResponseOutcome::Error { error } => {
            panic!("expected success, got {}: {}", error.code, error.message)
        }
    }
}

fn error_of(response: Response) -> RpcError {
    match response.outcome {
        ResponseOutcome::Error { error } => error,
        ResponseOutcome::Success { result } => panic!("expected an error, got {result}"),
    }
}

#[test]
fn a_client_reads_the_live_tree_and_drives_the_ui_through_the_hop() {
    let wake = Arc::new(Wake::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));

    devtools::start("test-app", {
        let wake = Arc::clone(&wake);
        Some(Box::new(move || wake.raise()))
    });
    let port = devtools::port().expect("the service bound a loopback port");
    // Auth is on by default, and the shell never sees the token except through
    // this in-process accessor — the same process that owns the secret.
    let token = devtools::token().expect("the service minted a handshake token");

    // The stand-in UI thread: park on the wake, drain the queue, repeat —
    // structurally identical to the desktop shell's woken `user_event` turn.
    let ui_thread = std::thread::spawn({
        let wake = Arc::clone(&wake);
        let stop = Arc::clone(&stop);
        let events = Arc::clone(&events);
        move || {
            let mut ui = TestUi {
                nodes: vec![
                    node(1, None, &[2], 0, 100.0),
                    node(2, Some(1), &[], 1, 40.0),
                ],
                events,
            };
            while !stop.load(Ordering::Relaxed) {
                wake.wait();
                devtools::pump(&mut ui);
            }
        }
    });

    let mut client = Client::connect(port);

    // Nothing is served before the token is presented — not even against a
    // fully live UI thread.
    let refused = error_of(client.call(1, Method::WidgetTree, Value::Null));
    assert_eq!(refused.code, RpcError::UNAUTHORIZED);

    // Handshake is answered from the cached info, without any hop.
    let handshake = result_of(
        client.call(
            2,
            Method::Handshake,
            serde_json::to_value(HandshakeParams {
                token: Some(token.clone()),
            })
            .expect("handshake params serialize"),
        ),
    );
    assert_eq!(handshake["app_name"], "test-app");

    // widget_tree hops to the UI thread and comes back nested.
    let tree = result_of(client.call(3, Method::WidgetTree, Value::Null));
    let roots = tree["roots"].as_array().expect("roots array");
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0]["id"], 1);
    assert_eq!(roots[0]["type_name"], "TestWidget");
    assert_eq!(roots[0]["children"][0]["id"], 2);
    assert_eq!(roots[0]["children"][0]["bounds"]["width"], 40.0);

    // widget_props resolves against the same snapshot.
    let props = result_of(client.call(4, Method::WidgetProps, serde_json::json!({ "id": 2 })));
    assert_eq!(props["id"], 2);
    let entries: Vec<(String, String)> =
        serde_json::from_value(props["entries"].clone()).expect("entries decode");
    assert!(entries.contains(&("type".to_string(), "TestWidget".to_string())));
    assert!(entries.contains(&("parent".to_string(), "1".to_string())));

    // metrics needs no hop at all.
    let metrics = result_of(client.call(5, Method::MetricsSnapshot, Value::Null));
    assert!(metrics["uptime_ms"].is_u64());

    // input_tap: the ack means "delivered", so the events are already recorded
    // by the time it arrives — no sleep, no polling.
    let ack = result_of(client.call(
        6,
        Method::InputTap,
        serde_json::json!({ "x": 12.0, "y": 34.0 }),
    ));
    assert_eq!(ack["ok"], true);

    let recorded = events.lock().expect("events mutex").clone();
    assert_eq!(
        recorded.len(),
        2,
        "a tap is a Down/Up gesture pair, not a lone Down: {recorded:?}"
    );
    match (&recorded[0], &recorded[1]) {
        (InputEvent::Pointer(down), InputEvent::Pointer(up)) => {
            assert_eq!(down.phase, PointerPhase::Down);
            assert_eq!(up.phase, PointerPhase::Up);
            assert_eq!(down.position.x, 12.0);
            assert_eq!(up.position.y, 34.0);
        }
        other => panic!("expected two pointer events, got {other:?}"),
    }

    // A malformed tap is answered with an error, never acked, and never
    // reaches the tree. (The non-finite-coordinate guard itself is unit-tested
    // in the module — JSON has no NaN literal to send here.)
    let rejected = client.call(8, Method::InputTap, serde_json::json!({ "x": 12.0 }));
    assert!(
        matches!(rejected.outcome, ResponseOutcome::Error { .. }),
        "a malformed tap must not be acked"
    );
    assert_eq!(
        events.lock().expect("events mutex").len(),
        2,
        "a rejected tap must deliver nothing"
    );

    stop.store(true, Ordering::Relaxed);
    wake.raise();
    ui_thread.join().expect("the UI thread exits cleanly");
}
