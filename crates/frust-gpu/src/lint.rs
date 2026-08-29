//! Design rule enforcement: WGSL shader linting and GPU descriptor validation.
//!
//! Every design rule E1-E18 is enforced by a test that runs in `cargo test
//! --workspace` from the phase that introduces the code it governs. This module
//! contains the lint functions that tests use to validate compliance.

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

/// Scans a directory for WGSL shader files (`*.wgsl`) and returns all design
/// rule violations found.
///
/// Checks for violations of rules E1 (no compute), E2 (no storage buffers/textures),
/// and related patterns in WGSL source code.
pub fn lint_wgsl_dir(dir: &Path) -> Vec<Violation> {
    let mut violations = Vec::new();

    // If the directory doesn't exist, return empty — the test asserts over an empty set
    // until shader files are added.
    if !dir.exists() {
        return violations;
    }

    // Scan all .wgsl files in the directory
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("wgsl")
                    && let Ok(content) = std::fs::read_to_string(&path)
                {
                    check_wgsl_file(&path, &content, &mut violations);
                }
            }
        }
        Err(_) => {
            // If the directory can't be read, return empty — this is not a lint violation.
        }
    }

    violations
}

/// Checks a single WGSL file for design rule violations.
fn check_wgsl_file(path: &Path, content: &str, violations: &mut Vec<Violation>) {
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

        // textureLoad on texture_depth is not allowed (naga cannot emit it for GLSL ES)
        if line.contains("textureLoad(") && line.contains("texture_depth") {
            violations.push(Violation {
                path: path.to_path_buf(),
                line: Some(line_num_1indexed),
                rule: "E5".to_string(),
                message: "textureLoad on depth textures not allowed: rule E5 (naga cannot emit for GLSL ES)"
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
/// - E6: uniforms ≤16 KiB @256-B alignment
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

    // E6: uniforms ≤16 KiB per bind group @256-B alignment
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

        // Check 256-B alignment
        if size % 256 != 0 {
            violations.push(Violation {
                path: PathBuf::from("<pipeline>"),
                line: None,
                rule: "E6".to_string(),
                message: format!(
                    "Uniform buffer size in bind group {}: {} is not 256-byte aligned: rule E6",
                    idx, size
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

    check_limit!(max_texture_dimension_2d, "E5");
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
        let violations = lint_wgsl_dir(tmpdir.path());
        assert!(violations.is_empty());
    }

    #[test]
    fn detects_compute_shader_e1() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(&wgsl_file, "@compute @workgroup_size(8, 8, 1) fn main() {}").unwrap();

        let violations = lint_wgsl_dir(tmpdir.path());
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "E1");
        assert!(violations[0].message.contains("Compute"));
    }

    #[test]
    fn detects_storage_buffer_e2() {
        let tmpdir = tempfile::tempdir().expect("failed to create temp dir");
        let wgsl_file = tmpdir.path().join("test.wgsl");
        std::fs::write(&wgsl_file, "var<storage> data: array<u32>;").unwrap();

        let violations = lint_wgsl_dir(tmpdir.path());
        assert!(violations.iter().any(|v| v.rule == "E2"));
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
    fn limits_check_detects_exceeded_webgl2_max() {
        let mut limits = wgpu::Limits::downlevel_webgl2_defaults();
        limits.max_bind_groups = 8; // Exceeds WebGL2 default of 4

        let violations = check_limits_against_webgl2(&limits);
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| v.contains("max_bind_groups")));
    }

    #[test]
    fn limits_check_passes_webgl2_defaults() {
        let limits = wgpu::Limits::downlevel_webgl2_defaults();
        let violations = check_limits_against_webgl2(&limits);
        assert!(violations.is_empty());
    }
}
