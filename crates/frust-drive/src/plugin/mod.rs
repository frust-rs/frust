//! Static plugin registry + idempotent project mutations.
//!
//! A generated frust project ships **clean** — no plugin code, permissions,
//! or platform contributions are scaffolded (the template-hygiene decision).
//! Instead, a plugin's contributions are applied to an existing generated
//! project on demand: [`add_plugin`] performs the exact edits a plugin's
//! README documents (a Cargo.toml dependency, an Info.plist key, a Gradle
//! library-module include, an Android manifest permission), each **idempotent**
//! — running it twice yields a byte-identical tree, every line item reported
//! [`AddOutcome::AlreadyPresent`] the second time.
//!
//! A plugin's Android Kotlin is **not** copied into the app: it ships as the
//! plugin's own `com.android.library` module under `platform/android/`, wired
//! in by [`Contribution::GradleModule`], with its permission carried by the
//! manifest merger and its R8 keep rules by `consumerProguardFiles`. That is
//! why there is no `KotlinFile`/`ProguardRule` contribution — a copied file
//! and a hand-appended keep rule both drift from the plugin they came from.
//!
//! The desktop lane ([`Contribution::MacosPlistEntry`]/
//! [`Contribution::MacosEntitlement`]/[`Contribution::LinuxDesktopEntry`])
//! never edits a project file at all: unlike every mobile contribution above,
//! `add_plugin` only *records* one (reporting
//! [`AddOutcome::AppliedAtBuild`]), and the desktop bundle a later
//! `frust build macos|windows|linux` assembles applies it fresh every time —
//! see [`apply::desktop_contributions`], the detection API a bundle-assembly
//! task consumes.
//!
//! v1 is a **static in-crate registry** ([`known_plugins`]): the
//! `frust-plugin.toml` cargo-metadata discovery ARCHITECTURE.md sketches stays
//! the deferred v2 path (needed only once plugins live outside this repo).
//! Every mutation is format-preserving and never rewrites a file it can't
//! parse; this module is **print-free** like every other drive core — the
//! front-end (CLI/TUI) owns all output (see `docs/CODE_STANDARDS.md`).

pub mod apply;
pub mod registry;

pub use apply::{DesktopContribution, add_plugin, add_plugin_with, desktop_contributions};
pub use registry::known_plugins;

use std::path::PathBuf;

/// One plugin's registry entry: its identity, the crate-directory segment its
/// path dependency is derived from, and the contributions applying it makes.
///
/// All fields are `&'static` so the whole registry is const data (the
/// `default_validators()`/`default_discoverers()` Vec-factory convention).
#[derive(Debug, Clone, Copy)]
pub struct PluginSpec {
    /// Stable id a caller (CLI/TUI) selects by, e.g. `"secure-storage"`.
    pub id: &'static str,
    /// One-line human summary for a selection UI.
    pub summary: &'static str,
    /// Path segment under `<frust repo>/plugins/` the crate lives in. In path
    /// mode the project's `frust = { path = ... }` dep is walked back to the
    /// repo root and down into this directory to write the plugin's own path
    /// dep; every [`Contribution::GradleModule`]/[`Contribution::SwiftPackageRef`]
    /// `rel_path` must sit under `plugins/<crate_dir>/`, since only the part
    /// below it is carried over to wherever cargo locates the package.
    pub crate_dir: &'static str,
    /// Contributions applied unconditionally when this plugin is added
    /// (always at least its own Cargo.toml dependency).
    pub base: &'static [Contribution],
    /// Opt-in feature bundles a caller can request by id (e.g.
    /// secure-storage's `"biometric-gate"`), each adding further contributions.
    pub optional_features: &'static [FeatureSpec],
    /// A sibling checkout this plugin needs present (facade-tier plugins whose
    /// own deps path into it), declared relative to the frust repo root —
    /// two levels above the facade package cargo locates for the project.
    /// [`add_plugin`] errors [`PluginAddError::SiblingCheckoutMissing`] when
    /// it isn't on disk. No current registry entry sets this —
    /// `clean-signals-frust` needs none because `clean-signals` is a crates.io
    /// dependency (see `registry.rs`'s `CLEAN_SIGNALS_FRUST`) — but the
    /// mechanism stays in place for a future facade-tier plugin that does.
    pub requires_sibling: Option<&'static str>,
}

