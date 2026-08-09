//! JSON-RPC 2.0 envelope types: [`Request`], [`Response`]/[`ResponseOutcome`],
//! [`Notification`], [`RpcError`], and the decoded-line discriminator
//! [`Incoming`]. Method-specific payloads (handshake info, widget tree,
//! frame stats, …) live in [`crate::messages`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The JSON-RPC 2.0 `"jsonrpc"` version literal every envelope carries.
pub const JSONRPC_VERSION: &str = "2.0";

fn jsonrpc_version() -> String {
    JSONRPC_VERSION.to_string()
}

/// A client→server call awaiting a [`Response`] correlated by `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    /// Absent on the wire deserializes as `Value::Null`, matching a
    /// parameterless method (e.g. `handshake`) — never a decode error.
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A server→client push carrying no `id` and expecting no [`Response`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            method: method.into(),
            params,
        }
    }
}

/// The reply to a [`Request`], correlated by `id`. Exactly one of `result`/
/// `error` is present on the wire (JSON-RPC 2.0 §5) — modeled as the
/// flattened [`ResponseOutcome`] rather than two `Option` fields so an
/// invalid "both present"/"neither present" shape can't be constructed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: u64,
    #[serde(flatten)]
    pub outcome: ResponseOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseOutcome {
    Success { result: Value },
    Error { error: RpcError },
}

impl Response {
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            outcome: ResponseOutcome::Success { result },
        }
    }

    pub fn error(id: u64, error: RpcError) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            outcome: ResponseOutcome::Error { error },
        }
    }
}

/// A JSON-RPC 2.0 error object (§5.1), plus a small implementation-defined
/// custom range this protocol occupies for its own conditions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    // Standard JSON-RPC 2.0 codes (§5.1).
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;

    /// JSON-RPC 2.0 reserves `-32000..=-32099` for implementation-defined
    /// server errors (§5.1). `NOT_SUPPORTED` is the first occupant — a
    /// capability-gated method (e.g. `screenshot`) rejects with it on a
    /// server whose handshake declared no matching [`crate::Capability`].
    pub const NOT_SUPPORTED: i32 = -32000;

    /// The second occupant of the implementation-defined range: the
    /// connection has not presented the server's per-process token
    /// ([`crate::HandshakeParams`]), so no method but `handshake` is
    /// dispatched. A client that receives this has the wrong token, or none —
    /// it must re-read the discovery line, never retry blindly.
    pub const UNAUTHORIZED: i32 = -32001;

    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::new(Self::NOT_SUPPORTED, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(Self::UNAUTHORIZED, message)
    }
}

/// A decoded line's discriminated shape — [`crate::decode_line`]'s return
/// type.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let req = Request::new(7, "handshake", Value::Null);
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_missing_jsonrpc_field_still_decodes() {
        let line = r#"{"id":1,"method":"handshake","params":null}"#;
        let req: Request = serde_json::from_str(line).unwrap();
        assert_eq!(req.jsonrpc, JSONRPC_VERSION);
    }

    #[test]
    fn response_success_round_trips_and_omits_error_key() {
        let resp = Response::success(3, serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn response_error_round_trips_and_omits_result_key() {
        let resp = Response::error(3, RpcError::not_supported("screenshot disabled"));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"error\""));
        assert!(!json.contains("\"result\""));
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
        match back.outcome {
            ResponseOutcome::Error { error } => {
                assert_eq!(error.code, RpcError::NOT_SUPPORTED);
            }
            ResponseOutcome::Success { .. } => panic!("expected an error outcome"),
        }
    }

    #[test]
    fn the_custom_error_codes_stay_inside_the_reserved_range() {
        // JSON-RPC 2.0 §5.1 reserves -32000..=-32099 for implementation-
        // defined server errors; drifting outside it would collide with a
        // standard code.
        for code in [RpcError::NOT_SUPPORTED, RpcError::UNAUTHORIZED] {
            assert!((-32099..=-32000).contains(&code), "out of range: {code}");
        }
        assert_ne!(RpcError::NOT_SUPPORTED, RpcError::UNAUTHORIZED);
        assert_eq!(RpcError::unauthorized("nope").code, RpcError::UNAUTHORIZED);
    }

    #[test]
    fn rpc_error_data_field_omitted_when_none() {
        let err = RpcError::new(RpcError::INTERNAL_ERROR, "boom");
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("\"data\""));
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        // No `deny_unknown_fields` anywhere in this crate — a newer client
        // or server may add a field a peer running an older crate version
        // doesn't know about; decoding must not fail on it.
        let line =
            r#"{"jsonrpc":"2.0","id":1,"method":"handshake","params":null,"extra":"future field"}"#;
        let req: Request = serde_json::from_str(line).unwrap();
        assert_eq!(req.method, "handshake");
    }
}
