//! # DAP Protocol Types
//!
//! Hand-rolled subset of the Debug Adapter Protocol (DAP) message types,
//! trimmed to what orchestration-v1 needs. References the [DAP
//! specification](https://microsoft.github.io/debug-adapter-protocol/specification).
//!
//! The wire format uses a `"type"` discriminator field to distinguish between
//! `"request"`, `"response"`, and `"event"` messages. This module models that
//! structure using a tagged serde enum.

use serde::{Deserialize, Serialize};

/// Top-level DAP protocol message — discriminated by the `"type"` field.
///
/// All three variants are transmitted in the same Content-Length framed format.
/// The `"type"` field selects the variant during deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DapMessage {
    /// A request from the debug adapter client (e.g., VS Code).
    #[serde(rename = "request")]
    Request(DapRequest),

    /// A response from the debug adapter server to a prior client request.
    #[serde(rename = "response")]
    Response(DapResponse),

    /// An unsolicited event sent by the debug adapter server.
    #[serde(rename = "event")]
    Event(DapEvent),
}

/// A DAP request from the client (e.g., VS Code).
///
/// The `seq` is a monotonically increasing sequence number used to correlate
/// requests with responses. The `command` names the operation (e.g.,
/// `"initialize"`, `"launch"`, `"threads"`). The optional `arguments`
/// payload is command-specific.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapRequest {
    /// Sequence number (monotonically increasing, per-sender).
    pub seq: i64,

    /// The command name (e.g., `"initialize"`, `"launch"`).
    pub command: String,

    /// Command-specific arguments, or `None` if the command takes no arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// A DAP response sent from the server to the client in reply to a request.
///
/// The `request_seq` correlates this response with the originating request.
/// `success` indicates whether the command succeeded. On failure, `message`
/// carries a human-readable error description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapResponse {
    /// Sequence number assigned by the server.
    pub seq: i64,

    /// The `seq` of the request this response answers.
    pub request_seq: i64,

    /// Whether the request was handled successfully.
    pub success: bool,

    /// Echoes the command name from the originating request.
    pub command: String,

    /// Human-readable error message when `success` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Command-specific response body, or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

impl DapResponse {
    /// Create a success response for the given request.
    ///
    /// Sets `request_seq` to the request's `seq`, `success` to `true`, and
    /// echoes the `command`. Sequence number is initialized to 0; the DAP
    /// server session assigns the final value before transmission.
    pub fn success(request: &DapRequest, body: Option<serde_json::Value>) -> Self {
        Self {
            seq: 0,
            request_seq: request.seq,
            success: true,
            command: request.command.clone(),
            message: None,
            body,
        }
    }

    /// Create an error response for the given request.
    ///
    /// Sets `request_seq` to the request's `seq`, `success` to `false`, and
    /// stores the error description in `message`.
    pub fn error(request: &DapRequest, message: impl Into<String>) -> Self {
        Self {
            seq: 0,
            request_seq: request.seq,
            success: false,
            command: request.command.clone(),
            message: Some(message.into()),
            body: None,
        }
    }
}

/// A DAP event sent unsolicited from the server to the client.
///
/// Events notify the client of state changes (e.g., `"initialized"`,
/// `"output"`, `"exited"`, `"terminated"`). The `body` is event-specific.
///
/// Note: `seq` is set to 0 in convenience constructors. The DAP server session
/// is responsible for assigning monotonic sequence numbers before transmission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapEvent {
    /// Sequence number assigned by the server.
    pub seq: i64,

    /// The event name (e.g., `"initialized"`, `"output"`, `"exited"`).
    pub event: String,

    /// Event-specific payload, or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

impl DapEvent {
    /// Create a new event with the given name and optional body.
    ///
    /// Sequence number is initialized to 0; the DAP server session assigns
    /// the final value before transmission.
    pub fn new(event: impl Into<String>, body: Option<serde_json::Value>) -> Self {
        Self {
            seq: 0,
            event: event.into(),
            body,
        }
    }

    /// Create the `"initialized"` event.
    ///
    /// Sent by the server immediately after a successful `"initialize"`
    /// response to signal that the adapter is ready to accept configuration
    /// requests.
    pub fn initialized() -> Self {
        Self::new("initialized", None)
    }

    /// Create a `"terminated"` event.
    ///
    /// Signals to the client that debugging has ended (e.g., the debuggee
    /// exited or the session was disconnected).
    pub fn terminated() -> Self {
        Self::new("terminated", None)
    }

    /// Create an `"output"` event for debug console output.
    ///
    /// # Arguments
    /// * `category` - The output category: `"console"`, `"stdout"`, `"stderr"`, or `"telemetry"`.
    /// * `output` - The text to display. Should end with a newline where appropriate.
    pub fn output(category: &str, output: &str) -> Self {
        let body = OutputEventBody {
            category: Some(category.to_owned()),
            output: output.to_owned(),
        };
        Self::new(
            "output",
            Some(serde_json::to_value(body).unwrap_or(serde_json::Value::Null)),
        )
    }

