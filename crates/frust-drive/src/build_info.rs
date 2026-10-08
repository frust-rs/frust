//! `BuildInfo` funnel between build flags and platform builders.
//!
//! `run` (debug-default) and `build` (release-default) both resolve their
//! flags into a plain [`BuildArgs`] and call [`BuildInfo::from_args`] with
//! their own `default_mode`. The clap layer lives in `frust-cli`
//! (`build_args::BuildArgs`, `#[derive(clap::Args)]`) and converts into this
//! crate's [`BuildArgs`] at the command-handler boundary — `frust-drive`
//! itself has no clap dependency (`docs/ARCHITECTURE.md`).

use std::collections::HashMap;

/// The Rust target triple every browser build compiles to — the single
/// spelling shared by the drive-side web build, `frust doctor`'s toolchain
/// check, and the recipe `crates/frust-shell-web/platform/web/README.md` documents. There is no
/// second wasm triple in play here: `wasm32-unknown-emscripten` and
/// `wasm32-wasip1` are neither what `wasm-bindgen --target web` consumes nor
/// what `crates/frust-shell-web` builds against.
pub const WASM_TARGET_TRIPLE: &str = "wasm32-unknown-unknown";

/// The three user-facing build modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    Debug,
    Profile,
    Release,
}

impl BuildMode {
    /// The `cargo build`/`cargo ndk` flag(s) selecting this mode's profile:
    /// `[]` (default `dev` profile) / `["--profile", "profile"]` /
    /// `["--release"]`. Shared by `android_build`/`ios_build` so the mode →
    /// cargo-profile mapping has one source.
    pub fn cargo_profile_arg(&self) -> &'static [&'static str] {
        match self {
            BuildMode::Debug => &[],
            BuildMode::Profile => &["--profile", "profile"],
            BuildMode::Release => &["--release"],
        }
    }

    /// The Gradle build-type infix used in a task name
    /// (`assemble<Flavor><Mode>`/`bundle<Flavor><Mode>`), e.g.
    /// `assemble{flavor}{Debug,Profile,Release}`.
    pub fn gradle_infix(&self) -> &'static str {
        match self {
            BuildMode::Debug => "Debug",
            BuildMode::Profile => "Profile",
            BuildMode::Release => "Release",
        }
    }

    /// The cargo output-directory segment this mode's artifact lands in
    /// (`target/[<triple>/]{debug,profile,release}/`).
    ///
    /// Cargo names the `dev` profile's directory `debug`, not `dev`, and a
    /// custom profile's directory after the profile itself — so this is
    /// `debug`/`profile`/`release`, deliberately NOT derived from
    /// [`BuildMode::cargo_profile_arg`] (whose debug arm is empty).
    ///
    /// The wasm counterpart of [`BuildMode::gradle_infix`] /
    /// [`BuildMode::xcode_configuration`]: a browser build has no
    /// project-generator of its own to name a configuration for, and instead
    /// has to locate the `.wasm` cargo produced at
    /// `target/{WASM_TARGET_TRIPLE}/<this>/<crate>.wasm` before handing it to
    /// `wasm-bindgen`.
    pub fn cargo_profile_dir(&self) -> &'static str {
        match self {
            BuildMode::Debug => "debug",
            BuildMode::Profile => "profile",
            BuildMode::Release => "release",
        }
    }

    /// Whether a browser build runs `wasm-opt` over the generated `.wasm` by
    /// default in this mode: release only.
    ///
    /// `wasm-opt` is a whole-module optimizer — minutes on a large module,
    /// and it discards the name section a debug build's stack traces are
    /// read through — so a debug/profile browser build wants it off (fast
    /// rebuilds, readable panics) and a shipped release build wants it on.
    /// A `[web] wasm-opt` key overrides this either way
    /// (`manifest::WebSection::wasm_opt_enabled` takes this as its
    /// `mode_default`); it is a default, not a policy.
    pub fn wasm_opt_default(&self) -> bool {
        matches!(self, BuildMode::Release)
    }

    /// The Xcode `-configuration` value for this mode, before any
    /// flavor/scheme suffix (`<Mode>-<Scheme>`) is appended.
    pub fn xcode_configuration(&self) -> &'static str {
        match self {
            BuildMode::Debug => "Debug",
            BuildMode::Profile => "Profile",
            BuildMode::Release => "Release",
        }
    }

    /// The cargo `--features` this mode selects when building a generated app
    /// (Flutter-mode log-level parity): the single source the
    /// desktop/`cargo run`, Android (`-Pfrust.cargoFeatures`), and iOS
    /// (`FRUST_FEATURES`) seams all thread through.
    ///
    /// - **debug / profile** → `["frust/perf-trace", "frust/devtools"]`:
    ///   instrumentation (`frust-perf` frame/startup emission + the
    ///   render-path probes) and the in-app devtools service are both compiled
    ///   IN. Profile keeps them via THESE features, never via a log
    ///   level — `release_max_level_*` keys off `debug_assertions`, which the
    ///   `[profile.profile]` inherits-release profile has OFF, so a log-level
    ///   ceiling would wrongly silence profile perf lines.
    /// - **release** → `["lean"]`: the generated app's own `lean` feature,
    ///   which forwards to `log/release_max_level_warn` — stray info/debug log
    ///   lines are constant-folded out while warn/error crash diagnostics
    ///   survive. `perf-trace` is absent, so a release artifact carries
    ///   neither the emission code nor its `frust-perf` string literals, and
    ///   `devtools` is absent, so it carries no loopback listener, no
    ///   discovery line and no way to drive the UI from off-process. That
    ///   absence is the release-purity guarantee: it comes from the FEATURE
    ///   never being selected here, not from a runtime `debug_assertions`
    ///   check inside the app.
    ///
    /// Never empty (every mode selects at least one feature), so the platform
    /// encoders can treat an empty result as "not applicable" without ever
    /// producing one here.
    pub fn cargo_features(&self) -> &'static [&'static str] {
        match self {
            BuildMode::Debug | BuildMode::Profile => &["frust/perf-trace", "frust/devtools"],
            BuildMode::Release => &["lean"],
        }
    }

    /// [`cargo_features`](Self::cargo_features) for a session that is (or is
    /// not) hot-patched: a **Debug** hot session adds [`HOTPATCH_FEATURE`]
    /// (the seam plus the in-app apply over devtools); every other
    /// combination is the mode's own list. Profile inherits the release
    /// profile, so it has no `debug_assertions` and the runtime would never
    /// consult a jump table, and Release ships no devtools listener to carry
    /// a patch at all — so neither ever compiles the feature in.
    pub fn session_cargo_features(&self, hot: bool) -> &'static [&'static str] {
        match self {
            BuildMode::Debug if hot => &["frust/perf-trace", "frust/devtools", HOTPATCH_FEATURE],
            _ => self.cargo_features(),
        }
    }
}

