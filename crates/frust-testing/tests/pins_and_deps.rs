//! Guards G5 and G7: the engine-core version pins stay exact, and the
//! engine-tier LAYER boundary (`docs/ARCHITECTURE.md`'s Cross-Unit Layer
//! Dependencies / `docs/RENDER_ARCHITECTURE.md`'s Layer Dependencies) stays a
//! one-way edge no manifest can reopen silently. "Engine-tier" here is the
//! name of that layer rule — the `frust-engine`/`frust-gpu` tier of the
//! dependency graph — never a cargo feature: the feature of that name was
//! retired when the engine became the only renderer, and this guard is about
//! the direction of an edge, not about how one is switched on.
//!
//! Both are plain manifest scans run as ordinary `cargo test` (this repo has
//! no lint-plugin tooling — see `docs/CODE_STANDARDS.md`; precedent:
//! `crates/frust-testing/tests/deps.rs` and `tests/dup_identities.rs`, which
//! already cover this same directory's manifests for a different guard).
//!
//! - **G5**: the root `[workspace.dependencies]` rows for `vello_common`,
//!   `glifo`, and `vello_cpu_oracle` each carry a
//!   literal `=` version — the engine-core render stack is exact-pinned,
//!   pre-1.0 and unstable, per `docs/RENDER_DEVELOPMENT.md`'s Version Pins.
//!   `parley_engine`, `parlance` and `fontique` (frust-text's font stack) are exact too.
//!   (The legacy `vello_cpu` row — the cpu-tier fallback's own pin — was
//!   retired alongside `frust-render`'s `cpu-tier` feature.)
//! - **G7**: `frust-engine` never depends on `vello`/`vello_cpu`/
//!   `frust-render`/`parley`; `frust-gpu` never depends on
//!   `frust-scene`/`frust-core`/`frust-widgets`/`frust-engine`; and no crate
//!   above RENDER (`frust-core`, `frust-widgets`, `frust`, every
//!   `plugins/*`) depends on `frust-engine` or `frust-gpu` — with one
//!   sanctioned exception: `crates/frust` may carry an **optional**,
//!   **non-default** `frust-gpu` edge (the feature-gated `frust::gpu`
//!   re-export), never `frust-engine`. See
//!   `g7_nothing_above_render_depends_on_the_engine_tier_layer` below for
//!   both halves of that carve-out.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The repository root, resolved from this crate's manifest dir
/// (`crates/frust-testing`) so the scan is working-directory-independent —
/// the same resolution `tests/deps.rs`/`tests/dup_identities.rs` use.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust-testing has two ancestors: crates/, then the repo root")
        .to_path_buf()
}

fn parse_manifest(path: &Path) -> toml::Table {
    let contents =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    contents
        .parse()
        .unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

// ---------------------------------------------------------------------
// G5: exact pins on the engine-core render stack
// ---------------------------------------------------------------------

/// Every `[workspace.dependencies]` row this guard requires to carry a
/// literal `=` version — the engine-core render stack's exact-pinned rows
/// (`docs/RENDER_DEVELOPMENT.md`'s Version Pins). The legacy `vello_cpu` row
/// (the retired cpu-tier fallback's own pin) is gone, not merely renamed:
/// `vello_cpu_oracle` predates its retirement and stays the CPU oracle's own
/// row.
const EXACT_PINNED_ROWS: &[&str] = &[
    "vello_common",
    "glifo",
    "vello_cpu_oracle",
    "parley_engine",
    "parlance",
    "fontique",
];

/// The version requirement string of `[workspace.dependencies].<name>`,
/// whether the row is a bare string (`name = "=0.1.0"`) or a table
/// (`name = { version = "=0.1.0", ... }`).
fn workspace_dependency_version(table: &toml::Table, name: &str) -> String {
    let workspace = table
        .get("workspace")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("root Cargo.toml has no [workspace] table"));
    let deps = workspace
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("root Cargo.toml has no [workspace.dependencies] table"));
    let row = deps
        .get(name)
        .unwrap_or_else(|| panic!("[workspace.dependencies] has no `{name}` row"));
    match row {
        toml::Value::String(version) => version.clone(),
        toml::Value::Table(row_table) => row_table
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("[workspace.dependencies].{name} has no `version` key"))
            .to_string(),
        other => {
            panic!("[workspace.dependencies].{name} is neither a string nor a table: {other:?}")
        }
    }
}

