//! [`add_plugin`]: apply a registry plugin's contributions to a generated
//! project. Every edit is idempotent and format-preserving; a file that fails
//! to parse (or lacks its insertion anchor) is never rewritten.

use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

use super::registry::{find_plugin, known_plugins};
use super::{
    AddItem, AddOutcome, AddReport, Contribution, NativeKind, NativeModule, PluginAddError,
    PluginSpec,
};
use crate::build_dirs::BuildLayout;
use crate::host_path;
use crate::packages::{CargoLocator, LockfilePolicy, PackageLocator};
use crate::platform_wiring::{self, SyncError};
use crate::process::RealProcessRunner;

/// Project-relative paths of the files a contribution edits.
const CARGO_TOML_REL: &str = "Cargo.toml";
const MANIFEST_REL: &str = "android/app/src/main/AndroidManifest.xml";
const PLIST_REL: &str = "ios/Runner/Info.plist";
const SETTINGS_GRADLE_REL: &str = "android/settings.gradle.kts";
const APP_BUILD_GRADLE_REL: &str = "android/app/build.gradle.kts";
const PBXPROJ_REL: &str = "ios/Runner.xcodeproj/project.pbxproj";
const LIB_RS_REL: &str = "src/lib.rs";

/// The marker comments the Android app template ships for plugin-contributed
/// Gradle wiring (`crates/frust-drive/templates/app/android.tmpl/settings.gradle.kts.tmpl` and
/// `app/build.gradle.kts.tmpl`). An insert goes on the line *after* the
/// marker; a project missing either marker is
/// [`PluginAddError::MalformedProjectFile`] and is never rewritten — guessing
/// at `include(":app")` or a bare `dependencies {` would be a second,
/// unpinned convention.
const SETTINGS_ANCHOR: &str = "// frust:plugin-includes";
const APP_DEPS_ANCHOR: &str = "// frust:plugin-dependencies";

/// The `settings.gradle.kts` helper every machine-local module directory is
/// resolved through: it loads the gitignored `local.properties` beside the
/// settings file and answers a key's directory, failing — with the command
/// that writes it — when the file or the key is missing. The app template
/// carries this text verbatim (a scaffold test pins that), and
/// [`apply_gradle_module`] inserts it above the plugin anchor of a project
/// generated before the template had it.
pub(crate) const SETTINGS_LOCAL_DIR_HELPER: &str = "\
val frustLocalPropertiesFile = rootDir.resolve(\"local.properties\")
val frustLocalProperties = java.util.Properties().apply {
    if (frustLocalPropertiesFile.isFile) frustLocalPropertiesFile.inputStream().use { load(it) }
}

fun frustLocalDir(key: String): java.io.File {
    val dir = frustLocalProperties.getProperty(key)
        ?: throw org.gradle.api.GradleException(
            \"`$key` is not set in $frustLocalPropertiesFile. Run `frust run` or `frust build` \" +
                \"for Android in the project directory: it writes the machine-local directory \" +
                \"of every Frust module there.\"
        )
    return frustLocalPropertiesFile.parentFile.resolve(dir)
}
";

/// What marks [`SETTINGS_LOCAL_DIR_HELPER`] as already defined.
const SETTINGS_HELPER_MARKER: &str = "fun frustLocalDir(";

/// Apply plugin `id` (with the requested optional `features`) to the generated
/// project at `project_root`, returning a per-edit [`AddReport`]. Idempotent:
/// a second call with the same arguments reports every item
/// [`AddOutcome::AlreadyPresent`] and writes nothing.
///
/// Plugin packages are located by `cargo metadata` ([`crate::packages`]),
/// allowed to add the new dependency to an existing `Cargo.lock` — the
/// manifest has just gained it, so a `--locked` resolve could only fail.
pub fn add_plugin(
    project_root: &Path,
    id: &str,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    let locator =
        CargoLocator::new(&RealProcessRunner).with_lockfile_policy(LockfilePolicy::AllowUpdate);
    add_plugin_with(&locator, project_root, id, features)
}

/// [`add_plugin`] with the package locator injected — the seam a test stubs,
/// since a fixture project's dependencies are not real crates cargo could
/// resolve.
///
/// The project's `frust` dependency decides the Cargo line: a `path` makes
/// this **path mode** (`<plugin> = { path = "<frust>/../../plugins/<dir>" }`,
/// relative to the same base as the `frust` path), a `version` without a
/// `path` **registry mode** (`<plugin> = "<that version>"`).
///
/// The tracked platform files never name where the plugin's Gradle module
/// or Swift package lives: `settings.gradle.kts` reads it from a
/// `local.properties` key and the Xcode project references the package by
/// name, through an `ios/<Package>` symlink ([`NativeModule`]). This call
/// writes that key and link — through [`crate::platform_wiring`], to the
/// directory under the package `locator` finds — for every module whose key
/// or link is not there yet and whose directory exists; `frust run`/`frust
/// build` refresh them from then on. In path mode, when the locator cannot
/// answer (a checkout not on disk yet, no network to resolve the rest of the
/// graph), the package is taken to be where the Cargo line points instead;
/// a module directory that does not exist is left for that later refresh.
///
/// Cargo can only locate a package the manifest depends on, so every
/// `Cargo.toml` edit is applied and written **before** the platform-file
/// contributions run, and the package is located before any platform file
/// is edited — a registry-mode project cargo cannot answer for is refused
/// with nothing but its `Cargo.toml` changed. The report still lists items
/// in registry order; a module whose tracked edits were already present but
/// whose key or link was just written reports [`AddOutcome::Applied`]. A run
/// that finds every edit, key and link already in place never asks the
/// locator.
pub fn add_plugin_with(
    locator: &dyn PackageLocator,
    project_root: &Path,
    id: &str,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    let spec = find_plugin(id).ok_or_else(|| PluginAddError::UnknownPlugin(id.to_string()))?;

    // Validate every requested feature up front, before mutating anything.
    for feat in features {
        if !spec.optional_features.iter().any(|f| f.id == *feat) {
            return Err(PluginAddError::UnknownFeature {
                plugin: id.to_string(),
                feature: (*feat).to_string(),
            });
        }
    }

    // Read + parse Cargo.toml once: its `frust` dependency decides how every
    // plugin dependency is written, and it is never rewritten if it can't
    // parse.
    let cargo_path = project_root.join(CARGO_TOML_REL);
    let cargo_src = fs::read_to_string(&cargo_path)
        .map_err(|_| PluginAddError::MissingProjectFile(CARGO_TOML_REL.to_string()))?;
    let mut doc = cargo_src
        .parse::<DocumentMut>()
        .map_err(|e| PluginAddError::UnparseableCargoToml(e.to_string()))?;

    let frust = FrustDep::read(&doc).ok_or(PluginAddError::NoFrustDependency)?;
    let resolution = Resolution::new(locator, project_root, &spec, &frust);

    // Base contributions, then the selected features' (in registry order for
    // a stable report regardless of the caller's feature ordering).
    let feature_contribs = spec
        .optional_features
        .iter()
        .filter(|f| features.contains(&f.id))
        .flat_map(|f| f.contributions.iter());
    let contributions: Vec<&Contribution> = spec.base.iter().chain(feature_contribs).collect();

    // A registry entry naming a platform path outside its own package cannot
    // be located; refuse it before anything is written.
    for contribution in &contributions {
        if let Some(rel_path) = native_rel_path(contribution) {
            package_location(&spec, rel_path)?;
        }
    }

    // Facade plugins need a sibling checkout present; check before any edit
    // so a missing sibling leaves the tree untouched.
    if let Some(sibling) = spec.requires_sibling {
        let expected = resolution.sibling(sibling)?;
        if !expected.exists() {
            return Err(PluginAddError::SiblingCheckoutMissing {
                sibling: sibling.to_string(),
                expected,
            });
        }
    }

    // The `Cargo.toml` edits first, in memory, then one write — only if a
    // dep or feature was actually inserted, so an all-`AlreadyPresent` run
    // touches no file (byte-identical tree).
    let mut outcomes: Vec<Option<AddOutcome>> = vec![None; contributions.len()];
    let mut cargo_changed = false;
    for (slot, contribution) in outcomes.iter_mut().zip(&contributions) {
        if is_cargo_contribution(contribution) {
            *slot = Some(apply_contribution(
                contribution,
                &resolution,
                &mut doc,
                &mut cargo_changed,
            )?);
        }
    }
    if cargo_changed {
        write_file(&cargo_path, CARGO_TOML_REL, &doc.to_string())?;
    }

    // The native modules whose machine-local key or link is still missing,
    // and — now that cargo can see the dependency, before any platform file
    // is edited — the package they live in. Every module of one plugin
    // ships in the same package (its base Cargo dependency's).
    let unwired: Vec<(usize, NativeModule)> = contributions
        .iter()
        .enumerate()
        .filter_map(|(at, contribution)| Some((at, native_module(&spec, contribution)?.ok()?)))
        .filter(|(_, module)| {
            platform_present(project_root, module)
                && !platform_wiring::module_slot_present(project_root, module)
        })
        .collect();
    let package_dir = match unwired.first() {
        Some((_, module)) => Some(resolution.package_dir(module.package)?),
        None => None,
    };

    // Everything else.
    let mut items = Vec::with_capacity(contributions.len());
    for (contribution, applied) in contributions.iter().zip(outcomes) {
        let outcome = match applied {
            Some(outcome) => outcome,
            None => apply_contribution(contribution, &resolution, &mut doc, &mut cargo_changed)?,
        };
        items.push(AddItem {
            description: contribution.describe(),
            outcome,
        });
    }

    // The machine-local keys and links, for the modules whose directory is
    // actually there.
    if let Some(package_dir) = package_dir {
        let mut slots = Vec::new();
        let mut targets = Vec::new();
        for (at, module) in unwired {
            let dir = package_dir.join(module.dir_in_package);
            if dir.is_dir() {
                let dir = host_path::canonicalize_simplified(&dir).unwrap_or(dir);
                slots.push(at);
                targets.push((module, dir));
            }
        }
        let wirings =
            platform_wiring::wire_modules(project_root, &targets).map_err(wiring_error)?;
        for (at, wiring) in slots.into_iter().zip(wirings) {
            if wiring.changed() {
                items[at].outcome = AddOutcome::Applied;
            }
        }
    }

    Ok(AddReport {
        plugin_id: id.to_string(),
        items,
    })
}

/// Whether the project has the platform directory `module` belongs to — a
/// module for an absent platform has nothing to wire.
fn platform_present(project_root: &Path, module: &NativeModule) -> bool {
    let platform = match module.kind {
        NativeKind::GradleModule { .. } => "android",
        NativeKind::SwiftPackage { .. } => "ios",
    };
    project_root.join(platform).is_dir()
}

/// A failed machine-local write, as the [`PluginAddError::Io`] a caller of
/// [`add_plugin`] already handles.
fn wiring_error(err: SyncError) -> PluginAddError {
    let path = match &err {
        SyncError::Io { path, .. } => path.display().to_string(),
        _ => platform_wiring::LOCAL_PROPERTIES.to_string(),
    };
    PluginAddError::Io {
        path,
        message: err.to_string(),
    }
}

/// Whether `contribution` edits `Cargo.toml` (applied before every other
/// contribution — see [`add_plugin_with`]).
fn is_cargo_contribution(contribution: &Contribution) -> bool {
    matches!(
        contribution,
        Contribution::CargoDep { .. } | Contribution::CargoFeature { .. }
    )
}

/// The repo-root-relative platform path a Gradle module or Swift package
/// contribution names, `None` for every other contribution.
fn native_rel_path(contribution: &Contribution) -> Option<&'static str> {
    match contribution {
        Contribution::GradleModule { rel_path, .. }
        | Contribution::SwiftPackageRef { rel_path, .. } => Some(rel_path),
        _ => None,
    }
}

/// The [`NativeModule`] a Gradle module or Swift package contribution of
/// `spec` wires, `None` for every other contribution; an error when its
/// registry path lies outside the plugin's own package.
pub(crate) fn native_module(
    spec: &PluginSpec,
    contribution: &Contribution,
) -> Option<Result<NativeModule, PluginAddError>> {
    let (rel_path, kind) = match *contribution {
        Contribution::GradleModule {
            gradle_name,
            rel_path,
        } => (rel_path, NativeKind::GradleModule { gradle_name }),
        Contribution::SwiftPackageRef {
            package_name,
            rel_path,
        } => (rel_path, NativeKind::SwiftPackage { package_name }),
        _ => return None,
    };
    Some(
        package_location(spec, rel_path).map(|(package, dir_in_package)| NativeModule {
            package,
            dir_in_package,
            kind,
        }),
    )
}

fn apply_contribution(
    contribution: &Contribution,
    resolution: &Resolution,
    doc: &mut DocumentMut,
    cargo_changed: &mut bool,
) -> Result<AddOutcome, PluginAddError> {
    let project_root = resolution.project_root;
    match contribution {
        Contribution::CargoDep { name } => apply_cargo_dep(
            doc,
            cargo_changed,
            name,
            resolution.spec.crate_dir,
            resolution.frust,
        ),
        Contribution::ManifestPermission { permission } => {
            apply_manifest_permission(project_root, permission)
        }
        Contribution::PlistEntry {
            key,
            value,
            comment,
        } => apply_plist_entry(project_root, key, value, comment),
        Contribution::GradleModule { gradle_name, .. } => {
            apply_gradle_module(project_root, gradle_name)
        }
        Contribution::SwiftPackageRef { package_name, .. } => {
            apply_swift_package_ref(project_root, package_name)
        }
        Contribution::IosFramework { name } => apply_ios_framework(project_root, name),
        Contribution::AppCrateMacro {
            invocation,
            cfg,
            comment,
        } => apply_app_crate_macro(project_root, invocation, *cfg, comment),
        Contribution::CargoFeature { name, feature } => {
            apply_cargo_feature(doc, cargo_changed, name, feature)
        }
        Contribution::ScaffoldFile {
            rel_path, contents, ..
        } => apply_scaffold_file(project_root, rel_path, contents),
        // The desktop lane applies to the *assembled bundle* at
        // `frust build macos|windows|linux` time, never to a project file
        // `add_plugin` could write (see each variant's own doc comment on
        // `Contribution` for the build-time rationale). `add_plugin` only
        // records the contribution; there is nothing to apply or to find
        // already present here, so the outcome is unconditional and never
        // touches `project_root`.
        Contribution::MacosPlistEntry { .. }
        | Contribution::MacosEntitlement { .. }
        | Contribution::LinuxDesktopEntry { .. } => Ok(AddOutcome::AppliedAtBuild),
    }
}

// ---------------------------------------------------------------------------
// Desktop-lane detection — `desktop_contributions`, the API a later
// bundle-assembly task consumes at `frust build macos|windows|linux` time.
// No project file is read or written on this path beyond `Cargo.toml`
// itself; nothing here ever calls `apply_contribution`.
// ---------------------------------------------------------------------------

/// A desktop-lane contribution ([`Contribution::MacosPlistEntry`] /
/// [`Contribution::MacosEntitlement`] / [`Contribution::LinuxDesktopEntry`])
/// owed by a plugin the project depends on — one [`desktop_contributions`]
/// result row.
#[derive(Debug, Clone, Copy)]
pub struct DesktopContribution {
    /// The owning plugin's [`PluginSpec::id`].
    pub plugin_id: &'static str,
    /// The contribution itself — always one of the three desktop variants.
    pub contribution: &'static Contribution,
}

/// Desktop-lane contributions owed by every plugin the project at
/// `project_root` depends on — the detection API `frust build macos|linux`
/// consumes to assemble a bundle (`add_plugin` itself never applies these;
/// see [`Contribution::MacosPlistEntry`]'s doc comment).
///
/// "Installed" means the plugin's own base [`Contribution::CargoDep`] name
/// appears as a `[dependencies]` key in the project's `Cargo.toml` — the
/// same source of truth [`add_plugin`] itself writes to, read here rather
/// than duplicated — **or** some other dependency entry renames itself via
/// an inline `package = "<name>"` key ([`plugin_is_installed`]). A dependency
/// carrying `optional = true` is never counted as installed, even when its
/// name matches: an optional dep the app never enabled by feature must not
/// merge the plugin's desktop-lane keys/entitlements into a bundle whose
/// binary may not actually contain it.
///
/// **v1 scope:** only a plugin's **base** contributions are scanned. A
/// desktop contribution gated behind an optional [`super::FeatureSpec`]
/// bundle is invisible to this function even when the caller has requested that
/// feature — `add_plugin`'s `features` argument is never persisted
/// anywhere, so there is no durable record of *which* features a project
/// selected for this function to read back. Widening this is future work,
/// gated on feature selection becoming durable. Likewise, only the top-level
/// `[dependencies]` table is consulted — a dep declared under a
/// target-scoped table (`[target.'cfg(...)'.dependencies]`) is invisible to
/// this function, matching the same base-only conservatism.
///
/// Order is deterministic: registry order ([`known_plugins`]), then each
/// plugin's own declaration order within `base`.
pub fn desktop_contributions(
    project_root: &Path,
) -> Result<Vec<DesktopContribution>, PluginAddError> {
    let cargo_path = project_root.join(CARGO_TOML_REL);
    let cargo_src = fs::read_to_string(&cargo_path)
        .map_err(|_| PluginAddError::MissingProjectFile(CARGO_TOML_REL.to_string()))?;
    let doc = cargo_src
        .parse::<DocumentMut>()
        .map_err(|e| PluginAddError::UnparseableCargoToml(e.to_string()))?;
    Ok(desktop_contributions_for(&known_plugins(), &doc))
}

