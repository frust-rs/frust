//! A hand-rolled NDJSON devtools server standing in for a running frust app.
//!
//! It enforces the same handshake-token gate the real service does, answers
//! the v1 methods with canned results, pushes one `frame_stats` notification
//! after acking a subscribe, and records every request it served so a test
//! can assert *how many* round trips a tool made — the property the
//! one-round-trip rule in `tools::driving` exists to protect.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use frust_devtools_protocol::{
    AckResult, Capability, FrameStats, HandshakeInfo, HandshakeParams, MetricsSnapshot,
    Notification, PROTOCOL_VERSION, RectPx, Request, Response, RpcError, ScreenshotResult,
    WidgetNode, WidgetProps, WidgetTreeDump, encode_line,
};
use serde_json::Value;

/// The token the fixture server requires at handshake — the stand-in for one
/// recovered from a real discovery line.
pub const FIXTURE_TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// How the fixture answers `screenshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotBehavior {
    /// Reject with `RpcError::NOT_SUPPORTED`, as a backend that never
    /// implemented it does.
    NotSupported,
    /// Return the canned PNG below.
    Png,
}

/// What a fixture server declares and how it behaves.
#[derive(Debug, Clone)]
pub struct FixtureConfig {
    pub capabilities: Vec<Capability>,
    pub screenshot: ScreenshotBehavior,
}

impl Default for FixtureConfig {
    fn default() -> Self {
        Self {
            capabilities: vec![
                Capability::WidgetTree,
                Capability::FrameStats,
                Capability::Input,
            ],
            screenshot: ScreenshotBehavior::NotSupported,
        }
    }
}

impl FixtureConfig {
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_screenshot(mut self, screenshot: ScreenshotBehavior) -> Self {
        self.screenshot = screenshot;
        self
    }
}

/// A running fixture server plus the log of what it served.
pub struct Fixture {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    handle: JoinHandle<()>,
}

impl Fixture {
    /// Binds an ephemeral loopback port and serves until a connection that
    /// actually spoke closes.
    ///
    /// It accepts **more than one** connection, because the engine's connect
    /// thread opens a bounded `connect_timeout` reachability probe and drops
    /// it before the client's own connect (see `engine::devtools`'s module
    /// doc). The probe sends nothing, so it is served and discarded; the
    /// thread returns once a connection that carried a request closes — which
    /// is also what proves the engine dropped its client at teardown.
    pub fn spawn(config: FixtureConfig) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                if serve_connection(stream, &config, &recorder) {
                    break;
                }
            }
        });
        Fixture {
            addr,
            requests,
            handle,
        }
    }

    /// Every `params` value served for `method`, in arrival order.
    pub fn requests_of(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(served, _)| served == method)
            .map(|(_, params)| params.clone())
            .collect()
    }

    pub fn request_count(&self, method: &str) -> usize {
        self.requests_of(method).len()
    }

    /// Joins the server thread — which only returns once the engine has
    /// dropped its connection.
    pub fn join(self) {
        self.handle.join().expect("fixture server thread");
    }
}

/// Serves one fixture connection until EOF; returns whether it carried any
/// request at all (`false` for the engine's reachability probe).
fn serve_connection(
    stream: TcpStream,
    config: &FixtureConfig,
    recorder: &Arc<Mutex<Vec<(String, Value)>>>,
) -> bool {
    let mut writer = stream.try_clone().expect("clone fixture stream");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut authenticated = false;
    let mut served_any = false;
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return served_any;
        }
        let Ok(request) = serde_json::from_str::<Request>(line.trim_end()) else {
            continue;
        };
        served_any = true;
        recorder
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((request.method.clone(), request.params.clone()));

        if request.method == "handshake" {
            let presented = serde_json::from_value::<HandshakeParams>(request.params.clone())
                .ok()
                .and_then(|params| params.token);
            authenticated = presented.as_deref() == Some(FIXTURE_TOKEN);
        }
        if !authenticated {
            let response = Response::error(
                request.id,
                RpcError::unauthorized("present the devtools token at handshake"),
            );
            let _ = writeln!(writer, "{}", encode_line(&response));
            continue;
        }

        let response = match answer(&request.method, config) {
            Ok(result) => Response::success(request.id, result),
            Err(error) => Response::error(request.id, error),
        };
        let _ = writeln!(writer, "{}", encode_line(&response));

        if request.method == "frame_stats_subscribe" {
            let notification = Notification::new(
                "frame_stats",
                serde_json::to_value(fixture_frame()).expect("encode frame stats"),
            );
            let _ = writeln!(writer, "{}", encode_line(&notification));
        }
    }
}

