//! Which dependency key an app's `Cargo.toml` gives the framework facade.
//!
//! cargo resolves `--features <dep>/<feature>` against the dependency *key*,
//! not the package name. The facade is published as `frust-ui` and imported
//! as `frust`, so an app may key it `frust = { package = "frust-ui", .. }`
//! (what `frust create` writes), `frust-ui = "0.6.0"` (the crates.io name), or
//! any other rename. The drive's own feature literals are spelled with the
//! canonical `frust/` prefix ([`crate::build_info::BuildMode::cargo_features`]);
//! [`FacadeKey::map`] rewrites them to the app's actual key at the argv
//! boundary.
//!
//! [`FacadeKey::resolve`] asks cargo first (`cargo metadata --no-deps`
//! through [`CargoLocator`]'s invocation seam), which sees a
//! `workspace = true` entry's inherited rename. When cargo cannot answer it
//! falls back to reading the app manifest's `[dependencies]` and
//! `[target.*.dependencies]` tables directly; that fallback **cannot see a
//! workspace-inherited rename** — a `ui.workspace = true` entry carries no
//! `package` field, so it reads as a dependency on a package named `ui`.
//! [`FacadeKey::resolve_with_source`] reports which path answered
//! ([`KeySource`]), and the release lanes' devtools refusal
//! (`cargo_manifest::feature_enables_devtools`) **fails closed** on a
//! [`KeySource::ManifestFallback`] key: since that key may be wrong, every
//! `<dep>/devtools` or `<dep>/hotpatch` spelling counts there, not only the
//! resolved key's.
//!
//! Every resolved key must be spellable in a cargo feature — the
//! dependency-key charset `[A-Za-z0-9_-]+` — or resolution answers
//! [`FacadeKeyError::InvalidKey`] rather than a key no `<key>/<feature>`
//! could name.

use std::fmt;
use std::path::Path;

use toml_edit::{DocumentMut, Item, TableLike};

use crate::cargo_manifest;
use crate::packages::CargoLocator;
use crate::process::{ProcessRunner, RealProcessRunner};

/// The key every scaffolded app gives the facade, and the prefix the drive's
/// canonical feature literals carry.
const CANONICAL_KEY: &str = "frust";

/// The facade's package names: the published `frust-ui` and the older
/// in-repo `frust`.
const FACADE_PACKAGES: [&str; 2] = ["frust-ui", "frust"];

/// The dependency key an app's manifest gives the framework facade.
/// [`Default`] is the scaffolded `frust`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FacadeKey(String);

/// Why the facade key could not be resolved — a library contract callers wrap
/// into their own errors (`docs/CODE_STANDARDS.md`'s Error Handling).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FacadeKeyError {
    /// The app depends on the facade under two or more different keys, so a
    /// `frust/<feature>` has no single spelling.
    #[error(
        "the app's Cargo.toml depends on the frust facade under more than one key ({}); \
         keep a single facade dependency so its cargo features have one spelling",
        keys.iter().map(|key| format!("`{key}`")).collect::<Vec<_>>().join(", ")
    )]
    Ambiguous { keys: Vec<String> },
    /// The app keys the facade under a name outside the dependency-key
    /// charset `[A-Za-z0-9_-]+` (a TOML quoted key such as `"fr ust"`), so
    /// no `<key>/<feature>` can spell its features.
    #[error(
        "the app's Cargo.toml names the frust facade under the dependency key {key:?}, which a \
         cargo feature cannot spell (expected [A-Za-z0-9_-]+); give the facade dependency a plain \
         key such as `frust`"
    )]
    InvalidKey { key: String },
}

/// Which path [`FacadeKey::resolve_with_source`] answered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeySource {
    /// The project has no `Cargo.toml`; the default key answered.
    NoManifest,
    /// `cargo metadata --no-deps` named the root package's dependencies.
    Cargo,
    /// cargo could not answer, so the manifest's own tables were read — the
    /// path that cannot see a workspace-inherited rename (see the module doc).
    ManifestFallback,
}

