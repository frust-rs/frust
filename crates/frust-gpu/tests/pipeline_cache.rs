//! Framing/validation tests for the persisted `wgpu::PipelineCache` blob.
//!
//! These ride the public `frust_gpu::pipeline_cache` surface (no device
//! needed) and are the same body `frust-render` runs against its own copy of
//! the framing, so a divergence between the two shows up as a failure here.

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
        transient_saves_memory: false,
    };
    let key_a = adapter_cache_key(&info);
    info.driver_info = "1.2.4".into();
    let key_b = adapter_cache_key(&info);
    assert_ne!(key_a, key_b, "a driver update must change the cache key");
}

#[test]
fn frame_layout_is_byte_stable() {
    // The on-disk layout is shared with `frust-render`'s copy of this framing,
    // so a blob written by either crate validates in the other. Pin the exact
    // bytes: changing them silently invalidates every persisted blob and must
    // be a deliberate magic-tag bump in both crates, not an accident.
    let framed = frame("ab", b"cd");
    assert_eq!(
        framed,
        b"FKPLCwg1\x02\x00\x00\x00ab\x02\x00\x00\x00cd".to_vec()
    );
}

#[test]
fn pipeline_cache_drift_guard() {
    use std::fs;
    use std::path::Path;

    // Derive workspace root from CARGO_MANIFEST_DIR (frust-gpu crate directory).
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .expect("frust-gpu is a crate")
        .parent()
        .expect("crates is a directory");

    let gpu_cache = workspace_root.join("crates/frust-gpu/src/pipeline_cache.rs");
    let render_cache = workspace_root.join("crates/frust-render/src/pipeline_cache.rs");

    let gpu_content =
        fs::read_to_string(&gpu_cache).expect("unable to read frust-gpu/src/pipeline_cache.rs");
    let render_content = fs::read_to_string(&render_cache)
        .expect("unable to read frust-render/src/pipeline_cache.rs");

    // Normalize away the two copies' legitimate surface differences before
    // comparing: doc comments (each copy describes itself), the item
    // visibility (frust-gpu exports `pub` for its integration tests where
    // frust-render keeps `pub(crate)`), `#[must_use]` attributes, and each
    // file's `#[cfg(test)]` tail. Everything that remains is the framing
    // logic itself, which must never diverge between the copies.
    let normalize = |content: &str| -> String {
        content
            .lines()
            .take_while(|line| !line.trim_start().starts_with("#[cfg(test)]"))
            .filter(|line| {
                let t = line.trim_start();
                !t.starts_with("//!")
                    && !t.starts_with("//")
                    && !t.starts_with("#[must_use]")
                    && !t.is_empty()
            })
            .map(|line| {
                line.replace("pub(crate) fn ", "pub fn ")
                    .replace("pub(crate) const ", "pub const ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let gpu_stripped = normalize(&gpu_content);
    let render_stripped = normalize(&render_content);

    assert_eq!(
        gpu_stripped, render_stripped,
        "crates/frust-gpu/src/pipeline_cache.rs must keep its framing logic identical to \
         crates/frust-render/src/pipeline_cache.rs (compared with comments, `#[must_use]`, \
         item visibility, and the test module normalized away). \
         If the framing layout changes, bump MAGIC in both crates/frust-gpu/src/pipeline_cache.rs \
         and crates/frust-render/src/pipeline_cache.rs, then rebuild both crates together."
    );

    // Verify MAGIC constant is identical in both files.
    let extract_magic = |content: &str| -> String {
        content
            .lines()
            .find(|line| line.contains("const MAGIC"))
            .expect("MAGIC const not found")
            .to_string()
    };

    let gpu_magic = extract_magic(&gpu_content);
    let render_magic = extract_magic(&render_content);

    assert_eq!(
        gpu_magic, render_magic,
        "MAGIC constant must be identical in both crates"
    );
}