#[test]
fn g5_engine_core_pins_carry_a_literal_exact_version() {
    let root = repo_root();
    let table = parse_manifest(&root.join("Cargo.toml"));

    let mut failures = Vec::new();
    for name in EXACT_PINNED_ROWS {
        let version = workspace_dependency_version(&table, name);
        if !version.starts_with('=') {
            failures.push(format!(
                "[workspace.dependencies].{name} = \"{version}\" is not exact-pinned — the \
                 engine-core render stack (vello_common/glifo/vello_cpu_oracle) \
                 must carry a literal `=` version (docs/RENDER_DEVELOPMENT.md's \
                 Version Pins)"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "engine-core pin(s) drifted off exact:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------
// G7: the engine-tier LAYER's dependency-direction boundary
// (the `frust-engine`/`frust-gpu` tier of the graph — not a cargo feature)
// ---------------------------------------------------------------------

/// The dependency-table names Cargo recognizes as ordinary (non-dev) edges —
/// `dev-dependencies` is deliberately excluded: a dev-only edge never reaches
/// a shipped app's graph, so it cannot reopen the layering boundary this
/// guard protects (the same distinction `tests/deps.rs` draws the other way).
const DEP_TABLE_NAMES: &[&str] = &["dependencies", "build-dependencies"];

/// Every dependency-table name Cargo recognizes, dev included. The parley and
/// fontique scans use this wider set: a dev-only edge still resolves a crate
/// into the lock, so it can still reintroduce a retired identity.
const ALL_DEP_TABLE_NAMES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// The package a dependency row resolves to: the `package` key when the row
/// renames the crate (`alias = { package = "real", ... }`), otherwise the key.
fn effective_package<'a>(alias: &'a str, row: &'a toml::Value) -> &'a str {
    row.as_table()
        .and_then(|table| table.get("package"))
        .and_then(toml::Value::as_str)
        .unwrap_or(alias)
}

/// Calls `visit(alias, row)` for every row under a dependency table named in
/// `table_names` — plain or per-target (`target.'cfg(...)'.*`), reached by
/// recursing through any other nested table. `[workspace.dependencies]` is
/// reached too, unless `skip_workspace` is set.
fn visit_dependency_rows(
    value: &toml::Value,
    table_names: &[&str],
    skip_workspace: bool,
    visit: &mut dyn FnMut(&str, &toml::Value),
) {
    let toml::Value::Table(table) = value else {
        return;
    };

    for (key, child) in table {
        if skip_workspace && key == "workspace" {
            continue;
        }
        if table_names.contains(&key.as_str()) {
            if let toml::Value::Table(deps) = child {
                for (alias, row) in deps {
                    visit(alias, row);
                }
            }
        } else {
            visit_dependency_rows(child, table_names, skip_workspace, visit);
        }
    }
}

/// Every dependency name a manifest declares under `dependencies` or
/// `build-dependencies` — plain or per-target — as a flat set. Each row
/// contributes both its key and its `package` value, so a renamed row
/// (`alias = { package = "x" }`) is seen under both names. Dev-dependencies
/// are never collected (see [`DEP_TABLE_NAMES`]).
fn direct_dependency_names(value: &toml::Value, out: &mut BTreeSet<String>) {
    visit_dependency_rows(value, DEP_TABLE_NAMES, false, &mut |alias, row| {
        out.insert(alias.to_string());
        out.insert(effective_package(alias, row).to_string());
    });
}

fn manifest_dependency_names(path: &Path) -> BTreeSet<String> {
    let table = parse_manifest(path);
    let mut out = BTreeSet::new();
    direct_dependency_names(&toml::Value::Table(table), &mut out);
    out
}

/// Every `<dir>/*/Cargo.toml` under `root`, one level deep, sorted for stable
/// failure output — the same shape `tests/deps.rs`'s `member_manifests` uses.
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

fn assert_none_depend_on(manifest_path: &Path, forbidden: &[&str]) {
    let deps = manifest_dependency_names(manifest_path);
    let root = repo_root();
    let relative = manifest_path.strip_prefix(&root).unwrap_or(manifest_path);
    for name in forbidden {
        assert!(
            !deps.contains(*name),
            "{}: [dependencies]/[build-dependencies] must not name `{name}` — the \
             engine-tier layer boundary (docs/ARCHITECTURE.md's Cross-Unit Layer Dependencies) \
             is a one-way edge",
            relative.display()
        );
    }
}

#[test]
fn g7_frust_engine_never_depends_on_the_legacy_render_stack() {
    let root = repo_root();
    assert_none_depend_on(
        &root.join("crates/frust-engine/Cargo.toml"),
        &["vello", "vello_cpu", "frust-render", "parley"],
    );
}

#[test]
fn g7_frust_gpu_never_depends_on_a_layer_above_it() {
    let root = repo_root();
    assert_none_depend_on(
        &root.join("crates/frust-gpu/Cargo.toml"),
        &["frust-scene", "frust-core", "frust-widgets", "frust-engine"],
    );
}

#[test]
fn g7_nothing_above_render_depends_on_the_engine_tier_layer() {
    let root = repo_root();

    let mut manifests = vec![
        root.join("crates/frust-core/Cargo.toml"),
        root.join("crates/frust-widgets/Cargo.toml"),
    ];
    manifests.extend(member_manifests(&root, "plugins"));
    assert!(
        manifests.len() > 2,
        "expected at least one plugins/*/Cargo.toml under {}",
        root.display()
    );

    for manifest_path in &manifests {
        assert_none_depend_on(manifest_path, &["frust-engine", "frust-gpu"]);
    }

    // `crates/frust` carries the one sanctioned exception: an optional,
    // non-default `frust-gpu` edge behind the facade's `gpu` feature
    // (`frust::gpu`, see `crates/frust/src/lib.rs`) so an app — or a future
    // 3D-rendering crate sitting beside the facade — can reach the GPU
    // substrate without depending on `frust-render`/`frust-gpu` itself.
    // `frust-engine` stays fully forbidden even here.
    let frust_manifest = root.join("crates/frust/Cargo.toml");
    assert_none_depend_on(&frust_manifest, &["frust-engine"]);
    assert_frust_gpu_edge_is_optional_and_non_default(&frust_manifest);
}

/// Asserts `crates/frust/Cargo.toml`'s `frust-gpu` dependency (the sanctioned
/// GPU-substrate exception above) is declared `optional = true` and that no
/// `[features].default` entry turns it on — the two guarantees that keep the
/// carve-out from silently widening into an unconditional edge every build
/// carries.
fn assert_frust_gpu_edge_is_optional_and_non_default(manifest_path: &Path) {
    let table = parse_manifest(manifest_path);
    let relative = manifest_path
        .strip_prefix(repo_root())
        .unwrap_or(manifest_path);

    let deps = table
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("{}: has no [dependencies] table", relative.display()));
    let frust_gpu = deps.get("frust-gpu").unwrap_or_else(|| {
        panic!(
            "{}: expected a `frust-gpu` row under [dependencies] — the sanctioned \
             GPU-substrate re-export edge (`frust::gpu`); if this crate no longer carries \
             the seam, drop this assertion too",
            relative.display()
        )
    });
    let is_optional = matches!(
        frust_gpu,
        toml::Value::Table(row) if row.get("optional").and_then(toml::Value::as_bool) == Some(true)
    );
    assert!(
        is_optional,
        "{}: `frust-gpu` must be declared `optional = true` — an unconditional edge would \
         reopen the engine-tier layer boundary for every build, not just a `gpu`-featured one",
        relative.display()
    );

    let default_features: Vec<String> = table
        .get("features")
        .and_then(toml::Value::as_table)
        .and_then(|features| features.get("default"))
        .and_then(toml::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        default_features
            .iter()
            .all(|entry| entry != "dep:frust-gpu" && entry != "gpu"),
        "{}: [features].default must not enable `gpu`/`dep:frust-gpu` — the GPU substrate \
         seam must stay opt-in",
        relative.display()
    );
}

// ---------------------------------------------------------------------
// G7: the parley crate is gone; fontique rides parley_engine
// ---------------------------------------------------------------------

/// Every file named `file_name` under `dir`, skipping build output, VCS state,
/// the nested `workflow/` repo and scratch directories. The manifest walk and
/// the lockfile walk share this, so both cover the same tree.
fn files_named(dir: &Path, file_name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(
                name.as_ref(),
                "target" | ".git" | "node_modules" | "workflow" | "tmp" | ".claude"
            ) {
                continue;
            }
            files_named(&path, file_name, out);
        } else if name == file_name {
            out.push(path);
        }
    }
}

/// Every `Cargo.toml` in the repository (root, members, standalone workspaces).
fn all_manifests() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = Vec::new();
    files_named(&root, "Cargo.toml", &mut out);
    out.sort();
    out
}

/// Every `Cargo.lock` in the repository: the root lock plus each standalone
/// workspace's own. Found by the same walk as the manifests, so a new
/// standalone lock is covered without a hand-kept list.
fn all_lockfiles() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = Vec::new();
    files_named(&root, "Cargo.lock", &mut out);
    out.sort();
    out
}

