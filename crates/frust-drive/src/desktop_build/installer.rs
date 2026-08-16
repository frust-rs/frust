//! Desktop installers: `.dmg` (macOS), NSIS `.exe` + WiX `.msi` (Windows),
//! `.deb` + `.AppImage` (Linux) — built by shelling out to a pinned
//! `cargo-packager` release over an already-assembled [`super::BundleReport`].
//!
//! **Separate from bundle assembly.** [`super::build`] never touches
//! `cargo-packager` — only [`build_installer`] does, so a plain
//! `frust build macos|windows|linux` still needs nothing but `cargo` and the
//! platform's own toolchain (`codesign`, `windows/build.rs`'s linker step).
//!
//! **Config contract.** `cargo-packager` reads its configuration from a JSON
//! or TOML file (`-c/--config <path>`) rather than exposing every knob as a
//! CLI flag, so this module synthesizes a minimal JSON config per invocation
//! from the manifest-resolved [`DesktopConfig`] plus the bundle's own
//! compiled executable — never touching the project's `Cargo.toml`, and
//! never relying on `cargo-packager`'s own `Cargo.toml`/`package.json`
//! auto-detection (see [`PackagerConfig`]'s `name` field doc for why that
//! matters). Verified against a real install of the pinned release
//! (`cargo install cargo-packager --version 0.11.8 --locked`): a `.deb` and
//! an `.AppImage` both built successfully from a config in exactly this
//! shape.
//!
//! **The `.dmg` arm builds its own `.app` — never the one this pipeline
//! assembled.** Confirmed against the pinned release's source
//! (`cargo-packager-0.11.8/src/package/mod.rs`'s `PackageFormat::Dmg` arm
//! and `package/app/mod.rs`): every `.dmg` build first calls `app::package`,
//! which synthesizes its *own* bundle from this config's `binaries`/`icons`
//! fields plus a freshly generated `Info.plist`, and only codesigns that
//! bundle when `config.macos().signing_identity` is `Some`. Left unset, the
//! shipped `.dmg` carries a bundle stripped of every identity value
//! [`super::macos::assemble`]/[`super::macos::codesign`] already resolved and
//! signed, regardless of what the prior `frust build macos` run produced —
//! this is what [`PackagerConfig`]'s `macos` block (Dmg-only) exists to fix:
//! `info_plist_path` names the *assembled* bundle's `Contents/Info.plist` (so
//! `app::package`'s `create_info_plist` merges its keys — `CFBundleExecutable`
//! included — over the synthesized one, per that function's source),
//! `signing_identity`/`entitlements` feed `app::package`'s own independent
//! codesign pass over the bundle it built.
//!
//! **…which is why the `.dmg` arm's `icons` entry is the bundle's own
//! `.icns`, not the project's source PNG.** `app::package` calls
//! `util::create_icns_file` and then names the file it produced in the plist
//! it synthesizes (`CFBundleIconFile`) — *before* merging our
//! `info_plist_path` over it, so **our** `CFBundleIconFile` (the binary name,
//! which is what [`super::macos::assemble`] names the generated `.icns`)
//! wins. Fed a PNG, `create_icns_file` writes `<product_name>.icns` (its
//! `dest_path.push(config.product_name)` + `set_extension("icns")`), i.e. the
//! *display* name, and the merged key then points at a file that isn't there —
//! a generic icon in the shipped `.dmg`. Fed a path that already ends in
//! `.icns`, the same function takes an early-return branch that copies the
//! file into `Contents/Resources` **under its own file name** (`out_dir.join(
//! icon_path.file_name())`) — so passing the assembled bundle's
//! `<binary_name>.icns` is what makes the merged key resolve. A bundle with no
//! `.icns` (no `[desktop] icon`, or a source the icon pipeline rejected) gets
//! no `icons` entry at all for `.dmg`: the raw PNG would only re-create the
//! name mismatch, and the plist key then simply dangles exactly as it already
//! does in the assembled bundle. Every other format keeps taking the source
//! PNG, which is what `create_icns_file`'s siblings (`.deb`/AppImage/Windows)
//! actually want.
//!
//! **Non-Apple signing identities are deliberately *not* wired into the
//! packager config.** `cargo-packager`'s own `codesign/macos.rs::sign` pushes
//! `--timestamp` unconditionally, and a secure timestamp needs Apple's
//! timestamp service over the network — so handing it an ad-hoc/self-signed
//! identity turns `--installer dmg` into a network-dependent (and typically
//! failing) build, contradicting the offline guarantee
//! [`super::macos::codesign`] keeps for exactly those identities. For an
//! identity that isn't Apple-issued
//! ([`super::macos::is_apple_issued_identity`], the same predicate the
//! bundle's own `codesign` step uses), `signingIdentity` is left out and an
//! [`InstallerNote::PackagerSigningSkipped`] says so: the `.app` inside the
//! `.dmg` ships unsigned, the assembled bundle's own signature is untouched,
//! and signing stays offline.
//!
//! **Notarization is opt-in, and the packaging child's Apple credentials are
//! scrubbed until it is.** `cargo-packager`'s app-packaging path does not stop
//! at codesigning: right after `codesign::try_sign` it falls through to
//! `codesign::notarize_auth()` (`0.11.8`'s `src/package/app/mod.rs` and
//! `src/codesign/macos.rs`), which resolves credentials **purely from the
//! ambient environment** — `APPLE_KEYCHAIN_PROFILE`, else
//! `APPLE_ID`+`APPLE_PASSWORD`+`APPLE_TEAM_ID`, else
//! `APPLE_API_KEY`+`APPLE_API_ISSUER` (with `APPLE_API_KEY_PATH`, or an
//! `AuthKey_<key id>.p8` discovered under `./private_keys`, `~/private_keys`,
//! `~/.private_keys`, `~/.appstoreconnect/private_keys`) — and on success
//! **uploads** the built `.app` to Apple's notary service, waits for the
//! verdict, and turns a rejection or a submission failure into a hard build
//! error. `try_sign` reads two more (`APPLE_CERTIFICATE`,
//! `APPLE_CERTIFICATE_PASSWORD`) to import a p12 into a temporary keychain.
//! Frust spawns the tool with this process's environment inherited, so a
//! developer's ordinary exported credentials would silently arm all of that.
//! The default is therefore to remove [`APPLE_CREDENTIAL_ENV_VARS`] from the
//! packaging child's environment — the fallback then resolves nothing and
//! `cargo-packager` logs a skip — with an [`InstallerNote::NotarizationSuppressed`]
//! saying so and naming the opt-in. `[macos] notarize = true` passes them
//! through instead, and says *that* with an
//! [`InstallerNote::NotarizationEnabled`]. Both notes are emitted only when the
//! config really carries a `signingIdentity` (Dmg + an Apple-issued identity),
//! since that is the only case in which the packager reaches its notarization
//! branch at all.
//!
//! **`.rpm` is a typed refusal, not a supported format.** `cargo-packager`
//! 0.11 has no RPM backend at all (only `.deb`, AppImage, and pacman on
//! Linux) — [`InstallerFormat::requested`] refuses `"rpm"` up front and names
//! `tauri-bundler` as the documented fallback, rather than letting it fall
//! through to a confusing "unsupported format" error from the tool itself.
//!
//! **Print-free.** Packaging output streams through the caller's `on_line`
//! sink, prefixed `[cargo-packager]`, exactly like [`super::cargo::build`]'s
//! `[cargo]` prefix.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build_info::BuildInfo;
use crate::manifest;
use crate::process::{ProcessRunner, tail_lines};

