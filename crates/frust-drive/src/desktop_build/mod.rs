//! Desktop bundle assembly: `cargo build` plus pure directory/file work into
//! the per-OS layout a user can actually double-click or ship.
//!
//! Three targets, one pipeline ([`build`]): resolve `frust.toml`'s desktop
//! identity, compile the app through [`crate::process::ProcessRunner`], locate
//! the produced binary, and lay out
//!
//! - macOS — `build/desktop/macos/<Display Name>.app/Contents/{MacOS,Resources}`
//!   + `Info.plist`, optionally `codesign`ed;
//! - Windows — `build/desktop/windows/<binary>.exe` (the icon is embedded by
//!   the project's own `windows/build.rs`, which this pipeline feeds by
//!   generating `build/desktop/windows/icon.ico` *before* the compile);
//! - Linux — `build/desktop/linux/<binary>/` with the binary, a
//!   `<identifier>.desktop` entry and a `share/icons/hicolor/` tree.
//!
//! **Host-locked**, like every other desktop toolchain: a macOS bundle can
//! only be assembled on macOS, and so on. The check reads
//! [`DesktopBundleTarget::host`] rather than a `cfg!`, so all three assemblies
//! are exercised by this module's own tests on one machine.
//!
//! **Print-free core.** Compile output, codesign output and progress lines all
//! leave through the `on_line` sink (`docs/CODE_STANDARDS.md`'s printing
//! anti-pattern, `tests/print_free_cores.rs`); everything else a caller might
//! want to show — skipped icons, a generated `Info.plist`, an unsigned
//! bundle — comes back as a typed [`BundleNote`] on the [`BundleReport`].
//!
//! **The project's own files win.** A scaffolded project ships `macos/`,
//! `windows/` and `linux/` template files rendered at `frust create` time;
//! this pipeline copies them verbatim rather than re-rendering them, so a hand
//! edit survives every build. The `[desktop]`/`[macos]`/`[linux]` manifest
//! values are the *fallback* source: a project scaffolded before those
//! templates existed (there is no `frust upgrade`) still gets a correct
//! `Info.plist`/`.desktop` generated from `frust.toml` instead of a failed
//! build.
//!
//! **Winning verbatim means the two can disagree, so a copied file is read
//! back and reconciled.** The executable, identifier and icon file *names* a
//! build produces come from the resolved manifest, while a copied
//! `Info.plist`/`app.desktop` names whatever it names — a rename in one place
//! and not the other otherwise ships a `.app` whose `CFBundleExecutable` points
//! at a file that isn't there, or a launcher entry with a dangling `Exec=`.
//! Nothing is rewritten (that is the decision above): the disagreement is
//! *reported*. One case is fatal — a `CFBundleExecutable` naming something
//! other than the binary in `Contents/MacOS` cannot launch, is fully
//! deterministic, and is therefore
//! [`DesktopBuildError::ExecutableIdentityMismatch`]. Every other drift is a
//! [`BundleNote`], because it degrades rather than breaks (a bundle keeps the
//! plist's identifier, a `.desktop` `Exec=` may legitimately resolve through
//! `PATH`). And a plist that cannot be *read back* at all — no `plutil`, a
//! non-zero exit, unparseable output — is one [`BundleNote::PlistUnverified`],
//! never a failure: tool trouble must not block a build (the same stance
//! `ios_run::bundle_id` takes on the identical tool).
//!
//! **Installed plugins contribute to the bundle, never to the project.** A
//! plugin declaring a macOS `Info.plist` key, a macOS entitlement or a Linux
//! desktop-entry key has it merged into the *assembled* bundle on every build
//! — after assembly, before `codesign` — rather than written into a project
//! file at `frust plugin add` time. See [`contributions`] for the merge rules
//! (an existing key always wins; an unappliable one is a hard refusal).
//!
//! **A missing or unusable icon never fails a build.** The bundle is assembled
//! without one and the reason comes back as a [`BundleNote`] — a bundle with
//! no icon still runs, and the scaffold's own placeholder logo is deliberately
//! below the icon pipeline's minimum source size.
//!
//! **But an identity value that isn't a safe path is a hard refusal.** The
//! display name, identifier and binary name each become a file or directory
//! under `build/desktop/` — one that a rebuild deletes recursively — so a value carrying
//! a separator, a `..`, or a leading `/` fails the build with a typed error
//! before anything is written ([`DesktopBuildError::UnsafeDesktopIdentity`],
//! and [`DesktopBuildError::UnsafeIconPath`] for an icon path pointing out of
//! the project). Note the line this draws through the icon rules above: an
//! icon that is *unusable* degrades the bundle and is a note, while an icon
//! path that *tries to leave the project* is a refusal. The first is a quality
//! question, the second a safety one, and a safety answer is never a note.
//! `bundle::prepare_dir` re-checks the same property against the actual
//! directory it is about to delete, so a future caller cannot reintroduce the
//! hole by assembling a path some other way.

mod bundle;
mod cargo;
mod config;
mod contributions;
mod installer;
mod linux;
mod macos;
mod windows;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::build_dirs::BuildLayout;
use crate::build_info::BuildInfo;
use crate::doctor::{EnvLookup, RealEnv};
use crate::manifest;
use crate::plugin::DesktopContribution;
use crate::process::ProcessRunner;

use self::config::DesktopConfig;

pub use installer::{
    CARGO_PACKAGER_PINNED, InstallerError, InstallerFormat, InstallerNote, InstallerReport,
    build_installer,
};

/// The project-relative directory every desktop build output lands under, and
/// the containment root [`bundle::prepare_dir`] refuses to step outside of —
/// `build/desktop`, derived from [`BuildLayout::desktop`] (any target works;
/// all three share the same parent) rather than duplicating its `"desktop"`
/// segment as a second literal.
fn desktop_root() -> PathBuf {
    BuildLayout::desktop(DesktopBundleTarget::Linux)
        .parent()
        .expect("BuildLayout::desktop always nests one segment under build/desktop")
        .to_path_buf()
}

/// The bundle `frust build macos|windows|linux` requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopBundleTarget {
    Macos,
    Windows,
    Linux,
}

impl DesktopBundleTarget {
    /// The lowercase target name — the `frust build <name>` subcommand word,
    /// and the `build/desktop/<name>/` output directory segment.
    pub fn as_str(self) -> &'static str {
        match self {
            DesktopBundleTarget::Macos => "macos",
            DesktopBundleTarget::Windows => "windows",
            DesktopBundleTarget::Linux => "linux",
        }
    }

    /// The target this host can assemble, or `None` on an OS with no desktop
    /// bundle layout of its own (a BSD, an unknown target). Read from
    /// [`std::env::consts::OS`] rather than a `cfg!` chain so the value is one
    /// expression instead of three conditionally-compiled ones.
    pub fn host() -> Option<DesktopBundleTarget> {
        match std::env::consts::OS {
            "macos" => Some(DesktopBundleTarget::Macos),
            "windows" => Some(DesktopBundleTarget::Windows),
            "linux" => Some(DesktopBundleTarget::Linux),
            _ => None,
        }
    }

    /// Whether this host can assemble this target — the host lock, in the
    /// form a front-end asks *before* offering the build (the CLI's target
    /// validation, the TUI's greyed-out build entry).
    pub fn supported_on_host(self) -> bool {
        DesktopBundleTarget::host() == Some(self)
    }

    /// The per-OS output root under the project: `build/desktop/macos`,
    /// `build/desktop/windows`, `build/desktop/linux`. The bundle itself lands
    /// inside it (see [`BundleReport::root`]).
    pub fn output_dir(self, project_dir: &Path) -> PathBuf {
        project_dir.join(BuildLayout::desktop(self))
    }
}

impl fmt::Display for DesktopBundleTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a successful assembly produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleReport {
    pub target: DesktopBundleTarget,
    /// The bundle root: the `.app` directory (macOS), `build/desktop/windows`
    /// (Windows), or `build/desktop/linux/<binary>` (Linux).
    pub root: PathBuf,
    /// The executable inside the bundle — what a launcher entry or a `dmg`
    /// packaging step points at.
    pub executable: PathBuf,
    /// Every file this run wrote, in write order (executable, icon
    /// container(s), launcher metadata). Directories are not listed.
    pub artifacts: Vec<PathBuf>,
    /// The entitlements file **this** build's `codesign` step was given: the
    /// generated `build/desktop/macos/<binary>.entitlements` when a plugin
    /// contributed an entitlement, else the project's own
    /// `macos/app.entitlements` when it ships one. `None` on every non-macOS
    /// target, on an unsigned build, and on a build with no entitlements at
    /// all.
    ///
    /// Carried on the report so a later packaging pass signs with exactly what
    /// the bundle was signed with, rather than re-deriving it by probing the
    /// output path and trusting whatever file happens to sit there (see
    /// [`installer::build_installer`]).
    pub entitlements: Option<PathBuf>,
    /// Everything worth telling a human that is not a failure — see
    /// [`BundleNote`].
    pub notes: Vec<BundleNote>,
}

