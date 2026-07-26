//! The static plugin registry (v1) — four entries mirroring `plugins/`:
//! `shared-preferences` (dependency only), `secure-storage` (dependency plus
//! an optional `biometric-gate` feature wiring in the plugin's own Android
//! library module and the iOS plist key its README documents),
//! `clean-signals-frust` (dependency, gated on the sibling `clean-signals-rs`
//! checkout), and `camera` (dependency, an app-side plist key, the plugin's
//! own Android library module, its own iOS Swift package — the first
//! registry entry to use [`Contribution::SwiftPackageRef`] — and an app-crate
//! export shim the task-14 device gate proved necessary; `workflow/plans/
//! features/frust-camera/PLAN.md`'s Affected Modules final accounting).

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

/// `camera`'s base contributions — PLAN.md's Affected Modules final
/// accounting, unwidened since (`workflow/plans/features/frust-camera/
/// PLAN.md`, task 10's own table). No optional features in v1, and no
/// `ManifestPermission`: `android.permission.CAMERA` rides the plugin's own
/// `AndroidManifest.xml`, manifest-merged like secure-storage's
/// `USE_BIOMETRIC` above — only the plist key stays app-side, since Apple
/// requires the usage string in the app's own `Info.plist`.
const CAMERA_BASE: &[Contribution] = &[
    Contribution::CargoDep {
        name: "frust-camera",
    },
    Contribution::PlistEntry {
        key: "NSCameraUsageDescription",
        value: "Take photos and record video.",
        // No `--` here: an XML comment may not contain a double hyphen.
        comment: "Camera usage description, required by Apple in the app's \
                  own Info.plist — no plugin module can supply it, unlike \
                  the GradleModule/SwiftPackageRef contributions below.",
    },
    Contribution::GradleModule {
        gradle_name: ":frust-camera",
        rel_path: "plugins/camera/platform/android",
    },
    // The first registry use of this variant (embedding Phase 3 defined the
    // shape without building it — `plugins/camera/platform/ios`'s own
    // `Package.swift` doc comment). `rel_path` points at the package
    // directory itself (no nested `FrustCamera/` subdirectory — that is
    // just the package/product *name*), mirroring `GradleModule`'s
    // `rel_path` convention above.
    Contribution::SwiftPackageRef {
        package_name: "FrustCamera",
        rel_path: "plugins/camera/platform/ios",
    },
    // Added by the task-14 device gate, which measured the failure this
    // prevents: without it a freshly scaffolded app that added the camera
    // plugin does **not link on iOS** —
    // `Undefined symbols: _frust_camera_session_handle`.
    //
    // That export lives in the plugin crate and is called only from Swift, so
    // nothing in Rust references it and the release profile's `lto = "fat"`
    // internalizes it away before the Swift side links. The catalog never hit
    // this because its own page calls the camera API heavily; an app that
    // merely *added* the plugin — i.e. every Add Plugin user — does not.
    // `apple.rs`'s `ios_exports!` doc says to invoke it "only if the device
    // gate shows the direct export missing". The gate showed exactly that.
    Contribution::AppCrateMacro {
        invocation: "frust_camera::ios_exports!();",
        cfg: Some("target_vendor = \"apple\""),
        comment: "Keeps frust-camera's Swift-called C export alive through \
                  release LTO (see frust_camera::ios_exports).",
    },
];

const CAMERA: PluginSpec = PluginSpec {
    id: "camera",
    summary: "Camera preview (a platform view), still capture, and a YUV/BGRA \
              image stream (CameraX / AVFoundation).",
    crate_dir: "camera",
    base: CAMERA_BASE,
    optional_features: &[],
    requires_sibling: None,
};

/// The v1 static plugin registry (Vec-factory convention). A caller (the CLI
/// or the TUI Add Plugin dialog) enumerates this to drive selection without
/// hardcoding plugin ids.
pub fn known_plugins() -> Vec<PluginSpec> {
    vec![
        SHARED_PREFERENCES,
        SECURE_STORAGE,
        CLEAN_SIGNALS_FRUST,
        CAMERA,
    ]
}

