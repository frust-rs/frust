//! The hot watch set and save debounce both front-ends share: `frust run
//! --watch` (`frust-cli`) and the workbench's "Watch: hot patch on save"
//! (`frust-tui`) watch the same paths and settle a save-burst on the same
//! rule, so an edit restarts or patches identically from either.
//!
//! [`watch_set`] reads the paths off a [`WorkspaceGraph`] by the classes
//! [`WorkspaceGraph::classify`] answers with ([`PathClass`]): the `src/`
//! trees of workspace members (replayable) and of local non-member path
//! packages (restart), and the build inputs (restart). This module only
//! names paths — the `notify` watcher and the debounce thread stay with each
//! front-end, so `frust-drive` carries no filesystem-watch dependency.
//!
//! [`PathClass`]: super::graph::PathClass

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use super::graph::WorkspaceGraph;

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
    /// `src/` of every local path package outside the workspace: only a
    /// fat rebuild picks these up, so the session answers with a restart.
    pub local_non_member: Vec<PathBuf>,
    /// Manifests, build scripts, the lockfile, cargo config and toolchain
    /// files: any change is a restart.
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
/// non-member `src/` trees, each package's manifest and build script plus
/// the workspace-level build inputs, and the package directories paths are
/// judged relative to.
pub fn watch_set(graph: &WorkspaceGraph) -> WatchSet {
    let mut set = WatchSet::default();
    let mut inputs = BTreeSet::new();
    set.roots.push(graph.workspace_root().to_path_buf());
    for package in graph.packages() {
        let src = package.dir.join("src");
        if package.member {
            set.replayable.push(src);
        } else {
            set.local_non_member.push(src);
        }
        set.roots.push(package.dir.clone());
        inputs.insert(package.dir.join("Cargo.toml"));
        inputs.insert(package.dir.join("build.rs"));
    }
    let root = graph.workspace_root();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "frust.toml",
        "rust-toolchain",
        "rust-toolchain.toml",
        ".cargo/config",
        ".cargo/config.toml",
    ] {
        inputs.insert(root.join(name));
    }
    set.build_inputs = inputs.into_iter().collect();
    set
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
}