use super::config::DesktopConfig;
use super::{BundleReport, DesktopBundleTarget};

/// The `cargo-packager` version Frust pins (LAW — see
/// `docs/CLI_DEVELOPMENT.md`'s Version Pins table, and the doctor validator
/// in `crate::doctor::cargo_packager`). Resolved from crates.io's current
/// stable release when this pipeline was built (`0.11.8`, published under
/// the upstream `crabnebula-dev/cargo-packager` repo's
/// `@crabnebula/packager-v0.11.8` tag) — bump only deliberately, re-running
/// this module's tests and a real
/// `cargo install cargo-packager --version <new> --locked` smoke afterward.
pub const CARGO_PACKAGER_PINNED: &str = "0.11.8";

/// Every environment variable `cargo-packager` 0.11.8 reads Apple credentials
/// from, removed from the packaging child's environment unless `[macos]
/// notarize = true` opts in (see this module's doc for what they arm).
///
/// Read off the pinned release's source rather than its documentation, and
/// complete as of that release: `src/codesign/macos.rs::notarize_auth` reads
/// the first seven (`APPLE_API_KEY_PATH` is the one the notarization docs
/// don't mention — it short-circuits the `AuthKey_*.p8` directory search),
/// and `try_sign`/`setup_keychain` in the same file read the last two to
/// import a p12 into a temporary keychain. Nothing else in the crate reads an
/// `APPLE_*` variable (`CI`, `CARGO_TERM_COLOR` and WiX's environment sweep
/// are the only other `env::var` sites, and none of them carry credentials).
/// Re-check this list against the source whenever [`CARGO_PACKAGER_PINNED`]
/// moves.
const APPLE_CREDENTIAL_ENV_VARS: &[&str] = &[
    "APPLE_KEYCHAIN_PROFILE",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_PATH",
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
];

/// How many trailing `cargo packager` output lines a failed invocation
/// reports — the same bound [`super::cargo::build`] uses for a failed
/// compile.
const FAILURE_TAIL_LINES: usize = 50;

/// Fallback installer version when `--build-name` isn't given — the same
/// `0.1.0` [`super::macos::assemble`]'s generated `Info.plist` falls back to.
const DEFAULT_INSTALLER_VERSION: &str = "0.1.0";

/// One installer `cargo-packager` can produce, grouped by the host it needs.
/// `.rpm` deliberately has no variant here — see this module's doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerFormat {
    /// macOS `.dmg`.
    Dmg,
    /// Windows NSIS `.exe` installer.
    NsisExe,
    /// Windows WiX Toolset `.msi`.
    WixMsi,
    /// Linux `.deb`.
    Deb,
    /// Linux `.AppImage`.
    AppImage,
}

impl InstallerFormat {
    /// The [`DesktopBundleTarget`] a bundle must already be assembled for
    /// before this format's installer can be built over it.
    pub fn target(self) -> DesktopBundleTarget {
        match self {
            InstallerFormat::Dmg => DesktopBundleTarget::Macos,
            InstallerFormat::NsisExe | InstallerFormat::WixMsi => DesktopBundleTarget::Windows,
            InstallerFormat::Deb | InstallerFormat::AppImage => DesktopBundleTarget::Linux,
        }
    }

    /// Every format available for `target` — one for macOS, two each for
    /// Windows and Linux.
    pub fn for_target(target: DesktopBundleTarget) -> &'static [InstallerFormat] {
        match target {
            DesktopBundleTarget::Macos => &[InstallerFormat::Dmg],
            DesktopBundleTarget::Windows => &[InstallerFormat::NsisExe, InstallerFormat::WixMsi],
            DesktopBundleTarget::Linux => &[InstallerFormat::Deb, InstallerFormat::AppImage],
        }
    }

    /// The exact string `cargo-packager`'s `formats`/`--formats` value takes
    /// for this format, and this format's CLI-facing name (`frust build
    /// --installer <name>`).
    pub fn as_str(self) -> &'static str {
        match self {
            InstallerFormat::Dmg => "dmg",
            InstallerFormat::NsisExe => "nsis",
            InstallerFormat::WixMsi => "wix",
            InstallerFormat::Deb => "deb",
            InstallerFormat::AppImage => "appimage",
        }
    }

    /// The file extension `cargo-packager` gives this format's output —
    /// [`discover_artifacts`]'s filter. `AppImage`'s case matters:
    /// `cargo-packager` emits `<name>.AppImage`, not `.appimage`.
    fn artifact_extension(self) -> &'static str {
        match self {
            InstallerFormat::Dmg => "dmg",
            InstallerFormat::NsisExe => "exe",
            InstallerFormat::WixMsi => "msi",
            InstallerFormat::Deb => "deb",
            InstallerFormat::AppImage => "AppImage",
        }
    }

    /// Parses a CLI-facing format name (`frust build --installer <name>`),
    /// case-insensitively. `"rpm"` is a typed refusal
    /// ([`InstallerError::RpmNotSupported`]) rather than
    /// [`InstallerError::UnknownFormat`] — it names a real package format,
    /// just one `cargo-packager` doesn't build.
    pub fn requested(name: &str) -> Result<InstallerFormat, InstallerError> {
        match name.to_ascii_lowercase().as_str() {
            "dmg" => Ok(InstallerFormat::Dmg),
            "nsis" | "exe" => Ok(InstallerFormat::NsisExe),
            "wix" | "msi" => Ok(InstallerFormat::WixMsi),
            "deb" => Ok(InstallerFormat::Deb),
            "appimage" => Ok(InstallerFormat::AppImage),
            "rpm" => Err(InstallerError::RpmNotSupported),
            other => Err(InstallerError::UnknownFormat(other.to_string())),
        }
    }
}

impl fmt::Display for InstallerFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything that can stop an installer build — a library-contract enum
/// callers match on, mirroring [`super::DesktopBuildError`]'s shape but kept
/// separate: an installer failure is never a bundle-assembly failure, and a
/// caller (the CLI/TUI) needs to tell "no cargo-packager" apart from "the
/// packaging run itself failed" apart from "you asked for a format this tool
/// can't build at all".
#[derive(Debug, thiserror::Error)]
pub enum InstallerError {
    #[error(
        "'.rpm' isn't a cargo-packager format (it builds .deb/AppImage/pacman on Linux, \
         nothing else) — use tauri-bundler (https://v2.tauri.app/distribute/rpm/) for an \
         .rpm, or `alien` to convert the .deb this pipeline produces"
    )]
    RpmNotSupported,

    #[error("unknown installer format '{0}' — supported: dmg, nsis, wix, deb, appimage")]
    UnknownFormat(String),

    #[error(
        "a {format} installer needs a {} bundle, but the given bundle is {}",
        format.target(),
        bundle_target
    )]
    BundleTargetMismatch {
        format: InstallerFormat,
        bundle_target: DesktopBundleTarget,
    },

    #[error("reading `frust.toml` in '{project_dir}': {reason}")]
    Manifest {
        project_dir: PathBuf,
        reason: String,
    },

    #[error(
        "cargo-packager not found on PATH. Run: cargo install cargo-packager --version {CARGO_PACKAGER_PINNED} --locked"
    )]
    ToolMissing,

    #[error(
        "cargo-packager {found} found, but Frust pins {CARGO_PACKAGER_PINNED} — run: cargo install cargo-packager --version {CARGO_PACKAGER_PINNED} --locked"
    )]
    VersionMismatch { found: String },

    #[error("{action} '{path}': {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("serializing the cargo-packager config: {0}")]
    ConfigSerialize(#[source] serde_json::Error),

    #[error("spawning `cargo packager`: {reason}")]
    PackagerSpawn { reason: String },

    #[error("`cargo packager --config {config}` failed:\n{tail}")]
    PackagerFailed { config: PathBuf, tail: String },
}

