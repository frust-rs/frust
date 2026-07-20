//! Release-signing gate (spec §12.5, PLAN.md decision 2): `frust build`
//! refuses to produce an unsigned/debug-signed release artifact — unlike
//! the Gradle template's own fallback (task 62), which debug-signs with a
//! warning so Android-Studio-driven builds stay usable, the CLI pipeline
//! makes a missing `android/key.properties` a hard error with guided
//! keytool/key.properties instructions.

use std::path::Path;

use anyhow::{Result, bail};

use crate::build_info::BuildMode;

/// For [`BuildMode::Release`], requires `<android_dir>/key.properties` to
/// exist; profile/debug builds are never gated. On a missing keystore,
/// bails with the exact keytool one-liner and key.properties format a user
/// needs to fix it.
pub fn check_release_signing(android_dir: &Path, mode: BuildMode) -> Result<()> {
    if mode != BuildMode::Release {
        return Ok(());
    }

    let key_properties = android_dir.join("key.properties");
    if key_properties.exists() {
        return Ok(());
    }

    bail!(
        "release build requires `{}`, which is missing.\n\n\
         Generate an upload keystore:\n\n\
         \tkeytool -genkey -v -keystore ~/upload-keystore.jks -keyalg RSA -storetype JKS \
-keysize 2048 -validity 10000 -alias upload\n\n\
         Then create `{}` with:\n\n\
         \tstorePassword=<keystore password>\n\
         \tkeyPassword=<key password>\n\
         \tkeyAlias=upload\n\
         \tstoreFile=<path to the .jks file>\n\n\
         `key.properties` must not be committed.",
        key_properties.display(),
        key_properties.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-signing-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn debug_and_profile_never_gated() {
        let dir = unique_temp_dir("debug-profile-skip");
        check_release_signing(&dir, BuildMode::Debug).unwrap();
        check_release_signing(&dir, BuildMode::Profile).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_without_key_properties_errs_with_keytool_command() {
        let dir = unique_temp_dir("release-missing");
        let err = check_release_signing(&dir, BuildMode::Release).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains(
                "keytool -genkey -v -keystore ~/upload-keystore.jks -keyalg RSA -storetype JKS \
-keysize 2048 -validity 10000 -alias upload"
            ),
            "{message}"
        );
        assert!(message.contains("storePassword"), "{message}");
        assert!(message.contains("keyPassword"), "{message}");
        assert!(message.contains("keyAlias"), "{message}");
        assert!(message.contains("storeFile"), "{message}");
        assert!(message.contains("must not be committed"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_with_key_properties_proceeds() {
        let dir = unique_temp_dir("release-present");
        fs::write(dir.join("key.properties"), "keyAlias=upload\n").unwrap();
        check_release_signing(&dir, BuildMode::Release).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
