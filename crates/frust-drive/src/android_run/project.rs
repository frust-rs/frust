//! Project detection for `frust run`'s Android path: reads `frust.toml`
//! (through the shared `crate::manifest` reader) and resolves the Android
//! identity a build produces through `crate::identity`; `android/` is checked
//! separately since the desktop-fallback path doesn't need it.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::identity::{self, AppIdentity, IdentityPlatform};

/// A detected Frust project, resolved from a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    /// The Android application id (manifest `package`/Gradle
    /// `applicationId`) the launch falls back to when the built APK's
    /// badging can't be read: [`identity`](Self::identity)'s identifier —
    /// `[android] identifier` if present, else derived from `org` + `name`
    /// (see `crate::android_id::derive`), plus a declared flavor's `app-id-suffix`.
    pub app_id: String,
    /// The resolved identity for the requested flavor (see
    /// [`detect_flavor`]); [`detect`] resolves it with no flavor.
    pub identity: AppIdentity,
}

/// Locates and parses `frust.toml` in `cwd` and resolves the identity with
/// no flavor. Does not require `android/` to exist — see
/// [`require_android_dir`] for that check, run only on the Android path.
pub fn detect(cwd: &Path) -> Result<Project> {
    detect_flavor(cwd, None)
}

/// [`detect`] for a build of `flavor`: a flavor declared in `[flavors]`
/// applies its suffixes to the identity; an undeclared (native) one leaves
/// the base identity.
pub fn detect_flavor(cwd: &Path, flavor: Option<&str>) -> Result<Project> {
    let toml_path = crate::manifest::path(cwd);
    let parsed = crate::manifest::load(cwd)?;

    // The typed error names the offending identifier and why it is invalid;
    // the identifier is validated here, before it can reach an adb command
    // line (see `crate::android_id`).
    let identity = identity::resolve(&parsed, IdentityPlatform::Android, flavor)
        .map_err(|err| anyhow::anyhow!("{err} (in `{}`)", toml_path.display()))?;

    Ok(Project {
        root: cwd.to_path_buf(),
        app_id: identity.identifier.clone(),
        identity,
    })
}

/// `android/` (a generated Gradle project, identified by its wrapper
/// script) must exist for the Android run path.
pub fn require_android_dir(root: &Path) -> Result<PathBuf> {
    let android_dir = root.join("android");
    if !android_dir.join("gradlew").exists() {
        anyhow::bail!(
            "no `android/` project found in `{}` — this Frust project predates Android \
             support, or `android/` wasn't generated. Re-run `frust create` to add it.",
            root.display()
        );
    }
    Ok(android_dir)
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
            "frust-cli-android-run-test-{tag}-{}-{n}",
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

    /// A declared flavor's suffix reaches the launch fallback id; an
    /// undeclared (native) flavor leaves the base id.
    #[test]
    fn detect_flavor_falls_back_to_the_resolved_identity() {
        let dir = unique_temp_dir("flavor");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n\
             [flavors.dev]\napp-id-suffix = \".dev\"\napp-name-suffix = \" Dev\"\n",
        )
        .unwrap();
        let project = detect_flavor(&dir, Some("dev")).unwrap();
        assert_eq!(project.app_id, "dev.f0x.myapp.dev");
        assert_eq!(project.identity.display_name.as_deref(), Some("Myapp Dev"));
        assert!(project.identity.identifier_stated && project.identity.flavor_declared);
        let native = detect_flavor(&dir, Some("paid")).unwrap();
        assert_eq!(native.app_id, "dev.f0x.myapp");
        assert!(!native.identity.identifier_stated && !native.identity.flavor_declared);
        assert_eq!(native.identity.display_name, None);
        assert_eq!(detect(&dir).unwrap().app_id, "dev.f0x.myapp");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A malformed flavor the build does not select never fails detection;
    /// selecting it fails with the manifest's message and the file path.
    #[test]
    fn detect_judges_only_the_selected_flavor() {
        let dir = unique_temp_dir("bad-flavor");
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[flavors.Dev]\n",
        )
        .unwrap();
        assert_eq!(detect(&dir).unwrap().app_id, "dev.f0x.myapp");
        let err = detect_flavor(&dir, Some("Dev")).unwrap_err().to_string();
        assert!(err.contains("invalid flavor name `Dev`"), "{err}");
        assert!(err.contains("frust.toml"), "{err}");
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
