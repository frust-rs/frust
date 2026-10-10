//! Typed request-param / result / notification-payload structs for each v1
//! [`crate::Method`]. A [`Request::params`](crate::Request)/
//! [`Response`](crate::Response) `result` field is transport-generic
//! (`serde_json::Value`); a caller matches on [`crate::Method`] and
//! `serde_json::from_value`/`to_value`s the type below that pairs with it —
//! that pairing is a documented convention here, not something the type
//! system enforces, since the envelope has to stay method-agnostic.
//!
//! # The hot-patch jump table travels as chunks
//!
//! A patch's jump table can map hundreds of thousands of symbols — several MB,
//! far past the 1 MiB request line both ends cap. So no line carries it: the
//! host uploads [`JumpTableWire::encode_map`]'s bytes as `table_chunk`s
//! ([`PatchChunkParams`], the same shape and 512 KiB raw bound as
//! `patch_chunk`) under the patch's `patch_id`, on every path including the
//! loopback file hand-off, and [`ApplyPatchParams`] names only their length
//! ([`ApplyPatchParams::table_len`]) next to the table's three scalars. The app
//! reassembles the stream per connection, bounds it, decodes it with
//! [`JumpTableWire::decode_map`] and hands its backend the complete table.
//!
//! [`crate::PROTOCOL_VERSION`] stays 1 across this change: a hot-patching host
//! and app always ship from the same frust build (the app is built by the very
//! `frust` that drives it), and the change is additive on the app side — a
//! pre-chunk host's inline `table.map` (with no `table_len`, which defaults to
//! 0) still decodes and is applied as sent. This crate never writes one.

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
    /// 4. patch bytes arrive over the authenticated channel — or, only on a
    ///    server that advertises [`HotpatchInfo::patch_file_hand_off`], as
    ///    the one path the wire may carry ([`ApplyPatchParams::file`]),
    ///    accepted only under [`PatchFile`]'s five checks;
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
    /// The app accepts [`ApplyPatchParams::file`] (a loopback hand-off by
    /// file) in place of `patch_chunk` uploads. Absent from an older app's
    /// answer, so it defaults to `false` and the client uploads chunks.
    #[serde(default)]
    pub patch_file_hand_off: bool,
}

/// `patch_chunk` and `table_chunk` params; answered by [`AckResult`]. At most
/// 512 KiB of raw bytes per chunk (the base64 text stays under the 1 MiB line
/// cap). A `table_chunk` carries a slice of `patch_id`'s encoded jump table
/// ([`JumpTableWire::encode_map`]), reassembled apart from the patch bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchChunkParams {
    pub patch_id: u64,
    pub offset: u64,
    pub total_len: u64,
    pub data_base64: String,
}

/// The jump table of an `apply_patch`, mirroring `frust-hotpatch`'s
/// `JumpTable` minus `lib`: the only filesystem path on the wire is
/// [`ApplyPatchParams::file`]'s, so a `lib` field (or any other unknown
/// field) is rejected at decode.
///
/// On an `apply_patch` line it is the three scalars only
/// ([`ApplyPatchParams::table`]): `map` travels as the `table_chunk` stream in
/// [`Self::encode_map`]'s encoding, and the app's service fills it from the
/// reassembled stream before its backend sees the params (see the module doc).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JumpTableWire {
    pub map: std::collections::HashMap<u64, u64>,
    pub aslr_reference: u64,
    pub new_base_address: u64,
    pub ifunc_count: u64,
}

impl JumpTableWire {
    /// Bytes per encoded entry: the base address, then the patch address, each
    /// a little-endian `u64`.
    pub const ENTRY_BYTES: usize = 16;

    /// The length of [`Self::encode_map`]'s output, without encoding.
    pub fn encoded_map_len(&self) -> u64 {
        (self.map.len() as u64).saturating_mul(Self::ENTRY_BYTES as u64)
    }

