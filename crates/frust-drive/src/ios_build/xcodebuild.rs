//! `xcodebuild` argument assembly for `frust build ios` (device/simulator
//! `build`) and `frust build ipa` (`archive`). Pure `Vec<String>` builders
//! so the full argv is unit-testable; `ios_build::mod` streams them through
//! the [`ProcessRunner`](crate::process::ProcessRunner).

use crate::identity::AppIdentity;

/// Where the archive is written, relative to the project root (given the
/// shared `-derivedDataPath build/ios`).
pub const ARCHIVE_PATH: &str = "build/ios/archive/Runner.xcarchive";

/// How a build should be signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signing {
    /// Automatic signing with the resolved development team (device/archive).
    Automatic { team: String },
    /// `--no-codesign`: signing explicitly disabled (device only).
    NoCodesign,
    /// Simulator builds: no signing args at all.
    Simulator,
}

/// The inputs shared by a `build` and an `archive` invocation.
#[derive(Debug, Clone)]
pub struct Invocation<'a> {
    pub scheme: &'a str,
    pub configuration: &'a str,
    pub simulator: bool,
    pub signing: Signing,
    /// `--build-name` → `MARKETING_VERSION` override (only when given).
    pub marketing_version: Option<&'a str>,
    /// `--build-number` → `CURRENT_PROJECT_VERSION` override (only when given).
    pub current_project_version: Option<u32>,
    /// base64(`K=V;K=V`) `--define`s → `FRUST_DEFINES` (only when non-empty).
    pub defines_b64: Option<&'a str>,
    /// base64(`feat,feat`) cargo features → `FRUST_FEATURES`; the pbxproj
    /// run-script decodes it and appends `--features <csv>` to its
    /// `cargo build` (only when non-empty).
    pub features_b64: Option<&'a str>,
    /// The resolved identity's bundle identifier → `PRODUCT_BUNDLE_IDENTIFIER`
    /// (only for an explicit identity, see [`Invocation::with_identity`]).
    pub bundle_identifier: Option<&'a str>,
    /// The resolved identity's display name → `FRUST_DISPLAY_NAME`, which the
    /// template's `Info.plist` reads as `CFBundleDisplayName` (only for an
    /// explicit identity).
    pub display_name: Option<&'a str>,
}

/// The identity build settings both argv builders (this module's
/// [`Invocation`] and `ios_run::xcodebuild`'s simulator build) push, in
/// order: `PRODUCT_BUNDLE_IDENTIFIER=<id>` then `FRUST_DISPLAY_NAME=<name>`,
/// each only when given.
pub(crate) fn identity_settings(
    bundle_identifier: Option<&str>,
    display_name: Option<&str>,
) -> Vec<String> {
    let mut settings = Vec::new();
    if let Some(id) = bundle_identifier {
        settings.push(format!("PRODUCT_BUNDLE_IDENTIFIER={id}"));
    }
    if let Some(name) = display_name {
        settings.push(format!("FRUST_DISPLAY_NAME={name}"));
    }
    settings
}

/// [`identity_settings`] for a resolved identity: both settings when it is
/// explicit (an `[ios] identifier` or a declared flavor), none otherwise, so
/// a project that pins nothing keeps its pbxproj values.
pub(crate) fn identity_settings_for(identity: Option<&AppIdentity>) -> Vec<String> {
    match identity.filter(|identity| identity.explicit) {
        Some(identity) => {
            identity_settings(Some(&identity.identifier), Some(&identity.display_name))
        }
        None => Vec::new(),
    }
}

/// The Xcode `ARCHS` value for a simulator build on this host. A simulator
/// `.app` only ever runs on the current machine, and the pbxproj's "Build
/// Rust staticlib" run-script produces a *single*-arch static lib, so the
/// Xcode build must be pinned to the host's native arch: the Release/Profile
/// configs build every arch by default (no `ONLY_ACTIVE_ARCH`, and a generic
/// simulator destination ignores it), which links a slice the staticlib
/// lacks (`ld: symbol(s) not found for architecture x86_64` on Apple
/// Silicon). `x86_64` host → `x86_64`; otherwise (`aarch64`) → `arm64`.
pub(crate) fn host_sim_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "arm64"
    }
}

