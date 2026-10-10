//! The workspace graph a hot-patch session replays over, read from
//! `cargo metadata`.
//!
//! **Path classes.** Every watched path is one of: [`PathClass::Replayable`]
//! (a lib or bin target of a workspace member, or the lib of a captured
//! local non-member — the only class a thin build can patch);
//! [`PathClass::LocalNonMember`] (a file of a `source == null` package
//! outside the workspace, such as a `--frust-path` checkout, whose compile
//! the fat build did not capture); [`PathClass::BuildInput`] (`Cargo.toml`,
//! `build.rs`, `.cargo/config.toml`, or a non-`.rs` file in a target's
//! dep-info, dioxus-cli 0.7.10's `serve/runner.rs` rule); or
//! [`PathClass::Unaffected`].
//!
//! **Local non-members.** cargo's `RUSTC_WORKSPACE_WRAPPER` captures members
//! only; a desktop session also captures non-members through
//! `RUSTC_WRAPPER` (see [`super::capture`]) and then calls
//! [`WorkspaceGraph::replay_non_members`] with the records. Each non-member
//! whose lib was captured (a proc macro excepted) becomes a lib unit, and
//! the dependency edges cover every local package from then on, so an edit
//! cascades through the non-members and members that depend on it up to the
//! tip; reaching a local package that is not replayable fails closed.
//! Until then, and in a session that never calls it, the graph is the
//! member-only one.
//!
//! **Replay units.** The unit is a (package, target) pair ([`ReplayUnit`]).
//! A changed `.rs` file maps to its package's lib target unless it is a bin's
//! root (`targets[].src_path`) or a module only a bin's dep-info lists.
//! dioxus-cli keyed replay by package and filtered the tip package out, so a
//! one-package app's own lib never replayed; here the tip lib is an ordinary
//! unit, ordered by a Kahn sort over lib targets ([`WorkspaceGraph::replay_order`]),
//! and the tip bin is replayed only when one of its own files changed.
//! [`ModifiedSet`] keeps the cumulative session set, with the dependents
//! cascade stopping at the tip. See `docs/CLI_ARCHITECTURE.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::process::ProcessRunner;

use super::HotpatchError;
use super::capture::{NonMember, NonMembers, RecordKey, RustcRecord, TargetKind};

/// `cargo metadata` target kinds that make a target a library.
const LIB_KINDS: &[&str] = &["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

/// One replayable target: a lib or bin target of a workspace member, or
/// the lib of a captured local non-member.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReplayUnit {
    /// The package name, as `cargo metadata` spells it.
    pub package: String,
    /// The target name, as `cargo metadata` spells it.
    pub target: String,
    pub kind: TargetKind,
}

impl ReplayUnit {
    pub fn lib(package: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            target: target.into(),
            kind: TargetKind::Lib,
        }
    }

    pub fn bin(package: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            target: target.into(),
            kind: TargetKind::Bin,
        }
    }

    /// The capture record this unit replays: the target name in rustc's
    /// crate-name spelling (hyphens as underscores), keyed by kind.
    pub fn record_key(&self) -> RecordKey {
        RecordKey {
            crate_name: self.target.replace('-', "_"),
            kind: self.kind,
        }
    }
}

impl std::fmt::Display for ReplayUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}.{}", self.package, self.target, self.kind.suffix())
    }
}

/// What a changed path means for a running session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathClass {
    /// Code of these replayable targets; a thin build can patch it.
    Replayable { units: BTreeSet<ReplayUnit> },
    /// A file of a path package outside the workspace whose compile was not
    /// captured (or that has no lib to replay), so only a fat rebuild picks
    /// the change up.
    LocalNonMember { package: String },
    /// A manifest, build script, cargo config, toolchain file or a non-`.rs`
    /// file a target's dep-info lists. `package` is the owning path
    /// package, when there is one.
    BuildInput { package: Option<String> },
    /// Nothing the running image was built from (a registry crate's file, a
    /// test or example root, a non-`.rs` file no dep-info lists).
    Unaffected,
}

/// The role a `cargo metadata` target plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetRole {
    Lib,
    Bin,
    BuildScript,
    /// Tests, examples and benches: never part of the running image.
    Other,
}

impl TargetRole {
    fn of(kinds: &[String]) -> Self {
        if kinds.iter().any(|kind| LIB_KINDS.contains(&kind.as_str())) {
            Self::Lib
        } else if kinds.iter().any(|kind| kind == "bin") {
            Self::Bin
        } else if kinds.iter().any(|kind| kind == "custom-build") {
            Self::BuildScript
        } else {
            Self::Other
        }
    }
}

/// A target of a path package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub role: TargetRole,
    pub src_path: PathBuf,
}

/// A path (`source == null`) package: a workspace member or a local
/// non-member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    /// The directory holding the package's `Cargo.toml`.
    pub dir: PathBuf,
    pub member: bool,
    pub targets: Vec<Target>,
}

impl Package {
    /// The package's lib target as a unit (members only).
    fn lib_unit(&self) -> Option<ReplayUnit> {
        if !self.member {
            return None;
        }
        self.lib_target_unit()
    }

    /// The package's lib target as a unit, member or not.
    fn lib_target_unit(&self) -> Option<ReplayUnit> {
        self.targets
            .iter()
            .find(|target| target.role == TargetRole::Lib)
            .map(|target| ReplayUnit::lib(&self.name, &target.name))
    }

    fn bin_units(&self) -> Vec<ReplayUnit> {
        if !self.member {
            return Vec::new();
        }
        self.targets
            .iter()
            .filter(|target| target.role == TargetRole::Bin)
            .map(|target| ReplayUnit::bin(&self.name, &target.name))
            .collect()
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetaPackage>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
    resolve: Option<MetaResolve>,
}

#[derive(Deserialize)]
struct MetaPackage {
    id: String,
    name: String,
    source: Option<String>,
    manifest_path: PathBuf,
    targets: Vec<MetaTarget>,
}

#[derive(Deserialize)]
struct MetaTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

#[derive(Deserialize)]
struct MetaResolve {
    nodes: Vec<MetaNode>,
}

#[derive(Deserialize)]
struct MetaNode {
    id: String,
    #[serde(default)]
    deps: Vec<MetaDep>,
}

#[derive(Deserialize)]
struct MetaDep {
    pkg: String,
    #[serde(default)]
    dep_kinds: Vec<MetaDepKind>,
}

#[derive(Deserialize)]
struct MetaDepKind {
    kind: Option<String>,
}

impl MetaDep {
    /// A normal (not dev-, not build-) edge. Old cargo without `dep_kinds`
    /// counts as normal, which only widens the cascade.
    fn is_normal(&self) -> bool {
        self.dep_kinds.is_empty() || self.dep_kinds.iter().any(|kind| kind.kind.is_none())
    }
}

/// The `cargo` argv that reads `manifest_path`'s workspace with its
/// resolved dependency graph, optionally filtered to one target triple.
pub fn metadata_args(manifest_path: &Path, filter_platform: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "metadata".to_string(),
        "--format-version".to_string(),
        "1".to_string(),
        "--manifest-path".to_string(),
        manifest_path.to_string_lossy().into_owned(),
    ];
    if let Some(triple) = filter_platform {
        args.push("--filter-platform".to_string());
        args.push(triple.to_string());
    }
    args
}