/// A non-fatal observation about an installer build, returned instead of
/// printed so the CLI and the TUI can each render it their own way — the
/// installer-side counterpart of [`super::BundleNote`]. Every note is also
/// streamed through `on_line` as a `warning: …` line while the build runs, so
/// a front-end that only echoes output still surfaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallerNote {
    /// `[macos] signing-identity` names an identity Apple did not issue, so it
    /// was **not** passed to `cargo-packager` — see this module's doc for the
    /// `--timestamp`/offline reason.
    PackagerSigningSkipped { identity: String },
    /// The default: this build could have notarized (it hands
    /// `cargo-packager` an Apple-issued identity), so the Apple credential
    /// variables were removed from the packaging child's environment and no
    /// upload to Apple happened.
    NotarizationSuppressed,
    /// `[macos] notarize = true`: the credentials were passed through, so the
    /// packaging step may submit the signed `.app` to Apple.
    NotarizationEnabled,
}

impl fmt::Display for InstallerNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallerNote::PackagerSigningSkipped { identity } => write!(
                f,
                "`[macos] signing-identity` '{identity}' is not Apple-issued, so \
                 cargo-packager's own signing was skipped (it always signs with \
                 `--timestamp`, which needs Apple's timestamp service over the network) \
                 — the .app inside the .dmg ships unsigned. The assembled bundle's own \
                 codesign is unaffected, and signing stays offline"
            ),
            InstallerNote::NotarizationSuppressed => write!(
                f,
                "cargo-packager notarizes — and uploads to Apple — every .app it signs as \
                 soon as Apple credentials are readable from its environment, so this build \
                 ran with all of them removed from it (APPLE_KEYCHAIN_PROFILE, \
                 APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID, \
                 APPLE_API_KEY/APPLE_API_ISSUER/APPLE_API_KEY_PATH, \
                 APPLE_CERTIFICATE/APPLE_CERTIFICATE_PASSWORD): the .dmg's app is signed but \
                 NOT notarized, and nothing was sent to Apple. Add `notarize = true` under \
                 `[macos]` in frust.toml to notarize on purpose"
            ),
            InstallerNote::NotarizationEnabled => write!(
                f,
                "`[macos] notarize = true`: Apple credentials are passed through to \
                 cargo-packager, which UPLOADS the signed .app to Apple's notary service \
                 (`xcrun notarytool submit --wait`) using whatever this build's environment \
                 carries (APPLE_KEYCHAIN_PROFILE, or APPLE_ID + APPLE_PASSWORD + \
                 APPLE_TEAM_ID, or APPLE_API_KEY + APPLE_API_ISSUER). With none of them set \
                 it warns and skips; once they resolve, a failed or rejected submission \
                 fails this build"
            ),
        }
    }
}

/// What a successful [`build_installer`] call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerReport {
    pub format: InstallerFormat,
    /// The directory `cargo-packager`'s `outDir` pointed at — fresh for this
    /// run (see [`prepare_out_dir`]), one per format so building both Windows
    /// (or both Linux) formats in sequence never clobbers the other's output.
    pub out_dir: PathBuf,
    /// Every file matching `format`'s extension found in `out_dir` after a
    /// successful run, sorted for determinism. Never includes the generated
    /// `packager.json` or `cargo-packager`'s own tool cache
    /// (`out_dir/.cargo-packager`, observed during the real smoke run).
    pub artifacts: Vec<PathBuf>,
    /// Everything worth telling a human that is not a failure — see
    /// [`InstallerNote`].
    pub notes: Vec<InstallerNote>,
}

/// Builds `format`'s installer over `bundle` (a prior [`super::build`] call's
/// [`BundleReport`] for the matching [`DesktopBundleTarget`]).
///
/// Order: verify `bundle.target` actually matches `format`, verify
/// `cargo-packager` is on `PATH` at the pinned version (never the packaging
/// run itself failing — a distinct, typed precondition), resolve `frust.toml`
/// for the desktop identity, write a fresh config into a per-format output
/// directory, then invoke `cargo packager --config <path>` through `runner`.
///
/// **Print-free**: packaging output streams through `on_line`, prefixed
/// `[cargo-packager]`.
pub fn build_installer(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    bundle: &BundleReport,
    format: InstallerFormat,
    on_line: &mut dyn FnMut(&str),
) -> Result<InstallerReport, InstallerError> {
    if format.target() != bundle.target {
        return Err(InstallerError::BundleTargetMismatch {
            format,
            bundle_target: bundle.target,
        });
    }

    verify_cargo_packager(runner)?;

    let manifest = manifest::load(project_dir).map_err(|err| InstallerError::Manifest {
        project_dir: project_dir.to_path_buf(),
        reason: format!("{err:#}"),
    })?;
    // Path-unsafe desktop identity values are refused here exactly as they are
    // in `super::build` — the same resolution, so the same typed rejection,
    // reported as the manifest problem it is.
    let config =
        DesktopConfig::resolve(project_dir, &manifest).map_err(|err| InstallerError::Manifest {
            project_dir: project_dir.to_path_buf(),
            reason: err.to_string(),
        })?;

    let out_dir = format
        .target()
        .dist_dir(project_dir)
        .join("installer")
        .join(format.as_str());
    prepare_out_dir(&out_dir)?;

    let mut notes = Vec::new();
    let packager_config = PackagerConfig::new(
        &config,
        info,
        bundle,
        format,
        &out_dir,
        project_dir,
        &mut notes,
    );
    let config_path = out_dir.join("packager.json");
    write_config(&config_path, &packager_config)?;

    // Credentials are scrubbed unless the manifest opts in — see this module's
    // doc. The scrub itself is unconditional (a format that never signs has
    // nothing to lose by it); only the note is conditional, on the config
    // really carrying the `signingIdentity` that gets `cargo-packager` as far
    // as its notarization branch.
    let scrubbed_env: &[&str] = if config.macos_notarize {
        &[]
    } else {
        APPLE_CREDENTIAL_ENV_VARS
    };
    if packager_config.signs_with_an_apple_identity() {
        notes.push(if config.macos_notarize {
            InstallerNote::NotarizationEnabled
        } else {
            InstallerNote::NotarizationSuppressed
        });
    }

    // Streamed *before* the packaging run, so the reason a `.dmg`'s inner app
    // will come out unsigned — or the fact that this run may upload it to
    // Apple — is on screen ahead of the tool's own output rather than after it
    // (the same `warning: ` shape the run pipelines use).
    for note in &notes {
        on_line(&format!("warning: {note}"));
    }

    let config_arg = config_path.to_string_lossy().into_owned();
    let args = ["packager", "--config", config_arg.as_str()];
    let mut prefixed = |line: &str| on_line(&format!("[cargo-packager] {line}"));
    let out = runner
        .run_streaming_scrubbed(
            "cargo",
            &args,
            Some(project_dir),
            &[],
            scrubbed_env,
            &mut prefixed,
        )
        .map_err(|err| InstallerError::PackagerSpawn {
            reason: format!("{err:#}"),
        })?;

    if !out.success {
        let tail = tail_lines(&out.stderr, FAILURE_TAIL_LINES);
        let tail = if tail.is_empty() {
            tail_lines(&out.stdout, FAILURE_TAIL_LINES)
        } else {
            tail
        };
        return Err(InstallerError::PackagerFailed {
            config: config_path,
            tail,
        });
    }

    let artifacts = discover_artifacts(&out_dir, format);
    on_line(&format!(
        "installer: {} artifact(s) in {}",
        artifacts.len(),
        out_dir.display()
    ));

    Ok(InstallerReport {
        format,
        out_dir,
        artifacts,
        notes,
    })
}

