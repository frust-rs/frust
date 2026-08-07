//! [`add_plugin`]: apply a registry plugin's contributions to a generated
//! project. Every edit is idempotent and format-preserving; a file that fails
//! to parse (or lacks its insertion anchor) is never rewritten.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, InlineTable, Item, Value};

use super::registry::find_plugin;
use super::{AddItem, AddOutcome, AddReport, Contribution, PluginAddError, PluginSpec};
use crate::scaffold::context::frust_path_from_project_subdir;

/// Project-relative paths of the files a contribution edits.
const CARGO_TOML_REL: &str = "Cargo.toml";
const MANIFEST_REL: &str = "android/app/src/main/AndroidManifest.xml";
const PLIST_REL: &str = "ios/Runner/Info.plist";
const SETTINGS_GRADLE_REL: &str = "android/settings.gradle.kts";
const APP_BUILD_GRADLE_REL: &str = "android/app/build.gradle.kts";
const PBXPROJ_REL: &str = "ios/Runner.xcodeproj/project.pbxproj";
const LIB_RS_REL: &str = "src/lib.rs";

/// The marker comments the Android app template ships for plugin-contributed
/// Gradle wiring (`templates/app/android.tmpl/settings.gradle.kts.tmpl` and
/// `app/build.gradle.kts.tmpl`). An insert goes on the line *after* the
/// marker; a project missing either marker is
/// [`PluginAddError::MalformedProjectFile`] and is never rewritten — guessing
/// at `include(":app")` or a bare `dependencies {` would be a second,
/// unpinned convention.
const SETTINGS_ANCHOR: &str = "// frust:plugin-includes";
const APP_DEPS_ANCHOR: &str = "// frust:plugin-dependencies";

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
        Contribution::GradleModule {
            gradle_name,
            rel_path,
        } => apply_gradle_module(project_root, frust_path, gradle_name, rel_path),
        Contribution::SwiftPackageRef {
            package_name,
            rel_path,
        } => apply_swift_package_ref(project_root, frust_path, package_name, rel_path),
        Contribution::AppCrateMacro {
            invocation,
            cfg,
            comment,
        } => apply_app_crate_macro(project_root, invocation, *cfg, comment),
    }
}

/// Append a macro invocation to the app crate's `src/lib.rs`
/// ([`Contribution::AppCrateMacro`]).
///
/// Appending — rather than inserting at an anchor — is deliberate. The
/// scaffold's `lib.rs` ends with `frust::app!(AppName);`, and its own doc
/// comment tells the app author never to edit that invocation; everything these
/// contributions plant belongs *after* it, at file scope, where item order does
/// not matter. So there is no anchor to maintain and nothing to half-edit.
///
/// Idempotence keys on the invocation text itself, so an author who moved the
/// line elsewhere in the file — or wrote it by hand before running Add Plugin,
/// which is exactly what a device gate run against a hand-edited project did —
/// is not handed a duplicate.
fn apply_app_crate_macro(
    project_root: &Path,
    invocation: &str,
    cfg: Option<&str>,
    comment: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(LIB_RS_REL);
    let src = read_required(&path, LIB_RS_REL)?;
    if src.contains(invocation) {
        return Ok(AddOutcome::AlreadyPresent);
    }
    let mut block = String::new();
    if !src.ends_with('\n') {
        block.push('\n');
    }
    block.push('\n');
    block.push_str(&format!("// {comment}\n"));
    if let Some(predicate) = cfg {
        block.push_str(&format!("#[cfg({predicate})]\n"));
    }
    block.push_str(invocation);
    block.push('\n');
    write_file(&path, LIB_RS_REL, &format!("{src}{block}"))?;
    Ok(AddOutcome::Applied)
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

/// Resolve a repo-root-relative directory (a [`Contribution::GradleModule`]'s
/// `rel_path`) the same way [`plugin_dep_path`] resolves a crate dir, and the
/// same way the scaffold derives `frust.embedding.dir`:
/// `<frust>/../../<rel_path>`. One path convention, not two.
///
/// Unlike [`plugin_dep_path`] — whose output lands in the project root's
/// `Cargo.toml`, the base a relative `frust_path` is written against — this
/// value lands one directory below the project root: in
/// `android/settings.gradle.kts` (via [`settings_include_block`]), resolved by
/// `file(...)` from `<project>/android/`, or in
/// `ios/Runner.xcodeproj/project.pbxproj` (via [`apply_swift_package_ref`]),
/// whose `relativePath` Xcode resolves against `<project>/ios/` — the
/// directory *containing* the `.xcodeproj`, not the bundle itself. A relative
/// `frust_path` therefore needs one extra `../` to climb out of that
/// subdirectory in either case, applied by the single shared
/// [`frust_path_from_project_subdir`] helper the scaffold's two embedding
/// accessors use; an absolute path is emitted unchanged.
///
/// Like those accessors, this **widens** the reach of the machine-specific
/// developer-checkout path `frust_path` already carries rather than merely
/// inheriting it: a missing checkout now fails Gradle *sync* (the project
/// won't configure), not just the Rust link step. A deliberate trade-off,
/// mitigated by the published-coordinate future in this feature's
/// `design/PUBLICATION_SEAM.md` §5.
fn repo_relative_path(frust_path: &str, rel_path: &str) -> String {
    format!(
        "{}/../../{rel_path}",
        frust_path_from_project_subdir(frust_path)
    )
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

/// Wire a plugin's `com.android.library` module into the generated project:
/// the `include(...)`/`projectDir`/build-dir-redirect trio in
/// `android/settings.gradle.kts` plus the `implementation(project(...))` line
/// in `android/app/build.gradle.kts`.
///
/// Both files are read and their replacements computed **before** either is
/// written, so a malformed second file can't leave the first half-edited.
/// Each half is independently skip-if-present, which is what makes a
/// half-applied state (someone deleted one of the two lines) complete rather
/// than duplicate: the outcome is [`AddOutcome::Applied`] if either half
/// changed, [`AddOutcome::AlreadyPresent`] only when both were already wired.
///
/// The module directory is *not* checked for existence — the mutation is a
/// text insert, and whether the path resolves is Gradle's problem at build
/// time, not `add_plugin`'s (a plugin can legitimately be wired before the
/// frust checkout moves into place).
fn apply_gradle_module(
    project_root: &Path,
    frust_path: &str,
    gradle_name: &str,
    rel_path: &str,
) -> Result<AddOutcome, PluginAddError> {
    let settings_path = project_root.join(SETTINGS_GRADLE_REL);
    let settings_src = read_required(&settings_path, SETTINGS_GRADLE_REL)?;
    let build_path = project_root.join(APP_BUILD_GRADLE_REL);
    let build_src = read_required(&build_path, APP_BUILD_GRADLE_REL)?;

    let settings_out = if settings_src.contains(&format!("include(\"{gradle_name}\")")) {
        None
    } else {
        Some(insert_after_anchor_line(
            &settings_src,
            SETTINGS_ANCHOR,
            &settings_include_block(gradle_name, frust_path, rel_path),
            SETTINGS_GRADLE_REL,
        )?)
    };

    let build_out = if build_src.contains(&format!("project(\"{gradle_name}\")")) {
        None
    } else {
        Some(insert_after_anchor_line(
            &build_src,
            APP_DEPS_ANCHOR,
            &format!("    implementation(project(\"{gradle_name}\"))\n"),
            APP_BUILD_GRADLE_REL,
        )?)
    };

    if settings_out.is_none() && build_out.is_none() {
        return Ok(AddOutcome::AlreadyPresent);
    }
    if let Some(out) = settings_out {
        write_file(&settings_path, SETTINGS_GRADLE_REL, &out)?;
    }
    if let Some(out) = build_out {
        write_file(&build_path, APP_BUILD_GRADLE_REL, &out)?;
    }
    Ok(AddOutcome::Applied)
}

/// The `settings.gradle.kts` block one [`Contribution::GradleModule`] adds —
/// deliberately the same shape the scaffold emits for `:frust-embedding`,
/// build-directory redirect included: a plugin module shares the embedding's
/// "two apps, one shared frust checkout" collision problem exactly.
fn settings_include_block(gradle_name: &str, frust_path: &str, rel_path: &str) -> String {
    let build_dir = gradle_name.trim_start_matches(':');
    let module_dir = repo_relative_path(frust_path, rel_path);
    format!(
        "\n\
         // Added by `frust` Add Plugin: a plugin's Android library module,\n\
         // included by path out of the frust checkout (the same derivation\n\
         // `frust.embedding.dir` and the plugin's Cargo path dep use).\n\
         include(\"{gradle_name}\")\n\
         project(\"{gradle_name}\").projectDir = file(\"{module_dir}\")\n\
         \n\
         gradle.lifecycle.beforeProject {{\n\
         \x20   if (path == \"{gradle_name}\") {{\n\
         \x20       layout.buildDirectory.set(rootDir.resolve(\"build/{build_dir}\"))\n\
         \x20   }}\n\
         }}\n"
    )
}

// ---------------------------------------------------------------------------
// `Contribution::SwiftPackageRef` — the six-site `project.pbxproj` applier.
// ---------------------------------------------------------------------------

/// The generated iOS project's object-id scheme: a 24-character uppercase-hex
/// id, hand-allocated as this fixed prefix plus a two-hex-digit counter
/// (`templates/app/ios.tmpl/.../project.pbxproj.tmpl` uses `…0001`–`…0033`;
/// `examples/glyph-catalog` re-allocated its embedding wiring at `…0034`–
/// `…0036`). A minted id continues the *target file's* own numbering, which is
/// what lets those two allocations coexist.
const PBX_ID_PREFIX: &str = "ABCDABCDABCDABCDABCD00";

/// Length of a pbxproj object id (24 uppercase-hex characters).
const PBX_ID_LEN: usize = 24;

/// The highest counter a `PBX_ID_PREFIX` id can carry (`NN` is two hex digits).
const PBX_ID_MAX: u32 = 0xFF;

/// The three objects one [`Contribution::SwiftPackageRef`] introduces —
/// resolved from whatever the target file already carries, minted only for the
/// pieces that are genuinely absent (so a half-wired project is *completed*
/// with its own ids rather than re-wired with a second, conflicting set).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SwiftPackageIds {
    /// The `PBXBuildFile` linking the product into the Frameworks phase.
    build_file: String,
    /// The `XCSwiftPackageProductDependency`.
    product_dependency: String,
    /// The `XCLocalSwiftPackageReference`.
    package_reference: String,
}

/// One of the six pbxproj sites a [`Contribution::SwiftPackageRef`] occupies.
/// All six are required: a project missing any one of them either fails to
/// build or fails to open, which is why this contribution is all-or-nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PbxSite {
    /// The `PBXBuildFile` object (`{ productRef = … }`).
    BuildFileObject,
    /// Its entry in the Frameworks build phase's `files` list.
    FrameworksEntry,
    /// The target's `packageProductDependencies` entry.
    ProductDependencyEntry,
    /// The project's `packageReferences` entry.
    PackageReferenceEntry,
    /// The `XCLocalSwiftPackageReference` object (carrying `relativePath`).
    PackageReferenceObject,
    /// The `XCSwiftPackageProductDependency` object (carrying `productName`).
    ProductDependencyObject,
}

