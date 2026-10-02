//! End-to-end Android build gate: `frust create` → generate a
//! throwaway upload keystore → `frust build apk --release` produces a
//! release-signed APK.
//!
//! **Every case asserts on the ARTIFACT** (`apksigner verify --print-certs`),
//! never on the gate's verdict alone. A green gate riding on a debug-signed
//! APK is the exact failure mode these tests exist to catch, and a release
//! gate shipped one precisely because the assertions stopped at the verdict.
//!
//! `[signing]` has three independent axes — properties **path**, key
//! **prefix**, env-var **names** — and each is a way for the CLI's resolution
//! and Gradle's to diverge. `frust build` now collapses all three by writing
//! what it resolved to `android/.frust-signing.properties` for Gradle to read
//! (see `frust-drive`'s `android_build::signing`), so one shape per axis is
//! covered here plus the default:
//!
//! | Test | Axis it pins |
//! |---|---|
//! | [`scaffolded_project_produces_a_signed_release_apk`] | the default rig, plus the two negative directions |
//! | [`prefix_only_signing_produces_a_release_signed_apk`] | **prefix** — default path, `prod.`-prefixed keys, stock Gradle |
//! | [`relocated_keystore_project_passes_the_gate_without_android_key_properties`] | **path** — and a project that edited the template's fallback lookups |
//! | [`env_only_ci_rig_produces_a_release_signed_apk_without_key_properties`] | **env names** — stock `frust.toml`, no `[signing.env]` block at all |
//! | [`an_old_template_that_ignores_the_generated_file_fails_the_release_build`] | the **precondition** that mechanism rests on, and the `external` waiver that must survive it |
//!
//! The default-rig test also covers both negative directions in one project
//! (reusing its warm cargo/Gradle caches): a stub `key.properties` must fail
//! the gate *before* Gradle runs, and a hand-run `./gradlew assembleRelease`
//! with no material at all must debug-sign with the template's warning — the
//! documented, no-Frust-promise fallback that the gate refuses to let a CLI
//! build reach.
//!
//! Each test also asserts `android/.frust-signing.properties` does **not**
//! exist once a build returns: it carries the keystore passwords in plaintext
//! and must not outlive the Gradle invocation that needed it.
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

use frust_drive::build_dirs::BuildLayout;

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

/// A `JAVA_HOME` for the one test that drives `./gradlew` itself (the CLI
/// resolves its own through the Android preflight). `$JAVA_HOME` first, else
/// Android Studio's bundled JBR — the same two candidates the preflight uses.
fn find_java_home() -> Option<String> {
    if let Ok(java_home) = std::env::var("JAVA_HOME")
        && Path::new(&java_home).join("bin/java").exists()
    {
        return Some(java_home);
    }
    Path::new(STUDIO_JBR_HOME)
        .join("bin/java")
        .exists()
        .then(|| STUDIO_JBR_HOME.to_string())
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
    release_build_with_env(project, ndk_home, &[])
}

/// [`release_build`] plus extra environment variables. They reach both halves
/// of the signing story in one shot: the CLI gate reads them directly, and
/// Gradle inherits them through the `./gradlew` child — which is exactly the
/// CI shape `[signing.env]` describes.
fn release_build_with_env(
    project: &Path,
    ndk_home: &str,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_frust"));
    command
        .args([
            "build",
            "apk",
            "--release",
            "--target-platform",
            "android-arm64",
        ])
        .current_dir(project)
        .env("ANDROID_NDK_HOME", ndk_home);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command
        .output()
        .expect("failed to spawn `frust build apk --release`")
}