impl Default for FacadeKey {
    fn default() -> Self {
        Self(CANONICAL_KEY.to_string())
    }
}

impl fmt::Display for FacadeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FacadeKey {
    /// A facade key spelled `key`, for a caller that already knows it.
    /// Unchecked: its callers pass literals (or keys a resolution already
    /// reported), so the charset check [`resolve_with`](Self::resolve_with)
    /// applies is not repeated here.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The facade key of the Cargo project at `project_dir`, asking cargo
    /// through the real process runner. See [`resolve_with`](Self::resolve_with).
    pub fn resolve(project_dir: &Path) -> Result<Self, FacadeKeyError> {
        Self::resolve_with(&RealProcessRunner, project_dir)
    }

    /// The facade key of the Cargo project at `project_dir`: the root
    /// package's normal dependencies (including target-specific ones) whose
    /// package is `frust-ui` or `frust`, keyed by their rename when they have
    /// one. Read from `cargo metadata --no-deps` through `runner`; on any
    /// metadata failure, from the manifest's own tables (see the module doc
    /// for what that fallback cannot see). No manifest, or no facade
    /// dependency, answers [`FacadeKey::default`]; two different keys are
    /// [`FacadeKeyError::Ambiguous`], and a key outside the dependency-key
    /// charset is [`FacadeKeyError::InvalidKey`].
    pub fn resolve_with(
        runner: &dyn ProcessRunner,
        project_dir: &Path,
    ) -> Result<Self, FacadeKeyError> {
        Self::resolve_with_source(runner, project_dir).0
    }

    /// [`resolve_with`](Self::resolve_with), also reporting which path
    /// answered. The source accompanies an error too: a caller widening a
    /// security check must know whether an [`FacadeKeyError::Ambiguous`] key
    /// list came from cargo or from the blind manifest fallback.
    pub(crate) fn resolve_with_source(
        runner: &dyn ProcessRunner,
        project_dir: &Path,
    ) -> (Result<Self, FacadeKeyError>, KeySource) {
        if !project_dir.join("Cargo.toml").is_file() {
            return (Ok(Self::default()), KeySource::NoManifest);
        }
        let (keys, source) = match CargoLocator::new(runner).root_dependencies(project_dir) {
            Ok(deps) => (
                deps.into_iter()
                    .filter(|dep| dep.kind.is_none() && is_facade_package(&dep.name))
                    .map(|dep| dep.rename.unwrap_or(dep.name))
                    .collect(),
                KeySource::Cargo,
            ),
            Err(_) => (
                cargo_manifest::read_app_manifest(project_dir)
                    .map(|doc| manifest_facade_keys(&doc))
                    .unwrap_or_default(),
                KeySource::ManifestFallback,
            ),
        };
        (Self::select(keys), source)
    }

    /// One key out of every facade dependency found: any key outside the
    /// dependency-key charset is invalid, none is the default, repeats of one
    /// key (a `[dependencies]` entry plus a target-specific one) are that
    /// key, and two different keys are ambiguous.
    fn select(keys: Vec<String>) -> Result<Self, FacadeKeyError> {
        if let Some(key) = keys.iter().find(|key| !is_dependency_key(key)) {
            return Err(FacadeKeyError::InvalidKey { key: key.clone() });
        }
        let mut distinct: Vec<String> = Vec::new();
        for key in keys {
            if !distinct.contains(&key) {
                distinct.push(key);
            }
        }
        match distinct.len() {
            0 => Ok(Self::default()),
            1 => Ok(Self(distinct.remove(0))),
            _ => Err(FacadeKeyError::Ambiguous { keys: distinct }),
        }
    }

    /// The key as cargo spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `canonical` as this app's cargo must spell it: a leading `frust/` is
    /// rewritten to `<key>/`; any other string — the app's own `lean`, a
    /// `<other-dep>/<feature>` — is returned unchanged.
    pub fn map(&self, canonical: &str) -> String {
        match canonical.strip_prefix("frust/") {
            Some(feature) => format!("{}/{feature}", self.0),
            None => canonical.to_string(),
        }
    }

