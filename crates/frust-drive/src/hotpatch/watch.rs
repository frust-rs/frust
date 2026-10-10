//! The hot watch set and save debounce both front-ends share: `frust run
//! --watch` (`frust-cli`) and the workbench's "Watch: hot patch on save"
//! (`frust-tui`) watch the same paths and settle a save-burst on the same
//! rule, so an edit restarts or patches identically from either.
//!
//! [`watch_set`] reads the paths off a [`WorkspaceGraph`] by the classes
//! [`WorkspaceGraph::classify`] answers with ([`PathClass`]): the `src/`
//! trees of workspace members (replayable) and of local non-member path
//! packages (replayed when captured, restart otherwise), the directory of
//! every lib/bin target root that sits outside `src/`, and the build inputs
//! (restart). This module only names paths — the `notify` watcher and the
//! debounce thread stay with each front-end, so `frust-drive` carries no
//! filesystem-watch dependency.
//!
//! [`is_relevant`] is THE noise rule: both front-ends' watcher filters and
//! [`changed_since`] delegate to it.
//!
//! [`changed_since`] answers the same set without a watcher: every source
//! file and build input modified at or after a point in time — what a
//! manual "hot patch now" replays when no save-burst named the paths.
//!
//! [`PathClass`]: super::graph::PathClass

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::graph::{TargetRole, WorkspaceGraph};

/// Debounce window for source changes, applied on the **trailing edge**:
/// after the first raw change tick, keep consuming ticks that arrive within
/// this window of the previous one, and act only once the tree has been quiet
/// for that long. A save that fires several raw filesystem events (an
/// editor's rename-then-write, a formatter's follow-up write, …) so triggers
/// one patch or relaunch, not several. 100 ms covers atomic-save and
/// format-on-save bursts, and measured milestone-1 steady-state
/// save->`on_change` latency at 312–324 ms.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// What the hot watcher registers, by the session graph's path classes. The
/// directories are watched recursively, the files individually; a path that
/// does not exist is skipped by the real watcher, not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchSet {
    /// `src/` of every workspace member: a thin build can patch these.
    pub replayable: Vec<PathBuf>,
    /// `src/` of every local path package outside the workspace. A captured
    /// non-member (desktop) replays like a member; an uncaptured one (and
    /// every non-member on a device) only a fat rebuild picks up, so the
    /// session answers with a restart.
    pub local_non_member: Vec<PathBuf>,
    /// Manifests, build scripts, the lockfile, cargo config and toolchain
    /// files: any change is a restart. Also the lib/bin target root files
    /// that sit directly in a package directory (`classify` answers those
    /// by their own class), watched as single files.
    pub build_inputs: Vec<PathBuf>,
    /// What a changed path is judged relative to by each front-end's noise
    /// filter: the workspace root and every package directory — never a
    /// `src/` tree, whose own `build/` or `target/` module is source.
    pub roots: Vec<PathBuf>,
}

impl WatchSet {
    /// Every directory to watch recursively.
    pub fn dirs(&self) -> impl Iterator<Item = &PathBuf> {
        self.replayable.iter().chain(&self.local_non_member)
    }
}

/// Derives the [`WatchSet`] from the session's workspace graph: member and
/// non-member `src/` trees, the directory of every lib/bin target root that
/// lies outside `src/` (the root file itself when it sits in the package
/// directory), each package's manifest and build script plus the
/// workspace-level build inputs, and the package directories paths are
/// judged relative to. Equivalent to [`watch_set_in`] with the workspace
/// root as the project root.
pub fn watch_set(graph: &WorkspaceGraph) -> WatchSet {
    watch_set_in(graph, graph.workspace_root())
}

