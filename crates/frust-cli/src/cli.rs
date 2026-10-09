//! Top-level clap parser, modeled on `flutter_tools`' command surface.

use clap::{Parser, Subcommand};

use crate::build_args::BuildArgs;

#[derive(Parser, Debug)]
#[command(name = "frust", version, about = "Tooling for Frust apps")]
pub struct Cli {
    /// Target device id or name (prefix match allowed).
    #[arg(short = 'd', long = "device-id", global = true, value_name = "ID")]
    pub device_id: Option<String>,

    /// Increase verbosity; repeatable (-v, -vv, …).
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Subcommand to run. With none given, `frust` opens the TUI workbench
    /// when both stdin and stdout are terminals; otherwise it prints this
    /// help and exits 2 (see `main.rs`'s `default_command` and the `None`
    /// branch in `main` that resolves this field before ever calling
    /// `commands::dispatch`).
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scaffold a new app (v1 generates a desktop-preview
    /// Rust crate; `android/`/`ios/` platform projects land alongside it).
    Create {
        /// Target directory for the new project (may be `.`).
        dir: String,

        /// Reverse-DNS organization identifier, e.g. `dev.f0x`.
        #[arg(long, default_value = "com.example")]
        org: String,

        /// Project name; defaults to the target directory's basename.
        #[arg(long = "project-name", value_name = "NAME")]
        project_name: Option<String>,

        /// One-line project description.
        #[arg(long, default_value = "A new Frust application.")]
        description: String,

        /// Overwrite a non-empty target directory.
        #[arg(long)]
        overwrite: bool,

        /// Override the embedded template directory (development only).
        #[arg(long = "template-dir", value_name = "PATH", hide = true)]
        template_dir: Option<String>,

        /// Depend on this frust checkout by path instead of the crates.io
        /// release (framework development).
        ///
        /// Accepts either the facade crate itself (a directory whose
        /// Cargo.toml names package `frust-ui` or `frust`, e.g.
        /// `<repo>/crates/frust`) or that repo's root (e.g. `<repo>`), which
        /// is normalised to the nested facade crate directory; anything else
        /// is rejected.
        #[arg(long = "frust-path", value_name = "PATH")]
        frust_path: Option<String>,

        /// URL scheme to register for deep links (e.g. `myapp`, no `://`) —
        /// generates a Android `<intent-filter>` (VIEW/BROWSABLE/DEFAULT)
        /// and an iOS `CFBundleURLTypes` entry. Omit to generate a
        /// project with no deep-link config (the default).
        #[arg(long = "deeplink-scheme", value_name = "SCHEME")]
        deeplink_scheme: Option<String>,

        /// Optional host restricting the Android deep-link intent-filter
        /// (`android:host`). Only meaningful alongside `--deeplink-scheme`;
        /// iOS's `CFBundleURLTypes` has no host concept.
        #[arg(long = "deeplink-host", value_name = "HOST")]
        deeplink_host: Option<String>,

        /// Opt-in clean-architecture variant: scaffolds a controller +
        /// use-case + `async_view` screen wired to `clean-signals-frust`
        /// instead of the default counter app.
        /// For framework development, `--frust-path <checkout>` emits path
        /// dependencies for `frust` and the in-repo `clean-signals-frust`
        /// plugin (the project won't build without this checkout present).
        /// Otherwise `Cargo.toml` emits registry dependencies on the
        /// published `frust` and `clean-signals-frust` crates (no checkout
        /// required). `clean-signals` is a crates.io dependency with no
        /// checkout requirement.
        #[arg(long = "arch", value_name = "ARCH")]
        arch: Option<ArchArg>,

        // `--platforms` and `--no-sync`: which platform projects to render,
        // and whether to wire them to the resolved shell crates afterwards.
        #[command(flatten)]
        platforms: CreatePlatformArgs,

        /// Scaffold an out-of-tree **design-system** crate instead of an
        /// app: a themed widget catalog crate depending on nothing but
        /// `frust`, with no platform (`android`/`ios`) project. `dir` is
        /// still the target directory and `--project-name` still overrides
        /// the inferred crate name (also the design system's identity tag);
        /// `--overwrite`/`--template-dir`/`--frust-path` still apply.
        /// `--org`/`--description` are unused (a design-system crate has no
        /// bundle identifier or app manifest); `--deeplink-scheme`/
        /// `--deeplink-host`/`--arch` are rejected outright rather than
        /// silently ignored, since a plain library crate has no deep-link
        /// config or app-architecture variant for them to apply to.
        #[arg(long = "design-system")]
        design_system: bool,
    },
    /// Validate the Frust toolchain (Rust targets, NDK, Android SDK, Xcode).
    Doctor,
    /// List connected devices, emulators, and simulators.
    Devices,
    /// Remove build outputs (cargo target dirs + Gradle/Xcode build dirs).
    Clean,
    /// Launch the TUI workbench: an advanced interactive interface for
    /// managing Frust projects.
    Tui,
    /// Build → install → launch → stream logs on a connected device;
    /// no Android device selected → `cargo run` passthrough; `-d web`
    /// builds for the browser and serves the artifact directory instead.
    Run {
        #[command(flatten)]
        build: BuildFlags,

        /// Dev loop for the project in the current directory: the desktop
        /// preview, or an Android device or a booted iOS simulator with
        /// `-d <serial|udid>`.
        /// A Debug run is **hot** by default: the app is fat-built once,
        /// and each save is compiled and patched into the running process
        /// (state kept), printing `patched in N ms (k components rebuilt)`.
        /// A change that cannot be patched (a framework or path-dependency
        /// edit, a manifest or build-script change, a layout or state-type
        /// change) prints `restart required: <reason>` and relaunches a
        /// fresh app (on a device, the full install-and-launch pipeline); a
        /// compile error prints its diagnostics and leaves the running app
        /// untouched. Profile/release runs, and `--no-hot`, use the plain
        /// relaunch loop instead: any change to `src/` or `Cargo.toml`
        /// kills the running app and relaunches it (app state resets).
        /// With `-d` the device must be Android or an iOS simulator and the
        /// build Debug: `-d web`, a physical iOS device and every other
        /// device are refused, as are `--profile`/`--release` and
        /// `--features` with a device. Ctrl-C removes the port forward and
        /// force-stops an Android app, and terminates a simulator app.
        #[arg(long)]
        watch: bool,

        /// With `--watch`, use the plain kill-and-relaunch loop instead of
        /// hot patching, on desktop (the `cargo run` child), on Android
        /// (`am force-stop` plus the full device pipeline on every change)
        /// and on an iOS simulator (`simctl terminate` plus the full
        /// simulator pipeline).
        /// A no-op without `--watch`: nothing else in `run` is hot.
        #[arg(long = "no-hot")]
        no_hot: bool,

        /// With `-d web`, skip opening a browser at the served URL once the
        /// dev server is up. Ignored for every other target — no other
        /// `run` lane ever opens one. The dev server always binds and prints
        /// its URL either way; this only controls the automatic
        /// `xdg-open`/`open`/`start` launch.
        #[arg(long = "no-open")]
        no_open: bool,
    },
    /// Produce a distributable artifact — release-signed
    /// APK/AAB via Gradle, or an iOS app/IPA via `xcodebuild`. Defaults to
    /// release mode (unlike `run`, which defaults to debug).
    Build {
        #[command(subcommand)]
        target: BuildTarget,
    },
}

/// `frust create`'s platform options: which platform projects the scaffold
/// renders (`--platforms`), and whether the generated `android/`/`ios/`
/// projects are then pointed at the embedding modules inside the
/// `frust-shell-android`/`frust-shell-ios` crates cargo resolves
/// (`--no-sync` skips that). Grouped because the second acts on the first's
/// output.
#[derive(clap::Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct CreatePlatformArgs {
    /// Comma-separated list of target platforms to include in the
    /// scaffold (e.g. `android,ios,macos,windows,linux,web`). Accepted
    /// values are the platform tags from `scaffold::known_platform_tags()`;
    /// unknown tags are rejected. Omit to use the default platform set
    /// (android, ios, macos, windows, linux — all except web), which
    /// preserves byte-identical output for existing projects. Including
    /// `web` adds a `web/` host page alongside the native platform
    /// projects; subsequent `frust build web` produces a browser-runnable
    /// artifact in `build/web`.
    #[arg(long = "platforms", value_name = "LIST")]
    pub list: Option<String>,

