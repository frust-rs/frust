//! Framing + validation for a persisted `wgpu::PipelineCache` blob.
//!
//! A `wgpu::PipelineCache` lets a driver reuse the machine code it compiled for
//! a device's render pipelines across process launches, cutting warm-start
//! shader/pipeline compilation to near zero. wgpu only implements it on Vulkan
//! (any Vulkan adapter — Android, Linux, Windows-on-Vulkan); Metal/DX12 drivers
//! manage their own caches, so this whole path is silently inert there (the
//! feature is absent → no cache is ever created, and
//! [`crate::pipeline::PipelineCache`] is simply built with `None`).
//!
//! The shell owns file I/O and hands the framework an opaque `Vec<u8>` blob (no
//! `serde`/file dependency lives here). Before that blob is ever fed to the
//! **unsafe** [`wgpu::Device::create_pipeline_cache`], it is wrapped in a small
//! self-describing header — a magic tag plus the adapter fingerprint the data
//! was produced on — checked here ([`unframe`]). A mismatch (wrong magic,
//! different adapter/driver, truncated blob) is treated as "no cache" and the
//! caller starts from an empty cache instead of handing the driver bytes from a
//! foreign device. This is belt-and-braces on top of wgpu's own
//! `create_pipeline_cache(fallback: true)`, which already falls back to an empty
//! cache for mismatched data rather than misbehaving.
//!
//! This is the **only** copy of the framing: `frust-render` used to carry an
//! identical one (kept in lockstep by a drift-guard test) and now re-exports
//! this module instead, so the on-disk layout has exactly one definition. A
//! blob persisted by any earlier frust build still validates here unchanged —
//! bump this module's `MAGIC` if the layout ever does change, so an old blob
//! is rejected
//! rather than misparsed.

/// Magic tag prefixing every framed blob: `Frust PipeLine Cache wgpu v1`.
/// Bumped if the framing layout below ever changes so an old on-disk blob is
/// rejected rather than misparsed.
const MAGIC: [u8; 8] = *b"FKPLCwg1";

/// A compact identity string for the adapter a cache blob was produced on.
///
/// A `wgpu::PipelineCache` is only ever valid for the same adapter+driver that
/// produced it (wgpu validates this internally too, but framing the key lets us
/// reject a foreign blob before the unsafe API is ever called). The `driver` /
/// `driver_info` fields are included so a driver update — which can invalidate
/// the compiled machine code — changes the key and discards the stale blob.
#[must_use]
pub fn adapter_cache_key(info: &wgpu::AdapterInfo) -> String {
    format!(
        "{:?}|{}|{}|{}|{:#x}|{:#x}",
        info.backend, info.name, info.driver, info.driver_info, info.vendor, info.device
    )
}

/// Wraps a raw `PipelineCache::get_data()` payload in the validated header
/// [`unframe`] checks, tagging it with the adapter it was produced on.
///
/// Layout: `MAGIC (8)` · `key_len: u32 LE` · `key bytes` · `payload_len: u32 LE`
/// · `payload bytes`.
#[must_use]
pub fn frame(adapter_key: &str, payload: &[u8]) -> Vec<u8> {
    let key = adapter_key.as_bytes();
    let mut out = Vec::with_capacity(MAGIC.len() + 8 + key.len() + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(key.len() as u32).to_le_bytes());
    out.extend_from_slice(key);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Validates a framed blob against `adapter_key` and returns the inner payload,
/// or `None` if the blob is not a well-formed frame for *this* adapter.
///
/// Rejects (returns `None`) on: a truncated/malformed frame, a wrong magic tag,
/// an adapter-key mismatch (foreign device or a post-driver-update key), or a
/// declared payload length that disagrees with the trailing bytes. A `None`
/// here means "start from an empty cache", never a panic.
#[must_use]
pub fn unframe<'a>(blob: &'a [u8], adapter_key: &str) -> Option<&'a [u8]> {
    let mut rest = blob;

    let magic = take(&mut rest, MAGIC.len())?;
    if magic != MAGIC {
        return None;
    }

    let key_len = take_u32(&mut rest)? as usize;
    let key = take(&mut rest, key_len)?;
    if key != adapter_key.as_bytes() {
        return None;
    }

    let payload_len = take_u32(&mut rest)? as usize;
    let payload = take(&mut rest, payload_len)?;
    // A trailing-byte surplus means the frame is malformed — reject it.
    if !rest.is_empty() {
        return None;
    }
    Some(payload)
}

/// Splits `n` bytes off the front of `*buf`, advancing it; `None` if short.
fn take<'a>(buf: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if buf.len() < n {
        return None;
    }
    let (head, tail) = buf.split_at(n);
    *buf = tail;
    Some(head)
}

/// Reads a little-endian `u32` off the front of `*buf`, advancing it.
fn take_u32(buf: &mut &[u8]) -> Option<u32> {
    let bytes = take(buf, 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
