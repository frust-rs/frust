//! Library manifests leave the `frust-ui` / `frust-text` default features off.
//!
//! `complex-scripts` is default-on in both crates and an app opts out with
//! `default-features = false`. Cargo unifies features across the whole dependency graph, so
//! one library that depends on either crate with default features turns the feature back on
//! and the app cannot undo it. Every manifest under `plugins/` and `crates/` (except the two
//! crates themselves) must therefore disable defaults on each dependency on them. A
//! `workspace = true` entry inherits from the root `[workspace.dependencies]` entry, which is
//! checked instead. `examples/` and `benchmarks/` are apps and are exempt.
//!
//! The scan is plain text over single-line dependency entries: no TOML parser dependency.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/frust sits two levels below the workspace root")
        .to_path_buf()
}

/// Collects `Cargo.toml` files below `dir`, skipping build output and hidden directories.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                collect(&path, out);
            }
        } else if name == "Cargo.toml" {
            out.push(path);
        }
    }
}

/// True when `line` declares a dependency on `frust-ui` (any local key) or `frust-text`.
fn is_facade_dependency(line: &str) -> bool {
    let key = line
        .split(|c: char| c == '=' || c == '.' || c.is_whitespace())
        .next()
        .unwrap_or("");
    line.contains("package = \"frust-ui\"") || key == "frust-text" || key == "frust-ui"
}

fn inherits_from_workspace(line: &str) -> bool {
    line.contains("workspace = true")
}

/// Violations in one manifest as `path:line: text`. In the root manifest only the
/// `[workspace.dependencies]` entries are checked; elsewhere a `workspace = true` entry passes.
fn violations(label: &str, contents: &str, is_root: bool) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_workspace_deps = false;
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_workspace_deps = line == "[workspace.dependencies]";
            continue;
        }
        if line.starts_with('#') || (is_root && !in_workspace_deps) {
            continue;
        }
        if !is_facade_dependency(line) || !line.contains('=') {
            continue;
        }
        if !is_root && inherits_from_workspace(line) {
            continue;
        }
        if !line.contains("default-features = false") {
            found.push(format!("{label}:{}: {line}", index + 1));
        }
    }
    found
}

/// Manifests whose crate directory is exempt: the two crates themselves.
fn is_exempt(root: &Path, manifest: &Path) -> bool {
    [root.join("crates/frust"), root.join("crates/frust-text")]
        .iter()
        .any(|dir| manifest.parent() == Some(dir.as_path()))
}

#[test]
fn violations_name_the_line_without_defaults_off() {
    let found = violations(
        "x/Cargo.toml",
        "[dependencies]\nfrust = { package = \"frust-ui\", path = \"p\" }\n",
        false,
    );
    assert_eq!(found.len(), 1);
    assert!(found[0].starts_with("x/Cargo.toml:2:"));
    assert!(violations(
        "x",
        "frust-text = { path = \"p\", default-features = false }\nfrust-text = { workspace = true }\n",
        false
    )
    .is_empty());
    assert_eq!(
        violations(
            "x",
            "[workspace.dependencies]\nfrust-text = { path = \"p\" }\n",
            true
        )
        .len(),
        1
    );
}

#[test]
fn library_manifests_leave_facade_defaults_off() {
    let root = workspace_root();
    let mut manifests = Vec::new();
    collect(&root.join("plugins"), &mut manifests);
    collect(&root.join("crates"), &mut manifests);
    manifests.sort();

    let mut found = Vec::new();
    let root_manifest = root.join("Cargo.toml");
    let contents = fs::read_to_string(&root_manifest).expect("reading the root manifest");
    found.extend(violations("Cargo.toml", &contents, true));

    for manifest in manifests.iter().filter(|m| !is_exempt(&root, m)) {
        let contents = fs::read_to_string(manifest)
            .unwrap_or_else(|e| panic!("reading {}: {e}", manifest.display()));
        let label = manifest
            .strip_prefix(&root)
            .unwrap_or(manifest)
            .display()
            .to_string();
        found.extend(violations(&label, &contents, false));
    }
    assert!(
        found.is_empty(),
        "library dependencies on frust-ui/frust-text must set `default-features = false` \
         (cargo unifies features, so one default-on edge re-enables `complex-scripts` for the app):\n{}",
        found.join("\n")
    );
}