/// Drives `./gradlew assembleRelease` directly, the way Android Studio (or a
/// CI job that never invokes Frust) would — the documented fallback path that
/// carries no Frust promise. The `-P` properties mirror what
/// `frust build apk --release --target-platform android-arm64` passes, so the
/// two share a cargo/Gradle cache instead of each compiling the Rust graph.
fn hand_run_gradle_release(project: &Path, ndk_home: &str) -> std::process::Output {
    let java_home = find_java_home()
        .expect("no JDK 17+ found: set JAVA_HOME (e.g. /usr/lib/jvm/java-17-openjdk)");
    Command::new("./gradlew")
        .args([
            "assembleRelease",
            "-Pfrust.targetPlatforms=arm64-v8a",
            "-Pfrust.splitPerAbi=false",
        ])
        .current_dir(project.join("android"))
        .env("JAVA_HOME", java_home)
        .env("ANDROID_NDK_HOME", ndk_home)
        .output()
        .expect("failed to spawn `./gradlew assembleRelease`")
}

/// The generated signing file holds the keystore passwords in plaintext and
/// must not survive the Gradle invocation that needed it — on any path.
fn assert_no_generated_signing_file(project: &Path) {
    let generated = project.join("android/.frust-signing.properties");
    assert!(
        !generated.exists(),
        "`{}` outlived the build; it carries plaintext keystore passwords",
        generated.display()
    );
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
        !project.join(gradle_outputs_dir()).exists(),
        "Gradle ran despite the signing gate rejecting the stub"
    );
    assert_no_generated_signing_file(project);

    std::fs::remove_file(&stub).expect("failed to remove the stub key.properties");
}

/// Reads the signer certificates `apksigner verify --print-certs` reports for
/// the release APK, or `None` when no `apksigner` is installed.
fn release_apk_certs(project: &Path) -> Option<String> {
    let apk = project.join(release_apk_path());
    assert!(apk.exists(), "expected a release APK at {}", apk.display());
    let size = std::fs::metadata(&apk).expect("APK metadata").len();
    assert!(
        size > 1_000_000,
        "release APK is implausibly small ({size} bytes)"
    );

    let apksigner = find_apksigner()?;
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
    Some(printed)
}

/// The template's documented no-Frust-promise fallback, exercised for real: a
/// hand-run Gradle release build with no signing material anywhere produces an
/// APK carrying the **Android debug** certificate, and says so in its output.
/// This is the artifact `frust build --release` must never let through, so
/// proving the framework still reaches it is what gives the gate's refusal
/// meaning.
fn assert_debug_signed_apk(project: &Path) {
    let Some(printed) = release_apk_certs(project) else {
        eprintln!(
            "note: no `apksigner` under $ANDROID_HOME/build-tools — skipping the \
             debug-certificate assertion"
        );
        return;
    };
    assert!(
        printed.contains("CN=Android Debug"),
        "an unsigned hand-run release build must carry the Android debug \
         certificate:\n{printed}"
    );
    assert!(
        !printed.contains(E2E_CN),
        "this APK should not carry the test keystore's certificate:\n{printed}"
    );
}

/// [`assert_debug_signed_apk`] with no escape hatch: a case whose *entire*
/// evidence is the certificate on the artifact has no meaningful
/// "`apksigner` was missing so we skipped it" outcome — a skipped assertion is
/// not evidence. Used by the cases that exist to prove what Gradle actually
/// signed, where a silent skip would turn the test into a no-op.
fn assert_debug_signed_apk_strict(project: &Path) {
    let printed = release_apk_certs(project).expect(
        "no `apksigner` under $ANDROID_HOME/build-tools — this case's whole point is which \
         certificate the artifact carries, so it cannot be skipped: install build-tools or \
         export ANDROID_HOME",
    );
    assert!(
        printed.contains("CN=Android Debug"),
        "expected the Android debug certificate on this artifact:\n{printed}"
    );
    assert!(
        !printed.contains(E2E_CN),
        "this APK should not carry the test keystore's certificate:\n{printed}"
    );
}

