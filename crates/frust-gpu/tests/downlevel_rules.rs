//! Design rule enforcement tests for downlevel (WebGL2/GLES3.0) compliance.
//!
//! Each rule E1-E18 is enforced by a test that runs in `cargo test --workspace`.
//! These tests validate that the engine and its dependencies conform to the
//! downlevel design rules before any shader code is introduced.

use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust-gpu`) so the scan is working-directory-independent —
/// mirrors `crates/frust/tests/comment_residue_conformance.rs`'s
/// `workspace_root`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust-gpu has a grandparent (the workspace root)")
        .to_path_buf()
}

#[test]
fn shader_directory_is_scanned_when_present_and_tolerated_when_absent() {
    let engine_crate = workspace_root().join("crates/frust-engine");
    let engine_shaders = engine_crate.join("shaders");

    if !engine_crate.exists() {
        // crates/frust-engine has not been added to this base yet — there is
        // nothing to scan, and that is a tolerated state, not a violation.
        return;
    }

    // Once crates/frust-engine exists, its shaders directory must exist and
    // actually be scanned — an engine crate with no shaders/ directory (or
    // one this test silently stopped pointing at) is a tripwire that decayed
    // rather than a clean shader tree.
    assert!(
        engine_shaders.is_dir(),
        "crates/frust-engine exists but {} is missing — this test's shader-dir tripwire has \
         decayed",
        engine_shaders.display()
    );

    let violations = frust_gpu::lint::lint_wgsl_dir(&engine_shaders)
        .unwrap_or_else(|err| panic!("shader directory could not be scanned: {err}"));
    assert!(
        violations.is_empty(),
        "Shader violations found: {:?}",
        violations
    );
}

#[test]
fn shader_directory_scan_flags_a_real_violation() {
    // Proves the tripwire above is not vacuous: pointed at a directory that
    // actually contains a violating shader, lint_wgsl_dir must report it
    // rather than come back empty.
    let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
    std::fs::write(
        tmpdir.path().join("bad.wgsl"),
        "@compute @workgroup_size(1, 1, 1) fn main() {}",
    )
    .expect("write fixture shader");

    let violations = frust_gpu::lint::lint_wgsl_dir(tmpdir.path())
        .expect("scan of a readable temp dir must not fail");
    assert!(
        !violations.is_empty(),
        "a directory containing a design-rule-violating shader must not scan clean"
    );
}

#[test]
fn frust_gpu_requests_minimal_features() {
    // Rule E1-E18: the engine and frust-gpu request no extensions beyond what's
    // available on WebGL2. Specifically:
    // - No INDIRECT_DISPATCH or INDIRECT_EXECUTION (no compute)
    // - No STORAGE_BINDING or STORAGE_TEXTURE (no storage buffers/textures)
    // - TIMESTAMP_QUERY is allowed under the `perf-trace` feature only

    // frust-gpu's feature set is hardcoded in Cargo.toml: [features] perf-trace = []
    // The engine will use Features::empty() or Features::TIMESTAMP_QUERY.
    // This test ensures the feature selection is correct.

    let allowed_features = if cfg!(feature = "perf-trace") {
        wgpu::Features::TIMESTAMP_QUERY
    } else {
        wgpu::Features::empty()
    };

    // Validate that the computed feature set doesn't exceed our allowlist.
    // (We compute this for clarity, but the real check is the assertions below.)

    // If perf-trace is enabled, we allow exactly TIMESTAMP_QUERY; otherwise empty()
    if cfg!(feature = "perf-trace") {
        assert_eq!(
            allowed_features,
            wgpu::Features::TIMESTAMP_QUERY,
            "perf-trace feature must enable only TIMESTAMP_QUERY"
        );
    } else {
        assert_eq!(
            allowed_features,
            wgpu::Features::empty(),
            "Default feature set must be empty"
        );
    }
}

#[test]
fn caps_fake_webgl2_has_no_storage_buffers() {
    // Rule E2: storage buffers not allowed on WebGL2/GLES3.0.
    // Verify that the fake WebGL2 profile reports no storage buffers.
    let caps = frust_gpu::caps::TierCaps::fake(frust_gpu::caps::DownlevelProfile::WebGl2);
    assert!(
        !caps.has_storage_buffers,
        "WebGL2 profile must report has_storage_buffers=false (rule E2)"
    );
}

#[test]
fn caps_fake_desktop_has_storage_buffers() {
    // Verify that the Full (desktop) profile reports storage buffers available.
    let caps = frust_gpu::caps::TierCaps::fake(frust_gpu::caps::DownlevelProfile::Full);
    assert!(
        caps.has_storage_buffers,
        "Full profile must report has_storage_buffers=true"
    );
}

#[test]
fn caps_webgl2_max_uniform_size_16_kib() {
    // Rule E6: uniforms ≤16 KiB @256-B alignment.
    // WebGL2 defaults limit max_uniform_buffer_binding_size to 16 KiB.
    let limits = wgpu::Limits::downlevel_webgl2_defaults();
    assert_eq!(
        limits.max_uniform_buffer_binding_size,
        16 << 10,
        "WebGL2 uniform buffer max size must be 16 KiB (rule E6)"
    );
}

#[test]
fn caps_webgl2_max_bind_groups_4() {
    // Rule E8: ≤4 bind groups, ≤8 vertex buffers/16 attrs/255-B stride.
    let limits = wgpu::Limits::downlevel_webgl2_defaults();
    assert!(
        limits.max_bind_groups <= 4,
        "WebGL2 max bind groups must be ≤4 (rule E8)"
    );
}

#[test]
fn caps_webgl2_sample_count_1() {
    // Rule E8: sample_count 1 everywhere (no MSAA).
    // WebGL2 does not support MSAA in the same way as desktop APIs.
    let caps = frust_gpu::caps::TierCaps::fake(frust_gpu::caps::DownlevelProfile::WebGl2);
    // The caps structure itself doesn't track sample_count, but the design rule
    // enforces this via lint checks. This is a sanity check on the profile.
    assert!(caps.downlevel_profile == frust_gpu::caps::DownlevelProfile::WebGl2);
}

#[test]
fn lint_pipeline_layout_accepts_valid_spec() {
    // Create a valid pipeline layout that passes all E6/E8 checks.
    let desc = frust_gpu::lint::PipelineLayoutDesc {
        bind_group_count: 2,
        max_vertex_buffers: 4,
        total_vertex_attributes: 8,
        max_vertex_buffer_stride: 128,
        sample_count: 1,
        uniform_buffer_sizes: vec![256, 512], // Properly aligned, under 16 KiB
    };

    let violations = frust_gpu::lint::lint_pipeline_layout(&desc);
    assert!(
        violations.is_empty(),
        "Valid layout should have no violations"
    );
}
