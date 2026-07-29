//! The build-launcher modal: artifact type
//! (`apk`/`appbundle`/`ios`/`ipa`, iOS kinds only where host-appropriate) +
//! the `BuildInfo` funnel (mode/flavor/defines) plus artifact-specific flags
//! (split-per-ABI, iOS simulator/codesign, `.ipa` export method), resolving
//! into a [`BuildSpec`] the runner drives directly through `frust-drive`'s
//! `android_build`/`ios_build` pipelines (never shelling out itself) as a
//! session reusing the same tab/log-view machinery every other session
//! uses — see `crate::runner`'s
//! `Effect::LaunchBuild` enactment.
//!
//! Everything here is plain data + pure transitions — no threads, no
//! process — so the modal's focus/toggle/launch-spec logic is unit-tested
//! without a TTY, the same shape `run_config`'s `RunConfig` uses.

use std::path::PathBuf;

use frust_drive::build_info::{BuildArgs, BuildInfo, BuildMode};

/// Android ABI names `frust build apk`'s `--target-platform` maps to —
/// `frust-drive`'s own copy (`android_build::tasks::ALL_ABIS`) is
/// crate-private, so this is a deliberate by-value duplicate, the same
/// duplication `frust-cli`'s `commands/build.rs` already carries as its own
/// local `TARGET_PLATFORMS` table.
const ALL_ABIS: &[&str] = &["arm64-v8a", "armeabi-v7a", "x86_64"];

/// The default `.ipa` export method (mirrors `frust-cli`'s own default choice
/// of the most common method as the modal's starting value).
const DEFAULT_EXPORT_METHOD: &str = "app-store-connect";

/// The artifact type the build launcher targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// An Android APK (`assemble<Flavor><Mode>`).
    Apk,
    /// An Android App Bundle (`bundle<Flavor><Mode>`).
    Appbundle,
    /// An iOS `.app` (device or Simulator).
    Ios,
    /// An archived + exported iOS `.ipa`.
    Ipa,
}

impl ArtifactKind {
    /// The checklist/cycle label.
    pub fn label(self) -> &'static str {
        match self {
            ArtifactKind::Apk => "apk",
            ArtifactKind::Appbundle => "appbundle",
            ArtifactKind::Ios => "ios",
            ArtifactKind::Ipa => "ipa",
        }
    }

    /// Every kind buildable from this host: Android kinds always, iOS kinds
    /// only on a macOS host (Xcode) — mirrors `frust build`'s
    /// `require_macos_host` gate (`frust-cli`'s `commands/build.rs`).
    pub fn available() -> Vec<ArtifactKind> {
        let mut kinds = vec![ArtifactKind::Apk, ArtifactKind::Appbundle];
        if cfg!(target_os = "macos") {
            kinds.push(ArtifactKind::Ios);
            kinds.push(ArtifactKind::Ipa);
        }
        kinds
    }
}

/// Which control in the build-launcher modal has keyboard focus. The rows
/// past `Defines` are conditional on [`BuildLauncher::kind`] — see
/// [`BuildLauncher::focus_order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildFocus {
    /// The artifact-type selector (`←`/`→` cycles apk/appbundle/ios/ipa).
    Kind,
    /// The build-mode selector (`←`/`→` cycles debug/profile/release).
    Mode,
    /// The flavor text field.
    Flavor,
    /// The defines text field (`KEY=VALUE` pairs, space-separated).
    Defines,
    /// The `--split-per-abi` toggle (Apk only).
    SplitPerAbi,
    /// The Simulator-vs-device toggle (Ios only).
    Simulator,
    /// The `--no-codesign` toggle (Ios only).
    NoCodesign,
    /// The `.ipa` export-method text field (Ipa only).
    ExportMethod,
    /// The launch button.
    Launch,
}

/// The resolved per-kind build target a launch requests — the artifact-shape
/// half of [`BuildSpec`], mirroring `frust-drive`'s `AndroidArtifact`/
/// `IosArtifact` enums (kept local since those enums also carry the resolved
/// ABI list/export method the modal computes itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildTargetSpec {
    /// `AndroidArtifact::Apk` — resolved ABIs (always every supported ABI
    /// today; a per-ABI picker is a future refinement) plus split-per-ABI.
    Apk {
        split_per_abi: bool,
        abis: Vec<String>,
    },
    /// `AndroidArtifact::Appbundle`.
    Appbundle,
    /// `IosArtifact::App`.
    IosApp { simulator: bool, codesign: bool },
    /// `IosArtifact::Ipa`.
    Ipa { export_method: String },
}