/// A non-fatal observation about an assembly, returned instead of printed so
/// the CLI and the TUI can each render it their own way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleNote {
    /// `frust.toml` has no `[desktop] icon`, so no icon was generated.
    IconNotConfigured,
    /// `[desktop] icon` names a file that isn't there.
    IconSourceMissing { path: PathBuf },
    /// The icon source exists but the icon pipeline rejected it (too small,
    /// not square, not a PNG, unreadable). The bundle is assembled without an
    /// icon rather than failed.
    IconUnusable { path: PathBuf, reason: String },
    /// The icon source is usable but smaller than the largest size generated
    /// from it, so some sizes are upscaled.
    IconBelowRecommendedSize { size: u32, recommended: u32 },
    /// The project has no `macos/Info.plist`, so a minimal one was generated
    /// from `frust.toml` (a project scaffolded before the desktop templates).
    GeneratedInfoPlist,
    /// The project has no `linux/app.desktop`, so a minimal desktop entry was
    /// generated from `frust.toml`.
    GeneratedDesktopEntry,
    /// No `[macos] signing-identity`, so the `.app` is unsigned — fine for
    /// local use, not for distribution.
    Unsigned,
    /// The `.app` was codesigned with this identity.
    Signed { identity: String },
    /// `[windows] file-version`/`product-version` are set, but the values a
    /// `.exe` carries are compiled in by the project's own
    /// `windows/build.rs` — nothing this pipeline writes can override them.
    WindowsVersionOverridesNotApplied,
    /// The project's own `windows/build.rs` predates the `build/` layout: it
    /// still embeds the legacy `windows/icon.ico` path rather than
    /// `build/desktop/windows/icon.ico` (there is no `frust upgrade` to
    /// regenerate it, docs/CLI_ARCHITECTURE.md's `build macos|windows|linux`
    /// section). Rather than rewrite the project's own file, the icon this
    /// run generated was *also* copied to the legacy path — one release's
    /// compatibility window, mirroring `android_build`'s read fallback for a
    /// project scaffolded before a template change.
    LegacyWindowsIconAlsoWritten,
    /// The project's own `macos/Info.plist` (copied verbatim) declares a
    /// `CFBundleIdentifier` other than the one `frust.toml` resolves to. Not
    /// fatal: the copied plist wins, so the `.app` is simply identified by the
    /// value it names — but a `[macos] signing-identity`, a notarization
    /// profile or a `defaults` domain keyed on the manifest value won't match.
    IdentifierIdentityMismatch { plist: String, manifest: String },
    /// A launcher file the project ships names an icon this run did not write
    /// (`macos/Info.plist`'s `CFBundleIconFile`, `linux/app.desktop`'s
    /// `Icon=`). Raised only when an icon really was generated — with the icon
    /// steps skipped there is already a note saying so.
    IconIdentityMismatch {
        /// The file and key the declared name was read from.
        declared_in: &'static str,
        declared: String,
        generated: String,
    },
    /// The project's own `linux/app.desktop` runs a program other than the
    /// bundled executable. A note rather than a refusal: `Exec=` legitimately
    /// varies (a bare name resolved through `PATH`, an installed absolute
    /// path, trailing `%f`/`%u` placeholders), so only the leading token's file
    /// name is compared and a difference is reported, not judged.
    ExecIdentityMismatch { exec: String, binary: String },
    /// The copied `macos/Info.plist` could not be read back, so its identity
    /// keys were not checked against the manifest. Environmental, never a
    /// build failure.
    PlistUnverified { reason: String },
    /// An installed plugin's desktop contribution was merged into the
    /// assembled bundle (see the [`contributions`] module).
    PluginContribution {
        plugin_id: &'static str,
        /// [`crate::plugin::Contribution::describe`]'s label for the edit.
        description: String,
    },
    /// An installed plugin declares a key the bundle's own launcher file
    /// already carries, so nothing was written — the existing (user-owned)
    /// value wins.
    PluginEntryPresent {
        plugin_id: &'static str,
        key: String,
        /// Which file already had the key, for the message.
        file: &'static str,
    },
    /// Plugins contributed entitlements to a build with no `[macos]
    /// signing-identity`. Entitlements take effect only through a signature,
    /// so none were applied — and none could have been.
    EntitlementsSkippedUnsigned { count: usize },
}

impl fmt::Display for BundleNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleNote::IconNotConfigured => write!(
                f,
                "no `[desktop] icon` in frust.toml — bundled without an app icon"
            ),
            BundleNote::IconSourceMissing { path } => write!(
                f,
                "`[desktop] icon` '{}' does not exist — bundled without an app icon",
                path.display()
            ),
            BundleNote::IconUnusable { path, reason } => write!(
                f,
                "`[desktop] icon` '{}' is unusable ({reason}) — bundled without an app icon",
                path.display()
            ),
            BundleNote::IconBelowRecommendedSize { size, recommended } => write!(
                f,
                "icon source is {size}x{size}; {recommended}x{recommended} avoids upscaling"
            ),
            BundleNote::GeneratedInfoPlist => write!(
                f,
                "no `macos/Info.plist` in the project — generated a minimal one from frust.toml"
            ),
            BundleNote::GeneratedDesktopEntry => write!(
                f,
                "no `linux/app.desktop` in the project — generated a minimal one from frust.toml"
            ),
            BundleNote::Unsigned => write!(
                f,
                "no `[macos] signing-identity` in frust.toml — the .app is unsigned"
            ),
            BundleNote::Signed { identity } => write!(f, "codesigned with '{identity}'"),
            BundleNote::WindowsVersionOverridesNotApplied => write!(
                f,
                "`[windows] file-version`/`product-version` are compiled in by the project's own \
                 windows/build.rs — edit that file to change them"
            ),
            BundleNote::LegacyWindowsIconAlsoWritten => write!(
                f,
                "windows/build.rs predates the build/ layout; icon also written to \
                 windows/icon.ico — see docs/DEVELOPMENT.md \"Migrating an already-scaffolded \
                 app to the build/ layout\""
            ),
            BundleNote::IdentifierIdentityMismatch { plist, manifest } => write!(
                f,
                "`macos/Info.plist` declares CFBundleIdentifier '{plist}', but frust.toml \
                 resolves '{manifest}' — the project's own plist is copied verbatim and wins; \
                 the manifest value names generated files only"
            ),
            BundleNote::IconIdentityMismatch {
                declared_in,
                declared,
                generated,
            } => write!(
                f,
                "{declared_in} names '{declared}', but this build generated '{generated}' — \
                 the bundled icon will not be found under the name the project declares"
            ),
            BundleNote::ExecIdentityMismatch { exec, binary } => write!(
                f,
                "`linux/app.desktop` has `Exec={exec}`, but this bundle's executable is \
                 '{binary}' — the entry only launches if '{exec}' resolves on PATH"
            ),
            BundleNote::PlistUnverified { reason } => write!(
                f,
                "could not read the copied `macos/Info.plist` back ({reason}) — its \
                 CFBundleExecutable/CFBundleIdentifier were not checked against frust.toml"
            ),
            BundleNote::PluginContribution {
                plugin_id,
                description,
            } => write!(f, "plugin `{plugin_id}` contributed {description}"),
            BundleNote::PluginEntryPresent {
                plugin_id,
                key,
                file,
            } => write!(
                f,
                "plugin `{plugin_id}` declares `{key}`, but {file} already carries that key — \
                 the existing value wins and nothing was written"
            ),
            BundleNote::EntitlementsSkippedUnsigned { count } => write!(
                f,
                "{count} plugin entitlement(s) not applied — without a `[macos] \
                 signing-identity` there is no signature for an entitlement to travel in"
            ),
        }
    }
}

/// Everything that can stop an assembly — a library-contract enum callers
/// match on (`docs/CODE_STANDARDS.md`'s Error Handling convention), not an
/// `anyhow` chain, since the CLI/TUI both branch on the host-lock case.
#[derive(Debug, thiserror::Error)]
pub enum DesktopBuildError {
    #[error(
        "a {target} bundle can only be built on {target} (this host is {}) — \
         desktop targets are host-locked",
        host.map(|h| h.as_str()).unwrap_or(std::env::consts::OS)
    )]
    HostMismatch {
        target: DesktopBundleTarget,
        host: Option<DesktopBundleTarget>,
    },
    #[error("reading `frust.toml` in '{project_dir}': {reason}")]
    Manifest {
        project_dir: PathBuf,
        reason: String,
    },
    #[error(
        "`{field}` is '{value}', which cannot be used as a file name — this value \
         becomes a directory or file under `build/desktop/`, so it must be a plain name: no \
         `/` or `\\`, no `..`, no leading `/`, not empty, no control characters. \
         Rename it in the manifest; it is refused rather than silently rewritten"
    )]
    UnsafeDesktopIdentity { field: &'static str, value: String },
    #[error(
        "`[desktop] icon` '{icon}' {reason} — the icon path is resolved relative to \
         the project directory and must stay inside it"
    )]
    UnsafeIconPath { icon: String, reason: &'static str },
    #[error(
        "refusing to prepare the bundle directory '{path}': it is not inside the \
         project's own '{output_root}' output directory — preparing a bundle directory \
         deletes it recursively first, so a target outside `build/desktop/` is never touched"
    )]
    UnsafeBundleDir { path: PathBuf, output_root: PathBuf },
    #[error(
        "the project's own `macos/Info.plist` names `CFBundleExecutable` '{found}', but this \
         build put '{expected}' in `Contents/MacOS` — the .app would not launch. That plist is \
         copied verbatim (a hand edit always wins), so fix the key there, or rename the binary \
         to match it"
    )]
    ExecutableIdentityMismatch { expected: String, found: String },
    #[error("spawning `cargo {args}`: {reason}")]
    CargoSpawn { args: String, reason: String },
    #[error("`cargo {args}` failed:\n{tail}")]
    CargoFailed { args: String, tail: String },
    #[error(
        "`cargo build` succeeded but no `{}` binary is at '{}' — \
         check the app crate really builds a binary target named `{binary}`",
        profile,
        path.display()
    )]
    BinaryNotFound {
        binary: String,
        profile: &'static str,
        path: PathBuf,
    },
    #[error("{action} '{path}': {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("reading the desktop contributions the project's installed plugins declare: {reason}")]
    PluginContributions { reason: String },
    #[error(
        "plugin `{plugin_id}` contributes {description}, and it cannot be applied: {reason}. \
         A bundle that silently ships without a declared key fails in front of a user at \
         runtime, so this refuses the build instead of skipping the contribution"
    )]
    ContributionUnappliable {
        plugin_id: &'static str,
        description: String,
        reason: String,
    },
    #[error("spawning `codesign`: {reason}")]
    CodesignSpawn { reason: String },
    #[error("`codesign --sign {identity}` failed:\n{tail}")]
    CodesignFailed { identity: String, tail: String },
}

/// Assembles the `target` desktop bundle for the Frust project at
/// `project_dir`, compiling it with `info`'s mode first.
///
/// `info` is taken as given — the release default belongs to the caller
/// (`frust build <os>` defaults to `--release`, exactly like the mobile
/// pipelines).
///
/// **Print-free**: compile/codesign output and progress lines go through
/// `on_line`; non-fatal observations come back as [`BundleReport::notes`].
pub fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: DesktopBundleTarget,
    on_line: &mut dyn FnMut(&str),
) -> Result<BundleReport, DesktopBuildError> {
    build_with_env(runner, &RealEnv, project_dir, info, target, on_line)
}

/// [`build`], with `env` injected so `CARGO_TARGET_DIR` resolution (the
/// `cargo` submodule's `resolve_target_dir`, reached through
/// `locate_binary`) can be driven without touching the real process
/// environment.
///
/// `pub` — unlike the crate's other `*_with_env` seams (`web_build`,
/// `android_build`, `android_run`, `ios_build`), which stay private because
/// only this crate's own tests need them — because `frust-cli`'s
/// `commands::build` test module needs to exercise the desktop lane
/// hermetically too (a test asserting on a fixture binary under a tempdir
/// must not have the assertion depend on whether `CARGO_TARGET_DIR` happens
/// to be set in the process running the test).
pub fn build_with_env(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    target: DesktopBundleTarget,
    on_line: &mut dyn FnMut(&str),
) -> Result<BundleReport, DesktopBuildError> {
    build_with_host(
        runner,
        env,
        project_dir,
        info,
        target,
        DesktopBundleTarget::host(),
        on_line,
    )
}

