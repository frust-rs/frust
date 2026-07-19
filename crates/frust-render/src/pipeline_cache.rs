//! Framing + validation for a persisted `wgpu::PipelineCache` blob.
//!
//! A `wgpu::PipelineCache` lets a driver reuse the machine code it compiled for
//! vello's shader pipelines across process launches, cutting warm-start
//! shader/pipeline compilation to near zero. wgpu only implements it on Vulkan
//! (Android); Metal/desktop drivers manage their own caches, so this whole path
//! is silently inert there (the feature is absent → no cache is ever created).
//!
//! The shell owns file I/O and hands the framework an opaque `Vec<u8>` blob (no
//! `serde`/file dependency lives here — see the task notes). Before that blob is
//! ever fed to the **unsafe** [`wgpu::Device::create_pipeline_cache`], it is
//! wrapped in a small self-describing header — a magic tag plus the adapter
//! fingerprint the data was produced on — checked here (`unframe`). A mismatch
//! (wrong magic, different adapter/driver, truncated blob) is treated as "no
//! cache" and the renderer starts from an empty cache instead of handing the
//! driver bytes from a foreign device. This is belt-and-braces on top of wgpu's
//! own `create_pipeline_cache(fallback: true)`, which already falls back to an
//! empty cache for mismatched data rather than misbehaving — see
//! [`crate::context::RenderContext::create_pipeline_cache`], the sole unsafe
//! call site.

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
pub(crate) fn adapter_cache_key(info: &wgpu::AdapterInfo) -> String {
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
pub(crate) fn frame(adapter_key: &str, payload: &[u8]) -> Vec<u8> {
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
pub(crate) fn unframe<'a>(blob: &'a [u8], adapter_key: &str) -> Option<&'a [u8]> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "Vulkan|Adreno 740|turnip|1.2.3|0x5143|0x43050a01";

    #[test]
    fn frame_unframe_round_trip() {
        let payload = b"vulkan pipeline blob bytes";
        let framed = frame(KEY, payload);
        assert_eq!(unframe(&framed, KEY), Some(&payload[..]));
    }

    #[test]
    fn empty_payload_round_trips() {
        let framed = frame(KEY, b"");
        assert_eq!(unframe(&framed, KEY), Some(&b""[..]));
    }

    #[test]
    fn wrong_adapter_key_is_rejected() {
        let framed = frame(KEY, b"blob");
        assert_eq!(unframe(&framed, "some other adapter"), None);
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut framed = frame(KEY, b"blob");
        framed[0] = b'X';
        assert_eq!(unframe(&framed, KEY), None);
    }

    #[test]
    fn truncated_blob_is_rejected() {
        let framed = frame(KEY, b"blob");
        for cut in 0..framed.len() {
            assert_eq!(
                unframe(&framed[..cut], KEY),
                None,
                "prefix of len {cut} must be rejected"
            );
        }
    }

    #[test]
    fn trailing_garbage_is_rejected() {
        let mut framed = frame(KEY, b"blob");
        framed.push(0xff);
        assert_eq!(unframe(&framed, KEY), None);
    }

    #[test]
    fn empty_blob_is_rejected() {
        assert_eq!(unframe(&[], KEY), None);
    }

    #[test]
    fn lying_payload_length_is_rejected() {
        // Hand-build a frame whose declared payload length overruns the buffer.
        let key = KEY.as_bytes();
        let mut blob = Vec::new();
        blob.extend_from_slice(&MAGIC);
        blob.extend_from_slice(&(key.len() as u32).to_le_bytes());
        blob.extend_from_slice(key);
        blob.extend_from_slice(&999u32.to_le_bytes());
        blob.extend_from_slice(b"short");
        assert_eq!(unframe(&blob, KEY), None);
    }

    #[test]
    fn adapter_key_includes_driver_info() {
        let mut info = wgpu::AdapterInfo {
            name: "Adreno 740".into(),
            vendor: 0x5143,
            device: 0x4305_0a01,
            device_type: wgpu::DeviceType::IntegratedGpu,
            device_pci_bus_id: String::new(),
            driver: "turnip".into(),
            driver_info: "1.2.3".into(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 64,
            subgroup_max_size: 128,
            transient_saves_memory: false,
        };
        let key_a = adapter_cache_key(&info);
        info.driver_info = "1.2.4".into();
        let key_b = adapter_cache_key(&info);
        assert_ne!(key_a, key_b, "a driver update must change the cache key");
    }
}