/// An opt-in bundle of contributions under a [`PluginSpec`], selected by id.
#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {
    /// Feature id a caller passes in `add_plugin(.., features)`.
    pub id: &'static str,
    /// One-line human summary (a checkbox row lists exactly what it adds).
    pub summary: &'static str,
    /// The extra contributions enabling this feature applies.
    pub contributions: &'static [Contribution],
}

/// A single idempotent edit a plugin makes to a generated project. Each maps
/// to one target file and one skip-if-present guard.
#[derive(Debug, Clone, Copy)]
pub enum Contribution {
    /// A `[dependencies]` entry written in the form the project's own `frust`
    /// dependency takes: a path into the same checkout (path mode), or the
    /// same version requirement (registry mode — the plugin crates release in
    /// lockstep with the facade). `name` is the crate/dependency name, and
    /// the package the plugin's platform files are located in.
    CargoDep { name: &'static str },
    /// A `<uses-permission android:name="..."/>` line inserted into the app's
    /// own `AndroidManifest.xml` before `</manifest>`.
    ///
    /// **Prefer [`Contribution::GradleModule`]**: a permission declared in a
    /// plugin module's own `src/main/AndroidManifest.xml` is folded into the
    /// app by the manifest merger, so the app's manifest is never edited and
    /// the permission cannot outlive the plugin. Reach for this variant only
    /// when the permission genuinely belongs to the *app* rather than to the
    /// plugin's module — e.g. one whose presence depends on app-level policy
    /// the merger cannot supply. No plugin uses it today.
    ManifestPermission { permission: &'static str },
    /// A `<key>/<string>` pair (with an explanatory XML comment) inserted into
    /// `Info.plist` before the root `</dict>`.
    PlistEntry {
        key: &'static str,
        value: &'static str,
        comment: &'static str,
    },
    /// A Gradle library module included into the generated project: appends
    /// the `include(...)` + `projectDir` (+ build-directory redirect) lines to
    /// `android/settings.gradle.kts` and the `implementation(project(...))`
    /// line to `android/app/build.gradle.kts`, both idempotently.
    ///
    /// One contribution, two files, **one** [`AddItem`] — the report is a
    /// per-contribution ledger, not a per-file one. A half-applied state (one
    /// file wired, the other not) completes the other file and reports
    /// [`AddOutcome::Applied`].
    GradleModule {
        /// The Gradle project path, e.g. `":frust-secure-storage"`.
        gradle_name: &'static str,
        /// The module directory, relative to the frust repo root, e.g.
        /// `"plugins/secure-storage/platform/android"`. Written into the
        /// project as the same subdirectory of the plugin's package wherever
        /// cargo locates it (`<package>/platform/android`).
        rel_path: &'static str,
    },
    /// A plugin's local Swift package added to the generated app's
    /// `ios/Runner.xcodeproj/project.pbxproj` as a second package reference
    /// beside the embedding's `FrustEmbedding` — the iOS counterpart of
    /// [`Contribution::GradleModule`].
    ///
    /// One contribution, **six** pbxproj sites, **one** [`AddItem`] (the
    /// `GradleModule` precedent, widened): the `PBXBuildFile` object and its
    /// entry in the Frameworks build phase, the target's
    /// `packageProductDependencies` entry, the project's `packageReferences`
    /// entry, and the `XCLocalSwiftPackageReference` /
    /// `XCSwiftPackageProductDependency` objects themselves. A half-applied
    /// state completes the missing sites and reports [`AddOutcome::Applied`];
    /// only a fully-wired project reports [`AddOutcome::AlreadyPresent`].
    ///
    /// The whole edit is built in memory, re-scanned, and written once — a
    /// half-edited `project.pbxproj` will not open in Xcode.
    SwiftPackageRef {
        /// The Swift package *and* product name, e.g. `"FrustCamera"` — both
        /// the `XCLocalSwiftPackageReference` comment and the
        /// `XCSwiftPackageProductDependency`'s `productName`, matching how the
        /// embedding's own wiring names `FrustEmbedding`.
        package_name: &'static str,
        /// The package directory, relative to the frust repo root, e.g.
        /// `"plugins/camera/platform/ios"` — resolved exactly like a
        /// [`Contribution::GradleModule`]'s `rel_path`. A located package is
        /// written as an absolute path; only a project-root-relative fallback
        /// needs the extra `../` for Xcode resolving `relativePath` against
        /// the directory *containing* the `.xcodeproj` (`<project>/ios/`).
        rel_path: &'static str,
    },
    /// A system framework linked into the generated app's Runner target by
    /// name: `"-framework", <name>,` appended to every `OTHER_LDFLAGS` list
    /// in `ios/Runner.xcodeproj/project.pbxproj` (one per build
    /// configuration).
    ///
    /// For a plugin whose Apple arm is pure Rust (`objc2`) rather than a
    /// Swift package. A crate's `#[link(name = "…", kind = "framework")]`
    /// reaches the linker only when rustc links the binary itself (macOS);
    /// the iOS app is an Xcode-linked staticlib, and a staticlib carries no
    /// framework link into that step — so a framework whose symbols the Rust
    /// code references by name (an `extern static` such as
    /// `ASWebAuthenticationSessionErrorDomain`) must be named here or the
    /// app fails to link (`auth-session`'s device gate found exactly that).
    /// A framework the scaffold already links (Metal, QuartzCore, CoreText,
    /// CoreGraphics, CoreFoundation, UIKit) needs no contribution.
    ///
    /// One contribution, one file, one [`AddItem`]: a half-applied state
    /// (the flag in some configurations but not all) completes the rest and
    /// reports [`AddOutcome::Applied`]; only a project whose every
    /// `OTHER_LDFLAGS` list already carries the flag reports
    /// [`AddOutcome::AlreadyPresent`].
    IosFramework {
        /// The framework's name as `-framework` takes it, e.g.
        /// `"AuthenticationServices"` — no `.framework` suffix.
        name: &'static str,
    },
    /// A macro invocation appended to the app crate's `src/lib.rs`.
    ///
    /// The escape hatch for a plugin whose platform side needs something the
    /// **app crate itself** must emit — because the app crate is the staticlib
    /// root, and some linker-visible properties only hold when the reference
    /// originates there.
    ///
    /// It exists for exactly one measured reason (`frust-camera`'s Swift
    /// integration): a `#[unsafe(no_mangle)]` C export that lives in a
    /// *dependency* crate and
    /// is called only from Swift is dropped by the release profile's
    /// `lto = "fat"` before the Swift side links, because nothing in Rust
    /// references it. An app that merely *added* the plugin and never calls its
    /// API is precisely the failing case — and precisely what Add Plugin
    /// produces. Planting a `#[used]` reference in the app crate keeps LTO from
    /// treating the symbol as dead.
    ///
    /// Prefer any other variant. Reach for this only when the contribution
    /// genuinely cannot live in the plugin's own crate or platform module.
    AppCrateMacro {
        /// The invocation to append, e.g. `"frust_camera::ios_exports!();"`.
        /// Also the idempotence key — an exact substring match against the
        /// existing file means the edit is already present.
        invocation: &'static str,
        /// An optional `#[cfg(...)]` predicate written above the invocation,
        /// e.g. `"target_vendor = \"apple\""`. `None` emits it unguarded.
        cfg: Option<&'static str>,
        /// A short `//` comment written above, explaining why the app crate has
        /// to carry this.
        comment: &'static str,
    },
    /// Enables a cargo feature on an already-contributed plugin dependency
    /// line in the app's `Cargo.toml`.
    ///
    /// The dependency **must already exist** — a [`Contribution::CargoDep`]
    /// for `name` applied by the same or an earlier contribution. Applying
    /// this to a missing dep is [`PluginAddError::NoSuchCargoDep`], never a
    /// silent dep creation (a feature with no base dependency line is a
    /// registry bug, not something to paper over). The edit is
    /// **idempotent**: re-applying pushes the feature into the dep's
    /// `features` array only if it isn't already there, never duplicating
    /// it.
    CargoFeature {
        /// The dependency name whose inline table gets a `features` entry —
        /// must match a `name` already contributed via
        /// [`Contribution::CargoDep`].
        name: &'static str,
        /// The cargo feature to enable, e.g. `"turso"`.
        feature: &'static str,
    },
    /// Creates a file at `rel_path` (relative to the project root) with
    /// exact `contents`, if it doesn't already exist — parent directories
    /// are created as needed.
    ///
    /// Every other variant above edits an existing project file at an
    /// anchor; this is the one that creates a file that might not exist
    /// yet. The idempotency guard is **presence alone**, never a content
    /// comparison: an existing file — even one whose contents differ from
    /// `contents`, because the user hand-edited it (a locale string, for
    /// instance) — is left untouched and reports
    /// [`AddOutcome::AlreadyPresent`]. `rel_path` must be a relative path
    /// with no `..` component ([`PluginAddError::UnsafeScaffoldPath`]) —
    /// checked defensively even though every registry entry is a static,
    /// trusted string, matching this module's existing defensive tone.
    ScaffoldFile {
        /// The path to create, relative to the project root, e.g.
        /// `"locales/en/main.ftl"`.
        rel_path: &'static str,
        /// The exact contents written when the file is absent.
        contents: &'static str,
        /// A short human-readable explanation for a selection/report UI —
        /// not written into the file itself.
        comment: &'static str,
    },
    /// A `<key>/<string>` pair merged into the assembled macOS bundle's
    /// `Contents/Info.plist` at `frust build macos` time — **not** into the
    /// project's own `macos/Info.plist` (a later bundle-assembly task
    /// consumes it via [`apply::desktop_contributions`]).
    ///
    /// Project desktop files (`macos/Info.plist`, `macos/app.entitlements`,
    /// `linux/app.desktop`) are user-owned, hand-editable, reconciled files
    /// — and a pre-Phase-B project may not even have them yet. So, unlike
    /// [`Contribution::PlistEntry`]'s mobile counterpart, a desktop
    /// contribution is never written into a project file at Add Plugin
    /// time: `add_plugin` only *records* it (reporting
    /// [`AddOutcome::AppliedAtBuild`]), and the assembled bundle applies it
    /// fresh on **every** `frust build macos|windows|linux` — the same
    /// "cannot outlive the plugin" property [`Contribution::GradleModule`]'s
    /// own doc argues for, reached here by build-time application instead of
    /// a manifest merger: remove the plugin dependency and the contribution
    /// simply stops being applied, with no file left carrying it.
    MacosPlistEntry {
        key: &'static str,
        value: &'static str,
        comment: &'static str,
    },
    /// A boolean-true entitlement (`<key>k</key>` / `<true/>`) merged into
    /// the entitlements passed to `codesign` at `frust build macos` time.
    ///
    /// v1 is boolean-true only — the dominant entitlement shape;
    /// value-carrying entitlements are a future widening. Applied at build
    /// time for the same reason [`Contribution::MacosPlistEntry`] is (see
    /// its doc comment): `macos/app.entitlements` is a user-owned,
    /// hand-editable project file, not an Add Plugin write target.
    MacosEntitlement {
        key: &'static str,
        comment: &'static str,
    },
    /// A `[Desktop Entry]` `key=value` line merged into the assembled Linux
    /// bundle's `<identifier>.desktop` at `frust build linux` time, only
    /// when the key is absent — an existing key (user-owned) always wins.
    ///
    /// Applied at build time for the same reason
    /// [`Contribution::MacosPlistEntry`] is (see its doc comment):
    /// `linux/app.desktop` is a user-owned, hand-editable project file a
    /// pre-Phase-B project may not even have at all.
    LinuxDesktopEntry {
        key: &'static str,
        value: &'static str,
        comment: &'static str,
    },
}

impl Contribution {
    /// A stable, human-readable label for this edit — the vocabulary the
    /// `AddReport` line items and a report UI use.
    pub fn describe(&self) -> String {
        match self {
            Contribution::CargoDep { name } => format!("Cargo.toml dependency `{name}`"),
            Contribution::ManifestPermission { permission } => {
                format!("AndroidManifest.xml permission `{permission}`")
            }
            Contribution::PlistEntry { key, .. } => format!("Info.plist key `{key}`"),
            Contribution::GradleModule { gradle_name, .. } => {
                format!("Gradle module `{gradle_name}`")
            }
            Contribution::SwiftPackageRef { package_name, .. } => {
                format!("Xcode Swift package `{package_name}`")
            }
            Contribution::IosFramework { name } => {
                format!("Xcode framework `{name}` (`OTHER_LDFLAGS`)")
            }
            Contribution::AppCrateMacro { invocation, .. } => {
                format!("app crate `src/lib.rs` invocation `{invocation}`")
            }
            Contribution::CargoFeature { name, feature } => {
                format!("Cargo.toml dependency `{name}` feature `{feature}`")
            }
            Contribution::ScaffoldFile { rel_path, .. } => {
                format!("scaffolded file `{rel_path}`")
            }
            Contribution::MacosPlistEntry { key, .. } => {
                format!("Info.plist key `{key}` (applied at `frust build macos`)")
            }
            Contribution::MacosEntitlement { key, .. } => {
                format!("entitlement `{key}` (applied at `frust build macos`)")
            }
            Contribution::LinuxDesktopEntry { key, .. } => {
                format!("desktop entry `{key}` (applied at `frust build linux`)")
            }
        }
    }
}

/// Whether a single [`Contribution`] made a change or was already there —
/// the per-item idempotency signal a report renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// The edit was made this run.
    Applied,
    /// The edit was already present; nothing was written.
    AlreadyPresent,
    /// The contribution is recorded in the registry and applied by every
    /// `frust build <os>`; `add_plugin` performs no project-file edit for it
    /// (see [`Contribution::MacosPlistEntry`] and its two siblings). Never
    /// [`AddOutcome::AlreadyPresent`] — nothing is written here for a second
    /// run to find already present.
    AppliedAtBuild,
}

/// One line of an [`AddReport`]: what the edit was and whether it applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddItem {
    /// Human-readable target (see [`Contribution::describe`]).
    pub description: String,
    /// Applied this run, or already present.
    pub outcome: AddOutcome,
}