/// The testable core of [`build_with_env`]: `host` is injected (rather than read from
/// `cfg!`/[`DesktopBundleTarget::host`]) so all three assemblies can be
/// exercised from one machine, and `env` is injected so `CARGO_TARGET_DIR`
/// resolution can be driven without touching the real process environment.
fn build_with_host(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    target: DesktopBundleTarget,
    host: Option<DesktopBundleTarget>,
    on_line: &mut dyn FnMut(&str),
) -> Result<BundleReport, DesktopBuildError> {
    build_with_contributions(runner, env, project_dir, info, target, host, None, on_line)
}

/// [`build_with_host`]'s core, with the plugin desktop-contribution set
/// injectable: `None` collects it from the project's installed plugins (the
/// production path), `Some(rows)` uses exactly those rows instead.
///
/// The injection exists because the contribution registry is `&'static` data
/// scanned out of the project's `Cargo.toml`, and the real registry carries no
/// desktop rows at all yet — an end-to-end test could otherwise only ever
/// observe the empty set, leaving the merge stage's placement in this pipeline
/// (after assembly, before `codesign`) unprovable.
#[allow(clippy::too_many_arguments)] // one injected seam past the threshold; every argument is a distinct injected dependency
fn build_with_contributions(
    runner: &dyn ProcessRunner,
    env: &dyn EnvLookup,
    project_dir: &Path,
    info: &BuildInfo,
    target: DesktopBundleTarget,
    host: Option<DesktopBundleTarget>,
    contributions: Option<&[DesktopContribution]>,
    on_line: &mut dyn FnMut(&str),
) -> Result<BundleReport, DesktopBuildError> {
    if host != Some(target) {
        return Err(DesktopBuildError::HostMismatch { target, host });
    }

    let manifest = manifest::load(project_dir).map_err(|err| DesktopBuildError::Manifest {
        project_dir: project_dir.to_path_buf(),
        reason: format!("{err:#}"),
    })?;
    // Path-checked as it is resolved: a `[desktop] name`/`identifier`, an
    // `[app] name` or a `[package] name` that would step outside
    // `build/desktop/` refuses here, before the first directory is created
    // (let alone removed).
    let config = DesktopConfig::resolve(project_dir, &manifest)?;

    let mut notes = Vec::new();
    // Windows only: the project's own `windows/build.rs` embeds
    // `build/desktop/windows/icon.ico` into the `.exe`, so the icon has to
    // exist BEFORE the compile — every other target's icon work happens at
    // assembly time. `generate_exe_icon` writes straight into
    // `build/desktop/windows` without clearing it first: that directory may
    // still hold a previous, still-good bundle, and a compile failure below
    // must leave it intact rather than emptied (the regression PROBLEM 2
    // fixes — `prepare_dir` used to run here, before the compile, so a
    // failing compile always lost the previous bundle). The directory is
    // only cleared once the compile that reads this icon has actually
    // succeeded — see the `bundle::prepare_dir_keeping` call beside
    // `windows::assemble` below, which clears stale content while keeping
    // the icon this run just generated (and the compile just read).
    let prebuilt = match target {
        DesktopBundleTarget::Windows => {
            windows::generate_exe_icon(project_dir, &config, &mut notes)
        }
        _ => None,
    };

    cargo::build(runner, project_dir, info, on_line)?;
    let binary = cargo::locate_binary(runner, env, project_dir, info, target, &config)?;

    // Which desktop contributions the installed plugins owe this target, read
    // from the same `Cargo.toml` the compile above just parsed — so a manifest
    // problem is reported by cargo, with cargo's own diagnostics, rather than
    // by this scan.
    let collected;
    let contributions = match contributions {
        Some(rows) => rows,
        None => {
            collected = contributions::collect(project_dir, target)?;
            &collected
        }
    };

    let mut report = match target {
        DesktopBundleTarget::Macos => {
            macos::assemble(runner, project_dir, info, &config, &binary, &mut notes)?
        }
        DesktopBundleTarget::Windows => {
            // The compile above already succeeded (its `?` would have
            // returned otherwise) and has read this run's icon, so it is now
            // safe to clear whatever the directory held from a previous run.
            // `prepare_dir_keeping` preserves exactly the icon
            // `generate_exe_icon` wrote, so `assemble` still finds it to
            // report as an artifact.
            let windows_root = DesktopBundleTarget::Windows.output_dir(project_dir);
            let keep: Vec<&Path> = prebuilt.iter().map(PathBuf::as_path).collect();
            bundle::prepare_dir_keeping(&windows_root, project_dir, &keep)?;
            windows::assemble(project_dir, &config, &binary, prebuilt, &mut notes)?
        }
        DesktopBundleTarget::Linux => linux::assemble(project_dir, &config, &binary, &mut notes)?,
    };

    // The plugin merge runs over the assembled bundle and **before** the
    // signature: `codesign` covers the bundle's files as they are when it
    // runs, so a plist merged afterwards would invalidate it. It also answers
    // which entitlements file the signature should carry.
    let entitlements = contributions::apply(
        project_dir,
        &config,
        &mut report,
        contributions,
        &mut notes,
        on_line,
    )?;

    if target == DesktopBundleTarget::Macos {
        macos::codesign(
            runner,
            project_dir,
            &config,
            &report.root,
            entitlements.as_deref(),
            &mut notes,
            on_line,
        )?;
    }

    // Notes are collected across every stage (pre-build icon, assembly,
    // codesign), so the assemblers leave the field empty and it is filled once
    // here — the report is only complete after the last stage has run. The
    // resolved entitlements follow the same fill-once-at-the-end contract: an
    // assembler cannot know them (the merge that decides them runs after it),
    // so it leaves `None` and this is the single site that answers.
    report.entitlements = entitlements;
    report.notes = notes;
    on_line(&format!("bundle: {}", report.root.display()));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::doctor::FakeEnv;
    use crate::plugin::Contribution;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A project fixture: a temp directory holding `frust.toml`, optional
    /// desktop template files, and a planted "compiled" binary where
    /// `cargo::locate_binary` will look for it.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "frust-drive-desktop-build-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Fixture { dir }
        }

        fn manifest(self, body: &str) -> Fixture {
            fs::write(self.dir.join("frust.toml"), body).unwrap();
            self
        }

        /// The default manifest every desktop section is configured in.
        fn default_manifest(self) -> Fixture {
            self.manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\
                 icon = \"assets/icon.png\"\n",
            )
        }

        fn file(self, rel: &str, body: &str) -> Fixture {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
            self
        }

        /// A real, square 1024px PNG at `assets/icon.png` — the icon pipeline
        /// runs for real over it, no fixture bytes committed.
        fn icon(self, size: u32) -> Fixture {
            let img = image::RgbaImage::from_fn(size, size, |x, y| {
                image::Rgba([(x % 256) as u8, (y % 256) as u8, 200, 255])
            });
            let path = self.dir.join("assets").join("icon.png");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            img.save_with_format(&path, image::ImageFormat::Png)
                .unwrap();
            self
        }

        /// Plants the binary a successful `cargo build` would have produced,
        /// at the default (`<project>/target/<profile>/`) location.
        fn binary(self, name: &str) -> Fixture {
            let dir = self.dir.join("target").join("release");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(name), b"#!/bin/sh\ntrue\n").unwrap();
            self
        }

        /// Join `rel` onto the fixture root **one component at a time**
        /// (`rel` is always written `/`-separated, even in a test's own
        /// literal) rather than via a single `self.dir.join(rel)` push.
        /// `PathBuf::join`/`push` never re-splits the string it is given, so
        /// a single push of a multi-segment literal leaves those embedded
        /// `/`s verbatim inside an otherwise backslash-separated path on
        /// Windows — harmless for opening the file (Windows accepts either
        /// separator), but it breaks byte-for-byte comparison against a path
        /// the product code built one `.join()` call per component (e.g.
        /// [`super::macos::codesign`]'s `--entitlements`/app-bundle
        /// arguments, matched against a [`FakeProcessRunner`] registration
        /// key by exact string). Splitting here keeps every fixture path
        /// separator-identical to the product's own.
        fn path(&self, rel: &str) -> PathBuf {
            let mut path = self.dir.clone();
            for component in rel.split('/') {
                path.push(component);
            }
            path
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.path(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn release_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..BuildArgs::default()
            },
            BuildMode::Release,
        )
        .unwrap()
    }

    /// The exact release invocation this pipeline makes for a project whose
    /// `Cargo.toml` can't be read (the fixtures') — `resolve_release_features`
    /// fails open and keeps `lean`.
    const RELEASE_BUILD: &str = "cargo build --release --features lean";

    fn cargo_ok() -> FakeProcessRunner {
        FakeProcessRunner::new().with(
            RELEASE_BUILD,
            Output {
                success: true,
                stdout: "    Finished `release` profile [optimized]".to_string(),
                stderr: String::new(),
            },
        )
    }

    fn run(
        runner: &FakeProcessRunner,
        fixture: &Fixture,
        target: DesktopBundleTarget,
    ) -> Result<BundleReport, DesktopBuildError> {
        build_with_host(
            runner,
            &FakeEnv::new(),
            &fixture.dir,
            &release_info(),
            target,
            Some(target),
            &mut |_| {},
        )
    }

    /// [`run`] with an injected plugin desktop-contribution set — the seam
    /// [`build_with_contributions`]'s doc explains. The real registry carries
    /// no desktop rows, so this is the only way an end-to-end build can meet
    /// one.
    fn run_with(
        runner: &FakeProcessRunner,
        fixture: &Fixture,
        target: DesktopBundleTarget,
        rows: &[DesktopContribution],
    ) -> Result<BundleReport, DesktopBuildError> {
        build_with_contributions(
            runner,
            &FakeEnv::new(),
            &fixture.dir,
            &release_info(),
            target,
            Some(target),
            Some(rows),
            &mut |_| {},
        )
    }

    #[test]
    fn host_returns_this_machines_target_and_only_that_one_is_supported() {
        let host = DesktopBundleTarget::host();
        assert_eq!(host.map(|t| t.as_str()), Some(std::env::consts::OS));
        for target in [
            DesktopBundleTarget::Macos,
            DesktopBundleTarget::Windows,
            DesktopBundleTarget::Linux,
        ] {
            assert_eq!(target.supported_on_host(), host == Some(target));
        }
    }

    /// The host lock refuses before anything is spawned: the fake runner has
    /// no registrations at all, so a `cargo build` attempt would itself error.
    #[test]
    fn a_foreign_target_is_refused_before_cargo_runs() {
        let fixture = Fixture::new("host-lock").default_manifest();
        let runner = FakeProcessRunner::new();
        let err = build_with_host(
            &runner,
            &FakeEnv::new(),
            &fixture.dir,
            &release_info(),
            DesktopBundleTarget::Macos,
            Some(DesktopBundleTarget::Linux),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                DesktopBuildError::HostMismatch {
                    target: DesktopBundleTarget::Macos,
                    host: Some(DesktopBundleTarget::Linux)
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("host-locked"), "{err}");
    }

    /// The traversal shapes every identity field is driven through below.
    /// Written the way they would appear in a hand-edited `frust.toml` — a
    /// crafted one, or a typo'd relative path someone meant as a hint.
    const TRAVERSALS: &[&str] = &["../../etc", "/etc/passwd", "..\\..\\windows", "sub/dir"];

    /// Every field that becomes a path refuses a traversal, on every target,
    /// **before the compile** — proven by the empty fake runner, for which any
    /// invocation at all would report `CargoSpawn` instead. Refusing that
    /// early is what keeps the value away from `prepare_dir`'s `remove_dir_all`.
    #[test]
    fn every_desktop_identity_field_refuses_a_traversal_before_the_compile() {
        for value in TRAVERSALS {
            let escaped = value.replace('\\', "\\\\");
            let manifests = [
                format!(
                    "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [desktop]\nname = \"{escaped}\"\n"
                ),
                format!(
                    "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [desktop]\nidentifier = \"{escaped}\"\n"
                ),
                format!("[app]\nname = \"{escaped}\"\norg = \"dev.f0x\"\n"),
            ];
            for manifest in &manifests {
                for target in [
                    DesktopBundleTarget::Macos,
                    DesktopBundleTarget::Windows,
                    DesktopBundleTarget::Linux,
                ] {
                    let fixture = Fixture::new("traversal").manifest(manifest);
                    let err = run(&FakeProcessRunner::new(), &fixture, target).unwrap_err();
                    assert!(
                        matches!(err, DesktopBuildError::UnsafeDesktopIdentity { .. }),
                        "{target} {value:?}: {err}"
                    );
                    assert!(
                        !fixture.path("build/desktop").exists(),
                        "{target} {value:?}: wrote into build/desktop/ anyway"
                    );
                }
            }
        }
    }

    /// `Cargo.toml`'s `[package] name` is the same hazard by another route —
    /// it names the Linux bundle *directory* — and is refused the same way.
    #[test]
    fn a_traversal_in_the_cargo_package_name_refuses_the_build() {
        for value in TRAVERSALS {
            let fixture = Fixture::new("traversal-package").default_manifest().file(
                "Cargo.toml",
                &format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
                    value.replace('\\', "\\\\")
                ),
            );
            let err = run(
                &FakeProcessRunner::new(),
                &fixture,
                DesktopBundleTarget::Linux,
            )
            .unwrap_err();
            assert!(
                matches!(err, DesktopBuildError::UnsafeDesktopIdentity { .. }),
                "{value:?}: {err}"
            );
            assert!(!fixture.path("build/desktop").exists(), "{value:?}");
        }
    }

    /// The defect in its concrete form: a display name climbing out of
    /// `build/desktop/macos` puts the `.app` — and the `remove_dir_all` that
    /// prepares it — on top of an existing directory outside the build
    /// output. The canary sits at exactly the path the unguarded code would
    /// have deleted.
    #[test]
    fn a_display_name_climbing_out_of_build_desktop_never_reaches_remove_dir_all() {
        let fixture = Fixture::new("escape-canary")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"../../victim\"\n",
            )
            .binary("my_app");
        // `<project>/build/desktop/macos/../../victim.app` is `<project>/victim.app`.
        let victim = fixture.path("victim.app");
        fs::create_dir_all(victim.join("nested")).unwrap();
        fs::write(victim.join("nested/keep.txt"), b"precious").unwrap();

        let err = run(&cargo_ok(), &fixture, DesktopBundleTarget::Macos).unwrap_err();
        assert!(
            matches!(
                err,
                DesktopBuildError::UnsafeDesktopIdentity {
                    field: "[desktop] name",
                    ..
                }
            ),
            "{err}"
        );
        assert!(
            victim.join("nested/keep.txt").is_file(),
            "the bundle assembly deleted a directory outside build/desktop/"
        );
    }

    /// Where the icon rules meet the path rules: an icon the pipeline cannot
    /// *use* is a note (see the tests above), but an icon path that tries to
    /// leave the project is a refusal. Quality degrades a bundle; safety does
    /// not degrade.
    #[test]
    fn an_icon_path_escaping_the_project_fails_the_build_instead_of_becoming_a_note() {
        for value in ["../../../etc/passwd", "/etc/passwd", "..\\..\\secrets.png"] {
            let fixture = Fixture::new("icon-escape")
                .manifest(&format!(
                    "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [desktop]\nicon = \"{}\"\n",
                    value.replace('\\', "\\\\")
                ))
                .binary("my_app");
            let err = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap_err();
            assert!(
                matches!(err, DesktopBuildError::UnsafeIconPath { .. }),
                "{value:?}: {err}"
            );
            assert!(!fixture.path("build/desktop").exists(), "{value:?}");
        }
    }

    #[test]
    fn linux_assembly_produces_binary_desktop_entry_and_hicolor_tree() {
        let fixture = Fixture::new("linux-full")
            .default_manifest()
            .icon(1024)
            .binary("my_app")
            .file(
                "linux/app.desktop",
                "[Desktop Entry]\nType=Application\nName=My App\nExec=my_app\n\
                 Icon=dev.f0x.my_app\nCategories=Utility;\nTerminal=false\n",
            );

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();

        assert_eq!(report.root, fixture.path("build/desktop/linux/my_app"));
        assert_eq!(
            report.executable,
            fixture.path("build/desktop/linux/my_app/my_app")
        );
        assert!(report.executable.is_file());

        // The project's own entry, copied verbatim under the identifier name
        // a `.desktop` install expects.
        let entry = fixture.read("build/desktop/linux/my_app/dev.f0x.my_app.desktop");
        assert!(entry.contains("Name=My App"), "{entry}");
        assert!(entry.contains("Exec=my_app"), "{entry}");
        assert!(
            !report.notes.contains(&BundleNote::GeneratedDesktopEntry),
            "{:?}",
            report.notes
        );

        // Icons: the hicolor tree, each leaf named after the identifier so the
        // entry's `Icon=` key resolves.
        for size in [16, 32, 48, 64, 128, 256, 512] {
            let icon = fixture.path(&format!(
                "build/desktop/linux/my_app/share/icons/hicolor/{size}x{size}/apps/dev.f0x.my_app.png"
            ));
            assert!(icon.is_file(), "missing {}", icon.display());
        }
        assert!(
            report
                .artifacts
                .iter()
                .any(|p| p.ends_with("512x512/apps/dev.f0x.my_app.png")),
            "{:?}",
            report.artifacts
        );
    }

    #[test]
    fn linux_generates_a_desktop_entry_when_the_project_predates_the_template() {
        let fixture = Fixture::new("linux-fallback")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [linux]\ncategories = [\"Graphics\", \"Utility\"]\n",
            )
            .binary("my_app");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();

        assert!(report.notes.contains(&BundleNote::GeneratedDesktopEntry));
        // Identifier and display name both fall back: `[app]` name/org, via
        // the same derivation the scaffold uses.
        let entry = fixture.read("build/desktop/linux/my_app/dev.f0x.my_app.desktop");
        assert!(entry.contains("Name=my_app"), "{entry}");
        assert!(entry.contains("Exec=my_app"), "{entry}");
        assert!(entry.contains("Icon=dev.f0x.my_app"), "{entry}");
        assert!(entry.contains("Categories=Graphics;Utility;"), "{entry}");
        assert!(report.notes.contains(&BundleNote::IconNotConfigured));
    }

    /// The plist a project of its own ships, in the shape the scaffolded
    /// template renders it — the file this pipeline copies verbatim.
    fn project_plist(executable: &str, identifier: &str, icon_file: &str) -> String {
        format!(
            "<plist version=\"1.0\"><dict>\n\
             <key>CFBundleExecutable</key><string>{executable}</string>\n\
             <key>CFBundleIdentifier</key><string>{identifier}</string>\n\
             <key>CFBundleIconFile</key><string>{icon_file}</string>\n\
             </dict></plist>",
        )
    }

    /// Registers a `plutil -extract <key> raw …` answer. Keyed on the prefix
    /// (the fake runner falls back to the longest registered prefix), so a
    /// fixture's temp path doesn't have to be spelled out.
    fn with_plutil(runner: FakeProcessRunner, key: &str, value: &str) -> FakeProcessRunner {
        runner.with(
            format!("plutil -extract {key} raw "),
            Output {
                success: true,
                stdout: format!("{value}\n"),
                stderr: String::new(),
            },
        )
    }

    #[test]
    fn macos_assembly_copies_the_projects_info_plist_and_names_the_app_bundle() {
        let fixture = Fixture::new("macos-full")
            .default_manifest()
            .icon(1024)
            .binary("my_app")
            .file(
                "macos/Info.plist",
                &project_plist("my_app", "dev.f0x.my_app", "my_app"),
            );
        // The plist agrees with the manifest, so the read-back records
        // nothing — the negative half of the reconciliation tests below.
        let runner = with_plutil(cargo_ok(), "CFBundleExecutable", "my_app");
        let runner = with_plutil(runner, "CFBundleIdentifier", "dev.f0x.my_app");
        let runner = with_plutil(runner, "CFBundleIconFile", "my_app");

        let report = run(&runner, &fixture, DesktopBundleTarget::Macos).unwrap();

        assert_eq!(report.root, fixture.path("build/desktop/macos/My App.app"));
        assert_eq!(
            report.executable,
            fixture.path("build/desktop/macos/My App.app/Contents/MacOS/my_app")
        );
        assert!(report.executable.is_file());
        assert!(
            fixture
                .path("build/desktop/macos/My App.app/Contents/Resources/my_app.icns")
                .is_file()
        );
        let plist = fixture.read("build/desktop/macos/My App.app/Contents/Info.plist");
        assert!(plist.contains("CFBundleExecutable"), "{plist}");
        assert!(!report.notes.contains(&BundleNote::GeneratedInfoPlist));
        assert!(report.notes.contains(&BundleNote::Unsigned));
        assert!(
            !report.notes.iter().any(|note| matches!(
                note,
                BundleNote::IdentifierIdentityMismatch { .. }
                    | BundleNote::IconIdentityMismatch { .. }
                    | BundleNote::PlistUnverified { .. }
            )),
            "{:?}",
            report.notes
        );
    }

    /// The defect, end to end: the project's own plist (copied verbatim, as it
    /// must be) names an executable this build didn't produce, so the `.app`
    /// would not launch. The one identity disagreement that fails a build.
    #[test]
    fn a_copied_info_plist_naming_another_executable_fails_the_build() {
        let fixture = Fixture::new("macos-exec-drift")
            .default_manifest()
            .binary("my_app")
            .file(
                "macos/Info.plist",
                &project_plist("renamed_app", "dev.f0x.my_app", "my_app"),
            );
        let runner = with_plutil(cargo_ok(), "CFBundleExecutable", "renamed_app");

        let err = run(&runner, &fixture, DesktopBundleTarget::Macos).unwrap_err();
        assert!(
            matches!(
                &err,
                DesktopBuildError::ExecutableIdentityMismatch { expected, found }
                    if expected == "my_app" && found == "renamed_app"
            ),
            "{err}"
        );
    }

    /// …while identifier and icon-name drift come back as notes on a bundle
    /// that was still assembled.
    #[test]
    fn a_copied_info_plists_identifier_and_icon_drift_are_notes() {
        let fixture = Fixture::new("macos-id-drift")
            .default_manifest()
            .icon(1024)
            .binary("my_app")
            .file(
                "macos/Info.plist",
                &project_plist("my_app", "com.example.other", "AppIcon"),
            );
        let runner = with_plutil(cargo_ok(), "CFBundleExecutable", "my_app");
        let runner = with_plutil(runner, "CFBundleIdentifier", "com.example.other");
        let runner = with_plutil(runner, "CFBundleIconFile", "AppIcon");

        let report = run(&runner, &fixture, DesktopBundleTarget::Macos).unwrap();
        assert!(
            report
                .notes
                .contains(&BundleNote::IdentifierIdentityMismatch {
                    plist: "com.example.other".to_string(),
                    manifest: "dev.f0x.my_app".to_string(),
                }),
            "{:?}",
            report.notes
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| matches!(note, BundleNote::IconIdentityMismatch { .. })),
            "{:?}",
            report.notes
        );
        assert!(report.executable.is_file());
    }

    /// Tool trouble never blocks a build: with no `plutil` to read the copied
    /// plist back (the fake runner registers none), the bundle is assembled
    /// and the unchecked identity is a single note.
    #[test]
    fn a_copied_info_plist_that_cannot_be_read_back_is_a_note_not_a_failure() {
        let fixture = Fixture::new("macos-unverified")
            .default_manifest()
            .binary("my_app")
            .file(
                "macos/Info.plist",
                &project_plist("my_app", "dev.f0x.my_app", "my_app"),
            );

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Macos).unwrap();
        assert_eq!(
            report
                .notes
                .iter()
                .filter(|note| matches!(note, BundleNote::PlistUnverified { .. }))
                .count(),
            1,
            "{:?}",
            report.notes
        );
        assert!(report.executable.is_file());
    }

    /// A generated plist is written from the manifest by construction, so
    /// there is nothing to reconcile and `plutil` is never run — proven by the
    /// fake runner having no registration for it, which would otherwise
    /// surface as a `PlistUnverified` note.
    #[test]
    fn a_generated_info_plist_is_never_read_back() {
        let fixture = Fixture::new("macos-generated-skip")
            .default_manifest()
            .binary("my_app");
        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Macos).unwrap();
        assert!(report.notes.contains(&BundleNote::GeneratedInfoPlist));
        assert!(
            !report
                .notes
                .iter()
                .any(|note| matches!(note, BundleNote::PlistUnverified { .. })),
            "{:?}",
            report.notes
        );
    }

    /// The Linux half of the same defect — and the difference in verdict: a
    /// `.desktop` `Exec=` naming another program is a note, since it may
    /// legitimately resolve through `PATH`.
    #[test]
    fn a_copied_desktop_entry_naming_another_program_is_a_note_not_a_failure() {
        let fixture = Fixture::new("linux-exec-drift")
            .default_manifest()
            .icon(1024)
            .binary("my_app")
            .file(
                "linux/app.desktop",
                "[Desktop Entry]\nType=Application\nName=My App\nExec=renamed_app %f\n\
                 Icon=com.example.other\nCategories=Utility;\nTerminal=false\n",
            );

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(
            report.notes.contains(&BundleNote::ExecIdentityMismatch {
                exec: "renamed_app %f".to_string(),
                binary: "my_app".to_string(),
            }),
            "{:?}",
            report.notes
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| matches!(note, BundleNote::IconIdentityMismatch { .. })),
            "{:?}",
            report.notes
        );
        assert!(report.executable.is_file());
    }

    /// A generated desktop entry is written from the manifest by
    /// construction — nothing to disagree with.
    #[test]
    fn a_generated_desktop_entry_is_never_reconciled() {
        let fixture = Fixture::new("linux-generated-skip")
            .default_manifest()
            .icon(1024)
            .binary("my_app");
        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(report.notes.contains(&BundleNote::GeneratedDesktopEntry));
        assert!(
            !report.notes.iter().any(|note| matches!(
                note,
                BundleNote::ExecIdentityMismatch { .. } | BundleNote::IconIdentityMismatch { .. }
            )),
            "{:?}",
            report.notes
        );
    }

    #[test]
    fn macos_generates_an_info_plist_when_the_project_predates_the_template() {
        let fixture = Fixture::new("macos-fallback")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\n\n[macos]\nminimum-system-version = \"13.0\"\n",
            )
            .binary("my_app");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Macos).unwrap();

        assert!(report.notes.contains(&BundleNote::GeneratedInfoPlist));
        let plist = fixture.read("build/desktop/macos/My App.app/Contents/Info.plist");
        for expected in [
            "<key>CFBundleName</key>\n\t<string>My App</string>",
            "<key>CFBundleExecutable</key>\n\t<string>my_app</string>",
            "<key>CFBundleIdentifier</key>\n\t<string>dev.f0x.my_app</string>",
            "<key>CFBundleIconFile</key>\n\t<string>my_app</string>",
            "<key>LSMinimumSystemVersion</key>\n\t<string>13.0</string>",
        ] {
            assert!(plist.contains(expected), "missing {expected} in\n{plist}");
        }
    }

    #[test]
    fn macos_codesigns_exactly_when_a_signing_identity_is_configured() {
        let fixture = Fixture::new("macos-signed")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\n\n\
                 [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
            )
            .binary("my_app")
            .file("macos/app.entitlements", "<plist><dict/></plist>");

        let app = fixture.path("build/desktop/macos/My App.app");
        // Built as `fixture.dir.join("macos/app.entitlements")` — one push of
        // the same compound literal `contributions::apply`'s own
        // `project_dir.join(PROJECT_ENTITLEMENTS_REL)` uses to resolve the
        // project's own entitlements file — rather than through
        // `Fixture::path`'s per-component join. `PathBuf::push` only ever
        // inserts a separator at the *join boundary*; it never rewrites a
        // `/` already embedded in the pushed argument itself, so a
        // two-segment literal pushed as one argument keeps that inner `/`
        // even on Windows. `Fixture::path`'s per-component splitting matches
        // how `build_dirs::BuildLayout`'s own accessors (e.g. `app`, above)
        // build a path — a different, unrelated convention this one
        // literal does not follow.
        let entitlements = fixture.dir.join("macos/app.entitlements");
        // A Developer ID identity is Apple-issued: both `--options runtime`
        // (Hardened Runtime, always) and `--timestamp` (a secure timestamp,
        // Apple-issued identities only) are present, in this exact order.
        let runner = cargo_ok().with(
            format!(
                "codesign --force --sign Developer ID Application: Example --options runtime --entitlements {} --timestamp {}",
                entitlements.display(),
                app.display()
            ),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let report = run(&runner, &fixture, DesktopBundleTarget::Macos).unwrap();
        assert!(report.notes.contains(&BundleNote::Signed {
            identity: "Developer ID Application: Example".to_string()
        }));
    }

    /// The regression case the codesign flag policy exists for: an ad-hoc or
    /// self-signed identity still gets the Hardened Runtime (harmless and
    /// desired for every identity), but never `--timestamp` — a secure
    /// timestamp needs Apple's timestamp service, and forcing it here would
    /// make a local, offline signature depend on the network for no reason
    /// (nothing trusts an ad-hoc signature's chain regardless).
    #[test]
    fn macos_codesign_omits_timestamp_for_a_non_apple_issued_identity() {
        let fixture = Fixture::new("macos-signed-adhoc")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\n\n\
                 [macos]\nsigning-identity = \"My Self Signed\"\n",
            )
            .binary("my_app");

        let app = fixture.path("build/desktop/macos/My App.app");
        let runner = cargo_ok().with(
            format!(
                "codesign --force --sign My Self Signed --options runtime {}",
                app.display()
            ),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let report = run(&runner, &fixture, DesktopBundleTarget::Macos).unwrap();
        assert!(report.notes.contains(&BundleNote::Signed {
            identity: "My Self Signed".to_string()
        }));
    }

    /// The negative half: with no identity configured, `codesign` is never
    /// invoked — proven by the fake runner holding no `codesign` registration
    /// at all, which would make any such call an error.
    #[test]
    fn macos_never_codesigns_without_an_identity() {
        let fixture = Fixture::new("macos-unsigned")
            .default_manifest()
            .binary("my_app");
        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Macos).unwrap();
        assert!(report.notes.contains(&BundleNote::Unsigned));
    }

    /// Codesigning is macOS-only by construction: the two other targets never
    /// reach the codesign step even with an identity configured (again proven
    /// by an unregistered `codesign` being a hard error in the fake runner).
    #[test]
    fn other_targets_never_codesign_even_with_an_identity_configured() {
        for target in [DesktopBundleTarget::Linux, DesktopBundleTarget::Windows] {
            let fixture = Fixture::new(&format!("no-codesign-{target}"))
                .manifest(
                    "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
                )
                .binary(if target == DesktopBundleTarget::Windows {
                    "my_app.exe"
                } else {
                    "my_app"
                });
            let report = run(&cargo_ok(), &fixture, target).unwrap();
            assert!(
                !report
                    .notes
                    .iter()
                    .any(|n| matches!(n, BundleNote::Signed { .. })),
                "{target}: {:?}",
                report.notes
            );
        }
    }

    #[test]
    fn windows_assembly_generates_the_ico_and_copies_the_exe() {
        let fixture = Fixture::new("windows-full")
            .default_manifest()
            .icon(1024)
            .binary("my_app.exe");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();

        assert_eq!(report.root, fixture.path("build/desktop/windows"));
        assert_eq!(
            report.executable,
            fixture.path("build/desktop/windows/my_app.exe")
        );
        assert!(report.executable.is_file());
        // The `.ico` lands in the same output dir the `.exe` does (where
        // `windows/build.rs` reads it from, relative to the project root),
        // and is reported as an artifact all the same.
        let ico = fixture.path("build/desktop/windows/icon.ico");
        assert!(ico.is_file());
        assert!(report.artifacts.contains(&ico), "{:?}", report.artifacts);
    }

    /// Ordering proof: the `.ico` must exist before the compile that embeds
    /// it. With no `cargo build` registered the compile fails — and the icon
    /// is on disk anyway, which can only be true if it was generated first.
    #[test]
    fn windows_icon_is_generated_before_the_compile() {
        let fixture = Fixture::new("windows-icon-order")
            .default_manifest()
            .icon(1024);
        let err = run(
            &FakeProcessRunner::new(),
            &fixture,
            DesktopBundleTarget::Windows,
        )
        .unwrap_err();
        assert!(matches!(err, DesktopBuildError::CargoSpawn { .. }), "{err}");
        assert!(fixture.path("build/desktop/windows/icon.ico").is_file());
    }

    /// The icon and the `.exe` now share one output directory
    /// (`build/desktop/windows`), generated at two different pipeline stages
    /// (icon before the compile, `.exe` after it) — proving the second stage
    /// does not wipe the first stage's file, while a genuinely stale file
    /// from an earlier run still gets cleared **once the compile that
    /// produced this run's replacement has actually succeeded**. PROBLEM 2:
    /// a *failing* compile must leave the previous, still-good bundle
    /// untouched instead — before this fix, `prepare_dir` ran before the
    /// compile and would already have emptied the directory by the time
    /// `cargo build` failed.
    #[test]
    fn windows_reassembly_clears_stale_content_but_keeps_this_runs_icon() {
        let fixture = Fixture::new("windows-rebuild")
            .default_manifest()
            .icon(1024)
            .binary("my_app.exe");
        run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();
        let stale = fixture.path("build/desktop/windows/stale-from-a-previous-run");
        fs::write(&stale, b"x").unwrap();

        // A failing compile must not touch the directory at all — the stale
        // file (standing in for the previous, still-valid bundle) survives,
        // and so does the previous run's own exe.
        let failing = FakeProcessRunner::new().with(
            RELEASE_BUILD,
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error[E0308]: mismatched types".to_string(),
            },
        );
        let err = run(&failing, &fixture, DesktopBundleTarget::Windows).unwrap_err();
        assert!(
            matches!(err, DesktopBuildError::CargoFailed { .. }),
            "{err}"
        );
        assert!(
            stale.exists(),
            "a failing compile emptied the previous bundle before it even ran"
        );
        assert!(fixture.path("build/desktop/windows/my_app.exe").is_file());
        assert!(fixture.path("build/desktop/windows/icon.ico").is_file());

        // A succeeding compile clears the stale content and keeps this run's
        // icon.
        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();
        assert!(!stale.exists());
        assert!(report.executable.is_file());
        assert!(fixture.path("build/desktop/windows/icon.ico").is_file());
    }

    /// PROBLEM 1 fix: a project scaffolded before the `build/` layout still
    /// carries a `windows/build.rs` rendered from the old template, which
    /// embeds the legacy `windows/icon.ico` path verbatim rather than
    /// `build/desktop/windows/icon.ico` — there is no `frust upgrade` to
    /// regenerate it. Without a dual write the `.exe` would silently lose its
    /// icon while the bundle report claims one was generated.
    #[test]
    fn a_legacy_windows_build_rs_also_gets_the_icon_at_the_old_path() {
        let legacy_build_rs = "fn main() {\n    \
             let icon_path = std::path::Path::new(\"windows/icon.ico\");\n}\n";
        let fixture = Fixture::new("windows-legacy-build-rs")
            .default_manifest()
            .icon(1024)
            .binary("my_app.exe")
            .file("windows/build.rs", legacy_build_rs);

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();

        assert!(fixture.path("build/desktop/windows/icon.ico").is_file());
        assert!(
            fixture.path("windows/icon.ico").is_file(),
            "legacy windows/icon.ico was not also written"
        );
        assert!(
            fixture
                .path(crate::build_dirs::LEGACY_WINDOWS_ICON_MARKER)
                .is_file(),
            "the mirror marker `clean` relies on was not written"
        );
        assert!(
            report
                .notes
                .contains(&BundleNote::LegacyWindowsIconAlsoWritten),
            "{:?}",
            report.notes
        );
    }

    /// A symlink parked at the legacy path is never written through (that
    /// would overwrite whatever it points at, outside the pipeline's own
    /// output): no copy, no marker, no note.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_legacy_icon_path_is_never_written_through() {
        let legacy_build_rs = "fn main() {\n    \
             let icon_path = std::path::Path::new(\"windows/icon.ico\");\n}\n";
        let fixture = Fixture::new("windows-legacy-symlink")
            .default_manifest()
            .icon(1024)
            .binary("my_app.exe")
            .file("windows/build.rs", legacy_build_rs)
            .file("elsewhere/target.ico", "user-owned bytes");
        std::os::unix::fs::symlink(
            fixture.path("elsewhere/target.ico"),
            fixture.path("windows/icon.ico"),
        )
        .unwrap();

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();

        assert_eq!(
            std::fs::read_to_string(fixture.path("elsewhere/target.ico")).unwrap(),
            "user-owned bytes"
        );
        assert!(
            !fixture
                .path(crate::build_dirs::LEGACY_WINDOWS_ICON_MARKER)
                .exists()
        );
        assert!(
            !report
                .notes
                .contains(&BundleNote::LegacyWindowsIconAlsoWritten),
            "{:?}",
            report.notes
        );
    }

    /// The inverse: a `windows/build.rs` already naming the new
    /// `build/desktop/windows/icon.ico` path never gets a legacy copy or a
    /// note — it isn't stale.
    #[test]
    fn a_new_windows_build_rs_gets_no_legacy_icon_copy() {
        let new_build_rs = "fn main() {\n    \
             let icon_path = std::path::Path::new(\"build/desktop/windows/icon.ico\");\n}\n";
        let fixture = Fixture::new("windows-new-build-rs")
            .default_manifest()
            .icon(1024)
            .binary("my_app.exe")
            .file("windows/build.rs", new_build_rs);

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();

        assert!(!fixture.path("windows/icon.ico").exists());
        assert!(
            !fixture
                .path(crate::build_dirs::LEGACY_WINDOWS_ICON_MARKER)
                .exists()
        );
        assert!(
            !report
                .notes
                .contains(&BundleNote::LegacyWindowsIconAlsoWritten),
            "{:?}",
            report.notes
        );
    }

    /// A project with no `windows/build.rs` at all (still on defaults, or one
    /// hand-deleted) is likewise left alone.
    #[test]
    fn a_project_with_no_windows_build_rs_gets_no_legacy_icon_copy() {
        let fixture = Fixture::new("windows-no-build-rs")
            .default_manifest()
            .icon(1024)
            .binary("my_app.exe");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();

        assert!(!fixture.path("windows/icon.ico").exists());
        assert!(
            !report
                .notes
                .contains(&BundleNote::LegacyWindowsIconAlsoWritten),
            "{:?}",
            report.notes
        );
    }

    #[test]
    fn windows_version_overrides_report_that_the_projects_build_rs_owns_them() {
        let fixture = Fixture::new("windows-versions")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [windows]\nfile-version = \"2.0.0.0\"\n",
            )
            .binary("my_app.exe");
        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Windows).unwrap();
        assert!(
            report
                .notes
                .contains(&BundleNote::WindowsVersionOverridesNotApplied)
        );
    }

    #[test]
    fn a_missing_icon_configuration_skips_icon_steps_without_failing() {
        let fixture = Fixture::new("no-icon")
            .manifest("[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n")
            .binary("my_app");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(report.notes.contains(&BundleNote::IconNotConfigured));
        assert!(report.executable.is_file());
        assert!(!fixture.path("build/desktop/linux/my_app/share").exists());
    }

    #[test]
    fn a_configured_but_absent_icon_file_is_a_note_not_a_failure() {
        let fixture = Fixture::new("icon-absent")
            .default_manifest()
            .binary("my_app");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(
            matches!(
                report.notes.first(),
                Some(BundleNote::IconSourceMissing { .. })
            ),
            "{:?}",
            report.notes
        );
        assert!(report.executable.is_file());
    }

    /// The scaffold's own placeholder logo is 128px — below the icon
    /// pipeline's 512px floor. A fresh project must still build.
    #[test]
    fn an_unusable_icon_source_is_a_note_not_a_failure() {
        let fixture = Fixture::new("icon-too-small")
            .default_manifest()
            .icon(128)
            .binary("my_app");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(
            matches!(report.notes.first(), Some(BundleNote::IconUnusable { .. })),
            "{:?}",
            report.notes
        );
        assert!(report.executable.is_file());
    }

    #[test]
    fn an_icon_below_the_recommended_size_still_generates_with_a_note() {
        let fixture = Fixture::new("icon-soft")
            .default_manifest()
            .icon(512)
            .binary("my_app");

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(
            report
                .notes
                .contains(&BundleNote::IconBelowRecommendedSize {
                    size: 512,
                    recommended: 1024
                })
        );
        assert!(
            fixture
                .path(
                    "build/desktop/linux/my_app/share/icons/hicolor/512x512/apps/dev.f0x.my_app.png"
                )
                .is_file()
        );
    }

    #[test]
    fn a_failed_cargo_build_surfaces_the_output_tail() {
        let fixture = Fixture::new("cargo-failed").default_manifest();
        let runner = FakeProcessRunner::new().with(
            RELEASE_BUILD,
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error[E0425]: cannot find value `nope` in this scope".to_string(),
            },
        );
        let err = run(&runner, &fixture, DesktopBundleTarget::Linux).unwrap_err();
        assert!(err.to_string().contains("E0425"), "{err}");
    }

    /// A green compile with nothing where the binary should be is a typed
    /// error, not a bundle with a missing executable.
    #[test]
    fn a_missing_binary_after_a_green_build_is_a_typed_error() {
        let fixture = Fixture::new("binary-missing").default_manifest();
        let err = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap_err();
        assert!(
            matches!(err, DesktopBuildError::BinaryNotFound { ref binary, .. } if binary == "my_app"),
            "{err}"
        );
    }

    /// Re-assembling over a previous bundle must not leave the previous run's
    /// files behind (a renamed icon, a dropped entry).
    #[test]
    fn reassembly_replaces_the_previous_bundle_contents() {
        let fixture = Fixture::new("rebuild").default_manifest().binary("my_app");
        run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        let stale = fixture.path("build/desktop/linux/my_app/stale-from-a-previous-run");
        fs::write(&stale, b"x").unwrap();

        run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(!stale.exists());
        assert!(fixture.path("build/desktop/linux/my_app/my_app").is_file());
    }

    /// Progress lines leave through `on_line` — the print-free contract's
    /// caller-visible half.
    #[test]
    fn compile_output_and_progress_reach_the_on_line_sink() {
        let fixture = Fixture::new("on-line").default_manifest().binary("my_app");
        let mut lines = Vec::new();
        build_with_host(
            &cargo_ok(),
            &FakeEnv::new(),
            &fixture.dir,
            &release_info(),
            DesktopBundleTarget::Linux,
            Some(DesktopBundleTarget::Linux),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(lines.iter().any(|l| l.starts_with("[cargo] ")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("bundle: ")), "{lines:?}");
    }

    // -----------------------------------------------------------------------
    // Plugin desktop contributions, end to end through the whole pipeline.
    // The rows are synthetic (`&'static` statics) because the real registry
    // declares no desktop contributions yet — see `build_with_contributions`.
    // -----------------------------------------------------------------------

    static PLIST_CAMERA: Contribution = Contribution::MacosPlistEntry {
        key: "NSCameraUsageDescription",
        value: "Scan a document & <sign> it",
        comment: "Camera access",
    };
    static PLIST_MIC: Contribution = Contribution::MacosPlistEntry {
        key: "NSMicrophoneUsageDescription",
        value: "Record a voice note",
        comment: "Microphone access",
    };
    static ENTITLEMENT_CAMERA: Contribution = Contribution::MacosEntitlement {
        key: "com.apple.security.device.camera",
        comment: "Camera device access",
    };
    static DESKTOP_MIME: Contribution = Contribution::LinuxDesktopEntry {
        key: "MimeType",
        value: "image/png;",
        comment: "Handled file types",
    };
    static DESKTOP_CATEGORIES: Contribution = Contribution::LinuxDesktopEntry {
        key: "Categories",
        value: "Graphics;",
        comment: "Launcher categories",
    };

    fn row(contribution: &'static Contribution) -> DesktopContribution {
        DesktopContribution {
            plugin_id: "camera",
            contribution,
        }
    }

    fn applied_descriptions(report: &BundleReport) -> Vec<&str> {
        report
            .notes
            .iter()
            .filter_map(|note| match note {
                BundleNote::PluginContribution { description, .. } => Some(description.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The milestone case: a signing macOS build over a generated plist takes
    /// both contributed keys into the bundle's own `Info.plist` and signs with
    /// a generated entitlements file carrying the contributed entitlement.
    #[test]
    fn a_signing_macos_build_merges_plist_keys_and_signs_with_generated_entitlements() {
        let fixture = Fixture::new("contrib-macos-signed")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\n\n\
                 [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
            )
            .binary("my_app");
        let entitlements = fixture.path("build/desktop/macos/my_app.entitlements");
        let runner = cargo_ok().with(
            format!(
                "codesign --force --sign Developer ID Application: Example --options runtime \
                 --entitlements {} --timestamp {}",
                entitlements.display(),
                fixture.path("build/desktop/macos/My App.app").display()
            ),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let report = run_with(
            &runner,
            &fixture,
            DesktopBundleTarget::Macos,
            &[
                row(&PLIST_CAMERA),
                row(&PLIST_MIC),
                row(&ENTITLEMENT_CAMERA),
            ],
        )
        .unwrap();

        let plist = fixture.read("build/desktop/macos/My App.app/Contents/Info.plist");
        assert!(
            plist.contains(
                "<key>NSCameraUsageDescription</key>\n\t\
                 <string>Scan a document &amp; &lt;sign&gt; it</string>"
            ),
            "{plist}"
        );
        assert!(
            plist.contains("<key>NSMicrophoneUsageDescription</key>"),
            "{plist}"
        );
        assert!(
            plist.contains("<!-- Camera access (frust plugin: camera) -->"),
            "{plist}"
        );

        // The entitlements are a generated build artifact; the project ships
        // none, so nothing of the project's was read or written.
        let merged = fixture.read("build/desktop/macos/my_app.entitlements");
        assert!(
            merged.contains("<key>com.apple.security.device.camera</key>"),
            "{merged}"
        );
        assert!(!fixture.path("macos/app.entitlements").exists());
        assert!(
            report.artifacts.contains(&entitlements),
            "{:?}",
            report.artifacts
        );
        // The exact file the signature covers, carried on the report so a
        // packaging pass signs with it rather than probing for it.
        assert_eq!(report.entitlements.as_deref(), Some(entitlements.as_path()));

        // The signature is the one registered above — an unregistered argv
        // would have failed the spawn — and every contribution is reported.
        assert!(report.notes.contains(&BundleNote::Signed {
            identity: "Developer ID Application: Example".to_string()
        }));
        assert_eq!(applied_descriptions(&report).len(), 3, "{:?}", report.notes);
    }

    /// Ordering proof: with `codesign` registered as *failing*, the build
    /// stops there — and the merged plist plus the generated entitlements are
    /// already on disk, which can only be true if the merge ran first. A
    /// signature over a bundle mutated afterwards would be invalid.
    #[test]
    fn contributions_are_merged_before_the_bundle_is_signed() {
        let fixture = Fixture::new("contrib-macos-order")
            .manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\n\n\
                 [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
            )
            .binary("my_app");
        let runner = cargo_ok().with(
            "codesign ",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "errSecInternalComponent".to_string(),
            },
        );

        let mut lines = Vec::new();
        let err = build_with_contributions(
            &runner,
            &FakeEnv::new(),
            &fixture.dir,
            &release_info(),
            DesktopBundleTarget::Macos,
            Some(DesktopBundleTarget::Macos),
            Some(&[row(&PLIST_CAMERA), row(&ENTITLEMENT_CAMERA)]),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap_err();

        assert!(
            matches!(err, DesktopBuildError::CodesignFailed { .. }),
            "{err}"
        );
        assert!(
            fixture
                .read("build/desktop/macos/My App.app/Contents/Info.plist")
                .contains("NSCameraUsageDescription")
        );
        assert!(
            fixture
                .path("build/desktop/macos/my_app.entitlements")
                .is_file()
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "plugins: applying 2 desktop contribution(s)"),
            "{lines:?}"
        );
    }

    /// A copied plist is merged exactly like a generated one — into the build
    /// copy, never the project's file — and a key the project already
    /// declares wins, however the plugin would have spelled it. The fixture
    /// nests a dictionary so the insertion anchor (the LAST `</dict>`) is
    /// actually load-bearing.
    #[test]
    fn a_copied_plist_keeps_its_own_key_and_takes_the_new_one_into_the_root_dict() {
        let project_plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>CFBundleExecutable</key>\n\t<string>my_app</string>\n\
             \t<key>NSCameraUsageDescription</key>\n\t<string>My own words</string>\n\
             \t<key>NSAppTransportSecurity</key>\n\t<dict>\n\
             \t\t<key>NSAllowsArbitraryLoads</key>\n\t\t<true/>\n\t</dict>\n\
             </dict>\n</plist>\n";
        let fixture = Fixture::new("contrib-macos-copied")
            .default_manifest()
            .binary("my_app")
            .file("macos/Info.plist", project_plist);
        let runner = with_plutil(cargo_ok(), "CFBundleExecutable", "my_app");
        let runner = with_plutil(runner, "CFBundleIdentifier", "dev.f0x.my_app");

        let report = run_with(
            &runner,
            &fixture,
            DesktopBundleTarget::Macos,
            &[row(&PLIST_CAMERA), row(&PLIST_MIC)],
        )
        .unwrap();

        let merged = fixture.read("build/desktop/macos/My App.app/Contents/Info.plist");
        assert!(merged.contains("<string>My own words</string>"), "{merged}");
        assert!(!merged.contains("Scan a document"), "{merged}");
        // The new key landed in the ROOT dict, after the nested one closed.
        let key_at = merged
            .find("<key>NSMicrophoneUsageDescription</key>")
            .unwrap();
        assert!(
            key_at > merged.find("NSAllowsArbitraryLoads").unwrap(),
            "{merged}"
        );
        assert!(key_at < merged.rfind("</dict>").unwrap(), "{merged}");
        // The project's own file is untouched.
        assert_eq!(fixture.read("macos/Info.plist"), project_plist);

        assert!(
            report.notes.iter().any(|note| matches!(
                note,
                BundleNote::PluginEntryPresent { key, .. } if key == "NSCameraUsageDescription"
            )),
            "{:?}",
            report.notes
        );
        assert_eq!(applied_descriptions(&report).len(), 1, "{:?}", report.notes);
    }

    /// An entitlement needs a signature to mean anything, so an unsigned build
    /// applies none: one note, no generated file, and no failure — the one
    /// carve-out from the refuse-rather-than-skip rule.
    #[test]
    fn an_unsigned_macos_build_skips_entitlement_contributions_without_failing() {
        let fixture = Fixture::new("contrib-macos-unsigned")
            .default_manifest()
            .binary("my_app");

        let report = run_with(
            &cargo_ok(),
            &fixture,
            DesktopBundleTarget::Macos,
            &[row(&ENTITLEMENT_CAMERA)],
        )
        .unwrap();

        assert!(
            report
                .notes
                .contains(&BundleNote::EntitlementsSkippedUnsigned { count: 1 })
        );
        assert!(
            !fixture
                .path("build/desktop/macos/my_app.entitlements")
                .exists()
        );
        assert!(report.notes.contains(&BundleNote::Unsigned));
        // Nothing was signed, so there is no entitlements file to report.
        assert_eq!(report.entitlements, None);
    }

    /// A plist with no `</dict>` cannot carry a contributed key, and a bundle
    /// shipping without a declared usage description dies in front of a user —
    /// so the build refuses rather than assembling a bundle that is missing it.
    #[test]
    fn a_plist_with_no_insertion_anchor_fails_a_build_that_has_contributions() {
        let fixture = Fixture::new("contrib-macos-malformed")
            .default_manifest()
            .binary("my_app")
            .file("macos/Info.plist", "<plist version=\"1.0\">nope</plist>\n");

        let err = run_with(
            &cargo_ok(),
            &fixture,
            DesktopBundleTarget::Macos,
            &[row(&PLIST_CAMERA)],
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                DesktopBuildError::ContributionUnappliable { plugin_id, .. } if *plugin_id == "camera"
            ),
            "{err}"
        );

        // …and the same project builds fine with nothing to contribute.
        let report = run_with(&cargo_ok(), &fixture, DesktopBundleTarget::Macos, &[]).unwrap();
        assert!(report.executable.is_file());
    }

    /// Linux, both entry provenances: a generated entry takes the contributed
    /// key, and a copied one keeps the key it already declares.
    #[test]
    fn a_linux_build_merges_contributed_keys_into_generated_and_copied_entries() {
        let generated = Fixture::new("contrib-linux-generated")
            .default_manifest()
            .binary("my_app");
        let report = run_with(
            &cargo_ok(),
            &generated,
            DesktopBundleTarget::Linux,
            &[row(&DESKTOP_MIME)],
        )
        .unwrap();
        let entry = generated.read("build/desktop/linux/my_app/dev.f0x.my_app.desktop");
        assert!(entry.contains("\nMimeType=image/png;\n"), "{entry}");
        assert!(
            entry.contains("# Handled file types (frust plugin: camera)"),
            "{entry}"
        );
        assert!(report.notes.contains(&BundleNote::GeneratedDesktopEntry));
        assert_eq!(applied_descriptions(&report).len(), 1, "{:?}", report.notes);

        let copied = Fixture::new("contrib-linux-copied")
            .default_manifest()
            .binary("my_app")
            .file(
                "linux/app.desktop",
                "[Desktop Entry]\nType=Application\nName=My App\nExec=my_app\n\
                 Icon=dev.f0x.my_app\nCategories=Utility;\nTerminal=false\n",
            );
        let report = run_with(
            &cargo_ok(),
            &copied,
            DesktopBundleTarget::Linux,
            &[row(&DESKTOP_CATEGORIES), row(&DESKTOP_MIME)],
        )
        .unwrap();
        let entry = copied.read("build/desktop/linux/my_app/dev.f0x.my_app.desktop");
        assert!(entry.contains("\nCategories=Utility;\n"), "{entry}");
        assert!(!entry.contains("Graphics;"), "{entry}");
        assert!(entry.contains("\nMimeType=image/png;\n"), "{entry}");
        // The project's own entry is read, never written.
        assert!(!copied.read("linux/app.desktop").contains("MimeType"));
        assert!(
            report.notes.iter().any(|note| matches!(
                note,
                BundleNote::PluginEntryPresent { key, .. } if key == "Categories"
            )),
            "{:?}",
            report.notes
        );
    }

    /// No installed plugins is the overwhelmingly common case and must change
    /// nothing: the production collection path (no injected rows) records no
    /// plugin note at all, and a target with no variant of its own (Windows)
    /// applies nothing even when rows exist.
    #[test]
    fn a_build_with_no_desktop_contributions_records_nothing() {
        let fixture = Fixture::new("contrib-none")
            .default_manifest()
            .binary("my_app");
        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(
            !report.notes.iter().any(|note| matches!(
                note,
                BundleNote::PluginContribution { .. }
                    | BundleNote::PluginEntryPresent { .. }
                    | BundleNote::EntitlementsSkippedUnsigned { .. }
            )),
            "{:?}",
            report.notes
        );
        assert!(
            !fixture
                .read("build/desktop/linux/my_app/dev.f0x.my_app.desktop")
                .contains("frust plugin")
        );

        let windows = Fixture::new("contrib-windows")
            .default_manifest()
            .binary("my_app.exe");
        let report = run_with(
            &cargo_ok(),
            &windows,
            DesktopBundleTarget::Windows,
            &[row(&PLIST_CAMERA), row(&DESKTOP_MIME)],
        )
        .unwrap();
        assert!(
            !report.notes.iter().any(|note| matches!(
                note,
                BundleNote::PluginContribution { .. } | BundleNote::PluginEntryPresent { .. }
            )),
            "{:?}",
            report.notes
        );
    }

    /// **Real smoke, this host.** A genuine `cargo build --release` through
    /// `RealProcessRunner` over a minimal project laid out exactly like a
    /// scaffolded one (frust.toml + `linux/app.desktop` + an icon source),
    /// producing a real Linux bundle: real compile, real target-dir
    /// resolution (which must respect this machine's shared `build.target-dir`
    /// cargo config, not assume `<project>/target`), real icon pipeline, real
    /// file assembly.
    ///
    /// `#[ignore]`d because it compiles a real cargo project — run it with
    /// `cargo test -p frust-drive --lib -- --ignored linux_bundle_smoke`.
    #[test]
    #[ignore = "compiles a real cargo project through cargo itself; run with `cargo test -p frust-drive --lib -- --ignored linux_bundle_smoke`"]
    fn linux_bundle_smoke_builds_a_real_bundle_on_this_host() {
        use crate::process::RealProcessRunner;

        if DesktopBundleTarget::host() != Some(DesktopBundleTarget::Linux) {
            eprintln!("not a Linux host — skipping");
            return;
        }

        let fixture = Fixture::new("linux-smoke")
            .default_manifest()
            .icon(1024)
            .file(
                "Cargo.toml",
                "[package]\nname = \"my_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 publish = false\n\n[workspace]\n",
            )
            .file("src/main.rs", "fn main() { println!(\"hello\"); }")
            .file(
                "linux/app.desktop",
                "[Desktop Entry]\nType=Application\nName=My App\nExec=my_app\n\
                 Icon=dev.f0x.my_app\nCategories=Utility;\nTerminal=false\n",
            );

        let mut lines = Vec::new();
        let report = build_with_host(
            &RealProcessRunner,
            &RealEnv,
            &fixture.dir,
            &release_info(),
            DesktopBundleTarget::Linux,
            DesktopBundleTarget::host(),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap_or_else(|err| panic!("real linux bundle build failed: {err}\n{lines:#?}"));

        assert_eq!(report.root, fixture.path("build/desktop/linux/my_app"));
        assert!(report.executable.is_file());
        assert!(
            fixture
                .path("build/desktop/linux/my_app/dev.f0x.my_app.desktop")
                .is_file()
        );
        assert!(
            fixture
                .path(
                    "build/desktop/linux/my_app/share/icons/hicolor/256x256/apps/dev.f0x.my_app.png"
                )
                .is_file()
        );

        // The bundled binary really is the compiled program.
        let out = std::process::Command::new(&report.executable)
            .output()
            .expect("the bundled binary should be executable");
        assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
        eprintln!("bundle at {}", report.root.display());
        for note in &report.notes {
            eprintln!("note: {note}");
        }
    }

    /// **Real smoke over a real `frust create` project.** The same end-to-end
    /// as above, but the project is scaffolded by [`crate::scaffold::generate`]
    /// rather than hand-written, so the pipeline meets the genuine article:
    /// the scaffolded `frust.toml` (whose `[desktop] icon` points at the
    /// 128px placeholder logo the template ships — deliberately below the icon
    /// pipeline's minimum, and therefore a note rather than a failed build),
    /// the scaffolded `linux/app.desktop`, and the real app crate.
    ///
    /// Debug mode on purpose: this compiles the app's entire dependency graph
    /// (winit/wgpu/frust-engine), and a release build of it would multiply an already
    /// long compile for no extra coverage of anything this module owns.
    #[test]
    #[ignore = "compiles a scaffolded project's full dependency graph (winit/wgpu/frust-engine); run with `cargo test -p frust-drive --lib -- --ignored scaffolded_linux_bundle`"]
    fn scaffolded_linux_bundle_smoke_builds_a_real_bundle_on_this_host() {
        use crate::build_info::BuildMode;
        use crate::process::RealProcessRunner;
        use crate::scaffold::{self, context::TemplateContext};

        if DesktopBundleTarget::host() != Some(DesktopBundleTarget::Linux) {
            eprintln!("not a Linux host — skipping");
            return;
        }

        // This crate is `crates/frust-drive`, so the facade crate a scaffolded
        // project depends on by path is its sibling.
        let frust_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("frust");

        let fixture = Fixture::new("scaffolded-linux-smoke");
        scaffold::generate(
            &fixture.dir,
            &TemplateContext {
                project_name: "my_app".to_string(),
                title_case_name: "My App".to_string(),
                org: "dev.f0x".to_string(),
                description: "A new Frust application.".to_string(),
                frust_version: "0.1.0".to_string(),
                frust_path: frust_path.to_string_lossy().into_owned(),
                deeplink_scheme: None,
                deeplink_host: None,
            },
            None,
            true,
            None,
        )
        .expect("scaffolding the project");

        let info = BuildInfo::from_args(BuildArgs::default(), BuildMode::Debug).unwrap();
        let mut lines = Vec::new();
        let report = build_with_host(
            &RealProcessRunner,
            &RealEnv,
            &fixture.dir,
            &info,
            DesktopBundleTarget::Linux,
            DesktopBundleTarget::host(),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap_or_else(|err| {
            panic!(
                "scaffolded linux bundle build failed: {err}\n{}",
                lines.join("\n")
            )
        });

        assert_eq!(report.root, fixture.path("build/desktop/linux/my_app"));
        assert!(report.executable.is_file());
        // The scaffolded entry is copied verbatim, under the identifier name.
        let entry = fixture.read("build/desktop/linux/my_app/dev.f0x.my_app.desktop");
        assert!(entry.contains("Name=My App"), "{entry}");
        // The template's placeholder logo is below the icon pipeline's floor:
        // a note, and a bundle with no icon tree — never a failed build.
        assert!(
            report
                .notes
                .iter()
                .any(|note| matches!(note, BundleNote::IconUnusable { .. })),
            "{:?}",
            report.notes
        );
        eprintln!("bundle at {}", report.root.display());
        for note in &report.notes {
            eprintln!("note: {note}");
        }
    }
}