/// Look up a registry entry by id.
pub(crate) fn find_plugin(id: &str) -> Option<PluginSpec> {
    known_plugins().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{AddOutcome, add_plugin};
    use crate::scaffold::{self, TemplateContext};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn registry_lists_the_four_v1_plugins() {
        let ids: Vec<&str> = known_plugins().iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                "shared-preferences",
                "secure-storage",
                "clean-signals-frust",
                "camera",
            ]
        );
    }

    /// The `camera` entry's base contributions, exactly PLAN.md's Affected
    /// Modules final accounting (task 10's own table) — no more, no fewer, in
    /// application order, and no optional features / sibling requirement.
    #[test]
    fn camera_base_contributions_match_the_final_accounting() {
        let spec = find_plugin("camera").unwrap();
        assert_eq!(spec.crate_dir, "camera");
        assert!(spec.optional_features.is_empty());
        assert_eq!(spec.requires_sibling, None);

        assert_eq!(spec.base.len(), 5);
        assert!(matches!(
            spec.base[0],
            Contribution::CargoDep {
                name: "frust-camera"
            }
        ));
        assert!(matches!(
            spec.base[1],
            Contribution::PlistEntry {
                key: "NSCameraUsageDescription",
                ..
            }
        ));
        assert!(matches!(
            spec.base[2],
            Contribution::GradleModule {
                gradle_name: ":frust-camera",
                rel_path: "plugins/camera/platform/android",
            }
        ));
        assert!(matches!(
            spec.base[3],
            Contribution::SwiftPackageRef {
                package_name: "FrustCamera",
                rel_path: "plugins/camera/platform/ios",
            }
        ));
        // The task-14 addition. Pinned by exact invocation because this string
        // IS the idempotence key, and because it is what keeps a fresh
        // scaffold's iOS link from failing on `_frust_camera_session_handle`.
        assert!(matches!(
            spec.base[4],
            Contribution::AppCrateMacro {
                invocation: "frust_camera::ios_exports!();",
                cfg: Some("target_vendor = \"apple\""),
                ..
            }
        ));
    }

    /// The `SwiftPackageRef` counterpart of
    /// `secure_storage_gradle_module_path_exists_in_this_checkout` below — a
    /// typo in `rel_path` would otherwise surface only as a missing
    /// `Package.swift` in a generated Xcode project, far from here.
    #[test]
    fn camera_swift_package_ref_path_exists_in_this_checkout() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = false;
        for plugin in known_plugins() {
            for contribution in plugin.base.iter().chain(
                plugin
                    .optional_features
                    .iter()
                    .flat_map(|f| f.contributions.iter()),
            ) {
                if let Contribution::SwiftPackageRef { rel_path, .. } = contribution {
                    found = true;
                    let dir = repo_root.join(rel_path);
                    assert!(
                        dir.join("Package.swift").is_file(),
                        "`{rel_path}` must be a Swift package directory (a \
                         `Package.swift` file) in this checkout"
                    );
                }
            }
        }
        assert!(found, "expected at least one SwiftPackageRef contribution");
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

    // -------------------------------------------------------------------
    // End-to-end idempotence, driven through the real registry entry (task
    // 10) — a fresh scaffold, `add_plugin(.., "camera", ..)`, applied twice.
    // `apply.rs` itself owns per-contribution unit tests against a
    // stand-in `PKG_REL` (its `SwiftPackageRef` section's own comment);
    // these instead exercise the *actual* `camera` registry entry end to
    // end, the four contributions together.
    // -------------------------------------------------------------------

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-registry-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A scaffold context whose `frust_path` deliberately does NOT exist on
    /// disk, mirroring `apply.rs`'s own `test_context` — the module
    /// directory / package directory need not exist for `add_plugin` to
    /// succeed (that's Gradle/Xcode's problem at build time).
    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: "0.1.0".into(),
            frust_path: "/nonexistent/frust/checkout".into(),
            deeplink_scheme: None,
            deeplink_host: None,
        }
    }

    fn scaffold_project(tag: &str) -> PathBuf {
        let dest = unique_temp_dir(tag);
        scaffold::generate(&dest, &test_context(), None, false, None).unwrap();
        dest
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// No duplicate/dangling/orphaned pbxproj object id and balanced
    /// `Begin`/`End` section markers — a lightweight stand-in for the
    /// scaffold test suite's own `pbx_*` structural checks (private to that
    /// module's tests, and to `apply.rs`'s own, so each of the three test
    /// modules that touches a pbxproj carries its own copy of this check —
    /// `apply.rs`'s `assert_pbxproj_well_formed` doc comment notes the same
    /// duplication against `scaffold/mod.rs`'s).
    fn assert_pbxproj_well_formed(pbxproj: &str) {
        assert_eq!(
            pbxproj.matches("/* Begin ").count(),
            pbxproj.matches("/* End ").count(),
            "unbalanced PBX sections:\n{pbxproj}"
        );
        let mut defined = Vec::new();
        for line in pbxproj.lines() {
            let Some(rest) = line.strip_prefix("\t\t") else {
                continue;
            };
            if rest.starts_with('\t') || !rest.contains(" = {") {
                continue;
            }
            let id = rest.split_whitespace().next().unwrap_or_default();
            if id.len() == 24 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                defined.push(id.to_string());
            }
        }
        let mut unique = defined.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            defined.len(),
            "duplicate PBX object id(s):\n{pbxproj}"
        );
    }

    #[test]
    fn camera_add_plugin_applies_every_contribution_and_reapply_is_idempotent() {
        let root = scaffold_project("camera-idempotence");

        let first = add_plugin(&root, "camera", &[]).unwrap();
        assert_eq!(first.plugin_id, "camera");
        assert_eq!(first.items.len(), 5, "{first:?}");
        assert!(
            first.items.iter().all(|i| i.outcome == AddOutcome::Applied),
            "{first:?}"
        );

        // The Cargo dependency, Info.plist key, Gradle module wiring, Swift
        // package reference and app-crate export shim all actually landed.
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("frust-camera"), "{cargo}");
        let plist = fs::read_to_string(root.join("ios/Runner/Info.plist")).unwrap();
        assert!(plist.contains("NSCameraUsageDescription"), "{plist}");
        let settings = fs::read_to_string(root.join("android/settings.gradle.kts")).unwrap();
        assert!(
            settings.contains("include(\":frust-camera\")"),
            "{settings}"
        );
        let app_build = fs::read_to_string(root.join("android/app/build.gradle.kts")).unwrap();
        assert!(
            app_build.contains("implementation(project(\":frust-camera\"))"),
            "{app_build}"
        );
        let pbxproj =
            fs::read_to_string(root.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbxproj.contains("FrustCamera"), "{pbxproj}");
        assert_pbxproj_well_formed(&pbxproj);
        let lib_rs = fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("frust_camera::ios_exports!();"), "{lib_rs}");
        assert!(
            lib_rs.contains("#[cfg(target_vendor = \"apple\")]"),
            "{lib_rs}"
        );

        let after_first = snapshot_tree(&root);
        let second = add_plugin(&root, "camera", &[]).unwrap();
        assert_eq!(second.items.len(), 5);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "{second:?}"
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The half-applied completion case for `camera` specifically (task 10):
    /// delete the Gradle include this entry's `GradleModule` contribution
    /// added, then re-apply — the missing half must complete (report
    /// `Applied`), not skip on the app-side dependency line it still finds
    /// present, and the surviving half must not be duplicated.
    #[test]
    fn camera_half_applied_gradle_include_completes_on_reapply() {
        let root = scaffold_project("camera-half-applied");
        add_plugin(&root, "camera", &[]).unwrap();

        let settings_path = root.join("android/settings.gradle.kts");
        let wired = fs::read_to_string(&settings_path).unwrap();
        assert!(wired.contains("include(\":frust-camera\")"), "{wired}");

        // Strip exactly the block `GradleModule` appended after the
        // `// frust:plugin-includes` anchor (that anchor is the last line
        // of a freshly scaffolded `settings.gradle.kts`, so the addition is
        // everything after it).
        let anchor = "// frust:plugin-includes";
        let anchor_at = wired.find(anchor).expect("plugin-includes anchor");
        let cut_at = anchor_at + anchor.len();
        let (before, added) = wired.split_at(cut_at);
        assert!(
            added.contains(":frust-camera"),
            "expected the camera GradleModule addition after the anchor: {added}"
        );
        fs::write(&settings_path, before).unwrap();

        let app_build_before = fs::read(root.join("android/app/build.gradle.kts")).unwrap();

        let report = add_plugin(&root, "camera", &[]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-camera"))
            .expect("a Gradle module line item");
        assert_eq!(
            item.outcome,
            AddOutcome::Applied,
            "a half-applied module must complete and report Applied: {report:?}"
        );

        let repaired = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            repaired.matches("include(\":frust-camera\")").count(),
            1,
            "the include line must be restored exactly once:\n{repaired}"
        );
        assert_eq!(
            repaired.matches("if (path == \":frust-camera\")").count(),
            1,
            "the build-dir redirect must be restored exactly once, not duplicated:\n{repaired}"
        );

        // The app-side dependency half was already wired and must not be
        // touched/duplicated.
        assert_eq!(
            fs::read(root.join("android/app/build.gradle.kts")).unwrap(),
            app_build_before,
            "the already-wired app dependency half must be left untouched"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