/// Checks `cargo-packager --version` (mirroring `doctor::CargoNdkValidator`'s
/// `cargo ndk --version` shape) and requires an exact match against
/// [`CARGO_PACKAGER_PINNED`] — pins are LAW, so a newer or older release
/// found on `PATH` is [`InstallerError::VersionMismatch`], not a soft warning.
fn verify_cargo_packager(runner: &dyn ProcessRunner) -> Result<(), InstallerError> {
    let out = runner
        .run("cargo", &["packager", "--version"])
        .map_err(|_| InstallerError::ToolMissing)?;
    if !out.success {
        return Err(InstallerError::ToolMissing);
    }
    let found = parse_version(&out.stdout).unwrap_or_else(|| out.stdout.trim().to_string());
    if found != CARGO_PACKAGER_PINNED {
        return Err(InstallerError::VersionMismatch { found });
    }
    Ok(())
}

/// Parses `"cargo-packager 0.11.8\n"` into `"0.11.8"` — the real pinned
/// release's `--version` output, confirmed by installing it.
fn parse_version(text: &str) -> Option<String> {
    let first_line = text.lines().next()?;
    first_line.split_whitespace().nth(1).map(str::to_string)
}

/// Creates `dir` fresh, mirroring [`super::bundle::prepare_dir`]: an existing
/// installer output directory is removed first, so building the same format
/// twice never leaves a stale artifact behind for [`discover_artifacts`] to
/// pick up alongside the new one.
fn prepare_out_dir(dir: &Path) -> Result<(), InstallerError> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|source| InstallerError::Io {
            action: "removing the previous installer output at",
            path: dir.to_path_buf(),
            source,
        })?;
    }
    fs::create_dir_all(dir).map_err(|source| InstallerError::Io {
        action: "creating directory",
        path: dir.to_path_buf(),
        source,
    })
}

fn write_config(path: &Path, config: &PackagerConfig) -> Result<(), InstallerError> {
    let json = serde_json::to_string_pretty(config).map_err(InstallerError::ConfigSerialize)?;
    fs::write(path, json).map_err(|source| InstallerError::Io {
        action: "writing",
        path: path.to_path_buf(),
        source,
    })
}

/// Every file in `out_dir` whose extension matches `format`'s
/// ([`InstallerFormat::artifact_extension`]), case-insensitively (AppImage's
/// own extension is mixed-case), sorted for determinism.
fn discover_artifacts(out_dir: &Path, format: InstallerFormat) -> Vec<PathBuf> {
    let ext = format.artifact_extension();
    let mut found: Vec<PathBuf> = fs::read_dir(out_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|found| found.eq_ignore_ascii_case(ext))
        })
        .collect();
    found.sort();
    found
}

/// The minimal `cargo-packager` config this pipeline synthesizes — see this
/// module's doc for why a generated file (not `Cargo.toml`'s own
/// `[package.metadata.packager]`, which this pipeline never touches) is the
/// right seam.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackagerConfig {
    /// **Load-bearing, not cosmetic.** Left unset, `cargo-packager` 0.11.8's
    /// config loader tries to auto-detect a package name by
    /// `set_current_dir`-ing into the *config file's own path* (not its
    /// parent directory) while hunting for a `Cargo.toml`/`package.json` —
    /// verified directly against the pinned release, this fails every time
    /// with `Not a directory (os error 20)`, since the config path names a
    /// file. Setting `name` here skips that lookup entirely.
    name: String,
    product_name: String,
    version: String,
    identifier: String,
    out_dir: String,
    binaries: Vec<PackagerBinary>,
    /// The icon input, resolved per format (see this module's doc): the
    /// assembled bundle's own `<binary_name>.icns` for `.dmg` — the only
    /// spelling whose name survives into the synthesized bundle's
    /// `CFBundleIconFile` — and the project's `[desktop] icon` source PNG for
    /// every other format. Omitted entirely (never a JSON `null`) when there
    /// is no such input.
    #[serde(skip_serializing_if = "Option::is_none")]
    icons: Option<Vec<String>>,
    formats: Vec<&'static str>,
    /// The macOS identity block — see this module's doc for why it exists.
    /// `Some` only for [`InstallerFormat::Dmg`] (the only arm that builds its
    /// own, otherwise identity-stripped `.app`); every other format leaves
    /// this key out of the written JSON entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    macos: Option<PackagerMacosConfig>,
}

#[derive(Debug, Serialize)]
struct PackagerBinary {
    path: String,
    main: bool,
}

/// `cargo-packager` 0.11.8's `MacOsConfig` fields this pipeline actually
/// drives, confirmed by field name against the pinned release's source
/// (`cargo-packager-0.11.8/src/config/mod.rs`'s `MacOsConfig`, `#[serde(rename_all
/// = "camelCase")]`): `info_plist_path` → `infoPlistPath`, `signing_identity`
/// → `signingIdentity`, `entitlements` → `entitlements` (no rename). Every
/// other `MacOsConfig` field (frameworks, notarization credentials, exception
/// domain, …) has no `DesktopConfig` counterpart and is left unset.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackagerMacosConfig {
    /// The already-assembled bundle's own `Contents/Info.plist` (copied from
    /// the project or generated by [`super::macos::assemble`] — either way it
    /// exists by the time an installer build runs over it). `app::package`
    /// merges this plist's keys over its own freshly generated one, which is
    /// what carries `CFBundleExecutable`/`CFBundleIdentifier`/`CFBundleIconFile`
    /// into the synthesized `.dmg` bundle.
    info_plist_path: String,
    /// `[macos] signing-identity`, resolved the same way
    /// [`super::macos::codesign`] reads it — but **only** when it names an
    /// Apple-issued identity (see this module's doc for the `--timestamp`
    /// reason a local one is suppressed). `None` leaves the synthesized bundle
    /// unsigned, exactly like an unconfigured `frust build macos`.
    #[serde(skip_serializing_if = "Option::is_none")]
    signing_identity: Option<String>,
    /// The entitlements the packager's own codesign pass signs the `.app` it
    /// synthesizes with, resolved by
    /// [`super::contributions::entitlements_for_packaging`]: the merged file a
    /// `frust build macos` generated beside the `.app` when a plugin
    /// contributed an entitlement, else `<project>/macos/app.entitlements`
    /// when the project ships one, else nothing. Sharing that resolution with
    /// the assembly's own `codesign` call is what keeps a `.dmg`'s inner app
    /// from being signed with fewer entitlements than the bundle it was built
    /// from.
    #[serde(skip_serializing_if = "Option::is_none")]
    entitlements: Option<String>,
}

