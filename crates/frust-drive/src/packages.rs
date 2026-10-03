//! Where a project's dependency packages live on disk, answered by cargo.
//!
//! Post-scaffold code that needs a file shipped inside a framework crate — the
//! browser host page in `frust-shell-web`, a plugin's Gradle module or Swift
//! package — asks cargo where that package is rather than deriving the
//! location from the project's `frust` dependency. The question then has one
//! answer whatever form the dependency takes: a path into a checkout resolves
//! to the checkout's crate directory, and a crates.io version resolves to the
//! unpacked crate under cargo's registry source cache.
//!
//! [`PackageLocator`] is the seam. [`CargoLocator`] runs `cargo metadata`
//! through an injected [`ProcessRunner`]; [`CachedLocator`] remembers each
//! answer so a caller asking twice runs cargo once; `StubLocator` (built under
//! `test` or the `test-util` feature) answers from a map, so a fixture test
//! never spawns cargo against a fake project. [`locate`] and [`locate_many`]
//! are the cargo-backed shorthand for a caller holding no runner.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::process::{ProcessRunner, RealProcessRunner};

/// Why a package directory could not be resolved — a library contract callers
/// wrap into their own error enums (`docs/CODE_STANDARDS.md`'s Error
/// Handling convention).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackagesError {
    /// The directory holds no `Cargo.toml`, so there is no dependency graph
    /// to ask about — checked before cargo is spawned.
    #[error(
        "'{}' has no Cargo.toml, so cargo has no dependency graph to locate packages in",
        project_dir.display()
    )]
    NoManifest { project_dir: PathBuf },
    /// `cargo` itself could not be run.
    #[error(
        "could not run `cargo metadata` for '{}': {reason} — is cargo installed and on PATH?",
        project_dir.display()
    )]
    CargoUnavailable {
        project_dir: PathBuf,
        reason: String,
    },
    /// `cargo metadata` ran and failed; `stderr` is cargo's own explanation.
    #[error(
        "`cargo metadata` failed for '{}' — a project that depends on the published frust \
         crates needs network access once, to download them:\n{stderr}",
        project_dir.display()
    )]
    MetadataFailed {
        project_dir: PathBuf,
        stderr: String,
    },
    /// `cargo metadata` succeeded but its output did not have the documented
    /// `--format-version 1` shape.
    #[error("`cargo metadata` for '{}' printed unreadable output: {reason}", project_dir.display())]
    MetadataUnreadable {
        project_dir: PathBuf,
        reason: String,
    },
    /// The package is not in the project's dependency graph.
    /// `frust_dependency` is [`describe_frust_dependency`]'s rendering of how
    /// the project depends on the framework, since that dependency is what
    /// brings every framework package into the graph.
    #[error(
        "package `{package}` is not in the dependency graph of '{}' — the project's frust \
         dependency is {frust_dependency}",
        project_dir.display()
    )]
    PackageAbsent {
        project_dir: PathBuf,
        package: String,
        frust_dependency: String,
    },
}

/// Resolves package names to the directories holding their `Cargo.toml`.
pub trait PackageLocator {
    /// The directory of each of `packages`, in the order given, as resolved in
    /// the dependency graph of the Cargo project at `project_dir`. Fails as a
    /// whole if any one of them cannot be resolved.
    fn locate_many(
        &self,
        project_dir: &Path,
        packages: &[&str],
    ) -> Result<Vec<PathBuf>, PackagesError>;

    /// [`locate_many`](Self::locate_many) for a single package.
    fn locate(&self, project_dir: &Path, package: &str) -> Result<PathBuf, PackagesError> {
        let mut dirs = self.locate_many(project_dir, &[package])?;
        dirs.pop().ok_or_else(|| PackagesError::MetadataUnreadable {
            project_dir: project_dir.to_path_buf(),
            reason: format!("no directory came back for `{package}`"),
        })
    }
}

/// How [`CargoLocator`] treats a project's existing `Cargo.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockfilePolicy {
    /// Pass `--locked` whenever `<project>/Cargo.lock` exists, so resolving a
    /// location never rewrites the project's lockfile.
    #[default]
    Respect,
    /// Never pass `--locked`: cargo may add entries to `Cargo.lock`. For a
    /// caller that has just added a dependency to the manifest, where a
    /// `--locked` resolve is guaranteed to fail.
    AllowUpdate,
}