    /// The one-line hint a front-end appends to a cargo feature error, naming
    /// both dependency forms and the feature spelling each one takes.
    pub fn hint() -> &'static str {
        "hint: frust features follow the app's dependency key — \
         `frust = { package = \"frust-ui\", .. }` takes `frust/<feature>`, \
         `frust-ui = \"<version>\"` takes `frust-ui/<feature>`"
    }
}

/// Whether `key` is spellable as the `<key>` of a cargo `<key>/<feature>`:
/// non-empty and within the crate-name/dependency-key charset
/// `[A-Za-z0-9_-]`.
fn is_dependency_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `package` names the framework facade.
fn is_facade_package(package: &str) -> bool {
    FACADE_PACKAGES.contains(&package)
}

/// The facade entries of one dependency table, as `(key, entry)` in document
/// order: every entry whose package — its `package = ".."` field, else its
/// key — is `frust-ui` or `frust`. Shared by every manifest reader that looks
/// for the facade, so a `frust-ui`-keyed or renamed facade is found the same
/// way everywhere.
pub(crate) fn facade_entries<'a>(
    deps: &'a dyn TableLike,
) -> impl Iterator<Item = (&'a str, &'a Item)> + 'a {
    deps.iter().filter(|(key, item)| {
        let package = item
            .as_table_like()
            .and_then(|table| table.get("package"))
            .and_then(Item::as_str)
            .unwrap_or(key);
        is_facade_package(package)
    })
}

