//! Which of a plugin's tracked contributions an app still lacks
//! ([`pending_contributions`]), applying every manifest-described plugin the
//! app depends on ([`sync_plugins`]), and the warning the machine-local
//! wiring returns when two plugins claim one native module name
//! ([`ModuleCollision`]). Every "already present" answer here is the very
//! predicate the matching applier in `apply.rs` skips by, so the two cannot
//! drift. Reads only; [`sync_plugins`] writes through the appliers.
//!
//! Over a whole catalog: which plugins the app depends on
//! ([`installed_plugin_ids`]), where each stands ([`plugin_statuses`], what
//! `frust plugin list` shows) and whether a native build must stop because a
//! manifest-described plugin is not wired in yet ([`pending_stop`]).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

use super::apply::{self, DepLookup};
use super::catalog::{CatalogEntry, PluginCatalog, package_of};
use super::{AddOutcome, AddReport, Contribution, NativeKind, PluginAddError};
use crate::packages::{PackageLocator, PackagesError};

/// One tracked contribution of a plugin that the app does not carry yet —
/// one [`pending_contributions`] row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingContribution {
    /// The owning plugin's id.
    pub plugin_id: Cow<'static, str>,
    /// The missing contribution, as the catalog states it.
    pub contribution: Contribution,
    /// Which edit of `contribution` is missing.
    step: PendingStep,
}

/// The edit of a contribution a [`PendingContribution`] row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingStep {
    /// The contribution itself, as the catalog states it.
    Contribution,
    /// The `.gitignore` line for the `ios/<package>` link a manifest-described
    /// plugin's [`Contribution::SwiftPackageRef`] makes.
    GitignoreLink,
    /// A [`Contribution::AndroidThemeParent`] the add refuses: the app's
    /// activity does not use a defined `FrustAppTheme` and names no accepted
    /// Material theme, so the missing edits are the app's own.
    AppThemeMissing,
}

impl PendingContribution {
    /// The missing edit's report label: [`Contribution::describe`], or for
    /// the ignore-file line of a Swift package link, that line.
    pub fn describe(&self) -> String {
        match (&self.contribution, self.step) {
            (Contribution::SwiftPackageRef { package_name, .. }, PendingStep::GitignoreLink) => {
                apply::gitignore_link_label(package_name)
            }
            _ => self.contribution.describe(),
        }
    }

    /// The project edits to make by hand before adding the plugin applies
    /// this row, as one clause, or `None` when the add applies it itself.
    /// `Some` exactly for a [`Contribution::AndroidThemeParent`] the add
    /// refuses with [`PluginAddError::AndroidAppThemeMissing`]: the
    /// `FrustAppTheme` style in `res/values/themes.xml` and the manifest
    /// theme naming it.
    pub fn manual_edits(&self) -> Option<String> {
        match (&self.contribution, self.step) {
            (Contribution::AndroidThemeParent { parent }, PendingStep::AppThemeMissing) => {
                let style = apply::APP_THEME_STYLE;
                Some(format!(
                    "add `<style name=\"{style}\" parent=\"{parent}\" />` inside `<resources>` \
                     in {themes} (create the file if it is missing) and set \
                     android:theme=\"@style/{style}\" on both the <application> and the \
                     <activity> element of {manifest}",
                    themes = apply::THEMES_REL,
                    manifest = apply::MANIFEST_REL,
                ))
            }
            _ => None,
        }
    }
}

/// The tracked contributions of `entry` — its base and the requested
/// `features`' — that the project at `project_root` does not carry yet, in
/// report order, computed without writing anything. Each is judged by the
/// predicate its applier skips by: the dependency line, the Cargo feature,
/// the `uses-permission`, the `Info.plist` `<key>`, both Gradle include
/// lines, the repository url, the `libc++_shared.so` property (read by the
/// build script and set in `gradle.properties`), the `FrustAppTheme` parent (or an activity
/// theme that is already an accepted Material one), all six pbxproj package sites, the framework
/// or library in every `OTHER_LDFLAGS` list, a manifest-described plugin's `ios/<package>`
/// link in the root `.gitignore` (when the app has one), and an entitlement's key in every file the
/// Runner configurations sign with plus `CODE_SIGN_ENTITLEMENTS` in each of
/// them. Machine-local wiring (`local.properties` keys, `ios/` links) and
/// the desktop lane, applied by every build, are never pending. A project
/// file a contribution needs that is missing or unreadable fails exactly as
/// adding the plugin would — except that a manifest-described plugin's
/// contributions for a platform whose directory (`android/`, `ios/`) the
/// project lacks are skipped, never pending, the way the machine-local
/// wiring skips that platform.
pub fn pending_contributions(
    project_root: &Path,
    entry: &CatalogEntry,
    features: &[&str],
) -> Result<Vec<PendingContribution>, PluginAddError> {
    let entry = &*for_project(project_root, entry);
    let contributions = apply::selected_contributions(&entry.spec, features)?;
    let doc = apply::read_cargo_doc(project_root)?;
    let mut pending = Vec::new();
    for contribution in contributions {
        if is_pending(project_root, entry, &doc, contribution)? {
            let step = match contribution {
                Contribution::AndroidThemeParent { parent }
                    if apply::android_theme_parent(project_root, parent)?
                        == apply::ThemeParent::NoAppTheme =>
                {
                    PendingStep::AppThemeMissing
                }
                _ => PendingStep::Contribution,
            };
            pending.push(PendingContribution {
                plugin_id: entry.spec.id.clone(),
                contribution: contribution.clone(),
                step,
            });
        }
        if let Contribution::SwiftPackageRef { package_name, .. } = contribution
            && !entry.is_built_in()
            && apply::gitignore_ios_link_missing(project_root, package_name)?
        {
            pending.push(PendingContribution {
                plugin_id: entry.spec.id.clone(),
                contribution: contribution.clone(),
                step: PendingStep::GitignoreLink,
            });
        }
    }
    Ok(pending)
}

/// The platform directory a contribution edits a file under, if it edits
/// one: `android` or `ios`.
pub(super) fn platform_dir(contribution: &Contribution) -> Option<&'static str> {
    match contribution {
        Contribution::ManifestPermission { .. }
        | Contribution::GradleModule { .. }
        | Contribution::MavenRepository { .. }
        | Contribution::AndroidLibcxxShared
        | Contribution::AndroidThemeParent { .. } => Some("android"),
        Contribution::PlistEntry { .. }
        | Contribution::PlistArrayEntry { .. }
        | Contribution::SwiftPackageRef { .. }
        | Contribution::IosFramework { .. }
        | Contribution::IosLibrary { .. }
        | Contribution::IosEntitlement { .. } => Some("ios"),
        _ => None,
    }
}

/// `entry` as it applies to the project at `project_root`: a built-in entry
/// unchanged, a manifest-described one without its contributions (base and
/// features alike) for a platform whose directory the project lacks.
pub(super) fn for_project<'e>(
    project_root: &Path,
    entry: &'e CatalogEntry,
) -> Cow<'e, CatalogEntry> {
    let absent = |contribution: &Contribution| {
        platform_dir(contribution).is_some_and(|dir| !project_root.join(dir).is_dir())
    };
    let mut all = entry.spec.base.iter().chain(
        entry
            .spec
            .optional_features
            .iter()
            .flat_map(|f| f.contributions.iter()),
    );
    if entry.is_built_in() || !all.any(&absent) {
        return Cow::Borrowed(entry);
    }
    let keep = |contributions: &[Contribution]| -> Vec<Contribution> {
        contributions
            .iter()
            .filter(|c| !absent(c))
            .cloned()
            .collect()
    };
    let mut entry = entry.clone();
    entry.spec.base = Cow::Owned(keep(&entry.spec.base));
    let features = entry
        .spec
        .optional_features
        .iter()
        .map(|feature| {
            let mut feature = feature.clone();
            feature.contributions = Cow::Owned(keep(&feature.contributions));
            feature
        })
        .collect();
    entry.spec.optional_features = Cow::Owned(features);
    Cow::Owned(entry)
}

/// The text of project file `rel`, failing as its applier would.
fn read(project_root: &Path, rel: &str) -> Result<String, PluginAddError> {
    apply::read_required(&project_root.join(rel), rel)
}