/// Runs `cargo metadata` through `runner` and returns its JSON document. A
/// spawn failure is [`HotpatchError::Process`]; a failed run is
/// [`HotpatchError::BuilderUnsupported`] carrying cargo's stderr.
pub fn cargo_metadata(
    runner: &dyn ProcessRunner,
    manifest_path: &Path,
    filter_platform: Option<&str>,
) -> Result<String, HotpatchError> {
    let args = metadata_args(manifest_path, filter_platform);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = runner
        .run("cargo", &argv)
        .map_err(|err| HotpatchError::Process {
            detail: format!("failed to spawn `cargo metadata`: {err:#}"),
        })?;
    if !output.success {
        return Err(HotpatchError::unsupported(format!(
            "`cargo metadata` failed for `{}`: {}",
            manifest_path.display(),
            output.stderr.trim()
        )));
    }
    Ok(output.stdout)
}

/// The workspace's path packages, their dependency edges, the tip (the
/// package and bin the session runs) and the dep-info known per unit.
#[derive(Debug, Clone)]
pub struct WorkspaceGraph {
    root: PathBuf,
    packages: Vec<Package>,
    /// Member name -> the members it depends on through a normal edge.
    member_deps: BTreeMap<String, BTreeSet<String>>,
    /// Local package name -> the local packages it depends on through a
    /// normal edge: the cascade's edges once a non-member is replayable
    /// ([`edges`](Self::edges)).
    local_deps: BTreeMap<String, BTreeSet<String>>,
    /// The non-members whose captured lib is a replay unit.
    replayable_non_members: BTreeSet<String>,
    tip: String,
    tip_bin: String,
    dep_info: BTreeMap<ReplayUnit, BTreeSet<PathBuf>>,
}

impl WorkspaceGraph {
    /// [`cargo_metadata`] then [`from_metadata`](Self::from_metadata).
    pub fn load(
        runner: &dyn ProcessRunner,
        manifest_path: &Path,
        filter_platform: Option<&str>,
        tip_package: &str,
        tip_bin: Option<&str>,
    ) -> Result<Self, HotpatchError> {
        let json = cargo_metadata(runner, manifest_path, filter_platform)?;
        Self::from_metadata(&json, tip_package, tip_bin)
    }

    /// Parses a `cargo metadata --format-version 1` document (dependencies
    /// resolved, i.e. without `--no-deps`). `tip_package` must be a member;
    /// `tip_bin` names its bin target, and may be omitted when the package
    /// has exactly one. A document without `resolve`, an unknown tip, or an
    /// ambiguous tip bin is [`HotpatchError::BuilderUnsupported`].
    pub fn from_metadata(
        json: &str,
        tip_package: &str,
        tip_bin: Option<&str>,
    ) -> Result<Self, HotpatchError> {
        let metadata: Metadata = serde_json::from_str(json).map_err(|err| {
            HotpatchError::unsupported(format!("unreadable `cargo metadata` output: {err}"))
        })?;
        let resolve = metadata.resolve.ok_or_else(|| {
            HotpatchError::unsupported(
                "`cargo metadata` output has no `resolve` graph (was it run with --no-deps?)",
            )
        })?;
        let members: BTreeSet<&str> = metadata
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect();

        let mut names_by_id: BTreeMap<&str, &str> = BTreeMap::new();
        let mut local_by_id: BTreeMap<&str, &str> = BTreeMap::new();
        let mut packages = Vec::new();
        for package in &metadata.packages {
            let member = members.contains(package.id.as_str());
            if member {
                names_by_id.insert(&package.id, &package.name);
            }
            if !member && package.source.is_some() {
                continue;
            }
            local_by_id.insert(&package.id, &package.name);
            let dir = package
                .manifest_path
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| {
                    HotpatchError::unsupported(format!(
                        "package `{}` has manifest path `{}` with no parent",
                        package.name,
                        package.manifest_path.display()
                    ))
                })?;
            packages.push(Package {
                name: package.name.clone(),
                dir: normalize(&dir),
                member,
                targets: package
                    .targets
                    .iter()
                    .map(|target| Target {
                        name: target.name.clone(),
                        role: TargetRole::of(&target.kind),
                        src_path: normalize(&target.src_path),
                    })
                    .collect(),
            });
        }
        if names_by_id.len() != members.len() {
            return Err(HotpatchError::unsupported(
                "`cargo metadata` lists a workspace member with no package entry",
            ));
        }

        let mut member_deps: BTreeMap<String, BTreeSet<String>> = names_by_id
            .values()
            .map(|name| (name.to_string(), BTreeSet::new()))
            .collect();
        let mut local_deps: BTreeMap<String, BTreeSet<String>> = local_by_id
            .values()
            .map(|name| (name.to_string(), BTreeSet::new()))
            .collect();
        for node in &resolve.nodes {
            let Some(from) = local_by_id.get(node.id.as_str()) else {
                continue;
            };
            let from_member = names_by_id.contains_key(node.id.as_str());
            for dep in node.deps.iter().filter(|dep| dep.is_normal()) {
                let Some(to) = local_by_id.get(dep.pkg.as_str()) else {
                    continue;
                };
                local_deps
                    .entry(from.to_string())
                    .or_default()
                    .insert(to.to_string());
                if from_member && names_by_id.contains_key(dep.pkg.as_str()) {
                    member_deps
                        .entry(from.to_string())
                        .or_default()
                        .insert(to.to_string());
                }
            }
        }

        let tip = packages
            .iter()
            .find(|package| package.member && package.name == tip_package)
            .ok_or_else(|| {
                HotpatchError::unsupported(format!(
                    "the tip package `{tip_package}` is not a workspace member"
                ))
            })?;
        let bins: Vec<&str> = tip
            .targets
            .iter()
            .filter(|target| target.role == TargetRole::Bin)
            .map(|target| target.name.as_str())
            .collect();
        let tip_bin = match tip_bin {
            Some(name) if bins.contains(&name) => name.to_string(),
            Some(name) => {
                return Err(HotpatchError::unsupported(format!(
                    "the tip package `{tip_package}` has no bin target `{name}` (bins: {bins:?})"
                )));
            }
            None => match bins.as_slice() {
                [only] => only.to_string(),
                _ => {
                    return Err(HotpatchError::unsupported(format!(
                        "the tip package `{tip_package}` needs a bin name: it has {bins:?}"
                    )));
                }
            },
        };