/// [`desktop_contributions`]'s pure inner logic, parameterized over the
/// registry list and the parsed `Cargo.toml` doc so a test can inject
/// synthetic [`PluginSpec`]s — the real registry carries zero desktop-lane
/// rows today (this is the seam, not a backfill), which would otherwise
/// make the detection logic itself untestable.
fn desktop_contributions_for(specs: &[PluginSpec], doc: &DocumentMut) -> Vec<DesktopContribution> {
    specs
        .iter()
        .filter(|spec| plugin_is_installed(spec, doc))
        .flat_map(|spec| {
            spec.base.iter().filter_map(move |contribution| {
                is_desktop_contribution(contribution).then_some(DesktopContribution {
                    plugin_id: spec.id,
                    contribution,
                })
            })
        })
        .collect()
}

/// Whether `contribution` is one of the three desktop-lane variants.
fn is_desktop_contribution(contribution: &Contribution) -> bool {
    matches!(
        contribution,
        Contribution::MacosPlistEntry { .. }
            | Contribution::MacosEntitlement { .. }
            | Contribution::LinuxDesktopEntry { .. }
    )
}

/// A plugin is "installed" iff its own base [`Contribution::CargoDep`] name
/// either appears as a `[dependencies]` key in `doc`, or names a **renamed**
/// dependency entry via that entry's inline `package = "<name>"` key (e.g.
/// `cam = { package = "frust-camera", path = "..." }`) — but never a dep
/// entry carrying `optional = true`, whose base contributions must not merge
/// into a bundle the plugin may not actually be linked into (see
/// [`desktop_contributions`]'s doc comment). Also the rule
/// [`crate::platform_wiring::sync`] picks the plugins whose native modules it
/// wires by.
pub(crate) fn plugin_is_installed(spec: &PluginSpec, doc: &DocumentMut) -> bool {
    let Some(name) = spec.base.iter().find_map(|c| match c {
        Contribution::CargoDep { name } => Some(*name),
        _ => None,
    }) else {
        return false;
    };
    let Some(deps) = doc
        .get("dependencies")
        .and_then(|item| item.as_table_like())
    else {
        return false;
    };
    deps.iter().any(|(key, value)| {
        if dep_is_optional(value) {
            return false;
        }
        key == name || dep_renamed_package(value) == Some(name)
    })
}

/// Whether a `[dependencies]` entry carries `optional = true`.
fn dep_is_optional(value: &Item) -> bool {
    value
        .as_table_like()
        .and_then(|t| t.get("optional"))
        .and_then(Item::as_bool)
        .unwrap_or(false)
}

/// A `[dependencies]` entry's `package = "..."` value, if it renames itself.
fn dep_renamed_package(value: &Item) -> Option<&str> {
    value.as_table_like()?.get("package")?.as_str()
}

/// Append a macro invocation to the app crate's `src/lib.rs`
/// ([`Contribution::AppCrateMacro`]).
///
/// Appending — rather than inserting at an anchor — is deliberate. The
/// scaffold's `lib.rs` ends with `frust::app!(AppName);`, and its own doc
/// comment tells the app author never to edit that invocation; everything these
/// contributions plant belongs *after* it, at file scope, where item order does
/// not matter. So there is no anchor to maintain and nothing to half-edit.
///
/// Idempotence keys on the invocation text itself, so an author who moved the
/// line elsewhere in the file — or wrote it by hand before running Add Plugin,
/// which is exactly what a device gate run against a hand-edited project did —
/// is not handed a duplicate.
fn apply_app_crate_macro(
    project_root: &Path,
    invocation: &str,
    cfg: Option<&str>,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(LIB_RS_REL);
    let src = read_required(&path, LIB_RS_REL)?;
    if src.contains(invocation) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let mut block = String::new();
    if !src.ends_with('\n') {
        block.push('\n');
    }
    block.push('\n');
    block.push_str(&format!("// {comment}\n"));
    if let Some(predicate) = cfg {
        block.push_str(&format!("#[cfg({predicate})]\n"));
    }
    block.push_str(invocation);
    block.push('\n');
    write_file(&path, LIB_RS_REL, &format!("{src}{block}"))?;
    Ok(AddOutcome::Applied)
}

/// How the project depends on the framework facade, read from its
/// `[dependencies].frust` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FrustDep {
    /// A `path` is present: a local checkout (path mode). `path` is the value
    /// as written, relative to the project root or absolute.
    Path { path: String, package: String },
    /// A `version` without a `path`, or the `frust = "<version>"` shorthand:
    /// the crates.io release (registry mode). `version` is the requirement as
    /// written.
    Registry { version: String, package: String },
}

impl FrustDep {
    /// The mode `doc`'s `frust` entry declares, or `None` when there is no
    /// such entry or it carries neither a `path` nor a `version`. `package`
    /// defaults to the dependency key, as cargo's own does — though every
    /// scaffolded project names `frust-ui` explicitly in both modes.
    fn read(doc: &DocumentMut) -> Option<Self> {
        let item = doc.get("dependencies")?.as_table_like()?.get("frust")?;
        if let Some(version) = item.as_str() {
            return Some(Self::Registry {
                version: version.to_string(),
                package: "frust".to_string(),
            });
        }
        let table = item.as_table_like()?;
        let package = table
            .get("package")
            .and_then(Item::as_str)
            .unwrap_or("frust")
            .to_string();
        if let Some(path) = table.get("path").and_then(Item::as_str) {
            return Some(Self::Path {
                path: path.to_string(),
                package,
            });
        }
        let version = table.get("version").and_then(Item::as_str)?;
        Some(Self::Registry {
            version: version.to_string(),
            package,
        })
    }

    /// The facade's package name.
    fn package(&self) -> &str {
        match self {
            Self::Path { package, .. } | Self::Registry { package, .. } => package,
        }
    }
}

/// The path-mode Cargo dependency path for a plugin: `<frust>/../../plugins/<crate_dir>`
/// — the facade crate sits two levels below the checkout root, every plugin
/// crate under its `plugins/`. Relative to the same base as `frust_path`.
fn plugin_dep_path(frust_path: &str, crate_dir: &str) -> String {
    format!("{frust_path}/../../plugins/{crate_dir}")
}

/// `path` with `.` dropped and each `..` folding away the component before
/// it — lexically, since the directory need not exist.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The package a plugin's platform files ship in: the crate its base
/// [`Contribution::CargoDep`] adds.
fn plugin_package(spec: &PluginSpec) -> Option<&'static str> {
    spec.base
        .iter()
        .find_map(|contribution| match contribution {
            Contribution::CargoDep { name } => Some(*name),
            _ => None,
        })
}

/// Where a registry platform path (repo-root-relative,
/// `plugins/<crate_dir>/…`) lives as a package location: the plugin's own
/// package ([`plugin_package`]) and the path inside it, e.g.
/// `platform/android` — the part that stays the same wherever cargo finds
/// the package.
fn package_location<'r>(
    spec: &PluginSpec,
    rel_path: &'r str,
) -> Result<(&'static str, &'r str), PluginAddError> {
    let outside = || PluginAddError::NativePathOutsidePackage {
        plugin: spec.id.to_string(),
        rel_path: rel_path.to_string(),
        crate_dir: spec.crate_dir.to_string(),
    };
    let package = plugin_package(spec).ok_or_else(outside)?;
    let inside = rel_path
        .strip_prefix("plugins/")
        .and_then(|rest| rest.strip_prefix(spec.crate_dir))
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .ok_or_else(outside)?;
    Ok((package, inside))
}

/// Where one `add_plugin` run resolves locations from: the project, the
/// plugin, how the project depends on frust, and the locator that answers
/// where a package lives.
struct Resolution<'a> {
    locator: &'a dyn PackageLocator,
    project_root: &'a Path,
    spec: &'a PluginSpec,
    frust: &'a FrustDep,
}

impl<'a> Resolution<'a> {
    fn new(
        locator: &'a dyn PackageLocator,
        project_root: &'a Path,
        spec: &'a PluginSpec,
        frust: &'a FrustDep,
    ) -> Self {
        Self {
            locator,
            project_root,
            spec,
            frust,
        }
    }

    /// The directory of the plugin's `package`: where the locator finds it;
    /// or — path mode only, when the locator cannot answer — the very path
    /// the project's Cargo line names for the plugin ([`plugin_dep_path`],
    /// made absolute against the project root and folded lexically), which
    /// is where cargo will look once the checkout is in place. Registry mode
    /// has no such fallback: the crate exists only where cargo unpacks it.
    fn package_dir(&self, package: &str) -> Result<PathBuf, PluginAddError> {
        match self.locator.locate(self.project_root, package) {
            Ok(dir) => Ok(dir),
            Err(source) => match self.frust {
                FrustDep::Path { path, .. } => {
                    let dep = plugin_dep_path(path, self.spec.crate_dir);
                    let dep = host_path::simplify(Path::new(&dep));
                    Ok(normalize_lexically(&self.project_root.join(dep)))
                }
                FrustDep::Registry { .. } => Err(PluginAddError::PackageNotLocated {
                    package: package.to_string(),
                    source,
                }),
            },
        }
    }

    /// Where a `requires_sibling` checkout is expected: `sibling`, relative
    /// to the checkout root two levels above the located facade package. In
    /// path mode a locator failure falls back to the `frust` path itself
    /// (simplified first: a project scaffolded on Windows before paths were
    /// written portably may carry a verbatim `\\?\C:\...` prefix, which
    /// `..` cannot climb).
    fn sibling(&self, sibling: &str) -> Result<PathBuf, PluginAddError> {
        let package = self.frust.package();
        let facade = match self.locator.locate(self.project_root, package) {
            Ok(dir) => dir,
            Err(source) => match self.frust {
                FrustDep::Path { path, .. } => {
                    let path = host_path::simplify(Path::new(path));
                    if path.is_absolute() {
                        path
                    } else {
                        self.project_root.join(path)
                    }
                }
                FrustDep::Registry { .. } => {
                    return Err(PluginAddError::PackageNotLocated {
                        package: package.to_string(),
                        source,
                    });
                }
            },
        };
        Ok(facade.join("..").join("..").join(sibling))
    }
}

fn apply_cargo_dep(
    doc: &mut DocumentMut,
    changed: &mut bool,
    name: &str,
    crate_dir: &str,
    frust: &FrustDep,
) -> Result<AddOutcome, PluginAddError> {
    let deps = doc
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
        // Unreachable in practice — `FrustDep::read` already found
        // `[dependencies].frust` — but avoid an `unwrap` at an FFI-free core.
        .ok_or(PluginAddError::NoFrustDependency)?;

    if deps.contains_key(name) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let value = match frust {
        FrustDep::Path { path, .. } => {
            let mut inline = InlineTable::new();
            inline.insert("path", Value::from(plugin_dep_path(path, crate_dir)));
            Value::InlineTable(inline)
        }
        // The plugin crates release in lockstep with the facade, so the
        // facade's own requirement is the plugin's.
        FrustDep::Registry { version, .. } => Value::from(version.as_str()),
    };
    deps.insert(name, Item::Value(value));
    *changed = true;
    Ok(AddOutcome::Applied)
}

/// Enable a cargo feature on an already-contributed dependency's inline table
/// ([`Contribution::CargoFeature`]): get-or-create the dep's `features` array
/// and push `feature` if it isn't already there.
///
/// The dependency must already exist (a prior [`Contribution::CargoDep`]
/// applied by the same or an earlier `add_plugin` call, per
/// [`apply_cargo_dep`]'s shape) — [`PluginAddError::NoSuchCargoDep`] if it
/// doesn't, never a silently minted dep. A registry-mode dependency written
/// as the `name = "<version>"` shorthand is promoted in place to
/// `{ version = "<version>", features = [...] }`, the only form that can
/// carry a feature.
fn apply_cargo_feature(
    doc: &mut DocumentMut,
    changed: &mut bool,
    name: &str,
    feature: &str,
) -> Result<AddOutcome, PluginAddError> {
    let no_such_dep = || PluginAddError::NoSuchCargoDep {
        name: name.to_string(),
        feature: feature.to_string(),
    };

    let deps = doc
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(no_such_dep)?;

    let dep = deps.get_mut(name).ok_or_else(no_such_dep)?;
    if let Some(version) = dep.as_str().map(str::to_string) {
        let mut inline = InlineTable::new();
        inline.insert("version", Value::from(version));
        *dep = Item::Value(Value::InlineTable(inline));
    }
    let dep = dep.as_inline_table_mut().ok_or_else(no_such_dep)?;

    let features = dep
        .entry("features")
        .or_insert_with(|| Value::Array(Array::new()))
        .as_array_mut()
        .ok_or_else(no_such_dep)?;

    if features.iter().any(|v| v.as_str() == Some(feature)) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    features.push(feature);
    *changed = true;
    Ok(AddOutcome::Applied)
}

fn apply_manifest_permission(
    project_root: &Path,
    permission: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(MANIFEST_REL);
    let src = read_required(&path, MANIFEST_REL)?;
    if src.contains(&format!("android:name=\"{permission}\"")) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let line = format!("    <uses-permission android:name=\"{permission}\" />\n");
    let out = insert_before_anchor(&src, "</manifest>", &line, MANIFEST_REL)?;
    write_file(&path, MANIFEST_REL, &out)?;
    Ok(AddOutcome::Applied)
}

fn apply_plist_entry(
    project_root: &Path,
    key: &str,
    value: &str,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(PLIST_REL);
    let src = read_required(&path, PLIST_REL)?;
    if src.contains(&format!("<key>{key}</key>")) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    // Insert before the root dict's close (the last `</dict>`, immediately
    // preceding `</plist>`), tab-indented like the surrounding entries.
    let block = format!("\t<!-- {comment} -->\n\t<key>{key}</key>\n\t<string>{value}</string>\n");
    let out = insert_before_anchor(&src, "</dict>", &block, PLIST_REL)?;
    write_file(&path, PLIST_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Create a file at `rel_path` (relative to the project root) with exact
/// `contents` if it doesn't already exist ([`Contribution::ScaffoldFile`]).
/// Parent directories are created as needed.
///
/// Idempotency guard: presence alone, never a content comparison — an
/// existing file (even one the user has since hand-edited, e.g. their own
/// locale strings) is left untouched and reports
/// [`AddOutcome::AlreadyPresent`]. Never a blind overwrite
/// (`docs/PLUGINS_CODE_STANDARDS.md`'s idempotency charter).
fn apply_scaffold_file(
    project_root: &Path,
    rel_path: &str,
    contents: &str,
) -> Result<AddOutcome, PluginAddError> {
    let safe_rel = safe_scaffold_rel_path(rel_path)?;
    let path = project_root.join(safe_rel);
    if path.exists() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PluginAddError::Io {
            path: rel_path.to_string(),
            message: e.to_string(),
        })?;
    }
    write_file(&path, rel_path, contents)?;
    Ok(AddOutcome::Applied)
}

/// Reject an absolute `rel_path` or one carrying a `..` component before it
/// is ever joined onto `project_root`. Every [`Contribution::ScaffoldFile`]
/// in the registry is a static, trusted string, but the check stays
/// defensive rather than assuming that forever (see
/// [`PluginAddError::UnsafeScaffoldPath`]).
///
/// `Path::is_absolute()` alone is not enough: on Windows, `/etc/passwd` has
/// no drive prefix so it is *not* `is_absolute()`, yet `project_root.join(...)`
/// on a rooted-without-prefix path replaces everything but the drive,
/// yielding `C:\etc\passwd` — a full escape from `project_root`. Rejecting
/// [`Component::RootDir`] (a bare root, drive-relative on Windows, absolute
/// on Unix) and [`Component::Prefix`] (`C:`, `\\server\share`) alongside
/// [`Component::ParentDir`] closes that gap on every OS; on Unix `RootDir`
/// alone already covers what `is_absolute()` used to check.
fn safe_scaffold_rel_path(rel_path: &str) -> Result<&Path, PluginAddError> {
    let path = Path::new(rel_path);
    let is_unsafe = path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if is_unsafe {
        return Err(PluginAddError::UnsafeScaffoldPath(rel_path.to_string()));
    }
    Ok(path)
}

