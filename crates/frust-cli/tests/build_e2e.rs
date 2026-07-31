//! End-to-end Android build gate: `frust create` → generate a
//! throwaway upload keystore → `frust build apk --release` produces a
//! release-signed APK.
//!
//! Two shapes are covered, because the CLI's release-signing gate has to be
//! right about both: the scaffolded default (`android/key.properties`) and a
//! *relocated* rig whose Gradle reads prefixed keys out of
//! `android/app/keystores/key.properties`, declared to Frust through
//! `frust.toml`'s `[signing]` section. Both tests also assert the gate's
//! false-positive case first — a stub `key.properties` with no keystore
//! behind it must fail before Gradle runs, since with the generated Gradle
//! template that stub would otherwise yield a silently debug-signed release.
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

/// The distinguished name baked into this test's throwaway keystore. The
/// Android debug key is `CN=Android Debug, O=Android, C=US`, so finding this
/// DN in the produced APK's signer certificate is what separates "really
/// release-signed" from "debug-signed with a warning".
const E2E_DNAME: &str = "CN=Frust Build E2E, OU=Eng, O=Frust, C=US";
const E2E_CN: &str = "CN=Frust Build E2E";
const STORE_PASS: &str = "e2etest1";

/// Scaffolds a fresh app against this workspace's facade crate.
fn scaffold(project: &Path) {
    let create_status = Command::new(env!("CARGO_BIN_EXE_frust"))
        .args([
            "create",
            project.to_str().expect("project path is valid UTF-8"),
            "--project-name",
            "fk_build_e2e",
            "--org",
            "dev.f0x",
            "--frust-path",
            workspace_frust_path()
                .to_str()
                .expect("frust_path is valid UTF-8"),
        ])
        .status()
        .expect("failed to spawn `frust create`");
    assert!(create_status.success(), "`frust create` exited non-zero");
}

/// Generates a throwaway upload keystore at `at` (never leaves the scratch
/// dir).
fn generate_keystore(at: &Path) {
    std::fs::create_dir_all(at.parent().expect("keystore has a parent"))
        .expect("failed to create the keystore directory");
    let keytool_status = Command::new(find_keytool())
        .args([
            "-genkey",
            "-v",
            "-keystore",
            at.to_str().unwrap(),
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
            STORE_PASS,
            "-keypass",
            STORE_PASS,
            "-dname",
            E2E_DNAME,
        ])
        .status()
        .expect("failed to spawn `keytool`");
    assert!(
        keytool_status.success(),
        "`keytool -genkey` exited non-zero"
    );
    assert!(at.exists(), "keystore was not created");
}

/// Runs `frust build apk --release` in `project`, capturing output.
/// Single-ABI (arm64) keeps the wall-clock to one Rust target while still
/// exercising the full signing pipeline.
fn release_build(project: &Path, ndk_home: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_frust"))
        .args([
            "build",
            "apk",
            "--release",
            "--target-platform",
            "android-arm64",
        ])
        .current_dir(project)
        .env("ANDROID_NDK_HOME", ndk_home)
        .output()
        .expect("failed to spawn `frust build apk --release`")
}

/// The gate's false-positive case: a stub `android/key.properties` written
/// only to satisfy a file-existence check must fail the release build, and
/// must fail *before* Gradle runs — with the generated template that stub
/// produces a debug-signed release APK, which is exactly what the gate
/// exists to prevent.
fn assert_stub_key_properties_is_rejected(project: &Path, ndk_home: &str) {
    let stub = project.join("android/key.properties");
    std::fs::write(&stub, "keyAlias=upload\n").expect("failed to write the stub key.properties");

    let out = release_build(project, ndk_home);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "a stub key.properties produced a green release build:\n{stderr}"
    );
    assert!(
        stderr.contains("no usable signing material"),
        "expected the signing gate to reject the stub, got:\n{stderr}"
    );
    // The gate runs before `./gradlew`, so no build directory appeared.
    assert!(
        !project.join("android/app/build/outputs").exists(),
        "Gradle ran despite the signing gate rejecting the stub"
    );

    std::fs::remove_file(&stub).expect("failed to remove the stub key.properties");
}

/// Asserts a release APK landed and, when `apksigner` is available, that its
/// signer certificate is this test's keystore rather than the Android debug
/// key.
fn assert_release_apk(project: &Path) {
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

    let Some(apksigner) = find_apksigner() else {
        eprintln!(
            "note: no `apksigner` under $ANDROID_HOME/build-tools — skipping the \
             signer-certificate assertion"
        );
        return;
    };
    let out = Command::new(&apksigner)
        .args(["verify", "--print-certs", apk.to_str().unwrap()])
        .output()
        .expect("failed to spawn `apksigner`");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "`apksigner verify` failed:\n{printed}"
    );
    assert!(
        printed.contains(E2E_CN),
        "release APK is not signed with this test's keystore (debug-signed?):\n{printed}"
    );
    assert!(
        !printed.contains("CN=Android Debug"),
        "release APK carries the Android debug certificate:\n{printed}"
    );
}

/// Highest-versioned `apksigner` under `$ANDROID_HOME/build-tools`, if any.
fn find_apksigner() -> Option<PathBuf> {
    let android_home = std::env::var("ANDROID_HOME")
        .or_else(|_| std::env::var("ANDROID_SDK_ROOT"))
        .ok()?;
    let mut versions: Vec<PathBuf> =
        std::fs::read_dir(Path::new(&android_home).join("build-tools"))
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("apksigner").is_file())
            .collect();
    versions.sort();
    versions.pop().map(|p| p.join("apksigner"))
}