        Ok(Self {
            root: normalize(&metadata.workspace_root),
            packages,
            member_deps,
            local_deps,
            replayable_non_members: BTreeSet::new(),
            tip: tip_package.to_string(),
            tip_bin,
            dep_info: BTreeMap::new(),
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.root
    }

    /// Every path package: members, then local non-members, in metadata
    /// order. A watcher needs every package directory listed here.
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// The tip package's name.
    pub fn tip(&self) -> &str {
        &self.tip
    }

    /// The tip package's lib target, when it has one.
    pub fn tip_lib(&self) -> Option<ReplayUnit> {
        self.package(&self.tip).and_then(Package::lib_unit)
    }

    /// The bin the session runs.
    pub fn tip_bin(&self) -> ReplayUnit {
        ReplayUnit::bin(&self.tip, &self.tip_bin)
    }

    /// Every lib and bin unit of every member, then the lib unit of every
    /// replayable non-member.
    pub fn units(&self) -> Vec<ReplayUnit> {
        self.members()
            .flat_map(|package| package.lib_unit().into_iter().chain(package.bin_units()))
            .chain(
                self.packages
                    .iter()
                    .filter(|package| self.replayable_non_members.contains(&package.name))
                    .filter_map(Package::lib_target_unit),
            )
            .collect()
    }

    /// The local non-members a fat build can capture: every one with a lib
    /// target, by name and manifest directory.
    pub fn non_members(&self) -> NonMembers {
        NonMembers::new(
            self.packages
                .iter()
                .filter(|package| !package.member && package.lib_target_unit().is_some())
                .map(|package| NonMember {
                    name: package.name.clone(),
                    dir: package.dir.clone(),
                }),
        )
    }

    /// Makes every local non-member whose lib the fat build captured into
    /// `records` a replayable lib unit, and from then on keys the
    /// dependency cascade on every local package's edges, so an edit to one
    /// replays it and its dependents up to the tip. Left out (its files
    /// stay [`PathClass::LocalNonMember`]): a non-member without a record,
    /// a proc macro (its replay emits no rlib), and one whose crate name a
    /// member unit's record already uses. Returns the names made
    /// replayable; with none, the graph stays the member-only one.
    pub fn replay_non_members(
        &mut self,
        records: &BTreeMap<RecordKey, RustcRecord>,
    ) -> Vec<String> {
        let member_keys: BTreeSet<RecordKey> = self
            .members()
            .flat_map(|package| package.lib_unit().into_iter().chain(package.bin_units()))
            .map(|unit| unit.record_key())
            .collect();
        let enabled: BTreeSet<String> = self
            .packages
            .iter()
            .filter(|package| !package.member)
            .filter_map(|package| {
                let key = package.lib_target_unit()?.record_key();
                let record = records.get(&key)?;
                let proc_macro = record.crate_types.iter().any(|ty| ty == "proc-macro");
                (!proc_macro && !member_keys.contains(&key)).then(|| package.name.clone())
            })
            .collect();
        self.replayable_non_members = enabled.clone();
        enabled.into_iter().collect()
    }

    /// Records the files `unit`'s dep-info lists (see [`parse_dep_info`]),
    /// replacing what was known. Dep-info decides bin-only modules and
    /// non-`.rs` build inputs.
    pub fn set_dep_info(&mut self, unit: ReplayUnit, files: BTreeSet<PathBuf>) {
        self.dep_info
            .insert(unit, files.iter().map(|file| normalize(file)).collect());
    }

    /// The directory rustc ran in for `unit`'s captured compile, which its
    /// relative arguments resolve against: the workspace root when the
    /// target's source lies under it, else the package directory (cargo's
    /// own rule for path packages).
    pub fn replay_cwd(&self, unit: &ReplayUnit) -> PathBuf {
        let package = self.local_package(&unit.package);
        let src = package
            .and_then(|package| package.targets.iter().find(|t| t.name == unit.target))
            .map(|target| target.src_path.as_path());
        match (src, package) {
            (Some(src), _) if src.starts_with(&self.root) => self.root.clone(),
            (_, Some(package)) => package.dir.clone(),
            _ => self.root.clone(),
        }
    }

    /// Classifies one changed path (absolute, as the watcher reports it).
    pub fn classify(&self, path: &Path) -> PathClass {
        let path = normalize(path);
        let owner = self.owner(&path);
        let owner_name = owner.map(|package| package.name.clone());
        if is_build_input_name(&path) || self.is_build_script(&path) {
            return PathClass::BuildInput {
                package: owner_name,
            };
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            if let Some(unit) = self.dep_info_units(&path).into_iter().next() {
                return PathClass::BuildInput {
                    package: Some(unit.package),
                };
            }
            return match owner {
                Some(package) if !package.member && self.non_member_lib(package).is_none() => {
                    PathClass::LocalNonMember {
                        package: package.name.clone(),
                    }
                }
                // Without any dep-info for the owner's targets there is no
                // telling whether the file is an `include_*!` input.
                Some(package) if !self.has_dep_info(package) => PathClass::BuildInput {
                    package: owner_name,
                },
                _ => PathClass::Unaffected,
            };
        }

        let mut units = BTreeSet::new();
        match owner {
            Some(package) if !package.member => match self.non_member_lib(package) {
                Some(lib) => {
                    let other_root = package
                        .targets
                        .iter()
                        .any(|t| t.role != TargetRole::Lib && t.src_path == path);
                    if other_root {
                        return PathClass::Unaffected;
                    }
                    units.insert(lib);
                }
                None => {
                    return PathClass::LocalNonMember {
                        package: package.name.clone(),
                    };
                }
            },
            Some(package) => {
                if package
                    .targets
                    .iter()
                    .any(|t| t.role == TargetRole::Other && t.src_path == path)
                {
                    return PathClass::Unaffected;
                }
                units.extend(self.member_units_for(package, &path));
            }
            None => {}
        }
        units.extend(self.dep_info_units(&path));
        if units.is_empty() {
            PathClass::Unaffected
        } else {
            PathClass::Replayable { units }
        }
    }

    /// The units an `.rs` file of member `package` maps to: a bin whose
    /// root it is; else the bins whose dep-info lists it when the lib's
    /// dep-info is known and does not; else the lib (plus any bin that also
    /// lists it). A lib-less package falls back to its bins.
    fn member_units_for(&self, package: &Package, path: &Path) -> BTreeSet<ReplayUnit> {
        let bins = package.bin_units();
        let roots: BTreeSet<ReplayUnit> = package
            .targets
            .iter()
            .filter(|t| t.role == TargetRole::Bin && t.src_path == path)
            .map(|t| ReplayUnit::bin(&package.name, &t.name))
            .collect();
        if !roots.is_empty() {
            return roots;
        }
        let listing = |unit: &ReplayUnit| {
            self.dep_info
                .get(unit)
                .is_some_and(|files| files.contains(path))
        };
        let bins_listing: BTreeSet<ReplayUnit> =
            bins.iter().filter(|u| listing(u)).cloned().collect();
        match package.lib_unit() {
            Some(lib) => match self.dep_info.get(&lib).map(|files| files.contains(path)) {
                Some(false) if !bins_listing.is_empty() => bins_listing,
                _ => std::iter::once(lib).chain(bins_listing).collect(),
            },
            None if bins_listing.is_empty() => bins.into_iter().collect(),
            None => bins_listing,
        }
    }

    /// Member units whose recorded dep-info lists `path`.
    fn dep_info_units(&self, path: &Path) -> BTreeSet<ReplayUnit> {
        self.dep_info
            .iter()
            .filter(|(_, files)| files.contains(path))
            .map(|(unit, _)| unit.clone())
            .collect()
    }

    fn has_dep_info(&self, package: &Package) -> bool {
        self.dep_info
            .keys()
            .any(|unit| unit.package == package.name)
    }

    fn is_build_script(&self, path: &Path) -> bool {
        self.packages.iter().any(|package| {
            package
                .targets
                .iter()
                .any(|t| t.role == TargetRole::BuildScript && t.src_path == path)
        })
    }

    /// The path package whose directory is the longest prefix of `path`.
    fn owner(&self, path: &Path) -> Option<&Package> {
        self.packages
            .iter()
            .filter(|package| path.starts_with(&package.dir))
            .max_by_key(|package| package.dir.components().count())
    }

    fn package(&self, name: &str) -> Option<&Package> {
        self.members().find(|package| package.name == name)
    }

    /// The path package named `name`, member or not.
    fn local_package(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|package| package.name == name)
    }