/// [`watch_set`] for a project whose root (where the tool was started) may
/// differ from the workspace root: the project root's toolchain files and
/// `.cargo` config are build inputs too, and the project root joins
/// [`WatchSet::roots`].
pub fn watch_set_in(graph: &WorkspaceGraph, project_root: &Path) -> WatchSet {
    let mut set = WatchSet::default();
    let mut inputs = BTreeSet::new();
    set.roots.push(graph.workspace_root().to_path_buf());
    for package in graph.packages() {
        let src = package.dir.join("src");
        let trees = if package.member {
            &mut set.replayable
        } else {
            &mut set.local_non_member
        };
        push_unique(trees, src.clone());
        for target in &package.targets {
            if target.role == TargetRole::Other {
                continue;
            }
            let Some(parent) = target.src_path.parent() else {
                continue;
            };
            if target.src_path.starts_with(&src) {
                continue;
            }
            if target.role == TargetRole::BuildScript || parent == package.dir {
                inputs.insert(target.src_path.clone());
            } else {
                push_unique(trees, parent.to_path_buf());
            }
        }
        push_unique(&mut set.roots, package.dir.clone());
        inputs.insert(package.dir.join("Cargo.toml"));
        inputs.insert(package.dir.join("build.rs"));
    }
    for name in ROOT_BUILD_INPUTS {
        inputs.insert(graph.workspace_root().join(name));
    }
    if project_root != graph.workspace_root() {
        push_unique(&mut set.roots, project_root.to_path_buf());
        for name in PROJECT_BUILD_INPUTS {
            inputs.insert(project_root.join(name));
        }
    }
    set.build_inputs = inputs.into_iter().collect();
    set
}

/// The build inputs the workspace root can carry.
const ROOT_BUILD_INPUTS: [&str; 7] = [
    "Cargo.toml",
    "Cargo.lock",
    "frust.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config",
    ".cargo/config.toml",
];

/// The build inputs a project root below the workspace root adds: cargo and
/// rustup find these walking up from where they run.
const PROJECT_BUILD_INPUTS: [&str; 4] = [
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config",
    ".cargo/config.toml",
];

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

/// What [`changed_since`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// The changed files, sorted and deduplicated.
    pub changed: Vec<PathBuf>,
    /// Entries the scan could not read (a listing or metadata call failed
    /// for a reason other than the entry not existing), sorted and
    /// deduplicated: the scan went on without them, so the caller can warn.
    pub skipped: Vec<PathBuf>,
}

/// Every regular file of `set` modified at or after `since`: the files
/// under its `src/` trees ([`WatchSet::dirs`], walked recursively) plus its
/// build inputs, each checked as one file.
///
/// The walk applies [`is_relevant`], judged relative to the deepest of
/// [`WatchSet::roots`] a path lies under: nothing under a `target/` or
/// `build/` directory there, no hidden entry (bar a root's own
/// `.cargo/config(.toml)`, which is a build input anyway), and no
/// `~`-suffixed editor backup. A directory or build input that does not
/// exist, or vanishes mid-walk, is skipped silently; a symlinked directory
/// is not followed. An entry whose listing or metadata fails for any other
/// reason (permissions, say) is skipped too and reported in
/// [`Scan::skipped`], and the walk goes on.
pub fn changed_since(set: &WatchSet, since: SystemTime) -> Scan {
    let mut changed = BTreeSet::new();
    let mut skipped = BTreeSet::new();
    for dir in set.dirs() {
        scan_dir(&set.roots, dir, since, &mut changed, &mut skipped);
    }
    for file in &set.build_inputs {
        check_file(file, since, &mut changed, &mut skipped);
    }
    Scan {
        changed: changed.into_iter().collect(),
        skipped: skipped.into_iter().collect(),
    }
}

/// [`changed_since`]'s recursive walk of one directory.
fn scan_dir(
    roots: &[PathBuf],
    dir: &Path,
    since: SystemTime,
    changed: &mut BTreeSet<PathBuf>,
    skipped: &mut BTreeSet<PathBuf>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return,
        Err(_) => {
            skipped.insert(dir.to_path_buf());
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            // A failed listing step names no entry: the directory is the
            // closest thing to report.
            skipped.insert(dir.to_path_buf());
            continue;
        };
        let path = entry.path();
        if !is_relevant(roots, &path) {
            continue;
        }
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {
                scan_dir(roots, &path, since, changed, skipped);
            }
            Ok(_) => check_file(&path, since, changed, skipped),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                skipped.insert(path);
            }
        }
    }
}

/// Records `path` in `changed` when it is a regular file (symlinks followed)
/// modified at or after `since`; a missing one is nothing, an unreadable one
/// goes in `skipped`.
fn check_file(
    path: &Path,
    since: SystemTime,
    changed: &mut BTreeSet<PathBuf>,
    skipped: &mut BTreeSet<PathBuf>,
) {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => match meta.modified() {
            Ok(modified) if modified >= since => {
                changed.insert(path.to_path_buf());
            }
            Ok(_) => {}
            Err(_) => {
                skipped.insert(path.to_path_buf());
            }
        },
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            skipped.insert(path.to_path_buf());
        }
    }
}