/// The result of [`add_plugin`]: the plugin added and a per-edit line item
/// list. Re-running `add_plugin` with the same arguments yields the same items
/// all reporting [`AddOutcome::AlreadyPresent`] (the idempotency contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddReport {
    /// The plugin id that was added.
    pub plugin_id: String,
    /// One item per contribution, in application order.
    pub items: Vec<AddItem>,
}

impl AddReport {
    /// `(applied, other)` line-item counts — a report header summary. The
    /// second bucket lumps [`AddOutcome::AlreadyPresent`] and
    /// [`AddOutcome::AppliedAtBuild`] together (neither wrote anything this
    /// run): a caller rendering its own per-bucket label must not
    /// blanket-describe that bucket as "already present" — a desktop-lane
    /// item's honest outcome is [`AddOutcome::AppliedAtBuild`], never
    /// [`AddOutcome::AlreadyPresent`].
    ///
    /// Kept for compatibility with any caller that only needs the coarse
    /// two-bucket shape; a caller that renders a per-outcome label (every
    /// current one does) wants [`Self::outcome_counts`] instead — the one
    /// counting rule the whole codebase shares.
    pub fn counts(&self) -> (usize, usize) {
        let applied = self
            .items
            .iter()
            .filter(|i| i.outcome == AddOutcome::Applied)
            .count();
        (applied, self.items.len() - applied)
    }