    /// A replayable non-member's lib unit.
    fn non_member_lib(&self, package: &Package) -> Option<ReplayUnit> {
        if package.member || !self.replayable_non_members.contains(&package.name) {
            return None;
        }
        package.lib_target_unit()
    }

    /// The lib unit of the package named `name`: a member's, or a
    /// replayable non-member's.
    fn lib_unit_of(&self, name: &str) -> Option<ReplayUnit> {
        match self.local_package(name) {
            Some(package) if package.member => package.lib_unit(),
            Some(package) => self.non_member_lib(package),
            None => None,
        }
    }

    fn members(&self) -> impl Iterator<Item = &Package> {
        self.packages.iter().filter(|package| package.member)
    }

    /// The dependency edges the cascade and replay order follow: between
    /// members only, or between every local package once a non-member is
    /// replayable.
    fn edges(&self) -> &BTreeMap<String, BTreeSet<String>> {
        if self.replayable_non_members.is_empty() {
            &self.member_deps
        } else {
            &self.local_deps
        }
    }

    /// Packages that depend directly (normal edge) on `name`.
    fn dependents_of(&self, name: &str) -> Vec<&str> {
        self.edges()
            .iter()
            .filter(|(_, deps)| deps.contains(name))
            .map(|(dependent, _)| dependent.as_str())
            .collect()
    }

    /// Every package `name` depends on, transitively (excluding itself
    /// unless a cycle leads back).
    fn transitive_deps(&self, name: &str) -> BTreeSet<&str> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&str> = vec![name];
        while let Some(current) = stack.pop() {
            for dep in self.edges().get(current).into_iter().flatten() {
                if seen.insert(dep.as_str()) {
                    stack.push(dep);
                }
            }
        }
        seen
    }

    /// The tip and every package it depends on: the packages compiled into
    /// the running image.
    fn tip_closure(&self) -> BTreeSet<&str> {
        let mut closure = self.transitive_deps(&self.tip);
        closure.insert(&self.tip);
        closure
    }

    /// Orders `units` for replay: lib targets first, dependencies before
    /// dependents (Kahn's algorithm; ties broken by unit order for
    /// determinism), then bins. A dependency cycle among the libs is
    /// [`HotpatchError::BuilderUnsupported`].
    pub fn replay_order(
        &self,
        units: &BTreeSet<ReplayUnit>,
    ) -> Result<Vec<ReplayUnit>, HotpatchError> {
        let libs: Vec<&ReplayUnit> = units.iter().filter(|u| u.kind == TargetKind::Lib).collect();
        let deps: BTreeMap<&ReplayUnit, BTreeSet<&str>> = libs
            .iter()
            .map(|unit| (*unit, self.transitive_deps(&unit.package)))
            .collect();
        // indegree[b] = how many other libs in the set b depends on.
        let mut indegree: BTreeMap<&ReplayUnit, usize> = libs
            .iter()
            .map(|unit| {
                let count = libs
                    .iter()
                    .filter(|other| other != &unit && deps[unit].contains(other.package.as_str()))
                    .count();
                (*unit, count)
            })
            .collect();
        let mut ready: BTreeSet<&ReplayUnit> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(unit, _)| *unit)
            .collect();
        let mut ordered = Vec::with_capacity(units.len());
        while let Some(unit) = ready.pop_first() {
            ordered.push(unit.clone());
            for other in &libs {
                if deps[other].contains(unit.package.as_str()) && *other != unit {
                    let degree = indegree.get_mut(other).expect("every lib has a degree");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(other);
                    }
                }
            }
        }
        if ordered.len() != libs.len() {
            return Err(HotpatchError::unsupported(
                "cycle in the workspace dependency graph: no replay order exists",
            ));
        }
        ordered.extend(units.iter().filter(|u| u.kind == TargetKind::Bin).cloned());
        Ok(ordered)
    }
}

/// What one change asks the builder to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPlan {
    /// Units to recompile now, in replay order: the changed units plus the
    /// dependents cascade (stopping at the tip).
    pub replay: Vec<ReplayUnit>,
    /// Every unit modified since the fat build, in replay order: what the
    /// next patch links.
    pub modified: Vec<ReplayUnit>,
}

/// The session's cumulative modified set. Every patch carries the objects
/// of every unit modified since the fat build, not only the latest change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifiedSet {
    units: BTreeSet<ReplayUnit>,
}

impl ModifiedSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn units(&self) -> &BTreeSet<ReplayUnit> {
        &self.units
    }

    /// Adds one change's units and returns the plan. A changed lib outside
    /// the tip's dependency closure is not in the running image and is
    /// dropped, as is any bin but the tip bin. A changed lib cascades to
    /// its dependents inside the closure; the cascade reaches the tip lib
    /// (or, for a lib-less tip, the tip bin) and stops there. A unit the
    /// graph does not know, or a cascade reaching a local non-member that is
    /// not replayable, is [`HotpatchError::BuilderUnsupported`].
    pub fn record_change<'a>(
        &mut self,
        graph: &WorkspaceGraph,
        changed: impl IntoIterator<Item = &'a ReplayUnit>,
    ) -> Result<ReplayPlan, HotpatchError> {
        let known: BTreeSet<ReplayUnit> = graph.units().into_iter().collect();
        let closure = graph.tip_closure();
        let tip_bin = graph.tip_bin();
        let mut now = BTreeSet::new();
        let mut queue: Vec<String> = Vec::new();
        for unit in changed {
            if !known.contains(unit) {
                return Err(HotpatchError::unsupported(format!(
                    "`{unit}` is not a lib or bin target of a workspace member"
                )));
            }
            match unit.kind {
                TargetKind::Bin if *unit == tip_bin => {
                    now.insert(unit.clone());
                }
                TargetKind::Bin => {}
                TargetKind::Lib if closure.contains(unit.package.as_str()) => {
                    queue.push(unit.package.clone());
                }
                TargetKind::Lib => {}
            }
        }

        let mut visited = BTreeSet::new();
        while let Some(name) = queue.pop() {
            if !visited.insert(name.clone()) {
                continue;
            }
            match graph.lib_unit_of(&name) {
                Some(lib) => {
                    now.insert(lib);
                }
                None if graph.package(&name).is_none() && name != graph.tip() => {
                    return Err(HotpatchError::unsupported(format!(
                        "the local path package `{name}` depends on a changed package, but its \
                         compile was not captured, so it cannot be replayed"
                    )));
                }
                None => {
                    // Only the tip can be lib-less and still be reached.
                    now.insert(tip_bin.clone());
                    continue;
                }
            }
            if name == graph.tip() {
                continue;
            }
            for dependent in graph.dependents_of(&name) {
                if closure.contains(dependent) && !visited.contains(dependent) {
                    queue.push(dependent.to_string());
                }
            }
        }

        self.units.extend(now.iter().cloned());
        Ok(ReplayPlan {
            replay: graph.replay_order(&now)?,
            modified: graph.replay_order(&self.units)?,
        })
    }
}

/// Whether the file name alone makes `path` a build input: a manifest or
/// lockfile, a build script, cargo config, or a toolchain file.
fn is_build_input_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let in_cargo_dir = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|dir| dir == ".cargo");
    matches!(
        name,
        "Cargo.toml" | "Cargo.lock" | "build.rs" | "rust-toolchain" | "rust-toolchain.toml"
    ) || (in_cargo_dir && matches!(name, "config" | "config.toml"))
}