    /// `map` as the `table_chunk` stream carries it: one [`Self::ENTRY_BYTES`]
    /// entry per mapping, in ascending base-address order (so one table always
    /// encodes to the same bytes, and a repeated key cannot be encoded).
    pub fn encode_map(&self) -> Vec<u8> {
        let mut entries: Vec<(u64, u64)> = self.map.iter().map(|(k, v)| (*k, *v)).collect();
        entries.sort_unstable();
        let mut out = Vec::with_capacity(entries.len() * Self::ENTRY_BYTES);
        for (base, patch) in entries {
            out.extend_from_slice(&base.to_le_bytes());
            out.extend_from_slice(&patch.to_le_bytes());
        }
        out
    }

    /// Decodes [`Self::encode_map`]'s bytes back into the map. Refuses a length
    /// that is not a whole number of entries and base addresses that are not
    /// strictly ascending (a repeated or reordered key means the stream was not
    /// produced by `encode_map`). The message names what was wrong.
    pub fn decode_map(bytes: &[u8]) -> Result<std::collections::HashMap<u64, u64>, String> {
        if !bytes.len().is_multiple_of(Self::ENTRY_BYTES) {
            return Err(format!(
                "a jump table of {} bytes is not a whole number of {}-byte entries",
                bytes.len(),
                Self::ENTRY_BYTES
            ));
        }
        let mut map = std::collections::HashMap::with_capacity(bytes.len() / Self::ENTRY_BYTES);
        let mut previous: Option<u64> = None;
        // Whole entries only (checked above): pairs of little-endian words.
        let words = bytes.as_chunks::<8>().0;
        for &[base, patch] in words.as_chunks::<2>().0 {
            let (base, patch) = (u64::from_le_bytes(base), u64::from_le_bytes(patch));
            if previous.is_some_and(|previous| base <= previous) {
                return Err(format!(
                    "jump table base address {base:#x} is not above the one before it"
                ));
            }
            previous = Some(base);
            map.insert(base, patch);
        }
        Ok(map)
    }
}

/// `apply_patch` params — applies the bytes previously sent as `patch_id`
/// chunks, or, when [`Self::file`] is set, the bytes of that host-written
/// file. `len` is the patch length either way (a named file's size must equal
/// it). Unknown fields (any other path included) are rejected.
///
/// The jump table's map is the `table_chunk` stream uploaded for `patch_id` on
/// the same connection, `table_len` bytes long, on the chunk and the file path
/// alike; `table` carries only its scalars on the wire (see the module doc).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchParams {
    pub patch_id: u64,
    pub len: u64,
    pub pid: u32,
    pub anchor_runtime: u64,
    /// The table's scalars. Its `map` is never written here: the map is the
    /// `table_chunk` stream, and this field decodes with an empty one — except
    /// from a pre-chunk host, whose inline `map` still decodes (see the module
    /// doc) and is valid only with `table_len` 0.
    #[serde(with = "table_scalars")]
    pub table: JumpTableWire,
    /// Length of the encoded map uploaded as `table_chunk`s for `patch_id`
    /// ([`JumpTableWire::encoded_map_len`]); `0` for an empty map, with no
    /// `table_chunk` sent, and the default for a pre-chunk host.
    #[serde(default)]
    pub table_len: u64,
    pub expected_seams: u32,
    /// The loopback hand-off: the patch the host already wrote, named in place
    /// of `patch_chunk` uploads. The **only** filesystem path any wire type
    /// carries, sent only to an app advertising
    /// [`HotpatchInfo::patch_file_hand_off`] and accepted only under
    /// [`PatchFile`]'s checks. Omitted when `None`, so a chunk-path request is
    /// byte-for-byte what an older app decodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PatchFile>,
}

/// [`ApplyPatchParams::table`]'s wire form: [`JumpTableWire`] without its map
/// when written; read with a pre-chunk host's optional inline map.
mod table_scalars {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::JumpTableWire;

    /// What is written: the scalars only.
    #[derive(Serialize)]
    struct Written {
        aslr_reference: u64,
        new_base_address: u64,
        ifunc_count: u64,
    }

    /// What is read: the scalars and a pre-chunk host's inline map, refusing
    /// any other field (a `lib` included).
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Read {
        #[serde(default)]
        map: std::collections::HashMap<u64, u64>,
        aslr_reference: u64,
        new_base_address: u64,
        ifunc_count: u64,
    }

