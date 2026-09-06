//! The hand-synced profile blocks are duplicated across manifests that Cargo
//! cannot share; DEVELOPMENT.md says they "must be kept in sync by hand" and
//! these tests are the automated backstop (there is no CI — the verify gate
//! is the only enforcement):
//!
//! - `[profile.dev.package.<shader-crate>]` overrides live in FIVE
//!   manifests (root, template, huddle, glyph-catalog, material3-demo) —
//!   drift silently reopens the iOS launch-watchdog crash.
//! - `[profile.release]` hardening and the `[profile.dev.package."*"]`
//!   wildcard live in the SAME five manifests (root,
//!   template, huddle, glyph-catalog, material3-demo — an earlier
//!   examples-convergence deleted `catalog` and `inbox`, folding their
//!   hand-sync role into `huddle`) — drift silently reopens the measured
//!   size/perf regression.
//!
//! `examples/glyph-catalog/Cargo.toml` and `examples/material3-demo/Cargo.toml`
//! are two more full `frust create` scaffolds carrying their own copies of
//! these blocks (same reason as `huddle`: each is a standalone workspace
//! whose `[profile.*]` blocks only take effect in a workspace root) — they
//! were hand-authored independently of the root/template/huddle trio, with
//! no automated backstop until this test grew to cover them too.
//!
//! The template manifest contains minijinja placeholders elsewhere in the
//! file, so the blocks are extracted line-wise rather than TOML-parsed.

use std::collections::{BTreeMap, BTreeSet};
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

/// Every manifest carrying the hand-synced profile blocks: root, template,
/// huddle (an earlier examples-convergence deleted the `catalog` and
/// `inbox` examples, the other two manifests this used to compare), plus
/// the two standalone example scaffolds that independently carry their own
/// copies of the same blocks (`glyph-catalog`, `material3-demo`).
fn synced_manifests() -> Vec<std::path::PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    vec![
        repo_root.join("Cargo.toml"),
        repo_root.join("templates/app/Cargo.toml.tmpl"),
        repo_root.join("examples/huddle/Cargo.toml"),
        repo_root.join("examples/glyph-catalog/Cargo.toml"),
        repo_root.join("examples/material3-demo/Cargo.toml"),
    ]
}

fn assert_section_identical(header: &str, expect_keys: &[&str]) {
    let manifests = synced_manifests();
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
            "root Cargo.toml's {header} is missing `{key}` — has the \
             profile hardening been removed?"
        );
    }
    for (path, other) in manifests.iter().zip(&blocks).skip(1) {
        assert_eq!(
            &blocks[0],
            other,
            "{header} in {} has drifted from the root Cargo.toml — every \
             hand-synced manifest must stay identical (see DEVELOPMENT.md's \
             profile-sync rule)",
            path.display()
        );
    }
}

#[test]
fn release_profile_identical_across_synced_manifests() {
    assert_section_identical(
        "[profile.release]",
        &["lto", "codegen-units", "strip", "panic"],
    );
}

#[test]
fn dev_wildcard_override_identical_across_synced_manifests() {
    assert_section_identical("[profile.dev.package.\"*\"]", &["opt-level"]);
}

#[test]
fn shader_stack_dev_overrides_identical_across_synced_manifests() {
    let manifests = synced_manifests();

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
         shader-stack dev-profile fix has been removed?"
    );
    for (path, other) in manifests.iter().zip(&overrides).skip(1) {
        assert_eq!(
            &overrides[0],
            other,
            "[profile.dev.package.*] overrides in {} have drifted from the \
             root Cargo.toml — every hand-synced manifest must stay identical \
             (see DEVELOPMENT.md's profile-sync rule)",
            path.display()
        );
    }
}

/// STRENGTHENS `shader_stack_dev_overrides_identical_across_synced_manifests`
/// above: that test only compares the *values* of the override names it
/// already knows about, so a crate added to one manifest's
/// `[profile.dev.package.*]` block but forgotten in another escapes it
/// entirely (both blocks agree on every key the shorter one has, and neither
/// test looks at the KEY SET). This one compares the full set of override
/// names across all five manifests instead.
#[test]
fn shader_stack_dev_override_name_set_is_identical_across_synced_manifests() {
    let manifests = synced_manifests();

    let name_sets: Vec<BTreeSet<String>> = manifests
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            dev_package_overrides(&text).into_keys().collect()
        })
        .collect();

    assert!(
        !name_sets[0].is_empty(),
        "root Cargo.toml has no [profile.dev.package.*] overrides — the \
         shader-stack dev-profile fix has been removed?"
    );
    for (path, other) in manifests.iter().zip(&name_sets).skip(1) {
        assert_eq!(
            &name_sets[0],
            other,
            "[profile.dev.package.*] override NAMES in {} differ from the root \
             Cargo.toml — a crate added to one manifest's shader-stack overrides but not \
             the others would otherwise escape the value-only comparison above (see \
             DEVELOPMENT.md's profile-sync rule)",
            path.display()
        );
    }
}
