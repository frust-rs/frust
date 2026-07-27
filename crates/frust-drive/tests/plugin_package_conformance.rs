//! Source-scan conformance test guarding the bare `dev.frust` Kotlin package
//! rule (`docs/CODE_STANDARDS.md`'s Plugin Conventions) — a cross-cutting
//! invariant that must see every plugin's **Kotlin** under `plugins/**`, not
//! just one.
//!
//! # Scope: Kotlin only, by design — not "every file under `plugins/**`"
//!
//! **Task p3-04 narrowed this claim (Option B)** after a conformance sweep
//! found the scan reporting green while silently skipping Swift
//! (`workflow/plans/features/frust-native-widgets/research/RESEARCH-P3.md`
//! §8; flagged as a latent trap earlier in
//! `RESEARCH-P2-REFRESH.md` §7). The bare-`dev.frust`-package rule this test
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
//! **Relocated by task c1-04** from
//! `crates/frust/tests/plugin_kotlin_conformance.rs`'s Part 2 (f2-06), which
//! originally lived in the published facade crate's own test suite —
//! coupling `cargo test -p frust` to every plugin's Kotlin layout, in tension
//! with `docs/ARCHITECTURE.md`'s "the facade never depends on or re-exports a
//! plugin." `frust-drive` is the right host instead: it is an explicit leaf
//! with no `frust-*`/framework-crate dependencies, it already owns the
//! analogous `print_free_cores.rs` source scan, and it is not the app-facing
//! published facade. The `frust-native-widgets`-specific parity/packing
//! checks from the same original file moved to
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
//! **The one exception is closed (task p3-03).** `frust-native-widgets` used
//! to put its two generic classes (`FrustNativeControlFactory`,
//! `FrustNativeListener`) directly in the bare `dev.frust` package and
//! hand-copy them into the consuming app, on the grounds that the package is
//! baked into their JNI export symbol names. p3-03 did the breaking symbol
//! rename: both classes now sit in `dev.frust.nativewidgets` inside
//! `plugins/native-widgets/platform/android`, this plugin's own
//! `com.android.library` module, and the exports are spelled
//! `Java_dev_frust_nativewidgets_*`. So the allowlist below is **empty**, and
//! it should stay that way — the next plugin needing a fixed JNI package has
//! the same subpackage answer available to it.
//!
//! [`no_plugin_kotlin_uses_the_bare_dev_frust_package`] therefore fails if
//! ANY `.kt` file under `plugins/**` declares the bare `package dev.frust`.
//! Scoped to `plugins/**` only — the embedding module's own
//! `platform/android/frust-embedding` legitimately ships bare `dev.frust`
//! Kotlin and must never trip this scan. Non-`.kt` files under `plugins/**`
//! (Swift included — see the Scope section above) are walked but deliberately
//! filtered out before the bare-package check runs; that filter is this
//! test's Kotlin-only scope boundary, not an oversight.

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

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            walk_files(&path, out);
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
/// **Empty, and meant to stay empty** (task p3-03 closed the one exception —
/// see the module doc). It is kept as a named, empty constant rather than
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
    let mut found_bare: Vec<String> = Vec::new();

    for path in all_plugin_files() {
        // Kotlin-only, deliberately: the bare-`dev.frust`-package exception
        // this test pins is a JNI-export-symbol-naming concept with no Swift
        // equivalent (see this file's module doc, "Scope: Kotlin only, by
        // design"). Every non-`.kt` file under `plugins/**` — Swift included
        // — is walked above and skipped here on purpose, not silently missed.
        if path.extension().is_none_or(|ext| ext != "kt") {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if declares_bare_dev_frust(&contents) {
            found_bare.push(rel(&path));
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
         frust-native-widgets's time-boxed one was closed by task p3-03, which renamed its JNI \
         exports to `Java_dev_frust_nativewidgets_*` rather than keep the bare package",
        unexpected.len(),
        unexpected,
    );

    // No stale-allowlist check: the allowlist is empty by design (its own doc
    // comment), so there is nothing that could go stale. Re-adding one means
    // re-adding this check too — an entry nothing verifies is exactly the
    // drift the p3-04 version of this test was built to catch.
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
