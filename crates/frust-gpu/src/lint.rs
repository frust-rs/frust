//! Design rule enforcement: WGSL shader linting and GPU descriptor validation.
//!
//! Every design rule E1-E18 is enforced by a test that runs in `cargo test
//! --workspace` from the phase that introduces the code it governs. This module
//! contains the lint functions that tests use to validate compliance.

use std::fmt;
use std::path::{Path, PathBuf};

/// A violation of a design rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The file path where the violation was found (absolute or relative).
    pub path: PathBuf,
    /// The line number (1-indexed), if known.
    pub line: Option<usize>,
    /// The design rule E-number (e.g., "E1", "E2").
    pub rule: String,
    /// Human-readable description of the violation.
    pub message: String,
}

/// An I/O failure encountered while scanning a directory for WGSL shaders.
///
/// Distinct from an empty [`Violation`] list: a directory (or file inside it)
/// that could not be read is a scan that did not complete, not a scan that
/// completed and found nothing clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintError {
    /// The path whose read failed — the directory being listed, or the
    /// `.wgsl` file whose contents could not be read.
    pub path: PathBuf,
    /// The underlying I/O failure, rendered to a string (`std::io::Error` is
    /// neither `Clone` nor `PartialEq`, and this type needs to stay
    /// comparable for tests).
    pub message: String,
}

impl fmt::Display for LintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for LintError {}

/// Scans a directory, recursing into every subdirectory, for WGSL shader
/// files (`*.wgsl`) and returns all design rule violations found.
///
/// Checks for violations of rules E1 (no compute), E2 (no storage buffers/textures),
/// and related patterns in WGSL source code.
///
/// A directory that does not exist is tolerated and reported as zero
/// violations — the convention callers rely on for a shader directory that
/// has not been added to a crate yet. A directory that exists but cannot be
/// listed, or a `.wgsl` file that cannot be read, is surfaced as a
/// [`LintError`] rather than folded into an empty result: a scan that could
/// not complete is not a clean scan.
pub fn lint_wgsl_dir(dir: &Path) -> Result<Vec<Violation>, LintError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut violations = Vec::new();
    scan_wgsl_dir(dir, &mut violations)?;
    Ok(violations)
}

/// Recursive worker behind [`lint_wgsl_dir`]: walks `dir` and every
/// subdirectory beneath it, checking each `.wgsl` file it finds.
fn scan_wgsl_dir(dir: &Path, violations: &mut Vec<Violation>) -> Result<(), LintError> {
    let entries = std::fs::read_dir(dir).map_err(|err| LintError {
        path: dir.to_path_buf(),
        message: err.to_string(),
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| LintError {
            path: dir.to_path_buf(),
            message: err.to_string(),
        })?;
        let path = entry.path();

        if path.is_dir() {
            scan_wgsl_dir(&path, violations)?;
            continue;
        }

        if path.extension().and_then(|s| s.to_str()) == Some("wgsl") {
            let content = std::fs::read_to_string(&path).map_err(|err| LintError {
                path: path.clone(),
                message: err.to_string(),
            })?;
            check_wgsl_file(&path, &content, violations);
        }
    }

    Ok(())
}

/// Checks a single WGSL file for design rule violations.
fn check_wgsl_file(path: &Path, content: &str, violations: &mut Vec<Violation>) {
    // E5 (textureLoad arm): correlated at file level, conservatively, rather
    // than requiring both halves on the same line — naga cannot emit
    // `textureLoad` on a depth texture for GLSL ES regardless of which
    // texture binding a given call actually targets, so any file that both
    // declares a `texture_depth*` binding and calls `textureLoad(` anywhere
    // is flagged, even when the two are unrelated to each other.
    let declares_depth_texture_binding = content
        .lines()
        .any(|line| line.contains("var") && line.contains("texture_depth"));

    for (line_num, line) in content.lines().enumerate() {
        let line_num_1indexed = line_num + 1;

        // E1: no compute shaders
        if line.contains("@compute") {
            violations.push(Violation {
                path: path.to_path_buf(),
                line: Some(line_num_1indexed),
                rule: "E1".to_string(),
                message: "Compute shaders not allowed: rule E1 (no compute)".to_string(),
            });
        }

        // E2: no storage_buffer or storage texture bindings
        if line.contains("var<storage") {
            violations.push(Violation {
                path: path.to_path_buf(),
                line: Some(line_num_1indexed),
                rule: "E2".to_string(),
                message: "Storage buffers not allowed: rule E2 (no storage buffers)".to_string(),
            });
        }

        if line.contains("texture_storage_") {
            violations.push(Violation {
                path: path.to_path_buf(),
                line: Some(line_num_1indexed),
                rule: "E2".to_string(),
                message: "Storage textures not allowed: rule E2 (no storage textures)".to_string(),
            });
        }

        // firstTrailingBit, firstLeadingBit, countLeadingZeros not allowed
        if line.contains("firstTrailingBit")
            || line.contains("firstLeadingBit")
            || line.contains("countLeadingZeros")
        {
            violations.push(Violation {
                path: path.to_path_buf(),
                line: Some(line_num_1indexed),
                rule: "E5".to_string(),
                message: "Bit manipulation intrinsics not allowed: rule E5".to_string(),
            });
        }

        // textureLoad anywhere in a file that also declares a texture_depth*
        // binding is not allowed (naga cannot emit it for GLSL ES) — see the
        // file-level correlation note above.
        if declares_depth_texture_binding && line.contains("textureLoad(") {
            violations.push(Violation {
                path: path.to_path_buf(),
                line: Some(line_num_1indexed),
                rule: "E5".to_string(),
                message:
                    "textureLoad in a file that declares a texture_depth binding not allowed: \
                     rule E5 (naga cannot emit for GLSL ES; correlated at file level, not \
                     per-line)"
                        .to_string(),
            });
        }
    }
}