/// Wire a plugin's `com.android.library` module into the generated project:
/// the `include(...)`/`projectDir`/build-dir-redirect trio in
/// `android/settings.gradle.kts` plus the `implementation(project(...))` line
/// in `android/app/build.gradle.kts`.
///
/// Both files are read and their replacements computed **before** either is
/// written, so a malformed second file can't leave the first half-edited.
/// Each half is independently skip-if-present, which is what makes a
/// half-applied state (someone deleted one of the two lines) complete rather
/// than duplicate: the outcome is [`AddOutcome::Applied`] if either half
/// changed, [`AddOutcome::AlreadyPresent`] only when both were already wired.
///
/// No directory is written into either file: the `projectDir` reads the
/// module's machine-local key ([`settings_include_block`]), which
/// [`add_plugin_with`] writes separately. A settings file that predates the
/// [`SETTINGS_LOCAL_DIR_HELPER`] gets the helper inserted above the anchor
/// line, so it is defined before every plugin block.
fn apply_gradle_module(
    project_root: &Path,
    gradle_name: &str,
) -> Result<AddOutcome, PluginAddError> {
    let settings_path = project_root.join(SETTINGS_GRADLE_REL);
    let settings_src = read_required(&settings_path, SETTINGS_GRADLE_REL)?;
    let build_path = project_root.join(APP_BUILD_GRADLE_REL);
    let build_src = read_required(&build_path, APP_BUILD_GRADLE_REL)?;

    let settings_out = if settings_src.contains(&format!("include(\"{gradle_name}\")")) {
        None
    } else {
        // The helper goes *above* the anchor line, so it is defined before
        // every plugin block — each later one lands right below the anchor.
        let with_helper = if settings_src.contains(SETTINGS_HELPER_MARKER) {
            settings_src
        } else {
            insert_before_anchor_line(
                &settings_src,
                SETTINGS_ANCHOR,
                &format!(
                    "// Added by `frust` Add Plugin: resolves a module directory from the\n\
                     // machine-local, gitignored `local.properties` beside this file.\n\
                     {SETTINGS_LOCAL_DIR_HELPER}\n"
                ),
                SETTINGS_GRADLE_REL,
            )?
        };
        Some(insert_after_anchor_line(
            &with_helper,
            SETTINGS_ANCHOR,
            &settings_include_block(gradle_name),
            SETTINGS_GRADLE_REL,
        )?)
    };

    let build_out = if build_src.contains(&format!("project(\"{gradle_name}\")")) {
        None
    } else {
        Some(insert_after_anchor_line(
            &build_src,
            APP_DEPS_ANCHOR,
            &format!("    implementation(project(\"{gradle_name}\"))\n"),
            APP_BUILD_GRADLE_REL,
        )?)
    };

    if settings_out.is_none() && build_out.is_none() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    if let Some(out) = settings_out {
        write_file(&settings_path, SETTINGS_GRADLE_REL, &out)?;
    }
    if let Some(out) = build_out {
        write_file(&build_path, APP_BUILD_GRADLE_REL, &out)?;
    }
    Ok(AddOutcome::Applied)
}

/// The `settings.gradle.kts` block one [`Contribution::GradleModule`] adds —
/// deliberately the same shape the scaffold emits for `:frust-embedding`:
/// the `projectDir` comes from the module's
/// [`platform_wiring::plugin_module_key`] through `frustLocalDir(...)`, and
/// the build-directory redirect is included, since a plugin module shares
/// the embedding's "two apps, one shared frust checkout" collision problem
/// exactly. The redirect literal is derived from [`BuildLayout::android_module`]
/// (not duplicated here) so it cannot drift from the
/// `<app>/build/android/<module>` root the scaffold's own
/// `settings.gradle.kts.tmpl` redirects every module under.
fn settings_include_block(gradle_name: &str) -> String {
    let build_dir = gradle_name.trim_start_matches(':');
    let key = platform_wiring::plugin_module_key(gradle_name);
    // `Path::display()` renders with the host's native separator (backslash
    // on Windows); this literal is embedded verbatim into a Kotlin-DSL
    // `rootDir.resolve(...)` string, which is always forward-slash regardless
    // of the host building the project — a raw backslash would also be an
    // invalid escape inside the generated Kotlin string literal.
    let build_redirect = format!(
        "../{}",
        host_path::to_portable_string(&BuildLayout::android_module(build_dir))
    );
    format!(
        "\n\
         // Added by `frust` Add Plugin: a plugin's Android library module,\n\
         // included from the plugin package cargo resolves for this app (its\n\
         // directory is machine-local: `{key}` in local.properties).\n\
         include(\"{gradle_name}\")\n\
         project(\"{gradle_name}\").projectDir = frustLocalDir(\"{key}\")\n\
         \n\
         gradle.lifecycle.beforeProject {{\n\
         \x20   if (path == \"{gradle_name}\") {{\n\
         \x20       layout.buildDirectory.set(rootDir.resolve(\"{build_redirect}\"))\n\
         \x20   }}\n\
         }}\n"
    )
}

// ---------------------------------------------------------------------------
// `Contribution::SwiftPackageRef` — the six-site `project.pbxproj` applier.
// ---------------------------------------------------------------------------

/// The generated iOS project's object-id scheme: a 24-character uppercase-hex
/// id, hand-allocated as this fixed prefix plus a two-hex-digit counter
/// (`crates/frust-drive/templates/app/ios.tmpl/.../project.pbxproj.tmpl` uses `…0001`–`…0033`;
/// `examples/glyph-catalog` re-allocated its embedding wiring at `…0034`–
/// `…0036`). A minted id continues the *target file's* own numbering, which is
/// what lets those two allocations coexist.
const PBX_ID_PREFIX: &str = "ABCDABCDABCDABCDABCD00";

/// Length of a pbxproj object id (24 uppercase-hex characters).
const PBX_ID_LEN: usize = 24;

/// The highest counter a `PBX_ID_PREFIX` id can carry (`NN` is two hex digits).
const PBX_ID_MAX: u32 = 0xFF;

/// The three objects one [`Contribution::SwiftPackageRef`] introduces —
/// resolved from whatever the target file already carries, minted only for the
/// pieces that are genuinely absent (so a half-wired project is *completed*
/// with its own ids rather than re-wired with a second, conflicting set).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SwiftPackageIds {
    /// The `PBXBuildFile` linking the product into the Frameworks phase.
    build_file: String,
    /// The `XCSwiftPackageProductDependency`.
    product_dependency: String,
    /// The `XCLocalSwiftPackageReference`.
    package_reference: String,
}

/// One of the six pbxproj sites a [`Contribution::SwiftPackageRef`] occupies.
/// All six are required: a project missing any one of them either fails to
/// build or fails to open, which is why this contribution is all-or-nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PbxSite {
    /// The `PBXBuildFile` object (`{ productRef = … }`).
    BuildFileObject,
    /// Its entry in the Frameworks build phase's `files` list.
    FrameworksEntry,
    /// The target's `packageProductDependencies` entry.
    ProductDependencyEntry,
    /// The project's `packageReferences` entry.
    PackageReferenceEntry,
    /// The `XCLocalSwiftPackageReference` object (carrying `relativePath`).
    PackageReferenceObject,
    /// The `XCSwiftPackageProductDependency` object (carrying `productName`).
    ProductDependencyObject,
}

/// Application order: the two list entries that point at objects come after
/// nothing in particular — each site is an independent single-line/blocked
/// splice against a distinct anchor — but a fixed order keeps the emitted
/// diff stable run to run.
const PBX_SITES: [PbxSite; 6] = [
    PbxSite::BuildFileObject,
    PbxSite::FrameworksEntry,
    PbxSite::ProductDependencyEntry,
    PbxSite::PackageReferenceEntry,
    PbxSite::PackageReferenceObject,
    PbxSite::ProductDependencyObject,
];

impl PbxSite {
    /// The `/* Begin … section */` this site lives in, plus the `key = ( … );`
    /// list within it (`None` for a site that *is* a top-level object).
    fn location(self) -> (&'static str, Option<&'static str>) {
        match self {
            PbxSite::BuildFileObject => ("PBXBuildFile", None),
            PbxSite::FrameworksEntry => ("PBXFrameworksBuildPhase", Some("files")),
            PbxSite::ProductDependencyEntry => {
                ("PBXNativeTarget", Some("packageProductDependencies"))
            }
            PbxSite::PackageReferenceEntry => ("PBXProject", Some("packageReferences")),
            PbxSite::PackageReferenceObject => ("XCLocalSwiftPackageReference", None),
            PbxSite::ProductDependencyObject => ("XCSwiftPackageProductDependency", None),
        }
    }

    /// The id-free text identifying this site's line — what a *presence* check
    /// matches, so a half-wired project is recognised no matter which ids its
    /// surviving sites use. Every pbxproj line of interest has the shape
    /// `<id> <needle>`, which is what makes [`Self::verify_needle`] a simple
    /// id-qualified extension of this one.
    fn needle(self, package_name: &str) -> String {
        match self {
            PbxSite::BuildFileObject => {
                format!("/* {package_name} in Frameworks */ = {{isa = PBXBuildFile;")
            }
            PbxSite::FrameworksEntry => format!("/* {package_name} in Frameworks */,"),
            PbxSite::ProductDependencyEntry => format!("/* {package_name} */,"),
            PbxSite::PackageReferenceEntry => {
                format!("/* XCLocalSwiftPackageReference \"{package_name}\" */,")
            }
            PbxSite::PackageReferenceObject => {
                format!("/* XCLocalSwiftPackageReference \"{package_name}\" */ = {{")
            }
            PbxSite::ProductDependencyObject => format!("/* {package_name} */ = {{"),
        }
    }

    /// The object this site names, and therefore whose id it must carry.
    fn id(self, ids: &SwiftPackageIds) -> &str {
        match self {
            PbxSite::BuildFileObject | PbxSite::FrameworksEntry => &ids.build_file,
            PbxSite::ProductDependencyEntry | PbxSite::ProductDependencyObject => {
                &ids.product_dependency
            }
            PbxSite::PackageReferenceEntry | PbxSite::PackageReferenceObject => {
                &ids.package_reference
            }
        }
    }

    /// The id-qualified text the post-edit re-scan requires — this is what
    /// catches a surviving site that points at a *different* id (drift the
    /// presence check alone would accept, and Xcode would reject).
    fn verify_needle(self, package_name: &str, ids: &SwiftPackageIds) -> String {
        format!("{} {}", self.id(ids), self.needle(package_name))
    }

    /// A stable label for [`PluginAddError::PbxWiringNotVerified`].
    fn label(self) -> &'static str {
        match self {
            PbxSite::BuildFileObject => "PBXBuildFile",
            PbxSite::FrameworksEntry => "PBXFrameworksBuildPhase.files",
            PbxSite::ProductDependencyEntry => "PBXNativeTarget.packageProductDependencies",
            PbxSite::PackageReferenceEntry => "PBXProject.packageReferences",
            PbxSite::PackageReferenceObject => "XCLocalSwiftPackageReference",
            PbxSite::ProductDependencyObject => "XCSwiftPackageProductDependency",
        }
    }
}

/// Add a plugin's local Swift package to the generated app's Xcode project as
/// a second package reference beside the embedding's `FrustEmbedding`.
///
/// The iOS counterpart of [`apply_gradle_module`], with the same
/// one-contribution/one-[`AddItem`] ledger and the same
/// complete-a-half-applied-state rule — but six sites instead of two, and a
/// stricter write discipline: the whole edited file is built in memory,
/// re-scanned site by site, and only then written **once**. A partially
/// edited `project.pbxproj` is not a smaller problem than none at all — Xcode
/// refuses to open it.
///
/// Anchors are the template's own section markers and list terminators; a
/// missing one is [`PluginAddError::MalformedProjectFile`], never a rewrite
/// (this module's standing rule). The reference's `relativePath` is the
/// package name itself — Xcode resolves it against `<project>/ios/`, through
/// the machine-local `ios/<package_name>` symlink [`add_plugin_with`]
/// writes — so no directory is written into the tracked file.
fn apply_swift_package_ref(
    project_root: &Path,
    package_name: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(PBXPROJ_REL);
    let src = read_required(&path, PBXPROJ_REL)?;

    let mut missing = Vec::new();
    for site in PBX_SITES {
        if !pbx_site_body(&src, site)?.contains(&site.needle(package_name)) {
            missing.push(site);
        }
    }
    if missing.is_empty() {
        return Ok(AddOutcome::AlreadyPresent);
    }

    let ids = resolve_or_mint_ids(&src, package_name)?;
    let mut out = src;
    for site in missing {
        out = insert_pbx_site(&out, site, package_name, &ids)?;
    }

    // All-or-nothing: every site must be present *and* carry this run's id
    // before a single byte is written.
    for site in PBX_SITES {
        if !pbx_site_body(&out, site)?.contains(&site.verify_needle(package_name, &ids)) {
            return Err(PluginAddError::PbxWiringNotVerified {
                file: PBXPROJ_REL.to_string(),
                package: package_name.to_string(),
                site: site.label().to_string(),
            });
        }
    }

    write_file(&path, PBXPROJ_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Append `"-framework", <name>,` to every `OTHER_LDFLAGS` list of the
/// `XCBuildConfiguration` section that does not already link `name` — one
/// list per build configuration. Lists are edited back to front so the
/// ranges scanned from the original stay valid, the whole edit is built in
/// memory, re-scanned, and written once; a project with no `OTHER_LDFLAGS`
/// list at all is [`PluginAddError::MalformedProjectFile`].
fn apply_ios_framework(project_root: &Path, name: &str) -> Result<AddOutcome, PluginAddError> {
    const SECTION: &str = "XCBuildConfiguration";
    const KEY: &str = "OTHER_LDFLAGS";
    let path = project_root.join(PBXPROJ_REL);
    let src = read_required(&path, PBXPROJ_REL)?;

    let lists = pbx_list_ranges(&src, SECTION, KEY)?;
    if lists.is_empty() {
        return Err(PluginAddError::MalformedProjectFile(
            PBXPROJ_REL.to_string(),
        ));
    }

    let mut out = src.clone();
    let mut changed = false;
    for range in lists.into_iter().rev() {
        let body = &src[range.clone()];
        if ldflags_link_framework(body, name) {
            continue;
        }
        // Indent like the list's own entries (five tabs in the scaffold),
        // falling back to the scaffold's indentation for an empty list.
        let indent = body
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map_or("\t\t\t\t\t", |line| {
                &line[..line.len() - line.trim_start().len()]
            });
        out.insert_str(
            range.end,
            &format!("{indent}\"-framework\",\n{indent}{name},\n"),
        );
        changed = true;
    }
    if !changed {
        return Ok(AddOutcome::AlreadyPresent);
    }

    // All-or-nothing: every list must link the framework before a byte is
    // written.
    for range in pbx_list_ranges(&out, SECTION, KEY)? {
        if !ldflags_link_framework(&out[range], name) {
            return Err(PluginAddError::PbxWiringNotVerified {
                file: PBXPROJ_REL.to_string(),
                package: name.to_string(),
                site: KEY.to_string(),
            });
        }
    }

    write_file(&path, PBXPROJ_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Whether an `OTHER_LDFLAGS` entry region already links `name`: a
/// `"-framework",` entry immediately followed by `name,` (or its quoted
/// form), matched on trimmed lines so a pbxproj Xcode has since reformatted
/// still reads as present.
fn ldflags_link_framework(body: &str, name: &str) -> bool {
    let entries: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let bare = format!("{name},");
    let quoted = format!("\"{name}\",");
    entries
        .windows(2)
        .any(|pair| pair[0] == "\"-framework\"," && (pair[1] == bare || pair[1] == quoted))
}

/// Splice one site into `src`, returning the whole edited file. A list entry
/// is appended after the last existing entry (immediately before the list's
/// own `);`); an object is appended to its section, before the `/* End … */`
/// marker.
fn insert_pbx_site(
    src: &str,
    site: PbxSite,
    package_name: &str,
    ids: &SwiftPackageIds,
) -> Result<String, PluginAddError> {
    let block = match site {
        PbxSite::FrameworksEntry
        | PbxSite::ProductDependencyEntry
        | PbxSite::PackageReferenceEntry => {
            let range = pbx_site_range(src, site)?;
            let entry = format!("\t\t\t\t{}\n", site.verify_needle(package_name, ids));
            let mut out = String::with_capacity(src.len() + entry.len());
            out.push_str(&src[..range.end]);
            out.push_str(&entry);
            out.push_str(&src[range.end..]);
            return Ok(out);
        }
        PbxSite::BuildFileObject => format!(
            "\t\t{} /* {package_name} in Frameworks */ = {{isa = PBXBuildFile; \
             productRef = {} /* {package_name} */; }};\n",
            ids.build_file, ids.product_dependency
        ),
        PbxSite::PackageReferenceObject => format!(
            "\t\t{} /* XCLocalSwiftPackageReference \"{package_name}\" */ = {{\n\
             \t\t\tisa = XCLocalSwiftPackageReference;\n\
             \t\t\trelativePath = \"{package_name}\";\n\
             \t\t}};\n",
            ids.package_reference
        ),
        PbxSite::ProductDependencyObject => format!(
            "\t\t{} /* {package_name} */ = {{\n\
             \t\t\tisa = XCSwiftPackageProductDependency;\n\
             \t\t\tproductName = {package_name};\n\
             \t\t}};\n",
            ids.product_dependency
        ),
    };
    insert_before_anchor(
        src,
        &format!("/* End {} section */", site.location().0),
        &block,
        PBXPROJ_REL,
    )
}

/// The byte range a site's presence is searched in: its section's body, or
/// (for a list site) just the entries between `key = (` and its `);`.
fn pbx_site_range(src: &str, site: PbxSite) -> Result<Range<usize>, PluginAddError> {
    let (section, list) = site.location();
    match list {
        Some(key) => pbx_list_range(src, section, key),
        None => pbx_section_range(src, section),
    }
}

fn pbx_site_body(src: &str, site: PbxSite) -> Result<&str, PluginAddError> {
    Ok(&src[pbx_site_range(src, site)?])
}

/// The body of one `/* Begin <name> section */ … /* End <name> section */`
/// block, so a section-scoped match can't pick up text from another section
/// (`files = (` alone appears in four of them).
fn pbx_section_range(src: &str, name: &str) -> Result<Range<usize>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let begin = format!("/* Begin {name} section */");
    let end = format!("/* End {name} section */");
    let start = src.find(&begin).ok_or_else(malformed)? + begin.len();
    let stop = src[start..].find(&end).ok_or_else(malformed)? + start;
    Ok(start..stop)
}

/// The entry region of the first `key = ( … );` list inside `section` — from
/// the byte after the opening line to the byte before the closing `);` line
/// (see [`pbx_list_ranges`] for the matching rule); a section with no such
/// list is [`PluginAddError::MalformedProjectFile`].
fn pbx_list_range(src: &str, section: &str, key: &str) -> Result<Range<usize>, PluginAddError> {
    pbx_list_ranges(src, section, key)?
        .into_iter()
        .next()
        .ok_or_else(|| PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string()))
}

/// The entry regions of every `key = ( … );` list inside `section`, in file
/// order — one per build configuration for a key such as `OTHER_LDFLAGS`.
/// Matched on trimmed line content rather than exact indentation, so a
/// project whose pbxproj Xcode has since rewritten still wires up; an
/// unterminated list is [`PluginAddError::MalformedProjectFile`].
fn pbx_list_ranges(
    src: &str,
    section: &str,
    key: &str,
) -> Result<Vec<Range<usize>>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let section = pbx_section_range(src, section)?;
    let open = format!("{key} = (");
    let body = &src[section.clone()];

    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    let mut start: Option<usize> = None;
    for line in body.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let trimmed = line.trim();
        match start {
            None if trimmed == open => start = Some(cursor),
            Some(open_end) if trimmed == ");" => {
                ranges.push(section.start + open_end..section.start + line_start);
                start = None;
            }
            _ => {}
        }
    }
    if start.is_some() {
        return Err(malformed());
    }
    Ok(ranges)
}

