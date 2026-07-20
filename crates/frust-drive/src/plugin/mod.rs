//! Static plugin registry + idempotent project mutations
//! (`frust-secure-storage` PLAN.md Phase 6).
//!
//! A generated frust project ships **clean** — no plugin code, permissions,
//! or platform contributions are scaffolded (the template-hygiene decision).
//! Instead, a plugin's contributions are applied to an existing generated
//! project on demand: [`add_plugin`] performs the exact edits a plugin's
//! README documents (a Cargo.toml dependency, an Android manifest permission,
//! an Info.plist key, a Kotlin helper file, an R8 keep rule), each **idempotent**
//! — running it twice yields a byte-identical tree, every line item reported
//! [`AddOutcome::AlreadyPresent`] the second time.
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
    /// A `<uses-permission android:name="..."/>` line inserted into
    /// `AndroidManifest.xml` before `</manifest>`.
    ManifestPermission { permission: &'static str },
    /// A `<key>/<string>` pair (with an explanatory XML comment) inserted into
    /// `Info.plist` before the root `</dict>`.
    PlistEntry {
        key: &'static str,
        value: &'static str,
        comment: &'static str,
    },
    /// A Kotlin helper written into `android/app/src/main/kotlin/...`,
    /// write-if-absent; `contents` is embedded from the plugin's canonical
    /// `platform/` copy at compile time.
    KotlinFile {
        relative_path: &'static str,
        contents: &'static str,
    },
    /// A marker-commented keep rule appended to `proguard-rules.pro`,
    /// skip-if-marker-present.
    ProguardRule {
        marker: &'static str,
        rule: &'static str,
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
            Contribution::KotlinFile { relative_path, .. } => {
                format!("Kotlin helper `{relative_path}`")
            }
            Contribution::ProguardRule { rule, .. } => format!("proguard-rules.pro rule `{rule}`"),
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
    /// A required sibling checkout (facade plugin) is not on disk.
    #[error("required sibling checkout `{sibling}` not found (expected at `{}`)", .expected.display())]
    SiblingCheckoutMissing { sibling: String, expected: PathBuf },
    /// A filesystem write failed.
    #[error("writing `{path}`: {message}")]
    Io { path: String, message: String },
}
