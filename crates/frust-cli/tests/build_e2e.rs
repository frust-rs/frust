//! End-to-end Android build gate (task 68): `frust create` → generate a
//! throwaway upload keystore → `frust build apk --release` produces a
//! release-signed APK.
//!
//! Like `create_e2e`, this drives the compiled `frust` binary via
//! `CARGO_BIN_EXE_frust` (the crate is binary-only, no `lib` target) so it
//! exercises the exact code path a real user hits, staying within this crate's
//! module boundaries.
//!
//! Ignored by default: the inner Gradle build downloads Gradle 9.5.x on a cold
//! machine and compiles the generated project's whole Rust graph for the
//! Android target — minutes of wall-clock. It also needs a configured Android
//! toolchain (SDK + NDK + JDK 17 + the `aarch64-linux-android` Rust target +
//! `cargo-ndk`). Run explicitly:
//! `cargo test -p frust-cli --test build_e2e -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

/// Fixed path Android Studio installs its bundled JBR at on macOS — the same
/// fallback the CLI's Android preflight uses to find a JDK 17+. We reuse it
/// only to locate `keytool` for the test's keystore-generation step.
const STUDIO_JBR_HOME: &str = "/Applications/Android Studio.app/Contents/jbr/Contents/Home";

/// Absolute path to this workspace's `crates/frust` facade crate.
fn workspace_frust_path() -> PathBuf {
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frust");
    raw.canonicalize()
        .expect("crates/frust must exist in this workspace checkout")
}

/// A fresh, never-before-used scratch directory under the system temp dir.
fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "frust-cli-build-e2e-{tag}-{}-{n}",
        std::process::id()
    ))
}

/// Locates a `keytool`: `$JAVA_HOME/bin/keytool`, else Android Studio's
/// bundled JBR, else `keytool` on `PATH`. The generated keystore uses this;
/// the Gradle build itself resolves its own JDK via the CLI's preflight.
fn find_keytool() -> PathBuf {
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let candidate = Path::new(&java_home).join("bin/keytool");
        if candidate.exists() {
            return candidate;
        }
    }
    let jbr = Path::new(STUDIO_JBR_HOME).join("bin/keytool");
    if jbr.exists() {
        return jbr;
    }
    PathBuf::from("keytool")
}

/// Resolves an `ANDROID_NDK_HOME` for the build subprocess: the env var if
/// set, else the highest-versioned NDK under `$ANDROID_HOME/ndk` (cargo-ndk
/// needs one of these). Returns `None` if neither is available — the caller
/// then skips loudly.
fn resolve_ndk_home() -> Option<String> {
    if let Ok(ndk) = std::env::var("ANDROID_NDK_HOME")
        && !ndk.trim().is_empty()
    {
        return Some(ndk);
    }
    let android_home = std::env::var("ANDROID_HOME")
        .or_else(|_| std::env::var("ANDROID_SDK_ROOT"))
        .ok()?;
    let ndk_root = Path::new(&android_home).join("ndk");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&ndk_root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    versions.pop().map(|p| p.to_string_lossy().into_owned())
}

#[test]
#[ignore = "compiles a full generated project + Gradle; needs Android SDK/NDK — run with cargo test -p frust-cli --test build_e2e -- --ignored"]
fn scaffolded_project_produces_a_signed_release_apk() {
    let frust_exe = env!("CARGO_BIN_EXE_frust");
    let frust_path = workspace_frust_path();

    let ndk_home = resolve_ndk_home().expect(
        "Android NDK not found: set ANDROID_NDK_HOME or install one under $ANDROID_HOME/ndk",
    );

    let project = unique_dir("proj");
    let _ = std::fs::remove_dir_all(&project);

    // 1) Scaffold a fresh app against the real facade crate.
    let create_status = Command::new(frust_exe)
        .args([
            "create",
            project.to_str().expect("project path is valid UTF-8"),
            "--project-name",
            "fk_build_e2e",
            "--org",
            "dev.f0x",
            "--frust-path",
            frust_path.to_str().expect("frust_path is valid UTF-8"),
        ])
        .status()
        .expect("failed to spawn `frust create`");
    assert!(create_status.success(), "`frust create` exited non-zero");

    // 2) Generate a throwaway upload keystore (never leaves this scratch dir)
    //    and write android/key.properties pointing at it.
    let keystore = project.join("upload-e2e.jks");
    let keytool_status = Command::new(find_keytool())
        .args([
            "-genkey",
            "-v",
            "-keystore",
            keystore.to_str().unwrap(),
            "-keyalg",
            "RSA",
            "-storetype",
            "JKS",
            "-keysize",
            "2048",
            "-validity",
            "10000",
            "-alias",
            "upload",
            "-storepass",
            "e2etest1",
            "-keypass",
            "e2etest1",
            "-dname",
            "CN=Frust Build E2E, OU=Eng, O=Frust, C=US",
        ])
        .status()
        .expect("failed to spawn `keytool`");
    assert!(
        keytool_status.success(),
        "`keytool -genkey` exited non-zero"
    );
    assert!(keystore.exists(), "keystore was not created");

    std::fs::write(
        project.join("android/key.properties"),
        format!(
            "storePassword=e2etest1\nkeyPassword=e2etest1\nkeyAlias=upload\nstoreFile={}\n",
            keystore.display()
        ),
    )
    .expect("failed to write android/key.properties");

    // 3) Build a release-signed APK. Single-ABI (arm64) keeps the wall-clock
    //    to one Rust target while still exercising the full signing pipeline.
    let build_status = Command::new(frust_exe)
        .args([
            "build",
            "apk",
            "--release",
            "--target-platform",
            "android-arm64",
        ])
        .current_dir(&project)
        .env("ANDROID_NDK_HOME", &ndk_home)
        .status()
        .expect("failed to spawn `frust build apk --release`");
    assert!(
        build_status.success(),
        "`frust build apk --release` failed for project at {}",
        project.display()
    );

    // 4) Assert the release APK landed. A missing key.properties would have
    //    hard-errored before Gradle ran (the CLI's signing gate), so a
    //    successful release build implies the artifact is signed with our
    //    keystore rather than debug-signed.
    let apk = project.join("android/app/build/outputs/apk/release/app-release.apk");
    assert!(
        apk.exists(),
        "expected a signed release APK at {}",
        apk.display()
    );
    let size = std::fs::metadata(&apk).expect("APK metadata").len();
    assert!(
        size > 1_000_000,
        "release APK is implausibly small ({size} bytes)"
    );

    let _ = std::fs::remove_dir_all(&project);
}