/// The `.icns` this bundle's assembly really wrote, read off
/// [`BundleReport::artifacts`] (the record of what was produced) rather than
/// re-derived from the bundle layout — `Contents/Resources/<binary_name>.icns`
/// is [`super::macos::assemble`]'s business, and an assembly that skipped the
/// icon steps lists no such artifact, which is exactly the "no icon input"
/// answer wanted here.
fn generated_icns<'a>(bundle: &'a BundleReport, config: &DesktopConfig) -> Option<&'a Path> {
    let expected = format!("{}.icns", config.binary_name);
    bundle
        .artifacts
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == expected)
        })
        .map(PathBuf::as_path)
}

impl PackagerConfig {
    fn new(
        config: &DesktopConfig,
        info: &BuildInfo,
        bundle: &BundleReport,
        format: InstallerFormat,
        out_dir: &Path,
        project_dir: &Path,
        notes: &mut Vec<InstallerNote>,
    ) -> PackagerConfig {
        let icons = match format {
            // The bundle's own `.icns`, or nothing at all — never the source
            // PNG, whose repacked name would not match the merged
            // `CFBundleIconFile` (this module's doc).
            InstallerFormat::Dmg => {
                generated_icns(bundle, config).map(|icns| vec![icns.to_string_lossy().into_owned()])
            }
            _ => config
                .icon_source
                .as_ref()
                .filter(|source| source.is_file())
                .map(|source| vec![source.to_string_lossy().into_owned()]),
        };

        let macos = (format == InstallerFormat::Dmg).then(|| PackagerMacosConfig {
            info_plist_path: bundle
                .root
                .join("Contents")
                .join("Info.plist")
                .to_string_lossy()
                .into_owned(),
            signing_identity: config.macos_signing_identity.clone().and_then(|identity| {
                if super::macos::is_apple_issued_identity(&identity) {
                    Some(identity)
                } else {
                    notes.push(InstallerNote::PackagerSigningSkipped { identity });
                    None
                }
            }),
            entitlements: super::contributions::entitlements_for_packaging(project_dir, config)
                .map(|path| path.to_string_lossy().into_owned()),
        });

        PackagerConfig {
            name: config.binary_name.clone(),
            product_name: config.display_name.clone(),
            version: info
                .build_name
                .clone()
                .unwrap_or_else(|| DEFAULT_INSTALLER_VERSION.to_string()),
            identifier: config.identifier.clone(),
            out_dir: out_dir.to_string_lossy().into_owned(),
            binaries: vec![PackagerBinary {
                path: bundle.executable.to_string_lossy().into_owned(),
                main: true,
            }],
            icons,
            formats: vec![format.as_str()],
            macos,
        }
    }

    /// Whether this config actually asks `cargo-packager` to codesign — i.e.
    /// carries a `signingIdentity`, which only a Dmg build with an
    /// Apple-issued identity ever does. That is exactly the condition under
    /// which the tool reaches its post-signing notarization branch, so it is
    /// what the notarization notes key off rather than the raw manifest value.
    fn signs_with_an_apple_identity(&self) -> bool {
        self.macos
            .as_ref()
            .is_some_and(|macos| macos.signing_identity.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::process::{FakeProcessRunner, Output};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-installer-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A project fixture: `frust.toml` plus a planted "compiled" binary the
    /// way a prior [`super::build`] call would have left it.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            Fixture { dir: temp_dir(tag) }
        }

        fn manifest(self, body: &str) -> Fixture {
            fs::write(self.dir.join("frust.toml"), body).unwrap();
            self
        }

        fn default_manifest(self) -> Fixture {
            self.manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n",
            )
        }

        fn bundle(&self, target: DesktopBundleTarget, binary_name: &str) -> BundleReport {
            let root = target.dist_dir(&self.dir).join(binary_name);
            let executable = root.join(binary_name);
            fs::create_dir_all(&root).unwrap();
            fs::write(&executable, b"#!/bin/sh\ntrue\n").unwrap();
            BundleReport {
                target,
                root,
                executable,
                artifacts: Vec::new(),
                notes: Vec::new(),
            }
        }

        /// The realistic macOS shape `super::macos::assemble` produces:
        /// `Contents/{MacOS,Resources}` plus an `Info.plist` whose
        /// `CFBundleIconFile` names the generated icon exactly the way the
        /// generated plist does (the bare binary name, no extension), with
        /// every written file listed in `artifacts`. `icon = false` is the
        /// assembly whose icon steps were skipped or rejected: no `.icns`, and
        /// no `CFBundleIconFile` key either.
        fn macos_app(&self, binary_name: &str, icon: bool) -> BundleReport {
            let root = DesktopBundleTarget::Macos
                .dist_dir(&self.dir)
                .join("My App.app");
            let contents = root.join("Contents");
            let resources = contents.join("Resources");
            fs::create_dir_all(contents.join("MacOS")).unwrap();
            fs::create_dir_all(&resources).unwrap();

            let executable = contents.join("MacOS").join(binary_name);
            fs::write(&executable, b"#!/bin/sh\ntrue\n").unwrap();
            let mut artifacts = vec![executable.clone()];

            let icon_key = if icon {
                let icns = resources.join(format!("{binary_name}.icns"));
                fs::write(&icns, b"not-really-an-icns").unwrap();
                artifacts.push(icns);
                format!("\t<key>CFBundleIconFile</key>\n\t<string>{binary_name}</string>\n")
            } else {
                String::new()
            };

            let plist = contents.join("Info.plist");
            fs::write(
                &plist,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <plist version=\"1.0\">\n<dict>\n\
                     \t<key>CFBundleExecutable</key>\n\t<string>{binary_name}</string>\n\
                     {icon_key}</dict>\n</plist>\n"
                ),
            )
            .unwrap();
            artifacts.push(plist);

