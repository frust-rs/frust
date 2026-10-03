//! Source-scan conformance test guarding the bare `dev.frust` Kotlin package
//! rule (`docs/CODE_STANDARDS.md`'s Plugin Conventions) — a cross-cutting
//! invariant that must see every plugin's **Kotlin** under `plugins/**`, not
//! just one.
//!
//! # Scope: Kotlin only, by design — not "every file under `plugins/**`"
//!
//! An earlier version of this scan reported green while silently skipping
//! Swift entirely — a latent trap, since an empty scan proves nothing. This
//! version is explicit about that boundary instead. The bare-`dev.frust`-package rule this test
//! pins is a **Kotlin/JNI-export-symbol-naming concept**: a Java-style
//! `package` declaration gets baked verbatim into a JNI export's mangled
//! symbol name (`docs/CODE_STANDARDS.md`'s "JNI export names are LAW"), which
//! is exactly why a second plugin copying the bare `dev.frust` package would
//! be a real, silent collision risk worth scanning for. **Swift has no
//! equivalent mechanism to police the same way** — a Swift module is its own
//! separate compilation unit with no `package`-style declaration statement, so
//! there is no way for a stray `.swift` file to "declare itself" inside
//! another module's namespace the way a stray `.kt` file could copy `package
//! dev.frust`. Broadening this specific scan to Swift would be checking for a
//! defect class that structurally cannot occur there.
//!
//! **Swift is intentionally, visibly out of scope for this reason.** As of
//! this writing the only Swift under `plugins/**` is
//! `plugins/camera/platform/ios/Package.swift` and
//! `plugins/camera/platform/ios/Sources/FrustCamera/CameraPreviewFactory.swift`
//! — both unaffected by, and untested by, this scan. iOS's own
//! platform-view-factory naming rule (`docs/CODE_STANDARDS.md`'s Naming
//! Conventions table: a bare `@objc(<Name>)` runtime name, e.g.
//! `@objc(CameraPreviewFactory)`, no package prefix — the opposite convention
//! from Android's fully-qualified `dev.frust.*` requirement) is a SEPARATE LAW
//! governing a different concern (the `viewType` string a host resolves via
//! `NSClassFromString`), and this test does not check it — that invariant, if
//! it ever needs a conformance scan of its own, belongs in a differently-named
//! test, not a broadened version of this one.
//!
//! This test lives in `frust-drive` rather than the published facade crate's
//! own test suite, since coupling `cargo test -p frust-ui` to every plugin's
//! Kotlin layout would be in tension
//! with `docs/ARCHITECTURE.md`'s "the facade never depends on or re-exports a
//! plugin." `frust-drive` is the right host instead: it is an explicit leaf
//! with no `frust-*`/framework-crate dependencies, it already owns the
//! analogous `print_free_cores.rs` source scan, and it is not the app-facing
//! published facade. The `frust-native-widgets`-specific parity/packing
//! checks live in
//! `plugins/native-widgets/tests/kotlin_conformance.rs` instead, beside the
//! code they protect.
//!
//! This is a plain `std::fs` source scan run as an ordinary `cargo test` (no
//! lint-plugin tooling exists in this repo — see `docs/CODE_STANDARDS.md`);
//! precedent: `crates/frust/tests/surface_mode_conformance.rs` and this
//! crate's own `tests/print_free_cores.rs`. A substring/line scan, not a
//! parser — correct for the small, hand-written shapes these files take
//! today.
//!
//! # No plugin Kotlin may use the bare `dev.frust` package
//!
//! `docs/CODE_STANDARDS.md`'s Plugin Conventions requires a plugin's Android
//! Kotlin to live in its own subpackage inside its own Gradle module
//! (`plugins/secure-storage`'s `dev.frust.securestorage`,
//! `plugins/camera`'s `dev.frust.camera`). `dev.frust` itself belongs
//! exclusively to the embedding module.
//!
//! **The one exception is closed.** `frust-native-widgets` used
//! to put its two generic classes (`FrustNativeControlFactory`,
//! `FrustNativeListener`) directly in the bare `dev.frust` package and
//! hand-copy them into the consuming app, on the grounds that the package is
//! baked into their JNI export symbol names. A breaking symbol
//! rename fixed this: both classes now sit in `dev.frust.nativewidgets` inside
//! `plugins/native-widgets/platform/android`, this plugin's own
//! `com.android.library` module, and the exports are spelled
//! `Java_dev_frust_nativewidgets_*`. So the allowlist below is **empty**, and
//! it should stay that way — the next plugin needing a fixed JNI package has
//! the same subpackage answer available to it.
//!
//! [`no_plugin_kotlin_uses_the_bare_dev_frust_package`] therefore fails if
//! ANY `.kt` file under `plugins/**` declares the bare `package dev.frust`.
//! Scoped to `plugins/**` only — the embedding module's own
//! `crates/frust-shell-android/platform/android/frust-embedding` legitimately ships bare `dev.frust`
//! Kotlin and must never trip this scan. Non-`.kt` files under `plugins/**`
//! (Swift included — see the Scope section above) are walked but deliberately
//! filtered out before the bare-package check runs; that filter is this
//! test's Kotlin-only scope boundary, not an oversight.
//!
//! # A plugin's platform files are found through its package
//!
//! `frust_drive::plugin::add_plugin` wires a plugin's Gradle module and Swift
//! package from wherever cargo locates the plugin's own crate — a checkout's
//! `plugins/<dir>` under a `frust` path dependency, the unpacked crates.io
//! release under a version. The `plugin_package_location` cases below hold
//! both halves of that to the real tree: every platform directory the
//! registry names exists inside the package its base Cargo dependency adds
//! (what a located crate has to carry), and the public `add_plugin_with`
//! seam writes the right Cargo line and platform paths for a project
//! scaffolded in each mode — through a stubbed locator, so no fixture runs
//! cargo. They need the `test-util` stub, which the packaged crate (no self
//! dev-dependency) does not build, so they compile only alongside it.

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust-drive`) so the scan is working-directory-independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust-drive has a grandparent (the workspace root)")
        .to_path_buf()
}

/// `path` relative to the workspace root, forward-slashed, for stable
/// failure messages and allowlist keys independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Directory names excluded from the walk wholesale — build outputs, never
/// source. `plugins/native-widgets/platform/android` is a
/// real `com.android.library` module: a direct Gradle invocation
/// there populates `build/`/`.gradle/` with **generated** Kotlin, which would
/// otherwise be judged by the bare-package rule below (wrongly — generated
/// code is not a plugin author's hand-written package declaration) and would
/// inflate the liveness guard's count, silently measuring the wrong thing. A
/// bare Cargo `target/` sits beside plugin crates for the same reason. This
/// list is a name match at any depth, not a path — `plugins/**/build/**` and
/// `plugins/**/.gradle/**` both get skipped, wherever they occur.
const EXCLUDED_DIR_NAMES: &[&str] = &["build", ".gradle", "target"];

/// True if `name` names one of [`EXCLUDED_DIR_NAMES`].
fn is_excluded_dir_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| EXCLUDED_DIR_NAMES.contains(&name))
}

/// The walk's single root-level entry point — **loud, not silent**: a
/// swallowed `read_dir` error here used to make `all_plugin_files()`
/// return an empty `Vec` if `plugins/` itself were missing, renamed, or
/// unreadable, and an empty scan made every assertion below pass vacuously.
/// Precedent: `print_free_cores.rs`'s own `walk` panics the same way. Every
/// deeper directory reached during recursion goes through [`walk_dir`]
/// instead, which is deliberately more lenient — see its own doc.
fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("reading directory {}: {e}", dir.display()));
    walk_entries(dir, entries, out);
}

/// Recurse into `dir`, tolerating a `read_dir` failure as "already gone,
/// nothing to scan" rather than panicking — unlike
/// [`walk_files`]'s root-level call, a subdirectory reached during recursion
/// can legitimately vanish between the parent listing it and this call
/// visiting it (a concurrent or incremental Gradle run under one of
/// `EXCLUDED_DIR_NAMES`), and that race is not a conformance defect this test
/// exists to catch.
fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        walk_entries(dir, entries, out);
    }
}

fn walk_entries(dir: &Path, entries: fs::ReadDir, out: &mut Vec<PathBuf>) {
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        // `Path::metadata` follows symlinks and, unlike `Path::is_dir`
        // (which silently reports `false` on any error, broken symlink
        // included), surfaces a broken symlink as `Err` — skipped outright
        // here instead of falling into the `else` arm below, getting pushed
        // onto `out`, and panicking `read_to_string` later.
        let Ok(meta) = path.metadata() else {
            continue;
        };
        if meta.is_dir() {
            let excluded = path.file_name().is_some_and(is_excluded_dir_name);
            if excluded {
                continue;
            }
            walk_dir(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Every file under `plugins/` of any extension (Kotlin, Swift, TOML, …),
/// sorted for a stable failure order. The Kotlin-only scope boundary lives at
/// the call site's extension filter, not here — this helper itself makes no
/// claim about which files matter to any particular check.
fn all_plugin_files() -> Vec<PathBuf> {
    let root = workspace_root().join("plugins");
    let mut out = Vec::new();
    walk_files(&root, &mut out);
    out.sort();
    out
}

/// Files allowed to declare the bare `package dev.frust` under `plugins/**`.
///
/// **Empty, and meant to stay empty** (see the module doc). It is kept as a
/// named, empty constant rather than
/// deleted so that adding an entry is a deliberate, reviewable act with an
/// obvious place to justify itself, instead of a quiet edit to the assertion
/// below. `docs/CODE_STANDARDS.md`'s Plugin Conventions has no exception
/// clause for a new entry to point at.
const BARE_DEV_FRUST_ALLOWLIST: &[&str] = &[];

/// True if `contents`' package declaration is the bare `dev.frust` — NOT a
/// subpackage like `dev.frust.camera`/`dev.frust.securestorage`, which are
/// the correct, unexceptional shape every other plugin follows.
fn declares_bare_dev_frust(contents: &str) -> bool {
    contents
        .lines()
        .any(|line| line.trim() == "package dev.frust")
}

#[test]
fn no_plugin_kotlin_uses_the_bare_dev_frust_package() {
    let all_files = all_plugin_files();
    let kotlin_files: Vec<&PathBuf> = all_files
        .iter()
        // Kotlin-only, deliberately: the bare-`dev.frust`-package exception
        // this test pins is a JNI-export-symbol-naming concept with no Swift
        // equivalent (see this file's module doc, "Scope: Kotlin only, by
        // design"). Every non-`.kt` file under `plugins/**` — Swift included
        // — is walked above and skipped here on purpose, not silently missed.
        .filter(|path| path.extension().is_some_and(|ext| ext == "kt"))
        .collect();

    // Liveness guard: the two assertions below are satisfied
    // vacuously by an empty set, so a scan that silently walked nothing would
    // make this test pass while proving nothing — the exact failure mode this
    // test exists to catch, since the allowlist below is empty (see the
    // module doc).
    // This has to hold for the scan's current, real shape: as of this writing
    // `plugins/secure-storage`, `plugins/camera`, and
    // `plugins/native-widgets` each ship real `.kt` files under `plugins/**`,
    // so "zero `.kt` files scanned" is never legitimate — it means either
    // `plugins/` was walked from the wrong root, or a plugin's Android Kotlin
    // moved somewhere `all_plugin_files()` no longer reaches. If a future
    // change legitimately drops every plugin's Kotlin (all platform plugins
    // going Swift/iOS-only, say), this guard must be revisited deliberately,
    // not silently satisfied.
    assert!(
        !kotlin_files.is_empty(),
        "scanned {} file(s) under plugins/** but found zero `.kt` files among them — this test \
         cannot prove anything about the bare `dev.frust` package rule without seeing real \
         Kotlin. Either `plugins/` was walked from the wrong root, or a plugin's Android Kotlin \
         moved somewhere all_plugin_files() no longer reaches; this is a liveness guard for the \
         scan itself, separate from (and prior to) the bare-package check below",
        all_files.len(),
    );

    let mut found_bare: Vec<String> = Vec::new();

    for path in kotlin_files {
        let contents =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if declares_bare_dev_frust(&contents) {
            found_bare.push(rel(path));
        }
    }

    let mut unexpected: Vec<&String> = found_bare
        .iter()
        .filter(|p| !BARE_DEV_FRUST_ALLOWLIST.contains(&p.as_str()))
        .collect();
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "found {} `.kt` file(s) under plugins/** declaring the bare `package dev.frust`: {:?} — \
         docs/CODE_STANDARDS.md's Plugin Conventions requires a plugin's Android Kotlin to live \
         in its OWN subpackage inside its OWN Gradle module (e.g. `dev.frust.<plugin>`, see \
         plugins/secure-storage's `dev.frust.securestorage`, plugins/camera's `dev.frust.camera`, \
         plugins/native-widgets's `dev.frust.nativewidgets`); `dev.frust` itself belongs \
         exclusively to the embedding module. There is no exception clause to point at — \
         frust-native-widgets's time-boxed one was closed when its JNI \
         exports were renamed to `Java_dev_frust_nativewidgets_*` rather than keep the bare package",
        unexpected.len(),
        unexpected,
    );

    // Stale-allowlist check: the allowlist is empty by design (its own doc
    // comment) and has nothing to go stale today, but the check itself stays
    // live — the moment a future entry is added without a matching bare-`dev.
    // frust` file to justify it, this is what catches the drift.
    let mut missing: Vec<&str> = BARE_DEV_FRUST_ALLOWLIST
        .iter()
        .filter(|p| !found_bare.contains(&p.to_string()))
        .copied()
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "expected these allowlisted files to declare `package dev.frust` but they don't (a stale \
         allowlist entry — the allowlist is supposed to be empty): {missing:?}"
    );
}

#[test]
fn scanner_rejects_a_subpackage_as_bare_dev_frust() {
    assert!(!declares_bare_dev_frust("package dev.frust.camera\n"));
    assert!(!declares_bare_dev_frust(
        "package dev.frust.securestorage\n"
    ));
    assert!(declares_bare_dev_frust("package dev.frust\n"));
    assert!(declares_bare_dev_frust(
        "// a comment\npackage dev.frust\n\nimport android.view.View\n"
    ));
}

#[cfg(feature = "test-util")]
mod plugin_package_location {
    use super::{rel, workspace_root};
    use frust_drive::host_path::to_portable_string;
    use frust_drive::packages::StubLocator;
    use frust_drive::plugin::{AddOutcome, Contribution, add_plugin_with, known_plugins};
    use frust_drive::scaffold::{FrustDependency, TemplateContext, generate};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-plugin-package-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A freshly scaffolded app under `<scratch>/app` depending on frust as
    /// `frust` says, returning `(scratch, app)`.
    fn scaffolded(tag: &str, frust: FrustDependency) -> (PathBuf, PathBuf) {
        let scratch = unique_temp_dir(tag);
        let app = scratch.join("app");
        let ctx = TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: "0.5.0".into(),
            frust,
            deeplink_scheme: None,
            deeplink_host: None,
        };
        generate(&app, &ctx, None, false, None).expect("scaffold generate");
        (scratch, app)
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// The `[package] name` of the crate at `dir`.
    fn package_name(dir: &Path) -> String {
        let doc = read(&dir.join("Cargo.toml"))
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        doc["package"]["name"].as_str().unwrap().to_string()
    }

    /// Every platform directory the registry names exists in this checkout
    /// inside `plugins/<crate_dir>/`, the directory of the very package the
    /// plugin's base Cargo dependency adds — so locating that package, by
    /// path or as a downloaded release, finds the directory under it.
    #[test]
    fn every_registry_platform_directory_ships_inside_its_plugins_package() {
        let root = workspace_root();
        let mut checked = 0;
        for spec in known_plugins() {
            let package = spec
                .base
                .iter()
                .find_map(|c| match c {
                    Contribution::CargoDep { name } => Some(*name),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("`{}` adds no Cargo dependency", spec.id));
            let crate_root = root.join("plugins").join(spec.crate_dir);
            let contributions = spec
                .base
                .iter()
                .chain(spec.optional_features.iter().flat_map(|f| f.contributions));
            for contribution in contributions {
                let rel_path = match contribution {
                    Contribution::GradleModule { rel_path, .. }
                    | Contribution::SwiftPackageRef { rel_path, .. } => *rel_path,
                    _ => continue,
                };
                let prefix = format!("plugins/{}/", spec.crate_dir);
                assert!(
                    rel_path.starts_with(&prefix),
                    "`{}` names `{rel_path}` outside its own package `{prefix}`",
                    spec.id
                );
                assert!(
                    root.join(rel_path).is_dir(),
                    "`{}` names `{rel_path}`, which is not a directory in this checkout",
                    spec.id
                );
                assert_eq!(
                    package_name(&crate_root),
                    package,
                    "`{}`'s platform files live in {}, whose package is not the `{package}` \
                     its Cargo dependency adds",
                    spec.id,
                    rel(&crate_root)
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no Gradle module or Swift package found in the registry — the scan proved nothing"
        );
    }

    /// Registry mode, the `frust create` default: the plugin dependency takes
    /// the facade's version, and the Gradle module and Swift package point
    /// into the crate cargo unpacked.
    #[test]
    fn registry_mode_add_writes_a_version_and_wires_the_unpacked_crate() {
        let (scratch, app) = scaffolded(
            "registry",
            FrustDependency::Registry {
                version: "0.5.0".into(),
            },
        );
        let unpacked = scratch.join("registry-src/frust-iap-0.5.0");
        let locator = StubLocator::new().with("frust-iap", unpacked.clone());

        let report = add_plugin_with(&locator, &app, "iap", &[]).expect("add iap");
        assert!(
            report
                .items
                .iter()
                .all(|i| i.outcome == AddOutcome::Applied),
            "{report:?}"
        );

        let cargo = read(&app.join("Cargo.toml"));
        assert!(cargo.contains("frust-iap = \"0.5.0\""), "{cargo}");
        let settings = read(&app.join("android/settings.gradle.kts"));
        let module = to_portable_string(&unpacked.join("platform/android"));
        assert!(
            settings.contains(&format!(
                "project(\":frust-iap\").projectDir = file(\"{module}\")"
            )),
            "{settings}"
        );
        let pbxproj = read(&app.join("ios/Runner.xcodeproj/project.pbxproj"));
        let package = to_portable_string(&unpacked.join("platform/ios/FrustIap"));
        assert!(
            pbxproj.contains(&format!("relativePath = \"{package}\";")),
            "{pbxproj}"
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// A registry project the locator cannot answer for (offline, say) is a
    /// typed refusal naming the package; no platform file is edited.
    #[test]
    fn registry_mode_add_without_a_located_crate_refuses_the_platform_edits() {
        let (scratch, app) = scaffolded(
            "registry-offline",
            FrustDependency::Registry {
                version: "0.5.0".into(),
            },
        );
        let settings_before = read(&app.join("android/settings.gradle.kts"));
        let offline = StubLocator::failing("error: failed to download `frust-iap v0.5.0`");

        let err = add_plugin_with(&offline, &app, "iap", &[]).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("frust-iap"), "{message}");
        assert!(message.contains("failed to download"), "{message}");
        assert_eq!(
            read(&app.join("android/settings.gradle.kts")),
            settings_before
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Path mode keeps its Cargo line — a path into the same checkout as the
    /// `frust` dependency — while the platform files come from the located
    /// package, as an absolute path.
    #[test]
    fn path_mode_add_keeps_its_path_line_and_wires_the_located_package() {
        let (scratch, app) = scaffolded(
            "path",
            FrustDependency::Path("../checkout/crates/frust".into()),
        );
        let located = scratch.join("checkout/plugins/iap");
        let locator = StubLocator::new().with("frust-iap", located.clone());

        add_plugin_with(&locator, &app, "iap", &[]).expect("add iap");

        let cargo = read(&app.join("Cargo.toml"));
        assert!(
            cargo.contains("frust-iap = { path = \"../checkout/crates/frust/../../plugins/iap\" }"),
            "{cargo}"
        );
        let settings = read(&app.join("android/settings.gradle.kts"));
        let module = to_portable_string(&located.join("platform/android"));
        assert!(
            settings.contains(&format!("file(\"{module}\")")),
            "{settings}"
        );
        let _ = fs::remove_dir_all(&scratch);
    }
}
