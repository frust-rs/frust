//! The shared `frust.toml` reader.
//!
//! One deserialiser for every manifest section the drive pipelines consume
//! (`[app]`, `[android]`, `[ios]`, `[signing]`, `[desktop]`, `[macos]`,
//! `[windows]`, `[linux]`), replacing the two near-identical
//! `[app]`/`[android]` and `[app]`/`[ios]` structs `android_run::project`
//! and `ios_run::project` each used to carry — those modules now own only
//! their id-resolution logic and read the manifest through [`load`].
//!
//! Unknown sections are ignored (`[deeplink]`, `[flavors]` are documentation
//! for the platform templates, not tooling input), but `[signing]` and its
//! `[signing.env]` subtable, and the desktop-shell sections
//! (`[desktop]`/`[macos]`/`[windows]`/`[linux]`), are `deny_unknown_fields`: a
//! typo in a section that decides whether a release artifact is really
//! signed, or that feeds the desktop packaging pipeline, must fail loudly
//! rather than silently fall back to the default.
//!
//! `ios_build::team` keeps its own deliberately-partial `[ios] team` parse —
//! it must tolerate manifests this reader rejects (it runs before, and
//! independently of, project detection).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The `frust.toml` sections the drive pipelines read.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub app: AppSection,
    #[serde(default)]
    pub android: Option<AndroidSection>,
    #[serde(default)]
    pub ios: Option<IosSection>,
    #[serde(default)]
    pub signing: Option<SigningSection>,
    #[serde(default)]
    pub desktop: Option<DesktopSection>,
    #[serde(default)]
    pub macos: Option<MacosSection>,
    #[serde(default)]
    pub windows: Option<WindowsSection>,
    #[serde(default)]
    pub linux: Option<LinuxSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppSection {
    pub name: String,
    pub org: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AndroidSection {
    #[serde(default)]
    pub identifier: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IosSection {
    #[serde(default)]
    pub identifier: Option<String>,
}

/// `[signing]` — where the project's Gradle build reads Android release
/// signing material from, so `android_build::signing`'s release gate can
/// verify that material really resolves instead of assuming the shipped
/// template's `android/key.properties`.
///
/// Absent section == [`SigningSection::default`], which describes exactly
/// what `frust create` scaffolds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SigningSection {
    /// Signing happens outside anything Frust can inspect (a CI signing
    /// service, a Gradle signing plugin). Waives the release gate — and the
    /// gate says so on every release build, since it can no longer promise
    /// the artifact is release-signed.
    #[serde(default)]
    pub external: bool,
    /// Path to the Java properties file carrying the signing material,
    /// relative to `android/` (absolute paths are taken as-is). Defaults to
    /// `key.properties`, what the generated `build.gradle.kts` reads.
    #[serde(default)]
    pub key_properties: Option<String>,
    /// Prefix on the property keys, for a file that carries several key
    /// sets (`prod.storeFile`, `develop.storeFile`, …). Defaults to none.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Environment variables each value falls back to when the properties
    /// file does not supply it — the CI half of the common
    /// "properties file locally, environment in CI" Gradle rig.
    #[serde(default)]
    pub env: Option<SigningEnv>,
}

/// `[signing.env]` — the env-var *name* each signing value may come from.
/// Only the names are configuration; the values are never read from the
/// manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SigningEnv {
    #[serde(default)]
    pub store_file: Option<String>,
    #[serde(default)]
    pub store_password: Option<String>,
    #[serde(default)]
    pub key_alias: Option<String>,
    #[serde(default)]
    pub key_password: Option<String>,
}

/// The default `[macos] minimum-system-version` when the section, or the
/// field, is absent — see [`MacosSection::minimum_system_version_or_default`].
const DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION: &str = "11.0";

/// `[desktop]` — cross-platform desktop identity shared by every desktop
/// packaging pipeline: window title / menu app name / bundle display name,
/// bundle id / `AppUserModelID` / Wayland `app_id`, and the source icon the
/// `.icns`/`.ico`/hicolor pipeline renders from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DesktopSection {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// `[macos]` — macOS-specific packaging overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MacosSection {
    /// Minimum macOS version the bundle declares it runs on. Defaults to
    /// [`DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION`] when absent — read that
    /// default through [`MacosSection::minimum_system_version_or_default`]
    /// rather than matching on this field directly.
    #[serde(default)]
    pub minimum_system_version: Option<String>,
    /// Codesigning identity (e.g. `"Developer ID Application: ..."`).
    /// Presence enables the codesign step; absence leaves the bundle
    /// unsigned.
    #[serde(default)]
    pub signing_identity: Option<String>,
    /// Opt in to Apple notarization during an `--installer dmg` build:
    /// `cargo-packager` then submits the `.app` it signed to Apple's notary
    /// service, using credentials read from the build's own environment.
    /// Absent (or `false`) is the default, and means those credentials are
    /// **removed** from the packaging tool's environment so an exported
    /// `APPLE_ID`/`APPLE_API_KEY` can't turn a local build into an upload —
    /// see `desktop_build::installer`, which owns the mechanism. Read through
    /// [`MacosSection::notarize_enabled`] rather than matching this directly.
    #[serde(default)]
    pub notarize: Option<bool>,
}