    /// `(applied, already_present, applied_at_build)` line-item counts — the
    /// one truthful counting rule every report-rendering caller (the TUI
    /// engine's toast, the add-plugin view's header) shares, so a
    /// desktop-lane item is never mislabeled "already present". Additive to
    /// [`Self::counts`], which stays for its coarser two-bucket callers.
    pub fn outcome_counts(&self) -> (usize, usize, usize) {
        let mut applied = 0;
        let mut already_present = 0;
        let mut applied_at_build = 0;
        for item in &self.items {
            match item.outcome {
                AddOutcome::Applied => applied += 1,
                AddOutcome::AlreadyPresent => already_present += 1,
                AddOutcome::AppliedAtBuild => applied_at_build += 1,
            }
        }
        (applied, already_present, applied_at_build)
    }
}

/// Typed failures a caller matches on (see `docs/CODE_STANDARDS.md`'s Error
/// Handling — a library contract, `thiserror`-derived).
#[derive(Debug, thiserror::Error)]
pub enum PluginAddError {
    /// No registry entry has this id (see [`known_plugins`]).
    #[error("unknown plugin `{0}` (see `known_plugins()`)")]
    UnknownPlugin(String),
    /// The plugin exists but has no such optional feature.
    #[error("plugin `{plugin}` has no optional feature `{feature}`")]
    UnknownFeature { plugin: String, feature: String },
    /// A project file the edit needs is absent — the target isn't a generated
    /// frust project root, or lacks the platform tree this contribution edits.
    #[error("project file `{0}` not found (is this a generated frust project root?)")]
    MissingProjectFile(String),
    /// A project file exists but lacks the expected insertion anchor
    /// (`</manifest>` / `</dict>`) — never rewritten.
    #[error("project file `{0}` is malformed (missing expected insertion anchor)")]
    MalformedProjectFile(String),
    /// The project's Cargo.toml doesn't parse — never rewritten.
    #[error("failed to parse the project's Cargo.toml: {0}")]
    UnparseableCargoToml(String),
    /// No `frust` dependency carrying a `path` or a `version` — nothing to
    /// write a plugin dependency against.
    #[error(
        "no `frust` dependency with a `path` or a `version` to write the plugin's dependency against"
    )]
    NoFrustDependency,
    /// Cargo could not say where a package lives in a project depending on
    /// the crates.io release, where — unlike a checkout path — there is no
    /// other place to derive it from.
    #[error("could not locate the `{package}` package through cargo: {source}")]
    PackageNotLocated {
        package: String,
        source: crate::packages::PackagesError,
    },
    /// A registry entry names a platform path outside its plugin's own
    /// package (`plugins/<crate_dir>/…`), which no package lookup can
    /// resolve — a registry bug, refused before anything is written.
    #[error(
        "plugin `{plugin}` names platform path `{rel_path}` outside its own package \
         (`plugins/{crate_dir}/…`, added by its base Cargo dependency), so it cannot be located"
    )]
    NativePathOutsidePackage {
        plugin: String,
        rel_path: String,
        crate_dir: String,
    },
    /// A freshly minted `project.pbxproj` object id is already in use.
    /// Unreachable while the mint scans the same file it writes, but a
    /// collision would silently redefine an existing object — the one
    /// corruption an idempotent applier must never risk — so it fails loudly
    /// instead ([`Contribution::SwiftPackageRef`]).
    #[error(
        "project file `{file}` already uses minted pbxproj object id `{id}` (refusing to overwrite)"
    )]
    PbxIdCollision { file: String, id: String },
    /// The `ABCD…00NN` object-id space of a `project.pbxproj` is full (`NN` is
    /// two hex digits, so 256 ids) — nothing is written.
    #[error("project file `{file}` has no free `…00NN` pbxproj object id left")]
    PbxIdSpaceExhausted { file: String },
    /// A [`Contribution::SwiftPackageRef`]'s or [`Contribution::IosFramework`]'s
    /// in-memory edit failed its own re-scan: one of the package's six sites
    /// is missing or carries a mismatched id, or one `OTHER_LDFLAGS` list
    /// still lacks the framework, so the file is left untouched rather than
    /// written half-wired.
    #[error(
        "pbxproj wiring for `{package}` in `{file}` failed verification (site `{site}`); \
         nothing was written"
    )]
    PbxWiringNotVerified {
        file: String,
        package: String,
        site: String,
    },
    /// A required sibling checkout (facade plugin) is not on disk.
    #[error("required sibling checkout `{sibling}` not found (expected at `{}`)", .expected.display())]
    SiblingCheckoutMissing { sibling: String, expected: PathBuf },
    /// A filesystem write failed.
    #[error("writing `{path}`: {message}")]
    Io { path: String, message: String },
    /// A [`Contribution::CargoFeature`] named a dependency with no existing
    /// `[dependencies]` entry — a `CargoDep` contribution for that name must
    /// apply first (in the same or an earlier `add_plugin` call).
    #[error("no `[dependencies].{name}` entry to enable feature `{feature}` on")]
    NoSuchCargoDep { name: String, feature: String },
    /// A [`Contribution::ScaffoldFile`]'s `rel_path` is not a safe relative
    /// path — absolute, or containing a `..` component. Registry entries
    /// are static, trusted strings, but the check stays defensive rather
    /// than assuming that forever.
    #[error("scaffold file path `{0}` must be a relative path with no `..` component")]
    UnsafeScaffoldPath(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_lane_describe_strings_name_their_build_time() {
        assert_eq!(
            Contribution::MacosPlistEntry {
                key: "NSSupportsSuddenTermination",
                value: "NO",
                comment: "test",
            }
            .describe(),
            "Info.plist key `NSSupportsSuddenTermination` (applied at `frust build macos`)"
        );
        assert_eq!(
            Contribution::MacosEntitlement {
                key: "com.apple.security.network.client",
                comment: "test",
            }
            .describe(),
            "entitlement `com.apple.security.network.client` (applied at `frust build macos`)"
        );
        assert_eq!(
            Contribution::LinuxDesktopEntry {
                key: "Categories",
                value: "Utility;",
                comment: "test",
            }
            .describe(),
            "desktop entry `Categories` (applied at `frust build linux`)"
        );
    }

    fn item(outcome: AddOutcome) -> AddItem {
        AddItem {
            description: "test item".to_string(),
            outcome,
        }
    }

    #[test]
    fn outcome_counts_buckets_all_three_outcomes_separately() {
        let report = AddReport {
            plugin_id: "test-plugin".to_string(),
            items: vec![
                item(AddOutcome::Applied),
                item(AddOutcome::Applied),
                item(AddOutcome::AlreadyPresent),
                item(AddOutcome::AppliedAtBuild),
            ],
        };
        assert_eq!(report.outcome_counts(), (2, 1, 1));
        // `counts()` stays the coarser two-bucket shape it always was.
        assert_eq!(report.counts(), (2, 2));
    }

    #[test]
    fn outcome_counts_of_an_empty_report_is_all_zero() {
        let report = AddReport {
            plugin_id: "test-plugin".to_string(),
            items: vec![],
        };
        assert_eq!(report.outcome_counts(), (0, 0, 0));
    }

    #[test]
    fn outcome_counts_never_lumps_applied_at_build_with_already_present() {
        let report = AddReport {
            plugin_id: "test-plugin".to_string(),
            items: vec![item(AddOutcome::AppliedAtBuild)],
        };
        assert_eq!(report.outcome_counts(), (0, 0, 1));
    }
}