/// Pipeline layout descriptor — a simplified representation for lint checking.
#[derive(Debug)]
pub struct PipelineLayoutDesc {
    /// Number of bind groups
    pub bind_group_count: usize,
    /// Maximum vertex buffers used
    pub max_vertex_buffers: usize,
    /// Total vertex attributes across all buffers
    pub total_vertex_attributes: usize,
    /// Maximum stride of any vertex buffer
    pub max_vertex_buffer_stride: usize,
    /// Sample count for rasterization
    pub sample_count: u32,
    /// Uniform buffer sizes per bind group
    pub uniform_buffer_sizes: Vec<usize>,
}

/// Validates that a pipeline layout descriptor conforms to design rules E6 and E8.
///
/// Checks:
/// - E6: uniform binding size ≤ `max_uniform_buffer_binding_size` (16 KiB on
///   downlevel). 256-byte alignment is a property of a binding's *offset*,
///   not its size, so it is not checked here — this descriptor carries sizes
///   only, no offsets.
/// - E8: ≤4 bind groups, ≤8 vertex buffers/16 attrs/255-B stride, sample_count 1 everywhere
///
/// Returns a vector of violations found.
pub fn lint_pipeline_layout(desc: &PipelineLayoutDesc) -> Vec<Violation> {
    let mut violations = Vec::new();

    // E8: ≤4 bind groups
    if desc.bind_group_count > 4 {
        violations.push(Violation {
            path: PathBuf::from("<pipeline>"),
            line: None,
            rule: "E8".to_string(),
            message: format!("Bind groups: {} > 4 (max): rule E8", desc.bind_group_count),
        });
    }

    // E8: ≤8 vertex buffers
    if desc.max_vertex_buffers > 8 {
        violations.push(Violation {
            path: PathBuf::from("<pipeline>"),
            line: None,
            rule: "E8".to_string(),
            message: format!(
                "Vertex buffers: {} > 8 (max): rule E8",
                desc.max_vertex_buffers
            ),
        });
    }

    // E8: ≤16 vertex attributes
    if desc.total_vertex_attributes > 16 {
        violations.push(Violation {
            path: PathBuf::from("<pipeline>"),
            line: None,
            rule: "E8".to_string(),
            message: format!(
                "Vertex attributes: {} > 16 (max): rule E8",
                desc.total_vertex_attributes
            ),
        });
    }

    // E8: vertex buffer stride ≤255 bytes
    if desc.max_vertex_buffer_stride > 255 {
        violations.push(Violation {
            path: PathBuf::from("<pipeline>"),
            line: None,
            rule: "E8".to_string(),
            message: format!(
                "Vertex buffer stride: {} > 255 bytes (max): rule E8",
                desc.max_vertex_buffer_stride
            ),
        });
    }

    // E8: sample_count must be 1 (no MSAA)
    if desc.sample_count != 1 {
        violations.push(Violation {
            path: PathBuf::from("<pipeline>"),
            line: None,
            rule: "E8".to_string(),
            message: format!(
                "Sample count: {} != 1 (no MSAA): rule E8",
                desc.sample_count
            ),
        });
    }

    // E6: uniform binding size ≤16 KiB (max_uniform_buffer_binding_size).
    // Alignment applies to a binding's offset, not its size, so a size that
    // merely isn't a multiple of 256 is not itself a violation.
    const MAX_UNIFORM_SIZE: usize = 16 << 10; // 16 KiB
    for (idx, &size) in desc.uniform_buffer_sizes.iter().enumerate() {
        if size > MAX_UNIFORM_SIZE {
            violations.push(Violation {
                path: PathBuf::from("<pipeline>"),
                line: None,
                rule: "E6".to_string(),
                message: format!(
                    "Uniform buffer size in bind group {}: {} > {} bytes (max): rule E6",
                    idx, size, MAX_UNIFORM_SIZE
                ),
            });
        }
    }

    violations
}

