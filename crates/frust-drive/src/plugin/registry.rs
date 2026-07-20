//! The static plugin registry (v1) — three entries mirroring `plugins/`:
//! `shared-preferences` (dependency only), `secure-storage` (dependency plus
//! an optional `biometric-gate` feature bundling the Android/iOS additions the
//! plugin's README documents), and `clean-signals-frust` (dependency, gated on
//! the sibling `clean-signals-rs` checkout).

use super::{Contribution, FeatureSpec, PluginSpec};

/// The `biometric-gate` feature's four contributions — the exact
/// Android/iOS additions `plugins/secure-storage/README.md` §2 documents. The
/// Kotlin helper is embedded verbatim from the plugin's canonical
/// `platform/FrustBiometric.kt` (the copy S05 landed) so the two never drift.
const SECURE_STORAGE_BIOMETRIC: &[Contribution] = &[
    Contribution::ManifestPermission {
        permission: "android.permission.USE_BIOMETRIC",
    },
    Contribution::PlistEntry {
        key: "NSFaceIDUsageDescription",
        value: "Unlock your stored credentials.",
        // No `--` here: an XML comment may not contain a double hyphen.
        comment: "Face ID / Touch ID usage description, added only when the \
                  frust-secure-storage biometric gate is used.",
    },
    Contribution::KotlinFile {
        relative_path: "android/app/src/main/kotlin/dev/frust/FrustBiometric.kt",
        contents: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/secure-storage/platform/FrustBiometric.kt"
        )),
    },
    Contribution::ProguardRule {
        marker: "# frust-secure-storage biometric helper (looked up via JNI/classloader)",
        rule: "-keep class dev.frust.FrustBiometric { *; }",
    },
];

/// secure-storage's single optional feature.
const SECURE_STORAGE_FEATURES: &[FeatureSpec] = &[FeatureSpec {
    id: "biometric-gate",
    summary: "Per-store Face ID / Touch ID / BiometricPrompt gate: adds the \
              USE_BIOMETRIC manifest permission, the NSFaceIDUsageDescription \
              plist key, the FrustBiometric.kt helper, and an R8 keep rule.",
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
    fn secure_storage_biometric_bundles_the_four_readme_contributions() {
        let spec = find_plugin("secure-storage").unwrap();
        let feature = spec
            .optional_features
            .iter()
            .find(|f| f.id == "biometric-gate")
            .expect("biometric-gate feature");
        // manifest permission + plist key + kotlin file + proguard rule.
        assert_eq!(feature.contributions.len(), 4);
        // The embedded Kotlin helper is the real canonical file, not a stub.
        let has_helper = feature.contributions.iter().any(|c| {
            matches!(
                c,
                Contribution::KotlinFile { contents, .. }
                    if contents.contains("object FrustBiometric")
                        && contents.contains("package dev.frust")
            )
        });
        assert!(
            has_helper,
            "biometric-gate must embed the real FrustBiometric.kt"
        );
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
