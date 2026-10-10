//! The wire leg: sends the patch edit 1 linked through the real devtools
//! transport, where the in-process apply never goes.
//!
//! A `frust_devtools` service runs in this driver with a checking backend, and
//! a `frust_drive::devtools_client::DevtoolsClient` uploads the patch bytes
//! (`patch_chunk`), the jump table (`table_chunk`) and the `apply_patch` that
//! names them, over a loopback socket under the service's own token, hot-patch
//! gate and 1 MiB line cap. The backend does not load anything: it compares
//! what the service reassembled with what was sent, byte for byte. The leg
//! passes only when the whole table crossed the wire intact.
//!
//! The table sent is the linked patch's real table padded with synthetic
//! entries to [`WIRE_ENTRIES`]. A fixture whose real table is that large would
//! need over a hundred thousand functions: measured at 3 min 40 s of build per
//! compile for 131072 generic instances, against a canary of seconds, so the
//! size comes from padding and the real entries stay in it. Padding addresses
//! lie above every real one and are never applied.
//!
//! This driver depends on both `frust-devtools` and `frust-drive`, which the
//! root workspace's tooling-isolation charter forbids for one of its crates.
//! The canary is a standalone workspace outside that graph, so the rule does
//! not bind it: it is the one place that can stand both ends of the wire up.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use frust_devtools::{AppInfo, BackendError, DevtoolsBackend, Service};
use frust_devtools_protocol::{
    ApplyPatchParams, Capability, HandshakeInfo, HotpatchInfo, InputScrollParams, InputTapParams,
    JumpTableWire, MetricsSnapshot, PROTOCOL_VERSION, PatchChunkParams, PatchOutcome, WidgetProps,
    WidgetTreeDump,
};
use frust_drive::devtools_client::DevtoolsClient;
use frust_hotpatch::JumpTable;

/// Entries in the table the wire leg sends: 2 MiB encoded at
/// [`JumpTableWire::ENTRY_BYTES`], twice the 1 MiB line cap the table once
/// overflowed.
pub const WIRE_ENTRIES: usize = 131_072;

/// What the checking backend must see arrive.
struct Expected {
    pid: u32,
    anchor_runtime: u64,
    patch: Vec<u8>,
    table: JumpTableWire,
}

/// The app side of the wire: answers `apply_patch` only when the reassembled
/// patch and table equal [`Expected`].
struct CheckingBackend {
    expected: Arc<Mutex<Option<Expected>>>,
    applied: u32,
}

