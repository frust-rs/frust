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

    #[command(subcommand)]
    pub command: Command,
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
        build: BuildArgs,

        /// Force the render tier (`gpu`/`cpu`) `frust-render` probes for
        /// at startup, by setting `FRUST_RENDER_TIER` for the launched
        /// process (see docs/DEVELOPMENT.md; `frust_render::select_render_tier`
        /// / `RENDER_TIER_ENV_VAR`). **Desktop-preview only in v1**: the
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
        build: BuildArgs,

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
        build: BuildArgs,

        /// Comma-separated target ABIs (`android-arm64`, `android-arm`,
        /// `android-x64`); defaults to all three.
        #[arg(long = "target-platform", value_name = "CSV")]
        target_platform: Option<String>,
    },
    /// iOS device/simulator build via `xcodebuild` (macOS host only).
    Ios {
        #[command(flatten)]
        build: BuildArgs,

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
        build: BuildArgs,

        /// Export method: `app-store-connect`, `release-testing`,
        /// `debugging`, or `enterprise`.
        #[arg(long = "export-method", value_name = "METHOD")]
        export_method: String,
    },
}

/// `frust run --render-tier` value (see `Command::Run`'s doc comment).
/// Deliberately independent of `frust_render::RenderTier` — `frust-cli`
/// has no compile-time dependency on the rendering stack (see
/// `docs/ARCHITECTURE.md`) — but its two variants and their lowercase env
/// string ([`RenderTierArg::env_value`]) must stay in sync with
/// `frust_render::parse_render_tier_override`'s accepted values by hand.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum RenderTierArg {
    Gpu,
    Cpu,
}

impl RenderTierArg {
    /// The value `FRUST_RENDER_TIER` is set to for the spawned process.
    pub fn env_value(self) -> &'static str {
        match self {
            RenderTierArg::Gpu => "gpu",
            RenderTierArg::Cpu => "cpu",
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

    #[test]
    fn parses_doctor() {
        let cli = Cli::parse_from(["frust", "doctor"]);
        assert!(matches!(cli.command, Command::Doctor));
    }

    #[test]
    fn parses_devices_with_global_flags() {
        let cli = Cli::parse_from(["frust", "-v", "-v", "devices", "-d", "pixel"]);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.device_id.as_deref(), Some("pixel"));
        assert!(matches!(cli.command, Command::Devices));
    }

    #[test]
    fn parses_create_dir_with_defaults() {
        let cli = Cli::parse_from(["frust", "create", "myapp"]);
        match cli.command {
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
        match cli.command {
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
        match cli.command {
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
        match cli.command {
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
        match cli.command {
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
        match cli.command {
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
        match cli.command {
            Command::Run {
                build,
                render_tier,
                watch,
            } => {
                assert!(!build.debug);
                assert!(!build.profile);
                assert!(!build.release);
                assert_eq!(render_tier, None);
                assert!(!watch);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_watch_flag() {
        let cli = Cli::parse_from(["frust", "run", "--watch"]);
        match cli.command {
            Command::Run { watch, .. } => assert!(watch),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_device_and_mode_flags() {
        let cli = Cli::parse_from(["frust", "-d", "emulator-5554", "run", "--release"]);
        assert_eq!(cli.device_id.as_deref(), Some("emulator-5554"));
        match cli.command {
            Command::Run { build, .. } => assert!(build.release),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_render_tier_gpu() {
        let cli = Cli::parse_from(["frust", "run", "--render-tier", "gpu"]);
        match cli.command {
            Command::Run { render_tier, .. } => {
                assert_eq!(render_tier, Some(RenderTierArg::Gpu));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_render_tier_cpu() {
        let cli = Cli::parse_from(["frust", "run", "--render-tier", "cpu"]);
        match cli.command {
            Command::Run { render_tier, .. } => {
                assert_eq!(render_tier, Some(RenderTierArg::Cpu));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_without_render_tier_is_none() {
        let cli = Cli::parse_from(["frust", "run"]);
        match cli.command {
            Command::Run { render_tier, .. } => assert_eq!(render_tier, None),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_render_tier_value() {
        let result = Cli::try_parse_from(["frust", "run", "--render-tier", "hybrid"]);
        assert!(result.is_err());
    }

    #[test]
    fn render_tier_arg_env_values() {
        assert_eq!(RenderTierArg::Gpu.env_value(), "gpu");
        assert_eq!(RenderTierArg::Cpu.env_value(), "cpu");
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
        match cli.command {
            Command::Build {
                target: BuildTarget::Apk { build, .. },
            } => {
                assert_eq!(build.flavor.as_deref(), Some("paid"));
                assert_eq!(build.build_name.as_deref(), Some("1.2.3"));
                assert_eq!(build.build_number, Some(42));
                assert_eq!(build.defines, vec!["A=B".to_string()]);
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
        match cli.command {
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
            Command::Build {
                target: BuildTarget::Appbundle { .. }
            }
        ));
    }

    #[test]
    fn parses_build_ios_flags() {
        let cli = Cli::parse_from(["frust", "build", "ios", "--simulator", "--no-codesign"]);
        match cli.command {
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
        match cli.command {
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
    fn parses_clean() {
        let cli = Cli::parse_from(["frust", "clean"]);
        assert!(matches!(cli.command, Command::Clean));
    }

    #[test]
    fn parses_tui() {
        let cli = Cli::parse_from(["frust", "tui"]);
        assert!(matches!(cli.command, Command::Tui));
    }
}
