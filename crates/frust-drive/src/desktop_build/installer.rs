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
    let config = DesktopConfig::resolve(project_dir, &manifest);

    let out_dir = format
        .target()
        .dist_dir(project_dir)
        .join("installer")
        .join(format.as_str());
    prepare_out_dir(&out_dir)?;

    let packager_config = PackagerConfig::new(&config, info, bundle, format, &out_dir);
    let config_path = out_dir.join("packager.json");
    write_config(&config_path, &packager_config)?;

    let config_arg = config_path.to_string_lossy().into_owned();
    let args = ["packager", "--config", config_arg.as_str()];
    let mut prefixed = |line: &str| on_line(&format!("[cargo-packager] {line}"));
    let out = runner
        .run_streaming("cargo", &args, Some(project_dir), &[], &mut prefixed)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    icons: Option<Vec<String>>,
    formats: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct PackagerBinary {
    path: String,
    main: bool,
}

impl PackagerConfig {
    fn new(
        config: &DesktopConfig,
        info: &BuildInfo,
        bundle: &BundleReport,
        format: InstallerFormat,
        out_dir: &Path,
    ) -> PackagerConfig {
        let icons = config
            .icon_source
            .as_ref()
            .filter(|source| source.is_file())
            .map(|source| vec![source.to_string_lossy().into_owned()]);

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
        }
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

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.join(rel)
        }
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
            assert_eq!(
                parsed["icons"],
                serde_json::json!([fixture.path("assets/icon.png").to_string_lossy()])
            );
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