/// Checks that GPU limits conform to WebGL2/GLES3.0 downlevel defaults.
///
/// Uses `wgpu::Limits::check_limits` to validate that the given limits are at or
/// below the WebGL2 downlevel defaults.
pub fn check_limits_against_webgl2(limits: &wgpu::Limits) -> Vec<String> {
    let mut violations = Vec::new();
    let webgl2_limits = wgpu::Limits::downlevel_webgl2_defaults();

    // Manually check each critical limit
    macro_rules! check_limit {
        ($field:ident, $rule:expr) => {
            if limits.$field > webgl2_limits.$field {
                violations.push(format!(
                    "{}: {} > {} (WebGL2 max): rule {}",
                    stringify!($field),
                    limits.$field,
                    webgl2_limits.$field,
                    $rule
                ));
            }
        };
    }

    // E7: texture dimension limit — its own rule id, distinct from the E8
    // bind-group/vertex-topology family and the E5 unsupported-intrinsic
    // family, since it concerns neither.
    check_limit!(max_texture_dimension_2d, "E7");
    check_limit!(max_bind_groups, "E8");
    check_limit!(max_vertex_attributes, "E8");
    check_limit!(max_vertex_buffers, "E8");

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile;

    #[test]
    fn empty_wgsl_dir_returns_no_violations() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let violations =
            lint_wgsl_dir(tmpdir.path()).expect("scan of a fresh temp dir must not fail");
        assert!(violations.is_empty());
    }

    #[test]
    fn detects_compute_shader_e1() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(&wgsl_file, "@compute @workgroup_size(8, 8, 1) fn main() {}").unwrap();

        let violations = lint_wgsl_dir(tmpdir.path()).expect("scan must not fail");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "E1");
        assert!(violations[0].message.contains("Compute"));
    }

    #[test]
    fn detects_storage_buffer_e2() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(&wgsl_file, "var<storage> data: array<u32>;").unwrap();

        let violations = lint_wgsl_dir(tmpdir.path()).expect("scan must not fail");
        assert!(violations.iter().any(|v| v.rule == "E2"));
    }

    #[test]
    fn lint_wgsl_dir_recurses_into_subdirectories() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let nested = tmpdir.path().join("nested").join("deeper");
        std::fs::create_dir_all(&nested).expect("create nested dirs");
        std::fs::write(
            nested.join("bad.wgsl"),
            "@compute @workgroup_size(1, 1, 1) fn main() {}",
        )
        .unwrap();

        let violations =
            lint_wgsl_dir(tmpdir.path()).expect("scan of a readable tree must not fail");
        assert!(
            violations.iter().any(|v| v.rule == "E1"),
            "a violation two directories deep must still be found: {violations:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn lint_wgsl_dir_surfaces_unreadable_subdirectory_as_lint_error() {
        use std::os::unix::fs::PermissionsExt;

        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let blocked = tmpdir.path().join("blocked");
        std::fs::create_dir(&blocked).expect("create blocked subdir");
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000))
            .expect("chmod blocked subdir unreadable");

        // Some environments (e.g. running as root in CI) ignore permission
        // bits entirely; skip rather than false-failing when that is true.
        let still_readable = std::fs::read_dir(&blocked).is_ok();
        if still_readable {
            let _ = std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755));
            return;
        }

        let result = lint_wgsl_dir(tmpdir.path());
        let _ = std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755));

        match result {
            Err(err) => assert_eq!(err.path, blocked),
            Ok(violations) => panic!(
                "a directory containing an unreadable subdirectory must surface a LintError, \
                 not a clean scan; got {violations:?}"
            ),
        }
    }

    #[test]
    fn detects_texture_load_correlated_with_depth_binding_e5() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(
            &wgsl_file,
            "@group(0) @binding(0) var t_depth: texture_depth_2d;\n\
             @group(0) @binding(1) var t_other: texture_2d<f32>;\n\
             fn main() {\n\
                 let x = textureLoad(t_other, vec2<i32>(0, 0), 0);\n\
             }\n",
        )
        .unwrap();

        let violations = lint_wgsl_dir(tmpdir.path()).expect("scan must not fail");
        assert!(
            violations.iter().any(|v| v.rule == "E5"),
            "a textureLoad anywhere in a file that also declares a texture_depth binding must \
             be flagged, even on a separate line and against a different texture: {violations:?}"
        );
    }

    #[test]
    fn texture_load_on_non_depth_texture_alone_is_not_flagged_e5() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(
            &wgsl_file,
            "@group(0) @binding(0) var t: texture_2d<f32>;\n\
             fn main() {\n\
                 let x = textureLoad(t, vec2<i32>(0, 0), 0);\n\
             }\n",
        )
        .unwrap();

        let violations = lint_wgsl_dir(tmpdir.path()).expect("scan must not fail");
        assert!(
            violations.iter().all(|v| v.rule != "E5"),
            "textureLoad on a plain (non-depth) texture, with no texture_depth binding anywhere \
             in the file, must not be flagged: {violations:?}"
        );
    }

    #[test]
    fn depth_binding_sampled_via_compare_without_texture_load_is_not_flagged_e5() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(
            &wgsl_file,
            "@group(0) @binding(0) var t_depth: texture_depth_2d;\n\
             @group(0) @binding(1) var s: sampler_comparison;\n\
             fn main() {\n\
                 let x = textureSampleCompare(t_depth, s, vec2<f32>(0.0, 0.0), 0.5);\n\
             }\n",
        )
        .unwrap();

        let violations = lint_wgsl_dir(tmpdir.path()).expect("scan must not fail");
        assert!(
            violations.iter().all(|v| v.rule != "E5"),
            "a depth binding sampled only via textureSampleCompare (no textureLoad call) must \
             not be flagged: {violations:?}"
        );
    }

    #[test]
    fn pipeline_layout_enforces_bind_group_limit_e8() {
        let desc = PipelineLayoutDesc {
            bind_group_count: 5,
            max_vertex_buffers: 2,
            total_vertex_attributes: 4,
            max_vertex_buffer_stride: 128,
            sample_count: 1,
            uniform_buffer_sizes: vec![],
        };

        let violations = lint_pipeline_layout(&desc);
        assert!(
            violations
                .iter()
                .any(|v| { v.rule == "E8" && v.message.contains("Bind groups") })
        );
    }

    #[test]
    fn pipeline_layout_enforces_msaa_disabled_e8() {
        let desc = PipelineLayoutDesc {
            bind_group_count: 2,
            max_vertex_buffers: 2,
            total_vertex_attributes: 4,
            max_vertex_buffer_stride: 128,
            sample_count: 4,
            uniform_buffer_sizes: vec![],
        };

        let violations = lint_pipeline_layout(&desc);
        assert!(
            violations
                .iter()
                .any(|v| { v.rule == "E8" && v.message.contains("Sample count") })
        );
    }

    #[test]
    fn pipeline_layout_enforces_uniform_size_e6() {
        let desc = PipelineLayoutDesc {
            bind_group_count: 1,
            max_vertex_buffers: 1,
            total_vertex_attributes: 2,
            max_vertex_buffer_stride: 16,
            sample_count: 1,
            uniform_buffer_sizes: vec![20 << 10], // 20 KiB, exceeds 16 KiB limit
        };

        let violations = lint_pipeline_layout(&desc);
        assert!(
            violations
                .iter()
                .any(|v| { v.rule == "E6" && v.message.contains("Uniform buffer size") })
        );
    }

    #[test]
    fn pipeline_layout_uniform_size_not_256_aligned_is_not_flagged_e6() {
        let desc = PipelineLayoutDesc {
            bind_group_count: 1,
            max_vertex_buffers: 1,
            total_vertex_attributes: 2,
            max_vertex_buffer_stride: 16,
            sample_count: 1,
            uniform_buffer_sizes: vec![300], // legal (well under 16 KiB), not a multiple of 256
        };

        let violations = lint_pipeline_layout(&desc);
        assert!(
            violations.iter().all(|v| v.rule != "E6"),
            "a uniform buffer under the size limit must not be flagged merely for not being a \
             multiple of 256 — alignment is a property of a binding's offset, not its size: \
             {violations:?}"
        );
    }

    #[test]
    fn limits_check_detects_exceeded_webgl2_max() {
        let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
        limits.max_bind_groups = 8; // Exceeds WebGL2 default of 4

        let violations = check_limits_against_webgl2(&limits);
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| v.contains("max_bind_groups")));
    }

    #[test]
    fn limits_check_flags_texture_dimension_as_e7_not_e5() {
        let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
        limits.max_texture_dimension_2d *= 2;

        let violations = check_limits_against_webgl2(&limits);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("max_texture_dimension_2d") && v.contains("rule E7")),
            "exceeding max_texture_dimension_2d must cite its own rule id, not E5: {violations:?}"
        );
    }

    #[test]
    fn limits_check_passes_webgl2_defaults() {
        let limits = wgpu::Limits::downlevel_webgl2_defaults();
        let violations = check_limits_against_webgl2(&limits);
        assert!(violations.is_empty());
    }
}