impl<'a> Invocation<'a> {
    /// Fills [`bundle_identifier`](Self::bundle_identifier) and
    /// [`display_name`](Self::display_name) from `identity` when it is
    /// explicit; clears both otherwise.
    pub fn with_identity(mut self, identity: Option<&'a AppIdentity>) -> Self {
        let explicit = identity.filter(|identity| identity.explicit);
        self.bundle_identifier = explicit.map(|identity| identity.identifier.as_str());
        self.display_name = explicit.map(|identity| identity.display_name.as_str());
        self
    }
}

impl Invocation<'_> {
    fn sdk(&self) -> &'static str {
        if self.simulator {
            "iphonesimulator"
        } else {
            "iphoneos"
        }
    }

    fn destination(&self) -> &'static str {
        // A single argv entry — the space inside the Simulator destination is
        // not a shell split, since `ProcessRunner` takes discrete args.
        if self.simulator {
            "generic/platform=iOS Simulator"
        } else {
            "generic/platform=iOS"
        }
    }

    /// The flags common to `build` and `archive`, minus the trailing verb.
    fn core(&self) -> Vec<String> {
        let mut args = vec![
            "xcodebuild".to_string(),
            "-project".to_string(),
            "ios/Runner.xcodeproj".to_string(),
            "-scheme".to_string(),
            self.scheme.to_string(),
            "-configuration".to_string(),
            self.configuration.to_string(),
            "-sdk".to_string(),
            self.sdk().to_string(),
            "-destination".to_string(),
            self.destination().to_string(),
            "-derivedDataPath".to_string(),
            "build/ios".to_string(),
        ];
        // Simulator builds must be pinned to the host arch (see
        // [`host_sim_arch`]); device/archive builds are arm64-only already.
        if self.simulator {
            args.push(format!("ARCHS={}", host_sim_arch()));
        }
        if let Some(version) = self.marketing_version {
            args.push(format!("MARKETING_VERSION={version}"));
        }
        if let Some(number) = self.current_project_version {
            args.push(format!("CURRENT_PROJECT_VERSION={number}"));
        }
        if let Some(defines) = self.defines_b64 {
            args.push(format!("FRUST_DEFINES={defines}"));
        }
        if let Some(features) = self.features_b64 {
            args.push(format!("FRUST_FEATURES={features}"));
        }
        args.extend(identity_settings(self.bundle_identifier, self.display_name));
        args.extend(self.signing_args());
        args
    }

    fn signing_args(&self) -> Vec<String> {
        match &self.signing {
            // The scaffold's target configs ship `CODE_SIGNING_ALLOWED = NO`
            // so unsigned/simulator builds work without an Apple account; a
            // signed build must re-enable signing on the command line, else
            // xcodebuild silently produces an *unsigned* `.app` even with a
            // team + automatic style set.
            Signing::Automatic { team } => vec![
                format!("DEVELOPMENT_TEAM={team}"),
                "CODE_SIGN_STYLE=Automatic".to_string(),
                "CODE_SIGNING_ALLOWED=YES".to_string(),
                "CODE_SIGNING_REQUIRED=YES".to_string(),
                "-allowProvisioningUpdates".to_string(),
                "-allowProvisioningDeviceRegistration".to_string(),
            ],
            Signing::NoCodesign => vec![
                "CODE_SIGNING_ALLOWED=NO".to_string(),
                "CODE_SIGNING_REQUIRED=NO".to_string(),
                "CODE_SIGN_IDENTITY=".to_string(),
            ],
            Signing::Simulator => Vec::new(),
        }
    }

    /// The full argv (after `xcrun`) for a device/simulator `build`.
    pub fn build_argv(&self) -> Vec<String> {
        let mut args = self.core();
        args.push("build".to_string());
        args
    }

    /// The full argv (after `xcrun`) for an `archive` at [`ARCHIVE_PATH`].
    pub fn archive_argv(&self) -> Vec<String> {
        let mut args = self.core();
        args.push("-archivePath".to_string());
        args.push(ARCHIVE_PATH.to_string());
        args.push("archive".to_string());
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed() -> Invocation<'static> {
        Invocation {
            scheme: "Runner",
            configuration: "Release",
            simulator: false,
            signing: Signing::Automatic {
                team: "ABCDE12345".to_string(),
            },
            marketing_version: None,
            current_project_version: None,
            defines_b64: None,
            features_b64: None,
            bundle_identifier: None,
            display_name: None,
        }
    }

    fn identity(explicit: bool) -> AppIdentity {
        AppIdentity {
            identifier: "dev.f0x.myApp.dev".to_string(),
            display_name: "My App Dev".to_string(),
            explicit,
            flavor: Some("dev".to_string()),
            flavor_declared: explicit,
        }
    }

    #[test]
    fn identity_settings_are_passed_only_for_an_explicit_identity() {
        let explicit = identity(true);
        let argv = signed().with_identity(Some(&explicit)).build_argv();
        let id = argv
            .iter()
            .position(|a| a == "PRODUCT_BUNDLE_IDENTIFIER=dev.f0x.myApp.dev")
            .expect("bundle identifier setting");
        assert_eq!(argv[id + 1], "FRUST_DISPLAY_NAME=My App Dev");
        // Before the signing args, which precede the verb.
        let team = argv
            .iter()
            .position(|a| a == "DEVELOPMENT_TEAM=ABCDE12345")
            .unwrap();
        assert!(id < team);
        assert!(
            signed()
                .with_identity(Some(&explicit))
                .archive_argv()
                .contains(&"FRUST_DISPLAY_NAME=My App Dev".to_string())
        );

        let derived = identity(false);
        for inv in [
            signed().with_identity(Some(&derived)),
            signed().with_identity(None),
        ] {
            let argv = inv.build_argv();
            assert!(
                !argv
                    .iter()
                    .any(|a| a.starts_with("PRODUCT_BUNDLE_IDENTIFIER=")
                        || a.starts_with("FRUST_DISPLAY_NAME=")),
                "{argv:?}"
            );
            assert_eq!(argv, signed().build_argv());
        }
    }

    #[test]
    fn identity_settings_for_mirrors_with_identity() {
        assert_eq!(
            identity_settings_for(Some(&identity(true))),
            [
                "PRODUCT_BUNDLE_IDENTIFIER=dev.f0x.myApp.dev",
                "FRUST_DISPLAY_NAME=My App Dev"
            ]
        );
        assert!(identity_settings_for(Some(&identity(false))).is_empty());
        assert!(identity_settings_for(None).is_empty());
    }

    #[test]
    fn device_signed_build_argv() {
        assert_eq!(
            signed().build_argv(),
            [
                "xcodebuild",
                "-project",
                "ios/Runner.xcodeproj",
                "-scheme",
                "Runner",
                "-configuration",
                "Release",
                "-sdk",
                "iphoneos",
                "-destination",
                "generic/platform=iOS",
                "-derivedDataPath",
                "build/ios",
                "DEVELOPMENT_TEAM=ABCDE12345",
                "CODE_SIGN_STYLE=Automatic",
                "CODE_SIGNING_ALLOWED=YES",
                "CODE_SIGNING_REQUIRED=YES",
                "-allowProvisioningUpdates",
                "-allowProvisioningDeviceRegistration",
                "build",
            ]
        );
    }

    #[test]
    fn device_no_codesign_build_argv() {
        let inv = Invocation {
            signing: Signing::NoCodesign,
            ..signed()
        };
        assert_eq!(
            inv.build_argv(),
            [
                "xcodebuild",
                "-project",
                "ios/Runner.xcodeproj",
                "-scheme",
                "Runner",
                "-configuration",
                "Release",
                "-sdk",
                "iphoneos",
                "-destination",
                "generic/platform=iOS",
                "-derivedDataPath",
                "build/ios",
                "CODE_SIGNING_ALLOWED=NO",
                "CODE_SIGNING_REQUIRED=NO",
                "CODE_SIGN_IDENTITY=",
                "build",
            ]
        );
    }

    #[test]
    fn simulator_build_argv_has_no_signing_args_and_pins_host_arch() {
        let inv = Invocation {
            simulator: true,
            signing: Signing::Simulator,
            ..signed()
        };
        assert_eq!(
            inv.build_argv(),
            vec![
                "xcodebuild".to_string(),
                "-project".to_string(),
                "ios/Runner.xcodeproj".to_string(),
                "-scheme".to_string(),
                "Runner".to_string(),
                "-configuration".to_string(),
                "Release".to_string(),
                "-sdk".to_string(),
                "iphonesimulator".to_string(),
                "-destination".to_string(),
                "generic/platform=iOS Simulator".to_string(),
                "-derivedDataPath".to_string(),
                "build/ios".to_string(),
                // Pinned to the host arch so the single-arch Rust staticlib
                // links (Release builds every arch otherwise).
                format!("ARCHS={}", host_sim_arch()),
                "build".to_string(),
            ]
        );
    }

    #[test]
    fn device_build_argv_omits_archs_override() {
        // Device/archive builds are arm64-only already; no ARCHS pin needed.
        assert!(
            !signed()
                .build_argv()
                .iter()
                .any(|a| a.starts_with("ARCHS="))
        );
    }

    #[test]
    fn version_and_defines_overrides_are_conditional() {
        let inv = Invocation {
            marketing_version: Some("1.2.3"),
            current_project_version: Some(42),
            defines_b64: Some("QUJDPTE="),
            features_b64: Some("ZnJ1c3QvcGVyZi10cmFjZQ=="),
            ..signed()
        };
        let argv = inv.build_argv();
        assert!(argv.contains(&"MARKETING_VERSION=1.2.3".to_string()));
        assert!(argv.contains(&"CURRENT_PROJECT_VERSION=42".to_string()));
        assert!(argv.contains(&"FRUST_DEFINES=QUJDPTE=".to_string()));
        assert!(argv.contains(&"FRUST_FEATURES=ZnJ1c3QvcGVyZi10cmFjZQ==".to_string()));
        // Ordering: overrides precede the signing args, which precede the verb.
        let mv = argv
            .iter()
            .position(|a| a == "MARKETING_VERSION=1.2.3")
            .unwrap();
        let team = argv
            .iter()
            .position(|a| a == "DEVELOPMENT_TEAM=ABCDE12345")
            .unwrap();
        assert!(mv < team);
    }

    #[test]
    fn signed_build_omits_overrides_when_absent() {
        let argv = signed().build_argv();
        assert!(!argv.iter().any(|a| a.starts_with("MARKETING_VERSION=")));
        assert!(
            !argv
                .iter()
                .any(|a| a.starts_with("CURRENT_PROJECT_VERSION="))
        );
        assert!(!argv.iter().any(|a| a.starts_with("FRUST_DEFINES=")));
        assert!(!argv.iter().any(|a| a.starts_with("FRUST_FEATURES=")));
    }

    #[test]
    fn archive_argv_appends_archive_path_and_verb() {
        let argv = signed().archive_argv();
        assert_eq!(argv.last().map(String::as_str), Some("archive"));
        let pos = argv.iter().position(|a| a == "-archivePath").unwrap();
        assert_eq!(argv[pos + 1], ARCHIVE_PATH);
        // Signing args still present for the archive.
        assert!(argv.iter().any(|a| a == "DEVELOPMENT_TEAM=ABCDE12345"));
    }
}
