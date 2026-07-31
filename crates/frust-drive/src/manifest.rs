//! The shared `frust.toml` reader.
//!
//! One deserialiser for every manifest section the drive pipelines consume
//! (`[app]`, `[android]`, `[ios]`, `[signing]`), replacing the two
//! near-identical `[app]`/`[android]` and `[app]`/`[ios]` structs
//! `android_run::project` and `ios_run::project` each used to carry — those
//! modules now own only their id-resolution logic and read the manifest
//! through [`load`].
//!
//! Unknown sections are ignored (`[deeplink]`, `[flavors]` are documentation
//! for the platform templates, not tooling input), but `[signing]` and its
//! `[signing.env]` subtable are `deny_unknown_fields`: a typo in the section
//! that decides whether a release artifact is really signed must fail loudly
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
