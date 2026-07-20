//! [`add_plugin`]: apply a registry plugin's contributions to a generated
//! project. Every edit is idempotent and format-preserving; a file that fails
//! to parse (or lacks its insertion anchor) is never rewritten.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, InlineTable, Item, Value};

use super::registry::find_plugin;
use super::{AddItem, AddOutcome, AddReport, Contribution, PluginAddError, PluginSpec};

/// Project-relative paths of the files a contribution edits.
const CARGO_TOML_REL: &str = "Cargo.toml";
const MANIFEST_REL: &str = "android/app/src/main/AndroidManifest.xml";
const PLIST_REL: &str = "ios/Runner/Info.plist";
const PROGUARD_REL: &str = "android/app/proguard-rules.pro";

/// Apply plugin `id` (with the requested optional `features`) to the generated
/// project at `project_root`, returning a per-edit [`AddReport`]. Idempotent:
/// a second call with the same arguments reports every item
/// [`AddOutcome::AlreadyPresent`] and writes nothing.
pub fn add_plugin(
    project_root: &Path,
    id: &str,
    features: &[&str],
) -> Result<AddReport, PluginAddError> {
    let spec = find_plugin(id).ok_or_else(|| PluginAddError::UnknownPlugin(id.to_string()))?;

    // Validate every requested feature up front, before mutating anything.
    for feat in features {
        if !spec.optional_features.iter().any(|f| f.id == *feat) {
            return Err(PluginAddError::UnknownFeature {
                plugin: id.to_string(),
                feature: (*feat).to_string(),
            });
        }
    }

    // Read + parse Cargo.toml once: it is the source of the `frust` path dep
    // every plugin path is derived from, and never rewritten if it can't parse.
    let cargo_path = project_root.join(CARGO_TOML_REL);
    let cargo_src = fs::read_to_string(&cargo_path)
        .map_err(|_| PluginAddError::MissingProjectFile(CARGO_TOML_REL.to_string()))?;
    let mut doc = cargo_src
        .parse::<DocumentMut>()
        .map_err(|e| PluginAddError::UnparseableCargoToml(e.to_string()))?;

    let frust_path = frust_dep_path(&doc).ok_or(PluginAddError::NoFrustDependency)?;

    // Facade plugins (clean-signals-frust) need a sibling checkout present;
    // check before any edit so a missing sibling leaves the tree untouched.
    if let Some(sibling) = spec.requires_sibling {
        let expected = resolve_sibling(project_root, &frust_path, sibling);
        if !expected.exists() {
            return Err(PluginAddError::SiblingCheckoutMissing {
                sibling: sibling.to_string(),
                expected,
            });
        }
    }

    // Base contributions, then the selected features' (in registry order for
    // a stable report regardless of the caller's feature ordering).
    let feature_contribs = spec
        .optional_features
        .iter()
        .filter(|f| features.contains(&f.id))
        .flat_map(|f| f.contributions.iter());
    let contributions = spec.base.iter().chain(feature_contribs);

    let mut items = Vec::new();
    let mut cargo_changed = false;
    for contribution in contributions {
        let outcome = apply_contribution(
            contribution,
            &spec,
            project_root,
            &frust_path,
            &mut doc,
            &mut cargo_changed,
        )?;
        items.push(AddItem {
            description: contribution.describe(),
            outcome,
        });
    }

    // A single write for the Cargo.toml, only if a dep was actually inserted —
    // so an all-`AlreadyPresent` run touches no file (byte-identical tree).
    if cargo_changed {
        write_file(&cargo_path, CARGO_TOML_REL, &doc.to_string())?;
    }

    Ok(AddReport {
        plugin_id: id.to_string(),
        items,
    })
}

fn apply_contribution(
    contribution: &Contribution,
    spec: &PluginSpec,
    project_root: &Path,
    frust_path: &str,
    doc: &mut DocumentMut,
    cargo_changed: &mut bool,
) -> Result<AddOutcome, PluginAddError> {
    match contribution {
        Contribution::CargoDep { name } => {
            apply_cargo_dep(doc, cargo_changed, name, spec.crate_dir, frust_path)
        }
        Contribution::ManifestPermission { permission } => {
            apply_manifest_permission(project_root, permission)
        }
        Contribution::PlistEntry {
            key,
            value,
            comment,
        } => apply_plist_entry(project_root, key, value, comment),
        Contribution::KotlinFile {
            relative_path,
            contents,
        } => apply_kotlin_file(project_root, relative_path, contents),
        Contribution::ProguardRule { marker, rule } => {
            apply_proguard_rule(project_root, marker, rule)
        }
    }
}

