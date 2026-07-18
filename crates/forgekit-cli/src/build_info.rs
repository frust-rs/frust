//! `BuildInfo` funnel between clap flags and platform builders (spec §12.1/12.2).
//!
//! `run` (`Command::Run`, debug-default) and `build` (`Command::Build`,
//! release-default) both `#[command(flatten)]` [`BuildArgs`] into their clap
//! structs and call [`BuildInfo::from_args`] with their own `default_mode`.

use std::collections::HashMap;

/// The three user-facing build modes (spec §12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    Debug,
    Profile,
    Release,
}

impl BuildMode {
    /// The `cargo build`/`cargo ndk` flag(s) selecting this mode's profile:
    /// `[]` (default `dev` profile) / `["--profile", "profile"]` /
    /// `["--release"]`. Shared by `android_build`/`ios_build` (tasks 64/65)
    /// so the mode → cargo-profile mapping has one source.
    pub fn cargo_profile_arg(&self) -> &'static [&'static str] {
        match self {
            BuildMode::Debug => &[],
            BuildMode::Profile => &["--profile", "profile"],
            BuildMode::Release => &["--release"],
        }
    }

    /// The Gradle build-type infix used in a task name (spec §12.5's
    /// `assemble<Flavor><Mode>`/`bundle<Flavor><Mode>`), e.g.
    /// `assemble{flavor}{Debug,Profile,Release}`.
    pub fn gradle_infix(&self) -> &'static str {
        match self {
            BuildMode::Debug => "Debug",
            BuildMode::Profile => "Profile",
            BuildMode::Release => "Release",
        }
    }

    /// The Xcode `-configuration` value for this mode (spec §12.6), before
    /// any flavor/scheme suffix (`<Mode>-<Scheme>`) is appended.
    pub fn xcode_configuration(&self) -> &'static str {
        match self {
            BuildMode::Debug => "Debug",
            BuildMode::Profile => "Profile",
            BuildMode::Release => "Release",
        }
    }
}

/// `#[command(flatten)]`-able flags shared by every command that produces a build.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildArgs {
    /// Build in debug mode (`dev` cargo profile).
    #[arg(long)]
    pub debug: bool,
    /// Build in profile mode (release opts + debug symbols + tracing).
    #[arg(long)]
    pub profile: bool,
    /// Build in release mode (LTO, stripped, `panic=abort`).
    #[arg(long)]
    pub release: bool,
    /// Product flavor to build (maps to a Gradle flavor / Xcode scheme).
    #[arg(long)]
    pub flavor: Option<String>,
    /// Compile-time app config, `KEY=VALUE`; repeatable.
    #[arg(long = "define", value_name = "KEY=VALUE")]
    pub defines: Vec<String>,
    /// Semantic version string embedded in the build (e.g. `1.2.3`).
    #[arg(long = "build-name", value_name = "VER")]
    pub build_name: Option<String>,
    /// Monotonically increasing build number embedded in the build; must be
    /// `>= 1`.
    #[arg(long = "build-number", value_name = "N")]
    pub build_number: Option<u32>,
}

/// A single validated funnel between flag parsing and platform builders
/// (Flutter's `build_info.dart` pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub mode: BuildMode,
    pub flavor: Option<String>,
    pub defines: HashMap<String, String>,
    pub build_name: Option<String>,
    pub build_number: Option<u32>,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum BuildInfoError {
    #[error("--debug, --profile, and --release are mutually exclusive")]
    ConflictingModes,
    #[error("invalid --define '{0}': expected KEY=VALUE with a non-empty key")]
    InvalidDefine(String),
    #[error("invalid --build-number '{0}': must be >= 1")]
    InvalidBuildNumber(u32),
}