/// Application order: the two list entries that point at objects come after
/// nothing in particular — each site is an independent single-line/blocked
/// splice against a distinct anchor — but a fixed order keeps the emitted
/// diff stable run to run.
const PBX_SITES: [PbxSite; 6] = [
    PbxSite::BuildFileObject,
    PbxSite::FrameworksEntry,
    PbxSite::ProductDependencyEntry,
    PbxSite::PackageReferenceEntry,
    PbxSite::PackageReferenceObject,
    PbxSite::ProductDependencyObject,
];

impl PbxSite {
    /// The `/* Begin … section */` this site lives in, plus the `key = ( … );`
    /// list within it (`None` for a site that *is* a top-level object).
    fn location(self) -> (&'static str, Option<&'static str>) {
        match self {
            PbxSite::BuildFileObject => ("PBXBuildFile", None),
            PbxSite::FrameworksEntry => ("PBXFrameworksBuildPhase", Some("files")),
            PbxSite::ProductDependencyEntry => {
                ("PBXNativeTarget", Some("packageProductDependencies"))
            }
            PbxSite::PackageReferenceEntry => ("PBXProject", Some("packageReferences")),
            PbxSite::PackageReferenceObject => ("XCLocalSwiftPackageReference", None),
            PbxSite::ProductDependencyObject => ("XCSwiftPackageProductDependency", None),
        }
    }

    /// The id-free text identifying this site's line — what a *presence* check
    /// matches, so a half-wired project is recognised no matter which ids its
    /// surviving sites use. Every pbxproj line of interest has the shape
    /// `<id> <needle>`, which is what makes [`Self::verify_needle`] a simple
    /// id-qualified extension of this one.
    fn needle(self, package_name: &str) -> String {
        match self {
            PbxSite::BuildFileObject => {
                format!("/* {package_name} in Frameworks */ = {{isa = PBXBuildFile;")
            }
            PbxSite::FrameworksEntry => format!("/* {package_name} in Frameworks */,"),
            PbxSite::ProductDependencyEntry => format!("/* {package_name} */,"),
            PbxSite::PackageReferenceEntry => {
                format!("/* XCLocalSwiftPackageReference \"{package_name}\" */,")
            }
            PbxSite::PackageReferenceObject => {
                format!("/* XCLocalSwiftPackageReference \"{package_name}\" */ = {{")
            }
            PbxSite::ProductDependencyObject => format!("/* {package_name} */ = {{"),
        }
    }