/// This package's three object ids: reused wherever the target file already
/// mentions them (the half-wired case), minted otherwise.
///
/// Minting continues the target file's own `…00NN` numbering — scan for the
/// highest, allocate upward — which is what lets the template's `…0033` and a
/// consumer's hand-renumbered `…0036` (glyph-catalog) coexist without a
/// registry of allocated ids. A minted id that somehow already occurs is
/// [`PluginAddError::PbxIdCollision`], never an overwrite.
fn resolve_or_mint_ids(src: &str, package_name: &str) -> Result<SwiftPackageIds, PluginAddError> {
    let existing_build_file = pbx_id_preceding(src, &format!("/* {package_name} in Frameworks */"));
    // The product dependency's own comment is the bare package name — which
    // the build-file line also carries (`productRef = <id> /* Name */;`), and
    // whose preceding token is the very id wanted here.
    let existing_product = pbx_id_preceding(src, &format!("/* {package_name} */"));
    let existing_package_ref = pbx_id_preceding(
        src,
        &format!("/* XCLocalSwiftPackageReference \"{package_name}\" */"),
    );

    let mut next = pbx_next_id_counter(src);
    let mut take = |existing: Option<String>| -> Result<String, PluginAddError> {
        match existing {
            Some(id) => Ok(id),
            None => {
                let id = pbx_mint_id(src, next)?;
                next += 1;
                Ok(id)
            }
        }
    };

    Ok(SwiftPackageIds {
        build_file: take(existing_build_file)?,
        product_dependency: take(existing_product)?,
        package_reference: take(existing_package_ref)?,
    })
}

/// One past the highest `PBX_ID_PREFIX` counter in `src` (0 if the file uses
/// none of them at all).
fn pbx_next_id_counter(src: &str) -> u32 {
    let mut highest: Option<u32> = None;
    for (at, _) in src.match_indices(PBX_ID_PREFIX) {
        let tail = at + PBX_ID_PREFIX.len();
        let Some(counter) = src.get(tail..tail + 2) else {
            continue;
        };
        if !counter.chars().all(is_pbx_hex_digit) {
            continue;
        }
        if let Ok(value) = u32::from_str_radix(counter, 16) {
            highest = Some(highest.map_or(value, |h: u32| h.max(value)));
        }
    }
    highest.map_or(0, |h| h + 1)
}

/// Format `counter` as an object id, refusing a value the scheme can't express
/// or an id the file already uses.
fn pbx_mint_id(src: &str, counter: u32) -> Result<String, PluginAddError> {
    if counter > PBX_ID_MAX {
        return Err(PluginAddError::PbxIdSpaceExhausted {
            file: PBXPROJ_REL.to_string(),
        });
    }
    let id = format!("{PBX_ID_PREFIX}{counter:02X}");
    if src.contains(&id) {
        return Err(PluginAddError::PbxIdCollision {
            file: PBXPROJ_REL.to_string(),
            id,
        });
    }
    Ok(id)
}

/// The object id immediately preceding the first occurrence of `needle` —
/// every pbxproj line naming an object writes `<id> /* comment */`, whether it
/// is a definition (`= {`), a list entry (`,`) or an inline reference
/// (`productRef = <id> /* … */;`).
fn pbx_id_preceding(src: &str, needle: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(rel) = src[from..].find(needle) {
        let at = from + rel;
        let head = src[..at].trim_end();
        let candidate = head
            .len()
            .checked_sub(PBX_ID_LEN)
            .and_then(|start| head.get(start..));
        if let Some(id) = candidate.filter(|id| is_pbx_object_id(id)) {
            return Some(id.to_string());
        }
        from = at + needle.len();
    }
    None
}

fn is_pbx_hex_digit(c: char) -> bool {
    c.is_ascii_digit() || ('A'..='F').contains(&c)
}

/// A 24-character uppercase-hex pbxproj object id — the style every id in the
/// generated project uses.
fn is_pbx_object_id(token: &str) -> bool {
    token.len() == PBX_ID_LEN && token.chars().all(is_pbx_hex_digit)
}

/// Insert `insertion` immediately before the last occurrence of `anchor`,
/// preserving all surrounding bytes. Errors [`PluginAddError::MalformedProjectFile`]
/// if the anchor is absent (the file is never rewritten in that case).
fn insert_before_anchor(
    src: &str,
    anchor: &str,
    insertion: &str,
    rel: &str,
) -> Result<String, PluginAddError> {
    let idx = src
        .rfind(anchor)
        .ok_or_else(|| PluginAddError::MalformedProjectFile(rel.to_string()))?;
    let mut out = String::with_capacity(src.len() + insertion.len());
    out.push_str(&src[..idx]);
    out.push_str(insertion);
    out.push_str(&src[idx..]);
    Ok(out)
}

/// Insert `insertion` immediately after the line containing the first
/// occurrence of `anchor`, preserving all surrounding bytes — the
/// marker-comment counterpart to [`insert_before_anchor`], for anchors whose
/// own text says "lines go below". Errors
/// [`PluginAddError::MalformedProjectFile`] if the anchor is absent (the file
/// is never rewritten in that case).
fn insert_after_anchor_line(
    src: &str,
    anchor: &str,
    insertion: &str,
    rel: &str,
) -> Result<String, PluginAddError> {
    let idx = src
        .find(anchor)
        .ok_or_else(|| PluginAddError::MalformedProjectFile(rel.to_string()))?;
    // Past the anchor's own line: the byte after its newline, or end-of-file
    // for a trailing anchor with no final newline.
    let split = match src[idx..].find('\n') {
        Some(nl) => idx + nl + 1,
        None => src.len(),
    };
    let mut out = String::with_capacity(src.len() + insertion.len() + 1);
    out.push_str(&src[..split]);
    if split == src.len() && !src.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(insertion);
    out.push_str(&src[split..]);
    Ok(out)
}

/// Insert `insertion` at the start of the line containing the first
/// occurrence of `anchor` — ahead of the whole anchor line, so the anchor
/// stays the marker later inserts go below. Errors
/// [`PluginAddError::MalformedProjectFile`] if the anchor is absent.
fn insert_before_anchor_line(
    src: &str,
    anchor: &str,
    insertion: &str,
    rel: &str,
) -> Result<String, PluginAddError> {
    let idx = src
        .find(anchor)
        .ok_or_else(|| PluginAddError::MalformedProjectFile(rel.to_string()))?;
    let line_start = src[..idx].rfind('\n').map_or(0, |nl| nl + 1);
    let mut out = String::with_capacity(src.len() + insertion.len());
    out.push_str(&src[..line_start]);
    out.push_str(insertion);
    out.push_str(&src[line_start..]);
    Ok(out)
}

fn read_required(path: &Path, rel: &str) -> Result<String, PluginAddError> {
    fs::read_to_string(path).map_err(|_| PluginAddError::MissingProjectFile(rel.to_string()))
}