    /// Skip wiring the generated Android/iOS projects to the frust shell
    /// crates (the offline path). Wiring runs `cargo metadata`, which needs
    /// network access once for a project on the published crates; without
    /// it, `frust run` / `frust build` for Android or iOS do the wiring
    /// later, before they invoke Gradle or Xcode.
    #[arg(long = "no-sync")]
    pub no_sync: bool,
}

/// The artifact `frust build` produces. Kept off
/// [`crate::build_info::BuildInfo`] — artifact selection is orthogonal to
/// the mode/flavor/defines/version funnel `run` and `build` share.
#[derive(Subcommand, Debug)]
pub enum BuildTarget {
    /// Release-signed APK via Gradle.
    Apk {
        #[command(flatten)]
        build: BuildFlags,

        /// Produce one APK per target ABI instead of a single fat APK.
        #[arg(long = "split-per-abi")]
        split_per_abi: bool,

        /// Comma-separated target ABIs (`android-arm64`, `android-arm`,
        /// `android-x64`); defaults to all three.
        #[arg(long = "target-platform", value_name = "CSV")]
        target_platform: Option<String>,
    },
    /// AAB for Play Store via Gradle.
    #[command(alias = "aab")]
    Appbundle {
        #[command(flatten)]
        build: BuildFlags,

        /// Comma-separated target ABIs (`android-arm64`, `android-arm`,
        /// `android-x64`); defaults to all three.
        #[arg(long = "target-platform", value_name = "CSV")]
        target_platform: Option<String>,
    },
    /// iOS device/simulator build via `xcodebuild` (macOS host only).
    Ios {
        #[command(flatten)]
        build: BuildFlags,

        /// Build for the iOS Simulator instead of a physical device.
        #[arg(long)]
        simulator: bool,

        /// Skip code signing (`CODE_SIGNING_ALLOWED=NO`).
        #[arg(long = "no-codesign")]
        no_codesign: bool,
    },
    /// Archive + `-exportArchive` for App Store/ad-hoc/enterprise
    /// distribution (macOS host only).
    Ipa {
        #[command(flatten)]
        build: BuildFlags,

        /// Export method: `app-store-connect`, `release-testing`,
        /// `debugging`, or `enterprise`.
        #[arg(long = "export-method", value_name = "METHOD")]
        export_method: String,
    },
    /// macOS `.app` bundle via `cargo build` (macOS host only — desktop
    /// targets are host-locked, like every other desktop toolchain: a
    /// `.app`/`.exe`/Linux bundle can only be assembled on that same OS).
    /// Defaults to release mode, like every other `frust build` target. Code
    /// signing is config-driven (`[macos] signing-identity` in
    /// `frust.toml`), not a flag — there is no `--no-codesign` here, unlike
    /// `build ios`.
    Macos {
        #[command(flatten)]
        build: BuildFlags,

        /// Also build the platform's installer set over the assembled
        /// bundle — a `.dmg` on macOS — via a pinned `cargo-packager`
        /// (`docs/CLI_DEVELOPMENT.md`'s Version Pins).
        #[arg(long)]
        installer: bool,
    },
    /// Windows `.exe` bundle via `cargo build` (Windows host only — see
    /// `Macos`'s doc comment for the host-lock rule this shares). Defaults
    /// to release mode, like every other `frust build` target.
    Windows {
        #[command(flatten)]
        build: BuildFlags,

        /// Also build the platform's installer set over the assembled
        /// bundle — an NSIS `.exe` and a WiX `.msi` on Windows — via a
        /// pinned `cargo-packager` (`docs/CLI_DEVELOPMENT.md`'s Version
        /// Pins).
        #[arg(long)]
        installer: bool,
    },
    /// Linux bundle (binary + `.desktop` entry + hicolor icon tree) via
    /// `cargo build` (Linux host only — see `Macos`'s doc comment for the
    /// host-lock rule this shares). Defaults to release mode, like every
    /// other `frust build` target.
    Linux {
        #[command(flatten)]
        build: BuildFlags,

        /// Also build the platform's installer set over the assembled
        /// bundle — a `.deb` and an `.AppImage` on Linux — via a pinned
        /// `cargo-packager` (`docs/CLI_DEVELOPMENT.md`'s Version Pins).
        #[arg(long)]
        installer: bool,
    },
    /// Browser build: `cargo build --target wasm32-unknown-unknown` +
    /// `wasm-bindgen --target web` + an optional `wasm-opt` pass, producing
    /// the servable artifact directory `frust run -d web` (or any static
    /// host) serves. No host lock — unlike `macos`/`windows`/`linux`, every
    /// desktop OS can cross-compile to `wasm32-unknown-unknown`. Defaults to
    /// release mode, like every other `frust build` target; `--features` is
    /// not yet plumbed through the browser pipeline (see
    /// `commands::build::reject_unplumbed_features`).
    Web {
        #[command(flatten)]
        build: BuildFlags,
    },
}

