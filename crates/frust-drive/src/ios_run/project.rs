//! Project detection for `frust run`'s iOS simulator path (mirrors
//! `android_run::project`): reads `frust.toml` through the shared
//! `crate::manifest` reader and resolves the iOS bundle identifier;
//! `ios/Runner.xcodeproj` is checked separately via [`require_ios_dir`],
//! since the desktop-fallback path doesn't need it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A detected Frust project, resolved from a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// The iOS bundle identifier (`PRODUCT_BUNDLE_IDENTIFIER` in the
    /// generated Xcode project), read from `[ios] identifier` if present,
    /// else derived from `org` + `name` — see `crate::ios_id::derive`.
    pub bundle_id: String,
}

/// Locates and parses `frust.toml` in `cwd`. Does not require `ios/` to
/// exist — see [`require_ios_dir`] for that check, run only on the iOS path.
pub fn detect(cwd: &Path) -> Result<Project> {
    let toml_path = crate::manifest::path(cwd);
    let parsed = crate::manifest::load(cwd)?;

    let bundle_id = match parsed.ios.and_then(|i| i.identifier) {
        Some(explicit) => {
            crate::ios_id::validate(&explicit).with_context(|| {
                format!(
                    "invalid `[ios] identifier` `{explicit}` in `{}`",
                    toml_path.display()
                )
            })?;
            explicit
        }
        None => {
            let derived = crate::ios_id::derive(&parsed.app.org, &parsed.app.name);
            crate::ios_id::validate(&derived).with_context(|| {
                format!(
                    "derived iOS bundle identifier `{derived}` (from `org`/`name` in `{}`) is invalid",
                    toml_path.display()
                )
            })?;
            derived
        }
    };

    Ok(Project {
        root: cwd.to_path_buf(),
        bundle_id,
    })
}

/// `ios/` (a generated Xcode project, identified by its `.xcodeproj`) must
/// exist for the iOS simulator run path.
pub fn require_ios_dir(root: &Path) -> Result<PathBuf> {
    let ios_dir = root.join("ios");
    if !ios_dir.join("Runner.xcodeproj").exists() {
        anyhow::bail!(
            "no `ios/Runner.xcodeproj` found in `{}` — this Frust project predates iOS \
             support, or `ios/` wasn't generated. Re-run `frust create` to add it.",
            root.display()
        );
    }
    Ok(ios_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-run-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detect_errs_without_frust_toml() {
        let dir = unique_temp_dir("no-toml");
        let err = detect(&dir).unwrap_err();
        assert!(err.to_string().contains("frust.toml"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_derives_bundle_id_when_ios_section_absent() {
        let dir = unique_temp_dir("derive");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\ndescription = \"x\"\n",
        )
        .unwrap();
        let project = detect(&dir).unwrap();
        assert_eq!(project.bundle_id, "dev.f0x.myapp");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_camel_cases_snake_case_project_name() {
        let dir = unique_temp_dir("camel-case");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        let project = detect(&dir).unwrap();
        assert_eq!(project.bundle_id, "dev.f0x.myApp");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_prefers_explicit_ios_identifier() {
        let dir = unique_temp_dir("explicit");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[ios]\nidentifier = \"dev.f0x.custom\"\n",
        )
        .unwrap();
        let project = detect(&dir).unwrap();
        assert_eq!(project.bundle_id, "dev.f0x.custom");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_bails_on_invalid_explicit_identifier_before_any_simctl_call() {
        // The resolved bundle id flows into `simctl launch <id> <bundle-id>`
        // if not validated here — `detect` must reject it before `frust
        // run` ever reaches the device-invoking pipeline (see ios_id.rs).
        let dir = unique_temp_dir("hostile-explicit");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[ios]\nidentifier = \"x; rm -rf /\"\n",
        )
        .unwrap();
        let err = detect(&dir).unwrap_err();
        assert!(err.to_string().contains("identifier"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_ios_dir_errs_when_xcodeproj_missing() {
        let dir = unique_temp_dir("no-ios");
        let err = require_ios_dir(&dir).unwrap_err();
        assert!(err.to_string().contains("ios/Runner.xcodeproj"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn require_ios_dir_ok_when_xcodeproj_present() {
        let dir = unique_temp_dir("with-ios");
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        let ios_dir = require_ios_dir(&dir).unwrap();
        assert_eq!(ios_dir, dir.join("ios"));
        let _ = fs::remove_dir_all(&dir);
    }
}
