//! Typed request-param / result / notification-payload structs for each v1
//! [`crate::Method`]. A [`Request::params`](crate::Request)/
//! [`Response`](crate::Response) `result` field is transport-generic
//! (`serde_json::Value`); a caller matches on [`crate::Method`] and
//! `serde_json::from_value`/`to_value`s the type below that pairs with it —
//! that pairing is a documented convention here, not something the type
//! system enforces, since the envelope has to stay method-agnostic.

use serde::{Deserialize, Serialize};

/// `handshake` request params — the per-process auth token the server printed
/// on its discovery line (`crate::format_discovery_line`).
///
/// **Inbound only.** The token travels client→server on this one method and
/// appears in no result, notification, or error payload the server ever
/// writes; a client that learned it from a log line must not echo it back
/// anywhere else.
///
/// `token` is optional so a client can still handshake against a server
/// running with auth switched off (and so a server can answer such a client's
/// `null` params rather than failing to decode them) — a server that requires
/// a token answers an absent one with [`crate::RpcError::UNAUTHORIZED`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

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
    /// The server can apply a hot patch (`hotpatch_info`, `patch_chunk`,
    /// `apply_patch`) — it executes code a client sends, so a server declares
    /// it only when all five preconditions hold:
    ///
    /// 1. debug builds only;
    /// 2. a per-session OS-CSPRNG token with `require_token` on;
    /// 3. a loopback-only listener;
    /// 4. patch bytes arrive over the authenticated channel, never as a path
    ///    taken from the wire;
    /// 5. the patch matches this process and this connection: `patch_id`
    ///    chunks arrived on the same connection, their length equals `len`,
    ///    and `pid` and `anchor_runtime` match.
    HotPatch,
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

/// `hotpatch_info` result — what a patch builder needs to target this
/// process and what the apply state currently is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotpatchInfo {
    pub anchor_runtime: u64,
    pub pid: u32,
    pub triple: String,
    pub patches_applied: u32,
    pub patch_bytes_loaded: u64,
    pub pending_layout_mismatches: Vec<String>,
}

/// `patch_chunk` params; answered by [`AckResult`]. At most 512 KiB of raw
/// bytes per chunk (the base64 text stays under the 1 MiB line cap).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchChunkParams {
    pub patch_id: u64,
    pub offset: u64,
    pub total_len: u64,
    pub data_base64: String,
}

/// The jump table of an `apply_patch`, mirroring `frust-hotpatch`'s
/// `JumpTable` minus `lib`: no wire type carries a filesystem path, so a
/// `lib` field (or any other unknown field) is rejected at decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JumpTableWire {
    pub map: std::collections::HashMap<u64, u64>,
    pub aslr_reference: u64,
    pub new_base_address: u64,
    pub ifunc_count: u64,
}

/// `apply_patch` params — applies the bytes previously sent as `patch_id`
/// chunks. Unknown fields (a path included) are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchParams {
    pub patch_id: u64,
    pub len: u64,
    pub pid: u32,
    pub anchor_runtime: u64,
    pub table: JumpTableWire,
    pub expected_seams: u32,
}

/// A seam key a patch missed: the image index and link-time address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissedKey {
    pub image: u32,
    pub link_address: u64,
}

/// `apply_patch` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchOutcome {
    pub applied: bool,
    pub seam_hits: u64,
    pub seam_fall_throughs: Vec<MissedKey>,
    pub layout_mismatches: Vec<String>,
    pub patches_applied: u32,
    pub patch_bytes_loaded: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotpatch_types_round_trip() {
        let info = HotpatchInfo {
            anchor_runtime: 0x1000,
            pid: 42,
            triple: "aarch64-apple-darwin".into(),
            patches_applied: 2,
            patch_bytes_loaded: 1_500_000,
            pending_layout_mismatches: vec!["Foo".into()],
        };
        let j = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<HotpatchInfo>(&j).unwrap(), info);

        let chunk = PatchChunkParams {
            patch_id: 7,
            offset: 524_288,
            total_len: 2_000_000,
            data_base64: "AAEC".into(),
        };
        let j = serde_json::to_string(&chunk).unwrap();
        assert_eq!(serde_json::from_str::<PatchChunkParams>(&j).unwrap(), chunk);

        let out = PatchOutcome {
            applied: true,
            seam_hits: 9,
            seam_fall_throughs: vec![MissedKey {
                image: 1,
                link_address: 0xdead,
            }],
            layout_mismatches: vec![],
            patches_applied: 3,
            patch_bytes_loaded: 10,
        };
        let j = serde_json::to_string(&out).unwrap();
        assert_eq!(serde_json::from_str::<PatchOutcome>(&j).unwrap(), out);
    }

    #[test]
    fn apply_patch_params_decode_string_keyed_map() {
        let json = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,
            "table":{"map":{"4096":8192,"16":32},"aslr_reference":1,
            "new_base_address":2,"ifunc_count":0},"expected_seams":2}"#;
        let p: ApplyPatchParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.table.map.get(&4096), Some(&8192));
        assert_eq!(p.table.map.get(&16), Some(&32));
        let back = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<ApplyPatchParams>(&back).unwrap(), p);
    }

    #[test]
    fn apply_patch_rejects_lib_and_path_fields() {
        let with_lib = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,
            "table":{"lib":"/tmp/evil.dylib","map":{},"aslr_reference":1,
            "new_base_address":2,"ifunc_count":0},"expected_seams":2}"#;
        assert!(serde_json::from_str::<ApplyPatchParams>(with_lib).is_err());
        let with_path = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,"path":"/x",
            "table":{"map":{},"aslr_reference":1,"new_base_address":2,"ifunc_count":0},
            "expected_seams":2}"#;
        assert!(serde_json::from_str::<ApplyPatchParams>(with_path).is_err());
    }

    #[test]
    fn hot_patch_capability_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&Capability::HotPatch).unwrap(),
            "\"hot_patch\""
        );
        assert_eq!(
            serde_json::from_str::<Capability>("\"hot_patch\"").unwrap(),
            Capability::HotPatch
        );
    }

    #[test]
    fn handshake_params_round_trip_with_and_without_a_token() {
        let with = HandshakeParams {
            token: Some("0123456789abcdef".to_string()),
        };
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(
            serde_json::from_str::<HandshakeParams>(&json).unwrap(),
            with
        );

        // Absent on the wire (an auth-off server, or a client that has no
        // token to present) must decode, not fail.
        let without: HandshakeParams = serde_json::from_str("{}").unwrap();
        assert_eq!(without, HandshakeParams::default());
        assert_eq!(without.token, None);
        assert!(!serde_json::to_string(&without).unwrap().contains("token"));
    }

    #[test]
    fn no_server_written_payload_carries_a_token_field() {
        // The token is inbound-only: `handshake`'s *result* must never echo
        // it back, or a log/transcript of the reply would leak the secret.
        let info = HandshakeInfo {
            app_name: "app".to_string(),
            frust_version: "0.1.0".to_string(),
            protocol_version: 1,
            capabilities: vec![Capability::WidgetTree],
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("token"));
    }

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