    /// Create an `"exited"` event indicating the debuggee has exited.
    ///
    /// # Arguments
    /// * `exit_code` - The exit code returned by the debuggee.
    pub fn exited(exit_code: i64) -> Self {
        let body = ExitedEventBody { exit_code };
        Self::new(
            "exited",
            Some(serde_json::to_value(body).unwrap_or(serde_json::Value::Null)),
        )
    }
}

/// Body of an `"output"` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEventBody {
    /// The output category: `"console"`, `"stdout"`, `"stderr"`, or `"telemetry"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,

    /// The output text to display.
    pub output: String,
}

/// Body of an `"exited"` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitedEventBody {
    /// The exit code returned by the debuggee.
    pub exit_code: i64,
}

/// Server capabilities advertised during the DAP initialization handshake.
///
/// Trimmed to the capabilities orchestration-v1 actually sets; unknown fields
/// are ignored by compliant clients, so widening this later is
/// backward-compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// The debug adapter supports the `configurationDone` request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_configuration_done_request: Option<bool>,

    /// The debug adapter supports the `terminate` request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_terminate_request: Option<bool>,
}

impl Capabilities {
    /// Default capabilities for Frust's DAP adapter (orchestration-v1).
    pub fn frust_defaults() -> Self {
        Self {
            supports_configuration_done_request: Some(true),
            supports_terminate_request: Some(true),
        }
    }
}

/// Arguments sent by the client in the DAP `"initialize"` request.
///
/// These fields describe the client's identity and capability preferences.
/// Fields the client omits are deserialized as `None`.
///
/// Note: `clientID` and `adapterID` use uppercase "ID" per the DAP spec (not
/// `clientId`/`adapterId`). These are explicitly renamed using `#[serde(rename)]`
/// rather than relying on `camelCase` which would produce lowercase `d`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequestArguments {
    /// A unique identifier for the client implementation (e.g., `"vscode"`).
    /// DAP spec uses `clientID` (uppercase ID).
    #[serde(default, rename = "clientID")]
    pub client_id: Option<String>,

    /// A human-readable name for the client (e.g., `"Visual Studio Code"`).
    #[serde(default)]
    pub client_name: Option<String>,

    /// The ID of the debug adapter (e.g., `"frust"`).
    /// DAP spec uses `adapterID` (uppercase ID).
    #[serde(default, rename = "adapterID")]
    pub adapter_id: Option<String>,

    /// The locale of the client, e.g., `"en-US"`.
    #[serde(default)]
    pub locale: Option<String>,

    /// Whether line numbers are 1-based. Defaults to `true` if omitted.
    #[serde(default)]
    pub lines_start_at1: Option<bool>,

    /// Whether column numbers are 1-based. Defaults to `true` if omitted.
    #[serde(default)]
    pub columns_start_at1: Option<bool>,

    /// Determines how paths are reported: `"path"` or `"uri"`.
    #[serde(default)]
    pub path_format: Option<String>,
}

/// Arguments sent by the client in the DAP `"launch"` request.
///
/// A `launch.json` configuration commonly carries extra editor-specific
/// fields beyond these three — unknown fields are silently ignored (no
/// `deny_unknown_fields`), and every field defaults to `None` when the
/// client omits it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchArguments {
    /// The Frust project directory to launch.
    #[serde(default)]
    pub project_root: Option<String>,

    /// The target device id, matching `frust-drive::devices::Device::id`.
    #[serde(default)]
    pub device: Option<String>,

    /// The build mode (`"debug"`, `"profile"`, or `"release"`).
    #[serde(default)]
    pub mode: Option<String>,
}

/// DAP Thread object.
///
/// Orchestration-v1 answers the `threads` request with a single static
/// entry — no isolate/thread-detail modeling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// Unique thread identifier.
    pub id: i64,
    /// A human-readable name for the thread.
    pub name: String,
}