/// A fully-resolved build launch request — what [`BuildLauncher::launch`]
/// hands the runner via `Effect::LaunchBuild`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSpec {
    /// The project the build runs against.
    pub project_root: PathBuf,
    /// The mode/flavor/defines funnel.
    pub info: BuildInfo,
    /// The artifact shape to build.
    pub target: BuildTargetSpec,
}

/// The build-launcher modal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildLauncher {
    /// The project the build runs against (the active project when the modal
    /// was opened).
    pub project_root: PathBuf,
    pub kind: ArtifactKind,
    pub mode: BuildMode,
    pub flavor: String,
    pub defines: String,
    /// Apk-only: one APK per ABI rather than a fat APK.
    pub split_per_abi: bool,
    /// Ios-only: Simulator build (no signing) vs. a device build.
    pub simulator: bool,
    /// Ios-only: skip codesigning a device build.
    pub no_codesign: bool,
    /// Ipa-only: the export method (`app-store-connect`/`release-testing`/
    /// `debugging`/`enterprise`).
    pub export_method: String,
    pub focus: BuildFocus,
}

impl BuildLauncher {
    /// A fresh modal for `project_root`: apk/release (mirrors `frust build`'s
    /// release-default), no flavor/defines, a fat (non-split) APK, a
    /// Simulator-targeted iOS default (buildable with no signing setup),
    /// focus on the kind selector.
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            kind: ArtifactKind::Apk,
            mode: BuildMode::Release,
            flavor: String::new(),
            defines: String::new(),
            split_per_abi: false,
            simulator: true,
            no_codesign: true,
            export_method: DEFAULT_EXPORT_METHOD.to_string(),
            focus: BuildFocus::Kind,
        }
    }

    /// The focus order for the *current* kind, top to bottom — the
    /// kind-conditional rows (split-per-abi / simulator+codesign /
    /// export-method) only appear for their matching kind.
    fn focus_order(&self) -> Vec<BuildFocus> {
        let mut order = vec![
            BuildFocus::Kind,
            BuildFocus::Mode,
            BuildFocus::Flavor,
            BuildFocus::Defines,
        ];
        match self.kind {
            ArtifactKind::Apk => order.push(BuildFocus::SplitPerAbi),
            ArtifactKind::Appbundle => {}
            ArtifactKind::Ios => {
                order.push(BuildFocus::Simulator);
                order.push(BuildFocus::NoCodesign);
            }
            ArtifactKind::Ipa => order.push(BuildFocus::ExportMethod),
        }
        order.push(BuildFocus::Launch);
        order
    }

    /// Move focus to the next control (wrapping), skipping rows the current
    /// kind doesn't show.
    pub fn focus_next(&mut self) {
        self.step_focus(1);
    }

    /// Move focus to the previous control (wrapping).
    pub fn focus_prev(&mut self) {
        self.step_focus(-1);
    }

    fn step_focus(&mut self, delta: isize) {
        let order = self.focus_order();
        let cur = order.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(order.len() as isize) as usize;
        self.focus = order[next];
    }

    /// Cycle the artifact kind by `delta`, wrapping within [`ArtifactKind::available`]
    /// (so a non-macOS host never lands on an iOS kind), and re-focus the kind
    /// row (mirrors `RunConfig::cycle_mode`).
    pub fn cycle_kind(&mut self, delta: isize) {
        let kinds = ArtifactKind::available();
        let cur = kinds.iter().position(|k| *k == self.kind).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(kinds.len() as isize) as usize;
        self.kind = kinds[next];
        self.focus = BuildFocus::Kind;
    }

    /// Cycle the build mode by `delta`, wrapping debug → profile → release →
    /// debug, and re-focus the mode row.
    pub fn cycle_mode(&mut self, delta: isize) {
        const MODES: [BuildMode; 3] = [BuildMode::Debug, BuildMode::Profile, BuildMode::Release];
        let cur = MODES.iter().position(|m| *m == self.mode).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(MODES.len() as isize) as usize;
        self.mode = MODES[next];
        self.focus = BuildFocus::Mode;
    }

    /// Toggle `split_per_abi` (`Space` on that row).
    pub fn toggle_split_per_abi(&mut self) {
        self.split_per_abi = !self.split_per_abi;
    }

    /// Toggle `simulator` (`Space` on that row).
    pub fn toggle_simulator(&mut self) {
        self.simulator = !self.simulator;
    }

    /// Toggle `no_codesign` (`Space` on that row).
    pub fn toggle_no_codesign(&mut self) {
        self.no_codesign = !self.no_codesign;
    }

    /// Feed a character into the focused text field (flavor/defines/export
    /// method); no-op on any other control.
    pub fn input_char(&mut self, c: char) {
        match self.focus {
            BuildFocus::Flavor => self.flavor.push(c),
            BuildFocus::Defines => self.defines.push(c),
            BuildFocus::ExportMethod => self.export_method.push(c),
            _ => {}
        }
    }

    /// Delete the last character of the focused text field; no-op elsewhere.
    pub fn backspace(&mut self) {
        match self.focus {
            BuildFocus::Flavor => {
                self.flavor.pop();
            }
            BuildFocus::Defines => {
                self.defines.pop();
            }
            BuildFocus::ExportMethod => {
                self.export_method.pop();
            }
            _ => {}
        }
    }

    /// The `BuildInfo` the mode/flavor/defines fields resolve to (mirrors
    /// `RunConfig::build_info`'s parsing: a malformed `defines` token is
    /// skipped rather than failing the launch).
    fn build_info(&self) -> BuildInfo {
        let flavor = {
            let f = self.flavor.trim();
            (!f.is_empty()).then(|| f.to_string())
        };
        let defines: Vec<String> = self
            .defines
            .split_whitespace()
            .filter(|tok| tok.split_once('=').is_some_and(|(k, _)| !k.is_empty()))
            .map(str::to_string)
            .collect();
        BuildInfo::from_args(
            BuildArgs {
                flavor,
                defines,
                ..BuildArgs::default()
            },
            self.mode,
        )
        .expect("pre-filtered defines and a bare mode always parse")
    }

    /// The resolved [`BuildSpec`] the Launch button emits.
    pub fn build_spec(&self) -> BuildSpec {
        let target = match self.kind {
            ArtifactKind::Apk => BuildTargetSpec::Apk {
                split_per_abi: self.split_per_abi,
                abis: ALL_ABIS.iter().map(|s| s.to_string()).collect(),
            },
            ArtifactKind::Appbundle => BuildTargetSpec::Appbundle,
            ArtifactKind::Ios => BuildTargetSpec::IosApp {
                simulator: self.simulator,
                codesign: !self.no_codesign,
            },
            ArtifactKind::Ipa => BuildTargetSpec::Ipa {
                export_method: {
                    let m = self.export_method.trim();
                    if m.is_empty() {
                        DEFAULT_EXPORT_METHOD.to_string()
                    } else {
                        m.to_string()
                    }
                },
            },
        };
        BuildSpec {
            project_root: self.project_root.clone(),
            info: self.build_info(),
            target,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher() -> BuildLauncher {
        BuildLauncher::new(PathBuf::from("/tmp/huddle"))
    }

    #[test]
    fn new_defaults_to_release_apk_focused_on_kind() {
        let l = launcher();
        assert_eq!(l.kind, ArtifactKind::Apk);
        assert_eq!(l.mode, BuildMode::Release);
        assert_eq!(l.focus, BuildFocus::Kind);
    }

    #[test]
    fn available_kinds_always_include_both_android_shapes() {
        let kinds = ArtifactKind::available();
        assert!(kinds.contains(&ArtifactKind::Apk));
        assert!(kinds.contains(&ArtifactKind::Appbundle));
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "asserts the macOS-only iOS kinds are offered; on other hosts see \
                  `ios_kinds_absent_on_non_macos_host`"
    )]
    fn ios_kinds_present_on_macos_host() {
        let kinds = ArtifactKind::available();
        assert!(kinds.contains(&ArtifactKind::Ios));
        assert!(kinds.contains(&ArtifactKind::Ipa));
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "asserts the macOS-only iOS kinds are absent; on macOS see \
                  `ios_kinds_present_on_macos_host`"
    )]
    fn ios_kinds_absent_on_non_macos_host() {
        let kinds = ArtifactKind::available();
        assert!(!kinds.contains(&ArtifactKind::Ios));
        assert!(!kinds.contains(&ArtifactKind::Ipa));
    }

    #[test]
    fn cycle_kind_wraps_within_available_kinds() {
        let mut l = launcher();
        let n = ArtifactKind::available().len();
        for _ in 0..n {
            l.cycle_kind(1);
        }
        assert_eq!(l.kind, ArtifactKind::Apk, "wrapped all the way around");
        assert_eq!(l.focus, BuildFocus::Kind);
    }

    #[test]
    fn cycle_mode_wraps_debug_profile_release() {
        let mut l = launcher();
        l.mode = BuildMode::Debug;
        l.cycle_mode(1);
        assert_eq!(l.mode, BuildMode::Profile);
        l.cycle_mode(1);
        assert_eq!(l.mode, BuildMode::Release);
        l.cycle_mode(1);
        assert_eq!(l.mode, BuildMode::Debug);
        assert_eq!(l.focus, BuildFocus::Mode);
    }

    #[test]
    fn focus_order_is_kind_conditional() {
        let mut l = launcher();
        l.kind = ArtifactKind::Apk;
        assert_eq!(
            l.focus_order(),
            vec![
                BuildFocus::Kind,
                BuildFocus::Mode,
                BuildFocus::Flavor,
                BuildFocus::Defines,
                BuildFocus::SplitPerAbi,
                BuildFocus::Launch,
            ]
        );
        l.kind = ArtifactKind::Appbundle;
        assert_eq!(
            l.focus_order(),
            vec![
                BuildFocus::Kind,
                BuildFocus::Mode,
                BuildFocus::Flavor,
                BuildFocus::Defines,
                BuildFocus::Launch,
            ]
        );
    }

    #[test]
    fn focus_next_wraps_and_skips_no_kind_conditional_rows() {
        let mut l = launcher();
        l.kind = ArtifactKind::Appbundle; // no SplitPerAbi row
        for expected in [
            BuildFocus::Mode,
            BuildFocus::Flavor,
            BuildFocus::Defines,
            BuildFocus::Launch,
            BuildFocus::Kind, // wraps
        ] {
            l.focus_next();
            assert_eq!(l.focus, expected);
        }
    }

    #[test]
    fn text_input_only_edits_the_focused_field() {
        let mut l = launcher();
        l.focus = BuildFocus::Flavor;
        for c in "paid".chars() {
            l.input_char(c);
        }
        l.focus = BuildFocus::Defines;
        for c in "A=1".chars() {
            l.input_char(c);
        }
        l.focus = BuildFocus::Kind;
        l.input_char('z'); // dropped
        assert_eq!(l.flavor, "paid");
        assert_eq!(l.defines, "A=1");
        l.focus = BuildFocus::Defines;
        l.backspace();
        assert_eq!(l.defines, "A=");
    }

    #[test]
    fn toggles_flip_independently() {
        let mut l = launcher();
        assert!(!l.split_per_abi);
        l.toggle_split_per_abi();
        assert!(l.split_per_abi);
        assert!(l.simulator);
        l.toggle_simulator();
        assert!(!l.simulator);
        assert!(l.no_codesign);
        l.toggle_no_codesign();
        assert!(!l.no_codesign);
    }

    #[test]
    fn build_spec_apk_resolves_all_abis_and_split_flag() {
        let mut l = launcher();
        l.split_per_abi = true;
        l.flavor = "paid".into();
        l.defines = "A=1 bad-token".into();
        let spec = l.build_spec();
        assert_eq!(spec.project_root, PathBuf::from("/tmp/huddle"));
        assert_eq!(spec.info.flavor.as_deref(), Some("paid"));
        assert_eq!(spec.info.defines.get("A").map(String::as_str), Some("1"));
        assert!(!spec.info.defines.contains_key("bad-token"));
        match spec.target {
            BuildTargetSpec::Apk {
                split_per_abi,
                abis,
            } => {
                assert!(split_per_abi);
                assert_eq!(abis, vec!["arm64-v8a", "armeabi-v7a", "x86_64"]);
            }
            other => panic!("expected Apk target, got {other:?}"),
        }
    }

    #[test]
    fn build_spec_appbundle_carries_no_extra_flags() {
        let mut l = launcher();
        l.kind = ArtifactKind::Appbundle;
        let spec = l.build_spec();
        assert_eq!(spec.target, BuildTargetSpec::Appbundle);
    }

    #[test]
    fn build_spec_ios_app_resolves_simulator_and_codesign() {
        let mut l = launcher();
        l.kind = ArtifactKind::Ios;
        l.simulator = false;
        l.no_codesign = false;
        let spec = l.build_spec();
        assert_eq!(
            spec.target,
            BuildTargetSpec::IosApp {
                simulator: false,
                codesign: true,
            }
        );
    }

    #[test]
    fn build_spec_ipa_defaults_export_method_when_blank() {
        let mut l = launcher();
        l.kind = ArtifactKind::Ipa;
        l.export_method = "  ".into();
        let spec = l.build_spec();
        assert_eq!(
            spec.target,
            BuildTargetSpec::Ipa {
                export_method: DEFAULT_EXPORT_METHOD.to_string(),
            }
        );
        l.export_method = "release-testing".into();
        let spec = l.build_spec();
        assert_eq!(
            spec.target,
            BuildTargetSpec::Ipa {
                export_method: "release-testing".to_string(),
            }
        );
    }
}