/// The `frust` dependency's `path` value (relative or absolute, as written),
/// or `None` if there is no `frust = { path = ... }` dep to derive from.
fn frust_dep_path(doc: &DocumentMut) -> Option<String> {
    let deps = doc.get("dependencies")?.as_table_like()?;
    let frust = deps.get("frust")?;
    let path = frust.as_table_like()?.get("path")?.as_str()?;
    Some(path.to_string())
}

/// Resolve a plugin's absolute crate directory from the project's `frust` path
/// dep: `<frust>/../../plugins/<crate_dir>` — the same walk the scaffold's
/// template uses for its own commented plugin example (`frust` resolves to the
/// facade crate dir, two levels below the repo root).
fn plugin_dep_path(frust_path: &str, crate_dir: &str) -> String {
    format!("{frust_path}/../../plugins/{crate_dir}")
}

/// Resolve a `requires_sibling` path to an absolute location for the on-disk
/// existence check: the frust repo root (two levels above the `frust` facade
/// crate dir) joined with the sibling's repo-root-relative path.
fn resolve_sibling(project_root: &Path, frust_path: &str, sibling: &str) -> PathBuf {
    let frust_abs = if Path::new(frust_path).is_absolute() {
        PathBuf::from(frust_path)
    } else {
        project_root.join(frust_path)
    };
    frust_abs.join("..").join("..").join(sibling)
}

fn apply_cargo_dep(
    doc: &mut DocumentMut,
    changed: &mut bool,
    name: &str,
    crate_dir: &str,
    frust_path: &str,
) -> Result<AddOutcome, PluginAddError> {
    let deps = doc
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
        // Unreachable in practice — `frust_dep_path` already read a path from
        // `[dependencies].frust` — but avoid an `unwrap` at an FFI-free core.
        .ok_or(PluginAddError::NoFrustDependency)?;

    if deps.contains_key(name) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let mut inline = InlineTable::new();
    inline.insert("path", Value::from(plugin_dep_path(frust_path, crate_dir)));
    deps.insert(name, Item::Value(Value::InlineTable(inline)));
    *changed = true;
    Ok(AddOutcome::Applied)
}

fn apply_manifest_permission(
    project_root: &Path,
    permission: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(MANIFEST_REL);
    let src = read_required(&path, MANIFEST_REL)?;
    if src.contains(&format!("android:name=\"{permission}\"")) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let line = format!("    <uses-permission android:name=\"{permission}\" />\n");
    let out = insert_before_anchor(&src, "</manifest>", &line, MANIFEST_REL)?;
    write_file(&path, MANIFEST_REL, &out)?;
    Ok(AddOutcome::Applied)
}

fn apply_plist_entry(
    project_root: &Path,
    key: &str,
    value: &str,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(PLIST_REL);
    let src = read_required(&path, PLIST_REL)?;
    if src.contains(&format!("<key>{key}</key>")) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    // Insert before the root dict's close (the last `</dict>`, immediately
    // preceding `</plist>`), tab-indented like the surrounding entries.
    let block = format!("\t<!-- {comment} -->\n\t<key>{key}</key>\n\t<string>{value}</string>\n");
    let out = insert_before_anchor(&src, "</dict>", &block, PLIST_REL)?;
    write_file(&path, PLIST_REL, &out)?;
    Ok(AddOutcome::Applied)
}

fn apply_kotlin_file(
    project_root: &Path,
    relative_path: &str,
    contents: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(relative_path);
    if path.exists() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| PluginAddError::Io {
            path: relative_path.to_string(),
            message: e.to_string(),
        })?;
    }
    write_file(&path, relative_path, contents)?;
    Ok(AddOutcome::Applied)
}