impl DevtoolsBackend for CheckingBackend {
    fn handshake_info(&self, app: &AppInfo) -> HandshakeInfo {
        HandshakeInfo {
            app_name: app.app_name.clone(),
            frust_version: app.frust_version.clone(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![Capability::HotPatch],
        }
    }

    fn widget_tree(&self) -> WidgetTreeDump {
        WidgetTreeDump { roots: Vec::new() }
    }

    fn widget_props(&self, _id: u64) -> Option<WidgetProps> {
        None
    }

    fn metrics_snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            rss_bytes: None,
            uptime_ms: 0,
        }
    }

    fn inject_tap(&self, _params: InputTapParams) -> Result<(), BackendError> {
        Err(BackendError::not_supported("the wire leg injects no input"))
    }

    fn inject_scroll(&self, _params: InputScrollParams) -> Result<(), BackendError> {
        Err(BackendError::not_supported("the wire leg injects no input"))
    }

    fn inject_text(&self, _text: &str) -> Result<(), BackendError> {
        Err(BackendError::not_supported("the wire leg injects no input"))
    }

    fn hotpatch_info(&self) -> Result<HotpatchInfo, BackendError> {
        let guard = lock(&self.expected);
        let expected = guard
            .as_ref()
            .ok_or_else(|| BackendError::unavailable("no patch is expected yet"))?;
        Ok(HotpatchInfo {
            anchor_runtime: expected.anchor_runtime,
            pid: expected.pid,
            triple: String::new(),
            patches_applied: self.applied,
            patch_bytes_loaded: 0,
            pending_layout_mismatches: Vec::new(),
            patch_file_hand_off: false,
        })
    }

    fn patch_chunk(&self, chunk: &PatchChunkParams) -> Result<Vec<u8>, BackendError> {
        decode_base64(&chunk.data_base64).map_err(BackendError::invalid_request)
    }

    fn apply_patch(
        &self,
        bytes: Vec<u8>,
        params: ApplyPatchParams,
    ) -> Result<PatchOutcome, BackendError> {
        let guard = lock(&self.expected);
        let expected = guard
            .as_ref()
            .ok_or_else(|| BackendError::unavailable("no patch is expected yet"))?;
        if params.pid != expected.pid || params.anchor_runtime != expected.anchor_runtime {
            return Err(BackendError::invalid_request(
                "the patch names another process",
            ));
        }
        if bytes != expected.patch {
            return Err(BackendError::invalid_request(format!(
                "the reassembled patch ({} bytes) differs from the one sent ({} bytes)",
                bytes.len(),
                expected.patch.len()
            )));
        }
        if params.table != expected.table {
            return Err(BackendError::invalid_request(format!(
                "the reassembled jump table ({} entries) differs from the one sent ({} entries)",
                params.table.map.len(),
                expected.table.map.len()
            )));
        }
        Ok(PatchOutcome {
            applied: true,
            seam_hits: 0,
            seam_fall_throughs: Vec::new(),
            layout_mismatches: Vec::new(),
            patches_applied: self.applied + 1,
            patch_bytes_loaded: bytes.len() as u64,
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The devtools service the wire leg sends to, running in this process.
pub struct WireService {
    handle: frust_devtools::ServiceHandle,
    expected: Arc<Mutex<Option<Expected>>>,
}

impl WireService {
    /// Starts the service on a loopback port with a checking backend.
    pub fn start() -> Result<Self> {
        let expected = Arc::new(Mutex::new(None));
        let backend = CheckingBackend {
            expected: Arc::clone(&expected),
            applied: 0,
        };
        let handle = Service::start(backend, AppInfo::new("hotpatch-canary", "canary"))
            .context("starting the devtools service")?;
        Ok(Self { handle, expected })
    }

    /// Sends `patch` and its jump table to the service through a client and
    /// returns the table's `(entries, encoded bytes)` once the service
    /// reports `applied`.
    pub fn send(
        &self,
        patch_id: u64,
        pid: u32,
        anchor_runtime: u64,
        patch: Vec<u8>,
        table: &JumpTable,
    ) -> Result<(usize, u64)> {
        let wire = padded_wire_table(table);
        let entries = wire.map.len();
        *lock(&self.expected) = Some(Expected {
            pid,
            anchor_runtime,
            patch: patch.clone(),
            table: wire.clone(),
        });

        let token = self
            .handle
            .token()
            .ok_or_else(|| anyhow!("the devtools service runs without a token"))?;
        let client = DevtoolsClient::connect(
            ("127.0.0.1", self.handle.port()),
            Duration::from_secs(30),
            Some(token),
        )?;
        let info = client.handshake()?;
        if !info.capabilities.contains(&Capability::HotPatch) {
            bail!(
                "the service offers no hot patching ({:?}); the canary must be a debug build",
                info.capabilities
            );
        }

        let started = Instant::now();
        client.upload_patch(patch_id, &patch)?;
        let table_len = client.upload_table(patch_id, &wire)?;
        let uploaded = started.elapsed();
        let outcome = client.apply_patch(&ApplyPatchParams {
            patch_id,
            len: patch.len() as u64,
            pid,
            anchor_runtime,
            table: JumpTableWire {
                map: HashMap::new(),
                ..wire
            },
            table_len,
            expected_seams: 0,
            file: None,
        })?;
        if !outcome.applied {
            bail!("the service did not apply the patch: {outcome:?}");
        }
        println!(
            "canary: timing: wire: uploaded {} patch bytes and {table_len} table bytes in {} ms, \
             applied in {} ms",
            patch.len(),
            uploaded.as_millis(),
            started.elapsed().as_millis()
        );
        Ok((entries, table_len))
    }

    /// Stops the service.
    pub fn stop(self) {
        self.handle.shutdown();
    }
}

/// The linked patch's table with synthetic entries added up to
/// [`WIRE_ENTRIES`]. A padding entry's two addresses lie above every real one
/// and 16 apart, so none collides with a real or another padding entry.
fn padded_wire_table(table: &JumpTable) -> JumpTableWire {
    let mut map: HashMap<u64, u64> = table.map.iter().map(|(k, v)| (*k, *v)).collect();
    let mut next = map
        .keys()
        .chain(map.values())
        .copied()
        .max()
        .unwrap_or(0)
        .saturating_add(0x1000)
        & !0xf;
    while map.len() < WIRE_ENTRIES {
        map.insert(next, next + 0x10_0000_0000);
        next += 16;
    }
    JumpTableWire {
        map,
        aslr_reference: table.aslr_reference,
        new_base_address: table.new_base_address,
        ifunc_count: table.ifunc_count,
    }
}

/// Standard, padded base64 (RFC 4648 section 4), the encoding `patch_chunk`
/// carries; the transfer encoding is the backend's to decode.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let digits = |byte: u8| -> Result<u32, String> {
        match byte {
            b'A'..=b'Z' => Ok(u32::from(byte - b'A')),
            b'a'..=b'z' => Ok(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Ok(u32::from(byte - b'0') + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            other => Err(format!("`{}` is not a base64 digit", other as char)),
        }
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("base64 text is not a whole number of quads".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for quad in bytes.chunks(4) {
        let pad = quad.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 {
            return Err("base64 quad has too much padding".into());
        }
        let mut word = 0u32;
        for byte in &quad[..4 - pad] {
            word = (word << 6) | digits(*byte)?;
        }
        word <<= 6 * pad as u32;
        out.extend_from_slice(&word.to_be_bytes()[1..4 - pad]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_every_padding_length() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
        assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");
        assert_eq!(decode_base64("Zm9v").unwrap(), b"foo");
        assert!(decode_base64("Zm9").is_err());
        assert!(decode_base64("Zm9!").is_err());
    }

    #[test]
    fn the_padded_table_keeps_every_real_entry_and_reaches_the_wire_size() {
        let mut real = JumpTable {
            lib: std::path::PathBuf::new(),
            map: Default::default(),
            aslr_reference: 1,
            new_base_address: 2,
            ifunc_count: 0,
        };
        real.map.insert(0x1_0000_3f00, 0x2_0000_0100);
        let wire = padded_wire_table(&real);
        assert_eq!(wire.map.len(), WIRE_ENTRIES);
        assert_eq!(wire.map[&0x1_0000_3f00], 0x2_0000_0100);
        assert!(wire.encoded_map_len() > 1 << 20);
        assert_eq!(
            JumpTableWire::decode_map(&wire.encode_map()).unwrap(),
            wire.map
        );
    }
}
