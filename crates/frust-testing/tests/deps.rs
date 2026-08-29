//! Guard G8: `frust-testing` never ships in a production dependency graph.
//!
//! `frust-testing` is `publish = false` and dev-only (see `src/lib.rs`'s
//! module doc) — every crate that wants its golden/oracle/fuzz contracts
//! must take it as a `[dev-dependencies]` edge, never `[dependencies]`,
//! `[build-dependencies]`, or a `[target.'cfg(...)'.*dependencies]` table.
//! Declaring it anywhere else would put it on `cargo tree -p frust -e
//! normal`'s output, exactly the leak this crate's own doc comment promises
//! never happens.
//!
//! This is a plain manifest scan run as an ordinary `cargo test` (this repo
//! has no lint-plugin tooling — see `docs/CODE_STANDARDS.md`; precedent:
//! `crates/frust-drive/tests/print_free_cores.rs`). It parses every
//! `crates/*/Cargo.toml` and `plugins/*/Cargo.toml` and fails if any
//! dependency table other than `dev-dependencies` (or a
//! `target.'cfg(...)'.dev-dependencies`) names `frust-testing`.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_NAME: &str = "frust-testing";

/// The repository root, resolved from this crate's manifest dir
/// (`crates/frust-testing`) so the scan is working-directory-independent.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust-testing has two ancestors: crates/, then the repo root")
        .to_path_buf()
}

/// Every `<dir>/*/Cargo.toml` under `root`, one level deep, sorted for
/// stable failure output.
fn member_manifests(root: &Path, dir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root.join(dir)) else {
        return out;
    };
    for entry in entries.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if manifest.is_file() {
            out.push(manifest);
        }
    }
    out.sort();
    out
}

/// Dependency-table keys a manifest may declare `frust-testing` under
/// without failing this guard: only `dev-dependencies`, plain or per-target.
fn is_dev_only_table(table_path: &[String]) -> bool {
    match table_path {
        [only] => only == "dev-dependencies",
        [first, _cfg, third] => first == "target" && third == "dev-dependencies",
        _ => false,
    }
}

/// Recursively walks `value`'s tables, collecting every `(table_path,
/// dependency_key)` pair where `dependency_key` is a dependency-shaped entry
/// (a string version, or a table with a `version`/`path`/`workspace` key)
/// named `FORBIDDEN_NAME`, under a table path ending in one of the
/// dependency-table names Cargo recognizes.
fn find_forbidden_deps(value: &toml::Value, path: &mut Vec<String>, hits: &mut Vec<Vec<String>>) {
    const DEP_TABLE_NAMES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

    let toml::Value::Table(table) = value else {
        return;
    };

    for (key, child) in table {
        path.push(key.clone());

        let is_dep_table = DEP_TABLE_NAMES.contains(&key.as_str());
        if is_dep_table {
            if let toml::Value::Table(deps) = child
                && deps.contains_key(FORBIDDEN_NAME)
            {
                hits.push(path.clone());
            }
        } else {
            // Recurse into `target.'cfg(...)'.<dep-table>` and any other
            // nested table (e.g. `target`) looking for more dependency
            // tables further down.
            find_forbidden_deps(child, path, hits);
        }

        path.pop();
    }
}

#[test]
fn no_manifest_lists_frust_testing_outside_dev_dependencies() {
    let root = repo_root();
    let mut manifests = member_manifests(&root, "crates");
    manifests.extend(member_manifests(&root, "plugins"));
    assert!(
        !manifests.is_empty(),
        "expected at least one crates/*/Cargo.toml under {}",
        root.display()
    );

    let mut violations = Vec::new();
    for manifest_path in &manifests {
        let contents = fs::read_to_string(manifest_path)
            .unwrap_or_else(|err| panic!("reading {}: {err}", manifest_path.display()));
        let table: toml::Table = contents
            .parse()
            .unwrap_or_else(|err| panic!("parsing {}: {err}", manifest_path.display()));
        let value = toml::Value::Table(table);

        let mut hits = Vec::new();
        find_forbidden_deps(&value, &mut Vec::new(), &mut hits);

        for table_path in hits {
            if !is_dev_only_table(&table_path) {
                violations.push(format!(
                    "{}: [{}] lists `{FORBIDDEN_NAME}`",
                    manifest_path
                        .strip_prefix(&root)
                        .unwrap_or(manifest_path)
                        .display(),
                    table_path.join(".")
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "`{FORBIDDEN_NAME}` must only ever appear under [dev-dependencies]:\n{}",
        violations.join("\n")
    );
}

#[test]
fn own_manifest_is_dev_only_test_infra() {
    let root = repo_root();
    let own_manifest = root.join("crates").join("frust-testing").join("Cargo.toml");
    let contents = fs::read_to_string(&own_manifest)
        .unwrap_or_else(|err| panic!("reading {}: {err}", own_manifest.display()));
    assert!(
        contents.contains("publish = false"),
        "frust-testing's own Cargo.toml must declare `publish = false`"
    );
}
