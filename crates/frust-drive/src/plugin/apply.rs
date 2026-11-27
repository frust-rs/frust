//! [`add_plugin`]: apply a registry plugin's contributions to a generated
//! project. Every edit is idempotent and format-preserving; a file that fails
//! to parse (or lacks its insertion anchor) is never rewritten.

use std::borrow::Cow;
use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

use super::catalog::{CatalogEntry, DepSource, PluginCatalog, PluginOrigin};
use super::{
    AddItem, AddOutcome, AddReport, Contribution, FloorPlatform, IosEntitlementValue, NativeKind,
    NativeModule, PluginAddError, PluginSpec,
};
use crate::build_dirs::BuildLayout;
use crate::facade_key;
use crate::host_path::{self, normalize_lexically};
use crate::packages::{CargoLocator, LockfilePolicy, PackageLocator};
use crate::platform_wiring::{self, SyncError};
use crate::process::RealProcessRunner;

/// Project-relative paths of the files a contribution edits.
pub(super) const CARGO_TOML_REL: &str = "Cargo.toml";
pub(super) const MANIFEST_REL: &str = "android/app/src/main/AndroidManifest.xml";
pub(super) const PLIST_REL: &str = "ios/Runner/Info.plist";
pub(super) const SETTINGS_GRADLE_REL: &str = "android/settings.gradle.kts";
pub(super) const APP_BUILD_GRADLE_REL: &str = "android/app/build.gradle.kts";
pub(super) const PBXPROJ_REL: &str = "ios/Runner.xcodeproj/project.pbxproj";
pub(super) const LIB_RS_REL: &str = "src/lib.rs";
pub(super) const THEMES_REL: &str = "android/app/src/main/res/values/themes.xml";
pub(super) const GRADLE_PROPERTIES_REL: &str = platform_wiring::GRADLE_PROPERTIES;

/// The Gradle property the app template's `cargoNdkBuild` task reads to pass
/// `cargo ndk --link-libcxx-shared` ([`Contribution::AndroidLibcxxShared`]).
pub(super) const LIBCXX_SHARED_PROPERTY: &str = "frust.cargoNdk.linkLibcxxShared";

/// The app-owned activity style the app template defines in [`THEMES_REL`]
/// and names in its manifest ([`Contribution::AndroidThemeParent`]).
pub(super) const APP_THEME_STYLE: &str = "FrustAppTheme";

/// The insertion anchors of `AndroidManifest.xml` and `Info.plist`.
pub(super) const MANIFEST_ANCHOR: &str = "</manifest>";
pub(super) const PLIST_ANCHOR: &str = "</dict>";

/// The marker comments the Android app template ships for plugin-contributed
/// Gradle wiring (`crates/frust-drive/templates/app/android.tmpl/settings.gradle.kts.tmpl` and
/// `app/build.gradle.kts.tmpl`). An insert goes on the line *after* the
/// marker; a project missing either marker is
/// [`PluginAddError::MalformedProjectFile`] and is never rewritten — guessing
/// at `include(":app")` or a bare `dependencies {` would be a second,
/// unpinned convention.
pub(super) const SETTINGS_ANCHOR: &str = "// frust:plugin-includes";
pub(super) const APP_DEPS_ANCHOR: &str = "// frust:plugin-dependencies";
pub(super) const REPOSITORIES_ANCHOR: &str = "// frust:plugin-repositories";

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
    add_plugin_from(project_root, &PluginCatalog::built_in(), id, features)
}

/// [`add_plugin`] for plugin `id` of `catalog` rather than of the built-in
/// registry alone: the same edits, the same idempotency, the same locator.
/// An id `catalog` lacks is [`PluginAddError::UnknownPlugin`].
///
/// A manifest-described entry ([`PluginOrigin::Manifest`]) is applied from
/// its own package: an app already depending on it (any table, any key)
/// keeps its line; otherwise `{ path = "<relative path>" }` is written for a
/// path package, `"<resolved version>"` for a crates.io one, never derived
/// from `frust`'s; a git or alternate-registry package is refused
/// ([`PluginAddError::DependencySourceNotWritable`]) before any write. Its
/// native paths are relative to the package directory the entry carries,
/// so the locator is never asked about it.
pub fn add_plugin_from(
    project_root: &Path,
    catalog: &PluginCatalog,
    id: &str,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    let locator =
        CargoLocator::new(&RealProcessRunner).with_lockfile_policy(LockfilePolicy::AllowUpdate);
    add_plugin_from_with(&locator, project_root, catalog, id, features)
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
///
/// A plugin or requested feature stating a platform floor
/// ([`PluginSpec::min_ios`] / [`PluginSpec::min_android_sdk`]) is checked
/// against the app's before anything is written, and refused with
/// [`PluginAddError::PlatformFloorTooLow`] when the app is below it.
pub fn add_plugin_with(
    locator: &dyn PackageLocator,
    project_root: &Path,
    id: &str,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    add_plugin_from_with(
        locator,
        project_root,
        &PluginCatalog::built_in(),
        id,
        features,
    )
}

/// [`add_plugin_from`] with the package locator injected, as
/// [`add_plugin_with`] is to [`add_plugin`].
pub fn add_plugin_from_with(
    locator: &dyn PackageLocator,
    project_root: &Path,
    catalog: &PluginCatalog,
    id: &str,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    let entry = catalog
        .find(id)
        .ok_or_else(|| PluginAddError::UnknownPlugin(id.to_string()))?;
    // A manifest-described plugin's items for a platform the app lacks are
    // skipped, as `pending_contributions` and `sync_plugins` skip them.
    let entry = super::pending::for_project(project_root, entry);
    add_entry_with(locator, project_root, &entry, features)
}

/// Read Cargo.toml: never rewritten (nor read further) if it can't parse.
pub(super) fn read_cargo_doc(project_root: &Path) -> Result<DocumentMut, PluginAddError> {
    fs::read_to_string(project_root.join(CARGO_TOML_REL))
        .map_err(|_| PluginAddError::MissingProjectFile(CARGO_TOML_REL.to_string()))?
        .parse::<DocumentMut>()
        .map_err(|e| PluginAddError::UnparseableCargoToml(e.to_string()))
}

/// The contributions adding `spec` with `features` applies: the base, then
/// the selected features' in registry order (a stable report regardless of
/// the caller's feature ordering). An unknown feature is refused.
pub(super) fn selected_contributions<'s>(
    spec: &'s PluginSpec,
    features: &[&str],
) -> Result<Vec<&'s Contribution>, PluginAddError> {
    for feat in features {
        if !spec.optional_features.iter().any(|f| f.id == *feat) {
            return Err(PluginAddError::UnknownFeature {
                plugin: spec.id.to_string(),
                feature: (*feat).to_string(),
            });
        }
    }
    let feature_contribs = spec
        .optional_features
        .iter()
        .filter(|f| features.contains(&&*f.id))
        .flat_map(|f| f.contributions.iter());
    Ok(spec.base.iter().chain(feature_contribs).collect())
}

/// What an add of one entry resolved before its first write.
pub(super) struct PreparedAdd<'e> {
    /// The contributions to apply, in report order.
    contributions: Vec<&'e Contribution>,
    /// The parsed `Cargo.toml`.
    doc: DocumentMut,
    /// How a missing plugin dependency line is written.
    form: DepForm,
}

/// Every check an add of `entry` with `features` makes before it writes
/// anything — the requested features, the Cargo line it would write, the
/// platform floors, the native-path rule, the iOS entitlement plan, the
/// Maven repository marker and the build script's `libc++_shared.so`
/// property — shared by [`add_plugin_from_with`] and
/// [`super::sync_plugins`], which runs it for every plugin before the first
/// write. Reads, never writes.
pub(super) fn prepare_add<'e>(
    project_root: &Path,
    entry: &'e CatalogEntry,
    features: &[&str],
) -> Result<PreparedAdd<'e>, PluginAddError> {
    let spec = &entry.spec;
    let contributions = selected_contributions(spec, features)?;
    let doc = read_cargo_doc(project_root)?;

    // A built-in's dependency line follows the app's `frust` dependency; a
    // manifest plugin's comes from its own package.
    let form = match &entry.origin {
        PluginOrigin::BuiltIn => {
            DepForm::Facade(FrustDep::read(&doc).ok_or(PluginAddError::NoFrustDependency)?)
        }
        PluginOrigin::Manifest {
            package,
            package_dir,
            version,
            source,
        } => {
            check_manifest_expressible(spec)?;
            if *source == DepSource::Other && !cargo_dep_present(&doc, package, &entry.origin) {
                return Err(PluginAddError::DependencySourceNotWritable {
                    plugin: spec.id.to_string(),
                    package: package.clone(),
                });
            }
            DepForm::Manifest {
                package_dir: package_dir.clone(),
                version: version.clone(),
                source: *source,
            }
        }
    };

    // The app's iOS / Android floors must meet the plugin's (and every
    // requested feature's); they are checked here and never raised.
    check_platform_floors(project_root, spec, features)?;

    // A platform path outside the plugin's own package cannot be located;
    // refuse it before anything is written.
    for contribution in &contributions {
        if let Some(rel_path) = native_rel_path(contribution) {
            package_location_in(spec, &entry.origin, rel_path)?;
        }
    }

    // An iOS entitlement edits two files whose shape is only known once the
    // pbxproj's Runner target is resolved; plan it in memory (and discard the
    // plan) so a project it cannot be applied to is refused before the first
    // write. A project without `ios/` is left to the applier itself, which
    // answers it as the plist and Swift package appliers do (a missing
    // project file); only the link flags count as satisfied there.
    if project_root.join(IOS_DIR_REL).is_dir() {
        for contribution in &contributions {
            if let Contribution::IosEntitlement {
                key,
                value,
                comment,
            } = contribution
            {
                plan_ios_entitlement(project_root, key, value, comment, false)?;
            }
        }
    }

    // A Maven repository needs its marker in settings.gradle.kts and a
    // well-formed url/group set; check both before the first write. A project
    // without `android/settings.gradle.kts` is left to the applier, which
    // answers it as every other Gradle contribution does.
    for contribution in &contributions {
        if let Contribution::MavenRepository { url, groups, .. } = contribution {
            validate_maven_repository(url, groups)?;
            if let Ok(settings) = fs::read_to_string(project_root.join(SETTINGS_GRADLE_REL))
                && !settings.contains(REPOSITORIES_ANCHOR)
            {
                return Err(missing_repositories_marker());
            }
        }
    }

    // The shared C++ runtime is packaged only by a `cargoNdkBuild` task that
    // reads its property; a build script that predates it is refused before
    // the first write. A project without the build script is left to the
    // applier, as for a Maven repository.
    if contributions
        .iter()
        .any(|c| matches!(c, Contribution::AndroidLibcxxShared))
        && project_root.join(ANDROID_DIR_REL).is_dir()
        && let Ok(build) = fs::read_to_string(project_root.join(APP_BUILD_GRADLE_REL))
        && !libcxx_shared_reader_present(&build)
    {
        return Err(missing_libcxx_shared_reader());
    }

    // A theme parent needs an activity that uses the app-owned style (or a
    // manifest theme that is already an accepted Material one); any other
    // app is refused here, before the Cargo dependency or any Gradle line is
    // written.
    for contribution in &contributions {
        if let Contribution::AndroidThemeParent { parent } = contribution
            && android_theme_parent(project_root, parent)? == ThemeParent::NoAppTheme
        {
            return Err(android_app_theme_missing(&spec.id, parent));
        }
    }

    Ok(PreparedAdd {
        contributions,
        doc,
        form,
    })
}

/// Manifest-described plugins carry no contribution their schema cannot
/// express: an app-crate macro or a scaffolded file is refused, as is a
/// sibling checkout (all three exist for built-in plugins only).
fn check_manifest_expressible(spec: &PluginSpec) -> Result<(), PluginAddError> {
    let all = spec.base.iter().chain(
        spec.optional_features
            .iter()
            .flat_map(|f| f.contributions.iter()),
    );
    for contribution in all {
        if matches!(
            contribution,
            Contribution::AppCrateMacro { .. } | Contribution::ScaffoldFile { .. }
        ) {
            return Err(PluginAddError::InvalidContribution(format!(
                "plugin `{}` is described by a package manifest, which cannot carry {}",
                spec.id,
                contribution.describe()
            )));
        }
    }
    if spec.requires_sibling.is_some() {
        return Err(PluginAddError::InvalidContribution(format!(
            "plugin `{}` is described by a package manifest, which cannot require a sibling \
             checkout",
            spec.id
        )));
    }
    Ok(())
}

/// [`add_entry_with`] for a synthetic built-in `spec` the static registry
/// does not carry.
#[cfg(test)]
fn add_spec_with(
    locator: &dyn PackageLocator,
    project_root: &Path,
    spec: &PluginSpec,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    add_entry_with(
        locator,
        project_root,
        &CatalogEntry::built_in(spec.clone()),
        features,
    )
}

/// [`add_plugin_from_with`] for an already-resolved catalog entry — the seam
/// a test drives with a synthetic entry the static registry does not carry.
pub(super) fn add_entry_with(
    locator: &dyn PackageLocator,
    project_root: &Path,
    entry: &CatalogEntry,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    let spec = &entry.spec;
    let PreparedAdd {
        contributions,
        mut doc,
        form,
    } = prepare_add(project_root, entry, features)?;
    let resolution = Resolution::new(locator, project_root, spec, &form);

    // Facade plugins need a sibling checkout present; check before any edit
    // so a missing sibling leaves the tree untouched.
    if let Some(sibling) = spec.requires_sibling.as_deref() {
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
        write_file(
            &project_root.join(CARGO_TOML_REL),
            CARGO_TOML_REL,
            &doc.to_string(),
        )?;
    }

    // The native modules whose machine-local key or link is still missing,
    // and — now that cargo can see the dependency, before any platform file
    // is edited — the package they live in. Every module of one plugin
    // ships in the same package (its base Cargo dependency's).
    let unwired: Vec<(usize, NativeModule)> = contributions
        .iter()
        .enumerate()
        .filter_map(|(at, contribution)| {
            Some((
                at,
                native_module_in(spec, &entry.origin, contribution)?.ok()?,
            ))
        })
        .filter(|(_, module)| {
            platform_present(project_root, module)
                && !platform_wiring::module_slot_present(project_root, module)
        })
        .collect();
    let package_dir = match unwired.first() {
        Some((_, module)) => Some(resolution.package_dir(&module.package)?),
        None => None,
    };

    // Everything else. A contribution for a platform whose directory the
    // project lacks has nothing to edit and is left out of the report, as
    // `pending::for_project` leaves it out of a manifest plugin's; a
    // manifest plugin's Swift package reports its `.gitignore` line as an
    // item of its own, as the pending report lists it. `item_at[i]` is the
    // report row of contribution `i`.
    let mut items = Vec::with_capacity(contributions.len());
    let mut item_at = vec![None; contributions.len()];
    for (at, (contribution, applied)) in contributions.iter().zip(outcomes).enumerate() {
        let outcome = match applied {
            Some(outcome) => outcome,
            None => apply_contribution(contribution, &resolution, &mut doc, &mut cargo_changed)?,
        };
        let platform_absent = super::pending::platform_dir(contribution)
            .is_some_and(|dir| !project_root.join(dir).is_dir());
        if platform_absent && outcome == AddOutcome::AlreadyPresent {
            continue;
        }
        item_at[at] = Some(items.len());
        items.push(AddItem {
            description: contribution.describe(),
            outcome,
        });
        if let Contribution::SwiftPackageRef { package_name, .. } = contribution
            && matches!(resolution.form, DepForm::Manifest { .. })
            && project_root.join(GITIGNORE_REL).is_file()
        {
            items.push(AddItem {
                description: gitignore_link_label(package_name),
                outcome: apply_gitignore_ios_link(project_root, package_name)?,
            });
        }
    }

    // The machine-local keys and links, for the modules whose directory is
    // actually there.
    if let Some(package_dir) = package_dir {
        let mut slots = Vec::new();
        let mut targets = Vec::new();
        for (at, module) in unwired {
            let dir = package_dir.join(&*module.dir_in_package);
            if dir.is_dir() {
                let dir = host_path::canonicalize_simplified(&dir).unwrap_or(dir);
                slots.push(at);
                targets.push((module, dir));
            }
        }
        let wirings =
            platform_wiring::wire_modules(project_root, &targets).map_err(wiring_error)?;
        for (at, wiring) in slots.into_iter().zip(wirings) {
            if wiring.changed()
                && let Some(row) = item_at[at]
            {
                items[row].outcome = AddOutcome::Applied;
            }
        }
    }

    Ok(AddReport {
        plugin_id: spec.id.to_string(),
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

/// The build setting an iOS floor is read from.
const IOS_DEPLOYMENT_TARGET: &str = "IPHONEOS_DEPLOYMENT_TARGET";
/// The Android app template's single floor variable, and the `defaultConfig`
/// property read when a project has no such variable.
const MIN_SDK_VALUE: &str = "minSdkValue";
const MIN_SDK: &str = "minSdk";
pub(super) const ANDROID_DIR_REL: &str = "android";

/// The highest floor over a plugin and its requested features: the iOS
/// version as the registry writes it, and the Android API level.
#[derive(Debug, Default, PartialEq, Eq)]
struct FloorRequirement<'s> {
    ios: Option<&'s str>,
    android_sdk: Option<u32>,
}

/// The [`FloorRequirement`] of adding `spec` with `features`: the maximum
/// over the spec's floors and those of every requested feature. A registry
/// `min_ios` that is not a dot-separated version is
/// [`PluginAddError::InvalidContribution`].
fn floor_requirement<'s>(
    spec: &'s PluginSpec,
    features: &[&str],
) -> Result<FloorRequirement<'s>, PluginAddError> {
    let requested = spec
        .optional_features
        .iter()
        .filter(|f| features.contains(&&*f.id))
        .map(|f| (f.min_ios.as_deref(), f.min_android_sdk));
    let mut requirement = FloorRequirement::default();
    let mut highest_ios: Option<Vec<u32>> = None;
    let own = (spec.min_ios.as_deref(), spec.min_android_sdk);
    for (ios, android_sdk) in std::iter::once(own).chain(requested) {
        if let Some(ios) = ios {
            let version = parse_ios_version(ios).ok_or_else(|| {
                PluginAddError::InvalidContribution(format!(
                    "plugin `{}` declares minimum iOS version `{ios}`, which is not a \
                     dot-separated version",
                    spec.id
                ))
            })?;
            if highest_ios
                .as_ref()
                .is_none_or(|highest| compare_versions(&version, highest).is_gt())
            {
                highest_ios = Some(version);
                requirement.ios = Some(ios);
            }
        }
        requirement.android_sdk = requirement.android_sdk.max(android_sdk);
    }
    Ok(requirement)
}

/// Refuse, before any write, an app whose iOS deployment target or Android
/// `minSdk` is below what adding `spec` with `features` needs. A platform the
/// app has no directory for is skipped; a floor that cannot be read is
/// [`PluginAddError::MalformedProjectFile`] naming the key looked for, never a
/// guess. Nothing is read when the add states no floor.
fn check_platform_floors(
    project_root: &Path,
    spec: &PluginSpec,
    features: &[&str],
) -> Result<(), PluginAddError> {
    let requirement = floor_requirement(spec, features)?;
    if let Some(required) = requirement.ios
        && project_root.join(IOS_DIR_REL).is_dir()
    {
        let src = read_required(&project_root.join(PBXPROJ_REL), PBXPROJ_REL)?;
        let (found, found_version) = lowest_ios_deployment_target(&src)?;
        let required_version = parse_ios_version(required).unwrap_or_default();
        if compare_versions(&found_version, &required_version).is_lt() {
            return Err(PluginAddError::PlatformFloorTooLow {
                plugin: spec.id.as_ref().into(),
                platform: FloorPlatform::Ios,
                required: required.to_string(),
                found,
                key: IOS_DEPLOYMENT_TARGET,
                file: PBXPROJ_REL,
                hint: "Raise it in every build configuration (and keep it at or above every \
                       Swift package's minimum), then re-run.",
            });
        }
    }
    if let Some(required) = requirement.android_sdk
        && project_root.join(ANDROID_DIR_REL).is_dir()
    {
        let src = read_required(
            &project_root.join(APP_BUILD_GRADLE_REL),
            APP_BUILD_GRADLE_REL,
        )?;
        let (key, found) = android_min_sdk(&src)?;
        if found < required {
            return Err(PluginAddError::PlatformFloorTooLow {
                plugin: spec.id.as_ref().into(),
                platform: FloorPlatform::Android,
                required: required.to_string(),
                found: found.to_string(),
                key,
                file: APP_BUILD_GRADLE_REL,
                hint: "Raise it (and any product flavor's own `minSdk` below it), then re-run.",
            });
        }
    }
    Ok(())
}

/// A dot-separated version (`"16"`, `"16.4"`, `"17.0.1"`) as its numeric
/// components; `None` for anything else.
fn parse_ios_version(text: &str) -> Option<Vec<u32>> {
    text.trim()
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        })
        .collect()
}