/// Whether `contribution` of `entry` still has to be applied.
fn is_pending(
    project_root: &Path,
    entry: &CatalogEntry,
    doc: &DocumentMut,
    contribution: &Contribution,
) -> Result<bool, PluginAddError> {
    let root = project_root;
    let present = match contribution {
        Contribution::CargoDep { name } => apply::cargo_dep_present(doc, name, &entry.origin),
        Contribution::CargoFeature { name, feature } => {
            apply::cargo_feature_present(doc, name, feature, DepLookup::of(&entry.origin))
        }
        // No `android/` directory: no manifest to edit, so satisfied.
        Contribution::ManifestPermission { .. } if !root.join(apply::ANDROID_DIR_REL).is_dir() => {
            true
        }
        Contribution::ManifestPermission { permission } => {
            apply::manifest_permission_present(&read(root, apply::MANIFEST_REL)?, permission)
        }
        Contribution::PlistEntry { key, .. } | Contribution::PlistArrayEntry { key, .. } => {
            apply::plist_key_present(&read(root, apply::PLIST_REL)?, key)
        }
        Contribution::GradleModule { gradle_name, .. } => {
            let settings = read(root, apply::SETTINGS_GRADLE_REL)?;
            let build = read(root, apply::APP_BUILD_GRADLE_REL)?;
            apply::settings_include_present(&settings, gradle_name)
                && apply::app_dependency_present(&build, gradle_name)
        }
        Contribution::MavenRepository { url, .. } => {
            apply::maven_repository_present(&read(root, apply::SETTINGS_GRADLE_REL)?, url)
        }
        Contribution::AndroidLibcxxShared => apply::libcxx_shared_present(root)?,
        Contribution::AndroidThemeParent { parent } => {
            apply::android_theme_parent(root, parent)? == apply::ThemeParent::Satisfied
        }
        Contribution::SwiftPackageRef { package_name, .. } => {
            apply::swift_package_present(&read(root, apply::PBXPROJ_REL)?, package_name)?
        }
        // No `ios/` directory: nothing to link, so satisfied.
        Contribution::IosFramework { name } => {
            apply::ios_link_flag_satisfied(root, apply::LinkFlag::Framework(name))?
        }
        Contribution::IosLibrary { name } => {
            apply::ios_link_flag_satisfied(root, apply::LinkFlag::Library(name))?
        }
        Contribution::IosEntitlement {
            key,
            value,
            comment,
        } => apply::ios_entitlement_present(root, key, value, comment)?,
        Contribution::AppCrateMacro { invocation, .. } => {
            apply::app_crate_macro_present(&read(root, apply::LIB_RS_REL)?, invocation)
        }
        Contribution::ScaffoldFile { rel_path, .. } => {
            apply::scaffold_file_present(root, rel_path)?
        }
        Contribution::MacosPlistEntry { .. }
        | Contribution::MacosEntitlement { .. }
        | Contribution::LinuxDesktopEntry { .. } => true,
    };
    Ok(!present)
}

/// Refuse a pending contribution whose insertion anchor its target file
/// lacks — the check [`sync_plugins`] makes for every plugin before its
/// first write, since an applier only finds a missing anchor once earlier
/// plugins have been written.
fn check_anchors(
    project_root: &Path,
    pending: &[PendingContribution],
) -> Result<(), PluginAddError> {
    let require = |rel: &str, anchor: &str| -> Result<(), PluginAddError> {
        if read(project_root, rel)?.contains(anchor) {
            Ok(())
        } else {
            Err(PluginAddError::MalformedProjectFile(rel.to_string()))
        }
    };
    for row in pending {
        match &row.contribution {
            Contribution::ManifestPermission { .. } => {
                require(apply::MANIFEST_REL, apply::MANIFEST_ANCHOR)?;
            }
            Contribution::PlistEntry { .. } | Contribution::PlistArrayEntry { .. } => {
                require(apply::PLIST_REL, apply::PLIST_ANCHOR)?;
            }
            Contribution::GradleModule { gradle_name, .. } => {
                let settings = read(project_root, apply::SETTINGS_GRADLE_REL)?;
                if !apply::settings_include_present(&settings, gradle_name) {
                    require(apply::SETTINGS_GRADLE_REL, apply::SETTINGS_ANCHOR)?;
                }
                let build = read(project_root, apply::APP_BUILD_GRADLE_REL)?;
                if !apply::app_dependency_present(&build, gradle_name) {
                    require(apply::APP_BUILD_GRADLE_REL, apply::APP_DEPS_ANCHOR)?;
                }
            }
            Contribution::MavenRepository { .. } => {
                require(apply::SETTINGS_GRADLE_REL, apply::REPOSITORIES_ANCHOR)?;
            }
            Contribution::AndroidLibcxxShared => {
                if !apply::libcxx_shared_reader_present(&read(
                    project_root,
                    apply::APP_BUILD_GRADLE_REL,
                )?) {
                    return Err(apply::missing_libcxx_shared_reader());
                }
            }
            // Pending means rewritable or unthemed; only the second refuses.
            Contribution::AndroidThemeParent { parent }
                if row.step == PendingStep::AppThemeMissing =>
            {
                return Err(apply::android_app_theme_missing(&row.plugin_id, parent));
            }
            // The pbxproj sites, `OTHER_LDFLAGS` lists and Runner
            // configurations were resolved by the pending check itself.
            _ => {}
        }
    }
    Ok(())
}

/// What [`sync_plugins`] did: one [`AddReport`] per manifest-described
/// plugin the app depends on, in catalog order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Per plugin, every base contribution applied or already present.
    pub plugins: Vec<AddReport>,
}

impl SyncReport {
    /// Whether any plugin's contribution was applied this run.
    pub fn changed(&self) -> bool {
        self.plugins
            .iter()
            .flat_map(|report| &report.items)
            .any(|item| item.outcome == AddOutcome::Applied)
    }
}

/// Apply the base contributions of every manifest-described plugin in
/// `catalog` that the project at `project_root` depends on (a non-optional
/// entry on its package in `[dependencies]` or a target table) — optional
/// features are chosen with an add, never here. Built-in plugins are left
/// to `add_plugin`. Every plugin is checked before the first write — its
/// platform floors, native paths, entitlement plan, Maven marker and the
/// insertion anchor of each pending contribution — so a refusal leaves the
/// project untouched. Idempotent like `add_plugin`: a second run reports
/// every item [`AddOutcome::AlreadyPresent`]. A contribution for a platform
/// whose directory the project lacks is skipped, as in
/// [`pending_contributions`]: neither applied nor reported.
pub fn sync_plugins(
    project_root: &Path,
    catalog: &PluginCatalog,
) -> Result<SyncReport, PluginAddError> {
    let doc = apply::read_cargo_doc(project_root)?;
    let entries: Vec<Cow<'_, CatalogEntry>> = catalog
        .iter()
        .filter(|entry| !entry.is_built_in() && apply::entry_is_installed(entry, &doc))
        .map(|entry| for_project(project_root, entry))
        .collect();
    for entry in &entries {
        apply::prepare_add(project_root, entry, &[])?;
        check_anchors(
            project_root,
            &pending_contributions(project_root, entry, &[])?,
        )?;
    }
    let mut plugins = Vec::with_capacity(entries.len());
    for entry in &entries {
        plugins.push(apply::add_entry_with(
            &CatalogLocated,
            project_root,
            entry,
            &[],
        )?);
    }
    Ok(SyncReport { plugins })
}

/// The locator a manifest-described plugin's add is given: its package
/// directory comes from its catalog entry, so cargo is never asked.
struct CatalogLocated;

impl PackageLocator for CatalogLocated {
    fn locate_many(
        &self,
        project_dir: &Path,
        packages: &[&str],
    ) -> Result<Vec<PathBuf>, PackagesError> {
        Err(PackagesError::MetadataUnreadable {
            project_dir: project_dir.to_path_buf(),
            reason: format!(
                "{packages:?}: a manifest-described plugin is located by its catalog entry"
            ),
        })
    }
}

/// Two installed plugins declare the same Gradle module or Swift package
/// name; the machine-local wiring can point the name at only one of them.
/// A built-in plugin wins, otherwise the first by package name; the other's
/// module is left unwired. Returned by `platform_wiring::sync_with_catalog`
/// in its report, never printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCollision {
    /// The module, with the name both declare.
    pub kind: NativeKind,
    /// The plugin whose module is wired.
    pub kept_plugin_id: String,
    /// The plugin whose module is not wired.
    pub dropped_plugin_id: String,
}

impl fmt::Display for ModuleCollision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let module = match &self.kind {
            NativeKind::GradleModule { gradle_name } => format!("Android module `{gradle_name}`"),
            NativeKind::SwiftPackage { package_name } => format!("iOS package `{package_name}`"),
        };
        write!(
            f,
            "{module}: declared by plugins `{}` and `{}`; `{}`'s is wired, `{}`'s is not",
            self.kept_plugin_id,
            self.dropped_plugin_id,
            self.kept_plugin_id,
            self.dropped_plugin_id
        )
    }
}

/// The ids of the plugins of `catalog` the app at `project_root` depends on,
/// by the one rule ([`apply::is_installed`]) `sync_plugins`, the wiring and
/// the add use. A missing or unparseable `Cargo.toml` is the error an add
/// would give.
pub fn installed_plugin_ids(
    project_root: &Path,
    catalog: &PluginCatalog,
) -> Result<BTreeSet<String>, PluginAddError> {
    let mut ids = BTreeSet::new();
    for entry in catalog {
        if apply::is_installed(project_root, entry)? {
            ids.insert(entry.spec.id.to_string());
        }
    }
    Ok(ids)
}

/// Where one catalog plugin stands in an app ([`plugin_statuses`]).
/// `Display` is the status as `frust plugin list` shows it; empty for a
/// plugin the app does not depend on.
#[derive(Debug)]
pub enum PluginStatus {
    /// The app does not depend on the plugin.
    NotInstalled,
    /// The app depends on it and carries every tracked base contribution.
    Added,
    /// The app depends on it; this many tracked base contributions are
    /// pending.
    Pending(usize),
    /// The app depends on it, and checking its contributions failed.
    CheckFailed(PluginAddError),
}

impl fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => Ok(()),
            Self::Added => f.write_str("added"),
            Self::Pending(count) => write!(f, "{count} pending"),
            Self::CheckFailed(err) => write!(f, "pending check failed: {err}"),
        }
    }
}