            BundleReport {
                target: DesktopBundleTarget::Macos,
                root,
                executable,
                artifacts,
                notes: Vec::new(),
            }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.join(rel)
        }
    }

    /// A `<key>K</key><string>V</string>` read out of a fixture plist — enough
    /// for the one key these tests compare against, without pulling a plist
    /// parser into `frust-drive` for a test.
    fn plist_string(plist: &str, key: &str) -> Option<String> {
        let after = plist.split_once(&format!("<key>{key}</key>"))?.1;
        let open = after.find("<string>")? + "<string>".len();
        let close = after[open..].find("</string>")? + open;
        Some(after[open..close].to_string())
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn info() -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default(), BuildMode::Release).unwrap()
    }

    fn version_ok() -> FakeProcessRunner {
        FakeProcessRunner::new().with(
            "cargo packager --version",
            Output {
                success: true,
                stdout: format!("cargo-packager {CARGO_PACKAGER_PINNED}\n"),
                stderr: String::new(),
            },
        )
    }

    #[test]
    fn requesting_rpm_is_a_typed_refusal_naming_the_fallback() {
        let err = InstallerFormat::requested("rpm").unwrap_err();
        assert!(matches!(err, InstallerError::RpmNotSupported), "{err}");
        assert!(err.to_string().contains("tauri-bundler"), "{err}");
    }

    #[test]
    fn requesting_an_unknown_format_is_a_typed_error() {
        let err = InstallerFormat::requested("foo").unwrap_err();
        assert!(
            matches!(&err, InstallerError::UnknownFormat(name) if name == "foo"),
            "{err}"
        );
    }

    #[test]
    fn requested_parses_every_supported_format_case_insensitively() {
        assert_eq!(
            InstallerFormat::requested("DMG").unwrap(),
            InstallerFormat::Dmg
        );
        assert_eq!(
            InstallerFormat::requested("Nsis").unwrap(),
            InstallerFormat::NsisExe
        );
        assert_eq!(
            InstallerFormat::requested("MSI").unwrap(),
            InstallerFormat::WixMsi
        );
        assert_eq!(
            InstallerFormat::requested("Wix").unwrap(),
            InstallerFormat::WixMsi
        );
        assert_eq!(
            InstallerFormat::requested("Deb").unwrap(),
            InstallerFormat::Deb
        );
        assert_eq!(
            InstallerFormat::requested("APPIMAGE").unwrap(),
            InstallerFormat::AppImage
        );
    }

    #[test]
    fn for_target_lists_the_right_formats_per_os() {
        assert_eq!(
            InstallerFormat::for_target(DesktopBundleTarget::Macos),
            &[InstallerFormat::Dmg]
        );
        assert_eq!(
            InstallerFormat::for_target(DesktopBundleTarget::Windows),
            &[InstallerFormat::NsisExe, InstallerFormat::WixMsi]
        );
        assert_eq!(
            InstallerFormat::for_target(DesktopBundleTarget::Linux),
            &[InstallerFormat::Deb, InstallerFormat::AppImage]
        );
    }

    #[test]
    fn a_bundle_built_for_the_wrong_target_is_a_typed_error() {
        let fixture = Fixture::new("target-mismatch").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Linux, "my_app");
        let err = build_installer(
            &version_ok(),
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                InstallerError::BundleTargetMismatch {
                    format: InstallerFormat::Dmg,
                    bundle_target: DesktopBundleTarget::Linux
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn missing_cargo_packager_is_a_typed_tool_missing_error() {
        let fixture = Fixture::new("tool-missing").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Linux, "my_app");
        let runner = FakeProcessRunner::new().missing("cargo packager --version");
        let err = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Deb,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, InstallerError::ToolMissing), "{err}");
        assert!(err.to_string().contains("cargo install cargo-packager"));
    }

    #[test]
    fn a_version_mismatch_is_a_typed_error_naming_the_pin() {
        let fixture = Fixture::new("version-mismatch").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Linux, "my_app");
        let runner = FakeProcessRunner::new().with(
            "cargo packager --version",
            Output {
                success: true,
                stdout: "cargo-packager 0.9.0\n".to_string(),
                stderr: String::new(),
            },
        );
        let err = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Deb,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(err, InstallerError::VersionMismatch { ref found } if found == "0.9.0"),
            "{err}"
        );
        assert!(err.to_string().contains(CARGO_PACKAGER_PINNED), "{err}");
    }

    /// The version check runs before anything else: with no `cargo packager
    /// --config` registration at all, a tool-missing/mismatched run must
    /// never reach the packaging invocation.
    #[test]
    fn a_missing_tool_never_reaches_the_packaging_invocation() {
        let fixture = Fixture::new("no-packaging-call").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Macos, "my_app");
        let runner = FakeProcessRunner::new().missing("cargo packager --version");
        build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |_| {},
        )
        .unwrap_err();
        // If a `cargo packager --config ...` call had been made, the fake
        // runner (unregistered) would have errored, and `build_installer`
        // would surface `PackagerSpawn`, not `ToolMissing` — the assertion
        // above already proves it stopped at the version check.
    }

    /// Correct invocation per format: the generated config file, and the
    /// exact `cargo packager --config <path>` invocation, both match what
    /// this module promises for every one of the five formats.
    #[test]
    fn each_format_synthesizes_the_matching_config_and_invocation() {
        for (target, binary, format) in [
            (DesktopBundleTarget::Macos, "my_app", InstallerFormat::Dmg),
            (
                DesktopBundleTarget::Windows,
                "my_app.exe",
                InstallerFormat::NsisExe,
            ),
            (
                DesktopBundleTarget::Windows,
                "my_app.exe",
                InstallerFormat::WixMsi,
            ),
            (DesktopBundleTarget::Linux, "my_app", InstallerFormat::Deb),
            (
                DesktopBundleTarget::Linux,
                "my_app",
                InstallerFormat::AppImage,
            ),
        ] {
            let fixture = Fixture::new(&format!("invocation-{target}-{format}")).manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\
                     icon = \"assets/icon.png\"\n",
            );
            fs::create_dir_all(fixture.path("assets")).unwrap();
            fs::write(fixture.path("assets/icon.png"), b"not-really-a-png").unwrap();
            let bundle = fixture.bundle(target, binary);

            let expected_out_dir = target
                .dist_dir(&fixture.dir)
                .join("installer")
                .join(format.as_str());
            let expected_config_path = expected_out_dir.join("packager.json");
            let expected_config_arg = expected_config_path.to_string_lossy().into_owned();

            let runner = version_ok().with(
                format!("cargo packager --config {expected_config_arg}"),
                Output {
                    success: true,
                    stdout: "Finished packaging 1 package".to_string(),
                    stderr: String::new(),
                },
            );

            let report =
                build_installer(&runner, &fixture.dir, &info(), &bundle, format, &mut |_| {})
                    .unwrap_or_else(|err| panic!("{target}/{format}: {err}"));

            assert_eq!(report.format, format);
            assert_eq!(report.out_dir, expected_out_dir);

            let written = fs::read_to_string(&expected_config_path).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
            assert_eq!(parsed["name"], "my_app");
            assert_eq!(parsed["productName"], "My App");
            assert_eq!(parsed["identifier"], "dev.f0x.my_app");
            assert_eq!(parsed["formats"], serde_json::json!([format.as_str()]));
            assert_eq!(
                parsed["binaries"][0]["path"],
                bundle.executable.to_string_lossy().as_ref()
            );
            assert_eq!(parsed["binaries"][0]["main"], true);
            // The `macos` identity block is Dmg-only — every other format's
            // config carries no such key at all — and so is the icon rule:
            // `.dmg` takes the bundle's own `.icns` (this fixture bundle has
            // none, so no `icons` key at all), every other format the source
            // PNG.
            if format == InstallerFormat::Dmg {
                assert!(
                    parsed.get("icons").is_none(),
                    "{target}/{format}: {written}"
                );
                assert!(
                    parsed.get("macos").is_some(),
                    "{target}/{format}: {written}"
                );
            } else {
                assert_eq!(
                    parsed["icons"],
                    serde_json::json!([fixture.path("assets/icon.png").to_string_lossy()]),
                    "{target}/{format}: {written}"
                );
                assert!(
                    parsed.get("macos").is_none(),
                    "{target}/{format}: {written}"
                );
            }
        }
    }

    /// The defect this task fixes, made concrete: a `.dmg` build's config
    /// names the *assembled* bundle's own `Contents/Info.plist` and carries
    /// the resolved signing identity/entitlements, so `cargo-packager`'s
    /// independently synthesized `.app` (see this module's doc) still ends up
    /// signed with the project's identity instead of a stripped default.
    #[test]
    fn dmg_config_carries_the_macos_block_with_plist_signing_identity_and_entitlements() {
        let fixture = Fixture::new("dmg-macos-block").manifest(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\n\
             [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
        );
        fs::create_dir_all(fixture.path("macos")).unwrap();
        fs::write(
            fixture.path("macos/app.entitlements"),
            "<?xml version=\"1.0\"?><plist><dict/></plist>",
        )
        .unwrap();
        let bundle = fixture.bundle(DesktopBundleTarget::Macos, "my_app");
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let report = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();

        // An Apple-issued identity is wired through unchanged — the only note
        // is the default credential scrub (asserted in full by its own test
        // below), never a signing-skipped one.
        assert_eq!(report.notes, vec![InstallerNote::NotarizationSuppressed]);
        assert!(
            !report
                .notes
                .iter()
                .any(|note| matches!(note, InstallerNote::PackagerSigningSkipped { .. })),
            "{:?}",
            report.notes
        );

        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        let expected_plist = bundle
            .root
            .join("Contents")
            .join("Info.plist")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            parsed["macos"]["infoPlistPath"], expected_plist,
            "{written}"
        );
        assert_eq!(
            parsed["macos"]["signingIdentity"], "Developer ID Application: Example",
            "{written}"
        );
        assert_eq!(
            parsed["macos"]["entitlements"],
            fixture
                .path("macos/app.entitlements")
                .to_string_lossy()
                .as_ref(),
            "{written}"
        );
    }

    /// The offline guarantee, kept end-to-end: an identity Apple did not issue
    /// is never handed to `cargo-packager` (whose own codesign pass always
    /// passes `--timestamp`), so `signingIdentity` is absent and a typed note
    /// plus a `warning:` line say what that costs. The rest of the block —
    /// plist path, entitlements — is unaffected.
    #[test]
    fn dmg_config_suppresses_a_non_apple_issued_signing_identity_with_a_note() {
        let fixture = Fixture::new("dmg-adhoc-identity").manifest(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\n\
             [macos]\nsigning-identity = \"My Self Signed\"\n",
        );
        fs::create_dir_all(fixture.path("macos")).unwrap();
        fs::write(
            fixture.path("macos/app.entitlements"),
            "<?xml version=\"1.0\"?><plist><dict/></plist>",
        )
        .unwrap();
        let bundle = fixture.macos_app("my_app", true);
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let report = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();

        assert_eq!(
            report.notes,
            vec![InstallerNote::PackagerSigningSkipped {
                identity: "My Self Signed".to_string(),
            }]
        );
        let warning = lines
            .iter()
            .find(|l| l.starts_with("warning:"))
            .unwrap_or_else(|| panic!("expected a warning line: {lines:?}"));
        assert!(warning.contains("My Self Signed"), "{warning}");
        assert!(warning.contains("ships unsigned"), "{warning}");
        assert!(warning.contains("offline"), "{warning}");

        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert!(
            parsed["macos"].get("signingIdentity").is_none(),
            "{written}"
        );
        assert!(parsed["macos"].get("infoPlistPath").is_some(), "{written}");
        assert!(parsed["macos"].get("entitlements").is_some(), "{written}");
    }

    /// The default contract: a `.dmg` build that hands `cargo-packager` an
    /// Apple-issued identity runs the tool with **every** credential variable
    /// its notarization fallback reads removed from the child's environment,
    /// so an exported `APPLE_ID`/`APPLE_API_KEY` can never turn a local build
    /// into an upload to Apple. The note names the opt-in.
    #[test]
    fn a_signed_dmg_build_scrubs_every_apple_credential_var_by_default() {
        let fixture = Fixture::new("dmg-scrub-default").manifest(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\n\
             [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
        );
        let bundle = fixture.macos_app("my_app", true);
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let report = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();

        // The exact set, in full — a variable dropped from the const would let
        // the fallback resolve credentials again.
        assert_eq!(
            runner.recorded_env_removals(),
            Some(
                APPLE_CREDENTIAL_ENV_VARS
                    .iter()
                    .map(|key| key.to_string())
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(
            runner.recorded_env_removals().unwrap().len(),
            9,
            "the full set cargo-packager 0.11.8 reads"
        );

        assert_eq!(report.notes, vec![InstallerNote::NotarizationSuppressed]);
        let warning = lines
            .iter()
            .find(|l| l.starts_with("warning:"))
            .unwrap_or_else(|| panic!("expected a warning line: {lines:?}"));
        assert!(warning.contains("NOT notarized"), "{warning}");
        assert!(warning.contains("notarize = true"), "{warning}");
    }

    /// The opt-in: `[macos] notarize = true` removes nothing, so the packaging
    /// child inherits the credentials and `cargo-packager` may upload the
    /// signed app — which the note says out loud, since a build that contacts
    /// Apple and can fail on Apple's verdict is not what `frust build` does by
    /// default.
    #[test]
    fn notarize_true_passes_the_credentials_through_and_warns_that_it_uploads() {
        let fixture = Fixture::new("dmg-notarize-optin").manifest(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\n\
             [macos]\nsigning-identity = \"Developer ID Application: Example\"\n\
             notarize = true\n",
        );
        let bundle = fixture.macos_app("my_app", true);
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let report = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();

        // The packaging call was made, and removed nothing — not "no call".
        assert_eq!(runner.recorded_env_removals(), Some(Vec::new()));
        assert_eq!(report.notes, vec![InstallerNote::NotarizationEnabled]);
        let warning = lines
            .iter()
            .find(|l| l.starts_with("warning:"))
            .unwrap_or_else(|| panic!("expected a warning line: {lines:?}"));
        assert!(warning.contains("UPLOADS"), "{warning}");
        assert!(warning.contains("notary service"), "{warning}");
        assert!(warning.contains("fails this build"), "{warning}");
        // The signing identity still reaches the config — the opt-in changes
        // the child's environment, nothing about the config itself.
        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            parsed["macos"]["signingIdentity"], "Developer ID Application: Example",
            "{written}"
        );
    }

    /// A build `cargo-packager` never codesigns never reaches its notarization
    /// branch either, so neither note is emitted — for a non-Apple identity
    /// (whose `signingIdentity` is suppressed) and for a format with no
    /// `macos` block at all. The scrub still applies: it costs nothing, and
    /// keeps one rule rather than a per-format exception.
    #[test]
    fn a_build_that_cannot_notarize_gets_no_notarization_note_but_is_still_scrubbed() {
        for (target, binary, format, manifest_body) in [
            (
                DesktopBundleTarget::Macos,
                "my_app",
                InstallerFormat::Dmg,
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [macos]\nsigning-identity = \"My Self Signed\"\n",
            ),
            (
                DesktopBundleTarget::Linux,
                "my_app",
                InstallerFormat::Deb,
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
            ),
        ] {
            let fixture =
                Fixture::new(&format!("no-notarize-note-{format}")).manifest(manifest_body);
            let bundle = fixture.bundle(target, binary);
            let out_dir = target
                .dist_dir(&fixture.dir)
                .join("installer")
                .join(format.as_str());
            let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
            let runner = version_ok().with(
                format!("cargo packager --config {config_arg}"),
                Output {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            );

            let report =
                build_installer(&runner, &fixture.dir, &info(), &bundle, format, &mut |_| {})
                    .unwrap_or_else(|err| panic!("{format}: {err}"));

            assert!(
                !report.notes.iter().any(|note| matches!(
                    note,
                    InstallerNote::NotarizationSuppressed | InstallerNote::NotarizationEnabled
                )),
                "{format}: {:?}",
                report.notes
            );
            assert_eq!(
                runner.recorded_env_removals(),
                Some(
                    APPLE_CREDENTIAL_ENV_VARS
                        .iter()
                        .map(|key| key.to_string())
                        .collect::<Vec<_>>()
                ),
                "{format}"
            );
        }
    }

    /// The icon defect this task fixes: the `.dmg` config names the *bundle's
    /// own* `.icns`, whose file name `cargo-packager`'s `create_icns_file`
    /// keeps verbatim — so the `CFBundleIconFile` its merged plist ends up with
    /// (ours, since our keys overwrite its own) actually resolves. Asserted as
    /// the agreement itself: the icons entry's file stem equals the plist's
    /// `CFBundleIconFile` value.
    #[test]
    fn dmg_config_icons_entry_agrees_with_the_plists_icon_key() {
        let fixture = Fixture::new("dmg-icns").manifest(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\
             icon = \"assets/icon.png\"\n",
        );
        fs::create_dir_all(fixture.path("assets")).unwrap();
        fs::write(fixture.path("assets/icon.png"), b"not-really-a-png").unwrap();
        let bundle = fixture.macos_app("my_app", true);
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |_| {},
        )
        .unwrap();

        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        let icons = parsed["icons"].as_array().expect(&written);
        assert_eq!(icons.len(), 1, "{written}");
        let icon_path = PathBuf::from(icons[0].as_str().unwrap());
        // Never the source PNG — the `.icns` the assembly wrote.
        assert_eq!(icon_path.extension().unwrap(), "icns", "{written}");
        assert_eq!(
            icon_path,
            bundle
                .root
                .join("Contents")
                .join("Resources")
                .join("my_app.icns"),
            "{written}"
        );

        let plist = fs::read_to_string(PathBuf::from(
            parsed["macos"]["infoPlistPath"].as_str().expect(&written),
        ))
        .unwrap();
        let declared = plist_string(&plist, "CFBundleIconFile")
            .unwrap_or_else(|| panic!("no CFBundleIconFile in the fixture plist:\n{plist}"));
        // Apple accepts the name with or without the extension; the stems are
        // what have to agree.
        let declared_stem = declared.strip_suffix(".icns").unwrap_or(&declared);
        assert_eq!(
            icon_path.file_stem().unwrap().to_string_lossy(),
            declared_stem,
            "{written}"
        );
    }

    /// The other half: an assembly whose icon steps were skipped or rejected
    /// wrote no `.icns`, so the `.dmg` config carries no `icons` entry at all —
    /// deliberately *not* the source PNG, which `create_icns_file` would repack
    /// under the display name and leave the merged `CFBundleIconFile` dangling.
    #[test]
    fn dmg_config_omits_icons_when_the_bundle_has_no_icns() {
        let fixture = Fixture::new("dmg-no-icns").manifest(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"dev.f0x.my_app\"\n\
             icon = \"assets/icon.png\"\n",
        );
        fs::create_dir_all(fixture.path("assets")).unwrap();
        fs::write(fixture.path("assets/icon.png"), b"not-really-a-png").unwrap();
        let bundle = fixture.macos_app("my_app", false);
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |_| {},
        )
        .unwrap();

        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert!(parsed.get("icons").is_none(), "{written}");
    }

    /// The negative half: no `[macos] signing-identity` and no
    /// `macos/app.entitlements` in the project. The block still carries the
    /// plist path (`cargo-packager` needs it regardless of signing), but
    /// neither optional key is written at all — never a JSON `null`.
    #[test]
    fn dmg_config_omits_signing_identity_and_entitlements_when_unconfigured() {
        let fixture = Fixture::new("dmg-macos-block-unsigned").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Macos, "my_app");
        let out_dir = DesktopBundleTarget::Macos
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("dmg");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Dmg,
            &mut |_| {},
        )
        .unwrap();

        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        let macos = parsed
            .get("macos")
            .unwrap_or_else(|| panic!("expected a `macos` block: {written}"));
        assert!(macos.get("infoPlistPath").is_some(), "{written}");
        assert!(macos.get("signingIdentity").is_none(), "{written}");
        assert!(macos.get("entitlements").is_none(), "{written}");
    }

    /// Restates the loop assertion above as its own named test, over every
    /// non-Dmg format, so a future reader searching for "no macos block" finds
    /// it without cross-referencing the parametrized loop.
    #[test]
    fn non_dmg_formats_carry_no_macos_block() {
        for (target, binary, format) in [
            (
                DesktopBundleTarget::Windows,
                "my_app.exe",
                InstallerFormat::NsisExe,
            ),
            (
                DesktopBundleTarget::Windows,
                "my_app.exe",
                InstallerFormat::WixMsi,
            ),
            (DesktopBundleTarget::Linux, "my_app", InstallerFormat::Deb),
            (
                DesktopBundleTarget::Linux,
                "my_app",
                InstallerFormat::AppImage,
            ),
        ] {
            let fixture = Fixture::new(&format!("no-macos-block-{format}")).manifest(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                     [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
            );
            let bundle = fixture.bundle(target, binary);
            let out_dir = target
                .dist_dir(&fixture.dir)
                .join("installer")
                .join(format.as_str());
            let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
            let runner = version_ok().with(
                format!("cargo packager --config {config_arg}"),
                Output {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            );

            build_installer(&runner, &fixture.dir, &info(), &bundle, format, &mut |_| {})
                .unwrap_or_else(|err| panic!("{format}: {err}"));

            let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
            assert!(parsed.get("macos").is_none(), "{format}: {written}");
        }
    }

    #[test]
    fn an_unconfigured_icon_is_omitted_from_the_config_rather_than_null() {
        let fixture = Fixture::new("no-icon").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Linux, "my_app");
        let out_dir = DesktopBundleTarget::Linux
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("deb");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );

        build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Deb,
            &mut |_| {},
        )
        .unwrap();

        let written = fs::read_to_string(out_dir.join("packager.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert!(parsed.get("icons").is_none(), "{written}");
    }

    #[test]
    fn packager_output_streams_through_the_on_line_sink() {
        let fixture = Fixture::new("on-line").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Linux, "my_app");
        let out_dir = DesktopBundleTarget::Linux
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("deb");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: true,
                stdout: "Packaging my_app_0.1.0_amd64.deb\nFinished packaging 1 package"
                    .to_string(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::Deb,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();

        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[cargo-packager] Packaging")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("installer: ")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_failed_packaging_invocation_surfaces_the_output_tail() {
        let fixture = Fixture::new("packaging-failed").default_manifest();
        let bundle = fixture.bundle(DesktopBundleTarget::Linux, "my_app");
        let out_dir = DesktopBundleTarget::Linux
            .dist_dir(&fixture.dir)
            .join("installer")
            .join("appimage");
        let config_arg = out_dir.join("packager.json").to_string_lossy().into_owned();
        let runner = version_ok().with(
            format!("cargo packager --config {config_arg}"),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "ERROR Could not find a square icon to use as AppImage icon".to_string(),
            },
        );

        let err = build_installer(
            &runner,
            &fixture.dir,
            &info(),
            &bundle,
            InstallerFormat::AppImage,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(err, InstallerError::PackagerFailed { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("square icon"), "{err}");
    }

    #[test]
    fn discover_artifacts_matches_the_formats_extension_case_insensitively() {
        let dir = temp_dir("discover");
        fs::write(dir.join("app_0.1.0_x86_64.AppImage"), b"x").unwrap();
        fs::write(dir.join("stale.appimage"), b"x").unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        fs::create_dir_all(dir.join("nested.AppImage")).unwrap();

        let mut found = discover_artifacts(&dir, InstallerFormat::AppImage);
        found.sort();
        assert_eq!(
            found,
            vec![
                dir.join("app_0.1.0_x86_64.AppImage"),
                dir.join("stale.appimage"),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_artifacts_on_a_missing_directory_is_empty_not_an_error() {
        let dir = std::env::temp_dir().join("frust-drive-installer-does-not-exist");
        assert!(discover_artifacts(&dir, InstallerFormat::Deb).is_empty());
    }
}