/// Compare two versions component by component, a missing component counting
/// as `0` (so `16` equals `16.0`).
fn compare_versions(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    (0..a.len().max(b.len()))
        .map(|at| {
            let left = a.get(at).copied().unwrap_or(0);
            let right = b.get(at).copied().unwrap_or(0);
            left.cmp(&right)
        })
        .find(|order| order.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// The lowest `IPHONEOS_DEPLOYMENT_TARGET` over every build configuration of
/// the project, project-level and target-level alike (so a flavored project's
/// extra configurations count too): its value as written, and that value
/// parsed. No setting at all, or one that is not a version, is
/// [`PluginAddError::MalformedProjectFile`].
fn lowest_ios_deployment_target(src: &str) -> Result<(String, Vec<u32>), PluginAddError> {
    let mut lowest: Option<(String, Vec<u32>)> = None;
    let objects = pbx_objects(src, pbx_section_range(src, "XCBuildConfiguration")?)?;
    for object in &objects {
        let statements = pbx_statements(src, object.body.clone())?;
        let Some(settings) = statements.iter().find(|s| s.key == "buildSettings") else {
            continue;
        };
        let body = pbx_dict_body(src, settings.value.clone())?;
        for setting in pbx_statements(src, body)? {
            if setting.key != IOS_DEPLOYMENT_TARGET {
                continue;
            }
            let raw = pbx_unquote(pbx_strip_comments(&src[setting.value.clone()]).trim());
            let version = parse_ios_version(&raw).ok_or_else(|| {
                PluginAddError::MalformedProjectFile(format!(
                    "{PBXPROJ_REL}: `{IOS_DEPLOYMENT_TARGET} = {raw}` is not a version the \
                     plugin's minimum iOS version can be compared with"
                ))
            })?;
            if lowest
                .as_ref()
                .is_none_or(|(_, low)| compare_versions(&version, low).is_lt())
            {
                lowest = Some((raw, version));
            }
        }
    }
    lowest.ok_or_else(|| {
        PluginAddError::MalformedProjectFile(format!(
            "{PBXPROJ_REL}: no build configuration sets `{IOS_DEPLOYMENT_TARGET}`, so the \
             plugin's minimum iOS version cannot be checked"
        ))
    })
}

/// The app's Android floor from `app/build.gradle.kts`: the integer assigned
/// to `minSdkValue`, else a literal `minSdk = <n>` (the first one, which is
/// `defaultConfig`'s in a generated project; per-flavor overrides are not
/// read). Returns the key read and its value; a floor that is absent or not
/// an integer literal is [`PluginAddError::MalformedProjectFile`].
fn android_min_sdk(src: &str) -> Result<(&'static str, u32), PluginAddError> {
    for key in [MIN_SDK_VALUE, MIN_SDK] {
        if let Some(value) = gradle_assignment(src, key) {
            return value.parse::<u32>().map(|level| (key, level)).map_err(|_| {
                PluginAddError::MalformedProjectFile(format!(
                    "{APP_BUILD_GRADLE_REL}: `{key} = {value}` is not an integer API level the \
                     plugin's minimum Android SDK can be compared with"
                ))
            });
        }
    }
    Err(PluginAddError::MalformedProjectFile(format!(
        "{APP_BUILD_GRADLE_REL}: found neither `val {MIN_SDK_VALUE} = <n>` nor \
         `{MIN_SDK} = <n>`, so the plugin's minimum Android SDK cannot be checked"
    )))
}

/// The right-hand side of the first `key = …` assignment in a Kotlin build
/// script (optionally `val`/`var`-declared, optionally typed), comments
/// skipped; `None` when no line assigns `key`.
fn gradle_assignment<'s>(src: &'s str, key: &str) -> Option<&'s str> {
    src.lines().find_map(|line| {
        let code = line.split("//").next().unwrap_or_default().trim();
        if code.starts_with("/*") || code.starts_with('*') {
            return None;
        }
        let declared = code
            .strip_prefix("val ")
            .or_else(|| code.strip_prefix("var "))
            .unwrap_or(code)
            .trim_start();
        let rest = declared.strip_prefix(key)?.trim_start();
        let rest = match rest.strip_prefix(':') {
            Some(typed) => &typed[typed.find('=')?..],
            None => rest,
        };
        let value = rest.strip_prefix('=')?;
        if value.starts_with('=') {
            return None;
        }
        Some(value.trim().trim_end_matches(';').trim_end())
    })
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
fn native_rel_path(contribution: &Contribution) -> Option<&str> {
    match contribution {
        Contribution::GradleModule { rel_path, .. }
        | Contribution::SwiftPackageRef { rel_path, .. } => Some(rel_path),
        _ => None,
    }
}

/// The [`NativeModule`] a Gradle module or Swift package contribution of
/// built-in `spec` wires, `None` for every other contribution; an error when
/// its registry path lies outside the plugin's own package.
pub(crate) fn native_module(
    spec: &PluginSpec,
    contribution: &Contribution,
) -> Option<Result<NativeModule, PluginAddError>> {
    native_module_in(spec, &PluginOrigin::BuiltIn, contribution)
}

/// [`native_module`] for a `spec` of `origin`, its path read by
/// [`package_location_in`]'s rule for that origin.
pub(crate) fn native_module_in(
    spec: &PluginSpec,
    origin: &PluginOrigin,
    contribution: &Contribution,
) -> Option<Result<NativeModule, PluginAddError>> {
    let (rel_path, kind) = match contribution {
        Contribution::GradleModule {
            gradle_name,
            rel_path,
        } => (
            rel_path,
            NativeKind::GradleModule {
                gradle_name: gradle_name.clone(),
            },
        ),
        Contribution::SwiftPackageRef {
            package_name,
            rel_path,
        } => (
            rel_path,
            NativeKind::SwiftPackage {
                package_name: package_name.clone(),
            },
        ),
        _ => return None,
    };
    Some(
        package_location_in(spec, origin, rel_path).map(|(package, inside)| NativeModule {
            package: package.clone(),
            // `inside` is a suffix of `rel_path`: re-sliced from the
            // `'static` original it stays borrowed, so a built-in module
            // allocates nothing.
            dir_in_package: match rel_path {
                Cow::Borrowed(path) => Cow::Borrowed(&path[path.len() - inside.len()..]),
                Cow::Owned(_) => Cow::Owned(inside.to_owned()),
            },
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
        Contribution::CargoDep { name } => match resolution.form {
            DepForm::Facade(frust) => {
                apply_cargo_dep(doc, cargo_changed, name, &resolution.spec.crate_dir, frust)
            }
            DepForm::Manifest {
                package_dir,
                version,
                source,
            } => apply_manifest_cargo_dep(
                doc,
                cargo_changed,
                name,
                &resolution.spec.id,
                project_root,
                ManifestDep {
                    package_dir,
                    version,
                    source: *source,
                },
            ),
        },
        Contribution::ManifestPermission { permission } => {
            apply_manifest_permission(project_root, permission)
        }
        Contribution::PlistEntry {
            key,
            value,
            comment,
        } => apply_plist_entry(project_root, key, value, comment),
        Contribution::PlistArrayEntry {
            key,
            values,
            comment,
        } => apply_plist_array_entry(project_root, key, values, comment),
        Contribution::GradleModule { gradle_name, .. } => {
            apply_gradle_module(project_root, gradle_name)
        }
        Contribution::MavenRepository {
            url,
            groups,
            comment,
        } => apply_maven_repository(project_root, url, groups, comment),
        Contribution::AndroidLibcxxShared => apply_android_libcxx_shared(project_root),
        Contribution::AndroidThemeParent { parent } => {
            apply_android_theme_parent(project_root, &resolution.spec.id, parent)
        }
        // A manifest plugin's `.gitignore` line is its own report item,
        // applied by `add_entry_with` right after this one.
        Contribution::SwiftPackageRef { package_name, .. } => {
            apply_swift_package_ref(project_root, package_name)
        }
        Contribution::IosFramework { name } => apply_ios_framework(project_root, name),
        Contribution::IosLibrary { name } => apply_ios_library(project_root, name),
        Contribution::IosEntitlement {
            key,
            value,
            comment,
        } => apply_ios_entitlement(project_root, key, value, comment),
        Contribution::AppCrateMacro {
            invocation,
            cfg,
            comment,
        } => apply_app_crate_macro(project_root, invocation, cfg.as_deref(), comment),
        Contribution::CargoFeature { name, feature } => {
            let lookup = match resolution.form {
                DepForm::Facade(_) => DepLookup::Dependencies,
                DepForm::Manifest { .. } => DepLookup::AnyTable,
            };
            apply_cargo_feature(doc, cargo_changed, name, feature, lookup)
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
/// result row. Both fields are copies of the catalog's data; for a built-in
/// plugin they borrow the static registry and allocate nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopContribution {
    /// The owning plugin's [`PluginSpec::id`].
    pub plugin_id: Cow<'static, str>,
    /// The contribution itself — always one of the three desktop variants.
    pub contribution: Contribution,
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
/// Order is deterministic: registry order ([`super::known_plugins`]), then
/// each plugin's own declaration order within `base`.
pub fn desktop_contributions(
    project_root: &Path,
) -> Result<Vec<DesktopContribution>, PluginAddError> {
    desktop_contributions_in(project_root, &PluginCatalog::built_in())
}

/// [`desktop_contributions`] over the plugins of `catalog`, in catalog order.
/// A manifest-described plugin counts as installed through an entry in
/// `[dependencies]` or a target table, but its schema carries no
/// desktop-lane contribution, so it contributes no row.
pub fn desktop_contributions_in(
    project_root: &Path,
    catalog: &PluginCatalog,
) -> Result<Vec<DesktopContribution>, PluginAddError> {
    let doc = read_cargo_doc(project_root)?;
    let specs = catalog
        .iter()
        .filter(|entry| entry_is_installed(entry, &doc))
        .map(|entry| &entry.spec);
    Ok(desktop_rows(specs))
}

/// [`desktop_contributions`]'s pure inner logic, parameterized over the
/// registry list and the parsed `Cargo.toml` doc so a test can inject
/// synthetic [`PluginSpec`]s, so the detection logic is testable independently
/// of which desktop-lane rows (`MacosPlistEntry`, `MacosEntitlement`) the
/// real registry carries at any given time.
#[cfg(test)]
fn desktop_contributions_for<'s>(
    specs: impl IntoIterator<Item = &'s PluginSpec>,
    doc: &DocumentMut,
) -> Vec<DesktopContribution> {
    desktop_rows(
        specs
            .into_iter()
            .filter(|spec| plugin_is_installed(spec, doc)),
    )
}

/// The desktop-lane rows of `specs`' base contributions, in order.
fn desktop_rows<'s>(specs: impl IntoIterator<Item = &'s PluginSpec>) -> Vec<DesktopContribution> {
    specs
        .into_iter()
        .flat_map(|spec| {
            spec.base
                .iter()
                .filter(|contribution| is_desktop_contribution(contribution))
                .map(move |contribution| DesktopContribution {
                    plugin_id: spec.id.clone(),
                    contribution: contribution.clone(),
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
        Contribution::CargoDep { name } => Some(&**name),
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

/// Whether the app `doc` describes depends on `entry`'s plugin: for a
/// built-in, [`plugin_is_installed`]; for a manifest-described plugin, a
/// non-optional entry on its package in `[dependencies]` or any
/// `[target.<cfg>.dependencies]` table, by key or `package = …`. Discovery
/// keeps only a package the app's root package reaches, preferring the
/// root's direct normal dependency; whether the app itself declares that
/// package is this check's to answer.
pub(crate) fn entry_is_installed(entry: &CatalogEntry, doc: &DocumentMut) -> bool {
    match &entry.origin {
        PluginOrigin::BuiltIn => plugin_is_installed(&entry.spec, doc),
        PluginOrigin::Manifest { package, .. } => dependency_tables(doc).iter().any(|deps| {
            find_package_entry(*deps, package).is_some_and(|(_, v)| !dep_is_optional(v))
        }),
    }
}

/// Whether the app at `project_root` depends on the plugin of `entry` — the
/// one rule `sync_plugins`, the machine-local wiring and
/// [`desktop_contributions_in`] pick plugins by. A built-in entry counts when
/// its own Cargo dependency is a non-optional `[dependencies]` entry, by key
/// or `package = "…"` rename; a manifest-described one when a non-optional
/// entry on its package sits in `[dependencies]` or any
/// `[target.<cfg>.dependencies]` table, by key or rename. Reads only; a
/// missing or unparseable `Cargo.toml` is the error an add would give.
pub fn is_installed(project_root: &Path, entry: &CatalogEntry) -> Result<bool, PluginAddError> {
    Ok(entry_is_installed(entry, &read_cargo_doc(project_root)?))
}

/// Where a plugin's dependency entry is looked for in the app's `Cargo.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DepLookup {
    /// `[dependencies].<name>` only: the line a built-in add writes.
    Dependencies,
    /// Any entry on the package — `[dependencies]` or a
    /// `[target.<cfg>.dependencies]` table, by key or `package = …`: the
    /// line a manifest-described plugin is found by.
    AnyTable,
}

impl DepLookup {
    /// The lookup a plugin of `origin` is found by.
    pub(super) fn of(origin: &PluginOrigin) -> Self {
        match origin {
            PluginOrigin::BuiltIn => Self::Dependencies,
            PluginOrigin::Manifest { .. } => Self::AnyTable,
        }
    }
}

/// `[dependencies]`, then every `[target.<cfg>.dependencies]`, in document
/// order.
fn dependency_tables(doc: &DocumentMut) -> Vec<&dyn toml_edit::TableLike> {
    let mut tables: Vec<&dyn toml_edit::TableLike> = Vec::new();
    if let Some(deps) = doc.get("dependencies").and_then(Item::as_table_like) {
        tables.push(deps);
    }
    if let Some(targets) = doc.get("target").and_then(Item::as_table_like) {
        for (_, target) in targets.iter() {
            if let Some(deps) = target.get("dependencies").and_then(Item::as_table_like) {
                tables.push(deps);
            }
        }
    }
    tables
}

/// The entry of `deps` on `package`: keyed by its name, or renamed onto it.
fn find_package_entry<'d>(
    deps: &'d dyn toml_edit::TableLike,
    package: &str,
) -> Option<(&'d str, &'d Item)> {
    deps.iter()
        .find(|(key, value)| *key == package || dep_renamed_package(value) == Some(package))
}

/// The dependency entry a [`Contribution::CargoDep`] / [`Contribution::CargoFeature`]
/// for `name` reads, found by `lookup`.
fn dependency_entry<'d>(doc: &'d DocumentMut, name: &str, lookup: DepLookup) -> Option<&'d Item> {
    match lookup {
        DepLookup::Dependencies => doc.get("dependencies")?.as_table_like()?.get(name),
        DepLookup::AnyTable => dependency_tables(doc)
            .into_iter()
            .find_map(|deps| find_package_entry(deps, name).map(|(_, item)| item)),
    }
}

/// [`dependency_entry`], mutably.
fn dependency_entry_mut<'d>(
    doc: &'d mut DocumentMut,
    name: &str,
    lookup: DepLookup,
) -> Option<&'d mut Item> {
    let key = |deps: &dyn toml_edit::TableLike| {
        find_package_entry(deps, name).map(|(key, _)| key.to_string())
    };
    if lookup == DepLookup::Dependencies {
        return doc
            .get_mut("dependencies")?
            .as_table_like_mut()?
            .get_mut(name);
    }
    if let Some(found) = doc
        .get("dependencies")
        .and_then(Item::as_table_like)
        .and_then(key)
    {
        return doc
            .get_mut("dependencies")?
            .as_table_like_mut()?
            .get_mut(&found);
    }
    let targets = doc.get("target").and_then(Item::as_table_like)?;
    let (cfg, found) = targets.iter().find_map(|(cfg, target)| {
        let deps = target.get("dependencies")?.as_table_like()?;
        Some((cfg.to_string(), key(deps)?))
    })?;
    doc.get_mut("target")?
        .as_table_like_mut()?
        .get_mut(&cfg)?
        .as_table_like_mut()?
        .get_mut("dependencies")?
        .as_table_like_mut()?
        .get_mut(&found)
}

/// Whether the [`Contribution::CargoDep`] for `name` of a plugin of `origin`
/// is in place — the predicate [`apply_cargo_dep`] and
/// [`apply_manifest_cargo_dep`] skip by.
pub(super) fn cargo_dep_present(doc: &DocumentMut, name: &str, origin: &PluginOrigin) -> bool {
    dependency_entry(doc, name, DepLookup::of(origin)).is_some()
}

/// Whether `feature` is enabled on the dependency entry `lookup` finds for
/// `name` — the predicate [`apply_cargo_feature`] skips by.
pub(super) fn cargo_feature_present(
    doc: &DocumentMut,
    name: &str,
    feature: &str,
    lookup: DepLookup,
) -> bool {
    dependency_entry(doc, name, lookup)
        .and_then(|dep| dep.as_table_like())
        .and_then(|dep| dep.get("features"))
        .and_then(Item::as_array)
        .is_some_and(|features| features.iter().any(|v| v.as_str() == Some(feature)))
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
    if app_crate_macro_present(&src, invocation) {
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

/// Whether `src/lib.rs` text `src` already carries `invocation`.
pub(super) fn app_crate_macro_present(src: &str, invocation: &str) -> bool {
    src.contains(invocation)
}

/// How the project depends on the framework facade, read from its
/// `[dependencies]` entry for the facade package under whichever key names
/// it (`frust`, `frust-ui`, or a rename with `package = "frust-ui"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FrustDep {
    /// A `path` is present: a local checkout (path mode). `path` is the value
    /// as written, relative to the project root or absolute.
    Path { path: String, package: String },
    /// A `version` without a `path`, or the `frust = "<version>"` shorthand:
    /// the crates.io release (registry mode). `version` is the requirement as
    /// written.
    Registry { version: String, package: String },
}

impl FrustDep {
    /// The mode `doc`'s facade entry declares (the first, in document order,
    /// whose package is the facade — [`facade_key::facade_entries`]), or
    /// `None` when there is no such entry or it carries neither a `path` nor
    /// a `version`. `package` defaults to the dependency key, as cargo's own
    /// does — though every scaffolded project names `frust-ui` explicitly in
    /// both modes.
    fn read(doc: &DocumentMut) -> Option<Self> {
        let deps = doc.get("dependencies")?.as_table_like()?;
        let (key, item) = facade_key::facade_entries(deps).next()?;
        if let Some(version) = item.as_str() {
            return Some(Self::Registry {
                version: version.to_string(),
                package: key.to_string(),
            });
        }
        let table = item.as_table_like()?;
        let package = table
            .get("package")
            .and_then(Item::as_str)
            .unwrap_or(key)
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
/// crate under its `plugins/`. Relative to the same base as `frust_path`,
/// and folded lexically ([`host_path::collapse_dot_segments`]), so
/// `../../crates/frust` gives `../../plugins/<crate_dir>`.
fn plugin_dep_path(frust_path: &str, crate_dir: &str) -> String {
    host_path::collapse_dot_segments(&format!("{frust_path}/../../plugins/{crate_dir}"))
}

/// The package a plugin's platform files ship in: the crate its base
/// [`Contribution::CargoDep`] adds.
fn plugin_package(spec: &PluginSpec) -> Option<&Cow<'static, str>> {
    spec.base
        .iter()
        .find_map(|contribution| match contribution {
            Contribution::CargoDep { name } => Some(name),
            _ => None,
        })
}

/// Where a registry platform path (repo-root-relative,
/// `plugins/<crate_dir>/…`) lives as a package location: the plugin's own
/// package ([`plugin_package`]) and the path inside it, e.g.
/// `platform/android` — the part that stays the same wherever cargo finds
/// the package.
#[cfg(test)]
fn package_location<'s, 'r>(
    spec: &'s PluginSpec,
    rel_path: &'r str,
) -> Result<(&'s Cow<'static, str>, &'r str), PluginAddError> {
    package_location_in(spec, &PluginOrigin::BuiltIn, rel_path)
}

/// [`package_location`] for a `spec` of `origin`: a built-in path keeps the
/// `plugins/<crate_dir>/` rule; a manifest-described one is already relative
/// to its package (discovery checked it stays inside), so it is taken whole,
/// refused only if it is not a plain relative path.
fn package_location_in<'s, 'r>(
    spec: &'s PluginSpec,
    origin: &PluginOrigin,
    rel_path: &'r str,
) -> Result<(&'s Cow<'static, str>, &'r str), PluginAddError> {
    let outside = || PluginAddError::NativePathOutsidePackage {
        plugin: spec.id.to_string(),
        rel_path: rel_path.to_string(),
        crate_dir: spec.crate_dir.to_string(),
    };
    let package = plugin_package(spec).ok_or_else(outside)?;
    let inside = match origin {
        PluginOrigin::BuiltIn => rel_path
            .strip_prefix("plugins/")
            .and_then(|rest| rest.strip_prefix(&*spec.crate_dir))
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|rest| !rest.is_empty()),
        PluginOrigin::Manifest { .. } => {
            let plain = !rel_path.is_empty()
                && !rel_path.contains('\\')
                && Path::new(rel_path)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)));
            plain.then_some(rel_path)
        }
    }
    .ok_or_else(outside)?;
    Ok((package, inside))
}

/// How an add writes a plugin dependency line the app lacks.
pub(super) enum DepForm {
    /// A built-in plugin: in the form of the app's own `frust` dependency.
    Facade(FrustDep),
    /// A manifest-described plugin: from its own package.
    Manifest {
        /// The directory cargo resolved the package to.
        package_dir: PathBuf,
        /// The version cargo resolved.
        version: String,
        /// How the package was resolved.
        source: DepSource,
    },
}

/// Where one `add_plugin` run resolves locations from: the project, the
/// plugin, how its dependency line is written, and the locator that answers
/// where a built-in plugin's package lives.
struct Resolution<'a> {
    locator: &'a dyn PackageLocator,
    project_root: &'a Path,
    spec: &'a PluginSpec,
    form: &'a DepForm,
}

impl<'a> Resolution<'a> {
    fn new(
        locator: &'a dyn PackageLocator,
        project_root: &'a Path,
        spec: &'a PluginSpec,
        form: &'a DepForm,
    ) -> Self {
        Self {
            locator,
            project_root,
            spec,
            form,
        }
    }

