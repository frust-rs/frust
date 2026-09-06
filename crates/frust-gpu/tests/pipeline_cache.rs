//! Framing/validation tests for the persisted `wgpu::PipelineCache` blob.
//!
//! These ride the public `frust_gpu::pipeline_cache` surface (no device
//! needed). They are the whole of the framing's coverage: `frust-render` used
//! to run an identical body against its own copy of the module, and now
//! re-exports this one instead.

use frust_gpu::pipeline_cache::{adapter_cache_key, frame, unframe};

const KEY: &str = "Vulkan|Adreno 740|turnip|1.2.3|0x5143|0x43050a01";

/// Byte offset of the `payload_len` field: `MAGIC (8)` · `key_len: u32 LE` ·
/// `key bytes`. Used by the lying-length test, which has to corrupt a field
/// the public API deliberately offers no way to set.
fn payload_len_offset(key: &str) -> usize {
    8 + 4 + key.len()
}

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
    // Corrupt a well-formed frame's declared payload length so it overruns
    // the buffer.
    let mut framed = frame(KEY, b"short");
    let at = payload_len_offset(KEY);
    framed[at..at + 4].copy_from_slice(&999u32.to_le_bytes());
    assert_eq!(unframe(&framed, KEY), None);
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
        // `Option<bool>` since wgpu 30 (`None` = the adapter does not say).
        transient_saves_memory: Some(false),
        // wgpu 30 reports the applied limit bucket here; frust never requests
        // bucketing, and `adapter_cache_key` reads none of these fields.
        limit_bucket: None,
    };
    let key_a = adapter_cache_key(&info);
    info.driver_info = "1.2.4".into();
    let key_b = adapter_cache_key(&info);
    assert_ne!(key_a, key_b, "a driver update must change the cache key");
}

#[test]
fn frame_layout_is_byte_stable() {
    // Every frust build that ever persisted a blob wrote this layout. Pin the
    // exact bytes: changing them silently invalidates every persisted blob on
    // disk and must be a deliberate magic-tag bump, not an accident.
    let framed = frame("ab", b"cd");
    assert_eq!(
        framed,
        b"FKPLCwg1\x02\x00\x00\x00ab\x02\x00\x00\x00cd".to_vec()
    );
}
