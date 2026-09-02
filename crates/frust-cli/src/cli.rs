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

        /// Override the computed path to the `frust` facade crate
        /// (development only; a temporary `frust_path` mechanism).
        /// Accepts either the facade crate itself (a directory whose
        /// Cargo.toml names package `frust`, e.g. `<repo>/crates/frust`)
        /// or that repo's root (e.g. `<repo>`), which is normalised to the
        /// nested facade crate directory; anything else is rejected.
        #[arg(long = "frust-path", value_name = "PATH", hide = true)]
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
        /// instead of the default notes-app demo.
        /// **Dev-machine-only while `frust` is unpublished**: the generated
        /// `Cargo.toml` path-deps into this Frust checkout for `frust` and
        /// the in-repo `clean-signals-frust` plugin — the project won't
        /// build without this checkout present. `clean-signals` itself is
        /// git+rev-pinned to its public repo (see `docs/DEVELOPMENT.md`'s
        /// Version-Pin Policy), so no sibling `clean-signals-rs` checkout
        /// is required.
        #[arg(long = "arch", value_name = "ARCH")]
        arch: Option<ArchArg>,

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
    /// no Android device selected → `cargo run` passthrough.
    Run {
        #[command(flatten)]
        build: BuildFlags,

        /// Force the render tier (`cpu`/`engine`)
        /// `frust-render` probes for at startup, by setting
        /// `FRUST_RENDER_TIER` for the launched process (see
        /// docs/DEVELOPMENT.md; `frust_render::select_render_tier`
        /// / `RENDER_TIER_ENV_VAR`). `engine` is the default tier
        /// (`frust-render`'s `engine-tier` feature, on by default) — this
        /// override is equivalent to leaving the flag unset. The
        /// vello-classic `gpu` tier is GONE and is no longer an accepted
        /// value. `cpu` is a
        /// non-default tier the app must have been BUILT with
        /// (`frust-render`'s `cpu-tier` feature); otherwise the
        /// launched process refuses the override at startup and says so.
        /// **Desktop-preview only in v1**: the
        /// `cargo run` fallback gets the env var directly; plumbing an
        /// override to a launched Android/iOS device (`adb`/`devicectl`
        /// env/intent extras) is not implemented yet — the on-device tier
        /// probe still runs regardless, it just can't be forced from here.
        #[arg(long = "render-tier", value_name = "TIER")]
        render_tier: Option<RenderTierArg>,

        /// Desktop-only rebuild-relaunch dev loop:
        /// watches the project's `src/` tree and `Cargo.toml`, and on
        /// any change kills the running `cargo run` child and relaunches a
        /// fresh one, streaming its output the whole time. This is
        /// explicitly a relaunch loop, not state-preserving hot reload —
        /// app state resets on every relaunch. **Desktop-preview only**:
        /// combining `--watch` with `-d <device>` is a hard error (the
        /// watch loop has no device-side kill/rebuild/relaunch story yet).
        #[arg(long)]
        watch: bool,
    },
    /// Produce a distributable artifact — release-signed
    /// APK/AAB via Gradle, or an iOS app/IPA via `xcodebuild`. Defaults to
    /// release mode (unlike `run`, which defaults to debug).
    Build {
        #[command(subcommand)]
        target: BuildTarget,
    },
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
/// (`templates/app/ios.tmpl/Runner.xcodeproj/project.pbxproj.tmpl`'s `cargo
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

/// `frust run --render-tier` value (see `Command::Run`'s doc comment).
/// Deliberately independent of `frust_render::RenderTier` — `frust-cli`
/// has no compile-time dependency on the rendering stack (see
/// `docs/ARCHITECTURE.md`) — but its variants and their lowercase env
/// string ([`RenderTierArg::env_value`]) must stay in sync with
/// `frust_render::parse_render_tier_override`'s accepted values by hand.
///
/// The non-default tier is listed here unconditionally, exactly
/// because this enum is hand-synced rather than derived: which of them a
/// given app actually contains is a property of the app's own cargo features,
/// which this CLI neither knows nor builds — it only sets an env var for the
/// launched process, and that process refuses an override it cannot honour.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum RenderTierArg {
    Cpu,
    Engine,
}

impl RenderTierArg {
    /// The value `FRUST_RENDER_TIER` is set to for the spawned process.
    pub fn env_value(self) -> &'static str {
        match self {
            RenderTierArg::Cpu => "cpu",
            RenderTierArg::Engine => "engine",
        }
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

