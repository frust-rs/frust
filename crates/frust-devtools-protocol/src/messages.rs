//! Typed request-param / result / notification-payload structs for each v1
//! [`crate::Method`]. A [`Request::params`](crate::Request)/
//! [`Response`](crate::Response) `result` field is transport-generic
//! (`serde_json::Value`); a caller matches on [`crate::Method`] and
//! `serde_json::from_value`/`to_value`s the type below that pairs with it —
//! that pairing is a documented convention here, not something the type
//! system enforces, since the envelope has to stay method-agnostic.

use serde::{Deserialize, Serialize};

/// `handshake` result — server identity plus the declared capability set a
/// client uses to know which other methods are safe to call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeInfo {
    pub app_name: String,
    pub frust_version: String,
    pub protocol_version: u32,
    pub capabilities: Vec<Capability>,
}

/// A capability a server may declare at handshake, gating which other
/// methods a client should expect to succeed (`screenshot` is the v1
/// example — see [`crate::RpcError::NOT_SUPPORTED`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    WidgetTree,
    FrameStats,
    Input,
    Metrics,
    Screenshot,
    /// A capability name introduced by a newer protocol version than this
    /// crate knows about. Deserializing an unrecognized capability must not
    /// fail the whole handshake — forward compatibility, at the cost of not
    /// round-tripping the original unrecognized name.
    #[serde(other)]
    Unknown,
}

/// `widget_tree` result. Nested (parent owns `children: Vec<WidgetNode>`)
/// rather than a flat id-indexed list with parent pointers — the simplest
/// v1 shape, and the one a debug client walks directly to render a tree
/// view with no separate reconstruction pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetTreeDump {
    pub roots: Vec<WidgetNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetNode {
    pub id: u64,
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<RectPx>,
    #[serde(default)]
    pub children: Vec<WidgetNode>,
}

/// A logical-px rectangle — the same coordinate space every framework
/// `InputEvent` uses (`docs/CODE_STANDARDS.md`'s Interaction Semantics:
/// "Events are logical-coordinate by the time they cross `AppTree`").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RectPx {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// `widget_props` request params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetPropsParams {
    pub id: u64,
}

/// `widget_props` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetProps {
    pub id: u64,
    pub entries: Vec<(String, String)>,
}

/// `frame_stats_subscribe` acknowledges with [`crate::AckResult`]; the
/// server then pushes this payload as the `frame_stats` **notification**
/// body on every subsequent frame.
///
/// Field names mirror `frust-shell-common::perf`'s `frust-perf raw` line
/// verbatim (format v3: `acquire_us`/`submit_us`, superseding v2's single
/// combined `present_us` — see that module's `format_raw_frame_line` doc)
/// so tooling maps this payload onto the same field set 1:1, with no
/// renaming step between the log-line parser and the wire type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameStats {
    pub n: u64,
    pub total_us: u64,
    pub rebuild_us: u64,
    pub layout_us: u64,
    pub paint_us: u64,
    pub encode_us: u64,
    pub acquire_us: u64,
    pub submit_us: u64,
    pub skipped: bool,
}

/// `metrics_snapshot` result — v1 minimal (process RSS is best-effort/
/// platform-dependent, hence optional; uptime is always known).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    pub uptime_ms: u64,
}

/// `input_tap` request params (logical px, see [`RectPx`]'s doc).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InputTapParams {
    pub x: f64,
    pub y: f64,
}

/// `input_scroll` request params (logical px; `dx`/`dy` are the scroll
/// delta, same convention as a wheel/drag event elsewhere in the
/// framework).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InputScrollParams {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
}

/// `input_text` request params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputTextParams {
    pub text: String,
}

/// The ack result `input_tap`/`input_scroll`/`input_text`/
/// `frame_stats_subscribe` all reply with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckResult {
    #[serde(default = "default_true")]
    pub ok: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AckResult {
    fn default() -> Self {
        Self { ok: true }
    }
}

/// `screenshot` result. Declared in the protocol so the method/result shape
/// exists for every client, but capability-gated in practice — a server
/// with no [`Capability::Screenshot`] rejects the request with
/// [`crate::RpcError::not_supported`] instead of ever returning this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub png_base64: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_round_trips() {
        for cap in [
            Capability::WidgetTree,
            Capability::FrameStats,
            Capability::Input,
            Capability::Metrics,
            Capability::Screenshot,
        ] {
            let json = serde_json::to_string(&cap).unwrap();
            let back: Capability = serde_json::from_str(&json).unwrap();
            assert_eq!(cap, back);
        }
    }

    #[test]
    fn unknown_capability_deserializes_to_unknown_variant() {
        let cap: Capability = serde_json::from_str("\"some_future_capability\"").unwrap();
        assert_eq!(cap, Capability::Unknown);
    }

    #[test]
    fn ack_result_defaults_ok_true_when_field_omitted() {
        let ack: AckResult = serde_json::from_str("{}").unwrap();
        assert_eq!(ack, AckResult { ok: true });
        assert_eq!(AckResult::default(), AckResult { ok: true });
    }

    #[test]
    fn frame_stats_field_names_match_perf_raw_line() {
        let stats = FrameStats {
            n: 42,
            total_us: 1_000,
            rebuild_us: 100,
            layout_us: 200,
            paint_us: 300,
            encode_us: 150,
            acquire_us: 50,
            submit_us: 200,
            skipped: false,
        };
        let json = serde_json::to_value(stats).unwrap();
        for key in [
            "n",
            "total_us",
            "rebuild_us",
            "layout_us",
            "paint_us",
            "encode_us",
            "acquire_us",
            "submit_us",
            "skipped",
        ] {
            assert!(json.get(key).is_some(), "missing field: {key}");
        }
    }
}