fn apply_proguard_rule(
    project_root: &Path,
    marker: &str,
    rule: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(PROGUARD_REL);
    let src = read_required(&path, PROGUARD_REL)?;
    if src.contains(marker) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let mut out = src;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(marker);
    out.push('\n');
    out.push_str(rule);
    out.push('\n');
    write_file(&path, PROGUARD_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Insert `insertion` immediately before the last occurrence of `anchor`,
/// preserving all surrounding bytes. Errors [`PluginAddError::MalformedProjectFile`]
/// if the anchor is absent (the file is never rewritten in that case).
fn insert_before_anchor(
    src: &str,
    anchor: &str,
    insertion: &str,
    rel: &str,
) -> Result<String, PluginAddError> {
    let idx = src
        .rfind(anchor)
        .ok_or_else(|| PluginAddError::MalformedProjectFile(rel.to_string()))?;
    let mut out = String::with_capacity(src.len() + insertion.len());
    out.push_str(&src[..idx]);
    out.push_str(insertion);
    out.push_str(&src[idx..]);
    Ok(out)
}

fn read_required(path: &Path, rel: &str) -> Result<String, PluginAddError> {
    fs::read_to_string(path).map_err(|_| PluginAddError::MissingProjectFile(rel.to_string()))
}

fn write_file(path: &Path, rel: &str, contents: &str) -> Result<(), PluginAddError> {
    fs::write(path, contents).map_err(|e| PluginAddError::Io {
        path: rel.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaffold::{self, TemplateContext};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-plugin-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A scaffold context whose `frust_path` deliberately does NOT exist on
    /// disk — so the derived clean-signals-rs sibling is absent (missing-sibling
    /// error case) and no accidental real checkout is picked up.
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

    /// Render a fresh scaffold into a tempdir and return its root.
    fn scaffold_project(tag: &str) -> PathBuf {
        let dest = unique_temp_dir(tag);
        scaffold::generate(&dest, &test_context(), None, false, None).unwrap();
        dest
    }

    /// Snapshot every file under `root` as `relpath -> bytes` for a
    /// byte-identity comparison.
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

    #[test]
    fn add_secure_storage_with_biometric_applies_every_edit() {
        let root = scaffold_project("ss-biometric");

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert_eq!(report.plugin_id, "secure-storage");
        // dep + manifest + plist + kotlin + proguard = 5 items, all applied.
        assert_eq!(report.items.len(), 5);
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied)
        );
        assert_eq!(report.counts(), (5, 0));

        // Cargo.toml: the dep landed with a path derived from the frust dep.
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("frust-secure-storage"), "{cargo}");
        assert!(
            cargo.contains("/nonexistent/frust/checkout/../../plugins/secure-storage"),
            "{cargo}"
        );

        // AndroidManifest.xml: the permission is inside the (still well-formed)
        // manifest.
        let manifest = fs::read_to_string(root.join(MANIFEST_REL)).unwrap();
        assert!(
            manifest
                .contains("<uses-permission android:name=\"android.permission.USE_BIOMETRIC\" />"),
            "{manifest}"
        );
        assert!(manifest.trim_end().ends_with("</manifest>"), "{manifest}");

        // Info.plist: the key + comment landed before the closing dict.
        let plist = fs::read_to_string(root.join(PLIST_REL)).unwrap();
        assert!(
            plist.contains("<key>NSFaceIDUsageDescription</key>"),
            "{plist}"
        );
        assert!(
            plist.contains("<string>Unlock your stored credentials.</string>"),
            "{plist}"
        );
        assert!(plist.trim_end().ends_with("</plist>"), "{plist}");
        // XML comment well-formedness: no illegal double hyphen in the comment.
        let comment_start = plist.find("<!-- Face ID").unwrap();
        let comment_end = plist[comment_start..].find("-->").unwrap() + comment_start;
        assert!(
            !plist[comment_start + 4..comment_end].contains("--"),
            "plist comment must not contain `--`"
        );

        // Kotlin helper: written at the fixed dev/frust package path, verbatim.
        let kt = root.join("android/app/src/main/kotlin/dev/frust/FrustBiometric.kt");
        let kt_src = fs::read_to_string(&kt).unwrap();
        assert!(kt_src.contains("object FrustBiometric"), "{kt_src}");
        assert!(kt_src.contains("package dev.frust"), "{kt_src}");

        // proguard-rules.pro: the marker + keep rule appended, existing rules
        // preserved.
        let proguard = fs::read_to_string(root.join(PROGUARD_REL)).unwrap();
        assert!(
            proguard.contains("-keep class dev.frust.FrustBiometric { *; }"),
            "{proguard}"
        );
        assert!(
            proguard.contains("looked up via JNI/classloader"),
            "{proguard}"
        );
        // A pre-existing scaffold rule survived the append.
        assert!(
            proguard.contains("-keep class dev.accesskit.android.** { *; }"),
            "{proguard}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn applying_twice_is_byte_identical_and_all_already_present() {
        let root = scaffold_project("idempotent");

        let first = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert!(first.items.iter().all(|i| i.outcome == AddOutcome::Applied));
        let after_first = snapshot_tree(&root);

        let second = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert_eq!(second.items.len(), 5);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "second apply must report every item AlreadyPresent"
        );
        let after_second = snapshot_tree(&root);

        assert_eq!(
            after_first, after_second,
            "a second apply must leave a byte-identical tree (the idempotency gate)"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn storage_only_touches_nothing_but_cargo_toml() {
        let root = scaffold_project("storage-only");

        let manifest_before = fs::read(root.join(MANIFEST_REL)).unwrap();
        let plist_before = fs::read(root.join(PLIST_REL)).unwrap();
        let proguard_before = fs::read(root.join(PROGUARD_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &[]).unwrap();
        // Only the Cargo.toml dependency is applied.
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        assert!(report.items[0].description.contains("frust-secure-storage"));

        // No platform file was touched.
        assert_eq!(fs::read(root.join(MANIFEST_REL)).unwrap(), manifest_before);
        assert_eq!(fs::read(root.join(PLIST_REL)).unwrap(), plist_before);
        assert_eq!(fs::read(root.join(PROGUARD_REL)).unwrap(), proguard_before);
        // And no Kotlin helper appeared.
        assert!(
            !root
                .join("android/app/src/main/kotlin/dev/frust/FrustBiometric.kt")
                .exists()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_plugin_errors() {
        let root = scaffold_project("unknown-plugin");
        let err = add_plugin(&root, "not-a-plugin", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::UnknownPlugin(id) if id == "not-a-plugin"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_feature_errors() {
        let root = scaffold_project("unknown-feature");
        let err = add_plugin(&root, "secure-storage", &["not-a-feature"]).unwrap_err();
        assert!(matches!(
            err,
            PluginAddError::UnknownFeature { plugin, feature }
                if plugin == "secure-storage" && feature == "not-a-feature"
        ));
        // The failed feature validation left Cargo.toml untouched.
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(!cargo.contains("frust-secure-storage"), "{cargo}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_frust_dep_errors() {
        let dir = unique_temp_dir("no-frust-dep");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();

        let err = add_plugin(&dir, "secure-storage", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::NoFrustDependency));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_cargo_toml_errors() {
        let dir = unique_temp_dir("no-cargo");
        fs::create_dir_all(&dir).unwrap();
        let err = add_plugin(&dir, "secure-storage", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::MissingProjectFile(f) if f == "Cargo.toml"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unparseable_cargo_toml_is_never_rewritten() {
        let dir = unique_temp_dir("bad-cargo");
        fs::create_dir_all(&dir).unwrap();
        let bad = "this is [not valid toml";
        fs::write(dir.join("Cargo.toml"), bad).unwrap();
        let err = add_plugin(&dir, "secure-storage", &[]).unwrap_err();
        assert!(matches!(err, PluginAddError::UnparseableCargoToml(_)));
        // Untouched.
        assert_eq!(fs::read_to_string(dir.join("Cargo.toml")).unwrap(), bad);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_sibling_for_clean_signals_frust_errors() {
        // test_context's frust_path is nonexistent, so the derived
        // clean-signals-rs sibling is absent.
        let root = scaffold_project("clean-signals-missing-sibling");
        let err = add_plugin(&root, "clean-signals-frust", &[]).unwrap_err();
        assert!(matches!(
            err,
            PluginAddError::SiblingCheckoutMissing { sibling, .. }
                if sibling == "../clean-signals-rs"
        ));
        // The gate failed before any edit — the dep was not added.
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(!cargo.contains("clean-signals-frust = {"), "{cargo}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_signals_frust_adds_dep_when_sibling_present() {
        let root = scaffold_project("clean-signals-present-sibling");
        // Point `frust` at a real in-tree crate dir so `resolve_sibling`'s
        // `..`-walk resolves on disk: repo root becomes `<root>/vendor`, and
        // the sibling `../clean-signals-rs` lands at `<root>/clean-signals-rs`.
        // Every intermediate directory must actually exist for a `..`-bearing
        // path's `.exists()` to resolve on Unix.
        let cargo_path = root.join(CARGO_TOML_REL);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        let rewritten = cargo.replace(
            "path = \"/nonexistent/frust/checkout\"",
            "path = \"vendor/crates/frust\"",
        );
        assert_ne!(rewritten, cargo, "expected to rewrite the frust path dep");
        fs::write(&cargo_path, rewritten).unwrap();
        fs::create_dir_all(root.join("vendor/crates/frust")).unwrap();
        fs::create_dir_all(root.join("clean-signals-rs")).unwrap();

        let report = add_plugin(&root, "clean-signals-frust", &[]).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        assert!(cargo.contains("clean-signals-frust"), "{cargo}");
        assert!(
            cargo.contains("vendor/crates/frust/../../plugins/clean-signals-frust"),
            "{cargo}"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
