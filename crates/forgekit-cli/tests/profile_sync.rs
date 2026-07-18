//! The hand-synced profile blocks are duplicated across manifests that Cargo
//! cannot share; DEVELOPMENT.md says they "must be kept in sync by hand" and
//! these tests are the automated backstop (there is no CI — the verify gate
//! is the only enforcement):
//!
//! - `[profile.dev.package.<shader-crate>]` overrides (spec §12.9, 6e
//!   Finding 5) live in THREE manifests (root, template, catalog) — drift
//!   silently reopens the iOS launch-watchdog crash.
//! - `[profile.release]` hardening and the `[profile.dev.package."*"]`
//!   wildcard (phase 7 task 11) live in FIVE manifests (those three plus
//!   team-demo and inbox) — drift silently reopens the measured size/perf
//!   regression (review round-0 finding 4).
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

/// Extracts the `key = value` pairs of one `[section]` (exact header match),
/// stopping at the next `[` header. Comment lines are skipped.
fn section_pairs(manifest: &str, header: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line == header {
            in_section = true;
        } else if line.starts_with('[') {
            in_section = false;
        } else if in_section
            && !line.is_empty()
            && !line.starts_with('#')
            && let Some((k, v)) = line.split_once('=')
        {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// The five manifests carrying the phase-7 hand-synced profile blocks.
fn five_manifests() -> Vec<std::path::PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    vec![
        repo_root.join("Cargo.toml"),
        repo_root.join("templates/app/Cargo.toml.tmpl"),
        repo_root.join("examples/catalog/Cargo.toml"),
        repo_root.join("examples/team-demo/Cargo.toml"),
        repo_root.join("examples/inbox/Cargo.toml"),
    ]
}

fn assert_section_identical(header: &str, expect_keys: &[&str]) {
    let manifests = five_manifests();
    let blocks: Vec<BTreeMap<String, String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            section_pairs(&text, header)
        })
        .collect();
    for key in expect_keys {
        assert!(
            blocks[0].contains_key(*key),
            "root Cargo.toml's {header} is missing `{key}` — the phase-7 \
             hardening (task 11) has been removed?"
        );
    }
    for (path, other) in manifests.iter().zip(&blocks).skip(1) {
        assert_eq!(
            &blocks[0],
            other,
            "{header} in {} has drifted from the root Cargo.toml — the five \
             manifests must stay identical (DEVELOPMENT.md; phase-7 review \
             round-0 finding 4)",
            path.display()
        );
    }
}

#[test]
fn release_profile_identical_across_all_five_manifests() {
    assert_section_identical(
        "[profile.release]",
        &["lto", "codegen-units", "strip", "panic"],
    );
}

#[test]
fn dev_wildcard_override_identical_across_all_five_manifests() {
    assert_section_identical("[profile.dev.package.\"*\"]", &["opt-level"]);
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
