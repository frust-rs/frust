//! Desktop bundle assembly: `cargo build` plus pure directory/file work into
//! the per-OS layout a user can actually double-click or ship.
//!
//! Three targets, one pipeline ([`build`]): resolve `frust.toml`'s desktop
//! identity, compile the app through [`crate::process::ProcessRunner`], locate
//! the produced binary, and lay out
//!
//! - macOS — `dist/macos/<Display Name>.app/Contents/{MacOS,Resources}` +
//!   `Info.plist`, optionally `codesign`ed;
//! - Windows — `dist/windows/<binary>.exe` (the icon is embedded by the
//!   project's own `windows/build.rs`, which this pipeline feeds by generating
//!   `windows/icon.ico` *before* the compile);
//! - Linux — `dist/linux/<binary>/` with the binary, a `<identifier>.desktop`
//!   entry and a `share/icons/hicolor/` tree.
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
//! **A missing or unusable icon never fails a build.** The bundle is assembled
//! without one and the reason comes back as a [`BundleNote`] — a bundle with
//! no icon still runs, and the scaffold's own placeholder logo is deliberately
//! below the icon pipeline's minimum source size.

mod bundle;
mod cargo;
mod config;
mod linux;
mod macos;
mod windows;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::build_info::BuildInfo;
use crate::doctor::{EnvLookup, RealEnv};
use crate::manifest;
use crate::process::ProcessRunner;

use self::config::DesktopConfig;

/// The bundle `frust build macos|windows|linux` requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopBundleTarget {
    Macos,
    Windows,
    Linux,
}

impl DesktopBundleTarget {
    /// The lowercase target name — the `frust build <name>` subcommand word,
    /// and the `dist/<name>/` output directory segment.
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

    /// The per-OS output root under the project: `dist/macos`, `dist/windows`,
    /// `dist/linux`. The bundle itself lands inside it (see
    /// [`BundleReport::root`]).
    pub fn dist_dir(self, project_dir: &Path) -> PathBuf {
        project_dir.join("dist").join(self.as_str())
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
    /// The bundle root: the `.app` directory (macOS), `dist/windows`
    /// (Windows), or `dist/linux/<binary>` (Linux).
    pub root: PathBuf,
    /// The executable inside the bundle — what a launcher entry or a `dmg`
    /// packaging step points at.
    pub executable: PathBuf,
    /// Every file this run wrote, in write order (executable, icon
    /// container(s), launcher metadata). Directories are not listed.
    pub artifacts: Vec<PathBuf>,
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
    build_with_host(
        runner,
        &RealEnv,
        project_dir,
        info,
        target,
        DesktopBundleTarget::host(),
        on_line,
    )
}

/// The testable core of [`build`]: `host` is injected (rather than read from
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
    if host != Some(target) {
        return Err(DesktopBuildError::HostMismatch { target, host });
    }

    let manifest = manifest::load(project_dir).map_err(|err| DesktopBuildError::Manifest {
        project_dir: project_dir.to_path_buf(),
        reason: format!("{err:#}"),
    })?;
    let config = DesktopConfig::resolve(project_dir, &manifest);

    let mut notes = Vec::new();
    // Windows only: the project's own `windows/build.rs` embeds
    // `windows/icon.ico` into the `.exe`, so the icon has to exist BEFORE the
    // compile — every other target's icon work happens at assembly time.
    let prebuilt = match target {
        DesktopBundleTarget::Windows => {
            windows::generate_exe_icon(project_dir, &config, &mut notes)
        }
        _ => None,
    };

    cargo::build(runner, project_dir, info, on_line)?;
    let binary = cargo::locate_binary(runner, env, project_dir, info, target, &config)?;

    let mut report = match target {
        DesktopBundleTarget::Macos => {
            macos::assemble(project_dir, info, &config, &binary, &mut notes)?
        }
        DesktopBundleTarget::Windows => {
            windows::assemble(project_dir, &config, &binary, prebuilt, &mut notes)?
        }
        DesktopBundleTarget::Linux => linux::assemble(project_dir, &config, &binary, &mut notes)?,
    };

    if target == DesktopBundleTarget::Macos {
        macos::codesign(
            runner,
            project_dir,
            &config,
            &report.root,
            &mut notes,
            on_line,
        )?;
    }

    // Notes are collected across every stage (pre-build icon, assembly,
    // codesign), so the assemblers leave the field empty and it is filled once
    // here — the report is only complete after the last stage has run.
    report.notes = notes;
    on_line(&format!("bundle: {}", report.root.display()));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::doctor::FakeEnv;
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
            let path = self.dir.join(rel);
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

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.join(rel)
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.dir.join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
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