/// The real [`PackageLocator`]: one `cargo metadata --format-version 1` run
/// (dependencies included, unlike the `--no-deps` target-directory probes
/// elsewhere in this crate) per [`locate_many`](PackageLocator::locate_many)
/// call, through the injected runner.
pub struct CargoLocator<'r> {
    runner: &'r dyn ProcessRunner,
    lockfile: LockfilePolicy,
}

impl<'r> CargoLocator<'r> {
    pub fn new(runner: &'r dyn ProcessRunner) -> Self {
        Self {
            runner,
            lockfile: LockfilePolicy::Respect,
        }
    }

    /// The same locator under `policy`.
    pub fn with_lockfile_policy(mut self, policy: LockfilePolicy) -> Self {
        self.lockfile = policy;
        self
    }

    /// The `cargo` argv for `project_dir`'s manifest at `manifest`.
    fn metadata_args(&self, project_dir: &Path, manifest: &Path) -> Vec<String> {
        let mut args = vec![
            "metadata".to_string(),
            "--format-version".to_string(),
            "1".to_string(),
            "--manifest-path".to_string(),
            manifest.to_string_lossy().into_owned(),
        ];
        if self.lockfile == LockfilePolicy::Respect && project_dir.join("Cargo.lock").is_file() {
            args.push("--locked".to_string());
        }
        args
    }
}

impl PackageLocator for CargoLocator<'_> {
    fn locate_many(
        &self,
        project_dir: &Path,
        packages: &[&str],
    ) -> Result<Vec<PathBuf>, PackagesError> {
        let manifest = project_dir.join("Cargo.toml");
        if !manifest.is_file() {
            return Err(PackagesError::NoManifest {
                project_dir: project_dir.to_path_buf(),
            });
        }
        let args = self.metadata_args(project_dir, &manifest);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let out =
            self.runner
                .run("cargo", &argv)
                .map_err(|err| PackagesError::CargoUnavailable {
                    project_dir: project_dir.to_path_buf(),
                    reason: format!("{err:#}"),
                })?;
        if !out.success {
            return Err(PackagesError::MetadataFailed {
                project_dir: project_dir.to_path_buf(),
                stderr: out.stderr.trim().to_string(),
            });
        }
        packages_from_metadata(project_dir, &out.stdout, packages)
    }
}

/// Picks `packages` out of a `cargo metadata --format-version 1` document:
/// each name's first `packages[]` entry, as the parent of its
/// `manifest_path`. Split from [`CargoLocator`] so the parse is tested
/// against canned JSON rather than a cargo run.
pub(crate) fn packages_from_metadata(
    project_dir: &Path,
    metadata: &str,
    packages: &[&str],
) -> Result<Vec<PathBuf>, PackagesError> {
    let unreadable = |reason: String| PackagesError::MetadataUnreadable {
        project_dir: project_dir.to_path_buf(),
        reason,
    };
    let doc: serde_json::Value =
        serde_json::from_str(metadata).map_err(|err| unreadable(err.to_string()))?;
    let listed = doc
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| unreadable("no `packages` array".to_string()))?;

    let mut dirs = Vec::with_capacity(packages.len());
    for wanted in packages {
        let entry = listed
            .iter()
            .find(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(*wanted));
        let Some(entry) = entry else {
            return Err(PackagesError::PackageAbsent {
                project_dir: project_dir.to_path_buf(),
                package: (*wanted).to_string(),
                frust_dependency: describe_frust_dependency(project_dir),
            });
        };
        let manifest_path = entry
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| unreadable(format!("`{wanted}` carries no `manifest_path`")))?;
        let dir = Path::new(manifest_path)
            .parent()
            .ok_or_else(|| unreadable(format!("`{wanted}`'s manifest path has no parent")))?;
        dirs.push(dir.to_path_buf());
    }
    Ok(dirs)
}

