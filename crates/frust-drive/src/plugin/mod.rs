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
//! v1 is a **static in-crate registry** ([`known_plugins`]): the
//! `frust-plugin.toml` cargo-metadata discovery ARCHITECTURE.md sketches stays
//! the deferred v2 path (needed only once plugins live outside this repo).
//! Every mutation is format-preserving and never rewrites a file it can't
//! parse; this module is **print-free** like every other drive core — the
//! front-end (CLI/TUI) owns all output (see `docs/CODE_STANDARDS.md`).

pub mod apply;
pub mod registry;

pub use apply::add_plugin;
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
    /// Path segment under `<frust repo>/plugins/` the crate lives in — the
    /// `frust = { path = ... }` dep the project already has is walked back to
    /// the repo root and down into this directory to derive the dep path.
    pub crate_dir: &'static str,
    /// Contributions applied unconditionally when this plugin is added
    /// (always at least its own Cargo.toml dependency).
    pub base: &'static [Contribution],
    /// Opt-in feature bundles a caller can request by id (e.g.
    /// secure-storage's `"biometric-gate"`), each adding further contributions.
    pub optional_features: &'static [FeatureSpec],
    /// A sibling checkout this plugin needs present (facade-tier plugins whose
    /// own deps path into it), declared relative to the frust repo root — e.g.
    /// clean-signals-frust's `"../clean-signals-rs"`. [`add_plugin`] errors
    /// [`PluginAddError::SiblingCheckoutMissing`] when it isn't on disk.
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
    /// A `[dependencies]` entry; the path is derived at apply time from the
    /// project's existing `frust` path dep (crates unpublished — version deps
    /// come post-publish). `name` is the crate/dependency name.
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
        /// `"plugins/secure-storage/platform/android"`.
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
        /// `"plugins/camera/platform/ios/FrustCamera"` — resolved exactly like
        /// a [`Contribution::GradleModule`]'s `rel_path`, since the written
        /// `relativePath` is resolved by Xcode against the directory
        /// *containing* the `.xcodeproj` (`<project>/ios/`), one level below
        /// the project root a relative `frust` path dep is written against.
        rel_path: &'static str,
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
            Contribution::AppCrateMacro { invocation, .. } => {
                format!("app crate `src/lib.rs` invocation `{invocation}`")
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
    /// `(applied, already_present)` line-item counts — a report header summary.
    pub fn counts(&self) -> (usize, usize) {
        let applied = self
            .items
            .iter()
            .filter(|i| i.outcome == AddOutcome::Applied)
            .count();
        (applied, self.items.len() - applied)
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
    /// No `frust = {{ path = ... }}` dependency to derive plugin paths from.
    #[error("no `frust = {{ path = ... }}` dependency to derive plugin paths from")]
    NoFrustDependency,
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
    /// A [`Contribution::SwiftPackageRef`]'s in-memory edit failed its own
    /// re-scan: one of the six sites is missing or carries a mismatched id, so
    /// the file is left untouched rather than written half-wired.
    #[error(
        "Swift package `{package}` wiring for `{file}` failed verification (site `{site}`); \
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
}