/// Parses a rustc dep-info file (Makefile syntax): the union of every
/// rule's prerequisites, `\ `-escaped spaces restored, relative paths
/// resolved against `cwd` (the directory rustc ran in).
pub fn parse_dep_info(text: &str, cwd: &Path) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        // The rule separator is the first `: ` (or a trailing `:`), so a
        // Windows drive colon (`C:\`) is never mistaken for it.
        let prerequisites = match line.find(": ") {
            Some(at) => &line[at + 2..],
            None => continue,
        };
        for file in split_escaped(prerequisites) {
            let file = PathBuf::from(file);
            files.insert(normalize(&cwd.join(file)));
        }
    }
    files
}

/// The environment variables a dep-info file says the compile read
/// (`env!`/`option_env!`): the names of its `# env-dep:NAME[=VALUE]`
/// comment lines, rustc's `\n`/`\r`/`\\` escapes undone. A name read but
/// unset is listed too (no `=VALUE`).
pub fn parse_dep_info_env(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("# env-dep:"))
        .map(|entry| entry.split_once('=').map_or(entry, |(name, _)| name))
        .map(unescape_dep_env)
        .collect()
}

/// Undoes rustc's dep-info escaping of an environment name.
fn unescape_dep_env(escaped: &str) -> String {
    let mut out = String::with_capacity(escaped.len());
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// [`parse_dep_info`] over a file.
pub fn read_dep_info(path: &Path, cwd: &Path) -> Result<BTreeSet<PathBuf>, HotpatchError> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| HotpatchError::io(format!("reading dep-info `{}`", path.display()), err))?;
    Ok(parse_dep_info(&text, cwd))
}