    /// The object this site names, and therefore whose id it must carry.
    fn id(self, ids: &SwiftPackageIds) -> &str {
        match self {
            PbxSite::BuildFileObject | PbxSite::FrameworksEntry => &ids.build_file,
            PbxSite::ProductDependencyEntry | PbxSite::ProductDependencyObject => {
                &ids.product_dependency
            }
            PbxSite::PackageReferenceEntry | PbxSite::PackageReferenceObject => {
                &ids.package_reference
            }
        }
    }

    /// The id-qualified text the post-edit re-scan requires — this is what
    /// catches a surviving site that points at a *different* id (drift the
    /// presence check alone would accept, and Xcode would reject).
    fn verify_needle(self, package_name: &str, ids: &SwiftPackageIds) -> String {
        format!("{} {}", self.id(ids), self.needle(package_name))
    }

    /// A stable label for [`PluginAddError::PbxWiringNotVerified`].
    fn label(self) -> &'static str {
        match self {
            PbxSite::BuildFileObject => "PBXBuildFile",
            PbxSite::FrameworksEntry => "PBXFrameworksBuildPhase.files",
            PbxSite::ProductDependencyEntry => "PBXNativeTarget.packageProductDependencies",
            PbxSite::PackageReferenceEntry => "PBXProject.packageReferences",
            PbxSite::PackageReferenceObject => "XCLocalSwiftPackageReference",
            PbxSite::ProductDependencyObject => "XCSwiftPackageProductDependency",
        }
    }
}

/// Add a plugin's local Swift package to the generated app's Xcode project as
/// a second package reference beside the embedding's `FrustEmbedding`.
///
/// The iOS counterpart of [`apply_gradle_module`], with the same
/// one-contribution/one-[`AddItem`] ledger and the same
/// complete-a-half-applied-state rule — but six sites instead of two, and a
/// stricter write discipline: the whole edited file is built in memory,
/// re-scanned site by site, and only then written **once**. A partially
/// edited `project.pbxproj` is not a smaller problem than none at all — Xcode
/// refuses to open it.
///
/// Anchors are the template's own section markers and list terminators; a
/// missing one is [`PluginAddError::MalformedProjectFile`], never a rewrite
/// (this module's standing rule). The package directory is *not* checked for
/// existence, exactly as [`apply_gradle_module`] documents.
fn apply_swift_package_ref(
    project_root: &Path,
    frust_path: &str,
    package_name: &str,
    rel_path: &str,
) -> Result<AddOutcome, PluginAddError> {
    let path = project_root.join(PBXPROJ_REL);
    let src = read_required(&path, PBXPROJ_REL)?;

    let ids = resolve_or_mint_ids(&src, package_name)?;
    let relative_path = repo_relative_path(frust_path, rel_path);

    let mut out = src;
    let mut changed = false;
    for site in PBX_SITES {
        if pbx_site_body(&out, site)?.contains(&site.needle(package_name)) {
            continue;
        }
        out = insert_pbx_site(&out, site, package_name, &ids, &relative_path)?;
        changed = true;
    }
    if !changed {
        return Ok(AddOutcome::AlreadyPresent);
    }

    // All-or-nothing: every site must be present *and* carry this run's id
    // before a single byte is written.
    for site in PBX_SITES {
        if !pbx_site_body(&out, site)?.contains(&site.verify_needle(package_name, &ids)) {
            return Err(PluginAddError::PbxWiringNotVerified {
                file: PBXPROJ_REL.to_string(),
                package: package_name.to_string(),
                site: site.label().to_string(),
            });
        }
    }

    write_file(&path, PBXPROJ_REL, &out)?;
    Ok(AddOutcome::Applied)
}

/// Splice one site into `src`, returning the whole edited file. A list entry
/// is appended after the last existing entry (immediately before the list's
/// own `);`); an object is appended to its section, before the `/* End … */`
/// marker.
fn insert_pbx_site(
    src: &str,
    site: PbxSite,
    package_name: &str,
    ids: &SwiftPackageIds,
    relative_path: &str,
) -> Result<String, PluginAddError> {
    let block = match site {
        PbxSite::FrameworksEntry
        | PbxSite::ProductDependencyEntry
        | PbxSite::PackageReferenceEntry => {
            let range = pbx_site_range(src, site)?;
            let entry = format!("\t\t\t\t{}\n", site.verify_needle(package_name, ids));
            let mut out = String::with_capacity(src.len() + entry.len());
            out.push_str(&src[..range.end]);
            out.push_str(&entry);
            out.push_str(&src[range.end..]);
            return Ok(out);
        }
        PbxSite::BuildFileObject => format!(
            "\t\t{} /* {package_name} in Frameworks */ = {{isa = PBXBuildFile; \
             productRef = {} /* {package_name} */; }};\n",
            ids.build_file, ids.product_dependency
        ),
        PbxSite::PackageReferenceObject => format!(
            "\t\t{} /* XCLocalSwiftPackageReference \"{package_name}\" */ = {{\n\
             \t\t\tisa = XCLocalSwiftPackageReference;\n\
             \t\t\trelativePath = \"{relative_path}\";\n\
             \t\t}};\n",
            ids.package_reference
        ),
        PbxSite::ProductDependencyObject => format!(
            "\t\t{} /* {package_name} */ = {{\n\
             \t\t\tisa = XCSwiftPackageProductDependency;\n\
             \t\t\tproductName = {package_name};\n\
             \t\t}};\n",
            ids.product_dependency
        ),
    };
    insert_before_anchor(
        src,
        &format!("/* End {} section */", site.location().0),
        &block,
        PBXPROJ_REL,
    )
}

/// The byte range a site's presence is searched in: its section's body, or
/// (for a list site) just the entries between `key = (` and its `);`.
fn pbx_site_range(src: &str, site: PbxSite) -> Result<Range<usize>, PluginAddError> {
    let (section, list) = site.location();
    match list {
        Some(key) => pbx_list_range(src, section, key),
        None => pbx_section_range(src, section),
    }
}

fn pbx_site_body(src: &str, site: PbxSite) -> Result<&str, PluginAddError> {
    Ok(&src[pbx_site_range(src, site)?])
}

/// The body of one `/* Begin <name> section */ … /* End <name> section */`
/// block, so a section-scoped match can't pick up text from another section
/// (`files = (` alone appears in four of them).
fn pbx_section_range(src: &str, name: &str) -> Result<Range<usize>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let begin = format!("/* Begin {name} section */");
    let end = format!("/* End {name} section */");
    let start = src.find(&begin).ok_or_else(malformed)? + begin.len();
    let stop = src[start..].find(&end).ok_or_else(malformed)? + start;
    Ok(start..stop)
}