/// The whole flag surface a build-producing subcommand exposes: the shared
/// mode/flavor/defines/version funnel plus the app cargo-feature passthrough.
///
/// A wrapper around [`BuildArgs`] rather than two more fields on it, because
/// that struct mirrors [`frust_drive::build_info::BuildArgs`] field-for-field
/// and converts into it — and `--features` is deliberately outside that
/// funnel. A `BuildInfo` describes *what* is being built (mode, flavor,
/// defines, version); these features are appended to whatever
/// `BuildMode::cargo_features` already selected, at each platform's own
/// cargo-argv site, and never reach the funnel's validation.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildFlags {
    #[command(flatten)]
    pub build: BuildArgs,

    /// Extra cargo features to compile the app with, on top of the ones the
    /// build mode already selects (`frust/perf-trace`+`frust/devtools` for
    /// debug/profile, `lean` for release). Repeatable, and a single value may
    /// itself be a space- or comma-separated list, matching cargo's own
    /// `--features` syntax. Strictly additive — it never replaces the mode's
    /// selection, it is appended after it, so a `--profile` build keeps its
    /// instrumentation. Reaches the app's own `[features]` table, so a name
    /// the app does not declare is cargo's error to report, not this CLI's.
    /// On `frust build`'s four release lanes (`apk`/`appbundle`/`ios`/`ipa`)
    /// every token is additionally charset-validated
    /// ([`feature_token_charset_ok`]) and refused outright if it would
    /// enable the in-app devtools listener
    /// (`commands::build::refuse_devtools_features`) — see
    /// `docs/DEVTOOLS_ARCHITECTURE.md`'s Trust model; `run` is unaffected.
    #[arg(long = "features", value_name = "FEATURES")]
    pub features: Vec<String>,
}