/// Every facade key in `doc`'s `[dependencies]` and
/// `[target.<cfg>.dependencies]` tables, in document order.
fn manifest_facade_keys(doc: &DocumentMut) -> Vec<String> {
    let targets = doc
        .get("target")
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|targets| targets.iter())
        .filter_map(|(_, target)| target.as_table_like()?.get("dependencies"));
    doc.get("dependencies")
        .into_iter()
        .chain(targets)
        .filter_map(Item::as_table_like)
        .flat_map(|deps| facade_entries(deps).map(|(key, _)| key.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-facade-key-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project_with(tag: &str, manifest: &str) -> PathBuf {
        let dir = temp_dir(tag);
        fs::write(dir.join("Cargo.toml"), manifest).unwrap();
        dir
    }

    const PACKAGE: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n";

    /// Resolves through the manifest fallback: a runner with no scripted
    /// response fails every cargo run, as a missing cargo would.
    fn resolve_from_manifest(tag: &str, manifest: &str) -> Result<FacadeKey, FacadeKeyError> {
        let project = project_with(tag, manifest);
        let resolved = FacadeKey::resolve_with(&FakeProcessRunner::new(), &project);
        let _ = fs::remove_dir_all(&project);
        resolved
    }

    fn key(key: &str) -> Result<FacadeKey, FacadeKeyError> {
        Ok(FacadeKey(key.to_string()))
    }

    fn metadata_key(project: &Path) -> String {
        format!(
            "cargo metadata --format-version 1 --no-deps --manifest-path {}",
            project.join("Cargo.toml").display()
        )
    }

    /// A trimmed `cargo metadata --no-deps` document: the root package at
    /// `project`, a sibling workspace member, and `deps` as the root's
    /// `dependencies[]` entries.
    fn metadata(project: &Path, deps: &str) -> String {
        let manifest = serde_json::to_string(&project.join("Cargo.toml")).unwrap();
        format!(
            r#"{{
            "packages": [
                {{"name":"tool","version":"0.1.0","id":"tool","manifest_path":"/elsewhere/tool/Cargo.toml",
                  "dependencies":[{{"name":"frust-ui","rename":"other","kind":null,"target":null}}]}},
                {{"name":"app","version":"0.1.0","id":"app","manifest_path":{manifest},
                  "dependencies":[{deps}]}}
            ],
            "workspace_members": ["app", "tool"],
            "resolve": null,
            "version": 1
        }}"#
        )
    }

    fn resolve_from_metadata(
        tag: &str,
        manifest: &str,
        deps: &str,
    ) -> Result<FacadeKey, FacadeKeyError> {
        let project = project_with(tag, manifest);
        let runner = FakeProcessRunner::new().with(
            metadata_key(&project),
            Output {
                success: true,
                stdout: metadata(&project, deps),
                stderr: String::new(),
            },
        );
        let resolved = FacadeKey::resolve_with(&runner, &project);
        let _ = fs::remove_dir_all(&project);
        resolved
    }

    #[test]
    fn the_scaffolded_frust_key_on_package_frust_ui() {
        assert_eq!(
            resolve_from_manifest(
                "scaffolded",
                &format!(
                    "{PACKAGE}[dependencies]\nfrust = {{ package = \"frust-ui\", version = \"0.6.0\" }}\n"
                ),
            ),
            key("frust")
        );
        assert_eq!(
            resolve_from_metadata(
                "scaffolded-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":"frust","kind":null,"target":null}"#,
            ),
            key("frust")
        );
    }

    #[test]
    fn the_unrenamed_crates_io_key() {
        assert_eq!(
            resolve_from_manifest(
                "frust-ui",
                &format!("{PACKAGE}[dependencies]\nfrust-ui = \"0.6.0\"\n"),
            ),
            key("frust-ui")
        );
        assert_eq!(
            resolve_from_metadata(
                "frust-ui-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":null,"kind":null,"target":null}"#,
            ),
            key("frust-ui")
        );
    }

    #[test]
    fn an_arbitrary_rename() {
        assert_eq!(
            resolve_from_manifest(
                "rename",
                &format!(
                    "{PACKAGE}[dependencies]\nui = {{ package = \"frust-ui\", version = \"0.6\" }}\n"
                ),
            ),
            key("ui")
        );
    }

    #[test]
    fn a_target_specific_dependency_table() {
        assert_eq!(
            resolve_from_manifest(
                "target",
                &format!(
                    "{PACKAGE}[target.'cfg(unix)'.dependencies]\nui = {{ package = \"frust-ui\", version = \"0.6\" }}\n"
                ),
            ),
            key("ui")
        );
        // The same key in both tables is one key, not an ambiguity.
        assert_eq!(
            resolve_from_manifest(
                "target-repeat",
                &format!(
                    "{PACKAGE}[dependencies]\nfrust-ui = \"0.6\"\n\n\
                     [target.'cfg(unix)'.dependencies]\nfrust-ui = {{ version = \"0.6\", features = [\"devtools\"] }}\n"
                ),
            ),
            key("frust-ui")
        );
        assert_eq!(
            resolve_from_metadata(
                "target-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":"ui","kind":null,"target":"cfg(unix)"}"#,
            ),
            key("ui")
        );
    }

    /// cargo reports a `workspace = true` entry with the rename its workspace
    /// declares — what the manifest fallback alone cannot see.
    #[test]
    fn a_workspace_inherited_rename_is_read_from_metadata() {
        let manifest = format!("{PACKAGE}[dependencies]\nui.workspace = true\n");
        assert_eq!(
            resolve_from_metadata(
                "workspace-md",
                &manifest,
                r#"{"name":"frust-ui","rename":"ui","kind":null,"target":null}"#,
            ),
            key("ui")
        );
        // The documented fallback limitation: without cargo, `ui` reads as a
        // dependency on a package named `ui`, and the default answers.
        assert_eq!(
            resolve_from_manifest("workspace-fallback", &manifest),
            key("frust")
        );
    }

    #[test]
    fn the_legacy_frust_package() {
        assert_eq!(
            resolve_from_manifest(
                "legacy",
                &format!("{PACKAGE}[dependencies]\nfrust = \"0.4.0\"\n"),
            ),
            key("frust")
        );
        assert_eq!(
            resolve_from_metadata(
                "legacy-md",
                PACKAGE,
                r#"{"name":"frust","rename":null,"kind":null,"target":null}"#,
            ),
            key("frust")
        );
    }

    #[test]
    fn no_facade_or_no_manifest_answers_the_default() {
        assert_eq!(
            resolve_from_manifest("none", &format!("{PACKAGE}[dependencies]\nlog = \"0.4\"\n"),),
            key("frust")
        );
        assert_eq!(
            resolve_from_metadata(
                "none-md",
                PACKAGE,
                r#"{"name":"log","rename":null,"kind":null,"target":null}"#,
            ),
            key("frust")
        );
        let empty = temp_dir("no-manifest");
        assert_eq!(
            FacadeKey::resolve_with(&FakeProcessRunner::new(), &empty),
            key("frust")
        );
        let _ = fs::remove_dir_all(&empty);
        assert_eq!(
            resolve_from_manifest("garbage", "this is { not ]= valid toml"),
            key("frust")
        );
    }

    /// A dev-dependency cannot carry a feature the app selects, so it never
    /// competes with the normal dependency's key.
    #[test]
    fn a_dev_dependency_is_not_the_facade_key() {
        assert_eq!(
            resolve_from_metadata(
                "dev-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":"frust","kind":null,"target":null},
                   {"name":"frust-ui","rename":null,"kind":"dev","target":null}"#,
            ),
            key("frust")
        );
        assert_eq!(
            resolve_from_manifest(
                "dev",
                &format!(
                    "{PACKAGE}[dependencies]\nfrust = {{ package = \"frust-ui\", version = \"0.6\" }}\n\n\
                     [dev-dependencies]\nfrust-ui = \"0.6\"\n"
                ),
            ),
            key("frust")
        );
    }

    #[test]
    fn two_different_keys_are_ambiguous() {
        let err = resolve_from_manifest(
            "ambiguous",
            &format!(
                "{PACKAGE}[dependencies]\nfrust = {{ package = \"frust-ui\", version = \"0.6\" }}\n\n\
                 [target.'cfg(unix)'.dependencies]\nfrust-ui = \"0.6\"\n"
            ),
        )
        .unwrap_err();
        assert_eq!(
            err,
            FacadeKeyError::Ambiguous {
                keys: vec!["frust".into(), "frust-ui".into()]
            }
        );
        let message = err.to_string();
        assert!(message.contains("`frust`"), "{message}");
        assert!(message.contains("`frust-ui`"), "{message}");
        assert!(matches!(
            resolve_from_metadata(
                "ambiguous-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":"frust","kind":null,"target":null},
                   {"name":"frust-ui","rename":"ui","kind":null,"target":"cfg(unix)"}"#,
            ),
            Err(FacadeKeyError::Ambiguous { keys }) if keys == ["frust", "ui"]
        ));
    }

    /// A key no cargo feature can spell is refused, from either path,
    /// rather than resolved to a `<key>/<feature>` cargo would reject.
    #[test]
    fn a_key_outside_the_dependency_charset_is_invalid() {
        let err = resolve_from_manifest(
            "invalid-space",
            &format!(
                "{PACKAGE}[dependencies]\n\"fr ust\" = {{ package = \"frust-ui\", version = \"0.6\" }}\n"
            ),
        )
        .unwrap_err();
        assert_eq!(
            err,
            FacadeKeyError::InvalidKey {
                key: "fr ust".into()
            }
        );
        let message = err.to_string();
        assert!(message.contains("\"fr ust\""), "{message}");
        assert!(message.contains("[A-Za-z0-9_-]+"), "{message}");
        assert_eq!(
            resolve_from_manifest(
                "invalid-quote",
                &format!(
                    "{PACKAGE}[dependencies]\n'fr\"ust' = {{ package = \"frust-ui\", version = \"0.6\" }}\n"
                ),
            ),
            Err(FacadeKeyError::InvalidKey {
                key: "fr\"ust".into()
            })
        );
        assert_eq!(
            resolve_from_metadata(
                "invalid-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":"fr.ust","kind":null,"target":null}"#,
            ),
            Err(FacadeKeyError::InvalidKey {
                key: "fr.ust".into()
            })
        );
        // The charset admits every key a manifest can spell bare.
        assert_eq!(
            resolve_from_metadata(
                "valid-md",
                PACKAGE,
                r#"{"name":"frust-ui","rename":"Ui_2-x","kind":null,"target":null}"#,
            ),
            key("Ui_2-x")
        );
    }

    #[test]
    fn the_source_names_the_path_that_answered() {
        let empty = temp_dir("source-none");
        assert_eq!(
            FacadeKey::resolve_with_source(&FakeProcessRunner::new(), &empty),
            (key("frust"), KeySource::NoManifest)
        );
        let _ = fs::remove_dir_all(&empty);

        let project = project_with(
            "source",
            &format!("{PACKAGE}[dependencies]\nui.workspace = true\n"),
        );
        assert_eq!(
            FacadeKey::resolve_with_source(&FakeProcessRunner::new(), &project),
            (key("frust"), KeySource::ManifestFallback)
        );
        let runner = FakeProcessRunner::new().with(
            metadata_key(&project),
            Output {
                success: true,
                stdout: metadata(
                    &project,
                    r#"{"name":"frust-ui","rename":"ui","kind":null,"target":null}"#,
                ),
                stderr: String::new(),
            },
        );
        assert_eq!(
            FacadeKey::resolve_with_source(&runner, &project),
            (key("ui"), KeySource::Cargo)
        );
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn map_rewrites_only_the_canonical_prefix() {
        let frust_ui = FacadeKey("frust-ui".into());
        assert_eq!(frust_ui.map("frust/devtools"), "frust-ui/devtools");
        assert_eq!(frust_ui.map("frust/perf-trace"), "frust-ui/perf-trace");
        assert_eq!(frust_ui.map("lean"), "lean");
        assert_eq!(frust_ui.map("frust-material/x"), "frust-material/x");
        assert_eq!(FacadeKey::default().map("frust/devtools"), "frust/devtools");
        assert_eq!(FacadeKey::default().as_str(), "frust");
        assert_eq!(frust_ui.to_string(), "frust-ui");
    }

    #[test]
    fn the_hint_names_both_dependency_forms() {
        let hint = FacadeKey::hint();
        assert!(!hint.contains('\n'), "{hint}");
        assert!(hint.contains("frust/<feature>"), "{hint}");
        assert!(hint.contains("frust-ui/<feature>"), "{hint}");
        assert!(hint.contains(r#"package = "frust-ui""#), "{hint}");
    }

    /// End to end against real cargo: a path-only workspace whose member
    /// inherits a renamed facade dependency resolves to the rename, which
    /// only cargo's view of the manifest carries. Ignored with the e2e
    /// family: it spawns real cargo (`--no-deps`, so nothing is fetched) and
    /// writes a workspace under the temp dir; run it with `--ignored`.
    #[test]
    #[ignore = "spawns real cargo against a temp workspace; run with --ignored (e2e family)"]
    fn real_cargo_reads_a_workspace_inherited_rename() {
        let root = temp_dir("real-cargo");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"app\", \"frust-ui\"]\nresolver = \"2\"\n\n\
             [workspace.dependencies]\nui = { package = \"frust-ui\", path = \"frust-ui\" }\n\
             frust = { package = \"frust-ui\", path = \"frust-ui\" }\n",
        );
        write(
            "frust-ui/Cargo.toml",
            "[package]\nname = \"frust-ui\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [features]\ndevtools = []\n",
        );
        write("frust-ui/src/lib.rs", "");
        write(
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nui.workspace = true\n",
        );
        write("app/src/lib.rs", "");
        assert_eq!(FacadeKey::resolve(&root.join("app")), key("ui"));

        write(
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nfrust.workspace = true\n",
        );
        assert_eq!(FacadeKey::resolve(&root.join("app")), key("frust"));
        let _ = fs::remove_dir_all(&root);
    }
}
