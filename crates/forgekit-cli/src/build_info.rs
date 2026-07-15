//! `BuildInfo` funnel between clap flags and platform builders (spec §12.1/12.2).
//!
//! Not consumed by any command yet — `run`/`build` (spec Phase 2+) will
//! `#[command(flatten)]` [`BuildArgs`] into their clap structs and call
//! [`BuildInfo::from_args`].

use std::collections::HashMap;

/// The three user-facing build modes (spec §12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    Debug,
    Profile,
    Release,
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
}

/// A single validated funnel between flag parsing and platform builders
/// (Flutter's `build_info.dart` pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub mode: BuildMode,
    pub flavor: Option<String>,
    pub defines: HashMap<String, String>,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum BuildInfoError {
    #[error("--debug, --profile, and --release are mutually exclusive")]
    ConflictingModes,
    #[error("invalid --define '{0}': expected KEY=VALUE with a non-empty key")]
    InvalidDefine(String),
}

impl BuildInfo {
    /// Resolves [`BuildArgs`] into a [`BuildInfo`], using `default_mode` when
    /// none of `--debug`/`--profile`/`--release` is given (each command picks
    /// its own default, e.g. `run` → debug, `build *` → release).
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

        Ok(BuildInfo { mode, flavor: args.flavor, defines })
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
        let info = BuildInfo::from_args(BuildArgs { release: true, ..args() }, BuildMode::Debug).unwrap();
        assert_eq!(info.mode, BuildMode::Release);
    }

    #[test]
    fn conflicting_mode_flags_rejected() {
        let err =
            BuildInfo::from_args(BuildArgs { debug: true, release: true, ..args() }, BuildMode::Debug).unwrap_err();
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
        assert_eq!(info.defines.get("API_URL").map(String::as_str), Some("https://example.com"));
        assert_eq!(info.defines.get("DEBUG_UI").map(String::as_str), Some("1"));
    }

    #[test]
    fn rejects_define_without_equals() {
        let err = BuildInfo::from_args(BuildArgs { defines: vec!["NOVALUE".into()], ..args() }, BuildMode::Release)
            .unwrap_err();
        assert_eq!(err, BuildInfoError::InvalidDefine("NOVALUE".into()));
    }

    #[test]
    fn rejects_define_with_empty_key() {
        let err = BuildInfo::from_args(BuildArgs { defines: vec!["=value".into()], ..args() }, BuildMode::Release)
            .unwrap_err();
        assert_eq!(err, BuildInfoError::InvalidDefine("=value".into()));
    }

    #[test]
    fn flavor_passes_through() {
        let info =
            BuildInfo::from_args(BuildArgs { flavor: Some("paid".into()), ..args() }, BuildMode::Debug).unwrap();
        assert_eq!(info.flavor.as_deref(), Some("paid"));
    }
}