/// How the project at `project_dir` declares its `frust` dependency, for an
/// error message: the entry as written (`` `frust = { package = "frust-ui",
/// version = "0.5.0" }` ``), or a sentence saying there is none.
pub fn describe_frust_dependency(project_dir: &Path) -> String {
    let declared = fs::read_to_string(project_dir.join("Cargo.toml"))
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|doc| {
            let item = doc.get("dependencies")?.as_table_like()?.get("frust")?;
            let rendered = match item {
                toml_edit::Item::Value(value) => value.to_string(),
                toml_edit::Item::Table(table) => table.clone().into_inline_table().to_string(),
                _ => return None,
            };
            Some(rendered.trim().to_string())
        });
    match declared {
        Some(entry) => format!("`frust = {entry}`"),
        None => "absent — its `[dependencies]` has no `frust` entry".to_string(),
    }
}

/// Wraps another locator and remembers each `(project, package)` answer —
/// errors included — for its own lifetime, so one pipeline run that needs
/// the same location from several places spawns cargo once.
pub struct CachedLocator<'a> {
    inner: &'a dyn PackageLocator,
    answers: RefCell<HashMap<(PathBuf, String), Result<PathBuf, PackagesError>>>,
}

impl<'a> CachedLocator<'a> {
    pub fn new(inner: &'a dyn PackageLocator) -> Self {
        Self {
            inner,
            answers: RefCell::new(HashMap::new()),
        }
    }
}

impl PackageLocator for CachedLocator<'_> {
    fn locate_many(
        &self,
        project_dir: &Path,
        packages: &[&str],
    ) -> Result<Vec<PathBuf>, PackagesError> {
        let key = |package: &str| (project_dir.to_path_buf(), package.to_string());
        let unanswered: Vec<&str> = {
            let answers = self.answers.borrow();
            packages
                .iter()
                .copied()
                .filter(|package| !answers.contains_key(&key(package)))
                .collect()
        };
        if !unanswered.is_empty() {
            let resolved = self.inner.locate_many(project_dir, &unanswered);
            let mut answers = self.answers.borrow_mut();
            match resolved {
                Ok(dirs) => {
                    for (package, dir) in unanswered.iter().zip(dirs) {
                        answers.insert(key(package), Ok(dir));
                    }
                }
                Err(err) => {
                    for package in &unanswered {
                        answers.insert(key(package), Err(err.clone()));
                    }
                }
            }
        }
        let answers = self.answers.borrow();
        packages
            .iter()
            .map(|package| {
                answers.get(&key(package)).cloned().unwrap_or_else(|| {
                    Err(PackagesError::MetadataUnreadable {
                        project_dir: project_dir.to_path_buf(),
                        reason: format!("no directory came back for `{package}`"),
                    })
                })
            })
            .collect()
    }
}

/// The directory of `package` in the dependency graph of the Cargo project at
/// `project_dir`, resolved by `cargo metadata` (`--locked` when the project
/// has a `Cargo.lock`).
pub fn locate(project_dir: &Path, package: &str) -> Result<PathBuf, PackagesError> {
    CargoLocator::new(&RealProcessRunner).locate(project_dir, package)
}

/// [`locate`] for several packages in one `cargo metadata` run; directories
/// come back in the order `packages` names them.
pub fn locate_many(project_dir: &Path, packages: &[&str]) -> Result<Vec<PathBuf>, PackagesError> {
    CargoLocator::new(&RealProcessRunner).locate_many(project_dir, packages)
}

/// A [`PackageLocator`] answering from a map instead of cargo — the fixture
/// seam for tests whose projects point at a fake checkout no real cargo
/// resolve could read. An unmapped package answers
/// [`PackagesError::PackageAbsent`] exactly as cargo's absence would, and
/// [`failing`](Self::failing) stands in for a `cargo metadata` failure.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Default)]
pub struct StubLocator {
    dirs: HashMap<String, PathBuf>,
    failure: Option<String>,
    calls: std::sync::atomic::AtomicUsize,
}