    pub(super) fn serialize<S: Serializer>(
        table: &JumpTableWire,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Written {
            aslr_reference: table.aslr_reference,
            new_base_address: table.new_base_address,
            ifunc_count: table.ifunc_count,
        }
        .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<JumpTableWire, D::Error> {
        let read = Read::deserialize(deserializer)?;
        Ok(JumpTableWire {
            map: read.map,
            aslr_reference: read.aslr_reference,
            new_base_address: read.new_base_address,
            ifunc_count: read.ifunc_count,
        })
    }
}

/// A patch handed off by file on a loopback session ([`ApplyPatchParams::file`]).
///
/// The app accepts it only when all five checks hold: the path opens with
/// `O_NOFOLLOW` (no symlink) and the open file is a regular file; it is owned
/// by the app's effective uid; its mode grants nothing to group or other
/// (`mode & 0o077 == 0`); its size equals [`ApplyPatchParams::len`]; and the
/// SHA-256 of the bytes read equals `sha256`. The app never logs `path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchFile {
    /// Absolute path of the host-written patch.
    pub path: String,
    /// SHA-256 of the patch bytes: 64 lowercase hex characters.
    pub sha256: String,
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
            patch_file_hand_off: true,
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
    fn a_pre_chunk_inline_table_map_still_decodes_with_no_table_len() {
        // The pre-chunk shape: the whole map on the `apply_patch` line.
        let json = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,
            "table":{"map":{"4096":8192,"16":32},"aslr_reference":1,
            "new_base_address":2,"ifunc_count":0},"expected_seams":2}"#;
        let p: ApplyPatchParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.table.map.get(&4096), Some(&8192));
        assert_eq!(p.table.map.get(&16), Some(&32));
        assert_eq!(p.table_len, 0);
        // ...and is never written back.
        assert!(!serde_json::to_string(&p).unwrap().contains("\"map\""));
    }

    #[test]
    fn apply_patch_params_never_put_the_map_on_the_line() {
        let mut params = sample_params(None);
        params.table.map = (0..1000u64).map(|i| (i * 16, i * 32)).collect();
        params.table_len = params.table.encoded_map_len();
        let j = serde_json::to_string(&params).unwrap();
        assert!(!j.contains("\"map\""), "{j}");
        assert!(j.contains("\"table_len\":16000"), "{j}");
        assert!(j.len() < 300, "the line stays small: {} bytes", j.len());
        let back: ApplyPatchParams = serde_json::from_str(&j).unwrap();
        assert!(back.table.map.is_empty());
        assert_eq!(back.table_len, 16_000);
        assert_eq!(back.table.aslr_reference, params.table.aslr_reference);
    }

    #[test]
    fn a_standalone_jump_table_still_serializes_its_map() {
        // Only `ApplyPatchParams::table` drops the map; the type itself keeps
        // its full shape for any other use.
        let table = JumpTableWire {
            map: [(1, 2)].into_iter().collect(),
            aslr_reference: 0,
            new_base_address: 0,
            ifunc_count: 0,
        };
        let j = serde_json::to_string(&table).unwrap();
        assert!(j.contains("\"map\":{\"1\":2}"), "{j}");
    }

    #[test]
    fn a_jump_table_map_round_trips_through_its_encoding() {
        let table = JumpTableWire {
            map: [(0x4000, 0x10), (0x10, 0x4000), (u64::MAX, 0), (0, u64::MAX)]
                .into_iter()
                .collect(),
            aslr_reference: 1,
            new_base_address: 2,
            ifunc_count: 0,
        };
        let bytes = table.encode_map();
        assert_eq!(bytes.len() as u64, table.encoded_map_len());
        assert_eq!(bytes.len(), 4 * JumpTableWire::ENTRY_BYTES);
        // Ascending keys, little-endian: the first entry is (0, u64::MAX).
        assert_eq!(&bytes[..8], &[0; 8]);
        assert_eq!(&bytes[8..16], &[0xff; 8]);
        assert_eq!(JumpTableWire::decode_map(&bytes).unwrap(), table.map);
        assert_eq!(JumpTableWire::decode_map(&[]).unwrap(), Default::default());
    }

    #[test]
    fn a_jump_table_encoding_that_is_cut_or_out_of_order_is_refused() {
        let table = JumpTableWire {
            map: [(1, 2), (3, 4)].into_iter().collect(),
            aslr_reference: 0,
            new_base_address: 0,
            ifunc_count: 0,
        };
        let bytes = table.encode_map();
        let cut = JumpTableWire::decode_map(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(cut.contains("whole number"), "{cut}");

        let mut swapped = bytes[16..].to_vec();
        swapped.extend_from_slice(&bytes[..16]);
        let swapped = JumpTableWire::decode_map(&swapped).unwrap_err();
        assert!(swapped.contains("not above"), "{swapped}");

        let mut repeated = bytes[..16].to_vec();
        repeated.extend_from_slice(&bytes[..16]);
        assert!(JumpTableWire::decode_map(&repeated).is_err());
    }

    #[test]
    fn hotpatch_info_without_the_hand_off_flag_defaults_to_chunks() {
        let json = r#"{"anchor_runtime":1,"pid":2,"triple":"t","patches_applied":0,
            "patch_bytes_loaded":0,"pending_layout_mismatches":[]}"#;
        let info: HotpatchInfo = serde_json::from_str(json).unwrap();
        assert!(!info.patch_file_hand_off);
    }

    fn sample_params(file: Option<PatchFile>) -> ApplyPatchParams {
        ApplyPatchParams {
            patch_id: 3,
            len: 4096,
            pid: 77,
            anchor_runtime: 0x1000,
            table: JumpTableWire {
                // The map never travels in JSON, so a round trip compares
                // equal only without one.
                map: Default::default(),
                aslr_reference: 1,
                new_base_address: 2,
                ifunc_count: 0,
            },
            table_len: 16,
            expected_seams: 1,
            file,
        }
    }

    #[test]
    fn apply_patch_params_round_trip_with_and_without_a_file() {
        let chunked = sample_params(None);
        let j = serde_json::to_string(&chunked).unwrap();
        // The chunk-path shape is unchanged on the wire: no `file` key at all.
        assert!(!j.contains("\"file\""), "{j}");
        assert_eq!(
            serde_json::from_str::<ApplyPatchParams>(&j).unwrap(),
            chunked
        );

        let named = sample_params(Some(PatchFile {
            path: "/tmp/frust-hotpatch/session-x/patch-1.dylib".into(),
            sha256: "ab".repeat(32),
        }));
        let j = serde_json::to_string(&named).unwrap();
        assert!(j.contains("\"file\":{\"path\""), "{j}");
        assert_eq!(serde_json::from_str::<ApplyPatchParams>(&j).unwrap(), named);
    }

    #[test]
    fn apply_patch_params_without_file_default_to_none() {
        let json = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,
            "table":{"aslr_reference":1,"new_base_address":2,"ifunc_count":0},"table_len":0,
            "expected_seams":2}"#;
        let p: ApplyPatchParams = serde_json::from_str(json).unwrap();
        assert_eq!(p.file, None);
    }

    #[test]
    fn patch_file_rejects_unknown_fields() {
        let json = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,
            "table":{"aslr_reference":1,"new_base_address":2,"ifunc_count":0},"table_len":0,
            "expected_seams":2,"file":{"path":"/x","sha256":"00","lib":"/y"}}"#;
        assert!(serde_json::from_str::<ApplyPatchParams>(json).is_err());
    }

    #[test]
    fn apply_patch_rejects_lib_and_path_fields() {
        let with_lib = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,
            "table":{"lib":"/tmp/evil.dylib","aslr_reference":1,
            "new_base_address":2,"ifunc_count":0},"table_len":0,"expected_seams":2}"#;
        assert!(serde_json::from_str::<ApplyPatchParams>(with_lib).is_err());
        let with_path = r#"{"patch_id":1,"len":10,"pid":5,"anchor_runtime":99,"path":"/x",
            "table":{"aslr_reference":1,"new_base_address":2,"ifunc_count":0},"table_len":0,
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
