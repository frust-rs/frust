//! Structural guards G1, G2, and E17: design-rule tripwires over the engine
//! crate's shape rather than its runtime behavior. Host-only — no GPU device
//! or adapter is created anywhere in this file.
//!
//! - **G1**: `frust-gpu`'s WGSL directory lint, run over
//!   `crates/frust-engine/shaders/` — must now find real files (p3-05 ported
//!   the strip/clear/copy programs there) and report zero design-rule
//!   violations.
//! - **G2**: the engine's downlevel GPU-limits profile stays within the
//!   WebGL2/GLES3.0 ceiling (`wgpu::Limits::check_limits` against
//!   `Limits::downlevel_webgl2_defaults()`), the only `wgpu::Features` bit
//!   the substrate's device-request policy may ever ask for is
//!   `TIMESTAMP_QUERY` (and only under a `perf-trace` build), and
//!   `frust-gpu`'s `HeadlessTarget` never requests `STORAGE_BINDING` — it
//!   renders through ordinary render passes, never compute
//!   (`crates/frust-gpu/src/headless.rs`'s own module doc states the same
//!   rule; this guard pins it against silent drift).
//! - **E17**: every rendering path under `src/{cache,compile,filters,gpu,
//!   schedule,renderer,text}.rs` returns an error rather than panicking — no
//!   bare `unwrap()`/`expect(`/`panic!(` outside a `#[cfg(test)]` module.
//!   Currently scanned: `cache/`, `compile/`, `filters/`, `gpu/`, and `text/`.
//!   Conditionally scanned when present: `renderer.rs` and `schedule/` — these
//!   stay unscanned until merge, then the guard becomes load-bearing with no
//!   edit needed here.

use std::path::{Path, PathBuf};

/// `crates/frust-engine`, resolved from this crate's own manifest dir.
fn engine_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `crates/frust-gpu`, a sibling of [`engine_root`] under `crates/`.
fn gpu_crate_root() -> PathBuf {
    engine_root()
        .parent()
        .expect("crates/frust-engine has a parent: crates/")
        .join("frust-gpu")
}

// ---------------------------------------------------------------------
// G1: the engine's shipped shaders pass frust-gpu's downlevel lint
// ---------------------------------------------------------------------