/// The entry region of a `key = ( … );` list inside `section` — from the byte
/// after the opening line to the byte before the closing `);` line. Matched on
/// trimmed line content rather than exact indentation, so a project whose
/// pbxproj Xcode has since rewritten still wires up.
fn pbx_list_range(src: &str, section: &str, key: &str) -> Result<Range<usize>, PluginAddError> {
    let malformed = || PluginAddError::MalformedProjectFile(PBXPROJ_REL.to_string());
    let section = pbx_section_range(src, section)?;
    let open = format!("{key} = (");
    let body = &src[section.clone()];

    let mut cursor = 0usize;
    let mut start: Option<usize> = None;
    for line in body.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let trimmed = line.trim();
        match start {
            None if trimmed == open => start = Some(cursor),
            Some(start) if trimmed == ");" => {
                return Ok(section.start + start..section.start + line_start);
            }
            _ => {}
        }
    }
    Err(malformed())
}

/// This package's three object ids: reused wherever the target file already
/// mentions them (the half-wired case), minted otherwise.
///
/// Minting continues the target file's own `…00NN` numbering — scan for the
/// highest, allocate upward — which is what lets the template's `…0033` and a
/// consumer's hand-renumbered `…0036` (glyph-catalog) coexist without a
/// registry of allocated ids. A minted id that somehow already occurs is
/// [`PluginAddError::PbxIdCollision`], never an overwrite.
fn resolve_or_mint_ids(src: &str, package_name: &str) -> Result<SwiftPackageIds, PluginAddError> {
    let existing_build_file = pbx_id_preceding(src, &format!("/* {package_name} in Frameworks */"));
    // The product dependency's own comment is the bare package name — which
    // the build-file line also carries (`productRef = <id> /* Name */;`), and
    // whose preceding token is the very id wanted here.
    let existing_product = pbx_id_preceding(src, &format!("/* {package_name} */"));
    let existing_package_ref = pbx_id_preceding(
        src,
        &format!("/* XCLocalSwiftPackageReference \"{package_name}\" */"),
    );

    let mut next = pbx_next_id_counter(src);
    let mut take = |existing: Option<String>| -> Result<String, PluginAddError> {
        match existing {
            Some(id) => Ok(id),
            None => {
                let id = pbx_mint_id(src, next)?;
                next += 1;
                Ok(id)
            }
        }
    };

    Ok(SwiftPackageIds {
        build_file: take(existing_build_file)?,
        product_dependency: take(existing_product)?,
        package_reference: take(existing_package_ref)?,
    })
}

/// One past the highest `PBX_ID_PREFIX` counter in `src` (0 if the file uses
/// none of them at all).
fn pbx_next_id_counter(src: &str) -> u32 {
    let mut highest: Option<u32> = None;
    for (at, _) in src.match_indices(PBX_ID_PREFIX) {
        let tail = at + PBX_ID_PREFIX.len();
        let Some(counter) = src.get(tail..tail + 2) else {
            continue;
        };
        if !counter.chars().all(is_pbx_hex_digit) {
            continue;
        }
        if let Ok(value) = u32::from_str_radix(counter, 16) {
            highest = Some(highest.map_or(value, |h: u32| h.max(value)));
        }
    }
    highest.map_or(0, |h| h + 1)
}

/// Format `counter` as an object id, refusing a value the scheme can't express
/// or an id the file already uses.
fn pbx_mint_id(src: &str, counter: u32) -> Result<String, PluginAddError> {
    if counter > PBX_ID_MAX {
        return Err(PluginAddError::PbxIdSpaceExhausted {
            file: PBXPROJ_REL.to_string(),
        });
    }
    let id = format!("{PBX_ID_PREFIX}{counter:02X}");
    if src.contains(&id) {
        return Err(PluginAddError::PbxIdCollision {
            file: PBXPROJ_REL.to_string(),
            id,
        });
    }
    Ok(id)
}

/// The object id immediately preceding the first occurrence of `needle` —
/// every pbxproj line naming an object writes `<id> /* comment */`, whether it
/// is a definition (`= {`), a list entry (`,`) or an inline reference
/// (`productRef = <id> /* … */;`).
fn pbx_id_preceding(src: &str, needle: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(rel) = src[from..].find(needle) {
        let at = from + rel;
        let head = src[..at].trim_end();
        let candidate = head
            .len()
            .checked_sub(PBX_ID_LEN)
            .and_then(|start| head.get(start..));
        if let Some(id) = candidate.filter(|id| is_pbx_object_id(id)) {
            return Some(id.to_string());
        }
        from = at + needle.len();
    }
    None
}

fn is_pbx_hex_digit(c: char) -> bool {
    c.is_ascii_digit() || ('A'..='F').contains(&c)
}