    #[test]
    fn parses_run_with_defaults() {
        let cli = Cli::parse_from(["frust", "run"]);
        match cli.command.unwrap() {
            Command::Run {
                build,
                render_tier,
                watch,
            } => {
                assert!(!build.build.debug);
                assert!(!build.build.profile);
                assert!(!build.build.release);
                assert_eq!(render_tier, None);
                assert!(!watch);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn run_features_flag_is_repeatable_and_splits_lists() {
        // The three spellings cargo itself accepts must be equivalent here,
        // since every downstream funnel joins the result back into one CSV.
        for argv in [
            vec!["frust", "run", "--features", "engine-tier,perf-trace"],
            vec!["frust", "run", "--features", "engine-tier perf-trace"],
            vec![
                "frust",
                "run",
                "--features",
                "engine-tier",
                "--features",
                "perf-trace",
            ],
        ] {
            let cli = Cli::parse_from(argv.clone());
            match cli.command.unwrap() {
                Command::Run { build, .. } => assert_eq!(
                    build.extra_features(),
                    vec!["engine-tier".to_string(), "perf-trace".to_string()],
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
                    | BuildTarget::Linux { build, .. } => build,
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
            "engine-tier",
            "--define",
            "FRUST_RENDER_TIER=engine",
        ]);
        match cli.command.unwrap() {
            Command::Build {
                target: BuildTarget::Apk { build, .. },
            } => {
                assert!(build.build.profile);
                assert_eq!(build.build.defines, vec!["FRUST_RENDER_TIER=engine"]);
                assert_eq!(build.extra_features(), vec!["engine-tier".to_string()]);
            }
            other => panic!("expected Build/Apk, got {other:?}"),
        }
    }

    #[test]
    fn feature_token_charset_ok_accepts_plain_and_pkg_qualified_names() {
        for token in [
            "engine-tier",
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
            "engine-tier;rm",
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
        let tokens = vec!["engine-tier".to_string(), "frust/devtools".to_string()];
        assert!(validate_feature_token_charset(&tokens).is_ok());
    }

    #[test]
    fn validate_feature_token_charset_names_the_first_bad_token() {
        let tokens = vec!["engine-tier".to_string(), "evil;touch".to_string()];
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

    #[test]
    fn parses_run_with_device_and_mode_flags() {
        let cli = Cli::parse_from(["frust", "-d", "emulator-5554", "run", "--release"]);
        assert_eq!(cli.device_id.as_deref(), Some("emulator-5554"));
        match cli.command.unwrap() {
            Command::Run { build, .. } => assert!(build.build.release),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn rejects_the_retired_gpu_render_tier_value() {
        // The vello-classic tier was deleted, and with it the `gpu` override
        // `frust_render::parse_render_tier_override` used to accept — so clap
        // must refuse the value here rather than set an env var the launched
        // process would only warn about and ignore.
        assert!(Cli::try_parse_from(["frust", "run", "--render-tier", "gpu"]).is_err());
    }

    #[test]
    fn parses_run_with_render_tier_cpu() {
        let cli = Cli::parse_from(["frust", "run", "--render-tier", "cpu"]);
        match cli.command.unwrap() {
            Command::Run { render_tier, .. } => {
                assert_eq!(render_tier, Some(RenderTierArg::Cpu));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_render_tier_engine() {
        let cli = Cli::parse_from(["frust", "run", "--render-tier", "engine"]);
        match cli.command.unwrap() {
            Command::Run { render_tier, .. } => {
                assert_eq!(render_tier, Some(RenderTierArg::Engine));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_without_render_tier_is_none() {
        let cli = Cli::parse_from(["frust", "run"]);
        match cli.command.unwrap() {
            Command::Run { render_tier, .. } => assert_eq!(render_tier, None),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_render_tier_value() {
        // clap validates against the enum, so the fixture has to name
        // something outside it. `vello` is the engine, never a tier
        // name, and matches the unknown-value fixture in
        // `frust_render::tier`'s own parse test.
        let result = Cli::try_parse_from(["frust", "run", "--render-tier", "vello"]);
        assert!(result.is_err());
    }

    #[test]
    fn render_tier_arg_env_values() {
        assert_eq!(RenderTierArg::Cpu.env_value(), "cpu");
        // Hand-synced with `frust_render::parse_render_tier_override`'s
        // accepted strings — this crate depends on no render crate to check
        // it against.
        assert_eq!(RenderTierArg::Engine.env_value(), "engine");
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
