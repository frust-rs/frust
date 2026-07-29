//! Project detection for `frust run`'s Android path: locates `frust.toml`
//! in the current directory and resolves the Android application id;
//! `android/` is checked separately since the desktop-fallback path doesn't
//! need it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The `[app]`/`[android]` subset of `frust.toml` that `frust run` reads.
#[derive(Debug, Clone, Deserialize)]
struct FrustToml {
    app: AppSection,
    #[serde(default)]
    android: Option<AndroidSection>,
}

#[derive(Debug, Clone, Deserialize)]
struct AppSection {
    name: String,
    org: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AndroidSection {
    #[serde(default)]
    identifier: Option<String>,
}

/// A detected Frust project, resolved from a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// The Android application id (manifest `package`/Gradle
    /// `applicationId`), read from `[android] identifier` if present, else
    /// derived from `org` + `name` — see [`derive_app_id`].
    pub app_id: String,
}

/// Locates and parses `frust.toml` in `cwd`. Does not require
/// `android/` to exist — see [`require_android_dir`] for that check, run
/// only on the Android path.
pub fn detect(cwd: &Path) -> Result<Project> {
    let toml_path = cwd.join("frust.toml");
    if !toml_path.exists() {
        bail!(
            "no `frust.toml` found in `{}` — is this a Frust project? Run `frust create` to scaffold one.",
            cwd.display()
        );
    }
    let raw = fs::read_to_string(&toml_path)
        .with_context(|| format!("reading `{}`", toml_path.display()))?;
    let parsed = parse(&raw).with_context(|| format!("parsing `{}`", toml_path.display()))?;

    let app_id = match parsed.android.and_then(|a| a.identifier) {
        Some(explicit) => {
            crate::android_id::validate(&explicit).with_context(|| {
                format!(
                    "invalid `[android] identifier` `{explicit}` in `{}`",
                    toml_path.display()
                )
            })?;
            explicit
        }
        None => {
            let derived = derive_app_id(&parsed.app.org, &parsed.app.name);
            crate::android_id::validate(&derived).with_context(|| {
                format!(
                    "derived Android application id `{derived}` (from `org`/`name` in `{}`) is invalid",
                    toml_path.display()
                )
            })?;
            derived
        }
    };

    Ok(Project {
        root: cwd.to_path_buf(),
        app_id,
    })
}

fn parse(raw: &str) -> Result<FrustToml> {
    toml::from_str(raw).context("invalid frust.toml")
}

/// `android/` (a generated Gradle project, identified by its wrapper
/// script) must exist for the Android run path.
pub fn require_android_dir(root: &Path) -> Result<PathBuf> {
    let android_dir = root.join("android");
    if !android_dir.join("gradlew").exists() {
        bail!(
            "no `android/` project found in `{}` — this Frust project predates Android \
             support, or `android/` wasn't generated. Re-run `frust create` to add it.",
            root.display()
        );
    }
    Ok(android_dir)
}

/// Derives an Android application id from `org` + `name` via
/// `crate::android_id::derive` — the same derivation
/// `scaffold::context::TemplateContext::android_identifier` uses when
/// rendering a new project's Android template. This is the fallback used
/// when `frust.toml` has no explicit `[android] identifier`; the result
/// is validated by [`detect`] before use.
fn derive_app_id(org: &str, name: &str) -> String {
    crate::android_id::derive(org, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-android-run-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn derive_app_id_joins_org_and_name() {
        assert_eq!(derive_app_id("dev.f0x", "myapp"), "dev.f0x.myapp");
    }

    #[test]
    fn derive_app_id_sanitizes_invalid_characters() {
        assert_eq!(derive_app_id("dev f0x", "my-app"), "dev_f0x.my_app");
    }

    #[test]
    fn detect_errs_without_frust_toml() {
        let dir = unique_temp_dir("no-toml");
        let err = detect(&dir).unwrap_err();
        assert!(err.to_string().contains("frust.toml"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_derives_app_id_when_android_section_absent() {
        let dir = unique_temp_dir("derive");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\ndescription = \"x\"\n",
        )
        .unwrap();
        let project = detect(&dir).unwrap();
        assert_eq!(project.app_id, "dev.f0x.myapp");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_prefers_explicit_android_identifier() {
        let dir = unique_temp_dir("explicit");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[android]\nidentifier = \"dev.f0x.custom\"\n",
        )
        .unwrap();
        let project = detect(&dir).unwrap();
        assert_eq!(project.app_id, "dev.f0x.custom");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_bails_on_hostile_explicit_identifier_before_any_adb_call() {
        // Injection path: `[android] identifier` flows verbatim into `adb
        // shell am start -n <id>/…` / `adb shell pidof <id>` if not
        // validated here — `detect` must reject it before `frust run`
        // ever reaches the adb-invoking pipeline (see android_id.rs).
        let dir = unique_temp_dir("hostile-explicit");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[android]\nidentifier = \"x; rm -rf /\"\n",
        )
        .unwrap();
        let err = detect(&dir).unwrap_err();
        assert!(err.to_string().contains("identifier"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_android_dir_errs_when_gradlew_missing() {
        let dir = unique_temp_dir("no-android");
        let err = require_android_dir(&dir).unwrap_err();
        assert!(err.to_string().contains("android/"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_android_dir_ok_when_gradlew_present() {
        let dir = unique_temp_dir("with-android");
        fs::create_dir_all(dir.join("android")).unwrap();
        fs::write(dir.join("android/gradlew"), "#!/bin/sh\n").unwrap();
        let android_dir = require_android_dir(&dir).unwrap();
        assert_eq!(android_dir, dir.join("android"));
        let _ = fs::remove_dir_all(&dir);
    }
}