/// A 24-character uppercase-hex pbxproj object id — the style every id in the
/// generated project uses.
fn is_pbx_object_id(token: &str) -> bool {
    token.len() == PBX_ID_LEN && token.chars().all(is_pbx_hex_digit)
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

/// Insert `insertion` immediately after the line containing the first
/// occurrence of `anchor`, preserving all surrounding bytes — the
/// marker-comment counterpart to [`insert_before_anchor`], for anchors whose
/// own text says "lines go below". Errors
/// [`PluginAddError::MalformedProjectFile`] if the anchor is absent (the file
/// is never rewritten in that case).
fn insert_after_anchor_line(
    src: &str,
    anchor: &str,
    insertion: &str,
    rel: &str,
) -> Result<String, PluginAddError> {
    let idx = src
        .find(anchor)
        .ok_or_else(|| PluginAddError::MalformedProjectFile(rel.to_string()))?;
    // Past the anchor's own line: the byte after its newline, or end-of-file
    // for a trailing anchor with no final newline.
    let split = match src[idx..].find('\n') {
        Some(nl) => idx + nl + 1,
        None => src.len(),
    };
    let mut out = String::with_capacity(src.len() + insertion.len() + 1);
    out.push_str(&src[..split]);
    if split == src.len() && !src.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(insertion);
    out.push_str(&src[split..]);
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
    /// disk — so a derived `../clean-signals-rs` sibling path is absent too,
    /// and no accidental real checkout is picked up.
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

    /// The project-relative files a `GradleModule` contribution edits, for
    /// tests that assert one was (or was not) touched.
    const PROGUARD_REL: &str = "android/app/proguard-rules.pro";

    #[test]
    fn add_secure_storage_with_biometric_applies_every_edit() {
        let root = scaffold_project("ss-biometric");

        let manifest_before = fs::read(root.join(MANIFEST_REL)).unwrap();
        let proguard_before = fs::read(root.join(PROGUARD_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert_eq!(report.plugin_id, "secure-storage");
        // dep + plist + gradle module = 3 items, all applied. The Android
        // permission, Kotlin helper and keep rule ride inside the module.
        assert_eq!(report.items.len(), 3);
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied)
        );
        assert_eq!(report.counts(), (3, 0));

        // Cargo.toml: the dep landed with a path derived from the frust dep.
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("frust-secure-storage"), "{cargo}");
        assert!(
            cargo.contains("/nonexistent/frust/checkout/../../plugins/secure-storage"),
            "{cargo}"
        );

        // The app's own manifest and proguard-rules.pro are NOT edited: the
        // permission rides the plugin module's manifest (merger) and the keep
        // rule its `consumerProguardFiles`.
        assert_eq!(fs::read(root.join(MANIFEST_REL)).unwrap(), manifest_before);
        assert_eq!(fs::read(root.join(PROGUARD_REL)).unwrap(), proguard_before);

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

        // No Kotlin file is copied into the app any more.
        assert!(
            !root
                .join("android/app/src/main/kotlin/dev/frust/FrustBiometric.kt")
                .exists()
        );

        // settings.gradle.kts: include + projectDir + build-dir redirect, with
        // the module path derived from the same frust path dep, and the
        // pre-existing `:frust-embedding` wiring untouched.
        let settings = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        assert!(
            settings.contains("include(\":frust-secure-storage\")"),
            "{settings}"
        );
        assert!(
            settings.contains(
                "project(\":frust-secure-storage\").projectDir = \
                 file(\"/nonexistent/frust/checkout/../../plugins/secure-storage/platform/android\")"
            ),
            "{settings}"
        );
        assert!(
            settings.contains("rootDir.resolve(\"build/frust-secure-storage\")"),
            "{settings}"
        );
        assert!(
            settings.contains("include(\":frust-embedding\")"),
            "{settings}"
        );
        assert!(settings.contains("include(\":app\")"), "{settings}");

        // app/build.gradle.kts: the module dependency landed inside the
        // dependencies block, beside the embedding's.
        let build = fs::read_to_string(root.join(APP_BUILD_GRADLE_REL)).unwrap();
        assert!(
            build.contains("implementation(project(\":frust-secure-storage\"))"),
            "{build}"
        );
        assert!(
            build.contains("implementation(project(\":frust-embedding\"))"),
            "{build}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// `test_context`'s `frust_path` is deliberately nonexistent, so the
    /// module directory the include points at does not resolve on disk — and
    /// applying must still succeed. The mutation is a text insert; whether the
    /// path resolves is Gradle's problem at build time. Pinned so a future
    /// change can't quietly add an existence check.
    #[test]
    fn gradle_module_applies_even_though_its_resolved_path_is_absent() {
        let root = scaffold_project("gradle-module-absent-path");
        let module_dir = PathBuf::from(repo_relative_path(
            "/nonexistent/frust/checkout",
            "plugins/secure-storage/platform/android",
        ));
        assert!(
            !module_dir.exists(),
            "precondition: the module dir is absent"
        );

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-secure-storage"))
            .expect("a Gradle module line item");
        assert_eq!(item.outcome, AddOutcome::Applied);

        let _ = fs::remove_dir_all(&root);
    }

    /// Lexical `..`/`.` collapse — the module directory need not exist (see
    /// the test above), so `fs::canonicalize` is unavailable.
    fn normalize_lexically(path: &Path) -> PathBuf {
        use std::path::Component;
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }

    /// `test_context`'s `frust_path` is absolute, which is immune to the
    /// base-directory question — only a **relative** `--frust-path` (a
    /// supported input) exposes it. The `projectDir` this writes is resolved
    /// by `file(...)` from `<project>/android/`, one level below the project
    /// root the Cargo `frust` path dep is expressed against, so it must carry
    /// one extra `../`.
    #[test]
    fn gradle_module_projectdir_resolves_from_the_android_subdirectory() {
        let root = scaffold_project("gradle-module-relative-frust-path");
        const REL_FRUST: &str = "../checkouts/frust/crates/frust";
        const MODULE_REL: &str = "plugins/secure-storage/platform/android";

        let cargo_path = root.join(CARGO_TOML_REL);
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        let rewritten = cargo.replace(
            "path = \"/nonexistent/frust/checkout\"",
            &format!("path = \"{REL_FRUST}\""),
        );
        assert_ne!(rewritten, cargo, "expected to rewrite the frust path dep");
        fs::write(&cargo_path, rewritten).unwrap();

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-secure-storage"))
            .expect("a Gradle module line item");
        // No on-disk existence check: the relative module dir is absent too.
        assert_eq!(item.outcome, AddOutcome::Applied);

        let settings = fs::read_to_string(root.join(SETTINGS_GRADLE_REL)).unwrap();
        let emitted = format!("../{REL_FRUST}/../../{MODULE_REL}");
        assert!(
            settings.contains(&format!(
                "project(\":frust-secure-storage\").projectDir = file(\"{emitted}\")"
            )),
            "{settings}"
        );

        // The intent behind that literal: resolved from `<project>/android/`
        // (Gradle's base for `settings.gradle.kts`), it must land exactly
        // where the project-root-relative convention reaches from `<project>/`
        // — the same walk `resolve_sibling` performs.
        let truth = normalize_lexically(&root.join(REL_FRUST).join("../..").join(MODULE_REL));
        let actual = normalize_lexically(&root.join("android").join(&emitted));
        assert_eq!(actual, truth);
        assert!(
            !actual.exists(),
            "the module dir is absent, yet apply succeeded"
        );

        // The Cargo dep path is unchanged by this correction: `Cargo.toml`
        // sits at the project root, so it needs no re-basing.
        let cargo = fs::read_to_string(&cargo_path).unwrap();
        assert!(
            cargo.contains(&format!("{REL_FRUST}/../../plugins/secure-storage")),
            "{cargo}"
        );

        // Still idempotent with a relative path.
        let before = snapshot_tree(&root);
        let second = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent)
        );
        assert_eq!(before, snapshot_tree(&root));

        let _ = fs::remove_dir_all(&root);
    }

    /// A half-applied module (settings wired, app dependency removed) must
    /// complete the missing half and report `Applied`, not skip on the first
    /// guard it finds satisfied.
    #[test]
    fn half_applied_gradle_module_completes_and_reports_applied() {
        let root = scaffold_project("gradle-module-half-applied");
        add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();

        // Undo only the app-module dependency half.
        let build_path = root.join(APP_BUILD_GRADLE_REL);
        let build = fs::read_to_string(&build_path).unwrap();
        let stripped = build.replace(
            "    implementation(project(\":frust-secure-storage\"))\n",
            "",
        );
        assert_ne!(stripped, build, "expected to strip the dependency line");
        fs::write(&build_path, &stripped).unwrap();
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap();
        let item = report
            .items
            .iter()
            .find(|i| i.description.contains(":frust-secure-storage"))
            .expect("a Gradle module line item");
        assert_eq!(
            item.outcome,
            AddOutcome::Applied,
            "a half-applied module must complete and report Applied"
        );

        // The missing half was restored, and the already-wired half was not
        // duplicated or rewritten.
        let build = fs::read_to_string(&build_path).unwrap();
        assert_eq!(
            build
                .matches("implementation(project(\":frust-secure-storage\"))")
                .count(),
            1,
            "{build}"
        );
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before,
            "the already-wired settings.gradle.kts must be byte-identical"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_gradle_without_its_anchor_is_never_rewritten() {
        let root = scaffold_project("gradle-module-no-settings-anchor");
        let settings_path = root.join(SETTINGS_GRADLE_REL);
        let mangled = fs::read_to_string(&settings_path)
            .unwrap()
            .replace(SETTINGS_ANCHOR, "// (marker removed)");
        fs::write(&settings_path, &mangled).unwrap();
        let build_before = fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap();

        let err = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == SETTINGS_GRADLE_REL),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), mangled);
        // The sibling file is untouched too — both are computed before either
        // is written.
        assert_eq!(
            fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap(),
            build_before
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn app_build_gradle_without_its_anchor_is_never_rewritten() {
        let root = scaffold_project("gradle-module-no-build-anchor");
        let build_path = root.join(APP_BUILD_GRADLE_REL);
        let mangled = fs::read_to_string(&build_path)
            .unwrap()
            .replace(APP_DEPS_ANCHOR, "// (marker removed)");
        fs::write(&build_path, &mangled).unwrap();
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();

        let err = add_plugin(&root, "secure-storage", &["biometric-gate"]).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == APP_BUILD_GRADLE_REL),
            "{err}"
        );
        assert_eq!(fs::read_to_string(&build_path).unwrap(), mangled);
        // settings.gradle.kts was not half-written before the failure.
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before
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
        assert_eq!(second.items.len(), 3);
        assert!(
            second
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::AlreadyPresent),
            "second apply must report every item AlreadyPresent"
        );
        // The Gradle module — two files behind one line item — is covered by
        // that blanket assertion; named here so a future contribution set
        // cannot silently drop it from the idempotency gate.
        assert!(
            second
                .items
                .iter()
                .any(|i| i.description.contains(":frust-secure-storage")),
            "the Gradle module must be one of the idempotency-checked items"
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
        let settings_before = fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap();
        let build_before = fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap();

        let report = add_plugin(&root, "secure-storage", &[]).unwrap();
        // Only the Cargo.toml dependency is applied.
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        assert!(report.items[0].description.contains("frust-secure-storage"));

        // No platform file was touched — including the two Gradle files the
        // `biometric-gate` module contribution would have edited.
        assert_eq!(fs::read(root.join(MANIFEST_REL)).unwrap(), manifest_before);
        assert_eq!(fs::read(root.join(PLIST_REL)).unwrap(), plist_before);
        assert_eq!(fs::read(root.join(PROGUARD_REL)).unwrap(), proguard_before);
        assert_eq!(
            fs::read(root.join(SETTINGS_GRADLE_REL)).unwrap(),
            settings_before
        );
        assert_eq!(
            fs::read(root.join(APP_BUILD_GRADLE_REL)).unwrap(),
            build_before
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
    fn clean_signals_frust_add_succeeds_without_sibling_checkout() {
        // test_context's frust_path is nonexistent, so a `../clean-signals-rs`
        // sibling derived from it is absent too — clean-signals-frust's
        // registry entry no longer requires one (clean-signals is
        // git+rev-pinned to its public repo; see `registry.rs`'s
        // `CLEAN_SIGNALS_FRUST`), so this must still succeed.
        let root = scaffold_project("clean-signals-no-sibling");
        let report = add_plugin(&root, "clean-signals-frust", &[]).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].outcome, AddOutcome::Applied);
        let cargo = fs::read_to_string(root.join(CARGO_TOML_REL)).unwrap();
        assert!(cargo.contains("clean-signals-frust = {"), "{cargo}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_signals_frust_add_succeeds_with_stale_sibling_present() {
        // Same as above, but with a leftover `../clean-signals-rs` directory
        // actually present on disk (a dev machine that still has the
        // pre-migration sibling checkout) — its presence or absence must
        // make no difference now that the registry entry declares no
        // `requires_sibling`.
        let root = scaffold_project("clean-signals-stale-sibling");
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

    /// The frust repo root, located robustly off `CARGO_MANIFEST_DIR`
    /// (`crates/frust-drive`) rather than the process CWD — this test must
    /// pass regardless of where `cargo test` is invoked from.
    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Discover every in-repo Android consumer under `examples/` and
    /// `benchmarks/` — a direct subdirectory counts if it looks like a
    /// frust-scaffolded Android project (carries both `Cargo.toml` and an
    /// `android/` Gradle project). Discovered dynamically rather than
    /// hard-coded: a project added later is automatically covered, and one
    /// removed (as `bubblebench` was) is simply absent from the listing
    /// rather than a failure — `fs::read_dir` on a since-deleted `examples/*`
    /// entry can't produce it, and a missing `examples/`/`benchmarks/`
    /// directory itself is tolerated the same way.
    fn discover_android_consumers(repo_root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for parent in ["examples", "benchmarks"] {
            let Ok(entries) = fs::read_dir(repo_root.join(parent)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path.join(CARGO_TOML_REL).is_file()
                    && path.join(SETTINGS_GRADLE_REL).is_file()
                    && path.join(APP_BUILD_GRADLE_REL).is_file()
                {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// Copy only the files `add_plugin` reads/writes for the
    /// `secure-storage` + `biometric-gate` contribution set (`Cargo.toml`,
    /// the Android manifest, the iOS plist, and the two Gradle files) from a
    /// real consumer project into a fresh tempdir, preserving their
    /// project-relative layout.
    ///
    /// Deliberately **not** a whole-directory copy: an in-repo Android
    /// consumer carries multi-gigabyte Gradle/cargo-ndk build output
    /// (`android/app/build`, `android/app/src/main/jniLibs`) that
    /// `add_plugin` never touches and a test must never copy, and the real
    /// project must never be mutated in place.
    fn copy_consumer_files(src_root: &Path, dest_root: &Path) -> std::io::Result<()> {
        for rel in [
            CARGO_TOML_REL,
            MANIFEST_REL,
            PLIST_REL,
            SETTINGS_GRADLE_REL,
            APP_BUILD_GRADLE_REL,
        ] {
            let src = src_root.join(rel);
            let dest = dest_root.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dest)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // `Contribution::SwiftPackageRef`
    //
    // Driven through `apply_swift_package_ref` directly rather than
    // `add_plugin`: no registry entry carries this contribution yet (the
    // camera plugin's registration is a later task), and the applier's whole
    // contract — six sites, minted ids, all-or-nothing write — is per
    // contribution, not per plugin.
    // -----------------------------------------------------------------------

    /// A plugin package name and repo-root-relative package directory standing
    /// in for the camera plugin's, whose registry entry lands in a later task.
    const PKG: &str = "FrustCamera";
    const PKG_REL: &str = "plugins/camera/platform/ios/FrustCamera";
    const TEST_FRUST_PATH: &str = "/nonexistent/frust/checkout";

    /// The three ids minted against a freshly rendered template, whose own
    /// highest allocation is `…0033` (the embedding's `PBXBuildFile`).
    const MINTED_BUILD_FILE: &str = "ABCDABCDABCDABCDABCD0034";
    const MINTED_PRODUCT: &str = "ABCDABCDABCDABCDABCD0035";
    const MINTED_PACKAGE_REF: &str = "ABCDABCDABCDABCDABCD0036";

    fn apply_test_package(root: &Path) -> Result<AddOutcome, PluginAddError> {
        apply_swift_package_ref(root, TEST_FRUST_PATH, PKG, PKG_REL)
    }

    fn read_pbxproj(root: &Path) -> String {
        fs::read_to_string(root.join(PBXPROJ_REL)).unwrap()
    }

    /// Every object-id token on one pbxproj line, in order of appearance.
    fn pbx_ids_on_line(line: &str) -> Vec<&str> {
        line.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| is_pbx_object_id(token))
            .collect()
    }

    /// Splits a `project.pbxproj` into `(defined ids, referenced ids)` — a
    /// *definition* is a top-level entry of the `objects = { … }` map (exactly
    /// two leading tabs, an object id, then ` = {`), every other mention is a
    /// reference.
    ///
    /// Mirrors the scaffold test suite's helper of the same name
    /// (`scaffold/mod.rs`), which is `#[cfg(test)]`-private to that module and
    /// so unreachable from here; both exist to catch the same corruption — a
    /// duplicate, dangling, or orphaned id makes Xcode refuse to open the
    /// project.
    fn pbx_ids(pbxproj: &str) -> (Vec<String>, Vec<String>) {
        let mut defined = Vec::new();
        let mut referenced = Vec::new();
        for line in pbxproj.lines() {
            let ids = pbx_ids_on_line(line);
            let top_level = line
                .strip_prefix("\t\t")
                .filter(|rest| !rest.starts_with('\t'));
            let is_definition = top_level.is_some_and(|rest| {
                ids.first().is_some_and(|id| rest.starts_with(id)) && rest.contains(" = {")
            });
            let mut ids = ids.into_iter();
            if is_definition {
                defined.push(ids.next().expect("a definition line has an id").to_string());
            }
            referenced.extend(ids.map(str::to_string));
        }
        (defined, referenced)
    }

    /// No duplicate definition, no reference to an undefined id, no defined
    /// object nothing points at — the three ways a hand-edited pbxproj breaks.
    fn assert_pbxproj_well_formed(pbxproj: &str) {
        let (defined, referenced) = pbx_ids(pbxproj);
        let mut unique = defined.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            defined.len(),
            "duplicate PBX object id(s):\n{pbxproj}"
        );
        for id in &referenced {
            assert!(
                defined.contains(id),
                "dangling PBX object id `{id}`:\n{pbxproj}"
            );
        }
        for id in &defined {
            assert!(
                referenced.contains(id),
                "orphaned PBX object `{id}`:\n{pbxproj}"
            );
        }
        assert_eq!(
            pbxproj.matches("/* Begin ").count(),
            pbxproj.matches("/* End ").count(),
            "unbalanced PBX sections:\n{pbxproj}"
        );
    }

    /// Assert all six sites are wired for `package_name` with `ids`, each in
    /// its own section (so a line landing in the wrong section is caught).
    fn assert_six_sites(pbxproj: &str, package_name: &str, ids: &SwiftPackageIds) {
        for site in PBX_SITES {
            let body = pbx_site_body(pbxproj, site).unwrap();
            let needle = site.verify_needle(package_name, ids);
            assert!(
                body.contains(&needle),
                "site `{}` is missing `{needle}`:\n{pbxproj}",
                site.label()
            );
            assert_eq!(
                body.matches(&needle).count(),
                1,
                "site `{}` is duplicated:\n{pbxproj}",
                site.label()
            );
        }
    }

    #[test]
    fn swift_package_ref_wires_all_six_pbxproj_sites() {
        let root = scaffold_project("swift-package-fresh");
        let before = read_pbxproj(&root);

        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);
        let pbxproj = read_pbxproj(&root);

        // Ids continue the template's own numbering, sequentially.
        let ids = SwiftPackageIds {
            build_file: MINTED_BUILD_FILE.to_string(),
            product_dependency: MINTED_PRODUCT.to_string(),
            package_reference: MINTED_PACKAGE_REF.to_string(),
        };
        assert_eq!(resolve_or_mint_ids(&before, PKG).unwrap(), ids);
        assert_six_sites(&pbxproj, PKG, &ids);

        // The two objects carrying real payload.
        assert!(
            pbxproj.contains(&format!(
                "\t\t\trelativePath = \"{TEST_FRUST_PATH}/../../{PKG_REL}\";"
            )),
            "{pbxproj}"
        );
        assert!(
            pbxproj.contains(&format!("\t\t\tproductName = {PKG};")),
            "{pbxproj}"
        );
        // …and the build file pointing at the product dependency.
        assert!(
            pbxproj.contains(&format!("productRef = {MINTED_PRODUCT} /* {PKG} */;")),
            "{pbxproj}"
        );

        // The embedding's own wiring is untouched beside it.
        assert_six_sites(
            &pbxproj,
            "FrustEmbedding",
            &SwiftPackageIds {
                build_file: "ABCDABCDABCDABCDABCD0033".to_string(),
                product_dependency: "ABCDABCDABCDABCDABCD0032".to_string(),
                package_reference: "ABCDABCDABCDABCDABCD0031".to_string(),
            },
        );

        assert_pbxproj_well_formed(&pbxproj);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn swift_package_ref_reapply_is_already_present_and_byte_identical() {
        let root = scaffold_project("swift-package-idempotent");
        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);
        let after_first = snapshot_tree(&root);

        assert_eq!(
            apply_test_package(&root).unwrap(),
            AddOutcome::AlreadyPresent
        );
        assert_eq!(
            after_first,
            snapshot_tree(&root),
            "a second apply must leave a byte-identical tree"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Two sites deleted by hand — one list entry, one whole object — must be
    /// completed **with the surviving ids** (minting a second set would leave
    /// the survivors dangling), reporting `Applied`.
    #[test]
    fn half_wired_swift_package_ref_completes_the_missing_sites() {
        let root = scaffold_project("swift-package-half-wired");
        apply_test_package(&root).unwrap();

        let path = root.join(PBXPROJ_REL);
        let wired = read_pbxproj(&root);
        let frameworks_entry = format!("\t\t\t\t{MINTED_BUILD_FILE} /* {PKG} in Frameworks */,\n");
        let product_object = format!(
            "\t\t{MINTED_PRODUCT} /* {PKG} */ = {{\n\
             \t\t\tisa = XCSwiftPackageProductDependency;\n\
             \t\t\tproductName = {PKG};\n\
             \t\t}};\n"
        );
        let mangled = wired
            .replace(&frameworks_entry, "")
            .replace(&product_object, "");
        assert_eq!(
            mangled.len(),
            wired.len() - frameworks_entry.len() - product_object.len(),
            "expected to strip exactly the two sites"
        );
        fs::write(&path, &mangled).unwrap();

        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);

        let repaired = read_pbxproj(&root);
        assert_six_sites(
            &repaired,
            PKG,
            &SwiftPackageIds {
                build_file: MINTED_BUILD_FILE.to_string(),
                product_dependency: MINTED_PRODUCT.to_string(),
                package_reference: MINTED_PACKAGE_REF.to_string(),
            },
        );
        assert_pbxproj_well_formed(&repaired);

        let _ = fs::remove_dir_all(&root);
    }

    /// A consumer that hand-allocated ids past the template's (exactly what
    /// `examples/glyph-catalog` did with `…0034`–`…0036`) must be minted
    /// *past*, never over.
    #[test]
    fn swift_package_ref_mints_past_pre_existing_ids() {
        let root = scaffold_project("swift-package-renumbered");
        let path = root.join(PBXPROJ_REL);

        // Renumber the embedding wiring 0031-0033 -> 0034-0036, descending so
        // no replacement clobbers a later one.
        let mut renumbered = read_pbxproj(&root);
        for (from, to) in [("0033", "0036"), ("0032", "0035"), ("0031", "0034")] {
            renumbered = renumbered.replace(
                &format!("ABCDABCDABCDABCDABCD{from}"),
                &format!("ABCDABCDABCDABCDABCD{to}"),
            );
        }
        assert!(renumbered.contains("ABCDABCDABCDABCDABCD0036"));
        assert!(!renumbered.contains("ABCDABCDABCDABCDABCD0033"));
        fs::write(&path, &renumbered).unwrap();

        assert_eq!(apply_test_package(&root).unwrap(), AddOutcome::Applied);

        let pbxproj = read_pbxproj(&root);
        assert_six_sites(
            &pbxproj,
            PKG,
            &SwiftPackageIds {
                build_file: "ABCDABCDABCDABCDABCD0037".to_string(),
                product_dependency: "ABCDABCDABCDABCDABCD0038".to_string(),
                package_reference: "ABCDABCDABCDABCDABCD0039".to_string(),
            },
        );
        assert_pbxproj_well_formed(&pbxproj);

        let _ = fs::remove_dir_all(&root);
    }

    /// `TEST_FRUST_PATH` is absolute, which is immune to the base-directory
    /// question — only a **relative** `--frust-path` exposes it. Xcode
    /// resolves an `XCLocalSwiftPackageReference`'s `relativePath` against
    /// `<project>/ios/` (the directory containing the `.xcodeproj`), one level
    /// below the project root a Cargo path dep is written against, so it needs
    /// one extra `../` — the off-by-one the embedding extraction shipped once
    /// and had to fix.
    #[test]
    fn swift_package_ref_relative_path_resolves_from_the_ios_subdirectory() {
        let root = scaffold_project("swift-package-relative-frust-path");
        const REL_FRUST: &str = "../checkouts/frust/crates/frust";

        assert_eq!(
            apply_swift_package_ref(&root, REL_FRUST, PKG, PKG_REL).unwrap(),
            AddOutcome::Applied
        );

        let emitted = format!("../{REL_FRUST}/../../{PKG_REL}");
        assert!(
            read_pbxproj(&root).contains(&format!("relativePath = \"{emitted}\";")),
            "{}",
            read_pbxproj(&root)
        );

        // The intent behind that literal: resolved from `<project>/ios/`, it
        // must land exactly where the project-root-relative convention reaches
        // from `<project>/` (the same walk `resolve_sibling` performs).
        let truth = normalize_lexically(&root.join(REL_FRUST).join("../..").join(PKG_REL));
        let actual = normalize_lexically(&root.join("ios").join(&emitted));
        assert_eq!(actual, truth);

        let _ = fs::remove_dir_all(&root);
    }

    /// A missing section marker is a hard error, never a rewrite — and the
    /// whole file is left untouched, not half-edited (the five earlier sites
    /// only ever existed in memory).
    #[test]
    fn swift_package_ref_without_a_section_anchor_is_never_rewritten() {
        let root = scaffold_project("swift-package-no-anchor");
        let path = root.join(PBXPROJ_REL);
        let mangled = read_pbxproj(&root).replace(
            "/* End XCLocalSwiftPackageReference section */",
            "// (marker removed)",
        );
        fs::write(&path, &mangled).unwrap();

        let err = apply_test_package(&root).unwrap_err();
        assert!(
            matches!(&err, PluginAddError::MalformedProjectFile(f) if f == PBXPROJ_REL),
            "{err}"
        );
        assert_eq!(read_pbxproj(&root), mangled, "the file must be untouched");

        let _ = fs::remove_dir_all(&root);
    }

    /// The regression guard for anchor drift in already-checked-in
    /// consumers: `add_plugin`'s tests above only ever exercise a **freshly
    /// scaffolded** project, which by construction always carries the
    /// `// frust:plugin-includes`/`// frust:plugin-dependencies` anchors
    /// `Contribution::GradleModule` hard-requires — so they are structurally
    /// incapable of catching an existing in-repo consumer drifting away from
    /// them (exactly how `glyph-catalog` and `layer-bench` shipped without
    /// either anchor).
    ///
    /// This drives the real `add_plugin` path — not a grep — against a
    /// tempdir copy of every in-repo Android consumer, so a future project
    /// that ships without the anchors fails **this** test with a message
    /// naming the project and the missing anchor, instead of silently
    /// failing Add Plugin at demo time.
    #[test]
    fn every_in_repo_android_consumer_accepts_add_plugin() {
        let repo_root = repo_root();
        let consumers = discover_android_consumers(&repo_root);
        assert!(
            !consumers.is_empty(),
            "expected to discover at least one in-repo Android consumer under \
             examples/ or benchmarks/ (repo root resolved to {})",
            repo_root.display()
        );

        for (i, consumer) in consumers.iter().enumerate() {
            let display = consumer
                .strip_prefix(&repo_root)
                .unwrap_or(consumer)
                .display()
                .to_string();

            let dest = unique_temp_dir(&format!("consumer-{i}"));
            fs::create_dir_all(&dest).unwrap();
            copy_consumer_files(consumer, &dest).unwrap_or_else(|e| {
                panic!("failed to copy {display}'s plugin-relevant files into a tempdir: {e}")
            });

            match add_plugin(&dest, "secure-storage", &["biometric-gate"]) {
                Ok(report) => {
                    assert!(
                        report
                            .items
                            .iter()
                            .all(|i| i.outcome == AddOutcome::Applied),
                        "{display}: expected every contribution to apply cleanly \
                         against an unmodified copy, got {report:?}"
                    );
                }
                Err(PluginAddError::MalformedProjectFile(file)) => {
                    panic!(
                        "{display} is missing a plugin anchor in `{file}` — \
                         `add_plugin` requires `{SETTINGS_ANCHOR}` in \
                         `{SETTINGS_GRADLE_REL}` and `{APP_DEPS_ANCHOR}` in \
                         `{APP_BUILD_GRADLE_REL}` (see \
                         `templates/app/android.tmpl` for the canonical \
                         placement, or `examples/huddle`'s android/ for a \
                         working in-repo example)"
                    );
                }
                Err(e) => panic!("{display}: add_plugin failed unexpectedly: {e}"),
            }

            let _ = fs::remove_dir_all(&dest);
        }
    }
}
