//! The static plugin registry (v1) — three entries mirroring `plugins/`:
//! `shared-preferences` (dependency only), `secure-storage` (dependency plus
//! an optional `biometric-gate` feature wiring in the plugin's own Android
//! library module and the iOS plist key its README documents), and
//! `clean-signals-frust` (dependency, gated on the sibling `clean-signals-rs`
//! checkout).

use super::{Contribution, FeatureSpec, PluginSpec};

/// The `biometric-gate` feature's two contributions — the exact Android/iOS
/// additions `plugins/secure-storage/README.md` §2 documents.
///
/// Android is a **single** contribution: the plugin's own
/// `com.android.library` module. `FrustBiometric.kt` lives inside it (no file
/// is copied into the app), its `AndroidManifest.xml` carries the
/// `USE_BIOMETRIC` permission for the manifest merger to fold in, and its
/// `consumerProguardFiles` carries the R8 keep rule — so the app's manifest
/// and `proguard-rules.pro` are never touched, and nothing can drift from the
/// plugin it came from. iOS still needs a real [`Contribution::PlistEntry`]:
/// Apple requires the usage-description string in the *app's* own
/// `Info.plist`.
const SECURE_STORAGE_BIOMETRIC: &[Contribution] = &[
    Contribution::PlistEntry {
        key: "NSFaceIDUsageDescription",
        value: "Unlock your stored credentials.",
        // No `--` here: an XML comment may not contain a double hyphen.
        comment: "Face ID / Touch ID usage description, added only when the \
                  frust-secure-storage biometric gate is used.",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-secure-storage",
        rel_path: "plugins/secure-storage/platform/android",
    },
];

/// secure-storage's single optional feature.
const SECURE_STORAGE_FEATURES: &[FeatureSpec] = &[FeatureSpec {
    id: "biometric-gate",
    summary: "Per-store Face ID / Touch ID / BiometricPrompt gate: adds the \
              NSFaceIDUsageDescription plist key and includes the plugin's \
              `:frust-secure-storage` Android library module (which carries \
              the FrustBiometric helper, the USE_BIOMETRIC permission, and \
              the R8 keep rule).",
    contributions: SECURE_STORAGE_BIOMETRIC,
}];

const SHARED_PREFERENCES: PluginSpec = PluginSpec {
    id: "shared-preferences",
    summary: "Synchronous, thread-safe key-value store (plaintext).",
    crate_dir: "shared-preferences",
    base: &[Contribution::CargoDep {
        name: "frust-shared-preferences",
    }],
    optional_features: &[],
    requires_sibling: None,
};

const SECURE_STORAGE: PluginSpec = PluginSpec {
    id: "secure-storage",
    summary: "Encrypted key-value store (Keychain / Keystore / keyring) with an \
              optional per-store biometric gate.",
    crate_dir: "secure-storage",
    base: &[Contribution::CargoDep {
        name: "frust-secure-storage",
    }],
    optional_features: SECURE_STORAGE_FEATURES,
    requires_sibling: None,
};

const CLEAN_SIGNALS_FRUST: PluginSpec = PluginSpec {
    id: "clean-signals-frust",
    summary: "Clean-architecture facade binding the clean-signals core to frust \
              (needs the clean-signals-rs sibling checkout).",
    crate_dir: "clean-signals-frust",
    base: &[Contribution::CargoDep {
        name: "clean-signals-frust",
    }],
    optional_features: &[],
    requires_sibling: Some("../clean-signals-rs"),
};

/// The v1 static plugin registry (Vec-factory convention). A caller (the CLI
/// or the TUI Add Plugin dialog) enumerates this to drive selection without
/// hardcoding plugin ids.
pub fn known_plugins() -> Vec<PluginSpec> {
    vec![SHARED_PREFERENCES, SECURE_STORAGE, CLEAN_SIGNALS_FRUST]
}

/// Look up a registry entry by id.
pub(crate) fn find_plugin(id: &str) -> Option<PluginSpec> {
    known_plugins().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_the_three_v1_plugins() {
        let ids: Vec<&str> = known_plugins().iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                "shared-preferences",
                "secure-storage",
                "clean-signals-frust"
            ]
        );
    }

    #[test]
    fn secure_storage_biometric_bundles_the_two_readme_contributions() {
        let spec = find_plugin("secure-storage").unwrap();
        let feature = spec
            .optional_features
            .iter()
            .find(|f| f.id == "biometric-gate")
            .expect("biometric-gate feature");
        // plist key + the plugin's own Gradle module. The Android permission,
        // Kotlin helper and R8 keep rule all ride inside the module now.
        assert_eq!(feature.contributions.len(), 2);
        let has_module = feature.contributions.iter().any(|c| {
            matches!(
                c,
                Contribution::GradleModule { gradle_name, rel_path }
                    if *gradle_name == ":frust-secure-storage"
                        && *rel_path == "plugins/secure-storage/platform/android"
            )
        });
        assert!(
            has_module,
            "biometric-gate must include the plugin's Android library module"
        );
    }

    /// The `rel_path` above is a repo-root-relative directory that must
    /// actually exist in this checkout — a typo would otherwise surface only
    /// as a Gradle failure in a generated app, far from here.
    #[test]
    fn secure_storage_gradle_module_path_exists_in_this_checkout() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for plugin in known_plugins() {
            for contribution in plugin.base.iter().chain(
                plugin
                    .optional_features
                    .iter()
                    .flat_map(|f| f.contributions.iter()),
            ) {
                if let Contribution::GradleModule { rel_path, .. } = contribution {
                    let dir = repo_root.join(rel_path);
                    assert!(
                        dir.join("build.gradle.kts").is_file(),
                        "`{rel_path}` must be a Gradle module directory in this checkout"
                    );
                }
            }
        }
    }

    #[test]
    fn clean_signals_frust_declares_its_sibling_requirement() {
        let spec = find_plugin("clean-signals-frust").unwrap();
        assert_eq!(spec.requires_sibling, Some("../clean-signals-rs"));
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(find_plugin("nope").is_none());
    }
}