impl MacosSection {
    /// [`MacosSection::minimum_system_version`], or
    /// [`DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION`] when absent.
    pub fn minimum_system_version_or_default(&self) -> &str {
        self.minimum_system_version
            .as_deref()
            .unwrap_or(DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION)
    }

    /// [`MacosSection::notarize`], defaulting to `false` — an absent key and
    /// an explicit `notarize = false` mean the same thing, and both are the
    /// credential-scrubbing default.
    pub fn notarize_enabled(&self) -> bool {
        self.notarize.unwrap_or(false)
    }
}

/// `[windows]` — Windows-specific packaging overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WindowsSection {
    #[serde(default)]
    pub file_version: Option<String>,
    #[serde(default)]
    pub product_version: Option<String>,
}

/// `[linux]` — Linux-specific packaging overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LinuxSection {
    /// `.desktop` `Categories=` entries.
    #[serde(default)]
    pub categories: Option<Vec<String>>,
}

/// `<project_root>/frust.toml`.
pub fn path(project_root: &Path) -> PathBuf {
    project_root.join("frust.toml")
}

/// Reads and parses `<project_root>/frust.toml`. A missing manifest is the
/// "is this a Frust project?" error every entry point reports.
pub fn load(project_root: &Path) -> Result<Manifest> {
    let toml_path = path(project_root);
    if !toml_path.exists() {
        bail!(
            "no `frust.toml` found in `{}` — is this a Frust project? Run `frust create` to scaffold one.",
            project_root.display()
        );
    }
    read(&toml_path)
}

/// Like [`load`], but a missing manifest is `Ok(None)` rather than an error —
/// for callers that run inside an already-detected project and only want a
/// section if there is one (the release-signing gate).
pub fn load_optional(project_root: &Path) -> Result<Option<Manifest>> {
    let toml_path = path(project_root);
    if !toml_path.exists() {
        return Ok(None);
    }
    read(&toml_path).map(Some)
}

fn read(toml_path: &Path) -> Result<Manifest> {
    let raw = fs::read_to_string(toml_path)
        .with_context(|| format!("reading `{}`", toml_path.display()))?;
    parse(&raw).with_context(|| format!("parsing `{}`", toml_path.display()))
}

