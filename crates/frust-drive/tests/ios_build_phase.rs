//! Host-side tripwire for the iOS "Build Rust staticlib" shell-script build
//! phase (`crates/frust-drive/templates/app/ios.tmpl/Runner.xcodeproj/project.pbxproj.tmpl`):
//! the phase must resolve the directory `cargo build` actually wrote to
//! through `cargo metadata` (mirroring
//! `desktop_build::cargo::resolve_target_dir`'s own precedence) rather than
//! assuming the literal `target/$TRIPLE` — a shared/global
//! `build.target-dir` (this very machine's `CARGO_TARGET_DIR` convention is
//! exactly that case) silently loses the built `.a` under the old
//! assumption.
//!
//! This lives in a *new* integration test file rather than the existing
//! `scaffold::tests` module — `crates/frust-drive/src/scaffold/mod.rs` is
//! out of this task's write scope (another task edits it concurrently) — but
//! follows the same `generate` + `TemplateContext` rendering pattern that
//! module's own iOS test (`generate_produces_ios_tree_with_stripped_dir_suffix_and_substitutions`)
//! uses. No Xcode toolchain is available on this host, so this test only
//! proves the *rendered* shell script text — not a real Xcode build; the
//! device/Mac gate remains the actual build proof.

use frust_drive::scaffold::{TemplateContext, generate};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

fn unique_temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "frust-drive-ios-build-phase-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn test_context() -> TemplateContext {
    TemplateContext {
        project_name: "my_app".into(),
        title_case_name: "My App".into(),
        org: "dev.f0x".into(),
        description: "A new Frust application.".into(),
        frust_version: "0.1.0".into(),
        frust_path: "/path/to/frust".into(),
        deeplink_scheme: None,
        deeplink_host: None,
    }
}

fn rendered_pbxproj() -> String {
    let dest = unique_temp_dir("pbxproj");
    let ctx = test_context();
    generate(&dest, &ctx, None, false, None).expect("scaffold generate");
    let pbxproj = dest.join("ios/Runner.xcodeproj/project.pbxproj");
    let text = fs::read_to_string(&pbxproj).expect("read generated pbxproj");
    let _ = fs::remove_dir_all(&dest);
    text
}

/// (a) No literal `target/$TRIPLE` assumption survives rendering — the
/// build phase must resolve the directory cargo actually used, not hardcode
/// cargo's own default.
#[test]
fn ios_build_phase_never_assumes_the_literal_target_triple_path() {
    let pbxproj = rendered_pbxproj();
    assert!(
        !pbxproj.contains("target/$TRIPLE"),
        "rendered pbxproj must not contain a literal `target/$TRIPLE` path:\n{pbxproj}"
    );
}

/// (b) The phase resolves the target directory through `cargo metadata`,
/// the same source of truth `desktop_build::cargo::resolve_target_dir` reads
/// (mirrored here in shell since the phase runs outside `frust-drive`'s own
/// process).
#[test]
fn ios_build_phase_resolves_target_dir_via_cargo_metadata() {
    let pbxproj = rendered_pbxproj();
    assert!(
        pbxproj.contains("cargo metadata --format-version 1 --no-deps"),
        "rendered pbxproj must call `cargo metadata --format-version 1 --no-deps`:\n{pbxproj}"
    );
    // `CARGO_TARGET_DIR` (when the environment already names one) must still
    // win over asking cargo, the same precedence
    // `desktop_build::cargo::resolve_target_dir` documents.
    assert!(
        pbxproj.contains("${CARGO_TARGET_DIR:-$(cargo metadata"),
        "rendered pbxproj must prefer `CARGO_TARGET_DIR` over `cargo metadata` when set:\n{pbxproj}"
    );
}

/// (c) The `cargo build` invocation itself, and the `FRUST_FEATURES` splice
/// feeding it, are unchanged by this task — only where the built artifact is
/// *found afterward* changes.
#[test]
fn ios_build_phase_keeps_the_cargo_build_invocation_and_features_splice_unchanged() {
    let pbxproj = rendered_pbxproj();
    assert!(
        pbxproj.contains("cargo build --target \\\"$TRIPLE\\\" $CARGO_FLAGS $CARGO_FEATURES"),
        "rendered pbxproj must still run `cargo build --target \"$TRIPLE\"` unmodified:\n{pbxproj}"
    );
    assert!(
        pbxproj.contains(
            "CARGO_FEATURES=\\\"--features $(echo \\\"$FRUST_FEATURES\\\" | base64 -d)\\\""
        ),
        "rendered pbxproj must still splice FRUST_FEATURES unmodified:\n{pbxproj}"
    );
}

/// The `.a` is still located via `$TRIPLE`/`$PROFILE_DIR`/the project name —
/// only the directory it is rooted under is now resolved dynamically.
#[test]
fn ios_build_phase_copies_the_resolved_staticlib_into_built_products_dir() {
    let pbxproj = rendered_pbxproj();
    assert!(
        pbxproj.contains(
            "cp \\\"$TARGET_DIR/$TRIPLE/$PROFILE_DIR/libmy_app.a\\\" \\\"$BUILT_PRODUCTS_DIR/\\\""
        ),
        "rendered pbxproj must copy the resolved staticlib path:\n{pbxproj}"
    );
}

/// Xcode runs script build phases with its own PATH, which does not include
/// rustup's `~/.cargo/bin`, so a Run from the Xcode UI failed with
/// `cargo: command not found` (only the frust CLI, which inherits the shell's
/// PATH, could build). The phase must put `${CARGO_HOME:-$HOME/.cargo}/bin`
/// on PATH before the first `cargo` invocation.
#[test]
fn ios_build_phase_puts_rustups_bin_dir_on_path_before_cargo() {
    let text = rendered_pbxproj();
    let export = text
        .find(r#"export PATH=\"${CARGO_HOME:-$HOME/.cargo}/bin:$PATH\""#)
        .expect("build phase exports rustup's bin dir on PATH");
    let first_cargo = text
        .find("cargo build --target")
        .expect("build phase runs cargo build");
    assert!(
        export < first_cargo,
        "PATH must be set before the first cargo invocation"
    );
}