/// Splits on unescaped spaces, unescaping `\ `.
fn split_escaped(text: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                current.push(' ');
                chars.next();
            }
            ' ' => {
                if !current.is_empty() {
                    items.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}

/// Removes `.` and resolves `..` lexically, so a dep-info path such as
/// `src/../shared.rs` compares equal to the watcher's path.
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};

    fn unsupported<T: std::fmt::Debug>(result: Result<T, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    fn target(name: &str, kinds: &[&str], src: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "kind": kinds,
            "crate_types": kinds,
            "src_path": src,
            "edition": "2024",
        })
    }

    fn package(
        id: &str,
        name: &str,
        source: Option<&str>,
        manifest: &str,
        targets: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "version": "0.1.0",
            "source": source,
            "manifest_path": manifest,
            "targets": targets,
            "dependencies": [],
        })
    }

    fn normal(pkg: &str) -> serde_json::Value {
        serde_json::json!({"name": pkg, "pkg": pkg, "dep_kinds": [{"kind": null, "target": null}]})
    }

    fn dev(pkg: &str) -> serde_json::Value {
        serde_json::json!({"name": pkg, "pkg": pkg, "dep_kinds": [{"kind": "dev", "target": null}]})
    }

    const APP: &str = "path+file:///w/app#0.1.0";
    const CORE: &str = "path+file:///w/core-ui#0.1.0";
    const BASE: &str = "path+file:///w/base#0.1.0";
    const OTHER: &str = "path+file:///w/other-app#0.1.0";
    const TOOL: &str = "path+file:///w/tool#0.1.0";
    const MATERIAL: &str = "path+file:///x/frust/plugins/material#0.6.0";
    const SERDE: &str = "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0";

    /// A workspace at `/w`: `app` (the tip: lib + bin, with an example)
    /// depends on `core-ui` (lib + build script), which depends on `base`;
    /// `other-app` depends on `base` but is not in the tip's closure; `tool`
    /// depends on `app`; `frust-material` is a local non-member path
    /// dependency of `app`; `serde` is a registry crate.
    fn workspace_metadata() -> String {
        serde_json::json!({
            "packages": [
                package(APP, "app", None, "/w/app/Cargo.toml", vec![
                    target("app", &["cdylib", "staticlib", "rlib"], "/w/app/src/lib.rs"),
                    target("app", &["bin"], "/w/app/src/main.rs"),
                    target("demo", &["example"], "/w/app/examples/demo.rs"),
                ]),
                package(CORE, "core-ui", None, "/w/core-ui/Cargo.toml", vec![
                    target("core_ui", &["lib"], "/w/core-ui/src/lib.rs"),
                    target("build-script-build", &["custom-build"], "/w/core-ui/build.rs"),
                ]),
                package(BASE, "base", None, "/w/base/Cargo.toml", vec![
                    target("base", &["lib"], "/w/base/src/lib.rs"),
                ]),
                package(OTHER, "other-app", None, "/w/other-app/Cargo.toml", vec![
                    target("other_app", &["lib"], "/w/other-app/src/lib.rs"),
                ]),
                package(TOOL, "tool", None, "/w/tool/Cargo.toml", vec![
                    target("tool", &["lib"], "/w/tool/src/lib.rs"),
                ]),
                package(MATERIAL, "frust-material", None, "/x/frust/plugins/material/Cargo.toml", vec![
                    target("frust_material", &["lib"], "/x/frust/plugins/material/src/lib.rs"),
                ]),
                package(SERDE, "serde", Some("registry+https://github.com/rust-lang/crates.io-index"),
                    "/home/.cargo/registry/src/serde-1.0.0/Cargo.toml", vec![
                    target("serde", &["lib"], "/home/.cargo/registry/src/serde-1.0.0/src/lib.rs"),
                ]),
            ],
            "workspace_members": [APP, CORE, BASE, OTHER, TOOL],
            "workspace_default_members": [APP],
            "resolve": {
                "nodes": [
                    {"id": APP, "deps": [normal(CORE), normal(MATERIAL), normal(SERDE), dev(TOOL)]},
                    {"id": CORE, "deps": [normal(BASE)]},
                    {"id": BASE, "deps": []},
                    {"id": OTHER, "deps": [normal(BASE)]},
                    {"id": TOOL, "deps": [normal(APP)]},
                    {"id": MATERIAL, "deps": [normal(SERDE)]},
                    {"id": SERDE, "deps": []},
                ],
                "root": null,
            },
            "target_directory": "/w/target",
            "version": 1,
            "workspace_root": "/w",
        })
        .to_string()
    }

    /// The `frust create` layout: one package, lib with three crate types
    /// plus the one-line `main.rs` bin.
    fn template_metadata() -> String {
        const ID: &str = "path+file:///p/my-app#0.1.0";
        serde_json::json!({
            "packages": [package(ID, "my-app", None, "/p/my-app/Cargo.toml", vec![
                target("my_app", &["cdylib", "staticlib", "rlib"], "/p/my-app/src/lib.rs"),
                target("my-app", &["bin"], "/p/my-app/src/main.rs"),
            ])],
            "workspace_members": [ID],
            "resolve": {"nodes": [{"id": ID, "deps": []}], "root": ID},
            "workspace_root": "/p/my-app",
        })
        .to_string()
    }

    fn graph() -> WorkspaceGraph {
        WorkspaceGraph::from_metadata(&workspace_metadata(), "app", None).unwrap()
    }

    fn files(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn replayable(units: &[ReplayUnit]) -> PathClass {
        PathClass::Replayable {
            units: units.iter().cloned().collect(),
        }
    }

    fn app_lib() -> ReplayUnit {
        ReplayUnit::lib("app", "app")
    }

    fn app_bin() -> ReplayUnit {
        ReplayUnit::bin("app", "app")
    }

    #[test]
    fn member_code_is_replayable_by_package_and_target() {
        let graph = graph();
        let class = |p: &str| graph.classify(Path::new(p));
        assert_eq!(class("/w/app/src/lib.rs"), replayable(&[app_lib()]));
        assert_eq!(
            class("/w/app/src/screens/home.rs"),
            replayable(&[app_lib()])
        );
        assert_eq!(class("/w/app/src/main.rs"), replayable(&[app_bin()]));
        assert_eq!(
            class("/w/core-ui/src/button.rs"),
            replayable(&[ReplayUnit::lib("core-ui", "core_ui")])
        );
        assert_eq!(
            class("/w/app/src/./screens/../lib.rs"),
            replayable(&[app_lib()]),
            "paths compare lexically normalised"
        );
    }

    /// A capture record for each of `keys` (`{crate}.{lib|bin}`), as a fat
    /// build leaves them; `proc_macro` keys are recorded as proc macros.
    fn records(keys: &[&str], proc_macro: &[&str]) -> BTreeMap<RecordKey, RustcRecord> {
        keys.iter()
            .map(|key| {
                let key = RecordKey::parse(key).unwrap();
                let ty = if proc_macro.contains(&key.crate_name.as_str()) {
                    "proc-macro"
                } else {
                    key.kind.suffix()
                };
                let record = RustcRecord {
                    args: vec!["rustc".to_string()],
                    envs: Vec::new(),
                    crate_types: vec![ty.to_string()],
                };
                (key, record)
            })
            .collect()
    }

    fn material_lib() -> ReplayUnit {
        ReplayUnit::lib("frust-material", "frust_material")
    }

    #[test]
    fn a_captured_path_dependency_outside_the_workspace_is_replayable() {
        let mut graph = graph();
        let replayable =
            graph.replay_non_members(&records(&["app.lib", "app.bin", "frust_material.lib"], &[]));
        assert_eq!(replayable, vec!["frust-material".to_string()]);
        assert_eq!(
            graph.classify(Path::new("/x/frust/plugins/material/src/lib.rs")),
            PathClass::Replayable {
                units: [material_lib()].into_iter().collect()
            }
        );
        assert_eq!(
            graph.classify(Path::new("/x/frust/plugins/material/src/button.rs")),
            PathClass::Replayable {
                units: [material_lib()].into_iter().collect()
            }
        );
        assert_eq!(
            graph.classify(Path::new("/x/frust/plugins/material/assets/icons.svg")),
            PathClass::BuildInput {
                package: Some("frust-material".to_string())
            },
            "without its dep-info, any file of it may be an `include_*!` input"
        );
        assert!(graph.units().contains(&material_lib()));
        assert_eq!(
            graph.replay_cwd(&material_lib()),
            PathBuf::from("/x/frust/plugins/material"),
            "a package outside the workspace root compiles in its own directory"
        );
        let local: Vec<&str> = graph
            .packages()
            .iter()
            .filter(|p| !p.member)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            local,
            vec!["frust-material"],
            "registry crates are not path packages"
        );
        assert_eq!(
            graph.non_members().names(),
            vec!["frust-material".to_string()]
        );
        assert_eq!(
            graph.non_members().packages()[0].dir,
            PathBuf::from("/x/frust/plugins/material")
        );
    }

    #[test]
    fn an_uncaptured_path_dependency_stays_a_local_non_member() {
        let mut graph = graph();
        let members_only = graph.units();
        assert!(
            graph
                .replay_non_members(&records(&["app.lib", "core_ui.lib"], &[]))
                .is_empty()
        );
        let material = PathClass::LocalNonMember {
            package: "frust-material".to_string(),
        };
        assert_eq!(
            graph.classify(Path::new("/x/frust/plugins/material/src/lib.rs")),
            material
        );
        assert_eq!(
            graph.classify(Path::new("/x/frust/plugins/material/assets/icons.svg")),
            material
        );
        assert_eq!(graph.units(), members_only);
        let err = unsupported(ModifiedSet::new().record_change(&graph, [&material_lib()]));
        assert!(err.contains("frust-material"), "{err}");
    }

    const WIDGETS: &str = "path+file:///x/frust/widgets#0.6.0";
    const FRAMEWORK_MATERIAL: &str = "path+file:///x/frust/material#0.6.0";
    const MACROS: &str = "path+file:///x/frust/macros#0.6.0";
    const UI: &str = "path+file:///w/ui#0.1.0";

    /// A workspace at `/w` with a framework checkout at `/x/frust`: `app`
    /// (the tip) depends on the member `ui` and the non-member
    /// `frust-material`; `ui` and `frust-material` both depend on the
    /// non-member `frust-widgets`, which uses the non-member proc macro
    /// `frust-macros`.
    fn framework_metadata() -> String {
        serde_json::json!({
            "packages": [
                package(APP, "app", None, "/w/app/Cargo.toml", vec![
                    target("app", &["lib"], "/w/app/src/lib.rs"),
                    target("app", &["bin"], "/w/app/src/main.rs"),
                ]),
                package(UI, "ui", None, "/w/ui/Cargo.toml", vec![
                    target("ui", &["lib"], "/w/ui/src/lib.rs"),
                ]),
                package(FRAMEWORK_MATERIAL, "frust-material", None, "/x/frust/material/Cargo.toml", vec![
                    target("frust_material", &["lib"], "/x/frust/material/src/lib.rs"),
                ]),
                package(WIDGETS, "frust-widgets", None, "/x/frust/widgets/Cargo.toml", vec![
                    target("frust_widgets", &["lib"], "/x/frust/widgets/src/lib.rs"),
                    target("gallery", &["example"], "/x/frust/widgets/examples/gallery.rs"),
                ]),
                package(MACROS, "frust-macros", None, "/x/frust/macros/Cargo.toml", vec![
                    target("frust_macros", &["proc-macro"], "/x/frust/macros/src/lib.rs"),
                ]),
            ],
            "workspace_members": [APP, UI],
            "resolve": {
                "nodes": [
                    {"id": APP, "deps": [normal(UI), normal(FRAMEWORK_MATERIAL)]},
                    {"id": UI, "deps": [normal(WIDGETS)]},
                    {"id": FRAMEWORK_MATERIAL, "deps": [normal(WIDGETS)]},
                    {"id": WIDGETS, "deps": [normal(MACROS)]},
                    {"id": MACROS, "deps": []},
                ],
                "root": null,
            },
            "workspace_root": "/w",
        })
        .to_string()
    }

    fn framework_graph(captured: &[&str]) -> WorkspaceGraph {
        let mut graph = WorkspaceGraph::from_metadata(&framework_metadata(), "app", None).unwrap();
        let mut keys = vec!["app.lib", "app.bin", "ui.lib"];
        keys.extend(captured);
        graph.replay_non_members(&records(&keys, &["frust_macros"]));
        graph
    }

    fn widgets_lib() -> ReplayUnit {
        ReplayUnit::lib("frust-widgets", "frust_widgets")
    }

    fn framework_material_lib() -> ReplayUnit {
        ReplayUnit::lib("frust-material", "frust_material")
    }

    #[test]
    fn an_edit_to_a_captured_path_dependency_replays_every_dependent_up_to_the_tip() {
        let graph = framework_graph(&[
            "frust_widgets.lib",
            "frust_material.lib",
            "frust_macros.lib",
        ]);
        let class = graph.classify(Path::new("/x/frust/widgets/src/flex.rs"));
        let PathClass::Replayable { units } = class else {
            panic!("expected replayable, got {class:?}");
        };
        assert_eq!(units, [widgets_lib()].into_iter().collect());
        let ui = ReplayUnit::lib("ui", "ui");
        let mut modified = ModifiedSet::new();
        let plan = modified.record_change(&graph, &units).unwrap();
        assert_eq!(
            plan.replay,
            vec![
                widgets_lib(),
                framework_material_lib(),
                ui.clone(),
                app_lib()
            ],
            "dependencies first; the cascade stops at the tip lib, never the bin"
        );
        // A later member-only edit still links the framework's objects.
        let plan = modified.record_change(&graph, [&ui]).unwrap();
        assert_eq!(plan.replay, vec![ui.clone(), app_lib()]);
        assert_eq!(
            plan.modified,
            vec![widgets_lib(), framework_material_lib(), ui, app_lib()]
        );
        assert_eq!(
            graph.classify(Path::new("/x/frust/widgets/examples/gallery.rs")),
            PathClass::Unaffected,
            "an example is not in the image"
        );
        assert_eq!(
            graph.classify(Path::new("/x/frust/macros/src/lib.rs")),
            PathClass::LocalNonMember {
                package: "frust-macros".to_string()
            },
            "a proc macro's replay emits no rlib: it restarts"
        );
    }

    #[test]
    fn a_cascade_into_an_uncaptured_path_dependency_fails_closed() {
        let graph = framework_graph(&["frust_widgets.lib"]);
        assert_eq!(
            graph.classify(Path::new("/x/frust/material/src/lib.rs")),
            PathClass::LocalNonMember {
                package: "frust-material".to_string()
            }
        );
        let err = unsupported(ModifiedSet::new().record_change(&graph, [&widgets_lib()]));
        assert!(err.contains("`frust-material`"), "{err}");
    }

    #[test]
    fn manifests_build_scripts_cargo_config_and_dep_info_inputs_are_build_inputs() {
        let mut graph = graph();
        graph.set_dep_info(
            app_lib(),
            files(&["/w/app/src/lib.rs", "/w/app/assets/logo.svg"]),
        );
        let input = |package: Option<&str>| PathClass::BuildInput {
            package: package.map(str::to_string),
        };
        let class = |p: &str| graph.classify(Path::new(p));
        assert_eq!(class("/w/app/Cargo.toml"), input(Some("app")));
        assert_eq!(class("/w/Cargo.toml"), input(None));
        assert_eq!(class("/w/Cargo.lock"), input(None));
        assert_eq!(class("/w/core-ui/build.rs"), input(Some("core-ui")));
        assert_eq!(class("/w/.cargo/config.toml"), input(None));
        assert_eq!(class("/w/app/.cargo/config"), input(Some("app")));
        assert_eq!(
            class("/x/frust/plugins/material/Cargo.toml"),
            input(Some("frust-material"))
        );
        assert_eq!(
            class("/w/app/assets/logo.svg"),
            input(Some("app")),
            "a non-.rs file in a target's dep-info is a build input"
        );
        assert_eq!(
            class("/w/app/README.md"),
            PathClass::Unaffected,
            "a non-.rs file no dep-info lists is not"
        );
        assert_eq!(
            class("/w/core-ui/assets/theme.json"),
            input(Some("core-ui")),
            "with no dep-info for the owner, a non-.rs file is assumed an input"
        );
    }

    #[test]
    fn registry_files_and_example_roots_are_unaffected() {
        let graph = graph();
        assert_eq!(
            graph.classify(Path::new(
                "/home/.cargo/registry/src/serde-1.0.0/src/lib.rs"
            )),
            PathClass::Unaffected
        );
        assert_eq!(
            graph.classify(Path::new("/w/app/examples/demo.rs")),
            PathClass::Unaffected
        );
        assert_eq!(
            graph.classify(Path::new("/elsewhere/x.rs")),
            PathClass::Unaffected
        );
    }

    #[test]
    fn a_module_only_the_bin_includes_maps_to_the_bin() {
        let mut graph = graph();
        graph.set_dep_info(app_lib(), files(&["/w/app/src/lib.rs", "/w/app/src/ui.rs"]));
        graph.set_dep_info(
            app_bin(),
            files(&[
                "/w/app/src/main.rs",
                "/w/app/src/cli.rs",
                "/w/app/src/ui.rs",
            ]),
        );
        let class = |p: &str| graph.classify(Path::new(p));
        assert_eq!(class("/w/app/src/cli.rs"), replayable(&[app_bin()]));
        assert_eq!(
            class("/w/app/src/ui.rs"),
            replayable(&[app_lib(), app_bin()]),
            "a module both include replays both"
        );
        assert_eq!(class("/w/app/src/new_module.rs"), replayable(&[app_lib()]));
    }

    #[test]
    fn a_file_outside_any_package_maps_through_dep_info() {
        let mut graph = graph();
        graph.set_dep_info(
            ReplayUnit::lib("core-ui", "core_ui"),
            files(&["/w/core-ui/src/lib.rs", "/w/shared/macros.rs"]),
        );
        assert_eq!(
            graph.classify(Path::new("/w/shared/macros.rs")),
            replayable(&[ReplayUnit::lib("core-ui", "core_ui")])
        );
    }

    #[test]
    fn the_tip_lib_is_replayed_and_the_tip_bin_only_when_its_own_file_changed() {
        let graph = WorkspaceGraph::from_metadata(&template_metadata(), "my-app", None).unwrap();
        let lib = ReplayUnit::lib("my-app", "my_app");
        let bin = ReplayUnit::bin("my-app", "my-app");
        assert_eq!(graph.tip_lib(), Some(lib.clone()));
        assert_eq!(graph.tip_bin(), bin);
        assert_eq!(lib.record_key().to_string(), "my_app.lib");
        assert_eq!(bin.record_key().to_string(), "my_app.bin");

        let PathClass::Replayable { units } = graph.classify(Path::new("/p/my-app/src/lib.rs"))
        else {
            panic!("lib.rs is replayable");
        };
        let mut set = ModifiedSet::new();
        let plan = set.record_change(&graph, &units).unwrap();
        assert_eq!(
            plan.replay,
            vec![lib.clone()],
            "the tip lib replays, the bin does not"
        );
        assert_eq!(plan.modified, vec![lib.clone()]);

        let PathClass::Replayable { units } = graph.classify(Path::new("/p/my-app/src/main.rs"))
        else {
            panic!("main.rs is replayable");
        };
        let plan = set.record_change(&graph, &units).unwrap();
        assert_eq!(
            plan.replay,
            vec![bin.clone()],
            "main.rs changed: the bin alone"
        );
        assert_eq!(plan.modified, vec![lib, bin]);
        assert_eq!(
            graph.replay_cwd(&ReplayUnit::lib("my-app", "my_app")),
            PathBuf::from("/p/my-app")
        );
    }

    #[test]
    fn the_cascade_stops_at_the_tip_and_the_set_is_cumulative() {
        let graph = graph();
        let core = ReplayUnit::lib("core-ui", "core_ui");
        let base = ReplayUnit::lib("base", "base");
        let mut set = ModifiedSet::new();

        let plan = set.record_change(&graph, [&core]).unwrap();
        assert_eq!(
            plan.replay,
            vec![core.clone(), app_lib()],
            "core-ui cascades to the tip lib; `tool` depends on the tip and is not reached"
        );

        let plan = set.record_change(&graph, [&app_bin()]).unwrap();
        assert_eq!(plan.replay, vec![app_bin()]);
        assert_eq!(
            plan.modified,
            vec![core.clone(), app_lib(), app_bin()],
            "the second patch still links the first change's units"
        );

        let plan = set.record_change(&graph, [&base]).unwrap();
        assert_eq!(
            plan.replay,
            vec![base.clone(), core.clone(), app_lib()],
            "dependencies replay before dependents; other-app is outside the tip's closure"
        );
        assert_eq!(plan.modified, vec![base, core, app_lib(), app_bin()]);
        assert_eq!(set.units().len(), 4);
    }

    #[test]
    fn units_outside_the_running_image_are_dropped_and_unknown_units_refused() {
        let graph = graph();
        let mut set = ModifiedSet::new();
        let plan = set
            .record_change(
                &graph,
                [
                    &ReplayUnit::lib("other-app", "other_app"),
                    &ReplayUnit::lib("tool", "tool"),
                ],
            )
            .unwrap();
        assert!(plan.replay.is_empty() && plan.modified.is_empty());
        unsupported(set.record_change(
            &graph,
            [&ReplayUnit::lib("frust-material", "frust_material")],
        ));
        unsupported(set.record_change(&graph, [&ReplayUnit::bin("app", "nope")]));
    }

    #[test]
    fn replay_order_is_topological_with_lexicographic_ties() {
        let graph = graph();
        let units: BTreeSet<ReplayUnit> = [
            app_bin(),
            app_lib(),
            ReplayUnit::lib("other-app", "other_app"),
            ReplayUnit::lib("core-ui", "core_ui"),
            ReplayUnit::lib("base", "base"),
        ]
        .into_iter()
        .collect();
        let order: Vec<String> = graph
            .replay_order(&units)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            order,
            vec![
                "base/base.lib",
                "core-ui/core_ui.lib",
                "app/app.lib",
                "other-app/other_app.lib",
                "app/app.bin",
            ]
        );
    }

    #[test]
    fn a_dependency_cycle_has_no_replay_order() {
        let mut graph = graph();
        graph
            .member_deps
            .get_mut("base")
            .unwrap()
            .insert("core-ui".to_string());
        let units: BTreeSet<ReplayUnit> = [
            ReplayUnit::lib("base", "base"),
            ReplayUnit::lib("core-ui", "core_ui"),
        ]
        .into_iter()
        .collect();
        assert!(unsupported(graph.replay_order(&units)).contains("cycle"));
    }

    #[test]
    fn dev_dependency_edges_do_not_cascade() {
        let graph = graph();
        assert!(
            graph.dependents_of("tool").is_empty(),
            "app only dev-depends on tool"
        );
        assert_eq!(graph.dependents_of("app"), vec!["tool"]);
    }

    #[test]
    fn malformed_metadata_and_tips_are_builder_unsupported() {
        unsupported(WorkspaceGraph::from_metadata("not json", "app", None));
        let mut no_resolve: serde_json::Value =
            serde_json::from_str(&workspace_metadata()).unwrap();
        no_resolve["resolve"] = serde_json::Value::Null;
        assert!(
            unsupported(WorkspaceGraph::from_metadata(
                &no_resolve.to_string(),
                "app",
                None
            ))
            .contains("resolve")
        );
        unsupported(WorkspaceGraph::from_metadata(
            &workspace_metadata(),
            "serde",
            None,
        ));
        unsupported(WorkspaceGraph::from_metadata(
            &workspace_metadata(),
            "core-ui",
            None,
        ));
        unsupported(WorkspaceGraph::from_metadata(
            &workspace_metadata(),
            "app",
            Some("x"),
        ));
        assert_eq!(
            WorkspaceGraph::from_metadata(&workspace_metadata(), "app", Some("app"))
                .unwrap()
                .tip_bin(),
            app_bin()
        );
    }

    #[test]
    fn replay_cwd_follows_cargo_path_package_rule() {
        let graph = graph();
        assert_eq!(graph.replay_cwd(&app_lib()), PathBuf::from("/w"));
        assert_eq!(graph.workspace_root(), Path::new("/w"));
    }

    #[test]
    fn cargo_metadata_runs_through_the_runner() {
        let manifest = Path::new("/w/app/Cargo.toml");
        let key = "cargo metadata --format-version 1 --manifest-path /w/app/Cargo.toml \
                   --filter-platform aarch64-apple-darwin";
        let runner = FakeProcessRunner::new().with(
            key,
            Output {
                success: true,
                stdout: workspace_metadata(),
                stderr: String::new(),
            },
        );
        let graph =
            WorkspaceGraph::load(&runner, manifest, Some("aarch64-apple-darwin"), "app", None)
                .unwrap();
        assert_eq!(graph.tip(), "app");

        let failing = FakeProcessRunner::new().with(
            "cargo metadata",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: could not find `Cargo.toml`\n".to_string(),
            },
        );
        assert!(unsupported(cargo_metadata(&failing, manifest, None)).contains("Cargo.toml"));
        assert!(matches!(
            cargo_metadata(&FakeProcessRunner::new(), manifest, None),
            Err(HotpatchError::Process { .. })
        ));
    }

    #[test]
    fn dep_info_parses_every_rule_with_escapes_and_relative_paths() {
        let text = "/t/deps/my_app.d: src/lib.rs src/my\\ file.rs assets/x.txt\n\
                    \n\
                    /t/deps/libmy_app.rlib: src/lib.rs src/my\\ file.rs assets/x.txt /abs/gen.rs\n\
                    \n\
                    src/lib.rs:\n\
                    src/my\\ file.rs:\n\
                    assets/x.txt:\n\
                    \n\
                    # env-dep:CARGO_PKG_NAME=my-app\n";
        assert_eq!(
            parse_dep_info(text, Path::new("/p/my-app")),
            files(&[
                "/abs/gen.rs",
                "/p/my-app/assets/x.txt",
                "/p/my-app/src/lib.rs",
                "/p/my-app/src/my file.rs",
            ])
        );
        assert_eq!(
            parse_dep_info("C:\\t\\x.d: C:\\w\\src\\lib.rs\n", Path::new("C:\\w")).len(),
            1,
            "a drive colon is not the rule separator"
        );
    }

    #[test]
    fn dep_info_env_names_are_read_set_or_not_with_escapes_undone() {
        let text = "/t/deps/app.d: src/main.rs\n\
                    \n\
                    src/main.rs:\n\
                    \n\
                    # env-dep:APP_BUILD_STAMP=2026-10-08\n\
                    # env-dep:APP_OPTIONAL\n\
                    # env-dep:CARGO_PKG_NAME=my-app\n\
                    # env-dep:ODD\\\\NAME=a=b\n";
        let names: Vec<String> = parse_dep_info_env(text).into_iter().collect();
        assert_eq!(
            names,
            vec![
                "APP_BUILD_STAMP",
                "APP_OPTIONAL",
                "CARGO_PKG_NAME",
                "ODD\\NAME"
            ]
        );
        assert!(parse_dep_info_env("/t/deps/app.d: src/main.rs\n").is_empty());
    }
}