/// Whether `path` is source rather than noise, judged relative to the
/// deepest root it lies under: no `target`/`build` first component, no
/// hidden component bar a root's own `.cargo/config(.toml)`, no `~`-suffixed
/// backup. THE relevance rule: `frust run --watch`, the workbench watcher
/// and [`changed_since`] all apply this one function.
pub fn is_relevant(roots: &[PathBuf], path: &Path) -> bool {
    let rel = roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|rel| rel.components().count())
        .unwrap_or(path);
    let names: Vec<String> = rel
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let Some(first) = names.first() else {
        return true;
    };
    if first == "target" || first == "build" {
        return false;
    }
    if let [dir, file] = names.as_slice()
        && dir == ".cargo"
        && (file == "config" || file == "config.toml")
    {
        return true;
    }
    if names.iter().any(|name| name.starts_with('.')) {
        return false;
    }
    !names.last().is_some_and(|last| last.ends_with('~'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace at `/w` whose member `app` (lib + bin + build script)
    /// depends on the local non-member path package `frust-material` and the
    /// registry crate `serde`.
    const METADATA: &str = r#"{
        "packages": [
            {"id": "path+file:///w/app#0.1.0", "name": "app", "source": null,
             "manifest_path": "/w/app/Cargo.toml",
             "targets": [
                {"name": "app", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/w/app/src/lib.rs"},
                {"name": "app", "kind": ["bin"], "crate_types": ["bin"], "src_path": "/w/app/src/main.rs"},
                {"name": "build-script-build", "kind": ["custom-build"], "crate_types": ["bin"], "src_path": "/w/app/build.rs"}]},
            {"id": "path+file:///x/material#0.6.0", "name": "frust-material", "source": null,
             "manifest_path": "/x/material/Cargo.toml",
             "targets": [
                {"name": "frust_material", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/x/material/src/lib.rs"}]},
            {"id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0", "name": "serde",
             "source": "registry+https://github.com/rust-lang/crates.io-index",
             "manifest_path": "/registry/serde-1.0.0/Cargo.toml",
             "targets": [
                {"name": "serde", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/registry/serde-1.0.0/src/lib.rs"}]}
        ],
        "workspace_members": ["path+file:///w/app#0.1.0"],
        "resolve": {"nodes": [
            {"id": "path+file:///w/app#0.1.0", "deps": [
                {"name": "frust_material", "pkg": "path+file:///x/material#0.6.0",
                 "dep_kinds": [{"kind": null, "target": null}]},
                {"name": "serde", "pkg": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0",
                 "dep_kinds": [{"kind": null, "target": null}]}]},
            {"id": "path+file:///x/material#0.6.0", "deps": []},
            {"id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0", "deps": []}],
         "root": "path+file:///w/app#0.1.0"},
        "workspace_root": "/w"
    }"#;

    fn fixture() -> WatchSet {
        let graph = WorkspaceGraph::from_metadata(METADATA, "app", None).unwrap();
        watch_set(&graph)
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn members_src_trees_are_the_replayable_class() {
        assert_eq!(fixture().replayable, paths(&["/w/app/src"]));
    }

    #[test]
    fn a_local_non_member_path_package_is_watched_whole_src_and_a_registry_crate_never() {
        let set = fixture();
        assert_eq!(set.local_non_member, paths(&["/x/material/src"]));
        assert!(
            !set.dirs()
                .chain(&set.build_inputs)
                .chain(&set.roots)
                .any(|path| path.starts_with("/registry")),
            "a registry crate is never watched: {set:?}"
        );
    }

    #[test]
    fn the_build_inputs_are_every_manifest_build_script_lockfile_config_and_toolchain_file() {
        assert_eq!(
            fixture().build_inputs,
            paths(&[
                "/w/.cargo/config",
                "/w/.cargo/config.toml",
                "/w/Cargo.lock",
                "/w/Cargo.toml",
                "/w/app/Cargo.toml",
                "/w/app/build.rs",
                "/w/frust.toml",
                "/w/rust-toolchain",
                "/w/rust-toolchain.toml",
                "/x/material/Cargo.toml",
                "/x/material/build.rs",
            ])
        );
    }

    #[test]
    fn the_roots_are_the_workspace_root_then_every_package_directory() {
        assert_eq!(fixture().roots, paths(&["/w", "/w/app", "/x/material"]));
    }

    #[test]
    fn dirs_lists_the_replayable_then_the_non_member_src_trees() {
        let set = fixture();
        let dirs: Vec<PathBuf> = set.dirs().cloned().collect();
        assert_eq!(dirs, paths(&["/w/app/src", "/x/material/src"]));
        assert!(
            !dirs.iter().any(|dir| dir.ends_with("target")),
            "no build output directory is watched"
        );
    }

    #[test]
    fn the_debounce_window_is_one_hundred_milliseconds() {
        assert_eq!(WATCH_DEBOUNCE, Duration::from_millis(100));
    }

    /// A workspace at `/w` whose member `app` keeps its lib root in `core/`
    /// and one bin root directly in the package directory, and whose
    /// non-member `tool` keeps its lib root in `crate/`.
    const OFFSET_ROOTS: &str = r#"{
        "packages": [
            {"id": "path+file:///w/app#0.1.0", "name": "app", "source": null,
             "manifest_path": "/w/app/Cargo.toml",
             "targets": [
                {"name": "app", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/w/app/core/lib.rs"},
                {"name": "app", "kind": ["bin"], "crate_types": ["bin"], "src_path": "/w/app/main.rs"},
                {"name": "t", "kind": ["test"], "crate_types": ["bin"], "src_path": "/w/app/tests/t.rs"}]},
            {"id": "path+file:///x/tool#0.1.0", "name": "tool", "source": null,
             "manifest_path": "/x/tool/Cargo.toml",
             "targets": [
                {"name": "tool", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/x/tool/crate/lib.rs"}]}
        ],
        "workspace_members": ["path+file:///w/app#0.1.0"],
        "resolve": {"nodes": [
            {"id": "path+file:///w/app#0.1.0", "deps": [
                {"name": "tool", "pkg": "path+file:///x/tool#0.1.0",
                 "dep_kinds": [{"kind": null, "target": null}]}]},
            {"id": "path+file:///x/tool#0.1.0", "deps": []}],
         "root": "path+file:///w/app#0.1.0"},
        "workspace_root": "/w"
    }"#;

    #[test]
    fn a_target_root_outside_src_is_watched_as_its_directory_or_as_the_file_in_the_package_dir() {
        let graph = WorkspaceGraph::from_metadata(OFFSET_ROOTS, "app", None).unwrap();
        let set = watch_set(&graph);
        assert_eq!(set.replayable, paths(&["/w/app/src", "/w/app/core"]));
        assert_eq!(
            set.local_non_member,
            paths(&["/x/tool/src", "/x/tool/crate"])
        );
        assert!(set.build_inputs.contains(&PathBuf::from("/w/app/main.rs")));
        assert!(
            !set.dirs().any(|dir| dir.ends_with("tests")),
            "a test root is never part of the running image: {set:?}"
        );
    }

    #[test]
    fn every_path_classify_calls_replayable_or_a_build_input_lies_under_the_watch_set() {
        use super::super::graph::PathClass;
        let graph = WorkspaceGraph::from_metadata(OFFSET_ROOTS, "app", None).unwrap();
        let set = watch_set(&graph);
        let covered = |path: &Path| {
            set.dirs().any(|dir| path.starts_with(dir))
                || set.build_inputs.iter().any(|f| f == path)
        };
        for path in [
            "/w/app/core/lib.rs",
            "/w/app/core/ui/mod.rs",
            "/w/app/main.rs",
            "/w/app/src/extra.rs",
            "/w/app/Cargo.toml",
            "/w/rust-toolchain.toml",
            "/x/tool/crate/lib.rs",
        ] {
            let class = graph.classify(Path::new(path));
            if matches!(
                class,
                PathClass::Replayable { .. } | PathClass::BuildInput { .. }
            ) {
                assert!(
                    covered(Path::new(path)),
                    "{path} ({class:?}) is not watched"
                );
            }
        }
    }

    #[test]
    fn a_project_root_below_the_workspace_root_adds_its_toolchain_and_cargo_config() {
        let graph = WorkspaceGraph::from_metadata(METADATA, "app", None).unwrap();
        let set = watch_set_in(&graph, Path::new("/w/app"));
        for name in [
            "/w/app/rust-toolchain",
            "/w/app/rust-toolchain.toml",
            "/w/app/.cargo/config",
            "/w/app/.cargo/config.toml",
        ] {
            assert!(set.build_inputs.contains(&PathBuf::from(name)), "{name}");
        }
        assert!(
            !set.build_inputs
                .contains(&PathBuf::from("/w/app/Cargo.lock"))
        );
        assert_eq!(set.roots, paths(&["/w", "/w/app", "/x/material"]));
        let at_root = watch_set_in(&graph, Path::new("/w"));
        assert_eq!(at_root, watch_set(&graph));
    }

    // ── changed_since ───────────────────────────────────────────────────

    /// A fresh, empty scratch directory for one test.
    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-hotpatch-watch-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write `path` (creating its parents) and stamp its mtime at `at`.
    fn touch(path: &Path, at: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(path).unwrap();
        file.set_modified(at).unwrap();
    }

    /// A single-package set rooted at `root`: `src/` replayable, the
    /// manifest and a lockfile as build inputs.
    fn single_package(root: &Path) -> WatchSet {
        WatchSet {
            replayable: vec![root.join("src")],
            local_non_member: Vec::new(),
            build_inputs: vec![
                root.join("Cargo.lock"),
                root.join("Cargo.toml"),
                root.join("build.rs"),
            ],
            roots: vec![root.to_path_buf()],
        }
    }

    #[test]
    fn changed_since_reports_newer_and_equal_files_but_not_older_ones() {
        let root = scratch("ages");
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let older = since - Duration::from_secs(5);
        let newer = since + Duration::from_secs(5);
        touch(&root.join("src/old.rs"), older);
        touch(&root.join("src/same.rs"), since);
        touch(&root.join("src/ui/new.rs"), newer);

        let changed = changed_since(&single_package(&root), since).changed;

        assert_eq!(
            changed,
            vec![root.join("src/same.rs"), root.join("src/ui/new.rs")]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn changed_since_checks_the_build_inputs_as_files_and_ignores_a_missing_one() {
        let root = scratch("inputs");
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        touch(&root.join("Cargo.toml"), since + Duration::from_secs(1));
        touch(&root.join("Cargo.lock"), since - Duration::from_secs(1));
        // `build.rs` is named by the set but does not exist; `src/` neither.

        let changed = changed_since(&single_package(&root), since).changed;

        assert_eq!(changed, vec![root.join("Cargo.toml")]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn changed_since_skips_build_output_hidden_entries_and_backups() {
        let root = scratch("noise");
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let newer = since + Duration::from_secs(5);
        // A `target/` tree inside a walked directory, judged against the
        // package root it sits directly under, is build output.
        let mut set = single_package(&root);
        set.replayable.push(root.join("target"));
        touch(&root.join("target/debug/app.d"), newer);
        touch(&root.join("src/.lib.rs.swp"), newer);
        touch(&root.join("src/.hidden/mod.rs"), newer);
        touch(&root.join("src/lib.rs~"), newer);
        // A directory merely named `target` or `build` inside `src/` is source.
        touch(&root.join("src/target/mod.rs"), newer);
        touch(&root.join("src/build/mod.rs"), newer);

        let changed = changed_since(&set, since).changed;

        assert_eq!(
            changed,
            vec![
                root.join("src/build/mod.rs"),
                root.join("src/target/mod.rs")
            ]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn changed_since_walks_non_member_trees_and_deduplicates_a_path_named_twice() {
        let root = scratch("dedup");
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let newer = since + Duration::from_secs(5);
        touch(&root.join("app/src/main.rs"), newer);
        touch(&root.join("widgets/src/lib.rs"), newer);
        let set = WatchSet {
            replayable: vec![root.join("app/src"), root.join("app/src")],
            local_non_member: vec![root.join("widgets/src")],
            build_inputs: vec![root.join("app/src/main.rs")],
            roots: vec![root.clone(), root.join("app"), root.join("widgets")],
        };

        let changed = changed_since(&set, since).changed;

        assert_eq!(
            changed,
            vec![
                root.join("app/src/main.rs"),
                root.join("widgets/src/lib.rs")
            ]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn changed_since_skips_an_unreadable_directory_reports_it_and_keeps_scanning() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("unreadable");
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let newer = since + Duration::from_secs(5);
        touch(&root.join("src/ok.rs"), newer);
        touch(&root.join("src/locked/hidden.rs"), newer);
        let locked = root.join("src/locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // A privileged user reads it anyway; the test then has nothing to prove.
        let readable = fs::read_dir(&locked).is_ok();

        let scan = changed_since(&single_package(&root), since);

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(scan.changed.contains(&root.join("src/ok.rs")));
        if !readable {
            assert_eq!(scan.skipped, vec![locked]);
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
