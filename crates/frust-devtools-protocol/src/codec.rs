//! NDJSON line framing: one JSON-RPC 2.0 object per `\n`-terminated line, no
//! `Content-Length` headers. Pure functions only — no I/O; the caller (the
//! in-app service or `frust-drive`/`frust-tui` tooling) owns the actual
//! stream/socket.

use serde::Serialize;
use serde_json::Value;

use crate::types::Incoming;

/// Serializes `value` to a single JSON-RPC line with **no** trailing
/// newline — the caller appends `\n` when writing it to a stream (matching
/// `decode_line`, which also takes one line with the newline already
/// stripped).
///
/// `serde_json` always escapes control characters, including a literal
/// `\n`, inside string values — a well-formed `Serialize` impl can
/// therefore never make this produce an embedded newline in the first
/// place. The `debug_assert!` below is a belt-and-suspenders guarantee
/// against a future custom `Serialize` impl breaking that invariant, not a
/// runtime check this protocol expects to ever fire.
pub fn encode_line(value: &impl Serialize) -> String {
    let line = serde_json::to_string(value)
        .expect("frust-devtools-protocol wire types are always representable as JSON");
    debug_assert!(
        !line.contains('\n'),
        "encode_line must never produce an embedded newline (NDJSON framing contract)"
    );
    line
}

/// Parses one NDJSON line into its discriminated [`Incoming`] shape.
///
/// Discrimination follows JSON-RPC 2.0's own shape rules rather than a tag
/// field the wire types don't carry: a `method` key marks a request (if
/// `id` is also present) or a notification (no `id`); an `id` key with no
/// `method` marks a response. A trailing `\n`, if present, is stripped
/// first so a caller can hand this either a bare line or one still carrying
/// its terminator.
pub fn decode_line(line: &str) -> Result<Incoming, DecodeError> {
    let value: Value = serde_json::from_str(line.trim_end_matches('\n'))?;
    let obj = value.as_object().ok_or(DecodeError::NotAnObject)?;
    let has_method = obj.contains_key("method");
    let has_id = obj.contains_key("id");

    if has_method && has_id {
        Ok(Incoming::Request(serde_json::from_value(value)?))
    } else if has_method {
        Ok(Incoming::Notification(serde_json::from_value(value)?))
    } else if has_id {
        Ok(Incoming::Response(serde_json::from_value(value)?))
    } else {
        Err(DecodeError::UnrecognizedShape)
    }
}

/// [`decode_line`]'s failure modes.
#[derive(Debug)]
pub enum DecodeError {
    /// The line isn't valid JSON at all.
    InvalidJson(serde_json::Error),
    /// The line parsed as JSON but isn't an object (e.g. a bare array or
    /// scalar).
    NotAnObject,
    /// The object has neither a `method` nor an `id` field, so it matches
    /// none of [`Incoming`]'s three shapes.
    UnrecognizedShape,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::InvalidJson(e) => write!(f, "invalid JSON: {e}"),
            DecodeError::NotAnObject => write!(f, "line is not a JSON object"),
            DecodeError::UnrecognizedShape => write!(
                f,
                "object has neither a `method` nor an `id` field — not a Request, Response, or Notification"
            ),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::InvalidJson(e) => Some(e),
            DecodeError::NotAnObject | DecodeError::UnrecognizedShape => None,
        }
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(e: serde_json::Error) -> Self {
        DecodeError::InvalidJson(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Request, Response};

    #[test]
    fn encode_line_never_embeds_a_raw_newline() {
        let req = Request::new(
            1,
            "input_text",
            serde_json::json!({"text": "line one\nline two"}),
        );
        let line = encode_line(&req);
        assert!(
            !line.contains('\n'),
            "embedded newline in encoded params must be escaped, not literal: {line:?}"
        );
        // ...and it still round-trips the exact string with the embedded newline intact.
        let decoded: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(decoded.params["text"], "line one\nline two");
    }

    #[test]
    fn decode_line_discriminates_request() {
        let line = encode_line(&Request::new(1, "handshake", Value::Null));
        match decode_line(&line).unwrap() {
            Incoming::Request(r) => assert_eq!(r.method, "handshake"),
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn decode_line_discriminates_notification() {
        let notif = crate::types::Notification::new("frame_stats", serde_json::json!({"n": 1}));
        let line = encode_line(&notif);
        match decode_line(&line).unwrap() {
            Incoming::Notification(n) => assert_eq!(n.method, "frame_stats"),
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn decode_line_discriminates_response() {
        let resp = Response::success(9, serde_json::json!({"ok": true}));
        let line = encode_line(&resp);
        match decode_line(&line).unwrap() {
            Incoming::Response(r) => assert_eq!(r.id, 9),
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn decode_line_accepts_trailing_newline() {
        let line = format!(
            "{}\n",
            encode_line(&Request::new(1, "handshake", Value::Null))
        );
        assert!(decode_line(&line).is_ok());
    }

    #[test]
    fn decode_line_rejects_shape_with_neither_method_nor_id() {
        let err = decode_line(r#"{"foo":"bar"}"#).unwrap_err();
        assert!(matches!(err, DecodeError::UnrecognizedShape));
    }

    #[test]
    fn decode_line_rejects_non_object() {
        let err = decode_line("[1,2,3]").unwrap_err();
        assert!(matches!(err, DecodeError::NotAnObject));
    }

    #[test]
    fn decode_line_rejects_invalid_json() {
        let err = decode_line("not json").unwrap_err();
        assert!(matches!(err, DecodeError::InvalidJson(_)));
    }

    #[test]
    fn decode_line_tolerates_unknown_method_name() {
        // A method this crate's `Method` enum doesn't know about must still
        // decode as a Request — dispatch, not framing, decides what an
        // unrecognized method means.
        let req = Request::new(1, "some_future_method", Value::Null);
        let line = encode_line(&req);
        match decode_line(&line).unwrap() {
            Incoming::Request(r) => assert_eq!(r.method, "some_future_method"),
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn decode_line_error_impls_std_error() {
        let err = decode_line("not json").unwrap_err();
        let _: &dyn std::error::Error = &err;
        assert!(std::error::Error::source(&err).is_some());
    }
}
