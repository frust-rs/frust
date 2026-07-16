//! Top-level clap parser (spec §12.1), modeled on `flutter_tools`' command surface.

use clap::{Parser, Subcommand};

use crate::build_info::BuildArgs;

#[derive(Parser, Debug)]
#[command(
    name = "forgekit",
    version,
    about = "Tooling for ForgeKit apps (spec §12)"
)]
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
    /// Scaffold a new app (spec §12.3 — v1 generates a desktop-preview
    /// Rust crate; `android/`/`ios/` platform projects land in spec Phase
    /// 2/3).
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
        #[arg(long, default_value = "A new ForgeKit application.")]
        description: String,

        /// Overwrite a non-empty target directory.
        #[arg(long)]
        overwrite: bool,

        /// Override the embedded template directory (development only).
        #[arg(long = "template-dir", value_name = "PATH", hide = true)]
        template_dir: Option<String>,

        /// Override the computed path to the `forgekit` facade crate
        /// (development only; spec §12.3's temporary `forgekit_path`
        /// mechanism).
        #[arg(long = "forgekit-path", value_name = "PATH", hide = true)]
        forgekit_path: Option<String>,
    },
    /// Validate the ForgeKit toolchain (Rust targets, NDK, Android SDK, Xcode).
    Doctor,
    /// List connected devices, emulators, and simulators.
    Devices,
    /// Remove build outputs (cargo target dirs + Gradle/Xcode build dirs).
    Clean,
    /// Build → install → launch → stream logs on a connected device
    /// (spec §12.4); no Android device selected → `cargo run` passthrough.
    Run {
        #[command(flatten)]
        build: BuildArgs,
    },
    /// Produce a distributable artifact (spec §12.5/12.6) — release-signed
    /// APK/AAB via Gradle, or an iOS app/IPA via `xcodebuild`. Defaults to
    /// release mode (unlike `run`, which defaults to debug).
    Build {
        #[command(subcommand)]
        target: BuildTarget,
    },
}

/// The artifact `forgekit build` produces (spec §12.5/12.6). Kept off
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
        let cli = Cli::parse_from(["forgekit", "doctor"]);
        assert!(matches!(cli.command, Command::Doctor));
    }

    #[test]
    fn parses_devices_with_global_flags() {
        let cli = Cli::parse_from(["forgekit", "-v", "-v", "devices", "-d", "pixel"]);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.device_id.as_deref(), Some("pixel"));
        assert!(matches!(cli.command, Command::Devices));
    }

    #[test]
    fn parses_create_dir_with_defaults() {
        let cli = Cli::parse_from(["forgekit", "create", "myapp"]);
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
            "forgekit",
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
    fn parses_run_with_defaults() {
        let cli = Cli::parse_from(["forgekit", "run"]);
        match cli.command {
            Command::Run { build } => {
                assert!(!build.debug);
                assert!(!build.profile);
                assert!(!build.release);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_device_and_mode_flags() {
        let cli = Cli::parse_from(["forgekit", "-d", "emulator-5554", "run", "--release"]);
        assert_eq!(cli.device_id.as_deref(), Some("emulator-5554"));
        match cli.command {
            Command::Run { build } => assert!(build.release),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_apk_with_full_flag_surface() {
        let cli = Cli::parse_from([
            "forgekit",
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
            "forgekit",
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
        let cli = Cli::parse_from(["forgekit", "build", "aab"]);
        assert!(matches!(
            cli.command,
            Command::Build {
                target: BuildTarget::Appbundle { .. }
            }
        ));
    }

    #[test]
    fn parses_build_ios_flags() {
        let cli = Cli::parse_from(["forgekit", "build", "ios", "--simulator", "--no-codesign"]);
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
            "forgekit",
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
        let result = Cli::try_parse_from(["forgekit", "build", "ipa"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_clean() {
        let cli = Cli::parse_from(["forgekit", "clean"]);
        assert!(matches!(cli.command, Command::Clean));
    }
}