#[test]
#[ignore = "compiles a full generated project + Gradle; needs Android SDK/NDK — run with cargo test -p frust-cli --test build_e2e -- --ignored"]
fn scaffolded_project_produces_a_signed_release_apk() {
    let ndk_home = resolve_ndk_home().expect(
        "Android NDK not found: set ANDROID_NDK_HOME or install one under $ANDROID_HOME/ndk",
    );

    let project = unique_dir("proj");
    let _ = std::fs::remove_dir_all(&project);

    // 1) Scaffold a fresh app against the real facade crate.
    scaffold(&project);

    // 2) A stub key.properties must not buy a green (debug-signed) release.
    assert_stub_key_properties_is_rejected(&project, &ndk_home);

    // 3) Real signing material at the scaffolded default location.
    let keystore = project.join("upload-e2e.jks");
    generate_keystore(&keystore);
    std::fs::write(
        project.join("android/key.properties"),
        format!(
            "storePassword={STORE_PASS}\nkeyPassword={STORE_PASS}\nkeyAlias=upload\nstoreFile={}\n",
            keystore.display()
        ),
    )
    .expect("failed to write android/key.properties");

    // 4) Build, and assert the artifact really carries our certificate.
    let out = release_build(&project, &ndk_home);
    assert!(
        out.status.success(),
        "`frust build apk --release` failed for project at {}:\n{}",
        project.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_release_apk(&project);

    let _ = std::fs::remove_dir_all(&project);
}

/// The relocated-keystore rig: Gradle reads `prod.`-prefixed keys out of
/// `android/app/keystores/key.properties`, and `android/key.properties` never
/// exists. Before `[signing]` was parsed, this project could not pass the CLI
/// gate at all despite Gradle signing it correctly. The Gradle edit is made
/// here in the test — no external app repository is involved.
#[test]
#[ignore = "compiles a full generated project + Gradle; needs Android SDK/NDK — run with cargo test -p frust-cli --test build_e2e -- --ignored"]
fn relocated_keystore_project_passes_the_gate_without_android_key_properties() {
    let ndk_home = resolve_ndk_home().expect(
        "Android NDK not found: set ANDROID_NDK_HOME or install one under $ANDROID_HOME/ndk",
    );

    let project = unique_dir("relocated");
    let _ = std::fs::remove_dir_all(&project);

    scaffold(&project);

    // 1) Rewrite the generated Gradle into the relocated pattern: a
    //    properties file under `app/keystores/`, `prod.`-prefixed keys.
    let gradle_path = project.join("android/app/build.gradle.kts");
    let gradle = std::fs::read_to_string(&gradle_path).expect("reading generated build.gradle.kts");
    let mut relocated = gradle.replace(
        "rootProject.file(\"key.properties\")",
        "rootProject.file(\"app/keystores/key.properties\")",
    );
    for key in ["keyAlias", "keyPassword", "storeFile", "storePassword"] {
        relocated = relocated.replace(
            &format!("keystoreProperties[\"{key}\"]"),
            &format!("keystoreProperties[\"prod.{key}\"]"),
        );
    }
    assert!(
        relocated != gradle && relocated.contains("app/keystores/key.properties"),
        "the generated build.gradle.kts no longer matches the strings this test rewrites"
    );
    std::fs::write(&gradle_path, relocated).expect("writing the relocated build.gradle.kts");

    // 2) Declare the rig to Frust. Without this the gate looks at
    //    `android/key.properties`, which this project deliberately lacks.
    let toml_path = project.join("frust.toml");
    let mut frust_toml = std::fs::read_to_string(&toml_path).expect("reading frust.toml");
    frust_toml.push_str(
        "\n[signing]\nkey-properties = \"app/keystores/key.properties\"\nprefix = \"prod\"\n",
    );
    std::fs::write(&toml_path, frust_toml).expect("writing frust.toml");

    // 3) A stub at the legacy path is still not signing material.
    assert_stub_key_properties_is_rejected(&project, &ndk_home);

    // 4) Real material at the relocated path. `storeFile` is absolute so the
    //    generated Gradle's `rootProject.file(...)` and the CLI gate resolve
    //    the same file (relative-base resolution is unit-tested in
    //    `frust-drive`'s `android_build::signing`).
    let keystore = project.join("android/app/keystores/prod.jks");
    generate_keystore(&keystore);
    std::fs::write(
        project.join("android/app/keystores/key.properties"),
        format!(
            "develop.storeFile=/nonexistent/develop.jks\n\
             prod.storePassword={STORE_PASS}\nprod.keyPassword={STORE_PASS}\n\
             prod.keyAlias=upload\nprod.storeFile={}\n",
            keystore.display()
        ),
    )
    .expect("failed to write the relocated key.properties");
    assert!(
        !project.join("android/key.properties").exists(),
        "this test must pass the gate without a legacy android/key.properties"
    );

    let out = release_build(&project, &ndk_home);
    assert!(
        out.status.success(),
        "`frust build apk --release` failed for the relocated-keystore project at {}:\n{}",
        project.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_release_apk(&project);

    let _ = std::fs::remove_dir_all(&project);
}