impl BuildInfo {
    /// Resolves [`BuildArgs`] into a [`BuildInfo`], using `default_mode` when
    /// none of `--debug`/`--profile`/`--release` is given (each command picks
    /// its own default, e.g. `run` → debug, `build *` → release).
    ///
    /// When mode is [`BuildMode::Profile`], automatically injects
    /// `FORGEKIT_TRACE=1` into the defines (spec §14 "--profile mode tracing")
    /// unless the user already provided a `FORGEKIT_TRACE` define.
    pub fn from_args(args: BuildArgs, default_mode: BuildMode) -> Result<Self, BuildInfoError> {
        let flags = [args.debug, args.profile, args.release];
        if flags.iter().filter(|set| **set).count() > 1 {
            return Err(BuildInfoError::ConflictingModes);
        }

        let mode = if args.debug {
            BuildMode::Debug
        } else if args.profile {
            BuildMode::Profile
        } else if args.release {
            BuildMode::Release
        } else {
            default_mode
        };

        let mut defines = HashMap::with_capacity(args.defines.len());
        for define in &args.defines {
            let Some((key, value)) = define.split_once('=') else {
                return Err(BuildInfoError::InvalidDefine(define.clone()));
            };
            if key.is_empty() {
                return Err(BuildInfoError::InvalidDefine(define.clone()));
            }
            defines.insert(key.to_string(), value.to_string());
        }

        // Auto-inject FORGEKIT_TRACE=1 in profile mode unless user provided it.
        if mode == BuildMode::Profile {
            defines
                .entry("FORGEKIT_TRACE".to_string())
                .or_insert_with(|| "1".to_string());
        }

        if let Some(n) = args.build_number
            && n == 0
        {
            return Err(BuildInfoError::InvalidBuildNumber(n));
        }

        Ok(BuildInfo {
            mode,
            flavor: args.flavor,
            defines,
            build_name: args.build_name,
            build_number: args.build_number,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> BuildArgs {
        BuildArgs::default()
    }

    #[test]
    fn defaults_to_given_mode_when_no_flag_set() {
        let info = BuildInfo::from_args(args(), BuildMode::Debug).unwrap();
        assert_eq!(info.mode, BuildMode::Debug);
    }

    #[test]
    fn explicit_flag_overrides_default() {
        let info = BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(info.mode, BuildMode::Release);
    }

    #[test]
    fn conflicting_mode_flags_rejected() {
        let err = BuildInfo::from_args(
            BuildArgs {
                debug: true,
                release: true,
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap_err();
        assert_eq!(err, BuildInfoError::ConflictingModes);
    }

    #[test]
    fn parses_defines() {
        let info = BuildInfo::from_args(
            BuildArgs {
                defines: vec!["API_URL=https://example.com".into(), "DEBUG_UI=1".into()],
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("API_URL").map(String::as_str),
            Some("https://example.com")
        );
        assert_eq!(info.defines.get("DEBUG_UI").map(String::as_str), Some("1"));
    }

    #[test]
    fn rejects_define_without_equals() {
        let err = BuildInfo::from_args(
            BuildArgs {
                defines: vec!["NOVALUE".into()],
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap_err();
        assert_eq!(err, BuildInfoError::InvalidDefine("NOVALUE".into()));
    }

    #[test]
    fn rejects_define_with_empty_key() {
        let err = BuildInfo::from_args(
            BuildArgs {
                defines: vec!["=value".into()],
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap_err();
        assert_eq!(err, BuildInfoError::InvalidDefine("=value".into()));
    }

    #[test]
    fn flavor_passes_through() {
        let info = BuildInfo::from_args(
            BuildArgs {
                flavor: Some("paid".into()),
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(info.flavor.as_deref(), Some("paid"));
    }

    #[test]
    fn build_name_and_number_pass_through() {
        let info = BuildInfo::from_args(
            BuildArgs {
                build_name: Some("1.2.3".into()),
                build_number: Some(42),
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap();
        assert_eq!(info.build_name.as_deref(), Some("1.2.3"));
        assert_eq!(info.build_number, Some(42));
    }

    #[test]
    fn rejects_zero_build_number() {
        let err = BuildInfo::from_args(
            BuildArgs {
                build_number: Some(0),
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap_err();
        assert_eq!(err, BuildInfoError::InvalidBuildNumber(0));
    }

    #[test]
    fn cargo_profile_arg_matches_each_mode() {
        assert_eq!(BuildMode::Debug.cargo_profile_arg(), &[] as &[&str]);
        assert_eq!(
            BuildMode::Profile.cargo_profile_arg(),
            &["--profile", "profile"]
        );
        assert_eq!(BuildMode::Release.cargo_profile_arg(), &["--release"]);
    }

    #[test]
    fn gradle_infix_matches_each_mode() {
        assert_eq!(BuildMode::Debug.gradle_infix(), "Debug");
        assert_eq!(BuildMode::Profile.gradle_infix(), "Profile");
        assert_eq!(BuildMode::Release.gradle_infix(), "Release");
    }

    #[test]
    fn xcode_configuration_matches_each_mode() {
        assert_eq!(BuildMode::Debug.xcode_configuration(), "Debug");
        assert_eq!(BuildMode::Profile.xcode_configuration(), "Profile");
        assert_eq!(BuildMode::Release.xcode_configuration(), "Release");
    }

    #[test]
    fn profile_mode_injects_forgekit_trace() {
        let info = BuildInfo::from_args(
            BuildArgs {
                profile: true,
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("FORGEKIT_TRACE").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn debug_mode_does_not_inject_forgekit_trace() {
        let info = BuildInfo::from_args(
            BuildArgs {
                debug: true,
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert!(!info.defines.contains_key("FORGEKIT_TRACE"));
    }

    #[test]
    fn release_mode_does_not_inject_forgekit_trace() {
        let info = BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap();
        assert!(!info.defines.contains_key("FORGEKIT_TRACE"));
    }

    #[test]
    fn user_defined_forgekit_trace_zero_wins_over_profile_injection() {
        let info = BuildInfo::from_args(
            BuildArgs {
                profile: true,
                defines: vec!["FORGEKIT_TRACE=0".into()],
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("FORGEKIT_TRACE").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn user_defined_forgekit_trace_custom_wins_over_profile_injection() {
        let info = BuildInfo::from_args(
            BuildArgs {
                profile: true,
                defines: vec!["FORGEKIT_TRACE=custom_value".into()],
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("FORGEKIT_TRACE").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn profile_default_mode_injects_forgekit_trace() {
        // Test that profile injection works when profile is the default mode
        let info = BuildInfo::from_args(args(), BuildMode::Profile).unwrap();
        assert_eq!(
            info.defines.get("FORGEKIT_TRACE").map(String::as_str),
            Some("1")
        );
    }
}