/// Asserts a release APK landed and, when `apksigner` is available, that its
/// signer certificate is this test's keystore rather than the Android debug
/// key. **This, not the CLI's exit status, is what every case must assert:**
/// the whole defect class is a green gate over a debug-signed artifact.
fn assert_release_apk(project: &Path) {
    let Some(printed) = release_apk_certs(project) else {
        eprintln!(
            "note: no `apksigner` under $ANDROID_HOME/build-tools — skipping the \
             signer-certificate assertion"
        );
        return;
    };
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

/// Derives the release APK output path from the new build layout.
/// The production resolver (frust-drive) is: `project_dir.join(BuildLayout::android_app()).join("outputs")`,
/// so the release APK is at `BuildLayout::android_app()/outputs/apk/release/app-release.apk`.
fn release_apk_path() -> PathBuf {
    BuildLayout::android_app().join("outputs/apk/release/app-release.apk")
}

/// Derives the Gradle outputs directory path from the new build layout.
/// This is the location where Gradle writes its build outputs, derived from BuildLayout::android_app().
/// The production resolver (frust-drive) is: `project_dir.join(BuildLayout::android_app()).join("outputs")`.
fn gradle_outputs_dir() -> PathBuf {
    BuildLayout::android_app().join("outputs")
}

/// Pins the contract between the test helpers and the production resolver.
/// The production resolver builds the path as: `project_dir.join(BuildLayout::android_app()).join("outputs")`.
/// These test helpers must match that shape exactly.
#[test]
fn build_layout_paths_match_the_drive_resolver() {
    assert_eq!(
        release_apk_path(),
        Path::new("build/android/app/outputs/apk/release/app-release.apk"),
        "release_apk_path() must match the production resolver's path"
    );
    assert_eq!(
        gradle_outputs_dir(),
        Path::new("build/android/app/outputs"),
        "gradle_outputs_dir() must match the production resolver's outputs directory"
    );
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

    // 3) The fallback the gate exists to keep a CLI build away from: a
    //    hand-run `./gradlew assembleRelease` with no signing material at all
    //    still succeeds, warns, and debug-signs. Run here (rather than in its
    //    own project) so its cargo/Gradle work warms the caches step 5 reuses.
    let gradle_out = hand_run_gradle_release(&project, &ndk_home);
    let gradle_printed = format!(
        "{}{}",
        String::from_utf8_lossy(&gradle_out.stdout),
        String::from_utf8_lossy(&gradle_out.stderr)
    );
    assert!(
        gradle_out.status.success(),
        "an unsigned hand-run `./gradlew assembleRelease` must still succeed:\n{gradle_printed}"
    );
    assert!(
        gradle_printed.contains("release build is debug-signed"),
        "expected the template's debug-signing warning:\n{gradle_printed}"
    );
    assert_debug_signed_apk(&project);
    assert_no_generated_signing_file(&project);

    // 4) Real signing material at the scaffolded default location.
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

    // 5) Build, and assert the artifact really carries our certificate — over
    //    the top of the debug-signed APK step 3 just produced, so a build that
    //    failed to re-sign would be caught rather than finding no APK at all.
    let out = release_build(&project, &ndk_home);
    assert!(
        out.status.success(),
        "`frust build apk --release` failed for project at {}:\n{}",
        project.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_release_apk(&project);
    assert_no_generated_signing_file(&project);

    let _ = std::fs::remove_dir_all(&project);
}

/// **The primary acceptance case.** `[signing] prefix = "prod"` against the
/// *default* properties path and a completely *unmodified* Gradle template —
/// the exact shape that once shipped broken: the gate resolved
/// `prod.storeFile` & co., passed with no warning, and Gradle (which has no
/// prefix concept and looks for a bare `storeFile`) found nothing and
/// debug-signed. Nothing about the template changed to fix it; the CLI now
/// resolves the prefix away and hands Gradle the result through
/// `android/.frust-signing.properties`.
///
/// The assertion is on the artifact's certificate, because the verdict is
/// exactly what passed while the bug shipped.
#[test]
#[ignore = "compiles a full generated project + Gradle; needs Android SDK/NDK — run with cargo test -p frust-cli --test build_e2e -- --ignored"]
fn prefix_only_signing_produces_a_release_signed_apk() {
    let ndk_home = resolve_ndk_home().expect(
        "Android NDK not found: set ANDROID_NDK_HOME or install one under $ANDROID_HOME/ndk",
    );

    let project = unique_dir("prefix-only");
    let _ = std::fs::remove_dir_all(&project);

    scaffold(&project);

    // 1) Declare only a prefix. The properties path stays the scaffolded
    //    default and `android/app/build.gradle.kts` is left exactly as
    //    generated — no `signingValue("prod.…")` rewrite anywhere.
    let toml_path = project.join("frust.toml");
    let mut frust_toml = std::fs::read_to_string(&toml_path).expect("reading frust.toml");
    frust_toml.push_str("\n[signing]\nprefix = \"prod\"\n");
    std::fs::write(&toml_path, frust_toml).expect("writing frust.toml");
    let gradle = std::fs::read_to_string(project.join("android/app/build.gradle.kts"))
        .expect("reading generated build.gradle.kts");
    assert!(
        !gradle.contains("prod."),
        "this test must run against the STOCK Gradle template — Gradle has no prefix concept, \
         and that is the whole point"
    );

    // 2) A multi-key-set properties file at the default path: `develop.` keys
    //    pointing nowhere alongside the real `prod.` ones.
    let keystore = project.join("prod-e2e.jks");
    generate_keystore(&keystore);
    std::fs::write(
        project.join("android/key.properties"),
        format!(
            "develop.storeFile=/nonexistent/develop.jks\ndevelop.storePassword=nope\n\
             prod.storePassword={STORE_PASS}\nprod.keyPassword={STORE_PASS}\n\
             prod.keyAlias=upload\nprod.storeFile={}\n",
            keystore.display()
        ),
    )
    .expect("failed to write the prefixed android/key.properties");

    let out = release_build(&project, &ndk_home);
    assert!(
        out.status.success(),
        "`frust build apk --release` failed for the prefix-only project at {}:\n{}",
        project.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_release_apk(&project);
    assert_no_generated_signing_file(&project);

    let _ = std::fs::remove_dir_all(&project);
}

/// The relocated-keystore rig: the project's own Gradle reads `prod.`-prefixed
/// keys out of `android/app/keystores/key.properties`, and
/// `android/key.properties` never exists. Before `[signing]` was parsed, this
/// project could not pass the CLI gate at all despite Gradle signing it
/// correctly. The Gradle edit is made here in the test — no external app
/// repository is involved.
///
/// It doubles as the guard on constraint the generated-file design rests on:
/// editing the template's **fallback** lookups (which is what a relocated rig
/// does) must not disturb the `frustSigning(...)` read that sits ahead of
/// them. The artifact must still carry this test's certificate.
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
            &format!("signingValue(\"{key}\""),
            &format!("signingValue(\"prod.{key}\""),
        );
    }
    assert!(
        relocated.contains("app/keystores/key.properties")
            && relocated.contains("signingValue(\"prod.storeFile\"")
            && relocated.contains("signingValue(\"prod.keyAlias\""),
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
    assert_no_generated_signing_file(&project);
    // No advisory. The gate used to warn "this release may be debug-signed"
    // whenever it resolved material the *stock* template could not follow;
    // that caveat is now false for every build that could ever read it — only
    // `frust build`/`frust run` see `on_line`, and those are exactly the
    // builds whose Gradle is handed the resolved values.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("may be debug-signed"),
        "the Gradle-divergence advisory is dead weight now that Gradle reads what the gate \
         resolved; it must not fire:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&project);
}

/// The CI rig with a **completely stock `frust.toml`**: no
/// `android/key.properties` anywhere, no `[signing]` section at all, just the
/// four `ANDROID_*` variables exported. Round 1 found the gate hard-failing
/// this shape — it only reached the environment through an explicit
/// `[signing.env]` block, so a CI job doing the documented thing got
/// "release build has no usable signing material" for a build Gradle would
/// have signed. The previous version of this test had to append a
/// `[signing.env]` block to work, which documented the gap instead of closing
/// it; the four `ANDROID_*` names are now the gate's defaults, matching the
/// template's own fallback names.
///
/// The assertion that matters is on the **artifact**, not the gate's verdict:
/// `apksigner verify --print-certs` must report this test's certificate.
#[test]
#[ignore = "compiles a full generated project + Gradle; needs Android SDK/NDK — run with cargo test -p frust-cli --test build_e2e -- --ignored"]
fn env_only_ci_rig_produces_a_release_signed_apk_without_key_properties() {
    let ndk_home = resolve_ndk_home().expect(
        "Android NDK not found: set ANDROID_NDK_HOME or install one under $ANDROID_HOME/ndk",
    );

    let project = unique_dir("env-ci");
    let _ = std::fs::remove_dir_all(&project);

    scaffold(&project);

    // 1) Nothing declared. The scaffolded frust.toml ships every `[signing]`
    //    line commented out, and this test must not touch it.
    let frust_toml =
        std::fs::read_to_string(project.join("frust.toml")).expect("reading frust.toml");
    assert!(
        !frust_toml.contains("\n[signing.env]"),
        "this test must run against a stock frust.toml with no [signing.env] block"
    );

    // 2) A keystore, and nothing on disk pointing at it. `storeFile` is
    //    absolute so the gate's candidate bases and Gradle's
    //    `rootProject.file(...)` land on the same file either way.
    let keystore = project.join("ci-upload.jks");
    generate_keystore(&keystore);
    assert!(
        !project.join("android/key.properties").exists(),
        "this test must sign with no key.properties at all"
    );

    // 3) Build with the four variables exported, CI-style.
    let store_file = keystore.to_str().expect("keystore path is valid UTF-8");
    let out = release_build_with_env(
        &project,
        &ndk_home,
        &[
            ("ANDROID_STORE_FILE", store_file),
            ("ANDROID_STORE_PASSWORD", STORE_PASS),
            ("ANDROID_KEY_ALIAS", "upload"),
            ("ANDROID_KEY_PASSWORD", STORE_PASS),
        ],
    );
    assert!(
        out.status.success(),
        "`frust build apk --release` failed for the env-only project at {}:\n{}",
        project.display(),
        String::from_utf8_lossy(&out.stderr)
    );

    // 4) The whole point: the artifact carries this test's certificate, not
    //    the Android debug key.
    assert_release_apk(&project);
    assert_no_generated_signing_file(&project);

    let _ = std::fs::remove_dir_all(&project);
}

/// **The installed-base route — the precondition the one-source-of-truth
/// mechanism cannot check.** `frust build` hands Gradle
/// `android/.frust-signing.properties`, but only a project whose
/// `build.gradle.kts` contains the `frustSigning(...)` read ever looks at it.
/// There is no `frust upgrade`/regenerate command, so every project scaffolded
/// by an earlier CLI has no such read — it follows the newly-shipped
/// `frust.toml` guidance, adds `[signing] prefix = "prod"`, and Gradle's
/// template falls through to `signingConfigs.getByName("debug")`, warns, and
/// **exits 0**. Keying off `out.success` alone reported that as a successful
/// release build over a debug-signed APK.
///
/// This test ages a freshly scaffolded project back to that shape by stripping
/// the four `frustSigning(...)` reads, then asserts three things in order:
/// the CLI **fails**, its message names the cause, and — the assertion that
/// actually matters — the APK Gradle left behind really does carry
/// `CN=Android Debug`. That last one is what makes the failure meaningful
/// rather than incidental: it proves the route reaches the bad artifact and
/// that the refusal is the only thing standing between a user and shipping it.
///
/// It then reuses the same (warm-cache) project for the `external = true`
/// waiver, which is the sharpest possible test of that path: `external` is a
/// declared bypass over exactly this situation — Gradle emits the very marker
/// that just failed the build — and it must still succeed, with its warning.
#[test]
#[ignore = "compiles a full generated project + Gradle; needs Android SDK/NDK — run with cargo test -p frust-cli --test build_e2e -- --ignored"]
fn an_old_template_that_ignores_the_generated_file_fails_the_release_build() {
    let ndk_home = resolve_ndk_home().expect(
        "Android NDK not found: set ANDROID_NDK_HOME or install one under $ANDROID_HOME/ndk",
    );

    let project = unique_dir("old-template");
    let _ = std::fs::remove_dir_all(&project);

    scaffold(&project);

    // 1) Age the generated Gradle back to a pre-`.frust-signing.properties`
    //    project: strip the four `frustSigning(...) ?:` reads so only the
    //    `key.properties` + `ANDROID_*` fallback is left — which is precisely
    //    what an installed-base project's signing block looks like. (The now
    //    unused `frustSigning` helper stays; what matters is that no resolved
    //    value comes from it.)
    let gradle_path = project.join("android/app/build.gradle.kts");
    let gradle = std::fs::read_to_string(&gradle_path).expect("reading generated build.gradle.kts");
    assert_eq!(
        gradle.matches("frustSigning(\"").count(),
        4,
        "the generated build.gradle.kts no longer has the four reads this test strips"
    );
    let mut aged = gradle;
    for key in ["storeFile", "storePassword", "keyAlias", "keyPassword"] {
        aged = aged.replace(&format!("frustSigning(\"{key}\") ?: "), "");
    }
    assert_eq!(
        aged.matches("frustSigning(\"").count(),
        0,
        "the four reads must be gone — that is the whole simulation"
    );
    std::fs::write(&gradle_path, aged).expect("writing the aged build.gradle.kts");

    // 2) The configuration `frust.toml.tmpl` documents, verbatim: a prefix, and
    //    real material behind it. The gate resolves all four and passes.
    let stock_toml =
        std::fs::read_to_string(project.join("frust.toml")).expect("reading frust.toml");
    std::fs::write(
        project.join("frust.toml"),
        format!("{stock_toml}\n[signing]\nprefix = \"prod\"\n"),
    )
    .expect("writing frust.toml");
    let keystore = project.join("prod-e2e.jks");
    generate_keystore(&keystore);
    std::fs::write(
        project.join("android/key.properties"),
        format!(
            "prod.storePassword={STORE_PASS}\nprod.keyPassword={STORE_PASS}\n\
             prod.keyAlias=upload\nprod.storeFile={}\n",
            keystore.display()
        ),
    )
    .expect("failed to write the prefixed android/key.properties");

    // 3) The build must fail. Before this backstop it exited 0 over an APK
    //    carrying `CN=Android Debug` — so if it succeeds here, report which
    //    certificate the CLI just blessed rather than a bare "expected failure"
    //    (this is also the message the negative control reads).
    let out = release_build(&project, &ndk_home);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() {
        let certs = release_apk_certs(&project)
            .unwrap_or_else(|| "<no apksigner available to name it>".to_string());
        panic!(
            "`frust build apk --release` reported success for a project whose Gradle debug-signs. \
             The artifact it blessed:\n{certs}"
        );
    }
    assert!(
        stderr.contains("Gradle debug-signed"),
        "expected the debug-signed refusal, got:\n{stderr}"
    );
    assert!(
        stderr.contains("build.gradle.kts"),
        "the refusal must name the file to fix:\n{stderr}"
    );
    // The artifact assertion, not the verdict: Gradle really did produce a
    // debug-signed release APK, and the CLI is the only thing that caught it.
    assert_debug_signed_apk_strict(&project);
    assert_no_generated_signing_file(&project);

    // 4) The `external = true` waiver over the identical situation: Gradle
    //    still emits the marker, and the build must still succeed — with the
    //    warning that says Frust promises nothing about the signature.
    std::fs::write(
        project.join("frust.toml"),
        format!("{stock_toml}\n[signing]\nexternal = true\n"),
    )
    .expect("writing frust.toml");
    let out = release_build(&project, &ndk_home);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "`[signing] external = true` is a declared bypass and must not hard-fail:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("external = true") && stdout.contains("cannot promise"),
        "the waiver must warn on every release build:\n{stdout}"
    );
    assert!(
        stdout.contains("release build is debug-signed"),
        "this case is only meaningful if Gradle emitted the marker the waiver survives:\n{stdout}"
    );
    // And the waiver really does let a debug-signed artifact through — that is
    // what "Frust cannot promise this artifact is release-signed" means.
    assert_debug_signed_apk_strict(&project);
    assert_no_generated_signing_file(&project);

    let _ = std::fs::remove_dir_all(&project);
}