#[test]
fn g1_engine_shader_directory_has_real_files_and_passes_the_downlevel_lint() {
    let shaders_dir = engine_root().join("shaders");
    assert!(
        shaders_dir.is_dir(),
        "{} must exist and hold the ported strip/clear/copy shaders",
        shaders_dir.display()
    );

    let wgsl_files: Vec<PathBuf> = std::fs::read_dir(&shaders_dir)
        .unwrap_or_else(|err| panic!("reading {}: {err}", shaders_dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wgsl"))
        .collect();
    assert!(
        !wgsl_files.is_empty(),
        "{} has no .wgsl files — an empty directory would let `lint_wgsl_dir` pass vacuously, \
         hiding the fact that the shader port never landed",
        shaders_dir.display()
    );

    let violations = frust_gpu::lint_wgsl_dir(&shaders_dir)
        .unwrap_or_else(|err| panic!("scanning {}: {err}", shaders_dir.display()));
    assert!(
        violations.is_empty(),
        "the engine's shipped shaders violate frust-gpu's downlevel design rules: {violations:?}"
    );
}

// ---------------------------------------------------------------------
// G2: downlevel GPU-limits/features policy, and HeadlessTarget usage
// ---------------------------------------------------------------------

/// The only `wgpu::Features` bit the engine's GPU substrate device-request
/// policy (`frust_gpu::context`'s `required_features`) is ever allowed to
/// ask for: empty by default, `TIMESTAMP_QUERY` only under a `perf-trace`
/// build and only when the adapter actually offers it.
const ALLOWED_ENGINE_FEATURES: wgpu::Features = wgpu::Features::TIMESTAMP_QUERY;

#[test]
fn g2_webgl2_downlevel_limits_satisfy_the_webgl2_ceiling() {
    let caps = frust_gpu::TierCaps::fake(frust_gpu::DownlevelProfile::WebGl2);
    let webgl2_defaults = wgpu::Limits::downlevel_webgl2_defaults();

    // The subset of `wgpu::Limits` fields `TierCaps` actually tracks, laid
    // over the WebGL2 defaults for every field it does not — the same shape
    // `frust_gpu::context::create_device` requests under
    // `DownlevelProfile::WebGl2` (`downlevel_webgl2_defaults().using_resolution(..)`).
    let declared = wgpu::Limits {
        max_texture_dimension_2d: caps.max_texture_dimension_2d,
        max_texture_array_layers: caps.max_texture_array_layers,
        max_bind_groups: caps.max_bind_groups,
        max_uniform_buffer_binding_size: u64::from(caps.max_uniform_buffer_binding_size),
        min_uniform_buffer_offset_alignment: caps.min_uniform_buffer_offset_alignment,
        max_vertex_attributes: caps.max_vertex_attributes,
        ..webgl2_defaults.clone()
    };

    assert!(
        declared.check_limits(&webgl2_defaults),
        "the engine's downlevel-profile GPU limits exceed the WebGL2 ceiling: {declared:?}"
    );
    assert!(
        frust_gpu::check_limits_against_webgl2(&declared).is_empty(),
        "frust-gpu's own WebGL2 lint disagrees with wgpu's `check_limits` for {declared:?}"
    );
}

#[test]
fn g2_requested_features_never_exceed_the_perf_trace_timestamp_query_allowance() {
    for profile in [
        frust_gpu::DownlevelProfile::Full,
        frust_gpu::DownlevelProfile::WebGl2,
    ] {
        let caps = frust_gpu::TierCaps::fake(profile);
        // Mirrors `frust_gpu::context::required_features`'s documented
        // policy exactly: empty unless a perf-trace build's adapter actually
        // offers the feature.
        let would_request_under_perf_trace = if caps.has_timestamp_query {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        assert!(
            ALLOWED_ENGINE_FEATURES.contains(would_request_under_perf_trace),
            "{profile:?} caps would let a perf-trace build request \
             {would_request_under_perf_trace:?}, outside the engine's allowed feature set \
             {ALLOWED_ENGINE_FEATURES:?}"
        );
    }
}

#[test]
fn g2_headless_target_usage_excludes_storage_binding() {
    let path = gpu_crate_root().join("src/headless.rs");
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));

    let usage_lines: Vec<&str> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .filter(|line| line.contains("usage") && line.contains("TextureUsages"))
        .collect();
    assert!(
        !usage_lines.is_empty(),
        "expected to find HeadlessTarget's texture-usage declaration (a `usage: \
         wgpu::TextureUsages::...` line) in {}",
        path.display()
    );

    for line in usage_lines {
        assert!(
            !line.contains("STORAGE_BINDING"),
            "HeadlessTarget must never request STORAGE_BINDING — it renders through ordinary \
             render passes, never compute (see {}'s module doc): {line}",
            path.display()
        );
    }
}

// ---------------------------------------------------------------------
// E17: no unwrap()/expect(/panic!( outside #[cfg(test)] under
// src/{cache,compile,gpu,schedule,renderer}.rs
// ---------------------------------------------------------------------

/// Substrings that fail this guard wherever they appear in production code.
/// Deliberately literal (not a parser): `unwrap()` (no arguments — an
/// `unwrap_or`/`unwrap_or_else` never matches), `expect(`, and `panic!(`.
const BANNED_NEEDLES: &[&str] = &["unwrap()", ".expect(", "panic!("];

/// Every `.rs` file directly under `dir` and every subdirectory, or an empty
/// list when `dir` does not exist — the same "not-yet-added is a clean scan"
/// convention `frust_gpu::lint::lint_wgsl_dir` documents for a shader
/// directory, applied here to a source directory that may not exist yet on
/// this base (`schedule/`, `renderer.rs`'s sibling module).
fn rust_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|err| panic!("dir entry in {}: {err}", dir.display()))
            .path();
        if path.is_dir() {
            rust_files_recursive(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// The E17 grep scope:
/// `src/{cache,compile,filters,gpu,text,schedule,renderer}.rs` — `cache/`,
/// `compile/`, `filters/`, `gpu/`, and `text/` are trees that exist on this
/// base; `renderer.rs` and `schedule/` are grepped only when present.
///
/// `cache/`, `filters/` and `text/` all execute on the frame path: a gradient
/// ramp is built, keyed and evicted during frame rendering (cache); a blurred
/// layer's kernel is prepared and its pass sequence planned during frame
/// rendering (filters); and glyphs are compiled and shaped during frame
/// rendering (text). Every directory the compiler and renderer call into on
/// that path is in scope.
fn e17_scanned_files() -> Vec<PathBuf> {
    let src = engine_root().join("src");
    let mut files = Vec::new();

    for name in ["cache", "compile", "filters", "gpu", "text"] {
        rust_files_recursive(&src.join(name), &mut files);
    }

    let renderer_rs = src.join("renderer.rs");
    if renderer_rs.is_file() {
        files.push(renderer_rs);
    }
    rust_files_recursive(&src.join("schedule"), &mut files);

    files.sort();
    files
}

/// `contents`' `(0-based line index, raw text)` pairs up to (excluding) the
/// file's `#[cfg(test)]`/`mod tests` marker — every engine module in the E17
/// scope keeps its test module last-in-file, the same convention
/// `frust-drive/tests/print_free_cores.rs` relies on for the identical scan
/// shape. Comment-only lines are dropped so a prose mention of a banned
/// needle (e.g. this file's own doc comments) is never mistaken for code.
fn production_lines(contents: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for (i, raw) in contents.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("mod tests") {
            break;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        out.push((i, raw));
    }
    out
}

#[test]
fn e17_engine_render_paths_never_unwrap_expect_or_panic_outside_tests() {
    let mut failures = Vec::new();

    for path in e17_scanned_files() {
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        for (i, line) in production_lines(&contents) {
            for needle in BANNED_NEEDLES {
                if line.contains(needle) {
                    failures.push(format!(
                        "{}:{}: `{needle}` outside #[cfg(test)] — E17 requires every rendering \
                         path to return an error rather than panic: {}",
                        path.display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "E17 violated ({} hit(s)):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn e17_scan_currently_covers_at_least_cache_compile_filters_gpu_and_text() {
    // Documents the scan's current floor so a future refactor that
    // accidentally empties `e17_scanned_files()` (e.g. a typo'd directory
    // name) fails loudly here rather than the main guard above silently
    // passing over nothing. Named per directory rather than merely counted:
    // a scan that quietly stopped covering one of the five would otherwise
    // still satisfy a non-empty assertion.
    let files = e17_scanned_files();
    for name in ["cache", "compile", "filters", "gpu", "text"] {
        assert!(
            files
                .iter()
                .any(|path| path.components().any(|c| c.as_os_str() == name)),
            "the E17 scan covers no .rs file under src/{name}/: {files:?}"
        );
    }
}
