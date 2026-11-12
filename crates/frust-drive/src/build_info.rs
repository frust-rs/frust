//! `BuildInfo` funnel between build flags and platform builders.
//!
//! `run` (debug-default) and `build` (release-default) both resolve their
//! flags into a plain [`BuildArgs`] and call [`BuildInfo::from_args`] with
//! their own `default_mode`. The clap layer lives in `frust-cli`
//! (`build_args::BuildArgs`, `#[derive(clap::Args)]`) and converts into this
//! crate's [`BuildArgs`] at the command-handler boundary — `frust-drive`
//! itself has no clap dependency (`docs/ARCHITECTURE.md`).

use std::collections::HashMap;

use crate::facade_key::FacadeKey;
use crate::manifest::Manifest;

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

    /// [`cargo_features`](Self::cargo_features) spelled for an app whose
    /// facade dependency is keyed `key`. The canonical `frust/..` literals
    /// stay the funnel every policy check reads; this mapped list is what
    /// goes on a cargo argv (or a Gradle/Xcode feature list), since cargo
    /// resolves `<dep>/<feature>` against the app's dependency key.
    pub fn cargo_features_for(&self, key: &FacadeKey) -> Vec<String> {
        map_features(self.cargo_features(), key)
    }

    /// [`session_cargo_features`](Self::session_cargo_features) spelled for
    /// an app whose facade dependency is keyed `key` — the argv-side mapping
    /// of the canonical funnel, as [`cargo_features_for`](Self::cargo_features_for).
    pub fn session_cargo_features_for(&self, hot: bool, key: &FacadeKey) -> Vec<String> {
        map_features(self.session_cargo_features(hot), key)
    }
}

/// `features` with each canonical `frust/` prefix rewritten to `key`.
fn map_features(features: &[&str], key: &FacadeKey) -> Vec<String> {
    features.iter().map(|feature| key.map(feature)).collect()
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

/// Where a resolved version value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    /// An explicit `--build-name` / `--build-number` flag.
    Flag,
    /// The `[app] version` / `build-number` keys of `frust.toml`.
    Manifest,
    /// Neither supplied one; the platform applies its own default.
    Default,
}

/// The app version and build number a build uses, with their provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVersion {
    /// The version name, or `None` when the platform default applies.
    pub name: Option<String>,
    /// The build number, or `None` when the platform default applies.
    pub number: Option<u32>,
    /// Where [`name`](Self::name) came from.
    pub name_source: VersionSource,
    /// Where [`number`](Self::number) came from.
    pub number_source: VersionSource,
}

/// The Android `versionName` used when no version is supplied.
pub const DEFAULT_ANDROID_VERSION_NAME: &str = "1.0";
/// The Android `versionCode` used when no build number is supplied.
pub const DEFAULT_ANDROID_VERSION_CODE: u32 = 1;

const UNSET_VERSION_HINT: &str =
    "set [app] version and build-number in frust.toml or pass --build-name / --build-number";

impl ResolvedVersion {
    /// The Android `(versionName, versionCode)` with the defaults applied
    /// for any unset value.
    pub fn android_name_and_code(&self) -> (String, String) {
        (
            self.name
                .clone()
                .unwrap_or_else(|| DEFAULT_ANDROID_VERSION_NAME.to_string()),
            self.number
                .unwrap_or(DEFAULT_ANDROID_VERSION_CODE)
                .to_string(),
        )
    }

    /// The `app version not set` warning for a lane that applies concrete
    /// values itself (Android): names the name and code it will use.
    pub fn android_unset_warning(&self) -> String {
        let (name, code) = self.android_name_and_code();
        format!("warning: app version not set: building {name} ({code}); {UNSET_VERSION_HINT}")
    }

    /// The `app version not set` warning for a lane that leaves an unset
    /// value to the native project (iOS): it names no values it does not know.
    pub fn native_unset_warning() -> String {
        format!(
            "warning: app version not set: the Xcode project's own version applies for the unset value; {UNSET_VERSION_HINT}"
        )
    }

    /// True when either value fell back to the platform default.
    pub fn is_defaulted(&self) -> bool {
        self.name_source == VersionSource::Default || self.number_source == VersionSource::Default
    }
}