/// The facade feature a Debug hot session compiles in (see
/// [`BuildMode::session_cargo_features`]).
pub const HOTPATCH_FEATURE: &str = "frust/hotpatch";

/// Plain (clap-free) build flags shared by every command that produces a
/// build. The `frust-cli` clap layer's `build_args::BuildArgs` mirrors this
/// struct field-for-field and converts into it (`BuildArgs::into_drive`).
#[derive(Debug, Clone, Default)]
pub struct BuildArgs {
    /// Build in debug mode (`dev` cargo profile).
    pub debug: bool,
    /// Build in profile mode (release opts + debug symbols + tracing).
    pub profile: bool,
    /// Build in release mode (LTO, stripped, `panic=abort`).
    pub release: bool,
    /// Product flavor to build (maps to a Gradle flavor / Xcode scheme).
    pub flavor: Option<String>,
    /// Compile-time app config, `KEY=VALUE`; repeatable.
    pub defines: Vec<String>,
    /// Semantic version string embedded in the build (e.g. `1.2.3`).
    pub build_name: Option<String>,
    /// Monotonically increasing build number embedded in the build; must be
    /// `>= 1`.
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
    /// `FRUST_TRACE=1` into the defines unless the user already provided a
    /// `FRUST_TRACE` define.
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

        // Auto-inject FRUST_TRACE=1 in profile mode unless user provided it.
        if mode == BuildMode::Profile {
            defines
                .entry("FRUST_TRACE".to_string())
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
    fn cargo_profile_dir_matches_each_mode() {
        // `dev` profile -> `debug/`, not `dev/` — the one place the cargo
        // directory name and the profile name disagree.
        assert_eq!(BuildMode::Debug.cargo_profile_dir(), "debug");
        assert_eq!(BuildMode::Profile.cargo_profile_dir(), "profile");
        assert_eq!(BuildMode::Release.cargo_profile_dir(), "release");
    }

    /// The wasm artifact path a browser build has to resolve, assembled from
    /// the two constants this module owns — proof they compose into the
    /// layout `crates/frust-shell-web/platform/web/README.md`'s recipe reads from.
    #[test]
    fn wasm_artifact_dir_composes_from_the_triple_and_the_profile_dir() {
        assert_eq!(WASM_TARGET_TRIPLE, "wasm32-unknown-unknown");
        assert_eq!(
            format!(
                "target/{WASM_TARGET_TRIPLE}/{}",
                BuildMode::Release.cargo_profile_dir()
            ),
            "target/wasm32-unknown-unknown/release"
        );
    }

    /// `wasm-opt` is a release-only default: a debug or profile browser
    /// build keeps its name section (readable panics) and its fast rebuild.
    #[test]
    fn wasm_opt_defaults_to_release_only() {
        assert!(!BuildMode::Debug.wasm_opt_default());
        assert!(!BuildMode::Profile.wasm_opt_default());
        assert!(BuildMode::Release.wasm_opt_default());
    }

    #[test]
    fn cargo_features_matches_each_mode() {
        // debug/profile compile instrumentation AND the devtools service IN;
        // release swaps to the `lean` log ceiling and carries neither.
        assert_eq!(
            BuildMode::Debug.cargo_features(),
            &["frust/perf-trace", "frust/devtools"]
        );
        assert_eq!(
            BuildMode::Profile.cargo_features(),
            &["frust/perf-trace", "frust/devtools"]
        );
        assert_eq!(BuildMode::Release.cargo_features(), &["lean"]);
    }

    #[test]
    fn session_cargo_features_add_hotpatch_for_debug_hot_sessions_only() {
        assert_eq!(
            BuildMode::Debug.session_cargo_features(true),
            &["frust/perf-trace", "frust/devtools", "frust/hotpatch"]
        );
        assert_eq!(HOTPATCH_FEATURE, "frust/hotpatch");
        for mode in [BuildMode::Debug, BuildMode::Profile, BuildMode::Release] {
            assert_eq!(
                mode.session_cargo_features(false),
                mode.cargo_features(),
                "a cold {mode:?} session is the mode's own list"
            );
            assert!(!mode.cargo_features().contains(&HOTPATCH_FEATURE));
        }
        for mode in [BuildMode::Profile, BuildMode::Release] {
            assert!(
                !mode
                    .session_cargo_features(true)
                    .contains(&HOTPATCH_FEATURE),
                "{mode:?} must never compile the hot-patch feature in"
            );
        }
    }

    #[test]
    fn release_cargo_features_never_contain_perf_trace() {
        assert!(
            !BuildMode::Release
                .cargo_features()
                .contains(&"frust/perf-trace")
        );
    }

    /// The release-purity half of the two-layer devtools gate: a release build
    /// must never select the feature, so the listener/protocol/discovery-line
    /// code is not merely inert but absent from the artifact.
    #[test]
    fn release_cargo_features_never_contain_devtools() {
        assert!(
            !BuildMode::Release
                .cargo_features()
                .contains(&"frust/devtools")
        );
    }

    #[test]
    fn debug_and_profile_cargo_features_select_devtools() {
        for mode in [BuildMode::Debug, BuildMode::Profile] {
            assert!(
                mode.cargo_features().contains(&"frust/devtools"),
                "mode {mode:?}: {:?}",
                mode.cargo_features()
            );
        }
    }

    #[test]
    fn profile_mode_injects_frust_trace() {
        let info = BuildInfo::from_args(
            BuildArgs {
                profile: true,
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("FRUST_TRACE").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn debug_mode_does_not_inject_frust_trace() {
        let info = BuildInfo::from_args(
            BuildArgs {
                debug: true,
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert!(!info.defines.contains_key("FRUST_TRACE"));
    }

    #[test]
    fn release_mode_does_not_inject_frust_trace() {
        let info = BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap();
        assert!(!info.defines.contains_key("FRUST_TRACE"));
    }

    #[test]
    fn user_defined_frust_trace_zero_wins_over_profile_injection() {
        let info = BuildInfo::from_args(
            BuildArgs {
                profile: true,
                defines: vec!["FRUST_TRACE=0".into()],
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("FRUST_TRACE").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn user_defined_frust_trace_custom_wins_over_profile_injection() {
        let info = BuildInfo::from_args(
            BuildArgs {
                profile: true,
                defines: vec!["FRUST_TRACE=custom_value".into()],
                ..args()
            },
            BuildMode::Debug,
        )
        .unwrap();
        assert_eq!(
            info.defines.get("FRUST_TRACE").map(String::as_str),
            Some("custom_value")
        );
    }

    #[test]
    fn profile_default_mode_injects_frust_trace() {
        // Test that profile injection works when profile is the default mode
        let info = BuildInfo::from_args(args(), BuildMode::Profile).unwrap();
        assert_eq!(
            info.defines.get("FRUST_TRACE").map(String::as_str),
            Some("1")
        );
    }
}
