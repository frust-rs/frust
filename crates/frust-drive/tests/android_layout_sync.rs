//! Host-side tripwire for the Android Gradle build-directory redirect: every
//! hand-written path literal in the rendered `crates/frust-drive/templates/app/android.tmpl/`
//! tree (`settings.gradle.kts`'s `beforeProject` redirects and the plugin
//! include block, `app/build.gradle.kts`'s `jniLibs` staging directory) must
//! agree with [`frust_drive::build_dirs::BuildLayout`] — the single source of
//! truth `frust-drive`'s own Android pipelines resolve their output through
//! (see `crates/frust-drive/src/build_dirs.rs`'s module doc). Every expected
//! string here is *derived* from `BuildLayout`'s accessors, never typed as a
//! literal, so a `BuildLayout` change that isn't mirrored in the template
//! fails this test instead of silently drifting (see
//! `docs/REVIEW_FOCUS.md`'s hand-synced-literal priority).
//!
//! This lives in a new integration test file rather than
//! `crates/frust-drive/src/scaffold/mod.rs`'s own test module — that module
//! is also in this task's write scope, but for its *other* two tripwires
//! (the `.cargo/config.toml` key-set comparison and the Windows icon-path
//! pin), not this one — and follows the same `generate` + `TemplateContext`
//! rendering pattern `crates/frust-drive/tests/ios_build_phase.rs` uses. No
//! Gradle toolchain is available on this host, so this test only proves the
//! *rendered* Kotlin DSL text — not a real Gradle build.

use frust_drive::build_dirs::BuildLayout;
use frust_drive::host_path::to_portable_string;
use frust_drive::plugin::add_plugin;
use frust_drive::scaffold::{TemplateContext, generate};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

fn unique_temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "frust-drive-android-layout-sync-{tag}-{}-{n}",
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
        frust: frust_drive::scaffold::FrustDependency::Path("/path/to/frust".into()),
        deeplink_scheme: None,
        deeplink_host: None,
    }
}

/// Renders a fresh app tree and returns its destination directory — the
/// caller reads whichever generated file(s) it needs and is responsible for
/// cleaning the directory up.
fn rendered_app(tag: &str) -> PathBuf {
    let dest = unique_temp_dir(tag);
    let ctx = test_context();
    generate(&dest, &ctx, None, false, None).expect("scaffold generate");
    dest
}

/// `settings.gradle.kts`'s three built-in `beforeProject` redirects (the
/// root project, `:app`, and `:frust-embedding`) must resolve exactly the
/// directories `BuildLayout::android_root`/`android_app`/`android_embedding`
/// name, from Gradle's own `rootDir` (the project's `android/` directory) —
/// i.e. `../<BuildLayout accessor>`.
#[test]
fn settings_gradle_beforeproject_redirects_match_build_layout() {
    let dest = rendered_app("settings-redirects");
    let settings = fs::read_to_string(dest.join("android/settings.gradle.kts"))
        .expect("read generated settings.gradle.kts");

    // The generated `settings.gradle.kts` is a static, hand-written
    // forward-slash literal (never rendered from `BuildLayout` at scaffold
    // time — see the template's own comment), so the expectation here must
    // compare the same portable form rather than `BuildLayout`'s own
    // `PathBuf`s, which `Display` with the host's native separator
    // (backslash on Windows).
    let root_redirect = format!(
        "rootDir.resolve(\"../{}\")",
        to_portable_string(&BuildLayout::android_root())
    );
    let app_redirect = format!(
        "rootDir.resolve(\"../{}\")",
        to_portable_string(&BuildLayout::android_app())
    );
    let embedding_redirect = format!(
        "rootDir.resolve(\"../{}\")",
        to_portable_string(&BuildLayout::android_embedding())
    );

    assert!(
        settings.contains(&root_redirect),
        "settings.gradle.kts must redirect the root project's buildDirectory to \
         `{root_redirect}` (BuildLayout::android_root()):\n{settings}"
    );
    assert!(
        settings.contains(&app_redirect),
        "settings.gradle.kts must redirect `:app`'s buildDirectory to \
         `{app_redirect}` (BuildLayout::android_app()):\n{settings}"
    );
    assert!(
        settings.contains(&embedding_redirect),
        "settings.gradle.kts must redirect `:frust-embedding`'s buildDirectory to \
         `{embedding_redirect}` (BuildLayout::android_embedding()):\n{settings}"
    );

    let _ = fs::remove_dir_all(&dest);
}