    /// The directory of the plugin's `package`. A manifest-described plugin
    /// carries it, so the locator is never asked. A built-in one is where
    /// the locator finds it; or — path mode only, when the locator cannot
    /// answer — the very path the project's Cargo line names for the plugin
    /// ([`plugin_dep_path`], made absolute against the project root and
    /// folded lexically), which is where cargo will look once the checkout
    /// is in place. Registry mode has no such fallback: the crate exists only
    /// where cargo unpacks it.
    fn package_dir(&self, package: &str) -> Result<PathBuf, PluginAddError> {
        let frust = match self.form {
            DepForm::Facade(frust) => frust,
            DepForm::Manifest { package_dir, .. } => return Ok(package_dir.clone()),
        };
        match self.locator.locate(self.project_root, package) {
            Ok(dir) => Ok(dir),
            Err(source) => match frust {
                FrustDep::Path { path, .. } => {
                    let dep = plugin_dep_path(path, &self.spec.crate_dir);
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
    /// `..` cannot climb). Only a built-in plugin requires one.
    fn sibling(&self, sibling: &str) -> Result<PathBuf, PluginAddError> {
        let DepForm::Facade(frust) = self.form else {
            return Err(PluginAddError::InvalidContribution(format!(
                "plugin `{}` is described by a package manifest, which cannot require a \
                 sibling checkout",
                self.spec.id
            )));
        };
        let package = frust.package();
        let facade = match self.locator.locate(self.project_root, package) {
            Ok(dir) => dir,
            Err(source) => match frust {
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
    if cargo_dep_present(doc, name, &PluginOrigin::BuiltIn) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let deps = doc
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
        // Unreachable in practice — `FrustDep::read` already found the
        // facade entry in `[dependencies]` — but avoid an `unwrap` at an
        // FFI-free core.
        .ok_or(PluginAddError::NoFrustDependency)?;
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

/// Where a manifest-described plugin's package was resolved.
struct ManifestDep<'a> {
    package_dir: &'a Path,
    version: &'a str,
    source: DepSource,
}

/// [`Contribution::CargoDep`] for a manifest-described plugin: an app that
/// already depends on the package (any table, any key) is left untouched;
/// otherwise `[dependencies]` gains `{ path = "<package dir, relative to the
/// app>" }` for a path package, or the resolved version for a crates.io one.
/// A package from any other source is
/// [`PluginAddError::DependencySourceNotWritable`].
fn apply_manifest_cargo_dep(
    doc: &mut DocumentMut,
    changed: &mut bool,
    name: &str,
    plugin: &str,
    project_root: &Path,
    dep: ManifestDep<'_>,
) -> Result<AddOutcome, PluginAddError> {
    if dependency_entry(doc, name, DepLookup::AnyTable).is_some() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let value = match dep.source {
        DepSource::Path => {
            let mut inline = InlineTable::new();
            inline.insert(
                "path",
                Value::from(relative_dep_path(project_root, dep.package_dir)),
            );
            Value::InlineTable(inline)
        }
        DepSource::Registry => Value::from(dep.version),
        DepSource::Other => {
            return Err(PluginAddError::DependencySourceNotWritable {
                plugin: plugin.to_string(),
                package: name.to_string(),
            });
        }
    };
    if doc.get("dependencies").is_none() {
        doc.insert("dependencies", toml_edit::table());
    }
    let deps = doc
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            PluginAddError::UnparseableCargoToml("`dependencies` is not a table".into())
        })?;
    deps.insert(name, Item::Value(value));
    *changed = true;
    Ok(AddOutcome::Applied)
}

/// `to` as a `path` dependency value written in `from`'s `Cargo.toml`: a
/// forward-slashed path relative to `from` (both canonicalized where they
/// exist), or `to` itself when the two share no root (another Windows
/// drive).
fn relative_dep_path(from: &Path, to: &Path) -> String {
    let resolve = |path: &Path| {
        host_path::canonicalize_simplified(path).unwrap_or_else(|_| normalize_lexically(path))
    };
    let (from, to) = (resolve(from), resolve(to));
    let from_parts: Vec<Component> = from.components().collect();
    let to_parts: Vec<Component> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return host_path::to_portable_string(&to);
    }
    let mut rel = PathBuf::new();
    for _ in common..from_parts.len() {
        rel.push("..");
    }
    for part in &to_parts[common..] {
        rel.push(part.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        ".".to_string()
    } else {
        host_path::to_portable_string(&rel)
    }
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
/// carry a feature. `lookup` says which entry is the plugin's (a
/// manifest-described plugin's may sit in a target table or be renamed).
fn apply_cargo_feature(
    doc: &mut DocumentMut,
    changed: &mut bool,
    name: &str,
    feature: &str,
    lookup: DepLookup,
) -> Result<AddOutcome, PluginAddError> {
    let no_such_dep = || PluginAddError::NoSuchCargoDep {
        name: name.to_string(),
        feature: feature.to_string(),
    };
    if cargo_feature_present(doc, name, feature, lookup) {
        return Ok(AddOutcome::AlreadyPresent);
    }

    let dep = dependency_entry_mut(doc, name, lookup).ok_or_else(no_such_dep)?;
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
    // A project without an `android/` directory has no manifest to edit, so
    // the permission is satisfied there.
    if !project_root.join(ANDROID_DIR_REL).is_dir() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let path = project_root.join(MANIFEST_REL);
    let src = read_required(&path, MANIFEST_REL)?;
    if manifest_permission_present(&src, permission) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let line = format!("    <uses-permission android:name=\"{permission}\" />\n");
    let out = insert_before_anchor(&src, MANIFEST_ANCHOR, &line, MANIFEST_REL)?;
    write_file(&path, MANIFEST_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Whether `AndroidManifest.xml` text `src` already names `permission`.
pub(super) fn manifest_permission_present(src: &str, permission: &str) -> bool {
    src.contains(&format!("android:name=\"{permission}\""))
}

fn apply_plist_entry(
    project_root: &Path,
    key: &str,
    value: &str,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    apply_plist_block(project_root, key, |key| {
        plist_entry_block(key, value, comment)
    })
}

/// [`Contribution::PlistArrayEntry`]: like [`apply_plist_entry`], with an
/// `<array>` of `<string>`s as the value. An existing key is never merged
/// into or reordered.
fn apply_plist_array_entry(
    project_root: &Path,
    key: &str,
    values: &[impl AsRef<str>],
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    apply_plist_block(project_root, key, |key| {
        plist_array_entry_block(key, values, comment)
    })
}

/// Shared body of the two Info.plist appliers: skip when the (escaped) key
/// is present, else insert the block before the root dict's close (the last
/// `</dict>`, immediately preceding `</plist>`).
fn apply_plist_block(
    project_root: &Path,
    key: &str,
    block: impl FnOnce(&str) -> String,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(PLIST_REL);
    let src = read_required(&path, PLIST_REL)?;
    if plist_key_present(&src, key) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let out = insert_before_anchor(&src, PLIST_ANCHOR, &block(&xml_escape(key)), PLIST_REL)?;
    write_file(&path, PLIST_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Whether plist text `src` already carries `<key>` (XML-escaped) `key`.
pub(super) fn plist_key_present(src: &str, key: &str) -> bool {
    src.contains(&format!("<key>{}</key>", xml_escape(key)))
}

/// The tab-indented comment / key / `<string>` lines of one
/// [`Contribution::PlistEntry`]. `escaped_key` is already XML-escaped.
fn plist_entry_block(escaped_key: &str, value: &str, comment: &str) -> String {
    format!(
        "\t<!-- {} -->\n\t<key>{escaped_key}</key>\n\t<string>{}</string>\n",
        xml_comment_safe(comment),
        xml_escape(value)
    )
}

/// The tab-indented comment / key / `<array>` lines of one
/// [`Contribution::PlistArrayEntry`]; no values gives `<array>` and
/// `</array>` on adjacent lines.
fn plist_array_entry_block(escaped_key: &str, values: &[impl AsRef<str>], comment: &str) -> String {
    let mut out = format!(
        "\t<!-- {} -->\n\t<key>{escaped_key}</key>\n\t<array>\n",
        xml_comment_safe(comment)
    );
    for value in values {
        out.push_str(&format!(
            "\t\t<string>{}</string>\n",
            xml_escape(value.as_ref())
        ));
    }
    out.push_str("\t</array>\n");
    out
}

/// `comment` with every `--` run collapsed to one `-`, so it cannot end or
/// malform the XML comment around it.
fn xml_comment_safe(comment: &str) -> String {
    let mut out = String::with_capacity(comment.len());
    let mut prev_dash = false;
    for ch in comment.chars() {
        if ch == '-' && prev_dash {
            continue;
        }
        prev_dash = ch == '-';
        out.push(ch);
    }
    out
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
    if scaffold_file_present(project_root, rel_path)? {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let path = project_root.join(safe_scaffold_rel_path(rel_path)?);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PluginAddError::Io {
            path: rel_path.to_string(),
            message: e.to_string(),
        })?;
    }
    write_file(&path, rel_path, contents)?;
    Ok(AddOutcome::Applied)
}

/// Whether the [`Contribution::ScaffoldFile`] at `rel_path` exists — by
/// presence alone; an unsafe `rel_path` is refused.
pub(super) fn scaffold_file_present(
    project_root: &Path,
    rel_path: &str,
) -> Result<bool, PluginAddError> {
    Ok(project_root
        .join(safe_scaffold_rel_path(rel_path)?)
        .exists())
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

    let settings_out = if settings_include_present(&settings_src, gradle_name) {
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

    let build_out = if app_dependency_present(&build_src, gradle_name) {
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

/// Whether `settings.gradle.kts` text `src` already includes `gradle_name`.
pub(super) fn settings_include_present(src: &str, gradle_name: &str) -> bool {
    src.contains(&format!("include(\"{gradle_name}\")"))
}

/// Whether `app/build.gradle.kts` text `src` already depends on `gradle_name`.
pub(super) fn app_dependency_present(src: &str, gradle_name: &str) -> bool {
    src.contains(&format!("project(\"{gradle_name}\")"))
}

/// Refuse a [`Contribution::MavenRepository`] that can never be applied: no
/// groups (a repository is never added unfiltered) or a url that is not
/// `http(s)`.
fn validate_maven_repository(url: &str, groups: &[impl AsRef<str>]) -> Result<(), PluginAddError> {
    if groups.is_empty() {
        return Err(PluginAddError::InvalidContribution(format!(
            "Maven repository `{url}` names no groups; a repository is never added unfiltered"
        )));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(PluginAddError::InvalidContribution(format!(
            "Maven repository url `{url}` is not an http(s) url"
        )));
    }
    Ok(())
}

/// The error for a settings file that predates the repositories marker,
/// quoting the exact line to add and where.
fn missing_repositories_marker() -> PluginAddError {
    PluginAddError::MalformedProjectFile(format!(
        "{SETTINGS_GRADLE_REL}: add the line `{REPOSITORIES_ANCHOR}` inside \
         `dependencyResolutionManagement {{ repositories {{ ... }} }}`, after `mavenCentral()`"
    ))
}

/// Escape `"` and `\` for a Kotlin string literal (`$` too, which would
/// start a template).
fn kotlin_string_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '"' | '\\' | '$') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The `exclusiveContent` block one [`Contribution::MavenRepository`] adds.
fn maven_repository_block(url: &str, groups: &[impl AsRef<str>], comment: &str) -> String {
    let comment = comment.replace(['\r', '\n'], " ");
    let mut block = format!(
        "        // {comment}\n\
         \x20       exclusiveContent {{\n\
         \x20           forRepository {{ maven {{ url = uri(\"{}\") }} }}\n\
         \x20           filter {{\n",
        kotlin_string_escape(url)
    );
    for group in groups {
        block.push_str(&format!(
            "                includeGroup(\"{}\")\n",
            kotlin_string_escape(group.as_ref())
        ));
    }
    block.push_str("            }\n        }\n");
    block
}

/// [`Contribution::MavenRepository`]: an `exclusiveContent` block at the
/// `// frust:plugin-repositories` marker, idempotent by url.
fn apply_maven_repository(
    project_root: &Path,
    url: &str,
    groups: &[impl AsRef<str>],
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    validate_maven_repository(url, groups)?;
    let settings_path = project_root.join(SETTINGS_GRADLE_REL);
    let src = read_required(&settings_path, SETTINGS_GRADLE_REL)?;
    if maven_repository_present(&src, url) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    if !src.contains(REPOSITORIES_ANCHOR) {
        return Err(missing_repositories_marker());
    }
    let out = insert_after_anchor_line(
        &src,
        REPOSITORIES_ANCHOR,
        &maven_repository_block(url, groups, comment),
        SETTINGS_GRADLE_REL,
    )?;
    write_file(&settings_path, SETTINGS_GRADLE_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Whether `settings.gradle.kts` text `src` already names repository `url`.
pub(super) fn maven_repository_present(src: &str, url: &str) -> bool {
    src.contains(&format!("uri(\"{}\")", kotlin_string_escape(url)))
}

/// The line [`missing_libcxx_shared_reader`] asks a project generated before
/// the template read [`LIBCXX_SHARED_PROPERTY`] to add to its
/// `cargoNdkBuild` command line, right after `add("ndk")`. It reads the
/// value as the template's `toBoolean()` and [`libcxx_shared_property_set`]
/// do: `true` in any letter case, surrounding whitespace ignored.
const LIBCXX_SHARED_READER_LINE: &str = "if ((project.findProperty(\"frust.cargoNdk.linkLibcxxShared\") as String?)?.trim().equals(\"true\", ignoreCase = true)) add(\"--link-libcxx-shared\")";

/// Whether `app/build.gradle.kts` text `src` reads [`LIBCXX_SHARED_PROPERTY`]
/// outside a comment — what makes setting the property do anything.
pub(super) fn libcxx_shared_reader_present(src: &str) -> bool {
    crate::android_build::tasks::reads_property(src, LIBCXX_SHARED_PROPERTY)
}

/// The error for a build script that predates [`LIBCXX_SHARED_PROPERTY`],
/// quoting the exact line to add and where.
pub(super) fn missing_libcxx_shared_reader() -> PluginAddError {
    PluginAddError::MalformedProjectFile(format!(
        "{APP_BUILD_GRADLE_REL}: the `cargoNdkBuild` task does not read \
         `{LIBCXX_SHARED_PROPERTY}`, so `libc++_shared.so` would not be packaged; add the line \
         `{LIBCXX_SHARED_READER_LINE}` right after `add(\"ndk\")` in its command line, then \
         re-run"
    ))
}

/// The range of the last line of `gradle.properties` text `src` that assigns
/// [`LIBCXX_SHARED_PROPERTY`] (the last assignment wins), from the key to the
/// line's ending (leading whitespace and the ending excluded), and the value
/// it assigns. Comment lines (`#`, `!`) are skipped;
/// the key ends at `=`, `:` or whitespace, as Java properties read it.
fn libcxx_shared_assignment(src: &str) -> Option<(Range<usize>, &str)> {
    let mut found = None;
    let mut start = 0;
    for line in src.split_inclusive('\n') {
        let end = start + line.trim_end_matches(['\r', '\n']).len();
        let text = &src[start..end];
        let code = text.trim_start();
        if !code.starts_with(['#', '!'])
            && let Some(rest) = code.strip_prefix(LIBCXX_SHARED_PROPERTY)
            && (rest.is_empty() || rest.starts_with(['=', ':', ' ', '\t']))
        {
            let rest = rest.trim_start_matches([' ', '\t']);
            let value = rest.strip_prefix(['=', ':']).unwrap_or(rest).trim();
            found = Some((end - code.len()..end, value));
        }
        start += line.len();
    }
    found
}

/// Whether `gradle.properties` text `src` sets [`LIBCXX_SHARED_PROPERTY`] to
/// a value the template's `toBoolean()` reads as true.
pub(super) fn libcxx_shared_property_set(src: &str) -> bool {
    libcxx_shared_assignment(src).is_some_and(|(_, value)| value.eq_ignore_ascii_case("true"))
}

/// Whether the project's `android/gradle.properties` sets
/// [`LIBCXX_SHARED_PROPERTY`], for a build that stages native libraries
/// outside the `cargoNdkBuild` task (the hot-patch fat build). A project
/// without the file, or one that cannot be read, does not set it.
pub(crate) fn libcxx_shared_requested(project_root: &Path) -> bool {
    std::fs::read_to_string(project_root.join(GRADLE_PROPERTIES_REL))
        .is_ok_and(|properties| libcxx_shared_property_set(&properties))
}

/// Whether the project packages `libc++_shared.so`: its build script reads
/// the property and its `gradle.properties` sets it. Both files are required.
/// A project without an `android/` directory has nothing to package, so the
/// contribution is satisfied there.
pub(super) fn libcxx_shared_present(project_root: &Path) -> Result<bool, PluginAddError> {
    if !project_root.join(ANDROID_DIR_REL).is_dir() {
        return Ok(true);
    }
    let build = read_required(
        &project_root.join(APP_BUILD_GRADLE_REL),
        APP_BUILD_GRADLE_REL,
    )?;
    let properties = read_required(
        &project_root.join(GRADLE_PROPERTIES_REL),
        GRADLE_PROPERTIES_REL,
    )?;
    Ok(libcxx_shared_reader_present(&build) && libcxx_shared_property_set(&properties))
}

/// [`Contribution::AndroidLibcxxShared`]: `frust.cargoNdk.linkLibcxxShared=true`
/// in `android/gradle.properties`, rewriting the line that sets it to another
/// value or appending one. Refused, with nothing written, when the build
/// script does not read the property.
fn apply_android_libcxx_shared(project_root: &Path) -> Result<AddOutcome, PluginAddError> {
    if !project_root.join(ANDROID_DIR_REL).is_dir() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let build = read_required(
        &project_root.join(APP_BUILD_GRADLE_REL),
        APP_BUILD_GRADLE_REL,
    )?;
    if !libcxx_shared_reader_present(&build) {
        return Err(missing_libcxx_shared_reader());
    }
    let path = project_root.join(GRADLE_PROPERTIES_REL);
    let src = read_required(&path, GRADLE_PROPERTIES_REL)?;
    if libcxx_shared_property_set(&src) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let line = format!("{LIBCXX_SHARED_PROPERTY}=true");
    let out = match libcxx_shared_assignment(&src) {
        Some((range, _)) => format!("{}{line}{}", &src[..range.start], &src[range.end..]),
        None => {
            let separator = if src.is_empty() || src.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            format!(
                "{src}{separator}# Added by `frust` Add Plugin: package the NDK's libc++_shared.so \
                 (see app/build.gradle.kts).\n{line}\n"
            )
        }
    };
    write_file(&path, GRADLE_PROPERTIES_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Where an app stands against a [`Contribution::AndroidThemeParent`] — the
/// one answer its applier and `pending.rs` both act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ThemeParent {
    /// Nothing to write: the app has no `android/` directory, its activity
    /// theme is itself an accepted Material theme
    /// ([`is_accepted_material_theme`]), or the activity uses `FrustAppTheme`
    /// and that style's parent is the requested one, an accepted Material
    /// theme, or an app-owned style that does not lead to a platform theme.
    Satisfied,
    /// The activity uses `FrustAppTheme` and the style has no parent, a
    /// platform parent, or an app-owned parent chain that ends in a platform
    /// theme: the whole [`THEMES_REL`] text with only `FrustAppTheme`'s own
    /// parent attribute rewritten.
    Rewrite(String),
    /// The activity theme is neither `FrustAppTheme` nor an accepted Material
    /// theme, or it is `FrustAppTheme` and [`THEMES_REL`] does not define
    /// the style — refused.
    NoAppTheme,
}

/// The Material theme families a plugin's native Material views run under.
const MATERIAL_THEME_FAMILIES: [&str; 3] = [
    "Theme.Material3",
    "Theme.Material3Expressive",
    "Theme.MaterialComponents",
];

/// How many app-owned parents [`style_ancestry`] follows before it gives up.
const STYLE_CHAIN_LIMIT: usize = 16;

/// Whether style name `name` (optionally `@style/`-prefixed) is an accepted
/// Material theme: one of [`MATERIAL_THEME_FAMILIES`], or one of them
/// followed by `.` and more.
pub(super) fn is_accepted_material_theme(name: &str) -> bool {
    let style = name.strip_prefix("@style/").unwrap_or(name);
    MATERIAL_THEME_FAMILIES.iter().any(|family| {
        style
            .strip_prefix(family)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

/// Whether style reference `name` is a platform theme (`android:…` or
/// `@android:style/…`), which Material views refuse.
fn is_platform_theme(name: &str) -> bool {
    name.starts_with("android:") || name.starts_with("@android:")
}

/// Where a parent chain of app-owned styles leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ancestry {
    /// An accepted Material theme.
    Material,
    /// A platform theme.
    Platform,
    /// A style the resources text does not define, a cycle, or a chain
    /// longer than [`STYLE_CHAIN_LIMIT`]: owned by the app, left alone.
    Unresolved,
}

/// The [`Ancestry`] of style reference `parent`, the parent of style `child`,
/// within resources text `src`: follows each app-owned style defined in `src`
/// to its parent (or, for a dotted name without a `parent` attribute, to its
/// implicit parent, the name before the last `.`) until an accepted Material
/// or a platform theme is reached.
fn style_ancestry(src: &str, child: &str, parent: &str) -> Ancestry {
    let mut seen = vec![child.to_string()];
    let mut current = parent.to_string();
    for _ in 0..STYLE_CHAIN_LIMIT {
        if is_accepted_material_theme(&current) {
            return Ancestry::Material;
        }
        if is_platform_theme(&current) {
            return Ancestry::Platform;
        }
        let name = current
            .strip_prefix("@style/")
            .unwrap_or(&current)
            .to_string();
        if seen.contains(&name) {
            return Ancestry::Unresolved;
        }
        current = match style_parent(src, &name) {
            None => return Ancestry::Unresolved,
            Some(Some(next)) => next.to_string(),
            Some(None) => match name.rsplit_once('.') {
                Some((implicit, _)) => implicit.to_string(),
                None => return Ancestry::Unresolved,
            },
        };
        seen.push(name);
    }
    Ancestry::Unresolved
}

/// The [`ThemeParent`] of the app at `project_root` for `parent`. Reads only.
/// The manifest decides first: its activity theme (the first `<activity>`'s
/// `android:theme`, else the `<application>`'s) must be an accepted Material
/// theme or `@style/FrustAppTheme`; only then does `FrustAppTheme`'s parent
/// in [`THEMES_REL`] matter. A missing [`THEMES_REL`] is an app without the
/// style; a missing `AndroidManifest.xml` (with `android/` present) fails as
/// every manifest contribution does. A `parent` that cannot be written into
/// an XML attribute is a registry bug, refused.
pub(super) fn android_theme_parent(
    project_root: &Path,
    parent: &str,
) -> Result<ThemeParent, PluginAddError> {
    if parent.is_empty()
        || parent.contains(['"', '\'', '<', '>', '&'])
        || parent.contains(char::is_whitespace)
    {
        return Err(PluginAddError::InvalidContribution(format!(
            "Android theme parent `{parent}` is not a style name"
        )));
    }
    if !project_root.join(ANDROID_DIR_REL).is_dir() {
        return Ok(ThemeParent::Satisfied);
    }
    let manifest = read_required(&project_root.join(MANIFEST_REL), MANIFEST_REL)?;
    match activity_theme(&manifest).as_deref() {
        Some(theme) if is_accepted_material_theme(theme) => return Ok(ThemeParent::Satisfied),
        Some(theme) if theme.strip_prefix("@style/") == Some(APP_THEME_STYLE) => {}
        _ => return Ok(ThemeParent::NoAppTheme),
    }
    let themes_path = project_root.join(THEMES_REL);
    if !themes_path.is_file() {
        return Ok(ThemeParent::NoAppTheme);
    }
    let src = read_required(&themes_path, THEMES_REL)?;
    let rewrite = match style_parent(&src, APP_THEME_STYLE) {
        None => return Ok(ThemeParent::NoAppTheme),
        Some(None) => true,
        Some(Some(current)) if current == parent => false,
        Some(Some(current)) => style_ancestry(&src, APP_THEME_STYLE, current) == Ancestry::Platform,
    };
    if !rewrite {
        return Ok(ThemeParent::Satisfied);
    }
    Ok(match set_style_parent(&src, APP_THEME_STYLE, parent) {
        Some(Some(out)) => ThemeParent::Rewrite(out),
        _ => ThemeParent::Satisfied,
    })
}

/// The refusal of a [`ThemeParent::NoAppTheme`] app, naming plugin `plugin`.
pub(super) fn android_app_theme_missing(plugin: &str, parent: &'static str) -> PluginAddError {
    PluginAddError::AndroidAppThemeMissing {
        plugin: plugin.into(),
        parent,
    }
}

/// [`Contribution::AndroidThemeParent`]: rewrite the style's parent when
/// [`android_theme_parent`] says so; nothing else in the file changes.
fn apply_android_theme_parent(
    project_root: &Path,
    plugin: &str,
    parent: &'static str,
) -> Result<AddOutcome, PluginAddError> {
    match android_theme_parent(project_root, parent)? {
        ThemeParent::Satisfied => Ok(AddOutcome::AlreadyPresent),
        ThemeParent::Rewrite(out) => {
            write_file(&project_root.join(THEMES_REL), THEMES_REL, &out)?;
            Ok(AddOutcome::Applied)
        }
        ThemeParent::NoAppTheme => Err(android_app_theme_missing(plugin, parent)),
    }
}

/// The parent of style `name` in resources text `src` set to `parent`:
/// `None` when no such `<style>` exists, `Some(None)` when its parent already
/// is `parent`, otherwise `Some(Some(text))` — `src` with only the parent
/// attribute's value replaced (or, for a style without one, a `parent`
/// attribute inserted after its `name`).
fn set_style_parent(src: &str, name: &str, parent: &str) -> Option<Option<String>> {
    for tag in xml_start_tags(src, "style") {
        let text = &src[tag.clone()];
        let Some(name_at) = xml_attr(text, "name") else {
            continue;
        };
        if &text[name_at.clone()] != name {
            continue;
        }
        let (at, insert) = match xml_attr(text, "parent") {
            Some(value) if &text[value.clone()] == parent => return Some(None),
            Some(value) => (
                tag.start + value.start..tag.start + value.end,
                parent.to_string(),
            ),
            // Past the closing quote of the `name` value.
            None => {
                let end = tag.start + name_at.end + 1;
                (end..end, format!(" parent=\"{parent}\""))
            }
        };
        let mut out = String::with_capacity(src.len() + insert.len());
        out.push_str(&src[..at.start]);
        out.push_str(&insert);
        out.push_str(&src[at.end..]);
        return Some(Some(out));
    }
    None
}

/// The parent of the first `<style>` named `name` in resources text `src`:
/// `None` when no such style exists, `Some(None)` when it has no `parent`
/// attribute.
fn style_parent<'s>(src: &'s str, name: &str) -> Option<Option<&'s str>> {
    xml_start_tags(src, "style").into_iter().find_map(|tag| {
        let text = &src[tag];
        let name_at = xml_attr(text, "name")?;
        (&text[name_at] == name).then(|| xml_attr(text, "parent").map(|value| &text[value]))
    })
}

/// The activity theme manifest text `src` names: the first `<activity>`'s
/// `android:theme`, or the `<application>`'s when the activity names none.
fn activity_theme(src: &str) -> Option<String> {
    let theme_of = |element: &str| {
        xml_start_tags(src, element)
            .into_iter()
            .next()
            .and_then(|tag| {
                let text = &src[tag];
                xml_attr(text, "android:theme").map(|value| text[value].to_string())
            })
    };
    theme_of("activity").or_else(|| theme_of("application"))
}

/// The byte ranges of every `<element …>` start tag in XML text `src`, `<`
/// through `>`, skipping comments; a `>` inside a quoted attribute value
/// does not end the tag. An unterminated tag or comment ends the scan.
fn xml_start_tags(src: &str, element: &str) -> Vec<Range<usize>> {
    let bytes = src.as_bytes();
    let mut tags = Vec::new();
    let mut at = 0;
    while let Some(offset) = src[at..].find('<') {
        let start = at + offset;
        if src[start..].starts_with("<!--") {
            let Some(close) = src[start + 4..].find("-->") else {
                break;
            };
            at = start + 4 + close + 3;
            continue;
        }
        let mut quote = None;
        let mut end = None;
        for (i, &b) in bytes.iter().enumerate().skip(start + 1) {
            match (quote, b) {
                (Some(q), _) if b == q => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') => quote = Some(b),
                (None, b'>') => {
                    end = Some(i + 1);
                    break;
                }
                (None, _) => {}
            }
        }
        let Some(end) = end else {
            break;
        };
        let after_name = src[start + 1..].strip_prefix(element);
        if after_name
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_whitespace() || c == '/' || c == '>')
        {
            tags.push(start..end);
        }
        at = end;
    }
    tags
}

/// The byte range, within start-tag text `tag` (`<name a="v" …>`), of
/// attribute `attr`'s value, quotes excluded.
fn xml_attr(tag: &str, attr: &str) -> Option<Range<usize>> {
    let bytes = tag.as_bytes();
    // Past `<` and the element name.
    let mut i = 1 + tag[1..].find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
    loop {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && !(bytes[i].is_ascii_whitespace() || matches!(bytes[i], b'=' | b'>' | b'/'))
        {
            i += 1;
        }
        if i == name_start {
            return None;
        }
        let name = &tag[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            // A bare attribute (not XML, but tolerated): no value.
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = *bytes.get(i).filter(|b| matches!(b, b'"' | b'\''))?;
        let value_start = i + 1;
        let value_end = value_start + tag[value_start..].find(quote as char)?;
        if name == attr {
            return Some(value_start..value_end);
        }
        i = value_end + 1;
    }
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

    let missing = swift_package_missing_sites(&src, package_name)?;
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

/// The app's root ignore file, where the machine-local `ios/<package>` link
/// of a manifest-described plugin is listed.
pub(super) const GITIGNORE_REL: &str = ".gitignore";

/// Whether ignore-file text `src` already covers the `ios/<package_name>`
/// link: a line equal to `/ios/<package_name>` or `ios/<package_name>`, or
/// `/ios/Frust*` for a package named `Frust...`. No general pattern matching.
///
/// The link is a symlink, so a line is read the way git reads it for a
/// non-directory entry: a trailing-slash form matches directories only and
/// does not count, leading whitespace is part of the pattern and does not
/// count, and only trailing spaces (and a CRLF line ending) are dropped.
pub(super) fn gitignore_covers_ios_link(src: &str, package_name: &str) -> bool {
    let exact = [
        format!("/ios/{package_name}"),
        format!("ios/{package_name}"),
    ];
    src.lines()
        .map(|line| line.trim_end_matches(' '))
        .any(|line| {
            exact.iter().any(|e| e == line)
                || (package_name.starts_with("Frust") && line == "/ios/Frust*")
        })
}

/// The report label of the `.gitignore` line for the `ios/<package_name>`
/// link, in an add report and a pending one alike.
pub(super) fn gitignore_link_label(package_name: &str) -> String {
    format!(".gitignore entry `/ios/{package_name}`")
}

/// Whether the app's root `.gitignore` exists and lacks an entry for the
/// `ios/<package_name>` link. An app without the file is never pending: it
/// may not be a git repository.
pub(super) fn gitignore_ios_link_missing(
    project_root: &Path,
    package_name: &str,
) -> Result<bool, PluginAddError> {
    let path = project_root.join(GITIGNORE_REL);
    if !path.is_file() {
        return Ok(false);
    }
    let src = read_required(&path, GITIGNORE_REL)?;
    Ok(!gitignore_covers_ios_link(&src, package_name))
}

/// Append `/ios/<package_name>` to the app's root `.gitignore` unless it
/// already covers the link or the file does not exist. The existing bytes
/// stay a prefix of the result; a missing final newline is supplied before
/// the new line.
fn apply_gitignore_ios_link(
    project_root: &Path,
    package_name: &str,
) -> Result<AddOutcome, PluginAddError> {
    if !gitignore_ios_link_missing(project_root, package_name)? {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let path = project_root.join(GITIGNORE_REL);
    let mut out = read_required(&path, GITIGNORE_REL)?;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("/ios/{package_name}\n"));
    write_file(&path, GITIGNORE_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// The pbxproj sites of Swift package `package_name` that pbxproj text `src`
/// lacks, in [`PBX_SITES`] order; a section a site lives in being absent is
/// [`PluginAddError::MalformedProjectFile`].
fn swift_package_missing_sites(
    src: &str,
    package_name: &str,
) -> Result<Vec<PbxSite>, PluginAddError> {
    let mut missing = Vec::new();
    for site in PBX_SITES {
        if !pbx_site_body(src, site)?.contains(&site.needle(package_name)) {
            missing.push(site);
        }
    }
    Ok(missing)
}

/// Whether pbxproj text `src` wires Swift package `package_name` at all six
/// sites.
pub(super) fn swift_package_present(src: &str, package_name: &str) -> Result<bool, PluginAddError> {
    Ok(swift_package_missing_sites(src, package_name)?.is_empty())
}

/// One link request an `OTHER_LDFLAGS` list carries: a framework
/// ([`Contribution::IosFramework`], the two entries `"-framework", <name>,`)
/// or a library ([`Contribution::IosLibrary`], the one entry `"-l<name>",`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkFlag<'n> {
    /// `-framework <name>`.
    Framework(&'n str),
    /// `-l<name>`.
    Library(&'n str),
}

impl LinkFlag<'_> {
    /// The entries appended to one list, each line indented by `indent`. A
    /// library entry is always quoted: `+` (as in `c++`) is not a character
    /// an unquoted pbxproj string may hold.
    fn entries(self, indent: &str) -> String {
        match self {
            LinkFlag::Framework(name) => format!("{indent}\"-framework\",\n{indent}{name},\n"),
            LinkFlag::Library(name) => format!("{indent}\"-l{name}\",\n"),
        }
    }

    /// The name a [`PluginAddError::PbxWiringNotVerified`] reports.
    fn label(self) -> String {
        match self {
            LinkFlag::Framework(name) => name.to_string(),
            LinkFlag::Library(name) => format!("-l{name}"),
        }
    }

    /// Whether an `OTHER_LDFLAGS` entry region already carries this flag,
    /// matched on trimmed lines (bare or quoted) so a pbxproj Xcode has since
    /// reformatted still reads as present.
    fn linked_in(self, body: &str) -> bool {
        match self {
            LinkFlag::Framework(name) => ldflags_link_framework(body, name),
            LinkFlag::Library(name) => {
                let bare = format!("-l{name},");
                let quoted = format!("\"-l{name}\",");
                body.lines()
                    .map(str::trim)
                    .any(|line| line == bare || line == quoted)
            }
        }
    }
}

/// [`Contribution::IosFramework`]: `-framework <name>` in every
/// `OTHER_LDFLAGS` list ([`apply_ios_link_flag`]).
fn apply_ios_framework(project_root: &Path, name: &str) -> Result<AddOutcome, PluginAddError> {
    apply_ios_link_flag(project_root, LinkFlag::Framework(name))
}

/// [`Contribution::IosLibrary`]: `-l<name>` in every `OTHER_LDFLAGS` list
/// ([`apply_ios_link_flag`]).
fn apply_ios_library(project_root: &Path, name: &str) -> Result<AddOutcome, PluginAddError> {
    apply_ios_link_flag(project_root, LinkFlag::Library(name))
}

/// Append `flag`'s entries to every `OTHER_LDFLAGS` list of the
/// `XCBuildConfiguration` section that does not already carry it — one list
/// per build configuration. Lists are edited back to front so the ranges
/// scanned from the original stay valid, the whole edit is built in memory,
/// re-scanned, and written once; a project with no `OTHER_LDFLAGS` list at
/// all is [`PluginAddError::MalformedProjectFile`]. A project without an
/// `ios/` directory has nothing to link, so the flag is satisfied there; one
/// with `ios/` but no pbxproj is [`PluginAddError::MissingProjectFile`].
fn apply_ios_link_flag(project_root: &Path, flag: LinkFlag) -> Result<AddOutcome, PluginAddError> {
    if !project_root.join(IOS_DIR_REL).is_dir() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let path = project_root.join(PBXPROJ_REL);
    let src = read_required(&path, PBXPROJ_REL)?;

    let lists = ldflags_lists(&src)?;
    let mut out = src.clone();
    let mut changed = false;
    for range in lists.into_iter().rev() {
        let body = &src[range.clone()];
        if flag.linked_in(body) {
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
        out.insert_str(range.end, &flag.entries(indent));
        changed = true;
    }
    if !changed {
        return Ok(AddOutcome::AlreadyPresent);
    }

    // All-or-nothing: every list must carry the flag before a byte is
    // written.
    if !link_flag_present(&out, flag)? {
        return Err(PluginAddError::PbxWiringNotVerified {
            file: PBXPROJ_REL.to_string(),
            package: flag.label(),
            site: LDFLAGS_KEY.to_string(),
        });
    }

    write_file(&path, PBXPROJ_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Whether the project at `project_root` carries `flag` in every
/// `OTHER_LDFLAGS` list — the predicate [`apply_ios_link_flag`] skips by,
/// `true` for a project without `ios/`.
pub(super) fn ios_link_flag_satisfied(
    project_root: &Path,
    flag: LinkFlag,
) -> Result<bool, PluginAddError> {
    if !project_root.join(IOS_DIR_REL).is_dir() {
        return Ok(true);
    }
    link_flag_present(
        &read_required(&project_root.join(PBXPROJ_REL), PBXPROJ_REL)?,
        flag,
    )
}

/// The build setting a framework or library is linked through.
const LDFLAGS_KEY: &str = "OTHER_LDFLAGS";

/// Every `OTHER_LDFLAGS` list of pbxproj text `src` (one per build
/// configuration); none at all is [`PluginAddError::MalformedProjectFile`].
fn ldflags_lists(src: &str) -> Result<Vec<Range<usize>>, PluginAddError> {
    let lists = pbx_list_ranges(src, "XCBuildConfiguration", LDFLAGS_KEY)?;
    if lists.is_empty() {
        return Err(PluginAddError::MalformedProjectFile(
            PBXPROJ_REL.to_string(),
        ));
    }
    Ok(lists)
}

/// Whether every `OTHER_LDFLAGS` list of pbxproj text `src` carries `flag`.
fn link_flag_present(src: &str, flag: LinkFlag) -> Result<bool, PluginAddError> {
    Ok(ldflags_lists(src)?
        .into_iter()
        .all(|range| flag.linked_in(&src[range])))
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

// ---------------------------------------------------------------------------
// `Contribution::IosEntitlement` — the entitlements file plus the Runner
// target's `CODE_SIGN_ENTITLEMENTS` build setting.
// ---------------------------------------------------------------------------

/// The build setting naming the file a target is signed with.
const CODE_SIGN_ENTITLEMENTS: &str = "CODE_SIGN_ENTITLEMENTS";

/// The entitlements file a generated app's Runner target signs with, as
/// `CODE_SIGN_ENTITLEMENTS` names it: relative to `ios/`, Xcode's `SRCROOT`.
const ENTITLEMENTS_SETTING: &str = "Runner/Runner.entitlements";

/// The directory every `SRCROOT`-relative setting resolves under.
const IOS_DIR_REL: &str = "ios";

/// The Runner target, by the `name` its `PBXNativeTarget` carries.
const RUNNER_TARGET: &str = "Runner";

/// The text of a newly created entitlements file, before its first key.
const EMPTY_ENTITLEMENTS: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
</dict>
</plist>
";

/// The writes one [`Contribution::IosEntitlement`] needs, computed entirely in
/// memory so [`add_spec_with`] can check it before anything is written.
#[derive(Debug)]
struct EntitlementPlan {
    /// Entitlements files to write, as `(project-relative path, contents)`.
    files: Vec<(String, String)>,
    /// The edited `project.pbxproj`, when a configuration lacked the setting.
    pbxproj: Option<String>,
}

impl EntitlementPlan {
    /// Whether nothing needs writing: every entitlements file the Runner
    /// configurations sign with has the key, and every configuration names
    /// one.
    fn is_noop(&self) -> bool {
        self.files.is_empty() && self.pbxproj.is_none()
    }
}

/// Whether entitlement `key` is fully in place: in every entitlements file
/// a Runner configuration signs with, whatever its value's type, and
/// `CODE_SIGN_ENTITLEMENTS` set in every Runner configuration (a flavored
/// project's included). Computed by [`plan_ios_entitlement`], never written.
pub(super) fn ios_entitlement_present(
    project_root: &Path,
    key: &str,
    value: &IosEntitlementValue,
    comment: &str,
) -> Result<bool, PluginAddError> {
    Ok(plan_ios_entitlement(project_root, key, value, comment, false)?.is_noop())
}

/// Request entitlement `key` on the Runner target: the key in the
/// entitlements file every Runner configuration signs with, and
/// `CODE_SIGN_ENTITLEMENTS` in every configuration lacking it (see
/// [`Contribution::IosEntitlement`]). The entitlements files are written
/// before the pbxproj, so the project never names a file not yet on disk.
/// Crate-visible for the scaffold, which requests the associated domains a
/// `frust create --deeplink-https-host` app needs the same way.
pub(crate) fn apply_ios_entitlement(
    project_root: &Path,
    key: &str,
    value: &IosEntitlementValue,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    let plan = plan_ios_entitlement(project_root, key, value, comment, false)?;
    write_entitlement_plan(project_root, plan)
}

/// Like [`apply_ios_entitlement`], but when an entitlements file already
/// carries `key` with a different value, that value is replaced in place
/// (every other key and comment is left as it is). For the scaffold only,
/// which is the documented single writer of its key; the plugin contribution
/// path keeps its skip-if-present rule through [`apply_ios_entitlement`].
pub(crate) fn replace_ios_entitlement(
    project_root: &Path,
    key: &str,
    value: &IosEntitlementValue,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    let plan = plan_ios_entitlement(project_root, key, value, comment, true)?;
    write_entitlement_plan(project_root, plan)
}

/// Writes `plan`: entitlements files first, then the pbxproj.
fn write_entitlement_plan(
    project_root: &Path,
    plan: EntitlementPlan,
) -> Result<AddOutcome, PluginAddError> {
    if plan.is_noop() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    for (rel, contents) in &plan.files {
        let path = project_root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| PluginAddError::Io {
                path: rel.clone(),
                message: e.to_string(),
            })?;
        }
        write_file(&path, rel, contents)?;
    }
    if let Some(pbxproj) = &plan.pbxproj {
        write_file(&project_root.join(PBXPROJ_REL), PBXPROJ_REL, pbxproj)?;
    }
    Ok(AddOutcome::Applied)
}

/// [`apply_ios_entitlement`]'s edits, without writing them.
///
/// Each Runner configuration signs with the file its own
/// `CODE_SIGN_ENTITLEMENTS` names; a configuration without one is given the
/// file the others already agree on, or [`ENTITLEMENTS_SETTING`] when none
/// (or more than one) is named. The key then goes into every file in that
/// set that lacks it. With `replace`, a file that has the key with another
/// value gets that value rewritten instead of being skipped.
fn plan_ios_entitlement(
    project_root: &Path,
    key: &str,
    value: &IosEntitlementValue,
    comment: &str,
    replace: bool,
) -> Result<EntitlementPlan, PluginAddError> {
    let src = read_required(&project_root.join(PBXPROJ_REL), PBXPROJ_REL)?;
    let configs = runner_target_configurations(&src)?;

    let mut named: Vec<&PbxSetting> = Vec::new();
    for setting in configs.iter().filter_map(|c| c.entitlements.as_ref()) {
        if !named.iter().any(|n| n.value == setting.value) {
            named.push(setting);
        }
    }
    let (default_raw, default_value) = match named.as_slice() {
        [only] => (only.raw.clone(), only.value.clone()),
        _ => (
            ENTITLEMENTS_SETTING.to_string(),
            ENTITLEMENTS_SETTING.to_string(),
        ),
    };

    // Configurations are edited back to front in *file* order (the list's
    // order need not match it) so every offset scanned from the original
    // stays valid.
    let mut missing: Vec<&PbxInsertion> = configs
        .iter()
        .filter(|c| c.entitlements.is_none())
        .map(|c| &c.insert)
        .collect();
    missing.sort_by_key(|insert| std::cmp::Reverse(insert.at));
    let pbx_changed = !missing.is_empty();
    let mut out = src.clone();
    for insert in missing {
        out.insert_str(
            insert.at,
            &insert.text(CODE_SIGN_ENTITLEMENTS, &default_raw),
        );
    }
    if pbx_changed {
        // All-or-nothing: every Runner configuration must carry the setting
        // before a byte is written.
        let verified = runner_target_configurations(&out)?;
        if verified.len() != configs.len() || verified.iter().any(|c| c.entitlements.is_none()) {
            return Err(PluginAddError::PbxWiringNotVerified {
                file: PBXPROJ_REL.to_string(),
                package: key.to_string(),
                site: CODE_SIGN_ENTITLEMENTS.to_string(),
            });
        }
    }

    let mut targets: Vec<String> = Vec::new();
    for config in &configs {
        let value = config
            .entitlements
            .as_ref()
            .map_or(default_value.as_str(), |s| s.value.as_str());
        if !targets.iter().any(|t| t == value) {
            targets.push(value.to_string());
        }
    }

    let presence = format!("<key>{}</key>", xml_escape(key));
    let block = entitlement_block(key, value, comment);
    let mut files = Vec::new();
    for setting in targets {
        let rel = entitlements_rel_path(&setting)?;
        let path = project_root.join(&rel);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => EMPTY_ENTITLEMENTS.to_string(),
            Err(e) => {
                return Err(PluginAddError::Io {
                    path: rel,
                    message: e.to_string(),
                });
            }
        };
        if text.contains(&presence) {
            if replace
                && let Some(edited) = replace_entitlement_value(&text, key, value, &rel)?
                && edited != text
            {
                files.push((rel, edited));
            }
            continue;
        }
        let edited = insert_before_anchor(&text, "</dict>", &block, &rel)?;
        files.push((rel, edited));
    }

    Ok(EntitlementPlan {
        files,
        pbxproj: pbx_changed.then_some(out),
    })
}

/// The tab-indented `<!-- comment -->` / `<key>` / value lines one
/// entitlement adds to the root `<dict>`, key and string values XML-escaped.
fn entitlement_block(key: &str, value: &IosEntitlementValue, comment: &str) -> String {
    format!(
        "\t<!-- {comment} -->\n\t<key>{}</key>\n{}",
        xml_escape(key),
        entitlement_value_xml(value)
    )
}

/// The tab-indented, newline-terminated plist value element(s) for `value`.
fn entitlement_value_xml(value: &IosEntitlementValue) -> String {
    match value {
        IosEntitlementValue::Bool(true) => "\t<true/>\n".to_string(),
        IosEntitlementValue::Bool(false) => "\t<false/>\n".to_string(),
        IosEntitlementValue::String(text) => format!("\t<string>{}</string>\n", xml_escape(text)),
        IosEntitlementValue::StringArray(items) if items.is_empty() => "\t<array/>\n".to_string(),
        IosEntitlementValue::StringArray(items) => {
            let mut out = "\t<array>\n".to_string();
            for item in items.iter() {
                out.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(item)));
            }
            out.push_str("\t</array>\n");
            out
        }
    }
}

/// `text` with the value following `<key>key</key>` replaced by `value`, or
/// `None` when the key is absent. The element after the key is located by
/// tag matching (nested elements of the same name included), so any prior
/// value type is replaced whole. A value that cannot be delimited is
/// [`PluginAddError::MalformedProjectFile`] rather than a guess.
fn replace_entitlement_value(
    text: &str,
    key: &str,
    value: &IosEntitlementValue,
    rel: &str,
) -> Result<Option<String>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(rel.to_string());
    let marker = format!("<key>{}</key>", xml_escape(key));
    let Some(key_at) = text.find(&marker) else {
        return Ok(None);
    };
    let after_key = key_at + marker.len();
    let rest = &text[after_key..];
    let start = after_key + (rest.len() - rest.trim_start().len());
    let tail = &text[start..];
    if !tail.starts_with('<') {
        return Err(malformed());
    }
    let tag_end = tail.find('>').ok_or_else(malformed)?;
    let open = &tail[1..tag_end];
    let end = if let Some(name) = open.strip_suffix('/') {
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(malformed());
        }
        start + tag_end + 1
    } else {
        let name = open.split_whitespace().next().ok_or_else(malformed)?;
        let open_tag = format!("<{name}");
        let close_tag = format!("</{name}>");
        let mut depth = 1usize;
        let mut at = tag_end + 1;
        loop {
            let next_close = tail[at..].find(&close_tag).ok_or_else(malformed)? + at;
            // An opening tag of the same name before that close nests.
            match tail[at..next_close].find(&open_tag) {
                Some(rel_open) => {
                    let open_at = at + rel_open;
                    let inner_end = tail[open_at..].find('>').ok_or_else(malformed)? + open_at;
                    if !tail[open_at..inner_end].ends_with('/') {
                        depth += 1;
                    }
                    at = inner_end + 1;
                }
                None => {
                    depth -= 1;
                    at = next_close + close_tag.len();
                    if depth == 0 {
                        break;
                    }
                }
            }
        }
        start + at
    };
    let replacement = entitlement_value_xml(value);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..after_key]);
    out.push('\n');
    out.push_str(replacement.trim_end_matches('\n'));
    out.push_str(&text[end..]);
    Ok(Some(out))
}

/// `text` with the three characters XML character data cannot carry
/// literally (`&`, `<`, `>`) replaced by their entities.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The project-relative path of the entitlements file a
/// `CODE_SIGN_ENTITLEMENTS` value names: relative to `ios/`, optionally
/// spelled through `$(SRCROOT)`/`$(PROJECT_DIR)`. A value naming another
/// build variable, an absolute path, or a `..` climb cannot be located
/// inside the project, so it is [`PluginAddError::MalformedProjectFile`]
/// rather than a guess.
fn entitlements_rel_path(setting: &str) -> Result<String, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let rel = [
        "$(SRCROOT)/",
        "${SRCROOT}/",
        "$(PROJECT_DIR)/",
        "${PROJECT_DIR}/",
    ]
    .iter()
    .find_map(|prefix| setting.strip_prefix(prefix))
    .unwrap_or(setting);
    let unsafe_path = rel.is_empty()
        || rel.contains('$')
        || rel.contains('\\')
        || rel.starts_with('/')
        || rel.contains(':')
        || rel.split('/').any(|part| part == "..");
    if unsafe_path {
        return Err(malformed());
    }
    Ok(format!("{IOS_DIR_REL}/{rel}"))
}

/// One build configuration of the Runner target.
#[derive(Debug)]
struct RunnerConfiguration {
    /// Its `CODE_SIGN_ENTITLEMENTS` setting, when it has one.
    entitlements: Option<PbxSetting>,
    /// Where a missing setting is inserted.
    insert: PbxInsertion,
}

/// A build-setting value: as written in the file, and unquoted.
#[derive(Debug)]
struct PbxSetting {
    raw: String,
    value: String,
}

/// Where a new `KEY = value;` statement goes inside a `buildSettings`
/// dictionary — sorted among its siblings, as Xcode writes them.
#[derive(Debug)]
struct PbxInsertion {
    /// Byte offset the statement is inserted at.
    at: usize,
    /// The indentation of its own line; `None` when the dictionary is written
    /// on one line, so the statement is inserted inline instead.
    indent: Option<String>,
}

impl PbxInsertion {
    fn text(&self, key: &str, raw_value: &str) -> String {
        match &self.indent {
            Some(indent) => format!("{indent}{key} = {raw_value};\n"),
            None => format!("{key} = {raw_value}; "),
        }
    }
}

/// Every build configuration of the Runner target, in configuration-list
/// order: the `PBXNativeTarget` named `Runner`, its `buildConfigurationList`,
/// that list's `buildConfigurations`, and each one's `buildSettings`. A
/// flavored project's extra `<Mode>-<Flavor>` configurations are found the
/// same way; the project-level list is never consulted. Any link missing is
/// [`PluginAddError::MalformedProjectFile`].
fn runner_target_configurations(src: &str) -> Result<Vec<RunnerConfiguration>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());

    let targets = pbx_objects(src, pbx_section_range(src, "PBXNativeTarget")?)?;
    let mut list_id = None;
    for target in &targets {
        let statements = pbx_statements(src, target.body.clone())?;
        if pbx_lookup(src, &statements, "name").as_deref() == Some(RUNNER_TARGET) {
            list_id = pbx_lookup(src, &statements, "buildConfigurationList");
            break;
        }
    }
    let list_id = list_id.ok_or_else(malformed)?;

    let lists = pbx_objects(src, pbx_section_range(src, "XCConfigurationList")?)?;
    let list = lists
        .iter()
        .find(|object| object.id == list_id)
        .ok_or_else(malformed)?;
    let statements = pbx_statements(src, list.body.clone())?;
    let entries = statements
        .iter()
        .find(|s| s.key == "buildConfigurations")
        .ok_or_else(malformed)?;
    let mut config_ids: Vec<String> = Vec::new();
    for id in pbx_strip_comments(&src[entries.value.clone()])
        .trim()
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(malformed)?
        .split(',')
        .map(|id| pbx_unquote(id.trim()))
    {
        // A configuration listed twice is still edited once.
        if !id.is_empty() && !config_ids.contains(&id) {
            config_ids.push(id);
        }
    }
    if config_ids.is_empty() {
        return Err(malformed());
    }

    let objects = pbx_objects(src, pbx_section_range(src, "XCBuildConfiguration")?)?;
    config_ids
        .iter()
        .map(|id| {
            let object = objects
                .iter()
                .find(|object| object.id == *id)
                .ok_or_else(malformed)?;
            let statements = pbx_statements(src, object.body.clone())?;
            let settings = statements
                .iter()
                .find(|s| s.key == "buildSettings")
                .ok_or_else(malformed)?;
            build_settings_configuration(src, settings.value.clone())
        })
        .collect()
}

/// One configuration's `buildSettings = { … }` value (`value` is the text
/// between its `=` and `;`), read for its `CODE_SIGN_ENTITLEMENTS` and the
/// sorted insertion point a missing one takes.
fn build_settings_configuration(
    src: &str,
    value: Range<usize>,
) -> Result<RunnerConfiguration, PluginAddError> {
    let body = pbx_dict_body(src, value)?;
    let (open, close) = (body.start - 1, body.end);
    let statements = pbx_statements(src, body)?;

    let entitlements = statements
        .iter()
        .find(|s| s.key == CODE_SIGN_ENTITLEMENTS)
        .map(|s| {
            let raw = pbx_strip_comments(&src[s.value.clone()]).trim().to_string();
            PbxSetting {
                value: pbx_unquote(&raw),
                raw,
            }
        });

    let insert = match statements
        .iter()
        .find(|s| s.key.as_str() > CODE_SIGN_ENTITLEMENTS)
    {
        Some(next) => before_statement(src, next.start),
        None => {
            let line_start = line_start_of(src, close);
            let closing_indent = &src[line_start..close];
            if line_start > open && closing_indent.trim().is_empty() {
                let indent = statements
                    .last()
                    .map(|s| &src[line_start_of(src, s.start)..s.start])
                    .filter(|indent| indent.trim().is_empty())
                    .map_or_else(|| format!("{closing_indent}\t"), str::to_string);
                PbxInsertion {
                    at: line_start,
                    indent: Some(indent),
                }
            } else {
                PbxInsertion {
                    at: close,
                    indent: None,
                }
            }
        }
    };
    Ok(RunnerConfiguration {
        entitlements,
        insert,
    })
}

/// The bytes between the braces of a statement value written `{ … }` (`value`
/// is the text between its `=` and `;`); any other shape is
/// [`PluginAddError::MalformedProjectFile`].
fn pbx_dict_body(src: &str, value: Range<usize>) -> Result<Range<usize>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let code = pbx_code_bytes(src, value);
    let open = code
        .iter()
        .find(|(_, byte)| !byte.is_ascii_whitespace())
        .filter(|(_, byte)| *byte == b'{')
        .map(|(at, _)| *at)
        .ok_or_else(malformed)?;
    let close = code
        .iter()
        .rev()
        .find(|(_, byte)| !byte.is_ascii_whitespace())
        .filter(|(_, byte)| *byte == b'}')
        .map(|(at, _)| *at)
        .ok_or_else(malformed)?;
    Ok(open + 1..close)
}

/// The insertion point ahead of the statement starting at `start`: its own
/// line when it begins one, else inline right before it.
fn before_statement(src: &str, start: usize) -> PbxInsertion {
    let line_start = line_start_of(src, start);
    let indent = &src[line_start..start];
    if indent.trim().is_empty() {
        PbxInsertion {
            at: line_start,
            indent: Some(indent.to_string()),
        }
    } else {
        PbxInsertion {
            at: start,
            indent: None,
        }
    }
}

/// The byte offset of the start of the line holding `at`.
fn line_start_of(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map_or(0, |nl| nl + 1)
}

/// One top-level object of a pbxproj section: its id and the bytes between
/// its braces.
#[derive(Debug)]
struct PbxObject {
    id: String,
    body: Range<usize>,
}

/// One `key = value;` statement of a pbxproj dictionary.
#[derive(Debug)]
struct PbxStatement {
    /// The key, unquoted (`"KEY[sdk=…]"` keeps its condition).
    key: String,
    /// The bytes between its `=` and `;`.
    value: Range<usize>,
    /// Where the statement's first byte is.
    start: usize,
}

/// The `(offset, byte)` pairs of `range` that are pbxproj syntax — outside
/// every quoted string and `/* … */` comment, so a brace, `=` or `;` inside
/// a shell script or an object's comment is never taken for structure.
fn pbx_code_bytes(src: &str, range: Range<usize>) -> Vec<(usize, u8)> {
    let bytes = src.as_bytes();
    let end = range.end;
    let mut out = Vec::new();
    let mut i = range.start;
    while i < end {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < end && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'/' if i + 1 < end && bytes[i + 1] == b'*' => {
                i = src[i + 2..end]
                    .find("*/")
                    .map_or(end, |found| i + 2 + found + 2);
            }
            byte => {
                out.push((i, byte));
                i += 1;
            }
        }
    }
    out
}

/// Every top-level `<id> … = { … };` object in `section`, in file order. An
/// unbalanced brace is [`PluginAddError::MalformedProjectFile`].
fn pbx_objects(src: &str, section: Range<usize>) -> Result<Vec<PbxObject>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(String, usize)> = None;
    for (at, byte) in pbx_code_bytes(src, section.clone()) {
        match byte {
            b'{' => {
                if depth == 0 {
                    let line_start = line_start_of(src, at).max(section.start);
                    let id = src[line_start..at]
                        .split_whitespace()
                        .next()
                        .map(pbx_unquote)
                        .unwrap_or_default();
                    open = Some((id, at + 1));
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1).ok_or_else(malformed)?;
                if depth == 0 {
                    let (id, start) = open.take().ok_or_else(malformed)?;
                    objects.push(PbxObject {
                        id,
                        body: start..at,
                    });
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(malformed());
    }
    Ok(objects)
}

/// The `key = value;` statements directly inside the dictionary body `body`
/// (nested dictionaries and lists stay inside their statement's value). A
/// statement without `=` or `;`, or an unbalanced bracket, is
/// [`PluginAddError::MalformedProjectFile`].
fn pbx_statements(src: &str, body: Range<usize>) -> Result<Vec<PbxStatement>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let mut statements = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut equals: Option<usize> = None;
    for (at, byte) in pbx_code_bytes(src, body) {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let first = *start.get_or_insert(at);
        match byte {
            b'{' | b'(' => depth += 1,
            b'}' | b')' => depth = depth.checked_sub(1).ok_or_else(malformed)?,
            b'=' if depth == 0 && equals.is_none() => equals = Some(at),
            b';' if depth == 0 => {
                let eq = equals.take().ok_or_else(malformed)?;
                statements.push(PbxStatement {
                    key: pbx_unquote(pbx_strip_comments(&src[first..eq]).trim()),
                    value: eq + 1..at,
                    start: first,
                });
                start = None;
            }
            _ => {}
        }
    }
    if start.is_some() || depth != 0 {
        return Err(malformed());
    }
    Ok(statements)
}

/// The unquoted, comment-free value of the statement keyed `key`.
fn pbx_lookup(src: &str, statements: &[PbxStatement], key: &str) -> Option<String> {
    statements
        .iter()
        .find(|s| s.key == key)
        .map(|s| pbx_unquote(pbx_strip_comments(&src[s.value.clone()]).trim()))
}

/// `text` without its `/* … */` comments (a comment-like run inside a quoted
/// string is kept).
fn pbx_strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut kept_from = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&text[kept_from..i]);
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |found| i + 2 + found + 2);
                kept_from = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[kept_from.min(text.len())..]);
    out
}

/// A pbxproj token without its surrounding quotes and `\"`/`\\` escapes; a
/// bare token is returned as is.
fn pbx_unquote(token: &str) -> String {
    match token
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => token.to_string(),
    }
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

pub(super) fn read_required(path: &Path, rel: &str) -> Result<String, PluginAddError> {
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
    use crate::plugin::registry::{find_plugin, known_plugins};
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
                    Some(name) => {
                        stub.with(&**name, checkout.join("plugins").join(&*spec.crate_dir))
                    }
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
        // Folded lexically: `<frust>/../../plugins` is `/nonexistent/plugins`.
        let plugins = FRUST_PATH.strip_suffix("/frust/checkout").unwrap();
        assert!(
            cargo.contains(&format!("{plugins}/plugins/secure-storage\"")),
            "{cargo}"
        );
        assert!(!cargo.contains("/../"), "{cargo}");

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

        // The Cargo dep path is relative like the `frust` one, folded.
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        assert!(
            cargo.contains(
                "frust-secure-storage = { path = \"../checkout/plugins/secure-storage\" }"
            ),
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

    /// The facade is found by package name, so an app keying it `frust-ui`
    /// (the crates.io name) or any rename still has a facade dependency;
    /// a `frust` key on some other package is not the facade.
    #[test]
    fn frust_dep_finds_the_facade_under_any_key() {
        for (manifest, expected) in [
            (
                "[dependencies]\nfrust-ui = \"0.6.0\"\n",
                Some(FrustDep::Registry {
                    version: "0.6.0".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                "[dependencies]\nfrust-ui = { path = \"../f/crates/frust\" }\n",
                Some(FrustDep::Path {
                    path: "../f/crates/frust".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                "[dependencies]\nlog = \"0.4\"\nui = { package = \"frust-ui\", version = \"0.6\" }\n",
                Some(FrustDep::Registry {
                    version: "0.6".into(),
                    package: "frust-ui".into(),
                }),
            ),
            (
                "[dependencies]\nfrust = { package = \"not-the-facade\", version = \"1\" }\n",
                None,
            ),
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
            cargo.contains("frust-iap = { path = \"/nonexistent/plugins/iap\" }"),
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
            let contributions = spec.base.iter().chain(
                spec.optional_features
                    .iter()
                    .flat_map(|f| f.contributions.iter()),
            );
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
            id: Cow::Borrowed("stray"),
            summary: Cow::Borrowed("synthetic, test-only"),
            crate_dir: Cow::Borrowed("stray"),
            base: Cow::Borrowed(&[
                Contribution::CargoDep {
                    name: Cow::Borrowed("frust-stray"),
                },
                Contribution::GradleModule {
                    gradle_name: Cow::Borrowed(":frust-stray"),
                    rel_path: Cow::Borrowed("plugins/other/platform/android"),
                },
            ]),
            optional_features: Cow::Borrowed(&[]),
            requires_sibling: None,
            ..PluginSpec::DEFAULT
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

        let outcome = apply_cargo_feature(
            &mut doc,
            &mut changed,
            "frust-secure-storage",
            "biometric",
            DepLookup::Dependencies,
        )
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

        let first = apply_cargo_feature(
            &mut doc,
            &mut changed,
            "frust-secure-storage",
            "biometric",
            DepLookup::Dependencies,
        )
        .unwrap();
        assert_eq!(first, AddOutcome::Applied);
        let after_first = doc.to_string();

        let second = apply_cargo_feature(
            &mut doc,
            &mut changed,
            "frust-secure-storage",
            "biometric",
            DepLookup::Dependencies,
        )
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

        let err = apply_cargo_feature(
            &mut doc,
            &mut changed,
            "frust-not-a-real-dep",
            "biometric",
            DepLookup::Dependencies,
        )
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
        let frust = DepForm::Facade(path_dep("/nonexistent/frust/checkout"));
        let locator = StubLocator::new();
        let resolution = Resolution::new(&locator, &root, &spec, &frust);

        apply_contribution(
            &Contribution::CargoDep {
                name: Cow::Borrowed("frust-secure-storage"),
            },
            &resolution,
            &mut doc,
            &mut changed,
        )
        .unwrap();

        let outcome = apply_contribution(
            &Contribution::CargoFeature {
                name: Cow::Borrowed("frust-secure-storage"),
                feature: Cow::Borrowed("biometric"),
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
        let frust = DepForm::Facade(path_dep("/nonexistent/frust/checkout"));
        let locator = StubLocator::new();
        let resolution = Resolution::new(&locator, &root, &spec, &frust);
        let contribution = Contribution::ScaffoldFile {
            rel_path: SCAFFOLD_REL.into(),
            contents: SCAFFOLD_CONTENTS.into(),
            comment: Cow::Borrowed("test-only scaffold file"),
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
        let frust = DepForm::Facade(path_dep("/nonexistent/frust/checkout"));
        let locator = StubLocator::new();
        let resolution = Resolution::new(&locator, &root, &spec, &frust);
        let before = snapshot_tree(&root);

        for contribution in [
            Contribution::MacosPlistEntry {
                key: Cow::Borrowed("NSSupportsSuddenTermination"),
                value: Cow::Borrowed("NO"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::MacosEntitlement {
                key: Cow::Borrowed("com.apple.security.network.client"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::LinuxDesktopEntry {
                key: Cow::Borrowed("Categories"),
                value: Cow::Borrowed("Utility;"),
                comment: Cow::Borrowed("test"),
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
        id: Cow::Borrowed("desktop-test-plugin"),
        summary: Cow::Borrowed("synthetic, test-only"),
        crate_dir: Cow::Borrowed("desktop-test-plugin"),
        base: Cow::Borrowed(&[
            Contribution::CargoDep {
                name: Cow::Borrowed("frust-desktop-test-plugin"),
            },
            Contribution::MacosPlistEntry {
                key: Cow::Borrowed("NSSupportsSuddenTermination"),
                value: Cow::Borrowed("NO"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::MacosEntitlement {
                key: Cow::Borrowed("com.apple.security.network.client"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::LinuxDesktopEntry {
                key: Cow::Borrowed("Categories"),
                value: Cow::Borrowed("Utility;"),
                comment: Cow::Borrowed("test"),
            },
        ]),
        optional_features: Cow::Borrowed(&[]),
        requires_sibling: None,
        ..PluginSpec::DEFAULT
    };

    const MOBILE_ONLY_PLUGIN: PluginSpec = PluginSpec {
        id: Cow::Borrowed("mobile-only-test-plugin"),
        summary: Cow::Borrowed("synthetic, test-only"),
        crate_dir: Cow::Borrowed("mobile-only-test-plugin"),
        base: Cow::Borrowed(&[
            Contribution::CargoDep {
                name: Cow::Borrowed("frust-mobile-only-test-plugin"),
            },
            Contribution::PlistEntry {
                key: Cow::Borrowed("NSCameraUsageDescription"),
                value: Cow::Borrowed("test"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::ManifestPermission {
                permission: Cow::Borrowed("android.permission.CAMERA"),
            },
        ]),
        optional_features: Cow::Borrowed(&[]),
        requires_sibling: None,
        ..PluginSpec::DEFAULT
    };

    const MIXED_PLUGIN: PluginSpec = PluginSpec {
        id: Cow::Borrowed("mixed-test-plugin"),
        summary: Cow::Borrowed("synthetic, test-only"),
        crate_dir: Cow::Borrowed("mixed-test-plugin"),
        base: Cow::Borrowed(&[
            Contribution::CargoDep {
                name: Cow::Borrowed("frust-mixed-test-plugin"),
            },
            Contribution::PlistEntry {
                key: Cow::Borrowed("NSCameraUsageDescription"),
                value: Cow::Borrowed("test"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::ManifestPermission {
                permission: Cow::Borrowed("android.permission.CAMERA"),
            },
            Contribution::MacosPlistEntry {
                key: Cow::Borrowed("NSSupportsSuddenTermination"),
                value: Cow::Borrowed("NO"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::MacosEntitlement {
                key: Cow::Borrowed("com.apple.security.network.client"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::LinuxDesktopEntry {
                key: Cow::Borrowed("Categories"),
                value: Cow::Borrowed("Utility;"),
                comment: Cow::Borrowed("test"),
            },
        ]),
        optional_features: Cow::Borrowed(&[]),
        requires_sibling: None,
        ..PluginSpec::DEFAULT
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
        assert!(
            matches!(&result[0].contribution, Contribution::MacosPlistEntry {
                key,
                ..
            } if key == "NSSupportsSuddenTermination")
        );
        assert!(
            matches!(&result[1].contribution, Contribution::MacosEntitlement {
                key,
                ..
            } if key == "com.apple.security.network.client")
        );
        assert!(
            matches!(&result[2].contribution, Contribution::LinuxDesktopEntry {
                key,
                ..
            } if key == "Categories")
        );

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
            cargo.contains("\"vendor/plugins/clean-signals-frust\""),
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

    // -----------------------------------------------------------------------
    // `Contribution::IosEntitlement`
    //
    // The whole-add tests drive `add_spec_with` with a synthetic spec — no
    // registry entry uses the variant — and the per-file ones call
    // `apply_ios_entitlement` directly.
    // -----------------------------------------------------------------------

    /// The entitlements file a fresh scaffold's Runner target is pointed at.
    const ENTITLEMENTS_REL: &str = "ios/Runner/Runner.entitlements";

    /// The build-setting line the applier adds.
    const ENTITLEMENTS_LINE: &str = "CODE_SIGN_ENTITLEMENTS = Runner/Runner.entitlements;";

    /// The template's Runner target configurations (Debug, Release, Profile)
    /// and its project-level ones, which must never be touched.
    const RUNNER_CONFIG_IDS: [&str; 3] = [
        "ABCDABCDABCDABCDABCD000E",
        "ABCDABCDABCDABCDABCD000F",
        "ABCDABCDABCDABCDABCD0026",
    ];
    const PROJECT_CONFIG_IDS: [&str; 3] = [
        "ABCDABCDABCDABCDABCD000C",
        "ABCDABCDABCDABCDABCD000D",
        "ABCDABCDABCDABCDABCD0025",
    ];

    const TEST_ENTITLEMENT: &str = "com.apple.developer.test";

    const ENTITLEMENT_PLUGIN: PluginSpec = PluginSpec {
        id: Cow::Borrowed("entitlement-test-plugin"),
        summary: Cow::Borrowed("synthetic, test-only"),
        crate_dir: Cow::Borrowed("entitlement-test-plugin"),
        base: Cow::Borrowed(&[
            Contribution::CargoDep {
                name: Cow::Borrowed("frust-entitlement-test-plugin"),
            },
            Contribution::PlistEntry {
                key: Cow::Borrowed("NSUserTrackingUsageDescription"),
                value: Cow::Borrowed("test"),
                comment: Cow::Borrowed("test"),
            },
            Contribution::IosEntitlement {
                key: Cow::Borrowed(TEST_ENTITLEMENT),
                value: IosEntitlementValue::Bool(true),
                comment: Cow::Borrowed("Test entitlement"),
            },
        ]),
        optional_features: Cow::Borrowed(&[]),
        requires_sibling: None,
        ..PluginSpec::DEFAULT
    };

    fn add_test_spec(root: &Path, spec: &PluginSpec) -> Result<AddReport, PluginAddError> {
        add_spec_with(&checkout_locator(root), root, spec, &[])
    }

    fn apply_test_entitlement(root: &Path) -> Result<AddOutcome, PluginAddError> {
        apply_ios_entitlement(
            root,
            TEST_ENTITLEMENT,
            &IosEntitlementValue::Bool(true),
            "Test entitlement",
        )
    }

    fn read_entitlements(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    /// The body of the `XCBuildConfiguration` object `id`.
    fn build_configuration_body(pbxproj: &str, id: &str) -> String {
        let section = pbx_section_range(pbxproj, "XCBuildConfiguration").unwrap();
        let object = pbx_objects(pbxproj, section)
            .unwrap()
            .into_iter()
            .find(|object| object.id == id)
            .unwrap_or_else(|| panic!("no build configuration `{id}`:\n{pbxproj}"));
        pbxproj[object.body].to_string()
    }

    /// `pbxproj` with a copy of the Runner target's Release configuration
    /// added as `name`, listed in the Runner target's configuration list —
    /// what a natively declared flavor's `<Mode>-<Flavor>` configuration
    /// looks like.
    fn with_runner_configuration(pbxproj: &str, id: &str, name: &str) -> String {
        let marker = "\t\tABCDABCDABCDABCDABCD000F /* Release */ = {\n";
        let close = "\n\t\t};\n";
        let start = pbxproj.find(marker).unwrap();
        let end = start + pbxproj[start..].find(close).unwrap() + close.len();
        let copy = pbxproj[start..end]
            .replace(
                "ABCDABCDABCDABCDABCD000F /* Release */",
                &format!("{id} /* {name} */"),
            )
            .replace("name = Release;", &format!("name = \"{name}\";"));
        let mut out = pbxproj.to_string();
        out.insert_str(end, &copy);
        let entry = "\t\t\t\tABCDABCDABCDABCDABCD0026 /* Profile */,\n";
        let at = out.find(entry).unwrap() + entry.len();
        out.insert_str(at, &format!("\t\t\t\t{id} /* {name} */,\n"));
        out
    }

    #[test]
    fn ios_entitlement_creates_the_file_and_sets_every_runner_configuration() {
        let root = scaffold_project("ios-entitlement-fresh");
        assert!(!root.join(ENTITLEMENTS_REL).exists());

        let report = add_test_spec(&root, &ENTITLEMENT_PLUGIN).unwrap();
        assert_eq!(report.plugin_id, "entitlement-test-plugin");
        assert_eq!(report.items.len(), 3, "{report:?}");
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );
        assert_eq!(
            report.items[2].description,
            format!("iOS entitlement `{TEST_ENTITLEMENT}` (`CODE_SIGN_ENTITLEMENTS`)")
        );

        // The file: a plist holding the key, created from nothing.
        let entitlements = read_entitlements(&root, ENTITLEMENTS_REL);
        assert!(entitlements.starts_with("<?xml"), "{entitlements}");
        assert!(
            entitlements.contains(&format!(
                "\t<!-- Test entitlement -->\n\t<key>{TEST_ENTITLEMENT}</key>\n\t<true/>\n</dict>\n</plist>\n"
            )),
            "{entitlements}"
        );

        // The setting: once in each Runner target configuration, sorted
        // where Xcode puts it, and in no project-level configuration.
        let pbxproj = read_pbxproj(&root);
        for id in RUNNER_CONFIG_IDS {
            let body = build_configuration_body(&pbxproj, id);
            assert_eq!(body.matches(ENTITLEMENTS_LINE).count(), 1, "{id}:\n{body}");
        }
        for id in PROJECT_CONFIG_IDS {
            let body = build_configuration_body(&pbxproj, id);
            assert!(!body.contains(CODE_SIGN_ENTITLEMENTS), "{id}:\n{body}");
        }
        assert_eq!(pbxproj.matches(CODE_SIGN_ENTITLEMENTS).count(), 3);
        assert_eq!(
            pbxproj
                .matches(
                    "\t\t\t\tCODE_SIGNING_REQUIRED = NO;\n\
                     \t\t\t\tCODE_SIGN_ENTITLEMENTS = Runner/Runner.entitlements;\n\
                     \t\t\t\tCODE_SIGN_STYLE = Automatic;\n"
                )
                .count(),
            3,
            "{pbxproj}"
        );
        assert_pbxproj_well_formed(&pbxproj);

        // A second add changes no byte.
        let after_first = snapshot_tree(&root);
        let again = add_test_spec(&root, &ENTITLEMENT_PLUGIN).unwrap();
        assert!(
            again
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{again:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second add must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ios_entitlement_values_are_plist_elements_with_escaped_text() {
        let root = scaffold_project("ios-entitlement-values");

        let applied = [
            (
                "aps-environment",
                IosEntitlementValue::String(Cow::Borrowed("dev&<x>")),
                "Push",
            ),
            (
                "com.apple.security.application-groups",
                IosEntitlementValue::StringArray(Cow::Borrowed(&[
                    Cow::Borrowed("group.a&b"),
                    Cow::Borrowed("group.<c>"),
                ])),
                "Groups",
            ),
            ("odd&<key>", IosEntitlementValue::Bool(false), "Odd key"),
        ];
        for (key, value, comment) in applied {
            assert_eq!(
                apply_ios_entitlement(&root, key, &value, comment).unwrap(),
                AddOutcome::Applied,
                "{key}"
            );
        }

        let entitlements = read_entitlements(&root, ENTITLEMENTS_REL);
        assert!(
            entitlements
                .contains("\t<key>aps-environment</key>\n\t<string>dev&amp;&lt;x&gt;</string>\n"),
            "{entitlements}"
        );
        assert!(
            entitlements.contains(
                "\t<key>com.apple.security.application-groups</key>\n\
                 \t<array>\n\
                 \t\t<string>group.a&amp;b</string>\n\
                 \t\t<string>group.&lt;c&gt;</string>\n\
                 \t</array>\n"
            ),
            "{entitlements}"
        );
        assert!(
            entitlements.contains("\t<key>odd&amp;&lt;key&gt;</key>\n\t<false/>\n"),
            "{entitlements}"
        );
        assert!(!entitlements.contains("&<"), "{entitlements}");

        // The escaped key is what the presence check looks for, too.
        assert_eq!(
            apply_ios_entitlement(
                &root,
                "odd&<key>",
                &IosEntitlementValue::Bool(false),
                "Odd key"
            )
            .unwrap(),
            AddOutcome::AlreadyPresent
        );
        // The setting was added once, by the first entitlement.
        assert_eq!(read_pbxproj(&root).matches(ENTITLEMENTS_LINE).count(), 3);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_string_array_entitlement_is_an_empty_array() {
        assert_eq!(
            entitlement_block(
                "k",
                &IosEntitlementValue::StringArray(Cow::Borrowed(&[])),
                "c"
            ),
            "\t<!-- c -->\n\t<key>k</key>\n\t<array/>\n"
        );
    }

    #[test]
    fn ios_entitlement_keeps_an_existing_files_other_keys_byte_for_byte() {
        let root = scaffold_project("ios-entitlement-existing-keys");
        let existing = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
            \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
            <plist version=\"1.0\">\n\
            <dict>\n\
            \x20   <key>keychain-access-groups</key>\n\
            \x20   <array>\n\
            \x20       <string>$(AppIdentifierPrefix)dev.f0x.myApp</string>\n\
            \x20   </array>\n\
            \x20   <key>com.apple.developer.associated-domains</key>\n\
            \x20   <array/>\n\
            </dict>\n\
            </plist>\n";
        fs::write(root.join(ENTITLEMENTS_REL), existing).unwrap();

        assert_eq!(apply_test_entitlement(&root).unwrap(), AddOutcome::Applied);

        let block = entitlement_block(
            TEST_ENTITLEMENT,
            &IosEntitlementValue::Bool(true),
            "Test entitlement",
        );
        let entitlements = read_entitlements(&root, ENTITLEMENTS_REL);
        assert_eq!(entitlements.matches(&block).count(), 1, "{entitlements}");
        assert_eq!(entitlements.replacen(&block, "", 1), existing);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ios_entitlement_reaches_a_flavored_fourth_runner_configuration() {
        const PAID: &str = "ABCDABCDABCDABCDABCD00F0";
        let root = scaffold_project("ios-entitlement-flavored");
        let flavored = with_runner_configuration(&read_pbxproj(&root), PAID, "Release-Paid");
        assert_pbxproj_well_formed(&flavored);
        fs::write(root.join(PBXPROJ_REL), &flavored).unwrap();

        assert_eq!(apply_test_entitlement(&root).unwrap(), AddOutcome::Applied);

        let pbxproj = read_pbxproj(&root);
        assert_eq!(pbxproj.matches(ENTITLEMENTS_LINE).count(), 4, "{pbxproj}");
        for id in RUNNER_CONFIG_IDS.into_iter().chain([PAID]) {
            let body = build_configuration_body(&pbxproj, id);
            assert_eq!(body.matches(ENTITLEMENTS_LINE).count(), 1, "{id}:\n{body}");
        }
        assert!(
            build_configuration_body(&pbxproj, PAID).contains("name = \"Release-Paid\";"),
            "{pbxproj}"
        );
        assert_pbxproj_well_formed(&pbxproj);

        let _ = fs::remove_dir_all(&root);
    }

    /// A pbxproj whose Runner configurations cannot be resolved refuses the
    /// whole add before its first write: not the entitlements file, nor the
    /// pbxproj, nor the `Cargo.toml` dependency and `Info.plist` key that
    /// would otherwise have landed first.
    #[test]
    fn an_unresolvable_runner_target_refuses_the_add_before_any_write() {
        let manglings: [(&str, &str); 2] = [
            (
                "\t\t\tbuildConfigurationList = ABCDABCDABCDABCDABCD000B \
                 /* Build configuration list for PBXNativeTarget \"Runner\" */;\n",
                "",
            ),
            (
                "\t\t\t\tABCDABCDABCDABCDABCD000E /* Debug */,\n\
                 \t\t\t\tABCDABCDABCDABCDABCD000F /* Release */,\n\
                 \t\t\t\tABCDABCDABCDABCDABCD0026 /* Profile */,\n",
                "",
            ),
        ];
        for (i, (from, to)) in manglings.into_iter().enumerate() {
            let root = scaffold_project(&format!("ios-entitlement-unresolvable-{i}"));
            let pbxproj = read_pbxproj(&root);
            let mangled = pbxproj.replace(from, to);
            assert_ne!(mangled, pbxproj, "mangling {i} must change the pbxproj");
            fs::write(root.join(PBXPROJ_REL), &mangled).unwrap();
            let cargo_before = fs::read(root.join(CARGO_TOML_REL)).unwrap();
            let plist_before = fs::read(root.join(PLIST_REL)).unwrap();
            let before = snapshot_tree(&root);

            let err = add_test_spec(&root, &ENTITLEMENT_PLUGIN).unwrap_err();
            assert!(
                matches!(&err, PluginAddError::MalformedProjectFile(f) if f == PBXPROJ_REL),
                "mangling {i}: {err}"
            );
            assert_eq!(fs::read(root.join(CARGO_TOML_REL)).unwrap(), cargo_before);
            assert_eq!(fs::read(root.join(PLIST_REL)).unwrap(), plist_before);
            assert_eq!(read_pbxproj(&root), mangled);
            assert!(!root.join(ENTITLEMENTS_REL).exists());
            assert_eq!(before, snapshot_tree(&root), "mangling {i}");

            let _ = fs::remove_dir_all(&root);
        }
    }

    /// An existing entitlements file the key cannot be inserted into is
    /// refused up front as well.
    #[test]
    fn an_entitlements_file_without_its_dict_refuses_the_add_before_any_write() {
        let root = scaffold_project("ios-entitlement-no-dict");
        fs::write(
            root.join(ENTITLEMENTS_REL),
            "<plist version=\"1.0\">\n</plist>\n",
        )
        .unwrap();
        let before = snapshot_tree(&root);

        let err = add_test_spec(&root, &ENTITLEMENT_PLUGIN).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == ENTITLEMENTS_REL),
            "{err}"
        );
        assert_eq!(before, snapshot_tree(&root));

        let _ = fs::remove_dir_all(&root);
    }

    /// A configuration already naming another entitlements file keeps its
    /// setting; the key goes into that file instead.
    #[test]
    fn ios_entitlement_writes_into_the_file_an_existing_setting_names() {
        let root = scaffold_project("ios-entitlement-other-file");
        let custom_line = "CODE_SIGN_ENTITLEMENTS = Runner/App.entitlements;";
        let style = "\t\t\t\tCODE_SIGN_STYLE = Automatic;\n";
        let pbxproj =
            read_pbxproj(&root).replace(style, &format!("\t\t\t\t{custom_line}\n{style}"));
        assert_eq!(pbxproj.matches(custom_line).count(), 3);
        fs::write(root.join(PBXPROJ_REL), &pbxproj).unwrap();
        fs::write(root.join("ios/Runner/App.entitlements"), EMPTY_ENTITLEMENTS).unwrap();

        assert_eq!(apply_test_entitlement(&root).unwrap(), AddOutcome::Applied);

        assert_eq!(read_pbxproj(&root), pbxproj, "the setting is left alone");
        assert!(!root.join(ENTITLEMENTS_REL).exists());
        let entitlements = read_entitlements(&root, "ios/Runner/App.entitlements");
        assert!(
            entitlements.contains(&format!("<key>{TEST_ENTITLEMENT}</key>")),
            "{entitlements}"
        );
        assert_eq!(
            apply_test_entitlement(&root).unwrap(),
            AddOutcome::AlreadyPresent
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// When only some configurations name a file, the rest are pointed at
    /// that same file, so every configuration signs with the entitlement.
    #[test]
    fn configurations_without_a_setting_join_the_file_the_others_name() {
        let root = scaffold_project("ios-entitlement-partial-setting");
        let custom_line = "CODE_SIGN_ENTITLEMENTS = Runner/App.entitlements;";
        let style = "\t\t\t\tCODE_SIGN_STYLE = Automatic;\n";
        let pbxproj =
            read_pbxproj(&root).replacen(style, &format!("\t\t\t\t{custom_line}\n{style}"), 1);
        fs::write(root.join(PBXPROJ_REL), &pbxproj).unwrap();

        assert_eq!(apply_test_entitlement(&root).unwrap(), AddOutcome::Applied);

        let edited = read_pbxproj(&root);
        assert_eq!(edited.matches(custom_line).count(), 3, "{edited}");
        assert!(!edited.contains(ENTITLEMENTS_LINE), "{edited}");
        assert!(!root.join(ENTITLEMENTS_REL).exists());
        assert!(
            read_entitlements(&root, "ios/Runner/App.entitlements")
                .contains(&format!("<key>{TEST_ENTITLEMENT}</key>"))
        );
        assert_pbxproj_well_formed(&edited);

        let _ = fs::remove_dir_all(&root);
    }

    /// The setting in place but the file gone: the file is recreated and the
    /// pbxproj left as it is.
    #[test]
    fn a_half_applied_ios_entitlement_completes_the_missing_file() {
        let root = scaffold_project("ios-entitlement-half");
        assert_eq!(apply_test_entitlement(&root).unwrap(), AddOutcome::Applied);
        let pbxproj = read_pbxproj(&root);
        let entitlements = read_entitlements(&root, ENTITLEMENTS_REL);
        fs::remove_file(root.join(ENTITLEMENTS_REL)).unwrap();

        assert_eq!(apply_test_entitlement(&root).unwrap(), AddOutcome::Applied);
        assert_eq!(read_pbxproj(&root), pbxproj);
        assert_eq!(read_entitlements(&root, ENTITLEMENTS_REL), entitlements);

        let _ = fs::remove_dir_all(&root);
    }

    /// A project without `ios/` refuses an entitlement with the missing
    /// pbxproj, as it refuses a plist key or a Swift package; only the link
    /// flags are satisfied there, because an app with no iOS project has no
    /// Xcode link to complete (a desktop- or Android-only app keeps adding a
    /// plugin whose iOS arm is link lines alone, such as `onnx`).
    #[test]
    fn a_project_without_ios_refuses_an_entitlement_and_satisfies_link_flags() {
        let root = scaffold_project("ios-entitlement-no-ios");
        fs::remove_dir_all(root.join("ios")).unwrap();

        let entitlement = apply_test_entitlement(&root).unwrap_err();
        assert!(
            matches!(&entitlement, PluginAddError::MissingProjectFile(f) if f == PBXPROJ_REL),
            "{entitlement}"
        );
        let plist = apply_plist_entry(&root, "K", "v", "c").unwrap_err();
        assert!(
            matches!(&plist, PluginAddError::MissingProjectFile(f) if f == PLIST_REL),
            "{plist}"
        );
        assert_eq!(
            apply_ios_framework(&root, "UserNotifications").unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(
            apply_ios_library(&root, "c++").unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert!(ios_link_flag_satisfied(&root, LinkFlag::Library("c++")).unwrap());
        assert!(ios_link_flag_satisfied(&root, LinkFlag::Framework("CoreML")).unwrap());
        assert!(!root.join("ios").exists(), "nothing is created under ios/");

        let _ = fs::remove_dir_all(&root);
    }

    /// Adding a plugin to a project without `ios/` (or without `android/`)
    /// leaves that platform's satisfied items out of the report — neither
    /// present nor applied — as the pending report and a manifest plugin's
    /// add leave them out; the other platform's items are still reported.
    #[test]
    fn an_add_does_not_report_items_for_an_absent_platform() {
        let spec = PluginSpec {
            id: Cow::Borrowed("absent-platform"),
            crate_dir: Cow::Borrowed("absent-platform"),
            base: Cow::Owned(vec![
                Contribution::CargoDep {
                    name: Cow::Borrowed("frust-absent-platform"),
                },
                Contribution::IosLibrary {
                    name: Cow::Borrowed("c++"),
                },
                Contribution::IosFramework {
                    name: Cow::Borrowed("CoreML"),
                },
                Contribution::ManifestPermission {
                    permission: Cow::Borrowed("android.permission.NFC"),
                },
            ]),
            ..PluginSpec::DEFAULT
        };
        for (absent, kept) in [("ios", "android"), ("android", "ios")] {
            let root = scaffold_project(&format!("add-no-{absent}"));
            fs::remove_dir_all(root.join(absent)).unwrap();
            let report = add_test_spec(&root, &spec).unwrap();
            let items: Vec<(&str, AddOutcome)> = report
                .items
                .iter()
                .map(|item| (item.description.as_str(), item.outcome))
                .collect();
            let expected: Vec<(&str, AddOutcome)> = if kept == "android" {
                vec![
                    (
                        "Cargo.toml dependency `frust-absent-platform`",
                        AddOutcome::Applied,
                    ),
                    (
                        "AndroidManifest.xml permission `android.permission.NFC`",
                        AddOutcome::Applied,
                    ),
                ]
            } else {
                vec![
                    (
                        "Cargo.toml dependency `frust-absent-platform`",
                        AddOutcome::Applied,
                    ),
                    (
                        "Xcode library `-lc++` (`OTHER_LDFLAGS`)",
                        AddOutcome::Applied,
                    ),
                    (
                        "Xcode framework `CoreML` (`OTHER_LDFLAGS`)",
                        AddOutcome::Applied,
                    ),
                ]
            };
            assert_eq!(items, expected, "without {absent}/");
            assert!(!root.join(absent).exists(), "{absent}/ is never created");
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// `ios/` present but its pbxproj gone is a broken iOS project, not an
    /// absent one: both link flags still fail on the missing file.
    #[test]
    fn link_flags_on_an_ios_directory_without_its_pbxproj_fail() {
        let root = scaffold_project("ios-links-no-pbxproj");
        fs::remove_file(root.join(PBXPROJ_REL)).unwrap();
        let before = snapshot_tree(&root);
        for err in [
            apply_ios_framework(&root, "CoreML").unwrap_err(),
            apply_ios_library(&root, "c++").unwrap_err(),
            ios_link_flag_satisfied(&root, LinkFlag::Library("c++")).unwrap_err(),
        ] {
            assert!(
                matches!(&err, PluginAddError::MissingProjectFile(f) if f == PBXPROJ_REL),
                "{err}"
            );
        }
        assert_eq!(snapshot_tree(&root), before);
        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // `Contribution::IosLibrary`
    // -----------------------------------------------------------------------

    /// `-l<name>` is appended, quoted, to every list; a second run writes
    /// nothing; a half-applied project is completed and reported `Applied`.
    #[test]
    fn ios_library_lands_in_every_list_once_and_completes_a_half_applied_project() {
        let root = scaffold_project("ios-library");
        let before = read_pbxproj(&root);
        let lists = before.matches("OTHER_LDFLAGS = (").count();
        assert_eq!(lists, 3);

        assert_eq!(
            apply_ios_library(&root, "c++").unwrap(),
            AddOutcome::Applied
        );
        let once = read_pbxproj(&root);
        let line = "\t\t\t\t\t\"-lc++\",\n";
        assert_eq!(once.matches(line).count(), lists, "{once}");
        // Only the three lines were added; the list closes right after each.
        assert_eq!(once.matches(&format!("{line}\t\t\t\t);")).count(), lists);
        assert_eq!(once.len(), before.len() + lists * line.len());
        assert_pbxproj_well_formed(&once);

        assert_eq!(
            apply_ios_library(&root, "c++").unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(read_pbxproj(&root), once);

        let half = once.replacen(line, "", 1);
        fs::write(root.join(PBXPROJ_REL), &half).unwrap();
        assert!(!ios_link_flag_satisfied(&root, LinkFlag::Library("c++")).unwrap());
        assert_eq!(
            apply_ios_library(&root, "c++").unwrap(),
            AddOutcome::Applied
        );
        assert_eq!(read_pbxproj(&root), once);
        let _ = fs::remove_dir_all(&root);
    }

    /// A library line is matched bare or quoted, on trimmed lines, and only
    /// as a whole entry: `-lc++` is not `-lc` and the app's own `-l<app>` is
    /// not another library.
    #[test]
    fn ios_library_presence_matches_whole_entries_bare_or_quoted() {
        let library = LinkFlag::Library("c++");
        assert!(library.linked_in("\t\"-lc++\",\n"));
        assert!(library.linked_in("  -lc++,  \n"));
        assert!(!library.linked_in("\"-lc++abi\",\n"));
        assert!(!library.linked_in("\"-lc\",\n"));
        assert!(!library.linked_in("\"-framework\",\nc++,\n"));
        assert!(LinkFlag::Library("c").linked_in("\"-lc\",\n"));
        assert!(!LinkFlag::Library("c").linked_in("\"-lc++\",\n"));
        // A framework of the same name is not the library, nor the reverse.
        assert!(!LinkFlag::Framework("z").linked_in("\"-lz\",\n"));
        assert!(!LinkFlag::Library("z").linked_in("\"-framework\",\nz,\n"));
    }

    #[test]
    fn entitlements_setting_paths_resolve_inside_ios_only() {
        for (setting, rel) in [
            (
                "Runner/Runner.entitlements",
                "ios/Runner/Runner.entitlements",
            ),
            (
                "$(SRCROOT)/Runner/App.entitlements",
                "ios/Runner/App.entitlements",
            ),
            ("${PROJECT_DIR}/App.entitlements", "ios/App.entitlements"),
        ] {
            assert_eq!(entitlements_rel_path(setting).unwrap(), rel, "{setting}");
        }
        for setting in [
            "",
            "/etc/App.entitlements",
            "../App.entitlements",
            "Runner/../../App.entitlements",
            "$(TARGET_NAME)/App.entitlements",
            "C:/App.entitlements",
        ] {
            assert!(
                matches!(
                    entitlements_rel_path(setting),
                    Err(PluginAddError::MalformedProjectFile(_))
                ),
                "{setting}"
            );
        }
    }

    /// Braces, `=` and `;` inside a quoted string or a comment are not
    /// structure: the shell-script phase and the object comments of a real
    /// scaffold must not confuse the object scan.
    #[test]
    fn the_pbx_scanner_skips_strings_and_comments() {
        let src = "/* Begin X section */\n\
                   \t\tAAAA /* a { comment ; */ = {\n\
                   \t\t\tisa = X;\n\
                   \t\t\tshellScript = \"echo \\\"${HOME}\\\"; }\";\n\
                   \t\t\tname = \"Two Words\";\n\
                   \t\t};\n\
                   /* End X section */\n";
        let section = pbx_section_range(src, "X").unwrap();
        let objects = pbx_objects(src, section).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].id, "AAAA");
        let statements = pbx_statements(src, objects[0].body.clone()).unwrap();
        let keys: Vec<&str> = statements.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["isa", "shellScript", "name"]);
        assert_eq!(
            pbx_lookup(src, &statements, "name").as_deref(),
            Some("Two Words")
        );
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

    // -----------------------------------------------------------------------
    // `Contribution::MavenRepository`
    // -----------------------------------------------------------------------

    fn maven_spec(contribs: &'static [Contribution]) -> PluginSpec {
        PluginSpec {
            id: Cow::Borrowed("maven-test-plugin"),
            summary: Cow::Borrowed("synthetic, test-only"),
            crate_dir: Cow::Borrowed("maven-test-plugin"),
            base: Cow::Borrowed(contribs),
            optional_features: Cow::Borrowed(&[]),
            requires_sibling: None,
            ..PluginSpec::DEFAULT
        }
    }

    const REPO_A: Contribution = Contribution::MavenRepository {
        url: Cow::Borrowed("https://repo.example.test/a"),
        groups: Cow::Borrowed(&[
            Cow::Borrowed("com.example.a"),
            Cow::Borrowed("com.example.b"),
        ]),
        comment: Cow::Borrowed("Repo A"),
    };
    const REPO_B: Contribution = Contribution::MavenRepository {
        url: Cow::Borrowed("https://repo.example.test/b"),
        groups: Cow::Borrowed(&[Cow::Borrowed("org.example")]),
        comment: Cow::Borrowed("Repo B"),
    };

    #[test]
    fn maven_repository_lands_in_dependency_resolution_after_maven_central() {
        let root = scaffold_project("maven-repo-lands");
        let path = root.join(SETTINGS_GRADLE_REL);
        assert_eq!(
            apply_maven_repository(
                &root,
                "https://repo.example.test/a",
                &["g.one", "g.two"],
                "Repo A"
            )
            .unwrap(),
            AddOutcome::Applied
        );
        let after = fs::read_to_string(&path).unwrap();
        let drm = after.find("dependencyResolutionManagement").unwrap();
        let central = drm + after[drm..].find("mavenCentral()").unwrap();
        let marker = after.find(REPOSITORIES_ANCHOR).unwrap();
        let block = after.find("exclusiveContent {").unwrap();
        assert!(
            drm < central && central < marker && marker < block,
            "{after}"
        );
        assert!(after.contains("uri(\"https://repo.example.test/a\")"));
        assert!(after.contains("includeGroup(\"g.one\")"));
        assert!(after.contains("includeGroup(\"g.two\")"));
        assert!(after.contains("// Repo A\n"));
        // pluginManagement is untouched.
        assert_eq!(after.matches("exclusiveContent").count(), 1);
        assert_eq!(
            apply_maven_repository(&root, "https://repo.example.test/a", &["g.one"], "Repo A")
                .unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), after);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_maven_repositories_make_two_blocks() {
        static SPEC_BASE: [Contribution; 3] = [
            Contribution::CargoDep {
                name: Cow::Borrowed("frust-maven-test-plugin"),
            },
            REPO_A,
            REPO_B,
        ];
        let root = scaffold_project("maven-repo-two");
        let spec = maven_spec(&SPEC_BASE);
        let report = add_test_spec(&root, &spec).unwrap();
        assert_eq!(report.items.len(), 3);
        let after = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        assert_eq!(after.matches("exclusiveContent {").count(), 2, "{after}");
        let again = add_test_spec(&root, &spec).unwrap();
        assert!(
            again
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent)
        );
        assert_eq!(
            fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            after
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maven_repository_without_the_marker_fails_before_any_write() {
        static SPEC_BASE: [Contribution; 2] = [
            Contribution::CargoDep {
                name: Cow::Borrowed("frust-maven-test-plugin"),
            },
            REPO_A,
        ];
        let root = scaffold_project("maven-repo-no-marker");
        let path = root.join(SETTINGS_GRADLE_REL);
        let mangled = fs::read_to_string(&path)
            .unwrap()
            .replace(REPOSITORIES_ANCHOR, "// (marker removed)");
        fs::write(&path, &mangled).unwrap();
        let cargo_before = fs::read(root.join(CARGO_TOML_REL)).unwrap();
        let err = add_test_spec(&root, &maven_spec(&SPEC_BASE)).unwrap_err();
        match &err {
            PluginAddError::MalformedProjectFile(msg) => {
                assert!(msg.contains(REPOSITORIES_ANCHOR), "{msg}");
                assert!(msg.contains("mavenCentral()"), "{msg}");
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), mangled);
        assert_eq!(fs::read(root.join(CARGO_TOML_REL)).unwrap(), cargo_before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maven_repository_rejects_empty_groups_and_non_http_urls() {
        let root = scaffold_project("maven-repo-invalid");
        let before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();
        assert!(matches!(
            apply_maven_repository(&root, "https://x.test/m", &[] as &[&str], "c"),
            Err(PluginAddError::InvalidContribution(_))
        ));
        assert!(matches!(
            apply_maven_repository(&root, "file:///tmp/m", &["g"], "c"),
            Err(PluginAddError::InvalidContribution(_))
        ));
        assert_eq!(fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maven_repository_escapes_quotes_and_backslashes() {
        let root = scaffold_project("maven-repo-escape");
        apply_maven_repository(&root, "https://x.test/a\"b\\c", &["g\"h"], "c").unwrap();
        let after = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        assert!(
            after.contains("uri(\"https://x.test/a\\\"b\\\\c\")"),
            "{after}"
        );
        assert!(after.contains("includeGroup(\"g\\\"h\")"), "{after}");
        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // `Contribution::AndroidLibcxxShared`
    // -----------------------------------------------------------------------

    const LIBCXX_TRUE: &str = "frust.cargoNdk.linkLibcxxShared=true";

    fn read_gradle_properties(root: &Path) -> String {
        fs::read_to_string(root.join(GRADLE_PROPERTIES_REL)).unwrap()
    }

    #[test]
    fn a_fresh_project_reads_the_property_and_ships_it_off() {
        let root = scaffold_project("libcxx-template");
        let build = fs::read_to_string(root.join(APP_BUILD_GRADLE_REL)).unwrap();
        assert!(libcxx_shared_reader_present(&build));
        assert!(
            build.contains("if (linkLibcxxShared) add(\"--link-libcxx-shared\")"),
            "{build}"
        );
        let properties = read_gradle_properties(&root);
        assert_eq!(
            libcxx_shared_assignment(&properties).map(|(_, value)| value),
            Some("false"),
            "{properties}"
        );
        assert!(!libcxx_shared_present(&root).unwrap());
        assert!(!libcxx_shared_requested(&root));
        assert!(!libcxx_shared_requested(&root.join("no-such-project")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adding_onnx_turns_the_property_on_once() {
        let root = scaffold_project("libcxx-onnx");
        let before = read_gradle_properties(&root);
        let report = add_plugin(&root, "onnx", &[]).unwrap();
        assert_eq!(
            report
                .items
                .iter()
                .map(|i| (i.description.as_str(), i.outcome))
                .collect::<Vec<_>>(),
            [
                ("Cargo.toml dependency `frust-onnx`", AddOutcome::Applied),
                (
                    "Android `libc++_shared.so` in the APK (`frust.cargoNdk.linkLibcxxShared`)",
                    AddOutcome::Applied
                ),
                (
                    "AndroidManifest.xml permission `android.permission.INTERNET`",
                    AddOutcome::Applied
                ),
                (
                    "Xcode library `-lc++` (`OTHER_LDFLAGS`)",
                    AddOutcome::Applied
                ),
                (
                    "Xcode framework `CoreML` (`OTHER_LDFLAGS`)",
                    AddOutcome::Applied
                ),
                (
                    "Xcode framework `Foundation` (`OTHER_LDFLAGS`)",
                    AddOutcome::Applied
                ),
            ]
        );
        let after = read_gradle_properties(&root);
        // The template's `false` line is rewritten in place; nothing else moves.
        assert_eq!(
            after,
            before.replace("frust.cargoNdk.linkLibcxxShared=false", LIBCXX_TRUE)
        );
        assert_eq!(after.matches("linkLibcxxShared").count(), 1, "{after}");
        assert!(libcxx_shared_present(&root).unwrap());
        assert!(libcxx_shared_requested(&root));

        let tree = snapshot_tree(&root);
        let again = add_plugin(&root, "onnx", &[]).unwrap();
        assert!(
            again
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{again:?}"
        );
        assert_eq!(snapshot_tree(&root), tree);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_property_line_is_appended_and_any_true_spelling_is_present() {
        let root = scaffold_project("libcxx-append");
        let path = root.join(GRADLE_PROPERTIES_REL);
        let without = "org.gradle.jvmargs=-Xmx2048m\nandroid.useAndroidX=true";
        fs::write(&path, without).unwrap();
        assert_eq!(
            apply_android_libcxx_shared(&root).unwrap(),
            AddOutcome::Applied
        );
        let after = read_gradle_properties(&root);
        assert!(after.starts_with(&format!("{without}\n# ")), "{after}");
        assert!(after.ends_with(&format!("\n{LIBCXX_TRUE}\n")), "{after}");
        assert_eq!(
            apply_android_libcxx_shared(&root).unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(read_gradle_properties(&root), after);

        // Java-properties separators and a case-insensitive `true` all count;
        // the last assignment wins, and a commented line is no assignment.
        for (text, set) in [
            ("frust.cargoNdk.linkLibcxxShared = TRUE\n", true),
            ("frust.cargoNdk.linkLibcxxShared: true\r\n", true),
            ("frust.cargoNdk.linkLibcxxShared true\n", true),
            (
                "frust.cargoNdk.linkLibcxxShared=true\nfrust.cargoNdk.linkLibcxxShared=false\n",
                false,
            ),
            ("# frust.cargoNdk.linkLibcxxShared=true\n", false),
            ("frust.cargoNdk.linkLibcxxSharedX=true\n", false),
        ] {
            assert_eq!(libcxx_shared_property_set(text), set, "{text:?}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_property_set_to_another_value_is_rewritten_in_place() {
        let root = scaffold_project("libcxx-rewrite");
        let path = root.join(GRADLE_PROPERTIES_REL);
        fs::write(
            &path,
            "a=1\r\n  frust.cargoNdk.linkLibcxxShared : no\r\nb=2\r\n",
        )
        .unwrap();
        assert_eq!(
            apply_android_libcxx_shared(&root).unwrap(),
            AddOutcome::Applied
        );
        assert_eq!(
            read_gradle_properties(&root),
            format!("a=1\r\n  {LIBCXX_TRUE}\r\nb=2\r\n")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adding_onnx_to_an_app_without_android_writes_only_the_dependency() {
        let dest = unique_temp_dir("libcxx-no-android");
        let platforms: Vec<_> = scaffold::ScaffoldPlatform::DEFAULT
            .iter()
            .copied()
            .filter(|p| *p != scaffold::ScaffoldPlatform::Android)
            .collect();
        scaffold::generate_with_platforms(&dest, &test_context(), None, false, None, &platforms)
            .unwrap();
        assert!(!dest.join(ANDROID_DIR_REL).exists());
        let before = snapshot_tree(&dest);

        let report = add_plugin(&dest, "onnx", &[]).unwrap();
        assert_eq!(
            report
                .items
                .iter()
                .map(|i| (i.description.as_str(), i.outcome))
                .collect::<Vec<_>>(),
            // The Android items have nothing to edit and are not reported.
            [
                ("Cargo.toml dependency `frust-onnx`", AddOutcome::Applied),
                (
                    "Xcode library `-lc++` (`OTHER_LDFLAGS`)",
                    AddOutcome::Applied
                ),
                (
                    "Xcode framework `CoreML` (`OTHER_LDFLAGS`)",
                    AddOutcome::Applied
                ),
                (
                    "Xcode framework `Foundation` (`OTHER_LDFLAGS`)",
                    AddOutcome::Applied
                ),
            ]
        );
        let after = snapshot_tree(&dest);
        let changed: Vec<&String> = after
            .keys()
            .filter(|k| before.get(*k) != after.get(*k))
            .collect();
        // The app still has `ios/`, so its Xcode link gains the three lines.
        assert_eq!(changed, [CARGO_TOML_REL, PBXPROJ_REL], "{changed:?}");
        assert_eq!(before.len(), after.len());
        assert!(!dest.join(ANDROID_DIR_REL).exists());
        assert!(libcxx_shared_present(&dest).unwrap());

        let catalog = PluginCatalog::built_in();
        let statuses = crate::plugin::plugin_statuses(&dest, &catalog).unwrap();
        let (_, status) = statuses
            .iter()
            .find(|(entry, _)| &*entry.spec.id == "onnx")
            .unwrap();
        assert!(
            matches!(status, crate::plugin::PluginStatus::Added),
            "{status:?}"
        );
        let _ = fs::remove_dir_all(&dest);
    }

    /// `models` and `onnx` on a scaffold generated without the android platform:
    /// only `Cargo.toml` and (for `onnx`'s link lines) the pbxproj change, no
    /// Android item is reported and `plugin_statuses` reports both added.
    #[test]
    fn models_and_onnx_on_an_app_without_android_change_no_android_file() {
        let dest = unique_temp_dir("internet-no-android");
        let platforms: Vec<_> = scaffold::ScaffoldPlatform::DEFAULT
            .iter()
            .copied()
            .filter(|p| *p != scaffold::ScaffoldPlatform::Android)
            .collect();
        scaffold::generate_with_platforms(&dest, &test_context(), None, false, None, &platforms)
            .unwrap();
        assert!(!dest.join(ANDROID_DIR_REL).exists());
        let before = snapshot_tree(&dest);

        for id in ["models", "onnx"] {
            let report = add_plugin(&dest, id, &[]).unwrap();
            assert!(
                !report
                    .items
                    .iter()
                    .any(|i| i.description.contains("android.permission.INTERNET")),
                "{report:?}"
            );
        }
        let after = snapshot_tree(&dest);
        let changed: Vec<&String> = after
            .keys()
            .filter(|k| before.get(*k) != after.get(*k))
            .collect();
        assert_eq!(changed, [CARGO_TOML_REL, PBXPROJ_REL], "{changed:?}");
        assert_eq!(before.len(), after.len());
        assert!(!dest.join(ANDROID_DIR_REL).exists());

        let catalog = PluginCatalog::built_in();
        let statuses = crate::plugin::plugin_statuses(&dest, &catalog).unwrap();
        for id in ["models", "onnx"] {
            let (_, status) = statuses
                .iter()
                .find(|(entry, _)| &*entry.spec.id == id)
                .unwrap();
            assert!(
                matches!(status, crate::plugin::PluginStatus::Added),
                "{id}: {status:?}"
            );
        }
        let _ = fs::remove_dir_all(&dest);
    }

    /// `haptics` on a desktop-only app: its VIBRATE permission has nothing to
    /// edit without an `android/` directory and is not reported, so only
    /// `Cargo.toml` changes.
    #[test]
    fn haptics_on_an_app_without_android_writes_only_the_dependency() {
        let dest = unique_temp_dir("haptics-no-android");
        let platforms: Vec<_> = scaffold::ScaffoldPlatform::DEFAULT
            .iter()
            .copied()
            .filter(|p| *p != scaffold::ScaffoldPlatform::Android)
            .collect();
        scaffold::generate_with_platforms(&dest, &test_context(), None, false, None, &platforms)
            .unwrap();
        let before = snapshot_tree(&dest);

        let report = add_plugin(&dest, "haptics", &[]).unwrap();
        assert!(
            !report
                .items
                .iter()
                .any(|i| i.description.contains("android.permission.VIBRATE")),
            "{report:?}"
        );
        let after = snapshot_tree(&dest);
        let changed: Vec<&String> = after
            .keys()
            .filter(|k| before.get(*k) != after.get(*k))
            .collect();
        assert_eq!(changed, [CARGO_TOML_REL], "{changed:?}");
        assert!(!dest.join(ANDROID_DIR_REL).exists());
        let _ = fs::remove_dir_all(&dest);
    }

    /// `onnx` on an app without `ios/` (desktop and Android only): its three
    /// link lines have nothing to edit and are not reported, so only
    /// `Cargo.toml` and the two Android files change, nothing under `ios/` is
    /// created, and nothing stays pending.
    #[test]
    fn adding_onnx_to_an_app_without_ios_writes_no_ios_file() {
        let dest = unique_temp_dir("onnx-no-ios");
        let platforms: Vec<_> = scaffold::ScaffoldPlatform::DEFAULT
            .iter()
            .copied()
            .filter(|p| *p != scaffold::ScaffoldPlatform::Ios)
            .collect();
        scaffold::generate_with_platforms(&dest, &test_context(), None, false, None, &platforms)
            .unwrap();
        assert!(!dest.join(IOS_DIR_REL).exists());
        let before = snapshot_tree(&dest);

        let report = add_plugin(&dest, "onnx", &[]).unwrap();
        let outcomes: Vec<(&str, AddOutcome)> = report
            .items
            .iter()
            .map(|i| (i.description.as_str(), i.outcome))
            .collect();
        assert_eq!(outcomes.len(), 3, "{report:?}");
        assert!(
            outcomes.iter().all(|(d, _)| !d.starts_with("Xcode")),
            "{report:?}"
        );
        let after = snapshot_tree(&dest);
        let changed: Vec<&String> = after
            .keys()
            .filter(|k| before.get(*k) != after.get(*k))
            .collect();
        assert_eq!(
            changed,
            [CARGO_TOML_REL, MANIFEST_REL, GRADLE_PROPERTIES_REL],
            "{changed:?}"
        );
        assert!(!dest.join(IOS_DIR_REL).exists());

        let catalog = PluginCatalog::built_in();
        let entry = catalog.find("onnx").unwrap();
        assert_eq!(
            crate::plugin::pending_contributions(&dest, entry, &[]).unwrap(),
            []
        );
        let _ = fs::remove_dir_all(&dest);
    }

    /// An app generated before the template moved to iOS 15.1 is refused by
    /// `onnx` with the platform-floor message, before anything is written;
    /// raising the target as the message says lets the add through.
    #[test]
    fn adding_onnx_to_an_app_at_ios_15_0_is_refused_until_it_is_raised() {
        let root = scaffold_project("onnx-ios-15-0");
        rewrite(
            &root,
            PBXPROJ_REL,
            "IPHONEOS_DEPLOYMENT_TARGET = 15.1;",
            "IPHONEOS_DEPLOYMENT_TARGET = 15.0;",
        );
        let before = snapshot_tree(&root);

        let err = add_plugin(&root, "onnx", &[]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "plugin `onnx` needs iOS 15.1 or later; \
             ios/Runner.xcodeproj/project.pbxproj sets IPHONEOS_DEPLOYMENT_TARGET = 15.0. \
             Raise it in every build configuration (and keep it at or above every Swift \
             package's minimum), then re-run."
        );
        assert_eq!(snapshot_tree(&root), before, "nothing may be written");

        rewrite(
            &root,
            PBXPROJ_REL,
            "IPHONEOS_DEPLOYMENT_TARGET = 15.0;",
            "IPHONEOS_DEPLOYMENT_TARGET = 15.1;",
        );
        let report = add_plugin(&root, "onnx", &[]).unwrap();
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_quoted_reader_line_and_the_presence_check_accept_the_same_values() {
        // The template and the quoted fix line both read the property as
        // Kotlin's `toBoolean()` does: `true` in any letter case, trimmed.
        assert!(
            LIBCXX_SHARED_READER_LINE.contains("equals(\"true\", ignoreCase = true)"),
            "{LIBCXX_SHARED_READER_LINE}"
        );
        assert!(libcxx_shared_reader_present(LIBCXX_SHARED_READER_LINE));
        for value in ["true", "TRUE", "True", " true ", "tRuE"] {
            assert!(
                libcxx_shared_property_set(&format!("{LIBCXX_SHARED_PROPERTY}={value}\n")),
                "{value:?}"
            );
        }
        for value in ["false", "yes", "1", "", "truee"] {
            assert!(
                !libcxx_shared_property_set(&format!("{LIBCXX_SHARED_PROPERTY}={value}\n")),
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_build_script_that_predates_the_property_is_refused_before_any_write() {
        let root = scaffold_project("libcxx-old-template");
        let build_path = root.join(APP_BUILD_GRADLE_REL);
        let old = fs::read_to_string(&build_path)
            .unwrap()
            .lines()
            .filter(|line| !line.contains("inkLibcxxShared"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&build_path, &old).unwrap();
        let properties_path = root.join(GRADLE_PROPERTIES_REL);
        fs::write(&properties_path, "android.useAndroidX=true\n").unwrap();
        let tree = snapshot_tree(&root);

        let err = add_plugin(&root, "onnx", &[]).unwrap_err();
        match &err {
            PluginAddError::MalformedProjectFile(msg) => {
                assert!(msg.starts_with(APP_BUILD_GRADLE_REL), "{msg}");
                assert!(msg.contains(LIBCXX_SHARED_READER_LINE), "{msg}");
                assert!(msg.contains("after `add(\"ndk\")`"), "{msg}");
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(snapshot_tree(&root), tree, "nothing may be written");

        // The line the error quotes is enough: once it is in place, the add
        // goes through.
        let fixed = old.replacen(
            "add(\"ndk\")",
            &format!("add(\"ndk\")\n            {LIBCXX_SHARED_READER_LINE}"),
            1,
        );
        assert_ne!(fixed, old);
        fs::write(&build_path, fixed).unwrap();
        let report = add_plugin(&root, "onnx", &[]).unwrap();
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );
        assert!(libcxx_shared_present(&root).unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_shared_cxx_runtime_needs_both_android_files() {
        let root = scaffold_project("libcxx-missing-files");
        fs::remove_file(root.join(GRADLE_PROPERTIES_REL)).unwrap();
        assert!(matches!(
            apply_android_libcxx_shared(&root),
            Err(PluginAddError::MissingProjectFile(rel)) if rel == GRADLE_PROPERTIES_REL
        ));
        fs::remove_file(root.join(APP_BUILD_GRADLE_REL)).unwrap();
        assert!(matches!(
            apply_android_libcxx_shared(&root),
            Err(PluginAddError::MissingProjectFile(rel)) if rel == APP_BUILD_GRADLE_REL
        ));
        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // `Contribution::PlistArrayEntry` and Info.plist escaping
    // -----------------------------------------------------------------------

    fn read_plist(root: &Path) -> String {
        fs::read_to_string(root.join(PLIST_REL)).unwrap()
    }

    #[test]
    fn plist_array_entry_writes_a_well_formed_array_and_reapply_is_a_no_op() {
        let root = scaffold_project("plist-array-two");
        let before = read_plist(&root);
        assert_eq!(
            apply_plist_array_entry(&root, "TestIds", &["a.one", "a.two"], "Ids").unwrap(),
            AddOutcome::Applied
        );
        let after = read_plist(&root);
        assert!(
            after.contains(
                "\t<!-- Ids -->\n\t<key>TestIds</key>\n\t<array>\n\
                 \t\t<string>a.one</string>\n\t\t<string>a.two</string>\n\t</array>\n</dict>"
            ),
            "{after}"
        );
        assert!(after.len() > before.len());
        assert_eq!(
            apply_plist_array_entry(&root, "TestIds", &["other"], "Ids").unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(read_plist(&root), after);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn plist_array_entry_with_no_values_is_an_empty_array() {
        let root = scaffold_project("plist-array-empty");
        assert_eq!(
            apply_plist_array_entry(&root, "TestIds", &[] as &[&str], "Ids").unwrap(),
            AddOutcome::Applied
        );
        assert!(read_plist(&root).contains("\t<key>TestIds</key>\n\t<array>\n\t</array>\n</dict>"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn plist_values_and_keys_are_xml_escaped_for_both_kinds() {
        let root = scaffold_project("plist-escape");
        apply_plist_entry(&root, "k&<s>", "a&b<c", "x -- y --- z").unwrap();
        apply_plist_array_entry(&root, "k&<a>", &["a&b<c", "d>e"], "arr").unwrap();
        let plist = read_plist(&root);
        assert!(
            plist.contains("<key>k&amp;&lt;s&gt;</key>\n\t<string>a&amp;b&lt;c</string>"),
            "{plist}"
        );
        assert!(
            plist.contains(
                "<key>k&amp;&lt;a&gt;</key>\n\t<array>\n\t\t<string>a&amp;b&lt;c</string>\n\t\t<string>d&gt;e</string>"
            ),
            "{plist}"
        );
        let comment_line = plist.lines().find(|l| l.contains("x - y")).unwrap();
        assert!(
            !comment_line
                .replace("<!--", "")
                .replace("-->", "")
                .contains("--")
        );
        // Escaped key is what the presence check looks for.
        assert_eq!(
            apply_plist_entry(&root, "k&<s>", "v", "c").unwrap(),
            AddOutcome::AlreadyPresent
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn plist_entries_without_a_dict_anchor_are_malformed_and_write_nothing() {
        let root = scaffold_project("plist-no-anchor");
        let broken = "<plist version=\"1.0\"></plist>\n";
        fs::write(root.join(PLIST_REL), broken).unwrap();
        assert!(matches!(
            apply_plist_array_entry(&root, "K", &["v"], "c"),
            Err(PluginAddError::MalformedProjectFile(_))
        ));
        assert!(matches!(
            apply_plist_entry(&root, "K", "v", "c"),
            Err(PluginAddError::MalformedProjectFile(_))
        ));
        assert_eq!(read_plist(&root), broken);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn every_registry_plist_entry_is_written_byte_identically_to_the_old_format() {
        let mut checked = 0;
        for spec in crate::plugin::known_plugins() {
            let all = spec.base.iter().chain(
                spec.optional_features
                    .iter()
                    .flat_map(|f| f.contributions.iter()),
            );
            for c in all {
                if let Contribution::PlistEntry {
                    key,
                    value,
                    comment,
                } = c
                {
                    let golden = format!(
                        "\t<!-- {comment} -->\n\t<key>{key}</key>\n\t<string>{value}</string>\n"
                    );
                    assert_eq!(
                        plist_entry_block(&xml_escape(key), value, comment),
                        golden,
                        "{}: {key}",
                        spec.id
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn plist_writes_leave_existing_build_setting_references_unchanged() {
        let root = scaffold_project("plist-build-setting");
        let before = read_plist(&root);
        assert!(before.contains("$(FRUST_DISPLAY_NAME)"), "{before}");
        apply_plist_entry(&root, "K", "v", "c").unwrap();
        apply_plist_array_entry(&root, "A", &["v"], "c").unwrap();
        let after = read_plist(&root);
        assert!(after.contains("$(FRUST_DISPLAY_NAME)"));
        let tail = before.rfind("</dict>").unwrap();
        assert_eq!(&after[..tail], &before[..tail]);
        let _ = fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // Platform floors (`PluginSpec::min_ios` / `min_android_sdk`)
    // -----------------------------------------------------------------------

    use crate::plugin::FeatureSpec;

    /// A synthetic plugin editing Cargo.toml, Info.plist, the pbxproj and both
    /// Gradle files, so a refused add can be shown to have touched none.
    const FLOOR_BASE: &[Contribution] = &[
        Contribution::CargoDep {
            name: Cow::Borrowed("frust-floor-test-plugin"),
        },
        Contribution::PlistEntry {
            key: Cow::Borrowed("NSFloorTestUsageDescription"),
            value: Cow::Borrowed("test"),
            comment: Cow::Borrowed("test"),
        },
        Contribution::IosFramework {
            name: Cow::Borrowed("FloorTestKit"),
        },
        Contribution::GradleModule {
            gradle_name: Cow::Borrowed(":frust-floor-test-plugin"),
            rel_path: Cow::Borrowed("plugins/floor-test-plugin/platform/android"),
        },
    ];

    /// A feature needing iOS 17 / API 33, above any floor its spec states.
    const FLOOR_FEATURES: &[FeatureSpec] = &[FeatureSpec {
        id: Cow::Borrowed("needs-17"),
        summary: Cow::Borrowed("synthetic, test-only"),
        contributions: Cow::Borrowed(&[]),
        min_ios: Some(Cow::Borrowed("17.0")),
        min_android_sdk: Some(33),
    }];

    /// The generated app's deployment-target line, in each of the three
    /// build configurations.
    const SCAFFOLD_TARGET: &str = "IPHONEOS_DEPLOYMENT_TARGET = 15.1;";

    fn floor_spec(
        base: &'static [Contribution],
        min_ios: Option<&'static str>,
        min_android_sdk: Option<u32>,
    ) -> PluginSpec {
        PluginSpec {
            id: Cow::Borrowed("floor-test-plugin"),
            summary: Cow::Borrowed("synthetic, test-only"),
            crate_dir: Cow::Borrowed("floor-test-plugin"),
            base: Cow::Borrowed(base),
            optional_features: Cow::Borrowed(FLOOR_FEATURES),
            min_ios: min_ios.map(Cow::Borrowed),
            min_android_sdk,
            ..PluginSpec::DEFAULT
        }
    }

    fn add_floor_spec(
        root: &Path,
        spec: &PluginSpec,
        features: &[&str],
    ) -> Result<AddReport, PluginAddError> {
        add_spec_with(&checkout_locator(root), root, spec, features)
    }

    /// Rewrite `rel` in place, asserting `from` was there to replace.
    fn rewrite(root: &Path, rel: &str, from: &str, to: &str) {
        let path = root.join(rel);
        let src = fs::read_to_string(&path).unwrap();
        assert!(src.contains(from), "`{from}` not in {rel}");
        fs::write(&path, src.replace(from, to)).unwrap();
    }

    #[test]
    fn ios_versions_compare_numerically_per_component() {
        let ordered = ["15.0", "15.1", "16", "16.4", "17.0"];
        for pair in ordered.windows(2) {
            let (low, high) = (
                parse_ios_version(pair[0]).unwrap(),
                parse_ios_version(pair[1]).unwrap(),
            );
            assert!(compare_versions(&low, &high).is_lt(), "{pair:?}");
            assert!(compare_versions(&high, &low).is_gt(), "{pair:?}");
        }
        assert!(
            compare_versions(
                &parse_ios_version("16").unwrap(),
                &parse_ios_version("16.0").unwrap()
            )
            .is_eq()
        );
        assert!(
            compare_versions(
                &parse_ios_version("16.10").unwrap(),
                &parse_ios_version("16.9").unwrap()
            )
            .is_gt()
        );
        for bad in ["", "16.", ".4", "a", "16.x", "16 .4", "-1"] {
            assert_eq!(parse_ios_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_min_ios_above_the_generated_app_is_refused_and_writes_nothing() {
        let root = scaffold_project("floor-ios-refused");
        let before = snapshot_tree(&root);
        let err =
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("16.4"), None), &[]).unwrap_err();
        match &err {
            PluginAddError::PlatformFloorTooLow {
                plugin,
                platform,
                required,
                found,
                key,
                file,
                ..
            } => {
                assert_eq!(&**plugin, "floor-test-plugin");
                assert_eq!(*platform, FloorPlatform::Ios);
                assert_eq!(required, "16.4");
                assert_eq!(found, "15.1");
                assert_eq!(*key, IOS_DEPLOYMENT_TARGET);
                assert_eq!(*file, PBXPROJ_REL);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(
            err.to_string(),
            "plugin `floor-test-plugin` needs iOS 16.4 or later; \
             ios/Runner.xcodeproj/project.pbxproj sets IPHONEOS_DEPLOYMENT_TARGET = 15.1. \
             Raise it in every build configuration (and keep it at or above every Swift \
             package's minimum), then re-run."
        );
        // Cargo.toml, Info.plist, the pbxproj and both Gradle files — the whole
        // tree — are byte-identical.
        assert_eq!(snapshot_tree(&root), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn raising_every_configuration_to_the_floor_lets_the_add_proceed() {
        for raised in ["16.4", "17.0", "16.4.1"] {
            let root = scaffold_project("floor-ios-raised");
            rewrite(
                &root,
                PBXPROJ_REL,
                SCAFFOLD_TARGET,
                &format!("IPHONEOS_DEPLOYMENT_TARGET = {raised};"),
            );
            let report = add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("16.4"), None), &[])
                .unwrap_or_else(|e| panic!("{raised}: {e}"));
            assert!(
                report
                    .items
                    .iter()
                    .all(|item| item.outcome == AddOutcome::Applied),
                "{raised}: {report:?}"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn one_configuration_left_below_the_floor_still_refuses() {
        let root = scaffold_project("floor-ios-partial");
        let path = root.join(PBXPROJ_REL);
        let src = fs::read_to_string(&path).unwrap();
        assert_eq!(src.matches(SCAFFOLD_TARGET).count(), 3);
        // Raise the first two configurations only.
        let raised = src
            .replacen(SCAFFOLD_TARGET, "IPHONEOS_DEPLOYMENT_TARGET = 17.0;", 2)
            .replacen(SCAFFOLD_TARGET, "IPHONEOS_DEPLOYMENT_TARGET = 16.0;", 1);
        fs::write(&path, &raised).unwrap();
        let err =
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("16.4"), None), &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::PlatformFloorTooLow { found, .. } if found == "16.0"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), raised);
        let _ = fs::remove_dir_all(&root);
    }

    /// A project carrying a flavor natively has a fourth build configuration;
    /// its deployment target counts like the other three.
    #[test]
    fn a_fourth_flavor_configuration_below_the_floor_refuses() {
        let root = scaffold_project("floor-ios-flavor");
        rewrite(
            &root,
            PBXPROJ_REL,
            SCAFFOLD_TARGET,
            "IPHONEOS_DEPLOYMENT_TARGET = 17.0;",
        );
        rewrite(
            &root,
            PBXPROJ_REL,
            "/* End XCBuildConfiguration section */",
            "\t\tABCDABCDABCDABCDABCD00F0 /* Debug-dev */ = {\n\
             \t\t\tisa = XCBuildConfiguration;\n\
             \t\t\tbuildSettings = {\n\
             \t\t\t\tIPHONEOS_DEPLOYMENT_TARGET = 15.1;\n\
             \t\t\t};\n\
             \t\t\tname = \"Debug-dev\";\n\
             \t\t};\n\
             /* End XCBuildConfiguration section */",
        );
        let pbxproj = fs::read_to_string(root.join(PBXPROJ_REL)).unwrap();
        assert_eq!(pbxproj.matches("isa = XCBuildConfiguration;").count(), 7);
        let err =
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("16.4"), None), &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::PlatformFloorTooLow { found, .. } if found == "15.1"),
            "{err}"
        );
        // At 15.0 the flavor configuration meets the floor too.
        add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("15.0"), None), &[]).unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_min_android_sdk_above_min_sdk_value_is_refused_and_writes_nothing() {
        let root = scaffold_project("floor-android-refused");
        let before = snapshot_tree(&root);
        let err = add_floor_spec(&root, &floor_spec(FLOOR_BASE, None, Some(31)), &[]).unwrap_err();
        match &err {
            PluginAddError::PlatformFloorTooLow {
                platform,
                required,
                found,
                key,
                file,
                ..
            } => {
                assert_eq!(*platform, FloorPlatform::Android);
                assert_eq!(required, "31");
                assert_eq!(found, "29");
                assert_eq!(*key, MIN_SDK_VALUE);
                assert_eq!(*file, APP_BUILD_GRADLE_REL);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(
            err.to_string(),
            "plugin `floor-test-plugin` needs Android minSdk 31 or later; \
             android/app/build.gradle.kts sets minSdkValue = 29. Raise it (and any product \
             flavor's own `minSdk` below it), then re-run."
        );
        assert_eq!(snapshot_tree(&root), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn min_sdk_value_at_or_above_the_floor_lets_the_add_proceed() {
        for raised in [31, 34] {
            let root = scaffold_project("floor-android-raised");
            rewrite(
                &root,
                APP_BUILD_GRADLE_REL,
                "val minSdkValue = 29",
                &format!("val minSdkValue = {raised}"),
            );
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, None, Some(31)), &[])
                .unwrap_or_else(|e| panic!("{raised}: {e}"));
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// Without `minSdkValue`, a literal `defaultConfig` `minSdk` is read, and
    /// the message names that key.
    #[test]
    fn a_literal_min_sdk_is_read_when_min_sdk_value_is_absent() {
        let root = scaffold_project("floor-android-literal");
        rewrite(
            &root,
            APP_BUILD_GRADLE_REL,
            "val minSdkValue = 29",
            "// floor below",
        );
        rewrite(
            &root,
            APP_BUILD_GRADLE_REL,
            "minSdk = minSdkValue",
            "minSdk = 30",
        );
        let err = add_floor_spec(&root, &floor_spec(FLOOR_BASE, None, Some(31)), &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::PlatformFloorTooLow { key, found, .. }
                if *key == MIN_SDK && found == "30"),
            "{err}"
        );
        assert!(err.to_string().contains("sets minSdk = 30."), "{err}");
        add_floor_spec(&root, &floor_spec(FLOOR_BASE, None, Some(30)), &[]).unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_floor_that_cannot_be_read_is_malformed_naming_the_key() {
        let root = scaffold_project("floor-unreadable");
        rewrite(
            &root,
            APP_BUILD_GRADLE_REL,
            "val minSdkValue = 29",
            "val minSdkValue = libs.versions.minSdk.get().toInt()",
        );
        let cargo_before = fs::read(root.join(CARGO_TOML_REL)).unwrap();
        let err = add_floor_spec(&root, &floor_spec(FLOOR_BASE, None, Some(31)), &[]).unwrap_err();
        match &err {
            PluginAddError::MalformedProjectFile(msg) => {
                assert!(msg.starts_with(APP_BUILD_GRADLE_REL), "{msg}");
                assert!(msg.contains("`minSdkValue = libs."), "{msg}");
            }
            other => panic!("unexpected error: {other}"),
        }

        rewrite(
            &root,
            PBXPROJ_REL,
            SCAFFOLD_TARGET,
            "IPHONEOS_DEPLOYMENT_TARGET = \"$(BASE)\";",
        );
        let err =
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("16.4"), None), &[]).unwrap_err();
        match &err {
            PluginAddError::MalformedProjectFile(msg) => {
                assert!(msg.starts_with(PBXPROJ_REL), "{msg}");
                assert!(
                    msg.contains("`IPHONEOS_DEPLOYMENT_TARGET = $(BASE)`"),
                    "{msg}"
                );
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(fs::read(root.join(CARGO_TOML_REL)).unwrap(), cargo_before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_floor_key_missing_entirely_is_malformed_naming_the_key() {
        let root = scaffold_project("floor-missing-key");
        rewrite(&root, PBXPROJ_REL, SCAFFOLD_TARGET, "SDKROOT = iphoneos;");
        let err =
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("16.4"), None), &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(msg)
                if msg.contains(IOS_DEPLOYMENT_TARGET)),
            "{err}"
        );
        rewrite(&root, APP_BUILD_GRADLE_REL, "val minSdkValue = 29", "");
        rewrite(&root, APP_BUILD_GRADLE_REL, "minSdk = minSdkValue", "");
        let err = add_floor_spec(&root, &floor_spec(FLOOR_BASE, None, Some(31)), &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(msg)
                if msg.contains(MIN_SDK_VALUE) && msg.contains("`minSdk = <n>`")),
            "{err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A platform the app has no directory for has no floor to check.
    #[test]
    fn a_missing_platform_directory_skips_its_check() {
        const CARGO_ONLY: &[Contribution] = &[Contribution::CargoDep {
            name: Cow::Borrowed("frust-floor-test-plugin"),
        }];
        let root = scaffold_project("floor-no-platforms");
        fs::remove_dir_all(root.join(IOS_DIR_REL)).unwrap();
        fs::remove_dir_all(root.join(ANDROID_DIR_REL)).unwrap();
        let report =
            add_floor_spec(&root, &floor_spec(CARGO_ONLY, Some("99.0"), Some(99)), &[]).unwrap();
        assert_eq!(report.items.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    /// A feature's floor counts only when that feature is requested, and only
    /// raises the requirement.
    #[test]
    fn a_feature_floor_raises_the_requirement_only_when_requested() {
        let root = scaffold_project("floor-feature");
        rewrite(
            &root,
            PBXPROJ_REL,
            SCAFFOLD_TARGET,
            "IPHONEOS_DEPLOYMENT_TARGET = 16.4;",
        );
        rewrite(
            &root,
            APP_BUILD_GRADLE_REL,
            "val minSdkValue = 29",
            "val minSdkValue = 31",
        );
        let spec = floor_spec(FLOOR_BASE, Some("16.4"), Some(31));
        let before = snapshot_tree(&root);
        let err = add_floor_spec(&root, &spec, &["needs-17"]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::PlatformFloorTooLow { platform: FloorPlatform::Ios,
                required, .. } if required == "17.0"),
            "{err}"
        );
        assert_eq!(snapshot_tree(&root), before);
        add_floor_spec(&root, &spec, &[]).unwrap();

        assert_eq!(
            floor_requirement(&spec, &["needs-17"]).unwrap(),
            FloorRequirement {
                ios: Some("17.0"),
                android_sdk: Some(33),
            }
        );
        assert_eq!(
            floor_requirement(&spec, &[]).unwrap(),
            FloorRequirement {
                ios: Some("16.4"),
                android_sdk: Some(31),
            }
        );
        // A feature below its spec never lowers the requirement.
        let high = floor_spec(FLOOR_BASE, Some("18.0"), Some(35));
        assert_eq!(
            floor_requirement(&high, &["needs-17"]).unwrap(),
            FloorRequirement {
                ios: Some("18.0"),
                android_sdk: Some(35),
            }
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_registry_min_ios_that_is_not_a_version_is_refused_before_any_write() {
        let root = scaffold_project("floor-invalid");
        let before = snapshot_tree(&root);
        assert!(matches!(
            add_floor_spec(&root, &floor_spec(FLOOR_BASE, Some("sixteen"), None), &[]),
            Err(PluginAddError::InvalidContribution(msg)) if msg.contains("`sixteen`")
        ));
        assert_eq!(snapshot_tree(&root), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_assignment_reads_declarations_and_skips_comments() {
        let src = "// val minSdkValue = 99\n\
                   /* minSdk = 98 */\n\
                   val minSdkValue: Int = 30 // the floor\n\
                   minSdkVersion = 12\n\
                   minSdk == 5\n\
                   minSdk = 24;\n";
        assert_eq!(gradle_assignment(src, MIN_SDK_VALUE), Some("30"));
        assert_eq!(gradle_assignment(src, MIN_SDK), Some("24"));
        assert_eq!(gradle_assignment(src, "targetSdk"), None);
    }

    // -----------------------------------------------------------------------
    // `Contribution::AndroidThemeParent`
    // -----------------------------------------------------------------------

    const M3_PARENT: &str = "Theme.Material3.DayNight.NoActionBar";
    const PLATFORM_THEME: &str = "@android:style/Theme.Material.NoActionBar";

    /// A fresh scaffold as an app generated before the template defined
    /// `FrustAppTheme`: no `themes.xml`, and the manifest naming `theme`
    /// directly on both elements.
    fn without_app_theme(root: &Path, theme: &str) {
        fs::remove_file(root.join(THEMES_REL)).unwrap();
        rewrite(root, MANIFEST_REL, "@style/FrustAppTheme", theme);
    }

    fn theme_item(report: &AddReport) -> &AddItem {
        report
            .items
            .iter()
            .find(|i| i.description.starts_with("Android theme"))
            .unwrap_or_else(|| panic!("{report:?}"))
    }

    #[test]
    fn theme_parent_on_a_fresh_scaffold_rewrites_only_the_style_parent() {
        let root = scaffold_project("theme-parent-fresh");
        let before = snapshot_tree(&root);
        let themes_before = fs::read_to_string(root.join(THEMES_REL)).unwrap();

        assert_eq!(
            apply_android_theme_parent(&root, "native-widgets", M3_PARENT).unwrap(),
            AddOutcome::Applied
        );

        let mut after = snapshot_tree(&root);
        let themes = String::from_utf8(after.remove(THEMES_REL).unwrap()).unwrap();
        let mut rest = before.clone();
        rest.remove(THEMES_REL);
        assert_eq!(after, rest, "only themes.xml changes");
        assert_eq!(
            themes,
            themes_before.replace(
                "parent=\"android:Theme.Material.NoActionBar\"",
                &format!("parent=\"{M3_PARENT}\"")
            )
        );
        assert_ne!(themes, themes_before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn theme_parent_twice_is_already_present_and_writes_nothing() {
        let root = scaffold_project("theme-parent-twice");
        let first = add_plugin(&root, "native-widgets", &[]).unwrap();
        assert_eq!(theme_item(&first).outcome, AddOutcome::Applied, "{first:?}");
        assert_eq!(
            theme_item(&first).description,
            format!("Android theme `FrustAppTheme` parent `{M3_PARENT}`")
        );

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "native-widgets", &[]).unwrap();
        assert_eq!(
            theme_item(&second).outcome,
            AddOutcome::AlreadyPresent,
            "{second:?}"
        );
        assert_eq!(
            apply_android_theme_parent(&root, "native-widgets", M3_PARENT).unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(snapshot_tree(&root), after_first);
        let _ = fs::remove_dir_all(&root);
    }

    /// An app whose manifest already names an accepted Material style (the
    /// in-tree native-widgets demo's shape) needs no `FrustAppTheme`.
    #[test]
    fn a_material3_manifest_theme_is_already_present() {
        for theme in [
            "@style/Theme.Material3.DayNight.NoActionBar",
            "@style/Theme.Material3Expressive.DayNight.NoActionBar",
            "@style/Theme.MaterialComponents.DayNight.NoActionBar",
        ] {
            let root = scaffold_project("theme-parent-m3-manifest");
            without_app_theme(&root, theme);
            let report = add_plugin(&root, "native-widgets", &[]).unwrap();
            assert_eq!(
                theme_item(&report).outcome,
                AddOutcome::AlreadyPresent,
                "{theme}: {report:?}"
            );
            assert!(
                !root.join(THEMES_REL).exists(),
                "{theme}: nothing is created"
            );
            assert!(
                fs::read_to_string(root.join(MANIFEST_REL))
                    .unwrap()
                    .contains(theme)
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// An app with neither the style nor a Material 3 manifest theme is
    /// refused before anything is written, the Cargo dependency and Gradle
    /// include included, and the refusal names both manual edits.
    #[test]
    fn an_app_without_the_style_or_a_material3_theme_is_refused_untouched() {
        let root = scaffold_project("theme-parent-refused");
        without_app_theme(&root, PLATFORM_THEME);
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        let settings = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        let before = snapshot_tree(&root);

        let err = add_plugin(&root, "native-widgets", &[]).unwrap_err();
        assert!(
            matches!(
                &err,
                PluginAddError::AndroidAppThemeMissing { plugin, parent }
                    if &**plugin == "native-widgets" && *parent == M3_PARENT
            ),
            "{err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains(&format!(
                "`<style name=\"FrustAppTheme\" parent=\"{M3_PARENT}\" />`"
            )),
            "{text}"
        );
        assert!(
            text.contains("android/app/src/main/res/values/themes.xml"),
            "{text}"
        );
        assert!(
            text.contains("android:theme=\"@style/FrustAppTheme\" on both the <application> and the <activity>"),
            "{text}"
        );
        assert!(
            text.contains("android/app/src/main/AndroidManifest.xml"),
            "{text}"
        );

        assert_eq!(
            fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap(),
            cargo
        );
        assert_eq!(
            fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings
        );
        assert_eq!(snapshot_tree(&root), before);
        assert!(matches!(
            apply_android_theme_parent(&root, "native-widgets", M3_PARENT),
            Err(PluginAddError::AndroidAppThemeMissing { .. })
        ));
        let _ = fs::remove_dir_all(&root);
    }

    /// Once the two manual edits are made, the add goes through.
    #[test]
    fn the_refused_app_is_addable_after_the_two_named_edits() {
        let root = scaffold_project("theme-parent-fixed");
        let themes = fs::read_to_string(root.join(THEMES_REL)).unwrap();
        without_app_theme(&root, PLATFORM_THEME);
        assert!(add_plugin(&root, "native-widgets", &[]).is_err());

        fs::write(root.join(THEMES_REL), themes).unwrap();
        rewrite(&root, MANIFEST_REL, PLATFORM_THEME, "@style/FrustAppTheme");
        let report = add_plugin(&root, "native-widgets", &[]).unwrap();
        assert_eq!(
            theme_item(&report).outcome,
            AddOutcome::Applied,
            "{report:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_app_without_android_skips_the_theme_parent() {
        let root = scaffold_project("theme-parent-no-android");
        fs::remove_dir_all(root.join(ANDROID_DIR_REL)).unwrap();
        assert_eq!(
            android_theme_parent(&root, M3_PARENT).unwrap(),
            ThemeParent::Satisfied
        );
        assert_eq!(
            apply_android_theme_parent(&root, "native-widgets", M3_PARENT).unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert!(!root.join(ANDROID_DIR_REL).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_theme_parent_that_is_not_a_style_name_is_refused() {
        let root = scaffold_project("theme-parent-invalid");
        for parent in ["", "Theme\" foo=\"x", "a b", "<x>"] {
            assert!(
                matches!(
                    android_theme_parent(&root, parent),
                    Err(PluginAddError::InvalidContribution(_))
                ),
                "{parent:?}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn style_parent_rewrite_touches_only_the_named_style() {
        let src = "<resources>\n\
                   \x20   <!-- <style name=\"FrustAppTheme\" parent=\"Commented\" /> -->\n\
                   \x20   <style name=\"Other\" parent=\"Keep\" />\n\
                   \x20   <style parent='Old' name='FrustAppTheme'>\n\
                   \x20       <item name=\"android:x\">y</item>\n\
                   \x20   </style>\n\
                   </resources>\n";
        let out = set_style_parent(src, APP_THEME_STYLE, "New")
            .unwrap()
            .unwrap();
        assert_eq!(out, src.replace("parent='Old'", "parent='New'"));
        assert_eq!(set_style_parent(&out, APP_THEME_STYLE, "New"), Some(None));
        assert_eq!(set_style_parent(src, "Missing", "New"), None);

        let bare = "<resources><style name=\"FrustAppTheme\"/></resources>";
        assert_eq!(
            set_style_parent(bare, APP_THEME_STYLE, "New")
                .unwrap()
                .unwrap(),
            "<resources><style name=\"FrustAppTheme\" parent=\"New\"/></resources>"
        );
        // `<styles>` is not a `<style>`.
        assert_eq!(
            set_style_parent("<styles name=\"FrustAppTheme\"/>", APP_THEME_STYLE, "New"),
            None
        );
    }

    #[test]
    fn the_activity_theme_falls_back_to_the_application_theme() {
        let manifest = |application: &str, activity: &str| {
            format!(
                "<manifest>\n<application android:label=\"a > b\"{application}>\n\
                 <!-- <activity android:theme=\"@style/Theme.Material3.Light\"> -->\n\
                 <activity android:name=\".MainActivity\"{activity}>\n\
                 </activity>\n</application>\n</manifest>\n"
            )
        };
        let theme = |name: &str| format!(" android:theme=\"{name}\"");
        let effective = |application: &str, activity: &str| {
            activity_theme(&manifest(&theme(application), &theme(activity)))
        };
        assert_eq!(
            activity_theme(&manifest("", &theme(M3_PARENT))).as_deref(),
            Some(M3_PARENT)
        );
        assert_eq!(
            activity_theme(&manifest(&theme("@style/Theme.Material3"), "")).as_deref(),
            Some("@style/Theme.Material3")
        );
        assert_eq!(
            effective("@style/Theme.Material3.DayNight", PLATFORM_THEME).as_deref(),
            Some(PLATFORM_THEME)
        );
        assert_eq!(activity_theme(&manifest("", "")), None);
    }

    #[test]
    fn the_accepted_material_families_are_name_prefixes_ending_at_a_dot() {
        for name in [
            "Theme.Material3",
            "Theme.Material3.DayNight.NoActionBar",
            "Theme.Material3.Light.NoActionBar",
            "@style/Theme.Material3Expressive",
            "Theme.Material3Expressive.DayNight.NoActionBar",
            "Theme.MaterialComponents",
            "@style/Theme.MaterialComponents.DayNight.NoActionBar",
        ] {
            assert!(is_accepted_material_theme(name), "{name}");
        }
        for name in [
            "",
            "Theme.Material3Fake",
            "Theme.MaterialComponentsX.Light",
            "Theme.Material",
            "android:Theme.Material.NoActionBar",
            PLATFORM_THEME,
            "@style/FrustAppTheme",
            "Theme.AppCompat.Light",
            "@android:style/Theme.Material3.Light",
        ] {
            assert!(!is_accepted_material_theme(name), "{name}");
        }
    }

    /// `themes.xml` holding `styles` as its whole `<resources>` body.
    fn write_themes(root: &Path, styles: &str) -> String {
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n{styles}</resources>\n"
        );
        fs::write(root.join(THEMES_REL), &text).unwrap();
        text
    }

    /// A deliberately chosen Material parent on `FrustAppTheme` is the app's:
    /// the add answers already present, nothing is pending, the file is
    /// untouched.
    #[test]
    fn an_accepted_material_parent_is_already_present_and_untouched() {
        for parent in [
            "Theme.Material3.Light.NoActionBar",
            "Theme.Material3Expressive.DayNight.NoActionBar",
            "@style/Theme.MaterialComponents.DayNight.NoActionBar",
        ] {
            let root = scaffold_project("theme-parent-accepted");
            let themes = write_themes(
                &root,
                &format!("    <style name=\"FrustAppTheme\" parent=\"{parent}\" />\n"),
            );
            assert_eq!(
                android_theme_parent(&root, M3_PARENT).unwrap(),
                ThemeParent::Satisfied,
                "{parent}"
            );
            let report = add_plugin(&root, "native-widgets", &[]).unwrap();
            assert_eq!(
                theme_item(&report).outcome,
                AddOutcome::AlreadyPresent,
                "{parent}: {report:?}"
            );
            assert_eq!(fs::read_to_string(root.join(THEMES_REL)).unwrap(), themes);
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// An app-owned parent defined in the same file is followed to the first
    /// Material or platform ancestor; only a platform one is rewritten, and
    /// then only `FrustAppTheme`'s own parent.
    #[test]
    fn an_app_style_parent_is_followed_to_its_material_or_platform_ancestor() {
        let root = scaffold_project("theme-parent-app-style");
        for styles in [
            "    <style name=\"Brand\" parent=\"Theme.Material3.Light.NoActionBar\" />\n\
             \x20   <style name=\"FrustAppTheme\" parent=\"Brand\" />\n",
            // A dotted style without a parent inherits from its prefix.
            "    <style name=\"Brand\" parent=\"@style/Theme.MaterialComponents.Light\" />\n\
             \x20   <style name=\"Brand.Dark\" />\n\
             \x20   <style name=\"FrustAppTheme\" parent=\"@style/Brand.Dark\" />\n",
        ] {
            let themes = write_themes(&root, styles);
            assert_eq!(
                android_theme_parent(&root, M3_PARENT).unwrap(),
                ThemeParent::Satisfied,
                "{styles}"
            );
            assert_eq!(
                apply_android_theme_parent(&root, "native-widgets", M3_PARENT).unwrap(),
                AddOutcome::AlreadyPresent
            );
            assert_eq!(fs::read_to_string(root.join(THEMES_REL)).unwrap(), themes);
        }

        let themes = write_themes(
            &root,
            "    <style name=\"Base\" parent=\"@android:style/Theme.Material.Light\" />\n\
             \x20   <style name=\"Brand\" parent=\"Base\" />\n\
             \x20   <style name=\"FrustAppTheme\" parent=\"Brand\" />\n",
        );
        let expected = themes.replace(
            "name=\"FrustAppTheme\" parent=\"Brand\"",
            &format!("name=\"FrustAppTheme\" parent=\"{M3_PARENT}\""),
        );
        assert_ne!(expected, themes);
        assert_eq!(
            android_theme_parent(&root, M3_PARENT).unwrap(),
            ThemeParent::Rewrite(expected.clone())
        );
        assert_eq!(
            apply_android_theme_parent(&root, "native-widgets", M3_PARENT).unwrap(),
            AddOutcome::Applied
        );
        assert_eq!(fs::read_to_string(root.join(THEMES_REL)).unwrap(), expected);
        let _ = fs::remove_dir_all(&root);
    }

    /// A parent this file cannot resolve — a style defined elsewhere, a
    /// cycle, `FrustAppTheme` naming itself — is the app's and never
    /// rewritten.
    #[test]
    fn an_unresolvable_app_style_parent_is_satisfied() {
        let root = scaffold_project("theme-parent-unresolved");
        for styles in [
            "    <style name=\"FrustAppTheme\" parent=\"Theme.AppCompat.Light.NoActionBar\" />\n",
            "    <style name=\"FrustAppTheme\" parent=\"@style/Brand\" />\n",
            "    <style name=\"A\" parent=\"B\" />\n\
             \x20   <style name=\"B\" parent=\"A\" />\n\
             \x20   <style name=\"FrustAppTheme\" parent=\"A\" />\n",
            "    <style name=\"FrustAppTheme\" parent=\"FrustAppTheme\" />\n",
        ] {
            let themes = write_themes(&root, styles);
            assert_eq!(
                android_theme_parent(&root, M3_PARENT).unwrap(),
                ThemeParent::Satisfied,
                "{styles}"
            );
            assert_eq!(
                apply_android_theme_parent(&root, "native-widgets", M3_PARENT).unwrap(),
                AddOutcome::AlreadyPresent
            );
            assert_eq!(fs::read_to_string(root.join(THEMES_REL)).unwrap(), themes);
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// The style only counts when the activity uses it: a manifest still on
    /// a platform theme is refused although `themes.xml` defines
    /// `FrustAppTheme`, and nothing is written.
    #[test]
    fn a_defined_style_the_manifest_does_not_use_is_refused_untouched() {
        let root = scaffold_project("theme-parent-unused-style");
        rewrite(&root, MANIFEST_REL, "@style/FrustAppTheme", PLATFORM_THEME);
        assert!(root.join(THEMES_REL).is_file());
        let before = snapshot_tree(&root);
        assert_eq!(
            android_theme_parent(&root, M3_PARENT).unwrap(),
            ThemeParent::NoAppTheme
        );
        let err = add_plugin(&root, "native-widgets", &[]).unwrap_err();
        assert!(
            matches!(err, PluginAddError::AndroidAppThemeMissing { .. }),
            "{err:?}"
        );
        assert_eq!(snapshot_tree(&root), before);
        let _ = fs::remove_dir_all(&root);
    }

    /// The manifest uses `FrustAppTheme` but `themes.xml` does not define
    /// it: refused.
    #[test]
    fn a_used_style_the_resources_lack_is_refused() {
        let root = scaffold_project("theme-parent-undefined-style");
        write_themes(
            &root,
            "    <style name=\"Other\" parent=\"Theme.Material3\" />\n",
        );
        assert_eq!(
            android_theme_parent(&root, M3_PARENT).unwrap(),
            ThemeParent::NoAppTheme
        );
        fs::remove_file(root.join(THEMES_REL)).unwrap();
        assert_eq!(
            android_theme_parent(&root, M3_PARENT).unwrap(),
            ThemeParent::NoAppTheme
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn style_parent_reads_the_first_named_style() {
        let src = "<resources>\n\
                   <!-- <style name=\"A\" parent=\"Commented\" /> -->\n\
                   <style name=\"A\" parent='First' />\n\
                   <style name=\"A\" parent=\"Second\" />\n\
                   <style name=\"Bare\" />\n\
                   </resources>\n";
        assert_eq!(style_parent(src, "A"), Some(Some("First")));
        assert_eq!(style_parent(src, "Bare"), Some(None));
        assert_eq!(style_parent(src, "Missing"), None);
    }
}