/// Response body for the DAP `"threads"` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadsResponseBody {
    pub threads: Vec<Thread>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── DapMessage serde round-trip ─────────────────────────────────────────

    #[test]
    fn test_dap_message_request_round_trip() {
        let msg = DapMessage::Request(DapRequest {
            seq: 3,
            command: "initialize".into(),
            arguments: Some(serde_json::json!({"clientID": "vscode"})),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "request");
        assert_eq!(json["seq"], 3);
        assert_eq!(json["command"], "initialize");

        let back: DapMessage = serde_json::from_value(json).unwrap();
        match back {
            DapMessage::Request(r) => {
                assert_eq!(r.seq, 3);
                assert_eq!(r.command, "initialize");
            }
            other => panic!("expected Request, got {:?}", other),
        }
    }

    #[test]
    fn test_dap_message_response_round_trip() {
        let req = DapRequest {
            seq: 1,
            command: "threads".into(),
            arguments: None,
        };
        let body = serde_json::to_value(ThreadsResponseBody {
            threads: vec![Thread {
                id: 1,
                name: "main".into(),
            }],
        })
        .unwrap();
        let msg = DapMessage::Response(DapResponse::success(&req, Some(body)));
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "response");
        assert_eq!(json["success"], true);
        assert_eq!(json["command"], "threads");

        let back: DapMessage = serde_json::from_value(json).unwrap();
        match back {
            DapMessage::Response(r) => {
                assert!(r.success);
                assert_eq!(r.request_seq, 1);
                assert_eq!(r.body.unwrap()["threads"][0]["name"], "main");
            }
            other => panic!("expected Response, got {:?}", other),
        }
    }

    #[test]
    fn test_dap_response_error() {
        let req = DapRequest {
            seq: 5,
            command: "launch".into(),
            arguments: None,
        };
        let resp = DapResponse::error(&req, "no device found");
        assert!(!resp.success);
        assert_eq!(resp.request_seq, 5);
        assert_eq!(resp.message.as_deref(), Some("no device found"));
        assert!(resp.body.is_none());
    }

    #[test]
    fn test_dap_message_event_round_trip() {
        let msg = DapMessage::Event(DapEvent::output("stdout", "hello\n"));
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["event"], "output");
        assert_eq!(json["body"]["output"], "hello\n");
        assert_eq!(json["body"]["category"], "stdout");

        let back: DapMessage = serde_json::from_value(json).unwrap();
        match back {
            DapMessage::Event(e) => assert_eq!(e.event, "output"),
            other => panic!("expected Event, got {:?}", other),
        }
    }

    #[test]
    fn test_dap_event_exited_body() {
        let event = DapEvent::exited(7);
        let body = event.body.unwrap();
        assert_eq!(body["exitCode"], 7);
    }

    #[test]
    fn test_dap_event_terminated_has_no_body() {
        let event = DapEvent::terminated();
        assert_eq!(event.event, "terminated");
        assert!(event.body.is_none());
    }

    // ── Capabilities ─────────────────────────────────────────────────────────

    #[test]
    fn test_capabilities_frust_defaults() {
        let caps = Capabilities::frust_defaults();
        let json = serde_json::to_value(&caps).unwrap();
        assert_eq!(json["supportsConfigurationDoneRequest"], true);
        assert_eq!(json["supportsTerminateRequest"], true);
    }

    #[test]
    fn test_capabilities_omits_unset_fields() {
        let caps = Capabilities::default();
        let json = serde_json::to_value(&caps).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    // ── InitializeRequestArguments ──────────────────────────────────────────

    #[test]
    fn test_initialize_request_arguments_client_id_uppercase() {
        let args: InitializeRequestArguments =
            serde_json::from_value(serde_json::json!({"clientID": "vscode"})).unwrap();
        assert_eq!(args.client_id.as_deref(), Some("vscode"));
    }

    #[test]
    fn test_initialize_request_arguments_defaults_on_empty() {
        let args: InitializeRequestArguments =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(args.client_id.is_none());
        assert!(args.adapter_id.is_none());
    }

    // ── LaunchArguments ──────────────────────────────────────────────────────

    #[test]
    fn test_launch_arguments_round_trip() {
        let args = LaunchArguments {
            project_root: Some("/home/me/app".into()),
            device: Some("emulator-5554".into()),
            mode: Some("debug".into()),
        };
        let json = serde_json::to_value(&args).unwrap();
        assert_eq!(json["projectRoot"], "/home/me/app");
        assert_eq!(json["device"], "emulator-5554");
        assert_eq!(json["mode"], "debug");

        let back: LaunchArguments = serde_json::from_value(json).unwrap();
        assert_eq!(back.project_root.as_deref(), Some("/home/me/app"));
        assert_eq!(back.device.as_deref(), Some("emulator-5554"));
        assert_eq!(back.mode.as_deref(), Some("debug"));
    }

    #[test]
    fn test_launch_arguments_defaults_when_missing() {
        let args: LaunchArguments = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(args.project_root.is_none());
        assert!(args.device.is_none());
        assert!(args.mode.is_none());
    }

    #[test]
    fn test_launch_arguments_tolerates_unknown_fields() {
        // A real launch.json commonly carries extra editor-specific fields
        // (e.g. VS Code's "type"/"request"/"name") beyond the three this
        // struct models — they must not fail deserialization.
        let raw = serde_json::json!({
            "type": "frust",
            "request": "launch",
            "name": "Frust",
            "projectRoot": "/home/me/app",
            "device": "emulator-5554",
            "mode": "debug",
            "someFutureEditorField": {"nested": true},
        });
        let args: LaunchArguments = serde_json::from_value(raw).unwrap();
        assert_eq!(args.project_root.as_deref(), Some("/home/me/app"));
        assert_eq!(args.device.as_deref(), Some("emulator-5554"));
        assert_eq!(args.mode.as_deref(), Some("debug"));
    }

    // ── Thread / ThreadsResponseBody ────────────────────────────────────────

    #[test]
    fn test_threads_response_body_round_trip() {
        let body = ThreadsResponseBody {
            threads: vec![Thread {
                id: 1,
                name: "main".into(),
            }],
        };
        let json = serde_json::to_value(&body).unwrap();
        let back: ThreadsResponseBody = serde_json::from_value(json).unwrap();
        assert_eq!(back.threads.len(), 1);
        assert_eq!(back.threads[0].id, 1);
        assert_eq!(back.threads[0].name, "main");
    }
}
