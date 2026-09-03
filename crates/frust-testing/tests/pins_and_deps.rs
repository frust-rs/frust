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
const EXACT_PINNED_ROWS: &[&str] = &["vello_common", "glifo", "vello_cpu_oracle"];

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

/// Every dependency name a manifest declares under `dependencies` or
/// `build-dependencies` — plain or per-target (`target.'cfg(...)'.*`) — as a
/// flat set. Dev-dependencies are never collected (see [`DEP_TABLE_NAMES`]).
fn direct_dependency_names(value: &toml::Value, out: &mut BTreeSet<String>) {
    let toml::Value::Table(table) = value else {
        return;
    };

    for (key, child) in table {
        if DEP_TABLE_NAMES.contains(&key.as_str()) {
            if let toml::Value::Table(deps) = child {
                out.extend(deps.keys().cloned());
            }
        } else {
            // Recurse into `target.'cfg(...)'.<dep-table>` (and any other
            // nested table) looking for more dependency tables further down.
            direct_dependency_names(child, out);
        }
    }
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
