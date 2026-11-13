//! Scheme/configuration resolution and `xcodebuild -list -json` verification
//! for `frust build ios|ipa`. No flavor → scheme `Runner`, configuration
//! `Debug|Profile|Release`; `--flavor <f>` → scheme `<Capitalized f>`,
//! configuration `<Mode>-<Scheme>` (e.g. `Release-Paid`) when the project has
//! that native scheme, else `Runner` + `<Mode>` for a flavor declared in
//! `frust.toml`'s `[flavors]` ([`resolve_verified`]).

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::build_info::BuildInfo;
use crate::process::ProcessRunner;

/// The Xcode scheme + `-configuration` a [`BuildInfo`] maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeConfig {
    pub scheme: String,
    pub configuration: String,
    /// Whether this came from a `--flavor` — a flavored build must verify the
    /// *configuration* (not just the scheme) exists, since a plain build uses
    /// a stock `Debug|Profile|Release` the template always ships.
    pub flavored: bool,
}

/// The `project` object of `xcodebuild -list -json` — only the two fields
/// scheme/configuration verification needs.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectList {
    #[serde(default)]
    pub schemes: Vec<String>,
    #[serde(default)]
    pub configurations: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ListJson {
    project: ProjectList,
}

/// Maps `(mode, flavor)` to `(scheme, configuration)`.
pub fn resolve(info: &BuildInfo) -> SchemeConfig {
    let mode_config = info.mode.xcode_configuration();
    match &info.flavor {
        None => SchemeConfig {
            scheme: "Runner".to_string(),
            configuration: mode_config.to_string(),
            flavored: false,
        },
        Some(flavor) => {
            let scheme = capitalize(flavor);
            SchemeConfig {
                configuration: format!("{mode_config}-{scheme}"),
                scheme,
                flavored: true,
            }
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Runs `xcrun xcodebuild -list -json -project ios/Runner.xcodeproj` in `root`
/// (captured, not streamed) and parses the scheme/configuration listing.
pub fn list(runner: &dyn ProcessRunner, root: &Path) -> Result<ProjectList> {
    let out = runner
        .run_streaming(
            "xcrun",
            &[
                "xcodebuild",
                "-list",
                "-json",
                "-project",
                "ios/Runner.xcodeproj",
            ],
            Some(root),
            &[],
            &mut |_| {},
        )
        .with_context(|| format!("running `xcodebuild -list` in `{}`", root.display()))?;
    if !out.success {
        bail!("`xcodebuild -list` failed:\n{}", out.stderr.trim());
    }
    parse_list(&out.stdout)
}

/// Parses `xcodebuild -list -json` output into its scheme/configuration lists.
fn parse_list(json: &str) -> Result<ProjectList> {
    let parsed: ListJson =
        serde_json::from_str(json).context("parsing `xcodebuild -list -json` output")?;
    Ok(parsed.project)
}

/// Lists the project's schemes and picks the scheme/configuration `info`
/// builds with, given whether its flavor is declared in `frust.toml`'s
/// `[flavors]` (`flavor_declared`, from `identity::resolve`):
///
/// - no flavor: [`resolve`]'s `Runner` + stock configuration;
/// - a flavor with a native scheme `<Flavor>`: [`resolve`]'s scheme and
///   `<Mode>-<Flavor>` configuration, as before `[flavors]` existed;
/// - a declared flavor with no native scheme: `Runner` + the plain `<Mode>`
///   configuration (the identity rides the command line instead);
/// - neither: an error listing the available schemes and naming the
///   `[flavors.<flavor>]` declaration.
///
/// Every chosen scheme (and a flavored configuration) is verified present,
/// erroring with the available names listed verbatim otherwise.
pub fn resolve_verified(
    runner: &dyn ProcessRunner,
    root: &Path,
    info: &BuildInfo,
    flavor_declared: bool,
) -> Result<SchemeConfig> {
    let project = list(runner, root)?;
    select(info, flavor_declared, &project)
}

/// [`resolve_verified`]'s choice against an already-listed project.
fn select(info: &BuildInfo, flavor_declared: bool, project: &ProjectList) -> Result<SchemeConfig> {
    let sc = resolve(info);
    let native = project.schemes.iter().any(|s| s == &sc.scheme);
    if sc.flavored && !native && flavor_declared {
        let plain = SchemeConfig {
            scheme: "Runner".to_string(),
            configuration: info.mode.xcode_configuration().to_string(),
            flavored: false,
        };
        verify(project, &plain, None)?;
        return Ok(plain);
    }
    verify(project, &sc, info.flavor.as_deref())?;
    Ok(sc)
}

/// Verifies the resolved scheme (and, when flavored, configuration) exists in
/// `project`, erroring with the available names listed verbatim otherwise.
/// `flavor` names the requested flavor, so a missing flavor scheme also says
/// how to declare it in `frust.toml`.
fn verify(project: &ProjectList, sc: &SchemeConfig, flavor: Option<&str>) -> Result<()> {
    if !project.schemes.iter().any(|s| s == &sc.scheme) {
        let declare = match flavor {
            Some(flavor) => format!(
                "\nTo build flavor `{flavor}` without a native scheme, declare \
                 `[flavors.{flavor}]` in frust.toml."
            ),
            None => String::new(),
        };
        bail!(
            "scheme `{}` not found in ios/Runner.xcodeproj.\nAvailable schemes: {}{declare}",
            sc.scheme,
            join_or_none(&project.schemes)
        );
    }
    if sc.flavored
        && !project
            .configurations
            .iter()
            .any(|c| c == &sc.configuration)
    {
        bail!(
            "configuration `{}` not found in ios/Runner.xcodeproj.\nAvailable configurations: {}",
            sc.configuration,
            join_or_none(&project.configurations)
        );
    }
    Ok(())
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildInfo, BuildMode};
    use crate::process::{FakeProcessRunner, Output};
    use std::collections::HashMap;

    fn info(mode: BuildMode, flavor: Option<&str>) -> BuildInfo {
        BuildInfo {
            mode,
            flavor: flavor.map(str::to_string),
            defines: HashMap::new(),
            build_name: None,
            build_number: None,
        }
    }

    #[test]
    fn maps_mode_and_flavor_to_scheme_and_configuration() {
        let cases = [
            (BuildMode::Debug, None, "Runner", "Debug", false),
            (BuildMode::Profile, None, "Runner", "Profile", false),
            (BuildMode::Release, None, "Runner", "Release", false),
            (
                BuildMode::Release,
                Some("paid"),
                "Paid",
                "Release-Paid",
                true,
            ),
            (BuildMode::Debug, Some("paid"), "Paid", "Debug-Paid", true),
            (
                BuildMode::Profile,
                Some("paid"),
                "Paid",
                "Profile-Paid",
                true,
            ),
        ];
        for (mode, flavor, scheme, config, flavored) in cases {
            let sc = resolve(&info(mode, flavor));
            assert_eq!(sc.scheme, scheme, "{mode:?}/{flavor:?}");
            assert_eq!(sc.configuration, config, "{mode:?}/{flavor:?}");
            assert_eq!(sc.flavored, flavored, "{mode:?}/{flavor:?}");
        }
    }

    const LIST_JSON: &str = r#"{
        "project": {
            "name": "Runner",
            "schemes": ["Runner", "Paid"],
            "configurations": ["Debug", "Profile", "Release", "Release-Paid"]
        }
    }"#;

    fn list_runner(json: &str) -> FakeProcessRunner {
        FakeProcessRunner::new().with(
            "xcrun xcodebuild -list -json -project ios/Runner.xcodeproj",
            Output {
                success: true,
                stdout: json.to_string(),
                stderr: String::new(),
            },
        )
    }

    #[test]
    fn parse_list_extracts_schemes_and_configurations() {
        let project = parse_list(LIST_JSON).unwrap();
        assert_eq!(project.schemes, vec!["Runner", "Paid"]);
        assert_eq!(
            project.configurations,
            vec!["Debug", "Profile", "Release", "Release-Paid"]
        );
    }

    fn verified(mode: BuildMode, flavor: Option<&str>, declared: bool) -> Result<SchemeConfig> {
        let runner = list_runner(LIST_JSON);
        resolve_verified(
            &runner,
            Path::new("/tmp/app"),
            &info(mode, flavor),
            declared,
        )
    }

    #[test]
    fn verify_passes_for_a_present_scheme() {
        let sc = verified(BuildMode::Release, None, false).unwrap();
        assert_eq!(sc, resolve(&info(BuildMode::Release, None)));
    }

    #[test]
    fn verify_passes_for_a_present_flavored_configuration() {
        let sc = verified(BuildMode::Release, Some("paid"), false).unwrap();
        assert_eq!(
            (sc.scheme.as_str(), sc.configuration.as_str()),
            ("Paid", "Release-Paid")
        );
        assert!(sc.flavored);
    }

    /// A native scheme wins even when `frust.toml` also declares the flavor.
    #[test]
    fn a_native_scheme_is_used_for_a_declared_flavor_too() {
        let sc = verified(BuildMode::Release, Some("paid"), true).unwrap();
        assert_eq!(
            (sc.scheme.as_str(), sc.configuration.as_str()),
            ("Paid", "Release-Paid")
        );
        // ... and its missing configuration is still today's error.
        let err = verified(BuildMode::Debug, Some("paid"), true).unwrap_err();
        assert!(err.to_string().contains("Debug-Paid"), "{err}");
    }

    /// A flavor declared only in `frust.toml` builds `Runner` with the plain
    /// mode configuration.
    #[test]
    fn a_frust_toml_only_flavor_builds_runner_with_the_plain_configuration() {
        for (mode, config) in [
            (BuildMode::Debug, "Debug"),
            (BuildMode::Profile, "Profile"),
            (BuildMode::Release, "Release"),
        ] {
            let sc = verified(mode, Some("dev"), true).unwrap();
            assert_eq!(
                sc,
                SchemeConfig {
                    scheme: "Runner".to_string(),
                    configuration: config.to_string(),
                    flavored: false,
                }
            );
        }
    }

    #[test]
    fn verify_errors_listing_available_schemes_when_missing() {
        let err = verified(BuildMode::Release, Some("free"), false).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Free"), "{message}");
        assert!(message.contains("Available schemes"), "{message}");
        assert!(message.contains("Runner"), "{message}");
        assert!(message.contains("Paid"), "{message}");
        assert!(
            message.contains("declare `[flavors.free]` in frust.toml"),
            "{message}"
        );
    }

    #[test]
    fn a_missing_runner_scheme_does_not_mention_flavors() {
        let runner = list_runner(r#"{"project":{"schemes":["Other"],"configurations":[]}}"#);
        let err = resolve_verified(
            &runner,
            Path::new("/tmp/app"),
            &info(BuildMode::Debug, None),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("`Runner`"), "{err}");
        assert!(!err.to_string().contains("[flavors"), "{err}");
    }

    #[test]
    fn verify_errors_listing_available_configurations_when_flavored_config_missing() {
        // Scheme `Paid` exists, but the `Debug-Paid` configuration doesn't.
        let err = verified(BuildMode::Debug, Some("paid"), false).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Debug-Paid"), "{message}");
        assert!(message.contains("Available configurations"), "{message}");
        assert!(message.contains("Release-Paid"), "{message}");
    }

    #[test]
    fn list_surfaces_failure() {
        let runner = FakeProcessRunner::new().with(
            "xcrun xcodebuild -list -json -project ios/Runner.xcodeproj",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "xcodebuild: error: could not open project".to_string(),
            },
        );
        let err = list(&runner, Path::new("/tmp/app")).unwrap_err();
        assert!(err.to_string().contains("could not open project"), "{err}");
    }
}