#[cfg(any(test, feature = "test-util"))]
impl StubLocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers `dir` for `package`.
    pub fn with(mut self, package: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        self.dirs.insert(package.into(), dir.into());
        self
    }

    /// Fails every lookup the way a failed `cargo metadata` run does, with
    /// `stderr` as cargo's message.
    pub fn failing(stderr: impl Into<String>) -> Self {
        Self {
            failure: Some(stderr.into()),
            ..Self::default()
        }
    }

    /// How many [`locate_many`](PackageLocator::locate_many) calls reached
    /// this stub — what a real locator would have spent on cargo runs.
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(any(test, feature = "test-util"))]
impl PackageLocator for StubLocator {
    fn locate_many(
        &self,
        project_dir: &Path,
        packages: &[&str],
    ) -> Result<Vec<PathBuf>, PackagesError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(stderr) = &self.failure {
            return Err(PackagesError::MetadataFailed {
                project_dir: project_dir.to_path_buf(),
                stderr: stderr.clone(),
            });
        }
        packages
            .iter()
            .map(|package| {
                self.dirs
                    .get(*package)
                    .cloned()
                    .ok_or_else(|| PackagesError::PackageAbsent {
                        project_dir: project_dir.to_path_buf(),
                        package: (*package).to_string(),
                        frust_dependency: describe_frust_dependency(project_dir),
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use crate::scaffold::{self, FrustDependency, TemplateContext};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-packages-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A trimmed `cargo metadata --format-version 1` document in cargo's
    /// real shape: the keys this module reads plus neighbours it must ignore.
    const METADATA: &str = r#"{
        "packages": [
            {
                "name": "frust-ui",
                "version": "0.5.0",
                "id": "path+file:///work/frust/crates/frust#frust-ui@0.5.0",
                "source": null,
                "manifest_path": "/work/frust/crates/frust/Cargo.toml"
            },
            {
                "name": "frust-shell-web",
                "version": "0.5.0",
                "id": "registry+https://github.com/rust-lang/crates.io-index#frust-shell-web@0.5.0",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "manifest_path": "/home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/frust-shell-web-0.5.0/Cargo.toml"
            },
            {
                "name": "frust-iap",
                "version": "0.5.0",
                "id": "path+file:///work/frust/plugins/iap#frust-iap@0.5.0",
                "source": null,
                "manifest_path": "/work/frust/plugins/iap/Cargo.toml"
            }
        ],
        "workspace_members": [],
        "resolve": null,
        "target_directory": "/work/app/target",
        "version": 1,
        "workspace_root": "/work/app"
    }"#;

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// A project directory carrying only a manifest, so [`CargoLocator`]'s
    /// up-front manifest check passes and the runner is reached.
    fn project_with(tag: &str, manifest: &str) -> PathBuf {
        let dir = temp_dir(tag);
        fs::write(dir.join("Cargo.toml"), manifest).unwrap();
        dir
    }

    fn metadata_key(project: &Path) -> String {
        format!(
            "cargo metadata --format-version 1 --manifest-path {}",
            project.join("Cargo.toml").display()
        )
    }

    #[test]
    fn the_metadata_parse_returns_manifest_parents_in_request_order() {
        let dirs = packages_from_metadata(
            Path::new("/work/app"),
            METADATA,
            &["frust-iap", "frust-shell-web", "frust-ui"],
        )
        .unwrap();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/work/frust/plugins/iap"),
                PathBuf::from(
                    "/home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/frust-shell-web-0.5.0"
                ),
                PathBuf::from("/work/frust/crates/frust"),
            ]
        );
    }

    #[test]
    fn an_absent_package_names_the_projects_frust_dependency() {
        let project = project_with(
            "absent",
            "[package]\nname = \"a\"\n\n[dependencies]\nfrust = { package = \"frust-ui\", version = \"0.5.0\" }\n",
        );
        let err = packages_from_metadata(&project, METADATA, &["frust-camera"]).unwrap_err();
        let message = err.to_string();
        assert!(
            matches!(&err, PackagesError::PackageAbsent { package, .. } if package == "frust-camera"),
            "{err:?}"
        );
        assert!(
            message.contains(r#"`frust = { package = "frust-ui", version = "0.5.0" }`"#),
            "{message}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn the_frust_dependency_description_covers_every_declaration_form() {
        for (tag, manifest, expected) in [
            (
                "path",
                "[dependencies]\nfrust = { package = \"frust-ui\", path = \"../frust/crates/frust\" }\n",
                r#"`frust = { package = "frust-ui", path = "../frust/crates/frust" }`"#,
            ),
            (
                "shorthand",
                "[dependencies]\nfrust = \"0.5.0\"\n",
                r#"`frust = "0.5.0"`"#,
            ),
            (
                "table",
                "[dependencies.frust]\npackage = \"frust-ui\"\nversion = \"0.5.0\"\n",
                r#"`frust = { package = "frust-ui", version = "0.5.0" }`"#,
            ),
            (
                "absent",
                "[dependencies]\nlog = \"0.4\"\n",
                "absent — its `[dependencies]` has no `frust` entry",
            ),
        ] {
            let project = project_with(tag, manifest);
            assert_eq!(describe_frust_dependency(&project), expected, "{tag}");
            let _ = fs::remove_dir_all(&project);
        }
    }

    #[test]
    fn unreadable_metadata_is_a_typed_error() {
        for bad in [
            "not json",
            "{\"version\":1}",
            r#"{"packages":[{"name":"x"}]}"#,
        ] {
            let err = packages_from_metadata(Path::new("/p"), bad, &["x"]).unwrap_err();
            assert!(
                matches!(err, PackagesError::MetadataUnreadable { .. }),
                "{bad}: {err:?}"
            );
        }
    }

    #[test]
    fn the_cargo_locator_runs_full_metadata_through_the_runner() {
        let project = project_with("runner", "[package]\nname = \"a\"\n");
        let runner = FakeProcessRunner::new().with(metadata_key(&project), ok(METADATA));
        let dir = CargoLocator::new(&runner)
            .locate(&project, "frust-iap")
            .unwrap();
        assert_eq!(dir, PathBuf::from("/work/frust/plugins/iap"));
        let _ = fs::remove_dir_all(&project);
    }

    /// `--locked` follows the lockfile: present when `Cargo.lock` exists under
    /// the default policy, absent without one, and never under
    /// [`LockfilePolicy::AllowUpdate`].
    #[test]
    fn locked_is_passed_only_when_a_lockfile_exists_and_is_respected() {
        let project = project_with("locked", "[package]\nname = \"a\"\n");
        let manifest = project.join("Cargo.toml");
        let runner = FakeProcessRunner::new();
        let respect = CargoLocator::new(&runner);
        assert!(
            !respect
                .metadata_args(&project, &manifest)
                .contains(&"--locked".to_string())
        );
        assert!(
            !respect
                .metadata_args(&project, &manifest)
                .contains(&"--no-deps".to_string()),
            "locating a dependency needs the dependency graph"
        );
        fs::write(project.join("Cargo.lock"), "version = 4\n").unwrap();
        assert_eq!(
            respect
                .metadata_args(&project, &manifest)
                .last()
                .map(String::as_str),
            Some("--locked")
        );
        let update = CargoLocator::new(&runner).with_lockfile_policy(LockfilePolicy::AllowUpdate);
        assert!(
            !update
                .metadata_args(&project, &manifest)
                .contains(&"--locked".to_string())
        );
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn a_missing_cargo_is_cargo_unavailable() {
        let project = project_with("no-cargo", "[package]\nname = \"a\"\n");
        let runner = FakeProcessRunner::new().missing(metadata_key(&project));
        let err = CargoLocator::new(&runner)
            .locate(&project, "frust-ui")
            .unwrap_err();
        assert!(
            matches!(err, PackagesError::CargoUnavailable { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&project);
    }

    /// cargo's own stderr is the explanation a user needs (an unreachable
    /// registry, a path dependency that moved), so it is carried verbatim.
    #[test]
    fn a_failed_metadata_run_surfaces_cargos_stderr() {
        let project = project_with("metadata-fails", "[package]\nname = \"a\"\n");
        let runner = FakeProcessRunner::new().with(
            metadata_key(&project),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: failed to get `frust-ui` as a dependency of package `a`\n"
                    .to_string(),
            },
        );
        let err = CargoLocator::new(&runner)
            .locate(&project, "frust-ui")
            .unwrap_err();
        let message = err.to_string();
        assert!(
            matches!(err, PackagesError::MetadataFailed { .. }),
            "{err:?}"
        );
        assert!(message.contains("failed to get `frust-ui`"), "{message}");
        assert!(message.contains("network access once"), "{message}");
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn a_directory_without_a_manifest_never_spawns_cargo() {
        let dir = temp_dir("no-manifest");
        let runner = FakeProcessRunner::new();
        let err = CargoLocator::new(&runner)
            .locate(&dir, "frust-ui")
            .unwrap_err();
        assert!(matches!(err, PackagesError::NoManifest { .. }), "{err:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stub_answers_from_its_map_and_reports_absence_like_cargo() {
        let project = project_with(
            "stub",
            "[dependencies]\nfrust = { package = \"frust-ui\", path = \"../frust\" }\n",
        );
        let stub = StubLocator::new().with("frust-shell-web", "/checkout/crates/frust-shell-web");
        assert_eq!(
            stub.locate(&project, "frust-shell-web").unwrap(),
            PathBuf::from("/checkout/crates/frust-shell-web")
        );
        let err = stub.locate(&project, "frust-camera").unwrap_err();
        assert!(err.to_string().contains("path = \"../frust\""), "{err}");
        assert!(matches!(
            StubLocator::failing("offline").locate(&project, "frust-ui"),
            Err(PackagesError::MetadataFailed { .. })
        ));
        let _ = fs::remove_dir_all(&project);
    }

    /// One inner call per distinct name, errors remembered as well as answers.
    #[test]
    fn the_cache_asks_its_inner_locator_once_per_package() {
        let project = temp_dir("cache");
        let stub = StubLocator::new().with("frust-ui", "/f/crates/frust");
        let cached = CachedLocator::new(&stub);
        for _ in 0..3 {
            assert_eq!(
                cached.locate(&project, "frust-ui").unwrap(),
                PathBuf::from("/f/crates/frust")
            );
        }
        assert_eq!(stub.calls(), 1);
        assert!(cached.locate(&project, "frust-camera").is_err());
        assert!(cached.locate(&project, "frust-camera").is_err());
        assert_eq!(stub.calls(), 2);
        let _ = fs::remove_dir_all(&project);
    }

    /// The workspace root of this checkout (`crates/frust-drive/../..`).
    fn checkout_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    /// End to end against real cargo: a path-mode project generated with
    /// `--frust-path` = this checkout resolves `frust-shell-web` to the
    /// checkout's own crate directory. The checkout's `Cargo.lock` seeds the
    /// generated project's, so cargo resolves the framework graph from the
    /// versions the workspace already pins rather than fetching an index; the
    /// first, lock-updating resolve then leaves a lockfile the default
    /// (`--locked`) [`locate_many`] reads as-is.
    #[test]
    fn real_cargo_locates_the_shell_crate_of_a_generated_path_mode_project() {
        let root = checkout_root();
        let project = temp_dir("real-cargo").join("my_app");
        let ctx = TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: env!("CARGO_PKG_VERSION").into(),
            frust: FrustDependency::Path(root.join("crates/frust").to_string_lossy().into_owned()),
            deeplink_scheme: None,
            deeplink_host: None,
        };
        scaffold::generate(&project, &ctx, None, false, None).unwrap();
        fs::copy(root.join("Cargo.lock"), project.join("Cargo.lock")).unwrap();

        let updating = CargoLocator::new(&RealProcessRunner)
            .with_lockfile_policy(LockfilePolicy::AllowUpdate)
            .locate(&project, "frust-ui")
            .unwrap();
        assert_eq!(updating.canonicalize().unwrap(), root.join("crates/frust"));

        let dirs = locate_many(&project, &["frust-shell-web", "frust-glyph"]).unwrap();
        assert_eq!(
            dirs.iter()
                .map(|dir| dir.canonicalize().unwrap())
                .collect::<Vec<_>>(),
            vec![
                root.join("crates/frust-shell-web"),
                root.join("plugins/glyph")
            ]
        );
        assert!(
            dirs[0].join("platform/web/index.html").is_file(),
            "the located crate carries the browser host page"
        );
        let _ = fs::remove_dir_all(project.parent().unwrap());
    }
}