        assert_eq!(report.root, fixture.path("dist/linux/my_app"));
        assert_eq!(report.executable, fixture.path("dist/linux/my_app/my_app"));
        assert!(report.executable.is_file());

        // The project's own entry, copied verbatim under the identifier name
        // a `.desktop` install expects.
        let entry = fixture.read("dist/linux/my_app/dev.f0x.my_app.desktop");
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
                "dist/linux/my_app/share/icons/hicolor/{size}x{size}/apps/dev.f0x.my_app.png"
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
        let entry = fixture.read("dist/linux/my_app/dev.f0x.my_app.desktop");
        assert!(entry.contains("Name=my_app"), "{entry}");
        assert!(entry.contains("Exec=my_app"), "{entry}");
        assert!(entry.contains("Icon=dev.f0x.my_app"), "{entry}");
        assert!(entry.contains("Categories=Graphics;Utility;"), "{entry}");
        assert!(report.notes.contains(&BundleNote::IconNotConfigured));
    }

    #[test]
    fn macos_assembly_copies_the_projects_info_plist_and_names_the_app_bundle() {
        let fixture = Fixture::new("macos-full")
            .default_manifest()
            .icon(1024)
            .binary("my_app")
            .file(
                "macos/Info.plist",
                "<plist><dict><key>CFBundleExecutable</key><string>my_app</string></dict></plist>",
            );

        let report = run(&cargo_ok(), &fixture, DesktopBundleTarget::Macos).unwrap();

        assert_eq!(report.root, fixture.path("dist/macos/My App.app"));
        assert_eq!(
            report.executable,
            fixture.path("dist/macos/My App.app/Contents/MacOS/my_app")
        );
        assert!(report.executable.is_file());
        assert!(
            fixture
                .path("dist/macos/My App.app/Contents/Resources/my_app.icns")
                .is_file()
        );
        let plist = fixture.read("dist/macos/My App.app/Contents/Info.plist");
        assert!(plist.contains("CFBundleExecutable"), "{plist}");
        assert!(!report.notes.contains(&BundleNote::GeneratedInfoPlist));
        assert!(report.notes.contains(&BundleNote::Unsigned));
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
        let plist = fixture.read("dist/macos/My App.app/Contents/Info.plist");
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

        let app = fixture.path("dist/macos/My App.app");
        let entitlements = fixture.path("macos/app.entitlements");
        let runner = cargo_ok().with(
            format!(
                "codesign --force --sign Developer ID Application: Example --entitlements {} {}",
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

        assert_eq!(report.root, fixture.path("dist/windows"));
        assert_eq!(report.executable, fixture.path("dist/windows/my_app.exe"));
        assert!(report.executable.is_file());
        // The `.ico` lands in the PROJECT (where `windows/build.rs` reads it),
        // not in the dist dir — and is reported as an artifact all the same.
        let ico = fixture.path("windows/icon.ico");
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
        assert!(fixture.path("windows/icon.ico").is_file());
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
        assert!(!fixture.path("dist/linux/my_app/share").exists());
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
                .path("dist/linux/my_app/share/icons/hicolor/512x512/apps/dev.f0x.my_app.png")
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
        let stale = fixture.path("dist/linux/my_app/stale-from-a-previous-run");
        fs::write(&stale, b"x").unwrap();

        run(&cargo_ok(), &fixture, DesktopBundleTarget::Linux).unwrap();
        assert!(!stale.exists());
        assert!(fixture.path("dist/linux/my_app/my_app").is_file());
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

        assert_eq!(report.root, fixture.path("dist/linux/my_app"));
        assert!(report.executable.is_file());
        assert!(
            fixture
                .path("dist/linux/my_app/dev.f0x.my_app.desktop")
                .is_file()
        );
        assert!(
            fixture
                .path("dist/linux/my_app/share/icons/hicolor/256x256/apps/dev.f0x.my_app.png")
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
    /// (winit/vello/wgpu), and a release build of it would multiply an already
    /// long compile for no extra coverage of anything this module owns.
    #[test]
    #[ignore = "compiles a scaffolded project's full dependency graph (winit/vello/wgpu); run with `cargo test -p frust-drive --lib -- --ignored scaffolded_linux_bundle`"]
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

        assert_eq!(report.root, fixture.path("dist/linux/my_app"));
        assert!(report.executable.is_file());
        // The scaffolded entry is copied verbatim, under the identifier name.
        let entry = fixture.read("dist/linux/my_app/dev.f0x.my_app.desktop");
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