/// Resolves the version for a build: each value comes from `info` (a flag)
/// when set, else from the manifest's `[app]` keys, else is left `None` with
/// source [`VersionSource::Default`]. A `None` manifest behaves as one with
/// no version keys.
pub fn resolve_version(info: &BuildInfo, manifest: Option<&Manifest>) -> ResolvedVersion {
    let app = manifest.map(|m| &m.app);
    let (name, name_source) = match (&info.build_name, app.and_then(|a| a.version.as_ref())) {
        (Some(n), _) => (Some(n.clone()), VersionSource::Flag),
        (None, Some(n)) => (Some(n.clone()), VersionSource::Manifest),
        (None, None) => (None, VersionSource::Default),
    };
    let (number, number_source) = match (info.build_number, app.and_then(|a| a.build_number)) {
        (Some(n), _) => (Some(n), VersionSource::Flag),
        (None, Some(n)) => (Some(n), VersionSource::Manifest),
        (None, None) => (None, VersionSource::Default),
    };
    ResolvedVersion {
        name,
        number,
        name_source,
        number_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: &str) -> Manifest {
        crate::manifest::parse(&format!("[app]\nname = \"a\"\norg = \"o\"\n{extra}")).unwrap()
    }

    fn info(name: Option<&str>, number: Option<u32>) -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                build_name: name.map(str::to_string),
                build_number: number,
                ..args()
            },
            BuildMode::Release,
        )
        .unwrap()
    }

    #[test]
    fn flag_beats_manifest() {
        let m = manifest("version = \"1.0\"\nbuild-number = 5\n");
        let r = resolve_version(&info(Some("2.0"), Some(9)), Some(&m));
        assert_eq!(r.name.as_deref(), Some("2.0"));
        assert_eq!(r.number, Some(9));
        assert_eq!(r.name_source, VersionSource::Flag);
        assert_eq!(r.number_source, VersionSource::Flag);
        assert!(!r.is_defaulted());
    }

    #[test]
    fn manifest_beats_default() {
        let m = manifest("version = \"1.0\"\nbuild-number = 5\n");
        let r = resolve_version(&info(None, None), Some(&m));
        assert_eq!(r.name.as_deref(), Some("1.0"));
        assert_eq!(r.number, Some(5));
        assert_eq!(r.name_source, VersionSource::Manifest);
        assert_eq!(r.number_source, VersionSource::Manifest);
        assert!(!r.is_defaulted());
    }

    #[test]
    fn neither_gives_none_with_default_source() {
        let m = manifest("");
        let r = resolve_version(&info(None, None), Some(&m));
        assert_eq!(r.name, None);
        assert_eq!(r.number, None);
        assert_eq!(r.name_source, VersionSource::Default);
        assert_eq!(r.number_source, VersionSource::Default);
        assert!(r.is_defaulted());
    }

    #[test]
    fn mixed_name_from_flag_number_from_manifest() {
        let m = manifest("version = \"1.0\"\nbuild-number = 5\n");
        let r = resolve_version(&info(Some("3.1"), None), Some(&m));
        assert_eq!(r.name.as_deref(), Some("3.1"));
        assert_eq!(r.name_source, VersionSource::Flag);
        assert_eq!(r.number, Some(5));
        assert_eq!(r.number_source, VersionSource::Manifest);
        let partial = manifest("build-number = 5\n");
        assert!(resolve_version(&info(None, None), Some(&partial)).is_defaulted());
    }

    #[test]
    fn missing_manifest_behaves_as_no_keys() {
        let r = resolve_version(&info(None, Some(4)), None);
        assert_eq!(r.name_source, VersionSource::Default);
        assert_eq!(r.number, Some(4));
        assert_eq!(r.number_source, VersionSource::Flag);
        assert!(r.is_defaulted());
    }

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
    fn mapped_cargo_features_follow_the_facade_key() {
        let frust_ui = FacadeKey::new("frust-ui");
        assert_eq!(
            BuildMode::Debug.cargo_features_for(&frust_ui),
            vec!["frust-ui/perf-trace", "frust-ui/devtools"]
        );
        assert_eq!(
            BuildMode::Release.cargo_features_for(&frust_ui),
            vec!["lean"]
        );
        assert_eq!(
            BuildMode::Debug.session_cargo_features_for(true, &frust_ui),
            vec![
                "frust-ui/perf-trace",
                "frust-ui/devtools",
                "frust-ui/hotpatch"
            ]
        );
        assert_eq!(
            BuildMode::Profile.session_cargo_features_for(true, &frust_ui),
            vec!["frust-ui/perf-trace", "frust-ui/devtools"]
        );
        // The canonical funnel itself never changes spelling.
        assert_eq!(
            BuildMode::Debug.cargo_features(),
            &["frust/perf-trace", "frust/devtools"]
        );
        for mode in [BuildMode::Debug, BuildMode::Profile, BuildMode::Release] {
            assert_eq!(
                mode.cargo_features_for(&FacadeKey::default()),
                mode.cargo_features(),
                "the default key is the canonical list, mode {mode:?}"
            );
            for hot in [false, true] {
                assert_eq!(
                    mode.session_cargo_features_for(hot, &FacadeKey::default()),
                    mode.session_cargo_features(hot),
                    "mode {mode:?} hot {hot}"
                );
            }
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