/// Every plugin of `catalog` with its [`PluginStatus`] in the app at
/// `project_root` (base contributions only), in listing order: the built-in
/// entries in catalog order, then the discovered ones by id. Fails only when
/// the app's `Cargo.toml` cannot be read ([`installed_plugin_ids`]).
pub fn plugin_statuses<'c>(
    project_root: &Path,
    catalog: &'c PluginCatalog,
) -> Result<Vec<(&'c CatalogEntry, PluginStatus)>, PluginAddError> {
    let installed = installed_plugin_ids(project_root, catalog)?;
    let mut discovered: Vec<&CatalogEntry> = catalog
        .iter()
        .filter(|entry| !entry.is_built_in())
        .collect();
    discovered.sort_by(|a, b| a.spec.id.cmp(&b.spec.id));
    Ok(catalog
        .iter()
        .filter(|entry| entry.is_built_in())
        .chain(discovered)
        .map(|entry| {
            let status = if !installed.contains(&*entry.spec.id) {
                PluginStatus::NotInstalled
            } else {
                match pending_contributions(project_root, entry, &[]) {
                    Ok(pending) if pending.is_empty() => PluginStatus::Added,
                    Ok(pending) => PluginStatus::Pending(pending.len()),
                    Err(err) => PluginStatus::CheckFailed(err),
                }
            };
            (entry, status)
        })
        .collect())
}

/// One installed manifest-described plugin whose tracked project edits are
/// still pending — a [`PendingStop`] block. `Display` is the block: one line
/// naming the plugin, then one `  - ` line per pending item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPlugin {
    /// The plugin's id.
    pub plugin_id: String,
    /// The package describing it.
    pub package: String,
    /// Its pending base contributions, in report order; never empty.
    pub pending: Vec<PendingContribution>,
}

impl fmt::Display for PendingPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "plugin {} ({}) is not wired into this project: {} change(s) pending. \
             Run `frust plugin sync` and re-run.",
            self.plugin_id,
            self.package,
            self.pending.len()
        )?;
        for item in &self.pending {
            write!(f, "\n  - {}", item.describe())?;
        }
        Ok(())
    }
}

/// A pending check [`pending_stop`] could not make; never a stop. `Display`
/// is the whole warning line, `warning: ` included.
#[derive(Debug)]
pub enum PendingWarning {
    /// Which plugins the app depends on could not be read.
    Unchecked(PluginAddError),
    /// One installed plugin's contributions could not be checked.
    PluginUnchecked {
        /// The plugin's id.
        plugin_id: String,
        /// Why.
        error: PluginAddError,
    },
}

impl fmt::Display for PendingWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unchecked(err) => write!(
                f,
                "warning: could not check the project's plugins for pending changes: {err}"
            ),
            Self::PluginUnchecked { plugin_id, error } => write!(
                f,
                "warning: could not check plugin `{plugin_id}` for pending changes: {error}"
            ),
        }
    }
}

/// What [`pending_stop`] found. A native build stops when
/// [`stops`](Self::stops); `Display` is then the message it stops with, the
/// [`PendingPlugin`] blocks one after another (empty when nothing stops).
#[derive(Debug, Default)]
pub struct PendingStop {
    /// Every installed manifest-described plugin with pending edits, in
    /// catalog order.
    pub plugins: Vec<PendingPlugin>,
    /// Every check that could not be made, in the order tried.
    pub warnings: Vec<PendingWarning>,
}

impl PendingStop {
    /// Whether a native build must stop: some plugin has pending edits.
    pub fn stops(&self) -> bool {
        !self.plugins.is_empty()
    }
}

impl fmt::Display for PendingStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, plugin) in self.plugins.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{plugin}")?;
        }
        Ok(())
    }
}