fn write_file(path: &Path, rel: &str, contents: &str) -> Result<(), PluginAddError> {
    fs::write(path, contents).map_err(|e| PluginAddError::Io {
        path: rel.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_build::local_properties;
    use crate::packages::{PackagesError, StubLocator};
    use crate::scaffold::{self, TemplateContext};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-plugin-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A scaffold context whose `frust_path` deliberately does NOT exist on
    /// disk — so a derived `../clean-signals-rs` sibling path is absent too,
    /// and no accidental real checkout is picked up.
    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: "0.1.0".into(),
            frust: crate::scaffold::FrustDependency::Path("/nonexistent/frust/checkout".into()),
            deeplink_scheme: None,
            deeplink_host: None,
        }
    }

    /// Render a fresh scaffold into a tempdir and return its root.
    fn scaffold_project(tag: &str) -> PathBuf {
        let dest = unique_temp_dir(tag);
        scaffold::generate(&dest, &test_context(), None, false, None).unwrap();
        dest
    }

    /// What cargo would answer for a path-mode fixture project: the facade
    /// package at the project's `frust` path, and every registry plugin's
    /// package at `<checkout>/plugins/<crate_dir>`, `<checkout>` being two
    /// levels above that path — folded lexically, since the fixture checkout
    /// never exists on disk. A project with no readable path-mode `frust`
    /// dependency gets a locator that finds nothing.
    fn checkout_locator(root: &Path) -> StubLocator {
        let frust = fs::read_to_string(root.join(CARGO_TOML_REL))
            .ok()
            .and_then(|text| text.parse::<DocumentMut>().ok())
            .and_then(|doc| FrustDep::read(&doc));
        let Some(FrustDep::Path { path, package }) = frust else {
            return StubLocator::new();
        };
        let facade = normalize_lexically(&root.join(host_path::simplify(Path::new(&path))));
        let checkout = normalize_lexically(&facade.join("../.."));
        known_plugins()
            .iter()
            .fold(
                StubLocator::new().with(package, facade),
                |stub, spec| match plugin_package(spec) {
                    Some(name) => stub.with(name, checkout.join("plugins").join(spec.crate_dir)),
                    None => stub,
                },
            )
    }

    /// [`super::add_plugin_with`] through [`checkout_locator`] — what every
    /// fixture test below means by applying a plugin, with no cargo run.
    fn add_plugin(
        project_root: &Path,
        id: &str,
        features: &[&str],
    ) -> Result<AddReport, PluginAddError> {
        add_plugin_with(&checkout_locator(project_root), project_root, id, features)
    }

    /// A path-mode `FrustDep` on `path`, as a scaffolded project declares it.
    fn path_dep(path: &str) -> FrustDep {
        FrustDep::Path {
            path: path.to_string(),
            package: "frust-ui".to_string(),
        }
    }

    /// The checkout root [`checkout_locator`] places plugins under for
    /// `test_context()`'s facade path (`<root>/frust/checkout` → `<root>`),
    /// host-real absolute like the facade path itself.
    #[cfg(windows)]
    const FIXTURE_CHECKOUT: &str = "C:/nonexistent";
    #[cfg(not(windows))]
    const FIXTURE_CHECKOUT: &str = "/nonexistent";

    /// What [`snapshot_tree`] records for a symlink, ahead of its target.
    const SYMLINK_MARK: &[u8] = b"symlink -> ";

    /// Snapshot every file under `root` as `relpath -> bytes` for a
    /// byte-identity comparison; a symlink (an `ios/` package link) is
    /// recorded as [`SYMLINK_MARK`] plus its target, never followed.
    fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.file_type().is_symlink() {
                    let mut bytes = SYMLINK_MARK.to_vec();
                    bytes.extend(fs::read_link(&path).unwrap().to_string_lossy().bytes());
                    out.insert(rel, bytes);
                } else if meta.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// The project-relative files a `GradleModule` contribution edits, for
    /// tests that assert one was (or was not) touched.
    const PROGUARD_REL: &str = "android/app/proguard-rules.pro";

    #[test]
    fn add_secure_storage_with_biometric_applies_every_edit() {
        let root = scaffold_project("ss-biometric");

        // `test_context()`'s default `frust_path` (`/nonexistent/frust/checkout`)
        // is a forward-slash-rooted literal with no drive letter — not
        // `Path::is_absolute()` on Windows, so `frust_path_from_project_subdir`
        // would treat it as relative and prepend an extra `../` climb. A real
        // project's `frust` path dep on Windows always carries a drive letter,
        // so swap in a host-real absolute stand-in (still guaranteed absent on
        // disk) to keep this fixture meaningful on every host — the same
        // post-scaffold Cargo.toml rewrite
        // `gradle_module_projectdir_resolves_from_the_android_subdirectory`
        // uses for its own relative-path fixture, below. Forward slashes even
        // on Windows: a raw `\` inside a TOML basic string is an invalid
        // escape (this crate's own `host_path` module doc explains why every
        // path this pipeline writes into a `Cargo.toml`/`.properties`/pbxproj
        // value is portable-slash, never a native backslash).
        #[cfg(windows)]
        const FRUST_PATH: &str = "C:/nonexistent/frust/checkout";
        #[cfg(not(windows))]
        const FRUST_PATH: &str = "/nonexistent/frust/checkout";
        #[cfg(windows)]
        {
            let cargo_path = root.join(CARGO_TOML_REL);
            let cargo_src = fs::read_to_string(&cargo_path).unwrap();
            let rewritten = cargo_src.replace(
                "path = \"/nonexistent/frust/checkout\"",
                &format!("path = \"{FRUST_PATH}\""),
            );
            assert_ne!(
                rewritten, cargo_src,
                "expected to rewrite the frust path dep"
            );
            fs::write(&cargo_path, rewritten).unwrap();
        }

        let manifest_before = fs::read(root.join(MANIFEST_REL)).unwrap();
        let proguard_before = fs::read(root.join(PROGUARD_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert_eq!(report.plugin_id, "secure-storage");
        // dep + plist + gradle module = 3 items, all applied. The Android
        // permission, Kotlin helper and keep rule ride inside the module.
        assert_eq!(report.items.len(), 3);
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied)
        );
        assert_eq!(report.counts(), (3, 0));

        // Cargo.toml: the dep landed with a path derived from the frust dep.
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("frust-secure-storage"), "{cargo}");
        assert!(
            cargo.contains(&format!("{FRUST_PATH}/../../plugins/secure-storage")),
            "{cargo}"
        );

        // The app's own manifest and proguard-rules.pro are NOT edited: the
        // permission rides the plugin module's manifest (merger) and the keep
        // rule its `consumerProguardFiles`.
        assert_eq!(fs::read(root.join(MANIFEST_REL)).unwrap(), manifest_before);
        assert_eq!(fs::read(root.join(PROGUARD_REL)).unwrap(), proguard_before);

        // Info.plist: the key + comment landed before the closing dict.
        let plist = fs::read_to_string(root.join(PLIST_REL)).unwrap();
        assert!(
            plist.contains("<key>NSFaceIDUsageDescription</key>"),
            "{plist}"
        );
        assert!(
            plist.contains("<string>Unlock your stored credentials.</string>"),
            "{plist}"
        );
        assert!(plist.trim_end().ends_with("</plist>"), "{plist}");
        // XML comment well-formedness: no illegal double hyphen in the comment.
        let comment_start = plist.find("<!-- Face ID").unwrap();
        let comment_end = plist[comment_start..].find("-->").unwrap() + comment_start;
        assert!(
            !plist[comment_start + 4..comment_end].contains("--"),
            "plist comment must not contain `--`"
        );

        // No Kotlin file is copied into the app any more.
        assert!(
            !root
                .join("android/app/src/main/kotlin/dev/frust/FrustBiometric.kt")
                .exists()
        );

        // settings.gradle.kts: include + projectDir + build-dir redirect, the
        // directory read from the module's machine-local key rather than
        // written here, and the pre-existing `:frust-embedding` wiring
        // untouched.
        let settings = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        assert!(
            settings.contains("include(\":frust-secure-storage\")"),
            "{settings}"
        );
        assert!(
            settings.contains(
                "project(\":frust-secure-storage\").projectDir = \
                 frustLocalDir(\"frust.plugin.frust-secure-storage.dir\")"
            ),
            "{settings}"
        );
        assert!(!settings.contains(FIXTURE_CHECKOUT), "{settings}");
        assert_eq!(
            settings.matches(SETTINGS_HELPER_MARKER).count(),
            1,
            "the template's helper is reused, not inserted again:\n{settings}"
        );
        assert!(
            settings.contains("rootDir.resolve(\"../build/android/frust-secure-storage\")"),
            "{settings}"
        );
        assert!(
            settings.contains("include(\":frust-embedding\")"),
            "{settings}"
        );
        assert!(settings.contains("include(\":app\")"), "{settings}");

        // app/build.gradle.kts: the module dependency landed inside the
        // dependencies block, beside the embedding's.
        let build = fs::read_to_string(root.join(APP_BUILD_GRADLE_REL)).unwrap();
        assert!(
            build.contains("implementation(project(\":frust-secure-storage\"))"),
            "{build}"
        );
        assert!(
            build.contains("implementation(project(\":frust-embedding\"))"),
            "{build}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The build-dir redirect `settings_include_block` emits must be
    /// forward-slash on every host: it is spliced verbatim into a Kotlin-DSL
    /// `rootDir.resolve("...")` string literal, where a raw backslash is an
    /// invalid escape sequence, not merely a stylistic mismatch. Pinned at
    /// the unit level (not just through the `add_plugin` integration tests
    /// above) so a future regression that reintroduces `Path::display()`
    /// fails here regardless of host.
    #[test]
    fn settings_include_block_build_redirect_is_always_forward_slash() {
        let block = settings_include_block(":frust-secure-storage");
        assert!(
            block.contains(r#"rootDir.resolve("../build/android/frust-secure-storage")"#),
            "{block}"
        );
        assert!(
            block.contains(
                r#"project(":frust-secure-storage").projectDir = frustLocalDir("frust.plugin.frust-secure-storage.dir")"#
            ),
            "{block}"
        );
        assert!(!block.contains('\\'), "{block}");
        assert!(!block.contains("file("), "no path literal at all: {block}");
    }

    /// `test_context`'s `frust_path` is deliberately nonexistent, so the
    /// module directory does not resolve on disk — and applying must still
    /// succeed. The tracked edits are text inserts naming no directory; the
    /// machine-local key is simply not written for a directory that is not
    /// there (`frust run`/`frust build` write it once it is).
    #[test]
    fn gradle_module_applies_even_though_its_resolved_path_is_absent() {
        let root = scaffold_project("gradle-module-absent-path");
        let module_dir =
            PathBuf::from(FIXTURE_CHECKOUT).join("plugins/secure-storage/platform/android");
        assert!(
            !module_dir.exists(),
            "precondition: the module dir is absent"
        );

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-secure-storage"))
            .expect("a Gradle module line item");
        assert_eq!(item.outcome, AddOutcome::Applied);
        assert!(
            !root.join(platform_wiring::LOCAL_PROPERTIES).exists(),
            "no key for an absent directory"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// A fresh project scaffolded at `<scratch>/app`, so a fixture checkout
    /// can sit beside it at `<scratch>/checkout` without touching the shared
    /// temp directory.
    fn scaffold_beside_checkout(tag: &str) -> (PathBuf, PathBuf) {
        let scratch = unique_temp_dir(tag);
        let app = scratch.join("app");
        scaffold::generate(&app, &test_context(), None, false, None).unwrap();
        let scratch = host_path::canonicalize_simplified(&scratch).unwrap();
        (scratch.clone(), scratch.join("app"))
    }

    /// Creates `dir_in_package` directories under `package_dir`, returning
    /// `package_dir` canonical.
    fn plant_package(package_dir: &Path, dirs_in_package: &[&str]) -> PathBuf {
        for dir in dirs_in_package {
            fs::create_dir_all(package_dir.join(dir)).unwrap();
        }
        host_path::canonicalize_simplified(package_dir).unwrap()
    }

    /// The path-mode fallback, when cargo cannot locate the plugin package:
    /// the package is where the Cargo line points — a **relative**
    /// `--frust-path` resolved against the project root, as cargo itself
    /// resolves it — and the key carries that absolute, folded directory.
    /// The tracked settings file names no path either way.
    #[test]
    fn path_mode_fallback_wires_where_the_cargo_line_points() {
        let (scratch, root) = scaffold_beside_checkout("gradle-module-relative-frust-path");
        const REL_FRUST: &str = "../checkout/crates/frust";
        let module = plant_package(
            &scratch.join("checkout/plugins/secure-storage"),
            &["platform/android"],
        )
        .join("platform/android");

        let cargo_path = root.join(CARGO_TOML_REL);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        let rewritten = cargo.replace(
            "path = \"/nonexistent/frust/checkout\"",
            &format!("path = \"{REL_FRUST}\""),
        );
        assert_ne!(rewritten, cargo, "expected to rewrite the frust path dep");
        fs::write(&cargo_path, rewritten).unwrap();

        let nothing_located = StubLocator::failing("error: failed to read `Cargo.toml`");
        let add = || {
            add_plugin_with(
                &nothing_located,
                &root,
                "secure-storage",
                &["biometric-gate"],
            )
        };
        let report = add().unwrap();
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );
        assert_eq!(nothing_located.calls(), 1);

        assert_eq!(
            local_properties::read_value(
                &root.join(platform_wiring::LOCAL_PROPERTIES),
                "frust.plugin.frust-secure-storage.dir"
            )
            .unwrap(),
            Some(host_path::to_portable_string(&module))
        );
        assert_eq!(
            tracked_files_naming(&root, &host_path::to_portable_string(&scratch)),
            Vec::<String>::new(),
            "no tracked file names the checkout"
        );

        // The Cargo dep path is the relative one, as written.
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        assert!(
            cargo.contains(&format!("{REL_FRUST}/../../plugins/secure-storage")),
            "{cargo}"
        );

        // Idempotent, and a wired project asks cargo nothing.
        let before = snapshot_tree(&root);
        let second = add().unwrap();
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(before, snapshot_tree(&root));
        assert_eq!(nothing_located.calls(), 1);

        let _ = fs::remove_dir_all(&scratch);
    }

    /// A project generated before the template defined `frustLocalDir`
    /// (its embedding included through a Gradle property) gets the helper
    /// inserted ahead of the first plugin block, once.
    #[test]
    fn a_settings_file_without_the_helper_gains_it_once() {
        let root = scaffold_project("gradle-module-no-helper");
        let settings_path = root.join(SETTINGS_GRADLE_REL);
        let current = fs::read_to_string(&settings_path).unwrap();
        let old = current.replace(SETTINGS_LOCAL_DIR_HELPER, "").replace(
            "frustLocalDir(\"frust.embedding.dir\")",
            "file(providers.gradleProperty(\"frust.embedding.dir\").get())",
        );
        assert!(!old.contains(SETTINGS_HELPER_MARKER), "{old}");
        fs::write(&settings_path, &old).unwrap();

        add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        add_plugin(&root, "camera", &[]).unwrap();
        let settings = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            settings.matches(SETTINGS_HELPER_MARKER).count(),
            1,
            "{settings}"
        );
        // Defined before the anchor, so before every plugin block — the one
        // added first and the one added after it alike.
        let helper_at = settings.find(SETTINGS_LOCAL_DIR_HELPER).expect("helper");
        let anchor_at = settings.find(SETTINGS_ANCHOR).unwrap();
        for include in [
            "include(\":frust-secure-storage\")",
            "include(\":frust-camera\")",
        ] {
            let include_at = settings.find(include).expect(include);
            assert!(
                helper_at < anchor_at && anchor_at < include_at,
                "{settings}"
            );
        }

        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // Registry mode — a project depending on the crates.io release
    // (`frust = { package = "frust-ui", version = ... }`), the form `frust
    // create` emits by default.
    // -----------------------------------------------------------------------

    /// A scaffolded project rewritten to registry mode at version `0.5.0`,
    /// under `<scratch>/app`; returns `(scratch, app)`.
    fn registry_project(tag: &str) -> (PathBuf, PathBuf) {
        let (scratch, root) = scaffold_beside_checkout(tag);
        let cargo_path = root.join(CARGO_TOML_REL);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        let rewritten = cargo.replace(
            "path = \"/nonexistent/frust/checkout\" }",
            "version = \"0.5.0\" }",
        );
        assert_ne!(rewritten, cargo, "expected to rewrite the frust dep");
        assert!(
            rewritten.contains("frust = { package = \"frust-ui\", version = \"0.5.0\" }"),
            "{rewritten}"
        );
        fs::write(&cargo_path, rewritten).unwrap();
        (scratch, root)
    }

    /// What cargo answers once a registry project depends on `iap` (or
    /// `secure-storage`): the unpacked crates under a stand-in registry
    /// cache at `<scratch>/registry`, their native directories planted.
    fn registry_locator(scratch: &Path) -> (PathBuf, StubLocator) {
        let registry = scratch.join("registry");
        let iap = plant_package(
            &registry.join("frust-iap-0.5.0"),
            &["platform/android", "platform/ios/FrustIap"],
        );
        let storage = plant_package(
            &registry.join("frust-secure-storage-0.5.0"),
            &["platform/android"],
        );
        let locator = StubLocator::new()
            .with("frust-iap", iap)
            .with("frust-secure-storage", storage);
        (
            host_path::canonicalize_simplified(&registry).unwrap(),
            locator,
        )
    }

    /// Every file under `root` whose text names `needle`, symlinks and the
    /// machine-local `local.properties` aside — the tracked files.
    fn tracked_files_naming(root: &Path, needle: &str) -> Vec<String> {
        snapshot_tree(root)
            .into_iter()
            .filter(|(rel, bytes)| {
                rel != platform_wiring::LOCAL_PROPERTIES
                    && !bytes.starts_with(SYMLINK_MARK)
                    && String::from_utf8_lossy(bytes).contains(needle)
            })
            .map(|(rel, _)| rel)
            .collect()
    }

    #[test]
    fn frust_dep_reads_every_declaration_form() {
        for (manifest, expected) in [
            (
                "[dependencies]\nfrust = { package = \"frust-ui\", path = \"../f/crates/frust\" }\n",
                Some(FrustDep::Path {
                    path: "../f/crates/frust".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                "[dependencies]\nfrust = { package = \"frust-ui\", version = \"0.5.0\" }\n",
                Some(FrustDep::Registry {
                    version: "0.5.0".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                // A path wins over a version, exactly as cargo builds it.
                "[dependencies]\nfrust = { package = \"frust-ui\", version = \"0.5\", path = \"f\" }\n",
                Some(FrustDep::Path {
                    path: "f".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                "[dependencies]\nfrust = \"0.5.0\"\n",
                Some(FrustDep::Registry {
                    version: "0.5.0".into(),
                    package: "frust".into(),
                }),
            ),
            (
                "[dependencies.frust]\npackage = \"frust-ui\"\nversion = \"0.5.0\"\n",
                Some(FrustDep::Registry {
                    version: "0.5.0".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                "[dependencies]\nfrust = { package = \"frust-ui\", git = \"https://x\" }\n",
                None,
            ),
            ("[dependencies]\nserde = \"1\"\n", None),
        ] {
            let doc = manifest.parse::<DocumentMut>().unwrap();
            assert_eq!(FrustDep::read(&doc), expected, "{manifest}");
        }
    }

    /// The registry-mode acceptance shape for `iap`: the Cargo line takes the
    /// facade's version; the tracked Gradle and Xcode files name the modules
    /// only by key and package name; and the machine-local key and link point
    /// into the plugin crate cargo unpacked — `platform/android` and
    /// `platform/ios/FrustIap` under it — so no tracked file carries the
    /// registry cache path.
    #[cfg(unix)]
    #[test]
    fn registry_mode_writes_a_version_line_and_wires_the_located_crate() {
        let (scratch, root) = registry_project("registry-iap");
        let (registry, locator) = registry_locator(&scratch);
        let unpacked = registry.join("frust-iap-0.5.0");

        let report = add_plugin_with(&locator, &root, "iap", &[]).unwrap();
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );

        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("frust-iap = \"0.5.0\""), "{cargo}");
        assert!(!cargo.contains("plugins/iap"), "{cargo}");

        let settings = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        assert!(
            settings.contains(
                "project(\":frust-iap\").projectDir = frustLocalDir(\"frust.plugin.frust-iap.dir\")"
            ),
            "{settings}"
        );
        let pbxproj = read_pbxproj(&root);
        assert!(
            pbxproj.contains("relativePath = \"FrustIap\";"),
            "{pbxproj}"
        );
        assert_pbxproj_well_formed(&pbxproj);
        assert_eq!(
            tracked_files_naming(&root, &host_path::to_portable_string(&registry)),
            Vec::<String>::new(),
            "no tracked file may name the registry cache"
        );

        assert_eq!(
            local_properties::read_value(
                &root.join(platform_wiring::LOCAL_PROPERTIES),
                "frust.plugin.frust-iap.dir"
            )
            .unwrap(),
            Some(host_path::to_portable_string(
                &unpacked.join("platform/android")
            ))
        );
        assert_eq!(
            fs::read_link(root.join("ios/FrustIap")).unwrap(),
            unpacked.join("platform/ios/FrustIap")
        );
        assert_eq!(locator.calls(), 1, "one lookup serves both platforms");

        // Idempotent, and a fully wired project asks cargo nothing.
        let before = snapshot_tree(&root);
        let second = add_plugin_with(&locator, &root, "iap", &[]).unwrap();
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(before, snapshot_tree(&root));
        assert_eq!(locator.calls(), 1);

        // A fresh clone on another machine: the tracked edits are there but
        // the machine-local wiring is not — re-adding writes just that.
        fs::remove_file(root.join(platform_wiring::LOCAL_PROPERTIES)).unwrap();
        fs::remove_file(root.join("ios/FrustIap")).unwrap();
        let third = add_plugin_with(&locator, &root, "iap", &[]).unwrap();
        assert_eq!(third.items[0].outcome, AddOutcome::AlreadyPresent);
        assert!(
            third.items[1..]
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{third:?}"
        );
        assert_eq!(before, snapshot_tree(&root));

        let _ = fs::remove_dir_all(&scratch);
    }

    /// A feature on a registry-mode dependency promotes the version
    /// shorthand to the table form rather than failing to find a table.
    #[test]
    fn registry_mode_features_promote_the_version_shorthand() {
        let (scratch, root) = registry_project("registry-feature");
        add_plugin_with(&StubLocator::new(), &root, "database", &["engine-turso"]).unwrap();
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        let doc = cargo.parse::<DocumentMut>().unwrap();
        let dep = doc["dependencies"]["frust-database"]
            .as_inline_table()
            .unwrap_or_else(|| panic!("expected an inline table: {cargo}"));
        assert_eq!(dep.get("version").and_then(Value::as_str), Some("0.5.0"));
        assert_eq!(
            dep.get("features")
                .and_then(Value::as_array)
                .map(|features| features
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()),
            Some(vec!["engine-turso"])
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Registry mode has no checkout path to fall back on: a lookup cargo
    /// cannot answer (offline, say) is a typed error carrying cargo's words,
    /// raised before any platform file is edited. The dependency line it was
    /// asked to add is already written — cargo can only look up a package
    /// the manifest names — so a rerun once cargo can answer completes the
    /// platform edits.
    #[test]
    fn registry_mode_without_a_located_package_is_a_typed_error() {
        let (scratch, root) = registry_project("registry-unlocated");
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();
        let pbxproj_before = read_pbxproj(&root);
        let offline = StubLocator::failing("error: failed to download `frust-iap v0.5.0`");

        let err = add_plugin_with(&offline, &root, "iap", &[]).unwrap_err();
        assert!(
            matches!(
                &err,
                PluginAddError::PackageNotLocated {
                    package,
                    source: PackagesError::MetadataFailed { .. },
                } if package == "frust-iap"
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("failed to download"), "{err}");
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before
        );
        assert_eq!(read_pbxproj(&root), pbxproj_before);
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("frust-iap = \"0.5.0\""), "{cargo}");

        let (_, online) = registry_locator(&scratch);
        let report = add_plugin_with(&online, &root, "iap", &[]).unwrap();
        assert_eq!(report.items[0].outcome, AddOutcome::AlreadyPresent);
        assert!(
            report.items[1..]
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Path mode asks cargo first and wires its answer — the located package
    /// — over the place the Cargo line points.
    #[test]
    fn path_mode_prefers_the_located_package_over_the_cargo_line() {
        let (scratch, root) = scaffold_beside_checkout("path-located");
        let located = plant_package(
            &scratch.join("work/frust/plugins/iap"),
            &["platform/android", "platform/ios/FrustIap"],
        );
        let locator = StubLocator::new().with("frust-iap", &located);
        add_plugin_with(&locator, &root, "iap", &[]).unwrap();
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(
            cargo.contains(
                "frust-iap = { path = \"/nonexistent/frust/checkout/../../plugins/iap\" }"
            ),
            "{cargo}"
        );
        assert_eq!(
            local_properties::read_value(
                &root.join(platform_wiring::LOCAL_PROPERTIES),
                "frust.plugin.frust-iap.dir"
            )
            .unwrap(),
            Some(host_path::to_portable_string(
                &located.join("platform/android")
            ))
        );
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(root.join("ios/FrustIap")).unwrap(),
            located.join("platform/ios/FrustIap")
        );
        assert_eq!(
            tracked_files_naming(&root, &host_path::to_portable_string(&scratch)),
            Vec::<String>::new()
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Every real registry entry's platform paths sit inside its own package,
    /// so none can trip [`PluginAddError::NativePathOutsidePackage`].
    #[test]
    fn every_registry_platform_path_is_inside_its_plugin_package() {
        for spec in known_plugins() {
            let contributions = spec
                .base
                .iter()
                .chain(spec.optional_features.iter().flat_map(|f| f.contributions));
            for contribution in contributions {
                if let Some(rel_path) = native_rel_path(contribution) {
                    let (package, inside) = package_location(&spec, rel_path)
                        .unwrap_or_else(|e| panic!("{}: {e}", spec.id));
                    assert!(inside.starts_with("platform/"), "{}: {inside}", spec.id);
                    assert!(package.starts_with("frust-"), "{}: {package}", spec.id);
                }
            }
        }
    }

    /// A platform path outside the plugin's own package is refused before
    /// any file is written.
    #[test]
    fn a_platform_path_outside_the_plugin_package_is_refused_untouched() {
        const STRAY: PluginSpec = PluginSpec {
            id: "stray",
            summary: "synthetic, test-only",
            crate_dir: "stray",
            base: &[
                Contribution::CargoDep {
                    name: "frust-stray",
                },
                Contribution::GradleModule {
                    gradle_name: ":frust-stray",
                    rel_path: "plugins/other/platform/android",
                },
            ],
            optional_features: &[],
            requires_sibling: None,
        };
        assert!(matches!(
            package_location(&STRAY, "plugins/other/platform/android"),
            Err(PluginAddError::NativePathOutsidePackage { .. })
        ));
        assert!(package_location(&STRAY, "plugins/stray/platform/android").is_ok());
        assert!(package_location(&STRAY, "plugins/stray").is_err());
        assert!(package_location(&STRAY, "plugins/strayer/platform/android").is_err());
    }

    /// A half-applied module (settings wired, app dependency removed) must
    /// complete the missing half and report `Applied`, not skip on the first
    /// guard it finds satisfied.
    #[test]
    fn half_applied_gradle_module_completes_and_reports_applied() {
        let root = scaffold_project("gradle-module-half-applied");
        add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();

        // Undo only the app-module dependency half.
        let build_path = root.join(APP_BUILD_GRADLE_REL);
        let build = fs::read_to_string(&build_path).unwrap();
        let stripped = build.replace(
            "    implementation(project(\":frust-secure-storage\"))\n",
            "",
        );
        assert_ne!(stripped, build, "expected to strip the dependency line");
        fs::write(&build_path, &stripped).unwrap();
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-secure-storage"))
            .expect("a Gradle module line item");
        assert_eq!(
            item.outcome,
            AddOutcome::Applied,
            "a half-applied module must complete and report Applied"
        );

        // The missing half was restored, and the already-wired half was not
        // duplicated or rewritten.
        let build = fs::read_to_string(&build_path).unwrap();
        assert_eq!(
            build
                .matches("implementation(project(\":frust-secure-storage\"))")
                .count(),
            1,
            "{build}"
        );
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before,
            "the already-wired settings.gradle.kts must be byte-identical"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_gradle_without_its_anchor_is_never_rewritten() {
        let root = scaffold_project("gradle-module-no-settings-anchor");
        let settings_path = root.join(SETTINGS_GRADLE_REL);
        let mangled = fs::read_to_string(&settings_path)
            .unwrap()
            .replace(SETTINGS_ANCHOR, "// (marker removed)");
        fs::write(&settings_path, &mangled).unwrap();
        let build_before = fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap();

        let err = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == SETTINGS_GRADLE_REL),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), mangled);
        // The sibling file is untouched too — both are computed before either
        // is written.
        assert_eq!(
            fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap(),
            build_before
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn app_build_gradle_without_its_anchor_is_never_rewritten() {
        let root = scaffold_project("gradle-module-no-build-anchor");
        let build_path = root.join(APP_BUILD_GRADLE_REL);
        let mangled = fs::read_to_string(&build_path)
            .unwrap()
            .replace(APP_DEPS_ANCHOR, "// (marker removed)");
        fs::write(&build_path, &mangled).unwrap();
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();

        let err = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == APP_BUILD_GRADLE_REL),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&build_path).unwrap(), mangled);
        // settings.gradle.kts was not half-written before the failure.
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn applying_twice_is_byte_identical_and_all_already_present() {
        let root = scaffold_project("idempotent");

        let first = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert!(first.items.iter().all(|i| i.outcome == AddOutcome::Applied));
        let after_first = snapshot_tree(&root);

        let second = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert_eq!(second.items.len(), 3);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "second apply must report every item AlreadyPresent"
        );
        // The Gradle module — two files behind one line item — is covered by
        // that blanket assertion; named here so a future contribution set
        // cannot silently drop it from the idempotency gate.
        assert!(
            second
                .items
                .iter()
                .any(|i| i.description.contains(":frust-secure-storage")),
            "the Gradle module must be one of the idempotency-checked items"
        );
        let after_second = snapshot_tree(&root);

        assert_eq!(
            after_first, after_second,
            "a second apply must leave a byte-identical tree (the idempotency gate)"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn storage_only_touches_nothing_but_cargo_toml() {
        let root = scaffold_project("storage-only");

        let manifest_before = fs::read(root.join(MANIFEST_REL)).unwrap();
        let plist_before = fs::read(root.join(PLIST_REL)).unwrap();
        let proguard_before = fs::read(root.join(PROGUARD_REL)).unwrap();
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();
        let build_before = fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &[]).unwrap();
        // Only the Cargo.toml dependency is applied.
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        assert!(report.items[0].description.contains("frust-secure-storage"));

        // No platform file was touched — including the two Gradle files the
        // `biometric-gate` module contribution would have edited.
        assert_eq!(fs::read(root.join(MANIFEST_REL)).unwrap(), manifest_before);
        assert_eq!(fs::read(root.join(PLIST_REL)).unwrap(), plist_before);
        assert_eq!(fs::read(root.join(PROGUARD_REL)).unwrap(), proguard_before);
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before
        );
        assert_eq!(
            fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap(),
            build_before
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_plugin_errors() {
        let root = scaffold_project("unknown-plugin");
        let err = add_plugin(&root, "not-a-plugin", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::UnknownPlugin(id) if id == "not-a-plugin"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_feature_errors() {
        let root = scaffold_project("unknown-feature");
        let err = add_plugin(&root, "secure-storage", &["not-a-feature"]).unwrap_err();
        assert!(matches!(
            err,
            PluginAddError::UnknownFeature { plugin, feature }
                if plugin == "secure-storage" && feature == "not-a-feature"
        ));
        // The failed feature validation left Cargo.toml untouched.
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(!cargo.contains("frust-secure-storage"), "{cargo}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_frust_dep_errors() {
        let dir = unique_temp_dir("no-frust-dep");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();

        let err = add_plugin(&dir, "secure-storage", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::NoFrustDependency));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_cargo_toml_errors() {
        let dir = unique_temp_dir("no-cargo");
        fs::create_dir_all(&dir).unwrap();
        let err = add_plugin(&dir, "secure-storage", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::MissingProjectFile(f) if f == "Cargo.toml"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unparseable_cargo_toml_is_never_rewritten() {
        let dir = unique_temp_dir("bad-cargo");
        fs::create_dir_all(&dir).unwrap();
        let bad = "this is [not valid toml";
        fs::write(dir.join("Cargo.toml"), bad).unwrap();
        let err = add_plugin(&dir, "secure-storage", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::UnparseableCargoToml(_)));
        // Untouched.
        assert_eq!(fs::read_to_string(dir.join("Cargo.toml")).unwrap(), bad);
        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // `Contribution::CargoFeature`
    // -----------------------------------------------------------------------

    /// Enabling a feature on a dep [`apply_cargo_dep`] just created lands a
    /// `features = [...]` array on that same inline table.
    #[test]
    fn cargo_feature_applies_onto_a_cargo_dep_created_line() {
        let root = scaffold_project("cargo-feature-apply");
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let mut changed = false;

        apply_cargo_dep(
            &mut doc,
            &mut changed,
            "frust-secure-storage",
            "secure-storage",
            &path_dep("/nonexistent/frust/checkout"),
        )
        .unwrap();
        assert!(changed);

        let outcome =
            apply_cargo_feature(&mut doc, &mut changed, "frust-secure-storage", "biometric")
                .unwrap();
        assert_eq!(outcome, AddOutcome::Applied);

        let rendered = doc.to_string();
        assert!(rendered.contains("frust-secure-storage"), "{rendered}");
        assert!(
            rendered.contains("features = [\"biometric\"]"),
            "{rendered}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Re-applying the same feature must not duplicate it in the array —
    /// the second call reports `AlreadyPresent` and leaves the document
    /// byte-identical.
    #[test]
    fn cargo_feature_is_idempotent_on_double_apply() {
        let root = scaffold_project("cargo-feature-idempotent");
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let mut changed = false;

        apply_cargo_dep(
            &mut doc,
            &mut changed,
            "frust-secure-storage",
            "secure-storage",
            &path_dep("/nonexistent/frust/checkout"),
        )
        .unwrap();

        let first =
            apply_cargo_feature(&mut doc, &mut changed, "frust-secure-storage", "biometric")
                .unwrap();
        assert_eq!(first, AddOutcome::Applied);
        let after_first = doc.to_string();

        let second =
            apply_cargo_feature(&mut doc, &mut changed, "frust-secure-storage", "biometric")
                .unwrap();
        assert_eq!(second, AddOutcome::AlreadyPresent);
        let after_second = doc.to_string();

        assert_eq!(
            after_first, after_second,
            "re-applying the same feature must not change the document"
        );
        assert_eq!(
            after_second.matches("biometric").count(),
            1,
            "the feature must appear exactly once: {after_second}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Applying onto a dependency with no existing `CargoDep` line is an
    /// error, not a silently minted dep.
    #[test]
    fn cargo_feature_errors_on_missing_dep() {
        let root = scaffold_project("cargo-feature-missing-dep");
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let mut changed = false;

        let err = apply_cargo_feature(&mut doc, &mut changed, "frust-not-a-real-dep", "biometric")
            .unwrap_err();
        assert!(
            matches!(
                &err,
                PluginAddError::NoSuchCargoDep { name, feature }
                    if name == "frust-not-a-real-dep" && feature == "biometric"
            ),
            "{err}"
        );
        assert!(!changed, "a failed apply must not mark the doc changed");

        let _ = fs::remove_dir_all(&root);
    }

    /// `apply_contribution`'s dispatch match handles `CargoFeature` — driven
    /// the same way the plugin's own `add_plugin` call would, through the
    /// real `secure-storage` registry entry rather than an ad hoc spec.
    #[test]
    fn cargo_feature_dispatches_through_apply_contribution() {
        let root = scaffold_project("cargo-feature-dispatch");
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let mut changed = false;
        let spec = find_plugin("secure-storage").unwrap();
        let frust = path_dep("/nonexistent/frust/checkout");
        let locator = StubLocator::new();
        let resolution = Resolution::new(&locator, &root, &spec, &frust);

        apply_contribution(
            &Contribution::CargoDep {
                name: "frust-secure-storage",
            },
            &resolution,
            &mut doc,
            &mut changed,
        )
        .unwrap();

        let outcome = apply_contribution(
            &Contribution::CargoFeature {
                name: "frust-secure-storage",
                feature: "biometric",
            },
            &resolution,
            &mut doc,
            &mut changed,
        )
        .unwrap();
        assert_eq!(outcome, AddOutcome::Applied);
        assert!(doc.to_string().contains("features = [\"biometric\"]"));

        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // `Contribution::ScaffoldFile`
    //
    // No registry entry carries this contribution yet (seeding
    // `locales/en/main.ftl` is a later task) — driven directly through
    // `apply_scaffold_file`/`apply_contribution`, the same not-yet-registered
    // shape the `SwiftPackageRef` section below uses.
    // -----------------------------------------------------------------------

    const SCAFFOLD_REL: &str = "locales/en/main.ftl";
    const SCAFFOLD_CONTENTS: &str = "hello = Hello, world!\n";

    #[test]
    fn scaffold_file_creates_with_exact_contents_when_absent() {
        let root = scaffold_project("scaffold-file-absent");
        assert!(!root.join(SCAFFOLD_REL).exists());

        let outcome = apply_scaffold_file(&root, SCAFFOLD_REL, SCAFFOLD_CONTENTS).unwrap();
        assert_eq!(outcome, AddOutcome::Applied);
        assert_eq!(
            fs::read_to_string(root.join(SCAFFOLD_REL)).unwrap(),
            SCAFFOLD_CONTENTS
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// An existing file is never overwritten, even if its contents differ
    /// from the registry's — the idempotency guard is presence alone (the
    /// user may have hand-edited their own locale strings).
    #[test]
    fn scaffold_file_existing_file_with_different_contents_is_untouched() {
        let root = scaffold_project("scaffold-file-present");
        let path = root.join(SCAFFOLD_REL);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "hand-edited = true\n").unwrap();

        let outcome = apply_scaffold_file(&root, SCAFFOLD_REL, SCAFFOLD_CONTENTS).unwrap();
        assert_eq!(outcome, AddOutcome::AlreadyPresent);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "hand-edited = true\n",
            "an existing file must never be overwritten"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_file_creates_nested_parent_directories() {
        let root = scaffold_project("scaffold-file-nested");
        const NESTED_REL: &str = "locales/fr/deep/nested/main.ftl";
        assert!(!root.join("locales/fr").exists());

        let outcome = apply_scaffold_file(&root, NESTED_REL, SCAFFOLD_CONTENTS).unwrap();
        assert_eq!(outcome, AddOutcome::Applied);
        assert_eq!(
            fs::read_to_string(root.join(NESTED_REL)).unwrap(),
            SCAFFOLD_CONTENTS
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_file_rejects_absolute_path() {
        let root = scaffold_project("scaffold-file-absolute");
        let err = apply_scaffold_file(&root, "/etc/passwd", SCAFFOLD_CONTENTS).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::UnsafeScaffoldPath(p) if p == "/etc/passwd"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Windows-only escape shapes `is_absolute()` alone would miss:
    /// `C:foo` (a drive [`Component::Prefix`] with no [`Component::RootDir`] —
    /// drive-relative, but still not project-relative) and `\\srv\sh\x` (a UNC
    /// [`Component::Prefix`]). Both must be rejected the same way `/etc/passwd`
    /// is on every OS (see `scaffold_file_rejects_absolute_path` above).
    #[test]
    #[cfg(windows)]
    fn scaffold_file_rejects_windows_prefixed_paths() {
        let root = scaffold_project("scaffold-file-windows-prefix");
        for rel in ["C:foo", "\\\\srv\\sh\\x"] {
            let err = apply_scaffold_file(&root, rel, SCAFFOLD_CONTENTS).unwrap_err();
            assert!(
                matches!(&err, PluginAddError::UnsafeScaffoldPath(p) if p == rel),
                "{rel}: {err}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_file_rejects_leading_parent_dir_component() {
        let root = scaffold_project("scaffold-file-parent-dir");
        let err = apply_scaffold_file(&root, "../escape.txt", SCAFFOLD_CONTENTS).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::UnsafeScaffoldPath(p) if p == "../escape.txt"),
            "{err}"
        );
        assert!(
            !root.parent().unwrap().join("escape.txt").exists(),
            "must never write outside the project root"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A buried `..` (not just a leading one) is caught too.
    #[test]
    fn scaffold_file_rejects_embedded_parent_dir_component() {
        let root = scaffold_project("scaffold-file-embedded-parent-dir");
        let err =
            apply_scaffold_file(&root, "locales/../../escape.txt", SCAFFOLD_CONTENTS).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::UnsafeScaffoldPath(_)),
            "{err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// `apply_contribution`'s dispatch match handles `ScaffoldFile` — driven
    /// through the same per-contribution function `add_plugin`'s own loop
    /// calls (the shape `cargo_feature_dispatches_through_apply_contribution`
    /// above uses), asserting a second run is a no-op — the idempotency
    /// contract `add_plugin` itself is built on.
    #[test]
    fn scaffold_file_dispatches_through_apply_contribution_and_reapply_is_idempotent() {
        let root = scaffold_project("scaffold-file-dispatch");
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let mut changed = false;
        let spec = find_plugin("secure-storage").unwrap();
        let frust = path_dep("/nonexistent/frust/checkout");
        let locator = StubLocator::new();
        let resolution = Resolution::new(&locator, &root, &spec, &frust);
        let contribution = Contribution::ScaffoldFile {
            rel_path: SCAFFOLD_REL,
            contents: SCAFFOLD_CONTENTS,
            comment: "test-only scaffold file",
        };

        let first = apply_contribution(&contribution, &resolution, &mut doc, &mut changed).unwrap();
        assert_eq!(first, AddOutcome::Applied);
        assert_eq!(
            fs::read_to_string(root.join(SCAFFOLD_REL)).unwrap(),
            SCAFFOLD_CONTENTS
        );

        let second =
            apply_contribution(&contribution, &resolution, &mut doc, &mut changed).unwrap();
        assert_eq!(
            second,
            AddOutcome::AlreadyPresent,
            "re-running must be a no-op the second time"
        );
        assert_eq!(
            fs::read_to_string(root.join(SCAFFOLD_REL)).unwrap(),
            SCAFFOLD_CONTENTS,
            "the second run must not rewrite the file"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // `Contribution::MacosPlistEntry` / `MacosEntitlement` /
    // `LinuxDesktopEntry` — the desktop lane. No registry entry carries any
    // of the three yet (zero backfill rows, by plan — this is the seam, not
    // a backfill), so these are driven directly through `apply_contribution`
    // and synthetic `PluginSpec`s, the same not-yet-registered shape the
    // `ScaffoldFile`/`SwiftPackageRef` sections above use.
    // -----------------------------------------------------------------------

    /// Every `apply_contribution` arm for the three desktop variants reports
    /// `AppliedAtBuild` unconditionally and touches no file — the contract
    /// that lets `add_plugin` record them without ever writing to a project
    /// desktop file (`macos/Info.plist`, `macos/app.entitlements`,
    /// `linux/app.desktop`), which a pre-Phase-B project may not even have.
    #[test]
    fn desktop_lane_contributions_apply_at_build_and_touch_no_file() {
        let root = scaffold_project("desktop-lane-apply-at-build");
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let mut changed = false;
        let spec = find_plugin("secure-storage").unwrap();
        let frust = path_dep("/nonexistent/frust/checkout");
        let locator = StubLocator::new();
        let resolution = Resolution::new(&locator, &root, &spec, &frust);
        let before = snapshot_tree(&root);

        for contribution in [
            Contribution::MacosPlistEntry {
                key: "NSSupportsSuddenTermination",
                value: "NO",
                comment: "test",
            },
            Contribution::MacosEntitlement {
                key: "com.apple.security.network.client",
                comment: "test",
            },
            Contribution::LinuxDesktopEntry {
                key: "Categories",
                value: "Utility;",
                comment: "test",
            },
        ] {
            let outcome =
                apply_contribution(&contribution, &resolution, &mut doc, &mut changed).unwrap();
            assert_eq!(outcome, AddOutcome::AppliedAtBuild);
        }

        assert!(
            !changed,
            "no desktop-lane contribution marks Cargo.toml changed"
        );
        assert_eq!(
            before,
            snapshot_tree(&root),
            "desktop-lane contributions must never touch a project file"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // `desktop_contributions` — the detection API a later bundle-assembly
    // task consumes. Driven through the pure inner `desktop_contributions_for`
    // with synthetic `PluginSpec`s, since the real registry has no
    // desktop-contributing row for the public wrapper to exercise.
    // -----------------------------------------------------------------------

    const DESKTOP_PLUGIN: PluginSpec = PluginSpec {
        id: "desktop-test-plugin",
        summary: "synthetic, test-only",
        crate_dir: "desktop-test-plugin",
        base: &[
            Contribution::CargoDep {
                name: "frust-desktop-test-plugin",
            },
            Contribution::MacosPlistEntry {
                key: "NSSupportsSuddenTermination",
                value: "NO",
                comment: "test",
            },
            Contribution::MacosEntitlement {
                key: "com.apple.security.network.client",
                comment: "test",
            },
            Contribution::LinuxDesktopEntry {
                key: "Categories",
                value: "Utility;",
                comment: "test",
            },
        ],
        optional_features: &[],
        requires_sibling: None,
    };

    const MOBILE_ONLY_PLUGIN: PluginSpec = PluginSpec {
        id: "mobile-only-test-plugin",
        summary: "synthetic, test-only",
        crate_dir: "mobile-only-test-plugin",
        base: &[
            Contribution::CargoDep {
                name: "frust-mobile-only-test-plugin",
            },
            Contribution::PlistEntry {
                key: "NSCameraUsageDescription",
                value: "test",
                comment: "test",
            },
            Contribution::ManifestPermission {
                permission: "android.permission.CAMERA",
            },
        ],
        optional_features: &[],
        requires_sibling: None,
    };

    const MIXED_PLUGIN: PluginSpec = PluginSpec {
        id: "mixed-test-plugin",
        summary: "synthetic, test-only",
        crate_dir: "mixed-test-plugin",
        base: &[
            Contribution::CargoDep {
                name: "frust-mixed-test-plugin",
            },
            Contribution::PlistEntry {
                key: "NSCameraUsageDescription",
                value: "test",
                comment: "test",
            },
            Contribution::ManifestPermission {
                permission: "android.permission.CAMERA",
            },
            Contribution::MacosPlistEntry {
                key: "NSSupportsSuddenTermination",
                value: "NO",
                comment: "test",
            },
            Contribution::MacosEntitlement {
                key: "com.apple.security.network.client",
                comment: "test",
            },
            Contribution::LinuxDesktopEntry {
                key: "Categories",
                value: "Utility;",
                comment: "test",
            },
        ],
        optional_features: &[],
        requires_sibling: None,
    };

    /// Parse a freshly scaffolded project's `Cargo.toml` and insert a bare
    /// `dep_name = { path = "..." }` dependency line — standing in for what
    /// `apply_cargo_dep` would have written, without pulling the whole
    /// `add_plugin` pipeline in.
    fn cargo_doc_with_dep(root: &Path, dep_name: &str) -> DocumentMut {
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let deps = doc
            .get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .unwrap();
        let mut inline = InlineTable::new();
        inline.insert(
            "path",
            Value::from("/nonexistent/frust/checkout/../../plugins/x"),
        );
        deps.insert(dep_name, Item::Value(Value::InlineTable(inline)));
        doc
    }

    /// (a) An installed plugin's three desktop variants come back in
    /// declaration order, each carrying the owning plugin's id.
    #[test]
    fn desktop_contributions_for_installed_plugin_collects_all_three_in_order() {
        let root = scaffold_project("desktop-contrib-installed");
        let doc = cargo_doc_with_dep(&root, "frust-desktop-test-plugin");

        let result = desktop_contributions_for(&[DESKTOP_PLUGIN], &doc);
        assert_eq!(result.len(), 3, "{result:?}");
        assert!(result.iter().all(|c| c.plugin_id == "desktop-test-plugin"));
        assert!(matches!(
            result[0].contribution,
            Contribution::MacosPlistEntry {
                key: "NSSupportsSuddenTermination",
                ..
            }
        ));
        assert!(matches!(
            result[1].contribution,
            Contribution::MacosEntitlement {
                key: "com.apple.security.network.client",
                ..
            }
        ));
        assert!(matches!(
            result[2].contribution,
            Contribution::LinuxDesktopEntry {
                key: "Categories",
                ..
            }
        ));

        let _ = fs::remove_dir_all(&root);
    }

    /// (b) A plugin the project does not depend on contributes nothing.
    #[test]
    fn desktop_contributions_for_not_installed_plugin_is_empty() {
        let root = scaffold_project("desktop-contrib-not-installed");
        let cargo_path = root.join(CARGO_TOML_REL);
        let doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();

        let result = desktop_contributions_for(&[DESKTOP_PLUGIN], &doc);
        assert!(result.is_empty(), "{result:?}");

        let _ = fs::remove_dir_all(&root);
    }

    /// (c) An installed plugin with no desktop variants contributes nothing.
    #[test]
    fn desktop_contributions_for_installed_plugin_with_no_desktop_variants_is_empty() {
        let root = scaffold_project("desktop-contrib-no-desktop-variants");
        let doc = cargo_doc_with_dep(&root, "frust-mobile-only-test-plugin");

        let result = desktop_contributions_for(&[MOBILE_ONLY_PLUGIN], &doc);
        assert!(result.is_empty(), "{result:?}");

        let _ = fs::remove_dir_all(&root);
    }

    /// (d) A missing or unparseable `Cargo.toml` is the same typed error
    /// `add_plugin` itself returns for the identical condition — through the
    /// public `desktop_contributions` wrapper, since these errors arise
    /// before any `PluginSpec` is even consulted.
    #[test]
    fn desktop_contributions_missing_cargo_toml_errors() {
        let dir = unique_temp_dir("desktop-contrib-missing-cargo");
        fs::create_dir_all(&dir).unwrap();
        let err = desktop_contributions(&dir).unwrap_err();
        assert!(matches!(err, PluginAddError::MissingProjectFile(f) if f == CARGO_TOML_REL));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_contributions_unparseable_cargo_toml_errors() {
        let dir = unique_temp_dir("desktop-contrib-bad-cargo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "this is [not valid toml").unwrap();
        let err = desktop_contributions(&dir).unwrap_err();
        assert!(matches!(err, PluginAddError::UnparseableCargoToml(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// (e) An installed plugin whose `base` mixes mobile and desktop
    /// contributions must leak only the desktop ones into the result.
    #[test]
    fn desktop_contributions_mobile_only_contributions_never_leak_into_the_result() {
        let root = scaffold_project("desktop-contrib-mixed-leak-check");
        let doc = cargo_doc_with_dep(&root, "frust-mixed-test-plugin");

        let result = desktop_contributions_for(&[MIXED_PLUGIN], &doc);
        assert_eq!(result.len(), 3, "{result:?}");
        assert!(
            result.iter().all(|c| matches!(
                c.contribution,
                Contribution::MacosPlistEntry { .. }
                    | Contribution::MacosEntitlement { .. }
                    | Contribution::LinuxDesktopEntry { .. }
            )),
            "{result:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Parse a freshly scaffolded project's `Cargo.toml` and insert a
    /// **renamed** dependency: `key_name = { package = "<dep_name>", path =
    /// "..." }` — the shape `cam = { package = "frust-camera", ... }` takes.
    fn cargo_doc_with_renamed_dep(root: &Path, key_name: &str, dep_name: &str) -> DocumentMut {
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let deps = doc
            .get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .unwrap();
        let mut inline = InlineTable::new();
        inline.insert("package", Value::from(dep_name));
        inline.insert(
            "path",
            Value::from("/nonexistent/frust/checkout/../../plugins/x"),
        );
        deps.insert(key_name, Item::Value(Value::InlineTable(inline)));
        doc
    }

    /// Parse a freshly scaffolded project's `Cargo.toml` and insert an
    /// `optional = true` dependency named `dep_name`.
    fn cargo_doc_with_optional_dep(root: &Path, dep_name: &str) -> DocumentMut {
        let cargo_path = root.join(CARGO_TOML_REL);
        let mut doc = fs::read_to_string(&cargo_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        let deps = doc
            .get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .unwrap();
        let mut inline = InlineTable::new();
        inline.insert(
            "path",
            Value::from("/nonexistent/frust/checkout/../../plugins/x"),
        );
        inline.insert("optional", Value::from(true));
        deps.insert(dep_name, Item::Value(Value::InlineTable(inline)));
        doc
    }

    /// (f) `plugin_is_installed` (R0-m6): a dependency renamed via an inline
    /// `package = "<name>"` key still counts as installed.
    #[test]
    fn desktop_contributions_for_renamed_dep_is_detected() {
        let root = scaffold_project("desktop-contrib-renamed-dep");
        let doc = cargo_doc_with_renamed_dep(&root, "desktop_test", "frust-desktop-test-plugin");

        let result = desktop_contributions_for(&[DESKTOP_PLUGIN], &doc);
        assert_eq!(result.len(), 3, "{result:?}");
        assert!(result.iter().all(|c| c.plugin_id == "desktop-test-plugin"));

        let _ = fs::remove_dir_all(&root);
    }

    /// (g) `plugin_is_installed` (R0-m6): an `optional = true` dependency is
    /// excluded even though its name matches — its base contributions must
    /// not merge into a bundle whose binary may not contain it.
    #[test]
    fn desktop_contributions_for_optional_dep_is_excluded() {
        let root = scaffold_project("desktop-contrib-optional-dep");
        let doc = cargo_doc_with_optional_dep(&root, "frust-desktop-test-plugin");

        let result = desktop_contributions_for(&[DESKTOP_PLUGIN], &doc);
        assert!(result.is_empty(), "{result:?}");

        let _ = fs::remove_dir_all(&root);
    }

    /// (h) `plugin_is_installed` (R0-m6): a plain, non-renamed, non-optional
    /// dependency is still detected — the pre-existing behaviour is
    /// preserved by the widened check.
    #[test]
    fn desktop_contributions_for_plain_dep_is_still_detected() {
        let root = scaffold_project("desktop-contrib-plain-dep");
        let doc = cargo_doc_with_dep(&root, "frust-desktop-test-plugin");

        let result = desktop_contributions_for(&[DESKTOP_PLUGIN], &doc);
        assert_eq!(result.len(), 3, "{result:?}");

        let _ = fs::remove_dir_all(&root);
    }

    /// The public wrapper reads the *real* registry, which carries zero
    /// desktop-contributing rows today (the seam, not a backfill) — even a
    /// freshly scaffolded project must therefore see an empty result.
    #[test]
    fn desktop_contributions_public_fn_is_empty_against_the_real_registry() {
        let root = scaffold_project("desktop-contrib-public-empty");
        let result = desktop_contributions(&root).unwrap();
        assert!(result.is_empty(), "{result:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_signals_frust_add_succeeds_without_sibling_checkout() {
        // test_context's frust_path is nonexistent, so a `../clean-signals-rs`
        // sibling derived from it is absent too — clean-signals-frust's
        // registry entry requires none (clean-signals is a crates.io
        // dependency; see `registry.rs`'s `CLEAN_SIGNALS_FRUST`), so this
        // must still succeed.
        let root = scaffold_project("clean-signals-no-sibling");
        let report = add_plugin(&root, "clean-signals-frust", &[]).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("clean-signals-frust = {"), "{cargo}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_signals_frust_add_succeeds_with_stale_sibling_present() {
        // Same as above, but with a leftover `../clean-signals-rs` directory
        // actually present on disk (a dev machine that still has the
        // pre-migration sibling checkout) — its presence or absence must
        // make no difference now that the registry entry declares no
        // `requires_sibling`.
        let root = scaffold_project("clean-signals-stale-sibling");
        let cargo_path = root.join(CARGO_TOML_REL);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        let rewritten = cargo.replace(
            "path = \"/nonexistent/frust/checkout\"",
            "path = \"vendor/crates/frust\"",
        );
        assert_ne!(rewritten, cargo, "expected to rewrite the frust path dep");
        fs::write(&cargo_path, rewritten).unwrap();
        fs::create_dir_all(root.join("vendor/crates/frust")).unwrap();
        fs::create_dir_all(root.join("clean-signals-rs")).unwrap();

        let report = add_plugin(&root, "clean-signals-frust", &[]).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        assert!(cargo.contains("clean-signals-frust"), "{cargo}");
        assert!(
            cargo.contains("vendor/crates/frust/../../plugins/clean-signals-frust"),
            "{cargo}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The frust repo root, located robustly off `CARGO_MANIFEST_DIR`
    /// (`crates/frust-drive`) rather than the process CWD — this test must
    /// pass regardless of where `cargo test` is invoked from.
    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Discover every in-repo Android consumer under `examples/` and
    /// `benchmarks/` — a direct subdirectory counts if it looks like a
    /// frust-scaffolded Android project (carries both `Cargo.toml` and an
    /// `android/` Gradle project). Discovered dynamically rather than
    /// hard-coded: a project added later is automatically covered, and one
    /// removed (as `bubblebench` was) is simply absent from the listing
    /// rather than a failure — `fs::read_dir` on a since-deleted `examples/*`
    /// entry can't produce it, and a missing `examples/`/`benchmarks/`
    /// directory itself is tolerated the same way.
    fn discover_android_consumers(repo_root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for parent in ["examples", "benchmarks"] {
            let Ok(entries) = fs::read_dir(repo_root.join(parent)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path.join(CARGO_TOML_REL).is_file()
                    && path.join(SETTINGS_GRADLE_REL).is_file()
                    && path.join(APP_BUILD_GRADLE_REL).is_file()
                {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// Copy only the files `add_plugin` reads/writes for the
    /// `secure-storage` + `biometric-gate` contribution set (`Cargo.toml`,
    /// the Android manifest, the iOS plist, and the two Gradle files) from a
    /// real consumer project into a fresh tempdir, preserving their
    /// project-relative layout.
    ///
    /// Deliberately **not** a whole-directory copy: an in-repo Android
    /// consumer carries multi-gigabyte Gradle/cargo-ndk build output
    /// (`android/app/build`, `android/app/src/main/jniLibs`) that
    /// `add_plugin` never touches and a test must never copy, and the real
    /// project must never be mutated in place.
    fn copy_consumer_files(src_root: &Path, dest_root: &Path) -> std::io::Result<()> {
        for rel in [
            CARGO_TOML_REL,
            MANIFEST_REL,
            PLIST_REL,
            SETTINGS_GRADLE_REL,
            APP_BUILD_GRADLE_REL,
        ] {
            let src = src_root.join(rel);
            let dest = dest_root.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dest)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // `Contribution::SwiftPackageRef`
    //
    // Driven through `apply_swift_package_ref` directly rather than
    // `add_plugin`: the applier's whole contract — six sites, minted ids,
    // all-or-nothing write — is per contribution, not per plugin.
    // -----------------------------------------------------------------------

    /// A plugin package name standing in for the camera plugin's.
    const PKG: &str = "FrustCamera";

    /// The three ids minted against a freshly rendered template, whose own
    /// highest allocation is `…0033` (the embedding's `PBXBuildFile`).
    const MINTED_BUILD_FILE: &str = "ABCDABCDABCDABCDABCD0034";
    const MINTED_PRODUCT: &str = "ABCDABCDABCDABCDABCD0035";
    const MINTED_PACKAGE_REF: &str = "ABCDABCDABCDABCDABCD0036";

    fn apply_test_package(root: &Path) -> Result<AddOutcome, PluginAddError> {
        apply_swift_package_ref(root, PKG)
    }

    fn read_pbxproj(root: &Path) -> String {
        fs::read_to_string(root.join(PBXPROJ_REL)).unwrap()
    }

    /// Every object-id token on one pbxproj line, in order of appearance.
    fn pbx_ids_on_line(line: &str) -> Vec<&str> {
        line.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| is_pbx_object_id(token))
            .collect()
    }

    /// Splits a `project.pbxproj` into `(defined ids, referenced ids)` — a
    /// *definition* is a top-level entry of the `objects = { … }` map (exactly
    /// two leading tabs, an object id, then ` = {`), every other mention is a
    /// reference.
    ///
    /// Mirrors the scaffold test suite's helper of the same name
    /// (`scaffold/mod.rs`), which is `#[cfg(test)]`-private to that module and
    /// so unreachable from here; both exist to catch the same corruption — a
    /// duplicate, dangling, or orphaned id makes Xcode refuse to open the
    /// project.
    fn pbx_ids(pbxproj: &str) -> (Vec<String>, Vec<String>) {
        let mut defined = Vec::new();
        let mut referenced = Vec::new();
        for line in pbxproj.lines() {
            let ids = pbx_ids_on_line(line);
            let top_level = line
                .strip_prefix("\t\t")
                .filter(|rest| !rest.starts_with('\t'));
            let is_definition = top_level.is_some_and(|rest| {
                ids.first().is_some_and(|id| rest.starts_with(id)) && rest.contains(" = {")
            });
            let mut ids = ids.into_iter();
            if is_definition {
                defined.push(ids.next().expect("a definition line has an id").to_string());
            }
            referenced.extend(ids.map(str::to_string));
        }
        (defined, referenced)
    }

    /// No duplicate definition, no reference to an undefined id, no defined
    /// object nothing points at — the three ways a hand-edited pbxproj breaks.
    fn assert_pbxproj_well_formed(pbxproj: &str) {
        let (defined, referenced) = pbx_ids(pbxproj);
        let mut unique = defined.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            defined.len(),
            "duplicate PBX object id(s):\n{pbxproj}"
        );
        for id in &referenced {
            assert!(
                defined.contains(id),
                "dangling PBX object id `{id}`:\n{pbxproj}"
            );
        }
        for id in &defined {
            assert!(
                referenced.contains(id),
                "orphaned PBX object `{id}`:\n{pbxproj}"
            );
        }
        assert_eq!(
            pbxproj.matches("/* Begin ").count(),
            pbxproj.matches("/* End ").count(),
            "unbalanced PBX sections:\n{pbxproj}"
        );
    }

    /// Assert all six sites are wired for `package_name` with `ids`, each in
    /// its own section (so a line landing in the wrong section is caught).
    fn assert_six_sites(pbxproj: &str, package_name: &str, ids: &SwiftPackageIds) {
        for site in PBX_SITES {
            let body = pbx_site_body(pbxproj, site).unwrap();
            let needle = site.verify_needle(package_name, ids);
            assert!(
                body.contains(&needle),
                "site `{}` is missing `{needle}`:\n{pbxproj}",
                site.label()
            );
            assert_eq!(
                body.matches(&needle).count(),
                1,
                "site `{}` is duplicated:\n{pbxproj}",
                site.label()
            );
        }
    }

    #[test]
    fn swift_package_ref_wires_all_six_pbxproj_sites() {
        let root = scaffold_project("swift-package-fresh");
        let before = read_pbxproj(&root);

        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);
        let pbxproj = read_pbxproj(&root);

        // Ids continue the template's own numbering, sequentially.
        let ids = SwiftPackageIds {
            build_file: MINTED_BUILD_FILE.to_string(),
            product_dependency: MINTED_PRODUCT.to_string(),
            package_reference: MINTED_PACKAGE_REF.to_string(),
        };
        assert_eq!(resolve_or_mint_ids(&before, PKG).unwrap(), ids);
        assert_six_sites(&pbxproj, PKG, &ids);

        // The two objects carrying real payload: the reference names the
        // package by the `ios/FrustCamera` link, never by a directory.
        assert!(
            pbxproj.contains(&format!("\t\t\trelativePath = \"{PKG}\";")),
            "{pbxproj}"
        );
        assert!(
            pbxproj.contains(&format!("\t\t\tproductName = {PKG};")),
            "{pbxproj}"
        );
        // …and the build file pointing at the product dependency.
        assert!(
            pbxproj.contains(&format!("productRef = {MINTED_PRODUCT} /* {PKG} */;")),
            "{pbxproj}"
        );

        // The embedding's own wiring is untouched beside it.
        assert_six_sites(
            &pbxproj,
            "FrustEmbedding",
            &SwiftPackageIds {
                build_file: "ABCDABCDABCDABCDABCD0033".to_string(),
                product_dependency: "ABCDABCDABCDABCDABCD0032".to_string(),
                package_reference: "ABCDABCDABCDABCDABCD0031".to_string(),
            },
        );

        assert_pbxproj_well_formed(&pbxproj);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn swift_package_ref_reapply_is_already_present_and_byte_identical() {
        let root = scaffold_project("swift-package-idempotent");
        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);
        let after_first = snapshot_tree(&root);

        assert_eq!(
            apply_test_package(&root).unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Two sites deleted by hand — one list entry, one whole object — must be
    /// completed **with the surviving ids** (minting a second set would leave
    /// the survivors dangling), reporting `Applied`.
    #[test]
    fn half_wired_swift_package_ref_completes_the_missing_sites() {
        let root = scaffold_project("swift-package-half-wired");
        apply_test_package(&root).unwrap();

        let path = root.join(PBXPROJ_REL);
        let wired = read_pbxproj(&root);
        let frameworks_entry = format!("\t\t\t\t{MINTED_BUILD_FILE} /* {PKG} in Frameworks */,\n");
        let product_object = format!(
            "\t\t{MINTED_PRODUCT} /* {PKG} */ = {{\n\
             \t\t\tisa = XCSwiftPackageProductDependency;\n\
             \t\t\tproductName = {PKG};\n\
             \t\t}};\n"
        );
        let mangled = wired
            .replace(&frameworks_entry, "")
            .replace(&product_object, "");
        assert_eq!(
            mangled.len(),
            wired.len() - frameworks_entry.len() - product_object.len(),
            "expected to strip exactly the two sites"
        );
        fs::write(&path, &mangled).unwrap();

        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);

        let repaired = read_pbxproj(&root);
        assert_six_sites(
            &repaired,
            PKG,
            &SwiftPackageIds {
                build_file: MINTED_BUILD_FILE.to_string(),
                product_dependency: MINTED_PRODUCT.to_string(),
                package_reference: MINTED_PACKAGE_REF.to_string(),
            },
        );
        assert_pbxproj_well_formed(&repaired);

        let _ = fs::remove_dir_all(&root);
    }

    /// A consumer that hand-allocated ids past the template's (exactly what
    /// `examples/glyph-catalog` did with `…0034`–`…0036`) must be minted
    /// *past*, never over.
    #[test]
    fn swift_package_ref_mints_past_pre_existing_ids() {
        let root = scaffold_project("swift-package-renumbered");
        let path = root.join(PBXPROJ_REL);

        // Renumber the embedding wiring 0031-0033 -> 0034-0036, descending so
        // no replacement clobbers a later one.
        let mut renumbered = read_pbxproj(&root);
        for (from, to) in [("0033", "0036"), ("0032", "0035"), ("0031", "0034")] {
            renumbered = renumbered.replace(
                &format!("ABCDABCDABCDABCDABCD{from}"),
                &format!("ABCDABCDABCDABCDABCD{to}"),
            );
        }
        assert!(renumbered.contains("ABCDABCDABCDABCDABCD0036"));
        assert!(!renumbered.contains("ABCDABCDABCDABCDABCD0033"));
        fs::write(&path, &renumbered).unwrap();

        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);

        let pbxproj = read_pbxproj(&root);
        assert_six_sites(
            &pbxproj,
            PKG,
            &SwiftPackageIds {
                build_file: "ABCDABCDABCDABCDABCD0037".to_string(),
                product_dependency: "ABCDABCDABCDABCDABCD0038".to_string(),
                package_reference: "ABCDABCDABCDABCDABCD0039".to_string(),
            },
        );
        assert_pbxproj_well_formed(&pbxproj);

        let _ = fs::remove_dir_all(&root);
    }

    /// A missing section marker is a hard error, never a rewrite — and the
    /// whole file is left untouched, not half-edited (the five earlier sites
    /// only ever existed in memory).
    #[test]
    fn swift_package_ref_without_a_section_anchor_is_never_rewritten() {
        let root = scaffold_project("swift-package-no-anchor");
        let path = root.join(PBXPROJ_REL);
        let mangled = read_pbxproj(&root).replace(
            "/* End XCLocalSwiftPackageReference section */",
            "// (marker removed)",
        );
        fs::write(&path, &mangled).unwrap();

        let err = apply_test_package(&root).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == PBXPROJ_REL),
            "{err}"
        );
        assert_eq!(read_pbxproj(&root), mangled, "the file must be untouched");

        let _ = fs::remove_dir_all(&root);
    }

    /// The regression guard for anchor drift in already-checked-in
    /// consumers: `add_plugin`'s tests above only ever exercise a **freshly
    /// scaffolded** project, which by construction always carries the
    /// `// frust:plugin-includes`/`// frust:plugin-dependencies` anchors
    /// `Contribution::GradleModule` hard-requires — so they are structurally
    /// incapable of catching an existing in-repo consumer drifting away from
    /// them (exactly how early consumers — `glyph-catalog` among them —
    /// shipped without either anchor).
    ///
    /// This drives the real `add_plugin` path — not a grep — against a
    /// tempdir copy of every in-repo Android consumer, so a future project
    /// that ships without the anchors fails **this** test with a message
    /// naming the project and the missing anchor, instead of silently
    /// failing Add Plugin at demo time.
    #[test]
    fn every_in_repo_android_consumer_accepts_add_plugin() {
        let repo_root = repo_root();
        let consumers = discover_android_consumers(&repo_root);
        assert!(
            !consumers.is_empty(),
            "expected to discover at least one in-repo Android consumer under \
             examples/ or benchmarks/ (repo root resolved to {})",
            repo_root.display()
        );

        for (i, consumer) in consumers.iter().enumerate() {
            let display = consumer
                .strip_prefix(&repo_root)
                .unwrap_or(consumer)
                .display()
                .to_string();

            let dest = unique_temp_dir(&format!("consumer-{i}"));
            fs::create_dir_all(&dest).unwrap();
            copy_consumer_files(consumer, &dest).unwrap_or_else(|e| {
                panic!("failed to copy {display}'s plugin-relevant files into a tempdir: {e}")
            });

            match add_plugin(&dest, "secure-storage", &["biometric-gate"]) {
                Ok(report) => {
                    assert!(
                        report
                            .items
                            .iter()
                            .all(|i| i.outcome == AddOutcome::Applied),
                        "{display}: expected every contribution to apply cleanly \
                         against an unmodified copy, got {report:?}"
                    );
                }
                Err(PluginAddError::MalformedProjectFile(file)) => {
                    panic!(
                        "{display} is missing a plugin anchor in `{file}` — \
                         `add_plugin` requires `{SETTINGS_ANCHOR}` in \
                         `{SETTINGS_GRADLE_REL}` and `{APP_DEPS_ANCHOR}` in \
                         `{APP_BUILD_GRADLE_REL}` (see \
                         `crates/frust-drive/templates/app/android.tmpl` for the canonical \
                         placement, or `examples/huddle`'s android/ for a \
                         working in-repo example)"
                    );
                }
                Err(e) => panic!("{display}: add_plugin failed unexpectedly: {e}"),
            }

            let _ = fs::remove_dir_all(&dest);
        }
    }
}