/// `app/build.gradle.kts` stages native libraries into
/// `BuildLayout::android_jni_libs()`, resolved as `../../<accessor>` from the
/// module directory (`android/app/`) — both in the `cargo ndk -o` argument
/// and in the `jniLibs.setSrcDirs(...)` call that makes it the *only*
/// packaging input (AGP's own `src/main/jniLibs` default is replaced, not
/// merely supplemented).
#[test]
fn app_build_gradle_jni_libs_dir_matches_build_layout_in_both_sites() {
    let dest = rendered_app("jnilibs-dir");
    let build_gradle = fs::read_to_string(dest.join("android/app/build.gradle.kts"))
        .expect("read generated app/build.gradle.kts");

    // Same portable-comparison rule as the `settings.gradle.kts` test above:
    // `app/build.gradle.kts` is a static forward-slash literal, so the
    // expectation must compare the portable form of `BuildLayout`'s
    // `PathBuf`, not its native-separator `Display`.
    let jni_libs_path = format!(
        "../../{}",
        to_portable_string(&BuildLayout::android_jni_libs())
    );

    let ndk_output_site = format!("file(\"{jni_libs_path}\").absolutePath");
    assert!(
        build_gradle.contains(&ndk_output_site),
        "app/build.gradle.kts's cargo-ndk `-o` argument must resolve \
         `{jni_libs_path}` (BuildLayout::android_jni_libs()):\n{build_gradle}"
    );

    let src_dirs_site = format!("setSrcDirs(listOf(\"{jni_libs_path}\"))");
    assert!(
        build_gradle.contains(&src_dirs_site),
        "app/build.gradle.kts's `jniLibs.setSrcDirs(...)` must REPLACE AGP's \
         default source set with `{jni_libs_path}` (BuildLayout::android_jni_libs()):\n{build_gradle}"
    );

    let _ = fs::remove_dir_all(&dest);
}

/// A plugin's Android library module — applied through the public
/// `frust_drive::plugin::add_plugin` API, the same idempotent mutation
/// plugin add (`plugin::add_plugin`) performs — gets the identical
/// `<app>/build/android/<module>` redirect treatment as the built-in
/// `:frust-embedding` module above, derived from
/// `BuildLayout::android_module`. `camera`'s base contribution set includes
/// exactly one `Contribution::GradleModule` (`:frust-camera`) and requires no
/// sibling checkout, so it applies cleanly against a freshly rendered
/// project tree with no extra fixture setup.
// Like the two tests above, the expectation is built using `to_portable_string`
// to ensure forward slashes on all platforms, matching the production renderer
// in `plugin::apply` which uses `to_portable_string` to emit portable paths
// (avoiding raw backslashes in generated Kotlin strings on Windows).
#[test]
fn plugin_gradle_module_include_block_redirect_matches_build_layout() {
    let dest = rendered_app("plugin-module-redirect");

    add_plugin(&dest, "camera", &[]).expect("add_plugin(camera) against a freshly scaffolded app");

    let settings = fs::read_to_string(dest.join("android/settings.gradle.kts"))
        .expect("read settings.gradle.kts after add_plugin");

    let module_redirect = format!(
        "../{}",
        to_portable_string(&BuildLayout::android_module("frust-camera"))
    );
    let beforeproject_line = format!("rootDir.resolve(\"{module_redirect}\")");

    assert!(
        settings.contains(":frust-camera"),
        "settings.gradle.kts must gain the `:frust-camera` include after \
         add_plugin:\n{settings}"
    );
    assert!(
        settings.contains(&beforeproject_line),
        "the plugin's Gradle module include block must redirect its \
         buildDirectory to `{module_redirect}` (BuildLayout::android_module(\"frust-camera\")):\n{settings}"
    );

    let _ = fs::remove_dir_all(&dest);
}