/// The fewest lockfiles the walk may find: the root plus 13 standalone
/// workspaces today (14 in all). A walk that finds fewer has stopped reading
/// the tree, and the lock-level guards below would pass vacuously.
const MIN_LOCKFILES: usize = 10;

/// [`all_lockfiles`], asserted to reach [`MIN_LOCKFILES`].
fn walked_lockfiles() -> Vec<PathBuf> {
    let locks = all_lockfiles();
    assert!(
        locks.len() >= MIN_LOCKFILES,
        "lockfile walk found {} Cargo.lock file(s), fewer than {MIN_LOCKFILES}: {locks:?}",
        locks.len()
    );
    locks
}

fn display_relative(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[test]
fn g7_no_manifest_in_the_workspace_names_parley() {
    let manifests = all_manifests();
    assert!(manifests.len() > 10, "manifest walk found too few files");
    for manifest in &manifests {
        let table = parse_manifest(manifest);
        let mut offending = Vec::new();
        visit_dependency_rows(
            &toml::Value::Table(table),
            ALL_DEP_TABLE_NAMES,
            false,
            &mut |alias: &str, row: &toml::Value| {
                let package = effective_package(alias, row);
                if alias == "parley" || package == "parley" {
                    offending.push(format!("`{alias}` (package `{package}`)"));
                }
            },
        );
        assert!(
            offending.is_empty(),
            "{}: dependency row(s) {} name `parley` — frust-text owns shaping and layout \
             over parley_engine",
            display_relative(manifest),
            offending.join(", ")
        );
    }
}

/// `(name, version, dependency names)` for every package in the lock at `path`.
fn lock_packages_with_deps(path: &Path) -> Vec<(String, String, Vec<String>)> {
    let lock = parse_manifest(path);
    lock.get("package")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("{} has no [[package]] entries", display_relative(path)))
        .iter()
        .map(|p| {
            let field = |k: &str| {
                p.get(k)
                    .and_then(toml::Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let deps = p
                .get("dependencies")
                .and_then(toml::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(toml::Value::as_str)
                        // "name version" when the lock disambiguates; else bare name.
                        .map(|d| d.split(' ').next().unwrap_or(d).to_string())
                        .collect()
                })
                .unwrap_or_default();
            (field("name"), field("version"), deps)
        })
        .collect()
}

#[test]
fn g7_no_lockfile_resolves_parley() {
    for lock in &walked_lockfiles() {
        let packages = lock_packages_with_deps(lock);
        assert!(
            packages.iter().all(|(name, _, _)| name != "parley"),
            "{} still resolves `parley` — frust-text owns shaping and layout over parley_engine",
            display_relative(lock)
        );
    }
}

#[test]
fn g7_parley_is_absent_and_the_font_stack_resolves_one_identity() {
    let root_lock = repo_root().join("Cargo.lock");
    for lock in &walked_lockfiles() {
        let packages = lock_packages_with_deps(lock);
        let relative = display_relative(lock);
        for name in ["parley_engine", "fontique", "parlance"] {
            let versions: Vec<&str> = packages
                .iter()
                .filter(|(n, _, _)| n == name)
                .map(|(_, v, _)| v.as_str())
                .collect();
            // The root lock must resolve each font-stack crate exactly once.
            // A standalone lock that never reaches the font stack has none, so
            // only a split identity (two entries of one name) fails there.
            if *lock == root_lock {
                assert_eq!(
                    versions.len(),
                    1,
                    "{relative}: `{name}` must resolve one identity: {versions:?}"
                );
            } else {
                assert!(
                    versions.len() <= 1,
                    "{relative}: `{name}` resolves more than one identity: {versions:?}"
                );
            }
        }
        // fontique is reached only through parley_engine and frust-text's own font-data edge.
        for (name, _, deps) in &packages {
            if deps.iter().any(|d| d == "fontique") {
                assert!(
                    matches!(name.as_str(), "parley_engine" | "frust-text"),
                    "{relative}: `{name}` depends on fontique; only parley_engine and frust-text may"
                );
            }
        }
    }
}

#[test]
fn g7_fontique_row_has_default_features_off() {
    let table = parse_manifest(&repo_root().join("Cargo.toml"));
    let row = table["workspace"]["dependencies"]["fontique"]
        .as_table()
        .expect("fontique is a table row");
    assert_eq!(
        row.get("default-features").and_then(toml::Value::as_bool),
        Some(false),
        "[workspace.dependencies].fontique must set default-features = false"
    );
}

#[test]
fn g7_every_manifest_naming_fontique_inherits_the_workspace_row() {
    let root = repo_root().join("Cargo.toml");
    let manifests = all_manifests();
    let mut named = 0usize;
    let mut offenders = Vec::new();
    for manifest in &manifests {
        let table = parse_manifest(manifest);
        let is_root = *manifest == root;
        // Member dependency tables. `[workspace]` is skipped here and read below.
        visit_dependency_rows(
            &toml::Value::Table(table.clone()),
            ALL_DEP_TABLE_NAMES,
            true,
            &mut |alias: &str, row: &toml::Value| {
                if effective_package(alias, row) != "fontique" {
                    return;
                }
                named += 1;
                let inherits = row
                    .as_table()
                    .and_then(|table| table.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    == Some(true);
                if !inherits {
                    offenders.push(format!(
                        "{}: `{alias}` names fontique without `workspace = true`",
                        display_relative(manifest)
                    ));
                }
            },
        );
        // `[workspace.dependencies]`. The root's own fontique row is the canonical
        // one, asserted by g7_fontique_row_has_default_features_off; any other
        // such row (a standalone workspace's) must match it, default-features off.
        if let Some(workspace) = table.get("workspace") {
            visit_dependency_rows(
                workspace,
                &["dependencies"],
                false,
                &mut |alias: &str, row: &toml::Value| {
                    if effective_package(alias, row) != "fontique" || is_root {
                        return;
                    }
                    named += 1;
                    let default_off = row
                        .as_table()
                        .and_then(|table| table.get("default-features"))
                        .and_then(toml::Value::as_bool)
                        == Some(false);
                    if !default_off {
                        offenders.push(format!(
                            "{}: `{alias}` is a second [workspace.dependencies] fontique row \
                             without `default-features = false`",
                            display_relative(manifest)
                        ));
                    }
                },
            );
        }
    }
    assert!(
        named > 0,
        "manifest walk found no manifest naming fontique — the scan is not reading the tree"
    );
    assert!(
        offenders.is_empty(),
        "fontique must come from the workspace row (a private row would bypass \
         default-features = false):\n{}",
        offenders.join("\n")
    );
}
