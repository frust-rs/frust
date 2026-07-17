//! The `[profile.dev.package.*]` shader-stack overrides (spec §12.9, 6e
//! Finding 5) are duplicated across three manifests that Cargo cannot share:
//! the root workspace, the app template, and the standalone catalog example.
//! DEVELOPMENT.md says they "must be kept in sync by hand" — this test is the
//! automated backstop, so drift in any one manifest fails the verify gate
//! instead of silently reopening the iOS launch-watchdog crash for one of the
//! three build entry points.
//!
//! The template manifest contains minijinja placeholders elsewhere in the
//! file, so the blocks are extracted line-wise rather than TOML-parsed.

use std::collections::BTreeMap;
use std::path::Path;

/// Extracts `package name -> opt-level` from every `[profile.dev.package.<name>]`
/// table in the manifest text.
fn dev_package_overrides(manifest: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("[profile.dev.package.") {
            current = rest.strip_suffix(']').map(|name| name.to_string());
        } else if line.starts_with('[') {
            current = None;
        } else if let (Some(name), Some(value)) = (&current, line.strip_prefix("opt-level")) {
            let value = value.trim_start_matches([' ', '=']).trim();
            out.insert(name.clone(), value.to_string());
        }
    }
    out
}

#[test]
fn shader_stack_dev_overrides_identical_across_all_three_manifests() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifests = [
        repo_root.join("Cargo.toml"),
        repo_root.join("templates/app/Cargo.toml.tmpl"),
        repo_root.join("examples/catalog/Cargo.toml"),
    ];

    let overrides: Vec<BTreeMap<String, String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            dev_package_overrides(&text)
        })
        .collect();

    assert!(
        !overrides[0].is_empty(),
        "root Cargo.toml has no [profile.dev.package.*] overrides — the \
         shader-stack dev-profile fix (6e Finding 5) has been removed?"
    );
    for (path, other) in manifests.iter().zip(&overrides).skip(1) {
        assert_eq!(
            &overrides[0],
            other,
            "[profile.dev.package.*] overrides in {} have drifted from the \
             root Cargo.toml — the three manifests must stay identical \
             (DEVELOPMENT.md, spec §12.9)",
            path.display()
        );
    }
}