/// Parses manifest text. Exposed for tests and for callers holding the
/// manifest as a string already.
pub fn parse(raw: &str) -> Result<Manifest> {
    toml::from_str(raw).context("invalid frust.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_scaffolded_shape_without_a_signing_section() {
        let m = parse("[app]\nname = \"myapp\"\norg = \"dev.f0x\"\ndescription = \"x\"\n").unwrap();
        assert_eq!(m.app.name, "myapp");
        assert_eq!(m.app.org, "dev.f0x");
        assert!(m.signing.is_none());
        assert!(m.desktop.is_none());
        assert!(m.macos.is_none());
        assert!(m.windows.is_none());
        assert!(m.linux.is_none());
    }

    #[test]
    fn desktop_sections_default_to_absent_and_the_macos_version_default_holds() {
        let m = parse("[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n").unwrap();
        assert!(m.desktop.is_none());
        assert!(m.macos.is_none());
        assert!(m.windows.is_none());
        assert!(m.linux.is_none());
        // The accessor default applies even to a freshly-constructed section
        // with no `minimum-system-version` field of its own.
        assert_eq!(
            MacosSection::default().minimum_system_version_or_default(),
            "11.0"
        );
    }

    #[test]
    fn parses_a_round_trip_of_every_desktop_section() {
        let m = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nname = \"My App\"\nidentifier = \"com.example.myapp\"\n\
             icon = \"assets/icon-1024.png\"\n\n\
             [macos]\nminimum-system-version = \"12.0\"\n\
             signing-identity = \"Developer ID Application: Example\"\nnotarize = true\n\n\
             [windows]\nfile-version = \"1.0.0.0\"\nproduct-version = \"1.0.0\"\n\n\
             [linux]\ncategories = [\"Utility\"]\n",
        )
        .unwrap();

        let desktop = m.desktop.unwrap();
        assert_eq!(desktop.name.as_deref(), Some("My App"));
        assert_eq!(desktop.identifier.as_deref(), Some("com.example.myapp"));
        assert_eq!(desktop.icon.as_deref(), Some("assets/icon-1024.png"));

        let macos = m.macos.unwrap();
        assert_eq!(macos.minimum_system_version_or_default(), "12.0");
        assert_eq!(
            macos.signing_identity.as_deref(),
            Some("Developer ID Application: Example")
        );
        assert_eq!(macos.notarize, Some(true));
        assert!(macos.notarize_enabled());

        let windows = m.windows.unwrap();
        assert_eq!(windows.file_version.as_deref(), Some("1.0.0.0"));
        assert_eq!(windows.product_version.as_deref(), Some("1.0.0"));

        let linux = m.linux.unwrap();
        assert_eq!(
            linux.categories.as_deref(),
            Some(&["Utility".to_string()][..])
        );
    }

    #[test]
    fn macos_minimum_system_version_defaults_when_the_section_is_present_but_the_field_is_not() {
        let m = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
        )
        .unwrap();
        let macos = m.macos.unwrap();
        assert_eq!(macos.minimum_system_version_or_default(), "11.0");
        assert!(macos.minimum_system_version.is_none());
    }

    /// Notarization is opt-in, and only the exact `notarize = true` opts in:
    /// an absent key and an explicit `false` both leave the packaging step's
    /// Apple credentials scrubbed.
    #[test]
    fn macos_notarize_defaults_to_off_and_only_true_opts_in() {
        let absent = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
        )
        .unwrap()
        .macos
        .unwrap();
        assert_eq!(absent.notarize, None);
        assert!(!absent.notarize_enabled());
        assert!(!MacosSection::default().notarize_enabled());

        let explicit_false = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [macos]\nnotarize = false\n",
        )
        .unwrap()
        .macos
        .unwrap();
        assert_eq!(explicit_false.notarize, Some(false));
        assert!(!explicit_false.notarize_enabled());
    }

    /// The key is kebab-case-strict like every other desktop key: a
    /// plausible-looking `notarise`/`notarization` typo must fail loudly
    /// rather than silently leave a build the author believes notarizes
    /// scrubbing its credentials instead.
    #[test]
    fn a_typo_in_the_macos_notarize_key_is_a_hard_error() {
        for line in ["notarise = true", "notarization = true"] {
            let err = parse(&format!(
                "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[macos]\n{line}\n"
            ))
            .unwrap_err();
            assert!(err.to_string().contains("invalid frust.toml"), "{err}");
        }
    }

    #[test]
    fn a_typo_in_the_desktop_section_is_a_hard_error() {
        let err = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [desktop]\nnaem = \"My App\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid frust.toml"), "{err}");
    }

    #[test]
    fn a_typo_in_the_macos_section_is_a_hard_error() {
        let err = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [macos]\nminimum_system_version = \"11.0\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid frust.toml"), "{err}");
    }

    #[test]
    fn a_typo_in_the_windows_section_is_a_hard_error() {
        let err = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [windows]\nfileversion = \"1.0.0.0\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid frust.toml"), "{err}");
    }

    #[test]
    fn a_typo_in_the_linux_section_is_a_hard_error() {
        let err = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [linux]\ncategory = [\"Utility\"]\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid frust.toml"), "{err}");
    }

    #[test]
    fn ignores_sections_the_drive_pipelines_do_not_read() {
        let m = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[deeplink]\nscheme = \"myapp\"\n\n\
             [flavors]\ndev = { app-id-suffix = \".dev\" }\n",
        )
        .unwrap();
        assert_eq!(m.app.name, "myapp");
    }

    #[test]
    fn parses_a_relocated_signing_section_with_env_fallbacks() {
        let m = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing]\nkey-properties = \"app/keystores/key.properties\"\nprefix = \"prod\"\n\n\
             [signing.env]\nstore-file = \"PROD_STORE_FILE\"\nkey-alias = \"PROD_KEY_ALIAS\"\n",
        )
        .unwrap();
        let signing = m.signing.unwrap();
        assert_eq!(
            signing.key_properties.as_deref(),
            Some("app/keystores/key.properties")
        );
        assert_eq!(signing.prefix.as_deref(), Some("prod"));
        assert!(!signing.external);
        let env = signing.env.unwrap();
        assert_eq!(env.store_file.as_deref(), Some("PROD_STORE_FILE"));
        assert_eq!(env.key_alias.as_deref(), Some("PROD_KEY_ALIAS"));
        assert_eq!(env.store_password, None);
    }

    #[test]
    fn a_typo_in_the_signing_section_is_a_hard_error_not_a_silent_default() {
        // The whole point of `deny_unknown_fields` here: `key_properties`
        // (snake) silently ignored would send the gate back to the default
        // path and green-light a project whose keystore lives elsewhere.
        let err = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing]\nkey_properties = \"app/keystores/key.properties\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid frust.toml"), "{err}");
    }

    #[test]
    fn a_typo_in_the_signing_env_subtable_is_a_hard_error_too() {
        let err = parse(
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [signing.env]\nstorefile = \"PROD_STORE_FILE\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid frust.toml"), "{err}");
    }
}
