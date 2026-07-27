//! Source-scan conformance test guarding the bare `dev.frust` Kotlin package
//! exception (`docs/CODE_STANDARDS.md`'s Plugin Conventions) — a
//! cross-cutting invariant that must see every plugin under `plugins/**`, not
//! just one.
//!
//! **Relocated by task c1-04** from
//! `crates/frust/tests/plugin_kotlin_conformance.rs`'s Part 2 (f2-06), which
//! originally lived in the published facade crate's own test suite —
//! coupling `cargo test -p frust` to every plugin's Kotlin layout, in tension
//! with `docs/ARCHITECTURE.md`'s "the facade never depends on or re-exports a
//! plugin." `frust-drive` is the right host instead: it is an explicit leaf
//! with no `frust-*`/framework-crate dependencies, it already owns the
//! analogous `print_free_cores.rs` source scan, and it is not the app-facing
//! published facade. The `frust-native-widgets`-specific parity/packing/
//! byte-identity checks from the same original file moved to
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
//! # The `dev.frust` exception must not spread
//!
//! `docs/CODE_STANDARDS.md`'s Plugin Conventions requires a plugin's Android
//! Kotlin to live in its own subpackage inside its own Gradle module
//! (`plugins/secure-storage`'s `dev.frust.securestorage` is the precedent
//! this scan expects everyone else to follow). `frust-native-widgets` has a
//! documented, time-boxed exception putting its two generic classes
//! (`FrustNativeControlFactory`, `FrustNativeListener`) directly in the bare
//! `dev.frust` package, because that package is baked into their JNI export
//! symbol names — moving them is a breaking symbol rename, correctly
//! deferred to Phase 3 packaging and NOT attempted here.
//!
//! [`only_the_two_known_files_use_the_bare_dev_frust_package`] fails if any
//! OTHER `.kt` file under `plugins/**` declares the bare `package dev.frust`
//! (a second plugin copying the exception rather than following the
//! `dev.frust.<plugin>` precedent), and fails just as loudly if either of
//! the two allowlisted files stops declaring it (a stale allowlist entry is
//! as much drift as a new violation). Scoped to `plugins/**` only — the
//! embedding module's own `platform/android/frust-embedding` legitimately
//! ships bare `dev.frust` Kotlin and must never trip this scan.

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

/// Every file under `plugins/`, sorted for a stable failure order.
fn all_plugin_files() -> Vec<PathBuf> {
    let root = workspace_root().join("plugins");
    let mut out = Vec::new();
    walk_files(&root, &mut out);
    out.sort();
    out
}

/// Files allowed to declare the bare `package dev.frust` under `plugins/**`
/// — `frust-native-widgets`'s documented, time-boxed exception
/// (`docs/CODE_STANDARDS.md`'s Plugin Conventions), because the package is
/// baked into these two classes' JNI export symbol names. Moving them is a
/// breaking symbol rename, correctly deferred to Phase 3 packaging.
const BARE_DEV_FRUST_ALLOWLIST: &[&str] = &[
    "plugins/native-widgets/platform/android/FrustNativeControlFactory.kt",
    "plugins/native-widgets/platform/android/FrustNativeListener.kt",
];

/// True if `contents`' package declaration is the bare `dev.frust` — NOT a
/// subpackage like `dev.frust.camera`/`dev.frust.securestorage`, which are
/// the correct, unexceptional shape every other plugin follows.
fn declares_bare_dev_frust(contents: &str) -> bool {
    contents
        .lines()
        .any(|line| line.trim() == "package dev.frust")
}

#[test]
fn only_the_two_known_files_use_the_bare_dev_frust_package() {
    let mut found_bare: Vec<String> = Vec::new();

    for path in all_plugin_files() {
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
        "found {} `.kt` file(s) under plugins/** declaring the bare `package dev.frust` outside \
         the allowlisted native-widgets exception ({} hit(s)): {:?} — docs/CODE_STANDARDS.md's \
         Plugin Conventions requires a plugin's own subpackage (e.g. `dev.frust.<plugin>`, see \
         plugins/secure-storage's `dev.frust.securestorage`); the bare package is a documented, \
         time-boxed exception for frust-native-widgets's two JNI-symbol-fixed classes ONLY — see \
         the exception clause in docs/CODE_STANDARDS.md and the Phase 3 packaging task that \
         closes it",
        unexpected.len(),
        unexpected.len(),
        unexpected,
    );

    let mut missing: Vec<&str> = BARE_DEV_FRUST_ALLOWLIST
        .iter()
        .filter(|p| !found_bare.contains(&p.to_string()))
        .copied()
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "expected these allowlisted files to declare `package dev.frust` but they don't (a stale \
         allowlist entry, or the exception was actually closed and this list needs shrinking): \
         {missing:?}"
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