/// Whether `frust run`/`frust build` must stop before any build process: the
/// manifest-described plugins of `catalog` the app at `project_root` depends
/// on whose tracked project edits ([`pending_contributions`], base only) are
/// still pending — `frust run`/`frust build` never edit a tracked file
/// themselves; `frust plugin sync` does. Reads only. A check that fails is a
/// [`PendingWarning`], never a stop; built-in plugins are not checked, and a
/// catalog of built-ins alone reads nothing.
pub fn pending_stop(project_root: &Path, catalog: &PluginCatalog) -> PendingStop {
    let mut stop = PendingStop::default();
    if catalog.iter().all(CatalogEntry::is_built_in) {
        return stop;
    }
    let installed = match installed_plugin_ids(project_root, catalog) {
        Ok(installed) => installed,
        Err(err) => {
            stop.warnings.push(PendingWarning::Unchecked(err));
            return stop;
        }
    };
    for entry in catalog
        .iter()
        .filter(|entry| !entry.is_built_in() && installed.contains(&*entry.spec.id))
    {
        match pending_contributions(project_root, entry, &[]) {
            Ok(pending) if pending.is_empty() => {}
            Ok(pending) => stop.plugins.push(PendingPlugin {
                plugin_id: entry.spec.id.to_string(),
                package: package_of(entry).to_string(),
                pending,
            }),
            Err(error) => stop.warnings.push(PendingWarning::PluginUnchecked {
                plugin_id: entry.spec.id.to_string(),
                error,
            }),
        }
    }
    stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_build::local_properties;
    use crate::packages::{GraphPackage, StubLocator};
    use crate::platform_wiring;
    use crate::plugin::manifest;
    use crate::scaffold::{self, TemplateContext};
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-plugin-pending-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        crate::host_path::canonicalize_simplified(&dir).unwrap()
    }

    /// A freshly scaffolded app at `<root>/app`.
    fn scaffold_app(root: &Path) -> PathBuf {
        let app = root.join("app");
        let ctx = TemplateContext {
            project_name: "acme_app".into(),
            title_case_name: "Acme App".into(),
            org: "com.example".into(),
            description: "A test app.".into(),
            frust_version: "0.1.0".into(),
            frust: scaffold::FrustDependency::Registry {
                version: "0.1.0".into(),
            },
            deeplink_scheme: None,
            deeplink_host: None,
        };
        scaffold::generate(&app, &ctx, None, false, None).unwrap();
        app
    }

    const PACKAGE: &str = "frust-acme-ble";

    /// The plugin's own `Cargo.toml` metadata: a Gradle module, a
    /// permission, a Swift package and a plist key.
    const MANIFEST: &str = r#"
        [package.metadata.frust.plugin]
        id = "acme-ble"
        summary = "Acme BLE scanner"

        [package.metadata.frust.plugin.android]
        gradle-module = { name = "frust-acme-ble", path = "platform/android" }
        permissions = ["android.permission.BLUETOOTH_CONNECT"]

        [package.metadata.frust.plugin.ios]
        swift-package = { name = "FrustAcmeBle", path = "platform/ios" }
        plist = { NSBluetoothAlwaysUsageDescription = "Finds nearby Acme devices." }
    "#;

    /// A plugin package at `<root>/<dir>` described by `metadata`, parsed
    /// by discovery's own parser into a catalog entry.
    fn plugin_entry(
        root: &Path,
        dir: &str,
        package: &str,
        source: Option<&str>,
        metadata: &str,
    ) -> CatalogEntry {
        let package_dir = root.join(dir);
        fs::create_dir_all(package_dir.join("platform/android")).unwrap();
        fs::create_dir_all(package_dir.join("platform/ios")).unwrap();
        let doc: toml::Table = toml::from_str(metadata).unwrap();
        let graph_package = GraphPackage {
            name: package.to_string(),
            version: "0.3.1".to_string(),
            source: source.map(str::to_string),
            manifest_path: package_dir.join("Cargo.toml"),
            metadata: serde_json::to_value(doc).unwrap()["package"]["metadata"].clone(),
            features: None,
        };
        manifest::plugin_entry(&graph_package, &graph_package.metadata["frust"]["plugin"]).unwrap()
    }

    /// `line` (`name = value`) added to the app's `[dependencies]`.
    fn depend(app: &Path, line: &str) {
        let path = app.join("Cargo.toml");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            text.replacen("[dependencies]\n", &format!("[dependencies]\n{line}\n"), 1),
        )
        .unwrap();
    }

    /// Every file under `root` (symlinks recorded by target, never followed).
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    out.insert(path, target.to_string_lossy().as_bytes().to_vec());
                } else if meta.is_dir() {
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, &mut out);
        out
    }

    fn read(app: &Path, rel: &str) -> String {
        fs::read_to_string(app.join(rel)).unwrap()
    }

    #[test]
    fn a_manifest_plugin_applies_from_its_own_package_and_is_then_not_pending() {
        let root = temp_root("apply");
        let app = scaffold_app(&root);
        let entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let line = "frust-acme-ble = { path = \"../acme-ble\", default-features = false }";
        depend(&app, line);
        let cargo_before = read(&app, "Cargo.toml");

        // Before: every contribution but the dependency line is pending.
        let before = pending_contributions(&app, &entry, &[]).unwrap();
        let expected: Vec<&Contribution> = entry.spec.base.iter().skip(1).collect();
        assert_eq!(
            before.iter().map(|p| &p.contribution).collect::<Vec<_>>(),
            expected
        );
        assert!(before.iter().all(|p| p.plugin_id == "acme-ble"));

        let catalog =
            PluginCatalog::with_discovered(PluginCatalog::built_in(), vec![entry.clone()]).0;
        let locator = StubLocator::failing("a manifest plugin is never located");
        let report =
            apply::add_plugin_from_with(&locator, &app, &catalog, "acme-ble", &[]).unwrap();
        assert_eq!(locator.calls(), 0);
        let outcomes: Vec<AddOutcome> = report.items.iter().map(|i| i.outcome).collect();
        // The scaffolded `/ios/Frust*` line already covers the package link.
        assert_eq!(
            outcomes,
            [
                AddOutcome::AlreadyPresent,
                AddOutcome::Applied,
                AddOutcome::Applied,
                AddOutcome::Applied,
                AddOutcome::AlreadyPresent,
                AddOutcome::Applied,
            ]
        );
        assert_eq!(
            report.items[4].description,
            ".gitignore entry `/ios/FrustAcmeBle`"
        );
        assert_eq!(
            read(&app, "Cargo.toml"),
            cargo_before,
            "the dependency line is the app's"
        );
        assert!(read(&app, apply::SETTINGS_GRADLE_REL).contains("include(\":frust-acme-ble\")"));
        assert!(
            read(&app, apply::APP_BUILD_GRADLE_REL)
                .contains("implementation(project(\":frust-acme-ble\"))")
        );
        assert!(
            read(&app, apply::MANIFEST_REL)
                .contains("android:name=\"android.permission.BLUETOOTH_CONNECT\"")
        );
        assert!(
            read(&app, apply::PLIST_REL).contains("<key>NSBluetoothAlwaysUsageDescription</key>")
        );
        assert!(
            read(&app, apply::PBXPROJ_REL)
                .contains("/* XCLocalSwiftPackageReference \"FrustAcmeBle\" */ = {")
        );
        // The machine-local wiring points into the package itself.
        let android_dir = root.join("acme-ble/platform/android");
        assert_eq!(
            local_properties::read_value(
                &app.join(platform_wiring::LOCAL_PROPERTIES),
                "frust.plugin.frust-acme-ble.dir"
            )
            .unwrap(),
            Some(android_dir.display().to_string())
        );
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(app.join("ios/FrustAcmeBle")).unwrap(),
            root.join("acme-ble/platform/ios")
        );

        // A second add is a byte-identical no-op, and nothing is pending.
        let tree = snapshot(&app);
        let again = apply::add_plugin_from_with(&locator, &app, &catalog, "acme-ble", &[]).unwrap();
        assert!(
            again
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{again:?}"
        );
        assert_eq!(snapshot(&app), tree);
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_dependency_line_is_written_from_the_package_source() {
        let root = temp_root("dep-line");
        let path_entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let registry = Some("registry+https://github.com/rust-lang/crates.io-index");
        let registry_entry = plugin_entry(&root, "registry/acme-ble", PACKAGE, registry, MANIFEST);
        for (entry, expected) in [
            (&path_entry, "frust-acme-ble = { path = \"../acme-ble\" }\n"),
            (&registry_entry, "frust-acme-ble = \"0.3.1\"\n"),
        ] {
            let app = scaffold_app(&root);
            let report = apply::add_entry_with(&StubLocator::new(), &app, entry, &[]).unwrap();
            assert_eq!(report.items[0].outcome, AddOutcome::Applied);
            let cargo = read(&app, "Cargo.toml");
            assert!(cargo.contains(expected), "{cargo}");
            assert!(
                !cargo.contains("plugins/acme-ble"),
                "never derived from the frust path: {cargo}"
            );
            let _ = fs::remove_dir_all(&app);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_git_package_without_a_dependency_line_is_refused_untouched() {
        let root = temp_root("git");
        let app = scaffold_app(&root);
        let git = Some("git+https://example.com/acme/frust-acme-ble#0123abcd");
        let entry = plugin_entry(&root, "acme-ble", PACKAGE, git, MANIFEST);
        let tree = snapshot(&app);
        let err = apply::add_entry_with(&StubLocator::new(), &app, &entry, &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::DependencySourceNotWritable { package, .. } if package == PACKAGE),
            "{err:?}"
        );
        assert!(err.to_string().contains("yourself"), "{err}");
        assert_eq!(snapshot(&app), tree);

        // The author's own git line is the app's to keep.
        depend(
            &app,
            "frust-acme-ble = { git = \"https://example.com/acme/frust-acme-ble\" }",
        );
        let report = apply::add_entry_with(&StubLocator::new(), &app, &entry, &[]).unwrap();
        assert_eq!(report.items[0].outcome, AddOutcome::AlreadyPresent);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_manifest_plugin_never_runs_app_crate_or_scaffold_contributions() {
        let root = temp_root("inexpressible");
        let app = scaffold_app(&root);
        let mut entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let mut base = entry.spec.base.to_vec();
        base.push(Contribution::ScaffoldFile {
            rel_path: Cow::Borrowed("acme.txt"),
            contents: Cow::Borrowed(""),
            comment: Cow::Borrowed("test"),
        });
        entry.spec.base = Cow::Owned(base);
        let tree = snapshot(&app);
        let err = apply::add_entry_with(&StubLocator::new(), &app, &entry, &[]).unwrap_err();
        assert!(
            matches!(err, PluginAddError::InvalidContribution(_)),
            "{err:?}"
        );
        assert_eq!(snapshot(&app), tree);

        // Nor can the schema express either: discovery never produces one.
        let parsed = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        assert!(!parsed.spec.base.iter().any(|c| matches!(
            c,
            Contribution::AppCrateMacro { .. } | Contribution::ScaffoldFile { .. }
        )));
        let _ = fs::remove_dir_all(&root);
    }

    const ENTITLEMENT_MANIFEST: &str = r#"
        [package.metadata.frust.plugin]
        id = "acme-push"
        summary = "Acme push"
        [package.metadata.frust.plugin.ios]
        entitlements = { "aps-environment" = "development" }
    "#;

    /// A string entitlement is pending until its key is in the file, and
    /// again as soon as any one Runner configuration lacks
    /// `CODE_SIGN_ENTITLEMENTS`.
    #[test]
    fn an_entitlement_is_pending_while_any_runner_configuration_lacks_the_setting() {
        let root = temp_root("entitlement");
        let app = scaffold_app(&root);
        let entry = plugin_entry(
            &root,
            "acme-push",
            "frust-acme-push",
            None,
            ENTITLEMENT_MANIFEST,
        );
        depend(&app, "frust-acme-push = { path = \"../acme-push\" }");
        let entitlement = &entry.spec.base[1];
        assert!(matches!(entitlement, Contribution::IosEntitlement { .. }));

        let before = pending_contributions(&app, &entry, &[]).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(&before[0].contribution, entitlement);

        apply::add_entry_with(&StubLocator::new(), &app, &entry, &[]).unwrap();
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);
        let setting = "CODE_SIGN_ENTITLEMENTS = Runner/Runner.entitlements;";
        let pbxproj = read(&app, apply::PBXPROJ_REL);
        assert!(pbxproj.matches(setting).count() >= 3, "{pbxproj}");

        fs::write(
            app.join(apply::PBXPROJ_REL),
            pbxproj.replacen(setting, "", 1),
        )
        .unwrap();
        let after = pending_contributions(&app, &entry, &[]).unwrap();
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0].describe(), entitlement.describe());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pending_refuses_an_unknown_feature_and_never_lists_the_desktop_lane() {
        let root = temp_root("features");
        let app = scaffold_app(&root);
        let mut entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        assert!(matches!(
            pending_contributions(&app, &entry, &["nope"]),
            Err(PluginAddError::UnknownFeature { .. })
        ));
        entry.spec.base = Cow::Owned(vec![
            Contribution::CargoDep {
                name: Cow::Borrowed(PACKAGE),
            },
            Contribution::MacosEntitlement {
                key: Cow::Borrowed("com.apple.security.network.client"),
                comment: Cow::Borrowed("test"),
            },
        ]);
        let pending = pending_contributions(&app, &entry, &[]).unwrap();
        assert_eq!(pending.len(), 1, "only the dependency line: {pending:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A catalog of the built-in registry plus `discovered`.
    fn catalog(discovered: Vec<CatalogEntry>) -> PluginCatalog {
        PluginCatalog::with_discovered(PluginCatalog::built_in(), discovered).0
    }

    #[test]
    fn sync_applies_every_installed_manifest_plugin_once() {
        let root = temp_root("sync");
        let app = scaffold_app(&root);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let push = plugin_entry(
            &root,
            "acme-push",
            "frust-acme-push",
            None,
            ENTITLEMENT_MANIFEST,
        );
        // Only `acme-ble` is a dependency of the app, in a target table.
        let path = app.join("Cargo.toml");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            format!(
                "{text}\n[target.'cfg(target_os = \"android\")'.dependencies]\n\
                 frust-acme-ble = {{ path = \"../acme-ble\" }}\n"
            ),
        )
        .unwrap();
        let catalog = catalog(vec![ble.clone(), push]);

        let report = sync_plugins(&app, &catalog).unwrap();
        assert_eq!(report.plugins.len(), 1, "{report:?}");
        assert_eq!(report.plugins[0].plugin_id, "acme-ble");
        assert!(report.changed());
        assert_eq!(pending_contributions(&app, &ble, &[]).unwrap(), []);

        let tree = snapshot(&app);
        let again = sync_plugins(&app, &catalog).unwrap();
        assert!(!again.changed(), "{again:?}");
        assert_eq!(snapshot(&app), tree);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_checks_every_plugin_before_the_first_write() {
        let root = temp_root("sync-refused");
        let app = scaffold_app(&root);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let high_floor = MANIFEST.replace("acme-ble\"\n", "acme-tag\"\n").replace(
            "summary = \"Acme BLE scanner\"",
            "summary = \"Acme tags\"\nmin-ios = \"99\"",
        );
        let tag = plugin_entry(&root, "acme-tag", "frust-acme-tag", None, &high_floor);
        assert_eq!(tag.spec.min_ios.as_deref(), Some("99"));
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        depend(&app, "frust-acme-tag = { path = \"../acme-tag\" }");
        let tree = snapshot(&app);

        let err = sync_plugins(&app, &catalog(vec![ble, tag])).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::PlatformFloorTooLow { plugin, .. } if &**plugin == "acme-tag"),
            "{err:?}"
        );
        assert_eq!(snapshot(&app), tree, "nothing was written");

        // A pending contribution whose anchor is gone is refused up front too.
        let manifest = read(&app, apply::MANIFEST_REL).replace("</manifest>", "");
        fs::write(app.join(apply::MANIFEST_REL), manifest).unwrap();
        let tree = snapshot(&app);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let err = sync_plugins(&app, &catalog(vec![ble])).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(rel) if rel == apply::MANIFEST_REL),
            "{err:?}"
        );
        assert_eq!(snapshot(&app), tree);
        let _ = fs::remove_dir_all(&root);
    }

    /// An app without `ios/` (or without `android/`) syncs a manifest plugin
    /// that also carries items for that platform: they are never pending,
    /// never applied, and the platform directory is not created.
    #[test]
    fn a_manifest_plugin_skips_a_platform_directory_the_app_lacks() {
        for (absent, kept) in [("ios", "android"), ("android", "ios")] {
            let root = temp_root(&format!("no-{absent}"));
            let app = scaffold_app(&root);
            fs::remove_dir_all(app.join(absent)).unwrap();
            let entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
            depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");

            let pending = pending_contributions(&app, &entry, &[]).unwrap();
            assert_eq!(pending.len(), 2, "{absent}: {pending:?}");
            assert!(
                pending
                    .iter()
                    .all(|p| platform_dir(&p.contribution) == Some(kept)),
                "{absent}: {pending:?}"
            );

            let catalog = catalog(vec![entry.clone()]);
            let report = sync_plugins(&app, &catalog).unwrap();
            assert_eq!(report.plugins.len(), 1, "{report:?}");
            // Kept `ios/` adds the package link's `.gitignore` row.
            assert_eq!(
                report.plugins[0].items.len(),
                if kept == "ios" { 4 } else { 3 },
                "the dependency line and the {kept} items only: {report:?}"
            );
            assert!(report.changed());
            assert!(!app.join(absent).exists(), "{absent}/ is never created");
            assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);

            let tree = snapshot(&app);
            let again = sync_plugins(&app, &catalog).unwrap();
            assert!(!again.changed(), "{again:?}");
            assert_eq!(snapshot(&app), tree);
            let _ = fs::remove_dir_all(&root);
        }
    }

    const LINK_PACKAGE: &str = "AcmeLink";

    /// The plugin of [`MANIFEST`] with a Swift package that is not named
    /// `Frust...`, so the template's `/ios/Frust*` line does not cover its link.
    fn link_entry(root: &Path) -> CatalogEntry {
        plugin_entry(
            root,
            "acme-ble",
            PACKAGE,
            None,
            &MANIFEST.replace("FrustAcmeBle", LINK_PACKAGE),
        )
    }

    fn gitignore_rows(app: &Path, entry: &CatalogEntry) -> Vec<String> {
        pending_contributions(app, entry, &[])
            .unwrap()
            .iter()
            .map(PendingContribution::describe)
            .filter(|d| d.starts_with(".gitignore"))
            .collect()
    }

    #[test]
    fn a_manifest_plugins_ios_link_is_pending_until_the_gitignore_lists_it() {
        let root = temp_root("gitignore-link");
        let app = scaffold_app(&root);
        let entry = link_entry(&root);
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        let before = read(&app, ".gitignore");

        assert_eq!(
            gitignore_rows(&app, &entry),
            [".gitignore entry `/ios/AcmeLink`"]
        );
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap().len(), 5);

        let catalog = catalog(vec![entry.clone()]);
        assert!(sync_plugins(&app, &catalog).unwrap().changed());
        assert_eq!(read(&app, ".gitignore"), format!("{before}/ios/AcmeLink\n"));
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);

        let tree = snapshot(&app);
        assert!(!sync_plugins(&app, &catalog).unwrap().changed());
        assert_eq!(snapshot(&app), tree);
        let _ = fs::remove_dir_all(&root);
    }

    /// The Swift package and its `.gitignore` line are two report items, so
    /// what a sync reports applied is exactly what was pending, row for row.
    #[test]
    fn a_swift_package_and_its_gitignore_line_are_two_applied_items() {
        let root = temp_root("gitignore-items");
        let app = scaffold_app(&root);
        let entry = link_entry(&root);
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        let pending: Vec<String> = pending_contributions(&app, &entry, &[])
            .unwrap()
            .iter()
            .map(PendingContribution::describe)
            .collect();

        let report = sync_plugins(&app, &catalog(vec![entry])).unwrap();
        let applied: Vec<&str> = report.plugins[0]
            .items
            .iter()
            .filter(|item| item.outcome == AddOutcome::Applied)
            .map(|item| item.description.as_str())
            .collect();
        assert_eq!(applied, pending, "{report:?}");
        let swift = applied
            .iter()
            .position(|d| *d == "Xcode Swift package `AcmeLink`")
            .unwrap();
        assert_eq!(applied[swift + 1], ".gitignore entry `/ios/AcmeLink`");
        let _ = fs::remove_dir_all(&root);
    }

    /// Contributions skipped while the app had no `android/` are pending as
    /// soon as the directory exists: the pending report reads the project as
    /// it is on every call.
    #[test]
    fn android_items_skipped_without_android_are_pending_once_it_is_added() {
        let root = temp_root("android-later");
        let app = scaffold_app(&root);
        fs::remove_dir_all(app.join("android")).unwrap();
        let entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let catalog = catalog(vec![entry.clone()]);
        let locator = StubLocator::failing("a manifest plugin is never located");
        apply::add_plugin_from_with(&locator, &app, &catalog, "acme-ble", &[]).unwrap();
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);

        // `android/` from the app template, as a fresh scaffold renders it.
        let donor = scaffold_app(&temp_root("android-later-template"));
        fs::rename(donor.join("android"), app.join("android")).unwrap();
        let pending = pending_contributions(&app, &entry, &[]).unwrap();
        let rows: Vec<String> = pending.iter().map(PendingContribution::describe).collect();
        assert_eq!(
            rows,
            [
                "Gradle module `:frust-acme-ble`",
                "AndroidManifest.xml permission `android.permission.BLUETOOTH_CONNECT`",
            ]
        );
        let stop = pending_stop(&app, &catalog);
        assert!(stop.warnings.is_empty(), "{:?}", stop.warnings);
        assert_eq!(stop.plugins.len(), 1, "{stop}");
        assert_eq!(stop.plugins[0].pending, pending);
        let _ = fs::remove_dir_all(donor.parent().unwrap());
        let _ = fs::remove_dir_all(&root);

        // A built-in plugin's permission, skipped the same way, is pending
        // (`frust plugin list`'s count) once `android/` exists.
        let root = temp_root("android-later-built-in");
        let app = scaffold_app(&root);
        fs::remove_dir_all(app.join("android")).unwrap();
        apply::add_plugin_with(&StubLocator::new(), &app, "haptics", &[]).unwrap();
        let built_in = PluginCatalog::built_in();
        let haptics = built_in.find("haptics").unwrap();
        assert_eq!(pending_contributions(&app, haptics, &[]).unwrap(), []);
        let donor = scaffold_app(&temp_root("android-later-built-in-template"));
        fs::rename(donor.join("android"), app.join("android")).unwrap();
        let rows: Vec<String> = pending_contributions(&app, haptics, &[])
            .unwrap()
            .iter()
            .map(PendingContribution::describe)
            .collect();
        assert_eq!(
            rows,
            ["AndroidManifest.xml permission `android.permission.VIBRATE`"]
        );
        let _ = fs::remove_dir_all(donor.parent().unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_gitignore_without_a_final_newline_gains_the_line_on_its_own_row() {
        let root = temp_root("gitignore-no-newline");
        let app = scaffold_app(&root);
        let entry = link_entry(&root);
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        fs::write(app.join(".gitignore"), "/target").unwrap();

        sync_plugins(&app, &catalog(vec![entry])).unwrap();
        assert_eq!(read(&app, ".gitignore"), "/target\n/ios/AcmeLink\n");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_covered_ios_link_leaves_the_gitignore_alone() {
        for covered in [
            "/ios/AcmeLink",
            "ios/AcmeLink",
            "/ios/AcmeLink  ",
            "/ios/AcmeLink\r",
        ] {
            let root = temp_root("gitignore-covered");
            let app = scaffold_app(&root);
            let entry = link_entry(&root);
            depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
            let text = format!("/target\n{covered}\n");
            fs::write(app.join(".gitignore"), &text).unwrap();

            assert_eq!(gitignore_rows(&app, &entry), Vec::<String>::new());
            sync_plugins(&app, &catalog(vec![entry])).unwrap();
            assert_eq!(read(&app, ".gitignore"), text, "{covered}");
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// The link is a symlink: git applies a trailing-slash line to
    /// directories only and keeps leading whitespace in the pattern, so
    /// neither form ignores it and the line is still appended.
    #[test]
    fn a_line_git_would_not_apply_to_the_link_does_not_cover_it() {
        for uncovered in [
            "/ios/AcmeLink/",
            "ios/AcmeLink/",
            "  /ios/AcmeLink",
            "/ios/AcmeLink\t",
        ] {
            let root = temp_root("gitignore-uncovered");
            let app = scaffold_app(&root);
            let entry = link_entry(&root);
            depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
            let text = format!("/target\n{uncovered}\n");
            fs::write(app.join(".gitignore"), &text).unwrap();

            assert_eq!(gitignore_rows(&app, &entry).len(), 1, "{uncovered:?}");
            sync_plugins(&app, &catalog(vec![entry])).unwrap();
            assert_eq!(
                read(&app, ".gitignore"),
                format!("{text}/ios/AcmeLink\n"),
                "{uncovered:?}"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// The scaffolded `/ios/Frust*` line covers a `Frust...` package only.
    #[test]
    fn the_frust_wildcard_covers_a_frust_named_package_only() {
        assert!(apply::gitignore_covers_ios_link(
            "/ios/Frust*\n",
            "FrustAcmeBle"
        ));
        assert!(!apply::gitignore_covers_ios_link(
            "/ios/Frust*\n",
            "AcmeLink"
        ));
        let root = temp_root("gitignore-frust");
        let app = scaffold_app(&root);
        let entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        assert_eq!(gitignore_rows(&app, &entry), Vec::<String>::new());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_app_without_a_gitignore_has_nothing_pending_and_gets_none() {
        let root = temp_root("gitignore-absent");
        let app = scaffold_app(&root);
        let entry = link_entry(&root);
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        fs::remove_file(app.join(".gitignore")).unwrap();

        assert_eq!(gitignore_rows(&app, &entry), Vec::<String>::new());
        sync_plugins(&app, &catalog(vec![entry.clone()])).unwrap();
        assert!(!app.join(".gitignore").exists());
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_app_without_ios_has_no_gitignore_item_pending() {
        let root = temp_root("gitignore-no-ios");
        let app = scaffold_app(&root);
        fs::remove_dir_all(app.join("ios")).unwrap();
        let entry = link_entry(&root);
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        let before = read(&app, ".gitignore");

        assert_eq!(gitignore_rows(&app, &entry), Vec::<String>::new());
        sync_plugins(&app, &catalog(vec![entry])).unwrap();
        assert_eq!(read(&app, ".gitignore"), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_built_in_plugin_never_reads_the_gitignore() {
        let root = temp_root("gitignore-built-in");
        let app = scaffold_app(&root);
        let entry = CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-link"),
            crate_dir: Cow::Borrowed("acme-link"),
            base: Cow::Owned(vec![Contribution::SwiftPackageRef {
                package_name: Cow::Borrowed(LINK_PACKAGE),
                rel_path: Cow::Borrowed("plugins/acme-link/platform/ios"),
            }]),
            ..crate::plugin::PluginSpec::DEFAULT
        });
        let before = read(&app, ".gitignore");
        let pending = pending_contributions(&app, &entry, &[]).unwrap();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].describe(), "Xcode Swift package `AcmeLink`");
        assert_eq!(read(&app, ".gitignore"), before);
        let _ = fs::remove_dir_all(&root);
    }

    /// The skip is for manifest-described plugins only: a built-in plugin's
    /// iOS item on an app without `ios/` still fails as its add would.
    #[test]
    fn a_built_in_plugin_still_needs_the_platform_files_it_edits() {
        let root = temp_root("built-in-no-ios");
        let app = scaffold_app(&root);
        fs::remove_dir_all(app.join("ios")).unwrap();
        let entry = CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-tag"),
            crate_dir: Cow::Borrowed("acme-tag"),
            base: Cow::Owned(vec![
                Contribution::CargoDep {
                    name: Cow::Borrowed("frust-acme-tag"),
                },
                Contribution::PlistEntry {
                    key: Cow::Borrowed("NSBluetoothAlwaysUsageDescription"),
                    value: Cow::Borrowed("Finds nearby Acme tags."),
                    comment: Cow::Borrowed("test"),
                },
            ]),
            ..crate::plugin::PluginSpec::DEFAULT
        });
        let err = pending_contributions(&app, &entry, &[]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MissingProjectFile(rel) if rel == apply::PLIST_REL),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A built-in plugin's link flags are satisfied on an app without `ios/`
    /// (nothing to link, the pending check agreeing with the applier), and
    /// pending on one with `ios/` until applied.
    #[test]
    fn a_built_in_plugins_link_flags_are_never_pending_without_ios() {
        let root = temp_root("built-in-links-no-ios");
        let app = scaffold_app(&root);
        let entry = CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-link"),
            crate_dir: Cow::Borrowed("acme-link"),
            base: Cow::Owned(vec![
                Contribution::IosLibrary {
                    name: Cow::Borrowed("c++"),
                },
                Contribution::IosFramework {
                    name: Cow::Borrowed("CoreML"),
                },
            ]),
            ..crate::plugin::PluginSpec::DEFAULT
        });
        let pending: Vec<String> = pending_contributions(&app, &entry, &[])
            .unwrap()
            .iter()
            .map(PendingContribution::describe)
            .collect();
        assert_eq!(
            pending,
            [
                "Xcode library `-lc++` (`OTHER_LDFLAGS`)",
                "Xcode framework `CoreML` (`OTHER_LDFLAGS`)",
            ]
        );
        fs::remove_dir_all(app.join("ios")).unwrap();
        assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);
        let _ = fs::remove_dir_all(&root);
    }

    /// Adding a manifest plugin to an app without `ios/` (or `android/`)
    /// skips that platform's items as sync does: nothing of it is applied,
    /// written or an error, and nothing stays pending.
    #[test]
    fn adding_a_manifest_plugin_skips_a_platform_directory_the_app_lacks() {
        for (absent, kept) in [("ios", "android"), ("android", "ios")] {
            let root = temp_root(&format!("add-no-{absent}"));
            let app = scaffold_app(&root);
            fs::remove_dir_all(app.join(absent)).unwrap();
            let entry = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
            depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
            let catalog = catalog(vec![entry.clone()]);

            let report = apply::add_plugin_from_with(
                &StubLocator::failing("a manifest plugin is never located"),
                &app,
                &catalog,
                "acme-ble",
                &[],
            )
            .unwrap();
            // Kept `ios/` adds the package link's `.gitignore` row, which the
            // scaffolded `/ios/Frust*` line already covers.
            let expected = if kept == "ios" { 4 } else { 3 };
            assert_eq!(report.items.len(), expected, "{absent}: {report:?}");
            assert!(
                report
                    .items
                    .iter()
                    .skip(1)
                    .filter(|i| !i.description.starts_with(".gitignore"))
                    .all(|i| i.outcome == AddOutcome::Applied),
                "{report:?}"
            );
            assert!(!app.join(absent).exists(), "{absent}/ is never created");
            if kept == "android" {
                assert!(read(&app, apply::SETTINGS_GRADLE_REL).contains("frust-acme-ble"));
            } else {
                assert!(read(&app, apply::PLIST_REL).contains("NSBluetoothAlwaysUsageDescription"));
            }
            assert_eq!(pending_contributions(&app, &entry, &[]).unwrap(), []);
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// A built-in plugin's add on an app without `ios/` is unchanged: it
    /// fails exactly as applying the entry unfiltered does.
    #[test]
    fn adding_a_built_in_plugin_on_an_app_without_ios_is_unchanged() {
        let root = temp_root("add-built-in-no-ios");
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        let a = scaffold_app(&root);
        fs::remove_dir_all(a.join("ios")).unwrap();
        let catalog = PluginCatalog::built_in();
        let through_add = apply::add_plugin_from_with(
            &StubLocator::new().with("frust-camera", &pkg),
            &a,
            &catalog,
            "camera",
            &[],
        );
        let b = root.join("b");
        fs::create_dir_all(&b).unwrap();
        let ctx_app = scaffold_app(&b);
        fs::remove_dir_all(ctx_app.join("ios")).unwrap();
        let direct = apply::add_entry_with(
            &StubLocator::new().with("frust-camera", &pkg),
            &ctx_app,
            catalog.find("camera").unwrap(),
            &[],
        );
        let (
            Err(PluginAddError::MissingProjectFile(a)),
            Err(PluginAddError::MissingProjectFile(b)),
        ) = (&through_add, &direct)
        else {
            panic!("{through_add:?} vs {direct:?}");
        };
        assert_eq!(a, b);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn is_installed_follows_one_rule_for_built_in_and_manifest_entries() {
        let root = temp_root("is-installed");
        let app = scaffold_app(&root);
        let built_in = CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-tag"),
            crate_dir: Cow::Borrowed("acme-tag"),
            base: Cow::Owned(vec![Contribution::CargoDep {
                name: Cow::Borrowed("frust-acme-tag"),
            }]),
            ..crate::plugin::PluginSpec::DEFAULT
        });
        let manifest = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        assert!(!apply::is_installed(&app, &built_in).unwrap());
        assert!(!apply::is_installed(&app, &manifest).unwrap());

        depend(&app, "frust-acme-tag = \"1\"");
        assert!(apply::is_installed(&app, &built_in).unwrap());

        depend(&app, "frust-acme-ble = \"0.3\"");
        assert!(apply::is_installed(&app, &manifest).unwrap());
        let _ = fs::remove_dir_all(&root);

        for (line, expected) in [
            (
                "frust-acme-ble = { version = \"0.3\", optional = true }",
                false,
            ),
            (
                "ble = { package = \"frust-acme-ble\", version = \"0.3\" }",
                true,
            ),
            (
                "ble = { package = \"frust-acme-ble\", version = \"0.3\", optional = true }",
                false,
            ),
        ] {
            let root = temp_root("is-installed-forms");
            let app = scaffold_app(&root);
            let manifest = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
            depend(&app, line);
            assert_eq!(
                apply::is_installed(&app, &manifest).unwrap(),
                expected,
                "{line}"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn a_collision_names_both_plugins() {
        let collision = ModuleCollision {
            kind: NativeKind::GradleModule {
                gradle_name: Cow::Borrowed(":frust-acme"),
            },
            kept_plugin_id: "acme-a".into(),
            dropped_plugin_id: "acme-b".into(),
        };
        assert_eq!(
            collision.to_string(),
            "Android module `:frust-acme`: declared by plugins `acme-a` and `acme-b`; \
             `acme-a`'s is wired, `acme-b`'s is not"
        );
    }

    /// The text of every warning of `stop`.
    fn warning_lines(stop: &PendingStop) -> Vec<String> {
        stop.warnings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn nothing_pending_does_not_stop() {
        let root = temp_root("stop-none");
        let app = scaffold_app(&root);
        // Built-ins alone: nothing is read, not even the manifest.
        let stop = pending_stop(&root.join("absent"), &PluginCatalog::built_in());
        assert!(!stop.stops() && stop.warnings.is_empty(), "{stop:?}");

        // A manifest plugin the app does not depend on is never checked.
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let stop = pending_stop(&app, &catalog(vec![ble.clone()]));
        assert!(!stop.stops() && stop.warnings.is_empty(), "{stop:?}");
        assert_eq!(stop.to_string(), "");

        // One the app depends on and carries in full does not stop either.
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        sync_plugins(&app, &catalog(vec![ble.clone()])).unwrap();
        let stop = pending_stop(&app, &catalog(vec![ble]));
        assert!(!stop.stops() && stop.warnings.is_empty(), "{stop:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn one_pending_plugin_stops_with_its_block() {
        let root = temp_root("stop-one");
        let app = scaffold_app(&root);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        let stop = pending_stop(&app, &catalog(vec![ble]));
        assert!(stop.stops());
        assert!(stop.warnings.is_empty(), "{stop:?}");
        assert_eq!(stop.plugins.len(), 1);
        assert_eq!(stop.plugins[0].plugin_id, "acme-ble");
        assert_eq!(stop.plugins[0].package, PACKAGE);
        assert_eq!(
            stop.to_string(),
            "plugin acme-ble (frust-acme-ble) is not wired into this project: 4 change(s) \
             pending. Run `frust plugin sync` and re-run.\n\
             \x20 - Gradle module `:frust-acme-ble`\n\
             \x20 - AndroidManifest.xml permission `android.permission.BLUETOOTH_CONNECT`\n\
             \x20 - Xcode Swift package `FrustAcmeBle`\n\
             \x20 - Info.plist key `NSBluetoothAlwaysUsageDescription`"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_pending_plugins_are_two_blocks_in_catalog_order() {
        let root = temp_root("stop-two");
        let app = scaffold_app(&root);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let push = plugin_entry(
            &root,
            "acme-push",
            "frust-acme-push",
            None,
            ENTITLEMENT_MANIFEST,
        );
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        depend(&app, "frust-acme-push = { path = \"../acme-push\" }");
        let stop = pending_stop(&app, &catalog(vec![ble, push]));
        assert!(stop.warnings.is_empty(), "{stop:?}");
        let ids: Vec<&str> = stop.plugins.iter().map(|p| &*p.plugin_id).collect();
        assert_eq!(ids, ["acme-ble", "acme-push"]);
        let message = stop.to_string();
        let heads: Vec<&str> = message
            .lines()
            .filter(|line| !line.starts_with("  - "))
            .collect();
        assert_eq!(
            heads,
            [
                "plugin acme-ble (frust-acme-ble) is not wired into this project: 4 change(s) \
                 pending. Run `frust plugin sync` and re-run.",
                "plugin acme-push (frust-acme-push) is not wired into this project: 1 change(s) \
                 pending. Run `frust plugin sync` and re-run.",
            ]
        );
        assert_eq!(message.lines().count(), 7, "{message}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A check that fails is a warning, never a stop, and the other plugins
    /// are still checked.
    #[test]
    fn a_failing_pending_check_is_a_warning() {
        let root = temp_root("stop-fails");
        let app = scaffold_app(&root);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let push = plugin_entry(
            &root,
            "acme-push",
            "frust-acme-push",
            None,
            ENTITLEMENT_MANIFEST,
        );
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        depend(&app, "frust-acme-push = { path = \"../acme-push\" }");
        fs::remove_file(app.join(apply::MANIFEST_REL)).unwrap();
        let stop = pending_stop(&app, &catalog(vec![ble.clone(), push]));
        let ids: Vec<&str> = stop.plugins.iter().map(|p| &*p.plugin_id).collect();
        assert_eq!(ids, ["acme-push"]);
        let warnings = warning_lines(&stop);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0]
                .starts_with("warning: could not check plugin `acme-ble` for pending changes: "),
            "{warnings:?}"
        );

        // An unreadable manifest leaves nothing checkable: one warning.
        fs::remove_file(app.join("Cargo.toml")).unwrap();
        let stop = pending_stop(&app, &catalog(vec![ble]));
        assert!(!stop.stops());
        let warnings = warning_lines(&stop);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with(
                "warning: could not check the project's plugins for pending changes: "
            ),
            "{warnings:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn statuses_list_built_ins_then_discovered_by_id() {
        let root = temp_root("statuses");
        let app = scaffold_app(&root);
        let ble = plugin_entry(&root, "acme-ble", PACKAGE, None, MANIFEST);
        let push = plugin_entry(
            &root,
            "acme-push",
            "frust-acme-push",
            None,
            ENTITLEMENT_MANIFEST,
        );
        depend(&app, "frust-acme-ble = { path = \"../acme-ble\" }");
        let catalog = catalog(vec![push, ble]);
        let statuses = plugin_statuses(&app, &catalog).unwrap();
        assert_eq!(statuses.len(), catalog.iter().count());
        let tail: Vec<(&str, String)> = statuses[statuses.len() - 2..]
            .iter()
            .map(|(entry, status)| (&*entry.spec.id, status.to_string()))
            .collect();
        assert_eq!(
            tail,
            [
                ("acme-ble", "4 pending".to_string()),
                ("acme-push", String::new())
            ]
        );
        assert!(
            statuses[..statuses.len() - 2]
                .iter()
                .all(|(e, _)| e.is_built_in())
        );
        let installed = installed_plugin_ids(&app, &catalog).unwrap();
        assert!(installed.contains("acme-ble"), "{installed:?}");
        assert!(!installed.contains("acme-push"), "{installed:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// A built-in entry carrying only a Material 3 theme parent.
    fn theme_entry() -> CatalogEntry {
        CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-theme"),
            crate_dir: Cow::Borrowed("acme-theme"),
            base: Cow::Owned(vec![Contribution::AndroidThemeParent {
                parent: "Theme.Material3.DayNight.NoActionBar",
            }]),
            ..crate::plugin::PluginSpec::DEFAULT
        })
    }

    /// The app's manifest with `FrustAppTheme` replaced by `theme` and no
    /// `themes.xml`: an app generated before the template defined the style.
    fn drop_app_theme(app: &Path, theme: &str) {
        fs::remove_file(app.join(apply::THEMES_REL)).unwrap();
        let manifest = read(app, apply::MANIFEST_REL);
        fs::write(
            app.join(apply::MANIFEST_REL),
            manifest.replace("@style/FrustAppTheme", theme),
        )
        .unwrap();
    }

    /// `pending_contributions` and the applier agree on all four shapes: a
    /// fresh scaffold (pending, applied), the same app again (neither), a
    /// Material 3 manifest theme without the style (neither), and an app with
    /// neither (pending, refused by the add and by the pre-write check).
    #[test]
    fn a_theme_parent_is_pending_exactly_when_its_add_would_write_or_refuse() {
        let root = temp_root("theme-parent");
        let entry = theme_entry();
        let add = |app: &Path| apply::add_entry_with(&StubLocator::new(), app, &entry, &[]);

        let fresh = scaffold_app(&root.join("fresh"));
        let pending = pending_contributions(&fresh, &entry, &[]).unwrap();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(
            pending[0].describe(),
            "Android theme `FrustAppTheme` parent `Theme.Material3.DayNight.NoActionBar`"
        );
        check_anchors(&fresh, &pending).unwrap();
        let report = add(&fresh).unwrap();
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);

        let before = snapshot(&fresh);
        assert!(
            pending_contributions(&fresh, &entry, &[])
                .unwrap()
                .is_empty()
        );
        let report = add(&fresh).unwrap();
        assert_eq!(report.items[0].outcome, AddOutcome::AlreadyPresent);
        assert_eq!(snapshot(&fresh), before);

        let m3 = scaffold_app(&root.join("m3"));
        drop_app_theme(&m3, "@style/Theme.Material3.DayNight.NoActionBar");
        assert!(pending_contributions(&m3, &entry, &[]).unwrap().is_empty());
        let before = snapshot(&m3);
        let report = add(&m3).unwrap();
        assert_eq!(report.items[0].outcome, AddOutcome::AlreadyPresent);
        assert_eq!(snapshot(&m3), before);

        let stock = scaffold_app(&root.join("stock"));
        drop_app_theme(&stock, "@android:style/Theme.Material.NoActionBar");
        let pending = pending_contributions(&stock, &entry, &[]).unwrap();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(matches!(
            check_anchors(&stock, &pending),
            Err(PluginAddError::AndroidAppThemeMissing { .. })
        ));
        let before = snapshot(&stock);
        assert!(matches!(
            add(&stock),
            Err(PluginAddError::AndroidAppThemeMissing { .. })
        ));
        assert_eq!(snapshot(&stock), before);

        let _ = fs::remove_dir_all(&root);
    }

    /// `pending_contributions` and the applier agree on the shared C++
    /// runtime: pending on a fresh scaffold (its template ships the property
    /// off), not once added, and pending but refused by the pre-write check on
    /// an app whose build script never reads the property.
    #[test]
    fn the_shared_cxx_runtime_is_pending_exactly_when_its_add_would_write_or_refuse() {
        let root = temp_root("libcxx");
        let entry = CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-native"),
            crate_dir: Cow::Borrowed("acme-native"),
            base: Cow::Owned(vec![Contribution::AndroidLibcxxShared]),
            ..crate::plugin::PluginSpec::DEFAULT
        });
        let add = |app: &Path| apply::add_entry_with(&StubLocator::new(), app, &entry, &[]);

        let fresh = scaffold_app(&root.join("fresh"));
        let pending = pending_contributions(&fresh, &entry, &[]).unwrap();
        assert_eq!(pending.len(), 1, "{pending:?}");
        check_anchors(&fresh, &pending).unwrap();
        assert_eq!(add(&fresh).unwrap().items[0].outcome, AddOutcome::Applied);
        assert!(
            pending_contributions(&fresh, &entry, &[])
                .unwrap()
                .is_empty()
        );
        let before = snapshot(&fresh);
        assert_eq!(
            add(&fresh).unwrap().items[0].outcome,
            AddOutcome::AlreadyPresent
        );
        assert_eq!(snapshot(&fresh), before);

        // A property set by hand does nothing while the build script ignores it.
        let old = scaffold_app(&root.join("old"));
        let build = read(&old, apply::APP_BUILD_GRADLE_REL)
            .lines()
            .filter(|line| !line.contains("inkLibcxxShared"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(old.join(apply::APP_BUILD_GRADLE_REL), build).unwrap();
        fs::write(
            old.join(apply::GRADLE_PROPERTIES_REL),
            "frust.cargoNdk.linkLibcxxShared=true\n",
        )
        .unwrap();
        let pending = pending_contributions(&old, &entry, &[]).unwrap();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(matches!(
            check_anchors(&old, &pending),
            Err(PluginAddError::MalformedProjectFile(msg)) if msg.contains("--link-libcxx-shared")
        ));
        let before = snapshot(&old);
        assert!(matches!(
            add(&old),
            Err(PluginAddError::MalformedProjectFile(_))
        ));
        assert_eq!(snapshot(&old), before);

        let _ = fs::remove_dir_all(&root);
    }

    /// Every theme shape the rule distinguishes, judged by
    /// `pending_contributions`, the pre-write check and the add itself: they
    /// agree on each, and only a rewritable parent changes `themes.xml`.
    #[test]
    fn pending_agrees_with_the_add_on_every_theme_shape() {
        const PLATFORM: &str = "@android:style/Theme.Material.NoActionBar";
        let root = temp_root("theme-shapes");
        let entry = theme_entry();
        let add = |app: &Path| apply::add_entry_with(&StubLocator::new(), app, &entry, &[]);
        let styles = |body: &str| {
            format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n{body}</resources>\n")
        };
        let parent_style = |parent: &str| {
            styles(&format!(
                "    <style name=\"FrustAppTheme\" parent=\"{parent}\" />\n"
            ))
        };

        enum Expect {
            Satisfied,
            Rewritten,
            Refused,
        }
        // (case, themes.xml text or none, manifest theme, expectation)
        let cases: Vec<(&str, Option<String>, &str, Expect)> = vec![
            (
                "m3-light",
                Some(parent_style("Theme.Material3.Light.NoActionBar")),
                "@style/FrustAppTheme",
                Expect::Satisfied,
            ),
            (
                "m3-expressive",
                Some(parent_style(
                    "Theme.Material3Expressive.DayNight.NoActionBar",
                )),
                "@style/FrustAppTheme",
                Expect::Satisfied,
            ),
            (
                "app-style-m3",
                Some(styles(
                    "    <style name=\"Brand\" parent=\"Theme.Material3.Light\" />\n\
                     \x20   <style name=\"FrustAppTheme\" parent=\"Brand\" />\n",
                )),
                "@style/FrustAppTheme",
                Expect::Satisfied,
            ),
            (
                "app-style-platform",
                Some(styles(
                    "    <style name=\"Brand\" parent=\"android:Theme.Material.Light\" />\n\
                     \x20   <style name=\"FrustAppTheme\" parent=\"Brand\" />\n",
                )),
                "@style/FrustAppTheme",
                Expect::Rewritten,
            ),
            (
                "app-style-unresolved",
                Some(parent_style("Theme.AppCompat.Light.NoActionBar")),
                "@style/FrustAppTheme",
                Expect::Satisfied,
            ),
            (
                "material-components-manifest",
                None,
                "@style/Theme.MaterialComponents.DayNight.NoActionBar",
                Expect::Satisfied,
            ),
            (
                "unused-style",
                Some(parent_style("android:Theme.Material.NoActionBar")),
                PLATFORM,
                Expect::Refused,
            ),
        ];

        for (case, themes, manifest_theme, expect) in cases {
            let app = scaffold_app(&root.join(case));
            match &themes {
                Some(text) => fs::write(app.join(apply::THEMES_REL), text).unwrap(),
                None => fs::remove_file(app.join(apply::THEMES_REL)).unwrap(),
            }
            let manifest = read(&app, apply::MANIFEST_REL);
            fs::write(
                app.join(apply::MANIFEST_REL),
                manifest.replace("@style/FrustAppTheme", manifest_theme),
            )
            .unwrap();
            let pending = pending_contributions(&app, &entry, &[]).unwrap();
            let before = snapshot(&app);
            match expect {
                Expect::Satisfied => {
                    assert!(pending.is_empty(), "{case}: {pending:?}");
                    let report = add(&app).unwrap();
                    assert_eq!(
                        report.items[0].outcome,
                        AddOutcome::AlreadyPresent,
                        "{case}"
                    );
                    assert_eq!(snapshot(&app), before, "{case}: nothing written");
                }
                Expect::Rewritten => {
                    assert_eq!(pending.len(), 1, "{case}: {pending:?}");
                    assert_eq!(pending[0].manual_edits(), None, "{case}");
                    check_anchors(&app, &pending).unwrap();
                    let report = add(&app).unwrap();
                    assert_eq!(report.items[0].outcome, AddOutcome::Applied, "{case}");
                    let after = read(&app, apply::THEMES_REL);
                    assert!(
                        after.contains(
                            "<style name=\"FrustAppTheme\" parent=\"Theme.Material3.DayNight.NoActionBar\" />"
                        ),
                        "{case}: {after}"
                    );
                    assert!(
                        after.contains("parent=\"android:Theme.Material.Light\""),
                        "{case}: the app's own style keeps its parent: {after}"
                    );
                    assert!(
                        pending_contributions(&app, &entry, &[]).unwrap().is_empty(),
                        "{case}: nothing pending after the add"
                    );
                }
                Expect::Refused => {
                    assert_eq!(pending.len(), 1, "{case}: {pending:?}");
                    let edits = pending[0].manual_edits().expect("manual edits");
                    assert!(edits.contains(apply::THEMES_REL), "{edits}");
                    assert!(edits.contains(apply::MANIFEST_REL), "{edits}");
                    assert!(
                        matches!(
                            check_anchors(&app, &pending),
                            Err(PluginAddError::AndroidAppThemeMissing { .. })
                        ),
                        "{case}"
                    );
                    assert!(
                        matches!(
                            add(&app),
                            Err(PluginAddError::AndroidAppThemeMissing { .. })
                        ),
                        "{case}"
                    );
                    assert_eq!(snapshot(&app), before, "{case}: nothing written");
                }
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_shared_cxx_runtime_is_never_pending_without_android() {
        let root = temp_root("libcxx-no-android");
        let app = scaffold_app(&root);
        fs::remove_dir_all(app.join("android")).unwrap();
        let entry = CatalogEntry::built_in(crate::plugin::PluginSpec {
            id: Cow::Borrowed("acme-native"),
            crate_dir: Cow::Borrowed("acme-native"),
            base: Cow::Owned(vec![Contribution::AndroidLibcxxShared]),
            ..crate::plugin::PluginSpec::DEFAULT
        });
        assert!(pending_contributions(&app, &entry, &[]).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_theme_parent_is_never_pending_without_android() {
        let root = temp_root("theme-parent-no-android");
        let app = scaffold_app(&root);
        fs::remove_dir_all(app.join("android")).unwrap();
        assert!(
            pending_contributions(&app, &theme_entry(), &[])
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(&root);
    }
}