fn answer(method: &str, config: &FixtureConfig) -> Result<Value, RpcError> {
    let value = match method {
        "handshake" => serde_json::to_value(HandshakeInfo {
            app_name: "fixture-app".into(),
            frust_version: "0.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: config.capabilities.clone(),
        }),
        "widget_tree" => serde_json::to_value(fixture_tree()),
        "widget_props" => serde_json::to_value(WidgetProps {
            id: SAVE_BUTTON_ID,
            entries: vec![
                ("label".to_string(), "Save".to_string()),
                ("enabled".to_string(), "true".to_string()),
            ],
        }),
        "metrics_snapshot" => serde_json::to_value(MetricsSnapshot {
            rss_bytes: Some(42 * 1024 * 1024),
            uptime_ms: 1_234,
        }),
        "screenshot" => match config.screenshot {
            ScreenshotBehavior::NotSupported => {
                return Err(RpcError::not_supported(
                    "this backend does not implement screenshot",
                ));
            }
            ScreenshotBehavior::Png => serde_json::to_value(ScreenshotResult {
                png_base64: fixture_png_base64(),
            }),
        },
        _ => serde_json::to_value(AckResult { ok: true }),
    };
    Ok(value.expect("encode fixture result"))
}

/// The one frame sample the fixture pushes; asserted field-for-field so a
/// silent re-shaping of the payload cannot pass.
pub fn fixture_frame() -> FrameStats {
    FrameStats {
        n: 7,
        total_us: 12_345,
        rebuild_us: 100,
        layout_us: 200,
        paint_us: 300,
        encode_us: 400,
        acquire_us: 500,
        submit_us: 600,
        skipped: false,
    }
}

pub const ROOT_ID: u64 = 1;
pub const SAVE_BUTTON_ID: u64 = 2;
pub const CANCEL_BUTTON_ID: u64 = 3;
pub const COLUMN_ID: u64 = 4;
pub const TEXT_ID: u64 = 5;

/// The tree every driving/diagnosis test asserts against:
///
/// ```text
/// AppRootWidget            (0,0 400x800)
/// ├─ ButtonWidget "Save"   (10,20 100x40)   center (60, 40)
/// ├─ ButtonWidget "Cancel" (120,20 100x40)  center (170, 40)
/// └─ ColumnWidget          (0,80 400x200)
///    └─ TextWidget "Hello" (0,80 200x20)
/// ```
pub fn fixture_tree() -> WidgetTreeDump {
    WidgetTreeDump {
        roots: vec![node(
            ROOT_ID,
            "AppRootWidget",
            None,
            rect(0.0, 0.0, 400.0, 800.0),
            vec![
                node(
                    SAVE_BUTTON_ID,
                    "ButtonWidget",
                    Some("Save"),
                    rect(10.0, 20.0, 100.0, 40.0),
                    Vec::new(),
                ),
                node(
                    CANCEL_BUTTON_ID,
                    "ButtonWidget",
                    Some("Cancel"),
                    rect(120.0, 20.0, 100.0, 40.0),
                    Vec::new(),
                ),
                node(
                    COLUMN_ID,
                    "ColumnWidget",
                    None,
                    rect(0.0, 80.0, 400.0, 200.0),
                    vec![node(
                        TEXT_ID,
                        "TextWidget",
                        Some("Hello"),
                        rect(0.0, 80.0, 200.0, 20.0),
                        Vec::new(),
                    )],
                ),
            ],
        )],
    }
}

fn node(
    id: u64,
    type_name: &str,
    debug_label: Option<&str>,
    bounds: Option<RectPx>,
    children: Vec<WidgetNode>,
) -> WidgetNode {
    WidgetNode {
        id,
        type_name: type_name.to_string(),
        debug_label: debug_label.map(str::to_string),
        bounds,
        children,
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Option<RectPx> {
    Some(RectPx {
        x,
        y,
        width,
        height,
    })
}

/// An 8-byte PNG signature, base64-encoded — enough for the screenshot
/// tool's "is this really a PNG?" check without shipping a real image.
pub fn fixture_png_base64() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .encode([0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}