impl BuildFlags {
    /// The passthrough features as one flat list: every `--features`
    /// occurrence split on commas and whitespace (cargo's own accepted
    /// syntax, so `--features "a,b"`, `--features "a b"` and `--features a
    /// --features b` are equivalent), trimmed, empty tokens dropped, order
    /// preserved. Splitting here rather than downstream keeps every funnel
    /// site handling one feature per element, which is what the Android
    /// `-Pfrust.cargoFeatures` CSV and the iOS `FRUST_FEATURES` CSV both need.
    ///
    /// Deliberately infallible and unvalidated — every existing caller
    /// (including `frust run`, which this task leaves unchanged) depends on
    /// a plain `Vec<String>` here. [`validate_feature_token_charset`] is the
    /// separate, opt-in check `frust build`'s release lanes run over this
    /// result before forwarding it anywhere.
    pub fn extra_features(&self) -> Vec<String> {
        self.features
            .iter()
            .flat_map(|spec| spec.split([',', ' ', '\t', '\n']))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// Whether a single `--features` passthrough token matches the strict
/// charset a release lane's own cargo-argv site requires: cargo's own
/// bare-feature-name charset (`[A-Za-z0-9_.-]+`), optionally prefixed by one
/// `<pkg>/` segment of the same charset — cargo's `pkg/feature`
/// conditional-dependency-feature syntax. Anything else (in particular a
/// shell metacharacter) is rejected: a validated CSV of these tokens is
/// still base64-decoded and re-spliced, **unquoted**, into a shell command
/// by the iOS release lane's own build phase
/// (`crates/frust-drive/templates/app/ios.tmpl/Runner.xcodeproj/project.pbxproj.tmpl`'s `cargo
/// build ... $CARGO_FEATURES`), so a token carrying a shell metacharacter
/// that reached that far would be a shell-injection primitive, not merely
/// an odd cargo argument.
pub(crate) fn feature_token_charset_ok(token: &str) -> bool {
    fn segment_ok(segment: &str) -> bool {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    }
    match token.split_once('/') {
        Some((pkg, feature)) => segment_ok(pkg) && segment_ok(feature),
        None => segment_ok(token),
    }
}

/// Validates every token in `tokens` against [`feature_token_charset_ok`],
/// returning the first offending token (owned, so the caller can name it in
/// an error) on failure. `frust build`'s release lanes
/// (`commands::build::validate_extra_features`) are the sole caller — see
/// that function and [`feature_token_charset_ok`] for why.
pub(crate) fn validate_feature_token_charset(tokens: &[String]) -> Result<(), String> {
    match tokens.iter().find(|token| !feature_token_charset_ok(token)) {
        Some(bad) => Err(bad.clone()),
        None => Ok(()),
    }
}

/// `frust create --arch` value (see `Command::Create`'s doc comment).
/// Deliberately independent of `crate::scaffold`'s own arch-tag vocabulary
/// (`scaffold::KNOWN_ARCHES`) — `frust-cli`'s `commands::create` module
/// converts one to the other via [`ArchArg::as_str`], mirroring how
/// `CreateArgs` stays decoupled from `clap` types generally. Only one
/// variant today (`clean-signals`); a future variant adds another arm here
/// plus a matching `scaffold::KNOWN_ARCHES` entry and template files.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum ArchArg {
    CleanSignals,
}

impl ArchArg {
    /// The `scaffold::generate` arch-tag string this variant corresponds
    /// to.
    pub fn as_str(self) -> &'static str {
        match self {
            ArchArg::CleanSignals => "clean-signals",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_debug_asserts() {
        Cli::command().debug_assert();
    }

    /// Bare `frust` (no subcommand) parses fine — `command` resolves to
    /// `None`, letting `main` decide the default action (TUI vs. help).
    #[test]
    fn parses_no_subcommand_as_none() {
        let cli = Cli::try_parse_from(["frust"]).expect("bare `frust` should parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_doctor() {
        let cli = Cli::parse_from(["frust", "doctor"]);
        assert!(matches!(cli.command, Some(Command::Doctor)));
    }

    #[test]
    fn parses_devices_with_global_flags() {
        let cli = Cli::parse_from(["frust", "-v", "-v", "devices", "-d", "pixel"]);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.device_id.as_deref(), Some("pixel"));
        assert!(matches!(cli.command, Some(Command::Devices)));
    }

    #[test]
    fn parses_create_dir_with_defaults() {
        let cli = Cli::parse_from(["frust", "create", "myapp"]);
        match cli.command.unwrap() {
            Command::Create {
                dir,
                org,
                project_name,
                overwrite,
                ..
            } => {
                assert_eq!(dir, "myapp");
                assert_eq!(org, "com.example");
                assert_eq!(project_name, None);
                assert!(!overwrite);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn parses_create_with_options() {
        let cli = Cli::parse_from([
            "frust",
            "create",
            "/tmp/x",
            "--project-name",
            "my_app",
            "--org",
            "dev.f0x",
            "--overwrite",
        ]);
        match cli.command.unwrap() {
            Command::Create {
                dir,
                org,
                project_name,
                overwrite,
                ..
            } => {
                assert_eq!(dir, "/tmp/x");
                assert_eq!(org, "dev.f0x");
                assert_eq!(project_name.as_deref(), Some("my_app"));
                assert!(overwrite);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn parses_create_with_deeplink_scheme_and_host() {
        let cli = Cli::parse_from([
            "frust",
            "create",
            "myapp",
            "--deeplink-scheme",
            "myapp",
            "--deeplink-host",
            "open",
        ]);
        match cli.command.unwrap() {
            Command::Create {
                deeplink_scheme,
                deeplink_host,
                ..
            } => {
                assert_eq!(deeplink_scheme.as_deref(), Some("myapp"));
                assert_eq!(deeplink_host.as_deref(), Some("open"));
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn parses_create_without_deeplink_flags_defaults_to_none() {
        let cli = Cli::parse_from(["frust", "create", "myapp"]);
        match cli.command.unwrap() {
            Command::Create {
                deeplink_scheme,
                deeplink_host,
                ..
            } => {
                assert_eq!(deeplink_scheme, None);
                assert_eq!(deeplink_host, None);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// `--arch clean-signals` parses to `ArchArg::CleanSignals`.
    #[test]
    fn parses_create_with_arch_clean_signals() {
        let cli = Cli::parse_from(["frust", "create", "myapp", "--arch", "clean-signals"]);
        match cli.command.unwrap() {
            Command::Create { arch, .. } => {
                assert_eq!(arch, Some(ArchArg::CleanSignals));
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// Omitting `--arch` defaults to `None` (the default template).
    #[test]
    fn parses_create_without_arch_defaults_to_none() {
        let cli = Cli::parse_from(["frust", "create", "myapp"]);
        match cli.command.unwrap() {
            Command::Create { arch, .. } => assert_eq!(arch, None),
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// An unrecognized `--arch` value is rejected by clap itself
    /// (only `clean-signals` is a valid `ArchArg` variant today).
    #[test]
    fn rejects_invalid_arch_value() {
        let result = Cli::try_parse_from(["frust", "create", "myapp", "--arch", "bogus"]);
        assert!(result.is_err());
    }

    #[test]
    fn arch_arg_as_str_matches_scaffold_known_arch_tag() {
        assert_eq!(ArchArg::CleanSignals.as_str(), "clean-signals");
    }

    /// `--platforms web` is accepted as a valid platforms string.
    #[test]
    fn parses_create_with_platforms_web() {
        let cli = Cli::parse_from(["frust", "create", "myapp", "--platforms", "web"]);
        match cli.command.unwrap() {
            Command::Create { platforms, .. } => {
                assert_eq!(platforms.list.as_deref(), Some("web"));
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// `--platforms android,web` is accepted as a valid comma-separated list.
    #[test]
    fn parses_create_with_multiple_platforms() {
        let cli = Cli::parse_from(["frust", "create", "myapp", "--platforms", "android,web"]);
        match cli.command.unwrap() {
            Command::Create { platforms, .. } => {
                assert_eq!(platforms.list.as_deref(), Some("android,web"));
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// Omitting `--platforms` defaults to `None` (the default platform set),
    /// and omitting `--no-sync` wires the platform projects.
    #[test]
    fn parses_create_without_platforms_defaults_to_none() {
        let cli = Cli::parse_from(["frust", "create", "myapp"]);
        match cli.command.unwrap() {
            Command::Create { platforms, .. } => {
                assert_eq!(platforms, CreatePlatformArgs::default());
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// `--no-sync` parses beside `--platforms`, in either order.
    #[test]
    fn parses_create_with_no_sync() {
        for argv in [
            [
                "frust",
                "create",
                "myapp",
                "--no-sync",
                "--platforms",
                "android",
            ],
            [
                "frust",
                "create",
                "myapp",
                "--platforms",
                "android",
                "--no-sync",
            ],
        ] {
            match Cli::parse_from(argv).command.unwrap() {
                Command::Create { platforms, .. } => assert_eq!(
                    platforms,
                    CreatePlatformArgs {
                        list: Some("android".to_string()),
                        no_sync: true,
                    }
                ),
                other => panic!("expected Create, got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_run_with_defaults() {
        let cli = Cli::parse_from(["frust", "run"]);
        match cli.command.unwrap() {
            Command::Run {
                build,
                watch,
                no_hot,
                no_open,
            } => {
                assert!(!no_hot);
                assert!(!build.build.debug);
                assert!(!build.build.profile);
                assert!(!build.build.release);
                assert!(!watch);
                assert!(!no_open);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn run_no_hot_parses_with_watch_and_is_a_no_op_without_it() {
        for (argv, want_watch) in [
            (vec!["frust", "run", "--watch", "--no-hot"], true),
            (vec!["frust", "run", "--no-hot"], false),
        ] {
            match Cli::parse_from(argv).command.unwrap() {
                Command::Run { watch, no_hot, .. } => {
                    assert_eq!(watch, want_watch);
                    assert!(no_hot);
                }
                other => panic!("expected Run, got {other:?}"),
            }
        }
    }

    #[test]
    fn run_features_flag_is_repeatable_and_splits_lists() {
        // The three spellings cargo itself accepts must be equivalent here,
        // since every downstream funnel joins the result back into one CSV.
        for argv in [
            vec!["frust", "run", "--features", "devtools,perf-trace"],
            vec!["frust", "run", "--features", "devtools perf-trace"],
            vec![
                "frust",
                "run",
                "--features",
                "devtools",
                "--features",
                "perf-trace",
            ],
        ] {
            let cli = Cli::parse_from(argv.clone());
            match cli.command.unwrap() {
                Command::Run { build, .. } => assert_eq!(
                    build.extra_features(),
                    vec!["devtools".to_string(), "perf-trace".to_string()],
                    "argv {argv:?}"
                ),
                other => panic!("expected Run, got {other:?}"),
            }
        }
    }

    #[test]
    fn features_flag_defaults_to_empty_on_every_build_target() {
        // The passthrough is opt-in: an untouched invocation must resolve to
        // no extras at all, which is what keeps every existing argv
        // byte-identical.
        for argv in [
            vec!["frust", "run"],
            vec!["frust", "build", "apk"],
            vec!["frust", "build", "appbundle"],
            vec!["frust", "build", "ios"],
            vec!["frust", "build", "macos"],
            vec!["frust", "build", "web"],
        ] {
            let cli = Cli::parse_from(argv.clone());
            let flags = match cli.command.unwrap() {
                Command::Run { build, .. } => build,
                Command::Build { target } => match target {
                    BuildTarget::Apk { build, .. }
                    | BuildTarget::Appbundle { build, .. }
                    | BuildTarget::Ios { build, .. }
                    | BuildTarget::Ipa { build, .. }
                    | BuildTarget::Macos { build, .. }
                    | BuildTarget::Windows { build, .. }
                    | BuildTarget::Linux { build, .. }
                    | BuildTarget::Web { build, .. } => build,
                },
                other => panic!("expected Run/Build, got {other:?}"),
            };
            assert!(flags.extra_features().is_empty(), "argv {argv:?}");
        }
    }

    #[test]
    fn build_apk_accepts_features_beside_the_mode_and_define_flags() {
        let cli = Cli::parse_from([
            "frust",
            "build",
            "apk",
            "--profile",
            "--features",
            "devtools",
            "--define",
            "FRUST_TRACE=1",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Apk { build, .. },
            } => {
                assert!(build.build.profile);
                assert_eq!(build.build.defines, vec!["FRUST_TRACE=1"]);
                assert_eq!(build.extra_features(), vec!["devtools".to_string()]);
            }
            other => panic!("expected Build/Apk, got {other:?}"),
        }
    }

    #[test]
    fn feature_token_charset_ok_accepts_plain_and_pkg_qualified_names() {
        for token in [
            "perf-trace",
            "perf_trace",
            "v1.2",
            "frust/devtools",
            "a-b_c.d/e-f_g.h",
        ] {
            assert!(feature_token_charset_ok(token), "{token}");
        }
    }

    #[test]
    fn feature_token_charset_ok_rejects_shell_metacharacters() {
        for token in [
            "perf-trace;rm",
            "$(rm -rf /)",
            "a b",
            "`whoami`",
            "a|b",
            "frust/devtools/extra",
            "",
            "frust/",
            "/devtools",
        ] {
            assert!(!feature_token_charset_ok(token), "{token}");
        }
    }

    #[test]
    fn validate_feature_token_charset_passes_on_all_valid_tokens() {
        let tokens = vec!["perf-trace".to_string(), "frust/devtools".to_string()];
        assert!(validate_feature_token_charset(&tokens).is_ok());
    }

    #[test]
    fn validate_feature_token_charset_names_the_first_bad_token() {
        let tokens = vec!["perf-trace".to_string(), "evil;touch".to_string()];
        let err = validate_feature_token_charset(&tokens).unwrap_err();
        assert_eq!(err, "evil;touch");
    }

    #[test]
    fn empty_and_whitespace_only_feature_values_resolve_to_no_extras() {
        let cli = Cli::parse_from(["frust", "run", "--features", " , ,", "--features", ""]);
        match cli.command.unwrap() {
            Command::Run { build, .. } => assert!(build.extra_features().is_empty()),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_watch_flag() {
        let cli = Cli::parse_from(["frust", "run", "--watch"]);
        match cli.command.unwrap() {
            Command::Run { watch, .. } => assert!(watch),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    /// `-d web --no-open` — the browser dev-server lane's own flag surface,
    /// parsed the same device-style way every other `frust run` target is.
    #[test]
    fn parses_run_with_device_web_and_no_open_flag() {
        let cli = Cli::parse_from(["frust", "-d", "web", "run", "--no-open"]);
        assert_eq!(cli.device_id.as_deref(), Some("web"));
        match cli.command.unwrap() {
            Command::Run { no_open, .. } => assert!(no_open),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    /// Omitting `--no-open` defaults to `false` — the dev server opens a
    /// browser automatically unless told not to.
    #[test]
    fn parses_run_without_no_open_flag_defaults_to_false() {
        let cli = Cli::parse_from(["frust", "-d", "web", "run"]);
        match cli.command.unwrap() {
            Command::Run { no_open, .. } => assert!(!no_open),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_device_and_mode_flags() {
        let cli = Cli::parse_from(["frust", "-d", "emulator-5554", "run", "--release"]);
        assert_eq!(cli.device_id.as_deref(), Some("emulator-5554"));
        match cli.command.unwrap() {
            Command::Run { build, .. } => assert!(build.build.release),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    /// The retired tier-selection flag is now an unknown argument, not a flag
    /// with one legal value: `frust-render` contains exactly one renderer, so
    /// there is nothing left to force. Pinned as a test because the flag was
    /// documented and scripted against — clap must refuse it outright rather
    /// than accept it and set an env var nothing reads.
    #[test]
    fn rejects_the_retired_render_tier_flag() {
        for value in ["engine", "gpu", "cpu", "vello"] {
            assert!(
                Cli::try_parse_from(["frust", "run", "--render-tier", value]).is_err(),
                "--render-tier {value} must be rejected"
            );
        }
        assert!(Cli::try_parse_from(["frust", "run", "--render-tier"]).is_err());
    }

    #[test]
    fn parses_build_apk_with_full_flag_surface() {
        let cli = Cli::parse_from([
            "frust",
            "build",
            "apk",
            "--flavor",
            "paid",
            "--build-name",
            "1.2.3",
            "--build-number",
            "42",
            "--define",
            "A=B",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Apk { build, .. },
            } => {
                assert_eq!(build.build.flavor.as_deref(), Some("paid"));
                assert_eq!(build.build.build_name.as_deref(), Some("1.2.3"));
                assert_eq!(build.build.build_number, Some(42));
                assert_eq!(build.build.defines, vec!["A=B".to_string()]);
            }
            other => panic!("expected Build/Apk, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_apk_split_per_abi_and_target_platform() {
        let cli = Cli::parse_from([
            "frust",
            "build",
            "apk",
            "--split-per-abi",
            "--target-platform",
            "android-arm64,android-x64",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target:
                    BuildTarget::Apk {
                        split_per_abi,
                        target_platform,
                        ..
                    },
            } => {
                assert!(split_per_abi);
                assert_eq!(
                    target_platform.as_deref(),
                    Some("android-arm64,android-x64")
                );
            }
            other => panic!("expected Build/Apk, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_appbundle_alias_aab() {
        let cli = Cli::parse_from(["frust", "build", "aab"]);
        assert!(matches!(
            cli.command,
            Some(Command::Build {
                target: BuildTarget::Appbundle { .. }
            })
        ));
    }

    #[test]
    fn parses_build_ios_flags() {
        let cli = Cli::parse_from(["frust", "build", "ios", "--simulator", "--no-codesign"]);
        match cli.command.unwrap() {
            Command::Build {
                target:
                    BuildTarget::Ios {
                        simulator,
                        no_codesign,
                        ..
                    },
            } => {
                assert!(simulator);
                assert!(no_codesign);
            }
            other => panic!("expected Build/Ios, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_ipa_export_method() {
        let cli = Cli::parse_from([
            "frust",
            "build",
            "ipa",
            "--export-method",
            "app-store-connect",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Ipa { export_method, .. },
            } => {
                assert_eq!(export_method, "app-store-connect");
            }
            other => panic!("expected Build/Ipa, got {other:?}"),
        }
    }

    #[test]
    fn build_ipa_requires_export_method() {
        let result = Cli::try_parse_from(["frust", "build", "ipa"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_build_macos_with_installer_flag() {
        let cli = Cli::parse_from(["frust", "build", "macos", "--installer"]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Macos { installer, .. },
            } => assert!(installer),
            other => panic!("expected Build/Macos, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_macos_without_installer_defaults_to_false() {
        let cli = Cli::parse_from(["frust", "build", "macos"]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Macos { installer, .. },
            } => assert!(!installer),
            other => panic!("expected Build/Macos, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_windows_with_full_flag_surface() {
        let cli = Cli::parse_from([
            "frust",
            "build",
            "windows",
            "--installer",
            "--build-name",
            "1.2.3",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Windows { build, installer },
            } => {
                assert!(installer);
                assert_eq!(build.build.build_name.as_deref(), Some("1.2.3"));
            }
            other => panic!("expected Build/Windows, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_linux_with_installer_flag() {
        let cli = Cli::parse_from(["frust", "build", "linux", "--installer"]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Linux { installer, .. },
            } => assert!(installer),
            other => panic!("expected Build/Linux, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_linux_without_flags_uses_build_arg_defaults() {
        let cli = Cli::parse_from(["frust", "build", "linux"]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Linux { build, installer },
            } => {
                assert!(!installer);
                assert!(!build.build.debug);
                assert!(!build.build.profile);
                assert!(!build.build.release);
            }
            other => panic!("expected Build/Linux, got {other:?}"),
        }
    }

    /// `build web` has no `--installer`/`--target-platform`/`--simulator`
    /// flags of its own — the plain `BuildFlags` funnel only, mirroring the
    /// desktop targets' shape without the installer axis (there is no
    /// installer format for a browser artifact directory).
    #[test]
    fn parses_build_web_defaults() {
        let cli = Cli::parse_from(["frust", "build", "web"]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Web { build },
            } => {
                assert!(!build.build.debug);
                assert!(!build.build.profile);
                assert!(!build.build.release);
                assert!(build.extra_features().is_empty());
            }
            other => panic!("expected Build/Web, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_web_with_release_and_build_name() {
        let cli = Cli::parse_from([
            "frust",
            "build",
            "web",
            "--release",
            "--build-name",
            "1.2.3",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Web { build },
            } => {
                assert!(build.build.release);
                assert_eq!(build.build.build_name.as_deref(), Some("1.2.3"));
            }
            other => panic!("expected Build/Web, got {other:?}"),
        }
    }

    #[test]
    fn parses_clean() {
        let cli = Cli::parse_from(["frust", "clean"]);
        assert!(matches!(cli.command, Some(Command::Clean)));
    }

    #[test]
    fn parses_tui() {
        let cli = Cli::parse_from(["frust", "tui"]);
        assert!(matches!(cli.command, Some(Command::Tui)));
    }
}
